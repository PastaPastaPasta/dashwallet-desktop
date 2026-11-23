//! The wallet's identities and their keys.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::credits::CreditLevelName;
use crate::dto::mirror_enum;
use crate::dto::platform::{DashPayProfile, KeyPurposeName};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySummary {
    /// Base58.
    pub identity: String,
    /// `i` in `m/9'/c'/5'/0'/0'/i'`.
    pub index: u32,
    pub names: Vec<String>,
    pub main_name: Option<String>,
    /// `names` and `mainName` are not known right now (a sync pass holds the
    /// wallet's identity state): both are empty until a later read, and the
    /// UI shows "updating" rather than "no name".
    pub names_updating: bool,
    /// Set while `mainName` is `null` but Platform has not said this session
    /// that the identity owns no name: it may still own one, so say
    /// "updating" or "unknown", never "no username" (DP1-08 D23).
    pub names_check: Option<NamesCheckName>,
    pub is_main: bool,
    /// Credits; `null` while unknown.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub balance: Option<u64>,
    /// The Platform block time (ms) of `balance`, once refreshed.
    pub balance_as_of: Option<u64>,
    /// `balance` may be behind (never refreshed, older than ten minutes, or
    /// spent since); the engine refreshes it in the background.
    pub balance_stale: bool,
    pub has_dashpay_keys: bool,
    pub profile: Option<DashPayProfile>,
    /// Verified only through the trusted fallback.
    pub unverified: bool,
    /// The balance against the cost table; `null` while either is unknown.
    pub credit_level: Option<CreditLevelName>,
}

impl From<engine::IdentitySummary> for IdentitySummary {
    fn from(s: engine::IdentitySummary) -> Self {
        let engine::IdentitySummary {
            identity,
            index,
            names,
            main_name,
            names_updating,
            names_check,
            is_main,
            balance,
            balance_as_of,
            balance_stale,
            has_dashpay_keys,
            profile,
            unverified,
            credit_level,
        } = s;
        IdentitySummary {
            identity,
            index,
            names,
            main_name,
            names_updating,
            names_check: names_check.map(Into::into),
            is_main,
            balance,
            balance_as_of,
            balance_stale,
            has_dashpay_keys,
            profile: profile.map(Into::into),
            unverified,
            credit_level: credit_level.map(Into::into),
        }
    }
}

/// Why an identity with no name may still own one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NamesCheckName {
    /// Platform has not answered yet: "updating".
    Pending,
    /// The question failed, or the identity owns a name not loaded yet:
    /// unknown. The engine asks again every minute.
    Failed,
}

mirror_enum!(engine::NamesCheck => NamesCheckName { Pending, Failed });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SecurityLevelName {
    Master,
    Critical,
    High,
    Medium,
}

mirror_enum!(engine::SecurityLevel => SecurityLevelName { Master, Critical, High, Medium });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum KeyTypeName {
    EcdsaSecp256k1,
    Bls12381,
    EcdsaHash160,
    Bip13ScriptHash,
    EddsaHash160,
}

mirror_enum!(engine::KeyType => KeyTypeName {
    EcdsaSecp256k1, Bls12381, EcdsaHash160, Bip13ScriptHash, EddsaHash160
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityKeyInfo {
    pub id: u32,
    pub purpose: KeyPurposeName,
    pub security_level: SecurityLevelName,
    pub key_type: KeyTypeName,
    /// Hex.
    pub public_key: String,
    pub read_only: bool,
    pub disabled_at: Option<u64>,
    /// Base58 id of the contract the key is bound to.
    pub contract_bound: Option<String>,
    /// The document type within `contractBound` (`contactRequest`).
    pub contract_bound_document_type: Option<String>,
}

impl From<engine::IdentityKeyInfo> for IdentityKeyInfo {
    fn from(k: engine::IdentityKeyInfo) -> Self {
        let engine::IdentityKeyInfo {
            id,
            purpose,
            security_level,
            key_type,
            public_key,
            read_only,
            disabled_at,
            contract_bound,
            contract_bound_document_type,
        } = k;
        IdentityKeyInfo {
            id,
            purpose: purpose.into(),
            security_level: security_level.into(),
            key_type: key_type.into(),
            public_key,
            read_only,
            disabled_at,
            contract_bound,
            contract_bound_document_type,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDetail {
    pub summary: IdentitySummary,
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
}

impl From<engine::IdentityDetail> for IdentityDetail {
    fn from(d: engine::IdentityDetail) -> Self {
        let engine::IdentityDetail {
            summary,
            revision,
            public_keys,
        } = d;
        IdentityDetail {
            summary: summary.into(),
            revision,
            public_keys: public_keys.into_iter().map(Into::into).collect(),
        }
    }
}

/// One identity's outcome of `identityRefreshAll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityRefresh {
    /// Base58.
    pub identity: String,
    /// The error code of a failed refresh; `null` when it succeeded.
    pub failure_code: Option<String>,
}

impl From<engine::IdentityRefresh> for IdentityRefresh {
    fn from(r: engine::IdentityRefresh) -> Self {
        let engine::IdentityRefresh {
            identity,
            failure_code,
        } = r;
        IdentityRefresh {
            identity,
            failure_code,
        }
    }
}

/// How `identityFind` reached the identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FoundBy {
    Id,
    /// The DPNS name, as spelled, whose `records.identity` it is.
    Name { label: String },
}

impl From<engine::FoundBy> for FoundBy {
    fn from(f: engine::FoundBy) -> Self {
        match f {
            engine::FoundBy::Id => FoundBy::Id,
            engine::FoundBy::Name { label } => FoundBy::Name { label },
        }
    }
}

/// Any identity Platform knows, read-only; never added to the wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FoundIdentity {
    /// Base58.
    pub identity: String,
    pub found_by: FoundBy,
    /// Its DPNS names as spelled, at most 10; empty when that read failed.
    pub names: Vec<String>,
    /// Credits; `null` while unknown.
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub balance: Option<u64>,
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
    /// One of this wallet's identities.
    pub own: bool,
    /// Verified only through the trusted fallback.
    pub unverified: bool,
}

impl From<engine::FoundIdentity> for FoundIdentity {
    fn from(f: engine::FoundIdentity) -> Self {
        let engine::FoundIdentity {
            identity,
            found_by,
            names,
            balance,
            revision,
            public_keys,
            own,
            unverified,
        } = f;
        FoundIdentity {
            identity,
            found_by: found_by.into(),
            names,
            balance,
            revision,
            public_keys: public_keys.into_iter().map(Into::into).collect(),
            own,
            unverified,
        }
    }
}
