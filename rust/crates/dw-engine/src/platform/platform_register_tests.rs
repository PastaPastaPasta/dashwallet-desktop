//! A registration from the Platform addresses (C-funding F8–F10, F22): the
//! quote over one proved read, then the create under the move lock, against
//! a scripted network: recorded before it is signed, and settled by
//! evidence alone wherever the process stopped.

use super::*;
use crate::platform::lease::Lease;
use crate::platform::registration_platform::{RegisterPlan, register_quote};
use crate::platform::registration_rows::PlatformPlan;

impl World {
    fn register_quote(&self, funding: u64) -> Result<RegisterPlan, CreditsError> {
        self.call(
            move |net, s, w, id| async move { register_quote(&*net, &s, &w, id, funding).await },
        )
    }
}

const FUNDING: u64 = 25_000_000;

/// Each input signs at the nonce the proved read shows, plus one; a send
/// failed by height may still execute, so it refuses the plan; once its
/// nonces are taken the quote plans again, at the next nonce.
#[test]
fn the_register_quote_signs_at_platforms_nonces_and_refuses_beside_a_hold() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let p = w.register_quote(FUNDING).unwrap();
    assert_eq!(p.inputs, [(w.address(0), (1, FUNDING))].into());
    let (_, to) = w.theirs(9);
    let held = w.held_send(WithdrawAmount::Credits { credits: DASH }, &to);
    match w.register_quote(FUNDING) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, held.id),
        other => panic!("{other:?}"),
    }
    w.lands(0);
    let p = w.register_quote(FUNDING).unwrap();
    assert_eq!(p.inputs, [(w.address(0), (2, FUNDING))].into());
}

/// A move whose outcome is unknown refuses the plan.
#[test]
fn the_register_quote_refuses_beside_a_pending_move() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::Lost);
    let one = WithdrawAmount::Credits { credits: DASH };
    let pending = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(pending.state, PlatformActivityState::Pending);
    match w.register_quote(FUNDING) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, pending.id),
        other => panic!("{other:?}"),
    }
}

/// One registration row as the funding step sees it: its stored plan and
/// its lease.
struct Row {
    plan: Option<PlatformPlan>,
    lease: Option<Lease>,
    draft: i64,
}

impl World {
    /// A registration lease of `credits`; `from_platform`: it carries the
    /// addresses' signer.
    fn register_lease(&self, credits: u64, from_platform: bool) -> Lease {
        let grant = self.f.grant(credits);
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        (self.f.engine)
            .block_on(async move {
                s.begin_registration_lease(id, &[grant], from_platform)
                    .await
            })
            .unwrap()
    }

    fn row(&self, draft: i64) -> Row {
        Row {
            plan: None,
            lease: Some(self.register_lease(20 * DASH, true)),
            draft,
        }
    }

    /// One funding pass of `row`; a plan it writes is the row's, and one
    /// cleared (`Replan`) is gone.
    fn pass(&self, row: &mut Row, resign: bool) -> Result<RegisterOutcome, CreditsError> {
        let written = Arc::new(Mutex::new(None));
        let ask = RegisterAsk {
            draft: row.draft,
            index: 0,
            funding_credits: FUNDING,
            fee_reserve_credits: self.reserve(),
            plan: row.plan.clone(),
            resign,
        };
        let (lease, into) = (row.lease.clone(), Arc::clone(&written));
        let out = self.call(move |net, s, w, id| async move {
            let write = move |plan: PlatformPlan| -> PlanWrite<'static> {
                *into.lock().unwrap() = Some(plan);
                Box::pin(async { Ok(()) })
            };
            register_fund(&*net, &s, &w, id, (ask, lease.as_ref()), write).await
        });
        if let Some(plan) = written.lock().unwrap().take() {
            row.plan = Some(plan);
        }
        if let Ok(RegisterOutcome::Replan(_)) = &out {
            row.plan = None;
        }
        out
    }

    /// The reserve the quote gives for one input, as the network's version
    /// prices it (a quote would refuse beside a pending create).
    fn reserve(&self) -> u64 {
        let keys = crate::platform::keys_policy::registration_specs().len();
        crate::platform::registration_platform::REGISTER_FEE_BAND
            * crate::platform::registration_platform::register_fee(self.net.version(), 1, keys)
    }

    fn registers(&self) -> Vec<MoveRecord> {
        (self.moves().into_iter())
            .filter(|m| m.kind == MoveKind::Register)
            .collect()
    }

    fn created(&self, plan: &PlatformPlan) -> bool {
        let id = Identifier::from_string(&plan.identity, Encoding::Base58).unwrap();
        self.net.identities.lock().unwrap().contains(&id)
    }
}

