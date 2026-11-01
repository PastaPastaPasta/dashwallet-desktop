//! Platform addresses (release 0.1, lane E2; m4 §2.12): the balance of the
//! wallet's DIP-17 payment addresses as the library's sync loop leaves it,
//! their activity, and "Move to Dash wallet", which withdraws it to the
//! wallet's own Core receive address. No Platform receive address in 0.1:
//! the wallet shows and empties what it finds there, it does not ask for
//! more (RELEASE-PLAN §1).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::credits::WithdrawAmount;
use super::dashpay::DashPay;
use super::errors::CreditsError;
use super::flows::GrantRequest;
use super::platform_moves;

/// The wallet's Platform addresses (payment account 0), as the last sync
/// pass left them; no network read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAddressBalance {
    /// The addresses' credits together.
    pub credits: u64,
    /// What "Move to Dash wallet" with `All` would withdraw from them now:
    /// 0 when it would be refused. A remainder below the least withdrawal
    /// and its fee (an `All` move leaves the fee estimate's surplus), or
    /// below the input minimum, is in `credits` only.
    pub movable_credits: u64,
    /// `movable_credits` in whole duffs, rounded down: what the Home
    /// breakdown adds to the spendable total.
    pub duffs: u64,
    /// The funded addresses, the largest first.
    pub addresses: Vec<PlatformAddressRow>,
    /// Unix seconds of this session's last completed sync pass; `None`
    /// before one (the balance is then the one stored at the last run).
    pub synced_at: Option<u64>,
    /// A sync pass runs now.
    pub syncing: bool,
}

/// One funded Platform address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAddressRow {
    /// The address in the network's bech32m form (DIP-18).
    pub address: String,
    pub credits: u64,
}

/// Which way an activity row moved credits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformActivityKind {
    /// An address's balance rose (seen by a sync pass).
    Incoming,
    /// An address's balance fell for a reason this wallet did not record
    /// (another device spent it), seen by a sync pass.
    Outgoing,
    /// A "Move to Dash wallet" this wallet made.
    MoveToCore,
}

/// Where an activity row stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformActivityState {
    /// Seen on Platform: a balance change a sync proved, or a move whose
    /// inputs Platform spent.
    Done,
    /// A move recorded and signed whose outcome is not known yet; the next
    /// read asks Platform again. No time limit; never retried by the engine.
    Pending,
    /// A move that can never execute (not fully signed, or refused by
    /// Platform's structure or signature check), or one a proof after its
    /// call shows unexecuted: the latter may still execute, so a next move
    /// signs the same inputs at the same nonces (m4 §2.12).
    Failed,
}

/// One row of the Platform addresses' activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformActivity {
    /// Opaque, stable across reads (m4 §1 ids).
    pub id: String,
    pub kind: PlatformActivityKind,
    pub state: PlatformActivityState,
    /// The credits the change or the move took or added.
    pub credits: u64,
    /// The Platform address an `Incoming` or `Outgoing` row changed;
    /// `None` for a move (it may spend several).
    pub address: Option<String>,
    /// A move's Core address.
    pub to: Option<String>,
    /// A move's expected payout on Core, in duffs.
    pub expected_duffs: Option<u64>,
    /// A done move's fee, as the proved balances of its inputs show it
    /// (`None` before, or when another change to them hides it).
    pub fee_credits: Option<u64>,
    /// Unix seconds the engine recorded the row.
    pub at: u64,
}

/// What "Move to Dash wallet" takes and pays out, at the network's
/// protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveToCoreQuote {
    /// The credits the addresses may lose: the withdrawal and the fee
    /// estimate.
    pub credits: u64,
    /// The fee estimate: dpp's minimum fee for the inputs, kept on the fee
    /// source. Platform takes the actual fee from what that input keeps;
    /// nothing in the transition caps it (the pin has no max-fee field), so
    /// with an amount the fee may exceed it; the done move reports it.
    pub fee_credits: u64,
    /// What reaches the Core address, in duffs.
    pub expected_duffs: u64,
    /// The wallet's own Core receive address the move pays: pass it to
    /// `move_to_core`.
    pub to: String,
    /// What to authorize: no duffs, `credits` credits.
    pub grant: GrantRequest,
}

/// A move's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveToCoreOutcome {
    /// The move's activity row id.
    pub id: String,
    pub credits: u64,
    pub expected_duffs: u64,
    pub to: String,
    /// `Done` once a proved read shows every input at its signed nonce;
    /// `Pending` until then (the library's answer is no such proof).
    pub state: PlatformActivityState,
    /// Done: the fee taken, as [`PlatformActivity::fee_credits`].
    pub fee_credits: Option<u64>,
}

impl DashPay {
    /// The Platform addresses' balance as the sync loop left it.
    pub async fn platform_addresses(&self) -> Result<PlatformAddressBalance, CreditsError> {
        self.on_wallet(move |session, manager, wallet| async move {
            Ok(platform_moves::balance(&manager, &wallet, session.vault.network()).await)
        })
        .await
    }

    /// The Platform addresses' activity, the newest first: the balance
    /// changes the sync passes saw and the moves this wallet made. Settles
    /// a pending move first when Platform can tell.
    pub async fn platform_activity(&self) -> Result<Vec<PlatformActivity>, CreditsError> {
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            let net = platform_moves::SdkMoves {
                sdk: manager.sdk_arc(),
                wallet: Arc::clone(&wallet),
            };
            platform_moves::activity(&net, &session, &wallet, wallet_id).await
        })
        .await
    }

    /// Runs a sync pass of the Platform addresses now and waits for it.
    pub async fn sync_platform_addresses(&self) -> Result<PlatformAddressBalance, CreditsError> {
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            platform_moves::sync(&session, &manager, wallet_id).await?;
            Ok(platform_moves::balance(&manager, &wallet, session.vault.network()).await)
        })
        .await
    }

    /// What moving `amount` of the Platform addresses' credits to the
    /// wallet's Core receive address takes and pays out.
    pub async fn move_to_core_quote(
        &self,
        amount: WithdrawAmount,
    ) -> Result<MoveToCoreQuote, CreditsError> {
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            let net = platform_moves::SdkMoves {
                sdk: manager.sdk_arc(),
                wallet: Arc::clone(&wallet),
            };
            platform_moves::quote(&net, &session, &wallet, wallet_id, amount).await
        })
        .await
    }

    /// Moves `amount` of the Platform addresses' credits to `to` (the
    /// quote's: one of the wallet's Core receiving addresses) under `grant`
    /// (a `PlatformOp` grant or a `PlatformAddressMove` lease).
    pub async fn move_to_core(
        &self,
        amount: WithdrawAmount,
        to: String,
        grant: String,
    ) -> Result<MoveToCoreOutcome, CreditsError> {
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            let net = platform_moves::SdkMoves {
                sdk: manager.sdk_arc(),
                wallet: Arc::clone(&wallet),
            };
            let ask = (amount, to.as_str());
            platform_moves::move_to_core(&net, &session, &wallet, wallet_id, ask, &grant).await
        })
        .await
    }
}
