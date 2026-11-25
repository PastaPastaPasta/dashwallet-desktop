//! Tests of credit_transfer.rs (C-routes D-C6, slice C6a-1): the rules at
//! every protocol version with address funds, the quote, and the write on
//! withdrawal_tests.rs's scripted Platform: the transition, the nonce it
//! shares with withdrawals, the decision by the proved nonce, and the
//! refusals that keep the grant.

use dpp::platform_value::string_encoding::Encoding;
use dpp::serialization::PlatformSerializable;
use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;
use dpp::state_transition::identity_credit_withdrawal_transition::accessors::IdentityCreditWithdrawalTransitionAccessorsV0;
use dpp::state_transition::{
    StateTransitionHasUserFeeIncrease, StateTransitionIdentityEstimatedFeeValidation,
    StateTransitionStructureValidation,
};
use dpp::version::feature_initial_protocol_versions::ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION;

use super::super::flows::FlowKind;
use super::super::journal_tests::id;
use super::super::withdrawal::document_id;
use super::super::withdrawal::tests::{Answer, DASH, FEE, ME, World, nonce_of, restart};
use super::*;

/// What a transfer Drive executes is charged on top of its amount here.
const TRANSFER_FEE: u64 = 10_000_000;

fn versions() -> impl Iterator<Item = &'static PlatformVersion> {
    (ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION..=PlatformVersion::latest().protocol_version)
        .map(|v| PlatformVersion::get(v).unwrap())
}

fn rules() -> TransferRules {
    TransferRules::at(PlatformVersion::latest()).unwrap()
}

fn me() -> String {
    id(ME).to_string(Encoding::Base58)
}

impl World {
    fn transfer_quote(
        &self,
        amount: WithdrawAmount,
    ) -> Result<WithdrawToPlatformQuote, CreditsError> {
        let me = me();
        self.call(
            move |net, s, w| async move { quote(&*net, &w, s.vault.network(), &me, amount).await },
        )
    }

    fn transfer_to(
        &self,
        to: &str,
        amount: WithdrawAmount,
        grant: &str,
    ) -> Result<WithdrawToPlatformOutcome, CreditsError> {
        let (wallet, to, grant, me) = (self.f.wallet, to.to_owned(), grant.to_owned(), me());
        self.call(move |net, s, w| async move {
            transfer(&*net, &s, &w, (wallet, &me), (&to, amount), &grant).await
        })
    }

    /// One of the wallet's own Platform addresses: the quote's.
    fn own(&self) -> String {
        self.transfer_quote(WithdrawAmount::Credits { credits: DASH })
            .unwrap()
            .to
    }

    fn send(&self, credits: u64) -> Result<WithdrawToPlatformOutcome, CreditsError> {
        let to = self.own();
        self.transfer_to(
            &to,
            WithdrawAmount::Credits { credits },
            &self.f.grant(4 * DASH),
        )
    }

    fn executed(&self) -> usize {
        self.net.transfers.lock().unwrap().len()
    }
}

fn hash(t: &StateTransition) -> String {
    hex::encode(t.transaction_id().unwrap())
}

fn unknown(t: &StateTransition) -> CreditsError {
    CreditsError::Platform(PlatformError::BroadcastUnknown { artifact: hash(t) })
}

fn landed(artifact: String) -> CreditsError {
    CreditsError::WithdrawalLanded { artifact }
}

