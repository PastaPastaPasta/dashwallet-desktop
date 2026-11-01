//! The facade's withdrawal calls on a real session (MG-13 P13, mn-engine
//! §2.5): `withdrawal_quote`, `withdraw_credits`, `retry_withdrawal`,
//! `withdrawals`, `recheck_withdrawal` and `mark_withdrawal`, over the
//! scripted [`Platform`] (the runtime's [`Seam`]) with the vault's attached
//! keys, a real `MasternodeWithdraw` grant and its lease. Each call's
//! success, the `withdrawal_ready` wait (P4), and each of §4's withdrawal
//! codes from its trigger.

use std::future::Future;
use std::time::Instant;

use super::*;
use crate::masternodes::{
    CreditWithdrawal, CreditWithdrawalState as State, Masternodes, MasternodesError,
    WithdrawalCheckingReason, WithdrawalDestinationCheck, WithdrawalKeyChoice as Key,
    WithdrawalKeySource, WithdrawalMark as Mark,
};
use crate::platform::lease::session_tests::{Fixture, fixture};

/// The core tests' waits, with no background recheck during a test.
const FACADE_WAITS: Waits = Waits {
    recheck: Duration::from_secs(3600),
    ..WAITS
};

/// A real session over the scripted Platform.
struct Fa {
    f: Fixture,
    plat: Arc<Platform>,
    mn: Masternodes,
}

impl Fa {
    /// The vault holds the masternode's owner and payout keys when
    /// `attached`; SPV's header tip is Platform's core height.
    fn new(attached: bool) -> Self {
        let f = fixture(false);
        f.session
            .trust
            .set_mode(crate::platform::trust::TrustMode::Enforcing);
        assert_eq!(f.session.trust.rule_5(), Rule5::Enforce);
        if attached {
            for (role, secret) in [("owner", &OWNER_SECRET), ("payout", &PAYOUT_SECRET)] {
                (f.session.vault())
                    .store_masternode_key(&PRO_TX, role, secret)
                    .unwrap();
            }
        }
        let fa = Self {
            mn: f.session.masternodes(),
            plat: Platform::new(&PRO_TX),
            f,
        };
        fa.tip(Some(TIP));
        fa
    }

    fn tip(&self, header_tip: Option<u32>) {
        *self.f.session.masternodes.withdrawals.seam.lock().unwrap() = Some(Seam {
            platform: Arc::clone(&self.plat),
            header_tip,
            waits: FACADE_WAITS,
        });
    }

    fn on<T>(&self, fut: impl Future<Output = T>) -> T {
        self.f.engine.block_on(fut)
    }

    /// The `MasternodeWithdraw` grant the user confirms (P12): the amount
    /// and the fee reserve, and a payout key's destination.
    fn grant(&self, max_credits: u64, destination: Option<&CoreScript>) -> String {
        self.f.grant(
            GrantPurpose::MasternodeWithdraw {
                max_credits,
                destination: destination.map(|s| WithdrawalDestination::of_script(s.as_bytes())),
            },
            Credential::None,
        )
    }

    /// `withdraw_credits` of `credits` by `key` to `to` (a payout key's
    /// default: its own address), under the grant of exactly that.
    fn withdraw(
        &self,
        key: Key,
        credits: u64,
        to: Option<&CoreScript>,
    ) -> Result<CreditWithdrawal, MasternodesError> {
        let bound = (key == Key::Payout).then(|| to.cloned().unwrap_or_else(payout_script));
        let grant = self.grant(credits + fee(), bound.as_ref());
        self.withdraw_with(key, credits, to, grant)
    }

    fn withdraw_with(
        &self,
        key: Key,
        credits: u64,
        to: Option<&CoreScript>,
        grant: String,
    ) -> Result<CreditWithdrawal, MasternodesError> {
        self.on(self.mn.withdraw_credits(
            self.f.wallet,
            display_hex(&PRO_TX),
            key,
            WithdrawAmount::Credits { credits },
            to.map(address),
            grant,
        ))
    }

    fn quote(&self) -> Result<crate::masternodes::WithdrawalQuote, MasternodesError> {
        self.on(self
            .mn
            .withdrawal_quote(self.f.wallet, display_hex(&PRO_TX)))
    }

    fn records(&self) -> Vec<CreditWithdrawal> {
        self.on(self.mn.withdrawals(display_hex(&PRO_TX))).unwrap()
    }

    fn recheck(&self, id: &str) -> Result<CreditWithdrawal, MasternodesError> {
        self.on(self.mn.recheck_withdrawal(id.into()))
    }

