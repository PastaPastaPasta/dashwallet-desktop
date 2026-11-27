//! Shielded → Platform (C-routes D-C1, C1b): `unshield_to_platform` and its
//! quote over the scripted library. The own-address rule (C1-1) refuses a
//! foreign `to` before the grant; the gates, the pending outcomes and the
//! error mapping are `unshield_to_core`'s, rerun for the new call.

use super::*;

/// An unshield to a Platform address of `credits` under `grant`.
fn unshield(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    credits: u64,
    to: Option<String>,
    grant: &str,
    confirmed: bool,
) -> Result<super::super::shielded::UnshieldToPlatformOutcome, ShieldedError> {
    let (wallet, grant) = (fx.wallet, grant.to_owned());
    run(fx, lib, move |s, lib| async move {
        let to = to.as_deref();
        pool::unshield_to_platform(&s, wallet, &*lib, credits, to, &grant, confirmed).await
    })
}

fn lease(fx: &Fixture) -> String {
    fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)])
}

/// A Platform address of this network outside the wallet's payment account.
fn foreign() -> String {
    platform_text(PlatformAddress::P2pkh([0xee; 20]))
}

#[test]
fn unshield_to_platform_quote_follows_platforms_unshield_rules() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let quote = run(&fx, &lib, |_, lib| async move {
        pool::unshield_to_platform_quote(&*lib, NOTE, None).await
    })
    .unwrap();
    let fee = fee_of(NOTE, ShieldedFeeKind::Unshield);
    assert_eq!(
        (quote.credits, quote.fee_credits, quote.total_credits),
        (NOTE, fee, NOTE + fee)
    );
    assert_eq!(quote.expected_duffs, None);
    assert_eq!(quote.grant.max_credits, NOTE + fee);
    assert!(fee > fee_of(NOTE, ShieldedFeeKind::Base));
    let own = run(&fx, &lib, |_, lib| async move {
        let to = platform_text(own_platform(2));
        pool::unshield_to_platform_quote(&*lib, NOTE, Some(&to)).await
    })
    .unwrap();
    assert_eq!(own, quote);

    let refused = run(&fx, &lib, |_, lib| async move {
        let lib = &*lib;
        let mainnet = own_platform(0).to_bech32m_string(Network::Mainnet);
        vec![
            pool::unshield_to_platform_quote(lib, 0, None).await,
            pool::unshield_to_platform_quote(lib, NOTE, Some(&foreign())).await,
            pool::unshield_to_platform_quote(lib, NOTE, Some(&mainnet)).await,
            pool::unshield_to_platform_quote(lib, NOTE, Some(&payee_text())).await,
            pool::unshield_to_platform_quote(lib, NOTE, Some(&core_address(3).to_string())).await,
            pool::unshield_to_platform_quote(lib, 2 * NOTE, None).await,
        ]
    });
    let refused: Vec<_> = refused.into_iter().map(Result::unwrap_err).collect();
    assert_eq!(
        refused[0],
        ShieldedError::BelowMinimum { min: 1, max: None }
    );
    assert_eq!(
        refused[1],
        ShieldedError::InvalidAddress {
            detail: "not this wallet's Platform address".into()
        }
    );
    for r in &refused[2..5] {
        assert_eq!(r.code(), "shielded.invalid_address", "{r:?}");
    }
    assert_eq!(refused[5].code(), "shielded.insufficient_funds");

    let below = Arc::new(FakeShielded::at(11, 11));
    let gated = run(&fx, &below, |_, lib| async move {
        code(pool::unshield_to_platform_quote(&*lib, NOTE, None).await)
    });
    assert_eq!(gated, "shielded.not_active");
}

/// The unshield pays the wallet's Platform receive address when given no
/// `to`, else the own address given, through the library's keyset
/// unshield, charged its quote.
#[test]
fn unshield_to_platform_pays_the_receive_address_or_an_own_one() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let fvk = fx.s.vault.shielded_viewing_key(&fx.wallet.0).unwrap();
    for (to, want) in [
        (None, own_platform(0)),
        (
            Some(format!(" {} ", platform_text(own_platform(2)))),
            own_platform(2),
        ),
    ] {
        let l = lease(&fx);
        let out = unshield(&fx, &lib, NOTE, to, &l, false).unwrap();
        assert_eq!(out.to, platform_text(want));
        assert_eq!(
            (out.credits, out.state, out.unresolved),
            (NOTE, ShieldedSpendState::Done, None)
        );
        assert_eq!(
            lib.spent().pop().unwrap(),
            Spent {
                operation: "unshield",
                to: platform_text(want),
                credits: NOTE,
                memo: Vec::new(),
                fvk,
            }
        );
        let fee = fee_of(NOTE, ShieldedFeeKind::Unshield);
        assert_eq!(fx.credits_spent(&l), NOTE + fee);
    }
}

