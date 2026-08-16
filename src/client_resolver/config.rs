//! Serde structs for client mapping entries used by [`ClientResolver`].
//!
//! These mirror the `[clients."IP-or-CIDR"]` entries from `policy.toml`
//! (PRD lines 1030-1048). The [`crate::policy::config::PolicyConfig`]
//! already parses these into a `HashMap<String, ClientMapping>`; this module
//! provides the [`CidrEntry`] adapter used by the resolver's CIDR matching
//! layer.

pub use crate::policy::config::ClientMapping;

use crate::client_resolver::cidr::CidrEntry;
use std::collections::HashMap;

/// Partition client mappings into exact-IP entries and CIDR entries.
///
/// A key is treated as a CIDR entry if it contains a `/`; otherwise it is an
/// exact IP. Invalid entries are silently skipped (they will never match).
pub fn split_mappings(
    clients: &HashMap<String, ClientMapping>,
) -> (HashMap<std::net::IpAddr, String>, Vec<CidrEntry>) {
    let mut exact: HashMap<std::net::IpAddr, String> = HashMap::new();
    let mut cidrs: Vec<CidrEntry> = Vec::new();

    for (key, mapping) in clients {
        if key.contains('/') {
            cidrs.push(CidrEntry {
                cidr: key.clone(),
                profile: mapping.policy.clone(),
            });
        } else if let Ok(ip) = key.parse::<std::net::IpAddr>() {
            exact.insert(ip, mapping.policy.clone());
        }
        // Invalid keys (not an IP, not a CIDR) are silently ignored.
    }

    (exact, cidrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_mappings() {
        let mut clients = HashMap::new();
        clients.insert(
            "192.168.1.10".to_string(),
            ClientMapping {
                name: "dad".to_string(),
                policy: "parents".to_string(),
            },
        );
        clients.insert(
            "192.168.10.0/24".to_string(),
            ClientMapping {
                name: "guest".to_string(),
                policy: "guest".to_string(),
            },
        );
        clients.insert(
            "not-an-ip".to_string(),
            ClientMapping {
                name: "bad".to_string(),
                policy: "x".to_string(),
            },
        );

        let (exact, cidrs) = split_mappings(&clients);
        assert_eq!(exact.len(), 1);
        assert_eq!(
            exact.get(&"192.168.1.10".parse().unwrap()),
            Some(&"parents".to_string())
        );
        assert_eq!(cidrs.len(), 1);
        assert_eq!(cidrs[0].cidr, "192.168.10.0/24");
        assert_eq!(cidrs[0].profile, "guest");
    }
}
