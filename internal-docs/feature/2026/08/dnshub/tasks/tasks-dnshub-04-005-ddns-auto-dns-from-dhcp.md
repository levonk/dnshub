---
story_id: "04-005"
story_title: "DDNS (auto-create/remove DNS records from DHCP leases)"
story_name: "ddns-auto-dns-from-dhcp"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 5
branch: "feature/current/dnshub/story-04-005-ddns-auto-dns-from-dhcp"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["dhcp", "ddns"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "dhcp", "ddns", "dns"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement DDNS (Dynamic DNS from DHCP): when a device gets a DHCP lease and includes a hostname (option 12), automatically create/update a DNS A/AAAA record in the local zone. When the lease expires or is released, remove the record. This makes local DNS "just work" — devices appear in DNS automatically. Both DHCPv4 (A records) and DHCPv6 (AAAA records) are supported.

## Current State

- **Relevant files and their roles:**
  - `src/dns/mod.rs` — DnshubHandler with local zone support (from story 01-001)
  - PRD lines 397-402 define DDNS scope
- **Existing code excerpts:**
  - `src/dns/mod.rs` — DnshubHandler chain with InMemoryZoneHandler for local zones
- **Repository conventions:** Use hickory-server zone management for local zone updates. DDNS updates are atomic (remove old record, add new record). Short TTL (60s per PRD) so stale records expire fast.
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
- src/dhcp/ddns/mod.rs — DdnsManager: subscribe to DHCP lease events (grant, release, expire), update local DNS zone accordingly
- src/dhcp/ddns/zone_updater.rs — ZoneUpdater: add_a_record(hostname, ip, ttl), remove_a_record(hostname, ip), add_aaaa_record(hostname, ipv6, ttl), remove_aaaa_record(hostname, ipv6) in the local zone
- src/dhcp/ddns/config.rs — DdnsConfig: enabled, zone (local zone name), ttl (default 60) per PRD lines 767-770
- src/dhcp/ddns/events.rs — LeaseEvent enum: Grant(mac, ip, hostname), Release(mac, ip, hostname), Expire(mac, ip, hostname)
- Integration: DdnsManager listens on a tokio channel for lease events from DHCP servers
- Unit tests for zone record add/remove, event handling

**Out of scope:**
- DHCPv4/v6 server core (stories 04-001, 04-002 — this story provides the event consumer)
- RFC 2136 dynamic DNS update protocol (dnshub updates its own local zone directly, not via DNS UPDATE messages)
- Reverse DNS (PTR records) — future
- DDNS for external DNS servers — future

## Sub-Tasks

- [ ] Create src/dhcp/ddns/config.rs with DdnsConfig: enabled (bool), zone (String), ttl (u32, default 60) per PRD lines 767-770
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dhcp/ddns/events.rs with LeaseEvent enum: Grant { mac, ip, hostname }, Release { mac, ip, hostname }, Expire { mac, ip, hostname }
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dhcp/ddns/zone_updater.rs with ZoneUpdater: add_a_record(hostname, ip, ttl), remove_a_record(hostname, ip), add_aaaa_record(hostname, ipv6, ttl), remove_aaaa_record(hostname, ipv6) using hickory-server zone management API
  **Verify**: `cargo test --lib dhcp::ddns::zone_updater` → all pass (add A record, query it, remove it, verify gone)
- [ ] Create src/dhcp/ddns/mod.rs with DdnsManager: subscribe to LeaseEvent channel, on Grant → add DNS record, on Release/Expire → remove DNS record, skip events without hostname
  **Verify**: `cargo test --lib dhcp::ddns` → all pass (send Grant event, verify record added; send Release, verify removed)
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/ddns/mod.rs` — DdnsManager (new)
- `src/dhcp/ddns/zone_updater.rs` — ZoneUpdater (new)
- `src/dhcp/ddns/config.rs` — DdnsConfig (new)
- `src/dhcp/ddns/events.rs` — LeaseEvent enum (new)

## Acceptance Criteria

- [ ] DHCP lease grant with hostname creates A/AAAA record in local zone
- [ ] DHCP lease release removes the DNS record
- [ ] DHCP lease expiry removes the DNS record
- [ ] Events without hostname are skipped (no empty-named records)
- [ ] TTL is configurable (default 60s per PRD)
- [ ] Both A (IPv4) and AAAA (IPv6) records are supported
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::ddns` — tests for zone updater and event handling
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log DDNS events (hostname, IP, action: add/remove, zone)
- Log DDNS errors (zone update failure, record conflict)

## Compliance

- Hostnames in DNS are device-provided (DHCP option 12) — treat as device metadata
- No personal data beyond device identification

## Risks & Mitigations

- Risk: Hostname conflicts (two devices with same hostname) — Mitigation: Last-write-wins, or append MAC suffix to hostname for uniqueness
- Risk: Zone update API in hickory-server 0.26 may differ — Mitigation: Check 0.26 API for zone record manipulation

## Dependencies & Sequencing

- Depends on: 01-001 (local zone handler in DNS server)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- hickory-server 0.26 does not expose zone record add/remove API
- Zone updates cause panics or data corruption

## Maintenance Notes

- Consider hostname conflict resolution strategy (append suffix, refuse duplicate, last-write-wins)
- Reviewers should verify records are actually queryable after DDNS update

## Commit Conventions

- `feat(dhcp): add DDNS manager for automatic DNS record updates`
- `feat(dhcp): add zone updater for A/AAAA record management`

## Changelog

- 2026-08-16: initialized story file
