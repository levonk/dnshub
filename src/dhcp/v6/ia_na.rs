//! IA_NA (Identity Association for Non-temporary Addresses) management.
//!
//! An IA_NA is a collection of non-temporary IPv6 addresses assigned to a
//! client, identified by an IAID (4-byte identifier chosen by the client).
//! RFC 8415 §21.4 defines the IA_NA option layout:
//!
//! ```text
//!     0                   1                   2                   3
//!     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//!     |          IAID (4 octets)                                      |
//!     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//!     |          T1 (renew)           |          T2 (rebind)         |
//!     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//!     |                                                               |
//!     .                  IA_NA-options (IAAddr, status)               .
//!     +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! This module provides [`IaNaAllocator`], which coordinates with a
//! [`LeaseStoreV6`](super::lease_store::LeaseStoreV6) to allocate, renew,
//! and release addresses within a configured pool. T1/T2 timers are
//! derived from [`DhcpV6Config`](super::config::DhcpV6Config).

use dhcproto::v6::{DhcpOption, DhcpOptions, IAAddr, IANA, StatusCode, Status};
use std::net::Ipv6Addr;
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::{DhcpPoolV6, DhcpV6Config};
use super::lease_store::{DhcpLeaseV6, LeaseState, LeaseStoreV6, LeaseStoreError};

/// Result of an IA_NA allocation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IaNaOutcome {
    /// An address was allocated/confirmed. Carries the IPv6 address,
    /// preferred lifetime, and valid lifetime.
    Allocated {
        addr: Ipv6Addr,
        preferred_lifetime: u32,
        valid_lifetime: u32,
    },
    /// The client's existing lease was renewed/extended.
    Renewed {
        addr: Ipv6Addr,
        preferred_lifetime: u32,
        valid_lifetime: u32,
    },
    /// No addresses are available in the pool.
    NoAddressesAvailable,
    /// The client has no existing binding for this IAID (used for RENEW/
    /// REBIND where the server has no record of the lease).
    NoBinding,
}

impl IaNaOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, IaNaOutcome::Allocated { .. } | IaNaOutcome::Renewed { .. })
    }

    /// The allocated/renewed address, if any.
    pub fn address(&self) -> Option<Ipv6Addr> {
        match self {
            IaNaOutcome::Allocated { addr, .. } | IaNaOutcome::Renewed { addr, .. } => Some(*addr),
            _ => None,
        }
    }
}

/// Allocates and manages IA_NA leases for a single DHCPv6 pool.
pub struct IaNaAllocator<S: LeaseStoreV6> {
    store: S,
    config: DhcpV6Config,
}

impl<S: LeaseStoreV6> IaNaAllocator<S> {
    /// Create a new allocator backed by `store` and governed by `config`.
    pub fn new(store: S, config: DhcpV6Config) -> Self {
        Self { store, config }
    }

    /// Current unix timestamp in seconds.
    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Preferred lifetime (seconds) for new leases.
    pub fn preferred_lifetime(&self) -> u32 {
        self.config.preferred_lifetime_secs()
    }

    /// Valid lifetime (seconds) for new leases.
    pub fn valid_lifetime(&self) -> u32 {
        self.config.valid_lifetime_secs()
    }

    /// T1 (renew) timer.
    pub fn t1(&self) -> u32 {
        self.config.t1_secs()
    }

    /// T2 (rebind) timer.
    pub fn t2(&self) -> u32 {
        self.config.t2_secs()
    }

