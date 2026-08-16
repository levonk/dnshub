//! Per-architecture bootfile selection (DHCP option 93).
//!
//! PXE clients send option 93 (Client System Architecture) to indicate
//! their firmware type. dnshub maps that code to a bootfile name from
//! the configured `[dhcp.pxe.bootfiles]` table.
//!
//! Common architecture codes (per PRD / real-world PXE usage):
//!
//! | code | meaning            |
//! |------|--------------------|
//! | 0    | x86 BIOS           |
//! | 6    | EFI IA32 (32-bit)  |
//! | 7    | x86-64 UEFI        |
//! | 9    | x86-64 UEFI HTTP   |
//! | 11   | ARM64 UEFI         |
//!
//! Note: dhcproto's `Architecture` enum remaps a few codes (e.g. 7 → `BC`,
//! 9 → `X86_64`), but round-tripping through `u16::from(arch)` preserves
//! the original wire code, so this mapper works on the raw `u16` value.

use std::collections::HashMap;

/// Well-known PXE client architecture codes (RFC 4578 + later additions).
pub mod arch_code {
    /// x86 BIOS (Intel x86 PC).
    pub const X86_BIOS: u16 = 0;
    /// EFI IA32 (32-bit UEFI).
    pub const EFI_IA32: u16 = 6;
    /// x86-64 UEFI.
    pub const X86_64_UEFI: u16 = 7;
    /// x86-64 UEFI HTTP boot.
    pub const X86_64_UEFI_HTTP: u16 = 9;
    /// ARM64 UEFI.
    pub const ARM64_UEFI: u16 = 11;
}

/// Maps PXE client architecture codes (option 93) to bootfile names.
#[derive(Debug, Clone, Default)]
pub struct BootfileMapper {
    /// Keyed by the string form of the architecture code, matching the
    /// `[dhcp.pxe.bootfiles]` TOML table.
    map: HashMap<String, String>,
}

impl BootfileMapper {
    /// Build a mapper from a `code (as string) → filename` map.
    pub fn new(map: HashMap<String, String>) -> Self {
        Self { map }
    }

    /// Build a mapper from `(code, filename)` pairs.
    pub fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (u16, String)>,
    {
        let map = pairs
            .into_iter()
            .map(|(code, name)| (code.to_string(), name))
            .collect();
        Self { map }
    }

    /// Select a bootfile for the given architecture code.
    ///
    /// Returns `None` if no mapping is configured for that architecture.
    pub fn select_bootfile(&self, arch_code: u16) -> Option<String> {
        self.map.get(&arch_code.to_string()).cloned()
    }

    /// Returns `true` if the mapper has an entry for the given code.
    pub fn has(&self, arch_code: u16) -> bool {
        self.map.contains_key(&arch_code.to_string())
    }

    /// Number of configured mappings.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the mapper is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mapper() -> BootfileMapper {
        BootfileMapper::from_pairs([
            (arch_code::X86_BIOS, "pxelinux.0".to_string()),
            (arch_code::X86_64_UEFI, "grubx64.efi".to_string()),
            (arch_code::ARM64_UEFI, "grubaa64.efi".to_string()),
        ])
    }

    #[test]
    fn bios_maps_to_pxelinux() {
        let m = default_mapper();
        assert_eq!(
            m.select_bootfile(arch_code::X86_BIOS),
            Some("pxelinux.0".to_string())
        );
    }

    #[test]
    fn uefi_x64_maps_to_grubx64() {
        let m = default_mapper();
        assert_eq!(
            m.select_bootfile(arch_code::X86_64_UEFI),
            Some("grubx64.efi".to_string())
        );
    }

    #[test]
    fn arm64_maps_to_grubaa64() {
        let m = default_mapper();
        assert_eq!(
            m.select_bootfile(arch_code::ARM64_UEFI),
            Some("grubaa64.efi".to_string())
        );
    }

    #[test]
    fn unknown_arch_returns_none() {
        let m = default_mapper();
        assert_eq!(m.select_bootfile(42), None);
    }

    #[test]
    fn empty_mapper_returns_none() {
        let m = BootfileMapper::default();
        assert!(m.is_empty());
        assert_eq!(m.select_bootfile(0), None);
    }

    #[test]
    fn from_string_map_round_trips() {
        let mut map = HashMap::new();
        map.insert("0".to_string(), "pxelinux.0".to_string());
        map.insert("7".to_string(), "grubx64.efi".to_string());
        let m = BootfileMapper::new(map);
        assert_eq!(m.len(), 2);
        assert!(m.has(0));
        assert!(m.has(7));
        assert!(!m.has(11));
        assert_eq!(m.select_bootfile(7), Some("grubx64.efi".to_string()));
    }
}
