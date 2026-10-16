//! The read calls (mn-engine §3): a wallet's overview and one masternode
//! with status and roles (MG-03 §2, §3), list search (§4), payouts (§5)
//! and an evonode's credits (§6).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::IdentityBalance;
use dashcore::hashes::{Hash, hash160};
use dashcore::{Address, Network, OutPoint, PubkeyHash, ScriptBuf};
use dpp::prelude::Identifier;
use platform_wallet::masternode::{
    MasternodeListQuery, MasternodeListSummary, TrackedMasternode, find_in_summaries,
};
use platform_wallet::wallet::provider_key_at_index::ProviderKeyKind;

use super::aggregate::{OwnedMasternode, collateral_amount};
use super::follow::{ListView, Notes, load_notes, read_list, wallet_masternodes};
use super::records::{
    AttachedKeyRole, CollateralCoin, CollateralState, EvonodeCredits, ListFacts, ListedMasternode,
    MasternodeDetail, MasternodeListState, MasternodeOverview, MasternodePayout, MasternodeRoles,
    MasternodeRow, MasternodeStatus, MasternodeType, RegistrationFacts,
};
use super::{Masternodes, MasternodesError, display_hex, outpoint_string, parse_pro_tx_hash};
use crate::coins::{CoinSnapshot, read_coins};
use crate::events::unix_now;
use crate::{EngineError, NetworkSession, TxRecord, TxStatusKind, TxType, WalletId};

/// The overview's payout total covers the last 30 days (MG-00 D10).
const PAYOUT_WINDOW_SECS: u64 = 30 * 24 * 60 * 60;
/// Derived owner and voting keys a wallet is checked against: indexes
/// 0–19, as MG-02's voting keys.
const DERIVED_KEYS: u32 = 20;

/// Everything one wallet's reads need, read under one operation guard.
struct WalletView {
    network: Network,
    list: ListView,
    /// The wallet's own masternodes, in the library's registration order.
    owned: Vec<OwnedMasternode>,
    /// The wallet's derived operator keys (a self-operator).
    operator_keys: HashSet<[u8; 48]>,
    /// Tracked masternodes, in the order they were tracked.
    tracked: Vec<TrackedMasternode>,
    owner_ids: HashSet<[u8; 20]>,
    voting_ids: HashSet<[u8; 20]>,
    /// Attached roles per tracked masternode; `None` while the vault is
    /// locked.
    attached: Option<HashMap<[u8; 32], Vec<AttachedKeyRole>>>,
    own_scripts: HashSet<ScriptBuf>,
    coins: CoinSnapshot,
    unspent: HashSet<OutPoint>,
    /// User- and dust-locked outpoints (`lock_key` form).
    locked: HashSet<(String, u32)>,
    /// Coinbase outputs to the wallet that count, newest first.
    generated: Vec<TxRecord>,
    notes: Notes,
    now: u64,
}

impl WalletView {
    async fn read(session: &NetworkSession, wallet: WalletId) -> Result<Self, MasternodesError> {
        let manager = session.manager()?;
        let pw = session.wallet(&wallet).await?;
        let list = read_list(session, &manager).await;
        let network = session.network.core_network();

        let (m, w) = (std::sync::Arc::clone(&manager), std::sync::Arc::clone(&pw));
        let hub = std::sync::Arc::clone(&session.hub);
        let (owned, operator_keys, tracked, owner_ids, voting_ids) =
            tokio::task::spawn_blocking(move || {
                let (owned, operator_index) =
                    wallet_masternodes(&m, &hub.history, &wallet).unwrap_or_default();
                let service = m.tracked_masternodes_service();
                let mut tracked: Vec<TrackedMasternode> = service
                    .hashes()
                    .iter()
                    .filter_map(|h| service.get(h))
                    .collect();
                tracked.sort_by_key(|t| t.added_at);
                (
                    owned,
                    operator_index.into_keys().collect::<HashSet<_>>(),
                    tracked,
                    derived_key_ids(&w, ProviderKeyKind::Owner),
                    derived_key_ids(&w, ProviderKeyKind::Voting),
                )
            })
            .await
            .map_err(EngineError::from)?;

        let hashes: Vec<[u8; 32]> = tracked.iter().map(|t| t.pro_tx_hash).collect();
        let attached = attached_roles(session, hashes).await?;

        let coins = {
            let state = pw.state().await;
            read_coins(&state.core_wallet)
        };
        let unspent = coins.coins.iter().map(|c| c.utxo.outpoint).collect();
        let id = wallet.to_string();
        let locked = session
            .appdb_op(move |db| {
                Ok(db
                    .locks(&id)?
                    .into_iter()
                    .filter(|row| row.is_locked())
                    .map(|row| (row.txid, row.vout))
                    .collect())
            })
            .await?;
        let own_scripts = session
            .with_view(&manager, wallet, |view| {
                view.owned.keys().cloned().collect()
            })
            .await?;
        let notes = session.appdb_op(load_notes).await?;
        let mut generated: Vec<TxRecord> = session
            .wallet_records(wallet)
            .await?
            .into_iter()
            .filter(counts_as_payout)
            .collect();
        sort_newest_first(&mut generated);
        Ok(Self {
            network,
            list,
            owned,
            operator_keys,
            tracked,
            owner_ids,
            voting_ids,
            attached,
            own_scripts,
            coins,
            unspent,
            locked,
            generated,
            notes,
            now: unix_now(),
        })
    }

