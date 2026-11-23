//! The top-up of an identity from the Platform addresses (C-routes D-C3,
//! slice C3a): the money rules of its test table.

use std::time::Duration;

use super::super::super::requests::tests::identity;
use super::*;

/// The wallet's own identity (the fixture's at index 0).
fn me() -> String {
    identity(1, BTreeMap::new(), 0)
        .id()
        .to_string(Encoding::Base58)
}

fn limits() -> Limits {
    Limits::top_up_at(PlatformVersion::latest())
}

impl World {
    fn top_up_quote(
        &self,
        identity: &str,
        amount: WithdrawAmount,
    ) -> Result<TopUpFromPlatformQuote, CreditsError> {
        let identity = identity.to_owned();
        self.call(move |net, s, w, id| async move {
            top_up_quote(&*net, &s, &w, id, (identity.as_str(), amount)).await
        })
    }

    fn top_up_as(
        &self,
        identity: &str,
        amount: WithdrawAmount,
        grant: &str,
    ) -> Result<TopUpFromPlatformOutcome, CreditsError> {
        let (identity, grant) = (identity.to_owned(), grant.to_owned());
        self.call(move |net, s, w, id| async move {
            top_up(&*net, &s, &w, id, (identity.as_str(), amount), &grant).await
        })
    }

    fn top_up(&self, amount: WithdrawAmount) -> Result<TopUpFromPlatformOutcome, CreditsError> {
        self.top_up_as(&me(), amount, &self.f.grant(100 * DASH))
    }

    /// The pool's addresses in `PlatformAddress` order.
    fn in_order(&self) -> Vec<PlatformAddress> {
        let mut pool = self.pool.clone();
        pool.sort();
        pool
    }

    fn sent(&self) -> usize {
        self.net.sent.lock().unwrap().len()
    }
}

fn credits(credits: u64) -> WithdrawAmount {
    WithdrawAmount::Credits { credits }
}

/// The plan's input 0 is the first selected address in `PlatformAddress`
/// order, not the largest, and keeps the band unclaimed, for an amount and
/// for Max (the usable balance less the band). The claims are what the
/// identity gains. Below the input minimum on input 0 or the funding
/// minimum in total, the quote refuses `credits.send_out_of_range` and
/// nothing is written or signed.
#[test]
fn top_up_from_platform_fee_on_input_0() {
    let w = World::new();
    let order = w.in_order();
    let l = limits();
    // The first in order holds less than the band: it is left out.
    w.net.fund(order[0], l.keep(1) / 2);
    w.net.fund(order[1], 2 * DASH);
    w.net.fund(order[2], 5 * DASH);

    let q = w.top_up_quote(&me(), credits(DASH)).unwrap();
    assert_eq!(q.credits, DASH);
    assert_eq!(q.fee_credits, l.fee(1));
    assert_eq!(q.debit_credits, DASH + l.keep(1));
    assert_eq!(q.grant.max_credits, q.debit_credits);
    assert_eq!(q.inputs.len(), 1);
    assert_eq!(q.inputs[0].address, bech32m(&order[1], w.network()));

    // More than input 0 can give: input 0 keeps the band for two inputs.
    let q = w.top_up_quote(&me(), credits(3 * DASH)).unwrap();
    assert_eq!(q.inputs[0].address, bech32m(&order[1], w.network()));
    assert_eq!(q.inputs[0].credits, 2 * DASH - l.keep(2));
    assert_eq!(q.inputs.iter().map(|i| i.credits).sum::<u64>(), 3 * DASH);

    let max = w.top_up_quote(&me(), WithdrawAmount::All).unwrap();
    assert_eq!(max.credits, 7 * DASH - l.keep(2));

    // Below the funding minimum, and input 0 below the input minimum.
    let refused = w.top_up_quote(&me(), credits(l.min - 1)).unwrap_err();
    assert!(
        matches!(refused, CreditsError::SendOutOfRange { min } if min == l.min),
        "{refused:?}"
    );
    // Max over one address just above the band: input 0 would claim less
    // than the input minimum.
    let w2 = World::new();
    w2.net.fund(w2.address(0), l.keep(1) + l.min_input / 2);
    let refused = w2.top_up_quote(&me(), WithdrawAmount::All).unwrap_err();
    assert!(
        matches!(refused, CreditsError::SendOutOfRange { min } if min == l.min_input),
        "{refused:?}"
    );
    let grant = w2.f.grant(10 * DASH);
    let refused = w2
        .top_up_as(&me(), WithdrawAmount::All, &grant)
        .unwrap_err();
    assert_eq!(refused.code(), "credits.send_out_of_range");
    assert!(w2.f.unused(&grant));
    assert!(w2.moves().is_empty(), "nothing written");
    assert_eq!(w2.sent(), 0);

    // Signed: the fee strategy names input 0, which is the record's fee
    // source, and Drive took the fee there.
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(w.net.sources.lock().unwrap()[0], Some(order[1]));
    let record = &w.moves()[0];
    assert_eq!(record.kind, MoveKind::ToIdentity);
    let fee_source: Vec<_> = (record.inputs.iter())
        .filter(|i| i.fee_source)
        .map(|i| i.address.clone())
        .collect();
    assert_eq!(fee_source, [bech32m(&order[1], w.network())]);
    assert_eq!(w.net.credits(order[1]), 2 * DASH - DASH - w.net.fee());
    assert_eq!(w.net.topped_up.load(Ordering::SeqCst), DASH);
}

