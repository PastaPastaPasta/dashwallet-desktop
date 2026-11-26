//! #762: a way out of a held Platform send, and the cancel that pays no
//! one. A held send's exit is "Move to Dash wallet" at its exact inputs and
//! nonces; a held withdrawal's or contact payout's is a transfer of its
//! inputs to one of the wallet's own Platform addresses. Each pair settles
//! on the withdrawal's document only: whichever executed takes the nonces,
//! the other fails, and neither is replaced.

use super::*;
use crate::DashNetwork;

fn one() -> WithdrawAmount {
    WithdrawAmount::Credits { credits: DASH }
}

impl World {
    /// The quote's move as the host confirms it: to its `to`, the exit of
    /// its `exit_of` at its planned withdrawal.
    fn exit_as(&self, q: &MoveToCoreQuote, grant: &str) -> Result<MoveToCoreOutcome, CreditsError> {
        let (to, exit_of, grant) = (q.to.clone(), q.exit_of.clone(), grant.to_owned());
        let amount = WithdrawAmount::Credits {
            credits: q.credits - q.fee_credits,
        };
        self.call(move |net, s, w, id| async move {
            let ask = (amount, to.as_str(), exit_of.as_deref());
            move_to_core(&*net, &s, &w, id, ask, &grant).await
        })
    }

    /// The exit beside the held send, quoted and confirmed.
    fn exit_held(&self) -> Result<MoveToCoreOutcome, CreditsError> {
        let q = self.quote(WithdrawAmount::All)?;
        assert!(q.exit_of.is_some(), "{q:?}");
        self.exit_as(&q, &self.f.grant(q.credits))
    }

    fn cancel_quote_of(&self, of: &str) -> Result<PlatformCancelQuote, CreditsError> {
        let of = of.to_owned();
        self.call(move |net, s, w, id| async move { cancel_quote(&*net, &s, &w, id, &of).await })
    }

    fn cancel_as(
        &self,
        of: &str,
        to: &str,
        grant: &str,
    ) -> Result<PlatformSendOutcome, CreditsError> {
        let (of, to, grant) = (of.to_owned(), to.to_owned(), grant.to_owned());
        self.call(move |net, s, w, id| async move {
            cancel(&*net, &s, &w, id, (of.as_str(), to.as_str()), &grant).await
        })
    }

    /// The cancel of `of`, quoted and confirmed.
    fn cancel_held(&self, of: &str) -> Result<PlatformSendOutcome, CreditsError> {
        let q = self.cancel_quote_of(of)?;
        self.cancel_as(of, &q.to, &self.f.grant(q.credits))
    }

    /// A move to Core of `amount` that goes out and is lost, then a later
    /// read shows it unexecuted: `Failed` by height, able to land.
    fn held_move(&self, amount: WithdrawAmount) -> MoveToCoreOutcome {
        self.net.then(Act::Lost);
        let out = self
            .move_to(amount, &self.to.clone(), &self.f.grant(20 * DASH))
            .unwrap();
        self.net.tick();
        self.activity();
        let held = self.moves().into_iter().find(|m| m.id == out.id).unwrap();
        assert_eq!(
            (held.state, held.can_land),
            (PlatformActivityState::Failed, true)
        );
        out
    }

    fn record(&self, id: &str) -> MoveRecord {
        self.moves().into_iter().find(|m| m.id == id).unwrap()
    }

    fn sent_count(&self) -> usize {
        self.net.sent.lock().unwrap().len()
    }

    /// The pool address whose bech32m is `address`.
    fn pool_address(&self, address: &str) -> PlatformAddress {
        (self.pool.iter())
            .copied()
            .find(|a| bech32m(a, self.network()) == address)
            .unwrap()
    }

    fn documents(&self) -> usize {
        self.net.documents.lock().unwrap().len()
    }

    /// `id`'s record as stored.
    fn stored(&self, id: &str) -> serde_json::Value {
        let key = format!("{MOVE_PREFIX}{id}");
        let store = self.net.store.lock().unwrap();
        let (db, scope) = store.as_ref().unwrap();
        let rows = db.settings_with_prefix(scope, MOVE_PREFIX).unwrap();
        let (_, v) = rows.into_iter().find(|(k, _)| *k == key).unwrap();
        serde_json::from_str(&v).unwrap()
    }

    /// The wallet's Platform balance as Platform holds it.
    fn platform_total(&self) -> u64 {
        self.pool.iter().map(|a| self.net.credits(*a)).sum()
    }
}

fn never_replaced(w: &World) {
    assert!(
        !(w.activity().iter()).any(|r| r.state == PlatformActivityState::Replaced),
        "never replaced"
    );
}

