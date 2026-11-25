//! Change keys together (MG-04b §3.3, D25, D99–D105, slice 2d).

use dashcore::hashes::sha256;
use dpp::secp256k1::{Message as Digest, ecdsa};
use dw_protx::shared::envelope::{SigsEnvelope, SigsKind, decode_message};
use dw_protx::shared::key_change;
use dw_protx::shared::rules::{SharedMn, check_update_shared_registrar};
use dw_protx::shared::{
    ProRegTxV3, ProUpShareTx, ProUpSharedRegTx, SpecialTx, TRANSACTION_PROVIDER_UPDATE_SHARE,
    TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR, payload_bytes,
};

use super::super::store::reserved_of;
use super::dissolve::{assert_update_pending, delisted, dissolve_now, listed, listed_with_reward};
use super::together::scanned_to;
use super::*;
use crate::masternodes::{
    DeadReason, KeepOr, KeyChangeRequest, MaintenanceSent, OperatorChoice, RewardChoice,
    SharedKind, SharedWarning, VotingChoice,
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
        service_addresses: vec!["1.2.3.4:19999".parse().unwrap()],
        operator_key_is_legacy: false,
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
    complete_with(f, rotate(txid))
}

fn complete_with(f: &Fx, request: KeyChangeRequest) -> (String, SharedOutcome) {
    let a = start(f, 0, request, 0).unwrap();
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
            .and_then(|s| ecdsa::verify(&s, Digest::from_digest(hash), &pk.inner.into()))
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
    assert!(f.session_of(0, &id).discardable);
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
    assert!(f.session_of(0, &id).discardable, "combined, not out");
    let sent = send(&f, 0, &id, COIN).unwrap();
    assert!(!sent.discardable, "out");
    assert_eq!(
        code(
            f.run(f.mn().discard_shared_session(f.wallets[0], id.clone()))
                .unwrap_err()
        ),
        "masternodes.shared_stage"
    );
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

/// MG-10 §4.4 change 1 (Opus r3 m1, Sol r3 m3): as a registration's
/// (D72), an abandoned key change is never handed off again, also once a
/// restart has dropped the abandon's pending spend.
#[test]
fn test_an_abandoned_key_change_is_never_resent() {
    let mut f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let fee = f.session_of(0, &id).fee.unwrap();
    send(&f, 0, &id, fee).unwrap();
    abandon(&f, 0, &id, COIN).unwrap();
    assert_eq!(f.net.sent().len(), 2);
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    let list = f.seam.list.lock().unwrap().clone();
    f.restart();
    *f.seam.shared_mined.lock().unwrap() = mined;
    *f.seam.list.lock().unwrap() = list;
    let e = send(&f, 0, &id, fee).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::SharedStage { stage } if stage == "abandoned"),
        "{e:?}"
    );
    assert_eq!(f.net.sent().len(), 2);
}

/// The control: a key change nobody abandoned is resent, the resend marked
/// before its hand-off and its outcome then recorded (R2).
#[test]
fn test_a_key_change_with_no_abandon_is_resent_and_marked() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let fee = f.session_of(0, &id).fee.unwrap();
    f.net.reject_next();
    send(&f, 0, &id, fee).unwrap();
    assert!(f.net.sent().is_empty());
    let row = f.row(0, &id);
    assert!(!row.resend_marked && row.last_error.is_some());
    send(&f, 0, &id, fee).unwrap();
    assert_eq!(f.net.sent().len(), 1);
    let row = f.row(0, &id);
    assert!(row.resend_marked);
    assert_eq!(row.last_error, None);
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

/// 0.1.x-bal §1: a key change picks no fee inputs, and takes no operator
/// key, while a replay runs.
#[test]
fn test_QT_127_a_replay_holds_a_key_changes_fee_inputs() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    f.replaying(0);
    assert!(super::replay_refusal(start(&f, 0, rotate(txid), 0)));
    f.checkpoint(0, 1_000);
    start(&f, 0, rotate(txid), 0).unwrap();
}

