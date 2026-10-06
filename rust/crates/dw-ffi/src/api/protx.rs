//! M3 provider transactions: Register Masternode/EvoNode (QT-123) with the
//! operator-secret gate (QT-124), Update Service / Update Registrar /
//! Revoke (QT-125, IOS-081 unban), and v24 shared masternodes (QT-126,
//! QT-127). Owner: R3 (`dw-protx`). Contract: docs/contracts/m3-engine.md
//! §2.4.
//!
//! Every flow is prepare → review → broadcast: `prepare_*` builds and signs
//! (inputs reserved, nothing sent) and returns an object the host reviews,
//! then `broadcast`/`submit` sends it or `abandon` releases its inputs —
//! the shape of `TxDraft`/`PreparedTx` (M1).

use std::sync::Arc;

use zeroize::Zeroize;

use crate::api::common::{ensure_open, not_implemented, parse_wallet_id};
use crate::api::masternode::parse_pro_tx_hash;
use crate::{MasternodeError, MasternodeType, NetworkSession, OutPoint};

/// Where a registration's collateral comes from (QT-123 "Collateral").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CollateralChoice {
    /// Default: the registration pays the collateral to a fresh wallet
    /// address (`protx register_fund`).
    FundNew,
    /// An exact-amount, confirmed, P2PKH, unlocked wallet UTXO.
    ExistingUtxo { outpoint: OutPoint },
    /// An outpoint the wallet does not hold (hardware wallet): the
    /// registration is prepared, then the collateral key's owner signs
    /// `collateral_sign_message` (`register_prepare` / `register_submit`).
    External { outpoint: OutPoint },
}

/// The operator BLS key (basic scheme only).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OperatorKeyChoice {
    /// A fresh key; its secret is shown once (QT-124) and never stored.
    Generate,
    /// An existing public key, 96 hex characters.
    Existing { public_key_hex: String },
}

/// EvoNode Platform fields. v24 networks take address lists; before v24
/// only the ports of the first entries count.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PlatformFields {
    /// 40 hex characters (Tenderdash node id).
    pub node_id_hex: String,
    pub p2p_addresses: Vec<String>,
    pub https_addresses: Vec<String>,
}

/// Who pays the provider-transaction fee.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FeeSourceChoice {
    /// The engine picks spendable coins (Update Service "Automatic").
    Automatic,
    /// Coins of one wallet address (Register wizard "Fee source"; it must
    /// also hold the collateral with `FundNew`).
    Address { address: String },
}

/// The Register wizard's answers (QT-123).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RegistrationRequest {
    pub wallet_id: String,
    pub node_type: MasternodeType,
    pub collateral: CollateralChoice,
    /// `IP:port` entries; empty = register without service (inactive until
    /// an Update Service).
    pub service_addresses: Vec<String>,
    /// `None` = a fresh wallet address.
    pub owner_address: Option<String>,
    /// `None` = the owner address.
    pub voting_address: Option<String>,
    pub operator_key: OperatorKeyChoice,
    pub payout_address: String,
    /// 0–10000 (hundredths of a percent).
    pub operator_reward_x100: u16,
    /// Required for EvoNodes, refused for regular masternodes.
    pub platform: Option<PlatformFields>,
    pub fee_source: FeeSourceChoice,
}

/// One wallet UTXO for "Existing wallet UTXO", with why it is unusable.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CollateralCandidate {
    pub outpoint: OutPoint,
    pub address: String,
    pub amount: u64,
    pub confirmations: u32,
    pub refusal: Option<crate::CollateralRefusal>,
}

/// One address for the wizard's "Fee source" list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FeeSourceCandidate {
    pub address: String,
    pub spendable: u64,
    pub label: Option<String>,
}

/// The review page (QT-123 "Review").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RegistrationSummary {
    pub node_type: MasternodeType,
    /// Known after prepare for every collateral choice.
    pub pro_tx_hash: String,
    pub collateral: OutPoint,
    pub collateral_address: String,
    pub owner_address: String,
    pub voting_address: String,
    pub payout_address: String,
    pub operator_public_key: String,
    pub operator_reward_x100: u16,
    pub service_addresses: Vec<String>,
    pub platform: Option<PlatformFields>,
    pub fee: u64,
    /// Collateral (with `FundNew`) + fee.
    pub total_spent: u64,
    /// The secret gate applies (the operator key was generated).
    pub operator_secret_required: bool,
    /// `Some` for external collateral: the message to sign with the
    /// collateral key (`payout|operatorReward|ownerAddr|votingAddr|payloadHash`).
    pub collateral_sign_message: Option<String>,
}

