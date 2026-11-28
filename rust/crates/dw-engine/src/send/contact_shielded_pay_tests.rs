//! C-funding D-C5's Shielded source (C5a) over the contact fixture and a
//! scripted library whose withdrawals pass the engine's hook steps: the
//! payout unshields to the contact's reserved address, its row names the
//! spend's transition before the spend's artifact exists, the journal's
//! disposition alone settles the contact's lock, a kill at each step
//! relaunches into the right state, and a pool-floor refusal holds the
//! contact with its address consumed.

use dpp::version::PlatformVersion;
use dw_appdb::DpContactPayoutRow;
use dw_appdb::dispatch::ArtifactStatus;
use platform_wallet::PlatformWalletError;

use super::*;
use crate::platform::payment_locks::PayCall;
use crate::platform::shielded_payout;
use crate::platform::shielded_tests::FakeShielded;
use crate::platform::withdrawal::Rules;
use crate::platform::{ContactPayOutcome, PaymentSource, PayoutState};

/// What the scripted library's notes hold, in credits.
const SHIELDED: u64 = 600_000_000;

fn credits() -> u64 {
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    rules.min.max(SHIELDED / 4)
}

fn code(e: &ContactError) -> &'static str {
    e.code()
}

impl Fixture {
    fn hooked(&self) -> Arc<FakeShielded> {
        Arc::new(FakeShielded::hooked_to(&self.session, self.wallet))
    }

    fn credits_grant(&self) -> String {
        self.grant(GrantPurpose::PlatformOp {
            max_duffs: 0,
            max_credits: SHIELDED,
        })
    }

    fn call(&self, to: u8, credits: u64, grant: &str, confirm: bool) -> PayCall {
        PayCall {
            identity: b58(ME),
            contact: b58(to),
            credits,
            source: PaymentSource::Shielded,
            note: Some("lunch".into()),
            grant: grant.into(),
            confirm_unrecorded: confirm,
        }
    }

    /// `contact_pay` from the Shielded balance through `lib`, in a task of
    /// the session's runtime.
    fn spawn_shielded(
        &self,
        lib: &Arc<FakeShielded>,
        call: PayCall,
    ) -> tokio::task::JoinHandle<Result<ContactPayOutcome, ContactError>> {
        let (s, w, lib) = (Arc::clone(&self.session), self.wallet, Arc::clone(lib));
        (self.session.rt).spawn(async move { shielded_payout::pay(&s, w, &*lib, call).await })
    }

    fn shielded(
        &self,
        lib: &Arc<FakeShielded>,
        to: u8,
        grant: &str,
    ) -> Result<ContactPayOutcome, ContactError> {
        let call = self.call(to, credits(), grant, false);
        self.engine
            .block_on(self.spawn_shielded(lib, call))
            .unwrap()
    }

    /// The wallet's payout rows, as (reference, state, st_hash, address).
    fn payout_rows(&self) -> Vec<(String, String, Option<String>, String)> {
        let w = self.wallet.to_string();
        let rows = (self.engine)
            .block_on(self.session.appdb_op(move |db| db.dp_contact_payouts(&w)))
            .unwrap();
        (rows.into_iter())
            .map(|r| (r.reference, r.state, r.st_hash, r.address))
            .collect()
    }

    fn has_artifact(&self, st_hash: [u8; 32]) -> bool {
        let (s, w) = (Arc::clone(&self.session), self.wallet);
        (self.engine)
            .block_on(async move { s.leases.has_shielded_artifact(w, st_hash).await })
            .unwrap()
    }

    /// The journal settles the artifact of `st_hash` as `status`, as a
    /// pass's read of the library's disposition does; `acked` is the
    /// user's acknowledgement in Repair.
    fn settle_artifact(&self, st_hash: [u8; 32], status: ArtifactStatus, acked: bool) {
        let (s, w) = (Arc::clone(&self.session), self.wallet);
        let disposition = match status {
            ArtifactStatus::Executed => Some(("executed", None)),
            ArtifactStatus::NotExecuted => Some(("not_executed", Some("pruned"))),
            _ => None,
        };
        (self.engine)
            .block_on(async move {
                (s.leases)
                    .settle_shielded_artifact(w, st_hash, status, disposition, acked)
                    .await
            })
            .unwrap();
    }

    /// The journal settles the artifact of `st_hash` as `status` with the
    /// library's `disposition` as stored.
    fn settle_artifact_as(
        &self,
        st_hash: [u8; 32],
        status: ArtifactStatus,
        disposition: (&'static str, Option<&'static str>),
    ) {
        let (s, w) = (Arc::clone(&self.session), self.wallet);
        (self.engine)
            .block_on(async move {
                (s.leases)
                    .settle_shielded_artifact(w, st_hash, status, Some(disposition), false)
                    .await
            })
            .unwrap();
    }

