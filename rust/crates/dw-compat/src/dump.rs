//! Dash Core's `dumpwallet` text format (`src/wallet/rpc/backup.cpp`):
//! a parser that also reads the HD comment header Core's own `importwallet`
//! ignores, and a writer that produces Core's exact bytes.
//!
//! Layout written by `dumpwallet` (legacy wallets only):
//!
//! ```text
//! # Wallet dump created by Dash Core <version>
//! # * Created on <ISO8601>
//! # * Best block at time of backup was <height> (<hash>),
//! #   mined on <ISO8601>
//!
//! # mnemonic: <words>                      ┐
//! # mnemonic passphrase: <passphrase>      │ only for HD wallets
//!                                          │
//! # HD seed: <hex>                         │
//!                                          │
//! # extended private masterkey: <xprv>     │
//! # extended public masterkey: <xpub>      │
//!                                          │
//! # external chain counter: N              │ one pair per account,
//! # internal chain counter: M              │ or "# WARNING: ACCOUNT i IS MISSING!"
//!                                          ┘
//! <WIF> <time> label=<enc>|reserve=1|change=1 # addr=<P2PKH>[ hdkeypath=<path>]
//!
//! <script hex> <time|0> script=1 # addr=<P2SH>
//!
//! # End of dump
//! ```
//!
//! Keys are sorted by (birth time, key id); scripts by script id. Labels
//! are `%xx`-encoded (lower-case hex) for bytes ≤ 32, ≥ 128 and `%`.
//! Files written on Windows have CRLF line ends; the parser accepts both.
//!
//! The parser keeps secrets in `Zeroizing` containers. Nothing here checks
//! that WIFs, addresses and hdkeypaths agree; `KeyEntry::secret` decodes a
//! WIF when the caller needs it.

use dw_uri::Network;
use dw_uri::keyio;
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// The four header lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpHeader {
    /// Text after "created by ", e.g. `Dash Core v24.0.0`.
    pub created_by: String,
    /// ISO 8601 creation time.
    pub created_on: String,
    pub best_block_height: i64,
    pub best_block_hash: String,
    /// ISO 8601 time of the best block.
    pub best_block_time: String,
}

/// One HD account's chain counters, or Core's "missing account" marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdAccount {
    Counters { external: u32, internal: u32 },
    Missing,
}

/// The HD section. Every field is secret except the counters.
#[derive(Clone, PartialEq, Eq)]
pub struct HdSection {
    pub mnemonic: Zeroizing<String>,
    /// Written verbatim; may contain any character except a newline.
    pub mnemonic_passphrase: Zeroizing<String>,
    /// Hex of the HD seed: the BIP39 seed, or a raw `sethdseed` seed.
    pub hd_seed_hex: Zeroizing<String>,
    pub xprv: Zeroizing<String>,
    pub xpub: String,
    pub accounts: Vec<HdAccount>,
}

impl std::fmt::Debug for HdSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HdSection")
            .field("xpub", &self.xpub)
            .field("accounts", &self.accounts)
            .finish_non_exhaustive()
    }
}

/// Why a key was in the wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRole {
    /// Has an address-book entry (possibly with an empty label).
    Label(String),
    /// In the keypool, never handed out.
    Reserve,
    /// Neither: used as change.
    Change,
}

/// One private key line.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub wif: Zeroizing<String>,
    /// ISO 8601 birth time as written (`1970-01-01T00:00:01Z` for imported
    /// keys without a time).
    pub time: String,
    pub role: KeyRole,
    /// P2PKH address of the key.
    pub address: String,
    /// `m/44'/…` for HD keys.
    pub hdkeypath: Option<String>,
}

impl std::fmt::Debug for KeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyEntry")
            .field("time", &self.time)
            .field("role", &self.role)
            .field("address", &self.address)
            .field("hdkeypath", &self.hdkeypath)
            .finish_non_exhaustive()
    }
}

impl KeyEntry {
    /// The decoded private key, if the WIF is valid for `network`.
    pub fn secret(&self, network: Network) -> Option<keyio::Secret> {
        keyio::decode_secret(&self.wif, network)
    }

    /// True for keys derived from the HD seed.
    pub fn is_hd(&self) -> bool {
        self.hdkeypath.is_some()
    }
}

/// One redeem-script line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntry {
    pub script_hex: String,
    /// ISO 8601 creation time; `None` when Core wrote `0`.
    pub time: Option<String>,
    /// P2SH address of the script.
    pub address: String,
}