/// The exit of a held send signs exactly its inputs at its nonces. It
/// executes: its document is present, so it is done, the send failed and
/// unable to land, the recipient unpaid, and the hold gone.
#[test]
fn a_held_sends_exit_signs_its_nonces_and_its_document_settles_it() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    let (them, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    let exit = w.exit_held().unwrap();
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[1][&a].0, sent[0][&a].0, "the same nonce");
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.documents(), 1, "the exit's withdrawal");
    assert_eq!(w.net.credits(them), 0, "the recipient is not paid");
    let held = w.record(&send.id);
    assert_eq!(
        (held.state, held.can_land),
        (PlatformActivityState::Failed, false)
    );
    never_replaced(&w);
    // The hold is gone: a new send quotes, from what else the wallet holds.
    w.net.fund(w.address(1), 3 * DASH);
    assert_eq!(w.send_quote(one(), &to).unwrap().retry_of, None);
}

/// The other branch: the send executes after its exit was signed. The
/// nonces are taken and the exit's document is absent: the send is done,
/// the exit failed, the recipient paid once and nothing withdrawn.
#[test]
fn a_held_send_that_executes_beside_its_exit_pays_once() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    w.net.then(Act::EarlierExecutes(0));
    let exit = w.exit_held().unwrap();
    assert_eq!(exit.state, PlatformActivityState::Failed);
    assert_eq!(w.net.credits(them), DASH, "paid once");
    assert_eq!(w.documents(), 0, "nothing withdrawn");
    assert_eq!(w.record(&send.id).state, PlatformActivityState::Done);
    let exit = w.record(&exit.id);
    assert_eq!(
        (exit.state, exit.can_land),
        (PlatformActivityState::Failed, false)
    );
    never_replaced(&w);
}

/// Beside a held send, only its named exit at its exact key signs: a move
/// that names nothing and a new send are `credits.move_pending`, and an
/// exit of only part of its inputs is `credits.quote_changed`. Nothing is
/// signed and the grants are kept.
#[test]
fn only_the_named_exit_at_the_sends_key_follows_a_held_send() {
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, DASH);
    w.net.fund(b, DASH);
    let (_, to) = w.theirs(9);
    let send = w.held_send(
        WithdrawAmount::Credits {
            credits: 3 * DASH / 2,
        },
        &to,
    );
    assert_eq!(w.record(&send.id).inputs.len(), 2);
    let grant = w.f.grant(5 * DASH);
    let pending = |r: Result<MoveToCoreOutcome, CreditsError>| match r {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, send.id),
        other => panic!("{other:?}"),
    };
    pending(w.move_to(WithdrawAmount::All, &w.to.clone(), &grant));
    pending(w.move_to(one(), &w.to.clone(), &grant));
    let refused = w.send(one(), &w.theirs(8).1, &grant).unwrap_err();
    assert_eq!(refused.code(), "credits.move_pending");
    // Named, but on one of its two inputs only.
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of.as_deref(), Some(send.id.as_str()));
    let part = MoveToCoreQuote {
        credits: DASH / 2 + q.fee_credits,
        ..q.clone()
    };
    assert_eq!(
        w.exit_as(&part, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 1, "nothing else signed");
    // The whole of it exits.
    w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    assert_eq!(w.sent_count(), 2);
}

/// An exit that names a send which no longer holds is not the move Confirm
/// showed: `credits.quote_changed`, nothing signed.
#[test]
fn an_exit_of_a_send_that_landed_is_refused() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    w.held_send(one(), &to);
    let q = w.quote(WithdrawAmount::All).unwrap();
    w.lands(0);
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit_as(&q, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.credits(them), DASH);
    assert_eq!(w.sent_count(), 1);
}

/// D5: the pair settles only on its nonces taken and the exit's document
/// read at or above that proof. Both out and unanswered, the exit executes:
/// with the document read below the nonces' proof both hold; read at it,
/// the exit is done and the send failed.
#[test]
fn a_held_send_and_its_exit_hold_until_the_document_is_read_at_the_proof() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    let q = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Lost);
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    assert!(w.record(&exit.id).holds());
    assert!(w.net.execute_sent(1));
    w.net.exit_lag.store(5, Ordering::SeqCst);
    w.activity();
    w.activity();
    assert!(
        w.record(&send.id).holds(),
        "a document read behind decides nothing"
    );
    assert!(w.record(&exit.id).holds());
    w.net.exit_lag.store(0, Ordering::SeqCst);
    w.activity();
    assert_eq!(w.record(&exit.id).state, PlatformActivityState::Done);
    let send = w.record(&send.id);
    assert_eq!(
        (send.state, send.can_land),
        (PlatformActivityState::Failed, false)
    );
    assert_eq!(w.net.credits(them), 0);
    never_replaced(&w);
}

/// A node's error to the exit settles nothing: the send and the exit hold
/// through three reads, and only the exit's proof settles them.
#[test]
fn a_node_error_to_the_exit_keeps_both_held() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    let q = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Invalid);
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    for _ in 0..3 {
        w.net.tick();
        w.activity();
        assert!(w.record(&send.id).holds());
        assert!(w.record(&exit.id).holds());
    }
    assert!(w.net.execute_sent(1));
    w.activity();
    assert_eq!(w.record(&exit.id).state, PlatformActivityState::Done);
    assert!(!w.record(&send.id).holds());
}