    fn owned(&self, hash: &[u8; 32]) -> Option<&OwnedMasternode> {
        self.owned.iter().find(|o| &o.record.pro_tx_hash == hash)
    }

    fn tracked(&self, hash: &[u8; 32]) -> Option<&TrackedMasternode> {
        self.tracked.iter().find(|t| &t.pro_tx_hash == hash)
    }

    /// Whether the wallet knows `hash` at all: its own, tracked, or listed.
    fn knows(&self, hash: &[u8; 32]) -> bool {
        self.owned(hash).is_some()
            || self.tracked(hash).is_some()
            || self.list.entry(hash).is_some()
    }

    fn payout_script(&self, hash: &[u8; 32]) -> Option<ScriptBuf> {
        self.owned(hash)
            .and_then(|o| o.record.payout_script.clone())
            .or_else(|| {
                self.tracked(hash)?
                    .snapshot
                    .registration
                    .as_ref()
                    .map(|r| r.payout_script.clone())
            })
            .map(ScriptBuf::from)
    }

    fn payout_address(&self, hash: &[u8; 32]) -> Option<String> {
        let script = self.payout_script(hash)?;
        Address::from_script(&script, self.network)
            .ok()
            .map(|a| a.to_string())
    }

    /// Payout records of `hash`, newest first (MG-03 §5).
    fn payouts(&self, hash: &[u8; 32]) -> Vec<MasternodePayout> {
        let Some(address) = self.payout_address(hash) else {
            return Vec::new();
        };
        self.generated
            .iter()
            .filter(|r| r.address.as_deref() == Some(address.as_str()))
            .map(payout_of)
            .collect()
    }

    fn status(&self, hash: &[u8; 32]) -> MasternodeStatus {
        let owned = self.owned(hash);
        if let Some(entry) = self.list.entry(hash) {
            return if entry.is_valid {
                MasternodeStatus::Enabled
            } else {
                MasternodeStatus::PoseBanned
            };
        }
        if owned.is_some_and(OwnedMasternode::unconfirmed) {
            return MasternodeStatus::Unconfirmed;
        }
        if !self.list.synced {
            return MasternodeStatus::Unknown;
        }
        let listed_before = self.notes.listed_seen.contains(&display_hex(hash))
            || self.tracked(hash).is_some_and(|t| t.snapshot.ever_listed);
        let registered_below = owned
            .and_then(OwnedMasternode::registered_height)
            .zip(self.list.height)
            .is_some_and(|(registered, list)| registered <= list);
        if listed_before || registered_below {
            MasternodeStatus::Removed
        } else {
            MasternodeStatus::Unknown
        }
    }

    fn roles(&self, hash: &[u8; 32]) -> MasternodeRoles {
        let owned = self.owned(hash);
        let entry = self.list.entry(hash);
        let attached = self
            .attached
            .as_ref()
            .map(|a| a.get(hash).cloned().unwrap_or_default());
        let has = |role| attached.as_ref().is_some_and(|a| a.contains(&role));
        let voting_id = entry
            .map(|e| e.voting_key_id)
            .or_else(|| owned.and_then(|o| o.record.voting_key_hash));
        let operator_key = entry
            .map(|e| e.operator_public_key)
            .or_else(|| owned.and_then(|o| o.record.operator_public_key));
        MasternodeRoles {
            collateral: owned
                .and_then(|o| o.collateral)
                .is_some_and(|c| self.unspent.contains(&c)),
            owner: owned
                .and_then(|o| o.record.owner_key_hash)
                .is_some_and(|id| self.owner_ids.contains(&id))
                || has(AttachedKeyRole::Owner),
            voting: voting_id.is_some_and(|id| self.voting_ids.contains(&id))
                || has(AttachedKeyRole::Voting),
            payout: self
                .payout_script(hash)
                .is_some_and(|s| self.own_scripts.contains(&s)),
            operator: operator_key.is_some_and(|k| self.operator_keys.contains(&k)),
            attached,
        }
    }

