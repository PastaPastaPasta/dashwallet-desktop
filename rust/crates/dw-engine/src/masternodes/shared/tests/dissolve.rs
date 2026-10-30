//! Dissolve now (MG-04b §5, §6.3, slice 2a) on [`confirmed`]'s
//! three-party masternode: A (wallet 0) holds share 0 (400), B and C 300
//! each; registered at 50, early period 1,000 blocks, penalty 1 DASH.

use dw_protx::shared::rules::{SharedMn, check_dissolution};
use dw_protx::shared::{ProDisTx, SpecialTx, TRANSACTION_PROVIDER_DISSOLVE};

use super::*;
use crate::masternodes::{
    DissolveRequest, ProTxOperation, ProTxOperationKind, ProTxOperationState, SharedWarning,
};

/// [`confirmed`]'s masternode in the synced list at 100, its registration
/// in the history with its payload (the vault's dissolution signer reads
/// the share table from it, D29).
pub(super) fn listed(f: &Fx) -> (String, Txid) {
    listed_with(f, COIN)
}

/// [`listed`] with the terms' early penalty `early_penalty`.
pub(super) fn listed_with(f: &Fx, early_penalty: u64) -> (String, Txid) {
    let (id, txid) = confirmed_with(f, 50, early_penalty);
    f.follow();
    let entry = platform_wallet::masternode::MasternodeListSummary {
        pro_tx_hash: txid.to_byte_array(),
        service_address: Some("1.2.3.4:19999".parse().unwrap()),
        platform_http_port: None,
        operator_public_key: [1; 48],
        voting_key_id: [1; 20],
        platform_node_id: None,
        is_valid: true,
        is_evonode: false,
        has_extended_net_info: true,
        net_info: None,
    };
    *f.seam.list.lock().unwrap() = Some(vec![entry]);
    let registration = decode_frozen(&parse(&f.message(0, &id))).unwrap().tx;
    let typed = super::super::send::typed(&registration).unwrap();
    assert_eq!(typed.txid(), txid);
    let mut mined = f.seam.shared_mined.lock().unwrap();
    mined.retain(|(t, _, _)| *t != txid);
    mined.push((txid, typed, 50));
    (id, txid)
}

/// `tx` in a block at `height` of wallet `w`'s history, as key-wallet
/// records it (the seam's blocks reach the session stages only).
fn confirm(f: &Fx, w: usize, tx: Transaction, height: u32) {
    use key_wallet::account::{AccountType, StandardAccountType};
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};
    let context =
        TransactionContext::InBlock(BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 0));
    let record = TransactionRecord::new(
        tx,
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        context,
        TransactionType::Standard,
        TransactionDirection::Incoming,
        Vec::new(),
        Vec::new(),
        0,
    );
    (f.session.hub.history).with_wallet(f.wallets[w], |h| h.upsert(&record, None));
}

fn request(txid: Txid, share: u16, fee: u64) -> DissolveRequest {
    DissolveRequest {
        pro_tx_hash: txid.to_string(),
        share_index: share,
        fee,
    }
}

/// Wallet `w` dissolves its share `w` now, at fee 1,000 under a 2-DASH
/// grant.
pub(super) fn dissolve_now(
    f: &Fx,
    txid: Txid,
    w: usize,
) -> Result<ProTxOperation, MasternodesError> {
    let share = u16::try_from(w).unwrap();
    f.run(f.mn().dissolve(
        f.wallets[w],
        request(txid, share, 1_000),
        f.grant(w, 2 * COIN),
    ))
}

pub(super) fn assert_update_pending(e: MasternodesError, operation: &str) {
    assert!(
        matches!(&e, MasternodesError::UpdatePending { operation: o } if o == operation),
        "{e:?}"
    );
}

fn script(address: &str) -> dashcore::ScriptBuf {
    address
        .parse::<dashcore::Address<_>>()
        .unwrap()
        .assume_checked()
        .script_pubkey()
}

