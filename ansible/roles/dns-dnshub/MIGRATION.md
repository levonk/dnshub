# Migration Guide: AdGuard Home + dnsdist + CoreDNS → dnshub

This document describes the step-by-step migration from the existing DNS
stack (AdGuard Home + dnsdist + CoreDNS) to dnshub, following PRD section
6.3 (lines 1687-1694). The migration is automated by the Ansible playbook
`ansible/migrate-to-dnshub.yml` and the role tasks in
`tasks/migrate.yml`.

> **High-risk operation.** Perform during a maintenance window. Keep the
> old containers stopped (not removed) for 24 hours as a rollback safety
> net.

## Overview

| Component | Old | New |
|-----------|-----|-----|
| DNS resolver + blocker | AdGuard Home (172.20.255.67) | dnshub (172.20.255.67) |
| DNS load balancer | dnsdist (172.20.255.49) | — (freed) |
| DNS policy server | CoreDNS (172.20.255.51) | — (freed) |
| Temporary dnshub IP | — | 172.20.255.68 (parallel deploy) |

dnshub consolidates all three functions into a single Rust service: DNS
(UDP/TCP), DoT, DoH, DHCP, blocklists, per-client policy, metrics, and a
web frontend.

## Prerequisites

- Ansible >= 2.14 with the `community.docker` collection installed.
- The `localnet-dns-dnshub` Docker image built and present on the target
  host (see the project `Dockerfile` and `build-and-push-images.sh`).
- The `localnet-network` Docker network exists.
- `dig`, `curl`, `nft` available on the target host.
- The target host is in the `dns_servers` inventory group.
- A maintenance window scheduled (expect ~30 minutes of DNS downtime
  during the cutover step).

## Migration Steps (PRD 6.3)

### Step 1 — Deploy dnshub on a temporary IP

dnshub is deployed alongside the existing stack on `172.20.255.68` so the
old services keep serving while dnshub is verified.

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory \
    --tags precheck,deploy
```

The `precheck` tag runs the pre-migration check script
(`migration-check.sh.j2`) which verifies: Docker is reachable, the
network exists, the image is present, old containers are running, the
temp IP is free, and nftables is available.

The `deploy` tag runs the `dns-dnshub` role with
`dnshub_ipv4_address` overridden to the temp IP.

### Step 2 — Verify resolution, blocking, and policy

Point a test client at `172.20.255.68` and run the verification script:

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory \
    --tags verify
```

The verification script (`scripts/verify-migration.sh`) checks:
1. **DNS resolution** — `dig @172.20.255.68 example.com` → `NOERROR`
2. **DNS blocking** — `dig @172.20.255.68 ads.example.com` → `REFUSED`
3. **Metrics** — `curl http://172.20.255.68:9090/metrics` → HTTP 200
4. **Query log** — `curl http://172.20.255.68:8080/api/v1/query-log` → HTTP 200

You can also run the script directly:

```bash
scripts/verify-migration.sh --host 172.20.255.68
```

### Step 3 — Verify metrics in Grafana, logs in Loki

- Confirm the dnshub Prometheus scrape target is up in Grafana.
- Confirm dnshub query logs are flowing to Loki.
- Confirm the dnshub dashboard shows cache hit/miss, blocklist hits, and
  per-client metrics.

### Step 4 — Switch nftables TPROXY redirect

The existing nftables TPROXY rule redirects client DNS traffic to
AdGuard on `.67`. This step:

1. Backs up the current nftables ruleset to
   `/etc/nftables.d/adguard-tproxy.backup.nft`.
2. Renders the dnshub TPROXY ruleset (`nftables-dnshub.j2`) targeting
   the temp IP.
3. Loads the new ruleset with `nft -f`.
4. Verifies DNS still resolves after the switch.

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory \
    --tags switch
```

### Step 5 — Stop old containers

AdGuard, dnsdist, and CoreDNS containers are **stopped** (not removed)
so they can be restarted for rollback:

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory \
    --tags cleanup
```

