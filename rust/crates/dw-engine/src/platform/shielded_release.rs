//! Releasing the notes of a shielded spend Platform refused at its first
//! broadcast (#444). The library records such a spend `Unresolved(Cancelled)`
//! and holds its notes until the host acknowledges it; the refusal proves
//! nothing, as the spend may still execute. The engine acknowledges it only
//! on proved evidence that it never executed and never can:
//! - its anchor is proved absent from Platform's anchors tree at height Ha
//!   (Platform executes a spend only while its anchor is recorded, and a
//!   pruned anchor never returns), at a block time more than
//!   [`ANCHOR_READ_MARGIN`] after the refusal, so Ha is above the height the
//!   spend was built at and the anchor is not merely unrecorded yet;
//! - each of its nullifiers is proved unspent at height Hn ≥ Ha, so no block
//!   up to Hn executed it;
//! - the library knows each note it holds, unspent, and the journal flags
//!   none of them spent.
//!
//! The same one nullifier read settles the residual case: every nullifier
//! it publishes proved spent means it executed; some but not all means it
//! never executed and never can (a transition inserts all its nullifiers at
//! once, and a spent one fails it). Anything else, a failed or unproved
//! read included, holds.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::{
    NoParamQuery, ShieldedAnchors, ShieldedNullifierStatuses, ShieldedNullifiersQuery,
};
use dpp::serialization::PlatformDeserializableTrusted;
use dpp::shielded::SerializedAction;
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_top_up_from_shielded_pool_transition::accessors::IdentityTopUpFromShieldedPoolTransitionAccessorsV0;
use dpp::state_transition::shielded_transfer_transition::accessors::ShieldedTransferTransitionAccessorsV0;
use dpp::state_transition::shielded_withdrawal_transition::accessors::ShieldedWithdrawalTransitionAccessorsV0;
use dpp::state_transition::state_transitions::shielded::identity_create_from_shielded_pool_transition::accessors::IdentityCreateFromShieldedPoolTransitionAccessorsV0;
use dpp::state_transition::unshield_transition::accessors::UnshieldTransitionAccessorsV0;
use dw_appdb::dispatch::{ArtifactOrigin, ArtifactStatus, ShieldedArtifact};
use platform_wallet::wallet::shielded::{
    NetworkShieldedCoordinator, ShieldedDispatchKind, ShieldedDispatchPayload,
    ShieldedSpendDisposition, ShieldedStore, SubwalletId, UnresolvedReason,
};

use super::shielded::{ShieldedActivity, ShieldedActivityStatus, ShieldedRefusal};
use super::shielded_pool::settle_then_ack;
use super::trust::BoxFuture;
use crate::session::Manager;
use crate::{NetworkSession, WalletId};

/// The disposition the library writes for a refused first send, as the
/// journal keeps it (`kind_str`, `reason_str`).
const CANCELLED: (&str, Option<&str>) = ("unresolved", Some("cancelled"));

/// How long after a read that found a spend's anchor still recorded the
/// spend is read again.
pub(crate) const ANCHOR_RECHECK: Duration = Duration::from_secs(600);

/// How long one transport attempt of a release read waits.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How much later than the refusal the anchors read's block time must be.
/// The SDK accepts a response only within 31 minutes of the local clock,
/// so the block the spend was built on is at most that far past the local
/// time the library stamped the refusal with; a block time past this
/// margin is from a later block, whatever the local clock says.
pub(crate) const ANCHOR_READ_MARGIN: Duration = Duration::from_secs(45 * 60);

/// The most nullifiers Platform answers in one read; the release packs
/// whole spends into reads up to it.
pub(crate) const NULLIFIER_READ_LIMIT: usize = 100;

/// A proved answer: its value, and the height and block time (ms) of the
/// response it came in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proved<T> {
    pub(crate) value: T,
    pub(crate) height: u64,
    pub(crate) time_ms: u64,
}

/// A proved nullifier read: each nullifier with whether it is spent.
pub(crate) type Statuses = Proved<Vec<([u8; 32], bool)>>;

