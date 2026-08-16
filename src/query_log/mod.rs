//! Query log: SQLite ring buffer + batched writer.
//!
//! [`QueryLogger`] is the high-level facade used by the DNS handler chain.
//! It wraps a [`SqliteQueryLogStore`] and provides batched writes (every
//! `batch_size` entries or `flush_interval`, whichever comes first) so
//! that the DNS hot path does not issue a SQLite write per query. A
//! background cleanup enforces the ring-buffer (`max_entries`) and
//! time-based (`retention_days`) retention caps.
//!
//! The schema and ring-buffer behavior follow PRD section 4.8
//! (lines 1197-1277): every DNS query is logged with timestamp, client
//! identity, domain, query type, response code, block metadata, upstream
//! tier, latency, and cache-hit flag.
//!
//! ## Concurrency
//!
//! [`QueryLogger`] is `Send + Sync`: the write buffer is guarded by a
//! [`Mutex`](std::sync::Mutex) and the underlying SQLite connection by
//! the store's own mutex. Buffered entries are flushed synchronously via
//! [`QueryLogger::flush`]; a production deployment should also call
//! `flush` on shutdown to avoid losing buffered entries.

pub mod entry;
pub mod store;

pub use entry::QueryLogEntry;
pub use store::{QueryLogFilter, QueryLogStoreError, SqliteQueryLogStore};

use std::sync::Mutex;

/// Default batch size: flush after this many buffered entries (PRD 4.8).
pub const DEFAULT_BATCH_SIZE: usize = 100;

/// Default flush interval in milliseconds (PRD 4.8: 1 second).
pub const DEFAULT_FLUSH_INTERVAL_MS: u64 = 1_000;

/// Configuration snapshot used by [`QueryLogger`] for retention. Mirrors
/// the relevant fields of [`crate::config::QueryLogConfig`].
#[derive(Debug, Clone)]
pub struct QueryLogRetention {
    /// Maximum number of entries to retain (ring-buffer cap).
    pub max_entries: usize,
    /// Time-based retention in days. Entries older than this are deleted.
    pub retention_days: u32,
}

impl QueryLogRetention {
    /// Build a retention snapshot from a [`crate::config::QueryLogConfig`].
    pub fn from_config(cfg: &crate::config::QueryLogConfig) -> Self {
        Self {
            max_entries: cfg.max_entries,
            retention_days: cfg.retention_days,
        }
    }
}

/// Buffered write state held behind the query logger's mutex.
struct Buffer {
    pending: Vec<QueryLogEntry>,
    last_flush: std::time::Instant,
}

/// High-level query log facade: batched writes + retention cleanup over a
/// [`SqliteQueryLogStore`].
///
/// Call [`QueryLogger::record_query`] on the DNS hot path; entries are
/// buffered and flushed in batches to minimize SQLite I/O. Call
/// [`QueryLogger::flush`] periodically (and on shutdown) to drain the
/// buffer, and [`QueryLogger::run_cleanup`] to enforce retention caps.
pub struct QueryLogger {
    store: SqliteQueryLogStore,
    buffer: Mutex<Buffer>,
    batch_size: usize,
    flush_interval: std::time::Duration,
    retention: QueryLogRetention,
}

impl QueryLogger {
    /// Create a query logger backed by an in-memory store (tests only).
    pub fn new_in_memory(retention: QueryLogRetention) -> Result<Self, QueryLogStoreError> {
        Self::with_store(
            SqliteQueryLogStore::open_in_memory()?,
            DEFAULT_BATCH_SIZE,
            DEFAULT_FLUSH_INTERVAL_MS,
            retention,
        )
    }

    /// Create a query logger backed by a persistent store at `path`.
    pub fn open<P: AsRef<std::path::Path>>(
        path: P,
        retention: QueryLogRetention,
    ) -> Result<Self, QueryLogStoreError> {
        Self::with_store(
            SqliteQueryLogStore::open(path)?,
            DEFAULT_BATCH_SIZE,
            DEFAULT_FLUSH_INTERVAL_MS,
            retention,
        )
    }

