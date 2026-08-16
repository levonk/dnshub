//! DNS-over-TLS (DoT) server — RFC 7858.
//!
//! [`DotServer`] loads a PEM certificate chain and private key from the paths
//! configured in `[server.tls]`, builds a rustls [`ServerConfig`] with the
//! `dot` ALPN identifier, and registers a hickory-server TLS listener on port
//! 853 (by default). The TLS listener runs on the *same*
//! [`Server<DnshubHandler>`](hickory_server::server::Server) as the UDP/TCP
//! listeners, so DoT clients are served by the identical handler chain —
//! per-client policy, blocklists, rate limiting, and forwarding all apply
//! exactly as they do for plain DNS. The source IP is extracted from the TLS
//! connection by hickory-server and is available to
//! [`ClientResolver`](crate::client_resolver::ClientResolver) via the
//! [`Request`](hickory_server::server::Request) passed to the handler.
//!
//! Certificates are mounted read-only from the host (reusing existing
//! Traefik/ACME certs — see PRD §4.7). Cert changes require a restart in v1;
//! hot-reload is deferred to a future story.

use crate::config::TlsServerConfig;
use crate::dns::DnshubHandler;
use hickory_net::tls::default_provider;
use hickory_server::server::{Server, default_tls_server_config};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ResolvesServerCert;
use rustls::sign::{CertifiedKey, SingleCertAndKey};
use rustls::ServerConfig;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::{info, warn};

/// TLS handshake timeout for incoming DoT connections (RFC 7858 §3.2).
///
/// A connection that does not complete the TLS handshake within this window is
/// dropped to mitigate resource-exhaustion from slow/silent clients.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// ALPN protocol identifier advertised by the DoT server (`"dot"`).
///
/// RFC 7858 §3.1 recommends (but does not require) the `dot` ALPN identifier.
/// hickory-server's [`default_tls_server_config`] sets this when given
/// `b"dot"`.
const DOT_ALPN: &[u8] = b"dot";

/// Errors that can occur while loading TLS material or building the rustls
/// [`ServerConfig`] for the DoT listener.
///
/// Each variant records the offending file path (where applicable) so
/// operators get a clear, actionable message instead of a generic I/O error.
#[derive(Debug)]
pub enum TlsLoadError {
    /// The PEM certificate chain file could not be read or parsed.
    CertRead {
        /// Configured path to the certificate chain PEM.
        path: String,
        /// Underlying error message.
        error: String,
    },
    /// The PEM private key file could not be read or parsed.
    KeyRead {
        /// Configured path to the private key PEM.
        path: String,
        /// Underlying error message.
        error: String,
    },
    /// The certificate chain file contained no certificates.
    EmptyCertChain {
        /// Configured path to the certificate chain PEM.
        path: String,
    },
    /// The private key could not be paired with the certificate (e.g. key
    /// does not match the cert's public key, or the key format is
    /// unsupported by the active crypto provider).
    KeyMismatch {
        /// Configured path to the private key PEM.
        path: String,
        /// Underlying error message.
        error: String,
    },
    /// The rustls [`ServerConfig`] could not be assembled (e.g. no safe
    /// default protocol versions available with the active provider).
    BuildConfig(String),
}

impl TlsLoadError {
    fn cert(path: impl Into<String>, error: impl std::fmt::Display) -> Self {
        TlsLoadError::CertRead {
            path: path.into(),
            error: error.to_string(),
        }
    }

    fn key(path: impl Into<String>, error: impl std::fmt::Display) -> Self {
        TlsLoadError::KeyRead {
            path: path.into(),
            error: error.to_string(),
        }
    }
}

impl std::fmt::Display for TlsLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsLoadError::CertRead { path, error } => {
                write!(f, "failed to read TLS certificate chain '{path}': {error}")
            }
            TlsLoadError::KeyRead { path, error } => {
                write!(f, "failed to read TLS private key '{path}': {error}")
            }
            TlsLoadError::EmptyCertChain { path } => {
                write!(f, "TLS certificate chain '{path}' contains no certificates")
            }
            TlsLoadError::KeyMismatch { path, error } => {
                write!(f, "TLS private key '{path}' is invalid or does not match the certificate: {error}")
            }
            TlsLoadError::BuildConfig(error) => {
                write!(f, "failed to build TLS server config: {error}")
            }
        }
    }
}

impl std::error::Error for TlsLoadError {}

