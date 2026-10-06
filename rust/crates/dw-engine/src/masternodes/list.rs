//! The Masternodes tab's list model (QT-118…122, IOS-080): the SPV
//! masternode list merged with what the wallets' provider transactions and
//! the tracked-masternode registry know, with owned detection (QT-120).
//!
//! Sources (research 02 §10.2, dash-qt `src/qt/masternodemodel.cpp`):
//! - the list (dash-spv's simplified masternode list, platform-wallet
//!   `MasternodeListSummary`): service, type, valid/banned, voting key id,
//!   operator key, Platform node id and HTTP port;
//! - the wallets' provider transactions (platform-wallet
//!   `aggregate_masternodes` over `provider_masternode_txs_blocking`): owner,
//!   payout, collateral, registration height, operator reward and payout,
//!   revocation;
//! - tracked masternodes (platform-wallet `TrackedMasternodes`): the label
//!   and the registration details a refresh learned.
//!
//! Everything else dash-qt shows (PoSe score, ban and revive heights, last
//! paid, next payment, consecutive payments, owner/payout of masternodes no
//! wallet registered) needs a full node and is `None` (DESIGN-opus §1.14).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use dashcore::address::Payload;
use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, PubkeyHash, ScriptBuf, Transaction, Txid};
use dw_protx::service::service_text;
use platform_wallet::masternode::{
    ListMembership, MasternodeListSummary, MasternodeRecord, TrackedMasternode,
    aggregate_masternodes,
};

use crate::session::Manager;
use crate::{EngineError, EngineEvent, MasternodeFailure, NetworkSession, WalletId};

/// Regular masternode or EvoNode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MasternodeType {
    Regular,
    Evo,
}

/// The list's type combo (QT-118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MasternodeTypeFilter {
    All,
    Regular,
    Evo,
    /// v24 shared masternodes. The wallets do not read share tables yet, so
    /// this filter matches nothing.
    Shared,
}

/// Why the wallets count a masternode as theirs (QT-120 "Owned").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OwnedRole {
    Collateral,
    Owner,
    Voting,
    Operator,
    Payout,
    OperatorPayout,
    PlatformNode,
    ShareOwner,
    ShareRefund,
    Tracked,
}

/// Status from the list (QT-119 status icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MasternodeListStatus {
    /// Valid in the list; the height it became valid is unknown on SPV.
    Active { since_height: Option<u32> },
    /// PoSe-banned in the list; the ban height is unknown on SPV.
    Banned { since_height: Option<u32> },
    /// Known to a wallet or tracked, but no longer in the synced list.
    Retired,
    /// The list has not synced.
    Unknown,
}

/// "Operator Reward" column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorReward {
    /// Hundredths of a percent.
    pub percent_x100: u16,
    pub payout_address: Option<String>,
}

/// One list row (QT-119). `None` = not known to an SPV wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeRow {
    /// Display order (explorers, `protx list`).
    pub pro_tx_hash: String,
    pub service: Option<String>,
    pub node_type: MasternodeType,
    /// `(held, total)` shares of a v24 shared masternode.
    pub shared: Option<(u32, u32)>,
    pub status: MasternodeListStatus,
    pub pose_score: Option<u32>,
    pub registered_height: Option<u32>,
    pub last_paid_height: Option<u32>,
    pub next_payment_height: Option<u32>,
    pub operator_reward: Option<OperatorReward>,
    pub collateral: Option<OutPoint>,
    pub collateral_address: Option<String>,
    pub owner_address: Option<String>,
    /// Empty when nothing the wallet holds names the voting key.
    pub voting_address: String,
    pub payout_addresses: Vec<String>,
    /// 96 hex characters, as the list or the payload carries it.
    pub operator_public_key: String,
    pub platform_node_id: Option<String>,
    pub owned_roles: Vec<OwnedRole>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeQuery {
    pub type_filter: MasternodeTypeFilter,
    pub text: Option<String>,
    pub owned_only: bool,
    pub hide_banned: bool,
}

impl Default for MasternodeQuery {
    fn default() -> Self {
        Self {
            type_filter: MasternodeTypeFilter::All,
            text: None,
            owned_only: false,
            hide_banned: false,
        }
    }
}

/// Masternode list availability (QT-118 "Node Count").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MasternodeListState {
    pub available: bool,
    pub height: Option<u32>,
    pub total: u32,
    pub enabled: u32,
    pub evo_total: u32,
    pub evo_enabled: u32,
    pub syncing: bool,
}

