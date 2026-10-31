//! Change keys together (MG-04b §3.3, D25, D99–D105, slice 2d).

use dashcore::hashes::sha256;
use dashcore::secp256k1::{Message as Digest, ecdsa};
use dw_protx::shared::envelope::{SigsEnvelope, SigsKind, decode_message};
use dw_protx::shared::key_change;
use dw_protx::shared::rules::{SharedMn, check_update_shared_registrar};
use dw_protx::shared::{
    ProRegTxV3, ProUpSharedRegTx, SpecialTx, TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR,
    payload_bytes,
};

use super::super::store::reserved_of;
use super::dissolve::{assert_update_pending, delisted, dissolve_now, listed};
use super::together::scanned_to;
use super::*;
use crate::masternodes::{
    DeadReason, KeepOr, KeyChangeRequest, MaintenanceSent, OperatorChoice, SharedKind,
    SharedWarning, VotingChoice,
};

/// The BLS12-381 G1 generator, basic serialization: a valid operator key
/// for the list's entry.
const LISTED_OPERATOR: &str = "97f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00adb22c6bb";

fn listed_operator() -> [u8; 48] {
    hex::decode(LISTED_OPERATOR).unwrap().try_into().unwrap()
}

/// The list's entry for `txid` with these keys (the service kept).
fn list_keys(f: &Fx, txid: Txid, operator: [u8; 48], voting: [u8; 20]) {
    let entry = platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: txid.to_byte_array(),
        service_address: Some("1.2.3.4:19999".parse().unwrap()),
        platform_http_port: None,
        operator_public_key: operator,
        voting_key_id: voting,
        platform_node_id: None,
        is_valid: true,
        is_evonode: false,
        has_extended_net_info: true,
        net_info: None,
    };
    *f.seam.list.lock().unwrap() = Some(vec![entry]);
}

/// A listed shared masternode of three wallets, one share each, its list
/// entry holding a valid operator key, and fee coins for wallet 0.
fn keyed(f: &Fx) -> (String, Txid) {
    let (id, txid) = listed(f);
    list_keys(f, txid, listed_operator(), [1; 20]);
    // The registration spent its contributions: no wallet holds them now.
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    for (_, tx, _) in mined {
        for input in &tx.input {
            for w in 0..f.wallets.len() {
                f.spend(w, input.previous_output);
            }
        }
    }
    f.credit(0, 9, COIN);
    (id, txid)
}

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

/// The operator key set to the keychain's next unused, the voting key kept.
fn rotate(txid: Txid) -> KeyChangeRequest {
    KeyChangeRequest {
        pro_tx_hash: txid.to_string(),
        operator: KeepOr::Set(OperatorChoice::NextUnused),
        voting: KeepOr::Keep,
    }
}

fn start(
    f: &Fx,
    w: usize,
    request: KeyChangeRequest,
    max: u64,
) -> Result<SharedOutcome, MasternodesError> {
    f.run(
        f.mn()
            .start_key_change(f.wallets[w], request, f.grant(w, max)),
    )
}

fn approve(f: &Fx, w: usize, id: &str, max: u64) -> Result<SharedOutcome, MasternodesError> {
    f.run(
        f.mn()
            .approve_maintenance(f.wallets[w], id.to_string(), f.grant(w, max), false),
    )
}

fn send(f: &Fx, w: usize, id: &str, max: u64) -> Result<SharedSession, MasternodesError> {
    f.run(
        f.mn()
            .send_maintenance(f.wallets[w], id.to_string(), f.grant(w, max)),
    )
    .map(|sent| match sent {
        MaintenanceSent::Session { session } => *session,
        MaintenanceSent::Operation { .. } => panic!("a key change is sent as its session"),
    })
}

fn abandon(f: &Fx, w: usize, id: &str, max: u64) -> Result<SharedSession, MasternodesError> {
    f.run(
        f.mn()
            .abandon_shared_session(f.wallets[w], id.to_string(), f.grant(w, max)),
    )
}

fn import_into(f: &Fx, w: usize, text: &str) -> Result<SharedImport, MasternodesError> {
    f.run(
        f.mn()
            .import_shared_message(f.wallets[w], text.to_string(), None),
    )
}

