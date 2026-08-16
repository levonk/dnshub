//! EDNS Client Subnet (ECS) strip handler (story 03-003).
//!
//! [`EcsStripHandler`] implements [`DnsMiddleware`](crate::dns::DnsMiddleware)
//! and removes the EDNS Client Subnet option (RFC 7871, option code 8) from
//! DNS queries before they are forwarded to upstream resolvers. This prevents
//! upstream resolvers from using the client's subnet for geo-targeting,
//! preserving client privacy.
//!
//! ## How it works
//!
//! The handler hooks into two extension points of [`DnsMiddleware`]:
//!
//! 1. [`process`](DnsMiddleware::process) — always returns
//!    [`MiddlewareAction::Continue`]; ECS stripping never blocks a query.
//! 2. [`rewrite`](DnsMiddleware::rewrite) — when `strip` is enabled, parses the
//!    raw request bytes into a [`Message`], removes the ECS option from the
//!    EDNS OPT record, re-encodes, and returns a new [`Request`] that the
//!    [`DnshubHandler`](crate::dns::DnshubHandler) forwards to the catalog
//!    instead of the original.
//!
//! If the request has no EDNS record or no ECS option, `rewrite` returns
//! `None` and the original request is forwarded unchanged.
//!
//! ## Response stripping
//!
//! [`EcsStripHandler::strip_ecs_from_message`] is a standalone function that
//! removes the ECS option from any [`Message`] (query or response). Full
//! response-path stripping requires intercepting the
//! [`ResponseHandler`](hickory_server::server::ResponseHandler) pipeline,
//! which is architecturally more involved in hickory-server 0.26 (the
//! `MessageResponse` type uses borrowed, non-mutable iterators). The
//! function is tested at the unit level and is ready for use when the
//! response interception layer is added.

use crate::dns::{DnsMiddleware, MiddlewareAction};
use async_trait::async_trait;
use hickory_proto::op::{Edns, Message};
use hickory_proto::rr::rdata::opt::EdnsCode;
use hickory_server::server::Request;
use tracing::debug;

/// EDNS Client Subnet option code (RFC 7871 §6.1.1).
///
/// This is the IANA-assigned code for the ECS option. We use the strongly
/// typed [`EdnsCode::Subnet`] variant rather than a raw `8` constant so the
/// compiler catches any future enum rename.
const ECS_CODE: EdnsCode = EdnsCode::Subnet;

/// A DNS middleware that strips the EDNS Client Subnet (ECS) option from
/// queries before forwarding to upstream.
///
/// Construct with [`EcsStripHandler::new`], passing `true` when
/// `config.ecs.strip` is enabled. Add to the
/// [`DnshubHandler`](crate::dns::DnshubHandler) middleware chain via
/// [`DnshubHandler::with_middleware`](crate::dns::DnshubHandler::with_middleware).
pub struct EcsStripHandler {
    /// Whether ECS stripping is active.
    strip: bool,
}

impl EcsStripHandler {
    /// Create a new `EcsStripHandler`.
    ///
    /// When `strip` is `true`, the ECS option is removed from forwarded
    /// queries. When `false`, the handler is a no-op pass-through.
    pub fn new(strip: bool) -> Self {
        Self { strip }
    }

    /// Returns `true` when ECS stripping is active.
    pub fn is_enabled(&self) -> bool {
        self.strip
    }

    /// Remove the ECS (EDNS Client Subnet, option code 8) option from a
    /// [`Message`]'s EDNS record, if present.
    ///
    /// This works on both query messages and response messages. Returns
    /// `true` if an ECS option was found and removed, `false` otherwise.
    ///
    /// If the EDNS record has no remaining options after ECS removal, the
    /// EDNS record itself is preserved (it may carry other metadata such as
    /// max payload size or DNSSEC OK flag).
    pub fn strip_ecs_from_message(msg: &mut Message) -> bool {
        let Some(edns) = msg.edns.as_mut() else {
            return false;
        };
        Self::strip_ecs_from_edns(edns)
    }