#[test]
fn test_QT_127_preview_dissolve_is_cores_builder_output() {
    let f = fx(3, true);
    let (id, txid) = listed(&f);
    let preview = |share: u16, fee: u64| {
        f.run(
            f.mn()
                .preview_dissolve(f.wallets[0], request(txid, share, fee)),
        )
    };
    let rows = f.session_of(0, &id).rows;
    // Mined at 101, 51 blocks after the registration: early, so the
    // penalty is paid, split 300:300 over B and C.
    let p = preview(0, 1_000).unwrap();
    assert!(p.early);
    assert_eq!(
        (p.penalty, p.fee, p.total_debit),
        (COIN, 1_000, COIN + 1_000)
    );
    assert_eq!(p.valid_from_height, 1_050);
    assert!(p.warnings.is_empty());
    let got: Vec<_> = p
        .returns
        .iter()
        .map(|r| (r.share_index, r.amount, r.refund_address.clone(), r.mine))
        .collect();
    assert_eq!(
        got,
        [
            (0, 399 * COIN - 1_000, rows[0].refund_address.clone(), true),
            (
                1,
                300 * COIN + COIN / 2,
                rows[1].refund_address.clone(),
                false
            ),
            (
                2,
                300 * COIN + COIN / 2,
                rows[2].refund_address.clone(),
                false
            ),
        ]
    );

    // From registered + period on, no penalty: mined at 1,050.
    *f.seam.list_height.lock().unwrap() = 1_049;
    let p = preview(0, 1_000).unwrap();
    assert_eq!((p.early, p.penalty, p.total_debit), (false, 0, 1_000));
    assert_eq!(p.returns[0].amount, 400 * COIN - 1_000);
    assert_eq!(p.returns[1].amount, 300 * COIN);
    *f.seam.list_height.lock().unwrap() = 1_048;
    assert_eq!(preview(0, 1_000).unwrap().penalty, COIN);

    // The fee: Core's builder's refusals, then the relay minimum.
    *f.seam.list_height.lock().unwrap() = 100;
    let invalid = |e: MasternodesError| match e.engine() {
        Some(crate::EngineError::InvalidArgument(m)) => m.clone(),
        _ => panic!("not invalid_argument: {e:?}"),
    };
    assert_eq!(invalid(preview(0, 0).unwrap_err()), "fee must be positive");
    assert_eq!(
        invalid(preview(0, 1_000_001).unwrap_err()),
        "fee exceeds the consensus ceiling of 1000000 duffs"
    );
    assert!(preview(0, 1_000_000).is_ok());
    let low = invalid(preview(0, 100).unwrap_err());
    assert!(
        low.starts_with("the fee is below the relay minimum of"),
        "{low}"
    );
    // Arguments come before the wallet (§7).
    let stranger = WalletId([9; 32]);
    let e = f
        .run(f.mn().preview_dissolve(stranger, request(txid, 0, 0)))
        .unwrap_err();
    assert_eq!(invalid(e), "fee must be positive");

    assert_eq!(
        code(preview(1, 1_000).unwrap_err()),
        "masternodes.share_not_mine"
    );
    *f.seam.list_height.lock().unwrap() = 50;
    assert_eq!(
        code(preview(0, 1_000).unwrap_err()),
        "masternodes.shared_too_early"
    );
    *f.seam.list_height.lock().unwrap() = 100;
    let entries = f.seam.list.lock().unwrap().replace(Vec::new());
    assert_eq!(
        code(preview(0, 1_000).unwrap_err()),
        "masternodes.shared_too_early"
    );
    *f.seam.list.lock().unwrap() = entries;
    let e = f
        .run(f.mn().preview_dissolve(
            f.wallets[0],
            DissolveRequest {
                pro_tx_hash: "cd".repeat(32),
                share_index: 0,
                fee: 1_000,
            },
        ))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.shared_registration_unknown");
}

