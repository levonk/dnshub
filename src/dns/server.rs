//! hickory-server `Server` setup (UDP + TCP + DoT + DoH).
//!
//! In hickory-server 0.26 the old `ServerFuture` was renamed to
//! [`Server`](hickory_server::server::Server). [`DnshubServer`] owns the
//! `Server<DnshubHandler>`, registers bound UDP/TCP sockets (and, when TLS is
//! enabled, DoT TLS listeners via [`DotServer`](crate::dns::dot::DotServer),
//! or DoH HTTPS listeners via [`DohServer`](crate::dns::doh::DohServer)),
//! and drives `block_until_done`. Graceful shutdown is triggered via the
//! server's shutdown token (wired to Ctrl+C in [`crate`]'s `main`).

use crate::config::ServerConfig;
use crate::dns::doh::DohServer;
use crate::dns::dot::DotServer;
use crate::dns::DnshubHandler;
use crate::dns::socket::{SocketConfig, bind_tcp, bind_udp};
use hickory_server::server::Server;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// TCP request timeout and per-connection response buffer size.
const TCP_TIMEOUT: Duration = Duration::from_secs(5);
const TCP_RESPONSE_BUFFER_SIZE: usize = 512;

/// Wraps a hickory-server [`Server`] bound to UDP and TCP sockets.
pub struct DnshubServer {
    server: Server<DnshubHandler>,
}

impl DnshubServer {
    /// Create a new server for the given handler. No sockets are registered yet.
    pub fn new(handler: DnshubHandler) -> Self {
        Self {
            server: Server::new(handler),
        }
    }

    /// Bind a UDP socket to `addr` with performance tuning from `config`
    /// (SO_REUSEPORT, SO_RCVBUF, SO_SNDBUF) and register it with the server.
    pub async fn register_udp(
        &mut self,
        addr: &str,
        config: &ServerConfig,
    ) -> std::io::Result<SocketAddr> {
        let socket_config = SocketConfig::for_udp(config.reuse_port, config.udp_buffer_size);
        let (socket, bound) = bind_udp(addr, &socket_config).await?;
        self.server.register_socket(socket);
        info!(addr = %bound, "registered UDP listener");
        Ok(bound)
    }

    /// Bind a TCP listener to `addr` with performance tuning from `config`
    /// (SO_REUSEPORT, SO_RCVBUF, SO_SNDBUF, SO_KEEPALIVE) and register it
    /// with the server.
    pub async fn register_tcp(
        &mut self,
        addr: &str,
        config: &ServerConfig,
    ) -> std::io::Result<SocketAddr> {
        let socket_config = SocketConfig::for_tcp(
            config.reuse_port,
            config.tcp_buffer_size,
            config.tcp_keepalive_secs,
        );
        let (listener, bound) = bind_tcp(addr, &socket_config).await?;
        self.server
            .register_listener(listener, TCP_TIMEOUT, TCP_RESPONSE_BUFFER_SIZE);
        info!(
            addr = %bound,
            max_connections = config.max_tcp_connections,
            "registered TCP listener"
        );
        Ok(bound)
    }

    /// Register DoT (DNS-over-TLS) listeners from a pre-built [`DotServer`].
    ///
    /// The TLS listeners are registered on the *same* underlying
    /// [`Server<DnshubHandler>`] as the UDP/TCP listeners, so DoT clients are
    /// served by the identical handler chain (policy, blocklists, forwarding).
    /// Returns the bound socket addresses in order.
    ///
    /// Call this only when `[server.tls].enabled` is `true` and the cert/key
    /// files are readable — [`DotServer::from_config`] fails fast on invalid
    /// TLS material so the caller can exit gracefully with a clear error.
    pub async fn register_tls(
        &mut self,
        dot: &DotServer,
    ) -> std::io::Result<Vec<SocketAddr>> {
        dot.register_into(&mut self.server).await
    }

    /// Register the DoH (HTTPS) listeners from a configured [`DohServer`].
    ///
    /// The HTTPS listeners are attached to the same hickory-server [`Server`]
    /// so they share the [`DnshubHandler`] (and therefore the same per-client
    /// policy, blocklists, and forwarding) as the UDP/TCP listeners. Returns
    /// the bound DoH socket addresses.
    pub async fn register_doh(
        &mut self,
        doh: &DohServer,
    ) -> Result<Vec<SocketAddr>, crate::dns::doh::DohError> {
        doh.register(&mut self.server).await
    }

    /// Run the server until `shutdown` is cancelled, then shut down gracefully.
    pub async fn run_until(self, shutdown: CancellationToken) {
        let mut server = self.server;
        // `shutdown_token()` returns a reference to the server's internal
        // cancellation token; clone it so we can cancel without borrowing.
        let server_shutdown_token = server.shutdown_token().clone();

        // Drive the server in the background.
        let server_task = tokio::spawn(async move {
            if let Err(e) = server.block_until_done().await {
                warn!(error = %e, "server future completed with error");
            }
        });

        // Wait for a shutdown signal, then trigger graceful shutdown.
        shutdown.cancelled().await;
        info!("shutdown signal received, stopping DNS server");
        server_shutdown_token.cancel();
        server_task
            .await
            .unwrap_or_else(|e| warn!(error = %e, "server task panicked"));
    }
}
