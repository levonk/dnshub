//! DHCPv4 state machine.
//!
//! Implements the RFC 2131 four-message exchange (DISCOVER → OFFER →
//! REQUEST → ACK) plus RELEASE, DECLINE, and INFORM handling. The state
//! machine is driven by [`DhcpStateMachine::handle`], which takes an
//! incoming [`dhcproto::v4::Message`] and returns an outgoing reply
//! `Message` (or `None` for messages that produce no response).
//!
//! ## States
//!
//! The DHCP client state machine (INIT, SELECTING, REQUESTING, BOUND,
//! RENEWING, REBINDING) is tracked on the client side; the server's
//! view is simpler — it tracks per-lease state in the lease store
//! (`Offered`, `Active`, `Released`, `Declined`, `Expired`). The
//! [`DhcpClientState`] enum is provided for observability and for
//! future client-classification hooks (story 04-004).
//!
//! ## Message flow
//!
//! ```text
//! DISCOVER  →  handle_discover  →  OFFER   (lease = Offered)
//! REQUEST   →  handle_request   →  ACK     (lease = Active)
//! REQUEST   →  handle_request   →  NAK     (if IP not offered to this client)
//! RELEASE   →  handle_release   →  (none)  (lease = Released / deleted)
//! DECLINE   →  handle_decline   →  (none)  (lease = Declined / conflicted)
//! INFORM    →  handle_inform    →  ACK     (no lease, just options)
//! ```

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, Message, MessageType, Opcode};

use super::config::{DhcpPoolV4, DhcpV4Config, StaticLeaseV4};
use super::lease_store::{
    LeaseState, LeaseStoreError, LeaseStoreV4, LeaseV4, StaticLeaseV4Record, now_ts,
};
use super::options::DhcpOptionBuilder;
use super::pool::PoolAllocator;

/// DHCP client states per RFC 2131 §4.1.
///
/// These are the client-side states; the server uses them for logging
/// and (future) classification hooks. The server's own state is the
/// [`LeaseState`] stored in the lease table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DhcpClientState {
    /// Client has no lease and is sending DISCOVER.
    Init,
    /// Client has received OFFER(s) and is sending REQUEST.
    Selecting,
    /// Client is requesting a specific IP (INIT-REBOOT or RENEWING).
    Requesting,
    /// Client has an active lease.
    Bound,
    /// Client is renewing (T1 expired, unicast to server).
    Renewing,
    /// Client is rebinding (T2 expired, broadcast to any server).
    Rebinding,
}

/// Outcome of handling a DHCP message.
#[derive(Debug)]
pub enum HandleOutcome {
    /// A reply message should be sent back to the client.
    Reply(Message),
    /// No reply (e.g. for RELEASE / DECLINE).
    NoReply,
    /// An error occurred while processing.
    Error(LeaseStoreError),
}

impl HandleOutcome {
    /// Unwrap into the reply `Message`, panicking if this is not a
    /// `Reply`. For tests.
    pub fn into_reply(self) -> Message {
        match self {
            HandleOutcome::Reply(m) => m,
            _ => panic!("expected a Reply outcome"),
        }
    }
}

/// The DHCPv4 server state machine.
///
/// Holds references to the lease store, pool allocator, and config. The
/// state machine is stateless across messages — all persistent state
/// lives in the lease store — so a single `DhcpStateMachine` can handle
/// messages from many clients concurrently (the lease store handles
/// serialization).
pub struct DhcpStateMachine<'a> {
    config: &'a DhcpV4Config,
    store: &'a dyn LeaseStoreV4,
    allocator: &'a PoolAllocator,
    /// The server's own IP address (used as SIADDR and server identifier).
    server_ip: Ipv4Addr,
}

impl<'a> DhcpStateMachine<'a> {
    /// Create a new state machine.
    pub fn new(
        config: &'a DhcpV4Config,
        store: &'a dyn LeaseStoreV4,
        allocator: &'a PoolAllocator,
        server_ip: Ipv4Addr,
    ) -> Self {
        Self {
            config,
            store,
            allocator,
            server_ip,
        }
    }

