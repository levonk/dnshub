//! Configuration validation rules for `dnshub.toml` and `blocklists.toml`.
//!
//! Validation is split out from the serde structs so the rules are easy to
//! audit and test independently. Each validator collects *all* failures into
//! a single `Vec<String>` rather than bailing on the first error — this gives
//! operators the full list of problems to fix in one pass.
//!
//! Rules implemented (see story 01-004 acceptance criteria):
//! - Required fields are present (e.g. at least one upstream).
//! - Listen addresses and upstream addresses parse as `SocketAddr`s (which
//!   also enforces the valid port range 0-65535).
//! - Server/upstream protocols are `"udp"` or `"tcp"`.
//! - Cache `min_ttl <= max_ttl`.
//! - Upstream tiers are unique (no duplicate tier indices).
//! - TLS/DoH, when enabled, have non-empty cert/key/listen.
//! - Logging level/format and tracing sample rate are within allowed sets.
//! - Blocklist source formats are `adblock`/`domains`/`hosts`, refresh
//!   intervals are positive, source names are unique, and the storage type
//!   is `lmdb` with a valid Bloom FPR when the Bloom filter is enabled.

use super::{BlocklistsConfig, DnshubConfig};
use std::collections::HashSet;
use std::net::SocketAddr;

/// Validate a [`DnshubConfig`]. Returns `Ok(())` if every check passes, or
/// `Err` with one message per failed check.
pub fn validate_config(config: &DnshubConfig) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    validate_server(&config.server, &mut errors);
    validate_cache(&config.cache, &mut errors);
    validate_upstreams(&config.upstreams, &mut errors);
    validate_rate_limit(&config.rate_limit, &mut errors);
    validate_query_log(&config.query_log, &mut errors);
    validate_metrics(&config.metrics, &mut errors);
    validate_logging(&config.logging, &mut errors);
    validate_tracing(&config.tracing, &mut errors);
    validate_frontend(&config.frontend, &mut errors);
    validate_dhcp(&config.dhcp, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Validate a [`BlocklistsConfig`].
pub fn validate_blocklists(config: &BlocklistsConfig) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let mut names: HashSet<&str> = HashSet::new();
    for src in &config.sources {
        if src.name.is_empty() {
            errors.push("blocklist source: name is required".to_string());
        } else if !names.insert(src.name.as_str()) {
            errors.push(format!(
                "blocklist source '{}': duplicate source name",
                src.name
            ));
        }

        if src.url.is_empty() {
            errors.push(format!(
                "blocklist source '{}': url is required",
                src.name
            ));
        }

        match src.format.as_str() {
            "adblock" | "domains" | "hosts" => {}
            other => errors.push(format!(
                "blocklist source '{}': format '{}' is not one of adblock/domains/hosts",
                src.name, other
            )),
        }

        match (src.refresh_hours, src.refresh_minutes) {
            (None, None) => errors.push(format!(
                "blocklist source '{}': refresh_hours or refresh_minutes is required",
                src.name
            )),
            (Some(h), _) if h == 0 => errors.push(format!(
                "blocklist source '{}': refresh_hours must be greater than 0",
                src.name
            )),
            (_, Some(m)) if m == 0 => errors.push(format!(
                "blocklist source '{}': refresh_minutes must be greater than 0",
                src.name
            )),
            _ => {}
        }
    }

    match config.storage.storage_type.as_str() {
        "lmdb" => {}
        "" => errors.push("blocklists [storage].type is required".to_string()),
        other => errors.push(format!(
            "blocklists [storage].type '{}' is not supported (expected 'lmdb')",
            other
        )),
    }

    if config.storage.path.is_empty() {
        errors.push("blocklists [storage].path is required".to_string());
    }

    if config.storage.bloom_filter {
        if config.storage.bloom_fpr <= 0.0 || config.storage.bloom_fpr >= 1.0 {
            errors.push(format!(
                "blocklists [storage].bloom_fpr {} must be in (0.0, 1.0)",
                config.storage.bloom_fpr
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_server(server: &super::ServerConfig, errors: &mut Vec<String>) {
    if server.listen.is_empty() {
        errors.push("[server].listen must contain at least one address".to_string());
    }
    for addr in &server.listen {
        if addr.parse::<SocketAddr>().is_err() {
            errors.push(format!(
                "[server].listen: '{}' is not a valid socket address (expected 'host:port')",
                addr
            ));
        }
    }

    if server.protocol.is_empty() {
        errors.push("[server].protocol must contain at least one of udp/tcp".to_string());
    }
    for proto in &server.protocol {
        if proto != "udp" && proto != "tcp" {
            errors.push(format!(
                "[server].protocol: '{}' is not 'udp' or 'tcp'",
                proto
            ));
        }
    }

    if let Some(tls) = &server.tls {
        if tls.enabled {
            if tls.listen.is_empty() {
                errors.push("[server.tls].listen must not be empty when tls is enabled".to_string());
            }
            for addr in &tls.listen {
                if addr.parse::<SocketAddr>().is_err() {
                    errors.push(format!(
                        "[server.tls].listen: '{}' is not a valid socket address",
                        addr
                    ));
                }
            }
            if tls.cert.is_empty() {
                errors.push("[server.tls].cert is required when tls is enabled".to_string());
            }
            if tls.key.is_empty() {
                errors.push("[server.tls].key is required when tls is enabled".to_string());
            }
        }
    }

    if let Some(doh) = &server.doh {
        if doh.enabled {
            if doh.listen.is_empty() {
                errors.push("[server.doh].listen must not be empty when doh is enabled".to_string());
            }
            for addr in &doh.listen {
                if addr.parse::<SocketAddr>().is_err() {
                    errors.push(format!(
                        "[server.doh].listen: '{}' is not a valid socket address",
                        addr
                    ));
                }
            }
            if doh.path.is_empty() {
                errors.push("[server.doh].path is required when doh is enabled".to_string());
            }
            if doh.cert.is_empty() {
                errors.push("[server.doh].cert is required when doh is enabled".to_string());
            }
            if doh.key.is_empty() {
                errors.push("[server.doh].key is required when doh is enabled".to_string());
            }
        }
    }
}

fn validate_cache(cache: &super::CacheConfig, errors: &mut Vec<String>) {
    if cache.min_ttl > cache.max_ttl {
        errors.push(format!(
            "[cache].min_ttl ({}) must be <= max_ttl ({})",
            cache.min_ttl, cache.max_ttl
        ));
    }
    if cache.max_entries == 0 {
        errors.push("[cache].max_entries must be greater than 0".to_string());
    }
}

fn validate_upstreams(upstreams: &[super::UpstreamConfig], errors: &mut Vec<String>) {
    if upstreams.is_empty() {
        errors.push("at least one [[upstreams]] entry is required".to_string());
        return;
    }

    let mut tiers: HashSet<u32> = HashSet::new();
    for up in upstreams {
        if up.name.is_empty() {
            errors.push("[[upstreams]].name is required".to_string());
        }
        if up.address.is_empty() {
            errors.push(format!("upstream '{}': address is required", up.name));
        } else if up.address.parse::<SocketAddr>().is_err() {
            errors.push(format!(
                "upstream '{}': address '{}' is not a valid socket address",
                up.name, up.address
            ));
        }
        if up.protocol != "udp" && up.protocol != "tcp" {
            errors.push(format!(
                "upstream '{}': protocol '{}' is not 'udp' or 'tcp'",
                up.name, up.protocol
            ));
        }
        if up.timeout_ms == 0 {
            errors.push(format!("upstream '{}': timeout_ms must be greater than 0", up.name));
        }
        if !tiers.insert(up.tier) {
            errors.push(format!("upstream '{}': duplicate tier {}", up.name, up.tier));
        }
    }
}

fn validate_rate_limit(rl: &super::RateLimitConfig, errors: &mut Vec<String>) {
    // No required fields (defaults to disabled). Only sanity-check when set.
    if rl.requests_per_second == 0 && rl.burst > 0 {
        errors.push(
            "[rate_limit].burst is set but requests_per_second is 0".to_string(),
        );
    }
}

fn validate_query_log(ql: &super::QueryLogConfig, errors: &mut Vec<String>) {
    if ql.enabled && ql.max_entries == 0 {
        errors.push("[query_log].max_entries must be greater than 0 when enabled".to_string());
    }
}

fn validate_metrics(m: &super::MetricsConfig, errors: &mut Vec<String>) {
    if m.listen.parse::<SocketAddr>().is_err() {
        errors.push(format!(
            "[metrics].listen: '{}' is not a valid socket address",
            m.listen
        ));
    }
    if m.path.is_empty() {
        errors.push("[metrics].path must not be empty".to_string());
    }
}

fn validate_logging(l: &super::LoggingConfig, errors: &mut Vec<String>) {
    match l.level.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {}
        other => errors.push(format!(
            "[logging].level '{}' is not one of trace/debug/info/warn/error",
            other
        )),
    }
    match l.format.as_str() {
        "json" | "plain" => {}
        other => errors.push(format!(
            "[logging].format '{}' is not one of json/plain",
            other
        )),
    }
}

fn validate_tracing(t: &super::TracingConfig, errors: &mut Vec<String>) {
    if t.enabled {
        if t.endpoint.is_empty() {
            errors.push("[tracing].endpoint is required when tracing is enabled".to_string());
        }
        if t.sample_rate < 0.0 || t.sample_rate > 1.0 {
            errors.push(format!(
                "[tracing].sample_rate {} must be in [0.0, 1.0]",
                t.sample_rate
            ));
        }
    }
}

fn validate_frontend(f: &super::FrontendConfig, errors: &mut Vec<String>) {
    if f.enabled {
        if f.listen.parse::<SocketAddr>().is_err() {
            errors.push(format!(
                "[frontend].listen: '{}' is not a valid socket address",
                f.listen
            ));
        }
        if f.static_dir.is_empty() {
            errors.push("[frontend].static_dir is required when frontend is enabled".to_string());
        }
    }
}

fn validate_dhcp(d: &super::DhcpConfig, errors: &mut Vec<String>) {
    if d.enabled {
        if d.interface.is_empty() {
            errors.push("[dhcp].interface is required when dhcp is enabled".to_string());
        }
        if d.listen.is_empty() {
            errors.push("[dhcp].listen is required when dhcp is enabled".to_string());
        } else if d.listen.parse::<SocketAddr>().is_err() {
            errors.push(format!(
                "[dhcp].listen: '{}' is not a valid socket address",
                d.listen
            ));
        }
        if d.pool_start.is_empty() {
            errors.push("[dhcp].pool_start is required when dhcp is enabled".to_string());
        }
        if d.pool_end.is_empty() {
            errors.push("[dhcp].pool_end is required when dhcp is enabled".to_string());
        }
        if d.lease_time_hours == 0 {
            errors.push("[dhcp].lease_time_hours must be greater than 0 when dhcp is enabled".to_string());
        }
    }

    if let Some(ra) = &d.ra {
        validate_ra(ra, errors);
    }
}

fn validate_ra(ra: &crate::dhcp::ra::RaConfig, errors: &mut Vec<String>) {
    if !ra.enabled {
        return;
    }

    if ra.prefix.is_empty() {
        errors.push("[dhcp.v6.ra].prefix is required when RA is enabled".to_string());
    } else if ra.parse_prefix().is_none() {
        errors.push(format!(
            "[dhcp.v6.ra].prefix '{}' is not a valid IPv6 CIDR (expected 'addr/len')",
            ra.prefix
        ));
    }

    if ra.valid_lifetime_secs == 0 {
        errors.push("[dhcp.v6.ra].valid_lifetime_secs must be greater than 0 when RA is enabled".to_string());
    }

    if ra.preferred_lifetime_secs > ra.valid_lifetime_secs {
        errors.push(format!(
            "[dhcp.v6.ra].preferred_lifetime_secs ({}) must be <= valid_lifetime_secs ({})",
            ra.preferred_lifetime_secs, ra.valid_lifetime_secs
        ));
    }

    for s in &ra.rdnss {
        if s.parse::<std::net::Ipv6Addr>().is_err() {
            errors.push(format!(
                "[dhcp.v6.ra].rdnss: '{}' is not a valid IPv6 address",
                s
            ));
        }
    }

    for d in &ra.dnssl {
        if d.is_empty() {
            errors.push("[dhcp.v6.ra].dnssl: domain must not be empty".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    fn valid_minimal_config() -> DnshubConfig {
        let mut cfg = DnshubConfig::default();
        // Populate the sections whose `Default` impls yield empty values that
        // would otherwise trip validation (serde defaults only apply on
        // deserialization, not via `Default`).
        cfg.metrics.listen = "0.0.0.0:9090".to_string();
        cfg.metrics.path = "/metrics".to_string();
        cfg.logging.level = "info".to_string();
        cfg.logging.format = "json".to_string();
        cfg.upstreams.push(UpstreamConfig {
            name: "test".to_string(),
            address: "1.1.1.1:53".to_string(),
            protocol: "udp".to_string(),
            timeout_ms: 1000,
            tier: 1,
        });
        cfg
    }

    #[test]
    fn valid_config_passes() {
        let cfg = valid_minimal_config();
        validate_config(&cfg).expect("minimal valid config should pass");
    }

    #[test]
    fn duplicate_upstream_tier_fails() {
        let mut cfg = valid_minimal_config();
        cfg.upstreams.push(UpstreamConfig {
            name: "dup".to_string(),
            address: "8.8.8.8:53".to_string(),
            protocol: "udp".to_string(),
            timeout_ms: 1000,
            tier: 1, // duplicate
        });
        let errs = validate_config(&cfg).expect_err("duplicate tier should fail");
        assert!(
            errs.iter().any(|e| e.contains("duplicate tier 1")),
            "expected duplicate tier error, got {errs:?}"
        );
    }

    #[test]
    fn invalid_upstream_address_fails() {
        let mut cfg = valid_minimal_config();
        cfg.upstreams[0].address = "not-an-address".to_string();
        let errs = validate_config(&cfg).expect_err("bad address should fail");
        assert!(
            errs.iter().any(|e| e.contains("not a valid socket address")),
            "expected address error, got {errs:?}"
        );
    }

    #[test]
    fn no_upstreams_fails() {
        let cfg = DnshubConfig::default();
        let errs = validate_config(&cfg).expect_err("no upstreams should fail");
        assert!(
            errs.iter().any(|e| e.contains("at least one [[upstreams]]")),
            "expected missing-upstream error, got {errs:?}"
        );
    }

    #[test]
    fn cache_min_gt_max_fails() {
        let mut cfg = valid_minimal_config();
        cfg.cache.min_ttl = 1000;
        cfg.cache.max_ttl = 100;
        let errs = validate_config(&cfg).expect_err("min_ttl > max_ttl should fail");
        assert!(
            errs.iter().any(|e| e.contains("min_ttl") && e.contains("max_ttl")),
            "expected ttl error, got {errs:?}"
        );
    }

    #[test]
    fn invalid_log_level_fails() {
        let mut cfg = valid_minimal_config();
        cfg.logging.level = "verbose".to_string();
        let errs = validate_config(&cfg).expect_err("bad log level should fail");
        assert!(
            errs.iter().any(|e| e.contains("[logging].level")),
            "expected log level error, got {errs:?}"
        );
    }

    #[test]
    fn tracing_sample_rate_out_of_range_fails() {
        let mut cfg = valid_minimal_config();
        cfg.tracing.enabled = true;
        cfg.tracing.endpoint = "http://jaeger:4317".to_string();
        cfg.tracing.sample_rate = 2.0;
        let errs = validate_config(&cfg).expect_err("bad sample rate should fail");
        assert!(
            errs.iter().any(|e| e.contains("sample_rate")),
            "expected sample_rate error, got {errs:?}"
        );
    }

    #[test]
    fn valid_blocklists_passes() {
        let cfg = BlocklistsConfig {
            sources: vec![SourceConfig {
                name: "test".to_string(),
                url: "https://example.com/list.txt".to_string(),
                format: "domains".to_string(),
                categories: vec!["ads".to_string()],
                refresh_hours: Some(24),
                refresh_minutes: None,
            }],
            storage: StorageConfig::default(),
        };
        validate_blocklists(&cfg).expect("valid blocklists should pass");
    }

    #[test]
    fn blocklist_missing_refresh_fails() {
        let cfg = BlocklistsConfig {
            sources: vec![SourceConfig {
                name: "test".to_string(),
                url: "https://example.com/list.txt".to_string(),
                format: "domains".to_string(),
                categories: vec![],
                refresh_hours: None,
                refresh_minutes: None,
            }],
            storage: StorageConfig::default(),
        };
        let errs = validate_blocklists(&cfg).expect_err("missing refresh should fail");
        assert!(
            errs.iter().any(|e| e.contains("refresh_hours or refresh_minutes")),
            "expected refresh error, got {errs:?}"
        );
    }

    #[test]
    fn blocklist_bad_format_fails() {
        let cfg = BlocklistsConfig {
            sources: vec![SourceConfig {
                name: "test".to_string(),
                url: "https://example.com/list.txt".to_string(),
                format: "csv".to_string(),
                categories: vec![],
                refresh_hours: Some(24),
                refresh_minutes: None,
            }],
            storage: StorageConfig::default(),
        };
        let errs = validate_blocklists(&cfg).expect_err("bad format should fail");
        assert!(
            errs.iter().any(|e| e.contains("format 'csv'")),
            "expected format error, got {errs:?}"
        );
    }

    #[test]
    fn blocklist_duplicate_name_fails() {
        let cfg = BlocklistsConfig {
            sources: vec![
                SourceConfig {
                    name: "dup".to_string(),
                    url: "https://a.example/list.txt".to_string(),
                    format: "domains".to_string(),
                    categories: vec![],
                    refresh_hours: Some(24),
                    refresh_minutes: None,
                },
                SourceConfig {
                    name: "dup".to_string(),
                    url: "https://b.example/list.txt".to_string(),
                    format: "domains".to_string(),
                    categories: vec![],
                    refresh_hours: Some(24),
                    refresh_minutes: None,
                },
            ],
            storage: StorageConfig::default(),
        };
        let errs = validate_blocklists(&cfg).expect_err("duplicate name should fail");
        assert!(
            errs.iter().any(|e| e.contains("duplicate source name")),
            "expected duplicate name error, got {errs:?}"
        );
    }

    #[test]
    fn blocklist_bad_bloom_fpr_fails() {
        let mut cfg = BlocklistsConfig {
            sources: vec![SourceConfig {
                name: "test".to_string(),
                url: "https://example.com/list.txt".to_string(),
                format: "domains".to_string(),
                categories: vec![],
                refresh_hours: Some(24),
                refresh_minutes: None,
            }],
            storage: StorageConfig::default(),
        };
        cfg.storage.bloom_fpr = 1.5;
        let errs = validate_blocklists(&cfg).expect_err("bad bloom fpr should fail");
        assert!(
            errs.iter().any(|e| e.contains("bloom_fpr")),
            "expected bloom_fpr error, got {errs:?}"
        );
    }

    #[test]
    fn valid_ra_config_passes() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: true,
            prefix: "fd00:1234:5678::/64".to_string(),
            preferred_lifetime_secs: 3600,
            valid_lifetime_secs: 7200,
            router_lifetime_secs: 1800,
            rdnss: vec!["fd00:1234:5678::67".to_string()],
            dnssl: vec!["levonk.com".to_string()],
        });
        validate_config(&cfg).expect("valid RA config should pass");
    }

    #[test]
    fn ra_disabled_skips_validation() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: false,
            prefix: String::new(),
            ..crate::dhcp::ra::RaConfig::default()
        });
        validate_config(&cfg).expect("disabled RA should skip validation");
    }

    #[test]
    fn ra_invalid_prefix_fails() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: true,
            prefix: "not-a-cidr".to_string(),
            ..crate::dhcp::ra::RaConfig::default()
        });
        let errs = validate_config(&cfg).expect_err("bad RA prefix should fail");
        assert!(
            errs.iter().any(|e| e.contains("[dhcp.v6.ra].prefix")),
            "expected RA prefix error, got {errs:?}"
        );
    }

    #[test]
    fn ra_preferred_gt_valid_fails() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: true,
            prefix: "fd00::/64".to_string(),
            preferred_lifetime_secs: 9999,
            valid_lifetime_secs: 1000,
            ..crate::dhcp::ra::RaConfig::default()
        });
        let errs = validate_config(&cfg).expect_err("preferred > valid should fail");
        assert!(
            errs.iter()
                .any(|e| e.contains("preferred_lifetime_secs") && e.contains("valid_lifetime_secs")),
            "expected lifetime error, got {errs:?}"
        );
    }

    #[test]
    fn ra_invalid_rdnss_fails() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: true,
            prefix: "fd00::/64".to_string(),
            rdnss: vec!["not-an-addr".to_string()],
            ..crate::dhcp::ra::RaConfig::default()
        });
        let errs = validate_config(&cfg).expect_err("bad RDNSS should fail");
        assert!(
            errs.iter().any(|e| e.contains("[dhcp.v6.ra].rdnss")),
            "expected RDNSS error, got {errs:?}"
        );
    }

    #[test]
    fn ra_empty_dnssl_fails() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: true,
            prefix: "fd00::/64".to_string(),
            dnssl: vec!["".to_string()],
            ..crate::dhcp::ra::RaConfig::default()
        });
        let errs = validate_config(&cfg).expect_err("empty DNSSL should fail");
        assert!(
            errs.iter().any(|e| e.contains("[dhcp.v6.ra].dnssl")),
            "expected DNSSL error, got {errs:?}"
        );
    }

    #[test]
    fn ra_zero_valid_lifetime_fails() {
        let mut cfg = valid_minimal_config();
        cfg.dhcp.ra = Some(crate::dhcp::ra::RaConfig {
            enabled: true,
            prefix: "fd00::/64".to_string(),
            valid_lifetime_secs: 0,
            ..crate::dhcp::ra::RaConfig::default()
        });
        let errs = validate_config(&cfg).expect_err("zero valid lifetime should fail");
        assert!(
            errs.iter()
                .any(|e| e.contains("valid_lifetime_secs must be greater than 0")),
            "expected valid_lifetime error, got {errs:?}"
        );
    }
}
