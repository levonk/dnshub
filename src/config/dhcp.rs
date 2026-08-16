//! DHCP configuration structs (`[dhcp]` section of `dnshub.toml`).
//!
//! Story 01-004 introduced a minimal [`DhcpConfig`] skeleton so the main
//! config parsed cleanly. Story 04-004 (this module) expands it with the
//! classification, MAC blocklist, per-pool options, and per-static-lease
//! fields defined in PRD section 4.4 (lines 757-946).
//!
//! The structs are re-exported from [`crate::config`] so existing references
//! (`crate::config::DhcpConfig`) continue to work unchanged.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::dhcp::classification::ClassificationRule;

/// `[dhcp]` — DHCP server configuration (story 04-001+).
///
/// The top-level fields (`pool_start`, `pool_end`, `subnet`, `router`,
/// `lease_time_hours`, `ntp_server`, `domain`) act as the default / legacy
/// single-pool configuration. Richer deployments use the `[[dhcp.pools]]`
/// array for multiple pools with per-pool overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DhcpConfig {
    /// Whether the DHCP server is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Network interface to bind the DHCP listener to, e.g. `"eth0"`.
    #[serde(default)]
    pub interface: String,
    /// DHCP listen address, e.g. `"0.0.0.0:67"`.
    #[serde(default)]
    pub listen: String,
    /// First address in the default dynamic pool, e.g. `"192.168.1.100"`.
    #[serde(default)]
    pub pool_start: String,
    /// Last address in the default dynamic pool, e.g. `"192.168.1.200"`.
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
    /// Default lease duration in hours.
    #[serde(default)]
    pub lease_time_hours: u32,
    /// NTP server option, e.g. `"172.20.255.55"`.
    #[serde(default)]
    pub ntp_server: String,

    /// Per-pool / per-subnet configuration (PRD lines 779-931).
    #[serde(default)]
    pub pools: Vec<DhcpPoolConfig>,

    /// MAC blocklist entries — devices refused DHCP (PRD lines 810-817).
    #[serde(default)]
    pub mac_blocklist: Vec<MacBlocklistConfig>,

    /// Client classification rules (PRD lines 819-830).
    #[serde(default)]
    pub classify: Vec<ClassifyConfig>,

    /// Static lease assignments (MAC → IP + profile) (PRD lines 933-945).
    #[serde(default, rename = "static")]
    pub static_leases: Vec<StaticLeaseConfig>,
}

/// `[[dhcp.pools]]` — a single IP pool with its own options and lease time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DhcpPoolConfig {
    /// Human-readable pool name, e.g. `"main"`, `"guest"`, `"iot"`.
    pub name: String,
    /// Subnet CIDR, e.g. `"192.168.1.0/24"`.
    #[serde(default)]
    pub subnet: String,
    /// First address in the pool, e.g. `"192.168.1.100"`.
    pub pool_start: String,
    /// Last address in the pool, e.g. `"192.168.1.200"`.
    pub pool_end: String,
    /// Router/gateway for this pool.
    #[serde(default)]
    pub router: String,
    /// Lease duration in hours for this pool.
    #[serde(default)]
    pub lease_time_hours: u32,
    /// Default profile for clients on this pool (PRD line 909).
    #[serde(default)]
    pub default_profile: Option<String>,
    /// Arbitrary DHCP options, keyed by option-code string (e.g. `"28"`).
    #[serde(default)]
    pub options: HashMap<String, String>,
}

impl DhcpPoolConfig {
    /// Lease time in seconds.
    pub fn lease_time_secs(&self) -> u32 {
        self.lease_time_hours.saturating_mul(3600)
    }
}

/// `[[dhcp.mac_blocklist]]` — a single blocklist entry.
///
/// Exactly one of `mac` or `oui` should be set. If both are set, `mac` takes
/// precedence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacBlocklistConfig {
    /// Full MAC address to block, e.g. `"00:11:22:33:44:99"`.
    #[serde(default)]
    pub mac: Option<String>,
    /// OUI vendor prefix to block, e.g. `"00:11:22"`.
    #[serde(default)]
    pub oui: Option<String>,
    /// Optional human-readable reason for the block.
    #[serde(default)]
    pub reason: Option<String>,
}

impl MacBlocklistConfig {
    /// Return the MAC-or-OUI string to block, preferring `mac` over `oui`.
    /// Returns `None` if neither is set.
    pub fn mac_or_oui(&self) -> Option<&str> {
        self.mac.as_deref().or(self.oui.as_deref())
    }
}

