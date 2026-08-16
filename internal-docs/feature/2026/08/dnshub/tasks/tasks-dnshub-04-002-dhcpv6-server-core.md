---
story_id: "04-002"
story_title: "DHCPv6 server core (lease store, state machine, IA_NA, options)"
story_name: "dhcpv6-server-core"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 2
branch: "feature/current/dnshub/story-04-002-dhcpv6-server-core"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "dhcpv6"]
priority: "MUST"
risk_level: "high"
tags: ["feat", "backend", "dhcp", "dhcpv6", "ipv6"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement the DHCPv6 server core: LeaseStoreV6 trait + SqliteLeaseStoreV6, DHCPv6 state machine (SOLLICIT → ADVERTISE → REQUEST → REPLY, RENEW, REBIND), IA_NA (Identity Association for Non-temporary Addresses) allocation, DUID-based client identification, and DHCPv6 options (DNS server option 23, domain search list option 24, NTP server option 56, information refresh time option 32).

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig stub (from story 01-004), to be expanded with v6 config
  - PRD section 4.4 (lines 440-448) defines DHCPv6 scope
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub
- **Repository conventions:** Use dhcproto 0.12 for DHCPv6 wire format. Use rusqlite for lease persistence. Use async-trait for LeaseStoreV6 trait. Use tokio for async UDP server on port 547.
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
- src/dhcp/v6/mod.rs — DHCPv6 server: UDP listener on port 547, message dispatch
- src/dhcp/v6/state_machine.rs — DHCPv6 state machine: handle SOLLICIT (advertise), REQUEST (reply/reply), RENEW, REBIND, RELEASE, CONFIRM, INFORMATION-REQUEST
- src/dhcp/v6/ia_na.rs — IA_NA allocation: manage Identity Associations, allocate IPv6 addresses from pool, handle T1/T2 timers
- src/dhcp/v6/duid.rs — DUID parsing and generation: DUID-LLT, DUID-EN, DUID-LL per RFC 3315
- src/dhcp/v6/options.rs — DHCPv6 option builder: option 23 (DNS_SERVER), 24 (DOMAIN_LIST), 32 (INFO_REFRESH_TIME), 56 (NTP_SERVER)
- src/dhcp/v6/lease_store.rs — LeaseStoreV6 trait + SqliteLeaseStoreV6: CRUD for dhcpv6_leases and dhcpv6_static_leases tables per PRD schema (lines 713-731)
- src/dhcp/v6/config.rs — DhcpV6Config: enabled, listen, domain_search, lease_time_hours, ntp_server, pools (name, prefix, pool_start, pool_end, dns_servers)
- SQLite schema: dhcpv6_leases, dhcpv6_static_leases tables per PRD lines 713-731
- Unit tests for state machine, IA_NA, DUID, options, lease store

**Out of scope:**
- RA/SLAAC (story 04-003)
- Prefix delegation (IA_PD — future)
- DHCPv6 relay (story 04-007)
- DDNS for DHCPv6 (story 04-005)
- Stateful + stateless mode switching (both supported, but no dynamic mode switching)

## Sub-Tasks

- [ ] Create src/dhcp/v6/config.rs with DhcpV6Config: enabled, listen, domain_search (Vec<String>), lease_time_hours, ntp_server, pools (Vec<DhcpPoolV6>) per PRD lines 832-855
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dhcp/v6/duid.rs with DUID parsing: parse DUID-LLT, DUID-EN, DUID-LL from client messages, generate server DUID
  **Verify**: `cargo test --lib dhcp::v6::duid` → all pass (parse each DUID type, round-trip)
- [ ] Create src/dhcp/v6/lease_store.rs with LeaseStoreV6 trait + SqliteLeaseStoreV4: get_lease_v6, get_lease_by_duid_v6, insert/update/delete/list, find_free_ipv6, static lease CRUD per PRD lines 538-552
  **Verify**: `cargo test --lib dhcp::v6::lease_store` → all pass (CRUD on test SQLite DB)
- [ ] Create SQLite schema: dhcpv6_leases and dhcpv6_static_leases tables per PRD lines 713-731
  **Verify**: `cargo test --lib dhcp::v6::lease_store` → all pass (tables created, schema matches)
- [ ] Create src/dhcp/v6/ia_na.rs with IA_NA management: allocate IPv6 from pool, handle IAID, set T1/T2 (preferred/valid lifetimes), track IA state
  **Verify**: `cargo test --lib dhcp::v6::ia_na` → all pass (allocate, renew, release)
- [ ] Create src/dhcp/v6/options.rs with DHCPv6 option builder: option 23 (DNS_SERVER), 24 (DOMAIN_LIST), 32 (INFO_REFRESH_TIME), 56 (NTP_SERVER) per RFC 3315
  **Verify**: `cargo test --lib dhcp::v6::options` → all pass (each option encodes correctly)
- [ ] Create src/dhcp/v6/state_machine.rs with DhcpV6StateMachine: handle SOLLICIT (advertise), REQUEST (reply), RENEW (extend), REBIND, RELEASE, CONFIRM, INFORMATION-REQUEST
  **Verify**: `cargo test --lib dhcp::v6::state_machine` → all pass (SOLLICIT→ADVERTISE, REQUEST→REPLY, RENEW, RELEASE)
- [ ] Create src/dhcp/v6/mod.rs with DhcpV6Server: UDP listener on port 547, parse via dhcproto, dispatch to state machine
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/v6/mod.rs` — DHCPv6 server (new)
- `src/dhcp/v6/state_machine.rs` — DHCPv6 state machine (new)
- `src/dhcp/v6/ia_na.rs` — IA_NA management (new)
- `src/dhcp/v6/duid.rs` — DUID parsing (new)
- `src/dhcp/v6/options.rs` — DHCPv6 options (new)
- `src/dhcp/v6/lease_store.rs` — LeaseStoreV6 + SqliteLeaseStoreV6 (new)
- `src/dhcp/v6/config.rs` — DhcpV6Config (new)

## Acceptance Criteria

- [ ] DHCPv6 server listens on port 547 UDP
- [ ] SOLLICIT → ADVERTISE: server allocates IPv6 and sends Advertise
- [ ] REQUEST → REPLY: server confirms lease and sends Reply
- [ ] RENEW: server extends lease (updates T1/T2 timers)
- [ ] DUID-based client identification works
- [ ] IA_NA allocation from pool works
- [ ] DHCPv6 options (23, 24, 32, 56) are correctly encoded
- [ ] Lease persistence in SQLite survives restart
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::v6` — tests for state machine, IA_NA, DUID, options, lease store
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log DHCPv6 events (SOLLICIT, ADVERTISE, REQUEST, REPLY, RENEW, RELEASE) with DUID and IPv6
- Metrics deferred to story 05-001

## Compliance

- DHCPv6 lease table contains DUIDs and hostnames — network infrastructure data
- IPv6 privacy extensions not in scope (SLAAC in story 04-03 handles privacy addresses)

## Risks & Mitigations

- Risk: DHCPv6 wire format complexity (DUID, IA_NA, IA_TA options) — Mitigation: Use dhcproto for encoding/decoding, test with known-good DHCPv6 client
- Risk: DUID format variations — Mitigation: Support all 3 DUID types (LLT, EN, LL), test with each
- Risk: IPv6 pool allocation with /64 prefixes — Mitigation: Simple sequential allocation within pool_start to pool_end range

## Dependencies & Sequencing

- Depends on: 01-004 (config loading)
- Unblocks: 05-004 (REST API exposes DHCPv6 lease data)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- dhcproto cannot encode/decode DHCPv6 messages
- DUID parsing fails for standard client DUID formats
- IA_NA allocation logic does not match RFC 3315

## Maintenance Notes

- IA_PD (prefix delegation) deferred to future — design IA_NA to be extensible
- Reviewers should verify RFC 3315 compliance for SOLLICIT/ADVERTISE/REQUEST/REPLY exchange

## Commit Conventions

- `feat(dhcp): add DHCPv6 lease store with SQLite persistence`
- `feat(dhcp): add DHCPv6 state machine (SOLLICIT/ADVERTISE/REQUEST/REPLY)`
- `feat(dhcp): add DHCPv6 IA_NA allocation and DUID parsing`
- `feat(dhcp): add DHCPv6 standard options`

## Changelog

- 2026-08-16: initialized story file
