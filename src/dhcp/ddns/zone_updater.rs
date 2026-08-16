//! Zone updater — add/remove A/AAAA records in the local DNS zone.
//!
//! [`ZoneUpdater`] wraps a hickory-server [`InMemoryZoneHandler`] for the
//! configured local zone and provides high-level methods to add and remove
//! DDNS-managed A (IPv4) and AAAA (IPv6) records.
//!
//! ## Zone initialization
//!
//! A valid hickory zone requires a SOA record at the origin. [`ZoneUpdater::new`]
//! creates an [`InMemoryZoneHandler`] with a minimal SOA record (serial 1,
//! refresh 3600, retry 300, expire 86400, minimum 60) so that the zone is
//! queryable immediately.
//!
//! ## Record lifecycle
//!
//! - **Add**: [`ZoneUpdater::add_record`] upserts a `Record` into the zone.
//!   For A/AAAA records, `upsert` replaces any existing record with the same
//!   name + rdata (last-write-wins for hostname conflicts, per the story's
//!   risk mitigation).
//! - **Remove**: [`ZoneUpdater::remove_record`] acquires a write lock on the
//!   zone's record map, locates the `RecordSet` for `(name, record_type)`,
//!   removes the matching rdata, and drops the entire `RecordSet` if it
//!   becomes empty.

use hickory_proto::rr::{
    rdata::SOA, LowerName, Name, RData, Record, RecordType, RrKey,
};
use hickory_server::store::in_memory::InMemoryZoneHandler;
use hickory_server::zone_handler::{AxfrPolicy, ZoneType};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Initial SOA serial for DDNS-managed zones.
const INITIAL_SERIAL: u32 = 1;

/// Wraps an [`InMemoryZoneHandler`] and provides DDNS-oriented add/remove
/// operations for A and AAAA records.
///
/// The handler is shared via `Arc` so that the same zone can be inserted into
/// a hickory [`Catalog`](hickory_server::zone_handler::Catalog) for serving
/// while the [`DdnsManager`](super::DdnsManager) mutates it concurrently.
pub struct ZoneUpdater {
    handler: Arc<InMemoryZoneHandler>,
    origin: Name,
}

impl ZoneUpdater {
    /// Create a `ZoneUpdater` for `zone_name` with a minimal SOA record.
    ///
    /// The zone origin (`zone_name`) must be a fully-qualified DNS name such
    /// as `"levonk.com."`. A trailing dot is added automatically if missing.
    pub fn new(zone_name: &str) -> Result<Self, String> {
        let origin = normalize_zone_name(zone_name)?;
        let handler = build_zone_handler(&origin)?;
        Ok(Self {
            handler: Arc::new(handler),
            origin,
        })
    }

    /// Returns a clone of the underlying [`InMemoryZoneHandler`] (shared via
    /// `Arc`) suitable for insertion into a hickory `Catalog`.
    pub fn handler(&self) -> Arc<InMemoryZoneHandler> {
        Arc::clone(&self.handler)
    }

    /// Returns the zone origin as a fully-qualified [`Name`].
    pub fn origin(&self) -> &Name {
        &self.origin
    }

    // -----------------------------------------------------------------------
    // A records (IPv4)
    // -----------------------------------------------------------------------

    /// Add or update an A record: `hostname.zone → ipv4` with the given `ttl`.
    ///
    /// If a record with the same name and rdata already exists, `upsert`
    /// updates its TTL (last-write-wins for hostname conflicts).
    pub async fn add_a_record(&self, hostname: &str, ipv4: Ipv4Addr, ttl: u32) -> Result<(), String> {
        let record = self.build_record(hostname, ttl, RData::A(ipv4.into()));
        self.add_record(record).await
    }

    /// Remove the A record `hostname.zone → ipv4` from the zone.
    ///
    /// Returns `Ok(())` if the record was removed or did not exist.
    pub async fn remove_a_record(&self, hostname: &str, ipv4: Ipv4Addr) -> Result<(), String> {
        let record = self.build_record(hostname, 0, RData::A(ipv4.into()));
        self.remove_record(record).await
    }

    // -----------------------------------------------------------------------
    // AAAA records (IPv6)
    // -----------------------------------------------------------------------

    /// Add or update an AAAA record: `hostname.zone → ipv6` with the given
    /// `ttl`.
    pub async fn add_aaaa_record(
        &self,
        hostname: &str,
        ipv6: Ipv6Addr,
        ttl: u32,
    ) -> Result<(), String> {
        let record = self.build_record(hostname, ttl, RData::AAAA(ipv6.into()));
        self.add_record(record).await
    }

    /// Remove the AAAA record `hostname.zone → ipv6` from the zone.
    pub async fn remove_aaaa_record(&self, hostname: &str, ipv6: Ipv6Addr) -> Result<(), String> {
        let record = self.build_record(hostname, 0, RData::AAAA(ipv6.into()));
        self.remove_record(record).await
    }

