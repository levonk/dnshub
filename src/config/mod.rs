//! Serde structs for `dnshub.toml`.
//!
//! This story (01-001) defines the configuration shape used by the DNS server,
//! cache, and upstream forwarding sections. Full TOML loading with validation
//! arrives in story 01-004; here we provide the data model and a helper to
//! build a default config for the server scaffold.
//!
//! The shape mirrors the `dnshub.toml` example in PRD section 4
//! (lines 1374-1478): `[server]`, `[server.tls]`, `[server.doh]`, `[cache]`,
//! `[[upstreams]]`, plus the reserved sections for later stories
//! (`[dhcp]`, `[rate_limit]`, `[ecs]`, `[query_log]`, `[metrics]`,
//! `[logging]`, `[tracing]`, `[frontend]`).

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Top-level dnshub configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DnshubConfig {
    /// DNS server listener configuration (UDP/TCP on :53).
    #[serde(default)]
    pub server: ServerConfig,

    /// Response cache configuration (LRU with TTL clamping).
    #[serde(default)]
    pub cache: CacheConfig,

    /// Upstream resolver tiers (ordered fallback). Tier 1 is the primary.
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,

    /// DHCP server configuration (story 04-001+).
    #[serde(default)]
    pub dhcp: DhcpConfig,

    /// Rate limiting (story 03-004).
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    /// ECS stripping (story 03-003).
    #[serde(default)]
    pub ecs: EcsConfig,

    /// Query log (story 05-003).
    #[serde(default)]
    pub query_log: QueryLogConfig,

    /// Prometheus metrics endpoint (story 01-003).
    #[serde(default)]
    pub metrics: MetricsConfig,

    /// Structured logging.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Jaeger tracing (story 05-002).
    #[serde(default)]
    pub tracing: TracingConfig,

    /// NextJS frontend (story 05-006).
    #[serde(default)]
    pub frontend: FrontendConfig,
}

/// `[server]` — DNS listener configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Socket addresses to listen on, e.g. `["0.0.0.0:53", "[::]:53"]`.
    #[serde(default = "default_server_listen")]
    pub listen: Vec<String>,

    /// Enabled transport protocols: `"udp"`, `"tcp"`.
    #[serde(default = "default_server_protocol")]
    pub protocol: Vec<String>,

    /// DoT (DNS-over-TLS) listener on :853 (story 04-009).
    #[serde(default)]
    pub tls: Option<TlsServerConfig>,

    /// DoH (DNS-over-HTTPS) listener on :443 (story 04-010).
    #[serde(default)]
    pub doh: Option<DohServerConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_server_listen(),
            protocol: default_server_protocol(),
            tls: None,
            doh: None,
        }
    }
}

fn default_server_listen() -> Vec<String> {
    vec!["0.0.0.0:53".to_string(), "[::]:53".to_string()]
}

fn default_server_protocol() -> Vec<String> {
    vec!["udp".to_string(), "tcp".to_string()]
}

/// `[server.tls]` — DoT listener (story 04-009).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TlsServerConfig {
    pub enabled: bool,
    pub listen: Vec<String>,
    pub cert: String,
    pub key: String,
}

/// `[server.doh]` — DoH listener (story 04-010).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DohServerConfig {
    pub enabled: bool,
    pub listen: Vec<String>,
    pub path: String,
    pub cert: String,
    pub key: String,
}

/// `[cache]` — response cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Minimum TTL (seconds) applied to cached responses.
    #[serde(default = "default_min_ttl")]
    pub min_ttl: u64,

    /// Maximum TTL (seconds) applied to cached responses.
    #[serde(default = "default_max_ttl")]
    pub max_ttl: u64,

    /// TTL (seconds) for negative responses (NXDOMAIN, NODATA).
    #[serde(default = "default_negative_ttl")]
    pub negative_ttl: u64,

    /// Serve stale responses when upstreams fail (RFC 8767, story 03-002).
    #[serde(default)]
    pub serve_stale: bool,

    /// How long stale entries remain usable (seconds).
    #[serde(default = "default_serve_stale_ttl")]
    pub serve_stale_ttl: u64,

    /// Maximum number of cached entries.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            min_ttl: default_min_ttl(),
            max_ttl: default_max_ttl(),
            negative_ttl: default_negative_ttl(),
            serve_stale: false,
            serve_stale_ttl: default_serve_stale_ttl(),
            max_entries: default_max_entries(),
        }
    }
}

