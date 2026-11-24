//! Rule 5 per read (MG-13 P12), the payout-change barriers' clearing (P16)
//! and the destination's check (P4, MG-00 D42), and the quote built on them.

use std::future::Future;

use dashcore::Network;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::{Identity, Purpose};
use dpp::prelude::Identifier;
use dw_appdb::{PayoutBarrierRow, WithdrawalRow};
use dw_vault::WithdrawalKey;
use tokio::time::Instant;

use super::drive::{Window, identity_key, key_hashes, window};
use super::net::{MnWithdrawNet, Proved};
use super::{Env, HeldKey, PayoutChange, WalletKeys, WithdrawalError, on_db};
use crate::masternodes::protx_records::ProTxOperationState;
use crate::masternodes::{WithdrawalCheckingReason, WithdrawalKeyChoice};
use crate::platform::trust::Rule5;
use crate::platform::withdrawal::{READ_BACK_TRIES, Rules, expected_duffs};
use crate::platform::{PlatformError, WithdrawAmount};

/// A read and whether it passed rule 5 under the mode (P12).
#[derive(Debug, Clone)]
pub(crate) struct Gated<T> {
    pub(crate) read: Proved<T>,
    pub(crate) passed: bool,
}

/// One read under rule 5 (P12, DP3-04 §3):
/// - verified (its proof used only SPV-verified quorum keys): passes;
/// - `Enforce`: an unverified read does not pass; the caller refuses
///   (`withdrawal_unverified`) or leaves the group undecided;
/// - `Probe`: DP3-04's single round, bounded by `probe`: the read is made
///   once more; verified, that one passes; a mismatch (rule 4) refuses at
///   once; anything else passes on the first read (rule 5 suspended);
/// - `Off`: passes (SPV is not consulted).
///
/// A mismatch on the first read is that error in every mode.
pub(crate) async fn gated<T, F, Fut>(
    rule: Rule5,
    probe: std::time::Duration,
    mut read: F,
) -> Result<Gated<T>, PlatformError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Proved<T>, PlatformError>>,
{
    let first = read().await?;
    if first.verified {
        return Ok(Gated {
            read: first,
            passed: true,
        });
    }
    match rule {
        Rule5::Enforce => Ok(Gated {
            read: first,
            passed: false,
        }),
        Rule5::Off => Ok(Gated {
            read: first,
            passed: true,
        }),
        Rule5::Probe => match tokio::time::timeout(probe, read()).await {
            Ok(Ok(again)) if again.verified => Ok(Gated {
                read: again,
                passed: true,
            }),
            Ok(Err(PlatformError::TrustMismatch)) => Err(PlatformError::TrustMismatch),
            _ => Ok(Gated {
                read: first,
                passed: true,
            }),
        },
    }
}

/// A read under rule 5 at or above `height`, made again while it answers
/// from below, `READ_BACK_TRIES` times at most: the last read, which the
/// caller compares with `height`. A failed read is `Err`.
pub(crate) async fn at_least<T, F, Fut>(
    env: &impl Env,
    height: u64,
    mut read: F,
) -> Result<Gated<T>, PlatformError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Proved<T>, PlatformError>>,
{
    let mut tries = 1;
    loop {
        let r = gated(env.rule_5(), env.waits().probe, &mut read).await?;
        if r.read.height >= height || tries == READ_BACK_TRIES {
            return Ok(r);
        }
        tries += 1;
    }
}

/// The identity id of the masternode `pro_tx_hash` (wire order): the
/// display-order hash (dw-vault `withdrawal.rs` module doc).
pub(crate) fn identity_of(pro_tx_hash: &[u8; 32]) -> Identifier {
    let mut id = *pro_tx_hash;
    id.reverse();
    Identifier::from(id)
}

/// What one look at the destination saw (P4).
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    /// The identity read rule 5 gated.
    pub(crate) identity: Gated<Option<Identity>>,
    pub(crate) header_tip: Option<u32>,
    /// `None` when (a)–(c) hold.
    pub(crate) reason: Option<WithdrawalCheckingReason>,
    /// The latest payout in the wallets' history is P2SH: Drive would pay
    /// an owner-key withdrawal to the P2PKH of its 20 bytes, which nobody
    /// can spend.
    pub(crate) payout_p2sh: bool,
}

