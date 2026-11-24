//! The mainnet flag of the Platform addresses' funds paths: on mainnet, at
//! a protocol version with address funds, every new send, move, quote and
//! receive address is `platform.feature_off{platform_address_sends}`
//! before anything is read, stored, redeemed or signed; a move signed
//! before (a held one) still settles, exits or is retried; every other
//! network is as before.

use super::*;
use crate::DashNetwork;

const OFF: &str = "platform_address_sends";

#[track_caller]
fn off<T: std::fmt::Debug>(r: Result<T, CreditsError>) {
    match r {
        Err(CreditsError::Platform(PlatformError::FeatureOff { feature })) if feature == OFF => {}
        other => panic!("expected feature_off{{{OFF}}}, got {other:?}"),
    }
}

fn held(id: &str, kind: MoveKind, input: (&str, u32, u64, u64), to: &str) -> MoveRecord {
    let (address, nonce, credits, before) = input;
    MoveRecord {
        id: id.into(),
        kind,
        inputs: vec![MoveInput {
            address: address.into(),
            nonce,
            credits,
            before,
            fee_source: true,
            unseen: 0,
        }],
        credits: credits + Limits::at(PlatformVersion::latest()).unwrap().fee(1),
        fee_credits: Limits::at(PlatformVersion::latest()).unwrap().fee(1),
        expected_duffs: 0,
        to: to.into(),
        fee_from_output: false,
        at: 1,
        state: PlatformActivityState::Failed,
        signed: true,
        answered: false,
        can_land: true,
        seen_at: Some(1),
        fee_taken: None,
        group: None,
        unattributed: false,
        version: Some(PlatformVersion::latest().protocol_version),
    }
}

impl World {
    fn mainnet() -> Self {
        let w = Self::on(DashNetwork::Mainnet);
        assert_eq!(w.network(), Network::Mainnet);
        assert!(PlatformVersion::latest().protocol_version >= ADDRESS_FUNDS);
        w
    }

    /// Stores `record` as the build before the flag left it.
    fn seed(&self, record: &MoveRecord) {
        self.commit(vec![move_write(record).unwrap()]);
    }

    /// A quoted new send, as a quote before the flag issued it.
    fn issued(&self, to: &str, credits: u64) -> String {
        let a = self.address(0);
        self.f.s.platform.send_ops.issue(SendOp {
            wallet: self.f.wallet,
            to: to.into(),
            amount: WithdrawAmount::Credits { credits },
            retry_of: None,
            credits,
            grant: None,
            plan: Plan {
                inputs: BTreeMap::from([(a, credits)]),
                fee_source: a,
                fee: 0,
                reserve: 0,
                fee_from_output: false,
            },
        })
    }

    fn try_receive_address(&self) -> Result<PlatformReceiveAddress, CreditsError> {
        self.call(
            |net, s, w, _| async move { Ok(receive_address(&*net, &w, s.vault.network()).await?) },
        )
    }

    fn shield_quote_all(&self) -> Result<PlatformShieldQuote, CreditsError> {
        let dp = self.f.s.dashpay(self.f.wallet);
        (self.f.engine).block_on(dp.platform_shield_quote(WithdrawAmount::All))
    }

    /// A Core address of this network that is not the wallet's.
    fn foreign_core(&self) -> String {
        use dpp::dashcore::hashes::Hash;
        dpp::dashcore::Address::new(
            self.network(),
            dpp::dashcore::address::Payload::PubkeyHash(
                dpp::dashcore::PubkeyHash::from_byte_array([7; 20]),
            ),
        )
        .to_string()
    }

    fn balance_json(&self) -> serde_json::Value {
        let dp = self.f.s.dashpay(self.f.wallet);
        let b = (self.f.engine).block_on(dp.platform_addresses()).unwrap();
        serde_json::to_value(b).unwrap()
    }

    /// Nothing read from Platform, signed, sent or stored.
    #[track_caller]
    fn untouched(&self) {
        assert_eq!(self.net.calls(), 0, "no call of the network");
        assert!(self.net.sent().is_empty(), "nothing broadcast");
        assert!(self.net.recorded_before_signing.lock().unwrap().is_empty());
        assert!(self.moves().is_empty(), "nothing stored");
    }
}

