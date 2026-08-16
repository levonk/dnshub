//! ICMPv6 Router Advertisement message builder.
//!
//! Constructs raw RA messages per RFC 4861 (Neighbor Discovery for
//! IPv6) with the following options:
//!
//! - **Prefix Information** (option type 3, RFC 4861 §4.6.2): advertises
//!   the on-link prefix with preferred/valid lifetimes for SLAAC.
//! - **RDNSS** (option type 25, RFC 8106): recursive DNS server
//!   addresses.
//! - **DNSSL** (option type 31, RFC 8106): DNS search list domains.
//!
//! The builder produces the ICMPv6 payload only (starting with the
//! type field). The IPv6 layer and ICMPv6 checksum are handled by the
//! kernel when sending via a raw `IPPROTO_ICMPV6` socket (the kernel
//! computes the checksum and fills in the source address for
//! link-local sockets).

use std::net::Ipv6Addr;

/// ICMPv6 message type for Router Advertisement (RFC 4861).
pub const ICMPV6_RA_TYPE: u8 = 134;

/// ICMPv6 message type for Router Solicitation (RFC 4861).
pub const ICMPV6_RS_TYPE: u8 = 133;

/// ND option type: Prefix Information.
pub const ND_OPT_PREFIX_INFORMATION: u8 = 3;

/// ND option type: RDNSS (RFC 8106).
pub const ND_OPT_RDNSS: u8 = 25;

/// ND option type: DNSSL (RFC 8106).
pub const ND_OPT_DNSSL: u8 = 31;

/// Flag bits for the RA header (RFC 4861 §4.2).
pub const RA_FLAG_MANAGED: u8 = 0x80;
pub const RA_FLAG_OTHER: u8 = 0x40;

/// Flag bits for the Prefix Information option (RFC 4861 §4.6.2).
pub const PREFIX_FLAG_ON_LINK: u8 = 0x80;
pub const PREFIX_FLAG_AUTONOMOUS: u8 = 0x40;

/// All-nodes multicast address `ff02::1`.
pub const ALL_NODES_MULTICAST: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);