    /// Whether the hook recorded the dispatch of payout `reference`.
    fn dispatched(&self, reference: &str) -> bool {
        let scope = dw_appdb::local_scope(&self.wallet.to_string());
        let key = dw_appdb::dp_payout_dispatched(reference);
        (self.engine)
            .block_on(self.session.appdb_op(move |db| db.setting(&scope, &key)))
            .unwrap()
            .is_some()
    }

    /// What a shielded pass's tail settles of the wallet's payouts.
    fn after_pass(&self) {
        (self.engine).block_on(self.session.settle_shielded_payouts(self.wallet));
    }

    fn restart_payments(&self) {
        self.session.platform.payments.restart();
        self.engine
            .block_on(self.session.load_payment_locks(self.wallet));
    }

    fn grant_kept(&self, grant: &str) -> bool {
        (self.session.vault())
            .check_grant(grant, dw_vault::GrantKind::PlatformOp, Some(&self.wallet.0))
            .is_ok()
    }
}

/// P2–P4, P7: the quote is the unshield's; the payout unshields the typed
/// amount to the contact's reserved DIP-15 address, its row names the
/// spend's transition, and a `Done` spend is sent: the lock clears and
/// the address is consumed, so the next payment reserves a new one.
#[test]
fn a_shielded_payout_unshields_to_the_contacts_reserved_address() {
    let f = fixture(false);
    let lib = f.hooked();
    let quote = (f.engine)
        .block_on(shielded_payout::quote(&*lib, credits()))
        .unwrap();
    assert_eq!(quote.source, PaymentSource::Shielded);
    assert_eq!(quote.credits, credits());
    assert!(quote.expected_duffs > 0 && quote.fee_credits > 0);
    assert_eq!(quote.grant.max_duffs, 0);
    assert_eq!(quote.grant.max_credits, credits() + quote.fee_credits);

    let grant = f.credits_grant();
    let paid = f.shielded(&lib, BOB, &grant).unwrap();
    assert_eq!(paid.state, PayoutState::Sent);
    assert_eq!(lib.withdrawn_to(), [f.contact_address(BOB, 0).to_string()]);
    let rows = f.payout_rows();
    assert_eq!(
        rows,
        [(
            paid.reference.clone(),
            "sent".to_string(),
            Some(hex::encode(FakeShielded::st_hash(0))),
            f.contact_address(BOB, 0).to_string(),
        )]
    );
    assert!(f.lock(BOB).is_none());
    assert!(f.reserved(BOB).is_none(), "the address is consumed");
    assert!(!f.grant_kept(&grant));

    let next = f.shielded(&lib, BOB, &f.credits_grant()).unwrap();
    assert_eq!(next.state, PayoutState::Sent);
    assert_ne!(next.reference, paid.reference);
    assert_eq!(
        lib.withdrawn_to().last(),
        Some(&f.contact_address(BOB, 1).to_string()),
        "never the same address twice"
    );
}

/// P9 and the double submit: an unconfirmed spend is pending and holds the
/// contact against a second payment from every source, the same grant
/// again included; the Repair acknowledgement settles nothing; only the
/// journal's disposition clears the lock. Mutant: clear the lock on the
/// acknowledgement.
#[test]
fn an_unconfirmed_payout_holds_the_contact_until_the_journal_settles_it() {
    let f = fixture(false);
    let lib = f.hooked();
    lib.fail_with(PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "withdraw",
        reason: "timeout".into(),
    });
    let grant = f.credits_grant();
    let paid = f.shielded(&lib, BOB, &grant).unwrap();
    assert_eq!(paid.state, PayoutState::Pending);
    let lock = f.lock(BOB).expect("the payout locks Bob");
    assert_eq!(
        (lock.reference.as_str(), lock.source),
        (paid.reference.as_str(), PaymentSource::Shielded)
    );
    assert!(f.reserved(BOB).is_none(), "the address is consumed");

    let again = f.shielded(&lib, BOB, &grant).unwrap_err();
    assert!(matches!(again, ContactError::PaymentLocked { ref txid } if *txid == paid.reference));
    let fresh = f.shielded(&lib, BOB, &f.credits_grant()).unwrap_err();
    assert_eq!(code(&fresh), "contact.payment_locked");
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    let core = d.set_contact_recipient(f.pay(BOB, AMOUNT));
    assert!(matches!(
        contact_error(core),
        ContactError::PaymentLocked { .. }
    ));
    assert_eq!(lib.withdrawn_to().len(), 1, "paid once");

    let hash = FakeShielded::st_hash(0);
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    f.settle_artifact(hash, ArtifactStatus::Unresolved { unrecorded: false }, true);
    f.after_pass();
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    assert!(f.lock(BOB).is_some(), "an acknowledgement is not evidence");

    f.settle_artifact(hash, ArtifactStatus::Executed, false);
    f.after_pass();
    assert!(f.lock(BOB).is_none(), "executed: sent");
    assert_eq!(f.payout_rows()[0].1, "sent");
}

