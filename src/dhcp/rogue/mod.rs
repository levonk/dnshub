//! Rogue DHCP server detection.
//!
//! [`RogueDetector`] periodically broadcasts a DHCPDISCOVER probe (via
//! [`probe::Probe`]) and compares every DHCPOFFER responder against the set of
//! IP addresses dnshub itself is known to serve DHCP from. Any responder not
//! in that allowlist is treated as a rogue server: the configured Prometheus
//! counter is incremented and (optionally) a structured log line is emitted.
//!
//! The detector is split from the probe logic so the comparison can be unit
//! tested without sockets — see [`RogueDetector::classify_responses`] and the
//! `detect_non_dnshub_response` / `no_false_positive_from_own_ip` tests.
//!
//! The periodic loop ([`RogueDetector::run`]) is a tokio task that the DHCP
//! server spawns at startup; it honours a [`tokio_util::sync::CancellationToken`]
//! for graceful shutdown.

pub mod config;
pub mod probe;

pub use config::RogueConfig;
pub use probe::{Probe, ProbeResponse, PROBE_SOURCE_PORT};

use crate::dhcp::audit::DhcpAuditEvent;
use crate::dhcp::audit::DhcpAuditEventType;
use crate::dhcp::audit::AuditLogger;
use metrics::counter;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A responder that the detector classified as rogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RogueServer {
    /// The IP the rogue server claimed as its own (`siaddr`).
    pub server_ip: Ipv4Addr,
    /// The IP it offered to the probe (`yiaddr`).
    pub offered_ip: Ipv4Addr,
}

/// Rogue DHCP server detector.
///
/// Construct with [`RogueDetector::new`] passing the resolved dnshub server
/// IPs (the addresses dnshub's own DHCP listener binds) and a [`RogueConfig`].
/// Call [`RogueDetector::run`] inside a `tokio::spawn` to start the periodic
/// probe loop; cancel via the supplied [`CancellationToken`].
pub struct RogueDetector {
    config: RogueConfig,
    /// IP addresses dnshub itself serves DHCP from. Responses from any of
    /// these are legitimate; anything else is rogue.
    own_ips: Vec<Ipv4Addr>,
    /// Optional audit log — when present, rogue detections are recorded as
    /// `Conflict` events for frontend visibility.
    audit: Option<std::sync::Arc<AuditLogger>>,
}

impl RogueDetector {
    /// Create a new detector. `own_ips` is the set of IPs dnshub's DHCP
    /// listener is bound to; offers from any other IP are flagged rogue.
    pub fn new(config: RogueConfig, own_ips: Vec<Ipv4Addr>) -> Self {
        Self {
            config,
            own_ips,
            audit: None,
        }
    }

    /// Attach an audit logger so rogue detections are persisted as
    /// `Conflict` events (PRD observability: audit log IS the device
    /// join/leave record; a rogue server is a security-relevant event).
    pub fn with_audit(mut self, audit: std::sync::Arc<AuditLogger>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Classify a batch of probe responses, returning the subset that came
    /// from servers not in `own_ips`. This is the pure decision function and
    /// is unit-tested independently of any socket.
    pub fn classify_responses(&self, responses: &[ProbeResponse]) -> Vec<RogueServer> {
        responses
            .iter()
            .filter(|r| !self.own_ips.contains(&r.server_ip))
            .map(|r| RogueServer {
                server_ip: r.server_ip,
                offered_ip: r.offered_ip,
            })
            .collect()
    }

    /// Handle a set of probe responses: classify rogues, increment the
    /// configured metric, optionally log, and optionally persist an audit
    /// `Conflict` event. Returns the rogues found (for callers/tests).
    pub fn handle_responses(
        &self,
        responses: &[ProbeResponse],
        probe_timestamp_ms: i64,
    ) -> Vec<RogueServer> {
        let rogues = self.classify_responses(responses);
        if rogues.is_empty() {
            return Vec::new();
        }

        for rogue in &rogues {
            // Increment the configured Prometheus counter. The metric name is
            // configurable so deployments can rename it; the default is
            // `dnshub_rogue_dhcp_detected` (PRD line 776). We pass an owned
            // `String` (not `&'static str`) because the name comes from config;
            // the `metrics` facade accepts `String` via `KeyName: From<T>`.
            let metric_name = self.config.alert_metric.clone();
            counter!(metric_name).increment(1);

            if self.config.alert_log {
                tracing::warn!(
                    server_ip = %rogue.server_ip,
                    offered_ip = %rogue.offered_ip,
                    "rogue DHCP server detected: DHCPOFFER from non-dnshub server"
                );
            }

            if let Some(audit) = &self.audit {
                let event = DhcpAuditEvent::new(
                    probe_timestamp_ms,
                    DhcpAuditEventType::Conflict,
                )
                .with_ip(rogue.offered_ip.to_string())
                .with_details(format!(
                    "rogue DHCP server detected at {}",
                    rogue.server_ip
                ));
                // Best-effort: a failed audit write must not crash the probe
                // loop. The error is logged but not propagated.
                if let Err(e) = audit.write_audit_event(&event) {
                    tracing::error!(error = %e, "failed to write rogue-detection audit event");
                }
            }
        }

        rogues
    }

    /// Run the periodic probe loop until `cancel` is cancelled.
    ///
    /// Each cycle builds a fresh probe (new xid), broadcasts it, waits up to
    /// `probe_timeout_ms` for offers, classifies the responses, and sleeps
    /// `probe_interval_secs` before the next cycle. The first probe runs
    /// immediately on startup.
    pub async fn run(&self, cancel: CancellationToken) {
        if !self.config.enabled {
            return;
        }
        let interval = Duration::from_secs(self.config.probe_interval_secs);
        let timeout = Duration::from_millis(self.config.probe_timeout_ms);

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let rogues = self.probe_once(timeout).await;
            if !rogues.is_empty() {
                tracing::warn!(
                    count = rogues.len(),
                    "rogue DHCP servers detected in this probe cycle"
                );
            }

            // Sleep until the next cycle, but wake early on cancellation.
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
            }
        }
    }

    /// Perform a single probe cycle: build + send a probe and handle the
    /// responses. Exposed for tests that want to drive one cycle without the
    /// full periodic loop.
    pub async fn probe_once(&self, timeout: Duration) -> Vec<RogueServer> {
        let probe = match Probe::build() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "failed to build DHCPDISCOVER probe");
                return Vec::new();
            }
        };

        let responses = match probe
            .send(PROBE_SOURCE_PORT, timeout, 16)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "rogue DHCP probe send failed");
                return Vec::new();
            }
        };

        let now_ms = chrono_like_unix_millis();
        self.handle_responses(&responses, now_ms)
    }
}

