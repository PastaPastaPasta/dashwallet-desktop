//! Platform addresses (release 0.1, lane E2; 0.2 X1b; m4 §2.12): the
//! balance of the wallet's DIP-17 payment addresses as the library's sync
//! loop leaves it, their activity, "Move to Dash wallet", which withdraws it
//! to the wallet's own Core receive address, the receive address (0.2) and
//! a send to another Platform address (0.2).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::credits::WithdrawAmount;
use super::dashpay::DashPay;
use super::errors::{CreditsError, PlatformError};
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
    /// A send of this wallet to a Platform address (`to`).
    Send,
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
    /// A move whose inputs, at the nonces it signed, another move of this
    /// wallet with other terms also signed, and Platform took: one of them
    /// executed and no proof says which, so the newest carries the group's
    /// outcome (`Done`, the proved debit, payout and fee unattributed) and
    /// the others read as replaced by it (m4 §7 item 69). A send whose
    /// identical retry executed in its place (0.2).
    Replaced,
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
    /// A move's Core address, a send's Platform address.
    pub to: Option<String>,
    /// A move's expected payout on Core, in duffs.
    pub expected_duffs: Option<u64>,
    /// A done move's fee, as the proved balances of its inputs show it
    /// (`None` before, or when another change to them hides it).
    pub fee_credits: Option<u64>,
    /// Unix seconds the engine recorded the row.
    pub at: u64,
    /// A move or send that may still execute (`Pending`, or `Failed` by
    /// height): until its nonces are taken it holds the next one, and a
    /// `Failed` one "may still go through". A `Failed` row without it never
    /// executes; a send superseded by its identical retry is `Replaced`.
    #[serde(default)]
    pub may_land: bool,
    /// A done send whose fee the balances did not show: `credits` counts
    /// dpp's estimate for it.
    #[serde(default)]
    pub estimated: bool,
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
    /// What the move may take; for a group's `Done`, the proved debit.
    pub credits: u64,
    /// The payout; `None` when a group's `Done` cannot attribute it.
    pub expected_duffs: Option<u64>,
    /// The Core address paid; for a group's `Done`, `None` unless every
    /// member named the same one (no evidence says which executed).
    pub to: Option<String>,
    /// `Done` once a proved read shows every input at its signed nonce;
    /// `Pending` until then (the library's answer is no such proof);
    /// `Replaced` when another move of the group carries the outcome.
    pub state: PlatformActivityState,
    /// Done: the fee taken, as [`PlatformActivity::fee_credits`].
    pub fee_credits: Option<u64>,
}

/// Why a string is no Platform address a send may pay
/// (`credits.recipient_invalid{reason}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipientProblem {
    /// A Dash Core address: Pay sends to it from the Core balance.
    Core,
    /// A shielded address.
    Shielded,
    /// A Platform (or Core, or shielded) address of another network.
    Network,
    /// One of the wallet's own Platform addresses.
    Own,
    /// Not an address.
    Malformed,
}

impl RecipientProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Shielded => "shielded",
            Self::Network => "network",
            Self::Own => "own",
            Self::Malformed => "malformed",
        }
    }
}

/// The wallet's Platform receive address: where another Platform wallet
/// pays it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformReceiveAddress {
    /// bech32m (DIP-18), the network's HRP; shown and encoded in a QR code
    /// as it is (no `dash:` URI carries one).
    pub address: String,
    /// Its index in the payment account (`m/9'/coin'/17'/0'/0'/index`).
    pub index: u32,
}

