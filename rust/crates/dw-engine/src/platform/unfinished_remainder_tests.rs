//! Tests of unfinished_remainder.rs (#402): recovering the value left on a
//! transfer to Shielded that Platform refused after a penalty, on
//! platform_fund_tests.rs's fixture and scripted library ([`FakeFund`]),
//! with the real vault, lease, dispatch journal and `app.sqlite`.

use std::str::FromStr;
use std::sync::Arc;

use dashcore::{OutPoint, ScriptBuf};
use key_wallet::bip32::DerivationPath;
use platform_wallet::{AssetLockFundingType, AssetLockStatus, TrackedAssetLock};

use super::super::errors::PlatformError;
use super::super::flows::FlowKind;
use super::super::platform_addresses::{PlatformActivityKind, PlatformActivityState};
use super::super::platform_fund as fund;
use super::super::platform_moves;
use super::super::requests::tests::Fixture;
use super::super::shielded::SHIELDED_ACTIVATION_VERSION;
use super::super::unfinished::RecoverQuote;
use super::super::unfinished_conformance as conformance;
use super::super::unfinished_locks::{self as locks, LockFinish, LockKindId, Mark, SpentRead};
use super::super::unfinished_remainder::{self as remainder, NOT_RECOVERABLE};
use super::{Answer, FakeFund, grant, run};

/// The shield's credit key (account 0, index 0), on testnet.
const SHIELD_KEY: &str = "m/9'/1'/5'/5'/0";

/// A lock's credits: 0.01 DASH.
const LOCK_DUFFS: u64 = 1_000_000;

/// The P2PKH script of `path` as a `PlatformAddressFund` lease's funding
/// signer derives it.
fn script_of(fx: &Fixture, path: &str) -> ScriptBuf {
    let (s, wallet, grant) = (Arc::clone(&fx.s), fx.wallet, grant(fx, 1, 0));
    let path = DerivationPath::from_str(path).unwrap();
    fx.engine.block_on(async move {
        let lease = (s.begin_lease(wallet, FlowKind::PlatformAddressFund, &[grant]))
            .await
            .unwrap();
        let funding = lease.uncounted_funding_signer().unwrap();
        let key = key_wallet::Signer::public_key(&funding, &path)
            .await
            .unwrap();
        ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(key).pubkey_hash())
    })
}

/// The fee floor at the fake's version.
fn floor(lib: &FakeFund) -> u64 {
    let rules = dpp::version::PlatformVersion::get(fund::FundLib::protocol_version(lib)).unwrap();
    fund::fee_credits(rules)
}

/// A proved type-5 lock `n` paying the shield key, tracked by `lib`, which
/// signs with that key.
fn shield_lock(fx: &Fixture, lib: &FakeFund, n: u8) -> TrackedAssetLock {
    let tx = conformance::transaction(n, 1, LOCK_DUFFS, &script_of(fx, SHIELD_KEY));
    let row = conformance::tracked(
        &tx,
        0,
        AssetLockFundingType::AssetLockShieldedAddressTopUp,
        AssetLockStatus::ChainLocked,
        true,
    );
    lib.track(row.clone());
    *lib.key.lock().unwrap() = DerivationPath::from_str(SHIELD_KEY).unwrap();
    row
}

/// What the shield kind keeps after a signed shield that Platform proved
/// partly used: `credits` left.
fn mark_remaining(fx: &Fixture, at: OutPoint, credits: u64) {
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    fx.engine.block_on(async move {
        locks::record_remaining(&s, wallet, LockKindId::Shielded, at, credits).await
    });
}

fn quote(fx: &Fixture, lib: &Arc<FakeFund>, at: OutPoint) -> RecoverQuote {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        remainder::quote(&s, wallet, &*lib, at).await
    })
    .unwrap()
}

fn recover(
    fx: &Fixture,
    lib: &Arc<FakeFund>,
    at: OutPoint,
    grant: &str,
) -> Result<LockFinish, PlatformError> {
    let (wallet, grant) = (fx.wallet, grant.to_owned());
    run(fx, lib, move |s, lib| async move {
        remainder::recover(&s, wallet, &*lib, at, &grant).await
    })
}

