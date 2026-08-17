//! Custom DNS-over-HTTPS (DoH) server with full RFC 8484 support.
//!
//! This module implements a production DoH server using `axum` + `hyper`
//! + `tokio-rustls` that supports **both** POST (wire-format body) and
//! GET (`?dns=` base64url parameter) requests, unlike hickory-server 0.26's
//! built-in HTTPS listener which only handles POST.
//!
//! ## Architecture
//!
//! The server listens for HTTPS (HTTP/2) connections on the configured
//! addresses. Each request is decoded into a hickory `Message`, dispatched
//! to the shared [`DnshubHandler`](crate::dns::DnshubHandler) via
//! `Request::from_bytes`, and the response is encoded back to wire format
//! and returned with `Content-Type: application/dns-message`.
//!
//! The same handler is used for UDP/TCP/DoT/DoH, so DoH clients pass
//! through the identical middleware chain (rate limiting, per-client
//! policy, ECS stripping) and forwarding catalog as plain DNS.
//!
//! ## TLS
//!
//! TLS certificates are loaded from the configured `cert`/`key` PEM paths.
//! When `[server.doh]` does not specify its own `cert`/`key`, the
//! certificates from `[server.tls]` (DoT) are reused.

use crate::config::{DohServerConfig, TlsServerConfig};
use crate::dns::doh::{parse_get_dns_param, DohError};
use crate::dns::DnshubHandler;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use hickory_proto::op::Message;
use hickory_server::server::{Request, RequestHandler};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

/// A custom DoH server that supports both GET and POST methods.
pub struct DohAxumServer {
    handler: Arc<DnshubHandler>,
    tls_acceptor: TlsAcceptor,
    path: String,
    listen: Vec<String>,
}

impl DohAxumServer {
    /// Build a custom DoH server from the DoH config and a shared handler.
    ///
    /// `tls` is the `[server.tls]` config (DoT). When `doh.cert` / `doh.key`
    /// are empty, the cert/key are taken from `tls`. If neither DoH nor DoT
    /// provide cert/key paths, an error is returned.
    pub fn new(
        doh: &DohServerConfig,
        tls: Option<&TlsServerConfig>,
        handler: Arc<DnshubHandler>,
    ) -> Result<Self, DohError> {
        let cert_path = effective_cert_path(doh, tls);
        let key_path = effective_key_path(doh, tls);

        let (cert_path, key_path) = match (cert_path, key_path) {
            (Some(c), Some(k)) => (c, k),
            _ => {
                return Err(DohError::Tls(std::io::Error::other(
                    "DoH requires a TLS certificate and key: set [server.doh].cert/key \
                     or enable [server.tls] with cert/key to reuse them",
                )));
            }
        };

        let tls_acceptor = load_tls_acceptor(cert_path, key_path)?;
        let path = effective_path(doh).to_string();

        info!(
            cert = %cert_path,
            key = %key_path,
            path = %path,
            "custom DoH server TLS material loaded",
        );

        Ok(Self {
            handler,
            tls_acceptor,
            path,
            listen: doh.listen.clone(),
        })
    }

    /// Start serving DoH on all configured listen addresses.
    ///
    /// Each address is served in a background tokio task. This method
    /// returns immediately after spawning the tasks; the caller is
    /// responsible for keeping the runtime alive.
    pub async fn serve_all(self: Arc<Self>) -> Vec<SocketAddr> {
        let mut bound = Vec::with_capacity(self.listen.len());
        for addr_str in &self.listen {
            let server = self.clone();
            let listener = match TcpListener::bind(addr_str).await {
                Ok(l) => l,
                Err(e) => {
                    warn!(addr = %addr_str, error = %e, "failed to bind DoH listener");
                    continue;
                }
            };
            let local = listener.local_addr().unwrap_or_else(|_| {
                addr_str.parse().unwrap_or_else(|_| {
                    "0.0.0.0:443".parse::<SocketAddr>().unwrap()
                })
            });
            info!(addr = %local, path = %server.path, "custom DoH server listening");
            bound.push(local);

            let server = server.clone();
            tokio::spawn(async move {
                server.serve_one(listener).await;
            });
        }
        bound
    }

