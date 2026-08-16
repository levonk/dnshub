//! DHCPv4 IP pool allocator with conflict detection.
//!
//! The pool allocator is responsible for finding a free IP address to
//! offer a client. It walks the configured pools in order and, for each
//! pool, asks the lease store for the first free address in the pool
//! range. Before returning an address it optionally performs a ping
//! (ICMP echo) check to detect conflicts — a device on the network that
//! already holds the address but is unknown to the DHCP server (e.g.
//! statically configured). If the ping succeeds, the address is marked
//! conflicted and the allocator tries the next one.
//!
//! Ping-before-offer is configurable (default: enabled). In environments
//! where ICMP is filtered, operators can disable it via config.

use std::net::Ipv4Addr;
use std::time::Duration;

use super::config::DhcpPoolV4;
use super::lease_store::{LeaseStoreError, LeaseStoreV4};

/// Outcome of a ping conflict check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingResult {
    /// No response — the address appears to be free.
    Free,
    /// A device responded — the address is in use (conflict).
    InUse,
}

/// Trait abstracting the ping (ICMP echo) probe used for conflict
/// detection. The default implementation, [`SystemPingProbe`], shells
/// out to the system `ping` command. Tests use [`NoopPingProbe`] which
/// always reports addresses as free.
#[async_trait::async_trait]
pub trait PingProbe: Send + Sync {
    /// Probe `ip`. Returns `PingResult::InUse` if a reply is received
    /// within the timeout, `PingResult::Free` otherwise.
    async fn probe(&self, ip: Ipv4Addr, timeout: Duration) -> PingResult;
}

/// A ping probe that always reports addresses as free.
///
/// Used in tests and as a fallback when ping is disabled.
pub struct NoopPingProbe;

#[async_trait::async_trait]
impl PingProbe for NoopPingProbe {
    async fn probe(&self, _ip: Ipv4Addr, _timeout: Duration) -> PingResult {
        PingResult::Free
    }
}

/// A ping probe that shells out to the system `ping` command.
///
/// This is a best-effort conflict detector. It uses `ping -c 1 -W <secs>`
/// (macOS/Linux compatible flags). If the `ping` binary is unavailable
/// or the command fails to execute, the address is treated as free.
pub struct SystemPingProbe;

#[async_trait::async_trait]
impl PingProbe for SystemPingProbe {
    async fn probe(&self, ip: Ipv4Addr, timeout: Duration) -> PingResult {
        let timeout_secs = timeout.as_secs().max(1);
        let output = tokio::process::Command::new("ping")
            .arg("-c")
            .arg("1")
            .arg("-W")
            .arg(timeout_secs.to_string())
            .arg(ip.to_string())
            .output()
            .await;
        match output {
            Ok(out) => {
                if out.status.success() {
                    PingResult::InUse
                } else {
                    PingResult::Free
                }
            }
            Err(_) => PingResult::Free,
        }
    }
}

/// IP pool allocator.
///
/// Walks the configured pools and finds a free, non-conflicted IP. The
/// allocator is given a reference to the lease store and a [`PingProbe`].
pub struct PoolAllocator {
    probe: Box<dyn PingProbe>,
    ping_timeout: Duration,
    ping_enabled: bool,
}

impl PoolAllocator {
    /// Create a new allocator with the given ping probe.
    pub fn new(probe: Box<dyn PingProbe>, ping_timeout: Duration, ping_enabled: bool) -> Self {
        Self {
            probe,
            ping_timeout,
            ping_enabled,
        }
    }

    /// Create a new allocator with [`NoopPingProbe`] (for tests).
    pub fn new_noop() -> Self {
        Self::new(Box::new(NoopPingProbe), Duration::from_millis(0), false)
    }

    /// Find a free IP across all configured pools.
    ///
    /// Returns the first free, non-conflicted address, or `None` if all
    /// pools are exhausted. On lease-store errors, returns the error.
    pub async fn find_free_ip(
        &self,
        pools: &[DhcpPoolV4],
        store: &dyn LeaseStoreV4,
    ) -> Result<Option<Ipv4Addr>, LeaseStoreError> {
        for pool in pools {
            if let Some(ip) = self.find_free_in_pool(pool, store).await? {
                return Ok(Some(ip));
            }
        }
        Ok(None)
    }

    /// Find a free IP within a single pool, applying ping conflict
    /// detection. Skips addresses that are in use or conflicted.
    async fn find_free_in_pool(
        &self,
        pool: &DhcpPoolV4,
        store: &dyn LeaseStoreV4,
    ) -> Result<Option<Ipv4Addr>, LeaseStoreError> {
        let start = match pool.start_addr() {
            Ok(a) => a,
            Err(_) => return Ok(None),
        };
        let end = match pool.end_addr() {
            Ok(a) => a,
            Err(_) => return Ok(None),
        };
        if u32::from(start) > u32::from(end) {
            return Ok(None);
        }

        // Ask the store for the first free IP, then ping-check it. If
        // conflicted, mark it and ask again. Limit retries to the pool
        // size to avoid an infinite loop.
        let pool_size = (u32::from(end) - u32::from(start) + 1) as usize;
        for _ in 0..pool_size {
            let candidate = match store.find_free_ip(start, end).await? {
                Some(ip) => ip,
                None => return Ok(None),
            };
            if !self.ping_enabled {
                return Ok(Some(candidate));
            }
            match self.probe.probe(candidate, self.ping_timeout).await {
                PingResult::Free => return Ok(Some(candidate)),
                PingResult::InUse => {
                    // Mark as conflicted so the store skips it next time.
                    // We don't need a full lease record — just mark it.
                    let _ = store.mark_conflicted(candidate).await;
                    continue;
                }
            }
        }
        Ok(None)
    }
}