/// The generated operator secret, shown once (QT-124). Bytes so the host
/// keeps them in a zeroing buffer; both are ASCII.
#[derive(uniffi::Record)]
pub struct OperatorSecret {
    /// 64 hex characters.
    pub secret_hex: Vec<u8>,
    /// `masternodeblsprivkey=<hex>`.
    pub config_line: Vec<u8>,
}

/// A prepared registration (signed unless external; inputs reserved).
#[derive(uniffi::Object)]
pub struct PreparedRegistration {
    pub(crate) session: Arc<dw_engine::NetworkSession>,
}

#[uniffi::export]
impl PreparedRegistration {
    pub fn summary(&self) -> Result<RegistrationSummary, MasternodeError> {
        ensure_open(&self.session)?;
        not_implemented("PreparedRegistration.summary")
    }

    /// The generated secret. Available until `submit` succeeds or the
    /// object is abandoned; `invalid_argument` when the key was not
    /// generated. The host holds the bytes in a zeroing buffer.
    pub fn operator_secret(&self) -> Result<OperatorSecret, MasternodeError> {
        ensure_open(&self.session)?;
        not_implemented("PreparedRegistration.operator_secret")
    }

    /// The "type the last 4 characters" gate: `true` and the gate opens
    /// when `last4` equals the secret's last four hex characters
    /// (case-insensitive). `submit` refuses until then.
    pub fn confirm_operator_secret(&self, last4: String) -> Result<bool, MasternodeError> {
        let _ = last4;
        ensure_open(&self.session)?;
        not_implemented("PreparedRegistration.confirm_operator_secret")
    }

    /// Broadcasts. `collateral_signature`: the base64 `signmessage`
    /// signature for external collateral, else `None`. Returns the
    /// proTxHash. Refused with `masternode.operator_secret_unconfirmed`
    /// before the gate opened (dash-qt's order: secret page before
    /// broadcast).
    pub async fn submit(
        &self,
        collateral_signature: Option<String>,
    ) -> Result<String, MasternodeError> {
        let _ = collateral_signature;
        ensure_open(&self.session)?;
        not_implemented("PreparedRegistration.submit")
    }

    /// Releases the reserved inputs and forgets the secret.
    pub async fn abandon(&self) -> Result<(), MasternodeError> {
        ensure_open(&self.session)?;
        not_implemented("PreparedRegistration.abandon")
    }
}

/// ProUpRevTx reason (QT-125 "Revoke").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum RevocationReason {
    NotSpecified,
    TerminationOfService,
    CompromisedKeys,
    ChangeOfKeys,
}

/// Update Service (ProUpServTx; also revives a PoSe-banned node, IOS-081
/// "Unban").
#[derive(uniffi::Record)]
pub struct UpdateServiceRequest {
    pub pro_tx_hash: String,
    pub service_addresses: Vec<String>,
    /// The operator secret typed in (dash-qt asks every time), 64 hex
    /// ASCII bytes. `None` = use the key a wallet derives or a tracked
    /// masternode has attached (`masternode.key_not_in_wallet` otherwise).
    pub operator_secret: Option<Vec<u8>>,
    pub platform: Option<PlatformFields>,
    /// Only when the operator reward is above 0.
    pub operator_payout_address: Option<String>,
    pub fee_source: FeeSourceChoice,
    /// The wallet paying the fee.
    pub fee_wallet_id: String,
}

/// Update Registrar (ProUpRegTx): only changed fields are sent; needs the
/// owner key.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UpdateRegistrarRequest {
    pub pro_tx_hash: String,
    /// 96 hex; legacy scheme when the ProTx version is LegacyBLS.
    pub operator_public_key: Option<String>,
    pub voting_address: Option<String>,
    pub payout_address: Option<String>,
    pub fee_source: FeeSourceChoice,
    pub fee_wallet_id: String,
}

