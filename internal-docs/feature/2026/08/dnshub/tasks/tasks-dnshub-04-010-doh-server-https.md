---
story_id: "04-010"
story_title: "DoH server (HTTPS listener)"
story_name: "doh-server-https"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 10
branch: "feature/current/dnshub/story-04-010-doh-server-https"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-001"]
parallel_safe: true
modules: ["dns-server", "doh", "https"]
priority: "SHOULD"
risk_level: "medium"
tags: ["feat", "backend", "dns", "doh", "https", "encryption"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement DoH (DNS over HTTPS) server on port 443 using hickory-server's built-in HTTPS listener. Serve DNS queries over HTTPS with RFC 8484 wire format at the configured path (/dns-query). Reuse the same TLS certificates as DoT. DoH clients are subject to the same per-client policy as plain DNS.

## Current State

- **Relevant files and their roles:**
  - `src/dns/server.rs` — DnshubServer (from story 01-001)
  - `src/config/server.rs` — ServerConfig with doh sub-section (from story 01-004)
  - PRD section 4.7 (lines 1154-1196) defines DoH scope
- **Existing code excerpts:**
  - `src/config/server.rs` — ServerConfig { listen, protocol, tls, doh: Option<DohConfig> }
- **Repository conventions:** Use hickory-server HTTPS listener (https-ring feature). RFC 8484 wire format (application/dns-message). DoH endpoint: https://dns.levonk.com/dns-query.
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
- src/dns/doh.rs — DohServer: start hickory-server HTTPS listener on port 443, serve DoH at configured path (/dns-query), use the same DnshubHandler
- src/dns/server.rs — update to start DoH server alongside UDP/TCP/DoT when doh.enabled = true
- src/config/server.rs — DohConfig: enabled, listen (Vec<String>), path (String), cert (path), key (path) per PRD lines 1185-1190
- RFC 8484 wire format: accept POST with application/dns-message body, return application/dns-message response; also support GET with ?dns= base64url parameter
- Unit tests for DoH config loading and message parsing

**Out of scope:**
- DoT server (story 04-009)
- HTTP/3 (QUIC) support — future
- DoH JSON API (RFC 8484 wire format only)
- Frontend serving on port 443 (frontend is on port 8080)

## Sub-Tasks

- [ ] Create src/dns/doh.rs with DohServer: load TLS cert/key, start hickory-server HTTPS listener on port 443, serve DoH at configured path using DnshubHandler
  **Verify**: `cargo build` → exit 0
- [ ] Implement RFC 8484 wire format: parse DNS message from HTTP POST body (application/dns-message), return DNS response as application/dns-message; support GET with ?dns= base64url parameter
  **Verify**: `cargo test --lib dns::doh` → all pass (parse wire format POST, parse GET query, build response)
- [ ] Update src/dns/server.rs to start DohServer when config.server.doh.enabled = true
  **Verify**: `cargo build` → exit 0
- [ ] Verify DohConfig in src/config/server.rs matches PRD lines 1185-1190 (enabled, listen, path, cert, key)
  **Verify**: `cargo build` → exit 0
- [ ] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dns/doh.rs` — DohServer (new)
- `src/dns/server.rs` — start DoH alongside other protocols (modified)
- `src/config/server.rs` — DohConfig (already exists, verify)

## Acceptance Criteria

- [ ] DoH server starts on port 443 when doh.enabled = true
- [ ] DoH endpoint serves at configured path (/dns-query)
- [ ] RFC 8484 wire format POST requests are handled correctly
- [ ] RFC 8484 GET requests with ?dns= base64url parameter are handled
- [ ] DoH server uses the same DnshubHandler (same policy, blocklists, forwarding)
- [ ] TLS certificates are loaded from configured paths
- [ ] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dns::doh` — tests for wire format parsing and config
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start server with test TLS cert, query with `curl -H 'content-type: application/dns-message' --data-binary @query.bin https://127.0.0.1/dns-query`

## Observability

- Log DoH server startup (listen address, path, cert path)
- Log HTTPS errors

## Compliance

- DoH encrypts DNS between client and dnshub — privacy enhancement
- RFC 8484 compliance for wire format

## Risks & Mitigations

- Risk: hickory-server HTTPS listener API in 0.26 — Mitigation: Check 0.26 API, use axum + hickory-proto if hickory-server HTTPS is not available
- Risk: Port 443 conflict with Traefik — Mitigation: DoH can be disabled via config, or Traefik can proxy to dnshub's DoH port

## Dependencies & Sequencing

- Depends on: 01-001 (server scaffold and handler chain)
- Unblocks: None directly

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Code, tests, docs updated; CI green
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- hickory-server 0.26 does not expose HTTPS listener API
- RFC 8484 wire format parsing fails for standard DoH client requests

## Maintenance Notes

- DoH and DoT can share the same TLS cert/key
- Reviewers should verify RFC 8484 compliance (content-type, wire format)

## Commit Conventions

- `feat(dns-server): add DoH server on port 443 with RFC 8484 wire format`
- `feat(dns-server): support DoH GET and POST methods`

## Changelog

- 2026-08-16: initialized story file
