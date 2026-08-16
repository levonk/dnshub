//! Lease events consumed by the DDNS manager.
//!
//! The DHCP server (stories 04-001 / 04-002) emits [`LeaseEvent`]s on a tokio
//! channel. The [`DdnsManager`](super::DdnsManager) subscribes to this channel
//! and translates each event into a DNS zone update:
//!
//! | Event    | DNS action                          |
//! |----------|-------------------------------------|
//! | `Grant`  | add A/AAAA record for `hostname`    |
//! | `Release`| remove the A/AAAA record            |
//! | `Expire` | remove the A/AAAA record            |
//!
//! Events that carry no hostname (`hostname` is `None` or empty) are skipped
//! by the manager — no empty-named records are ever created.

use std::net::IpAddr;

/// A DHCP lease lifecycle event.
///
/// Produced by the DHCP v4/v6 servers and consumed by the
/// [`DdnsManager`](super::DdnsManager).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseEvent {
    /// A lease was granted (DHCPACK / DHCPv6 REPLY).
    ///
    /// The manager adds an A record (IPv4) or AAAA record (IPv6) for
    /// `hostname` pointing at `ip`.
    Grant {
        /// Client MAC address (or DUID for DHCPv6, rendered as a string).
        mac: String,
        /// Assigned IP address (IPv4 → A, IPv6 → AAAA).
        ip: IpAddr,
        /// Hostname from DHCP option 12 (v4) / option 39 (v6).
        /// `None` or empty means the client did not send a hostname; the
        /// event is skipped.
        hostname: Option<String>,
    },

    /// A lease was explicitly released (DHCPRELEASE / DHCPv6 RELEASE).
    ///
    /// The manager removes the A/AAAA record for `hostname` pointing at `ip`.
    Release {
        mac: String,
        ip: IpAddr,
        hostname: Option<String>,
    },

    /// A lease expired (lease time elapsed, no renewal).
    ///
    /// The manager removes the A/AAAA record for `hostname` pointing at `ip`.
    Expire {
        mac: String,
        ip: IpAddr,
        hostname: Option<String>,
    },
}

impl LeaseEvent {
    /// Returns the MAC (or DUID) of the client associated with this event.
    pub fn mac(&self) -> &str {
        match self {
            LeaseEvent::Grant { mac, .. }
            | LeaseEvent::Release { mac, .. }
            | LeaseEvent::Expire { mac, .. } => mac,
        }
    }

    /// Returns the IP address associated with this event.
    pub fn ip(&self) -> IpAddr {
        match self {
            LeaseEvent::Grant { ip, .. }
            | LeaseEvent::Release { ip, .. }
            | LeaseEvent::Expire { ip, .. } => *ip,
        }
    }

    /// Returns the hostname (if any) associated with this event.
    ///
    /// Returns `None` when the client did not send a hostname or the hostname
    /// is empty/whitespace-only.
    pub fn hostname(&self) -> Option<&str> {
        match self {
            LeaseEvent::Grant { hostname, .. }
            | LeaseEvent::Release { hostname, .. }
            | LeaseEvent::Expire { hostname, .. } => {
                hostname.as_deref().map(|h| h.trim()).filter(|h| !h.is_empty())
            }
        }
    }

    /// Returns `true` if this event should trigger a DNS record addition
    /// (i.e. it is a `Grant` with a non-empty hostname).
    pub fn is_add(&self) -> bool {
        matches!(self, LeaseEvent::Grant { .. }) && self.hostname().is_some()
    }

    /// Returns `true` if this event should trigger a DNS record removal
    /// (i.e. it is a `Release` or `Expire` with a non-empty hostname).
    pub fn is_remove(&self) -> bool {
        matches!(self, LeaseEvent::Release { .. } | LeaseEvent::Expire { .. })
            && self.hostname().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_with_hostname_is_add() {
        let ev = LeaseEvent::Grant {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ip: "192.168.1.10".parse().unwrap(),
            hostname: Some("phone".into()),
        };
        assert!(ev.is_add());
        assert!(!ev.is_remove());
        assert_eq!(ev.hostname(), Some("phone"));
        assert_eq!(ev.mac(), "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn grant_without_hostname_is_neither_add_nor_remove() {
        let ev = LeaseEvent::Grant {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ip: "192.168.1.10".parse().unwrap(),
            hostname: None,
        };
        assert!(!ev.is_add());
        assert!(!ev.is_remove());
        assert_eq!(ev.hostname(), None);
    }

    #[test]
    fn grant_with_empty_hostname_is_skipped() {
        let ev = LeaseEvent::Grant {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ip: "192.168.1.10".parse().unwrap(),
            hostname: Some("   ".into()),
        };
        assert!(!ev.is_add());
        assert_eq!(ev.hostname(), None);
    }

    #[test]
    fn release_with_hostname_is_remove() {
        let ev = LeaseEvent::Release {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ip: "192.168.1.10".parse().unwrap(),
            hostname: Some("phone".into()),
        };
        assert!(ev.is_remove());
        assert!(!ev.is_add());
    }

    #[test]
    fn expire_with_hostname_is_remove() {
        let ev = LeaseEvent::Expire {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ip: "fd00::1".parse().unwrap(),
            hostname: Some("phone".into()),
        };
        assert!(ev.is_remove());
    }

    #[test]
    fn release_without_hostname_is_not_remove() {
        let ev = LeaseEvent::Release {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ip: "192.168.1.10".parse().unwrap(),
            hostname: None,
        };
        assert!(!ev.is_remove());
    }

    #[test]
    fn ipv6_event_returns_aaaa_ip() {
        let ip: IpAddr = "fd00::42".parse().unwrap();
        let ev = LeaseEvent::Grant {
            mac: "duid-xyz".into(),
            ip,
            hostname: Some("iot".into()),
        };
        assert!(ev.ip().is_ipv6());
    }
}
