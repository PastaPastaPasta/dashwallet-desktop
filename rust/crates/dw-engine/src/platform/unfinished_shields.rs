//! Core → Shielded locks (funding type 5) as an unfinished-lock kind
//! ([`LockKind`]), over the shield flows' own library seam
//! ([`ShieldedLib`]): the same rows, journal records, "consumption unknown"
//! marks and apply as `pending_shields` and `resume_shield`, and the same
//! per-wallet lock. A lock a shield step of this install signed is this
//! device's; any other the library tracks (a restore's, another device's)
//! is restored.

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::{OutPoint, ScriptBuf, Txid};

use super::errors::{PlatformError, ShieldedError, ShieldedSyncWait};
use super::flows::FlowKind;
use super::registration_flow::LockInfo;
use super::shielded::SHIELDED_ACTIVATION_VERSION;
use super::shielded_pool::{self as pool, Applied, ShieldedLib};
use super::unfinished_locks::{ApplyReply, LockKeySigner, LockKind, LockKindId, Readiness};
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

    async fn locks(&self) -> Vec<LockInfo> {
        self.lib.shield_locks().await
    }

    fn own_locks(&self) -> Option<HashSet<Txid>> {
        self.lib.own_locks()
    }

    async fn settled_locks(&self) -> HashSet<Txid> {
        self.lib.settled_locks().await
    }

    async fn unknown_locks(&self) -> HashSet<Txid> {
        self.lib.unknown_locks().await
    }

    async fn mark_unknown(&self, txid: Txid) {
        self.lib.mark_unknown(txid).await;
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

    async fn settle(&self, txid: Txid) {
        if let Err(e) = self.lib.settle(txid).await {
            tracing::warn!(%txid, error = %e, "could not record a finished shield lock settled");
        }
        // The shielded note is the library's once a pass finds it.
        self.lib.sync_now().await;
    }
}