/// C5a: a submit refused after its signature, by the hook (its artifact
/// was not written), sent nothing: not sent, the lock clears and the
/// address goes back to the pair, which the next payment pays.
#[test]
fn a_spend_the_hook_refused_returns_the_address() {
    let f = fixture(false);
    let lib = f.hooked();
    lib.drop_artifacts();
    let refused = f.shielded(&lib, BOB, &f.credits_grant()).unwrap_err();
    assert_eq!(code(&refused), "shielded.spend_refused");
    let rows = f.payout_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "not_sent");
    assert_eq!(
        rows[0].2,
        Some(hex::encode(FakeShielded::st_hash(0))),
        "the row named it before the artifact write failed"
    );
    assert!(!f.has_artifact(FakeShielded::st_hash(0)));
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.reserved(BOB), Some(f.contact_address(BOB, 0)));
    assert!(lib.withdrawn_to().is_empty(), "the library sent nothing");
}

/// C5a, P5: a pool-floor refusal of a hooked spend, as the pinned library
/// leaves it (its notes held, `Unresolved(Cancelled)`), is pending: the
/// lock stays and the address is consumed; a Core payment to the contact
/// is refused meanwhile, and a relaunch keeps it. Only the journal's
/// definitely unsent, read by a pass, clears the lock, and the address is
/// never reused. Mutant: give the address back on the refusal.
#[test]
fn a_pool_floor_refusal_holds_the_contact_with_its_address_consumed() {
    let f = fixture(false);
    let lib = f.hooked();
    lib.hold_failed();
    lib.fail_with(PlatformWalletError::ShieldedBroadcastFailed(
        "state transition broadcast failed: Shielded pool has insufficient notes for \
         outgoing transition: pool has 3 notes but minimum 250 required"
            .into(),
    ));
    let floor = f.shielded(&lib, BOB, &f.credits_grant()).unwrap();
    assert_eq!(floor.state, PayoutState::Pending);
    assert!(f.has_artifact(FakeShielded::st_hash(0)));
    assert_eq!(
        f.lock(BOB).map(|l| (l.source, l.reference)),
        Some((PaymentSource::Shielded, floor.reference.clone()))
    );
    assert!(f.reserved(BOB).is_none(), "consumed, not given back");
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    assert!(matches!(
        contact_error(d.set_contact_recipient(f.pay(BOB, AMOUNT))),
        ContactError::PaymentLocked { .. }
    ));
    f.after_pass();
    assert!(
        f.lock(BOB).is_some(),
        "an unresolved artifact settles nothing"
    );
    f.restart_payments();
    assert!(f.lock(BOB).is_some(), "a relaunch keeps it");

    f.settle_artifact(FakeShielded::st_hash(0), ArtifactStatus::NotExecuted, true);
    f.after_pass();
    assert!(f.lock(BOB).is_none());
    let rows = f.payout_rows();
    let refused = rows.iter().find(|r| r.0 == floor.reference).unwrap();
    assert_eq!(refused.1, "not_sent");
    assert!(f.reserved(BOB).is_none(), "still consumed");
    let next = f.shielded(&lib, BOB, &f.credits_grant()).unwrap();
    assert_eq!(next.state, PayoutState::Sent);
    assert_ne!(lib.withdrawn_to().last(), Some(&refused.3), "a new address");
}

/// C5a: `shielded.unrecorded_spend` refuses unless confirmed, and the
/// withdrawal range refuses, before the grant is redeemed or an address
/// taken.
#[test]
fn the_shielded_gates_refuse_before_the_grant_and_the_address() {
    let f = fixture(false);
    let lib = f.hooked();
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    let grant = f.credits_grant();
    let pay = |credits, confirm| {
        let call = f.call(BOB, credits, &grant, confirm);
        f.engine.block_on(f.spawn_shielded(&lib, call)).unwrap()
    };
    assert_eq!(
        code(&pay(rules.min - 1, false).unwrap_err()),
        "shielded.below_minimum"
    );
    assert_eq!(
        code(&pay(rules.max + 1, false).unwrap_err()),
        "invalid_argument"
    );
    let (s, w) = (Arc::clone(&f.session), f.wallet);
    (f.engine)
        .block_on(async move {
            (s.leases)
                .record_unrecorded_spend(w, [0xcc; 32], [7; 32], vec![[0xcd; 32]])
                .await
        })
        .unwrap();
    assert_eq!(
        code(&pay(credits(), false).unwrap_err()),
        "shielded.unrecorded_spend"
    );
    assert!(f.grant_kept(&grant));
    assert!(f.reserved(BOB).is_none(), "no address taken");
    assert!(f.payout_rows().is_empty());
    assert!(lib.withdrawn_to().is_empty());
    assert_eq!(pay(credits(), true).unwrap().state, PayoutState::Sent);
}

