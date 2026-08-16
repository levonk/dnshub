//! Parsers for blocklist source files.
//!
//! Supported formats:
//! - **Hosts** — lines starting with `0.0.0.0` or `127.0.0.1` followed by a domain.
//! - **Domains** — one domain per line, `#` comments and blank lines skipped.
//! - **Adblock Plus** — `||domain^` blocking rules (see [`parse_adblock`]).

use crate::blocklist::{BlocklistEntry, Format, Result};

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
        Format::Adblock => Ok(parse_adblock(content, source_id, categories)),
    }
}

/// Parse an Adblock Plus format blocklist file.
///
/// This is a deliberately conservative parser that only extracts domain
/// blocking rules of the form `||domain^` (optionally followed by `$`
/// options). Everything else is skipped:
///
/// - **Comments** — lines starting with `!` or `[` (header directives).
/// - **Exceptions** — lines starting with `@@` (allowlist rules).
/// - **Element hiding** — lines containing `##` or `#@#` (CSS selectors).
/// - **Regex filters** — rules that start with `/` (regex delimiters).
/// - **Generic URL filters** — rules without the `||` anchor that contain
///   path separators (`/`), wildcards (`*`), or other URL-level syntax.
///
/// Supported patterns:
/// ```text
/// ||ads.example.com^
/// ||tracker.example.com^$third-party
/// ||malware.example.org^$document
/// ```
///
/// The trailing `^` (separator) and any `$option` list are stripped before
/// extracting the domain. Domains are lowercased and trailing dots removed.
///
/// `source_id` is the bitmap ID for the source list.
/// `categories` is the category bitmap to assign to every parsed entry.
pub fn parse_adblock(content: &str, source_id: u16, categories: u32) -> Vec<BlocklistEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip comments and header directives.
        if line.starts_with('!') || line.starts_with('[') {
            continue;
        }
        // Skip exception (allowlist) rules.
        if line.starts_with("@@") {
            continue;
        }
        // Skip element-hiding rules (CSS selectors).
        if line.contains("##") || line.contains("#@#") {
            continue;
        }
        // Only handle domain-anchored blocking rules: `||...`.
        if !line.starts_with("||") {
            continue;
        }
        if let Some(entry) = parse_adblock_rule(line, source_id, categories) {
            entries.push(entry);
        }
    }
    entries
}

