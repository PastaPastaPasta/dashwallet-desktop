//! The session's side of a masternode withdrawal: its [`Env`], the wallet's
//! keys (P2), the grant's signer and lease (P3, P12), the Platform it reads,
//! the crate-level calls the facade binds (`masternodes/withdrawals.rs`),
//! the quote's `withdrawal_ready` wait (P4) and the background recheck (P8).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::provider_registration::MasternodePayoutShare;
use dashcore::{Network, OutPoint, Transaction, Txid};
use dpp::identity::IdentityPublicKey;
use dpp::identity::core_script::CoreScript;
use dpp::state_transition::StateTransition;
use dw_appdb::{AppDb, PayoutBarrierRow, WithdrawalMark, WithdrawalRow};
use dw_vault::{GrantKind, GrantPurpose, SignerError, WithdrawalKey};
use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use dpp::document::Document;
use dpp::identity::Identity;
use dpp::prelude::Identifier;
use dpp::version::PlatformVersion;

use super::background::Job;
use super::check::{Quote, look_until, quote};
use super::net::{Answer, MnWithdrawNet, Proved, SdkMnWithdrawals};
use super::recheck::{mark, recheck_all, recheck_record};
use super::sign::{Request, WithdrawalKeyChoice, retry, withdraw};
use super::{
    Alarm, Authorize, Chain, Env, HeldKey, Hold, PayoutChange, Waits, WalletKeys, WithdrawalError,
    WithdrawalRuntime, load, on_db,
};
use crate::history::{Finality, SweptBy, TxEntry, block_beats, instant_locked};
use crate::masternodes::protx_records::ProTxOperationState;
use crate::masternodes::{MasternodesError, display_hex, parse_pro_tx_hash};
use crate::platform::trust::Rule5;
use crate::platform::withdrawal::destination;
use crate::platform::{BudgetPurpose, FlowKind};
use crate::platform::{PlatformError, WithdrawAmount};
use crate::{EngineError, NetworkSession, WalletId};

/// The payout `pro_tx_hash`'s ProRegTx (`true`) or a ProUpRegTx of it
/// (`false`) sets. `None` for a payload this version does not decode, or
/// for several DIP-0026 shares of which none is P2SH. Any P2SH share is
/// named: an owner-key withdrawal would pay the P2PKH of its 20 bytes.
fn payout_set(
    txid: &Txid,
    tx: &Transaction,
    pro_tx_hash: &[u8; 32],
) -> Option<(Option<Vec<u8>>, bool)> {
    let one =
        |script: &dashcore::ScriptBuf, payouts: Option<&[MasternodePayoutShare]>| match payouts {
            None => Some(script.to_bytes()),
            Some([one]) => Some(one.script_payout.to_bytes()),
            Some(shares) => shares
                .iter()
                .find(|share| {
                    dashcore::Script::from_bytes(share.script_payout.as_bytes()).is_p2sh()
                })
                .map(|share| share.script_payout.to_bytes()),
        };
    match &tx.special_transaction_payload {
        Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(p))
            if p.pro_tx_hash.to_byte_array() == *pro_tx_hash =>
        {
            Some((one(&p.script_payout, p.payouts.as_deref()), false))
        }
        Some(TransactionPayload::ProviderRegistrationPayloadType(p))
            if txid.to_byte_array() == *pro_tx_hash =>
        {
            Some((one(&p.script_payout, p.payouts.as_deref()), true))
        }
        Some(TransactionPayload::RawPayloadType(3, bytes))
            if bytes.get(2..34) == Some(&pro_tx_hash[..]) =>
        {
            Some((None, false))
        }
        _ => None,
    }
}

type SeenPayout<'a> = ((Option<Vec<u8>>, bool), Vec<&'a TxEntry>);

/// Every ProRegTx and ProUpRegTx of `pro_tx_hash` in `histories`, grouped by txid.
fn seen_payout_changes<'a>(
    histories: &[&'a HashMap<Txid, TxEntry>],
    pro_tx_hash: &[u8; 32],
) -> HashMap<Txid, SeenPayout<'a>> {
    let mut seen: HashMap<Txid, SeenPayout<'a>> = HashMap::new();
    for (txid, e) in histories.iter().flat_map(|h| h.iter()) {
        if let Some(set) = payout_set(txid, &e.tx, pro_tx_hash) {
            seen.entry(*txid).or_insert((set, Vec::new())).1.push(e);
        }
    }
    seen
}

