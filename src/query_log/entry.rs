//! [`QueryLogEntry`] — a single DNS query log record.
//!
//! The field set mirrors the `query_log` SQLite schema defined in PRD
//! section 4.8 (lines 1213-1233): every DNS query is recorded with its
//! timestamp, client identity, queried domain, response status, block
//! metadata, upstream tier, latency, and cache hit flag.

use std::time::{SystemTime, UNIX_EPOCH};

/// A single DNS query log entry.
///
/// All timestamp fields are Unix milliseconds. `blocked`/`cached` are
/// stored as booleans in Rust and as `INTEGER` (0/1) in SQLite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryLogEntry {
    /// Row id assigned by SQLite (`None` for entries not yet inserted).
    pub id: Option<i64>,
    /// Unix-millisecond timestamp at which the query was received.
    pub timestamp: i64,
    /// Client IP address (e.g. `"192.168.1.20"`).
    pub client_ip: String,
    /// Resolved client hostname from the DHCP lease table, if known.
    pub client_name: Option<String>,
    /// Policy profile applied to this query, if any.
    pub profile: Option<String>,
    /// Queried domain name (e.g. `"example.com"`).
    pub domain: String,
    /// DNS query type as text (e.g. `"A"`, `"AAAA"`, `"MX"`).
    pub qtype: String,
    /// DNS response code as text (e.g. `"NOERROR"`, `"NXDOMAIN"`,
    /// `"SERVFAIL"`).
    pub response_code: String,
    /// Whether the query was blocked by a blocklist rule.
    pub blocked: bool,
    /// Block category (e.g. `"social"`, `"ads"`) when `blocked` is true.
    pub block_category: Option<String>,
    /// Blocklist source that matched, when `blocked` is true.
    pub block_source: Option<String>,
    /// Upstream tier that served the response (1 = primary). `None` for
    /// cached or blocked responses that did not reach an upstream.
    pub tier: Option<i32>,
    /// Resolution latency in milliseconds. `None` when not measured
    /// (e.g. cached or blocked responses).
    pub latency_ms: Option<i64>,
    /// Whether the response was served from the cache.
    pub cached: bool,
}

impl QueryLogEntry {
    /// Create a new entry with the given timestamp, client IP, domain,
    /// query type, and response code. All optional fields default to
    /// `None`/`false` and can be set via the builder methods below.
    pub fn new(
        timestamp: i64,
        client_ip: impl Into<String>,
        domain: impl Into<String>,
        qtype: impl Into<String>,
        response_code: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            timestamp,
            client_ip: client_ip.into(),
            client_name: None,
            profile: None,
            domain: domain.into(),
            qtype: qtype.into(),
            response_code: response_code.into(),
            blocked: false,
            block_category: None,
            block_source: None,
            tier: None,
            latency_ms: None,
            cached: false,
        }
    }

    /// Current time as Unix milliseconds.
    pub fn now_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Builder: set the resolved client hostname.
    pub fn with_client_name(mut self, name: impl Into<String>) -> Self {
        self.client_name = Some(name.into());
        self
    }

    /// Builder: set the policy profile.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Builder: mark this entry as blocked, optionally recording the
    /// block category and source list.
    pub fn blocked(mut self, category: Option<String>, source: Option<String>) -> Self {
        self.blocked = true;
        self.block_category = category;
        self.block_source = source;
        self
    }

    /// Builder: set the upstream tier that served the response.
    pub fn with_tier(mut self, tier: i32) -> Self {
        self.tier = Some(tier);
        self
    }

    /// Builder: set the resolution latency in milliseconds.
    pub fn with_latency_ms(mut self, latency_ms: i64) -> Self {
        self.latency_ms = Some(latency_ms);
        self
    }

    /// Builder: mark this entry as served from the cache.
    pub fn cached(mut self) -> Self {
        self.cached = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_required_fields() {
        let e = QueryLogEntry::new(123, "10.0.0.1", "example.com", "A", "NOERROR");
        assert_eq!(e.timestamp, 123);
        assert_eq!(e.client_ip, "10.0.0.1");
        assert_eq!(e.domain, "example.com");
        assert_eq!(e.qtype, "A");
        assert_eq!(e.response_code, "NOERROR");
        assert!(!e.blocked);
        assert!(!e.cached);
        assert!(e.client_name.is_none());
        assert!(e.profile.is_none());
        assert!(e.tier.is_none());
        assert!(e.latency_ms.is_none());
    }

    #[test]
    fn builders_set_optional_fields() {
        let e = QueryLogEntry::new(1, "10.0.0.1", "ads.com", "A", "NXDOMAIN")
            .with_client_name("laptop")
            .with_profile("kids")
            .blocked(Some("ads".to_string()), Some("stevenblack".to_string()))
            .with_latency_ms(0);
        assert!(e.blocked);
        assert_eq!(e.block_category.as_deref(), Some("ads"));
        assert_eq!(e.block_source.as_deref(), Some("stevenblack"));
        assert_eq!(e.client_name.as_deref(), Some("laptop"));
        assert_eq!(e.profile.as_deref(), Some("kids"));
        assert_eq!(e.latency_ms, Some(0));
    }

    #[test]
    fn cached_builder() {
        let e = QueryLogEntry::new(1, "10.0.0.1", "x.com", "A", "NOERROR")
            .with_tier(1)
            .cached();
        assert!(e.cached);
        assert_eq!(e.tier, Some(1));
    }

    #[test]
    fn now_millis_is_monotonicish() {
        let a = QueryLogEntry::now_millis();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = QueryLogEntry::now_millis();
        assert!(b >= a);
    }
}
