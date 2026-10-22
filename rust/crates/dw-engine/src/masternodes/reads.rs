//! The read calls (mn-engine §3): a wallet's overview and one masternode
//! with status and roles (MG-03 §2, §3), list search (§4), payouts (§5)
//! and an evonode's credits (§6).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::IdentityBalance;
use dashcore::hashes::{Hash, hash160};
use dashcore::{Address, Network, OutPoint, PubkeyHash, ScriptBuf};
use dpp::prelude::Identifier;
use platform_wallet::masternode::{
    MasternodeListQuery, MasternodeListSummary, TrackedMasternode, find_in_summaries,
};
use platform_wallet::wallet::provider_key_at_index::ProviderKeyKind;

use super::aggregate::{OwnedMasternode, collateral_kind};
use super::follow::{ListView, Notes, all_owned, load_notes, read_list, wallet_masternodes};
use super::records::{
    AttachedKey, AttachedKeyRole, CollateralCoin, CollateralState, EvonodeCredits, KeyCheck,
    ListFacts, ListedMasternode, MasternodeDetail, MasternodeListState, MasternodeOverview,
    MasternodePayout, MasternodeRoles, MasternodeRow, MasternodeStatus, MasternodeType,
    RegistrationFacts,
};
use super::tracking::{KeyReferences, reconcile_key_ids};
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
    /// Any loaded wallet's record of a tracked masternode, which its
    /// attached keys are checked against (D24).
    tracked_owned: HashMap<[u8; 32], OwnedMasternode>,
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

        let (m, w, hub) = (
            Arc::clone(&manager),
            Arc::clone(&pw),
            Arc::clone(&session.hub),
        );
        let (owned, operator_keys, tracked, tracked_owned, owner_ids, voting_ids) =
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
                let tracked_owned = if tracked.is_empty() {
                    HashMap::new()
                } else {
                    all_owned(&m, &hub.history)
                        .into_iter()
                        .filter(|o| {
                            tracked
                                .iter()
                                .any(|t| t.pro_tx_hash == o.record.pro_tx_hash)
                        })
                        .map(|o| (o.record.pro_tx_hash, o))
                        .collect()
                };
                (
                    owned,
                    operator_index.into_keys().collect::<HashSet<_>>(),
                    tracked,
                    tracked_owned,
                    derived_key_ids(&w, ProviderKeyKind::Owner),
                    derived_key_ids(&w, ProviderKeyKind::Voting),
                )
            })
            .await?;

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
        let notes = match &attached {
            Some(attached) => reconcile_key_ids(session, attached, notes).await?,
            None => notes,
        };
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
            tracked_owned,
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

    /// The start of the 30-day payout window.
    fn window_start(&self) -> u64 {
        self.now.saturating_sub(PAYOUT_WINDOW_SECS)
    }

    fn owned(&self, hash: &[u8; 32]) -> Option<&OwnedMasternode> {
        self.owned.iter().find(|o| &o.record.pro_tx_hash == hash)
    }

    /// The wallet's own record when its fields read right: not built from a
    /// version-3 payload (D25).
    fn decoded(&self, hash: &[u8; 32]) -> Option<&OwnedMasternode> {
        self.owned(hash).filter(|o| !o.shared_unsupported)
    }

    /// A version-3 ProRegTx/ProUpRegTx in the wallet, or a list entry seen
    /// at version 3, now or before (D29): its payouts may be shared (D25).
    fn shared_unsupported(&self, hash: &[u8; 32]) -> bool {
        self.owned(hash).is_some_and(|o| o.shared_unsupported)
            || self
                .list
                .entry(hash)
                .is_some_and(|e| e.has_extended_net_info)
            || self.notes.shared_seen.contains(&display_hex(hash))
    }

    /// A version-3 entry was ever in the synced list, or a version-3
    /// provider transaction is known (D30). Nothing here reads a version-3
    /// masternode's share scripts yet (the pinned library decodes them since
    /// BL-187; MG-03b), so any one of them may pay a script of the wallet's,
    /// a version-2 row's payout script included: every coinbase output to
    /// the wallet is then `unattributed`.
    fn shares_possible(&self) -> bool {
        self.notes.any_shared_seen
            || !self.notes.shared_seen.is_empty()
            || self.list.entries.iter().any(|e| e.has_extended_net_info)
            || self
                .owned
                .iter()
                .chain(self.tracked_owned.values())
                .any(|o| o.shared_unsupported)
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

    /// The wallet's own provider transactions' payout script. A tracked
    /// masternode's registry snapshot is unproved, so it isn't used (D6);
    /// a shared one's payout isn't one script (D25).
    fn payout_script(&self, hash: &[u8; 32]) -> Option<ScriptBuf> {
        if self.shared_unsupported(hash) {
            return None;
        }
        self.decoded(hash)
            .and_then(|o| o.record.payout_script.clone())
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
        let unattributed = self.shares_possible();
        self.generated
            .iter()
            .filter(|r| r.address.as_deref() == Some(address.as_str()))
            .map(|r| payout_of(r, unattributed))
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

    /// The keys attached to `hash`, each compared with what the wallet
    /// knows now (D24); `None` while the vault is locked.
    fn attached_keys(&self, hash: &[u8; 32]) -> Option<Vec<AttachedKey>> {
        let roles = self
            .attached
            .as_ref()?
            .get(hash)
            .cloned()
            .unwrap_or_default();
        let owned = self.tracked_owned.get(hash).or_else(|| self.owned(hash));
        let refs = KeyReferences::new(self.list.entry(hash), owned, self.shared_unsupported(hash));
        let key = display_hex(hash);
        Some(
            roles
                .into_iter()
                .map(|role| {
                    let id = self.notes.key_ids.get(&(key.clone(), role)).copied();
                    AttachedKey {
                        role,
                        key_id: id.map(hex::encode),
                        check: id.map_or(KeyCheck::Unchecked, |id| refs.check(role, id)),
                    }
                })
                .collect(),
        )
    }

    /// The roles, and whether the wallet holds one through its own provider
    /// transactions ("mine", §3), which an attached key doesn't make it. A
    /// version-3 provider transaction the wallet holds makes it "mine"
    /// whatever its roles, which can't be read (D25). A version-3 list entry
    /// hides the payout role, but a payout the wallet's own record names
    /// still ties it to the wallet (D29).
    fn roles(&self, hash: &[u8; 32]) -> (MasternodeRoles, bool) {
        let owned = self.owned(hash);
        let decoded = self.decoded(hash);
        let entry = self.list.entry(hash);
        let attached = self.attached_keys(hash);
        let holds = |role| {
            attached
                .iter()
                .flatten()
                .any(|k| k.role == role && k.holds())
        };
        let voting_id = entry
            .map(|e| e.voting_key_id)
            .or_else(|| decoded.and_then(|o| o.record.voting_key_hash));
        let operator_key = entry
            .map(|e| e.operator_public_key)
            .or_else(|| decoded.and_then(|o| o.record.operator_public_key));
        let collateral = owned
            .and_then(|o| o.collateral)
            .is_some_and(|c| self.unspent.contains(&c));
        let owner = decoded
            .and_then(|o| o.record.owner_key_hash)
            .is_some_and(|id| self.owner_ids.contains(&id));
        let voting = voting_id.is_some_and(|id| self.voting_ids.contains(&id));
        let payout = self
            .payout_script(hash)
            .is_some_and(|s| self.own_scripts.contains(&s));
        let operator = operator_key.is_some_and(|k| self.operator_keys.contains(&k));
        let paid = decoded
            .and_then(|o| o.record.payout_script.clone())
            .map(ScriptBuf::from)
            .is_some_and(|s| self.own_scripts.contains(&s));
        let mine = owned.is_some_and(|o| o.shared_unsupported)
            || owned.is_some() && (collateral || owner || voting || paid || operator);
        let roles = MasternodeRoles {
            collateral,
            owner: owner || holds(AttachedKeyRole::Owner),
            voting: voting || holds(AttachedKeyRole::Voting),
            payout,
            operator,
            attached,
        };
        (roles, mine)
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
        let since = self.window_start();
        let windowed = || payouts.iter().filter(|p| at_or_after(p.timestamp, since));
        let (roles, mine) = self.roles(hash);
        let decoded = self.decoded(hash);
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
                .or_else(|| decoded.and_then(|o| o.record.service_address.clone())),
            mine,
            tracked: tracked.is_some(),
            shared_unsupported: self.shared_unsupported(hash),
            roles,
            collateral: collateral_state(owned),
            payout_address: self.payout_address(hash),
            payouts_30d: windowed()
                .filter(|p| !p.unattributed)
                .map(|p| p.amount)
                .sum(),
            payouts_incomplete: self.shared_unsupported(hash) || windowed().any(|p| p.unattributed),
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
        // A provider transaction that gives the wallet no role (it paid the
        // fee of someone's ProRegTx) doesn't make a row.
        let masternodes: Vec<MasternodeRow> = hashes
            .iter()
            .map(|h| self.row(h))
            .filter(|r| r.mine || r.tracked)
            .collect();
        // Two masternodes sharing a payout address share its records (D9);
        // the wallet total counts each record once.
        let since = self.window_start();
        let addresses: HashSet<&str> = masternodes
            .iter()
            .filter_map(|r| r.payout_address.as_deref())
            .collect();
        let windowed = || {
            self.generated
                .iter()
                .filter(|r| at_or_after(r.timestamp, since))
        };
        let income: u64 = windowed().map(|r| r.amount.unsigned_abs()).sum();
        let payouts_30d = if self.shares_possible() {
            0
        } else {
            windowed()
                .filter(|r| r.address.as_deref().is_some_and(|a| addresses.contains(a)))
                .map(|r| r.amount.unsigned_abs())
                .sum()
        };
        MasternodeOverview {
            list: MasternodeListState {
                synced: self.list.synced,
                height: self.list.height,
            },
            collateral_coins: self.collateral_coins(&masternodes),
            payouts_incomplete: masternodes.iter().any(|r| r.payouts_incomplete)
                || (self.shares_possible() && income > 0),
            masternodes,
            payouts_30d,
            unattributed_30d: income - payouts_30d,
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
                let kind = kind_of(collateral_kind(u.value())?);
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

    /// A shared masternode's keys and payout aren't read yet (MG-03b), so
    /// left out (D25).
    fn registration_facts(&self, o: &OwnedMasternode) -> RegistrationFacts {
        let r = &o.record;
        let address = |id: Option<[u8; 20]>| {
            id.filter(|_| !o.shared_unsupported)
                .map(|id| p2pkh(id, self.network))
        };
        RegistrationFacts {
            registered_height: o.registered_height(),
            collateral: collateral_state(Some(o)),
            owner_address: address(r.owner_key_hash),
            voting_address: address(r.voting_key_hash),
            payout_address: self.payout_address(&r.pro_tx_hash),
            payout_script: self.payout_script(&r.pro_tx_hash).map(hex::encode),
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
/// is locked (the record ids need the data key, MG-01 D7). Any other vault
/// error fails the read.
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
    .await?;
    match roles {
        Ok(roles) => Ok(Some(roles)),
        Err(dw_vault::VaultError::Locked) => Ok(None),
        Err(e) => Err(EngineError::from(e).into()),
    }
}

fn at_or_after(timestamp: Option<u64>, since: u64) -> bool {
    timestamp.is_some_and(|t| t >= since)
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

fn payout_of(r: &TxRecord, unattributed: bool) -> MasternodePayout {
    MasternodePayout {
        txid: r.txid.clone(),
        record_index: r.record_index,
        amount: r.amount.unsigned_abs(),
        address: r.address.clone().unwrap_or_default(),
        height: r.block_height,
        timestamp: r.timestamp,
        matured: r.status.kind != TxStatusKind::Immature,
        unattributed,
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
    // The query isn't repeated: it may be a pasted key.
    Err(EngineError::InvalidArgument(
        "not a ProTx hash or a service address".into(),
    ))
}

impl Masternodes {
    /// Runs a read on the engine runtime under one operation guard.
    async fn read_wallet<T: Send + 'static>(
        &self,
        wallet: WalletId,
        f: impl FnOnce(&WalletView) -> Result<T, MasternodesError> + Send + 'static,
    ) -> Result<T, MasternodesError> {
        self.op(move |session| async move { f(&WalletView::read(&session, wallet).await?) })
            .await
            .map_err(|e| match e {
                // The id is the caller's 64 hex (D23).
                MasternodesError::Engine(EngineError::WalletNotFound(_)) => {
                    EngineError::WalletNotFound("the requested wallet".into()).into()
                }
                e => e,
            })
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
                return Err(not_found(&v.list));
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
                return Err(not_found(&v.list));
            }
            if v.shared_unsupported(&hash) {
                return Err(MasternodesError::Unsupported {
                    feature: "shared_masternodes".into(),
                    field: None,
                });
            }
            let mut payouts = v.payouts(&hash);
            if let Some(since) = since {
                payouts.retain(|p| at_or_after(p.timestamp, since));
            }
            Ok(payouts)
        })
        .await
    }

    /// Masternodes of the synced list matching a ProTx hash (64 hex) or a
    /// service address (`ip:port`, or `ip` for any port), in list order.
    pub async fn find(&self, query: String) -> Result<Vec<ListedMasternode>, MasternodesError> {
        let parsed = parse_query(&query)?;
        self.op(move |session| async move {
            let manager = session.manager()?;
            let list = read_list(&session, &manager).await;
            if !list.synced {
                return Err(MasternodesError::ListUnavailable);
            }
            let tracked = manager.tracked_masternode_hashes();
            let network = session.network.core_network();
            Ok(find_in_summaries(&list.entries, &parsed)
                .into_iter()
                .map(|e| ListedMasternode {
                    pro_tx_hash: display_hex(&e.pro_tx_hash),
                    facts: list_facts(e, network),
                    tracked: tracked.contains(&e.pro_tx_hash),
                })
                .collect())
        })
        .await
    }

    /// An evonode's claimable credits: its owner identity's balance, read
    /// with a Platform proof (MG-03 §6).
    pub async fn evonode_credits(
        &self,
        pro_tx_hash: String,
    ) -> Result<EvonodeCredits, MasternodesError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let shown = display_hex(&hash);
        let sdk = self
            .op(move |session| async move {
                let manager = session.manager()?;
                let list = read_list(&session, &manager).await;
                if !list.synced {
                    return Err(MasternodesError::ListUnavailable);
                }
                let entry = list.entry(&hash).ok_or_else(|| not_found(&list))?;
                if !entry.is_evonode {
                    return Err(MasternodesError::NotEvonode);
                }
                if !session.platform.enabled() {
                    return Err(MasternodesError::PlatformUnavailable {
                        detail: "this session runs without Platform".into(),
                    });
                }
                Ok(manager.sdk_arc())
            })
            .await?;
        // The fetch runs outside the guard: a slow DAPI call must not hold a
        // session close back. The identity id is the display-order hash.
        let mut id = hash;
        id.reverse();
        let balance = self
            .session
            .on_runtime(async move { Ok(IdentityBalance::fetch(&sdk, Identifier::from(id)).await) })
            .await?;
        match balance {
            Ok(Some(credits)) => Ok(EvonodeCredits {
                pro_tx_hash: shown,
                credits,
                duffs: credits / dpp::balances::credits::CREDITS_PER_DUFF,
                fetched_at: unix_now(),
            }),
            Ok(None) => Err(MasternodesError::NoIdentity),
            // The SDK's text may name the identity, which is the hash.
            Err(e) => {
                tracing::debug!(error = %e, "evonode credits");
                Err(MasternodesError::PlatformUnavailable {
                    detail: "the identity balance could not be read".into(),
                })
            }
        }
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
            field: None,
        })
    }
}

