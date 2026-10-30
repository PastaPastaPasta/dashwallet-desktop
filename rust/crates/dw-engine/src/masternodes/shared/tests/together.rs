//! Dissolve together (MG-04b §3.3, slice 2c) on [`listed`]'s three-party
//! masternode: A (wallet 0) holds share 0 (400), B and C 300 each. One
//! wallet prepares, every owner approves from the copied text, any wallet
//! holding every signature sends.

use std::collections::BTreeMap;

use dw_protx::shared::envelope::{
    ReceivedSigs, SharedMessage as Message, SigEntry, SigsEnvelope, SigsKind, decode_message,
    encode_base64, fingerprint,
};
use dw_protx::shared::rules::{SharedMn, check_dissolution, verify_hash_canonical};
use dw_protx::shared::together::{self, Checked};
use dw_protx::shared::{ProDisTx, ProRegTxV3, SpecialTx, TRANSACTION_PROVIDER_DISSOLVE};

use super::dissolve::{assert_update_pending, delisted, dissolve_now, listed, request};
use super::*;
use crate::masternodes::{
    DeadReason, ProTxOperation, ProTxOperationKind, ProTxOperationState, SharedKind, SharedWarning,
};

const FEE_T: u64 = 10_000;
/// A well-formed session id no wallet holds.
const SESSION_NONE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The registration's payload, as the history holds it.
fn registration(f: &Fx, id: &str) -> ProRegTxV3 {
    decode_frozen(&parse(&f.message(0, id))).unwrap().payload
}

fn shared_mn(p: &ProRegTxV3, txid: Txid) -> SharedMn<'_> {
    SharedMn {
        pro_tx_hash: txid,
        collateral: OutPoint::new(txid, p.collateral.vout),
        shares: &p.shares,
        registered_height: 50,
        early_period_blocks: p.early_period_blocks,
        early_penalty: p.early_penalty,
        voting_key_id: [1; 20],
    }
}

/// A sigs envelope as dash-qt writes it.
fn sigs_envelope(hash: Txid, tx: &SpecialTx, signatures: &[(u16, [u8; 65])]) -> SigsEnvelope {
    SigsEnvelope {
        network: dashcore::Network::Regtest,
        kind: SigsKind::Dissolve,
        pro_tx_hash: hash,
        tx: tx.to_bytes(),
        signatures: signatures.to_vec(),
    }
}

fn sigs_text(hash: Txid, tx: &SpecialTx, signatures: &[(u16, [u8; 65])]) -> String {
    sigs_envelope(hash, tx, signatures).to_text()
}

fn read_sigs(text: &str) -> ReceivedSigs {
    match decode_message(text, dashcore::Network::Regtest).unwrap() {
        Message::Sigs(r) => r,
        other => panic!("not a sigs envelope: {other:?}"),
    }
}

