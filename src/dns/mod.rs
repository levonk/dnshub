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
pub mod forwarding;
pub mod server;

use async_trait::async_trait;
use hickory_net::runtime::Time;
use hickory_proto::op::ResponseCode;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::{Catalog, MessageResponseBuilder};
use std::future::Future;
use std::pin::Pin;
use tracing::{debug, warn};

/// The action returned by a [`DnsMiddleware`] handler.
#[derive(Debug, Clone, Copy)]
pub enum MiddlewareAction {
    /// Continue down the chain (no opinion on this request).
    Continue,
    /// Short-circuit the chain and return this response code to the client.
    Reject(ResponseCode),
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
}

/// The top-level dnshub request handler.
///
/// Wraps a hickory [`Catalog`] (which performs zone dispatch / forwarding) and
/// runs a chain of [`DnsMiddleware`] before delegating to it. Implements
/// hickory-server's [`RequestHandler`] so it can be handed to
/// [`Server::new`](hickory_server::server::Server::new).
pub struct DnshubHandler {
    catalog: Catalog,
    middleware: Vec<Box<dyn DnsMiddleware>>,
}

impl DnshubHandler {
    /// Build a handler from an already-populated catalog and an (possibly empty)
    /// middleware chain.
    pub fn new(catalog: Catalog, middleware: Vec<Box<dyn DnsMiddleware>>) -> Self {
        Self { catalog, middleware }
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
                }
            }

            // Delegate to the catalog (zone dispatch / forwarding).
            self.catalog.handle_request::<R, T>(request, response_handle).await
        })
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