/// A starts, B and C approve from the copied text, A takes C's Reply: the
/// session id, A's start and the request's fee.
fn complete(f: &Fx, txid: Txid) -> (String, SharedOutcome) {
    let a = start(f, 0, rotate(txid), 0).unwrap();
    let id = a.session.id.clone();
    f.imported(1, &a.message.text);
    let b = approve(f, 1, &id, 0).unwrap();
    f.imported(2, &b.message.text);
    let c = approve(f, 2, &id, 0).unwrap();
    assert_eq!(c.session.stage, SharedStage::Combined);
    f.imported(0, &c.message.text);
    (id, a)
}

/// Whether every input of `tx` carries a P2PKH signature valid over `tx`
/// as it stands (SIGHASH_ALL, the payload included).
fn inputs_spend(tx: &SpecialTx) -> bool {
    (0..tx.input.len()).all(|index| {
        let mut pushes = tx.input[index].script_sig.instructions();
        let (Some(Ok(sig)), Some(Ok(key))) = (pushes.next(), pushes.next()) else {
            return false;
        };
        let (Some(sig), Some(key)) = (sig.push_bytes(), key.push_bytes()) else {
            return false;
        };
        let Ok(pk) = dashcore::PublicKey::from_slice(key.as_bytes()) else {
            return false;
        };
        let code = dashcore::ScriptBuf::new_p2pkh(&pk.pubkey_hash());
        let hash = tx.signature_hash_all(index, &code).unwrap();
        let der = &sig.as_bytes()[..sig.len().saturating_sub(1)];
        ecdsa::Signature::from_der(der)
            .and_then(|s| ecdsa::verify(&s, Digest::from_digest(hash), &pk.inner))
            .is_ok()
    })
}

fn special(tx: &Transaction) -> SpecialTx {
    SpecialTx::from_bytes(&dashcore::consensus::serialize(tx)).unwrap()
}