/// A message's verified signatures by share.
fn signatures_of(text: &str) -> BTreeMap<u16, [u8; 65]> {
    read_sigs(text)
        .entries
        .flatten()
        .unwrap()
        .into_iter()
        .map(|e| match e {
            SigEntry::Signature { index, sig } => (u16::try_from(index).unwrap(), sig),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn start(
    f: &Fx,
    w: usize,
    txid: Txid,
    actor: u16,
    max: u64,
) -> Result<SharedOutcome, MasternodesError> {
    f.run(f.mn().start_dissolve_together(
        f.wallets[w],
        request(txid, actor, FEE_T),
        f.grant(w, max),
    ))
}

fn approve(
    f: &Fx,
    w: usize,
    id: &str,
    max: u64,
    accept: bool,
) -> Result<SharedOutcome, MasternodesError> {
    f.run(
        f.mn()
            .approve_maintenance(f.wallets[w], id.to_string(), f.grant(w, max), accept),
    )
}

fn send(f: &Fx, w: usize, id: &str) -> Result<ProTxOperation, MasternodesError> {
    f.run(
        f.mn()
            .send_maintenance(f.wallets[w], id.to_string(), f.grant(w, 0)),
    )
}

fn import_into(
    f: &Fx,
    w: usize,
    text: &str,
    target: Option<&str>,
) -> Result<SharedImport, MasternodesError> {
    f.run(
        f.mn()
            .import_shared_message(f.wallets[w], text.to_string(), target.map(Into::into)),
    )
}

/// The import's session, whether it changed, its code and warnings.
fn imported_with(
    f: &Fx,
    w: usize,
    text: &str,
    target: Option<&str>,
) -> (SharedSession, bool, String, Vec<SharedWarning>) {
    match import_into(f, w, text, target).unwrap() {
        SharedImport::Session {
            session,
            changed,
            received_code,
            warnings,
        } => (*session, changed, received_code, warnings),
        other => panic!("not a session: {other:?}"),
    }
}

fn flags(s: &SharedSession) -> (Vec<bool>, Vec<bool>) {
    (
        s.rows.iter().map(|r| r.mine).collect(),
        s.rows.iter().map(|r| r.approved).collect(),
    )
}

fn assert_invalid(e: MasternodesError) {
    assert!(
        matches!(e.engine(), Some(crate::EngineError::InvalidArgument(_))),
        "{e:?}"
    );
}

/// A prepares with its share the actor, B and C approve from the copied
/// text, C (whose share isn't the actor's) sends: Core's combined bytes,
/// charged the fee once, at A's start (§6.3).
#[test]
fn test_QT_127_three_wallets_dissolve_together() {
    let f = fx(3, true);
    let (reg_id, txid) = listed(&f);
    let p = registration(&f, &reg_id);
    let mn = shared_mn(&p, txid);

    // The fee comes from A's share, so A's start is charged it.
    assert_eq!(
        send_code(&start(&f, 0, txid, 0, FEE_T - 1).unwrap_err()),
        "send.grant_exceeded"
    );
    let a = start(&f, 0, txid, 0, FEE_T).unwrap();
    let id = a.session.id.clone();
    let s = &a.session;
    assert_eq!(
        (s.kind, s.role, s.stage, s.next),
        (
            SharedKind::DissolveTogether,
            SharedRole::Coordinator,
            SharedStage::Signing,
            SharedNext::Copy
        )
    );
    assert_eq!(
        (s.fee, s.debit, s.actor_fee),
        (Some(FEE_T), Some(FEE_T), Some(FEE_T))
    );
    assert_eq!(
        flags(s),
        (vec![true, false, false], vec![true, false, false])
    );
    assert_eq!(s.pro_tx_hash.as_deref(), Some(txid.to_string().as_str()));
    assert_eq!(a.message.name, "Request");
    assert_eq!(
        a.message.file_name,
        format!("shared-mn-session-{}.json", s.code)
    );
    assert_eq!(s.code, id[..6].to_ascii_uppercase());
    // The request is `PrepareSharedDissolution`'s, the id m-17's.
    let received = read_sigs(&a.message.text);
    let prepared = together::prepare(&mn, 0, FEE_T).unwrap();
    assert_eq!(received.tx.as_ref(), Some(&prepared.tx));
    let checked: Checked = together::check(&prepared.tx, &mn).unwrap();
    assert_eq!(
        id,
        super::super::together::session_id(txid.to_byte_array(), checked.sign_hash)
    );
    assert_eq!(a.message.code, s.message_code);
    // Starting it again signs nothing new and charges nothing.
    assert_eq!(start(&f, 0, txid, 0, 0).unwrap().session.id, id);

    // B imports the Request: nothing of its own is charged.
    let (b, changed, code_b, warnings) = imported_with(&f, 1, &a.message.text, None);
    assert!(changed && warnings.is_empty());
    assert_eq!(code_b, a.message.code);
    assert_eq!(
        (b.id.as_str(), b.role, b.stage, b.next),
        (
            id.as_str(),
            SharedRole::Participant,
            SharedStage::Signing,
            SharedNext::Approve
        )
    );
    assert_eq!((b.debit, b.actor_fee), (Some(0), None));
    assert_eq!(
        flags(&b),
        (vec![false, true, false], vec![true, false, false])
    );
    let b = approve(&f, 1, &id, 0, false).unwrap();
    assert_eq!(b.message.name, "Reply");
    assert_eq!(
        (b.session.stage, b.session.next),
        (SharedStage::Signing, SharedNext::Copy)
    );
    assert_eq!(flags(&b.session).1, [true, true, false]);

    // C takes B's Reply and completes it.
    f.imported(2, &b.message.text);
    let c = approve(&f, 2, &id, 0, false).unwrap();
    assert_eq!(
        (c.session.stage, c.session.next),
        (SharedStage::Combined, SharedNext::Send)
    );
    // A absorbs C's Reply once.
    let (a2, changed, _, _) = imported_with(&f, 0, &c.message.text, None);
    assert!(changed);
    assert_eq!(
        (a2.stage, a2.next),
        (SharedStage::Combined, SharedNext::Send)
    );
    assert!(
        !imported_with(&f, 0, &c.message.text, None).1,
        "held already"
    );

    // C sends: its share isn't the actor's, so the row is foreign and
    // charged nothing; the switch refuses it.
    let off = f.run(
        f.sending_off()
            .send_maintenance(f.wallets[2], id.clone(), f.grant(2, 0)),
    );
    assert!(
        matches!(off, Err(MasternodesError::Unsupported { .. })),
        "{off:?}"
    );
    let op = send(&f, 2, &id).unwrap();
    assert_eq!(
        (op.kind, op.state, op.fee),
        (
            ProTxOperationKind::Dissolve,
            ProTxOperationState::Sent,
            FEE_T
        )
    );
    let sent = f.net.sent();
    assert_eq!(sent.len(), 1);
    let tx = &sent[0];
    let special = SpecialTx::from_bytes(&dashcore::consensus::serialize(tx)).unwrap();
    assert_eq!(special.tx_type, TRANSACTION_PROVIDER_DISSOLVE);
    let sigs = signatures_of(&c.message.text);
    assert_eq!(special, together::combine(&checked, &mn, &sigs).unwrap());
    let payload: ProDisTx = dashcore::consensus::deserialize(&special.payload).unwrap();
    assert_eq!((payload.actor_index, payload.sigs.len()), (0, 3));
    for (i, sig) in payload.sigs.iter().enumerate() {
        assert!(verify_hash_canonical(
            checked.sign_hash,
            p.shares[i].owner_key_id,
            sig
        ));
    }
    check_dissolution(&special, &payload, Some(&mn), 101, true, true).unwrap();
    let c = f.session_of(2, &id);
    assert_eq!((c.stage, c.next), (SharedStage::Sent, SharedNext::Wait));
    assert_eq!(c.txid, Some(tx.txid().to_string()));
    let row: serde_json::Value = serde_json::from_str(
        &f.db()
            .masternode_op(&f.wallets[2].to_string(), &op.id)
            .unwrap()
            .unwrap()
            .request,
    )
    .unwrap();
    assert_eq!(
        (
            row["penalty"].as_u64(),
            row["foreign"].as_bool(),
            row["together"].as_bool()
        ),
        (Some(0), Some(true), Some(true))
    );
    // Resent at no charge; it can't be discarded or abandoned.
    f.run(
        f.mn()
            .resend_operation(f.wallets[2], op.id.clone(), f.grant(2, 0)),
    )
    .unwrap();
    assert_eq!(f.net.sent().len(), 2);
    let discard = f.run(f.mn().discard_shared_session(f.wallets[2], id.clone()));
    assert_stage(discard.unwrap_err(), "sent");
    assert_invalid(
        f.run(
            f.mn()
                .abandon_shared_session(f.wallets[2], id.clone(), f.grant(2, 0)),
        )
        .unwrap_err(),
    );
    // D23: the sent dissolution holds C's own.
    assert_update_pending(dissolve_now(&f, txid, 2).unwrap_err(), &op.id);

    // Mined: every wallet's session reads confirmed.
    f.mine(tx.clone(), 101);
    for w in 0..3 {
        let s = f.session_of(w, &id);
        assert_eq!(
            (s.stage, s.next),
            (SharedStage::Confirmed { height: 101 }, SharedNext::Nothing)
        );
    }
    // The registration session is still listed beside it.
    let kinds: Vec<_> = f
        .run(f.mn().shared_sessions(f.wallets[1]))
        .unwrap()
        .into_iter()
        .map(|s| (s.kind, s.id == id))
        .collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&(SharedKind::DissolveTogether, true)));
    assert!(kinds.contains(&(SharedKind::Registration, false)));
}

/// D23 the other way: a sent dissolution of the masternode holds the
/// start and the send of a together session.
#[test]
fn test_QT_127_a_sent_dissolution_holds_dissolve_together() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let a = start(&f, 0, txid, 0, FEE_T).unwrap();
    let id = a.session.id;
    f.imported(1, &a.message.text);
    let b = approve(&f, 1, &id, 0, false).unwrap();
    f.imported(2, &b.message.text);
    assert_eq!(
        approve(&f, 2, &id, 0, false).unwrap().session.stage,
        SharedStage::Combined
    );
    let op = dissolve_now(&f, txid, 2).unwrap();
    assert_update_pending(send(&f, 2, &id).unwrap_err(), &op.id);
    assert_update_pending(start(&f, 2, txid, 2, FEE_T).unwrap_err(), &op.id);
    assert_eq!(f.net.sent().len(), 1);
}

