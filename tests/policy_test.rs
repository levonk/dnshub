//! Integration test: load the `policy.toml` fixture, build a PolicyHandler,
//! and verify that blocked domains return REFUSED while allowed domains
//! continue through the middleware chain.

use dnshub::blocklist::{BlocklistError, BlocklistMetadata, BlocklistStore};
use dnshub::client_resolver::ClientResolver;
use dnshub::dns::{DnsMiddleware, MiddlewareAction};
use dnshub::policy::{PolicyConfig, PolicyEngine, PolicyHandler, PolicyProfile};
use hickory_proto::op::Message;
use hickory_proto::rr::{Name, RecordType};
use hickory_proto::serialize::binary::BinEncodable;
use hickory_server::server::Request;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

/// A mock blocklist store backed by a HashMap.
struct MockBlocklist {
    entries: HashMap<String, BlocklistMetadata>,
}

impl BlocklistStore for MockBlocklist {
    fn lookup(&self, domain: &str) -> Result<Option<BlocklistMetadata>, BlocklistError> {
        Ok(self.entries.get(domain).copied())
    }
    fn len(&self) -> Result<u64, BlocklistError> {
        Ok(self.entries.len() as u64)
    }
}

/// Build a DNS Request for an A-record query of `domain` from `src_ip`.
fn make_request(domain: &str, src_ip: IpAddr) -> Request {
    let name = Name::from_utf8(domain).unwrap();
    let mut msg = Message::new(
        1,
        hickory_proto::op::MessageType::Query,
        hickory_proto::op::OpCode::Query,
    );
    msg.add_query(hickory_proto::op::Query::query(name, RecordType::A));
    let raw = msg.to_bytes().unwrap();
    Request::from_bytes(
        raw,
        SocketAddr::new(src_ip, 12345),
        hickory_server::net::xfer::Protocol::Udp,
    )
    .unwrap()
}

/// Load the policy.toml fixture and build a complete PolicyHandler.
fn build_handler_from_fixture() -> PolicyHandler {
    let fixture_path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/policy.toml");
    let config = PolicyConfig::from_file(&fixture_path).expect("load policy.toml fixture");

    // Build the client resolver from the config.
    let resolver = ClientResolver::from_config(&config);

    // Build a mock blocklist with a few entries.
    let mut entries = HashMap::new();
    entries.insert(
        "ads.example.com".to_string(),
        BlocklistMetadata {
            categories: 0,
            sources: 1,
            first_seen: 0,
            last_updated: 0,
        },
    );
    let store = MockBlocklist { entries };
    let engine = PolicyEngine::new(Arc::new(store));

    // Collect named profiles.
    let default_profile: PolicyProfile = config.default.clone();
    let profiles: HashMap<String, PolicyProfile> = config.policies.clone();

    PolicyHandler::new(resolver, engine, profiles, default_profile)
}

#[tokio::test]
async fn policy_toml_loads_all_sections() {
    let fixture_path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/policy.toml");
    let config = PolicyConfig::from_file(&fixture_path).expect("load policy.toml fixture");

    // Default profile.
    assert_eq!(config.default.name, "default");
    assert_eq!(config.default.blocked_categories.len(), 4);

    // Named profiles.
    assert_eq!(config.policies.len(), 4);
    assert!(config.policies.contains_key("parents"));
    assert!(config.policies.contains_key("kids"));
    assert!(config.policies.contains_key("iot"));
    assert!(config.policies.contains_key("guest"));

    // Client mappings (5 total: 3 exact IPs + 2 CIDRs).
    assert_eq!(config.clients.len(), 5);

    // DHCP integration.
    assert_eq!(
        config.dhcp_integration.hostname_map.get("kids-tablet"),
        Some(&"kids".to_string())
    );
}

#[tokio::test]
async fn blocked_domain_returns_refused() {
    let handler = build_handler_from_fixture();
    // 192.168.1.20 → kids profile, which has custom_blocklist = [tiktok.com, ...]
    let req = make_request("tiktok.com", IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
    let action = handler.process(&req).await;
    assert!(matches!(
        action,
        MiddlewareAction::Reject(hickory_proto::op::ResponseCode::Refused)
    ));
}

#[tokio::test]
async fn allowed_domain_continues() {
    let handler = build_handler_from_fixture();
    // 192.168.1.10 → parents profile (allowed_categories = ["all"])
    let req = make_request("anything.com", IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
    let action = handler.process(&req).await;
    assert!(matches!(action, MiddlewareAction::Continue));
}

#[tokio::test]
async fn custom_allowlist_overrides_blocklist() {
    let handler = build_handler_from_fixture();
    // kids profile has custom_allowlist = [khanacademy.org]
    let req = make_request(
        "khanacademy.org",
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
    );
    let action = handler.process(&req).await;
    assert!(matches!(action, MiddlewareAction::Continue));
}

#[tokio::test]
async fn wildcard_allowlist_matches_subdomain() {
    let handler = build_handler_from_fixture();
    // kids profile has custom_allowlist = [*.scratch.mit.edu]
    let req = make_request(
        "code.scratch.mit.edu",
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
    );
    let action = handler.process(&req).await;
    assert!(matches!(action, MiddlewareAction::Continue));
}

#[tokio::test]
async fn cidr_client_resolves_to_profile() {
    let handler = build_handler_from_fixture();
    // 192.168.10.50 → guest profile (CIDR 192.168.10.0/24, allowed_categories=["all"])
    let req = make_request("ads.example.com", IpAddr::V4(Ipv4Addr::new(192, 168, 10, 50)));
    let action = handler.process(&req).await;
    // guest has allowed_categories = ["all"], so even ads.example.com is allowed.
    assert!(matches!(action, MiddlewareAction::Continue));
}

#[tokio::test]
async fn default_client_category_block() {
    let handler = build_handler_from_fixture();
    // 10.0.0.1 → default profile, which blocks ads category.
    // ads.example.com is in the mock blocklist.
    let req = make_request(
        "ads.example.com",
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
    );
    let action = handler.process(&req).await;
    assert!(matches!(
        action,
        MiddlewareAction::Reject(hickory_proto::op::ResponseCode::Refused)
    ));
}
