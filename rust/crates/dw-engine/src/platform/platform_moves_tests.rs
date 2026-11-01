use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use dpp::identity::signer::Signer;

use super::super::money_gate_tests::{FakePlatform, Proof, gate_session};
use super::super::requests::tests::Fixture;
use super::super::trust::TrustMode;
use super::*;

const DASH: u64 = 100_000_000_000;

fn snap(entries: &[(&str, u64)]) -> Snapshot {
    entries.iter().map(|(a, c)| ((*a).to_owned(), *c)).collect()
}

fn kinds(rows: &[PlatformActivity]) -> Vec<(PlatformActivityKind, String, u64)> {
    rows.iter()
        .map(|r| (r.kind, r.address.clone().unwrap_or_default(), r.credits))
        .collect()
}

/// The first read reports every funded address as incoming; later reads
/// report each rise and fall once, and an emptied address leaves the
/// snapshot.
#[test]
fn balance_changes_become_rows_once() {
    let mut unseen = BTreeMap::new();
    let held = BTreeSet::new();
    let first = snap(&[("a", 500), ("b", 70)]);
    let (rows, next) = changes(None, &first, &held, &mut unseen, (10, 1));
    assert_eq!(
        kinds(&rows),
        vec![
            (PlatformActivityKind::Incoming, "a".into(), 500),
            (PlatformActivityKind::Incoming, "b".into(), 70),
        ]
    );
    assert_eq!(next, first);
    let (rows, again) = changes(Some(&next), &first, &held, &mut unseen, (11, 2));
    assert!(rows.is_empty());
    assert_eq!(again, first);
    let later = snap(&[("a", 650)]);
    let (rows, next) = changes(Some(&first), &later, &held, &mut unseen, (12, 3));
    assert_eq!(
        kinds(&rows),
        vec![
            (PlatformActivityKind::Incoming, "a".into(), 150),
            (PlatformActivityKind::Outgoing, "b".into(), 70),
        ]
    );
    assert_eq!(next, later);
    assert!(
        rows.iter()
            .all(|r| r.state == PlatformActivityState::Done && r.at == 12)
    );
    // n4: ids carry the proof height, so two reads in one second differ.
    assert_eq!(rows[0].id, "seen-3-a");
}

/// A pending move's inputs are not compared (no outgoing row, the stored
/// value kept); a done move's fall is the move's, read once, and only what
/// exceeds it is outgoing.
#[test]
fn a_moves_own_spend_is_no_outgoing_row() {
    let before = snap(&[("a", 1_000), ("b", 400)]);
    let now = snap(&[("a", 100), ("b", 400)]);
    let held: BTreeSet<String> = ["a".to_owned()].into();
    let mut unseen = BTreeMap::new();
    let (rows, next) = changes(Some(&before), &now, &held, &mut unseen, (5, 5));
    assert!(rows.is_empty());
    assert_eq!(
        next, before,
        "a pending move's input keeps its stored value"
    );

    let mut unseen: BTreeMap<String, u64> = [("a".to_owned(), 905)].into();
    let (rows, next) = changes(Some(&before), &now, &BTreeSet::new(), &mut unseen, (6, 6));
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(next, now);
    assert_eq!(unseen["a"], 0, "spent once read");

    let mut unseen: BTreeMap<String, u64> = [("a".to_owned(), 800)].into();
    let (rows, _) = changes(Some(&before), &now, &BTreeSet::new(), &mut unseen, (7, 7));
    assert_eq!(
        kinds(&rows),
        vec![(PlatformActivityKind::Outgoing, "a".into(), 100)]
    );
    // Not read yet (the balance did not move): kept for the next read.
    let mut unseen: BTreeMap<String, u64> = [("b".to_owned(), 50)].into();
    let (_, _) = changes(Some(&before), &now, &BTreeSet::new(), &mut unseen, (8, 8));
    assert_eq!(unseen["b"], 50);
}

