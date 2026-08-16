# dns-dnshub Ansible Role

Deploys the **dnshub** DNS orchestrator as a Docker container. dnshub
replaces AdGuard Home + dnsdist + CoreDNS with a single Rust service
(DNS, DoT, DoH, DHCP, blocklists, per-client policy, metrics, and a
NextJS frontend).

## Requirements

- Ansible >= 2.14
- `community.docker` collection (for `docker_container`, `docker_volume`,
  `docker_network`, and `docker_network_info`)
- Docker host with the `macvlan` driver available (DHCP requires L2
  broadcast access; see PRD 4.4)
- A physical interface on the host to use as the macvlan parent
  (`dnshub_macvlan_parent`, default `eth0`)
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

### Network — macvlan (DHCP L2 access)

The container is attached to a **macvlan** network so it has its own MAC
address on the physical LAN. This is required for DHCP (ports 67/547) which
needs L2 broadcast access; Docker bridge networking does not provide this.
The role creates the macvlan network if it does not already exist
(`tasks/macvlan.yml`).

| Variable | Default | Description |
|----------|---------|-------------|
| `dnshub_network` | `infra_dhcp_macvlan_name` (`localnet-macvlan`) | macvlan Docker network name |
| `dnshub_network_driver` | `macvlan` | Docker network driver |
| `dnshub_ipv4_address` | `infra_dhcp_macvlan_ip` (172.20.255.67) | Static IPv4 on the macvlan network |
| `dnshub_macvlan_parent` | `infra_dhcp_macvlan_parent` (`eth0`) | Host physical interface (parent) |
| `dnshub_macvlan_subnet` | `infra_dhcp_macvlan_subnet` (`172.20.255.0/24`) | macvlan subnet |
| `dnshub_macvlan_gateway` | `infra_dhcp_macvlan_gateway` (`172.20.255.1`) | macvlan gateway |
| `dnshub_macvlan_ip_range` | `infra_dhcp_macvlan_ip_range` (`172.20.255.64/28`) | IPAM allocation range |

### Ports (PRD 6.1)

| Variable | Default | Port / Protocol |
|----------|---------|-----------------|
| `dnshub_port_dns_host` / `_container` | 53 | DNS UDP/TCP |
| `dnshub_port_dot_host` / `_container` | 853 | DoT (TCP) |
| `dnshub_port_doh_host` / `_container` | 443 | DoH (TCP) |
| `dnshub_port_dhcp_host` / `_container` | 67 | DHCPv4 (UDP) |
| `dnshub_port_dhcpv6_host` / `_container` | 547 | DHCPv6 (UDP) |
| `dnshub_port_tftp_host` / `_container` | 69 | TFTP / PXE (UDP) |
| `dnshub_port_metrics_host` / `_container` | 9090 | Prometheus metrics |
| `dnshub_port_frontend_host` / `_container` | 8080 | NextJS frontend + REST API |

### Storage

| Variable | Default | Description |
|----------|---------|-------------|
| `dnshub_data_volume` | `infra_storage_dns_dnshub_volume` | Docker named volume for LMDB/SQLite data |
| `dnshub_config_dir` | `infra_storage_dns_dnshub_config_dir` | Host directory for rendered config files |
| `dnshub_tls_host_dir` | `infra_dnshub_tls_cert_dir` (`/srv/traefik/acme`) | Host TLS cert dir (mounted read-only) |
| `dnshub_tls_enabled` | `true` | Enable DoT/DoH TLS listeners |

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

- DHCP (port 67/547) requires L2 broadcast access. The role creates and
  attaches the container to a **macvlan** Docker network
  (`tasks/macvlan.yml`) so dnshub has its own MAC on the physical LAN.
  The host's physical interface must exist as the macvlan parent
  (`dnshub_macvlan_parent`); the role fails with a clear error if it is
  missing.
- TLS certificates are mounted read-only from the Traefik/ACME cert
  directory on the host (`dnshub_tls_host_dir`). No certs are baked into
  the image and no secrets live in the templates.
- No secrets are stored in the image or templates. Sensitive client
  policy mappings should come from Ansible Vault-encrypted host vars.

## License

MIT OR Apache-2.0
