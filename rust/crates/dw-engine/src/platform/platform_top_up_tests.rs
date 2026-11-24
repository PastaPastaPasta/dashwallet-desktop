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
            top_up(&*net, &s, &w, id, (identity.as_str(), amount, None), &grant).await
        })
    }

    /// Confirms `q` for `amount` as the sheet does: with its input 0.
    fn confirm_top_up(
        &self,
        q: &TopUpFromPlatformQuote,
        amount: WithdrawAmount,
        grant: &str,
    ) -> Result<TopUpFromPlatformOutcome, CreditsError> {
        let (fee_input, grant) = (q.inputs[0].clone(), grant.to_owned());
        self.call(move |net, s, w, id| async move {
            let ask = (me(), amount, Some(&fee_input));
            top_up(&*net, &s, &w, id, (ask.0.as_str(), ask.1, ask.2), &grant).await
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

    /// The quote's move, as the host confirms it: to its `to`, the exit of
    /// its `exit_of` at its planned withdrawal, a move of no record `All`.
    fn exit(&self, q: &MoveToCoreQuote, grant: &str) -> Result<MoveToCoreOutcome, CreditsError> {
        let (to, exit_of, grant) = (q.to.clone(), q.exit_of.clone(), grant.to_owned());
        let amount = match exit_of {
            Some(_) => credits(q.credits - q.fee_credits),
            None => WithdrawAmount::All,
        };
        self.call(move |net, s, w, id| async move {
            let ask = (amount, to.as_str(), exit_of.as_deref());
            move_to_core(&*net, &s, &w, id, ask, &grant).await
        })
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
    // The fee can take all input 0 keeps: the grant covers the claim and
    // that remainder, not only the band.
    assert_eq!(q.debit_credits, 2 * DASH);
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

/// A top-up signs the fee source and claim its Confirm showed or nothing:
/// a deposit since the quote to an address before input 0, or to a Max's
/// input 0, is `credits.quote_changed` with the grant kept and nothing
/// recorded or signed. A deposit that leaves input 0 as quoted confirms,
/// and so does a confirm without the quote's input 0, as before.
#[test]
fn a_top_up_takes_the_fee_input_its_quote_named() {
    let w = World::new();
    let order = w.in_order();
    w.net.fund(order[1], 2 * DASH);
    w.net.fund(order[2], 5 * DASH);
    let grant = w.f.grant(10 * DASH);
    let refused = |r: Result<TopUpFromPlatformOutcome, CreditsError>| {
        assert_eq!(r.unwrap_err().code(), "credits.quote_changed");
        assert!(w.f.unused(&grant));
        assert_eq!(w.sent(), 0);
        assert!(w.moves().is_empty(), "nothing recorded");
    };

    // An address before input 0 is funded: it becomes input 0.
    let q = w.top_up_quote(&me(), credits(DASH)).unwrap();
    assert_eq!(q.inputs[0].address, bech32m(&order[1], w.network()));
    w.net.fund(order[0], 3 * DASH);
    refused(w.confirm_top_up(&q, credits(DASH), &grant));

    // Max over input 0: a deposit to it raises the claim.
    let max = w.top_up_quote(&me(), WithdrawAmount::All).unwrap();
    w.net.fund(order[0], DASH);
    refused(w.confirm_top_up(&max, WithdrawAmount::All, &grant));

    // A deposit that leaves input 0's address and claim as quoted.
    let q = w.top_up_quote(&me(), credits(DASH)).unwrap();
    w.net.fund(order[0], DASH);
    w.net.fund(order[2], DASH);
    let done = w.confirm_top_up(&q, credits(DASH), &grant).unwrap();
    assert_eq!(done.credits, DASH);
    assert_eq!(w.sent(), 1);
    assert_eq!(w.net.sent.lock().unwrap()[0].keys().next(), Some(&order[0]));

    // Without the quote's input 0 the same race signs the submit's plan.
    let w = World::new();
    w.net.fund(order[1], 2 * DASH);
    let q = w.top_up_quote(&me(), credits(DASH)).unwrap();
    assert_eq!(q.inputs[0].address, bech32m(&order[1], w.network()));
    w.net.fund(order[0], 3 * DASH);
    w.top_up(credits(DASH)).unwrap();
    assert_eq!(w.net.sent.lock().unwrap()[0].keys().next(), Some(&order[0]));
}

/// Max over two inputs keeps the two-input band on input 0. Confirmed as
/// its quoted credits, as the sheet and dwcli confirm it, that total would
/// fit input 0 alone at the one-input band and claim more of it: the
/// top-up signs the quote's own plan instead, on unchanged balances.
#[test]
fn a_max_top_up_confirms_its_own_quote() {
    let w = World::new();
    let order = w.in_order();
    let l = limits();
    let small = l.keep(2) - l.keep(1);
    assert!(small >= l.min_input, "a second input the plan takes");
    w.net.fund(order[0], 10 * DASH);
    w.net.fund(order[1], small);
    let max = w.top_up_quote(&me(), WithdrawAmount::All).unwrap();
    assert_eq!(max.inputs.len(), 2);
    let balances = BTreeMap::from([(order[0], 10 * DASH), (order[1], small)]);
    let one = top_up_plan(&balances, credits(max.credits), &l).unwrap();
    assert_eq!(
        one.inputs.len(),
        1,
        "the same total as an amount: one input"
    );
    let grant = w.f.grant(max.debit_credits);
    let done = w
        .confirm_top_up(&max, credits(max.credits), &grant)
        .unwrap();
    assert_eq!(done.credits, max.credits);
    let sent = w.net.sent.lock().unwrap()[0].clone();
    let claims: Vec<_> = sent
        .iter()
        .map(|(a, (_, c))| (bech32m(a, w.network()), *c))
        .collect();
    let quoted: Vec<_> = max
        .inputs
        .iter()
        .map(|i| (i.address.clone(), i.credits))
        .collect();
    assert_eq!(claims, quoted, "the quote's inputs and claims");
}

/// A quoted fee input that is not a Platform address is refused before
/// anything is read.
#[test]
fn a_top_up_with_a_malformed_fee_input_is_refused() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let mut q = w.top_up_quote(&me(), credits(DASH)).unwrap();
    q.inputs[0].address = "not-an-address".into();
    let grant = w.f.grant(10 * DASH);
    let refused = w.confirm_top_up(&q, credits(DASH), &grant).unwrap_err();
    assert_eq!(refused.code(), "invalid_argument");
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent(), 0);
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
    w.net.fund(a, 5 * DASH);
    w.top_up_quote(&me(), credits(DASH)).unwrap();
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

/// A node's signature error proves nothing about a top-up (it signs no
/// identity key, and the SDK checked its witnesses): it stays pending and
/// able to land. Only the SDK's structure check releases it.
#[test]
fn a_node_signature_error_holds_the_top_up() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::SignatureRefused);
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(
        (out.state, out.may_land),
        (PlatformActivityState::Pending, true)
    );
    assert_eq!(
        w.top_up(credits(DASH)).unwrap_err().code(),
        "credits.move_pending"
    );
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::Invalid);
    assert!(w.top_up(credits(DASH)).is_err());
    let moves = w.moves();
    assert_eq!(
        (moves[0].state, moves[0].can_land),
        (PlatformActivityState::Failed, false)
    );
    w.top_up_quote(&me(), credits(DASH)).unwrap();
}

