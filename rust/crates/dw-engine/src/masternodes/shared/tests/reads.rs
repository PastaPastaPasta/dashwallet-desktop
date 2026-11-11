use super::dissolve::{confirm, listed, listed_with};
use super::*;
use crate::masternodes::{SharedEligibility, SharedShares, StandbyVariant};

fn shares(f: &Fx, txid: Txid) -> SharedShares {
    f.run(f.mn().shared_shares(f.wallets[0], txid.to_string()))
        .unwrap()
}

#[test]
fn ep_t5_history_shares_survive_restore_and_report_eligibility() {
    let f = fx(3, true);
    let (id, txid) = listed(&f);
    let original = shares(&f, txid);
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
    assert_eq!(shares(&restored, txid), expected);
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
    let unrelated = fx(1, true);
    assert_eq!(
        unrelated
            .run(
                unrelated
                    .mn()
                    .shared_shares(unrelated.wallets[0], txid.to_string())
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
    row.id = "ep-confirmed-tie".into();
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
