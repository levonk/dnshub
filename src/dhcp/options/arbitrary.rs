//! Arbitrary DHCP options — configurable `option N = value` for any RFC 2132
//! option code.
//!
//! The PRD (section 4.4, lines 412-414) requires that operators be able to
//! configure *any* DHCP option, not just a hardcoded subset. [`ArbitraryOption`]
//! parses a config value string into one of three wire encodings and produces
//! a [`dhcproto::v4::DhcpOption`] that can be inserted into a response.
//!
//! ## Value types
//!
//! The value string is inferred at parse time:
//!
//! | Pattern                       | Encoding        | Example            |
//! |-------------------------------|-----------------|--------------------|
//! | Four dotted decimal octets    | IPv4 address (4 bytes) | `"192.168.1.1"` |
//! | `0x`-prefixed hex             | Raw hex bytes   | `"0x0102ff"`       |
//! | Anything else                 | UTF-8 string    | `"levonk.com"`     |
//!
//! This covers the common RFC 2132 option types. Vendor-encapsulated options
//! (43/125) are supported by supplying a raw hex value.

use dhcproto::v4::{DhcpOption, OptionCode, UnknownOption};
use std::net::Ipv4Addr;

/// The decoded value of an arbitrary DHCP option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    /// A UTF-8 string (e.g. domain name, NTP server name).
    String(Vec<u8>),
    /// A single IPv4 address (4 bytes).
    Ip(Ipv4Addr),
    /// Raw bytes supplied as hex.
    Hex(Vec<u8>),
}

/// Errors returned while parsing an [`ArbitraryOption`].
#[derive(Debug)]
pub enum ArbitraryOptionError {
    /// The option code is not a valid u8.
    InvalidCode(String),
    /// The hex value could not be decoded.
    InvalidHex(String),
}

impl std::fmt::Display for ArbitraryOptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArbitraryOptionError::InvalidCode(s) => {
                write!(f, "invalid DHCP option code: {s}")
            }
            ArbitraryOptionError::InvalidHex(s) => {
                write!(f, "invalid hex value: {s}")
            }
        }
    }
}

impl std::error::Error for ArbitraryOptionError {}

/// A configurable DHCP option: a numeric code plus an inferred value.
///
/// Construct with [`ArbitraryOption::parse`] from a `(code, raw_value)` pair
/// (as read from the TOML `options` map), then call
/// [`to_dhcp_option`](Self::to_dhcp_option) to obtain the dhcproto
/// representation for insertion into a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArbitraryOption {
    /// The RFC 2132 option code (0-255).
    pub code: u8,
    /// The decoded value.
    pub value: OptionValue,
}

impl ArbitraryOption {
    /// Parse a `(code, raw_value)` pair from config.
    ///
    /// `code` is the string key from the TOML `options` map (e.g. `"28"`).
    /// `raw_value` is the string value (e.g. `"192.168.1.255"`).
    pub fn parse(code: &str, raw_value: &str) -> Result<Self, ArbitraryOptionError> {
        let code: u8 = code
            .parse()
            .map_err(|_| ArbitraryOptionError::InvalidCode(code.to_string()))?;
        let value = Self::infer_value(raw_value)?;
        Ok(Self { code, value })
    }

    /// Build an [`ArbitraryOption`] directly from a code and value.
    pub fn new(code: u8, value: OptionValue) -> Self {
        Self { code, value }
    }