/// What a ProUpRegTx of a masternode needs from the wallet (MG-04 D10,
/// D12), read as MG-03's reads do.
#[derive(Debug, Clone, Default)]
pub(super) struct OwnerFacts {
    /// The owner key id of a ProRegTx a loaded wallet holds.
    pub owner_key_id: Option<[u8; 20]>,
    /// An `owner` key is attached and matches that id.
    pub attached_owner: bool,
    /// The wallet's latest payout script of it.
    pub payout_script: Option<ScriptBuf>,
    /// Its collateral's script, when the collateral is this wallet's coin.
    pub collateral_script: Option<ScriptBuf>,
    pub operator_reward: Option<u16>,
    /// A version-3 provider transaction or list entry was seen (D25, D29).
    pub shared: bool,
}

/// [`OwnerFacts`] of `hash` for `wallet`.
pub(super) async fn owner_facts(
    session: &NetworkSession,
    wallet: WalletId,
    hash: [u8; 32],
) -> Result<OwnerFacts, MasternodesError> {
    let v = WalletView::read(session, wallet).await?;
    let record = (v.tracked_owned.get(&hash))
        .or_else(|| v.owned(&hash))
        .filter(|o| !o.shared_unsupported);
    let collateral = v.owned(&hash).and_then(|o| o.collateral);
    Ok(OwnerFacts {
        owner_key_id: record.and_then(|o| o.record.owner_key_hash),
        attached_owner: v
            .attached_keys(&hash)
            .into_iter()
            .flatten()
            .any(|k| k.role == AttachedKeyRole::Owner && k.check == KeyCheck::Matched),
        payout_script: v.payout_script(&hash),
        collateral_script: v
            .coins
            .coins
            .iter()
            .find(|c| Some(c.utxo.outpoint) == collateral)
            .map(|c| c.utxo.txout.script_pubkey.clone()),
        operator_reward: record.and_then(|o| o.operator_reward),
        shared: v.shared_unsupported(&hash),
    })
}

