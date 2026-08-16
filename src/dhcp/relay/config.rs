//! Configuration for the DHCP relay agent handler.
//!
//! Mirrors the `[dhcp.relay]` and `[dhcp.relay.option82]` sections of
//! `dnshub.toml` (PRD lines 888-910). The relay handler uses this config
//! to decide which relay agents are trusted and how Option 82 circuit IDs
//! map to policy profiles.
//!
//! Multi-VLAN pool configuration lives in `[[dhcp.pools]]` (see
//! [`crate::config::DhcpPoolConfig`]); this module only owns the relay
//! specific knobs.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// `[dhcp.relay]` — relay agent handling configuration.
///
/// When `enabled` is `true`, dnshub accepts relayed DHCPv4 requests
/// (giaddr != 0) and DHCPv6 Relay-Forw messages from the configured
/// trusted agents, selects a pool based on giaddr / link-address, and
/// sends the response back to the relay agent unicast.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayConfig {
    /// Master switch for relay handling. When `false`, relayed requests
    /// are rejected (treated as untrusted).
    #[serde(default)]
    pub enabled: bool,

    /// Trusted relay agent IPv4 addresses. Only requests whose source IP
    /// matches one of these entries are accepted as relayed. This is a
    /// security measure: it prevents rogue relays from exhausting pools
    /// or assigning profiles (PRD lines 493-495).
    #[serde(default)]
    pub trusted_agents: Vec<String>,

    /// Option 82 (Relay Agent Information, RFC 3046) sub-configuration.
    #[serde(default)]
    pub option82: Option82Config,
}

/// `[dhcp.relay.option82]` — Option 82 parsing and profile mapping.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Option82Config {
    /// When `true`, dnshub parses Option 82 sub-options (circuit ID and
    /// remote ID) and uses them for pool / profile selection.
    #[serde(default)]
    pub enabled: bool,

    /// Maps Option 82 circuit IDs (e.g. switch port identifiers such as
    /// `"port-24"`) to policy profile names (e.g. `"guest"`). This is the
    /// mechanism for port-based profile assignment (PRD lines 487-489,
    /// 898-899).
    #[serde(default)]
    pub circuit_id_map: HashMap<String, String>,
}

impl RelayConfig {
    /// Returns `true` if relay handling is enabled and at least one
    /// trusted agent is configured.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.trusted_agents.is_empty()
    }

    /// Parse every `trusted_agents` entry into an [`Ipv4Addr`]. Entries
    /// that fail to parse are skipped (they are surfaced as validation
    /// errors by the config validator).
    pub fn trusted_agent_addrs(&self) -> Vec<Ipv4Addr> {
        self.trusted_agents
            .iter()
            .filter_map(|s| s.parse::<Ipv4Addr>().ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inactive() {
        let cfg = RelayConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.is_active());
        assert!(cfg.trusted_agents.is_empty());
        assert!(!cfg.option82.enabled);
        assert!(cfg.option82.circuit_id_map.is_empty());
    }

    #[test]
    fn is_active_requires_enabled_and_agents() {
        let mut cfg = RelayConfig::default();
        cfg.enabled = true;
        // no agents -> inactive
        assert!(!cfg.is_active());
        cfg.trusted_agents = vec!["192.168.10.1".to_string()];
        assert!(cfg.is_active());
    }

    #[test]
    fn trusted_agent_addrs_parses_valid_ips() {
        let cfg = RelayConfig {
            enabled: true,
            trusted_agents: vec![
                "192.168.10.1".to_string(),
                "not-an-ip".to_string(),
                "192.168.20.1".to_string(),
            ],
            option82: Option82Config::default(),
        };
        let addrs = cfg.trusted_agent_addrs();
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0], "192.168.10.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(addrs[1], "192.168.20.1".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn circuit_id_map_round_trips_through_toml() {
        let mut map = HashMap::new();
        map.insert("port-24".to_string(), "guest".to_string());
        map.insert("port-1".to_string(), "iot".to_string());
        let cfg = Option82Config {
            enabled: true,
            circuit_id_map: map,
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Option82Config = toml::from_str(&s).unwrap();
        assert!(back.enabled);
        assert_eq!(back.circuit_id_map.get("port-24"), Some(&"guest".to_string()));
        assert_eq!(back.circuit_id_map.get("port-1"), Some(&"iot".to_string()));
    }
}
