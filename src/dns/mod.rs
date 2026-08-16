//! DNS server, RequestHandler chain, forwarding, and caching.
//!
//! ## Architecture (hickory-server 0.26)
//!
//! In hickory-server 0.26 the old `Authority` type was renamed to
//! `ZoneHandler`, and `RequestHandler` is no longer dyn-compatible (it uses a
//! native async fn returning `Pin<Box<dyn Future>>`). The native composition
//! model is therefore:
//!
//! 1. [`Catalog`](hickory_server::zone_handler::Catalog) implements
//!    [`RequestHandler`] and holds a set of [`ZoneHandler`]s
//!    (`ForwardZoneHandler`, `BlocklistZoneHandler`, `InMemoryZoneHandler`, …).
//! 2. [`DnshubHandler`] wraps the `Catalog` and adds a `Vec<Box<dyn
//!    DnsMiddleware>>` *pre-forward* chain for custom middleware (rate
//!    limiting, per-client policy, ECS stripping — future stories). It
//!    implements `RequestHandler` by running the middleware, then delegating
//!    to the `Catalog`.
//!
//! This keeps the handler chain extensible (new middleware can be added
//! without modifying existing handlers) while using the supported 0.26 API.
//!
//! ## Middleware chain (story 02-001)
//!
//! [`PolicyHandler`](crate::policy::PolicyHandler) implements [`DnsMiddleware`]
//! and is inserted into the chain via [`DnshubHandler::with_middleware_front`]
//! so it runs before forwarding. It resolves the client IP to a policy profile
//! via [`ClientResolver`](crate::client_resolver::ClientResolver), evaluates
//! the queried domain, and returns [`MiddlewareAction::Reject`] for blocked
//! queries or [`MiddlewareAction::Continue`] to let the request proceed.

pub mod caching;
pub mod ecs_strip;
pub mod forwarding;
pub mod serve_stale;
pub mod tiered_forward;
pub mod server;

use async_trait::async_trait;
use hickory_net::runtime::Time;
use hickory_proto::op::{Message, ResponseCode};
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::{Catalog, MessageResponseBuilder};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// The action returned by a [`DnsMiddleware`] handler.
#[derive(Debug, Clone)]
pub enum MiddlewareAction {
    /// Continue down the chain (no opinion on this request).
    Continue,
    /// Short-circuit the chain and return this response code to the client.
    Reject(ResponseCode),
    /// Serve a pre-built response message (e.g. from a cache), bypassing the
    /// catalog entirely. Used by [`ServeStaleHandler`](serve_stale::ServeStaleHandler)
    /// to serve fresh entries from the stale cache without an upstream
    /// round-trip.
    Serve(Message),
}

/// A dyn-compatible middleware trait that makes up the dnshub handler chain.
///
/// Each middleware inspects an incoming [`Request`] *before* it is forwarded.
/// Returning [`MiddlewareAction::Reject`] short-circuits the chain and sends an
/// empty response with the given code (e.g. blocklist returns `NXDOMAIN`,
/// rate limiter returns `Refused`). Returning [`MiddlewareAction::Continue`]
/// passes the request to the next handler, and ultimately to the [`Catalog`]
/// which performs the actual forwarding/zone lookup.
///
/// This is the extensibility point for stories 02-001 (PolicyHandler),
/// 03-003 (EcsStripHandler), 03-004 (RateLimitHandler), etc.
#[async_trait]
pub trait DnsMiddleware: Send + Sync + 'static {
    /// Inspect `request` before forwarding.
    async fn process(&self, _request: &Request) -> MiddlewareAction {
        MiddlewareAction::Continue
    }

    /// Optionally provide a replacement [`Request`] with modifications applied
    /// (e.g. EDNS Client Subnet stripping).
    ///
    /// Returns `None` by default — the original request is forwarded
    /// unchanged. When a middleware returns `Some(request)`, the
    /// [`DnshubHandler`] substitutes the new request for all subsequent
    /// middleware rewrites and for the final catalog delegation.
    async fn rewrite(&self, _request: &Request) -> Option<Request> {
        None
    }
}

