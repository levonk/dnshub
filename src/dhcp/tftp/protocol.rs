//! TFTP protocol packet types per RFC 1350.
//!
//! TFTP packets are UDP datagrams with a 2-byte big-endian opcode
//! followed by opcode-specific payload. The five base opcodes are:
//!
//! | opcode | name   | direction        |
//! |--------|--------|------------------|
//! | 1      | RRQ    | client → server  |
//! | 2      | WRQ    | client → server  |
//! | 3      | DATA   | both             |
//! | 4      | ACK    | both             |
//! | 5      | ERROR  | both             |
//! | 6      | OACK   | server → client (RFC 2347) |
//!
//! dnshub's TFTP server is read-only, so WRQ is always rejected with an
//! access-violation error. OACK is defined for completeness; blocksize
//! negotiation (RFC 2348) is out of scope for this story.

use std::io::{self, Cursor, Read};
use std::fmt;

/// TFTP opcodes (RFC 1350 §5).
pub mod opcode {
    pub const RRQ: u16 = 1;
    pub const WRQ: u16 = 2;
    pub const DATA: u16 = 3;
    pub const ACK: u16 = 4;
    pub const ERROR: u16 = 5;
    pub const OACK: u16 = 6;
}

/// TFTP transfer modes. dnshub only serves in `octet` mode.
pub mod mode {
    pub const OCTET: &str = "octet";
    pub const NETASCII: &str = "netascii";
    pub const MAIL: &str = "mail";
}

/// Standard TFTP error codes (RFC 1350 §5.5).
pub mod error_code {
    pub const NOT_DEFINED: u16 = 0;
    pub const FILE_NOT_FOUND: u16 = 1;
    pub const ACCESS_VIOLATION: u16 = 2;
    pub const DISK_FULL: u16 = 3;
    pub const ILLEGAL_OPERATION: u16 = 4;
    pub const UNKNOWN_TID: u16 = 5;
    pub const FILE_EXISTS: u16 = 6;
    pub const NO_SUCH_USER: u16 = 7;
}

/// Maximum DATA block size per RFC 1350 (512 bytes). A block smaller
/// than this signals the final block of a transfer.
pub const BLOCK_SIZE: usize = 512;

/// A decoded TFTP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Read request — filename + mode (+ optional RFC 2347 options).
    Rrq {
        filename: String,
        mode: String,
        options: Vec<(String, String)>,
    },
    /// Write request — same shape as RRQ. Always rejected by dnshub.
    Wrq {
        filename: String,
        mode: String,
        options: Vec<(String, String)>,
    },
    /// Data block — block number (starts at 1) + up to 512 bytes.
    Data { block: u16, data: Vec<u8> },
    /// Acknowledgement — block number being acknowledged.
    Ack { block: u16 },
    /// Error — error code + human-readable message.
    Error { code: u16, msg: String },
    /// Option acknowledgement (RFC 2347) — list of acknowledged options.
    Oack { options: Vec<(String, String)> },
}

impl Packet {
    /// Decode a TFTP packet from a UDP datagram.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < 2 {
            return Err(DecodeError::TooShort);
        }
        let opcode = u16::from_be_bytes([buf[0], buf[1]]);
        let rest = &buf[2..];
        match opcode {
            opcode::RRQ => {
                let (filename, mode, options) = decode_rrq(rest)?;
                Ok(Packet::Rrq {
                    filename,
                    mode,
                    options,
                })
            }
            opcode::WRQ => {
                let (filename, mode, options) = decode_rrq(rest)?;
                Ok(Packet::Wrq {
                    filename,
                    mode,
                    options,
                })
            }
            opcode::DATA => {
                if rest.len() < 2 {
                    return Err(DecodeError::TooShort);
                }
                let block = u16::from_be_bytes([rest[0], rest[1]]);
                Ok(Packet::Data {
                    block,
                    data: rest[2..].to_vec(),
                })
            }
            opcode::ACK => {
                if rest.len() < 2 {
                    return Err(DecodeError::TooShort);
                }
                let block = u16::from_be_bytes([rest[0], rest[1]]);
                Ok(Packet::Ack { block })
            }
            opcode::ERROR => {
                if rest.len() < 2 {
                    return Err(DecodeError::TooShort);
                }
                let code = u16::from_be_bytes([rest[0], rest[1]]);
                let msg = String::from_utf8_lossy(decode_cstring(&rest[2..])?).into_owned();
                Ok(Packet::Error { code, msg })
            }
            opcode::OACK => {
                let options = decode_options(rest)?;
                Ok(Packet::Oack { options })
            }
            _ => Err(DecodeError::UnknownOpcode(opcode)),
        }
    }

    /// Encode a TFTP packet into a UDP datagram.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Packet::Rrq {
                filename,
                mode,
                options,
            } => encode_request(opcode::RRQ, filename, mode, options),
            Packet::Wrq {
                filename,
                mode,
                options,
            } => encode_request(opcode::WRQ, filename, mode, options),
            Packet::Data { block, data } => {
                let mut out = Vec::with_capacity(4 + data.len());
                out.extend_from_slice(&opcode::DATA.to_be_bytes());
                out.extend_from_slice(&block.to_be_bytes());
                out.extend_from_slice(data);
                out
            }
            Packet::Ack { block } => {
                let mut out = Vec::with_capacity(4);
                out.extend_from_slice(&opcode::ACK.to_be_bytes());
                out.extend_from_slice(&block.to_be_bytes());
                out
            }
            Packet::Error { code, msg } => {
                let mut out = Vec::with_capacity(4 + msg.len() + 1);
                out.extend_from_slice(&opcode::ERROR.to_be_bytes());
                out.extend_from_slice(&code.to_be_bytes());
                out.extend_from_slice(msg.as_bytes());
                out.push(0);
                out
            }
            Packet::Oack { options } => {
                let mut out = Vec::with_capacity(2);
                out.extend_from_slice(&opcode::OACK.to_be_bytes());
                for (k, v) in options {
                    out.extend_from_slice(k.as_bytes());
                    out.push(0);
                    out.extend_from_slice(v.as_bytes());
                    out.push(0);
                }
                out
            }
        }
    }
}

