//! Response caching with TTL clamping and a separate stale-response cache
//! for serve-stale (RFC 8767, story 03-002).
//!
//! In hickory-server 0.26 the `ForwardZoneHandler` owns its
//! `hickory-resolver` resolver, and caching is configured on the resolver's
//! [`ResolverOpts`](hickory_resolver::config::ResolverOpts) rather than via a
//! separate `CachingClient` wrapper. This module applies the `[cache]`
//! settings (`min_ttl`, `max_ttl`, `negative_ttl`, `max_entries`) to those
//! options, which is the 0.26 equivalent of the story's "CachingHandler
//! wrapping CachingClient for LRU caching with TTL clamping".
//!
//! A second query for the same domain is served from the resolver's in-memory
//! LRU cache without re-contacting the upstream — satisfying the "caching
//! works" acceptance criterion.
//!
//! ## Stale cache (story 03-002)
//!
//! hickory-resolver's [`ResponseCache`](hickory_resolver::cache::ResponseCache)
//! does not expose expired entries — its `get()` returns `None` once the TTL
//! has elapsed (the entry is evicted by the moka expiry policy). RFC 8767
//! requires serving expired entries when all upstreams are unavailable, so we
//! maintain a **separate** [`StaleCache`] that retains successful responses
//! beyond their original TTL for up to `serve_stale_ttl` seconds. The
//! [`ServeStaleHandler`](crate::dns::serve_stale::ServeStaleHandler) populates
//! this cache from successful upstream responses and reads from it when the
//! catalog returns `SERVFAIL`.

use crate::config::CacheConfig;
use hickory_proto::op::{LowerQuery, Message, ResponseCode};
use hickory_proto::rr::RecordType;
use hickory_resolver::config::ResolverOpts;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::info;

/// A cache key: (lowercased query name, query type).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// Lowercased query name without trailing dot, e.g. `"example.com"`.
    pub name: String,
    /// Query record type (A, AAAA, …).
    pub qtype: RecordType,
}

impl CacheKey {
    /// Build a cache key from a hickory [`LowerQuery`] (as found in a
    /// [`Request`](hickory_server::server::Request)).
    ///
    /// The name is lowercased and the trailing dot is stripped so that
    /// `Example.COM.` and `example.com` collide.
    pub fn from_query(query: &LowerQuery) -> Self {
        Self {
            name: query
                .name()
                .to_string()
                .trim_end_matches('.')
                .to_ascii_lowercase(),
            qtype: query.query_type(),
        }
    }

    /// Build a cache key from a raw name string and record type.
    ///
    /// Convenience for tests and callers that already have the name as a
    /// string.
    pub fn from_name_qtype(name: &str, qtype: RecordType) -> Self {
        Self {
            name: name.trim_end_matches('.').to_ascii_lowercase(),
            qtype,
        }
    }
}

/// A cached response entry with its insertion time and original TTL.
#[derive(Debug, Clone)]
struct StaleEntry {
    /// The wire-format response message.
    message: Message,
    /// When the entry was inserted (i.e. when the upstream response was
    /// received).
    inserted_at: Instant,
    /// The original TTL (seconds) of the response, derived from the minimum
    /// TTL across answer records. Used to determine freshness vs. staleness.
    original_ttl: Duration,
}

impl StaleEntry {
    /// Returns `true` if the entry is still within its original TTL at `now`.
    fn is_fresh(&self, now: Instant) -> bool {
        now.duration_since(self.inserted_at) < self.original_ttl
    }

    /// Returns `true` if the entry is expired but still within the
    /// `serve_stale_ttl` window at `now`.
    fn is_stale(&self, now: Instant, serve_stale_ttl: Duration) -> bool {
        let elapsed = now.duration_since(self.inserted_at);
        elapsed >= self.original_ttl && elapsed < self.original_ttl + serve_stale_ttl
    }
}

