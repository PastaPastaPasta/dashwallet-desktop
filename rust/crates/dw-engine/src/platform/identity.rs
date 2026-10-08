//! The wallet's identities and their keys (DP1-01, DP1-05, DP6-01).

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::PlatformError;
use super::profile::Profile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySummary {
    pub identity: String,
    /// `i` in `m/9'/c'/5'/0'/0'/i'`.
    pub index: u32,
    pub names: Vec<String>,
    pub main_name: Option<String>,
    pub is_main: bool,
    /// Credits.
    pub balance: Option<u64>,
    pub has_dashpay_keys: bool,
    pub profile: Option<Profile>,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityDetail {
    pub summary: IdentitySummary,
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityKeyInfo {
    pub id: u32,
    pub purpose: KeyPurpose,
    pub security_level: SecurityLevel,
    pub key_type: KeyType,
    /// Hex.
    pub public_key: String,
    pub read_only: bool,
    pub disabled_at: Option<u64>,
    /// Base58 id of the contract the key is bound to.
    pub contract_bound: Option<String>,
    /// The document type within `contract_bound` (`contactRequest`).
    pub contract_bound_document_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPurpose {
    Authentication,
    Encryption,
    Decryption,
    Transfer,
    System,
    Voting,
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityLevel {
    Master,
    Critical,
    High,
    Medium,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    EcdsaSecp256k1,
    Bls12381,
    EcdsaHash160,
    Bip13ScriptHash,
    EddsaHash160,
}

#[expect(unused_variables, reason = "stubs until the DP tasks")]
impl DashPay {
    pub fn identities(&self) -> Result<Vec<IdentitySummary>, PlatformError> {
        stub("DashPay.identities")
    }

    pub async fn set_main_identity(&self, identity: String) -> Result<(), PlatformError> {
        stub("DashPay.set_main_identity")
    }

    pub async fn identity_detail(&self, identity: String) -> Result<IdentityDetail, PlatformError> {
        stub("DashPay.identity_detail")
    }

    pub async fn refresh_balance(&self, identity: String) -> Result<Option<u64>, PlatformError> {
        stub("DashPay.refresh_balance")
    }

    /// `grant` is an `IdentityScan` grant id only; a lease id is
    /// `platform.grant_invalid`.
    pub async fn discover_identities(&self, grant: String) -> Result<u32, PlatformError> {
        stub("DashPay.discover_identities")
    }
}