/// C6-5: at every version with address funds, Max passes dpp's estimated
/// fee check and the structure check; the reserve covers dpp's minimum and
/// a metered bound above Platform's own per-output price; below address
/// funds the transfer is off.
#[test]
fn max_leaves_the_reserve() {
    for v in versions() {
        let r = TransferRules::at(v).unwrap();
        let fees = &v.fee_version.state_transition_min_fees;
        let min_fee = fees.credit_transfer_to_addresses + fees.address_funds_transfer_output_cost;
        assert!(r.fee >= min_fee, "{}", v.protocol_version);
        assert!(
            r.fee >= fees.identity_update + 2 * fees.address_funds_transfer_output_cost,
            "{}",
            v.protocol_version
        );
        for balance in [r.fee + r.min, 3 * DASH + 7] {
            let max = r.credits(balance, WithdrawAmount::All).unwrap();
            assert_eq!(max, balance - r.fee);
            let t = IdentityCreditTransferToAddressesTransitionV0 {
                recipient_addresses: BTreeMap::from([(PlatformAddress::P2pkh([1; 20]), max)]),
                ..Default::default()
            };
            assert!(t.validate_structure(v).is_valid());
            let t = IdentityCreditTransferToAddressesTransition::V0(t);
            assert!(t.validate_estimated_fee(balance, v).unwrap().is_valid());
        }
    }
    let below = PlatformVersion::get(ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION - 1).unwrap();
    assert_eq!(
        TransferRules::at(below).unwrap_err().code(),
        "platform.feature_off"
    );
    let r = rules();
    let code = |b, a| r.credits(b, a).unwrap_err().code();
    assert_eq!(
        code(r.fee + r.min - 1, WithdrawAmount::All),
        "platform.insufficient_credits"
    );
    let ask = |credits| WithdrawAmount::Credits { credits };
    assert_eq!(
        r.credits(DASH, ask(r.min - 1)).unwrap_err(),
        CreditsError::WithdrawalOutOfRange {
            min: r.min,
            max: DASH - r.fee
        }
    );
    assert_eq!(
        code(DASH, ask(DASH - r.fee + 1)),
        "platform.insufficient_credits"
    );
    assert_eq!(r.credits(DASH, ask(DASH - r.fee)).unwrap(), DASH - r.fee);
}

/// The quote: the plan, the wallet's receive address and the grant of the
/// amount plus the reserve.
#[test]
fn the_quote_is_the_plan_its_address_and_grant() {
    let balance = 3 * DASH + 5;
    let w = World::new(balance);
    let q = w.transfer_quote(WithdrawAmount::All).unwrap();
    let r = rules();
    assert_eq!((q.credits, q.fee_credits), (balance - r.fee, r.fee));
    assert_eq!(
        q.grant,
        GrantRequest {
            max_duffs: 0,
            max_credits: balance
        }
    );
    let receive = w
        .call(|_, s, wallet| async move {
            Ok(next_receive_address(&wallet, s.vault.network())
                .await?
                .address)
        })
        .unwrap();
    assert_eq!(q.to, receive);
    let refused = w
        .transfer_quote(WithdrawAmount::Credits { credits: 1 })
        .unwrap_err();
    assert_eq!(refused.code(), "credits.withdrawal_out_of_range");
}

/// C6-2: dpp's own construction, one recipient, no fee increase, signed
/// with the CRITICAL TRANSFER key at the engine's nonce; executed, the
/// outcome names Platform's hash of it.
#[test]
fn the_transfer_is_dpps_own_construction() {
    let balance = 2 * DASH;
    let w = World::new(balance);
    w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    let to = w.own();
    let out = w
        .transfer_to(
            &to,
            WithdrawAmount::Credits { credits: DASH },
            &w.f.grant(2 * DASH),
        )
        .unwrap();
    let sent = w.net.sent();
    let [signed] = sent.as_slice() else {
        panic!("one broadcast: {sent:?}");
    };
    let StateTransition::IdentityCreditTransferToAddresses(t) = signed else {
        panic!("a transfer to addresses");
    };
    let address = parse(&to).unwrap();
    assert_eq!(t.recipient_addresses(), &BTreeMap::from([(address, DASH)]));
    assert_eq!(t.user_fee_increase(), 0);
    assert_eq!(
        signed.signature_public_key_id(),
        Some(3),
        "the TRANSFER key"
    );
    assert_eq!(t.nonce(), 5, "one past Platform's");

    let me = w.me();
    let lease_id = w.f.flow(FlowKind::Withdraw, vec![w.f.grant(1)]);
    let lease = w.f.s.lease_for(&w.f.wallet, &lease_id).unwrap().unwrap();
    let signer = lease.identity_signer(&[0]).unwrap();
    let platforms = w.f.engine.block_on(async move {
        IdentityCreditTransferToAddressesTransition::try_from_identity(
            &me,
            BTreeMap::from([(address, DASH)]),
            0,
            &signer,
            None,
            5,
            PlatformVersion::latest(),
            None,
        )
        .await
        .unwrap()
    });
    lease.end();
    assert_eq!(
        signed.serialize_to_bytes().unwrap(),
        platforms.serialize_to_bytes().unwrap()
    );
    assert_eq!(
        out,
        WithdrawToPlatformOutcome {
            credits: DASH,
            to,
            remaining_credits: Some(balance - DASH - TRANSFER_FEE),
            transition: hash(signed),
        }
    );
    assert_eq!(w.executed(), 1);
}

