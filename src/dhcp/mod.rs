//! DHCP server (v4 + v6) using dhcproto.
//!
//! TODO: implemented in later stories (04-001 through 04-008).
//!
//! Story 04-007 adds the relay agent handler ([`relay`]) which receives
//! relayed DHCP requests (giaddr / link-address), parses Option 82,
//! validates relay trust, and selects the appropriate multi-VLAN pool.

pub mod relay;
