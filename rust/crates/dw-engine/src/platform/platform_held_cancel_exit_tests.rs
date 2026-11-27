//! #1006: "Move to Dash wallet" beside a held cancel, and beside a held
//! withdrawal or contact payout too small to cancel. Beside a cancel C of a
//! withdrawal or payout W, the exit X signs C's inputs at C's nonces and
//! pays an address W does not, so each withdrawal leaves its own document:
//! X's present, X is done; W's present, W is done; every withdrawal's
//! proved absent at or above the nonces' proof, C is done.

use super::*;

/// A version whose transfer minimum is above every held cancel's output:
/// a held cancel's exact re-send is refused for good, and no held
/// withdrawal or payout of these tests has a cancel.
fn raised_transfer_minimum() -> &'static PlatformVersion {
    let mut v = PlatformVersion::latest().clone();
    v.dpp.state_transitions.address_funds.min_output_amount = 100 * DASH;
    Box::leak(Box::new(v))
}

/// A Core address of this network that is not the wallet's: a contact's.
fn contact_address(w: &World) -> String {
    let hash = dashcore::PubkeyHash::from_byte_array([7; 20]);
    let payload = dashcore::address::Payload::PubkeyHash(hash);
    dashcore::Address::new(w.network(), payload).to_string()
}

/// The held move `id` (the first transition sent) made a contact payout:
/// it pays a contact's address, and its document is the one Drive writes
/// for that address.
fn as_payout(w: &World, id: &str) {
    let mut p = w.record(id);
    p.kind = MoveKind::ToContact;
    p.to = contact_address(w);
    let script = withdrawal::destination(w.network(), &p.to).unwrap();
    w.net.effects.lock().unwrap()[0] = Effect::Withdrawal(script.as_bytes().to_vec());
    w.store(&[p]);
}

/// The withdrawal or payout `held` (as `as_payout` makes it when `payout`)
/// and its cancel, both held.
fn held_and_cancel(w: &World, payout: bool) -> (MoveToCoreOutcome, PlatformSendOutcome) {
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    if payout {
        as_payout(w, &held.id);
    }
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.then(Act::Lost);
    let out = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    w.net.tick();
    w.activity();
    assert!(w.record(&held.id).holds() && w.record(&out.id).holds());
    (held, out)
}

/// `held`'s held cancel sent again on its own terms and lost too, then a
/// version that refuses those terms: the cancel and its identical re-send
/// both hold, and neither can be sent again.
fn resend_held(w: &World, held: &str) -> PlatformSendOutcome {
    let q = w.cancel_quote_of(held).unwrap();
    w.net.then(Act::Lost);
    let again = w.cancel_as(held, &q.to, &w.f.grant(q.credits)).unwrap();
    w.net.tick();
    w.activity();
    assert!(w.record(&again.id).holds());
    *w.net.version.lock().unwrap() = raised_transfer_minimum();
    again
}

impl World {
    /// `DashPay::platform_move_to_core` on this world's net: the restore's
    /// Platform gate, then the quote's move.
    fn gated_exit(
        &self,
        q: &MoveToCoreQuote,
        grant: &str,
    ) -> Result<MoveToCoreOutcome, CreditsError> {
        let (to, exit_of, grant) = (q.to.clone(), q.exit_of.clone(), grant.to_owned());
        let amount = WithdrawAmount::Credits {
            credits: q.credits - q.fee_credits,
        };
        self.call(move |net, s, w, id| async move {
            s.refuse_platform_funds(id)?;
            let ask = (amount, to.as_str(), exit_of.as_deref());
            move_to_core(&*net, &s, &w, id, ask, &grant).await
        })
    }
}