/// One share of a shared masternode (details shares table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeShare {
    pub amount: u64,
    pub owner_address: String,
    pub payout_address: String,
    pub refund_address: String,
    pub mine: bool,
}

/// The details dialog (QT-122, IOS-080).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeDetail {
    pub row: MasternodeRow,
    pub consecutive_payments: Option<u32>,
    pub pose_ban_height: Option<u32>,
    pub pose_revived_height: Option<u32>,
    pub network_addresses: Vec<String>,
    pub platform_p2p_addresses: Vec<String>,
    pub platform_https_addresses: Vec<String>,
    pub shares: Vec<MasternodeShare>,
    pub early_period_end: Option<u32>,
    pub early_exit_penalty: Option<u64>,
    pub has_standby_dissolution: bool,
    pub revocation_reason: Option<u16>,
    pub wallet_transactions: u32,
}

/// What one wallet's provider transactions say about a masternode, beyond
/// platform-wallet's record.
#[derive(Debug, Clone)]
pub(crate) struct WalletMasternode {
    pub wallet_id: WalletId,
    pub record: MasternodeRecord,
    pub operator_reward: Option<u16>,
    /// Latest ProUpServTx operator payout script (empty = none).
    pub operator_payout: Option<ScriptBuf>,
    pub platform_p2p_port: Option<u16>,
    /// The output script of a collateral the ProRegTx itself created.
    pub collateral_script: Option<ScriptBuf>,
    /// The wallet derives this operator key at this index.
    pub operator_key_index: Option<u32>,
}

/// Everything known about one masternode (wire-order proTxHash).
#[derive(Debug, Clone, Default)]
pub(crate) struct Known {
    pub hash: [u8; 32],
    pub list: Option<MasternodeListSummary>,
    pub wallet: Option<WalletMasternode>,
    pub tracked: Option<TrackedMasternode>,
    pub owned: BTreeSet<OwnedRole>,
    /// Wallets with a role in it (owned detection), first = the record's.
    pub owners: BTreeMap<WalletId, BTreeSet<OwnedRole>>,
}

/// One pass over every source.
pub(crate) struct Snapshot {
    pub network: dashcore::Network,
    pub list_available: bool,
    pub known: BTreeMap<[u8; 32], Known>,
}

/// Display (explorer) hex of a wire-order hash.
pub(crate) fn display_hex(wire: &[u8; 32]) -> String {
    Txid::from_byte_array(*wire).to_string()
}

/// Wire order of a display-order proTxHash.
pub(crate) fn parse_pro_tx_hash(text: &str) -> Result<[u8; 32], EngineError> {
    let txid: Txid = text
        .parse()
        .map_err(|_| EngineError::InvalidArgument(format!("{text:?} is not a proTxHash")))?;
    Ok(txid.to_byte_array())
}

/// Every unspent outpoint the wallet holds, in any funds account.
pub(crate) fn wallet_unspent(
    info: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
) -> HashSet<OutPoint> {
    use key_wallet::managed_account::managed_account_ref::ManagedAccountRef;
    info.accounts
        .all_accounts()
        .iter()
        .filter_map(|a| match a {
            ManagedAccountRef::Funds(f) => Some(f.utxos.keys().copied().collect::<Vec<_>>()),
            ManagedAccountRef::Keys(_) => None,
        })
        .flatten()
        .collect()
}

/// A ProRegTx, ProUpServTx, ProUpRegTx or ProUpRevTx.
fn is_provider_tx(tx: &Transaction) -> bool {
    matches!(
        tx.special_transaction_payload,
        Some(
            TransactionPayload::ProviderRegistrationPayloadType(_)
                | TransactionPayload::ProviderUpdateServicePayloadType(_)
                | TransactionPayload::ProviderUpdateRegistrarPayloadType(_)
                | TransactionPayload::ProviderUpdateRevocationPayloadType(_)
        )
    )
}

fn p2pkh(hash: &[u8; 20], network: dashcore::Network) -> Address {
    Address::new(
        network,
        Payload::PubkeyHash(PubkeyHash::from_byte_array(*hash)),
    )
}

fn script_address(script: &[u8], network: dashcore::Network) -> Option<String> {
    Address::from_script(&ScriptBuf::from_bytes(script.to_vec()), network)
        .ok()
        .map(|a| a.to_string())
}

