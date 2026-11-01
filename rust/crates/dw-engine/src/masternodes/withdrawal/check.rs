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
use super::{Env, HeldKey, WalletKeys, WithdrawalError, on_db};
use crate::masternodes::WithdrawalCheckingReason;
use crate::masternodes::protx_records::ProTxOperationState;
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
}

/// One look (P4): the identity read under rule 5, the barriers' clearing
/// terms applied, then (a)–(c), in this order of reasons: SPV unsynced,
/// (c) unverified, (b) a barrier pending, (a) Platform behind.
pub(crate) async fn look(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    pro_tx_hash: &[u8; 32],
) -> Result<Seen, WithdrawalError> {
    let id = identity_of(pro_tx_hash);
    let identity = gated(env.rule_5(), env.waits().probe, || net.identity(id)).await?;
    let header_tip = env.chain().header_tip;
    let pending = pending_barriers(env, pro_tx_hash, &identity).await?;
    let core = identity.read.core_height;
    let reason = if header_tip.is_none() {
        Some(WithdrawalCheckingReason::SpvUnsynced)
    } else if !identity.passed {
        Some(WithdrawalCheckingReason::Unverified)
    } else if pending {
        Some(WithdrawalCheckingReason::PayoutChangePending)
    } else if header_tip.is_some_and(|tip| core < tip) {
        Some(WithdrawalCheckingReason::PlatformBehind)
    } else {
        None
    };
    Ok(Seen {
        identity,
        header_tip,
        reason,
    })
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
/// An operation that can't be read (its wallet deleted or not loaded)
/// leaves Platform evidence the only term (fail closed). Backfill not
/// complete (`barriers_ready` false) is pending whatever the table holds.
async fn pending_barriers(
    env: &impl Env,
    pro_tx_hash: &[u8; 32],
    identity: &Gated<Option<Identity>>,
) -> Result<bool, WithdrawalError> {
    let shown = crate::masternodes::display_hex(pro_tx_hash);
    let barriers = on_db(env, move |db| db.payout_barriers_of(&shown)).await?;
    let mut pending = !env.barriers_ready();
    for barrier in barriers {
        if clears(env, &barrier, identity).await? {
            let cleared = on_db(env, move |db| db.clear_payout_barrier_durable(barrier.id)).await?;
            tracing::info!(cleared, "a payout-change barrier cleared (MG-13 P16)");
        } else {
            pending = true;
        }
    }
    Ok(pending)
}

async fn clears(
    env: &impl Env,
    barrier: &PayoutBarrierRow,
    identity: &Gated<Option<Identity>>,
) -> Result<bool, WithdrawalError> {
    let platform = identity.passed.then_some(&identity.read);
    if let Some(read) = platform
        && let Some(id) = &read.value
        && read.time_ms >= barrier.created_at.saturating_mul(1000)
        && let Window::Pays { script, .. } = window(id)
        && script.as_bytes() == barrier.barrier.payout_script.as_slice()
    {
        return Ok(true);
    }
    Ok(match env.barrier_state(barrier).await? {
        Some(ProTxOperationState::Confirmed { height }) => {
            platform.is_some_and(|read| height <= read.core_height)
        }
        Some(ProTxOperationState::Conflicted) => true,
        _ => false,
    })
}

/// The look, again every `poll` until (a)–(c) hold, while only (a) fails
/// (Platform behind: it catches up), for at most `wait` (P4's bounded
/// waits: the quote's 240 s before consent, `withdraw`'s 60 s before the
/// lock and the grant). Any other reason ends the wait at once.
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
        if seen.reason != Some(WithdrawalCheckingReason::PlatformBehind)
            || Instant::now() + poll > deadline
        {
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
        (Some(_), Window::Pays { script, .. }) => Some(fixed(&script)),
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