/// The issue's case: a held cancel whose exact re-send a newer version
/// refuses for good. The re-send is refused with nothing signed; the move
/// quote names the cancel, pays an address the withdrawal does not, and
/// signs the cancel's inputs at its nonces. It executes: its document is
/// present, so it is done, and the withdrawal and the cancel failed, unable
/// to land. Stored as a way out, it keeps its link for older builds.
#[test]
fn a_held_cancel_whose_resend_is_refused_for_good_is_exited() {
    let w = World::new();
    let a = w.address(0);
    let (held, cancel) = held_and_cancel(&w, false);
    *w.net.version.lock().unwrap() = raised_transfer_minimum();
    let q = w.cancel_quote_of(&held.id).unwrap();
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.cancel_as(&held.id, &q.to, &grant).unwrap_err(),
        CreditsError::PreflightRefused
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 2, "no re-send signed");
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of.as_deref(), Some(cancel.id.as_str()));
    assert_ne!(q.to, w.record(&held.id).to, "its own document");
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    let sent = w.net.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[2][&a].0, sent[1][&a].0, "the cancel's nonce");
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.documents(), 1, "the exit's withdrawal");
    for id in [&held.id, &cancel.id] {
        let r = w.record(id);
        assert_eq!(
            (r.state, r.can_land),
            (PlatformActivityState::Failed, false)
        );
    }
    assert!(exit.id.starts_with(&format!("exit-of-{}-at-", cancel.id)));
    let stored = w.stored(&exit.id);
    assert_eq!(
        (&stored["kind"], &stored["exit_of"]),
        (
            &serde_json::json!("to_contact"),
            &serde_json::json!(cancel.id)
        )
    );
    assert_eq!(w.record(&exit.id).kind, MoveKind::ToCore);
    assert!(w.moves().iter().all(|m| !m.holds()));
    never_replaced(&w);
}

/// Who executed of the withdrawal or payout W, its cancel C and the exit X.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Winner {
    Exit,
    Held,
    Cancel,
}

/// W, C and X all held, then the winner's transition executes. Each
/// document read lags the nonces' proof first: nothing settles. Read at the
/// proof, the winner is done and the other two failed, unable to land,
/// never replaced; only the winner's effect is seen. With `resend`, C's
/// identical re-send C2 holds beside it, its terms refused: the exit runs
/// beside both, and C2, the newest, carries a cancel's win, C no longer
/// holding.
fn settles_for(payout: bool, winner: Winner, resend: bool) {
    let w = World::new();
    let (held, cancel) = held_and_cancel(&w, payout);
    let again = resend.then(|| resend_held(&w, &held.id));
    if let Some(c2) = &again {
        assert_eq!(w.record(&c2.id).terms(), w.record(&cancel.id).terms());
    }
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of.as_deref(), Some(cancel.id.as_str()));
    w.net.then(Act::Lost);
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    assert!(w.record(&exit.id).holds());
    let own = w.pool_address(&w.record(&cancel.id).to);
    let n = match winner {
        Winner::Held => 0,
        Winner::Cancel => 1,
        Winner::Exit => 2 + usize::from(resend),
    };
    assert!(w.net.execute_sent(n));
    // A present payout document is final whenever it is read; an absent one
    // and every exit's read count only at or above the proof.
    if !(payout && winner != Winner::Cancel) {
        w.net.exit_lag.store(5, Ordering::SeqCst);
        w.net.document_lag.store(5, Ordering::SeqCst);
        w.activity();
        w.activity();
        for id in [&held.id, &cancel.id, &exit.id]
            .into_iter()
            .chain(again.as_ref().map(|c| &c.id))
        {
            assert!(
                w.record(id).holds(),
                "{winner:?} {payout}: a read behind decides nothing"
            );
        }
        w.net.exit_lag.store(0, Ordering::SeqCst);
        w.net.document_lag.store(0, Ordering::SeqCst);
    }
    w.net.tick();
    w.activity();
    let ids = [
        (Winner::Held, &held.id),
        (Winner::Cancel, again.as_ref().map_or(&cancel.id, |c| &c.id)),
        (Winner::Exit, &exit.id),
    ];
    if again.is_some() {
        let c = w.record(&cancel.id);
        let lost = [
            PlatformActivityState::Failed,
            PlatformActivityState::Replaced,
        ];
        assert!(!c.holds() && !c.can_land, "{winner:?} {payout}");
        assert_eq!(
            c.state,
            lost[usize::from(winner == Winner::Cancel && !payout)],
            "{winner:?} {payout}"
        );
    }
    for (who, id) in ids {
        let r = w.record(id);
        let expected = match who == winner {
            true => (PlatformActivityState::Done, true),
            false => (PlatformActivityState::Failed, false),
        };
        assert_eq!(
            (r.state, r.can_land),
            expected,
            "{who:?} of {winner:?}, payout {payout}"
        );
    }
    let withdrawn = if winner == Winner::Cancel { 0 } else { 1 };
    assert_eq!(w.documents(), withdrawn, "{winner:?} {payout}");
    let cancelled = if winner == Winner::Cancel {
        DASH - w.net.fee()
    } else {
        0
    };
    assert_eq!(w.net.credits(own), cancelled, "{winner:?} {payout}");
    assert!(w.moves().iter().all(|m| !m.holds()));
    if !(resend && winner == Winner::Cancel) {
        never_replaced(&w);
    }
}

