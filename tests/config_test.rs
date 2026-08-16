//! Integration tests for `dnshub.toml` and `blocklists.toml` loading.
//!
//! These tests read the fixtures under `tests/fixtures/` and exercise the
//! `load_config` / `load_blocklists` entry points plus serde round-tripping.

use dnshub::config::{
    load_blocklists, load_config, BlocklistsConfig, ConfigError, DnshubConfig,
};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    p
}

#[test]
fn loads_valid_dnshub_toml() {
    let cfg = load_config(&fixture("valid-dnshub.toml")).expect("valid config should load");
    assert_eq!(cfg.server.listen, vec!["0.0.0.0:53", "[::]:53"]);
    assert_eq!(cfg.server.protocol, vec!["udp", "tcp"]);
    assert!(cfg.server.tls.as_ref().unwrap().enabled);
    assert!(cfg.server.doh.as_ref().unwrap().enabled);
    assert_eq!(cfg.server.doh.as_ref().unwrap().path, "/dns-query");
    assert_eq!(cfg.cache.min_ttl, 60);
    assert_eq!(cfg.cache.max_ttl, 86400);
    assert!(cfg.cache.serve_stale);
    assert_eq!(cfg.rate_limit.requests_per_second, 100);
    assert!(cfg.ecs.strip);
    assert!(cfg.query_log.enabled);
    assert_eq!(cfg.metrics.listen, "0.0.0.0:9090");
    assert_eq!(cfg.logging.level, "info");
    assert_eq!(cfg.logging.format, "json");
    assert!(!cfg.tracing.enabled);
    assert!(cfg.frontend.enabled);
    assert_eq!(cfg.upstreams.len(), 5);
    assert_eq!(cfg.upstreams[0].name, "unbound-validator");
    assert_eq!(cfg.upstreams[0].tier, 1);
    assert_eq!(cfg.upstreams[4].tier, 5);
    assert!(cfg.dhcp.enabled);
    assert_eq!(cfg.dhcp.interface, "eth0");
    assert_eq!(cfg.dhcp.lease_time_hours, 24);
}

#[test]
fn valid_dnshub_toml_passes_validation() {
    // load_config already validates; this asserts it did not error.
    load_config(&fixture("valid-dnshub.toml")).expect("valid config should validate");
}

#[test]
fn invalid_dnshub_toml_returns_validation_errors() {
    let err = load_config(&fixture("invalid-dnshub.toml"))
        .expect_err("invalid config should fail validation");
    match err {
        ConfigError::Validation(errs) => {
            // Expect several distinct failures.
            assert!(!errs.is_empty(), "should report at least one error");
            let joined = errs.join("\n");
            assert!(
                joined.contains("at least one [[upstreams]]"),
                "expected missing-upstream error, got: {joined}"
            );
            assert!(
                joined.contains("not a valid socket address"),
                "expected bad-address error, got: {joined}"
            );
            assert!(
                joined.contains("'quic'"),
                "expected bad-protocol error, got: {joined}"
            );
            assert!(
                joined.contains("min_ttl"),
                "expected ttl error, got: {joined}"
            );
            assert!(
                joined.contains("[logging].level"),
                "expected log-level error, got: {joined}"
            );
            assert!(
                joined.contains("sample_rate"),
                "expected sample_rate error, got: {joined}"
            );
            assert!(
                joined.contains("[frontend].static_dir"),
                "expected frontend static_dir error, got: {joined}"
            );
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn missing_file_returns_io_error() {
    let err = load_config(&fixture("does-not-exist.toml"))
        .expect_err("missing file should error");
    assert!(matches!(err, ConfigError::Io(_)), "expected Io error, got {err:?}");
}

#[test]
fn loads_valid_blocklists_toml() {
    let cfg =
        load_blocklists(&fixture("valid-blocklists.toml")).expect("valid blocklists should load");
    assert_eq!(cfg.sources.len(), 10);
    assert_eq!(cfg.sources[0].name, "easylist");
    assert_eq!(cfg.sources[0].format, "adblock");
    assert_eq!(cfg.sources[0].refresh_hours, Some(96));
    // urlhaus uses refresh_minutes instead of refresh_hours.
    let urlhaus = cfg.sources.iter().find(|s| s.name == "urlhaus").unwrap();
    assert_eq!(urlhaus.refresh_minutes, Some(15));
    assert_eq!(urlhaus.refresh_interval_minutes(), Some(15));
    assert_eq!(cfg.sources[0].refresh_interval_minutes(), Some(96 * 60));
    assert_eq!(cfg.storage.storage_type, "lmdb");
    assert_eq!(cfg.storage.path, "/var/lib/dnshub/blocklists.lmdb");
    assert!(cfg.storage.bloom_filter);
    assert!((cfg.storage.bloom_fpr - 0.001).abs() < f64::EPSILON);
}

#[test]
fn invalid_blocklists_toml_returns_validation_errors() {
    let err = load_blocklists(&fixture("invalid-blocklists.toml"))
        .expect_err("invalid blocklists should fail");
    match err {
        ConfigError::Validation(errs) => {
            let joined = errs.join("\n");
            assert!(joined.contains("duplicate source name"), "got: {joined}");
            assert!(joined.contains("url is required"), "got: {joined}");
            assert!(joined.contains("format 'csv'"), "got: {joined}");
            assert!(
                joined.contains("refresh_hours or refresh_minutes"),
                "got: {joined}"
            );
            assert!(joined.contains("refresh_hours must be greater than 0"), "got: {joined}");
            assert!(joined.contains("not supported"), "got: {joined}");
            assert!(joined.contains("bloom_fpr"), "got: {joined}");
        }
        other => panic!("expected Validation error, got {other:?}"),
    }
}

#[test]
fn config_round_trips_through_serde() {
    // A config loaded from TOML should serialize back to TOML and re-parse
    // to an equal config (defaults and all).
    let cfg = load_config(&fixture("valid-dnshub.toml")).expect("load");
    let toml_str = toml::to_string(&cfg).expect("serialize");
    let reparsed: DnshubConfig = toml::from_str(&toml_str).expect("reparse");
    assert_eq!(cfg.server.listen, reparsed.server.listen);
    assert_eq!(cfg.upstreams.len(), reparsed.upstreams.len());
    assert_eq!(cfg.cache.max_ttl, reparsed.cache.max_ttl);
    assert_eq!(cfg.dhcp.interface, reparsed.dhcp.interface);
}

#[test]
fn blocklists_round_trip_through_serde() {
    let cfg = load_blocklists(&fixture("valid-blocklists.toml")).expect("load");
    let toml_str = toml::to_string(&cfg).expect("serialize");
    let reparsed: BlocklistsConfig = toml::from_str(&toml_str).expect("reparse");
    assert_eq!(cfg.sources.len(), reparsed.sources.len());
    assert_eq!(cfg.storage.storage_type, reparsed.storage.storage_type);
}

#[test]
fn defaults_validate() {
    // The scaffold default config (with a tier-1 upstream) must validate.
    DnshubConfig::defaults()
        .validate()
        .expect("defaults should validate");
}
