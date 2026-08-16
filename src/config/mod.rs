//! Serde structs and loaders for `dnshub.toml` and `blocklists.toml`.
//!
//! Story 01-001 defined the configuration shape used by the DNS server,
//! cache, and upstream forwarding sections. Story 01-004 (this story) adds
//! full TOML loading with validation, the remaining config sections from
//! PRD section 4.10, and `blocklists.toml` support.
//!
//! The shape mirrors the `dnshub.toml` example in PRD section 4
//! (lines 1374-1478): `[server]`, `[server.tls]`, `[server.doh]`, `[dhcp]`,
//! `[cache]`, `[rate_limit]`, `[ecs]`, `[query_log]`, `[metrics]`,
//! `[logging]`, `[tracing]`, `[frontend]`, `[[upstreams]]`. The
//! `blocklists.toml` shape (PRD lines 1480-1558) is `[[sources]]` plus
//! `[storage]`.

mod hot_reload;
mod validation;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use hot_reload::HotReloadManager;
pub use validation::{validate_blocklists, validate_config};

/// Errors that can occur while loading or validating configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to read the config file from disk.
    Io(std::io::Error),
    /// The TOML could not be deserialized into the config struct.
    Parse(toml::de::Error),
    /// One or more validation rules failed. Each entry is a human-readable
    /// description of a single failed check.
    Validation(Vec<String>),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config I/O error: {e}"),
            ConfigError::Parse(e) => write!(f, "config parse error: {e}"),
            ConfigError::Validation(errs) => {
                write!(f, "config validation failed:")?;
                for e in errs {
                    write!(f, "\n  - {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(e) => Some(e),
            ConfigError::Parse(e) => Some(e),
            ConfigError::Validation(_) => None,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Parse(e)
    }
}

/// Load and validate `dnshub.toml` from `path`.
///
/// Reads the file, deserializes it into [`DnshubConfig`], then runs
/// [`DnshubConfig::validate`]. Returns the first error encountered (I/O,
/// parse, or validation).
pub fn load_config(path: &Path) -> Result<DnshubConfig, ConfigError> {
    let contents = std::fs::read_to_string(path)?;
    let config: DnshubConfig = toml::from_str(&contents)?;
    config.validate()?;
    Ok(config)
}

/// Load and validate `blocklists.toml` from `path`.
///
/// Reads the file, deserializes it into [`BlocklistsConfig`], then runs
/// [`BlocklistsConfig::validate`].
pub fn load_blocklists(path: &Path) -> Result<BlocklistsConfig, ConfigError> {
    let contents = std::fs::read_to_string(path)?;
    let config: BlocklistsConfig = toml::from_str(&contents)?;
    config.validate()?;
    Ok(config)
}

/// Reload `dnshub.toml` from `path`, returning a freshly loaded and
/// validated [`DnshubConfig`].
///
/// This is the same operation as [`load_config`] but is provided as a
/// distinct name so call sites (e.g. [`HotReloadManager`]) read clearly
/// as "reload from disk" rather than "initial load".
pub fn reload_config(path: &Path) -> Result<DnshubConfig, ConfigError> {
    load_config(path)
}

/// Atomic, lock-free config holder backed by [`ArcSwap`].
///
/// [`ConfigStore`] wraps the active [`DnshubConfig`] in an `ArcSwap` so
/// that a hot-reload (triggered by SIGHUP, see [`HotReloadManager`]) can
/// atomically publish a new config without blocking concurrent readers.
///
/// Readers call [`ConfigStore::load_full`] to obtain a stable
/// `Arc<DnshubConfig>` that remains valid for its lifetime even if a
/// swap happens concurrently — the same guarantee the blocklist
/// [`HotSwapStore`](crate::blocklist::HotSwapStore) provides.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    inner: Arc<ArcSwap<DnshubConfig>>,
}

impl ConfigStore {
    /// Create a new `ConfigStore` holding `config` as the initial value.
    pub fn new(config: DnshubConfig) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(config)),
        }
    }

    /// Load a stable `Arc<DnshubConfig>` snapshot of the active config.
    ///
    /// The returned `Arc` is stable: even if [`ConfigStore::swap`] is
    /// called concurrently, this `Arc` continues to point at the same
    /// config object.
    pub fn load_full(&self) -> Arc<DnshubConfig> {
        self.inner.load_full()
    }

    /// Atomically publish a new config.
    ///
    /// After this call, new calls to [`ConfigStore::load_full`] return the
    /// new config. Existing `Arc<DnshubConfig>` holders continue using the
    /// old config until their `Arc` is dropped.
    pub fn swap(&self, new_config: DnshubConfig) {
        self.inner.store(Arc::new(new_config));
        tracing::info!("config hot-swap completed");
    }

    /// Reload the config from `path` and, if it loads and validates
    /// successfully, atomically swap it in.
    ///
    /// On error the active config is left untouched.
    pub fn reload_from(&self, path: &Path) -> Result<(), ConfigError> {
        let new_config = reload_config(path)?;
        self.swap(new_config);
        Ok(())
    }
}

/// Paths used by the hot-reload manager to locate config files on disk.
///
/// `dnshub.toml` is the main config; `blocklists.toml` is optional and
/// only reloaded when present (the blocklist daemon is separately
/// triggered to refresh its sources).
#[derive(Debug, Clone)]
pub struct HotReloadPaths {
    /// Path to `dnshub.toml`.
    pub config: PathBuf,
    /// Optional path to `blocklists.toml`.
    pub blocklists: Option<PathBuf>,
}

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

