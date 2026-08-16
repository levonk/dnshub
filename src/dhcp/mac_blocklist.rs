//! MAC blocklist — refuse DHCP leases to specific devices.
//!
//! The [`MacBlocklist`] is backed by the `dhcp_mac_blocklist` SQLite table
//! (PRD lines 733-738). Each row stores either a full MAC address
//! (`"00:11:22:33:44:55"`) or an OUI vendor prefix (`"00:11:22"`), plus an
//! optional human-readable reason.
//!
//! When a DHCP request arrives, the server calls [`MacBlocklist::is_blocked`]
//! with the client's 6-byte MAC. The check tests the full MAC string first,
//! then the OUI prefix string — both are stored as lowercase colon-separated
//! hex so lookups are exact string matches against the primary key.
//!
//! This implements PRD section 4.4 (lines 394-396):
//! - block by full MAC address
//! - block by OUI vendor prefix

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::time::{SystemTime, UNIX_EPOCH};

/// A single blocklist row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacBlockEntry {
    /// The blocked MAC (`"00:11:22:33:44:55"`) or OUI (`"00:11:22"`),
    /// lowercase and colon-separated.
    pub mac_or_oui: String,
    /// Optional reason for the block.
    pub reason: Option<String>,
    /// Unix timestamp (seconds) when the block was created.
    pub created_at: i64,
}

/// Errors returned by [`MacBlocklist`] operations.
#[derive(Debug)]
pub enum MacBlocklistError {
    /// A SQLite operation failed.
    Sqlite(rusqlite::Error),
    /// The supplied MAC/OUI string could not be parsed.
    InvalidMac(String),
}

impl std::fmt::Display for MacBlocklistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacBlocklistError::Sqlite(e) => write!(f, "mac blocklist sqlite error: {e}"),
            MacBlocklistError::InvalidMac(s) => write!(f, "invalid mac or oui: {s}"),
        }
    }
}

impl std::error::Error for MacBlocklistError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MacBlocklistError::Sqlite(e) => Some(e),
            MacBlocklistError::InvalidMac(_) => None,
        }
    }
}

impl From<rusqlite::Error> for MacBlocklistError {
    fn from(e: rusqlite::Error) -> Self {
        MacBlocklistError::Sqlite(e)
    }
}

/// SQLite-backed MAC blocklist.
///
/// Wraps a [`rusqlite::Connection`] in a [`parking_lot::Mutex`] so the
/// blocklist can be shared across async tasks. The connection is expected to
/// be dedicated to the blocklist (it is not shared with the lease store).
pub struct MacBlocklist {
    conn: Mutex<Connection>,
}

impl MacBlocklist {
    /// Open (or create) the blocklist database at `path` and ensure the
    /// `dhcp_mac_blocklist` schema exists.
    pub fn open(path: &str) -> Result<Self, MacBlocklistError> {
        let conn = Connection::open(path)?;
        let blocklist = Self::with_connection(conn);
        blocklist.init_schema()?;
        Ok(blocklist)
    }

    /// Wrap an existing [`Connection`] (e.g. an in-memory database for
    /// tests). The caller is responsible for the schema — call
    /// [`init_schema`](Self::init_schema) afterwards.
    pub fn with_connection(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// Create the `dhcp_mac_blocklist` table if it does not already exist.
    pub fn init_schema(&self) -> Result<(), MacBlocklistError> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dhcp_mac_blocklist (
                mac_or_oui    TEXT PRIMARY KEY,
                reason        TEXT,
                created_at    INTEGER NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Returns `true` if `mac` is blocked — either by its full MAC address
    /// or by its OUI vendor prefix (first 3 octets).
    ///
    /// The full MAC is checked first; if present the OUI check is skipped.
    pub fn is_blocked(&self, mac: &[u8; 6]) -> bool {
        let conn = self.conn.lock();
        let full = format_mac(mac);
        let oui = format_oui(&[mac[0], mac[1], mac[2]]);
        // Check the full MAC first, then the OUI prefix.
        let blocked: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM dhcp_mac_blocklist WHERE mac_or_oui = ?1 LIMIT 1",
                params![full],
                |_| Ok(1),
            )
            .optional()
            .ok()
            .flatten();
        if blocked.is_some() {
            return true;
        }
        let blocked: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM dhcp_mac_blocklist WHERE mac_or_oui = ?1 LIMIT 1",
                params![oui],
                |_| Ok(1),
            )
            .optional()
            .ok()
            .flatten();
        blocked.is_some()
    }