/// `[[dhcp.classify]]` — a single classification rule mapping a matcher to a
/// profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifyConfig {
    /// The match predicate. Exactly one of `vendor_class`, `oui`, or
    /// `user_class` should be set.
    #[serde(rename = "match")]
    pub match_: ClassifyMatch,
    /// The profile to assign on match, e.g. `"phones"`.
    pub profile: String,
}

impl ClassifyConfig {
    /// Convert this config entry into a [`ClassificationRule`].
    ///
    /// Returns `None` if no matcher is set or the OUI string is unparseable.
    pub fn to_rule(&self) -> Option<ClassificationRule> {
        let m = &self.match_;
        if let Some(vc) = &m.vendor_class {
            return Some(ClassificationRule::vendor_class(vc));
        }
        if let Some(oui) = &m.oui {
            let oui = ClassificationRule::parse_oui(oui)?;
            return Some(ClassificationRule::oui(oui));
        }
        if let Some(uc) = &m.user_class {
            return Some(ClassificationRule::user_class(uc));
        }
        None
    }
}

/// The `match` sub-table of `[[dhcp.classify]]`.
///
/// Exactly one field should be set:
/// - `vendor_class` — prefix match on option 60.
/// - `oui` — match on the first 3 octets of the MAC (e.g. `"B8:27:EB"`).
/// - `user_class` — exact match on option 77.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassifyMatch {
    /// Prefix to match against DHCP option 60 (vendor class).
    #[serde(default)]
    pub vendor_class: Option<String>,
    /// OUI to match against the first 3 MAC octets.
    #[serde(default)]
    pub oui: Option<String>,
    /// Exact value to match against DHCP option 77 (user class).
    #[serde(default)]
    pub user_class: Option<String>,
}

/// `[[dhcp.static]]` — a static lease assignment (MAC → IP + profile).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticLeaseConfig {
    /// Client MAC address, e.g. `"00:11:22:33:44:55"`.
    pub mac: String,
    /// Fixed IPv4 address, e.g. `"192.168.1.10"`.
    pub ip: String,
    /// Optional hostname.
    #[serde(default)]
    pub hostname: Option<String>,
    /// Forced policy profile for this device.
    #[serde(default)]
    pub profile: Option<String>,
    /// Optional per-lease lease-time override in hours (PRD line 945).
    #[serde(default)]
    pub lease_time_hours: Option<u32>,
}