/// C6-1: anything but the wallet's own Platform address is
/// `invalid_argument`, before the grant; so is the quote's refusal.
#[test]
fn transfer_to_a_foreign_address_is_refused() {
    let w = World::new(2 * DASH);
    let grant = w.f.grant(2 * DASH);
    let network = w.f.s.vault.network();
    let foreign = bech32m(&PlatformAddress::P2pkh([9; 20]), network);
    let core =
        w.f.engine
            .block_on(w.f.s.next_receive_address(w.f.wallet, None))
            .unwrap();
    let ask = WithdrawAmount::Credits { credits: DASH };
    for to in [foreign.as_str(), core.address.as_str(), "nonsense"] {
        assert_eq!(
            w.transfer_to(to, ask, &grant).unwrap_err().code(),
            "invalid_argument"
        );
    }
    let small = w.f.grant(DASH);
    assert_eq!(
        w.transfer_to(&w.own(), ask, &small).unwrap_err().code(),
        "platform.grant_exceeded"
    );
    assert!(w.f.unused(&grant) && w.f.unused(&small));
    assert!(w.net.sent().is_empty());
}

/// C6-7: with only the transfer signed at N, a used N is the transfer
/// executed, whatever the answer; a nonce below N leaves it unresolved
/// (`platform.broadcast_unknown` naming the transition, never retried).
#[test]
fn only_the_transfer_at_a_used_nonce_executed() {
    let w = World::new(10 * DASH);
    w.net.answer(Answer::LandedUnanswered);
    let out = w.send(DASH).unwrap();
    assert_eq!((out.credits, out.remaining_credits), (DASH, None));
    assert_eq!(out.transition, hash(&w.net.sent()[0]));

    w.net.answer(Answer::Unknown);
    let refused = w.send(DASH).unwrap_err();
    assert_eq!(refused, unknown(&w.net.sent()[1]));
    assert_eq!(w.executed(), 1);
}

/// C6-8: while a transfer may still execute, a second transfer is
/// `credits.move_pending{id}` naming it, its grant kept and nothing sent;
/// a withdrawal still signs at its nonce, and once that executes the
/// transfer never can.
#[test]
fn no_second_transfer_while_one_is_unresolved() {
    let w = World::new(10 * DASH);
    w.net.answer(Answer::Pending);
    let first = w.send(DASH).unwrap_err();
    let t = w.net.sent()[0].clone();
    assert_eq!(first, unknown(&t));
    let grant = w.f.grant(4 * DASH);
    let refused = w
        .transfer_to(&w.own(), WithdrawAmount::Credits { credits: DASH }, &grant)
        .unwrap_err();
    assert_eq!(refused, CreditsError::MovePending { id: hash(&t) });
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.sent().len(), 1);

    let withdrawal = w.withdraw(
        WithdrawAmount::Credits { credits: DASH },
        &w.f.grant(4 * DASH),
    );
    let sent = w.net.sent();
    assert_eq!(nonce_of(&sent[1]), nonce_of(&t), "the transfer's nonce");
    assert!(withdrawal.is_err());
    w.net.execute_pending();
    assert_eq!(
        (w.net.landed(), w.executed()),
        (0, 1),
        "the transfer, first in"
    );
}

