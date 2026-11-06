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
//! receive address. While it runs that address is held out of the activity
//! reads; just before it releases a signature the fund's record is armed
//! (made then for a restored lock, paying that address), and disarmed
//! again if none was released. After a proved apply the settle records the
//! rise as the fund's, at most the lock's credits. Platform's "already
//! consumed" for the exact outpoint is kept ([`Mark::Reported`]) apart from
//! an unknown outcome: the kind takes the lock as finished elsewhere, so it
//! leaves the lists ([`locks::LockFinish::ReportedUsed`]), and the record ends as
//! `platform_moves::fund_used` says (completed with its estimate only for an
//! own fund whose apply released a signature; a restored lock's record is
//! removed). Neither settles the lock.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use dashcore::{OutPoint, ScriptBuf, Txid};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::TopUpSettled;
use dw_vault::{SignerError, VaultError};
use key_wallet::bip32::{DerivationPath, ExtendedPubKey};
use platform_wallet::PlatformWallet;
use platform_wallet::wallet::asset_lock::tracked::AssetLockStatus;

use super::errors::{CreditsError, PlatformError};
use super::flows::FlowKind;
use super::platform_fund::{self as fund, FundLib};
use super::platform_moves::{self, FundEvidence, FundHold};
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

    /// The address the fund paying `recorded` (its record's address, if
    /// any) pays: that one, else the receive address.
    async fn destination(
        &self,
        recorded: Option<&str>,
    ) -> Result<(dpp::address_funds::PlatformAddress, String), PlatformError> {
        if let Some(to) = recorded.and_then(platform_moves::parse) {
            return Ok((to, recorded.unwrap_or_default().to_owned()));
        }
        let to = self.lib.receive_address().await?;
        let address = platform_moves::bech32m(&to, self.lib.network());
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

    async fn reported_locks(&self) -> HashSet<OutPoint> {
        self.marks(Mark::Reported).await
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
        let txid = outpoint.txid.to_string();
        let before = platform_moves::fund_record(self.session, self.wallet, &txid).await?;
        let (to, address) = (self.destination(before.as_ref().map(|r| r.address.as_str()))).await?;
        let estimate = fund::estimate(self.lib, duffs);
        // Until the settle says what the rise is (module doc).
        let hold = platform_moves::hold_fund(self.session, self.wallet, address.clone()).await;
        let signer = ArmingSigner {
            inner: signer,
            session: self.session,
            wallet: self.wallet,
            lock: (outpoint, duffs),
            fund: (&address, estimate),
            arming: tokio::sync::OnceCell::new(),
        };
        let applied = self.lib.apply(outpoint, to, &signer).await;
        (self.session.hub.pump).mark_platform(self.wallet, [PlatformChange::PlatformAddresses]);
        let gained = applied.as_ref().ok().copied().flatten();
        let reply = reply_from(applied.map(drop), outpoint, signer.inner.signed());
        let arming = signer.arming.get().copied();
        if !signer.inner.signed()
            && let Some(arming) = arming
            && let Err(e) =
                platform_moves::disarm_fund(self.session, self.wallet, &txid, arming).await
        {
            tracing::warn!(%outpoint, error = %e, "could not disarm a fund that signed nothing");
        }
        match reply {
            Ok(ApplyReply::Done) => {
                *self.applied.lock().unwrap_or_else(PoisonError::into_inner) = Some(Applied {
                    address,
                    gained,
                    _hold: hold,
                });
            }
            Ok(ApplyReply::AlreadyConsumed) => {
                // Kept first: a failed record write is retried from it.
                locks::record(
                    self.session,
                    self.wallet,
                    LockKindId::PlatformAddress,
                    Mark::Reported,
                    outpoint,
                )
                .await;
                if let Err(e) =
                    platform_moves::fund_used(self.session, self.wallet, &txid, estimate).await
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
            let row = (at, lock.info.amount);
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

/// What the wallet's fund locks' evidence says, by outpoint
/// ([`FundEvidence`]): the library's proved `Consumed` (live, or persisted
/// for a `pending` fund's lock a reopen no longer loads) and the settled
/// records (applied), the reported-used records, the tracked locks, and
/// whether SPV and the wallet's scan caught up. The journal's settled txid
/// counts for a lock alone in its transaction only.
pub(crate) async fn fund_evidence(
    session: &NetworkSession,
    wallet: WalletId,
    pw: &PlatformWallet,
    pending: impl IntoIterator<Item = OutPoint>,
) -> FundEvidence {
    let rows = fund::fund_rows(&pw.asset_locks().list_tracked_locks().await);
    let consumed = (rows.iter())
        .filter(|l| l.info.status == LockStatus::Consumed)
        .map(|l| l.info.outpoint);
    let persisted = persisted_consumed(session, wallet, pending);
    let settled =
        locks::marks_of(session, wallet, LockKindId::PlatformAddress, Mark::Settled).await;
    let by_txid = (session.leases.top_ups_settled(wallet).await).unwrap_or_default();
    let alone = (rows.iter())
        .filter(|l| l.alone() && by_txid.contains(&l.info.outpoint.txid))
        .map(|l| l.info.outpoint);
    FundEvidence {
        done: consumed
            .chain(persisted)
            .chain(settled)
            .chain(alone)
            .collect(),
        reported: locks::marks_of(session, wallet, LockKindId::PlatformAddress, Mark::Reported)
            .await,
        tracked: rows.iter().map(|l| l.info.outpoint).collect(),
        caught_up: super::registration_flow::wallet_synced(session, wallet),
    }
}

/// The outpoints among `pending` whose persisted lock row is `Consumed`.
/// A failed read is no evidence: those funds stay as they are.
fn persisted_consumed(
    session: &NetworkSession,
    wallet: WalletId,
    pending: impl IntoIterator<Item = OutPoint>,
) -> Vec<OutPoint> {
    let persister = match session.live() {
        Ok(live) => live.persister,
        Err(_) => return Vec::new(),
    };
    (pending.into_iter())
        .filter(|at| match persister.asset_lock_status(&wallet.0, at) {
            Ok(status) => status == Some(AssetLockStatus::Consumed),
            Err(e) => {
                tracing::warn!(%at, error = %e, "reading a fund lock's stored status");
                false
            }
        })
        .collect()
}

/// The apply's signer: [`LockKeySigner`], arming the fund's record
/// ([`platform_moves::arm_fund`]) durably before the first signature it
/// asks for, and refusing to sign when that write fails.
struct ArmingSigner<'s, 'a> {
    inner: &'s LockKeySigner<'a>,
    session: &'s NetworkSession,
    wallet: WalletId,
    lock: (OutPoint, u64),
    /// The address the apply pays and the fund's estimate.
    fund: (&'s str, u64),
    /// What arming did, once it ran.
    arming: tokio::sync::OnceCell<platform_moves::Arming>,
}

#[async_trait::async_trait]
impl key_wallet::Signer for ArmingSigner<'_, '_> {
    type Error = SignerError;

    fn supported_methods(&self) -> &[key_wallet::SignerMethod] {
        key_wallet::Signer::supported_methods(self.inner)
    }

    async fn sign_ecdsa(
        &self,
        path: &DerivationPath,
        sighash: [u8; 32],
    ) -> Result<
        (
            dashcore::secp256k1::ecdsa::Signature,
            dashcore::secp256k1::PublicKey,
        ),
        SignerError,
    > {
        // Another key is refused below with nothing released: not armed.
        let key = key_wallet::Signer::public_key(self.inner, path).await?;
        if self.inner.is_the_lock_key(&key) {
            let (address, credits) = self.fund;
            let arm = || async {
                platform_moves::arm_fund(
                    self.session,
                    self.wallet,
                    self.lock,
                    address.into(),
                    credits,
                )
                .await
                .map_err(|e| SignerError::Vault(VaultError::Storage(e.to_string())))
            };
            self.arming.get_or_try_init(arm).await?;
        }
        key_wallet::Signer::sign_ecdsa(self.inner, path, sighash).await
    }

    async fn public_key(
        &self,
        path: &DerivationPath,
    ) -> Result<dashcore::secp256k1::PublicKey, SignerError> {
        key_wallet::Signer::public_key(self.inner, path).await
    }
}

#[async_trait::async_trait]
impl key_wallet::ExtendedPubKeySigner for ArmingSigner<'_, '_> {
    async fn extended_public_key(
        &self,
        path: &DerivationPath,
    ) -> Result<ExtendedPubKey, SignerError> {
        key_wallet::ExtendedPubKeySigner::extended_public_key(self.inner, path).await
    }
}
