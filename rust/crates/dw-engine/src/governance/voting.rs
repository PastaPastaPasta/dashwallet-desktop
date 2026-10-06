//! Voting with the wallets' DIP-3 voting keys (QT-131).
//!
//! A wallet votes for a masternode of the list whose voting key id is one
//! of the wallet's `ProviderVotingKeys` addresses. The vote names the
//! masternode's collateral outpoint, which the SPV list does not carry; it
//! comes from the provider transaction the wallet holds (platform-wallet
//! `wallet_masternodes_blocking`), or, for a key only one masternode uses,
//! from a synced vote that key signed. Masternodes with neither are not
//! offered.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, PubkeyHash};
use dw_governance::net::INV_GOVERNANCE_VOTE;
use dw_governance::object::{display_hex, outpoint_short, parse_display_hex};
use dw_governance::params::{EVONODE_VOTE_WEIGHT, VOTE_UPDATE_MIN_SECS};
use dw_governance::sync::relay;
use dw_governance::vote::{GovernanceVote, VoteOutcome, to_recoverable_compact};
use dw_vault::{GrantKind, VaultError};
use key_wallet::bip32::DerivationPath;
use key_wallet::managed_account::managed_account_type::ManagedAccountType;
use key_wallet::Signer;
use platform_wallet::masternode::MasternodeListSummary;

use super::{GovernanceFailure, VoteResult, VotingMasternode};
use crate::{EngineError, NetworkSession, WalletId};

/// How long a relay waits for peers to fetch the votes.
const RELAY_WAIT: Duration = Duration::from_secs(20);

/// A voting key of a wallet.
#[derive(Debug, Clone)]
pub(crate) struct VotingKey {
    pub wallet: WalletId,
    pub key_id: [u8; 20],
    pub path: DerivationPath,
    /// Index in the account's pool.
    pub index: u32,
}

/// A list masternode whose voting key a wallet holds.
#[derive(Debug, Clone)]
pub(crate) struct Controlled {
    pub wallet: WalletId,
    pub path: DerivationPath,
    pub key_id: [u8; 20],
    /// Wire order.
    pub pro_tx_hash: [u8; 32],
    pub weight: u32,
    pub is_valid: bool,
    pub collateral: Option<OutPoint>,
}

/// A proTxHash in display order (wire bytes reversed).
pub(crate) fn pro_tx_display(wire: &[u8; 32]) -> String {
    display_hex(wire)
}

impl NetworkSession {
    /// The voting keys of `wallets`.
    pub(crate) async fn wallet_voting_keys(&self, wallets: &[WalletId]) -> Vec<VotingKey> {
        let mut out = Vec::new();
        for id in wallets {
            let Ok(wallet) = self.wallet(id).await else {
                continue;
            };
            let state = wallet.state().await;
            for account in state.core_wallet.all_managed_accounts() {
                if !matches!(
                    account.managed_account_type(),
                    ManagedAccountType::ProviderVotingKeys { .. }
                ) {
                    continue;
                }
                for address in account.all_addresses() {
                    let Payload::PubkeyHash(hash) = address.payload() else {
                        continue;
                    };
                    if let Some(info) = account.get_address_info(&address) {
                        out.push(VotingKey {
                            wallet: *id,
                            key_id: hash.to_byte_array(),
                            path: info.path,
                            index: info.index,
                        });
                    }
                }
            }
        }
        out
    }

    /// The loaded wallets, or `only`.
    async fn governance_wallets(&self, only: Option<WalletId>) -> Vec<WalletId> {
        match only {
            Some(w) => vec![w],
            None => match self.manager() {
                Ok(m) => m.list_wallet_ids_blocking().into_iter().map(WalletId).collect(),
                Err(_) => Vec::new(),
            },
        }
    }