impl StaticLeaseConfig {
    /// Per-lease lease-time override in seconds, if set.
    pub fn lease_time_secs(&self) -> Option<u32> {
        self.lease_time_hours.map(|h| h.saturating_mul(3600))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_config_to_rule_vendor_class() {
        let cfg = ClassifyConfig {
            match_: ClassifyMatch {
                vendor_class: Some("Android-".to_string()),
                oui: None,
                user_class: None,
            },
            profile: "phones".to_string(),
        };
        let rule = cfg.to_rule().expect("rule");
        assert_eq!(rule, ClassificationRule::vendor_class("Android-"));
    }

    #[test]
    fn classify_config_to_rule_oui() {
        let cfg = ClassifyConfig {
            match_: ClassifyMatch {
                vendor_class: None,
                oui: Some("B8:27:EB".to_string()),
                user_class: None,
            },
            profile: "iot".to_string(),
        };
        let rule = cfg.to_rule().expect("rule");
        assert_eq!(rule, ClassificationRule::oui([0xb8, 0x27, 0xeb]));
    }

    #[test]
    fn classify_config_to_rule_user_class() {
        let cfg = ClassifyConfig {
            match_: ClassifyMatch {
                vendor_class: None,
                oui: None,
                user_class: Some("guest".to_string()),
            },
            profile: "guest".to_string(),
        };
        let rule = cfg.to_rule().expect("rule");
        assert_eq!(rule, ClassificationRule::user_class("guest"));
    }

    #[test]
    fn classify_config_no_matcher_returns_none() {
        let cfg = ClassifyConfig {
            match_: ClassifyMatch {
                vendor_class: None,
                oui: None,
                user_class: None,
            },
            profile: "x".to_string(),
        };
        assert!(cfg.to_rule().is_none());
    }

    #[test]
    fn classify_config_bad_oui_returns_none() {
        let cfg = ClassifyConfig {
            match_: ClassifyMatch {
                vendor_class: None,
                oui: Some("nope".to_string()),
                user_class: None,
            },
            profile: "x".to_string(),
        };
        assert!(cfg.to_rule().is_none());
    }

    #[test]
    fn mac_blocklist_config_prefers_mac() {
        let cfg = MacBlocklistConfig {
            mac: Some("00:11:22:33:44:55".to_string()),
            oui: Some("00:11:22".to_string()),
            reason: Some("x".to_string()),
        };
        assert_eq!(cfg.mac_or_oui(), Some("00:11:22:33:44:55"));
    }

    #[test]
    fn mac_blocklist_config_oui_only() {
        let cfg = MacBlocklistConfig {
            mac: None,
            oui: Some("00:11:22".to_string()),
            reason: None,
        };
        assert_eq!(cfg.mac_or_oui(), Some("00:11:22"));
    }

    #[test]
    fn mac_blocklist_config_neither() {
        let cfg = MacBlocklistConfig::default();
        assert_eq!(cfg.mac_or_oui(), None);
    }

    #[test]
    fn static_lease_lease_time_secs() {
        let mut lease = StaticLeaseConfig {
            mac: "00:11:22:33:44:55".to_string(),
            ip: "192.168.1.10".to_string(),
            hostname: None,
            profile: Some("kids".to_string()),
            lease_time_hours: Some(1),
        };
        assert_eq!(lease.lease_time_secs(), Some(3600));
        lease.lease_time_hours = None;
        assert_eq!(lease.lease_time_secs(), None);
    }

    #[test]
    fn dhcp_config_parses_full_example() {
        let toml = r#"
enabled = true
interface = "eth0"
listen = "0.0.0.0:67"
domain = "levonk.com"
ntp_server = "172.20.255.55"

[[pools]]
name = "main"
subnet = "192.168.1.0/24"
pool_start = "192.168.1.100"
pool_end = "192.168.1.200"
router = "192.168.1.1"
lease_time_hours = 24
options = { 28 = "192.168.1.255", 119 = "levonk.com" }

[[pools]]
name = "guest"
subnet = "192.168.10.0/24"
pool_start = "192.168.10.100"
pool_end = "192.168.10.200"
router = "192.168.10.1"
lease_time_hours = 4
options = { 6 = "192.168.10.67" }

[[mac_blocklist]]
mac = "00:11:22:33:44:99"
reason = "banned device"

[[mac_blocklist]]
oui = "00:11:22"
reason = "known-bad IoT vendor"

[[classify]]
match = { vendor_class = "Android-" }
profile = "phones"

[[classify]]
match = { oui = "B8:27:EB" }
profile = "iot"

[[classify]]
match = { user_class = "guest" }
profile = "guest"

[[static]]
mac = "00:11:22:33:44:55"
ip = "192.168.1.10"
hostname = "dad-laptop"
profile = "parents"

[[static]]
mac = "00:11:22:33:44:57"
ip = "192.168.1.20"
hostname = "kids-tablet"
profile = "kids"
lease_time_hours = 1
"#;
        let cfg: DhcpConfig = toml::from_str(toml).expect("parse");
        assert!(cfg.enabled);
        assert_eq!(cfg.pools.len(), 2);
        assert_eq!(cfg.pools[0].name, "main");
        assert_eq!(cfg.pools[0].options.get("28"), Some(&"192.168.1.255".to_string()));
        assert_eq!(cfg.pools[1].options.get("6"), Some(&"192.168.10.67".to_string()));
        assert_eq!(cfg.mac_blocklist.len(), 2);
        assert_eq!(cfg.mac_blocklist[0].mac.as_deref(), Some("00:11:22:33:44:99"));
        assert_eq!(cfg.mac_blocklist[1].oui.as_deref(), Some("00:11:22"));
        assert_eq!(cfg.classify.len(), 3);
        assert_eq!(cfg.classify[0].profile, "phones");
        assert_eq!(cfg.classify[0].match_.vendor_class.as_deref(), Some("Android-"));
        assert_eq!(cfg.classify[1].match_.oui.as_deref(), Some("B8:27:EB"));
        assert_eq!(cfg.classify[2].match_.user_class.as_deref(), Some("guest"));
        assert_eq!(cfg.static_leases.len(), 2);
        assert_eq!(cfg.static_leases[0].hostname.as_deref(), Some("dad-laptop"));
        assert_eq!(cfg.static_leases[1].lease_time_hours, Some(1));
    }

    #[test]
    fn dhcp_config_defaults_empty() {
        let cfg: DhcpConfig = toml::from_str("").expect("parse empty");
        assert!(!cfg.enabled);
        assert!(cfg.pools.is_empty());
        assert!(cfg.mac_blocklist.is_empty());
        assert!(cfg.classify.is_empty());
        assert!(cfg.static_leases.is_empty());
    }
}
