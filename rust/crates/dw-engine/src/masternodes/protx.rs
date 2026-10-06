//! Provider transactions (QT-123…125, IOS-081): Register Masternode/EvoNode
//! with the operator-secret gate, Update Service (also the unban),
//! Update Registrar and Revoke, each prepare → review → broadcast.
//!
//! Funding follows Dash Core's `FundSpecialTx` (`src/rpc/evo.cpp`): the fee
//! comes from the chosen fee-source address (change back to it) or from any
//! spendable coin (change to a fresh change address); a transaction without
//! an output of its own gets a zero-value `OP_RETURN` so it always has one.
//! The engine selects only coins a plain send could spend (no user-locked,
//! reserved, immature or untrusted coins, no CoinJoin coins) and hands them
//! to key-wallet's builder, whose payload finalizer commits the payload to
//! the chosen inputs and signs it before the inputs are signed (see
//! `dw_protx::payloads`).
//!
//! A registration's collateral is locked against spending once it was
//! broadcast, as Dash Core's wallet does (`AutoLockMasternodeCollaterals`).

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dashcore::address::Payload;
use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, ScriptBuf, Transaction, Txid};
use dw_protx::payloads::{
    self, CollateralSource, PlatformPorts, RegistrationSigner, RegistrationTerms,
};
use dw_protx::{bls, params, service};
use dw_vault::{GrantKind, VaultError, VaultSigner};
use key_wallet::wallet::managed_wallet_info::coin_selection::SelectionStrategy;
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use key_wallet::wallet::managed_wallet_info::transaction_builder::{
    BuilderError, TransactionBuilder,
};
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::core::{SEND_FUNDING_SOURCES, SignedCoreTransaction};
use platform_wallet::wallet::provider_key_at_index::ProviderKeyKind;
use zeroize::Zeroizing;

use super::list::{Known, MasternodeType, display_hex, parse_pro_tx_hash};
use crate::send::{MAX_TX_FEE, MIN_FEE_PER_KB, l1_address, reason_means_no_peers};
use crate::{
    CollateralRefusal, EngineError, MasternodeFailure, MasternodeKeyRole, NetworkSession, WalletId,
};

/// Where a registration's collateral comes from (QT-123).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollateralChoice {
    FundNew,
    ExistingUtxo(OutPoint),
    External(OutPoint),
}

/// The operator BLS key of a registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorKeyChoice {
    Generate,
    /// 96 hex characters, basic serialization.
    Existing(String),
}

/// EvoNode Platform fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformFields {
    pub node_id_hex: String,
    pub p2p_addresses: Vec<String>,
    pub https_addresses: Vec<String>,
}

