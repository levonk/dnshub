//! Option 82 (Relay Agent Information, RFC 3046) parser.
//!
//! Option 82 is carried in DHCPv4 option 82 as a sequence of sub-options.
//! The two sub-options dnshub cares about are:
//!
//! - sub-option 1 — Agent Circuit ID (typically identifies the switch port)
//! - sub-option 2 — Agent Remote ID (typically identifies the relay agent /
//!   switch)
//!
//! dnshub parses Option 82 from relayed requests and uses the circuit ID
//! for pool / profile selection (PRD lines 484-489). It does **not**
//! inject Option 82 — that is the relay agent's responsibility.
//!
//! The parser works on a raw byte slice so it can be used both on a
//! pre-decoded [`dhcproto::v4::relay::RelayAgentInformation`] and directly
//! on the option payload bytes. The raw-byte path is provided so that
//! callers that only have the option payload (e.g. from a custom decoder)
//! can still extract the circuit / remote IDs.

use dhcproto::v4::relay::{RelayAgentInformation, RelayCode, RelayInfo};

/// Parsed Option 82 sub-options of interest.
///
/// Both fields are raw bytes as carried on the wire. Callers can interpret
/// them as UTF-8 where the relay agent encodes text, or as opaque binary
/// otherwise. The `circuit_id_str` / `remote_id_str` helpers perform a
/// lossy UTF-8 conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Option82 {
    /// Sub-option 1 — Agent Circuit ID.
    pub circuit_id: Vec<u8>,
    /// Sub-option 2 — Agent Remote ID.
    pub remote_id: Vec<u8>,
}

impl Option82 {
    /// Returns the circuit ID as a lossy UTF-8 string.
    pub fn circuit_id_str(&self) -> String {
        String::from_utf8_lossy(&self.circuit_id).into_owned()
    }

    /// Returns the remote ID as a lossy UTF-8 string.
    pub fn remote_id_str(&self) -> String {
        String::from_utf8_lossy(&self.remote_id).into_owned()
    }

    /// Returns `true` if neither a circuit ID nor a remote ID was present.
    pub fn is_empty(&self) -> bool {
        self.circuit_id.is_empty() && self.remote_id.is_empty()
    }
}

/// Parse Option 82 sub-options from a pre-decoded
/// [`RelayAgentInformation`].
///
/// Returns `Some(Option82)` if at least one of the circuit ID or remote ID
/// sub-options is present, or `None` if neither is present.
pub fn from_relay_agent_info(info: &RelayAgentInformation) -> Option<Option82> {
    let circuit_id = match info.get(RelayCode::AgentCircuitId) {
        Some(RelayInfo::AgentCircuitId(data)) => data.clone(),
        _ => Vec::new(),
    };
    let remote_id = match info.get(RelayCode::AgentRemoteId) {
        Some(RelayInfo::AgentRemoteId(data)) => data.clone(),
        _ => Vec::new(),
    };
    if circuit_id.is_empty() && remote_id.is_empty() {
        None
    } else {
        Some(Option82 {
            circuit_id,
            remote_id,
        })
    }
}