#[test]
fn test_QT_127_dissolve_sends_an_operation() {
    let f = fx(3, true);
    let (id, txid) = listed(&f);
    let rows = f.session_of(0, &id).rows;
    let req = request(txid, 0, 1_000);
    let dissolve = |max: u64| {
        let grant = f.grant(0, max);
        f.run(f.mn().dissolve(f.wallets[0], req.clone(), grant))
    };
    // Charged what the share gives up: the penalty and the fee (D18).
    assert_eq!(
        send_code(&dissolve(COIN + 999).unwrap_err()),
        "send.grant_exceeded"
    );
    assert!(f.net.sent().is_empty());
    let off = f.run(
        f.sending_off()
            .dissolve(f.wallets[0], req.clone(), f.grant(0, 2 * COIN)),
    );
    assert!(
        matches!(off, Err(MasternodesError::Unsupported { .. })),
        "{off:?}"
    );

    let op = dissolve(COIN + 1_000).unwrap();
    assert_eq!(op.kind, ProTxOperationKind::Dissolve);
    assert_eq!(op.state, ProTxOperationState::Sent);
    assert_eq!(op.fee, 1_000);
    assert_eq!(op.pro_tx_hash.as_deref(), Some(txid.to_string().as_str()));
    let sent = f.net.sent();
    assert_eq!(sent.len(), 1);
    let tx = &sent[0];
    let special = SpecialTx::from_bytes(&dashcore::consensus::serialize(tx)).unwrap();
    assert_eq!(special.tx_type, TRANSACTION_PROVIDER_DISSOLVE);
    // Core's builder output: the collateral alone, then B, C, and A last.
    let frozen = decode_frozen(&parse(&f.message(0, &id))).unwrap().payload;
    let collateral = OutPoint::new(txid, frozen.collateral.vout);
    assert_eq!(tx.input.len(), 1);
    assert_eq!(
        (
            tx.input[0].previous_output,
            tx.input[0].sequence,
            tx.lock_time
        ),
        (collateral, u32::MAX, 0)
    );
    assert!(tx.input[0].script_sig.is_empty());
    let refund = |i: usize| script(rows[i].refund_address.as_deref().unwrap());
    assert_eq!(
        tx.output,
        [
            TxOut {
                value: 300 * COIN + COIN / 2,
                script_pubkey: refund(1)
            },
            TxOut {
                value: 300 * COIN + COIN / 2,
                script_pubkey: refund(2)
            },
            TxOut {
                value: 399 * COIN - 1_000,
                script_pubkey: refund(0)
            },
        ]
    );
    // One signature, by share 0's owner key over `MakeSignHash(tx, 1)`,
    // and Core's rules pass it at the next height.
    let p: ProDisTx = dashcore::consensus::deserialize(&special.payload).unwrap();
    assert_eq!((p.pro_tx_hash, p.actor_index, p.sigs.len()), (txid, 0, 1));
    let owner = rows[0].owner_address.clone().unwrap();
    let key_id =
        super::super::super::protx::keys::p2pkh_id(&owner, dashcore::Network::Regtest).unwrap();
    assert_eq!(key_id, frozen.shares[0].owner_key_id);
    let hash = p.sign_hash(
        special.version,
        special.lock_time,
        &special.prevouts(),
        &special.output,
        1,
    );
    assert!(dw_protx::shared::rules::verify_hash_canonical(
        hash, key_id, &p.sigs[0]
    ));
    let mn = SharedMn {
        pro_tx_hash: txid,
        collateral,
        shares: &frozen.shares,
        registered_height: 50,
        early_period_blocks: frozen.early_period_blocks,
        early_penalty: frozen.early_penalty,
        voting_key_id: [1; 20],
    };
    check_dissolution(&special, &p, Some(&mn), 101, true, true).unwrap();
    // No coin of the wallet is held for it (§6.3: no fee inputs).
    assert!(f.locked(0).is_empty());

    // D23: one at a time per masternode, a reward change included.
    assert_eq!(
        code(dissolve(2 * COIN).unwrap_err()),
        "masternodes.update_pending"
    );
    f.credit(0, 9, COIN);
    let update = f.run(f.mn().update_share(
        f.wallets[0],
        UpdateShareRequest {
            pro_tx_hash: txid.to_string(),
            share_index: 0,
            reward: RewardChoice::Refund,
        },
        f.grant(0, COIN),
    ));
    assert_update_pending(update.unwrap_err(), &op.id);

    // The resend: shared sending, charged the penalty and the fee again.
    let refused = f.run(f.sending_off().resend_operation(
        f.wallets[0],
        op.id.clone(),
        f.grant(0, 2 * COIN),
    ));
    assert!(
        matches!(refused, Err(MasternodesError::Unsupported { .. })),
        "{refused:?}"
    );
    let short = f.run(
        f.mn()
            .resend_operation(f.wallets[0], op.id.clone(), f.grant(0, COIN + 999)),
    );
    assert_eq!(send_code(&short.unwrap_err()), "send.grant_exceeded");
    f.run(
        f.mn()
            .resend_operation(f.wallets[0], op.id.clone(), f.grant(0, COIN + 1_000)),
    )
    .unwrap();
    let sent = f.net.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].txid(), tx.txid());

    // Mined: confirmed, and the masternode's next maintenance waits on
    // nothing (it leaves the list).
    confirm(&f, 0, tx.clone(), 101);
    let ops = f.run(f.mn().operations(f.wallets[0])).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].state, ProTxOperationState::Confirmed { height: 101 });
}

