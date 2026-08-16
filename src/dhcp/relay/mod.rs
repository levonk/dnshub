//! DHCP relay agent handler.
//!
//! This module implements the server-side relay handling described in PRD
//! lines 472-495: dnshub receives relayed DHCPv4 requests (giaddr != 0)
//! from routers/switches on other VLANs, selects the appropriate IP pool
//! based on `giaddr`, parses Option 82 for finer-grained profile
//! assignment, and arranges for the response to be sent back to the relay
//! agent unicast (to `giaddr`) rather than broadcast to the client.
//!
//! The handler is transport-agnostic: it operates on decoded
//! [`dhcproto::v4::Message`] values and returns a [`RelayDecision`] that
//! the caller (the DHCPv4 server, story 04-001) is responsible for acting
//! on — i.e. selecting the pool and unicasting the reply to `giaddr`.

pub mod config;
pub mod option82;
pub mod trust;
pub mod v6_relay;

pub use config::{Option82Config, RelayConfig};
pub use option82::Option82;
pub use trust::RelayTrust;
pub use v6_relay::{V6RelayDecision, V6RelayHandler};

use std::net::Ipv4Addr;

use dhcproto::v4::{relay::RelayAgentInformation, DhcpOption, Message, OptionCode};

use crate::config::DhcpPoolConfig;

/// A multi-VLAN pool entry used by the relay handler for selection.
///
/// Wraps a [`DhcpPoolConfig`] (from `[[dhcp.pools]]`) and exposes the
/// fields the relay handler needs for giaddr-based selection. Pools are
/// matched by their `router` (gateway) address: a relayed request whose
/// `giaddr` equals a pool's router is served from that pool.
#[derive(Debug, Clone)]
pub struct RelayPool {
    /// Human-readable pool name, e.g. `"vlan10-guest"`.
    pub name: String,
    /// Gateway / router address for this pool. Matched against `giaddr`.
    pub router: Ipv4Addr,
    /// Default policy profile for clients on this VLAN, e.g. `"guest"`.
    pub default_profile: String,
}

impl RelayPool {
    /// Build a [`RelayPool`] from a [`DhcpPoolConfig`], parsing the
    /// `router` string. Returns `None` if the router address cannot be
    /// parsed (such pools are skipped — surfaced as validation errors).
    pub fn from_config(cfg: &DhcpPoolConfig) -> Option<Self> {
        let router = cfg.router.parse::<Ipv4Addr>().ok()?;
        Some(Self {
            name: cfg.name.clone(),
            router,
            default_profile: cfg.default_profile.clone().unwrap_or_default(),
        })
    }
}

/// The relay handler's decision for a relayed DHCPv4 request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayDecision {
    /// The gateway IP address (giaddr) the request was relayed through.
    pub giaddr: Ipv4Addr,
    /// The selected pool name (matched by giaddr), if any.
    pub pool: Option<String>,
    /// The selected policy profile. Determined by, in priority order:
    /// 1. Option 82 circuit ID → `circuit_id_map` lookup
    /// 2. the pool's `default_profile`
    /// 3. `"default"`
    pub profile: String,
    /// Parsed Option 82 sub-options, if present.
    pub option82: Option<Option82>,
    /// The reply must be sent unicast to `giaddr` (not broadcast).
    pub unicast_to_giaddr: bool,
}

/// Outcome of evaluating a relayed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayOutcome {
    /// The request is relayed and accepted; the caller should serve it
    /// per the enclosed [`RelayDecision`].
    Accepted(RelayDecision),
    /// The request is relayed but the source agent is not trusted.
    Untrusted {
        agent_ip: Ipv4Addr,
        giaddr: Ipv4Addr,
    },
    /// The request is not relayed (giaddr == 0).
    NotRelayed,
}