/// The record is pending, its inputs at their proved nonce + 1, durably
/// before the first signature.
#[test]
fn top_up_from_platform_record_before_signing() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    w.net.state.lock().unwrap().get_mut(&a).unwrap().0 = 6;
    w.top_up(credits(DASH)).unwrap();
    assert_eq!(*w.net.recorded_before_signing.lock().unwrap(), vec![true]);
    assert_eq!(w.net.sent.lock().unwrap()[0][&a].0, 7);
    assert_eq!(w.moves()[0].inputs[0].nonce, 7);
}

/// The money gate, the identity check, address funds (PV 11), the plan,
/// rule 5 and the grant refuse with the grant unredeemed and nothing
/// signed.
#[test]
fn top_up_from_platform_refusal_keeps_the_grant() {
    use super::super::super::money_gate_tests::{FakePlatform, Proof, gate_session};
    use dpp::version::feature_initial_protocol_versions::ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION as V;
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let grant = w.f.grant(3 * DASH);
    let code = |r: Result<TopUpFromPlatformOutcome, CreditsError>| r.unwrap_err().code();

    // Not one of the wallet's identities.
    let theirs = identity(2, BTreeMap::new(), 0)
        .id()
        .to_string(Encoding::Base58);
    assert_eq!(
        code(w.top_up_as(&theirs, credits(DASH), &grant)),
        "identity.not_found"
    );
    assert_eq!(
        w.top_up_quote(&theirs, credits(DASH)).unwrap_err().code(),
        "identity.not_found"
    );
    // The plan: more than the addresses hold.
    assert_eq!(
        code(w.top_up_as(&me(), credits(30 * DASH), &grant)),
        "platform.insufficient_credits"
    );
    // A grant short of the debit, and a lease of another flow.
    let small = w.f.grant(DASH);
    assert_eq!(
        code(w.top_up_as(&me(), credits(DASH), &small)),
        "platform.grant_exceeded"
    );
    assert!(w.f.unused(&small));
    let other = w.f.flow(FlowKind::ProfileEdit, vec![w.f.grant(3 * DASH)]);
    assert_eq!(
        code(w.top_up_as(&me(), credits(DASH), &other)),
        "platform.grant_invalid"
    );
    // Below address funds.
    *w.net.version.lock().unwrap() = PlatformVersion::get(V - 1).unwrap();
    assert_eq!(
        code(w.top_up_as(&me(), credits(DASH), &grant)),
        "platform.feature_off"
    );
    assert_eq!(
        w.top_up_quote(&me(), credits(DASH)).unwrap_err().code(),
        "platform.feature_off"
    );
    *w.net.version.lock().unwrap() = PlatformVersion::latest();
    // Rule 5 on the address read.
    let platform = FakePlatform::new(Proof::Verified);
    gate_session(&w.f.s, TrustMode::Enforcing, Arc::clone(&platform));
    w.net.unverified.store(true, Ordering::SeqCst);
    // The money gate: Platform does not prove the identity.
    assert_eq!(
        code(w.top_up_as(&me(), credits(DASH), &grant)),
        "platform.context_unavailable"
    );
    assert!(platform.fetches() > 0, "the money gate asked");
    let me_id = identity(1, BTreeMap::new(), 0).id();
    let keys = w
        .call(move |_, _, wallet, _| async move {
            let (_, held) = withdrawal::own(&wallet, me_id).await?;
            Ok(held.public_keys().clone())
        })
        .unwrap();
    platform.answer_identity(me_id, keys);
    assert_eq!(
        code(w.top_up_as(&me(), credits(DASH), &grant)),
        "platform.context_unavailable",
        "the unverified address read"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent(), 0);
    assert!(w.moves().is_empty());

    w.net.unverified.store(false, Ordering::SeqCst);
    let out = w.top_up_as(&me(), credits(DASH), &grant).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    // A lease of the move's own flow tops up too.
    let lease =
        w.f.flow(FlowKind::PlatformAddressMove, vec![w.f.grant(3 * DASH)]);
    w.top_up_as(&me(), credits(DASH / 10), &lease).unwrap();
    assert_eq!(w.sent(), 2);
}

