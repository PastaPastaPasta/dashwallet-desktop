//! Core → Shielded locks (funding type 5) as an unfinished-lock kind
//! ([`LockKind`]), over the shield flows' own library seam
//! ([`ShieldedLib`]): the same rows, journal records, "consumption unknown"
//! marks and apply as `pending_shields` and `resume_shield`, and the same
//! per-wallet lock. A lock a shield step of this install signed is this
//! device's; any other the library tracks (a restore's, another device's)
//! is restored. The runner's records are by outpoint ([`locks::record`]);
//! 0.1's by txid are kept for a lock alone in its transaction, so 0.1's
//! pending shields agree.

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::{OutPoint, ScriptBuf, Txid};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::version::PlatformVersion;

use super::errors::{PlatformError, ShieldedError, ShieldedSyncWait};
use super::flows::FlowKind;
use super::shielded::SHIELDED_ACTIVATION_VERSION;
use super::shielded_pool::{self as pool, Applied, ShieldedLib};
use super::unfinished_locks::{
    self as locks, ApplyReply, KindLock, LockKeySigner, LockKind, LockKindId, Mark, Marks, Minimum,
    Readiness,
};
use crate::{NetworkSession, PlatformChange, WalletId};

/// The wallet's shield locks as a [`LockKind`].
pub(crate) struct ShieldLocks<'a, L> {
    session: &'a NetworkSession,
    wallet: WalletId,
    lib: &'a L,
}

impl<'a, L: ShieldedLib> ShieldLocks<'a, L> {
    pub(crate) fn new(session: &'a NetworkSession, wallet: WalletId, lib: &'a L) -> Self {
        Self {
            session,
            wallet,
            lib,
        }
    }
}

impl<L: ShieldedLib> LockKind for ShieldLocks<'_, L> {
    fn id(&self) -> LockKindId {
        LockKindId::Shielded
    }

    fn flow(&self) -> FlowKind {
        FlowKind::Shield
    }

    async fn locks(&self) -> Vec<KindLock> {
        self.lib.shield_lock_rows().await
    }

    /// The shield funding steps' signed locks (`shield/<k>/funding`).
    /// MIX-SHIELD's drain of the CoinJoin account funds under the same
    /// steps (its lease is `ShieldDrain`, which the row does not keep), so
    /// a drain lock is this device's and lists once, as a shield lock.
    fn own_locks(&self) -> Option<HashSet<Txid>> {
        self.lib.own_locks()
    }

    async fn settled_locks(&self) -> Marks {
        Marks {
            outpoints: self.marks(Mark::Settled).await,
            txids: self.lib.settled_locks().await,
        }
    }

    async fn unknown_locks(&self) -> Marks {
        Marks {
            outpoints: self.marks(Mark::Unknown).await,
            txids: self.lib.unknown_locks().await,
        }
    }

    async fn mark_unknown(&self, lock: &KindLock) {
        self.record(Mark::Unknown, lock).await;
        if lock.alone() {
            self.lib.mark_unknown(lock.info.outpoint.txid).await;
        }
    }

    async fn readiness(&self) -> Readiness {
        match pool::gate(self.lib).await {
            Ok(_) => {}
            Err(e @ ShieldedError::NotActive { .. }) => {
                return Readiness::NotActive(e.code().into());
            }
            Err(e) => return Readiness::Held(e.code().into()),
        }
        match self.lib.bind().await {
            Ok(true) => Readiness::Ready,
            Ok(false) => Readiness::Held(
                ShieldedError::not_synced(ShieldedSyncWait::Account)
                    .code()
                    .into(),
            ),
            Err(e) => Readiness::Held(e.code().into()),
        }
    }

    fn not_active(&self) -> Option<String> {
        let protocol_version = self.lib.protocol_version();
        (protocol_version < SHIELDED_ACTIVATION_VERSION).then(|| {
            let e = ShieldedError::NotActive {
                protocol_version,
                activation_version: SHIELDED_ACTIVATION_VERSION,
            };
            e.code().into()
        })
    }

    /// The shield's own check before its signature
    /// (`fund_from_asset_lock.rs:290-302`): the lock's value must clear
    /// Platform's pool fee, as 0.1's shield quote reads it.
    fn minimum(&self) -> Option<Minimum> {
        let version = PlatformVersion::get(self.lib.protocol_version()).ok()?;
        let fee = pool::shield_pool_fee(version).ok()?;
        let duffs = fee / CREDITS_PER_DUFF + 1;
        Some(Minimum {
            duffs,
            code: ShieldedError::BelowMinimum { min: duffs }.code(),
        })
    }

    fn caught_up(&self) -> bool {
        self.lib.funding_hold().is_none()
    }

    fn wallet_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        pool::wallet_lock(self.session, self.wallet)
    }

    async fn credit_script(&self, outpoint: OutPoint) -> Option<ScriptBuf> {
        self.lib.credit_script(outpoint).await
    }

    async fn apply(
        &self,
        outpoint: OutPoint,
        signer: &LockKeySigner<'_>,
    ) -> Result<ApplyReply, PlatformError> {
        let applied = self.lib.apply(outpoint, signer).await;
        (self.session.hub.pump).mark_platform(self.wallet, [PlatformChange::Shielded]);
        Ok(match applied? {
            Applied::Done => ApplyReply::Done,
            Applied::Taken => ApplyReply::AlreadyConsumed,
            Applied::Unknown => ApplyReply::Unknown,
        })
    }

    async fn settle(&self, lock: &KindLock) {
        self.record(Mark::Settled, lock).await;
        let txid = lock.info.outpoint.txid;
        if lock.alone()
            && let Err(e) = self.lib.settle(txid).await
        {
            tracing::warn!(%txid, error = %e, "could not record a finished shield lock settled");
        }
        // The shielded note is the library's once a pass finds it.
        self.lib.sync_now().await;
    }
}

impl<L> ShieldLocks<'_, L> {
    async fn marks(&self, mark: Mark) -> HashSet<OutPoint> {
        locks::marks_of(self.session, self.wallet, LockKindId::Shielded, mark).await
    }

    async fn record(&self, mark: Mark, lock: &KindLock) {
        let at = lock.info.outpoint;
        locks::record(self.session, self.wallet, LockKindId::Shielded, mark, at).await;
    }
}