/// Build an ICMPv6 Router Advertisement message.
///
/// # Arguments
///
/// - `cur_hop_limit` — default hop limit to advertise (0 means
///   unspecified).
/// - `flags` — RA header flags (managed/other config bits).
/// - `router_lifetime_secs` — router lifetime in seconds (0 = not a
///   default router).
/// - `reachable_time_secs` — reachable time in seconds (0 = unspecified).
/// - `retrans_timer_secs` — retransmit timer in seconds (0 = unspecified).
/// - `prefix` — optional `(address, prefix_length, preferred_lifetime,
///   valid_lifetime)` tuple for the Prefix Information option.
/// - `rdnss_lifetime_secs` — lifetime for RDNSS option.
/// - `rdnss` — recursive DNS server addresses.
/// - `dnssl_lifetime_secs` — lifetime for DNSSL option.
/// - `dnssl` — domain search list entries.
///
/// Returns the raw ICMPv6 message bytes (without IPv6 header or
/// checksum — the kernel fills the checksum for raw ICMPv6 sockets).
#[allow(clippy::too_many_arguments)]
pub fn build_router_advertisement(
    cur_hop_limit: u8,
    flags: u8,
    router_lifetime_secs: u16,
    reachable_time_secs: u32,
    retrans_timer_secs: u32,
    prefix: Option<(Ipv6Addr, u8, u32, u32)>,
    rdnss_lifetime_secs: u32,
    rdnss: &[Ipv6Addr],
    dnssl_lifetime_secs: u32,
    dnssl: &[String],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);

    // --- ICMPv6 RA header (16 bytes) ---
    // Type (1) + Code (1) + Checksum (2, kernel-filled) + Cur Hop Limit (1)
    // + Flags (1) + Router Lifetime (2) + Reachable Time (4) + Retrans Timer (4)
    buf.push(ICMPV6_RA_TYPE); // type
    buf.push(0); // code
    buf.extend_from_slice(&[0u8, 0]); // checksum (placeholder, kernel fills)
    buf.push(cur_hop_limit); // cur hop limit
    buf.push(flags); // flags
    buf.extend_from_slice(&router_lifetime_secs.to_be_bytes()); // router lifetime
    buf.extend_from_slice(&reachable_time_secs.to_be_bytes()); // reachable time
    buf.extend_from_slice(&retrans_timer_secs.to_be_bytes()); // retrans timer

    // --- Prefix Information option (32 bytes) ---
    if let Some((addr, prefix_len, preferred, valid)) = prefix {
        buf.push(ND_OPT_PREFIX_INFORMATION); // type
        buf.push(4); // length (units of 8 bytes → 32 bytes)
        buf.push(prefix_len); // prefix length
        // On-link + autonomous flags for SLAAC
        buf.push(PREFIX_FLAG_ON_LINK | PREFIX_FLAG_AUTONOMOUS);
        buf.extend_from_slice(&valid.to_be_bytes()); // valid lifetime
        buf.extend_from_slice(&preferred.to_be_bytes()); // preferred lifetime
        buf.extend_from_slice(&[0u8; 4]); // reserved
        buf.extend_from_slice(&addr.octets()); // prefix (16 bytes)
    }

    // --- RDNSS option (RFC 8106) ---
    if !rdnss.is_empty() {
        // length = 1 (header) + 2 * num_addresses, in 8-byte units
        let length_units: u8 = (1 + 2 * rdnss.len() as u8).try_into().unwrap_or(255);
        buf.push(ND_OPT_RDNSS); // type
        buf.push(length_units); // length
        buf.extend_from_slice(&[0u8, 0]); // reserved
        buf.extend_from_slice(&rdnss_lifetime_secs.to_be_bytes()); // lifetime
        for addr in rdnss {
            buf.extend_from_slice(&addr.octets());
        }
    }

    // --- DNSSL option (RFC 8106) ---
    if !dnssl.is_empty() {
        let domains_encoded = encode_dnssl_domains(dnssl);
        // total payload after type+length = reserved(2) + lifetime(4) + domains
        let payload_len = 2 + 4 + domains_encoded.len();
        // length in 8-byte units, rounded up
        let length_units = ((payload_len + 7) / 8) as u8;
        let padding = (length_units as usize) * 8 - payload_len;

        buf.push(ND_OPT_DNSSL); // type
        buf.push(length_units); // length
        buf.extend_from_slice(&[0u8, 0]); // reserved
        buf.extend_from_slice(&dnssl_lifetime_secs.to_be_bytes()); // lifetime
        buf.extend_from_slice(&domains_encoded);
        buf.extend(std::iter::repeat(0u8).take(padding)); // pad to 8-byte boundary
    }

    buf
}

