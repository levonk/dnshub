//! DUID (DHCP Unique Identifier) parsing and generation per RFC 8415 §11.
//!
//! A DUID is a variable-length identifier carried in the ClientId (option 1)
//! and ServerId (option 2) DHCPv6 options. RFC 8415 defines four DUID
//! formats:
//!
//! | Type | Name        | Layout                                              |
//! |------|-------------|-----------------------------------------------------|
//! | 1    | DUID-LLT    | `type(2) | htype(2) | time(4) | link-layer-addr`    |
//! | 2    | DUID-EN     | `type(2) | enterprise-number(4) | identifier`       |
//! | 3    | DUID-LL     | `type(2) | htype(2) | link-layer-addr`             |
//! | 4    | DUID-UUID   | `type(2) | uuid(16)`                                |
//!
//! This module parses raw DUID bytes into a typed [`Duid`] enum and provides
//! helpers to generate a server DUID. The wire encoding/decoding itself is
//! delegated to [`dhcproto::v6::Duid`] for round-trip correctness.

use dhcproto::v6::duid::Duid as WireDuid;
use dhcproto::v6::HType;
use std::net::Ipv6Addr;

/// Parsed DUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Duid {
    /// DUID-LLT (type 1) — link-layer address with timestamp.
    Llt {
        htype: u16,
        time: u32,
        addr: Vec<u8>,
    },
    /// DUID-EN (type 2) — enterprise number + vendor-assigned identifier.
    En {
        enterprise: u32,
        id: Vec<u8>,
    },
    /// DUID-LL (type 3) — link-layer address (no timestamp).
    Ll {
        htype: u16,
        addr: Vec<u8>,
    },
    /// DUID-UUID (type 4).
    Uuid {
        uuid: [u8; 16],
    },
    /// An unrecognized DUID type. Stores the raw bytes verbatim.
    Unknown {
        type_code: u16,
        data: Vec<u8>,
    },
}

impl Duid {
    /// Parse raw DUID bytes into a typed [`Duid`].
    ///
    /// Returns [`Duid::Unknown`] for unrecognized type codes rather than
    /// an error, so the server can still echo back a client DUID it does
    /// not understand.
    pub fn parse(bytes: &[u8]) -> Self {
        if bytes.len() < 2 {
            return Duid::Unknown {
                type_code: 0,
                data: bytes.to_vec(),
            };
        }
        let type_code = u16::from_be_bytes([bytes[0], bytes[1]]);
        match type_code {
            1 => {
                // DUID-LLT: htype(2) + time(4) + addr
                if bytes.len() < 8 {
                    return Self::malformed(type_code, bytes);
                }
                let htype = u16::from_be_bytes([bytes[2], bytes[3]]);
                let time = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
                let addr = bytes[8..].to_vec();
                Duid::Llt { htype, time, addr }
            }
            2 => {
                // DUID-EN: enterprise(4) + id
                if bytes.len() < 6 {
                    return Self::malformed(type_code, bytes);
                }
                let enterprise = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
                let id = bytes[6..].to_vec();
                Duid::En { enterprise, id }
            }
            3 => {
                // DUID-LL: htype(2) + addr
                if bytes.len() < 4 {
                    return Self::malformed(type_code, bytes);
                }
                let htype = u16::from_be_bytes([bytes[2], bytes[3]]);
                let addr = bytes[4..].to_vec();
                Duid::Ll { htype, addr }
            }
            4 => {
                // DUID-UUID: 16 bytes
                if bytes.len() < 18 {
                    return Self::malformed(type_code, bytes);
                }
                let mut uuid = [0u8; 16];
                uuid.copy_from_slice(&bytes[2..18]);
                Duid::Uuid { uuid }
            }
            other => Self::malformed(other, bytes),
        }
    }

    fn malformed(type_code: u16, bytes: &[u8]) -> Self {
        Duid::Unknown {
            type_code,
            data: bytes.to_vec(),
        }
    }

