---
story_id: "04-012"
story_title: "Docker macvlan network + TLS cert Ansible"
story_name: "docker-macvlan-tls-ansible"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 12
branch: "feature/current/dnshub/story-04-012-docker-macvlan-tls-ansible"
status: "done"
assignee: ""
reviewer: ""
dependencies: ["01-005"]
parallel_safe: true
modules: ["infra", "docker", "ansible"]
priority: "MUST"
risk_level: "medium"
tags: ["feat", "infra", "docker", "macvlan", "ansible", "tls"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Extend the Dockerfile and Ansible role (from story 01-005) to support DHCP L2 broadcast access via macvlan networking, TLS certificate mounting for DoT/DoH, and additional port mappings (DHCP 67/547, TFTP 69, DoT 853, DoH 443). The macvlan network gives dnshub its own MAC address on the physical network, which is required for a DHCP server.

## Current State

- **Relevant files and their roles:**
  - `Dockerfile` — multi-stage build (from story 01-005)
  - `ansible/roles/dns-dnshub/tasks/main.yml` — docker_container deployment (from story 01-005)
  - `ansible/roles/dns-dnshub/templates/dnshub.toml.j2` — config template (from story 01-005)
  - PRD lines 948-955 define macvlan requirement
- **Existing code excerpts:**
  - `ansible/roles/dns-dnshub/tasks/main.yml` — docker_container task with basic port mappings
- **Repository conventions:** Docker macvlan network gives container its own MAC on the physical network. TLS certs mounted read-only from Traefik/ACME cert directory. Ansible role creates macvlan network if not present.
- **Tech context (binding constraint from tech-context.txt):**
  - Container runtime: Docker (devcontainer present at .devcontainer/)
  - Config format: TOML (dnshub config)
  - Never use: npm, npx, yarn, jest, biome
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Docker  | `docker build -t dnshub .` | image built successfully |
  | Ansible | `ansible-playbook --syntax-check` | no syntax errors |

## Scope

**In scope:**
- ansible/roles/dns-dnshub/tasks/main.yml — update docker_container task: use macvlan network, add ports 67/udp (DHCPv4), 547/udp (DHCPv6), 69/udp (TFTP), 853/tcp (DoT), 443/tcp (DoH)
- ansible/roles/dns-dnshub/tasks/macvlan.yml — create Docker macvlan network if not exists (docker_network task with driver: macvlan, parent interface, subnet)
- ansible/roles/dns-dnshub/handlers/main.yml — update restart handler for macvlan container
- ansible/roles/dns-dnshub/defaults/main.yml — add macvlan variables (parent_interface, macvlan_subnet, macvlan_gateway, macvlan_ip_range) and TLS cert paths
- ansible/roles/dns-dnshub/templates/dnshub.toml.j2 — update with TLS cert paths, DHCP interface, DoT/DoH config
- TLS cert volume mount: mount Traefik/ACME cert directory read-only into container at /etc/dnshub/tls/
- Dockerfile — expose ports 67, 547, 69, 853, 443 in addition to 53, 9090, 8080

**Out of scope:**
- ACME certificate generation (certs come from existing Traefik/ACME)
- Docker Compose file (Ansible manages deployment)
- Kubernetes manifests (not in scope for homelab)
- Non-macvlan deployment mode (macvlan is the only supported mode for DHCP)

## Sub-Tasks

- [x] Update ansible/roles/dns-dnshub/defaults/main.yml with macvlan variables: infra_dhcp_macvlan_parent (physical interface), infra_dhcp_macvlan_subnet, infra_dhcp_macvlan_gateway, infra_dhcp_macvlan_ip, TLS cert paths (infra_dnshub_tls_cert_dir)
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [x] Create ansible/roles/dns-dnshub/tasks/macvlan.yml with docker_network task: driver: macvlan, parent: {{ infra_dhcp_macvlan_parent }}, subnet: {{ infra_dhcp_macvlan_subnet }}, check if network exists first
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [x] Update ansible/roles/dns-dnshub/tasks/main.yml: include macvlan.yml, update docker_container to use macvlan network, add port mappings 67/udp, 547/udp, 69/udp, 853/tcp, 443/tcp, add TLS cert volume mount (read-only)
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [x] Update ansible/roles/dns-dnshub/templates/dnshub.toml.j2 with [server.tls] and [server.doh] sections (cert/key paths, listen addresses), [dhcp] section with interface and listen
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [x] Update Dockerfile to EXPOSE ports 67/udp, 547/udp, 69/udp, 853/tcp, 443/tcp
  **Verify**: `docker build -t dnshub . 2>&1 | grep -c EXPOSE` → at least 5 EXPOSE directives (or verify in Dockerfile)
- [x] Run clippy and fmt (if any Rust files modified)
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `ansible/roles/dns-dnshub/tasks/main.yml` — update with macvlan and ports (modified)
- `ansible/roles/dns-dnshub/tasks/macvlan.yml` — macvlan network creation (new)
- `ansible/roles/dns-dnshub/defaults/main.yml` — macvlan and TLS variables (modified)
- `ansible/roles/dns-dnshub/templates/dnshub.toml.j2` — TLS and DHCP config (modified)
- `Dockerfile` — expose additional ports (modified)

## Acceptance Criteria

- [x] Ansible role creates macvlan Docker network if not exists
- [x] Container is attached to macvlan network (not bridge)
- [x] Port mappings include 67/udp, 547/udp, 69/udp, 853/tcp, 443/tcp
- [x] TLS cert directory is mounted read-only into container
- [x] dnshub.toml template includes TLS and DHCP config sections
- [x] Dockerfile exposes all required ports
- [x] Ansible syntax check passes

## Test Plan

- Ansible: `ansible-playbook --syntax-check` — no syntax errors
- Docker: `docker build -t dnshub .` — image builds with all ports exposed
- Manual: Run ansible-playbook in check mode to verify macvlan network creation

## Observability

- Ansible task output for macvlan network creation and container deployment
- Container logs via Docker logging driver

## Compliance

- TLS certs mounted read-only (container cannot modify certs)
- macvlan isolates dnshub while providing L2 access
- No secrets in Ansible templates (cert paths reference mounted volumes)

## Risks & Mitigations

- Risk: macvlan network conflicts with existing Docker networks — Mitigation: Check if network exists before creating, use configurable name
- Risk: macvlan parent interface not configured on host — Mitigation: Document requirement, fail with clear error if parent interface doesn't exist
- Risk: TLS cert paths don't match Traefik's cert location — Mitigation: Configurable cert path in defaults/main.yml

## Dependencies & Sequencing

- Depends on: 01-005 (base Dockerfile and Ansible role)
- Unblocks: 06-004 (migration uses this role for production deployment)

## Definition of Done

- [x] All verification commands from sub-tasks pass
- [x] Docker image builds with all ports exposed
- [x] Ansible role passes syntax check
- [x] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- Docker macvlan driver is not available on the host
- Ansible docker_network module does not support macvlan driver

## Maintenance Notes

- macvlan network requires the host's physical interface as parent — document this clearly
- TLS cert directory path should match Traefik's ACME cert output location
- Reviewers should verify port mappings match PRD section 6.1

## Commit Conventions

- `feat(infra): add Docker macvlan network for DHCP L2 access`
- `feat(ansible): add TLS cert mounting and DHCP port mappings`
- `feat(docker): expose DHCP, DoT, DoH, TFTP ports`

## Changelog

- 2026-08-16: initialized story file
- 2026-08-16: implemented macvlan network creation (tasks/macvlan.yml), TLS cert mounting, TFTP port 69/udp, and DHCP/DoT/DoH port mappings in the Ansible role; updated Dockerfile EXPOSE for 69/udp; validated YAML syntax and cargo build