/// Whether `txid` is mined in any loaded wallet's history.
fn is_mined_anywhere(histories: &[&HashMap<Txid, TxEntry>], txid: &Txid) -> bool {
    histories.iter().any(|h| {
        h.get(txid)
            .is_some_and(|e| e.context.block_info().is_some())
    })
}

/// The unmined reading of `txid` across `histories`: `(instant_locked, inputs)`,
/// or `None` if `txid` is mined in any wallet or absent from every wallet.
fn unmined_anywhere<'a>(
    histories: &[&'a HashMap<Txid, TxEntry>],
    txid: &Txid,
) -> Option<(bool, &'a [dashcore::TxIn])> {
    if is_mined_anywhere(histories, txid) {
        return None;
    }
    let mut locked = false;
    let mut inputs = None;
    for h in histories {
        if let Some(e) = h.get(txid) {
            locked |= instant_locked(e);
            inputs.get_or_insert(e.tx.input.as_slice());
        }
    }
    inputs.map(|i| (locked, i))
}

/// The unmined ancestor closure of the unconfirmed, non-abandoned payout
/// changes of `pro_tx_hash`, built across `histories` without cloning any
/// `TxEntry` (#982, #953).
struct PayoutClosure {
    /// Unmined transactions in the closure: `txid -> (instant_locked, inputs)`.
    unmined: HashMap<Txid, (bool, Vec<OutPoint>)>,
    /// All outpoints spent by any transaction in `unmined`.
    watched_inputs: HashSet<OutPoint>,
    /// Unmined txids and non-mined parent txids whose `swept_by` notes may
    /// mark them conflicted (#953).
    swept_targets: HashSet<Txid>,
}

fn payout_closure(
    histories: &[&HashMap<Txid, TxEntry>],
    seen: &HashMap<Txid, SeenPayout<'_>>,
) -> PayoutClosure {
    let mut queue: Vec<Txid> = seen
        .iter()
        .filter_map(|(txid, (_, entries))| {
            let mined = entries.iter().any(|e| e.context.block_info().is_some());
            if mined {
                return None;
            }
            let locked = entries.iter().any(|e| instant_locked(e));
            let abandoned = !locked && entries.iter().any(|e| e.abandoned);
            (!abandoned).then_some(*txid)
        })
        .collect();
    let mut unmined: HashMap<Txid, (bool, Vec<OutPoint>)> = HashMap::new();
    let mut watched_inputs: HashSet<OutPoint> = HashSet::new();
    let mut swept_targets: HashSet<Txid> = HashSet::new();
    while let Some(id) = queue.pop() {
        if unmined.contains_key(&id) {
            continue;
        }
        swept_targets.insert(id);
        let Some((locked, tx_inputs)) = unmined_anywhere(histories, &id) else {
            continue;
        };
        let inputs: Vec<OutPoint> = tx_inputs.iter().map(|i| i.previous_output).collect();
        for o in &inputs {
            watched_inputs.insert(*o);
        }
        if !locked {
            for o in &inputs {
                let parent = o.txid;
                if !is_mined_anywhere(histories, &parent) {
                    swept_targets.insert(parent);
                    if !unmined.contains_key(&parent)
                        && unmined_anywhere(histories, &parent).is_some()
                    {
                        queue.push(parent);
                    }
                }
            }
        }
        unmined.insert(id, (locked, inputs));
    }
    PayoutClosure {
        unmined,
        watched_inputs,
        swept_targets,
    }
}

/// Candidate rival block heights (at most `at_most`) that could conflict an
/// unconfirmed payout change of `pro_tx_hash` or one of its unmined ancestors
/// (#982, #953). Empty when no unconfirmed payout change has a mined rival.
pub(super) fn payout_change_rival_heights(
    histories: &[&HashMap<Txid, TxEntry>],
    swept_by: &[&BTreeMap<Txid, SweptBy>],
    pro_tx_hash: &[u8; 32],
    at_most: u32,
) -> BTreeSet<u32> {
    let seen = seen_payout_changes(histories, pro_tx_hash);
    let closure = payout_closure(histories, &seen);
    if closure.unmined.is_empty() {
        return BTreeSet::new();
    }
    let mut heights = BTreeSet::new();
    for entry in histories.iter().flat_map(|h| h.values()) {
        let Some(block) = entry.context.block_info() else {
            continue;
        };
        if block.height() <= at_most
            && entry
                .tx
                .input
                .iter()
                .any(|i| closure.watched_inputs.contains(&i.previous_output))
        {
            heights.insert(block.height());
        }
    }
    for target in &closure.swept_targets {
        for map in swept_by {
            if let Some(note) = map.get(target)
                && !note.claim
                && let Some((height, _)) = note.block
                && height <= at_most
            {
                heights.insert(height);
            }
        }
    }
    heights
}