    /// Assemble a logger from an already-open store, custom batch size,
    /// flush interval (ms), and retention policy.
    pub fn with_store(
        store: SqliteQueryLogStore,
        batch_size: usize,
        flush_interval_ms: u64,
        retention: QueryLogRetention,
    ) -> Result<Self, QueryLogStoreError> {
        Ok(Self {
            store,
            buffer: Mutex::new(Buffer {
                pending: Vec::with_capacity(batch_size.max(1)),
                last_flush: std::time::Instant::now(),
            }),
            batch_size: batch_size.max(1),
            flush_interval: std::time::Duration::from_millis(flush_interval_ms.max(1)),
            retention,
        })
    }

    /// Record a query entry. The entry is buffered; the buffer is flushed
    /// automatically once `batch_size` entries accumulate or the flush
    /// interval elapses. Returns the number of entries flushed (0 if
    /// still buffered).
    pub fn record_query(&self, entry: QueryLogEntry) -> Result<usize, QueryLogStoreError> {
        let mut buf = self.buffer.lock().expect("query log buffer poisoned");
        buf.pending.push(entry);
        if buf.pending.len() >= self.batch_size
            || buf.last_flush.elapsed() >= self.flush_interval
        {
            let entries = std::mem::take(&mut buf.pending);
            buf.last_flush = std::time::Instant::now();
            drop(buf);
            let n = self.store.insert_batch(&entries)?;
            return Ok(n);
        }
        Ok(0)
    }

    /// Flush any buffered entries to the store immediately. Returns the
    /// number of entries written. Call this on shutdown to avoid losing
    /// buffered data.
    pub fn flush(&self) -> Result<usize, QueryLogStoreError> {
        let mut buf = self.buffer.lock().expect("query log buffer poisoned");
        if buf.pending.is_empty() {
            return Ok(0);
        }
        let entries = std::mem::take(&mut buf.pending);
        buf.last_flush = std::time::Instant::now();
        drop(buf);
        self.store.insert_batch(&entries)
    }

    /// Run retention cleanup: enforce the ring-buffer cap
    /// (`max_entries`) and the time-based cap (`retention_days`). Returns
    /// the total number of entries deleted. Intended to be called
    /// periodically from a background task, not on the write hot path.
    pub fn run_cleanup(&self) -> Result<i64, QueryLogStoreError> {
        self.run_cleanup_at(QueryLogEntry::now_millis())
    }

    /// Like [`QueryLogger::run_cleanup`] but uses `now_millis` as the
    /// reference time for the `retention_days` cutoff. Useful for tests
    /// that need deterministic time.
    pub fn run_cleanup_at(&self, now_millis: i64) -> Result<i64, QueryLogStoreError> {
        let mut deleted = 0;
        // Repeatedly prune the ring buffer until we are at or below the
        // cap (each call deletes at most RING_BUFFER_PRUNE_BATCH rows).
        loop {
            let d = self.store.prune_ring_buffer(self.retention.max_entries as i64)?;
            if d == 0 {
                break;
            }
            deleted += d;
        }
        // Time-based retention.
        if self.retention.retention_days > 0 {
            deleted += self
                .store
                .prune_by_age(self.retention.retention_days, now_millis)?;
        }
        Ok(deleted)
    }

    /// Delegate: query entries matching `filter` (newest-first).
    pub fn query(
        &self,
        filter: &QueryLogFilter,
    ) -> Result<Vec<QueryLogEntry>, QueryLogStoreError> {
        self.store.query(filter)
    }

    /// Delegate: count entries matching `filter`.
    pub fn count(&self, filter: &QueryLogFilter) -> Result<i64, QueryLogStoreError> {
        self.store.count(filter)
    }

    /// Delegate: total row count.
    pub fn count_all(&self) -> Result<i64, QueryLogStoreError> {
        self.store.count_all()
    }

    /// Delegate: export matching entries as CSV.
    pub fn export_csv(
        &self,
        filter: &QueryLogFilter,
    ) -> Result<String, QueryLogStoreError> {
        self.store.export_csv(filter)
    }