/// A top-up whose inputs could not pay their own "Move to Dash wallet"
/// would leave a hold with no way out: refused before anything is written,
/// with the grant unredeemed.
#[test]
fn a_top_up_its_exit_cannot_pay_is_refused() {
    let w = World::new();
    let l = limits();
    let a = w.address(0);
    let balance = l.keep(1) + 2 * l.min;
    w.net.fund(a, balance);
    let exit = Limits::at(PlatformVersion::latest()).unwrap();
    assert!(plan(&[(a, balance)].into(), WithdrawAmount::All, &exit).is_err());
    let refused = w.top_up_quote(&me(), credits(l.min)).unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");
    let grant = w.f.grant(10 * DASH);
    let refused = w.top_up_as(&me(), credits(l.min), &grant).unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");
    assert!(w.f.unused(&grant));
    assert!(w.moves().is_empty(), "nothing written");
    assert_eq!(w.sent(), 0);
}

/// Exits at one key and one payout share a document id: when it is
/// present, one of them is done (the newest answered, else the oldest),
/// the rest and the top-up failed, none replaced.
#[test]
fn several_present_exits_settle_one() {
    let failed_top_up = || {
        let mut m = record_of(MoveKind::ToIdentity, "t", 1);
        m.state = PlatformActivityState::Failed;
        m.seen_at = Some(5);
        m
    };
    let present: Exits = [("x".to_owned(), true), ("y".to_owned(), true)].into();
    let states = |moves: &[MoveRecord]| -> Vec<_> { moves.iter().map(|m| m.state).collect() };
    let (done, failed) = (PlatformActivityState::Done, PlatformActivityState::Failed);

    let mut moves = vec![
        failed_top_up(),
        record_of(MoveKind::ToCore, "x", 2),
        record_of(MoveKind::ToCore, "y", 3),
    ];
    settle_with(&mut moves, &seen_at(20, 4), &present);
    assert_eq!(states(&moves), [failed, done, failed]);

    let mut moves = vec![
        failed_top_up(),
        record_of(MoveKind::ToCore, "x", 2),
        record_of(MoveKind::ToCore, "y", 3),
    ];
    moves[2].answered = true;
    settle_with(&mut moves, &seen_at(20, 4), &present);
    assert_eq!(states(&moves), [failed, failed, done]);
    assert!(moves.iter().all(|m| !m.holds()));
    assert!(moves.iter().all(|m| m.state == done || !m.can_land));

    // Exits of other amounts share the document, which does not say which
    // paid: the newest carries the group, unattributed.
    let mut moves = vec![
        failed_top_up(),
        record_of(MoveKind::ToCore, "x", 2),
        record_of(MoveKind::ToCore, "y", 3),
    ];
    moves[1].answered = true;
    moves[2].inputs[0].credits += 1;
    settle_with(&mut moves, &seen_at(20, 4), &present);
    assert_eq!(states(&moves), [failed, failed, done]);
    assert!(
        moves[2].group.is_some(),
        "not attributed to one exit's terms"
    );
    assert!(moves.iter().all(|m| !m.holds()));
}