/// D23 at approval: a wallet whose own dissolution of the masternode may
/// be mined signs nothing more for it, and isn't charged.
#[test]
fn test_QT_127_a_sent_dissolution_holds_approve_maintenance() {
    let f = fx(3, true);
    let (reg_id, txid) = listed(&f);
    let p = registration(&f, &reg_id);
    // B's share pays the fee, so B's approval would be charged it.
    let prepared = together::prepare(&shared_mn(&p, txid), 1, FEE_T).unwrap();
    let (b, _, _, _) = imported_with(&f, 1, &sigs_text(txid, &prepared.tx, &[]), None);
    let op = dissolve_now(&f, txid, 1).unwrap();
    assert_update_pending(approve(&f, 1, &b.id, FEE_T, true).unwrap_err(), &op.id);
    assert_eq!(flags(&f.session_of(1, &b.id)).1, [false, false, false]);
}

/// Starting a request this wallet imported first signs it and keeps the
/// session a participant's, its message the Reply; the charge is the
/// fee, the actor share being this wallet's.
#[test]
fn test_QT_127_starting_an_imported_request_signs_it() {
    let f = fx(3, true);
    let (reg_id, txid) = listed(&f);
    let p = registration(&f, &reg_id);
    let prepared = together::prepare(&shared_mn(&p, txid), 0, FEE_T).unwrap();
    let (a, _, _, _) = imported_with(&f, 0, &sigs_text(txid, &prepared.tx, &[]), None);
    assert_eq!(a.role, SharedRole::Participant);
    assert_eq!(
        send_code(&start(&f, 0, txid, 0, FEE_T - 1).unwrap_err()),
        "send.grant_exceeded"
    );
    let started = start(&f, 0, txid, 0, FEE_T).unwrap();
    assert_eq!(started.session.id, a.id);
    assert_eq!(
        (started.session.role, started.message.name.as_str()),
        (SharedRole::Participant, "Reply")
    );
    assert_eq!(flags(&started.session).1, [true, false, false]);
}