use dpp::version::feature_initial_protocol_versions::ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION as ADDRESS_FUNDS;

/// Fail-first: on mainnet at a protocol with address funds, each path is
/// refused with the flag's feature, and nothing is read, stored, redeemed,
/// signed or broadcast.
#[test]
fn every_new_send_and_move_is_off_on_mainnet() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (_, theirs) = w.theirs(9);
    let grant = w.f.grant(3 * DASH);

    off(w.quote(WithdrawAmount::All));
    off(w.move_all(&grant));
    off(w.send_quote(WithdrawAmount::Credits { credits: DASH }, &theirs));
    let op = w.issued(&theirs, DASH);
    off(w.confirm(&op, &grant));
    off(w.retry_quote(
        WithdrawAmount::Credits { credits: DASH },
        &theirs,
        Some("move-0-0"),
    ));
    off(w.try_receive_address());
    off(w.shield_quote_all());
    // A grant for a send no quote issued is refused as before.
    let dp = w.f.s.dashpay(w.f.wallet);
    let unknown = (w.f.engine).block_on(dp.platform_send_grant("op-none".into()));
    assert_eq!(unknown.unwrap_err().code(), "invalid_argument");

    w.untouched();
    assert!(w.f.unused(&grant), "the grant is not redeemed");
    // The refused send stays quoted: it was not taken.
    assert!((w.f.s.platform.send_ops).cap(&op, w.f.wallet).is_ok());
    // Reads of the balance and its activity still work.
    let balance = w.balance_json();
    assert_eq!(balance["sends_active"], false, "{balance}");
    assert_eq!(balance["sends_off"], OFF, "{balance}");
    assert!(
        w.activity()
            .iter()
            .all(|r| r.kind == PlatformActivityKind::Incoming)
    );
}

/// The same calls on a test network behave as before the flag.
#[test]
fn test_networks_send_and_move_as_before() {
    for network in [DashNetwork::Testnet, DashNetwork::Regtest] {
        let w = World::on(network.clone());
        let a = w.address(0);
        w.net.fund(a, 5 * DASH);
        let (_, theirs) = w.theirs(9);
        assert!(w.try_receive_address().is_ok(), "{network:?}");
        w.quote(WithdrawAmount::Credits { credits: DASH }).unwrap();
        let one = WithdrawAmount::Credits { credits: DASH };
        let moved = w.move_to(one, &w.to.clone(), &w.f.grant(2 * DASH)).unwrap();
        assert_eq!(moved.state, PlatformActivityState::Done, "{network:?}");
        let sent = w.send(one, &theirs, &w.f.grant(5 * DASH)).unwrap();
        assert_eq!(sent.state, PlatformActivityState::Done, "{network:?}");
        assert_eq!(w.net.sent().len(), 2, "{network:?}");
        let balance = w.balance_json();
        assert_eq!(balance["sends_active"], true, "{balance}");
        assert!(balance["sends_off"].is_null(), "{balance}");
    }
}

/// A "Move to Dash wallet" a build before the flag signed and that may
/// still execute: on mainnet its exit runs, at its very inputs and nonce,
/// even with a larger address funded since; once nothing holds, the next
/// move is off again.
#[test]
fn a_held_move_from_before_still_exits_on_mainnet() {
    let w = World::mainnet();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    w.net.fund(b, 9 * DASH);
    let address = bech32m(&a, Network::Mainnet);
    w.seed(&held(
        "move-1-0",
        MoveKind::ToCore,
        (&address, 1, 3 * DASH, 3 * DASH),
        &w.to.clone(),
    ));

    w.quote(WithdrawAmount::All).unwrap();
    let exit = w.move_all(&w.f.grant(3 * DASH)).unwrap();
    assert_eq!(exit.state, PlatformActivityState::Done);
    let sent = w.net.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].keys().collect::<Vec<_>>(),
        vec![&a],
        "its inputs only"
    );
    assert_eq!(sent[0][&a].0, 1, "at its nonce");
    assert!(w.moves().iter().all(|m| !m.holds()), "{:?}", w.moves());

    let grant = w.f.grant(3 * DASH);
    off(w.quote(WithdrawAmount::All));
    off(w.move_all(&grant));
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.sent().len(), 1, "nothing more signed");
}

