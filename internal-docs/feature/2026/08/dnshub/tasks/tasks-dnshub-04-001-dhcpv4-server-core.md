---
story_id: "04-001"
story_title: "DHCPv4 server core (lease store, state machine, pool allocator, options)"
story_name: "dhcpv4-server-core"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 1
branch: "feature/current/dnshub/story-04-001-dhcpv4-server-core"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "dhcpv4"]
priority: "MUST"
risk_level: "high"
tags: ["feat", "backend", "dhcp", "dhcpv4", "leases"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement the DHCPv4 server core: LeaseStoreV4 trait + SqliteLeaseStoreV4, DHCP state machine (DISCOVER → OFFER → REQUEST → ACK → RELEASE/DECLINE), IP pool allocator with conflict detection (ping before offer), and standard DHCP options (router, DNS, domain, lease time, broadcast, NTP, classless static routes). Uses dhcproto crate for wire format encoding/decoding.

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig stub (from story 01-004), to be expanded with full v4 config
  - PRD section 4.4 (lines 369-946) defines full DHCP scope
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub with enabled, interface, listen fields
- **Repository conventions:** Use dhcproto 0.12 for DHCP wire format. Use rusqlite for lease persistence. Use async-trait for LeaseStore trait. Use tokio for async UDP server on port 67.
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
- src/dhcp/mod.rs — module root with DhcpServer, DhcpConfig full struct
- src/dhcp/v4/mod.rs — DHCPv4 server: UDP listener on port 67, message dispatch
- src/dhcp/v4/state_machine.rs — DHCP state machine: handle DISCOVER (offer), REQUEST (ack), RELEASE, DECLINE, INFORM
- src/dhcp/v4/pool.rs — IP pool allocator: find_free_ip, ping-before-offer conflict detection, pool range management
- src/dhcp/v4/options.rs — DHCP option builder: option 3 (router), 6 (DNS), 15 (domain), 28 (broadcast), 42 (NTP), 51 (lease time), 121 (classless static routes), 252 (WPAD)
- src/dhcp/v4/lease_store.rs — LeaseStoreV4 trait + SqliteLeaseStoreV4: CRUD for dhcp_leases and dhcp_static_leases tables per PRD schema (lines 693-754)
- src/dhcp/v4/config.rs — DhcpV4Config serde structs: enabled, interface, listen, domain, ntp_server, pools (name, subnet, pool_start, pool_end, router, lease_time_hours, options), static leases
- src/config/dhcp.rs — expand DhcpConfig with full v4 fields
- SQLite schema: dhcp_leases, dhcp_static_leases tables per PRD lines 693-711
- Unit tests for state machine, pool allocator, lease store, option builder

**Out of scope:**
- DHCPv6 (story 04-002)
- RA/SLAAC (story 04-003)
- MAC blocklist + client classification (story 04-004)
- DDNS (story 04-005)
- PXE/BOOTP/TFTP (story 04-006)
- DHCP relay (story 04-007)
- Lease audit log + rogue detection (story 04-008)
- DoT/DoH (stories 04-009, 04-010)
- ClientResolver integration (story 04-011)
- Docker macvlan (story 04-012)

## Sub-Tasks

- [ ] Create src/dhcp/v4/config.rs with DhcpV4Config: enabled, interface, listen, domain, ntp_server, pools (Vec<DhcpPoolV4>), static_leases (Vec<StaticLeaseV4>) per PRD lines 758-946
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dhcp/v4/lease_store.rs with LeaseStoreV4 trait (async_trait): get_lease, get_lease_by_mac, insert_lease, update_lease, delete_lease, list_leases, find_free_ip, get_static_lease, upsert_static_lease. Implement SqliteLeaseStoreV4 with rusqlite.
  **Verify**: `cargo test --lib dhcp::v4::lease_store` → all pass (CRUD operations on test SQLite DB)
- [ ] Create SQLite schema initialization: dhcp_leases and dhcp_static_leases tables per PRD lines 693-711
  **Verify**: `cargo test --lib dhcp::v4::lease_store` → all pass (tables created, schema matches PRD)
- [ ] Create src/dhcp/v4/pool.rs with PoolAllocator: find_free_ip(pool, store) -> Option<Ipv4Addr>, ping_before_offer(ip) -> bool (conflict detection), allocate from pool range
  **Verify**: `cargo test --lib dhcp::v4::pool` → all pass (find free IP, skip allocated, detect conflict)
- [ ] Create src/dhcp/v4/options.rs with DhcpOptionBuilder: build options 3, 6, 15, 28, 42, 51, 121, 252 from pool config
  **Verify**: `cargo test --lib dhcp::v4::options` → all pass (each option encodes correctly per RFC 2132)
- [ ] Create src/dhcp/v4/state_machine.rs with DhcpStateMachine: handle_discover (find or allocate IP, send OFFER), handle_request (confirm lease, send ACK), handle_release (free lease), handle_decline (mark IP as conflicted)
  **Verify**: `cargo test --lib dhcp::v4::state_machine` → all pass (DISCOVER→OFFER, REQUEST→ACK, RELEASE, DECLINE)
- [ ] Create src/dhcp/v4/mod.rs with DhcpV4Server: UDP listener on port 67, parse incoming DHCP messages via dhcproto, dispatch to state machine, send responses
  **Verify**: `cargo build` → exit 0
- [ ] Update src/config/dhcp.rs with full DhcpConfig including v4 section
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/mod.rs` — module root (new)
- `src/dhcp/v4/mod.rs` — DHCPv4 server (new)
- `src/dhcp/v4/state_machine.rs` — DHCP state machine (new)
- `src/dhcp/v4/pool.rs` — IP pool allocator (new)
- `src/dhcp/v4/options.rs` — DHCP option builder (new)
- `src/dhcp/v4/lease_store.rs` — LeaseStoreV4 + SqliteLeaseStoreV4 (new)
- `src/dhcp/v4/config.rs` — DhcpV4Config (new)
- `src/config/dhcp.rs` — expand DhcpConfig (modified)

## Acceptance Criteria

- [ ] DHCPv4 server listens on port 67 UDP and processes DHCP messages
- [ ] DISCOVER → OFFER: server allocates IP and sends DHCPOFFER
- [ ] REQUEST → ACK: server confirms lease and sends DHCPACK
- [ ] RELEASE: server frees the lease
- [ ] DECLINE: server marks IP as conflicted
- [ ] Static leases (MAC → IP) are honored
- [ ] Pool allocator finds free IPs and detects conflicts (ping before offer)
- [ ] Standard DHCP options (3, 6, 15, 28, 42, 51, 121, 252) are correctly encoded
- [ ] Lease persistence in SQLite survives restart
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::v4` — tests for state machine, pool, options, lease store
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start DHCPv4 server, use `dhclient` or `dhcping` to request a lease

## Observability

- Log DHCP events (DISCOVER, OFFER, REQUEST, ACK, RELEASE, DECLINE) with client MAC and IP
- Metrics for DHCP operations deferred to story 05-001

## Compliance

- DHCP lease table contains MAC addresses and hostnames — treat as network infrastructure data
- Lease audit log in story 04-008 provides security visibility

## Risks & Mitigations

- Risk: No mature Rust DHCP server library exists — Mitigation: Build custom state machine using dhcproto for wire format only; state machine is well-defined (4-message exchange)
- Risk: L2 broadcast requirements in Docker — Mitigation: Use macvlan networking (story 04-012); for testing, use host network mode
- Risk: Lease conflict detection (ping) may not work in all network configs — Mitigation: Make ping-before-offer configurable, default to enabled
- Risk: SQLite concurrency for lease writes — Mitigation: Use WAL mode, serialize writes via single writer task

## Dependencies & Sequencing

- Depends on: 01-004 (config loading for DhcpConfig)
- Unblocks: 05-004 (REST API exposes DHCP lease data)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- dhcproto crate cannot encode/decode DHCPv4 messages correctly
- SQLite lease store has concurrency issues under test
- DHCP state machine logic does not match RFC 2131

## Maintenance Notes

- LeaseStoreV4 trait is designed for pluggable backends (FailoverLeaseStore in Phase 7)
- The state machine should be extensible for client classification hooks (story 04-004)
- Reviewers should verify RFC 2131 compliance for the 4-message exchange

## Commit Conventions

- `feat(dhcp): add DHCPv4 lease store with SQLite persistence`
- `feat(dhcp): add DHCPv4 state machine (DISCOVER/OFFER/REQUEST/ACK)`
- `feat(dhcp): add DHCPv4 pool allocator with conflict detection`
- `feat(dhcp): add DHCPv4 standard options builder`

## Changelog

- 2026-08-16: initialized story file
