//! DHCPv4 lease persistence: `LeaseStoreV4` trait + `SqliteLeaseStoreV4`.
//!
//! The lease store is the authoritative record of which IP address is
//! assigned to which client (identified by MAC address). It backs the
//! DHCP state machine: the pool allocator consults it to find free
//! addresses, and the state machine updates it on OFFER / ACK / RELEASE /
//! DECLINE.
//!
//! The [`LeaseStoreV4`] trait is `async_trait`-based so that future
//! backends (PostgreSQL, etcd — see PRD section 4.4 storage options) can
//! drop in without changing call sites. The default backend,
//! [`SqliteLeaseStoreV4`], uses `rusqlite` with WAL mode for safe
//! concurrent reads and serialized writes.
//!
//! ## Schema
//!
//! The SQLite schema mirrors the PRD (lines 693-711):
//!
//! - `dhcp_leases` — dynamic leases keyed by IP address.
//! - `dhcp_static_leases` — static MAC→IP bindings keyed by MAC address.

use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};

/// Lifecycle state of a DHCPv4 lease.
///
/// Mirrors the `lease_state` column in the `dhcp_leases` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeaseState {
    /// Server has sent DHCPOFFER but not yet received DHCPREQUEST.
    Offered,
    /// Lease is active (DHCPACK sent, not expired).
    Active,
    /// Lease expired without renewal.
    Expired,
    /// Client sent DHCPRELEASE.
    Released,
    /// Client sent DHCPDECLINE — IP is marked conflicted.
    Declined,
}

impl LeaseState {
    /// String representation stored in the `lease_state` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            LeaseState::Offered => "offered",
            LeaseState::Active => "active",
            LeaseState::Expired => "expired",
            LeaseState::Released => "released",
            LeaseState::Declined => "declined",
        }
    }

    /// Parse a `lease_state` column value back into a [`LeaseState`].
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "offered" => Some(LeaseState::Offered),
            "active" => Some(LeaseState::Active),
            "expired" => Some(LeaseState::Expired),
            "released" => Some(LeaseState::Released),
            "declined" => Some(LeaseState::Declined),
            _ => None,
        }
    }
}

/// A single DHCPv4 lease record (one row in `dhcp_leases`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseV4 {
    /// Assigned IP address (primary key).
    pub ip: Ipv4Addr,
    /// Client MAC address, e.g. `"00:11:22:33:44:55"`.
    pub mac: String,
    /// Client hostname (DHCP option 12), if provided.
    pub hostname: Option<String>,
    /// Client identifier (DHCP option 61), if provided.
    pub client_id: Option<String>,
    /// Vendor class (DHCP option 60), if provided.
    pub vendor_class: Option<String>,
    /// Assigned policy profile, if any.
    pub profile: Option<String>,
    /// Unix timestamp at which the lease expires.
    pub lease_expires: i64,
    /// Current lifecycle state.
    pub lease_state: LeaseState,
    /// Unix timestamp when the lease record was created.
    pub created_at: i64,
    /// Unix timestamp when the lease record was last updated.
    pub updated_at: i64,
}

/// A static lease record (one row in `dhcp_static_leases`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticLeaseV4Record {
    /// Client MAC address (primary key).
    pub mac: String,
    /// Fixed IP address.
    pub ip: Ipv4Addr,
    /// Optional hostname.
    pub hostname: Option<String>,
    /// Forced policy profile.
    pub profile: Option<String>,
}

/// Errors returned by lease store operations.
#[derive(Debug)]
pub enum LeaseStoreError {
    /// SQLite error.
    Sqlite(rusqlite::Error),
    /// A lease record was not found.
    NotFound,
    /// A lease for this MAC already exists with a different IP.
    MacConflict,
    /// Failed to parse a stored value (corrupt row).
    Parse(String),
}

impl std::fmt::Display for LeaseStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaseStoreError::Sqlite(e) => write!(f, "lease store sqlite error: {e}"),
            LeaseStoreError::NotFound => write!(f, "lease not found"),
            LeaseStoreError::MacConflict => write!(f, "mac already has a different lease"),
            LeaseStoreError::Parse(s) => write!(f, "lease store parse error: {s}"),
        }
    }
}

impl std::error::Error for LeaseStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LeaseStoreError::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for LeaseStoreError {
    fn from(e: rusqlite::Error) -> Self {
        LeaseStoreError::Sqlite(e)
    }
}

