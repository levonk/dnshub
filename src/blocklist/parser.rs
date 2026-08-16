//! Parsers for blocklist source files.
//!
//! Supported formats (this story):
//! - **Hosts** — lines starting with `0.0.0.0` or `127.0.0.1` followed by a domain.
//! - **Domains** — one domain per line, `#` comments and blank lines skipped.
//!
//! Adblock Plus format parsing arrives in story 02-002.

use crate::blocklist::{BlocklistEntry, BlocklistError, Format, Result};

/// Parse a hosts-format blocklist file.
///
/// Recognised line patterns:
/// ```text
/// 0.0.0.0 ads.example.com
/// 127.0.0.1 tracker.example.com
/// 0.0.0.0 ads.example.com # comment
/// ```
///
/// Lines that don't start with `0.0.0.0` or `127.0.0.1` are silently skipped
/// (they may be comments, metadata, or other directives).
///
/// `source_id` is the bitmap ID for the source list this file belongs to.
/// `categories` is the category bitmap to assign to every parsed entry.
pub fn parse_hosts(content: &str, source_id: u16, categories: u32) -> Vec<BlocklistEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip inline comments.
        let line = match line.split_once('#') {
            Some((before, _)) => before.trim(),
            None => line,
        };
        let mut parts = line.split_whitespace();
        let ip = parts.next();
        let domain = parts.next();
        match (ip, domain) {
            (Some(ip), Some(domain)) if is_loopback_ip(ip) => {
                if let Some(entry) = make_entry(domain, source_id, categories) {
                    entries.push(entry);
                }
            }
            _ => {}
        }
    }
    entries
}

/// Parse a domains-format blocklist file.
///
/// Each non-comment, non-blank line is a domain name:
/// ```text
/// ads.example.com
/// tracker.example.com
/// # this is a comment
/// ```
///
/// `source_id` is the bitmap ID for the source list.
/// `categories` is the category bitmap to assign to every parsed entry.
pub fn parse_domains(content: &str, source_id: u16, categories: u32) -> Vec<BlocklistEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip inline comments.
        let domain = match line.split_once('#') {
            Some((before, _)) => before.trim(),
            None => line,
        };
        if let Some(entry) = make_entry(domain, source_id, categories) {
            entries.push(entry);
        }
    }
    entries
}

/// Parse a blocklist source file using the specified format.
pub fn parse_source(
    content: &str,
    format: Format,
    source_id: u16,
    categories: u32,
) -> Result<Vec<BlocklistEntry>> {
    match format {
        Format::Hosts => Ok(parse_hosts(content, source_id, categories)),
        Format::Domains => Ok(parse_domains(content, source_id, categories)),
        Format::Adblock => Err(BlocklistError::Parse(
            "Adblock Plus format parser is implemented in story 02-002".to_string(),
        )),
    }
}

/// Check if an IP string is a loopback / null IP used in hosts files.
fn is_loopback_ip(ip: &str) -> bool {
    ip == "0.0.0.0" || ip == "127.0.0.1" || ip == "::1" || ip == "0::0" || ip == "::"
}

/// Create a `BlocklistEntry` from a domain string, returning `None` if the
/// domain is invalid (empty, contains whitespace, or is `localhost`).
fn make_entry(domain: &str, source_id: u16, categories: u32) -> Option<BlocklistEntry> {
    let domain = domain.trim_end_matches('.');
    if domain.is_empty() || domain == "localhost" || domain == "ip6-localhost" {
        return None;
    }
    // Reject entries with internal whitespace (malformed).
    if domain.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some(BlocklistEntry {
        domain: domain.to_lowercase(),
        categories,
        sources: source_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hosts_basic() {
        let content = r#"
# StevenBlack hosts file
0.0.0.0 ads.example.com
127.0.0.1 tracker.example.com
0.0.0.0 malware.example.org # dangerous
"#;
        let entries = parse_hosts(content, 1, 0);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].domain, "ads.example.com");
        assert_eq!(entries[1].domain, "tracker.example.com");
        assert_eq!(entries[2].domain, "malware.example.org");
        assert_eq!(entries[0].sources, 1);
    }

    #[test]
    fn test_parse_hosts_skips_non_hosts_lines() {
        let content = r#"
# Comment
127.0.0.1 localhost
255.255.255.0 broadcast
0.0.0.0 blocked.com
some random text
"#;
        let entries = parse_hosts(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "blocked.com");
    }

    #[test]
    fn test_parse_hosts_empty() {
        let entries = parse_hosts("", 1, 0);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_domains_basic() {
        let content = r#"
# HaGeZi domain list
ads.example.com
tracker.example.com

# more entries
malware.example.org
"#;
        let entries = parse_domains(content, 2, 0);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].domain, "ads.example.com");
        assert_eq!(entries[1].domain, "tracker.example.com");
        assert_eq!(entries[2].domain, "malware.example.org");
        assert_eq!(entries[0].sources, 2);
    }

    #[test]
    fn test_parse_domains_inline_comment() {
        let content = "example.com # inline comment\nanother.com";
        let entries = parse_domains(content, 1, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].domain, "example.com");
        assert_eq!(entries[1].domain, "another.com");
    }

    #[test]
    fn test_parse_domains_empty() {
        let entries = parse_domains("", 1, 0);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_domains_lowercase() {
        let content = "ADS.Example.COM";
        let entries = parse_domains(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "ads.example.com");
    }

    #[test]
    fn test_parse_source_hosts() {
        let content = "0.0.0.0 ads.com";
        let entries = parse_source(content, Format::Hosts, 1, 0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "ads.com");
    }

    #[test]
    fn test_parse_source_domains() {
        let content = "ads.com\ntracker.com";
        let entries = parse_source(content, Format::Domains, 1, 0).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_parse_source_adblock_not_implemented() {
        let result = parse_source("||ads.com^", Format::Adblock, 1, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_hosts_trailing_dot() {
        let content = "0.0.0.0 example.com.";
        let entries = parse_hosts(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "example.com");
    }

    #[test]
    fn test_parse_hosts_ipv6() {
        let content = "::1 ads.example.com";
        let entries = parse_hosts(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "ads.example.com");
    }

    #[test]
    fn test_parse_hosts_localhost_filtered() {
        let content = "127.0.0.1 localhost\n0.0.0.0 ads.com";
        let entries = parse_hosts(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "ads.com");
    }
}