/// A thread-safe cache of DNS responses retained for serve-stale (RFC 8767).
///
/// Unlike hickory-resolver's `ResponseCache`, this cache keeps entries *after*
/// their TTL expires so that [`ServeStaleHandler`](crate::dns::serve_stale::ServeStaleHandler)
/// can serve them when all upstreams are unavailable. Entries are evicted
/// once they exceed `original_ttl + serve_stale_ttl`.
///
/// Only successful responses (NOERROR with at least one answer record) are
/// cached — negative responses and errors are not served stale.
pub struct StaleCache {
    entries: RwLock<HashMap<CacheKey, StaleEntry>>,
}

impl StaleCache {
    /// Create an empty stale cache.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Store a successful upstream response in the stale cache.
    ///
    /// The `original_ttl` is derived from the minimum TTL across the answer
    /// records in `message`. If the message has no answer records or is not a
    /// NOERROR response, it is not cached.
    pub fn insert(&self, key: CacheKey, message: Message, now: Instant) {
        // Only cache positive responses (NOERROR with answers).
        if message.metadata.response_code != ResponseCode::NoError {
            return;
        }
        if message.answers.is_empty() {
            return;
        }

        let original_ttl = message
            .answers
            .iter()
            .map(|r| Duration::from_secs(u64::from(r.ttl)))
            .min()
            .unwrap_or(Duration::from_secs(0));

        let entry = StaleEntry {
            message,
            inserted_at: now,
            original_ttl,
        };

        self.entries.write().insert(key, entry);
    }

    /// Retrieve a **fresh** (unexpired) cached response for `key`.
    ///
    /// Returns `Some(Message)` if the entry exists and is still within its
    /// original TTL at `now`.
    pub fn get_fresh(&self, key: &CacheKey, now: Instant) -> Option<Message> {
        self.entries.read().get(key).and_then(|e| {
            if e.is_fresh(now) {
                Some(e.message.clone())
            } else {
                None
            }
        })
    }

    /// Retrieve an **expired** (stale) cached response for `key`.
    ///
    /// Returns `Some(Message)` if the entry exists, is past its original TTL,
    /// and is still within the `serve_stale_ttl` window at `now`. This is the
    /// RFC 8767 serve-stale path: the entry is no longer fresh but is served
    /// when upstream is unavailable.
    pub fn get_stale(
        &self,
        key: &CacheKey,
        now: Instant,
        serve_stale_ttl: Duration,
    ) -> Option<Message> {
        self.entries.read().get(key).and_then(|e| {
            if e.is_stale(now, serve_stale_ttl) {
                Some(e.message.clone())
            } else {
                None
            }
        })
    }

    /// Remove entries that have exceeded `original_ttl + serve_stale_ttl`.
    ///
    /// Called opportunistically to bound memory growth.
    pub fn evict_expired(&self, now: Instant, serve_stale_ttl: Duration) {
        let mut entries = self.entries.write();
        entries.retain(|_, e| {
            now.duration_since(e.inserted_at) < e.original_ttl + serve_stale_ttl
        });
    }

    /// Returns the number of entries currently in the cache.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }
}

impl Default for StaleCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Applies `[cache]` settings to a resolver's [`ResolverOpts`].
///
/// This is the 0.26 equivalent of wrapping `CachingClient`: it enables the
/// LRU response cache, sets the cache capacity, and clamps positive and
/// negative response TTLs to the configured bounds.
pub struct CachingHandler;

