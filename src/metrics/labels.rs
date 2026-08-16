//! Label normalization helpers for dnshub metrics.
//!
//! Prometheus label cardinality must be bounded to keep memory and scrape
//! payload sizes reasonable. Raw client IP addresses are **never** used as
//! metric labels — they would create an unbounded number of label series and
//! leak personally-identifiable information into the scrape output (PRD
//! section 4.6).
//!
//! Instead, every metric that needs a per-client dimension uses a
//! **client tag**: the profile name when one is assigned (e.g. `kids`,
//! `iot`, `guest`), falling back to the client hostname, and finally to the
//! sentinel `"unknown"`. Profile names are bounded by the configuration
//! (parents, kids, iot, guest, default), and hostnames are bounded by the
//! DHCP lease table, so the total cardinality is small and stable.
//!
//! [`client_to_tag`] is the single entry point used by all metric recording
//! sites in [`super::counters`].

/// Sentinel label value used when no profile or hostname is available.
pub const UNKNOWN_TAG: &str = "unknown";

/// Maximum length of a normalized label value. Values longer than this are
/// truncated to prevent pathological hostnames from inflating series names.
const MAX_LABEL_LEN: usize = 64;

/// Normalize a client identity into a stable, bounded metric label value.
///
/// The function prefers, in order:
///
/// 1. **`profile`** — the profile name assigned to the client (e.g. `kids`,
///    `iot`, `guest`). This is the primary, bounded dimension.
/// 2. **`hostname`** — the DHCP hostname of the client, used when no profile
///    is assigned. Hostnames are bounded by the lease table.
/// 3. [`UNKNOWN_TAG`] — the literal `"unknown"` sentinel, used when neither
///    a profile nor a hostname is known.
///
/// The raw client IP (`ip`) is accepted so callers can pass it through
/// uniformly, but it is **never** used as a label value. It is only present
/// in the signature to make call sites self-documenting and to support
/// future hashed/bucketed fallbacks without changing the API.
///
/// # Arguments
///
/// * `ip` — The raw client IP address (e.g. `"192.168.1.42"`). Never emitted.
/// * `hostname` — The client hostname from DHCP, if known.
/// * `profile` — The assigned profile name, if any.
///
/// # Returns
///
/// A stable, non-empty label string with no raw IP. Long values are
/// truncated to [`MAX_LABEL_LEN`] characters.
///
/// # Examples
///
/// ```
/// use dnshub::metrics::labels::client_to_tag;
///
/// // Profile takes priority.
/// assert_eq!(client_to_tag("10.0.0.5", Some("laptop"), Some("kids")), "kids");
/// // Hostname is used when no profile is assigned.
/// assert_eq!(client_to_tag("10.0.0.5", Some("phone"), None), "phone");
/// // Unknown sentinel when neither is available.
/// assert_eq!(client_to_tag("10.0.0.5", None, None), "unknown");
/// ```
pub fn client_to_tag(ip: &str, hostname: Option<&str>, profile: Option<&str>) -> String {
    let _ = ip; // Accepted for API symmetry; never used as a label.

    let raw = profile
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| hostname.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or(UNKNOWN_TAG);

    truncate_label(raw)
}

/// Truncate a label value so the total byte length (including a trailing
/// ellipsis marker when truncation occurs) does not exceed [`MAX_LABEL_LEN`].
///
/// The ellipsis `…` (U+2026) is 3 bytes in UTF-8, so when truncation is
/// needed the original value is cut at `MAX_LABEL_LEN - 3` bytes (adjusted
/// to a char boundary) and the ellipsis is appended, yielding a total of at
/// most `MAX_LABEL_LEN` bytes.
fn truncate_label(value: &str) -> String {
    if value.len() <= MAX_LABEL_LEN {
        return value.to_string();
    }

    const ELLIPSIS: &str = "…";
    let ellipsis_len = ELLIPSIS.as_bytes().len();
    let budget = MAX_LABEL_LEN.saturating_sub(ellipsis_len);

    // Truncate at a char boundary within the budget to avoid splitting
    // multi-byte characters.
    let mut end = budget;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = value[..end].to_string();
    out.push_str(ELLIPSIS);
    out
}