    fn generation(&self) -> u64 {
        self.f.session.masternodes.generation()
    }
}

fn fee() -> u64 {
    Rules::at(PlatformVersion::latest()).unwrap().fee
}

fn address(script: &CoreScript) -> String {
    dashcore::Address::from_script(
        &dashcore::ScriptBuf::from_bytes(script.as_bytes().to_vec()),
        Network::Regtest,
    )
    .unwrap()
    .to_string()
}

fn code<T: std::fmt::Debug>(r: Result<T, MasternodesError>) -> &'static str {
    r.unwrap_err().code()
}

/// Waits up to `bound` for `done`.
fn until(bound: Duration, done: impl Fn() -> bool) -> bool {
    let end = Instant::now() + bound;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

// ---------------------------------------------------------------------------
// The calls.

#[test]
fn the_quote_shows_the_held_keys_the_rules_and_each_fixed_destination() {
    let fa = Fa::new(true);
    let q = fa.quote().unwrap();
    assert_eq!(q.pro_tx_hash, display_hex(&PRO_TX));
    let owner = q.owner_key.unwrap();
    assert_eq!(
        (owner.kind, owner.source, owner.identity_key_id),
        (Key::Owner, WithdrawalKeySource::AttachedOwner, 1)
    );
    let [payout] = q.payout_keys.as_slice() else {
        panic!("one payout key: {:?}", q.payout_keys);
    };
    assert_eq!(
        (payout.kind, &payout.source, payout.identity_key_id),
        (Key::Payout, &WithdrawalKeySource::AttachedPayout, 0)
    );
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    assert_eq!(
        (
            q.rules.min_credits,
            q.rules.max_credits,
            q.rules.fee_credits
        ),
        (rules.min, rules.max, rules.fee)
    );
    let claimable = rules.credits(BALANCE, WithdrawAmount::All).unwrap();
    assert_eq!(q.claimable_credits, claimable);
    assert_eq!(
        q.expected_duffs,
        Some(expected_duffs(claimable, PlatformVersion::latest()).unwrap())
    );
    // The owner key's destination is Drive's window: the payout key's
    // address here, as is the payout key's own.
    let fixed = WithdrawalDestinationCheck::Fixed {
        address: address(&payout_script()),
        platform_height: 10,
        core_height: TIP,
        header_tip: TIP,
        lag: 0,
    };
    assert_eq!(q.owner_destination.as_ref(), Some(&fixed));
    assert_eq!(q.payout_destination.as_ref(), Some(&fixed));
    assert!(q.status.identity_found);
    assert_eq!(q.status.balance, Some(BALANCE));
    let keys: Vec<_> = (q.status.keys.iter())
        .map(|k| (k.id, k.purpose.as_str(), k.disabled))
        .collect();
    assert_eq!(keys, [(0, "transfer", false), (1, "owner", false)]);
    assert_eq!((q.status.header_tip, q.status.lag), (Some(TIP), Some(0)));
    assert!(q.open.is_none());
    assert!(fa.plat.sent().is_empty(), "a quote sends nothing");
}

#[test]
fn a_quote_while_platform_is_behind_returns_checking_at_once_and_signals_when_it_catches_up() {
    let fa = Fa::new(true);
    fa.tip(Some(TIP + 5));
    let before = fa.generation();
    let started = Instant::now();
    let q = fa.quote().unwrap();
    // At once: not after the wait's bound (FACADE_WAITS.quote).
    assert!(
        started.elapsed() < FACADE_WAITS.quote,
        "{:?}",
        started.elapsed()
    );
    let checking = WithdrawalDestinationCheck::Checking {
        reason: WithdrawalCheckingReason::PlatformBehind,
    };
    assert_eq!(q.owner_destination.as_ref(), Some(&checking));
    assert_eq!(q.payout_destination.as_ref(), Some(&checking));
    assert_eq!(q.status.lag, Some(5));
    // The wait reads again in the background; once Platform's core height
    // reaches the header tip, §6's event goes out (`withdrawal_ready`).
    assert!(until(Duration::from_millis(200), || fa
        .plat
        .reads(Read::Identity)
        >= 2));
    fa.plat.with(|w| w.core_height = TIP + 5);
    assert!(
        until(Duration::from_secs(3), || fa.generation() > before),
        "withdrawal_ready"
    );
    let q = fa.quote().unwrap();
    assert!(
        matches!(
            q.owner_destination,
            Some(WithdrawalDestinationCheck::Fixed { lag: 0, .. })
        ),
        "{q:?}"
    );
}

/// Fix round 1 M1: a quote in flight when the close begins starts no
/// `withdrawal_ready` wait. Once the close returns, nothing reads Platform
/// for it, signals, or holds the session.
#[test]
fn a_quote_in_flight_at_close_starts_no_ready_wait() {
    let fa = Fa::new(true);
    fa.tip(Some(TIP + 5));
    // The quote's identity read waits until the close has begun.
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let held: Hook = Box::new(move || {
        entered_tx.send(()).unwrap();
        released.recv().unwrap();
    });
    *fa.plat.hook.lock().unwrap() = Some((Read::Identity, held));
    let withdrawals = &fa.f.session.masternodes.withdrawals;
    std::thread::scope(|scope| {
        let quote = scope.spawn(|| fa.quote());
        entered.recv().unwrap();
        let close = scope.spawn(|| fa.on(fa.f.engine.close_network(crate::DashNetwork::Regtest)));
        assert!(until(Duration::from_secs(5), || withdrawals
            .work
            .is_closed()));
        release.send(()).unwrap();
        let q = quote.join().unwrap().unwrap();
        assert_eq!(
            q.owner_destination,
            Some(WithdrawalDestinationCheck::Checking {
                reason: WithdrawalCheckingReason::PlatformBehind,
            })
        );
        assert!(close.join().unwrap().unwrap());
    });
    // Platform catches up: a wait would read it and signal.
    let (reads, generation) = (fa.plat.reads(Read::Identity), fa.generation());
    fa.plat.with(|w| w.core_height = TIP + 5);
    std::thread::sleep(FACADE_WAITS.poll * 10);
    assert_eq!(fa.plat.reads(Read::Identity), reads, "a wait read Platform");
    assert_eq!(fa.generation(), generation, "withdrawal_ready after close");
    // The test's handles are the session's last.
    let session = Arc::downgrade(&fa.f.session);
    let Fa { f, mn, .. } = fa;
    drop(mn);
    let crate::platform::lease::session_tests::Fixture { session: held, .. } = f;
    drop(held);
    assert!(session.upgrade().is_none(), "something holds the session");
}

#[test]
fn the_ready_wait_stops_at_its_bound_and_one_runs_per_masternode() {
    let fa = Fa::new(true);
    fa.tip(Some(TIP + 5));
    let before = fa.generation();
    fa.quote().unwrap();
    fa.quote().unwrap();
    // Two quotes, one wait: at most one read per poll, plus the quotes'.
    std::thread::sleep(FACADE_WAITS.quote + Duration::from_millis(500));
    let polls = FACADE_WAITS.quote.as_millis() / FACADE_WAITS.poll.as_millis();
    let reads = fa.plat.reads(Read::Identity) as u128;
    assert!(reads <= polls + 3, "{reads} identity reads for one wait");
    assert_eq!(fa.generation(), before, "no withdrawal_ready while behind");
    // Past its bound, a catch-up signals nothing until the next quote.
    fa.plat.with(|w| w.core_height = TIP + 5);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fa.generation(), before);
}

