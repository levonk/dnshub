//! Classification rules used to auto-assign DHCP clients to policy profiles.
//!
//! A [`ClassificationRule`] describes a single match predicate. The DHCP
//! server evaluates rules in declaration order (see [`super::ClientClassifier`])
//! and the first matching rule wins — the client is assigned that rule's
//! profile.
//!
//! Three match kinds are supported (PRD section 4.4, lines 407-411):
//!
//! | Kind          | DHCP option   | Match mode   |
//! |---------------|---------------|--------------|
//! | [`VendorClass`]  | option 60 | prefix match |
//! | [`Oui`]          | MAC       | first 3 octets (OUI vendor prefix) |
//! | [`UserClass`]    | option 77 | exact match  |
//!
//! [`VendorClass`]: ClassificationRule::VendorClass
//! [`Oui`]: ClassificationRule::Oui
//! [`UserClass`]: ClassificationRule::UserClass

/// A single classification predicate.
///
/// Rules are compared against the raw bytes carried in the DHCP options
/// (vendor class / user class are `Vec<u8>` in dhcproto) and the client
/// hardware address (MAC). All byte comparisons are case-sensitive; callers
/// are expected to normalise OUI strings to lowercase before constructing an
/// [`Oui`](ClassificationRule::Oui) rule (see [`ClassificationRule::oui`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassificationRule {
    /// Prefix match on DHCP option 60 (vendor class identifier).
    ///
    /// e.g. `prefix = "Android-"` matches any device whose vendor class
    /// starts with `Android-`.
    VendorClass {
        /// The prefix that the vendor class (option 60) must start with.
        prefix: Vec<u8>,
    },
    /// Match on the first three octets of the client MAC (the OUI).
    ///
    /// e.g. `oui = [0xb8, 0x27, 0xeb]` matches Raspberry Pi Foundation
    /// devices.
    Oui {
        /// The 3-byte Organizationally Unique Identifier to match.
        oui: [u8; 3],
    },
    /// Exact match on DHCP option 77 (user class).
    ///
    /// e.g. `value = "guest"` matches a client that explicitly identifies
    /// itself with the user class `guest`.
    UserClass {
        /// The exact user class (option 77) byte sequence to match.
        value: Vec<u8>,
    },
}

impl ClassificationRule {
    /// Create a [`VendorClass`](ClassificationRule::VendorClass) rule from a
    /// string prefix. The prefix is compared as raw UTF-8 bytes against the
    /// leading bytes of option 60.
    pub fn vendor_class(prefix: &str) -> Self {
        ClassificationRule::VendorClass {
            prefix: prefix.as_bytes().to_vec(),
        }
    }

    /// Create an [`Oui`](ClassificationRule::Oui) rule from a 3-byte OUI.
    pub fn oui(oui: [u8; 3]) -> Self {
        ClassificationRule::Oui { oui }
    }

    /// Create a [`UserClass`](ClassificationRule::UserClass) rule from an
    /// exact-match string.
    pub fn user_class(value: &str) -> Self {
        ClassificationRule::UserClass {
            value: value.as_bytes().to_vec(),
        }
    }

    /// Parse an OUI from a colon- or hyphen-separated hex string such as
    /// `"B8:27:EB"` or `"00-11-22"`. The result is normalised to lowercase
    /// bytes.
    ///
    /// Returns `None` if the string does not contain exactly three octets
    /// or if any octet is not valid hexadecimal.
    pub fn parse_oui(s: &str) -> Option<[u8; 3]> {
        let parts: Vec<&str> = s.split([':', '-']).collect();
        if parts.len() != 3 {
            return None;
        }
        let mut oui = [0u8; 3];
        for (i, part) in parts.iter().enumerate() {
            oui[i] = u8::from_str_radix(part.trim(), 16).ok()?;
        }
        Some(oui)
    }

    /// Test whether this rule matches the supplied client attributes.
    ///
    /// - `mac` is the 6-byte client hardware address.
    /// - `vendor_class` is the raw bytes of DHCP option 60, if present.
    /// - `user_class` is the raw bytes of DHCP option 77, if present.
    pub fn matches(
        &self,
        mac: &[u8; 6],
        vendor_class: Option<&[u8]>,
        user_class: Option<&[u8]>,
    ) -> bool {
        match self {
            ClassificationRule::VendorClass { prefix } => {
                vendor_class
                    .map(|vc| vc.starts_with(prefix))
                    .unwrap_or(false)
            }
            ClassificationRule::Oui { oui } => mac[..3] == *oui,
            ClassificationRule::UserClass { value } => {
                user_class.map(|uc| uc == value).unwrap_or(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(octets: &[u8; 6]) -> [u8; 6] {
        *octets
    }

    #[test]
    fn vendor_class_prefix_match() {
        let rule = ClassificationRule::vendor_class("Android-");
        let mac = mac(&[0; 6]);
        assert!(rule.matches(&mac, Some(b"Android-Phone-14"), None));
        assert!(rule.matches(&mac, Some(b"Android-"), None));
        assert!(!rule.matches(&mac, Some(b"android-phone"), None)); // case-sensitive
        assert!(!rule.matches(&mac, Some(b"iOS"), None));
        assert!(!rule.matches(&mac, None, None));
    }

    #[test]
    fn oui_match_first_three_octets() {
        let rule = ClassificationRule::oui([0xb8, 0x27, 0xeb]);
        let rpi = mac(&[0xb8, 0x27, 0xeb, 0x12, 0x34, 0x56]);
        let other = mac(&[0x00, 0x27, 0xeb, 0x12, 0x34, 0x56]);
        assert!(rule.matches(&rpi, None, None));
        assert!(!rule.matches(&other, None, None));
        // OUI match ignores vendor/user class
        assert!(rule.matches(&rpi, Some(b"anything"), Some(b"anything")));
    }

    #[test]
    fn user_class_exact_match() {
        let rule = ClassificationRule::user_class("guest");
        let mac = mac(&[0; 6]);
        assert!(rule.matches(&mac, None, Some(b"guest")));
        assert!(!rule.matches(&mac, None, Some(b"Guest"))); // case-sensitive
        assert!(!rule.matches(&mac, None, Some(b"guest-network"))); // exact, not prefix
        assert!(!rule.matches(&mac, None, None));
    }

    #[test]
    fn parse_oui_colon_separated() {
        assert_eq!(
            ClassificationRule::parse_oui("B8:27:EB"),
            Some([0xb8, 0x27, 0xeb])
        );
    }

    #[test]
    fn parse_oui_hyphen_separated() {
        assert_eq!(
            ClassificationRule::parse_oui("00-11-22"),
            Some([0x00, 0x11, 0x22])
        );
    }

    #[test]
    fn parse_oui_lowercase() {
        assert_eq!(
            ClassificationRule::parse_oui("b8:27:eb"),
            Some([0xb8, 0x27, 0xeb])
        );
    }

    #[test]
    fn parse_oui_invalid_returns_none() {
        assert_eq!(ClassificationRule::parse_oui("B8:27"), None);
        assert_eq!(ClassificationRule::parse_oui("B8:27:EB:99"), None);
        assert_eq!(ClassificationRule::parse_oui("ZZ:27:EB"), None);
        assert_eq!(ClassificationRule::parse_oui(""), None);
    }
}