fn default_min_ttl() -> u64 {
    60
}
fn default_max_ttl() -> u64 {
    86_400
}
fn default_negative_ttl() -> u64 {
    300
}
fn default_serve_stale_ttl() -> u64 {
    86_400
}
fn default_max_entries() -> usize {
    100_000
}

/// `[[upstreams]]` — a single upstream resolver tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// Human-readable name, e.g. `"unbound-validator"`.
    pub name: String,

    /// Upstream address, e.g. `"172.20.255.50:15353"`.
    pub address: String,

    /// Transport protocol: `"udp"` or `"tcp"`.
    #[serde(default = "default_upstream_protocol")]
    pub protocol: String,

    /// Per-tier query timeout in milliseconds.
    #[serde(default = "default_upstream_timeout_ms")]
    pub timeout_ms: u64,

    /// Fallback tier index (1 = primary, 5 = last resort).
    #[serde(default = "default_upstream_tier")]
    pub tier: u32,
}

fn default_upstream_protocol() -> String {
    "udp".to_string()
}
fn default_upstream_timeout_ms() -> u64 {
    2000
}
fn default_upstream_tier() -> u32 {
    1
}

impl UpstreamConfig {
    /// Parse the configured `address` into a `SocketAddr`.
    pub fn socket_addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        self.address.parse()
    }

    /// Returns the first tier-1 upstream from a list, if any.
    pub fn first_tier_one(upstreams: &[UpstreamConfig]) -> Option<&UpstreamConfig> {
        upstreams.iter().find(|u| u.tier == 1)
    }
}

// ---------------------------------------------------------------------------
// Reserved sections for later stories (defined so the config parses cleanly).
// ---------------------------------------------------------------------------

/// `[dhcp]` — DHCP server (story 04-001+). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DhcpConfig {
    #[serde(default)]
    pub enabled: bool,
}

/// `[rate_limit]` — token bucket rate limiting (story 03-004). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitConfig {
    #[serde(default)]
    pub requests_per_second: u32,
    #[serde(default)]
    pub burst: u32,
    #[serde(default)]
    pub per_client: bool,
}

/// `[ecs]` — EDNS Client Subnet stripping (story 03-003). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EcsConfig {
    #[serde(default)]
    pub strip: bool,
}

/// `[query_log]` — SQLite ring buffer (story 05-003). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_query_log_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_query_log_retention_days")]
    pub retention_days: u32,
}

fn default_query_log_max_entries() -> usize {
    100_000
}
fn default_query_log_retention_days() -> u32 {
    7
}

/// `[metrics]` — Prometheus endpoint (story 01-003). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsConfig {
    #[serde(default = "default_metrics_listen")]
    pub listen: String,
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

fn default_metrics_listen() -> String {
    "0.0.0.0:9090".to_string()
}
fn default_metrics_path() -> String {
    "/metrics".to_string()
}

/// `[logging]` — structured logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_format() -> String {
    "json".to_string()
}

/// `[tracing]` — Jaeger tracing (story 05-002). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TracingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_trace_sample_rate")]
    pub sample_rate: f64,
}

fn default_trace_sample_rate() -> f64 {
    0.05
}

/// `[frontend]` — NextJS static export (story 05-006). Reserved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrontendConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_frontend_listen")]
    pub listen: String,
    #[serde(default)]
    pub static_dir: String,
}

fn default_frontend_listen() -> String {
    "0.0.0.0:8080".to_string()
}

impl DnshubConfig {
    /// Build a default config suitable for running the server scaffold.
    ///
    /// The default forwards to `1.1.1.1:53` (Cloudflare) as the tier-1
    /// upstream so the server can resolve out of the box without external
    /// configuration. Production deployments override this via `dnshub.toml`.
    pub fn defaults() -> Self {
        Self {
            upstreams: vec![UpstreamConfig {
                name: "cloudflare".to_string(),
                address: "1.1.1.1:53".to_string(),
                protocol: "udp".to_string(),
                timeout_ms: 2000,
                tier: 1,
            }],
            ..Self::default()
        }
    }
}
