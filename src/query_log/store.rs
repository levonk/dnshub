//! [`SqliteQueryLogStore`] — embedded SQLite ring-buffer backing the query log.
//!
//! The store creates the `query_log` table and indexes defined in PRD
//! section 4.8 (lines 1213-1233), inserts [`QueryLogEntry`] records, runs
//! ring-buffer and time-based retention cleanup, and supports filtered
//! queries plus CSV export for the frontend viewer (story 05-004/05-006).
//!
//! ## Concurrency
//!
//! The store owns a single [`rusqlite::Connection`] guarded by a
//! [`Mutex`](std::sync::Mutex). SQLite serializes writers, so a single
//! connection is safe behind the mutex. WAL mode (`PRAGMA journal_mode=WAL`)
//! is enabled on persistent databases so that concurrent readers do not
//! block the writer — important because the frontend polls the query log
//! while the DNS hot path is inserting entries.
//!
//! ## Ring buffer
//!
//! [`SqliteQueryLogStore::prune_ring_buffer`] enforces the
//! `max_entries` cap by deleting the oldest rows in batches of 1,000 (per
//! PRD section 4.8) so that cleanup never deletes an unbounded number of
//! rows in a single statement. [`SqliteQueryLogStore::prune_by_age`]
//! enforces the `retention_days` time-based cap. The
//! [`QueryLogger`](super::QueryLogger) invokes both periodically from a
//! background cleanup rather than on the write hot path.

use rusqlite::{params, Connection, OpenFlags};

use crate::query_log::entry::QueryLogEntry;

/// Errors returned by [`SqliteQueryLogStore`] operations.
#[derive(Debug)]
pub enum QueryLogStoreError {
    /// A SQLite operation failed.
    Sqlite(rusqlite::Error),
    /// A CSV export operation failed.
    Csv(String),
}

impl std::fmt::Display for QueryLogStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryLogStoreError::Sqlite(e) => write!(f, "query log sqlite error: {e}"),
            QueryLogStoreError::Csv(e) => write!(f, "query log csv error: {e}"),
        }
    }
}

impl std::error::Error for QueryLogStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            QueryLogStoreError::Sqlite(e) => Some(e),
            QueryLogStoreError::Csv(_) => None,
        }
    }
}

impl From<rusqlite::Error> for QueryLogStoreError {
    fn from(e: rusqlite::Error) -> Self {
        QueryLogStoreError::Sqlite(e)
    }
}

/// Filter clause for [`SqliteQueryLogStore::query`] and
/// [`SqliteQueryLogStore::count`].
///
/// All fields are optional; `None` means "no constraint on that field".
/// Multiple set fields are AND-ed together. Results are ordered
/// newest-first.
#[derive(Debug, Clone, Default)]
pub struct QueryLogFilter {
    /// Restrict to entries from this client IP.
    pub client_ip: Option<String>,
    /// Restrict to entries for this domain (exact match).
    pub domain: Option<String>,
    /// Restrict to blocked entries only when `true`, non-blocked when
    /// `false`. `None` imposes no blocked constraint.
    pub blocked: Option<bool>,
    /// Restrict to entries served from cache when `true`.
    pub cached: Option<bool>,
    /// Inclusive lower bound on `timestamp` (unix millis).
    pub from_timestamp: Option<i64>,
    /// Inclusive upper bound on `timestamp` (unix millis).
    pub to_timestamp: Option<i64>,
    /// Maximum number of rows to return (newest first). `None` returns
    /// all matching rows.
    pub limit: Option<i64>,
}

/// Number of rows deleted in a single ring-buffer pruning pass when the
/// `max_entries` cap is exceeded (PRD section 4.8).
pub const RING_BUFFER_PRUNE_BATCH: i64 = 1_000;

/// Persistent SQLite-backed query log ring buffer.
///
/// Wraps a single SQLite connection guarded by a
/// [`Mutex`](std::sync::Mutex). The schema is created idempotently on
/// construction via [`SqliteQueryLogStore::open`] /
/// [`SqliteQueryLogStore::open_in_memory`].
pub struct SqliteQueryLogStore {
    conn: std::sync::Mutex<Connection>,
}

