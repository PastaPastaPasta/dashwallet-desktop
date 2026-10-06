//! Session operations over the history read model: loading persisted
//! history at open, `history_page` and `tx_detail`.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{ScriptBuf, Txid};
use dw_appdb::{AppDb, LabelKind};
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::managed_account_type::ManagedAccountType;
use key_wallet::managed_account::managed_core_funds_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::transaction_record::TransactionRecord;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletInfo;
use platform_wallet::changeset::PlatformWalletPersistence;

use crate::history::{
    AddressChain, ChainView, HistoryPage, HistoryQuery, Labels, Owned, TxDetail, detail_of, page,
    records_of, validate_query,
};
use crate::session::{Manager, WALLET_DB_FILE};
use crate::{EngineError, NetworkSession, WalletId};

/// Every script the wallet's funds accounts own: BIP44 and BIP32 accounts,
/// DIP9 CoinJoin accounts and DashPay receiving accounts. Watch-only DashPay
/// contact accounts are not the wallet's.
pub(crate) fn owned_scripts(info: &PlatformWalletInfo) -> HashMap<ScriptBuf, Owned> {
    let accounts = &info.core_wallet.accounts;
    let mut map = HashMap::new();
    let mut add = |account: &ManagedCoreFundsAccount, coinjoin: bool| {
        let pools: Vec<(
            &key_wallet::managed_account::address_pool::AddressPool,
            AddressChain,
        )> = match account.managed_account_type() {
            ManagedAccountType::Standard {
                external_addresses,
                internal_addresses,
                ..
            }
            | ManagedAccountType::CoinJoin {
                external_addresses,
                internal_addresses,
                ..
            } => vec![
                (external_addresses, AddressChain::Receiving),
                (internal_addresses, AddressChain::Change),
            ],
            other => other
                .address_pools()
                .into_iter()
                .map(|p| (p, AddressChain::Receiving))
                .collect(),
        };
        for (pool, chain) in pools {
            for info in pool.addresses.values() {
                map.insert(
                    info.script_pubkey.clone(),
                    Owned {
                        address: info.address.clone(),
                        chain,
                        coinjoin,
                    },
                );
            }
        }
    };
    for a in accounts.standard_bip44_accounts.values() {
        add(a, false);
    }
    for a in accounts.standard_bip32_accounts.values() {
        add(a, false);
    }
    for a in accounts.coinjoin_accounts.values() {
        add(a, true);
    }
    for a in accounts.dashpay_receival_accounts.values() {
        add(a, false);
    }
    map
}

/// Txids of persisted transaction records per wallet, read from
/// `wallet.sqlite` over a read-only connection (SqlitePersister has no
/// enumeration API; `get_core_tx_record` then reads each record).
fn persisted_txids(db_path: &Path) -> Result<HashMap<[u8; 32], Vec<Txid>>, rusqlite::Error> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut stmt = conn
        .prepare("SELECT wallet_id, txid FROM core_transactions WHERE record_blob IS NOT NULL")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out: HashMap<[u8; 32], Vec<Txid>> = HashMap::new();
    for row in rows {
        let (wallet, txid) = row?;
        let (Ok(wallet), Ok(txid)) = (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid)) else {
            continue;
        };
        out.entry(wallet).or_default().push(txid);
    }
    Ok(out)
}

/// Persisted records, first-seen times and abandoned transactions of one
/// wallet.
struct LoadedHistory {
    wallet: WalletId,
    records: Vec<TransactionRecord>,
    seen: Vec<(String, u64)>,
    abandoned: Vec<(Txid, dashcore::Transaction)>,
}

impl NetworkSession {
    /// Fills the history store at open: persisted records first (they
    /// include chainlocked transactions platform-wallet no longer keeps in
    /// memory), then the in-memory records, then first-seen times.
    pub(crate) async fn load_history(&self) -> Result<(), EngineError> {
        let ids: Vec<WalletId> = self
            .manager()?
            .list_wallet_ids_blocking()
            .into_iter()
            .map(WalletId)
            .collect();
        self.load_history_for(ids).await
    }