/// Beside a held withdrawal's cancel: X's document present, X is done;
/// W's, W is done; both absent at or above the proof, C is done.
#[test]
fn the_exit_of_a_held_cancel_settles_on_the_documents() {
    for winner in [Winner::Exit, Winner::Held, Winner::Cancel] {
        settles_for(false, winner, false);
    }
}

/// The same beside a held contact payout's cancel (P8): the payout's
/// document present, it is done and the contact paid once.
#[test]
fn the_exit_of_a_held_payouts_cancel_settles_on_the_documents() {
    for winner in [Winner::Exit, Winner::Held, Winner::Cancel] {
        settles_for(true, winner, false);
    }
}

/// A held cancel sent again on its own terms, the re-send held too, both
/// refused by a newer version: the quote names one of them, the exit runs
/// beside both and settles on the documents, beside a withdrawal and
/// beside a payout.
#[test]
fn the_exit_beside_a_held_cancel_and_its_resend_settles_on_the_documents() {
    for payout in [false, true] {
        for winner in [Winner::Exit, Winner::Held, Winner::Cancel] {
            settles_for(payout, winner, true);
        }
    }
}

/// Beside a held cancel only its identical re-sends leave the exit free: a
/// cancel of other terms, a cancel of another record on the same terms, or
/// a transfer on them that cancels nothing holds it, quoted before or
/// after, with nothing signed. The quote reads only cancels, as before: a
/// transfer that cancels nothing is refused by the move.
#[test]
fn an_exit_beside_a_transfer_that_is_not_its_cancels_resend_is_refused() {
    type Change = fn(&World, &MoveRecord, &mut MoveRecord);
    let changes: [Change; 3] = [
        |w, c, other| {
            other.id = format!("{}-9", c.id);
            other.to = bech32m(&w.address(5), w.network());
        },
        |_, c, other| {
            let at = c.id.rsplit_once("-at-").unwrap().1;
            other.id = format!("cancel-of-move-9-9-at-{at}-9");
            other.cancel_of = Some("move-9-9".into());
        },
        |_, _, other| {
            other.id = "move-1-9".into();
            other.cancel_of = None;
        },
    ];
    for (i, change) in changes.into_iter().enumerate() {
        let w = World::new();
        let (_, cancel) = held_and_cancel(&w, false);
        let q = w.quote(WithdrawAmount::All).unwrap();
        let c = w.record(&cancel.id);
        let mut other = c.clone();
        other.at += 1;
        change(&w, &c, &mut other);
        w.store(&[other.clone()]);
        assert_eq!(w.record(&other.id).cancel_of, other.cancel_of, "{i}");
        let refused = CreditsError::MovePending {
            id: other.id.clone(),
        };
        let grant = w.f.grant(q.credits);
        assert_eq!(w.exit_as(&q, &grant).unwrap_err(), refused, "{i}");
        if other.cancel_of.is_some() {
            assert_eq!(w.quote(WithdrawAmount::All).unwrap_err(), refused, "{i}");
        }
        assert!(w.f.unused(&grant));
        assert_eq!(w.sent_count(), 2, "{i}");
    }
}

/// A re-send of the cancel that Platform has not answered holds the quote,
/// as it holds the move: nothing is offered until it is answered.
#[test]
fn a_pending_resend_of_the_cancel_holds_the_exit() {
    let w = World::new();
    let (held, cancel) = held_and_cancel(&w, false);
    let q = w.cancel_quote_of(&held.id).unwrap();
    w.net.then(Act::Lost);
    let again = w.cancel_as(&held.id, &q.to, &w.f.grant(q.credits)).unwrap();
    let c2 = w.record(&again.id);
    assert_eq!(c2.state, PlatformActivityState::Pending);
    assert_eq!(c2.terms(), w.record(&cancel.id).terms());
    let refused = CreditsError::MovePending {
        id: again.id.clone(),
    };
    assert_eq!(w.quote(WithdrawAmount::All).unwrap_err(), refused);
}

