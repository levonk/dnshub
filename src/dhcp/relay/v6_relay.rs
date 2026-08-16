//! DHCPv6 relay (Relay-Forw / Relay-Rel) handling.
//!
//! DHCPv6 relay agents encapsulate client messages in Relay-Forw
//! messages (RFC 8415 section 9). A Relay-Forw carries a `link-address`
//! (the address the relay agent used to reach the server, identifying the
//! client's link / VLAN), a `peer-address` (the client's address), and an
//! `Interface-Id` option, plus a nested `RelayMsg` option containing the
//! encapsulated client message (which may itself be another Relay-Forw
//! for nested relays).
//!
//! dnshub uses the `link-address` to select the appropriate IPv6 pool,
//! unwraps the nested client message, and builds a Relay-Rel to wrap the
//! server's reply on the way back. Up to 8 nested relay layers are
//! handled per RFC 8415.
//!
//! The nesting walk is implemented against the raw datagram bytes rather
//! than relying on dhcproto's option decoder for the innermost `RelayMsg`
//! option: per RFC 8415 the innermost encapsulated payload is a regular
//! client message (not a relay message), and dhcproto's `RelayMsg` variant
//! only decodes nested `RelayMessage`s. Parsing the TLV chain manually
//! lets us extract the innermost client message type reliably.

use std::net::Ipv6Addr;

use dhcproto::Encoder;
use dhcproto::v6::{MessageType, RelayMessage};

use ipnet::Ipv6Net;

/// Maximum number of nested DHCPv6 relay messages handled (RFC 8415).
pub const MAX_RELAY_DEPTH: u8 = 8;

/// DHCPv6 RelayMsg option code (RFC 8415 section 21.10).
const OPT_RELAY_MSG: u16 = 9;

/// A DHCPv6 pool entry used by the v6 relay handler for link-address
/// based selection.
#[derive(Debug, Clone)]
pub struct V6RelayPool {
    /// Human-readable pool name, e.g. `"vlan10-v6-guest"`.
    pub name: String,
    /// The IPv6 subnet (prefix) for this pool. A relayed request whose
    /// `link-address` falls within this subnet is served from this pool.
    pub subnet: Ipv6Net,
    /// Default policy profile for clients on this link.
    pub default_profile: String,
}

/// The relay handler's decision for a relayed DHCPv6 Relay-Forw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V6RelayDecision {
    /// The link-address from the outermost Relay-Forw.
    pub link_addr: Ipv6Addr,
    /// The peer-address from the outermost Relay-Forw.
    pub peer_addr: Ipv6Addr,
    /// The hop count of the outermost Relay-Forw (nesting depth).
    pub hop_count: u8,
    /// The selected pool name (matched by link-address subnet), if any.
    pub pool: Option<String>,
    /// The selected policy profile (pool default, or `"default"`).
    pub profile: String,
    /// The innermost encapsulated message type (the actual client
    /// message, e.g. Solicit / Request).
    pub inner_msg_type: MessageType,
}

/// Outcome of evaluating a DHCPv6 relay message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V6RelayOutcome {
    /// The message is a Relay-Forw and was accepted.
    Accepted(V6RelayDecision),
    /// The message is a Relay-Rel (server-bound reply) — not expected by
    /// the server handler.
    RelayRepl { hop_count: u8 },
    /// The message is not a relay message (regular client message).
    NotRelayed,
    /// The relay nesting exceeds [`MAX_RELAY_DEPTH`].
    TooDeeplyNested,
    /// The raw bytes could not be decoded as a relay message.
    DecodeError,
}

/// DHCPv6 relay handler.
#[derive(Debug, Clone, Default)]
pub struct V6RelayHandler {
    pools: Vec<V6RelayPool>,
}

impl V6RelayHandler {
    /// Build a [`V6RelayHandler`] with the given v6 pools.
    pub fn new(pools: Vec<V6RelayPool>) -> Self {
        Self { pools }
    }

    /// Returns the configured v6 pools.
    pub fn pools(&self) -> &[V6RelayPool] {
        &self.pools
    }