    /// Serve DoH on a single TCP listener.
    async fn serve_one(self: Arc<Self>, listener: TcpListener) {
        let router = self.build_router();
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "DoH accept error");
                    continue;
                }
            };

            let acceptor = self.tls_acceptor.clone();
            let router = router.clone();
            tokio::spawn(async move {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        // Use hyper directly to serve HTTP/2 over TLS.
                        let io = hyper_util::rt::TokioIo::new(tls_stream);
                        let svc = hyper::service::service_fn(move |req| {
                            let router = router.clone();
                            async move {
                                use tower::ServiceExt;
                                router.oneshot(req).await
                            }
                        });
                        if let Err(e) = hyper::server::conn::http2::Builder::new(
                            hyper_util::rt::TokioExecutor::new(),
                        )
                        .serve_connection(io, svc)
                        .await
                        {
                            warn!(error = %e, peer = %peer, "DoH HTTP/2 connection error");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, peer = %peer, "DoH TLS handshake failed");
                    }
                }
            });
        }
    }

    /// Build the axum router for DoH requests.
    fn build_router(&self) -> Router {
        let path = self.path.clone();
        let state = self.handler.clone();

        // POST: wire-format body.
        let post_route = post(handle_post);
        // GET: ?dns= base64url parameter.
        let get_route = axum::routing::get(handle_get);

        Router::new()
            .route(&path, post_route.get(get_route))
            .with_state(state)
    }
}

/// Axum state is the shared handler.
type HandlerState = Arc<DnshubHandler>;

/// Handle a POST DoH request (RFC 8484 §4.1.1).
///
/// The request body is the raw DNS wire-format message.
async fn handle_post(
    State(handler): State<HandlerState>,
    body: Bytes,
) -> axum::response::Response {
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty DoH POST body").into_response();
    }
    process_dns_query(&handler, &body, std::net::SocketAddr::from(([0, 0, 0, 0], 0))).await
}

/// Handle a GET DoH request (RFC 8484 §4.1.2).
///
/// The DNS message is base64url-encoded in the `?dns=` query parameter.
async fn handle_get(
    State(handler): State<HandlerState>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let dns_param = match params.get("dns") {
        Some(v) => v,
        None => {
            return (StatusCode::BAD_REQUEST, "missing ?dns= parameter").into_response();
        }
    };

    let target = format!("/?dns={dns_param}");
    let decoded = match parse_get_dns_param(&target) {
        Ok(bytes) => bytes,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid ?dns= parameter: {e}"))
                .into_response();
        }
    };

    process_dns_query(&handler, &decoded, std::net::SocketAddr::from(([0, 0, 0, 0], 0))).await
}

/// Process a decoded DNS wire-format query through the handler and
/// return the HTTP response with the encoded DNS message.
async fn process_dns_query(
    handler: &DnshubHandler,
    wire: &[u8],
    src: std::net::SocketAddr,
) -> axum::response::Response {
    // Parse the wire-format message into a hickory Request.
    let request = match Request::from_bytes(wire.to_vec(), src, hickory_server::net::xfer::Protocol::Https) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "DoH: failed to parse DNS message");
            return (StatusCode::BAD_REQUEST, "invalid DNS message").into_response();
        }
    };

    // Use a capturing response handler to get the response bytes.
    let capturing = crate::dns::serve_stale::CapturingResponseHandler::new(
        hickory_server::net::xfer::Protocol::Https,
    );

    let info = handler.handle_request::<_, hickory_net::runtime::TokioTime>(&request, capturing.clone()).await;

    // Get the captured response bytes.
    let captured = match capturing.take() {
        Some(c) => c,
        None => {
            // The handler didn't send a response via the capturing handler.
            // Build a SERVFAIL response.
            let mut msg = Message::new(0, hickory_proto::op::MessageType::Response, hickory_proto::op::OpCode::Query);
            msg.metadata.response_code = hickory_proto::op::ResponseCode::ServFail;
            let bytes = msg.to_vec().unwrap_or_default();
            return build_doh_response(&bytes);
        }
    };

    // If the response code indicates failure and we have no bytes, encode a SERVFAIL.
    let response_bytes = if captured.bytes.is_empty() {
        let mut msg = Message::new(0, hickory_proto::op::MessageType::Response, hickory_proto::op::OpCode::Query);
        msg.metadata.response_code = hickory_proto::op::ResponseCode::ServFail;
        msg.to_vec().unwrap_or_default()
    } else {
        captured.bytes
    };

    let _ = info; // info contains response code/counts; bytes are authoritative
    build_doh_response(&response_bytes)
}

