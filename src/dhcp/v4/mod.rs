//! DHCPv4 server: UDP listener on port 67, message dispatch.
//!
//! The server binds a UDP socket on the configured listen address
//! (default `0.0.0.0:67`), reads incoming DHCP messages, decodes them
//! with `dhcproto`, dispatches to the [`DhcpStateMachine`], and sends
//! the reply back to the client.
//!
//! ## Broadcast handling
//!
//! DHCPDISCOVER is typically broadcast. The server replies with a
//! DHCPOFFER sent to the broadcast address (when the broadcast flag is
//! set) or to the offered IP (unicast). The reply destination is
//! determined by [`DhcpV4Server::reply_dest`].

pub mod config;
pub mod lease_store;
pub mod options;
pub mod pool;
pub mod state_machine;

use std::net::SocketAddr;
use std::sync::Arc;

use dhcproto::{Decoder, Decodable, Encoder, Encodable};
use dhcproto::v4::{Message, Opcode, SERVER_PORT};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use self::config::DhcpV4Config;
use self::lease_store::{LeaseStoreV4, SqliteLeaseStoreV4};
use self::pool::PoolAllocator;
use self::state_machine::{DhcpStateMachine, HandleOutcome};

/// DHCPv4 server.
///
/// Holds the config, lease store, and pool allocator. The server is
/// started with [`DhcpV4Server::run`], which loops until the
/// `CancellationToken` is cancelled.
pub struct DhcpV4Server {
    config: Arc<DhcpV4Config>,
    store: Arc<dyn LeaseStoreV4>,
    allocator: PoolAllocator,
    /// The server's own IP address (used as SIADDR / server identifier).
    server_ip: std::net::Ipv4Addr,
}

impl DhcpV4Server {
    /// Create a new DHCPv4 server with the given config and lease store.
    pub fn new(
        config: Arc<DhcpV4Config>,
        store: Arc<dyn LeaseStoreV4>,
        server_ip: std::net::Ipv4Addr,
    ) -> Self {
        Self {
            config,
            store,
            allocator: PoolAllocator::default(),
            server_ip,
        }
    }

    /// Create a new DHCPv4 server with a custom pool allocator (for
    /// tests or when ping should be disabled).
    pub fn with_allocator(
        config: Arc<DhcpV4Config>,
        store: Arc<dyn LeaseStoreV4>,
        allocator: PoolAllocator,
        server_ip: std::net::Ipv4Addr,
    ) -> Self {
        Self {
            config,
            store,
            allocator,
            server_ip,
        }
    }

    /// Run the server: bind the UDP socket and process messages until
    /// `cancel` is triggered.
    pub async fn run(
        self,
        listen: SocketAddr,
        cancel: CancellationToken,
    ) -> std::io::Result<()> {
        let socket = UdpSocket::bind(listen).await?;
        tracing::info!(%listen, "dhcpv4 server listening");

        let Self {
            config,
            store,
            allocator,
            server_ip,
        } = self;

        let mut buf = vec![0u8; 1500];
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("dhcpv4 server shutting down");
                    break;
                }
                result = socket.recv_from(&mut buf) => {
                    let (n, src) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!(?e, "dhcpv4: recv_from error");
                            continue;
                        }
                    };
                    let data = &buf[..n];
                    if let Err(e) = handle_packet(
                        data, src, &socket, &config, &*store, &allocator, server_ip,
                    ).await {
                        tracing::error!(?e, "dhcpv4: error handling packet");
                    }
                }
            }
        }
        Ok(())
    }

    /// Convenience: open a [`SqliteLeaseStoreV4`] and run the server.
    pub async fn run_with_sqlite(
        config: DhcpV4Config,
        db_path: &std::path::Path,
        server_ip: std::net::Ipv4Addr,
        cancel: CancellationToken,
    ) -> std::io::Result<()> {
        let store = Arc::new(SqliteLeaseStoreV4::open(db_path).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })?);
        let listen: SocketAddr = config
            .listen
            .parse()
            .unwrap_or_else(|_| {
                SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, SERVER_PORT))
            });
        let config = Arc::new(config);
        let server = DhcpV4Server::new(config, store, server_ip);
        server.run(listen, cancel).await
    }
}