    fn row(&self, hash: &[u8; 32]) -> MasternodeRow {
        let key = display_hex(hash);
        let owned = self.owned(hash);
        let tracked = self.tracked(hash);
        let entry = self.list.entry(hash);
        let status = self.status(hash);
        let kind = entry
            .map(|e| e.is_evonode)
            .or_else(|| {
                owned
                    .filter(|o| o.record.has_registration)
                    .map(|o| o.record.is_evonode)
            })
            .or_else(|| {
                tracked
                    .and_then(|t| t.snapshot.list.as_ref())
                    .map(|l| l.is_evonode)
            })
            .map(kind_of);
        let payouts = self.payouts(hash);
        let since = self.now.saturating_sub(PAYOUT_WINDOW_SECS);
        MasternodeRow {
            kind,
            type_index: owned.map_or(0, |o| o.record.type_index),
            label: tracked
                .and_then(|t| t.label.clone())
                .or_else(|| self.notes.labels.get(&key).cloned()),
            status,
            banned_seen_at: (status == MasternodeStatus::PoseBanned)
                .then(|| self.notes.banned_seen.get(&key).copied())
                .flatten(),
            service: entry
                .and_then(|e| e.service_address.map(|s| s.to_string()))
                .or_else(|| owned.and_then(|o| o.record.service_address.clone())),
            mine: owned.is_some(),
            tracked: tracked.is_some(),
            roles: self.roles(hash),
            collateral: collateral_state(owned),
            payout_address: self.payout_address(hash),
            payouts_30d: payouts
                .iter()
                .filter(|p| p.timestamp.is_some_and(|t| t >= since))
                .map(|p| p.amount)
                .sum(),
            last_payout: payouts.into_iter().next(),
            pro_tx_hash: key,
        }
    }

    fn overview(&self) -> MasternodeOverview {
        let mut hashes: Vec<[u8; 32]> = self.owned.iter().map(|o| o.record.pro_tx_hash).collect();
        for t in &self.tracked {
            if !hashes.contains(&t.pro_tx_hash) {
                hashes.push(t.pro_tx_hash);
            }
        }
        let masternodes: Vec<MasternodeRow> = hashes.iter().map(|h| self.row(h)).collect();
        // Two masternodes sharing a payout address share its records (D9);
        // the wallet total counts each record once.
        let since = self.now.saturating_sub(PAYOUT_WINDOW_SECS);
        let addresses: HashSet<String> = hashes
            .iter()
            .filter_map(|h| self.payout_address(h))
            .collect();
        let payouts_30d = self
            .generated
            .iter()
            .filter(|r| r.address.as_ref().is_some_and(|a| addresses.contains(a)))
            .filter(|r| r.timestamp.is_some_and(|t| t >= since))
            .map(|r| r.amount.unsigned_abs())
            .sum();
        MasternodeOverview {
            list: MasternodeListState {
                synced: self.list.synced,
                height: self.list.height,
            },
            collateral_coins: self.collateral_coins(&masternodes),
            masternodes,
            payouts_30d,
        }
    }

    /// Unspent outputs of exactly a collateral amount that are no listed
    /// row's collateral (MG-00 D1), largest first.
    fn collateral_coins(&self, rows: &[MasternodeRow]) -> Vec<CollateralCoin> {
        let known: HashSet<&str> = rows
            .iter()
            .filter_map(|m| match &m.collateral {
                CollateralState::Known { outpoint } => Some(outpoint.as_str()),
                CollateralState::Unresolved => None,
            })
            .collect();
        let tip = self.coins.height;
        let mut coins: Vec<CollateralCoin> = self
            .coins
            .coins
            .iter()
            .filter_map(|c| {
                let u = &c.utxo;
                let kind = [MasternodeType::Regular, MasternodeType::Evonode]
                    .into_iter()
                    .find(|k| u.value() == collateral_amount(*k == MasternodeType::Evonode))?;
                let outpoint = outpoint_string(&u.outpoint);
                if known.contains(outpoint.as_str()) {
                    return None;
                }
                Some(CollateralCoin {
                    user_locked: self
                        .locked
                        .contains(&(u.outpoint.txid.to_string(), u.outpoint.vout)),
                    outpoint,
                    address: u.address.to_string(),
                    amount: u.value(),
                    kind,
                    confirmations: if u.is_confirmed && tip >= u.height {
                        tip - u.height + 1
                    } else {
                        0
                    },
                })
            })
            .collect();
        coins.sort_by(|a, b| {
            b.amount
                .cmp(&a.amount)
                .then_with(|| a.outpoint.cmp(&b.outpoint))
        });
        coins
    }

