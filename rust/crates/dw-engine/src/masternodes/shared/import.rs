//! §3.0's import table (MG-04b), read top-down, first match: by role, the
//! session's state and the copies' stages and revisions, which of
//! dw-protx's merge steps applies, or the refusal. Pure: the held session
//! is untouched on refusal, and nothing here signs.

use std::cmp::Ordering;

use dw_protx::shared::session::funding::{decode_frozen, verify_consent_signature};
use dw_protx::shared::session::merge::{
    SkippedSignature, absorb_draft_reply, adopt_combined, adopt_locked_terms, merge_approvals,
    merge_funding_inputs, own_unchanged,
};
use dw_protx::shared::session::{Session, Stage};

use super::super::MasternodesError;
use super::super::shared_records::SharedRole;
use super::build::combine;
use super::store::{Loaded, envelope_refusal, merge_refusal, message_refusal};

/// What an import does to the wallet's sessions.
#[derive(Debug)]
pub(crate) enum Applied {
    /// A new participant session, `invited`: no coin touched.
    Insert(Session),
    /// The held session becomes this copy.
    Update(Session),
    /// Nothing changes (a re-pasted message, a reply that adds nothing).
    Same,
}

fn frozen_or_later(stage: Stage) -> bool {
    stage >= Stage::Frozen
}

fn locked(stage: Stage) -> bool {
    matches!(stage, Stage::Frozen | Stage::Signing)
}

fn combined_or_later(stage: Stage) -> bool {
    stage >= Stage::Combined
}

/// `theirs` against no session of the wallet: an Invitation makes a new
/// participant session; anything frozen has no recorded side to check
/// against (§4).
fn first(theirs: &Session) -> Result<Applied, MasternodesError> {
    if frozen_or_later(theirs.stage) {
        return Err(message_refusal("not_participant"));
    }
    let mut invited = theirs.clone();
    invited.sigs.clear();
    Ok(Applied::Insert(invited))
}

/// Locked terms taken by a participant (`joined`, or M-1 case 2): our own
/// approval is never carried over, only a fresh `approve_shared` signs,
/// and only the others' approvals that verify are kept.
fn relocked(theirs: &Session, own_share: usize) -> Result<Session, MasternodesError> {
    let frozen = decode_frozen(theirs).map_err(envelope_refusal)?;
    let mut s = theirs.clone();
    s.sigs.retain(|sig| {
        sig.share_index != own_share as i32
            && verify_consent_signature(&frozen, sig.share_index, &sig.signature)
    });
    s.stage = if s.sigs.is_empty() {
        Stage::Frozen
    } else {
        Stage::Signing
    };
    Ok(s)
}

/// A copy at one of the funding stages merged per input (m-7); a Final
/// Record is merged as a signed copy and then kept as `broadcast`.
fn merged_inputs(l: &Loaded, theirs: &Session) -> Result<Session, MasternodesError> {
    let as_signed = |s: &Session| {
        let mut s = s.clone();
        if s.stage == Stage::Broadcast {
            s.stage = Stage::FundingSigned;
        }
        s
    };
    let ours = as_signed(&l.session);
    let mut merged = merge_funding_inputs(&ours, &as_signed(theirs), &l.own_inputs(), l.out())
        .map_err(merge_refusal)?;
    let broadcast = theirs.stage == Stage::Broadcast || l.session.stage == Stage::Broadcast;
    if broadcast && merged.stage == Stage::FundingSigned {
        merged.stage = Stage::Broadcast;
    }
    Ok(merged)
}