impl Default for PoolAllocator {
    fn default() -> Self {
        Self::new(
            Box::new(SystemPingProbe),
            Duration::from_secs(1),
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::v4::lease_store::SqliteLeaseStoreV4;
    use std::collections::HashMap;

    fn pool(name: &str, start: &str, end: &str) -> DhcpPoolV4 {
        DhcpPoolV4 {
            name: name.to_string(),
            subnet: "192.168.1.0/24".to_string(),
            pool_start: start.to_string(),
            pool_end: end.to_string(),
            router: "192.168.1.1".to_string(),
            lease_time_hours: 24,
            options: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn find_free_ip_returns_first_free() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let allocator = PoolAllocator::new_noop();
        let pools = vec![pool("main", "192.168.1.100", "192.168.1.102")];
        let ip = allocator
            .find_free_ip(&pools, &store)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 100));
    }

    #[tokio::test]
    async fn find_free_ip_skips_occupied() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        // Insert a lease at .100
        let ts = crate::dhcp::v4::lease_store::now_ts();
        let lease = crate::dhcp::v4::lease_store::LeaseV4 {
            ip: Ipv4Addr::new(192, 168, 1, 100),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname: None,
            client_id: None,
            vendor_class: None,
            profile: None,
            lease_expires: ts + 3600,
            lease_state: crate::dhcp::v4::lease_store::LeaseState::Active,
            created_at: ts,
            updated_at: ts,
        };
        store.insert_lease(&lease).await.unwrap();

        let allocator = PoolAllocator::new_noop();
        let pools = vec![pool("main", "192.168.1.100", "192.168.1.102")];
        let ip = allocator
            .find_free_ip(&pools, &store)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 101));
    }

    #[tokio::test]
    async fn find_free_ip_exhausted_returns_none() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let allocator = PoolAllocator::new_noop();
        let pools = vec![pool("main", "192.168.1.100", "192.168.1.100")];
        // Insert lease at the only address
        let ts = crate::dhcp::v4::lease_store::now_ts();
        let lease = crate::dhcp::v4::lease_store::LeaseV4 {
            ip: Ipv4Addr::new(192, 168, 1, 100),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname: None,
            client_id: None,
            vendor_class: None,
            profile: None,
            lease_expires: ts + 3600,
            lease_state: crate::dhcp::v4::lease_store::LeaseState::Active,
            created_at: ts,
            updated_at: ts,
        };
        store.insert_lease(&lease).await.unwrap();
        let ip = allocator.find_free_ip(&pools, &store).await.unwrap();
        assert_eq!(ip, None);
    }

    /// A ping probe that reports a specific IP as in-use.
    struct ConflictProbe {
        conflicted: Ipv4Addr,
    }

    #[async_trait::async_trait]
    impl PingProbe for ConflictProbe {
        async fn probe(&self, ip: Ipv4Addr, _timeout: Duration) -> PingResult {
            if ip == self.conflicted {
                PingResult::InUse
            } else {
                PingResult::Free
            }
        }
    }

    #[tokio::test]
    async fn find_free_ip_skips_conflicted_via_ping() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let allocator = PoolAllocator::new(
            Box::new(ConflictProbe {
                conflicted: Ipv4Addr::new(192, 168, 1, 100),
            }),
            Duration::from_millis(10),
            true,
        );
        let pools = vec![pool("main", "192.168.1.100", "192.168.1.102")];
        let ip = allocator
            .find_free_ip(&pools, &store)
            .await
            .unwrap()
            .unwrap();
        // .100 is conflicted → skip to .101
        assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 101));
        // .100 should now be marked declined in the store (placeholder row).
        let lease = store
            .get_lease(Ipv4Addr::new(192, 168, 1, 100))
            .await
            .unwrap()
            .expect("declined placeholder should exist");
        assert_eq!(lease.lease_state, crate::dhcp::v4::lease_store::LeaseState::Declined);
    }

    #[tokio::test]
    async fn find_free_ip_across_multiple_pools() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let allocator = PoolAllocator::new_noop();
        let pools = vec![
            pool("main", "192.168.1.100", "192.168.1.100"),
            pool("guest", "192.168.10.100", "192.168.10.100"),
        ];
        // Fill the first pool
        let ts = crate::dhcp::v4::lease_store::now_ts();
        let lease = crate::dhcp::v4::lease_store::LeaseV4 {
            ip: Ipv4Addr::new(192, 168, 1, 100),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname: None,
            client_id: None,
            vendor_class: None,
            profile: None,
            lease_expires: ts + 3600,
            lease_state: crate::dhcp::v4::lease_store::LeaseState::Active,
            created_at: ts,
            updated_at: ts,
        };
        store.insert_lease(&lease).await.unwrap();
        let ip = allocator
            .find_free_ip(&pools, &store)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ip, Ipv4Addr::new(192, 168, 10, 100));
    }

    #[tokio::test]
    async fn find_free_ip_invalid_pool_returns_none() {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        let allocator = PoolAllocator::new_noop();
        let pools = vec![pool("bad", "not-an-ip", "also-bad")];
        let ip = allocator.find_free_ip(&pools, &store).await.unwrap();
        assert_eq!(ip, None);
    }

    #[tokio::test]
    async fn noop_probe_always_free() {
        let probe = NoopPingProbe;
        let r = probe
            .probe(Ipv4Addr::new(192, 168, 1, 1), Duration::from_millis(1))
            .await;
        assert_eq!(r, PingResult::Free);
    }
}
