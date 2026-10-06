//! Coin control: the UTXO list with labels, locks and confirmations, user
//! locks persisted in dw-appdb, and dash-qt's dust attack protection
//! (QT-068…075; docs/contracts/m1-engine.md §2.8).
//!
//! Locks are app metadata: key-wallet's own `Utxo::is_locked` flag is not
//! persisted, so the engine keeps "Lock unspent" and dust locks in
//! `app.sqlite` and applies them when it lists coins and builds candidate
//! sets for a payment.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashcore::{Address, OutPoint, Txid};
use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE, LabelKind};
use key_wallet::Utxo;
use key_wallet::bip32::ChildNumber;
use key_wallet::managed_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::transaction_record::TransactionDirection;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use crate::{EngineError, NetworkSession, WalletId};

/// Settings key of the dust protection threshold (duffs; absent = off).
pub const DUST_PROTECTION_KEY: &str = "dust_protection_threshold";
/// dash-qt's range for the threshold (QT-075).
pub const DUST_PROTECTION_MAX: u64 = 1_000_000;
/// CoinJoin denominations in duffs (Dash Core `CoinJoin::GetStandardDenominations`).
pub const COINJOIN_DENOMINATIONS: [u64; 5] =
    [1_000_010_000, 100_001_000, 10_000_100, 1_000_010, 100_001];

/// Wall clock in UNIX seconds.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether `address` is one of the wallet's derived addresses.
pub(crate) fn wallet_owns(info: &ManagedWalletInfo, address: &Address) -> bool {
    info.all_managed_accounts()
        .into_iter()
        .any(|account| account.get_address_info(address).is_some())
}

/// One unspent output of the wallet with what coin control shows about it.
#[derive(Debug, Clone)]
pub(crate) struct WalletCoin {
    pub utxo: Utxo,
    /// Held by an account a plain send funds from (BIP44, BIP32, DashPay
    /// receiving), as opposed to CoinJoin.
    pub send_account: bool,
    pub coinjoin_account: bool,
    /// On an internal (change) chain.
    pub is_change: bool,
    pub block_time: Option<u64>,
    pub chain_locked: bool,
    /// The funding transaction spent none of the wallet's coins (or, when
    /// key-wallet no longer has the record, the coin is not change).
    pub foreign_incoming: bool,
    pub user_locked: bool,
    pub reserved: bool,
}

impl WalletCoin {
    /// Dash Core `IsTrusted`: confirmed, InstantSend-locked, or our own
    /// unconfirmed change.
    fn trusted(&self) -> bool {
        self.utxo.is_confirmed || self.utxo.is_instantlocked || self.utxo.is_trusted
    }

    /// Eligible for automatic coin selection.
    pub(crate) fn auto_selectable(&self, height: u32) -> bool {
        self.utxo.is_spendable(height) && self.trusted() && !self.user_locked && !self.reserved
    }

    /// Eligible when picked by hand (coin control may spend untrusted
    /// unconfirmed coins, never locked, reserved or immature ones).
    pub(crate) fn chosen_selectable(&self, height: u32) -> bool {
        self.utxo.is_spendable(height) && !self.user_locked && !self.reserved
    }

    fn denominated(&self) -> bool {
        self.coinjoin_account && COINJOIN_DENOMINATIONS.contains(&self.utxo.value())
    }
}

/// Every unspent output of a wallet at its processed height.
#[derive(Debug, Clone)]
pub(crate) struct CoinSnapshot {
    pub height: u32,
    pub coins: Vec<WalletCoin>,
}

/// UTXO list filter (contract `UtxoFilter`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoinFilter {
    pub include_locked: bool,
    pub fully_mixed_only: bool,
    pub min_confirmations: Option<u32>,
}

/// One row of the coin control list (contract `Utxo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinInfo {
    pub outpoint: OutPoint,
    pub address: String,
    pub amount: u64,
    pub confirmations: u32,
    pub block_height: Option<u32>,
    /// Block time; `None` while unconfirmed or once the record was pruned.
    pub timestamp: Option<u64>,
    pub instant_locked: bool,
    pub chain_locked: bool,
    pub user_locked: bool,
    pub reserved: bool,
    pub label: Option<String>,
    pub is_change: bool,
    pub is_coinbase: bool,
    pub coinjoin_denominated: bool,
    /// Mixing rounds are not tracked yet: always `None` (unknown).
    pub coinjoin_rounds: Option<u32>,
    pub spendable: bool,
}