    /// Remove the ECS option from an [`Edns`] record in place.
    ///
    /// Returns `true` if an ECS option was found and removed.
    fn strip_ecs_from_edns(edns: &mut Edns) -> bool {
        let had_ecs = edns.option(ECS_CODE).is_some();
        if had_ecs {
            edns.options_mut().remove(ECS_CODE);
        }
        had_ecs
    }

    /// Create a new [`Request`] with the ECS option stripped from its EDNS
    /// record.
    ///
    /// Returns `None` when:
    /// - stripping is disabled (`self.strip == false`),
    /// - the request has no EDNS record, or
    /// - the EDNS record has no ECS option.
    ///
    /// Otherwise, parses the raw request bytes into a [`Message`], removes
    /// the ECS option, re-encodes to wire format, and constructs a new
    /// `Request` with the same source address and protocol.
    fn strip_ecs_from_request(&self, request: &Request) -> Option<Request> {
        if !self.strip {
            return None;
        }

        // Fast path: check whether the request carries an ECS option before
        // doing the expensive parse-reencode round-trip.
        let edns = request.edns.as_ref()?;
        if edns.option(ECS_CODE).is_none() {
            return None;
        }

        // Parse the raw wire bytes, strip ECS, and re-encode.
        let mut msg = Message::from_vec(request.as_slice()).ok()?;
        if !Self::strip_ecs_from_message(&mut msg) {
            return None;
        }

        let new_raw = msg.to_vec().ok()?;
        let new_request = Request::from_bytes(new_raw, request.src(), request.protocol()).ok()?;

        debug!("stripped ECS option from outgoing query");
        Some(new_request)
    }
}

#[async_trait]
impl DnsMiddleware for EcsStripHandler {
    async fn process(&self, _request: &Request) -> MiddlewareAction {
        // ECS stripping never blocks a query — it only rewrites the request
        // via `rewrite`. Returning Continue lets the chain proceed.
        MiddlewareAction::Continue
    }

