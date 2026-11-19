//! The masternodes facade's records (`docs/contracts/mn-engine.md` §5) and
//! the masternode keychain's, as the renderer sees them (MG-10 §3.1). Enum
//! values are snake_case and data enums are tagged by `kind`, as the engine
//! writes them; field names are camelCase. Credits are decimal strings.

use dw_engine::masternodes as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::governance::MasternodeTypeName;
use super::mirror_enum;
use crate::error::{AppError, EngineCode};

/// `From` an engine record by destructuring every field (no `..`), so a new
/// engine field fails to compile until its twin has it. `into`, `opt`, `vec`
/// and `optvec` convert the field; a bare field moves as is.
macro_rules! mirror_struct {
    ($from:path => $to:ident { $($field:ident $(: $conv:ident)?),+ $(,)? }) => {
        impl From<$from> for $to {
            fn from(x: $from) -> Self {
                use $from as From_;
                let From_ { $($field),+ } = x;
                $to { $($field: mirror_struct!(@ $field $($conv)?)),+ }
            }
        }
    };
    (@ $f:ident) => { $f };
    (@ $f:ident into) => { $f.into() };
    (@ $f:ident opt) => { $f.map(Into::into) };
    (@ $f:ident vec) => { $f.into_iter().map(Into::into).collect() };
    (@ $f:ident optvec) => { $f.map(|v| v.into_iter().map(Into::into).collect()) };
}

/// [`mirror_enum!`] both ways, for an enum the renderer also sends.
macro_rules! mirror_both {
    ($from:path => $to:ident { $($v:ident),+ $(,)? }) => {
        mirror_enum!($from => $to { $($v),+ });
        impl From<$to> for $from {
            fn from(x: $to) -> Self {
                use $from as Into_;
                match x {
                    $($to::$v => Into_::$v),+
                }
            }
        }
    };
}

mirror_enum!(engine::MasternodeType => MasternodeTypeName { Regular, Evonode });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeStatusName {
    Enabled,
    PoseBanned,
    Removed,
    Unconfirmed,
    Unknown,
}