impl Seen {
    /// The proved identity, or `NoIdentity`.
    pub(crate) fn identity(&self) -> Result<&Identity, WithdrawalError> {
        self.identity
            .read
            .value
            .as_ref()
            .ok_or(WithdrawalError::NoIdentity)
    }

    /// `Ok` when (a)–(c) hold, else the `checking` refusal.
    pub(crate) fn holds(&self) -> Result<(), WithdrawalError> {
        match self.reason {
            None => Ok(()),
            Some(reason) => Err(WithdrawalError::DestinationChecking { reason }),
        }
    }

    /// [`Self::holds`] for a withdrawal by `key`; an owner key is refused
    /// too while the payout is P2SH.
    pub(crate) fn holds_for(&self, key: WithdrawalKeyChoice) -> Result<(), WithdrawalError> {
        self.holds()?;
        match self.owner_reason(key) {
            None => Ok(()),
            Some(reason) => Err(WithdrawalError::DestinationChecking { reason }),
        }
    }

    fn owner_reason(&self, key: WithdrawalKeyChoice) -> Option<WithdrawalCheckingReason> {
        (key == WithdrawalKeyChoice::Owner && self.payout_p2sh)
            .then_some(WithdrawalCheckingReason::OwnerPayoutP2sh)
    }
}

/// One look (P4): the identity read under rule 5, the barriers' clearing
/// terms applied, then (a)–(c), in this order of reasons: SPV unsynced,
/// (c) unverified, (b) a barrier pending, (b) a payout change unconfirmed
/// in the wallets' history, (a) Platform behind, or a change in a block
/// above the proof's core height.
pub(crate) async fn look(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    pro_tx_hash: &[u8; 32],
) -> Result<Seen, WithdrawalError> {
    let id = identity_of(pro_tx_hash);
    let identity = gated(env.rule_5(), env.waits().probe, || net.identity(id)).await?;
    let header_tip = env.chain().header_tip;
    let pending = pending_barriers(env, pro_tx_hash, &identity).await?;
    let changes = env.payout_changes(pro_tx_hash);
    let (unconfirmed, unapplied) = unpaid_changes(&changes, &identity.read);
    let core = identity.read.core_height;
    let reason = if header_tip.is_none() {
        Some(WithdrawalCheckingReason::SpvUnsynced)
    } else if !identity.passed {
        Some(WithdrawalCheckingReason::Unverified)
    } else if pending == Some(Pending::Abandoned) {
        Some(WithdrawalCheckingReason::PayoutChangeAbandoned)
    } else if pending.is_some() {
        Some(WithdrawalCheckingReason::PayoutChangePending)
    } else if unconfirmed {
        Some(WithdrawalCheckingReason::PayoutChangeUnconfirmed)
    } else if unapplied || header_tip.is_some_and(|tip| core < tip) {
        Some(WithdrawalCheckingReason::PlatformBehind)
    } else {
        None
    };
    let payout_p2sh = owner_payout_p2sh(&changes, identity.read.value.as_ref());
    Ok(Seen {
        identity,
        header_tip,
        reason,
        payout_p2sh,
    })
}

/// Of the payout changes Drive's window in `read` doesn't pay (all of
/// them, with no window): whether one is out of a block, and whether one
/// is in a block above the proof's core height.
pub(super) fn unpaid_changes(
    changes: &[PayoutChange],
    read: &Proved<Option<Identity>>,
) -> (bool, bool) {
    let paid = match read.value.as_ref().map(window) {
        Some(Window::Pays { script, .. }) => Some(script),
        _ => None,
    };
    let unpaid = || {
        (changes.iter().filter(|c| !c.registration)).filter(|c| {
            paid.as_ref()
                .is_none_or(|p| c.script.as_deref() != Some(p.as_bytes()))
        })
    };
    (
        unpaid().any(|c| c.height.is_none()),
        unpaid().any(|c| c.height.is_some_and(|h| h > read.core_height)),
    )
}

/// Whether an owner-key withdrawal would pay a script nobody can spend: the
/// latest payout of `changes` is P2SH while Drive's window in `identity`
/// still pays P2PKH (dashpay/platform#5840). Once Drive's window pays the
/// P2SH script itself, nothing is refused.
pub(super) fn owner_payout_p2sh(changes: &[PayoutChange], identity: Option<&Identity>) -> bool {
    latest_is_p2sh(changes)
        && identity.map(window).is_some_and(|w| {
            matches!(w, Window::Pays { script, .. }
                if dashcore::Script::from_bytes(script.as_bytes()).is_p2pkh())
        })
}