/// The cancel of a held move to Core: a transfer of exactly its inputs at
/// its nonces and amounts, its fee from the output, to one of the wallet's
/// own Platform addresses that is no input. It executes: the move's
/// document is absent, so the cancel is done and the move failed; nothing
/// reaches Core, and the Platform balance falls by the fee alone.
#[test]
fn a_cancel_of_a_held_move_signs_its_nonces_and_pays_no_one() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    let held = w.held_move(one());
    let record = w.record(&held.id);
    let q = w.cancel_quote_of(&held.id).unwrap();
    assert_eq!(q.of, held.id);
    assert_eq!(q.credits, record.given(), "the held amount");
    let to = w.pool_address(&q.to);
    assert_ne!(to, a, "never an input");
    let before = w.platform_total();
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[1][&a], (sent[0][&a].0, record.inputs[0].credits));
    assert_eq!(out.state, PlatformActivityState::Done);
    assert!(out.fee_from_amount);
    assert_eq!(w.net.credits(to), DASH - w.net.fee());
    assert_eq!(w.platform_total(), before - w.net.fee());
    assert_eq!(w.documents(), 0, "nothing withdrawn");
    let moved = w.record(&held.id);
    assert_eq!(
        (moved.state, moved.can_land),
        (PlatformActivityState::Failed, false)
    );
    let cancel = w.record(&out.id);
    assert_eq!(cancel.cancel_of.as_deref(), Some(held.id.as_str()));
    let row = (w.activity().into_iter()).find(|r| r.id == out.id).unwrap();
    assert_eq!(row.cancels.as_deref(), Some(held.id.as_str()));
    never_replaced(&w);
}

/// The other branch: the move executes after its cancel was signed. Its
/// document is present: the move is done, the cancel failed, and the own
/// address received nothing.
#[test]
fn a_move_that_executes_beside_its_cancel_is_done() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.then(Act::EarlierExecutes(0));
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Failed);
    assert_eq!(w.documents(), 1);
    assert_eq!(w.net.credits(w.pool_address(&q.to)), 0);
    assert_eq!(w.record(&held.id).state, PlatformActivityState::Done);
    assert!(!w.record(&out.id).holds());
    never_replaced(&w);
}

/// A version whose withdrawal minimum is above every held move's amount.
fn raised_withdrawal_minimum() -> &'static PlatformVersion {
    let mut v = PlatformVersion::latest().clone();
    v.system_limits.min_withdrawal_amount = 100 * DASH;
    Box::leak(Box::new(v))
}

/// #762's follow-up: a withdrawal stranded at a version that no longer
/// builds it. Its re-exit is refused with nothing signed; the cancel builds
/// as a transfer, settles it, and nothing is paid twice: no withdrawal, the
/// amount on the wallet's own address less the fee.
#[test]
fn a_withdrawal_stranded_by_a_version_rise_is_settled_by_its_cancel() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    let held = w.held_move(one());
    *w.net.version.lock().unwrap() = raised_withdrawal_minimum();
    let grant = w.f.grant(5 * DASH);
    assert!(w.move_to(one(), &w.to.clone(), &grant).is_err());
    assert!(w.move_all(&grant).is_err());
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 1, "no re-exit signed");
    let before = w.platform_total();
    let out = w.cancel_held(&held.id).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(w.documents(), 0, "nothing withdrawn");
    assert_eq!(w.platform_total(), before - w.net.fee());
    assert!(!w.record(&held.id).holds());
    assert_eq!(w.record(&held.id).state, PlatformActivityState::Failed);
    assert!(w.moves().iter().all(|m| !m.holds()));
}

/// The cancel follows a withdrawal only: beside a held send, a held top-up
/// or a move that is a held send's exit it is refused, at the quote and at
/// the Confirm, with nothing signed and the grant kept.
#[test]
fn a_cancel_beside_a_hold_that_leaves_no_document_is_refused() {
    // A held send.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    let own = bech32m(&w.address(3), w.network());
    let grant = w.f.grant(5 * DASH);
    assert_eq!(
        w.cancel_quote_of(&send.id).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert_eq!(
        w.cancel_as(&send.id, &own, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    // The held send's exit, held too.
    let q = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Lost);
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    w.net.tick();
    w.activity();
    assert!(w.record(&exit.id).holds());
    let pending = |r: Result<_, CreditsError>| match r {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, send.id),
        other => panic!("{other:?}"),
    };
    pending(w.cancel_quote_of(&exit.id).map(drop));
    pending(w.cancel_as(&exit.id, &own, &grant).map(drop));
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 2, "nothing more signed");

    // A held top-up.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let mut top_up = w.record(&held.id);
    top_up.kind = MoveKind::ToIdentity;
    w.store(&[top_up]);
    let grant = w.f.grant(5 * DASH);
    assert_eq!(
        w.cancel_quote_of(&held.id).unwrap_err().code(),
        "credits.quote_changed"
    );
    let own = bech32m(&w.address(3), w.network());
    assert_eq!(
        w.cancel_as(&held.id, &own, &grant).unwrap_err().code(),
        "credits.quote_changed"
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 1);
}

