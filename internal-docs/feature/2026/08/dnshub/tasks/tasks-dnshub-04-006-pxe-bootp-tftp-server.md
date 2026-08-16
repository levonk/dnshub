---
story_id: "04-006"
story_title: "PXE/BOOTP/TFTP server (network boot, per-arch bootfile, iPXE)"
story_name: "pxe-bootp-tftp-server"
prd_name: "dnshub"
prd_file: "internal-docs/feature/2026/08/dnshub/feat-202608110000-dnshub.md"
phase: 4
parallel_id: 6
branch: "feature/current/dnshub/story-04-006-pxe-bootp-tftp-server"
status: "done"
assignee: ""
reviewer: ""
dependencies: ["01-004"]
parallel_safe: true
modules: ["dhcp", "pxe", "tftp"]
priority: "COULD"
risk_level: "high"
tags: ["feat", "backend", "dhcp", "pxe", "bootp", "tftp", "network-boot"]
due: "2026-09-15"
created_at: "2026-08-16"
updated_at: "2026-08-16"
---

## Summary

Implement PXE/BOOTP network boot support: parse PXE DHCP options (option 60 "PXEClient", options 93/94/97 for client architecture/UUID, options 66/67/150 for TFTP server/bootfile), per-architecture bootfile mapping (BIOS vs UEFI vs ARM), built-in TFTP server (RFC 1350, read-only, file allowlist), iPXE chainloading support, PXE proxy mode, and static BOOTP entries.

## Current State

- **Relevant files and their roles:**
  - `src/config/dhcp.rs` — DhcpConfig (from story 01-004), to be expanded with PXE config
  - PRD lines 450-471, 857-886 define PXE/BOOTP/TFTP scope
- **Existing code excerpts:**
  - `src/config/dhcp.rs` — DhcpConfig stub
- **Repository conventions:** Use dhcproto for DHCP option parsing. TFTP server uses tokio UDP with RFC 1350 protocol. BOOTP is treated as infinite-lease DHCP.
- **Tech context (binding constraint from tech-context.txt):**
  - Package manager: cargo (Rust)
  - Build: `cargo build` | Test: `cargo test` | Lint: `cargo clippy` | Format: `cargo fmt`
  - Ad-hoc runner: `cargo add` / `cargo binstall -y`
  - Never use: npm, npx, yarn, jest, biome
  - Rust toolchain: cargo 1.95.0 on PATH
- **Build/test/lint commands:**
  | Purpose | Command | Expected Result |
  |---------|---------|-----------------|
  | Build   | `cargo build` | exit 0, no errors |
  | Tests   | `cargo test` | all pass |
  | Lint    | `cargo clippy -- -D warnings` | exit 0, no warnings |
  | Format  | `cargo fmt -- --check` | exit 0, no changes needed |

## Scope

**In scope:**
- src/dhcp/pxe/mod.rs — PxeHandler: detect PXE requests (option 60 = "PXEClient"), select bootfile based on option 93 (architecture), add PXE options to DHCP responses
- src/dhcp/pxe/bootfile.rs — BootfileMapper: map architecture codes (0=BIOS, 7=UEFI x64, 9=UEFI HTTP, 11=ARM64 UEFI) to bootfile names per config
- src/dhcp/pxe/ipxe.rs — iPXE chainloading: detect iPXE client, return chain URL for HTTP-based boot
- src/dhcp/pxe/bootp.rs — BOOTP static entries: MAC → IP → bootfile mapping, treat as infinite-lease DHCP per PRD line 451-453
- src/dhcp/pxe/proxy.rs — PXE proxy mode: provide PXE info without address allocation (for mixed environments)
- src/dhcp/tftp/mod.rs — TFTP server: RFC 1350, read-only, tokio UDP on port 69, configurable root directory and file allowlist
- src/dhcp/tftp/protocol.rs — TFTP protocol: RRQ (read request), DATA, ACK, ERROR packets
- src/dhcp/pxe/config.rs — PxeConfig: enabled, proxy_mode, tftp (listen, root_dir, allowlist), bootfiles (map of arch code to filename), ipxe (enabled, chain_url), bootp_static entries per PRD lines 857-886
- src/config/dhcp.rs — expand with [dhcp.pxe] section
- Unit tests for PXE option parsing, bootfile mapping, TFTP protocol

**Out of scope:**
- DHCPv4/v6 server core (stories 04-001, 04-002 — this story provides PXE options that the DHCP server adds)
- TFTP write support (read-only only)
- TFTP blocksize negotiation (RFC 2348) — future optimization

## Sub-Tasks

- [x] Create src/dhcp/pxe/config.rs with PxeConfig: enabled, proxy_mode (bool), tftp (TftpConfig: enabled, listen, root_dir, allowlist), bootfiles (HashMap<String, String>), ipxe (IpxeConfig: enabled, chain_url), bootp_static (Vec<BootpStaticEntry>) per PRD lines 857-886
  **Verify**: `cargo build` → exit 0
- [x] Create src/dhcp/pxe/bootfile.rs with BootfileMapper: select_bootfile(arch_code: u16) -> Option<String>, map architecture codes (0, 7, 9, 11) to bootfile names from config
  **Verify**: `cargo test --lib dhcp::pxe::bootfile` → all pass (BIOS→pxelinux.0, UEFI→grubx64.efi, unknown→None)
- [x] Create src/dhcp/pxe/mod.rs with PxeHandler: is_pxe_request(options) -> bool (check option 60 = "PXEClient"), build_pxe_response(options, bootfile, tftp_server) -> DhcpOptions (options 66, 67, 150)
  **Verify**: `cargo test --lib dhcp::pxe` → all pass (detect PXE request, build response with correct options)