/// P15: a `ContactPayment` lease (a Core spend's) is refused by a
/// Shielded payout, by the unshield's credits cap, before an address is
/// taken; a `ContactPayFromBalance` lease from Shielded pays, and carries
/// no Core spend signer.
#[test]
fn a_payout_takes_only_its_own_flows_lease() {
    let f = fixture(false);
    let lib = f.hooked();
    let begin = |flow, grants: Vec<String>, source| {
        let s = Arc::clone(&f.session);
        f.engine
            .block_on(async move { s.begin_flow(f.wallet, flow, grants, source).await })
    };
    let core = begin(FlowKind::ContactPayment, vec![f.spend(COIN)], None).unwrap();
    assert_eq!(
        code(&f.shielded(&lib, BOB, &core).unwrap_err()),
        "platform.grant_exceeded"
    );
    assert!(lib.withdrawn_to().is_empty());
    assert!(f.reserved(BOB).is_none());
    let lease = begin(
        FlowKind::ContactPayFromBalance,
        vec![f.credits_grant()],
        Some(PaymentSource::Shielded),
    )
    .unwrap();
    let held = f.session.lease_for(&f.wallet, &lease).unwrap().unwrap();
    assert!(!held.has_spend(), "no Core spend signer");
    assert_eq!(
        f.shielded(&lib, BOB, &lease).unwrap().state,
        PayoutState::Sent
    );
    let fvk = f.session.vault().shielded_viewing_key(&f.wallet.0).unwrap();
    assert_eq!(lib.spent_fvks(), [fvk], "the wallet's shielded spend key");
}

/// A Platform payout's flow lease holds no shielded spend key: a Shielded
/// payout under it fails closed before anything is signed, the row
/// settles not sent and the address goes back with the pair.
#[test]
fn a_platform_flow_lease_fails_closed_at_the_shielded_spend_key() {
    let f = fixture(false);
    let lib = f.hooked();
    let s = Arc::clone(&f.session);
    let grants = vec![f.credits_grant()];
    let lease = (f.engine)
        .block_on(async move {
            let flow = FlowKind::ContactPayFromBalance;
            (s.begin_flow(f.wallet, flow, grants, Some(PaymentSource::Platform))).await
        })
        .unwrap();
    let refused = f.shielded(&lib, BOB, &lease).unwrap_err();
    assert_eq!(code(&refused), "platform.needs_grant", "{refused:?}");
    assert!(lib.spent_fvks().is_empty(), "nothing signed");
    let rows = f.payout_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, "not_sent");
    assert!(f.lock(BOB).is_none());
    assert_eq!(
        f.reserved(BOB).map(|a| a.to_string()),
        Some(rows[0].3.clone())
    );
}

/// The kill matrix (C5a): the call killed at each step relaunches into
/// the state its records prove. Before the row names the spend, and after
/// that but before the artifact, nothing was sent: the relaunch clears the
/// row as not sent, unlocks the contact and gives its address back. After
/// the artifact and the library's send, the lock stays and the journal
/// settles it. Mutant: clear a row whose `st_hash` has an artifact.
#[test]
fn a_kill_at_each_step_relaunches_into_what_the_records_prove() {
    let f = fixture(false);
    let lib = f.hooked();
    // The row each killed call wrote: the one not seen before it.
    let mut seen: Vec<String> = Vec::new();
    let mut kill_at = |pause: &Arc<crate::coins::TestPause>| {
        let task = f.spawn_shielded(&lib, f.call(BOB, credits(), &f.credits_grant(), false));
        assert!(pause.wait_reached(Duration::from_secs(60)), "the pause");
        task.abort();
        let _ = f.engine.block_on(task);
        f.restart_payments();
        let rows = f.payout_rows();
        let row = (rows.into_iter())
            .find(|r| !seen.contains(&r.0))
            .expect("the killed call's row");
        seen.push(row.0.clone());
        row
    };
    let reserved = |row: &(String, String, Option<String>, String)| {
        f.reserved(BOB).map(|a| a.to_string()) == Some(row.3.clone())
    };

    // Before the hook's `st_hash` write.
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.platform.payments.hold_st_hash.lock().unwrap() = Some(Arc::clone(&pause));
    let row = kill_at(&pause);
    assert_eq!((row.1.as_str(), row.2.as_deref()), ("not_sent", None));
    assert!(f.lock(BOB).is_none());
    assert!(reserved(&row), "the address went back to the pair");

    // After the `st_hash` write, before the artifact.
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.leases.hold_artifact_write.lock().unwrap() = Some(Arc::clone(&pause));
    let row = kill_at(&pause);
    assert_eq!(row.1, "not_sent");
    assert_eq!(row.2, Some(hex::encode(FakeShielded::st_hash(0))));
    assert!(f.lock(BOB).is_none());
    assert!(reserved(&row), "the address went back to the pair");
    assert!(lib.withdrawn_to().is_empty(), "nothing was sent");
    assert!(!f.dispatched(&row.0), "the hook never let it go");

    // After the artifact and the send, before the call settled.
    let pause = Arc::new(crate::coins::TestPause::default());
    (f.session.platform.payments).pause_settle(Arc::clone(&pause));
    let row = kill_at(&pause);
    assert_eq!(row.1, "pending");
    assert!(f.dispatched(&row.0), "recorded before the library sent it");
    let hash = FakeShielded::st_hash(0);
    assert_eq!(row.2, Some(hex::encode(hash)));
    assert_eq!(f.lock(BOB).map(|l| l.reference), Some(row.0.clone()));
    assert!(f.reserved(BOB).is_none(), "the address may have been paid");
    assert_eq!(lib.withdrawn_to(), std::slice::from_ref(&row.3));
    f.settle_artifact(hash, ArtifactStatus::Executed, true);
    f.restart_payments();
    assert!(f.lock(BOB).is_none());
    let rows = f.payout_rows();
    let settled = rows.iter().find(|r| r.0 == row.0).unwrap();
    assert_eq!(settled.1, "sent");
}

