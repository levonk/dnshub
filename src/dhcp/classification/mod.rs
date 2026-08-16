//! Client classification — auto-assign DHCP clients to policy profiles.
//!
//! The [`ClientClassifier`] holds an ordered list of
//! [`ClassificationRule`]s, each paired with a profile name. When a DHCP
//! request arrives, the server calls [`ClientClassifier::classify`] with the
//! client's MAC address and the vendor class (option 60) / user class
//! (option 77) values from the request. Rules are evaluated in declaration
//! order and the **first match wins** — the client is assigned that rule's
//! profile. If no rule matches, `None` is returned and the client falls back
//! to the pool's `default_profile` (or the global default).
//!
//! This implements PRD section 4.4 (lines 407-411):
//! - vendor class (option 60) prefix match
//! - MAC OUI (first 3 octets) match
//! - user class (option 77) exact match

pub mod rules;

pub use rules::ClassificationRule;

use crate::config::ClassifyConfig;

/// A profile name (e.g. `"phones"`, `"iot"`, `"guest"`).
pub type ProfileName = String;

/// Ordered set of classification rules mapping clients to profiles.
///
/// Rules are evaluated in insertion order; the first matching rule
/// determines the profile. Construct one explicitly with [`new`](Self::new)
/// and [`add_rule`](Self::add_rule), or build it from config with
/// [`from_config`](Self::from_config).
#[derive(Debug, Clone, Default)]
pub struct ClientClassifier {
    rules: Vec<(ClassificationRule, ProfileName)>,
}

impl ClientClassifier {
    /// Create an empty classifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a rule that assigns `profile` to any client matching `rule`.
    pub fn add_rule(&mut self, rule: ClassificationRule, profile: impl Into<ProfileName>) {
        self.rules.push((rule, profile.into()));
    }

    /// Build a classifier from `[[dhcp.classify]]` config entries.
    ///
    /// Each entry's `match` table may contain exactly one of `vendor_class`,
    /// `oui`, or `user_class`. Entries with no matcher or an unparseable OUI
    /// are skipped (and logged at `warn` level).
    pub fn from_config(entries: &[ClassifyConfig]) -> Self {
        let mut classifier = Self::new();
        for entry in entries {
            let Some(rule) = entry.to_rule() else {
                tracing::warn!(
                    profile = %entry.profile,
                    "classification entry has no valid matcher, skipping"
                );
                continue;
            };
            classifier.add_rule(rule, entry.profile.clone());
        }
        classifier
    }

    /// Classify a client, returning the profile assigned by the first
    /// matching rule, or `None` if no rule matches.
    ///
    /// - `mac` — 6-byte client hardware address.
    /// - `vendor_class` — raw bytes of DHCP option 60, if present.
    /// - `user_class` — raw bytes of DHCP option 77, if present.
    pub fn classify(
        &self,
        mac: &[u8; 6],
        vendor_class: Option<&[u8]>,
        user_class: Option<&[u8]>,
    ) -> Option<&str> {
        for (rule, profile) in &self.rules {
            if rule.matches(mac, vendor_class, user_class) {
                tracing::debug!(
                    mac = ?mac,
                    ?rule,
                    profile = %profile,
                    "classification matched"
                );
                return Some(profile.as_str());
            }
        }
        tracing::debug!(mac = ?mac, "no classification rule matched");
        None
    }

    /// Number of rules registered.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the classifier has any rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ClassifyConfig, ClassifyMatch};

    #[test]
    fn first_match_wins() {
        let mut classifier = ClientClassifier::new();
        classifier.add_rule(ClassificationRule::vendor_class("Android-"), "phones");
        classifier.add_rule(ClassificationRule::oui([0xb8, 0x27, 0xeb]), "iot");

        let android_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let profile = classifier.classify(&android_mac, Some(b"Android-Phone"), None);
        assert_eq!(profile, Some("phones"));
    }

    #[test]
    fn oui_rule_matches_after_vendor_class_misses() {
        let mut classifier = ClientClassifier::new();
        classifier.add_rule(ClassificationRule::vendor_class("Android-"), "phones");
        classifier.add_rule(ClassificationRule::oui([0xb8, 0x27, 0xeb]), "iot");

        // RPi MAC but no vendor class → first rule misses, OUI rule matches.
        let rpi_mac = [0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56];
        let profile = classifier.classify(&rpi_mac, Some(b"Linux-6.6"), None);
        assert_eq!(profile, Some("iot"));
    }

    #[test]
    fn no_match_returns_none() {
        let mut classifier = ClientClassifier::new();
        classifier.add_rule(ClassificationRule::vendor_class("Android-"), "phones");

        let mac = [0; 6];
        assert_eq!(classifier.classify(&mac, Some(b"iOS"), None), None);
        assert_eq!(classifier.classify(&mac, None, None), None);
    }

    #[test]
    fn user_class_exact_match() {
        let mut classifier = ClientClassifier::new();
        classifier.add_rule(ClassificationRule::user_class("guest"), "guest");

        let mac = [0; 6];
        assert_eq!(classifier.classify(&mac, None, Some(b"guest")), Some("guest"));
        assert_eq!(classifier.classify(&mac, None, Some(b"Guest")), None);
    }

    #[test]
    fn empty_classifier_returns_none() {
        let classifier = ClientClassifier::new();
        let mac = [0; 6];
        assert_eq!(classifier.classify(&mac, Some(b"anything"), Some(b"anything")), None);
        assert!(classifier.is_empty());
        assert_eq!(classifier.len(), 0);
    }

    #[test]
    fn from_config_builds_classifier() {
        let entries = vec![
            ClassifyConfig {
                match_: ClassifyMatch {
                    vendor_class: Some("Android-".to_string()),
                    oui: None,
                    user_class: None,
                },
                profile: "phones".to_string(),
            },
            ClassifyConfig {
                match_: ClassifyMatch {
                    vendor_class: None,
                    oui: Some("B8:27:EB".to_string()),
                    user_class: None,
                },
                profile: "iot".to_string(),
            },
            ClassifyConfig {
                match_: ClassifyMatch {
                    vendor_class: None,
                    oui: None,
                    user_class: Some("guest".to_string()),
                },
                profile: "guest".to_string(),
            },
        ];
        let classifier = ClientClassifier::from_config(&entries);
        assert_eq!(classifier.len(), 3);

        let android_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        assert_eq!(
            classifier.classify(&android_mac, Some(b"Android-Phone"), None),
            Some("phones")
        );

        let rpi_mac = [0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56];
        assert_eq!(
            classifier.classify(&rpi_mac, Some(b"Linux"), None),
            Some("iot")
        );

        let guest_mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        assert_eq!(
            classifier.classify(&guest_mac, None, Some(b"guest")),
            Some("guest")
        );
    }

    #[test]
    fn from_config_skips_invalid_entries() {
        let entries = vec![
            ClassifyConfig {
                match_: ClassifyMatch {
                    vendor_class: None,
                    oui: Some("not-an-oui".to_string()),
                    user_class: None,
                },
                profile: "iot".to_string(),
            },
            ClassifyConfig {
                match_: ClassifyMatch {
                    vendor_class: Some("Android-".to_string()),
                    oui: None,
                    user_class: None,
                },
                profile: "phones".to_string(),
            },
        ];
        let classifier = ClientClassifier::from_config(&entries);
        // Only the valid vendor_class entry survives.
        assert_eq!(classifier.len(), 1);
    }
}
