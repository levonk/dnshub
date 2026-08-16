//! DNS middleware handler that enforces per-client policy (PRD section 4.5).
//!
//! [`PolicyHandler`] implements the [`DnsMiddleware`](crate::dns::DnsMiddleware)
//! trait. For each incoming request it:
//!
//! 1. Extracts the client IP from the request source address.
//! 2. Resolves the client IP to a profile name via [`ClientResolver`].
//! 3. Evaluates the queried domain against the profile via [`PolicyEngine`].
//! 4. Returns [`MiddlewareAction::Reject(Refused)`] for blocked queries, or
//!    [`MiddlewareAction::Continue`] to let the request proceed to forwarding.
//!
//! Policy decisions are logged via `tracing` (client tag, profile, domain,
//! decision). Per-client metrics are deferred to story 02-003.

use crate::client_resolver::ClientResolver;
use crate::dns::{DnsMiddleware, MiddlewareAction};
use crate::policy::engine::{PolicyDecision, PolicyEngine};
use crate::policy::profile::PolicyProfile;
use async_trait::async_trait;
use hickory_proto::op::ResponseCode;
use hickory_server::server::Request;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// A DNS middleware that enforces per-client policy.
pub struct PolicyHandler {
    resolver: ClientResolver,
    engine: PolicyEngine,
    /// A snapshot of all profiles keyed by name, used to look up the
    /// profile for a resolved client. In a hot-reload world (story 02-004)
    /// this would be an `ArcSwap<HashMap<...>>`.
    profiles: Arc<std::collections::HashMap<String, PolicyProfile>>,
    /// The default profile (for clients that resolve to "default").
    default_profile: PolicyProfile,
}

impl PolicyHandler {
    /// Build a new policy handler.
    ///
    /// - `resolver`: maps client IPs to profile names.
    /// - `engine`: evaluates domains against profiles.
    /// - `profiles`: all named profiles from `policy.toml`.
    /// - `default_profile`: the `[default]` profile.
    pub fn new(
        resolver: ClientResolver,
        engine: PolicyEngine,
        profiles: std::collections::HashMap<String, PolicyProfile>,
        default_profile: PolicyProfile,
    ) -> Self {
        Self {
            resolver,
            engine,
            profiles: Arc::new(profiles),
            default_profile,
        }
    }

    /// Look up the profile for a resolved profile name.
    fn profile_for(&self, name: &str) -> &PolicyProfile {
        if name == self.default_profile.name {
            return &self.default_profile;
        }
        self.profiles.get(name).unwrap_or(&self.default_profile)
    }

    /// Extract the queried domain name from a request, in lowercase without
    /// a trailing dot.
    fn query_domain(request: &Request) -> Option<String> {
        let queries = request.queries.queries();
        let query = queries.first()?;
        Some(query.name().to_string().trim_end_matches('.').to_ascii_lowercase())
    }
}

#[async_trait]
impl DnsMiddleware for PolicyHandler {
    async fn process(&self, request: &Request) -> MiddlewareAction {
        let client_ip = request.src().ip();

        // Extract the queried domain. If there's no query (e.g. a malformed
        // request), let it pass through — the catalog will handle the error.
        let Some(domain) = Self::query_domain(request) else {
            debug!("policy: no query in request, allowing");
            return MiddlewareAction::Continue;
        };

        // Resolve client IP → profile name.
        let profile_name = self.resolver.resolve(client_ip);
        let profile = self.profile_for(profile_name);

        // Evaluate the domain against the profile.
        let result = self.engine.evaluate(&domain, profile);

        match result.decision {
            PolicyDecision::Allow | PolicyDecision::Redirect => {
                info!(
                    client_ip = %client_ip,
                    profile = %profile_name,
                    domain = %domain,
                    reason = ?result.reason,
                    "policy: allow"
                );
                MiddlewareAction::Continue
            }
            PolicyDecision::Block => {
                warn!(
                    client_ip = %client_ip,
                    profile = %profile_name,
                    domain = %domain,
                    reason = ?result.reason,
                    "policy: block"
                );
                MiddlewareAction::Reject(ResponseCode::Refused)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::{BlocklistError, BlocklistMetadata, BlocklistStore};
    use crate::client_resolver::{ClientResolver, CidrEntry};
    use crate::policy::engine::PolicyEngine;
    use crate::policy::profile::PolicyProfile;
    use hickory_proto::op::Message;
    use hickory_proto::rr::{Name, RecordType};
    use hickory_proto::serialize::binary::BinEncodable;
    use hickory_server::server::Request;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    /// Mock blocklist store for handler tests.
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

    /// Build a DNS `Request` carrying an A-record query for `domain` from
    /// `src_ip`. Uses the wire-format encoder/decoder (no `testing` feature).
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

    fn make_handler() -> PolicyHandler {
        let mut exact = HashMap::new();
        exact.insert(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            "parents".to_string(),
        );
        let cidrs = vec![CidrEntry {
            cidr: "192.168.20.0/24".to_string(),
            profile: "iot".to_string(),
        }];
        let resolver = ClientResolver::new(exact, cidrs, "default");

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

        let default_profile = PolicyProfile {
            name: "default".to_string(),
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        let iot_profile = PolicyProfile {
            name: "iot".to_string(),
            blocked_categories: vec!["ads".to_string()],
            custom_blocklist: vec!["blocked.iot.com".to_string()],
            ..Default::default()
        };
        let parents_profile = PolicyProfile {
            name: "parents".to_string(),
            allowed_categories: vec!["all".to_string()],
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        let mut profiles = HashMap::new();
        profiles.insert("iot".to_string(), iot_profile);
        profiles.insert("parents".to_string(), parents_profile);

        PolicyHandler::new(resolver, engine, profiles, default_profile)
    }

    #[tokio::test]
    async fn test_blocked_domain_returns_refused() {
        let handler = make_handler();
        // iot client (192.168.20.5) with default profile blocks ads.example.com
        let req = make_request("ads.example.com", IpAddr::V4(Ipv4Addr::new(192, 168, 20, 5)));
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Reject(ResponseCode::Refused)));
    }

    #[tokio::test]
    async fn test_allowed_domain_continues() {
        let handler = make_handler();
        let req = make_request("clean.example.com", IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn test_parents_profile_allows_blocked_domain() {
        let handler = make_handler();
        // parents profile has allowed_categories = ["all"]
        let req = make_request("ads.example.com", IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)));
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn test_custom_blocklist_blocks() {
        let handler = make_handler();
        // iot profile has custom_blocklist = ["blocked.iot.com"]
        let req = make_request("blocked.iot.com", IpAddr::V4(Ipv4Addr::new(192, 168, 20, 5)));
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Reject(ResponseCode::Refused)));
    }

    #[tokio::test]
    async fn test_default_client_blocked_domain() {
        let handler = make_handler();
        // 10.0.0.1 → default profile, which blocks ads
        let req = make_request("ads.example.com", IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        let action = handler.process(&req).await;
        assert!(matches!(action, MiddlewareAction::Reject(ResponseCode::Refused)));
    }
}
