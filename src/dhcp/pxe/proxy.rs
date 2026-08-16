//! PXE proxy mode.
//!
//! In proxy mode dnshub provides PXE boot information (options 66, 67,
//! 150) to clients *without* performing any address allocation. This is
//! useful in mixed environments where another DHCP server owns the
//! lease pool but cannot serve PXE options.
//!
//! Per RFC 5071 / PXE spec, a proxy DHCP server responds with:
//! - `siaddr` (next-server) set to the TFTP server,
//! - option 60 = `"PXEClient"` echoed back,
//! - option 66 (TFTP server name), option 67 (bootfile), option 150
//!   (TFTP server address),
//! - **no** lease IP (`yiaddr` = 0.0.0.0) and **no** lease-time option.

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, DhcpOptions};

use crate::dhcp::pxe::bootfile::BootfileMapper;
use crate::dhcp::pxe::config::PxeConfig;

/// The set of PXE options a proxy response carries, ready to be merged
/// into a DHCP response by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyPxeOptions {
    /// TFTP server name (option 66).
    pub tftp_server_name: String,
    /// Bootfile name (option 67).
    pub bootfile: String,
    /// TFTP server address (option 150).
    pub tftp_server_addr: Ipv4Addr,
}

/// Build the PXE options for a proxy response.
///
/// Returns `None` if PXE/proxy mode is disabled or no bootfile could be
/// selected for the client's architecture.
pub fn build_proxy_pxe_options(
    config: &PxeConfig,
    mapper: &BootfileMapper,
    arch_code: u16,
    tftp_server_addr: Ipv4Addr,
    tftp_server_name: &str,
) -> Option<ProxyPxeOptions> {
    if !config.enabled || !config.proxy_mode {
        return None;
    }
    let bootfile = mapper.select_bootfile(arch_code)?;
    Some(ProxyPxeOptions {
        tftp_server_name: tftp_server_name.to_string(),
        bootfile,
        tftp_server_addr,
    })
}

/// Merge proxy PXE options into a `DhcpOptions` set, suitable for a
/// proxy DHCP OFFER/ACK. No lease-time or IP-related options are added.
pub fn merge_proxy_options(opts: &mut DhcpOptions, pxe: &ProxyPxeOptions) {
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

    fn proxy_config() -> PxeConfig {
        let mut bootfiles = HashMap::new();
        bootfiles.insert("0".to_string(), "pxelinux.0".to_string());
        bootfiles.insert("7".to_string(), "grubx64.efi".to_string());
        PxeConfig {
            enabled: true,
            proxy_mode: true,
            bootfiles,
            ..Default::default()
        }
    }

    fn mapper() -> BootfileMapper {
        BootfileMapper::from_pairs([
            (arch_code::X86_BIOS, "pxelinux.0".to_string()),
            (arch_code::X86_64_UEFI, "grubx64.efi".to_string()),
        ])
    }

    #[test]
    fn proxy_options_built_for_known_arch() {
        let cfg = proxy_config();
        let m = mapper();
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        let pxe =
            build_proxy_pxe_options(&cfg, &m, arch_code::X86_64_UEFI, tftp, "tftp.levonk.com")
                .unwrap();
        assert_eq!(pxe.bootfile, "grubx64.efi");
        assert_eq!(pxe.tftp_server_name, "tftp.levonk.com");
        assert_eq!(pxe.tftp_server_addr, tftp);
    }

    #[test]
    fn proxy_options_none_when_proxy_mode_disabled() {
        let mut cfg = proxy_config();
        cfg.proxy_mode = false;
        let m = mapper();
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        assert_eq!(
            build_proxy_pxe_options(&cfg, &m, arch_code::X86_64_UEFI, tftp, "tftp"),
            None
        );
    }

    #[test]
    fn proxy_options_none_when_pxe_disabled() {
        let mut cfg = proxy_config();
        cfg.enabled = false;
        let m = mapper();
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        assert_eq!(
            build_proxy_pxe_options(&cfg, &m, arch_code::X86_64_UEFI, tftp, "tftp"),
            None
        );
    }

    #[test]
    fn proxy_options_none_for_unknown_arch() {
        let cfg = proxy_config();
        let m = mapper();
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        assert_eq!(
            build_proxy_pxe_options(&cfg, &m, 42, tftp, "tftp"),
            None
        );
    }

    #[test]
    fn merge_proxy_options_adds_only_pxe_options() {
        let cfg = proxy_config();
        let m = mapper();
        let tftp: Ipv4Addr = "192.168.1.67".parse().unwrap();
        let pxe = build_proxy_pxe_options(&cfg, &m, arch_code::X86_BIOS, tftp, "tftp").unwrap();

        let mut opts = DhcpOptions::new();
        merge_proxy_options(&mut opts, &pxe);

        use dhcproto::v4::OptionCode;
        assert!(opts.get(OptionCode::TFTPServerName).is_some());
        assert!(opts.get(OptionCode::BootfileName).is_some());
        assert!(opts.get(OptionCode::TFTPServerAddress).is_some());
        // Proxy mode must NOT add lease time.
        assert!(opts.get(OptionCode::AddressLeaseTime).is_none());
    }
}