impl SqliteQueryLogStore {
    /// Open (or create) the query log database at `path`, creating the
    /// `query_log` table and indexes if they do not already exist. WAL
    /// mode is enabled for concurrent read access during writes.
    pub fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self, QueryLogStoreError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::init(&conn)?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    /// Create an in-memory query log (used by tests).
    pub fn open_in_memory() -> Result<Self, QueryLogStoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn init(conn: &Connection) -> Result<(), QueryLogStoreError> {
        // WAL mode enables concurrent readers while the writer inserts
        // entries. In-memory databases ignore the pragma (always memory),
        // so the call is safe to issue unconditionally.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS query_log (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp      INTEGER NOT NULL,
                client_ip      TEXT NOT NULL,
                client_name    TEXT,
                profile        TEXT,
                domain         TEXT NOT NULL,
                qtype          TEXT NOT NULL,
                response_code  TEXT NOT NULL,
                blocked        INTEGER NOT NULL,
                block_category TEXT,
                block_source   TEXT,
                tier           INTEGER,
                latency_ms     INTEGER,
                cached         INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_query_log_timestamp ON query_log(timestamp);
            CREATE INDEX IF NOT EXISTS idx_query_log_client   ON query_log(client_ip);
            CREATE INDEX IF NOT EXISTS idx_query_log_domain   ON query_log(domain);",
        )?;
        Ok(())
    }

    /// Insert a single entry. Returns the assigned row id.
    pub fn insert(&self, entry: &QueryLogEntry) -> Result<i64, QueryLogStoreError> {
        let conn = self.conn.lock().expect("query log mutex poisoned");
        conn.execute(
            "INSERT INTO query_log
                (timestamp, client_ip, client_name, profile, domain, qtype,
                 response_code, blocked, block_category, block_source, tier,
                 latency_ms, cached)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                entry.timestamp,
                entry.client_ip,
                entry.client_name,
                entry.profile,
                entry.domain,
                entry.qtype,
                entry.response_code,
                entry.blocked as i64,
                entry.block_category,
                entry.block_source,
                entry.tier,
                entry.latency_ms,
                entry.cached as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a batch of entries in a single transaction. Returns the
    /// number of rows inserted.
    pub fn insert_batch(
        &self,
        entries: &[QueryLogEntry],
    ) -> Result<usize, QueryLogStoreError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().expect("query log mutex poisoned");
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO query_log
                    (timestamp, client_ip, client_name, profile, domain, qtype,
                     response_code, blocked, block_category, block_source, tier,
                     latency_ms, cached)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for e in entries {
                stmt.execute(params![
                    e.timestamp,
                    e.client_ip,
                    e.client_name,
                    e.profile,
                    e.domain,
                    e.qtype,
                    e.response_code,
                    e.blocked as i64,
                    e.block_category,
                    e.block_source,
                    e.tier,
                    e.latency_ms,
                    e.cached as i64,
                ])?;
            }
        }
        tx.commit()?;
        Ok(entries.len())
    }

    /// Total number of rows in the query log.
    pub fn count_all(&self) -> Result<i64, QueryLogStoreError> {
        let conn = self.conn.lock().expect("query log mutex poisoned");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM query_log", [], |r| r.get(0))?;
        Ok(n)
    }

    /// Count entries matching `filter` without materializing rows.
    pub fn count(&self, filter: &QueryLogFilter) -> Result<i64, QueryLogStoreError> {
        let conn = self.conn.lock().expect("query log mutex poisoned");
        let (sql, args) = build_count_sql(filter);
        let n: i64 = conn.query_row(&sql, rusqlite::params_from_iter(args.iter()), |r| {
            r.get(0)
        })?;
        Ok(n)
    }

    /// Query entries matching `filter`, ordered newest-first.
    pub fn query(
        &self,
        filter: &QueryLogFilter,
    ) -> Result<Vec<QueryLogEntry>, QueryLogStoreError> {
        let conn = self.conn.lock().expect("query log mutex poisoned");
        let (sql, args) = build_select_sql(filter);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_to_entry)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Enforce the ring-buffer cap: if the row count exceeds
    /// `max_entries`, delete the oldest [`RING_BUFFER_PRUNE_BATCH`] rows
    /// (in a single `DELETE`). Returns the number of rows deleted.
    ///
    /// This is intended to be called periodically from a background
    /// cleanup task, not on the write hot path. Repeated calls will
    /// eventually bring the row count back under `max_entries`.
    pub fn prune_ring_buffer(&self, max_entries: i64) -> Result<i64, QueryLogStoreError> {
        let conn = self.conn.lock().expect("query log mutex poisoned");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM query_log", [], |r| r.get(0))?;
        if count <= max_entries {
            return Ok(0);
        }
        // Delete the lesser of the configured batch size and the actual
        // number of over-cap rows, so a single pass never deletes more
        // than necessary to reach the cap.
        let to_delete = std::cmp::min(RING_BUFFER_PRUNE_BATCH, count - max_entries);
        let deleted = conn.execute(
            "DELETE FROM query_log
             WHERE id IN (
                 SELECT id FROM query_log
                 ORDER BY id ASC
                 LIMIT ?1
             )",
            params![to_delete],
        )? as i64;
        Ok(deleted)
    }

    /// Delete entries older than `cutoff_millis` (unix millis). Returns
    /// the number of rows deleted. Used to enforce `retention_days`.
    pub fn prune_older_than(&self, cutoff_millis: i64) -> Result<i64, QueryLogStoreError> {
        let conn = self.conn.lock().expect("query log mutex poisoned");
        let deleted = conn.execute(
            "DELETE FROM query_log WHERE timestamp < ?1",
            params![cutoff_millis],
        )? as i64;
        Ok(deleted)
    }

    /// Convenience wrapper: delete entries older than `retention_days`
    /// days relative to `now_millis`.
    pub fn prune_by_age(
        &self,
        retention_days: u32,
        now_millis: i64,
    ) -> Result<i64, QueryLogStoreError> {
        if retention_days == 0 {
            return Ok(0);
        }
        let cutoff = now_millis - (retention_days as i64) * 86_400_000;
        self.prune_older_than(cutoff)
    }

    /// Export entries matching `filter` as CSV. The first row is a
    /// header naming every column; subsequent rows are the matching
    /// entries ordered newest-first. Fields containing commas, quotes,
    /// or newlines are RFC 4180-quoted.
    pub fn export_csv(
        &self,
        filter: &QueryLogFilter,
    ) -> Result<String, QueryLogStoreError> {
        let entries = self.query(filter)?;
        let mut out = String::new();
        out.push_str(
            "id,timestamp,client_ip,client_name,profile,domain,qtype,response_code,\
             blocked,block_category,block_source,tier,latency_ms,cached\n",
        );
        for e in entries {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                csv_field(&e.id.map(|i| i.to_string()).unwrap_or_default()),
                csv_field(&e.timestamp.to_string()),
                csv_field(&e.client_ip),
                csv_field_opt(&e.client_name),
                csv_field_opt(&e.profile),
                csv_field(&e.domain),
                csv_field(&e.qtype),
                csv_field(&e.response_code),
                e.blocked as i64,
                csv_field_opt(&e.block_category),
                csv_field_opt(&e.block_source),
                e.tier.map(|t| t.to_string()).unwrap_or_default(),
                e.latency_ms.map(|l| l.to_string()).unwrap_or_default(),
                e.cached as i64,
            ));
        }
        Ok(out)
    }
}