/// The fields of a wallet's provider transactions that the record does not
/// carry: operator reward (ProRegTx), operator payout (latest ProUpServTx),
/// Platform P2P port, a self-created collateral's script.
fn wallet_extras(
    txs: &[(u32, u32, Transaction)],
    hash: &[u8; 32],
) -> (
    Option<u16>,
    Option<ScriptBuf>,
    Option<u16>,
    Option<ScriptBuf>,
) {
    let mut ordered: Vec<&(u32, u32, Transaction)> = txs.iter().collect();
    ordered.sort_by_key(|(h, p, _)| (*h, *p));
    let (mut reward, mut op_payout, mut p2p, mut collateral) = (None, None, None, None);
    for (_, _, tx) in ordered {
        match &tx.special_transaction_payload {
            Some(TransactionPayload::ProviderRegistrationPayloadType(p))
                if &tx.txid().to_byte_array() == hash =>
            {
                reward = Some(p.operator_reward);
                p2p = p.platform_p2p_port.or(p2p);
                if p.collateral_outpoint.txid == Txid::all_zeros() {
                    collateral = tx
                        .output
                        .get(p.collateral_outpoint.vout as usize)
                        .map(|o| o.script_pubkey.clone());
                }
            }
            Some(TransactionPayload::ProviderUpdateServicePayloadType(p))
                if &p.pro_tx_hash.to_byte_array() == hash =>
            {
                op_payout = Some(p.script_payout.clone());
                p2p = p.platform_p2p_port.or(p2p);
            }
            _ => {}
        }
    }
    (reward, op_payout, p2p, collateral)
}

/// Reads every wallet's provider records (blocking manager accessors).
///
/// platform-wallet keeps only recent transactions in memory after a restart,
/// so the provider transactions also come from the session's history store,
/// which holds every persisted record (`history_ops::load_history`):
/// `stored` per wallet, merged with the in-memory ones by txid.
fn wallet_masternodes_blocking(
    manager: &Manager,
    stored: &HashMap<WalletId, Vec<(u32, u32, Transaction)>>,
    membership: &dyn Fn(&[u8; 32]) -> ListMembership,
) -> Vec<WalletMasternode> {
    let mut out = Vec::new();
    for raw in manager.list_wallet_ids_blocking() {
        let Some((_, in_memory, _, operator_index, platform_index)) =
            manager.provider_masternode_txs_blocking(&raw)
        else {
            continue;
        };
        let mut by_txid: BTreeMap<Txid, (u32, u32, Transaction)> = BTreeMap::new();
        for (h, p, tx) in stored
            .get(&WalletId(raw))
            .into_iter()
            .flatten()
            .chain(&in_memory)
        {
            let slot = by_txid
                .entry(tx.txid())
                .or_insert_with(|| (*h, *p, tx.clone()));
            // A block height beats "unconfirmed" from the other source.
            if slot.0 == 0 && *h > 0 {
                *slot = (*h, *p, tx.clone());
            }
        }
        let txs: Vec<(u32, u32, Transaction)> = by_txid.into_values().collect();
        let records = aggregate_masternodes(txs.iter().map(|(h, p, tx)| (*h, *p, tx)), membership);
        for mut record in records {
            record.operator_key_index = record
                .operator_public_key
                .and_then(|k| operator_index.get(&k).copied());
            record.platform_key_index = record
                .platform_node_id
                .and_then(|id| platform_index.get(&id).copied());
            let (operator_reward, operator_payout, platform_p2p_port, collateral_script) =
                wallet_extras(&txs, &record.pro_tx_hash);
            out.push(WalletMasternode {
                wallet_id: WalletId(raw),
                operator_key_index: record.operator_key_index,
                record,
                operator_reward,
                operator_payout,
                platform_p2p_port,
                collateral_script,
            });
        }
    }
    out
}

impl Known {
    fn new(hash: [u8; 32]) -> Self {
        Self {
            hash,
            ..Default::default()
        }
    }

    pub(crate) fn is_evo(&self) -> bool {
        self.list.as_ref().map(|l| l.is_evonode).unwrap_or_else(|| {
            self.wallet
                .as_ref()
                .map(|w| w.record.is_evonode)
                .or_else(|| {
                    self.tracked
                        .as_ref()
                        .and_then(|t| t.snapshot.registration.as_ref())
                        .map(|r| r.is_evonode)
                })
                .unwrap_or(false)
        })
    }

