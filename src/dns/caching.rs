//! Response caching with TTL clamping.
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
//! works" acceptance criterion. Serve-stale (RFC 8767) is story 03-002.

use crate::config::CacheConfig;
use hickory_resolver::config::ResolverOpts;
use tracing::info;

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
}