    fn detail(&self, hash: &[u8; 32]) -> MasternodeDetail {
        let owned = self.owned(hash);
        MasternodeDetail {
            row: self.row(hash),
            list: self.list.entry(hash).map(|e| list_facts(e, self.network)),
            registration: owned.map(|o| self.registration_facts(o)),
            payout_checked_height: None,
            tracked_at: self.tracked(hash).map(|t| t.added_at),
        }
    }

    fn registration_facts(&self, o: &OwnedMasternode) -> RegistrationFacts {
        let r = &o.record;
        RegistrationFacts {
            registered_height: o.registered_height(),
            collateral: collateral_state(Some(o)),
            owner_address: r.owner_key_hash.map(|id| p2pkh(id, self.network)),
            voting_address: r.voting_key_hash.map(|id| p2pkh(id, self.network)),
            payout_address: self.payout_address(&r.pro_tx_hash),
            payout_script: r.payout_script.as_ref().map(hex::encode),
            operator_reward: o.operator_reward,
            revoked: r.revoked,
            revocation_reason: r.revoked.then_some(r.revocation_reason),
            transactions: r.tx_count,
        }
    }
}

/// hash160s of a wallet's derived keys of `kind` at indexes 0–19. A wallet
/// that can't derive them (no provider account) has none. Blocking.
fn derived_key_ids(
    wallet: &platform_wallet::PlatformWallet,
    kind: ProviderKeyKind,
) -> HashSet<[u8; 20]> {
    (0..DERIVED_KEYS)
        .map_while(|i| {
            wallet
                .derive_provider_key_at_index(kind, i, None, false)
                .ok()
        })
        .map(|k| hash160::Hash::hash(&k.public_key_bytes).to_byte_array())
        .collect()
}

/// The attached roles of each tracked masternode; `None` while the vault
/// is locked (the record ids need the data key, MG-01 D7).
async fn attached_roles(
    session: &NetworkSession,
    hashes: Vec<[u8; 32]>,
) -> Result<Option<HashMap<[u8; 32], Vec<AttachedKeyRole>>>, MasternodesError> {
    if hashes.is_empty() {
        return Ok(Some(HashMap::new()));
    }
    let vault = session.vault.clone();
    let roles = tokio::task::spawn_blocking(move || {
        hashes
            .into_iter()
            .map(|h| {
                let mut roles: Vec<AttachedKeyRole> = vault
                    .masternode_key_roles(&h)?
                    .iter()
                    .filter_map(|r| AttachedKeyRole::from_vault_name(r))
                    .collect();
                roles.sort();
                Ok((h, roles))
            })
            .collect::<Result<HashMap<_, _>, dw_vault::VaultError>>()
    })
    .await
    .map_err(EngineError::from)?;
    Ok(roles.ok())
}

fn kind_of(evonode: bool) -> MasternodeType {
    if evonode {
        MasternodeType::Evonode
    } else {
        MasternodeType::Regular
    }
}

fn collateral_state(owned: Option<&OwnedMasternode>) -> CollateralState {
    match owned.and_then(|o| o.collateral) {
        Some(o) => CollateralState::Known {
            outpoint: outpoint_string(&o),
        },
        None => CollateralState::Unresolved,
    }
}

fn p2pkh(id: [u8; 20], network: Network) -> String {
    Address::new(
        network,
        dashcore::address::Payload::PubkeyHash(PubkeyHash::from_byte_array(id)),
    )
    .to_string()
}

pub(super) fn list_facts(e: &MasternodeListSummary, network: Network) -> ListFacts {
    ListFacts {
        kind: kind_of(e.is_evonode),
        valid: e.is_valid,
        service: e.service_address.map(|s| s.to_string()),
        operator_public_key: hex::encode(e.operator_public_key),
        voting_address: p2pkh(e.voting_key_id, network),
        platform_node_id: e.platform_node_id.map(hex::encode),
        platform_http_port: e.platform_http_port,
    }
}