/// Mainnet: off (flag `shielded_payout::FEATURE`). On mainnet a payment
/// from the Shielded balance refuses as every network did before this
/// slice: `begin_flow` and the payout itself are
/// `platform.feature_off{feature: shielded_contact_payment}`, and the
/// facade takes the old refusing path. Nothing is redeemed, reserved,
/// written or sent. Testnet, devnet and regtest pay.
#[test]
fn mainnet_refuses_a_shielded_payout_before_anything_is_written() {
    use crate::platform::shielded_payout::{FEATURE, enabled_on};
    assert!(!enabled_on(&DashNetwork::Mainnet));
    assert!(enabled_on(&DashNetwork::Testnet));
    assert!(enabled_on(&DashNetwork::Regtest));
    assert!(enabled_on(&DashNetwork::Devnet { name: "d".into() }));

    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Mainnet,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform: true,
                ..Default::default()
            },
        ))
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    engine
        .block_on(session.adopt_signing_window_default())
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    let purpose = GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: SHIELDED,
    };
    let grant = |s: &Arc<NetworkSession>| {
        (s.vault()
            .authorize(purpose, Some(&wallet.0), Credential::None))
        .unwrap()
        .id
    };
    let kept = |g: &str| {
        (session.vault())
            .check_grant(g, dw_vault::GrantKind::PlatformOp, Some(&wallet.0))
            .is_ok()
    };
    let off = |e: &ContactError| matches!(e, ContactError::Platform(PlatformError::FeatureOff { feature }) if feature == FEATURE);

    let g = grant(&session);
    let s = Arc::clone(&session);
    let begun = engine.block_on(async move {
        let flow = FlowKind::ContactPayFromBalance;
        s.begin_flow(wallet, flow, vec![g], Some(PaymentSource::Shielded))
            .await
    });
    assert!(
        matches!(&begun, Err(PlatformError::FeatureOff { feature }) if feature == FEATURE),
        "{begun:?}"
    );

    let lib = Arc::new(FakeShielded::hooked_to(&session, wallet));
    let g = grant(&session);
    let call = PayCall {
        identity: b58(ME),
        contact: b58(BOB),
        credits: credits(),
        source: PaymentSource::Shielded,
        note: None,
        grant: g.clone(),
        confirm_unrecorded: true,
    };
    let (s, l) = (Arc::clone(&session), Arc::clone(&lib));
    let paid = engine.block_on(async move {
        let pair = (id(ME), id(BOB));
        shielded_payout::pay_gated(&s, wallet, &*l, pair, call).await
    });
    assert!(paid.as_ref().is_err_and(off), "{paid:?}");
    assert!(kept(&g), "the grant is kept");

    let g = grant(&session);
    let dashpay = session.dashpay(wallet);
    let (quoted, paid) = engine.block_on(async {
        let quoted =
            (dashpay.contact_pay_quote(b58(ME), b58(BOB), credits(), PaymentSource::Shielded))
                .await;
        let paid = dashpay
            .contact_pay(
                b58(ME),
                b58(BOB),
                credits(),
                PaymentSource::Shielded,
                None,
                g.clone(),
                true,
            )
            .await;
        (quoted, paid)
    });
    assert!(quoted.is_err(), "{quoted:?}");
    assert!(paid.is_err(), "{paid:?}");
    assert!(kept(&g), "the grant is kept");

    let w = wallet.to_string();
    let rows = engine
        .block_on(session.appdb_op(move |db| db.dp_contact_payouts(&w)))
        .unwrap();
    assert!(rows.is_empty(), "no payout row");
    let pair = (id(ME), id(BOB));
    assert!(
        session
            .platform
            .payments
            .reserved_checked(wallet, pair)
            .is_none()
    );
    assert!(lib.withdrawn_to().is_empty(), "nothing sent");
}