impl CachingHandler {
    /// Mutate `opts` in place to enable caching per `cache` config.
    pub fn apply(opts: &mut ResolverOpts, cache: &CacheConfig) {
        // Enable the in-memory LRU response cache. `cache_size` is a plain
        // `u64` in hickory-resolver 0.26 (0 disables caching).
        opts.cache_size = cache.max_entries as u64;

        // Clamp positive response TTLs to [min_ttl, max_ttl].
        opts.positive_min_ttl = Some(std::time::Duration::from_secs(cache.min_ttl));
        opts.positive_max_ttl = Some(std::time::Duration::from_secs(cache.max_ttl));

        // Clamp negative response (NXDOMAIN/NODATA) TTL.
        opts.negative_min_ttl = Some(std::time::Duration::from_secs(0));
        opts.negative_max_ttl = Some(std::time::Duration::from_secs(cache.negative_ttl));

        info!(
            max_entries = cache.max_entries,
            min_ttl = cache.min_ttl,
            max_ttl = cache.max_ttl,
            negative_ttl = cache.negative_ttl,
            "configured response cache"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{Message, MessageType, OpCode};
    use hickory_proto::rr::{Name, RData, Record, RecordType};
    use std::net::Ipv4Addr;

    /// Build a NOERROR response Message with a single A record at `ttl`.
    fn make_response(name: &str, ttl: u32, ip: Ipv4Addr) -> Message {
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NoError;
        let record = Record::from_rdata(
            Name::from_utf8(name).unwrap(),
            ttl,
            RData::A(ip.into()),
        );
        msg.add_answer(record);
        msg
    }

    #[test]
    fn applies_cache_bounds() {
        let cache = CacheConfig {
            min_ttl: 30,
            max_ttl: 600,
            negative_ttl: 60,
            serve_stale: false,
            serve_stale_ttl: 0,
            max_entries: 4096,
        };
        let mut opts = ResolverOpts::default();
        CachingHandler::apply(&mut opts, &cache);

        assert_eq!(opts.cache_size, 4096);
        assert_eq!(opts.positive_min_ttl, Some(std::time::Duration::from_secs(30)));
        assert_eq!(opts.positive_max_ttl, Some(std::time::Duration::from_secs(600)));
        assert_eq!(opts.negative_max_ttl, Some(std::time::Duration::from_secs(60)));
    }

    #[test]
    fn stale_cache_stores_and_retrieves_fresh() {
        let cache = StaleCache::new();
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        let now = Instant::now();
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));
        cache.insert(key.clone(), msg.clone(), now);

        // Fresh within TTL.
        let fresh = cache.get_fresh(&key, now);
        assert!(fresh.is_some());
        // Not stale yet.
        let stale = cache.get_stale(&key, now, Duration::from_secs(86400));
        assert!(stale.is_none());
    }

    #[test]
    fn stale_cache_returns_stale_after_ttl() {
        let cache = StaleCache::new();
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        // Insert with TTL of 0 seconds — immediately expired.
        let now = Instant::now();
        let msg = make_response("example.com.", 0, Ipv4Addr::new(1, 2, 3, 4));
        cache.insert(key.clone(), msg, now);

        // Not fresh (TTL 0).
        assert!(cache.get_fresh(&key, now).is_none());
        // Stale within serve_stale_ttl window.
        let stale = cache.get_stale(&key, now, Duration::from_secs(86400));
        assert!(stale.is_some());
    }

    #[test]
    fn stale_cache_evicts_beyond_serve_stale_window() {
        let cache = StaleCache::new();
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        let now = Instant::now();
        let msg = make_response("example.com.", 0, Ipv4Addr::new(1, 2, 3, 4));
        cache.insert(key.clone(), msg, now);

        let serve_stale_ttl = Duration::from_secs(60);
        // Evict with a very large "now" — everything should be gone.
        let future = now + Duration::from_secs(120);
        cache.evict_expired(future, serve_stale_ttl);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn stale_cache_does_not_store_negative_responses() {
        let cache = StaleCache::new();
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        let now = Instant::now();
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NXDomain;
        cache.insert(key.clone(), msg, now);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn stale_cache_does_not_store_empty_responses() {
        let cache = StaleCache::new();
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        let now = Instant::now();
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NoError;
        // No answer records.
        cache.insert(key.clone(), msg, now);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_key_normalizes_name() {
        let k1 = CacheKey::from_name_qtype("Example.COM.", RecordType::A);
        let k2 = CacheKey::from_name_qtype("example.com", RecordType::A);
        assert_eq!(k1, k2);
    }
}
