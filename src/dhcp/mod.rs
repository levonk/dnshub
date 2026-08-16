//! DHCP server (v4 + v6) using dhcproto.
//!
//! The full DHCP server (lease state machine, packet handling) is implemented
//! in Phase 04 stories 04-001 through 04-007. This module currently provides
//! the cross-cutting observability pieces that the server calls into:
//!
//! - [`audit`] — lease audit log (SQLite-backed device join/leave events,
//!   story 04-008).
//! - [`rogue`] — rogue DHCP server detection via periodic DHCPDISCOVER probes
//!   (story 04-008).
//!
//! Both submodules are self-contained: they do not depend on the lease state
//! machine and can be exercised independently.

pub mod audit;
pub mod rogue;