/// F9, F10: the plan is written into the row first, then the `Register`
/// record, read back, then the create is signed at Platform's nonce + 1;
/// the SDK's proved result registers the planned identity, the record is
/// done and the history shows it as a move to an identity.
#[test]
fn a_registration_is_planned_recorded_signed_and_proved() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let reserve = w.reserve();
    let mut row = w.row(1);
    let out = w.pass(&mut row, true).unwrap();
    let plan = row.plan.clone().unwrap();
    assert_eq!(out, RegisterOutcome::Registered(plan.identity.clone()));
    assert_eq!(
        plan.inputs,
        vec![(bech32m(&w.address(0), w.network()), 1, FUNDING)]
    );
    assert!(w.created(&plan));
    assert_eq!(*w.net.recorded_before_signing.lock().unwrap(), [true]);
    let records = w.registers();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(
        (
            r.state,
            r.can_land,
            r.signed,
            r.answered,
            r.credits,
            r.to.as_str()
        ),
        (
            PlatformActivityState::Done,
            true,
            true,
            true,
            FUNDING + reserve,
            plan.identity.as_str()
        )
    );
    let row = find(&w.activity(), PlatformActivityKind::ToIdentity).clone();
    assert_eq!(row.state, PlatformActivityState::Done);
}

/// Funds packet: a submit that fails after signing. The answer is lost
/// whether or not the create executed: the pass is `Unknown` and the row
/// keeps its plan; the next pass settles it by evidence, signing nothing
/// (the nonces taken and the identity proved), or, when it never executed,
/// holds it once failed by height, and only a re-sign signs again, at the
/// same nonce, giving the same identity.
#[test]
fn a_submit_failing_after_signing_is_settled_by_evidence() {
    for executed in [true, false] {
        let w = World::new();
        w.net.fund(w.address(0), 5 * DASH);
        let mut row = w.row(1);
        w.net.then(if executed {
            Act::ExecutesUnanswered
        } else {
            Act::Lost
        });
        assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
        let plan = row.plan.clone().unwrap();
        if executed {
            let out = w.pass(&mut row, true).unwrap();
            assert_eq!(out, RegisterOutcome::Registered(plan.identity.clone()));
            assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing signed again");
            continue;
        }
        // Pending at the same height: waits, signs nothing.
        assert_eq!(w.registers()[0].state, PlatformActivityState::Pending);
        assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
        w.net.tick();
        assert_eq!(w.pass(&mut row, false).unwrap(), RegisterOutcome::Held);
        assert_eq!(w.net.sent.lock().unwrap().len(), 1);
        let out = w.pass(&mut row, true).unwrap();
        assert_eq!(out, RegisterOutcome::Registered(plan.identity.clone()));
        let sent = w.net.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1], "the same inputs at the same nonces");
        assert_eq!(row.plan, Some(plan));
    }
}

/// Funds packet: a crash at each step. Stopped after the plan was written
/// but before its record, before signing, after signing, after the
/// broadcast, or before the outcome was stored, the restarted row settles
/// by evidence alone and registers the planned identity once, at the
/// plan's nonces.
#[test]
fn a_registration_stopped_at_each_step_settles_after_a_restart() {
    // After the plan, before the record: the write "fails" as a crash would
    // end the pass.
    let mut w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    let planned = {
        let written = Arc::new(Mutex::new(None));
        let ask = RegisterAsk {
            draft: 1,
            index: 0,
            funding_credits: FUNDING,
            fee_reserve_credits: w.reserve(),
            plan: None,
            resign: true,
        };
        let (lease, into) = (row.lease.clone(), Arc::clone(&written));
        let out = w.call(move |net, s, wallet, id| async move {
            let write = move |plan: PlatformPlan| -> PlanWrite<'static> {
                *into.lock().unwrap() = Some(plan);
                Box::pin(async {
                    Err(PlatformError::Storage {
                        detail: "the process stops".into(),
                    })
                })
            };
            register_fund(&*net, &s, &wallet, id, (ask, lease.as_ref()), write).await
        });
        assert!(out.is_err());
        written.lock().unwrap().take().unwrap()
    };
    assert!(w.registers().is_empty(), "no record, nothing signed");
    assert!(w.net.sent.lock().unwrap().is_empty());
    w.reopen();
    row.lease = Some(w.register_lease(20 * DASH, true));
    row.plan = Some(planned.clone());
    let out = w.pass(&mut row, true).unwrap();
    assert_eq!(out, RegisterOutcome::Registered(planned.identity.clone()));
    for (stop, executed) in [
        (Act::StopsBeforeSigning, false),
        (Act::StopsAfterSigning, false),
        (Act::StopsAfterBroadcast, true),
        (Act::StopsBeforeOutcome, true),
    ] {
        let mut w = World::new();
        w.net.fund(w.address(0), 5 * DASH);
        let mut row = w.row(1);
        w.net.then(stop);
        assert!(w.pass(&mut row, true).is_err(), "{stop:?}");
        let plan = row.plan.clone().expect("written before the record");
        assert_eq!(w.registers()[0].state, PlatformActivityState::Pending);
        w.reopen();
        row.lease = Some(w.register_lease(20 * DASH, true));
        let out = w.pass(&mut row, true).unwrap();
        if executed {
            assert_eq!(
                out,
                RegisterOutcome::Registered(plan.identity.clone()),
                "{stop:?}"
            );
            continue;
        }
        // Maybe signed, never sent: it waits, then holds, then the re-sign
        // at the recorded nonces registers it.
        assert_eq!(out, RegisterOutcome::Unknown, "{stop:?}");
        w.net.tick();
        assert_eq!(
            w.pass(&mut row, false).unwrap(),
            RegisterOutcome::Held,
            "{stop:?}"
        );
        let out = w.pass(&mut row, true).unwrap();
        assert_eq!(
            out,
            RegisterOutcome::Registered(plan.identity.clone()),
            "{stop:?}"
        );
        let sent = w.net.sent.lock().unwrap().clone();
        assert_eq!(sent.last().unwrap()[&w.address(0)].0, 1, "{stop:?}");
    }
}

