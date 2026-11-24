//! Recovering the value left on a transfer to Shielded that Platform refused
//! after a penalty (#402). Drive keeps such a lock partly used: its stored
//! `AssetLockValue` holds the remaining credits, which an address funding
//! signed by the same credit key spends. The shield kind keeps what a proved
//! read left ([`Mark::Remaining`], `unfinished_shields`) and lists the lock
//! Recoverable, or too small below Platform's fee for moving it.
//!
//! A recover runs on the fund side under a `PlatformAddressFund` lease: the
//! value lands on the wallet's own Platform receive address, the only
//! destination (no caller names one), through the library's
//! `fund_from_shielded_remainder_asset_lock`, signed with the lock's credit
//! key alone ([`LockKeySigner`]). Before anything is read it passes the shield
//! flow's own network gate and the fund's; then, under the shield and fund
//! locks, it reads Platform's proved state again and goes on only for a
//! remaining value at least Platform's fee. The grant must carry exactly
//! those credits (a quote reads them first, [`quote`]), and is redeemed only
//! once every refusal before it has passed. Just before the lock key signs,
//! a `recovered` record of the wallet's Platform activity is armed, so a rise
//! of the address is the recover's while it is pending.
//!
//! Only a proved apply settles the lock: [`Mark::Settled`], the library's own
//! row `Consumed`, and the record done. A failure after the signature leaves
//! the lock listed; the next recover's proved read settles it if Platform
//! shows it used (completing an armed record with its estimate), or offers
//! it again.

use std::sync::Arc;

use dashcore::OutPoint;
use dpp::version::PlatformVersion;

use super::errors::PlatformError;
use super::flows::{BudgetPurpose, FlowKind};
use super::lease::Lease;
use super::platform_fund::{self as fund, FundLib};
use super::platform_moves;
use super::shielded_pool as pool;
use super::unfinished::{RecoverQuote, UnfinishedState};
use super::unfinished_funds::ArmingSigner;
use super::unfinished_locks::{
    self as locks, ApplyReply, Evidence, KindLock, LockFinish, LockKeySigner, LockKindId, Mark,
    Marks, Readiness, SpentRead, reply_from,
};
use crate::{NetworkSession, PlatformChange, WalletId};

/// The code a recover of a lock that is not listed Recoverable is refused
/// with.
pub(crate) const NOT_RECOVERABLE: &str = "recovery.not_recoverable";

/// The call permit's label.
const LABEL: &str = "recover";

/// What a recover's checks found, under the shield and fund locks.
enum Standing {
    /// Refused before the grant: the code.
    Refused(String),
    /// Nothing left to move: recovered, or used elsewhere.
    Finished(LockFinish),
    TooSmall {
        credits: u64,
        floor: u64,
    },
    Ready {
        lock: KindLock,
        credits: u64,
        floor: u64,
    },
}

/// `DashPay::recover_transfer_quote`: what a recover of the partly used
/// shield lock at `outpoint` would move, from Platform's proved state now.
pub(crate) async fn quote(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl FundLib,
    outpoint: OutPoint,
) -> Result<RecoverQuote, PlatformError> {
    let (shield, fund) = (pool::wallet_lock(session, wallet), fund::fund_lock(wallet));
    let (_shield, _fund) = (shield.lock().await, fund.lock().await);
    Ok(match standing(session, wallet, lib, outpoint).await? {
        Standing::Refused(error) => RecoverQuote::NotApplied { error },
        Standing::Finished(_) => RecoverQuote::Finished,
        Standing::TooSmall { credits, floor } => RecoverQuote::TooSmall {
            credits,
            fee_credits: floor,
        },
        Standing::Ready { credits, floor, .. } => RecoverQuote::Ready {
            credits,
            fee_credits: floor,
        },
    })
}

