---
story_id: "01-005"
story_title: "Dockerfile + Ansible role"
story_name: "dockerfile-and-ansible-role"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 1
parallel_id: 5
branch: "feature/current/dnshub/story-01-005-dockerfile-and-ansible-role"
status: "todo"
assignee: ""
reviewer: ""
dependencies: []
parallel_safe: true
modules: ["infra", "ansible"]
priority: "SHOULD"
risk_level: "low"
tags: ["feat", "infra", "docker", "ansible"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Create a multi-stage Dockerfile for building the dnshub Rust service and an Ansible role (`dns-dnshub`) for deploying the container. The Dockerfile uses a Rust builder stage and a minimal runtime stage. The Ansible role deploys the container with config templates, volume mounts, and port mappings per PRD section 6.

## Current State

- **Relevant files and their roles:**
  - No files exist yet — greenfield project. This story creates infrastructure files only (no Rust code).
- **Existing code excerpts:** None — greenfield.
- **Repository conventions:** Multi-stage Docker build, multi-arch via buildx. Ansible role follows `dns-` prefix convention. Config templates use Jinja2 (.j2).
- **Tech context (binding constraint from tech-context.txt):**
  - Greenfield Rust service — no existing Cargo.toml or src/ yet
  - Package manager: cargo (Rust)
  - Container runtime: Docker (devcontainer present at .devcontainer/)
  - CI/CD: GitHub Actions (to be set up under .github/workflows/)
  - Config format: TOML (dnshub config)
  - Never use: npm, npx, yarn, jest, biome
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Build   | `cargo build` | exit 0, no errors (if Rust code exists) |
  | Docker  | `docker build -t dnshub .` | image built successfully |
  | Lint    | `cargo clippy -- -D warnings` | exit 0, no warnings |

## Scope

**In scope:**
- Dockerfile — multi-stage build: Rust builder (cargo build --release), minimal runtime (debian:slim or scratch with ca-certificates for LMDB/SQLite)
- .dockerignore — exclude target/, .git, internal-docs/
- ansible/roles/dns-dnshub/defaults/main.yml — infra_* variable references per PRD lines 1657-1684
- ansible/roles/dns-dnshub/handlers/main.yml — restart handler
- ansible/roles/dns-dnshub/meta/main.yml — Galaxy metadata
- ansible/roles/dns-dnshub/tasks/main.yml — docker_container deployment task
- ansible/roles/dns-dnshub/templates/dnshub.toml.j2 — main config template from infra_* vars
- ansible/roles/dns-dnshub/templates/blocklists.toml.j2 — blocklist sources template
- ansible/roles/dns-dnshub/README.md — role documentation

**Out of scope:**
- macvlan Docker network (story 04-012 — DHCP requires L2 access)
- TLS cert mounting (story 04-012)
- policy.toml template (story 02-001)
- GitHub Actions CI/CD pipeline (future)
- Migration from existing stack (story 06-004)

## Sub-Tasks

- [ ] Create Dockerfile with multi-stage build: builder stage using rust:1.95 image, cargo build --release; runtime stage using debian:bookworm-slim with ca-certificates, liblmdb0, libsqlite3-0
  **Verify**: `docker build -t dnshub . 2>&1 | tail -1` → "Successfully tagged dnshub" or equivalent success message
- [ ] Create .dockerignore excluding target/, .git/, internal-docs/, frontend/node_modules/
  **Verify**: `cat .dockerignore` → contains target/, .git/, internal-docs/
- [ ] Create ansible/roles/dns-dnshub/defaults/main.yml with infra_* variable references from PRD lines 1657-1684 (ports, network IP, domain, storage volume, config dir)
  **Verify**: `ansible-playbook --syntax-check --inventory localhost, test-playbook.yml` → no syntax errors (or verify YAML is valid)
- [ ] Create ansible/roles/dns-dnshub/tasks/main.yml with docker_container task: image name, ports (53, 853, 443, 9090, 8080), volumes (config dir, data volume), restart_policy, env vars
  **Verify**: YAML lint or `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create ansible/roles/dns-dnshub/handlers/main.yml with restart handler that restarts the docker_container
  **Verify**: YAML is valid
- [ ] Create ansible/roles/dns-dnshub/templates/dnshub.toml.j2 from PRD dnshub.toml example (lines 1374-1478) with Jinja2 variables for listen addresses, upstream addresses, cache settings
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create ansible/roles/dns-dnshub/templates/blocklists.toml.j2 from PRD blocklists.toml example (lines 1480-1558) with Jinja2 variables for source URLs, refresh intervals
  **Verify: `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create ansible/roles/dns-dnshub/meta/main.yml with Galaxy metadata (author, description, dependencies)
  **Verify**: YAML is valid
- [ ] Create ansible/roles/dns-dnshub/README.md with role description, variables, and usage example
  **Verify**: `cat ansible/roles/dns-dnshub/README.md` → contains role name and usage

## Relevant Files

- `Dockerfile` — multi-stage build for dnshub
- `.dockerignore` — Docker build exclusions
- `ansible/roles/dns-dnshub/defaults/main.yml` — default variables
- `ansible/roles/dns-dnshub/handlers/main.yml` — restart handler
- `ansible/roles/dns-dnshub/meta/main.yml` — Galaxy metadata
- `ansible/roles/dns-dnshub/tasks/main.yml` — deployment tasks
- `ansible/roles/dns-dnshub/templates/dnshub.toml.j2` — main config template
- `ansible/roles/dns-dnshub/templates/blocklists.toml.j2` — blocklist config template
- `ansible/roles/dns-dnshub/README.md` — role documentation

## Acceptance Criteria

- [ ] Dockerfile builds successfully (multi-stage, produces minimal image)
- [ ] Ansible role passes syntax check
- [ ] Config templates use infra_* variables per PRD section 6.2
- [ ] Port mappings match PRD: 53 (DNS), 853 (DoT), 443 (DoH), 9090 (metrics), 8080 (frontend)
- [ ] Volume mounts for config dir and data volume are configured

## Test Plan

- Docker: `docker build -t dnshub .` — image builds successfully
- Ansible: `ansible-playbook --syntax-check` — no syntax errors
- YAML: Validate all YAML files are well-formed
- Manual: Run ansible-playbook in check mode against localhost

## Observability

- Container logs via Docker logging driver (JSON to stdout, picked up by Loki)
- Ansible task output for deployment verification

## Compliance

- No secrets in Dockerfile or Ansible templates (use vault for sensitive data)
- TLS certs mounted read-only from host (not baked into image)

## Risks & Mitigations

- Risk: LMDB C library missing in runtime image — Mitigation: Use debian:bookworm-slim with liblmdb0 package, or statically link via heed
- Risk: Multi-arch build complexity — Mitigation: Use buildx, defer multi-arch to CI pipeline
- Risk: Ansible role structure doesn't match existing conventions — Mitigation: Follow dns- prefix convention per PRD section 6.2

## Dependencies & Sequencing

- Depends on: None (infrastructure files only, no Rust code dependency)
- Unblocks: 04-012 (macvlan + TLS cert Ansible extends this role), 06-004 (migration uses this role)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Docker image builds successfully
- [ ] Ansible role passes syntax check
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- Docker build fails due to missing system libraries for LMDB/SQLite
- Ansible syntax check fails and cannot be resolved

## Maintenance Notes

- The Dockerfile should be updated when new system dependencies are added
- The Ansible role will be extended in story 04-012 for macvlan networking and TLS cert mounting
- Reviewers should verify port mappings and volume mounts match PRD section 6.1

## Commit Conventions

- `feat(infra): add multi-stage Dockerfile for dnshub`
- `feat(ansible): add dns-dnshub role with config templates`

## Changelog

- 2026-08-16: initialized story file
