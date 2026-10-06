//! Creating and submitting proposals (QT-132, QT-133): Core's
//! `gobject prepare` (a 1 DASH `OP_RETURN <object hash>` collateral
//! transaction from the wallet, plus change) and `gobject submit` (relaying
//! the object once the collateral has a confirmation). The wallet's
//! proposals are kept in app.sqlite (`settings_kv`, scope = wallet id, key
//! `gov.proposal.<hash>`), so they resume after a restart and travel with
//! the wallet's app rows in backups.

use std::sync::Arc;
use std::time::Duration;

use dashcore::blockdata::opcodes::all::OP_RETURN;
use dashcore::blockdata::script::Builder;
use dashcore::hashes::Hash;
use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_governance::net::INV_GOVERNANCE_OBJECT;
use dw_governance::object::{GovernanceObject, display_hex, parse_display_hex};
use dw_governance::params::{PROPOSAL_MIN_SUBMIT_CONFIRMATIONS, governance_params};
use dw_governance::proposal::{self, parse_proposal};
use dw_governance::sync::relay;
use dw_psbt::{InputData, KeyPaths};
use dw_vault::GrantKind;
use key_wallet::Utxo;
use key_wallet::transaction_checking::TransactionContext;
use platform_wallet::broadcaster::{BroadcastError, SpvBroadcaster, TransactionBroadcaster};
use serde::{Deserialize, Serialize};

use super::voting::vault_error;
use super::{CollateralStatus, GovernanceFailure, PendingProposal, ProposalDraft};
use crate::send::FeeMode;
use crate::send::plan::{self, InputChoice, PlanError, PlanOutput};
use crate::{EngineError, NetworkSession, WalletId};

/// Setting key prefix of the wallet's proposals.
const RECORD_PREFIX: &str = "gov.proposal.";
/// How long a submit waits for peers to fetch the object.
const RELAY_WAIT: Duration = Duration::from_secs(20);
/// How long a broadcast waits for SPV peers.
const BROADCAST_READY_WAIT: Duration = Duration::from_secs(30);

/// A stored proposal of the wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRecord {
    /// The `govobj` payload, hex.
    object: String,
    /// Display order.
    collateral_txid: String,
    created_at: u64,
    submitted: bool,
}

/// A wallet proposal with its collateral state.
#[derive(Debug, Clone)]
pub(crate) struct OwnProposal {
    pub object: GovernanceObject,
    pub collateral_txid: Txid,
    pub created_at: u64,
    pub submitted: bool,
    pub confirmations: u32,
    pub status: CollateralStatus,
}

impl OwnProposal {
    /// The Resume Proposals entry; `payment_count` is filled by the caller
    /// (it needs the network's cycle).
    fn pending(&self) -> PendingProposal {
        let data = parse_proposal(&self.object.data).unwrap_or_default();
        PendingProposal {
            hash: display_hex(&self.object.hash()),
            name: data.name.clone().unwrap_or_default(),
            url: data.url.clone().unwrap_or_default(),
            payment_amount: data.payment_amount.unwrap_or(0),
            payment_count: 0,
            collateral_txid: self.collateral_txid.to_string(),
            collateral_status: self.status,
            confirmations: self.confirmations,
            created_at: self.created_at,
            end_epoch: data.end_epoch.unwrap_or(0).max(0) as u64,
        }
    }
}

/// `OP_RETURN <32-byte hash>` (Core `CScript() << OP_RETURN <<
/// ToByteVector(hash)`, internal byte order).
fn collateral_script(hash: &[u8; 32]) -> ScriptBuf {
    Builder::new()
        .push_opcode(OP_RETURN)
        .push_slice(*hash)
        .into_script()
}

