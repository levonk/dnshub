//! DHCP server (v4 + v6) using dhcproto.
//!
//! This module provides the supporting machinery that the DHCPv4/v6 server
//! cores (stories 04-001, 04-002) call:
//!
//! - [`classification`] — auto-assign clients to policy profiles by vendor
//!   class (option 60), MAC OUI, or user class (option 77).
//! - [`mac_blocklist`] — refuse DHCP to specific MAC addresses or OUI vendor
//!   prefixes, backed by the `dhcp_mac_blocklist` SQLite table.
//! - [`options::arbitrary`] — configurable `option N = value` encoding for
//!   any RFC 2132 option code.
//! - [`pool_options`] — merge global DHCP options with per-pool overrides and
//!   resolve per-profile lease times.
//!
//! The server core itself (wire-level request handling, lease state machine)
//! is implemented in stories 04-001 and 04-002.

pub mod classification;
pub mod mac_blocklist;
pub mod options;
pub mod pool_options;
