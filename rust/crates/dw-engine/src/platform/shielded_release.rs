//! What the dispatch hook proves of a shielded spend before anything of it
//! is written or sent, and what the journal proves of one after (#444).
//!
//! The hook refuses a spend whose signed bundle's anchor and nullifiers
//! are not the ones the library hands it ([`bundle_matches`]), and one
//! whose anchor a proved read of Platform's anchors tree does not find
//! recorded ([`anchor_recorded_at_dispatch`]); the height of an
//! SPV-verified such read is kept durably in app.sqlite (the spend's Hp:
//! only an SPV-verified read counts, the trusted-only toggle
//! notwithstanding).
//!
//! The pinned library holds a spend refused at its first broadcast itself
//! and releases its notes once its anchor is proved pruned or its
//! nullifiers land; the engine reads that outcome from the spend's history
//! row (`shielded_pool::settle_outcomes`) and releases nothing itself.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::{NoParamQuery, ShieldedAnchors};
use dpp::serialization::PlatformDeserializableTrusted;
use dpp::shielded::SerializedAction;
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_top_up_from_shielded_pool_transition::accessors::IdentityTopUpFromShieldedPoolTransitionAccessorsV0;
use dpp::state_transition::shielded_transfer_transition::accessors::ShieldedTransferTransitionAccessorsV0;
use dpp::state_transition::shielded_withdrawal_transition::accessors::ShieldedWithdrawalTransitionAccessorsV0;
use dpp::state_transition::state_transitions::shielded::identity_create_from_shielded_pool_transition::accessors::IdentityCreateFromShieldedPoolTransitionAccessorsV0;
use dpp::state_transition::unshield_transition::accessors::UnshieldTransitionAccessorsV0;
use dw_appdb::dispatch::{ArtifactStatus, ShieldedArtifact};
use platform_wallet::wallet::shielded::{ShieldedDispatchKind, ShieldedDispatchPayload};

use super::trust::BoxFuture;
use crate::session::Manager;
use crate::{NetworkSession, WalletId};

/// The disposition the 0.1 library wrote for a refused first send, as the
/// journal keeps it (`kind_str`, `reason_str`).
const CANCELLED: (&str, Option<&str>) = ("unresolved", Some("cancelled"));

/// How long one transport attempt of a dispatch read waits.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A proved answer: its value, the height and block time (ms) of the
/// response it came in, and whether every quorum key its proof used was
/// SPV-verified (set by [`scoped`], the reads leave it `false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proved<T> {
    pub(crate) value: T,
    pub(crate) height: u64,
    pub(crate) time_ms: u64,
    pub(crate) verified: bool,
}

/// `read` in its own trust scope, its provenance recorded.
async fn scoped<T>(read: BoxFuture<Option<Proved<T>>>) -> Option<Proved<T>> {
    let (read, tally) = super::trust::scoped(read).await;
    read.map(|p| Proved {
        verified: tally.verified(),
        ..p
    })
}

/// Platform's proved reads the dispatch hook takes: the SDK, or a test's
/// script. Each targets the credit pool, the one every spend the engine sends
/// spends from. `None` when the read failed or came unproved.
pub(crate) trait ReleaseNet: Send + Sync {
    /// The whole anchors tree.
    fn anchors(&self) -> BoxFuture<Option<Proved<Vec<[u8; 32]>>>>;
}

#[cfg_attr(test, allow(dead_code))]
struct SdkReads(Arc<dash_sdk::Sdk>);

#[cfg_attr(test, allow(dead_code))]
fn settings() -> dash_sdk::RequestSettings {
    dash_sdk::RequestSettings {
        timeout: Some(READ_TIMEOUT),
        retries: Some(1),
        ban_failed_address: Some(false),
        ..Default::default()
    }
}

