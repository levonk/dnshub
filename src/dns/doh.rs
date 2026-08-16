//! DNS-over-HTTPS (DoH) server — RFC 8484.
//!
//! [`DohServer`] configures hickory-server's built-in HTTPS (HTTP/2) listener
//! ([`Server::register_https_listener`](hickory_server::server::Server::register_https_listener))
//! to serve DoH on port 443 (or any configured address). The listener performs
//! TLS termination (rustls), HTTP/2 framing, and RFC 8484 wire-format POST
//! handling (`application/dns-message`), delegating each query to the same
//! [`DnshubHandler`](crate::dns::DnshubHandler) registered for UDP/TCP so DoH
//! clients are subject to the same per-client policy, blocklists, and tiered
//! forwarding as plain DNS.
//!
//! ## TLS certificates
//!
//! TLS certificates are loaded from the configured `cert`/`key` PEM paths.
//! When `[server.doh]` does not specify its own `cert`/`key`, the certificates
//! from `[server.tls]` (DoT) are reused — DoH and DoT can share the same
//! TLS material.
//!
//! ## GET support
//!
//! RFC 8484 §4.1 also defines a GET method where the DNS message is carried
//! base64url-encoded in the `?dns=` query parameter. hickory-net 0.26's h2
//! handler does not implement GET (it returns an error for GET requests), so
//! the production listener currently serves POST only. The base64url decoding
//! and wire-format parsing helpers ([`parse_get_dns_param`],
//! [`parse_doh_message`]) are implemented and unit-tested here so the logic is
//! available for a future h2 handler with GET support or a custom listener.

use crate::config::{DohServerConfig, TlsServerConfig};
use hickory_proto::op::Message;
use hickory_proto::serialize::binary::DecodeError;
use hickory_server::net::tls::default_provider;
use hickory_server::server::{RequestHandler, Server};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ResolvesServerCert;
use rustls::sign::{CertifiedKey, SingleCertAndKey};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::info;

/// TLS handshake timeout for incoming HTTPS (h2) connections.
const HTTPS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Default DoH endpoint path per RFC 8484 / PRD §4.7.
pub const DEFAULT_DOH_PATH: &str = "/dns-query";

/// Errors that can occur while building or running the DoH server.
#[derive(Debug)]
pub enum DohError {
    /// Failed to read or parse the TLS certificate / key material.
    Tls(io::Error),
    /// Failed to bind an HTTPS listener.
    Bind(io::Error),
    /// hickory-server rejected the HTTPS listener registration.
    Register(io::Error),
}

impl std::fmt::Display for DohError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DohError::Tls(e) => write!(f, "DoH TLS error: {e}"),
            DohError::Bind(e) => write!(f, "DoH bind error: {e}"),
            DohError::Register(e) => write!(f, "DoH listener registration error: {e}"),
        }
    }
}

impl std::error::Error for DohError {}

impl From<io::Error> for DohError {
    fn from(e: io::Error) -> Self {
        DohError::Tls(e)
    }
}

/// A configured DoH (DNS-over-HTTPS) server.
///
/// Owns the loaded TLS certificate resolver. Call [`DohServer::register`] to
/// attach the HTTPS listener(s) to a hickory-server [`Server`]; the server
/// then drives the listener alongside its UDP/TCP sockets.
///
/// The same [`DnshubHandler`](crate::dns::DnshubHandler) is used for every
/// transport, so DoH queries pass through the identical middleware chain
/// (rate limiting, per-client policy, ECS stripping) and forwarding catalog
/// as plain DNS.
pub struct DohServer {
    config: DohServerConfig,
    cert_resolver: Arc<dyn ResolvesServerCert>,
}

impl DohServer {
    /// Build a [`DohServer`] from the DoH config, reusing the DoT TLS
    /// certificate/key when the DoH config does not specify its own.
    ///
    /// `tls` is the `[server.tls]` config (DoT). When `doh.cert` / `doh.key`
    /// are empty, the cert/key are taken from `tls`. If neither DoH nor DoT
    /// provide cert/key paths, an error is returned.
    pub fn new(doh: &DohServerConfig, tls: Option<&TlsServerConfig>) -> Result<Self, DohError> {
        let cert_path = effective_cert_path(doh, tls);
        let key_path = effective_key_path(doh, tls);

        let (cert_path, key_path) = match (cert_path, key_path) {
            (Some(c), Some(k)) => (c, k),
            _ => {
                return Err(DohError::Tls(io::Error::other(
                    "DoH requires a TLS certificate and key: set [server.doh].cert/key \
                     or enable [server.tls] with cert/key to reuse them",
                )));
            }
        };

        let cert_resolver = load_cert_resolver(cert_path, key_path)?;
        info!(
            cert = %cert_path,
            key = %key_path,
            path = %effective_path(doh),
            "DoH TLS material loaded",
        );

        Ok(Self {
            config: doh.clone(),
            cert_resolver,
        })
    }

