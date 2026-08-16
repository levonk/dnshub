//! DHCPDISCOVER probe construction and response parsing for rogue DHCP
//! server detection.
//!
//! The probe is a minimal RFC 2131 DHCPDISCOVER: a random transaction id,
//! the broadcast flag set, a zeroed client hardware address (we are not
//! actually requesting a lease), and a `ParameterRequestList` asking for the
//! common options a real server would return. The probe is sent as a UDP
//! broadcast to `255.255.255.255:67`; any DHCPOFFER received with a matching
//! transaction id is recorded as a [`ProbeResponse`].
//!
//! The wire-format work uses [`dhcproto`]; the network I/O is split into a
//! pure [`Probe::build`] / [`Probe::parse_offer`] pair (unit-testable without
//! sockets) and a [`Probe::send`] method that performs the actual UDP
//! broadcast. Tests exercise the pure path; the socket path is covered by the
//! [`RogueDetector`] integration tests where a loopback responder is stood up.

use dhcproto::{v4, Decoder, Decodable, Encodable};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Default source port for the probe. RFC 2131 says clients use an
/// ephemeral port; we pick a fixed high port so tests can bind a responder.
pub const PROBE_SOURCE_PORT: u16 = 6868;

/// The DHCP server port (RFC 2131).
pub const DHCP_SERVER_PORT: u16 = 67;

/// Monotonic counter mixed into generated transaction ids so two probes
/// built in the same nanosecond still get distinct xids.
static XID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a transaction id without depending on the `rand` crate (which is
/// not a direct dependency of dnshub). Combines the current unix nanos with a
/// per-process atomic counter — sufficient entropy for a probe xid, which
/// only needs to be unique within a single probe cycle on one LAN.
fn fresh_xid() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u32)
        .unwrap_or(0);
    let ctr = XID_COUNTER.fetch_add(1, Ordering::Relaxed);
    nanos.wrapping_add(ctr.wrapping_mul(0x9E37_79B9))
}

/// A constructed DHCPDISCOVER probe and its transaction id.
#[derive(Debug, Clone)]
pub struct Probe {
    /// The transaction id embedded in the DHCPDISCOVER. Responses must match
    /// this to be attributed to the probe.
    pub xid: u32,
    /// The encoded DHCPDISCOVER bytes ready to send.
    pub bytes: Vec<u8>,
}

/// A parsed DHCPOFFER response attributed to a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    /// The transaction id (matches the probe's `xid`).
    pub xid: u32,
    /// The IP address the responding server claims as its own (`siaddr`).
    pub server_ip: Ipv4Addr,
    /// The IP address offered to the (fake) client (`yiaddr`).
    pub offered_ip: Ipv4Addr,
}

impl Probe {
    /// Build a DHCPDISCOVER probe with a caller-supplied transaction id.
    ///
    /// The probe sets the broadcast flag, leaves `ciaddr`/`yiaddr`/`siaddr`
    /// unset, zeroes `chaddr`, and requests `SubnetMask`, `Router`,
    /// `DomainNameServer`, and `DomainName` via the parameter request list —
    /// the same shape a real client sends so a rogue server answers normally.
    pub fn build_with_xid(xid: u32) -> Result<Self, dhcproto::error::EncodeError> {
        let mut msg = v4::Message::default();
        msg.set_xid(xid)
            .set_flags(v4::Flags::default().set_broadcast())
            .set_chaddr(&[0u8; 6]);
        msg.opts_mut()
            .insert(v4::DhcpOption::MessageType(v4::MessageType::Discover));
        msg.opts_mut()
            .insert(v4::DhcpOption::ParameterRequestList(vec![
                v4::OptionCode::SubnetMask,
                v4::OptionCode::Router,
                v4::OptionCode::DomainNameServer,
                v4::OptionCode::DomainName,
            ]));

        let bytes = msg.to_vec()?;
        Ok(Self { xid, bytes })
    }

    /// Build a DHCPDISCOVER probe with a fresh transaction id.
    pub fn build() -> Result<Self, dhcproto::error::EncodeError> {
        Self::build_with_xid(fresh_xid())
    }