/// The participant's rows of §3.0.
fn participant(l: &Loaded, theirs: &Session) -> Result<Option<Session>, MasternodesError> {
    let ours = &l.session;
    let rev = theirs.revision.cmp(&ours.revision);
    let Some(rec) = &l.recorded else {
        // `invited`.
        if frozen_or_later(theirs.stage) {
            return Err(message_refusal("not_participant"));
        }
        if rev == Ordering::Greater {
            let mut s = theirs.clone();
            s.sigs.clear();
            return Ok(Some(s));
        }
        return Err(fallback(ours, theirs));
    };
    let share = rec.share_index;
    let joined = ours.stage == Stage::Draft;
    let approving = locked(ours.stage) && !l.out();
    if joined && theirs.stage == Stage::Draft && rev == Ordering::Greater {
        own_unchanged(ours, theirs, share).map_err(merge_refusal)?;
        return Ok(Some(theirs.clone()));
    }
    if joined && locked(theirs.stage) {
        let adopted = adopt_locked_terms(ours, theirs).map_err(merge_refusal)?;
        return relocked(&adopted, share).map(Some);
    }
    if approving && rev == Ordering::Greater && theirs.stage == Stage::Draft {
        // M-1 case 1: back to `joined`; the approval goes with the old copy.
        own_unchanged(ours, theirs, share).map_err(merge_refusal)?;
        let mut s = theirs.clone();
        s.sigs.clear();
        return Ok(Some(s));
    }
    if approving && rev == Ordering::Greater && locked(theirs.stage) {
        own_unchanged(ours, theirs, share).map_err(merge_refusal)?;
        return relocked(theirs, share).map(Some);
    }
    if locked(ours.stage) && locked(theirs.stage) && rev == Ordering::Equal {
        // Others' approvals; a different one for our share is skipped.
        let (merged, _) = merge_approvals(ours, theirs).map_err(merge_refusal)?;
        return Ok(Some(merged));
    }
    if locked(ours.stage)
        && l.own_approval()
        && theirs.stage == Stage::Combined
        && rev == Ordering::Equal
    {
        let (merged, _) = merge_approvals(ours, theirs).map_err(merge_refusal)?;
        return adopt_combined(&merged, theirs, &l.own_inputs())
            .map(Some)
            .map_err(merge_refusal);
    }
    if combined_or_later(theirs.stage) && rev == Ordering::Greater {
        return Err(message_refusal("conflict"));
    }
    if (combined_or_later(ours.stage) || l.out()) && rev == Ordering::Greater {
        return Err(message_refusal("conflict"));
    }
    if combined_or_later(ours.stage) && combined_or_later(theirs.stage) && rev == Ordering::Equal {
        return merged_inputs(l, theirs).map(Some);
    }
    Err(fallback(ours, theirs))
}

/// The coordinator's rows of §3.0. A combine follows the last approval.
fn coordinator(l: &Loaded, theirs: &Session) -> Result<Option<Session>, MasternodesError> {
    let ours = &l.session;
    let rev = theirs.revision.cmp(&ours.revision);
    if ours.stage == Stage::Draft {
        if theirs.stage != Stage::Draft {
            return Err(message_refusal("older"));
        }
        let own_label = l.recorded.as_ref().map_or("", |r| r.label.as_str());
        let merged = absorb_draft_reply(ours, theirs, own_label).map_err(merge_refusal)?;
        if let Some(m) = &merged {
            m.check_labels().map_err(envelope_refusal)?;
        }
        return Ok(merged);
    }
    if locked(ours.stage) && theirs.stage == Stage::Draft {
        return Err(message_refusal("older"));
    }
    if locked(ours.stage) && locked(theirs.stage) && rev == Ordering::Equal {
        let (mut merged, skipped) = merge_approvals(ours, theirs).map_err(merge_refusal)?;
        // Stricter than dash-qt's warning: a bad or second approval refuses
        // the whole message.
        if let Some(s) = skipped.first() {
            let share = match s {
                SkippedSignature::Invalid { share } | SkippedSignature::Different { share } => {
                    *share
                }
            };
            return Err(MasternodesError::SharedMessage {
                reason: "bad_signature".into(),
                share: u16::try_from(share).ok(),
            });
        }
        if merged.signed_count() == merged.shares.len() {
            combine(&mut merged)?;
        }
        return Ok(Some(merged));
    }
    let funding = |s: Stage| matches!(s, Stage::Combined | Stage::FundingSigned);
    if funding(ours.stage) && funding(theirs.stage) && rev == Ordering::Equal {
        return merged_inputs(l, theirs).map(Some);
    }
    if rev == Ordering::Greater || (rev == Ordering::Equal && theirs.stage > ours.stage) {
        return Err(message_refusal("conflict"));
    }
    Err(fallback(ours, theirs))
}

/// The table's last rows: a lower revision is `older`; the same copy
/// again is caught before; anything else `conflict`.
fn fallback(ours: &Session, theirs: &Session) -> MasternodesError {
    if theirs.revision < ours.revision
        || (theirs.revision == ours.revision && theirs.stage < ours.stage)
    {
        message_refusal("older")
    } else {
        message_refusal("conflict")
    }
}

/// Applies `theirs` to the wallet's session of its id (`ours`).
pub(crate) fn apply(ours: Option<&Loaded>, theirs: &Session) -> Result<Applied, MasternodesError> {
    let Some(l) = ours else {
        return first(theirs);
    };
    if l.session.message_code() == theirs.message_code() {
        return Ok(Applied::Same);
    }
    let next = match l.role {
        SharedRole::Participant => participant(l, theirs)?,
        SharedRole::Coordinator => coordinator(l, theirs)?,
    };
    Ok(match next {
        Some(s) if s.message_code() != l.session.message_code() => Applied::Update(s),
        _ => Applied::Same,
    })
}
