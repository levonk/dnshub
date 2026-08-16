---
story_id: "05-001"
story_title: "Full Prometheus metrics (all labels from PRD section 4.6)"
story_name: "full-prometheus-metrics"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 5
parallel_id: 1
branch: "feature/current/dnshub/story-05-001-full-prometheus-metrics"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-003", "02-003"]
parallel_safe: true
modules: ["metrics", "prometheus"]
priority: "MUST"
risk_level: "low"
tags: ["feat", "backend", "metrics", "prometheus", "observability"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement the full set of Prometheus metrics as defined in PRD section 4.6 (lines 1088-1133). Add upstream latency histograms, tier usage/failure counters, DNSSEC validation counters, blocklist daemon refresh/hot-swap metrics, cache hit ratio gauge, and blocklist entries per source gauge. All metrics use appropriate label cardinality (client_tag not raw IP, TLD-level domain labels).

## Current State

- **Relevant files and their roles:**
  - `src/metrics/counters.rs` — basic + per-client metrics (from stories 01-003, 02-003)
  - `src/metrics/recorder.rs` — Prometheus exporter (from story 01-003)
  - PRD lines 1088-1133 define all metrics to export
- **Existing code excerpts:**
  - `src/metrics/counters.rs` — record_query, record_cache_hit, record_cache_miss, record_blocklist_hit, record_policy_decision, record_error
- **Repository conventions:** Use metrics crate facade. Use histogram for latency. Use gauge for cache size and hit ratio. Label cardinality: client_tag (profile/hostname), TLD-level domain, tier number, category name, source name.
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
- src/metrics/counters.rs — add all missing metrics from PRD lines 1088-1133:
  - `dnshub_upstream_latency_seconds` (histogram, labels: tier, upstream) per PRD line 1107-1109
  - `dnshub_tier_queries_total` (counter, label: tier) per PRD line 1110
  - `dnshub_tier_failures_total` (counter, label: tier) per PRD line 1111
  - `dnshub_dnssec_validation_total` (counter, label: result: valid|bogus|indeterminate) per PRD line 1114
  - `dnshub_blocklist_refresh_total` (counter, labels: source, status: success|failure|stale) per PRD lines 1123-1124
  - `dnshub_blocklist_last_refresh_timestamp` (gauge, label: source) per PRD lines 1125-1126
  - `dnshub_blocklist_hot_swap_total` (counter, label: status: success|failure) per PRD line 1127
  - `dnshub_blocklist_entries_total` (gauge, label: source) per PRD line 1104
  - `dnshub_cache_hit_ratio` (gauge) per PRD line 1098
  - `dnshub_cache_size_entries` (gauge) — already exists, verify
- src/metrics/histograms.rs — histogram helper functions for upstream latency
- Integration: wire metric recording calls into DNS handler chain, blocklist daemon, and forwarding handlers
- Unit tests for all new metrics

**Out of scope:**
- Grafana dashboard (story 05-005)
- Jaeger traces (story 05-002)
- Query log (story 05-003)
- REST API (story 05-004)

## Sub-Tasks

- [ ] Create src/metrics/histograms.rs with record_upstream_latency(tier, upstream, duration_secs) using `histogram!("dnshub_upstream_latency_seconds", "tier" => tier, "upstream" => upstream)`
  **Verify**: `cargo test --lib metrics::histograms` → all pass (record latency, verify via recorder)
- [ ] Add tier metrics to src/metrics/counters.rs: record_tier_query(tier), record_tier_failure(tier) per PRD lines 1110-1111
  **Verify**: `cargo test --lib metrics::counters` → all pass
- [ ] Add DNSSEC metrics: record_dnssec_validation(result: valid|bogus|indeterminate) per PRD line 1114
  **Verify**: `cargo test --lib metrics::counters` → all pass
- [ ] Add blocklist daemon metrics: record_blocklist_refresh(source, status), set_blocklist_last_refresh(source, timestamp), record_blocklist_hot_swap(status), set_blocklist_entries(source, count) per PRD lines 1104, 1123-1127
  **Verify**: `cargo test --lib metrics::counters` → all pass
- [ ] Add cache hit ratio gauge: set_cache_hit_ratio(ratio) per PRD line 1098
  **Verify**: `cargo test --lib metrics::counters` → all pass
- [ ] Wire metric recording into handler chain: record upstream latency in TieredForwardHandler, record blocklist refresh in daemon, record cache hit ratio in caching handler
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/metrics/counters.rs` — add all missing metrics (modified)
- `src/metrics/histograms.rs` — upstream latency histogram (new)
- `src/dns/tiered_forward.rs` — wire latency recording (modified, if exists from 03-001)
- `src/blocklist/daemon.rs` — wire refresh metrics (modified)

## Acceptance Criteria

- [ ] All metrics from PRD lines 1088-1133 are registered and visible in Prometheus scrape
- [ ] Upstream latency histogram records per-tier and per-upstream
- [ ] Blocklist daemon metrics record refresh status and entry counts
- [ ] Cache hit ratio gauge is updated
- [ ] Label cardinality is bounded (no raw IP, no full domain names)
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib metrics` — tests for all new metrics
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: `curl http://127.0.0.1:9090/metrics | grep dnshub` — verify all metrics present

## Observability

- This story IS the observability enhancement
- All metrics are visible via Prometheus scrape endpoint

## Compliance

- Label cardinality is bounded to prevent Prometheus performance issues
- No personal data in metric labels (client_tag, not raw IP)

## Risks & Mitigations

- Risk: Label cardinality from many upstream names — Mitigation: Upstream names are bounded (5 tiers, configured names)
- Risk: Histogram bucket configuration — Mitigation: Use default buckets or configure for DNS latency range (1ms-10s)

## Dependencies & Sequencing

- Depends on: 01-003 (basic metrics), 02-003 (per-client metrics)
- Unblocks: 05-005 (Grafana dashboard queries these metrics)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- metrics crate histogram API does not work as expected
- Label cardinality cannot be bounded

## Maintenance Notes

- Reviewers should verify all PRD section 4.6 metrics are present
- Histogram buckets should be tuned for DNS latency range

## Commit Conventions

- `feat(metrics): add upstream latency histogram and tier metrics`
- `feat(metrics): add blocklist daemon and DNSSEC metrics`
- `feat(metrics): add cache hit ratio gauge`

## Changelog

- 2026-08-16: initialized story file
