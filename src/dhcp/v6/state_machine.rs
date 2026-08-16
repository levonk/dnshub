//! DHCPv6 server state machine.
//!
//! Implements the server side of the RFC 8415 message exchanges:
//!
//! | Client → Server        | Server → Client  | Handler                  |
//! |------------------------|------------------|--------------------------|
//! | SOLLICIT               | ADVERTISE        | [`handle_solicit`]       |
//! | REQUEST                | REPLY            | [`handle_request`]       |
//! | RENEW                  | REPLY            | [`handle_renew`]         |
//! | REBIND                 | REPLY            | [`handle_rebind`]        |
//! | RELEASE                | REPLY            | [`handle_release`]       |
//! | CONFIRM                | REPLY            | [`handle_confirm`]       |
//! | INFORMATION-REQUEST    | REPLY            | [`handle_information`]   |
//!
//! The state machine is stateless beyond the lease store: each call is a
//! pure function of the incoming [`Message`] and the persisted lease state.
//! T1/T2 timers and lifetimes come from [`DhcpV6Config`].

use dhcproto::v6::{DhcpOption, Message, MessageType, OptionCode, StatusCode, Status};
use std::net::Ipv6Addr;

use super::config::{DhcpPoolV6, DhcpV6Config};
use super::duid;
use super::ia_na::{IaNaAllocator, IaNaOutcome};
use super::lease_store::{LeaseStoreError, LeaseStoreV6};
use super::options;

/// Errors produced by the state machine.
#[derive(Debug)]
pub enum StateMachineError {
    /// The incoming message was missing a required option (e.g. ClientId).
    MissingOption(&'static str),
    /// The ServerId in the message did not match this server's DUID.
    ServerIdMismatch,
    /// The lease store returned an error.
    Store(LeaseStoreError),
    /// No pool is configured.
    NoPool,
}

impl std::fmt::Display for StateMachineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateMachineError::MissingOption(name) => {
                write!(f, "missing required option: {name}")
            }
            StateMachineError::ServerIdMismatch => write!(f, "server DUID mismatch"),
            StateMachineError::Store(e) => write!(f, "lease store error: {e}"),
            StateMachineError::NoPool => write!(f, "no DHCPv6 pool configured"),
        }
    }
}

impl std::error::Error for StateMachineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateMachineError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LeaseStoreError> for StateMachineError {
    fn from(e: LeaseStoreError) -> Self {
        StateMachineError::Store(e)
    }
}

/// RFC 8415 DHCPv6 server state machine.
pub struct DhcpV6StateMachine<S: LeaseStoreV6> {
    config: DhcpV6Config,
    server_duid: Vec<u8>,
    allocator: IaNaAllocator<S>,
}

impl<S: LeaseStoreV6> DhcpV6StateMachine<S> {
    /// Create a new state machine.
    ///
    /// `server_duid` is the raw DUID bytes this server advertises in the
    /// ServerId option (see [`duid::server_duid_llt`]).
    pub fn new(config: DhcpV6Config, server_duid: Vec<u8>, store: S) -> Self {
        let allocator = IaNaAllocator::new(store, config.clone());
        Self {
            config,
            server_duid,
            allocator,
        }
    }

    /// Borrow the configured pools.
    pub fn config(&self) -> &DhcpV6Config {
        &self.config
    }

    /// The first configured pool (homelab single-pool default). Returns
    /// [`StateMachineError::NoPool`] if none are configured.
    fn first_pool(&self) -> Result<&DhcpPoolV6, StateMachineError> {
        self.config
            .pools
            .first()
            .ok_or(StateMachineError::NoPool)
    }