/// D4: the cancel pays one of the wallet's own addresses that it does not
/// spend: an input or an address not the wallet's is refused before
/// anything is signed; the quote's amount is the held record's.
#[test]
fn a_cancel_pays_only_an_own_address_it_does_not_spend() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    let held = w.held_move(one());
    let grant = w.f.grant(5 * DASH);
    for to in [bech32m(&a, w.network()), w.theirs(9).1] {
        assert_eq!(
            w.cancel_as(&held.id, &to, &grant).unwrap_err().code(),
            "invalid_argument",
            "{to}"
        );
    }
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 1);
    assert!(w.record(&held.id).holds());
    // Another own address it does not spend.
    let to = bech32m(&w.address(4), w.network());
    let out = w.cancel_as(&held.id, &to, &grant).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(out.amount_credits, DASH);
}

/// D4: the cancel pays the wallet itself, so it runs under the move's own
/// flow: a `PlatformAddressMove` lease cancels, as it moves.
#[test]
fn a_cancel_runs_under_the_moves_own_flow() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    let lease =
        w.f.flow(FlowKind::PlatformAddressMove, vec![w.f.grant(q.credits)]);
    let out = w.cancel_as(&held.id, &q.to, &lease).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    // Another flow's lease never cancels.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    let grants = vec![w.f.grant(q.credits)];
    let source = Some(crate::platform::PaymentSource::Platform);
    let lease = (w.f.engine)
        .block_on(
            w.f.s
                .begin_flow(w.f.wallet, FlowKind::ContactPayFromBalance, grants, source),
        )
        .unwrap();
    assert_eq!(
        w.cancel_as(&held.id, &q.to, &lease).unwrap_err().code(),
        "platform.grant_invalid"
    );
    assert_eq!(w.sent_count(), 1);
}

/// A node's error to the cancel settles nothing: the move and its cancel
/// hold through three reads, and only the proof of one settles them.
#[test]
fn a_node_error_to_the_cancel_keeps_both_held() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.then(Act::SignatureRefused);
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    for _ in 0..3 {
        w.net.tick();
        w.activity();
        assert!(w.record(&held.id).holds());
        assert!(w.record(&out.id).holds());
    }
    assert!(w.net.execute_sent(0));
    w.activity();
    assert_eq!(w.record(&held.id).state, PlatformActivityState::Done);
    assert!(!w.record(&out.id).holds());
}

/// The cancel signs exactly what each input gave the held record: once an
/// input carries less, it is not the cancel Confirm showed, and nothing is
/// signed.
#[test]
fn a_cancel_whose_inputs_carry_less_than_the_held_amount_is_refused() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.state.lock().unwrap().get_mut(&a).unwrap().1 = DASH / 2;
    w.net.tick();
    let sent = w.sent_count();
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits));
    assert!(matches!(out, Err(CreditsError::QuoteChanged)), "{out:?}");
    assert_eq!(w.sent_count(), sent, "nothing signed");
}

/// A held cancel has no exit of its own: a move beside it waits, and a
/// move naming it as its exit is refused unsigned, so no group holds a
/// withdrawal, its cancel and an exit of the cancel.
#[test]
fn a_held_cancel_is_no_exit_target() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.then(Act::SignatureRefused);
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    w.net.tick();
    w.activity();
    assert!(w.record(&out.id).holds());
    let quoted = w.quote(WithdrawAmount::All);
    assert!(
        matches!(&quoted, Err(CreditsError::MovePending { id }) if *id == out.id),
        "{quoted:?}"
    );
    let sent = w.sent_count();
    let (to, id, grant) = (w.to.clone(), out.id.clone(), w.f.grant(5 * DASH));
    let named = w.call(move |net, s, wl, wid| async move {
        let ask = (WithdrawAmount::All, to.as_str(), Some(id.as_str()));
        move_to_core(&*net, &s, &wl, wid, ask, &grant).await
    });
    assert!(
        matches!(named, Err(CreditsError::QuoteChanged)),
        "{named:?}"
    );
    // A move that names nothing waits for it, as its quote does.
    let plain = w.move_to(WithdrawAmount::All, &w.to.clone(), &w.f.grant(5 * DASH));
    assert!(
        matches!(&plain, Err(CreditsError::MovePending { id }) if *id == out.id),
        "{plain:?}"
    );
    // The same cancel again, from its quote: one cancel holds at a time.
    let again = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits));
    assert!(
        matches!(&again, Err(CreditsError::MovePending { id }) if *id == out.id),
        "{again:?}"
    );
    // A cancel is no send to retry.
    let c = w.record(&out.id);
    assert!(!c.retried_by(&c.to, WithdrawAmount::All));
    assert_eq!(w.sent_count(), sent, "nothing signed");
    assert!(w.record(&held.id).holds() && w.record(&out.id).holds());
}

