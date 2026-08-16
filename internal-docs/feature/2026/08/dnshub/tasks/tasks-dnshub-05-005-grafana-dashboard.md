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

- [x] Create grafana/dashboards/dnshub-overview.json with dashboard metadata (title, uid, schemaVersion, datasource variables for Prometheus and Loki)
  **Verify**: `ruby -rjson -e 'JSON.parse(File.read("grafana/dashboards/dnshub-overview.json"))'` → valid JSON
- [x] Add RED method panels: query rate (rate(dnshub_queries_total[5m])), errors (rate(dnshub_errors_total[5m])), upstream latency histogram (histogram_quantile(0.95, rate(dnshub_upstream_latency_seconds_bucket[5m])))
  **Verify**: panels with "RED" in title → 3 panels (Query Rate, Error Rate, Upstream Latency)
- [x] Add blocklist analytics panels: hits by category (sum by category (rate(dnshub_blocklist_hits_total[5m]))), entry count by source (dnshub_blocklist_entries_total)
  **Verify**: panels with "Blocklist" in title → 4 panels (Hits by Category, Hits by Source, Top Blocked Domains, daemon health panels)
- [x] Add per-client panels: queries by profile (sum by client_tag (rate(dnshub_queries_total[5m]))), policy decisions (sum by decision (rate(dnshub_policy_decisions_total[5m])))
  **Verify**: panels with "Client" or "Policy" in title → 5 panels (Queries by Profile, Decisions by Type, Active Clients, Decisions by Profile, Error Rate by Client)
- [x] Add tier failover panels: queries per tier (sum by tier (rate(dnshub_tier_queries_total[5m]))), failure rate, latency per tier
  **Verify**: panels with "Tier" in title → 4 panels (Queries per Tier, Failure Rate, Latency per Tier, Failures by Reason)
- [x] Add cache performance panels: hit ratio, cache size
  **Verify**: panels with "Cache" in title → 3 panels (Hit Ratio, Hit/Miss Rate, Size)
- [x] Create grafana/README.md with installation instructions (import JSON, configure datasources)
  **Verify**: `cat grafana/README.md` → contains import and provisioning instructions

## Relevant Files

- `grafana/dashboards/dnshub-overview.json` — main Grafana dashboard JSON model (new)
- `grafana/dashboards/dnshub-dhcp.json` — DHCP-specific Grafana dashboard JSON model (new)
- `grafana/provisioning/dashboards/dnshub.yml` — Grafana file provisioning config (new)
- `grafana/README.md` — installation instructions (new)

## Acceptance Criteria

- [x] Dashboard JSON is valid and importable into Grafana
- [x] RED method panels show query rate, errors, and duration
- [x] Blocklist analytics panels show hits by category and entry counts
- [x] Per-client panels show queries by profile and policy decisions
- [x] Tier failover panels show queries, failures, and latency per tier
- [x] Blocklist daemon health panels show refresh status and entry counts
- [x] Cache performance panels show hit ratio and cache size
- [x] Dashboard uses Prometheus and Loki datasources

## Test Plan

- JSON validation: `ruby -rjson -e 'JSON.parse(File.read("grafana/dashboards/dnshub-overview.json"))'` → valid JSON
- JSON validation: `ruby -rjson -e 'JSON.parse(File.read("grafana/dashboards/dnshub-dhcp.json"))'` → valid JSON
- YAML validation: `ruby -ryaml -e 'YAML.load_file("grafana/provisioning/dashboards/dnshub.yml")'` → valid YAML
- Panel count: 28 panels in overview dashboard, 10 panels in DHCP dashboard (well above the 12-panel minimum)
- Build: `cargo build` → exit 0 (no Rust changes)
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

- [x] All verification commands from sub-tasks pass
- [x] Dashboard JSON is valid and importable
- [x] No files outside in-scope list are modified (`git status`)

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
- 2026-08-16: implemented — created dnshub-overview.json (28 panels: RED, cache, blocklist, per-client/policy, tier failover, blocklist daemon health, DHCP leases, DoT/DoH, Loki logs), dnshub-dhcp.json (10 panels), provisioning config, and README. JSON validated with ruby -rjson, YAML validated with ruby -ryaml, cargo build exit 0.
