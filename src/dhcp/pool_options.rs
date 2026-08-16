//! Per-pool / per-subnet DHCP option overrides and per-profile lease times.
//!
//! Different IP pools (guest, IoT, main) can carry different DHCP options
//! (DNS server, router, NTP, domain search) and different lease times. This
//! module merges global options with pool-specific overrides — pool options
//! win on conflict — and resolves the effective lease time for a client given
//! its pool and (optionally) its profile.
//!
//! This implements PRD section 4.4 (lines 403-429):
//! - per-pool options override global options
//! - lease time per-profile (shorter for kids, longer for IoT)

use crate::dhcp::options::arbitrary::{ArbitraryOption, ArbitraryOptionError};
use std::collections::HashMap;

/// A set of options and a lease time associated with a pool or the global
/// default.
#[derive(Debug, Clone, Default)]
pub struct PoolOptionSet {
    /// Arbitrary DHCP options keyed by option code (string form, as in the
    /// TOML `options` map).
    pub options: HashMap<String, String>,
    /// Lease time in seconds.
    pub lease_time_secs: u32,
}

impl PoolOptionSet {
    /// Create an empty set with zero lease time.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an option (`code` → `value`).
    pub fn with_option(mut self, code: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.insert(code.into(), value.into());
        self
    }

    /// Set the lease time (seconds).
    pub fn with_lease_time(mut self, secs: u32) -> Self {
        self.lease_time_secs = secs;
        self
    }
}

/// Resolves effective DHCP options and lease times for a client.
///
/// Holds the global defaults, per-pool overrides, and per-profile lease-time
/// overrides. The DHCP server consults this when building an OFFER/ACK.
#[derive(Debug, Clone, Default)]
pub struct PerPoolOptions {
    /// Global options applied to every pool unless overridden.
    pub global: PoolOptionSet,
    /// Per-pool option sets, keyed by pool name.
    pub pools: HashMap<String, PoolOptionSet>,
    /// Per-profile lease-time overrides (seconds), keyed by profile name.
    /// e.g. `{"kids": 3600, "iot": 604800}`.
    pub profile_lease_overrides: HashMap<String, u32>,
}

impl PerPoolOptions {
    /// Create an empty `PerPoolOptions` (no pools, no overrides).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the global option set.
    pub fn with_global(mut self, global: PoolOptionSet) -> Self {
        self.global = global;
        self
    }

    /// Register a pool's option set.
    pub fn with_pool(mut self, name: impl Into<String>, options: PoolOptionSet) -> Self {
        self.pools.insert(name.into(), options);
        self
    }

    /// Register a per-profile lease-time override.
    pub fn with_profile_lease(mut self, profile: impl Into<String>, secs: u32) -> Self {
        self.profile_lease_overrides.insert(profile.into(), secs);
        self
    }

    /// Resolve the effective options for `pool_name`: global options merged
    /// with pool-specific overrides (pool wins on conflict).
    ///
    /// Returns the merged map as parsed [`ArbitraryOption`]s. Parse errors
    /// for individual options are logged and skipped so one bad entry does
    /// not break the whole pool.
    pub fn resolve(&self, pool_name: &str) -> Vec<ArbitraryOption> {
        let merged = self.merged_raw(pool_name);
        let mut opts = Vec::with_capacity(merged.len());
        for (code, value) in &merged {
            match ArbitraryOption::parse(code, value) {
                Ok(opt) => opts.push(opt),
                Err(e) => {
                    tracing::warn!(
                        pool = %pool_name,
                        code = %code,
                        error = %e,
                        "skipping unparseable DHCP option"
                    );
                }
            }
        }
        opts
    }

    /// Resolve the raw merged option map (code-string → value-string) for
    /// `pool_name`, with pool overrides applied on top of globals.
    pub fn merged_raw(&self, pool_name: &str) -> HashMap<String, String> {
        let mut merged = self.global.options.clone();
        if let Some(pool) = self.pools.get(pool_name) {
            for (code, value) in &pool.options {
                merged.insert(code.clone(), value.clone());
            }
        }
        merged
    }

    /// Resolve the effective lease time (seconds) for a client.
    ///
    /// Precedence (highest first):
    /// 1. per-profile override (if `profile` is set and present)
    /// 2. pool lease time (if the pool is configured with a non-zero time)
    /// 3. global lease time
    /// 4. `fallback` (the caller-supplied default, e.g. from `[dhcp]`)
    pub fn resolve_lease_time(
        &self,
        pool_name: &str,
        profile: Option<&str>,
        fallback: u32,
    ) -> u32 {
        if let Some(profile) = profile {
            if let Some(&secs) = self.profile_lease_overrides.get(profile) {
                return secs;
            }
        }
        if let Some(pool) = self.pools.get(pool_name) {
            if pool.lease_time_secs > 0 {
                return pool.lease_time_secs;
            }
        }
        if self.global.lease_time_secs > 0 {
            return self.global.lease_time_secs;
        }
        fallback
    }
}

