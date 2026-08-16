//! DHCPv6 lease storage: [`LeaseStoreV6`] trait + [`SqliteLeaseStoreV6`].
//!
//! Implements the v6 portion of the `LeaseStore` abstraction from PRD
//! section 4.4 (lines 538-552). The SQLite schema matches PRD lines
//! 713-731 (`dhcpv6_leases` and `dhcpv6_static_leases`).
//!
//! The trait is `async` (via [`async_trait`]) so a future
//! `FailoverLeaseStore` (RFC 8156) or `RedisLeaseStore` can drop in
//! without touching the state machine. The default
//! [`SqliteLeaseStoreV6`] uses a blocking `rusqlite::Connection` wrapped
//! in a [`parking_lot::Mutex`] — lease operations are short and
//! infrequent, so a mutex is simpler and lower-overhead than a connection
//! pool for the single-instance homelab target.

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use std::net::Ipv6Addr;
use std::path::Path;
use std::sync::Arc;

use super::config::DhcpPoolV6;

/// Errors returned by lease store operations.
#[derive(Debug)]
pub enum LeaseStoreError {
    /// SQLite returned an error.
    Sqlite(rusqlite::Error),
    /// A required column could not be parsed from a row.
    Parse(String),
}

impl std::fmt::Display for LeaseStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaseStoreError::Sqlite(e) => write!(f, "sqlite error: {e}"),
            LeaseStoreError::Parse(msg) => write!(f, "lease parse error: {msg}"),
        }
    }
}

impl std::error::Error for LeaseStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LeaseStoreError::Sqlite(e) => Some(e),
            LeaseStoreError::Parse(_) => None,
        }
    }
}

impl From<rusqlite::Error> for LeaseStoreError {
    fn from(e: rusqlite::Error) -> Self {
        LeaseStoreError::Sqlite(e)
    }
}

type Result<T> = std::result::Result<T, LeaseStoreError>;

/// Lifecycle state of a DHCPv6 lease (PRD `lease_state` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// Address advertised in an ADVERTISE but not yet committed (REQUEST).
    Advertised,
    /// Lease is active — confirmed by a REQUEST/REPLY or RENEW/REPLY.
    Active,
    /// Lease has expired past its valid lifetime.
    Expired,
    /// Client explicitly released the lease.
    Released,
}

impl LeaseState {
    pub fn as_str(&self) -> &'static str {
        match self {
            LeaseState::Advertised => "advertised",
            LeaseState::Active => "active",
            LeaseState::Expired => "expired",
            LeaseState::Released => "released",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "active" => LeaseState::Active,
            "expired" => LeaseState::Expired,
            "released" => LeaseState::Released,
            _ => LeaseState::Advertised,
        }
    }
}

/// A dynamic DHCPv6 lease row (PRD `dhcpv6_leases` table, lines 713-724).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpLeaseV6 {
    pub ipv6_address: Ipv6Addr,
    /// Hex-encoded DUID (see [`super::duid::Duid::to_hex`]).
    pub duid: String,
    pub iaid: u32,
    pub hostname: Option<String>,
    pub vendor_class: Option<String>,
    pub profile: Option<String>,
    /// Unix timestamp (seconds) when the lease expires.
    pub lease_expires: i64,
    pub lease_state: LeaseState,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A static DHCPv6 lease row (PRD `dhcpv6_static_leases`, lines 726-731).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticLeaseV6 {
    pub duid: String,
    pub ipv6_address: Ipv6Addr,
    pub hostname: Option<String>,
    pub profile: Option<String>,
}

/// Async lease storage abstraction for DHCPv6 (PRD lines 538-552).
#[async_trait]
pub trait LeaseStoreV6: Send + Sync {
    /// Fetch the dynamic lease for a given IPv6 address, if any.
    async fn get_lease_v6(&self, ip: &Ipv6Addr) -> Result<Option<DhcpLeaseV6>>;
    /// Fetch the dynamic lease for a given DUID, if any.
    async fn get_lease_by_duid_v6(&self, duid: &str) -> Result<Option<DhcpLeaseV6>>;
    /// Insert a new dynamic lease.
    async fn insert_lease_v6(&self, lease: &DhcpLeaseV6) -> Result<()>;
    /// Update an existing dynamic lease (matched by `ipv6_address`).
    async fn update_lease_v6(&self, lease: &DhcpLeaseV6) -> Result<()>;
    /// Delete a dynamic lease by address.
    async fn delete_lease_v6(&self, ip: &Ipv6Addr) -> Result<()>;
    /// List all dynamic leases belonging to `pool` (matched by address
    /// range of the pool).
    async fn list_leases_v6(&self, pool: &DhcpPoolV6) -> Result<Vec<DhcpLeaseV6>>;
    /// Find the first free IPv6 address in `pool` (not present in the
    /// leases table and not expired-active). Returns `None` if the pool
    /// is exhausted.
    async fn find_free_ipv6(&self, pool: &DhcpPoolV6) -> Result<Option<Ipv6Addr>>;