/// Funds packet: a double submit. A second pass of a registered row, a
/// pass beside a pending create, and a second registration while the first
/// holds sign nothing new.
#[test]
fn a_double_submit_signs_nothing_new() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    let first = w.pass(&mut row, true).unwrap();
    assert_eq!(w.pass(&mut row, true).unwrap(), first);
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);

    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    w.net.then(Act::Lost);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    let mut other = w.row(2);
    match w.pass(&mut other, true) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, w.registers()[0].id),
        other => panic!("{other:?}"),
    }
    assert!(other.plan.is_none(), "refused before its plan is written");
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
}

/// F22: a re-sign of a held create that releases no signature says
/// nothing about the first: its record still holds, the row keeps its plan
/// and waits, and the first create executing registers the identity.
#[test]
fn a_re_sign_that_signs_nothing_keeps_the_held_create() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    w.net.then(Act::Lost);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    let plan = row.plan.clone().unwrap();
    w.net.tick();
    assert_eq!(w.pass(&mut row, false).unwrap(), RegisterOutcome::Held);
    w.net.then(Act::Unsigned);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Held);
    assert_eq!(row.plan, Some(plan.clone()));
    let r = &w.registers()[0];
    assert_eq!((r.state, r.can_land), (PlatformActivityState::Failed, true));
    w.net.execute_sent(0);
    let out = w.pass(&mut row, false).unwrap();
    assert_eq!(out, RegisterOutcome::Registered(plan.identity));
}

/// F10: a plan written but never recorded (its signing refused) leaves its
/// nonces to another draft, whose create gives the same identity; once
/// that create takes them, the first row plans again rather than claim
/// the other draft's identity.
#[test]
fn another_drafts_create_of_the_same_identity_is_not_this_rows() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut first = w.row(1);
    first.lease = Some(w.register_lease(FUNDING, true));
    assert!(w.pass(&mut first, true).is_err(), "over the lease's budget");
    let planned = first.plan.clone().expect("written before the permit");
    assert!(w.registers().is_empty());
    let mut second = w.row(2);
    let out = w.pass(&mut second, true).unwrap();
    assert_eq!(out, RegisterOutcome::Registered(planned.identity.clone()));
    first.lease = Some(w.register_lease(20 * DASH, true));
    assert_eq!(
        w.pass(&mut first, true).unwrap(),
        RegisterOutcome::Replan(None)
    );
    assert!(first.plan.is_none());
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
}

/// A create a node refused after its signature left keeps its hold: the
/// answer cannot prove it never reached another node. A restored plan
/// (no record here) that releases nothing holds too: another install may
/// have signed it.
#[test]
fn a_released_or_restored_create_keeps_its_hold() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    w.net.then(Act::Invalid);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    assert!(row.plan.is_some());
    let r = &w.registers()[0];
    assert_eq!(
        (r.state, r.can_land),
        (PlatformActivityState::Pending, true)
    );

    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut first = w.row(1);
    first.lease = Some(w.register_lease(FUNDING, true));
    assert!(w.pass(&mut first, true).is_err(), "over the lease's budget");
    let mut restored = w.row(1);
    restored.plan = first.plan.clone();
    w.net.then(Act::Unsigned);
    assert_eq!(
        w.pass(&mut restored, true).unwrap(),
        RegisterOutcome::Unknown
    );
    assert_eq!(restored.plan, first.plan, "kept");
    let r = &w.registers()[0];
    assert_eq!(
        (r.state, r.can_land),
        (PlatformActivityState::Pending, true)
    );
}

