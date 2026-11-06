//! "Move mixed coins" to Shielded: one Core → Shielded
//! asset lock that drains CoinJoin account 0, then its apply to the
//! wallet's own shielded address, as a shield's (`shielded_pool.rs`).
//!
//! The lock spends every coin of the account (the library's drain;
//! CoinJoin coins never pool with others, which would link them), so the
//! move is refused while the wallet mixes or another move of its coins
//! runs (the drain holds the wallet's claim, `CoinJoinRuntime::claim_sweep`,
//! through its funding), while any coin of the account is held (a "Lock
//! unspent" coin, a coin a sweep's record or another spend holds: the
//! library's selection cannot be told to skip one) or not confirmed yet
//! (a sweep's rule: SPV cannot see a mixing transaction's foreign
//! unconfirmed ancestors, so the mempool's ancestor limits cannot be
//! checked), past [`MAX_LOCK_INPUTS`] coins (combine them first: the
//! consolidation of `coinjoin.rs`), and while a shield is pending.
//!
//! Its grant is a `Spend` grant of at least the coins' sum, the lock's
//! debit: the fee comes out of the lock. Its lease (`FlowKind::ShieldDrain`)
//! issues the `ShieldDrainFunding` signer, which signs CoinJoin account 0's
//! inputs and the lock's credit key only and stops with the lease. The
//! vault cannot see what a signature spends, so the funding call's signer
//! signs only the planned coins, within the grant ([`drain_signer`]).
//!
//! The lock is a shield step's (`shield/<k>/funding`), with the shield's
//! funding key, so the dispatch journal, `pending_shields`,
//! `resume_shield` and the stuck-lock recovery treat it as any shield's.

use dashcore::OutPoint;
use dpp::balances::credits::CREDITS_PER_DUFF;
use dw_vault::GrantKind;
use key_wallet::bip32::DerivationPath;
use std::sync::Arc;

use super::errors::{PlatformError, ShieldedError};
use super::flows::{BudgetPurpose, FlowKind};
use super::lease::CallPermit;
use super::registration_flow::FundError;
use super::shielded::{MixedShieldQuote, ShieldOutcome};
use super::shielded_pool::{
    self as pool, MAX_LOCK_INPUTS, ShieldedLib, journal_unread, next_step, open_shields,
    priced_gate, shield_pool_fee, wallet_lock,
};
use super::signers::CountingFundingSigner;
use crate::coinjoin::CoinJoinFailure;
use crate::coins::CoinFence;
use crate::{NetworkSession, WalletId};

/// One coin of CoinJoin account 0 as the drain sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainCoin {
    pub outpoint: OutPoint,
    pub value: u64,
    /// Its address's key, `m/9'/coin'/4'/0'/{0,1}/i`.
    pub path: DerivationPath,
    pub fully_mixed: bool,
    /// In a block: what a move spends (an InstantSend lock is not
    /// enough, as for a sweep).
    pub confirmed: bool,
    /// Locked by the user, or reserved by a spend or a sweep the engine
    /// holds.
    pub held: bool,
}

/// The smallest lock a drain may build against a pool fee of `pool_fee`
/// credits: one that leaves a credit after the fee.
pub(super) fn drain_floor(pool_fee: u64) -> u64 {
    pool_fee / CREDITS_PER_DUFF + 1
}

