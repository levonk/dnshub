# Task Index: dnshub

**PRD:** `internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md`
**Created:** 2026-08-16
**Total stories:** 35

## Story Index

| Story ID | Title | Phase | Status | Assignee | Parallel-safe | Dependencies | Dependants | Modules | Branch |
|---|---|---:|---|---|---|---|---|---|---|
| 01-001 | Server scaffold + RequestHandler chain + upstream forwarding + caching | 01 | [ ] Todo |  | true | — | 02-001, 03-001, 03-002, 03-003, 03-004, 04-005, 04-009, 04-010, 05-002, 05-003, 05-004, 06-001 | dns-server, config | feature/current/dnshub/story-01-001-server-scaffold-and-forwarding |
| 01-002 | Blocklist storage (LMDB + Bloom filter) + daemon (fetcher/parser/compiler) | 01 | [ ] Todo |  | true | — | 02-001, 02-002, 02-004, 06-002, 06-003 | blocklist, storage | feature/current/dnshub/story-01-002-blocklist-storage-and-daemon |
| 01-003 | Basic Prometheus metrics | 01 | [ ] Todo |  | true | — | 02-003, 05-001, 05-005 | metrics | feature/current/dnshub/story-01-003-basic-prometheus-metrics |
| 01-004 | TOML config loading (dnshub.toml + blocklists.toml) | 01 | [ ] Todo |  | true | — | 02-001, 02-004, 04-001, 04-002, 04-003, 04-004, 04-006, 04-007, 04-008 | config | feature/current/dnshub/story-01-004-toml-config-loading |
| 01-005 | Dockerfile + Ansible role | 01 | [ ] Todo |  | true | — | 04-012, 06-004 | infra, ansible | feature/current/dnshub/story-01-005-dockerfile-and-ansible-role |
| 02-001 | PolicyEngine + ClientResolver + PolicyHandler + policy.toml | 02 | [ ] Todo |  | true | 01-001, 01-002, 01-004 | 04-011 | policy, client-resolver | feature/current/dnshub/story-02-001-policy-engine-and-client-resolver |
| 02-002 | Category bitmap in LMDB + Adblock Plus parser | 02 | [ ] Todo |  | true | 01-002 | — | blocklist, parser | feature/current/dnshub/story-02-002-category-bitmap-and-adblock-parser |
| 02-003 | Per-client metrics | 02 | [ ] Todo |  | true | 01-003 | 05-001 | metrics | feature/current/dnshub/story-02-003-per-client-metrics |
| 02-004 | Hot-reload (SIGHUP) for policy + blocklists | 02 | [ ] Todo |  | true | 01-002, 01-004 | 06-003 | config, hot-reload | feature/current/dnshub/story-02-004-hot-reload-sighup |
| 03-001 | TieredForwardHandler with per-tier timeout | 03 | [ ] Todo |  | true | 01-001 | — | dns-server, forwarding | feature/current/dnshub/story-03-001-tiered-forward-handler |
| 03-002 | ServeStaleHandler (RFC 8767) | 03 | [ ] Todo |  | true | 01-001 | — | dns-server, cache | feature/current/dnshub/story-03-002-serve-stale-handler |
| 03-003 | EcsStripHandler | 03 | [ ] Todo |  | true | 01-001 | — | dns-server, ecs | feature/current/dnshub/story-03-003-ecs-strip-handler |
| 03-004 | RateLimitHandler (token bucket) | 03 | [ ] Todo |  | true | 01-001 | — | dns-server, rate-limit | feature/current/dnshub/story-03-004-rate-limit-handler |
| 04-001 | DHCPv4 server core (lease store, state machine, pool allocator, options) | 04 | [ ] Todo |  | true | 01-004 | 05-004 | dhcp, dhcpv4 | feature/current/dnshub/story-04-001-dhcpv4-server-core |
| 04-002 | DHCPv6 server core (lease store, state machine, IA_NA, options) | 04 | [ ] Todo |  | true | 01-004 | 05-004 | dhcp, dhcpv6 | feature/current/dnshub/story-04-002-dhcpv6-server-core |
| 04-003 | RA/SLAAC (Router Advertisements with RDNSS/DNSSL) | 04 | [ ] Todo |  | true | 01-004 | — | dhcp, ra-slaac | feature/current/dnshub/story-04-003-ra-slaac-router-advertisements |
| 04-004 | MAC blocklist + client classification + per-pool options | 04 | [ ] Todo |  | true | 01-004 | — | dhcp, classification | feature/current/dnshub/story-04-004-mac-blocklist-and-client-classification |
| 04-005 | DDNS (auto-create/remove DNS records from DHCP leases) | 04 | [ ] Todo |  | true | 01-001 | — | dhcp, ddns | feature/current/dnshub/story-04-005-ddns-auto-dns-from-dhcp |
| 04-006 | PXE/BOOTP/TFTP server (network boot, per-arch bootfile, iPXE) | 04 | [ ] Todo |  | true | 01-004 | — | dhcp, pxe, tftp | feature/current/dnshub/story-04-006-pxe-bootp-tftp-server |
| 04-007 | DHCP relay agent + multi-VLAN + Option 82 | 04 | [ ] Todo |  | true | 01-004 | — | dhcp, relay | feature/current/dnshub/story-04-007-dhcp-relay-agent-multi-vlan |
| 04-008 | Lease audit log + rogue DHCP detection | 04 | [ ] Todo |  | true | 01-004 | — | dhcp, audit, rogue-detection | feature/current/dnshub/story-04-008-lease-audit-log-rogue-detection |
| 04-009 | DoT server (TLS listener, cert mounting) | 04 | [ ] Todo |  | true | 01-001 | — | dns-server, dot, tls | feature/current/dnshub/story-04-009-dot-server-tls |
| 04-010 | DoH server (HTTPS listener) | 04 | [ ] Todo |  | true | 01-001 | — | dns-server, doh, https | feature/current/dnshub/story-04-010-doh-server-https |
| 04-011 | ClientResolver DHCP integration (IP to hostname to profile from lease tables) | 04 | [ ] Todo |  | true | 02-001 | — | policy, client-resolver, dhcp | feature/current/dnshub/story-04-011-client-resolver-dhcp-integration |
| 04-012 | Docker macvlan network + TLS cert Ansible | 04 | [ ] Todo |  | true | 01-005 | 06-004 | infra, docker, ansible | feature/current/dnshub/story-04-012-docker-macvlan-tls-ansible |
| 05-001 | Full Prometheus metrics (all labels from PRD section 4.6) | 05 | [ ] Todo |  | true | 01-003, 02-003 | — | metrics, prometheus | feature/current/dnshub/story-05-001-full-prometheus-metrics |
| 05-002 | tracing JSON logs to Loki + Jaeger traces | 05 | [ ] Todo |  | true | 01-001 | — | observability, tracing, jaeger | feature/current/dnshub/story-05-002-tracing-json-jaeger |
| 05-003 | Query log SQLite ring buffer + QueryLogHandler | 05 | [ ] Todo |  | true | 01-001 | — | query-log, sqlite | feature/current/dnshub/story-05-003-query-log-sqlite-ring-buffer |
| 05-004 | REST API (axum, all endpoints from PRD section 4.9) | 05 | [ ] Todo |  | true | 01-001, 04-001, 04-002 | — | api, axum | feature/current/dnshub/story-05-004-rest-api-axum |
| 05-005 | Grafana dashboard (metrics + Loki logs) | 05 | [ ] Todo |  | true | 01-003 | — | observability, grafana | feature/current/dnshub/story-05-005-grafana-dashboard |
| 05-006 | NextJS frontend (config + status + DHCP + query log viewer) | 05 | [ ] Todo |  | true | — | — | frontend, nextjs | feature/current/dnshub/story-05-006-nextjs-frontend |
| 06-001 | Performance tuning (SO_REUSEPORT, buffer sizes) | 06 | [ ] Todo |  | true | 01-001 | — | dns-server, performance | feature/current/dnshub/story-06-001-performance-tuning-so-reuseport |
| 06-002 | Blocklist source failure handling (backoff, circuit breaker) | 06 | [ ] Todo |  | true | 01-002 | — | blocklist, reliability | feature/current/dnshub/story-06-002-blocklist-failure-handling-backoff |
| 06-003 | Hot-reload testing under load | 06 | [ ] Todo |  | true | 02-004, 01-002 | — | testing, hot-reload | feature/current/dnshub/story-06-003-hot-reload-testing-under-load |
| 06-004 | Migration from existing stack | 06 | [ ] Todo |  | true | 01-005, 04-012 | — | infra, migration | feature/current/dnshub/story-06-004-migration-from-existing-stack |