    /// Add a block entry for a full MAC or OUI string.
    ///
    /// `mac_or_oui` may be a full MAC (`"00:11:22:33:44:55"`) or an OUI
    /// (`"00:11:22"`), with any separator (`:` or `-`) and any case. It is
    /// normalised to lowercase colon-separated form before storage.
    /// Inserting a duplicate key updates the reason and timestamp.
    pub fn add_mac_block(
        &self,
        mac_or_oui: &str,
        reason: Option<&str>,
    ) -> Result<(), MacBlocklistError> {
        let normalised = normalise_mac_or_oui(mac_or_oui)
            .ok_or_else(|| MacBlocklistError::InvalidMac(mac_or_oui.to_string()))?;
        let now = unix_now();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO dhcp_mac_blocklist (mac_or_oui, reason, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(mac_or_oui) DO UPDATE SET
                reason = excluded.reason,
                created_at = excluded.created_at",
            params![normalised, reason, now],
        )?;
        Ok(())
    }

    /// Remove a block entry. `mac_or_oui` is normalised the same way as in
    /// [`add_mac_block`](Self::add_mac_block).
    pub fn remove_mac_block(&self, mac_or_oui: &str) -> Result<bool, MacBlocklistError> {
        let normalised = normalise_mac_or_oui(mac_or_oui)
            .ok_or_else(|| MacBlocklistError::InvalidMac(mac_or_oui.to_string()))?;
        let conn = self.conn.lock();
        let removed = conn.execute(
            "DELETE FROM dhcp_mac_blocklist WHERE mac_or_oui = ?1",
            params![normalised],
        )?;
        Ok(removed > 0)
    }

