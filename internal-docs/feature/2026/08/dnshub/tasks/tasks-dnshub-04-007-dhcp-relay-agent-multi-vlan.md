---
story_id: "04-007"
story_title: "DHCP relay agent + multi-VLAN + Option 82"
story_name: "dhcp-relay-agent-multi-vlan"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 7
branch: "feature/current/dnshub/story-04-007-dhcp-relay-agent-multi-vlan"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "relay"]
priority: "SHOULD"
risk_level: "high"
tags: ["feat", "backend", "dhcp", "relay", "vlan", "option-82"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement DHCP relay agent support: receive relayed DHCP requests (giaddr field set) from routers/switches on other VLANs, select the appropriate IP pool based on giaddr or link-address (v6), send responses back to the relay agent (unicast to giaddr), parse Option 82 (Relay Agent Information) for circuit ID and remote ID sub-options, and support DHCPv6 relay (Relay-Forw/Relay-Rel messages). Includes relay trust configuration (only accept from configured agent IPs).

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig (from story 01-004), to be expanded with relay config
  - PRD lines 472-495 define DHCP relay scope
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub
- **Repository conventions:** Use dhcproto for relay message parsing. Option 82 sub-options: circuit ID (sub-option 1), remote ID (sub-option 2). DHCPv6 relay uses Relay-Forw/Relay-Rel message types.
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
- src/dhcp/relay/mod.rs — RelayHandler: detect relayed requests (giaddr != 0), select pool based on giaddr, send response unicast to giaddr
- src/dhcp/relay/option82.rs — Option82 parser: parse circuit ID (sub-option 1) and remote ID (sub-option 2), use for pool selection and profile assignment
- src/dhcp/relay/trust.rs — RelayTrust: only accept relayed requests from configured trusted agent IPs, reject untrusted relays
- src/dhcp/relay/v6_relay.rs — DHCPv6 relay: handle Relay-Forw/Relay-Rel messages, link-address based pool selection, nested relay messages
- src/dhcp/relay/config.rs — RelayConfig: enabled, trusted_agents (Vec<Ipv4Addr>), option82 (enabled, circuit_id_map: HashMap<String, String>) per PRD lines 888-910
- src/config/dhcp.rs — expand with [dhcp.relay] section and multi-VLAN pools
- Unit tests for relay detection, Option 82 parsing, trust checking, v6 relay

**Out of scope:**
- DHCPv4/v6 server core (stories 04-001, 04-002 — this story provides relay handling that wraps the DHCP server)
- DHCP relay agent functionality (dnshub as a relay, not a server receiving relays — dnshub is the server)
- Option 82 injection (dnshub parses Option 82 from relays, doesn't add it)

## Sub-Tasks

- [ ] Create src/dhcp/relay/config.rs with RelayConfig: enabled, trusted_agents (Vec<String>), option82 (Option82Config: enabled, circuit_id_map: HashMap<String, String>) per PRD lines 888-910
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dhcp/relay/trust.rs with RelayTrust: is_trusted(agent_ip) -> bool, check against configured trusted_agents list
  **Verify**: `cargo test --lib dhcp::relay::trust` → all pass (trusted IP accepted, untrusted rejected)
- [ ] Create src/dhcp/relay/option82.rs with Option82 parser: parse_relay_agent_info(options) -> Option<(circuit_id, remote_id)>, extract sub-option 1 (circuit ID) and sub-option 2 (remote ID)
  **Verify**: `cargo test --lib dhcp::relay::option82` → all pass (parse Option 82 with circuit ID and remote ID, no Option 82 → None)
- [ ] Create src/dhcp/relay/mod.rs with RelayHandler: is_relayed(message) -> bool (giaddr != 0), select_pool(giaddr, option82) -> PoolName, send response unicast to giaddr (not broadcast)
  **Verify**: `cargo test --lib dhcp::relay` → all pass (detect relay, select pool by giaddr, select pool by circuit_id_map)
- [ ] Create src/dhcp/relay/v6_relay.rs with DHCPv6 relay handling: parse Relay-Forw message, extract link-address, select pool based on link-address, build Relay-Rel response
  **Verify**: `cargo test --lib dhcp::relay::v6_relay` → all pass (parse Relay-Forw, extract link-address, build Relay-Rel)
- [ ] Update src/config/dhcp.rs with [dhcp.relay] section and multi-VLAN pool configuration (default_profile per pool) per PRD lines 888-931
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/relay/mod.rs` — RelayHandler (new)
- `src/dhcp/relay/option82.rs` — Option82 parser (new)
- `src/dhcp/relay/trust.rs` — RelayTrust (new)
- `src/dhcp/relay/v6_relay.rs` — DHCPv6 relay (new)
- `src/dhcp/relay/config.rs` — RelayConfig (new)
- `src/config/dhcp.rs` — expand with relay config (modified)

## Acceptance Criteria

- [ ] Relayed DHCP requests (giaddr != 0) are detected
- [ ] Pool is selected based on giaddr (gateway IP address)
- [ ] Option 82 circuit ID is parsed and used for pool/profile selection
- [ ] Responses are sent unicast to giaddr (not broadcast)
- [ ] Untrusted relay agents are rejected
- [ ] DHCPv6 Relay-Forw/Relay-Rel messages are handled
- [ ] DHCPv6 pool selection uses link-address
- [ ] Multi-VLAN pools with per-VLAN default profiles work
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::relay` — tests for relay detection, Option 82, trust, v6 relay
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log relay events (agent IP, giaddr, selected pool, circuit ID)
- Log untrusted relay attempts (agent IP, rejection)

## Compliance

- Relay trust configuration is a security measure — prevents rogue relays from exhausting pools
- Option 82 contains network topology info (switch port) — treat as infrastructure metadata

## Risks & Mitigations

- Risk: Option 82 format varies by relay agent vendor — Mitigation: Parse standard sub-options 1 and 2, log unparseable Option 82
- Risk: DHCPv6 relay message nesting (multiple Relay-Forw layers) — Mitigation: Handle up to 8 nested relay messages per RFC 8415
- Risk: Rogue relay agents exhausting IP pools — Mitigation: Relay trust configuration, only accept from configured IPs

## Dependencies & Sequencing

- Depends on: 01-004 (config loading)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- dhcproto cannot parse Option 82 or DHCPv6 Relay-Forw messages
- Relay trust logic has security bypasses

## Maintenance Notes

- Option 82 circuit_id_map maps circuit IDs to profiles — this is the mechanism for port-based profile assignment
- Reviewers should verify untrusted relay rejection works correctly

## Commit Conventions

- `feat(dhcp): add DHCP relay agent handler with giaddr-based pool selection`
- `feat(dhcp): add Option 82 parsing for circuit ID and remote ID`
- `feat(dhcp): add relay trust configuration for security`
- `feat(dhcp): add DHCPv6 relay (Relay-Forw/Relay-Rel) handling`

## Changelog

- 2026-08-16: initialized story file