    /// Fetch a static lease for a DUID, if any.
    async fn get_static_lease_v6(&self, duid: &str) -> Result<Option<StaticLeaseV6>>;
    /// Insert or replace a static lease.
    async fn upsert_static_lease_v6(&self, lease: &StaticLeaseV6) -> Result<()>;
    /// Delete a static lease by DUID.
    async fn delete_static_lease_v6(&self, duid: &str) -> Result<()>;
    /// List all static leases.
    async fn list_static_leases_v6(&self) -> Result<Vec<StaticLeaseV6>>;
}

/// SQLite-backed implementation of [`LeaseStoreV6`].
///
/// Holds a single [`Connection`] behind a [`parking_lot::Mutex`]. The
/// schema is created on construction via [`SqliteLeaseStoreV6::open`].
pub struct SqliteLeaseStoreV6 {
    conn: Arc<parking_lot::Mutex<Connection>>,
}

impl SqliteLeaseStoreV6 {
    /// Open (or create) the SQLite database at `path` and ensure the
    /// DHCPv6 schema exists.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(parking_lot::Mutex::new(conn)),
        })
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(parking_lot::Mutex::new(conn)),
        })
    }

    /// Create the `dhcpv6_leases` and `dhcpv6_static_leases` tables per
    /// PRD lines 713-731 if they do not already exist.
    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dhcpv6_leases (
                ipv6_address  TEXT PRIMARY KEY,
                duid          TEXT NOT NULL,
                iaid          INTEGER NOT NULL,
                hostname      TEXT,
                vendor_class  TEXT,
                profile       TEXT,
                lease_expires INTEGER NOT NULL,
                lease_state   TEXT NOT NULL,
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS dhcpv6_static_leases (
                duid          TEXT PRIMARY KEY,
                ipv6_address  TEXT NOT NULL,
                hostname      TEXT,
                profile       TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_dhcpv6_leases_duid
                ON dhcpv6_leases(duid);
            CREATE INDEX IF NOT EXISTS idx_dhcpv6_leases_expires
                ON dhcpv6_leases(lease_expires);
            CREATE INDEX IF NOT EXISTS idx_dhcpv6_leases_state
                ON dhcpv6_leases(lease_state);
            ",
        )?;
        Ok(())
    }

    fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock();
        f(&conn)
    }
}