/// Async trait for DHCPv4 lease storage backends.
///
/// The default implementation is [`SqliteLeaseStoreV4`]. The trait is
/// designed so a PostgreSQL or etcd backend (PRD section 4.4) can be
/// added later without changing the state machine or pool allocator.
#[async_trait]
pub trait LeaseStoreV4: Send + Sync {
    /// Get the lease for `ip`, if any.
    async fn get_lease(&self, ip: Ipv4Addr) -> Result<Option<LeaseV4>, LeaseStoreError>;

    /// Get the lease for `mac`, if any.
    async fn get_lease_by_mac(
        &self,
        mac: &str,
    ) -> Result<Option<LeaseV4>, LeaseStoreError>;

    /// Insert a new lease. Fails if a lease for the IP already exists.
    async fn insert_lease(&self, lease: &LeaseV4) -> Result<(), LeaseStoreError>;

    /// Update an existing lease (matched by IP).
    async fn update_lease(&self, lease: &LeaseV4) -> Result<(), LeaseStoreError>;

    /// Delete the lease for `ip`.
    async fn delete_lease(&self, ip: Ipv4Addr) -> Result<(), LeaseStoreError>;

    /// List all leases.
    async fn list_leases(&self) -> Result<Vec<LeaseV4>, LeaseStoreError>;

    /// Find a free IP in the range `[start, end]` that has no active or
    /// offered lease. Returns the first free IP, or `None` if the range
    /// is exhausted.
    async fn find_free_ip(
        &self,
        start: Ipv4Addr,
        end: Ipv4Addr,
    ) -> Result<Option<Ipv4Addr>, LeaseStoreError>;

    /// Get the static lease for `mac`, if any.
    async fn get_static_lease(
        &self,
        mac: &str,
    ) -> Result<Option<StaticLeaseV4Record>, LeaseStoreError>;

    /// Insert or update a static lease (matched by MAC).
    async fn upsert_static_lease(
        &self,
        lease: &StaticLeaseV4Record,
    ) -> Result<(), LeaseStoreError>;

    /// Mark `ip` as conflicted (DECLINE). Sets the lease state to
    /// `Declined` so the pool allocator skips it.
    async fn mark_conflicted(&self, ip: Ipv4Addr) -> Result<(), LeaseStoreError>;
}

/// SQLite-backed [`LeaseStoreV4`].
///
/// The connection is wrapped in a [`Mutex`] to serialize writes (the
/// story risk note recommends a single writer). WAL mode is enabled for
/// concurrent readers. The schema is created on open if it does not
/// already exist.
pub struct SqliteLeaseStoreV4 {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteLeaseStoreV4 {
    /// Open (or create) a SQLite lease store at `path`.
    ///
    /// Enables WAL mode and creates the `dhcp_leases` and
    /// `dhcp_static_leases` tables if they do not exist.
    pub fn open(path: &Path) -> Result<Self, LeaseStoreError> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory lease store (for tests).
    pub fn open_in_memory() -> Result<Self, LeaseStoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init(conn: &Connection) -> Result<(), LeaseStoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dhcp_leases (
                ip_address   TEXT PRIMARY KEY,
                mac_address  TEXT NOT NULL,
                hostname     TEXT,
                client_id    TEXT,
                vendor_class TEXT,
                profile      TEXT,
                lease_expires INTEGER NOT NULL,
                lease_state  TEXT NOT NULL,
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS dhcp_static_leases (
                mac_address  TEXT PRIMARY KEY,
                ip_address   TEXT NOT NULL,
                hostname     TEXT,
                profile      TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_dhcp_leases_mac ON dhcp_leases(mac_address);
            CREATE INDEX IF NOT EXISTS idx_dhcp_leases_state ON dhcp_leases(lease_state);",
        )?;
        Ok(())
    }

