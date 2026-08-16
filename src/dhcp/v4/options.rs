//! DHCPv4 standard option builder.
//!
//! Builds [`dhcproto::v4::DhcpOption`] values from pool/server config.
//! Supports the standard options listed in the story scope:
//!
//! | Code | Option                  | Source |
//! |------|-------------------------|--------|
//! | 3    | Router (gateway)        | pool `router` |
//! | 6    | DNS server              | pool `options[6]` or server IP |
//! | 15   | Domain name             | server `domain` |
//! | 28   | Broadcast address       | pool `options[28]` or computed |
//! | 42   | NTP server              | server `ntp_server` |
//! | 51   | IP address lease time   | pool `lease_time_hours` |
//! | 121  | Classless static routes | pool `options[121]` |
//! | 252  | WPAD URL                | pool `options[252]` |
//!
//! Arbitrary options from the pool `options` map (keyed by numeric code
//! as a string) are also parsed and emitted as `DhcpOption::Unknown` so
//! any RFC 2132 option can be configured without code changes.

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, OptionCode, UnknownOption};
use ipnet::Ipv4Net;

use super::config::{DhcpPoolV4, DhcpV4Config};

/// Builds [`DhcpOption`]s from pool and server configuration.
pub struct DhcpOptionBuilder<'a> {
    /// The server's own IP address (used as the DNS server option when
    /// the pool doesn't override it, and as the server identifier).
    server_ip: Ipv4Addr,
    /// Top-level DHCPv4 config (domain, ntp_server).
    config: &'a DhcpV4Config,
}

impl<'a> DhcpOptionBuilder<'a> {
    /// Create a new builder.
    pub fn new(server_ip: Ipv4Addr, config: &'a DhcpV4Config) -> Self {
        Self { server_ip, config }
    }

    /// Build the full set of standard options for `pool`.
    ///
    /// Returns a `Vec<DhcpOption>` suitable for inserting into a
    /// `dhcproto::v4::Message`'s options map. The message type option
    /// (53) and server identifier (54) are added separately by the
    /// state machine.
    pub fn build_pool_options(&self, pool: &DhcpPoolV4) -> Vec<DhcpOption> {
        let mut opts = Vec::new();

        // Option 3 — Router (gateway)
        if let Some(Ok(router)) = pool.router_addr() {
            opts.push(DhcpOption::Router(vec![router]));
        }

        // Option 6 — DNS server. Use pool override or the server itself.
        if let Some(dns) = self.parse_option_ipv4s(pool, "6") {
            opts.push(DhcpOption::DomainNameServer(dns));
        } else {
            opts.push(DhcpOption::DomainNameServer(vec![self.server_ip]));
        }

        // Option 15 — Domain name
        if !self.config.domain.is_empty() {
            opts.push(DhcpOption::DomainName(self.config.domain.clone()));
        }

        // Option 28 — Broadcast address
        if let Some(Ok(bcast)) = pool.options.get("28").map(|s| s.parse::<Ipv4Addr>()) {
            opts.push(DhcpOption::BroadcastAddr(bcast));
        } else if let Some(bcast) = self.compute_broadcast(pool) {
            opts.push(DhcpOption::BroadcastAddr(bcast));
        }

        // Option 42 — NTP server
        if !self.config.ntp_server.is_empty() {
            if let Ok(ntp) = self.config.ntp_server.parse::<Ipv4Addr>() {
                opts.push(DhcpOption::NtpServers(vec![ntp]));
            }
        }

        // Option 51 — IP address lease time (seconds)
        opts.push(DhcpOption::AddressLeaseTime(pool.lease_time_secs()));

        // Option 121 — Classless static routes
        if let Some(routes) = self.parse_classless_static_routes(pool) {
            if !routes.is_empty() {
                opts.push(DhcpOption::ClasslessStaticRoute(routes));
            }
        }

        // Option 252 — WPAD URL (emitted as Unknown option)
        if let Some(wpad) = pool.options.get("252") {
            if !wpad.is_empty() {
                opts.push(DhcpOption::Unknown(UnknownOption::new(
                    OptionCode::Unknown(252),
                    wpad.as_bytes().to_vec(),
                )));
            }
        }

        // Emit any other arbitrary options not handled above.
        for (key, value) in &pool.options {
            let code: u8 = match key.parse() {
                Ok(c) => c,
                Err(_) => continue,
            };
            // Skip codes we already handled explicitly.
            if matches!(code, 3 | 6 | 28 | 121 | 252) {
                continue;
            }
            if value.is_empty() {
                continue;
            }
            opts.push(self.build_arbitrary_option(code, value));
        }

        opts
    }

