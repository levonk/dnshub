---
story_id: "06-003"
story_title: "Hot-reload testing under load"
story_name: "hot-reload-testing-under-load"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 6
parallel_id: 3
branch: "feature/current/dnshub/story-06-003-hot-reload-testing-under-load"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["02-004", "01-002"]
parallel_safe: true
modules: ["testing", "hot-reload"]
priority: "SHOULD"
risk_level: "medium"
tags: ["test", "backend", "hot-reload", "load-testing", "reliability"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Create comprehensive tests for hot-reload behavior under DNS query load. Verify that SIGHUP-triggered config reloads and blocklist refreshes do not cause query drops, errors, or inconsistencies. Test concurrent reload + query load scenarios, verify ArcSwap atomicity under load, and test blocklist hot-swap while queries are active.

## Current State

- **Relevant files and their roles:**
  - `src/config/hot_reload.rs` — HotReloadManager with SIGHUP handler (from story 02-004)
  - `src/blocklist/hot_swap.rs` — ArcSwap hot-swap for blocklist database (from story 01-002)
  - PRD section 7 Phase 6 (line 1763) mentions hot-reload testing
- **Existing code excerpts:**
  - `src/config/hot_reload.rs` — SIGHUP handler, reload config, ArcSwap swap
  - `src/blocklist/hot_swap.rs` — ArcSwap<Database> for atomic blocklist swap
- **Repository conventions:** Use tokio for concurrent test execution. Use criterion or custom load generator for query load. Verify zero query drops during reload.
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
- tests/hot_reload_test.rs — integration tests for hot-reload under load:
  - Test 1: Send SIGHUP during active query load, verify zero query drops
  - Test 2: Trigger blocklist hot-swap during active query load, verify zero errors
  - Test 3: Reload config with invalid values during load, verify old config remains active, zero errors
  - Test 4: Rapid SIGHUP signals (multiple reloads in quick succession), verify no panics or corruption
  - Test 5: Blocklist hot-swap with concurrent blocklist lookups, verify ArcSwap atomicity
- tests/load_generator.rs — helper module for generating DNS query load (async UDP queries at configurable rate)
- tests/common/mod.rs — shared test utilities (start test server, send queries, verify responses)

**Out of scope:**
- Performance benchmarks (story 06-001)
- Blocklist failure handling (story 06-002)
- Migration testing (story 06-004)
- CI/CD pipeline setup (future)

## Sub-Tasks

- [ ] Create tests/common/mod.rs with test utilities: start_test_server(port) -> TestServer, send_query(server, domain) -> Response, generate_load(server, rate, duration) -> LoadResult
  **Verify**: `cargo test --test common` → compiles (or verify with `cargo build --tests`)
- [ ] Create tests/load_generator.rs with async load generator: send N queries per second for M seconds, track success/failure/timeout counts
  **Verify**: `cargo build --tests` → exit 0
- [ ] Create tests/hot_reload_test.rs Test 1: start server, generate load (100 qps for 10s), send SIGHUP at 5s, verify zero query failures
  **Verify**: `cargo test --test hot_reload_test -- test_sighup_during_load` → pass (0 failures during reload)
- [ ] Create tests/hot_reload_test.rs Test 2: start server, generate load, trigger blocklist hot-swap at midpoint, verify zero errors
  **Verify**: `cargo test --test hot_reload_test -- test_blocklist_hot_swap_during_load` → pass
- [ ] Create tests/hot_reload_test.rs Test 3: start server, generate load, send SIGHUP with invalid config, verify old config remains, zero errors
  **Verify**: `cargo test --test hot_reload_test -- test_invalid_config_reload_during_load` → pass
- [ ] Create tests/hot_reload_test.rs Test 4: start server, generate load, send 10 rapid SIGHUP signals, verify no panics
  **Verify**: `cargo test --test hot_reload_test -- test_rapid_sighup` → pass
- [ ] Create tests/hot_reload_test.rs Test 5: start server, concurrent blocklist lookups + hot-swap, verify ArcSwap atomicity (no corrupted reads)
  **Verify**: `cargo test --test hot_reload_test -- test_arcswap_atomicity` → pass
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `tests/hot_reload_test.rs` — hot-reload integration tests (new)
- `tests/load_generator.rs` — async load generator (new)
- `tests/common/mod.rs` — shared test utilities (new)

## Acceptance Criteria

- [ ] SIGHUP during active query load produces zero query failures
- [ ] Blocklist hot-swap during active query load produces zero errors
- [ ] Invalid config reload during load keeps old config active with zero errors
- [ ] Rapid SIGHUP signals don't cause panics or corruption
- [ ] ArcSwap atomicity verified under concurrent access
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Integration: `cargo test --test hot_reload_test` — all 5 hot-reload tests pass
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Test results include query success/failure counts during reload
- Test logs show reload events and any errors

## Compliance

- No personal data in test queries (use example.com, test.test domains)

## Risks & Mitigations

- Risk: Test flakiness due to timing — Mitigation: Use generous timeouts, verify counts not exact timing
- Risk: ArcSwap race condition not caught by tests — Mitigation: Run test with multiple iterations, use loom for concurrency verification if needed

## Dependencies & Sequencing

- Depends on: 02-004 (hot-reload SIGHUP), 01-002 (blocklist hot-swap)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] All 5 hot-reload tests pass consistently
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- Hot-reload tests are flaky and cannot be made reliable
- ArcSwap atomicity violations are detected (indicates a bug in stories 01-002 or 02-004)

## Maintenance Notes

- Tests should be run in CI to catch hot-reload regressions
- Consider using loom crate for formal concurrency verification of ArcSwap usage
- Reviewers should verify tests actually send SIGHUP and trigger hot-swap (not just mock them)

## Commit Conventions

- `test(hot-reload): add integration tests for SIGHUP during query load`
- `test(hot-reload): add blocklist hot-swap atomicity tests`
- `test(hot-reload): add invalid config and rapid SIGHUP tests`

## Changelog

- 2026-08-16: initialized story file
