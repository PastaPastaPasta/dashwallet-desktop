//! Records that cross the IPC boundary and more than one domain uses.
//! Domain-only records live in their `api/` module.
//!
//! Conventions (E0-13 D-rows "types" and "numbers"):
//!
//! - Every record mirrors an engine record and derives `ts_rs::TS`, so the
//!   renderer's types are generated (`apps/desktop/src/api/generated`).
//! - Fields and enum values are camelCase.
//! - A `From` between a mirror and its engine record destructures every
//!   field (no `..`) and matches every variant, so an engine change fails to
//!   compile here instead of dropping a field silently.
//! - Wallet ids are lower-case hex, identity ids Base58, txids hex.
//! - Duffs are JSON numbers: 21e14 duffs, the most that can exist, is far
//!   below 2^53. Credits are decimal strings ([`credits`]): 1 DASH is 1e11
//!   credits, so an identity's balance can pass 2^53.

pub mod governance;
pub mod history;
pub mod platform;

use dw_engine::{
    DashNetwork, SyncPhase, SyncPhaseProgress, SyncSnapshot, WalletBalances, WalletInfo,
};
use dw_vault::{LockState, VaultStatus};
use serde::{Deserialize, Serialize};
use tauri::Theme;
use ts_rs::TS;

use crate::error::AppError;
use crate::launch::LaunchOptions;

/// `impl From<$from> for $to` for two enums with the same unit variants. A
/// variant missing on either side is a compile error.
macro_rules! mirror_enum {
    ($from:path => $to:ident { $($v:ident),+ $(,)? }) => {
        impl From<$from> for $to {
            fn from(x: $from) -> Self {
                use $from as From_;
                match x {
                    $(From_::$v => $to::$v),+
                }
            }
        }
    };
}
pub(crate) use mirror_enum;

/// `s` cut so that it encodes (as a JSON string, quotes aside) to at most
/// `max` bytes (`max` ≥ 64), at a character boundary, ending with a marker
/// that says how many bytes were cut. Engine text the app queues or keeps
/// passes through it, so no payload grows with what the engine sends
/// (E0-13 D3); counting the escapes, not the bytes, bounds the encoded
/// event too (and what the webview receives within a constant factor:
/// Tauri may escape a large payload once more). Reads at most about `max`
/// characters of `s`.
pub fn cap_text(mut s: String, max: usize) -> String {
    debug_assert!(max >= 64, "cap_text needs room for its marker");
    // "… [+N bytes]" with N up to 20 digits is at most 33 bytes, no escapes.
    let budget = max.saturating_sub(40);
    let (mut encoded, mut end) = (0, None);
    for (i, c) in s.char_indices() {
        encoded += json_len(c);
        if encoded > budget && end.is_none() {
            end = Some(i);
        }
        if encoded > max {
            break;
        }
    }
    let Some(end) = end.filter(|_| encoded > max) else {
        return s;
    };
    let cut = s.len() - end;
    s.truncate(end);
    s.push_str(&format!("… [+{cut} bytes]"));
    s
}

/// The bytes `c` takes in a JSON string as serde_json writes it.
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// Credit amounts as decimal strings: `#[serde(with = "crate::dto::credits")]`
/// with `#[ts(type = "string")]` (and `credits::option` with
/// `#[ts(type = "string | null")]`).
pub mod credits {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }

    pub mod option {
        use serde::{Deserialize, Deserializer, Serializer, de::Error};

        pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(v) => s.collect_str(v),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
            Option::<String>::deserialize(d)?
                .map(|s| s.parse().map_err(D::Error::custom))
                .transpose()
        }
    }
}

/// Serde for bytes as a base64 string (standard alphabet, padded):
/// `#[ts(type = "string")] #[serde(with = "crate::dto::base64")]`. JSON
/// would otherwise carry `Vec<u8>` as an array of numbers.
pub mod base64 {
    use ::base64::Engine as _;
    use ::base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

/// A transaction output reference, `txid:vout`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OutPoint {
    /// 64 lower-case hex characters, display byte order.
    pub txid: String,
    pub vout: u32,
}

impl OutPoint {
    /// The engine outpoint; the txid must be 64 lower-case hex characters.
    pub fn to_core(&self) -> Result<dashcore::OutPoint, AppError> {
        Ok(dashcore::OutPoint::new(parse_txid(&self.txid)?, self.vout))
    }

    /// A list of outpoints as the engine takes it.
    pub fn to_core_all(list: &[OutPoint]) -> Result<Vec<dashcore::OutPoint>, AppError> {
        list.iter().map(OutPoint::to_core).collect()
    }
}

