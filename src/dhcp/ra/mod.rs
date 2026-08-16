//! Router Advertisement (RA) sender for SLAAC.
//!
//! The [`RaSender`] transmits ICMPv6 Router Advertisement messages on a
//! configured network interface. It operates in two modes:
//!
//! 1. **Periodic** — sends an RA every `router_lifetime_secs / 3`
//!    seconds (per RFC 4861 §6.2.4, the interval should be between
//!    3 and 4 times less than the router lifetime to ensure at least
//!    3 RAs per lifetime).
//! 2. **Solicited** — responds to Router Solicitation (ICMPv6 type 133)
//!    messages with an immediate RA, subject to rate limiting.
//!
//! RA messages are sent to the all-nodes multicast address `ff02::1`.
//!
//! # Permissions
//!
//! Sending raw ICMPv6 messages requires `CAP_NET_RAW` (root) or
//! appropriate socket permissions. In Docker macvlan deployments this
//! is provided by the container's networking namespace.
//!
//! # RFC compliance
//!
//! - RFC 4861: Neighbor Discovery for IP Version 6 (IPv6)
//! - RFC 8106: IPv6 Router Advertisement Options for DNS Configuration

pub mod config;
pub mod message;

pub use config::RaConfig;
pub use message::{
    build_router_advertisement, ALL_NODES_MULTICAST, ICMPV6_RA_TYPE, ICMPV6_RS_TYPE,
    RA_FLAG_MANAGED, RA_FLAG_OTHER,
};

use std::net::Ipv6Addr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Default cur hop limit advertised in RAs (64, common for Linux).
const DEFAULT_CUR_HOP_LIMIT: u8 = 64;

/// Minimum delay between solicited RAs in milliseconds (RFC 4861 §6.2.6
/// suggests at least 3 seconds between unsolicited RAs; we apply a
/// shorter floor for solicited responses).
const SOLICITED_RA_MIN_INTERVAL_MS: u64 = 500;

/// RA sender for a single interface.
///
/// Construct with [`RaSender::new`] from an [`RaConfig`], then call
/// [`RaSender::run`] to start the periodic transmission loop. The
/// loop runs until the provided [`CancellationToken`] is cancelled.
pub struct RaSender {
    config: RaConfig,
    interface: String,
    prefix: Option<(Ipv6Addr, u8, u32, u32)>,
    rdnss: Vec<Ipv6Addr>,
}

impl RaSender {
    /// Create a new RA sender for `interface` using `config`.
    ///
    /// Parses the prefix and RDNSS entries from the config. Invalid
    /// entries are logged and skipped.
    pub fn new(interface: String, config: RaConfig) -> Self {
        let prefix = config.parse_prefix().map(|(addr, len)| {
            (
                addr,
                len,
                config.preferred_lifetime_secs,
                config.valid_lifetime_secs,
            )
        });
        if config.enabled && prefix.is_none() && !config.prefix.is_empty() {
            tracing::warn!(
                interface = %interface,
                prefix = %config.prefix,
                "RA prefix failed to parse; RA will not include Prefix Information option"
            );
        }

        let rdnss = config.parse_rdnss();
        if config.enabled && rdnss.len() < config.rdnss.len() {
            tracing::warn!(
                interface = %interface,
                configured = config.rdnss.len(),
                parsed = rdnss.len(),
                "some RDNSS entries failed to parse and were skipped"
            );
        }

        Self {
            config,
            interface,
            prefix,
            rdnss,
        }
    }

    /// The configured interface name.
    pub fn interface(&self) -> &str {
        &self.interface
    }

    /// Whether this sender is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Compute the periodic RA interval (RFC 4861 §6.2.4: at most
    /// router_lifetime / 3, with a minimum of 4 seconds).
    pub fn interval(&self) -> Duration {
        let lifetime = self.config.router_lifetime_secs;
        if lifetime == 0 {
            // If not advertising as a router, use a sensible default.
            return Duration::from_secs(600);
        }
        let secs = (lifetime / 3).max(4) as u64;
        Duration::from_secs(secs)
    }

    /// Build the RA message bytes for this sender's configuration.
    pub fn build_message(&self) -> Vec<u8> {
        build_router_advertisement(
            DEFAULT_CUR_HOP_LIMIT,
            0, // no M/O flags (SLAAC only)
            self.config.router_lifetime_secs.try_into().unwrap_or(0),
            0, // reachable time unspecified
            0, // retrans timer unspecified
            self.prefix,
            self.config.router_lifetime_secs.max(600), // RDNSS lifetime
            &self.rdnss,
            self.config.router_lifetime_secs.max(600), // DNSSL lifetime
            &self.config.dnssl,
        )
    }