/// A send a build before the flag signed and that may still execute: on
/// mainnet its exact retry ("Retry this payment") re-signs its terms; a
/// new send is still off.
#[test]
fn a_held_send_from_before_is_retried_on_mainnet() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (them, theirs) = w.theirs(9);
    let address = bech32m(&a, Network::Mainnet);
    w.seed(&held(
        "move-1-0",
        MoveKind::Transfer,
        (&address, 1, DASH, 3 * DASH),
        &theirs,
    ));

    let one = WithdrawAmount::Credits { credits: DASH };
    off(w.send_quote(one, &theirs));
    let quote = w.retry_quote(one, &theirs, Some("move-1-0")).unwrap();
    assert_eq!(quote.retry_of.as_deref(), Some("move-1-0"));
    let retried = w
        .confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert_eq!(retried.state, PlatformActivityState::Done);
    let sent = w.net.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0][&a], (1, DASH), "its very input, nonce and claim");
    assert_eq!(w.net.credits(them), DASH);

    off(w.send_quote(one, &theirs));
    assert_eq!(w.net.sent().len(), 1);
}

/// A move a build before the flag signed settles on mainnet by the proof
/// of its nonce: Activity still reads and settles it.
#[test]
fn a_pending_move_from_before_settles_on_mainnet() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (them, theirs) = w.theirs(9);
    let address = bech32m(&a, Network::Mainnet);
    let mut record = held(
        "move-1-0",
        MoveKind::Transfer,
        (&address, 1, DASH, 3 * DASH),
        &theirs,
    );
    record.state = PlatformActivityState::Pending;
    w.seed(&record);
    // It lands after all.
    assert!(w.net.execute(
        &BTreeMap::from([(a, (1, DASH))]),
        Some(a),
        Some((them, DASH))
    ));

    let row = (w.activity().into_iter())
        .find(|r| r.id == "move-1-0")
        .unwrap();
    assert_eq!(row.state, PlatformActivityState::Done, "{row:?}");
    assert_eq!(w.moves()[0].state, PlatformActivityState::Done);
    assert!(w.net.sent().is_empty(), "settling signs nothing");
}

fn identity_b58() -> String {
    dpp::prelude::Identifier::new([7; 32]).to_string(Encoding::Base58)
}

/// A top-up from Platform a build before the flag signed and that may
/// still execute: on mainnet its exit ("Move to Dash wallet") runs at its
/// very inputs and nonce to the wallet's own address; the same exit to an
/// address outside the wallet, or a move at another nonce than the
/// record's, is the flag's refusal, with nothing signed and the grant kept.
#[test]
fn a_held_top_up_exits_on_mainnet_on_its_exact_terms_only() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let address = bech32m(&a, Network::Mainnet);
    let mut top_up = held(
        "move-1-0",
        MoveKind::ToIdentity,
        (&address, 1, 2 * DASH, 3 * DASH),
        &identity_b58(),
    );
    top_up.expected_duffs = 0;
    w.seed(&top_up);

    let grant = w.f.grant(3 * DASH);
    off(w.move_to(WithdrawAmount::All, &w.foreign_core(), &grant));
    assert!(w.net.sent().is_empty());
    assert!(w.f.unused(&grant));

    let exit = w.move_all(&w.f.grant(3 * DASH)).unwrap();
    let sent = w.net.sent();
    assert_eq!(sent.len(), 1, "{exit:?}");
    assert_eq!(sent[0].keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[0][&a].0, 1, "at its nonce");
    assert_eq!(exit.state, PlatformActivityState::Done);
}

/// A held record whose nonce is not the one a move would sign at now
/// (another key): on mainnet the move is refused by the flag, not planned
/// anew.
#[test]
fn a_move_beside_a_held_record_of_another_key_is_off_on_mainnet() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let address = bech32m(&a, Network::Mainnet);
    w.seed(&held(
        "move-1-0",
        MoveKind::ToIdentity,
        (&address, 5, 2 * DASH, 3 * DASH),
        &identity_b58(),
    ));
    let grant = w.f.grant(3 * DASH);
    off(w.move_all(&grant));
    assert!(w.net.sent().is_empty());
    assert!(w.f.unused(&grant));
}