fn record(id: &str, at: u64, inputs: &[(&str, u32)]) -> MoveRecord {
    MoveRecord {
        id: id.into(),
        inputs: inputs
            .iter()
            .map(|(a, n)| MoveInput {
                address: (*a).to_owned(),
                nonce: *n,
                credits: 1,
                before: 0,
                fee_source: false,
                unseen: 0,
            })
            .collect(),
        credits: 2,
        fee_credits: 1,
        expected_duffs: 0,
        to: "y".into(),
        at,
        state: PlatformActivityState::Pending,
        signed: true,
        answered: false,
        can_land: true,
        seen_at: None,
        fee_taken: None,
    }
}

fn seen(height: u64, nonces: &[(&str, Option<u32>)]) -> Seen {
    Seen {
        height,
        at: (nonces.iter())
            .map(|(a, n)| ((*a).to_owned(), n.map(|n| (n, 0))))
            .collect(),
    }
}

/// Sol M1: no clock ages a move out. The first read after its call sets
/// the height it must be proved unexecuted after; only a proof above it
/// with every nonce unused fails it, and failed it still holds its inputs.
/// (eaa83f96 called it failed ten minutes after `at`.)
#[test]
fn an_unknown_move_never_fails_by_age() {
    let mut moves = vec![record("m", 1, &[("a", 4), ("b", 9)])];
    let below = |h| seen(h, &[("a", Some(3)), ("b", Some(8))]);
    settle(&mut moves, &below(10));
    assert_eq!(moves[0].seen_at, Some(10));
    assert_eq!(moves[0].state, PlatformActivityState::Pending);
    // However late: the same height is no evidence.
    moves[0].at = 0;
    settle(&mut moves, &below(10));
    assert_eq!(moves[0].state, PlatformActivityState::Pending);
    // An unknown address is no evidence either.
    settle(&mut moves, &seen(11, &[("a", Some(3)), ("b", None)]));
    assert_eq!(moves[0].state, PlatformActivityState::Pending);
    settle(&mut moves, &below(11));
    assert_eq!(moves[0].state, PlatformActivityState::Failed);
    assert!(moves[0].holds(), "it may still execute");
    // It does: done after all.
    settle(&mut moves, &seen(12, &[("a", Some(4)), ("b", Some(9))]));
    assert_eq!(moves[0].state, PlatformActivityState::Done);
    assert!(!moves[0].holds());
}

/// Opus m1: a failed move and its retry at the same inputs and nonces are
/// settled from one proof: the newest the library answered is done, else
/// the oldest; the other failed for good.
#[test]
fn a_retry_and_its_failed_move_settle_from_one_proof() {
    let landed = seen(20, &[("a", Some(4))]);
    let pair = |answered: bool| {
        let mut f = record("f", 1, &[("a", 4)]);
        f.state = PlatformActivityState::Failed;
        f.seen_at = Some(5);
        let mut r = record("r", 2, &[("a", 4)]);
        r.answered = answered;
        r.seen_at = Some(15);
        vec![f, r]
    };
    let mut moves = pair(true);
    assert_eq!(settle(&mut moves, &landed), BTreeSet::from([0, 1]));
    assert_eq!(moves[1].state, PlatformActivityState::Done);
    assert_eq!(moves[0].state, PlatformActivityState::Failed);
    assert!(!moves[0].can_land);
    let mut moves = pair(false);
    settle(&mut moves, &landed);
    assert_eq!(moves[0].state, PlatformActivityState::Done);
    assert_eq!(
        (moves[1].state, moves[1].can_land),
        (PlatformActivityState::Failed, false)
    );
}

/// Opus m3 and n3: a done move reports the fee its inputs' balances show,
/// and only its fee source's fall carries it.
#[test]
fn a_done_move_takes_the_fee_on_its_fee_source() {
    let mut r = record("m", 1, &[("a", 1), ("b", 1)]);
    r.fee_credits = 50;
    (
        r.inputs[0].before,
        r.inputs[0].credits,
        r.inputs[0].fee_source,
    ) = (1_000, 600, true);
    (r.inputs[1].before, r.inputs[1].credits) = (500, 500);
    let after = Seen {
        height: 3,
        at: [("a".into(), Some((1, 380))), ("b".into(), Some((1, 0)))].into(),
    };
    r.done(&after);
    assert_eq!(r.fee_taken, Some(20));
    assert_eq!((r.inputs[0].unseen, r.inputs[1].unseen), (620, 500));
    assert_eq!(r.activity().fee_credits, Some(20));
    // A balance before not on record: the fee is unknown, the estimate
    // stands for the allowance.
    r.inputs[1].before = 0;
    r.done(&after);
    assert_eq!((r.fee_taken, r.inputs[0].unseen), (None, 650));
}