    fn row_to_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<LeaseV4> {
        let ip_str: String = row.get("ip_address")?;
        let ip: Ipv4Addr = ip_str
            .parse()
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(e),
            ))?;
        let mac: String = row.get("mac_address")?;
        let hostname: Option<String> = row.get("hostname")?;
        let client_id: Option<String> = row.get("client_id")?;
        let vendor_class: Option<String> = row.get("vendor_class")?;
        let profile: Option<String> = row.get("profile")?;
        let lease_expires: i64 = row.get("lease_expires")?;
        let state_str: String = row.get("lease_state")?;
        let created_at: i64 = row.get("created_at")?;
        let updated_at: i64 = row.get("updated_at")?;
        let lease_state = LeaseState::from_str(&state_str).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown lease_state '{state_str}'"),
                )),
            )
        })?;
        Ok(LeaseV4 {
            ip,
            mac,
            hostname,
            client_id,
            vendor_class,
            profile,
            lease_expires,
            lease_state,
            created_at,
            updated_at,
        })
    }

    fn row_to_static(row: &rusqlite::Row<'_>) -> rusqlite::Result<StaticLeaseV4Record> {
        let mac: String = row.get("mac_address")?;
        let ip_str: String = row.get("ip_address")?;
        let ip: Ipv4Addr = ip_str
            .parse()
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(e),
            ))?;
        let hostname: Option<String> = row.get("hostname")?;
        let profile: Option<String> = row.get("profile")?;
        Ok(StaticLeaseV4Record {
            mac,
            ip,
            hostname,
            profile,
        })
    }
}

#[async_trait]
impl LeaseStoreV4 for SqliteLeaseStoreV4 {
    async fn get_lease(&self, ip: Ipv4Addr) -> Result<Option<LeaseV4>, LeaseStoreError> {
        let conn = self.conn.lock();
        let lease = conn
            .query_row(
                "SELECT * FROM dhcp_leases WHERE ip_address = ?1",
                params![ip.to_string()],
                Self::row_to_lease,
            )
            .optional()?;
        Ok(lease)
    }

    async fn get_lease_by_mac(
        &self,
        mac: &str,
    ) -> Result<Option<LeaseV4>, LeaseStoreError> {
        let conn = self.conn.lock();
        let lease = conn
            .query_row(
                "SELECT * FROM dhcp_leases WHERE mac_address = ?1 \
                 ORDER BY updated_at DESC LIMIT 1",
                params![mac],
                Self::row_to_lease,
            )
            .optional()?;
        Ok(lease)
    }