#[test]
fn ep_registered_key_change_call_sites_preserve_refusal_order() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, request) = complete(&f, txid);
    let entries = f.seam.list.lock().unwrap().clone();
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    for (guard, expected) in [
        (0, "masternodes.unsupported"),
        (1, "masternodes.list_unavailable"),
        (2, "masternodes.shared_registration_unknown"),
        (3, "masternodes.shared_too_early"),
    ] {
        *f.seam.list.lock().unwrap() = entries.clone();
        *f.seam.shared_mined.lock().unwrap() = mined.clone();
        *f.seam.list_height.lock().unwrap() = 100;
        f.db()
            .set_setting_durable(GLOBAL_SCOPE, "masternodes.any_shared_seen", Some("1"))
            .unwrap();
        match guard {
            0 => {
                f.db()
                    .set_setting_durable(GLOBAL_SCOPE, "masternodes.any_shared_seen", None)
                    .unwrap();
                *f.seam.list.lock().unwrap() = Some(Vec::new());
            }
            1 => *f.seam.list.lock().unwrap() = None,
            2 => f.seam.shared_mined.lock().unwrap().clear(),
            _ => *f.seam.list_height.lock().unwrap() = 50,
        }
        for call in 0..4 {
            let result = match call {
                0 => start(&f, 0, rotate(txid), 0).map(|_| ()),
                1 => import_into(&f, 1, &request.message.text).map(|_| ()),
                2 => approve(&f, 1, &id, 0).map(|_| ()),
                _ => send(&f, 0, &id, COIN).map(|_| ()),
            };
            let expected = if guard == 2 && call == 3 {
                "masternodes.shared_stage"
            } else {
                expected
            };
            assert_eq!(
                result.unwrap_err().code(),
                expected,
                "key change call {call}, guard {guard}"
            );
        }
    }
}

#[test]
fn ep_conflict_height_preserves_key_change_follower_release() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, request) = complete(&f, txid);
    send(&f, 0, &id, request.session.fee.unwrap()).unwrap();
    let row = f.row(0, &id);
    let tx: Transaction =
        dashcore::consensus::deserialize(row.signed_tx.as_ref().unwrap()).unwrap();
    let reserved = f.locked(0);
    assert!(!reserved.is_empty());
    let mut conflict = tx.clone();
    conflict.lock_time += 1;
    conflict.special_transaction_payload = None;
    f.mine(conflict, 60);
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Conflicted);
    f.tip(64);
    f.follow();
    assert_eq!(f.locked(0), reserved);
    f.tip(65);
    f.follow();
    assert!(f.locked(0).is_empty());
}

/// A listed masternode whose share 0 was registered with `reward`, fee
/// coins ready, as [`keyed`] leaves one whose reward is the refund.
fn keyed_reward(f: &Fx, reward: RewardChoice) -> (String, Txid) {
    let (id, txid) = listed_with_reward(f, COIN, reward);
    list_keys(f, txid, listed_operator(), [1; 20]);
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

fn p2pkh(id: [u8; 20]) -> dashcore::ScriptBuf {
    dashcore::ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array(id))
}

fn voting_of(txid: Txid, id: [u8; 20]) -> KeyChangeRequest {
    KeyChangeRequest {
        pro_tx_hash: txid.to_string(),
        operator: KeepOr::Keep,
        voting: KeepOr::Set(VotingChoice::Address {
            address: super::super::standby::p2pkh_address(id, dashcore::Network::Regtest),
        }),
    }
}

/// A confirmed ProUpShareTx setting share `index`'s reward script.
fn mined_share_update(pro_tx: Txid, index: u16, reward: dashcore::ScriptBuf) -> Transaction {
    let payload = ProUpShareTx {
        version: ProUpShareTx::CURRENT_VERSION,
        pro_tx_hash: pro_tx,
        share_index: index,
        reward_script: reward,
        inputs_hash: [0; 32],
        sig: vec![0; 65],
    };
    let special = SpecialTx {
        version: 3,
        tx_type: TRANSACTION_PROVIDER_UPDATE_SHARE,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0x91; 32]), 0),
            script_sig: dashcore::ScriptBuf::new(),
            sequence: u32::MAX,
            witness: Default::default(),
        }],
        output: Vec::new(),
        lock_time: 7,
        payload: payload_bytes(&payload),
    };
    super::super::send::typed(&special).expect("a ProUpShareTx round-trips")
}