/// DHCPv4 relay handler.
///
/// Holds the relay configuration, the trust validator, and the set of
/// multi-VLAN pools used for giaddr-based pool selection.
#[derive(Debug, Clone, Default)]
pub struct RelayHandler {
    config: RelayConfig,
    trust: RelayTrust,
    pools: Vec<RelayPool>,
}

impl RelayHandler {
    /// Build a [`RelayHandler`] from a [`RelayConfig`] and a list of
    /// [`DhcpPoolConfig`]s (the `[[dhcp.pools]]` entries).
    pub fn new(config: RelayConfig, pools: &[DhcpPoolConfig]) -> Self {
        let trust = RelayTrust::from_config(&config);
        let pools = pools.iter().filter_map(RelayPool::from_config).collect();
        Self {
            config,
            trust,
            pools,
        }
    }

    /// Build a [`RelayHandler`] from explicit pools (useful for tests).
    pub fn with_pools(config: RelayConfig, pools: Vec<RelayPool>) -> Self {
        let trust = RelayTrust::from_config(&config);
        Self {
            config,
            trust,
            pools,
        }
    }

    /// Returns `true` if `msg` is a relayed DHCPv4 request (giaddr != 0).
    pub fn is_relayed(msg: &Message) -> bool {
        msg.giaddr() != Ipv4Addr::UNSPECIFIED
    }

    /// Returns the configured trusted-agent validator.
    pub fn trust(&self) -> &RelayTrust {
        &self.trust
    }

    /// Returns the configured relay pools.
    pub fn pools(&self) -> &[RelayPool] {
        &self.pools
    }

    /// Returns the relay configuration.
    pub fn config(&self) -> &RelayConfig {
        &self.config
    }

    /// Evaluate a decoded DHCPv4 [`Message`].
    ///
    /// - If giaddr == 0, returns [`RelayOutcome::NotRelayed`].
    /// - If the source agent (`giaddr`) is not trusted, returns
    ///   [`RelayOutcome::Untrusted`] and logs the rejection.
    /// - Otherwise returns [`RelayOutcome::Accepted`] with a
    ///   [`RelayDecision`] containing the selected pool and profile.
    ///
    /// `agent_ip` is the IP the request was received from (the relay
    /// agent's source address). Per RFC 3046 / PRD lines 493-495, only
    /// requests from configured trusted agents are accepted.
    pub fn evaluate(&self, msg: &Message, agent_ip: Ipv4Addr) -> RelayOutcome {
        let giaddr = msg.giaddr();
        if giaddr == Ipv4Addr::UNSPECIFIED {
            return RelayOutcome::NotRelayed;
        }

        if !self.config.enabled {
            tracing::warn!(
                giaddr = %giaddr,
                agent = %agent_ip,
                "relay handling disabled; rejecting relayed request"
            );
            return RelayOutcome::Untrusted { agent_ip, giaddr };
        }

        if !self.trust.is_trusted(agent_ip) {
            tracing::warn!(
                giaddr = %giaddr,
                agent = %agent_ip,
                "untrusted relay agent rejected"
            );
            return RelayOutcome::Untrusted { agent_ip, giaddr };
        }

        let option82 = self.parse_option82(msg);
        let pool = self.select_pool(giaddr);
        let profile = self.select_profile(&pool, option82.as_ref());

        tracing::info!(
            giaddr = %giaddr,
            agent = %agent_ip,
            pool = ?pool,
            profile = %profile,
            circuit_id = ?option82.as_ref().map(|o| o.circuit_id_str()),
            "accepted relayed DHCPv4 request"
        );

        RelayOutcome::Accepted(RelayDecision {
            giaddr,
            pool,
            profile,
            option82,
            unicast_to_giaddr: true,
        })
    }

    /// Select a pool name based on `giaddr`. Returns `None` if no pool's
    /// router matches the giaddr.
    pub fn select_pool(&self, giaddr: Ipv4Addr) -> Option<String> {
        self.pools
            .iter()
            .find(|p| p.router == giaddr)
            .map(|p| p.name.clone())
    }