/// Whether an open owner-key record must stay unsent. Drive takes the
/// destination at execution, so any P2SH payout in `changes` pays the
/// P2PKH of that script's 20 bytes until the window has moved on. The hold
/// lifts only when the window pays a P2SH script, or pays the latest
/// payout's P2PKH script and [`unpaid_changes`] reports nothing unconfirmed
/// or above the proof's core height. No window that shows either leaves it
/// held.
pub(super) fn owner_key_resend_held(
    changes: &[PayoutChange],
    read: &Proved<Option<Identity>>,
) -> bool {
    if !any_is_p2sh(changes) {
        return false;
    }
    let Some(Window::Pays { script, .. }) = read.value.as_ref().map(window) else {
        return true;
    };
    if dashcore::Script::from_bytes(script.as_bytes()).is_p2sh() {
        return false;
    }
    if pays_latest_p2pkh(changes, script.as_bytes()) {
        let (unconfirmed, unapplied) = unpaid_changes(changes, read);
        if !unconfirmed && !unapplied {
            return false;
        }
    }
    true
}

/// Whether any payout in `changes` is P2SH.
pub(super) fn any_is_p2sh(changes: &[PayoutChange]) -> bool {
    changes
        .iter()
        .any(|c| c.script.as_deref().is_some_and(script_is_p2sh))
}

/// Whether the latest payout of `changes` is P2SH: the unconfirmed
/// changes, else those in the highest block, else the registration's.
pub(super) fn latest_is_p2sh(changes: &[PayoutChange]) -> bool {
    let Some(top) = changes.iter().map(payout_rank).max() else {
        return false;
    };
    (changes.iter().filter(|c| payout_rank(c) == top))
        .any(|c| c.script.as_deref().is_some_and(script_is_p2sh))
}

/// The window's `paid` script is every latest payout, and that payout is
/// P2PKH. Several payouts share the latest rank only when each of them is
/// that script.
fn pays_latest_p2pkh(changes: &[PayoutChange], paid: &[u8]) -> bool {
    let Some(top) = changes.iter().map(payout_rank).max() else {
        return false;
    };
    let mut latest = changes.iter().filter(|c| payout_rank(c) == top);
    latest.next().is_some_and(|first| {
        script_is_paid_p2pkh(first, paid) && latest.all(|c| script_is_paid_p2pkh(c, paid))
    })
}

fn script_is_paid_p2pkh(change: &PayoutChange, paid: &[u8]) -> bool {
    change
        .script
        .as_deref()
        .is_some_and(|script| script == paid && dashcore::Script::from_bytes(script).is_p2pkh())
}

fn script_is_p2sh(script: &[u8]) -> bool {
    dashcore::Script::from_bytes(script).is_p2sh()
}

/// Unconfirmed changes outrank a block; a block outranks the registration.
fn payout_rank(change: &PayoutChange) -> u64 {
    match (change.registration, change.height) {
        (true, _) => 0,
        (false, Some(height)) => u64::from(height) + 1,
        (false, None) => u64::MAX,
    }
}

/// Whether a payout-change barrier of the masternode is still pending
/// after P16's clearing terms, clearing the ones a term releases:
/// - **Platform evidence**: `identity`, read at or after the barrier and
///   passing rule 5, has Drive's window paying the barrier's script;
/// - **confirmed and covered**: the operation is `Confirmed { h }` in its
///   wallet's history and `identity`'s proof (passing rule 5) has a core
///   height of at least `h`;
/// - **conflicted**: the operation is `Conflicted`.
///
/// Oldest first: while an older barrier of the masternode is pending, a
/// later one doesn't clear on Platform evidence (its change may be applied
/// after the older one's, as in A→B→A, so the window paying its script now
/// proves nothing); it clears only when confirmed and covered, or
/// conflicted. An operation that can't be read (its wallet deleted or not
/// loaded) leaves Platform evidence the only term (fail closed). Backfill
/// not complete (`barriers_ready` false) is pending whatever the table
/// holds, and counts as an older pending barrier. `Abandoned` when every
/// pending one is an operation its wallet abandoned (MG-10 §1.10).
async fn pending_barriers(
    env: &impl Env,
    pro_tx_hash: &[u8; 32],
    identity: &Gated<Option<Identity>>,
) -> Result<Option<Pending>, WithdrawalError> {
    let shown = crate::masternodes::display_hex(pro_tx_hash);
    let barriers = on_db(env, move |db| db.payout_barriers_of(&shown)).await?;
    let mut pending = (!env.barriers_ready()).then_some(Pending::Other);
    for barrier in barriers {
        match clears(env, &barrier, identity, pending.is_none()).await? {
            None => {
                let cleared =
                    on_db(env, move |db| db.clear_payout_barrier_durable(barrier.id)).await?;
                tracing::info!(cleared, "a payout-change barrier cleared (MG-13 P16)");
            }
            Some(p) => pending = Some(pending.map_or(p, |q| q.max(p))),
        }
    }
    Ok(pending)
}