fn assert_payee_reuse<T: std::fmt::Debug>(result: &Result<T, MasternodesError>) {
    assert!(
        matches!(
            result,
            Err(MasternodesError::ProtxRefused { reason, .. })
                if reason == "bad-proupsharedreg-payee-reuse"
        ),
        "{result:?}"
    );
}

/// A listed masternode whose share 0 was registered paying `id`.
fn keyed_paying(f: &Fx, id: [u8; 20]) -> Txid {
    let address = super::super::standby::p2pkh_address(id, dashcore::Network::Regtest);
    let (reg_id, txid) = keyed_reward(f, RewardChoice::Address { address });
    assert_eq!(registration(f, &reg_id).shares[0].reward_script, p2pkh(id));
    txid
}

/// D29 residual: the payee-reuse pre-check refuses every reward script the
/// wallet saw a share hold: the one a later ProUpShareTx wrote and the
/// registration's it replaced.
#[test]
fn test_QT_127_a_key_change_refuses_every_seen_reward_script() {
    let f = fx(3, true);
    let old_id = [0x44; 20];
    let new_id = [0x55; 20];
    let txid = keyed_paying(&f, old_id);
    f.mine(mined_share_update(txid, 0, p2pkh(new_id)), 60);

    assert_payee_reuse(&start(&f, 0, voting_of(txid, new_id), 0));
    assert_payee_reuse(&start(&f, 0, voting_of(txid, old_id), 0));
    assert!(f.locked(0).is_empty());

    let other = start(&f, 0, voting_of(txid, [0x66; 20]), 0).unwrap();
    assert_eq!(other.session.stage, SharedStage::Signing);
}

/// A → B → A: both scripts the share held are refused, whichever it holds
/// now.
#[test]
fn test_QT_127_a_key_change_refuses_a_script_a_share_left_and_took_back() {
    let f = fx(3, true);
    let a = [0x44; 20];
    let b = [0x55; 20];
    let txid = keyed_paying(&f, a);
    f.mine(mined_share_update(txid, 0, p2pkh(b)), 60);
    f.mine(mined_share_update(txid, 0, p2pkh(a)), 70);

    assert_payee_reuse(&start(&f, 0, voting_of(txid, a), 0));
    assert_payee_reuse(&start(&f, 0, voting_of(txid, b), 0));
    assert!(f.locked(0).is_empty());
}

/// Two updates of one share in one block: both scripts are refused,
/// whichever txid sorts first.
#[test]
fn test_QT_127_a_key_change_refuses_both_same_block_updates() {
    let f = fx(3, true);
    let txid = keyed_paying(&f, [0x44; 20]);
    let (x, y) = ([0x55; 20], [0x66; 20]);
    let (ux, uy) = (
        mined_share_update(txid, 0, p2pkh(x)),
        mined_share_update(txid, 0, p2pkh(y)),
    );
    assert_ne!(ux.txid(), uy.txid());
    f.mine(ux, 60);
    f.mine(uy, 60);

    assert_payee_reuse(&start(&f, 0, voting_of(txid, x), 0));
    assert_payee_reuse(&start(&f, 0, voting_of(txid, y), 0));
    assert!(f.locked(0).is_empty());
}

