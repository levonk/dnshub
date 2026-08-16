//! DHCP server (v4 + v6) using `dhcproto`.
//!
//! This module is the root for all DHCP functionality. Phase 04 stories
//! implement the DHCPv4 and DHCPv6 server cores, RA/SLAAC, MAC blocklist,
//! DDNS, PXE, relay, and audit log.
//!
//! - [`v4`] — DHCPv4 server, state machine, pool allocator, options
//!   builder, and SQLite lease store (story 04-001).
//! - [`v6`] — DHCPv6 server, state machine, IA_NA, DUID, options, and
//!   SQLite lease store (story 04-002).
//!
//! ## Architecture
//!
//! ```text
//!  UDP :67 ──► DhcpV4Server ──► DhcpStateMachine ──► LeaseStoreV4
//!                                   │                    │
//!                                   ▼                    ▼
//!                             PoolAllocator        SQLite (WAL)
//!                                   │
//!                                   ▼
//!                             DhcpOptionBuilder ──► dhcproto encode
//!                                   │
//!                                   ▼
//!                              UDP reply
//! ```

pub mod v4;
pub mod v6;
pub mod ra;
pub mod classification;
pub mod mac_blocklist;
pub mod options;
pub mod pool_options;
pub mod ddns;
pub mod pxe;
pub mod tftp;
pub mod relay;