/// A whole dump file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpFile {
    pub header: DumpHeader,
    pub hd: Option<HdSection>,
    pub keys: Vec<KeyEntry>,
    pub scripts: Vec<ScriptEntry>,
}

/// A line the parser could not read (1-based line number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub reason: &'static str,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

impl std::error::Error for ParseError {}

/// `EncodeDumpString`.
pub fn encode_dump_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c <= 32 || c >= 128 || c == b'%' {
            let _ = write!(out, "%{c:02x}");
        } else {
            out.push(char::from(c));
        }
    }
    out
}

/// `DecodeDumpString`, including its arithmetic on non-hex characters
/// (Core computes `(c >> 6) * 9 + ((c - '0') & 15)` on a signed `char`
/// without checking that `c` is a hex digit). Bytes that do not form UTF-8
/// are replaced with U+FFFD.
pub fn decode_dump_string(s: &str) -> String {
    let b = s.as_bytes();
    let nibble = |c: u8| -> i32 {
        let c = i32::from(c as i8);
        (c >> 6) * 9 + ((c - i32::from(b'0')) & 15)
    };
    let mut out = Vec::with_capacity(b.len());
    let mut pos = 0;
    while pos < b.len() {
        let mut c = b[pos];
        if c == b'%' && pos + 2 < b.len() {
            c = ((nibble(b[pos + 1]) << 4) | nibble(b[pos + 2])) as u8;
            pos += 2;
        }
        out.push(c);
        pos += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn header_value<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)
}

/// Parses a dump file. Lines that are neither comments nor key/script lines
/// are skipped, as Core's `importwallet` skips them.
pub fn parse(text: &str) -> Result<DumpFile, ParseError> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let err = |line: usize, reason| ParseError {
        line: line + 1,
        reason,
    };

    let created_by = lines
        .first()
        .and_then(|l| header_value(l, "# Wallet dump created by "))
        .ok_or_else(|| {
            err(
                0,
                "not a dumpwallet file: missing \"# Wallet dump created by\"",
            )
        })?
        .to_owned();
    let created_on = lines
        .get(1)
        .and_then(|l| header_value(l, "# * Created on "))
        .ok_or_else(|| err(1, "missing \"# * Created on\""))?
        .to_owned();
    let best = lines
        .get(2)
        .and_then(|l| header_value(l, "# * Best block at time of backup was "))
        .and_then(|l| l.strip_suffix("),"))
        .ok_or_else(|| err(2, "missing best block line"))?;
    let (height, hash) = best
        .split_once(" (")
        .ok_or_else(|| err(2, "malformed best block line"))?;
    let best_block_height = height
        .parse()
        .map_err(|_| err(2, "malformed best block height"))?;
    let best_block_time = lines
        .get(3)
        .and_then(|l| header_value(l, "#   mined on "))
        .ok_or_else(|| err(3, "missing \"#   mined on\""))?
        .to_owned();
    let header = DumpHeader {
        created_by,
        created_on,
        best_block_height,
        best_block_hash: hash.to_owned(),
        best_block_time,
    };

    let mut mnemonic = None;
    let mut passphrase = None;
    let mut seed = None;
    let mut xprv = None;
    let mut xpub = None;
    let mut accounts = Vec::new();
    let mut pending_external: Option<u32> = None;
    let mut keys = Vec::new();
    let mut scripts = Vec::new();

    for (n, line) in lines.iter().enumerate().skip(4) {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            if let Some(v) = rest.strip_prefix(" mnemonic passphrase: ") {
                passphrase = Some(Zeroizing::new(v.to_owned()));
            } else if let Some(v) = rest.strip_prefix(" mnemonic: ") {
                mnemonic = Some(Zeroizing::new(v.to_owned()));
            } else if let Some(v) = rest.strip_prefix(" HD seed: ") {
                seed = Some(Zeroizing::new(v.to_owned()));
            } else if let Some(v) = rest.strip_prefix(" extended private masterkey: ") {
                xprv = Some(Zeroizing::new(v.to_owned()));
            } else if let Some(v) = rest.strip_prefix(" extended public masterkey: ") {
                xpub = Some(v.to_owned());
            } else if let Some(v) = rest.strip_prefix(" external chain counter: ") {
                pending_external = Some(
                    v.parse()
                        .map_err(|_| err(n, "malformed external chain counter"))?,
                );
            } else if let Some(v) = rest.strip_prefix(" internal chain counter: ") {
                let internal = v
                    .parse()
                    .map_err(|_| err(n, "malformed internal chain counter"))?;
                let external = pending_external
                    .take()
                    .ok_or_else(|| err(n, "internal counter without external"))?;
                accounts.push(HdAccount::Counters { external, internal });
            } else if rest.starts_with(" WARNING: ACCOUNT ") && rest.ends_with(" IS MISSING!") {
                accounts.push(HdAccount::Missing);
            }
            continue;
        }
        let (fields, comment) = match line.find(" # ") {
            Some(i) => (&line[..i], Some(&line[i + 3..])),
            None => (*line, None),
        };
        let tokens: Vec<&str> = fields.split(' ').collect();
        if tokens.len() < 2 {
            continue;
        }
        let mut address = String::new();
        let mut hdkeypath = None;
        for item in comment.unwrap_or_default().split(' ') {
            if let Some(a) = item.strip_prefix("addr=") {
                address = a.to_owned();
            } else if let Some(p) = item.strip_prefix("hdkeypath=") {
                hdkeypath = Some(p.to_owned());
            }
        }
        // importwallet tries the token as a WIF first, then as hex. WIFs are
        // Base58 and never all-hex in practice, so hex means a script.
        if tokens[0].len().is_multiple_of(2) && tokens[0].bytes().all(|b| b.is_ascii_hexdigit()) {
            let time = (tokens[1] != "0").then(|| tokens[1].to_owned());
            scripts.push(ScriptEntry {
                script_hex: tokens[0].to_owned(),
                time,
                address,
            });
            continue;
        }
        // importwallet's flag rules: a key gets an address-book entry (empty
        // label unless `label=` gives one) unless `change=1` or `reserve=1`
        // comes after the last `label=`.
        let mut role = KeyRole::Label(String::new());
        for t in &tokens[2..] {
            if *t == "change=1" {
                role = KeyRole::Change;
            } else if *t == "reserve=1" {
                role = KeyRole::Reserve;
            } else if let Some(l) = t.strip_prefix("label=") {
                role = KeyRole::Label(decode_dump_string(l));
            }
        }
        keys.push(KeyEntry {
            wif: Zeroizing::new(tokens[0].to_owned()),
            time: tokens[1].to_owned(),
            role,
            address,
            hdkeypath,
        });
    }

    let hd = match (mnemonic, passphrase, seed, xprv, xpub) {
        (None, None, None, None, None) => None,
        (Some(mnemonic), Some(mnemonic_passphrase), Some(hd_seed_hex), Some(xprv), Some(xpub)) => {
            Some(HdSection {
                mnemonic,
                mnemonic_passphrase,
                hd_seed_hex,
                xprv,
                xpub,
                accounts,
            })
        }
        _ => {
            return Err(ParseError {
                line: 0,
                reason: "incomplete HD section",
            });
        }
    };
    Ok(DumpFile {
        header,
        hd,
        keys,
        scripts,
    })
}

