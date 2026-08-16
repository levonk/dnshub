---
story_id: "04-009"
story_title: "DoT server (TLS listener, cert mounting)"
story_name: "dot-server-tls"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 9
branch: "feature/current/dnshub/story-04-009-dot-server-tls"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["dns-server", "dot", "tls"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "dns", "dot", "tls", "encryption"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement DoT (DNS over TLS) server on port 853 using hickory-server's built-in TLS listener. Load TLS certificates from configured paths (reuse existing Traefik/ACME certs mounted into the container). DoT clients are subject to the same per-client policy as plain DNS — the source IP is extracted from the TLS connection and mapped to a profile via ClientResolver.

## Current State

- **Relevant files and their roles:**
  - `src/dns/server.rs` — DnshubServer with UDP/TCP on :53 (from story 01-001)
  - `src/config/server.rs` — ServerConfig with tls sub-section (from story 01-004)
  - PRD section 4.7 (lines 1154-1196) defines DoT scope
- **Existing code excerpts:**
  - `src/config/server.rs` — ServerConfig { listen, protocol, tls: Option<TlsConfig> }
  - `src/dns/server.rs` — DnshubServer starts ServerFuture on UDP/TCP
- **Repository conventions:** Use hickory-server TLS listener (tls-ring feature). Use rustls + tokio-rustls for TLS. Cert/key loaded from file paths in config.
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
- src/dns/dot.rs — DotServer: start hickory-server TLS listener on port 853, load cert/key from configured paths, use the same DnshubHandler as the UDP/TCP server
- src/dns/server.rs — update to start DoT server alongside UDP/TCP when tls.enabled = true
- src/config/server.rs — TlsConfig: enabled, listen (Vec<String>), cert (path), key (path) per PRD lines 1179-1183
- TLS certificate loading: read PEM cert and key from file, create rustls ServerConfig
- Unit tests for TLS config loading and server startup

**Out of scope:**
- DoH server (story 04-010)
- ACME certificate generation (certs are mounted from existing Traefik/ACME)
- TLS cert hot-reload (future — restart required for cert changes in v1)
- Client certificate authentication (not in PRD)

## Sub-Tasks

- [ ] Create src/dns/dot.rs with DotServer: load TLS cert/key from configured paths, create rustls ServerConfig, start hickory-server TLS listener on configured port (:853) using the DnshubHandler
  **Verify**: `cargo build` → exit 0
- [ ] Implement TLS cert loading: read PEM files, create rustls::ServerConfig with tokio-rustls, handle cert/key file errors gracefully
  **Verify**: `cargo test --lib dns::dot` → all pass (load test cert/key, verify ServerConfig created)
- [ ] Update src/dns/server.rs to start DotServer when config.server.tls.enabled = true
  **Verify**: `cargo build` → exit 0
- [ ] Verify TlsConfig in src/config/server.rs matches PRD lines 1179-1183 (enabled, listen, cert, key)
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dns/dot.rs` — DotServer (new)
- `src/dns/server.rs` — start DoT alongside UDP/TCP (modified)
- `src/config/server.rs` — TlsConfig (already exists, verify)

## Acceptance Criteria

- [ ] DoT server starts on port 853 when tls.enabled = true
- [ ] TLS certificates are loaded from configured file paths
- [ ] DoT server uses the same DnshubHandler (same policy, blocklists, forwarding)
- [ ] Source IP from TLS connection is available for ClientResolver
- [ ] Invalid cert/key files produce a clear error, not a crash
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dns::dot` — tests for TLS config loading
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start server with test TLS cert, query with `kdig @127.0.0.1 +tls example.com`

## Observability

- Log DoT server startup (listen address, cert path)
- Log TLS handshake errors

## Compliance

- TLS encrypts DNS between client and dnshub — privacy enhancement
- TLS certs are mounted read-only from host (not baked into image)

## Risks & Mitigations

- Risk: hickory-server TLS listener API in 0.26 — Mitigation: Check 0.26 API for TLS server support, use rustls directly if needed
- Risk: TLS cert file permissions in Docker — Mitigation: Ansible mounts cert dir read-only (story 04-012)

## Dependencies & Sequencing

- Depends on: 01-001 (server scaffold and handler chain)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- hickory-server 0.26 does not expose TLS listener API
- rustls cannot load PEM cert/key files

## Maintenance Notes

- TLS cert changes require restart in v1 (hot-reload deferred to future)
- Reviewers should verify cert/key paths are configurable and errors are handled

## Commit Conventions

- `feat(dns-server): add DoT server on port 853 with TLS`
- `feat(dns-server): load TLS certificates from configured paths`

## Changelog

- 2026-08-16: initialized story file
