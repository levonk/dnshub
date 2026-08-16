//! DHCPv4 server configuration structs.
//!
//! These mirror the `[dhcp]` and `[[dhcp.pools]]` / `[[dhcp.static]]`
//! sections of `dnshub.toml` (PRD section 4.4, lines 758-946). The
//! top-level [`crate::config::DhcpConfig`] holds a [`DhcpV4Config`] in its
//! `v4` field so the v4 server can be configured independently of the
//! (future) v6 server.
//!
//! Each pool ([`DhcpPoolV4`]) describes a single subnet range with its
//! own router, lease time, and arbitrary DHCP options. Static leases
//! ([`StaticLeaseV4`]) bind a MAC address to a fixed IP and optional
//! policy profile.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// DHCPv4 server configuration (`[dhcp.v4]` / `[dhcp]` v4 fields).
///
/// Fields default to disabled so a config without a `[dhcp]` section
/// parses cleanly and the server stays off.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DhcpV4Config {
    /// Master switch for the DHCPv4 server.
    #[serde(default)]
    pub enabled: bool,
    /// Network interface to bind the DHCP listener to, e.g. `"eth0"`.
    #[serde(default)]
    pub interface: String,
    /// DHCP listen address, e.g. `"0.0.0.0:67"`.
    #[serde(default)]
    pub listen: String,
    /// Domain name option (DHCP option 15), e.g. `"levonk.com"`.
    #[serde(default)]
    pub domain: String,
    /// NTP server option (DHCP option 42), e.g. `"172.20.255.55"`.
    #[serde(default)]
    pub ntp_server: String,
    /// IP pools served by this server. Each pool has its own subnet
    /// range, router, lease time, and options.
    #[serde(default)]
    pub pools: Vec<DhcpPoolV4>,
    /// Static leases (MAC → IP) honored before dynamic allocation.
    #[serde(default)]
    pub static_leases: Vec<StaticLeaseV4>,
}

/// A single DHCPv4 address pool (`[[dhcp.pools]]`).
///
/// Pools are independent subnet ranges. The pool allocator walks the
/// configured pools in order when searching for a free address.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DhcpPoolV4 {
    /// Human-readable pool name, e.g. `"main"`, `"guest"`.
    pub name: String,
    /// Subnet in CIDR notation, e.g. `"192.168.1.0/24"`.
    pub subnet: String,
    /// First address in the dynamic pool, e.g. `"192.168.1.100"`.
    pub pool_start: String,
    /// Last address in the dynamic pool, e.g. `"192.168.1.200"`.
    pub pool_end: String,
    /// Default gateway/router option (DHCP option 3).
    #[serde(default)]
    pub router: String,
    /// Lease duration in hours.
    #[serde(default = "default_lease_time_hours")]
    pub lease_time_hours: u32,
    /// Arbitrary DHCP options keyed by option code, e.g. `{ 28 = "192.168.1.255" }`.
    #[serde(default)]
    pub options: HashMap<String, String>,
}

fn default_lease_time_hours() -> u32 {
    24
}

impl DhcpPoolV4 {
    /// Parse `pool_start` into an [`Ipv4Addr`].
    pub fn start_addr(&self) -> Result<Ipv4Addr, std::net::AddrParseError> {
        self.pool_start.parse()
    }

    /// Parse `pool_end` into an [`Ipv4Addr`].
    pub fn end_addr(&self) -> Result<Ipv4Addr, std::net::AddrParseError> {
        self.pool_end.parse()
    }

    /// Parse `router` into an [`Ipv4Addr`], returning `None` if empty.
    pub fn router_addr(&self) -> Option<Result<Ipv4Addr, std::net::AddrParseError>> {
        if self.router.is_empty() {
            None
        } else {
            Some(self.router.parse())
        }
    }

    /// Lease time in seconds.
    pub fn lease_time_secs(&self) -> u32 {
        self.lease_time_hours.saturating_mul(3600)
    }
}

/// A static DHCPv4 lease (`[[dhcp.static]]`).
///
/// Static leases bind a MAC address to a fixed IP address, optionally
/// with a hostname and forced policy profile. The server checks static
/// leases before allocating from the dynamic pool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticLeaseV4 {
    /// Client MAC address, e.g. `"00:11:22:33:44:55"`.
    pub mac: String,
    /// Fixed IP address, e.g. `"192.168.1.10"`.
    pub ip: String,
    /// Optional hostname (DHCP option 12).
    #[serde(default)]
    pub hostname: String,
    /// Forced policy profile for this device.
    #[serde(default)]
    pub profile: String,
    /// Optional per-lease override of the pool lease time (hours).
    #[serde(default)]
    pub lease_time_hours: Option<u32>,
}

impl StaticLeaseV4 {
    /// Parse `ip` into an [`Ipv4Addr`].
    pub fn ip_addr(&self) -> Result<Ipv4Addr, std::net::AddrParseError> {
        self.ip.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_disabled() {
        let cfg = DhcpV4Config::default();
        assert!(!cfg.enabled);
        assert!(cfg.pools.is_empty());
        assert!(cfg.static_leases.is_empty());
    }

    #[test]
    fn pool_addr_parsing() {
        let pool = DhcpPoolV4 {
            name: "main".to_string(),
            subnet: "192.168.1.0/24".to_string(),
            pool_start: "192.168.1.100".to_string(),
            pool_end: "192.168.1.200".to_string(),
            router: "192.168.1.1".to_string(),
            lease_time_hours: 24,
            options: HashMap::new(),
        };
        assert_eq!(
            pool.start_addr().unwrap(),
            Ipv4Addr::new(192, 168, 1, 100)
        );
        assert_eq!(pool.end_addr().unwrap(), Ipv4Addr::new(192, 168, 1, 200));
        assert_eq!(
            pool.router_addr().unwrap().unwrap(),
            Ipv4Addr::new(192, 168, 1, 1)
        );
        assert_eq!(pool.lease_time_secs(), 86_400);
    }

    #[test]
    fn pool_empty_router_returns_none() {
        let pool = DhcpPoolV4 {
            name: "main".to_string(),
            subnet: "192.168.1.0/24".to_string(),
            pool_start: "192.168.1.100".to_string(),
            pool_end: "192.168.1.200".to_string(),
            router: String::new(),
            lease_time_hours: 24,
            options: HashMap::new(),
        };
        assert!(pool.router_addr().is_none());
    }

    #[test]
    fn static_lease_ip_parsing() {
        let sl = StaticLeaseV4 {
            mac: "00:11:22:33:44:55".to_string(),
            ip: "192.168.1.10".to_string(),
            hostname: "dad-laptop".to_string(),
            profile: "parents".to_string(),
            lease_time_hours: Some(1),
        };
        assert_eq!(sl.ip_addr().unwrap(), Ipv4Addr::new(192, 168, 1, 10));
    }

    #[test]
    fn serde_roundtrip() {
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

[[static_leases]]
mac = "00:11:22:33:44:55"
ip = "192.168.1.10"
hostname = "dad-laptop"
profile = "parents"
"#;
        let cfg: DhcpV4Config = toml::from_str(toml).expect("parse");
        assert!(cfg.enabled);
        assert_eq!(cfg.interface, "eth0");
        assert_eq!(cfg.pools.len(), 1);
        assert_eq!(cfg.pools[0].name, "main");
        assert_eq!(cfg.static_leases.len(), 1);
        assert_eq!(cfg.static_leases[0].mac, "00:11:22:33:44:55");
    }
}