## Phase Summary

| Phase | Title | Story Count | Story IDs |
|---|---|---:|---|
| 01 | MVP — Caching resolver + blocklists + upstream forwarding | 5 | 01-001 through 01-005 |
| 02 | Per-client policy + categorized blocklists | 4 | 02-001 through 02-004 |
| 03 | Full orchestrator — tiered fallback + serve-stale + ECS | 4 | 03-001 through 03-004 |
| 04 | DHCP server (v4 + v6) + DoT/DoH serving | 12 | 04-001 through 04-012 |
| 05 | Observability + query logging + frontend | 6 | 05-001 through 05-006 |
| 06 | Hardening + production | 4 | 06-001 through 06-004 |

## Dependency Rules

- Stories within the same phase MUST NOT depend on each other.
- All dependencies reference stories from earlier phases only.
- Each story is designed to be independently implementable and testable.

## High-Risk Stories

| Story ID | Title | Risk Level | Reason |
|---|---|---|---|
| 04-001 | DHCPv4 server core | high | Custom DHCP state machine (no mature Rust DHCP server library), L2 broadcast requirements, lease conflict detection |
| 04-002 | DHCPv6 server core | high | DHCPv6 wire format complexity, DUID/IAID handling, stateful + stateless modes |
| 04-006 | PXE/BOOTP/TFTP server | high | TFTP protocol implementation, PXE option handling, multiple architecture support |
| 04-007 | DHCP relay agent + multi-VLAN | high | Option 82 parsing, giaddr-based pool selection, relay trust security |
| 01-001 | Server scaffold + RequestHandler chain | high | hickory-server API churn risk (0.25 to 0.26 broke Authority to ZoneHandler), foundation for all subsequent work |
| 01-002 | Blocklist storage + daemon | high | LMDB C dependency, Bloom filter sizing, hot-swap atomicity, multiple format parsers |