impl ReleaseNet for SdkReads {
    fn anchors(&self) -> BoxFuture<Option<Proved<Vec<[u8; 32]>>>> {
        let sdk = Arc::clone(&self.0);
        Box::pin(async move {
            if !sdk.prove() {
                return None;
            }
            match ShieldedAnchors::fetch_with_metadata(&sdk, NoParamQuery {}, Some(settings()))
                .await
            {
                Ok((anchors, metadata)) => Some(Proved {
                    value: anchors.map(|a| a.0).unwrap_or_default(),
                    height: metadata.height,
                    time_ms: metadata.time_ms,
                    verified: false,
                }),
                Err(e) => {
                    tracing::debug!(error = %e, "the shielded anchors were not read");
                    None
                }
            }
        })
    }
}

/// Tests read through their script; with none, every read fails.
#[cfg(test)]
fn release_net(session: &NetworkSession, _: &Manager) -> Arc<dyn ReleaseNet> {
    struct Offline;
    impl ReleaseNet for Offline {
        fn anchors(&self) -> BoxFuture<Option<Proved<Vec<[u8; 32]>>>> {
            Box::pin(async { None })
        }
    }
    (super::runtime::guard(&session.platform.release_net).clone())
        .unwrap_or_else(|| Arc::new(Offline))
}

#[cfg(not(test))]
fn release_net(_: &NetworkSession, manager: &Manager) -> Arc<dyn ReleaseNet> {
    Arc::new(SdkReads(manager.sdk_arc()))
}

/// The app.sqlite key of the highest height `st_hash`'s anchor was proved
/// recorded at. It is written at dispatch before the spend is sent, and a
/// refusal after that write leaves it: a stored height alone never proves
/// the spend was dispatched. A reader takes it as that proof only together
/// with the spend's artifact in the journal, or the row is cleared on each
/// refusal before the send.
fn presence_key(st_hash: [u8; 32]) -> String {
    format!("shielded_anchor_recorded/{}", hex::encode(st_hash))
}

/// A stored height: digits only, as app.sqlite's raise reads one.
fn stored_height(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// The highest height the anchor of `wallet`'s spend `st_hash` was proved
/// recorded at (`Some(None)`: none); `None` when it cannot be read.
pub(crate) async fn presence(
    session: &NetworkSession,
    wallet: WalletId,
    st_hash: [u8; 32],
) -> Option<Option<u64>> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = presence_key(st_hash);
    let value = (session.appdb_op(move |db| db.setting(&scope, &key)).await).ok()?;
    match value {
        None => Some(None),
        Some(v) => stored_height(&v).map(Some),
    }
}

/// Records that the anchor of `wallet`'s spend `st_hash` was proved recorded
/// at `height`, keeping the highest, durably against power loss once it
/// returns `Ok`. A stored height at least `height` is left as it is, with
/// no write. A stored value that is not a height (digits only) is replaced,
/// as any one proved height orders the release; until then the wallet's
/// pass reads nothing (#912). An error when what is stored after is not a
/// height of at least `height`.
pub(crate) async fn record_presence(
    session: &NetworkSession,
    wallet: WalletId,
    st_hash: [u8; 32],
    height: u64,
) -> Result<(), String> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let key = presence_key(st_hash);
    let stored = (session.appdb_op(move |db| {
        if let Some(known) = db.setting(&scope, &key)?
            && stored_height(&known).is_some_and(|k| k >= height)
        {
            return Ok(known);
        }
        let stored = db.raise_setting_durable(&scope, &key, height)?;
        if stored_height(&stored).is_some() {
            return Ok(stored);
        }
        let height = height.to_string();
        db.set_setting_durable(&scope, &key, Some(&height))?;
        Ok(height)
    }))
    .await
    .map_err(|e| e.to_string())?;
    match stored_height(&stored) {
        Some(kept) if kept >= height => Ok(()),
        _ => Err(format!(
            "the stored height is {stored:?}, not at least {height}"
        )),
    }
}