/// The ProRegTx and ProUpRegTxs of `pro_tx_hash` in the loaded wallets'
/// `histories`, once each, excluding those out of a block that a wallet
/// abandoned or that a block `finality` proves final conflicts (MG-04's
/// reading, #740, #953, #982).
#[cfg(test)]
pub(super) fn payout_changes_in(
    histories: &[&HashMap<Txid, TxEntry>],
    pro_tx_hash: &[u8; 32],
    chainlock: Option<u32>,
    finality: &Finality,
) -> Vec<PayoutChange> {
    payout_changes_in_with_swept(histories, &[], pro_tx_hash, chainlock, finality)
}

/// [`payout_changes_in`] with each wallet's persisted `swept_by` notes (#953),
/// walking only the unmined ancestors of unconfirmed payout changes without
/// cloning any wallet history (#982).
pub(super) fn payout_changes_in_with_swept(
    histories: &[&HashMap<Txid, TxEntry>],
    swept_by: &[&BTreeMap<Txid, SweptBy>],
    pro_tx_hash: &[u8; 32],
    chainlock: Option<u32>,
    finality: &Finality,
) -> Vec<PayoutChange> {
    let seen = seen_payout_changes(histories, pro_tx_hash);
    let closure = payout_closure(histories, &seen);
    let mut rivals: HashMap<OutPoint, Vec<(Txid, &TransactionContext)>> = HashMap::new();
    if !closure.watched_inputs.is_empty() {
        for (id, entry) in histories.iter().flat_map(|h| h.iter()) {
            if entry.context.block_info().is_none() {
                continue;
            }
            for input in &entry.tx.input {
                if closure.watched_inputs.contains(&input.previous_output) {
                    rivals
                        .entry(input.previous_output)
                        .or_default()
                        .push((*id, &entry.context));
                }
            }
        }
    }
    let swept_beats = |txid: &Txid, locked: bool| -> bool {
        swept_by.iter().any(|map| {
            map.get(txid).is_some_and(|note| {
                !note.claim
                    && note.block.is_some_and(|(height, hash)| {
                        block_beats(
                            &TransactionContext::InBlock(BlockInfo::new(height, hash, 0)),
                            locked,
                            chainlock,
                            |b| finality.proves(b),
                        )
                    })
            })
        })
    };
    let mut memo: HashMap<Txid, bool> = HashMap::new();
    let mut visiting: HashSet<Txid> = HashSet::new();
    #[allow(clippy::too_many_arguments)]
    fn is_conflicted(
        txid: Txid,
        closure: &PayoutClosure,
        rivals: &HashMap<OutPoint, Vec<(Txid, &TransactionContext)>>,
        histories: &[&HashMap<Txid, TxEntry>],
        swept_beats: &dyn Fn(&Txid, bool) -> bool,
        chainlock: Option<u32>,
        finality: &Finality,
        memo: &mut HashMap<Txid, bool>,
        visiting: &mut HashSet<Txid>,
    ) -> bool {
        if let Some(&cached) = memo.get(&txid) {
            return cached;
        }
        if !visiting.insert(txid) {
            return false;
        }
        let result = if let Some((locked, inputs)) = closure.unmined.get(&txid) {
            let direct = inputs.iter().any(|o| {
                rivals.get(o).is_some_and(|list| {
                    list.iter().any(|(wid, ctx)| {
                        *wid != txid && block_beats(ctx, *locked, chainlock, |b| finality.proves(b))
                    })
                })
            }) || swept_beats(&txid, *locked);
            if direct {
                true
            } else if *locked {
                false
            } else {
                inputs.iter().any(|o| {
                    let parent = o.txid;
                    if is_mined_anywhere(histories, &parent) {
                        false
                    } else if closure.unmined.contains_key(&parent) {
                        is_conflicted(
                            parent,
                            closure,
                            rivals,
                            histories,
                            swept_beats,
                            chainlock,
                            finality,
                            memo,
                            visiting,
                        )
                    } else {
                        swept_beats(&parent, false)
                    }
                })
            }
        } else {
            false
        };
        visiting.remove(&txid);
        memo.insert(txid, result);
        result
    }
    let change = |(txid, ((script, registration), entries)): (Txid, SeenPayout<'_>)| {
        let height = (entries.iter())
            .filter_map(|e| e.context.block_info().map(|b| b.height()))
            .max();
        if height.is_none() {
            let locked = entries.iter().any(|e| instant_locked(e));
            let abandoned = !locked && entries.iter().any(|e| e.abandoned);
            if abandoned
                || is_conflicted(
                    txid,
                    &closure,
                    &rivals,
                    histories,
                    &swept_beats,
                    chainlock,
                    finality,
                    &mut memo,
                    &mut visiting,
                )
            {
                return None;
            }
        }
        Some(PayoutChange {
            script,
            height,
            registration,
        })
    };
    seen.into_iter().filter_map(change).collect()
}