/// The top-level dnshub request handler.
///
/// Wraps a hickory [`Catalog`] (which performs zone dispatch / forwarding) and
/// runs a chain of [`DnsMiddleware`] before delegating to it. Implements
/// hickory-server's [`RequestHandler`] so it can be handed to
/// [`Server::new`](hickory_server::server::Server::new).
///
/// When a [`ServeStaleHandler`](serve_stale::ServeStaleHandler) is attached via
/// [`Self::with_serve_stale`], the handler also implements RFC 8767
/// serve-stale: successful upstream responses are captured into the stale
/// cache, and `SERVFAIL` responses are replaced with stale entries (with
/// modified TTLs) when available.
pub struct DnshubHandler {
    catalog: Catalog,
    middleware: Vec<Box<dyn DnsMiddleware>>,
    /// Optional serve-stale handler. When present and enabled, the catalog's
    /// response is intercepted via a [`CapturingResponseHandler`] so that
    /// successful responses populate the stale cache and `SERVFAIL` responses
    /// trigger a stale-cache lookup.
    serve_stale: Option<Arc<serve_stale::ServeStaleHandler>>,
}

impl DnshubHandler {
    /// Build a handler from an already-populated catalog and an (possibly empty)
    /// middleware chain.
    pub fn new(catalog: Catalog, middleware: Vec<Box<dyn DnsMiddleware>>) -> Self {
        Self {
            catalog,
            middleware,
            serve_stale: None,
        }
    }

    /// Build a handler with no middleware (forwarding only).
    pub fn from_catalog(catalog: Catalog) -> Self {
        Self::new(catalog, Vec::new())
    }

    /// Append a middleware to the end of the chain.
    pub fn with_middleware(mut self, mw: Box<dyn DnsMiddleware>) -> Self {
        self.middleware.push(mw);
        self
    }

    /// Prepend a middleware to the front of the chain.
    ///
    /// Policy middleware (story 02-001) should be prepended so it runs
    /// before other middleware (rate limiting, ECS stripping) and can
    /// short-circuit blocked queries early.
    pub fn with_middleware_front(mut self, mw: Box<dyn DnsMiddleware>) -> Self {
        self.middleware.insert(0, mw);
        self
    }

    /// Conditionally add an [`EcsStripHandler`](ecs_strip::EcsStripHandler) to
    /// the middleware chain.
    ///
    /// When `strip` is `true`, an `EcsStripHandler` is appended to the end of
    /// the chain so it runs after policy / rate-limit middleware. When `false`,
    /// this is a no-op — the handler is not added.
    pub fn with_ecs_strip(self, strip: bool) -> Self {
        if strip {
            self.with_middleware(Box::new(ecs_strip::EcsStripHandler::new(true)))
        } else {
            self
        }
    }

    /// Attach a serve-stale handler (RFC 8767).
    ///
    /// When attached, the handler intercepts catalog responses: successful
    /// responses are stored in the stale cache, and `SERVFAIL` responses are
    /// replaced with stale entries (TTL modified to `serve_stale_ttl`) when
    /// available. The [`ServeStaleHandler`](serve_stale::ServeStaleHandler) is
    /// also added to the middleware chain so that fresh stale-cache entries
    /// are served without an upstream round-trip.
    pub fn with_serve_stale(mut self, handler: serve_stale::ServeStaleHandler) -> Self {
        // Add the handler to the middleware chain so its `process` method
        // (which serves fresh stale-cache entries) runs before forwarding.
        // The clone shares the same underlying StaleCache via Arc.
        let ss = Arc::new(handler.clone());
        self.middleware.push(Box::new(handler));
        self.serve_stale = Some(ss);
        self
    }
}

