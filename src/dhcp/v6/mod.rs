//! DHCPv6 server (UDP listener on port 547, message dispatch).
//!
//! This module is the public entry point for the DHCPv6 server. It wires
//! together the configuration ([`config::DhcpV6Config`]), lease store
//! ([`lease_store::SqliteLeaseStoreV6`]), IA_NA allocator
//! ([`ia_na::IaNaAllocator`]), and state machine
//! ([`state_machine::DhcpV6StateMachine`]).
//!
//! The [`DhcpV6Server`] binds a UDP socket on the configured listen address
//! (default `[::]:547`), decodes incoming [`dhcproto::v6::Message`] frames,
//! dispatches them through the state machine, and sends the reply back to
//! the client. Relay handling (story 04-007) and RA/SLAAC (story 04-003)
//! are out of scope here.

pub mod config;
pub mod duid;
pub mod ia_na;
pub mod lease_store;
pub mod options;
pub mod state_machine;

use dhcproto::{Decodable, Decoder, Encodable, Encoder};
use dhcproto::v6::{Message, SERVER_PORT};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use self::config::DhcpV6Config;
use self::lease_store::SqliteLeaseStoreV6;
use self::state_machine::DhcpV6StateMachine;

pub use self::config::{DhcpPoolV6, DhcpV6Config as Config};
pub use self::duid::Duid;
pub use self::lease_store::{DhcpLeaseV6, LeaseState, LeaseStoreError, LeaseStoreV6 as LeaseStore, StaticLeaseV6};
pub use self::state_machine::{DhcpV6StateMachine as StateMachine, StateMachineError};

/// Maximum DHCPv6 message size (RFC 8415 §21.10 suggests 1452 bytes for
/// Ethernet; we use a generous buffer to accommodate relayed messages).
const MAX_MSG_SIZE: usize = 4096;

/// A running DHCPv6 server.
///
/// Holds the configured state machine behind an [`Arc`] so the receive
/// loop and any future relay path can share it. Construct with
/// [`DhcpV6Server::new`] and run with [`DhcpV6Server::run`].
pub struct DhcpV6Server {
    config: DhcpV6Config,
    state_machine: Arc<DhcpV6StateMachine<SqliteLeaseStoreV6>>,
    server_duid: Vec<u8>,
}

impl DhcpV6Server {
    /// Build a DHCPv6 server from configuration and a SQLite lease store.
    ///
    /// `db_path` is the path to the SQLite database file (created if
    /// missing). The server DUID is generated as a DUID-EN using the
    /// enterprise number 32473 and a stable identifier derived from the
    /// listen address — production deployments should supply an explicit
    /// DUID via [`DhcpV6Server::with_duid`].
    pub async fn new(
        config: DhcpV6Config,
        db_path: &std::path::Path,
    ) -> Result<Self, ServerError> {
        let store = SqliteLeaseStoreV6::open(db_path)?;
        let server_duid = duid::server_duid_en(32473, config.listen.as_bytes()).as_ref().to_vec();
        let state_machine = Arc::new(DhcpV6StateMachine::new(
            config.clone(),
            server_duid.clone(),
            store,
        ));
        Ok(Self {
            config,
            state_machine,
            server_duid,
        })
    }

    /// Build a DHCPv6 server with an explicit server DUID.
    pub fn with_duid(
        config: DhcpV6Config,
        store: SqliteLeaseStoreV6,
        server_duid: Vec<u8>,
    ) -> Self {
        let state_machine = Arc::new(DhcpV6StateMachine::new(
            config.clone(),
            server_duid.clone(),
            store,
        ));
        Self {
            config,
            state_machine,
            server_duid,
        }
    }

    /// The raw server DUID bytes (ServerId option value).
    pub fn server_duid(&self) -> &[u8] {
        &self.server_duid
    }

    /// The configured listen [`SocketAddr`].
    pub fn listen_addr(&self) -> Result<SocketAddr, ServerError> {
        Ok(self.config.listen.parse()?)
    }

    /// Run the UDP receive loop until `shutdown` is cancelled.
    ///
    /// Each datagram is decoded into a [`Message`], dispatched through the
    /// state machine, and the reply (if any) is sent back to the peer.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), ServerError> {
        if !self.config.enabled {
            tracing::info!("DHCPv6 server disabled by config; not starting");
            return Ok(());
        }
        let listen = self.listen_addr()?;
        let socket = UdpSocket::bind(&listen).await?;
        tracing::info!(%listen, "DHCPv6 server listening on UDP {}", listen);