fn address(byte: u8) -> PlatformAddress {
    PlatformAddress::P2pkh([byte; 20])
}

fn limits() -> Limits {
    Limits {
        min_input: 50,
        max_inputs: 3,
        min: 100,
        max: 2_000,
        fee_base: 7,
        fee_per_input: 1,
    }
}

fn balances(entries: &[(u8, u64)]) -> BTreeMap<PlatformAddress, u64> {
    entries.iter().map(|(b, c)| (address(*b), *c)).collect()
}

/// The minimum fee is dpp's estimate.
#[test]
fn the_fee_is_dpps_estimate() {
    use dpp::state_transition::address_credit_withdrawal_transition::AddressCreditWithdrawalTransition;
    let v = PlatformVersion::latest();
    let l = Limits::at(v).unwrap();
    for n in 1..=3 {
        assert_eq!(
            l.fee(n),
            AddressCreditWithdrawalTransition::estimate_min_fee(n, false, v)
        );
    }
    assert_eq!((l.min, l.max), withdrawal::address_range(v).unwrap());
}

/// An amount takes the fee source (the largest) first, keeping the fee on
/// it, then the largest; the fee grows with the inputs.
#[test]
fn a_partial_move_keeps_the_fee_on_the_fee_source() {
    let b = balances(&[(1, 300), (2, 1_000), (3, 500)]);
    let p = plan(&b, WithdrawAmount::Credits { credits: 400 }, &limits()).unwrap();
    assert_eq!(p.inputs, balances(&[(2, 400)]));
    assert_eq!((p.fee_source, p.fee), (address(2), 8));

    let p = plan(&b, WithdrawAmount::Credits { credits: 1_200 }, &limits()).unwrap();
    assert_eq!(p.inputs, balances(&[(2, 991), (3, 209)]));
    assert_eq!((p.fee_source, p.fee, p.credits()), (address(2), 9, 1_200));
    let strategy = p.fee_strategy();
    let [AddressFundsFeeStrategyStep::DeductFromInput(i)] = strategy.as_slice() else {
        panic!()
    };
    assert_eq!(p.inputs.keys().nth(usize::from(*i)), Some(&address(2)));

    // All: every input, less the fee for three.
    let p = plan(&b, WithdrawAmount::All, &limits()).unwrap();
    assert_eq!(p.inputs, balances(&[(1, 300), (2, 990), (3, 500)]));
    assert_eq!(p.fee, 10);
    let short = plan(&b, WithdrawAmount::Credits { credits: 1_791 }, &limits());
    assert!(matches!(short, Err(Refused::Short { .. })), "{short:?}");
}

/// Sol m5: a balance above Platform's largest withdrawal still moves a
/// smaller amount, and `All` takes the largest (eaa83f96 planned every
/// funded address first and refused both).
#[test]
fn a_balance_above_the_largest_withdrawal_still_moves() {
    let b = balances(&[(1, 6_000)]);
    let p = plan(&b, WithdrawAmount::Credits { credits: 150 }, &limits()).unwrap();
    assert_eq!(p.inputs, balances(&[(1, 150)]));
    let p = plan(&b, WithdrawAmount::All, &limits()).unwrap();
    assert_eq!(p.credits(), 2_000);
    // More funded addresses than one transition takes: the largest.
    let b = balances(&[(1, 60), (2, 70), (3, 80), (4, 90)]);
    let p = plan(&b, WithdrawAmount::All, &limits()).unwrap();
    assert_eq!(p.inputs.len(), 3);
    assert!(!p.inputs.contains_key(&address(1)));
}

