//! Configuration for IPv6 Router Advertisements (RA / SLAAC).
//!
//! The shape mirrors the `[dhcp.v6.ra]` section of `dnshub.toml`
//! (PRD lines 840-848). When `enabled` is `true` the [`RaSender`]
//! (see [`crate::dhcp::ra`]) transmits periodic RA messages on the
//! configured interface and responds to Router Solicitations.
//!
//! [`RaSender`]: crate::dhcp::ra::RaSender

use serde::{Deserialize, Serialize};
use std::net::Ipv6Addr;

/// `[dhcp.v6.ra]` — Router Advertisement / SLAAC configuration.
///
/// Fields follow RFC 4861 (Neighbor Discovery) and RFC 8106 (RDNSS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaConfig {
    /// Whether the RA sender is active.
    #[serde(default)]
    pub enabled: bool,

    /// IPv6 prefix to advertise, e.g. `"fd00:1234:5678::/64"`.
    ///
    /// Parsed into [`RaConfig::prefix_addr`] and [`RaConfig::prefix_len`]
    /// via [`RaConfig::parse_prefix`].
    #[serde(default)]
    pub prefix: String,

    /// Preferred lifetime in seconds for the advertised prefix
    /// (RFC 4861 Prefix Information option).
    #[serde(default = "default_preferred_lifetime_secs")]
    pub preferred_lifetime_secs: u32,

    /// Valid lifetime in seconds for the advertised prefix.
    #[serde(default = "default_valid_lifetime_secs")]
    pub valid_lifetime_secs: u32,

    /// Router lifetime in seconds. Set to 0 to stop advertising as a
    /// default router while still sending DNS info (RDNSS/DNSSL).
    #[serde(default = "default_router_lifetime_secs")]
    pub router_lifetime_secs: u32,

    /// Recursive DNS servers advertised via RDNSS (RFC 8106, option 25).
    /// Each entry should be a valid IPv6 address string.
    #[serde(default)]
    pub rdnss: Vec<String>,

    /// DNS Search List domains advertised via DNSSL (RFC 8106, option 31).
    #[serde(default)]
    pub dnssl: Vec<String>,
}

impl Default for RaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            prefix: String::new(),
            preferred_lifetime_secs: default_preferred_lifetime_secs(),
            valid_lifetime_secs: default_valid_lifetime_secs(),
            router_lifetime_secs: default_router_lifetime_secs(),
            rdnss: Vec::new(),
            dnssl: Vec::new(),
        }
    }
}

fn default_preferred_lifetime_secs() -> u32 {
    3600
}

fn default_valid_lifetime_secs() -> u32 {
    7200
}

fn default_router_lifetime_secs() -> u32 {
    1800
}

impl RaConfig {
    /// Parse the `prefix` string (e.g. `"fd00:1234:5678::/64"`) into the
    /// address and prefix-length components.
    ///
    /// Returns `None` if the string is not a valid CIDR.
    pub fn parse_prefix(&self) -> Option<(Ipv6Addr, u8)> {
        let (addr_str, len_str) = self.prefix.split_once('/')?;
        let addr: Ipv6Addr = addr_str.parse().ok()?;
        let len: u8 = len_str.parse().ok()?;
        if len > 128 {
            return None;
        }
        Some((addr, len))
    }

    /// Parse the `rdnss` entries into `Ipv6Addr`s, skipping invalid ones.
    pub fn parse_rdnss(&self) -> Vec<Ipv6Addr> {
        self.rdnss
            .iter()
            .filter_map(|s| s.parse::<Ipv6Addr>().ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let cfg = RaConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.prefix.is_empty());
        assert_eq!(cfg.preferred_lifetime_secs, 3600);
        assert_eq!(cfg.valid_lifetime_secs, 7200);
        assert_eq!(cfg.router_lifetime_secs, 1800);
    }

    #[test]
    fn parse_valid_prefix() {
        let cfg = RaConfig {
            prefix: "fd00:1234:5678::/64".to_string(),
            ..RaConfig::default()
        };
        let (addr, len) = cfg.parse_prefix().expect("valid prefix");
        assert_eq!(len, 64);
        assert_eq!(addr, "fd00:1234:5678::".parse::<Ipv6Addr>().unwrap());
    }

    #[test]
    fn parse_invalid_prefix_returns_none() {
        let cfg = RaConfig {
            prefix: "not-a-prefix".to_string(),
            ..RaConfig::default()
        };
        assert!(cfg.parse_prefix().is_none());
    }

    #[test]
    fn parse_prefix_too_large_returns_none() {
        let cfg = RaConfig {
            prefix: "fd00::/200".to_string(),
            ..RaConfig::default()
        };
        assert!(cfg.parse_prefix().is_none());
    }

    #[test]
    fn parse_rdnss_filters_invalid() {
        let cfg = RaConfig {
            rdnss: vec![
                "fd00:1234:5678::67".to_string(),
                "not-an-addr".to_string(),
                "2001:db8::1".to_string(),
            ],
            ..RaConfig::default()
        };
        let addrs = cfg.parse_rdnss();
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0], "fd00:1234:5678::67".parse::<Ipv6Addr>().unwrap());
        assert_eq!(addrs[1], "2001:db8::1".parse::<Ipv6Addr>().unwrap());
    }

    #[test]
    fn serde_roundtrip() {
        let toml_str = r#"
enabled = true
prefix = "fd00:1234:5678::/64"
preferred_lifetime_secs = 1800
valid_lifetime_secs = 3600
router_lifetime_secs = 900
rdnss = ["fd00:1234:5678::67"]
dnssl = ["levonk.com"]
"#;
        let cfg: RaConfig = toml::from_str(toml_str).expect("parse");
        assert!(cfg.enabled);
        assert_eq!(cfg.prefix, "fd00:1234:5678::/64");
        assert_eq!(cfg.preferred_lifetime_secs, 1800);
        assert_eq!(cfg.valid_lifetime_secs, 3600);
        assert_eq!(cfg.router_lifetime_secs, 900);
        assert_eq!(cfg.rdnss, vec!["fd00:1234:5678::67"]);
        assert_eq!(cfg.dnssl, vec!["levonk.com"]);
    }
}