/// A stored plan signs only as it was made: one that repeats an address,
/// claims nothing or names nonce 0 is refused before anything is recorded
/// or signed.
#[test]
fn a_tampered_plan_signs_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    w.pass(&mut row, true).unwrap();
    let good = row.plan.clone().unwrap();
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let first = good.inputs[0].clone();
    for inputs in [
        vec![first.clone(), first.clone()],
        vec![(first.0.clone(), first.1, 0)],
        vec![(first.0.clone(), 0, first.2)],
    ] {
        let mut row = w.row(1);
        row.plan = Some(PlatformPlan {
            inputs,
            ..good.clone()
        });
        assert!(matches!(
            w.pass(&mut row, true),
            Err(CreditsError::Platform(_))
        ));
    }
    assert!(w.registers().is_empty());
    assert!(w.net.sent.lock().unwrap().is_empty());
}

/// F10, F22: beside a held create, "Move to Dash wallet" signs exactly its
/// inputs at its nonces; its document present says the exit won and the
/// row goes back to Confirm. Absent, with the nonces taken and no identity,
/// the create failed inside its block (paid).
#[test]
fn the_way_out_and_a_paid_failure_settle_by_their_documents() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    w.net.then(Act::Lost);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    w.net.tick();
    assert_eq!(w.pass(&mut row, false).unwrap(), RegisterOutcome::Held);
    let exit = w.move_all(&w.f.grant(20 * DASH)).unwrap();
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(
        sent[1].keys().collect::<Vec<_>>(),
        sent[0].keys().collect::<Vec<_>>()
    );
    assert_eq!(sent[1][&w.address(0)].0, sent[0][&w.address(0)].0);
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Exited);
    assert!(!w.created(row.plan.as_ref().unwrap()));

    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.create_fails.store(true, Ordering::SeqCst);
    let mut row = w.row(1);
    w.net.then(Act::ExecutesUnanswered);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    assert_eq!(
        w.pass(&mut row, true).unwrap(),
        RegisterOutcome::PaidFailure
    );
    // An identity read that fails decides nothing.
    w.net.documents_down.store(true, Ordering::SeqCst);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
}

/// F22: beside a held create and another funded address, "Move to Dash
/// wallet" quotes only the create's inputs, and a typed amount moves from
/// them at the create's nonces, as the move plans it.
#[test]
fn the_way_out_quotes_only_the_held_creates_inputs() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.fund(w.address(1), 3 * DASH);
    w.net.fee.store(1, Ordering::SeqCst);
    let mut row = w.row(1);
    w.net.then(Act::Lost);
    assert_eq!(w.pass(&mut row, true).unwrap(), RegisterOutcome::Unknown);
    w.net.tick();
    assert_eq!(w.pass(&mut row, false).unwrap(), RegisterOutcome::Held);
    let create = w.net.sent.lock().unwrap()[0].clone();
    let held: u64 = {
        let state = w.net.state.lock().unwrap();
        create.keys().map(|a| state[a].1).sum()
    };
    assert!(held < 8 * DASH);

    assert_eq!(w.quote(WithdrawAmount::All).unwrap().credits, held);
    let amount = WithdrawAmount::Credits { credits: DASH / 2 };
    let q = w.quote(amount).unwrap();
    let out = w
        .move_to(amount, &w.to.clone(), &w.f.grant(q.grant.max_credits))
        .unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    let exit = w.net.sent.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        exit.keys().collect::<Vec<_>>(),
        create.keys().collect::<Vec<_>>()
    );
    for (a, (nonce, _)) in &exit {
        assert_eq!(*nonce, create[a].0);
    }
}

/// F10: in the process that made the call, a create that released no
/// signature can never execute: its record fails unable to land, the plan
/// is cleared with the refusal, and the next plan signs at the same nonces.
/// A lease without the addresses' signer plans nothing.
#[test]
fn a_create_that_released_nothing_plans_again() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    w.net.then(Act::Unsigned);
    match w.pass(&mut row, true).unwrap() {
        RegisterOutcome::Replan(Some(_)) => {}
        other => panic!("{other:?}"),
    }
    assert!(row.plan.is_none());
    let r = &w.registers()[0];
    assert_eq!(
        (r.state, r.can_land),
        (PlatformActivityState::Failed, false)
    );
    let out = w.pass(&mut row, true).unwrap();
    assert!(matches!(out, RegisterOutcome::Registered(_)), "{out:?}");

    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let mut row = w.row(1);
    row.lease = Some(w.register_lease(20 * DASH, false));
    let e = w.pass(&mut row, true).unwrap_err();
    assert_eq!(e.code(), "platform.needs_grant", "{e:?}");
    assert!(row.plan.is_none() && w.registers().is_empty());
    row.lease = None;
    let e = w.pass(&mut row, true).unwrap_err();
    assert_eq!(e.code(), "platform.needs_grant", "{e:?}");
}