    /// The list masternodes whose voting keys `only` (or any wallet)
    /// holds, with collaterals where known. `None` when no wallet holds a
    /// voting key of the list (or the list is not synced).
    pub(crate) async fn controlled_masternodes(
        &self,
        only: Option<WalletId>,
        list: Option<&[MasternodeListSummary]>,
    ) -> Option<Vec<Controlled>> {
        let list = list?;
        let wallets = self.governance_wallets(only).await;
        let keys = self.wallet_voting_keys(&wallets).await;
        let by_key: HashMap<[u8; 20], &VotingKey> = keys.iter().map(|k| (k.key_id, k)).collect();
        let mut users: HashMap<[u8; 20], usize> = HashMap::new();
        for m in list {
            *users.entry(m.voting_key_id).or_default() += 1;
        }
        // Collaterals from the wallets' provider transactions.
        let mut collaterals: HashMap<[u8; 32], OutPoint> = HashMap::new();
        if let Ok(manager) = self.manager() {
            let ids = wallets.clone();
            let found = tokio::task::spawn_blocking(move || {
                let mut out = Vec::new();
                for id in ids {
                    if let Some(w) = manager.wallet_masternodes_blocking(&id.0) {
                        for r in w.records {
                            if let Some((txid, vout)) = r.collateral {
                                out.push((r.pro_tx_hash, txid, vout));
                            }
                        }
                    }
                }
                out
            })
            .await
            .unwrap_or_default();
            // The ProRegTxs in the wallets' history: platform-wallet keeps
            // provider records in memory only while the session that saw
            // them runs (a chainlocked record reloads as a bare txid), the
            // history store reloads every stored transaction.
            let mut found = found;
            for id in &wallets {
                for entry in self.hub.history.snapshot(id).into_values() {
                    if let Some(
                        dashcore::transaction::TransactionPayload::ProviderRegistrationPayloadType(p),
                    ) = &entry.tx.special_transaction_payload
                    {
                        found.push((
                            entry.tx.txid().to_byte_array(),
                            p.collateral_outpoint.txid.to_byte_array(),
                            p.collateral_outpoint.vout,
                        ));
                    }
                }
            }
            for (pro, txid, vout) in found {
                // A null collateral hash means the ProRegTx pays its own
                // collateral (`protx register_fund`): Core's outpoint is
                // (proTxHash, n).
                let txid = if txid == [0; 32] { pro } else { txid };
                collaterals.insert(
                    pro,
                    OutPoint::new(dashcore::Txid::from_byte_array(txid), vout),
                );
            }
        }
        // Keys only one masternode uses: a synced vote they signed names
        // its collateral.
        let unique_keys: HashSet<[u8; 20]> = list
            .iter()
            .filter(|m| by_key.contains_key(&m.voting_key_id) && users[&m.voting_key_id] == 1)
            .map(|m| m.voting_key_id)
            .collect();
        let mut from_votes: HashMap<[u8; 20], OutPoint> = HashMap::new();
        if !unique_keys.is_empty() {
            let store = self.governance.shared.store();
            for o in store.objects() {
                for v in store.votes_on(&o.object.hash()) {
                    if unique_keys.contains(&v.key_id) {
                        from_votes.entry(v.key_id).or_insert(v.outpoint);
                    }
                }
            }
        }
        tracing::debug!(
            wallets = wallets.len(),
            voting_keys = keys.len(),
            list = list.len(),
            in_list = list.iter().filter(|m| by_key.contains_key(&m.voting_key_id)).count(),
            wallet_collaterals = collaterals.len(),
            vote_collaterals = from_votes.len(),
            "governance: masternodes the wallets vote for"
        );
        let out: Vec<Controlled> = list
            .iter()
            .filter_map(|m| {
                let key = by_key.get(&m.voting_key_id)?;
                Some(Controlled {
                    wallet: key.wallet,
                    path: key.path.clone(),
                    key_id: m.voting_key_id,
                    pro_tx_hash: m.pro_tx_hash,
                    weight: if m.is_evonode { EVONODE_VOTE_WEIGHT } else { 1 },
                    is_valid: m.is_valid,
                    collateral: collaterals
                        .get(&m.pro_tx_hash)
                        .copied()
                        .or_else(|| from_votes.get(&m.voting_key_id).copied()),
                })
            })
            .collect();
        (!out.is_empty()).then_some(out)
    }

    /// Whether `hash` names a synced object or one of the wallets' own.
    async fn proposal_known(&self, hash: &str, key: &[u8; 32]) -> Result<bool, EngineError> {
        if self.governance.shared.store().has_object(key) {
            return Ok(true);
        }
        Ok(self.find_own_proposal(hash).await?.is_some())
    }