    /// Current operator key: the list's, else the latest payload's.
    pub(crate) fn operator_public_key(&self) -> Option<[u8; 48]> {
        self.list
            .as_ref()
            .map(|l| l.operator_public_key)
            .or_else(|| {
                self.wallet
                    .as_ref()
                    .and_then(|w| w.record.operator_public_key)
            })
            .or_else(|| {
                self.tracked
                    .as_ref()
                    .and_then(|t| t.snapshot.registration.as_ref())
                    .map(|r| r.operator_public_key)
            })
    }

    pub(crate) fn voting_key_hash(&self) -> Option<[u8; 20]> {
        self.list
            .as_ref()
            .map(|l| l.voting_key_id)
            .or_else(|| self.wallet.as_ref().and_then(|w| w.record.voting_key_hash))
            .or_else(|| {
                self.tracked
                    .as_ref()
                    .and_then(|t| t.snapshot.registration.as_ref())
                    .map(|r| r.voting_key_hash)
            })
    }

    pub(crate) fn owner_key_hash(&self) -> Option<[u8; 20]> {
        self.wallet
            .as_ref()
            .and_then(|w| w.record.owner_key_hash)
            .or_else(|| {
                self.tracked.as_ref().and_then(|t| {
                    t.snapshot
                        .platform
                        .as_ref()
                        .and_then(|p| p.owner_key_hash)
                        .or_else(|| t.snapshot.registration.as_ref().map(|r| r.owner_key_hash))
                })
            })
    }

    /// Current owner payout script.
    pub(crate) fn payout_script(&self) -> Option<ScriptBuf> {
        self.wallet
            .as_ref()
            .and_then(|w| w.record.payout_script.clone())
            .or_else(|| {
                self.tracked
                    .as_ref()
                    .and_then(|t| t.snapshot.registration.as_ref())
                    .map(|r| r.payout_script.clone())
            })
            .map(ScriptBuf::from_bytes)
    }

    pub(crate) fn platform_node_id(&self) -> Option<[u8; 20]> {
        self.list
            .as_ref()
            .and_then(|l| l.platform_node_id)
            .or_else(|| self.wallet.as_ref().and_then(|w| w.record.platform_node_id))
            .or_else(|| {
                self.tracked
                    .as_ref()
                    .and_then(|t| t.snapshot.registration.as_ref())
                    .and_then(|r| r.platform_node_id)
            })
    }

    pub(crate) fn service(&self) -> Option<SocketAddr> {
        self.list
            .as_ref()
            .and_then(|l| l.service_address)
            .or_else(|| {
                let text = self
                    .wallet
                    .as_ref()
                    .and_then(|w| w.record.service_address.clone())
                    .or_else(|| {
                        self.tracked
                            .as_ref()
                            .and_then(|t| t.snapshot.registration.as_ref())
                            .and_then(|r| r.service_address.clone())
                    })?;
                text.parse::<SocketAddr>()
                    .ok()
                    .filter(|a| !dw_protx::service::is_no_service(a))
            })
    }

    /// The collateral outpoint (a self-created collateral resolved to the
    /// ProRegTx's own txid).
    pub(crate) fn collateral(&self) -> Option<OutPoint> {
        let (txid, vout) = self
            .wallet
            .as_ref()
            .and_then(|w| w.record.collateral)
            .or_else(|| {
                self.tracked
                    .as_ref()
                    .and_then(|t| t.snapshot.registration.as_ref())
                    .map(|r| r.collateral)
            })?;
        Some(dw_protx::payloads::resolve_collateral(
            Txid::from_byte_array(self.hash),
            OutPoint {
                txid: Txid::from_byte_array(txid),
                vout,
            },
        ))
    }

    fn status(&self, list_available: bool) -> MasternodeListStatus {
        match (&self.list, list_available) {
            (Some(l), _) if l.is_valid => MasternodeListStatus::Active { since_height: None },
            (Some(_), _) => MasternodeListStatus::Banned { since_height: None },
            (None, true) => MasternodeListStatus::Retired,
            (None, false) => MasternodeListStatus::Unknown,
        }
    }