/// Encode domain names for the DNSSL option per RFC 8106 / RFC 1035.
///
/// Each domain is encoded as a sequence of length-prefixed labels,
/// terminated by a zero-length label. Domains are concatenated with
/// no additional separator.
fn encode_dnssl_domains(domains: &[String]) -> Vec<u8> {
    let mut buf = Vec::new();
    for domain in domains {
        let domain = domain.trim_end_matches('.');
        if domain.is_empty() {
            // Just the root terminator.
            buf.push(0);
            continue;
        }
        for label in domain.split('.') {
            let label_bytes = label.as_bytes();
            // Labels are limited to 63 bytes (RFC 1035).
            let len = label_bytes.len().min(63) as u8;
            buf.push(len);
            buf.extend_from_slice(&label_bytes[..len as usize]);
        }
        buf.push(0); // root terminator for this domain
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ra_header_fields_correct() {
        let msg = build_router_advertisement(
            64,
            0,
            1800,
            0,
            0,
            None,
            0,
            &[],
            0,
            &[],
        );
        // Minimum RA is 16 bytes (header only, no options).
        assert_eq!(msg.len(), 16);
        assert_eq!(msg[0], ICMPV6_RA_TYPE); // type 134
        assert_eq!(msg[1], 0); // code
        assert_eq!(msg[4], 64); // cur hop limit
        assert_eq!(msg[5], 0); // flags
        // router lifetime at offset 6 (big-endian u16)
        assert_eq!(u16::from_be_bytes([msg[6], msg[7]]), 1800);
        // reachable time at offset 8 (big-endian u32)
        assert_eq!(u32::from_be_bytes([msg[8], msg[9], msg[10], msg[11]]), 0);
        // retrans timer at offset 12
        assert_eq!(u32::from_be_bytes([msg[12], msg[13], msg[14], msg[15]]), 0);
    }

    #[test]
    fn ra_with_managed_flag() {
        let msg = build_router_advertisement(
            64,
            RA_FLAG_MANAGED,
            1800,
            0,
            0,
            None,
            0,
            &[],
            0,
            &[],
        );
        assert_eq!(msg[5], RA_FLAG_MANAGED);
    }

    #[test]
    fn prefix_option_encoded_correctly() {
        let prefix_addr: Ipv6Addr = "fd00:1234:5678::".parse().unwrap();
        let msg = build_router_advertisement(
            64,
            0,
            1800,
            0,
            0,
            Some((prefix_addr, 64, 3600, 7200)),
            0,
            &[],
            0,
            &[],
        );
        // 16 (header) + 32 (prefix info) = 48
        assert_eq!(msg.len(), 48);

        let opt = &msg[16..];
        assert_eq!(opt[0], ND_OPT_PREFIX_INFORMATION); // type 3
        assert_eq!(opt[1], 4); // length = 4 * 8 = 32 bytes
        assert_eq!(opt[2], 64); // prefix length
        assert_eq!(opt[3], PREFIX_FLAG_ON_LINK | PREFIX_FLAG_AUTONOMOUS);
        // valid lifetime at offset 4
        assert_eq!(u32::from_be_bytes([opt[4], opt[5], opt[6], opt[7]]), 7200);
        // preferred lifetime at offset 8
        assert_eq!(
            u32::from_be_bytes([opt[8], opt[9], opt[10], opt[11]]),
            3600
        );
        // reserved at offset 12 (4 bytes, all zero)
        assert_eq!(&opt[12..16], &[0u8; 4]);
        // prefix at offset 16 (16 bytes)
        assert_eq!(&opt[16..32], &prefix_addr.octets());
    }

    #[test]
    fn rdnss_option_encoded_correctly() {
        let dns1: Ipv6Addr = "fd00:1234:5678::67".parse().unwrap();
        let dns2: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let msg = build_router_advertisement(
            64,
            0,
            1800,
            0,
            0,
            None,
            600,
            &[dns1, dns2],
            0,
            &[],
        );
        // 16 (header) + RDNSS option
        // RDNSS: type(1) + length(1) + reserved(2) + lifetime(4) + 2*16 = 40
        // length units = 1 + 2*2 = 5 → 40 bytes
        assert_eq!(msg.len(), 16 + 40);

        let opt = &msg[16..];
        assert_eq!(opt[0], ND_OPT_RDNSS); // type 25
        assert_eq!(opt[1], 5); // length = 5 * 8 = 40 bytes
        // reserved at offset 2
        assert_eq!(&opt[2..4], &[0u8, 0]);
        // lifetime at offset 4
        assert_eq!(u32::from_be_bytes([opt[4], opt[5], opt[6], opt[7]]), 600);
        // first DNS server at offset 8
        assert_eq!(&opt[8..24], &dns1.octets());
        // second DNS server at offset 24
        assert_eq!(&opt[24..40], &dns2.octets());
    }

    #[test]
    fn rdnss_single_server() {
        let dns: Ipv6Addr = "fd00::1".parse().unwrap();
        let msg = build_router_advertisement(
            64, 0, 1800, 0, 0, None, 300, &[dns], 0, &[],
        );
        // RDNSS with 1 server: length = 1 + 2*1 = 3 → 24 bytes
        assert_eq!(msg.len(), 16 + 24);
        assert_eq!(msg[16], ND_OPT_RDNSS);
        assert_eq!(msg[17], 3);
        assert_eq!(&msg[24..40], &dns.octets());
    }

    #[test]
    fn dnssl_option_encoded_correctly() {
        let domains = vec!["levonk.com".to_string(), "home.arpa".to_string()];
        let msg = build_router_advertisement(
            64, 0, 1800, 0, 0, None, 0, &[], 600, &domains,
        );
        // DNSSL option starts at offset 16
        let opt = &msg[16..];
        assert_eq!(opt[0], ND_OPT_DNSSL); // type 31
        // reserved at offset 2
        assert_eq!(&opt[2..4], &[0u8, 0]);
        // lifetime at offset 4
        assert_eq!(u32::from_be_bytes([opt[4], opt[5], opt[6], opt[7]]), 600);

        // Verify domain encoding starts at offset 8
        // "levonk.com" → [6]levonk[3]com[0] = 12 bytes
        // "home.arpa" → [4]home[4]arpa[0] = 11 bytes
        let encoded = &opt[8..];
        assert_eq!(encoded[0], 6); // "levonk" length
        assert_eq!(&encoded[1..7], b"levonk");
        assert_eq!(encoded[7], 3); // "com" length
        assert_eq!(&encoded[8..11], b"com");
        assert_eq!(encoded[11], 0); // root terminator
        assert_eq!(encoded[12], 4); // "home" length
        assert_eq!(&encoded[13..17], b"home");
        assert_eq!(encoded[17], 4); // "arpa" length
        assert_eq!(&encoded[18..22], b"arpa");
        assert_eq!(encoded[22], 0); // root terminator
    }

    #[test]
    fn dnssl_padding_to_eight_byte_boundary() {
        let domains = vec!["a.b".to_string()];
        let msg = build_router_advertisement(
            64, 0, 1800, 0, 0, None, 0, &[], 300, &domains,
        );
        let opt = &msg[16..];
        let length_units = opt[1] as usize;
        let total_opt_bytes = length_units * 8 + 2; // +2 for type and length bytes
        // The option must be exactly type(1) + length(1) + length_units * 8 bytes.
        assert_eq!(opt.len(), total_opt_bytes);
        // "a.b" → [1]a[1]b[0] = 4 bytes, payload = 2+4+4 = 10, padded to 16
        // total = 2 + 16 = 18
        assert_eq!(total_opt_bytes, 18);
        // Verify trailing bytes after the domain are zero padding.
        // domain data ends at offset 8+4=12, padding fills 12..18
        for &b in &opt[12..18] {
            assert_eq!(b, 0, "padding byte should be zero");
        }
    }

    #[test]
    fn full_ra_with_all_options() {
        let prefix_addr: Ipv6Addr = "fd00:1234:5678::".parse().unwrap();
        let dns: Ipv6Addr = "fd00:1234:5678::67".parse().unwrap();
        let domains = vec!["levonk.com".to_string()];

        let msg = build_router_advertisement(
            64,
            0,
            1800,
            0,
            0,
            Some((prefix_addr, 64, 3600, 7200)),
            600,
            &[dns],
            600,
            &domains,
        );

        // 16 (header) + 32 (prefix) + 24 (rdnss 1 server) + 26 (dnssl)
        // DNSSL "levonk.com": domains=12, payload=2+4+12=18, units=3, total=2+24=26
        assert_eq!(msg.len(), 16 + 32 + 24 + 26);
        // Verify type
        assert_eq!(msg[0], ICMPV6_RA_TYPE);
    }

    #[test]
    fn router_lifetime_zero_not_default_router() {
        let msg = build_router_advertisement(
            64, 0, 0, 0, 0, None, 0, &[], 0, &[],
        );
        assert_eq!(u16::from_be_bytes([msg[6], msg[7]]), 0);
    }

    #[test]
    fn encode_dnssl_empty_domain() {
        let encoded = encode_dnssl_domains(&["".to_string()]);
        assert_eq!(encoded, vec![0]);
    }

    #[test]
    fn encode_dnssl_trailing_dot_stripped() {
        let encoded = encode_dnssl_domains(&["example.com.".to_string()]);
        // Should be same as without trailing dot
        assert_eq!(encoded, encode_dnssl_domains(&["example.com".to_string()]));
    }

    #[test]
    fn all_nodes_multicast_correct() {
        assert_eq!(
            ALL_NODES_MULTICAST,
            "ff02::1".parse::<Ipv6Addr>().unwrap()
        );
    }
}