/// `[dhcp]` — DHCP server (story 04-001+).
///
/// This is a config skeleton: the fields mirror the PRD `dnshub.toml`
/// example (lines 1392-1402) so the main config parses cleanly, but the
/// DHCP server logic itself is implemented in Phase 04 stories.
///
/// Story 04-008 adds the `[dhcp.audit]` and `[dhcp.rogue_detection]`
/// sub-sections (PRD lines 740-754, 772-777).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DhcpConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Network interface to bind the DHCP listener to, e.g. `"eth0"`.
    #[serde(default)]
    pub interface: String,
    /// DHCP listen address, e.g. `"0.0.0.0:67"`.
    #[serde(default)]
    pub listen: String,
    /// First address in the dynamic pool, e.g. `"192.168.1.100"`.
    #[serde(default)]
    pub pool_start: String,
    /// Last address in the dynamic pool, e.g. `"192.168.1.200"`.
    #[serde(default)]
    pub pool_end: String,
    /// Subnet mask, e.g. `"255.255.255.0"`.
    #[serde(default)]
    pub subnet: String,
    /// Default gateway/router option, e.g. `"192.168.1.1"`.
    #[serde(default)]
    pub router: String,
    /// Domain name option, e.g. `"levonk.com"`.
    #[serde(default)]
    pub domain: String,
    /// Lease duration in hours.
    #[serde(default)]
    pub lease_time_hours: u32,
    /// NTP server option, e.g. `"172.20.255.55"`.
    #[serde(default)]
    pub ntp_server: String,
    /// `[dhcp.audit]` — lease audit log (story 04-008, PRD lines 740-754).
    #[serde(default)]
    pub audit: crate::dhcp::audit::AuditConfig,
    /// `[dhcp.rogue_detection]` — rogue DHCP server detection (story 04-008,
    /// PRD lines 772-777).
    #[serde(default)]
    pub rogue_detection: crate::dhcp::rogue::RogueConfig,
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
            // The sections below use `#[serde(default = "...")]` helpers that
            // only apply during deserialization. Mirror those defaults here so
            // that `defaults()` produces a config that passes `validate()`.
            metrics: MetricsConfig {
                listen: default_metrics_listen(),
                path: default_metrics_path(),
            },
            logging: LoggingConfig {
                level: default_log_level(),
                format: default_log_format(),
            },
            ..Self::default()
        }
    }

    /// Validate this config in-place, returning `Err` with a list of every
    /// failed check. See [`validation::validate_config`] for the rules.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validation::validate_config(self).map_err(ConfigError::Validation)
    }
}

impl BlocklistsConfig {
    /// Validate this blocklists config, returning `Err` with a list of every
    /// failed check. See [`validation::validate_blocklists`] for the rules.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validation::validate_blocklists(self).map_err(ConfigError::Validation)
    }
}

// ---------------------------------------------------------------------------
// blocklists.toml — blocklist sources and storage (PRD lines 1480-1558).
// ---------------------------------------------------------------------------

/// Top-level `blocklists.toml` configuration.
///
/// Contains the `[[sources]]` array and the `[storage]` table. This is a
/// separate file from `dnshub.toml` (see PRD section 4.10) and is loaded via
/// [`load_blocklists`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlocklistsConfig {
    /// Blocklist sources to download and refresh.
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    /// On-disk storage backend for the compiled blocklist index.
    #[serde(default)]
    pub storage: StorageConfig,
}

/// A single `[[sources]]` entry in `blocklists.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceConfig {
    /// Human-readable name, e.g. `"easylist"`.
    pub name: String,
    /// Download URL for the list.
    pub url: String,
    /// List format: `"adblock"`, `"domains"`, or `"hosts"`.
    pub format: String,
    /// Categories tagged on every entry from this source.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Refresh interval in hours. Mutually exclusive with `refresh_minutes`.
    #[serde(default)]
    pub refresh_hours: Option<u64>,
    /// Refresh interval in minutes. Mutually exclusive with `refresh_hours`.
    #[serde(default)]
    pub refresh_minutes: Option<u64>,
}

impl SourceConfig {
    /// Effective refresh interval in minutes, preferring `refresh_minutes`
    /// when set and falling back to `refresh_hours * 60`.
    pub fn refresh_interval_minutes(&self) -> Option<u64> {
        if let Some(m) = self.refresh_minutes {
            return Some(m);
        }
        self.refresh_hours.map(|h| h * 60)
    }
}

/// The `[storage]` table in `blocklists.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage backend type. Currently only `"lmdb"` is supported.
    #[serde(rename = "type", default = "default_storage_type")]
    pub storage_type: String,

    /// Filesystem path to the storage database.
    #[serde(default = "default_storage_path")]
    pub path: String,

    /// Whether to maintain a Bloom filter for the fast negative path.
    #[serde(default = "default_bloom_filter")]
    pub bloom_filter: bool,

    /// Bloom filter false-positive rate (e.g. `0.001` = 0.1%).
    #[serde(default = "default_bloom_fpr")]
    pub bloom_fpr: f64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_type: default_storage_type(),
            path: default_storage_path(),
            bloom_filter: default_bloom_filter(),
            bloom_fpr: default_bloom_fpr(),
        }
    }
}

fn default_storage_type() -> String {
    "lmdb".to_string()
}
fn default_storage_path() -> String {
    "/var/lib/dnshub/blocklists.lmdb".to_string()
}
fn default_bloom_filter() -> bool {
    true
}
fn default_bloom_fpr() -> f64 {
    0.001
}
