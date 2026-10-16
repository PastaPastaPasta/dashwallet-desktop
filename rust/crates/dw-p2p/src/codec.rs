//! Message framing (Dash Core `src/protocol.h` `CMessageHeader`): 4-byte
//! network magic, 12-byte NUL-padded command, little-endian payload length,
//! the first 4 bytes of the payload's double SHA-256, then the payload.
//!
//! The crate frames every message itself and leaves payloads as bytes:
//! CoinJoin and governance messages have no typed `NetworkMessage` variant
//! in dashcore, and decoding unrelated traffic (headers, inv, addr) would
//! only add ways for a session to fail.

use dashcore::hashes::{Hash, sha256d};

/// Header size in bytes.
pub const HEADER_LEN: usize = 24;
/// Command field size (`CMessageHeader::COMMAND_SIZE`).
pub const COMMAND_LEN: usize = 12;
/// Largest payload Dash Core accepts (`MAX_PROTOCOL_MESSAGE_LENGTH`,
/// `src/net.h`: 3 MiB).
pub const MAX_PAYLOAD: usize = 3 * 1024 * 1024;

/// A framing error. Any of these ends the session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("command {0:?} is empty, too long or not printable ASCII")]
    BadCommand(String),
    #[error("payload of {0} bytes is above the 3 MiB limit")]
    TooLarge(usize),
    #[error("network magic {found:#010x} is not {expected:#010x}")]
    WrongMagic { expected: u32, found: u32 },
    #[error("checksum mismatch for {0:?}")]
    BadChecksum(String),
}

/// A parsed header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub command: String,
    pub length: usize,
    pub checksum: [u8; 4],
}

fn checksum(payload: &[u8]) -> [u8; 4] {
    let h = sha256d::Hash::hash(payload);
    let b = h.as_byte_array();
    [b[0], b[1], b[2], b[3]]
}

fn valid_command(command: &str) -> bool {
    !command.is_empty()
        && command.len() <= COMMAND_LEN
        && command.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// Frames `payload` as `command` for the network with `magic`.
pub fn encode(magic: u32, command: &str, payload: &[u8]) -> Result<Vec<u8>, CodecError> {
    if !valid_command(command) {
        return Err(CodecError::BadCommand(command.to_string()));
    }
    if payload.len() > MAX_PAYLOAD {
        return Err(CodecError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&magic.to_le_bytes());
    let mut cmd = [0u8; COMMAND_LEN];
    cmd[..command.len()].copy_from_slice(command.as_bytes());
    out.extend_from_slice(&cmd);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&checksum(payload));
    out.extend_from_slice(payload);
    Ok(out)
}

/// Parses a header received on the network with `magic`.
pub fn decode_header(magic: u32, raw: &[u8; HEADER_LEN]) -> Result<Header, CodecError> {
    let found = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if found != magic {
        return Err(CodecError::WrongMagic {
            expected: magic,
            found,
        });
    }
    let cmd = &raw[4..4 + COMMAND_LEN];
    let end = cmd.iter().position(|b| *b == 0).unwrap_or(COMMAND_LEN);
    // Core rejects a command with bytes after the first NUL.
    if cmd[end..].iter().any(|b| *b != 0) {
        return Err(CodecError::BadCommand(
            String::from_utf8_lossy(cmd).into_owned(),
        ));
    }
    let command = String::from_utf8_lossy(&cmd[..end]).into_owned();
    if !valid_command(&command) {
        return Err(CodecError::BadCommand(command));
    }
    let length = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as usize;
    if length > MAX_PAYLOAD {
        return Err(CodecError::TooLarge(length));
    }
    Ok(Header {
        command,
        length,
        checksum: [raw[20], raw[21], raw[22], raw[23]],
    })
}

/// Checks the payload against its header's checksum.
pub fn verify_payload(header: &Header, payload: &[u8]) -> Result<(), CodecError> {
    if checksum(payload) == header.checksum {
        Ok(())
    } else {
        Err(CodecError::BadChecksum(header.command.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGTEST: u32 = 0xDCB7C1FC;

    #[test]
    fn round_trip() {
        let framed = encode(REGTEST, "dsa", &[1, 2, 3]).unwrap();
        assert_eq!(framed.len(), HEADER_LEN + 3);
        let header: [u8; HEADER_LEN] = framed[..HEADER_LEN].try_into().unwrap();
        let h = decode_header(REGTEST, &header).unwrap();
        assert_eq!(h.command, "dsa");
        assert_eq!(h.length, 3);
        verify_payload(&h, &framed[HEADER_LEN..]).unwrap();
        assert!(verify_payload(&h, &[1, 2, 4]).is_err());
    }

    #[test]
    fn empty_payload_checksum_is_cores() {
        // Core: the checksum of an empty payload is 0x5df6e0e2.
        let framed = encode(REGTEST, "verack", &[]).unwrap();
        assert_eq!(&framed[20..24], &[0x5d, 0xf6, 0xe0, 0xe2]);
    }

    #[test]
    fn rejects_bad_headers() {
        let framed = encode(REGTEST, "ping", &[0; 8]).unwrap();
        let header: [u8; HEADER_LEN] = framed[..HEADER_LEN].try_into().unwrap();
        assert!(matches!(
            decode_header(0xBD6B0CBF, &header),
            Err(CodecError::WrongMagic { .. })
        ));
        let mut junk = header;
        junk[9] = b'x'; // a byte after the command's NUL padding
        assert!(matches!(
            decode_header(REGTEST, &junk),
            Err(CodecError::BadCommand(_))
        ));
        assert!(encode(REGTEST, "thirteenchars", &[]).is_err());
        assert!(encode(REGTEST, "", &[]).is_err());
        let mut big = header;
        big[16..20].copy_from_slice(&((MAX_PAYLOAD as u32) + 1).to_le_bytes());
        assert!(matches!(
            decode_header(REGTEST, &big),
            Err(CodecError::TooLarge(_))
        ));
    }
}
