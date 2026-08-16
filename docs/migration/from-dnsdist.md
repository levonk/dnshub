# Migration: dnsdist → dnshub

This guide covers migrating from [dnsdist](https://dnsdist.org/) to
dnshub. dnsdist is a DNS load balancer that sits in front of upstream
resolvers; dnshub replaces it (and the separate AdGuard/CoreDNS
services) with a single service that includes caching, blocklists, and
tiered upstream forwarding.

## What Maps to What

| dnsdist | dnshub |
|---------|--------|
| `dnsdist.conf` | `dnshub.toml` |
| `setLocal("0.0.0.0:53")` | `[server] listen` |
| `newServer({address="..."})` / `newServer("...")` | `[[upstreams]]` |
| `addTLSLocal(...)` | `[server.tls]` |
| `addDOHLocal(...)` | `[server.doh]` |
| `setCacheTTL(N)` | `[cache] min_ttl` |
| `setRLRate(N)` | `[rate_limit] requests_per_second` |
| ACL / `setACL()` | (not needed — dnshub serves all by default; use policy.toml for restrictions) |
| `addAction(DNSAction.Pool, ...)` routing | `[[upstreams]] tier` ordered fallback |

## Config Conversion

```bash
python3 scripts/migrate-config.py --source dnsdist \
    --input /etc/dnsdist/dnsdist.conf \
    --output dnshub.toml
```

No PyYAML dependency — dnsdist config is a text file parsed with regex.

### What is converted automatically

- **Listen address** — `setLocal("...")` → `[server] listen`.
- **Upstream servers** — `newServer({address="..."})` and
  `newServer("...")` forms are both parsed. Each becomes an
  `[[upstreams]]` entry with an incrementing tier.
- **TLS listener** — `addTLSLocal(addr, cert, key)` → `[server.tls]`
  with `enabled = true`.
- **DoH listener** — `addDOHLocal(addr, cert, key, path)` →
  `[server.doh]` with `enabled = true`.
- **Cache TTL** — `setCacheTTL(N)` → `[cache] min_ttl`.
- **Rate limit** — `setRLRate(N)` → `[rate_limit] requests_per_second`.

### What needs manual attention

- **Server pools** — dnsdist's pool-based routing
  (`newServer({pool="recursors"})` + `addAction(PoolAction(...))`) is
  more flexible than dnshub's ordered tier fallback. Map pools to tiers
  in order of preference, or use `policy.toml` to route specific clients
  to specific tiers.
- **Dynamic blocks / PBK** — dnsdist's dynamic blocking (e.g.
  `dynBlockRulesGroup`) is not auto-converted. Use dnshub's rate limiting
  (`[rate_limit]`) and blocklists instead.
- **Snippets / Lua** — dnsdist Lua snippets are not converted. Replicate
  custom logic using dnshub's policy engine (`policy.toml`).
- **Blocklists** — dnsdist does not carry blocklist subscriptions (it is
  a load balancer, not a filter). Blocklists come from AdGuard Home or
  CoreDNS in the existing stack. See
  [`from-adguard-home.md`](from-adguard-home.md) or
  [`from-coredns.md`](from-coredns.md).
- **Health checks** — dnsdist's `checkName`/`checkInterval` upstream
  health checks have no direct equivalent in dnshub's MVP. dnshub
  handles upstream failure via tiered fallback and retry logic.

## IP Reclamation

dnsdist currently uses `172.20.255.49`. After migration this IP is
**freed** (left unused). dnshub takes over the AdGuard IP
(`172.20.255.67`) as the single DNS endpoint.

## Migration Steps

Follow the general migration procedure in
[`ansible/roles/dns-dnshub/MIGRATION.md`](../../ansible/roles/dns-dnshub/MIGRATION.md).
The dnsdist-specific notes:

1. **Back up** `dnsdist.conf` before starting.
2. **Convert** the config using `migrate-config.py` to capture upstreams
   and TLS settings.
3. **Review** the generated `dnshub.toml` — especially upstream tiers
   and pool-to-tier mapping.
4. **Deploy** dnshub on the temp IP alongside the existing stack.
5. **Verify** resolution against dnshub.
6. **Switch** the nftables redirect to dnshub.
7. **Stop** dnsdist — it remains available for rollback for 24h.
8. **Reclaim** the `.49` IP.

## Rollback

See the rollback procedure in
[`MIGRATION.md`](../../ansible/roles/dns-dnshub/MIGRATION.md). To roll
back to dnsdist: stop dnshub, restore the nftables backup, and start the
`dnsdist` container.