/// Load a PEM certificate chain and private key from the paths in `tls` and
/// build a rustls [`ServerConfig`] configured for DoT (ALPN `"dot"`, safe
/// default protocol versions, no client auth).
///
/// The returned config is wrapped in [`Arc`] so it can be shared across
/// multiple TLS listeners (one per bind address).
///
/// # Errors
///
/// Returns a [`TlsLoadError`] variant identifying which file failed and why,
/// rather than panicking — this lets the caller log a clear message and exit
/// gracefully.
pub fn load_tls_server_config(tls: &TlsServerConfig) -> Result<Arc<ServerConfig>, TlsLoadError> {
    let cert_chain = read_cert_chain(Path::new(&tls.cert))?;
    let key = read_private_key(Path::new(&tls.key))?;

    let certified_key =
        CertifiedKey::from_der(cert_chain, key, &default_provider()).map_err(|e| {
            TlsLoadError::KeyMismatch {
                path: tls.key.clone(),
                error: e.to_string(),
            }
        })?;

    let resolver: Arc<dyn ResolvesServerCert> = Arc::new(SingleCertAndKey::from(certified_key));
    let config = default_tls_server_config(DOT_ALPN, resolver)
        .map_err(|e| TlsLoadError::BuildConfig(e.to_string()))?;
    Ok(Arc::new(config))
}