/// Map a SQLite row to a [`QueryLogEntry`].
fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueryLogEntry> {
    Ok(QueryLogEntry {
        id: Some(row.get(0)?),
        timestamp: row.get(1)?,
        client_ip: row.get(2)?,
        client_name: row.get(3)?,
        profile: row.get(4)?,
        domain: row.get(5)?,
        qtype: row.get(6)?,
        response_code: row.get(7)?,
        blocked: row.get::<_, i64>(8)? != 0,
        block_category: row.get(9)?,
        block_source: row.get(10)?,
        tier: row.get(11)?,
        latency_ms: row.get(12)?,
        cached: row.get::<_, i64>(13)? != 0,
    })
}

/// Build the `SELECT ... FROM query_log` SQL and bound args for `filter`.
fn build_select_sql(filter: &QueryLogFilter) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = String::from(
        "SELECT id, timestamp, client_ip, client_name, profile, domain, qtype,
                response_code, blocked, block_category, block_source, tier,
                latency_ms, cached
         FROM query_log WHERE 1=1",
    );
    let mut args = append_where(&mut sql, filter);
    sql.push_str(" ORDER BY timestamp DESC, id DESC");
    if let Some(limit) = filter.limit {
        sql.push_str(" LIMIT ?");
        args.push(rusqlite::types::Value::Integer(limit));
    }
    (sql, args)
}

