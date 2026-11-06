//! Core → Platform address locks (funding type 4) as an unfinished-lock
//! kind ([`LockKind`]), over the fund flow's own library seam
//! ([`FundLib`]). `platform_fund`, `resume_platform_fund` and the
//! recovery's Finish all apply through it, under the same per-wallet lock
//! as a fund. A lock a `paddr/` step of this install signed is this
//! device's; any other the library tracks (a restore's, another device's)
//! is restored. Its records are the runner's, by outpoint
//! ([`locks::record`]), and the journal's `top_up_settled` by txid for a
//! lock alone in its transaction; it keeps no records by txid of unknown
//! consumption (no release wrote any).
//!
//! The apply pays the address the fund's activity record names, else the
//! receive address, and records the lock's `from_core` row first when it
//! has none (a restored lock). While it runs that address is held out of
//! the activity reads; after a proved apply the settle records the rise as
//! the fund's, at most the lock's credits. Platform's answer that the
//! lock is used completes the record with its estimate, as the activity
//! does for any lock known finished ([`finished_funds`]); neither settles
//! the lock.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use dashcore::{OutPoint, ScriptBuf, Txid};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::TopUpSettled;
use platform_wallet::PlatformWallet;

use super::errors::{CreditsError, PlatformError};
use super::flows::FlowKind;
use super::platform_fund::{self as fund, FundLib};
use super::platform_moves::{self, FundHold};
use super::registration_flow::LockStatus;
use super::unfinished_locks::{
    self as locks, ApplyReply, KindLock, LockKeySigner, LockKind, LockKindId, Mark, Marks, Minimum,
    Readiness, reply_from,
};
use crate::{NetworkSession, PlatformChange, WalletId};

/// The wallet's fund locks as a [`LockKind`].
pub(crate) struct FundLocks<'a, L> {
    session: &'a NetworkSession,
    wallet: WalletId,
    lib: &'a L,
    /// The proved apply [`LockKind::settle`] records: the address it paid,
    /// held out of the activity reads until then, and what its proof shows
    /// that address gained.
    applied: Mutex<Option<Applied>>,
    /// The credits the last settle recorded.
    credited: Mutex<Option<u64>>,
}

struct Applied {
    address: String,
    gained: Option<u64>,
    _hold: FundHold,
}

impl<'a, L: FundLib> FundLocks<'a, L> {
    pub(crate) fn new(session: &'a NetworkSession, wallet: WalletId, lib: &'a L) -> Self {
        Self {
            session,
            wallet,
            lib,
            applied: Mutex::default(),
            credited: Mutex::default(),
        }
    }

    /// The credits the last settle recorded as the fund's.
    pub(crate) fn credited(&self) -> Option<u64> {
        *self.credited.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn marks(&self, mark: Mark) -> HashSet<OutPoint> {
        locks::marks_of(self.session, self.wallet, LockKindId::PlatformAddress, mark).await
    }

    async fn record(&self, mark: Mark, lock: &KindLock) {
        let at = lock.info.outpoint;
        locks::record(
            self.session,
            self.wallet,
            LockKindId::PlatformAddress,
            mark,
            at,
        )
        .await;
    }

    async fn settled_txids(&self) -> HashSet<Txid> {
        (self.session.leases.top_ups_settled(self.wallet).await).unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %self.wallet, error = %e, "could not read the settled funds");
            HashSet::new()
        })
    }

    /// The address the fund `txid` pays: its record's, else the receive
    /// address, recorded then as the lock's `from_core` row of `duffs`.
    async fn destination(
        &self,
        txid: Txid,
        duffs: u64,
    ) -> Result<(dpp::address_funds::PlatformAddress, String), PlatformError> {
        let txid = txid.to_string();
        let recorded = platform_moves::fund_address(self.session, self.wallet, &txid).await?;
        if let Some(to) = recorded.as_deref().and_then(platform_moves::parse) {
            return Ok((to, recorded.unwrap_or_default()));
        }
        let to = self.lib.receive_address().await?;
        let address = platform_moves::bech32m(&to, self.lib.network());
        let estimate = fund::estimate(self.lib, duffs);
        let at = (txid.clone(), duffs);
        if let Err(e) =
            platform_moves::record_fund(self.session, self.wallet, at, address.clone(), estimate)
                .await
        {
            tracing::warn!(%txid, error = %e, "could not record a fund in the Platform activity");
        }
        Ok((to, address))
    }
}