    /// Parse a UDP datagram as a DHCPOFFER, returning `Some(ProbeResponse)`
    /// only if it is a DHCPOFFER whose transaction id matches `self.xid`.
    /// Non-OFFER messages and mismatched xids return `None` (they are not
    /// attributed to this probe).
    pub fn parse_offer(&self, datagram: &[u8]) -> Option<ProbeResponse> {
        let msg = v4::Message::decode(&mut Decoder::new(datagram)).ok()?;
        if msg.xid() != self.xid {
            return None;
        }
        if !msg.opts().has_msg_type(v4::MessageType::Offer) {
            return None;
        }
        Some(ProbeResponse {
            xid: msg.xid(),
            server_ip: msg.siaddr(),
            offered_ip: msg.yiaddr(),
        })
    }

    /// Send the probe as a UDP broadcast to `255.255.255.255:67` from
    /// `src_port`, then wait up to `timeout` for DHCPOFFER responses. Each
    /// matching response is collected; the call returns when the timeout
    /// elapses or `max_responses` offers have been received.
    ///
    /// This is the live network path used by [`super::RogueDetector`]. It is
    /// intentionally async so the detector can run it on a tokio task.
    pub async fn send(
        &self,
        src_port: u16,
        timeout: Duration,
        max_responses: usize,
    ) -> std::io::Result<Vec<ProbeResponse>> {
        let bind_addr: SocketAddr =
            SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, src_port).into();
        let sock = UdpSocket::bind(bind_addr).await?;
        sock.set_broadcast(true)?;

        let broadcast_addr: SocketAddr =
            SocketAddrV4::new(Ipv4Addr::BROADCAST, DHCP_SERVER_PORT).into();
        sock.send_to(&self.bytes, broadcast_addr).await?;

        let mut responses = Vec::new();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        loop {
            if responses.len() >= max_responses {
                break;
            }
            // Reuse a single 4 KiB buffer (max DHCP message is 576 but offers
            // with options can be larger; 4 KiB is a safe upper bound).
            let mut buf = vec![0u8; 4096];
            tokio::select! {
                _ = &mut deadline => break,
                recv = sock.recv_from(&mut buf) => {
                    let (n, _peer) = match recv {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                    if let Some(resp) = self.parse_offer(&buf[..n]) {
                        responses.push(resp);
                    }
                }
            }
        }
        Ok(responses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dhcproto::v4::{DhcpOption, MessageType};

    fn build_offer(xid: u32, siaddr: Ipv4Addr, yiaddr: Ipv4Addr) -> Vec<u8> {
        let mut msg = v4::Message::default();
        msg.set_xid(xid)
            .set_opcode(v4::Opcode::BootReply)
            .set_chaddr(&[0u8; 6]);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Offer));
        // Manually set siaddr/yiaddr via the public setters.
        msg.set_siaddr(siaddr);
        msg.set_yiaddr(yiaddr);
        msg.to_vec().expect("encode offer")
    }

    #[test]
    fn probe_is_discover_with_broadcast_flag() {
        let probe = Probe::build_with_xid(0xdead_beef).unwrap();
        let msg =
            v4::Message::decode(&mut Decoder::new(&probe.bytes)).expect("decode probe");
        assert_eq!(msg.xid(), 0xdead_beef);
        assert!(msg.flags().broadcast());
        assert!(msg.opts().has_msg_type(MessageType::Discover));
        assert_eq!(probe.xid, 0xdead_beef);
    }

    #[test]
    fn parse_matching_offer() {
        let probe = Probe::build_with_xid(42).unwrap();
        let offer = build_offer(
            42,
            Ipv4Addr::new(192, 168, 1, 1),
            Ipv4Addr::new(192, 168, 1, 50),
        );
        let resp = probe.parse_offer(&offer).expect("offer matches probe");
        assert_eq!(resp.xid, 42);
        assert_eq!(resp.server_ip, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(resp.offered_ip, Ipv4Addr::new(192, 168, 1, 50));
    }

    #[test]
    fn parse_ignores_mismatched_xid() {
        let probe = Probe::build_with_xid(1).unwrap();
        let offer = build_offer(2, Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 2));
        assert!(probe.parse_offer(&offer).is_none());
    }

    #[test]
    fn parse_ignores_non_offer() {
        let probe = Probe::build_with_xid(7).unwrap();
        // Build an ACK with matching xid — should be ignored.
        let mut msg = v4::Message::default();
        msg.set_xid(7).set_opcode(v4::Opcode::BootReply);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Ack));
        let ack = msg.to_vec().unwrap();
        assert!(probe.parse_offer(&ack).is_none());
    }

    #[test]
    fn parse_ignores_garbage() {
        let probe = Probe::build_with_xid(7).unwrap();
        assert!(probe.parse_offer(&[0u8; 4]).is_none());
        assert!(probe.parse_offer(&[]).is_none());
    }
}