/// D27: another party's dissolution of the masternode pays this wallet's
/// refund, so the history holds it: this wallet's own reads `conflicted`.
#[test]
fn test_QT_127_another_dissolution_mined_conflicts_ours() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let op = dissolve_now(&f, txid, 0).unwrap();
    let ours = f.net.sent()[0].clone();
    let mut theirs = ours.clone();
    theirs.output.swap(0, 1);
    confirm(&f, 0, theirs, 101);
    let ops = f.run(f.mn().operations(f.wallets[0])).unwrap();
    assert_eq!(
        (ops[0].id.as_str(), ops[0].state),
        (op.id.as_str(), ProTxOperationState::Conflicted)
    );
}

/// D11: a recorded owner key index that isn't the actor share's signs
/// nothing the network would take: the vault refuses, nothing is saved
/// or sent.
#[test]
fn test_QT_127_dissolve_refuses_a_key_that_isnt_the_actors() {
    let f = fx(3, true);
    let (id, txid) = listed(&f);
    f.rewrite(0, &id, |u| {
        let mut c: serde_json::Value =
            serde_json::from_str(u.contribution.as_deref().unwrap()).unwrap();
        c["owner_index"] = (c["owner_index"].as_u64().unwrap() + 1).into();
        u.contribution = Some(c.to_string());
    });
    let e = dissolve_now(&f, txid, 0).unwrap_err();
    assert!(matches!(e, MasternodesError::ShareNotMine), "{e:?}");
    assert!(f.net.sent().is_empty());
    assert!(f.run(f.mn().operations(f.wallets[0])).unwrap().is_empty());
}

/// D23 the other way: a sent reward change of the masternode holds its
/// dissolution until it is mined or dropped.
#[test]
fn test_QT_127_a_sent_update_share_holds_dissolve() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    f.credit(0, 9, COIN);
    let update = f
        .run(f.mn().update_share(
            f.wallets[0],
            UpdateShareRequest {
                pro_tx_hash: txid.to_string(),
                share_index: 0,
                reward: RewardChoice::NewAddress,
            },
            f.grant(0, COIN),
        ))
        .unwrap();
    assert_eq!(update.kind, ProTxOperationKind::UpdateShare);
    let e = dissolve_now(&f, txid, 0).unwrap_err();
    assert_update_pending(e, &update.id);
    assert_eq!(f.net.sent().len(), 1);
}

/// [`unseen_copy`]'s row.
const UNSEEN: &str = "unseen";

/// A copy, as row [`UNSEEN`], of `op`'s sent row whose record says its
/// transaction pays this wallet nothing (the fixture's penalty can't
/// consume a share).
fn unseen_copy(f: &Fx, op: &str) {
    let w = f.wallets[0].to_string();
    let db = f.db();
    let row = db.masternode_op(&w, op).unwrap().unwrap();
    let mut record: serde_json::Value = serde_json::from_str(&row.request).unwrap();
    record["actor_output"] = 0.into();
    let unseen = dw_appdb::MasternodeOpRow {
        id: UNSEEN.into(),
        request: record.to_string(),
        sent_at: None,
        ..row
    };
    db.insert_masternode_op_durable(&w, &unseen, &[], 0)
        .unwrap();
    db.begin_masternode_op_attempt(&w, UNSEEN, 1).unwrap();
}

