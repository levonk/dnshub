//! DHCP lease audit event types.
//!
//! Each event records a single device join/leave lifecycle transition
//! (DHCPACK, RELEASE, DECLINE, expiry, or a blocked lease attempt) as defined
//! in PRD section 4.4 (lines 740-754). The shape mirrors the
//! `dhcp_audit_log` SQLite table columns.
//!
//! The [`DhcpAuditEvent`] struct is the in-memory representation; the
//! [`AuditLogger`] in [`super`] handles persistence.
//!
//! [`AuditLogger`]: super::AuditLogger

use serde::{Deserialize, Serialize};
use std::fmt;

/// The kind of lease lifecycle event recorded in the audit log.
///
/// Variants correspond to the `event_type` TEXT column of the
/// `dhcp_audit_log` table (PRD lines 744).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DhcpAuditEventType {
    /// A lease was granted (DHCPACK).
    Ack,
    /// A lease was renewed (DHCPACK for a REQUEST).
    Renew,
    /// A client released its lease (DHCPRELEASE).
    Release,
    /// A client declined an offered address (DHCPDECLINE).
    Decline,
    /// A lease expired without explicit release.
    Expire,
    /// An address conflict was detected (e.g. ping before offer failed).
    Conflict,
}

impl DhcpAuditEventType {
    /// Returns the stable string stored in the `event_type` column.
    pub fn as_str(self) -> &'static str {
        match self {
            DhcpAuditEventType::Ack => "ack",
            DhcpAuditEventType::Renew => "renew",
            DhcpAuditEventType::Release => "release",
            DhcpAuditEventType::Decline => "decline",
            DhcpAuditEventType::Expire => "expire",
            DhcpAuditEventType::Conflict => "conflict",
        }
    }
}

impl fmt::Display for DhcpAuditEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DhcpAuditEventType {
    type Err = UnknownEventType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ack" => Ok(DhcpAuditEventType::Ack),
            "renew" => Ok(DhcpAuditEventType::Renew),
            "release" => Ok(DhcpAuditEventType::Release),
            "decline" => Ok(DhcpAuditEventType::Decline),
            "expire" => Ok(DhcpAuditEventType::Expire),
            "conflict" => Ok(DhcpAuditEventType::Conflict),
            other => Err(UnknownEventType(other.to_string())),
        }
    }
}

/// Error returned when an `event_type` string from the database does not
/// match any known [`DhcpAuditEventType`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEventType(pub String);

impl fmt::Display for UnknownEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown DHCP audit event type: `{}`", self.0)
    }
}

impl std::error::Error for UnknownEventType {}

/// A single DHCP lease audit event.
///
/// Fields mirror the `dhcp_audit_log` table columns (PRD lines 741-751):
/// timestamp (unix millis), event_type, mac_address, duid, ip_address,
/// hostname, profile, and a freeform `details` string for extra context
/// (e.g. the reason a lease was blocked).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhcpAuditEvent {
    /// Wall-clock time of the event as unix milliseconds.
    pub timestamp: i64,
    /// The kind of lifecycle event.
    pub event_type: DhcpAuditEventType,
    /// Client MAC address (v4), when known.
    pub mac_address: Option<String>,
    /// Client DUID (DHCPv6), when known.
    pub duid: Option<String>,
    /// Leased/offered IP address, when applicable.
    pub ip_address: Option<String>,
    /// Client hostname (DHCP option 12), when supplied.
    pub hostname: Option<String>,
    /// Assigned policy profile, when known.
    pub profile: Option<String>,
    /// Freeform extra context (e.g. block reason).
    pub details: Option<String>,
}

impl DhcpAuditEvent {
    /// Create a new event with the given timestamp and type; all optional
    /// fields start as `None` and can be set via the builder-style methods.
    pub fn new(timestamp: i64, event_type: DhcpAuditEventType) -> Self {
        Self {
            timestamp,
            event_type,
            mac_address: None,
            duid: None,
            ip_address: None,
            hostname: None,
            profile: None,
            details: None,
        }
    }

    /// Set the client MAC address.
    pub fn with_mac(mut self, mac: impl Into<String>) -> Self {
        self.mac_address = Some(mac.into());
        self
    }

    /// Set the client DUID (DHCPv6).
    pub fn with_duid(mut self, duid: impl Into<String>) -> Self {
        self.duid = Some(duid.into());
        self
    }

    /// Set the leased/offered IP address.
    pub fn with_ip(mut self, ip: impl Into<String>) -> Self {
        self.ip_address = Some(ip.into());
        self
    }

    /// Set the client hostname.
    pub fn with_hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = Some(hostname.into());
        self
    }

    /// Set the assigned policy profile.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Set the freeform details string.
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_roundtrip() {
        for v in [
            DhcpAuditEventType::Ack,
            DhcpAuditEventType::Renew,
            DhcpAuditEventType::Release,
            DhcpAuditEventType::Decline,
            DhcpAuditEventType::Expire,
            DhcpAuditEventType::Conflict,
        ] {
            let s = v.as_str();
            assert_eq!(s.parse::<DhcpAuditEventType>().unwrap(), v);
            assert_eq!(format!("{v}"), s);
        }
    }

    #[test]
    fn unknown_event_type_is_error() {
        let err = "bogus".parse::<DhcpAuditEventType>().unwrap_err();
        assert_eq!(err, UnknownEventType("bogus".to_string()));
    }

    #[test]
    fn builder_sets_optional_fields() {
        let ev = DhcpAuditEvent::new(1_700_000_000_000, DhcpAuditEventType::Ack)
            .with_mac("00:11:22:33:44:55")
            .with_ip("192.168.1.42")
            .with_hostname("laptop")
            .with_profile("parents")
            .with_details("granted");
        assert_eq!(ev.timestamp, 1_700_000_000_000);
        assert_eq!(ev.event_type, DhcpAuditEventType::Ack);
        assert_eq!(ev.mac_address.as_deref(), Some("00:11:22:33:44:55"));
        assert_eq!(ev.ip_address.as_deref(), Some("192.168.1.42"));
        assert_eq!(ev.hostname.as_deref(), Some("laptop"));
        assert_eq!(ev.profile.as_deref(), Some("parents"));
        assert_eq!(ev.details.as_deref(), Some("granted"));
        assert!(ev.duid.is_none());
    }
}