impl RequestHandler for DnshubHandler {
    fn handle_request<'life0, 'life1, 'async_trait, R, T>(
        &'life0 self,
        request: &'life1 Request,
        response_handle: R,
    ) -> Pin<Box<dyn Future<Output = ResponseInfo> + Send + 'async_trait>>
    where
        R: 'async_trait + ResponseHandler,
        T: 'async_trait + Time,
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        Box::pin(async move {
            // Run the pre-forward middleware chain.
            for mw in &self.middleware {
                match mw.process(request).await {
                    MiddlewareAction::Continue => {
                        debug!(middleware = mw.label(), "middleware: continue");
                    }
                    MiddlewareAction::Reject(code) => {
                        warn!(middleware = mw.label(), code = ?code, "middleware: reject");
                        let response = MessageResponseBuilder::from_message_request(request)
                            .error_msg(&request.metadata, code);
                        // Clone the response handle so the original remains
                        // available for the catalog path if this send fails.
                        let mut rh = response_handle.clone();
                        if let Ok(info) = rh.send_response(response).await {
                            return info;
                        }
                        // If sending the rejection failed, fall through to the
                        // catalog so the client gets *some* response.
                        break;
                    }
                    MiddlewareAction::Serve(message) => {
                        debug!(middleware = mw.label(), "middleware: serve cached");
                        let response = serve_stale::build_message_response(request, message);
                        let mut rh = response_handle.clone();
                        if let Ok(info) = rh.send_response(response).await {
                            return info;
                        }
                        // If sending failed, fall through to the catalog.
                        break;
                    }
                }
            }

            // Apply request rewrites (e.g. ECS stripping). Each middleware
            // that returns `Some(request)` replaces the current request for
            // subsequent rewrites and for the final catalog delegation.
            let mut owned_request: Option<Request> = None;
            for mw in &self.middleware {
                let current: &Request = owned_request.as_ref().unwrap_or(request);
                if let Some(new_req) = mw.rewrite(current).await {
                    debug!(middleware = mw.label(), "middleware: rewrote request");
                    owned_request = Some(new_req);
                }
            }
            let final_request: &Request = owned_request.as_ref().unwrap_or(request);

            // If serve-stale is enabled, intercept the catalog's response so
            // that successful responses populate the stale cache and SERVFAIL
            // responses trigger a stale-cache lookup (RFC 8767).
            if let Some(ss) = &self.serve_stale {
                if ss.is_enabled() {
                    return self
                        .handle_request_with_serve_stale::<R, T>(final_request, response_handle, ss)
                        .await;
                }
            }

            // Delegate to the catalog (zone dispatch / forwarding).
            self.catalog.handle_request::<R, T>(final_request, response_handle).await
        })
    }
}

impl DnshubHandler {
    /// Handle a request with serve-stale interception.
    ///
    /// The catalog's response is captured (not sent to the client) via a
    /// [`CapturingResponseHandler`](serve_stale::CapturingResponseHandler).
    /// Then:
    ///
    /// - If the response is **successful** (NOERROR with answers): store it in
    ///   the stale cache and forward it to the client.
    /// - If the response is **SERVFAIL** and a stale entry exists: serve the
    ///   stale entry with TTL modified to `serve_stale_ttl` (RFC 8767 §4).
    /// - If the response is **SERVFAIL** with no stale entry: forward the
    ///   SERVFAIL to the client.
    /// - Otherwise (NXDOMAIN, NODATA, etc.): forward the response as-is.
    async fn handle_request_with_serve_stale<R, T>(
        &self,
        request: &Request,
        response_handle: R,
        ss: &serve_stale::ServeStaleHandler,
    ) -> ResponseInfo
    where
        R: ResponseHandler,
        T: Time,
    {
        let protocol = request.protocol();
        let capturing = serve_stale::CapturingResponseHandler::new(protocol);

        // The catalog sends its response to the capturing handler (which
        // encodes it to bytes but does not send to the network).
        let catalog_info = self
            .catalog
            .handle_request::<serve_stale::CapturingResponseHandler, T>(request, capturing.clone())
            .await;

        let Some(captured) = capturing.take() else {
            // No response was captured (e.g. encoding error). Fall back to
            // a SERVFAIL, trying stale first.
            warn!("serve-stale: no response captured from catalog");
            return self
                .serve_stale_or_fail(request, response_handle, ss, catalog_info)
                .await;
        };

        let message = serve_stale::decode_captured(&captured.bytes);

        // Successful responses populate the stale cache.
        if message.metadata.response_code == hickory_proto::op::ResponseCode::NoError
            && !message.answers.is_empty()
        {
            ss.store_response(request, &message);
            // Opportunistic eviction.
            ss.evict_expired();
        }

        // SERVFAIL → try serve-stale.
        if captured.info.response_code == hickory_proto::op::ResponseCode::ServFail {
            return self
                .serve_stale_or_replay(request, response_handle, ss, message, captured.info)
                .await;
        }

        // Non-SERVFAIL: replay the captured response to the real client.
        self.replay_response(request, response_handle, message, captured.info)
            .await
    }