/// A read whose nonce is below one a done record signed is behind what
/// this wallet already settled on: nothing is planned on it, whatever the
/// done record was.
#[test]
fn a_read_behind_a_settled_move_is_refused() {
    for kind in [MoveKind::ToIdentity, MoveKind::ToCore, MoveKind::Transfer] {
        let mut done = record_of(kind, "t", 1);
        done.state = PlatformActivityState::Done;
        let moves = [done];
        assert_eq!(
            behind(&moves, &seen_at(20, 3)),
            Err(PlatformError::Unavailable),
            "{kind:?}"
        );
        assert_eq!(behind(&moves, &seen_at(20, 4)), Ok(()), "{kind:?}");
        assert_eq!(
            behind(&moves, &seen(20, &[("a", None)])),
            Ok(()),
            "{kind:?}"
        );
    }
}

/// After a done top-up, a send planned on a read behind it is refused
/// before anything is signed: it would sign the nonce the top-up took.
#[test]
fn a_send_on_a_read_behind_a_done_top_up_is_refused() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    w.net.fee.store(1, Ordering::SeqCst);
    assert_eq!(
        w.top_up(credits(DASH)).unwrap().state,
        PlatformActivityState::Done
    );
    let sent = w.sent();
    w.net.state.lock().unwrap().get_mut(&a).unwrap().0 -= 1;
    let (_, to) = w.theirs(9);
    let grant = w.f.grant(10 * DASH);
    assert_eq!(
        w.send(credits(DASH / 10), &to, &grant).unwrap_err().code(),
        "platform.unavailable"
    );
    assert_eq!(w.sent(), sent, "nothing signed");
}