/// Parse Option 82 sub-options from the raw option payload bytes.
///
/// The payload is a TLV sequence: `{code: u8, len: u8, data: [u8; len]}*`.
/// Only sub-options 1 (circuit ID) and 2 (remote ID) are extracted; other
/// sub-options are skipped. Malformed trailing bytes are ignored.
///
/// Returns `Some(Option82)` if at least one of the two sub-options is
/// present, or `None` if neither is present (or the payload is empty).
pub fn from_bytes(payload: &[u8]) -> Option<Option82> {
    let mut circuit_id = Vec::new();
    let mut remote_id = Vec::new();
    let mut pos = 0;
    while pos + 2 <= payload.len() {
        let code = payload[pos];
        let len = payload[pos + 1] as usize;
        let data_start = pos + 2;
        let data_end = data_start + len;
        if data_end > payload.len() {
            // truncated sub-option — stop parsing
            break;
        }
        let data = &payload[data_start..data_end];
        match code {
            1 => circuit_id = data.to_vec(),
            2 => remote_id = data.to_vec(),
            _ => {}
        }
        pos = data_end;
    }
    if circuit_id.is_empty() && remote_id.is_empty() {
        None
    } else {
        Some(Option82 {
            circuit_id,
            remote_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_circuit_and_remote_from_bytes() {
        // sub-option 1 (circuit id): "port-24"
        // sub-option 2 (remote id): "sw1"
        let payload = [
            1, 7, b'p', b'o', b'r', b't', b'-', b'2', b'4',
            2, 3, b's', b'w', b'1',
        ];
        let opt = from_bytes(&payload).expect("should parse");
        assert_eq!(opt.circuit_id_str(), "port-24");
        assert_eq!(opt.remote_id_str(), "sw1");
        assert!(!opt.is_empty());
    }

    #[test]
    fn parse_only_circuit_id_from_bytes() {
        let payload = [1, 5, b'p', b'o', b'r', b't', b'1'];
        let opt = from_bytes(&payload).expect("should parse");
        assert_eq!(opt.circuit_id_str(), "port1");
        assert!(opt.remote_id.is_empty());
    }

    #[test]
    fn parse_skips_unknown_suboptions() {
        // sub-option 5 (link selection, 4 bytes) then sub-option 1
        let payload = [
            5, 4, 192, 168, 10, 1,
            1, 4, b'p', b'o', b'r', b't',
        ];
        let opt = from_bytes(&payload).expect("should parse");
        assert_eq!(opt.circuit_id_str(), "port");
        assert!(opt.remote_id.is_empty());
    }

    #[test]
    fn no_option82_returns_none() {
        assert!(from_bytes(&[]).is_none());
        // unknown sub-option only -> no circuit/remote id
        assert!(from_bytes(&[5, 4, 192, 168, 10, 1]).is_none());
    }

    #[test]
    fn truncated_payload_stops_parsing() {
        // claims 10 bytes of data but only 2 follow
        let payload = [1, 10, b'a', b'b'];
        // circuit id would be truncated -> we stop, no valid sub-option
        let opt = from_bytes(&payload);
        assert!(opt.is_none());
    }

    #[test]
    fn from_relay_agent_info_extracts_suboptions() {
        let mut info = RelayAgentInformation::default();
        info.insert(RelayInfo::AgentCircuitId(vec![b'p', b'o', b'r', b't']));
        info.insert(RelayInfo::AgentRemoteId(vec![b's', b'w', b'1']));
        let opt = from_relay_agent_info(&info).expect("should parse");
        assert_eq!(opt.circuit_id_str(), "port");
        assert_eq!(opt.remote_id_str(), "sw1");
    }

    #[test]
    fn from_relay_agent_info_none_when_empty() {
        let info = RelayAgentInformation::default();
        assert!(from_relay_agent_info(&info).is_none());
    }

    #[test]
    fn from_relay_agent_info_ignores_other_suboptions() {
        let mut info = RelayAgentInformation::default();
        info.insert(RelayInfo::LinkSelection(
            "192.168.10.1".parse().unwrap(),
        ));
        assert!(from_relay_agent_info(&info).is_none());
    }

    #[test]
    fn roundtrip_bytes_and_relay_agent_info_agree() {
        let payload = [
            1, 6, b'p', b'o', b'r', b't', b'-', b'9',
            2, 3, b's', b'w', b'2',
        ];
        let from_raw = from_bytes(&payload).unwrap();

        let mut info = RelayAgentInformation::default();
        info.insert(RelayInfo::AgentCircuitId(b"port-9".to_vec()));
        info.insert(RelayInfo::AgentRemoteId(b"sw2".to_vec()));
        let from_info = from_relay_agent_info(&info).unwrap();

        assert_eq!(from_raw, from_info);
    }
}
