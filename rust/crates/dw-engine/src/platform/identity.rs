//! The wallet's identities and their keys (DP1-01, DP1-05, DP6-01).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::credits::CreditLevel;
use super::dashpay::DashPay;
use super::errors::PlatformError;
use super::profile::Profile;
use super::registration_flow::parse_identity;
use super::topup;

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
    /// Set while `main_name` is `None` and Platform has not said this
    /// session that the identity owns no name: it may still own one, so the
    /// UI says "updating" or "unknown", never "no name" (DP1-08 D23). `None`
    /// for an identity with a name, and for one Platform said owns none.
    pub names_check: Option<NamesCheck>,
    pub is_main: bool,
    /// Credits.
    pub balance: Option<u64>,
    /// The Platform block time (ms) of `balance`, once a refresh read it
    /// (DP1-06).
    pub balance_as_of: Option<u64>,
    /// `balance` may be behind: never refreshed, older than ten minutes, or
    /// spent by a write since its refresh. The engine refreshes it in the
    /// background (`balances.rs`); it stays set while a refresh fails.
    pub balance_stale: bool,
    /// `balance` against the cost table (DP1-06); `None` while the balance
    /// is unknown.
    pub credit_level: Option<CreditLevel>,
    pub has_dashpay_keys: bool,
    pub profile: Option<Profile>,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

/// Why an identity that shows no name may still own one (DP1-08 D23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamesCheck {
    /// Platform has not answered yet, or the names pass the wallet owes has
    /// not ended.
    Pending,
    /// The question failed, or Platform said it owns a name not loaded yet:
    /// the engine asks again every minute.
    Failed,
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

/// One identity's outcome in `refresh_identities` (DP6-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRefresh {
    pub identity: String,
    /// The error code of a failed refresh.
    pub failure_code: Option<String>,
}

/// Any identity Platform knows, as `find_identity` read it (DP6-01). Never
/// added to the wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundIdentity {
    pub identity: String,
    pub found_by: FoundBy,
    /// The labels, as spelled, of the DPNS names whose `records.identity`
    /// is this identity, at most 10; empty when that read failed.
    pub names: Vec<String>,
    /// Credits.
    pub balance: Option<u64>,
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
    /// One of this wallet's identities.
    pub own: bool,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

/// How `find_identity` reached the identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FoundBy {
    Id,
    /// The DPNS name, as spelled, whose `records.identity` it is.
    Name {
        label: String,
    },
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
    /// carries it. Signalled `Platform{Identities}` and `Platform{Startup}`
    /// (DP6-01 D15).
    pub async fn set_main_identity(&self, identity: String) -> Result<(), PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.set_main_identity_of(id, identity).await) })
            .await?
    }

    /// One of the wallet's identities with its revision and public keys,
    /// from the library's memory (DP6-01): its summary as `identities()`
    /// has it, else `identity.not_found`.
    pub async fn identity_detail(&self, identity: String) -> Result<IdentityDetail, PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.identity_detail_of(id, identity).await) })
            .await?
    }

    /// Folds Platform's keys and revision of one of the wallet's identities
    /// into the library's, then refreshes its balance as `refresh_balance`
    /// does (DP6-01, `identity_reads.rs`); the detail after. Not one of the
    /// wallet's, or not on Platform: `identity.not_found`.
    pub async fn refresh_identity(
        &self,
        identity: String,
    ) -> Result<IdentityDetail, PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.refresh_identity_of(id, identity).await) })
            .await?
    }

    /// `refresh_identity` for each of the wallet's identities in index
    /// order, one at a time; each outcome with a failed one's code.
    pub async fn refresh_identities(&self) -> Result<Vec<IdentityRefresh>, PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.refresh_identities_of(id).await) })
            .await?
    }

    /// Any identity by its Base58 id or a DPNS name (`alice` or
    /// `alice.dash`, any case), read-only (DP6-01): `None` if Platform has
    /// no such identity or name; neither form: `invalid_argument`.
    /// Nothing is added to the wallet.
    pub async fn find_identity(
        &self,
        query: String,
    ) -> Result<Option<FoundIdentity>, PlatformError> {
        let (session, id) = (Arc::clone(&self.session), self.wallet_id());
        self.session
            .on_runtime(async move { Ok(session.find_identity_of(id, query).await) })
            .await?
    }

    /// Platform's balance of one of the wallet's identities (DP1-06):
    /// fetched with its proof, stored with its block time so an older
    /// answer never replaces a newer one, and signalled
    /// `Platform{Identities}`. `None` when Platform has no balance for it.
    /// Needs no key, so it runs with the vault locked.
    pub async fn refresh_balance(&self, identity: String) -> Result<Option<u64>, PlatformError> {
        let wallet = self.wallet_id;
        topup::on_lib(&self.session, wallet, move |session, _| async move {
            (session.refresh_balance_of(wallet, parse_identity(&identity)?)).await
        })
        .await
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