/// Build the `SELECT COUNT(*)` SQL and bound args for `filter`.
fn build_count_sql(filter: &QueryLogFilter) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = String::from("SELECT COUNT(*) FROM query_log WHERE 1=1");
    let args = append_where(&mut sql, filter);
    (sql, args)
}

/// Append the shared `WHERE` clauses for `filter` to `sql`, returning
/// the bound argument list.
fn append_where(sql: &mut String, filter: &QueryLogFilter) -> Vec<rusqlite::types::Value> {
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(ref ip) = filter.client_ip {
        sql.push_str(" AND client_ip = ?");
        args.push(rusqlite::types::Value::Text(ip.clone()));
    }
    if let Some(ref dom) = filter.domain {
        sql.push_str(" AND domain = ?");
        args.push(rusqlite::types::Value::Text(dom.clone()));
    }
    if let Some(b) = filter.blocked {
        sql.push_str(" AND blocked = ?");
        args.push(rusqlite::types::Value::Integer(b as i64));
    }
    if let Some(c) = filter.cached {
        sql.push_str(" AND cached = ?");
        args.push(rusqlite::types::Value::Integer(c as i64));
    }
    if let Some(from) = filter.from_timestamp {
        sql.push_str(" AND timestamp >= ?");
        args.push(rusqlite::types::Value::Integer(from));
    }
    if let Some(to) = filter.to_timestamp {
        sql.push_str(" AND timestamp <= ?");
        args.push(rusqlite::types::Value::Integer(to));
    }
    args
}