/// Why a payout-change barrier is still pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Pending {
    /// Its operation was abandoned in Activity and may still be mined.
    Abandoned,
    Other,
}

/// `None` when `barrier` clears, else why it is pending.
async fn clears(
    env: &impl Env,
    barrier: &PayoutBarrierRow,
    identity: &Gated<Option<Identity>>,
    evidence: bool,
) -> Result<Option<Pending>, WithdrawalError> {
    let platform = identity.passed.then_some(&identity.read);
    if evidence
        && let Some(read) = platform
        && let Some(id) = &read.value
        && read.time_ms >= barrier.created_at.saturating_mul(1000)
        && let Window::Pays { script, .. } = window(id)
        && script.as_bytes() == barrier.barrier.payout_script.as_slice()
    {
        return Ok(None);
    }
    let clears = match env.barrier_state(barrier).await? {
        Some(ProTxOperationState::Confirmed { height }) => {
            platform.is_some_and(|read| height <= read.core_height)
        }
        Some(ProTxOperationState::Conflicted) => true,
        Some(ProTxOperationState::Abandoned) => return Ok(Some(Pending::Abandoned)),
        _ => false,
    };
    Ok((!clears).then_some(Pending::Other))
}

/// The look, again every `poll` until (a)–(c) hold, while only (a) fails
/// (Platform behind: it catches up) or (c) (unverified: SPV may verify the
/// quorum meanwhile, H-09), for at most `wait` (P4's bounded waits: the
/// quote's 240 s before consent, `withdraw`'s 60 s before the lock and the
/// grant). Any other reason ends the wait at once.
pub(crate) async fn look_until(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    pro_tx_hash: &[u8; 32],
    wait: std::time::Duration,
) -> Result<Seen, WithdrawalError> {
    let deadline = Instant::now() + wait;
    loop {
        let seen = look(env, net, pro_tx_hash).await?;
        let poll = env.waits().poll;
        let retried = matches!(
            seen.reason,
            Some(WithdrawalCheckingReason::PlatformBehind | WithdrawalCheckingReason::Unverified)
        );
        if !retried || Instant::now() + poll > deadline {
            return Ok(seen);
        }
        tokio::time::sleep(poll).await;
    }
}

/// One of the wallet's keys as the quote shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyView {
    pub(crate) key: WithdrawalKey,
    /// The identity key it signs with.
    pub(crate) identity_key: u32,
}

/// Where a withdrawal by one key pays (P4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destination {
    /// Withdraw on: the owner key's Drive window, or the payout key's own
    /// address by default.
    Fixed {
        address: String,
        script: CoreScript,
        platform_height: u64,
        core_height: u32,
        header_tip: u32,
        /// `header_tip − core_height`, 0 when Platform is ahead.
        lag: u32,
    },
    /// Withdraw off (MG-00 D42).
    Checking { reason: WithdrawalCheckingReason },
}

/// Platform's side as the detail pane shows it (P13).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlatformStatus {
    pub(crate) identity_found: bool,
    /// Every key: id, purpose, disabled.
    pub(crate) keys: Vec<(u32, Purpose, bool)>,
    pub(crate) balance: Option<u64>,
    pub(crate) platform_height: u64,
    pub(crate) core_height: u32,
    pub(crate) header_tip: Option<u32>,
    pub(crate) lag: Option<u32>,
}

