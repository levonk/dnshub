//! Policy evaluation engine (PRD section 4.5, lines 1055-1062).
//!
//! [`PolicyEngine::evaluate`] applies a [`PolicyProfile`] to a queried domain
//! and returns a [`PolicyDecision`]. The evaluation order is:
//!
//! 1. **custom_allowlist** — if the domain matches, allow immediately
//!    (skip all blocklist checks).
//! 2. **custom_blocklist** — if the domain matches, block immediately.
//! 3. **category blocklists** — if the domain's categories intersect the
//!    profile's `blocked_categories`, block — *unless* the profile's
//!    `allowed_categories` contains `"all"` (which disables category checks).
//! 4. Otherwise **allow** (forward to upstream).

use crate::blocklist::{BlocklistMetadata, BlocklistStore};
use crate::policy::profile::PolicyProfile;
use std::sync::Arc;

/// The outcome of evaluating a policy for a single query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// The query is allowed and should be forwarded.
    Allow,
    /// The query is blocked; return REFUSED/NXDOMAIN to the client.
    Block,
    /// The query should be redirected to a different upstream tier.
    ///
    /// (Reserved for future use; currently treated like [`Allow`].)
    Redirect,
}

/// The reason a particular decision was reached, for logging/metrics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyReason {
    /// Matched the custom allowlist.
    CustomAllowlist,
    /// Matched the custom blocklist.
    CustomBlocklist,
    /// Matched a blocked category in the blocklist store.
    CategoryBlock,
    /// No rule matched; default allow.
    DefaultAllow,
    /// `allowed_categories` contains `"all"`; category check skipped.
    AllCategoriesAllowed,
}

/// A fully-resolved policy decision with its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResult {
    /// The decision (allow/block/redirect).
    pub decision: PolicyDecision,
    /// Why the decision was reached.
    pub reason: PolicyReason,
}

impl PolicyResult {
    /// Convenience: is this an allow?
    pub fn is_allow(&self) -> bool {
        matches!(self.decision, PolicyDecision::Allow | PolicyDecision::Redirect)
    }

    /// Convenience: is this a block?
    pub fn is_block(&self) -> bool {
        matches!(self.decision, PolicyDecision::Block)
    }
}

/// The policy evaluation engine.
///
/// Holds an [`Arc`] reference to a [`BlocklistStore`] so it can check whether
/// a queried domain is in a blocked category. The store is shared with the
/// blocklist subsystem and may be hot-swapped externally (story 02-004).
pub struct PolicyEngine {
    blocklist: Arc<dyn BlocklistStore>,
}

impl PolicyEngine {
    /// Create a new engine backed by the given blocklist store.
    pub fn new(blocklist: Arc<dyn BlocklistStore>) -> Self {
        Self { blocklist }
    }

    /// Evaluate `domain` against `profile`, returning a [`PolicyResult`].
    ///
    /// `domain` should be the forward domain name in lowercase without a
    /// trailing dot (e.g. `ads.example.com`).
    pub fn evaluate(&self, domain: &str, profile: &PolicyProfile) -> PolicyResult {
        let normalized = normalize_domain(domain);

        // 1. Custom allowlist — allow immediately.
        if matches_any_pattern(&normalized, &profile.custom_allowlist) {
            return PolicyResult {
                decision: PolicyDecision::Allow,
                reason: PolicyReason::CustomAllowlist,
            };
        }

        // 2. Custom blocklist — block immediately.
        if matches_any_pattern(&normalized, &profile.custom_blocklist) {
            return PolicyResult {
                decision: PolicyDecision::Block,
                reason: PolicyReason::CustomBlocklist,
            };
        }

        // 3. Category blocklists.
        //    If allowed_categories contains "all", skip category checks.
        if profile.allows_all_categories() {
            return PolicyResult {
                decision: PolicyDecision::Allow,
                reason: PolicyReason::AllCategoriesAllowed,
            };
        }

        // Consult the blocklist store for category metadata.
        if let Ok(Some(meta)) = self.blocklist.lookup(&normalized) {
            if category_intersects(&meta, profile) {
                return PolicyResult {
                    decision: PolicyDecision::Block,
                    reason: PolicyReason::CategoryBlock,
                };
            }
        }

        // 4. Default allow.
        PolicyResult {
            decision: PolicyDecision::Allow,
            reason: PolicyReason::DefaultAllow,
        }
    }
}

/// Normalize a domain: lowercase, strip trailing dot, trim whitespace.
fn normalize_domain(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Check whether `domain` matches any pattern in `patterns`.
///
/// Patterns support glob-style wildcards:
/// - `*.example.com` matches `foo.example.com` and `bar.baz.example.com`
///   (any subdomain), but **not** `example.com` itself.
/// - `example.com` matches exactly `example.com`.
/// - A leading `*.` is the only wildcard form supported.
pub fn matches_any_pattern(domain: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| matches_pattern(domain, p))
}

/// Check whether `domain` matches a single glob `pattern`.
pub fn matches_pattern(domain: &str, pattern: &str) -> bool {
    let pat = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    let dom = domain.trim().trim_end_matches('.').to_ascii_lowercase();

    if let Some(suffix) = pat.strip_prefix("*.") {
        // *.example.com matches any subdomain of example.com.
        // The domain must have at least one label more than the suffix.
        if dom == suffix {
            return false;
        }
        dom == suffix || dom.ends_with(&format!(".{suffix}"))
    } else {
        dom == pat
    }
}