/// The fee can take more than the band, up to all input 0 keeps: the
/// grant the quote asks for covers it.
#[test]
fn a_fee_above_the_band_stays_within_the_grant() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 2 * DASH);
    let q = w.top_up_quote(&me(), credits(DASH)).unwrap();
    let fee = 4 * limits().keep(1);
    w.net.fee.store(fee, Ordering::SeqCst);
    let out = w.top_up(credits(DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    let debit = 2 * DASH - w.net.state.lock().unwrap()[&a].1;
    assert_eq!(debit, DASH + fee);
    assert!(
        debit <= q.grant.max_credits,
        "{debit} > {}",
        q.grant.max_credits
    );
}

/// A held top-up's exit takes exactly its inputs: the Core quote shows
/// that move, not one of every address, and a top-up Platform has not
/// answered holds the quote as it holds the move.
#[test]
fn the_core_quote_beside_a_held_top_up_is_its_exit() {
    let (w, a) = shortfall();
    w.net.fund(w.address(1), 3 * DASH);
    w.net.fee.store(1, Ordering::SeqCst);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.credits, 5 * DASH, "only the held input");
    assert_eq!(q.exit_of, Some(w.moves()[0].id.clone()));
    let exit = w.exit(&q, &w.f.grant(10 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(sent.last().unwrap().keys().collect::<Vec<_>>(), vec![&a]);

    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::Lost);
    assert_eq!(
        w.top_up(credits(DASH)).unwrap().state,
        PlatformActivityState::Pending
    );
    assert_eq!(
        w.quote(WithdrawAmount::All).unwrap_err().code(),
        "credits.move_pending"
    );
}

/// The held top-up lands between its exit's quote and the Confirm. Named by
/// the quote's `exit_of`, the exit no longer stands: `credits.quote_changed`,
/// nothing signed or recorded, and the grant kept, which a new Confirm
/// then spends.
#[test]
fn an_exit_after_its_top_up_landed_is_refused_and_keeps_the_grant() {
    let (w, a) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of, Some(w.moves()[0].id.clone()));
    w.net.execute_sent(0);
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit(&q, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant), "the grant kept");
    assert_eq!(w.sent(), 1, "nothing signed");
    let moves = w.moves();
    assert_eq!(moves.len(), 1, "nothing recorded");
    assert_eq!(moves[0].state, PlatformActivityState::Done);
    assert_eq!(w.net.topped_up.load(Ordering::SeqCst), DASH);

    let again = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(again.exit_of, None);
    assert!(again.credits <= q.credits);
    let moved = w.exit(&again, &grant).unwrap();
    assert_eq!(moved.state, PlatformActivityState::Done);
    assert_eq!(w.sent(), 2);
    assert_eq!(w.net.sent.lock().unwrap()[1][&a].0, 2, "after the top-up's");

    // A record pending since the quote does not hide that it changed.
    w.net.fund(w.address(1), 3 * DASH);
    w.net.then(Act::Lost);
    let (_, to) = w.theirs(9);
    let pending = w.send(credits(DASH), &to, &w.f.grant(10 * DASH)).unwrap();
    assert_eq!(pending.state, PlatformActivityState::Pending);
    let kept = w.f.grant(q.credits);
    assert_eq!(
        w.exit(&q, &kept).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&kept));
    assert_eq!(w.sent(), 3);
}

/// An exit whose top-up Activity settled is answered as changed even on a
/// read behind that top-up, so Confirm offers Back; a move on that read
/// still signs nothing.
#[test]
fn an_exit_on_a_read_behind_its_landed_top_up_is_answered_changed() {
    let (w, a) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of, Some(w.moves()[0].id.clone()));
    w.net.execute_sent(0);
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Done);
    w.net.state.lock().unwrap().get_mut(&a).unwrap().0 -= 1;
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit(&q, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant), "the grant kept");
    assert_eq!(w.sent(), 1, "nothing signed");
    assert_eq!(w.moves().len(), 1, "nothing recorded");

    let again = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(again.exit_of, None);
    assert_eq!(
        w.exit(&again, &w.f.grant(again.credits))
            .unwrap_err()
            .code(),
        "platform.unavailable"
    );
    assert_eq!(w.sent(), 1, "nothing signed on the lagging read");
}