/// The cancel of a held contact payout: its documents decide as a payout's
/// exit does (C-funding P8), the cancel the one that executed once every
/// withdrawal's document is proved absent.
#[test]
fn a_cancel_of_a_held_contact_payout_settles_on_its_document() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let mut payout = w.record(&held.id);
    payout.kind = MoveKind::ToContact;
    w.store(&[payout]);
    let out = w.cancel_held(&held.id).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(w.documents(), 0);
    let payout = w.record(&held.id);
    assert_eq!(
        (payout.state, payout.can_land),
        (PlatformActivityState::Failed, false)
    );
    never_replaced(&w);
}

/// D1: a build before #762 reads an exit or a cancel as a contact payout,
/// whose pair it settles only on a withdrawal's document; this build reads
/// back its kind. The held send and move are stored as they were.
#[test]
fn an_exit_and_a_cancel_are_stored_as_contact_payouts() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    let exit = w.exit_held().unwrap();
    let stored = w.stored(&exit.id);
    assert_eq!(
        (&stored["kind"], &stored["exit_of"]),
        (
            &serde_json::json!("to_contact"),
            &serde_json::json!(send.id)
        )
    );
    assert_eq!(w.stored(&send.id)["kind"], "transfer");
    assert!(w.stored(&send.id).get("exit_of").is_none());
    assert_eq!(w.record(&exit.id).kind, MoveKind::ToCore);
    let row = (w.activity().into_iter())
        .find(|r| r.id == exit.id)
        .unwrap();
    assert_eq!(row.kind, PlatformActivityKind::MoveToCore);

    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let out = w.cancel_held(&held.id).unwrap();
    let stored = w.stored(&out.id);
    assert_eq!(
        (&stored["kind"], &stored["cancel_of"]),
        (
            &serde_json::json!("to_contact"),
            &serde_json::json!(held.id)
        )
    );
    assert_eq!(w.stored(&held.id)["kind"], "to_core");
    assert_eq!(w.record(&out.id).kind, MoveKind::Transfer);
    let row = (w.activity().into_iter()).find(|r| r.id == out.id).unwrap();
    assert_eq!(row.kind, PlatformActivityKind::Send);

    // A held top-up's exit is stored as itself: a build before #762 settles
    // that pair on the exit's document already (C-routes C3-9).
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let mut top_up = w.record(&held.id);
    top_up.kind = MoveKind::ToIdentity;
    w.store(&[top_up]);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of.as_deref(), Some(held.id.as_str()));
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    let stored = w.stored(&exit.id);
    assert_eq!(stored["kind"], "to_core");
    assert!(stored.get("exit_of").is_none());
}

/// What a build before #762 leaves of a way-out row it rewrites, as it
/// fails it by height: the fields it does not know dropped, its kind and
/// id kept.
fn rewritten_by_base(w: &World, id: &str) {
    let mut v = w.stored(id);
    let o = v.as_object_mut().unwrap();
    for k in ["exit_of", "cancel_of", "link_conflict"] {
        o.remove(k);
    }
    o.insert("state".into(), "failed".into());
    w.commit(vec![(format!("{MOVE_PREFIX}{id}"), Some(v.to_string()))]);
}

/// A held send's exit, held too: both may still execute.
fn held_send_and_exit(w: &World) -> (PlatformSendOutcome, MoveToCoreOutcome, PlatformAddress) {
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    let q = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Lost);
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    w.net.tick();
    w.activity();
    (send, exit, them)
}

/// A held move to Core and its cancel, held too.
fn held_move_and_cancel(w: &World) -> (MoveToCoreOutcome, PlatformSendOutcome) {
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.then(Act::Lost);
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    w.net.tick();
    w.activity();
    (held, out)
}

fn settled_for(w: &World, winner: &str, loser: &str) {
    assert_eq!(w.record(winner).state, PlatformActivityState::Done);
    let l = w.record(loser);
    assert_eq!(
        (l.state, l.can_land),
        (PlatformActivityState::Failed, false)
    );
}

/// D1 H1, H2: a way out's id names what it is the way out of, so a row a
/// build before #762 rewrote (dropping `exit_of` or `cancel_of`) reads back
/// linked, and its pair settles as before, either way.
#[test]
fn a_way_out_rewritten_by_an_older_build_keeps_its_link() {
    for exit_executes in [false, true] {
        let w = World::new();
        let (send, exit, them) = held_send_and_exit(&w);
        assert_eq!(way_out_of(&exit.id), Some((Link::Exit, send.id.as_str())));
        rewritten_by_base(&w, &exit.id);
        assert!(w.stored(&exit.id).get("exit_of").is_none());
        let e = w.record(&exit.id);
        assert_eq!(
            (e.kind, e.exit_of.as_deref(), e.link_conflict),
            (MoveKind::ToCore, Some(send.id.as_str()), false)
        );
        assert!(w.net.execute_sent(if exit_executes { 1 } else { 0 }));
        w.net.tick();
        w.activity();
        if exit_executes {
            settled_for(&w, &exit.id, &send.id);
            assert_eq!(w.net.credits(them), 0);
        } else {
            settled_for(&w, &send.id, &exit.id);
            assert_eq!(w.documents(), 0);
        }
        never_replaced(&w);
    }
    for move_executes in [false, true] {
        let w = World::new();
        let (held, out) = held_move_and_cancel(&w);
        assert_eq!(way_out_of(&out.id), Some((Link::Cancel, held.id.as_str())));
        rewritten_by_base(&w, &out.id);
        let c = w.record(&out.id);
        assert_eq!(
            (c.kind, c.cancel_of.as_deref(), c.link_conflict),
            (MoveKind::Transfer, Some(held.id.as_str()), false)
        );
        assert!(w.net.execute_sent(if move_executes { 0 } else { 1 }));
        w.net.tick();
        w.activity();
        if move_executes {
            settled_for(&w, &held.id, &out.id);
        } else {
            settled_for(&w, &out.id, &held.id);
        }
        never_replaced(&w);
    }
}