/// Build an HTTP response with the DNS wire-format body.
fn build_doh_response(bytes: &[u8]) -> axum::response::Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/dns-message"),
         (header::CACHE_CONTROL, "max-age=60")],
        Bytes::copy_from_slice(bytes),
    )
        .into_response()
}

/// Load TLS certificate chain and private key, build a `ServerConfig`,
/// and return a `TlsAcceptor` configured for HTTP/2 (ALPN "h2").
fn load_tls_acceptor(
    cert_path: &str,
    key_path: &str,
) -> Result<TlsAcceptor, DohError> {
    let cert_chain = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| std::io::Error::other(format!("failed to read cert PEM {cert_path}: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| std::io::Error::other(format!("failed to parse cert PEM {cert_path}: {e}")))?;

    if cert_chain.is_empty() {
        return Err(DohError::Tls(std::io::Error::other(format!(
            "no certificates found in {cert_path}"
        ))));
    }

    let key = PrivateKeyDer::from_pem_file(key_path)
        .map_err(|e| std::io::Error::other(format!("failed to read key PEM {key_path}: {e}")))?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| std::io::Error::other(format!("failed to build TLS server config: {e}")))?;

    // Set ALPN protocols to support HTTP/2 (h2).
    config.alpn_protocols = vec![b"h2".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Resolve the effective certificate path: DoH's own, else DoT's.
fn effective_cert_path<'a>(
    doh: &'a DohServerConfig,
    tls: Option<&'a TlsServerConfig>,
) -> Option<&'a str> {
    if !doh.cert.is_empty() {
        Some(&doh.cert)
    } else {
        tls.filter(|t| t.enabled && !t.cert.is_empty()).map(|t| t.cert.as_str())
    }
}

/// Resolve the effective private-key path: DoH's own, else DoT's.
fn effective_key_path<'a>(
    doh: &'a DohServerConfig,
    tls: Option<&'a TlsServerConfig>,
) -> Option<&'a str> {
    if !doh.key.is_empty() {
        Some(&doh.key)
    } else {
        tls.filter(|t| t.enabled && !t.key.is_empty()).map(|t| t.key.as_str())
    }
}

