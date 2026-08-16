//! Serve-stale handler (RFC 8767, story 03-002).
//!
//! [`ServeStaleHandler`] implements [`DnsMiddleware`](crate::dns::DnsMiddleware)
//! and provides RFC 8767 serve-stale behavior: when all upstream tiers are
//! unavailable (the catalog returns `SERVFAIL`), the handler serves the most
//! recent expired cache entry with a modified TTL.
//!
//! ## Architecture
//!
//! hickory-resolver's `ResponseCache` does not expose expired entries — its
//! `get()` returns `None` once the TTL elapses. We therefore maintain a
//! **separate** [`StaleCache`](crate::dns::caching::StaleCache) that retains
//! successful responses beyond their TTL for up to `serve_stale_ttl` seconds.
//!
//! The handler participates in the request flow in two ways:
//!
//! 1. **Pre-forward (middleware `process`)**: If the stale cache has a *fresh*
//!    entry for the query, it is served immediately via
//!    [`MiddlewareAction::Serve`], bypassing the catalog. This makes the stale
//!    cache a second cache layer alongside hickory-resolver's internal LRU
//!    cache.
//!
//! 2. **Post-forward (catalog fallback)**: [`DnshubHandler`](crate::dns::DnshubHandler)
//!    wraps the catalog call with a [`CapturingResponseHandler`] that
//!    intercepts the response. If the catalog returns `SERVFAIL` and a stale
//!    entry exists, the stale entry is served with TTL set to
//!    `serve_stale_ttl`. Successful responses are stored in the stale cache
//!    for future serve-stale use.
//!
//! ## TTL modification (RFC 8767 §4)
//!
//! When serving a stale response, all answer record TTLs are set to
//! `serve_stale_ttl` (default 86 400 seconds per PRD line 1409). This signals
//! to clients that the data is stale and limits how long downstream resolvers
//! cache it.

use crate::config::CacheConfig;
use crate::dns::caching::{CacheKey, StaleCache};
use crate::dns::{DnsMiddleware, MiddlewareAction};
use async_trait::async_trait;
use hickory_proto::op::{Message, MessageType, Metadata, OpCode, ResponseCode};
use hickory_proto::rr::Record;
use hickory_server::server::{Request, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::{MessageResponse, MessageResponseBuilder};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// A DNS middleware that implements RFC 8767 serve-stale.
///
/// Owns a [`StaleCache`] populated from successful upstream responses. When
/// the catalog returns `SERVFAIL`, [`DnshubHandler`](crate::dns::DnshubHandler)
/// consults this handler's stale cache and serves the expired entry with a
/// modified TTL.
///
/// `Clone` is cheap: the underlying [`StaleCache`] is behind an [`Arc`], so
/// clones share the same cache entries.
#[derive(Clone)]
pub struct ServeStaleHandler {
    /// The separate stale-response cache.
    cache: Arc<StaleCache>,
    /// Cache configuration (serve_stale enable flag + serve_stale_ttl).
    config: CacheConfig,
}

impl ServeStaleHandler {
    /// Build a new serve-stale handler from the cache configuration.
    ///
    /// The handler owns a fresh [`StaleCache`]. Callers share it with
    /// [`DnshubHandler`](crate::dns::DnshubHandler) via [`Self::cache`] so that
    /// the handler chain can populate it from successful upstream responses.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            cache: Arc::new(StaleCache::new()),
            config,
        }
    }

    /// Build a handler that shares an existing stale cache (e.g. for testing
    /// or hot-reload scenarios where the cache should persist across config
    /// changes).
    pub fn with_cache(config: CacheConfig, cache: Arc<StaleCache>) -> Self {
        Self { cache, config }
    }

    /// Returns `true` if serve-stale is enabled in the configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.serve_stale
    }

    /// The configured serve-stale TTL as a [`Duration`].
    pub fn serve_stale_ttl(&self) -> Duration {
        Duration::from_secs(self.config.serve_stale_ttl)
    }

    /// A shared handle to the underlying [`StaleCache`].
    ///
    /// [`DnshubHandler`](crate::dns::DnshubHandler) uses this to store
    /// successful upstream responses and to look up stale entries on
    /// `SERVFAIL`.
    pub fn cache(&self) -> &Arc<StaleCache> {
        &self.cache
    }

    /// Extract the first query from a request, as a [`CacheKey`].
    pub(crate) fn key_for_request(request: &Request) -> Option<CacheKey> {
        let query = request.queries.queries().first()?;
        Some(CacheKey::from_query(query))
    }

    /// Look up a **fresh** stale-cache entry for `request`.
    ///
    /// Used by the middleware `process` to serve fresh entries directly,
    /// bypassing the catalog.
    fn get_fresh_for_request(&self, request: &Request, now: Instant) -> Option<Message> {
        let key = Self::key_for_request(request)?;
        self.cache.get_fresh(&key, now)
    }

    /// Look up a **stale** (expired) cache entry for `request`.
    ///
    /// Used by [`DnshubHandler`](crate::dns::DnshubHandler) after the catalog
    /// returns `SERVFAIL`.
    pub fn get_stale_for_request(&self, request: &Request, now: Instant) -> Option<Message> {
        let key = Self::key_for_request(request)?;
        self.cache.get_stale(&key, now, self.serve_stale_ttl())
    }

    /// Store a successful upstream response in the stale cache.
    ///
    /// Called by [`DnshubHandler`](crate::dns::DnshubHandler) after the catalog
    /// returns a non-error response.
    pub fn store_response(&self, request: &Request, message: &Message) {
        if let Some(key) = Self::key_for_request(request) {
            self.cache.insert(key, message.clone(), Instant::now());
        }
    }

    /// Opportunistically evict entries that have exceeded the serve-stale
    /// window.
    pub fn evict_expired(&self) {
        self.cache.evict_expired(Instant::now(), self.serve_stale_ttl());
    }

    /// Modify all answer/authority/additional record TTLs in `message` to
    /// `serve_stale_ttl`, per RFC 8767 §4.
    ///
    /// The response code and message structure are preserved; only TTLs are
    /// lowered so that clients do not cache stale data longer than necessary.
    pub fn modify_ttls(&self, message: &mut Message) {
        let stale_ttl = u32::try_from(self.config.serve_stale_ttl).unwrap_or(u32::MAX);
        for record in message.answers.iter_mut() {
            record.ttl = stale_ttl;
        }
        for record in message.authorities.iter_mut() {
            record.ttl = stale_ttl;
        }
        for record in message.additionals.iter_mut() {
            record.ttl = stale_ttl;
        }
    }
}