    /// Dispatch an incoming message to the appropriate handler.
    pub async fn handle(&self, msg: &Message) -> Result<Message, StateMachineError> {
        match msg.msg_type() {
            MessageType::Solicit => self.handle_solicit(msg).await,
            MessageType::Request => self.handle_request(msg).await,
            MessageType::Renew => self.handle_renew(msg).await,
            MessageType::Rebind => self.handle_rebind(msg).await,
            MessageType::Release => self.handle_release(msg).await,
            MessageType::Confirm => self.handle_confirm(msg).await,
            MessageType::InformationRequest => self.handle_information(msg).await,
            // Unsupported message types get a minimal Reply with an error.
            other => Ok(self.error_reply(msg, Status::UnspecFail, &format!("unsupported message type {other:?}"))),
        }
    }

    /// Extract the ClientId (option 1) from a message.
    fn client_id<'a>(msg: &'a Message) -> Option<&'a [u8]> {
        match msg.opts().get(OptionCode::ClientId)? {
            DhcpOption::ClientId(bytes) => Some(bytes.as_slice()),
            _ => None,
        }
    }

    /// Extract the ServerId (option 2) from a message.
    fn server_id(msg: &Message) -> Option<&[u8]> {
        match msg.opts().get(OptionCode::ServerId)? {
            DhcpOption::ServerId(bytes) => Some(bytes.as_slice()),
            _ => None,
        }
    }

    /// Extract the first IA_NA (option 3) from a message.
    fn first_ia_na(msg: &Message) -> Option<&dhcproto::v6::IANA> {
        match msg.opts().get(OptionCode::IANA)? {
            DhcpOption::IANA(ia) => Some(ia),
            _ => None,
        }
    }

    /// Extract the FQDN or hostname requested by the client. DHCPv6 carries
    /// the hostname in the FQDN option (option 39) — not implemented here,
    /// so we return `None`. The lease `hostname` column is left nullable.
    fn client_hostname(_msg: &Message) -> Option<&str> {
        None
    }

    /// Verify the message's ServerId matches this server's DUID. Returns
    /// `Ok(())` if it matches or if the message has no ServerId (SOLLICIT,
    /// INFORMATION-REQUEST). Returns an error for explicit mismatches.
    fn check_server_id(&self, msg: &Message) -> Result<(), StateMachineError> {
        match Self::server_id(msg) {
            None => Ok(()),
            Some(id) if id == self.server_duid.as_slice() => Ok(()),
            Some(_) => Err(StateMachineError::ServerIdMismatch),
        }
    }

    /// Build the base reply: same xid, ServerId = ours, ClientId = theirs.
    fn base_reply(&self, msg: &Message, msg_type: MessageType) -> Result<Message, StateMachineError> {
        let client_id = Self::client_id(msg)
            .ok_or(StateMachineError::MissingOption("ClientId"))?
            .to_vec();
        let mut reply = Message::new_with_id(msg_type, msg.xid());
        reply.opts_mut().insert(DhcpOption::ServerId(self.server_duid.clone()));
        reply.opts_mut().insert(DhcpOption::ClientId(client_id));
        Ok(reply)
    }

    /// Append the standard server options (DNS, domain search, NTP, info
    /// refresh) to a reply, drawn from the pool/config.
    fn add_server_options(&self, reply: &mut Message, pool: &DhcpPoolV6) {
        if !pool.dns_servers.is_empty() {
            reply.opts_mut().insert(options::dns_servers(&pool.dns_servers));
        }
        if !self.config.domain_search.is_empty() {
            reply
                .opts_mut()
                .insert(options::domain_search_list(&self.config.domain_search));
        }
        if let Some(ntp) = self.config.ntp_server {
            reply.opts_mut().insert(options::ntp_server(ntp));
        }
    }

    fn error_reply(&self, msg: &Message, status: Status, message: &str) -> Message {
        let mut reply = Message::new_with_id(MessageType::Reply, msg.xid());
        reply.opts_mut().insert(DhcpOption::ServerId(self.server_duid.clone()));
        if let Some(id) = Self::client_id(msg) {
            reply.opts_mut().insert(DhcpOption::ClientId(id.to_vec()));
        }
        reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
            status,
            msg: message.to_string(),
        }));
        reply
    }

    // -----------------------------------------------------------------------
    // Handlers
    // -----------------------------------------------------------------------

    /// SOLLICIT → ADVERTISE (RFC 8415 §18.2.1).
    pub async fn handle_solicit(&self, msg: &Message) -> Result<Message, StateMachineError> {
        let client_id = Self::client_id(msg)
            .ok_or(StateMachineError::MissingOption("ClientId"))?;
        let duid_hex = duid::Duid::parse(client_id).to_hex();
        let pool = self.first_pool()?;
        let hostname = Self::client_hostname(msg).map(|s| s.to_string());

        let ia = match Self::first_ia_na(msg) {
            Some(ia) => ia,
            None => {
                // No IA_NA: respond with Advertise + status NoAddrsAvail.
                let mut reply = self.base_reply(msg, MessageType::Advertise)?;
                reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
                    status: Status::NoAddrsAvail,
                    msg: "no IA_NA option in solicit".to_string(),
                }));
                return Ok(reply);
            }
        };

        let outcome = self
            .allocator
            .allocate(pool, &duid_hex, ia.id, hostname.as_deref(), false)
            .await?;
        let mut reply = self.base_reply(msg, MessageType::Advertise)?;
        reply
            .opts_mut()
            .insert(DhcpOption::IANA(self.allocator.build_ia_na(ia.id, &outcome)));
        self.add_server_options(&mut reply, pool);
        Ok(reply)
    }

    /// REQUEST → REPLY (RFC 8415 §18.2.2).
    pub async fn handle_request(&self, msg: &Message) -> Result<Message, StateMachineError> {
        self.check_server_id(msg)?;
        let client_id = Self::client_id(msg)
            .ok_or(StateMachineError::MissingOption("ClientId"))?;
        let duid_hex = duid::Duid::parse(client_id).to_hex();
        let pool = self.first_pool()?;
        let hostname = Self::client_hostname(msg).map(|s| s.to_string());

        let ia = match Self::first_ia_na(msg) {
            Some(ia) => ia,
            None => {
                let mut reply = self.base_reply(msg, MessageType::Reply)?;
                reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
                    status: Status::NoAddrsAvail,
                    msg: "no IA_NA option in request".to_string(),
                }));
                return Ok(reply);
            }
        };

        let outcome = self
            .allocator
            .allocate(pool, &duid_hex, ia.id, hostname.as_deref(), true)
            .await?;
        let mut reply = self.base_reply(msg, MessageType::Reply)?;
        reply
            .opts_mut()
            .insert(DhcpOption::IANA(self.allocator.build_ia_na(ia.id, &outcome)));
        reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
            status: Status::Success,
            msg: "lease granted".to_string(),
        }));
        self.add_server_options(&mut reply, pool);
        Ok(reply)
    }

    /// RENEW → REPLY (RFC 8415 §18.2.3).
    pub async fn handle_renew(&self, msg: &Message) -> Result<Message, StateMachineError> {
        self.check_server_id(msg)?;
        let client_id = Self::client_id(msg)
            .ok_or(StateMachineError::MissingOption("ClientId"))?;
        let duid_hex = duid::Duid::parse(client_id).to_hex();
        let pool = self.first_pool()?;

        let ia = match Self::first_ia_na(msg) {
            Some(ia) => ia,
            None => {
                return Ok(self.error_reply(msg, Status::NoBinding, "no IA_NA in renew"));
            }
        };

        // The client's existing address is carried inside the IA_NA as an
        // IAAddr option (RFC 8415 §18.2.3). Without it we cannot identify
        // the binding to renew, so we return NoBinding.
        let Some(addr) = ia_addr_from_ia(ia) else {
            let mut reply = self.base_reply(msg, MessageType::Reply)?;
            reply
                .opts_mut()
                .insert(DhcpOption::IANA(self.allocator.build_ia_na(ia.id, &IaNaOutcome::NoBinding)));
            return Ok(reply);
        };

        let outcome = self.allocator.renew(pool, &duid_hex, ia.id, addr, None).await?;
        let mut reply = self.base_reply(msg, MessageType::Reply)?;
        reply
            .opts_mut()
            .insert(DhcpOption::IANA(self.allocator.build_ia_na(ia.id, &outcome)));
        if outcome.is_success() {
            reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
                status: Status::Success,
                msg: "lease renewed".to_string(),
            }));
        }
        self.add_server_options(&mut reply, pool);
        Ok(reply)
    }

    /// REBIND → REPLY (RFC 8415 §18.2.4). Treated like RENEW.
    pub async fn handle_rebind(&self, msg: &Message) -> Result<Message, StateMachineError> {
        self.handle_renew(msg).await
    }

    /// RELEASE → REPLY (RFC 8415 §18.2.6).
    pub async fn handle_release(&self, msg: &Message) -> Result<Message, StateMachineError> {
        self.check_server_id(msg)?;
        let client_id = Self::client_id(msg)
            .ok_or(StateMachineError::MissingOption("ClientId"))?;
        let duid_hex = duid::Duid::parse(client_id).to_hex();

        let ia = match Self::first_ia_na(msg) {
            Some(ia) => ia,
            None => {
                return Ok(self.error_reply(msg, Status::Success, "released (no IA_NA)"));
            }
        };
        let addr = ia_addr_from_ia(ia);
        let mut reply = self.base_reply(msg, MessageType::Reply)?;
        if let Some(addr) = addr {
            self.allocator.release(&duid_hex, ia.id, addr).await?;
            reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
                status: Status::Success,
                msg: "lease released".to_string(),
            }));
        } else {
            reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
                status: Status::NoBinding,
                msg: "no address in release".to_string(),
            }));
        }
        Ok(reply)
    }

    /// CONFIRM → REPLY (RFC 8415 §18.2.5). Replies Success if the client's
    /// addresses are still on-link (we assume yes for the homelab).
    pub async fn handle_confirm(&self, msg: &Message) -> Result<Message, StateMachineError> {
        self.check_server_id(msg)?;
        let mut reply = self.base_reply(msg, MessageType::Reply)?;
        reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
            status: Status::Success,
            msg: "addresses still on-link".to_string(),
        }));
        Ok(reply)
    }

    /// INFORMATION-REQUEST → REPLY (RFC 8415 §18.2.7, stateless).
    pub async fn handle_information(&self, msg: &Message) -> Result<Message, StateMachineError> {
        self.check_server_id(msg)?;
        let pool = self.first_pool().ok();
        let mut reply = self.base_reply(msg, MessageType::Reply)?;
        if let Some(pool) = pool {
            self.add_server_options(&mut reply, pool);
        }
        reply
            .opts_mut()
            .insert(options::information_refresh_time(
                self.config.info_refresh_time_secs(),
            ));
        reply.opts_mut().insert(DhcpOption::StatusCode(StatusCode {
            status: Status::Success,
            msg: "information provided".to_string(),
        }));
        Ok(reply)
    }
}

