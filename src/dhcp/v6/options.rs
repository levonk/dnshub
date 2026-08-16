//! DHCPv6 standard option builders (RFC 8415 / RFC 3646 / RFC 5908).
//!
//! Each builder returns a [`dhcproto::v6::DhcpOption`] ready to be inserted
//! into a reply's option set. The supported options are:
//!
//! | Code | Option                    | RFC        |
//! |------|---------------------------|------------|
//! | 23   | DNS recursive name server | RFC 3646   |
//! | 24   | Domain search list        | RFC 3646   |
//! | 32   | Information refresh time  | RFC 8415   |
//! | 56   | NTP server                | RFC 5908   |

use dhcproto::{v6::DhcpOption, Name};
use std::net::Ipv6Addr;

/// Build option 23 (DNS recursive name server, RFC 3646).
///
/// Carries the list of DNS server addresses the client should use.
pub fn dns_servers(servers: &[Ipv6Addr]) -> DhcpOption {
    DhcpOption::DomainNameServers(servers.to_vec())
}

/// Build option 24 (domain search list, RFC 3646).
///
/// `domains` are encoded as DNS names (wire format). Each entry is parsed
/// via [`Name::from_utf8`]; entries that fail to parse are skipped (a
/// malformed domain in config should not break the whole reply).
pub fn domain_search_list(domains: &[String]) -> DhcpOption {
    let names: Vec<Name> = domains
        .iter()
        .filter_map(|d| Name::from_utf8(d).ok())
        .collect();
    DhcpOption::DomainSearchList(names)
}

/// Build option 32 (information refresh time, RFC 8415 §21.31).
///
/// The time in seconds after which a stateless client (INFORMATION-REQUEST)
/// should refresh its configuration. Only meaningful in REPLY to
/// INFORMATION-REQUEST.
pub fn information_refresh_time(seconds: u32) -> DhcpOption {
    DhcpOption::InformationRefreshTime(seconds)
}

/// Build option 56 (NTP server, RFC 5908).
///
/// Encodes a single NTP server address as a `ServerAddress` sub-option.
pub fn ntp_server(addr: Ipv6Addr) -> DhcpOption {
    DhcpOption::NtpServer(vec![dhcproto::v6::NtpSuboption::ServerAddress(addr)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use dhcproto::v6::{DhcpOption, NtpSuboption, OptionCode};

    #[test]
    fn dns_servers_option() {
        let servers = vec![
            "fd00:1234:5678::67".parse().unwrap(),
            "fd00:1234:5678::68".parse().unwrap(),
        ];
        let opt = dns_servers(&servers);
        match opt {
            DhcpOption::DomainNameServers(ref v) => assert_eq!(v, &servers),
            other => panic!("expected DomainNameServers, got {other:?}"),
        }
        assert_eq!(OptionCode::from(&opt), OptionCode::DomainNameServers);
    }

    #[test]
    fn domain_search_list_option() {
        let domains = vec!["levonk.com".to_string(), "home.arpa".to_string()];
        let opt = domain_search_list(&domains);
        match &opt {
            DhcpOption::DomainSearchList(names) => {
                assert_eq!(names.len(), 2);
                // hickory Name display omits the trailing root dot.
                assert_eq!(names[0].to_string(), "levonk.com");
            }
            other => panic!("expected DomainSearchList, got {other:?}"),
        }
        assert_eq!(OptionCode::from(&opt), OptionCode::DomainSearchList);
    }

    #[test]
    fn domain_search_list_skips_malformed() {
        // A label longer than 63 characters is invalid per RFC 1035 and
        // hickory rejects it. The valid entry must still be encoded.
        let long_label = "a".repeat(70);
        let domains = vec!["levonk.com".to_string(), long_label];
        let opt = domain_search_list(&domains);
        match opt {
            DhcpOption::DomainSearchList(names) => assert_eq!(names.len(), 1),
            other => panic!("expected DomainSearchList, got {other:?}"),
        }
    }

    #[test]
    fn information_refresh_time_option() {
        let opt = information_refresh_time(3600);
        match opt {
            DhcpOption::InformationRefreshTime(t) => assert_eq!(t, 3600),
            other => panic!("expected InformationRefreshTime, got {other:?}"),
        }
        assert_eq!(OptionCode::from(&opt), OptionCode::InformationRefreshTime);
    }

    #[test]
    fn ntp_server_option() {
        let addr: Ipv6Addr = "fd00:1234:5678::55".parse().unwrap();
        let opt = ntp_server(addr);
        match opt {
            DhcpOption::NtpServer(ref subs) => {
                assert_eq!(subs.len(), 1);
                match &subs[0] {
                    NtpSuboption::ServerAddress(a) => assert_eq!(*a, addr),
                    other => panic!("expected ServerAddress, got {other:?}"),
                }
            }
            other => panic!("expected NtpServer, got {other:?}"),
        }
        assert_eq!(OptionCode::from(&opt), OptionCode::NtpServer);
    }

    #[test]
    fn options_round_trip_through_message() {
        use dhcproto::{v6::Message, Decodable, Decoder, Encodable, Encoder};
        let mut msg = Message::new(dhcproto::v6::MessageType::Reply);
        msg.opts_mut().insert(dns_servers(&[
            "fd00::67".parse().unwrap(),
        ]));
        msg.opts_mut().insert(domain_search_list(&[
            "levonk.com".to_string(),
        ]));
        msg.opts_mut().insert(information_refresh_time(600));
        msg.opts_mut().insert(ntp_server("fd00::55".parse().unwrap()));

        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        msg.encode(&mut e).unwrap();

        let decoded = Message::decode(&mut Decoder::new(&buf)).unwrap();
        assert!(decoded
            .opts()
            .get(OptionCode::DomainNameServers)
            .is_some());
        assert!(decoded.opts().get(OptionCode::DomainSearchList).is_some());
        assert!(decoded
            .opts()
            .get(OptionCode::InformationRefreshTime)
            .is_some());
        assert!(decoded.opts().get(OptionCode::NtpServer).is_some());
    }
}