    /// The RFC 8415 DUID type code (1=LLT, 2=EN, 3=LL, 4=UUID, 0=unknown).
    pub fn type_code(&self) -> u16 {
        match self {
            Duid::Llt { .. } => 1,
            Duid::En { .. } => 2,
            Duid::Ll { .. } => 3,
            Duid::Uuid { .. } => 4,
            Duid::Unknown { type_code, .. } => *type_code,
        }
    }

    /// Re-encode the parsed DUID back to its wire bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Duid::Llt { htype, time, addr } => {
                let mut v = Vec::with_capacity(8 + addr.len());
                v.extend_from_slice(&1u16.to_be_bytes());
                v.extend_from_slice(&htype.to_be_bytes());
                v.extend_from_slice(&time.to_be_bytes());
                v.extend_from_slice(addr);
                v
            }
            Duid::En { enterprise, id } => {
                let mut v = Vec::with_capacity(6 + id.len());
                v.extend_from_slice(&2u16.to_be_bytes());
                v.extend_from_slice(&enterprise.to_be_bytes());
                v.extend_from_slice(id);
                v
            }
            Duid::Ll { htype, addr } => {
                let mut v = Vec::with_capacity(4 + addr.len());
                v.extend_from_slice(&3u16.to_be_bytes());
                v.extend_from_slice(&htype.to_be_bytes());
                v.extend_from_slice(addr);
                v
            }
            Duid::Uuid { uuid } => {
                let mut v = Vec::with_capacity(18);
                v.extend_from_slice(&4u16.to_be_bytes());
                v.extend_from_slice(uuid);
                v
            }
            Duid::Unknown { data, .. } => data.clone(),
        }
    }

    /// Render the DUID as a stable hex string suitable for use as a SQLite
    /// key (the `duid` column in `dhcpv6_leases`).
    pub fn to_hex(&self) -> String {
        hex_encode(&self.to_bytes())
    }
}

/// Generate a server DUID-LLT from a hardware address.
///
/// Uses [`dhcproto::v6::Duid::link_layer_time`] so the wire encoding is
/// RFC-compliant. `time` is the DUID-LLT timestamp (seconds since
/// 2000-01-01 UTC per RFC 8415 §11.2).
pub fn server_duid_llt(htype: HType, time: u32, addr: Ipv6Addr) -> WireDuid {
    WireDuid::link_layer_time(htype, time, addr)
}

/// Generate a server DUID-EN from an enterprise number and identifier.
pub fn server_duid_en(enterprise: u32, id: &[u8]) -> WireDuid {
    WireDuid::enterprise(enterprise, id)
}

/// Generate a server DUID-LL from a hardware address.
pub fn server_duid_ll(htype: HType, addr: Ipv6Addr) -> WireDuid {
    WireDuid::link_layer(htype, addr)
}

