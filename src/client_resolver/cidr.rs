//! CIDR range matching for client policy resolution (PRD section 4.5).
//!
//! Uses the [`ipnet`] crate to test whether a client IP falls inside a CIDR
//! range. This is the fallback mechanism after exact-IP matching: if no
//! static mapping matches the client IP exactly, the resolver checks CIDR
//! ranges (e.g. `192.168.10.0/24` → `guest`).

use ipnet::IpNet;
use std::net::IpAddr;

/// Check whether `ip` falls inside `cidr`.
///
/// `cidr` may be an IPv4 or IPv6 CIDR string (e.g. `"192.168.10.0/24"`).
/// Returns `false` if `cidr` is not a valid CIDR or if the IP family differs.
pub fn match_ip(ip: IpAddr, cidr: &str) -> bool {
    let Ok(net) = cidr.parse::<IpNet>() else {
        return false;
    };
    net.contains(&ip)
}

/// A CIDR range paired with the profile name it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CidrEntry {
    /// The CIDR range string (e.g. `"192.168.10.0/24"`).
    pub cidr: String,
    /// The profile name to apply for clients in this range.
    pub profile: String,
}

/// Find the first CIDR entry that contains `ip`, returning the associated
/// profile name.
///
/// Entries are checked in order; the first match wins. This is O(n) with
/// small n (typically a handful of CIDR ranges per deployment).
pub fn find_matching_cidr<'a>(ip: IpAddr, entries: &'a [CidrEntry]) -> Option<&'a str> {
    for entry in entries {
        if match_ip(ip, &entry.cidr) {
            return Some(&entry.profile);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn test_exact_cidr_match() {
        assert!(match_ip(ip("192.168.10.50"), "192.168.10.0/24"));
    }

    #[test]
    fn test_cidr_no_match() {
        assert!(!match_ip(ip("192.168.1.50"), "192.168.10.0/24"));
    }

    #[test]
    fn test_cidr_boundary() {
        assert!(match_ip(ip("192.168.10.0"), "192.168.10.0/24"));
        assert!(match_ip(ip("192.168.10.255"), "192.168.10.0/24"));
        assert!(!match_ip(ip("192.168.11.0"), "192.168.10.0/24"));
    }

    #[test]
    fn test_invalid_cidr() {
        assert!(!match_ip(ip("192.168.1.1"), "not-a-cidr"));
    }

    #[test]
    fn test_ipv6_cidr() {
        assert!(match_ip(
            ip("2001:db8::1"),
            "2001:db8::/32"
        ));
        assert!(!match_ip(ip("2001:db9::1"), "2001:db8::/32"));
    }

    #[test]
    fn test_find_matching_cidr() {
        let entries = vec![
            CidrEntry {
                cidr: "192.168.10.0/24".to_string(),
                profile: "guest".to_string(),
            },
            CidrEntry {
                cidr: "192.168.20.0/24".to_string(),
                profile: "iot".to_string(),
            },
        ];
        assert_eq!(
            find_matching_cidr(ip("192.168.10.50"), &entries),
            Some("guest")
        );
        assert_eq!(
            find_matching_cidr(ip("192.168.20.5"), &entries),
            Some("iot")
        );
        assert_eq!(
            find_matching_cidr(ip("10.0.0.1"), &entries),
            None
        );
    }

    #[test]
    fn test_first_match_wins() {
        let entries = vec![
            CidrEntry {
                cidr: "192.168.0.0/16".to_string(),
                profile: "broad".to_string(),
            },
            CidrEntry {
                cidr: "192.168.10.0/24".to_string(),
                profile: "narrow".to_string(),
            },
        ];
        // 192.168.10.50 matches both; the first entry wins.
        assert_eq!(
            find_matching_cidr(ip("192.168.10.50"), &entries),
            Some("broad")
        );
    }

    #[test]
    fn test_mismatched_ip_family() {
        assert!(!match_ip(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            "2001:db8::/32"
        ));
    }
}