/// A prepares (charged 0), B and C approve from the copied text (charged
/// 0), A alone sends, charged the fee: Core's request, the signatures in
/// share order, the fee inputs signed over the combined payload; then the
/// funder reads `confirmed` from its history, an approver from the list.
#[test]
fn test_QT_127_three_wallets_change_keys_together() {
    let f = fx(3, true);
    let (reg_id, txid) = keyed(&f);
    let p = registration(&f, &reg_id);
    let mn = shared_mn(&p, txid);
    let network = dashcore::Network::Regtest;

    let a = start(&f, 0, rotate(txid), 0).unwrap();
    let id = a.session.id.clone();
    let s = &a.session;
    assert_eq!(
        (s.kind, s.role, s.stage, s.next),
        (
            SharedKind::KeyChange,
            SharedRole::Coordinator,
            SharedStage::Signing,
            SharedNext::Copy
        )
    );
    let fee = s.fee.unwrap();
    assert!(fee > 0);
    assert_eq!(s.debit, Some(fee));
    assert_eq!(a.message.name, "Request");
    // The base is the list's keys; the request sets a new operator key and
    // keeps the voting key.
    let base = s.base_keys.clone().unwrap();
    assert_eq!(base.operator_public_key, LISTED_OPERATOR);
    let view = s.key_change.clone().unwrap();
    assert!(view.funder);
    assert_eq!(view.current.as_ref(), Some(&base));
    assert_ne!(view.requested.operator_public_key, base.operator_public_key);
    assert_eq!(view.requested.voting_address, base.voting_address);
    assert_eq!(
        s.warnings,
        [SharedWarning::OperatorReset, SharedWarning::UsableByFunder]
    );
    // Its fee inputs are reserved and locked with the row.
    assert!(!s.reserved.is_empty());
    let reserved: Vec<OutPoint> = reserved_of(&f.row(0, &id));
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));

    // Core's request: type 12 on A's fee inputs, one empty signature per
    // share, `inputsHash`; no fee-input signature leaves the wallet before
    // the send (R3, DEC-965).
    let received = match decode_message(&a.message.text, network).unwrap() {
        dw_protx::shared::envelope::SharedMessage::Sigs(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(received.kind, SigsKind::Registrar);
    let request = received.tx.clone().unwrap();
    assert_eq!(
        request.tx_type,
        TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR
    );
    assert!(request.input.iter().all(|i| i.script_sig.is_empty()));
    let checked = key_change::check(&request, &mn).unwrap();
    assert_eq!(checked.payload.sigs, vec![[0; 65]; 3]);
    assert_eq!(
        hex::encode(checked.keys.operator_public_key),
        view.requested.operator_public_key
    );
    assert_eq!(
        request
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>(),
        reserved
    );
    // m-17: SHA-256 of `registrar`, the ProTx hash and the sign hash.
    let mut preimage = b"registrar".to_vec();
    preimage.extend(txid.to_byte_array());
    preimage.extend(checked.sign_hash);
    assert_eq!(
        id,
        hex::encode(sha256::Hash::hash(&preimage).to_byte_array())
    );
    assert_eq!(
        flags_of(s),
        (vec![true, false, false], vec![true, false, false])
    );

    // B imports and approves, charged 0.
    let (b, changed) = f.imported(1, &a.message.text);
    assert!(changed);
    assert_eq!(
        (b.id.as_str(), b.role, b.stage, b.next, b.fee),
        (
            id.as_str(),
            SharedRole::Participant,
            SharedStage::Signing,
            SharedNext::Approve,
            None
        )
    );
    assert!(!b.key_change.as_ref().unwrap().funder);
    assert_eq!(b.base_keys.as_ref(), Some(&base));
    let b = approve(&f, 1, &id, 0).unwrap();
    assert_eq!(b.message.name, "Reply");
    f.imported(2, &b.message.text);
    let c = approve(&f, 2, &id, 0).unwrap();
    // C holds every signature but can't send: it copies them to A.
    assert_eq!(
        (c.session.stage, c.session.next),
        (SharedStage::Combined, SharedNext::Copy)
    );
    let e = send(&f, 2, &id, COIN).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::SharedStage { stage } if stage == "participant"),
        "{e:?}"
    );
    assert!(f.net.sent().is_empty());

    let (a2, _) = f.imported(0, &c.message.text);
    assert_eq!(
        (a2.stage, a2.next),
        (SharedStage::Combined, SharedNext::Send)
    );
    assert_eq!(
        send_code(&send(&f, 0, &id, fee - 1).unwrap_err()),
        "send.grant_exceeded"
    );
    assert!(f.net.sent().is_empty());
    let sent = send(&f, 0, &id, fee).unwrap();
    assert_eq!(sent.stage, SharedStage::Sent);
    let out = f.net.sent();
    assert_eq!(out.len(), 1);
    let tx = special(&out[0]);
    assert_eq!(
        sent.txid.as_deref(),
        Some(out[0].txid().to_string().as_str())
    );
    let p12: ProUpSharedRegTx = dashcore::consensus::deserialize(&tx.payload).unwrap();
    assert_eq!(p12.sigs.len(), 3);
    assert_eq!(p12.sign_hash(), checked.sign_hash);
    check_update_shared_registrar(&tx, &p12, Some(&mn), &[], true, true).unwrap();
    // The fee inputs are signed over the combined transaction, and a
    // signature over the request would no longer spend (R3).
    assert!(inputs_spend(&tx));
    let mut over_request = tx.clone();
    over_request.payload = payload_bytes(&checked.payload);
    assert!(!inputs_spend(&over_request));
    assert_eq!(
        (tx.output.clone(), tx.lock_time),
        (request.output.clone(), 0)
    );
    // Sent: the follower keeps its locks.
    f.follow();
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));

    // D23: nothing else of the masternode while it may be mined.
    f.credit(0, 10, COIN);
    assert_update_pending(start(&f, 0, rotate(txid), 0).unwrap_err(), &id);
    assert_update_pending(dissolve_now(&f, txid, 0).unwrap_err(), &id);

    // In a block: the funder's history; the list holds the new keys.
    f.mine(out[0].clone(), 101);
    let op_key: [u8; 48] = checked.keys.operator_public_key;
    list_keys(&f, txid, op_key, [1; 20]);
    assert_eq!(
        f.session_of(0, &id).stage,
        SharedStage::Confirmed { height: 101 }
    );
    // B can't see the transaction: confirmed by the list, at its height.
    assert_eq!(
        f.session_of(1, &id).stage,
        SharedStage::Confirmed { height: 100 }
    );
    // Buried, the funder's locks go.
    f.tip(200);
    f.follow();
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
}

fn flags_of(s: &SharedSession) -> (Vec<bool>, Vec<bool>) {
    (
        s.rows.iter().map(|r| r.mine).collect(),
        s.rows.iter().map(|r| r.approved).collect(),
    )
}