/// Wallet 0's filter-scan checkpoint and best processed block.
fn scanned(f: &Fx, synced: u32, processed: u32) {
    let mut state = f
        .session
        .hub
        .wallet_state(&f.wallets[0])
        .unwrap_or_default();
    (state.synced_height, state.processed_height) = (synced, processed);
    f.session.hub.set_wallet_state(f.wallets[0], state);
}

/// Wallet 0's operation `id`'s state.
fn op_state_of(f: &Fx, id: &str) -> ProTxOperationState {
    let ops = f.run(f.mn().operations(f.wallets[0])).unwrap();
    ops.iter().find(|o| o.id == id).unwrap().state
}

/// The masternode out of the synced list, which reads `height`.
fn delisted(f: &Fx, height: u32) {
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    *f.seam.list_height.lock().unwrap() = height;
}

/// D81 in the engine: a `sent` dissolve paying this wallet nothing reads
/// `confirmed` by the list, at its height, once the masternode has left it,
/// the wallet's filter scan has reached that height, and the registration
/// is still in a block.
#[test]
fn test_QT_127_an_unseen_dissolution_confirms_by_the_list_after_the_scan() {
    use ProTxOperationState::{Confirmed, Sent};
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let op = dissolve_now(&f, txid, 0).unwrap();
    unseen_copy(&f, &op.id);
    scanned(&f, 100, 100);
    assert_eq!(op_state_of(&f, UNSEEN), Sent, "still listed");
    delisted(&f, 105);
    // The best processed block isn't a scan checkpoint: filter batches
    // commit with gaps (review r1 m-1).
    scanned(&f, 104, 105);
    assert_eq!(op_state_of(&f, UNSEEN), Sent, "the scan is behind the list");
    // The list's reading may lag its entries: the headers bound it.
    f.tip(106);
    scanned(&f, 105, 106);
    assert_eq!(
        op_state_of(&f, UNSEEN),
        Sent,
        "the scan is behind the headers"
    );
    scanned(&f, 106, 106);
    assert_eq!(op_state_of(&f, UNSEEN), Confirmed { height: 106 });
    // The row paying this wallet waits for its history.
    assert_eq!(op_state_of(&f, &op.id), Sent);
    // A reorg takes the registration out of its block: nothing was
    // dissolved, so the row is `sent` again (review r1 n-1).
    f.seam
        .shared_mined
        .lock()
        .unwrap()
        .retain(|(t, _, _)| *t != txid);
    assert_eq!(op_state_of(&f, UNSEEN), Sent, "the registration is out");
}

/// D81, review r1 m-1: another participant's dissolution empties the list
/// before this wallet's scan reaches its block. The row stays `sent`, then
/// reads `conflicted` once the scan finds that dissolution, which pays
/// this wallet's refund.
#[test]
fn test_QT_127_anothers_dissolution_ahead_of_the_scan_stays_sent_then_conflicts() {
    use ProTxOperationState::{Conflicted, Sent};
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let op = dissolve_now(&f, txid, 0).unwrap();
    unseen_copy(&f, &op.id);
    dissolve_now(&f, txid, 1).unwrap();
    let theirs = f.net.sent()[1].clone();
    let refund = script(
        f.session_of(0, &only_session(&f)).rows[0]
            .refund_address
            .as_deref()
            .unwrap(),
    );
    assert!(theirs.output.iter().any(|o| o.script_pubkey == refund));
    delisted(&f, 105);
    scanned(&f, 104, 105);
    assert_eq!(op_state_of(&f, UNSEEN), Sent);
    confirm(&f, 0, theirs, 105);
    scanned(&f, 105, 105);
    assert_eq!(op_state_of(&f, UNSEEN), Conflicted);
    assert_eq!(op_state_of(&f, &op.id), Conflicted);
}