/// A last input below the input minimum borrows from the one before; when
/// that cannot stay above the minimum there is no plan. Outside the range
/// is out of range; nothing movable is short by the least withdrawal and
/// its fee.
#[test]
fn a_small_last_input_borrows_or_refuses() {
    let b = balances(&[(1, 300), (2, 1_000)]);
    let p = plan(&b, WithdrawAmount::Credits { credits: 1_000 }, &limits()).unwrap();
    assert_eq!(p.inputs, balances(&[(2, 950), (1, 50)]));
    let tight = Limits {
        min_input: 960,
        ..limits()
    };
    let b = balances(&[(1, 990), (2, 1_000)]);
    assert_eq!(
        plan(&b, WithdrawAmount::Credits { credits: 1_000 }, &tight),
        Err(Refused::OutOfRange)
    );
    assert_eq!(
        plan(&b, WithdrawAmount::Credits { credits: 20 }, &limits()),
        Err(Refused::OutOfRange)
    );
    assert_eq!(
        plan(&balances(&[(1, 90)]), WithdrawAmount::All, &limits()),
        Err(Refused::Short {
            needed: 108,
            available: 90
        })
    );
}

/// Opus m5: what an `All` move leaves behind is not movable.
#[test]
fn a_remainder_too_small_to_move_is_not_movable() {
    assert_eq!(movable(&balances(&[(1, 107)]), &limits()), 0);
    assert_eq!(movable(&balances(&[(1, 40), (2, 30)]), &limits()), 0);
    assert_eq!(movable(&balances(&[(1, 300), (2, 40)]), &limits()), 292);
}

/// Opus m2: Platform's structure or signature check refusing is final;
/// a transport failure is not.
#[test]
fn only_a_stateless_refusal_is_final() {
    use dpp::consensus::basic::state_transition::TransitionNoInputsError;
    let structure = dash_sdk::Error::from(TransitionNoInputsError::new());
    assert!(never_valid(&PlatformWalletError::Sdk(structure)));
    let lost = dash_sdk::Error::TimeoutReached(Duration::from_secs(1), "wait".into());
    assert!(!never_valid(&PlatformWalletError::Sdk(lost)));
}

// ---- the move against a scripted network ----

/// What the next `withdraw` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    /// Executes and answers.
    Executes,
    /// Answers with the addresses unchanged: no execution.
    AnswersOnly,
    /// Executes; the answer is lost.
    ExecutesUnanswered,
    /// Neither: the broadcast is lost.
    Lost,
    /// Signed, then refused by the structure check.
    Invalid,
}

struct FakeMoves {
    /// address → (nonce, credits) on "Platform".
    state: Mutex<BTreeMap<PlatformAddress, (u32, u64)>>,
    height: AtomicU64,
    acts: Mutex<VecDeque<Act>>,
    /// The fee an executed transition takes.
    fee: u64,
    sent: Mutex<Vec<BTreeMap<PlatformAddress, (u32, u64)>>>,
    /// Reads use a key SPV does not verify.
    unverified: AtomicBool,
    /// Where the moves are stored, to see one recorded before signing.
    store: Mutex<Option<(Arc<dw_appdb::AppDb>, String)>>,
    recorded_before_signing: Mutex<Vec<bool>>,
}

impl FakeMoves {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::default(),
            height: AtomicU64::new(100),
            acts: Mutex::default(),
            // Below the estimate, as Platform's actual fee is.
            fee: Limits::at(PlatformVersion::latest()).unwrap().fee(1) / 2,
            sent: Mutex::default(),
            unverified: AtomicBool::new(false),
            store: Mutex::default(),
            recorded_before_signing: Mutex::default(),
        })
    }

    fn fund(&self, address: PlatformAddress, credits: u64) {
        let mut state = self.state.lock().unwrap();
        state.entry(address).or_insert((0, 0)).1 += credits;
        self.tick();
    }

    fn tick(&self) {
        self.height.fetch_add(1, Ordering::SeqCst);
    }

    fn then(&self, act: Act) {
        self.acts.lock().unwrap().push_back(act);
    }

    fn credits(&self, address: PlatformAddress) -> u64 {
        self.state.lock().unwrap().get(&address).map_or(0, |s| s.1)
    }

    /// Executes `inputs` as Drive would: every nonce the stored one + 1.
    fn execute(&self, inputs: &BTreeMap<PlatformAddress, (u32, u64)>, source: PlatformAddress) {
        let mut state = self.state.lock().unwrap();
        if !(inputs.iter()).all(|(a, (n, _))| state.get(a).is_some_and(|s| s.0 + 1 == *n)) {
            return;
        }
        for (a, (n, c)) in inputs {
            let s = state.get_mut(a).unwrap();
            s.0 = *n;
            s.1 -= c + if *a == source { self.fee } else { 0 };
        }
        drop(state);
        self.tick();
    }
}