/// D1 H1: link fields that disagree with the id leave the row a contact
/// payout with no link, kept so: its pair holds unless a withdrawal's
/// document settles it.
#[test]
fn a_way_out_whose_fields_disagree_with_its_id_holds() {
    for exit_executes in [false, true] {
        let w = World::new();
        let (send, exit, _) = held_send_and_exit(&w);
        let mut v = w.stored(&exit.id);
        v["exit_of"] = "move-1-0".into();
        w.commit(vec![(
            format!("{MOVE_PREFIX}{}", exit.id),
            Some(v.to_string()),
        )]);
        let e = w.record(&exit.id);
        assert_eq!(
            (
                e.kind,
                e.exit_of.clone(),
                e.cancel_of.clone(),
                e.link_conflict
            ),
            (MoveKind::ToContact, None, None, true)
        );
        assert!(!e.pays_contact());
        assert!(w.net.execute_sent(if exit_executes { 1 } else { 0 }));
        for _ in 0..2 {
            w.net.tick();
            w.activity();
        }
        assert!(w.record(&exit.id).link_conflict, "kept across writes");
        if exit_executes {
            settled_for(&w, &exit.id, &send.id);
        } else {
            assert!(w.record(&send.id).holds() && w.record(&exit.id).holds());
        }
    }
}

fn held_of(kind: MoveKind, id: &str, at: u64) -> MoveRecord {
    let mut m = record(id, at, &[("a", 4)]);
    m.kind = kind;
    m.state = PlatformActivityState::Failed;
    m.seen_at = Some(5);
    m
}

fn cancel_of(of: &str, id: &str, at: u64) -> MoveRecord {
    let mut m = record(id, at, &[("a", 4)]);
    m.kind = MoveKind::Transfer;
    m.to = "own".into();
    m.fee_from_output = true;
    m.cancel_of = Some(of.into());
    m
}

fn states(moves: &[MoveRecord]) -> Vec<(PlatformActivityState, bool)> {
    moves.iter().map(|m| (m.state, m.can_land)).collect()
}

const DONE: (PlatformActivityState, bool) = (PlatformActivityState::Done, true);
const LOST: (PlatformActivityState, bool) = (PlatformActivityState::Failed, false);

/// D5 and D6 at the settle: a withdrawal and its cancel, or a send and its
/// exit, settle only on the nonces reached and the document known; the
/// document present names the withdrawal, absent the other; never replaced.
#[test]
fn a_pair_settles_on_the_nonces_and_the_document_only() {
    let pair = || vec![held_of(MoveKind::ToCore, "w", 1), cancel_of("w", "c", 2)];
    let reached = seen(20, &[("a", Some(4))]);
    let unreached = seen(20, &[("a", Some(3))]);
    let absent: Exits = [("w".to_owned(), false)].into();
    let present: Exits = [("w".to_owned(), true)].into();

    let mut moves = pair();
    settle_with(&mut moves, &unreached, &absent);
    assert!(moves.iter().all(MoveRecord::holds), "unreached");
    settle_with(&mut moves, &reached, &Exits::new());
    assert!(moves.iter().all(MoveRecord::holds), "the document unknown");
    settle_with(&mut moves, &reached, &absent);
    assert_eq!(states(&moves), [LOST, DONE]);

    let mut moves = pair();
    settle_with(&mut moves, &reached, &present);
    assert_eq!(states(&moves), [DONE, LOST]);

    // A send and its exit.
    let mut send = held_of(MoveKind::Transfer, "s", 1);
    send.to = "them".into();
    let exit = held_of(MoveKind::ToCore, "x", 2);
    let mut moves = vec![send.clone(), exit.clone()];
    settle_with(&mut moves, &reached, &[("x".to_owned(), false)].into());
    assert_eq!(states(&moves), [DONE, LOST]);
    let mut moves = vec![send, exit];
    settle_with(&mut moves, &reached, &[("x".to_owned(), true)].into());
    assert_eq!(states(&moves), [LOST, DONE]);
}