### Step 6 — Move dnshub to the final IP and reclaim old IPs

dnshub is stopped, redeployed on `172.20.255.67` (the former AdGuard
IP), and the nftables redirect is re-rendered to target the final IP.
The `.49` (dnsdist) and `.51` (CoreDNS) IPs are reclaimed (left unused).

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory \
    --tags reclaim
```

A final verification run confirms dnshub is serving correctly on `.67`.

## Full Migration (all steps)

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory
```

## Dry Run

```bash
ansible-playbook ansible/migrate-to-dnshub.yml -i inventory \
    --check --diff
```

## Rollback Procedure

Rollback is **manual** (intentionally — automated rollback of a
high-risk operation is riskier than a deliberate manual revert).

### If cutover failed (after step 4, before step 6)

1. Restore the old nftables redirect:
   ```bash
   nft -f /etc/nftables.d/adguard-tproxy.backup.nft
   ```
2. Restart the old containers:
   ```bash
   docker start adguardhome dnsdist coredns
   ```
3. Stop dnshub:
   ```bash
   docker stop dnshub
   ```
4. Verify DNS is working via AdGuard again:
   ```bash
   dig @172.20.255.67 example.com
   ```

### If dnshub is on the final IP (.67) and needs rollback

1. Stop dnshub: `docker stop dnshub`
2. Restore nftables: `nft -f /etc/nftables.d/adguard-tproxy.backup.nft`
3. Start AdGuard: `docker start adguardhome`
   - Note: AdGuard was on `.67` — if dnshub took `.67`, you must first
     stop dnshub (done in step 1) so AdGuard can reclaim the IP.
4. Verify: `dig @172.20.255.67 example.com`

### Rollback safety window

Old containers are stopped (not removed) for **24 hours** after
migration. After 24 hours of stable dnshub operation, remove them
manually:

```bash
docker rm adguardhome dnsdist coredns
```

## Verification Checklist

Use this checklist after the migration is complete:

- [ ] `dig @172.20.255.67 example.com` → `NOERROR` with an A record
- [ ] `dig @172.20.255.67 ads.example.com` → `REFUSED`
- [ ] `curl http://172.20.255.67:9090/metrics` → HTTP 200, Prometheus format
- [ ] `curl http://172.20.255.67:8080/api/v1/query-log` → HTTP 200, JSON
- [ ] Grafana dashboard shows dnshub metrics (query rate, cache hit/miss, blocklist hits)
- [ ] Loki shows dnshub query logs
- [ ] A test client using `.67` as its DNS server resolves and blocks correctly
- [ ] DHCP leases are being handed out (if DHCP is enabled)
- [ ] DoT (`dig @172.20.255.67 +tls example.com`) resolves
- [ ] DoH (`curl -H 'accept: application/dns-json' https://172.20.255.67/dns-query?name=example.com`) resolves
- [ ] Old containers are stopped: `docker ps -a | grep -E 'adguardhome|dnsdist|coredns'` shows `Exited`
- [ ] `.49` and `.51` IPs are free on the network

## Config Conversion

Use `scripts/migrate-config.py` to convert existing configs to
`dnshub.toml` as a starting point:

```bash
# From AdGuard Home
python3 scripts/migrate-config.py --source adguard \
    --input /opt/AdGuardHome/conf/AdGuardHome.yaml \
    --output dnshub.toml --blocklists-out blocklists.toml

# From dnsdist
python3 scripts/migrate-config.py --source dnsdist \
    --input /etc/dnsdist/dnsdist.conf \
    --output dnshub.toml

# From CoreDNS
python3 scripts/migrate-config.py --source coredns \
    --input /etc/coredns/Corefile \
    --output dnshub.toml --blocklists-out blocklists.toml
```

Review the generated config — upstream tiers, TLS cert paths, and
blocklist subscriptions should be verified before deploying. See
`docs/migration/from-*.md` for source-specific conversion notes.

## Variables

See `vars/migration.yml` for all migration variables and their defaults.
Override via inventory group/host vars as needed.