/// `withdrawal_quote` (P13): the keys held, what can be claimed, the rules,
/// each key's destination and the open record.
#[derive(Debug, Clone)]
pub(crate) struct Quote {
    pub(crate) owner_key: Option<KeyView>,
    pub(crate) payout_keys: Vec<KeyView>,
    /// The balance less the reserve, in whole duffs (P11); 0 below the
    /// least withdrawal.
    pub(crate) claimable: u64,
    pub(crate) expected_duffs: Option<u64>,
    pub(crate) rules: Rules,
    pub(crate) owner_destination: Option<Destination>,
    pub(crate) payout_destination: Option<Destination>,
    pub(crate) status: PlatformStatus,
    pub(crate) open: Option<WithdrawalRow>,
}

/// The keys of `identity` the wallet holds, each against its identity key
/// (P2): owner keys on OWNER keys, payout keys on enabled TRANSFER keys.
pub(crate) async fn held(
    keys: &impl WalletKeys,
    identity: &Identity,
) -> Result<(Option<KeyView>, Vec<KeyView>), WithdrawalError> {
    let owner = key_hashes(identity, Purpose::OWNER);
    let payout = key_hashes(identity, Purpose::TRANSFER);
    let held = keys.held(owner, payout).await?;
    let view = |h: &HeldKey, purpose| {
        identity_key(identity, purpose, &h.hash).map(|k| KeyView {
            key: h.key.clone(),
            identity_key: k.id(),
        })
    };
    let owner = (held.iter())
        .filter(|h| h.key.is_owner())
        .find_map(|h| view(h, Purpose::OWNER).ok());
    let payout = (held.iter())
        .filter(|h| !h.key.is_owner())
        .filter_map(|h| view(h, Purpose::TRANSFER).ok())
        .collect();
    Ok((owner, payout))
}

/// `withdrawal_quote(wallet, pro_tx_hash)` (P13), after waiting up to
/// `wait` for Platform to catch up (P4; `Duration::ZERO` reads once). The
/// grant comes after it.
pub(crate) async fn quote(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    keys: &impl WalletKeys,
    network: Network,
    pro_tx_hash: &[u8; 32],
    wait: std::time::Duration,
) -> Result<Quote, WithdrawalError> {
    let seen = look_until(env, net, pro_tx_hash, wait).await?;
    let identity = seen.identity()?;
    let (owner_key, payout_keys) = held(keys, identity).await?;
    let id = identity_of(pro_tx_hash);
    let balance = net.balance(id).await?.value;
    let version = net.version();
    let rules = Rules::at(version)?;
    let claimable = balance
        .and_then(|b| rules.credits(b, WithdrawAmount::All).ok())
        .unwrap_or(0);
    let expected = (claimable > 0)
        .then(|| expected_duffs(claimable, version))
        .transpose()?;
    let read = &seen.identity.read;
    let fixed = |script: &CoreScript| match (seen.reason, seen.header_tip) {
        (None, Some(tip)) => Destination::Fixed {
            address: crate::masternodes::withdrawals::address(script.as_bytes(), network),
            script: script.clone(),
            platform_height: read.height,
            core_height: read.core_height,
            header_tip: tip,
            lag: tip.saturating_sub(read.core_height),
        },
        (reason, _) => Destination::Checking {
            reason: reason.unwrap_or(WithdrawalCheckingReason::SpvUnsynced),
        },
    };
    let owner_destination = match (&owner_key, window(identity)) {
        (Some(_), Window::Pays { script, .. }) => {
            Some(match seen.owner_reason(WithdrawalKeyChoice::Owner) {
                Some(reason) if seen.reason.is_none() => Destination::Checking { reason },
                _ => fixed(&script),
            })
        }
        _ => None,
    };
    let payout_destination = payout_keys.first().and_then(|k| {
        let key = identity.public_keys().get(&k.identity_key)?;
        super::drive::script_of(key).map(|s| fixed(&s))
    });
    let shown = id.to_string(dpp::platform_value::string_encoding::Encoding::Base58);
    let open = on_db(env, move |db| db.open_withdrawal(&shown)).await?;
    Ok(Quote {
        owner_key,
        payout_keys,
        claimable,
        expected_duffs: expected,
        rules,
        owner_destination,
        payout_destination,
        status: PlatformStatus {
            identity_found: true,
            keys: (identity.public_keys().values())
                .map(|k| (k.id(), k.purpose(), k.is_disabled()))
                .collect(),
            balance,
            platform_height: read.height,
            core_height: read.core_height,
            header_tip: seen.header_tip,
            lag: seen
                .header_tip
                .map(|tip| tip.saturating_sub(read.core_height)),
        },
        open,
    })
}
