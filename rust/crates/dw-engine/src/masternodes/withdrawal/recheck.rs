//! Recheck (MG-13 P8), marks (P9) and late proof: each undecided nonce
//! group, oldest first, under the app-wide identity lock.
//!
//! 1. **Documents.** Every record's candidate documents (P7), read at or
//!    above the group's floor: a match makes that record `done` and its
//!    siblings `sibling_paid` (equal-amount siblings: the newest is `done`).
//! 2. **No match: the proved nonce state**, read at a height H at or above
//!    the floor (an SPV-verified read raises the floor to H, group-wide):
//!    - n taken by Drive (`takes`): the open record's stored bytes again,
//!      unchanged (a `superseded` sibling is never sent again);
//!    - n above the tip: nothing is decided (DP6-02's future-state guard);
//!    - n at or below the tip and not taken: `not_executed` for the whole
//!      group needs (i) that read, (ii) the owner-key candidates from an
//!      identity proved at H or above, (iii) Drive's mapping, (iv) no
//!      matching document at H or above for any sibling and candidate
//!      (absent, or present with other effect fields).
//! 3. **Undecidable** (a read below the floor or H, a failed read, a read
//!    rule 5 does not pass under `Enforce`): the round again; after
//!    `READ_BACK_TRIES` rounds the open record is `couldnt_confirm`.
//!
//! Under `Enforce`, step 1's positive match, (i), (ii) and (iv) must pass
//! rule 5 (P8 step 4, P12). No path signs again.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use dpp::document::Document;
use dpp::identity::Identity;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_nonce::IDENTITY_NONCE_VALUE_FILTER;
use dpp::prelude::Identifier;
use dw_appdb::{
    NotExecutedEvidence, WithdrawalMark, WithdrawalNote, WithdrawalProof, WithdrawalRow,
    WithdrawalState,
};

use super::check::{Gated, gated};
use super::drive::{candidates, document_of, matches, needs_identity, status_of};
use super::net::MnWithdrawNet;
use super::sign::{hand_off, nonce_read, shown};
use super::{Alarm, Env, Stage, WithdrawalError, fault, identity_lock, points};
use crate::platform::withdrawal::{READ_BACK_TRIES, takes};

/// How one group's recheck ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A record is `done`, its siblings `sibling_paid`.
    Done,
    /// The whole group is `not_executed`.
    NotExecuted,
    /// Drive still takes the nonce: the open record was sent again.
    Pending,
    /// The nonce is above the tip the read saw: nothing decided.
    Stale,
    /// Every round was undecidable: the open record is `couldnt_confirm`.
    CouldntConfirm(String),
    /// The group was already final.
    Final,
}

/// One round's result.
enum Round {
    Decided(Outcome),
    Undecidable(String),
}

fn undecidable(why: impl Into<String>) -> Round {
    Round::Undecidable(why.into())
}

async fn group(
    env: &impl Env,
    id: &Identifier,
    nonce: u64,
) -> Result<Vec<WithdrawalRow>, WithdrawalError> {
    let (db, shown) = (Arc::clone(env.db()), shown(id));
    Ok(tokio::task::spawn_blocking(move || db.withdrawal_group(&shown, nonce)).await??)
}