    /// The masternodes that can vote on `hash` (QT-131 table).
    pub async fn voting_masternodes(
        self: &Arc<Self>,
        hash: String,
        wallet: Option<WalletId>,
    ) -> Result<Vec<VotingMasternode>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let key = parse_display_hex(&hash)
                .ok_or_else(|| EngineError::InvalidArgument("hash must be 64 hex characters".into()))?;
            if !this.proposal_known(&hash, &key).await? {
                return Err(GovernanceFailure::ProposalNotFound(hash).into());
            }
            let list = this
                .governance_masternode_list()
                .await
                .ok_or(GovernanceFailure::NotSynced)?;
            let network = this.network.core_network();
            let controlled = this
                .controlled_masternodes(wallet, Some(&list))
                .await
                .unwrap_or_default();
            let store = this.governance.shared.store();
            Ok(controlled
                .into_iter()
                .filter_map(|m| {
                    let collateral = m.collateral?;
                    let current = store.current_vote(&key, &collateral);
                    Some(VotingMasternode {
                        pro_tx_hash: pro_tx_display(&m.pro_tx_hash),
                        collateral: outpoint_short(&collateral),
                        voting_address: Address::new(
                            network,
                            Payload::PubkeyHash(PubkeyHash::from_byte_array(m.key_id)),
                        )
                        .to_string(),
                        weight: m.weight,
                        current_vote: current.map(|v| v.outcome),
                        vote_time: current.map(|v| v.time.max(0) as u64),
                        next_vote_at: current
                            .map(|v| v.time.max(0) as u64 + VOTE_UPDATE_MIN_SECS),
                        label: None,
                    })
                })
                .collect())
        })
        .await
    }

    /// Signs a funding vote per masternode with its voting key and relays
    /// them (QT-131). `grant_id`: a `Governance` grant of the wallet that
    /// holds the keys; masternodes of other wallets get
    /// `governance.grant_invalid`.
    pub async fn cast_votes(
        self: &Arc<Self>,
        hash: String,
        outcome: VoteOutcome,
        pro_tx_hashes: Vec<String>,
        grant_id: String,
    ) -> Result<Vec<VoteResult>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.cast_votes_inner(hash, outcome, pro_tx_hashes, grant_id).await
        })
        .await
    }

    async fn cast_votes_inner(
        self: &Arc<Self>,
        hash: String,
        outcome: VoteOutcome,
        pro_tx_hashes: Vec<String>,
        grant_id: String,
    ) -> Result<Vec<VoteResult>, EngineError> {
        let key = parse_display_hex(&hash)
            .ok_or_else(|| EngineError::InvalidArgument("hash must be 64 hex characters".into()))?;
        if !self.proposal_known(&hash, &key).await? {
            return Err(GovernanceFailure::ProposalNotFound(hash).into());
        }
        let list = self
            .governance_masternode_list()
            .await
            .ok_or(GovernanceFailure::NotSynced)?;
        let controlled = self
            .controlled_masternodes(None, Some(&list))
            .await
            .unwrap_or_default();
        let by_hash: HashMap<String, &Controlled> = controlled
            .iter()
            .filter(|m| m.collateral.is_some())
            .map(|m| (pro_tx_display(&m.pro_tx_hash), m))
            .collect();
        let chosen: Vec<Option<&Controlled>> =
            pro_tx_hashes.iter().map(|h| by_hash.get(h).copied()).collect();
        if chosen.iter().all(Option::is_none) {
            return Err(GovernanceFailure::NoVotingKeys.into());
        }

        // The grant's wallet: the one it validates for.
        let wallets: Vec<WalletId> = {
            let mut w: Vec<WalletId> = chosen.iter().flatten().map(|m| m.wallet).collect();
            w.sort();
            w.dedup();
            w
        };
        let mut grant_wallet = None;
        let mut last_err = VaultError::GrantInvalid;
        for w in &wallets {
            match self.vault.check_grant(&grant_id, GrantKind::Governance, Some(&w.0)) {
                Ok(()) => {
                    grant_wallet = Some(*w);
                    break;
                }
                Err(e) => last_err = e,
            }
        }
        let Some(grant_wallet) = grant_wallet else {
            return Err(vault_error(last_err));
        };
        if !self.vault.has_wallet_secret(&grant_wallet.0) {
            return Err(GovernanceFailure::WatchOnly.into());
        }
        let vault = self.vault.clone();
        let gid = grant_id.clone();
        let signer = tokio::task::spawn_blocking(move || {
            let token = vault.redeem_grant(&gid, GrantKind::Governance, Some(&grant_wallet.0))?;
            vault.signer(&grant_wallet.0, &token)
        })
        .await?
        .map_err(vault_error)?;

        let network = self.network.core_network();
        let now = crate::events::unix_now() as i64;
        let mut results: Vec<VoteResult> = Vec::with_capacity(pro_tx_hashes.len());
        let mut signed: Vec<(usize, GovernanceVote, [u8; 20])> = Vec::new();
        for (i, (h, m)) in pro_tx_hashes.iter().zip(&chosen).enumerate() {
            let fail = |f: GovernanceFailure, detail: Option<String>| VoteResult {
                pro_tx_hash: h.clone(),
                failure: Some(f),
                detail,
            };
            let Some(m) = m else {
                results.push(fail(GovernanceFailure::NoVotingKeys, None));
                continue;
            };
            if m.wallet != grant_wallet {
                results.push(fail(
                    GovernanceFailure::GrantInvalid,
                    Some("the grant is for another wallet".into()),
                ));
                continue;
            }
            let collateral = m.collateral.expect("filtered above");
            let previous = self.governance.shared.store().current_vote(&key, &collateral).cloned();
            if let Some(prev) = previous {
                let next = prev.time + VOTE_UPDATE_MIN_SECS as i64;
                if next > now {
                    results.push(fail(
                        GovernanceFailure::VoteTooOften {
                            retry_after_secs: (next - now) as u64,
                        },
                        None,
                    ));
                    continue;
                }
            }
            let mut vote = GovernanceVote::funding(collateral, key, outcome, now);
            let digest = vote.signing_digest(network);
            let sig = match signer.sign_ecdsa(&m.path, digest).await {
                Ok((sig, pk)) => to_recoverable_compact(&sig.serialize_compact(), &pk, &digest),
                Err(e) => {
                    results.push(fail(GovernanceFailure::VaultLocked, Some(e.to_string())));
                    continue;
                }
            };
            let Some(sig) = sig else {
                results.push(fail(
                    GovernanceFailure::BroadcastRejected("signature recovery failed".into()),
                    None,
                ));
                continue;
            };
            vote.signature = sig.to_vec();
            results.push(VoteResult {
                pro_tx_hash: h.clone(),
                failure: None,
                detail: None,
            });
            signed.push((i, vote, m.key_id));
        }
        drop(signer);

        if signed.is_empty() {
            return Ok(results);
        }
        let candidates = self.governance_peer_candidates().await;
        let tip = self.governance_tip().map_or(0, |t| t.height);
        let cfg = self.governance_sync_config(tip);
        let items = signed
            .iter()
            .map(|(_, v, _)| (INV_GOVERNANCE_VOTE, v.hash(), v.encode()))
            .collect();
        let report = relay(&cfg, &candidates, items, RELAY_WAIT).await;
        if report.announced_to == 0 {
            return Err(GovernanceFailure::NoPeers.into());
        }
        for (i, vote, key_id) in &signed {
            if report.fetched.contains(&vote.hash()) {
                self.governance.shared.store().record_own_vote(vote, *key_id);
                results[*i].detail = Some(format!(
                    "relayed to {} peer(s), vote {}",
                    report.announced_to,
                    display_hex(&vote.hash())
                ));
            } else {
                results[*i].failure = Some(GovernanceFailure::BroadcastRejected(
                    "no peer requested the vote".into(),
                ));
            }
        }
        self.governance.changed();
        Ok(results)
    }
}

impl NetworkSession {
    /// The wallet's DIP-3 voting key addresses and derivation paths, by pool
    /// index (headless hosts register masternodes with them).
    pub async fn governance_voting_addresses(
        self: &Arc<Self>,
        wallet: WalletId,
    ) -> Result<Vec<(String, String)>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet).await?;
            let network = this.network.core_network();
            let mut keys = this.wallet_voting_keys(&[wallet]).await;
            keys.sort_by_key(|k| k.index);
            Ok(keys
                .into_iter()
                .map(|k| {
                    let address = Address::new(
                        network,
                        Payload::PubkeyHash(PubkeyHash::from_byte_array(k.key_id)),
                    );
                    (address.to_string(), k.path.to_string())
                })
                .collect())
        })
        .await
    }
}

/// Vault refusals as governance failures.
pub(super) fn vault_error(e: VaultError) -> EngineError {
    match e {
        VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly => {
            GovernanceFailure::VaultLocked.into()
        }
        VaultError::GrantInvalid | VaultError::GrantPurposeMismatch => {
            GovernanceFailure::GrantInvalid.into()
        }
        other => EngineError::Vault(other),
    }
}
