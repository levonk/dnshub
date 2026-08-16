# Migration: AdGuard Home → dnshub

This guide covers migrating from [AdGuard Home](https://adguard.com/adguard-home.html)
to dnshub. AdGuard Home provides DNS filtering, DHCP, and a web UI; dnshub
replaces it with a single Rust service offering DNS, DoT, DoH, DHCP,
blocklists, per-client policy, metrics, and a NextJS frontend.

## What Maps to What

| AdGuard Home | dnshub |
|--------------|--------|
| `AdGuardHome.yaml` | `dnshub.toml` + `blocklists.toml` + `policy.toml` |
| `dns.upstream_dns` | `[[upstreams]]` in `dnshub.toml` |
| `dns.port` | `[server] listen` |
| `dns.cache_ttl` | `[cache] min_ttl` |
| `dns.ratelimit` | `[rate_limit] requests_per_second` |
| `filters[]` (subscription URLs) | `[[sources]]` in `blocklists.toml` |
| `user_rules[]` (custom block/allow) | `policy.toml` `[profiles]` / `[clients]` |
| Web UI (port 3000) | NextJS frontend (port 8080) |
| Built-in DHCP | `[dhcp]` in `dnshub.toml` |

## Config Conversion

```bash
python3 scripts/migrate-config.py --source adguard \
    --input /opt/AdGuardHome/conf/AdGuardHome.yaml \
    --output dnshub.toml \
    --blocklists-out blocklists.toml
```

> **Requires PyYAML** (`pip install pyyaml`). The AdGuard Home config is
> YAML; the converter uses PyYAML to parse it.

### What is converted automatically

- **Upstream DNS servers** — `dns.upstream_dns` entries become
  `[[upstreams]]` tiers. `tls://` prefixes are mapped to TCP protocol;
  `https://` and `quic://` upstreams are skipped with a comment (configure
  them manually as dnshub uses plain UDP/TCP upstream tiers).
- **Cache TTL** — `dns.cache_ttl` → `[cache] min_ttl`.
- **Rate limit** — `dns.ratelimit` → `[rate_limit] requests_per_second`
  (burst is set to 2× rps).
- **Filter subscriptions** — `filters[].url` entries become
  `[[sources]]` in `blocklists.toml`. List format is inferred from the
  URL extension (`.txt` → `adblock`, otherwise `hosts`).

### What needs manual attention

- **Custom user rules** — AdGuard's `user_rules` (e.g. `||example.com^`,
  `@@||example.com^`) are not auto-converted. Map allow-rules to
  `policy.toml` profiles with `allow_domains` and block-rules to
  `block_categories` or custom blocklist sources.
- **DoT/DoH listeners** — AdGuard Home does not expose DoT/DoH in the
  YAML config directly. Configure `[server.tls]` and `[server.doh]` in
  `dnshub.toml` with your TLS cert/key paths.
- **DHCP settings** — AdGuard's DHCP config (`dhcp` section) should be
  manually transcribed to the `[dhcp]` table in `dnshub.toml`.
- **Client settings** — AdGuard's per-client overrides map to
  `policy.toml` `[clients]` entries.
- **TLS certificates** — dnshub mounts certs from the host (Traefik/ACME
  by default). Point `[server.tls] cert`/`key` to your cert paths.
- **Query log** — AdGuard logs to its own SQLite; dnshub has a built-in
  SQLite query log ring buffer (`[query_log]`). No data migration needed.

## Blocklists

AdGuard Home's filter subscriptions are converted to `blocklists.toml`
sources. dnshub re-fetches all blocklists on startup and on their refresh
schedule — there is no need to migrate the compiled blocklist database.

If you used AdGuard's custom filter lists, add them as additional
`[[sources]]` entries with the appropriate `format` (`adblock`, `hosts`,
or `domains`).

## DHCP

AdGuard Home's DHCP leases are **not migrated** — clients renew leases
automatically, and dnshub starts handing out new leases immediately.
Static DHCP assignments should be manually added to `policy.toml` or the
dnshub DHCP static-lease config.

## Migration Steps

Follow the general migration procedure in
[`ansible/roles/dns-dnshub/MIGRATION.md`](../../ansible/roles/dns-dnshub/MIGRATION.md).
The AdGuard-specific notes:

1. **Back up** `AdGuardHome.yaml` and the AdGuard data directory before
   starting.
2. **Convert** the config using `migrate-config.py`.
3. **Review** the generated `dnshub.toml` — especially upstreams, TLS,
   and DHCP.
4. **Deploy** dnshub on the temp IP (`172.20.255.68`) alongside AdGuard.
5. **Verify** resolution and blocking against dnshub.
6. **Switch** the nftables redirect from AdGuard (`.67`) to dnshub.
7. **Stop** AdGuard — it remains available for rollback for 24h.
8. **Move** dnshub to `.67`.

## Rollback

See the rollback procedure in
[`MIGRATION.md`](../../ansible/roles/dns-dnshub/MIGRATION.md). To roll
back to AdGuard: stop dnshub, restore the nftables backup, and start the
`adguardhome` container.