/// A resend checks the seen reward scripts too: a confirmed ProUpShareTx
/// paying the requested voting key since the send refuses it, the saved
/// bytes not handed off and the session as it was.
#[test]
fn test_QT_127_a_resend_refuses_a_voting_key_a_share_now_pays() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let voting = [0x77; 20];
    let (id, _) = complete_with(&f, voting_of(txid, voting));
    let fee = f.session_of(0, &id).fee.unwrap();
    send(&f, 0, &id, fee).unwrap();
    let before = f.row(0, &id);
    f.mine(mined_share_update(txid, 0, p2pkh(voting)), 60);

    assert_payee_reuse(&send(&f, 0, &id, fee));
    assert_eq!(f.net.sent().len(), 1);
    let after = f.row(0, &id);
    assert_eq!(
        (&after.signed_tx, &after.txid, after.out_at),
        (&before.signed_tx, &before.txid, before.out_at)
    );
    assert_eq!(f.session_of(0, &id).stage, SharedStage::Sent);
}

/// A ChainLocked copy of the funder's key change ranks before the sent
/// txid in a block without a ChainLock (`copy_rank`): its height is the
/// stage's.
#[test]
fn test_QT_127_a_chainlocked_copy_ranks_before_an_unlocked_original() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let fee = f.session_of(0, &id).fee.unwrap();
    send(&f, 0, &id, fee).unwrap();
    let original = f.net.sent().pop().unwrap();
    let mut copy = original.clone();
    let mut script = copy.input[0].script_sig.as_bytes().to_vec();
    script.push(0);
    copy.input[0].script_sig = dashcore::ScriptBuf::from_bytes(script);
    assert_ne!(copy.txid(), original.txid());
    f.mine(original, 101);
    f.mine_locked(copy, 105);
    assert_eq!(
        f.session_of(0, &id).stage,
        SharedStage::Confirmed { height: 105 }
    );
}

/// D103 residual: a scriptSig-malleated copy of the funder's key change,
/// mined instead of the txid it sent, reads `confirmed`. The locks are
/// released once it is buried, not held as a `conflicted` spend.
#[test]
fn test_QT_127_a_malleated_key_change_confirms_by_payload() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let fee = f.session_of(0, &id).fee.unwrap();
    send(&f, 0, &id, fee).unwrap();
    let reserved = reserved_of(&f.row(0, &id));
    let mut malleated = f.net.sent().pop().unwrap();
    let original = malleated.txid();
    let mut script = malleated.input[0].script_sig.as_bytes().to_vec();
    script.push(0);
    malleated.input[0].script_sig = dashcore::ScriptBuf::from_bytes(script);
    assert_ne!(malleated.txid(), original);
    f.mine(malleated, 101);
    assert_eq!(
        f.session_of(0, &id).stage,
        SharedStage::Confirmed { height: 101 }
    );
    f.tip(200);
    f.follow();
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
    f.follow();
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
}

/// A funder's key change left at `signing` surfaces an abandon hint after
/// the overdue quiet period. An approver's session does not, and the fee
/// inputs stay locked.
#[test]
fn test_QT_127_a_quiet_key_change_hints_abandon() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let started = start(&f, 0, rotate(txid), 0).unwrap();
    let id = started.session.id;
    let reserved = reserved_of(&f.row(0, &id));
    let fresh = serde_json::to_string(&f.session_of(0, &id).warnings).unwrap();
    assert!(!fresh.contains("abandon_hint"), "{fresh}");
    let quiet =
        crate::coins::now_secs().saturating_sub(crate::masternodes::protx::ops::OVERDUE_SECS);
    f.rewrite(0, &id, |u| u.updated_at = quiet);
    let hinted = serde_json::to_string(&f.session_of(0, &id).warnings).unwrap();
    assert!(hinted.contains("\"abandon_hint\""), "{hinted}");
    let session = f.session_of(0, &id);
    assert_eq!(
        (session.stage, session.next),
        (SharedStage::Signing, SharedNext::Copy)
    );
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));

    f.imported(1, &started.message.text);
    f.rewrite(1, &id, |u| u.updated_at = quiet);
    let approver = serde_json::to_string(&f.session_of(1, &id).warnings).unwrap();
    assert!(!approver.contains("abandon_hint"), "{approver}");
}

