---
story_id: "01-001"
story_title: "Server scaffold + RequestHandler chain + upstream forwarding + caching"
story_name: "server-scaffold-and-forwarding"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 1
parallel_id: 1
branch: "feature/current/dnshub/story-01-001-server-scaffold-and-forwarding"
status: "todo"
assignee: ""
reviewer: ""
dependencies: []
parallel_safe: true
modules: ["dns-server", "config"]
priority: "MUST"
risk_level: "high"
tags: ["feat", "backend", "dns", "scaffold"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Create the foundational Rust project scaffold for dnshub: Cargo.toml with all dependencies from PRD section 5, src/main.rs with tokio runtime, a hickory-server based DNS server listening on port 53 (UDP/TCP), a custom RequestHandler chain architecture, ForwardAuthority for upstream forwarding to a single tier, and CachingClient for response caching. This is the foundation that all subsequent stories build upon.

## Current State

- **Relevant files and their roles:**
  - No files exist yet — this is a greenfield project. Only generic git repo scaffolding is present.
- **Existing code excerpts:** None — project is greenfield.
- **Repository conventions:** Rust project with cargo. Follow hickory-server 0.26 API (ZoneHandler, not Authority). Use tokio async runtime. TOML config via serde + toml crate.
- **Tech context (binding constraint from tech-context.txt):**
  - Greenfield Rust service — no existing Cargo.toml or src/ yet
  - Package manager: cargo (Rust)
  - Build: `cargo build` | Test: `cargo test` | Lint: `cargo clippy` | Format: `cargo fmt`
  - Ad-hoc runner: `cargo add` / `cargo binstall -y` (never install tools on host)
  - Never use: npm, npx, yarn, jest, biome
  - Rust toolchain: cargo 1.95.0 on PATH (rustc resolved internally via nix store)
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Build   | `cargo build` | exit 0, no errors |
  | Tests   | `cargo test` | all pass |
  | Lint    | `cargo clippy -- -D warnings` | exit 0, no warnings |
  | Format  | `cargo fmt -- --check` | exit 0, no changes needed |

## Scope

**In scope:**
- Cargo.toml with all dependencies from PRD section 5 (hickory-server 0.26, hickory-resolver 0.26, hickory-proto 0.26, tokio, serde, toml, metrics, metrics-exporter-prometheus, tracing, tracing-subscriber, axum, tower-http, rustls, tokio-rustls, ipnet, reqwest, flate2, regex, once_cell, parking_lot, heed, fastbloom, arc-swap, rusqlite, dhcproto, async-trait)
- src/main.rs with tokio main, config loading stub, server startup
- src/lib.rs with module declarations for all future modules (stubs)
- src/dns/mod.rs with RequestHandler trait wrapper and handler chain
- src/dns/server.rs with hickory-server ServerFuture setup (UDP + TCP on :53)
- src/dns/forwarding.rs with ForwardAuthority wrapping a single upstream
- src/dns/caching.rs with CachingClient wrapper (LRU cache, TTL clamping)
- src/config/mod.rs with serde structs for dnshub.toml (server, cache, upstreams sections)
- Basic integration test: start server, send a DNS query, get a response
- .gitignore for Rust (target/, *.lmdb, etc.)

**Out of scope:**
- Blocklist storage (story 01-002)
- Blocklist daemon (story 01-002)
- Prometheus metrics endpoint (story 01-003)
- Full TOML config loading with validation (story 01-004)
- Dockerfile / Ansible (story 01-005)
- Per-client policy (story 02-001)
- Tiered fallback (story 03-001)
- Serve-stale (story 03-002)
- DoT/DoH (stories 04-009, 04-010)

## Sub-Tasks

- [ ] Create Cargo.toml with all dependencies from PRD section 5
  **Verify**: `cargo metadata --no-deps --format-version 1 | jq '.packages[0].name'` → `"dnshub"`
- [ ] Create src/lib.rs with module declarations (dns, config, blocklist, metrics, policy, dhcp, query_log, api — all stubbed with `// TODO: implemented in later stories`)
  **Verify**: `cargo build` → exit 0
- [ ] Create src/config/mod.rs with serde structs for [server], [cache], [[upstreams]] sections from PRD dnshub.toml example (lines 1374-1478)
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dns/mod.rs defining a `DnshubHandler` struct that implements hickory-server's `RequestHandler` trait, delegating to an inner handler chain (Vec of handlers)
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dns/server.rs with `DnshubServer` that starts a hickory-server `ServerFuture` on UDP and TCP port 53, using the `DnshubHandler`
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dns/forwarding.rs with `ForwardingHandler` that wraps hickory-server's `ForwardAuthority` to forward queries to a single upstream resolver (configured via [upstreams] with tier=1)
  **Verify**: `cargo build` → exit 0
- [ ] Create src/dns/caching.rs with `CachingHandler` that wraps hickory's `CachingClient` for LRU caching with TTL clamping (min_ttl, max_ttl, negative_ttl from config)
  **Verify**: `cargo build` → exit 0
- [ ] Create src/main.rs with `#[tokio::main]` that loads config, builds the handler chain (Caching → Forwarding), starts the server, and handles graceful shutdown via Ctrl+C
  **Verify**: `cargo build` → exit 0
- [ ] Create tests/integration_test.rs that starts the server on a test port, sends a DNS A query for "example.com" via hickory-resolver client, and asserts a response is received
  **Verify**: `cargo test --test integration_test` → 1 passed
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `Cargo.toml` — project manifest with all dependencies
- `src/lib.rs` — library root with module declarations
- `src/main.rs` — binary entry point with tokio runtime and server startup
- `src/config/mod.rs` — serde config structs for dnshub.toml
- `src/dns/mod.rs` — DnshubHandler implementing RequestHandler trait
- `src/dns/server.rs` — hickory-server ServerFuture setup
- `src/dns/forwarding.rs` — ForwardingHandler wrapping ForwardAuthority
- `src/dns/caching.rs` — CachingHandler wrapping CachingClient
- `tests/integration_test.rs` — integration test for basic DNS resolution
- `.gitignore` — Rust gitignore

## Acceptance Criteria

- [ ] `cargo build` succeeds with zero errors
- [ ] `cargo test` passes all tests including integration test
- [ ] `cargo clippy -- -D warnings` passes with zero warnings
- [ ] Server starts on port 53 (UDP + TCP) and responds to DNS queries
- [ ] Forwarding to an upstream resolver works (query example.com gets a response)
- [ ] Caching works (second query for same domain is served from cache)
- [ ] All dependencies from PRD section 5 are in Cargo.toml

## Test Plan

- Unit: `cargo test` — integration test verifies DNS query/response cycle
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start server with `cargo run`, query with `dig @127.0.0.1 example.com` — should return A record

## Observability

- Initialize tracing subscriber with JSON format to stdout (basic setup; full observability in Phase 05)
- Log server startup, config load, and shutdown events

## Compliance

- No personal data handled in this story
- DNS queries are transient; no persistent storage yet

## Risks & Mitigations

- Risk: hickory-server 0.26 API churn (Authority renamed to ZoneHandler in 0.25-0.26) — Mitigation: Pin to 0.26.x, implement against traits not concrete types, check hickory-server docs for 0.26 API
- Risk: hickory-server "not for production" crates.io warning — Mitigation: Performance test in Phase 06; fallback to hickory-proto only if needed
- Risk: UDP packet drops under load — Mitigation: SO_REUSEPORT tuning deferred to Phase 06

## Dependencies & Sequencing

- Depends on: None (first story)
- Unblocks: 02-001 (PolicyEngine needs handler chain), 03-001 (TieredForwardHandler), 03-002 (ServeStaleHandler), 03-003 (EcsStripHandler), 03-004 (RateLimitHandler), 04-005 (DDNS needs local zones), 04-009 (DoT server), 04-010 (DoH server), 05-002 (tracing), 05-003 (query log), 05-004 (REST API), 06-001 (performance tuning)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)
- [ ] Story file updated with status

## STOP Conditions

Stop and report if:
- hickory-server 0.26 API does not match the documented RequestHandler/ForwardAuthority/CachingClient traits
- A dependency from PRD section 5 fails to compile or is unavailable
- The integration test cannot establish a UDP/TCP connection to the server

## Maintenance Notes

- This is the foundation story — all subsequent stories depend on it
- Reviewers should verify the handler chain architecture is extensible (new handlers can be added without modifying existing ones)
- The Cargo.toml lists all dependencies upfront to avoid conflicts when parallel stories add modules

## Commit Conventions

- `feat(dns-server): scaffold hickory-server with RequestHandler chain`
- `feat(dns-server): add forwarding and caching handlers`
- `feat(config): add serde structs for dnshub.toml`
- `test(dns-server): add integration test for basic DNS resolution`

## Changelog

- 2026-08-16: initialized story file