    /// Allocate a new address for a SOLLICIT (advertise) or REQUEST.
    ///
    /// Checks for a static lease first (DUID → fixed address), then falls
    /// back to dynamic allocation from the pool. On SOLLICIT the lease is
    /// recorded as `Advertised`; on REQUEST it is recorded as `Active`.
    /// `commit` controls which state is written.
    pub async fn allocate(
        &self,
        pool: &DhcpPoolV6,
        duid: &str,
        iaid: u32,
        hostname: Option<&str>,
        commit: bool,
    ) -> Result<IaNaOutcome, LeaseStoreError> {
        // 1. Static lease takes precedence.
        if let Some(static_lease) = self.store.get_static_lease_v6(duid).await? {
            let addr = static_lease.ipv6_address;
            let now = Self::now();
            let state = if commit {
                LeaseState::Active
            } else {
                LeaseState::Advertised
            };
            let lease = DhcpLeaseV6 {
                ipv6_address: addr,
                duid: duid.to_string(),
                iaid,
                hostname: hostname.map(|h| h.to_string()),
                vendor_class: None,
                profile: static_lease.profile,
                lease_expires: now + self.valid_lifetime() as i64,
                lease_state: state,
                created_at: now,
                updated_at: now,
            };
            self.upsert_lease(&lease).await?;
            return Ok(IaNaOutcome::Allocated {
                addr,
                preferred_lifetime: self.preferred_lifetime(),
                valid_lifetime: self.valid_lifetime(),
            });
        }

        // 2. Reuse an existing active lease for this DUID+IAID if present.
        if let Some(existing) = self.store.get_lease_by_duid_v6(duid).await? {
            if existing.iaid == iaid && Self::in_pool(pool, existing.ipv6_address) {
                let now = Self::now();
                let mut updated = existing.clone();
                updated.lease_expires = now + self.valid_lifetime() as i64;
                updated.lease_state = if commit {
                    LeaseState::Active
                } else {
                    LeaseState::Advertised
                };
                updated.updated_at = now;
                if let Some(h) = hostname {
                    updated.hostname = Some(h.to_string());
                }
                self.upsert_lease(&updated).await?;
                return Ok(IaNaOutcome::Renewed {
                    addr: updated.ipv6_address,
                    preferred_lifetime: self.preferred_lifetime(),
                    valid_lifetime: self.valid_lifetime(),
                });
            }
        }

        // 3. Dynamic allocation: find a free address in the pool.
        let Some(addr) = self.store.find_free_ipv6(pool).await? else {
            return Ok(IaNaOutcome::NoAddressesAvailable);
        };
        let now = Self::now();
        let state = if commit {
            LeaseState::Active
        } else {
            LeaseState::Advertised
        };
        let lease = DhcpLeaseV6 {
            ipv6_address: addr,
            duid: duid.to_string(),
            iaid,
            hostname: hostname.map(|h| h.to_string()),
            vendor_class: None,
            profile: None,
            lease_expires: now + self.valid_lifetime() as i64,
            lease_state: state,
            created_at: now,
            updated_at: now,
        };
        self.insert_lease(&lease).await?;
        Ok(IaNaOutcome::Allocated {
            addr,
            preferred_lifetime: self.preferred_lifetime(),
            valid_lifetime: self.valid_lifetime(),
        })
    }

    /// Renew an existing lease (RENEW/REBIND). Requires an existing binding.
    pub async fn renew(
        &self,
        pool: &DhcpPoolV6,
        duid: &str,
        iaid: u32,
        addr: Ipv6Addr,
        hostname: Option<&str>,
    ) -> Result<IaNaOutcome, LeaseStoreError> {
        // Verify the client holds this address.
        let Some(existing) = self.store.get_lease_v6(&addr).await? else {
            return Ok(IaNaOutcome::NoBinding);
        };
        if existing.duid != duid || existing.iaid != iaid || !Self::in_pool(pool, addr) {
            return Ok(IaNaOutcome::NoBinding);
        }
        let now = Self::now();
        let mut updated = existing.clone();
        updated.lease_expires = now + self.valid_lifetime() as i64;
        updated.lease_state = LeaseState::Active;
        updated.updated_at = now;
        if let Some(h) = hostname {
            updated.hostname = Some(h.to_string());
        }
        self.upsert_lease(&updated).await?;
        Ok(IaNaOutcome::Renewed {
            addr,
            preferred_lifetime: self.preferred_lifetime(),
            valid_lifetime: self.valid_lifetime(),
        })
    }