/// [`keyed`] with the funder's fee coins worth `values`.
fn keyed_with(f: &Fx, values: &[u64]) -> (String, Txid) {
    let (id, txid) = listed(f);
    list_keys(f, txid, listed_operator(), [1; 20]);
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    for (_, tx, _) in mined {
        for input in &tx.input {
            for w in 0..f.wallets.len() {
                f.spend(w, input.previous_output);
            }
        }
    }
    for (seed, value) in (60u8..).zip(values) {
        f.credit(0, seed, *value);
    }
    (id, txid)
}

/// QT-127's abandon with several fee inputs (MG-10 §4.4 change 4, the
/// same-coin bound; the named `input.len() == 1` above holds only for one
/// fee input): one spend of every reserved input, the n-input fee.
#[test]
fn test_QT_127_a_sent_key_change_with_several_fee_inputs_is_abandoned_by_one_spend_of_all() {
    // The one-input fee of this request, from a probe on a large coin: a
    // coin worth less than it can't pay it alone.
    let probe = fx(3, true);
    let (_, t) = keyed(&probe);
    let (pid, _) = complete(&probe, t);
    let one_input = probe.session_of(0, &pid).fee.unwrap();
    drop(probe);
    // One duff under the fee without its change output (34 bytes at the
    // relay minimum): a coin can't pay it alone, two can, and two are worth
    // more than their spend back's fee and dust.
    let value = one_input - 35;
    let f = fx(3, true);
    let (_, txid) = keyed_with(&f, &[value, value, value]);
    let (id, _) = complete(&f, txid);
    send(&f, 0, &id, COIN).unwrap();
    let reserved = reserved_of(&f.row(0, &id));
    assert!(
        reserved.len() >= 2,
        "{reserved:?}, one-input fee {one_input}"
    );
    abandon(&f, 0, &id, COIN).unwrap();
    let spend = f.net.sent()[1].clone();
    assert_eq!(spend.input.len(), reserved.len());
    assert!(
        spend
            .input
            .iter()
            .all(|i| reserved.contains(&i.previous_output))
    );
    let fee = super::super::recovery::spend_fee(reserved.len());
    assert_eq!(spend.output[0].value, value * reserved.len() as u64 - fee);
}

/// MG-10 §4.4 change 4 for a key change: the funder's abandon is refused
/// `abandoning` while its spend back may be out, and the read lists the
/// replay at that spend's fee; the approver's read is `discard{0}`, its
/// abandon calls refused `participant`.
#[test]
fn test_a_key_change_spend_back_that_may_be_out_is_only_sent_again() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    send(&f, 0, &id, COIN).unwrap();
    abandon(&f, 0, &id, COIN).unwrap();
    *f.seam.spv.lock().unwrap() = Some((true, 1));
    let e = abandon(&f, 0, &id, COIN).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::SharedStage { stage } if stage == "abandoning"),
        "{e:?}"
    );
    let fee = super::super::recovery::spend_fee(1);
    let r = f.session_of(0, &id).recovery;
    assert_eq!(
        r.actions,
        [crate::masternodes::RecoveryAction::ResendAbandon { charge: fee }]
    );
    assert_eq!(
        r.abandon_spend.and_then(|s| s.state),
        Some(crate::masternodes::AbandonSpendState::Uncertain)
    );
    let resend = |w: usize, max: u64| {
        f.run(
            f.mn()
                .resend_shared_abandon(f.wallets[w], id.clone(), f.grant(w, max)),
        )
    };
    assert_eq!(
        send_code(&resend(0, fee - 1).unwrap_err()),
        "send.grant_exceeded"
    );
    resend(0, fee).unwrap();
    let out = f.net.sent();
    assert_eq!(out.len(), 3);
    assert_eq!(out[1].txid(), out[2].txid());

    let approver = f.session_of(1, &id).recovery;
    assert_eq!(
        approver.actions,
        [crate::masternodes::RecoveryAction::Discard { releases: 0 }]
    );
    for e in [
        abandon(&f, 1, &id, COIN).unwrap_err(),
        resend(1, COIN).unwrap_err(),
    ] {
        assert!(
            matches!(&e, MasternodesError::SharedStage { stage } if stage == "participant"),
            "{e:?}"
        );
    }
}

