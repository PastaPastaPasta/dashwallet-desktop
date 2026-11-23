//! The steps of an import into a held session (MG-04b §3.0), each the
//! counterpart of one of dash-qt's (rc.3 `mnsharesession.cpp:1204-1551`)
//! with the wallet's stricter rules (D15, D33, m-7). Which step applies is
//! the engine's decision, by role, state and the copies' stages and
//! revisions (§3.0's table); every step here is pure and leaves the held
//! session untouched on refusal.

use dashcore::OutPoint;

use super::super::envelope::decode_base64;
use super::funding::{Frozen, decode_frozen, rebuild_funding_tx, verify_consent_signature};
use super::{Contribution, Session, Signature, Stage};

/// Why an import is refused; the engine's `shared_message{reason}` and
/// `shared_funding_mismatch{reason}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// Another session id.
    #[error("the message is for another session")]
    OtherSession,
    /// The masternode-wide terms, the share count, or a row's label or
    /// amount differ ("The reply does not match your current draft").
    #[error("the terms changed")]
    TermsChanged,
    /// A copy this session has moved past (D33, §3.0).
    #[error("the message is older than this session")]
    Older,
    #[error("the message conflicts with this session")]
    Conflict,
    /// A filled row of ours differs in theirs (`SameShare`).
    #[error("a share's details changed")]
    RowChanged,
    /// A contribution of ours differs in or is missing from theirs
    /// (`SameContribution`).
    #[error("a contribution changed")]
    ContributionChanged,
    #[error("the approval for share {share} doesn't verify")]
    BadSignature { share: u16 },
    /// A scriptSig for one of this wallet's inputs that this wallet didn't
    /// make (m-7).
    #[error("the message signs one of this wallet's inputs")]
    ForeignInput,
}

fn same_id(ours: &Session, theirs: &Session) -> Result<(), Refusal> {
    if ours.session_id == theirs.session_id {
        Ok(())
    } else {
        Err(Refusal::OtherSession)
    }
}

/// The share count, the terms, and every row's label and amount
/// (`absorbDraftReply`'s and `adoptLockedTerms`' first test).
fn same_table(ours: &Session, theirs: &Session) -> Result<(), Refusal> {
    let rows = ours.shares.len() == theirs.shares.len()
        && ours
            .shares
            .iter()
            .zip(&theirs.shares)
            .all(|(a, b)| a.label == b.label && a.amount == b.amount);
    if rows && ours.same_terms(theirs) {
        Ok(())
    } else {
        Err(Refusal::TermsChanged)
    }
}

fn contribution<'a>(session: &'a Session, label: &str) -> Option<&'a Contribution> {
    session.contributions.iter().find(|c| c.label == label)
}

/// `session` holds `held`'s contribution (by label), unchanged.
fn has_same(session: &Session, held: &Contribution) -> bool {
    contribution(session, &held.label).is_some_and(|c| c.same(held))
}

/// `absorbDraftReply`, by the coordinator whose own contribution is
/// labelled `own_label`. `Ok(None)` is a reply that adds nothing (no bump,
/// `:1265-1270`); otherwise the merged session, `revision` = ours + 1.
///
/// Stricter than dash-qt: the reply's copy of our contribution must be the
/// current one (D33, else [`Refusal::Older`]), and it adds at most one
/// contribution and fills only the row that contribution names (D15).
pub fn absorb_draft_reply(
    ours: &Session,
    theirs: &Session,
    own_label: &str,
) -> Result<Option<Session>, Refusal> {
    same_id(ours, theirs)?;
    if ours.stage != Stage::Draft || theirs.stage != Stage::Draft {
        return Err(Refusal::Conflict);
    }
    same_table(ours, theirs)?;
    let current = contribution(ours, own_label);
    if current.is_some_and(|c| !has_same(theirs, c)) {
        return Err(Refusal::Older);
    }

    let mut new = theirs
        .contributions
        .iter()
        .filter(|c| contribution(ours, &c.label).is_none());
    let added = new.next();
    if new.next().is_some() {
        return Err(Refusal::Conflict);
    }
    let named_row = added.and_then(|c| ours.share_index_of(&c.label));

    let mut merged = ours.clone();
    let mut absorbed = false;
    for (i, (mine, sent)) in merged.shares.iter_mut().zip(&theirs.shares).enumerate() {
        for (mine, sent) in [
            (&mut mine.owner_address, &sent.owner_address),
            (&mut mine.refund_address, &sent.refund_address),
            (&mut mine.reward_address, &sent.reward_address),
        ] {
            if sent.is_empty() || mine == sent {
                continue;
            }
            // "%1's share already has details from another reply.", or a
            // row this reply's contribution doesn't name (D15).
            if !mine.is_empty() || named_row != Some(i) {
                return Err(Refusal::Conflict);
            }
            mine.clone_from(sent);
            absorbed = true;
        }
    }
    if let Some(c) = added {
        add_contribution(&mut merged, c)?;
        absorbed = true;
    }
    if !absorbed {
        return Ok(None);
    }
    // By share, unknown labels last (`:1274-1278`); stable.
    let order = |c: &Contribution| {
        merged
            .share_index_of(&c.label)
            .unwrap_or(merged.shares.len())
    };
    let mut sorted = merged.contributions.clone();
    sorted.sort_by_key(order);
    merged.contributions = sorted;
    rebuild_funding_tx(&mut merged).map_err(|_| Refusal::Conflict)?;
    merged.revision = ours.revision + 1;
    Ok(Some(merged))
}

