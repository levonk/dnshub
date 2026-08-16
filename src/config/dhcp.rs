//! DHCP-related configuration types re-exported for the config tree.
//!
//! The PXE / BOOTP / TFTP configuration structs live with the runtime
//! handler in [`crate::dhcp::pxe::config`] so the DHCP subsystem owns its
//! own shape. This module re-exports them under `crate::config::dhcp`
//! so the top-level `[dhcp.pxe]` TOML section deserializes cleanly
//! alongside the rest of [`crate::config`].

pub use crate::dhcp::pxe::config::{BootpStaticEntry, IpxeConfig, PxeConfig, TftpConfig};