/// D2: a group with two members that leave no document and differ (a send
/// and a cancel; no call signs such a pair) settles nothing, with or
/// without a withdrawal beside them: every member keeps holding.
#[test]
fn two_members_that_leave_no_document_and_differ_settle_nothing() {
    let reached = seen(20, &[("a", Some(4))]);
    let mut send = held_of(MoveKind::Transfer, "s", 1);
    send.to = "them".into();
    let c = cancel_of("w", "c", 3);
    let w = held_of(MoveKind::ToCore, "w", 2);
    let mut moves = vec![send.clone(), w, c.clone()];
    settle_with(&mut moves, &reached, &[("w".to_owned(), false)].into());
    assert!(moves.iter().all(MoveRecord::holds));
    let mut moves = vec![send, c];
    settle_with(&mut moves, &reached, &Exits::new());
    assert!(moves.iter().all(MoveRecord::holds));
}

/// A send, its identical retry and the exit: the exit's document absent,
/// the newest send is done and the other reads replaced by it (it is not
/// the exit), the exit failed.
#[test]
fn a_send_its_retry_and_the_exit_settle_as_one_send() {
    let reached = seen(20, &[("a", Some(4))]);
    let mut send = held_of(MoveKind::Transfer, "s", 1);
    send.to = "them".into();
    let mut retry = send.clone();
    retry.id = "r".into();
    retry.at = 3;
    let exit = held_of(MoveKind::ToCore, "x", 2);
    let mut moves = vec![send, exit, retry];
    settle_with(&mut moves, &reached, &[("x".to_owned(), false)].into());
    assert_eq!(
        moves.iter().map(|m| m.state).collect::<Vec<_>>(),
        [
            PlatformActivityState::Replaced,
            PlatformActivityState::Failed,
            PlatformActivityState::Done
        ]
    );
    assert!(moves.iter().all(|m| !m.holds()));
}

/// D5 for a payout: its cancel settles it only once the nonces are reached
/// and every withdrawal's document is known; present, the payout is done.
#[test]
fn a_payout_and_its_cancel_settle_on_the_nonces_and_the_documents() {
    let pair = || vec![held_of(MoveKind::ToContact, "p", 1), cancel_of("p", "c", 2)];
    let reached = seen(20, &[("a", Some(4))]);
    let unreached = seen(20, &[("a", Some(3))]);
    let absent: Documents = [("p".to_owned(), false)].into();

    let mut moves = pair();
    settle_payouts(&mut moves, &unreached, &absent);
    assert!(moves.iter().all(MoveRecord::holds), "unreached");
    settle_payouts(&mut moves, &reached, &Documents::new());
    assert!(moves.iter().all(MoveRecord::holds), "the document unknown");
    settle_payouts(&mut moves, &reached, &absent);
    assert_eq!(states(&moves), [LOST, DONE]);

    let mut moves = pair();
    settle_payouts(&mut moves, &reached, &[("p".to_owned(), true)].into());
    assert_eq!(states(&moves), [DONE, LOST]);

    // Without a cancel, every document absent fails every member (P8).
    let mut moves = vec![held_of(MoveKind::ToContact, "p", 1)];
    settle_payouts(&mut moves, &reached, &absent);
    assert_eq!(states(&moves), [LOST]);

    // Two inputs, and another write took one of their nonces: neither
    // executed nor can, so both fail, the payout's document absent.
    let two = |id: &str, at| {
        let mut m = record(id, at, &[("a", 4), ("b", 7)]);
        m.state = PlatformActivityState::Failed;
        m.seen_at = Some(5);
        m
    };
    let mut payout = two("p", 1);
    payout.kind = MoveKind::ToContact;
    let mut cancel = two("c", 2);
    cancel.kind = MoveKind::Transfer;
    cancel.cancel_of = Some("p".into());
    let partly = seen(20, &[("a", Some(4)), ("b", Some(6))]);
    let mut moves = vec![payout.clone(), cancel.clone()];
    settle_payouts(&mut moves, &partly, &Documents::new());
    assert!(moves.iter().all(MoveRecord::holds), "the document unknown");
    settle_payouts(&mut moves, &partly, &absent);
    assert_eq!(states(&moves), [LOST, LOST]);
}

