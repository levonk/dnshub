# Migration: CoreDNS → dnshub

This guide covers migrating from [CoreDNS](https://coredns.io/) to
dnshub. CoreDNS is used in the existing stack as a policy server; dnshub
replaces it (and the separate AdGuard/dnsdist services) with a single
service that includes per-client policy, blocklists, and tiered upstream
forwarding.

## What Maps to What

| CoreDNS | dnshub |
|---------|--------|
| `Corefile` | `dnshub.toml` + `blocklists.toml` + `policy.toml` |
| Server block header (`.:53 { ... }`) | `[server] listen` |
| `forward . 1.1.1.1 8.8.8.8` | `[[upstreams]]` |
| `cache 60` / `cache { ... }` | `[cache] min_ttl` |
| `tls cert key` | `[server.tls]` |
| `blocklist URL` plugin | `[[sources]]` in `blocklists.toml` |
| `rewrite` / `metadata` / `view` plugins | `policy.toml` `[profiles]` / `[clients]` |
| `prometheus :9153` | `[metrics] listen = "0.0.0.0:9090"` |

## Config Conversion

```bash
python3 scripts/migrate-config.py --source coredns \
    --input /etc/coredns/Corefile \
    --output dnshub.toml \
    --blocklists-out blocklists.toml
```

No PyYAML dependency — the Corefile is a text file parsed with regex.

### What is converted automatically

- **Listen address** — the first server block header (e.g. `.:53 { }`)
  determines `[server] listen`. A bare `.` defaults to `0.0.0.0:53`.
- **Upstream servers** — the `forward` plugin's upstream addresses become
  `[[upstreams]]` tiers. `tls://` prefixes map to TCP; `https://`
  upstreams are skipped with a comment.
- **TLS** — the `tls cert key` plugin directive → `[server.tls]` with
  `enabled = true`.
- **Cache TTL** — `cache N` → `[cache] min_ttl`. Block-style
  `cache { ... }` without a TTL defaults to 60 seconds.
- **Blocklists** — the `blocklist URL` plugin directive becomes
  `[[sources]]` entries in `blocklists.toml`.

### What needs manual attention

- **`rewrite` plugin** — CoreDNS rewrite rules (e.g. name rewriting,
  EDNS0 modifications) are not auto-converted. dnshub's MVP does not
  support query rewriting — file an issue if this is needed.
- **`view` plugin** — CoreDNS's view-based policy (different responses
  for different clients) maps to dnshub's `policy.toml` per-client
  profiles. Convert views manually to `[[clients]]` entries with
  `match` (IP/CIDR) and `profile` references.
- **`metadata` / `geoip` plugins** — not converted. dnshub's policy
  engine is IP/CIDR-based.
- **`hosts` plugin** — CoreDNS's `hosts` plugin (serving static
  records) is not converted. If you have static DNS entries, consider
  adding them as a local zone in dnshub (future feature) or keep a
  small CoreDNS instance for static records.
- **`etcd` / `k8s` plugins** — not applicable to dnshub's use case
  (dnshub is a recursive resolver + filter, not an authoritative
  server for Kubernetes).
- **Multiple server blocks** — the converter only processes the first
  server block. If you have multiple zones, review and merge manually.
- **DoH** — CoreDNS does not have a built-in DoH plugin in the standard
  build. Configure `[server.doh]` in `dnshub.toml` manually.

## IP Reclamation

CoreDNS currently uses `172.20.255.51`. After migration this IP is
**freed** (left unused). dnshub takes over the AdGuard IP
(`172.20.255.67`) as the single DNS endpoint.

## Migration Steps

Follow the general migration procedure in
[`ansible/roles/dns-dnshub/MIGRATION.md`](../../ansible/roles/dns-dnshub/MIGRATION.md).
The CoreDNS-specific notes:

1. **Back up** the `Corefile` before starting.
2. **Convert** the config using `migrate-config.py` to capture upstreams,
   TLS, and blocklist sources.
3. **Review** the generated `dnshub.toml` and `blocklists.toml` —
   especially upstream tiers and any `view`/`rewrite` plugins that need
   manual conversion to `policy.toml`.
4. **Deploy** dnshub on the temp IP alongside the existing stack.
5. **Verify** resolution and policy against dnshub (especially per-client
   policy if you used CoreDNS views).
6. **Switch** the nftables redirect to dnshub.
7. **Stop** CoreDNS — it remains available for rollback for 24h.
8. **Reclaim** the `.51` IP.

## Rollback

See the rollback procedure in
[`MIGRATION.md`](../../ansible/roles/dns-dnshub/MIGRATION.md). To roll
back to CoreDNS: stop dnshub, restore the nftables backup, and start the
`coredns` container.
