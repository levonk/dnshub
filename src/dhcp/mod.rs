//! DHCP server (v4 + v6) using `dhcproto`.
//!
//! This module is the root for all DHCP functionality. Phase 04 story
//! 04-001 implements the DHCPv4 server core:
//!
//! - [`v4`] — DHCPv4 server, state machine, pool allocator, options
//!   builder, and SQLite lease store.
//!
//! DHCPv6 (story 04-002), RA/SLAAC (04-003), MAC blocklist (04-004),
//! DDNS (04-005), PXE (04-006), relay (04-007), and audit log (04-008)
//! are implemented in later stories.
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