/// Sort key Core uses for key lines: (birth time, CKeyID bytes). The key id
/// is the HASH160 in the P2PKH address; lines whose address does not decode
/// sort after the others with the same time.
fn key_sort_key(k: &KeyEntry, network: Network) -> (i64, [u8; 20]) {
    let id = match keyio::decode_destination(&k.address, network) {
        Ok(keyio::Destination::PubKeyHash(h)) => h,
        _ => [0xff; 20],
    };
    (parse_iso8601(&k.time).unwrap_or(i64::MAX), id)
}

fn script_sort_key(s: &ScriptEntry, network: Network) -> [u8; 20] {
    match keyio::decode_destination(&s.address, network) {
        Ok(keyio::Destination::ScriptHash(h)) => h,
        _ => [0xff; 20],
    }
}

/// Writes `dump` in Core's format. Keys and scripts are written in Core's
/// order (time then key id; script id), whatever order `dump` holds them in.
/// The result is `Zeroizing` because it contains every private key.
pub fn write(dump: &DumpFile, network: Network) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::new());
    let h = &dump.header;
    let _ = writeln!(out, "# Wallet dump created by {}", h.created_by);
    let _ = writeln!(out, "# * Created on {}", h.created_on);
    let _ = writeln!(
        out,
        "# * Best block at time of backup was {} ({}),",
        h.best_block_height, h.best_block_hash
    );
    let _ = writeln!(out, "#   mined on {}", h.best_block_time);
    out.push('\n');
    if let Some(hd) = &dump.hd {
        let _ = writeln!(out, "# mnemonic: {}", hd.mnemonic.as_str());
        let _ = write!(
            out,
            "# mnemonic passphrase: {}\n\n",
            hd.mnemonic_passphrase.as_str()
        );
        let _ = write!(out, "# HD seed: {}\n\n", hd.hd_seed_hex.as_str());
        let _ = writeln!(out, "# extended private masterkey: {}", hd.xprv.as_str());
        let _ = write!(out, "# extended public masterkey: {}\n\n", hd.xpub);
        for (i, acc) in hd.accounts.iter().enumerate() {
            match acc {
                HdAccount::Counters { external, internal } => {
                    let _ = write!(
                        out,
                        "# external chain counter: {external}\n# internal chain counter: {internal}\n\n"
                    );
                }
                HdAccount::Missing => {
                    let _ = write!(out, "# WARNING: ACCOUNT {i} IS MISSING!\n\n");
                }
            }
        }
    }
    let mut keys: Vec<&KeyEntry> = dump.keys.iter().collect();
    keys.sort_by_cached_key(|k| key_sort_key(k, network));
    for k in keys {
        let role = match &k.role {
            KeyRole::Label(l) => format!("label={}", encode_dump_string(l)),
            KeyRole::Reserve => "reserve=1".to_owned(),
            KeyRole::Change => "change=1".to_owned(),
        };
        let path = k
            .hdkeypath
            .as_ref()
            .map(|p| format!(" hdkeypath={p}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{} {} {role} # addr={}{path}",
            k.wif.as_str(),
            k.time,
            k.address
        );
    }
    out.push('\n');
    let mut scripts: Vec<&ScriptEntry> = dump.scripts.iter().collect();
    scripts.sort_by_cached_key(|s| script_sort_key(s, network));
    for s in scripts {
        let _ = writeln!(
            out,
            "{} {} script=1 # addr={}",
            s.script_hex,
            s.time.as_deref().unwrap_or("0"),
            s.address
        );
    }
    out.push('\n');
    out.push_str("# End of dump\n");
    out
}