/// `DashPay::recover_transfer`: moves what is left on the partly used shield
/// lock at `outpoint` to the wallet's Platform receive address under a
/// `PlatformAddressFund` lease redeemed from `grant` (module doc).
pub(crate) async fn recover(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl FundLib,
    outpoint: OutPoint,
    grant: &str,
) -> Result<LockFinish, PlatformError> {
    let (shield, fund) = (pool::wallet_lock(session, wallet), fund::fund_lock(wallet));
    let (_shield, _fund) = (shield.lock().await, fund.lock().await);
    let (lock, credits, floor) = match standing(session, wallet, lib, outpoint).await? {
        Standing::Refused(error) => return Ok(LockFinish::NotApplied { error }),
        Standing::Finished(done) => return Ok(done),
        Standing::TooSmall { floor, .. } => {
            return Ok(LockFinish::NotApplied {
                error: super::errors::CreditsError::BelowMinimum { min: floor }
                    .code()
                    .into(),
            });
        }
        Standing::Ready {
            lock,
            credits,
            floor,
        } => (lock, credits, floor),
    };
    let script = (lib.credit_script(outpoint).await).ok_or(PlatformError::Unavailable)?;
    let to = lib.receive_address().await?;
    let address = platform_moves::bech32m(&to, lib.network());
    let lease = authority(session, wallet, grant, credits).await?;
    let funding = lease.uncounted_funding_signer()?;
    let estimate = credits.saturating_sub(floor);
    let hold = platform_moves::hold_fund(session, wallet, address.clone()).await;
    let ran = lease
        .scope(async {
            let permit = lease.call_permit(LABEL, None)?;
            let signer = LockKeySigner::new(&funding, script, permit.signatures());
            let arming = ArmingSigner::new(&signer, session, wallet, (outpoint, lock.info.amount))
                .paying(&address, estimate)
                .recovering();
            let applied = lib.apply_remainder(outpoint, to, &arming).await;
            let signed = signer.signed();
            permit.finish(applied.is_ok());
            Ok::<_, PlatformError>((applied, signed, arming.arming.get().copied()))
        })
        .await;
    (session.hub.pump).mark_platform(wallet, [PlatformChange::PlatformAddresses]);
    let (applied, signed, arming) = match ran {
        Ok(ran) => ran,
        Err(e) => {
            return Ok(LockFinish::NotApplied {
                error: e.code().into(),
            });
        }
    };
    let txid = outpoint.txid.to_string();
    if !signed
        && let Some(arming) = arming
        && let Err(e) = platform_moves::disarm_fund(session, wallet, &txid, arming).await
    {
        tracing::warn!(%outpoint, error = %e, "could not disarm a recover that signed nothing");
    }
    let gained = applied.as_ref().ok().copied().flatten();
    Ok(match reply_from(applied.map(drop), outpoint, signed) {
        Ok(ApplyReply::Done) => {
            let credited = gained.map_or((estimate, true), |g| (g.min(credits), false));
            settle(session, wallet, &lock, address, credited).await;
            drop(hold);
            lib.sync_now().await;
            (session.hub.pump).mark_platform(wallet, [PlatformChange::PlatformAddresses]);
            LockFinish::Applied
        }
        Ok(how) => {
            tracing::warn!(%outpoint, ?how, "a recover may have executed, unproved");
            LockFinish::Checking
        }
        Err(e) => {
            tracing::warn!(%outpoint, error = %e, "a recover was refused before its signature");
            LockFinish::NotApplied {
                error: e.code().into(),
            }
        }
    })
}

/// The shield flow's network gate, then the fund's: the rules after one
/// proved read of the version. The first check reads nothing.
async fn gates(lib: &impl FundLib) -> Result<&'static PlatformVersion, String> {
    pool::active(lib.protocol_version()).map_err(|e| e.code().to_owned())?;
    let rules = fund::gate(lib).await.map_err(|e| e.code().to_owned())?;
    pool::active(rules.protocol_version).map_err(|e| e.code().to_owned())?;
    Ok(rules)
}

