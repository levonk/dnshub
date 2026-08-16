# dnshub

A Rust DNS orchestration service designed to replace the glue layer currently
spread across AdGuard Home, dnsdist, and CoreDNS. Built on Hickory DNS with
Tokio, dnshub consolidates blocklists, rate limiting, ECS stripping, caching,
ordered upstream fallback, per-client policy, DHCP, DoT/DoH, and observability
into a single service.

## Features

- **DNS serving** — UDP/TCP on port 53, DoT on 853, DoH on 443
- **DHCP** — DHCPv4, DHCPv6 (IA_NA), RA/SLAAC, PXE/BOOTP/TFTP, relay agent
- **Blocklists** — LMDB + Bloom filter storage, hosts/domains/Adblock Plus parsers,
  categorized sources, hot-swap, circuit breaker, cached boot
- **Policy** — Per-client profiles via DHCP lease identity, custom allow/block,
  category-based filtering
- **Forwarding** — Tiered upstream fallback with per-tier timeouts, serve-stale
  caching (RFC 8767), ECS stripping
- **Rate limiting** — Token bucket per-client and global
- **Observability** — Prometheus metrics, structured JSON tracing, Grafana
  dashboards, SQLite query log ring buffer
- **REST API** — axum-based API for config, status, DHCP leases, blocklists,
  query log, audit events
- **Frontend** — Static-export NextJS dashboard
- **Config** — TOML-based with SIGHUP hot-reload
- **Deployment** — Dockerfile with macvlan networking, Ansible role, migration
  tooling from existing DNS stack

## Architecture

```
src/
├── dns/           # Hickory DNS server, middleware chain, DoT/DoH
├── dhcp/          # DHCPv4/v6 servers, RA, PXE, relay, DDNS, audit
├── blocklist/     # LMDB storage, Bloom filter, daemon, health tracking
├── policy/        # Policy engine, client resolver, profiles
├── config/        # TOML config loading, validation, hot-reload
├── metrics/       # Prometheus counters, gauges, histograms
├── observability/ # Tracing (Jaeger/OTLP), structured logging
├── query_log/     # SQLite ring buffer for recent queries
├── api/           # axum REST API server
├── client_resolver/ # IP → client identity resolution
└── frontend/      # NextJS static-export frontend
```

## Status

All 35 stories across 6 phases implemented and merged. 843 tests passing.

See `internal-docs/feature/2026/08/dnshub/` for the PRD, task index, and
blocker report.

## Development

### Prerequisites

- [nix](https://nixos.org) + [direnv](https://direnv.net)
- [devbox](https://www.jetify.io/devbox) (dev shell)
- [just](https://github.com/casey/just) (command runner)

### Quick Start

```bash
# Clone and enter project directory
git clone <repository-url> dnshub
cd dnshub

# Bootstrap automatically (direnv activates devbox, which runs bootstrap-internal)
just bootstrap

# Start development
just dev
```

### Standard Developer Workflow

This project uses the **Devbox-first developer experience**:

1. **Enter directory** → `direnv` auto-activates `devbox` environment
2. **Bootstrap** → `devbox init_hook` calls `just bootstrap-internal` automatically
3. **Development** → Use `just` commands for all common tasks

#### Available Commands

```bash
just bootstrap    # Install/verify tools and set up environment
just dev          # Start development server
just build        # Build the project
just test         # Run tests
just lint         # Run linters
just typecheck    # Run type checking
just clean        # Clean build artifacts
just doctor       # Check environment health
just quality      # Run all quality checks (lint + test + typecheck)
```

#### Developer Flows

**New developers** (simple commands):
```bash
just dev          # Ensures devbox environment, then starts dev server
just build        # Ensures devbox environment, then builds
```

**Power users** (already in devbox shell):
```bash
just dev-internal # Direct call to dev implementation
just build-internal # Direct call to build implementation
```

**CI/CD and automation**:
```bash
devbox run build  # Calls just build-internal directly
devbox run test   # Calls just test-internal directly
```

### Project Structure

```
dnshub/
├── justfile           # Command recipes (replaces Makefile)
├── devbox.json        # Development environment configuration
├── .envrc            # direnv configuration (auto-activates devbox)
├── src/              # Source code
├── tests/            # Test files
├── docs/             # Project documentation
└── internal-docs/    # Architecture decisions and specs
```

### Environment Management

- **devbox**: Provides reproducible Nix-based development environment
- **direnv**: Automatically activates environment when entering directory
- **just**: Provides consistent command interface across all projects

### Copier

This repository was generated with `copier`.

- Commit `.copier-answers.yml` so future `copier update` runs are reproducible.

## License

Copyright (c) 2026 The Authors
Licensed under the [AGPL-3.0](./LICENSE.md).