/// C6-7 at a shared nonce: a withdrawal and a transfer both signed at N
/// and N used. The withdrawal's document there says it executed and the
/// transfer did not; none says the transfer did. Either way the next write
/// is refused once as `credits.withdrawal_landed` and the one after signs
/// past N.
#[test]
fn a_shared_nonce_is_decided_by_the_withdrawal_document() {
    let ask = WithdrawAmount::Credits { credits: DASH };
    for withdrawal_first in [true, false] {
        let w = World::new(10 * DASH);
        w.net.answer(Answer::Pending);
        if withdrawal_first {
            w.withdraw(ask, &w.f.grant(4 * DASH)).unwrap_err();
            w.send(DASH).unwrap_err();
        } else {
            w.send(DASH).unwrap_err();
            w.withdraw(ask, &w.f.grant(4 * DASH)).unwrap_err();
        }
        let sent = w.net.sent();
        let k = nonce_of(&sent[0]);
        assert_eq!(nonce_of(&sent[1]), k, "one nonce");
        w.net.execute_pending();
        let (withdrawal, transfer) = if withdrawal_first {
            (&sent[0], &sent[1])
        } else {
            (&sent[1], &sent[0])
        };
        let want = if withdrawal_first {
            assert_eq!((w.net.landed(), w.executed()), (1, 0));
            let StateTransition::IdentityCreditWithdrawal(wd) = withdrawal else {
                panic!("a withdrawal");
            };
            let script = wd.output_script().unwrap();
            landed(document_id(&id(ME), k, &script).to_string(Encoding::Base58))
        } else {
            assert_eq!((w.net.landed(), w.executed()), (0, 1));
            landed(hash(transfer))
        };
        w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
        let grant = w.f.grant(4 * DASH);
        let to = w.own();
        assert_eq!(w.transfer_to(&to, ask, &grant).unwrap_err(), want);
        assert!(w.f.unused(&grant));
        w.transfer_to(&to, ask, &grant).unwrap();
        assert_eq!(nonce_of(&w.net.sent()[2]), k + 1);
    }
}

/// C6-8: a failed transfer holds the route. Drive's CheckTx refusal (here
/// a fee above the balance) reaches the engine as no answer, and a node's
/// refusal with the nonce still free is unproved: either way every later
/// transfer is `credits.move_pending` naming it, its grant kept. A
/// withdrawal that executes at that nonce unlocks it.
#[test]
fn a_failed_transfer_keeps_the_route_held() {
    for answer in [Answer::Applied { fee: 100 * DASH }, Answer::Refused] {
        let w = World::new(10 * DASH);
        let checktx = matches!(answer, Answer::Applied { .. });
        w.net.answer(answer);
        let failed = w.send(DASH).unwrap_err();
        let t = w.net.sent()[0].clone();
        if checktx {
            assert_eq!(failed, unknown(&t));
        } else {
            assert_eq!(failed.code(), "platform.insufficient_credits");
        }
        assert_eq!(w.executed(), 0);

        w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
        let grant = w.f.grant(4 * DASH);
        let held = w
            .transfer_to(&w.own(), WithdrawAmount::Credits { credits: DASH }, &grant)
            .unwrap_err();
        assert_eq!(held, CreditsError::MovePending { id: hash(&t) });
        assert!(w.f.unused(&grant));
        assert_eq!(w.net.sent().len(), 1);

        w.withdraw(
            WithdrawAmount::Credits { credits: DASH },
            &w.f.grant(4 * DASH),
        )
        .unwrap();
        assert_eq!(nonce_of(&w.net.sent()[1]), nonce_of(&t));
        w.send(DASH).unwrap();
        assert_eq!(nonce_of(&w.net.sent()[2]), nonce_of(&t) + 1);
        assert_eq!((w.net.landed(), w.executed()), (1, 1));
    }
}

/// C6-7: an `Ok` from the broadcast is not the transfer executed; with its
/// nonce still free it is `platform.broadcast_unknown`, and it still holds
/// the route.
#[test]
fn a_successful_answer_is_never_evidence() {
    let w = World::new(10 * DASH);
    w.net.answer(Answer::LyingSuccess);
    let t = w.send(DASH).unwrap_err();
    assert_eq!(t, unknown(&w.net.sent()[0]));
    assert_eq!(w.executed(), 0);
    assert_eq!(
        w.send(DASH).unwrap_err(),
        CreditsError::MovePending {
            id: hash(&w.net.sent()[0])
        }
    );
}

/// C6-7: a nonce read older than the one the transfer was signed after
/// decides nothing, not even a refusal; read again current, the transfer
/// that executed is `credits.withdrawal_landed` once.
#[test]
fn a_lagging_read_decides_nothing() {
    let w = World::new(10 * DASH);
    let before = w.net.snapshot();
    w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    w.send(DASH).unwrap();
    let net = Arc::downgrade(&w.net);
    *w.net.before_send.lock().unwrap() = Some(Box::new(move |_: &StateTransition| {
        net.upgrade().unwrap().lag(before);
    }));
    w.net.answer(Answer::LandedRefused);
    let refused = w.send(DASH).unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");
    assert_eq!(w.executed(), 2);

    *w.net.before_send.lock().unwrap() = None;
    w.net.caught_up();
    w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    let t = w.net.sent()[1].clone();
    assert_eq!(w.send(DASH).unwrap_err(), landed(hash(&t)));
    w.send(DASH).unwrap();
    assert_eq!(nonce_of(&w.net.sent()[2]), nonce_of(&t) + 1);
}