    /// Release a lease (RELEASE). Marks the lease as `Released`.
    pub async fn release(
        &self,
        duid: &str,
        iaid: u32,
        addr: Ipv6Addr,
    ) -> Result<bool, LeaseStoreError> {
        let Some(existing) = self.store.get_lease_v6(&addr).await? else {
            return Ok(false);
        };
        if existing.duid != duid || existing.iaid != iaid {
            return Ok(false);
        }
        let now = Self::now();
        let mut updated = existing.clone();
        updated.lease_state = LeaseState::Released;
        updated.updated_at = now;
        self.upsert_lease(&updated).await?;
        Ok(true)
    }

    /// Build an [`IANA`] option for a reply/advertise carrying the outcome.
    pub fn build_ia_na(&self, iaid: u32, outcome: &IaNaOutcome) -> IANA {
        let mut opts = DhcpOptions::new();
        match outcome {
            IaNaOutcome::Allocated { addr, preferred_lifetime, valid_lifetime }
            | IaNaOutcome::Renewed { addr, preferred_lifetime, valid_lifetime } => {
                opts.insert(DhcpOption::IAAddr(IAAddr {
                    addr: *addr,
                    preferred_life: *preferred_lifetime,
                    valid_life: *valid_lifetime,
                    opts: DhcpOptions::new(),
                }));
            }
            IaNaOutcome::NoAddressesAvailable => {
                opts.insert(DhcpOption::StatusCode(StatusCode {
                    status: Status::NoAddrsAvail,
                    msg: "no addresses available".to_string(),
                }));
            }
            IaNaOutcome::NoBinding => {
                opts.insert(DhcpOption::StatusCode(StatusCode {
                    status: Status::NoBinding,
                    msg: "no binding for this IAID".to_string(),
                }));
            }
        }
        IANA {
            id: iaid,
            t1: self.t1(),
            t2: self.t2(),
            opts,
        }
    }

    fn in_pool(pool: &DhcpPoolV6, addr: Ipv6Addr) -> bool {
        let n = u128::from(addr);
        let start = u128::from(pool.pool_start);
        let end = u128::from(pool.pool_end);
        n >= start && n <= end
    }

    async fn upsert_lease(&self, lease: &DhcpLeaseV6) -> Result<(), LeaseStoreError> {
        // Try update first; if no row matched, insert.
        self.store.update_lease_v6(lease).await?;
        // update_lease_v6 does not report rows affected; verify presence and
        // insert if missing. This keeps the trait surface simple.
        if self.store.get_lease_v6(&lease.ipv6_address).await?.is_none() {
            self.store.insert_lease_v6(lease).await?;
        }
        Ok(())
    }

    async fn insert_lease(&self, lease: &DhcpLeaseV6) -> Result<(), LeaseStoreError> {
        // If a stale advertised row exists for this address, replace it.
        if self.store.get_lease_v6(&lease.ipv6_address).await?.is_some() {
            self.store.update_lease_v6(lease).await?;
        } else {
            self.store.insert_lease_v6(lease).await?;
        }
        Ok(())
    }

    /// Borrow the underlying store (for tests / inspection).
    pub fn store(&self) -> &S {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::v6::lease_store::SqliteLeaseStoreV6;

    fn config() -> DhcpV6Config {
        DhcpV6Config {
            enabled: true,
            lease_time_hours: 24,
            ..DhcpV6Config::default()
        }
    }

    fn pool() -> DhcpPoolV6 {
        DhcpPoolV6 {
            name: "main".into(),
            prefix: "fd00:1234:5678::/64".into(),
            pool_start: "fd00:1234:5678::100".parse().unwrap(),
            pool_end: "fd00:1234:5678::103".parse().unwrap(),
            dns_servers: vec!["fd00:1234:5678::67".parse().unwrap()],
        }
    }

    #[tokio::test]
    async fn allocate_dynamic_then_renew_then_release() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let alloc = IaNaAllocator::new(store, config());

        // Advertise (SOLLICIT) — not committed.
        let outcome = alloc
            .allocate(&pool(), "duid1", 42, Some("laptop"), false)
            .await
            .unwrap();
        let addr = outcome.address().expect("allocated");
        assert!(outcome.is_success());
        assert_eq!(alloc.store().get_lease_v6(&addr).await.unwrap().unwrap().lease_state, LeaseState::Advertised);

        // Commit (REQUEST).
        let outcome2 = alloc
            .allocate(&pool(), "duid1", 42, Some("laptop"), true)
            .await
            .unwrap();
        assert_eq!(outcome2.address(), Some(addr));
        assert_eq!(alloc.store().get_lease_v6(&addr).await.unwrap().unwrap().lease_state, LeaseState::Active);

        // Renew.
        let renewed = alloc
            .renew(&pool(), "duid1", 42, addr, Some("laptop"))
            .await
            .unwrap();
        assert!(matches!(renewed, IaNaOutcome::Renewed { .. }));

        // Release.
        let released = alloc.release("duid1", 42, addr).await.unwrap();
        assert!(released);
        assert_eq!(alloc.store().get_lease_v6(&addr).await.unwrap().unwrap().lease_state, LeaseState::Released);
    }