/// The exit named on the cancel's re-send, an exit named on the cancel
/// itself held too, pays that one's address and never the withdrawal's.
/// It executes, and every other member fails, unable to land.
#[test]
fn an_exit_named_on_a_cancels_resend_pays_the_cancels_exits_address() {
    let w = World::new();
    let (held, cancel) = held_and_cancel(&w, false);
    let first = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Lost);
    let lost = w.exit_as(&first, &w.f.grant(first.credits)).unwrap();
    w.net.tick();
    w.activity();
    assert!(w.record(&lost.id).holds());
    let again = resend_held(&w, &held.id);
    // The re-send first in the store: the next quote names it.
    let mut c = w.record(&cancel.id);
    c.at = w.record(&again.id).at + 1;
    w.store(&[c]);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of.as_deref(), Some(again.id.as_str()));
    assert_eq!(q.to, first.to);
    assert_ne!(q.to, w.record(&held.id).to);
    let exit = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.documents(), 1);
    for id in [&held.id, &cancel.id, &again.id, &lost.id] {
        let r = w.record(id);
        assert_eq!(
            (r.state, r.can_land),
            (PlatformActivityState::Failed, false),
            "{id}"
        );
    }
    assert!(w.moves().iter().all(|m| !m.holds()));
}

/// The exit of a held cancel pays an address no withdrawal at its inputs
/// pays: one that does, however it is spelled, is not the move Confirm
/// showed, refused unsigned.
#[test]
fn an_exit_of_a_held_cancel_to_its_withdrawals_address_is_refused() {
    let w = World::new();
    let (held, _) = held_and_cancel(&w, false);
    let mut padded = w.record(&held.id);
    let to = padded.to.clone();
    padded.to = format!(" {to}\n");
    w.store(&[padded]);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_ne!(q.to, to);
    let grant = w.f.grant(q.credits);
    for to in [to.clone(), format!(" {to}\n")] {
        let same = MoveToCoreQuote { to, ..q.clone() };
        assert_eq!(
            w.exit_as(&same, &grant).unwrap_err(),
            CreditsError::QuoteChanged
        );
    }
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 2);
    assert_eq!(
        w.exit_as(&q, &grant).unwrap().state,
        PlatformActivityState::Done
    );
}

/// A second exit of a held cancel, the first one held too, pays the first
/// one's address, never the withdrawal's: a retry takes no new address, and
/// the documents still tell the exits from the withdrawal. The second one
/// executes: it is done, and the first, the withdrawal and the cancel
/// failed, unable to land.
#[test]
fn a_second_exit_of_a_held_cancel_pays_the_first_ones_address() {
    let w = World::new();
    let (held, cancel) = held_and_cancel(&w, false);
    let first = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Lost);
    let lost = w.exit_as(&first, &w.f.grant(first.credits)).unwrap();
    w.net.tick();
    w.activity();
    assert!(w.record(&lost.id).holds());
    let again = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(again.exit_of.as_deref(), Some(cancel.id.as_str()));
    assert_eq!(again.to, first.to);
    assert_ne!(again.to, w.record(&held.id).to);
    let exit = w.exit_as(&again, &w.f.grant(again.credits)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    assert_eq!(w.documents(), 1);
    for id in [&held.id, &cancel.id, &lost.id] {
        let r = w.record(id);
        assert_eq!(
            (r.state, r.can_land),
            (PlatformActivityState::Failed, false),
            "{id}"
        );
    }
    assert!(w.moves().iter().all(|m| !m.holds()));
}

/// The exit follows the named cancel's terms only: another transfer at
/// the key that differs holds it, as any such pair would settle nothing.
#[test]
fn an_exit_of_a_held_cancel_follows_its_terms_only() {
    let w = World::new();
    let (_, cancel) = held_and_cancel(&w, false);
    let q = w.quote(WithdrawAmount::All).unwrap();
    let mut other = w.record(&cancel.id);
    other.id = "move-1-9".into();
    other.cancel_of = None;
    other.to = bech32m(&w.address(5), w.network());
    w.store(&[other]);
    let grant = w.f.grant(q.credits);
    assert_eq!(
        w.exit_as(&q, &grant).unwrap_err(),
        CreditsError::MovePending {
            id: "move-1-9".into()
        }
    );
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 2);
}

