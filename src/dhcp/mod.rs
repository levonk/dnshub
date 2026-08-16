//! DHCP server (v4 + v6) using dhcproto.
//!
//! The core DHCPv4/v6 server logic is implemented in Phase 04 stories
//! (04-001 through 04-008). This module currently provides the PXE /
//! BOOTP / TFTP network-boot subsystem (story 04-006), which the DHCP
//! server core will integrate into its OFFER/ACK responses.

pub mod pxe;
pub mod tftp;
