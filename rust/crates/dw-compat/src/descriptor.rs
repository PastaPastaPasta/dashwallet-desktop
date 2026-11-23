//! The output descriptors Dash Core descriptor wallets use, as far as this
//! wallet needs them: the checksum (`src/script/descriptor.cpp`
//! `DescriptorChecksum`), single-key `pkh(KEY/path/*)` descriptors over one
//! BIP32 master key (what `createwallet` / `upgradetohd` write), the
//! `listdescriptors true` JSON and the `importdescriptors` request JSON.
//!
//! Dash Core v24 descriptor wallets hold three of them, all rooted at the
//! BIP32 master key: `44h/<coin>h/0h/0/*` (receive, active external),
//! `44h/<coin>h/0h/1/*` (change, active internal) and the CoinJoin
//! `9h/<coin>h/4h/0h/0/*` (inactive, `"coinjoin": true`).

use serde::Deserialize;
use zeroize::Zeroizing;

const INPUT_CHARSET: &str = "0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
const CHECKSUM_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const GENERATOR: [u64; 5] = [
    0xf5dee51989,
    0xa9fdca3312,
    0x1bab10e32d,
    0x3706b1677a,
    0x644d626ffd,
];

fn polymod(symbols: impl IntoIterator<Item = u64>) -> u64 {
    let mut chk: u64 = 1;
    for value in symbols {
        let top = chk >> 35;
        chk = ((chk & 0x7_ffff_ffff) << 5) ^ value;
        for (i, g) in GENERATOR.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn expand(s: &str) -> Option<Vec<u64>> {
    let mut symbols = Vec::with_capacity(s.len() * 2);
    let mut groups = Vec::with_capacity(3);
    for c in s.chars() {
        let v = INPUT_CHARSET.find(c)? as u64;
        symbols.push(v & 31);
        groups.push(v >> 5);
        if groups.len() == 3 {
            symbols.push(groups[0] * 9 + groups[1] * 3 + groups[2]);
            groups.clear();
        }
    }
    match groups.len() {
        1 => symbols.push(groups[0]),
        2 => symbols.push(groups[0] * 3 + groups[1]),
        _ => {}
    }
    Some(symbols)
}

/// The 8-character checksum of `desc` (without `#`). `None` when `desc` has a
/// character outside Core's input charset.
pub fn checksum(desc: &str) -> Option<String> {
    let mut symbols = expand(desc)?;
    symbols.extend([0u64; 8]);
    let c = polymod(symbols) ^ 1;
    Some(
        (0..8)
            .map(|i| char::from(CHECKSUM_CHARSET[((c >> (5 * (7 - i))) & 31) as usize]))
            .collect(),
    )
}

/// `desc#checksum`.
pub fn with_checksum(desc: &str) -> Option<String> {
    Some(format!("{desc}#{}", checksum(desc)?))
}

/// Splits `desc#checksum` and verifies the checksum; a descriptor without one
/// is accepted as is.
pub fn strip_checksum(s: &str) -> Result<&str, DescriptorError> {
    match s.rsplit_once('#') {
        None => Ok(s),
        Some((desc, sum)) => {
            if checksum(desc).as_deref() == Some(sum) {
                Ok(desc)
            } else {
                Err(DescriptorError::BadChecksum)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptorError {
    BadChecksum,
    /// Not `pkh(KEY/path/*)` over a master key.
    Unsupported(String),
    /// The JSON is not `listdescriptors` output.
    Json(String),
}

impl std::fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadChecksum => f.write_str("descriptor checksum mismatch"),
            Self::Unsupported(d) => write!(f, "unsupported descriptor: {d}"),
            Self::Json(d) => write!(f, "not listdescriptors output: {d}"),
        }
    }
}

impl std::error::Error for DescriptorError {}

/// A `pkh(KEY/path/*)` descriptor rooted at a BIP32 master key.
#[derive(Clone, PartialEq, Eq)]
pub struct PkhDescriptor {
    /// The extended key as written (`xprv`/`tprv` or `xpub`/`tpub`). Secret
    /// when private.
    pub key: Zeroizing<String>,
    /// Path below the key, `h` for hardened steps as Core writes them, without
    /// the final `/*`.
    pub path: String,
}

impl std::fmt::Debug for PkhDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PkhDescriptor")
            .field("path", &self.path)
            .field("private", &self.is_private())
            .finish_non_exhaustive()
    }
}

impl PkhDescriptor {
    /// Parses `pkh(KEY/a/b/.../*)`, checksum optional. Key origins
    /// (`[fingerprint/path]KEY`) are refused: Core writes the master key
    /// without one, and a key with an origin is not a master key.
    pub fn parse(s: &str) -> Result<Self, DescriptorError> {
        let desc = strip_checksum(s.trim())?;
        let inner = desc
            .strip_prefix("pkh(")
            .and_then(|d| d.strip_suffix(')'))
            .ok_or_else(|| DescriptorError::Unsupported("not pkh(...)".into()))?;
        if inner.starts_with('[') {
            return Err(DescriptorError::Unsupported(
                "key origin: not a master key".into(),
            ));
        }
        let (key, rest) = inner
            .split_once('/')
            .ok_or_else(|| DescriptorError::Unsupported("no derivation path".into()))?;
        let path = rest
            .strip_suffix("/*")
            .ok_or_else(|| DescriptorError::Unsupported("not a ranged descriptor".into()))?;
        let step_ok = |s: &str| {
            let digits = s.strip_suffix(['h', '\'']).unwrap_or(s);
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        };
        if !path.split('/').all(step_ok) {
            return Err(DescriptorError::Unsupported(format!("path {path:?}")));
        }
        Ok(Self {
            key: Zeroizing::new(key.to_owned()),
            path: path.replace('\'', "h"),
        })
    }

    pub fn is_private(&self) -> bool {
        self.key.starts_with("xprv") || self.key.starts_with("tprv")
    }

    /// `pkh(KEY/path/*)#checksum`. Contains the private key when `key` is one.
    pub fn to_string_with_checksum(&self) -> Zeroizing<String> {
        let body = Zeroizing::new(format!("pkh({}/{}/*)", self.key.as_str(), self.path));
        // Base58 keys, digits, `h`, `/`, `*` and parentheses are all in the
        // input charset, so the checksum always exists.
        let sum = checksum(&body).unwrap_or_default();
        Zeroizing::new(format!("{}#{sum}", body.as_str()))
    }

    /// The `m/…` path of the chain (`m/44'/1'/0'/0`).
    pub fn chain_path(&self) -> String {
        format!("m/{}", self.path.replace('h', "'"))
    }
}

/// One entry of `listdescriptors true`.
#[derive(Clone, Deserialize)]
pub struct ListedDescriptor {
    pub desc: String,
    #[serde(default)]
    pub mnemonic: Option<String>,
    #[serde(default, rename = "mnemonicpassphrase")]
    pub mnemonic_passphrase: Option<String>,
    #[serde(default)]
    pub timestamp: Option<u64>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub internal: Option<bool>,
    #[serde(default)]
    pub coinjoin: Option<bool>,
    #[serde(default)]
    pub range: Option<(u32, u32)>,
    #[serde(default)]
    pub next_index: Option<u32>,
}

impl std::fmt::Debug for ListedDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListedDescriptor")
            .field("active", &self.active)
            .field("internal", &self.internal)
            .field("coinjoin", &self.coinjoin)
            .finish_non_exhaustive()
    }
}