/// The session as the withdrawal's core reads it.
pub(crate) struct SessionEnv {
    session: Arc<NetworkSession>,
    db: Arc<AppDb>,
}

impl SessionEnv {
    pub(crate) fn of(session: &Arc<NetworkSession>) -> Result<Self, WithdrawalError> {
        Ok(Self {
            session: Arc::clone(session),
            db: session.live()?.appdb,
        })
    }
}

impl Env for SessionEnv {
    fn db(&self) -> &Arc<AppDb> {
        &self.db
    }

    fn rule_5(&self) -> Rule5 {
        self.session.trust.strict_rule_5()
    }

    /// The synced header tip (`sync.rs` `tip_height()`, not the chain-locked
    /// height, P4 (a)), only once dash-spv is caught up.
    fn chain(&self) -> Chain {
        #[cfg(test)]
        if let Some(seam) = &*(self.session.masternodes.withdrawals.seam.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Chain {
                header_tip: seam.header_tip,
            };
        }
        let tracker = self.session.hub.tracker();
        Chain {
            header_tip: tracker.caught_up().then(|| tracker.tip_height()).flatten(),
        }
    }

    fn barriers_ready(&self) -> bool {
        self.session.masternodes.barriers_ready()
    }

    /// The operation's row is read on the tracked path (`on_db`), so close
    /// waits for it like every other app database call of the runtime.
    async fn barrier_state(
        &self,
        barrier: &PayoutBarrierRow,
    ) -> Result<Option<ProTxOperationState>, WithdrawalError> {
        let Ok(wallet) = barrier.barrier.created_by.parse::<WalletId>() else {
            return Ok(None);
        };
        let (w, id) = (wallet.to_string(), barrier.barrier.op_id.clone());
        #[cfg(test)]
        let pause = (self.session.masternodes.withdrawals.pause_op_read)
            .lock()
            .unwrap()
            .take();
        let row = on_db(self, move |db| {
            #[cfg(test)]
            if let Some(pause) = pause {
                return pause.hold_then(|| db.masternode_op(&w, &id));
            }
            db.masternode_op(&w, &id)
        })
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(self.session.barrier_operation_state(wallet, &row).await?)
    }

    async fn payout_changes(&self, pro_tx_hash: &[u8; 32]) -> Vec<PayoutChange> {
        let hub = &self.session.hub;
        // Fast path (#982): when no unconfirmed, non-abandoned payout change
        // of `pro_tx_hash` exists, return the confirmed changes directly with
        // zero appdb reads and zero SPV header reads.
        let wallets = match hub.history.read_wallets(|map| {
            let histories: Vec<&HashMap<Txid, TxEntry>> = map.values().map(|h| &h.txs).collect();
            let seen = seen_payout_changes(&histories, pro_tx_hash);
            let has_unconfirmed = seen.values().any(|(_, entries)| {
                let mined = entries.iter().any(|e| e.context.block_info().is_some());
                if mined {
                    return false;
                }
                let locked = entries.iter().any(|e| instant_locked(e));
                let abandoned = !locked && entries.iter().any(|e| e.abandoned);
                !abandoned
            });
            if !has_unconfirmed {
                Ok(seen
                    .into_iter()
                    .filter_map(|(_, ((script, registration), entries))| {
                        let height = entries
                            .iter()
                            .filter_map(|e| e.context.block_info().map(|b| b.height()))
                            .max()?;
                        Some(PayoutChange {
                            script,
                            height: Some(height),
                            registration,
                        })
                    })
                    .collect())
            } else {
                Err(map.keys().copied().collect::<Vec<WalletId>>())
            }
        }) {
            Ok(confirmed) => return confirmed,
            Err(wallets) => wallets,
        };
        let swept_by: Vec<BTreeMap<Txid, SweptBy>> = on_db(self, move |db| {
            let mut out = Vec::with_capacity(wallets.len());
            for &w in &wallets {
                let map = crate::tx_actions::load_swept_by(db, w)?;
                if !map.is_empty() {
                    out.push(map);
                }
            }
            Ok(out)
        })
        .await
        .unwrap_or_default();
        let swept_refs: Vec<&BTreeMap<Txid, SweptBy>> = swept_by.iter().collect();
        let finality = (self.session)
            .finality(|at_most| {
                (hub.history).read_all(|histories| {
                    payout_change_rival_heights(histories, &swept_refs, pro_tx_hash, at_most)
                })
            })
            .await;
        let chainlock =
            (hub.tracker().chainlock_height()).max(hub.best_chainlock().map(|(h, _)| h));
        (hub.history).read_all(|histories| {
            payout_changes_in_with_swept(histories, &swept_refs, pro_tx_hash, chainlock, &finality)
        })
    }

