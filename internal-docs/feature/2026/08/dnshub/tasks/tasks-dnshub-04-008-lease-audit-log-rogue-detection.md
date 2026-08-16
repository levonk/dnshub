---
story_id: "04-008"
story_title: "Lease audit log + rogue DHCP detection"
story_name: "lease-audit-log-rogue-detection"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 8
branch: "feature/current/dnshub/story-04-008-lease-audit-log-rogue-detection"
status: "done"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "audit", "rogue-detection"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "dhcp", "audit", "rogue-detection", "security"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement DHCP lease audit log (track device join/leave events: DHCPACK, RELEASE, DECLINE, expiry with timestamp, MAC, IP, hostname, stored in SQLite, viewable in frontend) and rogue DHCP server detection (periodically send DHCPDISCOVER probes, if a response comes from a non-dnshub IP, raise an alert via metric and log).

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig (from story 01-004), to be expanded with audit and rogue detection config
  - PRD lines 430-435 define lease audit log, lines 434-435 define rogue DHCP detection
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub
- **Repository conventions:** Use rusqlite for audit log persistence. Use dhcproto for DHCPDISCOVER probe construction. Use tokio for periodic probe task.
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
- src/dhcp/audit/mod.rs — AuditLogger: write_audit_event(event), list_audit_events(filter), backed by SQLite dhcp_audit_log table
- src/dhcp/audit/events.rs — DhcpAuditEvent: event_type (ack, release, decline, expire, blocked), timestamp, mac_address, duid, ip_address, hostname, profile, details per PRD lines 740-754
- src/dhcp/audit/config.rs — AuditConfig: enabled (bool)
- src/dhcp/rogue/mod.rs — RogueDetector: periodically send DHCPDISCOVER probes, check if response comes from non-dnshub IP, raise alert
- src/dhcp/rogue/probe.rs — DHCPDISCOVER probe: construct and send DHCPDISCOVER, listen for DHCPOFFER responses, record responder IP
- src/dhcp/rogue/config.rs — RogueConfig: enabled, probe_interval_secs (default 300), alert_metric, alert_log per PRD lines 772-777
- SQLite schema: dhcp_audit_log table per PRD lines 740-754
- src/config/dhcp.rs — expand with audit and rogue detection config
- Unit tests for audit log CRUD, rogue detection probe logic

**Out of scope:**
- DHCPv4/v6 server core (stories 04-001, 04-002 — this story provides modules that the DHCP servers call)
- REST API for audit log viewing (story 05-004)
- Frontend audit log viewer (story 05-006)
- Automated rogue DHCP blocking (alert only, no automatic blocking)

## Sub-Tasks

- [x] Create src/dhcp/audit/events.rs with DhcpAuditEvent: event_type (enum: Ack, Release, Decline, Expire, Blocked), timestamp (i64 unix millis), mac_address (Option<String>), duid (Option<String>), ip_address (Option<String>), hostname (Option<String>), profile (Option<String>), details (Option<String>) per PRD lines 740-754
  **Verify**: `cargo build` → exit 0
- [x] Create src/dhcp/audit/mod.rs with AuditLogger: write_audit_event(event) -> Result<()>, list_audit_events(filter) -> Vec<DhcpAuditEvent>, backed by SQLite. Create dhcp_audit_log table with indexes per PRD lines 740-754.
  **Verify**: `cargo test --lib dhcp::audit` → all pass (write event, list events, filter by MAC, filter by event type)
- [x] Create src/dhcp/rogue/probe.rs with DHCPDISCOVER probe: construct DHCPDISCOVER message via dhcproto, send via UDP broadcast, listen for DHCPOFFER responses, record responder IP and offered IP
  **Verify**: `cargo test --lib dhcp::rogue::probe` → all pass (construct probe, parse response)
- [x] Create src/dhcp/rogue/mod.rs with RogueDetector: periodic probe task (every probe_interval_secs), compare responder IP to dnshub's own IP, if different → raise alert (log + metric)
  **Verify**: `cargo test --lib dhcp::rogue` → all pass (detect non-dnshub response, no false positive from own IP)
- [x] Create src/dhcp/rogue/config.rs with RogueConfig: enabled, probe_interval_secs (u64, default 300), alert_metric (String), alert_log (bool) per PRD lines 772-777
  **Verify**: `cargo build` → exit 0
- [x] Update src/config/dhcp.rs with [dhcp.rogue_detection] section per PRD lines 772-777
  **Verify**: `cargo build` → exit 0
- [x] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/audit/mod.rs` — AuditLogger (new)
- `src/dhcp/audit/events.rs` — DhcpAuditEvent (new)
- `src/dhcp/audit/config.rs` — AuditConfig (new)
- `src/dhcp/rogue/mod.rs` — RogueDetector (new)
- `src/dhcp/rogue/probe.rs` — DHCPDISCOVER probe (new)
- `src/dhcp/rogue/config.rs` — RogueConfig (new)
- `src/config/dhcp.rs` — expand with audit and rogue config (modified)

## Acceptance Criteria

- [x] Audit log records DHCPACK, RELEASE, DECLINE, expiry events
- [x] Audit log stores timestamp, MAC, IP, hostname, profile, details
- [x] Audit log is queryable by MAC, event type, time range
- [x] Rogue DHCP detection sends periodic DHCPDISCOVER probes
- [x] Rogue detection alerts when response comes from non-dnshub IP
- [x] Rogue detection does not alert on dnshub's own responses
- [x] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::audit` and `cargo test --lib dhcp::rogue` — tests for audit log and rogue detection
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Audit log IS observability (device join/leave events)
- Rogue DHCP detection raises metric (dnshub_rogue_dhcp_detected) and log alert
- Both are viewable in frontend (story 05-006)

## Compliance

- Audit log contains MAC addresses and hostnames — security visibility data
- Retain audit log per organizational policy (no automatic deletion in v1)

## Risks & Mitigations

- Risk: Rogue DHCP probe may interfere with real DHCP clients — Mitigation: Use specific transaction ID, ignore responses not matching, low probe frequency (default 5 min)
- Risk: Audit log grows unbounded — Mitigation: Add retention policy in future; for v1, manual cleanup
- Risk: DHCPDISCOVER broadcast may not work in all Docker network modes — Mitigation: Requires macvlan (story 04-012)

## Dependencies & Sequencing

- Depends on: 01-004 (config loading)
- Unblocks: None directly

## Definition of Done

- [x] All verification commands from sub-tasks pass
- [x] Code, tests, docs updated; CI green
- [x] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- DHCPDISCOVER probe cannot be sent via UDP broadcast in test environment
- Audit log SQLite operations have concurrency issues

## Maintenance Notes

- Audit log should be queryable by time range, MAC, and event type for frontend
- Rogue detection should be disableable via config for environments where probing is not desired
- Reviewers should verify rogue detection does not produce false positives

## Commit Conventions

- `feat(dhcp): add lease audit log with SQLite persistence`
- `feat(dhcp): add rogue DHCP server detection with periodic probes`

## Changelog

- 2026-08-16: initialized story file
- 2026-08-16: implemented lease audit log (SQLite `dhcp_audit_log` table with
  Ack/Renew/Release/Decline/Expire/Conflict events, filtered queries by MAC,
  event type, time range, limit) and rogue DHCP server detection
  (DHCPDISCOVER probe via dhcproto, periodic tokio task, allowlist of dnshub's
  own IPs, Prometheus counter + structured log + audit `Conflict` event on
  detection). Added `AuditConfig` and `RogueConfig` to `DhcpConfig`. 25 new
  unit tests, all 273 lib tests + integration tests pass.