    /// [`Self::load_history`] for some wallets (a wallet just loaded).
    pub(crate) async fn load_history_for(&self, ids: Vec<WalletId>) -> Result<(), EngineError> {
        let live = self.live()?;
        let db_path = self.data_dir().join(WALLET_DB_FILE);
        let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
        let wallet_ids = ids.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            let txids = persisted_txids(&db_path).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "could not list persisted transactions; history starts empty");
                HashMap::new()
            });
            wallet_ids
                .into_iter()
                .map(|wallet| {
                    let records = txids
                        .get(&wallet.0)
                        .into_iter()
                        .flatten()
                        .filter_map(|t| match persister.get_core_tx_record(wallet.0, t) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(%wallet, txid = %t, error = %e, "unreadable transaction record");
                                None
                            }
                        })
                        .collect();
                    let seen = appdb.tx_seen_times(&wallet.to_string()).unwrap_or_else(|e| {
                        tracing::warn!(%wallet, error = %e, "could not read first-seen times");
                        Vec::new()
                    });
                    let abandoned = crate::tx_actions::load_abandoned(&appdb, wallet);
                    LoadedHistory {
                        wallet,
                        records,
                        seen,
                        abandoned,
                    }
                })
                .collect::<Vec<_>>()
        })
        .await?;

        let wm = live.manager.wallet_manager_arc();
        let wm = wm.read().await;
        for LoadedHistory {
            wallet,
            records,
            seen,
            abandoned,
        } in loaded
        {
            let in_memory: Vec<TransactionRecord> = wm
                .get_wallet_info(&wallet.0)
                .map(|info| {
                    info.core_wallet
                        .transaction_history()
                        .into_iter()
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let seen: HashMap<String, u64> = seen.into_iter().collect();
            self.hub.history.with_wallet(wallet, |h| {
                for r in records.iter().chain(&in_memory) {
                    h.upsert(r, None);
                }
                // Abandoned transactions are gone from wallet.sqlite; the
                // app database keeps them so the history still shows them.
                for (txid, tx) in &abandoned {
                    h.txs
                        .entry(*txid)
                        .or_insert_with(|| crate::history::TxEntry {
                            tx: tx.clone(),
                            context: key_wallet::transaction_checking::TransactionContext::Mempool,
                            instant_locked: false,
                            first_seen: None,
                            own_inputs: Default::default(),
                            abandoned: true,
                        })
                        .abandoned = true;
                }
                for (txid, entry) in h.txs.iter_mut() {
                    if let Some(t) = seen.get(&txid.to_string()) {
                        entry.first_seen = Some(*t);
                    }
                }
            });
        }
        Ok(())
    }

    /// Transaction and address labels of a wallet (dw-appdb).
    pub(crate) async fn labels(
        appdb: &Arc<AppDb>,
        wallet: WalletId,
    ) -> Result<Labels, EngineError> {
        let appdb = Arc::clone(appdb);
        tokio::task::spawn_blocking(move || {
            let w = wallet.to_string();
            Ok::<_, dw_appdb::AppDbError>(Labels {
                tx: appdb.labels(&w, LabelKind::Tx)?.into_iter().collect(),
                address: appdb.labels(&w, LabelKind::Address)?.into_iter().collect(),
            })
        })
        .await?
        .map_err(|e| EngineError::Storage(e.to_string()))
    }

    /// Runs `f` with the wallet's chain view. `WalletNotFound` for an
    /// unknown wallet.
    pub(crate) async fn with_view<R>(
        &self,
        manager: &Manager,
        wallet: WalletId,
        f: impl FnOnce(&ChainView<'_>) -> R,
    ) -> Result<R, EngineError> {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm
            .get_wallet_info(&wallet.0)
            .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
        let owned = owned_scripts(info);
        let meta = &info.core_wallet.metadata;
        let tip = meta
            .last_processed_height
            .max(meta.synced_height)
            .max(self.hub.wallet_state(&wallet).map_or(0, |s| s.tip()));
        let chainlock_height = meta
            .last_applied_chain_lock
            .as_ref()
            .map(|c| c.block_height)
            .max(self.hub.tracker().chainlock_height());
        drop(wm);
        let history = self.hub.history.snapshot(&wallet);
        let view = ChainView {
            owned: &owned,
            history: &history,
            tip,
            chainlock_height,
            network: self.network.core_network(),
        };
        Ok(f(&view))
    }

    /// One page of dash-qt history records (QT-086…089, IOS-027/028).
    pub async fn history_page(
        self: &Arc<Self>,
        wallet: WalletId,
        query: HistoryQuery,
    ) -> Result<HistoryPage, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            validate_query(&query)?;
            let records = this.wallet_records(wallet).await?;
            page(records, &query)
        })
        .await
    }

    /// Every dash-qt record of a wallet, unsorted. Records of a watch-only
    /// wallet are marked `involves_watch_only` (m2-engine.md §5). Caller
    /// holds the operation guard.
    pub(crate) async fn wallet_records(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<crate::TxRecord>, EngineError> {
        let live = self.live()?;
        let labels = Self::labels(&live.appdb, wallet).await?;
        let watch_only = !self.vault.has_wallet_secret(&wallet.0);
        let mut records = self
            .with_view(&live.manager, wallet, |view| {
                view.history
                    .iter()
                    .flat_map(|(txid, entry)| records_of(txid, entry, view, &labels).0)
                    .collect::<Vec<_>>()
            })
            .await?;
        if watch_only {
            for r in &mut records {
                r.involves_watch_only = true;
            }
        }
        Ok(records)
    }

    /// The transaction details dialog (QT-092, IOS-031). `txid` is display
    /// hex.
    pub async fn tx_detail(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: String,
    ) -> Result<TxDetail, EngineError> {
        let parsed = parse_txid(&txid)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let labels = Self::labels(&live.appdb, wallet).await?;
            let appdb = Arc::clone(&live.appdb);
            let message = tokio::task::spawn_blocking(move || {
                appdb.tx_message(&wallet.to_string(), &parsed.to_string())
            })
            .await?
            .map_err(|e| EngineError::Storage(e.to_string()))?;
            let watch_only = !this.vault.has_wallet_secret(&wallet.0);
            let mut detail = this
                .with_view(&live.manager, wallet, |view| {
                    view.history
                        .get(&parsed)
                        .map(|entry| detail_of(&parsed, entry, view, &labels, message))
                })
                .await?
                .ok_or(EngineError::TxNotFound(txid))?;
            for r in &mut detail.records {
                r.involves_watch_only = watch_only;
            }
            Ok(detail)
        })
        .await
    }
}

/// Parses a display-order txid: 64 lower-case hex characters.
pub(crate) fn parse_txid(s: &str) -> Result<Txid, EngineError> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(EngineError::InvalidArgument(
            "txid must be 64 lower-case hex characters".into(),
        ));
    }
    Txid::from_str(s).map_err(|e| EngineError::InvalidArgument(format!("txid: {e}")))
}
