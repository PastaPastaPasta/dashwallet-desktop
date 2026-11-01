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
//!    rule 5 does not pass under `Enforce`): the round again. A round
//!    counts toward `READ_BACK_TRIES` only when Platform answered with
//!    reads that passed rule 5 and still decided nothing; after that many
//!    counted rounds the open record is `couldnt_confirm`. A round lost to
//!    transport or to rule 5 does not count, and after that many lost ones
//!    the record stays where it is ("sent; checking its result", MG-00
//!    D44): a DAPI outage, or SPV without the quorum keys yet, makes no
//!    `couldnt_confirm`.
//!
//! Under `Enforce`, step 1's positive match, (i), (ii) and (iv) must pass
//! rule 5 (P8 step 4, P12). No path signs again.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use dpp::document::Document;
use dpp::identity::Identity;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_nonce::IDENTITY_NONCE_VALUE_FILTER;
use dpp::prelude::Identifier;
use dw_appdb::{
    NotExecutedEvidence, WithdrawalMark, WithdrawalNote, WithdrawalProof, WithdrawalRow,
    WithdrawalState,
};

use super::check::{Gated, at_least};
use super::drive::{candidates, document_status, matches, needs_identity};
use super::net::{MnWithdrawNet, Proved};
use super::sign::{floor_of, hand_off, shown};
use super::{
    Alarm, Env, WithdrawalError, fault_after, fault_before, identity_lock, load, on_db, parse_id,
    points,
};
use crate::platform::PlatformError;
use crate::platform::withdrawal::{READ_BACK_TRIES, document_id, takes};

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
    /// `READ_BACK_TRIES` counted rounds decided nothing: the open record is
    /// `couldnt_confirm`.
    CouldntConfirm(String),
    /// `READ_BACK_TRIES` rounds were lost to transport or rule 5: nothing
    /// decided, the records where they were.
    Unanswered(String),
    /// The group was already final.
    Final,
}

/// Why a round decided nothing.
struct Undecided {
    why: String,
    /// Platform answered with reads that passed rule 5: the round counts
    /// toward `READ_BACK_TRIES` (module doc, step 3).
    counts: bool,
}

/// One round's result.
enum Round {
    Decided(Outcome),
    Undecidable(Undecided),
}

/// Platform answered, rule 5 passed, and nothing is decided: a counted
/// round.
fn answered(why: impl Into<String>) -> Undecided {
    Undecided {
        why: why.into(),
        counts: true,
    }
}

/// A read failed rule 5: the round does not count.
fn unverified(what: &str) -> Undecided {
    Undecided {
        why: format!("{what} read failed rule 5"),
        counts: false,
    }
}

/// `what`'s read under rule 5 at or above `height` ([`at_least`]), or why
/// the round is undecidable: a read below `height` (counted when it passed
/// rule 5) or a failed one (not counted).
async fn reached<T, F, Fut>(
    env: &impl Env,
    height: u64,
    what: &str,
    read: F,
) -> Result<Gated<T>, Undecided>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Proved<T>, PlatformError>>,
{
    match at_least(env, height, read).await {
        Ok(r) if r.read.height >= height => Ok(r),
        Ok(r) => Err(Undecided {
            why: format!("{what} read was below the floor"),
            counts: r.passed,
        }),
        Err(e) => Err(Undecided {
            why: format!("{what} read failed: {e}"),
            counts: false,
        }),
    }
}

async fn group(
    env: &impl Env,
    id: &Identifier,
    nonce: u64,
) -> Result<Vec<WithdrawalRow>, WithdrawalError> {
    let shown = shown(id);
    on_db(env, move |db| db.withdrawal_group(&shown, nonce)).await
}

