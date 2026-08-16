//! DHCP lease audit log backed by SQLite.
//!
//! [`AuditLogger`] persists [`DhcpAuditEvent`]s to the `dhcp_audit_log` table
//! (PRD section 4.4, lines 740-754) and provides filtered queries for the
//! frontend audit log viewer (story 05-006). The schema matches the PRD
//! exactly:
//!
//! ```sql
//! CREATE TABLE dhcp_audit_log (
//!     id            INTEGER PRIMARY KEY AUTOINCREMENT,
//!     timestamp     INTEGER NOT NULL,   -- unix millis
//!     event_type    TEXT NOT NULL,      -- "ack","renew","release","decline","expire","conflict"
//!     mac_address   TEXT,
//!     duid          TEXT,               -- for DHCPv6 events
//!     ip_address    TEXT,
//!     hostname      TEXT,
//!     profile       TEXT,
//!     details       TEXT                -- additional context (e.g. block reason)
//! );
//! CREATE INDEX idx_dhcp_audit_timestamp ON dhcp_audit_log(timestamp);
//! CREATE INDEX idx_dhcp_audit_mac ON dhcp_audit_log(mac_address);
//! ```
//!
//! The logger is intentionally synchronous (rusqlite is blocking). The DHCP
//! server hot path should call [`AuditLogger::write_audit_event`] from a
//! blocking task or a dedicated writer thread; for v1 a simple
//! `tokio::task::spawn_blocking` wrapper is sufficient.
//!
//! Concurrency: each [`AuditLogger`] owns its own [`rusqlite::Connection`].
//! SQLite serializes writers, so a single logger instance is safe to share
//! behind a `Mutex` (the DHCP server has one writer). For higher throughput a
//! future story can batch writes or use WAL mode.

pub mod config;
pub mod events;

pub use config::AuditConfig;
pub use events::{DhcpAuditEvent, DhcpAuditEventType, UnknownEventType};

use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;
use std::sync::Mutex;

/// Errors returned by [`AuditLogger`] operations.
#[derive(Debug)]
pub enum AuditError {
    /// A SQLite operation failed.
    Sqlite(rusqlite::Error),
    /// An `event_type` string read back from the database was not recognized.
    UnknownEventType(UnknownEventType),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::Sqlite(e) => write!(f, "audit log sqlite error: {e}"),
            AuditError::UnknownEventType(e) => write!(f, "audit log decode error: {e}"),
        }
    }
}

impl std::error::Error for AuditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AuditError::Sqlite(e) => Some(e),
            AuditError::UnknownEventType(e) => Some(e),
        }
    }
}

impl From<rusqlite::Error> for AuditError {
    fn from(e: rusqlite::Error) -> Self {
        AuditError::Sqlite(e)
    }
}

impl From<UnknownEventType> for AuditError {
    fn from(e: UnknownEventType) -> Self {
        AuditError::UnknownEventType(e)
    }
}

/// Filter clause for [`AuditLogger::list_audit_events`].
///
/// All fields are optional; `None` means "no constraint on this field".
/// Multiple set fields are AND-ed together.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Restrict to events with this `event_type`.
    pub event_type: Option<DhcpAuditEventType>,
    /// Restrict to events for this MAC address.
    pub mac_address: Option<String>,
    /// Inclusive lower bound on `timestamp` (unix millis).
    pub from_timestamp: Option<i64>,
    /// Inclusive upper bound on `timestamp` (unix millis).
    pub to_timestamp: Option<i64>,
    /// Maximum number of rows to return (newest first). `None` returns all
    /// matching rows.
    pub limit: Option<i64>,
}

/// Persistent DHCP lease audit log.
///
/// Wraps a single SQLite connection guarded by a [`Mutex`]. The schema is
/// created idempotently on construction via [`AuditLogger::open`] /
/// [`AuditLogger::open_in_memory`].
pub struct AuditLogger {
    conn: Mutex<Connection>,
}

