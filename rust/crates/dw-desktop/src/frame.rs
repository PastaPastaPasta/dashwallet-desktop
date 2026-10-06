//! Wire format of a forwarded launch (QT-001): `DWI1`, a little-endian u32
//! argument count, then each argument as a u32 byte length and UTF-8 bytes.
//! The primary answers one byte, `1`, after it took the arguments.
//! Limits keep a hostile or broken peer from making the primary allocate
//! much or block.

use std::io::{Read, Write};

use crate::DesktopError;

const MAGIC: &[u8; 4] = b"DWI1";
/// Most arguments one launch may forward.
pub(crate) const MAX_ARGS: usize = 64;
/// Longest single argument, in bytes (a BIP21 URI with a long message).
pub(crate) const MAX_ARG_BYTES: usize = 8 * 1024;
/// Longest frame, in bytes.
const MAX_TOTAL_BYTES: usize = 64 * 1024;
pub(crate) const ACK: u8 = b'1';

/// Checks `args` against the limits before anything is sent.
pub(crate) fn check_args(args: &[String]) -> Result<(), DesktopError> {
    if args.len() > MAX_ARGS {
        return Err(DesktopError::InvalidArgument(format!(
            "{} arguments; at most {MAX_ARGS} can be forwarded",
            args.len()
        )));
    }
    let mut total = 0usize;
    for arg in args {
        if arg.len() > MAX_ARG_BYTES {
            return Err(DesktopError::InvalidArgument(format!(
                "an argument of {} bytes; at most {MAX_ARG_BYTES}",
                arg.len()
            )));
        }
        total += arg.len() + 4;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(DesktopError::InvalidArgument(format!(
            "{total} bytes of arguments; at most {MAX_TOTAL_BYTES}"
        )));
    }
    Ok(())
}

pub(crate) fn write(out: &mut impl Write, args: &[String]) -> Result<(), DesktopError> {
    check_args(args)?;
    let mut buf = Vec::with_capacity(8 + args.iter().map(|a| a.len() + 4).sum::<usize>());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for arg in args {
        buf.extend_from_slice(&(arg.len() as u32).to_le_bytes());
        buf.extend_from_slice(arg.as_bytes());
    }
    out.write_all(&buf)?;
    out.flush()?;
    Ok(())
}

/// Reads one frame; `None` for anything that is not a valid frame within
/// the limits.
pub(crate) fn read(input: &mut impl Read) -> Option<Vec<String>> {
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    let count = read_u32(input)? as usize;
    if count > MAX_ARGS {
        return None;
    }
    let mut total = 0usize;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        let len = read_u32(input)? as usize;
        total += len + 4;
        if len > MAX_ARG_BYTES || total > MAX_TOTAL_BYTES {
            return None;
        }
        let mut bytes = vec![0u8; len];
        input.read_exact(&mut bytes).ok()?;
        args.push(String::from_utf8(bytes).ok()?);
    }
    Some(args)
}

fn read_u32(input: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    input.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_limits() {
        let args = vec!["dash:yX?amount=1".to_string(), String::new(), "ü".into()];
        let mut buf = Vec::new();
        write(&mut buf, &args).unwrap();
        assert_eq!(read(&mut &buf[..]), Some(args));

        assert!(read(&mut &b"XXXX\0\0\0\0"[..]).is_none());
        // Truncated.
        assert!(read(&mut &buf[..buf.len() - 1]).is_none());
        // Too many arguments.
        let mut many = MAGIC.to_vec();
        many.extend_from_slice(&(MAX_ARGS as u32 + 1).to_le_bytes());
        assert!(read(&mut &many[..]).is_none());
        // One argument claiming 1 GiB.
        let mut huge = MAGIC.to_vec();
        huge.extend_from_slice(&1u32.to_le_bytes());
        huge.extend_from_slice(&(1u32 << 30).to_le_bytes());
        assert!(read(&mut &huge[..]).is_none());
        // Not UTF-8.
        let mut bad = MAGIC.to_vec();
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.push(0xff);
        assert!(read(&mut &bad[..]).is_none());

        assert!(check_args(&vec![String::new(); MAX_ARGS + 1]).is_err());
        assert!(check_args(&["x".repeat(MAX_ARG_BYTES + 1)]).is_err());
        assert!(check_args(&vec!["x".repeat(MAX_ARG_BYTES); 9]).is_err());
    }
}