    /// Dispatch an incoming message to the appropriate handler.
    pub async fn handle(&self, msg: &Message) -> HandleOutcome {
        let msg_type = match msg.opts().msg_type() {
            Some(mt) => mt,
            None => {
                tracing::debug!("dhcp: ignoring message with no message-type option");
                return HandleOutcome::NoReply;
            }
        };
        match msg_type {
            MessageType::Discover => self.handle_discover(msg).await,
            MessageType::Request => self.handle_request(msg).await,
            MessageType::Release => self.handle_release(msg).await,
            MessageType::Decline => self.handle_decline(msg).await,
            MessageType::Inform => self.handle_inform(msg).await,
            other => {
                tracing::debug!(?other, "dhcp: unsupported message type, ignoring");
                HandleOutcome::NoReply
            }
        }
    }

    /// Handle DHCPDISCOVER: allocate an IP and send DHCPOFFER.
    ///
    /// 1. Check for a static lease (MAC → fixed IP). If found, offer it.
    /// 2. Check for an existing lease for this MAC. If found, re-offer it.
    /// 3. Otherwise, allocate a new IP from the pool via the allocator.
    async fn handle_discover(&self, msg: &Message) -> HandleOutcome {
        let mac = mac_from_msg(msg);
        let pool = match self.select_pool(msg) {
            Some(p) => p,
            None => {
                tracing::warn!("dhcp: no pool configured for DISCOVER");
                return HandleOutcome::NoReply;
            }
        };

        // 1. Static lease?
        if let Some(ip) = self.static_lease_ip(&mac).await {
            return self.build_offer(msg, ip, pool).await;
        }

        // 2. Existing lease for this MAC?
        if let Ok(Some(existing)) = self.store.get_lease_by_mac(&mac).await {
            if is_lease_valid(&existing) {
                return self.build_offer(msg, existing.ip, pool).await;
            }
        }

        // 3. Allocate new from pool.
        match self.allocator.find_free_ip(&self.config.pools, self.store).await {
            Ok(Some(ip)) => self.build_offer(msg, ip, pool).await,
            Ok(None) => {
                tracing::warn!("dhcp: pool exhausted, cannot offer");
                HandleOutcome::NoReply
            }
            Err(e) => HandleOutcome::Error(e),
        }
    }

    /// Handle DHCPREQUEST: confirm the lease and send DHCPACK (or NAK).
    async fn handle_request(&self, msg: &Message) -> HandleOutcome {
        let mac = mac_from_msg(msg);
        let pool = match self.select_pool(msg) {
            Some(p) => p,
            None => {
                return self.build_nak(msg);
            }
        };

        // The requested IP can come from option 50 (Requested IP Address)
        // or from ciaddr (for RENEWING/REBINDING).
        let requested = msg
            .opts()
            .get(dhcproto::v4::OptionCode::RequestedIpAddress)
            .and_then(|o| match o {
                DhcpOption::RequestedIpAddress(a) => Some(*a),
                _ => None,
            })
            .unwrap_or(msg.ciaddr());

        if requested.is_unspecified() {
            // No requested IP — can't confirm.
            return self.build_nak(msg);
        }

        // Static lease: the requested IP must match the static binding.
        if let Some(static_ip) = self.static_lease_ip(&mac).await {
            if static_ip == requested {
                return self.build_ack(msg, requested, pool, true);
            } else {
                return self.build_nak(msg);
            }
        }

        // Check that this IP was offered to this client (or is already
        // leased to it). For RENEWING, the existing lease must match.
        match self.store.get_lease(requested).await {
            Ok(Some(lease)) if lease.mac == mac => {
                // Confirm: update to Active.
                let updated = LeaseV4 {
                    lease_state: LeaseState::Active,
                    lease_expires: now_ts() + pool.lease_time_secs() as i64,
                    updated_at: now_ts(),
                    ..lease
                };
                if let Err(e) = self.store.update_lease(&updated).await {
                    return HandleOutcome::Error(e);
                }
                self.build_ack(msg, requested, pool, false)
            }
            Ok(Some(_)) => {
                // Leased to a different MAC.
                self.build_nak(msg)
            }
            Ok(None) => {
                // No existing lease for this IP. Per RFC 2131, the
                // server should NAK a REQUEST for an IP that was not
                // offered to this client (SELECTING/INIT-REBOOT states).
                self.build_nak(msg)
            }
            Err(e) => HandleOutcome::Error(e),
        }
    }