/// An unknown outcome leaves the record pending and able to land; the next
/// move, send, shield quote and top-up from Platform are
/// `credits.move_pending`, and a second submit signs nothing.
#[test]
fn top_up_from_platform_unknown_holds_the_next_move() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::Lost);
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(
        (out.state, out.may_land),
        (PlatformActivityState::Pending, true)
    );
    let held = |code: &str| assert_eq!(code, "credits.move_pending");
    held(w.top_up(credits(DASH)).unwrap_err().code());
    held(w.top_up_quote(&me(), credits(DASH)).unwrap_err().code());
    held(w.move_all(&w.f.grant(5 * DASH)).unwrap_err().code());
    let (_, to) = w.theirs(9);
    held(w.send_quote(credits(DASH), &to).unwrap_err().code());
    held(
        w.call(|net, s, wallet, id| async move {
            shield_quote(
                &*net,
                &s,
                &wallet,
                id,
                (PlatformVersion::latest(), WithdrawAmount::All),
            )
            .await
        })
        .unwrap_err()
        .code(),
    );
    assert_eq!(w.sent(), 1, "signed once");
    let row = find(&w.activity(), PlatformActivityKind::ToIdentity).clone();
    assert_eq!(
        (row.state, row.may_land),
        (PlatformActivityState::Pending, true)
    );
}

/// Only the inputs' proved nonces settle it: an answer with the
/// identity's balance and no execution stays pending, and it is done once
/// a proof shows its nonce.
#[test]
fn top_up_from_platform_settles_on_proved_nonces() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::AnswersOnly);
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(out.balance, Some(0), "answered");
    assert_eq!(out.state, PlatformActivityState::Pending);
    w.net.execute_sent(0);
    let row = find(&w.activity(), PlatformActivityKind::ToIdentity).clone();
    assert_eq!(
        (row.state, row.may_land),
        (PlatformActivityState::Done, false)
    );
    assert_eq!(w.net.topped_up.load(Ordering::SeqCst), DASH);
}

/// A done top-up is one `toIdentity` row naming the identity, with what it
/// took and the fee the balances show; its inputs' fall is no `outgoing`
/// row.
#[test]
fn top_up_from_platform_attribution() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.activity();
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(out.fee_credits, Some(w.net.fee()));
    let rows = w.activity();
    let row = find(&rows, PlatformActivityKind::ToIdentity);
    assert_eq!(row.to.as_deref(), Some(me().as_str()));
    assert_eq!(row.credits, DASH + w.net.fee());
    assert_eq!(row.fee_credits, Some(w.net.fee()));
    assert!(
        !rows
            .iter()
            .any(|r| r.kind == PlatformActivityKind::Outgoing),
        "{rows:?}"
    );
}

/// Past the band: Drive refuses unpaid and the signed top-up stays valid.
/// Failed by height it holds: a new top-up or send is
/// `credits.move_pending`. "Move to Dash wallet" of exactly its inputs at
/// its nonces runs beside it, and the exit's withdrawal document decides
/// which executed: present, the exit is done and the top-up failed, and
/// the hold clears.
#[test]
fn top_up_fee_shortfall_holds_and_to_core_exits() {
    let (w, a) = shortfall();
    let grant = w.f.grant(10 * DASH);
    assert_eq!(
        w.top_up(credits(DASH)).unwrap_err().code(),
        "credits.move_pending"
    );
    let (_, to) = w.theirs(9);
    assert_eq!(
        w.send(credits(DASH), &to, &grant).unwrap_err().code(),
        "credits.move_pending"
    );

    // The exit, at the top-up's key; the fee is back within reach.
    w.net.fee.store(1, Ordering::SeqCst);
    let exit = w.move_all(&grant).unwrap();
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[1][&a].0, sent[0][&a].0, "the same nonce");
    assert_eq!(exit.state, PlatformActivityState::Done);
    let moves = w.moves();
    assert_eq!(
        (moves[0].kind, moves[0].state, moves[0].can_land),
        (MoveKind::ToIdentity, PlatformActivityState::Failed, false)
    );
    assert_eq!(w.net.topped_up.load(Ordering::SeqCst), 0);
    assert!(
        !(w.activity().iter()).any(|r| r.state == PlatformActivityState::Replaced),
        "never replaced"
    );
    // The hold is gone.
    w.top_up_quote(&me(), credits(DASH / 1_000)).unwrap();
}