/// The drain of `coins` against a pool fee of `pool_fee` credits: its
/// quote, or why there is none.
pub(super) fn plan_of(
    coins: &[DrainCoin],
    pool_fee: u64,
) -> Result<MixedShieldQuote, ShieldedError> {
    if coins.iter().any(|c| c.held) {
        return Err(ShieldedError::CoinsLocked);
    }
    let unconfirmed = coins.iter().filter(|c| !c.confirmed).count();
    if unconfirmed > 0 {
        return Err(ShieldedError::CoinsUnconfirmed {
            coins: unconfirmed as u64,
        });
    }
    if coins.is_empty() {
        return Err(ShieldedError::NothingToMove);
    }
    if coins.len() > MAX_LOCK_INPUTS {
        return Err(ShieldedError::TooManyInputs {
            inputs: coins.len() as u64,
            max: MAX_LOCK_INPUTS as u64,
        });
    }
    let total_duffs: u64 = coins.iter().map(|c| c.value).sum();
    let fully_mixed_duffs: u64 = (coins.iter().filter(|c| c.fully_mixed))
        .map(|c| c.value)
        .sum();
    let fee_duffs = crate::fees::asset_lock_fee_bound(coins.len());
    let duffs = total_duffs.saturating_sub(fee_duffs);
    let min = drain_floor(pool_fee);
    if duffs < min {
        return Err(ShieldedError::BelowMinimum { min });
    }
    Ok(MixedShieldQuote {
        inputs: coins.len() as u32,
        total_duffs,
        fee_duffs,
        duffs,
        pool_fee_credits: pool_fee,
        credits: duffs
            .saturating_mul(CREDITS_PER_DUFF)
            .saturating_sub(pool_fee),
        fully_mixed_duffs,
        not_fully_mixed_duffs: total_duffs - fully_mixed_duffs,
    })
}

/// The checks before a drain reads coins: the pool's gate, the journal,
/// mixing, a pending shield. Returns the pool fee.
async fn precheck(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl ShieldedLib,
) -> Result<u64, ShieldedError> {
    let version = priced_gate(lib).await?;
    if !session.leases.journal_ready() {
        return Err(journal_unread(session));
    }
    if session.mixing(wallet) {
        return Err(ShieldedError::MixingActive);
    }
    if let Some(open) = (open_shields(lib).await?.iter()).find(|o| !o.consumption_unknown) {
        return Err(ShieldedError::ShieldPending {
            txid: open.lock.outpoint.txid.to_string(),
        });
    }
    shield_pool_fee(version)
}

/// What draining the CoinJoin account into the pool would move. Reads
/// coins only.
pub(crate) async fn quote(
    session: &NetworkSession,
    wallet: WalletId,
    lib: &impl ShieldedLib,
) -> Result<MixedShieldQuote, ShieldedError> {
    let pool_fee = precheck(session, wallet, lib).await?;
    let fence = session
        .coin_fence(wallet)
        .await
        .map_err(PlatformError::from)?;
    let coins = lib.drain_coins(&fence).await?;
    plan_of(&coins, pool_fee)
}

/// The `Spend` budget `grant` gives a drain: a `ShieldDrain` lease's
/// remaining funding, or a vault grant's cap, read without redeeming it.
async fn drain_cap(
    session: &NetworkSession,
    wallet: WalletId,
    grant: &str,
) -> Result<u64, PlatformError> {
    if let Some(lease) = session.lease_for(&wallet, grant) {
        let lease = lease?;
        if lease.flow() != FlowKind::ShieldDrain {
            return Err(PlatformError::GrantInvalid);
        }
        return Ok(lease.available(BudgetPurpose::Funding)?);
    }
    let vault = session.vault.clone();
    let grant = grant.to_owned();
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&grant, GrantKind::Spend, Some(&wallet.0))
    })
    .await
    .map_err(|e| PlatformError::Internal {
        detail: e.to_string(),
    })?
    .map_err(PlatformError::from)?;
    match purpose {
        dw_vault::GrantPurpose::Spend { max_duffs } => Ok(max_duffs),
        _ => Err(PlatformError::GrantInvalid),
    }
}

/// The funding signer of a drain's call: `permit`'s, signing at most
/// [`MAX_LOCK_INPUTS`] inputs, and only `coins`, worth `total_duffs`.
pub(super) fn drain_signer(
    permit: &CallPermit,
    coins: &[DrainCoin],
    total_duffs: u64,
) -> Result<CountingFundingSigner, FundError> {
    Ok((permit.funding_signer())
        .map_err(|e| FundError::Refused(e.into()))?
        .with_input_cap(MAX_LOCK_INPUTS)
        .with_drain_cap(coins.iter().map(|c| (c.path.clone(), c.value)), total_duffs))
}

/// A drain's refusal of the wallet's claim on its CoinJoin account.
fn claim_refused(e: CoinJoinFailure) -> ShieldedError {
    match e {
        CoinJoinFailure::MixingActive => ShieldedError::MixingActive,
        CoinJoinFailure::MoveRunning => ShieldedError::MoveRunning,
        other => PlatformError::Internal {
            detail: other.to_string(),
        }
        .into(),
    }
}

