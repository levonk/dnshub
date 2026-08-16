#!/usr/bin/env python3
"""
migrate-config.py — convert existing DNS stack configs to dnshub.toml.

Reads configuration from AdGuard Home, dnsdist, or CoreDNS and emits a
dnshub.toml (plus a blocklists.toml stub when blocklist sources are found)
that can be dropped into the dnshub config directory and refined.

Supported sources:
  --source adguard   --input AdGuardHome.yaml
  --source dnsdist   --input dnsdist.conf
  --source coredns   --input Corefile

Usage:
  python3 scripts/migrate-config.py \\
      --source adguard \\
      --input /path/to/AdGuardHome.yaml \\
      --output dnshub.toml \\
      --blocklists-out blocklists.toml

The generated config is a starting point — review upstream tiers, TLS
cert paths, and blocklist subscriptions before deploying. See
docs/migration/from-*.md for source-specific notes.

Exit codes:
  0 — config written successfully
  1 — input error (missing file, unsupported source, parse failure)
  2 — usage error
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from typing import Any

try:
    import yaml  # PyYAML
except ImportError:
    yaml = None  # type: ignore[assignment]


# ---------------------------------------------------------------------------
# dnshub.toml emission helpers
# ---------------------------------------------------------------------------

def _toml_bool(value: bool) -> str:
    return "true" if value else "false"


def _toml_str_list(items: list[str]) -> str:
    return "[" + ", ".join(f'"{i}"' for i in items) + "]"


def emit_server(listen_dns: str = "0.0.0.0:53") -> list[str]:
    return [
        "[server]",
        f'listen = ["{listen_dns}", "[::]:53"]',
        'protocol = ["udp", "tcp"]',
        "",
    ]


def emit_tls(cert: str, key: str, enabled: bool = True) -> list[str]:
    return [
        "[server.tls]",
        f"enabled = {_toml_bool(enabled)}",
        'listen = ["0.0.0.0:853", "[::]:853"]',
        f'cert = "{cert}"',
        f'key = "{key}"',
        "",
    ]


def emit_doh(cert: str, key: str, enabled: bool = True, path: str = "/dns-query") -> list[str]:
    return [
        "[server.doh]",
        f"enabled = {_toml_bool(enabled)}",
        'listen = ["0.0.0.0:443", "[::]:443"]',
        f'path = "{path}"',
        f'cert = "{cert}"',
        f'key = "{key}"',
        "",
    ]


def emit_upstreams(upstreams: list[dict[str, Any]]) -> list[str]:
    lines: list[str] = []
    for u in upstreams:
        lines.append("[[upstreams]]")
        lines.append(f'name = "{u["name"]}"')
        lines.append(f'address = "{u["address"]}"')
        lines.append(f'protocol = "{u.get("protocol", "udp")}"')
        lines.append(f'timeout_ms = {u.get("timeout_ms", 2000)}')
        lines.append(f'tier = {u.get("tier", 1)}')
        lines.append("")
    return lines


def emit_cache(
    min_ttl: int = 60,
    max_ttl: int = 86400,
    negative_ttl: int = 300,
    serve_stale: bool = True,
) -> list[str]:
    return [
        "[cache]",
        f"min_ttl = {min_ttl}",
        f"max_ttl = {max_ttl}",
        f"negative_ttl = {negative_ttl}",
        f"serve_stale = {_toml_bool(serve_stale)}",
        f"serve_stale_ttl = 86400",
        "max_entries = 100000",
        "",
    ]


def emit_rate_limit(rps: int = 100, burst: int = 200, per_client: bool = True) -> list[str]:
    return [
        "[rate_limit]",
        f"requests_per_second = {rps}",
        f"burst = {burst}",
        f"per_client = {_toml_bool(per_client)}",
        "",
    ]


def emit_metrics(listen: str = "0.0.0.0:9090", path: str = "/metrics") -> list[str]:
    return [
        "[metrics]",
        f'listen = "{listen}"',
        f'path = "{path}"',
        "",
    ]


def emit_logging(level: str = "info", fmt: str = "json") -> list[str]:
    return [
        "[logging]",
        f'level = "{level}"',
        f'format = "{fmt}"',
        "",
    ]


def emit_query_log(enabled: bool = True) -> list[str]:
    return [
        "[query_log]",
        f"enabled = {_toml_bool(enabled)}",
        "max_entries = 100000",
        "retention_days = 7",
        "",
    ]


def emit_frontend(enabled: bool = True) -> list[str]:
    return [
        "[frontend]",
        f"enabled = {_toml_bool(enabled)}",
        'listen = "0.0.0.0:8080"',
        'static_dir = "/etc/dnshub/frontend"',
        "",
    ]


def emit_blocklists_sources(sources: list[dict[str, Any]]) -> list[str]:
    lines: list[str] = []
    for s in sources:
        lines.append("[[sources]]")
        lines.append(f'name = "{s["name"]}"')
        lines.append(f'url = "{s["url"]}"')
        lines.append(f'format = "{s["format"]}"')
        cats = s.get("categories", [])
        lines.append(f"categories = {_toml_str_list(cats)}")
        if "refresh_minutes" in s:
            lines.append(f'refresh_minutes = {s["refresh_minutes"]}')
        else:
            lines.append(f'refresh_hours = {s.get("refresh_hours", 24)}')
        lines.append("")
    lines.append("[storage]")
    lines.append('type = "lmdb"')
    lines.append('path = "/var/lib/dnshub/blocklists.lmdb"')
    lines.append("bloom_filter = true")
    lines.append("bloom_fpr = 0.001")
    lines.append("")
    return lines


# ---------------------------------------------------------------------------
# AdGuard Home converter
# ---------------------------------------------------------------------------

def convert_adguard(input_path: str) -> tuple[list[str], list[str]]:
    if yaml is None:
        die("AdGuard Home config is YAML — install PyYAML (pip install pyyaml) to use this converter.", code=1)
    with open(input_path, "r", encoding="utf-8") as f:
        data = yaml.safe_load(f)

    lines: list[str] = ["# dnshub.toml — converted from AdGuard Home (AdGuardHome.yaml)", ""]

    # Server / listen
    bind_port = data.get("dns", {}).get("port", 53)
    listen_dns = f"0.0.0.0:{bind_port}"
    lines += emit_server(listen_dns)

    # TLS / DoH — AdGuard does not expose DoT/DoH in the YAML directly;
    # leave defaults for the operator to fill in.
    lines += emit_tls("/etc/dnshub/tls/fullchain.pem", "/etc/dnshub/tls/privkey.pem", enabled=False)
    lines += emit_doh("/etc/dnshub/tls/fullchain.pem", "/etc/dnshub/tls/privkey.pem", enabled=False)

    # Upstreams — AdGuard upstream_dns_servers
    upstream_list: list[dict[str, Any]] = []
    raw_upstreams = data.get("dns", {}).get("upstream_dns", []) or []
    for i, addr in enumerate(raw_upstreams):
        # AdGuard uses URLs like tls://1.1.1.1 or https://dns.google/dns-query
        protocol = "udp"
        clean = addr
        if clean.startswith("tls://"):
            protocol = "tcp"
            clean = clean[len("tls://"):]
        elif clean.startswith("https://") or clean.startswith("quic://"):
            # DoH/DoQ upstreams are not directly supported by dnshub's
            # plain upstream config — skip with a comment.
            lines.append(f"# Skipped AdGuard upstream '{addr}' (DoH/DoQ — configure manually)")
            continue
        # Ensure port
        if ":" not in clean.split("/", 1)[0]:
            clean = f"{clean}:53"
        upstream_list.append({
            "name": f"adguard-upstream-{i+1}",
            "address": clean,
            "protocol": protocol,
            "timeout_ms": 2000,
            "tier": i + 1,
        })
    if not upstream_list:
        upstream_list = [{"name": "cloudflare", "address": "1.1.1.1:53", "protocol": "udp", "timeout_ms": 2000, "tier": 1}]
    lines += emit_upstreams(upstream_list)

    # Cache
    cache_ttl = data.get("dns", {}).get("cache_ttl", 60)
    lines += emit_cache(min_ttl=cache_ttl)

    # Rate limit
    rps = data.get("dns", {}).get("ratelimit", 0) or 0
    if rps:
        lines += emit_rate_limit(rps=rps, burst=rps * 2)
    else:
        lines += emit_rate_limit()

    lines += emit_metrics()
    lines += emit_logging()
    lines += emit_query_log()
    lines += emit_frontend()

    # Blocklists — AdGuard filters
    blocklist_sources: list[dict[str, Any]] = []
    filters = data.get("filters", []) or []
    for flt in filters:
        url = flt.get("url", "")
        if not url:
            continue
        name = flt.get("name", url.split("/")[-1] or "adguard-filter")
        # AdGuard filter lists are typically in hosts/adblock format.
        fmt = "adblock" if url.endswith(".txt") else "hosts"
        blocklist_sources.append({
            "name": name,
            "url": url,
            "format": fmt,
            "categories": ["ads"],
            "refresh_hours": int(flt.get("interval", 24)) if flt.get("interval") else 24,
        })
    # Also user rules
    user_rules = data.get("user_rules", []) or []
    if user_rules:
        lines.append(f"# AdGuard user_rules ({len(user_rules)} entries) — convert manually to policy.toml")
        lines.append("")

    blocklists_lines: list[str] = []
    if blocklist_sources:
        blocklists_lines = ["# blocklists.toml — converted from AdGuard Home filters", ""] + emit_blocklists_sources(blocklist_sources)

    return lines, blocklists_lines


# ---------------------------------------------------------------------------
# dnsdist converter
# ---------------------------------------------------------------------------

def convert_dnsdist(input_path: str) -> tuple[list[str], list[str]]:
    with open(input_path, "r", encoding="utf-8") as f:
        text = f.read()

    lines: list[str] = ["# dnshub.toml — converted from dnsdist (dnsdist.conf)", ""]

    # Listen addresses — newServer / setLocal
    listen_dns = "0.0.0.0:53"
    locals = re.findall(r"setLocal\(['\"]([^'\"]+)['\"]\)", text)
    if locals:
        first = locals[0]
        # dnsdist uses "0.0.0.0:53" format already
        listen_dns = first
    lines += emit_server(listen_dns)

    # TLS
    tls_cert = ""
    tls_key = ""
    doh_cert = ""
    doh_key = ""
    doh_path = "/dns-query"
    doh_enabled = False
    tls_enabled = False
    for m in re.finditer(r"addTLSLocal\(['\"]([^'\"]+)['\"],\s*['\"]([^'\"]+)['\"],\s*['\"]([^'\"]+)['\"]\)", text):
        tls_enabled = True
        tls_cert = m.group(2)
        tls_key = m.group(3)
    for m in re.finditer(r"addDOHLocal\(['\"]([^'\"]+)['\"],\s*['\"]([^'\"]+)['\"],\s*['\"]([^'\"]+)['\"](?:,\s*['\"]([^'\"]+)['\"])?\)", text):
        doh_enabled = True
        doh_cert = m.group(2)
        doh_key = m.group(3)
        if m.group(4):
            doh_path = m.group(4)
    cert = tls_cert or doh_cert or "/etc/dnshub/tls/fullchain.pem"
    key = tls_key or doh_key or "/etc/dnshub/tls/privkey.pem"
    lines += emit_tls(cert, key, enabled=tls_enabled)
    lines += emit_doh(cert, key, enabled=doh_enabled, path=doh_path)

    # Upstreams — newServer("ip:port")
    upstream_list: list[dict[str, Any]] = []
    for i, m in enumerate(re.finditer(r"newServer\(\s*\{?\s*address\s*=\s*['\"]([^'\"]+)['\"]", text)):
        upstream_list.append({
            "name": f"dnsdist-upstream-{i+1}",
            "address": m.group(1),
            "protocol": "udp",
            "timeout_ms": 2000,
            "tier": i + 1,
        })
    # Also newServer("1.2.3.4:53") shorthand
    if not upstream_list:
        for i, m in enumerate(re.finditer(r"newServer\(['\"]([^'\"]+)['\"]\)", text)):
            upstream_list.append({
                "name": f"dnsdist-upstream-{i+1}",
                "address": m.group(1),
                "protocol": "udp",
                "timeout_ms": 2000,
                "tier": i + 1,
            })
    if not upstream_list:
        upstream_list = [{"name": "cloudflare", "address": "1.1.1.1:53", "protocol": "udp", "timeout_ms": 2000, "tier": 1}]
    lines += emit_upstreams(upstream_list)

    # Cache — dnsdist PCache
    cache_ttl_match = re.search(r"setCacheTTL\((\d+)\)", text)
    cache_ttl = int(cache_ttl_match.group(1)) if cache_ttl_match else 60
    lines += emit_cache(min_ttl=cache_ttl)

    # Rate limit
    rl_match = re.search(r"setRLRate\((\d+)", text)
    rps = int(rl_match.group(1)) if rl_match else 100
    lines += emit_rate_limit(rps=rps, burst=rps * 2)

    lines += emit_metrics()
    lines += emit_logging()
    lines += emit_query_log()
    lines += emit_frontend()

    # dnsdist does not carry blocklist subscriptions
    return lines, []


# ---------------------------------------------------------------------------
# CoreDNS converter
# ---------------------------------------------------------------------------

def convert_coredns(input_path: str) -> tuple[list[str], list[str]]:
    with open(input_path, "r", encoding="utf-8") as f:
        text = f.read()

    lines: list[str] = ["# dnshub.toml — converted from CoreDNS (Corefile)", ""]

    # CoreDNS server blocks are separated by curly braces. The first block
    # is usually ". { ... }" (the default zone). Extract upstreams from
    # the forward plugin.
    listen_dns = "0.0.0.0:53"
    # Listen address is the block header before '{'
    block_headers = re.findall(r"^([^\s{]+)\s*\{", text, re.MULTILINE)
    if block_headers:
        first = block_headers[0]
        if ":" in first:
            listen_dns = first
        elif first != ".":
            listen_dns = f"0.0.0.0:{first}"
    lines += emit_server(listen_dns)

    # TLS / DoH — CoreDNS tls plugin
    tls_cert = ""
    tls_key = ""
    tls_match = re.search(r"tls\s+([^\s]+)\s+([^\s]+)", text)
    if tls_match:
        tls_cert = tls_match.group(1)
        tls_key = tls_match.group(2)
    lines += emit_tls(tls_cert or "/etc/dnshub/tls/fullchain.pem", tls_key or "/etc/dnshub/tls/privkey.pem", enabled=bool(tls_cert))

    # Upstreams — forward plugin: forward . 1.1.1.1 8.8.8.8
    # Parse line-by-line so we only capture addresses on the forward line.
    upstream_list: list[dict[str, Any]] = []
    fwd_addrs: list[str] = []
    for line in text.splitlines():
        m = re.match(r"\s*forward\s+(?:\.\s+|\S+\s+)(.+)", line)
        if m:
            # Take tokens up to the first '{' (block open) or end of line.
            rest = m.group(1).split("{")[0].strip()
            fwd_addrs = rest.split()
            break
    for i, addr in enumerate(fwd_addrs):
        protocol = "udp"
        clean = addr
        if clean.startswith("tls://"):
            protocol = "tcp"
            clean = clean[len("tls://"):]
        elif clean.startswith("https://"):
            lines.append(f"# Skipped CoreDNS forward '{addr}' (DoH — configure manually)")
            continue
        if ":" not in clean:
            clean = f"{clean}:53"
        upstream_list.append({
            "name": f"coredns-upstream-{i+1}",
            "address": clean,
            "protocol": protocol,
            "timeout_ms": 2000,
            "tier": i + 1,
        })
    if not upstream_list:
        upstream_list = [{"name": "cloudflare", "address": "1.1.1.1:53", "protocol": "udp", "timeout_ms": 2000, "tier": 1}]
    lines += emit_upstreams(upstream_list)

    # Cache — cache plugin: cache { success 4096 5 } or cache 60
    cache_ttl = 60
    cache_match = re.search(r"cache(?:\s+(\d+))?\s*\{", text)
    if cache_match and cache_match.group(1):
        cache_ttl = int(cache_match.group(1))
    lines += emit_cache(min_ttl=cache_ttl)

    lines += emit_rate_limit()
    lines += emit_metrics()
    lines += emit_logging()
    lines += emit_query_log()
    lines += emit_frontend()

    # Blocklists — CoreDNS blocklist plugin (if present)
    blocklist_sources: list[dict[str, Any]] = []
    for m in re.finditer(r"blocklist\s+([^\s{]+)(?:\s+(\d+)[smh])?", text):
        url = m.group(1)
        blocklist_sources.append({
            "name": url.split("/")[-1] or "coredns-blocklist",
            "url": url,
            "format": "domains",
            "categories": ["ads"],
            "refresh_hours": 24,
        })

    blocklists_lines: list[str] = []
    if blocklist_sources:
        blocklists_lines = ["# blocklists.toml — converted from CoreDNS blocklist plugin", ""] + emit_blocklists_sources(blocklist_sources)

    return lines, blocklists_lines


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

CONVERTERS = {
    "adguard": convert_adguard,
    "dnsdist": convert_dnsdist,
    "coredns": convert_coredns,
}


def die(msg: str, code: int = 1) -> None:
    print(f"migrate-config: error: {msg}", file=sys.stderr)
    sys.exit(code)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Convert AdGuard Home / dnsdist / CoreDNS config to dnshub.toml.",
    )
    parser.add_argument("--source", required=True, choices=sorted(CONVERTERS), help="source stack")
    parser.add_argument("--input", required=True, help="path to the source config file")
    parser.add_argument("--output", default="dnshub.toml", help="output dnshub.toml path")
    parser.add_argument("--blocklists-out", default=None, help="output blocklists.toml path (written only if sources found)")
    args = parser.parse_args(argv)

    if not os.path.isfile(args.input):
        die(f"input file not found: {args.input}", code=1)

    converter = CONVERTERS[args.source]
    try:
        toml_lines, blocklists_lines = converter(args.input)
    except Exception as exc:  # noqa: BLE001
        die(f"failed to parse {args.source} config: {exc}", code=1)

    with open(args.output, "w", encoding="utf-8") as f:
        f.write("\n".join(toml_lines) + "\n")
    print(f"Wrote {args.output} ({len(toml_lines)} lines)")

    if blocklists_lines:
        out = args.blocklists_out or os.path.join(os.path.dirname(args.output) or ".", "blocklists.toml")
        with open(out, "w", encoding="utf-8") as f:
            f.write("\n".join(blocklists_lines) + "\n")
        print(f"Wrote {out} ({len(blocklists_lines)} lines)")
    else:
        print("No blocklist sources detected — blocklists.toml not written.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