/// Revoke (ProUpRevTx); needs the operator secret.
#[derive(uniffi::Record)]
pub struct RevokeRequest {
    pub pro_tx_hash: String,
    pub operator_secret: Option<Vec<u8>>,
    pub reason: RevocationReason,
    pub fee_source: FeeSourceChoice,
    pub fee_wallet_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ProviderTxKind {
    UpdateService,
    UpdateRegistrar,
    Revoke,
    /// v24 ProUpShareTx (change a share's reward address).
    UpdateShare,
    /// v24 ProUpSharedRegTx (rotate keys of a shared masternode).
    UpdateSharedRegistrar,
    /// v24 ProDissolveTx.
    Dissolve,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProviderTxSummary {
    pub kind: ProviderTxKind,
    pub pro_tx_hash: String,
    pub txid: String,
    pub fee: u64,
    /// Penalty paid by a dissolve during the early period.
    pub penalty: Option<u64>,
    /// "Changing the operator key immediately PoSe-bans the masternode…"
    pub bans_masternode: bool,
}

/// A signed, not yet broadcast maintenance transaction.
#[derive(uniffi::Object)]
pub struct PreparedProviderTx {
    pub(crate) session: Arc<dw_engine::NetworkSession>,
}

#[uniffi::export]
impl PreparedProviderTx {
    pub fn summary(&self) -> Result<ProviderTxSummary, MasternodeError> {
        ensure_open(&self.session)?;
        not_implemented("PreparedProviderTx.summary")
    }

    /// Returns the txid.
    pub async fn broadcast(&self) -> Result<String, MasternodeError> {
        ensure_open(&self.session)?;
        not_implemented("PreparedProviderTx.broadcast")
    }

    pub async fn abandon(&self) -> Result<(), MasternodeError> {
        ensure_open(&self.session)?;
        not_implemented("PreparedProviderTx.abandon")
    }
}

/// One share the coordinator proposes (QT-126).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedShareTerms {
    /// ≥ 100 DASH; all shares sum to 1000 DASH.
    pub amount: u64,
    pub label: Option<String>,
    /// The coordinator's own share.
    pub mine: bool,
}

/// The terms a coordinator starts a shared masternode with.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedMasternodeTerms {
    /// 2–8 shares.
    pub shares: Vec<SharedShareTerms>,
    /// ≤ 420480 blocks.
    pub early_period_blocks: u32,
    /// Below the smallest share.
    pub early_exit_penalty: u64,
    pub service_addresses: Vec<String>,
    pub operator_key: OperatorKeyChoice,
    pub operator_reward_x100: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum SharedRole {
    Coordinator,
    Participant,
}

/// Stage of a shared session (dash-qt's three rounds, then broadcast).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum SharedStage {
    Invitation,
    Details,
    LockedTerms,
    Approvals,
    SigningRequest,
    SignedContributions,
    Broadcast,
    Completed,
    Abandoned,
}

/// What a session is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum SharedSessionPurpose {
    Register,
    RotateKeys,
    DissolveTogether,
}

/// A shared session the wallet takes part in (persisted in app.sqlite).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedSessionInfo {
    pub session_id: String,
    /// First 6 hex characters of the session id.
    pub session_code: String,
    pub purpose: SharedSessionPurpose,
    pub role: SharedRole,
    pub stage: SharedStage,
    pub revision: u32,
    /// `XXXX-XXXX` of the latest message.
    pub fingerprint: String,
    pub wallet_id: String,
    pub my_share_indexes: Vec<u32>,
    /// Coins reserved for the session (persistent locks).
    pub reserved_inputs: Vec<OutPoint>,
    /// A reserved coin was spent elsewhere.
    pub reserved_coin_spent: bool,
    /// The proTxHash once registered, or the masternode a rotate/dissolve
    /// session concerns.
    pub pro_tx_hash: Option<String>,
}

/// An outgoing message for the other participants (clipboard or `.json`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedEnvelope {
    pub json: String,
    pub fingerprint: String,
    pub suggested_file_name: String,
}

/// What a paste or a dropped file is (QT-127 paste routing).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SharedMessageKind {
    /// A session message; imported into `session`.
    Envelope { session: SharedSessionInfo },
    /// A standby dissolution file's transactions.
    StandbyDissolution {
        pro_tx_hash: Option<String>,
        transactions_hex: Vec<String>,
    },
}

/// A participant's contribution to one share (QT-126 "reserve coins").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ShareContribution {
    pub share_index: u32,
    pub inputs: Vec<OutPoint>,
    /// `None` = fresh wallet addresses.
    pub owner_address: Option<String>,
    pub payout_address: Option<String>,
    pub refund_address: Option<String>,
}

