---
story_id: "06-004"
story_title: "Migration from existing stack"
story_name: "migration-from-existing-stack"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 6
parallel_id: 4
branch: "feature/current/dnshub/story-06-004-migration-from-existing-stack"
status: "todo"
assignee: ""
reviewer: ""
dependencies: ["01-005", "04-012"]
parallel_safe: true
modules: ["infra", "migration"]
priority: "SHOULD"
risk_level: "high"
tags: ["feat", "infra", "migration", "ansible", "production"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement the migration plan from the existing DNS stack (AdGuard Home + dnsdist + CoreDNS) to dnshub. Follow PRD section 6.3 (lines 1687-1694): deploy dnshub alongside existing stack on a different IP, verify with test client, verify metrics/logs, switch nftables TPROXY redirect, remove old containers, reclaim IPs. Create Ansible migration playbook and verification scripts.

## Current State

- **Relevant files and their roles:**
  - `ansible/roles/dns-dnshub/` — Ansible role for dnshub deployment (from stories 01-005, 04-012)
  - PRD section 6.3 (lines 1687-1694) defines migration plan
  - PRD section 6.1 (lines 1614-1631) defines IP allocation
- **Existing code excerpts:**
  - `ansible/roles/dns-dnshub/tasks/main.yml` — docker_container deployment with macvlan
- **Repository conventions:** Follow PRD migration plan step-by-step. Use Ansible for deployment automation. Verify at each step before proceeding.
- **Tech context (binding constraint from tech-context.txt):**
  - Container runtime: Docker
  - CI/CD: GitHub Actions
  - Config format: TOML
  - Never use: npm, npx, yarn, jest, biome
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Ansible | `ansible-playbook --syntax-check` | no syntax errors |
  | Build   | `cargo build` | exit 0 (if Rust changes needed) |

## Scope

**In scope:**
- ansible/migrate-to-dnshub.yml — migration playbook with steps:
  1. Deploy dnshub on temporary IP (172.20.255.68) alongside existing stack
  2. Configure test client to use dnshub, verify resolution + blocking + policy
  3. Verify metrics in Grafana, logs in Loki
  4. Switch nftables TPROXY redirect from AdGuard (.67) to dnshub
  5. Remove AdGuard, dnsdist, CoreDNS containers
  6. Reclaim IPs (.49, .51) and move dnshub to .67
- ansible/roles/dns-dnshub/tasks/migration.yml — migration tasks: deploy on temp IP, verify, switch redirect, cleanup
- ansible/roles/dns-dnshub/vars/migration.yml — migration variables (temp IP, old service IPs, nftables rules)
- scripts/verify-migration.sh — verification script: test DNS resolution, test blocking, test policy, test metrics endpoint, test query log
- ansible/roles/dns-dnshub/templates/nftables-dnshub.j2 — nftables TPROXY redirect rules for dnshub
- Migration documentation in ansible/roles/dns-dnshub/MIGRATION.md

**Out of scope:**
- Rollback procedure (document but don't automate — manual rollback is safer)
- Data migration from AdGuard (no data to migrate — blocklists are re-fetched, DHCP leases are re-assigned)
- keepalived VRRP reconfiguration (kept as sidecar, no changes)

## Sub-Tasks

- [ ] Create ansible/roles/dns-dnshub/vars/migration.yml with migration variables: temp_ip (172.20.255.68), old_adguard_ip (172.20.255.67), old_dnsdist_ip (172.20.255.49), old_coredns_ip (172.20.255.51)
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create ansible/roles/dns-dnshub/templates/nftables-dnshub.j2 with nftables TPROXY redirect rules pointing to dnshub IP
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create ansible/roles/dns-dnshub/tasks/migration.yml with migration steps: deploy on temp IP, run verification, switch nftables, remove old containers, reclaim IPs
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create ansible/migrate-to-dnshub.yml as the main migration playbook that includes migration tasks with tags for each step (deploy, verify, switch, cleanup)
  **Verify**: `ansible-playbook --syntax-check` → no syntax errors
- [ ] Create scripts/verify-migration.sh: test DNS resolution (dig @dnshub_ip example.com), test blocking (dig @dnshub_ip ads.example.com → REFUSED), test metrics (curl dnshub_ip:9090/metrics), test query log (curl dnshub_ip:8080/api/v1/query-log)
  **Verify**: `bash scripts/verify-migration.sh --help` → shows usage (or verify script is executable)
- [ ] Create ansible/roles/dns-dnshub/MIGRATION.md with step-by-step migration guide, rollback instructions, and verification checklist
  **Verify**: `cat ansible/roles/dns-dnshub/MIGRATION.md` → contains all 6 steps and rollback

## Relevant Files

- `ansible/migrate-to-dnshub.yml` — main migration playbook (new)
- `ansible/roles/dns-dnshub/tasks/migration.yml` — migration tasks (new)
- `ansible/roles/dns-dnshub/vars/migration.yml` — migration variables (new)
- `ansible/roles/dns-dnshub/templates/nftables-dnshub.j2` — nftables rules (new)
- `scripts/verify-migration.sh` — verification script (new)
- `ansible/roles/dns-dnshub/MIGRATION.md` — migration documentation (new)

## Acceptance Criteria

- [ ] Migration playbook passes Ansible syntax check
- [ ] Migration steps match PRD section 6.3 (lines 1687-1694)
- [ ] Verification script tests DNS resolution, blocking, metrics, and query log
- [ ] nftables TPROXY redirect template targets dnshub IP
- [ ] Migration documentation includes rollback instructions
- [ ] Temporary IP (172.20.255.68) used for initial deployment
- [ ] Final step moves dnshub to 172.20.255.67 and reclaims .49, .51

## Test Plan

- Ansible: `ansible-playbook --syntax-check` — no syntax errors
- Script: `bash scripts/verify-migration.sh --help` — shows usage
- Manual: Run migration playbook in check mode (--check --diff) to verify dry-run

## Observability

- Migration playbook logs each step
- Verification script outputs pass/fail for each check
- Post-migration: verify Grafana dashboard shows dnshub metrics

## Compliance

- Migration is a high-risk operation — document rollback procedure
- No data migration needed (blocklists re-fetched, DHCP leases re-assigned)
- Old containers are removed only after verification passes

## Risks & Mitigations

- Risk: Migration breaks existing DNS — Mitigation: Deploy alongside on temp IP, verify before switching, document rollback
- Risk: DHCP lease disruption during migration — Mitigation: Clients renew leases automatically; dnshub starts serving DHCP immediately
- Risk: nftables TPROXY redirect error — Mitigation: Test redirect rules before applying, keep old rules as backup

## Dependencies & Sequencing

- Depends on: 01-005 (base Ansible role), 04-012 (macvlan + TLS Ansible)
- Unblocks: None (this is the final production deployment story)

## Definition of Done

- [ ] All verification commands from sub-tasks pass
- [ ] Migration playbook passes syntax check
- [ ] Verification script is functional
- [ ] Migration documentation is complete with rollback
- [ ] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- Ansible migration playbook has syntax errors that cannot be resolved
- nftables TPROXY redirect rules do not match existing infrastructure patterns

## Maintenance Notes

- Migration should be performed during a maintenance window
- Keep old containers stopped (not removed) for 24h as rollback safety
- Reviewers should verify migration steps match PRD section 6.3 exactly
- Document the rollback procedure: stop dnshub, restore nftables redirect to AdGuard, start old containers

## Commit Conventions

- `feat(infra): add migration playbook from existing DNS stack`
- `feat(infra): add nftables TPROXY redirect for dnshub`
- `feat(infra): add migration verification script`
- `docs(infra): add migration guide with rollback instructions`

## Changelog

- 2026-08-16: initialized story file