/// `masternodes.not_found`, or `list_unavailable` when the list that
/// wasn't searched might have held it.
pub(super) fn not_found(list: &ListView) -> MasternodesError {
    if list.synced {
        MasternodesError::NotFound
    } else {
        MasternodesError::ListUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TxCategory, TxStatus};
    use dashcore::Transaction;
    use dashcore::transaction::TransactionPayload;

    use super::super::aggregate::aggregate;
    use super::super::aggregate::fixtures::{coins, null, payout, registration, registration_v};
    use dw_protx::params::MASTERNODE_COLLATERAL;

    const NOW: u64 = 1_800_000_000;

    /// Pays the registration's reward to `payout(seed)`: its `scriptPayout`,
    /// or from version 3 its one payout.
    fn with_payout(mut tx: Transaction, seed: u8) -> Transaction {
        if let Some(TransactionPayload::ProviderRegistrationPayloadType(p)) =
            &mut tx.special_transaction_payload
        {
            match &mut p.payouts {
                Some(payouts) => payouts[0].script_payout = payout(seed),
                None => p.script_payout = payout(seed),
            }
        }
        tx
    }

    fn reward(seed: u8, amount: i64) -> TxRecord {
        TxRecord {
            txid: format!("{seed:064x}"),
            record_index: 0,
            tx_type: TxType::Generated,
            category: TxCategory::Reward,
            status: TxStatus {
                kind: TxStatusKind::Confirmed,
                confirmations: 200,
                instant_locked: false,
                chain_locked: true,
                matures_in: None,
            },
            timestamp: Some(NOW - 1000),
            block_height: Some(100),
            amount,
            fee: None,
            address: Some(
                Address::from_script(&payout(seed), Network::Regtest)
                    .unwrap()
                    .to_string(),
            ),
            label: None,
            counts_toward_balance: true,
            involves_watch_only: false,
            counterparty: None,
        }
    }

    fn view(txs: &[Transaction], list: ListView, generated: Vec<TxRecord>) -> WalletView {
        let held: Vec<_> = txs
            .iter()
            .enumerate()
            .map(|(i, tx)| (10 + i as u32, 0, tx.clone()))
            .collect();
        WalletView {
            network: Network::Regtest,
            list,
            owned: aggregate(&held),
            operator_keys: HashSet::new(),
            tracked: Vec::new(),
            tracked_owned: HashMap::new(),
            owner_ids: HashSet::new(),
            voting_ids: HashSet::new(),
            attached: Some(HashMap::new()),
            own_scripts: [payout(4), payout(6)].into(),
            coins: CoinSnapshot {
                height: 100,
                coins: Vec::new(),
            },
            unspent: HashSet::new(),
            locked: HashSet::new(),
            generated,
            notes: Notes::default(),
            now: NOW,
        }
    }

    /// A synced list of `tx`'s masternode, at version 3 when `shared`.
    fn listed(tx: &Transaction, shared: bool) -> ListView {
        ListView {
            synced: true,
            height: Some(100),
            entries: vec![MasternodeListSummary {
                pro_tx_hash: tx.txid().to_byte_array(),
                service_address: None,
                platform_http_port: None,
                operator_public_key: [2; 48],
                voting_key_id: [3; 20],
                platform_node_id: None,
                is_valid: true,
                is_evonode: false,
                has_extended_net_info: shared,
            }],
        }
    }

    /// A version-3 registration stays in Mine, flagged, with no payout and
    /// none of its payouts in a total (D25); its shares may pay the other
    /// row's script, so that one's payouts are unattributed too (D30).
    #[test]
    fn a_shared_masternode_is_mine_but_out_of_the_payout_totals() {
        let plain = registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL]));
        let shared = with_payout(
            registration_v(3, null(0), false, 0, coins(&[MASTERNODE_COLLATERAL, 1])),
            6,
        );
        let v = view(
            &[plain.clone(), shared.clone()],
            ListView::default(),
            vec![reward(4, 500), reward(6, 700)],
        );
        let overview = v.overview();
        assert_eq!(overview.masternodes.len(), 2);
        let row = |tx: &Transaction| {
            overview
                .masternodes
                .iter()
                .find(|r| r.pro_tx_hash == display_hex(&tx.txid().to_byte_array()))
                .unwrap()
        };
        let p = row(&plain);
        assert!(!p.shared_unsupported && p.mine && p.roles.payout);
        assert_eq!(p.payouts_30d, 0);
        assert!(p.payouts_incomplete);
        assert!(p.last_payout.as_ref().unwrap().unattributed);
        let s = row(&shared);
        assert!(s.shared_unsupported);
        assert!(s.mine, "its ProRegTx is in the wallet");
        assert!(!s.roles.payout && !s.roles.owner && !s.roles.voting);
        assert_eq!((s.payouts_30d, s.payout_address.as_ref()), (0, None));
        assert!(s.last_payout.is_none());
        assert!(s.payouts_incomplete);
        assert_eq!(overview.payouts_30d, 0);
        assert_eq!(overview.unattributed_30d, 1200, "both stay visible");
        assert!(overview.payouts_incomplete);
        let hash = shared.txid().to_byte_array();
        assert!(v.shared_unsupported(&hash) && v.payouts(&hash).is_empty());
        let facts = v.detail(&hash).registration.unwrap();
        assert_eq!(facts.owner_address, None);
        assert_eq!(facts.payout_script, None);
        assert_eq!(facts.operator_reward, None);

        // A version-3 list entry flags a version-2 registration too: a
        // ProUpRegTx the wallet doesn't hold may have shared its payout.
        let v = view(
            std::slice::from_ref(&plain),
            listed(&plain, true),
            vec![reward(4, 500)],
        );
        let overview = v.overview();
        let row = &overview.masternodes[0];
        assert!(row.shared_unsupported && !row.roles.payout);
        assert!(row.mine, "its own record pays the wallet (D29)");
        assert_eq!(overview.payouts_30d, 0);
        assert!(overview.payouts_incomplete);

        // Remembered once seen, before the list syncs (D29).
        let mut v = view(
            std::slice::from_ref(&plain),
            ListView::default(),
            vec![reward(4, 500)],
        );
        v.notes
            .shared_seen
            .insert(display_hex(&plain.txid().to_byte_array()));
        assert!(v.overview().masternodes[0].shared_unsupported);
    }

    /// A fee the wallet paid for someone's provider transaction makes no
    /// row, even at version 3; while the list has a version-3 entry, a
    /// coinbase output that pays no row leaves the total incomplete (D29).
    #[test]
    fn unattributed_coinbase_income_marks_the_total_incomplete() {
        let foreign = with_payout(
            registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL])),
            9,
        );
        for shared in [false, true] {
            let v = view(
                std::slice::from_ref(&foreign),
                listed(&foreign, shared),
                vec![reward(4, 500)],
            );
            let overview = v.overview();
            assert!(overview.masternodes.is_empty(), "a fee is no role");
            assert_eq!(overview.payouts_30d, 0);
            assert_eq!(overview.unattributed_30d, 500);
            assert_eq!(overview.payouts_incomplete, shared);
        }
    }

    /// A synced list of `tx`'s masternode at version 2 and, when `other`,
    /// another masternode's entry at version 3.
    fn listed_beside(tx: &Transaction, other: bool) -> ListView {
        let mut list = listed(tx, false);
        if other {
            let mut entry = list.entries[0].clone();
            entry.pro_tx_hash = [0xee; 32];
            entry.has_extended_net_info = true;
            list.entries.push(entry);
        }
        list
    }

    /// Nothing reads a version-3 masternode's share scripts yet (MG-03b),
    /// so once one was in the list, now or before, any
    /// coinbase output to a version-2 row's payout script may be a share:
    /// it is unattributed, out of the row's and the wallet's total, and
    /// counted in `unattributed_30d` (D30).
    #[test]
    fn a_version_2_payout_is_unattributed_once_any_version_3_entry_was_seen() {
        let plain = with_payout(
            registration(null(0), false, 0, coins(&[MASTERNODE_COLLATERAL])),
            4,
        );
        let check = |v: &WalletView, possible: bool| {
            let overview = v.overview();
            let row = &overview.masternodes[0];
            assert!(!row.shared_unsupported && row.roles.payout);
            assert!(row.payout_address.is_some());
            let last = row.last_payout.as_ref().expect("its payout stays listed");
            assert_eq!(last.unattributed, possible);
            assert_eq!(row.payouts_30d, if possible { 0 } else { 500 });
            assert_eq!(row.payouts_incomplete, possible);
            assert_eq!(overview.payouts_30d, if possible { 0 } else { 500 });
            assert_eq!(overview.unattributed_30d, if possible { 500 } else { 0 });
            assert_eq!(overview.payouts_incomplete, possible);
            let hash = plain.txid().to_byte_array();
            assert_eq!(v.payouts(&hash).len(), 1);
        };
        for other in [false, true] {
            let v = view(
                std::slice::from_ref(&plain),
                listed_beside(&plain, other),
                vec![reward(4, 500)],
            );
            check(&v, other);
        }
        // A removed version-3 masternode's shares: the list no longer has
        // it, the remembered marker still counts (D30).
        let mut v = view(
            std::slice::from_ref(&plain),
            listed_beside(&plain, false),
            vec![reward(4, 500)],
        );
        v.notes.any_shared_seen = true;
        check(&v, true);
        // As does a remembered version-3 entry of a followed masternode.
        let mut v = view(
            std::slice::from_ref(&plain),
            listed_beside(&plain, false),
            vec![reward(4, 500)],
        );
        v.notes.shared_seen.insert("ee".repeat(32));
        check(&v, true);
    }

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