/// Standby dissolution (QT-127): raw transactions saved to `.txt` for a
/// later `sendrawtransaction`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StandbyDissolution {
    pub pro_tx_hash: String,
    pub transactions_hex: Vec<String>,
    pub suggested_file_name: String,
}

/// Overwrites a typed operator secret the call no longer needs. Stubs wipe
/// it before returning; implementations move it into a `Zeroizing` buffer.
fn wipe(secret: &mut Option<Vec<u8>>) {
    if let Some(bytes) = secret.as_mut() {
        bytes.zeroize();
    }
}

#[uniffi::export]
impl NetworkSession {
    /// "Existing wallet UTXO" choices for `node_type`, usable ones first.
    pub async fn collateral_candidates(
        &self,
        wallet_id: String,
        node_type: MasternodeType,
    ) -> Result<Vec<CollateralCandidate>, MasternodeError> {
        let _ = node_type;
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.collateral_candidates")
    }

    /// Wallet addresses with spendable coins ("Fee source" list).
    pub async fn fee_source_candidates(
        &self,
        wallet_id: String,
    ) -> Result<Vec<FeeSourceCandidate>, MasternodeError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.fee_source_candidates")
    }

    /// Builds the registration (QT-123). `grant_id`: a `MasternodeOp`
    /// grant (a `FundNew` registration also spends the collateral, which
    /// the grant must cover as a spend). Rules and their codes:
    /// m3-engine.md §2.4.
    pub async fn prepare_registration(
        &self,
        request: RegistrationRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedRegistration>, MasternodeError> {
        let _ = grant_id;
        parse_wallet_id(&request.wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.prepare_registration")
    }

    /// Update Service (QT-125, IOS-081 unban). `MasternodeOp` grant.
    pub async fn prepare_update_service(
        &self,
        mut request: UpdateServiceRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, MasternodeError> {
        let _ = grant_id;
        wipe(&mut request.operator_secret);
        parse_pro_tx_hash(&request.pro_tx_hash)?;
        parse_wallet_id(&request.fee_wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.prepare_update_service")
    }

    /// Update Registrar (QT-125). `MasternodeOp` grant. Refused for shared
    /// masternodes (they rotate keys through a shared session).
    pub async fn prepare_update_registrar(
        &self,
        request: UpdateRegistrarRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, MasternodeError> {
        let _ = grant_id;
        parse_pro_tx_hash(&request.pro_tx_hash)?;
        parse_wallet_id(&request.fee_wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.prepare_update_registrar")
    }

    /// Revoke (QT-125). `MasternodeOp` grant.
    pub async fn prepare_revoke(
        &self,
        mut request: RevokeRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, MasternodeError> {
        let _ = grant_id;
        wipe(&mut request.operator_secret);
        parse_pro_tx_hash(&request.pro_tx_hash)?;
        parse_wallet_id(&request.fee_wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.prepare_revoke")
    }

    /// Starts a shared-masternode registration as coordinator (QT-126).
    pub async fn create_shared_session(
        &self,
        wallet_id: String,
        terms: SharedMasternodeTerms,
    ) -> Result<SharedSessionInfo, MasternodeError> {
        let _ = terms;
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.create_shared_session")
    }

    /// Reads pasted text or a dropped file's contents (≤ 2 MiB, this
    /// network only) and routes it: a session message is checked and
    /// imported into its session (created for an invitation), a standby
    /// dissolution is returned for broadcast.
    pub async fn import_shared_message(
        &self,
        wallet_id: String,
        text: String,
    ) -> Result<SharedMessageKind, MasternodeError> {
        parse_wallet_id(&wallet_id)?;
        if text.len() > dw_protx::params::MAX_ENVELOPE_BYTES {
            return Err(MasternodeError::SharedEnvelopeTooLarge {
                size_bytes: text.len() as u64,
            });
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.import_shared_message")
    }

    /// Open sessions of the wallet (resumable after restart).
    pub async fn shared_sessions(
        &self,
        wallet_id: String,
    ) -> Result<Vec<SharedSessionInfo>, MasternodeError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_sessions")
    }

    /// The message to send the others for the session's current stage.
    pub async fn shared_session_message(
        &self,
        session_id: String,
    ) -> Result<SharedEnvelope, MasternodeError> {
        let _ = session_id;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_session_message")
    }

    /// Participant: reserves coins and fills in a share's addresses.
    pub async fn shared_session_contribute(
        &self,
        session_id: String,
        contribution: ShareContribution,
    ) -> Result<SharedSessionInfo, MasternodeError> {
        let _ = (session_id, contribution);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_session_contribute")
    }

    /// Approves the locked terms (consent signature with the share owner
    /// key). `MasternodeOp` grant.
    pub async fn shared_session_approve(
        &self,
        session_id: String,
        grant_id: String,
    ) -> Result<SharedSessionInfo, MasternodeError> {
        let _ = (session_id, grant_id);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_session_approve")
    }

    /// Signs the wallet's inputs of the session transaction after checking
    /// that no foreign or short-changed input is asked
    /// (`masternode.shared_inputs_refused`). `MasternodeOp` grant.
    pub async fn shared_session_sign(
        &self,
        session_id: String,
        grant_id: String,
    ) -> Result<SharedSessionInfo, MasternodeError> {
        let _ = (session_id, grant_id);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_session_sign")
    }

    /// Coordinator: combines the signed contributions and broadcasts.
    /// Returns the txid (the proTxHash for a registration).
    pub async fn shared_session_broadcast(
        &self,
        session_id: String,
    ) -> Result<String, MasternodeError> {
        let _ = session_id;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_session_broadcast")
    }

    /// Leaves the session and releases its reserved coins (close
    /// protection's "release").
    pub async fn shared_session_abandon(&self, session_id: String) -> Result<(), MasternodeError> {
        let _ = session_id;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.shared_session_abandon")
    }

    /// Change Reward Address of the wallet's share (ProUpShareTx).
    /// `MasternodeOp` grant.
    pub async fn prepare_share_reward_update(
        &self,
        pro_tx_hash: String,
        share_index: u32,
        payout_address: String,
        fee_wallet_id: String,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, MasternodeError> {
        let _ = (share_index, payout_address, grant_id);
        parse_pro_tx_hash(&pro_tx_hash)?;
        parse_wallet_id(&fee_wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.prepare_share_reward_update")
    }

    /// Rotate Operator/Voting Key: starts a session every owner approves,
    /// then the preparer sends (ProUpSharedRegTx).
    pub async fn start_shared_key_rotation(
        &self,
        wallet_id: String,
        pro_tx_hash: String,
        operator_key: Option<OperatorKeyChoice>,
        voting_address: Option<String>,
    ) -> Result<SharedSessionInfo, MasternodeError> {
        let _ = (operator_key, voting_address);
        parse_wallet_id(&wallet_id)?;
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.start_shared_key_rotation")
    }

    /// Dissolve Now (unilateral). During the early period the wallet's
    /// principal pays the penalty; `accept_penalty` must be true then.
    /// `MasternodeOp` grant.
    pub async fn prepare_dissolve_now(
        &self,
        pro_tx_hash: String,
        accept_penalty: bool,
        fee_wallet_id: String,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, MasternodeError> {
        let _ = (accept_penalty, grant_id);
        parse_pro_tx_hash(&pro_tx_hash)?;
        parse_wallet_id(&fee_wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.prepare_dissolve_now")
    }

    /// Dissolve Together: a unanimous session that returns every
    /// principal.
    pub async fn start_dissolve_together(
        &self,
        wallet_id: String,
        pro_tx_hash: String,
    ) -> Result<SharedSessionInfo, MasternodeError> {
        parse_wallet_id(&wallet_id)?;
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.start_dissolve_together")
    }

    /// Create Standby Dissolution: the two raw transactions to keep for a
    /// later broadcast; the engine records that one exists. `MasternodeOp`
    /// grant.
    pub async fn create_standby_dissolution(
        &self,
        wallet_id: String,
        pro_tx_hash: String,
        grant_id: String,
    ) -> Result<StandbyDissolution, MasternodeError> {
        let _ = grant_id;
        parse_wallet_id(&wallet_id)?;
        parse_pro_tx_hash(&pro_tx_hash)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.create_standby_dissolution")
    }

    /// Broadcasts saved standby-dissolution transactions in order.
    /// Returns their txids.
    pub async fn broadcast_standby_dissolution(
        &self,
        transactions_hex: Vec<String>,
    ) -> Result<Vec<String>, MasternodeError> {
        let _ = transactions_hex;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.broadcast_standby_dissolution")
    }
}