    /// Parse a comma-separated list of IPv4 addresses from the pool
    /// `options` map (e.g. `"192.168.1.1,192.168.1.2"`).
    fn parse_option_ipv4s(&self, pool: &DhcpPoolV4, key: &str) -> Option<Vec<Ipv4Addr>> {
        let raw = pool.options.get(key)?;
        let addrs: Vec<Ipv4Addr> = raw
            .split(',')
            .filter_map(|s| s.trim().parse::<Ipv4Addr>().ok())
            .collect();
        if addrs.is_empty() {
            None
        } else {
            Some(addrs)
        }
    }

    /// Compute the broadcast address for `pool` from its subnet CIDR.
    fn compute_broadcast(&self, pool: &DhcpPoolV4) -> Option<Ipv4Addr> {
        let net: Ipv4Net = pool.subnet.parse().ok()?;
        Some(net.broadcast())
    }

    /// Parse option 121 (classless static routes) from the pool options.
    ///
    /// Format: `"10.0.0.0/8,192.168.30.1;192.168.0.0/16,10.0.0.1"` —
    /// semicolon-separated `dest/gw` pairs, each `CIDR,GATEWAY`.
    fn parse_classless_static_routes(&self, pool: &DhcpPoolV4) -> Option<Vec<(Ipv4Net, Ipv4Addr)>> {
        let raw = pool.options.get("121")?;
        let mut routes = Vec::new();
        for entry in raw.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let parts: Vec<&str> = entry.split(',').collect();
            if parts.len() != 2 {
                continue;
            }
            let dest: Ipv4Net = match parts[0].trim().parse() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let gw: Ipv4Addr = match parts[1].trim().parse() {
                Ok(a) => a,
                Err(_) => continue,
            };
            routes.push((dest, gw));
        }
        Some(routes)
    }

    /// Build an arbitrary option as `DhcpOption::Unknown`. For options
    /// that dhcproto has native types for (a small subset), we try to
    /// parse the value; otherwise we emit raw bytes.
    fn build_arbitrary_option(&self, code: u8, value: &str) -> DhcpOption {
        // Try to parse as a single IPv4 address for known single-addr options.
        if let Ok(addr) = value.parse::<Ipv4Addr>() {
            match code {
                1 => return DhcpOption::SubnetMask(addr),
                16 => return DhcpOption::SwapServer(addr),
                32 => return DhcpOption::RouterSolicitationAddr(addr),
                50 => return DhcpOption::RequestedIpAddress(addr),
                54 => return DhcpOption::ServerIdentifier(addr),
                150 => return DhcpOption::TFTPServerAddress(addr),
                _ => {}
            }
        }
        // Try to parse as comma-separated IPv4 list for multi-addr options.
        let addrs: Vec<Ipv4Addr> = value
            .split(',')
            .filter_map(|s| s.trim().parse::<Ipv4Addr>().ok())
            .collect();
        if !addrs.is_empty() {
            match code {
                4 => return DhcpOption::TimeServer(addrs.clone()),
                5 => return DhcpOption::NameServer(addrs.clone()),
                7 => return DhcpOption::LogServer(addrs.clone()),
                8 => return DhcpOption::QuoteServer(addrs.clone()),
                9 => return DhcpOption::LprServer(addrs.clone()),
                40 => return DhcpOption::NisDomain(value.to_string()),
                41 => return DhcpOption::NisServers(addrs.clone()),
                44 => return DhcpOption::NetBiosNameServers(addrs.clone()),
                45 => return DhcpOption::NetBiosDatagramDistributionServer(addrs.clone()),
                _ => {}
            }
        }
        // Fall back to raw bytes.
        DhcpOption::Unknown(UnknownOption::new(
            OptionCode::Unknown(code),
            value.as_bytes().to_vec(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::v4::config::{DhcpPoolV4, DhcpV4Config};
    use dhcproto::v4::OptionCode;
    use std::collections::HashMap;

    fn make_pool() -> DhcpPoolV4 {
        let mut options = HashMap::new();
        options.insert("28".to_string(), "192.168.1.255".to_string());
        options.insert("121".to_string(), "10.0.0.0/8,192.168.1.1".to_string());
        options.insert("252".to_string(), "http://wpad/wpad.dat".to_string());
        DhcpPoolV4 {
            name: "main".to_string(),
            subnet: "192.168.1.0/24".to_string(),
            pool_start: "192.168.1.100".to_string(),
            pool_end: "192.168.1.200".to_string(),
            router: "192.168.1.1".to_string(),
            lease_time_hours: 24,
            options,
        }
    }

    fn make_config() -> DhcpV4Config {
        DhcpV4Config {
            enabled: true,
            interface: "eth0".to_string(),
            listen: "0.0.0.0:67".to_string(),
            domain: "levonk.com".to_string(),
            ntp_server: "172.20.255.55".to_string(),
            pools: vec![],
            static_leases: vec![],
        }
    }

    #[test]
    fn builds_router_option() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        let router = opts.iter().find(|o| matches!(o, DhcpOption::Router(_)));
        assert!(router.is_some(), "router option should be present");
        if let Some(DhcpOption::Router(ips)) = router {
            assert_eq!(*ips, vec![Ipv4Addr::new(192, 168, 1, 1)]);
        }
    }

    #[test]
    fn builds_dns_option_defaulting_to_server() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        let dns = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::DomainNameServer(_)));
        assert!(dns.is_some());
        if let Some(DhcpOption::DomainNameServer(ips)) = dns {
            assert_eq!(*ips, vec![Ipv4Addr::new(192, 168, 1, 67)]);
        }
    }

    #[test]
    fn builds_dns_option_from_pool_override() {
        let cfg = make_config();
        let mut pool = make_pool();
        pool.options
            .insert("6".to_string(), "192.168.10.67".to_string());
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&pool);
        if let Some(DhcpOption::DomainNameServer(ips)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::DomainNameServer(_)))
        {
            assert_eq!(*ips, vec![Ipv4Addr::new(192, 168, 10, 67)]);
        } else {
            panic!("DNS option missing");
        }
    }

    #[test]
    fn builds_domain_name_option() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        if let Some(DhcpOption::DomainName(d)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::DomainName(_)))
        {
            assert_eq!(d, "levonk.com");
        } else {
            panic!("domain name option missing");
        }
    }

    #[test]
    fn builds_broadcast_option_from_config() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        if let Some(DhcpOption::BroadcastAddr(a)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::BroadcastAddr(_)))
        {
            assert_eq!(*a, Ipv4Addr::new(192, 168, 1, 255));
        } else {
            panic!("broadcast option missing");
        }
    }

    #[test]
    fn computes_broadcast_from_subnet_when_not_configured() {
        let cfg = make_config();
        let mut pool = make_pool();
        pool.options.remove("28");
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&pool);
        if let Some(DhcpOption::BroadcastAddr(a)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::BroadcastAddr(_)))
        {
            assert_eq!(*a, Ipv4Addr::new(192, 168, 1, 255));
        } else {
            panic!("broadcast option should be computed from subnet");
        }
    }

    #[test]
    fn builds_ntp_option() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        if let Some(DhcpOption::NtpServers(ips)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::NtpServers(_)))
        {
            assert_eq!(*ips, vec![Ipv4Addr::new(172, 20, 255, 55)]);
        } else {
            panic!("NTP option missing");
        }
    }

    #[test]
    fn builds_lease_time_option() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        if let Some(DhcpOption::AddressLeaseTime(t)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::AddressLeaseTime(_)))
        {
            assert_eq!(*t, 86_400); // 24 hours
        } else {
            panic!("lease time option missing");
        }
    }

    #[test]
    fn builds_classless_static_routes_option() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        if let Some(DhcpOption::ClasslessStaticRoute(routes)) = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::ClasslessStaticRoute(_)))
        {
            assert_eq!(routes.len(), 1);
            assert_eq!(routes[0].1, Ipv4Addr::new(192, 168, 1, 1));
        } else {
            panic!("classless static route option missing");
        }
    }

    #[test]
    fn builds_wpad_option_as_unknown() {
        let cfg = make_config();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        let wpad = opts.iter().find(|o| {
            matches!(o, DhcpOption::Unknown(u) if u.code() == OptionCode::Unknown(252))
        });
        assert!(wpad.is_some(), "WPAD option should be present");
    }

    #[test]
    fn arbitrary_single_ipv4_option_uses_native_type() {
        let cfg = make_config();
        let mut pool = make_pool();
        pool.options
            .insert("1".to_string(), "255.255.255.0".to_string());
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&pool);
        let mask = opts
            .iter()
            .find(|o| matches!(o, DhcpOption::SubnetMask(_)));
        assert!(mask.is_some(), "subnet mask option should use native type");
        if let Some(DhcpOption::SubnetMask(a)) = mask {
            assert_eq!(*a, Ipv4Addr::new(255, 255, 255, 0));
        }
    }

    #[test]
    fn empty_domain_omits_option() {
        let mut cfg = make_config();
        cfg.domain = String::new();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        assert!(opts
            .iter()
            .all(|o| !matches!(o, DhcpOption::DomainName(_))));
    }

    #[test]
    fn empty_ntp_omits_option() {
        let mut cfg = make_config();
        cfg.ntp_server = String::new();
        let builder = DhcpOptionBuilder::new(Ipv4Addr::new(192, 168, 1, 67), &cfg);
        let opts = builder.build_pool_options(&make_pool());
        assert!(opts.iter().all(|o| !matches!(o, DhcpOption::NtpServers(_))));
    }
}