/// The grant `grant` is still unredeemed.
fn kept(fx: &Fixture, grant: &str) -> bool {
    let (s, wallet, grant) = (Arc::clone(&fx.s), fx.wallet, grant.to_owned());
    fx.engine
        .block_on(async move {
            s.begin_lease(wallet, FlowKind::PlatformAddressFund, &[grant])
                .await
        })
        .is_ok()
}

fn marks(fx: &Fixture, mark: Mark) -> std::collections::HashSet<OutPoint> {
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    fx.engine
        .block_on(async move { locks::marks_of(&s, wallet, LockKindId::Shielded, mark).await })
}

fn remaining(fx: &Fixture) -> std::collections::HashMap<OutPoint, u64> {
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    fx.engine
        .block_on(async move { locks::remaining_of(&s, wallet, LockKindId::Shielded).await })
}

/// The wallet's rows of `app.sqlite` a recover could write: the unfinished
/// transfers' records and the Platform activity's (background tasks such as
/// the auto-backup write others).
fn rows(fx: &Fixture) -> Vec<(String, String)> {
    let appdb = fx.s.live().unwrap().appdb;
    let scope = dw_appdb::local_scope(&fx.wallet.to_string());
    let mut rows = appdb.settings_with_prefix(&scope, "recovery.").unwrap();
    rows.extend(
        appdb
            .settings_with_prefix(&scope, "platform_address/")
            .unwrap(),
    );
    rows
}

fn record(fx: &Fixture, at: OutPoint) -> Option<platform_moves::FundRecord> {
    let (s, wallet, txid) = (Arc::clone(&fx.s), fx.wallet, at.txid.to_string());
    fx.engine
        .block_on(async move { platform_moves::fund_record(&s, wallet, &txid).await })
        .unwrap()
}

/// A proved Remaining at least the fee is quoted, and a grant of exactly
/// those credits recovers it: to the wallet's own receive address, signed
/// with the lock's credit key under a `PlatformAddressFund` lease (the
/// vault's real funding signer), settled as the shield kind records it,
/// the library's row `Consumed`, and a done "recovered" activity row.
#[test]
fn a_partly_used_shield_lock_is_recovered_to_the_wallets_own_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let at = shield_lock(&fx, &lib, 0x51).out_point;
    let (left, fee) = (600_000_000, floor(&lib));
    mark_remaining(&fx, at, left);
    lib.proves(at, SpentRead::Remaining { credits: left });

    assert_eq!(
        quote(&fx, &lib, at),
        RecoverQuote::Ready {
            credits: left,
            fee_credits: fee
        }
    );
    let gained = left - fee;
    lib.answer(Answer::Done(Some(gained)));
    let g = grant(&fx, 1, left);
    assert_eq!(recover(&fx, &lib, at, &g).unwrap(), LockFinish::Applied);

    assert_eq!(*lib.recovered.lock().unwrap(), [(at, lib.receive)]);
    assert!(lib.applied().is_empty(), "never the type-4 apply");
    assert_eq!(*lib.signed.lock().unwrap(), [true], "the lock key signed");
    assert!(marks(&fx, Mark::Settled).contains(&at), "settled");
    let row = (lib.tracked.lock().unwrap().iter())
        .find(|l| l.out_point == at)
        .map(|l| l.status.clone());
    assert_eq!(row, Some(AssetLockStatus::Consumed), "the library's row");
    let r = record(&fx, at).expect("a recovered record");
    assert_eq!(
        r.duffs,
        left.div_ceil(1000),
        "sized by what was left, not the lock"
    );
    let activity = r.activity();
    assert_eq!(activity.kind, PlatformActivityKind::Recovered);
    assert_eq!(activity.state, PlatformActivityState::Done);
    assert_eq!((activity.credits, activity.estimated), (gained, false));
    let to = platform_moves::bech32m(&lib.receive, dashcore::Network::Testnet);
    assert_eq!(activity.address.as_deref(), Some(to.as_str()));

    // Settled: a second recover moves nothing.
    let again = grant(&fx, 1, left);
    assert_eq!(recover(&fx, &lib, at, &again).unwrap(), LockFinish::Applied);
    assert_eq!(lib.recovered.lock().unwrap().len(), 1);
    assert!(kept(&fx, &again));
}

