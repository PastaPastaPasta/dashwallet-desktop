//! The session side of the shared calls (MG-04b §7, §9): reading and
//! saving `shared_mn_sessions`, the chain view a stage is derived from, the
//! keys and coins a party commits (D10, D9), the checks every signing call
//! runs (§4, §5, D17), and the follower's lock release (§11).

use std::collections::{BTreeMap, HashMap, HashSet};

use dashcore::hashes::Hash as _;
use dashcore::{OutPoint, Txid};
use dw_appdb::{SharedSessionRow, SharedSessionUpdate};
use dw_protx::shared::rules as shared_rules;
use dw_protx::shared::session::consent::check_consent;
use dw_protx::shared::session::funding::{Frozen, check_frozen, decode_destination, key_id_of};
use dw_protx::shared::session::merge::check_join_signatures;
use dw_protx::shared::session::{Session, Share};
use dw_vault::{SignerError, VaultError};
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use key_wallet::{DerivationPath, Utxo};
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use super::super::MasternodesError;
use super::super::follow::load_notes;
use super::super::protx::build::{Context, refused, tx_size};
use super::super::protx::keys::{InUse, p2pkh_id};
use super::super::records::HeldShares;
use super::super::shared_records::{RewardChoice, SharedRole, SharedSession, SharedStage};
use super::build::{Selected, placeholder, select};
use super::store::{
    Approved, ChainView, Loaded, Recorded, RecordedChange, consent_refusal, derive_stage,
    encode_outpoints, envelope_refusal, funding_refusal, json, out_stage, releasable, reserved_of,
    role_of, session_hash, stage_refusal, view,
};
use crate::coins::{CoinFence, now_secs, own_address_info, read_coins};
use crate::send::plan::fee_for;
use crate::send::{MIN_FEE_PER_KB, SendFailure};
use crate::{EngineError, NetworkSession, WalletId};

/// Labels of the addresses a session issues (address book, D10).
const REFUND_LABEL: &str = "Shared masternode refund";
pub(super) const REWARD_LABEL: &str = "Shared masternode reward";

/// A party's own row as it commits it (D10): the keychain owner key, a
/// fresh refund address and the reward.
pub(crate) struct OwnRow {
    pub owner_index: u32,
    pub owner_address: String,
    pub refund_address: String,
    /// `""`: rewards go to the refund address.
    pub reward_address: String,
}

impl OwnRow {
    /// Writes the three addresses into the session's row of this party.
    pub(crate) fn fill(&self, row: &mut Share) {
        row.owner_address.clone_from(&self.owner_address);
        row.refund_address.clone_from(&self.refund_address);
        row.reward_address.clone_from(&self.reward_address);
    }
}

impl From<&Recorded> for OwnRow {
    fn from(rec: &Recorded) -> Self {
        Self {
            owner_index: rec.owner_index,
            owner_address: rec.owner_address.clone(),
            refund_address: rec.refund_address.clone(),
            reward_address: rec.reward_address.clone(),
        }
    }
}

/// A vault signer's refusal as §10 names it, never with the vault's text.
pub(crate) fn signer_refusal(e: SignerError) -> MasternodesError {
    match e {
        SignerError::KeyMismatch(_) => MasternodesError::ShareNotMine,
        SignerError::Vault(VaultError::InvalidArgument(_)) => funding_refusal("template"),
        SignerError::Vault(v) => v.into(),
        SignerError::Locked => VaultError::Locked.into(),
        SignerError::NoSecret => VaultError::NoSecret.into(),
        _ => EngineError::Internal("the owner key did not sign".into()).into(),
    }
}

/// The request's reward address, checked before anything else (§7's
/// arguments): P2PKH or P2SH on the session's network.
pub(crate) fn check_reward(
    reward: &RewardChoice,
    network: dashcore::Network,
) -> Result<(), MasternodesError> {
    match reward {
        RewardChoice::Address { address }
            if decode_destination(address.trim(), network).is_none() =>
        {
            Err(
                EngineError::InvalidArgument("the reward is not a P2PKH or P2SH address".into())
                    .into(),
            )
        }
        _ => Ok(()),
    }
}