fn all_final(group: &[WithdrawalRow]) -> bool {
    group.iter().all(|r| r.state.is_final())
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
/// once; undecidable when a read is (below `height`, failed, or a positive
/// match whose read failed rule 5).
async fn documents(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    group: &[WithdrawalRow],
    identity: Option<&Identity>,
    height: u64,
) -> Result<Found, Undecided> {
    let mut read: HashMap<Identifier, Gated<Option<Document>>> = HashMap::new();
    let mut found = Found {
        matches: Vec::new(),
        lowest: u64::MAX,
        unverified: false,
    };
    for row in group {
        let scripts = candidates(row, identity);
        if scripts.is_empty() {
            return Err(answered(format!(
                "no candidate script for record {}",
                row.signed.id
            )));
        }
        for script in scripts {
            let doc_id = document_id(id, row.signed.nonce, &script);
            if let Entry::Vacant(slot) = read.entry(doc_id) {
                let r = reached(env, height, "a document", || net.document(doc_id)).await?;
                found.lowest = found.lowest.min(r.read.height);
                slot.insert(r);
            }
            let r = &read[&doc_id];
            if let Some(doc) = &r.read.value
                && matches(doc, row, id, &script)
            {
                if !r.passed {
                    return Err(unverified("a matching document's"));
                }
                found
                    .matches
                    .push((row.clone(), script.clone(), doc.clone(), r.clone()));
            } else if !r.passed {
                found.unverified = true;
            }
        }
    }
    Ok(found)
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
        document_status: document_status(doc).unwrap_or(0),
    };
    let (done, from) = (row.signed.id.clone(), row.state);
    let marked: Vec<String> = (group.iter())
        .filter(|r| r.mark.is_some())
        .map(|r| r.signed.id.clone())
        .collect();
    on_db(env, move |db| {
        if !db.finalize_withdrawal_group_durable(&done, &[from], &proof)? {
            return Ok(false);
        }
        for id in marked {
            db.note_withdrawal_durable(
                &id,
                &WithdrawalNote {
                    late_proof_link: Some(done.clone()),
                    ..WithdrawalNote::default()
                },
            )?;
        }
        Ok(true)
    })
    .await
}

/// [`finalize`] as a round's end: `done`, or a counted round when another
/// recheck moved the group first.
async fn finalized(
    env: &impl Env,
    found: &Found,
    group: &[WithdrawalRow],
) -> Result<Round, WithdrawalError> {
    Ok(match finalize(env, found, group).await? {
        true => Round::Decided(Outcome::Done),
        false => Round::Undecidable(answered("another recheck moved the group")),
    })
}