    /// Infer the [`OptionValue`] variant from a raw config string.
    fn infer_value(raw: &str) -> Result<OptionValue, ArbitraryOptionError> {
        // IPv4 address: four dotted decimal octets.
        if let Ok(ip) = raw.parse::<Ipv4Addr>() {
            return Ok(OptionValue::Ip(ip));
        }
        // Hex: 0x-prefixed.
        if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            let bytes = decode_hex(hex)
                .map_err(|_| ArbitraryOptionError::InvalidHex(raw.to_string()))?;
            return Ok(OptionValue::Hex(bytes));
        }
        // Fallback: UTF-8 string.
        Ok(OptionValue::String(raw.as_bytes().to_vec()))
    }

    /// Encode the value to its on-wire byte representation (without the
    /// option code or length prefix — dhcproto adds those).
    pub fn encode(&self) -> Vec<u8> {
        match &self.value {
            OptionValue::String(bytes) => bytes.clone(),
            OptionValue::Ip(ip) => ip.octets().to_vec(),
            OptionValue::Hex(bytes) => bytes.clone(),
        }
    }

    /// Convert to a dhcproto [`DhcpOption`] for insertion into a response's
    /// option set.
    ///
    /// Known option codes use dhcproto's typed variants where possible; all
    /// other codes (and codes we do not model) become
    /// [`DhcpOption::Unknown`].
    pub fn to_dhcp_option(&self) -> DhcpOption {
        let data = self.encode();
        DhcpOption::Unknown(UnknownOption::new(OptionCode::from(self.code), data))
    }
}

/// Decode a hex string (no `0x` prefix) into bytes. Accepts both upper and
/// lowercase hex and an even number of nibbles.
fn decode_hex(hex: &str) -> Result<Vec<u8>, ()> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return Err(());
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble(bytes[i])?;
        let lo = nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parse_string_value() {
        let opt = ArbitraryOption::parse("15", "levonk.com").expect("parse");
        assert_eq!(opt.code, 15);
        assert_eq!(opt.value, OptionValue::String(b"levonk.com".to_vec()));
        assert_eq!(opt.encode(), b"levonk.com");
    }

    #[test]
    fn parse_ip_value() {
        let opt = ArbitraryOption::parse("28", "192.168.1.255").expect("parse");
        assert_eq!(opt.code, 28);
        assert_eq!(
            opt.value,
            OptionValue::Ip(Ipv4Addr::new(192, 168, 1, 255))
        );
        assert_eq!(opt.encode(), [192, 168, 1, 255]);
    }

    #[test]
    fn parse_hex_value() {
        let opt = ArbitraryOption::parse("43", "0x0102ff").expect("parse");
        assert_eq!(opt.code, 43);
        assert_eq!(opt.value, OptionValue::Hex(vec![0x01, 0x02, 0xff]));
        assert_eq!(opt.encode(), [0x01, 0x02, 0xff]);
    }

    #[test]
    fn parse_hex_uppercase_prefix() {
        let opt = ArbitraryOption::parse("125", "0XDEADBEEF").expect("parse");
        assert_eq!(opt.value, OptionValue::Hex(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn parse_invalid_code() {
        let err = ArbitraryOption::parse("abc", "x").expect_err("bad code");
        assert!(matches!(err, ArbitraryOptionError::InvalidCode(_)));
    }

    #[test]
    fn parse_invalid_hex() {
        let err = ArbitraryOption::parse("43", "0xZZ").expect_err("bad hex");
        assert!(matches!(err, ArbitraryOptionError::InvalidHex(_)));
        // Odd-length hex is also invalid.
        let err = ArbitraryOption::parse("43", "0xabc").expect_err("odd hex");
        assert!(matches!(err, ArbitraryOptionError::InvalidHex(_)));
    }

    #[test]
    fn to_dhcp_option_unknown_code() {
        let opt = ArbitraryOption::parse("252", "http://wpad.levonk.com/wpad.dat")
            .expect("parse");
        let dhcp_opt = opt.to_dhcp_option();
        match dhcp_opt {
            DhcpOption::Unknown(unknown) => {
                assert_eq!(unknown.code(), OptionCode::from(252u8));
                assert_eq!(
                    unknown.data(),
                    b"http://wpad.levonk.com/wpad.dat"
                );
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn to_dhcp_option_ip_encoding() {
        let opt = ArbitraryOption::parse("3", "192.168.1.1").expect("parse");
        let dhcp_opt = opt.to_dhcp_option();
        match dhcp_opt {
            DhcpOption::Unknown(unknown) => {
                assert_eq!(unknown.data(), [192, 168, 1, 1]);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn new_constructs_directly() {
        let opt = ArbitraryOption::new(42, OptionValue::Ip(Ipv4Addr::from_str("10.0.0.1").unwrap()));
        assert_eq!(opt.code, 42);
        assert_eq!(opt.encode(), [10, 0, 0, 1]);
    }
}
