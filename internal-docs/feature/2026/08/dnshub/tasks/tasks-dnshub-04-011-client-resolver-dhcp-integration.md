---
story_id: "04-011"
story_title: "ClientResolver DHCP integration (IP to hostname to profile from lease tables)"
story_name: "client-resolver-dhcp-integration"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 11
branch: "feature/current/dnshub/story-04-011-client-resolver-dhcp-integration"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["02-001"]
parallel_safe: true
modules: ["policy", "client-resolver", "dhcp"]
priority: "MUST"
risk_level: "medium"
tags: ["feat", "backend", "policy", "client-resolver", "dhcp-integration"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Integrate the ClientResolver (from story 02-001) with the DHCP lease tables (from stories 04-001, 04-002). The ClientResolver now checks DHCP static leases (MAC → IP → profile) and DHCP dynamic leases (IP → hostname → profile via hostname_map) as the highest-priority resolution path, before falling back to static IP and CIDR matching. This makes per-client policy "just work" — devices identified by DHCP automatically get their assigned profile.

## Current State

- **Relevant files and their roles:**
  - `src/client_resolver/mod.rs` — ClientResolver with static IP + CIDR matching (from story 02-001)
  - `src/dhcp/v4/lease_store.rs` — SqliteLeaseStoreV4 with lease queries (from story 04-001)
  - `src/dhcp/v6/lease_store.rs` — SqliteLeaseStoreV6 with lease queries (from story 04-002)
  - PRD lines 957-968 define client resolution order
- **Existing code excerpts:**
  - `src/client_resolver/mod.rs` — resolve(client_ip) -> ProfileName: static IP → CIDR → default
  - `src/dhcp/v4/lease_store.rs` — get_lease_by_ip(ip) -> Option<DhcpLeaseV4>
- **Repository conventions:** ClientResolver resolution order per PRD lines 963-968: DHCP static lease → DHCP dynamic lease → static IP mapping → CIDR range → default.
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
- src/client_resolver/mod.rs — update resolve() to check DHCP lease tables first: query SqliteLeaseStoreV4 and SqliteLeaseStoreV6 for the client IP, extract profile from lease metadata
- src/client_resolver/dhcp_lookup.rs — DhcpLookup: query v4 and v6 lease stores by IP, return (hostname, profile) if found
- src/client_resolver/hostname_map.rs — HostnameMap: map hostname to profile via [dhcp_integration].hostname_map from policy.toml per PRD line 1052
- Integration: ClientResolver holds references to lease stores (Arc<dyn LeaseStoreV4>, Arc<dyn LeaseStoreV6>)
- Unit tests for DHCP lease lookup, hostname map, full resolution order

**Out of scope:**
- DHCPv4/v6 server core (stories 04-001, 04-002 — this story reads from their lease stores)
- PolicyEngine changes (story 02-001 already uses ClientResolver)
- REST API for client mapping (story 05-004)

## Sub-Tasks

- [ ] Create src/client_resolver/dhcp_lookup.rs with DhcpLookup: query_v4(ip) -> Option<(hostname, profile)>, query_v6(ip) -> Option<(hostname, profile)>, using LeaseStoreV4 and LeaseStoreV6 traits
  **Verify**: `cargo test --lib client_resolver::dhcp_lookup` → all pass (query v4 lease, query v6 lease, not found → None)
- [ ] Create src/client_resolver/hostname_map.rs with HostnameMap: resolve(hostname) -> Option<ProfileName>, using [dhcp_integration].hostname_map from policy.toml per PRD line 1052
  **Verify**: `cargo test --lib client_resolver::hostname_map` → all pass (match hostname, no match → None)
- [ ] Update src/client_resolver/mod.rs resolve() to implement full resolution order per PRD lines 963-968: DHCP static lease (MAC → IP → profile) → DHCP dynamic lease (IP → hostname → profile via hostname_map) → static IP mapping → CIDR range → default
  **Verify**: `cargo test --lib client_resolver` → all pass (test each resolution path, verify priority order)
- [ ] Wire lease store references into ClientResolver constructor (accept Arc<dyn LeaseStoreV4> and Arc<dyn LeaseStoreV6>)
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/client_resolver/mod.rs` — update resolve() with DHCP lookup (modified)
- `src/client_resolver/dhcp_lookup.rs` — DhcpLookup (new)
- `src/client_resolver/hostname_map.rs` — HostnameMap (new)

## Acceptance Criteria

- [ ] ClientResolver checks DHCP static leases first (highest priority)
- [ ] ClientResolver checks DHCP dynamic leases second (IP → hostname → profile)
- [ ] Hostname map from policy.toml is used for dynamic lease profile assignment
- [ ] Static IP and CIDR matching work as fallback
- [ ] Default profile is returned when no match is found
- [ ] Resolution order matches PRD lines 963-968
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib client_resolver` — tests for DHCP lookup, hostname map, full resolution order
- Integration: `cargo test --test integration_test` — verify policy works with DHCP-identified clients
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log client resolution (IP, resolved hostname, profile, source: dhcp-static/dhcp-dynamic/static-ip/cidr/default)

## Compliance

- DHCP lease table contains MAC-to-IP mappings — used for policy resolution, not exposed externally
- Client IP is transient metadata for policy decisions

## Risks & Mitigations

- Risk: Lease store query adds latency to hot path — Mitigation: SQLite queries are fast (<1ms for indexed lookup), cache results if needed
- Risk: Lease store not available (DHCP disabled) — Mitigation: Gracefully handle None, fall through to static IP/CIDR

## Dependencies & Sequencing

- Depends on: 02-001 (ClientResolver from Phase 02)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- LeaseStoreV4/V6 traits from stories 04-001/04-002 are not compatible with ClientResolver
- Resolution order does not match PRD lines 963-968

## Maintenance Notes

- ClientResolver should gracefully handle DHCP being disabled (no lease stores)
- Reviewers should verify the resolution priority order is correct

## Commit Conventions

- `feat(client-resolver): add DHCP lease table lookup for client identification`
- `feat(client-resolver): add hostname map for dynamic lease profile assignment`

## Changelog

- 2026-08-16: initialized story file
