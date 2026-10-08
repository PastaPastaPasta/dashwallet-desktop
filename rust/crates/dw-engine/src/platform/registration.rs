//! Registration: quote, the persisted state machine (§3.4), resume, discard
//! and stranded asset locks (DP1-02).

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::RegistrationError;
use super::flows::GrantRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub label: String,
    pub temporary_label: Option<String>,
    pub funding: RegistrationFunding,
    pub initial_profile: Option<InitialProfile>,
}

/// The profile entered at Draft. Kept in `dp_registration.initial_profile`
/// until `ProfileCreated`, across restarts and restores.
/// `avatar_candidate` must name a candidate whose `url` is set (a `Url` or
/// `Gravatar` source, or a file after `upload_avatar`); the engine stores its
/// URL, hash and fingerprint, never the candidate id, and never uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_candidate: Option<String>,
}

/// Persisted as `dp_registration.funding` in DP1-02's versioned encoding
/// (§3.4), never in this type's serde form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistrationFunding {
    CoreBalance,
    /// A `stash_invitation` id.
    Invitation {
        link_id: String,
    },
    /// The id goes to the row's `identity` column.
    ExistingIdentity {
        identity: String,
    },
    /// Developer builds only. `key` is a `prepare_faucet_lock` key id and
    /// `proof` the hex `assetLockProof` the faucet returned for its public
    /// key. The engine re-derives the private key; nothing secret crosses
    /// the facade.
    FaucetAssetLock {
        key: String,
        proof: String,
    },
}

/// An asset-lock key the engine derived for the faucet's
/// `POST /api/asset-lock-proof`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaucetLockKey {
    pub key: String,
    /// The compressed secp256k1 public key, 66 hex characters.
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationQuote {
    pub contested: bool,
    /// Duffs locked into the asset lock.
    pub lock_duffs: u64,
    pub fee_duffs: u64,
    /// `lock_duffs + fee_duffs`: what the grant must cover.
    pub total_duffs: u64,
    /// Credits the identity keeps after the name and profile.
    pub remaining_credits: u64,
    /// The grant to ask for: `max_duffs` covers `total_duffs`, and
    /// `max_credits` the identity creation, name and profile.
    pub grant: GrantRequest,
}

/// One `dp_registration` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationStatus {
    pub draft: String,
    pub phase: RegistrationPhase,
    pub label: String,
    pub temporary_label: Option<String>,
    pub identity: Option<String>,
    pub txid: Option<String>,
    /// What a parked or slow flow waits for; `None` while it runs or after
    /// it ends. `resume_registration` needs a grant exactly when this is
    /// `Unlock` or `Authorize`.
    pub waiting: Option<RegistrationWait>,
    /// A lease holds a key for this flow ("Registration in progress — Lock
    /// to cancel").
    pub holds_key: bool,
    /// True once the asset lock is anything but definitely not sent
    /// ("Funds locked — finishing").
    pub funds_committed: bool,
    /// While `phase` is `Contested`.
    pub contest_ends_at: Option<u64>,
    /// Set when `phase` is `Failed`.
    pub failure: Option<RegistrationFailure>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// The §3.4 state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationPhase {
    Draft,
    KeysPrepared,
    FundingSent,
    ProofWaiting,
    IdentityRegistered,
    NameRequested,
    NameRegistered,
    Contested,
    ProfileCreated,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationWait {
    /// Parked keyless: "Unlock to finish". The resume needs a grant.
    Unlock,
    /// The vault is unlocked but the flow's grant died (an unlock, a scope
    /// or a passphrase change) or its budget ran short: "Confirm to
    /// finish". The resume needs a grant. Pending E0-04 rev1.
    Authorize,
    /// Held until SPV has synced: "Waiting for the network to sync".
    Sync,
    InstantSend,
    ChainLock,
    /// Platform or the peers are unreachable; retried on reconnect.
    Network,
}

/// Why a flow is `Failed`, with the parameters its code carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationFailure {
    /// The phase the flow stopped in.
    pub phase: RegistrationPhase,
    pub code: String,
    pub retryable: bool,
    /// `needed` and `available` of a `*_insufficient*` code.
    pub needed: Option<u64>,
    pub available: Option<u64>,
}

/// Tools ▸ Repair "Finish transfers".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReport {
    pub resumed: u32,
    pub completed: u32,
    pub still_pending: u32,
}

#[expect(unused_variables, reason = "stubs until DP1-02")]
impl DashPay {
    pub async fn registration_quote(
        &self,
        req: RegistrationRequest,
    ) -> Result<RegistrationQuote, RegistrationError> {
        stub("DashPay.registration_quote")
    }

    pub async fn start_registration(
        &self,
        req: RegistrationRequest,
        grant: String,
    ) -> Result<String, RegistrationError> {
        stub("DashPay.start_registration")
    }

    pub async fn registrations(&self) -> Result<Vec<RegistrationStatus>, RegistrationError> {
        stub("DashPay.registrations")
    }

    pub async fn resume_registration(
        &self,
        draft: String,
        grant: Option<String>,
    ) -> Result<(), RegistrationError> {
        stub("DashPay.resume_registration")
    }

    pub async fn discard_registration(&self, draft: String) -> Result<(), RegistrationError> {
        stub("DashPay.discard_registration")
    }

    pub async fn finish_asset_locks(
        &self,
        grant: String,
    ) -> Result<FinishReport, RegistrationError> {
        stub("DashPay.finish_asset_locks")
    }

    pub async fn prepare_faucet_lock(
        &self,
        grant: String,
    ) -> Result<FaucetLockKey, RegistrationError> {
        stub("DashPay.prepare_faucet_lock")
    }
}
