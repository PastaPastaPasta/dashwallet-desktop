//! C-funding D-C5's Shielded source (C5a) over the contact fixture and a
//! scripted library whose withdrawals pass the engine's hook steps: the
//! payout unshields to the contact's reserved address, its row names the
//! spend's transition before the spend's artifact exists, the journal's
//! disposition alone settles the contact's lock, a kill at each step
//! relaunches into the right state, and a pool-floor refusal holds the
//! contact with its address consumed.

use dpp::version::PlatformVersion;
use dw_appdb::dispatch::ArtifactStatus;
use platform_wallet::PlatformWalletError;

use super::*;
use crate::platform::payment_locks::PayCall;
use crate::platform::shielded_payout;
use crate::platform::shielded_tests::FakeShielded;
use crate::platform::withdrawal::Rules;
use crate::platform::{ContactPayOutcome, PaymentSource, PayoutState, ShieldedError};

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

/// C5a, P5: a pool-floor refusal holds the lock and keeps the address
/// consumed; a Core payment to the contact is refused meanwhile; A5's
/// definitely unsent clears the lock and the address is not reused.
/// Mutant: give the address back on the refusal.
#[test]
fn a_pool_floor_refusal_holds_the_contact_with_its_address_consumed() {
    let f = fixture(false);
    let lib = f.hooked();
    lib.fail_with(PlatformWalletError::ShieldedBroadcastFailed(
        "state transition broadcast failed: Shielded pool has insufficient notes for \
         outgoing transition: pool has 3 notes but minimum 250 required"
            .into(),
    ));
    let floor = f.shielded(&lib, BOB, &f.credits_grant()).unwrap_err();
    assert_eq!(
        floor,
        ContactError::Shielded(ShieldedError::PoolTooSmall {
            notes: 3,
            required: 250
        })
    );
    assert_eq!(f.lock(BOB).map(|l| l.source), Some(PaymentSource::Shielded));
    assert!(f.reserved(BOB).is_none(), "consumed, not given back");
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    assert!(matches!(
        contact_error(d.set_contact_recipient(f.pay(BOB, AMOUNT))),
        ContactError::PaymentLocked { .. }
    ));
    f.restart_payments();
    assert!(f.lock(BOB).is_some(), "a relaunch keeps it");

    f.settle_artifact(FakeShielded::st_hash(0), ArtifactStatus::NotExecuted, true);
    assert_eq!(f.resolve(BOB), LockResolution::NotSent);
    assert!(f.lock(BOB).is_none());
    assert!(f.reserved(BOB).is_none(), "still consumed");
    let next = f.shielded(&lib, BOB, &f.credits_grant()).unwrap();
    assert_eq!(next.state, PayoutState::Sent);
    assert_eq!(
        lib.withdrawn_to().last(),
        Some(&f.contact_address(BOB, 1).to_string())
    );
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

    // After the artifact and the send, before the call settled.
    let pause = Arc::new(crate::coins::TestPause::default());
    (f.session.platform.payments).pause_settle(Arc::clone(&pause));
    let row = kill_at(&pause);
    assert_eq!(row.1, "pending");
    let hash = FakeShielded::st_hash(0);
    assert_eq!(row.2, Some(hex::encode(hash)));
    assert_eq!(f.lock(BOB).map(|l| l.reference), Some(row.0.clone()));
    assert!(f.reserved(BOB).is_none(), "the address may have been paid");
    assert_eq!(lib.withdrawn_to(), [row.3.clone()]);
    f.settle_artifact(hash, ArtifactStatus::Executed, true);
    f.restart_payments();
    assert!(f.lock(BOB).is_none());
    let rows = f.payout_rows();
    let settled = rows.iter().find(|r| r.0 == row.0).unwrap();
    assert_eq!(settled.1, "sent");
}
