//! The quote of a registration from the Platform addresses over one proved
//! read (C-funding F9): Platform's nonces, and the refusal beside a move
//! that may still execute.

use super::*;
use crate::platform::registration_platform::{RegisterPlan, register_quote};

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