/// The lock at `outpoint` as the shield kind lists it, and Platform's proved
/// state of it now.
async fn standing(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: &impl FundLib,
    outpoint: OutPoint,
) -> Result<Standing, PlatformError> {
    let rules = match gates(lib).await {
        Ok(rules) => rules,
        Err(code) => return Ok(Standing::Refused(code)),
    };
    let floor = fund::fee_credits(rules);
    let kind = LockKindId::Shielded;
    let own = (session.leases.shield_lock_txids(wallet)).ok_or(PlatformError::Unavailable)?;
    let rows = lib.shield_lock_rows().await;
    let Some(lock) = rows.into_iter().find(|l| l.info.outpoint == outpoint) else {
        return Err(PlatformError::InvalidArgument {
            detail: "no tracked shield lock has that outpoint".into(),
        });
    };
    let settled = Marks {
        outpoints: locks::marks_of(session, wallet, kind, Mark::Settled).await,
        txids: (session.leases.top_ups_settled(wallet).await).unwrap_or_default(),
    };
    let used = locks::marks_of(session, wallet, kind, Mark::Used).await;
    let remaining = locks::remaining_of(session, wallet, kind).await;
    let ev = Evidence {
        own: &own,
        settled: &settled,
        unknown: &Marks::default(),
        reported_used: &used,
        readiness: &Readiness::Ready,
        minimum: None,
        remaining: &remaining,
        remainder_floor: Some(floor),
    };
    let Some(listed) = locks::classify(kind, &lock, &ev) else {
        let done = match used.contains(&outpoint) {
            true => LockFinish::ReportedUsed,
            false => LockFinish::Applied,
        };
        return Ok(Standing::Finished(done));
    };
    let partly_used = matches!(
        listed.state,
        UnfinishedState::Recoverable | UnfinishedState::TooSmall
    ) && listed.remaining_credits.is_some();
    if !partly_used {
        return Ok(Standing::Refused(NOT_RECOVERABLE.into()));
    }
    Ok(match lib.spent(outpoint).await {
        SpentRead::Remaining { credits } => {
            if remaining.get(&outpoint) != Some(&credits) {
                locks::record_remaining(session, wallet, kind, outpoint, credits).await;
            }
            match credits < floor {
                true => Standing::TooSmall { credits, floor },
                false => Standing::Ready {
                    lock,
                    credits,
                    floor,
                },
            }
        }
        SpentRead::Consumed => Standing::Finished(used_up(session, wallet, &lock).await),
        SpentRead::Unspent => Standing::Refused(NOT_RECOVERABLE.into()),
        SpentRead::Unread => Standing::Refused(PlatformError::Unavailable.code().into()),
    })
}

/// Platform proved the partly used `lock` used up. A recover of this
/// wallet's armed for it executed: settled, its record done with its
/// estimate. Otherwise it was used elsewhere ([`Mark::Used`]).
async fn used_up(session: &NetworkSession, wallet: WalletId, lock: &KindLock) -> LockFinish {
    let at = lock.info.outpoint;
    let record = platform_moves::fund_record(session, wallet, &at.txid.to_string()).await;
    match record {
        Ok(Some(r)) if r.recovered && r.armed => {
            let credited = (r.credits, true);
            settle(session, wallet, lock, r.address, credited).await;
            LockFinish::Applied
        }
        _ => {
            locks::record(session, wallet, LockKindId::Shielded, Mark::Used, at).await;
            LockFinish::ReportedUsed
        }
    }
}

/// After a recover's proved apply: the lock settled as the shield kind
/// records it, and its record done with `credited`.
async fn settle(
    session: &NetworkSession,
    wallet: WalletId,
    lock: &KindLock,
    address: String,
    credited: (u64, bool),
) {
    let at = lock.info.outpoint;
    locks::record(session, wallet, LockKindId::Shielded, Mark::Settled, at).await;
    if lock.alone()
        && let Err(e) = (session.leases)
            .settle_top_up(wallet, at.txid, dw_appdb::dispatch::TopUpSettled::Applied)
            .await
    {
        tracing::warn!(%at, error = %e, "could not record a recovered lock settled");
    }
    let row = (at, lock.info.amount);
    if let Err(e) = platform_moves::fund_applied(session, wallet, row, address, credited).await {
        tracing::warn!(%at, error = %e, "could not record a recover in the Platform activity");
    }
}

/// The `PlatformAddressFund` lease of a recover of `credits`, redeemed from
/// the vault grant `grant`: a funding budget for the lock key's signer, and
/// exactly `credits` as its credits cap. A held lease is refused (it cannot
/// carry the recover's size). Every refusal keeps the grant.
async fn authority(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    grant: &str,
    credits: u64,
) -> Result<Lease, PlatformError> {
    if session.lease_for(&wallet, grant).is_some() {
        return Err(PlatformError::GrantInvalid);
    }
    let cap = |purpose| super::registration::grant_cap(session, wallet, grant, purpose);
    if cap(BudgetPurpose::Funding).await? == 0 {
        return Err(PlatformError::NeedsGrant {
            purpose: BudgetPurpose::Funding,
        });
    }
    if cap(BudgetPurpose::Credits).await? != credits {
        return Err(PlatformError::GrantInvalid);
    }
    Ok(session
        .begin_lease(wallet, FlowKind::PlatformAddressFund, &[grant.to_owned()])
        .await?)
}
