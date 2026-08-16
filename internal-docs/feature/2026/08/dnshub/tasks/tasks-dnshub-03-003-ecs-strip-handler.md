---
story_id: "03-003"
story_title: "EcsStripHandler"
story_name: "ecs-strip-handler"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 3
parallel_id: 3
branch: "feature/current/dnshub/story-03-003-ecs-strip-handler"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["dns-server", "ecs"]
priority: "SHOULD"
risk_level: "low"
tags: ["feat", "backend", "dns", "ecs", "privacy"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement EcsStripHandler that removes EDNS Client Subnet (ECS) information from DNS queries before forwarding to upstream, and from responses before returning to clients. This prevents upstream resolvers from using the client's subnet for geo-targeting, preserving privacy. Configurable via [ecs] section (strip = true/false).

## Current State

- **Relevant files and their roles:**
  - `src/dns/mod.rs` — DnshubHandler chain (from story 01-001)
  - `src/config/ecs.rs` — EcsConfig with strip bool (from story 01-004)
- **Existing code excerpts:**
  - `src/config/ecs.rs` — EcsConfig { strip: bool }
- **Repository conventions:** Use hickory-proto for EDNS OPT record parsing. Handler chain insertion point is in src/dns/mod.rs.
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
- src/dns/ecs_strip.rs — EcsStripHandler implementing RequestHandler: extracts EDNS OPT record from query, removes ECS (EDNS Client Subnet) option if present, delegates to inner handler, removes ECS from response if present
- src/dns/mod.rs — insert EcsStripHandler into chain (after policy, before forwarding)
- Unit tests: query with ECS → stripped, query without ECS → unchanged, response with ECS → stripped

**Out of scope:**
- ECS preservation for specific upstreams (future — some upstreams may benefit from ECS)
- ECS-based policy decisions (not in PRD)

## Sub-Tasks

- [ ] Create src/dns/ecs_strip.rs with EcsStripHandler: parse EDNS OPT record from DNS message, identify ECS option (option code 8), remove it, delegate to inner handler, remove ECS from response
  **Verify**: `cargo build` → exit 0
- [ ] Implement ECS removal: use hickory-proto Edns struct, iterate options, filter out ecsdata option
  **Verify**: `cargo test --lib dns::ecs_strip` → all pass (query with ECS → stripped, without ECS → unchanged)
- [ ] Wire EcsStripHandler into handler chain in src/dns/mod.rs (conditional on config ecs.strip = true)
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dns/ecs_strip.rs` — EcsStripHandler (new)
- `src/dns/mod.rs` — wire EcsStripHandler into chain (modified)

## Acceptance Criteria

- [ ] ECS option is removed from outgoing queries when strip = true
- [ ] ECS option is removed from incoming responses when strip = true
- [ ] Queries without ECS are passed through unchanged
- [ ] Handler is conditional on config (ecs.strip = true)
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dns::ecs_strip` — tests for ECS removal in queries and responses
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log ECS strip events (count, not per-query to avoid hot-path overhead)

## Compliance

- ECS stripping enhances client privacy by preventing upstream geo-targeting
- No personal data stored

## Risks & Mitigations

- Risk: hickory-proto Edns API may differ in 0.26 — Mitigation: Check 0.26 API for OPT record manipulation
- Risk: Removing ECS may break upstreams that rely on it — Mitigation: Configurable via ecs.strip = false

## Dependencies & Sequencing

- Depends on: 01-001 (handler chain)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- hickory-proto does not expose EDNS OPT record manipulation API in 0.26

## Maintenance Notes

- Future: consider per-upstream ECS policy (strip for some, preserve for others)
- Reviewers should verify ECS option code 8 is correctly identified and removed

## Commit Conventions

- `feat(dns-server): add EcsStripHandler for EDNS Client Subnet removal`

## Changelog

- 2026-08-16: initialized story file