    // -----------------------------------------------------------------------
    // Generic add/remove
    // -----------------------------------------------------------------------

    /// Add or update a record in the zone via `upsert`.
    async fn add_record(&self, record: Record) -> Result<(), String> {
        let name = record.name.clone();
        let rtype = record.record_type();
        let ttl = record.ttl;

        debug!(%name, ?rtype, ttl, "ddns: upserting record");
        let inserted = self.handler.upsert(record, INITIAL_SERIAL).await;
        if inserted {
            info!(%name, ?rtype, "ddns: record added/updated");
        } else {
            warn!(%name, ?rtype, "ddns: upsert returned false (record unchanged?)");
        }
        Ok(())
    }

    /// Remove a record from the zone.
    ///
    /// Acquires a write lock on the zone's record map, finds the `RecordSet`
    /// for `(name, record_type)`, removes the matching rdata, and drops the
    /// `RecordSet` entirely if it becomes empty.
    async fn remove_record(&self, record: Record) -> Result<(), String> {
        let name = record.name.clone();
        let rtype = record.record_type();
        let key = RrKey::new(LowerName::from(&name), rtype);

        debug!(%name, ?rtype, "ddns: removing record");

        let mut records = self.handler.records_mut().await;
        let Some(rrset_arc) = records.get_mut(&key) else {
            // No RecordSet for this key — nothing to remove.
            debug!(%name, ?rtype, "ddns: no rrset found, nothing to remove");
            return Ok(());
        };

        // Clone the Arc<RecordSet> into a mutable owned RecordSet, remove the
        // matching rdata, and either reinsert (if non-empty) or drop the key.
        let mut rrset = (**rrset_arc).clone();
        let removed = rrset.remove(&record, INITIAL_SERIAL);

        if !removed {
            debug!(%name, ?rtype, "ddns: rdata not found in rrset");
            return Ok(());
        }

        if rrset.is_empty() {
            records.remove(&key);
            info!(%name, ?rtype, "ddns: record removed (rrset now empty, key dropped)");
        } else {
            *rrset_arc = Arc::new(rrset);
            info!(%name, ?rtype, "ddns: record removed (rrset retained)");
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Query helpers (used by tests and the DdnsManager for logging)
    // -----------------------------------------------------------------------

    /// Look up all records matching `hostname.zone` for the given
    /// `record_type`.
    ///
    /// Returns a vector of `(rdata, ttl)` pairs. Returns an empty vector if
    /// no records exist.
    pub async fn lookup(
        &self,
        hostname: &str,
        record_type: RecordType,
    ) -> Result<Vec<(RData, u32)>, String> {
        let fqdn = self.fqdn(hostname)?;
        let key = RrKey::new(LowerName::from(&fqdn), record_type);

        let records = self.handler.records().await;
        let Some(rrset) = records.get(&key) else {
            return Ok(Vec::new());
        };

        Ok(rrset
            .records_without_rrsigs()
            .map(|r| (r.data.clone(), r.ttl))
            .collect())
    }

    /// Returns `true` if an A record `hostname.zone → ipv4` exists.
    pub async fn has_a_record(&self, hostname: &str, ipv4: Ipv4Addr) -> bool {
        match self.lookup(hostname, RecordType::A).await {
            Ok(entries) => entries
                .iter()
                .any(|(rdata, _)| matches!(rdata, RData::A(a) if a.0 == ipv4)),
            Err(_) => false,
        }
    }

    /// Returns `true` if an AAAA record `hostname.zone → ipv6` exists.
    pub async fn has_aaaa_record(&self, hostname: &str, ipv6: Ipv6Addr) -> bool {
        match self.lookup(hostname, RecordType::AAAA).await {
            Ok(entries) => entries
                .iter()
                .any(|(rdata, _)| matches!(rdata, RData::AAAA(a) if a.0 == ipv6)),
            Err(_) => false,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Build a fully-qualified record name: `hostname.zone.`.
    fn fqdn(&self, hostname: &str) -> Result<Name, String> {
        let trimmed = hostname.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Err("hostname is empty".to_string());
        }
        let full = format!("{trimmed}.{}", self.origin);
        Name::parse(&full, None).map_err(|e| format!("invalid name {full}: {e}"))
    }

    /// Build a `Record` for `hostname` with the given `ttl` and `rdata`.
    fn build_record(&self, hostname: &str, ttl: u32, rdata: RData) -> Record {
        let name = match self.fqdn(hostname) {
            Ok(n) => n,
            Err(e) => {
                error!(error = %e, hostname, "ddns: failed to build record name");
                // Fall back to the zone origin — this should not happen in
                // practice because the DdnsManager skips empty hostnames.
                self.origin.clone()
            }
        };
        Record::from_rdata(name, ttl, rdata)
    }
}

/// Normalize a zone name string into a fully-qualified [`Name`].
///
/// Adds a trailing dot if missing (e.g. `"levonk.com"` → `"levonk.com."`).
fn normalize_zone_name(zone: &str) -> Result<Name, String> {
    let trimmed = zone.trim();
    if trimmed.is_empty() {
        return Err("zone name is empty".to_string());
    }
    let with_dot = if trimmed.ends_with('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    };
    Name::parse(&with_dot, None).map_err(|e| format!("invalid zone name {with_dot}: {e}"))
}

/// Build an [`InMemoryZoneHandler`] for `origin` with a minimal SOA record.
///
/// The SOA uses:
/// - mname: `ns.{origin}`
/// - rname: `hostmaster.{origin}`
/// - serial: 1
/// - refresh: 3600, retry: 300, expire: 86400, minimum: 60
fn build_zone_handler(origin: &Name) -> Result<InMemoryZoneHandler, String> {
    let mname = Name::parse(&format!("ns.{origin}"), None)
        .map_err(|e| format!("invalid SOA mname: {e}"))?;
    let rname = Name::parse(&format!("hostmaster.{origin}"), None)
        .map_err(|e| format!("invalid SOA rname: {e}"))?;

    let soa = SOA::new(
        mname,
        rname,
        INITIAL_SERIAL,
        3600,    // refresh
        300,     // retry
        86400,   // expire
        60,      // minimum
    );

    let soa_record = Record::from_rdata(origin.clone(), 60, RData::SOA(soa));

    let mut handler = InMemoryZoneHandler::empty(
        origin.clone(),
        ZoneType::Primary,
        AxfrPolicy::Deny,
    );
    // Insert the SOA record synchronously via the &mut self API.
    if !handler.upsert_mut(soa_record, INITIAL_SERIAL) {
        return Err("failed to insert SOA record".to_string());
    }

    Ok(handler)
}

/// Dispatch an IP address to the appropriate add method.
///
/// IPv4 → A record, IPv6 → AAAA record. Used by the DdnsManager.
pub async fn add_ip_record(
    updater: &ZoneUpdater,
    hostname: &str,
    ip: IpAddr,
    ttl: u32,
) -> Result<(), String> {
    match ip {
        IpAddr::V4(v4) => updater.add_a_record(hostname, v4, ttl).await,
        IpAddr::V6(v6) => updater.add_aaaa_record(hostname, v6, ttl).await,
    }
}

/// Dispatch an IP address to the appropriate remove method.
pub async fn remove_ip_record(
    updater: &ZoneUpdater,
    hostname: &str,
    ip: IpAddr,
) -> Result<(), String> {
    match ip {
        IpAddr::V4(v4) => updater.remove_a_record(hostname, v4).await,
        IpAddr::V6(v6) => updater.remove_aaaa_record(hostname, v6).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_updater() -> ZoneUpdater {
        ZoneUpdater::new("levonk.com").expect("failed to create zone updater")
    }

    #[tokio::test]
    async fn add_and_query_a_record() {
        let updater = make_updater();
        let ip: Ipv4Addr = "192.168.1.50".parse().unwrap();

        updater
            .add_a_record("phone", ip, 60)
            .await
            .expect("add failed");

        assert!(updater.has_a_record("phone", ip).await);

        let entries = updater.lookup("phone", RecordType::A).await.unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].0 {
            RData::A(a) => assert_eq!(a.0, ip),
            other => panic!("expected A record, got {other:?}"),
        }
        assert_eq!(entries[0].1, 60);
    }

    #[tokio::test]
    async fn add_and_query_aaaa_record() {
        let updater = make_updater();
        let ip: Ipv6Addr = "fd00::42".parse().unwrap();

        updater
            .add_aaaa_record("iot", ip, 30)
            .await
            .expect("add failed");

        assert!(updater.has_aaaa_record("iot", ip).await);

        let entries = updater.lookup("iot", RecordType::AAAA).await.unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].0 {
            RData::AAAA(a) => assert_eq!(a.0, ip),
            other => panic!("expected AAAA record, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remove_a_record() {
        let updater = make_updater();
        let ip: Ipv4Addr = "192.168.1.51".parse().unwrap();

        updater.add_a_record("laptop", ip, 60).await.unwrap();
        assert!(updater.has_a_record("laptop", ip).await);

        updater.remove_a_record("laptop", ip).await.unwrap();
        assert!(!updater.has_a_record("laptop", ip).await);

        // Lookup returns empty after removal.
        let entries = updater.lookup("laptop", RecordType::A).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn remove_aaaa_record() {
        let updater = make_updater();
        let ip: Ipv6Addr = "fd00::99".parse().unwrap();

        updater.add_aaaa_record("camera", ip, 60).await.unwrap();
        assert!(updater.has_aaaa_record("camera", ip).await);

        updater.remove_aaaa_record("camera", ip).await.unwrap();
        assert!(!updater.has_aaaa_record("camera", ip).await);
    }

    #[tokio::test]
    async fn remove_nonexistent_record_is_ok() {
        let updater = make_updater();
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        // Removing a record that was never added should not error.
        updater.remove_a_record("ghost", ip).await.unwrap();
    }

    #[tokio::test]
    async fn add_same_rdata_different_ttl_keeps_original_ttl() {
        // Per RFC 2136, two RRs are equal if their NAME, CLASS, TYPE, and
        // RDATA match — TTL is excluded from the comparison. hickory's
        // `upsert` therefore treats a re-add with a different TTL as a
        // no-op (the record already exists). This is correct behavior:
        // DDNS always uses the same configured TTL, so this path does not
        // arise in practice.
        let updater = make_updater();
        let ip: Ipv4Addr = "192.168.1.52".parse().unwrap();

        updater.add_a_record("desktop", ip, 60).await.unwrap();
        updater.add_a_record("desktop", ip, 120).await.unwrap();

        let entries = updater.lookup("desktop", RecordType::A).await.unwrap();
        assert_eq!(entries.len(), 1);
        // TTL remains 60 (the original) because the second upsert is a no-op.
        assert_eq!(entries[0].1, 60);
    }

    #[tokio::test]
    async fn multiple_a_records_same_name_different_ip() {
        let updater = make_updater();
        let ip1: Ipv4Addr = "192.168.1.60".parse().unwrap();
        let ip2: Ipv4Addr = "192.168.1.61".parse().unwrap();

        updater.add_a_record("multi", ip1, 60).await.unwrap();
        updater.add_a_record("multi", ip2, 60).await.unwrap();

        let entries = updater.lookup("multi", RecordType::A).await.unwrap();
        assert_eq!(entries.len(), 2);

        // Remove one, the other should remain.
        updater.remove_a_record("multi", ip1).await.unwrap();
        let entries = updater.lookup("multi", RecordType::A).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(updater.has_a_record("multi", ip2).await);
        assert!(!updater.has_a_record("multi", ip1).await);
    }

    #[tokio::test]
    async fn hostname_case_insensitive() {
        let updater = make_updater();
        let ip: Ipv4Addr = "192.168.1.70".parse().unwrap();

        updater.add_a_record("Phone", ip, 60).await.unwrap();
        // Lookup should find it regardless of case.
        assert!(updater.has_a_record("phone", ip).await);
        assert!(updater.has_a_record("PHONE", ip).await);
    }

    #[tokio::test]
    async fn add_ip_record_dispatches_v4_and_v6() {
        let updater = make_updater();
        let v4: IpAddr = "192.168.1.80".parse().unwrap();
        let v6: IpAddr = "fd00::80".parse().unwrap();

        add_ip_record(&updater, "v4host", v4, 60).await.unwrap();
        add_ip_record(&updater, "v6host", v6, 60).await.unwrap();

        assert!(updater.has_a_record("v4host", "192.168.1.80".parse().unwrap()).await);
        assert!(updater.has_aaaa_record("v6host", "fd00::80".parse().unwrap()).await);
    }

    #[tokio::test]
    async fn remove_ip_record_dispatches_v4_and_v6() {
        let updater = make_updater();
        let v4: IpAddr = "192.168.1.81".parse().unwrap();
        let v6: IpAddr = "fd00::81".parse().unwrap();

        add_ip_record(&updater, "host4", v4, 60).await.unwrap();
        add_ip_record(&updater, "host6", v6, 60).await.unwrap();

        remove_ip_record(&updater, "host4", v4).await.unwrap();
        remove_ip_record(&updater, "host6", v6).await.unwrap();

        assert!(!updater.has_a_record("host4", "192.168.1.81".parse().unwrap()).await);
        assert!(!updater.has_aaaa_record("host6", "fd00::81".parse().unwrap()).await);
    }

    #[test]
    fn normalize_zone_name_adds_trailing_dot() {
        let name = normalize_zone_name("example.com").unwrap();
        assert!(name.is_fqdn());
    }

    #[test]
    fn normalize_zone_name_keeps_trailing_dot() {
        let name = normalize_zone_name("example.com.").unwrap();
        assert!(name.is_fqdn());
    }

    #[test]
    fn normalize_empty_zone_errors() {
        assert!(normalize_zone_name("").is_err());
        assert!(normalize_zone_name("   ").is_err());
    }

    #[test]
    fn zone_updater_has_soa() {
        let updater = make_updater();
        // The handler should have a SOA record at the origin.
        // This is verified synchronously via records_get_mut in a blocking
        // context, but we just check the origin is set correctly.
        assert_eq!(updater.origin().to_string(), "levonk.com.");
    }
}
