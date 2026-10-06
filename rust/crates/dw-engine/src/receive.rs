//! Receive addresses and payment requests (QT-081…085, QT-096,
//! IOS-053…055).
//!
//! Addresses come from the BIP44 account 0 address pools, which key-wallet
//! keeps filled to the gap limit past the highest used address. An address
//! counts as *issued* once it was handed out by `next_receive_address` or a
//! payment request: those are stored in dw-appdb's address book with purpose
//! `receive`, so issuance survives restarts and never repeats an address.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dashcore::Address;
use dw_appdb::{BookPurpose, LabelKind};
use dw_uri::core::{SendCoinsRecipient, format_bitcoin_uri};
use key_wallet::managed_account::address_pool::{AddressPool, AddressState};
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::managed_account_type::ManagedAccountType;

use crate::events::unix_now;
use crate::history::AddressChain;
use crate::{EngineError, NetworkSession, WalletId};

/// Dash Core `MAX_MONEY` (21 million DASH) in duffs.
const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInfo {
    pub address: String,
    pub chain: AddressChain,
    pub index: u32,
    /// e.g. `m/44'/1'/0'/0/7`.
    pub derivation_path: String,
    /// Has received funds at least once.
    pub used: bool,
    pub label: Option<String>,
    /// Current balance held by this address.
    pub balance: Option<u64>,
    /// Wallet transactions paying to or spending from this address.
    pub tx_count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressFilter {
    pub chain: Option<AddressChain>,
    pub used: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveRequest {
    pub id: u64,
    pub created_at: u64,
    pub address: String,
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
    pub uri: String,
}

/// One pool address with what the list needs.
#[derive(Debug, Clone)]
struct PoolAddress {
    address: Address,
    chain: AddressChain,
    index: u32,
    path: String,
    state: AddressState,
}

/// Snapshot of the BIP44 account 0 pools and per-address balances.
struct PoolView {
    addresses: Vec<PoolAddress>,
    balances: HashMap<Address, u64>,
}

fn pool_addresses(pool: &AddressPool, chain: AddressChain) -> Vec<PoolAddress> {
    pool.addresses
        .values()
        .map(|a| PoolAddress {
            address: a.address.clone(),
            chain,
            index: a.index,
            path: a.path.to_string(),
            state: a.state,
        })
        .collect()
}

/// Builds the `dash:` URI of a request (dash-qt `formatBitcoinURI`).
fn request_uri(
    address: &str,
    amount: Option<u64>,
    label: Option<&str>,
    message: Option<&str>,
) -> String {
    format_bitcoin_uri(&SendCoinsRecipient {
        address: address.to_string(),
        label: label.unwrap_or_default().to_string(),
        message: message.unwrap_or_default().to_string(),
        amount: amount.and_then(|a| i64::try_from(a).ok()).unwrap_or(0),
    })
}

impl NetworkSession {
    async fn pool_view(&self, wallet: WalletId) -> Result<PoolView, EngineError> {
        let manager = self.manager()?;
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm
            .get_wallet_info(&wallet.0)
            .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
        let account = info
            .core_wallet
            .accounts
            .standard_bip44_accounts
            .get(&0)
            .ok_or_else(|| EngineError::Wallet("wallet has no BIP44 account 0".into()))?;
        let mut addresses = Vec::new();
        if let ManagedAccountType::Standard {
            external_addresses,
            internal_addresses,
            ..
        } = account.managed_account_type()
        {
            addresses.extend(pool_addresses(external_addresses, AddressChain::Receiving));
            addresses.extend(pool_addresses(internal_addresses, AddressChain::Change));
        }
        let mut balances: HashMap<Address, u64> = HashMap::new();
        for utxo in account.utxos.values() {
            *balances.entry(utxo.address.clone()).or_default() += utxo.txout.value;
        }
        Ok(PoolView {
            addresses,
            balances,
        })
    }

    /// Txs per address (outputs paying it plus known inputs spending from it).
    fn tx_counts(&self, wallet: &WalletId) -> HashMap<String, u32> {
        let network = self.network.core_network();
        let mut counts: HashMap<String, u32> = HashMap::new();
        for entry in self.hub.history.snapshot(wallet).values() {
            let mut touched: HashSet<String> = entry
                .tx
                .output
                .iter()
                .filter_map(|o| Address::from_script(&o.script_pubkey, network).ok())
                .map(|a| a.to_string())
                .collect();
            touched.extend(entry.own_inputs.values().map(|(_, a)| a.to_string()));
            for a in touched {
                *counts.entry(a).or_default() += 1;
            }
        }
        counts
    }

    fn address_info(
        &self,
        a: &PoolAddress,
        view: &PoolView,
        labels: &HashMap<String, String>,
        counts: &HashMap<String, u32>,
    ) -> AddressInfo {
        let text = a.address.to_string();
        AddressInfo {
            label: labels.get(&text).cloned(),
            tx_count: counts.get(&text).copied().unwrap_or(0),
            balance: Some(view.balances.get(&a.address).copied().unwrap_or(0)),
            used: matches!(a.state, AddressState::Used),
            chain: a.chain,
            index: a.index,
            derivation_path: a.path.clone(),
            address: text,
        }
    }

    async fn address_labels(
        &self,
        wallet: WalletId,
    ) -> Result<HashMap<String, String>, EngineError> {
        let appdb = self.live()?.appdb;
        tokio::task::spawn_blocking(move || appdb.labels(&wallet.to_string(), LabelKind::Address))
            .await?
            .map(|v| v.into_iter().collect())
            .map_err(|e| EngineError::Storage(e.to_string()))
    }

    /// Addresses of the BIP44 account (QT-096), by chain then index.
    pub async fn addresses(
        self: &Arc<Self>,
        wallet: WalletId,
        filter: AddressFilter,
    ) -> Result<Vec<AddressInfo>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let view = this.pool_view(wallet).await?;
            let labels = this.address_labels(wallet).await?;
            let counts = this.tx_counts(&wallet);
            let mut out: Vec<AddressInfo> = view
                .addresses
                .iter()
                .map(|a| this.address_info(a, &view, &labels, &counts))
                .filter(|a| filter.chain.is_none_or(|c| c == a.chain))
                .filter(|a| filter.used.is_none_or(|u| u == a.used))
                .collect();
            out.sort_by_key(|a| (a.chain == AddressChain::Change, a.index));
            Ok(out)
        })
        .await
    }

    /// The first unused receiving address (IOS-053). Re-queried after
    /// `HistoryChanged`: once paid, the next one becomes current.
    pub async fn current_receive_address(
        self: &Arc<Self>,
        wallet: WalletId,
    ) -> Result<AddressInfo, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let view = this.pool_view(wallet).await?;
            let current = view
                .addresses
                .iter()
                .filter(|a| {
                    a.chain == AddressChain::Receiving && !matches!(a.state, AddressState::Used)
                })
                .min_by_key(|a| a.index)
                .ok_or(EngineError::GapLimit)?;
            let labels = this.address_labels(wallet).await?;
            Ok(this.address_info(current, &view, &labels, &this.tx_counts(&wallet)))
        })
        .await
    }

    /// Issues a fresh receiving address: unused, not reserved by
    /// key-wallet and never issued before. Stored as a `receive` address-book
    /// entry with `label` (dash-qt `getNewDestination(label)`, QT-081).
    /// `GapLimit` when every address inside the gap is already issued.
    pub async fn next_receive_address(
        self: &Arc<Self>,
        wallet: WalletId,
        label: Option<String>,
    ) -> Result<AddressInfo, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.issue_address(wallet, label).await
        })
        .await
    }

    async fn issue_address(
        &self,
        wallet: WalletId,
        label: Option<String>,
    ) -> Result<AddressInfo, EngineError> {
        let view = self.pool_view(wallet).await?;
        let appdb = self.live()?.appdb;
        let w = wallet.to_string();
        let issued: HashSet<String> = {
            let appdb = Arc::clone(&appdb);
            let w = w.clone();
            tokio::task::spawn_blocking(move || -> Result<HashSet<String>, dw_appdb::AppDbError> {
                let mut set: HashSet<String> = appdb
                    .address_book(&w, Some(BookPurpose::Receive))?
                    .into_iter()
                    .map(|e| e.address)
                    .collect();
                set.extend(appdb.receive_requests(&w)?.into_iter().map(|r| r.address));
                Ok(set)
            })
            .await?
            .map_err(|e| EngineError::Storage(e.to_string()))?
        };
        let fresh = view
            .addresses
            .iter()
            .filter(|a| {
                a.chain == AddressChain::Receiving
                    && matches!(a.state, AddressState::Available)
                    && !issued.contains(&a.address.to_string())
            })
            .min_by_key(|a| a.index)
            .ok_or(EngineError::GapLimit)?
            .clone();
        let text = fresh.address.to_string();
        let stored_label = label.clone().unwrap_or_default();
        let addr = text.clone();
        tokio::task::spawn_blocking(move || {
            appdb.upsert_book_entry(&w, &addr, BookPurpose::Receive, &stored_label, unix_now())
        })
        .await?
        .map_err(|e| EngineError::Storage(e.to_string()))?;
        let mut labels = HashMap::new();
        if let Some(l) = label.filter(|l| !l.is_empty()) {
            labels.insert(text, l);
        }
        Ok(self.address_info(&fresh, &view, &labels, &self.tx_counts(&wallet)))
    }

    /// Stores a payment request on a freshly issued address (QT-081/082).
    /// An amount of 0 means "any amount", as in dash-qt.
    pub async fn create_receive_request(
        self: &Arc<Self>,
        wallet: WalletId,
        amount: Option<u64>,
        label: Option<String>,
        message: Option<String>,
    ) -> Result<ReceiveRequest, EngineError> {
        let amount = amount.filter(|a| *a > 0);
        if amount.is_some_and(|a| a > MAX_MONEY) {
            return Err(EngineError::InvalidArgument(
                "amount above the maximum supply".into(),
            ));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let label = label.filter(|l| !l.is_empty());
            let message = message.filter(|m| !m.is_empty());
            let issued = this.issue_address(wallet, label.clone()).await?;
            let appdb = this.live()?.appdb;
            let created_at = unix_now();
            let (address, l, m) = (issued.address.clone(), label.clone(), message.clone());
            let id = tokio::task::spawn_blocking(move || {
                appdb.add_receive_request(
                    &wallet.to_string(),
                    created_at,
                    &address,
                    amount,
                    l.as_deref(),
                    m.as_deref(),
                )
            })
            .await?
            .map_err(|e| EngineError::Storage(e.to_string()))?;
            Ok(ReceiveRequest {
                id: u64::try_from(id)
                    .map_err(|_| EngineError::Internal(format!("request id {id}")))?,
                created_at,
                uri: request_uri(
                    &issued.address,
                    amount,
                    label.as_deref(),
                    message.as_deref(),
                ),
                address: issued.address,
                amount,
                label,
                message,
            })
        })
        .await
    }

    /// Stored requests, newest first (QT-083).
    pub async fn receive_requests(
        self: &Arc<Self>,
        wallet: WalletId,
    ) -> Result<Vec<ReceiveRequest>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.require_wallet(&wallet)?;
            let appdb = this.live()?.appdb;
            let rows =
                tokio::task::spawn_blocking(move || appdb.receive_requests(&wallet.to_string()))
                    .await?
                    .map_err(|e| EngineError::Storage(e.to_string()))?;
            Ok(rows
                .into_iter()
                .filter_map(|r| {
                    Some(ReceiveRequest {
                        id: u64::try_from(r.id).ok()?,
                        created_at: r.created_at,
                        uri: request_uri(
                            &r.address,
                            r.amount,
                            r.label.as_deref(),
                            r.message.as_deref(),
                        ),
                        address: r.address,
                        amount: r.amount,
                        label: r.label,
                        message: r.message,
                    })
                })
                .collect())
        })
        .await
    }

    pub async fn delete_receive_request(
        self: &Arc<Self>,
        wallet: WalletId,
        request_id: u64,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.require_wallet(&wallet)?;
            let id =
                i64::try_from(request_id).map_err(|_| EngineError::RequestNotFound(request_id))?;
            let appdb = this.live()?.appdb;
            let deleted = tokio::task::spawn_blocking(move || {
                appdb.delete_receive_request(&wallet.to_string(), id)
            })
            .await?
            .map_err(|e| EngineError::Storage(e.to_string()))?;
            if deleted {
                Ok(())
            } else {
                Err(EngineError::RequestNotFound(request_id))
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uri_matches_dash_qt_format() {
        assert_eq!(request_uri("yAddr", None, None, None), "dash:yAddr");
        assert_eq!(
            request_uri("yAddr", Some(150_000_000), Some("Rent"), Some("May")),
            "dash:yAddr?amount=1.50000000&label=Rent&message=May"
        );
    }
}