    /// The configured DoH endpoint path (defaults to `/dns-query`).
    pub fn path(&self) -> &str {
        effective_path(&self.config)
    }

    /// Register the HTTPS (h2) listener for each configured listen address
    /// onto `server`. Returns the bound socket addresses.
    ///
    /// Each listener serves RFC 8484 wire-format POST requests at
    /// [`Self::path`] using the server's request handler.
    pub async fn register<T: RequestHandler>(
        &self,
        server: &mut Server<T>,
    ) -> Result<Vec<SocketAddr>, DohError> {
        let path = self.path().to_string();
        let mut bound = Vec::with_capacity(self.config.listen.len());
        for addr in &self.config.listen {
            let listener = TcpListener::bind(addr)
                .await
                .map_err(DohError::Bind)?;
            let local = listener.local_addr().map_err(DohError::Bind)?;
            server
                .register_https_listener(
                    listener,
                    HTTPS_HANDSHAKE_TIMEOUT,
                    self.cert_resolver.clone(),
                    None,
                    path.clone(),
                )
                .map_err(DohError::Register)?;
            info!(addr = %local, path = %path, "registered DoH (HTTPS) listener");
            bound.push(local);
        }
        Ok(bound)
    }
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
        DEFAULT_DOH_PATH
    } else {
        &doh.path
    }
}

