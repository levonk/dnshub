//! NextJS static-export frontend assets and serving.
//!
//! The dnshub frontend is a NextJS 15 application (static export mode,
//! `output: 'export'`) that lives in the `frontend/` directory at the
//! repository root. It is built with `pnpm build` (see the frontend
//! `package.json`) and the resulting `out/` directory is served either by
//! the Rust binary (from `/etc/dnshub/frontend/`) or by Traefik.
//!
//! ## Pages (PRD section 4.9)
//!
//! | Route              | Purpose                                      |
//! |--------------------|----------------------------------------------|
//! | `/`                | Dashboard: service health, cache, tiers      |
//! | `/status`          | Detailed service status                      |
//! | `/config`          | Active config viewer/editor                  |
//! | `/dhcp`            | DHCP overview                                |
//! | `/dhcp/leases`     | Active DHCP leases (v4 + v6), release/renew  |
//! | `/dhcp/blocklist`  | MAC blocklist (add/remove)                   |
//! | `/dhcp/audit`      | Lease audit log (filterable)                 |
//! | `/dhcp/pools`      | DHCP pools (local + relayed VLANs)           |
//! | `/dhcp/pxe`        | PXE/BOOTP bootfile mappings + static entries |
//! | `/dhcp/relay`      | Trusted relay agents + Option 82 mappings    |
//! | `/dhcp/rogue`      | Rogue DHCP detection status                  |
//! | `/blocklists`      | Blocklist source health, manual refresh      |
//! | `/query-log`       | Query log viewer (TanStack Table, 2s poll)   |
//!
//! ## API
//!
//! The frontend calls the dnshub REST API (`/api/v1/*`) implemented in
//! story 05-004. The API client lives in `frontend/src/lib/api.ts`. During
//! development the API base URL can be overridden via the
//! `NEXT_PUBLIC_API_BASE_URL` environment variable.
//!
//! This Rust module is intentionally a thin documentation stub: the
//! frontend is a separate Node/pnpm project and is not compiled into the
//! Rust binary. A future story may embed the built `out/` directory via
//! `rust-embed` or `include_dir` so the binary serves the SPA directly.