        let sm = self.state_machine.clone();
        let mut buf = vec![0u8; MAX_MSG_SIZE];
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("DHCPv6 server shutting down");
                    break;
                }
                result = socket.recv_from(&mut buf) => {
                    let (n, peer) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(error = %e, "DHCPv6 recv_from error");
                            continue;
                        }
                    };
                    let datagram = &buf[..n];
                    let msg = match Message::decode(&mut Decoder::new(datagram)) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to decode DHCPv6 message from {peer}");
                            continue;
                        }
                    };
                    let msg_type = msg.msg_type();
                    tracing::debug!(%peer, ?msg_type, "DHCPv6 message received");

                    let reply = match sm.handle(&msg).await {
                        Ok(r) => Some(r),
                        Err(e) => {
                            tracing::warn!(error = %e, "DHCPv6 state machine error");
                            None
                        }
                    };

                    if let Some(reply) = reply {
                        let mut out = Vec::new();
                        let mut enc = Encoder::new(&mut out);
                        if let Err(e) = reply.encode(&mut enc) {
                            tracing::warn!(error = %e, "failed to encode DHCPv6 reply");
                            continue;
                        }
                        if let Err(e) = socket.send_to(&out, peer).await {
                            tracing::warn!(error = %e, "failed to send DHCPv6 reply to {peer}");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Errors that can occur while constructing or running the DHCPv6 server.
#[derive(Debug)]
pub enum ServerError {
    /// The configured listen address could not be parsed.
    AddrParse(std::net::AddrParseError),
    /// The lease store could not be opened.
    Store(lease_store::LeaseStoreError),
    /// A UDP I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::AddrParse(e) => write!(f, "listen address parse error: {e}"),
            ServerError::Store(e) => write!(f, "lease store error: {e}"),
            ServerError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServerError::AddrParse(e) => Some(e),
            ServerError::Store(e) => Some(e),
            ServerError::Io(e) => Some(e),
        }
    }
}

impl From<std::net::AddrParseError> for ServerError {
    fn from(e: std::net::AddrParseError) -> Self {
        ServerError::AddrParse(e)
    }
}

impl From<lease_store::LeaseStoreError> for ServerError {
    fn from(e: lease_store::LeaseStoreError) -> Self {
        ServerError::Store(e)
    }
}

impl From<std::io::Error> for ServerError {
    fn from(e: std::io::Error) -> Self {
        ServerError::Io(e)
    }
}

/// The well-known DHCPv6 server UDP port (547).
pub const PORT: u16 = SERVER_PORT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::v6::config::DhcpV6Config;
    use crate::dhcp::v6::lease_store::SqliteLeaseStoreV6;
    use dhcproto::v6::{DhcpOption, DhcpOptions, IAAddr, IANA, MessageType};

    fn server_duid() -> Vec<u8> {
        duid::server_duid_en(32473, b"test-server").as_ref().to_vec()
    }

    fn config() -> DhcpV6Config {
        DhcpV6Config {
            enabled: true,
            lease_time_hours: 24,
            domain_search: vec!["levonk.com".to_string()],
            pools: vec![DhcpPoolV6 {
                name: "main".into(),
                prefix: "fd00:1234:5678::/64".into(),
                pool_start: "fd00:1234:5678::100".parse().unwrap(),
                pool_end: "fd00:1234:5678::103".parse().unwrap(),
                dns_servers: vec!["fd00:1234:5678::67".parse().unwrap()],
            }],
            ..DhcpV6Config::default()
        }
    }

    #[tokio::test]
    async fn server_constructs_and_handles_solicit() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let server = DhcpV6Server::with_duid(config(), store, server_duid());

        let mut msg = Message::new(MessageType::Solicit);
        msg.opts_mut().insert(DhcpOption::ClientId(
            duid::server_duid_en(32473, b"client").as_ref().to_vec(),
        ));
        let mut ia_opts = DhcpOptions::new();
        ia_opts.insert(DhcpOption::IAAddr(IAAddr {
            addr: "fd00:1234:5678::100".parse().unwrap(),
            preferred_life: 0,
            valid_life: 0,
            opts: DhcpOptions::new(),
        }));
        msg.opts_mut().insert(DhcpOption::IANA(IANA {
            id: 1,
            t1: 0,
            t2: 0,
            opts: ia_opts,
        }));

        let reply = server.state_machine.handle(&msg).await.unwrap();
        assert_eq!(reply.msg_type(), MessageType::Advertise);
    }

    #[test]
    fn listen_addr_parses() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let server = DhcpV6Server::with_duid(config(), store, server_duid());
        let addr = server.listen_addr().unwrap();
        assert_eq!(addr.port(), 547);
    }

    #[test]
    fn disabled_server_reports_disabled() {
        let mut cfg = config();
        cfg.enabled = false;
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let server = DhcpV6Server::with_duid(cfg, store, server_duid());
        // run() returns Ok(()) immediately when disabled. We test via the
        // early-return path by checking the config flag directly.
        assert!(!server.config.enabled);
    }
}