/// m-15: a request another wallet prepared that takes its fee from this
/// wallet's share is approved only with `accept_actor_fee`, unless the fee
/// is the relay minimum; below it the request is refused on import (D96).
#[test]
fn test_QT_127_an_actor_fee_needs_acceptance() {
    let f = fx(3, true);
    let (reg_id, txid) = listed(&f);
    let p = registration(&f, &reg_id);
    let mn = shared_mn(&p, txid);
    // dash-qt's RPC prepares with B's share the actor.
    let prepared = together::prepare(&mn, 1, FEE_T).unwrap();
    let (b, _, _, _) = imported_with(&f, 1, &sigs_text(txid, &prepared.tx, &[]), None);
    assert_eq!(
        (b.debit, b.actor_fee, b.next),
        (Some(FEE_T), Some(FEE_T), SharedNext::Approve)
    );
    match approve(&f, 1, &b.id, FEE_T, false).unwrap_err() {
        e @ MasternodesError::SharedActorFee { fee } => {
            assert_eq!(fee, FEE_T);
            assert_eq!(e.code(), "masternodes.shared_actor_fee");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        send_code(&approve(&f, 1, &b.id, FEE_T - 1, true).unwrap_err()),
        "send.grant_exceeded"
    );
    assert_eq!(flags(&f.session_of(1, &b.id)).1, [false, false, false]);
    let ok = approve(&f, 1, &b.id, FEE_T, true).unwrap();
    assert_eq!(flags(&ok.session).1, [false, true, false]);
    // C holds no actor share: no acceptance, no charge.
    f.imported(2, &ok.message.text);
    approve(&f, 2, &b.id, 0, false).unwrap();

    // At the relay minimum of the combined size no acceptance is needed.
    let size = together::prepare(&mn, 1, 1)
        .unwrap()
        .signed(vec![[0; 65]; 3])
        .to_bytes()
        .len() as u64;
    let floor = together::prepare(&mn, 1, size).unwrap();
    let (b, _, _, _) = imported_with(&f, 1, &sigs_text(txid, &floor.tx, &[]), None);
    approve(&f, 1, &b.id, size, false).unwrap();
    // One duff less is never imported.
    let below = together::prepare(&mn, 1, size - 1).unwrap();
    assert_invalid(import_into(&f, 1, &sigs_text(txid, &below.tx, &[]), None).unwrap_err());
}

/// §2.5 and D92 at import: the request must be the registration's, built
/// by Core's builder byte for byte; signatures are taken one by one as
/// dash-qt takes them; a bare array needs its target; the key change's
/// envelope stays unsupported (D45).
#[test]
fn test_QT_127_dissolve_together_import_refusals_and_skips() {
    let f = fx(3, true);
    let (reg_id, txid) = listed(&f);
    let p = registration(&f, &reg_id);
    let mn = shared_mn(&p, txid);
    let a = start(&f, 0, txid, 0, FEE_T).unwrap();
    let id = a.session.id.clone();
    let tx = read_sigs(&a.message.text).tx.unwrap();
    let a_sig = signatures_of(&a.message.text)[&0];

    let refused = |text: &str| import_into(&f, 1, text, None).unwrap_err();
    let registrar = SigsEnvelope {
        kind: SigsKind::Registrar,
        ..sigs_envelope(txid, &tx, &[])
    }
    .to_text();
    assert!(matches!(
        refused(&registrar),
        MasternodesError::Unsupported { .. }
    ));
    // Before its target is looked up.
    assert!(matches!(
        import_into(&f, 1, &registrar, Some(SESSION_NONE)).unwrap_err(),
        MasternodesError::Unsupported { .. }
    ));
    let testnet = SigsEnvelope {
        network: dashcore::Network::Testnet,
        ..sigs_envelope(txid, &tx, &[])
    }
    .to_text();
    assert_message(refused(&testnet), "network");
    // Another masternode's request under this one's hash.
    let other_hash = Txid::from_byte_array([9; 32]);
    let other = together::prepare(&shared_mn(&p, other_hash), 0, FEE_T).unwrap();
    assert_message(
        refused(&sigs_text(txid, &other.tx, &[])),
        "other_masternode",
    );
    assert_eq!(
        refused(&sigs_text(other_hash, &other.tx, &[])).code(),
        "masternodes.shared_registration_unknown"
    );
    assert_message(
        refused(&sigs_text(
            txid,
            &together::prepare(&mn, 0, FEE_T)
                .unwrap()
                .signed(vec![[0; 65]; 3]),
            &[],
        )),
        "signed",
    );
    let mut edited = tx.clone();
    edited.output[0].value -= 1;
    assert_message(refused(&sigs_text(txid, &edited, &[])), "outputs");
    let mut locked = tx.clone();
    locked.lock_time = 1;
    assert_message(refused(&sigs_text(txid, &locked, &[])), "outputs");
    assert!(
        f.run(f.mn().shared_sessions(f.wallets[1]))
            .unwrap()
            .iter()
            .all(|s| s.id != id)
    );

    // `signatures` that isn't an array: the request is taken, with
    // dash-qt's warning (no fingerprint: not an edited copy).
    let mut odd: serde_json::Value = serde_json::from_str(&sigs_text(txid, &tx, &[])).unwrap();
    odd["signatures"] = "none".into();
    odd.as_object_mut().unwrap().remove("fingerprint");
    let (b, _, _, warnings) = imported_with(&f, 1, &odd.to_string(), None);
    assert_eq!(b.id, id);
    assert_eq!(warnings, [SharedWarning::NoApprovals]);

    // A signature that doesn't verify, and one out of range, are skipped;
    // A's is taken.
    let text = sigs_text(txid, &tx, &[(0, a_sig), (1, [7; 65]), (5, a_sig)]);
    let (b, changed, _, warnings) = imported_with(&f, 1, &text, None);
    assert!(changed);
    assert_eq!(flags(&b).1, [true, false, false]);
    assert_eq!(
        warnings,
        [SharedWarning::SkippedSignatures {
            shares: vec![1],
            count: 2
        }]
    );
    // Another signature for share 0 is skipped, the first kept.
    let (b, changed, _, warnings) =
        imported_with(&f, 1, &sigs_text(txid, &tx, &[(0, [8; 65])]), None);
    assert!(!changed);
    assert_eq!(flags(&b).1, [true, false, false]);
    assert_eq!(
        warnings,
        [SharedWarning::SkippedSignatures {
            shares: vec![0],
            count: 1
        }]
    );

    // B's signature as a bare array: refused without a target, taken by
    // A's session with it; its code is the array's own.
    let b_sig = signatures_of(&approve(&f, 1, &id, 0, false).unwrap().message.text)[&1];
    let bare = format!(
        "[{{\"shareIndex\": 1, \"signature\": \"{}\"}}]",
        encode_base64(&b_sig)
    );
    assert_message(import_into(&f, 0, &bare, None).unwrap_err(), "parse");
    assert_invalid(import_into(&f, 0, &bare, Some(&reg_id)).unwrap_err());
    let (a2, changed, code, _) = imported_with(&f, 0, &bare, Some(&id));
    assert!(changed);
    assert_eq!(flags(&a2).1, [true, true, false]);
    assert_eq!(code, fingerprint(&Json::read(bare.as_bytes()).unwrap()));
    // A message of another session for this target is refused.
    let other_fee = together::prepare(&mn, 0, FEE_T + 1).unwrap();
    assert_message(
        import_into(&f, 0, &sigs_text(txid, &other_fee.tx, &[]), Some(&id)).unwrap_err(),
        "other_session",
    );
}

/// The masternode out of the synced list before a send from here: the
/// session reads `dead{delisted}` and can be discarded.
#[test]
fn test_QT_127_a_delisted_masternode_kills_the_session() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let a = start(&f, 0, txid, 0, FEE_T).unwrap();
    let id = a.session.id;
    f.imported(1, &a.message.text);
    delisted(&f, 200);
    let s = f.session_of(0, &id);
    assert_eq!(
        (s.stage, s.next),
        (
            SharedStage::Dead {
                reason: DeadReason::Delisted
            },
            SharedNext::Nothing
        )
    );
    // Approve and send name the stage, before the list's refusal.
    assert_stage(send(&f, 0, &id).unwrap_err(), "dead");
    assert_stage(approve(&f, 1, &id, 0, false).unwrap_err(), "dead");
    f.run(f.mn().discard_shared_session(f.wallets[0], id.clone()))
        .unwrap();
    assert!(matches!(
        f.run(f.mn().shared_session(f.wallets[0], id)),
        Err(MasternodesError::SharedSessionNotFound)
    ));
}