/// Platform's proved reads the release takes: the SDK, or a test's script.
/// Each targets the credit pool, the one every spend the engine sends
/// spends from. `None` when the read failed or came unproved.
pub(crate) trait ReleaseNet: Send + Sync {
    /// The whole anchors tree.
    fn anchors(&self) -> BoxFuture<Option<Proved<Vec<[u8; 32]>>>>;
    /// Each of `nullifiers` with whether it is spent.
    fn nullifiers(&self, nullifiers: Vec<[u8; 32]>) -> BoxFuture<Option<Statuses>>;
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
                }),
                Err(e) => {
                    tracing::debug!(error = %e, "the shielded anchors were not read");
                    None
                }
            }
        })
    }

    fn nullifiers(&self, nullifiers: Vec<[u8; 32]>) -> BoxFuture<Option<Statuses>> {
        let sdk = Arc::clone(&self.0);
        Box::pin(async move {
            if !sdk.prove() {
                return None;
            }
            let query = ShieldedNullifiersQuery(nullifiers);
            match ShieldedNullifierStatuses::fetch_with_metadata(&sdk, query, Some(settings()))
                .await
            {
                Ok((statuses, metadata)) => Some(Proved {
                    value: (statuses.map(|s| s.0).unwrap_or_default().into_iter())
                        .map(|s| (s.nullifier, s.is_spent))
                        .collect(),
                    height: metadata.height,
                    time_ms: metadata.time_ms,
                }),
                Err(e) => {
                    tracing::debug!(error = %e, "the shielded nullifiers were not read");
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
        fn nullifiers(&self, _: Vec<[u8; 32]>) -> BoxFuture<Option<Statuses>> {
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

/// A refused first send whose notes the library holds: its library
/// disposition and its journal artifact, as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Held {
    pub(crate) subwallet: SubwalletId,
    pub(crate) st_hash: [u8; 32],
    /// When the library wrote the disposition (ms).
    pub(crate) at_ms: u64,
    pub(crate) held: BTreeSet<[u8; 32]>,
    pub(crate) anchor: Option<[u8; 32]>,
    pub(crate) selected: Vec<([u8; 32], bool)>,
    pub(crate) published: Vec<[u8; 32]>,
}

impl Held {
    /// Every nullifier the evidence reads: held ∪ selected ∪ published.
    pub(crate) fn nullifiers(&self) -> BTreeSet<[u8; 32]> {
        let mut all = self.held.clone();
        all.extend(self.selected.iter().map(|&(n, _)| n));
        all.extend(self.published.iter().copied());
        all
    }
}

/// The refused first sends of `wallet` the release may settle: each
/// `Unresolved(Cancelled)` disposition holding notes whose one artifact was
/// written by the hook, reads `unresolved` with the library's disposition,
/// and is not acknowledged. `None` when either side cannot be read.
async fn held(
    session: &NetworkSession,
    coordinator: &NetworkShieldedCoordinator,
    wallet: WalletId,
) -> Option<(Vec<Held>, HashSet<[u8; 32]>)> {
    let artifacts = (session.leases.shielded_artifacts(wallet).await).ok()?;
    let dispositions = (coordinator.spend_dispositions(wallet.0).await).ok()?;
    let journal_spent: HashSet<[u8; 32]> = (artifacts.iter())
        .flat_map(|a| &a.selected)
        .filter_map(|&(n, spent)| spent.then_some(n))
        .collect();
    let mut found = Vec::new();
    for d in dispositions {
        if d.disposition != ShieldedSpendDisposition::Unresolved(UnresolvedReason::Cancelled)
            || d.held_nullifiers.is_empty()
        {
            continue;
        }
        let mut of_spend = (artifacts.iter()).filter(|a| a.st_hash == Some(d.st_hash));
        let (Some(artifact), None) = (of_spend.next(), of_spend.next()) else {
            continue;
        };
        if artifact.activity != d.activity_id || !standing(artifact) {
            continue;
        }
        found.push(Held {
            subwallet: d.subwallet,
            st_hash: d.st_hash,
            at_ms: d.at_ms,
            held: d.held_nullifiers.iter().copied().collect(),
            anchor: artifact.anchor,
            selected: artifact.selected.clone(),
            published: artifact.published.clone(),
        });
    }
    Some((found, journal_spent))
}

fn standing(a: &ShieldedArtifact) -> bool {
    a.origin == ArtifactOrigin::Dispatched
        && a.status == (ArtifactStatus::Unresolved { unrecorded: false })
        && disposition_is_cancelled(a)
        && !a.acked
}

/// Whether the journal keeps the library's `Unresolved(Cancelled)` for `a`.
pub(crate) fn disposition_is_cancelled(a: &ShieldedArtifact) -> bool {
    a.disposition
        .as_ref()
        .is_some_and(|(kind, reason)| (kind.as_str(), reason.as_deref()) == CANCELLED)
}

/// What the evidence proves of one held spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Never executed and never can: its notes go back.
    NotExecuted,
    /// Executed: settled so; its notes stay held until the journal reads
    /// them spent.
    Executed,
    /// Its anchor is still recorded: read again after [`ANCHOR_RECHECK`].
    AnchorRecorded,
    /// Not proved either way: read again at the next pass.
    Hold,
}

/// The verdict on `spend` from the anchors read and the nullifier read,
/// with the library's notes of the account and the journal's spent flags.
pub(crate) fn verdict(
    spend: &Held,
    anchors: Option<&Proved<Vec<[u8; 32]>>>,
    nullifiers: &Statuses,
    notes: &HashMap<[u8; 32], bool>,
    journal_spent: &HashSet<[u8; 32]>,
) -> Verdict {
    let published: BTreeSet<[u8; 32]> = spend.published.iter().copied().collect();
    if published.len() != spend.published.len()
        || !spend.held.is_subset(&published)
        || !(spend.selected.iter()).all(|(n, _)| published.contains(n))
    {
        return Verdict::Hold;
    }
    let statuses: HashMap<[u8; 32], bool> = nullifiers.value.iter().copied().collect();
    let Some(spent) = (spend.nullifiers().iter())
        .map(|n| statuses.get(n).copied())
        .collect::<Option<Vec<bool>>>()
    else {
        return Verdict::Hold;
    };
    let published_spent = (spend.published.iter()).filter(|n| statuses[*n]).count();
    if published_spent == spend.published.len() {
        return Verdict::Executed;
    }
    if published_spent > 0 {
        // Each held note Platform reads spent must read spent locally too
        // before the others go back, or the next spend could select it.
        let local = (spend.held.iter())
            .filter(|n| statuses[*n])
            .all(|n| notes.get(n) == Some(&true));
        return if local {
            Verdict::NotExecuted
        } else {
            Verdict::Hold
        };
    }
    if spent.iter().any(|&s| s) {
        return Verdict::Hold;
    }
    let (Some(anchor), Some(anchors)) = (spend.anchor, anchors) else {
        return Verdict::Hold;
    };
    if anchors.value.contains(&anchor) {
        return Verdict::AnchorRecorded;
    }
    let known_unspent = (spend.held.iter()).all(|n| notes.get(n) == Some(&false));
    let flagged = (spend.held.iter()).any(|n| journal_spent.contains(n));
    let margin = u64::try_from(ANCHOR_READ_MARGIN.as_millis()).unwrap_or(u64::MAX);
    if nullifiers.height < anchors.height
        || anchors.time_ms <= spend.at_ms.saturating_add(margin)
        || !known_unspent
        || flagged
    {
        return Verdict::Hold;
    }
    Verdict::NotExecuted
}

/// The library's notes of `subwallet`, by nullifier, with whether each is
/// spent; `None` when they cannot be read.
async fn notes(
    coordinator: &NetworkShieldedCoordinator,
    subwallet: SubwalletId,
) -> Option<HashMap<[u8; 32], bool>> {
    let store = coordinator.store().read().await;
    let notes = store.get_all_notes(subwallet).ok()?;
    Some(
        notes
            .into_iter()
            .map(|n| (n.nullifier, n.is_spent))
            .collect(),
    )
}

/// `spends` packed into nullifier reads of at most [`NULLIFIER_READ_LIMIT`]
/// each, by index. A spend is never split: its statuses all come from one
/// read at one height.
pub(crate) fn batches(spends: &[Held]) -> Vec<Vec<usize>> {
    let mut out: Vec<(Vec<usize>, BTreeSet<[u8; 32]>)> = Vec::new();
    for (i, spend) in spends.iter().enumerate() {
        let wanted = spend.nullifiers();
        match out.last_mut() {
            Some((of, all)) if all.union(&wanted).count() <= NULLIFIER_READ_LIMIT => {
                of.push(i);
                all.extend(wanted);
            }
            _ => out.push((vec![i], wanted)),
        }
    }
    out.into_iter().map(|(of, _)| of).collect()
}

/// Settles each refused first send of `wallet` the evidence proves final
/// (the module's rule), inside a shielded pass: under its locks, so no spend
/// runs meanwhile, after [`super::shielded_pool::consume_dispositions`] and
/// before the contact payouts settle on the artifacts. The journal settle
/// commits before the library's acknowledgement; a crash between the two is
/// acknowledged at the next pass. No held spend, no read.
pub(crate) async fn release_final_rejections(
    session: &NetworkSession,
    manager: &Manager,
    coordinator: &NetworkShieldedCoordinator,
    wallet: WalletId,
) {
    if !session.leases.journal_ready() {
        return;
    }
    let Some((found, _)) = held(session, coordinator, wallet).await else {
        return;
    };
    let now = Instant::now();
    let due: Vec<Held> = {
        let mut backoff =
            (session.platform.release_backoff.lock()).unwrap_or_else(PoisonError::into_inner);
        backoff.retain(|_, until| *until > now);
        (found.into_iter())
            .filter(|h| !backoff.contains_key(&h.st_hash))
            .collect()
    };
    if due.is_empty() {
        return;
    }
    let net = release_net(session, manager);
    let anchors = net.anchors().await;
    let mut read: HashMap<[u8; 32], Arc<Statuses>> = HashMap::new();
    for batch in batches(&due) {
        let wanted: BTreeSet<[u8; 32]> = batch.iter().flat_map(|&i| due[i].nullifiers()).collect();
        let Some(statuses) = net.nullifiers(wanted.into_iter().collect()).await else {
            tracing::debug!(wallet_id = %wallet, "refused shielded spends stay held: their nullifiers were not read");
            break;
        };
        let statuses = Arc::new(statuses);
        read.extend(
            batch
                .iter()
                .map(|&i| (due[i].st_hash, Arc::clone(&statuses))),
        );
    }
    if read.is_empty() {
        return;
    }
    #[cfg(test)]
    {
        let pause = (session.platform.release_pause.lock())
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            let _ = pause.resume.await;
        }
    }
    // Read again after the network: the settle needs the very spend the
    // evidence was read for (E4).
    let Some((now_held, journal_spent)) = held(session, coordinator, wallet).await else {
        return;
    };
    for spend in due {
        let Some(nullifiers) = read.get(&spend.st_hash) else {
            continue;
        };
        if !now_held.contains(&spend) {
            continue;
        }
        let Some(notes) = notes(coordinator, spend.subwallet).await else {
            continue;
        };
        let verdict = verdict(&spend, anchors.as_ref(), nullifiers, &notes, &journal_spent);
        let transition = hex::encode(spend.st_hash);
        let (status, ack) = match verdict {
            Verdict::NotExecuted => (ArtifactStatus::NotExecuted, true),
            Verdict::Executed => {
                let durable = spend.held.iter().all(|n| journal_spent.contains(n));
                (ArtifactStatus::Executed, durable)
            }
            Verdict::AnchorRecorded => {
                (session.platform.release_backoff.lock())
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(spend.st_hash, now + ANCHOR_RECHECK);
                tracing::debug!(wallet_id = %wallet, %transition, "a refused shielded spend stays held: its anchor is still recorded");
                continue;
            }
            Verdict::Hold => {
                tracing::debug!(wallet_id = %wallet, %transition, "a refused shielded spend stays held: the evidence does not prove it final");
                continue;
            }
        };
        tracing::info!(
            wallet_id = %wallet,
            %transition,
            ?verdict,
            anchors_height = anchors.as_ref().map(|a| a.height),
            nullifiers_height = nullifiers.height,
            "a refused shielded spend is settled on proved evidence"
        );
        settle_then_ack(
            session,
            coordinator,
            spend.subwallet,
            spend.st_hash,
            status,
            Some(CANCELLED),
            ack,
        )
        .await;
    }
}

/// How [`super::shielded_pool::consume_dispositions`] reads a library
/// `Unresolved(Cancelled)` whose artifact the release already settled: a
/// `not_executed` one acknowledged in the journal is acknowledged again (a
/// crash came between the two), an `executed` one stays executed and is
/// acknowledged once `durable`. `None` for any other.
pub(crate) fn settled_by_release(
    artifacts: &[ShieldedArtifact],
    st_hash: [u8; 32],
    durable: bool,
) -> Option<(ArtifactStatus, bool)> {
    let mut of_spend = (artifacts.iter()).filter(|a| a.st_hash == Some(st_hash));
    let (Some(a), None) = (of_spend.next(), of_spend.next()) else {
        return None;
    };
    if a.origin != ArtifactOrigin::Dispatched || !disposition_is_cancelled(a) {
        return None;
    }
    match a.status {
        ArtifactStatus::NotExecuted if a.acked => Some((ArtifactStatus::NotExecuted, true)),
        ArtifactStatus::Executed => Some((ArtifactStatus::Executed, durable)),
        _ => None,
    }
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
/// the release reads both from the artifact, so a spend whose bytes say
/// otherwise is refused before it is written or sent.
pub(crate) fn bundle_matches(payload: &ShieldedDispatchPayload) -> bool {
    bundle(&payload.kind, &payload.st_bytes).is_some_and(|(anchor, nullifiers)| {
        anchor == payload.anchor && nullifiers == payload.published_nullifiers
    })
}

/// Reads each of `rows` that is a refused first send from the library's
/// outcome and its journal artifact rather than the library's row, which
/// reads failed from the refusal on: held (pending, never failed) while the
/// library holds its notes, released once settled `not_executed` and no
/// longer held (held while the library's outcomes cannot be read),
/// confirmed once settled `executed`. Rows stay as they are where neither
/// reads the spend refused.
pub(crate) async fn mark_refused(
    session: &NetworkSession,
    wallet: WalletId,
    rows: &mut [ShieldedActivity],
) {
    let open = |s: &ShieldedActivityStatus| {
        matches!(
            s,
            ShieldedActivityStatus::Pending | ShieldedActivityStatus::Failed
        )
    };
    if !rows.iter().any(|r| open(&r.status)) {
        return;
    }
    let cancelled = ShieldedSpendDisposition::Unresolved(UnresolvedReason::Cancelled);
    let mut held: Option<HashSet<String>> = None;
    if let Ok(manager) = session.manager()
        && let Some(coordinator) = manager.shielded_coordinator().await
        && let Ok(dispositions) = coordinator.spend_dispositions(wallet.0).await
    {
        held = Some(
            (dispositions.into_iter())
                .filter(|d| d.disposition == cancelled)
                .map(|d| hex::encode(d.activity_id))
                .collect(),
        );
    }
    let mut settled: HashMap<String, ArtifactStatus> = HashMap::new();
    if session.leases.journal_ready()
        && let Ok(artifacts) = session.leases.shielded_artifacts(wallet).await
    {
        settled.extend(
            (artifacts.iter())
                .filter(|a| a.origin == ArtifactOrigin::Dispatched && disposition_is_cancelled(a))
                .map(|a| (hex::encode(a.activity), a.status)),
        );
    }
    for row in rows.iter_mut().filter(|r| open(&r.status)) {
        let library_holds = held.as_ref().is_none_or(|held| held.contains(&row.id));
        let (status, refusal) = match settled.get(&row.id) {
            Some(ArtifactStatus::Executed) => (ShieldedActivityStatus::Confirmed, None),
            Some(_) if library_holds => {
                (ShieldedActivityStatus::Pending, Some(ShieldedRefusal::Held))
            }
            None if held.as_ref().is_some_and(|held| held.contains(&row.id)) => {
                (ShieldedActivityStatus::Pending, Some(ShieldedRefusal::Held))
            }
            Some(ArtifactStatus::Unresolved { .. }) => {
                (ShieldedActivityStatus::Pending, Some(ShieldedRefusal::Held))
            }
            Some(ArtifactStatus::NotExecuted) => (
                ShieldedActivityStatus::Failed,
                Some(ShieldedRefusal::Released),
            ),
            _ => continue,
        };
        row.status = status;
        row.refusal = refusal;
    }
}