/// A read under rule 5 at or above `height`, made again while it answers
/// from below, `READ_BACK_TRIES` times at most: `Ok(None)` when none
/// reached it. A failed read is `Err`.
async fn at_least<T, F, Fut>(
    env: &impl Env,
    height: u64,
    mut read: F,
) -> Result<Option<Gated<T>>, WithdrawalError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<super::net::Proved<T>, crate::platform::PlatformError>>,
{
    for _ in 0..READ_BACK_TRIES {
        let r = gated(env.rule_5(), env.waits().probe, &mut read).await?;
        if r.read.height >= height {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// The documents of `group` at or above a height (P7).
struct Found {
    /// Every matching (record, script, document, read) whose read passed
    /// rule 5.
    matches: Vec<(WithdrawalRow, CoreScript, Document, Gated<Option<Document>>)>,
    /// The lowest height the reads answered at.
    lowest: u64,
    /// An absence or a non-match was read without passing rule 5: it
    /// decides nothing under `Enforce` (P8 (iv)).
    unverified: bool,
}

/// Reads every record's candidate documents at or above `height`, each
/// once: `Ok(Err(why))` when a read is undecidable (below `height`, failed,
/// or a positive match whose read failed rule 5).
async fn documents(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    group: &[WithdrawalRow],
    identity: Option<&Identity>,
    height: u64,
) -> Result<Result<Found, String>, WithdrawalError> {
    let mut read: HashMap<Identifier, Gated<Option<Document>>> = HashMap::new();
    let mut found = Found {
        matches: Vec::new(),
        lowest: u64::MAX,
        unverified: false,
    };
    for row in group {
        let scripts = candidates(row, identity);
        if scripts.is_empty() {
            return Ok(Err(format!(
                "no candidate script for record {}",
                row.signed.id
            )));
        }
        for script in scripts {
            let doc_id = document_of(id, row, &script);
            if let Entry::Vacant(slot) = read.entry(doc_id) {
                let r = match at_least(env, height, || net.document(doc_id)).await {
                    Ok(Some(r)) => r,
                    Ok(None) => return Ok(Err("a document read below the floor".into())),
                    Err(WithdrawalError::Platform(e)) => {
                        return Ok(Err(format!("a document read failed: {e}")));
                    }
                    Err(e) => return Err(e),
                };
                found.lowest = found.lowest.min(r.read.height);
                slot.insert(r);
            }
            let r = &read[&doc_id];
            if let Some(doc) = &r.read.value
                && matches(doc, row, id, &script)
            {
                if !r.passed {
                    return Ok(Err("a matching document's read failed rule 5".into()));
                }
                found
                    .matches
                    .push((row.clone(), script.clone(), doc.clone(), r.clone()));
            } else if !r.passed {
                found.unverified = true;
            }
        }
    }
    Ok(Ok(found))
}

/// Makes the newest matching record `done` and its siblings `sibling_paid`
/// (P7), and links the group's marked records to it (late proof, P14).
/// `false` when the compare-and-set lost (another recheck moved one).
async fn finalize(
    env: &impl Env,
    found: &Found,
    group: &[WithdrawalRow],
) -> Result<bool, WithdrawalError> {
    let Some((row, script, doc, read)) = (found.matches.iter()).max_by_key(|(r, ..)| {
        (
            r.signed.signed_at,
            group.iter().position(|g| g.signed.id == r.signed.id),
        )
    }) else {
        return Ok(false);
    };
    let proof = WithdrawalProof {
        document_id: shown(&dpp::document::DocumentV0Getters::id(doc)),
        height: read.read.height,
        block_time: read.read.time_ms,
        paid_script: script.as_bytes().to_vec(),
        document_status: status_of(doc).unwrap_or(0),
    };
    let (db, id, from) = (Arc::clone(env.db()), row.signed.id.clone(), row.state);
    let applied = tokio::task::spawn_blocking(move || {
        db.finalize_withdrawal_group_durable(&id, &[from], &proof)
    })
    .await??;
    if applied {
        let marked: Vec<String> = (group.iter())
            .filter(|r| r.mark.is_some())
            .map(|r| r.signed.id.clone())
            .collect();
        let (db, done) = (Arc::clone(env.db()), row.signed.id.clone());
        tokio::task::spawn_blocking(move || {
            for id in marked {
                db.note_withdrawal_durable(
                    &id,
                    &WithdrawalNote {
                        late_proof_link: Some(done.clone()),
                        ..WithdrawalNote::default()
                    },
                )?;
            }
            Ok::<_, dw_appdb::AppDbError>(())
        })
        .await??;
    }
    Ok(applied)
}

/// The identity read at or above `height` the owner-key candidates come
/// from, when the group has an owner-key record; `Ok(Err(why))` when it is
/// undecidable. `passed` false under `Enforce` is undecidable only where
/// the caller needs (ii).
async fn identity_for(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    group: &[WithdrawalRow],
    height: u64,
) -> Result<Result<Option<Gated<Option<Identity>>>, String>, WithdrawalError> {
    if !group.iter().any(needs_identity) {
        return Ok(Ok(None));
    }
    match at_least(env, height, || net.identity(*id)).await {
        Ok(Some(r)) => Ok(Ok(Some(r))),
        Ok(None) => Ok(Err("the identity read was below the floor".into())),
        Err(WithdrawalError::Platform(e)) => Ok(Err(format!("the identity read failed: {e}"))),
        Err(e) => Err(e),
    }
}

/// P8 step 1 alone, as `withdraw` reads back after its hand-off: a match
/// decides; anything else leaves the group as it is.
pub(crate) async fn read_back(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
) -> Result<bool, WithdrawalError> {
    let lock = identity_lock(*id);
    let _guard = lock.lock().await;
    let group = group(env, id, nonce).await?;
    if group.iter().all(|r| r.state.is_final()) {
        return Ok(false);
    }
    Ok(matches!(
        step_one(env, net, id, &group).await?,
        Some(Round::Decided(_))
    ))
}

/// Step 1: `None` when no document matches.
async fn step_one(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    group: &[WithdrawalRow],
) -> Result<Option<Round>, WithdrawalError> {
    let floor = group.iter().map(|r| r.floor_height).max().unwrap_or(0);
    fault(env.db(), points::READ_DOCUMENTS, Stage::Before)?;
    let identity = match identity_for(env, net, id, group, floor).await? {
        Ok(identity) => identity,
        Err(why) => return Ok(Some(undecidable(why))),
    };
    let identity = identity.as_ref().and_then(|g| g.read.value.as_ref());
    let found = match documents(env, net, id, group, identity, floor).await? {
        Ok(found) => found,
        Err(why) => return Ok(Some(undecidable(why))),
    };
    fault(env.db(), points::READ_DOCUMENTS, Stage::After)?;
    if found.matches.is_empty() {
        return Ok(None);
    }
    Ok(Some(match finalize(env, &found, group).await? {
        true => Round::Decided(Outcome::Done),
        false => undecidable("another recheck moved the group"),
    }))
}

/// One round of P8 over the group (module doc).
async fn round(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
) -> Result<Round, WithdrawalError> {
    let group = group(env, id, nonce).await?;
    if group.iter().all(|r| r.state.is_final()) {
        return Ok(Round::Decided(Outcome::Final));
    }
    if let Some(decided) = step_one(env, net, id, &group).await? {
        return Ok(decided);
    }
    // Step 2.
    let floor = group.iter().map(|r| r.floor_height).max().unwrap_or(0);
    fault(env.db(), points::READ_NONCE, Stage::Before)?;
    let read = match nonce_read(env, net, id, floor).await {
        Ok(Some(read)) => read,
        Ok(None) => return Ok(undecidable("the nonce read was below the floor")),
        Err(WithdrawalError::Platform(e)) => {
            return Ok(undecidable(format!("the nonce read failed: {e}")));
        }
        Err(e) => return Err(e),
    };
    fault(env.db(), points::READ_NONCE, Stage::After)?;
    let h = read.read.height;
    if read.read.verified {
        let (db, shown) = (Arc::clone(env.db()), shown(id));
        tokio::task::spawn_blocking(move || db.raise_withdrawal_floor_durable(&shown, nonce, h))
            .await??;
    }
    let state = read.read.value;
    let tip = state & IDENTITY_NONCE_VALUE_FILTER;
    if takes(state, nonce, *id) {
        if let Some(open) = group.iter().find(|r| r.state.is_open()) {
            hand_off(env, net, open, points::REBROADCAST).await?;
        }
        return Ok(Round::Decided(Outcome::Pending));
    }
    if nonce > tip {
        return Ok(Round::Decided(Outcome::Stale));
    }
    // (i) holds: n ≤ tip at H, and Drive no longer takes it.
    if !read.passed {
        return Ok(undecidable("the nonce read failed rule 5"));
    }
    fault(env.db(), points::READ_PROOF, Stage::Before)?;
    // (ii): the candidates from an identity proved at H or above.
    let identity = match identity_for(env, net, id, &group, h).await? {
        Ok(identity) => identity,
        Err(why) => return Ok(undecidable(why)),
    };
    if identity.as_ref().is_some_and(|g| !g.passed) {
        return Ok(undecidable("the identity read failed rule 5"));
    }
    let identity_height = identity.as_ref().map_or(h, |g| g.read.height);
    let identity = identity.as_ref().and_then(|g| g.read.value.as_ref());
    if group.iter().any(needs_identity) && identity.is_none() {
        return Ok(undecidable("no identity at H for the owner-key candidates"));
    }
    // (iii) and (iv).
    let found = match documents(env, net, id, &group, identity, h).await? {
        Ok(found) => found,
        Err(why) => return Ok(undecidable(why)),
    };
    if !found.matches.is_empty() {
        return Ok(match finalize(env, &found, &group).await? {
            true => Round::Decided(Outcome::Done),
            false => undecidable("another recheck moved the group"),
        });
    }
    if found.unverified {
        return Ok(undecidable("a document read failed rule 5"));
    }
    fault(env.db(), points::READ_PROOF, Stage::After)?;
    let evidence = NotExecutedEvidence {
        nonce_height: h,
        identity_height,
        document_height: found.lowest,
        reason: Some(format!(
            "nonce {nonce} used (tip {tip}) at height {h}; no matching document at or above it"
        )),
    };
    let expected: Vec<(String, WithdrawalState)> = group
        .iter()
        .map(|r| (r.signed.id.clone(), r.state))
        .collect();
    let (db, shown) = (Arc::clone(env.db()), shown(id));
    let applied = tokio::task::spawn_blocking(move || {
        let expected: Vec<(&str, WithdrawalState)> =
            expected.iter().map(|(id, s)| (id.as_str(), *s)).collect();
        db.not_executed_withdrawal_group_durable(&shown, nonce, &expected, &evidence)
    })
    .await??;
    Ok(if applied {
        tracing::info!(
            nonce,
            "a masternode withdrawal group is not executed (MG-13 P8)"
        );
        Round::Decided(Outcome::NotExecuted)
    } else {
        undecidable("another recheck moved the group")
    })
}

/// Rechecks the group (identity `id`, nonce `nonce`) under its lock (module
/// doc), up to `READ_BACK_TRIES` rounds.
pub(crate) async fn recheck_group(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
) -> Result<Outcome, WithdrawalError> {
    let lock = identity_lock(*id);
    let _guard = lock.lock().await;
    let mut why = String::new();
    for _ in 0..READ_BACK_TRIES {
        match round(env, net, id, nonce).await? {
            Round::Decided(outcome) => return Ok(outcome),
            Round::Undecidable(w) => why = w,
        }
    }
    // Step 3: the open record, if one is left, couldn't be confirmed.
    let group = group(env, id, nonce).await?;
    let db = Arc::clone(env.db());
    let reason = why.clone();
    tokio::task::spawn_blocking(move || {
        use WithdrawalState::*;
        for r in group.iter().filter(|r| r.state.is_open()) {
            if r.state == CouldntConfirm {
                db.note_withdrawal_durable(
                    &r.signed.id,
                    &WithdrawalNote {
                        state_reason: Some(reason.clone()),
                        ..WithdrawalNote::default()
                    },
                )?;
            } else {
                db.set_withdrawal_state_durable(
                    &r.signed.id,
                    &[r.state],
                    CouldntConfirm,
                    Some(&reason),
                )?;
            }
        }
        Ok::<_, dw_appdb::AppDbError>(())
    })
    .await??;
    Ok(Outcome::CouldntConfirm(why))
}

/// Every undecided group, oldest first (P8: on session start, on demand,
/// periodically while anything is undecided). A group's Platform failure
/// is logged and the next group runs; a storage failure stops the pass.
pub(crate) async fn recheck_all(
    env: &impl Env,
    net: &impl MnWithdrawNet,
) -> Result<Vec<Outcome>, WithdrawalError> {
    let db = Arc::clone(env.db());
    let rows = tokio::task::spawn_blocking(move || db.undecided_withdrawals()).await??;
    let mut groups: Vec<(String, u64)> = Vec::new();
    for r in &rows {
        let key = (r.signed.identity_id.clone(), r.signed.nonce);
        if !groups.contains(&key) {
            groups.push(key);
        }
    }
    let mut outcomes = Vec::new();
    for (identity, nonce) in groups {
        let Ok(id) = Identifier::from_string(
            &identity,
            dpp::platform_value::string_encoding::Encoding::Base58,
        ) else {
            tracing::warn!(%identity, "a withdrawal record's identity does not parse");
            continue;
        };
        match recheck_group(env, net, &id, nonce).await {
            Ok(outcome) => outcomes.push(outcome),
            Err(e @ WithdrawalError::Engine(_)) => return Err(e),
            Err(e) => tracing::info!(error = %e, nonce, "rechecking a masternode withdrawal"),
        }
    }
    Ok(outcomes)
}

/// `recheck_withdrawal(id)` (P13): an undecided record's group as
/// [`recheck_group`]; a `done` one's document status refreshed; a
/// `not_executed` one's documents read once more, where a match is an
/// invariant violation: logged, alarmed, the state kept (P6).
pub(crate) async fn recheck_record(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    record_id: &str,
) -> Result<WithdrawalRow, WithdrawalError> {
    let row = load(env, record_id).await?;
    let id = Identifier::from_string(
        &row.signed.identity_id,
        dpp::platform_value::string_encoding::Encoding::Base58,
    )
    .map_err(|_| WithdrawalError::NotFound)?;
    match row.state {
        s if !s.is_final() => {
            recheck_group(env, net, &id, row.signed.nonce).await?;
        }
        WithdrawalState::Done => {
            if let Some(proof) = &row.proof
                && let Ok(doc_id) = Identifier::from_string(
                    &proof.document_id,
                    dpp::platform_value::string_encoding::Encoding::Base58,
                )
                && let Ok(read) = net.document(doc_id).await
                && let Some(status) = read.value.as_ref().and_then(status_of)
            {
                let (db, rid) = (Arc::clone(env.db()), row.signed.id.clone());
                tokio::task::spawn_blocking(move || {
                    db.note_withdrawal_durable(
                        &rid,
                        &WithdrawalNote {
                            document_status: Some(status),
                            ..WithdrawalNote::default()
                        },
                    )
                })
                .await??;
            }
        }
        WithdrawalState::NotExecuted => {
            let group = group(env, &id, row.signed.nonce).await?;
            let identity = match identity_for(env, net, &id, &group, row.floor_height).await? {
                Ok(identity) => identity,
                Err(why) => {
                    tracing::info!(%why, "rechecking a not-executed masternode withdrawal");
                    None
                }
            };
            let identity = identity.as_ref().and_then(|g| g.read.value.as_ref());
            if let Ok(found) = documents(env, net, &id, &group, identity, row.floor_height).await?
                && !found.matches.is_empty()
            {
                env.alarm(Alarm {
                    identity: row.signed.identity_id.clone(),
                    nonce: row.signed.nonce,
                    what: "a document matches a not-executed group (late proof)".into(),
                });
            }
        }
        _ => {}
    }
    load(env, record_id).await
}

async fn load(env: &impl Env, id: &str) -> Result<WithdrawalRow, WithdrawalError> {
    let (db, id) = (Arc::clone(env.db()), id.to_owned());
    (tokio::task::spawn_blocking(move || db.withdrawal(&id)).await??)
        .ok_or(WithdrawalError::NotFound)
}

/// `mark_withdrawal(id, mark)` (P9, D47): only from `couldnt_confirm`, the
/// mark stored apart from the proof; it closes the record for the open set
/// and leaves the group undecided.
pub(crate) async fn mark(
    env: &impl Env,
    record_id: &str,
    mark: WithdrawalMark,
) -> Result<WithdrawalRow, WithdrawalError> {
    let row = load(env, record_id).await?;
    let id = Identifier::from_string(
        &row.signed.identity_id,
        dpp::platform_value::string_encoding::Encoding::Base58,
    )
    .map_err(|_| WithdrawalError::NotFound)?;
    let lock = identity_lock(id);
    let _guard = lock.lock().await;
    let (db, rid, now) = (Arc::clone(env.db()), record_id.to_owned(), env.now());
    let applied =
        tokio::task::spawn_blocking(move || db.mark_withdrawal_durable(&rid, mark, now)).await??;
    if !applied {
        return Err(WithdrawalError::MarkState);
    }
    load(env, record_id).await
}