    pub(crate) fn row(&self, network: dashcore::Network, list_available: bool) -> MasternodeRow {
        let wallet = self.wallet.as_ref();
        let registered_height = wallet
            .filter(|w| w.record.has_registration)
            .map(|w| w.record.registration_height)
            .or_else(|| {
                self.tracked
                    .as_ref()
                    .and_then(|t| t.snapshot.registration.as_ref())
                    .map(|r| r.height)
            })
            .filter(|h| *h > 0);
        let operator_reward = wallet.and_then(|w| {
            w.operator_reward.map(|percent_x100| OperatorReward {
                percent_x100,
                payout_address: w
                    .operator_payout
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .and_then(|s| script_address(s.as_bytes(), network)),
            })
        });
        let collateral_address = wallet
            .and_then(|w| w.collateral_script.as_ref())
            .and_then(|s| script_address(s.as_bytes(), network));
        MasternodeRow {
            pro_tx_hash: display_hex(&self.hash),
            service: self.service().as_ref().and_then(service_text),
            node_type: if self.is_evo() {
                MasternodeType::Evo
            } else {
                MasternodeType::Regular
            },
            shared: None,
            status: self.status(list_available),
            pose_score: None,
            registered_height,
            last_paid_height: None,
            next_payment_height: None,
            operator_reward,
            collateral: self.collateral(),
            collateral_address,
            owner_address: self
                .owner_key_hash()
                .map(|h| p2pkh(&h, network).to_string()),
            voting_address: self
                .voting_key_hash()
                .map(|h| p2pkh(&h, network).to_string())
                .unwrap_or_default(),
            payout_addresses: self
                .payout_script()
                .and_then(|s| script_address(s.as_bytes(), network))
                .into_iter()
                .collect(),
            operator_public_key: self
                .operator_public_key()
                .map(hex::encode)
                .unwrap_or_default(),
            platform_node_id: self.platform_node_id().map(hex::encode),
            owned_roles: self.owned.iter().copied().collect(),
            label: self.tracked.as_ref().and_then(|t| t.label.clone()),
        }
    }

