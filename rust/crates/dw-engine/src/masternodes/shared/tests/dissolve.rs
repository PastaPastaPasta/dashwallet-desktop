//! Dissolve now (MG-04b §5, §6.3, slice 2a) on [`confirmed`]'s
//! three-party masternode: A (wallet 0) holds share 0 (400), B and C 300
//! each; registered at 50, early period 1,000 blocks, penalty 1 DASH.

use dw_protx::shared::rules::{SharedMn, check_dissolution};
use dw_protx::shared::{ProDisTx, SpecialTx, TRANSACTION_PROVIDER_DISSOLVE};

use super::*;
use crate::masternodes::{DissolveRequest, ProTxOperationKind, ProTxOperationState};

/// [`confirmed`]'s masternode in the synced list at 100, its registration
/// in the history with its payload (the vault's dissolution signer reads
/// the share table from it, D29).
fn listed(f: &Fx) -> (String, Txid) {
    let (id, txid) = confirmed(f, 50);
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
    assert!(
        matches!(update, Err(MasternodesError::UpdatePending { ref operation }) if *operation == op.id),
        "{update:?}"
    );

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
    let op = f
        .run(
            f.mn()
                .dissolve(f.wallets[0], request(txid, 0, 1_000), f.grant(0, 2 * COIN)),
        )
        .unwrap();
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
    let e = f
        .run(
            f.mn()
                .dissolve(f.wallets[0], request(txid, 0, 1_000), f.grant(0, 2 * COIN)),
        )
        .unwrap_err();
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
    let e = f
        .run(
            f.mn()
                .dissolve(f.wallets[0], request(txid, 0, 1_000), f.grant(0, 2 * COIN)),
        )
        .unwrap_err();
    assert!(
        matches!(e, MasternodesError::UpdatePending { ref operation } if *operation == update.id),
        "{e:?}"
    );
    assert_eq!(f.net.sent().len(), 1);
}

/// D81 in the engine: a `sent` dissolve paying this wallet nothing (the
/// fixture's penalty can't consume a share, so a copy of a sent row whose
/// record says so) reads `confirmed` by the list, at its height, once the
/// masternode has left it and the wallet's scan has reached that height.
#[test]
fn test_QT_127_an_unseen_dissolution_confirms_by_the_list_after_the_scan() {
    let f = fx(3, true);
    let (_, txid) = listed(&f);
    let op = f
        .run(
            f.mn()
                .dissolve(f.wallets[0], request(txid, 0, 1_000), f.grant(0, 2 * COIN)),
        )
        .unwrap();
    let w = f.wallets[0].to_string();
    let db = f.db();
    let row = db.masternode_op(&w, &op.id).unwrap().unwrap();
    let mut record: serde_json::Value = serde_json::from_str(&row.request).unwrap();
    record["actor_output"] = 0.into();
    let unseen = dw_appdb::MasternodeOpRow {
        id: "unseen".into(),
        request: record.to_string(),
        sent_at: None,
        ..row
    };
    db.insert_masternode_op_durable(&w, &unseen, &[], 0)
        .unwrap();
    db.begin_masternode_op_attempt(&w, "unseen", 1).unwrap();
    let scanned = |height: u32| {
        let mut state = f
            .session
            .hub
            .wallet_state(&f.wallets[0])
            .unwrap_or_default();
        (state.synced_height, state.processed_height) = (height, height);
        f.session.hub.set_wallet_state(f.wallets[0], state);
    };
    let state = || {
        let ops = f.run(f.mn().operations(f.wallets[0])).unwrap();
        let of = |id: &str| ops.iter().find(|o| o.id == id).unwrap().state;
        (of("unseen"), of(&op.id))
    };
    use ProTxOperationState::{Confirmed, Sent};
    scanned(100);
    assert_eq!(state(), (Sent, Sent), "still listed");
    *f.seam.list.lock().unwrap() = Some(Vec::new());
    *f.seam.list_height.lock().unwrap() = 105;
    scanned(104);
    assert_eq!(state(), (Sent, Sent), "the scan is behind the list");
    scanned(105);
    // The row paying this wallet waits for its history.
    assert_eq!(state(), (Confirmed { height: 105 }, Sent));
}