    /// Select a profile, preferring an Option 82 circuit ID mapping over
    /// the pool's default profile, falling back to `"default"`.
    fn select_profile(&self, pool: &Option<String>, option82: Option<&Option82>) -> String {
        if let Some(opt) = option82 {
            let key = opt.circuit_id_str();
            if let Some(profile) = self.config.option82.circuit_id_map.get(&key) {
                return profile.clone();
            }
        }
        pool.as_ref()
            .and_then(|name| {
                self.pools
                    .iter()
                    .find(|p| &p.name == name)
                    .map(|p| p.default_profile.clone())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    /// Parse Option 82 from the message, if present and enabled.
    fn parse_option82(&self, msg: &Message) -> Option<Option82> {
        if !self.config.option82.enabled {
            return None;
        }
        let info = msg
            .opts()
            .get(OptionCode::RelayAgentInformation)
            .and_then(|opt| match opt {
                DhcpOption::RelayAgentInformation(info) => Some(info),
                _ => None,
            })?;
        option82::from_relay_agent_info(info)
    }
}

/// Convenience: extract the [`RelayAgentInformation`] from a message, if
/// present. Exposed for callers that want to inspect Option 82 directly.
pub fn relay_agent_info(msg: &Message) -> Option<&RelayAgentInformation> {
    msg.opts()
        .get(OptionCode::RelayAgentInformation)
        .and_then(|opt| match opt {
            DhcpOption::RelayAgentInformation(info) => Some(info),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dhcproto::v4::{relay::RelayInfo, Message};

    fn trusted_agent() -> Ipv4Addr {
        "192.168.10.1".parse().unwrap()
    }

    fn relayed_msg(giaddr: &str) -> Message {
        let mut msg = Message::default();
        msg.set_giaddr(giaddr.parse::<Ipv4Addr>().unwrap());
        msg
    }

    fn relayed_msg_with_option82(giaddr: &str, circuit_id: &str) -> Message {
        let mut msg = relayed_msg(giaddr);
        let mut info = RelayAgentInformation::default();
        info.insert(RelayInfo::AgentCircuitId(circuit_id.as_bytes().to_vec()));
        msg.opts_mut()
            .insert(DhcpOption::RelayAgentInformation(info));
        msg
    }

    fn vlan_pools() -> Vec<RelayPool> {
        vec![
            RelayPool {
                name: "vlan10-guest".to_string(),
                router: "192.168.10.1".parse().unwrap(),
                default_profile: "guest".to_string(),
            },
            RelayPool {
                name: "vlan20-iot".to_string(),
                router: "192.168.20.1".parse().unwrap(),
                default_profile: "iot".to_string(),
            },
        ]
    }

    fn handler_with_option82() -> RelayHandler {
        let mut cfg = RelayConfig {
            enabled: true,
            trusted_agents: vec!["192.168.10.1".to_string()],
            option82: Option82Config {
                enabled: true,
                circuit_id_map: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("port-24".to_string(), "guest".to_string());
                    m.insert("port-1".to_string(), "iot".to_string());
                    m
                },
            },
        };
        // also trust the vlan20 agent for some tests
        cfg.trusted_agents.push("192.168.20.1".to_string());
        RelayHandler::with_pools(cfg, vlan_pools())
    }

    #[test]
    fn is_relayed_detects_giaddr() {
        assert!(!RelayHandler::is_relayed(&Message::default()));
        assert!(RelayHandler::is_relayed(&relayed_msg("192.168.10.1")));
    }

    #[test]
    fn not_relayed_when_giaddr_zero() {
        let h = handler_with_option82();
        let msg = Message::default();
        match h.evaluate(&msg, trusted_agent()) {
            RelayOutcome::NotRelayed => {}
            other => panic!("expected NotRelayed, got {other:?}"),
        }
    }

    #[test]
    fn untrusted_agent_rejected() {
        let h = handler_with_option82();
        let msg = relayed_msg("192.168.10.1");
        let rogue: Ipv4Addr = "10.0.0.99".parse().unwrap();
        match h.evaluate(&msg, rogue) {
            RelayOutcome::Untrusted { agent_ip, .. } => {
                assert_eq!(agent_ip, rogue);
            }
            other => panic!("expected Untrusted, got {other:?}"),
        }
    }

    #[test]
    fn select_pool_by_giaddr() {
        let h = handler_with_option82();
        let msg = relayed_msg("192.168.20.1");
        match h.evaluate(&msg, "192.168.20.1".parse().unwrap()) {
            RelayOutcome::Accepted(dec) => {
                assert_eq!(dec.giaddr, "192.168.20.1".parse::<Ipv4Addr>().unwrap());
                assert_eq!(dec.pool.as_deref(), Some("vlan20-iot"));
                assert_eq!(dec.profile, "iot");
                assert!(dec.unicast_to_giaddr);
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn option82_circuit_id_overrides_profile() {
        let h = handler_with_option82();
        // giaddr matches vlan10-guest (default profile "guest"), but
        // circuit ID "port-1" maps to "iot".
        let msg = relayed_msg_with_option82("192.168.10.1", "port-1");
        match h.evaluate(&msg, trusted_agent()) {
            RelayOutcome::Accepted(dec) => {
                assert_eq!(dec.pool.as_deref(), Some("vlan10-guest"));
                assert_eq!(dec.profile, "iot");
                assert_eq!(
                    dec.option82.as_ref().unwrap().circuit_id_str(),
                    "port-1"
                );
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn option82_disabled_returns_none() {
        let mut cfg = RelayConfig {
            enabled: true,
            trusted_agents: vec!["192.168.10.1".to_string()],
            option82: Option82Config {
                enabled: false,
                circuit_id_map: std::collections::HashMap::new(),
            },
        };
        cfg.trusted_agents.push("192.168.20.1".to_string());
        let h = RelayHandler::with_pools(cfg, vlan_pools());
        let msg = relayed_msg_with_option82("192.168.10.1", "port-24");
        match h.evaluate(&msg, trusted_agent()) {
            RelayOutcome::Accepted(dec) => {
                assert!(dec.option82.is_none());
                // falls back to pool default profile
                assert_eq!(dec.profile, "guest");
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn unknown_giaddr_selects_no_pool_default_profile() {
        let h = handler_with_option82();
        // trust a third agent so the request is accepted
        let mut cfg = h.config().clone();
        cfg.trusted_agents.push("192.168.99.1".to_string());
        let h2 = RelayHandler::with_pools(cfg, vlan_pools());
        let msg = relayed_msg("192.168.99.1");
        // giaddr 192.168.99.1 has no matching pool, but the agent is
        // trusted. Use the agent IP as the source.
        match h2.evaluate(&msg, "192.168.99.1".parse().unwrap()) {
            RelayOutcome::Accepted(dec) => {
                assert!(dec.pool.is_none());
                assert_eq!(dec.profile, "default");
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn relay_handler_disabled_rejects() {
        let cfg = RelayConfig {
            enabled: false,
            trusted_agents: vec!["192.168.10.1".to_string()],
            option82: Option82Config::default(),
        };
        let h = RelayHandler::with_pools(cfg, vlan_pools());
        let msg = relayed_msg("192.168.10.1");
        match h.evaluate(&msg, trusted_agent()) {
            RelayOutcome::Untrusted { .. } => {}
            other => panic!("expected Untrusted when disabled, got {other:?}"),
        }
    }

    #[test]
    fn relay_agent_info_helper() {
        let mut msg = Message::default();
        assert!(relay_agent_info(&msg).is_none());
        let mut info = RelayAgentInformation::default();
        info.insert(RelayInfo::AgentCircuitId(vec![1, 2, 3]));
        msg.opts_mut()
            .insert(DhcpOption::RelayAgentInformation(info));
        assert!(relay_agent_info(&msg).is_some());
    }
}