/// A session survives a restart: the row is all of it.
#[test]
fn test_QT_127_a_dissolve_together_session_survives_a_restart() {
    let mut f = fx(3, true);
    let (_, txid) = listed(&f);
    let a = start(&f, 0, txid, 0, FEE_T).unwrap();
    f.imported(1, &a.message.text);
    // The seam's list and blocks are the test's, not the data's.
    let list = f.seam.list.lock().unwrap().clone();
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    f.restart();
    *f.seam.list.lock().unwrap() = list;
    *f.seam.shared_mined.lock().unwrap() = mined;
    f.follow();
    let b = approve(&f, 1, &a.session.id, 0, false).unwrap();
    assert_eq!(flags(&b.session).1, [true, true, false]);
    assert_eq!(f.session_of(0, &a.session.id).stage, SharedStage::Signing);
}

/// The switch refuses the three signing calls; import and reads work.
#[test]
fn test_QT_127_dissolve_together_behind_the_sending_switch() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let off = f.sending_off();
    let unsupported = |r: Result<SharedOutcome, MasternodesError>| {
        assert!(
            matches!(r, Err(MasternodesError::Unsupported { .. })),
            "{r:?}"
        )
    };
    unsupported(f.run(off.start_dissolve_together(
        f.wallets[0],
        request(txid, 0, FEE_T),
        f.grant(0, FEE_T),
    )));
    let a = start(&f, 0, txid, 0, FEE_T).unwrap();
    f.imported(1, &a.message.text);
    unsupported(f.run(off.approve_maintenance(
        f.wallets[1],
        a.session.id.clone(),
        f.grant(1, 0),
        false,
    )));
    assert_eq!(f.session_of(1, &a.session.id).stage, SharedStage::Signing);
}

/// The 2c stages in the contract's encoding (D94): the existing
/// `SharedStage` values, unchanged.
#[test]
fn test_QT_127_dissolve_together_stages_decode() {
    let cases = [
        (SharedStage::Signing, r#"{"kind":"signing"}"#),
        (SharedStage::Combined, r#"{"kind":"combined"}"#),
        (SharedStage::Sent, r#"{"kind":"sent"}"#),
        (
            SharedStage::Confirmed { height: 101 },
            r#"{"kind":"confirmed","height":101}"#,
        ),
        (SharedStage::Conflicted, r#"{"kind":"conflicted"}"#),
        (
            SharedStage::Dead {
                reason: DeadReason::Delisted,
            },
            r#"{"kind":"dead","reason":"delisted"}"#,
        ),
    ];
    for (stage, json) in cases {
        assert_eq!(serde_json::to_string(&stage).unwrap(), json);
        assert_eq!(serde_json::from_str::<SharedStage>(json).unwrap(), stage);
    }
    assert_eq!(
        serde_json::to_string(&SharedKind::DissolveTogether).unwrap(),
        r#""dissolve_together""#
    );
}