    pub(crate) fn detail(
        &self,
        network: dashcore::Network,
        list_available: bool,
    ) -> MasternodeDetail {
        let row = self.row(network, list_available);
        let service = self.service();
        let with_port = |port: Option<u16>| -> Vec<String> {
            match (service, port) {
                (Some(s), Some(p)) => service_text(&SocketAddr::new(s.ip(), p))
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        };
        let evo = self.is_evo();
        let http_port = self
            .list
            .as_ref()
            .and_then(|l| l.platform_http_port)
            .or_else(|| {
                self.wallet
                    .as_ref()
                    .and_then(|w| w.record.platform_http_port)
            });
        let p2p_port = self.wallet.as_ref().and_then(|w| w.platform_p2p_port);
        MasternodeDetail {
            network_addresses: row.service.clone().into_iter().collect(),
            platform_p2p_addresses: if evo { with_port(p2p_port) } else { Vec::new() },
            platform_https_addresses: if evo {
                with_port(http_port)
            } else {
                Vec::new()
            },
            shares: Vec::new(),
            early_period_end: None,
            early_exit_penalty: None,
            has_standby_dissolution: false,
            revocation_reason: self
                .wallet
                .as_ref()
                .filter(|w| w.record.revoked)
                .map(|w| w.record.revocation_reason),
            wallet_transactions: self.wallet.as_ref().map(|w| w.record.tx_count).unwrap_or(0),
            consecutive_payments: None,
            pose_ban_height: None,
            pose_revived_height: None,
            row,
        }
    }
}

/// The literal, case-insensitive search fields of a row (dash-qt's filter
/// columns; not the operator key or the platform node id).
fn search_text(row: &MasternodeRow) -> String {
    let mut parts: Vec<String> = vec![row.pro_tx_hash.clone()];
    parts.extend(row.service.clone());
    parts.push(
        match row.node_type {
            MasternodeType::Regular => "Regular",
            MasternodeType::Evo => "Evo",
        }
        .to_string(),
    );
    parts.extend(row.registered_height.map(|h| h.to_string()));
    parts.extend(row.payout_addresses.iter().cloned());
    if let Some(r) = &row.operator_reward {
        parts.push(format!(
            "{}.{:02}%",
            r.percent_x100 / 100,
            r.percent_x100 % 100
        ));
        parts.extend(r.payout_address.clone());
    }
    parts.extend(row.collateral.map(|c| format!("{}-{}", c.txid, c.vout)));
    parts.extend(row.collateral_address.clone());
    parts.extend(row.owner_address.clone());
    parts.push(row.voting_address.clone());
    parts.join("\n").to_lowercase()
}

/// Whether `row` passes `query`.
pub(crate) fn matches(row: &MasternodeRow, query: &MasternodeQuery) -> bool {
    let type_ok = match query.type_filter {
        MasternodeTypeFilter::All => true,
        MasternodeTypeFilter::Regular => row.node_type == MasternodeType::Regular,
        MasternodeTypeFilter::Evo => row.node_type == MasternodeType::Evo,
        MasternodeTypeFilter::Shared => row.shared.is_some(),
    };
    let owned_ok = !query.owned_only || !row.owned_roles.is_empty();
    let banned_ok =
        !query.hide_banned || !matches!(row.status, MasternodeListStatus::Banned { .. });
    let text_ok = match query.text.as_deref().map(str::trim) {
        None | Some("") => true,
        Some(t) => search_text(row).contains(&t.to_lowercase()),
    };
    type_ok && owned_ok && banned_ok && text_ok
}

impl NetworkSession {
    /// Reads the list, the wallets' provider records and the tracked
    /// masternodes, and runs owned detection.
    pub(crate) async fn masternode_snapshot(&self) -> Result<Snapshot, EngineError> {
        let manager = self.manager()?;
        let network = self.network.core_network();
        let summaries = manager.spv().masternode_list_summaries().await;
        let list_available = summaries.is_some();
        let list: HashMap<[u8; 32], MasternodeListSummary> = summaries
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.pro_tx_hash, s))
            .collect();
        let validity: HashMap<[u8; 32], bool> =
            list.iter().map(|(k, v)| (*k, v.is_valid)).collect();
        let stored: HashMap<WalletId, Vec<(u32, u32, Transaction)>> = manager
            .list_wallet_ids_blocking()
            .into_iter()
            .map(|raw| {
                let txs = self
                    .hub
                    .history
                    .snapshot(&WalletId(raw))
                    .into_values()
                    .filter(|e| !e.abandoned && is_provider_tx(&e.tx))
                    .map(|e| {
                        let (h, p) = e
                            .context
                            .block_info()
                            .map(|b| (b.height(), b.position().unwrap_or(0)))
                            .unwrap_or((0, 0));
                        (h, p, e.tx)
                    })
                    .collect();
                (WalletId(raw), txs)
            })
            .collect();
        let (wallet_mns, tracked) = {
            let manager = Arc::clone(&manager);
            tokio::task::spawn_blocking(move || {
                let membership = |hash: &[u8; 32]| -> ListMembership {
                    if !list_available {
                        return ListMembership::ListUnavailable;
                    }
                    match validity.get(hash) {
                        Some(true) => ListMembership::ValidEntry,
                        Some(false) => ListMembership::InvalidEntry,
                        None => ListMembership::Absent,
                    }
                };
                let wallet_mns = wallet_masternodes_blocking(&manager, &stored, &membership);
                let service = manager.tracked_masternodes_service();
                let tracked: Vec<TrackedMasternode> = service
                    .hashes()
                    .iter()
                    .filter_map(|h| service.get(h))
                    .collect();
                (wallet_mns, tracked)
            })
            .await?
        };

        let mut known: BTreeMap<[u8; 32], Known> = list
            .into_iter()
            .map(|(hash, summary)| {
                let mut k = Known::new(hash);
                k.list = Some(summary);
                (hash, k)
            })
            .collect();
        for w in wallet_mns {
            let entry = known
                .entry(w.record.pro_tx_hash)
                .or_insert_with(|| Known::new(w.record.pro_tx_hash));
            // A masternode several wallets registered keeps the first
            // record; the others still count in owned detection below.
            if entry.wallet.is_none() {
                entry.wallet = Some(w);
            }
        }
        for t in tracked {
            let entry = known
                .entry(t.pro_tx_hash)
                .or_insert_with(|| Known::new(t.pro_tx_hash));
            entry.owned.insert(OwnedRole::Tracked);
            entry.tracked = Some(t);
        }