/// RFC 4180-quote a required string field for CSV output.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// RFC 4180-quote an optional string field (empty when `None`).
fn csv_field_opt(s: &Option<String>) -> String {
    match s {
        Some(v) => csv_field(v),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_log::entry::QueryLogEntry;

    fn make_store() -> SqliteQueryLogStore {
        SqliteQueryLogStore::open_in_memory().expect("open in-memory query log")
    }

    fn sample_entry(ts: i64, ip: &str, domain: &str) -> QueryLogEntry {
        QueryLogEntry::new(ts, ip, domain, "A", "NOERROR")
    }

    #[test]
    fn insert_and_count() {
        let store = make_store();
        assert_eq!(store.count_all().unwrap(), 0);
        let id = store
            .insert(&sample_entry(100, "10.0.0.1", "a.com"))
            .unwrap();
        assert!(id > 0);
        assert_eq!(store.count_all().unwrap(), 1);
    }

    #[test]
    fn insert_batch_roundtrips() {
        let store = make_store();
        let entries: Vec<_> = (0..50)
            .map(|i| sample_entry(i, "10.0.0.1", &format!("d{i}.com")))
            .collect();
        let n = store.insert_batch(&entries).unwrap();
        assert_eq!(n, 50);
        assert_eq!(store.count_all().unwrap(), 50);

        let listed = store.query(&QueryLogFilter::default()).unwrap();
        assert_eq!(listed.len(), 50);
        // newest first
        assert_eq!(listed[0].timestamp, 49);
        assert_eq!(listed[49].timestamp, 0);
    }

    #[test]
    fn insert_batch_empty_is_noop() {
        let store = make_store();
        let n = store.insert_batch(&[]).unwrap();
        assert_eq!(n, 0);
        assert_eq!(store.count_all().unwrap(), 0);
    }

    #[test]
    fn query_filters_by_client_ip() {
        let store = make_store();
        store.insert(&sample_entry(1, "10.0.0.1", "a.com")).unwrap();
        store.insert(&sample_entry(2, "10.0.0.2", "b.com")).unwrap();
        store.insert(&sample_entry(3, "10.0.0.1", "c.com")).unwrap();

        let f = QueryLogFilter {
            client_ip: Some("10.0.0.1".to_string()),
            ..Default::default()
        };
        let listed = store.query(&f).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|e| e.client_ip == "10.0.0.1"));
        assert_eq!(store.count(&f).unwrap(), 2);
    }

    #[test]
    fn query_filters_by_domain() {
        let store = make_store();
        store.insert(&sample_entry(1, "10.0.0.1", "a.com")).unwrap();
        store.insert(&sample_entry(2, "10.0.0.1", "b.com")).unwrap();

        let f = QueryLogFilter {
            domain: Some("a.com".to_string()),
            ..Default::default()
        };
        let listed = store.query(&f).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].domain, "a.com");
    }

    #[test]
    fn query_filters_by_time_range() {
        let store = make_store();
        for ts in [10, 20, 30, 40, 50] {
            store.insert(&sample_entry(ts, "10.0.0.1", "x.com")).unwrap();
        }
        let f = QueryLogFilter {
            from_timestamp: Some(20),
            to_timestamp: Some(40),
            ..Default::default()
        };
        let listed = store.query(&f).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].timestamp, 40);
        assert_eq!(listed[2].timestamp, 20);
    }

    #[test]
    fn query_filters_blocked_and_cached() {
        let store = make_store();
        store.insert(&sample_entry(1, "c", "d")).unwrap();
        store
            .insert(
                &QueryLogEntry::new(2, "c", "d", "A", "NXDOMAIN")
                    .blocked(Some("ads".to_string()), Some("list".to_string())),
            )
            .unwrap();
        store.insert(&sample_entry(3, "c", "d").cached()).unwrap();

        let blocked = store
            .query(&QueryLogFilter {
                blocked: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(blocked.len(), 1);
        assert!(blocked[0].blocked);
        assert_eq!(blocked[0].block_category.as_deref(), Some("ads"));

        let cached = store
            .query(&QueryLogFilter {
                cached: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(cached.len(), 1);
        assert!(cached[0].cached);
    }

    #[test]
    fn query_limit() {
        let store = make_store();
        for ts in 0..10 {
            store.insert(&sample_entry(ts, "c", "d")).unwrap();
        }
        let f = QueryLogFilter {
            limit: Some(3),
            ..Default::default()
        };
        let listed = store.query(&f).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].timestamp, 9);
    }

    #[test]
    fn roundtrips_all_optional_fields() {
        let store = make_store();
        let e = QueryLogEntry::new(123, "10.0.0.5", "example.com", "AAAA", "NOERROR")
            .with_client_name("laptop")
            .with_profile("kids")
            .blocked(Some("social".to_string()), Some("sb".to_string()))
            .with_tier(2)
            .with_latency_ms(42);
        store.insert(&e).unwrap();
        let listed = store.query(&QueryLogFilter::default()).unwrap();
        assert_eq!(listed.len(), 1);
        let got = &listed[0];
        assert_eq!(got.client_name.as_deref(), Some("laptop"));
        assert_eq!(got.profile.as_deref(), Some("kids"));
        assert_eq!(got.block_category.as_deref(), Some("social"));
        assert_eq!(got.block_source.as_deref(), Some("sb"));
        assert_eq!(got.tier, Some(2));
        assert_eq!(got.latency_ms, Some(42));
        assert!(got.blocked);
        assert!(!got.cached);
        assert!(got.id.is_some());
    }

    #[test]
    fn ring_buffer_prunes_oldest_batch() {
        let store = make_store();
        // Insert 100 entries; cap at 50 -> prune deletes 1000-batch but
        // only 50 exist over cap, so a single pass brings us to 50.
        let entries: Vec<_> = (0..100)
            .map(|i| sample_entry(i, "c", &format!("d{i}.com")))
            .collect();
        store.insert_batch(&entries).unwrap();
        assert_eq!(store.count_all().unwrap(), 100);

        let deleted = store.prune_ring_buffer(50).unwrap();
        // Over-cap by 50; batch size is 1000, so all 50 over-cap deleted.
        assert_eq!(deleted, 50);
        assert_eq!(store.count_all().unwrap(), 50);

        // Newest 50 retained (timestamps 50..100).
        let listed = store.query(&QueryLogFilter::default()).unwrap();
        assert_eq!(listed.len(), 50);
        assert_eq!(listed[0].timestamp, 99);
        assert_eq!(listed[49].timestamp, 50);
    }

    #[test]
    fn ring_buffer_noop_when_under_cap() {
        let store = make_store();
        store.insert(&sample_entry(1, "c", "d")).unwrap();
        let deleted = store.prune_ring_buffer(100).unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(store.count_all().unwrap(), 1);
    }

    #[test]
    fn ring_buffer_prunes_in_batches() {
        let store = make_store();
        // Insert 2500 entries; cap at 1000. Over-cap by 1500.
        let entries: Vec<_> = (0..2500)
            .map(|i| sample_entry(i, "c", &format!("d{i}.com")))
            .collect();
        store.insert_batch(&entries).unwrap();
        assert_eq!(store.count_all().unwrap(), 2500);

        // First pass: deletes min(1000, 1500) = 1000.
        let d1 = store.prune_ring_buffer(1000).unwrap();
        assert_eq!(d1, 1000);
        assert_eq!(store.count_all().unwrap(), 1500);

        // Second pass: deletes min(1000, 500) = 500.
        let d2 = store.prune_ring_buffer(1000).unwrap();
        assert_eq!(d2, 500);
        assert_eq!(store.count_all().unwrap(), 1000);

        // Third pass: at cap, noop.
        let d3 = store.prune_ring_buffer(1000).unwrap();
        assert_eq!(d3, 0);
    }

    #[test]
    fn prune_by_age_deletes_old_entries() {
        let store = make_store();
        // now = 10_000_000 ms. retention 7 days -> cutoff = now - 7*86400000.
        let now = 10_000_000_i64;
        let cutoff = now - 7 * 86_400_000;
        store.insert(&sample_entry(cutoff - 1_000, "c", "old.com")).unwrap();
        store.insert(&sample_entry(cutoff, "c", "edge.com")).unwrap();
        store.insert(&sample_entry(cutoff + 1_000, "c", "new.com")).unwrap();

        let deleted = store.prune_by_age(7, now).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.count_all().unwrap(), 2);
        let listed = store.query(&QueryLogFilter::default()).unwrap();
        assert!(listed.iter().all(|e| e.domain != "old.com"));
    }

    #[test]
    fn prune_by_age_zero_is_noop() {
        let store = make_store();
        store.insert(&sample_entry(1, "c", "d")).unwrap();
        let deleted = store.prune_by_age(0, 1_000_000).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn export_csv_header_and_rows() {
        let store = make_store();
        store
            .insert(&QueryLogEntry::new(100, "10.0.0.1", "a.com", "A", "NOERROR"))
            .unwrap();
        store
            .insert(
                &QueryLogEntry::new(200, "10.0.0.2", "b.com", "AAAA", "NXDOMAIN")
                    .blocked(Some("ads".to_string()), Some("sb".to_string()))
                    .with_tier(1)
                    .with_latency_ms(5),
            )
            .unwrap();
        let csv = store.export_csv(&QueryLogFilter::default()).unwrap();
        let lines: Vec<&str> = csv.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 3); // header + 2 rows
        assert!(lines[0].starts_with("id,timestamp,client_ip,"));
        // newest first
        assert!(lines[1].contains("10.0.0.2"));
        assert!(lines[1].contains("b.com"));
        assert!(lines[1].contains(",1,")); // blocked=1
        assert!(lines[2].contains("10.0.0.1"));
        // blocked=0, block_category="", block_source="", tier="",
        // latency_ms="", cached=0 -> trailing "0,,,,,0"
        assert!(lines[2].ends_with("0,,,,,0"));
    }

    #[test]
    fn export_csv_quotes_commas() {
        let store = make_store();
        // client_name with a comma forces quoting.
        store
            .insert(
                &QueryLogEntry::new(1, "10.0.0.1", "a.com", "A", "NOERROR")
                    .with_client_name("last,first"),
            )
            .unwrap();
        let csv = store.export_csv(&QueryLogFilter::default()).unwrap();
        assert!(csv.contains("\"last,first\""));
    }

    #[test]
    fn export_csv_respects_filter() {
        let store = make_store();
        store.insert(&sample_entry(1, "10.0.0.1", "a.com")).unwrap();
        store.insert(&sample_entry(2, "10.0.0.2", "b.com")).unwrap();
        let csv = store
            .export_csv(&QueryLogFilter {
                client_ip: Some("10.0.0.1".to_string()),
                ..Default::default()
            })
            .unwrap();
        let lines: Vec<&str> = csv.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 2); // header + 1 row
        assert!(lines[1].contains("10.0.0.1"));
    }
}