    /// Handle DHCPRELEASE: free the lease.
    async fn handle_release(&self, msg: &Message) -> HandleOutcome {
        let mac = mac_from_msg(msg);
        let ip = msg.ciaddr();
        if ip.is_unspecified() {
            tracing::debug!("dhcp: RELEASE with no ciaddr, ignoring");
            return HandleOutcome::NoReply;
        }
        // Verify the lease belongs to this MAC, then delete it.
        match self.store.get_lease(ip).await {
            Ok(Some(lease)) if lease.mac == mac => {
                if let Err(e) = self.store.delete_lease(ip).await {
                    return HandleOutcome::Error(e);
                }
                tracing::info!(%mac, %ip, "dhcp: lease released");
            }
            _ => {
                tracing::debug!(%mac, %ip, "dhcp: RELEASE for unknown lease, ignoring");
            }
        }
        HandleOutcome::NoReply
    }

    /// Handle DHCPDECLINE: mark the IP as conflicted.
    async fn handle_decline(&self, msg: &Message) -> HandleOutcome {
        let mac = mac_from_msg(msg);
        let requested = msg
            .opts()
            .get(dhcproto::v4::OptionCode::RequestedIpAddress)
            .and_then(|o| match o {
                DhcpOption::RequestedIpAddress(a) => Some(*a),
                _ => None,
            });
        let ip = match requested {
            Some(ip) => ip,
            None => {
                tracing::debug!("dhcp: DECLINE with no requested IP, ignoring");
                return HandleOutcome::NoReply;
            }
        };
        // Mark conflicted. If a lease exists, update its state; otherwise
        // insert a declined placeholder so the pool allocator skips it.
        match self.store.get_lease(ip).await {
            Ok(Some(_)) => {
                if let Err(e) = self.store.mark_conflicted(ip).await {
                    return HandleOutcome::Error(e);
                }
            }
            Ok(None) => {
                let ts = now_ts();
                let lease = LeaseV4 {
                    ip,
                    mac: mac.clone(),
                    hostname: None,
                    client_id: None,
                    vendor_class: None,
                    profile: None,
                    lease_expires: ts + 3600, // hold for 1h
                    lease_state: LeaseState::Declined,
                    created_at: ts,
                    updated_at: ts,
                };
                if let Err(e) = self.store.insert_lease(&lease).await {
                    return HandleOutcome::Error(e);
                }
            }
            Err(e) => return HandleOutcome::Error(e),
        }
        tracing::info!(%mac, %ip, "dhcp: lease declined (conflict)");
        HandleOutcome::NoReply
    }

    /// Handle DHCPINFORM: send ACK with options but no lease.
    ///
    /// INFORM is used by clients that already have an IP (ciaddr set)
    /// and just want configuration options.
    async fn handle_inform(&self, msg: &Message) -> HandleOutcome {
        let pool = match self.select_pool(msg) {
            Some(p) => p,
            None => {
                tracing::debug!("dhcp: INFORM with no matching pool, ignoring");
                return HandleOutcome::NoReply;
            }
        };
        // ACK with no lease time / no yiaddr (client already has its IP).
        let mut reply = self.base_reply(msg, MessageType::Ack);
        reply.set_ciaddr(msg.ciaddr());
        // INFORM ACKs do not include lease time per RFC 2131.
        let builder = DhcpOptionBuilder::new(self.server_ip, self.config);
        for opt in builder.build_pool_options(pool) {
            // Skip lease time for INFORM.
            if !matches!(opt, DhcpOption::AddressLeaseTime(_)) {
                reply.opts_mut().insert(opt);
            }
        }
        HandleOutcome::Reply(reply)
    }

