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
    pub fixture: bool,
    pub selftest: bool,
    /// Milliseconds the self-test waits on each screen (0 = no pause).
    pub selftest_pause_ms: u32,
    pub theme: Option<ThemeName>,
    /// The network `open_network` opens when it names none.
    pub network: NetworkInfo,
    pub app_version: String,
    pub engine_version: String,
}

impl LaunchInfo {
    pub fn new(opts: &LaunchOptions) -> Self {
        LaunchInfo {
            fixture: opts.fixture,
            selftest: opts.selftest.is_some(),
            selftest_pause_ms: opts.selftest_pause_ms,
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
        } = s;
        LockStatus {
            state: state.into(),
            encrypted,
            quick_unlock_enrolled,
            failed_attempts,
            retry_after_secs,
            wallets_with_secrets: wallets_with_secrets.iter().map(hex::encode).collect(),
        }
    }
}

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
    /// One row per phase, in phase order.
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
            phases: phases.into_iter().map(Into::into).collect(),
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
}