    /// List all block entries, ordered by creation time (oldest first).
    pub fn list_mac_blocks(&self) -> Result<Vec<MacBlockEntry>, MacBlocklistError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT mac_or_oui, reason, created_at
             FROM dhcp_mac_blocklist
             ORDER BY created_at ASC, mac_or_oui ASC",
        )?;
        let entries = stmt.query_map([], |row| {
            Ok(MacBlockEntry {
                mac_or_oui: row.get(0)?,
                reason: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for entry in entries {
            out.push(entry?);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a 6-byte MAC as a lowercase colon-separated string.
fn format_mac(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// Format a 3-byte OUI as a lowercase colon-separated string.
fn format_oui(oui: &[u8; 3]) -> String {
    format!("{:02x}:{:02x}:{:02x}", oui[0], oui[1], oui[2])
}

/// Normalise a user-supplied MAC or OUI string to lowercase colon-separated
/// hex. Accepts `:` or `-` separators and any case. Returns `None` if the
/// input does not contain exactly 3 (OUI) or 6 (MAC) valid hex octets.
fn normalise_mac_or_oui(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    match parts.len() {
        3 | 6 => {
            let mut octets = Vec::with_capacity(parts.len());
            for part in parts {
                octets.push(u8::from_str_radix(part.trim(), 16).ok()?);
            }
            Some(
                octets
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":"),
            )
        }
        _ => None,
    }
}

/// Current unix timestamp in seconds.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blocklist() -> MacBlocklist {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let bl = MacBlocklist::with_connection(conn);
        bl.init_schema().expect("init schema");
        bl
    }

    #[test]
    fn block_by_full_mac() {
        let bl = make_blocklist();
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        assert!(!bl.is_blocked(&mac));

        bl.add_mac_block("00:11:22:33:44:55", Some("banned device"))
            .expect("add block");
        assert!(bl.is_blocked(&mac));
    }

    #[test]
    fn block_by_oui_prefix() {
        let bl = make_blocklist();
        bl.add_mac_block("00:11:22", Some("known-bad IoT vendor"))
            .expect("add oui block");

        // Any MAC with that OUI is blocked.
        let mac_a = [0x00, 0x11, 0x22, 0xaa, 0xbb, 0xcc];
        let mac_b = [0x00, 0x11, 0x22, 0x01, 0x02, 0x03];
        assert!(bl.is_blocked(&mac_a));
        assert!(bl.is_blocked(&mac_b));

        // Different OUI is not blocked.
        let other = [0xaa, 0xbb, 0xcc, 0x33, 0x44, 0x55];
        assert!(!bl.is_blocked(&other));
    }

    #[test]
    fn full_mac_takes_precedence_over_oui() {
        // If both a full MAC and its OUI are blocked, the full-MAC row is
        // found first and the OUI row is never consulted.
        let bl = make_blocklist();
        bl.add_mac_block("00:11:22:33:44:55", Some("specific device"))
            .expect("add full mac");
        bl.add_mac_block("00:11:22", Some("vendor"))
            .expect("add oui");
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        assert!(bl.is_blocked(&mac));
    }

    #[test]
    fn unblock_removes_entry() {
        let bl = make_blocklist();
        bl.add_mac_block("00:11:22:33:44:55", Some("temp"))
            .expect("add");
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        assert!(bl.is_blocked(&mac));

        let removed = bl.remove_mac_block("00:11:22:33:44:55").expect("remove");
        assert!(removed);
        assert!(!bl.is_blocked(&mac));

        // Removing a non-existent entry returns false.
        let removed_again = bl.remove_mac_block("00:11:22:33:44:55").expect("remove");
        assert!(!removed_again);
    }

    #[test]
    fn list_mac_blocks() {
        let bl = make_blocklist();
        bl.add_mac_block("00:11:22", Some("vendor")).expect("add");
        bl.add_mac_block("aa:bb:cc:dd:ee:ff", Some("device")).expect("add");

        let entries = bl.list_mac_blocks().expect("list");
        assert_eq!(entries.len(), 2);

        let ouis: Vec<&str> = entries.iter().map(|e| e.mac_or_oui.as_str()).collect();
        assert!(ouis.contains(&"00:11:22"));
        assert!(ouis.contains(&"aa:bb:cc:dd:ee:ff"));

        // Reasons are preserved.
        let vendor_entry = entries.iter().find(|e| e.mac_or_oui == "00:11:22").unwrap();
        assert_eq!(vendor_entry.reason.as_deref(), Some("vendor"));
    }

    #[test]
    fn normalisation_accepts_various_formats() {
        let bl = make_blocklist();
        // Uppercase, hyphen-separated.
        bl.add_mac_block("AA-BB-CC-DD-EE-FF", Some("upper-hyphen"))
            .expect("add");
        let mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        assert!(bl.is_blocked(&mac));

        // Mixed case, colon-separated OUI.
        bl.add_mac_block("B8:27:eB", Some("rpi")).expect("add");
        let rpi = [0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56];
        assert!(bl.is_blocked(&rpi));

        // Stored form is lowercase colon-separated.
        let entries = bl.list_mac_blocks().expect("list");
        assert!(entries.iter().any(|e| e.mac_or_oui == "aa:bb:cc:dd:ee:ff"));
        assert!(entries.iter().any(|e| e.mac_or_oui == "b8:27:eb"));
    }

    #[test]
    fn add_rejects_invalid_input() {
        let bl = make_blocklist();
        assert!(bl.add_mac_block("not-a-mac", None).is_err());
        assert!(bl.add_mac_block("00:11", None).is_err());
        assert!(bl.add_mac_block("ZZ:ZZ:ZZ:ZZ:ZZ:ZZ", None).is_err());
    }

    #[test]
    fn duplicate_add_updates_reason() {
        let bl = make_blocklist();
        bl.add_mac_block("00:11:22:33:44:55", Some("first")).expect("add");
        bl.add_mac_block("00:11:22:33:44:55", Some("second")).expect("update");

        let entries = bl.list_mac_blocks().expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].reason.as_deref(), Some("second"));
    }
}