/// Below Platform's fee the lock is quoted too small, and a recover is
/// refused before its grant: nothing applied, the grant kept.
#[test]
fn a_remaining_value_below_the_fee_is_never_recovered() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let at = shield_lock(&fx, &lib, 0x52).out_point;
    let fee = floor(&lib);
    mark_remaining(&fx, at, fee - 1);
    lib.proves(at, SpentRead::Remaining { credits: fee - 1 });
    assert_eq!(
        quote(&fx, &lib, at),
        RecoverQuote::TooSmall {
            credits: fee - 1,
            fee_credits: fee
        }
    );
    let g = grant(&fx, 1, fee - 1);
    let refused = recover(&fx, &lib, at, &g).unwrap();
    let min = super::super::errors::CreditsError::BelowMinimum { min: fee }.code();
    assert_eq!(refused, LockFinish::NotApplied { error: min.into() });
    assert!(lib.recovered.lock().unwrap().is_empty());
    assert!(kept(&fx, &g), "the grant is kept");
}

/// Only a lock the shield kind lists Recoverable is offered: one with no
/// proved Remaining, whatever Platform says now, is refused before its
/// grant, and so is a recover the fresh read no longer backs (unspent, or
/// no proved answer). A fresh read that shows the lock used up takes it off
/// the list as used elsewhere; a type-4 lock is no shield lock. None of them
/// signs or redeems anything.
#[test]
fn only_a_proved_remaining_is_recovered() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let left = 600_000_000;

    let plain = shield_lock(&fx, &lib, 0x53).out_point;
    lib.proves(plain, SpentRead::Remaining { credits: left });
    let g = grant(&fx, 1, left);
    let refused = recover(&fx, &lib, plain, &g).unwrap();
    assert_eq!(
        refused,
        LockFinish::NotApplied {
            error: NOT_RECOVERABLE.into()
        }
    );
    assert_eq!(
        quote(&fx, &lib, plain),
        RecoverQuote::NotApplied {
            error: NOT_RECOVERABLE.into()
        }
    );

    let unread = shield_lock(&fx, &lib, 0x54).out_point;
    mark_remaining(&fx, unread, left);
    let refused = recover(&fx, &lib, unread, &g).unwrap();
    let unavailable = PlatformError::Unavailable.code();
    assert_eq!(
        refused,
        LockFinish::NotApplied {
            error: unavailable.into()
        }
    );

    let unspent = shield_lock(&fx, &lib, 0x55).out_point;
    mark_remaining(&fx, unspent, left);
    lib.proves(unspent, SpentRead::Unspent);
    let refused = recover(&fx, &lib, unspent, &g).unwrap();
    assert_eq!(
        refused,
        LockFinish::NotApplied {
            error: NOT_RECOVERABLE.into()
        }
    );

    let used = shield_lock(&fx, &lib, 0x56).out_point;
    mark_remaining(&fx, used, left);
    lib.proves(used, SpentRead::Consumed);
    assert_eq!(
        recover(&fx, &lib, used, &g).unwrap(),
        LockFinish::ReportedUsed
    );
    assert!(marks(&fx, Mark::Used).contains(&used));
    assert!(
        !marks(&fx, Mark::Settled).contains(&used),
        "not this wallet's recover"
    );
    assert!(record(&fx, used).is_none());
    assert_eq!(quote(&fx, &lib, used), RecoverQuote::Finished);

    let fund_row = lib.lock(0x5F, LOCK_DUFFS, AssetLockStatus::ChainLocked, true);
    let fund_lock = fund_row.out_point;
    lib.track(fund_row);
    mark_remaining(&fx, fund_lock, left);
    lib.proves(fund_lock, SpentRead::Remaining { credits: left });
    let refused = recover(&fx, &lib, fund_lock, &g).unwrap_err();
    assert!(
        matches!(refused, PlatformError::InvalidArgument { .. }),
        "an Add from Dash wallet lock: {refused:?}"
    );

    assert!(lib.recovered.lock().unwrap().is_empty());
    assert!(lib.signed.lock().unwrap().is_empty());
    assert!(kept(&fx, &g), "the grant is kept");
}

