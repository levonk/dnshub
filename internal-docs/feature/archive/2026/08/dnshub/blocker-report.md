# Blocker Report: dnshub

**Date:** 2026-08-16
**Status:** All 35 stories implemented and merged. No hard blockers. All previously deferred integration items are now resolved.

## Summary

All 35 stories across Phases 01-06 have been implemented, merged into `main`,
and validated with 825 passing tests and 0 failures. The project is feature-complete
per the PRD scope. The five previously deferred integration tasks have all been
completed and wired into the main binary.

## Previously Deferred Items — All Resolved

### 1. Main startup wiring (src/main.rs) — RESOLVED

**Stories affected:** 04-009, 04-010, 05-003, 05-004
**Resolution:** `src/main.rs` now initializes and wires all subsystems:

- Tiered forwarding via `ForwardingHandler::install_tiered` (all configured upstream tiers).
- Blocklist daemon instantiated and started when `blocklists.toml` exists; its
  `refresh_notify()` handle is shared with `HotReloadManager` so SIGHUP triggers
  immediate blocklist refresh.
- REST API server (`ApiServer`) spawned as a background tokio task when
  `[api].enabled` is true.
- Query logger attached to `DnshubHandler` via `with_query_logger`; DNS queries
  are recorded after responses are generated.
- Observability initialized (JSON logging + Jaeger OTLP tracing) before the
  handler chain is built.
- Custom DoH server (GET + POST) started when `[server.doh].enabled` is true.
- Frontend static file serving via `tower-http` `ServeDir` when
  `[frontend].static_dir` is configured.

### 2. Jaeger OTLP exporter dependencies — RESOLVED

**Story affected:** 05-002
**Resolution:** `Cargo.toml` now includes `opentelemetry_sdk` 0.32 and
`opentelemetry-otlp` 0.32 with the `grpc-tonic` feature. The tracing
implementation in `src/observability/tracing.rs` builds a real
`SdkTracerProvider` with a tonic gRPC OTLP exporter and batch span processor,
configured from `[tracing]` settings (endpoint, service name, sample rate).

### 3. Frontend static file serving — RESOLVED

**Story affected:** 05-004, 05-006
**Resolution:** `Cargo.toml` enables the `fs` feature on `tower-http`. The
`ApiServer` accepts an optional `frontend_static_dir` via `ApiServerOptions`
and mounts a `ServeDir` at the router root when configured. The main binary
passes `[frontend].static_dir` through when `[frontend].enabled` is true and
the path exists on disk.

### 4. DoH GET request production handling — RESOLVED

**Story affected:** 04-010
**Resolution:** A custom DoH server (`src/dns/doh_axum.rs`) replaces the
hickory-server built-in HTTPS listener. It uses `axum` + `hyper` + `tokio-rustls`
to serve HTTP/2 over TLS and supports both:

- POST with `application/dns-message` wire-format body (RFC 8484 §4.1.1)
- GET with `?dns=` base64url-encoded parameter (RFC 8484 §4.1.2)

Both methods decode the DNS message, dispatch it through the shared
`DnshubHandler` (same middleware chain, blocklists, and forwarding as
UDP/TCP/DoT), and return the encoded response with
`Content-Type: application/dns-message`. TLS certificates are loaded from
`[server.doh].cert`/`key` or reused from `[server.tls]` when DoH's own
cert/key are not configured.

### 5. Auth middleware not applied — RESOLVED

**Story affected:** 05-004
**Resolution:** `DnshubConfig` now includes an `ApiConfig` section with an
optional `auth_token` field. When `[api].auth_token` is set, the
`require_token` middleware is applied to all `/api/v1/*` routes via
`axum::middleware::from_fn_with_state`. When the token is `None` (the
default), no auth middleware is added — suitable for deployments behind
Traefik/Authelia.

## Environmental Limitations (Host Tooling)

These limitations affected validation during development but do not affect the code:

- `cargo clippy`: not installed (rustup not available on host)
- `cargo fmt` / `rustfmt`: installed version is deprecated (use `rustfmt-nightly`)
- `devbox`: broken on x86_64-darwin (pinned Nixpkgs snapshot unsupported)
- Docker: not installed (Dockerfile and Ansible roles validated via syntax checks only)
- Ansible: not installed (playbooks validated via Ruby YAML parsing)
- Frontend pnpm build/lint/test: not run (workflow restrictions on npm/npx)

## Known Vulnerabilities (Transitive Dependencies)

`cargo audit` reports issues in transitive dependencies that are not directly
controlled by this project:

- `idna` 0.2.3 (via `trust-dns-proto` 0.22.0 via `dhcproto`): Punycode label
  validation issue. Fixed in `idna` >= 1.0.0; upstream `dhcproto` must upgrade.
- `bincode` 1.3.3 (via `heed-types` via `heed`): Unmaintained crate. No fix
  available until `heed` upgrades its `heed-types` dependency.

Neither vulnerability is introduced by the dnshub code itself.

## Flaky Tests

- `blocklist::daemon::tests::test_trigger_refresh_runs_refresh_all_in_run_loop`:
  Occasionally fails under full parallel test load but passes in isolation.
  Pre-existing timing sensitivity, not caused by any story changes.