    fn waits(&self) -> Waits {
        self.session.masternodes.withdrawals.waits()
    }

    fn now(&self) -> u64 {
        crate::events::unix_now()
    }

    fn alarm(&self, alarm: Alarm) {
        self.session.masternodes.withdrawals.alarm(alarm);
    }

    fn hold(&self) -> Hold {
        self.session.masternodes.withdrawals.hold()
    }
}

/// The wallet's withdrawal keys of one masternode (P2): the keychain owner
/// key by platform-wallet's derive-and-compare and its funding accounts'
/// payout addresses (`masternode_withdrawal_keys`,
/// `rs-platform-wallet/src/wallet/masternode_withdrawal.rs:158-201`, each
/// TRANSFER key's hash160 handed over as its P2PKH script), and the keys
/// attached to the tracked masternode, by the vault's key ids.
pub(crate) struct SessionKeys {
    session: Arc<NetworkSession>,
    wallet: WalletId,
    /// Wire order.
    pro_tx_hash: [u8; 32],
}

impl WalletKeys for SessionKeys {
    async fn held(
        &self,
        owner: Vec<[u8; 20]>,
        payout: Vec<[u8; 20]>,
    ) -> Result<Vec<HeldKey>, WithdrawalError> {
        let manager = self.session.manager()?;
        let wallet = (manager.get_wallet(&self.wallet.0).await)
            .ok_or(EngineError::WalletNotFound(self.wallet.to_string()))?;
        let (vault, hash) = (self.session.vault.clone(), self.pro_tx_hash);
        Ok(tokio::task::spawn_blocking(move || {
            let mut held = keychain_owner_keys(&wallet, &owner);
            held.extend(wallet_payout_keys(&wallet, &payout));
            held.extend(attached_keys(&vault, &hash, &owner, &payout));
            held
        })
        .await?)
    }
}

/// The keychain owner keys among `owner` (hash160s).
fn keychain_owner_keys(wallet: &PlatformWallet, owner: &[[u8; 20]]) -> Vec<HeldKey> {
    let mut held = Vec::new();
    for h in owner {
        match wallet.masternode_withdrawal_keys(Some(h), None, None) {
            Ok(k) => {
                if let Some((index, _)) = k.owner_key {
                    held.push(HeldKey {
                        key: WithdrawalKey::KeychainOwner { index },
                        hash: *h,
                    });
                }
            }
            Err(e) => tracing::debug!(error = %e, "the keychain owner key"),
        }
    }
    held
}

/// The wallet's payout keys among `payout` (hash160s), by their P2PKH
/// scripts.
fn wallet_payout_keys(wallet: &PlatformWallet, payout: &[[u8; 20]]) -> Vec<HeldKey> {
    let mut held = Vec::new();
    for h in payout {
        let script = CoreScript::new_p2pkh(*h);
        match wallet.masternode_withdrawal_keys(None, Some(script.as_bytes()), None) {
            Ok(k) => {
                if let Some(path) = k.transfer_key {
                    held.push(HeldKey {
                        key: WithdrawalKey::WalletPayout { path },
                        hash: *h,
                    });
                }
            }
            Err(e) => tracing::debug!(error = %e, "the wallet's payout key"),
        }
    }
    held
}