/// Parse a TOML `options` map (`code-string → value-string`) into a
/// [`PoolOptionSet`], recording any parse errors.
pub fn parse_option_map(
    raw: &HashMap<String, String>,
    lease_time_secs: u32,
) -> Result<PoolOptionSet, ArbitraryOptionError> {
    // Validate every entry up front so a config error surfaces at load time
    // rather than at request time.
    for (code, value) in raw {
        ArbitraryOption::parse(code, value)?;
    }
    Ok(PoolOptionSet {
        options: raw.clone(),
        lease_time_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn global() -> PoolOptionSet {
        PoolOptionSet::new()
            .with_lease_time(86_400) // 24h
            .with_option("6", "192.168.1.67") // DNS
            .with_option("15", "levonk.com") // domain
    }

    fn guest_pool() -> PoolOptionSet {
        PoolOptionSet::new()
            .with_lease_time(14_400) // 4h
            .with_option("6", "192.168.10.67") // override DNS
    }

    fn per_pool() -> PerPoolOptions {
        PerPoolOptions::new()
            .with_global(global())
            .with_pool("guest", guest_pool())
            .with_pool("main", PoolOptionSet::new().with_lease_time(86_400))
            .with_profile_lease("kids", 3600) // 1h
            .with_profile_lease("iot", 604_800) // 7d
    }

    #[test]
    fn merge_global_with_pool_overrides() {
        let pp = per_pool();
        let merged = pp.merged_raw("guest");
        // Pool override wins.
        assert_eq!(merged.get("6"), Some(&"192.168.10.67".to_string()));
        // Global option not overridden is retained.
        assert_eq!(merged.get("15"), Some(&"levonk.com".to_string()));
    }

    #[test]
    fn resolve_returns_parsed_options() {
        let pp = per_pool();
        let opts = pp.resolve("guest");
        let dns = opts.iter().find(|o| o.code == 6).expect("option 6 present");
        assert_eq!(dns.value, crate::dhcp::options::arbitrary::OptionValue::Ip(
            Ipv4Addr::new(192, 168, 10, 67)
        ));
        let domain = opts.iter().find(|o| o.code == 15).expect("option 15 present");
        assert_eq!(
            domain.value,
            crate::dhcp::options::arbitrary::OptionValue::String(b"levonk.com".to_vec())
        );
    }

    #[test]
    fn unknown_pool_returns_global_only() {
        let pp = per_pool();
        let merged = pp.merged_raw("nonexistent");
        assert_eq!(merged.get("6"), Some(&"192.168.1.67".to_string()));
        assert_eq!(merged.get("15"), Some(&"levonk.com".to_string()));
    }

    #[test]
    fn lease_time_profile_overrides_pool() {
        let pp = per_pool();
        // kids profile → 1h, regardless of pool.
        assert_eq!(pp.resolve_lease_time("guest", Some("kids"), 0), 3600);
        assert_eq!(pp.resolve_lease_time("main", Some("kids"), 0), 3600);
    }

    #[test]
    fn lease_time_pool_when_no_profile() {
        let pp = per_pool();
        // guest pool → 4h
        assert_eq!(pp.resolve_lease_time("guest", None, 0), 14_400);
    }

    #[test]
    fn lease_time_global_when_pool_zero() {
        let pp = per_pool();
        // main pool has 86400 (24h) which is non-zero, so it wins over global.
        assert_eq!(pp.resolve_lease_time("main", None, 0), 86_400);
    }

    #[test]
    fn lease_time_fallback_when_all_zero() {
        let pp = PerPoolOptions::new();
        assert_eq!(pp.resolve_lease_time("anything", None, 7200), 7200);
    }

    #[test]
    fn lease_time_unknown_profile_falls_through() {
        let pp = per_pool();
        // unknown profile → pool lease time
        assert_eq!(pp.resolve_lease_time("guest", Some("unknown"), 0), 14_400);
    }

    #[test]
    fn parse_option_map_validates_entries() {
        let mut raw = HashMap::new();
        raw.insert("6".to_string(), "192.168.1.1".to_string());
        raw.insert("15".to_string(), "levonk.com".to_string());
        let set = parse_option_map(&raw, 3600).expect("valid map");
        assert_eq!(set.lease_time_secs, 3600);
        assert_eq!(set.options.len(), 2);
    }

    #[test]
    fn parse_option_map_rejects_bad_code() {
        let mut raw = HashMap::new();
        raw.insert("not-a-code".to_string(), "x".to_string());
        assert!(parse_option_map(&raw, 0).is_err());
    }

    #[test]
    fn parse_option_map_rejects_bad_hex() {
        let mut raw = HashMap::new();
        raw.insert("43".to_string(), "0xZZ".to_string());
        assert!(parse_option_map(&raw, 0).is_err());
    }
}