impl MoveNet for FakeMoves {
    fn version(&self) -> &'static PlatformVersion {
        PlatformVersion::latest()
    }

    async fn read(&self, addresses: BTreeSet<PlatformAddress>) -> Result<Proved, PlatformError> {
        trust::note_lookup(!self.unverified.load(Ordering::SeqCst));
        let state = self.state.lock().unwrap();
        let infos = (addresses.into_iter())
            .map(|a| (a, state.get(&a).copied()))
            .collect();
        Ok((self.height.load(Ordering::SeqCst), infos))
    }

    async fn withdraw(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        _script: CoreScript,
        fee_strategy: AddressFundsFeeStrategy,
        signer: &VaultPlatformAddressSigner,
    ) -> Result<(), PlatformWalletError> {
        let recorded = self
            .store
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(db, scope)| {
                (db.settings_with_prefix(scope, MOVE_PREFIX).unwrap().iter()).any(|(_, v)| {
                    let m: MoveRecord = serde_json::from_str(v).unwrap();
                    m.state == PlatformActivityState::Pending && !m.signed
                })
            });
        self.recorded_before_signing.lock().unwrap().push(recorded);
        for address in inputs.keys() {
            signer.sign(address, b"transition").await.unwrap();
        }
        self.sent.lock().unwrap().push(inputs.clone());
        let [AddressFundsFeeStrategyStep::DeductFromInput(i)] = fee_strategy.as_slice() else {
            panic!("{fee_strategy:?}")
        };
        let source = *inputs.keys().nth(usize::from(*i)).unwrap();
        let lost = || {
            PlatformWalletError::Sdk(dash_sdk::Error::TimeoutReached(
                Duration::from_secs(1),
                "wait".into(),
            ))
        };
        match self
            .acts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Act::Executes)
        {
            Act::Executes => {
                self.execute(&inputs, source);
                Ok(())
            }
            Act::AnswersOnly => Ok(()),
            Act::ExecutesUnanswered => {
                self.execute(&inputs, source);
                Err(lost())
            }
            Act::Lost => Err(lost()),
            Act::Invalid => {
                use dpp::consensus::basic::state_transition::TransitionNoInputsError;
                Err(PlatformWalletError::Sdk(
                    TransitionNoInputsError::new().into(),
                ))
            }
        }
    }
}

/// A session with a wallet, its Platform payment addresses and `net`.
struct World {
    f: Fixture,
    net: Arc<FakeMoves>,
    pool: Vec<PlatformAddress>,
    to: String,
}

impl World {
    fn new() -> Self {
        let f = Fixture::new(0);
        let net = FakeMoves::new();
        let mut w = Self {
            f,
            net,
            pool: Vec::new(),
            to: String::new(),
        };
        let (pool, to) = w
            .call(|_, s, wallet, id| async move {
                let to = s.current_receiving_address(id).await.unwrap();
                Ok((pool(&wallet).await?.into_keys().collect(), to.to_string()))
            })
            .unwrap();
        (w.pool, w.to) = (pool, to);
        let live = w.f.s.live().unwrap();
        *w.net.store.lock().unwrap() =
            Some((live.appdb, dw_appdb::local_scope(&w.f.wallet.to_string())));
        w
    }

