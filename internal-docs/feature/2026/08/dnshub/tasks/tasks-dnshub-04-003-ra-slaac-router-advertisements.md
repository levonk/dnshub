---
story_id: "04-003"
story_title: "RA/SLAAC (Router Advertisements with RDNSS/DNSSL)"
story_name: "ra-slaac-router-advertisements"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 3
branch: "feature/current/dnshub/story-04-003-ra-slaac-router-advertisements"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "ra-slaac"]
priority: "MUST"
risk_level: "medium"
tags: ["feat", "backend", "dhcp", "ipv6", "ra", "slaac"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement IPv6 Router Advertisement (RA) sending for SLAAC (Stateless Address Autoconfiguration). Send RA messages on configured interfaces with prefix information, preferred/valid lifetimes, router lifetime, RDNSS (DNS server in RA, RFC 8106), and DNSSL (domain search list in RA). This enables devices to self-assign IPv6 addresses via SLAAC, which is the dominant IPv6 allocation mode.

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig stub (from story 01-004), RA config to be added
  - PRD lines 840-848 define RA configuration
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub
- **Repository conventions:** Use ndisc crate or raw ICMPv6 for Router Advertisements. RA messages sent periodically and in response to Router Solicitations (RS).
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
- src/dhcp/ra/mod.rs — RA sender module: periodic RA transmission, RS (Router Solicitation) response
- src/dhcp/ra/message.rs — RA message builder: ICMPv6 type 134, prefix information option (type 3), RDNSS option (type 25, RFC 8106), DNSSL option (type 31)
- src/dhcp/ra/config.rs — RaConfig: enabled, prefix, preferred_lifetime_secs, valid_lifetime_secs, router_lifetime_secs, rdnss (Vec<Ipv6Addr>), dnssl (Vec<String>)
- src/config/dhcp.rs — add [dhcp.v6.ra] section per PRD lines 840-848
- Unit tests for RA message construction, option encoding

**Out of scope:**
- DHCPv6 stateful server (story 04-002)
- SLAAC address generation (handled by client OS, not dnshub)
- RA-based DNS discovery via RDNSS is in scope; full RFC 8106 compliance
- M/O flag management (managed/other config flags for DHCPv6)

## Sub-Tasks

- [ ] Create src/dhcp/ra/config.rs with RaConfig: enabled, prefix (String), preferred_lifetime_secs (u32), valid_lifetime_secs (u32), router_lifetime_secs (u32), rdnss (Vec<String>), dnssl (Vec<String>) per PRD lines 840-848
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dhcp/ra/message.rs with RA message builder: construct ICMPv6 Router Advertisement (type 134), Prefix Information option (type 3) with prefix and lifetimes, RDNSS option (type 25) with DNS servers, DNSSL option (type 31) with domain search list
  **Verify**: `cargo test --lib dhcp::ra::message` → all pass (verify option types, lifetimes, addresses encoded correctly)
- [ ] Create src/dhcp/ra/mod.rs with RaSender: periodic RA transmission (every router_lifetime_secs/3), respond to Router Solicitations (ICMPv6 type 133) with immediate RA, send to all-nodes multicast (ff02::1)
  **Verify**: `cargo build` → exit 0
- [ ] Update src/config/dhcp.rs with [dhcp.v6.ra] section
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/ra/mod.rs` — RA sender (new)
- `src/dhcp/ra/message.rs` — RA message builder (new)
- `src/dhcp/ra/config.rs` — RaConfig (new)
- `src/config/dhcp.rs` — add RA config section (modified)

## Acceptance Criteria

- [ ] RA messages are constructed with correct ICMPv6 type (134) and options
- [ ] Prefix Information option includes prefix, preferred/valid lifetimes
- [ ] RDNSS option includes configured DNS servers per RFC 8106
- [ ] DNSSL option includes configured domain search list
- [ ] RA sender transmits periodically and responds to Router Solicitations
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::ra` — tests for message construction and option encoding
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start RA sender on a test interface, use `rdisc6` or `ip -6 route show` to verify RA received

## Observability

- Log RA transmission events (interface, prefix, interval)
- Log Router Solicitation reception

## Compliance

- RA messages are network infrastructure, no personal data
- RFC 4861 (Neighbor Discovery) and RFC 8106 (RDNSS) compliance

## Risks & Mitigations

- Risk: Raw ICMPv6 socket requires root/CAP_NET_RAW — Mitigation: Document permission requirement, Docker macvlan provides this
- Risk: RA conflicts with other routers on the network — Mitigation: Configurable router_lifetime (set to 0 to stop advertising as router while still sending DNS info)

## Dependencies & Sequencing

- Depends on: 01-004 (config loading)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- ICMPv6 raw socket cannot be created in the test environment
- RA message format does not match RFC 4861

## Maintenance Notes

- RA sender should coexist with DHCPv6 stateful server (story 04-002) — M/O flags control whether clients use SLAAC, DHCPv6, or both
- Reviewers should verify RDNSS and DNSSL option encoding per RFC 8106

## Commit Conventions

- `feat(dhcp): add IPv6 Router Advertisement sender with RDNSS/DNSSL`
- `feat(dhcp): add RA config section`

## Changelog

- 2026-08-16: initialized story file
