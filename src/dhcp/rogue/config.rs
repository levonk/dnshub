//! Rogue DHCP server detection configuration.
//!
//! [`RogueConfig`] mirrors the `[dhcp.rogue_detection]` section of
//! `dnshub.toml` (PRD lines 772-777). When enabled, the [`super::RogueDetector`]
//! periodically broadcasts DHCPDISCOVER probes and alerts if a DHCPOFFER
//! arrives from a server that is not dnshub itself.

use serde::{Deserialize, Serialize};

/// `[dhcp.rogue_detection]` — rogue DHCP server detection (story 04-008).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RogueConfig {
    /// Whether periodic rogue-DHCP probing is active.
    #[serde(default)]
    pub enabled: bool,
    /// Seconds between probe cycles. Default 300 (5 minutes) per PRD line 775.
    #[serde(default = "default_probe_interval_secs")]
    pub probe_interval_secs: u64,
    /// Prometheus metric name incremented on a rogue detection. Default
    /// `dnshub_rogue_dhcp_detected` per PRD line 776.
    #[serde(default = "default_alert_metric")]
    pub alert_metric: String,
    /// Whether to emit a structured log line on detection (PRD line 777).
    #[serde(default = "default_alert_log")]
    pub alert_log: bool,
    /// Maximum time in milliseconds to wait for DHCPOFFER responses per probe
    /// cycle. Default 2000ms — generous enough for a single broadcast round
    /// trip on a LAN without delaying the probe loop.
    #[serde(default = "default_probe_timeout_ms")]
    pub probe_timeout_ms: u64,
}

impl Default for RogueConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            probe_interval_secs: default_probe_interval_secs(),
            alert_metric: default_alert_metric(),
            alert_log: default_alert_log(),
            probe_timeout_ms: default_probe_timeout_ms(),
        }
    }
}

fn default_probe_interval_secs() -> u64 {
    300
}
fn default_alert_metric() -> String {
    "dnshub_rogue_dhcp_detected".to_string()
}
fn default_alert_log() -> bool {
    true
}
fn default_probe_timeout_ms() -> u64 {
    2000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_prd() {
        let cfg = RogueConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.probe_interval_secs, 300);
        assert_eq!(cfg.alert_metric, "dnshub_rogue_dhcp_detected");
        assert!(cfg.alert_log);
        assert_eq!(cfg.probe_timeout_ms, 2000);
    }
}