/// A held withdrawal or contact payout too small to cancel: its cancel is
/// `credits.send_out_of_range` with nothing signed. The move quote names it
/// and signs its inputs at its nonces, and the move settles it as one
/// beside it always has: the move's document present, the move is done and
/// the held record no longer holds, the contact unpaid.
#[test]
fn a_held_item_too_small_to_cancel_is_exited() {
    let v = PlatformVersion::latest();
    let (exit, send) = (Limits::at(v).unwrap(), Limits::transfer_at(v));
    let small = send.min + send.keep(1) - 1;
    assert!(small >= exit.min, "a withdrawal of it plans");
    for payout in [false, true] {
        let w = World::new();
        let a = w.address(0);
        w.net.fund(a, 5 * DASH);
        let held = w.held_move(WithdrawAmount::Credits { credits: small });
        if payout {
            as_payout(&w, &held.id);
        }
        assert!(matches!(
            w.cancel_quote_of(&held.id),
            Err(CreditsError::SendOutOfRange { .. })
        ));
        let own = bech32m(&w.address(3), w.network());
        let grant = w.f.grant(DASH);
        assert_eq!(
            w.cancel_as(&held.id, &own, &grant).unwrap_err().code(),
            "credits.send_out_of_range"
        );
        assert!(w.f.unused(&grant));
        assert_eq!(w.sent_count(), 1, "no cancel signed");
        let q = w.quote(WithdrawAmount::All).unwrap();
        assert_eq!(
            q.exit_of.as_deref(),
            Some(held.id.as_str()),
            "payout {payout}"
        );
        let out = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
        let sent = w.net.sent();
        assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&a]);
        assert_eq!(sent[1][&a].0, sent[0][&a].0, "the same nonce");
        assert_eq!(out.state, PlatformActivityState::Done, "payout {payout}");
        assert_eq!(w.documents(), 1);
        assert!(!w.record(&held.id).holds(), "payout {payout}");
        assert!(
            w.stored(&out.id).get("exit_of").is_none(),
            "stored as before"
        );
        assert!(w.moves().iter().all(|m| !m.holds()));
    }
}

/// A move beside a held withdrawal or payout that has a cancel is as it
/// was before: unnamed, all of the held record's inputs at its nonces,
/// stored and settled as before.
#[test]
fn a_move_beside_a_cancellable_held_item_is_unchanged() {
    for payout in [false, true] {
        let w = World::new();
        let a = w.address(0);
        w.net.fund(a, 5 * DASH);
        let held = w.held_move(one());
        if payout {
            as_payout(&w, &held.id);
        }
        assert!(w.cancel_quote_of(&held.id).is_ok());
        let q = w.quote(WithdrawAmount::All).unwrap();
        assert_eq!(q.exit_of, None, "payout {payout}");
        let out = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
        let sent = w.net.sent();
        assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&a]);
        assert_eq!(sent[1][&a].0, sent[0][&a].0, "the same nonce");
        assert_eq!(out.state, PlatformActivityState::Done);
        let stored = w.stored(&out.id);
        assert_eq!(stored["kind"], "to_core");
        assert!(stored.get("exit_of").is_none());
        assert!(!w.record(&held.id).holds());
        assert!(w.moves().iter().all(|m| !m.holds()));
    }
}

/// The move recomputes whether its quote names the held record: a named
/// exit of one whose cancel is offered again at Confirm, and an unnamed
/// move beside one whose cancel is gone, are `credits.quote_changed`, with
/// nothing signed and the grant kept. Quoted again, each goes through.
#[test]
fn a_move_whose_held_item_gained_or_lost_its_cancel_is_refused() {
    let latest = PlatformVersion::latest();
    let raised = raised_transfer_minimum();
    for (quoted_at, confirmed_at) in [(raised, latest), (latest, raised)] {
        for payout in [false, true] {
            let w = World::new();
            w.net.fund(w.address(0), 5 * DASH);
            let held = w.held_move(one());
            if payout {
                as_payout(&w, &held.id);
            }
            *w.net.version.lock().unwrap() = quoted_at;
            let q = w.quote(WithdrawAmount::All).unwrap();
            let named = std::ptr::eq(quoted_at, raised);
            assert_eq!(q.exit_of.is_some(), named, "payout {payout}");
            *w.net.version.lock().unwrap() = confirmed_at;
            let grant = w.f.grant(q.credits);
            assert_eq!(
                w.exit_as(&q, &grant).unwrap_err(),
                CreditsError::QuoteChanged,
                "named {named}, payout {payout}"
            );
            assert!(w.f.unused(&grant));
            assert_eq!(w.sent_count(), 1, "nothing signed");
            assert!(w.record(&held.id).holds());
            let q = w.quote(WithdrawAmount::All).unwrap();
            assert_eq!(q.exit_of.is_some(), !named);
            let out = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
            assert_eq!(
                out.state,
                PlatformActivityState::Done,
                "named {named}, payout {payout}"
            );
        }
    }
}