mirror_enum!(engine::MasternodeStatus => MasternodeStatusName {
    Enabled, PoseBanned, Removed, Unconfirmed, Unknown,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AttachedKeyRoleName {
    Owner,
    Voting,
    Payout,
}

mirror_both!(engine::AttachedKeyRole => AttachedKeyRoleName { Owner, Voting, Payout });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum KeyCheckName {
    Matched,
    KeyMismatch,
    Unchecked,
}

mirror_enum!(engine::KeyCheck => KeyCheckName { Matched, KeyMismatch, Unchecked });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AttachedKey {
    pub role: AttachedKeyRoleName,
    pub key_id: Option<String>,
    pub check: KeyCheckName,
}

mirror_struct!(engine::AttachedKey => AttachedKey { role: into, key_id, check: into });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollateralStateName {
    Known { outpoint: String },
    Unresolved,
}

impl From<engine::CollateralState> for CollateralStateName {
    fn from(c: engine::CollateralState) -> Self {
        match c {
            engine::CollateralState::Known { outpoint } => Self::Known { outpoint },
            engine::CollateralState::Unresolved => Self::Unresolved,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeRoles {
    pub collateral: bool,
    pub owner: bool,
    pub voting: bool,
    pub payout: bool,
    pub operator: bool,
    /// `null` while the vault is locked.
    pub attached: Option<Vec<AttachedKey>>,
}

mirror_struct!(engine::MasternodeRoles => MasternodeRoles {
    collateral, owner, voting, payout, operator, attached: optvec,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListFacts {
    pub kind: MasternodeTypeName,
    pub valid: bool,
    pub service: Option<String>,
    pub operator_public_key: String,
    pub voting_address: String,
    pub platform_node_id: Option<String>,
    pub platform_http_port: Option<u16>,
}

mirror_struct!(engine::ListFacts => ListFacts {
    kind: into, valid, service, operator_public_key, voting_address, platform_node_id,
    platform_http_port,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationFacts {
    pub registered_height: Option<u32>,
    pub collateral: CollateralStateName,
    pub owner_address: Option<String>,
    pub voting_address: Option<String>,
    pub payout_address: Option<String>,
    pub payout_script: Option<String>,
    /// Basis points.
    pub operator_reward: Option<u16>,
    pub revoked: bool,
    pub revocation_reason: Option<u16>,
    pub transactions: u32,
}

mirror_struct!(engine::RegistrationFacts => RegistrationFacts {
    registered_height, collateral: into, owner_address, voting_address, payout_address,
    payout_script, operator_reward, revoked, revocation_reason, transactions,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodePayout {
    pub txid: String,
    pub record_index: u32,
    pub amount: u64,
    pub address: String,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
    pub matured: bool,
    /// Possibly a v24 share paid to the same script: no row's `payouts30d`
    /// counts it.
    pub unattributed: bool,
}

mirror_struct!(engine::MasternodePayout => MasternodePayout {
    txid, record_index, amount, address, height, timestamp, matured, unattributed,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HeldShares {
    pub held: u8,
    pub of: u8,
}

mirror_struct!(engine::HeldShares => HeldShares { held, of });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeRow {
    pub pro_tx_hash: String,
    pub kind: Option<MasternodeTypeName>,
    pub type_index: u32,
    pub label: Option<String>,
    pub status: MasternodeStatusName,
    pub banned_seen_at: Option<u64>,
    pub service: Option<String>,
    pub mine: bool,
    pub tracked: bool,
    pub shared_unsupported: bool,
    pub shares: Option<HeldShares>,
    pub roles: MasternodeRoles,
    pub collateral: CollateralStateName,
    pub collateral_released: bool,
    pub payout_address: Option<String>,
    pub payouts_30d: u64,
    pub payouts_incomplete: bool,
    pub last_payout: Option<MasternodePayout>,
}

mirror_struct!(engine::MasternodeRow => MasternodeRow {
    pro_tx_hash, kind: opt, type_index, label, status: into, banned_seen_at, service, mine,
    tracked, shared_unsupported, shares: opt, roles: into, collateral: into,
    collateral_released, payout_address, payouts_30d, payouts_incomplete, last_payout: opt,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeListState {
    pub synced: bool,
    pub height: Option<u32>,
}

mirror_struct!(engine::MasternodeListState => MasternodeListState { synced, height });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CollateralCoin {
    pub outpoint: String,
    pub address: String,
    pub amount: u64,
    pub kind: MasternodeTypeName,
    pub confirmations: u32,
    pub user_locked: bool,
}

mirror_struct!(engine::CollateralCoin => CollateralCoin {
    outpoint, address, amount, kind: into, confirmations, user_locked,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeOverview {
    pub list: MasternodeListState,
    pub masternodes: Vec<MasternodeRow>,
    pub collateral_coins: Vec<CollateralCoin>,
    pub payouts_30d: u64,
    pub unattributed_30d: u64,
    pub payouts_incomplete: bool,
}

mirror_struct!(engine::MasternodeOverview => MasternodeOverview {
    list: into, masternodes: vec, collateral_coins: vec, payouts_30d, unattributed_30d,
    payouts_incomplete,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RegistrarSignerName {
    Keychain,
    Attached,
}

mirror_enum!(engine::RegistrarSigner => RegistrarSignerName { Keychain, Attached });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistrarCapabilityName {
    Available { signer: RegistrarSignerName },
    RegistrationUnavailable,
    NoOwnerKey,
    VaultLocked,
    Shared,
}

impl From<engine::RegistrarCapability> for RegistrarCapabilityName {
    fn from(r: engine::RegistrarCapability) -> Self {
        use engine::RegistrarCapability as R;
        match r {
            R::Available { signer } => Self::Available {
                signer: signer.into(),
            },
            R::RegistrationUnavailable => Self::RegistrationUnavailable,
            R::NoOwnerKey => Self::NoOwnerKey,
            R::VaultLocked => Self::VaultLocked,
            R::Shared => Self::Shared,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeDetail {
    pub row: MasternodeRow,
    pub registrar: RegistrarCapabilityName,
    pub list: Option<ListFacts>,
    pub registration: Option<RegistrationFacts>,
    /// Always `null` until the ProTx-hash watch exists (MG-03 D5).
    pub payout_checked_height: Option<u32>,
    pub tracked_at: Option<u64>,
}

mirror_struct!(engine::MasternodeDetail => MasternodeDetail {
    row: into, registrar: into, list: opt, registration: opt, payout_checked_height, tracked_at,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ListedMasternode {
    pub pro_tx_hash: String,
    pub facts: ListFacts,
    pub tracked: bool,
}

mirror_struct!(engine::ListedMasternode => ListedMasternode { pro_tx_hash, facts: into, tracked });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UntrackOutcome {
    pub keys_deleted: u32,
}

mirror_struct!(engine::UntrackOutcome => UntrackOutcome { keys_deleted });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EvonodeCredits {
    pub pro_tx_hash: String,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub duffs: u64,
    pub fetched_at: u64,
}

mirror_struct!(engine::EvonodeCredits => EvonodeCredits { pro_tx_hash, credits, duffs, fetched_at });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeKeyRoleName {
    Owner,
    Voting,
    Operator,
    PlatformNode,
}

mirror_both!(dw_engine::MasternodeKeyRole => MasternodeKeyRoleName {
    Owner, Voting, Operator, PlatformNode,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeKeyInfo {
    pub role: MasternodeKeyRoleName,
    pub index: u32,
    pub derivation_path: String,
    pub address: Option<String>,
    /// An operator key in the basic BLS scheme.
    pub public_key_hex: String,
    /// An operator key in the legacy BLS scheme.
    pub legacy_public_key_hex: Option<String>,
    pub platform_node_id: Option<String>,
}

mirror_struct!(dw_engine::MasternodeKeyInfo => MasternodeKeyInfo {
    role: into, index, derivation_path, address, public_key_hex, legacy_public_key_hex,
    platform_node_id,
});

/// A revealed provider private key. Every field is secret.
#[derive(Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RevealedMasternodeKey {
    pub private_key_hex: String,
    pub wif: Option<String>,
    pub tenderdash_key: Option<String>,
}

impl std::fmt::Debug for RevealedMasternodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RevealedMasternodeKey(<redacted>)")
    }
}

impl TryFrom<dw_engine::RevealedMasternodeKey> for RevealedMasternodeKey {
    type Error = AppError;

    fn try_from(k: dw_engine::RevealedMasternodeKey) -> Result<Self, AppError> {
        let dw_engine::RevealedMasternodeKey {
            private_key_hex,
            wif,
            tenderdash_key,
        } = k;
        let text = |bytes: &[u8]| {
            std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| {
                AppError::new(EngineCode::Internal, "a revealed key is not ASCII text")
            })
        };
        Ok(RevealedMasternodeKey {
            private_key_hex: text(&private_key_hex)?,
            wif: wif.as_deref().map(|w| text(w)).transpose()?,
            tenderdash_key: tenderdash_key.as_deref().map(|t| text(t)).transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::*;

    #[test]
    fn enums_are_snake_case_and_tagged_by_kind() {
        assert_eq!(
            serde_json::to_value(MasternodeStatusName::PoseBanned).unwrap(),
            "pose_banned"
        );
        assert_eq!(
            serde_json::to_value(MasternodeKeyRoleName::PlatformNode).unwrap(),
            "platform_node"
        );
        assert_eq!(
            serde_json::to_value(RegistrarCapabilityName::from(
                engine::RegistrarCapability::Available {
                    signer: engine::RegistrarSigner::Attached
                }
            ))
            .unwrap(),
            serde_json::json!({ "kind": "available", "signer": "attached" })
        );
        assert_eq!(
            serde_json::to_value(RegistrarCapabilityName::from(
                engine::RegistrarCapability::RegistrationUnavailable
            ))
            .unwrap(),
            serde_json::json!({ "kind": "registration_unavailable" })
        );
        assert_eq!(
            serde_json::to_value(CollateralStateName::from(engine::CollateralState::Known {
                outpoint: format!("{}-1", "ab".repeat(32))
            }))
            .unwrap(),
            serde_json::json!({ "kind": "known", "outpoint": format!("{}-1", "ab".repeat(32)) })
        );
        for role in [
            AttachedKeyRoleName::Owner,
            AttachedKeyRoleName::Voting,
            AttachedKeyRoleName::Payout,
        ] {
            let back = AttachedKeyRoleName::from(engine::AttachedKeyRole::from(role));
            assert_eq!(back, role);
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                engine::AttachedKeyRole::from(role).vault_name()
            );
        }
    }

    #[test]
    fn fields_are_camel_case_and_credits_are_strings() {
        let credits = EvonodeCredits::from(engine::EvonodeCredits {
            pro_tx_hash: "cd".repeat(32),
            credits: u64::MAX,
            duffs: 7,
            fetched_at: 1,
        });
        assert_eq!(
            serde_json::to_value(&credits).unwrap(),
            serde_json::json!({
                "proTxHash": "cd".repeat(32),
                "credits": u64::MAX.to_string(),
                "duffs": 7,
                "fetchedAt": 1,
            })
        );
        let overview = MasternodeOverview::from(engine::MasternodeOverview {
            list: engine::MasternodeListState {
                synced: false,
                height: None,
            },
            masternodes: vec![],
            collateral_coins: vec![],
            payouts_30d: 5,
            unattributed_30d: 6,
            payouts_incomplete: true,
        });
        let v = serde_json::to_value(&overview).unwrap();
        assert_eq!(v["payouts30d"], 5);
        assert_eq!(v["unattributed30d"], 6);
        assert_eq!(v["collateralCoins"], serde_json::json!([]));
    }

    #[test]
    fn a_revealed_key_is_text_and_never_printed() {
        let secret = "11".repeat(32);
        let ok = RevealedMasternodeKey::try_from(dw_engine::RevealedMasternodeKey {
            private_key_hex: Zeroizing::new(secret.clone().into_bytes()),
            wif: None,
            tenderdash_key: Some(Zeroizing::new(b"AAAA".to_vec())),
        })
        .unwrap();
        assert_eq!(ok.private_key_hex, secret);
        assert_eq!(ok.tenderdash_key.as_deref(), Some("AAAA"));
        assert_eq!(format!("{ok:?}"), "RevealedMasternodeKey(<redacted>)");
        let err = RevealedMasternodeKey::try_from(dw_engine::RevealedMasternodeKey {
            private_key_hex: Zeroizing::new(vec![0xff]),
            wif: None,
            tenderdash_key: None,
        })
        .unwrap_err();
        assert_eq!(err.code_str(), "internal");
    }
}
