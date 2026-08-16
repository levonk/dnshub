//! PXE request handling.
//!
//! [`PxeHandler`] is the entry point for PXE network-boot logic. It:
//!
//! 1. Detects PXE requests (DHCP option 60 = `"PXEClient"`).
//! 2. Reads the client architecture from option 93.
//! 3. Selects a bootfile — preferring an iPXE chain URL when the client
//!    is an already-loaded iPXE client, otherwise falling back to the
//!    per-architecture [`BootfileMapper`].
//! 4. Builds the PXE response options (66 = TFTP server name,
//!    67 = bootfile name, 150 = TFTP server address) to be merged into
//!    the DHCP OFFER/ACK by the server core (stories 04-001/04-002).
//!
//! See PRD lines 450-471 and 857-886 for the full PXE/BOOTP/TFTP scope.

pub mod bootfile;
pub mod bootp;
pub mod config;
pub mod ipxe;
pub mod proxy;

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, DhcpOptions, OptionCode};

pub use bootfile::BootfileMapper;
pub use bootp::{BootpAssignment, BootpStaticTable};
pub use config::{BootpStaticEntry, IpxeConfig, PxeConfig, TftpConfig};
pub use ipxe::{ipxe_chain_bootfile, is_ipxe_client};

/// The option-60 value that identifies a PXE boot request.
pub const PXE_CLIENT_CLASS_ID: &[u8] = b"PXEClient";

/// PXE response options ready to be merged into a DHCP OFFER/ACK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PxeResponseOptions {
    /// Option 66 — TFTP server name (hostname or IP string).
    pub tftp_server_name: String,
    /// Option 67 — bootfile name (or iPXE chain URL).
    pub bootfile: String,
    /// Option 150 — TFTP server address.
    pub tftp_server_addr: Ipv4Addr,
}

/// Stateful PXE handler bundling the configured mapper, BOOTP table and
/// config references.
#[derive(Debug, Clone)]
pub struct PxeHandler {
    config: PxeConfig,
    mapper: BootfileMapper,
    bootp: BootpStaticTable,
}

impl PxeHandler {
    /// Build a handler from a PXE config.
    pub fn new(config: PxeConfig) -> Self {
        let mapper = BootfileMapper::new(config.bootfiles.clone());
        let bootp = BootpStaticTable::from_config(&config.bootp_static);
        Self {
            config,
            mapper,
            bootp,
        }
    }

    /// Reference the underlying PXE config.
    pub fn config(&self) -> &PxeConfig {
        &self.config
    }

    /// Reference the bootfile mapper.
    pub fn mapper(&self) -> &BootfileMapper {
        &self.mapper
    }

    /// Reference the BOOTP static table.
    pub fn bootp_table(&self) -> &BootpStaticTable {
        &self.bootp
    }

    /// Returns `true` if `opts` carries option 60 = `"PXEClient"`.
    pub fn is_pxe_request(&self, opts: &DhcpOptions) -> bool {
        is_pxe_request(opts)
    }

    /// Build the PXE response options for a client.
    ///
    /// `arch_code` is the raw option-93 value (use [`extract_arch_code`]).
    /// `tftp_server_addr` is dnshub's own TFTP address (option 150 /
    /// `siaddr`); `tftp_server_name` is the hostname/IP string for
    /// option 66.
    ///
    /// Returns `None` if PXE is disabled, the request is not a PXE
    /// request, or no bootfile could be selected for the architecture.
    pub fn build_pxe_response(
        &self,
        opts: &DhcpOptions,
        arch_code: u16,
        tftp_server_addr: Ipv4Addr,
        tftp_server_name: &str,
    ) -> Option<PxeResponseOptions> {
        if !self.config.enabled {
            return None;
        }
        if !is_pxe_request(opts) {
            return None;
        }

        // iPXE chainload takes precedence over arch-based bootfile.
        let bootfile = ipxe_chain_bootfile(&self.config.ipxe, opts)
            .or_else(|| self.mapper.select_bootfile(arch_code))?;

        Some(PxeResponseOptions {
            tftp_server_name: tftp_server_name.to_string(),
            bootfile,
            tftp_server_addr,
        })
    }
}

/// Returns `true` if the DHCP options carry option 60 = `"PXEClient"`.
pub fn is_pxe_request(opts: &DhcpOptions) -> bool {
    match opts.get(OptionCode::ClassIdentifier) {
        Some(DhcpOption::ClassIdentifier(bytes)) => {
            // Option 60 may carry a trailing vendor sub-type (e.g.
            // "PXEClient:Arch:00007:UNDI:003001"). Match the prefix.
            bytes.starts_with(PXE_CLIENT_CLASS_ID)
        }
        _ => false,
    }
}

/// Extract the raw client architecture code (option 93) from a request.
///
/// Returns `None` if option 93 is absent. The raw `u16` is recovered via
/// `u16::from(Architecture)`, which round-trips the original wire code
/// even for codes dhcproto's `Architecture` enum maps to a named variant.
pub fn extract_arch_code(opts: &DhcpOptions) -> Option<u16> {
    match opts.get(OptionCode::ClientSystemArchitecture) {
        Some(DhcpOption::ClientSystemArchitecture(arch)) => Some(u16::from(*arch)),
        _ => None,
    }
}

