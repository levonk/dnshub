---
story_id: "03-002"
story_title: "ServeStaleHandler (RFC 8767)"
story_name: "serve-stale-handler"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 3
parallel_id: 2
branch: "feature/current/dnshub/story-03-002-serve-stale-handler"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["dns-server", "cache"]
priority: "MUST"
risk_level: "medium"
tags: ["feat", "backend", "dns", "cache", "serve-stale", "rfc-8767"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement ServeStaleHandler (RFC 8767) that wraps the caching layer and serves expired cache entries when all upstream tiers are unavailable. When a cache entry expires, the handler attempts to refresh from upstream; if upstream fails, it serves the stale entry with a modified TTL. This ensures DNS resolution continues during upstream outages.

## Current State

- **Relevant files and their roles:**
  - `src/dns/caching.rs` — CachingHandler with LRU cache and TTL clamping (from story 01-001)
  - `src/config/cache.rs` — CacheConfig with serve_stale, serve_stale_ttl fields (from story 01-004)
- **Existing code excerpts:**
  - `src/dns/caching.rs` — CachingHandler wraps CachingClient
  - `src/config/cache.rs` — CacheConfig { min_ttl, max_ttl, negative_ttl, serve_stale: bool, serve_stale_ttl: u64, max_entries }
- **Repository conventions:** Follow RFC 8767 for serve-stale behavior. Use hickory CachingClient as base. Modify TTL on stale responses.
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
- src/dns/serve_stale.rs — ServeStaleHandler: wraps CachingHandler, on cache miss delegates to upstream (inner handler), on upstream failure checks for expired cache entry and serves it with modified TTL (serve_stale_ttl from config)
- src/dns/caching.rs — add get_stale(domain, qtype) method that returns expired entries
- src/dns/mod.rs — insert ServeStaleHandler into chain wrapping the caching + forwarding handlers
- Unit tests: stale entry served on upstream failure, fresh entry served when upstream available, no stale entry → SERVFAIL

**Out of scope:**
- TieredForwardHandler (story 03-001 — ServeStaleHandler wraps whatever forwarding handler exists)
- ECS stripping (story 03-003)
- Rate limiting (story 03-004)
- Prefetching (future — refresh stale entries in background before they're queried)

## Sub-Tasks

- [ ] Create src/dns/serve_stale.rs with ServeStaleHandler implementing RequestHandler: on request, check cache; if fresh, return; if expired, try upstream; if upstream fails, return stale with modified TTL; if no stale, return SERVFAIL
  **Verify**: `cargo build` → exit 0
- [ ] Add get_stale(domain, qtype) -> Option<Response> to src/dns/caching.rs that returns expired cache entries (entries past their TTL but still in cache)
  **Verify**: `cargo test --lib dns::caching` → all pass (store entry, expire it, retrieve stale)
- [ ] Implement TTL modification on stale responses: set TTL to serve_stale_ttl from config (default 86400 seconds per PRD line 1409)
  **Verify**: `cargo test --lib dns::serve_stale` → all pass (verify TTL is modified on stale response)
- [ ] Wire ServeStaleHandler into handler chain in src/dns/mod.rs (wraps caching + forwarding)
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dns/serve_stale.rs` — ServeStaleHandler (new)
- `src/dns/caching.rs` — add get_stale() method (modified)
- `src/dns/mod.rs` — wire ServeStaleHandler into chain (modified)

## Acceptance Criteria

- [ ] Fresh cache entries are served normally (no stale behavior)
- [ ] Expired entries are served when upstream is unavailable
- [ ] Stale entries have TTL modified to serve_stale_ttl
- [ ] No stale entry available → SERVFAIL (not a stale empty response)
- [ ] Serve-stale can be disabled via config (serve_stale = false)
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dns::serve_stale` — tests for fresh, stale-on-failure, no-stale
- Integration: `cargo test --test integration_test` — verify serve-stale works end-to-end
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed

## Observability

- Log serve-stale events (domain, original TTL, stale TTL, reason: upstream timeout/failure)
- Metric for serve-stale usage (future: dnshub_serve_stale_total)

## Compliance

- RFC 8767 compliance: stale entries should have decreasing TTLs, clients should not cache stale entries longer than serve_stale_ttl

## Risks & Mitigations

- Risk: Stale entries may contain outdated information (IP changes) — Mitigation: serve_stale_ttl limits how long stale is served; background refresh attempts continue
- Risk: CachingClient API may not expose expired entries — Mitigation: May need to access cache internals or maintain a separate stale cache

## Dependencies & Sequencing

- Depends on: 01-001 (caching handler and handler chain)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- hickory CachingClient does not expose expired entries for serve-stale
- RFC 8767 compliance cannot be achieved with hickory-server's cache API

## Maintenance Notes

- Consider prefetching as a future enhancement (refresh popular stale entries in background)
- Reviewers should verify TTL modification on stale responses

## Commit Conventions

- `feat(dns-server): add ServeStaleHandler per RFC 8767`
- `feat(dns-server): wire ServeStaleHandler into handler chain`

## Changelog

- 2026-08-16: initialized story file