#[test]
fn an_owner_key_withdrawal_is_stored_sent_and_done_on_its_document() {
    let fa = Fa::new(true);
    let before = fa.generation();
    let r = fa.withdraw(Key::Owner, AMOUNT, None).unwrap();
    assert_eq!(r.state, State::Sent);
    assert_eq!((r.key, r.credits, r.nonce), (Key::Owner, AMOUNT, 3));
    assert_eq!(r.pro_tx_hash, display_hex(&PRO_TX));
    assert_eq!(r.identity, shown(&identity_of(&PRO_TX)));
    assert_eq!(r.created_by, fa.f.wallet.to_string());
    assert_eq!(r.destination, None);
    assert_eq!(r.expected_destination, Some(address(&payout_script())));
    assert!(r.sent_at.is_some());
    assert_eq!(fa.plat.sent().len(), 1);
    assert!(fa.generation() > before, "a changed record is §6's event");
    // Platform executes it; the recheck finds its document.
    assert_eq!(fa.plat.block(), 1);
    let done = fa.recheck(&r.id).unwrap();
    assert_eq!(done.state, State::Done);
    assert_eq!(done.paid_to, Some(address(&payout_script())));
    assert_eq!(
        done.document_status,
        Some(crate::platform::WithdrawalStatus::Queued)
    );
    let proof = done.proof.clone().unwrap();
    assert_eq!(
        proof.document_id,
        document_id(&identity_of(&PRO_TX), 3, &payout_script())
            .to_string(dpp::platform_value::string_encoding::Encoding::Base58)
    );
    assert_eq!(fa.records(), vec![done]);
}