    /// Delegate: insert an entry directly (bypassing the batch buffer).
    /// Mainly useful for tests; production code should use
    /// [`QueryLogger::record_query`].
    pub fn insert_direct(&self, entry: &QueryLogEntry) -> Result<i64, QueryLogStoreError> {
        self.store.insert(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_log::entry::QueryLogEntry;

    fn retention() -> QueryLogRetention {
        QueryLogRetention {
            max_entries: 100_000,
            retention_days: 7,
        }
    }

    fn sample(ts: i64, ip: &str, dom: &str) -> QueryLogEntry {
        QueryLogEntry::new(ts, ip, dom, "A", "NOERROR")
    }

    #[test]
    fn batches_flush_at_batch_size() {
        let logger = QueryLogger::new_in_memory(retention()).unwrap();
        // batch_size = 100; first 99 are buffered.
        for i in 0..99 {
            assert_eq!(logger.record_query(sample(i, "c", "d.com")).unwrap(), 0);
        }
        assert_eq!(logger.count_all().unwrap(), 0);
        // 100th entry triggers a flush.
        assert_eq!(
            logger.record_query(sample(99, "c", "d.com")).unwrap(),
            100
        );
        assert_eq!(logger.count_all().unwrap(), 100);
    }

    #[test]
    fn flush_drains_buffer() {
        let logger = QueryLogger::new_in_memory(retention()).unwrap();
        for i in 0..10 {
            logger.record_query(sample(i, "c", "d.com")).unwrap();
        }
        assert_eq!(logger.count_all().unwrap(), 0);
        let n = logger.flush().unwrap();
        assert_eq!(n, 10);
        assert_eq!(logger.count_all().unwrap(), 10);

        // Second flush is a noop.
        assert_eq!(logger.flush().unwrap(), 0);
    }

    #[test]
    fn flush_on_empty_is_noop() {
        let logger = QueryLogger::new_in_memory(retention()).unwrap();
        assert_eq!(logger.flush().unwrap(), 0);
    }

    #[test]
    fn run_cleanup_enforces_ring_buffer() {
        let r = QueryLogRetention {
            max_entries: 50,
            retention_days: 0, // disable time-based for this test
        };
        let logger = QueryLogger::new_in_memory(r).unwrap();
        // Insert 100 directly via the store to bypass batching.
        let entries: Vec<_> = (0..100)
            .map(|i| sample(i, "c", &format!("d{i}.com")))
            .collect();
        logger.store.insert_batch(&entries).unwrap();
        assert_eq!(logger.count_all().unwrap(), 100);

        let deleted = logger.run_cleanup().unwrap();
        assert_eq!(deleted, 50);
        assert_eq!(logger.count_all().unwrap(), 50);
    }

    #[test]
    fn run_cleanup_enforces_retention_days() {
        let r = QueryLogRetention {
            max_entries: 100_000,
            retention_days: 1,
        };
        let logger = QueryLogger::new_in_memory(r).unwrap();
        let now = QueryLogEntry::now_millis();
        let cutoff = now - 86_400_000;
        // old (before cutoff), edge (== cutoff, kept), new (after cutoff).
        logger.store.insert(&sample(cutoff - 5_000, "c", "old.com")).unwrap();
        logger.store.insert(&sample(cutoff, "c", "edge.com")).unwrap();
        logger.store.insert(&sample(cutoff + 5_000, "c", "new.com")).unwrap();

        let deleted = logger.run_cleanup_at(now).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(logger.count_all().unwrap(), 2);
    }

    #[test]
    fn query_and_export_through_logger() {
        let logger = QueryLogger::new_in_memory(retention()).unwrap();
        logger
            .record_query(sample(1, "10.0.0.1", "a.com"))
            .unwrap();
        logger.flush().unwrap();
        let listed = logger
            .query(&QueryLogFilter {
                client_ip: Some("10.0.0.1".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(logger.count(&QueryLogFilter::default()).unwrap(), 1);

        let csv = logger.export_csv(&QueryLogFilter::default()).unwrap();
        assert!(csv.starts_with("id,timestamp,client_ip,"));
        assert!(csv.contains("a.com"));
    }

    #[test]
    fn from_config_snapshot() {
        use crate::config::QueryLogConfig;
        let cfg = QueryLogConfig {
            enabled: true,
            max_entries: 50_000,
            retention_days: 3,
        };
        let r = QueryLogRetention::from_config(&cfg);
        assert_eq!(r.max_entries, 50_000);
        assert_eq!(r.retention_days, 3);
    }
}
