---
story_id: "05-005"
story_title: "Grafana dashboard (metrics + Loki logs)"
story_name: "grafana-dashboard"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 5
parallel_id: 5
branch: "feature/current/dnshub/story-05-005-grafana-dashboard"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-003"]
parallel_safe: true
modules: ["observability", "grafana"]
priority: "SHOULD"
risk_level: "low"
tags: ["feat", "observability", "grafana", "dashboard"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Create a Grafana dashboard JSON model for dnshub with panels covering: RED method (Rate, Errors, Duration) for DNS queries, blocklist analytics (hits by category, top blocked domains), per-client panel (queries by profile, policy decisions), tier failover panel (queries per tier, failure rate, latency per tier), and blocklist daemon health (last refresh per source, entry count per source).

## Current State

- **Relevant files and their roles:**
  - `src/metrics/counters.rs` — Prometheus metrics (from stories 01-003, 02-003, 05-001)
  - PRD lines 1147-1152 define dashboard panels
- **Existing code excerpts:** None — this story creates a JSON file only.
- **Repository conventions:** Grafana dashboard JSON model. Prometheus data source for metrics. Loki data source for logs.
- **Tech context (binding constraint from tech-context.txt):**
  - Config format: TOML (dnshub config), JSON (frontend package.json)
  - Never use: npm, npx, yarn, jest, biome
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Build   | `cargo build` | exit 0 (no Rust changes) |
  | JSON    | `jq . grafana/dnshub-dashboard.json` | valid JSON |

## Scope

**In scope:**
- grafana/dnshub-dashboard.json — Grafana dashboard JSON model with panels:
  - RED method panel: query rate (dnshub_queries_total), errors (dnshub_errors_total), duration (dnshub_upstream_latency_seconds)
  - Blocklist analytics: hits by category over time (dnshub_blocklist_hits_total by category), top blocked domains (Loki LogQL)
  - Per-client panel: queries by profile (dnshub_queries_total by client_tag), policy decisions (dnshub_policy_decisions_total by decision)
  - Tier failover panel: queries per tier (dnshub_tier_queries_total), failure rate (dnshub_tier_failures_total / dnshub_tier_queries_total), latency per tier (dnshub_upstream_latency_seconds by tier)
  - Blocklist daemon health: last refresh per source (dnshub_blocklist_last_refresh_timestamp), entry count per source (dnshub_blocklist_entries_total), refresh status (dnshub_blocklist_refresh_total by status)
  - Cache performance: hit/miss ratio (dnshub_cache_hits_total / (dnshub_cache_hits_total + dnshub_cache_misses_total)), cache size (dnshub_cache_size_entries)
- grafana/README.md — dashboard installation instructions

**Out of scope:**
- Grafana provisioning configuration (existing Grafana infrastructure)
- Alert rules (future)
- Frontend analytics (use Grafana, not NextJS per PRD section 4.9)

## Sub-Tasks

- [ ] Create grafana/dnshub-dashboard.json with dashboard metadata (title, uid, schemaVersion, datasource variables for Prometheus and Loki)
  **Verify**: `jq . grafana/dnshub-dashboard.json > /dev/null` → valid JSON
- [ ] Add RED method panels: query rate (rate(dnshub_queries_total[5m])), errors (rate(dnshub_errors_total[5m])), upstream latency histogram (histogram_quantile(0.95, rate(dnshub_upstream_latency_seconds_bucket[5m])))
  **Verify**: `jq '.panels[] | select(.title | contains("RED"))' grafana/dnshub-dashboard.json` → at least 3 panels
- [ ] Add blocklist analytics panels: hits by category (sum by category (rate(dnshub_blocklist_hits_total[5m]))), entry count by source (dnshub_blocklist_entries_total)
  **Verify**: `jq '.panels[] | select(.title | contains("Blocklist"))' grafana/dnshub-dashboard.json` → at least 2 panels
- [ ] Add per-client panels: queries by profile (sum by client_tag (rate(dnshub_queries_total[5m]))), policy decisions (sum by decision (rate(dnshub_policy_decisions_total[5m])))
  **Verify**: `jq '.panels[] | select(.title | contains("Client") or contains("Policy"))' grafana/dnshub-dashboard.json` → at least 2 panels
- [ ] Add tier failover panels: queries per tier (sum by tier (rate(dnshub_tier_queries_total[5m]))), failure rate, latency per tier
  **Verify**: `jq '.panels[] | select(.title | contains("Tier"))' grafana/dnshub-dashboard.json` → at least 3 panels
- [ ] Add cache performance panels: hit ratio, cache size
  **Verify**: `jq '.panels[] | select(.title | contains("Cache"))' grafana/dnshub-dashboard.json` → at least 2 panels
- [ ] Create grafana/README.md with installation instructions (import JSON, configure datasources)
  **Verify**: `cat grafana/README.md` → contains import instructions

## Relevant Files

- `grafana/dnshub-dashboard.json` — Grafana dashboard JSON model (new)
- `grafana/README.md` — installation instructions (new)

## Acceptance Criteria

- [ ] Dashboard JSON is valid and importable into Grafana
- [ ] RED method panels show query rate, errors, and duration
- [ ] Blocklist analytics panels show hits by category and entry counts
- [ ] Per-client panels show queries by profile and policy decisions
- [ ] Tier failover panels show queries, failures, and latency per tier
- [ ] Blocklist daemon health panels show refresh status and entry counts
- [ ] Cache performance panels show hit ratio and cache size
- [ ] Dashboard uses Prometheus and Loki datasources

## Test Plan

- JSON validation: `jq . grafana/dnshub-dashboard.json` → valid JSON
- Panel count: verify at least 12 panels across all categories
- Manual: Import dashboard into Grafana, verify panels render with test data

## Observability

- This dashboard IS the observability visualization for operations
- Correlates Prometheus metrics with Loki logs

## Compliance

- Dashboard shows aggregated metrics, not individual client data
- No personal data in dashboard panels

## Risks & Mitigations

- Risk: Grafana schema version compatibility — Mitigation: Use schemaVersion compatible with Grafana 10+
- Risk: Prometheus query syntax errors — Mitigation: Test queries in Grafana explore before adding to dashboard

## Dependencies & Sequencing

- Depends on: 01-003 (basic metrics must exist in Prometheus)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Dashboard JSON is valid and importable
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- Grafana JSON format is not importable
- Prometheus queries do not match available metrics

## Maintenance Notes

- Dashboard should be updated when new metrics are added
- Reviewers should verify Prometheus queries match metric names from stories 01-003, 02-003, 05-001

## Commit Conventions

- `feat(grafana): add dnshub dashboard with RED, blocklist, tier, and cache panels`

## Changelog

- 2026-08-16: initialized story file
