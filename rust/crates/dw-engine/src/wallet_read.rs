//! A wallet's info read under the manager's lock, failing with
//! `WalletNotFound` once the wallet left the manager, where
//! `PlatformWallet::state` and `state_mut` panic. The guard is held for the
//! span `state()`'s was: across an await only where that span already was
//! (`replay_guard`).

use std::sync::Arc;

use platform_wallet::PlatformWalletInfo;
use platform_wallet::key_wallet_manager::WalletManager;
use platform_wallet::wallet::platform_wallet::PlatformWallet;
use tokio::sync::{OwnedRwLockMappedWriteGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard};

use crate::EngineError;

pub(crate) type InfoRead =
    OwnedRwLockReadGuard<WalletManager<PlatformWalletInfo>, PlatformWalletInfo>;

pub(crate) type InfoWrite =
    OwnedRwLockMappedWriteGuard<WalletManager<PlatformWalletInfo>, PlatformWalletInfo>;

fn gone(wallet: &PlatformWallet) -> EngineError {
    EngineError::WalletNotFound(hex::encode(wallet.wallet_id()))
}

pub(crate) async fn info_of(wallet: &PlatformWallet) -> Result<InfoRead, EngineError> {
    let id = wallet.wallet_id();
    let wm = Arc::clone(wallet.wallet_manager()).read_owned().await;
    OwnedRwLockReadGuard::try_map(wm, |wm| wm.get_wallet_info(&id)).map_err(|_| gone(wallet))
}

pub(crate) async fn info_mut_of(wallet: &PlatformWallet) -> Result<InfoWrite, EngineError> {
    let id = wallet.wallet_id();
    let wm = Arc::clone(wallet.wallet_manager()).write_owned().await;
    OwnedRwLockWriteGuard::try_map(wm, |wm| wm.get_wallet_info_mut(&id)).map_err(|_| gone(wallet))
}

#[cfg(test)]
#[path = "wallet_gone_tests.rs"]
pub(crate) mod gone_tests;