/// T4 "key change before out" and "dead{abandoned}": before its send the
/// funder's read lists the release (`abandon`, charged 0, no spend) and
/// the discard; abandoned, only the discard; the replay call refuses at
/// that stage.
#[test]
fn test_a_key_change_before_out_lists_its_release() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    *f.seam.spv.lock().unwrap() = Some((true, 1));
    let releases: u64 = COIN;
    let r = f.session_of(0, &id).recovery;
    assert_eq!(
        r.actions,
        [
            crate::masternodes::RecoveryAction::Abandon {
                charge: 0,
                spends: false,
                replaces: None
            },
            crate::masternodes::RecoveryAction::Discard { releases }
        ]
    );
    let e = f
        .run(
            f.mn()
                .resend_shared_abandon(f.wallets[0], id.clone(), f.grant(0, COIN)),
        )
        .unwrap_err();
    assert_eq!(code(e), "masternodes.shared_stage");
    let s = abandon(&f, 0, &id, 0).unwrap();
    assert_eq!(
        s.recovery.actions,
        [crate::masternodes::RecoveryAction::Discard { releases: 0 }]
    );
}

/// T4 "terminal with A absent from history", a key change `dead`: a sent
/// key change whose masternode left the list, its spend back never
/// recorded, lists nothing (out: no discard); both calls refuse `dead`.
#[test]
fn test_a_dead_key_change_lists_nothing_and_refuses_both_exits() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    send(&f, 0, &id, COIN).unwrap();
    f.net.reject_next();
    abandon(&f, 0, &id, COIN).unwrap();
    delisted(&f, 200);
    scanned_to(&f, 0, 200);
    *f.seam.spv.lock().unwrap() = Some((true, 1));
    let s = f.session_of(0, &id);
    assert!(matches!(s.stage, SharedStage::Dead { .. }), "{:?}", s.stage);
    assert!(s.recovery.actions.is_empty(), "{:?}", s.recovery);
    assert_eq!(s.recovery.abandon_spend.unwrap().state, None);
    let resend = f
        .run(
            f.mn()
                .resend_shared_abandon(f.wallets[0], id.clone(), f.grant(0, COIN)),
        )
        .unwrap_err();
    for e in [resend, abandon(&f, 0, &id, COIN).unwrap_err()] {
        assert!(
            matches!(&e, MasternodesError::SharedStage { stage } if stage == "dead"),
            "{e:?}"
        );
    }
}

/// The value check through the call (MG-10 §4.4 change 2 parity): a held
/// fee input worth exactly the spend's fee plus dust is
/// `send.amount_with_fee_exceeds_balance` and reads `too_small`; one duff
/// more is listed and passes, with the call's real change address.
#[test]
fn test_QT_127_the_abandon_value_check_matches_the_read() {
    let boundary = super::super::recovery::spend_fee(1) + crate::send::plan::dust_threshold(25);
    for value in [boundary, boundary + 1] {
        let f = fx(3, true);
        let (_, txid) = keyed_with(&f, &[value]);
        let (id, _) = complete(&f, txid);
        send(&f, 0, &id, COIN).unwrap();
        // As a restart leaves it: no claim on the fee input.
        f.session.spends.forget_wallet(&f.wallets[0]);
        *f.seam.spv.lock().unwrap() = Some((true, 1));
        let r = f.session_of(0, &id).recovery;
        let listed = r
            .actions
            .iter()
            .any(|a| matches!(a, crate::masternodes::RecoveryAction::Abandon { .. }));
        match value == boundary {
            true => {
                assert!(!listed, "{r:?}");
                assert_eq!(
                    r.blocked.map(|b| b.reason),
                    Some(crate::masternodes::BlockedReason::TooSmall)
                );
                let e = abandon(&f, 0, &id, COIN).unwrap_err();
                assert_eq!(send_code(&e), "send.amount_with_fee_exceeds_balance");
            }
            false => {
                assert!(listed, "{r:?}");
                abandon(&f, 0, &id, COIN).unwrap();
                assert_eq!(
                    f.net.sent().last().unwrap().output[0].value,
                    1 + crate::send::plan::dust_threshold(25)
                );
            }
        }
    }
}