/// Extract the IPv6 address from the IAAddr sub-option of an IA_NA.
fn ia_addr_from_ia(ia: &dhcproto::v6::IANA) -> Option<Ipv6Addr> {
    match ia.opts.get(OptionCode::IAAddr)? {
        DhcpOption::IAAddr(addr) => Some(addr.addr),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::v6::lease_store::SqliteLeaseStoreV6;
    use dhcproto::v6::{DhcpOption, DhcpOptions, IAAddr, IANA};

    fn server_duid() -> Vec<u8> {
        duid::server_duid_en(32473, b"dnshub-server").as_ref().to_vec()
    }

    fn config() -> DhcpV6Config {
        DhcpV6Config {
            enabled: true,
            lease_time_hours: 24,
            domain_search: vec!["levonk.com".to_string()],
            ntp_server: Some("fd00:1234:5678::55".parse().unwrap()),
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

    fn client_duid() -> Vec<u8> {
        duid::server_duid_en(32473, b"client-1").as_ref().to_vec()
    }

    fn solicit() -> Message {
        let mut msg = Message::new(MessageType::Solicit);
        msg.opts_mut().insert(DhcpOption::ClientId(client_duid()));
        let mut ia_opts = DhcpOptions::new();
        ia_opts.insert(DhcpOption::IAAddr(IAAddr {
            addr: "fd00:1234:5678::100".parse().unwrap(),
            preferred_life: 0,
            valid_life: 0,
            opts: DhcpOptions::new(),
        }));
        msg.opts_mut().insert(DhcpOption::IANA(IANA {
            id: 42,
            t1: 0,
            t2: 0,
            opts: ia_opts,
        }));
        msg
    }

    fn request(addr: Ipv6Addr) -> Message {
        let mut msg = Message::new(MessageType::Request);
        msg.opts_mut().insert(DhcpOption::ClientId(client_duid()));
        msg.opts_mut().insert(DhcpOption::ServerId(server_duid()));
        let mut ia_opts = DhcpOptions::new();
        ia_opts.insert(DhcpOption::IAAddr(IAAddr {
            addr,
            preferred_life: 3600,
            valid_life: 7200,
            opts: DhcpOptions::new(),
        }));
        msg.opts_mut().insert(DhcpOption::IANA(IANA {
            id: 42,
            t1: 0,
            t2: 0,
            opts: ia_opts,
        }));
        msg
    }

    fn renew(addr: Ipv6Addr) -> Message {
        let mut msg = Message::new(MessageType::Renew);
        msg.opts_mut().insert(DhcpOption::ClientId(client_duid()));
        msg.opts_mut().insert(DhcpOption::ServerId(server_duid()));
        let mut ia_opts = DhcpOptions::new();
        ia_opts.insert(DhcpOption::IAAddr(IAAddr {
            addr,
            preferred_life: 3600,
            valid_life: 7200,
            opts: DhcpOptions::new(),
        }));
        msg.opts_mut().insert(DhcpOption::IANA(IANA {
            id: 42,
            t1: 0,
            t2: 0,
            opts: ia_opts,
        }));
        msg
    }

    fn release(addr: Ipv6Addr) -> Message {
        let mut msg = Message::new(MessageType::Release);
        msg.opts_mut().insert(DhcpOption::ClientId(client_duid()));
        msg.opts_mut().insert(DhcpOption::ServerId(server_duid()));
        let mut ia_opts = DhcpOptions::new();
        ia_opts.insert(DhcpOption::IAAddr(IAAddr {
            addr,
            preferred_life: 0,
            valid_life: 0,
            opts: DhcpOptions::new(),
        }));
        msg.opts_mut().insert(DhcpOption::IANA(IANA {
            id: 42,
            t1: 0,
            t2: 0,
            opts: ia_opts,
        }));
        msg
    }

    fn information_request() -> Message {
        let mut msg = Message::new(MessageType::InformationRequest);
        msg.opts_mut().insert(DhcpOption::ClientId(client_duid()));
        msg
    }

    #[tokio::test]
    async fn solicit_returns_advertise() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        let reply = sm.handle(&solicit()).await.unwrap();
        assert_eq!(reply.msg_type(), MessageType::Advertise);
        // ServerId present and matches.
        match reply.opts().get(OptionCode::ServerId) {
            Some(DhcpOption::ServerId(id)) => assert_eq!(id, &server_duid()),
            other => panic!("expected ServerId, got {other:?}"),
        }
        // IA_NA present with an IAAddr.
        let ia = match reply.opts().get(OptionCode::IANA) {
            Some(DhcpOption::IANA(ia)) => ia,
            other => panic!("expected IANA, got {other:?}"),
        };
        assert!(ia.opts.get(OptionCode::IAAddr).is_some());
    }

    #[tokio::test]
    async fn request_returns_reply_with_lease() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        // SOLLICIT first to advertise an address.
        let adv = sm.handle(&solicit()).await.unwrap();
        let adv_ia = match adv.opts().get(OptionCode::IANA) {
            Some(DhcpOption::IANA(ia)) => ia,
            _ => panic!("no IANA in advertise"),
        };
        let addr = match adv_ia.opts.get(OptionCode::IAAddr) {
            Some(DhcpOption::IAAddr(a)) => a.addr,
            _ => panic!("no IAAddr in advertise"),
        };
        // REQUEST the advertised address.
        let reply = sm.handle(&request(addr)).await.unwrap();
        assert_eq!(reply.msg_type(), MessageType::Reply);
        assert!(reply.opts().get(OptionCode::IANA).is_some());
        let status = match reply.opts().get(OptionCode::StatusCode) {
            Some(DhcpOption::StatusCode(s)) => s.status,
            _ => panic!("no status code"),
        };
        assert_eq!(status, Status::Success);
    }

    #[tokio::test]
    async fn renew_extends_lease() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        let _ = sm.handle(&solicit()).await.unwrap();
        let adv = sm.handle(&solicit()).await.unwrap();
        let addr = match adv.opts().get(OptionCode::IANA).unwrap() {
            DhcpOption::IANA(ia) => match ia.opts.get(OptionCode::IAAddr).unwrap() {
                DhcpOption::IAAddr(a) => a.addr,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let _ = sm.handle(&request(addr)).await.unwrap();
        let reply = sm.handle(&renew(addr)).await.unwrap();
        assert_eq!(reply.msg_type(), MessageType::Reply);
        let status = match reply.opts().get(OptionCode::StatusCode).unwrap() {
            DhcpOption::StatusCode(s) => s.status,
            _ => unreachable!(),
        };
        assert_eq!(status, Status::Success);
    }

    #[tokio::test]
    async fn release_returns_success() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        let adv = sm.handle(&solicit()).await.unwrap();
        let addr = match adv.opts().get(OptionCode::IANA).unwrap() {
            DhcpOption::IANA(ia) => match ia.opts.get(OptionCode::IAAddr).unwrap() {
                DhcpOption::IAAddr(a) => a.addr,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let _ = sm.handle(&request(addr)).await.unwrap();
        let reply = sm.handle(&release(addr)).await.unwrap();
        assert_eq!(reply.msg_type(), MessageType::Reply);
        let status = match reply.opts().get(OptionCode::StatusCode).unwrap() {
            DhcpOption::StatusCode(s) => s.status,
            _ => unreachable!(),
        };
        assert_eq!(status, Status::Success);
    }

    #[tokio::test]
    async fn information_request_returns_stateless_options() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        let reply = sm.handle(&information_request()).await.unwrap();
        assert_eq!(reply.msg_type(), MessageType::Reply);
        assert!(reply.opts().get(OptionCode::DomainNameServers).is_some());
        assert!(reply.opts().get(OptionCode::DomainSearchList).is_some());
        assert!(reply.opts().get(OptionCode::NtpServer).is_some());
        assert!(reply
            .opts()
            .get(OptionCode::InformationRefreshTime)
            .is_some());
    }

    #[tokio::test]
    async fn request_with_wrong_server_id_is_rejected() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        let mut msg = request("fd00:1234:5678::100".parse().unwrap());
        msg.opts_mut().remove(OptionCode::ServerId);
        msg.opts_mut().insert(DhcpOption::ServerId(vec![0xff, 0xff]));
        let result = sm.handle(&msg).await;
        assert!(matches!(result, Err(StateMachineError::ServerIdMismatch)));
    }

    #[tokio::test]
    async fn solicit_without_client_id_errors() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let sm = DhcpV6StateMachine::new(config(), server_duid(), store);
        let msg = Message::new(MessageType::Solicit);
        let result = sm.handle(&msg).await;
        assert!(matches!(result, Err(StateMachineError::MissingOption(_))));
    }
}