    /// Run the periodic RA transmission loop.
    ///
    /// Sends an initial RA immediately, then repeats every
    /// [`RaSender::interval`] seconds until `cancel` is triggered.
    ///
    /// This method does not open a raw socket itself — it calls
    /// [`RaSender::send_ra`] which logs the transmission. Actual
    /// socket wiring is handled by the DHCP server orchestration
    /// (story 04-001) which calls [`RaSender::build_message`] and
    /// sends it via a raw ICMPv6 socket.
    pub async fn run(&self, cancel: CancellationToken) {
        if !self.config.enabled {
            tracing::info!(
                interface = %self.interface,
                "RA sender disabled, not starting"
            );
            return;
        }

        let interval = self.interval();
        tracing::info!(
            interface = %self.interface,
            interval_secs = interval.as_secs(),
            prefix = %self.config.prefix,
            router_lifetime_secs = self.config.router_lifetime_secs,
            "starting periodic RA sender"
        );

        loop {
            self.send_ra();
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = cancel.cancelled() => {
                    tracing::info!(
                        interface = %self.interface,
                        "RA sender stopped"
                    );
                    return;
                }
            }
        }
    }

    /// Transmit a single RA message.
    ///
    /// Logs the transmission event. The actual socket send is
    /// performed by the caller when wiring this into the DHCP server.
    pub fn send_ra(&self) {
        let msg = self.build_message();
        tracing::info!(
            interface = %self.interface,
            msg_len = msg.len(),
            rdnss_count = self.rdnss.len(),
            dnssl_count = self.config.dnssl.len(),
            "transmitting Router Advertisement to ff02::1"
        );
    }

    /// Handle an incoming Router Solicitation (ICMPv6 type 133).
    ///
    /// In a full implementation this would send an immediate RA in
    /// response, subject to rate limiting (min interval between
    /// solicited RAs). Here we log the reception and return the RA
    /// bytes that should be sent in response.
    pub fn handle_router_solicitation(&self) -> Vec<u8> {
        tracing::info!(
            interface = %self.interface,
            "received Router Solicitation, responding with immediate RA"
        );
        self.build_message()
    }

    /// The minimum interval between solicited RAs.
    pub fn solicited_min_interval(&self) -> Duration {
        Duration::from_millis(SOLICITED_RA_MIN_INTERVAL_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RaConfig {
        RaConfig {
            enabled: true,
            prefix: "fd00:1234:5678::/64".to_string(),
            preferred_lifetime_secs: 3600,
            valid_lifetime_secs: 7200,
            router_lifetime_secs: 1800,
            rdnss: vec!["fd00:1234:5678::67".to_string()],
            dnssl: vec!["levonk.com".to_string()],
        }
    }

    #[test]
    fn sender_builds_message_with_all_options() {
        let sender = RaSender::new("eth0".to_string(), test_config());
        let msg = sender.build_message();
        // 16 (header) + 32 (prefix) + 24 (rdnss) + 26 (dnssl)
        // DNSSL "levonk.com": domains=12, payload=18, units=3, total=2+24=26
        assert_eq!(msg.len(), 16 + 32 + 24 + 26);
        assert_eq!(msg[0], ICMPV6_RA_TYPE);
    }

    #[test]
    fn interval_is_one_third_of_router_lifetime() {
        let sender = RaSender::new("eth0".to_string(), test_config());
        // 1800 / 3 = 600
        assert_eq!(sender.interval(), Duration::from_secs(600));
    }

    #[test]
    fn interval_minimum_four_seconds() {
        let mut cfg = test_config();
        cfg.router_lifetime_secs = 6; // 6/3 = 2, clamped to 4
        let sender = RaSender::new("eth0".to_string(), cfg);
        assert_eq!(sender.interval(), Duration::from_secs(4));
    }

    #[test]
    fn interval_default_when_lifetime_zero() {
        let mut cfg = test_config();
        cfg.router_lifetime_secs = 0;
        let sender = RaSender::new("eth0".to_string(), cfg);
        assert_eq!(sender.interval(), Duration::from_secs(600));
    }

    #[test]
    fn disabled_sender_reports_disabled() {
        let mut cfg = test_config();
        cfg.enabled = false;
        let sender = RaSender::new("eth0".to_string(), cfg);
        assert!(!sender.is_enabled());
    }

    #[test]
    fn invalid_prefix_still_builds_message() {
        let mut cfg = test_config();
        cfg.prefix = "not-valid".to_string();
        let sender = RaSender::new("eth0".to_string(), cfg);
        let msg = sender.build_message();
        // No prefix option, just header + rdnss + dnssl
        // 16 + 24 (rdnss) + 26 (dnssl "levonk.com")
        assert_eq!(msg.len(), 16 + 24 + 26);
    }

    #[test]
    fn handle_rs_returns_ra_message() {
        let sender = RaSender::new("eth0".to_string(), test_config());
        let msg = sender.handle_router_solicitation();
        assert_eq!(msg[0], ICMPV6_RA_TYPE);
    }

    #[test]
    fn sender_interface_accessor() {
        let sender = RaSender::new("wlan0".to_string(), test_config());
        assert_eq!(sender.interface(), "wlan0");
    }

    #[test]
    fn solicited_min_interval_is_500ms() {
        let sender = RaSender::new("eth0".to_string(), test_config());
        assert_eq!(
            sender.solicited_min_interval(),
            Duration::from_millis(500)
        );
    }

    #[tokio::test]
    async fn run_disabled_sender_returns_immediately() {
        let mut cfg = test_config();
        cfg.enabled = false;
        let sender = RaSender::new("eth0".to_string(), cfg);
        let cancel = CancellationToken::new();
        // Should return immediately without hanging.
        sender.run(cancel).await;
    }

    #[tokio::test]
    async fn run_stops_on_cancellation() {
        let sender = RaSender::new("eth0".to_string(), test_config());
        let cancel = CancellationToken::new();
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel2.cancel();
        });
        sender.run(cancel).await;
        // If we reach here, cancellation worked.
    }
}
