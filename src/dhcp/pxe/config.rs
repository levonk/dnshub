//! PXE / BOOTP / TFTP configuration types.
//!
//! These structs mirror the `[dhcp.pxe]` section of `dnshub.toml`
//! (PRD lines 857-886). They are defined here (in the `dhcp::pxe` module)
//! so the runtime PXE handler owns its own configuration shape, and
//! re-exported from [`crate::config::dhcp`] for the config tree.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// `[dhcp.pxe]` — PXE / BOOTP / TFTP network-boot configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PxeConfig {
    /// Master switch for PXE handling.
    #[serde(default)]
    pub enabled: bool,
    /// When `true`, dnshub provides PXE boot info (options 66/67/150)
    /// without performing any address allocation — useful in mixed
    /// environments where another DHCP server owns the lease pool.
    #[serde(default)]
    pub proxy_mode: bool,

    /// Built-in read-only TFTP server.
    #[serde(default)]
    pub tftp: TftpConfig,

    /// Per-architecture bootfile mapping, keyed by the string form of
    /// DHCP option 93 (client system architecture). Common keys:
    /// `"0"` (x86 BIOS), `"7"` (x86-64 UEFI), `"9"` (x86-64 UEFI HTTP),
    /// `"11"` (ARM64 UEFI).
    #[serde(default)]
    pub bootfiles: HashMap<String, String>,

    /// iPXE chainloading configuration.
    #[serde(default)]
    pub ipxe: IpxeConfig,

    /// Static BOOTP entries — diskless clients with a fixed
    /// MAC → IP → bootfile mapping, treated as infinite-lease DHCP.
    #[serde(default)]
    pub bootp_static: Vec<BootpStaticEntry>,
}

/// `[dhcp.pxe.tftp]` — built-in read-only TFTP server (RFC 1350).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TftpConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Listen address, e.g. `"0.0.0.0:69"`.
    #[serde(default = "default_tftp_listen")]
    pub listen: String,
    /// Root directory containing boot images.
    #[serde(default)]
    pub root_dir: String,
    /// Allowlist of glob patterns (e.g. `["*.efi", "pxelinux.0"]`).
    /// An empty list means "serve any file under `root_dir`".
    #[serde(default)]
    pub allowlist: Vec<String>,
}

fn default_tftp_listen() -> String {
    "0.0.0.0:69".to_string()
}

/// `[dhcp.pxe.ipxe]` — iPXE chainloading for HTTP-based boot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IpxeConfig {
    #[serde(default)]
    pub enabled: bool,
    /// HTTP URL the iPXE client fetches as its next-stage script/image,
    /// e.g. `"http://boot.levonk.com/boot.ipxe"`.
    #[serde(default)]
    pub chain_url: String,
}

/// `[[dhcp.pxe.bootp_static]]` — a single static BOOTP entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BootpStaticEntry {
    /// Client MAC address, e.g. `"00:11:22:33:44:AA"`.
    #[serde(default)]
    pub mac: String,
    /// Fixed IPv4 address, e.g. `"192.168.1.50"`.
    #[serde(default)]
    pub ip: String,
    /// Bootfile name, e.g. `"coreos-install.ipxe"`.
    #[serde(default)]
    pub bootfile: String,
    /// TFTP server address, e.g. `"192.168.1.67"`.
    #[serde(default)]
    pub tftp_server: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pxe_config_defaults_disabled() {
        let cfg = PxeConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.proxy_mode);
        assert!(!cfg.tftp.enabled);
        assert!(!cfg.ipxe.enabled);
        assert!(cfg.bootfiles.is_empty());
        assert!(cfg.bootp_static.is_empty());
    }

    #[test]
    fn pxe_config_parses_from_toml() {
        let toml = r#"
enabled = true
proxy_mode = false

[tftp]
enabled = true
listen = "0.0.0.0:69"
root_dir = "/etc/dnshub/tftp"
allowlist = ["*.efi", "pxelinux.0"]

[bootfiles]
"0" = "pxelinux.0"
"7" = "grubx64.efi"
"11" = "grubaa64.efi"

[ipxe]
enabled = true
chain_url = "http://boot.levonk.com/boot.ipxe"

[[bootp_static]]
mac = "00:11:22:33:44:AA"
ip = "192.168.1.50"
bootfile = "coreos-install.ipxe"
tftp_server = "192.168.1.67"
"#;
        let cfg: PxeConfig = toml::from_str(toml).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.tftp.enabled);
        assert_eq!(cfg.tftp.root_dir, "/etc/dnshub/tftp");
        assert_eq!(cfg.bootfiles.get("0"), Some(&"pxelinux.0".to_string()));
        assert_eq!(cfg.bootfiles.get("7"), Some(&"grubx64.efi".to_string()));
        assert_eq!(cfg.bootfiles.get("11"), Some(&"grubaa64.efi".to_string()));
        assert!(cfg.ipxe.enabled);
        assert_eq!(cfg.ipxe.chain_url, "http://boot.levonk.com/boot.ipxe");
        assert_eq!(cfg.bootp_static.len(), 1);
        assert_eq!(cfg.bootp_static[0].mac, "00:11:22:33:44:AA");
        assert_eq!(cfg.bootp_static[0].bootfile, "coreos-install.ipxe");
    }
}
