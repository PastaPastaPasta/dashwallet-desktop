use super::dissolve::{confirm, listed, listed_with, listed_with_reward};
use super::*;
use crate::masternodes::{SharedEligibility, SharedShares, StandbyVariant};

fn shares(f: &Fx, txid: Txid) -> SharedShares {
    f.run(f.mn().shared_shares(f.wallets[0], txid.to_string()))
        .unwrap()
}

#[test]
fn ep_t5_injected_history_shares_report_sessionless_eligibility() {
    let f = fx(3, true);
    let (id, txid) = listed_with_reward(&f, COIN, RewardChoice::NewAddress);
    let original = shares(&f, txid);
    let rows = f.session_of(0, &id).rows;
    assert_ne!(rows[0].reward_address, rows[0].refund_address);
    for (index, (fact, row)) in original.shares.iter().zip(&rows).enumerate() {
        assert_eq!(usize::from(fact.index), index);
        assert_eq!(fact.refund_address, *row.refund_address.as_ref().unwrap());
        assert_eq!(
            fact.reward.at_registration,
            *row.reward_address
                .as_ref()
                .or(row.refund_address.as_ref())
                .unwrap()
        );
    }
    assert_eq!(original.session.as_deref(), Some(id.as_str()));
    assert_eq!(original.eligibility, SharedEligibility::Ok);
    assert_eq!(original.registered_height, 50);
    assert_eq!(original.spend_height, 101);
    assert_eq!(original.early_penalty, COIN);
    assert_eq!(original.full_valid_from, 1_050);
    assert!(!original.full_valid_now);
    assert_eq!(
        original
            .shares
            .iter()
            .map(|s| (s.amount, s.mine))
            .collect::<Vec<_>>(),
        [(400 * COIN, true), (300 * COIN, false), (300 * COIN, false)]
    );
    let mut restored = fx(3, true);
    *restored.seam.list.lock().unwrap() = f.seam.list.lock().unwrap().clone();
    let registration = f.seam.shared_mined.lock().unwrap()[0].1.clone();
    confirm(&restored, 0, registration.clone(), 50);
    let mut expected = original.clone();
    expected.session = None;
    let restored_facts = shares(&restored, txid);
    for (fact, row) in restored_facts.shares.iter().zip(&rows) {
        assert_eq!(fact.refund_address, *row.refund_address.as_ref().unwrap());
        assert_eq!(
            fact.reward.at_registration,
            *row.reward_address
                .as_ref()
                .or(row.refund_address.as_ref())
                .unwrap()
        );
    }
    assert_eq!(restored_facts, expected);
    let cap = expected.early_penalty + 1_000;
    restored
        .run(restored.mn().create_standby(
            restored.wallets[0],
            txid.to_string(),
            1_000,
            restored.grant(0, cap),
        ))
        .unwrap();
    restored
        .run(restored.mn().send_standby(
            restored.wallets[0],
            txid.to_string(),
            0,
            1_000,
            StandbyVariant::Immediate,
            restored.grant(0, cap),
        ))
        .unwrap();
    *restored.seam.list_height.lock().unwrap() = 50;
    assert_eq!(
        shares(&restored, txid).eligibility,
        SharedEligibility::TooEarly
    );
    let entries = restored.seam.list.lock().unwrap().replace(Vec::new());
    assert_eq!(
        shares(&restored, txid).eligibility,
        SharedEligibility::TooEarly
    );
    *restored.seam.list_height.lock().unwrap() = 51;
    let delisted = shares(&restored, txid);
    assert_eq!(delisted.eligibility, SharedEligibility::Delisted);
    assert_eq!(delisted.shares, expected.shares);
    *restored.seam.list.lock().unwrap() = None;
    restored.tip(200);
    let unavailable = shares(&restored, txid);
    assert_eq!(unavailable.eligibility, SharedEligibility::ListUnavailable);
    assert_eq!(unavailable.spend_height, 201);
    restored
        .db()
        .set_setting_durable(GLOBAL_SCOPE, "masternodes.any_shared_seen", None)
        .unwrap();
    assert_eq!(
        shares(&restored, txid).eligibility,
        SharedEligibility::UpgradeUnknown
    );
    *restored.seam.list.lock().unwrap() = Some(Vec::new());
    assert_eq!(
        shares(&restored, txid).eligibility,
        SharedEligibility::UpgradeUnknown
    );
    restored
        .db()
        .set_setting_durable(GLOBAL_SCOPE, "masternodes.any_shared_seen", Some("1"))
        .unwrap();
    *restored.seam.list.lock().unwrap() = entries;
    *restored.seam.list_height.lock().unwrap() = 100;
    let unknown = restored
        .run(
            restored
                .mn()
                .shared_shares(restored.wallets[0], "ab".repeat(32)),
        )
        .unwrap_err();
    assert_eq!(unknown.code(), "masternodes.shared_registration_unknown");
    let bad = restored
        .run(
            restored
                .mn()
                .shared_shares(restored.wallets[0], "invalid".into()),
        )
        .unwrap_err();
    assert_eq!(bad.code(), "invalid_argument");
    restored.restart();
    *restored.seam.list.lock().unwrap() = f.seam.list.lock().unwrap().clone();
    confirm(&restored, 0, registration, 50);
    restored.tip(0);
    assert_eq!(shares(&restored, txid), expected);
    let unrelated = fx(0, true);
    let unrelated_wallet = unrelated
        .run(unrelated.session.import_wallet(
            Zeroizing::new(PHRASES[0].as_bytes().to_vec()),
            Zeroizing::new(vec![b'X'; 8]),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(!f.wallets.contains(&unrelated_wallet));
    assert_eq!(
        unrelated
            .run(
                unrelated
                    .mn()
                    .shared_shares(unrelated_wallet, txid.to_string())
            )
            .unwrap_err()
            .code(),
        "masternodes.shared_registration_unknown"
    );
    let zero = fx(3, true);
    let (_, hash) = listed_with(&zero, 0);
    let zero = shares(&zero, hash);
    assert!(zero.early_period_blocks > 0);
    assert!(zero.spend_height < zero.full_valid_from);
    assert!(zero.full_valid_now);
}

#[test]
fn ep_t7_reward_destination_keeps_confirmed_and_attempt_separate() {
    use crate::masternodes::RewardAttemptState;
    let mut f = fx(3, true);
    let (_, txid) = listed(&f);
    f.credit(0, 9, COIN);
    let original = shares(&f, txid).shares[0].reward.clone();
    let req = UpdateShareRequest {
        pro_tx_hash: txid.to_string(),
        share_index: 0,
        reward: RewardChoice::NewAddress,
    };
    let op = f
        .run(
            f.mn()
                .update_share(f.wallets[0], req.clone(), f.grant(0, COIN)),
        )
        .unwrap();
    let b = f.net.sent().last().unwrap().clone();
    confirm(&f, 0, b.clone(), 110);
    f.spend(0, b.input[0].previous_output);
    f.credit(0, 10, COIN);
    let confirmed = shares(&f, txid).shares[0].reward.clone();
    assert_eq!(confirmed.at_registration, original.at_registration);
    assert_eq!(
        confirmed.latest_confirmed_local.as_ref().unwrap().address,
        op.terms.payout_address
    );
    assert_eq!(
        confirmed
            .latest_confirmed_local
            .as_ref()
            .unwrap()
            .confirmed_height,
        110
    );
    assert!(confirmed.latest_attempt.is_none());
    let wallet = f.wallets[0].to_string();
    let mut row = f.db().masternode_op(&wallet, &op.id).unwrap().unwrap();
    row.id = "0".repeat(64);
    assert!(row.id < op.id);
    row.created_at += 1;
    row.sent_at = None;
    f.db()
        .insert_masternode_op_durable(&wallet, &row, &[], row.created_at)
        .unwrap();
    assert_eq!(
        shares(&f, txid).shares[0]
            .reward
            .latest_confirmed_local
            .as_ref()
            .unwrap()
            .operation,
        row.id
    );
    *f.seam.list_height.lock().unwrap() = 110;
    f.tip(115);
    f.follow();
    f.net.reject_next();
    let pending = f
        .run(f.mn().update_share(f.wallets[0], req, f.grant(0, COIN)))
        .unwrap();
    assert_ne!(pending.terms.payout_address, op.terms.payout_address);
    assert_ne!(op.terms.payout_address, original.at_registration);
    let mut next = f.db().masternode_op(&wallet, &pending.id).unwrap().unwrap();
    assert!(
        f.db()
            .delete_unsent_masternode_op(&wallet, &next.id)
            .unwrap()
    );
    next.created_at = row.created_at + 1;
    let tx: Transaction =
        dashcore::consensus::deserialize(next.signed_tx.as_ref().unwrap()).unwrap();
    f.db()
        .insert_masternode_op_durable(&wallet, &next, &[], next.created_at)
        .unwrap();
    let reward = shares(&f, txid).shares[0].reward.clone();
    let attempt = reward.latest_attempt.unwrap();
    assert_eq!(attempt.state, RewardAttemptState::NotSent);
    assert_eq!(attempt.sent_at, None);
    assert_eq!(
        attempt.address, pending.terms.payout_address,
        "resolved NewAddress from terms"
    );
    f.db()
        .begin_masternode_op_attempt(&wallet, &next.id, next.created_at)
        .unwrap();
    assert_eq!(
        shares(&f, txid).shares[0]
            .reward
            .latest_attempt
            .as_ref()
            .unwrap()
            .state,
        RewardAttemptState::Sent
    );
    let mut conflict = tx.clone();
    conflict.lock_time += 1;
    confirm(&f, 0, conflict.clone(), 111);
    let reward = shares(&f, txid).shares[0].reward.clone();
    assert_eq!(
        reward.latest_attempt.as_ref().unwrap().state,
        RewardAttemptState::Conflicted
    );
    assert_eq!(
        reward.latest_confirmed_local.as_ref().unwrap().operation,
        row.id
    );
    f.restart();
    *f.seam.shared_mined.lock().unwrap() = vec![(
        txid,
        super::super::send::typed(
            &decode_frozen(&parse(&f.message(0, &only_session(&f))))
                .unwrap()
                .tx,
        )
        .unwrap(),
        50,
    )];
    confirm(&f, 0, b, 110);
    confirm(&f, 0, conflict, 111);
    assert_eq!(shares(&f, txid).shares[0].reward, reward);
    let mut discarded = next;
    discarded.id = "ep-discarded".into();
    discarded.created_at += 1;
    discarded.sent_at = None;
    f.db()
        .insert_masternode_op_durable(&wallet, &discarded, &[], discarded.created_at)
        .unwrap();
    assert!(
        f.db()
            .delete_unsent_masternode_op(&wallet, &discarded.id)
            .unwrap()
    );
    assert_eq!(shares(&f, txid).shares[0].reward, reward);
}

/// A one-transaction block enters the production wallet-manager block path.
/// Its emitted events feed the engine handler; the manager's existing
/// persistence adapter builds and commits the records and scan checkpoint.
#[test]
fn ep_r2_scanned_shared_registration_survives_reopening() {
    use dash_spv::EventHandler;
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    use platform_wallet::changeset::PlatformWalletPersistence;
    use platform_wallet::key_wallet_manager::{WalletEvent, WalletInterface};
    use std::collections::BTreeSet;
    use std::time::Duration;
    let f = fx(3, true);
    let (id, txid) = listed_with_reward(&f, COIN, RewardChoice::NewAddress);
    let mut expected = shares(&f, txid);
    let rows = f.session_of(0, &id).rows;
    assert_ne!(rows[0].reward_address, rows[0].refund_address);
    for (fact, row) in expected.shares.iter().zip(&rows) {
        assert_eq!(fact.refund_address, *row.refund_address.as_ref().unwrap());
        assert_eq!(
            fact.reward.at_registration,
            *row.reward_address
                .as_ref()
                .or(row.refund_address.as_ref())
                .unwrap()
        );
    }
    expected.session = None;
    let registration = f.seam.shared_mined.lock().unwrap()[0].1.clone();
    let mut restored = fx(3, true);
    *restored.seam.list.lock().unwrap() = f.seam.list.lock().unwrap().clone();
    let wallet_id = restored.wallets[0];
    assert!(
        restored
            .session
            .hub
            .history
            .get(&wallet_id, &txid)
            .is_none()
    );
    assert!(
        restored
            .run(restored.mn().shared_sessions(wallet_id))
            .unwrap()
            .is_empty()
    );
    let mut block = dashcore::Block {
        header: dashcore::blockdata::block::Header {
            version: dashcore::blockdata::block::Version::from_consensus(1),
            prev_blockhash: dashcore::BlockHash::all_zeros(),
            merkle_root: dashcore::TxMerkleNode::all_zeros(),
            time: 1_793_598_712,
            bits: dashcore::CompactTarget::from_consensus(0x207f_ffff),
            nonce: 0,
        },
        txdata: vec![registration],
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    let events = restored.run(async {
        let wm = restored.session.manager().unwrap().wallet_manager_arc();
        let mut wm = wm.write().await;
        let mut events = wm.subscribe_events();
        let result = wm
            .process_block_for_wallets(
                &block,
                block.block_hash(),
                50,
                &BTreeSet::from([wallet_id.0]),
            )
            .await;
        assert!(result.new_txids.contains(&txid));
        wm.update_wallet_synced_height(&wallet_id.0, 100);
        let mut emitted = Vec::new();
        while let Ok(event) = events.try_recv() {
            emitted.push(event);
        }
        assert!(emitted.iter().any(|event| matches!(
            event,
            WalletEvent::BlockProcessed { wallet_id: id, inserted, .. }
                if *id == wallet_id.0 && inserted.iter().any(|r| r.txid == txid)
        )));
        emitted
    });
    for event in events {
        restored.session.hub.on_wallet_event(&event);
    }
    restored.run(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let persisted = restored.session.live().unwrap().store.load().unwrap();
                if persisted
                    .wallets
                    .get(&wallet_id.0)
                    .is_some_and(|wallet| wallet.wallet_info.synced_height() == 100)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the production event adapter must persist the scanned checkpoint");
    });
    assert_eq!(shares(&restored, txid), expected);
    assert!(
        restored
            .run(restored.mn().shared_sessions(wallet_id))
            .unwrap()
            .is_empty()
    );
    restored.restart();
    *restored.seam.list.lock().unwrap() = f.seam.list.lock().unwrap().clone();
    assert_eq!(
        shares(&restored, txid),
        expected,
        "history reconstructed from persisted scanner records"
    );
    assert!(
        restored
            .run(restored.mn().shared_sessions(wallet_id))
            .unwrap()
            .is_empty()
    );
    let cap = expected.early_penalty + 1_000;
    let short = restored
        .run(restored.mn().create_standby(
            wallet_id,
            txid.to_string(),
            1_000,
            restored.grant(0, cap - 1),
        ))
        .unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    restored
        .run(restored.mn().create_standby(
            wallet_id,
            txid.to_string(),
            1_000,
            restored.grant(0, cap),
        ))
        .unwrap();
    let short = restored
        .run(restored.mn().send_standby(
            wallet_id,
            txid.to_string(),
            0,
            1_000,
            StandbyVariant::Immediate,
            restored.grant(0, cap - 1),
        ))
        .unwrap_err();
    assert_eq!(send_code(&short), "send.grant_exceeded");
    restored
        .run(restored.mn().send_standby(
            wallet_id,
            txid.to_string(),
            0,
            1_000,
            StandbyVariant::Immediate,
            restored.grant(0, cap),
        ))
        .unwrap();
}

#[test]
fn ep_r4_share_reads_match_tip_registration_and_owner() {
    let f = fx(3, true);
    let (first, first_hash) = listed(&f);
    let first_tx = f.seam.shared_mined.lock().unwrap()[0].1.clone();
    for wallet in 0..3 {
        for input in &first_tx.input {
            f.spend(wallet, input.previous_output);
        }
    }
    let (second, second_hash) = confirmed_with_reward(&f, 60, COIN, RewardChoice::Refund, 11);
    assert_ne!(first_hash, second_hash);
    let second_tx =
        super::super::send::typed(&decode_frozen(&parse(&f.message(0, &second))).unwrap().tx)
            .unwrap();
    for wallet in 0..3 {
        for input in &second_tx.input {
            f.spend(wallet, input.previous_output);
        }
    }
    {
        let mut mined = f.seam.shared_mined.lock().unwrap();
        mined.retain(|(id, _, _)| *id != second_hash);
        mined.push((second_hash, second_tx, 60));
    }
    f.follow();
    {
        let mut list = f.seam.list.lock().unwrap();
        let list = list.as_mut().unwrap();
        let mut entry = list[0].clone();
        entry.pro_tx_hash = second_hash.to_byte_array();
        entry.service_address = Some("1.2.3.11:19999".parse().unwrap());
        list.push(entry);
    }
    f.credit(0, 21, 401 * COIN);
    let mut request = request(&[("A", 400), ("B", 300), ("C", 300)], FEE);
    request.terms.service = "1.2.3.21:19999".into();
    let draft = start(&f, 0, request).unwrap().session;
    assert_eq!(draft.stage, SharedStage::Draft);
    assert_eq!(
        f.run(f.mn().shared_sessions(f.wallets[0])).unwrap().len(),
        3
    );
    f.tip(150);
    for (hash, session) in [(first_hash, first), (second_hash, second)] {
        let facts = shares(&f, hash);
        assert_eq!(facts.session.as_deref(), Some(session.as_str()));
        assert_eq!(facts.spend_height, 151, "header tip is above list 100");
        let other = f
            .run(f.mn().shared_shares(f.wallets[1], hash.to_string()))
            .unwrap();
        assert_eq!(
            other.shares.iter().map(|s| s.mine).collect::<Vec<_>>(),
            [false, true, false],
            "public read for share 1's owner"
        );
    }
}

#[test]
fn ep_r4_reward_height_precedes_creation_time() {
    let f = fx(3, true);
    let (_, hash) = listed(&f);
    f.credit(0, 9, COIN);
    let req = UpdateShareRequest {
        pro_tx_hash: hash.to_string(),
        share_index: 0,
        reward: RewardChoice::NewAddress,
    };
    let first = f
        .run(
            f.mn()
                .update_share(f.wallets[0], req.clone(), f.grant(0, COIN)),
        )
        .unwrap();
    let first_tx = f.net.sent().last().unwrap().clone();
    confirm(&f, 0, first_tx.clone(), 110);
    f.spend(0, first_tx.input[0].previous_output);
    *f.seam.list_height.lock().unwrap() = 110;
    f.tip(115);
    f.follow();
    f.credit(0, 10, COIN);
    f.net.reject_next();
    let second = f
        .run(f.mn().update_share(f.wallets[0], req, f.grant(0, COIN)))
        .unwrap();
    assert_ne!(first.terms.payout_address, second.terms.payout_address);
    let wallet = f.wallets[0].to_string();
    let mut row = f.db().masternode_op(&wallet, &second.id).unwrap().unwrap();
    assert!(
        f.db()
            .delete_unsent_masternode_op(&wallet, &row.id)
            .unwrap()
    );
    row.created_at = first.created_at - 1;
    f.db()
        .insert_masternode_op_durable(&wallet, &row, &[], row.created_at)
        .unwrap();
    let tx = dashcore::consensus::deserialize(row.signed_tx.as_ref().unwrap()).unwrap();
    confirm(&f, 0, tx, 120);
    let latest = shares(&f, hash).shares[0]
        .reward
        .latest_confirmed_local
        .clone()
        .unwrap();
    assert_eq!(latest.operation, second.id);
    assert_eq!(latest.address, second.terms.payout_address);
    assert_eq!(latest.confirmed_height, 120);
}