    fn call<T: Send + 'static, Fut>(
        &self,
        body: impl FnOnce(Arc<FakeMoves>, Arc<NetworkSession>, Arc<PlatformWallet>, WalletId) -> Fut
        + Send
        + 'static,
    ) -> Result<T, CreditsError>
    where
        Fut: Future<Output = Result<T, CreditsError>> + Send + 'static,
    {
        let net = Arc::clone(&self.net);
        let id = self.f.wallet;
        let dp = self.f.s.dashpay(id);
        (self.f.engine).block_on(dp.on_wallet(move |s, _, w| body(net, s, w, id)))
    }

    fn address(&self, n: usize) -> PlatformAddress {
        self.pool[n]
    }

    fn move_to(
        &self,
        amount: WithdrawAmount,
        to: &str,
        grant: &str,
    ) -> Result<MoveToCoreOutcome, CreditsError> {
        let (to, grant) = (to.to_owned(), grant.to_owned());
        self.call(move |net, s, w, id| async move {
            move_to_core(&*net, &s, &w, id, (amount, &to), &grant).await
        })
    }

    fn move_all(&self, grant: &str) -> Result<MoveToCoreOutcome, CreditsError> {
        self.move_to(WithdrawAmount::All, &self.to.clone(), grant)
    }

    fn activity(&self) -> Vec<PlatformActivity> {
        self.call(|net, s, w, id| async move { activity(&*net, &s, &w, id).await })
            .unwrap()
    }

    fn moves(&self) -> Vec<MoveRecord> {
        let id = self.f.wallet;
        let s = Arc::clone(&self.f.s);
        (self.f.engine)
            .block_on(async move { Store::of(&s, id).unwrap().moves().await })
            .unwrap()
    }
}

fn find(activity: &[PlatformActivity], kind: PlatformActivityKind) -> &PlatformActivity {
    activity.iter().find(|r| r.kind == kind).unwrap()
}

/// Opus m4 (replaces 2c02d646's test, which passed at eaa83f96): the
/// smoke's order, funded, never read, all moved, then read: one incoming
/// row of the funding and the move, not the fee left over as incoming.
#[test]
fn the_smoke_order_reads_the_funding_before_the_move() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let out = w.move_all(&w.f.grant(3 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    // The estimate's surplus stays, and is not incoming.
    let estimate = Limits::at(PlatformVersion::latest()).unwrap().fee(1);
    assert_eq!(w.net.credits(a), estimate - w.net.fee);
    let done = PlatformActivityState::Done;
    let activity = w.activity();
    assert_eq!(activity.len(), 2, "{activity:?}");
    let incoming = find(&activity, PlatformActivityKind::Incoming);
    assert_eq!((incoming.state, incoming.credits), (done, 3 * DASH));
    let moved = find(&activity, PlatformActivityKind::MoveToCore);
    assert_eq!((moved.state, moved.credits), (done, 3 * DASH));
    // Opus m3: the fee taken, not the estimate.
    assert_eq!(out.fee_credits, Some(w.net.fee));
    assert_eq!(moved.fee_credits, Some(w.net.fee));
}

/// Opus m4: the move is recorded pending before anything is signed.
#[test]
fn a_move_is_recorded_before_it_is_signed() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.move_all(&w.f.grant(3 * DASH)).unwrap();
    assert_eq!(*w.net.recorded_before_signing.lock().unwrap(), vec![true]);
}

/// Sol M2: an answer whose proof does not show the nonce is no execution:
/// the move stays pending and holds the next one off; it is done once a
/// proof shows its nonce.
#[test]
fn an_answer_without_the_nonce_stays_pending() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    w.net.then(Act::AnswersOnly);
    let out = w.move_all(&w.f.grant(3 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Pending);
    let grant = w.f.grant(3 * DASH);
    assert_eq!(
        w.move_all(&grant).unwrap_err().code(),
        "credits.move_pending"
    );
    assert!(w.f.unused(&grant));
    // It lands after all.
    let sent = w.net.sent.lock().unwrap()[0].clone();
    w.net.execute(&sent, a);
    let activity = w.activity();
    let moved = find(&activity, PlatformActivityKind::MoveToCore);
    assert_eq!(moved.state, PlatformActivityState::Done);
}

