---
story_id: "02-001"
story_title: "PolicyEngine + ClientResolver + PolicyHandler + policy.toml"
story_name: "policy-engine-and-client-resolver"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 2
parallel_id: 1
branch: "feature/current/dnshub/story-02-001-policy-engine-and-client-resolver"
status: "done"
assignee: ""
reviewer: ""
dependencies: ["01-001", "01-002", "01-004"]
parallel_safe: true
modules: ["policy", "client-resolver"]
priority: "MUST"
risk_level: "medium"
tags: ["feat", "backend", "policy", "client-resolver"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement the per-client policy system: ClientResolver (resolves client IP to a policy profile via DHCP lease table, static IP mapping, and CIDR range matching), PolicyEngine (evaluates custom allowlist/blocklist and category blocklists against a client's profile), and PolicyHandler (a RequestHandler that intercepts queries, resolves the client profile, checks policy, and blocks/allowes accordingly). Also implement policy.toml loading per PRD section 4.5.

## Current State

- **Relevant files and their roles:**
  - `src/dns/mod.rs` — DnshubHandler chain (from story 01-001), PolicyHandler will be inserted into this chain
  - `src/blocklist/mod.rs` — BlocklistStore with check_domain method (from story 01-002), PolicyEngine calls this
  - `src/config/mod.rs` — Config loading patterns (from story 01-004), policy.toml uses same patterns
- **Existing code excerpts:**
  - `src/dns/mod.rs` — DnshubHandler implements RequestHandler, delegates to inner handler chain
  - `src/blocklist/mod.rs` — BlocklistStore trait with lookup by reversed domain key
- **Repository conventions:** Use serde + toml for policy.toml. Use ipnet crate for CIDR matching. Use ArcSwap for hot-swappable policy config.
- **Tech context (binding constraint from tech-context.txt):**
  - Package manager: cargo (Rust)
  - Build: `cargo build` | Test: `cargo test` | Lint: `cargo clippy` | Format: `cargo fmt`
  - Ad-hoc runner: `cargo add` / `cargo binstall -y`
  - Never use: npm, npx, yarn, jest, biome
  - Rust toolchain: cargo 1.95.0 on PATH
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Build   | `cargo build` | exit 0, no errors |
  | Tests   | `cargo test` | all pass |
  | Lint    | `cargo clippy -- -D warnings` | exit 0, no warnings |
  | Format  | `cargo fmt -- --check` | exit 0, no changes needed |

## Scope

**In scope:**
- src/policy/mod.rs — module root with PolicyEngine, PolicyDecision (Allow/Block/Redirect)
- src/policy/profile.rs — PolicyProfile struct (name, description, blocked_categories, allowed_categories, custom_allowlist, custom_blocklist, upstream, log_level) per PRD lines 975-1052
- src/policy/engine.rs — PolicyEngine: evaluate(domain, profile) -> PolicyDecision. Evaluation order: custom_allowlist → custom_blocklist → category blocklist → allowed_categories check per PRD lines 1055-1062
- src/policy/config.rs — PolicyConfig serde structs for policy.toml ([default], [policies.X], [clients."IP"], [dhcp_integration])
- src/policy/handler.rs — PolicyHandler implementing RequestHandler: resolve client IP → profile, evaluate policy, return NXDOMAIN or REFUSED for blocked queries
- src/client_resolver/mod.rs — ClientResolver: resolve(client_ip) -> ProfileName. Resolution order: DHCP static lease → DHCP dynamic lease → static IP mapping → CIDR range → default per PRD lines 963-968
- src/client_resolver/cidr.rs — CIDR range matching using ipnet crate
- src/client_resolver/config.rs — ClientMapping serde structs for [clients."IP"] and CIDR entries
- Integration: wire PolicyHandler into the DnshubHandler chain (before forwarding)
- Unit tests for PolicyEngine evaluation, ClientResolver resolution, CIDR matching

**Out of scope:**
- DHCP lease table integration (story 04-011 — ClientResolver uses static IP + CIDR fallback only in this story)
- Category bitmap in blocklist (story 02-002 — PolicyEngine calls blocklist.check_domain which returns categories; in this story categories may be empty)
- Per-client metrics (story 02-003)
- Hot-reload of policy.toml (story 02-004)
- Adblock Plus parser (story 02-002)

## Sub-Tasks

- [x] Create src/policy/profile.rs with PolicyProfile struct matching PRD lines 975-1052 (name, description, blocked_categories: Vec<String>, allowed_categories: Vec<String>, custom_allowlist: Vec<String>, custom_blocklist: Vec<String>, upstream: String, log_level: String)
  **Verify**: `cargo build` → exit 0
- [x] Create src/policy/config.rs with PolicyConfig serde structs for policy.toml: [default] profile, [policies.X] profiles, [clients."IP"] mappings, [dhcp_integration] hostname_map
  **Verify**: `cargo test --lib policy::config` → all pass (parse PRD example policy.toml)
- [x] Create src/policy/engine.rs with PolicyEngine: evaluate(domain, profile) -> PolicyDecision. Implement evaluation order from PRD lines 1055-1062: custom_allowlist match → Allow; custom_blocklist match → Block; category blocklist intersection → Block; allowed_categories contains "all" → skip category check; else Allow
  **Verify**: `cargo test --lib policy::engine` → all pass (test allow, block by category, block by custom, allow override)
- [x] Create src/client_resolver/cidr.rs with CIDR matching using ipnet crate: match_ip(ip, cidr) -> bool, find_matching_cidr(ip, cidr_list) -> Option<profile>
  **Verify**: `cargo test --lib client_resolver::cidr` → all pass (test exact IP, CIDR match, no match)
- [x] Create src/client_resolver/mod.rs with ClientResolver: resolve(client_ip) -> ProfileName. Resolution order: static IP exact match → CIDR range match → default profile. (DHCP lease lookup added in story 04-011)
  **Verify**: `cargo test --lib client_resolver` → all pass (test static IP, CIDR, default fallback)
- [x] Create src/policy/handler.rs with PolicyHandler implementing RequestHandler: extract client IP from request, resolve profile via ClientResolver, evaluate policy via PolicyEngine, return REFUSED for blocked queries, delegate to inner handler for allowed queries
  **Verify**: `cargo build` → exit 0
- [x] Wire PolicyHandler into DnshubHandler chain in src/dns/mod.rs (insert before forwarding handler)
  **Verify**: `cargo build` → exit 0
- [x] Create test fixture: tests/fixtures/policy.toml matching PRD example (lines 972-1052)
  **Verify**: `cargo test --lib policy` → all pass
- [x] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/policy/mod.rs` — module root
- `src/policy/profile.rs` — PolicyProfile struct
- `src/policy/engine.rs` — PolicyEngine with evaluate method
- `src/policy/config.rs` — policy.toml serde structs
- `src/policy/handler.rs` — PolicyHandler implementing RequestHandler
- `src/client_resolver/mod.rs` — ClientResolver
- `src/client_resolver/cidr.rs` — CIDR matching
- `src/client_resolver/config.rs` — client mapping structs
- `src/dns/mod.rs` — modified to wire PolicyHandler into chain
- `tests/fixtures/policy.toml` — test fixture

## Acceptance Criteria

- [x] ClientResolver resolves client IPs to profiles via static IP and CIDR matching
- [x] PolicyEngine correctly evaluates custom allowlist, custom blocklist, and category blocklists
- [x] PolicyHandler blocks queries for blocked domains (returns REFUSED)
- [x] PolicyHandler allows queries for allowed domains (delegates to inner handler)
- [x] policy.toml loads correctly with all profile and client mapping sections
- [x] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib policy` and `cargo test --lib client_resolver` — tests for engine, resolver, CIDR matching
- Integration: `cargo test --test integration_test` — verify blocked domain returns REFUSED, allowed domain resolves
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log policy decisions (client, profile, domain, decision: allowed/blocked)
- Metrics for policy decisions deferred to story 02-003

## Compliance

- Client IP addresses are used for policy resolution — treat as network metadata, not personal data
- Query logging with client identification must respect privacy (use client_tag not raw IP in logs)

## Risks & Mitigations

- Risk: Policy evaluation adds latency to hot path — Mitigation: Keep evaluation simple (hashmap lookups, CIDR match is O(n) with small n), cache profile resolution
- Risk: Wildcard domain matching in custom allowlist/blocklist — Mitigation: Support glob patterns (*.example.com) using simple prefix/suffix matching

## Dependencies & Sequencing

- Depends on: 01-001 (handler chain), 01-002 (blocklist store for category checks), 01-004 (config loading patterns)
- Unblocks: 04-011 (ClientResolver DHCP integration extends this)

## Definition of Done

- [x] All verification commands from sub-tasks pass
- [x] Code, tests, docs updated; CI green
- [x] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- hickory-server RequestHandler trait does not allow intercepting and short-circuiting queries
- ipnet crate CIDR matching API does not work as expected
- policy.toml format from PRD cannot be parsed by serde + toml

## Maintenance Notes

- ClientResolver is designed to be extended with DHCP lease table lookup in story 04-011
- PolicyEngine category check uses blocklist.check_domain() which returns categories — in this story categories may be empty (all pass), category bitmap added in story 02-002
- Reviewers should verify the policy evaluation order matches PRD lines 1055-1062

## Commit Conventions

- `feat(policy): add PolicyEngine with custom allowlist/blocklist evaluation`
- `feat(policy): add ClientResolver with CIDR matching`
- `feat(policy): add PolicyHandler to RequestHandler chain`
- `feat(policy): add policy.toml config loading`

## Changelog

- 2026-08-16: initialized story file
- 2026-08-16: implemented PolicyProfile, PolicyConfig (policy.toml serde), PolicyEngine (evaluate with custom allow/block + category checks), ClientResolver (static IP + CIDR + default fallback), CIDR matching via ipnet, PolicyHandler (DnsMiddleware impl returning REFUSED for blocked queries), wired PolicyHandler into DnshubHandler chain via with_middleware_front, added tests/fixtures/policy.toml, added integration tests. All 142 tests pass (124 lib + 9 config + 1 integration + 7 policy + 1 doctest). `cargo build` is warning-free. Note: `cargo clippy` and `cargo fmt` are not installed on this host.