/// Builds the HD section Core writes for a seed: the xprv/xpub of the BIP32
/// master key and the given account counters.
pub fn hd_section_from_seed(
    mnemonic: &str,
    mnemonic_passphrase: &str,
    seed: &[u8],
    accounts: Vec<HdAccount>,
    network: Network,
) -> Result<HdSection, key_wallet::bip32::Error> {
    let master = key_wallet::ExtendedPrivKey::new_master(network, seed)?;
    let xpub = key_wallet::ExtendedPubKey::from_priv(&master);
    let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
    Ok(HdSection {
        mnemonic: Zeroizing::new(mnemonic.to_owned()),
        mnemonic_passphrase: Zeroizing::new(mnemonic_passphrase.to_owned()),
        hd_seed_hex: Zeroizing::new(hex),
        xprv: Zeroizing::new(master.to_string()),
        xpub: xpub.to_string(),
        accounts,
    })
}

/// `FormatISO8601DateTime`: `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub fn format_iso8601(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ` to Unix seconds. `None` for anything else.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = &s[r];
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())
            .flatten()
    };
    let (y, mo, d, h, mi, se) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se)
}

// Howard Hinnant's civil-date algorithms (proleptic Gregorian calendar).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_string_codec() {
        assert_eq!(
            encode_dump_string("spaces and % and # and \u{e9}"),
            "spaces%20and%20%25%20and%20#%20and%20%c3%a9"
        );
        assert_eq!(
            decode_dump_string("spaces%20and%20%25%20and%20#%20and%20%c3%a9"),
            "spaces and % and # and \u{e9}"
        );
        assert_eq!(decode_dump_string("tab%09here"), "tab\there");
        // Core needs two characters after the % *and* one more before the end.
        assert_eq!(decode_dump_string("a%4"), "a%4");
        assert_eq!(decode_dump_string("%41"), "A");
        // Non-hex digits go through the same arithmetic: (4 << 4) | (9 + 7).
        assert_eq!(decode_dump_string("%4G"), "P");
    }

    #[test]
    fn iso8601() {
        assert_eq!(format_iso8601(1), "1970-01-01T00:00:01Z");
        assert_eq!(parse_iso8601("2014-12-04T17:15:37Z"), Some(1_417_713_337));
        assert_eq!(format_iso8601(1_417_713_337), "2014-12-04T17:15:37Z");
        assert_eq!(parse_iso8601("0"), None);
    }
}
