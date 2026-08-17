---
story_id: "04-004"
story_title: "MAC blocklist + client classification + per-pool options"
story_name: "mac-blocklist-and-client-classification"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 4
branch: "feature/current/dnshub/story-04-004-mac-blocklist-and-client-classification"
status: "done"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "classification"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "dhcp", "mac-blocklist", "classification"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement MAC blocklist (refuse DHCP to specific MAC addresses or OUI vendor prefixes), client classification (auto-assign devices to pools/profiles based on vendor class option 60, MAC OUI, or user class option 77), per-pool/per-subnet DHCP options (different pools get different DNS, router, NTP options), lease time per-profile, and arbitrary DHCP options (configurable `option N = value` for any RFC 2132 option).

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig (from story 01-004), to be expanded with classification and MAC blocklist config
  - PRD lines 394-446 define MAC blocklist, classification, per-pool options, arbitrary options
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub
- **Repository conventions:** Use dhcproto for option parsing. MAC blocklist stored in SQLite (dhcp_mac_blocklist table). Classification rules in TOML config.
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
- src/dhcp/classification/mod.rs — ClientClassifier: classify(mac, vendor_class, user_class) -> Option<ProfileName> based on rules
- src/dhcp/classification/rules.rs — ClassificationRule: match by vendor_class (prefix), OUI (first 3 octets of MAC), user_class (exact match)
- src/dhcp/mac_blocklist.rs — MacBlocklist: is_blocked(mac) -> bool, check full MAC and OUI prefix, add/remove blocks, backed by SQLite dhcp_mac_blocklist table per PRD lines 733-738
- src/dhcp/options/arbitrary.rs — ArbitraryOption: parse and encode `option N = value` from config, support both standard and vendor-encapsulated options (43/125)
- src/dhcp/pool_options.rs — PerPoolOptions: merge pool-specific options with global options, lease time per-profile override
- src/config/dhcp.rs — expand with [[dhcp.mac_blocklist]], [[dhcp.classify]], per-pool options, lease_time_hours per static lease
- SQLite schema: dhcp_mac_blocklist table per PRD lines 733-738
- Unit tests for classification, MAC blocklist, arbitrary options, per-pool options

**Out of scope:**
- DHCPv4/v6 server core (stories 04-001, 04-002 — this story provides modules that the DHCP servers call)
- Audit log (story 04-008)
- REST API for MAC blocklist management (story 05-004)

## Sub-Tasks

- [x] Create src/dhcp/classification/rules.rs with ClassificationRule: VendorClassMatch (prefix on option 60), OuiMatch (first 3 octets), UserClassMatch (exact on option 77)
  **Verify**: `cargo test --lib dhcp::classification::rules` → all pass (match by vendor class, OUI, user class)
- [x] Create src/dhcp/classification/mod.rs with ClientClassifier: classify(mac, vendor_class, user_class) -> Option<ProfileName>, iterate rules in order, return first match
  **Verify**: `cargo test --lib dhcp::classification` → all pass (multiple rules, first match wins, no match → None)
- [x] Create src/dhcp/mac_blocklist.rs with MacBlocklist: is_blocked(mac) -> bool (check full MAC then OUI prefix), add_mac_block(mac_or_oui, reason), remove_mac_block, list_mac_blocks, backed by SQLite
  **Verify**: `cargo test --lib dhcp::mac_blocklist` → all pass (block by full MAC, block by OUI, unblock, list)
- [x] Create src/dhcp/options/arbitrary.rs with ArbitraryOption: parse "N = value" from config, encode to DHCP option bytes, support string, IP, and hex value types
  **Verify**: `cargo test --lib dhcp::options::arbitrary` → all pass (parse and encode various option types)
- [x] Create src/dhcp/pool_options.rs with PerPoolOptions: merge global options with pool-specific overrides, apply lease_time per-profile (shorter for kids, longer for IoT per PRD lines 427-429)
  **Verify**: `cargo test --lib dhcp::pool_options` → all pass (merge options, override lease time)
- [x] Update src/config/dhcp.rs with [[dhcp.mac_blocklist]] (mac, oui, reason), [[dhcp.classify]] (match, profile), per-pool options, lease_time_hours per static lease per PRD lines 810-946
  **Verify**: `cargo build` → exit 0
- [x] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/classification/mod.rs` — ClientClassifier (new)
- `src/dhcp/classification/rules.rs` — ClassificationRule (new)
- `src/dhcp/mac_blocklist.rs` — MacBlocklist (new)
- `src/dhcp/options/arbitrary.rs` — ArbitraryOption (new)
- `src/dhcp/pool_options.rs` — PerPoolOptions (new)
- `src/config/dhcp.rs` — expand with classification and MAC blocklist config (modified)

## Acceptance Criteria

- [x] Client classification works by vendor class (option 60 prefix match)
- [x] Client classification works by MAC OUI (first 3 octets)
- [x] Client classification works by user class (option 77 exact match)
- [x] MAC blocklist blocks by full MAC address
- [x] MAC blocklist blocks by OUI vendor prefix
- [x] Arbitrary DHCP options can be configured and encoded
- [x] Per-pool options override global options
- [x] Lease time per-profile works (different lease times for different profiles)
- [x] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::classification` and `cargo test --lib dhcp::mac_blocklist` and `cargo test --lib dhcp::options` — tests for all modules
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log classification decisions (MAC, matched rule, assigned profile)
- Log MAC blocklist blocks (MAC, reason)

## Compliance

- MAC addresses are device identifiers — treat as network infrastructure metadata
- MAC blocklist is a security measure

## Risks & Mitigations

- Risk: OUI database may be outdated — Mitigation: OUI matching uses configured prefixes, not a database
- Risk: Arbitrary option encoding may not handle all RFC 2132 types — Mitigation: Support common types (string, IP, hex), document limitations

## Dependencies & Sequencing

- Depends on: 01-004 (config loading)
- Unblocks: None directly (modules are called by DHCP servers 04-001, 04-002)

## Definition of Done

- [x] All verification commands from sub-tasks pass
- [x] Code, tests, docs updated; CI green
- [x] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- dhcproto cannot parse vendor class (option 60) or user class (option 77)
- Arbitrary option encoding fails for standard option types

## Maintenance Notes

- Classification rules are evaluated in order — first match wins
- MAC blocklist checks full MAC first, then OUI prefix
- Reviewers should verify per-pool option merging logic

## Commit Conventions

- `feat(dhcp): add client classification by vendor class, OUI, and user class`
- `feat(dhcp): add MAC blocklist with full MAC and OUI prefix support`
- `feat(dhcp): add arbitrary DHCP options and per-pool option overrides`

## Changelog

- 2026-08-16: initialized story file
- 2026-08-16: implemented story 04-004 — added ClassificationRule (vendor class prefix, OUI, user class exact) and ClientClassifier (first-match-wins), MacBlocklist backed by SQLite dhcp_mac_blocklist table (full MAC + OUI prefix, add/remove/list), ArbitraryOption (string/IP/hex value inference → dhcproto DhcpOption::Unknown), PerPoolOptions (global+pool merge, per-profile lease-time precedence), and expanded DhcpConfig with [[dhcp.pools]], [[dhcp.mac_blocklist]], [[dhcp.classify]], [[dhcp.static]] (config/dhcp.rs moved to its own submodule and re-exported). 54 new unit tests, all passing.
