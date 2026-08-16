//! Relay agent trust validation.
//!
//! Only relayed DHCP requests originating from a configured trusted relay
//! agent IP are accepted. This is a security measure: it prevents rogue
//! relays from exhausting IP pools or assigning profiles (PRD lines
//! 493-495).
//!
//! [`RelayTrust`] is constructed from a [`super::config::RelayConfig`] and
//! exposes a single decision method, [`RelayTrust::is_trusted`].

use std::collections::HashSet;
use std::net::Ipv4Addr;

use super::config::RelayConfig;

/// Trust validator for DHCP relay agents.
///
/// Holds a pre-parsed set of trusted relay agent IPv4 addresses. The set
/// is built once at config load time so the per-request check is an O(1)
/// hash lookup.
#[derive(Debug, Clone, Default)]
pub struct RelayTrust {
    trusted: HashSet<Ipv4Addr>,
}

impl RelayTrust {
    /// Build a [`RelayTrust`] from a [`RelayConfig`], parsing the
    /// `trusted_agents` strings into [`Ipv4Addr`]s. Unparseable entries
    /// are silently dropped (they are surfaced as validation errors by
    /// the config validator).
    pub fn from_config(cfg: &RelayConfig) -> Self {
        let trusted = cfg.trusted_agent_addrs().into_iter().collect();
        Self { trusted }
    }

    /// Build a [`RelayTrust`] from an explicit iterator of addresses.
    pub fn from_addrs<I: IntoIterator<Item = Ipv4Addr>>(addrs: I) -> Self {
        Self {
            trusted: addrs.into_iter().collect(),
        }
    }

    /// Returns `true` if `agent_ip` is in the trusted relay agent set.
    ///
    /// When the trust set is empty, no relay is trusted — relay handling
    /// is effectively disabled (the relay handler should reject the
    /// request).
    pub fn is_trusted(&self, agent_ip: Ipv4Addr) -> bool {
        self.trusted.contains(&agent_ip)
    }

    /// Returns `true` if no trusted agents are configured.
    pub fn is_empty(&self) -> bool {
        self.trusted.is_empty()
    }

    /// Returns the number of configured trusted agents.
    pub fn len(&self) -> usize {
        self.trusted.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_ip_is_accepted() {
        let trust = RelayTrust::from_addrs([
            "192.168.10.1".parse().unwrap(),
            "192.168.20.1".parse().unwrap(),
        ]);
        assert!(trust.is_trusted("192.168.10.1".parse().unwrap()));
        assert!(trust.is_trusted("192.168.20.1".parse().unwrap()));
    }

    #[test]
    fn untrusted_ip_is_rejected() {
        let trust = RelayTrust::from_addrs(["192.168.10.1".parse().unwrap()]);
        assert!(!trust.is_trusted("10.0.0.1".parse().unwrap()));
        assert!(!trust.is_trusted("192.168.10.2".parse().unwrap()));
    }

    #[test]
    fn empty_trust_rejects_everything() {
        let trust = RelayTrust::default();
        assert!(trust.is_empty());
        assert!(!trust.is_trusted(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn from_config_parses_trusted_agents() {
        let cfg = RelayConfig {
            enabled: true,
            trusted_agents: vec![
                "192.168.10.1".to_string(),
                "192.168.20.1".to_string(),
                "bad".to_string(),
            ],
            option82: super::super::config::Option82Config::default(),
        };
        let trust = RelayTrust::from_config(&cfg);
        assert_eq!(trust.len(), 2);
        assert!(trust.is_trusted("192.168.10.1".parse().unwrap()));
        assert!(trust.is_trusted("192.168.20.1".parse().unwrap()));
        assert!(!trust.is_trusted("192.168.30.1".parse().unwrap()));
    }
}
