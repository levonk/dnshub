---
story_id: "05-003"
story_title: "Query log SQLite ring buffer + QueryLogHandler"
story_name: "query-log-sqlite-ring-buffer"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 5
parallel_id: 3
branch: "feature/current/dnshub/story-05-003-query-log-sqlite-ring-buffer"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["query-log", "sqlite"]
priority: "MUST"
risk_level: "medium"
tags: ["feat", "backend", "query-log", "sqlite", "ring-buffer"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement the query log SQLite ring buffer and QueryLogHandler. Every DNS query is logged to an embedded SQLite database with fields: timestamp, client_ip, client_name, profile, domain, qtype, response_code, blocked, block_category, block_source, tier, latency_ms, cached. The ring buffer retains the last N queries (default 100,000) with batched writes (every 100 queries or 1 second). The QueryLogHandler is inserted into the handler chain to record queries as they pass through.

## Current State

- **Relevant files and their roles:**
  - `src/dns/mod.rs` — DnshubHandler chain (from story 01-001)
  - `src/config/query_log.rs` — QueryLogConfig (enabled, max_entries, retention_days) from story 01-004
  - PRD section 4.8 (lines 1197-1277) defines query log scope
- **Existing code excerpts:**
  - `src/config/query_log.rs` — QueryLogConfig { enabled: bool, max_entries: u64, retention_days: u64 }
- **Repository conventions:** Use rusqlite with bundled feature. Batched writes (every 100 queries or 1 second). Ring buffer: delete oldest entries in batches of 1,000 when full. SQLite WAL mode for concurrent reads.
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
- src/query_log/mod.rs — QueryLogger: record_query(entry), batched writes (every 100 or 1s), ring buffer cleanup
- src/query_log/store.rs — SqliteQueryLogStore: create query_log table per PRD lines 1213-1233, insert entries, query with filters, delete oldest in batches
- src/query_log/handler.rs — QueryLogHandler: implements RequestHandler, records query after processing (domain, qtype, response_code, blocked, latency, tier, cached), delegates to inner handler
- src/query_log/config.rs — verify QueryLogConfig (enabled, max_entries, retention_days)
- src/query_log/ring_buffer.rs — Ring buffer logic: count entries, delete oldest batch (1,000) when max_entries exceeded, time-based cleanup (retention_days)
- SQLite schema: query_log table with indexes per PRD lines 1213-1233
- Integration: insert QueryLogHandler into handler chain (after response, before returning to client)
- Unit tests for insert, query, ring buffer cleanup, batched writes

**Out of scope:**
- REST API for query log (story 05-004)
- Frontend query log viewer (story 05-006)
- Loki log format (story 05-002 — tracing handles Loki output)
- CSV export (story 05-004)

## Sub-Tasks

- [ ] Create src/query_log/store.rs with SqliteQueryLogStore: create query_log table per PRD lines 1213-1233 (id, timestamp, client_ip, client_name, profile, domain, qtype, response_code, blocked, block_category, block_source, tier, latency_ms, cached), create indexes (timestamp, client_ip, domain)
  **Verify**: `cargo test --lib query_log::store` → all pass (table created, indexes present)
- [ ] Implement insert_query(entry) with batched writes: buffer 100 entries or flush after 1 second, use WAL mode for concurrent reads
  **Verify**: `cargo test --lib query_log::store` → all pass (insert 100 entries, verify batch flush)
- [ ] Create src/query_log/ring_buffer.rs with ring buffer cleanup: count entries, if > max_entries delete oldest 1,000 in one DELETE, also time-based cleanup (delete entries older than retention_days)
  **Verify**: `cargo test --lib query_log::ring_buffer` → all pass (insert 100 entries with max=50, verify oldest deleted)
- [ ] Create src/query_log/handler.rs with QueryLogHandler: implements RequestHandler, records query after inner handler returns (extract domain, qtype, response_code, latency, blocked status from response), delegates to inner handler
  **Verify**: `cargo test --lib query_log::handler` → all pass (process query, verify entry recorded)
- [ ] Create src/query_log/mod.rs with QueryLogger: holds SqliteQueryLogStore, batched write task, ring buffer cleanup task
  **Verify**: `cargo build` → exit 0
- [ ] Wire QueryLogHandler into handler chain in src/dns/mod.rs (after all processing, before returning response)
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/query_log/mod.rs` — QueryLogger (new)
- `src/query_log/store.rs` — SqliteQueryLogStore (new)
- `src/query_log/handler.rs` — QueryLogHandler (new)
- `src/query_log/ring_buffer.rs` — ring buffer cleanup (new)
- `src/query_log/config.rs` — verify QueryLogConfig (new)
- `src/dns/mod.rs` — wire QueryLogHandler into chain (modified)

## Acceptance Criteria

- [ ] Every DNS query is logged to SQLite with all fields per PRD lines 1213-1228
- [ ] Batched writes (every 100 queries or 1 second) minimize I/O
- [ ] Ring buffer retains last max_entries queries (default 100,000)
- [ ] Oldest entries deleted in batches of 1,000 when full
- [ ] Time-based cleanup (retention_days) also works
- [ ] SQLite WAL mode allows concurrent reads during writes
- [ ] QueryLogHandler records queries without adding significant latency
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib query_log` — tests for store, ring buffer, handler
- Integration: `cargo test --test integration_test` — verify queries are logged end-to-end
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Query log IS observability data (SQLite for live viewing, Loki for long-term)
- Batched writes minimize hot-path I/O impact

## Compliance

- Query log contains client IPs and queried domains — operational data
- Retention is bounded (max_entries + retention_days, whichever triggers first)
- Access to query log is via authenticated REST API (story 05-004)

## Risks & Mitigations

- Risk: SQLite write contention on hot path — Mitigation: Batched writes (100/1s), WAL mode, single writer task
- Risk: Ring buffer cleanup blocks writes — Mitigation: Cleanup runs in background task, not in write path
- Risk: Query log grows too fast — Mitigation: Configurable max_entries and retention_days, batch deletion

## Dependencies & Sequencing

- Depends on: 01-001 (handler chain)
- Unblocks: 05-004 (REST API reads from this SQLite table), 05-006 (frontend views query log)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- SQLite WAL mode has issues with concurrent access in test
- Batched writes cause data loss on shutdown (flush on shutdown needed)

## Maintenance Notes

- Add flush-on-shutdown to ensure buffered entries are written before exit
- Reviewers should verify ring buffer cleanup doesn't block writes
- SQLite database is shared with DHCP lease tables (different tables, same file)

## Commit Conventions

- `feat(query-log): add SQLite query log store with ring buffer`
- `feat(query-log): add QueryLogHandler to DNS handler chain`
- `feat(query-log): add batched writes and ring buffer cleanup`

## Changelog

- 2026-08-16: initialized story file