/// C1-1: a `to` outside the wallet's payment account, or one that cannot
/// be checked, is refused before the grant is redeemed or a key leaves
/// the vault.
#[test]
fn unshield_to_platform_refuses_a_foreign_to() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    for to in [foreign(), payee_text(), core_address(3).to_string()] {
        let grant = fx.grant(10 * NOTE);
        let refused = unshield(&fx, &lib, NOTE, Some(to), &grant, false);
        assert_eq!(code(refused), "shielded.invalid_address");
        assert!(fx.unused(&grant), "the refusal keeps the grant");
    }
    *lib.platform.lock().unwrap() = None;
    for to in [None, Some(platform_text(own_platform(1)))] {
        let grant = fx.grant(10 * NOTE);
        let refused = unshield(&fx, &lib, NOTE, to, &grant, false);
        assert_eq!(code(refused), "storage");
        assert!(fx.unused(&grant));
    }
    assert!(lib.spent().is_empty(), "no key left the vault");
}

/// Off on mainnet: the quote and the call refuse `platform.feature_off`
/// before any read, grant or write; on elsewhere.
#[test]
fn unshield_to_platform_is_off_on_mainnet() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        network: Network::Mainnet,
        ..FakeShielded::default()
    });
    *lib.platform.lock().unwrap() = None;
    let quote = run(&fx, &lib, |_, lib| async move {
        pool::unshield_to_platform_quote(&*lib, NOTE, None).await
    });
    let ShieldedError::Platform(PlatformError::FeatureOff { feature }) = quote.unwrap_err() else {
        panic!("the quote is not feature_off");
    };
    assert_eq!(feature, pool::UNSHIELD_TO_PLATFORM);
    let grant = fx.grant(10 * NOTE);
    assert_eq!(
        code(unshield(&fx, &lib, NOTE, None, &grant, true)),
        "platform.feature_off"
    );
    assert!(fx.unused(&grant), "the grant is not redeemed");
    assert_eq!(lib.refreshes.load(Ordering::SeqCst), 0, "nothing quoted");
    assert!(lib.spent().is_empty(), "no key left the vault");
}

/// Off on mainnet through the facade, first: a mainnet session refuses
/// `platform.feature_off` before the wallet or its shielded store is
/// opened, so a wallet that cannot be opened (here one the session never
/// had) still reads `feature_off`, not `wallet_not_found`.
#[test]
fn the_facade_refuses_unshield_to_platform_on_mainnet_before_opening_the_pool() {
    use super::super::journal_tests::{Recorder, engine};
    use super::super::topup_tests::options;
    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Mainnet, options()))
        .unwrap();
    let dashpay = s.dashpay(crate::WalletId([0x5c; 32]));
    let quote = engine.block_on(dashpay.unshield_to_platform_quote(NOTE, None));
    assert_eq!(code(quote), "platform.feature_off");
    let call = engine.block_on(dashpay.unshield_to_platform(NOTE, None, "grant".into(), true));
    assert_eq!(code(call), "platform.feature_off");
    assert!(
        !s.data_dir().join(pool::SHIELDED_DB_FILE).exists(),
        "no shielded store was opened"
    );
}

/// Without the dispatch hook nothing is quoted, read or signed: never the
/// seed path, which the library would send unhooked.
#[test]
fn unshield_to_platform_refused_without_hook() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    lib.unhooked.store(true, Ordering::SeqCst);
    *lib.platform.lock().unwrap() = None;
    let grant = fx.grant(10 * NOTE);
    assert_eq!(
        code(unshield(&fx, &lib, NOTE, None, &grant, false)),
        "platform.unavailable"
    );
    assert_eq!(lib.refreshes.load(Ordering::SeqCst), 0, "nothing quoted");
    assert!(fx.unused(&grant));

    // Read again under the wallet's lock: gone by then, still refused.
    lib.unhooked.store(true, Ordering::SeqCst);
    *lib.hook_reads.lock().unwrap() = [true].into();
    *lib.platform.lock().unwrap() = Some(pool::PlatformPool {
        receive: own_platform(0),
        addresses: (0..3).map(own_platform).collect(),
    });
    let grant = fx.grant(10 * NOTE);
    assert_eq!(
        code(unshield(&fx, &lib, NOTE, None, &grant, false)),
        "platform.unavailable"
    );
    assert!(fx.unused(&grant));
    assert!(lib.spent().is_empty(), "no key left the vault");
}