/// M-5 (mutant 22): once the list's keys leave the base, approve and send
/// refuse `shared_message{stale}` and the session reads `stale`; back on the
/// base, they go on.
#[test]
fn test_QT_127_a_key_change_is_stale_once_the_list_leaves_its_base() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let a = start(&f, 0, rotate(txid), 0).unwrap();
    let id = a.session.id.clone();
    f.imported(1, &a.message.text);
    list_keys(&f, txid, listed_operator(), [2; 20]);
    assert_eq!(f.session_of(1, &id).stage, SharedStage::Stale);
    assert_message(approve(&f, 1, &id, 0).unwrap_err(), "stale");
    // A later import doesn't record the base again (D104).
    import_into(&f, 1, &a.message.text).unwrap();
    assert_eq!(f.session_of(1, &id).stage, SharedStage::Stale);
    list_keys(&f, txid, listed_operator(), [1; 20]);
    let b = approve(&f, 1, &id, 0).unwrap();
    f.imported(2, &b.message.text);
    let c = approve(&f, 2, &id, 0).unwrap();
    f.imported(0, &c.message.text);
    list_keys(&f, txid, listed_operator(), [2; 20]);
    let s = f.session_of(0, &id);
    // The funder is offered the abandon (D25).
    assert_eq!((s.stage, s.next), (SharedStage::Stale, SharedNext::Abandon));
    assert_message(send(&f, 0, &id, COIN).unwrap_err(), "stale");
    assert!(f.net.sent().is_empty());
    // Stale holds its locks; the funder's exit is the abandon.
    f.follow();
    assert!(!f.locked(0).is_empty());
    list_keys(&f, txid, listed_operator(), [1; 20]);
    send(&f, 0, &id, COIN).unwrap();
    assert_eq!(f.net.sent().len(), 1);
}

/// R3 (DEC-965, D102): before its send the funder's abandon spends nothing,
/// charges nothing, releases the fee inputs and keeps the row
/// `dead{abandoned}`; the request is never sent from it again, a later
/// Reply doesn't revive it, and an approver has nothing to abandon. Started
/// again with the same keys, the same request replaces the abandoned row.
#[test]
fn test_QT_127_an_unsent_key_change_is_abandoned_without_a_spend() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, a) = complete(&f, txid);
    let reserved = reserved_of(&f.row(0, &id));
    assert!(!reserved.is_empty());
    let e = abandon(&f, 1, &id, 0).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::SharedStage { stage } if stage == "participant"),
        "{e:?}"
    );
    let s = abandon(&f, 0, &id, 0).unwrap();
    assert_eq!(
        (s.stage, s.next),
        (
            SharedStage::Dead {
                reason: DeadReason::Abandoned
            },
            SharedNext::Nothing
        )
    );
    assert!(f.net.sent().is_empty());
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
    assert!(s.reserved.is_empty());
    let e = send(&f, 0, &id, COIN).unwrap_err();
    assert_eq!(code(e), "masternodes.shared_stage");
    import_into(&f, 0, &a.message.text).unwrap();
    assert_eq!(
        f.session_of(0, &id).stage,
        SharedStage::Dead {
            reason: DeadReason::Abandoned
        }
    );
    assert_eq!(
        code(abandon(&f, 0, &id, 0).unwrap_err()),
        "masternodes.shared_stage"
    );
    // The same keys on the coins it released: the same request and id.
    let requested = s.key_change.clone().unwrap().requested.operator_public_key;
    let again = KeyChangeRequest {
        operator: KeepOr::Set(OperatorChoice::Given {
            public_key: requested,
        }),
        ..rotate(txid)
    };
    let b = start(&f, 0, again, 0).unwrap();
    assert_eq!(
        (b.session.id.as_str(), b.session.stage),
        (id.as_str(), SharedStage::Signing)
    );
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));
    // The row stays until discarded.
    f.run(f.mn().discard_shared_session(f.wallets[0], id.clone()))
        .unwrap();
    assert!(
        f.db()
            .shared_session(&f.wallets[0].to_string(), &id)
            .unwrap()
            .is_none()
    );
}

