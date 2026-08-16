//! DDNS configuration (PRD lines 767-770).
//!
//! [`DdnsConfig`] mirrors the `[dhcp.ddns]` table from `dnshub.toml`:
//!
//! ```toml
//! [dhcp.ddns]
//! enabled = true
//! zone = "levonk.com"           # local zone to update
//! ttl = 60                      # short TTL so stale records expire fast
//! ```
//!
//! When `enabled` is `true`, the [`DdnsManager`](super::DdnsManager) subscribes
//! to DHCP lease events and creates/removes A/AAAA records in the local DNS
//! zone for any lease that carries a hostname (DHCP option 12).

use serde::{Deserialize, Serialize};

/// Default TTL (seconds) for DDNS-managed records.
///
/// Per the PRD, a short TTL (60s) is used so that stale records — left behind
/// when a lease expires without a clean RELEASE — become unreachable quickly
/// once the upstream caching resolver's TTL expires.
pub const DEFAULT_DDNS_TTL: u32 = 60;

/// `[dhcp.ddns]` — Dynamic DNS from DHCP leases.
///
/// When enabled, every DHCP lease grant that includes a hostname (option 12)
/// triggers an A (IPv4) or AAAA (IPv6) record creation in the configured local
/// zone. Lease release or expiry removes the corresponding record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DdnsConfig {
    /// Whether DDNS is active. When `false`, the
    /// [`DdnsManager`](super::DdnsManager) drops all incoming lease events.
    #[serde(default)]
    pub enabled: bool,

    /// The local zone name to update, e.g. `"levonk.com"`.
    ///
    /// Hostnames from DHCP leases are appended to this zone origin to form
    /// fully-qualified record names (e.g. `phone.levonk.com`).
    #[serde(default)]
    pub zone: String,

    /// TTL (seconds) for DDNS-managed A/AAAA records.
    ///
    /// Defaults to 60 per the PRD so stale records expire fast.
    #[serde(default = "default_ttl")]
    pub ttl: u32,
}

fn default_ttl() -> u32 {
    DEFAULT_DDNS_TTL
}

impl Default for DdnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            zone: String::new(),
            ttl: DEFAULT_DDNS_TTL,
        }
    }
}

impl DdnsConfig {
    /// Returns `true` if DDNS is enabled **and** a zone is configured.
    ///
    /// The [`DdnsManager`](super::DdnsManager) uses this to decide whether to
    /// process lease events. An enabled config with an empty zone is treated
    /// as disabled because there is no zone to update.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.zone.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_with_empty_zone() {
        let cfg = DdnsConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.zone.is_empty());
        assert_eq!(cfg.ttl, DEFAULT_DDNS_TTL);
        assert!(!cfg.is_active());
    }

    #[test]
    fn enabled_with_zone_is_active() {
        let cfg = DdnsConfig {
            enabled: true,
            zone: "levonk.com".to_string(),
            ttl: 30,
        };
        assert!(cfg.is_active());
    }

    #[test]
    fn enabled_without_zone_is_inactive() {
        let cfg = DdnsConfig {
            enabled: true,
            zone: String::new(),
            ttl: 60,
        };
        assert!(!cfg.is_active());
    }

    #[test]
    fn deserializes_from_toml() {
        let toml = r#"
enabled = true
zone = "home.arpa"
ttl = 120
"#;
        let cfg: DdnsConfig = toml::from_str(toml).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.zone, "home.arpa");
        assert_eq!(cfg.ttl, 120);
        assert!(cfg.is_active());
    }

    #[test]
    fn deserializes_with_default_ttl() {
        let toml = r#"
enabled = true
zone = "home.arpa"
"#;
        let cfg: DdnsConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.ttl, DEFAULT_DDNS_TTL);
    }
}