/// While a pair waits on a document after its nonces were proved taken,
/// the inputs' fall is no row of its own: the member that settles carries
/// it, and no `unseen` is left to swallow a later fall.
#[test]
fn a_pair_waiting_on_its_document_shows_no_outgoing_row() {
    let no_outgoing = |w: &World| {
        w.net.exit_lag.store(0, Ordering::SeqCst);
        w.net.tick();
        w.activity();
        let rows = w.activity();
        assert!(
            !rows
                .iter()
                .any(|r| r.kind == PlatformActivityKind::Outgoing),
            "{rows:?}"
        );
        assert!(
            (w.moves().iter())
                .flat_map(|m| &m.inputs)
                .all(|i| i.unseen == 0)
        );
    };
    for exit_executes in [false, true] {
        let w = World::new();
        let (send, exit, _) = held_send_and_exit(&w);
        w.net.exit_lag.store(5, Ordering::SeqCst);
        assert!(w.net.execute_sent(if exit_executes { 1 } else { 0 }));
        w.net.tick();
        w.activity();
        assert!(w.record(&send.id).holds() && w.record(&exit.id).holds());
        no_outgoing(&w);
        assert!(!w.record(&send.id).holds() && !w.record(&exit.id).holds());
    }
    for move_executes in [false, true] {
        let w = World::new();
        let (held, out) = held_move_and_cancel(&w);
        w.net.exit_lag.store(5, Ordering::SeqCst);
        assert!(w.net.execute_sent(if move_executes { 0 } else { 1 }));
        w.net.tick();
        w.activity();
        assert!(w.record(&held.id).holds() && w.record(&out.id).holds());
        no_outgoing(&w);
        assert!(!w.record(&held.id).holds() && !w.record(&out.id).holds());
    }
}

/// Q2: a new send whose inputs could not pay their own "Move to Dash
/// wallet" is refused at its quote and again at its Confirm (here a
/// version rise between them), with nothing written or signed and the
/// grant kept. An exact retry of a held send is not checked.
#[test]
fn a_new_send_with_no_possible_exit_is_refused() {
    let exit = Limits::at(PlatformVersion::latest()).unwrap();
    let send = Limits::transfer_at(PlatformVersion::latest());
    let w = World::new();
    let a = w.address(0);
    let balance = exit.min + exit.keep(1) - 1;
    assert!(balance >= send.min + send.keep(1), "a send plans");
    w.net.fund(a, balance);
    let (_, to) = w.theirs(9);
    let amount = WithdrawAmount::Credits { credits: send.min };
    assert_eq!(
        w.send_quote(amount, &to).unwrap_err().code(),
        "platform.insufficient_credits"
    );
    // Quoted with room, confirmed at a version whose exit needs more.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let q = w.send_quote(one(), &to).unwrap();
    *w.net.version.lock().unwrap() = raised_withdrawal_minimum();
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.confirm(&q.operation, &grant).unwrap_err().code(),
        "platform.insufficient_credits"
    );
    assert!(w.f.unused(&grant));
    assert!(w.moves().is_empty(), "nothing written");
    assert_eq!(w.sent_count(), 0);
    // A held send's exact retry still re-signs.
    *w.net.version.lock().unwrap() = PlatformVersion::latest();
    let held = w.held_send(one(), &to);
    *w.net.version.lock().unwrap() = raised_withdrawal_minimum();
    let q = w.retry_quote(one(), &to, Some(&held.id)).unwrap();
    assert_eq!(q.retry_of.as_deref(), Some(held.id.as_str()));
    w.confirm(&q.operation, &w.f.grant(q.credits)).unwrap();
    assert_eq!(w.sent_count(), 2);
}

/// The mainnet flag (#758) stays off for new sends and moves; the exit of
/// a held send and the cancel of a held move are exits of a record signed
/// before, so they pass; a cancel with nothing held is off.
#[test]
fn the_exit_and_the_cancel_pass_the_mainnet_flag() {
    let off = |r: Result<(), CreditsError>| match r {
        Err(CreditsError::Platform(PlatformError::FeatureOff { feature })) => {
            assert_eq!(feature, PLATFORM_ADDRESS_SENDS)
        }
        other => panic!("{other:?}"),
    };
    let w = World::on(DashNetwork::Mainnet);
    assert_eq!(w.network(), Network::Mainnet);
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (them, to) = w.theirs(9);
    let mut send = record("move-1-0", 1, &[(&bech32m(&a, Network::Mainnet), 1)]);
    send.kind = MoveKind::Transfer;
    send.inputs[0].credits = DASH;
    send.inputs[0].before = 3 * DASH;
    send.inputs[0].fee_source = true;
    send.to = to.clone();
    send.state = PlatformActivityState::Failed;
    send.seen_at = Some(1);
    w.store(&[send]);
    off(w.send_quote(one(), &w.theirs(8).1).map(drop));
    let exit = w.exit_held().unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.net.credits(them), 0);
    assert!(w.moves().iter().all(|m| !m.holds()));
    off(w.cancel_quote_of("move-1-0").map(drop));
    off(w
        .cancel_as(
            "move-1-0",
            &bech32m(&w.address(3), Network::Mainnet),
            &w.f.grant(DASH),
        )
        .map(drop));

    let w = World::on(DashNetwork::Mainnet);
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let mut moved = record("move-1-0", 1, &[(&bech32m(&a, Network::Mainnet), 1)]);
    moved.inputs[0].credits = DASH;
    moved.inputs[0].before = 3 * DASH;
    moved.inputs[0].fee_source = true;
    moved.to = w.to.clone();
    moved.state = PlatformActivityState::Failed;
    moved.seen_at = Some(1);
    w.store(&[moved]);
    let out = w.cancel_held("move-1-0").unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert!(w.moves().iter().all(|m| !m.holds()));
}