/// Sol M1 and the owner's rule: a lost move stays pending however long;
/// a proof above its first read's height fails it, and then a next move
/// must sign its very inputs at its nonces, even with a larger address
/// funded since. The two settle from one proof.
#[test]
fn an_unknown_move_holds_its_inputs_until_settled() {
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    w.net.then(Act::Lost);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(first.state, PlatformActivityState::Pending);
    w.activity();
    assert_eq!(
        w.moves()[0].state,
        PlatformActivityState::Pending,
        "same height"
    );
    w.net.fund(b, 9 * DASH);
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
    // The retry: from `a` alone, at the same nonce, though `b` is larger.
    let retry = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    let sent = w.net.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].keys().collect::<Vec<_>>(), vec![&a]);
    assert_eq!(sent[1][&a].0, sent[0][&a].0, "the same nonce");
    // The retry executed and was answered: it is done, the first failed.
    assert_eq!(retry.state, PlatformActivityState::Done);
    let moves = w.moves();
    assert_eq!(
        (moves[0].state, moves[0].can_land),
        (PlatformActivityState::Failed, false)
    );
    let outgoing = (w.activity().iter())
        .filter(|r| r.kind == PlatformActivityKind::Outgoing)
        .count();
    assert_eq!(outgoing, 0);
}

/// A move that executed but whose answer was lost is done on its nonce.
#[test]
fn an_unanswered_move_is_done_on_its_nonce() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.net.then(Act::ExecutesUnanswered);
    let out = w.move_all(&w.f.grant(3 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
}

/// Opus m2: refused by the structure check after signing, the move can
/// never execute: failed at once with that error, and the next one moves.
#[test]
fn a_structure_refusal_fails_at_once() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.net.then(Act::Invalid);
    assert!(w.move_all(&w.f.grant(3 * DASH)).is_err());
    let moves = w.moves();
    assert_eq!(
        (moves[0].state, moves[0].can_land),
        (PlatformActivityState::Failed, false)
    );
    assert_eq!(
        w.move_all(&w.f.grant(3 * DASH)).unwrap().state,
        PlatformActivityState::Done
    );
}

/// Opus m4 and n2: every refusal before the signature keeps the grant
/// and sends nothing.
#[test]
fn refusals_keep_the_grant() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let grant = w.f.grant(3 * DASH);
    let code = |r: Result<MoveToCoreOutcome, CreditsError>| r.unwrap_err().code();
    // Short of the quote.
    let small = w.f.grant(DASH);
    assert_eq!(code(w.move_all(&small)), "platform.grant_exceeded");
    assert!(w.f.unused(&small));
    // A lease of another flow.
    let other = w.f.flow(FlowKind::ProfileEdit, vec![w.f.grant(3 * DASH)]);
    assert_eq!(code(w.move_all(&other)), "platform.grant_invalid");
    // Not one of the wallet's receiving addresses.
    use dpp::dashcore::hashes::Hash;
    let theirs = dpp::dashcore::Address::new(
        w.f.s.vault.network(),
        dpp::dashcore::address::Payload::PubkeyHash(dpp::dashcore::PubkeyHash::from_byte_array(
            [7; 20],
        )),
    )
    .to_string();
    assert_eq!(
        code(w.move_to(WithdrawAmount::All, &theirs, &grant)),
        "invalid_argument"
    );
    assert!(w.f.unused(&grant));
    assert!(w.net.sent.lock().unwrap().is_empty());
    // A lease of this flow moves.
    let lease = w.f.flow(FlowKind::PlatformAddressMove, vec![grant]);
    w.move_all(&lease).unwrap();
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
}

/// Opus m6: under an enforcing mode with the fallback latch set, a move
/// whose proofs do not verify against SPV moves nothing and keeps the
/// grant.
#[test]
fn an_unverified_read_moves_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    gate_session(
        &w.f.s,
        TrustMode::Enforcing,
        FakePlatform::new(Proof::Verified),
    );
    w.net.unverified.store(true, Ordering::SeqCst);
    let grant = w.f.grant(3 * DASH);
    assert_eq!(
        w.move_all(&grant).unwrap_err().code(),
        "platform.context_unavailable"
    );
    assert!(w.f.unused(&grant));
    assert!(w.net.sent.lock().unwrap().is_empty());
    w.net.unverified.store(false, Ordering::SeqCst);
    w.move_all(&grant).unwrap();
}

/// Sol m3: a proof older than the baseline's (a node behind) records no
/// activity.
#[test]
fn an_older_proof_records_no_activity() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    assert_eq!(w.activity().len(), 1);
    w.net.state.lock().unwrap().get_mut(&a).unwrap().1 = DASH;
    w.net.height.fetch_sub(5, Ordering::SeqCst);
    assert_eq!(w.activity().len(), 1);
}