/// Errors that can occur while decoding a TFTP packet.
#[derive(Debug)]
pub enum DecodeError {
    /// Packet shorter than the minimum 2-byte opcode (or truncated body).
    TooShort,
    /// A NUL terminator was expected but not found.
    MissingNull,
    /// Unrecognized opcode.
    UnknownOpcode(u16),
    /// Underlying I/O error.
    Io(io::Error),
}

impl PartialEq for DecodeError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DecodeError::TooShort, DecodeError::TooShort) => true,
            (DecodeError::MissingNull, DecodeError::MissingNull) => true,
            (DecodeError::UnknownOpcode(a), DecodeError::UnknownOpcode(b)) => a == b,
            (DecodeError::Io(a), DecodeError::Io(b)) => a.kind() == b.kind(),
            _ => false,
        }
    }
}

impl Eq for DecodeError {}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::TooShort => write!(f, "packet too short"),
            DecodeError::MissingNull => write!(f, "missing null terminator"),
            DecodeError::UnknownOpcode(op) => write!(f, "unknown opcode {op}"),
            DecodeError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for DecodeError {
    fn from(e: io::Error) -> Self {
        DecodeError::Io(e)
    }
}

fn encode_request(opcode: u16, filename: &str, mode: &str, options: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + filename.len() + 1 + mode.len() + 1);
    out.extend_from_slice(&opcode.to_be_bytes());
    out.extend_from_slice(filename.as_bytes());
    out.push(0);
    out.extend_from_slice(mode.as_bytes());
    out.push(0);
    for (k, v) in options {
        out.extend_from_slice(k.as_bytes());
        out.push(0);
        out.extend_from_slice(v.as_bytes());
        out.push(0);
    }
    out
}

/// Decode an RRQ/WRQ body: filename\0mode\0[opt\0val\0]...
fn decode_rrq(buf: &[u8]) -> Result<(String, String, Vec<(String, String)>), DecodeError> {
    let mut cur = Cursor::new(buf);
    let filename = String::from_utf8_lossy(&decode_cstring_reader(&mut cur)?).into_owned();
    let mode = String::from_utf8_lossy(&decode_cstring_reader(&mut cur)?).into_owned();
    // Remaining bytes are RFC 2347 options as k\0v\0 pairs.
    let mut options = Vec::new();
    let mut remaining = Vec::new();
    cur.read_to_end(&mut remaining)?;
    let mut iter = split_cstrings(&remaining);
    while let Some(key) = iter.next() {
        let value = iter.next().unwrap_or_default();
        options.push((key, value));
    }
    Ok((filename, mode, options))
}

fn decode_options(buf: &[u8]) -> Result<Vec<(String, String)>, DecodeError> {
    let mut options = Vec::new();
    let mut iter = split_cstrings(buf);
    while let Some(key) = iter.next() {
        let value = iter.next().unwrap_or_default();
        options.push((key, value));
    }
    Ok(options)
}

/// Read a NUL-terminated C string from a cursor.
fn decode_cstring_reader(cur: &mut Cursor<&[u8]>) -> Result<Vec<u8>, DecodeError> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match cur.read(&mut byte) {
            Ok(0) => return Err(DecodeError::MissingNull),
            Ok(_) => {
                if byte[0] == 0 {
                    return Ok(bytes);
                }
                bytes.push(byte[0]);
            }
            Err(e) => return Err(DecodeError::Io(e)),
        }
    }
}

