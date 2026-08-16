//! Static BOOTP entries (RFC 951).
//!
//! BOOTP is a subset of DHCP for diskless clients: the server returns a
//! fixed IP address and bootfile based on the client's MAC, with no lease
//! renewal (treated as an infinite-lease DHCP lease per PRD line 451-453).
//!
//! dnshub resolves a BOOTP static entry by MAC address and returns the
//! configured `(ip, bootfile, tftp_server)` triple.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use crate::dhcp::pxe::config::BootpStaticEntry;

/// Lookup result for a static BOOTP entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootpAssignment {
    /// Fixed IPv4 address for the client.
    pub ip: Ipv4Addr,
    /// Bootfile name.
    pub bootfile: String,
    /// TFTP server address (where the bootfile lives).
    pub tftp_server: Ipv4Addr,
}

/// In-memory index of static BOOTP entries keyed by MAC address.
#[derive(Debug, Clone, Default)]
pub struct BootpStaticTable {
    entries: HashMap<String, BootpAssignment>,
}

impl BootpStaticTable {
    /// Build a table from the config's `bootp_static` list.
    ///
    /// Entries whose `ip` or `tftp_server` cannot be parsed as IPv4 are
    /// silently skipped (logged by the caller if desired).
    pub fn from_config(entries: &[BootpStaticEntry]) -> Self {
        let mut map = HashMap::new();
        for e in entries {
            let Ok(ip) = e.ip.parse::<Ipv4Addr>() else {
                continue;
            };
            let Ok(tftp_server) = e.tftp_server.parse::<Ipv4Addr>() else {
                continue;
            };
            map.insert(
                normalize_mac(&e.mac),
                BootpAssignment {
                    ip,
                    bootfile: e.bootfile.clone(),
                    tftp_server,
                },
            );
        }
        Self { entries: map }
    }

    /// Look up a static BOOTP assignment by MAC address.
    ///
    /// The MAC is normalized (lowercased, whitespace-trimmed) before
    /// lookup so `"00:11:22:33:44:AA"` and `"00:11:22:33:44:aa"` match.
    pub fn lookup(&self, mac: &str) -> Option<&BootpAssignment> {
        self.entries.get(&normalize_mac(mac))
    }

    /// Number of configured static entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Normalize a MAC address for matching: trim whitespace and lowercase.
/// Accepts common separators (`:`, `-`) — comparison is on the raw
/// string after lowercasing, so callers should keep a consistent format
/// in config.
fn normalize_mac(mac: &str) -> String {
    mac.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<BootpStaticEntry> {
        vec![
            BootpStaticEntry {
                mac: "00:11:22:33:44:AA".to_string(),
                ip: "192.168.1.50".to_string(),
                bootfile: "coreos-install.ipxe".to_string(),
                tftp_server: "192.168.1.67".to_string(),
            },
            BootpStaticEntry {
                mac: "aa:bb:cc:dd:ee:ff".to_string(),
                ip: "192.168.1.51".to_string(),
                bootfile: "netboot.xyz".to_string(),
                tftp_server: "192.168.1.67".to_string(),
            },
        ]
    }

    #[test]
    fn lookup_by_mac_returns_assignment() {
        let table = BootpStaticTable::from_config(&sample_entries());
        let a = table.lookup("00:11:22:33:44:AA").unwrap();
        assert_eq!(a.ip, "192.168.1.50".parse::<Ipv4Addr>().unwrap());
        assert_eq!(a.bootfile, "coreos-install.ipxe");
        assert_eq!(a.tftp_server, "192.168.1.67".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let table = BootpStaticTable::from_config(&sample_entries());
        assert!(table.lookup("00:11:22:33:44:aa").is_some());
        assert!(table.lookup("  00:11:22:33:44:aa  ").is_some());
    }

    #[test]
    fn lookup_unknown_mac_returns_none() {
        let table = BootpStaticTable::from_config(&sample_entries());
        assert!(table.lookup("ff:ff:ff:ff:ff:ff").is_none());
    }

    #[test]
    fn invalid_ip_entries_are_skipped() {
        let entries = vec![BootpStaticEntry {
            mac: "00:11:22:33:44:BB".to_string(),
            ip: "not-an-ip".to_string(),
            bootfile: "x".to_string(),
            tftp_server: "192.168.1.67".to_string(),
        }];
        let table = BootpStaticTable::from_config(&entries);
        assert!(table.is_empty());
        assert!(table.lookup("00:11:22:33:44:BB").is_none());
    }

    #[test]
    fn invalid_tftp_server_entries_are_skipped() {
        let entries = vec![BootpStaticEntry {
            mac: "00:11:22:33:44:CC".to_string(),
            ip: "192.168.1.52".to_string(),
            bootfile: "x".to_string(),
            tftp_server: "bad".to_string(),
        }];
        let table = BootpStaticTable::from_config(&entries);
        assert!(table.is_empty());
    }

    #[test]
    fn empty_config_yields_empty_table() {
        let table = BootpStaticTable::from_config(&[]);
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }
}