    /// On SERVFAIL, attempt to serve a stale entry. If none is available,
    /// replay the original SERVFAIL.
    async fn serve_stale_or_replay<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: R,
        ss: &serve_stale::ServeStaleHandler,
        original: Message,
        original_info: ResponseInfo,
    ) -> ResponseInfo {
        if let Some(mut stale_msg) = ss.get_stale_for_request(request, std::time::Instant::now()) {
            info!(
                domain = %serve_stale::ServeStaleHandler::key_for_request(request)
                    .map(|k| k.name)
                    .unwrap_or_default(),
                stale_ttl = ss.serve_stale_ttl().as_secs(),
                "serve-stale: serving stale entry on upstream failure"
            );
            // Modify TTLs per RFC 8767 §4.
            ss.modify_ttls(&mut stale_msg);
            let response = serve_stale::build_message_response(request, stale_msg);
            let mut rh = response_handle.clone();
            if let Ok(info) = rh.send_response(response).await {
                return info;
            }
        }

        // No stale entry (or send failed): replay the original SERVFAIL.
        self.replay_response(request, response_handle, original, original_info)
            .await
    }

    /// Fallback when no response was captured at all: try stale, else SERVFAIL.
    async fn serve_stale_or_fail<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: R,
        ss: &serve_stale::ServeStaleHandler,
        _catalog_info: ResponseInfo,
    ) -> ResponseInfo {
        if let Some(mut stale_msg) = ss.get_stale_for_request(request, std::time::Instant::now()) {
            info!(
                domain = %serve_stale::ServeStaleHandler::key_for_request(request)
                    .map(|k| k.name)
                    .unwrap_or_default(),
                "serve-stale: serving stale entry (no captured response)"
            );
            ss.modify_ttls(&mut stale_msg);
            let response = serve_stale::build_message_response(request, stale_msg);
            let mut rh = response_handle.clone();
            if let Ok(info) = rh.send_response(response).await {
                return info;
            }
        }

        // No stale entry: send SERVFAIL.
        let response = MessageResponseBuilder::from_message_request(request)
            .error_msg(&request.metadata, hickory_proto::op::ResponseCode::ServFail);
        let mut rh = response_handle.clone();
        match rh.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                warn!(error = %e, "serve-stale: failed to send SERVFAIL fallback");
                // Construct a minimal SERVFAIL ResponseInfo from the response
                // metadata. We cannot use the private ResponseInfo::serve_failed,
                // so build one from the request metadata.
                let mut metadata =
                    hickory_proto::op::Metadata::response_from_request(&request.metadata);
                metadata.response_code = hickory_proto::op::ResponseCode::ServFail;
                ResponseInfo::from(hickory_proto::op::Header {
                    metadata,
                    counts: hickory_proto::op::HeaderCounts::default(),
                })
            }
        }
    }

    /// Re-encode `message` and send it to the real client handler.
    async fn replay_response<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: R,
        message: Message,
        _original_info: ResponseInfo,
    ) -> ResponseInfo {
        let response = serve_stale::build_message_response(request, message);
        let mut rh = response_handle.clone();
        match rh.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                warn!(error = %e, "serve-stale: failed to replay response");
                let mut metadata =
                    hickory_proto::op::Metadata::response_from_request(&request.metadata);
                metadata.response_code = hickory_proto::op::ResponseCode::ServFail;
                ResponseInfo::from(hickory_proto::op::Header {
                    metadata,
                    counts: hickory_proto::op::HeaderCounts::default(),
                })
            }
        }
    }
}

/// Extension trait so middleware can report a human-readable label in logs.
pub trait DnsMiddlewareLabel {
    /// A short, static label identifying the middleware type.
    fn label(&self) -> &'static str;
}

impl<T: DnsMiddleware + ?Sized> DnsMiddlewareLabel for &T {
    fn label(&self) -> &'static str {
        "middleware"
    }
}