/// Drains CoinJoin account 0 into one shield lock under a `ShieldDrain`
/// lease begun from `grant` (a `Spend` grant of at least the coins' sum),
/// then applies it, as `shield` does its lock.
pub(crate) async fn drain(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl ShieldedLib,
    grant: &str,
) -> Result<ShieldOutcome, ShieldedError> {
    pool::open_for_funding(session, wallet, lib).await?;
    let lock = wallet_lock(session, wallet);
    let _shielding = lock.lock().await;
    let pool_fee = precheck(session, wallet, lib).await?;
    if !session.leases.refresh_funding(wallet).await {
        return Err(PlatformError::Unavailable.into());
    }
    let step = next_step(session, wallet)?;
    // Through the funding's broadcast: no mixing starts, and no sweep,
    // consolidation or resend of the account runs.
    let claim = (session.coinjoin.claim_sweep(wallet)).map_err(claim_refused)?;
    // Held from the coin read to the permit, which keeps it until the
    // library tracks the lock: no engine spend takes a coin of the account
    // in between.
    let fence = session
        .coin_fence(wallet)
        .await
        .map_err(PlatformError::from)?;
    let coins = lib.drain_coins(&fence).await?;
    let quote = plan_of(&coins, pool_fee)?;
    let remaining = drain_cap(session, wallet, grant).await?;
    if quote.total_duffs > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Funding,
            needed: quote.total_duffs,
            remaining,
        }
        .into());
    }
    let lease = pool::authority(session, wallet, grant, FlowKind::ShieldDrain).await?;
    // Before the funding charges the budget (`topup.rs`); it also signs
    // the lock's credit key at the apply.
    let signer = lease
        .uncounted_funding_signer()
        .map_err(PlatformError::from)?;
    let funded = lease
        .scope_step(Some(step.clone()), async {
            let permit = (session.drain_call_permit(
                &lease,
                step.clone(),
                quote.total_duffs,
                step.clone(),
                pool::SHIELD_KEY,
                fence,
            ))
            .await
            .map_err(FundError::Refused)?;
            let floor = drain_floor(pool_fee);
            let funded = (lib.fund_drain(&coins, quote.total_duffs, floor, &permit)).await;
            permit.finish(funded.is_ok());
            funded
        })
        .await;
    drop(claim);
    pool::finish_lock(session, wallet, lib, &lease, &signer, step, funded).await
}

/// The coins of CoinJoin account 0 in `view` ([`NetworkSession::mix_view`]),
/// every one the library's drain would take. One the wallet cannot place
/// on the account's address paths reads held: the drain signer could not
/// sign it, so the plan refuses rather than leave it out. An immature
/// coinbase output reads unconfirmed: the library's drain skips it, and
/// it is final once mature.
pub(super) fn drain_coins_of(
    view: crate::coinjoin::MixView,
    network: dashcore::Network,
) -> Vec<DrainCoin> {
    let height = view.height;
    (view.coins.into_iter())
        .filter(|c| c.coin.coinjoin_account)
        .map(|c| {
            let path = c
                .path
                .filter(|p| dw_vault::paths::is_coinjoin_address(p, network));
            let utxo = &c.coin.utxo;
            DrainCoin {
                outpoint: utxo.outpoint,
                value: utxo.value(),
                fully_mixed: c.fully_mixed,
                confirmed: utxo.is_confirmed && utxo.is_mature(height),
                held: c.coin.user_locked || c.coin.reserved || path.is_none(),
                path: path.unwrap_or_default(),
            }
        })
        .collect()
}

/// [`ShieldedLib::drain_coins`] of the real library: the fenced wallet's
/// mix view, refused while a replay runs.
pub(crate) async fn read_drain_coins(
    session: &NetworkSession,
    fence: &CoinFence,
) -> Result<Vec<DrainCoin>, ShieldedError> {
    let wallet = fence.wallet_id();
    let view = (session.replay_guarded(wallet, session.mix_view(fence)))
        .await
        .map_err(PlatformError::from)?;
    Ok(drain_coins_of(view, session.network.core_network()))
}
