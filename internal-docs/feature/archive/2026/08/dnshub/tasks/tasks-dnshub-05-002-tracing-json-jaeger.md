---
story_id: "05-002"
story_title: "tracing JSON logs to Loki + Jaeger traces"
story_name: "tracing-json-jaeger"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 5
parallel_id: 2
branch: "feature/current/dnshub/story-05-002-tracing-json-jaeger"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["observability", "tracing", "jaeger"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "observability", "tracing", "jaeger", "loki"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement structured JSON logging via tracing + tracing-subscriber (output to stdout for Promtail/Alloy to pick up and send to Loki) and Jaeger distributed traces via tracing-opentelemetry. Sample traces at 1-10% to minimize hot-path overhead. Correlate logs with metrics using timestamp and client_tag.

## Current State

- **Relevant files and their roles:**
  - `src/main.rs` — basic tracing subscriber init (from story 01-001)
  - `src/config/logging.rs` — LoggingConfig (level, format) from story 01-004
  - `src/config/tracing.rs` — TracingConfig (enabled, endpoint, sample_rate) from story 01-004
  - PRD lines 1139-1145 define logging and tracing scope
- **Existing code excerpts:**
  - `src/config/logging.rs` — LoggingConfig { level: String, format: String }
  - `src/config/tracing.rs` — TracingConfig { enabled: bool, endpoint: String, sample_rate: f64 }
- **Repository conventions:** Use tracing 0.1 + tracing-subscriber 0.3 with json feature. Use tracing-opentelemetry 0.33 + opentelemetry 0.27 for Jaeger. JSON logs to stdout.
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
- src/observability/mod.rs — module root with init_observability(config) function
- src/observability/logging.rs — initialize tracing-subscriber with JSON formatter to stdout, env-filter for log level, structured fields (timestamp, level, target, fields)
- src/observability/tracing.rs — initialize tracing-opentelemetry with Jaeger exporter, configurable sample rate (1-10%), span creation for DNS query processing
- src/observability/config.rs — verify LoggingConfig and TracingConfig structs
- Integration: add tracing spans to DNS handler chain (query processing, tier forwarding, blocklist lookup)
- Log format matches PRD example (lines 1253-1273): timestamp, level, target, fields with client_ip, client_name, profile, domain, qtype, response_code, blocked, etc.

**Out of scope:**
- Promtail/Alloy configuration (existing infrastructure)
- Grafana LogQL queries (story 05-005 Grafana dashboard)
- Full query log to SQLite (story 05-003)
- OpenTelemetry SDK metrics (using metrics crate instead per PRD section 4.6)

## Sub-Tasks

- [ ] Create src/observability/logging.rs with init_logging(config): create tracing-subscriber with fmt layer, JSON format, env-filter, install as global default
  **Verify**: `cargo test --lib observability::logging` → all pass (verify JSON output format)
- [ ] Create src/observability/tracing.rs with init_tracing(config): create tracing-opentelemetry layer with Jaeger exporter, configure sample rate, install alongside logging layer
  **Verify**: `cargo build` → exit 0 (Jaeger exporter initialization)
- [ ] Create src/observability/mod.rs with init_observability(config) that calls both init_logging and init_tracing (if enabled)
  **Verify**: `cargo build` → exit 0
- [ ] Add tracing spans to DNS handler chain: #[instrument] on handler methods, span for query processing with client_ip, domain, qtype fields
  **Verify**: `cargo build` → exit 0
- [ ] Add structured log fields per PRD lines 1253-1273: client_ip, client_name, profile, domain, qtype, response_code, blocked, block_category, block_source, tier, latency_ms, cached
  **Verify**: `cargo test --lib observability` → all pass (verify log fields present in JSON output)
- [ ] Update src/main.rs to call init_observability(config) at startup
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/observability/mod.rs` — module root (new)
- `src/observability/logging.rs` — JSON logging init (new)
- `src/observability/tracing.rs` — Jaeger tracing init (new)
- `src/main.rs` — call init_observability (modified)

## Acceptance Criteria

- [ ] JSON logs are output to stdout in the format matching PRD lines 1253-1273
- [ ] Log level is configurable via config (info, warn, error)
- [ ] Jaeger traces are exported when tracing.enabled = true
- [ ] Trace sampling rate is configurable (sample_rate)
- [ ] Tracing spans cover DNS query processing, tier forwarding, blocklist lookup
- [ ] Structured log fields include client_ip, domain, qtype, response_code, blocked, profile
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib observability` — tests for logging init and JSON format
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start server, send queries, verify JSON logs on stdout, verify spans in Jaeger (if enabled)

## Observability

- This story IS the observability infrastructure for logs and traces
- JSON logs → Promtail/Alloy → Loki
- Traces → Jaeger

## Compliance

- Logs may contain client IPs and queried domains — treat as operational data
- Log retention is managed by Loki (not dnshub)
- Trace sampling limits performance impact and data volume

## Risks & Mitigations

- Risk: tracing-opentelemetry 0.33 API may differ from docs — Mitigation: Check crate docs, pin version
- Risk: JSON logging adds overhead on hot path — Mitigation: tracing is async and non-blocking; use env-filter to reduce log volume
- Risk: Jaeger exporter connection failure — Mitigation: Graceful degradation, log warning, continue without traces

## Dependencies & Sequencing

- Depends on: 01-001 (main.rs with basic tracing init)
- Unblocks: 05-005 (Grafana dashboard correlates with Loki logs)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- tracing-opentelemetry 0.33 cannot connect to Jaeger exporter
- JSON log format does not match PRD example

## Maintenance Notes

- Trace sampling rate should be tuned based on query volume (1% for high volume, 10% for low)
- Reviewers should verify JSON log fields match PRD lines 1253-1273

## Commit Conventions

- `feat(observability): add JSON logging via tracing-subscriber`
- `feat(observability): add Jaeger traces via tracing-opentelemetry`
- `feat(observability): add tracing spans to DNS handler chain`

## Changelog

- 2026-08-16: initialized story file