    // -- helpers --------------------------------------------------------

    /// Select the pool for a message. For now, returns the first pool.
    /// Client classification (story 04-004) will refine this.
    fn select_pool(&self, _msg: &Message) -> Option<&DhcpPoolV4> {
        self.config.pools.first()
    }

    /// Look up a static lease for `mac` and return its IP if present.
    async fn static_lease_ip(&self, mac: &str) -> Option<Ipv4Addr> {
        match self.store.get_static_lease(mac).await {
            Ok(Some(sl)) => Some(sl.ip),
            _ => None,
        }
    }

    /// Build a DHCPOFFER message offering `ip` from `pool`.
    async fn build_offer(&self, msg: &Message, ip: Ipv4Addr, pool: &DhcpPoolV4) -> HandleOutcome {
        let mut reply = self.base_reply(msg, MessageType::Offer);
        reply.set_yiaddr(ip);
        let builder = DhcpOptionBuilder::new(self.server_ip, self.config);
        for opt in builder.build_pool_options(pool) {
            reply.opts_mut().insert(opt);
        }
        // Record the offered lease so REQUEST can confirm it.
        let ts = now_ts();
        let mac = mac_from_msg(msg);
        let lease = LeaseV4 {
            ip,
            mac: mac.clone(),
            hostname: hostname_from_msg(msg),
            client_id: client_id_from_msg(msg),
            vendor_class: vendor_class_from_msg(msg),
            profile: None,
            lease_expires: ts + pool.lease_time_secs() as i64,
            lease_state: LeaseState::Offered,
            created_at: ts,
            updated_at: ts,
        };
        // Insert or update the offered lease. If insert fails (IP already
        // has a row), update it instead.
        if let Err(_) = self.store.insert_lease(&lease).await {
            let _ = self.store.update_lease(&lease).await;
        }
        tracing::info!(%mac, %ip, "dhcp: OFFER");
        HandleOutcome::Reply(reply)
    }

    /// Build a DHCPACK message confirming `ip` from `pool`.
    fn build_ack(
        &self,
        msg: &Message,
        ip: Ipv4Addr,
        pool: &DhcpPoolV4,
        is_static: bool,
    ) -> HandleOutcome {
        let mut reply = self.base_reply(msg, MessageType::Ack);
        reply.set_yiaddr(ip);
        let builder = DhcpOptionBuilder::new(self.server_ip, self.config);
        for opt in builder.build_pool_options(pool) {
            reply.opts_mut().insert(opt);
        }
        let mac = mac_from_msg(msg);
        tracing::info!(%mac, %ip, is_static, "dhcp: ACK");
        HandleOutcome::Reply(reply)
    }

    /// Build a DHCPNAK message.
    fn build_nak(&self, msg: &Message) -> HandleOutcome {
        let mut reply = self.base_reply(msg, MessageType::Nak);
        // NAK should carry the server identifier and a message.
        reply.opts_mut().insert(DhcpOption::ServerIdentifier(self.server_ip));
        HandleOutcome::Reply(reply)
    }

    /// Build a reply `Message` with common fields set.
    fn base_reply(&self, msg: &Message, msg_type: MessageType) -> Message {
        let mut reply = Message::default();
        reply.set_opcode(Opcode::BootReply);
        reply.set_xid(msg.xid());
        reply.set_flags(msg.flags());
        // Copy the client hardware address.
        reply.set_chaddr(msg.chaddr());
        reply.set_giaddr(msg.giaddr());
        // Server identifier (option 54).
        reply.opts_mut().insert(DhcpOption::ServerIdentifier(self.server_ip));
        // Message type (option 53).
        reply.opts_mut().insert(DhcpOption::MessageType(msg_type));
        reply
    }
}

// -- free functions -----------------------------------------------------