    async fn insert_lease(&self, lease: &LeaseV4) -> Result<(), LeaseStoreError> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO dhcp_leases \
             (ip_address, mac_address, hostname, client_id, vendor_class, \
              profile, lease_expires, lease_state, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                lease.ip.to_string(),
                lease.mac,
                lease.hostname,
                lease.client_id,
                lease.vendor_class,
                lease.profile,
                lease.lease_expires,
                lease.lease_state.as_str(),
                lease.created_at,
                lease.updated_at,
            ],
        )?;
        Ok(())
    }

    async fn update_lease(&self, lease: &LeaseV4) -> Result<(), LeaseStoreError> {
        let conn = self.conn.lock();
        let affected = conn.execute(
            "UPDATE dhcp_leases SET \
             mac_address = ?2, hostname = ?3, client_id = ?4, vendor_class = ?5, \
             profile = ?6, lease_expires = ?7, lease_state = ?8, updated_at = ?9 \
             WHERE ip_address = ?1",
            params![
                lease.ip.to_string(),
                lease.mac,
                lease.hostname,
                lease.client_id,
                lease.vendor_class,
                lease.profile,
                lease.lease_expires,
                lease.lease_state.as_str(),
                lease.updated_at,
            ],
        )?;
        if affected == 0 {
            Err(LeaseStoreError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn delete_lease(&self, ip: Ipv4Addr) -> Result<(), LeaseStoreError> {
        let conn = self.conn.lock();
        let affected = conn.execute(
            "DELETE FROM dhcp_leases WHERE ip_address = ?1",
            params![ip.to_string()],
        )?;
        if affected == 0 {
            Err(LeaseStoreError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn list_leases(&self) -> Result<Vec<LeaseV4>, LeaseStoreError> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT * FROM dhcp_leases ORDER BY lease_expires")?;
        let leases = stmt
            .query_map([], Self::row_to_lease)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(leases)
    }

    async fn find_free_ip(
        &self,
        start: Ipv4Addr,
        end: Ipv4Addr,
    ) -> Result<Option<Ipv4Addr>, LeaseStoreError> {
        let conn = self.conn.lock();
        // Collect IPs that have an active or offered lease.
        let mut stmt = conn.prepare(
            "SELECT ip_address FROM dhcp_leases \
             WHERE lease_state IN ('offered', 'active', 'declined')",
        )?;
        let used: std::collections::HashSet<u32> = stmt
            .query_map([], |row| {
                let s: String = row.get(0)?;
                let ip: Ipv4Addr = s.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok(u32::from(ip))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let start_u = u32::from(start);
        let end_u = u32::from(end);
        for candidate in start_u..=end_u {
            if !used.contains(&candidate) {
                return Ok(Some(Ipv4Addr::from(candidate)));
            }
        }
        Ok(None)
    }

    async fn get_static_lease(
        &self,
        mac: &str,
    ) -> Result<Option<StaticLeaseV4Record>, LeaseStoreError> {
        let conn = self.conn.lock();
        let lease = conn
            .query_row(
                "SELECT * FROM dhcp_static_leases WHERE mac_address = ?1",
                params![mac],
                Self::row_to_static,
            )
            .optional()?;
        Ok(lease)
    }

    async fn upsert_static_lease(
        &self,
        lease: &StaticLeaseV4Record,
    ) -> Result<(), LeaseStoreError> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO dhcp_static_leases (mac_address, ip_address, hostname, profile) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(mac_address) DO UPDATE SET \
             ip_address = excluded.ip_address, \
             hostname = excluded.hostname, \
             profile = excluded.profile",
            params![
                lease.mac,
                lease.ip.to_string(),
                lease.hostname,
                lease.profile,
            ],
        )?;
        Ok(())
    }

    async fn mark_conflicted(&self, ip: Ipv4Addr) -> Result<(), LeaseStoreError> {
        let conn = self.conn.lock();
        // Try to update an existing row first.
        let affected = conn.execute(
            "UPDATE dhcp_leases SET lease_state = 'declined', updated_at = ?2 \
             WHERE ip_address = ?1",
            params![ip.to_string(), now_ts()],
        )?;
        if affected == 0 {
            // No existing row — insert a declined placeholder so the
            // pool allocator skips this IP on subsequent searches.
            let ts = now_ts();
            conn.execute(
                "INSERT OR IGNORE INTO dhcp_leases \
                 (ip_address, mac_address, hostname, client_id, vendor_class, \
                  profile, lease_expires, lease_state, created_at, updated_at) \
                 VALUES (?1, ?2, NULL, NULL, NULL, NULL, ?3, 'declined', ?4, ?4)",
                params![ip.to_string(), "00:00:00:00:00:00", ts + 3600, ts],
            )?;
        }
        Ok(())
    }
}

/// Current unix timestamp (seconds).
pub fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lease(ip: Ipv4Addr, mac: &str, state: LeaseState) -> LeaseV4 {
        let ts = now_ts();
        LeaseV4 {
            ip,
            mac: mac.to_string(),
            hostname: Some("host".to_string()),
            client_id: None,
            vendor_class: None,
            profile: None,
            lease_expires: ts + 3600,
            lease_state: state,
            created_at: ts,
            updated_at: ts,
        }
    }

    #[tokio::test]
    async fn insert_and_get_lease() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let lease = make_lease(Ipv4Addr::new(192, 168, 1, 100), "00:11:22:33:44:55", LeaseState::Offered);
        store.insert_lease(&lease).await.unwrap();

        let got = store
            .get_lease(Ipv4Addr::new(192, 168, 1, 100))
            .await
            .unwrap()
            .expect("lease should exist");
        assert_eq!(got.mac, "00:11:22:33:44:55");
        assert_eq!(got.lease_state, LeaseState::Offered);

        let by_mac = store
            .get_lease_by_mac("00:11:22:33:44:55")
            .await
            .unwrap()
            .expect("lease by mac should exist");
        assert_eq!(by_mac.ip, Ipv4Addr::new(192, 168, 1, 100));
    }

    #[tokio::test]
    async fn update_lease() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let mut lease = make_lease(Ipv4Addr::new(192, 168, 1, 101), "aa:bb:cc:dd:ee:ff", LeaseState::Offered);
        store.insert_lease(&lease).await.unwrap();

        lease.lease_state = LeaseState::Active;
        lease.updated_at = now_ts();
        store.update_lease(&lease).await.unwrap();

        let got = store
            .get_lease(Ipv4Addr::new(192, 168, 1, 101))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.lease_state, LeaseState::Active);
    }

    #[tokio::test]
    async fn update_missing_lease_fails() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let lease = make_lease(Ipv4Addr::new(10, 0, 0, 1), "aa:bb:cc:dd:ee:ff", LeaseState::Active);
        let err = store.update_lease(&lease).await.unwrap_err();
        assert!(matches!(err, LeaseStoreError::NotFound));
    }

    #[tokio::test]
    async fn delete_lease() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let lease = make_lease(Ipv4Addr::new(192, 168, 1, 102), "11:22:33:44:55:66", LeaseState::Active);
        store.insert_lease(&lease).await.unwrap();
        store
            .delete_lease(Ipv4Addr::new(192, 168, 1, 102))
            .await
            .unwrap();
        assert!(store
            .get_lease(Ipv4Addr::new(192, 168, 1, 102))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_missing_fails() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let err = store
            .delete_lease(Ipv4Addr::new(10, 0, 0, 99))
            .await
            .unwrap_err();
        assert!(matches!(err, LeaseStoreError::NotFound));
    }

    #[tokio::test]
    async fn list_leases() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        store
            .insert_lease(&make_lease(Ipv4Addr::new(192, 168, 1, 10), "a:b:c:d:e:f", LeaseState::Active))
            .await
            .unwrap();
        store
            .insert_lease(&make_lease(Ipv4Addr::new(192, 168, 1, 11), "a:b:c:d:e:1", LeaseState::Active))
            .await
            .unwrap();
        let all = store.list_leases().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn find_free_ip_skips_used() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        // Occupy .100
        store
            .insert_lease(&make_lease(Ipv4Addr::new(192, 168, 1, 100), "a:b:c:d:e:f", LeaseState::Active))
            .await
            .unwrap();
        let free = store
            .find_free_ip(
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 102),
            )
            .await
            .unwrap();
        assert_eq!(free, Some(Ipv4Addr::new(192, 168, 1, 101)));
    }

    #[tokio::test]
    async fn find_free_ip_exhausted() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        store
            .insert_lease(&make_lease(Ipv4Addr::new(192, 168, 1, 100), "a:b:c:d:e:f", LeaseState::Active))
            .await
            .unwrap();
        let free = store
            .find_free_ip(
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 100),
            )
            .await
            .unwrap();
        assert_eq!(free, None);
    }

    #[tokio::test]
    async fn find_free_ip_skips_declined() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        store
            .insert_lease(&make_lease(Ipv4Addr::new(192, 168, 1, 100), "a:b:c:d:e:f", LeaseState::Declined))
            .await
            .unwrap();
        let free = store
            .find_free_ip(
                Ipv4Addr::new(192, 168, 1, 100),
                Ipv4Addr::new(192, 168, 1, 101),
            )
            .await
            .unwrap();
        // .100 is declined (conflicted) → skip to .101
        assert_eq!(free, Some(Ipv4Addr::new(192, 168, 1, 101)));
    }

    #[tokio::test]
    async fn static_lease_upsert_and_get() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let sl = StaticLeaseV4Record {
            mac: "00:11:22:33:44:55".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 10),
            hostname: Some("dad".to_string()),
            profile: Some("parents".to_string()),
        };
        store.upsert_static_lease(&sl).await.unwrap();

        let got = store
            .get_static_lease("00:11:22:33:44:55")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.ip, Ipv4Addr::new(192, 168, 1, 10));

        // Upsert (update) with a new IP.
        let sl2 = StaticLeaseV4Record {
            mac: "00:11:22:33:44:55".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 20),
            hostname: None,
            profile: None,
        };
        store.upsert_static_lease(&sl2).await.unwrap();
        let got2 = store
            .get_static_lease("00:11:22:33:44:55")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got2.ip, Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(got2.hostname, None);
    }

    #[tokio::test]
    async fn mark_conflicted_updates_state() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let lease = make_lease(Ipv4Addr::new(192, 168, 1, 50), "a:b:c:d:e:f", LeaseState::Active);
        store.insert_lease(&lease).await.unwrap();
        store
            .mark_conflicted(Ipv4Addr::new(192, 168, 1, 50))
            .await
            .unwrap();
        let got = store
            .get_lease(Ipv4Addr::new(192, 168, 1, 50))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.lease_state, LeaseState::Declined);
    }

    #[test]
    fn lease_state_roundtrip() {
        for s in [
            LeaseState::Offered,
            LeaseState::Active,
            LeaseState::Expired,
            LeaseState::Released,
            LeaseState::Declined,
        ] {
            let str = s.as_str();
            assert_eq!(LeaseState::from_str(str), Some(s));
        }
        assert_eq!(LeaseState::from_str("bogus"), None);
    }
}