/// The identity read at or above `height` the owner-key candidates come
/// from, when the group has an owner-key record. `passed` false under
/// `Enforce` is undecidable only where the caller needs (ii).
async fn identity_for(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    group: &[WithdrawalRow],
    height: u64,
) -> Result<Option<Gated<Option<Identity>>>, Undecided> {
    if !group.iter().any(needs_identity) {
        return Ok(None);
    }
    reached(env, height, "the identity", || net.identity(*id))
        .await
        .map(Some)
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
    if all_final(&group) {
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
    let floor = floor_of(group);
    fault_before(env.db(), points::READ_DOCUMENTS)?;
    let found = async {
        let identity = identity_for(env, net, id, group, floor).await?;
        let identity = identity.as_ref().and_then(|g| g.read.value.as_ref());
        documents(env, net, id, group, identity, floor).await
    };
    let found = match found.await {
        Ok(found) => found,
        Err(why) => return Ok(Some(Round::Undecidable(why))),
    };
    fault_after(env.db(), points::READ_DOCUMENTS)?;
    if found.matches.is_empty() {
        return Ok(None);
    }
    Ok(Some(finalized(env, &found, group).await?))
}

/// One round of P8 over the group (module doc).
async fn round(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
) -> Result<Round, WithdrawalError> {
    let group = group(env, id, nonce).await?;
    if all_final(&group) {
        return Ok(Round::Decided(Outcome::Final));
    }
    if let Some(decided) = step_one(env, net, id, &group).await? {
        return Ok(decided);
    }
    let (h, tip) = match step_nonce(env, net, id, nonce, &group).await? {
        Ok(Some(at)) => at,
        Ok(None) => return Ok(Round::Decided(Outcome::Pending)),
        Err(round) => return Ok(round),
    };
    match not_executed(env, net, id, nonce, &group, h, tip).await? {
        Ok(round) => Ok(round),
        Err(why) => Ok(Round::Undecidable(why)),
    }
}

/// Step 2's nonce read: the floor raised to an SPV-verified read's H; the
/// open record handed off again while Drive takes the nonce (`Ok(None)`);
/// else `Ok(Some((H, tip)))` once (i) holds, n at or below the tip and not
/// taken, or the round's end.
async fn step_nonce(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
    group: &[WithdrawalRow],
) -> Result<Result<Option<(u64, u64)>, Round>, WithdrawalError> {
    fault_before(env.db(), points::READ_NONCE)?;
    let read = match reached(env, floor_of(group), "the nonce", || net.nonce_state(*id)).await {
        Ok(read) => read,
        Err(why) => return Ok(Err(Round::Undecidable(why))),
    };
    fault_after(env.db(), points::READ_NONCE)?;
    let h = read.read.height;
    if read.read.verified {
        let shown = shown(id);
        on_db(env, move |db| {
            db.raise_withdrawal_floor_durable(&shown, nonce, h)
        })
        .await?;
    }
    let state = read.read.value;
    let tip = state & IDENTITY_NONCE_VALUE_FILTER;
    if takes(state, nonce, *id) {
        if let Some(open) = group.iter().find(|r| r.state.is_open()) {
            hand_off(env, net, open, points::REBROADCAST).await?;
        }
        return Ok(Ok(None));
    }
    if nonce > tip {
        return Ok(Err(Round::Decided(Outcome::Stale)));
    }
    // (i) holds: n ≤ tip at H, and Drive no longer takes it.
    if !read.passed {
        return Ok(Err(Round::Undecidable(unverified("the nonce"))));
    }
    Ok(Ok(Some((h, tip))))
}

/// Step 2's proof that nothing at `nonce` paid, (ii)–(iv) at or above H
/// once (i) holds: `not_executed` for the whole group, or `done` on a match
/// read at H or above.
async fn not_executed(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
    group: &[WithdrawalRow],
    h: u64,
    tip: u64,
) -> Result<Result<Round, Undecided>, WithdrawalError> {
    fault_before(env.db(), points::READ_PROOF)?;
    let read = async {
        // (ii): the candidates from an identity proved at H or above.
        let identity = identity_for(env, net, id, group, h).await?;
        if identity.as_ref().is_some_and(|g| !g.passed) {
            return Err(unverified("the identity"));
        }
        let identity_height = identity.as_ref().map_or(h, |g| g.read.height);
        let identity = identity.as_ref().and_then(|g| g.read.value.as_ref());
        if group.iter().any(needs_identity) && identity.is_none() {
            return Err(answered("no identity at H for the owner-key candidates"));
        }
        // (iii) and (iv).
        let found = documents(env, net, id, group, identity, h).await?;
        Ok((identity_height, found))
    };
    let (identity_height, found) = match read.await {
        Ok(read) => read,
        Err(why) => return Ok(Err(why)),
    };
    if !found.matches.is_empty() {
        return Ok(Ok(finalized(env, &found, group).await?));
    }
    if found.unverified {
        return Ok(Err(unverified("a document")));
    }
    fault_after(env.db(), points::READ_PROOF)?;
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
    let shown = shown(id);
    let applied = on_db(env, move |db| {
        let expected: Vec<(&str, WithdrawalState)> =
            expected.iter().map(|(id, s)| (id.as_str(), *s)).collect();
        db.not_executed_withdrawal_group_durable(&shown, nonce, &expected, &evidence)
    })
    .await?;
    Ok(if applied {
        tracing::info!(
            nonce,
            "a masternode withdrawal group is not executed (MG-13 P8)"
        );
        Ok(Round::Decided(Outcome::NotExecuted))
    } else {
        Err(answered("another recheck moved the group"))
    })
}

/// Rechecks the group (identity `id`, nonce `nonce`) under its lock (module
/// doc): rounds until one decides, `READ_BACK_TRIES` rounds count, or as
/// many are lost.
pub(crate) async fn recheck_group(
    env: &impl Env,
    net: &impl MnWithdrawNet,
    id: &Identifier,
    nonce: u64,
) -> Result<Outcome, WithdrawalError> {
    let lock = identity_lock(*id);
    let _guard = lock.lock().await;
    let (mut counted, mut lost) = (0, 0);
    let why = loop {
        match round(env, net, id, nonce).await? {
            Round::Decided(outcome) => return Ok(outcome),
            Round::Undecidable(u) if u.counts => {
                counted += 1;
                if counted == READ_BACK_TRIES {
                    break u.why;
                }
            }
            Round::Undecidable(u) => {
                lost += 1;
                if lost == READ_BACK_TRIES {
                    tracing::info!(why = %u.why, nonce,
                        "a masternode withdrawal's reads were lost; still checking (MG-13 P8)");
                    return Ok(Outcome::Unanswered(u.why));
                }
            }
        }
    };
    // Step 3: the open record, if one is left, couldn't be confirmed.
    let group = group(env, id, nonce).await?;
    let reason = why.clone();
    on_db(env, move |db| {
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
        Ok(())
    })
    .await?;
    Ok(Outcome::CouldntConfirm(why))
}

/// Every undecided group, oldest first (P8: on session start, on demand,
/// periodically while anything is undecided). A group's Platform failure
/// is logged and the next group runs; a storage failure stops the pass.
pub(crate) async fn recheck_all(
    env: &impl Env,
    net: &impl MnWithdrawNet,
) -> Result<Vec<Outcome>, WithdrawalError> {
    let rows = on_db(env, |db| db.undecided_withdrawals()).await?;
    let mut groups: Vec<(String, u64)> = Vec::new();
    for r in &rows {
        let key = (r.signed.identity_id.clone(), r.signed.nonce);
        if !groups.contains(&key) {
            groups.push(key);
        }
    }
    let mut outcomes = Vec::new();
    for (identity, nonce) in groups {
        let Ok(id) = parse_id(&identity) else {
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
    let id = parse_id(&row.signed.identity_id)?;
    match row.state {
        s if !s.is_final() => {
            recheck_group(env, net, &id, row.signed.nonce).await?;
        }
        WithdrawalState::Done => {
            if let Some(proof) = &row.proof
                && let Ok(doc_id) = parse_id(&proof.document_id)
                && let Ok(read) = net.document(doc_id).await
                && let Some(status) = read.value.as_ref().and_then(document_status)
            {
                let rid = row.signed.id.clone();
                on_db(env, move |db| {
                    db.note_withdrawal_durable(
                        &rid,
                        &WithdrawalNote {
                            document_status: Some(status),
                            ..WithdrawalNote::default()
                        },
                    )
                })
                .await?;
            }
        }
        WithdrawalState::NotExecuted => {
            let group = group(env, &id, row.signed.nonce).await?;
            let identity = match identity_for(env, net, &id, &group, row.floor_height).await {
                Ok(identity) => identity,
                Err(why) => {
                    tracing::info!(why = %why.why, "rechecking a not-executed masternode withdrawal");
                    None
                }
            };
            let identity = identity.as_ref().and_then(|g| g.read.value.as_ref());
            if let Ok(found) = documents(env, net, &id, &group, identity, row.floor_height).await
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

/// `mark_withdrawal(id, mark)` (P9, D47): only from `couldnt_confirm`, the
/// mark stored apart from the proof; it closes the record for the open set
/// and leaves the group undecided.
pub(crate) async fn mark(
    env: &impl Env,
    record_id: &str,
    mark: WithdrawalMark,
) -> Result<WithdrawalRow, WithdrawalError> {
    let row = load(env, record_id).await?;
    let lock = identity_lock(parse_id(&row.signed.identity_id)?);
    let _guard = lock.lock().await;
    let (rid, now) = (record_id.to_owned(), env.now());
    if !on_db(env, move |db| db.mark_withdrawal_durable(&rid, mark, now)).await? {
        return Err(WithdrawalError::MarkState);
    }
    load(env, record_id).await
}