/// `addContribution`'s checks (`mnsharesession.cpp:815-858`): a label, at
/// least one input, no input already contributed, positive change.
fn add_contribution(session: &mut Session, c: &Contribution) -> Result<(), Refusal> {
    let reused = session
        .contributions
        .iter()
        .flat_map(|e| &e.inputs)
        .any(|known| {
            c.inputs
                .iter()
                .any(|new| known.txid.eq_ignore_ascii_case(&new.txid) && known.vout == new.vout)
        });
    if c.label.is_empty()
        || c.inputs.is_empty()
        || reused
        || c.change.as_ref().is_some_and(|ch| ch.amount <= 0)
    {
        return Err(Refusal::Conflict);
    }
    session.contributions.push(c.clone());
    Ok(())
}

/// `adoptLockedTerms` (`mnsharesession.cpp:1286-1351`): their locked copy,
/// if it is the agreement ours answered: the same table and terms, every
/// row we hold filled unchanged, every contribution we hold unchanged.
pub fn adopt_locked_terms(ours: &Session, theirs: &Session) -> Result<Session, Refusal> {
    same_id(ours, theirs)?;
    if ours.stage != Stage::Draft || !matches!(theirs.stage, Stage::Frozen | Stage::Signing) {
        return Err(Refusal::Conflict);
    }
    same_table(ours, theirs)?;
    for (mine, locked) in ours.shares.iter().zip(&theirs.shares) {
        if !mine.owner_address.is_empty() && !mine.same(locked) {
            return Err(Refusal::RowChanged);
        }
    }
    for mine in &ours.contributions {
        if !has_same(theirs, mine) {
            return Err(Refusal::ContributionChanged);
        }
    }
    Ok(theirs.clone())
}

/// M-1's test for a participant taking a higher revision (§3.0): its row
/// (`share`) and its contribution exactly as in `ours`, the copy it
/// recorded.
pub fn own_unchanged(ours: &Session, theirs: &Session, share: usize) -> Result<(), Refusal> {
    same_id(ours, theirs)?;
    let (Some(mine), Some(sent)) = (ours.shares.get(share), theirs.shares.get(share)) else {
        return Err(Refusal::RowChanged);
    };
    if mine.label != sent.label || !mine.same(sent) {
        return Err(Refusal::RowChanged);
    }
    let own = contribution(ours, &mine.label);
    if own.is_some_and(|c| !has_same(theirs, c)) {
        return Err(Refusal::ContributionChanged);
    }
    Ok(())
}

/// Why one received signature wasn't added (dash-qt warns and goes on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedSignature {
    /// Out of range, not base64, or not verifying.
    Invalid { share: i32 },
    /// Another signature for this share is already recorded.
    Different { share: i32 },
}

/// `addSignature` (`mnsharesession.cpp:1108-1140`) on a frozen session:
/// verified, a byte-identical duplicate a no-op, a different one refused;
/// inserted in ascending order, `frozen` → `signing`. `Ok(true)` when added.
pub fn add_signature(
    session: &mut Session,
    frozen: &Frozen,
    share: i32,
    signature: &str,
) -> Result<bool, SkippedSignature> {
    if !verify_consent_signature(frozen, share, signature) {
        return Err(SkippedSignature::Invalid { share });
    }
    if let Some(existing) = session.signature_for(share) {
        return if decode_base64(&existing.signature) == decode_base64(signature) {
            Ok(false)
        } else {
            Err(SkippedSignature::Different { share })
        };
    }
    let at = session
        .sigs
        .iter()
        .position(|s| s.share_index > share)
        .unwrap_or(session.sigs.len());
    session.sigs.insert(
        at,
        Signature {
            share_index: share,
            signature: signature.to_owned(),
        },
    );
    if session.stage == Stage::Frozen {
        session.stage = Stage::Signing;
    }
    Ok(true)
}

