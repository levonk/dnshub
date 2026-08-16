# dns-dnshub Ansible Role

Deploys the **dnshub** DNS orchestrator as a Docker container. dnshub
replaces AdGuard Home + dnsdist + CoreDNS with a single Rust service
(DNS, DoT, DoH, DHCP, blocklists, per-client policy, metrics, and a
NextJS frontend).

## Requirements

- Ansible >= 2.14
- `community.docker` collection (for `docker_container` and `docker_volume`)
- Docker host with the `localnet-network` network already created
- The `localnet-dns-dnshub` image built and available on the host
  (see the project `Dockerfile` and `build-and-push-images.sh`)

## Role Variables

All variables default to values derived from the `infra_*` inventory
variables defined in the Infrahub inventory (see PRD section 6.2).

### Image / container

| Variable | Default | Description |
|----------|---------|-------------|
| `dnshub_image` | `localnet-dns-dnshub` | Container image name |
| `dnshub_tag` | `latest` | Image tag |
| `dnshub_container_name` | `dnshub` | Container name |
| `dnshub_restart_policy` | `unless-stopped` | Docker restart policy |

### Network

| Variable | Default | Description |
|----------|---------|-------------|
| `dnshub_network` | `localnet-network` | Docker network to attach |
| `dnshub_ipv4_address` | `infra_network_ip_dns_dnshub` (172.20.255.67) | Static IPv4 on the network |

### Ports (PRD 6.1)

| Variable | Default | Port / Protocol |
|----------|---------|-----------------|
| `dnshub_port_dns_host` / `_container` | 53 | DNS UDP/TCP |
| `dnshub_port_dot_host` / `_container` | 853 | DoT (TCP) |
| `dnshub_port_doh_host` / `_container` | 443 | DoH (TCP) |
| `dnshub_port_dhcp_host` / `_container` | 67 | DHCPv4 (UDP) |
| `dnshub_port_dhcpv6_host` / `_container` | 547 | DHCPv6 (UDP) |
| `dnshub_port_metrics_host` / `_container` | 9090 | Prometheus metrics |
| `dnshub_port_frontend_host` / `_container` | 8080 | NextJS frontend + REST API |

### Storage

| Variable | Default | Description |
|----------|---------|-------------|
| `dnshub_data_volume` | `infra_storage_dns_dnshub_volume` | Docker named volume for LMDB/SQLite data |
| `dnshub_config_dir` | `infra_storage_dns_dnshub_config_dir` | Host directory for rendered config files |
| `dnshub_tls_host_dir` | `/srv/traefik/acme` | Host TLS cert dir (mounted read-only) |

### Config

The role renders three TOML config files from Jinja2 templates:

- `dnshub.toml.j2` — main service config (listen addresses, upstreams, cache, rate limit, metrics, frontend)
- `blocklists.toml.j2` — blocklist source subscriptions and LMDB storage
- `policy.toml.j2` — per-client policy (rendered only when `dnshub_policy_enabled` is true; full schema in story 02-001)

All runtime defaults (listen addresses, upstream tiers, cache settings,
blocklist sources) are defined in `defaults/main.yml` and can be overridden
via group/host vars.

## Dependencies

None.

## Example Playbook

```yaml
- hosts: dns_servers
  roles:
    - role: dns-dnshub
      vars:
        dnshub_image: localnet-dns-dnshub
        dnshub_tag: latest
        # Override upstreams for this host:
        dnshub_upstreams:
          - name: unbound-validator
            address: "172.20.255.50:15353"
            protocol: udp
            timeout_ms: 2000
            tier: 1
```

## Notes

- DHCP (port 67/547) requires L2 broadcast access. On a standard Docker
  bridge network this will not work — use macvlan or host networking.
  Macvlan setup is handled in story 04-012.
- TLS certificates are mounted read-only from the Traefik/ACME cert
  directory on the host. No certs are baked into the image.
- No secrets are stored in the image or templates. Sensitive client
  policy mappings should come from Ansible Vault-encrypted host vars.

## License

MIT OR Apache-2.0