/// The retry of a held send on mainnet with another recipient or amount,
/// or of a send that settled, is the flag's refusal, with nothing signed.
#[test]
fn a_retry_on_other_terms_or_of_a_settled_send_is_off_on_mainnet() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (_, theirs) = w.theirs(9);
    let (_, other) = w.theirs(8);
    let address = bech32m(&a, Network::Mainnet);
    w.seed(&held(
        "move-1-0",
        MoveKind::Transfer,
        (&address, 1, DASH, 3 * DASH),
        &theirs,
    ));
    let mut done = held(
        "move-2-1",
        MoveKind::Transfer,
        (&address, 1, DASH, 3 * DASH),
        &theirs,
    );
    done.state = PlatformActivityState::Done;
    w.seed(&done);

    let one = WithdrawAmount::Credits { credits: DASH };
    off(w.retry_quote(one, &other, Some("move-1-0")));
    off(w.retry_quote(
        WithdrawAmount::Credits { credits: 2 * DASH },
        &theirs,
        Some("move-1-0"),
    ));
    off(w.retry_quote(WithdrawAmount::All, &theirs, Some("move-1-0")));
    off(w.retry_quote(one, &theirs, Some("move-2-1")));
    assert_eq!(w.net.calls(), 0, "refused from the store alone");
    assert!(w.net.sent().is_empty());
}

/// A hold that settles in the move's own read (its nonce taken since): on
/// mainnet the quote and the move are then the flag's refusal, never a new
/// move, with nothing signed.
#[test]
fn a_hold_that_settles_during_the_read_is_off_on_mainnet() {
    let w = World::mainnet();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let address = bech32m(&a, Network::Mainnet);
    w.seed(&held(
        "move-1-0",
        MoveKind::ToCore,
        (&address, 1, 3 * DASH, 3 * DASH),
        &w.to.clone(),
    ));
    // Another device took the record's nonce: the read shows it.
    w.net.take_nonce(a);
    off(w.quote(WithdrawAmount::All));
    let grant = w.f.grant(3 * DASH);
    off(w.move_all(&grant));
    assert!(w.net.sent().is_empty());
    assert!(w.f.unused(&grant));
}

#[test]
fn an_exit_whose_inputs_differ_from_the_held_record_is_off_on_mainnet() {
    let w = World::mainnet();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    let mut top_up = held(
        "move-1-0",
        MoveKind::ToIdentity,
        (&bech32m(&a, Network::Mainnet), 1, 2 * DASH, 3 * DASH),
        &identity_b58(),
    );
    top_up.inputs.push(MoveInput {
        address: bech32m(&b, Network::Mainnet),
        nonce: 1,
        credits: DASH,
        before: DASH,
        fee_source: false,
        unseen: 0,
    });
    top_up.expected_duffs = 0;
    w.seed(&top_up);
    let grant = w.f.grant(3 * DASH);
    off(w.move_all(&grant));
    assert!(w.net.sent().is_empty());
    assert!(w.f.unused(&grant));
}

#[test]
fn a_smaller_exit_on_part_of_the_held_inputs_is_off_on_mainnet() {
    let w = World::mainnet();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    w.net.fund(b, 3 * DASH);
    let mut moved = held(
        "move-1-0",
        MoveKind::ToCore,
        (&bech32m(&a, Network::Mainnet), 1, 3 * DASH, 3 * DASH),
        &w.to.clone(),
    );
    moved.inputs.push(MoveInput {
        address: bech32m(&b, Network::Mainnet),
        nonce: 1,
        credits: 3 * DASH,
        before: 3 * DASH,
        fee_source: false,
        unseen: 0,
    });
    w.seed(&moved);
    let grant = w.f.grant(6 * DASH);
    off(w.move_to(
        WithdrawAmount::Credits { credits: DASH },
        &w.to.clone(),
        &grant,
    ));
    assert!(w.net.sent().is_empty());
    assert!(w.net.recorded_before_signing.lock().unwrap().is_empty());
    assert!(w.f.unused(&grant));
}