/// `mergeEnvelope`'s same-revision union of approvals (`:1391-1424`): both
/// copies frozen on one consent hash ([`Refusal::Conflict`] otherwise); each
/// of theirs added as [`add_signature`] adds it.
pub fn merge_approvals(
    ours: &Session,
    theirs: &Session,
) -> Result<(Session, Vec<SkippedSignature>), Refusal> {
    same_id(ours, theirs)?;
    if ours.revision != theirs.revision
        || ours.consent_hash.is_empty()
        || !ours.consent_hash.eq_ignore_ascii_case(&theirs.consent_hash)
    {
        return Err(Refusal::Conflict);
    }
    let frozen = decode_frozen(ours).map_err(|_| Refusal::Conflict)?;
    let mut merged = ours.clone();
    let mut skipped = Vec::new();
    for sig in &theirs.sigs {
        if let Err(s) = add_signature(&mut merged, &frozen, sig.share_index, &sig.signature) {
            skipped.push(s);
        }
    }
    Ok((merged, skipped))
}

/// MG-04b D12 at `combined`: every share's join signature in the payload is
/// the session's verified approval for it.
pub fn check_join_signatures(session: &Session, frozen: &Frozen) -> Result<(), Refusal> {
    if frozen.payload.join_sigs.len() != session.shares.len() {
        return Err(Refusal::Conflict);
    }
    for (i, join) in frozen.payload.join_sigs.iter().enumerate() {
        let share = i as i32;
        let approved = session.signature_for(share).is_some_and(|s| {
            decode_base64(&s.signature).as_deref() == Some(join.as_slice())
                && verify_consent_signature(frozen, share, &s.signature)
        });
        if !approved {
            return Err(Refusal::BadSignature { share: i as u16 });
        }
    }
    Ok(())
}

/// `setCombinedTx` (`:1496-1516`) plus D12: our approving copy, every share
/// signed, takes their combined transaction if its consent digest is ours
/// and its join signatures are our approvals. A copy carrying a scriptSig
/// on one of `own_inputs` is refused (m-7): only this wallet signs those,
/// and a later merge never replaces a scriptSig it holds.
pub fn adopt_combined(
    ours: &Session,
    theirs: &Session,
    own_inputs: &[OutPoint],
) -> Result<Session, Refusal> {
    same_id(ours, theirs)?;
    if !matches!(ours.stage, Stage::Frozen | Stage::Signing)
        || theirs.stage < Stage::Combined
        || ours.signed_count() < ours.shares.len()
    {
        return Err(Refusal::Conflict);
    }
    let mut merged = ours.clone();
    merged.protx.clone_from(&theirs.protx);
    let frozen = decode_frozen(&merged).map_err(|_| Refusal::Conflict)?;
    check_join_signatures(&merged, &frozen)?;
    if frozen
        .tx
        .input
        .iter()
        .any(|i| own_inputs.contains(&i.previous_output) && !i.script_sig.is_empty())
    {
        return Err(Refusal::ForeignInput);
    }
    merged.stage = Stage::Combined;
    Ok(merged)
}

/// §3.0's per-input merge of two copies of the combined transaction
/// (`mergeEnvelope`, `:1426-1463`, stricter, m-7): the transactions without
/// scriptSigs identical; a scriptSig on one of `own_inputs` never taken
/// unless it is the one we hold; a foreign input's later scriptSig replaces
/// the earlier unless `fixed` (the session's `out_at` is set). Every input
/// signed moves the copy to `fundingSigned`.
pub fn merge_funding_inputs(
    ours: &Session,
    theirs: &Session,
    own_inputs: &[OutPoint],
    fixed: bool,
) -> Result<Session, Refusal> {
    same_id(ours, theirs)?;
    let stages = |s: &Session| matches!(s.stage, Stage::Combined | Stage::FundingSigned);
    if ours.revision != theirs.revision || !stages(ours) || !stages(theirs) {
        return Err(Refusal::Conflict);
    }
    let mut our_tx = decode_frozen(ours).map_err(|_| Refusal::Conflict)?.tx;
    let mut their_copy = ours.clone();
    their_copy.protx.clone_from(&theirs.protx);
    let their_tx = decode_frozen(&their_copy)
        .map_err(|_| Refusal::Conflict)?
        .tx;
    let unsigned = |tx: &crate::shared::SpecialTx| {
        let mut tx = tx.clone();
        tx.input
            .iter_mut()
            .for_each(|i| i.script_sig = Default::default());
        tx
    };
    if unsigned(&our_tx) != unsigned(&their_tx) {
        return Err(Refusal::Conflict);
    }
    for (mine, sent) in our_tx.input.iter_mut().zip(&their_tx.input) {
        if sent.script_sig.is_empty() || mine.script_sig == sent.script_sig {
            continue;
        }
        if own_inputs.contains(&mine.previous_output) {
            return Err(Refusal::ForeignInput);
        }
        if fixed && !mine.script_sig.is_empty() {
            return Err(Refusal::Conflict);
        }
        mine.script_sig.clone_from(&sent.script_sig);
    }
    let mut merged = ours.clone();
    merged.protx = hex::encode(our_tx.to_bytes());
    if our_tx.input.iter().all(|i| !i.script_sig.is_empty()) {
        merged.stage = merged.stage.max(Stage::FundingSigned);
    }
    Ok(merged)
}