/// Merge [`PxeResponseOptions`] into a `DhcpOptions` set (options 66,
/// 67, 150). The caller (DHCP server core) is responsible for setting
/// `siaddr`/`yiaddr` and lease-time on the message itself.
pub fn merge_pxe_options(opts: &mut DhcpOptions, pxe: &PxeResponseOptions) {
    opts.insert(DhcpOption::TFTPServerName(
        pxe.tftp_server_name.as_bytes().to_vec(),
    ));
    opts.insert(DhcpOption::BootfileName(pxe.bootfile.as_bytes().to_vec()));
    opts.insert(DhcpOption::TFTPServerAddress(pxe.tftp_server_addr));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dhcp::pxe::bootfile::arch_code;
    use std::collections::HashMap;

    fn handler() -> PxeHandler {
        let mut bootfiles = HashMap::new();
        bootfiles.insert("0".to_string(), "pxelinux.0".to_string());
        bootfiles.insert("7".to_string(), "grubx64.efi".to_string());
        let mut ipxe = IpxeConfig::default();
        ipxe.enabled = false; // disable iPXE for arch-based tests
        PxeHandler::new(PxeConfig {
            enabled: true,
            proxy_mode: false,
            bootfiles,
            ipxe,
            ..Default::default()
        })
    }

    fn pxe_request_opts(arch: u16) -> DhcpOptions {
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::ClassIdentifier(b"PXEClient:Arch:00007".to_vec()));
        opts.insert(DhcpOption::ClientSystemArchitecture(
            dhcproto::v4::Architecture::from(arch),
        ));
        opts
    }

    #[test]
    fn detects_pxe_request() {
        let h = handler();
        let opts = pxe_request_opts(arch_code::X86_64_UEFI);
        assert!(h.is_pxe_request(&opts));
    }

    #[test]
    fn non_pxe_request_not_detected() {
        let h = handler();
        let opts = DhcpOptions::new();
        assert!(!h.is_pxe_request(&opts));
    }

    #[test]
    fn detects_pxe_request_with_plain_class_id() {
        let h = handler();
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::ClassIdentifier(b"PXEClient".to_vec()));
        assert!(h.is_pxe_request(&opts));
    }

    #[test]
    fn extracts_arch_code() {
        let opts = pxe_request_opts(arch_code::X86_64_UEFI);
        assert_eq!(extract_arch_code(&opts), Some(arch_code::X86_64_UEFI));
    }

    #[test]
    fn extracts_arch_code_bios() {
        let opts = pxe_request_opts(arch_code::X86_BIOS);
        assert_eq!(extract_arch_code(&opts), Some(arch_code::X86_BIOS));
    }

    #[test]
    fn arch_code_none_when_absent() {
        let opts = DhcpOptions::new();
        assert_eq!(extract_arch_code(&opts), None);
    }

    #[test]
    fn builds_response_for_uefi() {
        let h = handler();
        let opts = pxe_request_opts(arch_code::X86_64_UEFI);
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        let resp = h
            .build_pxe_response(&opts, arch_code::X86_64_UEFI, tftp, "tftp.levonk.com")
            .unwrap();
        assert_eq!(resp.bootfile, "grubx64.efi");
        assert_eq!(resp.tftp_server_name, "tftp.levonk.com");
        assert_eq!(resp.tftp_server_addr, tftp);
    }

    #[test]
    fn builds_response_for_bios() {
        let h = handler();
        let opts = pxe_request_opts(arch_code::X86_BIOS);
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        let resp = h
            .build_pxe_response(&opts, arch_code::X86_BIOS, tftp, "tftp")
            .unwrap();
        assert_eq!(resp.bootfile, "pxelinux.0");
    }

    #[test]
    fn response_none_when_not_pxe_request() {
        let h = handler();
        let opts = DhcpOptions::new();
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        assert_eq!(
            h.build_pxe_response(&opts, arch_code::X86_BIOS, tftp, "tftp"),
            None
        );
    }

    #[test]
    fn response_none_when_pxe_disabled() {
        let mut cfg = PxeConfig {
            enabled: true,
            bootfiles: HashMap::new(),
            ..Default::default()
        };
        cfg.enabled = false;
        let h = PxeHandler::new(cfg);
        let opts = pxe_request_opts(arch_code::X86_BIOS);
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        assert_eq!(
            h.build_pxe_response(&opts, arch_code::X86_BIOS, tftp, "tftp"),
            None
        );
    }

    #[test]
    fn response_none_for_unknown_arch() {
        let h = handler();
        let opts = pxe_request_opts(42);
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        assert_eq!(h.build_pxe_response(&opts, 42, tftp, "tftp"), None);
    }

    #[test]
    fn ipxe_chain_url_overrides_arch_bootfile() {
        let mut bootfiles = HashMap::new();
        bootfiles.insert("7".to_string(), "grubx64.efi".to_string());
        let cfg = PxeConfig {
            enabled: true,
            bootfiles,
            ipxe: IpxeConfig {
                enabled: true,
                chain_url: "http://boot.levonk.com/boot.ipxe".to_string(),
            },
            ..Default::default()
        };
        let h = PxeHandler::new(cfg);
        let mut opts = pxe_request_opts(arch_code::X86_64_UEFI);
        // Mark as iPXE client.
        opts.insert(DhcpOption::UserClass(b"iPXE".to_vec()));
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        let resp = h
            .build_pxe_response(&opts, arch_code::X86_64_UEFI, tftp, "tftp")
            .unwrap();
        assert_eq!(resp.bootfile, "http://boot.levonk.com/boot.ipxe");
    }

    #[test]
    fn merge_pxe_options_sets_66_67_150() {
        let mut opts = DhcpOptions::new();
        let pxe = PxeResponseOptions {
            tftp_server_name: "tftp.levonk.com".to_string(),
            bootfile: "grubx64.efi".to_string(),
            tftp_server_addr: "192.168.1.67".parse().unwrap(),
        };
        merge_pxe_options(&mut opts, &pxe);
        assert!(opts.get(OptionCode::TFTPServerName).is_some());
        assert!(opts.get(OptionCode::BootfileName).is_some());
        assert!(opts.get(OptionCode::TFTPServerAddress).is_some());
    }
}