/// While the library cannot spend from the key set, a Shielded contact
/// quote and payment refuse `platform.unavailable` after the payee's gates
/// and before the pool opens, so a pass holding the note store delays
/// neither; a stranger is still refused as a stranger, and the grant is
/// kept.
#[test]
fn a_shielded_contact_payment_refuses_after_the_payee_while_spends_are_off() {
    let f = fixture(false);
    let dashpay = f.session.dashpay(f.wallet);
    let grant = f.credits_grant();
    let refusals = |to: u8| {
        let quote = dashpay.contact_pay_quote(b58(ME), b58(to), credits(), PaymentSource::Shielded);
        let call = f.call(to, credits(), &grant, true);
        let pay = dashpay.contact_pay(
            call.identity,
            call.contact,
            call.credits,
            call.source,
            call.note,
            call.grant,
            call.confirm_unrecorded,
        );
        let quoted = f.engine.block_on(quote).unwrap_err();
        (quoted, f.engine.block_on(pay).unwrap_err())
    };
    let spends_on = refusals(DAVE);
    let off = &f.session.keyset_spends_off;
    off.store(true, std::sync::atomic::Ordering::SeqCst);
    let stranger = refusals(DAVE);
    assert_eq!(
        (code(&stranger.0), code(&stranger.1)),
        (code(&spends_on.0), code(&spends_on.1)),
        "the payee's gate first"
    );
    assert_ne!(code(&stranger.0), "platform.unavailable");
    assert_ne!(code(&stranger.1), "platform.unavailable");
    let call = f.call(BOB, credits(), &grant, true);
    let manager = f.session.manager().unwrap();
    let held = f.session.dashpay(f.wallet);
    let s = Arc::clone(&f.session);
    let (quoted, paid) = f.engine.block_on(async move {
        let coordinator = (crate::platform::shielded_pool::coordinator(&s, &manager, None))
            .await
            .unwrap();
        let _pass = coordinator.store().write().await;
        let wait = std::time::Duration::from_secs(10);
        let quote = held.contact_pay_quote(b58(ME), b58(BOB), credits(), PaymentSource::Shielded);
        let quoted =
            (tokio::time::timeout(wait, quote).await).expect("the quote waits for no pass");
        let pay = held.contact_pay(
            call.identity,
            call.contact,
            call.credits,
            call.source,
            call.note,
            call.grant,
            call.confirm_unrecorded,
        );
        let paid = (tokio::time::timeout(wait, pay).await).expect("the payment waits for no pass");
        (quoted, paid)
    });
    assert_eq!(code(&quoted.unwrap_err()), "platform.unavailable");
    assert_eq!(code(&paid.unwrap_err()), "platform.unavailable");
    assert!(f.grant_kept(&grant), "the grant is kept");
    assert!(f.payout_rows().is_empty(), "no payout row");

    let txid = Txid::from_byte_array([4; 32]);
    let pair = (id(ME), id(BOB));
    (f.engine).block_on(f.session.lock_payment(f.wallet, pair, txid, vec![]));
    let locked = |e: ContactError| match e {
        ContactError::PaymentLocked { txid } => txid,
        other => panic!("expected contact.payment_locked, got {other:?}"),
    };
    let (quoted, paid) = refusals(BOB);
    assert_eq!(locked(quoted), txid.to_string(), "the pair's lock first");
    assert_eq!(locked(paid), txid.to_string(), "the pair's lock first");
}

/// A load that settles a payout an earlier load locked drops that lock
/// from memory: a second bring-up without a restart leaves no stale lock.
#[test]
fn a_second_load_clears_a_payout_the_journal_settled() {
    let f = fixture(false);
    let lib = f.hooked();
    lib.fail_with(PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "withdraw",
        reason: "timeout".into(),
    });
    let paid = f.shielded(&lib, BOB, &f.credits_grant()).unwrap();
    f.restart_payments();
    assert_eq!(f.lock(BOB).map(|l| l.reference), Some(paid.reference));
    f.settle_artifact(FakeShielded::st_hash(0), ArtifactStatus::Executed, false);
    (f.engine).block_on(f.session.load_payment_locks(f.wallet));
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_rows()[0].1, "sent");
}

