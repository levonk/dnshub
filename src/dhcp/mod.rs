//! DHCP server (v4 + v6) using dhcproto.
//!
//! The v4 server is implemented in later stories (04-001). The v6 server
//! core (lease store, state machine, IA_NA, options, DUID) lives in the
//! [`v6`] submodule (story 04-002).

pub mod v6;
