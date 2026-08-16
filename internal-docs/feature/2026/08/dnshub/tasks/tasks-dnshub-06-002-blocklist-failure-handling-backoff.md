---
story_id: "06-002"
story_title: "Blocklist source failure handling (backoff, circuit breaker)"
story_name: "blocklist-failure-handling-backoff"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 6
parallel_id: 2
branch: "feature/current/dnshub/story-06-002-blocklist-failure-handling-backoff"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-002"]
parallel_safe: true
modules: ["blocklist", "reliability"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "blocklist", "reliability", "backoff", "circuit-breaker"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement robust blocklist source failure handling: exponential backoff on fetch failures (1min → 5min → 15min → 1hr → 6hr), circuit breaker after N consecutive failures (stop fetching, serve stale), and graceful degradation on boot (load cached LMDB from disk, serve immediately, refresh in background; block until critical lists loaded if no cache).

## Current State

- **Relevant files and their roles:**
  - `src/blocklist/daemon.rs` — BlocklistDaemon with basic fetch and refresh (from story 01-002)
  - PRD lines 359-367 define failure handling behavior
- **Existing code excerpts:**
  - `src/blocklist/daemon.rs` — per-source refresh loop, HTTP fetch, schema validation
- **Repository conventions:** Exponential backoff sequence: 1min → 5min → 15min → 1hr → 6hr. Circuit breaker: after N consecutive failures, stop fetching and serve stale. Critical lists: malware/phishing must be loaded before serving.
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
- src/blocklist/backoff.rs — ExponentialBackoff: calculate next retry delay (1min → 5min → 15min → 1hr → 6hr), reset on success
- src/blocklist/circuit_breaker.rs — CircuitBreaker: track consecutive failures, open circuit after N failures (configurable, default 10), stop fetching, serve stale, half-open state for retry
- src/blocklist/daemon.rs — update with backoff and circuit breaker integration, boot behavior (load cached LMDB, block until critical lists loaded)
- src/blocklist/boot.rs — Boot behavior: load cached LMDB from disk, serve immediately, refresh in background; if no cache, block until critical lists (malware/phishing) loaded, serve non-critical as they arrive
- src/config/blocklists.rs — add failure handling config: max_consecutive_failures (default 10), critical_sources (list of source names that must be loaded before serving)
- Unit tests for backoff, circuit breaker, boot behavior

**Out of scope:**
- Blocklist source health REST API (story 05-004)
- Blocklist source metrics (story 05-001)
- Alternative source mirrors (future)

## Sub-Tasks

- [ ] Create src/blocklist/backoff.rs with ExponentialBackoff: new() with sequence [60, 300, 900, 3600, 21600] seconds, next_delay() -> Duration, reset() on success, current_attempt() -> u32
  **Verify**: `cargo test --lib blocklist::backoff` → all pass (verify delay sequence, reset on success)
- [ ] Create src/blocklist/circuit_breaker.rs with CircuitBreaker: record_failure(), record_success(), is_open() -> bool, state (Closed, Open, HalfOpen), open after max_consecutive_failures, half-open after cooldown
  **Verify**: `cargo test --lib blocklist::circuit_breaker` → all pass (fail N times → open, success → closed, half-open retry)
- [ ] Update src/blocklist/daemon.rs to use backoff on fetch failure and circuit breaker to stop fetching after N failures
  **Verify**: `cargo test --lib blocklist::daemon` → all pass (fetch failure → backoff, N failures → circuit open, serve stale)
- [ ] Create src/blocklist/boot.rs with boot behavior: check for cached LMDB on disk, if exists load and serve immediately, refresh in background; if no cache, block until critical sources loaded
  **Verify**: `cargo test --lib blocklist::boot` → all pass (cache exists → serve immediately, no cache → block until critical)
- [ ] Update src/config/blocklists.rs with failure handling config: max_consecutive_failures (default 10), critical_sources (Vec<String>, default ["hagezi-tif", "urlhaus"])
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/blocklist/backoff.rs` — ExponentialBackoff (new)
- `src/blocklist/circuit_breaker.rs` — CircuitBreaker (new)
- `src/blocklist/daemon.rs` — integrate backoff and circuit breaker (modified)
- `src/blocklist/boot.rs` — boot behavior (new)
- `src/config/blocklists.rs` — failure handling config (modified)

## Acceptance Criteria

- [ ] Exponential backoff sequence: 1min → 5min → 15min → 1hr → 6hr on fetch failures
- [ ] Backoff resets on successful fetch
- [ ] Circuit breaker opens after N consecutive failures (default 10)
- [ ] Circuit breaker half-open state allows retry after cooldown
- [ ] On boot with cached LMDB: serve immediately, refresh in background
- [ ] On boot without cache: block until critical sources (malware/phishing) loaded
- [ ] Non-critical sources are served as they arrive (don't block boot)
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib blocklist` — tests for backoff, circuit breaker, boot behavior
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log backoff events (source, attempt, next delay)
- Log circuit breaker state changes (closed → open, open → half-open, half-open → closed)
- Log boot behavior (cache loaded, blocking on critical sources, all sources loaded)

## Compliance

- Serving stale blocklists during failures is a security trade-off (better stale than none)
- Critical sources (malware/phishing) are prioritized for boot blocking

## Risks & Mitigations

- Risk: Circuit breaker prevents blocklist updates for too long — Mitigation: Half-open state allows periodic retry, 6hr max backoff
- Risk: Boot blocking on critical sources takes too long — Mitigation: Timeout after configurable period, serve with whatever is loaded

## Dependencies & Sequencing

- Depends on: 01-002 (blocklist daemon)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- Backoff timing is incorrect (delays don't match PRD sequence)
- Circuit breaker logic has race conditions

## Maintenance Notes

- Critical sources list should be configurable (different deployments may have different critical lists)
- Reviewers should verify the backoff sequence matches PRD line 360

## Commit Conventions

- `feat(blocklist): add exponential backoff for source fetch failures`
- `feat(blocklist): add circuit breaker for consecutive failures`
- `feat(blocklist): add boot behavior with critical source blocking`

## Changelog

- 2026-08-16: initialized story file