/// The restore's Platform hold refuses the exit of a held cancel, or of a
/// held item too small to cancel, unsigned: its grant unredeemed and every
/// record held. Once the restore's check clears the wallet's row, the same
/// call is done. `gated_exit` composes the facade's gate and the move; that
/// the facade calls the gate for a named exit is `restore_holds_tests`' C6
/// row.
#[test]
fn an_exit_the_restore_hold_refused_goes_through_once_the_hold_clears() {
    for too_small in [false, true] {
        let w = World::new();
        let held = if too_small {
            let send = Limits::transfer_at(PlatformVersion::latest());
            w.net.fund(w.address(0), 5 * DASH);
            let credits = send.min + send.keep(1) - 1;
            w.held_move(WithdrawAmount::Credits { credits }).id
        } else {
            held_and_cancel(&w, false).1.id
        };
        let q = w.quote(WithdrawAmount::All).unwrap();
        assert_eq!(q.exit_of.as_deref(), Some(held.as_str()));
        let sent = w.sent_count();
        let grant = w.f.grant(q.credits);
        w.restore_row(true);
        match w.gated_exit(&q, &grant) {
            Err(CreditsError::Platform(PlatformError::Restored)) => {}
            other => panic!("{other:?}"),
        }
        (w.f.s.vault)
            .grant_purpose(&grant, dw_vault::GrantKind::PlatformOp, Some(&w.f.wallet.0))
            .unwrap();
        assert_eq!(w.sent_count(), sent, "nothing signed");
        assert!(w.moves().iter().all(MoveRecord::holds));
        w.restore_row(false);
        let out = w.gated_exit(&q, &grant).unwrap();
        assert_eq!(
            out.state,
            PlatformActivityState::Done,
            "too small {too_small}"
        );
        assert!(w.moves().iter().all(|m| !m.holds()));
    }
}

/// On mainnet, where new moves, sends and cancels stay off (#758), the
/// exit of a held cancel and of a held withdrawal too small to cancel pay
/// no one but the wallet and finish a record signed before: they pass.
#[test]
fn the_exit_of_a_held_cancel_passes_the_mainnet_flag() {
    for too_small in [false, true] {
        let w = World::on(crate::DashNetwork::Mainnet);
        let mut moved = held_withdrawal(&w);
        let named = if too_small {
            let send = Limits::transfer_at(PlatformVersion::latest());
            moved.inputs[0].credits = send.min + send.keep(1) - 1;
            w.store(&[moved.clone()]);
            moved.id.clone()
        } else {
            let mut c = moved.clone();
            c.id = "cancel-of-move-1-0-at-1-0".into();
            c.kind = MoveKind::Transfer;
            c.cancel_of = Some(moved.id.clone());
            c.to = bech32m(&w.address(3), Network::Mainnet);
            c.inputs[0].fee_source = false;
            c.fee_from_output = true;
            c.fee_credits = Limits::transfer_at(w.net.version()).fee(1);
            c.expected_duffs = 0;
            c.at = 2;
            w.store(&[c.clone()]);
            c.id
        };
        let q = w.quote(WithdrawAmount::All).unwrap();
        assert_eq!(
            q.exit_of.as_deref(),
            Some(named.as_str()),
            "too small {too_small}"
        );
        let out = w.exit_as(&q, &w.f.grant(q.credits)).unwrap();
        assert_eq!(
            out.state,
            PlatformActivityState::Done,
            "too small {too_small}"
        );
        assert!(w.moves().iter().all(|m| !m.holds()));
    }
}

