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
