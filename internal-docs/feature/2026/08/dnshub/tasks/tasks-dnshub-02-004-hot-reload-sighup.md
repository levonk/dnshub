---
story_id: "02-004"
story_title: "Hot-reload (SIGHUP) for policy + blocklists"
story_name: "hot-reload-sighup"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 2
parallel_id: 4
branch: "feature/current/dnshub/story-02-004-hot-reload-sighup"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-002", "01-004"]
parallel_safe: true
modules: ["config", "hot-reload"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "config", "hot-reload", "sighup"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement SIGHUP-based hot-reload for dnshub.toml, policy.toml, and blocklists. On SIGHUP, the service reloads config files and atomically swaps the active configuration via ArcSwap, without restarting. Blocklist daemon triggers a refresh on SIGHUP. This enables configuration changes to take effect without downtime.

## Current State

- **Relevant files and their roles:**
  - `src/config/mod.rs` — Config loading (from story 01-004), reload function to be added
  - `src/blocklist/mod.rs` — BlocklistStore with ArcSwap hot-swap (from story 01-002), SIGHUP trigger to be added
- **Existing code excerpts:**
  - `src/blocklist/hot_swap.rs` — ArcSwap<Database> for atomic blocklist swap
- **Repository conventions:** Use ArcSwap for atomic config swaps. Use tokio::signal for SIGHUP handling. Config swaps must be lock-free.
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
- src/config/hot_reload.rs — HotReloadManager: listens for SIGHUP via tokio::signal, reloads dnshub.toml and policy.toml, validates new config, ArcSwaps the active config
- src/config/mod.rs — add ArcSwap<Config> wrapper for atomic config access; add reload(path) -> Result<Config> method
- src/blocklist/daemon.rs — add trigger_refresh() method that SIGHUP calls to force immediate blocklist refresh
- src/main.rs — install SIGHUP handler that calls HotReloadManager
- Unit tests for config reload, validation on reload, ArcSwap swap

**Out of scope:**
- Hot-reload testing under load (story 06-003)
- REST API trigger for hot-reload (story 05-004)
- DHCP config hot-reload (Phase 04 — DHCP config changes require restart for port bindings)

## Sub-Tasks

- [ ] Create src/config/hot_reload.rs with HotReloadManager: holds ArcSwap<Config>, on SIGHUP reloads from disk, validates, swaps if valid, logs error and keeps old config if invalid
  **Verify**: `cargo test --lib config::hot_reload` → all pass (test valid reload, invalid reload keeps old config)
- [ ] Add ArcSwap<Config> wrapper to src/config/mod.rs: get_config() -> Config guard, reload(path) -> Result<()>
  **Verify**: `cargo build` → exit 0
- [ ] Add trigger_refresh() to src/blocklist/daemon.rs that forces immediate refresh of all sources (bypasses refresh interval)
  **Verify**: `cargo test --lib blocklist::daemon` → all pass (trigger refresh, verify daemon fetches)
- [ ] Install SIGHUP handler in src/main.rs using tokio::signal::unix::SignalKind::hangup, calls HotReloadManager and blocklist daemon trigger_refresh
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/config/hot_reload.rs` — HotReloadManager (new)
- `src/config/mod.rs` — ArcSwap<Config> wrapper (modified)
- `src/blocklist/daemon.rs` — trigger_refresh() (modified)
- `src/main.rs` — SIGHUP handler installation (modified)

## Acceptance Criteria

- [ ] SIGHUP triggers config reload from disk
- [ ] Invalid config on reload keeps the old config (no crash, logs error)
- [ ] SIGHUP triggers blocklist daemon refresh
- [ ] Config swap is atomic via ArcSwap (no lock contention on hot path)
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib config::hot_reload` and `cargo test --lib blocklist::daemon` — tests for reload and refresh trigger
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start server, modify config, send SIGHUP, verify new config is active

## Observability

- Log reload events (success, failure, what changed)
- Log blocklist refresh triggers

## Compliance

- No personal data in config files

## Risks & Mitigations

- Risk: SIGHUP during active query processing causes inconsistency — Mitigation: ArcSwap is lock-free, old config remains valid until all readers drop it
- Risk: Config validation fails on reload — Mitigation: Keep old config, log error, do not swap

## Dependencies & Sequencing

- Depends on: 01-002 (blocklist daemon), 01-004 (config loading)
- Unblocks: 06-003 (hot-reload testing under load)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- tokio::signal::unix is not available on the target platform
- ArcSwap swap produces panics or data corruption

## Maintenance Notes

- DHCP config changes (port bindings, interface) cannot be hot-reloaded — require restart
- Reviewers should verify invalid config does not crash the server

## Commit Conventions

- `feat(config): add SIGHUP hot-reload with ArcSwap`
- `feat(blocklist): add trigger_refresh for SIGHUP reload`

## Changelog

- 2026-08-16: initialized story file