/// Extract the client MAC address from a message as a colon-separated
/// string, e.g. `"00:11:22:33:44:55"`.
fn mac_from_msg(msg: &Message) -> String {
    let chaddr = msg.chaddr();
    chaddr
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Extract the hostname (option 12) from a message.
fn hostname_from_msg(msg: &Message) -> Option<String> {
    msg.opts()
        .get(dhcproto::v4::OptionCode::Hostname)
        .and_then(|o| match o {
            DhcpOption::Hostname(s) => Some(s.clone()),
            _ => None,
        })
}

/// Extract the client identifier (option 61) from a message.
fn client_id_from_msg(msg: &Message) -> Option<String> {
    msg.opts()
        .get(dhcproto::v4::OptionCode::ClientIdentifier)
        .and_then(|o| match o {
            DhcpOption::ClientIdentifier(bytes) => {
                Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
            }
            _ => None,
        })
}

/// Extract the vendor class (option 60) from a message.
fn vendor_class_from_msg(msg: &Message) -> Option<String> {
    msg.opts()
        .get(dhcproto::v4::OptionCode::ClassIdentifier)
        .and_then(|o| match o {
            DhcpOption::ClassIdentifier(bytes) => {
                String::from_utf8(bytes.clone()).ok()
            }
            _ => None,
        })
}

/// Check whether a lease is still valid (not expired/released/declined).
fn is_lease_valid(lease: &LeaseV4) -> bool {
    matches!(lease.lease_state, LeaseState::Offered | LeaseState::Active)
        && lease.lease_expires > now_ts()
}

/// Convert a [`StaticLeaseV4`] config entry to a store record.
pub fn static_lease_to_record(sl: &StaticLeaseV4) -> Result<StaticLeaseV4Record, std::net::AddrParseError> {
    Ok(StaticLeaseV4Record {
        mac: sl.mac.clone(),
        ip: sl.ip_addr()?,
        hostname: if sl.hostname.is_empty() {
            None
        } else {
            Some(sl.hostname.clone())
        },
        profile: if sl.profile.is_empty() {
            None
        } else {
            Some(sl.profile.clone())
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::v4::config::{DhcpPoolV4, DhcpV4Config, StaticLeaseV4};
    use crate::dhcp::v4::lease_store::SqliteLeaseStoreV4;
    use crate::dhcp::v4::pool::PoolAllocator;
    use dhcproto::v4::{Flags, HType, OptionCode};
    use std::collections::HashMap;

    fn make_config() -> DhcpV4Config {
        let pool = DhcpPoolV4 {
            name: "main".to_string(),
            subnet: "192.168.1.0/24".to_string(),
            pool_start: "192.168.1.100".to_string(),
            pool_end: "192.168.1.200".to_string(),
            router: "192.168.1.1".to_string(),
            lease_time_hours: 24,
            options: HashMap::new(),
        };
        DhcpV4Config {
            enabled: true,
            interface: "eth0".to_string(),
            listen: "0.0.0.0:67".to_string(),
            domain: "levonk.com".to_string(),
            ntp_server: "172.20.255.55".to_string(),
            pools: vec![pool],
            static_leases: vec![],
        }
    }

    fn make_discover(mac: &[u8]) -> Message {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_htype(HType::Eth);
        msg.set_chaddr(mac);
        msg.set_xid(0x12345678);
        msg.set_flags(Flags::default().set_broadcast());
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Discover));
        msg
    }

    fn make_request(mac: &[u8], requested_ip: Ipv4Addr) -> Message {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_htype(HType::Eth);
        msg.set_chaddr(mac);
        msg.set_xid(0x12345678);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Request));
        msg.opts_mut()
            .insert(DhcpOption::RequestedIpAddress(requested_ip));
        msg
    }

    fn make_release(mac: &[u8], ciaddr: Ipv4Addr) -> Message {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_chaddr(mac);
        msg.set_ciaddr(ciaddr);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Release));
        msg
    }

    fn make_decline(mac: &[u8], ip: Ipv4Addr) -> Message {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_chaddr(mac);
        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Decline));
        msg.opts_mut().insert(DhcpOption::RequestedIpAddress(ip));
        msg
    }

    const MAC: [u8; 6] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];

    /// Test fixture owning the pool allocator so the state machine can
    /// borrow it.
    struct TestFixture<'a> {
        config: &'a DhcpV4Config,
        store: &'a SqliteLeaseStoreV4,
        allocator: PoolAllocator,
    }

    impl<'a> TestFixture<'a> {
        fn new(config: &'a DhcpV4Config, store: &'a SqliteLeaseStoreV4) -> Self {
            Self {
                config,
                store,
                allocator: PoolAllocator::new_noop(),
            }
        }

        fn sm(&self) -> DhcpStateMachine<'_> {
            DhcpStateMachine::new(
                self.config,
                self.store,
                &self.allocator,
                Ipv4Addr::new(192, 168, 1, 67),
            )
        }
    }

    #[tokio::test]
    async fn discover_returns_offer() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();
        let msg = make_discover(&MAC);
        let reply = sm.handle(&msg).await.into_reply();
        assert_eq!(reply.opts().msg_type(), Some(MessageType::Offer));
        // Should have offered the first pool IP.
        assert_eq!(reply.yiaddr(), Ipv4Addr::new(192, 168, 1, 100));
    }

    #[tokio::test]
    async fn discover_then_request_returns_ack() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();

        // DISCOVER
        let discover = make_discover(&MAC);
        let offer = sm.handle(&discover).await.into_reply();
        let offered_ip = offer.yiaddr();
        assert_eq!(offered_ip, Ipv4Addr::new(192, 168, 1, 100));

        // REQUEST for the offered IP
        let request = make_request(&MAC, offered_ip);
        let ack = sm.handle(&request).await.into_reply();
        assert_eq!(ack.opts().msg_type(), Some(MessageType::Ack));
        assert_eq!(ack.yiaddr(), offered_ip);

        // Lease should now be Active in the store.
        let lease = store.get_lease(offered_ip).await.unwrap().unwrap();
        assert_eq!(lease.lease_state, LeaseState::Active);
    }

    #[tokio::test]
    async fn request_for_wrong_ip_returns_nak() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();

        // REQUEST for an IP that was never offered.
        let request = make_request(&MAC, Ipv4Addr::new(192, 168, 1, 150));
        let reply = sm.handle(&request).await.into_reply();
        assert_eq!(reply.opts().msg_type(), Some(MessageType::Nak));
    }

    #[tokio::test]
    async fn request_for_other_clients_ip_returns_nak() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();

        // Client A gets a lease.
        let discover_a = make_discover(&MAC);
        let offer_a = sm.handle(&discover_a).await.into_reply();
        let ip_a = offer_a.yiaddr();
        let req_a = make_request(&MAC, ip_a);
        sm.handle(&req_a).await.into_reply();

        // Client B requests the same IP.
        let mac_b = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let req_b = make_request(&mac_b, ip_a);
        let reply = sm.handle(&req_b).await.into_reply();
        assert_eq!(reply.opts().msg_type(), Some(MessageType::Nak));
    }

    #[tokio::test]
    async fn release_frees_lease() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();

        // Get a lease.
        let discover = make_discover(&MAC);
        let offer = sm.handle(&discover).await.into_reply();
        let ip = offer.yiaddr();
        let req = make_request(&MAC, ip);
        sm.handle(&req).await.into_reply();
        assert!(store.get_lease(ip).await.unwrap().is_some());

        // Release it.
        let release = make_release(&MAC, ip);
        let outcome = sm.handle(&release).await;
        assert!(matches!(outcome, HandleOutcome::NoReply));
        assert!(store.get_lease(ip).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn decline_marks_conflicted() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();

        // Offer an IP.
        let discover = make_discover(&MAC);
        let offer = sm.handle(&discover).await.into_reply();
        let ip = offer.yiaddr();

        // Client declines it.
        let decline = make_decline(&MAC, ip);
        let outcome = sm.handle(&decline).await;
        assert!(matches!(outcome, HandleOutcome::NoReply));

        // The IP should be marked declined.
        let lease = store.get_lease(ip).await.unwrap().unwrap();
        assert_eq!(lease.lease_state, LeaseState::Declined);

        // A subsequent DISCOVER should skip the declined IP.
        let discover2 = make_discover(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        let offer2 = sm.handle(&discover2).await.into_reply();
        assert_ne!(offer2.yiaddr(), ip);
    }

    #[tokio::test]
    async fn static_lease_is_honored() {
        let mut config = make_config();
        config.static_leases.push(StaticLeaseV4 {
            mac: "00:11:22:33:44:55".to_string(),
            ip: "192.168.1.50".to_string(),
            hostname: "fixed".to_string(),
            profile: "parents".to_string(),
            lease_time_hours: None,
        });
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        // Pre-load the static lease into the store.
        let record = static_lease_to_record(&config.static_leases[0]).unwrap();
        store.upsert_static_lease(&record).await.unwrap();

        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();
        let discover = make_discover(&MAC);
        let offer = sm.handle(&discover).await.into_reply();
        assert_eq!(offer.yiaddr(), Ipv4Addr::new(192, 168, 1, 50));
    }

    #[tokio::test]
    async fn static_lease_request_wrong_ip_returns_nak() {
        let mut config = make_config();
        config.static_leases.push(StaticLeaseV4 {
            mac: "00:11:22:33:44:55".to_string(),
            ip: "192.168.1.50".to_string(),
            hostname: String::new(),
            profile: String::new(),
            lease_time_hours: None,
        });
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let record = static_lease_to_record(&config.static_leases[0]).unwrap();
        store.upsert_static_lease(&record).await.unwrap();

        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();
        // Request a different IP than the static binding.
        let req = make_request(&MAC, Ipv4Addr::new(192, 168, 1, 100));
        let reply = sm.handle(&req).await.into_reply();
        assert_eq!(reply.opts().msg_type(), Some(MessageType::Nak));
    }

    #[tokio::test]
    async fn no_pool_returns_no_reply() {
        let mut config = make_config();
        config.pools.clear();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();
        let msg = make_discover(&MAC);
        let outcome = sm.handle(&msg).await;
        assert!(matches!(outcome, HandleOutcome::NoReply));
    }

    #[tokio::test]
    async fn offer_includes_server_identifier() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();
        let msg = make_discover(&MAC);
        let reply = sm.handle(&msg).await.into_reply();
        let sid = reply.opts().get(OptionCode::ServerIdentifier);
        assert!(sid.is_some(), "server identifier must be present");
    }

    #[tokio::test]
    async fn reoffer_existing_lease_on_discover() {
        let config = make_config();
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let fixture = TestFixture::new(&config, &store);
        let sm = fixture.sm();

        // First DISCOVER → OFFER at .100
        let d1 = make_discover(&MAC);
        let o1 = sm.handle(&d1).await.into_reply();
        assert_eq!(o1.yiaddr(), Ipv4Addr::new(192, 168, 1, 100));

        // Second DISCOVER from same MAC should re-offer .100
        let d2 = make_discover(&MAC);
        let o2 = sm.handle(&d2).await.into_reply();
        assert_eq!(o2.yiaddr(), Ipv4Addr::new(192, 168, 1, 100));
    }

    #[test]
    fn mac_from_msg_formats_correctly() {
        let mut msg = Message::default();
        msg.set_chaddr(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(mac_from_msg(&msg), "00:11:22:33:44:55");
    }

    #[test]
    fn client_state_variants() {
        let states = [
            DhcpClientState::Init,
            DhcpClientState::Selecting,
            DhcpClientState::Requesting,
            DhcpClientState::Bound,
            DhcpClientState::Renewing,
            DhcpClientState::Rebinding,
        ];
        // Just ensure they're all constructible and distinct.
        for (i, s) in states.iter().enumerate() {
            for (j, s2) in states.iter().enumerate() {
                if i == j {
                    assert_eq!(s, s2);
                } else {
                    assert_ne!(s, s2);
                }
            }
        }
    }
}