/// The shield flow's own network gate comes first: where Platform's
/// shielded pool is not active, a quote and a recover are refused before
/// anything is read, written, redeemed or signed. The wallet's recovery and
/// Platform activity rows of `app.sqlite` stay byte-equal.
#[test]
fn a_recover_is_behind_the_shield_flows_network_gate() {
    let fx = Fixture::new(0);
    let mut fake = FakeFund::new(&fx);
    fake.version = SHIELDED_ACTIVATION_VERSION - 1;
    let lib = Arc::new(fake);
    let at = shield_lock(&fx, &lib, 0x57).out_point;
    mark_remaining(&fx, at, 600_000_000);
    lib.proves(
        at,
        SpentRead::Remaining {
            credits: 700_000_000,
        },
    );
    let before = rows(&fx);
    let g = grant(&fx, 1, 700_000_000);

    let quoted = quote(&fx, &lib, at);
    let refused = recover(&fx, &lib, at, &g).unwrap();
    let code = |r: &LockFinish| match r {
        LockFinish::NotApplied { error } => error.clone(),
        other => panic!("{other:?}"),
    };
    assert!(
        code(&refused).starts_with("shielded.not_active"),
        "{refused:?}"
    );
    assert_eq!(
        quoted,
        RecoverQuote::NotApplied {
            error: code(&refused)
        }
    );
    assert_eq!(rows(&fx), before, "nothing written");
    assert_eq!(
        remaining(&fx).get(&at),
        Some(&600_000_000),
        "the hint is not re-read"
    );
    assert!(lib.recovered.lock().unwrap().is_empty());
    assert!(kept(&fx, &g), "the grant is not redeemed");
}

/// Off on mainnet, first: a quote and a recover are refused
/// `platform.feature_off` before any Platform read, lock, record or grant.
/// The wallet's recovery and activity rows stay byte-equal, the grant is
/// still redeemable, and the shield side still keeps the lock out of the
/// pending shields there.
#[test]
fn a_recover_is_off_on_mainnet_before_anything_is_read() {
    let fx = Fixture::new(0);
    let mut fake = FakeFund::new(&fx);
    fake.network = dashcore::Network::Mainnet;
    let lib = Arc::new(fake);
    let at = shield_lock(&fx, &lib, 0x5C).out_point;
    mark_remaining(&fx, at, 600_000_000);
    lib.proves(
        at,
        SpentRead::Remaining {
            credits: 600_000_000,
        },
    );
    let before = rows(&fx);
    let g = grant(&fx, 1, 600_000_000);

    let off = super::super::errors::PlatformError::FeatureOff {
        feature: remainder::FEATURE.into(),
    }
    .code();
    assert_eq!(
        quote(&fx, &lib, at),
        RecoverQuote::NotApplied { error: off.into() }
    );
    assert_eq!(
        recover(&fx, &lib, at, &g).unwrap(),
        LockFinish::NotApplied { error: off.into() }
    );
    assert_eq!(
        lib.platform_reads.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no Platform read"
    );
    assert_eq!(rows(&fx), before, "nothing written");
    assert!(lib.recovered.lock().unwrap().is_empty());
    assert!(lib.signed.lock().unwrap().is_empty());
    assert!(kept(&fx, &g), "the grant is not redeemed");
}

/// The grant is sized from the fresh proved read, never the listed hint: a
/// grant of other credits, or a held lease, is refused before it is
/// redeemed, and the hint takes the fresh value.
#[test]
fn the_grant_must_carry_the_fresh_reads_credits() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let at = shield_lock(&fx, &lib, 0x58).out_point;
    mark_remaining(&fx, at, 600_000_000);
    lib.proves(
        at,
        SpentRead::Remaining {
            credits: 550_000_000,
        },
    );

    for credits in [600_000_000, 550_000_001, 0] {
        let g = grant(&fx, 1, credits);
        assert_eq!(
            recover(&fx, &lib, at, &g).unwrap_err(),
            PlatformError::GrantInvalid,
            "{credits}"
        );
        assert!(kept(&fx, &g), "{credits}: kept");
    }
    assert_eq!(remaining(&fx).get(&at), Some(&550_000_000));

    let (s, wallet, held) = (Arc::clone(&fx.s), fx.wallet, grant(&fx, 1, 550_000_000));
    let lease = fx
        .engine
        .block_on(async move {
            s.begin_lease(wallet, FlowKind::PlatformAddressFund, &[held])
                .await
        })
        .unwrap();
    let named = super::super::lease::lease_string(&lease.id());
    assert_eq!(
        recover(&fx, &lib, at, &named).unwrap_err(),
        PlatformError::GrantInvalid
    );

    let no_funding = grant(&fx, 0, 550_000_000);
    assert!(matches!(
        recover(&fx, &lib, at, &no_funding).unwrap_err(),
        PlatformError::NeedsGrant { .. }
    ));
    assert!(lib.recovered.lock().unwrap().is_empty());
}