impl DnsMiddlewareLabel for Box<dyn DnsMiddleware> {
    fn label(&self) -> &'static str {
        "middleware"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CacheConfig;
    use crate::dns::caching::CacheKey;
    use crate::dns::serve_stale::{CapturingResponseHandler, ServeStaleHandler};
    use hickory_net::runtime::TokioTime;
    use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
    use hickory_proto::rr::{Name, RData, Record, RecordType};
    use hickory_proto::serialize::binary::BinEncodable;
    use hickory_server::net::xfer::Protocol;
    use hickory_server::zone_handler::Catalog;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Instant;

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
        msg.add_answer(Record::from_rdata(
            Name::from_utf8(name).unwrap(),
            ttl,
            RData::A(ip.into()),
        ));
        msg
    }

    fn make_cache_config(serve_stale: bool, serve_stale_ttl: u64) -> CacheConfig {
        CacheConfig {
            min_ttl: 0,
            max_ttl: 86400,
            negative_ttl: 60,
            serve_stale,
            serve_stale_ttl,
            max_entries: 4096,
        }
    }

    /// A `DnshubHandler` with an empty catalog (no zone handlers) returns
    /// `Refused` for any query. We use this to test that the pre-forward
    /// middleware `Serve` action bypasses the catalog entirely when a fresh
    /// stale-cache entry exists.
    #[tokio::test]
    async fn serve_stale_fresh_entry_bypasses_catalog() {
        let catalog = Catalog::new();
        let ss = ServeStaleHandler::new(make_cache_config(true, 86400));

        // Pre-populate the stale cache with a fresh entry.
        let key = CacheKey::from_name_qtype("example.com", RecordType::A);
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));
        ss.cache().insert(key, msg, Instant::now());

        let handler = DnshubHandler::from_catalog(catalog).with_serve_stale(ss);

        let req = make_request("example.com");
        let capturing = CapturingResponseHandler::new(Protocol::Udp);
        let info = handler
            .handle_request::<CapturingResponseHandler, TokioTime>(
                &req,
                capturing.clone(),
            )
            .await;

        // The fresh entry should have been served (NOERROR), not Refused.
        assert_eq!(info.response_code, ResponseCode::NoError);
        let captured = capturing.take().expect("response was sent");
        let decoded = Message::from_vec(&captured.bytes).unwrap();
        assert_eq!(decoded.answers.len(), 1);
    }

    /// When serve-stale is disabled, the handler delegates to the catalog
    /// normally (empty catalog → Refused).
    #[tokio::test]
    async fn serve_stale_disabled_delegates_to_catalog() {
        let catalog = Catalog::new();
        let ss = ServeStaleHandler::new(make_cache_config(false, 86400));

        // Even with a fresh entry, disabled means Continue.
        let key = CacheKey::from_name_qtype("example.com", RecordType::A);
        let msg = make_response("example.com.", 300, Ipv4Addr::new(1, 2, 3, 4));
        ss.cache().insert(key, msg, Instant::now());

        let handler = DnshubHandler::from_catalog(catalog).with_serve_stale(ss);

        let req = make_request("example.com");
        let capturing = CapturingResponseHandler::new(Protocol::Udp);
        let info = handler
            .handle_request::<CapturingResponseHandler, TokioTime>(
                &req,
                capturing.clone(),
            )
            .await;

        // Disabled → catalog handles it → empty catalog returns Refused.
        assert_eq!(info.response_code, ResponseCode::Refused);
    }

    /// When the catalog returns a non-SERVFAIL error (Refused from empty
    /// catalog) and there is no stale entry, the response is forwarded as-is.
    #[tokio::test]
    async fn no_stale_entry_forwards_catalog_response() {
        let catalog = Catalog::new();
        let ss = ServeStaleHandler::new(make_cache_config(true, 86400));

        let handler = DnshubHandler::from_catalog(catalog).with_serve_stale(ss);

        let req = make_request("nonexistent.example");
        let capturing = CapturingResponseHandler::new(Protocol::Udp);
        let info = handler
            .handle_request::<CapturingResponseHandler, TokioTime>(
                &req,
                capturing.clone(),
            )
            .await;

        // No fresh entry in stale cache → catalog handles → Refused.
        assert_eq!(info.response_code, ResponseCode::Refused);
    }

    /// Verify that the ServeStaleHandler in the middleware chain shares the
    /// same StaleCache as the one used for post-forward logic.
    #[tokio::test]
    async fn with_serve_stale_shares_cache_between_middleware_and_postforward() {
        let catalog = Catalog::new();
        let ss = ServeStaleHandler::new(make_cache_config(true, 86400));
        let cache = Arc::clone(ss.cache());

        let handler = DnshubHandler::from_catalog(catalog).with_serve_stale(ss);

        // Store a response via the shared cache.
        let key = CacheKey::from_name_qtype("test.com", RecordType::A);
        let msg = make_response("test.com.", 300, Ipv4Addr::new(5, 6, 7, 8));
        cache.insert(key, msg, Instant::now());

        // The middleware should serve it fresh.
        let req = make_request("test.com");
        let capturing = CapturingResponseHandler::new(Protocol::Udp);
        let info = handler
            .handle_request::<CapturingResponseHandler, TokioTime>(
                &req,
                capturing.clone(),
            )
            .await;

        assert_eq!(info.response_code, ResponseCode::NoError);
    }
}