/// A coinbase output to the wallet whose block wasn't orphaned.
fn counts_as_payout(r: &TxRecord) -> bool {
    r.tx_type == TxType::Generated
        && r.amount > 0
        && !matches!(
            r.status.kind,
            TxStatusKind::NotAccepted | TxStatusKind::Conflicted | TxStatusKind::Abandoned
        )
}

fn sort_newest_first(records: &mut [TxRecord]) {
    records.sort_by(|a, b| {
        (b.block_height, b.timestamp, &b.txid, b.record_index).cmp(&(
            a.block_height,
            a.timestamp,
            &a.txid,
            a.record_index,
        ))
    });
}

fn payout_of(r: &TxRecord) -> MasternodePayout {
    MasternodePayout {
        txid: r.txid.clone(),
        record_index: r.record_index,
        amount: r.amount.unsigned_abs(),
        address: r.address.clone().unwrap_or_default(),
        height: r.block_height,
        timestamp: r.timestamp,
        matured: r.status.kind != TxStatusKind::Immature,
    }
}

/// `find`'s query: a ProTx hash, or a service address with or without its
/// port.
fn parse_query(query: &str) -> Result<MasternodeListQuery, EngineError> {
    let q = query.trim();
    if q.len() == 64 {
        return parse_pro_tx_hash(q).map(MasternodeListQuery::ProTxHash);
    }
    if let Ok(addr) = q.parse::<SocketAddr>() {
        return Ok(MasternodeListQuery::ServiceAddress {
            ip: addr.ip(),
            port: Some(addr.port()),
        });
    }
    if let Ok(ip) = q.parse::<IpAddr>() {
        return Ok(MasternodeListQuery::ServiceAddress { ip, port: None });
    }
    Err(EngineError::InvalidArgument(format!(
        "not a ProTx hash or a service address: {query:?}"
    )))
}