/// An exit with nothing changed since its quote moves exactly the quoted
/// withdrawal: the quote and the move plan the same inputs, fee and
/// amount, so it neither loops on `credits.quote_changed` nor moves more.
/// Its debit is the grant the quote asked for, not one credit more, and it
/// pays only the wallet's own Core address.
#[test]
fn an_exit_reports_the_amount_it_moved() {
    let (w, a) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    w.net.fund(a, DASH);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert!(q.exit_of.is_some());
    let short = w.f.grant(q.credits - 1);
    assert_eq!(
        w.exit(&q, &short).unwrap_err().code(),
        "platform.grant_exceeded"
    );
    assert!(w.f.unused(&short));
    use dpp::dashcore::hashes::Hash;
    let theirs = dpp::dashcore::Address::new(
        w.f.s.vault.network(),
        dpp::dashcore::address::Payload::PubkeyHash(dpp::dashcore::PubkeyHash::from_byte_array(
            [7; 20],
        )),
    );
    let elsewhere = MoveToCoreQuote {
        to: theirs.to_string(),
        ..q.clone()
    };
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit(&elsewhere, &grant).unwrap_err().code(),
        "invalid_argument"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent(), 1, "nothing signed");

    let out = w.exit(&q, &grant).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(out.expected_duffs, Some(q.expected_duffs));
    let sent = w.net.sent.lock().unwrap()[1].clone();
    assert_eq!(sent.keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[&a].1, q.credits - q.fee_credits, "the quoted amount");
}

/// A named exit moves its quoted amount or nothing: when the held input
/// carries less at submit than the quote read, the plan is short, which
/// is `credits.quote_changed` before the grant is redeemed or anything
/// signed; the top-up still holds.
#[test]
fn an_exit_whose_input_fell_since_its_quote_is_refused() {
    let (w, a) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    w.net.fund(a, DASH);
    let q = w.quote(WithdrawAmount::All).unwrap();
    w.net.state.lock().unwrap().get_mut(&a).unwrap().1 -= DASH;
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit(&q, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant), "the grant kept");
    assert_eq!(w.sent(), 1, "nothing signed");
    let moves = w.moves();
    assert_eq!(moves.len(), 1, "nothing recorded");
    assert!(moves[0].holds());
}

/// A two-input held top-up: a deposit since the exit's quote never makes
/// the exit narrower. One that lets one input carry the amount is
/// `credits.quote_changed` with the grant kept; a smaller one leaves the
/// exit exactly the held inputs at the held nonces, at the quoted amount,
/// and the deposit stays on its address.
#[test]
fn a_deposit_on_one_input_of_a_two_input_exit_never_narrows_it() {
    let held_world = || {
        let w = World::new();
        let order = w.in_order();
        w.net.fund(order[0], 2 * DASH);
        w.net.fund(order[1], 5 * DASH);
        w.net.fee.store(5 * DASH, Ordering::SeqCst);
        let out = w.top_up(credits(3 * DASH)).unwrap();
        assert_eq!(out.state, PlatformActivityState::Pending);
        w.activity();
        w.net.tick();
        w.activity();
        let held = w.moves()[0].clone();
        assert!(held.holds());
        assert_eq!(held.inputs.len(), 2);
        w.net.fee.store(1, Ordering::SeqCst);
        let q = w.quote(WithdrawAmount::All).unwrap();
        assert_eq!(q.exit_of, Some(held.id.clone()));
        (w, order, held, q)
    };

    let (w, order, _, q) = held_world();
    w.net.fund(order[1], 3 * DASH);
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit(&q, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent(), 1, "nothing signed");
    assert_eq!(w.moves().len(), 1, "nothing recorded");

    let (w, order, held, q) = held_world();
    w.net.fund(order[0], DASH / 10);
    let out = w.exit(&q, &w.f.grant(q.credits)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    let sent = w.net.sent.lock().unwrap()[1].clone();
    let at: Vec<_> = (sent.iter())
        .map(|(a, (nonce, _))| (bech32m(a, w.network()), *nonce))
        .collect();
    let mut key: Vec<_> = (held.inputs.iter())
        .map(|i| (i.address.clone(), i.nonce))
        .collect();
    key.sort();
    let mut at = at;
    at.sort();
    assert_eq!(at, key, "the held inputs at the held nonces");
    let moved: u64 = sent.values().map(|(_, c)| *c).sum();
    assert_eq!(moved, q.credits - q.fee_credits, "the quoted amount");
    assert_eq!(
        w.net.state.lock().unwrap()[&order[0]].1,
        DASH / 10,
        "the deposit stays"
    );
}

/// Platform answers an exit it was sent with an input short of funds: a
/// state error, after the grant was redeemed and the inputs signed. The
/// exit is recorded and may still execute; it is not
/// `credits.quote_changed`, which only a plan refuses.
#[test]
fn an_exit_platform_refuses_after_signing_is_recorded() {
    let (w, _) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    let q = w.quote(WithdrawAmount::All).unwrap();
    let grant = w.f.grant(q.credits);
    w.net.then(Act::NotEnoughFunds);
    let out = w.exit(&q, &grant);
    assert!(!matches!(out, Err(CreditsError::QuoteChanged)), "{out:?}");
    let out = out.unwrap();
    assert_ne!(out.state, PlatformActivityState::Done);
    assert!(!w.f.unused(&grant), "redeemed");
    assert_eq!(w.sent(), 2, "signed and sent");
    let moves = w.moves();
    assert_eq!(moves.len(), 2, "recorded");
    let exit = moves.iter().find(|m| m.id == out.id).unwrap();
    assert!(exit.signed && exit.can_land);
}

/// An exit beside a held top-up that Platform's stateless checks refuse
/// for good, every input signed, fails alone: only a send's exact retry
/// releases the record it names, so the top-up still holds, its inputs
/// stay held, and the next quote is its exit again.
#[test]
fn an_exit_refused_for_good_leaves_its_top_up_held() {
    let (w, a) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    let q = w.quote(WithdrawAmount::All).unwrap();
    let top_up = w.moves()[0].id.clone();
    w.net.then(Act::Invalid);
    let refused = w.exit(&q, &w.f.grant(q.credits)).unwrap_err();
    assert_ne!(refused.code(), "credits.quote_changed");
    let moves = w.moves();
    assert_eq!(moves.len(), 2);
    let exit = moves.iter().find(|m| m.id != top_up).unwrap();
    assert!(exit.signed, "every input signed");
    assert_eq!(
        (exit.state, exit.can_land),
        (PlatformActivityState::Failed, false)
    );
    let held = moves.iter().find(|m| m.id == top_up).unwrap();
    assert!(held.holds(), "the top-up still holds");

    let again = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(again.exit_of, Some(top_up));
    let out = w.exit(&again, &w.f.grant(again.credits)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2][&a].0, sent[0][&a].0, "at the top-up's nonce");
}