/// Who pays the provider transaction's fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeeSourceChoice {
    Automatic,
    Address(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRequest {
    pub wallet_id: WalletId,
    pub node_type: MasternodeType,
    pub collateral: CollateralChoice,
    pub service_addresses: Vec<String>,
    pub owner_address: Option<String>,
    pub voting_address: Option<String>,
    pub operator_key: OperatorKeyChoice,
    pub payout_address: String,
    pub operator_reward_x100: u16,
    pub platform: Option<PlatformFields>,
    pub fee_source: FeeSourceChoice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollateralCandidate {
    pub outpoint: OutPoint,
    pub address: String,
    pub amount: u64,
    pub confirmations: u32,
    pub refusal: Option<CollateralRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSourceCandidate {
    pub address: String,
    pub spendable: u64,
    pub label: Option<String>,
}

/// The review page (QT-123).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationSummary {
    pub node_type: MasternodeType,
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
    pub total_spent: u64,
    pub operator_secret_required: bool,
    pub collateral_sign_message: Option<String>,
}

/// ProUpRevTx reason (`CProUpRevTx::REASON_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RevocationReason {
    NotSpecified,
    TerminationOfService,
    CompromisedKeys,
    ChangeOfKeys,
}

impl RevocationReason {
    fn code(self) -> u16 {
        match self {
            Self::NotSpecified => 0,
            Self::TerminationOfService => 1,
            Self::CompromisedKeys => 2,
            Self::ChangeOfKeys => 3,
        }
    }
}

/// Update Service (ProUpServTx; IOS-081 unban).
pub struct UpdateServiceRequest {
    pub pro_tx_hash: String,
    pub service_addresses: Vec<String>,
    /// Typed operator secret (64 hex ASCII); `None` = a wallet-derived or
    /// attached key.
    pub operator_secret: Option<Zeroizing<Vec<u8>>>,
    pub platform: Option<PlatformFields>,
    pub operator_payout_address: Option<String>,
    pub fee_source: FeeSourceChoice,
    pub fee_wallet_id: WalletId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRegistrarRequest {
    pub pro_tx_hash: String,
    pub operator_public_key: Option<String>,
    pub voting_address: Option<String>,
    pub payout_address: Option<String>,
    pub fee_source: FeeSourceChoice,
    pub fee_wallet_id: WalletId,
}

pub struct RevokeRequest {
    pub pro_tx_hash: String,
    pub operator_secret: Option<Zeroizing<Vec<u8>>>,
    pub reason: RevocationReason,
    pub fee_source: FeeSourceChoice,
    pub fee_wallet_id: WalletId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderTxKind {
    UpdateService,
    UpdateRegistrar,
    Revoke,
    UpdateShare,
    UpdateSharedRegistrar,
    Dissolve,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTxSummary {
    pub kind: ProviderTxKind,
    pub pro_tx_hash: String,
    pub txid: String,
    pub fee: u64,
    pub penalty: Option<u64>,
    pub bans_masternode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Broadcasting,
    Sent,
    /// Handed to the network without a verdict; never released again.
    Unknown,
    Released,
}

/// A signed provider transaction with its inputs reserved.
pub(crate) struct Prepared {
    session: Arc<NetworkSession>,
    wallet_id: WalletId,
    inputs: Vec<OutPoint>,
    txid: Txid,
    fee: u64,
    signed: Mutex<Option<Arc<SignedCoreTransaction>>>,
    phase: Mutex<Phase>,
}

impl Prepared {
    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn signed(&self) -> Option<Arc<SignedCoreTransaction>> {
        self.signed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The signed transaction, consensus-encoded.
    pub(crate) fn raw(&self) -> Option<Vec<u8>> {
        self.signed()
            .map(|s| dashcore::consensus::serialize(s.transaction()))
    }

    async fn release(&self) {
        self.session
            .spends
            .remove(&self.wallet_id, self.inputs.iter().copied());
        let signed = self.signed.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let (Some(signed), Ok(wallet)) = (signed, self.session.wallet(&self.wallet_id).await) {
            wallet.core().abandon_transaction(&signed).await;
        }
    }

    /// Sends the transaction. A first broadcast that never reached a peer
    /// (no peers, rejected before dispatch, rejected by the network)
    /// releases the inputs; one without an acceptance verdict keeps them and
    /// may be sent again.
    async fn broadcast(&self) -> Result<Txid, EngineError> {
        let first = {
            let mut phase = self.phase();
            let first = match *phase {
                Phase::Pending => true,
                Phase::Unknown => false,
                Phase::Broadcasting | Phase::Sent | Phase::Released => {
                    return Err(EngineError::InvalidArgument(
                        "the provider transaction is no longer pending".into(),
                    ));
                }
            };
            *phase = Phase::Broadcasting;
            first
        };
        let outcome = self.dispatch(first).await;
        let next = match &outcome {
            Ok(_) => Phase::Sent,
            Err(EngineError::Masternode(MasternodeFailure::BroadcastRejected(r)))
                if r.starts_with(UNKNOWN_PREFIX) =>
            {
                Phase::Unknown
            }
            Err(EngineError::Masternode(
                MasternodeFailure::NoPeers | MasternodeFailure::BroadcastRejected(_),
            )) if first => Phase::Released,
            Err(_) if first => Phase::Pending,
            Err(_) => Phase::Unknown,
        };
        *self.phase() = next;
        match next {
            Phase::Sent => self
                .session
                .spends
                .remove(&self.wallet_id, self.inputs.iter().copied()),
            Phase::Released => self.release().await,
            _ => {}
        }
        if matches!(next, Phase::Sent | Phase::Unknown) {
            self.session.hub.note_announced(self.txid);
            if let Ok(manager) = self.session.manager() {
                self.session
                    .refresh_wallet_state(&manager, self.wallet_id)
                    .await;
            }
            self.session.hub.pump.mark_balances(self.wallet_id);
            self.session.hub.pump.mark_history(self.wallet_id, None);
            self.session.announce_masternodes();
        }
        outcome
    }

    async fn dispatch(&self, first: bool) -> Result<Txid, EngineError> {
        let wallet = self.session.wallet(&self.wallet_id).await?;
        let manager = self.session.manager()?;
        if !manager.spv().is_started() {
            return Err(if first {
                MasternodeFailure::NoPeers
            } else {
                MasternodeFailure::BroadcastRejected(format!("{UNKNOWN_PREFIX} SPV is not running"))
            }
            .into());
        }
        let signed = self.signed().ok_or_else(|| {
            EngineError::InvalidArgument("the provider transaction was released".into())
        })?;
        let sent = if first {
            wallet.core().broadcast_finalized_transaction(&signed).await
        } else {
            wallet
                .core()
                .broadcast_transaction(signed.transaction())
                .await
        };
        match sent {
            Ok(txid) => Ok(txid),
            Err(PlatformWalletError::TransactionBroadcastUnconfirmed(reason)) => Err(
                MasternodeFailure::BroadcastRejected(format!("{UNKNOWN_PREFIX} {reason}")).into(),
            ),
            Err(other) if !first => Err(MasternodeFailure::BroadcastRejected(format!(
                "{UNKNOWN_PREFIX} not sent this time: {other}"
            ))
            .into()),
            Err(PlatformWalletError::TransactionBroadcast(reason)) => {
                Err(if reason_means_no_peers(&reason) {
                    MasternodeFailure::NoPeers
                } else {
                    MasternodeFailure::BroadcastRejected(reason)
                }
                .into())
            }
            Err(other) => Err(other.into()),
        }
    }

    async fn abandon(&self) -> Result<(), EngineError> {
        {
            let mut phase = self.phase();
            match *phase {
                Phase::Pending => *phase = Phase::Released,
                Phase::Released => return Ok(()),
                Phase::Broadcasting | Phase::Sent | Phase::Unknown => {
                    return Err(EngineError::InvalidArgument(
                        "the provider transaction was handed to the network".into(),
                    ));
                }
            }
        }
        self.release().await;
        Ok(())
    }
}

/// Reason prefix of a broadcast without an acceptance verdict: the inputs
/// stay reserved and `broadcast` may be called again.
pub const UNKNOWN_PREFIX: &str = "outcome unknown:";

impl Drop for Prepared {
    fn drop(&mut self) {
        if *self.phase() != Phase::Pending {
            return;
        }
        let session = Arc::clone(&self.session);
        let wallet_id = self.wallet_id;
        let inputs = std::mem::take(&mut self.inputs);
        let signed = self.signed.lock().unwrap_or_else(|p| p.into_inner()).take();
        session.spends.remove(&wallet_id, inputs.iter().copied());
        let Some(signed) = signed else { return };
        let rt = session.rt.clone();
        rt.spawn(async move {
            let Ok(_op) = session.enter().await else {
                return;
            };
            if let Ok(wallet) = session.wallet(&wallet_id).await {
                wallet.core().abandon_transaction(&signed).await;
            }
        });
    }
}

/// A prepared registration (QT-123/124).
pub struct PreparedRegistration {
    prepared: Prepared,
    summary: RegistrationSummary,
    /// The generated operator secret until submit/abandon.
    secret: Mutex<Option<Zeroizing<[u8; 32]>>>,
    gate_open: AtomicBool,
    collateral: OutPoint,
}

impl std::fmt::Debug for PreparedRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRegistration")
            .field("pro_tx_hash", &self.summary.pro_tx_hash)
            .finish_non_exhaustive()
    }
}

impl PreparedRegistration {
    pub fn summary(&self) -> &RegistrationSummary {
        &self.summary
    }

    pub fn raw(&self) -> Option<Vec<u8>> {
        self.prepared.raw()
    }

    /// The generated secret (64 hex) and the `masternodeblsprivkey=` line,
    /// as ASCII bytes in zeroing buffers.
    pub fn operator_secret(&self) -> Result<(Zeroizing<Vec<u8>>, Zeroizing<Vec<u8>>), EngineError> {
        let guard = self.secret.lock().unwrap_or_else(|p| p.into_inner());
        let secret = guard.as_ref().ok_or_else(|| {
            EngineError::InvalidArgument(
                "no operator secret: the key was not generated, or the registration is done".into(),
            )
        })?;
        let hex = bls::secret_hex(secret);
        let line = Zeroizing::new(format!("masternodeblsprivkey={}", hex.as_str()));
        Ok((
            Zeroizing::new(hex.as_bytes().to_vec()),
            Zeroizing::new(line.as_bytes().to_vec()),
        ))
    }

    /// QT-124 gate: `last4` against the secret's last four hex characters,
    /// case-insensitively.
    pub fn confirm_operator_secret(&self, last4: &str) -> Result<bool, EngineError> {
        let guard = self.secret.lock().unwrap_or_else(|p| p.into_inner());
        let secret = guard.as_ref().ok_or_else(|| {
            EngineError::InvalidArgument("no generated operator secret to confirm".into())
        })?;
        let hex = bls::secret_hex(secret);
        let ok = last4.trim().len() == 4 && hex[60..].eq_ignore_ascii_case(last4.trim());
        if ok {
            self.gate_open.store(true, Ordering::SeqCst);
        }
        Ok(ok)
    }

    /// Broadcasts after the gate; returns the proTxHash. Locks the collateral
    /// once sent.
    pub async fn submit(
        self: &Arc<Self>,
        collateral_signature: Option<String>,
    ) -> Result<String, EngineError> {
        if collateral_signature.is_some() {
            return Err(EngineError::InvalidArgument(
                "a collateral signature is only for external collateral".into(),
            ));
        }
        if self.summary.operator_secret_required && !self.gate_open.load(Ordering::SeqCst) {
            return Err(MasternodeFailure::OperatorSecretUnconfirmed.into());
        }
        let this = Arc::clone(self);
        let session = Arc::clone(&self.prepared.session);
        session
            .on_runtime(async move {
                let _op = this.prepared.session.enter().await?;
                let txid = this.prepared.broadcast().await?;
                *this.secret.lock().unwrap_or_else(|p| p.into_inner()) = None;
                // Dash Core locks masternode collaterals so a payment never
                // spends one (`CWallet::AutoLockMasternodeCollaterals`). The
                // lock is written directly: a collateral the registration
                // created may not be in the wallet's view yet.
                let id = this.prepared.wallet_id.to_string();
                let (ctxid, cvout) = (this.collateral.txid.to_string(), this.collateral.vout);
                let locked = this
                    .prepared
                    .session
                    .appdb_op(move |db| {
                        db.lock_manual(&id, &ctxid, cvout, crate::coins::now_secs())
                            .map(drop)
                    })
                    .await;
                if let Err(e) = locked {
                    tracing::warn!(error = %e, "locking the new masternode collateral failed");
                }
                Ok(txid.to_string())
            })
            .await
    }

    pub async fn abandon(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        let session = Arc::clone(&self.prepared.session);
        session
            .on_runtime(async move {
                let _op = this.prepared.session.enter().await?;
                *this.secret.lock().unwrap_or_else(|p| p.into_inner()) = None;
                this.prepared.abandon().await
            })
            .await
    }
}

/// A signed maintenance transaction (QT-125).
pub struct PreparedProviderTx {
    prepared: Prepared,
    summary: ProviderTxSummary,
}

impl std::fmt::Debug for PreparedProviderTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedProviderTx")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

impl PreparedProviderTx {
    pub fn summary(&self) -> &ProviderTxSummary {
        &self.summary
    }

    pub fn raw(&self) -> Option<Vec<u8>> {
        self.prepared.raw()
    }

    pub async fn broadcast(self: &Arc<Self>) -> Result<String, EngineError> {
        let this = Arc::clone(self);
        let session = Arc::clone(&self.prepared.session);
        session
            .on_runtime(async move {
                let _op = this.prepared.session.enter().await?;
                this.prepared.broadcast().await.map(|t| t.to_string())
            })
            .await
    }

    pub async fn abandon(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        let session = Arc::clone(&self.prepared.session);
        session
            .on_runtime(async move {
                let _op = this.prepared.session.enter().await?;
                this.prepared.abandon().await
            })
            .await
    }
}

/// Maps a vault refusal while redeeming a grant or deriving a key.
pub(crate) fn vault_failure(e: VaultError) -> EngineError {
    match e {
        VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly => {
            MasternodeFailure::VaultLocked.into()
        }
        VaultError::GrantInvalid | VaultError::GrantPurposeMismatch => {
            MasternodeFailure::GrantInvalid.into()
        }
        VaultError::NoSecret => MasternodeFailure::WatchOnly.into(),
        other => EngineError::Vault(other),
    }
}

fn signer_failure(e: dw_vault::SignerError) -> EngineError {
    match e {
        dw_vault::SignerError::Locked => MasternodeFailure::VaultLocked.into(),
        dw_vault::SignerError::NoSecret => MasternodeFailure::WatchOnly.into(),
        dw_vault::SignerError::Vault(v) => vault_failure(v),
        other => EngineError::Internal(other.to_string()),
    }
}

fn builder_error(e: payloads::PayloadError) -> BuilderError {
    match e {
        payloads::PayloadError::Bls(_) | payloads::PayloadError::BadSecret => {
            BuilderError::SigningFailed(e.to_string())
        }
        other => BuilderError::InvalidData(other.to_string()),
    }
}

/// A P2PKH key-id of an address text of this network, or the error for
/// `role`.
fn p2pkh_key(
    text: &str,
    network: dashcore::Network,
    role: MasternodeKeyRole,
) -> Result<[u8; 20], EngineError> {
    match l1_address(text.trim(), network).map(|a| a.payload().clone()) {
        Ok(Payload::PubkeyHash(h)) => Ok(h.to_byte_array()),
        _ => Err(MasternodeFailure::InvalidKey {
            role,
            detail: format!("{text:?} is not a P2PKH address of this network"),
        }
        .into()),
    }
}

/// A payout address: P2PKH or P2SH of this network.
fn payout_address(text: &str, network: dashcore::Network) -> Result<Address, EngineError> {
    l1_address(text.trim(), network).map_err(|_| {
        MasternodeFailure::InvalidPayout(format!("{text:?} is not a Dash address of this network"))
            .into()
    })
}

fn key_id_of(address: &Address) -> Option<[u8; 20]> {
    match address.payload() {
        Payload::PubkeyHash(h) => Some(h.to_byte_array()),
        _ => None,
    }
}

/// Platform node id and ports of a version-2 EvoNode payload.
fn platform_ports(
    fields: &PlatformFields,
    network: dashcore::Network,
    core_port: u16,
) -> Result<PlatformPorts, EngineError> {
    let id = fields.node_id_hex.trim();
    let mut node_id = [0u8; 20];
    if id.len() != 40 || hex::decode_to_slice(id, &mut node_id).is_err() || node_id == [0u8; 20] {
        return Err(MasternodeFailure::InvalidKey {
            role: MasternodeKeyRole::PlatformNode,
            detail: "the Platform node id is 40 hexadecimal characters".into(),
        }
        .into());
    }
    let defaults = params::default_ports(network);
    let port = |list: &[String], default: u16| -> Result<u16, EngineError> {
        match list.first() {
            None => Ok(default),
            Some(text) => service::platform_port(text).ok_or_else(|| {
                MasternodeFailure::InvalidService(format!("{text:?} has no port")).into()
            }),
        }
    };
    let p2p_port = port(&fields.p2p_addresses, defaults.platform_p2p)?;
    let http_port = port(&fields.https_addresses, defaults.platform_https)?;
    service::check_platform_ports(network, core_port, p2p_port, http_port)
        .map_err(|e| MasternodeFailure::InvalidService(e.to_string()))?;
    Ok(PlatformPorts {
        node_id,
        p2p_port,
        http_port,
    })
}

/// What a provider build funds and how it is finished.
struct BuildPlan {
    wallet_id: WalletId,
    fee_source: FeeSourceChoice,
    outputs: Vec<(Address, u64)>,
    payload: TransactionPayload,
    finalizer:
        Box<dyn FnOnce(&Transaction) -> Result<TransactionPayload, payloads::PayloadError> + Send>,
    /// Coins that must not fund it (an existing collateral).
    exclude: HashSet<OutPoint>,
}

impl NetworkSession {
    /// Funds, finalizes and signs a provider transaction; reserves its
    /// inputs. Nothing is sent.
    async fn build_provider_tx(
        self: &Arc<Self>,
        plan: BuildPlan,
        signer: &VaultSigner,
    ) -> Result<Prepared, EngineError> {
        let network = self.network.core_network();
        let wallet = self.wallet(&plan.wallet_id).await?;
        let snapshot = self.coin_snapshot(&wallet, plan.wallet_id).await?;
        let source = match &plan.fee_source {
            FeeSourceChoice::Automatic => None,
            FeeSourceChoice::Address(text) => {
                Some(l1_address(text.trim(), network).map_err(|_| {
                    EngineError::InvalidArgument(format!(
                        "fee source {text:?} is not an address of this network"
                    ))
                })?)
            }
        };
        let candidates: Vec<key_wallet::Utxo> = snapshot
            .coins
            .iter()
            .filter(|c| c.send_account && c.auto_selectable(snapshot.height))
            .filter(|c| !plan.exclude.contains(&c.utxo.outpoint))
            .filter(|c| source.as_ref().is_none_or(|a| &c.utxo.address == a))
            .map(|c| c.utxo.clone())
            .collect();
        let available: u64 = candidates.iter().map(|u| u.value()).sum();
        let total_out: u64 = plan.outputs.iter().map(|(_, v)| *v).sum();
        if candidates.is_empty() || available < total_out {
            return Err(MasternodeFailure::InsufficientFunds {
                needed: total_out,
                available,
            }
            .into());
        }
        let change = match &source {
            Some(a) => a.clone(),
            None => wallet.core().next_change_address_for_account(0).await?,
        };
        let finalizer = plan.finalizer;
        let mut builder = TransactionBuilder::new()
            .set_fee_rate(FeeRate::new(MIN_FEE_PER_KB))
            .set_current_height(snapshot.height)
            .set_selection_strategy(SelectionStrategy::LargestFirst)
            .set_change_address(change)
            .add_inputs(candidates);
        for (address, amount) in &plan.outputs {
            builder = builder.add_output(address, *amount);
        }
        if plan.outputs.is_empty() {
            builder = builder
                .add_op_return(&[])
                .map_err(|e| EngineError::Internal(e.to_string()))?;
        }
        let builder = builder
            .set_special_payload(plan.payload)
            .set_payload_finalizer(move |unsigned| finalizer(unsigned).map_err(builder_error));
        let signed = wallet
            .core()
            .finalize_transaction_with_options(builder, &SEND_FUNDING_SOURCES, 0, signer, true)
            .await
            .map_err(|e| match e {
                PlatformWalletError::CoreInsufficientFunds {
                    available: a,
                    required,
                    ..
                }
                | PlatformWalletError::CorePooledInsufficientFunds {
                    available: a,
                    required,
                    ..
                } => MasternodeFailure::InsufficientFunds {
                    needed: required.unwrap_or(total_out),
                    available: a.unwrap_or(available),
                }
                .into(),
                other => EngineError::from(other),
            })?;
        if signed.fee() > MAX_TX_FEE {
            wallet.core().abandon_transaction(&signed).await;
            return Err(EngineError::Internal(format!(
                "provider transaction fee {} is above the {MAX_TX_FEE} duff ceiling",
                signed.fee()
            )));
        }
        let inputs: Vec<OutPoint> = signed
            .transaction()
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect();
        self.spends.add(plan.wallet_id, inputs.iter().copied());
        Ok(Prepared {
            session: Arc::clone(self),
            wallet_id: plan.wallet_id,
            txid: signed.transaction().txid(),
            fee: signed.fee(),
            inputs,
            signed: Mutex::new(Some(Arc::new(signed))),
            phase: Mutex::new(Phase::Pending),
        })
    }

    /// Redeems a `MasternodeOp` grant for `wallet_id` and opens its signer.
    async fn masternode_signer(
        &self,
        wallet_id: WalletId,
        grant_id: String,
    ) -> Result<(VaultSigner, dw_vault::GrantToken), EngineError> {
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || {
            let token =
                vault.redeem_grant(&grant_id, GrantKind::MasternodeOp, Some(&wallet_id.0))?;
            let signer = vault.signer(&wallet_id.0, &token)?;
            Ok::<_, VaultError>((signer, token))
        })
        .await?
        .map_err(vault_failure)
    }

    /// Refuses early when no key could sign (watch-only wallet, locked
    /// vault), before any work that a redemption would undo.
    fn check_masternode_grant(
        &self,
        wallet_id: WalletId,
        grant_id: &str,
    ) -> Result<(), EngineError> {
        if !self.vault.has_wallet_secret(&wallet_id.0) {
            return Err(MasternodeFailure::WatchOnly.into());
        }
        match self
            .vault
            .check_grant(grant_id, GrantKind::MasternodeOp, Some(&wallet_id.0))
        {
            Err(e @ (VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly)) => {
                Err(vault_failure(e))
            }
            _ => Ok(()),
        }
    }

    /// "Existing wallet UTXO" choices: wallet coins of exactly the
    /// collateral amount, usable ones first.
    pub async fn collateral_candidates(
        self: &Arc<Self>,
        wallet_id: WalletId,
        node_type: MasternodeType,
    ) -> Result<Vec<CollateralCandidate>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.collateral_candidates_inner(wallet_id, node_type).await
        })
        .await
    }

    async fn collateral_candidates_inner(
        self: &Arc<Self>,
        wallet_id: WalletId,
        node_type: MasternodeType,
    ) -> Result<Vec<CollateralCandidate>, EngineError> {
        let amount = collateral_amount(node_type);
        let wallet = self.wallet(&wallet_id).await?;
        let snapshot = self.coin_snapshot(&wallet, wallet_id).await?;
        let collaterals: HashSet<OutPoint> = self
            .masternode_snapshot()
            .await?
            .known
            .values()
            .filter_map(Known::collateral)
            .collect();
        let mut out: Vec<CollateralCandidate> = snapshot
            .coins
            .iter()
            .filter(|c| c.send_account && c.utxo.value() == amount)
            .map(|c| {
                let confirmations = if c.utxo.height > 0 && snapshot.height >= c.utxo.height {
                    snapshot.height - c.utxo.height + 1
                } else {
                    0
                };
                let refusal = if collaterals.contains(&c.utxo.outpoint) {
                    Some(CollateralRefusal::AlreadyCollateral)
                } else if !c.utxo.address.script_pubkey().is_p2pkh() {
                    Some(CollateralRefusal::NotP2pkh)
                } else if c.user_locked || c.reserved {
                    Some(CollateralRefusal::Locked)
                } else if confirmations == 0 {
                    Some(CollateralRefusal::Unconfirmed)
                } else {
                    None
                };
                CollateralCandidate {
                    outpoint: c.utxo.outpoint,
                    address: c.utxo.address.to_string(),
                    amount: c.utxo.value(),
                    confirmations,
                    refusal,
                }
            })
            .collect();
        out.sort_by_key(|c| (c.refusal.is_some(), std::cmp::Reverse(c.confirmations)));
        Ok(out)
    }

    /// The wizard's "Fee source" list: addresses with spendable coins.
    pub async fn fee_source_candidates(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<Vec<FeeSourceCandidate>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let wallet = this.wallet(&wallet_id).await?;
            let snapshot = this.coin_snapshot(&wallet, wallet_id).await?;
            let mut by_address: std::collections::BTreeMap<String, u64> = Default::default();
            for c in snapshot
                .coins
                .iter()
                .filter(|c| c.send_account && c.auto_selectable(snapshot.height))
            {
                *by_address.entry(c.utxo.address.to_string()).or_default() += c.utxo.value();
            }
            let mut out: Vec<FeeSourceCandidate> = by_address
                .into_iter()
                .map(|(address, spendable)| FeeSourceCandidate {
                    address,
                    spendable,
                    label: None,
                })
                .collect();
            out.sort_by_key(|c| std::cmp::Reverse(c.spendable));
            Ok(out)
        })
        .await
    }

    /// A fresh provider owner key address of the wallet: the lowest index
    /// whose key no known masternode uses.
    async fn fresh_owner_address(
        &self,
        wallet_id: WalletId,
        used: &HashSet<[u8; 20]>,
    ) -> Result<Address, EngineError> {
        let wallet = self.wallet(&wallet_id).await?;
        let network = self.network.core_network();
        let used = used.clone();
        tokio::task::spawn_blocking(move || {
            for index in 0..1000 {
                let key = wallet.derive_provider_key_at_index(
                    ProviderKeyKind::Owner,
                    index,
                    None,
                    false,
                )?;
                let address = key.address.ok_or_else(|| {
                    PlatformWalletError::KeyDerivation("owner key without an address".into())
                })?;
                let address = Address::from_str(&address)
                    .map_err(|e| PlatformWalletError::KeyDerivation(e.to_string()))?
                    .require_network(network)
                    .map_err(|e| PlatformWalletError::KeyDerivation(e.to_string()))?;
                if key_id_of(&address).is_some_and(|h| !used.contains(&h)) {
                    return Ok(address);
                }
            }
            Err(PlatformWalletError::KeyDerivation(
                "the first 1000 owner keys are all in use".into(),
            ))
        })
        .await?
        .map_err(EngineError::from)
    }

    /// Builds a registration (QT-123). Rules: m3-engine.md §2.4.
    pub async fn prepare_registration(
        self: &Arc<Self>,
        request: RegistrationRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedRegistration>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.prepare_registration_inner(request, grant_id).await
        })
        .await
    }

    async fn prepare_registration_inner(
        self: &Arc<Self>,
        request: RegistrationRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedRegistration>, EngineError> {
        let network = self.network.core_network();
        let wallet_id = request.wallet_id;
        let wallet = self.wallet(&wallet_id).await?;
        self.check_masternode_grant(wallet_id, &grant_id)?;
        let evo = request.node_type == MasternodeType::Evo;
        let amount = collateral_amount(request.node_type);

        let service_addr = service::single_service(&request.service_addresses, network)
            .map_err(|e| MasternodeFailure::InvalidService(e.to_string()))?;
        let platform = match (&request.platform, evo) {
            (Some(fields), true) => {
                if service::is_no_service(&service_addr) {
                    return Err(MasternodeFailure::InvalidService(
                        "an EvoNode registration needs its Core service address".into(),
                    )
                    .into());
                }
                Some(platform_ports(fields, network, service_addr.port())?)
            }
            (None, true) => {
                return Err(MasternodeFailure::InvalidKey {
                    role: MasternodeKeyRole::PlatformNode,
                    detail: "an EvoNode needs its Platform node id".into(),
                }
                .into());
            }
            (Some(_), false) => {
                return Err(EngineError::InvalidArgument(
                    "Platform fields are for EvoNodes only".into(),
                ));
            }
            (None, false) => None,
        };
        if request.operator_reward_x100 > params::MAX_OPERATOR_REWARD {
            return Err(EngineError::InvalidArgument(
                "the operator reward is at most 100.00%".into(),
            ));
        }
        let (secret, operator_public_key) = match &request.operator_key {
            OperatorKeyChoice::Generate => {
                let secret = bls::generate().map_err(|e| EngineError::Internal(e.to_string()))?;
                let (basic, _) =
                    bls::public_keys(&secret).map_err(|e| EngineError::Internal(e.to_string()))?;
                (Some(secret), basic)
            }
            OperatorKeyChoice::Existing(hex_key) => {
                let key =
                    bls::parse_public_hex(hex_key).map_err(|e| MasternodeFailure::InvalidKey {
                        role: MasternodeKeyRole::Operator,
                        detail: e.to_string(),
                    })?;
                (None, key)
            }
        };

        let snap = self.masternode_snapshot().await?;
        let used_owner_keys: HashSet<[u8; 20]> = snap
            .known
            .values()
            .filter_map(Known::owner_key_hash)
            .collect();
        let owner_key = match &request.owner_address {
            Some(text) => p2pkh_key(text, network, MasternodeKeyRole::Owner)?,
            None => key_id_of(
                &self
                    .fresh_owner_address(wallet_id, &used_owner_keys)
                    .await?,
            )
            .ok_or_else(|| EngineError::Internal("owner address is not P2PKH".into()))?,
        };
        let voting_key = match &request.voting_address {
            Some(text) => p2pkh_key(text, network, MasternodeKeyRole::Voting)?,
            None => owner_key,
        };
        let payout = payout_address(&request.payout_address, network)?;
        let payout_key = key_id_of(&payout);
        if payout_key == Some(owner_key) {
            return Err(MasternodeFailure::DuplicateAddress(
                "the payout address is the owner address".into(),
            )
            .into());
        }
        if payout_key == Some(voting_key) {
            return Err(MasternodeFailure::DuplicateAddress(
                "the payout address is the voting address".into(),
            )
            .into());
        }

        // The collateral and the key that proves it.
        let (collateral_source, collateral_address, exclude, collateral_secret) = match &request
            .collateral
        {
            CollateralChoice::External(_) => {
                return Err(EngineError::NotImplemented(
                    "NetworkSession.prepare_registration.external".into(),
                ));
            }
            CollateralChoice::FundNew => {
                let info = self.next_receive_address(wallet_id, None).await?;
                let address = l1_address(&info.address, network)
                    .map_err(|_| EngineError::Internal("fresh address does not parse".into()))?;
                (
                    CollateralSource::FundNew {
                        script: address.script_pubkey(),
                        amount,
                    },
                    address,
                    HashSet::new(),
                    None,
                )
            }
            CollateralChoice::ExistingUtxo(outpoint) => {
                let candidates = self
                    .collateral_candidates_inner(wallet_id, request.node_type)
                    .await?;
                let candidate = match candidates.iter().find(|c| c.outpoint == *outpoint) {
                    Some(c) => c,
                    None => {
                        let refusal = if self.wallet_holds(&wallet, *outpoint).await {
                            CollateralRefusal::WrongAmount
                        } else {
                            CollateralRefusal::NotFound
                        };
                        return Err(MasternodeFailure::CollateralUnavailable(refusal).into());
                    }
                };
                if let Some(refusal) = candidate.refusal {
                    return Err(MasternodeFailure::CollateralUnavailable(refusal).into());
                }
                let address = l1_address(&candidate.address, network).map_err(|_| {
                    MasternodeFailure::CollateralUnavailable(CollateralRefusal::NotP2pkh)
                })?;
                (
                    CollateralSource::Existing(*outpoint),
                    address,
                    HashSet::from([*outpoint]),
                    Some(()),
                )
            }
        };
        let collateral_key = key_id_of(&collateral_address);
        if collateral_key == Some(owner_key) || collateral_key == Some(voting_key) {
            return Err(MasternodeFailure::DuplicateAddress(
                "the collateral address is the owner or voting address".into(),
            )
            .into());
        }
        if collateral_address == payout {
            return Err(MasternodeFailure::DuplicateAddress(
                "the payout address is the collateral address".into(),
            )
            .into());
        }

        let (signer, _token) = self.masternode_signer(wallet_id, grant_id).await?;
        let registration_signer = match collateral_secret {
            None => RegistrationSigner::FundNew {
                script: collateral_address.script_pubkey(),
                amount,
            },
            Some(()) => {
                let path = self
                    .address_path(&wallet, &collateral_address)
                    .await
                    .ok_or(MasternodeFailure::CollateralUnavailable(
                        CollateralRefusal::NotFound,
                    ))?;
                let signer = signer.clone();
                let secret = tokio::task::spawn_blocking(move || signer.secp_secret(&path))
                    .await?
                    .map_err(signer_failure)?;
                RegistrationSigner::Collateral { secret, network }
            }
        };
        let terms = RegistrationTerms {
            evo,
            collateral: collateral_source.clone(),
            service: service_addr,
            owner_key_hash: owner_key,
            voting_key_hash: voting_key,
            operator_public_key,
            operator_reward: request.operator_reward_x100,
            payout_script: payout.script_pubkey(),
            platform,
        };
        let outputs = match &collateral_source {
            CollateralSource::FundNew { .. } => vec![(collateral_address.clone(), amount)],
            CollateralSource::Existing(_) => Vec::new(),
        };
        let fund_new = matches!(collateral_source, CollateralSource::FundNew { .. });
        let prepared = self
            .build_provider_tx(
                BuildPlan {
                    wallet_id,
                    fee_source: request.fee_source.clone(),
                    outputs,
                    payload: TransactionPayload::ProviderRegistrationPayloadType(
                        payloads::registration_placeholder(&terms),
                    ),
                    finalizer: Box::new(move |tx| {
                        payloads::finalize_registration(tx, &registration_signer)
                    }),
                    exclude,
                },
                &signer,
            )
            .await?;

        let pro_tx_hash = prepared.txid;
        let collateral = match collateral_source {
            CollateralSource::Existing(o) => o,
            CollateralSource::FundNew { .. } => {
                let tx = prepared
                    .signed()
                    .map(|s| s.transaction().clone())
                    .ok_or_else(|| EngineError::Internal("prepared transaction missing".into()))?;
                let Some(TransactionPayload::ProviderRegistrationPayloadType(p)) =
                    &tx.special_transaction_payload
                else {
                    return Err(EngineError::Internal("ProRegTx payload missing".into()));
                };
                payloads::resolve_collateral(pro_tx_hash, p.collateral_outpoint)
            }
        };
        let summary = RegistrationSummary {
            node_type: request.node_type,
            pro_tx_hash: pro_tx_hash.to_string(),
            collateral,
            collateral_address: collateral_address.to_string(),
            owner_address: p2pkh_address(owner_key, network),
            voting_address: p2pkh_address(voting_key, network),
            payout_address: payout.to_string(),
            operator_public_key: hex::encode(operator_public_key),
            operator_reward_x100: request.operator_reward_x100,
            service_addresses: service::service_text(&service_addr).into_iter().collect(),
            platform: request.platform.clone(),
            fee: prepared.fee,
            total_spent: prepared.fee + if fund_new { amount } else { 0 },
            operator_secret_required: secret.is_some(),
            collateral_sign_message: None,
        };
        Ok(Arc::new(PreparedRegistration {
            prepared,
            summary,
            secret: Mutex::new(secret),
            gate_open: AtomicBool::new(false),
            collateral,
        }))
    }

    /// Whether `wallet` holds `outpoint` unspent.
    async fn wallet_holds(
        &self,
        wallet: &platform_wallet::wallet::platform_wallet::PlatformWallet,
        outpoint: OutPoint,
    ) -> bool {
        let state = wallet.state().await;
        super::list::wallet_unspent(&state.core_wallet).contains(&outpoint)
    }

    /// The derivation path of one of the wallet's addresses.
    pub(crate) async fn address_path(
        &self,
        wallet: &platform_wallet::wallet::platform_wallet::PlatformWallet,
        address: &Address,
    ) -> Option<key_wallet::bip32::DerivationPath> {
        let state = wallet.state().await;
        state
            .core_wallet
            .all_managed_accounts()
            .into_iter()
            .find_map(|account| account.get_address_info(address))
            .map(|info| info.path)
    }

    /// The operator secret of an update: typed (checked against the
    /// masternode's key), derived by the fee wallet, or attached to the
    /// tracked masternode.
    async fn operator_secret_for(
        &self,
        known: &Known,
        typed: Option<Zeroizing<Vec<u8>>>,
        fee_wallet: WalletId,
        signer: &VaultSigner,
        token: &dw_vault::GrantToken,
    ) -> Result<Zeroizing<[u8; 32]>, EngineError> {
        let public = known.operator_public_key().ok_or_else(|| {
            MasternodeFailure::UnsupportedEntry("the masternode's operator key is unknown".into())
        })?;
        if let Some(bytes) = typed {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| MasternodeFailure::OperatorSecretMismatch)?;
            let secret = bls::parse_secret_hex(text)
                .map_err(|_| MasternodeFailure::OperatorSecretMismatch)?;
            if !bls::matches(&secret, &public) {
                return Err(MasternodeFailure::OperatorSecretMismatch.into());
            }
            return Ok(secret);
        }
        if let Some(w) = known.wallet.as_ref().filter(|w| w.wallet_id == fee_wallet)
            && let Some(index) = w.operator_key_index
        {
            let wallet = self.wallet(&fee_wallet).await?;
            let signer = signer.clone();
            let secret = tokio::task::spawn_blocking(move || {
                signer.with_seed(|seed| {
                    wallet.derive_provider_key_at_index(
                        ProviderKeyKind::Operator,
                        index,
                        Some(&seed[..]),
                        true,
                    )
                })
            })
            .await?
            .map_err(signer_failure)??;
            let bytes = secret
                .private_key
                .ok_or_else(|| EngineError::Internal("no operator private key".into()))?;
            let mut out = Zeroizing::new([0u8; 32]);
            out.copy_from_slice(&bytes[..32]);
            if !bls::matches(&out, &public) {
                return Err(MasternodeFailure::OperatorSecretMismatch.into());
            }
            return Ok(out);
        }
        if let Some(stored) = self
            .vault
            .masternode_key(
                token,
                &known.hash,
                super::keys::role_name(MasternodeKeyRole::Operator),
            )
            .map_err(vault_failure)?
        {
            let text = std::str::from_utf8(&stored)
                .map_err(|_| EngineError::Internal("attached operator key is not text".into()))?;
            let secret = bls::parse_secret_hex(text).map_err(|_| {
                EngineError::Internal("attached operator key does not parse".into())
            })?;
            if bls::matches(&secret, &public) {
                return Ok(secret);
            }
        }
        Err(MasternodeFailure::KeyNotInWallet(MasternodeKeyRole::Operator).into())
    }

    /// Looks a masternode up for a maintenance flow.
    async fn known_masternode(
        &self,
        pro_tx_hash: &str,
    ) -> Result<(Known, dashcore::Network), EngineError> {
        let hash = parse_pro_tx_hash(pro_tx_hash)?;
        let snap = self.masternode_snapshot().await?;
        let known = snap
            .known
            .get(&hash)
            .cloned()
            .ok_or_else(|| MasternodeFailure::NotFound(pro_tx_hash.to_string()))?;
        Ok((known, snap.network))
    }

    /// Update Service (QT-125, IOS-081 unban): new service, operator
    /// signature.
    pub async fn prepare_update_service(
        self: &Arc<Self>,
        request: UpdateServiceRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.prepare_update_service_inner(request, grant_id).await
        })
        .await
    }

    async fn prepare_update_service_inner(
        self: &Arc<Self>,
        mut request: UpdateServiceRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, EngineError> {
        let wallet_id = request.fee_wallet_id;
        self.wallet(&wallet_id).await?;
        self.check_masternode_grant(wallet_id, &grant_id)?;
        let (known, network) = self.known_masternode(&request.pro_tx_hash).await?;
        if known.list.as_ref().is_some_and(|l| l.has_extended_net_info) {
            return Err(MasternodeFailure::UnsupportedEntry(
                "the masternode advertises v24 extended network info, which a version-2 \
                 ProUpServTx would overwrite"
                    .into(),
            )
            .into());
        }
        let service_addr = match request.service_addresses.as_slice() {
            [] => {
                return Err(MasternodeFailure::InvalidService(
                    "Update Service needs the Core P2P address".into(),
                )
                .into());
            }
            list => service::single_service(list, network)
                .map_err(|e| MasternodeFailure::InvalidService(e.to_string()))?,
        };
        let platform = if known.is_evo() {
            let fields = match request.platform.take() {
                Some(f) => f,
                None => PlatformFields {
                    node_id_hex: known.platform_node_id().map(hex::encode).ok_or_else(|| {
                        MasternodeFailure::InvalidKey {
                            role: MasternodeKeyRole::PlatformNode,
                            detail: "the EvoNode's Platform node id is unknown".into(),
                        }
                    })?,
                    p2p_addresses: Vec::new(),
                    https_addresses: Vec::new(),
                },
            };
            Some(platform_ports(&fields, network, service_addr.port())?)
        } else {
            if request.platform.is_some() {
                return Err(EngineError::InvalidArgument(
                    "Platform fields are for EvoNodes only".into(),
                ));
            }
            None
        };
        // Operator payout (platform-wallet's rule): reward 0 = no payout
        // script; a reward > 0 needs the address given explicitly, since an
        // empty script would clear it on-chain. With the reward unknown the
        // address is required for the same reason.
        let reward = known.wallet.as_ref().and_then(|w| w.operator_reward);
        let payout_script = match (reward, request.operator_payout_address.as_deref()) {
            (Some(0), None) => ScriptBuf::new(),
            (Some(0), Some(_)) => {
                return Err(MasternodeFailure::InvalidPayout(
                    "the operator reward is 0, so there is no operator payout address".into(),
                )
                .into());
            }
            (_, Some(text)) => payout_address(text, network)?.script_pubkey(),
            (Some(_), None) => {
                return Err(MasternodeFailure::InvalidPayout(
                    "the operator payout address must be given: an empty one would clear it".into(),
                )
                .into());
            }
            (None, None) => {
                return Err(MasternodeFailure::InvalidPayout(
                    "the operator reward is unknown to this wallet; give the operator payout \
                     address (an empty one would clear it)"
                        .into(),
                )
                .into());
            }
        };
        let (signer, token) = self.masternode_signer(wallet_id, grant_id).await?;
        let secret = self
            .operator_secret_for(
                &known,
                request.operator_secret.take(),
                wallet_id,
                &signer,
                &token,
            )
            .await?;
        let pro_tx = Txid::from_byte_array(known.hash);
        let placeholder =
            payloads::update_service_placeholder(pro_tx, service_addr, payout_script, platform);
        let prepared = self
            .build_provider_tx(
                BuildPlan {
                    wallet_id,
                    fee_source: request.fee_source.clone(),
                    outputs: Vec::new(),
                    payload: TransactionPayload::ProviderUpdateServicePayloadType(placeholder),
                    finalizer: Box::new(move |tx| payloads::finalize_update_service(tx, &secret)),
                    exclude: HashSet::new(),
                },
                &signer,
            )
            .await?;
        Ok(Arc::new(PreparedProviderTx {
            summary: ProviderTxSummary {
                kind: ProviderTxKind::UpdateService,
                pro_tx_hash: display_hex(&known.hash),
                txid: prepared.txid.to_string(),
                fee: prepared.fee,
                penalty: None,
                bans_masternode: false,
            },
            prepared,
        }))
    }

    /// Update Registrar (QT-125): changed fields only, owner signature.
    pub async fn prepare_update_registrar(
        self: &Arc<Self>,
        request: UpdateRegistrarRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.prepare_update_registrar_inner(request, grant_id).await
        })
        .await
    }

    async fn prepare_update_registrar_inner(
        self: &Arc<Self>,
        request: UpdateRegistrarRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, EngineError> {
        let wallet_id = request.fee_wallet_id;
        let wallet = self.wallet(&wallet_id).await?;
        self.check_masternode_grant(wallet_id, &grant_id)?;
        let (known, network) = self.known_masternode(&request.pro_tx_hash).await?;
        let owner = known
            .owner_key_hash()
            .ok_or(MasternodeFailure::KeyNotInWallet(MasternodeKeyRole::Owner))?;
        let owner_address = Address::new(
            network,
            Payload::PubkeyHash(dashcore::PubkeyHash::from_byte_array(owner)),
        );
        let owner_path = self
            .address_path(&wallet, &owner_address)
            .await
            .ok_or(MasternodeFailure::KeyNotInWallet(MasternodeKeyRole::Owner))?;
        let current_operator = known.operator_public_key().ok_or_else(|| {
            MasternodeFailure::UnsupportedEntry("the masternode's operator key is unknown".into())
        })?;
        let current_voting = known.voting_key_hash().ok_or_else(|| {
            MasternodeFailure::UnsupportedEntry("the masternode's voting key is unknown".into())
        })?;
        let current_payout = known.payout_script().ok_or_else(|| {
            MasternodeFailure::UnsupportedEntry("the masternode's payout is unknown".into())
        })?;
        let operator = match &request.operator_public_key {
            Some(text) => {
                bls::parse_public_hex(text).map_err(|e| MasternodeFailure::InvalidKey {
                    role: MasternodeKeyRole::Operator,
                    detail: e.to_string(),
                })?
            }
            None => current_operator,
        };
        let bans = operator != current_operator
            && bls::legacy_of(&operator).is_none_or(|legacy| legacy != current_operator);
        let voting = match &request.voting_address {
            Some(text) => p2pkh_key(text, network, MasternodeKeyRole::Voting)?,
            None => current_voting,
        };
        let payout = match &request.payout_address {
            Some(text) => payout_address(text, network)?.script_pubkey(),
            None => current_payout,
        };
        let payout_key = Address::from_script(&payout, network)
            .ok()
            .as_ref()
            .and_then(key_id_of);
        if payout_key == Some(owner) || payout_key == Some(voting) {
            return Err(MasternodeFailure::DuplicateAddress(
                "the payout address is the owner or voting address".into(),
            )
            .into());
        }
        let (signer, _token) = self.masternode_signer(wallet_id, grant_id).await?;
        let owner_secret = {
            let signer = signer.clone();
            tokio::task::spawn_blocking(move || signer.secp_secret(&owner_path))
                .await?
                .map_err(signer_failure)?
        };
        let placeholder = payloads::update_registrar_placeholder(
            Txid::from_byte_array(known.hash),
            operator,
            voting,
            payout,
        );
        let prepared = self
            .build_provider_tx(
                BuildPlan {
                    wallet_id,
                    fee_source: request.fee_source.clone(),
                    outputs: Vec::new(),
                    payload: TransactionPayload::ProviderUpdateRegistrarPayloadType(placeholder),
                    finalizer: Box::new(move |tx| {
                        payloads::finalize_update_registrar(tx, &owner_secret)
                    }),
                    exclude: HashSet::new(),
                },
                &signer,
            )
            .await?;
        Ok(Arc::new(PreparedProviderTx {
            summary: ProviderTxSummary {
                kind: ProviderTxKind::UpdateRegistrar,
                pro_tx_hash: display_hex(&known.hash),
                txid: prepared.txid.to_string(),
                fee: prepared.fee,
                penalty: None,
                bans_masternode: bans,
            },
            prepared,
        }))
    }

    /// Revoke (QT-125): operator signature.
    pub async fn prepare_revoke(
        self: &Arc<Self>,
        request: RevokeRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.prepare_revoke_inner(request, grant_id).await
        })
        .await
    }

    async fn prepare_revoke_inner(
        self: &Arc<Self>,
        mut request: RevokeRequest,
        grant_id: String,
    ) -> Result<Arc<PreparedProviderTx>, EngineError> {
        let wallet_id = request.fee_wallet_id;
        self.wallet(&wallet_id).await?;
        self.check_masternode_grant(wallet_id, &grant_id)?;
        let (known, _) = self.known_masternode(&request.pro_tx_hash).await?;
        let (signer, token) = self.masternode_signer(wallet_id, grant_id).await?;
        let secret = self
            .operator_secret_for(
                &known,
                request.operator_secret.take(),
                wallet_id,
                &signer,
                &token,
            )
            .await?;
        let placeholder =
            payloads::revoke_placeholder(Txid::from_byte_array(known.hash), request.reason.code());
        let prepared = self
            .build_provider_tx(
                BuildPlan {
                    wallet_id,
                    fee_source: request.fee_source.clone(),
                    outputs: Vec::new(),
                    payload: TransactionPayload::ProviderUpdateRevocationPayloadType(placeholder),
                    finalizer: Box::new(move |tx| payloads::finalize_revoke(tx, &secret)),
                    exclude: HashSet::new(),
                },
                &signer,
            )
            .await?;
        Ok(Arc::new(PreparedProviderTx {
            summary: ProviderTxSummary {
                kind: ProviderTxKind::Revoke,
                pro_tx_hash: display_hex(&known.hash),
                txid: prepared.txid.to_string(),
                fee: prepared.fee,
                penalty: None,
                bans_masternode: false,
            },
            prepared,
        }))
    }
}

fn collateral_amount(node_type: MasternodeType) -> u64 {
    match node_type {
        MasternodeType::Regular => params::MASTERNODE_COLLATERAL,
        MasternodeType::Evo => params::EVONODE_COLLATERAL,
    }
}

fn p2pkh_address(key: [u8; 20], network: dashcore::Network) -> String {
    Address::new(
        network,
        Payload::PubkeyHash(dashcore::PubkeyHash::from_byte_array(key)),
    )
    .to_string()
}