/// `(txid display hex, vout)`, the dw-appdb key of an outpoint.
fn lock_key(o: &OutPoint) -> (String, u32) {
    (o.txid.to_string(), o.vout)
}

fn is_internal_path(path: &key_wallet::DerivationPath) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() >= 2 && p[p.len() - 2] == ChildNumber::Normal { index: 1 }
}

fn collect(
    coins: &mut Vec<WalletCoin>,
    account: &ManagedCoreFundsAccount,
    send_account: bool,
    coinjoin_account: bool,
) {
    for utxo in account.utxos.values() {
        let txid = utxo.outpoint.txid;
        let record = account.transactions().get(&txid);
        let is_change = !coinjoin_account
            && account
                .get_address_info(&utxo.address)
                .is_some_and(|info| is_internal_path(&info.path));
        coins.push(WalletCoin {
            utxo: utxo.clone(),
            send_account,
            coinjoin_account,
            is_change,
            block_time: record
                .and_then(|r| r.context.block_info())
                .map(|b| u64::from(b.timestamp())),
            chain_locked: record.is_some_and(|r| r.context.is_chain_locked())
                || account.transaction_is_finalized(&txid),
            // key-wallet drops the record of a ChainLocked transaction
            // (only its txid survives), so the direction is often gone by
            // the time coins are read. Without it, a coin on a receiving
            // (non-change) address counts as foreign: dust protection then
            // locks it, and the user can unlock it.
            foreign_incoming: record.map_or(!is_change, |r| {
                r.direction == TransactionDirection::Incoming
            }),
            user_locked: false,
            reserved: false,
        });
    }
}

/// Reads every funds account of `info` (key-wallet state only).
fn read_coins(info: &ManagedWalletInfo) -> CoinSnapshot {
    let accounts = &info.accounts;
    let mut coins = Vec::new();
    for a in accounts.standard_bip44_accounts.values() {
        collect(&mut coins, a, true, false);
    }
    for a in accounts.standard_bip32_accounts.values() {
        collect(&mut coins, a, true, false);
    }
    for a in accounts.dashpay_receival_accounts.values() {
        collect(&mut coins, a, true, false);
    }
    for a in accounts.coinjoin_accounts.values() {
        collect(&mut coins, a, false, true);
    }
    CoinSnapshot {
        height: info.last_processed_height(),
        coins,
    }
}

fn parse_threshold(value: Option<String>) -> Result<Option<u64>, AppDbError> {
    value
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| AppDbError::Corrupt(format!("{DUST_PROTECTION_KEY} {v:?}")))
        })
        .transpose()
}

