//! The wallet's identities and their keys (DP1-01, DP1-05, DP6-01).

use std::sync::Arc;

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
    /// `names` and `main_name` are not known right now (a sync pass holds
    /// the wallet's identity state): both are empty until a later read, and
    /// the UI shows "updating" rather than "no name" (DEC-138).
    pub names_updating: bool,
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

impl DashPay {
    /// The wallet's identities by index, from memory (DP1-05). The main
    /// identity is the chosen one while the wallet still has it, else the
    /// lowest index; an identity's main name is the chosen one while it still
    /// owns it, else the name it acquired first (`recovery.rs`).
    pub fn identities(&self) -> Result<Vec<IdentitySummary>, PlatformError> {
        Ok(self.session.identity_summaries(self.wallet_id())?)
    }

    /// Chooses the main identity: one of the wallet's, else
    /// `identity.not_found`. Stored in `dp_main_identity`, so a `.dwbackup`
    /// carries it.
    pub async fn set_main_identity(&self, identity: String) -> Result<(), PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.set_main_identity_of(id, identity).await) })
            .await?
    }

    #[expect(unused_variables, reason = "stub until DP6-01")]
    pub async fn identity_detail(&self, identity: String) -> Result<IdentityDetail, PlatformError> {
        stub("DashPay.identity_detail")
    }

    #[expect(unused_variables, reason = "stub until DP6-01")]
    pub async fn refresh_balance(&self, identity: String) -> Result<Option<u64>, PlatformError> {
        stub("DashPay.refresh_balance")
    }

    /// Same-seed discovery past the highest identity index on file (DP1-05,
    /// DP6-01's "find"); the number of identities found, whose DashPay state
    /// and names follow in the background. `grant` is an `IdentityScan` grant
    /// id only; a lease id is `platform.grant_invalid`. Also clears
    /// Platform's earlier proof that the seed owns none, so the next start's
    /// bring-up looks again.
    pub async fn discover_identities(&self, grant: String) -> Result<u32, PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.discover_identities_of(id, grant).await) })
            .await?
    }
}