/// An unreadable key change is judged by its stored columns, as an
/// unreadable registration is: before out, a spent reserved coin releases
/// its locks.
#[test]
fn test_an_unreadable_key_change_before_out_releases_once_a_coin_is_spent() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let id = start(&f, 0, rotate(txid), 0).unwrap().session.id;
    let reserved = reserved_of(&f.row(0, &id));
    assert!(!reserved.is_empty());
    f.rewrite(0, &id, |u| u.envelope = "{".into());
    f.follow();
    assert!(
        reserved.iter().all(|o| f.locked(0).contains(o)),
        "unspent: kept"
    );
    f.spend(0, reserved[0]);
    f.follow();
    assert!(f.locked(0).is_empty());
}

/// An unreadable key change that may be out keeps its coins until a spend
/// of one of them is buried, then releases them.
#[test]
fn test_an_unreadable_out_key_change_releases_on_a_buried_spend() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let id = start(&f, 0, rotate(txid), 0).unwrap().session.id;
    let reserved = reserved_of(&f.row(0, &id));
    assert!(
        f.db()
            .mark_shared_session_out_durable(&f.wallets[0].to_string(), &id, &[7], "tx", 1)
            .unwrap()
    );
    f.rewrite(0, &id, |u| u.envelope = "{".into());
    let spend = Transaction {
        version: 3,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: reserved[0],
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: None,
    };
    f.mine(spend, 200);
    f.tip(200);
    f.follow();
    assert!(!f.locked(0).is_empty(), "one deep: kept");
    f.tip(204);
    f.follow();
    assert!(!f.locked(0).is_empty(), "five deep: kept");
    f.tip(205);
    f.follow();
    assert!(f.locked(0).is_empty());
}

/// An unreadable key change whose own stored transaction is mined waits
/// for it to be buried, as the readable path does: kept at 1 and 5 deep,
/// released at 6. Its own transaction is not taken for a conflicting spend.
#[test]
fn test_an_unreadable_key_change_mined_by_its_stored_txid_releases_once_buried() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    let fee = f.session_of(0, &id).fee.unwrap();
    send(&f, 0, &id, fee).unwrap();
    let tx = f.net.sent().pop().unwrap();
    assert_eq!(
        f.row(0, &id).txid.as_deref(),
        Some(tx.txid().to_string().as_str())
    );
    let reserved = reserved_of(&f.row(0, &id));
    f.rewrite(0, &id, |u| u.envelope = "{".into());
    f.mine(tx, 101);
    f.tip(101);
    f.follow();
    assert!(
        reserved.iter().all(|o| f.locked(0).contains(o)),
        "one deep: kept"
    );
    f.tip(105);
    f.follow();
    assert!(
        reserved.iter().all(|o| f.locked(0).contains(o)),
        "five deep: kept"
    );
    f.tip(106);
    f.follow();
    assert!(reserved.iter().all(|o| !f.locked(0).contains(o)));
}

/// A funder's unsent key change reads `discardable`, and Discard releases
/// every fee coin it locked, durably: after a restart a new key change
/// selects them again.
#[test]
fn test_a_discarded_key_change_frees_its_fee_coins() {
    let mut f = fx(3, true);
    let (_, txid) = keyed(&f);
    let a = start(&f, 0, rotate(txid), 0).unwrap();
    let id = a.session.id.clone();
    let reserved = reserved_of(&f.row(0, &id));
    assert!(!reserved.is_empty());
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));
    assert!(a.session.discardable, "started");
    assert!(f.session_of(0, &id).discardable, "unsent");
    f.run(f.mn().discard_shared_session(f.wallets[0], id.clone()))
        .unwrap();
    assert!(f.locked(0).is_empty());
    assert!(f.net.sent().is_empty());
    let b = start(&f, 0, rotate(txid), 0).unwrap();
    assert_eq!(reserved_of(&f.row(0, &b.session.id)), reserved);
    f.run(f.mn().discard_shared_session(f.wallets[0], b.session.id))
        .unwrap();
    f.restart();
    assert!(f.locked(0).is_empty(), "the unlock survives a restart");
}