impl NetworkSession {
    /// The wallet's stored proposals with their collateral state.
    pub(crate) async fn own_proposals(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<OwnProposal>, EngineError> {
        let scope = wallet.to_string();
        let rows = self
            .appdb_op(move |db| db.settings_with_prefix(&scope, RECORD_PREFIX))
            .await?;
        let tip = self.governance_tip().map(|t| t.height);
        let mut out = Vec::new();
        for (_, value) in rows {
            let Ok(r) = serde_json::from_str::<StoredRecord>(&value) else {
                tracing::warn!("ignoring a malformed stored proposal");
                continue;
            };
            let Some(object) = hex::decode(&r.object)
                .ok()
                .and_then(|b| GovernanceObject::decode(&b).ok())
            else {
                continue;
            };
            let Ok(txid) = r.collateral_txid.parse::<Txid>() else {
                continue;
            };
            let (status, confirmations) = match self.hub.history.get(&wallet, &txid) {
                None => (CollateralStatus::Unknown, 0),
                Some(e) => {
                    let height = match &e.context {
                        TransactionContext::InBlock(i)
                        | TransactionContext::InChainLockedBlock(i) => Some(i.height()),
                        _ => None,
                    };
                    let c = match (height, tip) {
                        (Some(h), Some(t)) if t >= h => t - h + 1,
                        _ => 0,
                    };
                    let s = if c >= PROPOSAL_MIN_SUBMIT_CONFIRMATIONS {
                        CollateralStatus::Ready
                    } else {
                        CollateralStatus::Pending
                    };
                    (s, c)
                }
            };
            out.push(OwnProposal {
                object,
                collateral_txid: txid,
                created_at: r.created_at,
                submitted: r.submitted,
                confirmations,
                status,
            });
        }
        out.sort_by_key(|p| p.created_at);
        Ok(out)
    }

    /// The wallets' own proposal with display hash `hash`, if any.
    pub(crate) async fn find_own_proposal(
        &self,
        hash: &str,
    ) -> Result<Option<OwnProposal>, EngineError> {
        let wallets = match self.manager() {
            Ok(m) => m.list_wallet_ids_blocking(),
            Err(_) => Vec::new(),
        };
        for w in wallets {
            if let Some(p) = self
                .own_proposals(WalletId(w))
                .await?
                .into_iter()
                .find(|p| display_hex(&p.object.hash()) == hash)
            {
                return Ok(Some(p));
            }
        }
        Ok(None)
    }

    async fn store_record(
        &self,
        wallet: WalletId,
        hash: &str,
        r: &StoredRecord,
    ) -> Result<(), EngineError> {
        let scope = wallet.to_string();
        let key = format!("{RECORD_PREFIX}{hash}");
        let value = serde_json::to_string(r).map_err(|e| EngineError::Internal(e.to_string()))?;
        self.appdb_op(move |db| db.set_setting(&scope, &key, Some(&value)))
            .await
    }

    async fn delete_record(&self, wallet: WalletId, hash: &str) -> Result<(), EngineError> {
        let scope = wallet.to_string();
        let key = format!("{RECORD_PREFIX}{hash}");
        self.appdb_op(move |db| db.set_setting(&scope, &key, None))
            .await
    }

    /// Created, not yet submitted, unexpired proposals (Resume Proposals).
    pub async fn pending_proposals(
        self: &Arc<Self>,
        wallet: WalletId,
    ) -> Result<Vec<PendingProposal>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet).await?;
            let now = crate::events::unix_now() as i64;
            let p = governance_params(this.network.core_network());
            Ok(this
                .own_proposals(wallet)
                .await?
                .into_iter()
                .filter(|o| !o.submitted)
                .filter_map(|o| {
                    let data = parse_proposal(&o.object.data)?;
                    if data.end_epoch.is_some_and(|e| e <= now) {
                        return None;
                    }
                    let mut pending = o.pending();
                    pending.payment_count = data.payments_requested(&p);
                    Some(pending)
                })
                .collect())
        })
        .await
    }

    /// `gobject prepare`: validates, pays the 1 DASH collateral and stores
    /// the proposal as pending. `grant_id`: a `Spend` grant covering 1 DASH
    /// plus the fee.
    pub async fn create_proposal(
        self: &Arc<Self>,
        wallet: WalletId,
        draft: ProposalDraft,
        grant_id: String,
    ) -> Result<PendingProposal, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.create_proposal_inner(wallet, draft, grant_id).await
        })
        .await
    }

    async fn create_proposal_inner(
        self: &Arc<Self>,
        wallet_id: WalletId,
        draft: ProposalDraft,
        grant_id: String,
    ) -> Result<PendingProposal, EngineError> {
        let network = self.network.core_network();
        let tip = self.require_tip()?;
        self.check_draft(&draft, tip)?;
        let wallet = self.wallet(&wallet_id).await?;
        if !self.vault.has_wallet_secret(&wallet_id.0) {
            return Err(GovernanceFailure::WatchOnly.into());
        }
        if let Err(
            e @ (dw_vault::VaultError::NoVault
            | dw_vault::VaultError::Locked
            | dw_vault::VaultError::MixingOnly),
        ) = self
            .vault
            .check_grant(&grant_id, GrantKind::Spend, Some(&wallet_id.0))
        {
            return Err(vault_error(e));
        }
        let p = governance_params(network);
        let now = crate::events::unix_now();
        let object = proposal::proposal_object(network, &draft, tip, now as i64);
        let hash = object.hash();
        let hash_hex = display_hex(&hash);

        // Coin selection as a plain send: spendable coins of the standard
        // accounts, the 1 DASH OP_RETURN output, change to P2PKH.
        let snapshot = self.coin_snapshot(&wallet, wallet_id).await?;
        let coins: Vec<Utxo> = snapshot
            .coins
            .iter()
            .filter(|c| c.send_account && c.auto_selectable(snapshot.height))
            .map(|c| c.utxo.clone())
            .collect();
        let output = PlanOutput {
            script: collateral_script(&hash),
            amount: p.proposal_fee,
            subtract_fee: false,
        };
        let rate = FeeMode::Recommended { target_blocks: 6 }.rate()?;
        let planned = plan::plan(
            InputChoice::Select(&coins),
            &[output],
            rate,
            25,
            snapshot.height,
        )
        .map_err(|e| match e {
            PlanError::AmountExceedsBalance { available } => GovernanceFailure::InsufficientFunds {
                needed: p.proposal_fee,
                available,
            },
            PlanError::AmountWithFeeExceedsBalance { fee, available } => {
                GovernanceFailure::InsufficientFunds {
                    needed: p.proposal_fee + fee,
                    available,
                }
            }
            other => GovernanceFailure::BroadcastRejected(other.to_string()),
        })?;
        if planned.fee > crate::send::MAX_TX_FEE {
            return Err(GovernanceFailure::BroadcastRejected(format!(
                "fee {} is absurd",
                planned.fee
            ))
            .into());
        }
        let outflow = p.proposal_fee + planned.fee;

        let vault = self.vault.clone();
        let gid = grant_id.clone();
        let signer = tokio::task::spawn_blocking(move || {
            let token = vault.redeem_grant(&gid, GrantKind::Spend, Some(&wallet_id.0))?;
            if token.max_duffs().unwrap_or(0) < outflow {
                return Err(dw_vault::VaultError::GrantInvalid);
            }
            vault.signer(&wallet_id.0, &token)
        })
        .await?
        .map_err(vault_error)?;

        let mut outputs = vec![TxOut {
            value: p.proposal_fee,
            script_pubkey: collateral_script(&hash),
        }];
        if let Some(change) = planned.change {
            let addr = wallet.core().next_change_address_for_account(0).await?;
            outputs.push(TxOut {
                value: change,
                script_pubkey: addr.script_pubkey(),
            });
        }
        let tx = Transaction {
            version: 3,
            lock_time: 0,
            input: planned
                .inputs
                .iter()
                .map(|u| TxIn {
                    previous_output: u.outpoint,
                    sequence: 0xffff_fffe,
                    ..Default::default()
                })
                .collect(),
            output: outputs,
            special_transaction_payload: None,
        };

        // Sign through a PSBT with the wallet's key paths.
        let (inputs, paths) = {
            let state = wallet.state().await;
            let info = &state.core_wallet;
            let mut paths = KeyPaths::new();
            let mut inputs = Vec::with_capacity(planned.inputs.len());
            for (i, u) in planned.inputs.iter().enumerate() {
                let prev = self
                    .hub
                    .history
                    .get(&wallet_id, &u.outpoint.txid)
                    .map(|e| e.tx)
                    .ok_or_else(|| {
                        EngineError::Internal(format!(
                            "previous transaction {} is not in the wallet's history",
                            u.outpoint.txid
                        ))
                    })?;
                let path = info
                    .all_managed_accounts()
                    .into_iter()
                    .find_map(|a| a.get_address_info(&u.address))
                    .map(|a| a.path)
                    .ok_or_else(|| {
                        EngineError::Internal(format!("no key path for {}", u.address))
                    })?;
                paths.insert(i, path);
                inputs.push(InputData {
                    prev_tx: prev,
                    derivation: None,
                });
            }
            (inputs, paths)
        };
        let n_out = tx.output.len();
        let mut psbt = dw_psbt::create_unsigned(tx, inputs, vec![None; n_out])
            .map_err(|e| EngineError::Internal(format!("PSBT: {e}")))?;
        dw_psbt::sign(&mut psbt, &paths, &signer)
            .await
            .map_err(|e| EngineError::Internal(format!("signing the collateral: {e}")))?;
        drop(signer);
        if !dw_psbt::finalize(&mut psbt) {
            return Err(EngineError::Internal(
                "the collateral is not fully signed".into(),
            ));
        }
        let signed = dw_psbt::extract(&psbt).map_err(|e| EngineError::Internal(e.to_string()))?;
        let txid = signed.txid();

        let mut object = object;
        object.collateral_hash = txid.to_byte_array();
        let record = StoredRecord {
            object: hex::encode(object.encode()),
            collateral_txid: txid.to_string(),
            created_at: now,
            submitted: false,
        };
        let inputs: Vec<OutPoint> = planned.inputs.iter().map(|u| u.outpoint).collect();
        self.spends.add(wallet_id, inputs.iter().copied());
        self.store_record(wallet_id, &hash_hex, &record).await?;

        let manager = self.manager()?;
        let outcome = if manager.spv().is_started() {
            let broadcaster = SpvBroadcaster::new(manager.spv_arc());
            broadcaster.wait_until_ready(BROADCAST_READY_WAIT).await;
            broadcaster.broadcast(&signed).await
        } else {
            Err(BroadcastError::Rejected {
                reason: "SPV is not running".into(),
            })
        };
        match outcome {
            Ok(_) => {}
            // The collateral may be out; the proposal stays pending and its
            // collateral shows as Unknown until the wallet sees it.
            Err(BroadcastError::MaybeSent { reason }) => {
                tracing::warn!(%reason, txid = %txid, "proposal collateral outcome unknown");
            }
            Err(BroadcastError::Rejected { reason }) => {
                self.spends.remove(&wallet_id, inputs);
                self.delete_record(wallet_id, &hash_hex).await?;
                return Err(if crate::send::reason_means_no_peers(&reason) {
                    GovernanceFailure::NoPeers
                } else {
                    GovernanceFailure::BroadcastRejected(reason)
                }
                .into());
            }
        }
        self.hub.note_announced(txid);
        self.refresh_wallet_state(&manager, wallet_id).await;
        self.hub.pump.mark_balances(wallet_id);
        self.hub.pump.mark_history(wallet_id, None);
        self.governance.changed();

        let own = self
            .own_proposals(wallet_id)
            .await?
            .into_iter()
            .find(|o| o.collateral_txid == txid);
        let mut pending = match own {
            Some(o) => o.pending(),
            None => OwnProposal {
                object,
                collateral_txid: txid,
                created_at: now,
                submitted: false,
                confirmations: 0,
                status: CollateralStatus::Unknown,
            }
            .pending(),
        };
        pending.payment_count = draft.payment_count;
        Ok(pending)
    }

    /// `gobject submit`: relays the wallet's proposal once its collateral
    /// has a confirmation. Returns the object hash.
    pub async fn submit_proposal(
        self: &Arc<Self>,
        wallet: WalletId,
        hash: String,
    ) -> Result<String, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet).await?;
            let key = parse_display_hex(&hash).ok_or_else(|| {
                EngineError::InvalidArgument("hash must be 64 hex characters".into())
            })?;
            let own = this
                .own_proposals(wallet)
                .await?
                .into_iter()
                .find(|o| o.object.hash() == key)
                .ok_or_else(|| GovernanceFailure::ProposalNotFound(hash.clone()))?;
            let data = parse_proposal(&own.object.data).unwrap_or_default();
            if data
                .end_epoch
                .is_some_and(|e| e <= crate::events::unix_now() as i64)
            {
                return Err(GovernanceFailure::ProposalExpired.into());
            }
            if own.confirmations < PROPOSAL_MIN_SUBMIT_CONFIRMATIONS {
                return Err(GovernanceFailure::CollateralUnconfirmed {
                    confirmations: own.confirmations,
                }
                .into());
            }
            let candidates = this.governance_peer_candidates().await;
            let tip = this.governance_tip().map_or(0, |t| t.height);
            let cfg = this.governance_sync_config(tip);
            let report = relay(
                &cfg,
                &candidates,
                vec![(INV_GOVERNANCE_OBJECT, key, own.object.encode())],
                RELAY_WAIT,
            )
            .await;
            if report.announced_to == 0 {
                return Err(GovernanceFailure::NoPeers.into());
            }
            if !report.fetched.contains(&key) {
                return Err(GovernanceFailure::BroadcastRejected(
                    "no peer requested the proposal".into(),
                )
                .into());
            }
            let record = StoredRecord {
                object: hex::encode(own.object.encode()),
                collateral_txid: own.collateral_txid.to_string(),
                created_at: own.created_at,
                submitted: true,
            };
            this.store_record(wallet, &hash, &record).await?;
            this.governance
                .shared
                .store()
                .add_object(own.object.clone(), this.network.core_network());
            this.governance.changed();
            Ok(hash)
        })
        .await
    }
}