/// Read and parse a PEM certificate chain from `path`.
///
/// Returns the parsed chain in order (leaf first). Fails with
/// [`TlsLoadError::CertRead`] on I/O or PEM parse errors, and with
/// [`TlsLoadError::EmptyCertChain`] if the file parses but contains no
/// certificates.
fn read_cert_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsLoadError> {
    let chain = CertificateDer::pem_file_iter(path)
        .map_err(|e| TlsLoadError::cert(path.display().to_string(), e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsLoadError::cert(path.display().to_string(), e))?;

    if chain.is_empty() {
        return Err(TlsLoadError::EmptyCertChain {
            path: path.display().to_string(),
        });
    }
    Ok(chain)
}

/// Read and parse a PEM private key from `path`.
fn read_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsLoadError> {
    PrivateKeyDer::from_pem_file(path)
        .map_err(|e| TlsLoadError::key(path.display().to_string(), e))
}

/// A resolved DoT server: a rustls [`ServerConfig`] plus the list of listen
/// addresses to bind TLS listeners on.
///
/// Built from a [`TlsServerConfig`] via [`DotServer::from_config`]. The TLS
/// listeners are registered onto an existing hickory-server
/// [`Server<DnshubHandler>`] with [`DotServer::register_into`], so DoT shares
/// the same handler chain (policy, blocklists, forwarding) as UDP/TCP.
pub struct DotServer {
    listen: Vec<String>,
    tls_config: Arc<ServerConfig>,
}

impl DotServer {
    /// Build a [`DotServer`] from the `[server.tls]` config section.
    ///
    /// Reads and validates the cert/key files immediately so that startup
    /// fails fast with a clear error if the TLS material is missing or
    /// invalid, rather than failing lazily on the first handshake.
    pub fn from_config(tls: &TlsServerConfig) -> Result<Self, TlsLoadError> {
        let tls_config = load_tls_server_config(tls)?;
        info!(
            listen = ?tls.listen,
            cert = %tls.cert,
            "DoT server configured (TLS material loaded)",
        );
        Ok(Self {
            listen: tls.listen.clone(),
            tls_config,
        })
    }

    /// The listen addresses that will be bound by [`register_into`](Self::register_into).
    pub fn listen(&self) -> &[String] {
        &self.listen
    }

    /// The resolved rustls [`ServerConfig`] shared by all DoT listeners.
    pub fn tls_config(&self) -> &Arc<ServerConfig> {
        &self.tls_config
    }

    /// Bind a TLS listener for each configured address and register it with
    /// `server`, so DoT queries are dispatched to the same
    /// [`DnshubHandler`] as UDP/TCP.
    ///
    /// Returns the bound socket addresses in order. A bind failure for one
    /// address aborts registration of the remaining addresses and returns the
    /// [`std::io::Error`].
    pub async fn register_into(
        &self,
        server: &mut Server<DnshubHandler>,
    ) -> std::io::Result<Vec<SocketAddr>> {
        let mut bound = Vec::with_capacity(self.listen.len());
        for addr in &self.listen {
            let listener = TcpListener::bind(addr).await?;
            let local = listener.local_addr()?;
            server.register_tls_listener_with_tls_config(
                listener,
                TLS_HANDSHAKE_TIMEOUT,
                self.tls_config.clone(),
            )?;
            info!(addr = %local, "registered DoT (TLS) listener");
            bound.push(local);
        }
        Ok(bound)
    }
}

/// Log a TLS-load failure at `warn` level with full context.
///
/// Provided as a convenience for call sites that want to log the error before
/// exiting rather than formatting it themselves.
pub fn log_tls_load_error(error: &TlsLoadError) {
    warn!(error = %error, "failed to initialize DoT server");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TlsServerConfig;
    use std::path::PathBuf;

    /// A self-signed RSA test certificate (CN=dot.test.example) for unit
    /// tests. Embedded so the tests are self-contained and do not depend on
    /// fixture files that may be excluded from the repository by global
    /// gitignore rules (`*.pem`).
    const TEST_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDNDCCAhygAwIBAgIUJGeHZFWWI82DJ3a59XNrwfi8tYcwDQYJKoZIhvcNAQEL\n\
BQAwGzEZMBcGA1UEAwwQZG90LnRlc3QuZXhhbXBsZTAeFw0yNjA4MTYxMzIyMTda\n\
Fw0zNjA4MTMxMzIyMTdaMBsxGTAXBgNVBAMMEGRvdC50ZXN0LmV4YW1wbGUwggEi\n\
MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCv8uBpHHh9M3Jm1ZfaqQhgYRq9\n\
x4alTUEEaDpxc6jNYoOl/NTYk6c5HZUbVlaeDYmvbw9rcMS+9BM40IMbt3shjWMs\n\
1WoD0n8mXfcuEwfxtM+ugXlJ0zYc5i96yIvCK1/NnKs8iwLG+VXy+NKLdkqORwt9\n\
kjmpHe94SwxGQdY8/r5RNm2RkboXZ7PzSubCnIe0emVglskPbYXj6etE4N8OmJ+u\n\
BmvWnOwq0Dbi4hhKCmB286LrsEdJRc7dqlGG1M5qGdVx/uNFxNMLcOeHKJKas7OA\n\
jnWg6d1ofFGVS+S3ovR3YXK4DYZoB5PU01cmbVB0wCew6njs47DbiZCHftsrAgMB\n\
AAGjcDBuMB0GA1UdDgQWBBS4t1MzmIS+EpYsPg2gTs2zn/Ro+zAfBgNVHSMEGDAW\n\
gBS4t1MzmIS+EpYsPg2gTs2zn/Ro+zAPBgNVHRMBAf8EBTADAQH/MBsGA1UdEQQU\n\
MBKCEGRvdC50ZXN0LmV4YW1wbGUwDQYJKoZIhvcNAQELBQADggEBAFFql44khnNl\n\
8ytmQoD9r1pvWa4d6/AxmeTEO6xvA9Hb663OHjQnfHkrO7j5yslms4vunkt7zfJG\n\
7xRVIGZ1jfoa9IWLGhgxLuGPUKqqotYXrDTgbl/bF0WEhI08iksPRHfCmz28nGRK\n\
YpkQsjLLEkxvsXt9lUiT7UPllpQUC06wWSuPJIZyRGSR8GcZ/lQzY3LVmFvhFB75\n\
0u8A4no5nJVSb8B6l9wjwYR29H5dx9PPF3N+t+N4Lv+Tq6j2Iu2JwyKHqhJPxy7N\n\
z1BO9WMD0wsazRKqq7CLNT9Uf7iHfZ3Uib4jKbdmNA1lJMqSmmbxNXChZqu6jy0W\n\
TBaO+otDp3U=\n\
-----END CERTIFICATE-----\n";

    /// The matching RSA private key for [`TEST_CERT_PEM`].
    const TEST_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCv8uBpHHh9M3Jm\n\
1ZfaqQhgYRq9x4alTUEEaDpxc6jNYoOl/NTYk6c5HZUbVlaeDYmvbw9rcMS+9BM4\n\
0IMbt3shjWMs1WoD0n8mXfcuEwfxtM+ugXlJ0zYc5i96yIvCK1/NnKs8iwLG+VXy\n\
+NKLdkqORwt9kjmpHe94SwxGQdY8/r5RNm2RkboXZ7PzSubCnIe0emVglskPbYXj\n\
6etE4N8OmJ+uBmvWnOwq0Dbi4hhKCmB286LrsEdJRc7dqlGG1M5qGdVx/uNFxNML\n\
cOeHKJKas7OAjnWg6d1ofFGVS+S3ovR3YXK4DYZoB5PU01cmbVB0wCew6njs47Db\n\
iZCHftsrAgMBAAECggEAAnT9PKWnyOgkywxOsx7x770369BY0aZhAOCIxZyLuoOL\n\
cnnl74a9MD2PH1ogj0KjItWhdJSAxflYDD34pVYMHIZmWvedtKMtCmpVw0PGP11D\n\
PCJ5lymAHUuhxJ/ovczUTUQWgUhSRptiOJ1g/LBFc4/1dZjK7RPHuLENryF6ZzpR\n\
/fyb4Z004SUxgsiizTm6qhtGqCP7ZjvoJAp69jJ4m9en7UrZGSllFOvXUzScxPfe\n\
YmfKYvxESdjCkkihjjG7XoD/8oVpl8RftDFwNMUPCrdFqRPr9N8wNgebuEDEHhz0\n\
EgQLs/ZhOrdynGOTfVVu1UWQt+w5b1Gksisn6U2e2QKBgQDzhjciaWHbNlaR1zDO\n\
Drm333zHthCc7oL10hfK7cljAK5q73Yj1y72qeIf94sxqRixOUyvc2L4SF2jZYZ/\n\
2rQ95skUxGjjVe5uNWQBDgu+6toskeqGtni1zkCPuKtnShlQMIfpRnYlkuTaXdwt\n\
fhFFcSHouK3lRDrtWR2D2wYCpwKBgQC49msGnzq3BdlbU6EmpIjtoF6po4Kj6diE\n\
0ncIDr1uDZ80wkNU8ptEh5eVqN3lIOGRg1D8l+YV0nVdP6nOIicoKDDregoj5gtN\n\
Eib2INnkCPVuXTm/oGIJ0Tt5gwbQwofmXK9KnK7MJfU+cS9QB6vXLT89CH0w+DOp\n\
sXtvwGQH3QKBgQCsaSaZv2Bfsf3ibScJjBVin+CZCEaExLyFS4Q60NUWucHCxdyv\n\
jUabrjUBCuJKe3yW5IltYlT8kUdySovJ805O0RkmEdRst0cCUdyGfqpENcPXcEtJ\n\
quCVXvwIhOcdTrHTOzjOKGu3OGO8Ul1y++FAd9NZD39WZVMO/VvPIX8E2wKBgDo8\n\
xGgaXKdh/RUnWNdM+Rww4X1yUWEA8T6o8fekhHqRaW54ODEYDlFejBkASZWqa7ug\n\
aDCQN07prDCHKhUQZdncBcMu8uBov2gt7fyTTWfidjygt90hR50ltx9EZTH3/khH\n\
KJ5KhTMcRIK7qpT9RVsEESRLdvejPskQa/g80II1AoGAS84fNpXgkm2Gl6AIp8Xs\n\
nwemYC8ow1/yTxqL29/2R0HfYFYIRDDMZEK4meoRMZckSftH8j+xuazqa2XltFJR\n\
W329DwkiCRAgcRiITr0/D9TsAhyGcBxLlx7i8gL/1+H6qrIxTKkP+V8HDO5T32Wk\n\
A7Tb2wWBUbpPwg1gpzEziAk=\n\
-----END PRIVATE KEY-----\n";

    /// Write `content` to a uniquely-named file in the system temp dir and
    /// return its path. Each call uses a process-wide atomic counter so that
    /// parallel test threads do not collide on the same filename (a collision
    /// would let one thread observe a truncated/empty file mid-write).
    fn write_temp(name: &str, content: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "dnshub-dot-test-{}-{}-{}.pem",
            name,
            std::process::id(),
            seq,
        ));
        std::fs::write(&path, content).expect("write temp pem");
        path
    }

    /// A valid `[server.tls]` config pointing at the embedded test cert/key
    /// written to temp files.
    fn valid_tls_config() -> TlsServerConfig {
        let cert = write_temp("cert", TEST_CERT_PEM);
        let key = write_temp("key", TEST_KEY_PEM);
        TlsServerConfig {
            enabled: true,
            listen: vec!["0.0.0.0:853".to_string()],
            cert: cert.display().to_string(),
            key: key.display().to_string(),
        }
    }

    #[test]
    fn tls_config_parses_from_toml() {
        let toml = r#"
enabled = true
listen = ["0.0.0.0:853", "[::]:853"]
cert = "/etc/dnshub/tls/fullchain.pem"
key = "/etc/dnshub/tls/privkey.pem"
"#;
        let parsed: TlsServerConfig = toml::from_str(toml).expect("tls config should parse");
        assert!(parsed.enabled);
        assert_eq!(parsed.listen.len(), 2);
        assert_eq!(parsed.listen[0], "0.0.0.0:853");
        assert_eq!(parsed.listen[1], "[::]:853");
        assert_eq!(parsed.cert, "/etc/dnshub/tls/fullchain.pem");
        assert_eq!(parsed.key, "/etc/dnshub/tls/privkey.pem");
    }

    #[test]
    fn tls_config_disabled_parses() {
        // All fields are required by serde (no per-field defaults), so a
        // disabled config still specifies them — validation only enforces
        // non-empty cert/key/listen when `enabled = true`.
        let toml = r#"
enabled = false
listen = []
cert = ""
key = ""
"#;
        let parsed: TlsServerConfig =
            toml::from_str(toml).expect("disabled tls config should parse");
        assert!(!parsed.enabled);
        assert!(parsed.listen.is_empty());
        assert!(parsed.cert.is_empty());
        assert!(parsed.key.is_empty());
    }

    #[test]
    fn load_tls_server_config_succeeds_with_valid_cert() {
        let tls = valid_tls_config();
        let config =
            load_tls_server_config(&tls).expect("valid cert/key should produce a ServerConfig");
        // ALPN must advertise the DoT protocol identifier.
        assert_eq!(config.alpn_protocols, vec![b"dot".to_vec()]);
    }

    #[test]
    fn load_tls_server_config_missing_cert_file_is_graceful_error() {
        let mut tls = valid_tls_config();
        tls.cert = "/tmp/dnshub-dot-test-does-not-exist-cert-12345.pem".to_string();
        let err = load_tls_server_config(&tls).expect_err("missing cert should error");
        assert!(
            matches!(err, TlsLoadError::CertRead { .. }),
            "expected CertRead error, got {err:?}"
        );
        // Must not panic — the error must carry the path for the operator.
        assert!(err.to_string().contains("does-not-exist-cert"));
    }

    #[test]
    fn load_tls_server_config_missing_key_file_is_graceful_error() {
        let mut tls = valid_tls_config();
        tls.key = "/tmp/dnshub-dot-test-does-not-exist-key-12345.pem".to_string();
        let err = load_tls_server_config(&tls).expect_err("missing key should error");
        assert!(
            matches!(err, TlsLoadError::KeyRead { .. }),
            "expected KeyRead error, got {err:?}"
        );
        assert!(err.to_string().contains("does-not-exist-key"));
    }

    #[test]
    fn load_tls_server_config_empty_cert_chain_is_error() {
        // A PEM file with no certificates parses but yields an empty chain.
        let empty = write_temp("empty", "# empty pem file\n");
        let mut tls = valid_tls_config();
        tls.cert = empty.display().to_string();
        let err = load_tls_server_config(&tls).expect_err("empty cert chain should error");
        assert!(
            matches!(err, TlsLoadError::EmptyCertChain { .. }),
            "expected EmptyCertChain error, got {err:?}"
        );
    }

    #[test]
    fn load_tls_server_config_key_cert_mismatch_is_error() {
        // Pointing the key at a *certificate* PEM yields a key parse failure
        // (a cert PEM is not a private key PEM).
        let mut tls = valid_tls_config();
        tls.key = tls.cert.clone();
        let err = load_tls_server_config(&tls).expect_err("cert-as-key should error");
        // The pki parser rejects a certificate PEM as a private key.
        assert!(
            matches!(err, TlsLoadError::KeyRead { .. } | TlsLoadError::KeyMismatch { .. }),
            "expected key parse error, got {err:?}"
        );
    }

    #[test]
    fn dot_server_from_config_exposes_listen_and_config() {
        let tls = valid_tls_config();
        let dot = DotServer::from_config(&tls).expect("valid config should build DotServer");
        assert_eq!(dot.listen(), &["0.0.0.0:853"]);
        // The shared ServerConfig advertises the dot ALPN.
        assert_eq!(dot.tls_config().alpn_protocols, vec![b"dot".to_vec()]);
    }

    #[tokio::test]
    async fn dot_server_registers_tls_listener_on_loopback() {
        use crate::dns::DnshubHandler;
        use hickory_server::server::Server;
        use hickory_server::zone_handler::Catalog;

        // Bind to an ephemeral loopback port so the test does not require
        // privilege or collide with other tests.
        let mut tls = valid_tls_config();
        tls.listen = vec!["127.0.0.1:0".to_string()];
        let dot = DotServer::from_config(&tls).expect("valid config should build DotServer");

        let handler = DnshubHandler::from_catalog(Catalog::new());
        let mut server = Server::new(handler);
        let bound = dot
            .register_into(&mut server)
            .await
            .expect("TLS listener should bind on loopback");
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].ip().to_string(), "127.0.0.1");
        // Drop `server` to cancel the spawned listener task.
        server.shutdown_token().cancel();
    }
}