/// F2: an unreadable key change whose hand-off went out has an exit.
/// `abandon_shared_session` refuses it before spending anything (its
/// record can't be returned); `unreadable_shared_sessions` lists it
/// `abandonable` with the coin test's charge, and
/// `abandon_unreadable_shared_session`, under a grant of exactly that charge
/// (one duff less is refused), spends its reserved coins back to the wallet. Its
/// locks stay until that spend is buried: kept one and five deep, released
/// six deep.
#[test]
fn test_an_unreadable_out_key_change_is_abandoned_and_released_once_buried() {
    let f = fx(3, true);
    let (_, txid) = keyed(&f);
    let (id, _) = complete(&f, txid);
    send(&f, 0, &id, COIN).unwrap();
    let reserved = reserved_of(&f.row(0, &id));
    assert!(!reserved.is_empty());
    f.rewrite(0, &id, |u| u.envelope = "{".into());
    assert!(abandon(&f, 0, &id, COIN).is_err());
    assert_eq!(f.net.sent().len(), 1, "refused before any spend");
    let listed = f
        .run(f.mn().unreadable_shared_sessions(f.wallets[0]))
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, SharedKind::KeyChange);
    assert!(!listed[0].discardable);
    assert!(listed[0].abandonable);
    let fee = listed[0].abandon_charge.expect("charged while abandonable");
    let short = f.run(f.mn().abandon_unreadable_shared_session(
        f.wallets[0],
        id.clone(),
        f.grant(0, fee - 1),
    ));
    assert_eq!(send_code(&short.unwrap_err()), "send.grant_exceeded");
    assert_eq!(f.net.sent().len(), 1);
    let after = f
        .run(
            f.mn()
                .abandon_unreadable_shared_session(f.wallets[0], id.clone(), f.grant(0, fee)),
        )
        .unwrap();
    assert_eq!(after.id, id);
    let out = f.net.sent();
    assert_eq!(out.len(), 2);
    let spend = out[1].clone();
    assert!(
        spend
            .input
            .iter()
            .all(|i| reserved.contains(&i.previous_output))
    );
    assert_eq!(fee, super::super::recovery::spend_fee(spend.input.len()));
    // The wallet's record of the spend (the test network records none).
    for i in &spend.input {
        f.spend(0, i.previous_output);
    }
    let listed = f
        .run(f.mn().unreadable_shared_sessions(f.wallets[0]))
        .unwrap();
    assert!(!listed[0].abandonable, "every reserved coin spent");
    assert_eq!(listed[0].abandon_charge, None);
    assert_eq!(spend.output.len(), 1);
    assert_eq!(
        f.row(0, &id).abandon_txid.as_deref(),
        Some(spend.txid().to_string().as_str())
    );
    let log: serde_json::Value =
        serde_json::from_str(f.row(0, &id).abandon_log.as_deref().unwrap()).unwrap();
    assert_eq!(log[0]["fee"].as_u64(), Some(fee));
    assert!(reserved.iter().all(|o| f.locked(0).contains(o)));
    f.mine(spend, 200);
    f.tip(200);
    f.follow();
    assert!(!f.locked(0).is_empty(), "one deep: kept");
    assert_eq!(
        code(
            f.run(f.mn().abandon_unreadable_shared_session(
                f.wallets[0],
                id.clone(),
                f.grant(0, fee)
            ))
            .unwrap_err()
        ),
        "masternodes.shared_stage"
    );
    f.tip(204);
    f.follow();
    assert!(!f.locked(0).is_empty(), "five deep: kept");
    f.tip(205);
    f.follow();
    assert!(f.locked(0).is_empty());
}