/// The other branch: the top-up executes after the exit was signed. The
/// nonces are taken and the exit's document is absent: the top-up is done,
/// the exit failed, and no "Moved to Dash wallet" row is done.
#[test]
fn top_up_executes_beside_its_exit() {
    let (w, _) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    w.net.then(Act::EarlierExecutes(0));
    let exit = w.move_all(&w.f.grant(10 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Failed);
    assert_eq!(w.net.topped_up.load(Ordering::SeqCst), DASH);
    let rows = w.activity();
    let top_up = find(&rows, PlatformActivityKind::ToIdentity);
    assert_eq!(top_up.state, PlatformActivityState::Done);
    let moved = find(&rows, PlatformActivityKind::MoveToCore);
    assert_eq!(
        (moved.state, moved.may_land),
        (PlatformActivityState::Failed, false)
    );
}

/// With the nonces taken and the exit's document unread (the read failed
/// or was behind), neither is settled: both keep holding.
#[test]
fn an_undecided_exit_and_top_up_both_hold() {
    let mut moves = vec![
        {
            let mut m = record_of(MoveKind::ToIdentity, "t", 1);
            m.state = PlatformActivityState::Failed;
            m.seen_at = Some(5);
            m
        },
        record_of(MoveKind::ToCore, "x", 2),
    ];
    let landed = seen_at(20, 4);
    settle_with(&mut moves, &landed, &Exits::new());
    assert!(moves.iter().all(MoveRecord::holds));
    settle_with(&mut moves, &landed, &[("x".to_owned(), false)].into());
    assert_eq!(moves[0].state, PlatformActivityState::Done);
    assert_eq!(
        (moves[1].state, moves[1].can_land),
        (PlatformActivityState::Failed, false)
    );
}

/// The identity's write lock is taken before the move lock, and no path
/// takes them the other way round: a top-up waiting for the identity (a
/// withdrawal holding it) holds no move lock, so a move runs meanwhile;
/// then the top-up runs. Neither deadlocks.
#[test]
fn lock_order_top_up_from_platform_vs_withdraw_vs_move() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.fund(w.address(1), 5 * DASH);
    let (grant, move_grant) = (w.f.grant(10 * DASH), w.f.grant(10 * DASH));
    let identity_id = identity(1, BTreeMap::new(), 0).id();
    let to = w.to.clone();
    let (top_up_out, move_out) = w
        .call(move |net, s, wallet, id| async move {
            let locks = identity_locks(id, identity_id);
            // A withdrawal of the identity in flight.
            let withdrawal = locks.write.lock().await;
            let me = me();
            let top_up = top_up(&*net, &s, &wallet, id, (me.as_str(), credits(DASH)), &grant);
            tokio::pin!(top_up);
            let early = tokio::time::timeout(Duration::from_millis(300), &mut top_up).await;
            assert!(early.is_err(), "the top-up waits for the identity");
            let moved = tokio::time::timeout(
                Duration::from_secs(30),
                move_to_core(
                    &*net,
                    &s,
                    &wallet,
                    id,
                    (credits(DASH), to.as_str()),
                    &move_grant,
                ),
            )
            .await
            .expect("the move runs while the top-up waits");
            drop(withdrawal);
            let topped = tokio::time::timeout(Duration::from_secs(30), top_up)
                .await
                .expect("the top-up runs once the identity is free");
            Ok((topped, moved))
        })
        .unwrap();
    assert_eq!(move_out.unwrap().state, PlatformActivityState::Done);
    assert_eq!(top_up_out.unwrap().state, PlatformActivityState::Done);
}

/// A top-up refused unpaid for a fee past the band, then failed by height:
/// the world and its one funded address.
fn shortfall() -> (World, PlatformAddress) {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    // Platform meters more than input 0 keeps.
    w.net.fee.store(5 * DASH, Ordering::SeqCst);
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Pending);
    w.activity();
    w.net.tick();
    w.activity();
    let moves = w.moves();
    assert_eq!(
        (moves[0].state, moves[0].holds()),
        (PlatformActivityState::Failed, true),
        "failed by height, and holding"
    );
    (w, a)
}

fn record_of(kind: MoveKind, id: &str, at: u64) -> MoveRecord {
    let mut m = record(id, at, &[("a", 4)]);
    m.kind = kind;
    m
}

fn seen_at(height: u64, nonce: u32) -> Seen {
    seen(height, &[("a", Some(nonce))])
}
