# dnshub Grafana Dashboards

Grafana dashboards for monitoring the dnshub DNS server. The dashboards
visualize Prometheus metrics and Loki logs collected by dnshub.

## Dashboards

| File | UID | Description |
|------|-----|-------------|
| `dashboards/dnshub-overview.json` | `dnshub-overview` | Main dashboard: RED method (Rate, Errors, Duration), cache performance, blocklist analytics, per-client policy decisions, tier failover, blocklist daemon health, DHCP leases, DoT/DoH connections, and log streams. |
| `dashboards/dnshub-dhcp.json` | `dnshub-dhcp` | DHCP-specific dashboard: active leases, pool capacity, utilization, allocation/release rates, message types, lease duration distribution, NAK rate, and DHCP logs. |

## Data Sources

The dashboards expect two Grafana data sources, selected via dashboard
variables at the top of each dashboard:

1. **Prometheus** (`${DS_PROMETHEUS}`) — scrapes dnshub's `/metrics`
   endpoint. All `dnshub_*` metrics are exposed here.
2. **Loki** (`${DS_LOKI}`) — collects dnshub structured logs via Promtail
   or Grafana Alloy. Logs are tagged with `job="dnshub"`.

## Provisioning

The file `provisioning/dashboards/dnshub.yml` configures Grafana's file
provisioning to load dashboards from the `dashboards/` directory
automatically.

### Docker Compose

Mount the `grafana/` directory into the Grafana container and point
provisioning at it:

```yaml
services:
  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    volumes:
      - ./grafana/provisioning:/etc/grafana/provisioning
      - ./grafana/dashboards:/var/lib/grafana/dashboards
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
```

Grafana will load both dashboards into the `dnshub` folder on startup.

### Manual Import

If you are not using provisioning:

1. Open Grafana → **Dashboards** → **New** → **Import**.
2. Upload the JSON file from `dashboards/dnshub-overview.json` (or
   `dashboards/dnshub-dhcp.json`).
3. Select the Prometheus and Loki data sources when prompted.
4. Click **Import**.

## Metrics Reference

The dashboards query the following Prometheus metrics. Metrics marked
**(planned)** are defined in the PRD but not yet emitted by the running
service; their panels will show "no data" until the corresponding story
lands.

| Metric | Type | Labels | Story |
|--------|------|--------|-------|
| `dnshub_queries_total` | counter | `client`, `qtype` | 01-003 |
| `dnshub_cache_hits_total` | counter | — | 01-003 |
| `dnshub_cache_misses_total` | counter | — | 01-003 |
| `dnshub_cache_size_entries` | gauge | — | 01-003 |
| `dnshub_cache_hit_ratio` | gauge | — | 01-003 |
| `dnshub_blocklist_hits_total` | counter | `category`, `source` | 02-003 |
| `dnshub_errors_total` | counter | `error_type`, `tier`, `client` | 01-003 |
| `dnshub_policy_decisions_total` | counter | `client`, `profile`, `decision` | 02-003 |
| `dnshub_clients_active` | gauge | — | 02-003 |
| `dnshub_tier_queries_total` | counter | `tier`, `upstream` | 05-001 |
| `dnshub_tier_failures_total` | counter | `tier`, `upstream`, `reason` | 05-001 |
| `dnshub_upstream_latency_seconds` | histogram | `tier` | (planned) |
| `dnshub_blocklist_last_refresh_timestamp` | gauge | `source` | (planned) |
| `dnshub_blocklist_entries_total` | gauge | `source` | (planned) |
| `dnshub_blocklist_refresh_total` | counter | `status` | (planned) |
| `dnshub_dhcp_leases_active` | gauge | — | (planned) |
| `dnshub_dhcp_leases_capacity` | gauge | — | (planned) |
| `dnshub_dhcp_leases_allocated_total` | counter | — | (planned) |
| `dnshub_dhcp_leases_released_total` | counter | — | (planned) |
| `dnshub_dhcp_messages_total` | counter | `type` | (planned) |
| `dnshub_dhcp_nak_total` | counter | — | (planned) |
| `dnshub_dhcp_lease_duration_seconds` | histogram | — | (planned) |
| `dnshub_dot_connections_active` | gauge | — | (planned) |
| `dnshub_doh_connections_active` | gauge | — | (planned) |
| `dnshub_dot_connections_total` | counter | — | (planned) |
| `dnshub_doh_connections_total` | counter | — | (planned) |

## Compatibility

- Grafana schema version 39 (Grafana 10+).
- Prometheus data source with PromQL.
- Loki data source with LogQL.

## Validation

```sh
ruby -rjson -e 'JSON.parse(File.read("grafana/dashboards/dnshub-overview.json")); puts "overview OK"'
ruby -rjson -e 'JSON.parse(File.read("grafana/dashboards/dnshub-dhcp.json")); puts "dhcp OK"'
```