/// Normalize a free-form string into a safe label value.
///
/// This is a thin wrapper around [`truncate_label`] for callers that have a
/// single candidate value (e.g. an upstream name) and want consistent
/// truncation behavior. Empty input yields [`UNKNOWN_TAG`].
pub fn normalize_label(value: Option<&str>) -> String {
    let raw = value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(UNKNOWN_TAG);
    truncate_label(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_takes_priority_over_hostname() {
        assert_eq!(
            client_to_tag("192.168.1.10", Some("laptop"), Some("kids")),
            "kids"
        );
    }

    #[test]
    fn hostname_used_when_no_profile() {
        assert_eq!(
            client_to_tag("192.168.1.10", Some("phone"), None),
            "phone"
        );
    }

    #[test]
    fn unknown_when_neither_profile_nor_hostname() {
        assert_eq!(client_to_tag("192.168.1.10", None, None), "unknown");
    }

    #[test]
    fn empty_profile_falls_back_to_hostname() {
        assert_eq!(
            client_to_tag("192.168.1.10", Some("tablet"), Some("")),
            "tablet"
        );
    }

    #[test]
    fn whitespace_only_profile_falls_back_to_hostname() {
        assert_eq!(
            client_to_tag("192.168.1.10", Some("tablet"), Some("   ")),
            "tablet"
        );
    }

    #[test]
    fn empty_hostname_falls_back_to_unknown() {
        assert_eq!(client_to_tag("192.168.1.10", Some(""), Some("")), "unknown");
    }

    #[test]
    fn whitespace_only_hostname_falls_back_to_unknown() {
        assert_eq!(
            client_to_tag("192.168.1.10", Some("   "), None),
            "unknown"
        );
    }

    #[test]
    fn raw_ip_is_never_returned() {
        // Even with no profile/hostname, the IP must not leak into the label.
        let tag = client_to_tag("10.0.0.42", None, None);
        assert!(!tag.contains("10.0.0.42"));
        assert_eq!(tag, "unknown");
    }

    #[test]
    fn leading_trailing_whitespace_is_trimmed() {
        assert_eq!(
            client_to_tag("10.0.0.1", Some("  desktop  "), Some("  iot  ")),
            "iot"
        );
        assert_eq!(
            client_to_tag("10.0.0.1", Some("  desktop  "), None),
            "desktop"
        );
    }

    #[test]
    fn long_hostname_is_truncated() {
        let long = "a".repeat(200);
        let tag = client_to_tag("10.0.0.1", Some(&long), None);
        assert!(
            tag.len() <= MAX_LABEL_LEN,
            "truncated tag should be at most MAX_LABEL_LEN bytes, got {} ({} bytes)",
            tag,
            tag.len()
        );
        assert!(tag.ends_with('…'));
    }

    #[test]
    fn truncation_preserves_char_boundaries() {
        // Multi-byte characters near the boundary must not be split.
        let long = "é".repeat(100); // each 'é' is 2 bytes
        let tag = client_to_tag("10.0.0.1", Some(&long), None);
        // The result must be valid UTF-8 (String guarantees this) and bounded.
        assert!(tag.len() <= MAX_LABEL_LEN);
        assert!(tag.ends_with('…'));
    }

    #[test]
    fn exactly_max_len_is_not_truncated() {
        let exact = "a".repeat(MAX_LABEL_LEN);
        let tag = client_to_tag("10.0.0.1", Some(&exact), None);
        assert_eq!(tag, exact);
        assert!(!tag.ends_with('…'));
    }

    #[test]
    fn normalize_label_some_value() {
        assert_eq!(normalize_label(Some("unbound")), "unbound");
    }

    #[test]
    fn normalize_label_none_yields_unknown() {
        assert_eq!(normalize_label(None), "unknown");
    }

    #[test]
    fn normalize_label_empty_yields_unknown() {
        assert_eq!(normalize_label(Some("")), "unknown");
        assert_eq!(normalize_label(Some("   ")), "unknown");
    }

    #[test]
    fn normalize_label_trims_whitespace() {
        assert_eq!(normalize_label(Some("  odoh  ")), "odoh");
    }

    #[test]
    fn normalize_label_truncates_long_values() {
        let long = "x".repeat(300);
        let tag = normalize_label(Some(&long));
        assert!(tag.len() <= MAX_LABEL_LEN);
        assert!(tag.ends_with('…'));
    }

    #[test]
    fn unknown_tag_constant_is_stable() {
        assert_eq!(UNKNOWN_TAG, "unknown");
    }
}