impl From<dashcore::OutPoint> for OutPoint {
    fn from(o: dashcore::OutPoint) -> Self {
        let dashcore::OutPoint { txid, vout } = o;
        OutPoint {
            txid: txid.to_string(),
            vout,
        }
    }
}

/// A txid as the renderer sends it: 64 lower-case hex characters, display
/// byte order.
pub fn parse_txid(s: &str) -> Result<dashcore::Txid, AppError> {
    let lower_hex = s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    lower_hex
        .then(|| s.parse::<dashcore::Txid>().ok())
        .flatten()
        .ok_or_else(|| AppError::invalid_argument(format!("txid {s:?} is not 64 lowercase hex")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NetworkKind {
    Mainnet,
    Testnet,
    Devnet,
    Regtest,
}

/// A network as the renderer names it: `mainnet`, `testnet`, `regtest` or
/// `devnet-<name>` (the engine's directory name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub kind: NetworkKind,
    /// `devnet-<name>` for devnets, else the kind.
    pub dir_name: String,
    /// dash-qt's unit name on this network: `DASH` on mainnet, `tDASH` elsewhere (QT-003).
    pub unit_name: String,
}

impl From<&DashNetwork> for NetworkInfo {
    fn from(n: &DashNetwork) -> Self {
        let kind = match n {
            DashNetwork::Mainnet => NetworkKind::Mainnet,
            DashNetwork::Testnet => NetworkKind::Testnet,
            DashNetwork::Devnet { .. } => NetworkKind::Devnet,
            DashNetwork::Regtest => NetworkKind::Regtest,
        };
        NetworkInfo {
            kind,
            dir_name: n.dir_name(),
            unit_name: if kind == NetworkKind::Mainnet {
                "DASH"
            } else {
                "tDASH"
            }
            .into(),
        }
    }
}

/// Parses a network's directory name (`NetworkInfo.dirName`).
pub fn parse_network(name: &str) -> Result<DashNetwork, AppError> {
    let network = match name {
        "mainnet" => DashNetwork::Mainnet,
        "testnet" => DashNetwork::Testnet,
        "regtest" => DashNetwork::Regtest,
        other => match other.strip_prefix("devnet-") {
            Some(devnet) => DashNetwork::Devnet {
                name: devnet.into(),
            },
            None => {
                return Err(AppError::invalid_argument(format!(
                    "unknown network {other:?}"
                )));
            }
        },
    };
    network.validate()?;
    Ok(network)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThemeName {
    Light,
    Dark,
}

mirror_enum!(ThemeName => Theme { Light, Dark });

/// What the renderer needs to know before its first query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LaunchInfo {
    pub selftest: bool,
    /// Milliseconds the self-test waits on each screen (0 = no pause).
    pub selftest_pause_ms: u32,
    /// The self-test can unlock the vault (`selftest_unlock`): the app was
    /// launched with `--selftest-passphrase-file`.
    pub selftest_unlock: bool,
    /// The renderer reports each link it routes (`selftest_link_routed`):
    /// the app was launched with `--selftest-link-trace`.
    pub selftest_link_trace: bool,
    pub theme: Option<ThemeName>,
    /// The network `open_network` opens when it names none.
    pub network: NetworkInfo,
    pub app_version: String,
    pub engine_version: String,
}

impl LaunchInfo {
    pub fn new(opts: &LaunchOptions) -> Self {
        LaunchInfo {
            selftest: opts.selftest.is_some(),
            selftest_pause_ms: opts.selftest_pause_ms,
            selftest_unlock: opts.selftest.is_some() && opts.selftest_passphrase_file.is_some(),
            selftest_link_trace: opts.selftest_link_trace.is_some(),
            theme: opts.theme,
            network: NetworkInfo::from(&opts.network),
            app_version: env!("CARGO_PKG_VERSION").into(),
            engine_version: dw_engine::ENGINE_VERSION.into(),
        }
    }
}

/// Balance buckets in duffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Balances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
    /// The fully mixed CoinJoin balance once computed; before that, the
    /// spendable balance of the CoinJoin accounts.
    pub coinjoin: u64,
}

impl From<WalletBalances> for Balances {
    fn from(b: WalletBalances) -> Self {
        let WalletBalances {
            confirmed,
            unconfirmed,
            immature,
            locked,
            total,
            coinjoin,
        } = b;
        Balances {
            confirmed,
            unconfirmed,
            immature,
            locked,
            total,
            coinjoin,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WalletSummary {
    pub wallet_id: String,
    pub name: String,
    /// No signing keys are available (the vault holds no seed for it).
    pub watch_only: bool,
    /// The vault holds the wallet's recovery phrase.
    pub has_mnemonic: bool,
    pub hd: bool,
    pub birth_height: Option<u32>,
    /// UNIX seconds the wallet was added on this device, when known.
    pub created_at: Option<u64>,
    /// `null` until the scan has processed the birth block: a zero would be a guess.
    pub balances: Option<Balances>,
}

impl From<WalletInfo> for WalletSummary {
    fn from(w: WalletInfo) -> Self {
        let WalletInfo {
            wallet_id,
            name,
            watch_only,
            has_mnemonic,
            hd,
            birth_height,
            created_at,
            balances,
        } = w;
        WalletSummary {
            wallet_id: wallet_id.to_string(),
            name,
            watch_only,
            has_mnemonic,
            hd,
            birth_height,
            created_at,
            balances: balances.map(Balances::from),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LockStateName {
    NoVault,
    NoKeys,
    Unencrypted,
    Locked,
    UnlockedMixingOnly,
    Unlocked,
}

mirror_enum!(LockState => LockStateName { NoVault, NoKeys, Unencrypted, Locked, UnlockedMixingOnly, Unlocked });

/// The vault's state (`Vault::status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LockStatus {
    pub state: LockStateName,
    /// A passphrase slot exists.
    pub encrypted: bool,
    /// Quick unlock (slot B) is enrolled.
    pub quick_unlock_enrolled: bool,
    pub failed_attempts: u32,
    /// Seconds until the next passphrase attempt is accepted, when throttled.
    pub retry_after_secs: Option<u64>,
    /// Wallets with a seed record, sorted.
    pub wallets_with_secrets: Vec<String>,
    /// A link stashed before the vault existed is still plaintext beside
    /// it: its seal failed or a crash stopped it (DP5-01 D26). The next
    /// unlock or stash call seals it. Absent when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(optional, as = "Option<bool>")]
    pub unsealed_links: bool,
}

impl From<VaultStatus> for LockStatus {
    fn from(s: VaultStatus) -> Self {
        let VaultStatus {
            state,
            encrypted,
            quick_unlock_enrolled,
            failed_attempts,
            retry_after_secs,
            wallets_with_secrets,
            unsealed_links,
        } = s;
        LockStatus {
            state: state.into(),
            encrypted,
            quick_unlock_enrolled,
            failed_attempts,
            retry_after_secs,
            wallets_with_secrets: wallets_with_secrets.iter().map(hex::encode).collect(),
            unsealed_links,
        }
    }
}

/// Rows a [`SyncStatus`] lists: one per [`SyncPhaseName`].
pub const MAX_SYNC_PHASES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SyncPhaseName {
    Headers,
    FilterHeaders,
    Filters,
    Masternodes,
}

mirror_enum!(SyncPhase => SyncPhaseName { Headers, FilterHeaders, Filters, Masternodes });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncPhaseStatus {
    pub phase: SyncPhaseName,
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    pub done: bool,
}

impl From<SyncPhaseProgress> for SyncPhaseStatus {
    fn from(p: SyncPhaseProgress) -> Self {
        let SyncPhaseProgress {
            phase,
            current_height,
            target_height,
            done,
        } = p;
        SyncPhaseStatus {
            phase: phase.into(),
            current_height,
            target_height,
            done,
        }
    }
}

/// Raw sync state; the renderer derives the percentage (damping is the host's job).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub running: bool,
    /// One row per phase, in phase order: at most [`MAX_SYNC_PHASES`].
    pub phases: Vec<SyncPhaseStatus>,
    pub active_phase: Option<SyncPhaseName>,
    pub tip_height: Option<u32>,
    /// UNIX seconds of the best header.
    pub tip_time: Option<u64>,
    pub chainlock_height: Option<u32>,
    pub connected_peers: u32,
    pub caught_up: bool,
    pub seconds_since_progress: Option<u64>,
}

impl From<SyncSnapshot> for SyncStatus {
    fn from(s: SyncSnapshot) -> Self {
        let SyncSnapshot {
            running,
            phases,
            active_phase,
            tip_height,
            tip_time,
            chainlock_height,
            connected_peers,
            caught_up,
            seconds_since_progress,
        } = s;
        SyncStatus {
            running,
            phases: phases
                .into_iter()
                .take(MAX_SYNC_PHASES)
                .map(Into::into)
                .collect(),
            active_phase: active_phase.map(Into::into),
            tip_height,
            tip_time,
            chainlock_height,
            connected_peers,
            caught_up,
            seconds_since_progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list caps that stand for "one per variant" count their enums:
    /// each match lists every variant, so a new one fails to compile here
    /// until its cap is looked at again.
    #[test]
    fn per_variant_caps_count_their_enums() {
        use SyncPhaseName as P;
        use platform::{BudgetPurposeName as B, MAX_LEASE_BUDGETS};
        let phases = |p: P| match p {
            P::Headers | P::FilterHeaders | P::Filters | P::Masternodes => 4,
        };
        assert_eq!(MAX_SYNC_PHASES, phases(P::Headers));
        let budgets = |b: B| match b {
            B::Funding | B::Credits | B::Spend | B::Crypto => 4,
        };
        assert_eq!(MAX_LEASE_BUDGETS, budgets(B::Funding));
    }

    #[test]
    fn cap_text_cuts_at_a_char_boundary_and_says_how_much() {
        assert_eq!(cap_text("short".into(), 64), "short");
        let exact = "y".repeat(64);
        assert_eq!(cap_text(exact.clone(), 64), exact);
        // Three-byte characters: the cut never splits one.
        let long = "€".repeat(10_000);
        let cut = cap_text(long, 1024);
        assert!(cut.len() <= 1024, "{}", cut.len());
        let (kept, marker) = cut.split_once('…').unwrap();
        assert!(kept.chars().all(|c| c == '€'));
        assert_eq!(marker, format!(" [+{} bytes]", 30_000 - kept.len()));
        // Escapes count as what they encode to: a control character is six
        // bytes in JSON, a newline or a quote two.
        for c in ['\u{1}', '\n', '"', 'x'] {
            let cut = cap_text(c.to_string().repeat(10_000), 1024);
            let encoded = serde_json::to_string(&cut).unwrap().len() - 2;
            assert!((1024 - 46..=1024).contains(&encoded), "{c:?}: {encoded}");
        }
        let fits = "\n".repeat(512);
        assert_eq!(cap_text(fits.clone(), 1024), fits);
    }

    #[test]
    fn outpoints_round_trip_and_reject_bad_txids() {
        let o = OutPoint {
            txid: "0a".repeat(32),
            vout: 7,
        };
        let core = o.to_core().unwrap();
        assert_eq!(OutPoint::from(core), o);
        for bad in ["", "zz", &"0A".repeat(32), &"0a".repeat(31), "not hex"] {
            let o = OutPoint {
                txid: bad.to_string(),
                vout: 0,
            };
            assert_eq!(o.to_core().unwrap_err().code_str(), "invalid_argument");
            assert_eq!(parse_txid(bad).unwrap_err().code_str(), "invalid_argument");
        }
        let hex = "00".repeat(31) + "01";
        assert_eq!(parse_txid(&hex).unwrap().to_string(), hex);
    }

    #[test]
    fn an_outpoint_crosses_as_txid_and_vout() {
        let o = dashcore::OutPoint {
            txid: parse_txid(&("11".repeat(32))).unwrap(),
            vout: 3,
        };
        let json = serde_json::to_value(OutPoint::from(o)).unwrap();
        assert_eq!(json["txid"], "11".repeat(32));
        assert_eq!(json["vout"], 3);
    }

    #[test]
    fn networks_parse_from_their_directory_names() {
        for n in [
            DashNetwork::Mainnet,
            DashNetwork::Testnet,
            DashNetwork::Regtest,
            DashNetwork::Devnet {
                name: "paloma".into(),
            },
        ] {
            assert_eq!(parse_network(&n.dir_name()).unwrap(), n);
        }
        assert_eq!(
            parse_network("signet").unwrap_err().code_str(),
            "invalid_argument"
        );
    }

    #[test]
    fn credits_cross_as_strings() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct C {
            #[serde(with = "credits")]
            a: u64,
            #[serde(with = "credits::option")]
            b: Option<u64>,
        }
        let c = C {
            a: u64::MAX,
            b: None,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#"{"a":"18446744073709551615","b":null}"#);
        assert_eq!(serde_json::from_str::<C>(&json).unwrap(), c);
    }

    #[test]
    fn bytes_cross_as_base64() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct B(#[serde(with = "super::base64")] Vec<u8>);
        let json = serde_json::to_string(&B(vec![0, 255, 7])).unwrap();
        assert_eq!(json, r#""AP8H""#);
        assert_eq!(
            serde_json::from_str::<B>(&json).unwrap(),
            B(vec![0, 255, 7])
        );
        assert!(serde_json::from_str::<B>(r#""not base64!""#).is_err());
    }
}