#[test]
fn a_payout_key_withdrawal_pays_the_destination_its_grant_binds() {
    // By default, the key's own address.
    let fa = Fa::new(true);
    let r = fa.withdraw(Key::Payout, AMOUNT, None).unwrap();
    assert_eq!(r.destination, Some(address(&payout_script())));
    assert_eq!(
        decoded(&fa.plat.sent()[0]).output_script,
        Some(payout_script())
    );

    // A chosen address, under a grant bound to it.
    let fa = Fa::new(true);
    let to = CoreScript::new_p2pkh([0x42; 20]);
    // A grant bound to another destination: the vault's scope refuses to
    // sign (`signer`), and nothing is stored or sent.
    let other = fa.grant(AMOUNT + fee(), Some(&payout_script()));
    let e = fa
        .withdraw_with(Key::Payout, AMOUNT, Some(&to), other)
        .unwrap_err();
    assert_eq!(e.code(), "signer");
    assert!(fa.plat.sent().is_empty());
    assert!(fa.records().is_empty());
    let r = fa.withdraw(Key::Payout, AMOUNT, Some(&to)).unwrap();
    assert_eq!(r.destination, Some(address(&to)));
    assert_eq!(decoded(&fa.plat.sent()[0]).output_script, Some(to.clone()));
    fa.plat.block();
    let done = fa.recheck(&r.id).unwrap();
    assert_eq!(
        (done.state, done.paid_to),
        (State::Done, Some(address(&to)))
    );
}

#[test]
fn try_again_supersedes_the_refused_record_with_a_sibling_at_its_nonce() {
    let fa = Fa::new(true);
    fa.plat
        .answer(Answer::Refused("10001 merits".into()), false);
    let refused = fa.withdraw(Key::Payout, AMOUNT, None).unwrap();
    assert_eq!(refused.state, State::Refused);
    assert_eq!(refused.state_reason.as_deref(), Some("10001 merits"));
    // Another key and amount: P2–P4 apply again.
    let half = AMOUNT / 2;
    let grant = fa.grant(half + fee(), None);
    let sibling = fa
        .on(fa.mn.retry_withdrawal(
            fa.f.wallet,
            refused.id.clone(),
            Key::Owner,
            WithdrawAmount::Credits { credits: half },
            None,
            grant,
        ))
        .unwrap();
    assert_eq!(
        (sibling.nonce, sibling.key, sibling.credits),
        (refused.nonce, Key::Owner, half)
    );
    let states: Vec<_> = fa
        .records()
        .iter()
        .map(|r| (r.id.clone(), r.state))
        .collect();
    assert_eq!(
        states,
        [(refused.id, State::Superseded), (sibling.id, State::Sent)]
    );
}

#[test]
fn a_mark_applies_to_a_couldnt_confirm_record_only() {
    let fa = Fa::new(true);
    let r = fa.withdraw(Key::Payout, AMOUNT, None).unwrap();
    let mark = |m| fa.on(fa.mn.mark_withdrawal(r.id.clone(), m));
    assert_eq!(
        code(mark(Mark::Received)),
        "masternodes.withdrawal_mark_state"
    );
    fa.plat.lagging(Read::Document);
    let c = fa.recheck(&r.id).unwrap();
    assert_eq!(c.state, State::CouldntConfirm);
    let before = fa.generation();
    let m = mark(Mark::NotReceived).unwrap();
    assert_eq!(m.state, State::MarkedNotReceived);
    assert_eq!(m.mark, Some(Mark::NotReceived));
    assert!(m.marked_at.is_some());
    assert!(fa.generation() > before);
    assert_eq!(
        code(mark(Mark::Received)),
        "masternodes.withdrawal_mark_state"
    );
}

// ---------------------------------------------------------------------------
// §4's withdrawal codes, each from its trigger. Nothing is sent.

#[test]
fn no_usable_key_is_withdrawal_key_missing() {
    let fa = Fa::new(false);
    let q = fa.quote().unwrap();
    assert!(q.owner_key.is_none() && q.payout_keys.is_empty());
    for key in [Key::Owner, Key::Payout] {
        assert_eq!(
            code(fa.withdraw(key, AMOUNT, None)),
            "masternodes.withdrawal_key_missing"
        );
    }
    // The identity's matching key disabled: the same.
    let fa = Fa::new(true);
    fa.plat.with(|w| {
        w.identity = Some(masternode_identity(
            &PRO_TX,
            vec![payout_key(0), disabled(owner_key())],
        ))
    });
    assert_eq!(
        code(fa.withdraw(Key::Owner, AMOUNT, None)),
        "masternodes.withdrawal_key_missing"
    );
    assert!(fa.plat.sent().is_empty());
}