/// Load a TLS certificate chain and private key from PEM files and build a
/// [`ResolvesServerCert`] suitable for hickory-server's HTTPS listener.
pub fn load_cert_resolver(
    cert_path: &str,
    key_path: &str,
) -> Result<Arc<dyn ResolvesServerCert>, DohError> {
    let cert_chain = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| io::Error::other(format!("failed to read cert PEM {cert_path}: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| io::Error::other(format!("failed to parse cert PEM {cert_path}: {e}")))?;

    if cert_chain.is_empty() {
        return Err(DohError::Tls(io::Error::other(format!(
            "no certificates found in {cert_path}"
        ))));
    }

    let key = PrivateKeyDer::from_pem_file(key_path)
        .map_err(|e| io::Error::other(format!("failed to read key PEM {key_path}: {e}")))?;

    let certified_key = CertifiedKey::from_der(cert_chain, key, &default_provider())
        .map_err(|e| io::Error::other(format!("failed to build certified key: {e}")))?;

    Ok(Arc::new(SingleCertAndKey::from(certified_key)))
}

// ---------------------------------------------------------------------------
// RFC 8484 wire-format parsing helpers (POST body + GET ?dns= parameter).
// ---------------------------------------------------------------------------

/// Parse a DNS wire-format message from an HTTP POST body
/// (`application/dns-message`).
///
/// Returns the decoded [`Message`] or an error if the bytes are not a valid
/// DNS message.
pub fn parse_doh_message(body: &[u8]) -> Result<Message, DecodeError> {
    Message::from_vec(body)
}

/// Extract the `dns=` query parameter from an HTTP request target (the path
/// plus query string, e.g. `/dns-query?dns=AAAB...`) and base64url-decode it
/// into the raw DNS wire-format message bytes.
///
/// Per RFC 8484 §4.1 the parameter value is base64url-encoded **without**
/// padding. Padding is tolerated if present.
pub fn parse_get_dns_param(request_target: &str) -> Result<Vec<u8>, DohError> {
    let query = request_target.split_once('?').map(|(_, q)| q).unwrap_or("");
    for pair in query.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        if k == "dns" {
            let decoded = base64url_decode(v.as_bytes())?;
            if decoded.is_empty() {
                return Err(DohError::Tls(io::Error::other(
                    "DoH GET ?dns= parameter is empty",
                )));
            }
            return Ok(decoded);
        }
    }
    Err(DohError::Tls(io::Error::other(
        "DoH GET request missing ?dns= parameter",
    )))
}

/// Base64url decoder (RFC 4648 §5, no padding; tolerates padding and
/// whitespace). Implemented locally to avoid pulling in a base64 dependency.
fn base64url_decode(input: &[u8]) -> Result<Vec<u8>, DohError> {
    fn decode_val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            // Tolerate standard-base64 URL-unsafe chars too (some clients use
            // '+' and '/' despite the RFC mandating '-' and '_').
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    // Strip whitespace and '=' padding for length computation.
    let filtered: Vec<u8> = input
        .iter()
        .copied()
        .filter(|&b| !b.is_ascii_whitespace() && b != b'=')
        .collect();

    if filtered.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::with_capacity(filtered.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in &filtered {
        let v = decode_val(b).ok_or_else(|| {
            DohError::Tls(io::Error::other(format!(
                "invalid base64url character: {:#?}",
                b as char
            )))
        })?;
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1u32 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DohServerConfig, TlsServerConfig};
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
    fn parse_doh_post_body_decodes_wire_format() {
        let wire = sample_query_bytes();
        let msg = parse_doh_message(&wire).expect("POST body should parse");
        assert_eq!(msg.metadata.id, 0x1234);
        assert_eq!(msg.metadata.message_type, MessageType::Query);
        assert_eq!(msg.metadata.op_code, OpCode::Query);
        assert_eq!(msg.queries.len(), 1);
        let q = &msg.queries[0];
        assert_eq!(q.query_type(), RecordType::A);
        assert_eq!(q.name().to_string(), "example.com.");
    }

    #[test]
    fn parse_doh_post_body_rejects_garbage() {
        let bad = [0u8; 3];
        assert!(parse_doh_message(&bad).is_err());
    }

    #[test]
    fn parse_get_dns_param_decodes_base64url() {
        let wire = sample_query_bytes();
        let encoded = base64url_encode(&wire);
        let target = format!("/dns-query?dns={encoded}");
        let decoded = parse_get_dns_param(&target).expect("GET ?dns= should decode");
        assert_eq!(decoded, wire);
        // Round-trip through the wire-format parser.
        let msg = parse_doh_message(&decoded).expect("decoded GET body should parse");
        assert_eq!(msg.metadata.id, 0x1234);
        assert_eq!(msg.queries.len(), 1);
    }

    #[test]
    fn parse_get_dns_param_tolerates_padding() {
        let wire = sample_query_bytes();
        let mut encoded = base64url_encode(&wire);
        while encoded.len() % 4 != 0 {
            encoded.push('=');
        }
        let target = format!("/dns-query?dns={encoded}");
        let decoded = parse_get_dns_param(&target).expect("padded ?dns= should decode");
        assert_eq!(decoded, wire);
    }

    #[test]
    fn parse_get_dns_param_missing_param_errors() {
        assert!(parse_get_dns_param("/dns-query").is_err());
        assert!(parse_get_dns_param("/dns-query?foo=bar").is_err());
    }

    #[test]
    fn parse_get_dns_param_ignores_other_params() {
        let wire = sample_query_bytes();
        let encoded = base64url_encode(&wire);
        let target = format!("/dns-query?ct=application/dns-message&dns={encoded}");
        let decoded = parse_get_dns_param(&target).expect("?dns= should be found");
        assert_eq!(decoded, wire);
    }

    #[test]
    fn base64url_decode_known_vector() {
        // "hello" -> base64url (no pad) = "aGVsbG8"
        assert_eq!(base64url_decode(b"aGVsbG8").unwrap(), b"hello");
        // RFC 4648 §10 test vectors (base64url):
        assert_eq!(base64url_decode(b"").unwrap(), Vec::<u8>::new());
        assert_eq!(base64url_decode(b"Zg").unwrap(), b"f");
        assert_eq!(base64url_decode(b"Zm8").unwrap(), b"fo");
        assert_eq!(base64url_decode(b"Zm9v").unwrap(), b"foo");
        assert_eq!(base64url_decode(b"Zm9vYg").unwrap(), b"foob");
        assert_eq!(base64url_decode(b"Zm9vYmE").unwrap(), b"fooba");
        assert_eq!(base64url_decode(b"Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn base64url_decode_rejects_invalid_chars() {
        assert!(base64url_decode(b"Zm9v!").is_err());
    }

    #[test]
    fn doh_config_parsing_roundtrip() {
        let toml = r#"
enabled = true
listen = ["0.0.0.0:443", "[::]:443"]
path = "/dns-query"
cert = "/etc/dnshub/cert.pem"
key = "/etc/dnshub/key.pem"
"#;
        let cfg: DohServerConfig = toml::from_str(toml).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.listen, vec!["0.0.0.0:443", "[::]:443"]);
        assert_eq!(cfg.path, "/dns-query");
        assert_eq!(cfg.cert, "/etc/dnshub/cert.pem");
        assert_eq!(cfg.key, "/etc/dnshub/key.pem");
    }

    #[test]
    fn doh_config_defaults_are_empty() {
        let cfg = DohServerConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.listen.is_empty());
        assert!(cfg.path.is_empty());
        assert!(cfg.cert.is_empty());
        assert!(cfg.key.is_empty());
    }

    #[test]
    fn effective_path_defaults_to_dns_query() {
        let cfg = DohServerConfig::default();
        assert_eq!(effective_path(&cfg), DEFAULT_DOH_PATH);
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