/// Decode a NUL-terminated C string from a slice, returning the string
/// bytes and advancing past the terminator.
fn decode_cstring(buf: &[u8]) -> Result<&[u8], DecodeError> {
    let pos = buf.iter().position(|&b| b == 0).ok_or(DecodeError::MissingNull)?;
    Ok(&buf[..pos])
}

/// Iterator yielding successive NUL-terminated strings from a slice.
fn split_cstrings(buf: &[u8]) -> impl Iterator<Item = String> + '_ {
    let mut idx = 0;
    std::iter::from_fn(move || {
        if idx >= buf.len() {
            return None;
        }
        let end = buf[idx..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| idx + p)
            .unwrap_or(buf.len());
        let s = String::from_utf8_lossy(&buf[idx..end]).into_owned();
        idx = end + 1; // skip the NUL
        Some(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_rrq_octet() {
        let pkt = Packet::Rrq {
            filename: "pxelinux.0".to_string(),
            mode: mode::OCTET.to_string(),
            options: vec![],
        };
        let enc = pkt.encode();
        // opcode(2) + filename + \0 + mode + \0
        let expected = [
            0x00, 0x01, b'p', b'x', b'e', b'l', b'i', b'n', b'u', b'x', b'.', b'0', 0x00, b'o',
            b'c', b't', b'e', b't', 0x00,
        ];
        assert_eq!(enc, expected);

        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_rrq_with_options() {
        let pkt = Packet::Rrq {
            filename: "grubx64.efi".to_string(),
            mode: mode::OCTET.to_string(),
            options: vec![("blksize".to_string(), "1428".to_string())],
        };
        let enc = pkt.encode();
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_wrq() {
        let pkt = Packet::Wrq {
            filename: "upload.txt".to_string(),
            mode: mode::OCTET.to_string(),
            options: vec![],
        };
        let enc = pkt.encode();
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_data() {
        let payload = vec![0xAB; 512];
        let pkt = Packet::Data {
            block: 1,
            data: payload.clone(),
        };
        let enc = pkt.encode();
        // opcode(2) + block(2) + 512
        assert_eq!(enc.len(), 4 + 512);
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_data_final_block_short() {
        let payload = vec![0x01, 0x02, 0x03];
        let pkt = Packet::Data {
            block: 5,
            data: payload.clone(),
        };
        let enc = pkt.encode();
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_ack() {
        let pkt = Packet::Ack { block: 42 };
        let enc = pkt.encode();
        assert_eq!(enc, [0x00, 0x04, 0x00, 0x2A]);
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_error() {
        let pkt = Packet::Error {
            code: error_code::FILE_NOT_FOUND,
            msg: "File not found".to_string(),
        };
        let enc = pkt.encode();
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn round_trip_oack() {
        let pkt = Packet::Oack {
            options: vec![("blksize".to_string(), "1428".to_string())],
        };
        let enc = pkt.encode();
        let dec = Packet::decode(&enc).unwrap();
        assert_eq!(pkt, dec);
    }

    #[test]
    fn decode_rejects_too_short() {
        assert_eq!(Packet::decode(&[0x00]), Err(DecodeError::TooShort));
        assert_eq!(Packet::decode(&[]), Err(DecodeError::TooShort));
    }

    #[test]
    fn decode_rejects_unknown_opcode() {
        // opcode 99
        let buf = [0x00, 0x63, 0x00];
        assert_eq!(Packet::decode(&buf), Err(DecodeError::UnknownOpcode(99)));
    }

    #[test]
    fn decode_data_too_short() {
        let buf = [0x00, 0x03, 0x00]; // opcode DATA, only 1 byte for block
        assert_eq!(Packet::decode(&buf), Err(DecodeError::TooShort));
    }

    #[test]
    fn decode_ack_too_short() {
        let buf = [0x00, 0x04, 0x00];
        assert_eq!(Packet::decode(&buf), Err(DecodeError::TooShort));
    }

    #[test]
    fn decode_error_missing_null() {
        let buf = [0x00, 0x05, 0x00, 0x01, b'h', b'i']; // no null terminator
        assert_eq!(Packet::decode(&buf), Err(DecodeError::MissingNull));
    }

    #[test]
    fn decode_rrq_missing_null() {
        // opcode RRQ, no nulls
        let buf = [0x00, 0x01, b'f', b'o', b'o'];
        assert_eq!(Packet::decode(&buf), Err(DecodeError::MissingNull));
    }

    #[test]
    fn block_size_is_512() {
        assert_eq!(BLOCK_SIZE, 512);
    }
}