    async fn rewrite(&self, request: &Request) -> Option<Request> {
        self.strip_ecs_from_request(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{Message, MessageType, OpCode};
    use hickory_proto::rr::rdata::opt::{EdnsOption, NSIDPayload};
    use hickory_proto::rr::{Name, RecordType};
    use hickory_server::net::xfer::Protocol;
    use hickory_server::server::Request;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    /// Build a DNS query `Message` for `domain` (A record) with an optional
    /// EDNS record containing the given options.
    fn build_message(domain: &str, edns_options: Vec<EdnsOption>) -> Message {
        let name = Name::from_utf8(domain).unwrap();
        let mut msg = Message::new(1, MessageType::Query, OpCode::Query);
        msg.add_query(hickory_proto::op::Query::query(name, RecordType::A));

        if !edns_options.is_empty() {
            let mut edns = Edns::new();
            for opt in edns_options {
                edns.options_mut().insert(opt);
            }
            msg.set_edns(edns);
        }

        msg
    }

    /// Build a DNS `Request` from a `Message` by encoding to wire format
    /// and decoding back via `Request::from_bytes`.
    fn request_from_message(msg: &Message) -> Request {
        let raw = msg.to_vec().unwrap();
        Request::from_bytes(
            raw,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 5353),
            Protocol::Udp,
        )
        .unwrap()
    }

    /// An ECS option carrying a test client subnet (192.168.1.0/24).
    fn ecs_option() -> EdnsOption {
        EdnsOption::Subnet(hickory_proto::rr::rdata::opt::ClientSubnet::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0)),
            24,
            0,
        ))
    }

    // --- strip_ecs_from_message tests (response + query level) ---

    #[test]
    fn strip_ecs_from_message_removes_ecs() {
        let mut msg = build_message("example.com", vec![ecs_option()]);
        assert!(EcsStripHandler::strip_ecs_from_message(&mut msg));
        // ECS should be gone.
        let edns = msg.edns.as_ref().unwrap();
        assert!(edns.option(ECS_CODE).is_none());
    }

    #[test]
    fn strip_ecs_from_message_preserves_other_options() {
        let mut msg = build_message(
            "example.com",
            vec![
                ecs_option(),
                EdnsOption::NSID(NSIDPayload::new(vec![0x41, 0x42]).unwrap()),
            ],
        );
        assert!(EcsStripHandler::strip_ecs_from_message(&mut msg));
        let edns = msg.edns.as_ref().unwrap();
        // ECS removed, NSID preserved.
        assert!(edns.option(ECS_CODE).is_none());
        assert!(edns.option(EdnsCode::NSID).is_some());
    }

    #[test]
    fn strip_ecs_from_message_no_edns_returns_false() {
        let mut msg = build_message("example.com", vec![]);
        assert!(!EcsStripHandler::strip_ecs_from_message(&mut msg));
        assert!(msg.edns.is_none());
    }

    #[test]
    fn strip_ecs_from_message_no_ecs_returns_false() {
        let mut msg = build_message(
            "example.com",
            vec![EdnsOption::NSID(NSIDPayload::new(vec![0x43]).unwrap())],
        );
        assert!(!EcsStripHandler::strip_ecs_from_message(&mut msg));
        // NSID should still be there.
        let edns = msg.edns.as_ref().unwrap();
        assert!(edns.option(EdnsCode::NSID).is_some());
    }

    #[test]
    fn strip_ecs_from_message_on_response() {
        // Simulate a response message with ECS in the EDNS record.
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        let mut edns = Edns::new();
        edns.options_mut().insert(ecs_option());
        msg.set_edns(edns);

        assert!(EcsStripHandler::strip_ecs_from_message(&mut msg));
        let edns = msg.edns.as_ref().unwrap();
        assert!(edns.option(ECS_CODE).is_none());
    }

    // --- EcsStripHandler::rewrite tests (query level) ---

    #[tokio::test]
    async fn rewrite_strips_ecs_from_query() {
        let handler = EcsStripHandler::new(true);
        let msg = build_message("example.com", vec![ecs_option()]);
        let req = request_from_message(&msg);

        // Verify the original request has ECS.
        assert!(req.edns.as_ref().unwrap().option(ECS_CODE).is_some());

        let new_req = handler.rewrite(&req).await;
        assert!(new_req.is_some(), "rewrite should return a new request");
        let new_req = new_req.unwrap();

        // The new request should not have ECS.
        assert!(
            new_req.edns.as_ref().unwrap().option(ECS_CODE).is_none(),
            "ECS should be stripped from rewritten request"
        );
    }

    #[tokio::test]
    async fn rewrite_passes_through_query_without_ecs() {
        let handler = EcsStripHandler::new(true);
        let msg = build_message("example.com", vec![]);
        let req = request_from_message(&msg);

        let result = handler.rewrite(&req).await;
        assert!(
            result.is_none(),
            "rewrite should return None when there is no ECS to strip"
        );
    }

    #[tokio::test]
    async fn rewrite_passes_through_query_with_edns_but_no_ecs() {
        let handler = EcsStripHandler::new(true);
        let msg = build_message(
            "example.com",
            vec![EdnsOption::NSID(NSIDPayload::new(vec![0x44]).unwrap())],
        );
        let req = request_from_message(&msg);

        let result = handler.rewrite(&req).await;
        assert!(
            result.is_none(),
            "rewrite should return None when EDNS has no ECS option"
        );
    }

    #[tokio::test]
    async fn rewrite_disabled_returns_none() {
        let handler = EcsStripHandler::new(false);
        let msg = build_message("example.com", vec![ecs_option()]);
        let req = request_from_message(&msg);

        let result = handler.rewrite(&req).await;
        assert!(
            result.is_none(),
            "rewrite should return None when stripping is disabled"
        );
    }

    #[tokio::test]
    async fn process_always_continues() {
        let handler = EcsStripHandler::new(true);
        let msg = build_message("example.com", vec![ecs_option()]);
        let req = request_from_message(&msg);

        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn process_continues_without_ecs() {
        let handler = EcsStripHandler::new(true);
        let msg = build_message("example.com", vec![]);
        let req = request_from_message(&msg);

        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Continue));
    }

    #[test]
    fn is_enabled_reflects_config() {
        assert!(EcsStripHandler::new(true).is_enabled());
        assert!(!EcsStripHandler::new(false).is_enabled());
    }
}