/// D102 out (mutant 31): a sent key change's abandon spends a fee input to
/// the wallet; the row and its locks stay until a block settles it.
#[test]
fn test_QT_127_a_sent_key_change_is_abandoned_by_a_spend_to_the_wallet() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    send(&f, 0, &id, COIN).unwrap();
    let reserved = reserved_of(&f.row(0, &id));
    let s = abandon(&f, 0, &id, COIN).unwrap();
    assert_eq!(s.stage, SharedStage::Sent);
    let out = f.net.sent();
    assert_eq!(out.len(), 2);
    let spend = out[1].clone();
    assert_eq!(spend.input.len(), 1);
    assert!(reserved.contains(&spend.input[0].previous_output));
    assert_eq!(
        f.row(0, &id).abandon_txid.as_deref(),
        Some(spend.txid().to_string().as_str())
    );
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));
    f.mine(spend, 102);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Conflicted);
    f.tip(103);
    f.follow();
    assert!(
        reserved.iter().all(|o| f.locked(0).contains(o)),
        "not buried"
    );
    f.tip(200);
    f.follow();
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
}

/// r1 M-1: a request out stays valid in Core whatever keys the list took
/// since, so once the list leaves its base for other keys than its own the
/// funder's session reads `stale`: no resend (it would undo the later
/// change), other maintenance still waits (Core may mine it), and the
/// abandon spends a fee input to the wallet. The list holding its own keys
/// before the scan finds its block is not stale.
#[test]
fn test_QT_127_a_sent_key_change_whose_base_moved_is_stale() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, a) = complete(&f, txid);
    let requested = a.session.key_change.unwrap().requested.operator_public_key;
    let requested: [u8; 48] = hex::decode(requested).unwrap().try_into().unwrap();
    send(&f, 0, &id, COIN).unwrap();
    list_keys(&f, txid, requested, [1; 20]);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Sent);
    list_keys(&f, txid, listed_operator(), [2; 20]);
    let s = f.session_of(0, &id);
    assert_eq!((s.stage, s.next), (SharedStage::Stale, SharedNext::Abandon));
    assert_message(send(&f, 0, &id, COIN).unwrap_err(), "stale");
    assert_eq!(f.net.sent().len(), 1);
    assert_update_pending(dissolve_now(&f, txid, 0).unwrap_err(), &id);
    f.follow();
    let reserved = reserved_of(&f.row(0, &id));
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));
    abandon(&f, 0, &id, COIN).unwrap();
    let out = f.net.sent();
    assert_eq!(out.len(), 2);
    assert!(reserved.contains(&out[1].input[0].previous_output));
}

/// r1 m-1: a request out whose masternode left the list can never be mined
/// (`bad-proupsharedreg-hash`): once the wallet has scanned to the list's
/// height it reads `dead{delisted}` and the follower releases its locks,
/// with no spend.
#[test]
fn test_QT_127_a_sent_key_change_of_a_delisted_masternode_is_dead() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    send(&f, 0, &id, COIN).unwrap();
    let reserved = reserved_of(&f.row(0, &id));
    delisted(&f, 200);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Sent);
    scanned_to(&f, 0, 200);
    assert_eq!(
        f.session_of(0, &id).stage,
        SharedStage::Dead {
            reason: DeadReason::Delisted
        }
    );
    f.follow();
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
    assert_eq!(f.net.sent().len(), 1);
}

/// A sent key change is resent as the same bytes, charged the fee again.
#[test]
fn test_QT_127_a_sent_key_change_is_resent() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let fee = f.session_of(0, &id).fee.unwrap();
    send(&f, 0, &id, fee).unwrap();
    assert_eq!(
        send_code(&send(&f, 0, &id, fee - 1).unwrap_err()),
        "send.grant_exceeded"
    );
    let s = send(&f, 0, &id, fee).unwrap();
    assert_eq!(s.stage, SharedStage::Sent);
    let out = f.net.sent();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].txid(), out[1].txid());
}

/// D99: an open key change's keys are in use: the keychain's next unused
/// operator key skips its key, and naming it is `protx-dup` naming the
/// session (MG-04 D51), as Core's mempool indexes it.
#[test]
fn test_QT_127_an_open_key_change_holds_its_keys() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let a = start(&f, 0, rotate(txid), 0).unwrap();
    let first = a.session.key_change.unwrap().requested.operator_public_key;
    f.credit(0, 10, COIN);
    let given = KeyChangeRequest {
        operator: KeepOr::Set(OperatorChoice::Given {
            public_key: first.clone(),
        }),
        ..rotate(txid)
    };
    let e = start(&f, 0, given, 0).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::ProtxRefused { reason, operation }
            if reason == "protx-dup" && operation.as_deref() == Some(a.session.id.as_str())),
        "{e:?}"
    );
    let b = start(&f, 0, rotate(txid), 0).unwrap();
    assert_ne!(
        b.session.key_change.unwrap().requested.operator_public_key,
        first
    );
}

