//! Core Max (ROUTE-05; C-routes D-C2): a shield or an "Add from Dash
//! wallet" of everything in BIP44 account 0, as one `DrainAll` asset lock
//! with the fee out of the lock, under the shield's or the fund's own
//! lease and step (`shield/<k>/funding`, `paddr/<k>/funding`), so their
//! pending listings, resumes and Repair treat it as any of theirs.
//!
//! The library's drain spends every final coin of the account (confirmed
//! or InstantSend-locked) and cannot be told to skip one, so Max refuses
//! while any such coin is held (a user's lock, a reservation, a
//! denomination Dash Core counts as mixed, which funding leaves alone), past
//! [`MAX_LOCK_INPUTS`] coins, and below the lock's floor, all before
//! signing. A coin not final yet stays behind (`arriving_duffs`), as do
//! the other accounts' coins (`left_duffs`). It takes the wallet's
//! CoinJoin claim (`CoinJoinRuntime::claim_sweep`) through the funding:
//! the mixer denominates BIP44 coins, so Max is refused while the wallet
//! mixes or another move of its coins runs, and no mixing starts
//! meanwhile. The vault cannot see the debit: the call's drain signer
//! (`shield_drain::drain_signer`) signs only the planned coins, within the
//! grant, and only a build that is exactly the plan; anything else is
//! `coins_changed`, unsigned.

use std::collections::HashMap;

use dashcore::OutPoint;
use key_wallet::bip32::DerivationPath;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::wallet::managed_wallet_info::asset_lock_builder::AssetLockFundingAccount;

use super::credits::{CoreMax, CoreMaxShort};
use super::errors::{CreditsError, PlatformError, ShieldedError};
use super::lease::{CallPermit, FundingKey};
use super::registration_flow::{FundError, PwLib};
use super::shield_drain::{DrainCoin, drain_refused, drain_signer};
use super::shielded_pool::MAX_LOCK_INPUTS;
use crate::coinjoin::{CoinJoinFailure, SweepClaim};
use crate::coins::{CoinFence, CoinSnapshot};
use crate::{NetworkSession, WalletId};

/// BIP44 account 0 as a Core Max reads it: its final coins, and what
/// stays behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CoreCoins {
    pub coins: Vec<DrainCoin>,
    pub left_duffs: u64,
    pub arriving_duffs: u64,
}

/// A Core Max's plan: the coins the lock spends, their sum, its fee bound
/// and the lock's value at least.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaxPlan {
    pub coins: Vec<DrainCoin>,
    pub total_duffs: u64,
    pub fee_duffs: u64,
    pub duffs: u64,
    pub max: CoreMax,
}

/// Why there is no Core Max.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaxRefusal {
    MixingActive,
    MoveRunning,
    CoinsLocked,
    TooManyInputs { inputs: u64, max: u64 },
    BelowMinimum { min: u64, max: CoreMaxShort },
}

impl From<MaxRefusal> for ShieldedError {
    fn from(r: MaxRefusal) -> Self {
        match r {
            MaxRefusal::MixingActive => Self::MixingActive,
            MaxRefusal::MoveRunning => Self::MoveRunning,
            MaxRefusal::CoinsLocked => Self::CoinsLocked,
            MaxRefusal::TooManyInputs { inputs, max } => Self::TooManyInputs { inputs, max },
            MaxRefusal::BelowMinimum { min, max } => Self::BelowMinimum {
                min,
                max: Some(max),
            },
        }
    }
}

impl From<MaxRefusal> for CreditsError {
    fn from(r: MaxRefusal) -> Self {
        match r {
            MaxRefusal::MixingActive => Self::MixingActive,
            MaxRefusal::MoveRunning => Self::MoveRunning,
            MaxRefusal::CoinsLocked => Self::CoinsLocked,
            MaxRefusal::TooManyInputs { inputs, max } => Self::TooManyInputs { inputs, max },
            MaxRefusal::BelowMinimum { min, max } => Self::BelowMinimum {
                min,
                max: Some(max),
            },
        }
    }
}

/// The wallet's CoinJoin claim, held from the plan through the funding.
pub(crate) fn claim(
    session: &NetworkSession,
    wallet: WalletId,
) -> Result<SweepClaim<'_>, MaxRefusal> {
    session.coinjoin.claim_sweep(wallet).map_err(|e| match e {
        CoinJoinFailure::MixingActive => MaxRefusal::MixingActive,
        _ => MaxRefusal::MoveRunning,
    })
}

/// The drain of `read` into a lock of at least `floor` duffs.
pub(crate) fn plan(read: CoreCoins, floor: u64) -> Result<MaxPlan, MaxRefusal> {
    if read.coins.iter().any(|c| c.held) {
        return Err(MaxRefusal::CoinsLocked);
    }
    let inputs = read.coins.len();
    if inputs > MAX_LOCK_INPUTS {
        return Err(MaxRefusal::TooManyInputs {
            inputs: inputs as u64,
            max: MAX_LOCK_INPUTS as u64,
        });
    }
    let total_duffs: u64 = read.coins.iter().map(|c| c.value).sum();
    let fee_duffs = crate::fees::asset_lock_fee_bound(inputs);
    let duffs = total_duffs.saturating_sub(fee_duffs);
    if duffs < floor {
        return Err(MaxRefusal::BelowMinimum {
            min: floor,
            max: CoreMaxShort {
                duffs,
                left_duffs: read.left_duffs,
            },
        });
    }
    Ok(MaxPlan {
        coins: read.coins,
        total_duffs,
        fee_duffs,
        duffs,
        max: CoreMax {
            inputs: inputs as u32,
            left_duffs: read.left_duffs,
            arriving_duffs: read.arriving_duffs,
        },
    })
}