/// C6-7: a read-back older than the pre-sign read is skipped, even one so
/// far behind that Drive could not take N from it; the next, current, read
/// decides the transfer executed.
#[test]
fn a_read_far_behind_is_skipped() {
    let w = World::new(10 * DASH);
    let (behind, _) = w.net.snapshot();
    for n in 5..=30 {
        w.net.other_write(n);
    }
    let net = Arc::downgrade(&w.net);
    *w.net.before_send.lock().unwrap() = Some(Box::new(move |_: &StateTransition| {
        net.upgrade().unwrap().lag_for((behind, 0), 1);
    }));
    w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    let out = w.send(DASH);
    assert_eq!(nonce_of(&w.net.sent()[0]), 31, "more than 24 past the read");
    assert_eq!(w.executed(), 1);
    assert_eq!(out.unwrap().transition, hash(&w.net.sent()[0]));
}

/// R1 for the nonce record: the transfer is in the session's record at its
/// nonce before it leaves, so nothing it signed can execute unrecorded.
#[test]
fn the_transfer_is_recorded_before_it_is_sent() {
    let w = World::new(10 * DASH);
    let s = Arc::clone(&w.f.s);
    *w.net.before_send.lock().unwrap() = Some(Box::new(move |t: &StateTransition| {
        let (k, signed) = s.withdrawal_nonces.unconfirmed(id(ME)).expect("recorded");
        assert_eq!(k, nonce_of(t));
        let st_hash = t.transaction_id().unwrap();
        assert_eq!(signed, vec![Signed::Transfer { st_hash }]);
    }));
    w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    w.send(DASH).unwrap();
    assert!(
        w.f.s.withdrawal_nonces.unconfirmed(id(ME)).is_none(),
        "executed"
    );
}

/// A restart forgets the session's record (BL-335), and the decision
/// Platform holds stands: a transfer seen executed is never signed again
/// at its nonce, and the next transfer after an unknown one signs at that
/// same nonce, so at most one of them executes.
#[test]
fn a_restart_keeps_the_decision() {
    let w = World::new(10 * DASH);
    w.net.answer(Answer::Pending);
    w.send(DASH).unwrap_err();
    let k = nonce_of(&w.net.sent()[0]);
    restart(&w);
    w.send(2 * DASH).unwrap_err();
    assert_eq!(nonce_of(&w.net.sent()[1]), k);
    w.net.execute_pending();
    assert_eq!(w.executed(), 1);
    assert_eq!(w.net.balance_now(), 10 * DASH - DASH - FEE, "the first one");

    restart(&w);
    w.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    w.send(DASH).unwrap();
    assert_eq!(nonce_of(&w.net.sent()[2]), k + 1);
    assert_eq!(w.executed(), 2);
}

/// The mainnet flag (`platform_address_sends`): on mainnet, at a protocol
/// with address funds, the quote and the transfer are refused before the
/// identity, a read, a nonce or the grant; a test network moves as before.
#[test]
fn a_transfer_to_the_platform_addresses_is_off_on_mainnet() {
    let off = |r: Result<(), CreditsError>| match r {
        Err(CreditsError::Platform(PlatformError::FeatureOff { feature }))
            if feature == "platform_address_sends" => {}
        other => panic!("{other:?}"),
    };
    let w = World::on(crate::DashNetwork::Mainnet, 3 * DASH);
    let to = w
        .call(|_, s, wallet| async move {
            Ok(next_receive_address(&wallet, s.vault.network())
                .await?
                .address)
        })
        .unwrap();
    let grant = w.f.grant(4 * DASH);
    off(w.transfer_quote(WithdrawAmount::All).map(drop));
    off(w
        .transfer_to(&to, WithdrawAmount::Credits { credits: DASH }, &grant)
        .map(drop));
    assert!(w.net.sent().is_empty(), "nothing broadcast");
    assert_eq!(w.executed(), 0);
    assert!(w.f.unused(&grant), "the grant is not redeemed");

    let t = World::on(crate::DashNetwork::Testnet, 3 * DASH);
    t.net.answer(Answer::Applied { fee: TRANSFER_FEE });
    t.send(DASH).unwrap();
    assert_eq!(t.executed(), 1);
}