/// A library that cannot spend from the key set refuses a move to Platform
/// as it refuses one without the hook: nothing quoted, the grant kept, no
/// key out of the vault.
#[test]
fn unshield_to_platform_refused_while_the_library_cannot_spend_from_the_key_set() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    lib.no_keyset_spends.store(true, Ordering::SeqCst);
    let grant = fx.grant(10 * NOTE);
    assert_eq!(
        code(unshield(&fx, &lib, NOTE, None, &grant, false)),
        "platform.unavailable"
    );
    assert_eq!(lib.refreshes.load(Ordering::SeqCst), 0, "nothing quoted");
    assert!(fx.unused(&grant));
    assert!(lib.spent().is_empty(), "no key left the vault");
}

/// The journal and `confirm_unrecorded` gates, as `unshield_to_core`'s: an
/// unread journal refuses, a marked unrecorded spend refuses until
/// confirmed, each before the grant.
#[test]
fn unshield_to_platform_waits_for_the_journal_and_a_confirmed_unrecorded_spend() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    lib.loses_record.store(true, Ordering::SeqCst);
    lib.fail_with(PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "unshield",
        reason: "proof fetch failed".into(),
    });
    let out = unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), false).unwrap();
    assert_eq!(
        (out.state, out.unresolved),
        (ShieldedSpendState::Pending, None)
    );
    let grant = fx.grant(10 * NOTE);
    assert_eq!(
        code(unshield(&fx, &lib, NOTE / 4, None, &grant, false)),
        "shielded.unrecorded_spend"
    );
    assert!(fx.unused(&grant));
    let spent = lib.spent().len();
    let out = unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), true).unwrap();
    assert_eq!(out.state, ShieldedSpendState::Done);
    assert_eq!(lib.spent().len(), spent + 1);

    fx.s.leases.load_journal(None, Vec::new(), Vec::new());
    assert!(!fx.s.leases.journal_ready());
    let grant = fx.grant(10 * NOTE);
    assert_eq!(
        code(unshield(&fx, &lib, NOTE / 4, None, &grant, true)),
        "platform.unavailable"
    );
    assert!(fx.unused(&grant));
}

/// A submit that fails after signing is pending whenever the library still
/// holds the spend (its signed transition armed, or its notes), so no host
/// offers it again; an unconfirmed broadcast names the library's record.
/// A refusal before anything was sent, and Platform's pool floor, are
/// errors, with the quote charged once the keys were out.
#[test]
fn unshield_to_platform_failures_after_signing() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let charged = NOTE / 4 + fee_of(NOTE / 4, ShieldedFeeKind::Unshield);

    lib.fail_with(PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "unshield",
        reason: "proof fetch failed".into(),
    });
    let l = lease(&fx);
    let out = unshield(&fx, &lib, NOTE / 4, None, &l, false).unwrap();
    assert_eq!(
        (out.state, out.unresolved),
        (ShieldedSpendState::Pending, Some(unresolved(0)))
    );
    assert_eq!(fx.credits_spent(&l), charged);

    *lib.after_failure.lock().unwrap() = AfterFailure::Armed;
    lib.fail_with(PlatformWalletError::ShieldedBroadcastFailed(
        "rejected".into(),
    ));
    let out = unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), false).unwrap();
    assert_eq!(
        (out.state, out.unresolved),
        (ShieldedSpendState::Pending, Some(unresolved(1)))
    );

    *lib.after_failure.lock().unwrap() = AfterFailure::Held;
    lib.fail_with(PlatformWalletError::ShieldedDispatchRefused {
        reason: "the plan check failed".into(),
    });
    let out = unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), false).unwrap();
    assert_eq!(
        (out.state, out.unresolved),
        (ShieldedSpendState::Pending, None)
    );

    *lib.after_failure.lock().unwrap() = AfterFailure::Free;
    lib.fail_with(PlatformWalletError::ShieldedDispatchRefused {
        reason: "the plan check failed".into(),
    });
    assert_eq!(
        code(unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), false)),
        "shielded.spend_refused"
    );
    let pool_text = "state transition broadcast failed: Shielded pool has insufficient notes \
                     for outgoing transition: pool has 3 notes but minimum 250 required";
    lib.fail_with(PlatformWalletError::ShieldedBroadcastFailed(
        pool_text.into(),
    ));
    let l = lease(&fx);
    assert_eq!(
        unshield(&fx, &lib, NOTE / 4, None, &l, false).unwrap_err(),
        ShieldedError::PoolTooSmall {
            notes: 3,
            required: 250,
        }
    );
    assert_eq!(fx.credits_spent(&l), charged);

    *lib.after_failure.lock().unwrap() = AfterFailure::Unreadable;
    lib.fail_with(PlatformWalletError::ShieldedBroadcastFailed(
        "rejected".into(),
    ));
    match unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), false) {
        Err(ShieldedError::Platform(PlatformError::BroadcastUnknown { artifact })) => {
            assert_eq!(artifact, "shielded unshield")
        }
        other => panic!("not broadcast_unknown: {other:?}"),
    }
}