/// A revoked operator key (the null key Core's `ResetOperatorFields`
/// leaves) moves the list off every base that held a key: a request made
/// before reads `stale`; one keeping the null key fails Core's rules, and
/// one setting an operator key starts.
#[test]
fn test_QT_127_a_revoked_operator_key_leaves_the_base() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let voting = KeyChangeRequest {
        pro_tx_hash: txid.to_string(),
        operator: KeepOr::Keep,
        voting: KeepOr::Set(VotingChoice::NextUnused),
    };
    let a = start(&f, 0, voting.clone(), 0).unwrap();
    let id = a.session.id.clone();
    f.imported(1, &a.message.text);
    list_keys(&f, txid, [0; 48], [1; 20]);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Stale);
    assert_message(approve(&f, 1, &id, 0).unwrap_err(), "stale");
    f.credit(0, 10, COIN);
    let e = start(&f, 0, voting, 0).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-key-null"),
        "{e:?}"
    );
    let b = start(&f, 0, rotate(txid), 0).unwrap();
    assert_eq!(b.session.stage, SharedStage::Signing);
    assert_eq!(
        b.session.base_keys.unwrap().operator_public_key,
        "00".repeat(48)
    );
}

/// D23 the other way (D105): a key change's start, approval and send wait
/// for a pending reward change or dissolution of the masternode.
#[test]
fn test_QT_127_a_key_change_waits_for_a_pending_operation() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let op = dissolve_now(&f, txid, 0).unwrap();
    assert_update_pending(start(&f, 0, rotate(txid), 0).unwrap_err(), &op.id);
    assert!(f.locked(0).is_empty());

    // An approval waits for its wallet's operation.
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let a = start(&f, 0, rotate(txid), 0).unwrap();
    f.imported(1, &a.message.text);
    let op = dissolve_now(&f, txid, 1).unwrap();
    assert_update_pending(approve(&f, 1, &a.session.id, 0).unwrap_err(), &op.id);

    // A send waits for the funder's: a key change not yet out holds nothing,
    // so the dissolution goes first.
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let op = dissolve_now(&f, txid, 0).unwrap();
    assert_update_pending(send(&f, 0, &id, COIN).unwrap_err(), &op.id);
    assert_eq!(f.net.sent().len(), 1);
}

/// What `start_key_change` refuses before signing or locking: a request
/// that changes nothing; a voting address that isn't P2PKH (dash-qt's
/// rule); Core's rules (a voting key that is a share's payee, an operator
/// key another masternode uses).
#[test]
fn test_QT_127_start_key_change_refusals() {
    let f = fx(3, true);
    let (reg_id, txid) = keyed(&f);
    let keep = KeyChangeRequest {
        pro_tx_hash: txid.to_string(),
        operator: KeepOr::Keep,
        voting: KeepOr::Keep,
    };
    assert_eq!(
        code(start(&f, 0, keep.clone(), 0).unwrap_err()),
        "invalid_argument"
    );
    let network = dashcore::Network::Regtest;
    let voting = |address: String| KeyChangeRequest {
        voting: KeepOr::Set(VotingChoice::Address { address }),
        ..keep.clone()
    };
    let p2sh = dashcore::Address::new(
        network,
        dashcore::address::Payload::ScriptHash(dashcore::ScriptHash::from_byte_array([7; 20])),
    );
    let e = start(&f, 0, voting(p2sh.to_string()), 0).unwrap_err();
    assert!(
        matches!(e.engine(), Some(crate::EngineError::InvalidArgument(m)) if m.contains("P2PKH")),
        "{e:?}"
    );
    // Share 1's owner key as the voting key: MG-04 D52.
    let p = registration(&f, &reg_id);
    let owner = super::super::standby::p2pkh_address(p.shares[1].owner_key_id, network);
    assert_eq!(
        code(start(&f, 0, voting(owner), 0).unwrap_err()),
        "invalid_argument"
    );
    // Share 1's refund address: Core's payee rule.
    let refund = super::super::dissolve::address_of(&p.shares[1].refund_script, network).unwrap();
    let e = start(&f, 0, voting(refund), 0).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-proupsharedreg-payee-reuse"),
        "{e:?}"
    );
    // Another masternode's operator key.
    let mut entries = f.seam.list.lock().unwrap().clone().unwrap();
    let mut other = entries[0].clone();
    other.pro_tx_hash = [9; 32];
    entries.push(other);
    *f.seam.list.lock().unwrap() = Some(entries);
    let mut entries = f.seam.list.lock().unwrap().clone().unwrap();
    let a = start(&f, 0, rotate(txid), 0).unwrap();
    let requested = a.session.key_change.unwrap().requested.operator_public_key;
    f.run(f.mn().discard_shared_session(f.wallets[0], a.session.id))
        .unwrap();
    entries[1].operator_public_key = hex::decode(&requested).unwrap().try_into().unwrap();
    *f.seam.list.lock().unwrap() = Some(entries);
    // The keychain's next unused key skips it; given, Core's rule refuses it.
    let given = KeyChangeRequest {
        operator: KeepOr::Set(OperatorChoice::Given {
            public_key: requested,
        }),
        ..keep.clone()
    };
    let e = start(&f, 0, given, 0).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::ProtxRefused { reason, .. } if reason == "bad-protx-dup-key"),
        "{e:?}"
    );
    assert!(f.locked(0).is_empty());
    assert!(f.net.sent().is_empty());
}