impl Drop for ListedDescriptor {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.desc.zeroize();
        if let Some(m) = &mut self.mnemonic {
            m.zeroize();
        }
        if let Some(p) = &mut self.mnemonic_passphrase {
            p.zeroize();
        }
    }
}

#[derive(Deserialize)]
struct ListDescriptors {
    descriptors: Vec<ListedDescriptor>,
}

/// What a `listdescriptors true` dump says about the wallet's HD root.
pub struct ListedWallet {
    /// The active receive descriptor (BIP44 account 0, external chain).
    pub external: PkhDescriptor,
    /// The phrase and passphrase, when the wallet was created with one.
    pub mnemonic: Option<(Zeroizing<String>, Zeroizing<String>)>,
    /// Highest `next_index` of the active chains (addresses handed out).
    pub next_index: u32,
}

impl std::fmt::Debug for ListedWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListedWallet")
            .field("external", &self.external)
            .field("has_mnemonic", &self.mnemonic.is_some())
            .field("next_index", &self.next_index)
            .finish()
    }
}

/// Reads `listdescriptors true` output (the object, or its `descriptors`
/// array). Needs an active external `pkh` descriptor with a private master
/// key at `44h/<coin>h/0h/0`; every private descriptor must use that same key.
pub fn parse_listdescriptors(json: &[u8]) -> Result<ListedWallet, DescriptorError> {
    let listed: Vec<ListedDescriptor> = match serde_json::from_slice::<ListDescriptors>(json) {
        Ok(l) => l.descriptors,
        Err(_) => serde_json::from_slice::<Vec<ListedDescriptor>>(json)
            .map_err(|e| DescriptorError::Json(e.to_string()))?,
    };
    let mut external = None;
    let mut mnemonic = None;
    let mut next_index = 0;
    let mut keys = Vec::new();
    for d in &listed {
        let parsed = PkhDescriptor::parse(&d.desc)?;
        if !parsed.is_private() {
            return Err(DescriptorError::Unsupported(
                "public descriptor: listdescriptors needs `true` (private keys)".into(),
            ));
        }
        keys.push(parsed.key.clone());
        if d.active {
            next_index = next_index.max(d.next_index.unwrap_or(0));
        }
        if let (Some(m), true) = (&d.mnemonic, mnemonic.is_none()) {
            mnemonic = Some((
                Zeroizing::new(m.clone()),
                Zeroizing::new(d.mnemonic_passphrase.clone().unwrap_or_default()),
            ));
        }
        let is_receive = d.active
            && d.internal != Some(true)
            && d.coinjoin != Some(true)
            && parsed.path.starts_with("44h/")
            && parsed.path.ends_with("h/0h/0");
        if is_receive && external.is_none() {
            external = Some(parsed);
        }
    }
    let external = external.ok_or_else(|| {
        DescriptorError::Unsupported("no active pkh receive descriptor at 44h/<coin>h/0h/0".into())
    })?;
    if keys.iter().any(|k| k.as_str() != external.key.as_str()) {
        return Err(DescriptorError::Unsupported(
            "descriptors use more than one master key".into(),
        ));
    }
    Ok(ListedWallet {
        external,
        mnemonic,
        next_index,
    })
}