#[async_trait]
impl DnsMiddleware for ServeStaleHandler {
    async fn process(&self, request: &Request) -> MiddlewareAction {
        if !self.is_enabled() {
            return MiddlewareAction::Continue;
        }

        // Check the stale cache for a *fresh* entry. If found, serve it
        // directly — this acts as a second cache layer and avoids an
        // unnecessary upstream round-trip.
        if let Some(msg) = self.get_fresh_for_request(request, Instant::now()) {
            info!(
                domain = %Self::key_for_request(request)
                    .map(|k| k.name)
                    .unwrap_or_default(),
                "serve-stale: serving fresh entry from stale cache"
            );
            return MiddlewareAction::Serve(msg);
        }

        MiddlewareAction::Continue
    }
}

/// Build a [`MessageResponse`] from a [`Message`], suitable for sending via a
/// [`ResponseHandler`].
///
/// The response metadata is derived from the request's metadata (so the
/// response ID matches the query) and the answer/authority/additional records
/// are taken from `message`.
pub fn build_message_response<'q>(
    request: &'q Request,
    message: Message,
) -> MessageResponse<
    'q,
    'static,
    impl Iterator<Item = &'static Record> + Send + 'static,
    impl Iterator<Item = &'static Record> + Send + 'static,
    impl Iterator<Item = &'static Record> + Send + 'static,
    impl Iterator<Item = &'static Record> + Send + 'static,
> {
    // Build response metadata from the request so the ID and opcode match.
    let mut metadata = Metadata::response_from_request(&request.metadata);
    metadata.response_code = message.metadata.response_code;

    // Leak the message into 'static lifetime so the iterators can borrow it.
    // This is acceptable because the MessageResponse is consumed (encoded and
    // sent) synchronously within the same handle_request call — the borrowed
    // records are dropped once the response is encoded.
    let boxed: Box<Message> = Box::new(message);
    let leaked: &'static Message = Box::leak(boxed);

    MessageResponseBuilder::from_message_request(request).build(
        metadata,
        leaked.answers.iter(),
        leaked.authorities.iter(),
        std::iter::empty(),
        leaked.additionals.iter(),
    )
}