/// What a send to a Platform address takes, at the network's protocol
/// version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSendQuote {
    /// The most the addresses can lose, as Platform bounds it:
    /// `amount_credits` and, unless the amount pays the fee,
    /// `max_fee_credits`. The grant's cap.
    pub credits: u64,
    /// The transfer's output: what the recipient receives, less the fee
    /// when `fee_from_amount`.
    pub amount_credits: u64,
    /// The fee estimate: dpp's minimum fee for the transfer. Platform
    /// takes the fee it meters, usually about this.
    pub fee_credits: u64,
    /// The most the fee can take: all the fee source keeps after the send
    /// (`DeductFromInput` takes the metered fee from it, and nothing on
    /// Platform caps it lower), at least three times the estimate; the plan
    /// picks the source that keeps the least. `None` for Max: the fee comes
    /// out of the amount, bounded only by it.
    pub max_fee_credits: Option<u64>,
    /// Max: the output pays the fee, so the recipient receives
    /// `amount_credits` less it, and nothing is left on the inputs.
    pub fee_from_amount: bool,
    /// The recipient, normalized.
    pub to: String,
    /// What to authorize: no duffs, `credits` credits.
    pub grant: GrantRequest,
    /// Pass it to `platform_send`: it pays this quote's recipient and
    /// amount, once.
    pub operation: String,
    /// The held send this quote retries on that send's stored terms (a
    /// send that may still execute, the same recipient and amount). Its
    /// send pays nothing more if that one landed meanwhile.
    pub retry_of: Option<String>,
}

/// A send's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSendOutcome {
    /// The send's activity row id; a retry's of a send that landed, that
    /// send's.
    pub id: String,
    /// Until it is `Done` its amount; then what it took: its inputs and,
    /// unless the amount paid it, the fee (as the balances show it, or the
    /// estimate when `estimated`).
    pub credits: u64,
    /// The amount it sends (for Max, before the fee).
    pub amount_credits: u64,
    /// Max: the amount paid the fee.
    pub fee_from_amount: bool,
    /// The recipient; for a group's `Done`, `None` unless every member
    /// named it.
    pub to: Option<String>,
    /// As [`MoveToCoreOutcome::state`]; `Replaced` when its identical retry
    /// executed in its place.
    pub state: PlatformActivityState,
    /// It may still execute (`Pending`, or `Failed` by height): the
    /// recipient may still be paid, once, and only its retry may follow.
    pub may_land: bool,
    /// Done: the fee taken from the fee source as the balances show it;
    /// `None` when the amount paid it (Max) or the balances hide it.
    pub fee_credits: Option<u64>,
    /// Done: `credits` counts the fee estimate (`fee_credits` `None`).
    pub estimated: bool,
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

    /// The wallet's Platform receive address: the lowest unused one of the
    /// payment account, the same until a sync pass sees it funded. No
    /// vault, no network.
    pub async fn platform_receive_address(&self) -> Result<PlatformReceiveAddress, CreditsError> {
        self.on_wallet(move |session, manager, wallet| async move {
            if !session.platform.enabled() {
                return Err(PlatformError::FeatureOff {
                    feature: "platform".into(),
                }
                .into());
            }
            let net = platform_moves::SdkMoves {
                sdk: manager.sdk_arc(),
                wallet: Arc::clone(&wallet),
            };
            Ok(platform_moves::receive_address(&net, &wallet, session.vault.network()).await?)
        })
        .await
    }

    /// What sending `amount` of the Platform addresses' credits to the
    /// Platform address `to` takes; `retry_of`, the send it retries on its
    /// stored terms (the result sheet's "Retry this payment").
    pub async fn platform_send_quote(
        &self,
        to: String,
        amount: WithdrawAmount,
        retry_of: Option<String>,
    ) -> Result<PlatformSendQuote, CreditsError> {
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            let net = platform_moves::SdkMoves {
                sdk: manager.sdk_arc(),
                wallet: Arc::clone(&wallet),
            };
            let ask = (to.as_str(), amount, retry_of.as_deref());
            platform_moves::send_quote(&net, &session, &wallet, wallet_id, ask).await
        })
        .await
    }

    /// Sends the quoted `operation` (its recipient and amount) under
    /// `grant`, a fresh `PlatformOp` grant (no lease).
    pub async fn platform_send(
        &self,
        operation: String,
        grant: String,
    ) -> Result<PlatformSendOutcome, CreditsError> {
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            let net = platform_moves::SdkMoves {
                sdk: manager.sdk_arc(),
                wallet: Arc::clone(&wallet),
            };
            platform_moves::send(&net, &session, &wallet, wallet_id, &operation, &grant).await
        })
        .await
    }
}