/// Parse a single `||domain^` (or `||domain^$options`) Adblock Plus rule
/// into a [`BlocklistEntry`], returning `None` if the rule should be skipped
/// (regex-style, contains wildcards, or yields an invalid domain).
fn parse_adblock_rule(line: &str, source_id: u16, categories: u32) -> Option<BlocklistEntry> {
    // Strip the `||` anchor.
    let rest = &line[2..];

    // Strip `$options` suffix (e.g. `$third-party`, `$document`).
    let rest = match rest.split_once('$') {
        Some((before, _)) => before,
        None => rest,
    };

    // Skip rules that contain wildcards or regex anchors — we only handle
    // concrete domains.
    if rest.contains('*') || rest.contains('|') {
        return None;
    }

    // The domain runs up to the first separator character (`^`, `/`, `?`,
    // `=`, `&`) or end of string. A leading `/` indicates a regex
    // filter which we skip.
    if rest.starts_with('/') {
        return None;
    }

    let domain_part = rest
        .find(|c: char| matches!(c, '^' | '/' | '?' | '=' | '&'))
        .map(|idx| &rest[..idx])
        .unwrap_or(rest);

    // Strip the trailing separator marker if present (already handled above
    // via the find, but be defensive about a lone trailing `^`).
    let domain_part = domain_part.trim_end_matches('^');

    // Reject domains that end with a dot (e.g. from a wildcard cut like
    // `ads.`) or are otherwise malformed.
    let domain_part = domain_part.trim_end_matches('.');
    if domain_part.is_empty() {
        return None;
    }

    if let Some(entry) = make_entry(domain_part, source_id, categories) {
        // Reject entries that look like regex/wildcard patterns (contain
        // regex metacharacters in the domain).
        if entry.domain.contains('*')
            || entry.domain.contains('^')
            || entry.domain.contains('/')
        {
            return None;
        }
        Some(entry)
    } else {
        None
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
    fn test_parse_source_adblock() {
        let content = "||ads.com^\n||tracker.com^$third-party";
        let entries = parse_source(content, Format::Adblock, 1, 0).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].domain, "ads.com");
        assert_eq!(entries[1].domain, "tracker.com");
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

    #[test]
    fn test_parse_adblock_basic() {
        let content = r#"
! Title: Example Adblock List
||ads.example.com^
||tracker.example.com^
||malware.example.org^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].domain, "ads.example.com");
        assert_eq!(entries[1].domain, "tracker.example.com");
        assert_eq!(entries[2].domain, "malware.example.org");
        assert_eq!(entries[0].sources, 1);
    }

    #[test]
    fn test_parse_adblock_with_options() {
        let content = r#"
||ads.com^$third-party
||tracker.com^$document
||malware.com^$third-party,domain=example.com
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].domain, "ads.com");
        assert_eq!(entries[1].domain, "tracker.com");
        assert_eq!(entries[2].domain, "malware.com");
    }

    #[test]
    fn test_parse_adblock_skips_comments() {
        let content = r#"
! This is a comment
[Adblock Plus 2.0]
! Another comment
||ads.com^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "ads.com");
    }

    #[test]
    fn test_parse_adblock_skips_exceptions() {
        let content = r#"
||ads.com^
@@||allowlisted.com^
||tracker.com^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].domain, "ads.com");
        assert_eq!(entries[1].domain, "tracker.com");
    }

    #[test]
    fn test_parse_adblock_skips_element_hiding() {
        let content = r#"
||ads.com^
example.com##.ad-banner
example.com#@#.ad-content
||tracker.com^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].domain, "ads.com");
        assert_eq!(entries[1].domain, "tracker.com");
    }

    #[test]
    fn test_parse_adblock_skips_regex_filters() {
        let content = r#"
||ads.com^
/ads/\d+/
||tracker.com^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].domain, "ads.com");
        assert_eq!(entries[1].domain, "tracker.com");
    }

    #[test]
    fn test_parse_adblock_skips_generic_url_filters() {
        // Rules without the `||` anchor are not domain-anchored; skip them.
        let content = r#"
||ads.com^
banner/ads/
*ads*
||tracker.com^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].domain, "ads.com");
        assert_eq!(entries[1].domain, "tracker.com");
    }

    #[test]
    fn test_parse_adblock_wildcard_in_domain_skipped() {
        // `||ads.*^` contains a wildcard; we skip it because we only handle
        // concrete domains.
        let content = r#"
||ads.*^
||concrete.com^
"#;
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "concrete.com");
    }

    #[test]
    fn test_parse_adblock_no_separator() {
        // `||domain` without a trailing `^` is still valid.
        let content = "||noseparator.com";
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "noseparator.com");
    }

    #[test]
    fn test_parse_adblock_lowercase() {
        let content = "||ADS.Example.COM^";
        let entries = parse_adblock(content, 1, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].domain, "ads.example.com");
    }

    #[test]
    fn test_parse_adblock_categories_propagated() {
        let content = "||ads.com^";
        let entries = parse_adblock(content, 1, 0b0011);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].categories, 0b0011);
    }

    #[test]
    fn test_parse_adblock_empty() {
        assert!(parse_adblock("", 1, 0).is_empty());
    }

    #[test]
    fn test_parse_adblock_easylist_excerpt() {
        let content = r#"
! EasyList excerpt
[Adblock Plus 2.0]
! Checksum: abc123
||doubleclick.net^
||googlesyndication.com^$third-party
||adservice.google.com^
@@||adservice.google.com/adsid/$first-party
||pagead2.googlesyndication.com^##.ad
/ads/\d+/banner\.js
||analytics.example.com^$script
"#;
        let entries = parse_adblock(content, 1, 0);
        let domains: Vec<&str> = entries.iter().map(|e| e.domain.as_str()).collect();
        assert_eq!(
            domains,
            vec![
                "doubleclick.net",
                "googlesyndication.com",
                "adservice.google.com",
                "analytics.example.com",
            ]
        );
    }
}