// -- free functions -----------------------------------------------------

/// Handle a single received packet: decode, dispatch, send reply.
async fn handle_packet(
    data: &[u8],
    src: SocketAddr,
    socket: &UdpSocket,
    config: &DhcpV4Config,
    store: &dyn LeaseStoreV4,
    allocator: &PoolAllocator,
    server_ip: std::net::Ipv4Addr,
) -> Result<(), Box<dyn std::error::Error>> {
    let msg = match Message::decode(&mut Decoder::new(data)) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(?e, "dhcpv4: failed to decode message, ignoring");
            return Ok(());
        }
    };

    // Only process BootRequest messages.
    if msg.opcode() != Opcode::BootRequest {
        tracing::debug!("dhcpv4: ignoring non-BootRequest message");
        return Ok(());
    }

    let sm = DhcpStateMachine::new(config, store, allocator, server_ip);
    let outcome = sm.handle(&msg).await;
    match outcome {
        HandleOutcome::Reply(reply) => {
            let dest = reply_dest(&msg, src);
            let mut buf = Vec::new();
            let mut enc = Encoder::new(&mut buf);
            reply.encode(&mut enc)?;
            socket.send_to(&buf, dest).await?;
        }
        HandleOutcome::NoReply => {}
        HandleOutcome::Error(e) => {
            tracing::error!(%e, "dhcpv4: state machine error");
        }
    }
    Ok(())
}

/// Determine the reply destination for a DHCP message.
///
/// Per RFC 2131 §4.1:
/// - If the broadcast flag is set, reply to the broadcast address
///   (`255.255.255.255:68`).
/// - Otherwise, reply to the client's IP (unicast) on port 68.
/// - For relayed messages (giaddr non-zero), reply to the relay agent
///   on port 67.
fn reply_dest(msg: &Message, src: SocketAddr) -> SocketAddr {
    // If relayed, reply to the relay agent (giaddr:67).
    let giaddr = msg.giaddr();
    if !giaddr.is_unspecified() {
        return SocketAddr::from((giaddr, SERVER_PORT));
    }

    // If broadcast flag is set, broadcast to :68.
    if msg.flags().broadcast() {
        return SocketAddr::from((std::net::Ipv4Addr::BROADCAST, 68));
    }

    // If ciaddr is set (RENEWING/REBINDING), unicast to ciaddr:68.
    let ciaddr = msg.ciaddr();
    if !ciaddr.is_unspecified() {
        return SocketAddr::from((ciaddr, 68));
    }

    // Fall back to the source address of the request.
    src
}

#[cfg(test)]
mod tests {
    use super::*;
    use dhcproto::v4::Flags;

    #[test]
    fn reply_dest_broadcast_flag() {
        let mut msg = Message::default();
        msg.set_flags(Flags::default().set_broadcast());
        let src: SocketAddr = "192.168.1.1:68".parse().unwrap();
        let dest = reply_dest(&msg, src);
        assert_eq!(dest.port(), 68);
        assert_eq!(dest.ip(), std::net::IpAddr::V4(std::net::Ipv4Addr::BROADCAST));
    }

    #[test]
    fn reply_dest_relayed() {
        let mut msg = Message::default();
        msg.set_giaddr(std::net::Ipv4Addr::new(10, 0, 0, 1));
        let src: SocketAddr = "192.168.1.1:67".parse().unwrap();
        let dest = reply_dest(&msg, src);
        assert_eq!(dest.port(), SERVER_PORT);
        assert_eq!(dest.ip(), std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn reply_dest_unicast_ciaddr() {
        let mut msg = Message::default();
        msg.set_ciaddr(std::net::Ipv4Addr::new(192, 168, 1, 50));
        let src: SocketAddr = "192.168.1.1:68".parse().unwrap();
        let dest = reply_dest(&msg, src);
        assert_eq!(dest.port(), 68);
        assert_eq!(dest.ip(), std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 50)));
    }

    #[test]
    fn reply_dest_fallback_to_src() {
        let msg = Message::default();
        let src: SocketAddr = "192.168.1.99:68".parse().unwrap();
        let dest = reply_dest(&msg, src);
        assert_eq!(dest, src);
    }
}