/// The effective DoH path, defaulting to `/dns-query` when empty.
fn effective_path(doh: &DohServerConfig) -> &str {
    if doh.path.is_empty() {
        crate::dns::doh::DEFAULT_DOH_PATH
    } else {
        &doh.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DohServerConfig;
    use hickory_proto::op::{Message, MessageType, OpCode, Query};
    use hickory_proto::rr::{Name, RecordType};
    use hickory_proto::serialize::binary::BinEncodable;

    /// Build a minimal DNS query message and encode it to wire format.
    fn sample_query_bytes() -> Vec<u8> {
        let name = Name::from_utf8("example.com").unwrap();
        let mut msg = Message::new(0x1234, MessageType::Query, OpCode::Query);
        msg.add_query(Query::query(name, RecordType::A));
        msg.to_bytes().unwrap()
    }

    #[test]
    fn effective_path_defaults_to_dns_query() {
        let cfg = DohServerConfig::default();
        assert_eq!(effective_path(&cfg), "/dns-query");
        let cfg = DohServerConfig {
            path: "/query".to_string(),
            ..Default::default()
        };
        assert_eq!(effective_path(&cfg), "/query");
    }

    #[test]
    fn effective_cert_key_reuses_dot_when_doh_empty() {
        let doh = DohServerConfig {
            enabled: true,
            listen: vec!["0.0.0.0:443".to_string()],
            ..Default::default()
        };
        let tls = TlsServerConfig {
            enabled: true,
            listen: vec!["0.0.0.0:853".to_string()],
            cert: "/etc/dnshub/dot.pem".to_string(),
            key: "/etc/dnshub/dot.key".to_string(),
        };
        assert_eq!(
            effective_cert_path(&doh, Some(&tls)),
            Some("/etc/dnshub/dot.pem")
        );
        assert_eq!(
            effective_key_path(&doh, Some(&tls)),
            Some("/etc/dnshub/dot.key")
        );
    }

    #[test]
    fn effective_cert_key_prefers_doh_over_dot() {
        let doh = DohServerConfig {
            enabled: true,
            listen: vec!["0.0.0.0:443".to_string()],
            cert: "/etc/dnshub/doh.pem".to_string(),
            key: "/etc/dnshub/doh.key".to_string(),
            ..Default::default()
        };
        let tls = TlsServerConfig {
            enabled: true,
            listen: vec!["0.0.0.0:853".to_string()],
            cert: "/etc/dnshub/dot.pem".to_string(),
            key: "/etc/dnshub/dot.key".to_string(),
        };
        assert_eq!(
            effective_cert_path(&doh, Some(&tls)),
            Some("/etc/dnshub/doh.pem")
        );
        assert_eq!(effective_key_path(&doh, Some(&tls)), Some("/etc/dnshub/doh.key"));
    }

    #[test]
    fn effective_cert_key_errors_when_neither_configured() {
        let doh = DohServerConfig::default();
        assert_eq!(effective_cert_path(&doh, None), None);
        assert_eq!(effective_key_path(&doh, None), None);
    }

    #[test]
    fn build_doh_response_has_correct_headers() {
        let bytes = sample_query_bytes();
        let response = build_doh_response(&bytes);
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "application/dns-message"
        );
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "max-age=60");
    }

    #[tokio::test]
    async fn handle_post_empty_body_returns_400() {
        let handler = Arc::new(DnshubHandler::from_catalog(
            hickory_server::zone_handler::Catalog::new(),
        ));
        let response = handle_post(State(handler), Bytes::new()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_get_missing_dns_param_returns_400() {
        let handler = Arc::new(DnshubHandler::from_catalog(
            hickory_server::zone_handler::Catalog::new(),
        ));
        let params: HashMap<String, String> = HashMap::new();
        let response = handle_get(State(handler), Query(params)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_get_with_garbage_dns_param_returns_400() {
        let handler = Arc::new(DnshubHandler::from_catalog(
            hickory_server::zone_handler::Catalog::new(),
        ));
        let mut params = HashMap::new();
        params.insert("dns".to_string(), "!!!invalid!!!".to_string());
        let response = handle_get(State(handler), Query(params)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn handle_post_with_valid_dns_query_returns_200() {
        // Build a handler with an empty catalog. The query will get a
        // SERVFAIL or REFUSED response, but the HTTP status should be 200
        // with application/dns-message content type.
        let handler = Arc::new(DnshubHandler::from_catalog(
            hickory_server::zone_handler::Catalog::new(),
        ));
        let wire = sample_query_bytes();
        let response = handle_post(State(handler), Bytes::from(wire)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "application/dns-message"
        );
    }

    #[tokio::test]
    async fn handle_get_with_valid_dns_query_returns_200() {
        let handler = Arc::new(DnshubHandler::from_catalog(
            hickory_server::zone_handler::Catalog::new(),
        ));
        let wire = sample_query_bytes();
        let encoded = base64url_encode(&wire);
        let mut params = HashMap::new();
        params.insert("dns".to_string(), encoded);
        let response = handle_get(State(handler), Query(params)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "application/dns-message"
        );
    }

    /// Minimal base64url encoder (no padding) for test round-tripping.
    fn base64url_encode(input: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
        let mut chunks = input.chunks_exact(3);
        for c in &mut chunks {
            let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
            out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            out.push(TABLE[(n & 0x3f) as usize] as char);
        }
        let rem = chunks.remainder();
        match rem.len() {
            1 => {
                let n = (rem[0] as u32) << 16;
                out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
                out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
            }
            2 => {
                let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
                out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
                out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
                out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
            }
            _ => {}
        }
        out
    }
}