impl<L: FundLib> LockKind for FundLocks<'_, L> {
    fn id(&self) -> LockKindId {
        LockKindId::PlatformAddress
    }

    fn flow(&self) -> FlowKind {
        FlowKind::PlatformAddressFund
    }

    async fn locks(&self) -> Vec<KindLock> {
        self.lib.fund_lock_rows().await
    }

    /// The fund steps' signed locks (`paddr/<k>/funding`).
    fn own_locks(&self) -> Option<HashSet<Txid>> {
        self.session.leases.fund_lock_txids(self.wallet)
    }

    async fn settled_locks(&self) -> Marks {
        Marks {
            outpoints: self.marks(Mark::Settled).await,
            txids: self.settled_txids().await,
        }
    }

    async fn unknown_locks(&self) -> Marks {
        Marks {
            outpoints: self.marks(Mark::Unknown).await,
            txids: HashSet::new(),
        }
    }

    async fn mark_unknown(&self, lock: &KindLock) {
        self.record(Mark::Unknown, lock).await;
    }

    async fn readiness(&self) -> Readiness {
        if let Err(e) = fund::gate(self.lib).await {
            return match e {
                PlatformError::FeatureOff { .. } => Readiness::NotActive(e.code().into()),
                e => Readiness::Held(e.code().into()),
            };
        }
        match self.lib.funding_hold() {
            Some(wait) => Readiness::Held(super::topup::held(wait).code().into()),
            None => Readiness::Ready,
        }
    }

    fn not_active(&self) -> Option<String> {
        let rules = PlatformVersion::get(self.lib.protocol_version()).ok()?;
        platform_moves::sends_active(rules)
            .err()
            .map(|e| e.code().into())
    }

    /// The fund's own minimum (the fee estimate plus one duff), which the
    /// quote refuses below: a lock under it would add nothing.
    fn minimum(&self) -> Option<Minimum> {
        let rules = PlatformVersion::get(self.lib.protocol_version()).ok()?;
        let duffs = fund::minimum_duffs(rules);
        Some(Minimum {
            duffs,
            code: CreditsError::BelowMinimum { min: duffs }.code(),
        })
    }

    fn caught_up(&self) -> bool {
        self.lib.funding_hold().is_none()
    }

    fn wallet_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        fund::fund_lock(self.wallet)
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<ScriptBuf> {
        self.lib.credit_script(outpoint).await
    }

    async fn apply(
        &self,
        outpoint: OutPoint,
        signer: &LockKeySigner<'_>,
    ) -> Result<ApplyReply, PlatformError> {
        let duffs = (self.lib.fund_lock_rows().await.into_iter())
            .find(|l| l.info.outpoint == outpoint)
            .map(|l| l.info.amount)
            .ok_or_else(|| PlatformError::InvalidArgument {
                detail: "no tracked fund lock has that outpoint".into(),
            })?;
        let (to, address) = self.destination(outpoint.txid, duffs).await?;
        // Until the settle says what the rise is (module doc).
        let hold = platform_moves::hold_fund(self.session, self.wallet, address.clone()).await;
        let applied = self.lib.apply(outpoint, to, signer).await;
        (self.session.hub.pump).mark_platform(self.wallet, [PlatformChange::PlatformAddresses]);
        let gained = applied.as_ref().ok().copied().flatten();
        let reply = reply_from(applied.map(drop), outpoint, signer.signed());
        match reply {
            Ok(ApplyReply::Done) => {
                *self.applied.lock().unwrap_or_else(PoisonError::into_inner) = Some(Applied {
                    address,
                    gained,
                    _hold: hold,
                });
            }
            Ok(ApplyReply::AlreadyConsumed) => {
                let estimate = (fund::estimate(self.lib, duffs), true);
                let row = (outpoint.txid.to_string(), duffs);
                if let Err(e) =
                    platform_moves::fund_applied(self.session, self.wallet, row, address, estimate)
                        .await
                {
                    tracing::warn!(%outpoint, error = %e, "could not record a used fund lock in the Platform activity");
                }
            }
            _ => {}
        }
        reply
    }

    async fn settle(&self, lock: &KindLock) {
        let at = lock.info.outpoint;
        self.record(Mark::Settled, lock).await;
        if lock.alone()
            && let Err(e) = (self.session.leases)
                .settle_top_up(self.wallet, at.txid, TopUpSettled::Applied)
                .await
        {
            tracing::warn!(%at, error = %e, "could not record a fund's lock settled");
        }
        let applied = self
            .applied
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        // At most the lock's credits: a rise the cached balance before did
        // not show is not the fund's beyond them.
        let lock_credits = lock.info.amount.saturating_mul(CREDITS_PER_DUFF);
        let credits = (applied.as_ref().and_then(|a| a.gained)).map_or_else(
            || (fund::estimate(self.lib, lock.info.amount), true),
            |g| (g.min(lock_credits), false),
        );
        let address = match &applied {
            Some(a) => Some(a.address.clone()),
            None => (platform_moves::fund_address(self.session, self.wallet, &at.txid.to_string()))
                .await
                .ok()
                .flatten(),
        };
        if let Some(address) = address {
            let row = (at.txid.to_string(), lock.info.amount);
            if let Err(e) =
                platform_moves::fund_applied(self.session, self.wallet, row, address, credits).await
            {
                tracing::warn!(%at, error = %e, "could not record a fund applied in the Platform activity");
            }
        }
        *self.credited.lock().unwrap_or_else(PoisonError::into_inner) = Some(credits.0);
        drop(applied);
        self.lib.sync_now().await;
        (self.session.hub.pump).mark_platform(self.wallet, [PlatformChange::PlatformAddresses]);
    }
}

/// The txids of the wallet's fund locks known finished: the library's
/// proved `Consumed`, or recorded settled (by outpoint or by txid).
pub(crate) async fn finished_funds(
    session: &NetworkSession,
    wallet: WalletId,
    pw: &PlatformWallet,
) -> HashSet<String> {
    let rows = fund::fund_rows(&pw.asset_locks().list_tracked_locks().await);
    let consumed = (rows.into_iter())
        .filter(|l| l.info.status == LockStatus::Consumed)
        .map(|l| l.info.outpoint.txid);
    let marked = locks::marks_of(session, wallet, LockKindId::PlatformAddress, Mark::Settled).await;
    let settled = (session.leases.top_ups_settled(wallet).await).unwrap_or_else(|e| {
        tracing::warn!(wallet_id = %wallet, error = %e, "could not read the settled funds");
        HashSet::new()
    });
    (consumed.chain(marked.into_iter().map(|o| o.txid)))
        .chain(settled)
        .map(|t| t.to_string())
        .collect()
}