/// What an imported `registrar` request must be (D101, D104): a type-12
/// transaction of this masternode with `inputsHash` over its inputs; the
/// funder's scriptSigs and sequences aren't read. A dissolve envelope
/// holding it is no dissolution.
#[test]
fn test_QT_127_an_imported_key_change_is_checked() {
    let f = fx(3, true);
    let (reg_id, txid) = keyed(&f);
    let p = registration(&f, &reg_id);
    let mn = shared_mn(&p, txid);
    let keys = key_change::Keys {
        operator_public_key: listed_operator(),
        voting_key_id: [4; 20],
    };
    let inputs = [OutPoint::new(Txid::from_byte_array([5; 32]), 1)];
    let request = key_change::prepare(&mn, keys, &inputs, None);
    let text = |tx: &SpecialTx, kind| {
        SigsEnvelope {
            network: dashcore::Network::Regtest,
            kind,
            pro_tx_hash: txid,
            tx: tx.to_bytes(),
            signatures: Vec::new(),
        }
        .to_text()
    };
    let mut bad = request.clone();
    let mut payload: ProUpSharedRegTx = dashcore::consensus::deserialize(&bad.payload).unwrap();
    payload.inputs_hash[0] ^= 1;
    bad.payload = payload_bytes(&payload);
    assert_message(
        import_into(&f, 1, &text(&bad, SigsKind::Registrar)).unwrap_err(),
        "inputs_hash",
    );
    let typed = SpecialTx {
        tx_type: 11,
        ..request.clone()
    };
    assert_message(
        import_into(&f, 1, &text(&typed, SigsKind::Registrar)).unwrap_err(),
        "not_key_change",
    );
    assert_message(
        import_into(&f, 1, &text(&request, SigsKind::Dissolve)).unwrap_err(),
        "not_dissolution",
    );
    // dash-qt's preparer: its fee input signed at prepare, a non-final
    // sequence.
    let mut theirs = request.clone();
    theirs.input[0].sequence = 0xffff_fffe;
    theirs.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x51]);
    let (s, changed) = f.imported(1, &text(&theirs, SigsKind::Registrar));
    assert!(changed);
    assert_eq!(
        (s.kind, s.stage),
        (SharedKind::KeyChange, SharedStage::Signing)
    );
    assert_eq!(s.base_keys.unwrap().operator_public_key, LISTED_OPERATOR);
    assert!(!s.key_change.unwrap().funder);
    approve(&f, 1, &s.id, 0).unwrap();
}

/// A restart keeps the funder's locks and its session; nothing is chosen
/// again.
#[test]
fn test_QT_127_a_key_change_survives_a_restart() {
    let mut f = fx(3, true);
    let (_, txid) = keyed(&f);
    let a = start(&f, 0, rotate(txid), 0).unwrap();
    let id = a.session.id.clone();
    let reserved = reserved_of(&f.row(0, &id));
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    let list = f.seam.list.lock().unwrap().clone();
    f.restart();
    *f.seam.shared_mined.lock().unwrap() = mined;
    *f.seam.list.lock().unwrap() = list;
    let s = f.session_of(0, &id);
    assert_eq!((s.stage, s.fee), (SharedStage::Signing, a.session.fee));
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));
    assert_eq!(f.message(0, &id), a.message.text);
}