/// One request of `importdescriptors`.
pub struct ImportRequest {
    pub descriptor: PkhDescriptor,
    /// UNIX seconds to rescan from, or `None` for `"now"`.
    pub timestamp: Option<u64>,
    pub active: bool,
    pub internal: bool,
    /// Inclusive end of the range to import.
    pub range_end: u32,
    pub next_index: u32,
}

fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

/// The `importdescriptors` request array, one object per line, as dashd's
/// `importdescriptors '<json>'` takes it. Contains private keys when the
/// descriptors are private.
pub fn import_descriptors_json(requests: &[ImportRequest]) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::from("[\n"));
    for (i, r) in requests.iter().enumerate() {
        let ts = r
            .timestamp
            .map_or_else(|| "\"now\"".to_owned(), |t| t.to_string());
        let desc = r.descriptor.to_string_with_checksum();
        out.push_str(&format!(
            " {{\"desc\": {}, \"timestamp\": {ts}, \"active\": {}, \"internal\": {}, \"range\": [0, {}], \"next_index\": {}}}{}\n",
            json_string(&desc),
            r.active,
            r.internal,
            r.range_end,
            r.next_index,
            if i + 1 < requests.len() { "," } else { "" },
        ));
    }
    out.push_str("]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED: &str = include_str!("../../../../testdata/compat/listdescriptors_plain.json");

    #[test]
    fn checksums_match_dashd() {
        // Checksums dashd wrote into listdescriptors_plain.json.
        let v: serde_json::Value = serde_json::from_str(LISTED).unwrap();
        for d in v["descriptors"].as_array().unwrap() {
            let full = d["desc"].as_str().unwrap();
            let (body, sum) = full.rsplit_once('#').unwrap();
            assert_eq!(checksum(body).unwrap(), sum, "{body}");
            assert_eq!(strip_checksum(full).unwrap(), body);
        }
        assert_eq!(
            strip_checksum("pkh(tpub/0/*)#aaaaaaaa"),
            Err(DescriptorError::BadChecksum)
        );
        assert!(checksum("pkh(é)").is_none());
    }

    #[test]
    fn parses_listdescriptors_of_dashd() {
        let w = parse_listdescriptors(LISTED.as_bytes()).unwrap();
        assert_eq!(w.external.path, "44h/1h/0h/0");
        assert!(w.external.is_private());
        assert_eq!(w.external.chain_path(), "m/44'/1'/0'/0");
        let (m, p) = w.mnemonic.as_ref().unwrap();
        assert!(m.starts_with("abandon abandon"));
        assert_eq!(p.as_str(), "TREZOR");
        // dashd handed out four receive addresses and one change address.
        assert_eq!(w.next_index, 4);
        let v: serde_json::Value = serde_json::from_str(LISTED).unwrap();
        let first = v["descriptors"][0]["desc"].as_str().unwrap();
        assert_eq!(w.external.to_string_with_checksum().as_str(), first);
    }

    #[test]
    fn rejects_foreign_descriptors() {
        for bad in [
            "wpkh(tprv8ZgxMBicQKsPd/0/*)",
            "pkh([d34db33f/44h]tprv8ZgxMBicQKsPd/0/*)",
            "pkh(tprv8ZgxMBicQKsPd/0/1)",
            "pkh(tprv8ZgxMBicQKsPd/x/*)",
        ] {
            assert!(PkhDescriptor::parse(bad).is_err(), "{bad}");
        }
        let d = PkhDescriptor::parse("pkh(tpubX/44'/1'/0'/1/*)").unwrap();
        assert_eq!(d.path, "44h/1h/0h/1");
        assert!(!d.is_private());
        assert!(parse_listdescriptors(b"{}").is_err());
    }

    #[test]
    fn import_json_is_valid_json() {
        let req = ImportRequest {
            descriptor: PkhDescriptor::parse("pkh(tprvA/44h/1h/0h/0/*)").unwrap(),
            timestamp: Some(1_700_000_000),
            active: true,
            internal: false,
            range_end: 999,
            next_index: 3,
        };
        let json = import_descriptors_json(&[req]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let d = v[0]["desc"].as_str().unwrap();
        assert!(strip_checksum(d).is_ok());
        assert_eq!(v[0]["range"][1], 999);
        assert_eq!(v[0]["timestamp"], 1_700_000_000u64);
    }
}