impl NetworkSession {
    /// Runs a dw-appdb call on the blocking pool.
    pub(crate) async fn appdb_op<T, F>(&self, f: F) -> Result<T, EngineError>
    where
        F: FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.live()?.appdb;
        tokio::task::spawn_blocking(move || f(&db))
            .await?
            .map_err(EngineError::from)
    }

    /// The wallet's coins with user/dust locks and reservations applied.
    /// With dust protection on, first dust-locks small foreign incoming
    /// coins (Dash Core: value ≤ threshold, funding transaction not ours;
    /// a released dust lock is not renewed).
    pub(crate) async fn coin_snapshot(
        &self,
        wallet: &PlatformWallet,
        wallet_id: WalletId,
    ) -> Result<CoinSnapshot, EngineError> {
        let mut snapshot = {
            let state = wallet.state().await;
            read_coins(&state.core_wallet)
        };
        let id = wallet_id.to_string();
        let dust_candidates: Vec<(u64, String, u32)> = snapshot
            .coins
            .iter()
            .filter(|c| c.foreign_incoming && !c.is_change && !c.utxo.is_coinbase)
            .map(|c| {
                let (txid, vout) = lock_key(&c.utxo.outpoint);
                (c.utxo.value(), txid, vout)
            })
            .collect();
        let locked: HashSet<(String, u32)> = self
            .appdb_op(move |db| {
                if let Some(threshold) =
                    parse_threshold(db.setting(GLOBAL_SCOPE, DUST_PROTECTION_KEY)?)?
                {
                    let now = now_secs();
                    for (value, txid, vout) in &dust_candidates {
                        if *value <= threshold {
                            db.lock_dust(&id, txid, *vout, now)?;
                        }
                    }
                }
                Ok(db
                    .locks(&id)?
                    .into_iter()
                    .filter(|row| row.is_locked())
                    .map(|row| (row.txid, row.vout))
                    .collect())
            })
            .await?;
        let unspent: HashSet<OutPoint> = snapshot.coins.iter().map(|c| c.utxo.outpoint).collect();
        self.spends.retain_unspent(&wallet_id, &unspent);
        let reserved = self.spends.snapshot(&wallet_id);
        for coin in &mut snapshot.coins {
            coin.user_locked = locked.contains(&lock_key(&coin.utxo.outpoint));
            coin.reserved = reserved.contains(&coin.utxo.outpoint);
        }
        Ok(snapshot)
    }

    /// Coin control list of `wallet_id`, largest first (dash-qt's default
    /// sort). `fully_mixed_only` needs CoinJoin rounds, which are not
    /// tracked yet: `NotImplemented`.
    pub async fn utxos(
        self: &Arc<Self>,
        wallet_id: WalletId,
        filter: CoinFilter,
    ) -> Result<Vec<CoinInfo>, EngineError> {
        if filter.fully_mixed_only {
            return Err(EngineError::NotImplemented(
                "NetworkSession.utxos(fully_mixed_only): CoinJoin rounds are not tracked yet"
                    .into(),
            ));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let wallet = this.wallet(&wallet_id).await?;
            let snapshot = this.coin_snapshot(&wallet, wallet_id).await?;
            let has_keys = this.vault.has_wallet_secret(&wallet_id.0);
            let id = wallet_id.to_string();
            let labels: HashMap<String, String> = this
                .appdb_op(move |db| Ok(db.labels(&id, LabelKind::Address)?.into_iter().collect()))
                .await?;
            let height = snapshot.height;
            let mut rows: Vec<CoinInfo> = snapshot
                .coins
                .iter()
                .filter(|c| filter.include_locked || !c.user_locked)
                .map(|c| {
                    let u = &c.utxo;
                    let address = u.address.to_string();
                    CoinInfo {
                        outpoint: u.outpoint,
                        label: labels.get(&address).cloned(),
                        address,
                        amount: u.value(),
                        confirmations: u.confirmations(height),
                        block_height: u.is_confirmed.then_some(u.height),
                        timestamp: c.block_time,
                        instant_locked: u.is_instantlocked,
                        chain_locked: c.chain_locked,
                        user_locked: c.user_locked,
                        reserved: c.reserved,
                        is_change: c.is_change,
                        is_coinbase: u.is_coinbase,
                        coinjoin_denominated: c.denominated(),
                        coinjoin_rounds: None,
                        spendable: has_keys && c.chosen_selectable(height),
                    }
                })
                .filter(|row| {
                    filter
                        .min_confirmations
                        .is_none_or(|n| row.confirmations >= n)
                })
                .collect();
            rows.sort_by(|a, b| b.amount.cmp(&a.amount).then(a.outpoint.cmp(&b.outpoint)));
            Ok(rows)
        })
        .await
    }

    /// Checks that every outpoint is an unspent output of the wallet.
    async fn require_unspent(
        &self,
        wallet_id: WalletId,
        outpoints: &[OutPoint],
    ) -> Result<(), EngineError> {
        let wallet = self.wallet(&wallet_id).await?;
        let unspent: HashSet<OutPoint> = {
            let state = wallet.state().await;
            read_coins(&state.core_wallet)
                .coins
                .into_iter()
                .map(|c| c.utxo.outpoint)
                .collect()
        };
        match outpoints.iter().find(|o| !unspent.contains(o)) {
            Some(o) => Err(EngineError::OutpointNotFound(*o)),
            None => Ok(()),
        }
    }

    /// "Lock unspent" (QT-070): excludes the outpoints from automatic
    /// selection and coin control until unlocked. Persisted.
    pub async fn lock_outpoints(
        self: &Arc<Self>,
        wallet_id: WalletId,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.require_unspent(wallet_id, &outpoints).await?;
            let id = wallet_id.to_string();
            this.appdb_op(move |db| {
                let now = now_secs();
                for o in &outpoints {
                    let (txid, vout) = lock_key(o);
                    db.lock_manual(&id, &txid, vout, now)?;
                }
                Ok(())
            })
            .await
        })
        .await
    }

    /// "Unlock unspent": deletes a user lock or releases a dust lock (which
    /// dust protection then leaves alone). An outpoint that is neither
    /// locked nor an unspent output is `OutpointNotFound`.
    pub async fn unlock_outpoints(
        self: &Arc<Self>,
        wallet_id: WalletId,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let id = wallet_id.to_string();
            let keys: Vec<(String, u32)> = outpoints.iter().map(lock_key).collect();
            let active: HashSet<(String, u32)> = this
                .appdb_op({
                    let id = id.clone();
                    move |db| {
                        Ok(db
                            .locks(&id)?
                            .into_iter()
                            .filter(|r| r.is_locked())
                            .map(|r| (r.txid, r.vout))
                            .collect())
                    }
                })
                .await?;
            let unlisted: Vec<OutPoint> = outpoints
                .iter()
                .zip(&keys)
                .filter(|(_, k)| !active.contains(*k))
                .map(|(o, _)| *o)
                .collect();
            this.require_unspent(wallet_id, &unlisted).await?;
            this.appdb_op(move |db| {
                let now = now_secs();
                for (txid, vout) in &keys {
                    db.unlock(&id, txid, *vout, now)?;
                }
                Ok(())
            })
            .await
        })
        .await
    }

    /// Outpoints currently locked (user or dust), oldest first.
    pub async fn locked_outpoints(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<Vec<OutPoint>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet_id).await?;
            let id = wallet_id.to_string();
            let rows = this.appdb_op(move |db| db.locks(&id)).await?;
            rows.into_iter()
                .filter(|r| r.is_locked())
                .map(|r| {
                    Txid::from_str(&r.txid)
                        .map(|txid| OutPoint::new(txid, r.vout))
                        .map_err(|_| EngineError::Storage(format!("bad lock txid {:?}", r.txid)))
                })
                .collect()
        })
        .await
    }

    /// Dust attack protection threshold in duffs (`None` = off; QT-075).
    pub async fn dust_protection(self: &Arc<Self>) -> Result<Option<u64>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.manager()?;
            this.appdb_op(|db| parse_threshold(db.setting(GLOBAL_SCOPE, DUST_PROTECTION_KEY)?))
                .await
        })
        .await
    }

    /// Turns dust protection on (1…1,000,000 duffs) or off. Coins are locked
    /// the next time the wallet's coins are read.
    pub async fn set_dust_protection(
        self: &Arc<Self>,
        threshold: Option<u64>,
    ) -> Result<(), EngineError> {
        if let Some(t) = threshold
            && !(1..=DUST_PROTECTION_MAX).contains(&t)
        {
            return Err(EngineError::InvalidArgument(format!(
                "dust threshold {t} is outside 1..={DUST_PROTECTION_MAX}"
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.manager()?;
            this.appdb_op(move |db| {
                db.set_setting(
                    GLOBAL_SCOPE,
                    DUST_PROTECTION_KEY,
                    threshold.map(|t| t.to_string()).as_deref(),
                )
            })
            .await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_chain_detection() {
        let change = key_wallet::DerivationPath::from_str("m/44'/1'/0'/1/7").unwrap();
        let receive = key_wallet::DerivationPath::from_str("m/44'/1'/0'/0/7").unwrap();
        assert!(is_internal_path(&change));
        assert!(!is_internal_path(&receive));
    }

    #[test]
    fn threshold_parsing() {
        assert_eq!(parse_threshold(None).unwrap(), None);
        assert_eq!(parse_threshold(Some("10000".into())).unwrap(), Some(10_000));
        assert!(parse_threshold(Some("x".into())).is_err());
    }
}