/// The dispatch hook's chain-order read, before anything of the spend is
/// written or sent: one proved anchors read must find the spend's anchor
/// recorded, and its height is kept as the spend's Hp when the read is
/// SPV-verified, whatever rule 5 says. A failed read, or one without the
/// anchor, or a height not kept, refuses the spend. Tests without a
/// script of the reads model one that proves the anchor recorded at
/// height 1.
pub(crate) async fn anchor_recorded_at_dispatch(
    session: &NetworkSession,
    payload: &ShieldedDispatchPayload,
) -> Result<(), String> {
    let wallet = WalletId(payload.wallet_id);
    #[cfg(test)]
    if super::runtime::guard(&session.platform.release_net).is_none() {
        return (record_presence(session, wallet, payload.st_hash, 1).await)
            .map_err(|e| format!("the anchor's height was not kept: {e}"));
    }
    let manager = session.manager().map_err(|e| e.to_string())?;
    let net = release_net(session, &manager);
    let Some(anchors) = scoped(net.anchors()).await else {
        return Err("Platform's recorded anchors could not be read".into());
    };
    if !anchors.value.contains(&payload.anchor) {
        return Err("the spend's anchor is not recorded on Platform".into());
    }
    if anchors.verified {
        (record_presence(session, wallet, payload.st_hash, anchors.height).await)
            .map_err(|e| format!("the anchor's height was not kept: {e}"))?;
    }
    Ok(())
}

/// Whether the journal keeps the library's `Unresolved(Cancelled)` for `a`.
pub(crate) fn disposition_is_cancelled(a: &ShieldedArtifact) -> bool {
    a.disposition
        .as_ref()
        .is_some_and(|(kind, reason)| (kind.as_str(), reason.as_deref()) == CANCELLED)
}

/// Whether the journal proves `a`'s transition executed: settled `executed`
/// on the library's own `executed` disposition, or by the 0.1 build's
/// release of a refused first send that published a padding nullifier (one of no
/// selected note: a random dummy note's, which no other spend can spend,
/// so all published spent is this transition). A release-settled spend of
/// only real notes may have been spent by another device holding the same
/// notes, which proves nothing of this transition.
pub(crate) fn proves_execution(a: &ShieldedArtifact) -> bool {
    let padded = || {
        let selected: HashSet<[u8; 32]> = a.selected.iter().map(|&(n, _)| n).collect();
        a.published.iter().any(|n| !selected.contains(n))
    };
    a.status == ArtifactStatus::Executed
        && ((a.disposition.as_ref()).is_some_and(|(kind, _)| kind == "executed")
            || (disposition_is_cancelled(a) && padded()))
}

/// The anchor and the action nullifiers of the bundle in signed transition
/// `st_bytes`, when it is of `kind`'s family.
fn bundle(kind: &ShieldedDispatchKind, st_bytes: &[u8]) -> Option<([u8; 32], Vec<[u8; 32]>)> {
    let st = StateTransition::deserialize_from_bytes_trusted(st_bytes).ok()?;
    let nullifiers = |actions: &[SerializedAction]| actions.iter().map(|a| a.nullifier).collect();
    match (kind, &st) {
        (ShieldedDispatchKind::Transfer, StateTransition::ShieldedTransfer(t)) => {
            Some((t.anchor(), nullifiers(t.actions())))
        }
        (ShieldedDispatchKind::Unshield, StateTransition::Unshield(t)) => {
            Some((t.anchor(), nullifiers(t.actions())))
        }
        (ShieldedDispatchKind::Withdraw, StateTransition::ShieldedWithdrawal(t)) => {
            Some((t.anchor(), nullifiers(t.actions())))
        }
        (
            ShieldedDispatchKind::IdentityTopUp { .. },
            StateTransition::IdentityTopUpFromShieldedPool(t),
        ) => Some((t.anchor(), nullifiers(t.actions()))),
        (
            ShieldedDispatchKind::IdentityCreate { .. },
            StateTransition::IdentityCreateFromShieldedPool(t),
        ) => Some((t.anchor(), nullifiers(t.actions()))),
        _ => None,
    }
}

/// Whether the signed bytes of `payload` carry the anchor and the
/// nullifiers the library hands the hook with them, and are of its kind:
/// the journal keeps both in the artifact, so a spend whose bytes say
/// otherwise is refused before it is written or sent.
pub(crate) fn bundle_matches(payload: &ShieldedDispatchPayload) -> bool {
    bundle(&payload.kind, &payload.st_bytes).is_some_and(|(anchor, nullifiers)| {
        anchor == payload.anchor && nullifiers == payload.published_nullifiers
    })
}
