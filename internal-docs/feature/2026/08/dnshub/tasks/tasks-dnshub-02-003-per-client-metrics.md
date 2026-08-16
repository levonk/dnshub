---
story_id: "02-003"
story_title: "Per-client metrics"
story_name: "per-client-metrics"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 2
parallel_id: 3
branch: "feature/current/dnshub/story-02-003-per-client-metrics"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-003"]
parallel_safe: true
modules: ["metrics"]
priority: "SHOULD"
risk_level: "low"
tags: ["feat", "backend", "metrics", "per-client"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Enhance the basic metrics module to add per-client policy decision metrics. Add `dnshub_policy_decisions_total` counter with client, profile, and decision labels per PRD section 4.6. Add per-client query rate metrics using client_tag (profile name or hostname, not raw IP) to avoid unbounded cardinality.

## Current State

- **Relevant files and their roles:**
  - `src/metrics/counters.rs` — basic metric helpers (from story 01-003), per-client metrics to be added here
  - `src/metrics/recorder.rs` — Prometheus exporter (from story 01-003)
- **Existing code excerpts:**
  - `src/metrics/counters.rs` — record_query(client_tag, qtype), record_cache_hit(), record_cache_miss(), etc.
- **Repository conventions:** Use client_tag (profile name or hostname) not raw IP for labels. Use TLD-level domain labels for query rate. Follow metrics crate facade pattern.
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
- src/metrics/counters.rs — add per-client metric helpers:
  - `dnshub_policy_decisions_total` (counter, labels: client, profile, decision: allowed|blocked|redirected) per PRD line 1117-1120
  - `dnshub_queries_total` enhanced with client_tag label (already exists, add client dimension)
- src/metrics/labels.rs — label normalization helpers: client_to_tag(ip, hostname, profile) -> String (returns profile name or hostname, never raw IP)
- Unit tests for per-client metric recording and label normalization

**Out of scope:**
- Full metrics with all labels (story 05-001)
- Upstream latency histograms (story 05-001)
- Blocklist daemon metrics (story 05-001)
- Grafana dashboard (story 05-005)

## Sub-Tasks

- [ ] Create src/metrics/labels.rs with client_to_tag(ip, hostname, profile) -> String: returns profile name if available, else hostname, else "unknown". Never returns raw IP.
  **Verify**: `cargo test --lib metrics::labels` → all pass (test profile, hostname, unknown cases)
- [ ] Add record_policy_decision(client, profile, decision) to src/metrics/counters.rs using `counter!("dnshub_policy_decisions_total", "client" => client, "profile" => profile, "decision" => decision)`
  **Verify**: `cargo test --lib metrics::counters` → all pass (increment and verify)
- [ ] Update record_query() to accept and use client_tag label
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/metrics/counters.rs` — add per-client metric helpers (modified)
- `src/metrics/labels.rs` — label normalization (new)

## Acceptance Criteria

- [ ] `dnshub_policy_decisions_total` counter records with client, profile, decision labels
- [ ] client_to_tag returns profile name or hostname, never raw IP
- [ ] Query rate metric includes client_tag dimension
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib metrics` — tests for label normalization and per-client counters
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- This story enhances observability with per-client dimensions
- Metrics are visible via Prometheus scrape endpoint

## Compliance

- Client_tag (profile/hostname) is used instead of raw IP to prevent cardinality explosion and protect privacy
- No personal data in metric labels

## Risks & Mitigations

- Risk: Label cardinality from many hostnames — Mitigation: Use profile name primarily, hostname as fallback; profiles are bounded (parents, kids, iot, guest, default)

## Dependencies & Sequencing

- Depends on: 01-003 (basic metrics module)
- Unblocks: 05-001 (full metrics builds on this)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- metrics crate does not support multi-label counters as expected

## Maintenance Notes

- The label normalization logic should be reused by all metric recording sites
- Reviewers should verify no raw IP addresses appear in metric labels

## Commit Conventions

- `feat(metrics): add per-client policy decision metrics`
- `feat(metrics): add client tag label normalization`

## Changelog

- 2026-08-16: initialized story file