/// Return the current time as unix milliseconds without pulling in chrono
/// (kept out of Cargo.toml per story constraints). Uses
/// `std::time::SystemTime` since the Unix epoch.
fn chrono_like_unix_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        // System clock before epoch — treat as zero rather than panicking.
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn detector(own: Vec<Ipv4Addr>) -> RogueDetector {
        RogueDetector::new(
            RogueConfig {
                enabled: true,
                probe_interval_secs: 1,
                alert_metric: "dnshub_rogue_dhcp_detected".to_string(),
                alert_log: false, // keep test output quiet
                probe_timeout_ms: 50,
            },
            own,
        )
    }

    fn resp(server: Ipv4Addr, offered: Ipv4Addr) -> ProbeResponse {
        ProbeResponse {
            xid: 1,
            server_ip: server,
            offered_ip: offered,
        }
    }

    #[test]
    fn detect_non_dnshub_response() {
        let det = detector(vec![Ipv4Addr::new(192, 168, 1, 1)]);
        let responses = vec![
            resp(Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 50)),
            resp(Ipv4Addr::new(10, 0, 0, 99), Ipv4Addr::new(10, 0, 0, 5)),
        ];
        let rogues = det.classify_responses(&responses);
        assert_eq!(rogues.len(), 1);
        assert_eq!(rogues[0].server_ip, Ipv4Addr::new(10, 0, 0, 99));
        assert_eq!(rogues[0].offered_ip, Ipv4Addr::new(10, 0, 0, 5));
    }

    #[test]
    fn no_false_positive_from_own_ip() {
        let det = detector(vec![
            Ipv4Addr::new(192, 168, 1, 1),
            Ipv4Addr::new(192, 168, 1, 2),
        ]);
        let responses = vec![
            resp(Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 50)),
            resp(Ipv4Addr::new(192, 168, 1, 2), Ipv4Addr::new(192, 168, 1, 51)),
        ];
        assert!(det.classify_responses(&responses).is_empty());
    }

    #[test]
    fn no_rogues_when_no_responses() {
        let det = detector(vec![Ipv4Addr::new(192, 168, 1, 1)]);
        assert!(det.classify_responses(&[]).is_empty());
    }

    #[test]
    fn handle_responses_records_audit_conflict() {
        use crate::dhcp::audit::AuditFilter;
        let audit = std::sync::Arc::new(
            AuditLogger::open_in_memory().expect("open in-memory audit log"),
        );
        let det = detector(vec![Ipv4Addr::new(192, 168, 1, 1)]).with_audit(audit.clone());

        let responses = vec![resp(Ipv4Addr::new(10, 0, 0, 99), Ipv4Addr::new(10, 0, 0, 5))];
        let rogues = det.handle_responses(&responses, 1_234_567);
        assert_eq!(rogues.len(), 1);

        let events = audit
            .list_audit_events(&AuditFilter {
                event_type: Some(DhcpAuditEventType::Conflict),
                ..Default::default()
            })
            .expect("list audit events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, DhcpAuditEventType::Conflict);
        assert_eq!(events[0].timestamp, 1_234_567);
        assert_eq!(events[0].ip_address.as_deref(), Some("10.0.0.5"));
        assert!(events[0]
            .details
            .as_deref()
            .unwrap()
            .contains("10.0.0.99"));
    }

    #[test]
    fn handle_responses_no_rogues_writes_nothing() {
        use crate::dhcp::audit::AuditFilter;
        let audit = std::sync::Arc::new(
            AuditLogger::open_in_memory().expect("open in-memory audit log"),
        );
        let det = detector(vec![Ipv4Addr::new(192, 168, 1, 1)]).with_audit(audit.clone());

        let responses = vec![resp(Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 50))];
        let rogues = det.handle_responses(&responses, 1);
        assert!(rogues.is_empty());
        assert!(audit
            .list_audit_events(&AuditFilter::default())
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn run_disabled_does_nothing() {
        let det = RogueDetector::new(
            RogueConfig {
                enabled: false,
                ..Default::default()
            },
            vec![Ipv4Addr::new(192, 168, 1, 1)],
        );
        // Should return immediately without probing.
        let cancel = CancellationToken::new();
        tokio::time::timeout(
            Duration::from_millis(100),
            det.run(cancel.clone()),
        )
        .await
        .expect("disabled run returns immediately");
    }
}