/// Check whether the blocklist metadata's category bitmap intersects the
/// profile's `blocked_categories`.
///
/// In this story the blocklist store does not yet populate category names
/// (story 02-002 adds the category bitmap). When `meta.categories == 0` (no
/// categories set), we treat the domain as blocked-by-default if the profile
/// has any `blocked_categories` at all — this lets the engine work end-to-end
/// before category bitmaps are populated.
fn category_intersects(meta: &BlocklistMetadata, profile: &PolicyProfile) -> bool {
    if profile.blocked_categories.is_empty() {
        return false;
    }
    if meta.categories == 0 {
        // No category bitmap yet: treat "domain is in blocklist" as a match
        // for any non-empty blocked_categories list.
        true
    } else {
        // Category bitmap is set: check if any blocked category bit is on.
        // The mapping from category name → bit index is defined in story
        // 02-002. For now, any non-zero intersection means blocked.
        meta.categories != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::{BlocklistError, BlocklistMetadata, BlocklistStore};

    /// A mock blocklist store for unit testing the engine.
    struct MockBlocklist {
        entries: std::collections::HashMap<String, BlocklistMetadata>,
    }

    impl MockBlocklist {
        fn new() -> Self {
            Self {
                entries: std::collections::HashMap::new(),
            }
        }
        fn with(mut self, domain: &str, meta: BlocklistMetadata) -> Self {
            self.entries.insert(domain.to_string(), meta);
            self
        }
    }

    impl BlocklistStore for MockBlocklist {
        fn lookup(&self, domain: &str) -> Result<Option<BlocklistMetadata>, BlocklistError> {
            Ok(self.entries.get(domain).copied())
        }
        fn len(&self) -> Result<u64, BlocklistError> {
            Ok(self.entries.len() as u64)
        }
    }

    fn meta(categories: u32) -> BlocklistMetadata {
        BlocklistMetadata {
            categories,
            sources: 1,
            first_seen: 0,
            last_updated: 0,
        }
    }

    fn make_engine(store: MockBlocklist) -> PolicyEngine {
        PolicyEngine::new(Arc::new(store))
    }

    #[test]
    fn test_custom_allowlist_overrides_blocklist() {
        let store = MockBlocklist::new().with("ads.example.com", meta(1));
        let engine = make_engine(store);
        let profile = PolicyProfile {
            custom_allowlist: vec!["ads.example.com".to_string()],
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        let r = engine.evaluate("ads.example.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Allow);
        assert_eq!(r.reason, PolicyReason::CustomAllowlist);
    }

    #[test]
    fn test_custom_blocklist_blocks() {
        let store = MockBlocklist::new();
        let engine = make_engine(store);
        let profile = PolicyProfile {
            custom_blocklist: vec!["tiktok.com".to_string()],
            ..Default::default()
        };
        let r = engine.evaluate("tiktok.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Block);
        assert_eq!(r.reason, PolicyReason::CustomBlocklist);
    }

    #[test]
    fn test_category_block() {
        let store = MockBlocklist::new().with("ads.example.com", meta(0));
        let engine = make_engine(store);
        let profile = PolicyProfile {
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        let r = engine.evaluate("ads.example.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Block);
        assert_eq!(r.reason, PolicyReason::CategoryBlock);
    }

    #[test]
    fn test_allowed_all_skips_category_check() {
        let store = MockBlocklist::new().with("ads.example.com", meta(1));
        let engine = make_engine(store);
        let profile = PolicyProfile {
            blocked_categories: vec!["ads".to_string()],
            allowed_categories: vec!["all".to_string()],
            ..Default::default()
        };
        let r = engine.evaluate("ads.example.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Allow);
        assert_eq!(r.reason, PolicyReason::AllCategoriesAllowed);
    }

    #[test]
    fn test_default_allow() {
        let store = MockBlocklist::new();
        let engine = make_engine(store);
        let profile = PolicyProfile {
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        let r = engine.evaluate("clean.example.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Allow);
        assert_eq!(r.reason, PolicyReason::DefaultAllow);
    }

    #[test]
    fn test_wildcard_allowlist() {
        let store = MockBlocklist::new().with("a.scratch.mit.edu", meta(0));
        let engine = make_engine(store);
        let profile = PolicyProfile {
            custom_allowlist: vec!["*.scratch.mit.edu".to_string()],
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        let r = engine.evaluate("a.scratch.mit.edu", &profile);
        assert_eq!(r.decision, PolicyDecision::Allow);
        assert_eq!(r.reason, PolicyReason::CustomAllowlist);
    }

    #[test]
    fn test_wildcard_does_not_match_base() {
        let store = MockBlocklist::new();
        let engine = make_engine(store);
        let profile = PolicyProfile {
            custom_allowlist: vec!["*.example.com".to_string()],
            ..Default::default()
        };
        // *.example.com should NOT match example.com itself.
        let r = engine.evaluate("example.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Allow);
        assert_eq!(r.reason, PolicyReason::DefaultAllow);
    }

    #[test]
    fn test_normalize_domain() {
        assert_eq!(normalize_domain("ADS.Example.COM."), "ads.example.com");
        assert_eq!(normalize_domain("  example.com  "), "example.com");
    }

    #[test]
    fn test_empty_blocked_categories_never_blocks() {
        let store = MockBlocklist::new().with("ads.example.com", meta(1));
        let engine = make_engine(store);
        let profile = PolicyProfile {
            blocked_categories: vec![],
            ..Default::default()
        };
        let r = engine.evaluate("ads.example.com", &profile);
        assert_eq!(r.decision, PolicyDecision::Allow);
    }
}