/// The destination is the wallet's own receive address and nothing else:
/// neither the call (`DashPay::recover_transfer(txid, vout, grant)`) nor
/// the quote names one, and the recover pays the receive address the
/// library gives, whatever the lock's own fund records say.
#[test]
fn a_recover_pays_only_the_wallets_receive_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let at = shield_lock(&fx, &lib, 0x59).out_point;
    let left = 600_000_000;
    mark_remaining(&fx, at, left);
    lib.proves(at, SpentRead::Remaining { credits: left });
    lib.answer(Answer::Done(None));
    let g = grant(&fx, 1, left);
    assert_eq!(recover(&fx, &lib, at, &g).unwrap(), LockFinish::Applied);
    assert_eq!(*lib.recovered.lock().unwrap(), [(at, lib.receive)]);
    let to = platform_moves::bech32m(&lib.receive, dashcore::Network::Testnet);
    assert_eq!(record(&fx, at).unwrap().address, to);
}

/// The lock key and no other: a recover whose library asks for another
/// key's signature is refused with nothing signed, and leaves no armed
/// record behind.
#[test]
fn a_recover_signs_with_the_locks_credit_key_only() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let at = shield_lock(&fx, &lib, 0x5A).out_point;
    let left = 600_000_000;
    mark_remaining(&fx, at, left);
    lib.proves(at, SpentRead::Remaining { credits: left });
    *lib.key.lock().unwrap() = DerivationPath::from_str("m/9'/1'/5'/4'/0").unwrap();
    lib.answer(Answer::Failed);
    let g = grant(&fx, 1, left);
    let refused = recover(&fx, &lib, at, &g).unwrap();
    assert!(
        matches!(refused, LockFinish::NotApplied { .. }),
        "{refused:?}"
    );
    assert_eq!(
        *lib.signed.lock().unwrap(),
        [false],
        "the other key is refused"
    );
    assert!(record(&fx, at).is_none(), "nothing armed");
    assert!(!marks(&fx, Mark::Settled).contains(&at));
}

/// A failure after the signature keeps the lock listed and its record
/// armed; the next recover's proved read shows it used up, so it was this
/// wallet's: settled, its record done with its estimate.
#[test]
fn a_recover_that_failed_after_signing_settles_on_the_next_proved_read() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeFund::new(&fx));
    let at = shield_lock(&fx, &lib, 0x5B).out_point;
    let (left, fee) = (600_000_000, floor(&lib));
    mark_remaining(&fx, at, left);
    lib.proves(at, SpentRead::Remaining { credits: left });
    lib.answer(Answer::Failed);
    let g = grant(&fx, 1, left);
    assert_eq!(recover(&fx, &lib, at, &g).unwrap(), LockFinish::Checking);
    assert!(!marks(&fx, Mark::Settled).contains(&at));
    assert_eq!(remaining(&fx).get(&at), Some(&left), "still offered");
    let pending = record(&fx, at).unwrap();
    assert!(pending.armed && pending.recovered);
    assert_eq!(pending.activity().state, PlatformActivityState::Pending);

    lib.proves(at, SpentRead::Consumed);
    let again = grant(&fx, 1, left);
    assert_eq!(recover(&fx, &lib, at, &again).unwrap(), LockFinish::Applied);
    assert_eq!(lib.recovered.lock().unwrap().len(), 1, "not applied again");
    assert!(marks(&fx, Mark::Settled).contains(&at));
    let done = record(&fx, at).unwrap().activity();
    assert_eq!(done.kind, PlatformActivityKind::Recovered);
    assert_eq!(done.state, PlatformActivityState::Done);
    assert_eq!((done.credits, done.estimated), (left - fee, true));
}
