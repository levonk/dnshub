//! Serde structs for `policy.toml` (PRD section 4.5, lines 972-1052).
//!
//! The TOML file has four top-level sections:
//!
//! ```toml
//! [default]                      # the catch-all profile
//! [policies.X]                   # named profiles
//! [clients."192.168.1.10"]       # exact-IP → profile mapping
//! [clients."192.168.10.0/24"]    # CIDR → profile mapping
//! [dhcp_integration]             # hostname → profile map (story 04-011)
//! ```

use crate::policy::profile::PolicyProfile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level `policy.toml` configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PolicyConfig {
    /// The default profile applied to unmapped clients (`[default]`).
    #[serde(default)]
    pub default: PolicyProfile,

    /// Named profiles (`[policies.X]`), keyed by profile name.
    #[serde(default)]
    pub policies: HashMap<String, PolicyProfile>,

    /// Client → profile mappings (`[clients."IP-or-CIDR"]`).
    ///
    /// Keys may be exact IPs (`192.168.1.10`) or CIDR ranges
    /// (`192.168.10.0/24`). The [`crate::client_resolver::ClientResolver`]
    /// distinguishes the two at resolution time.
    #[serde(default)]
    pub clients: HashMap<String, ClientMapping>,

    /// DHCP lease integration (`[dhcp_integration]`).
    ///
    /// The `hostname_map` maps DHCP hostnames to profile names. This is
    /// used by the ClientResolver once DHCP lease data is available
    /// (story 04-011); in this story it is parsed but not consulted.
    #[serde(default)]
    pub dhcp_integration: DhcpIntegration,
}

/// A single `[clients."X"]` entry mapping an IP or CIDR to a profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientMapping {
    /// Human-readable client name, e.g. `"dad-laptop"`.
    #[serde(default)]
    pub name: String,

    /// The profile name to apply, e.g. `"parents"`.
    pub policy: String,
}

/// `[dhcp_integration]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DhcpIntegration {
    /// Map of DHCP hostname → profile name.
    #[serde(default)]
    pub hostname_map: HashMap<String, String>,
}

impl PolicyConfig {
    /// Load and parse `policy.toml` from a string.
    pub fn from_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Load and parse `policy.toml` from a file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self, PolicyConfigError> {
        let contents = std::fs::read_to_string(path)?;
        Self::from_str(&contents).map_err(PolicyConfigError::from)
    }

    /// Look up a profile by name, falling back to `default` if not found.
    pub fn profile(&self, name: &str) -> &PolicyProfile {
        self.policies
            .get(name)
            .unwrap_or(&self.default)
    }
}

/// Errors loading `policy.toml`.
#[derive(Debug)]
pub enum PolicyConfigError {
    /// File I/O error.
    Io(std::io::Error),
    /// TOML parse error.
    Parse(toml::de::Error),
}

impl std::fmt::Display for PolicyConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "policy.toml I/O error: {e}"),
            Self::Parse(e) => write!(f, "policy.toml parse error: {e}"),
        }
    }
}

impl std::error::Error for PolicyConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for PolicyConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<toml::de::Error> for PolicyConfigError {
    fn from(e: toml::de::Error) -> Self {
        Self::Parse(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRD_EXAMPLE: &str = r#"
[default]
name = "default"
description = "Default policy for unknown clients"
blocked_categories = ["ads", "tracker", "malware", "phishing"]
allowed_categories = []
custom_allowlist = []
custom_blocklist = []
upstream = "tiered"
log_level = "info"

[policies.parents]
name = "parents"
description = "Full access for parents"
blocked_categories = ["ads", "tracker", "malware", "phishing"]
allowed_categories = ["all"]
custom_allowlist = []
custom_blocklist = []
upstream = "tiered"
log_level = "info"

[policies.kids]
name = "kids"
description = "Filtered access for children"
blocked_categories = ["adult", "gambling", "social", "dating", "piracy", "ads", "tracker", "malware", "phishing"]
allowed_categories = []
custom_allowlist = ["khanacademy.org", "*.scratch.mit.edu"]
custom_blocklist = ["tiktok.com", "instagram.com", "snapchat.com"]
upstream = "tiered"
log_level = "warn"

[policies.iot]
name = "iot"
description = "Locked down for IoT devices"
blocked_categories = ["ads", "tracker", "telemetry", "social", "streaming", "games", "adult", "gambling", "dating"]
allowed_categories = []
custom_allowlist = ["*.vendor.com", "ota.vendor.com"]
custom_blocklist = []
upstream = "tiered"
log_level = "error"

[policies.guest]
name = "guest"
description = "Ad-blocking only for guest network"
blocked_categories = ["ads", "tracker", "malware", "phishing"]
allowed_categories = ["all"]
custom_allowlist = []
custom_blocklist = []
upstream = "tiered"
log_level = "info"

[clients."192.168.1.10"]
name = "dad-laptop"
policy = "parents"

[clients."192.168.1.11"]
name = "mom-phone"
policy = "parents"

[clients."192.168.1.20"]
name = "kids-tablet"
policy = "kids"

[clients."192.168.10.0/24"]
name = "guest-network"
policy = "guest"

[clients."192.168.20.0/24"]
name = "iot-network"
policy = "iot"

[dhcp_integration]
hostname_map = { "kids-tablet" = "kids", "dad-laptop" = "parents" }
"#;

    #[test]
    fn parse_prd_example() {
        let cfg = PolicyConfig::from_str(PRD_EXAMPLE).expect("parse PRD example");
        assert_eq!(cfg.default.name, "default");
        assert_eq!(cfg.default.blocked_categories.len(), 4);
        assert_eq!(cfg.policies.len(), 4);
        assert!(cfg.policies.contains_key("parents"));
        assert!(cfg.policies.contains_key("kids"));
        assert!(cfg.policies.contains_key("iot"));
        assert!(cfg.policies.contains_key("guest"));
    }

    #[test]
    fn parse_client_mappings() {
        let cfg = PolicyConfig::from_str(PRD_EXAMPLE).unwrap();
        assert_eq!(cfg.clients.len(), 5);
        let dad = cfg.clients.get("192.168.1.10").unwrap();
        assert_eq!(dad.policy, "parents");
        assert_eq!(dad.name, "dad-laptop");
        let guest = cfg.clients.get("192.168.10.0/24").unwrap();
        assert_eq!(guest.policy, "guest");
    }

    #[test]
    fn parse_dhcp_integration() {
        let cfg = PolicyConfig::from_str(PRD_EXAMPLE).unwrap();
        assert_eq!(
            cfg.dhcp_integration.hostname_map.get("kids-tablet"),
            Some(&"kids".to_string())
        );
        assert_eq!(
            cfg.dhcp_integration.hostname_map.get("dad-laptop"),
            Some(&"parents".to_string())
        );
    }

    #[test]
    fn profile_lookup_falls_back_to_default() {
        let cfg = PolicyConfig::from_str(PRD_EXAMPLE).unwrap();
        let p = cfg.profile("nonexistent");
        assert_eq!(p.name, "default");
        let kids = cfg.profile("kids");
        assert_eq!(kids.name, "kids");
        assert!(kids.custom_blocklist.contains(&"tiktok.com".to_string()));
    }

    #[test]
    fn parse_empty_config() {
        let cfg = PolicyConfig::from_str("").unwrap();
        assert_eq!(cfg.policies.len(), 0);
        assert_eq!(cfg.clients.len(), 0);
    }
}