#[test]
fn a_shared_masternode_is_withdrawal_shared() {
    let fa = Fa::new(true);
    let marker = format!("masternodes.shared_seen:{}", display_hex(&PRO_TX));
    fa.on(fa
        .f
        .session
        .appdb_op(move |db| db.set_setting_durable(dw_appdb::GLOBAL_SCOPE, &marker, Some("1"))))
        .unwrap();
    assert_eq!(code(fa.quote()), "masternodes.withdrawal_shared");
    assert_eq!(
        code(fa.withdraw(Key::Owner, AMOUNT, None)),
        "masternodes.withdrawal_shared"
    );
    assert!(fa.plat.sent().is_empty());
}

#[test]
fn a_window_with_no_usable_transfer_key_is_withdrawal_no_payout_key() {
    let fa = Fa::new(true);
    fa.plat.with(|w| {
        w.identity = Some(masternode_identity(
            &PRO_TX,
            vec![disabled(payout_key(0)), owner_key()],
        ))
    });
    let e = fa.withdraw(Key::Owner, AMOUNT, None).unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_no_payout_key");
    assert!(matches!(e, MasternodesError::WithdrawalNoPayoutKey { .. }));
    assert!(fa.plat.sent().is_empty());
}

#[test]
fn each_failing_condition_is_withdrawal_destination_checking_with_its_reason() {
    let fa = Fa::new(true);
    let grant = fa.grant(AMOUNT + fee(), None);
    let reason = |e: MasternodesError| match e {
        MasternodesError::WithdrawalDestinationChecking { reason } => reason,
        e => panic!("{e:?}"),
    };
    let refused = |want| {
        let e = (fa.withdraw_with(Key::Owner, AMOUNT, None, grant.clone())).unwrap_err();
        assert_eq!(e.code(), "masternodes.withdrawal_destination_checking");
        assert_eq!(reason(e), want);
        assert!(fa.plat.sent().is_empty());
        assert!(fa.records().is_empty());
    };
    fa.tip(Some(TIP + 1));
    refused(WithdrawalCheckingReason::PlatformBehind);
    fa.tip(None);
    refused(WithdrawalCheckingReason::SpvUnsynced);
    fa.tip(Some(TIP));
    fa.plat.deviate(Read::Identity, Dev::Unverified, ALWAYS);
    refused(WithdrawalCheckingReason::Unverified);
    fa.plat.calm();
    // Each wait came before the grant: it is still unredeemed (P4).
    let r = fa.withdraw_with(Key::Owner, AMOUNT, None, grant.clone());
    assert_eq!(r.unwrap().state, State::Sent);

    // (b): a payout-change barrier whose operation can't be read (its
    // wallet unknown) holds until Platform pays its script (P16).
    let fa = Fa::new(true);
    let grant = fa.grant(AMOUNT + fee(), None);
    let row = MasternodeOpRow {
        id: "op".into(),
        kind: "update_registrar".into(),
        terms: "{}".into(),
        request: "{}".into(),
        unsigned_tx: None,
        sign_address: None,
        sign_message: None,
        fee_inputs: "[]".into(),
        signed_tx: Some(vec![2]),
        txid: Some("ab".repeat(32)),
        pro_tx_hash: Some(display_hex(&PRO_TX)),
        locked_collateral: false,
        fee: 7,
        created_at: 1,
        signed_at: Some(1),
        sent_at: None,
        last_error: None,
    };
    fa.on(fa.f.session.appdb_op(move |db| {
        db.insert_masternode_op_with_barrier_durable("w", &row, &[], 1, Some(&[0x51]))
    }))
    .unwrap();
    let q = fa.quote().unwrap();
    assert_eq!(
        q.owner_destination,
        Some(WithdrawalDestinationCheck::Checking {
            reason: WithdrawalCheckingReason::PayoutChangePending
        })
    );
    let e = (fa.withdraw_with(Key::Owner, AMOUNT, None, grant)).unwrap_err();
    assert_eq!(reason(e), WithdrawalCheckingReason::PayoutChangePending);
    assert!(fa.plat.sent().is_empty());
}