#[test]
fn a_held_moves_quote_takes_what_its_exit_signs() {
    for network in [DashNetwork::Testnet, DashNetwork::Mainnet] {
        let w = World::on(network.clone());
        let (a, b) = (w.address(0), w.address(1));
        w.net.fund(a, 3 * DASH);
        w.net.fund(b, 9 * DASH);
        w.seed(&held(
            "move-1-0",
            MoveKind::ToCore,
            (&bech32m(&a, w.network()), 1, 3 * DASH, 3 * DASH),
            &w.to.clone(),
        ));
        let quote = w.quote(WithdrawAmount::All).unwrap();
        let exit = w.move_all(&w.f.grant(quote.credits)).unwrap();
        assert_eq!(exit.state, PlatformActivityState::Done, "{network:?}");
        let sent = w.net.sent();
        assert_eq!(sent.len(), 1, "{network:?}");
        assert_eq!(sent[0].keys().collect::<Vec<_>>(), vec![&a], "{network:?}");
        let exit = (w.moves().into_iter()).find(|m| m.id == exit.id).unwrap();
        let signed: u64 = sent[0].values().map(|(_, credits)| *credits).sum();
        assert_eq!(
            (quote.credits, exit.credits),
            (3 * DASH, 3 * DASH),
            "{network:?}: Confirm showed what was signed"
        );
        assert!(signed <= quote.credits, "{network:?}");
    }
}

impl World {
    /// A held top-up's exit as the host confirms it: its quote's
    /// `exit_of`, at the quote's planned withdrawal.
    fn named_exit(
        &self,
        q: &MoveToCoreQuote,
        grant: &str,
    ) -> Result<MoveToCoreOutcome, CreditsError> {
        let (to, exit_of, grant) = (q.to.clone(), q.exit_of.clone(), grant.to_owned());
        let amount = WithdrawAmount::Credits {
            credits: q.credits - q.fee_credits,
        };
        self.call(move |net, s, w, id| async move {
            move_to_core(
                &*net,
                &s,
                &w,
                id,
                (amount, to.as_str(), exit_of.as_deref()),
                &grant,
            )
            .await
        })
    }
}

/// A held top-up's exit named by its quote: on mainnet each way it no
/// longer stands (the top-up settled since the quote, the plan is not at
/// its nonces, another record holds beside it) answers the flag, with
/// nothing signed or recorded and the grant kept.
#[test]
fn a_named_top_up_exit_that_no_longer_stands_is_off_on_mainnet() {
    let setup = || {
        let w = World::mainnet();
        let a = w.address(0);
        w.net.fund(a, 3 * DASH);
        let address = bech32m(&a, Network::Mainnet);
        let top_up = held(
            "move-1-0",
            MoveKind::ToIdentity,
            (&address, 1, 3 * DASH, 3 * DASH),
            "identity",
        );
        w.seed(&top_up);
        let q = w.quote(WithdrawAmount::All).unwrap();
        assert_eq!(q.exit_of.as_deref(), Some("move-1-0"));
        (w, top_up, q)
    };
    let refused = |w: &World, q: &MoveToCoreQuote, records: usize| {
        let grant = w.f.grant(q.credits);
        off(w.named_exit(q, &grant));
        assert!(w.net.sent().is_empty(), "nothing broadcast");
        assert!(w.net.recorded_before_signing.lock().unwrap().is_empty());
        assert!(w.f.unused(&grant), "the grant kept");
        assert_eq!(w.moves().len(), records, "nothing recorded");
    };

    let (w, mut top_up, q) = setup();
    (top_up.state, top_up.can_land) = (PlatformActivityState::Done, false);
    w.seed(&top_up);
    refused(&w, &q, 1);

    let (w, mut top_up, q) = setup();
    top_up.inputs[0].nonce = 3;
    w.seed(&top_up);
    refused(&w, &q, 1);

    let (w, _, q) = setup();
    let b = w.address(1);
    w.net.fund(b, DASH);
    let other = held(
        "move-2-0",
        MoveKind::ToCore,
        (&bech32m(&b, Network::Mainnet), 1, DASH, DASH),
        &w.to.clone(),
    );
    w.seed(&other);
    refused(&w, &q, 2);
}