impl AuditLogger {
    /// Open (or create) the audit log database at `path`, creating the
    /// `dhcp_audit_log` table and indexes if they do not already exist.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, AuditError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory audit log (used by tests and the rogue detector's
    /// unit tests).
    pub fn open_in_memory() -> Result<Self, AuditError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init(conn: &Connection) -> Result<(), AuditError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dhcp_audit_log (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp     INTEGER NOT NULL,
                event_type    TEXT NOT NULL,
                mac_address   TEXT,
                duid          TEXT,
                ip_address    TEXT,
                hostname      TEXT,
                profile       TEXT,
                details       TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_dhcp_audit_timestamp ON dhcp_audit_log(timestamp);
            CREATE INDEX IF NOT EXISTS idx_dhcp_audit_mac ON dhcp_audit_log(mac_address);
            CREATE INDEX IF NOT EXISTS idx_dhcp_audit_event_type ON dhcp_audit_log(event_type);",
        )?;
        Ok(())
    }

    /// Persist a single audit event. Returns the assigned row id.
    pub fn write_audit_event(&self, event: &DhcpAuditEvent) -> Result<i64, AuditError> {
        let conn = self.conn.lock().expect("audit log mutex poisoned");
        conn.execute(
            "INSERT INTO dhcp_audit_log
                (timestamp, event_type, mac_address, duid, ip_address, hostname, profile, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.timestamp,
                event.event_type.as_str(),
                event.mac_address,
                event.duid,
                event.ip_address,
                event.hostname,
                event.profile,
                event.details,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Query audit events matching `filter`, ordered newest-first.
    pub fn list_audit_events(
        &self,
        filter: &AuditFilter,
    ) -> Result<Vec<DhcpAuditEvent>, AuditError> {
        let conn = self.conn.lock().expect("audit log mutex poisoned");
        let mut sql = String::from(
            "SELECT timestamp, event_type, mac_address, duid, ip_address, hostname, profile, details
             FROM dhcp_audit_log WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(et) = filter.event_type {
            sql.push_str(" AND event_type = ?");
            args.push(rusqlite::types::Value::Text(et.as_str().to_string()));
        }
        if let Some(ref mac) = filter.mac_address {
            sql.push_str(" AND mac_address = ?");
            args.push(rusqlite::types::Value::Text(mac.clone()));
        }
        if let Some(from) = filter.from_timestamp {
            sql.push_str(" AND timestamp >= ?");
            args.push(rusqlite::types::Value::Integer(from));
        }
        if let Some(to) = filter.to_timestamp {
            sql.push_str(" AND timestamp <= ?");
            args.push(rusqlite::types::Value::Integer(to));
        }
        sql.push_str(" ORDER BY timestamp DESC, id DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(" LIMIT ?");
            args.push(rusqlite::types::Value::Integer(limit));
        }

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            let ts: i64 = row.get(0)?;
            let et_str: String = row.get(1)?;
            let mac: Option<String> = row.get(2)?;
            let duid: Option<String> = row.get(3)?;
            let ip: Option<String> = row.get(4)?;
            let hostname: Option<String> = row.get(5)?;
            let profile: Option<String> = row.get(6)?;
            let details: Option<String> = row.get(7)?;
            Ok((ts, et_str, mac, duid, ip, hostname, profile, details))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (ts, et_str, mac, duid, ip, hostname, profile, details) = row?;
            let event_type: DhcpAuditEventType = et_str.parse()?;
            out.push(DhcpAuditEvent {
                timestamp: ts,
                event_type,
                mac_address: mac,
                duid,
                ip_address: ip,
                hostname,
                profile,
                details,
            });
        }
        Ok(out)
    }

    /// Count events matching `filter` without materializing rows. Useful for
    /// the frontend pagination indicator.
    pub fn count_audit_events(&self, filter: &AuditFilter) -> Result<i64, AuditError> {
        let conn = self.conn.lock().expect("audit log mutex poisoned");
        let mut sql = String::from("SELECT COUNT(*) FROM dhcp_audit_log WHERE 1=1");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(et) = filter.event_type {
            sql.push_str(" AND event_type = ?");
            args.push(rusqlite::types::Value::Text(et.as_str().to_string()));
        }
        if let Some(ref mac) = filter.mac_address {
            sql.push_str(" AND mac_address = ?");
            args.push(rusqlite::types::Value::Text(mac.clone()));
        }
        if let Some(from) = filter.from_timestamp {
            sql.push_str(" AND timestamp >= ?");
            args.push(rusqlite::types::Value::Integer(from));
        }
        if let Some(to) = filter.to_timestamp {
            sql.push_str(" AND timestamp <= ?");
            args.push(rusqlite::types::Value::Integer(to));
        }

        let count: i64 = conn
            .query_row(&sql, rusqlite::params_from_iter(args.iter()), |row| {
                row.get(0)
            })
            .map_err(AuditError::from)?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_log() -> AuditLogger {
        AuditLogger::open_in_memory().expect("open in-memory audit log")
    }

    #[test]
    fn write_and_list_event() {
        let log = make_log();
        let ev = DhcpAuditEvent::new(1_000, DhcpAuditEventType::Ack)
            .with_mac("00:11:22:33:44:55")
            .with_ip("192.168.1.10")
            .with_hostname("laptop")
            .with_profile("parents");
        let id = log.write_audit_event(&ev).unwrap();
        assert!(id > 0);

        let listed = log.list_audit_events(&AuditFilter::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], ev);
    }

    #[test]
    fn list_orders_newest_first() {
        let log = make_log();
        log.write_audit_event(&DhcpAuditEvent::new(100, DhcpAuditEventType::Ack))
            .unwrap();
        log.write_audit_event(&DhcpAuditEvent::new(300, DhcpAuditEventType::Release))
            .unwrap();
        log.write_audit_event(&DhcpAuditEvent::new(200, DhcpAuditEventType::Expire))
            .unwrap();

        let listed = log.list_audit_events(&AuditFilter::default()).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].timestamp, 300);
        assert_eq!(listed[1].timestamp, 200);
        assert_eq!(listed[2].timestamp, 100);
    }

    #[test]
    fn filter_by_event_type() {
        let log = make_log();
        log.write_audit_event(&DhcpAuditEvent::new(1, DhcpAuditEventType::Ack))
            .unwrap();
        log.write_audit_event(&DhcpAuditEvent::new(2, DhcpAuditEventType::Release))
            .unwrap();
        log.write_audit_event(&DhcpAuditEvent::new(3, DhcpAuditEventType::Ack))
            .unwrap();

        let filter = AuditFilter {
            event_type: Some(DhcpAuditEventType::Ack),
            ..Default::default()
        };
        let listed = log.list_audit_events(&filter).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|e| e.event_type == DhcpAuditEventType::Ack));
        assert_eq!(log.count_audit_events(&filter).unwrap(), 2);
    }

    #[test]
    fn filter_by_mac() {
        let log = make_log();
        log.write_audit_event(
            &DhcpAuditEvent::new(1, DhcpAuditEventType::Ack).with_mac("aa:bb:cc:dd:ee:ff"),
        )
        .unwrap();
        log.write_audit_event(
            &DhcpAuditEvent::new(2, DhcpAuditEventType::Ack).with_mac("00:11:22:33:44:55"),
        )
        .unwrap();

        let filter = AuditFilter {
            mac_address: Some("aa:bb:cc:dd:ee:ff".to_string()),
            ..Default::default()
        };
        let listed = log.list_audit_events(&filter).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].mac_address.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
    }

    #[test]
    fn filter_by_time_range() {
        let log = make_log();
        for ts in [10, 20, 30, 40, 50] {
            log.write_audit_event(&DhcpAuditEvent::new(ts, DhcpAuditEventType::Ack))
                .unwrap();
        }
        let filter = AuditFilter {
            from_timestamp: Some(20),
            to_timestamp: Some(40),
            ..Default::default()
        };
        let listed = log.list_audit_events(&filter).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].timestamp, 40);
        assert_eq!(listed[2].timestamp, 20);
    }

    #[test]
    fn filter_limit() {
        let log = make_log();
        for ts in 0..10 {
            log.write_audit_event(&DhcpAuditEvent::new(ts, DhcpAuditEventType::Ack))
                .unwrap();
        }
        let filter = AuditFilter {
            limit: Some(3),
            ..Default::default()
        };
        let listed = log.list_audit_events(&filter).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].timestamp, 9);
    }

    #[test]
    fn all_event_types_roundtrip() {
        let log = make_log();
        for et in [
            DhcpAuditEventType::Ack,
            DhcpAuditEventType::Renew,
            DhcpAuditEventType::Release,
            DhcpAuditEventType::Decline,
            DhcpAuditEventType::Expire,
            DhcpAuditEventType::Conflict,
        ] {
            log.write_audit_event(&DhcpAuditEvent::new(1, et)).unwrap();
        }
        let listed = log.list_audit_events(&AuditFilter::default()).unwrap();
        assert_eq!(listed.len(), 6);
        let mut types: Vec<_> = listed.iter().map(|e| e.event_type).collect();
        types.sort();
        assert_eq!(
            types,
            [
                DhcpAuditEventType::Ack,
                DhcpAuditEventType::Renew,
                DhcpAuditEventType::Release,
                DhcpAuditEventType::Decline,
                DhcpAuditEventType::Expire,
                DhcpAuditEventType::Conflict,
            ]
        );
    }

    #[test]
    fn count_with_no_rows() {
        let log = make_log();
        assert_eq!(
            log.count_audit_events(&AuditFilter::default()).unwrap(),
            0
        );
        assert!(log
            .list_audit_events(&AuditFilter::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn duid_and_details_persist() {
        let log = make_log();
        let ev = DhcpAuditEvent::new(5, DhcpAuditEventType::Conflict)
            .with_duid("0001000112345678")
            .with_details("ping conflict on 192.168.1.50");
        log.write_audit_event(&ev).unwrap();
        let listed = log.list_audit_events(&AuditFilter::default()).unwrap();
        assert_eq!(listed[0].duid.as_deref(), Some("0001000112345678"));
        assert_eq!(
            listed[0].details.as_deref(),
            Some("ping conflict on 192.168.1.50")
        );
    }
}