/// The masternode's attached `owner` and `payout` keys among `owner` and
/// `payout`. They need the vault's full-scope key; a locked vault shows
/// none (it could not sign either).
fn attached_keys(
    vault: &dw_vault::Vault,
    hash: &[u8; 32],
    owner: &[[u8; 20]],
    payout: &[[u8; 20]],
) -> Vec<HeldKey> {
    let mut held = Vec::new();
    for (role, hashes) in [("owner", owner), ("payout", payout)] {
        match vault.masternode_key_id(hash, role) {
            Ok(Some(id)) if hashes.contains(&id) => held.push(HeldKey {
                key: if role == "owner" {
                    WithdrawalKey::AttachedOwner { pro_tx_hash: *hash }
                } else {
                    WithdrawalKey::AttachedPayout { pro_tx_hash: *hash }
                },
                hash: id,
            }),
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %e, role, "an attached withdrawal key"),
        }
    }
    held
}

/// The grant, redeemed into a `masternode_withdraw` lease whose Credits
/// budget is charged the amount and the reserve, and its single-use
/// `MasternodeWithdrawal` signer (P3, P12). The charge goes back only when
/// nothing was signed.
pub(crate) struct LeaseGrant {
    pub(super) session: Arc<NetworkSession>,
    pub(super) wallet: WalletId,
    pub(super) grant: String,
}

impl Authorize for LeaseGrant {
    async fn sign(
        self,
        credits: u64,
        key: &WithdrawalKey,
        identity_key: &IdentityPublicKey,
        transition: &StateTransition,
        pro_tx_hash: [u8; 32],
    ) -> Result<[u8; 65], WithdrawalError> {
        let lease = (self.session)
            .begin_lease(self.wallet, FlowKind::MasternodeWithdraw, &[self.grant])
            .await
            .map_err(PlatformError::from)?;
        let signed = async {
            let (charge, signer) =
                (lease.charge_withdrawal_signer(credits)).map_err(PlatformError::from)?;
            let (key, identity_key, transition) =
                (key.clone(), identity_key.clone(), transition.clone());
            let signed = tokio::task::spawn_blocking(move || {
                signer.sign_masternode_withdrawal(&key, &identity_key, &transition, &pro_tx_hash)
            })
            .await?;
            signed.map_err(|e| {
                lease.refund(BudgetPurpose::Credits, charge);
                match e {
                    SignerError::KeyMismatch(_) => WithdrawalError::KeyMissing,
                    e => EngineError::from(e).into(),
                }
            })
        }
        .await;
        lease.end();
        signed
    }
}

/// `withdraw_credits`' arguments as the facade passes them (P13).
#[derive(Debug, Clone)]
pub(crate) struct WithdrawCall {
    pub(crate) pro_tx_hash: String,
    pub(crate) key: WithdrawalKeyChoice,
    pub(crate) amount: WithdrawAmount,
    /// A payout-key withdrawal's address; `None`: the signing key's.
    pub(crate) destination: Option<String>,
}

/// The Platform the session's withdrawal calls read and send to: the SDK's
/// proved reads in their own trust scopes ([`SdkMnWithdrawals`]); in tests,
/// the scripted one the runtime's seam holds.
enum SessionNet {
    Sdk(SdkMnWithdrawals),
    #[cfg(test)]
    Scripted(Arc<super::tests::Platform>),
}