/// A double submit: two unshields under one grant. The wallet's lock runs
/// them one at a time and the second finds the grant already redeemed,
/// `platform.grant_invalid` with no key out, so the library sends one.
#[test]
fn unshield_to_platform_double_submit_sends_once() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let grant = fx.grant(NOTE + fee_of(NOTE, ShieldedFeeKind::Unshield));
    let (s, l, lib2) = (Arc::clone(&fx.s), grant.clone(), Arc::clone(&lib));
    let results = fx.engine.block_on(async move {
        let call = || {
            let (s, lib, l) = (Arc::clone(&s), Arc::clone(&lib2), l.clone());
            tokio::spawn(async move {
                pool::unshield_to_platform(&s, wallet, &*lib, NOTE, None, &l, false).await
            })
        };
        let (a, b) = (call(), call());
        [a.await.unwrap(), b.await.unwrap()]
    });
    let done = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(done, 1, "{results:?}");
    let refused = results.into_iter().find_map(Result::err).unwrap();
    assert_eq!(refused.code(), "platform.grant_invalid");
    assert_eq!(lib.spent().len(), 1, "the library sent one unshield");
}

/// The library's call runs with the wallet's unshield intent standing (the
/// address and the quoted credits the hook checks the signed transition
/// against); the record row the hook notes there is the outcome's, and the
/// intent is gone once the call returns.
#[test]
fn unshield_to_platform_reports_the_record_its_hook_armed() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let (s, wallet) = (Arc::clone(&fx.s), fx.wallet);
    *lib.probe.lock().unwrap() = Some(Box::new(move || {
        let mut intents = s.platform.unshield_intents.lock().unwrap();
        let Some(intent) = intents.get_mut(&wallet) else {
            return false;
        };
        let seen = (intent.to, intent.credits) == (own_platform(2), NOTE);
        intent.record = Some("from-shielded-ab".into());
        seen
    }));
    let to = Some(platform_text(own_platform(2)));
    let out = unshield(&fx, &lib, NOTE, to, &lease(&fx), false).unwrap();
    assert_eq!(lib.probed.lock().unwrap().as_slice(), [true]);
    assert_eq!(out.record.as_deref(), Some("from-shielded-ab"));
    assert!(fx.s.platform.unshield_intents.lock().unwrap().is_empty());

    // A send stands no intent; a failed unshield leaves none behind.
    let l = lease(&fx);
    let out = run(&fx, &lib, move |s, lib| async move {
        pool::send(
            &s,
            wallet,
            &*lib,
            (&payee_text(), NOTE / 4),
            None,
            &l,
            false,
        )
        .await
    });
    assert!(out.is_ok());
    assert_eq!(lib.probed.lock().unwrap().as_slice(), [true, false]);
    lib.fail_with(PlatformWalletError::ShieldedDispatchRefused {
        reason: "the plan check failed".into(),
    });
    assert!(unshield(&fx, &lib, NOTE / 4, None, &lease(&fx), false).is_err());
    assert!(fx.s.platform.unshield_intents.lock().unwrap().is_empty());
}
