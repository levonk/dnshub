//! iPXE chainloading support.
//!
//! iPXE is an open-source network boot firmware that supports HTTP,
//! iSCSI, and other protocols beyond plain TFTP. The typical chainload
//! flow is:
//!
//! 1. PXE client (firmware) sends a DHCP discover; dnshub returns a
//!    small iPXE image (`undionly.kpxe`) as the bootfile via TFTP.
//! 2. iPXE loads and sends a *second* DHCP discover, this time
//!    identifying itself as iPXE (option 175 / user-class "iPXE").
//! 3. dnshub detects the iPXE client and returns the configured
//!    `chain_url` (an HTTP URL) as the bootfile, so iPXE fetches the
//!    real boot script/image over HTTP — much faster than TFTP and
//!    supports larger images and scripts.
//!
//! This module handles step 3: detecting an iPXE client and producing
//! the chainload bootfile response.

use dhcproto::v4::{DhcpOption, DhcpOptions, OptionCode};

use crate::dhcp::pxe::config::IpxeConfig;

/// iPXE-specific DHCP option code (175). Fits in a `u8` (DHCP option
/// codes are 0-255).
pub const IPXE_OPTION_CODE: u8 = 175;

/// The user-class string iPXE advertises (RFC 3004 user-class, option 77).
pub const IPXE_USER_CLASS: &str = "iPXE";

/// Detect whether a DHCP request originates from an already-loaded iPXE
/// client.
///
/// iPXE identifies itself in one (or both) of two ways:
/// - option 175 (iPXE private option space) is present, or
/// - option 77 (User Class) contains the bytes `"iPXE"`.
pub fn is_ipxe_client(opts: &DhcpOptions) -> bool {
    // Option 175 — present in any real iPXE request.
    if let Some(DhcpOption::Unknown(_)) = opts.get(OptionCode::Unknown(IPXE_OPTION_CODE)) {
        return true;
    }
    // Option 77 — User Class. iPXE uses the bare string "iPXE".
    if let Some(DhcpOption::UserClass(bytes)) = opts.get(OptionCode::UserClass) {
        // RFC 3004 user-class is length-prefixed instances; iPXE in
        // practice sends a single instance. Accept either the raw
        // "iPXE" bytes or the length-prefixed form.
        if bytes.as_slice() == IPXE_USER_CLASS.as_bytes()
            || (bytes.len() == IPXE_USER_CLASS.len() + 1
                && bytes[0] as usize == IPXE_USER_CLASS.len()
                && &bytes[1..] == IPXE_USER_CLASS.as_bytes())
        {
            return true;
        }
    }
    false
}

/// Decide the bootfile for an iPXE client.
///
/// If iPXE chainloading is enabled and the client is an iPXE client,
/// returns the configured `chain_url` (to be placed in option 67).
/// Otherwise returns `None` so the caller falls back to the
/// architecture-based bootfile.
pub fn ipxe_chain_bootfile(config: &IpxeConfig, opts: &DhcpOptions) -> Option<String> {
    if !config.enabled || config.chain_url.is_empty() {
        return None;
    }
    if !is_ipxe_client(opts) {
        return None;
    }
    Some(config.chain_url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipxe_cfg() -> IpxeConfig {
        IpxeConfig {
            enabled: true,
            chain_url: "http://boot.levonk.com/boot.ipxe".to_string(),
        }
    }

    #[test]
    fn detects_ipxe_via_option_175() {
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::Unknown(dhcproto::v4::UnknownOption::new(
            OptionCode::Unknown(IPXE_OPTION_CODE),
            vec![0x01],
        )));
        assert!(is_ipxe_client(&opts));
    }

    #[test]
    fn detects_ipxe_via_user_class_bare() {
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::UserClass(b"iPXE".to_vec()));
        assert!(is_ipxe_client(&opts));
    }

    #[test]
    fn detects_ipxe_via_user_class_length_prefixed() {
        let mut opts = DhcpOptions::new();
        // RFC 3004: [len][bytes]
        let mut bytes = vec![5];
        bytes.extend_from_slice(b"iPXE");
        // Actually iPXE is 4 chars; fix length.
        bytes[0] = 4;
        opts.insert(DhcpOption::UserClass(bytes));
        assert!(is_ipxe_client(&opts));
    }

    #[test]
    fn non_ipxe_client_not_detected() {
        let opts = DhcpOptions::new();
        assert!(!is_ipxe_client(&opts));
    }

    #[test]
    fn chain_bootfile_returned_for_ipxe_client() {
        let cfg = ipxe_cfg();
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::UserClass(b"iPXE".to_vec()));
        assert_eq!(
            ipxe_chain_bootfile(&cfg, &opts),
            Some("http://boot.levonk.com/boot.ipxe".to_string())
        );
    }

    #[test]
    fn chain_bootfile_none_for_non_ipxe() {
        let cfg = ipxe_cfg();
        let opts = DhcpOptions::new();
        assert_eq!(ipxe_chain_bootfile(&cfg, &opts), None);
    }

    #[test]
    fn chain_bootfile_none_when_disabled() {
        let cfg = IpxeConfig {
            enabled: false,
            chain_url: "http://boot.levonk.com/boot.ipxe".to_string(),
        };
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::UserClass(b"iPXE".to_vec()));
        assert_eq!(ipxe_chain_bootfile(&cfg, &opts), None);
    }

    #[test]
    fn chain_bootfile_none_when_url_empty() {
        let cfg = IpxeConfig {
            enabled: true,
            chain_url: String::new(),
        };
        let mut opts = DhcpOptions::new();
        opts.insert(DhcpOption::UserClass(b"iPXE".to_vec()));
        assert_eq!(ipxe_chain_bootfile(&cfg, &opts), None);
    }
}