    /// Evaluate a raw DHCPv6 datagram.
    ///
    /// Peeks at the first byte to decide whether this is a Relay-Forw
    /// (12) / Relay-Rel (13) or a regular client message, then parses
    /// the relay header and walks the nested `RelayMsg` options to find
    /// the innermost client message and selects a pool based on the
    /// outermost `link-address`.
    pub fn evaluate(&self, datagram: &[u8]) -> V6RelayOutcome {
        if datagram.is_empty() {
            return V6RelayOutcome::DecodeError;
        }
        let msg_type = MessageType::from(datagram[0]);
        match msg_type {
            MessageType::RelayForw => self.handle_relay_forw(datagram),
            MessageType::RelayRepl => match parse_relay_header(datagram) {
                Some((_msg_type, hop, _link, _peer)) => V6RelayOutcome::RelayRepl { hop_count: hop },
                None => V6RelayOutcome::DecodeError,
            },
            _ => V6RelayOutcome::NotRelayed,
        }
    }

    fn handle_relay_forw(&self, datagram: &[u8]) -> V6RelayOutcome {
        let (_, hop_count, link_addr, peer_addr) = match parse_relay_header(datagram) {
            Some(h) => h,
            None => return V6RelayOutcome::DecodeError,
        };
        if hop_count >= MAX_RELAY_DEPTH {
            return V6RelayOutcome::TooDeeplyNested;
        }

        // Walk the nested RelayMsg options to find the innermost message
        // type. Start from the outer relay's options section.
        let opts_start = 1 + 1 + 16 + 16;
        let inner_msg_type = match innermost_msg_type(&datagram[opts_start..]) {
            Some(t) => t,
            None => return V6RelayOutcome::DecodeError,
        };

        let pool = self.select_pool(link_addr);
        let profile = pool
            .as_ref()
            .and_then(|name| {
                self.pools
                    .iter()
                    .find(|p| &p.name == name)
                    .map(|p| p.default_profile.clone())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "default".to_string());

        tracing::info!(
            link_addr = %link_addr,
            peer_addr = %peer_addr,
            hop_count = hop_count,
            inner_msg_type = ?inner_msg_type,
            pool = ?pool,
            profile = %profile,
            "accepted DHCPv6 Relay-Forw"
        );

        V6RelayOutcome::Accepted(V6RelayDecision {
            link_addr,
            peer_addr,
            hop_count,
            pool,
            profile,
            inner_msg_type,
        })
    }

    /// Select a pool name based on `link-address`. The first pool whose
    /// subnet contains the link-address wins.
    pub fn select_pool(&self, link_addr: Ipv6Addr) -> Option<String> {
        self.pools
            .iter()
            .find(|p| p.subnet.contains(&link_addr))
            .map(|p| p.name.clone())
    }

    /// Build a Relay-Rel wrapping `original` (the Relay-Forw we received),
    /// with the server's reply message encoded as the nested `RelayMsg`
    /// option.
    ///
    /// The caller supplies `reply_bytes` — the raw bytes of the server's
    /// reply (either a regular client message such as a Reply, or a
    /// nested Relay-Rel for multi-hop relays). This helper sets the
    /// Relay-Rel's `link-address`, `peer-address`, and hop-count from the
    /// original Relay-Forw and inserts the reply as the `RelayMsg` option.
    ///
    /// The encoding is performed manually because dhcproto's
    /// `RelayMessage` exposes no constructor for setting link/peer/hop
    /// and its `RelayMsg` option variant only accepts decoded
    /// `RelayMessage`s (not plain client messages). The manual encoding
    /// matches the RFC 8415 wire format exactly:
    /// ```text
    /// msg_type(1) | hop_count(1) | link_addr(16) | peer_addr(16)
    ///   | option-code(2) | option-len(2) | <reply_bytes>
    /// ```
    pub fn build_relay_repl(
        &self,
        original: &RelayMessage,
        reply_bytes: &[u8],
    ) -> Result<Vec<u8>, dhcproto::error::EncodeError> {
        let mut buf = Vec::with_capacity(1 + 1 + 16 + 16 + 2 + 2 + reply_bytes.len());
        let mut enc = Encoder::new(&mut buf);
        enc.write_u8(u8::from(MessageType::RelayRepl))?;
        enc.write_u8(original.hop_count())?;
        enc.write_slice(&original.link_addr().octets())?;
        enc.write_slice(&original.peer_addr().octets())?;
        enc.write_u16(OPT_RELAY_MSG)?;
        enc.write_u16(reply_bytes.len() as u16)?;
        enc.write_slice(reply_bytes)?;
        Ok(buf)
    }
}

/// Parse the fixed-size header of a DHCPv6 relay message from `buf`.
///
/// Returns `(msg_type, hop_count, link_addr, peer_addr)` or `None` if the
/// buffer is too short for the 34-byte relay header.
fn parse_relay_header(buf: &[u8]) -> Option<(MessageType, u8, Ipv6Addr, Ipv6Addr)> {
    if buf.len() < 1 + 1 + 16 + 16 {
        return None;
    }
    let msg_type = MessageType::from(buf[0]);
    let hop_count = buf[1];
    let mut link = [0u8; 16];
    link.copy_from_slice(&buf[2..18]);
    let mut peer = [0u8; 16];
    peer.copy_from_slice(&buf[18..34]);
    Some((msg_type, hop_count, Ipv6Addr::from(link), Ipv6Addr::from(peer)))
}

/// Walk the nested `RelayMsg` options starting from an options section
/// (`buf` points at the first option TLV) to find the innermost message
/// type.
///
/// At each relay layer, the options section is a TLV sequence
/// `{code: u16, len: u16, data: [u8; len]}*`. We locate the `RelayMsg`
/// option (code 9) and recurse into its data. If the data's first byte is
/// a Relay-Forw/Relay-Rel, it is another relay layer; otherwise it is the
/// innermost client message and its first byte is the message type.
/// Recursion is bounded by [`MAX_RELAY_DEPTH`].
fn innermost_msg_type(opts_buf: &[u8]) -> Option<MessageType> {
    find_innermost_payload(opts_buf, 0)
}

/// Recursive helper for [`innermost_msg_type`]. `depth` is the number of
/// relay layers unwrapped so far.
fn find_innermost_payload(opts_buf: &[u8], depth: u8) -> Option<MessageType> {
    if depth >= MAX_RELAY_DEPTH {
        return None;
    }
    let payload = find_relay_msg(opts_buf)?;
    if payload.is_empty() {
        return None;
    }
    let inner_type = MessageType::from(payload[0]);
    if inner_type == MessageType::RelayForw || inner_type == MessageType::RelayRepl {
        // Nested relay layer: skip its 34-byte header and continue
        // walking its options section.
        if payload.len() < 1 + 1 + 16 + 16 {
            return None;
        }
        find_innermost_payload(&payload[1 + 1 + 16 + 16..], depth + 1)
    } else {
        Some(inner_type)
    }
}

/// Scan an options TLV sequence for the first `RelayMsg` option (code 9)
/// and return a slice over its payload bytes.
fn find_relay_msg(opts_buf: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    while pos + 4 <= opts_buf.len() {
        let code = u16::from_be_bytes([opts_buf[pos], opts_buf[pos + 1]]);
        let len = u16::from_be_bytes([opts_buf[pos + 2], opts_buf[pos + 3]]) as usize;
        let data_start = pos + 4;
        let data_end = data_start + len;
        if data_end > opts_buf.len() {
            return None;
        }
        if code == OPT_RELAY_MSG {
            return Some(&opts_buf[data_start..data_end]);
        }
        pos = data_end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dhcproto::{Decodable, Decoder};
    use dhcproto::v6::OptionCode;

    fn link_addr() -> Ipv6Addr {
        "fd00:10::1".parse().unwrap()
    }

    fn pools() -> Vec<V6RelayPool> {
        vec![V6RelayPool {
            name: "vlan10-v6".to_string(),
            subnet: "fd00:10::/64".parse().unwrap(),
            default_profile: "guest".to_string(),
        }]
    }

    /// Build a minimal Relay-Forw datagram with the given link/peer
    /// addresses and an empty options section.
    fn relay_forw_bytes(link: Ipv6Addr, peer: Ipv6Addr, hop: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.write_u8(u8::from(MessageType::RelayForw)).unwrap();
        enc.write_u8(hop).unwrap();
        enc.write_slice(&link.octets()).unwrap();
        enc.write_slice(&peer.octets()).unwrap();
        buf
    }

    /// Build a Relay-Forw whose RelayMsg option carries a Solicit (msg
    /// type 1) with a 3-byte transaction id.
    fn relay_forw_with_solicit(link: Ipv6Addr, peer: Ipv6Addr) -> Vec<u8> {
        // inner solicit: msg_type=1, xid=[0xAA,0xBB,0xCC], no opts
        let mut inner = Vec::new();
        let mut ie = Encoder::new(&mut inner);
        ie.write_u8(1).unwrap();
        ie.write::<3>([0xAA, 0xBB, 0xCC]).unwrap();

        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.write_u8(u8::from(MessageType::RelayForw)).unwrap();
        enc.write_u8(0).unwrap();
        enc.write_slice(&link.octets()).unwrap();
        enc.write_slice(&peer.octets()).unwrap();
        enc.write_u16(OPT_RELAY_MSG).unwrap();
        enc.write_u16(inner.len() as u16).unwrap();
        enc.write_slice(&inner).unwrap();
        buf
    }

    /// Build a doubly-nested Relay-Forw: outer wraps an inner Relay-Forw
    /// which wraps a Solicit.
    fn nested_relay_forw(link: Ipv6Addr, peer: Ipv6Addr) -> Vec<u8> {
        // innermost solicit
        let mut solicit = Vec::new();
        let mut se = Encoder::new(&mut solicit);
        se.write_u8(1).unwrap();
        se.write::<3>([1, 2, 3]).unwrap();

        // inner Relay-Forw wrapping the solicit
        let mut inner_forw = Vec::new();
        let mut ie = Encoder::new(&mut inner_forw);
        ie.write_u8(u8::from(MessageType::RelayForw)).unwrap();
        ie.write_u8(1).unwrap();
        ie.write_slice(&"fd00:20::1".parse::<Ipv6Addr>().unwrap().octets())
            .unwrap();
        ie.write_slice(&"fe80::2".parse::<Ipv6Addr>().unwrap().octets())
            .unwrap();
        ie.write_u16(OPT_RELAY_MSG).unwrap();
        ie.write_u16(solicit.len() as u16).unwrap();
        ie.write_slice(&solicit).unwrap();

        // outer Relay-Forw wrapping the inner Relay-Forw
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.write_u8(u8::from(MessageType::RelayForw)).unwrap();
        enc.write_u8(0).unwrap();
        enc.write_slice(&link.octets()).unwrap();
        enc.write_slice(&peer.octets()).unwrap();
        enc.write_u16(OPT_RELAY_MSG).unwrap();
        enc.write_u16(inner_forw.len() as u16).unwrap();
        enc.write_slice(&inner_forw).unwrap();
        buf
    }

    #[test]
    fn not_relayed_for_regular_message() {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.write_u8(1).unwrap();
        enc.write::<3>([0, 0, 0]).unwrap();
        let h = V6RelayHandler::new(pools());
        assert_eq!(h.evaluate(&buf), V6RelayOutcome::NotRelayed);
    }

    #[test]
    fn relay_forw_accepted_and_pool_selected() {
        let h = V6RelayHandler::new(pools());
        let bytes = relay_forw_with_solicit(link_addr(), "fe80::1".parse().unwrap());
        match h.evaluate(&bytes) {
            V6RelayOutcome::Accepted(dec) => {
                assert_eq!(dec.link_addr, link_addr());
                assert_eq!(dec.pool.as_deref(), Some("vlan10-v6"));
                assert_eq!(dec.profile, "guest");
                assert_eq!(dec.hop_count, 0);
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn relay_forw_unknown_link_falls_back_to_default_profile() {
        let h = V6RelayHandler::new(pools());
        let unknown_link: Ipv6Addr = "fd00:99::1".parse().unwrap();
        let bytes = relay_forw_with_solicit(unknown_link, "fe80::1".parse().unwrap());
        match h.evaluate(&bytes) {
            V6RelayOutcome::Accepted(dec) => {
                assert_eq!(dec.link_addr, unknown_link);
                assert!(dec.pool.is_none());
                assert_eq!(dec.profile, "default");
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn relay_repl_reported() {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.write_u8(u8::from(MessageType::RelayRepl)).unwrap();
        enc.write_u8(2).unwrap();
        enc.write_slice(&link_addr().octets()).unwrap();
        enc.write_slice(&[0u8; 16]).unwrap();
        let h = V6RelayHandler::new(pools());
        match h.evaluate(&buf) {
            V6RelayOutcome::RelayRepl { hop_count } => assert_eq!(hop_count, 2),
            other => panic!("expected RelayRepl, got {other:?}"),
        }
    }

    #[test]
    fn empty_datagram_is_decode_error() {
        let h = V6RelayHandler::new(pools());
        assert_eq!(h.evaluate(&[]), V6RelayOutcome::DecodeError);
    }

    #[test]
    fn relay_forw_with_solicit_extracts_inner_msg_type() {
        let h = V6RelayHandler::new(pools());
        let bytes = relay_forw_with_solicit(link_addr(), "fe80::1".parse().unwrap());
        match h.evaluate(&bytes) {
            V6RelayOutcome::Accepted(dec) => {
                assert_eq!(dec.inner_msg_type, MessageType::Solicit);
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn nested_relay_forw_extracts_innermost_msg_type() {
        let h = V6RelayHandler::new(pools());
        let bytes = nested_relay_forw(link_addr(), "fe80::1".parse().unwrap());
        match h.evaluate(&bytes) {
            V6RelayOutcome::Accepted(dec) => {
                assert_eq!(dec.inner_msg_type, MessageType::Solicit);
                assert_eq!(dec.hop_count, 0);
            }
            other => panic!("expected Accepted, got {other:?}"),
        }
    }

    #[test]
    fn build_relay_repl_roundtrips_header() {
        let h = V6RelayHandler::new(pools());
        let forw_bytes = relay_forw_bytes(link_addr(), "fe80::1".parse().unwrap(), 1);
        let forw = RelayMessage::decode(&mut Decoder::new(&forw_bytes)).unwrap();
        // reply is a plain Reply (msg type 7) with xid
        let mut reply = Vec::new();
        let mut re = Encoder::new(&mut reply);
        re.write_u8(7).unwrap();
        re.write::<3>([0xAA, 0xBB, 0xCC]).unwrap();
        let out = h.build_relay_repl(&forw, &reply).unwrap();
        // decode the header back
        let repl = RelayMessage::decode(&mut Decoder::new(&out)).unwrap();
        assert_eq!(repl.msg_type(), MessageType::RelayRepl);
        assert_eq!(repl.hop_count(), 1);
        assert_eq!(repl.link_addr(), link_addr());
        assert_eq!(repl.peer_addr(), "fe80::1".parse::<Ipv6Addr>().unwrap());
    }

    #[test]
    fn select_pool_by_subnet_containment() {
        let h = V6RelayHandler::new(pools());
        assert_eq!(
            h.select_pool("fd00:10::abcd".parse().unwrap()).as_deref(),
            Some("vlan10-v6")
        );
        assert!(h
            .select_pool("fd00:99::1".parse().unwrap())
            .is_none());
    }

    #[test]
    fn option_code_relay_msg_matches_constant() {
        assert_eq!(u16::from(OptionCode::RelayMsg), OPT_RELAY_MSG);
    }
}