#[test]
fn an_open_record_is_withdrawal_pending() {
    let fa = Fa::new(true);
    fa.withdraw(Key::Owner, AMOUNT, None).unwrap();
    assert_eq!(
        code(fa.withdraw(Key::Payout, AMOUNT, None)),
        "masternodes.withdrawal_pending"
    );
    assert_eq!(fa.plat.sent().len(), 1);
}

#[test]
fn an_unverified_nonce_read_under_enforce_is_withdrawal_unverified() {
    let fa = Fa::new(true);
    fa.plat.deviate(Read::Nonce, Dev::Unverified, ALWAYS);
    assert_eq!(
        code(fa.withdraw(Key::Owner, AMOUNT, None)),
        "masternodes.withdrawal_unverified"
    );
    assert!(fa.plat.sent().is_empty());
    assert!(fa.records().is_empty());
}

#[test]
fn try_again_from_another_state_is_withdrawal_retry_state() {
    let fa = Fa::new(true);
    let r = fa.withdraw(Key::Owner, AMOUNT, None).unwrap();
    assert_eq!(r.state, State::Sent);
    let grant = fa.grant(AMOUNT + fee(), None);
    let retry = fa.on(fa.mn.retry_withdrawal(
        fa.f.wallet,
        r.id,
        Key::Owner,
        WithdrawAmount::Credits { credits: AMOUNT },
        None,
        grant,
    ));
    assert_eq!(code(retry), "masternodes.withdrawal_retry_state");
    assert_eq!(fa.plat.sent().len(), 1);
}

#[test]
fn amounts_outside_drives_rules_are_withdrawal_amount_codes() {
    let fa = Fa::new(true);
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    let e = fa.withdraw(Key::Owner, rules.min - 1, None).unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_amount_out_of_range");
    assert!(matches!(
        e,
        MasternodesError::WithdrawalAmountOutOfRange { min, max } if (min, max) == (rules.min, rules.max)
    ));
    let e = fa.withdraw(Key::Owner, BALANCE, None).unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_amount_insufficient");
    assert!(matches!(
        e,
        MasternodesError::WithdrawalAmountInsufficient { needed, available }
            if needed == BALANCE + rules.fee && available == BALANCE
    ));
    // A grant short of the amount and the reserve: the lease refuses the
    // charge and nothing is signed.
    let short = fa.grant(AMOUNT, None);
    let e = fa
        .withdraw_with(Key::Owner, AMOUNT, None, short)
        .unwrap_err();
    assert_eq!(e.code(), "platform.grant_exceeded");
    assert!(fa.plat.sent().is_empty());
    assert!(fa.records().is_empty());
}

#[test]
fn unknown_records_identities_and_malformed_arguments_are_refused() {
    let fa = Fa::new(true);
    let unknown = "00".repeat(16);
    assert_eq!(
        code(fa.recheck(&unknown)),
        "masternodes.withdrawal_not_found"
    );
    assert_eq!(
        code(fa.on(fa.mn.mark_withdrawal(unknown.clone(), Mark::Received))),
        "masternodes.withdrawal_not_found"
    );
    let grant = fa.grant(AMOUNT + fee(), None);
    let retry = |id: String| {
        fa.on(fa.mn.retry_withdrawal(
            fa.f.wallet,
            id,
            Key::Owner,
            WithdrawAmount::All,
            None,
            grant.clone(),
        ))
    };
    assert_eq!(code(retry(unknown)), "masternodes.withdrawal_not_found");
    for bad in ["zz".to_string(), "AB".repeat(16), "ab".repeat(32)] {
        assert_eq!(code(fa.recheck(&bad)), "invalid_argument");
        assert_eq!(code(retry(bad)), "invalid_argument");
    }
    assert_eq!(
        code(fa.on(fa.mn.withdrawals("zz".into()))),
        "invalid_argument"
    );
    assert_eq!(
        code(fa.on(fa.mn.withdrawal_quote(fa.f.wallet, "zz".into()))),
        "invalid_argument"
    );
    // An owner key takes no destination.
    let to = CoreScript::new_p2pkh([0x42; 20]);
    assert_eq!(
        code(fa.withdraw_with(Key::Owner, AMOUNT, Some(&to), grant.clone())),
        "invalid_argument"
    );
    // Platform has no identity for the masternode.
    fa.plat.with(|w| w.identity = None);
    assert_eq!(code(fa.quote()), "masternodes.no_identity");
    assert!(fa.plat.sent().is_empty());
}
