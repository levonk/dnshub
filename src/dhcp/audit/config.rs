//! Audit log configuration.
//!
//! [`AuditConfig`] controls whether lease lifecycle events are persisted to
//! the SQLite `dhcp_audit_log` table. It is embedded in the `[dhcp.audit]`
//! section of `dnshub.toml`.

use serde::{Deserialize, Serialize};

/// `[dhcp.audit]` — lease audit log configuration (story 04-008).
///
/// When `enabled` is `true`, the DHCP server records every lease lifecycle
/// event (ACK, RENEW, RELEASE, DECLINE, EXPIRE, CONFLICT) to the
/// `dhcp_audit_log` SQLite table via [`super::AuditLogger`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Whether audit logging is active. When `false`, the DHCP server skips
    /// writing events entirely (the table may still exist from a prior run).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        assert!(AuditConfig::default().enabled);
    }
}