/// D83, review r1 m-2: an early dissolution leaving the actor's output in
/// (0, dust) is previewed with the warning but refused on send, nothing
/// saved or sent; a fee raised by the output omits it, and that sends.
#[test]
fn test_QT_127_dissolve_refuses_a_dust_actor_output() {
    let f = fx(3, true);
    let (_, txid) = listed_with(&f, 300 * COIN - 1_100);
    let p = f
        .run(
            f.mn()
                .preview_dissolve(f.wallets[1], request(txid, 1, 1_000)),
        )
        .unwrap();
    assert_eq!((p.penalty, p.returns[1].amount), (300 * COIN - 1_100, 100));
    assert_eq!(p.warnings, vec![SharedWarning::ActorOutputDust]);
    let e = f
        .run(f.mn().dissolve(
            f.wallets[1],
            request(txid, 1, 1_000),
            f.grant(1, 300 * COIN),
        ))
        .unwrap_err();
    assert!(e.to_string().contains("dust"), "{e}");
    assert_eq!(code(e), "invalid_argument");
    assert!(f.net.sent().is_empty());
    assert!(f.run(f.mn().operations(f.wallets[1])).unwrap().is_empty());

    let p = f
        .run(
            f.mn()
                .preview_dissolve(f.wallets[1], request(txid, 1, 1_100)),
        )
        .unwrap();
    assert_eq!((p.returns[1].amount, p.warnings.len()), (0, 0));
    let op = f
        .run(f.mn().dissolve(
            f.wallets[1],
            request(txid, 1, 1_100),
            f.grant(1, 300 * COIN),
        ))
        .unwrap();
    assert_eq!(op.state, ProTxOperationState::Sent);
    assert_eq!(
        f.net.sent()[0].output.len(),
        2,
        "the actor's output omitted"
    );
}

/// D23, review r1 n-2: a reward change one block deep still holds the
/// dissolution (a reorg may put it back out); buried, it doesn't. When a
/// reorg does put it back out after the dissolution went, the two resend
/// as `update_registrar` rows do (MG-04 D43, D49): the reward change is
/// superseded by the later dissolution, which waits for the reward change
/// to be mined and buried again, then resends.
#[test]
fn test_QT_127_a_reward_change_holds_dissolve_until_buried_and_after_a_reorg() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    f.credit(0, 9, COIN);
    let update = f
        .run(f.mn().update_share(
            f.wallets[0],
            UpdateShareRequest {
                pro_tx_hash: txid.to_string(),
                share_index: 0,
                reward: RewardChoice::NewAddress,
            },
            f.grant(0, COIN),
        ))
        .unwrap();
    let u = f.net.sent()[0].clone();
    confirm(&f, 0, u.clone(), 101);
    f.tip(101);
    assert_update_pending(dissolve_now(&f, txid, 0).unwrap_err(), &update.id);
    f.tip(106);
    let op = dissolve_now(&f, txid, 0).unwrap();
    assert_eq!(f.net.sent().len(), 2);

    // A reorg takes the reward change's block.
    (f.session.hub.history).with_wallet(f.wallets[0], |h| h.truncate(&[u.txid()], &[]));
    f.tip(105);
    assert_eq!(op_state_of(&f, &update.id), ProTxOperationState::Sent);
    let resend = |id: &str, max: u64| {
        f.run(
            f.mn()
                .resend_operation(f.wallets[0], id.to_string(), f.grant(0, max)),
        )
    };
    assert_update_pending(resend(&op.id, 2 * COIN).unwrap_err(), &update.id);
    let superseded = resend(&update.id, COIN).unwrap_err();
    assert!(
        matches!(&superseded, MasternodesError::UpdateSuperseded { operation } if *operation == op.id),
        "{superseded:?}"
    );
    assert_eq!(f.net.sent().len(), 2, "neither resent");

    // Mined again: one block deep it still holds, buried it doesn't.
    confirm(&f, 0, u, 106);
    f.tip(106);
    assert_update_pending(resend(&op.id, 2 * COIN).unwrap_err(), &update.id);
    f.tip(111);
    resend(&op.id, 2 * COIN).unwrap();
    assert_eq!(f.net.sent().len(), 3);
    assert_eq!(f.net.sent()[2].txid(), f.net.sent()[1].txid());
}