/// #1060: beside a held payout P and its held cancel C, the exit X1 held,
/// credits deposited to the held input, then X2 by a new All quote at X1's
/// address and another amount. Both resolve to one document, present: it
/// does not say which paid, so X2, the newest, is done for the group and
/// unattributed, and X1, C and P failed, unable to land.
#[test]
fn two_exits_sharing_one_document_settle_unattributed() {
    let w = World::new();
    let (held, cancel) = held_and_cancel(&w, true);
    let first = w.quote(WithdrawAmount::All).unwrap();
    w.net.then(Act::Lost);
    let lost = w.exit_as(&first, &w.f.grant(first.credits)).unwrap();
    w.net.tick();
    w.activity();
    let mut first_record = w.record(&lost.id);
    assert!(first_record.holds());
    first_record.at = 0;
    w.store(&[first_record]);
    w.net.fund(w.address(0), DASH);
    let again = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(again.exit_of.as_deref(), Some(cancel.id.as_str()));
    assert_eq!(again.to, first.to);
    assert_ne!(again.credits, first.credits, "other terms");
    let exit = w.exit_as(&again, &w.f.grant(again.credits)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done, "{exit:?}");
    assert!(exit.unattributed, "{exit:?}");
    let r = w.record(&exit.id);
    assert!(r.unattributed && r.group.is_some(), "{r:?}");
    for id in [&held.id, &cancel.id, &lost.id] {
        let r = w.record(id);
        assert_eq!(
            (r.state, r.can_land),
            (PlatformActivityState::Failed, false),
            "{id}"
        );
    }
    let rows = w.activity();
    let row = rows.iter().find(|r| r.id == exit.id).unwrap();
    assert!(row.unattributed, "{row:?}");
    assert!(w.moves().iter().all(|m| !m.holds()));
    never_replaced(&w);
}

#[test]
fn an_unnamed_move_checks_the_held_send_beside_its_exit() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let send = w.held_send(one(), &to);
    w.net.then(Act::Lost);
    let exit = w.exit_held().unwrap();
    w.net.tick();
    w.activity();
    let mut sent = w.record(&send.id);
    let mut exited = w.record(&exit.id);
    assert!(sent.holds() && exited.holds());
    assert!(exited.id < sent.id);
    sent.at = 0;
    exited.at = 0;
    w.store(&[sent, exited]);

    let grant = w.f.grant(5 * DASH);
    let error = w.move_to(one(), &w.to, &grant).unwrap_err();
    assert_eq!(error, CreditsError::MovePending { id: send.id });
    assert!(w.f.unused(&grant));
    assert_eq!(w.sent_count(), 2);
}

#[test]
fn a_new_cancel_refuses_a_conflicting_link_at_its_key() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let held = w.held_move(one());
    let q = w.cancel_quote_of(&held.id).unwrap();
    let mut conflict = w.record(&held.id);
    conflict.id = "conflicting-link".into();
    conflict.link_conflict = true;
    w.store(&[conflict.clone()]);

    for of in [&held.id, &conflict.id] {
        let error = CreditsError::MovePending {
            id: conflict.id.clone(),
        };
        assert_eq!(w.cancel_quote_of(of).unwrap_err(), error);
        let grant = w.f.grant(q.credits);
        assert_eq!(w.cancel_as(of, &q.to, &grant).unwrap_err(), error);
        assert!(w.f.unused(&grant));
    }
    assert_eq!(w.sent_count(), 1);
}

#[test]
fn a_cancel_resend_returns_the_equivalent_attempt_that_settled() {
    for payout in [false, true] {
        let w = World::new();
        let (held, cancel) = held_and_cancel(&w, payout);
        let mut earlier = w.record(&cancel.id);
        earlier.at = u64::MAX;
        earlier.answered = true;
        w.store(&[earlier]);

        let q = w.cancel_quote_of(&held.id).unwrap();
        let grant = w.f.grant(q.credits);
        let outcome = w.cancel_as(&held.id, &q.to, &grant).unwrap();
        assert_eq!(outcome.state, PlatformActivityState::Done, "{outcome:?}");
        assert_eq!(outcome.id, cancel.id);
        assert_eq!(w.sent_count(), 3);
        assert!(w.moves().iter().all(|m| !m.holds()));
    }
}