impl Masternodes {
    /// Runs a read on the engine runtime under one operation guard.
    async fn read_wallet<T: Send + 'static>(
        &self,
        wallet: WalletId,
        f: impl FnOnce(&WalletView) -> Result<T, MasternodesError> + Send + 'static,
    ) -> Result<T, MasternodesError> {
        let session = std::sync::Arc::clone(&self.session);
        self.session
            .on_runtime(async move {
                let _op = session.enter().await?;
                let view = WalletView::read(&session, wallet).await;
                Ok(view.and_then(|v| f(&v)))
            })
            .await?
    }

    /// Whether the SPV list is synced, and its height (D3). Memory only;
    /// `list`'s state can still say `synced: false` while the summaries
    /// are being read.
    pub fn list_state(&self) -> Result<MasternodeListState, MasternodesError> {
        self.session.live()?;
        let height = self.session.hub.tracker().masternode_list_height();
        Ok(MasternodeListState {
            synced: height.is_some(),
            height,
        })
    }

    /// The wallet's My masternodes page (UX-SPEC §4.25.2): its own
    /// masternodes, then the tracked ones, with status, roles and payouts,
    /// and the collateral-shaped coins.
    pub async fn list(&self, wallet: WalletId) -> Result<MasternodeOverview, MasternodesError> {
        self.read_wallet(wallet, |v| Ok(v.overview())).await
    }

    /// One masternode in full: a wallet's own, a tracked one, or any
    /// masternode of the synced list.
    pub async fn masternode(
        &self,
        wallet: WalletId,
        pro_tx_hash: String,
    ) -> Result<MasternodeDetail, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        self.read_wallet(wallet, move |v| {
            if !v.knows(&hash) {
                return Err(not_found(&pro_tx_hash, &v.list));
            }
            Ok(v.detail(&hash))
        })
        .await
    }

    /// Payouts of a masternode to its payout address, newest first; with
    /// `since` (Unix seconds), only those at or after it.
    pub async fn payouts(
        &self,
        wallet: WalletId,
        pro_tx_hash: String,
        since: Option<u64>,
    ) -> Result<Vec<MasternodePayout>, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        self.read_wallet(wallet, move |v| {
            if !v.knows(&hash) {
                return Err(not_found(&pro_tx_hash, &v.list));
            }
            let mut payouts = v.payouts(&hash);
            if let Some(since) = since {
                payouts.retain(|p| p.timestamp.is_some_and(|t| t >= since));
            }
            Ok(payouts)
        })
        .await
    }

    /// Masternodes of the synced list matching a ProTx hash (64 hex) or a
    /// service address (`ip:port`, or `ip` for any port), in list order.
    pub async fn find(&self, query: String) -> Result<Vec<ListedMasternode>, MasternodesError> {
        let parsed = parse_query(&query)?;
        let session = std::sync::Arc::clone(&self.session);
        self.session
            .on_runtime(async move {
                let _op = session.enter().await?;
                let manager = session.manager()?;
                let list = read_list(&session, &manager).await;
                if !list.synced {
                    return Ok(Err(MasternodesError::ListUnavailable));
                }
                let tracked = manager.tracked_masternode_hashes();
                let network = session.network.core_network();
                Ok(Ok(find_in_summaries(&list.entries, &parsed)
                    .into_iter()
                    .map(|e| ListedMasternode {
                        pro_tx_hash: display_hex(&e.pro_tx_hash),
                        facts: list_facts(e, network),
                        tracked: tracked.contains(&e.pro_tx_hash),
                    })
                    .collect()))
            })
            .await?
    }

    /// An evonode's claimable credits: its owner identity's balance, read
    /// with a Platform proof (MG-03 §6).
    pub async fn evonode_credits(
        &self,
        pro_tx_hash: String,
    ) -> Result<EvonodeCredits, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let session = std::sync::Arc::clone(&self.session);
        self.session
            .on_runtime(async move {
                let sdk = {
                    let _op = session.enter().await?;
                    let manager = session.manager()?;
                    let list = read_list(&session, &manager).await;
                    let refusal = match list.entry(&hash) {
                        _ if !list.synced => Some(MasternodesError::ListUnavailable),
                        None => Some(not_found(&pro_tx_hash, &list)),
                        Some(e) if !e.is_evonode => Some(MasternodesError::NotEvonode {
                            pro_tx_hash: pro_tx_hash.clone(),
                        }),
                        Some(_) if !session.platform.enabled() => {
                            Some(MasternodesError::PlatformUnavailable {
                                detail: "this session runs without Platform".into(),
                            })
                        }
                        Some(_) => None,
                    };
                    if let Some(refusal) = refusal {
                        return Ok(Err(refusal));
                    }
                    manager.sdk_arc()
                };
                // The fetch runs outside the guard: a slow DAPI call must
                // not hold a session close back.
                let mut id = hash;
                id.reverse();
                let balance = IdentityBalance::fetch(&sdk, Identifier::from(id)).await;
                Ok(match balance {
                    Ok(Some(credits)) => Ok(EvonodeCredits {
                        pro_tx_hash,
                        credits,
                        duffs: credits / dpp::balances::credits::CREDITS_PER_DUFF,
                        fetched_at: unix_now(),
                    }),
                    Ok(None) => Err(MasternodesError::NoIdentity { pro_tx_hash }),
                    Err(e) => Err(MasternodesError::PlatformUnavailable {
                        detail: e.to_string(),
                    }),
                })
            })
            .await?
    }

    /// The collateral outpoint of any listed masternode, checked against a
    /// header (MG-00 D43). Needs a header-by-hash and block fetch dash-spv
    /// doesn't expose at the pin (MG-03 D11).
    pub async fn resolve_collateral(
        &self,
        pro_tx_hash: String,
    ) -> Result<String, MasternodesError> {
        parse_pro_tx_hash(&pro_tx_hash)?;
        Err(MasternodesError::Unsupported {
            feature: "collateral_lookup".into(),
        })
    }
}

/// `masternodes.not_found`, or `list_unavailable` when the list that
/// wasn't searched might have held it.
fn not_found(pro_tx_hash: &str, list: &ListView) -> MasternodesError {
    if list.synced {
        MasternodesError::NotFound {
            query: pro_tx_hash.into(),
        }
    } else {
        MasternodesError::ListUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_parse_hashes_and_service_addresses() {
        let hash = "ab".repeat(32);
        assert!(matches!(
            parse_query(&hash),
            Ok(MasternodeListQuery::ProTxHash(_))
        ));
        assert_eq!(
            parse_query("10.0.0.1:9999").unwrap(),
            MasternodeListQuery::ServiceAddress {
                ip: "10.0.0.1".parse().unwrap(),
                port: Some(9999)
            }
        );
        assert_eq!(
            parse_query(" 10.0.0.1 ").unwrap(),
            MasternodeListQuery::ServiceAddress {
                ip: "10.0.0.1".parse().unwrap(),
                port: None
            }
        );
        for bad in ["", "node.example", &"z".repeat(64), "XbadAddress"] {
            assert_eq!(parse_query(bad).unwrap_err().code(), "invalid_argument");
        }
    }
}