impl MnWithdrawNet for SessionNet {
    fn version(&self) -> &'static PlatformVersion {
        match self {
            Self::Sdk(n) => n.version(),
            #[cfg(test)]
            Self::Scripted(n) => n.version(),
        }
    }

    async fn identity(&self, id: Identifier) -> Result<Proved<Option<Identity>>, PlatformError> {
        match self {
            Self::Sdk(n) => n.identity(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.identity(id).await,
        }
    }

    async fn nonce_state(&self, id: Identifier) -> Result<Proved<u64>, PlatformError> {
        match self {
            Self::Sdk(n) => n.nonce_state(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.nonce_state(id).await,
        }
    }

    async fn balance(&self, id: Identifier) -> Result<Proved<Option<u64>>, PlatformError> {
        match self {
            Self::Sdk(n) => n.balance(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.balance(id).await,
        }
    }

    async fn document(&self, id: Identifier) -> Result<Proved<Option<Document>>, PlatformError> {
        match self {
            Self::Sdk(n) => n.document(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.document(id).await,
        }
    }

    async fn broadcast(&self, bytes: &[u8]) -> Answer {
        match self {
            Self::Sdk(n) => n.broadcast(bytes).await,
            #[cfg(test)]
            Self::Scripted(n) => n.broadcast(bytes).await,
        }
    }
}

impl NetworkSession {
    /// Tests: what a quote's look does to the payout-change barriers of
    /// `pro_tx_hash` when no Platform read passes, so only their
    /// operations' states clear them (MG-13 P16): it deletes those it
    /// clears.
    #[cfg(test)]
    pub(crate) async fn withdrawal_clear_barriers(self: &Arc<Self>, pro_tx_hash: &[u8; 32]) {
        let env = SessionEnv::of(self).unwrap();
        (super::check::clear_barriers_unproved(&env, pro_tx_hash).await).unwrap();
    }

    /// Tests: the heights of the ProUpRegTxs of `pro_tx_hash` a withdrawal
    /// reads from the history (`None` out of a block); its ProRegTx is left
    /// out.
    #[cfg(test)]
    pub(crate) async fn withdrawal_payout_changes(
        self: &Arc<Self>,
        pro_tx_hash: &[u8; 32],
    ) -> Vec<Option<u32>> {
        let env = SessionEnv::of(self).unwrap();
        (env.payout_changes(pro_tx_hash).await)
            .into_iter()
            .filter(|c| !c.registration)
            .map(|c| c.height)
            .collect()
    }

    /// The Platform the withdrawal calls use;
    /// `masternodes.platform_unavailable` in a session without Platform.
    fn mn_withdraw_net(&self) -> Result<SessionNet, WithdrawalError> {
        #[cfg(test)]
        if let Some(seam) = &*(self.masternodes.withdrawals.seam.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Ok(SessionNet::Scripted(Arc::clone(&seam.platform)));
        }
        if !self.platform.enabled() {
            return Err(MasternodesError::PlatformUnavailable {
                detail: "this session runs without Platform".into(),
            }
            .into());
        }
        Ok(SessionNet::Sdk(SdkMnWithdrawals(self.manager()?.sdk_arc())))
    }

    /// The masternode's keys, refusing a shared masternode (F8, P2).
    async fn withdrawal_keys(
        self: &Arc<Self>,
        wallet: WalletId,
        pro_tx_hash: &str,
    ) -> Result<([u8; 32], SessionKeys), WithdrawalError> {
        let hash = parse_pro_tx_hash(pro_tx_hash)?;
        if crate::masternodes::reads::owner_facts(self, wallet, hash)
            .await?
            .shared
        {
            return Err(WithdrawalError::Shared);
        }
        Ok((
            hash,
            SessionKeys {
                session: Arc::clone(self),
                wallet,
                pro_tx_hash: hash,
            },
        ))
    }

    /// `withdrawal_quote` (P13), waiting up to `wait` while only Platform is
    /// behind (P4); the facade reads once and leaves the wait to
    /// [`Self::mn_withdrawal_ready_wait`].
    pub(crate) async fn mn_withdrawal_quote(
        self: &Arc<Self>,
        wallet: WalletId,
        pro_tx_hash: &str,
        wait: std::time::Duration,
    ) -> Result<Quote, WithdrawalError> {
        let net = &self.mn_withdraw_net()?;
        let (hash, keys) = self.withdrawal_keys(wallet, pro_tx_hash).await?;
        let env = SessionEnv::of(self)?;
        quote(&env, net, &keys, self.network.core_network(), &hash, wait).await
    }

    fn request(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
        call: &WithdrawCall,
    ) -> Result<Request, WithdrawalError> {
        let network: Network = self.network.core_network();
        let destination = match &call.destination {
            Some(to) => Some(destination(network, to)?),
            None => None,
        };
        Ok(Request {
            pro_tx_hash: hash,
            key: call.key,
            amount: call.amount,
            destination,
            created_by: wallet.to_string(),
        })
    }

    /// `withdraw_credits` (P13) under `grant` (a `MasternodeWithdraw`
    /// grant), or with `sibling_of` the `refused` record's Try again
    /// (`retry_withdrawal`, P15).
    pub(crate) async fn mn_withdraw(
        self: &Arc<Self>,
        wallet: WalletId,
        call: &WithdrawCall,
        grant: &str,
        sibling_of: Option<&str>,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        if self.platform.enabled() && self.wallet_platform_held(wallet) {
            return Err(crate::platform::PlatformError::Restored.into());
        }
        let hash = parse_pro_tx_hash(&call.pro_tx_hash)?;
        // The grant names the masternode the user acted on: one for
        // another is refused here, unredeemed, and its signer would refuse
        // to sign this one (dw-vault). A grant this read can't name goes on
        // to the lease, which refuses it again.
        if let Ok(GrantPurpose::MasternodeWithdraw { pro_tx_hash, .. }) =
            (self.vault).grant_purpose(grant, GrantKind::MasternodeWithdraw, Some(&wallet.0))
            && pro_tx_hash != hash
        {
            return Err(WithdrawalError::TargetMismatch);
        }
        let net = &self.mn_withdraw_net()?;
        let (_, keys) = self.withdrawal_keys(wallet, &call.pro_tx_hash).await?;
        let req = self.request(wallet, hash, call)?;
        let env = SessionEnv::of(self)?;
        let auth = LeaseGrant {
            session: Arc::clone(self),
            wallet,
            grant: grant.to_owned(),
        };
        let network = self.network.core_network();
        match sibling_of {
            None => withdraw(&env, net, &keys, auth, network, &req).await,
            Some(refused) => retry(&env, net, &keys, auth, network, refused, &req).await,
        }
    }

    /// `recheck_withdrawal` (P13).
    pub(crate) async fn mn_recheck_withdrawal(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        let net = self.mn_withdraw_net()?;
        recheck_record(&SessionEnv::of(self)?, &net, id).await
    }

    /// `mark_withdrawal` (P9, P13).
    pub(crate) async fn mn_mark_withdrawal(
        self: &Arc<Self>,
        id: &str,
        to: WithdrawalMark,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        mark(&SessionEnv::of(self)?, id, to).await
    }

    /// The withdrawal record `id`.
    pub(crate) async fn mn_withdrawal(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        load(&SessionEnv::of(self)?, id).await
    }

    /// P4's `withdrawal_ready`: after a quote whose destination read
    /// `checking{platform_behind}` or `checking{unverified}` (H-09), the look
    /// again every `Waits::poll` for at most `Waits::quote` (240 s) while
    /// only (a) or (c) fails, and §6's event once
    /// (a)–(c) hold, so the host reads the quote again. Before consent: no
    /// grant waits on it. One per masternode; none starts once the close
    /// has begun, and the close stops the running ones.
    pub(crate) fn mn_withdrawal_ready_wait(self: &Arc<Self>, pro_tx_hash: [u8; 32]) {
        let weak = Arc::downgrade(self);
        let wait = async move {
            let Some(s) = weak.upgrade() else { return };
            let (Ok(env), Ok(net)) = (SessionEnv::of(&s), s.mn_withdraw_net()) else {
                return;
            };
            let wait = env.waits().quote;
            match look_until(&env, &net, &pro_tx_hash, wait).await {
                Ok(seen) if seen.reason.is_none() => s.masternodes.local_change(),
                Ok(seen) => tracing::debug!(reason = ?seen.reason,
                    "a masternode withdrawal's destination still checking"),
                Err(e) => tracing::debug!(error = %e,
                    "waiting for Platform to catch up (MG-13 P4)"),
            }
        };
        (self.masternodes.withdrawals).wait_ready(&self.rt, pro_tx_hash, wait);
    }

    /// `withdrawals(pro_tx_hash)` (P13), oldest first.
    pub(crate) async fn mn_withdrawals(
        self: &Arc<Self>,
        pro_tx_hash: &str,
    ) -> Result<Vec<WithdrawalRow>, WithdrawalError> {
        let shown = display_hex(&parse_pro_tx_hash(pro_tx_hash)?);
        on_db(&SessionEnv::of(self)?, move |db| {
            db.withdrawals_of_masternode(&shown)
        })
        .await
    }
}

impl WithdrawalRuntime {
    /// Starts the background recheck (P8): every undecided group at once,
    /// then every `Waits::recheck` while the session lives; nothing to do
    /// when nothing is undecided. Not with Platform off, nor once
    /// [`Self::stop`] has begun; it aborts it and waits for it.
    pub(crate) fn start(&self, session: &Arc<NetworkSession>) {
        if !session.platform.enabled() {
            return;
        }
        #[cfg(test)]
        {
            let gate = &session.hub.restore;
            *gate.restored_at_recheck_start.lock().unwrap() = Some(gate.is_restored());
        }
        let weak = Arc::downgrade(session);
        let task = async move {
            loop {
                let Some(s) = weak.upgrade() else { return };
                let every = s.masternodes.withdrawals.waits().recheck;
                if !s.any_platform_held()
                    && let (Ok(env), Ok(net)) = (SessionEnv::of(&s), s.mn_withdraw_net())
                {
                    #[cfg(test)]
                    (s.hub.restore.recheck_runs).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Err(e) = recheck_all(&env, &net).await {
                        tracing::info!(error = %e, "the masternode withdrawals' recheck");
                    }
                }
                drop(s);
                tokio::time::sleep(every).await;
            }
        };
        self.work.spawn(&session.rt, Job::Recheck, task);
    }
}