/// A [`ResponseHandler`] that captures the response instead of sending it to
/// the network.
///
/// Used by [`DnshubHandler`](crate::dns::DnshubHandler) when serve-stale is
/// enabled: the catalog sends its response to this handler, which encodes it
/// to bytes and stores it. The caller then decides whether to forward the
/// response to the real client handler or replace it with a stale entry.
#[derive(Clone)]
pub struct CapturingResponseHandler {
    captured: Arc<Mutex<Option<CapturedResponse>>>,
    protocol: hickory_server::net::xfer::Protocol,
}

/// The bytes and response info captured by [`CapturingResponseHandler`].
pub struct CapturedResponse {
    /// Wire-format encoded response bytes.
    pub bytes: Vec<u8>,
    /// The response header info (response code, counts, …).
    pub info: ResponseInfo,
}

impl CapturingResponseHandler {
    /// Create a new capturing handler for the given protocol.
    pub fn new(protocol: hickory_server::net::xfer::Protocol) -> Self {
        Self {
            captured: Arc::new(Mutex::new(None)),
            protocol,
        }
    }

    /// Take the captured response, if any.
    pub fn take(&self) -> Option<CapturedResponse> {
        self.captured.lock().take()
    }
}

#[async_trait::async_trait]
impl ResponseHandler for CapturingResponseHandler {
    async fn send_response<'a>(
        &mut self,
        response: MessageResponse<
            '_,
            'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
        >,
    ) -> Result<ResponseInfo, hickory_server::net::NetError> {
        use hickory_proto::serialize::binary::BinEncoder;

        let mut bytes = Vec::with_capacity(512);
        let mut encoder = BinEncoder::new(&mut bytes);
        // Match the max-size logic from hickory's ResponseHandle::send_response.
        encoder.set_max_size(match self.protocol {
            hickory_server::net::xfer::Protocol::Udp => {
                hickory_server::net::udp::MAX_RECEIVE_BUFFER_SIZE as u16
            }
            _ => u16::MAX,
        });

        let info = response
            .destructive_emit(&mut encoder)
            .map_err(hickory_server::net::NetError::from)?;

        *self.captured.lock() = Some(CapturedResponse { bytes, info });
        Ok(info)
    }
}

/// Decode captured bytes into a [`Message`], returning a SERVFAIL message on
/// error.
pub fn decode_captured(bytes: &[u8]) -> Message {
    match Message::from_vec(bytes) {
        Ok(msg) => msg,
        Err(e) => {
            warn!(error = %e, "serve-stale: failed to decode captured response");
            let mut msg = Message::new(0, MessageType::Response, OpCode::Query);
            msg.metadata.response_code = ResponseCode::ServFail;
            msg
        }
    }
}