/// Two settlements of one payout: only the one that wrote its row gives
/// its address back. Between them the address went to another payment,
/// which the second must not hand out again.
#[test]
fn a_payout_settled_twice_gives_its_address_back_once() {
    let f = fixture(false);
    let pair = (id(ME), id(BOB));
    let address = f.contact_address(BOB, 0);
    let row = DpContactPayoutRow {
        reference: "sh-twice".into(),
        identity: b58(ME),
        contact: b58(BOB),
        source: "shielded".into(),
        st_hash: None,
        address: address.to_string(),
        credits: credits(),
        expected_duffs: 1,
        note: None,
        state: "pending".into(),
        since: 1,
    };
    let s = &f.session;
    f.engine
        .block_on(s.record_payout(f.wallet, pair, row))
        .unwrap();
    assert!(f.lock(BOB).is_some(), "the row is the lock");
    let settle = || {
        let key = (PaymentSource::Shielded, "sh-twice");
        let back = Some(address.clone());
        (f.engine).block_on(s.settle_payout(f.wallet, pair, key, PayoutState::NotSent, back))
    };
    assert!(settle());
    assert_eq!(f.reserved(BOB), Some(address.clone()));
    let claimed = s.claim_payment(f.wallet, pair, "next", &address);
    assert!(claimed.is_ok());
    assert!(!settle(), "settled already");
    assert!(f.reserved(BOB).is_none(), "not handed out again");
    assert!(f.lock(BOB).is_none());
}

/// A `begin_flow(ContactPayFromBalance, shielded)` lease with room for
/// two payouts pays one: a repeated call with it is refused before an
/// address, however much credit the lease has left.
#[test]
fn a_flow_lease_pays_one_payout() {
    let f = fixture(false);
    let lib = f.hooked();
    let s = Arc::clone(&f.session);
    let grants = vec![f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10 * SHIELDED,
    })];
    let lease = (f.engine)
        .block_on(async move {
            let flow = FlowKind::ContactPayFromBalance;
            (s.begin_flow(f.wallet, flow, grants, Some(PaymentSource::Shielded))).await
        })
        .unwrap();
    let pay = || {
        f.engine
            .block_on(f.spawn_shielded(&lib, f.call(BOB, credits(), &lease, false)))
    };
    let first = pay().unwrap();
    assert!(first.is_ok(), "{first:?}");
    let second = pay().unwrap();
    assert_eq!(
        code(&second.unwrap_err()),
        "platform.lease_expired",
        "the lease ended"
    );
    assert_eq!(lib.withdrawn_to().len(), 1, "paid once");
    assert_eq!(f.payout_rows().len(), 1);
    assert!(f.reserved(BOB).is_none(), "no second address reserved");
}

#[test]
fn two_payouts_racing_on_one_flow_lease_pay_once() {
    let f = fixture(false);
    let lib = f.hooked();
    let grants = vec![f.grant(GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: 10 * SHIELDED,
    })];
    let lease = f
        .engine
        .block_on(f.session.begin_flow(
            f.wallet,
            FlowKind::ContactPayFromBalance,
            grants,
            Some(PaymentSource::Shielded),
        ))
        .unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session
        .platform
        .payments
        .hold_shielded_lease
        .lock()
        .unwrap() = Some(Arc::clone(&pause));
    let call = || f.call(BOB, credits(), &lease, false);
    let first = f.spawn_shielded(&lib, call());
    assert!(pause.wait_reached(Duration::from_secs(60)));

    let (first, second) = f.engine.block_on(async {
        let call = call();
        let pair = f.session.pay_gates(f.wallet, &call).await.unwrap();
        let second = shielded_payout::pay_gated(&f.session, f.wallet, &*lib, pair, call);
        tokio::pin!(second);
        // FakeShielded's hook check is ready, so the second call waits on the wallet's spend lock.
        let waiting = futures_util::poll!(second.as_mut()).is_pending();
        pause.release.notify_one();
        assert!(
            waiting,
            "the second payout must wait while the first holds the lease"
        );
        (first.await.unwrap(), second.await)
    });
    assert_eq!(first.unwrap().state, PayoutState::Sent);
    assert_eq!(code(&second.unwrap_err()), "platform.lease_expired");
    assert_eq!(lib.withdrawn_to().len(), 1);
    assert_eq!(f.payout_rows().len(), 1);
    assert!(f.reserved(BOB).is_none(), "no second address reserved");
}

/// A payout's `begin_flow` refuses a `PlatformOp` grant that caps no
/// credits before redeeming it: the grant stays for another try.
#[test]
fn a_payout_lease_refuses_a_grant_without_credits_and_keeps_it() {
    let f = fixture(false);
    let grant = f.grant(GrantPurpose::PlatformOp {
        max_duffs: 1_000,
        max_credits: 0,
    });
    let s = Arc::clone(&f.session);
    let g = grant.clone();
    let begun = f.engine.block_on(async move {
        let flow = FlowKind::ContactPayFromBalance;
        (s.begin_flow(f.wallet, flow, vec![g], Some(PaymentSource::Shielded))).await
    });
    assert!(begun.is_err());
    assert!(f.grant_kept(&grant));
}