fn row_to_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<DhcpLeaseV6> {
    let ip_str: String = row.get(0)?;
    let ipv6_address: Ipv6Addr = ip_str
        .parse()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
    let state_str: String = row.get(7)?;
    Ok(DhcpLeaseV6 {
        ipv6_address,
        duid: row.get(1)?,
        iaid: row.get(2)?,
        hostname: row.get(3)?,
        vendor_class: row.get(4)?,
        profile: row.get(5)?,
        lease_expires: row.get(6)?,
        lease_state: LeaseState::from_str(&state_str),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_static(row: &rusqlite::Row<'_>) -> rusqlite::Result<StaticLeaseV6> {
    let ip_str: String = row.get(1)?;
    let ipv6_address: Ipv6Addr = ip_str
        .parse()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    Ok(StaticLeaseV6 {
        duid: row.get(0)?,
        ipv6_address,
        hostname: row.get(2)?,
        profile: row.get(3)?,
    })
}

#[async_trait]
impl LeaseStoreV6 for SqliteLeaseStoreV6 {
    async fn get_lease_v6(&self, ip: &Ipv6Addr) -> Result<Option<DhcpLeaseV6>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ipv6_address, duid, iaid, hostname, vendor_class, profile,
                        lease_expires, lease_state, created_at, updated_at
                 FROM dhcpv6_leases WHERE ipv6_address = ?1",
            )?;
            let lease = stmt
                .query_row(params![ip.to_string()], row_to_lease)
                .optional()?;
            Ok(lease)
        })
    }

    async fn get_lease_by_duid_v6(&self, duid: &str) -> Result<Option<DhcpLeaseV6>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ipv6_address, duid, iaid, hostname, vendor_class, profile,
                        lease_expires, lease_state, created_at, updated_at
                 FROM dhcpv6_leases
                 WHERE duid = ?1 AND lease_state IN ('advertised', 'active')
                 ORDER BY updated_at DESC LIMIT 1",
            )?;
            let lease = stmt
                .query_row(params![duid], row_to_lease)
                .optional()?;
            Ok(lease)
        })
    }

    async fn insert_lease_v6(&self, lease: &DhcpLeaseV6) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO dhcpv6_leases
                    (ipv6_address, duid, iaid, hostname, vendor_class, profile,
                     lease_expires, lease_state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    lease.ipv6_address.to_string(),
                    lease.duid,
                    lease.iaid,
                    lease.hostname,
                    lease.vendor_class,
                    lease.profile,
                    lease.lease_expires,
                    lease.lease_state.as_str(),
                    lease.created_at,
                    lease.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    async fn update_lease_v6(&self, lease: &DhcpLeaseV6) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE dhcpv6_leases
                 SET duid = ?2, iaid = ?3, hostname = ?4, vendor_class = ?5,
                     profile = ?6, lease_expires = ?7, lease_state = ?8,
                     updated_at = ?10
                 WHERE ipv6_address = ?1",
                params![
                    lease.ipv6_address.to_string(),
                    lease.duid,
                    lease.iaid,
                    lease.hostname,
                    lease.vendor_class,
                    lease.profile,
                    lease.lease_expires,
                    lease.lease_state.as_str(),
                    lease.created_at,
                    lease.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    async fn delete_lease_v6(&self, ip: &Ipv6Addr) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM dhcpv6_leases WHERE ipv6_address = ?1",
                params![ip.to_string()],
            )?;
            Ok(())
        })
    }

    async fn list_leases_v6(&self, pool: &DhcpPoolV6) -> Result<Vec<DhcpLeaseV6>> {
        let start = u128::from(pool.pool_start);
        let end = u128::from(pool.pool_end);
        // SQLite stores IPv6 as text. To filter by range without relying on
        // text ordering of IPv6 strings, we load all rows and filter in Rust
        // by parsed address. Pools are small (homelab scale) so this is fine.
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ipv6_address, duid, iaid, hostname, vendor_class, profile,
                        lease_expires, lease_state, created_at, updated_at
                 FROM dhcpv6_leases",
            )?;
            let rows = stmt.query_map([], row_to_lease)?;
            let mut out = Vec::new();
            for row in rows {
                let lease = row?;
                let n = u128::from(lease.ipv6_address);
                if n >= start && n <= end {
                    out.push(lease);
                }
            }
            Ok(out)
        })
    }

    async fn find_free_ipv6(&self, pool: &DhcpPoolV6) -> Result<Option<Ipv6Addr>> {
        // Collect the set of in-use (advertised or active) addresses, then
        // walk the pool range for the first gap.
        let used: std::collections::HashSet<Ipv6Addr> = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ipv6_address FROM dhcpv6_leases
                 WHERE lease_state IN ('advertised', 'active')",
            )?;
            let rows = stmt.query_map([], |row| {
                let s: String = row.get(0)?;
                let ip: Ipv6Addr = s.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok(ip)
            })?;
            let mut set = std::collections::HashSet::new();
            for row in rows {
                set.insert(row?);
            }
            Ok(set)
        })?;
        for ip in pool.iter_addresses() {
            if !used.contains(&ip) {
                return Ok(Some(ip));
            }
        }
        Ok(None)
    }

    async fn get_static_lease_v6(&self, duid: &str) -> Result<Option<StaticLeaseV6>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT duid, ipv6_address, hostname, profile
                 FROM dhcpv6_static_leases WHERE duid = ?1",
            )?;
            let lease = stmt
                .query_row(params![duid], row_to_static)
                .optional()?;
            Ok(lease)
        })
    }

    async fn upsert_static_lease_v6(&self, lease: &StaticLeaseV6) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO dhcpv6_static_leases
                    (duid, ipv6_address, hostname, profile)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    lease.duid,
                    lease.ipv6_address.to_string(),
                    lease.hostname,
                    lease.profile,
                ],
            )?;
            Ok(())
        })
    }

    async fn delete_static_lease_v6(&self, duid: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM dhcpv6_static_leases WHERE duid = ?1",
                params![duid],
            )?;
            Ok(())
        })
    }

    async fn list_static_leases_v6(&self) -> Result<Vec<StaticLeaseV6>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT duid, ipv6_address, hostname, profile
                 FROM dhcpv6_static_leases ORDER BY duid",
            )?;
            let rows = stmt.query_map([], row_to_static)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> DhcpPoolV6 {
        DhcpPoolV6 {
            name: "main".into(),
            prefix: "fd00:1234:5678::/64".into(),
            pool_start: "fd00:1234:5678::100".parse().unwrap(),
            pool_end: "fd00:1234:5678::103".parse().unwrap(),
            dns_servers: vec!["fd00:1234:5678::67".parse().unwrap()],
        }
    }

    fn lease(ip: &str, duid: &str, state: LeaseState) -> DhcpLeaseV6 {
        DhcpLeaseV6 {
            ipv6_address: ip.parse().unwrap(),
            duid: duid.to_string(),
            iaid: 1,
            hostname: Some("host".into()),
            vendor_class: None,
            profile: None,
            lease_expires: 1_000_000,
            lease_state: state,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn insert_get_update_delete() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let l = lease("fd00:1234:5678::100", "aabb", LeaseState::Active);
        store.insert_lease_v6(&l).await.unwrap();

        let got = store
            .get_lease_v6(&"fd00:1234:5678::100".parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, l);

        let by_duid = store.get_lease_by_duid_v6("aabb").await.unwrap().unwrap();
        assert_eq!(by_duid.ipv6_address, l.ipv6_address);

        let mut updated = l.clone();
        updated.hostname = Some("renamed".into());
        updated.lease_state = LeaseState::Released;
        store.update_lease_v6(&updated).await.unwrap();
        let got2 = store
            .get_lease_v6(&l.ipv6_address)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got2.hostname.as_deref(), Some("renamed"));
        assert_eq!(got2.lease_state, LeaseState::Released);

        store.delete_lease_v6(&l.ipv6_address).await.unwrap();
        assert!(store.get_lease_v6(&l.ipv6_address).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_leases_in_pool_range() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        store
            .insert_lease_v6(&lease("fd00:1234:5678::100", "a", LeaseState::Active))
            .await
            .unwrap();
        store
            .insert_lease_v6(&lease("fd00:1234:5678::102", "b", LeaseState::Active))
            .await
            .unwrap();
        // outside the pool
        store
            .insert_lease_v6(&lease("fd00:1234:5678::200", "c", LeaseState::Active))
            .await
            .unwrap();
        let listed = store.list_leases_v6(&pool()).await.unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[tokio::test]
    async fn find_free_ipv6_skips_used() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        store
            .insert_lease_v6(&lease("fd00:1234:5678::100", "a", LeaseState::Active))
            .await
            .unwrap();
        store
            .insert_lease_v6(&lease(
                "fd00:1234:5678::101",
                "b",
                LeaseState::Advertised,
            ))
            .await
            .unwrap();
        let free = store.find_free_ipv6(&pool()).await.unwrap().unwrap();
        assert_eq!(free, "fd00:1234:5678::102".parse::<Ipv6Addr>().unwrap());
    }

    #[tokio::test]
    async fn find_free_ipv6_none_when_exhausted() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let p = pool();
        for ip in p.iter_addresses() {
            let mut l = lease(&ip.to_string(), "x", LeaseState::Active);
            l.ipv6_address = ip;
            store.insert_lease_v6(&l).await.unwrap();
        }
        assert!(store.find_free_ipv6(&p).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn static_lease_upsert_get_delete() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let s = StaticLeaseV6 {
            duid: "deadbeef".into(),
            ipv6_address: "fd00:1234:5678::50".parse().unwrap(),
            hostname: Some("printer".into()),
            profile: Some("iot".into()),
        };
        store.upsert_static_lease_v6(&s).await.unwrap();
        let got = store.get_static_lease_v6("deadbeef").await.unwrap().unwrap();
        assert_eq!(got, s);

        // replace
        let mut s2 = s.clone();
        s2.hostname = Some("printer2".into());
        store.upsert_static_lease_v6(&s2).await.unwrap();
        let got2 = store.get_static_lease_v6("deadbeef").await.unwrap().unwrap();
        assert_eq!(got2.hostname.as_deref(), Some("printer2"));

        store.delete_static_lease_v6("deadbeef").await.unwrap();
        assert!(store.get_static_lease_v6("deadbeef").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn schema_persists_across_reopen() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "dnshub-v6-test-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let store = SqliteLeaseStoreV6::open(&path).unwrap();
            store
                .insert_lease_v6(&lease("fd00::1", "zz", LeaseState::Active))
                .await
                .unwrap();
        }
        // Reopen — schema must already exist and the lease must survive.
        let store = SqliteLeaseStoreV6::open(&path).unwrap();
        let got = store
            .get_lease_v6(&"fd00::1".parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.duid, "zz");
        let _ = std::fs::remove_file(&path);
    }
}