impl NetworkSession {
    /// D3: v24 is active once a version-3 entry was seen (MG-03's durable
    /// notes), else unknown.
    pub(crate) async fn v24_active(&self) -> Result<bool, MasternodesError> {
        let notes = self.appdb_op(load_notes).await?;
        Ok(notes.any_shared_seen || !notes.shared_seen.is_empty())
    }

    pub(crate) async fn shared_rows(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<SharedSessionRow>, MasternodesError> {
        let w = wallet.to_string();
        Ok(self.appdb_op(move |db| db.shared_sessions(&w)).await?)
    }

    /// The stored row of a session of the wallet, if it has one.
    pub(crate) async fn shared_row(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<Option<SharedSessionRow>, MasternodesError> {
        let (w, i) = (wallet.to_string(), id.to_string());
        Ok(self.appdb_op(move |db| db.shared_session(&w, &i)).await?)
    }

    /// A stored row's stage; one whose envelope no longer parses reads as
    /// its role's out stage (D57).
    pub(crate) async fn stored_stage(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
    ) -> Result<SharedStage, MasternodesError> {
        Ok(match Loaded::of(row.clone(), self.network.core_network()) {
            Ok(l) => derive_stage(&l, &self.shared_chain(wallet).await?),
            Err(_) => out_stage(role_of(&row.role)),
        })
    }

    /// A stored session, parsed (`shared_session_not_found` otherwise).
    pub(crate) async fn shared_loaded(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<Loaded, MasternodesError> {
        let row = self
            .shared_row(wallet, id)
            .await?
            .ok_or(MasternodesError::SharedSessionNotFound)?;
        Loaded::of(row, self.network.core_network())
    }

    /// What a session's stage is derived from (§9.2).
    pub(crate) async fn shared_chain(
        &self,
        wallet: WalletId,
    ) -> Result<ChainView, MasternodesError> {
        let (history, unspent) = self.op_view(wallet).await?;
        #[cfg(test)]
        let history = {
            use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
            let mut history = history;
            let mined = (self.masternodes.protx_seam())
                .map(|s| s.shared_mined.lock().unwrap().clone())
                .unwrap_or_default();
            for (txid, tx, height) in mined {
                let info = BlockInfo::new(height, dashcore::BlockHash::from_byte_array([0; 32]), 0);
                history.insert(
                    txid,
                    crate::history::TxEntry {
                        tx,
                        context: TransactionContext::InBlock(info),
                        instant_locked: false,
                        first_seen: None,
                        own_inputs: Default::default(),
                        abandoned: false,
                    },
                );
            }
            history
        };
        Ok(ChainView { history, unspent })
    }

    pub(crate) async fn shared_view(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<SharedSession, MasternodesError> {
        let l = self.shared_loaded(wallet, id).await?;
        let chain = self.shared_chain(wallet).await?;
        Ok(view(&l, &chain))
    }

    /// Every owner key of every share of the wallet's sessions, and the
    /// keys their terms name, count as in use (D10). A session whose
    /// envelope no longer parses still holds the keychain owner keys its
    /// `my_shares` column names; an index that names none holds nothing
    /// (D64).
    pub(crate) async fn shared_keys_in_use(
        &self,
        wallet: WalletId,
        used: &mut InUse,
    ) -> Result<(), MasternodesError> {
        let network = self.network.core_network();
        let mut unreadable = Vec::new();
        for row in self.shared_rows(wallet).await? {
            let owners = owner_indexes(&row.my_shares);
            let Ok(l) = Loaded::of(row, network) else {
                unreadable.extend(owners);
                continue;
            };
            for share in &l.session.shares {
                used.owner.extend(key_id_of(&share.owner_address, network));
            }
            used.voting
                .extend(key_id_of(&l.session.terms.voting_address, network));
            if let Ok(key) = dw_protx::bls::parse_public_hex(&l.session.terms.operator_pub_key) {
                used.operator.insert(key);
            }
        }
        if !unreadable.is_empty() {
            let keys = self.owner_key_ids(wallet).await?;
            used.owner.extend(
                keys.into_iter()
                    .filter(|(index, _)| unreadable.contains(index))
                    .map(|(_, id)| id),
            );
        }
        Ok(())
    }

    /// This wallet's row of a session (D10): the lowest free keychain owner
    /// key (which then counts as in use), a fresh receive address as
    /// refund, the reward.
    pub(crate) async fn own_row(
        &self,
        wallet: WalletId,
        used: &mut InUse,
        reward: &RewardChoice,
    ) -> Result<OwnRow, MasternodesError> {
        let owner = self.pick_owner(wallet, None, used).await?;
        let refund = self.issued(wallet, REFUND_LABEL).await?.to_string();
        let reward_address = match reward {
            RewardChoice::Refund => String::new(),
            RewardChoice::NewAddress => self.issued(wallet, REWARD_LABEL).await?.to_string(),
            RewardChoice::Address { address } => address.trim().to_string(),
        };
        let row = OwnRow {
            owner_index: owner.index.ok_or_else(|| {
                EngineError::Internal("a keychain owner key without index".into())
            })?,
            owner_address: owner.address,
            refund_address: refund,
            reward_address,
        };
        used.owner
            .extend(p2pkh_id(&row.owner_address, self.network.core_network()));
        Ok(row)
    }

    /// The coins a contribution may take: the automatic-selection
    /// candidates the owner signer signs for, plus `also` (session `id`'s
    /// own reserved coins, for `set_shared_fee`) while unspent and no other
    /// row holds them (BL-346).
    async fn shared_candidates(
        &self,
        fence: &CoinFence,
        w: &PlatformWallet,
        id: &str,
        also: &[OutPoint],
    ) -> Result<Vec<(Utxo, DerivationPath)>, MasternodesError> {
        let mut coins = self
            .protx_candidates(fence, w, &HashSet::new(), None)
            .await?;
        if !also.is_empty() {
            let others = self.held_by_others(fence.wallet_id(), id).await?;
            let state = w.state().await;
            for c in read_coins(&state.core_wallet).coins {
                if also.contains(&c.utxo.outpoint)
                    && !others.contains(&c.utxo.outpoint)
                    && !coins.iter().any(|(u, _)| u.outpoint == c.utxo.outpoint)
                    && let Some(info) = own_address_info(&state.core_wallet, &c.utxo.address)
                {
                    coins.push((c.utxo.clone(), info.path));
                }
            }
        }
        Ok(coins)
    }

    /// Selects `target` and names the change (a fresh internal address, or
    /// `reuse`), as this wallet records it (§4, m-1).
    async fn shared_selection(
        &self,
        w: &PlatformWallet,
        candidates: Vec<(Utxo, DerivationPath)>,
        target: u64,
        reuse: Option<&str>,
    ) -> Result<(Selected, Option<RecordedChange>), MasternodesError> {
        let selected = select(candidates, target).map_err(EngineError::from)?;
        let change = match selected.change {
            Some(amount) => Some(RecordedChange {
                address: match reuse {
                    Some(a) => a.to_string(),
                    None => w
                        .core()
                        .next_change_address_for_account(0)
                        .await
                        .map_err(EngineError::from)?
                        .to_string(),
                },
                amount,
            }),
            None => None,
        };
        Ok((selected, change))
    }

    /// The reservation of session `id`'s contribution (D9): `target`
    /// selected from the candidates (and `also`, see
    /// [`Self::shared_candidates`]) under the wallet's coin fence, which the
    /// caller took before it picked the keys and holds until the row is
    /// written (D63).
    pub(crate) async fn shared_reserve(
        &self,
        fence: &CoinFence,
        id: &str,
        also: &[OutPoint],
        target: u64,
        reuse: Option<&str>,
    ) -> Result<(Selected, Option<RecordedChange>), MasternodesError> {
        let w = self.wallet(&fence.wallet_id()).await?;
        let candidates = self.shared_candidates(fence, &w, id, also).await?;
        self.shared_selection(&w, candidates, target, reuse).await
    }

    /// This wallet's coins of `outpoints` with their scripts and key paths,
    /// each still an unspent wallet coin (D17: `shared_stage{dead}`
    /// otherwise).
    pub(crate) async fn reserved_inputs(
        &self,
        wallet: WalletId,
        l: &Loaded,
        outpoints: &[OutPoint],
    ) -> Result<Vec<super::build::OwnInput>, MasternodesError> {
        let dead = || {
            stage_refusal(SharedStage::Dead {
                reason: super::super::shared_records::DeadReason::InputsSpent,
            })
        };
        // Each one reserved by this session, by no other row (BL-346), and
        // still persistently locked: a released lock lets another send
        // select the coin.
        let reserved = reserved_of(&l.row);
        let others = self.held_by_others(wallet, &l.row.id).await?;
        let w = wallet.to_string();
        let locked: HashSet<OutPoint> = self
            .appdb_op(move |db| db.locks(&w))
            .await?
            .into_iter()
            .filter(|r| r.is_locked())
            .filter_map(|r| Some(OutPoint::new(r.txid.parse().ok()?, r.vout)))
            .collect();
        if !outpoints
            .iter()
            .all(|o| reserved.contains(o) && !others.contains(o) && locked.contains(o))
        {
            return Err(dead());
        }
        let w = self.wallet(&wallet).await?;
        let state = w.state().await;
        let coins = read_coins(&state.core_wallet).coins;
        outpoints
            .iter()
            .map(|o| {
                let coin = coins.iter().find(|c| c.utxo.outpoint == *o)?;
                let path = own_address_info(&state.core_wallet, &coin.utxo.address)?.path;
                Some((*o, coin.utxo.txout.script_pubkey.clone(), path))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(dead)
    }

    /// The coins a masternode row other than session `id` holds (BL-346).
    async fn held_by_others(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<HashSet<OutPoint>, MasternodesError> {
        let (w, id) = (wallet.to_string(), id.to_string());
        let held = self
            .appdb_op(move |db| db.held_outpoints(&w, dw_appdb::Holder::Session(&id)))
            .await?;
        Ok(held
            .iter()
            .filter_map(|(txid, vout)| Some(OutPoint::new(txid.parse().ok()?, *vout)))
            .collect())
    }

    /// Writes `session` and this wallet's side over the stored row.
    pub(crate) async fn save_shared(
        &self,
        wallet: WalletId,
        l: &Loaded,
        session: &Session,
        approved: Option<&Approved>,
    ) -> Result<(), MasternodesError> {
        let update = SharedSessionUpdate {
            envelope: session.to_text(),
            my_shares: l.row.my_shares.clone(),
            contribution: l.row.contribution.clone(),
            approved_terms: approved.map(json).transpose()?,
            signed_tx: None,
            txid: None,
            last_error: None,
            updated_at: now_secs(),
        };
        let (w, id) = (wallet.to_string(), l.row.id.clone());
        let saved = self
            .appdb_op(move |db| db.update_shared_session_durable(&w, &id, &update))
            .await?;
        if !saved {
            return Err(MasternodesError::SharedSessionNotFound);
        }
        self.masternodes.local_change();
        Ok(())
    }

    /// A new session row with its reservation locked in the same write.
    pub(crate) async fn insert_shared(
        &self,
        wallet: WalletId,
        session: &Session,
        role: SharedRole,
        recorded: Option<&Recorded>,
        reserved: &[OutPoint],
        approved: Option<&Approved>,
    ) -> Result<(), MasternodesError> {
        let now = now_secs();
        let my_shares = match recorded {
            Some(r) => my_shares_json(r)?,
            None => "{}".into(),
        };
        let row = SharedSessionRow {
            id: session.session_id.clone(),
            kind: "registration".into(),
            role: super::store::role_name(role).into(),
            envelope: session.to_text(),
            my_shares,
            contribution: recorded.map(json).transpose()?,
            reserved: json(&encode_outpoints(reserved))?,
            approved_terms: approved.map(json).transpose()?,
            base_keys: None,
            signed_tx: None,
            txid: None,
            out_at: None,
            abandon_txid: None,
            created_at: now,
            updated_at: now,
            last_error: None,
        };
        let w = wallet.to_string();
        self.appdb_op(move |db| db.insert_shared_session_durable(&w, &row, now))
            .await?;
        self.masternodes.local_change();
        Ok(())
    }

    /// Replaces this wallet's contribution, share map and reservation
    /// (join, fee change) in one write with its locks.
    pub(crate) async fn replace_contribution(
        &self,
        wallet: WalletId,
        l: &Loaded,
        session: &Session,
        recorded: &Recorded,
    ) -> Result<(), MasternodesError> {
        let now = now_secs();
        let (w, id) = (wallet.to_string(), l.row.id.clone());
        let reserved = json(&recorded.inputs)?;
        let contribution = json(recorded)?;
        let my_shares = my_shares_json(recorded)?;
        let envelope = session.to_text();
        let replaced = self
            .appdb_op(move |db| {
                db.replace_shared_session_reservation_durable(
                    &w,
                    &id,
                    &reserved,
                    Some(&contribution),
                    &my_shares,
                    &envelope,
                    now,
                    now,
                )
            })
            .await?;
        if !replaced {
            return Err(MasternodesError::SharedSessionNotFound);
        }
        self.masternodes.local_change();
        Ok(())
    }

    /// §4's checks before a consent or funding signature, in §7's order:
    /// the frozen registration (`check_frozen`, D12's template), Core's
    /// rules (§5), at `combined` every join signature (D12), at lock the fee
    /// floor (`fee_floor`, §3.1 step 3, D26), then this wallet's own side
    /// (`check_consent`, D16, m-1). Returns the registration checked.
    pub(crate) fn consent_checks(
        &self,
        l: &Loaded,
        ctx: &Context,
        unspent: &HashSet<OutPoint>,
        combined: bool,
        fee_floor: Option<&Recorded>,
    ) -> Result<Frozen, MasternodesError> {
        let network = self.network.core_network();
        let frozen = check_frozen(&l.session).map_err(envelope_refusal)?;
        shared_rules::check_registration(
            &frozen.tx,
            &frozen.payload,
            &ctx.listed,
            true,
            network,
            combined,
        )
        .map_err(refused)?;
        if combined {
            check_join_signatures(&l.session, &frozen)
                .map_err(|_| funding_refusal("join_signature"))?;
        }
        if let Some(rec) = fee_floor {
            check_fee_floor(&frozen, rec)?;
        }
        let rec = l
            .recorded
            .as_ref()
            .ok_or_else(|| stage_refusal(SharedStage::Invited))?;
        let reserved = reserved_of(&l.row);
        check_consent(&l.session, &frozen, &rec.own_side(), |o| {
            unspent.contains(o) || reserved.contains(o)
        })
        .map_err(consent_refusal)?;
        Ok(frozen)
    }

    /// The ProTx hashes of the wallet's registration sessions that hold
    /// their complete registration (D30's "shared" at the pin).
    pub(crate) async fn shared_pro_tx_hashes(
        &self,
        wallet: WalletId,
    ) -> Result<HashSet<Txid>, MasternodesError> {
        let rows = self.shared_rows(wallet).await?;
        if rows.is_empty() {
            return Ok(HashSet::new());
        }
        let chain = self.shared_chain(wallet).await?;
        Ok(loaded_all(rows, self.network.core_network())
            .iter()
            .filter_map(|l| session_hash(l, &chain))
            .collect())
    }

    /// The wallet's session whose registration is confirmed with ProTx hash
    /// `txid`, and the height it confirmed at.
    pub(crate) async fn shared_confirmed(
        &self,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(Loaded, u32), MasternodesError> {
        let chain = self.shared_chain(wallet).await?;
        let rows = self.shared_rows(wallet).await?;
        loaded_all(rows, self.network.core_network())
            .into_iter()
            .find_map(|l| match derive_stage(&l, &chain) {
                SharedStage::Confirmed { height } if session_hash(&l, &chain) == Some(txid) => {
                    Some((l, height))
                }
                _ => None,
            })
            .ok_or(MasternodesError::SharedRegistrationUnknown)
    }

    /// The shared masternodes the wallet's confirmed sessions registered,
    /// with this wallet's shares of each (§13; a share key without its
    /// session is MG-03b's, D29).
    pub(crate) async fn held_shares(
        &self,
        wallet: WalletId,
    ) -> Result<HashMap<[u8; 32], HeldShares>, MasternodesError> {
        let rows = self.shared_rows(wallet).await?;
        if rows.iter().all(|r| r.out_at.is_none()) {
            return Ok(HashMap::new());
        }
        let chain = self.shared_chain(wallet).await?;
        let mut held: HashMap<[u8; 32], HeldShares> = HashMap::new();
        for l in loaded_all(rows, self.network.core_network()) {
            let (SharedStage::Confirmed { .. }, Some(txid)) =
                (derive_stage(&l, &chain), session_hash(&l, &chain))
            else {
                continue;
            };
            let of = u8::try_from(l.session.shares.len()).unwrap_or(u8::MAX);
            let entry = held
                .entry(txid.to_byte_array())
                .or_insert(HeldShares { held: 0, of });
            entry.held = entry.held.saturating_add(1);
        }
        Ok(held)
    }

    /// The follower's pass over the wallet's sessions (§11): a session
    /// read `confirmed` or `dead`, or `conflicted` by a buried spend
    /// ([`releasable`]), releases its locks; one out and open never does
    /// (D17). Each row is judged by
    /// [`Self::follow_shared_row`] against one chain snapshot read after
    /// the rows.
    pub(crate) async fn shared_follow_pass(
        &self,
        wallet: WalletId,
    ) -> Result<(), MasternodesError> {
        let rows = self.shared_rows(wallet).await?;
        if rows.iter().all(|r| reserved_of(r).is_empty()) {
            return Ok(());
        }
        let chain = self.shared_chain(wallet).await?;
        let mut released = false;
        for listed in &rows {
            released |= self.follow_shared_row(wallet, listed, &chain).await?;
        }
        if released {
            self.masternodes.local_change();
            self.hub.pump.mark_balances(wallet);
        }
        Ok(())
    }

    /// One row of the pass, `chain` read after `listed` (D60). A session a
    /// call holds in flight waits for the next pass. The rest are read again
    /// under the mark, and one that changed since `listed` waits too: a call
    /// may have re-reserved it onto coins newer than `chain`, which would
    /// read as spent. The release writes only while the reservation is still
    /// the one judged. Whether it released.
    pub(crate) async fn follow_shared_row(
        &self,
        wallet: WalletId,
        listed: &SharedSessionRow,
        chain: &ChainView,
    ) -> Result<bool, MasternodesError> {
        if reserved_of(listed).is_empty() {
            return Ok(false);
        }
        let Some(_flight) = self.try_claim_shared_session(wallet, &listed.id) else {
            return Ok(false);
        };
        let Some(row) = self.shared_row(wallet, &listed.id).await? else {
            return Ok(false);
        };
        if &row != listed {
            return Ok(false);
        }
        if !releasable(&row, self.network.core_network(), chain, self.buried()) {
            return Ok(false);
        }
        let (w, id, expected) = (wallet.to_string(), row.id.clone(), row.reserved);
        Ok(self
            .appdb_op(move |db| {
                db.release_shared_session_locks_durable(&w, &id, &expected, now_secs())
            })
            .await?)
    }
}

/// The `my_shares` column: the share this wallet took and its owner key.
fn my_shares_json(rec: &Recorded) -> Result<String, MasternodesError> {
    json(&BTreeMap::from([(
        rec.share_index.to_string(),
        rec.owner_index,
    )]))
}

/// The keychain owner indexes a `my_shares` column names (none when it
/// doesn't read: the engine alone writes it).
fn owner_indexes(my_shares: &str) -> Vec<u32> {
    serde_json::from_str::<BTreeMap<String, u32>>(my_shares)
        .map(|m| m.into_values().collect())
        .unwrap_or_default()
}

/// Core's rules on `session`'s terms, stand-ins in its empty rows (§5):
/// what `start_shared` and `join_shared` run before any key is spent.
pub(crate) fn check_placeholder(
    session: &Session,
    ctx: &Context,
    network: dashcore::Network,
) -> Result<(), MasternodesError> {
    let (tx, p) = placeholder(session)?;
    shared_rules::check_registration(&tx, &p, &ctx.listed, true, network, false).map_err(refused)
}

/// The fee floor at lock (§3.1 step 3, D26): the relay minimum of the
/// final registration with every input counted as P2PKH, against the fee
/// the coordinator's recorded contribution pays.
pub(crate) fn check_fee_floor(frozen: &Frozen, rec: &Recorded) -> Result<(), MasternodesError> {
    let tx = &frozen.tx;
    let lens: Vec<usize> = tx.output.iter().map(|o| o.script_pubkey.len()).collect();
    let needed = fee_for(
        FeeRate::new(MIN_FEE_PER_KB),
        tx_size(tx.input.len(), &lens, tx.payload.len()),
    )
    .ok_or(EngineError::Send(SendFailure::TxTooLarge))?;
    let paid = rec.debit().saturating_sub(rec.amount);
    if paid < needed {
        return Err(MasternodesError::SharedFundingMismatch {
            reason: "fee_low".into(),
            needed: Some(needed),
        });
    }
    Ok(())
}

/// This wallet's recorded contribution for share `index` of `row`.
pub(crate) fn recorded_of(
    index: usize,
    own: &OwnRow,
    row: &Share,
    selected: &Selected,
    change: Option<RecordedChange>,
    fee: Option<u64>,
) -> Recorded {
    Recorded {
        share_index: index,
        owner_index: own.owner_index,
        label: row.label.clone(),
        amount: u64::try_from(row.amount).unwrap_or(0),
        owner_address: own.owner_address.clone(),
        refund_address: own.refund_address.clone(),
        reward_address: own.reward_address.clone(),
        inputs: encode_outpoints(
            &selected
                .inputs
                .iter()
                .map(|(u, _)| u.outpoint)
                .collect::<Vec<_>>(),
        ),
        input_total: selected.total,
        change,
        fee,
    }
}

/// Puts this wallet's row and contribution into `session` (sorted by share,
/// as `absorbDraftReply` sorts) and rebuilds the funding template.
pub(crate) fn put_own_side(session: &mut Session, rec: &Recorded) -> Result<(), MasternodesError> {
    let row = &mut session.shares[rec.share_index];
    row.owner_address.clone_from(&rec.owner_address);
    row.refund_address.clone_from(&rec.refund_address);
    row.reward_address.clone_from(&rec.reward_address);
    session.contributions.retain(|c| c.label != rec.label);
    session.contributions.push(rec.contribution());
    let order = |label: &str| session.share_index_of(label).unwrap_or(usize::MAX);
    let mut sorted = session.contributions.clone();
    sorted.sort_by_key(|c| order(&c.label));
    session.contributions = sorted;
    dw_protx::shared::session::funding::rebuild_funding_tx(session)
        .map_err(|_| funding_refusal("template"))?;
    Ok(())
}

/// Every session of the wallet, parsed; one that doesn't parse is skipped.
pub(crate) fn loaded_all(rows: Vec<SharedSessionRow>, network: dashcore::Network) -> Vec<Loaded> {
    rows.into_iter()
        .filter_map(|r| match Loaded::of(r, network) {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::warn!(
                    code = e.code(),
                    "skipped a shared session that doesn't parse"
                );
                None
            }
        })
        .collect()
}