/// While a payout's call runs, its row is the pair's shown lock, and
/// neither "Check again" nor a pass settles it from under the call (its
/// row names no transition yet only because the call has not got there).
#[test]
fn a_payout_under_way_is_shown_locked_and_left_to_its_call() {
    let f = fixture(false);
    let lib = f.hooked();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.platform.payments.hold_st_hash.lock().unwrap() = Some(Arc::clone(&pause));
    let task = f.spawn_shielded(&lib, f.call(BOB, credits(), &f.credits_grant(), false));
    assert!(pause.wait_reached(Duration::from_secs(60)));
    let lock = f.lock(BOB).expect("shown while under way");
    assert_eq!(lock.source, PaymentSource::Shielded);
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    f.after_pass();
    assert_eq!(f.payout_rows()[0].1, "pending");
    assert!(f.reserved(BOB).is_none(), "not given back mid-call");
    pause.release.notify_one();
    let paid = f.engine.block_on(task).unwrap().unwrap();
    assert_eq!(paid.state, PayoutState::Sent);
    assert_eq!(paid.reference, lock.reference);
    assert!(f.lock(BOB).is_none());
}

/// C-funding P6 with a lost or older dispatch journal: a payout whose row
/// names its transition and whose dispatch the hook recorded stays held
/// when the journal has no artifact for it: on a load, a pass and "Check
/// again" alike, with its address consumed. Only a row the hook never let
/// go (no dispatch recorded) reads as never sent.
#[test]
fn a_dispatched_payout_whose_artifact_is_missing_stays_locked() {
    let f = fixture(false);
    let pair = (id(ME), id(BOB));
    let reference = "sh-lost-journal";
    let row = DpContactPayoutRow {
        reference: reference.into(),
        identity: b58(ME),
        contact: b58(BOB),
        source: "shielded".into(),
        st_hash: Some(hex::encode(FakeShielded::st_hash(9))),
        address: f.contact_address(BOB, 0).to_string(),
        credits: credits(),
        expected_duffs: 1,
        note: None,
        state: "pending".into(),
        since: 1,
    };
    let s = &f.session;
    f.engine
        .block_on(s.record_payout(f.wallet, pair, row))
        .unwrap();
    let scope = dw_appdb::local_scope(&f.wallet.to_string());
    let key = dw_appdb::dp_payout_dispatched(reference);
    (f.engine)
        .block_on(s.appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("1"))))
        .unwrap();
    assert!(
        !f.has_artifact(FakeShielded::st_hash(9)),
        "the journal lost it"
    );

    f.restart_payments();
    assert_eq!(f.lock(BOB).map(|l| l.reference), Some(reference.into()));
    f.after_pass();
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    assert!(f.lock(BOB).is_some());
    assert_eq!(f.payout_rows()[0].1, "pending");
    assert!(f.reserved(BOB).is_none(), "its address may have been paid");
}

/// P7: only the library's own `executed` disposition is proof. A spend it
/// could not attribute, which the journal reads as executed for display
/// once its history row is confirmed, keeps the contact locked on a pass,
/// "Check again" and a load; the stored `executed` disposition clears it.
#[test]
fn an_execution_the_library_could_not_attribute_keeps_the_contact_locked() {
    let f = fixture(false);
    let lib = f.hooked();
    lib.fail_with(PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "withdraw",
        reason: "timeout".into(),
    });
    let paid = f.shielded(&lib, BOB, &f.credits_grant()).unwrap();
    assert_eq!(paid.state, PayoutState::Pending);
    let hash = FakeShielded::st_hash(0);
    let unattributed = ("unresolved", Some("unattributable"));
    f.settle_artifact_as(hash, ArtifactStatus::Executed, unattributed);
    f.after_pass();
    assert!(f.lock(BOB).is_some(), "a pass");
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    f.restart_payments();
    assert!(f.lock(BOB).is_some(), "a load");
    assert_eq!(f.payout_rows()[0].1, "pending");

    f.settle_artifact_as(hash, ArtifactStatus::Executed, ("executed", None));
    f.after_pass();
    assert!(f.lock(BOB).is_none());
    assert_eq!(f.payout_rows()[0].1, "sent");
}

/// P6, P5: a spend the hook refused because it could not record its
/// dispatch is settled not sent on the call's own end, its address
/// consumed (its artifact exists, settled `not_executed`), and the contact
/// is free to pay again at a new address.
#[test]
fn a_payout_whose_dispatch_was_not_recorded_is_not_sent_with_its_address_consumed() {
    let f = fixture(false);
    let lib = f.hooked();
    (f.session.platform.payments.fail_dispatch_record)
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let refused = f.shielded(&lib, BOB, &f.credits_grant()).unwrap_err();
    assert_eq!(code(&refused), "shielded.spend_refused");
    let rows = f.payout_rows();
    assert_eq!(rows[0].1, "not_sent");
    assert!(f.lock(BOB).is_none());
    assert!(f.reserved(BOB).is_none(), "its address stays consumed");
    assert!(lib.withdrawn_to().is_empty(), "the library sent nothing");
    assert!(!f.dispatched(&rows[0].0));
}