        self.detect_owned(&manager, network, &mut known).await;
        Ok(Snapshot {
            network,
            list_available,
            known,
        })
    }

    /// QT-120: marks every role a wallet holds a key or coin for.
    async fn detect_owned(
        &self,
        manager: &Manager,
        network: dashcore::Network,
        known: &mut BTreeMap<[u8; 32], Known>,
    ) {
        for raw in manager.list_wallet_ids_blocking() {
            let wallet_id = WalletId(raw);
            let Some(wallet) = manager.get_wallet(&raw).await else {
                continue;
            };
            let state = wallet.state().await;
            let info = &state.core_wallet;
            let owns = |hash: &[u8; 20]| crate::coins::wallet_owns(info, &p2pkh(hash, network));
            let owns_script = |script: &ScriptBuf| {
                Address::from_script(script, network)
                    .is_ok_and(|a| crate::coins::wallet_owns(info, &a))
            };
            let unspent = wallet_unspent(info);
            for k in known.values_mut() {
                let mut roles = BTreeSet::new();
                if k.collateral().is_some_and(|c| unspent.contains(&c))
                    || k.wallet
                        .as_ref()
                        .and_then(|w| w.collateral_script.as_ref())
                        .is_some_and(&owns_script)
                {
                    roles.insert(OwnedRole::Collateral);
                }
                if k.owner_key_hash().is_some_and(|h| owns(&h)) {
                    roles.insert(OwnedRole::Owner);
                }
                if k.voting_key_hash().is_some_and(|h| owns(&h)) {
                    roles.insert(OwnedRole::Voting);
                }
                if k.payout_script().is_some_and(|s| owns_script(&s)) {
                    roles.insert(OwnedRole::Payout);
                }
                if let Some(w) = &k.wallet {
                    if w.operator_payout
                        .as_ref()
                        .is_some_and(|s| !s.is_empty() && owns_script(s))
                    {
                        roles.insert(OwnedRole::OperatorPayout);
                    }
                    if w.wallet_id == wallet_id {
                        if w.record.operator_key_index.is_some() {
                            roles.insert(OwnedRole::Operator);
                        }
                        if w.record.platform_key_index.is_some() {
                            roles.insert(OwnedRole::PlatformNode);
                        }
                    }
                }
                if !roles.is_empty() {
                    k.owned.extend(roles.iter().copied());
                    k.owners.entry(wallet_id).or_default().extend(roles);
                }
            }
        }
    }

    /// QT-118 "Node Count" and list availability. In-memory read: the counts
    /// the session cached when the masternode phase last moved.
    pub fn masternode_list_state(&self) -> Result<MasternodeListState, EngineError> {
        let _op = self.try_enter()?;
        let tracker = self.hub.tracker();
        let snapshot = tracker.snapshot();
        let counts = tracker.masternode_counts();
        let height = tracker.masternode_list_height();
        drop(tracker);
        Ok(match counts {
            Some((mn, evo)) => MasternodeListState {
                available: true,
                height,
                total: mn.total + evo.total,
                enabled: mn.enabled + evo.enabled,
                evo_total: evo.total,
                evo_enabled: evo.enabled,
                syncing: !snapshot.caught_up,
            },
            None => MasternodeListState {
                syncing: !snapshot.caught_up,
                ..Default::default()
            },
        })
    }

    /// The filtered list (QT-118/119), sorted by proTxHash (display order).
    /// Works without a wallet; before the list synced it holds the wallets'
    /// and tracked masternodes with `Unknown` status.
    pub async fn masternodes(
        self: &Arc<Self>,
        query: MasternodeQuery,
    ) -> Result<Vec<MasternodeRow>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let snap = this.masternode_snapshot().await?;
            let mut rows: Vec<MasternodeRow> = snap
                .known
                .values()
                .map(|k| k.row(snap.network, snap.list_available))
                .filter(|r| matches(r, &query))
                .collect();
            rows.sort_by(|a, b| a.pro_tx_hash.cmp(&b.pro_tx_hash));
            Ok(rows)
        })
        .await
    }

    /// The details dialog (QT-122).
    pub async fn masternode_detail(
        self: &Arc<Self>,
        pro_tx_hash: String,
    ) -> Result<MasternodeDetail, EngineError> {
        let hash = parse_pro_tx_hash(&pro_tx_hash)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let snap = this.masternode_snapshot().await?;
            let known = snap
                .known
                .get(&hash)
                .ok_or(MasternodeFailure::NotFound(pro_tx_hash))?;
            Ok(known.detail(snap.network, snap.list_available))
        })
        .await
    }

    /// Announces a change of the list model (tracked masternodes, a new
    /// provider transaction).
    pub(crate) fn announce_masternodes(&self) {
        self.hub.emit(EngineEvent::Masternodes {
            network: self.network.clone(),
        });
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn summary(seed: u8, valid: bool, evo: bool) -> MasternodeListSummary {
        MasternodeListSummary {
            pro_tx_hash: [seed; 32],
            service_address: Some(format!("1.2.3.{seed}:19999").parse().unwrap()),
            platform_http_port: evo.then_some(22001),
            operator_public_key: [seed; 48],
            voting_key_id: [seed; 20],
            platform_node_id: evo.then_some([seed ^ 0xff; 20]),
            is_valid: valid,
            is_evonode: evo,
            has_extended_net_info: false,
        }
    }

    fn known(seed: u8, valid: bool, evo: bool) -> Known {
        let mut k = Known::new([seed; 32]);
        k.list = Some(summary(seed, valid, evo));
        k
    }

    #[test]
    fn test_QT_119_list_row_has_spv_fields_and_none_for_full_node_columns() {
        let row = known(7, true, false).row(dashcore::Network::Testnet, true);
        assert_eq!(row.service.as_deref(), Some("1.2.3.7:19999"));
        assert_eq!(
            row.status,
            MasternodeListStatus::Active { since_height: None }
        );
        assert_eq!(row.pose_score, None);
        assert_eq!(row.last_paid_height, None);
        assert_eq!(row.next_payment_height, None);
        assert_eq!(
            row.owner_address, None,
            "owner unknown without a wallet record"
        );
        assert!(row.voting_address.starts_with('y'));
        assert_eq!(row.operator_public_key.len(), 96);
        assert_eq!(row.pro_tx_hash, display_hex(&[7; 32]));
    }

    #[test]
    fn test_QT_118_filters_by_type_text_owned_and_banned() {
        let net = dashcore::Network::Testnet;
        let regular = known(1, true, false).row(net, true);
        let banned = known(2, false, false).row(net, true);
        let mut evo_k = known(3, true, true);
        evo_k.owned.insert(OwnedRole::Voting);
        let evo = evo_k.row(net, true);
        let q = |f: MasternodeTypeFilter| MasternodeQuery {
            type_filter: f,
            ..Default::default()
        };
        assert!(matches(&regular, &q(MasternodeTypeFilter::Regular)));
        assert!(!matches(&evo, &q(MasternodeTypeFilter::Regular)));
        assert!(matches(&evo, &q(MasternodeTypeFilter::Evo)));
        assert!(!matches(&evo, &q(MasternodeTypeFilter::Shared)));
        let hide = MasternodeQuery {
            hide_banned: true,
            ..Default::default()
        };
        assert!(!matches(&banned, &hide) && matches(&regular, &hide));
        let owned = MasternodeQuery {
            owned_only: true,
            ..Default::default()
        };
        assert!(matches(&evo, &owned) && !matches(&regular, &owned));
        let text = MasternodeQuery {
            text: Some("1.2.3.1:".into()),
            ..Default::default()
        };
        assert!(matches(&regular, &text) && !matches(&banned, &text));
        let upper_hash = MasternodeQuery {
            text: Some(regular.pro_tx_hash[..10].to_uppercase()),
            ..Default::default()
        };
        assert!(matches(&regular, &upper_hash));
    }

    #[test]
    fn test_QT_119_status_without_and_after_the_list() {
        let k = Known::new([9; 32]);
        assert_eq!(k.status(false), MasternodeListStatus::Unknown);
        assert_eq!(k.status(true), MasternodeListStatus::Retired);
        assert_eq!(
            known(9, false, false).status(true),
            MasternodeListStatus::Banned { since_height: None }
        );
    }

    #[test]
    fn test_QT_122_detail_lists_platform_addresses_of_evonodes() {
        let d = known(4, true, true).detail(dashcore::Network::Testnet, true);
        assert_eq!(d.network_addresses, vec!["1.2.3.4:19999".to_string()]);
        assert_eq!(
            d.platform_https_addresses,
            vec!["1.2.3.4:22001".to_string()]
        );
        assert!(
            d.platform_p2p_addresses.is_empty(),
            "the list has no P2P port"
        );
        assert_eq!(d.revocation_reason, None);
    }

    #[test]
    fn test_QT_120_pro_tx_hash_parses_display_order() {
        let shown = display_hex(
            &[1u8; 31]
                .iter()
                .copied()
                .chain([2])
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
        );
        let wire = parse_pro_tx_hash(&shown).unwrap();
        assert_eq!(wire[31], 2);
        assert!(parse_pro_tx_hash("xyz").is_err());
    }
}