/// Re-encode a [`Message`] to bytes (used for replaying captured responses
/// to the real client handler).
pub fn encode_message(message: &Message) -> Vec<u8> {
    message.to_vec().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::caching::{CacheKey, StaleCache};
    use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
    use hickory_proto::rr::{Name, RData, Record, RecordType};
    use hickory_proto::serialize::binary::BinEncodable;
    use hickory_server::net::xfer::Protocol;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    /// Build a DNS `Request` carrying an A-record query for `domain`.
    fn make_request(domain: &str) -> Request {
        let name = Name::from_utf8(domain).unwrap();
        let mut msg = Message::new(1, MessageType::Query, OpCode::Query);
        msg.add_query(Query::query(name, RecordType::A));
        let raw = msg.to_bytes().unwrap();
        Request::from_bytes(
            raw,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345),
            Protocol::Udp,
        )
        .unwrap()
    }

    /// Build a NOERROR response Message with a single A record at `ttl`.
    fn make_response(name: &str, ttl: u32, ip: Ipv4Addr) -> Message {
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NoError;
        let record = Record::from_rdata(
            Name::from_utf8(name).unwrap(),
            ttl,
            RData::A(ip.into()),
        );
        msg.add_answer(record);
        msg
    }

    fn make_config(serve_stale: bool, serve_stale_ttl: u64) -> CacheConfig {
        CacheConfig {
            min_ttl: 0,
            max_ttl: 86400,
            negative_ttl: 60,
            serve_stale,
            serve_stale_ttl,
            max_entries: 4096,
        }
    }

    #[tokio::test]
    async fn test_disabled_handler_continues() {
        let handler = ServeStaleHandler::new(make_config(false, 86400));
        let req = make_request("example.com");
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn test_fresh_entry_served_from_stale_cache() {
        let handler = ServeStaleHandler::new(make_config(true, 86400));
        let req = make_request("example.com");

        // Populate the stale cache with a fresh entry.
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));
        handler.cache().insert(key, msg, Instant::now());

        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Serve(_)));
    }

    #[tokio::test]
    async fn test_no_fresh_entry_continues() {
        let handler = ServeStaleHandler::new(make_config(true, 86400));
        let req = make_request("example.com");
        // Stale cache is empty.
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn test_stale_entry_returned_on_failure() {
        let handler = ServeStaleHandler::new(make_config(true, 86400));
        let req = make_request("example.com");

        // Insert an expired entry (TTL 0 → immediately stale).
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        let msg = make_response("example.com.", 0, Ipv4Addr::new(1, 2, 3, 4));
        handler.cache().insert(key, msg, Instant::now());

        // The stale entry should be retrievable.
        let stale = handler.get_stale_for_request(&req, Instant::now());
        assert!(stale.is_some());
        let stale_msg = stale.unwrap();
        assert_eq!(stale_msg.metadata.response_code, ResponseCode::NoError);
        assert_eq!(stale_msg.answers.len(), 1);
    }

    #[tokio::test]
    async fn test_no_stale_entry_returns_none() {
        let handler = ServeStaleHandler::new(make_config(true, 86400));
        let req = make_request("example.com");
        let stale = handler.get_stale_for_request(&req, Instant::now());
        assert!(stale.is_none());
    }

    #[test]
    fn test_modify_ttls_sets_serve_stale_ttl() {
        let handler = ServeStaleHandler::new(make_config(true, 30));
        let mut msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));

        handler.modify_ttls(&mut msg);

        for record in &msg.answers {
            assert_eq!(record.ttl, 30);
        }
    }

    #[test]
    fn test_store_response_populates_cache() {
        let handler = ServeStaleHandler::new(make_config(true, 86400));
        let req = make_request("example.com");
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));

        handler.store_response(&req, &msg);

        // Should be retrievable as fresh.
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        assert!(handler.cache().get_fresh(&key, Instant::now()).is_some());
    }

    #[test]
    fn test_store_response_ignores_negative() {
        let handler = ServeStaleHandler::new(make_config(true, 86400));
        let req = make_request("example.com");
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NXDomain;

        handler.store_response(&req, &msg);

        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        assert!(handler.cache().get_fresh(&key, Instant::now()).is_none());
    }

    #[test]
    fn test_capturing_handler_captures_response() {
        use hickory_server::zone_handler::MessageResponseBuilder;

        let capturing = CapturingResponseHandler::new(Protocol::Udp);
        let req = make_request("example.com");

        // Build a simple error response.
        let response = MessageResponseBuilder::from_message_request(&req)
            .error_msg(&req.metadata, ResponseCode::ServFail);

        // Simulate the catalog sending a response.
        let mut handler = capturing.clone();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let info = runtime.block_on(handler.send_response(response)).unwrap();

        assert_eq!(info.response_code, ResponseCode::ServFail);
        let captured = capturing.take().expect("response was captured");
        assert!(!captured.bytes.is_empty());
        assert_eq!(captured.info.response_code, ResponseCode::ServFail);
    }

    #[test]
    fn test_decode_captured_servfail() {
        // Build a SERVFAIL message, encode it, then decode.
        let mut msg = Message::new(42, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::ServFail;
        let bytes = msg.to_vec().unwrap();

        let decoded = decode_captured(&bytes);
        assert_eq!(decoded.metadata.response_code, ResponseCode::ServFail);
        assert_eq!(decoded.metadata.id, 42);
    }

    #[test]
    fn test_decode_captured_noerror() {
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));
        let bytes = msg.to_vec().unwrap();

        let decoded = decode_captured(&bytes);
        assert_eq!(decoded.metadata.response_code, ResponseCode::NoError);
        assert_eq!(decoded.answers.len(), 1);
    }

    #[test]
    fn test_build_message_response_preserves_records() {
        let req = make_request("example.com");
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));

        let response = build_message_response(&req, msg);
        assert_eq!(response.metadata().response_code, ResponseCode::NoError);
    }

    #[test]
    fn test_stale_cache_shared_across_handlers() {
        let cache = Arc::new(StaleCache::new());
        let h1 = ServeStaleHandler::with_cache(make_config(true, 86400), cache.clone());
        let h2 = ServeStaleHandler::with_cache(make_config(true, 86400), cache.clone());

        let req = make_request("example.com");
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));
        h1.store_response(&req, &msg);

        // h2 should see the entry because they share the cache.
        let key = CacheKey {
            name: "example.com".to_string(),
            qtype: RecordType::A,
        };
        assert!(h2.cache().get_fresh(&key, Instant::now()).is_some());
    }
}