/// Encode a byte slice as a lowercase hex string (no separators).
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duid_llt() {
        // type=1, htype=1 (Eth), time=0x5f5e0c00, addr=00:11:22:33:44:55
        let bytes = [
            0x00, 0x01, // type
            0x00, 0x01, // htype (Eth)
            0x5f, 0x5e, 0x0c, 0x00, // time
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, // addr
        ];
        let duid = Duid::parse(&bytes);
        match duid {
            Duid::Llt { htype, time, ref addr } => {
                assert_eq!(htype, 1);
                assert_eq!(time, 0x5f5e_0c00);
                assert_eq!(addr, &vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
            }
            other => panic!("expected Llt, got {other:?}"),
        }
        assert_eq!(duid.type_code(), 1);
        assert_eq!(duid.to_bytes(), bytes.to_vec());
    }

    #[test]
    fn parse_duid_en() {
        // type=2, enterprise=32473 (0x7ED9), id="dnshub"
        let bytes = [
            0x00, 0x02, // type
            0x00, 0x00, 0x7e, 0xd9, // enterprise 32473
            b'd', b'n', b's', b'h', b'u', b'b', // id
        ];
        let duid = Duid::parse(&bytes);
        match duid {
            Duid::En { enterprise, ref id } => {
                assert_eq!(enterprise, 32473);
                assert_eq!(id, &b"dnshub".to_vec());
            }
            other => panic!("expected En, got {other:?}"),
        }
        assert_eq!(duid.type_code(), 2);
        assert_eq!(duid.to_bytes(), bytes.to_vec());
    }

    #[test]
    fn parse_duid_ll() {
        // type=3, htype=1, addr=00:11:22:33:44:55
        let bytes = [
            0x00, 0x03, // type
            0x00, 0x01, // htype
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, // addr
        ];
        let duid = Duid::parse(&bytes);
        match duid {
            Duid::Ll { htype, ref addr } => {
                assert_eq!(htype, 1);
                assert_eq!(addr, &vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
            }
            other => panic!("expected Ll, got {other:?}"),
        }
        assert_eq!(duid.type_code(), 3);
        assert_eq!(duid.to_bytes(), bytes.to_vec());
    }

    #[test]
    fn parse_duid_uuid() {
        let uuid = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ];
        let mut bytes = vec![0x00, 0x04];
        bytes.extend_from_slice(&uuid);
        let duid = Duid::parse(&bytes);
        match duid {
            Duid::Uuid { uuid: u } => assert_eq!(u, uuid),
            other => panic!("expected Uuid, got {other:?}"),
        }
        assert_eq!(duid.type_code(), 4);
        assert_eq!(duid.to_bytes(), bytes);
    }

    #[test]
    fn parse_unknown_type() {
        // type=99 — unrecognized
        let bytes = [0x00, 0x63, 0xaa, 0xbb, 0xcc];
        let duid = Duid::parse(&bytes);
        match duid {
            Duid::Unknown { type_code, data } => {
                assert_eq!(type_code, 99);
                assert_eq!(data, bytes.to_vec());
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn parse_truncated_is_unknown() {
        let duid = Duid::parse(&[0x00]);
        assert!(matches!(duid, Duid::Unknown { .. }));
    }

    #[test]
    fn round_trip_all_types() {
        let samples: Vec<Vec<u8>> = vec![
            vec![
                0x00, 0x01, 0x00, 0x01, 0x5f, 0x5e, 0x0c, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44,
                0x55,
            ],
            vec![0x00, 0x02, 0x00, 0x00, 0x7e, 0xf9, b'd', b'n', b's'],
            vec![0x00, 0x03, 0x00, 0x01, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            {
                let mut v = vec![0x00, 0x04];
                v.extend_from_slice(&[1u8; 16]);
                v
            },
        ];
        for bytes in samples {
            let duid = Duid::parse(&bytes);
            assert_eq!(duid.to_bytes(), bytes, "round-trip failed for {duid:?}");
        }
    }

    #[test]
    fn server_duid_generation_round_trips() {
        let addr: Ipv6Addr = "fd00::1".parse().unwrap();
        let wire = server_duid_llt(HType::Eth, 0x5f5e_0c00, addr);
        let parsed = Duid::parse(wire.as_ref());
        match parsed {
            Duid::Llt { htype, time, .. } => {
                assert_eq!(htype, 1);
                assert_eq!(time, 0x5f5e_0c00);
            }
            other => panic!("expected Llt, got {other:?}"),
        }

        let wire_en = server_duid_en(32473, b"dnshub");
        let parsed_en = Duid::parse(wire_en.as_ref());
        assert!(matches!(parsed_en, Duid::En { enterprise: 32473, .. }));

        let wire_ll = server_duid_ll(HType::Eth, addr);
        let parsed_ll = Duid::parse(wire_ll.as_ref());
        assert!(matches!(parsed_ll, Duid::Ll { htype: 1, .. }));
    }

    #[test]
    fn hex_encoding_is_lowercase_no_separators() {
        let duid = Duid::parse(&[0x00, 0x03, 0x00, 0x01, 0xab, 0xcd]);
        assert_eq!(duid.to_hex(), "00030001abcd");
    }
}