/// The same race with no `exit_of`: as before it, the move is planned on the
/// read under the lock, an ordinary withdrawal of what the addresses hold.
#[test]
fn a_move_without_exit_of_after_its_top_up_landed_is_planned_anew() {
    let (w, a) = shortfall();
    let b = w.address(1);
    w.net.fund(b, DASH / 2);
    w.net.fee.store(1, Ordering::SeqCst);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.credits, 5 * DASH, "the exit: the held input only");
    w.net.execute_sent(0);
    let grant = w.f.grant(q.credits);
    let moved = w.move_all(&grant).unwrap();
    assert_eq!(moved.state, PlatformActivityState::Done);
    assert_eq!(w.sent(), 2);
    let sent = w.net.sent.lock().unwrap()[1].clone();
    assert_eq!(
        sent.keys().collect::<Vec<_>>().len(),
        2,
        "every funded address"
    );
    assert_eq!(sent[&a].0, 2, "after the top-up's");
    assert!(sent.contains_key(&b));
    assert_eq!(w.net.topped_up.load(Ordering::SeqCst), DASH);
}

/// Its exit signs exactly a held top-up's inputs: a quote of only some of
/// them (an amount the larger input carries) is refused as the move would
/// refuse it, and the quote of all of them names the top-up.
#[test]
fn a_partial_exit_of_a_two_input_top_up_is_refused_at_the_quote() {
    let w = World::new();
    let order = w.in_order();
    w.net.fund(order[0], 2 * DASH);
    w.net.fund(order[1], 5 * DASH);
    w.net.fee.store(5 * DASH, Ordering::SeqCst);
    let out = w.top_up(credits(3 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Pending);
    w.activity();
    w.net.tick();
    w.activity();
    let held = w.moves()[0].clone();
    assert!(held.holds());
    assert_eq!(held.inputs.len(), 2);
    w.net.fee.store(1, Ordering::SeqCst);
    assert_eq!(
        w.quote(credits(DASH)).unwrap_err().code(),
        "credits.move_pending"
    );
    let all = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(all.exit_of, Some(held.id.clone()));
    let exit = w.exit(&all, &w.f.grant(all.credits)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.sent(), 2);
}

/// An `exit_of` that names no held top-up is `credits.quote_changed`, with
/// nothing signed and the grant kept.
#[test]
fn an_exit_of_no_held_top_up_is_refused() {
    let (w, _) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    let mut q = w.quote(WithdrawAmount::All).unwrap();
    let grant = w.f.grant(q.credits);
    q.exit_of = Some("move-0-9".into());
    assert_eq!(
        w.exit(&q, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent(), 1);
}

/// A contact payout holds beside the held top-up an exit names, on one of
/// its addresses at another nonce and on one more (no call order here makes
/// this; a record from elsewhere can). The quote names the top-up, and the
/// exit is refused `credits.move_pending` for the payout: nothing signed or
/// recorded, the grant kept.
#[test]
fn an_exit_beside_a_held_contact_payout_is_refused_for_the_payout() {
    let (w, _) = shortfall();
    w.net.fund(w.address(1), 3 * DASH);
    w.net.fee.store(1, Ordering::SeqCst);
    let top_up = w.moves()[0].clone();
    let mut payout = top_up.clone();
    payout.id = "move-payout".into();
    payout.kind = MoveKind::ToContact;
    payout.inputs[0].nonce += 1;
    let mut other = payout.inputs[0].clone();
    other.address = bech32m(&w.address(1), w.network());
    other.nonce = 1;
    other.fee_source = false;
    payout.inputs.push(other);
    let write = move_write(&payout).unwrap();
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    (w.f.engine)
        .block_on(async move { Store::of(&s, id).unwrap().commit(vec![write]).await })
        .unwrap();
    assert!(w.moves().iter().all(MoveRecord::holds));

    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of, Some(top_up.id.clone()));
    let grant = w.f.grant(q.credits);
    let refused = w.exit(&q, &grant).unwrap_err();
    assert!(
        matches!(&refused, CreditsError::MovePending { id } if id == "move-payout"),
        "{refused:?}"
    );
    assert!(w.f.unused(&grant), "the grant kept");
    assert_eq!(w.sent(), 1, "nothing signed");
    assert_eq!(w.moves().len(), 2, "nothing recorded");
}

/// The top-up executed beside its exit, and the exit's document cannot be
/// read: both hold. The quote decides the hold as the submit does and
/// refuses; the inputs' fall is no `outgoing` row. Once the document is
/// read absent, the top-up is done and the hold clears.
#[test]
fn an_undecided_group_holds_the_quote() {
    let (w, _) = shortfall();
    w.net.fee.store(1, Ordering::SeqCst);
    w.net.documents_down.store(true, Ordering::SeqCst);
    w.net.then(Act::EarlierExecutes(0));
    let exit = w.move_all(&w.f.grant(10 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Pending);
    let held = |code: &str| assert_eq!(code, "credits.move_pending");
    held(
        w.top_up_quote(&me(), credits(DASH / 1_000))
            .unwrap_err()
            .code(),
    );
    held(w.top_up(credits(DASH / 1_000)).unwrap_err().code());
    let rows = w.activity();
    assert!(
        !rows
            .iter()
            .any(|r| r.kind == PlatformActivityKind::Outgoing),
        "{rows:?}"
    );
    assert_eq!(w.sent(), 2);

    w.net.documents_down.store(false, Ordering::SeqCst);
    let rows = w.activity();
    assert_eq!(
        find(&rows, PlatformActivityKind::ToIdentity).state,
        PlatformActivityState::Done
    );
    w.top_up_quote(&me(), credits(DASH / 1_000)).unwrap();
}

/// A top-up whose process stopped anywhere after its record (before
/// signing, after signing, after the broadcast, before its outcome was
/// stored) is settled after the restart by proofs alone. Executed, it is
/// done and credited once, and the next top-up runs. Not executed, it
/// holds (a new top-up is `credits.move_pending`, its grant unused) and,
/// failed by height, "Move to Dash wallet" of its inputs releases it with
/// nothing credited; then a top-up runs again.
#[test]
fn a_top_up_stopped_anywhere_is_settled_after_a_restart() {
    for (stop, executed) in [
        (Act::StopsBeforeSigning, false),
        (Act::StopsAfterSigning, false),
        (Act::StopsAfterBroadcast, true),
        (Act::StopsBeforeOutcome, true),
    ] {
        let w = World::new();
        let (a, b) = (w.address(0), w.address(1));
        w.net.fund(a, 3 * DASH);
        w.net.then(stop);
        assert!(w.top_up(credits(DASH)).is_err(), "{stop:?}");
        let state = |w: &World| w.moves()[0].state;
        assert_eq!(state(&w), PlatformActivityState::Pending, "{stop:?}");
        // The restart: the first read.
        w.activity();
        let topped = |w: &World| w.net.topped_up.load(Ordering::SeqCst);
        if executed {
            assert_eq!(state(&w), PlatformActivityState::Done, "{stop:?}");
            assert_eq!(topped(&w), DASH, "{stop:?}");
            let next = w.top_up(credits(DASH / 10)).unwrap();
            assert_eq!(next.state, PlatformActivityState::Done, "{stop:?}");
            assert_eq!(topped(&w), DASH + DASH / 10, "credited once each");
            continue;
        }
        assert_eq!(state(&w), PlatformActivityState::Pending, "{stop:?}");
        let grant = w.f.grant(10 * DASH);
        assert_eq!(
            w.top_up_as(&me(), credits(DASH), &grant)
                .unwrap_err()
                .code(),
            "credits.move_pending",
            "{stop:?}"
        );
        assert!(w.f.unused(&grant));
        w.net.fund(b, 9 * DASH);
        w.activity();
        assert_eq!(state(&w), PlatformActivityState::Failed, "{stop:?}");
        assert_eq!(
            w.top_up_as(&me(), credits(DASH), &grant)
                .unwrap_err()
                .code(),
            "credits.move_pending",
            "no retry of a top-up: {stop:?}"
        );
        let exit = w.move_all(&grant).unwrap();
        assert_eq!(exit.state, PlatformActivityState::Done, "{stop:?}");
        let moves = w.moves();
        assert_eq!(
            (moves[0].state, moves[0].can_land),
            (PlatformActivityState::Failed, false),
            "{stop:?}"
        );
        let sent = w.net.sent.lock().unwrap().clone();
        let last = sent.last().unwrap();
        assert_eq!(last.keys().collect::<Vec<_>>(), vec![&a], "{stop:?}");
        assert_eq!(last[&a].0, 1, "the nonce the stopped top-up recorded");
        assert_eq!(topped(&w), 0, "{stop:?}");
        // The exit leaves `a` the change of its fee estimate, so the next
        // top-up's input 0 is `a`, keeping the band: Platform meters a
        // top-up's fee, not the withdrawal's this fake charged so far.
        w.net.fee.store(limits().fee(2), Ordering::SeqCst);
        let next = w.top_up(credits(DASH)).unwrap();
        assert_eq!(next.state, PlatformActivityState::Done, "{stop:?}");
        assert_eq!(topped(&w), DASH, "{stop:?}");
    }
}

/// Off on mainnet, on everywhere else: both entry points check the
/// network first, before the identity, the read, the grant or a write.
#[test]
fn top_up_from_platform_is_off_on_mainnet() {
    assert_eq!(
        top_up_open(Network::Mainnet),
        Err(PlatformError::FeatureOff {
            feature: TOP_UP_FROM_PLATFORM.into()
        })
    );
    for on in [Network::Testnet, Network::Devnet, Network::Regtest] {
        assert_eq!(top_up_open(on), Ok(()), "{on:?}");
    }
    let w = World::new();
    assert_ne!(w.network(), Network::Mainnet);
    w.net.fund(w.address(0), 5 * DASH);
    assert_eq!(
        w.top_up(credits(DASH)).unwrap().state,
        PlatformActivityState::Done
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
            let ask = (me.as_str(), credits(DASH), None);
            let top_up = top_up(&*net, &s, &wallet, id, ask, &grant);
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
                    (credits(DASH), to.as_str(), None),
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