/// [`CoreCoins`] of `snapshot`, `account` being BIP44 account 0's coins
/// with their address paths. A coin of the account the wallet cannot place
/// on a BIP44 address reads held: the drain signer could not sign it.
pub(crate) fn coins_of(
    snapshot: CoinSnapshot,
    account: &HashMap<OutPoint, Option<DerivationPath>>,
    network: dashcore::Network,
) -> CoreCoins {
    let height = snapshot.height;
    let mut out = CoreCoins::default();
    for c in snapshot.coins {
        let (utxo, value) = (&c.utxo, c.utxo.value());
        let Some(path) = account.get(&utxo.outpoint) else {
            if c.trusted_spendable(height) {
                out.left_duffs += value;
            }
            continue;
        };
        if !((utxo.is_confirmed || utxo.is_instantlocked) && utxo.is_mature(height)) {
            out.arriving_duffs += value;
            continue;
        }
        let path = (path.clone()).filter(|p| dw_vault::paths::is_bip44_address(p, network));
        out.coins.push(DrainCoin {
            outpoint: utxo.outpoint,
            value,
            fully_mixed: false,
            confirmed: true,
            held: c.user_locked
                || c.reserved
                || c.unresolved_lock
                || c.counted_denomination
                || path.is_none(),
            path: path.unwrap_or_default(),
        });
    }
    out
}

/// The fenced wallet's [`CoreCoins`], refused while a replay runs.
pub(crate) async fn read(
    session: &NetworkSession,
    fence: &CoinFence,
) -> Result<CoreCoins, PlatformError> {
    let id = fence.wallet_id();
    let read = async {
        let wallet = session.wallet(&id).await?;
        let snapshot = session.coin_snapshot(fence, &wallet).await?;
        let state = crate::wallet_read::info_of(&wallet).await?;
        let info = &state.core_wallet;
        let account: HashMap<_, _> = (info.accounts.standard_bip44_accounts.get(&0))
            .map(|a| {
                (a.utxos.values())
                    .map(|u| {
                        let path = crate::coins::own_address_info(info, &u.address);
                        (u.outpoint, path.map(|i| i.path))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok::<_, crate::EngineError>((snapshot, account))
    };
    let (snapshot, account) = session.replay_guarded(id, read).await?;
    Ok(coins_of(snapshot, &account, session.network.core_network()))
}

/// The library's `DrainAll` lock of BIP44 account 0 of `key`'s type,
/// signed by `permit`'s drain signer for `coins` worth `total_duffs`
/// crediting `credit_script`: built, tracked, broadcast and waited for its
/// proof. A build that is not the plan is refused before any signature.
pub(crate) async fn fund(
    pw: &PwLib,
    permit: &CallPermit,
    (coins, total_duffs): (&[DrainCoin], u64),
    minimum_lock_duffs: u64,
    key: FundingKey,
    credit_script: dashcore::ScriptBuf,
) -> Result<OutPoint, FundError> {
    let signer = drain_signer(
        permit,
        coins,
        total_duffs,
        minimum_lock_duffs,
        credit_script,
        true,
    )?;
    let account = AssetLockFundingAccount::Bip44 { account_index: 0 };
    let funded = (pw.fund_drain_with(account, key, minimum_lock_duffs, &signer)).await;
    if funded.is_err()
        && let Some(refusal) = signer.drain_refusal()
    {
        return Err(drain_refused(permit, refusal));
    }
    match funded {
        Err(_) if signer.capped() => Err(FundError::TooManyInputs {
            max: MAX_LOCK_INPUTS as u64,
        }),
        funded => funded,
    }
}

/// The script the library's next lock of `funding_type` (a shield's or a
/// fund's) credits: the next unused address of its funding account, as
/// platform-wallet's `peek_next_funding_address` picks it. Read before the
/// build, which holds the wallet's lock while it signs; the shield's or
/// the fund's lock keeps any other lock of the type out.
pub(crate) async fn credit_script(
    pw: &PwLib,
    funding_type: platform_wallet::AssetLockFundingType,
) -> Result<dashcore::ScriptBuf, FundError> {
    use platform_wallet::AssetLockFundingType as T;
    let shielded = match funding_type {
        T::AssetLockShieldedAddressTopUp => true,
        T::AssetLockAddressTopUp => false,
        other => {
            return Err(FundError::Refused(PlatformError::Internal {
                detail: format!("no drain funds a {other:?} lock"),
            }));
        }
    };
    let (xpub, account) = {
        let id = pw.wallet.wallet_id();
        let wm = pw.wallet.wallet_manager().read().await;
        let Some((wallet, info)) = wm.get_wallet_and_info(&id) else {
            return Err(FundError::Refused(PlatformError::WalletNotFound));
        };
        let (keys, managed) = (&wallet.accounts, &info.core_wallet.accounts);
        let (xpub, account) = match shielded {
            true => (
                keys.asset_lock_shielded_address_topup.as_ref(),
                managed.asset_lock_shielded_address_topup.clone(),
            ),
            false => (
                keys.asset_lock_address_topup.as_ref(),
                managed.asset_lock_address_topup.clone(),
            ),
        };
        (xpub.map(|a| a.account_xpub), account)
    };
    let unavailable = |detail: &str| {
        FundError::Refused(PlatformError::Internal {
            detail: format!("the {funding_type:?} funding key: {detail}"),
        })
    };
    let mut account = account.ok_or_else(|| unavailable("no funding account"))?;
    let address = account
        .next_address(xpub.as_ref(), false)
        .map_err(unavailable)?;
    Ok(address.script_pubkey())
}