    #[tokio::test]
    async fn allocate_static_lease_takes_precedence() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        store
            .upsert_static_lease_v6(&super::super::lease_store::StaticLeaseV6 {
                duid: "static1".into(),
                ipv6_address: "fd00:1234:5678::50".parse().unwrap(),
                hostname: Some("printer".into()),
                profile: Some("iot".into()),
            })
            .await
            .unwrap();
        let alloc = IaNaAllocator::new(store, config());
        let outcome = alloc
            .allocate(&pool(), "static1", 1, None, true)
            .await
            .unwrap();
        assert_eq!(
            outcome.address(),
            Some("fd00:1234:5678::50".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[tokio::test]
    async fn allocate_two_clients_get_different_addresses() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let alloc = IaNaAllocator::new(store, config());
        let a = alloc
            .allocate(&pool(), "duid-a", 1, None, true)
            .await
            .unwrap()
            .address()
            .unwrap();
        let b = alloc
            .allocate(&pool(), "duid-b", 1, None, true)
            .await
            .unwrap()
            .address()
            .unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn renew_no_binding_for_unknown_duid() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let alloc = IaNaAllocator::new(store, config());
        let outcome = alloc
            .renew(
                &pool(),
                "unknown",
                1,
                "fd00:1234:5678::100".parse().unwrap(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(outcome, IaNaOutcome::NoBinding);
    }

    #[tokio::test]
    async fn pool_exhaustion_returns_no_addresses() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let alloc = IaNaAllocator::new(store, config());
        let p = pool();
        // Fill the pool (4 addresses).
        for i in 0..4 {
            alloc
                .allocate(&p, &format!("duid{i}"), 1, None, true)
                .await
                .unwrap();
        }
        let outcome = alloc.allocate(&p, "duid-overflow", 1, None, true).await.unwrap();
        assert_eq!(outcome, IaNaOutcome::NoAddressesAvailable);
    }

    #[tokio::test]
    async fn build_ia_na_carries_iaaddr_on_success() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let alloc = IaNaAllocator::new(store, config());
        let outcome = IaNaOutcome::Allocated {
            addr: "fd00::1".parse().unwrap(),
            preferred_lifetime: 3600,
            valid_lifetime: 7200,
        };
        let ia_na = alloc.build_ia_na(42, &outcome);
        assert_eq!(ia_na.id, 42);
        assert_eq!(ia_na.t1, config().t1_secs());
        assert_eq!(ia_na.t2, config().t2_secs());
        let iaaddr = ia_na.opts.get(dhcproto::v6::OptionCode::IAAddr);
        assert!(iaaddr.is_some());
    }

    #[tokio::test]
    async fn build_ia_na_no_addrs_status() {
        let store = SqliteLeaseStoreV6::open_in_memory().unwrap();
        let alloc = IaNaAllocator::new(store, config());
        let ia_na = alloc.build_ia_na(1, &IaNaOutcome::NoAddressesAvailable);
        let status = ia_na.opts.get(dhcproto::v6::OptionCode::StatusCode);
        assert!(status.is_some());
    }
}