- [x] Create src/dhcp/pxe/ipxe.rs with iPXE chainloading: detect iPXE client (option 175), return chain_url in bootfile option
  **Verify**: `cargo test --lib dhcp::pxe::ipxe` → all pass (detect iPXE, return chain URL)
- [x] Create src/dhcp/pxe/bootp.rs with BOOTP static: lookup_bootp_static(mac) -> Option<(ip, bootfile, tftp_server)>, treat as infinite-lease DHCP
  **Verify**: `cargo test --lib dhcp::pxe::bootp` → all pass (lookup by MAC, return IP and bootfile)
- [x] Create src/dhcp/pxe/proxy.rs with PXE proxy mode: provide PXE options without IP allocation (proxy_mode = true)
  **Verify**: `cargo test --lib dhcp::pxe::proxy` → all pass (proxy response has PXE options but no IP lease)
- [x] Create src/dhcp/tftp/protocol.rs with TFTP packet types: RRQ, DATA, ACK, ERROR, encode/decode per RFC 1350
  **Verify**: `cargo test --lib dhcp::tftp::protocol` → all pass (encode/decode each packet type)
- [x] Create src/dhcp/tftp/mod.rs with TftpServer: UDP listener on port 69, handle RRQ (read request), serve files from root_dir, enforce allowlist, send DATA blocks, handle ACK, handle ERROR
  **Verify**: `cargo test --lib dhcp::tftp` → all pass (serve a test file, verify DATA blocks sent)
- [x] Update src/config/dhcp.rs with [dhcp.pxe] section
  **Verify**: `cargo build` → exit 0
- [x] Run clippy and fmt
  **Verify**: `cargo clippy -- -D warnings && cargo fmt -- --check` → exit 0

## Relevant Files

- `src/dhcp/pxe/mod.rs` — PxeHandler (new)
- `src/dhcp/pxe/bootfile.rs` — BootfileMapper (new)
- `src/dhcp/pxe/ipxe.rs` — iPXE chainloading (new)
- `src/dhcp/pxe/bootp.rs` — BOOTP static entries (new)
- `src/dhcp/pxe/proxy.rs` — PXE proxy mode (new)
- `src/dhcp/pxe/config.rs` — PxeConfig (new)
- `src/dhcp/tftp/mod.rs` — TFTP server (new)
- `src/dhcp/tftp/protocol.rs` — TFTP protocol (new)
- `src/config/dhcp.rs` — expand with PXE config (modified)

## Acceptance Criteria

- [x] PXE requests (option 60 = "PXEClient") are detected
- [x] Bootfile is selected based on client architecture (option 93)
- [x] PXE response includes options 66 (TFTP server), 67 (bootfile), 150 (TFTP server address)
- [x] iPXE chainloading returns chain URL for HTTP-based boot
- [x] BOOTP static entries (MAC → IP → bootfile) work
- [x] PXE proxy mode provides PXE info without IP allocation
- [x] TFTP server serves files from configured root directory
- [x] TFTP server enforces file allowlist
- [x] TFTP protocol (RRQ, DATA, ACK, ERROR) works per RFC 1350
- [x] All tests pass, clippy clean, fmt clean

## Test Plan

- Unit: `cargo test --lib dhcp::pxe` and `cargo test --lib dhcp::tftp` — tests for PXE handling and TFTP protocol
- Lint: `cargo clippy -- -D warnings` — zero warnings
- Format: `cargo fmt -- --check` — no formatting changes needed
- Manual: Start TFTP server, use `tftp` client to fetch a test file

## Observability

- Log PXE boot requests (client MAC, architecture, bootfile)
- Log TFTP file transfers (filename, client IP, success/failure)

## Compliance

- TFTP server is read-only — no write access
- File allowlist prevents unauthorized file access

## Risks & Mitigations

- Risk: TFTP protocol implementation complexity (block numbering, retransmission) — Mitigation: Implement basic RFC 1350, defer options (blksize, timeout) to future
- Risk: PXE option parsing varies by client implementation — Mitigation: Support standard options (93, 94, 97, 66, 67, 150), test with common PXE clients
- Risk: TFTP server security (path traversal) — Mitigation: Enforce allowlist, sanitize file paths, reject paths with `..`

## Dependencies & Sequencing

- Depends on: 01-004 (config loading)
- Unblocks: None directly

## Definition of Done

- [x] All verification commands from sub-tasks pass
- [x] Code, tests, docs updated; CI green
- [x] No files outside in-scope list are modified (`git status`)

## STOP Conditions

Stop and report if:
- TFTP protocol implementation cannot serve files correctly
- PXE option encoding does not match what real PXE clients expect

## Maintenance Notes

- TFTP server should be read-only with strict allowlist
- Consider adding TFTP blocksize negotiation (RFC 2348) for performance in future
- Reviewers should verify path traversal protection in TFTP server

## Commit Conventions

- `feat(dhcp): add PXE handler with per-architecture bootfile mapping`
- `feat(dhcp): add iPXE chainloading and BOOTP static entries`
- `feat(dhcp): add TFTP server per RFC 1350`
- `feat(dhcp): add PXE proxy mode`

## Changelog

- 2026-08-16: initialized story file
- 2026-08-16: implemented PXE/BOOTP/TFTP server — PxeHandler with per-architecture bootfile mapping (option 93), iPXE chainloading (option 175/user-class), BOOTP static entries, PXE proxy mode, RFC 1350 TFTP server (read-only, allowlist, path-traversal protection), PxeConfig wired into [dhcp.pxe] config section. 71 unit tests added, all passing.
