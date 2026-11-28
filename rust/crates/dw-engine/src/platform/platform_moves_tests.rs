use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use dpp::consensus::ConsensusError;
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
    let (rows, next) = changes(
        None,
        &first,
        &held,
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (10, 1),
    );
    assert_eq!(
        kinds(&rows),
        vec![
            (PlatformActivityKind::Incoming, "a".into(), 500),
            (PlatformActivityKind::Incoming, "b".into(), 70),
        ]
    );
    assert_eq!(next, first);
    let (rows, again) = changes(
        Some(&next),
        &first,
        &held,
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (11, 2),
    );
    assert!(rows.is_empty());
    assert_eq!(again, first);
    let later = snap(&[("a", 650)]);
    let (rows, next) = changes(
        Some(&first),
        &later,
        &held,
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (12, 3),
    );
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
    let (rows, next) = changes(
        Some(&before),
        &now,
        &held,
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (5, 5),
    );
    assert!(rows.is_empty());
    assert_eq!(
        next, before,
        "a pending move's input keeps its stored value"
    );

    let mut unseen: BTreeMap<String, u64> = [("a".to_owned(), 905)].into();
    let (rows, next) = changes(
        Some(&before),
        &now,
        &BTreeSet::new(),
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (6, 6),
    );
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(next, now);
    assert_eq!(unseen["a"], 0, "spent once read");

    let mut unseen: BTreeMap<String, u64> = [("a".to_owned(), 800)].into();
    let (rows, _) = changes(
        Some(&before),
        &now,
        &BTreeSet::new(),
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (7, 7),
    );
    assert_eq!(
        kinds(&rows),
        vec![(PlatformActivityKind::Outgoing, "a".into(), 100)]
    );
    // Not read yet (the balance did not move): kept for the next read.
    let mut unseen: BTreeMap<String, u64> = [("b".to_owned(), 50)].into();
    let (_, _) = changes(
        Some(&before),
        &now,
        &BTreeSet::new(),
        &mut unseen,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        (8, 8),
    );
    assert_eq!(unseen["b"], 50);
}

fn record(id: &str, at: u64, inputs: &[(&str, u32)]) -> MoveRecord {
    MoveRecord {
        id: id.into(),
        kind: MoveKind::ToCore,
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
        fee_from_output: false,
        at,
        state: PlatformActivityState::Pending,
        signed: true,
        answered: false,
        can_land: true,
        seen_at: None,
        fee_taken: None,
        group: None,
        unattributed: false,
        version: None,
        rule: HOLD_RULE,
        cancel_of: None,
        exit_of: None,
        link_conflict: false,
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
        band: 1,
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

// ---- the move against a scripted network ----

/// What the next `withdraw` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    /// Executes and answers.
    Executes,
    /// Answers with the addresses unchanged: no execution.
    AnswersOnly,
    /// Executes; the answer is lost.
    ExecutesUnanswered,
    /// Neither: the broadcast is lost.
    Lost,
    /// Signed and broadcast; a node answers with a structure error.
    Invalid,
    /// The process stops before anything is signed.
    StopsBeforeSigning,
    /// The process stops once every input is signed, before the broadcast.
    StopsAfterSigning,
    /// Executes; the process stops before the call returns.
    StopsAfterBroadcast,
    /// Executes and answers; the process stops in the read that follows,
    /// before the outcome is stored.
    StopsBeforeOutcome,
    /// The transition sent `n`-th (from 0) executes instead of this one,
    /// which is answered with the addresses' snapshot.
    EarlierExecutes(usize),
    /// Signed and broadcast; a node answers with a signature error.
    SignatureRefused,
    /// Signed and broadcast; the wait ends unconfirmed, a structure refusal
    /// its diagnostic. Nothing executes.
    Unconfirmed,
    /// The protocol version rises after the pre-check, then a node answers
    /// the signed call with a structure error.
    VersionRisesThenInvalid,
    /// Signed and broadcast; Platform answers that an input's balance is
    /// short (a state error: nothing proves it never executes).
    NotEnoughFunds,
    /// The signer refuses every input: the transition, short of its
    /// witnesses, is refused as Platform's checks refuse a malformed one
    /// (it fails for good: not every input was signed), with nothing signed.
    Unsigned,
}

pub(crate) struct FakeMoves {
    /// address → (nonce, credits) on "Platform".
    state: Mutex<BTreeMap<PlatformAddress, (u32, u64)>>,
    height: AtomicU64,
    acts: Mutex<VecDeque<Act>>,
    /// The fee an executed transition meters.
    fee: AtomicU64,
    /// The protocol version the network runs.
    version: Mutex<&'static PlatformVersion>,
    sent: Mutex<Vec<BTreeMap<PlatformAddress, (u32, u64)>>>,
    /// Reads use a key SPV does not verify.
    unverified: AtomicBool,
    /// Where the moves are stored, to see one recorded before signing.
    store: Mutex<Option<(Arc<dw_appdb::AppDb>, String)>>,
    pub(crate) recorded_before_signing: Mutex<Vec<bool>>,
    /// Each sent transition's fee source (`None`: its output pays it).
    sources: Mutex<Vec<Option<PlatformAddress>>>,
    /// Each sent transition's output, a transfer's.
    outputs: Mutex<Vec<Option<(PlatformAddress, u64)>>>,
    /// The next read stops the process.
    stop_in_read: AtomicBool,
    /// The next read uses the fallback, setting the latch: the session's
    /// provenance.
    sets_latch: Mutex<Option<Arc<super::super::provenance::Provenance>>>,
    /// The next read waits, once it is in, for `read_release`.
    block_read: AtomicBool,
    /// What each sent transition does besides moving its inputs.
    effects: Mutex<Vec<Effect>>,
    /// The withdrawal documents executed withdrawals created.
    documents: Mutex<BTreeSet<Identifier>>,
    /// The identities registrations created.
    pub(crate) identities: Mutex<BTreeSet<Identifier>>,
    /// An executed create makes no identity: it fails inside its block,
    /// taking its nonces and its fee.
    pub(crate) create_fails: AtomicBool,
    /// Reads of a withdrawal document fail.
    documents_down: AtomicBool,
    /// What executed top-ups added to the identity.
    topped_up: AtomicU64,
    pub(crate) read_entered: tokio::sync::Notify,
    pub(crate) read_release: tokio::sync::Notify,
    /// How far below the address proofs' height a document read's proof is.
    pub(crate) document_lag: AtomicU64,
    /// How far an exit's document read lags the nonce reads (#762).
    pub(crate) exit_lag: AtomicU64,
    /// Each signing's count of `pending` contact payout rows on disk.
    pub(crate) payout_rows_at_signing: Mutex<Vec<usize>>,
    /// The withdrawal document reads made so far.
    withdrawal_reads: AtomicUsize,
    /// The `MoveNet` calls made so far.
    calls: AtomicUsize,
    /// The addresses each read asked for, in order.
    pub(crate) read_sets: Mutex<Vec<BTreeSet<PlatformAddress>>>,
}

impl FakeMoves {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::default(),
            height: AtomicU64::new(100),
            acts: Mutex::default(),
            // Below the estimate, as Platform's actual fee is.
            fee: AtomicU64::new(Limits::at(PlatformVersion::latest()).unwrap().fee(1) / 2),
            version: Mutex::new(PlatformVersion::latest()),
            sent: Mutex::default(),
            unverified: AtomicBool::new(false),
            store: Mutex::default(),
            recorded_before_signing: Mutex::default(),
            sources: Mutex::default(),
            outputs: Mutex::default(),
            stop_in_read: AtomicBool::new(false),
            sets_latch: Mutex::default(),
            block_read: AtomicBool::new(false),
            effects: Mutex::default(),
            documents: Mutex::default(),
            identities: Mutex::default(),
            create_fails: AtomicBool::new(false),
            documents_down: AtomicBool::new(false),
            topped_up: AtomicU64::new(0),
            read_entered: tokio::sync::Notify::new(),
            read_release: tokio::sync::Notify::new(),
            document_lag: AtomicU64::new(0),
            exit_lag: AtomicU64::new(0),
            payout_rows_at_signing: Mutex::default(),
            withdrawal_reads: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            read_sets: Mutex::default(),
        })
    }

    /// The `MoveNet` calls made so far.
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The withdrawal document reads made so far: none without a payout.
    pub(crate) fn withdrawal_reads(&self) -> usize {
        self.withdrawal_reads.load(Ordering::SeqCst)
    }

    /// The executed withdrawal documents.
    pub(crate) fn documents(&self) -> usize {
        self.documents.lock().unwrap().len()
    }

    /// Another device takes `address`'s next nonce, spending nothing.
    pub(crate) fn take_nonce(&self, address: PlatformAddress) {
        self.state.lock().unwrap().get_mut(&address).unwrap().0 += 1;
        self.tick();
    }

    /// A lagging node: reads prove `address` one nonce behind (`true`), or
    /// current again (`false`).
    pub(crate) fn lag(&self, address: PlatformAddress, behind: bool) {
        let mut state = self.state.lock().unwrap();
        let nonce = &mut state.get_mut(&address).unwrap().0;
        *nonce = if behind { *nonce - 1 } else { *nonce + 1 };
    }

    /// Platform's metered fee from now on.
    pub(crate) fn set_fee(&self, fee: u64) {
        self.fee.store(fee, Ordering::SeqCst);
    }

    pub(crate) fn set_version(&self, version: &'static PlatformVersion) {
        *self.version.lock().unwrap() = version;
    }

    /// The transitions sent so far.
    pub(crate) fn sent(&self) -> Vec<BTreeMap<PlatformAddress, (u32, u64)>> {
        self.sent.lock().unwrap().clone()
    }

    /// The next read waits for `read_release` once it is in (and says so
    /// with `read_entered`), before it reads anything.
    pub(crate) fn block_next_read(&self) {
        self.block_read.store(true, Ordering::SeqCst);
    }

    /// Watches `session`'s moves of `wallet`, to see each recorded before
    /// it is signed.
    pub(crate) fn watch(&self, session: &NetworkSession, wallet: WalletId) {
        let live = session.live().unwrap();
        *self.store.lock().unwrap() =
            Some((live.appdb, dw_appdb::local_scope(&wallet.to_string())));
    }

    pub(crate) fn fund(&self, address: PlatformAddress, credits: u64) {
        let mut state = self.state.lock().unwrap();
        state.entry(address).or_insert((0, 0)).1 += credits;
        self.tick();
    }

    pub(crate) fn tick(&self) {
        self.height.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn then(&self, act: Act) {
        self.acts.lock().unwrap().push_back(act);
    }

    pub(crate) fn credits(&self, address: PlatformAddress) -> u64 {
        self.state.lock().unwrap().get(&address).map_or(0, |s| s.1)
    }

    pub(crate) fn fee(&self) -> u64 {
        self.fee.load(Ordering::SeqCst)
    }

    /// Executes `inputs` as Drive would: every nonce the stored one + 1;
    /// the metered fee from what `source` keeps, all of it if need be, or
    /// from the output when `None` (dpp's `DeductFromInput` and
    /// `ReduceOutput`); a fee they cannot cover executes nothing.
    fn execute(
        &self,
        inputs: &BTreeMap<PlatformAddress, (u32, u64)>,
        source: Option<PlatformAddress>,
        output: Option<(PlatformAddress, u64)>,
    ) -> bool {
        self.execute_as(inputs, source, output, &Effect::Moves)
    }

    /// [`Self::execute`], and what the transition does besides: a
    /// withdrawal creates its document (its id from the first input's
    /// nonce and address and the payout script, as Drive derives it), a
    /// top-up adds its claims to the identity.
    fn execute_as(
        &self,
        inputs: &BTreeMap<PlatformAddress, (u32, u64)>,
        source: Option<PlatformAddress>,
        output: Option<(PlatformAddress, u64)>,
        effect: &Effect,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if !(inputs.iter()).all(|(a, (n, _))| state.get(a).is_some_and(|s| s.0 + 1 == *n)) {
            return false;
        }
        let fee = self.fee();
        let covered = match (source, output) {
            (Some(a), _) => state[&a].1 - inputs[&a].1 >= fee,
            (None, Some((_, credits))) => credits >= fee,
            (None, None) => true,
        };
        if !covered {
            return false;
        }
        for (a, (n, c)) in inputs {
            let s = state.get_mut(a).unwrap();
            s.0 = *n;
            s.1 -= c + if Some(*a) == source { fee } else { 0 };
        }
        if let Some((to, credits)) = output {
            let paid = credits - if source.is_none() { fee } else { 0 };
            state.entry(to).or_insert((0, 0)).1 += paid;
        }
        drop(state);
        match effect {
            Effect::Moves => {}
            Effect::Withdrawal(script) => {
                use dpp::document::Document;
                use dpp::system_data_contracts::withdrawals_contract;
                let (first, (nonce, _)) = inputs.first_key_value().unwrap();
                let mut entropy = nonce.to_be_bytes().to_vec();
                entropy.extend_from_slice(&first.to_bytes());
                entropy.extend_from_slice(script);
                self.documents
                    .lock()
                    .unwrap()
                    .insert(Document::generate_document_id_v0(
                        &withdrawals_contract::ID,
                        &withdrawals_contract::OWNER_ID,
                        "withdrawal",
                        &entropy,
                    ));
            }
            Effect::TopUp => {
                let claims: u64 = inputs.values().map(|(_, c)| *c).sum();
                self.topped_up.fetch_add(claims, Ordering::SeqCst);
            }
            Effect::Register(id) => {
                if !self.create_fails.load(Ordering::SeqCst) {
                    self.identities.lock().unwrap().insert(*id);
                }
            }
        }
        self.tick();
        true
    }

    /// Executes the `n`-th sent transition (from 0) as Drive would.
    pub(crate) fn execute_sent(&self, n: usize) -> bool {
        let inputs = self.sent.lock().unwrap()[n].clone();
        let source = self.sources.lock().unwrap()[n];
        let output = self.outputs.lock().unwrap()[n];
        let effect = self.effects.lock().unwrap()[n].clone();
        self.execute_as(&inputs, source, output, &effect)
    }

    /// The scripted broadcast of a withdrawal (`output` `None`) or a
    /// transfer.
    async fn broadcast(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        output: Option<(PlatformAddress, u64)>,
        (fee_strategy, effect): (AddressFundsFeeStrategy, Effect),
        signer: &VaultPlatformAddressSigner,
    ) -> Result<(), Box<PlatformWalletError>> {
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
        let payouts = (self.store.lock().unwrap().as_ref()).map_or(0, |(db, scope)| {
            let wallet = scope.trim_start_matches("local:");
            (db.dp_contact_payouts(wallet).unwrap().iter())
                .filter(|r| r.state == "pending")
                .count()
        });
        self.payout_rows_at_signing.lock().unwrap().push(payouts);
        let act = self.acts.lock().unwrap().pop_front();
        let act = act.unwrap_or(Act::Executes);
        if act == Act::StopsBeforeSigning {
            panic!("the process stops");
        }
        if act == Act::Unsigned {
            use dpp::consensus::basic::state_transition::TransitionNoInputsError;
            return Err(Box::new(PlatformWalletError::Sdk(
                TransitionNoInputsError::new().into(),
            )));
        }
        for address in inputs.keys() {
            signer.sign(address, b"transition").await.unwrap();
        }
        let source = match fee_strategy.as_slice() {
            [AddressFundsFeeStrategyStep::DeductFromInput(i)] => {
                Some(*inputs.keys().nth(usize::from(*i)).unwrap())
            }
            [AddressFundsFeeStrategyStep::ReduceOutput(0)] if output.is_some() => None,
            _ => panic!("{fee_strategy:?}"),
        };
        self.sent.lock().unwrap().push(inputs.clone());
        self.sources.lock().unwrap().push(source);
        self.outputs.lock().unwrap().push(output);
        self.effects.lock().unwrap().push(effect.clone());
        let lost = || {
            PlatformWalletError::Sdk(dash_sdk::Error::TimeoutReached(
                Duration::from_secs(1),
                "wait".into(),
            ))
        };
        let result = match act {
            Act::Executes => {
                self.execute_as(&inputs, source, output, &effect);
                Ok(())
            }
            Act::AnswersOnly => Ok(()),
            Act::ExecutesUnanswered => {
                self.execute_as(&inputs, source, output, &effect);
                Err(lost())
            }
            Act::Lost => Err(lost()),
            Act::Invalid => {
                use dpp::consensus::basic::state_transition::TransitionNoInputsError;
                Err(PlatformWalletError::Sdk(
                    TransitionNoInputsError::new().into(),
                ))
            }
            Act::Unconfirmed => {
                use dpp::consensus::basic::state_transition::TransitionNoInputsError;
                Err(PlatformWalletError::Sdk(
                    dash_sdk::Error::StateTransitionResultUnconfirmed(Box::new(
                        TransitionNoInputsError::new().into(),
                    )),
                ))
            }
            Act::VersionRisesThenInvalid => {
                use dpp::consensus::basic::state_transition::TransitionNoInputsError;
                *self.version.lock().unwrap() = raised();
                Err(PlatformWalletError::Sdk(
                    TransitionNoInputsError::new().into(),
                ))
            }
            Act::SignatureRefused => {
                use dpp::consensus::signature::InvalidStateTransitionSignatureError;
                Err(PlatformWalletError::Sdk(
                    ConsensusError::from(InvalidStateTransitionSignatureError::new("node".into()))
                        .into(),
                ))
            }
            Act::NotEnoughFunds => {
                use dpp::consensus::state::address_funds::AddressNotEnoughFundsError;
                use dpp::consensus::state::state_error::StateError;
                let (address, (_, credits)) = inputs.first_key_value().unwrap();
                let short = AddressNotEnoughFundsError::new(*address, 0, *credits);
                Err(PlatformWalletError::Sdk(
                    ConsensusError::from(StateError::AddressNotEnoughFundsError(short)).into(),
                ))
            }
            Act::StopsBeforeSigning | Act::StopsAfterSigning => panic!("the process stops"),
            Act::Unsigned => unreachable!("returned before signing"),
            Act::StopsAfterBroadcast => {
                self.execute_as(&inputs, source, output, &effect);
                panic!("the process stops")
            }
            Act::StopsBeforeOutcome => {
                self.execute_as(&inputs, source, output, &effect);
                self.stop_in_read.store(true, Ordering::SeqCst);
                Ok(())
            }
            Act::EarlierExecutes(n) => {
                self.execute_sent(n);
                Ok(())
            }
        };
        result.map_err(Box::new)
    }
}

impl MoveNet for FakeMoves {
    fn version(&self) -> &'static PlatformVersion {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.version.lock().unwrap()
    }

    async fn read(&self, addresses: BTreeSet<PlatformAddress>) -> Result<Proved, PlatformError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.stop_in_read.swap(false, Ordering::SeqCst) {
            panic!("the process stops");
        }
        if self.block_read.swap(false, Ordering::SeqCst) {
            self.read_entered.notify_one();
            self.read_release.notified().await;
        }
        let latch = self.sets_latch.lock().unwrap().take();
        if let Some(provenance) = latch {
            // A trusted-service key served this read's proofs.
            provenance.fallback_in_use().unwrap();
            self.unverified.store(true, Ordering::SeqCst);
        }
        trust::note_lookup(!self.unverified.load(Ordering::SeqCst));
        self.read_sets.lock().unwrap().push(addresses.clone());
        let state = self.state.lock().unwrap();
        let infos = (addresses.into_iter())
            .map(|a| (a, state.get(&a).copied()))
            .collect();
        Ok((self.height.load(Ordering::SeqCst), infos))
    }

    async fn withdraw(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        script: CoreScript,
        fee_strategy: AddressFundsFeeStrategy,
        signer: &VaultPlatformAddressSigner,
    ) -> Result<(), PlatformWalletError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let effect = Effect::Withdrawal(script.as_bytes().to_vec());
        (self
            .broadcast(inputs, None, (fee_strategy, effect), signer)
            .await)
            .map_err(|e| *e)
    }

    async fn withdrawal(&self, id: dpp::prelude::Identifier) -> Result<(bool, u64), PlatformError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.withdrawal_reads.fetch_add(1, Ordering::SeqCst);
        trust::note_lookup(!self.unverified.load(Ordering::SeqCst));
        let height = self.height.load(Ordering::SeqCst);
        let lag = self.document_lag.load(Ordering::SeqCst);
        let present = self.documents.lock().unwrap().contains(&id);
        Ok((present, height.saturating_sub(lag)))
    }

    async fn transfer(
        &self,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        to: (PlatformAddress, u64),
        fee_strategy: AddressFundsFeeStrategy,
        signer: &VaultPlatformAddressSigner,
    ) -> Result<(), PlatformWalletError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self
            .broadcast(inputs, Some(to), (fee_strategy, Effect::Moves), signer)
            .await)
            .map_err(|e| *e)
    }

    async fn top_up(
        &self,
        _identity: &Identity,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        signer: &VaultPlatformAddressSigner,
    ) -> Result<u64, PlatformWalletError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // dpp's constructor fixes the fee on input 0.
        let strategy = vec![AddressFundsFeeStrategyStep::DeductFromInput(0)];
        (self
            .broadcast(inputs, None, (strategy, Effect::TopUp), signer)
            .await)
            .map_err(|e| *e)?;
        Ok(self.topped_up.load(Ordering::SeqCst))
    }

    async fn withdrawal_document(&self, id: Identifier) -> Result<(bool, u64), PlatformError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.documents_down.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        trust::note_lookup(!self.unverified.load(Ordering::SeqCst));
        let present = self.documents.lock().unwrap().contains(&id);
        let lag = self.exit_lag.load(Ordering::SeqCst);
        Ok((
            present,
            self.height.load(Ordering::SeqCst).saturating_sub(lag),
        ))
    }

    async fn register(
        &self,
        identity: &Identity,
        inputs: BTreeMap<PlatformAddress, (u32, u64)>,
        _index: u32,
        (_, signer): (&VaultIdentitySigner, &VaultPlatformAddressSigner),
    ) -> Result<Identifier, PlatformWalletError> {
        let id = dpp::state_transition::identity_id_from_input_addresses(&inputs)
            .map_err(|e| PlatformWalletError::Sdk(e.into()))?;
        assert_eq!(identity.id(), id, "the identity its inputs give");
        let strategy = vec![AddressFundsFeeStrategyStep::DeductFromInput(0)];
        (self
            .broadcast(inputs, None, (strategy, Effect::Register(id)), signer)
            .await)
            .map_err(|e| *e)?;
        Ok(id)
    }

    async fn identity_exists(&self, id: Identifier) -> Result<(bool, u64), PlatformError> {
        if self.documents_down.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        trust::note_lookup(!self.unverified.load(Ordering::SeqCst));
        let present = self.identities.lock().unwrap().contains(&id);
        Ok((present, self.height.load(Ordering::SeqCst)))
    }
}

/// What a sent transition does besides moving its inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// A transfer's output, or nothing more.
    Moves,
    /// A withdrawal to the payout script.
    Withdrawal(Vec<u8>),
    /// A top-up of the identity by its claims.
    TopUp,
    /// A registration's create of the identity with this id.
    Register(Identifier),
}

/// A session with a wallet, its Platform payment addresses and `net`.
pub(in crate::platform) struct World {
    pub(in crate::platform) f: Fixture,
    pub(in crate::platform) net: Arc<FakeMoves>,
    pool: Vec<PlatformAddress>,
    to: String,
}

impl World {
    pub(crate) fn new() -> Self {
        Self::over(Fixture::new(0))
    }

    /// [`Self::new`] on a session with Platform on, so a proved protocol
    /// version reaches the pre-check.
    pub(crate) fn with_platform() -> Self {
        Self::over(Fixture::with_platform())
    }

    /// [`Self::new`] with the session on `network`.
    pub(crate) fn on(network: crate::DashNetwork) -> Self {
        Self::over(Fixture::on(network, 0))
    }

    fn over(f: Fixture) -> Self {
        let net = FakeMoves::new();
        let mut w = Self {
            f,
            net,
            pool: Vec::new(),
            to: String::new(),
        };
        let (pool, to) = w
            .call(|_, s, wallet, id| async move {
                let to = s
                    .current_receiving_address(id, &Default::default())
                    .await
                    .unwrap();
                Ok((pool(&wallet).await?.into_keys().collect(), to.to_string()))
            })
            .unwrap();
        (w.pool, w.to) = (pool, to);
        let live = w.f.s.live().unwrap();
        *w.net.store.lock().unwrap() =
            Some((live.appdb, dw_appdb::local_scope(&w.f.wallet.to_string())));
        w
    }

    pub(crate) fn call<T: Send + 'static, Fut>(
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

    pub(crate) fn address(&self, n: usize) -> PlatformAddress {
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
            move_to_core(&*net, &s, &w, id, (amount, &to, None), &grant).await
        })
    }

    fn move_all(&self, grant: &str) -> Result<MoveToCoreOutcome, CreditsError> {
        self.move_to(WithdrawAmount::All, &self.to.clone(), grant)
    }

    pub(crate) fn activity(&self) -> Vec<PlatformActivity> {
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
    assert_eq!(w.net.credits(a), estimate - w.net.fee());
    let done = PlatformActivityState::Done;
    let activity = w.activity();
    assert_eq!(activity.len(), 2, "{activity:?}");
    let incoming = find(&activity, PlatformActivityKind::Incoming);
    assert_eq!((incoming.state, incoming.credits), (done, 3 * DASH));
    let moved = find(&activity, PlatformActivityKind::MoveToCore);
    assert_eq!((moved.state, moved.credits), (done, 3 * DASH));
    // Opus m3: the fee taken, not the estimate.
    assert_eq!(out.fee_credits, Some(w.net.fee()));
    assert_eq!(moved.fee_credits, Some(w.net.fee()));
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
    assert_eq!(
        serde_json::to_value(&out).unwrap()["pending_dispatch"],
        "broadcast"
    );
    let activity = w.activity();
    let row = find(&activity, PlatformActivityKind::MoveToCore);
    assert_eq!(
        serde_json::to_value(row).unwrap()["pending_dispatch"],
        "broadcast"
    );
    let grant = w.f.grant(3 * DASH);
    assert_eq!(
        w.move_all(&grant).unwrap_err().code(),
        "credits.move_pending"
    );
    assert!(w.f.unused(&grant));
    // It lands after all.
    let sent = w.net.sent.lock().unwrap()[0].clone();
    w.net.execute(&sent, Some(a), None);
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
    // Without a contact payout, settling reads no withdrawal document.
    assert_eq!(w.net.withdrawal_reads(), 0);
}

/// A move whose wait ends unconfirmed may still execute, whatever its
/// diagnostic says: pending and able to land, its inputs held, so the next
/// move is `credits.move_pending` and nothing is signed again.
#[test]
fn an_unconfirmed_move_is_pending_and_holds_its_inputs() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.net.then(Act::Unconfirmed);
    let out = w.move_all(&w.f.grant(3 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Pending);
    assert_eq!(
        serde_json::to_value(&out).unwrap()["pending_dispatch"],
        "queued"
    );
    let moves = w.moves();
    let mut preparing = moves[0].clone();
    preparing.signed = false;
    preparing.answered = false;
    assert_eq!(
        serde_json::to_value(preparing.activity()).unwrap()["pending_dispatch"],
        "preparing"
    );
    let activity = w.activity();
    let row = find(&activity, PlatformActivityKind::MoveToCore);
    assert_eq!(
        serde_json::to_value(row).unwrap()["pending_dispatch"],
        "queued"
    );
    assert!(moves[0].signed && moves[0].can_land && moves[0].holds());
    assert_eq!(
        w.move_all(&w.f.grant(3 * DASH)).unwrap_err().code(),
        "credits.move_pending"
    );
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
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

/// #471: a node's structure or signature error to a signed move is one
/// node's answer, not proof. The move keeps its hold: a send is refused
/// until proof, and the move's exact re-send, the same inputs at the same
/// nonces, goes out and settles it.
#[test]
fn a_consensus_error_after_signing_keeps_a_moves_hold() {
    for act in [Act::Invalid, Act::SignatureRefused] {
        let w = World::new();
        w.net.fund(w.address(0), 3 * DASH);
        w.net.then(act);
        let out = w.move_all(&w.f.grant(3 * DASH)).unwrap();
        assert_eq!(out.state, PlatformActivityState::Pending, "{act:?}");
        assert!(w.moves()[0].holds(), "{act:?}");
        w.net.tick();
        w.activity();
        let held = w.moves()[0].clone();
        assert_eq!(
            (held.state, held.can_land),
            (PlatformActivityState::Failed, true),
            "{act:?}"
        );
        let (_, to) = w.theirs(9);
        let one = WithdrawAmount::Credits { credits: DASH };
        assert_eq!(
            w.send(one, &to, &w.f.grant(20 * DASH)).unwrap_err().code(),
            "credits.move_pending",
            "{act:?}"
        );
        assert_eq!(
            w.net.sent.lock().unwrap().len(),
            1,
            "{act:?}: nothing else signed"
        );
        let again = w.move_all(&w.f.grant(3 * DASH)).unwrap();
        assert_eq!(again.state, PlatformActivityState::Done, "{act:?}");
        assert_same_transition(&w, 0, 1);
        w.activity();
        assert!(w.moves().iter().all(|m| !m.holds()), "{act:?}");
    }
}

/// #471: a record keeps the outcome rule it was written under. One stored
/// before the field existed reads as rule 0, one written now as
/// [`HOLD_RULE`], and both survive a write and a read unchanged.
#[test]
fn a_move_record_keeps_its_outcome_rule() {
    let now = record("m", 1, &[("a", 4)]);
    let mut stored = serde_json::to_value(&now).unwrap();
    assert_eq!(stored["rule"], HOLD_RULE);
    let back: MoveRecord = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(back, now);
    stored.as_object_mut().unwrap().remove("rule");
    let legacy: MoveRecord = serde_json::from_value(stored).unwrap();
    assert_eq!(legacy.rule, 0);
    let again: MoveRecord = serde_json::from_str(&serde_json::to_string(&legacy).unwrap()).unwrap();
    assert_eq!(again, legacy);

    let mut w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.net.then(Act::Lost);
    w.move_all(&w.f.grant(3 * DASH)).unwrap();
    w.reopen();
    assert_eq!(w.moves()[0].rule, HOLD_RULE, "written and read back");
}

/// #471: the rule is set when a record is made, never later. A record
/// stored before the field existed keeps rule 0 through the move that
/// rewrites every record and through a restart; the new move's record
/// carries [`HOLD_RULE`].
#[test]
fn a_record_stored_before_the_rule_keeps_rule_0() {
    let mut w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.net.fund(w.address(1), 3 * DASH);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.move_to(one, &w.to.clone(), &w.f.grant(3 * DASH)).unwrap();
    assert_eq!(first.state, PlatformActivityState::Done);
    let (db, scope) = w.net.store.lock().unwrap().clone().unwrap();
    let key = format!("{MOVE_PREFIX}{}", first.id);
    let mut stored: serde_json::Value =
        serde_json::from_str(&db.setting(&scope, &key).unwrap().unwrap()).unwrap();
    stored.as_object_mut().unwrap().remove("rule");
    db.set_setting_durable(&scope, &key, Some(&stored.to_string()))
        .unwrap();
    w.reopen();

    let second = w.move_to(one, &w.to.clone(), &w.f.grant(3 * DASH)).unwrap();
    assert_eq!(second.state, PlatformActivityState::Done);
    w.reopen();
    let moves = w.moves();
    let rule = |id: &str| moves.iter().find(|m| m.id == id).unwrap().rule;
    assert_eq!(rule(&first.id), 0, "stored before the rule");
    assert_eq!(rule(&second.id), HOLD_RULE, "made now");
}

/// #471: once proof shows the refused move's nonces taken (it executed
/// through another node), the hold is released and a send goes out.
#[test]
fn a_moves_hold_after_a_consensus_error_ends_on_proof() {
    for act in [Act::Invalid, Act::SignatureRefused] {
        let w = World::new();
        w.net.fund(w.address(0), 5 * DASH);
        w.net.then(act);
        w.move_to(
            WithdrawAmount::Credits { credits: DASH },
            &w.to.clone(),
            &w.f.grant(3 * DASH),
        )
        .unwrap();
        let (them, to) = w.theirs(9);
        let one = WithdrawAmount::Credits { credits: DASH };
        assert_eq!(
            w.send(one, &to, &w.f.grant(20 * DASH)).unwrap_err().code(),
            "credits.move_pending",
            "{act:?}"
        );
        w.lands(0);
        w.activity();
        assert_eq!(w.moves()[0].state, PlatformActivityState::Done, "{act:?}");
        let out = w.send(one, &to, &w.f.grant(20 * DASH)).unwrap();
        assert_eq!(out.state, PlatformActivityState::Done, "{act:?}");
        assert_eq!(w.net.credits(them), DASH, "{act:?}");
    }
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

// ---- review round 2 ----

/// The app databases whose next outcome write fails (by address).
static FAILING_OUTCOME_WRITES: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());

/// `commit_outcome`'s injected failure: once, for `appdb`.
/// The next outcome write of `session` fails.
pub(crate) fn fail_next_outcome_write_of(session: &NetworkSession) {
    let appdb = session.live().unwrap().appdb;
    let key = Arc::as_ptr(&appdb) as usize;
    FAILING_OUTCOME_WRITES.lock().unwrap().insert(key);
}

pub(crate) fn outcome_write_fails(appdb: &Arc<dw_appdb::AppDb>) -> bool {
    let key = Arc::as_ptr(appdb) as usize;
    FAILING_OUTCOME_WRITES.lock().unwrap().remove(&key)
}

/// Sets the `FromShielded` record of `activity` to `state` in place.
pub(crate) async fn set_record_state(
    session: &NetworkSession,
    wallet: WalletId,
    activity: [u8; 32],
    state: PlatformActivityState,
) {
    let store = Store::of(session, wallet).unwrap();
    let mut record = (store.shielded_record(&hex::encode(activity)).await)
        .unwrap()
        .unwrap();
    record.state = state;
    store
        .commit(vec![from_shielded_write(&record).unwrap()])
        .await
        .unwrap();
}

static FAILING_ARMS: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());

/// The next arming of a `FromShielded` record of the session's wallets
/// fails (a stop between the artifact and the arming).
pub(crate) fn fail_next_arm_of(session: &NetworkSession) {
    let appdb = session.live().unwrap().appdb;
    FAILING_ARMS
        .lock()
        .unwrap()
        .insert(Arc::as_ptr(&appdb) as usize);
}

pub(crate) fn arm_write_fails(appdb: &Arc<dw_appdb::AppDb>) -> bool {
    FAILING_ARMS
        .lock()
        .unwrap()
        .remove(&(Arc::as_ptr(appdb) as usize))
}

impl World {
    /// Closes the session and opens it again over the same `app.sqlite`,
    /// `wallet.sqlite` and journal (a restart).
    pub(crate) fn reopen(&mut self) {
        (self.f.engine)
            .block_on(self.f.engine.close_network(crate::DashNetwork::Regtest))
            .unwrap();
        let options = crate::SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            quorum_url: Some("http://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            no_platform: true,
            ..Default::default()
        };
        self.f.s = (self.f.engine)
            .block_on(
                self.f
                    .engine
                    .open_network(crate::DashNetwork::Regtest, options),
            )
            .unwrap();
        let live = self.f.s.live().unwrap();
        *self.net.store.lock().unwrap() = Some((
            live.appdb,
            dw_appdb::local_scope(&self.f.wallet.to_string()),
        ));
    }

    pub(crate) fn fail_next_outcome_write(&self) {
        let appdb = self.f.s.live().unwrap().appdb;
        let key = Arc::as_ptr(&appdb) as usize;
        FAILING_OUTCOME_WRITES.lock().unwrap().insert(key);
    }

    fn quote(&self, amount: WithdrawAmount) -> Result<MoveToCoreQuote, CreditsError> {
        self.call(move |net, s, w, id| async move { quote(&*net, &s, &w, id, amount).await })
    }
}

/// Review r2 R2-M1: a move whose process stopped anywhere after its record
/// (before signing, after signing, after the broadcast, before its outcome
/// was stored) is settled after the restart by proofs alone, as a live
/// one: done once its nonces are reached; else pending at the first read,
/// failed and holding at a higher one, and then the same inputs at the
/// same nonces move. (At 1c732e2a such a record could never land, so
/// nothing settled it and every later move was `move_pending`.)
#[test]
fn a_move_stopped_anywhere_is_settled_after_a_restart() {
    let one = WithdrawAmount::Credits { credits: DASH };
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
        assert!(
            w.move_to(one, &w.to, &w.f.grant(2 * DASH)).is_err(),
            "{stop:?}"
        );
        let state = |w: &World| w.moves()[0].state;
        assert_eq!(state(&w), PlatformActivityState::Pending, "{stop:?}");
        // The restart: the first read.
        w.activity();
        if executed {
            assert_eq!(state(&w), PlatformActivityState::Done, "{stop:?}");
            let next = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
            assert_eq!(next.state, PlatformActivityState::Done, "{stop:?}");
            continue;
        }
        assert_eq!(state(&w), PlatformActivityState::Pending, "{stop:?}");
        let grant = w.f.grant(2 * DASH);
        assert_eq!(
            w.move_to(one, &w.to, &grant).unwrap_err().code(),
            "credits.move_pending",
            "{stop:?}"
        );
        assert!(w.f.unused(&grant));
        w.net.fund(b, 9 * DASH);
        w.activity();
        assert_eq!(state(&w), PlatformActivityState::Failed, "{stop:?}");
        let retry = w.move_to(one, &w.to, &grant).unwrap();
        assert_eq!(retry.state, PlatformActivityState::Done, "{stop:?}");
        let sent = w.net.sent.lock().unwrap().clone();
        let last = sent.last().unwrap();
        assert_eq!(last.keys().collect::<Vec<_>>(), vec![&a], "{stop:?}");
        assert_eq!(last[&a].0, 1, "the nonce the stopped move recorded");
    }
}

/// Review r2 R2-M1: a failed outcome write leaves the record stored before
/// signing, which the next read settles; the next move then moves.
#[test]
fn a_failed_outcome_write_is_settled_by_the_next_read() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.fail_next_outcome_write();
    let one = WithdrawAmount::Credits { credits: DASH };
    let out = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(
        w.moves()[0].state,
        PlatformActivityState::Pending,
        "not stored"
    );
    let activity = w.activity();
    let moved = find(&activity, PlatformActivityKind::MoveToCore);
    assert_eq!(moved.state, PlatformActivityState::Done);
    let next = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(next.state, PlatformActivityState::Done);
}

/// Review r2 R2-m1: a held move and its retry with another amount, the
/// earlier one executing while the retry is answered with a snapshot. No
/// proof says which executed: one group `Done` with the proved debit, no
/// payout or fee attributed, the other `replaced`, no outgoing row. (At
/// 1c732e2a the answered retry won: its payout, and the earlier one's
/// extra debit reported as its fee.)
#[test]
fn a_retry_with_other_terms_settles_as_one_group() {
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    w.net.then(Act::Lost);
    let first = w
        .move_to(
            WithdrawAmount::Credits { credits: DASH },
            &w.to,
            &w.f.grant(2 * DASH),
        )
        .unwrap();
    assert_eq!(first.state, PlatformActivityState::Pending);
    w.net.fund(b, 9 * DASH);
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
    let before = w.net.credits(a);
    w.net.then(Act::EarlierExecutes(0));
    let retry = w
        .move_to(
            WithdrawAmount::Credits { credits: DASH / 2 },
            &w.to,
            &w.f.grant(2 * DASH),
        )
        .unwrap();
    let debit = before - w.net.credits(a);
    assert_eq!(debit, DASH + w.net.fee(), "the earlier one executed");
    assert_eq!(retry.state, PlatformActivityState::Done);
    assert_eq!(
        (retry.credits, retry.expected_duffs, retry.fee_credits),
        (debit, None, None)
    );
    assert_eq!(retry.to.as_deref(), Some(w.to.as_str()), "one destination");
    let activity = w.activity();
    let moves: Vec<_> = (activity.iter())
        .filter(|r| r.kind == PlatformActivityKind::MoveToCore)
        .map(|r| {
            (
                r.id.as_str(),
                r.state,
                r.credits,
                r.expected_duffs,
                r.fee_credits,
            )
        })
        .collect();
    assert_eq!(
        moves,
        vec![
            (
                retry.id.as_str(),
                PlatformActivityState::Done,
                debit,
                None,
                None
            ),
            (
                first.id.as_str(),
                PlatformActivityState::Replaced,
                first.credits,
                first.expected_duffs,
                None
            ),
        ]
    );
    assert!(
        activity
            .iter()
            .all(|r| r.kind != PlatformActivityKind::Outgoing),
        "{activity:?}"
    );
}

/// R2-m1, the rule alone: moves of one key with other terms are one group.
#[test]
fn differing_terms_settle_as_a_group() {
    let mut f = record("f", 1, &[("a", 4)]);
    f.state = PlatformActivityState::Failed;
    f.seen_at = Some(5);
    f.inputs[0].before = 1_000;
    let mut r = record("r", 2, &[("a", 4)]);
    (
        r.answered,
        r.seen_at,
        r.inputs[0].credits,
        r.inputs[0].before,
    ) = (true, Some(15), 9, 900);
    let mut moves = vec![f, r];
    let landed = Seen {
        height: 20,
        at: [("a".into(), Some((4, 300)))].into(),
    };
    assert_eq!(settle(&mut moves, &landed), BTreeSet::from([0, 1]));
    assert_eq!(
        (moves[0].state, moves[0].can_land),
        (PlatformActivityState::Replaced, false)
    );
    assert_eq!(moves[1].state, PlatformActivityState::Done);
    assert_eq!(
        moves[1].group,
        Some(GroupOutcome {
            debit: Some(600),
            to: Some("y".into())
        })
    );
    assert_eq!((moves[1].fee_taken, moves[1].inputs[0].unseen), (None, 600));
    let row = moves[1].activity();
    assert_eq!(
        (row.credits, row.expected_duffs, row.fee_credits),
        (600, None, None)
    );
    // Nothing says which of them executed: every row says so, the newest too.
    assert!(
        moves
            .iter()
            .all(|m| m.unattributed && m.activity().unattributed)
    );
}

/// Moves of one key with equal terms leave the same history whichever
/// executed: the group names its winner, and no row is unattributed.
#[test]
fn equal_terms_settle_attributed() {
    let mut f = record("f", 1, &[("a", 4)]);
    f.state = PlatformActivityState::Failed;
    f.seen_at = Some(5);
    let mut r = record("r", 2, &[("a", 4)]);
    (r.answered, r.seen_at) = (true, Some(15));
    let mut moves = vec![f, r];
    let landed = Seen {
        height: 20,
        at: [("a".into(), Some((4, 300)))].into(),
    };
    settle(&mut moves, &landed);
    assert_eq!(moves[1].state, PlatformActivityState::Done);
    assert!(
        moves
            .iter()
            .all(|m| !m.unattributed && !m.activity().unattributed)
    );
}

/// Review r2 R2-m2: in an enforcing mode with the latch clear, the read
/// that sets it (its proofs used the trusted service) decides nothing: the
/// quote and the move are refused, no grant redeemed, nothing signed, and
/// the activity records nothing. (At 1c732e2a the latch was sampled before
/// the read, so that read went through.)
#[test]
fn the_read_that_sets_the_latch_is_gated() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    w.f.s.trust.set_mode(TrustMode::Enforcing);
    (w.f.s.platform.money_gate).set_waits(super::super::money_gate::Waits {
        wait: Duration::from_millis(400),
        retry: Duration::from_millis(50),
        probe: Duration::from_millis(200),
    });
    let provenance = w.f.s.live().unwrap().provenance;
    assert!(!provenance.fallback_used());
    let latch = |w: &World| *w.net.sets_latch.lock().unwrap() = Some(Arc::clone(&provenance));
    latch(&w);
    assert_eq!(
        w.quote(WithdrawAmount::All).unwrap_err().code(),
        "platform.context_unavailable"
    );
    assert!(provenance.fallback_used(), "the read set it");
    let grant = w.f.grant(3 * DASH);
    assert_eq!(
        w.move_all(&grant).unwrap_err().code(),
        "platform.context_unavailable"
    );
    assert!(w.f.unused(&grant));
    assert!(w.net.sent.lock().unwrap().is_empty());
    assert!(w.activity().is_empty(), "nothing recorded");
}

/// The nit of review r1 (n6) and r2: only the library's in-flight refusal
/// waits for the running pass; another `AddressSync` error is the pass's.
#[test]
fn only_the_in_flight_refusal_waits_for_the_running_pass() {
    let pin = "a platform-address sync pass is already in flight; retry once it completes";
    assert!(pass_in_flight(&PlatformWalletError::AddressSync(
        pin.into()
    )));
    assert!(!pass_in_flight(&PlatformWalletError::AddressSync(
        "a proof did not verify".into()
    )));
}

/// Review r2 R2-m3 on the test-only fund route: a lock only 51 of the
/// wallet's coins reach is refused before a grant is redeemed or anything
/// signed.
#[cfg(all(feature = "test-fund", debug_assertions))]
#[test]
fn the_fund_route_refuses_a_lock_of_more_than_fifty_coins() {
    use dashcore::hashes::Hash;
    let w = World::new();
    let duffs = 3_000_000;
    let wallet = w.f.engine.block_on(w.f.s.wallet(&w.f.wallet)).unwrap();
    w.f.engine.block_on(async {
        let address = (wallet.core().next_receive_address_for_account(0).await).unwrap();
        let mut state = wallet.state_mut().await;
        let account = (state.core_wallet.accounts.standard_bip44_accounts)
            .get_mut(&0)
            .unwrap();
        for vout in 0..51 {
            let outpoint =
                dashcore::OutPoint::new(dashcore::Txid::from_byte_array([0xed; 32]), vout);
            let mut utxo = key_wallet::Utxo::new(
                outpoint,
                dashcore::TxOut {
                    value: duffs / 51 + 1_000,
                    script_pubkey: address.script_pubkey(),
                },
                address.clone(),
                1,
                false,
            );
            utxo.is_confirmed = true;
            account.utxos.insert(outpoint, utxo);
        }
    });
    let address_grant = w.f.grant(DASH);
    let spend_grant = (w.f.s.vault)
        .authorize(
            dw_vault::GrantPurpose::Spend { max_duffs: duffs },
            Some(&w.f.wallet.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    let funded = w.f.engine.block_on(w.f.s.fund_platform_addresses_for_test(
        w.f.wallet,
        duffs,
        address_grant.clone(),
        spend_grant.clone(),
    ));
    let e = funded.unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e}");
    assert!(e.to_string().contains("would use 51 coins"), "{e}");
    assert!(w.f.unused(&address_grant));
    let spend = (w.f.s.vault).check_grant(
        &spend_grant,
        dw_vault::GrantKind::Spend,
        Some(&w.f.wallet.0),
    );
    assert!(spend.is_ok(), "{spend:?}");
}

/// Review r3 R3-m3: a held move to one own address and its retry to
/// another, the earlier one executing while the retry is answered with a
/// snapshot. Nothing says which address was paid: the group's outcome, as
/// its activity row, names none (at 016082be the outcome named the
/// retry's).
#[test]
fn a_group_with_two_destinations_names_none() {
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    w.net.then(Act::Lost);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    w.net.fund(b, 9 * DASH);
    w.activity();
    // The current address is not recorded as issued, so the first issue
    // may be `w.to` itself: issue until another own address comes.
    let other = std::iter::repeat_with(|| {
        (w.f.engine)
            .block_on(w.f.s.next_receive_address(w.f.wallet, None))
            .unwrap()
            .address
    })
    .take(3)
    .find(|a| *a != w.to)
    .unwrap();
    w.net.then(Act::EarlierExecutes(0));
    let retry = w.move_to(one, &other, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(first.to.as_deref(), Some(w.to.as_str()));
    assert_eq!(retry.state, PlatformActivityState::Done);
    assert_eq!(
        (retry.to.as_deref(), retry.expected_duffs, retry.fee_credits),
        (None, None, None)
    );
    let activity = w.activity();
    let row = (activity.iter()).find(|r| r.id == retry.id).unwrap();
    assert_eq!(
        (row.state, row.to.as_deref()),
        (PlatformActivityState::Done, None)
    );
}

// ---- 0.2 X1b: the receive address and sends to Platform addresses ----

impl World {
    fn network(&self) -> Network {
        self.f.s.vault.network()
    }

    /// A Platform address that is not the wallet's, in this network's form.
    fn theirs(&self, byte: u8) -> (PlatformAddress, String) {
        let a = PlatformAddress::P2pkh([byte; 20]);
        (a, a.to_bech32m_string(self.network()))
    }

    /// Quotes `amount` to `to`, then confirms the quote under `grant`.
    fn send(
        &self,
        amount: WithdrawAmount,
        to: &str,
        grant: &str,
    ) -> Result<PlatformSendOutcome, CreditsError> {
        let quote = self.send_quote(amount, to)?;
        self.confirm(&quote.operation, grant)
    }

    /// `platform_send` of the quoted `operation` under `grant`, issued for
    /// it as `authorize_platform_send` issues one.
    fn confirm(&self, operation: &str, grant: &str) -> Result<PlatformSendOutcome, CreditsError> {
        self.bind(operation, grant);
        self.pay(operation, grant)
    }

    /// Binds `grant` to the quoted `operation` (`platform_send_bind`); an
    /// unknown or sent one binds nothing.
    fn bind(&self, operation: &str, grant: &str) {
        let _ = (self.f.s.platform.send_ops).bind(operation, self.f.wallet, grant);
    }

    /// `platform_send` of `operation` under `grant` as it is.
    fn pay(&self, operation: &str, grant: &str) -> Result<PlatformSendOutcome, CreditsError> {
        let (operation, grant) = (operation.to_owned(), grant.to_owned());
        self.call(
            move |net, s, w, id| async move { send(&*net, &s, &w, id, &operation, &grant).await },
        )
    }

    fn send_quote(
        &self,
        amount: WithdrawAmount,
        to: &str,
    ) -> Result<PlatformSendQuote, CreditsError> {
        self.retry_quote(amount, to, None)
    }

    /// The quote of a send the host names as the retry of `retry_of`.
    fn retry_quote(
        &self,
        amount: WithdrawAmount,
        to: &str,
        retry_of: Option<&str>,
    ) -> Result<PlatformSendQuote, CreditsError> {
        let (to, retry_of) = (to.to_owned(), retry_of.map(str::to_owned));
        self.call(move |net, s, w, id| async move {
            let ask = (to.as_str(), amount, retry_of.as_deref());
            send_quote(&*net, &s, &w, id, ask).await
        })
    }

    fn receive_address(&self) -> PlatformReceiveAddress {
        self.call(|net, s, w, _| async move {
            Ok(receive_address(&*net, &s, &w, s.vault.network()).await?)
        })
        .unwrap()
    }
}

fn reason(r: Result<impl std::fmt::Debug, CreditsError>) -> RecipientProblem {
    match r {
        Err(CreditsError::RecipientInvalid { reason }) => reason,
        other => panic!("{other:?}"),
    }
}

/// A bech32m string of `hrp` over `payload` (dw-uri's shielded form when
/// `payload` is the shielded type byte and 43 bytes).
fn bech32m_of(hrp: &str, payload: &[u8]) -> String {
    bech32::encode::<bech32::Bech32m>(bech32::Hrp::parse(hrp).unwrap(), payload).unwrap()
}

/// R1: the receive address is the payment account's lowest unused one, in
/// the network's form; asking again gives the same address until it is
/// seen funded, then the next one.
#[test]
fn the_receive_address_is_the_lowest_unused_and_stable() {
    let w = World::new();
    // The payment account's addresses by their index.
    let by_index: BTreeMap<u32, String> = (w.call(|_, s, wallet, _| async move {
        let network = s.vault.network();
        Ok((pool(&wallet).await?.into_iter())
            .map(|(a, path)| {
                (
                    u32::from(*path.as_ref().last().unwrap()),
                    bech32m(&a, network),
                )
            })
            .collect())
    }))
    .unwrap();
    let first = w.receive_address();
    assert_eq!(first, w.receive_address(), "stable until funded");
    assert_eq!((first.index, &first.address), (0, &by_index[&0]));
    assert!(first.address.starts_with("tdash1"), "{}", first.address);
    // A sync pass funds it (platform-wallet's `on_address_found` write).
    let funded = parse(&first.address).unwrap();
    w.call(move |_, _, wallet, _| async move {
        use key_wallet::KeySource;
        use key_wallet::account::account_collection::PlatformPaymentAccountKey;
        let key = PlatformPaymentAccountKey {
            account: 0,
            key_class: 0,
        };
        let mut wm = wallet.wallet_manager().write().await;
        let (core, info) = wm.get_wallet_mut_and_info_mut(&wallet.wallet_id()).unwrap();
        let xpub = core.accounts.platform_payment_accounts[&key].account_xpub;
        let account = (info.core_wallet)
            .platform_payment_managed_account_at_index_mut(0)
            .unwrap();
        let PlatformAddress::P2pkh(hash) = funded else {
            unreachable!()
        };
        let p2pkh = PlatformP2PKHAddress::new(hash);
        account.set_address_credit_balance(p2pkh, DASH, Some(&KeySource::Public(xpub)));
        Ok(())
    })
    .unwrap();
    let next = w.receive_address();
    assert_eq!((next.index, &next.address), (1, &by_index[&1]));
}

/// C-funding F13 (mobile parity, on the condition that it is never handed
/// out again): the fallback a Shielded-funded registration fixed is
/// published beside its identity, so the receive address skips it. The
/// row is its record; the account's mark is in memory only, so a wallet
/// that never marked it (a restart) skips it too. Mutant: hand out an
/// address without marking the rows' fallbacks.
#[test]
fn a_registration_fallback_is_never_the_receive_address() {
    let w = World::new();
    let first = w.receive_address();
    assert_eq!(first.index, 0);
    insert_shielded_row(&w, &first.address);
    let next = w.receive_address();
    assert_eq!(next.index, 1, "{next:?}");
    assert_ne!(next.address, first.address);
    assert_eq!(w.receive_address(), next, "stable");
}

/// A Shielded-funded registration row of `fallback`.
fn insert_shielded_row(w: &World, fallback: &str) {
    let funding = super::super::registration_rows::Funding::Shielded {
        denomination: DASH,
        fallback: fallback.to_owned(),
        identity: None,
    }
    .encode();
    let row = dw_appdb::NewRegistration {
        identity_index: Some(0),
        identity: None,
        label: "alice".into(),
        temp_label: None,
        funding,
        initial_profile: None,
        phase: "draft".into(),
    };
    w.call(move |_, s, _, id| async move {
        let wallet = id.to_string();
        s.appdb_op(move |db| db.insert_registration(&wallet, &row, 1))
            .await
            .map_err(PlatformError::from)?;
        Ok(())
    })
    .unwrap();
}

/// The account's index of `address`.
fn index_of(w: &World, address: String) -> Option<u32> {
    w.call(move |_, _, wallet, _| async move {
        let target = parse(&address).unwrap();
        Ok(pool(&wallet)
            .await?
            .get(&target)
            .map(|p| u32::from(*p.as_ref().last().unwrap())))
    })
    .unwrap()
}

fn new_fallback(w: &World) -> String {
    w.call(|_, s, wallet, _| async move {
        Ok(fallback_address(&s, &wallet, s.vault.network()).await?)
    })
    .unwrap()
}

/// `address` published by a create, as the dispatch hook marks it.
fn mark_published(w: &World, address: &str) {
    let key = dw_appdb::dp_registration_published(address);
    w.call(move |_, s, _, id| async move {
        let scope = dw_appdb::local_scope(&id.to_string());
        s.appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("1")))
            .await
            .map_err(PlatformError::from)?;
        Ok(())
    })
    .unwrap();
}

/// A new fallback a create then published: never given again.
fn published_fallback(w: &World) -> String {
    let address = new_fallback(w);
    mark_published(w, &address);
    address
}

/// The account's in-memory marks of `indices` lost, as at a restart.
fn forget_marks(w: &World, indices: std::ops::RangeInclusive<u32>) {
    w.call(move |_, _, wallet, _| async move {
        use key_wallet::managed_account::address_pool::AddressState;
        let mut wm = wallet.wallet_manager().write().await;
        let info = wm.get_wallet_info_mut(&wallet.wallet_id()).unwrap();
        let account = (info.core_wallet)
            .platform_payment_managed_account_at_index_mut(0)
            .unwrap();
        for i in indices {
            let mut info = (account.addresses.addresses().get(&i).cloned()).unwrap();
            info.state = AddressState::Available;
            account.addresses.insert(info).unwrap();
        }
        Ok(())
    })
    .unwrap();
}

/// C-funding F13: a new fallback is never the address Receive may be
/// showing: that one and the fallback after it are marked used. Its
/// durable record outlives the account's in-memory mark (a restart) and
/// its registration's row. Mutants: give the shown address; forget the
/// record.
#[test]
fn a_new_fallback_is_never_an_address_handed_out_before_or_after() {
    let w = World::new();
    let shown = w.receive_address();
    assert_eq!(shown.index, 0);
    let fallback = new_fallback(&w);
    assert_eq!(
        index_of(&w, fallback.clone()),
        Some(1),
        "not the shown address"
    );
    assert_eq!(w.receive_address().index, 2);
    forget_marks(&w, 1..=2);
    let next = w.receive_address();
    assert_eq!(next.index, 2, "{next:?}");
    assert_ne!(next.address, fallback);
}

/// C-funding F13 across a restart: every mark the account kept in memory
/// is lost, yet the next fallback is above every address handed out before
/// it (the highest is recorded durably), and Receive keeps showing the
/// address it showed. Mutants: no mark below the highest handed out; no
/// record of it.
#[test]
fn a_fallback_after_a_restart_is_above_every_address_handed_out() {
    let w = World::new();
    assert_eq!(w.receive_address().index, 0);
    let first = published_fallback(&w);
    assert_eq!(index_of(&w, first.clone()), Some(1));
    let shown = w.receive_address();
    assert_eq!(shown.index, 2);
    forget_marks(&w, 0..=2);
    assert_eq!(w.receive_address(), shown, "Receive's address stays");
    let second = published_fallback(&w);
    assert_eq!(index_of(&w, second.clone()), Some(3), "above the shown 2");
    assert_ne!(second, first);
    let after = w.receive_address();
    assert_eq!(after.index, 4, "{after:?}");
    forget_marks(&w, 0..=4);
    assert_eq!(w.receive_address(), after, "stable after a second restart");
}

/// C-funding F13: an unreadable record of what was handed out refuses
/// every hand-out instead of maybe handing out an address shown before.
/// Mutants: read either record as absent.
#[test]
fn an_unreadable_hand_out_record_refuses_every_hand_out() {
    for key in [
        dw_appdb::DP_PLATFORM_RECEIVE_HANDED_OUT.to_owned(),
        dw_appdb::dp_registration_fallback("unreadable"),
    ] {
        let w = World::new();
        w.call(move |_, s, _, id| async move {
            let scope = dw_appdb::local_scope(&id.to_string());
            s.appdb_op(move |db| db.set_setting_durable(&scope, &key, Some("x")))
                .await
                .map_err(PlatformError::from)?;
            Ok(())
        })
        .unwrap();
        let refused = w
            .call(|net, s, wallet, _| async move {
                let network = s.vault.network();
                let receive = receive_address(&*net, &s, &wallet, network).await;
                let fallback = fallback_address(&s, &wallet, network).await;
                Ok([
                    receive.err().map(|e| format!("{e:?}")),
                    fallback.err().map(|e| format!("{e:?}")),
                ])
            })
            .unwrap();
        for e in refused {
            assert!(
                e.as_deref().is_some_and(|e| e.contains("unreadable")),
                "{e:?}"
            );
        }
    }
}

/// A reload that has derived the payment account only up to `last`, with
/// no address marked: what the account holds before a sync restores it.
fn reload_derived_to(w: &World, last: u32) {
    w.call(move |_, _, wallet, _| async move {
        use key_wallet::managed_account::address_pool::{AddressInfo, AddressPool, AddressState};
        let mut wm = wallet.wallet_manager().write().await;
        let info = wm.get_wallet_info_mut(&wallet.wallet_id()).unwrap();
        let pool = &mut (info.core_wallet)
            .platform_payment_managed_account_at_index_mut(0)
            .unwrap()
            .addresses;
        let mut reloaded = AddressPool::new_without_generation(
            pool.base_path().clone(),
            pool.pool_type(),
            pool.gap_limit(),
            pool.network(),
        );
        for info in pool.addresses().range(..=last).map(|(_, info)| info) {
            let info = AddressInfo {
                state: AddressState::Available,
                ..info.clone()
            };
            reloaded.insert(info).unwrap();
        }
        *pool = reloaded;
        Ok(())
    })
    .unwrap();
}

/// C-funding F13: a fallback and the addresses handed out below the
/// highest are skipped even before a reload derives them again. Mutant:
/// mark them without deriving them first.
#[test]
fn a_fallback_not_derived_again_after_a_reload_is_still_skipped() {
    let w = World::new();
    assert_eq!(w.receive_address().index, 0);
    let fallback = published_fallback(&w);
    assert_eq!(index_of(&w, fallback.clone()), Some(1));
    let shown = w.receive_address();
    assert_eq!(shown.index, 2);
    reload_derived_to(&w, 0);
    assert_eq!(index_of(&w, fallback.clone()), None, "not derived");
    assert_eq!(w.receive_address(), shown, "never the fallback");
    let next = published_fallback(&w);
    assert_eq!(index_of(&w, next), Some(3));
}

/// C-funding F13 (bot r4702054059): after a reload that derived only index
/// 0, the activity read still asks for every recorded fallback, so a
/// refund there is read as the refund it is. Mutant: activity reads
/// without deriving them first.
#[test]
fn the_activity_read_covers_every_recorded_fallback_after_a_reload() {
    let w = World::new();
    let fallback = published_fallback(&w);
    reload_derived_to(&w, 0);
    assert_eq!(index_of(&w, fallback.clone()), None, "not derived");
    w.activity();
    let read = w.net.read_sets.lock().unwrap().last().cloned();
    assert!(read.unwrap().contains(&parse(&fallback).unwrap()));
}

/// C-funding F13 (bot r4699213569): a fallback no row names and no create
/// published never left this install, so the next registration is given
/// it again: fifteen starts dropped before their create was sent take two
/// indices in all, and Receive stays where the first left it. Mutant: never
/// give one again.
#[test]
fn starts_dropped_before_dispatch_take_no_more_indices() {
    let w = World::new();
    let first = new_fallback(&w);
    assert_eq!(index_of(&w, first.clone()), Some(1));
    for _ in 0..15 {
        assert_eq!(new_fallback(&w), first);
    }
    assert_eq!(w.receive_address().index, 2);
}

/// C-funding F13: a fallback a create published, or a row still names, is
/// never given again. Mutants: give a published one again; a named one.
#[test]
fn a_published_or_named_fallback_is_never_given_again() {
    let w = World::new();
    let published = published_fallback(&w);
    let named = new_fallback(&w);
    assert_ne!(named, published);
    assert_eq!(index_of(&w, named.clone()), Some(3));
    insert_shielded_row(&w, &named);
    let third = new_fallback(&w);
    assert_eq!(index_of(&w, third.clone()), Some(5));
    assert!(third != published && third != named);
}

/// Q3: a P2SH Platform address of this network is a recipient, as a
/// P2PKH one is.
#[test]
fn a_p2sh_platform_recipient_is_accepted() {
    let w = World::new();
    let pool = w.call(|_, _, wallet, _| async move { Ok(pool(&wallet).await?) });
    let pool = pool.unwrap();
    let p2sh = PlatformAddress::P2sh([5; 20]);
    let text = p2sh.to_bech32m_string(w.network());
    assert_eq!(recipient(w.network(), &text, &pool), Ok(p2sh));
    let (p2pkh, text) = w.theirs(9);
    assert_eq!(
        recipient(w.network(), &format!(" {text}\n"), &pool),
        Ok(p2pkh)
    );
}

/// Q3: a Platform address of another network is refused as such, though
/// dpp's own parse takes either HRP.
#[test]
fn a_platform_address_of_another_network_is_refused() {
    let w = World::new();
    let pool = (w.call(|_, _, wallet, _| async move { Ok(pool(&wallet).await?) })).unwrap();
    for a in [
        PlatformAddress::P2pkh([9; 20]),
        PlatformAddress::P2sh([5; 20]),
    ] {
        let mainnet = a.to_bech32m_string(Network::Mainnet);
        assert!(PlatformAddress::from_bech32m_string(&mainnet).is_ok());
        assert_eq!(
            recipient(w.network(), &mainnet, &pool),
            Err(RecipientProblem::Network)
        );
    }
}

/// Q3: a Core or a shielded address, one of the wallet's own Platform
/// addresses, and no address at all are refused, each with its reason;
/// the send refuses them before any read, and keeps the grant.
#[test]
fn a_core_shielded_own_or_malformed_recipient_is_refused() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let mut shielded = vec![dw_uri::keyio::SHIELDED_TYPE_BYTE];
    shielded.extend([3; 43]);
    let cases = [
        (w.to.clone(), RecipientProblem::Core),
        (bech32m_of("tdash", &shielded), RecipientProblem::Shielded),
        (bech32m(&w.address(1), w.network()), RecipientProblem::Own),
        ("tdash1notanaddress".to_owned(), RecipientProblem::Malformed),
    ];
    let grant = w.f.grant(DASH);
    let one = WithdrawAmount::Credits { credits: DASH / 2 };
    for (to, want) in cases {
        assert_eq!(reason(w.send_quote(one, &to)), want, "{to}");
        assert_eq!(reason(w.send(one, &to, &grant)), want, "{to}");
    }
    assert!(w.f.unused(&grant));
    assert!(w.net.sent.lock().unwrap().is_empty());
}

/// The send's fee estimate is dpp's transfer estimate with one output.
#[test]
fn the_send_fee_is_dpps_transfer_estimate() {
    use dpp::state_transition::address_funds_transfer_transition::AddressFundsTransferTransition;
    let v = PlatformVersion::latest();
    let l = Limits::transfer_at(v);
    for n in 1..=3 {
        assert_eq!(
            l.fee(n),
            AddressFundsTransferTransition::estimate_min_fee(n, 1, v)
        );
        assert_eq!(l.keep(n), TRANSFER_FEE_BAND * l.fee(n));
    }
    let funds = &v.dpp.state_transitions.address_funds;
    assert_eq!(l.min, funds.min_output_amount.max(funds.min_input_amount));
}

/// Q4: an amount is refused up front only when no address carries it and
/// the band; the fee source keeps the band.
#[test]
fn a_send_is_refused_only_when_the_amount_and_band_do_not_fit() {
    let l = Limits::transfer_at(PlatformVersion::latest());
    let amount = 2 * l.min;
    let fits = balances(&[(1, amount + l.keep(1))]);
    let p = transfer_plan(&fits, WithdrawAmount::Credits { credits: amount }, &l).unwrap();
    assert_eq!(p.inputs, balances(&[(1, amount)]));
    assert_eq!(
        (p.fee, p.reserve, p.may_lose()),
        (l.fee(1), l.keep(1), amount + l.keep(1))
    );
    let short = balances(&[(1, amount + l.keep(1) - 1)]);
    assert_eq!(
        transfer_plan(&short, WithdrawAmount::Credits { credits: amount }, &l),
        Err(Refused::Short {
            needed: amount + l.keep(1),
            available: amount + l.keep(1) - 1,
        })
    );
    assert_eq!(
        transfer_plan(&fits, WithdrawAmount::Credits { credits: l.min - 1 }, &l),
        Err(Refused::OutOfRange)
    );
}

/// Q5: Max moves every input in full to the output, which pays the fee: no
/// remainder is left on them, and the output carries the band.
#[test]
fn max_leaves_nothing_on_its_inputs_and_covers_the_fee() {
    let l = Limits::transfer_at(PlatformVersion::latest());
    let b = balances(&[(1, 5 * DASH), (2, 2 * DASH), (3, l.min_input - 1)]);
    let p = transfer_plan(&b, WithdrawAmount::All, &l).unwrap();
    assert_eq!(p.inputs, balances(&[(1, 5 * DASH), (2, 2 * DASH)]));
    assert!(p.fee_from_output);
    assert_eq!(p.reserve, 0);
    assert_eq!(
        p.fee_strategy(),
        vec![AddressFundsFeeStrategyStep::ReduceOutput(0)]
    );
    assert!(p.credits() >= l.min + l.keep(2));
    // Too little to carry the band: refused, not sent.
    let tiny = balances(&[(1, l.min + l.keep(1) - 1)]);
    assert!(matches!(
        transfer_plan(&tiny, WithdrawAmount::All, &l),
        Err(Refused::Short { .. })
    ));

    // Against the network: the inputs end at 0, the recipient gets the
    // rest less the fee, and the send's fall is no outgoing row.
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    w.net.fund(b, DASH);
    w.activity();
    let (them, to) = w.theirs(9);
    let quote = w.send_quote(WithdrawAmount::All, &to).unwrap();
    assert!(quote.fee_from_amount);
    assert_eq!((quote.amount_credits, quote.credits), (4 * DASH, 4 * DASH));
    let out = w
        .send(WithdrawAmount::All, &to, &w.f.grant(4 * DASH))
        .unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!((w.net.credits(a), w.net.credits(b)), (0, 0), "no dust left");
    assert_eq!(w.net.credits(them), 4 * DASH - w.net.fee());
    assert_eq!(out.fee_credits, None, "the recipient's output paid it");
    let activity = w.activity();
    assert!(
        !activity
            .iter()
            .any(|r| r.kind == PlatformActivityKind::Outgoing)
    );
}

/// R3: a send pays the recipient at the nonces it signs, shows as a `Send`
/// row with its recipient and the fee taken, and its own fall is no
/// outgoing row.
#[test]
fn a_send_pays_the_recipient_and_shows_as_a_send() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quote = w.send_quote(one, &to).unwrap();
    let l = Limits::transfer_at(PlatformVersion::latest());
    // The fee may take all its source keeps: the 2 DASH left on `a`.
    assert_eq!(
        (
            quote.amount_credits,
            quote.fee_credits,
            quote.fee_source_credits
        ),
        (DASH, l.fee(1), Some(2 * DASH))
    );
    assert_eq!(
        (quote.credits, quote.to.as_str(), quote.retry_of.as_deref()),
        (3 * DASH, to.as_str(), None)
    );
    assert!(!quote.fee_from_amount);
    assert_eq!(quote.grant.max_credits, quote.credits);
    let out = w
        .confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert!(!out.may_land && !out.estimated);
    assert_eq!((out.amount_credits, out.fee_from_amount), (DASH, false));
    assert_eq!(w.net.credits(them), DASH);
    assert_eq!(w.net.credits(a), 2 * DASH - w.net.fee());
    assert_eq!(out.fee_credits, Some(w.net.fee()));
    assert_eq!(out.to.as_deref(), Some(to.as_str()));
    // Done, it shows what left the addresses: the amount and the fee taken,
    // not the band the plan kept for it.
    assert_eq!(out.credits, DASH + w.net.fee());
    let activity = w.activity();
    let sent = find(&activity, PlatformActivityKind::Send);
    assert_eq!(sent.credits, DASH + w.net.fee());
    assert_eq!(
        (sent.state, sent.to.as_deref(), sent.expected_duffs),
        (PlatformActivityState::Done, Some(to.as_str()), None)
    );
    assert!(
        !activity
            .iter()
            .any(|r| r.kind == PlatformActivityKind::Outgoing)
    );
    // Without a contact payout, settling reads no withdrawal document.
    assert_eq!(w.net.withdrawal_reads(), 0);
}

/// A send that may still land holds its inputs' nonces against a move to
/// Core, as a move does against a send: one hold for both.
#[test]
fn a_send_and_a_move_share_one_hold() {
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let (_, to) = w.theirs(9);
    w.net.then(Act::Lost);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.send(one, &to, &w.f.grant(10 * DASH)).unwrap();
    assert_eq!(first.state, PlatformActivityState::Pending);
    let grant = w.f.grant(2 * DASH);
    match w.move_to(one, &w.to, &grant) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, first.id),
        other => panic!("{other:?}"),
    }
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
}

/// A send failed by height may still land: a send to another recipient
/// at its very inputs and nonces is refused (only one of them could land,
/// and nothing would say which recipient was paid); its exact retry goes.
#[test]
fn a_failed_send_blocks_a_send_to_another_recipient() {
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fund(a, 3 * DASH);
    let (_, first_to) = w.theirs(9);
    let (_, other_to) = w.theirs(8);
    w.net.then(Act::Lost);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.send(one, &first_to, &w.f.grant(10 * DASH)).unwrap();
    assert_eq!(first.state, PlatformActivityState::Pending);
    w.net.fund(b, 9 * DASH);
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
    assert!(w.moves()[0].can_land);
    let grant = w.f.grant(10 * DASH);
    match w.send(one, &other_to, &grant) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, first.id),
        other => panic!("{other:?}"),
    }
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing else signed");
    // Its exact retry: the same transition, inputs, nonces, output and fee
    // source.
    let retry = w.send(one, &first_to, &grant).unwrap();
    assert_eq!(retry.state, PlatformActivityState::Done);
    assert_same_transition(&w, 0, 1);
}

/// Sends `a` and `b` (in sending order) are one transition: inputs and
/// nonces, output, fee source.
fn assert_same_transition(w: &World, a: usize, b: usize) {
    let sent = w.net.sent.lock().unwrap().clone();
    let outputs = w.net.outputs.lock().unwrap().clone();
    let sources = w.net.sources.lock().unwrap().clone();
    assert_eq!(sent[a], sent[b]);
    assert_eq!(outputs[a], outputs[b]);
    assert_eq!(sources[a], sources[b]);
}

/// A held send's exact retry re-signs its own terms: a payment that arrived
/// at its input since (Max would now plan more) does not hold it back, and
/// the quote shows those terms.
#[test]
fn a_failed_max_send_is_retried_on_its_own_terms() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (them, to) = w.theirs(9);
    w.net.then(Act::Lost);
    let first = w
        .send(WithdrawAmount::All, &to, &w.f.grant(3 * DASH))
        .unwrap();
    assert_eq!(first.state, PlatformActivityState::Pending);
    w.net.tick();
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
    assert!(w.moves()[0].can_land);
    w.net.fund(a, DASH);
    let quote = w.send_quote(WithdrawAmount::All, &to).unwrap();
    assert_eq!(
        (quote.amount_credits, quote.fee_from_amount),
        (3 * DASH, true)
    );
    assert_eq!(quote.retry_of.as_deref(), Some(first.id.as_str()));
    let retry = (w.confirm(&quote.operation, &w.f.grant(quote.credits))).unwrap();
    assert_eq!(retry.state, PlatformActivityState::Done);
    assert_same_transition(&w, 0, 1);
    assert_eq!(w.net.credits(them), 3 * DASH - w.net.fee());
    assert_eq!(w.net.credits(a), DASH);
}

/// While a send may still land, a move to Core, and another amount to the
/// same recipient, are refused; while a move may, a send is.
#[test]
fn a_held_send_and_a_held_move_refuse_anything_but_their_retry() {
    let one = WithdrawAmount::Credits { credits: DASH };
    let two = WithdrawAmount::Credits { credits: 2 * DASH };
    let pending = |r: Result<_, CreditsError>, id: &str| match r {
        Err(CreditsError::MovePending { id: held }) => assert_eq!(held, id),
        other => panic!("{other:?}"),
    };
    // A send held.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    w.net.then(Act::Lost);
    let first = w.send(one, &to, &w.f.grant(10 * DASH)).unwrap();
    w.net.tick();
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
    pending(
        w.move_to(one, &w.to, &w.f.grant(2 * DASH)).map(drop),
        &first.id,
    );
    pending(w.send(two, &to, &w.f.grant(3 * DASH)).map(drop), &first.id);
    pending(
        w.send(WithdrawAmount::All, &to, &w.f.grant(5 * DASH))
            .map(drop),
        &first.id,
    );
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing else signed");
    // A move held.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    w.net.then(Act::Lost);
    let first = w.move_to(one, &w.to, &w.f.grant(2 * DASH)).unwrap();
    w.net.tick();
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
    assert!(w.moves()[0].can_land);
    pending(w.send(one, &to, &w.f.grant(10 * DASH)).map(drop), &first.id);
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing else signed");
}

/// A 0.1 move record (no `kind`, no `fee_from_output`) reads as a move to
/// Core.
#[test]
fn a_0_1_move_record_reads_as_to_core() {
    let mut json = serde_json::to_value(record("m", 1, &[("x", 1)])).unwrap();
    let fields = json.as_object_mut().unwrap();
    fields.remove("kind");
    fields.remove("fee_from_output");
    let read: MoveRecord = serde_json::from_value(json).unwrap();
    assert_eq!((read.kind, read.fee_from_output), (MoveKind::ToCore, false));
    let row = read.activity();
    assert_eq!(row.kind, PlatformActivityKind::MoveToCore);
    assert_eq!(row.expected_duffs, Some(0));
}

/// A group's outcome stored before records carried `unattributed` still
/// shows no named winner; a record with no group outcome stays named.
#[test]
fn a_group_outcome_from_before_the_mark_names_no_winner() {
    let read = |group: bool| {
        let mut json = serde_json::to_value(record("m", 1, &[("x", 1)])).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.remove("unattributed");
        if group {
            let outcome = GroupOutcome {
                debit: Some(DASH),
                to: None,
            };
            fields.insert("group".into(), serde_json::to_value(outcome).unwrap());
        }
        let read: MoveRecord = serde_json::from_value(json).unwrap();
        assert!(!read.unattributed);
        read.activity().unattributed
    };
    assert!(read(true));
    assert!(!read(false));
}

impl World {
    /// A send of `amount` to `to` that goes out and is lost, then a later
    /// read shows it unexecuted: `Failed` by height, able to land.
    fn held_send(&self, amount: WithdrawAmount, to: &str) -> PlatformSendOutcome {
        self.net.then(Act::Lost);
        let out = self.send(amount, to, &self.f.grant(20 * DASH)).unwrap();
        self.net.tick();
        self.activity();
        let held = &self.moves()[0];
        assert_eq!(
            (held.state, held.can_land),
            (PlatformActivityState::Failed, true)
        );
        out
    }

    /// Executes the `n`-th transition sent, late.
    fn lands(&self, n: usize) {
        let inputs = self.net.sent.lock().unwrap()[n].clone();
        let source = self.net.sources.lock().unwrap()[n];
        let output = self.net.outputs.lock().unwrap()[n];
        self.net.execute(&inputs, source, output);
    }

    fn store(&self, records: &[MoveRecord]) {
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        let writes: Vec<_> = records.iter().map(|r| move_write(r).unwrap()).collect();
        (self.f.engine)
            .block_on(async move { Store::of(&s, id).unwrap().commit(writes).await })
            .unwrap();
    }
}

/// The held send lands between its retry's quote and the
/// Confirm: the Confirm answers that send's settled outcome and signs
/// nothing, so the recipient is paid once, also once the records reload.
#[test]
fn a_retry_after_the_held_send_landed_pays_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.held_send(one, &to);
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(quote.retry_of.as_deref(), Some(first.id.as_str()));
    w.lands(0);
    let grant = w.f.grant(quote.credits);
    let out = w.confirm(&quote.operation, &grant).unwrap();
    assert_eq!(
        (out.id.as_str(), out.state),
        (first.id.as_str(), PlatformActivityState::Done)
    );
    assert!(w.f.unused(&grant), "nothing redeemed");
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing signed");
    assert_eq!(w.net.credits(them), DASH, "paid once");
    // Reloaded: the one send, done, and no hold.
    let moves = w.moves();
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].state, PlatformActivityState::Done);
    assert!(!moves[0].holds());
    // The operation is spent; a new quote is a new payment.
    assert_eq!(
        w.confirm(&quote.operation, &w.f.grant(quote.credits))
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    assert_eq!(w.net.credits(them), DASH);
}

/// "Retry this payment" after the held send landed,
/// and Activity read it: the retry's quote still names that send, so its
/// Confirm answers that it landed and pays nothing; a quote without the
/// name is a new payment. A named retry on other terms is refused.
#[test]
fn a_named_retry_of_a_send_that_landed_pays_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.held_send(one, &to);
    w.lands(0);
    w.activity();
    assert!(!w.moves()[0].holds());
    let quote = w.retry_quote(one, &to, Some(&first.id)).unwrap();
    let grant = w.f.grant(quote.credits);
    let out = w.confirm(&quote.operation, &grant).unwrap();
    // Paid once first: a quote that dropped the name would pay
    // again here.
    assert_eq!(w.net.credits(them), DASH, "paid once");
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing signed");
    assert!(w.f.unused(&grant), "nothing redeemed");
    assert_eq!(
        (out.id.as_str(), out.state, out.may_land),
        (first.id.as_str(), PlatformActivityState::Done, false)
    );
    assert_eq!(quote.retry_of.as_deref(), Some(first.id.as_str()));
    let two = WithdrawAmount::Credits { credits: 2 * DASH };
    assert_eq!(
        w.retry_quote(two, &to, Some(&first.id)).unwrap_err().code(),
        "invalid_argument"
    );
    assert_eq!(w.send_quote(one, &to).unwrap().retry_of, None);
}

/// Still failed at the Confirm, the retry re-signs the held send once;
/// whichever lands, the recipient is paid once.
#[test]
fn a_retry_of_a_still_failed_send_is_signed_once() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.held_send(one, &to);
    let quote = w.send_quote(one, &to).unwrap();
    let out = w
        .confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_ne!(out.id, first.id);
    assert_eq!(w.net.sent.lock().unwrap().len(), 2);
    assert_same_transition(&w, 0, 1);
    // The original can no longer land: its nonces are taken.
    w.lands(0);
    w.activity();
    assert_eq!(w.net.credits(them), DASH, "paid once");
    let rows = w.activity();
    let old = rows.iter().find(|r| r.id == first.id).unwrap();
    assert_eq!(
        (old.state, old.may_land),
        (PlatformActivityState::Replaced, false)
    );
}

/// A quote made with no hold is a new send; a hold that appears before
/// its Confirm refuses it, unsigned, its grant kept.
#[test]
fn a_hold_after_a_quote_without_one_refuses_the_send() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let (_, other) = w.theirs(8);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(quote.retry_of, None);
    let held = w.held_send(one, &other);
    let grant = w.f.grant(quote.credits);
    match w.confirm(&quote.operation, &grant) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, held.id),
        other => panic!("{other:?}"),
    }
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
}

/// A hold of the very same terms that appears after a quote made
/// with none still refuses it: a new send is never re-signed at a held
/// send's inputs and nonces, however equal the terms (only a quote naming
/// it as a retry is).
#[test]
fn a_same_terms_hold_after_a_quote_without_one_refuses_the_send() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(quote.retry_of, None);
    let held = w.held_send(one, &to);
    let grant = w.f.grant(quote.credits);
    match w.confirm(&quote.operation, &grant) {
        Err(CreditsError::MovePending { id }) => assert_eq!(id, held.id),
        other => panic!("{other:?}"),
    }
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing re-signed");
    assert_eq!(w.net.credits(them), 0);
}

/// Beside a hold the quote refuses anything but the exact retry,
/// before anything is asked of the user.
#[test]
fn the_quote_refuses_beside_a_hold() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let (_, other) = w.theirs(8);
    let one = WithdrawAmount::Credits { credits: DASH };
    let held = w.held_send(one, &to);
    for (amount, to) in [
        (one, other.as_str()),
        (WithdrawAmount::Credits { credits: 2 * DASH }, to.as_str()),
        (WithdrawAmount::All, to.as_str()),
    ] {
        match w.send_quote(amount, to) {
            Err(CreditsError::MovePending { id }) => assert_eq!(id, held.id),
            other => panic!("{other:?}"),
        }
    }
    assert!(w.send_quote(one, &to).unwrap().retry_of.is_some());
}

/// `DeductFromInput` takes the metered fee from all the fee source
/// keeps (the pin caps nothing lower). The plan picks the source that
/// keeps the least, and that remainder is the bound the quote states and
/// the grant covers; a fee above three times the estimate is taken within
/// it.
#[test]
fn the_fee_bound_is_what_the_fee_source_keeps() {
    let l = Limits::transfer_at(PlatformVersion::latest());
    let small = DASH * 3 / 10;
    let b = balances(&[(1, 10 * DASH), (2, small)]);
    let quarter = WithdrawAmount::Credits { credits: DASH / 4 };
    let p = transfer_plan(&b, quarter, &l).unwrap();
    assert_eq!((p.fee_source, p.inputs.len()), (address(2), 1));
    assert_eq!(p.reserve, small - DASH / 4);
    // A source too small to carry it alone gives what it can and keeps the
    // band; the largest gives the rest.
    let b = balances(&[(1, 10 * DASH), (2, DASH / 10)]);
    let p = transfer_plan(&b, quarter, &l).unwrap();
    assert_eq!((p.fee_source, p.reserve), (address(2), l.keep(2)));
    assert_eq!(p.credits(), DASH / 4);

    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (them, to) = w.theirs(9);
    let fee = 5 * l.fee(1);
    w.net.fee.store(fee, Ordering::SeqCst);
    let quote = w
        .send_quote(WithdrawAmount::Credits { credits: DASH }, &to)
        .unwrap();
    assert!(fee > l.keep(1), "above the band");
    assert_eq!(quote.fee_source_credits, Some(2 * DASH));
    assert_eq!(quote.fee_source, Some(a.to_bech32m_string(w.network())));
    let out = w
        .confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(w.net.credits(them), DASH);
    assert_eq!((out.fee_credits, out.credits), (Some(fee), DASH + fee));
    assert!(out.credits <= quote.credits);
    // Max: the inputs give exactly their balances; the fee comes out of the
    // recipient's amount, however much it meters.
    let quote = w.send_quote(WithdrawAmount::All, &to).unwrap();
    assert_eq!(
        (quote.fee_source_credits, quote.credits),
        (None, quote.amount_credits)
    );
    let left = w.net.credits(a);
    w.confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert_eq!(w.net.credits(a), 0);
    assert_eq!(w.net.credits(them), DASH + left - fee);
}

/// What the fee source keeps is a snapshot, not a ceiling. A
/// deposit to it after the send is signed pays a larger metered fee when
/// the original lands late, so the gross debit passes the quote's planned
/// debit, its grant's cap. The outcome reads the inputs' fall, which the
/// deposit offsets: an "about" figure, never above the gross debit.
#[test]
fn a_deposit_after_signing_can_pay_a_larger_fee() {
    let estimate = Limits::transfer_at(PlatformVersion::latest()).fee(1);
    let w = World::new();
    let a = w.address(0);
    let (them, to) = w.theirs(9);
    w.net.fund(a, DASH + 3 * estimate);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(
        (quote.fee_source_credits, quote.credits),
        (Some(3 * estimate), DASH + 3 * estimate)
    );
    assert_eq!(quote.fee_source, Some(a.to_bech32m_string(w.network())));
    w.net.then(Act::Lost);
    let sent = w
        .confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert!(sent.may_land);
    let before = w.net.credits(a);
    let deposit = 2 * estimate;
    w.net.fund(a, deposit);
    w.net.fee.store(5 * estimate, Ordering::SeqCst);
    w.lands(0);
    let gross = before + deposit - w.net.credits(a);
    assert_eq!((gross, w.net.credits(them)), (DASH + 5 * estimate, DASH));
    assert!(gross > quote.credits, "the planned debit is no ceiling");
    let row = w.activity().into_iter().find(|r| r.id == sent.id).unwrap();
    assert_eq!(row.state, PlatformActivityState::Done);
    assert_eq!(
        (row.credits, row.fee_credits),
        (DASH + 3 * estimate, Some(3 * estimate))
    );
    assert!(row.credits <= gross);
}

/// The quote names the fee source Confirm shows. A deposit to another
/// address between the quote and the send makes that one the better fee
/// source; the send is refused rather than signed with it: nothing signed,
/// the grant unused, and a new quote names the new source and pays.
#[test]
fn a_deposit_elsewhere_after_the_quote_refuses_the_reviewed_send() {
    let estimate = Limits::transfer_at(PlatformVersion::latest()).fee(1);
    let w = World::new();
    // Platform meters the estimate, which the new fee source covers.
    w.net.fee.store(estimate, Ordering::SeqCst);
    let (a, b) = (w.address(0), w.address(1));
    let (them, to) = w.theirs(9);
    w.net.fund(a, 3 * DASH);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(quote.fee_source, Some(a.to_bech32m_string(w.network())));
    assert_eq!(quote.credits, 3 * DASH);
    w.net.fund(b, DASH + 3 * estimate);
    let grant = w.f.grant(quote.credits);
    let e = w.confirm(&quote.operation, &grant).unwrap_err();
    assert_eq!(e.code(), "credits.quote_changed", "{e}");
    assert!(w.net.sent.lock().unwrap().is_empty(), "nothing signed");
    assert!(w.f.unused(&grant), "nothing redeemed");
    assert!(w.moves().is_empty());
    let again = w.send_quote(one, &to).unwrap();
    assert_eq!(again.fee_source, Some(b.to_bech32m_string(w.network())));
    w.confirm(&again.operation, &w.f.grant(again.credits))
        .unwrap();
    assert_eq!(w.net.credits(them), DASH);
}

/// A deposit to the quoted fee source itself leaves the reviewed plan's fee
/// as it was (Confirm said a later deposit to it is reachable), so the fee
/// check passes. The deposit still raises the planned debit by what the
/// source now keeps: under the quote's own grant that is
/// `platform.grant_exceeded`, nothing signed; under a grant that covers it,
/// the send goes.
#[test]
fn a_deposit_to_the_quoted_fee_source_keeps_the_reviewed_fee() {
    let estimate = Limits::transfer_at(PlatformVersion::latest()).fee(1);
    let one = WithdrawAmount::Credits { credits: DASH };
    let deposited = |grant: &dyn Fn(&World, u64) -> String| {
        let w = World::new();
        let a = w.address(0);
        let (them, to) = w.theirs(9);
        w.net.fund(a, 3 * DASH);
        let quote = w.send_quote(one, &to).unwrap();
        w.net.fund(a, 2 * estimate);
        let grant = grant(&w, quote.credits);
        let out = w.confirm(&quote.operation, &grant);
        (w, them, grant, out)
    };
    let (w, them, grant, out) = deposited(&|w, cap| w.f.grant(cap));
    assert_eq!(out.unwrap_err().code(), "platform.grant_exceeded");
    assert!(w.f.unused(&grant));
    assert!(w.net.sent.lock().unwrap().is_empty());
    assert_eq!(w.net.credits(them), 0);
    let (w, them, _, out) = deposited(&|w, _| w.f.grant(20 * DASH));
    out.unwrap();
    assert_eq!(w.net.credits(them), DASH);
    assert_eq!(w.net.sent.lock().unwrap().len(), 1);
}

/// Platform's answer to an address-funds transition is a proved snapshot of
/// the addresses, not proof that this transition executed. A send it
/// answered whose fee source cannot cover the metered fee executed nothing:
/// it reads pending and may still land, never done, and is done once a
/// proof shows its nonces.
#[test]
fn an_answered_send_that_executed_nothing_reads_pending() {
    let estimate = Limits::transfer_at(PlatformVersion::latest()).fee(1);
    let w = World::new();
    let a = w.address(0);
    let (them, to) = w.theirs(9);
    w.net.fund(a, DASH + 3 * estimate);
    w.net.fee.store(5 * estimate, Ordering::SeqCst);
    let quote = w
        .send_quote(WithdrawAmount::Credits { credits: DASH }, &to)
        .unwrap();
    let out = (w.confirm(&quote.operation, &w.f.grant(quote.credits))).unwrap();
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "signed and answered");
    assert_eq!(
        (out.state, out.may_land),
        (PlatformActivityState::Pending, true)
    );
    assert_eq!(
        (w.net.credits(them), w.net.credits(a)),
        (0, DASH + 3 * estimate)
    );
    let row = w.activity().into_iter().find(|r| r.id == out.id).unwrap();
    assert_eq!(
        (row.state, row.may_land),
        (PlatformActivityState::Pending, true)
    );
    // It lands once its fee source covers the fee.
    w.net.fund(a, 2 * estimate);
    w.lands(0);
    assert_eq!(w.net.credits(them), DASH);
    let row = w.activity().into_iter().find(|r| r.id == out.id).unwrap();
    assert_eq!(row.state, PlatformActivityState::Done);
}

/// A send failed by height may still go through, in Activity too, read
/// afresh, and so may one a node refused (#471); one its identical retry
/// superseded reads as replaced.
#[test]
fn activity_tells_a_live_send_from_a_refused_or_superseded_one() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let held = w.held_send(one, &to);
    let row = |w: &World, id: &str| w.activity().into_iter().find(|r| r.id == id).unwrap();
    let r = row(&w, &held.id);
    assert_eq!(
        (r.state, r.may_land, r.credits),
        (PlatformActivityState::Failed, true, DASH)
    );
    // Its retry lands in its place.
    w.send(one, &to, &w.f.grant(20 * DASH)).unwrap();
    let r = row(&w, &held.id);
    assert_eq!(
        (r.state, r.may_land),
        (PlatformActivityState::Replaced, false)
    );

    // A node's consensus error settles nothing (#471): it may still land.
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.then(Act::Invalid);
    let refused = w.send_quote(one, &to).unwrap();
    w.confirm(&refused.operation, &w.f.grant(20 * DASH))
        .unwrap();
    w.net.tick();
    let r = (w.activity().into_iter())
        .find(|r| r.kind == PlatformActivityKind::Send)
        .unwrap();
    assert_eq!((r.state, r.may_land), (PlatformActivityState::Failed, true));
}

/// #471: a node's structure or signature error to a held send's identical
/// retry releases nothing, whatever version either was signed at. A
/// different send is refused until proof; the exact retry stays open,
/// and the recipient is paid once.
#[test]
fn a_consensus_error_to_a_sends_retry_keeps_its_hold() {
    let one = WithdrawAmount::Credits { credits: DASH };
    let two = WithdrawAmount::Credits { credits: 2 * DASH };
    for act in [Act::Invalid, Act::SignatureRefused] {
        for version in [None, Some(Some(1)), Some(None)] {
            let w = World::new();
            w.net.fund(w.address(0), 5 * DASH);
            let (them, to) = w.theirs(9);
            let held = w.held_send(one, &to);
            if let Some(v) = version {
                let mut r = w.moves()[0].clone();
                r.version = v;
                w.store(&[r]);
            }
            w.net.then(act);
            let quote = w.send_quote(one, &to).unwrap();
            w.confirm(&quote.operation, &w.f.grant(20 * DASH)).unwrap();
            assert_same_transition(&w, 0, 1);
            w.net.tick();
            w.activity();
            let still = |w: &World| (w.moves().into_iter().find(|m| m.id == held.id)).unwrap();
            assert!(still(&w).holds(), "{act:?} {version:?}");
            assert_eq!(
                w.send(two, &to, &w.f.grant(20 * DASH)).unwrap_err().code(),
                "credits.move_pending",
                "{act:?} {version:?}"
            );
            assert_eq!(w.net.sent.lock().unwrap().len(), 2, "nothing else signed");
            let quote = w.send_quote(one, &to).unwrap();
            let out = w.confirm(&quote.operation, &w.f.grant(20 * DASH)).unwrap();
            assert_eq!(
                out.state,
                PlatformActivityState::Done,
                "{act:?} {version:?}"
            );
            assert_same_transition(&w, 0, 2);
            w.lands(0);
            w.activity();
            assert_eq!(w.net.credits(them), DASH, "paid once");
            assert!(w.moves().iter().all(|m| !m.holds()), "{act:?} {version:?}");
        }
    }
}

/// A fixture version whose structure check refuses every fee strategy,
/// which no plan reads: every plan is one Platform would refuse.
pub(crate) fn no_fee_steps() -> &'static PlatformVersion {
    let mut v = PlatformVersion::latest().clone();
    v.dpp.state_transitions.max_address_fee_strategies = 0;
    Box::leak(Box::new(v))
}

/// The refusal [`no_fee_steps`] forces: Platform's own structure check,
/// with no newer network version read, which only the pre-check returns.
/// Its consensus error is not carried, so never shown.
pub(crate) fn assert_no_fee_steps(e: &CreditsError) {
    assert_eq!(*e, CreditsError::PreflightRefused);
    assert!(!format!("{e:?} {e}").contains("Fee strategy"), "{e:?}");
}

/// A pre-check's structural refusal says "rules have changed" only when
/// the proved network version is above both the planned version and the
/// latest this build knows; an unread, equal or merely newer-than-planned
/// version is the generic refusal. Other errors keep their own mapping.
#[test]
fn a_pre_check_refusal_names_a_rules_change_only_past_every_known_version() {
    use dpp::consensus::basic::BasicError;
    use dpp::consensus::basic::state_transition::FeeStrategyTooManyStepsError;
    use dpp::consensus::signature::InvalidStateTransitionSignatureError;
    let basic = || {
        let e = BasicError::FeeStrategyTooManyStepsError(FeeStrategyTooManyStepsError::new(1, 0));
        dpp::ProtocolError::ConsensusError(Box::new(ConsensusError::from(e)))
    };
    let latest = PlatformVersion::latest();
    let top = latest.protocol_version;
    let older = PlatformVersion::get(top - 1).unwrap();
    for (version, network, expected) in [
        (latest, None, CreditsError::PreflightRefused),
        (latest, Some(top), CreditsError::PreflightRefused),
        (older, Some(top), CreditsError::PreflightRefused),
        (latest, Some(top + 1), CreditsError::RulesUnsupported),
        (older, Some(top + 1), CreditsError::RulesUnsupported),
    ] {
        for kind in [MoveKind::ToCore, MoveKind::Transfer, MoveKind::ToIdentity] {
            let got = refused(basic(), kind, version, network);
            assert_eq!(got, expected, "{kind:?} {network:?}");
        }
    }
    let signature = ConsensusError::from(InvalidStateTransitionSignatureError::new("x".into()));
    let other = refused(signature.into(), MoveKind::ToCore, latest, Some(top + 1));
    assert!(matches!(other, CreditsError::Platform(_)), "{other:?}");
    let built = dpp::ProtocolError::Generic("builder".into());
    let other = refused(built, MoveKind::Transfer, latest, Some(top + 1));
    assert!(matches!(other, CreditsError::Platform(_)), "{other:?}");
}

/// The latest version, one protocol version higher.
fn raised() -> &'static PlatformVersion {
    let mut v = PlatformVersion::latest().clone();
    v.protocol_version += 1;
    Box::leak(Box::new(v))
}

/// #471 T1: a move to Core and a send whose plan Platform's structure check
/// refuses are refused before anything is signed, recorded or redeemed, and
/// hold nothing: at the real version both go through.
#[test]
fn a_plan_the_structure_check_refuses_signs_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    *w.net.version.lock().unwrap() = no_fee_steps();
    let (move_grant, send_grant) = (w.f.grant(3 * DASH), w.f.grant(20 * DASH));
    assert_no_fee_steps(&w.move_to(one, &w.to.clone(), &move_grant).unwrap_err());
    let quote = w.send_quote(one, &to).unwrap();
    assert_no_fee_steps(&w.confirm(&quote.operation, &send_grant).unwrap_err());
    assert!(w.net.sent.lock().unwrap().is_empty(), "nothing signed");
    assert!(w.moves().is_empty(), "nothing recorded");
    assert!(
        w.f.unused(&move_grant) && w.f.unused(&send_grant),
        "nothing redeemed"
    );
    *w.net.version.lock().unwrap() = PlatformVersion::latest();
    let out = w.send(one, &to, &w.f.grant(20 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_eq!(w.net.credits(them), DASH);
    let out = w.move_to(one, &w.to.clone(), &w.f.grant(3 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
}

/// #471 T3: the version rises between the pre-check and the signed call,
/// and a node answers with a structure error: the move and the send hold,
/// and nothing but proof releases them.
#[test]
fn a_version_rise_after_the_pre_check_releases_nothing() {
    let one = WithdrawAmount::Credits { credits: DASH };
    for send in [false, true] {
        let w = World::new();
        w.net.fund(w.address(0), 5 * DASH);
        let (_, to) = w.theirs(9);
        w.net.then(Act::VersionRisesThenInvalid);
        if send {
            w.send(one, &to, &w.f.grant(20 * DASH)).unwrap();
        } else {
            w.move_to(one, &w.to.clone(), &w.f.grant(3 * DASH)).unwrap();
        }
        assert_eq!(w.net.version().protocol_version, raised().protocol_version);
        for _ in 0..3 {
            w.net.tick();
            w.activity();
            assert!(w.moves()[0].holds(), "send {send}");
        }
        w.lands(0);
        w.activity();
        assert_eq!(
            w.moves()[0].state,
            PlatformActivityState::Done,
            "send {send}"
        );
    }
}

/// #471: the pre-check refusing a held send's exact retry releases nothing.
/// Nothing is signed, recorded or redeemed, and the send keeps its hold
/// until proof; at the real version its retry goes out and settles it.
#[test]
fn a_retry_the_structure_check_refuses_signs_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let held = w.held_send(one, &to);
    *w.net.version.lock().unwrap() = no_fee_steps();
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(quote.retry_of.as_deref(), Some(held.id.as_str()));
    let grant = w.f.grant(20 * DASH);
    assert_no_fee_steps(&w.confirm(&quote.operation, &grant).unwrap_err());
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing signed");
    assert!(w.f.unused(&grant), "nothing redeemed");
    assert_eq!(w.moves().len(), 1, "nothing recorded");
    assert!(w.moves()[0].holds());
    *w.net.version.lock().unwrap() = PlatformVersion::latest();
    let quote = w.send_quote(one, &to).unwrap();
    let out = w.confirm(&quote.operation, &w.f.grant(20 * DASH)).unwrap();
    assert_eq!(out.state, PlatformActivityState::Done);
    assert_same_transition(&w, 0, 1);
    assert_eq!(w.net.credits(them), DASH, "paid once");
}

/// A payment that arrives at the fee source while a send settles hides
/// its fee; the done send's debit counts the estimate and says so.
#[test]
fn a_deposit_that_hides_the_fee_marks_the_debit_estimated() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 3 * DASH);
    let (_, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let held = w.held_send(one, &to);
    let quote = w.send_quote(one, &to).unwrap();
    w.lands(0);
    w.net.fund(a, DASH);
    let out = w
        .confirm(&quote.operation, &w.f.grant(quote.credits))
        .unwrap();
    assert_eq!(out.id, held.id);
    let estimate = Limits::transfer_at(PlatformVersion::latest()).fee(1);
    assert_eq!(
        (out.estimated, out.fee_credits, out.credits),
        (true, None, DASH + estimate)
    );
    let row = w.activity().into_iter().find(|r| r.id == held.id).unwrap();
    assert!(row.estimated);
    // Without the deposit the balances show the fee: not estimated.
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let out = w.send(one, &to, &w.f.grant(20 * DASH)).unwrap();
    assert_eq!((out.estimated, out.fee_credits), (false, Some(w.net.fee())));
}

/// A move beside a held send is quoted as that send's exit (#762): planned
/// on its inputs only and naming it; once the send lands the quote names
/// nothing.
#[test]
fn the_move_quote_beside_a_held_send_is_its_exit() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    w.net.fund(w.address(1), 3 * DASH);
    let (_, to) = w.theirs(9);
    let held = w.held_send(WithdrawAmount::Credits { credits: DASH }, &to);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of.as_deref(), Some(held.id.as_str()));
    let inputs = w.moves()[0].inputs.clone();
    assert_eq!(inputs.len(), 1);
    let input = (w.pool.iter())
        .find(|a| bech32m(a, w.network()) == inputs[0].address)
        .copied()
        .unwrap();
    assert!(q.credits <= w.net.credits(input), "its one input only");
    let exit = Limits::at(PlatformVersion::latest()).unwrap();
    assert_eq!(q.fee_credits, exit.fee(1));
    w.lands(0);
    let q = w.quote(WithdrawAmount::All).unwrap();
    assert_eq!(q.exit_of, None);
}

/// A new process quotes afresh, and no operation id from before
/// it pays there, neither one sent nor one never sent, also once the new
/// process has quoted.
#[test]
fn an_operation_id_from_before_a_restart_pays_nothing() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let sent = w.send_quote(one, &to).unwrap();
    w.confirm(&sent.operation, &w.f.grant(20 * DASH)).unwrap();
    let unsent = w.send_quote(one, &to).unwrap();
    // A new process: none of the old operation state.
    w.f.s.platform.send_ops.reset();
    let fresh = [
        w.send_quote(one, &to).unwrap(),
        w.send_quote(one, &to).unwrap(),
    ];
    for old in [&sent.operation, &unsent.operation] {
        assert!(fresh.iter().all(|q| &q.operation != old));
        let e = w.confirm(old, &w.f.grant(20 * DASH)).unwrap_err();
        assert_eq!(e.code(), "invalid_argument", "{old}: {e}");
    }
    assert_eq!(w.net.credits(them), DASH);
    w.confirm(&fresh[0].operation, &w.f.grant(20 * DASH))
        .unwrap();
    assert_eq!(w.net.credits(them), 2 * DASH);
}

/// A send's grant is issued for its operation and pays that one
/// alone. Another operation's grant, or one issued for none, is refused
/// before it is redeemed: neither it nor the operation is used.
#[test]
fn a_grant_pays_only_the_operation_it_was_issued_for() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (alice, to_alice) = w.theirs(8);
    let (bob, to_bob) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let qa = w.send_quote(one, &to_alice).unwrap();
    let qb = w.send_quote(one, &to_bob).unwrap();
    assert_eq!(qa.credits, qb.credits, "the same cap");
    let dp = w.f.s.dashpay(w.f.wallet);
    let op = qa.operation.clone();
    let cap = (w.f.engine).block_on(async move { dp.platform_send_grant(op).await });
    assert_eq!(cap.unwrap().max_credits, qa.credits);
    let ga = w.f.grant(qa.credits);
    w.bind(&qa.operation, &ga);
    for grant in [ga.clone(), w.f.grant(20 * DASH)] {
        let e = w.pay(&qb.operation, &grant).unwrap_err();
        assert_eq!(e.code(), "platform.grant_invalid", "{e}");
    }
    assert!(w.net.sent.lock().unwrap().is_empty());
    w.confirm(&qb.operation, &w.f.grant(qb.credits)).unwrap();
    w.pay(&qa.operation, &ga).unwrap();
    assert_eq!((w.net.credits(alice), w.net.credits(bob)), (DASH, DASH));
}

/// A second bind replaces the first and hands it back for the host to
/// revoke: the replaced grant no longer pays the send, the new one does.
#[test]
fn a_rebind_returns_the_grant_it_replaces() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let q = w
        .send_quote(WithdrawAmount::Credits { credits: DASH }, &to)
        .unwrap();
    let ops = &w.f.s.platform.send_ops;
    let (first, second) = (w.f.grant(q.credits), w.f.grant(q.credits));
    assert_eq!(ops.bind(&q.operation, w.f.wallet, &first).unwrap(), None);
    assert_eq!(
        ops.bind(&q.operation, w.f.wallet, &second).unwrap(),
        Some(first.clone())
    );
    let e = w.pay(&q.operation, &first).unwrap_err();
    assert_eq!(e.code(), "platform.grant_invalid", "{e}");
    assert!(w.f.unused(&first));
    w.pay(&q.operation, &second).unwrap();
    assert_eq!(w.net.credits(them), DASH);
}

/// An operation is its wallet's: another wallet can neither read its cap,
/// bind a grant to it nor take it, and the operation stays quoted for its
/// own wallet, which pays it.
#[test]
fn another_wallet_cannot_use_an_operation() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let q = w
        .send_quote(WithdrawAmount::Credits { credits: DASH }, &to)
        .unwrap();
    let ops = &w.f.s.platform.send_ops;
    let other = crate::session::WalletId([7; 32]);
    let grant = w.f.grant(q.credits);
    let refused = |e: PlatformError| assert_eq!(e.code(), "invalid_argument", "{e}");
    refused(ops.cap(&q.operation, other).unwrap_err());
    refused(ops.bind(&q.operation, other, &grant).unwrap_err());
    refused(ops.take(&q.operation, other, &grant).unwrap_err());
    assert_eq!(ops.cap(&q.operation, w.f.wallet).unwrap(), q.credits);
    w.confirm(&q.operation, &grant).unwrap();
    assert_eq!(w.net.credits(them), DASH);
}

/// The session keeps the newest quotes only: the one past them evicts the
/// oldest, which then names nothing (`invalid_argument`, nothing issued or
/// paid) and no newer quote, while the next oldest still pays.
#[test]
fn a_quote_past_the_kept_ones_evicts_the_oldest() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quotes: Vec<_> = (0..=super::OPS_KEPT)
        .map(|_| w.send_quote(one, &to).unwrap().operation)
        .collect();
    let ops = &w.f.s.platform.send_ops;
    let (oldest, kept) = (&quotes[0], &quotes[1..]);
    assert!(kept.iter().all(|q| q != oldest));
    let e = ops.cap(oldest, w.f.wallet).unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e}");
    let grant = w.f.grant(20 * DASH);
    let e = w.confirm(oldest, &grant).unwrap_err();
    assert_eq!(e.code(), "invalid_argument", "{e}");
    assert!(w.f.unused(&grant));
    assert_eq!(w.net.credits(them), 0);
    w.confirm(&kept[0], &w.f.grant(20 * DASH)).unwrap();
    assert_eq!(w.net.credits(them), DASH);
}

/// A send pays someone else, so only a fresh grant pays it, once:
/// no lease (a move's, or a send flow's a caller holds), and no grant
/// twice.
#[test]
fn a_send_takes_a_fresh_grant_and_no_lease() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    for flow in [FlowKind::PlatformAddressMove, FlowKind::PlatformAddressSend] {
        let lease = w.f.flow(flow, vec![w.f.grant(20 * DASH)]);
        let quote = w.send_quote(one, &to).unwrap();
        assert_eq!(
            w.confirm(&quote.operation, &lease).unwrap_err().code(),
            "platform.grant_invalid",
            "{flow:?}"
        );
    }
    assert!(w.net.sent.lock().unwrap().is_empty());
    let grant = w.f.grant(20 * DASH);
    w.send(one, &to, &grant).unwrap();
    assert!(w.send(one, &to, &grant).is_err(), "a grant pays one send");
    assert_eq!(w.net.credits(them), DASH);
    // A move still takes its own flow's lease.
    let lease =
        w.f.flow(FlowKind::PlatformAddressMove, vec![w.f.grant(20 * DASH)]);
    w.move_to(one, &w.to, &lease).unwrap();
}

/// A `ContactPayFromBalance` lease pays a contact only (C-funding P15): a
/// move to the wallet's Core address and a send refuse it as any other
/// flow's lease, and nothing is signed.
#[test]
fn a_contact_payment_lease_neither_moves_nor_sends() {
    let w = World::new();
    w.net.fund(w.address(0), 5 * DASH);
    let (_, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let lease = || {
        let grants = vec![w.f.grant(20 * DASH)];
        let flow = FlowKind::ContactPayFromBalance;
        let source = Some(crate::platform::PaymentSource::Platform);
        (w.f.engine)
            .block_on(w.f.s.begin_flow(w.f.wallet, flow, grants, source))
            .unwrap()
    };
    let moved = w.move_to(one, &w.to, &lease());
    assert_eq!(moved.unwrap_err().code(), "platform.grant_invalid");
    let quote = w.send_quote(one, &to).unwrap();
    let sent = w.confirm(&quote.operation, &lease());
    assert_eq!(sent.unwrap_err().code(), "platform.grant_invalid");
    assert!(w.net.sent.lock().unwrap().is_empty());
}

/// The own-address refusal covers the derived pool only: an address of
/// this wallet's payment account beyond it is accepted as a recipient.
#[test]
fn an_own_address_beyond_the_pool_is_accepted() {
    use key_wallet::bip32::ChildNumber;
    let w = World::new();
    let (pool, xpub) = (w.call(|_, _, wallet, _| async move {
        use key_wallet::account::account_collection::PlatformPaymentAccountKey;
        let key = PlatformPaymentAccountKey {
            account: 0,
            key_class: 0,
        };
        let wm = wallet.wallet_manager().read().await;
        let core = wm.get_wallet(&wallet.wallet_id()).unwrap();
        let xpub = core.accounts.platform_payment_accounts[&key].account_xpub;
        Ok((pool(&wallet).await?, xpub))
    }))
    .unwrap();
    let at = |index: u32| {
        let child = xpub
            .derive_pub(&[ChildNumber::from_normal_idx(index).unwrap()])
            .unwrap();
        let hash = dpp::dashcore::PublicKey::new(child.public_key).pubkey_hash();
        PlatformAddress::P2pkh(dpp::dashcore::hashes::Hash::to_byte_array(hash))
    };
    // The derivation is the pool's.
    assert_eq!(
        recipient(w.network(), &bech32m(&at(0), w.network()), &pool),
        Err(RecipientProblem::Own)
    );
    let beyond = at(pool.len() as u32 + 5);
    assert!(!pool.contains_key(&beyond));
    assert_eq!(
        recipient(w.network(), &bech32m(&beyond, w.network()), &pool),
        Ok(beyond)
    );
}

/// Below address funds (protocol version 11) the send's quote,
/// the send and the receive address are refused; without Platform the
/// receive address is.
#[test]
fn sends_and_receive_need_address_funds_and_platform() {
    use dpp::version::feature_initial_protocol_versions::ADDRESS_FUNDS_INITIAL_PROTOCOL_VERSION as V;
    let w = World::new();
    w.net.fund(w.address(0), 3 * DASH);
    let (_, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let quote = w.send_quote(one, &to).unwrap();
    *w.net.version.lock().unwrap() = PlatformVersion::get(V - 1).unwrap();
    let off = |r: Result<(), CreditsError>| match r {
        Err(CreditsError::Platform(PlatformError::FeatureOff { feature })) => feature,
        other => panic!("{other:?}"),
    };
    assert_eq!(off(w.send_quote(one, &to).map(drop)), "platform_addresses");
    assert_eq!(
        off(w.confirm(&quote.operation, &w.f.grant(20 * DASH)).map(drop)),
        "platform_addresses"
    );
    assert_eq!(
        off(w
            .call(|net, s, wallet, _| async move {
                Ok(receive_address(&*net, &s, &wallet, s.vault.network()).await?)
            })
            .map(drop)),
        "platform_addresses"
    );
    assert!(w.net.sent.lock().unwrap().is_empty());

    let f = super::super::contacts_tests::Fixture::new(true);
    let dp = f.s.dashpay(f.wallet);
    match f
        .engine
        .block_on(async move { dp.platform_receive_address().await })
    {
        Err(CreditsError::Platform(PlatformError::FeatureOff { feature })) => {
            assert_eq!(feature, "platform")
        }
        other => panic!("{other:?}"),
    }
}

/// A 0.1 move record (no `kind`, `fee_from_output` or `version`)
/// read back holds and settles as a move to Core; a 0.1 move failed by
/// height still lets a new move at its very inputs and nonces go.
#[test]
fn a_0_1_record_reloads_and_settles() {
    let one = WithdrawAmount::Credits { credits: DASH };
    // A move stored as 0.1 stored it, failed by height: it holds.
    let stored = || {
        let w = World::new();
        w.net.fund(w.address(0), 3 * DASH);
        w.net.then(Act::Lost);
        w.move_to(one, &w.to, &w.f.grant(3 * DASH)).unwrap();
        let current = w.moves()[0].clone();
        let mut json = serde_json::to_value(&current).unwrap();
        for field in ["kind", "fee_from_output", "version"] {
            json.as_object_mut().unwrap().remove(field);
        }
        let old: MoveRecord = serde_json::from_value(json).unwrap();
        assert_eq!((old.kind, old.version), (MoveKind::ToCore, None));
        w.store(&[old]);
        w.net.tick();
        w.activity();
        assert_eq!(w.moves()[0].state, PlatformActivityState::Failed);
        assert!(w.moves()[0].holds());
        (w, current.id)
    };
    // Its own transition landing late settles it: Done, a move to Core
    // with its payout.
    let (w, id) = stored();
    w.lands(0);
    let rows = w.activity();
    let row = rows.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        (row.state, row.kind, row.may_land),
        (
            PlatformActivityState::Done,
            PlatformActivityKind::MoveToCore,
            false
        )
    );
    assert!(row.expected_duffs.is_some());
    assert!(!w.moves()[0].holds());
    // A new move at its very inputs and nonces goes, and the 0.1 record is
    // the group's loser.
    let (w, id) = stored();
    let next = w.move_to(one, &w.to, &w.f.grant(3 * DASH)).unwrap();
    assert_eq!(next.state, PlatformActivityState::Done);
    let rows = w.activity();
    let moved = rows.iter().find(|r| r.id == next.id).unwrap();
    assert_eq!(moved.kind, PlatformActivityKind::MoveToCore);
    assert!(moved.expected_duffs.is_some());
    let old = rows.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        (old.state, old.may_land),
        (PlatformActivityState::Failed, false)
    );
}

/// 0.2 R2: an "Add from Dash wallet" is its own `from_core` row, never an
/// `incoming` one. While its apply runs a read does not compare its
/// address; once the apply is proved, the address's first rise up to what
/// the fund added is the fund's, and only a rise beyond it, or a later one,
/// is incoming.
#[test]
fn the_rise_after_an_applied_fund_is_no_incoming_row() {
    let w = World::new();
    let (id, s) = (w.f.wallet, Arc::clone(&w.f.s));
    let to = bech32m(&w.address(0), Network::Testnet);
    w.net.fund(w.address(0), DASH);
    assert_eq!(
        kinds(&w.activity()),
        [(PlatformActivityKind::Incoming, to.clone(), DASH)]
    );
    let txid = "ab".repeat(32);
    let (at, a) = (
        dashcore::OutPoint::new(txid.parse().unwrap(), 0),
        to.clone(),
    );
    let s2 = Arc::clone(&s);
    (w.f.engine)
        .block_on(async move { record_fund(&s2, id, (at, 1_000_000), a, 999, true).await })
        .unwrap();
    // The apply executes on Platform while a read runs: not compared.
    let hold = FundHold::new(&s.platform.fund_holds, id, to.clone());
    w.net.fund(w.address(0), 2 * DASH);
    let during = w.activity();
    assert_eq!(during.len(), 2, "{during:?}");
    let fund = find(&during, PlatformActivityKind::FromCore);
    assert_eq!(
        (fund.state, fund.credits, fund.address.as_deref()),
        (PlatformActivityState::Pending, 999, Some(to.as_str()))
    );
    let a = to.clone();
    (w.f.engine)
        .block_on(async move { fund_applied(&s, id, (at, 1_000_000), a, (2 * DASH, false)).await })
        .unwrap();
    drop(hold);
    let after = w.activity();
    let incoming: Vec<_> = (after.iter())
        .filter(|r| r.kind == PlatformActivityKind::Incoming)
        .collect();
    assert_eq!(incoming.len(), 1, "only the first funding: {after:?}");
    let fund = find(&after, PlatformActivityKind::FromCore);
    assert_eq!(
        (fund.id.as_str(), fund.state, fund.credits),
        (
            format!("fund-{txid}").as_str(),
            PlatformActivityState::Done,
            2 * DASH
        )
    );
    // Read once: the next rise of the address is someone's payment.
    w.net.fund(w.address(0), 3 * DASH);
    let later = w.activity();
    let rows: Vec<_> = (later.iter())
        .filter(|r| r.kind == PlatformActivityKind::Incoming)
        .map(|r| r.credits)
        .collect();
    assert_eq!(rows.len(), 2, "{later:?}");
    assert!(rows.contains(&(3 * DASH)));
}

/// What exceeds an applied fund's credits in the address's first rise is
/// incoming.
#[test]
fn only_a_rise_beyond_an_applied_fund_is_incoming() {
    let before = snap(&[("a", 100)]);
    let now = snap(&[("a", 1_150)]);
    let mut arriving = BTreeMap::from([("a".to_owned(), 1_000)]);
    let (rows, _) = changes(
        Some(&before),
        &now,
        &BTreeSet::new(),
        &mut BTreeMap::new(),
        &mut arriving,
        &mut BTreeMap::new(),
        (5, 5),
    );
    assert_eq!(
        kinds(&rows),
        [(PlatformActivityKind::Incoming, "a".into(), 50)]
    );
    assert_eq!(arriving["a"], 0, "spent");
}

/// #533: a send planned on a read behind a done move would sign the nonce
/// the move took, and the next current read would show it "reached" though
/// nothing was paid. A done move from A, then a send drawing on A and B on
/// a read behind on A, though quoted on a current one: refused before
/// anything is signed or recorded, also when submitted again after a
/// restart. Once the read is current, the same send goes through, signed
/// once.
#[test]
fn a_send_on_a_read_behind_a_done_move_is_refused() {
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.net.fee.store(1, Ordering::SeqCst);
    w.net.fund(a, 5 * DASH);
    let one = WithdrawAmount::Credits { credits: DASH };
    let moved = w.move_to(one, &w.to.clone(), &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(moved.state, PlatformActivityState::Done);
    w.net.fund(b, 5 * DASH);
    let sent = || w.net.sent.lock().unwrap().len();
    let before = sent();
    let (_, to) = w.theirs(9);
    let seven = WithdrawAmount::Credits { credits: 7 * DASH };
    let lag = |behind: bool| {
        let mut state = w.net.state.lock().unwrap();
        let nonce = &mut state.get_mut(&a).unwrap().0;
        *nonce = if behind { *nonce - 1 } else { *nonce + 1 };
    };
    let records = || {
        (w.moves().into_iter())
            .map(|m| (m.id, m.state))
            .collect::<Vec<_>>()
    };
    let recorded = records();
    // Quoted on a current read, then submitted on a lagging node's proof
    // that shows A before the move; twice, with a restart between.
    for _ in 0..2 {
        let quote = w.send_quote(seven, &to).unwrap();
        lag(true);
        let grant = w.f.grant(10 * DASH);
        assert_eq!(
            w.confirm(&quote.operation, &grant).unwrap_err().code(),
            "platform.unavailable"
        );
        assert_eq!(sent(), before, "nothing signed");
        assert!(w.f.unused(&grant));
        assert_eq!(records(), recorded, "nothing recorded");
        lag(false);
        w.f.s.platform.send_ops.reset();
    }
    let paid = w.send(seven, &to, &w.f.grant(10 * DASH)).unwrap();
    assert_eq!(paid.state, PlatformActivityState::Done);
    assert_eq!(sent(), before + 1, "signed once");
    let last = w.net.sent.lock().unwrap().last().unwrap().clone();
    assert!(last.contains_key(&a) && last.contains_key(&b), "{last:?}");
    assert_eq!(last[&a].0, 2, "the nonce after the move's");
}

/// A named retry of a send that landed answers that send's outcome on a read
/// behind it too: it signs nothing, so the stale-read refusal does not apply.
#[test]
fn a_retry_of_a_landed_send_on_a_read_behind_it_pays_nothing() {
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    let (them, to) = w.theirs(9);
    let one = WithdrawAmount::Credits { credits: DASH };
    let first = w.held_send(one, &to);
    let quote = w.send_quote(one, &to).unwrap();
    assert_eq!(quote.retry_of.as_deref(), Some(first.id.as_str()));
    w.lands(0);
    w.activity();
    assert_eq!(w.moves()[0].state, PlatformActivityState::Done);
    w.net.state.lock().unwrap().get_mut(&a).unwrap().0 -= 1;
    let grant = w.f.grant(quote.credits);
    let out = w.confirm(&quote.operation, &grant).unwrap();
    assert_eq!(
        (out.id.as_str(), out.state),
        (first.id.as_str(), PlatformActivityState::Done)
    );
    assert!(w.f.unused(&grant), "nothing redeemed");
    assert_eq!(w.net.sent.lock().unwrap().len(), 1, "nothing signed");
    assert_eq!(w.net.credits(them), DASH, "paid once");
}

#[path = "platform_shield_tests.rs"]
mod shield;

// ---- C-routes C1b-2: credits from Shielded ----

/// An unshield artifact's selected and published nullifiers.
type Spending = (Vec<[u8; 32]>, Vec<[u8; 32]>);

impl World {
    /// The dispatch artifact of unshield `n`, registered as the hook
    /// writes it before the first send: one note and a padding action.
    fn unshield_artifact(&self, n: u8) {
        let note = [n.wrapping_add(100); 32];
        self.unshield_artifact_spending(n, (vec![note], vec![note, [0xee; 32]]));
    }

    /// As [`Self::unshield_artifact`], its selected and published
    /// nullifiers `spending`.
    fn unshield_artifact_spending(&self, n: u8, spending: Spending) {
        let (selected, published) = spending;
        use dw_appdb::dispatch::{ArtifactKind, ArtifactOrigin, NewShieldedArtifact};
        let artifact = NewShieldedArtifact {
            activity: [n; 32],
            origin: ArtifactOrigin::Dispatched,
            st_hash: Some([n.wrapping_add(1); 32]),
            kind: Some(ArtifactKind::Unshield),
            anchor: Some([0xa0; 32]),
            identity_id: None,
            selected,
            published,
        };
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        let wrote = (self.f.engine)
            .block_on(async move { s.leases.record_shielded_artifact(id, artifact).await });
        assert_eq!(wrote, Ok(true));
    }

    /// The hook's accept of unshield `n` crediting `to` with `credits`: its
    /// record, its artifact, then the record armed.
    fn arm_unshield(&self, n: u8, to: PlatformAddress, credits: u64) -> Result<(), PlatformError> {
        self.arm_unshield_spending(n, to, credits, None)
    }

    /// As [`Self::arm_unshield`]; `spending`, when given, is the artifact's
    /// selected and published nullifiers.
    fn arm_unshield_spending(
        &self,
        n: u8,
        to: PlatformAddress,
        credits: u64,
        spending: Option<Spending>,
    ) -> Result<(), PlatformError> {
        self.stage_unshield(n, to, credits, n.wrapping_add(1))?;
        match spending {
            Some(spending) => self.unshield_artifact_spending(n, spending),
            None => self.unshield_artifact(n),
        }
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        let armed = (self.f.engine).block_on(async move {
            arm_from_shielded(&s, id, ([n; 32], [n.wrapping_add(1); 32])).await
        });
        assert_eq!(armed, Ok(true));
        Ok(())
    }

    /// Unshield `n`'s record as the hook writes it before the artifact,
    /// unarmed, its transition hash `[hash; 32]`.
    fn stage_unshield(
        &self,
        n: u8,
        to: PlatformAddress,
        credits: u64,
        hash: u8,
    ) -> Result<(), PlatformError> {
        let record = FromShieldedRecord::staged(
            FromShieldedSource::Unshield,
            ([n; 32], [hash; 32]),
            bech32m(&to, Network::Regtest),
            credits,
        );
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        (self.f.engine).block_on(async move { stage_from_shielded(&s, id, record).await })
    }

    fn shielded_rows(&self) -> Vec<PlatformActivity> {
        (self.activity().into_iter())
            .filter(|r| r.kind == PlatformActivityKind::FromShielded)
            .collect()
    }

    /// Unshield `n`'s artifact as the library's disposition settles it.
    fn settle_unshield(&self, n: u8, status: dw_appdb::dispatch::ArtifactStatus) {
        use dw_appdb::dispatch::ArtifactStatus;
        let disposition = match status {
            ArtifactStatus::Executed => ("executed", None),
            ArtifactStatus::NotExecuted => ("not_executed", Some("pruned")),
            _ => ("unresolved", Some("cancelled")),
        };
        self.settle_unshield_as(n, status, disposition);
    }

    /// Unshield `n`'s artifact settled `status`, the library's disposition
    /// stored as `disposition`.
    fn settle_unshield_as(
        &self,
        n: u8,
        status: dw_appdb::dispatch::ArtifactStatus,
        disposition: (&'static str, Option<&'static str>),
    ) {
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        let settled = (self.f.engine).block_on(async move {
            (s.leases)
                .settle_shielded_artifact(
                    id,
                    [n.wrapping_add(1); 32],
                    status,
                    Some(disposition),
                    false,
                )
                .await
        });
        assert_eq!(settled, Ok(dw_appdb::dispatch::ArtifactSettled::Settled));
    }

    fn commit(&self, writes: Vec<Write>) {
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        (self.f.engine)
            .block_on(async move { Store::of(&s, id).unwrap().commit(writes).await })
            .unwrap();
    }

    fn shielded_row(&self, n: u8) -> PlatformActivity {
        let id = format!("from-shielded-{}", hex::encode([n; 32]));
        let activity = self.activity();
        (activity.into_iter().find(|r| r.id == id)).expect("the FromShielded row")
    }
}

fn incoming_total(activity: &[PlatformActivity]) -> u64 {
    (activity.iter())
        .filter(|r| r.kind == PlatformActivityKind::Incoming)
        .map(|r| r.credits)
        .sum()
}

/// Before the hook's accept a rise of the address is `incoming`. Once the
/// record is armed it absorbs the rise, up to its credits: no `incoming`
/// row for it, the excess `incoming`. An artifact read `not_executed` fails
/// the record and gives back what it absorbed as `incoming`, once.
#[test]
fn from_shielded_absorbs_only_while_armed() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    assert!(w.activity().is_empty());
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), DASH, "unarmed: incoming");

    w.arm_unshield(1, a, 2 * DASH).unwrap();
    let row = w.shielded_row(1);
    assert_eq!(
        (row.kind, row.state, row.credits, row.may_land),
        (
            PlatformActivityKind::FromShielded,
            PlatformActivityState::Pending,
            2 * DASH,
            true
        )
    );
    assert_eq!(row.address, Some(bech32m(&a, Network::Regtest)));
    w.net.fund(a, 2 * DASH + DASH / 2);
    let activity = w.activity();
    assert_eq!(
        incoming_total(&activity),
        DASH + DASH / 2,
        "armed: the rise up to its credits is the record's, the excess incoming"
    );

    // A second record, its rise absorbed, then not executed: given back.
    let b = w.address(1);
    w.arm_unshield(2, b, DASH).unwrap();
    w.net.fund(b, DASH / 4);
    let before = incoming_total(&w.activity());
    w.settle_unshield(2, ArtifactStatus::NotExecuted);
    let activity = w.activity();
    assert_eq!(w.shielded_row(2).state, PlatformActivityState::Failed);
    assert!(!w.shielded_row(2).may_land);
    assert_eq!(
        incoming_total(&activity),
        before + DASH / 4,
        "given back once"
    );
    assert_eq!(incoming_total(&w.activity()), before + DASH / 4);
    // Failed, it absorbs nothing more.
    w.net.fund(b, DASH);
    assert_eq!(incoming_total(&w.activity()), before + DASH / 4 + DASH);
}

/// The record is `done` only on an executed artifact, never on its own
/// absorbing or on time; once done, what it did not absorb is the first
/// rise's, and a later rise is `incoming`. It never holds a move.
#[test]
fn from_shielded_done_only_on_executed_artifact() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let (a, b) = (w.address(0), w.address(1));
    w.arm_unshield(1, a, 2 * DASH).unwrap();
    w.net.fund(a, 2 * DASH);
    for _ in 0..3 {
        assert_eq!(w.shielded_row(1).state, PlatformActivityState::Pending);
    }
    assert_eq!(incoming_total(&w.activity()), 0);
    w.settle_unshield(1, ArtifactStatus::Unresolved { unrecorded: false });
    assert_eq!(w.shielded_row(1).state, PlatformActivityState::Pending);
    w.settle_unshield(1, ArtifactStatus::Executed);
    assert_eq!(w.shielded_row(1).state, PlatformActivityState::Done);
    w.net.fund(a, 3 * DASH);
    assert_eq!(
        incoming_total(&w.activity()),
        3 * DASH,
        "after done: incoming"
    );

    // Executed, its credit landed before the read that settles it: that
    // read's rise is its own.
    w.arm_unshield(2, b, DASH).unwrap();
    w.settle_unshield(2, ArtifactStatus::Executed);
    w.net.fund(b, DASH);
    assert_eq!(incoming_total(&w.activity()), 3 * DASH);
    assert_eq!(w.shielded_row(2).state, PlatformActivityState::Done);
    w.net.fund(b, 2 * DASH);
    assert_eq!(incoming_total(&w.activity()), 5 * DASH);

    // A pending record holds no move.
    w.arm_unshield(3, a, DASH).unwrap();
    let out = w.move_all(&w.f.grant(10 * DASH)).unwrap();
    assert_ne!(out.state, PlatformActivityState::Failed);
}

/// Armed once per spend; a write that fails is an error (the hook refuses
/// the send); the record survives a restart, pending until its artifact
/// settles it.
#[test]
fn from_shielded_is_durable_and_armed_once() {
    use dw_appdb::dispatch::ArtifactStatus;
    let mut w = World::new();
    let a = w.address(0);
    w.fail_next_outcome_write();
    assert!(w.arm_unshield(1, a, DASH).is_err());
    let id = format!("from-shielded-{}", hex::encode([1u8; 32]));
    assert!(w.activity().iter().all(|r| r.id != id), "nothing written");

    w.arm_unshield(2, a, DASH).unwrap();
    w.stage_unshield(2, a, 5 * DASH, 9).unwrap();
    assert_eq!(w.shielded_row(2).credits, DASH, "an armed record stands");

    w.reopen();
    let row = w.shielded_row(2);
    assert_eq!(
        (row.state, row.credits),
        (PlatformActivityState::Pending, DASH)
    );
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), 0);
    w.settle_unshield(2, ArtifactStatus::Executed);
    assert_eq!(w.shielded_row(2).state, PlatformActivityState::Done);
}

/// Written before its artifact, a record is no row and absorbs nothing
/// until armed; an unarmed one is replaced by the record of the
/// transition the library armed again; a spend with no record (dispatched
/// before records were kept) arms nothing.
#[test]
fn from_shielded_unarmed_is_no_row() {
    let w = World::new();
    let a = w.address(0);
    w.stage_unshield(1, a, DASH, 7).unwrap();
    w.stage_unshield(1, a, 2 * DASH, 2).unwrap();
    w.net.fund(a, DASH);
    assert!(w.shielded_rows().is_empty(), "unarmed: no row");
    assert_eq!(
        incoming_total(&w.activity()),
        DASH,
        "unarmed: absorbs nothing"
    );
    w.unshield_artifact(1);
    // The first transition's hash: not this record's.
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let stale =
        (w.f.engine).block_on(async move { arm_from_shielded(&s, id, ([1; 32], [7; 32])).await });
    assert!(stale.is_err(), "{stale:?}");
    assert!(w.shielded_rows().is_empty(), "still unarmed");
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let armed =
        (w.f.engine).block_on(async move { arm_from_shielded(&s, id, ([1; 32], [2; 32])).await });
    assert_eq!(armed, Ok(true));
    let row = w.shielded_row(1);
    assert_eq!(
        (row.state, row.credits),
        (PlatformActivityState::Pending, 2 * DASH),
        "the second transition's"
    );
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let none =
        (w.f.engine).block_on(async move { arm_from_shielded(&s, id, ([5; 32], [6; 32])).await });
    assert_eq!(none, Ok(false));

    // An unarmed record's spend never left: its artifact settling does not
    // make it done, and it takes no later rise.
    let b = w.address(1);
    w.stage_unshield(3, b, DASH, 4).unwrap();
    w.unshield_artifact(3);
    w.settle_unshield(3, dw_appdb::dispatch::ArtifactStatus::Executed);
    let before = incoming_total(&w.activity());
    w.net.fund(b, DASH);
    assert_eq!(incoming_total(&w.activity()), before + DASH);
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let records = (w.f.engine).block_on(async move { from_shielded_records(&s, id).await });
    let unarmed = records
        .iter()
        .find(|r| r.activity == hex::encode([3u8; 32]))
        .unwrap();
    assert_eq!(
        (unarmed.state, unarmed.armed),
        (PlatformActivityState::Pending, false)
    );
}

/// Two records of one address, both executed before either rise is read:
/// the first read above their height shows both credits, neither
/// `incoming`, and the next rise is `incoming`.
#[test]
fn from_shielded_two_executed_take_one_rise_each() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.arm_unshield(1, a, DASH).unwrap();
    w.arm_unshield(2, a, DASH).unwrap();
    w.settle_unshield(1, ArtifactStatus::Executed);
    w.settle_unshield(2, ArtifactStatus::Executed);
    w.net.fund(a, 2 * DASH);
    assert_eq!(incoming_total(&w.activity()), 0);
    assert_eq!(w.shielded_row(2).state, PlatformActivityState::Done);
    w.net.fund(a, DASH);
    assert_eq!(
        incoming_total(&w.activity()),
        DASH,
        "the next rise is incoming"
    );
}

/// Two pending records of one address, the older absorbing the one rise:
/// when the older fails and the newer executes, the rise is the newer's,
/// in either order of settling, and no `incoming` row shows it.
#[test]
fn from_shielded_failed_records_rise_goes_to_the_other() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    for (address, newer_first) in [(0, false), (1, true)] {
        let a = w.address(address);
        let (old, new) = (10 + 2 * address as u8, 11 + 2 * address as u8);
        w.arm_unshield(old, a, DASH).unwrap();
        w.arm_unshield(new, a, DASH).unwrap();
        let before = incoming_total(&w.activity());
        w.net.fund(a, DASH);
        assert_eq!(incoming_total(&w.activity()), before);
        if newer_first {
            w.settle_unshield(new, ArtifactStatus::Executed);
            assert_eq!(w.shielded_row(new).state, PlatformActivityState::Done);
            w.settle_unshield(old, ArtifactStatus::NotExecuted);
        } else {
            w.settle_unshield(old, ArtifactStatus::NotExecuted);
            w.settle_unshield(new, ArtifactStatus::Executed);
        }
        let activity = w.activity();
        assert_eq!(w.shielded_row(old).state, PlatformActivityState::Failed);
        assert_eq!(w.shielded_row(new).state, PlatformActivityState::Done);
        assert_eq!(
            incoming_total(&activity),
            before,
            "no incoming for the one rise ({newer_first})"
        );
        w.net.fund(a, DASH);
        assert_eq!(
            incoming_total(&w.activity()),
            before + DASH,
            "the next rise is incoming ({newer_first})"
        );
    }
}

/// Rule 2: only the library's own proof of the spend makes the record
/// done. An execution it could not tie to this spend (a confirmed history
/// row beside an unattributable outcome) leaves it pending, its rise
/// absorbed, not `incoming`.
#[test]
fn from_shielded_unattributable_execution_stays_pending() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.arm_unshield(1, a, DASH).unwrap();
    let unattributable = ("unresolved", Some("unattributable"));
    w.settle_unshield_as(1, ArtifactStatus::Executed, unattributable);
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), 0, "its rise, absorbed");
    for _ in 0..2 {
        let row = w.shielded_row(1);
        assert_eq!(
            (row.state, row.may_land),
            (PlatformActivityState::Pending, true)
        );
    }
    w.arm_unshield(2, a, DASH).unwrap();
    w.settle_unshield(2, ArtifactStatus::Executed);
    assert_eq!(
        w.shielded_row(2).state,
        PlatformActivityState::Done,
        "proved"
    );
}

/// Q80 C2 (a): an execution the library could not attribute proves no
/// payout either: the evidence of its transition is unknown.
#[test]
fn payout_evidence_of_an_unattributable_execution_is_unknown() {
    use crate::platform::payment_locks::PayoutEvidence;
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    w.arm_unshield(1, w.address(0), DASH).unwrap();
    w.settle_unshield_as(
        1,
        ArtifactStatus::Executed,
        ("unresolved", Some("unattributable")),
    );
    let evidence = (w.f.engine).block_on(
        w.f.s
            .shielded_payout_evidence(w.f.wallet, "payout", [2; 32]),
    );
    assert_eq!(evidence, PayoutEvidence::Unknown);
}

/// Q80 C2 (b): a refused first send the library still holds
/// (`Unresolved(Cancelled)`, not settled by the release) proves nothing:
/// its payout evidence is unknown and its record stays pending.
#[test]
fn a_held_refused_spend_proves_no_payout_and_no_move() {
    use crate::platform::payment_locks::PayoutEvidence;
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.arm_unshield(1, a, DASH).unwrap();
    w.settle_unshield_as(
        1,
        ArtifactStatus::Unresolved { unrecorded: false },
        ("unresolved", Some("cancelled")),
    );
    w.net.fund(a, DASH);
    let evidence = (w.f.engine).block_on(
        w.f.s
            .shielded_payout_evidence(w.f.wallet, "payout", [2; 32]),
    );
    assert_eq!(evidence, PayoutEvidence::Unknown);
    for _ in 0..2 {
        assert_eq!(w.shielded_row(1).state, PlatformActivityState::Pending);
    }
}

/// A refused first send the release settled `executed` (every nullifier it
/// published proved spent, the library's `Unresolved(Cancelled)` kept,
/// one of them padding no other spend can publish) is proved: the record
/// is done and its payout sent.
#[test]
fn from_shielded_executed_by_the_release_is_done() {
    use crate::platform::payment_locks::PayoutEvidence;
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.arm_unshield(1, a, DASH).unwrap();
    w.settle_unshield_as(
        1,
        ArtifactStatus::Executed,
        ("unresolved", Some("cancelled")),
    );
    assert_eq!(w.shielded_row(1).state, PlatformActivityState::Done);
    let evidence = (w.f.engine).block_on(
        w.f.s
            .shielded_payout_evidence(w.f.wallet, "payout", [2; 32]),
    );
    assert_eq!(evidence, PayoutEvidence::Sent);
}

/// A spend of two notes and no padding nullifier: every published one spent may be
/// another device's spend of the same notes: the release's `executed`
/// proves nothing of this transition. The record stays pending and its
/// payout evidence unknown, as before the release settled it.
#[test]
fn a_release_executed_spend_without_padding_proves_nothing() {
    use crate::platform::payment_locks::PayoutEvidence;
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    let notes = vec![[101; 32], [102; 32]];
    (w.arm_unshield_spending(1, a, DASH, Some((notes.clone(), notes)))).unwrap();
    w.settle_unshield_as(
        1,
        ArtifactStatus::Executed,
        ("unresolved", Some("cancelled")),
    );
    w.net.fund(a, DASH);
    for _ in 0..2 {
        assert_eq!(w.shielded_row(1).state, PlatformActivityState::Pending);
    }
    let evidence = (w.f.engine).block_on(
        w.f.s
            .shielded_payout_evidence(w.f.wallet, "payout", [2; 32]),
    );
    assert_eq!(evidence, PayoutEvidence::Unknown);
}

/// A done record's credit is what the first read above the height it was
/// settled at shows of it. What that read did not show as its own (the
/// address fell in the same read: a debit beside the credit) is no later
/// rise's: a later unrelated payment is `incoming`.
#[test]
fn from_shielded_done_allowance_ends_at_its_first_read() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.net.fund(a, 5 * DASH);
    assert_eq!(incoming_total(&w.activity()), 5 * DASH);
    w.arm_unshield(1, a, 2 * DASH).unwrap();
    w.settle_unshield(1, ArtifactStatus::Executed);
    // 5 out and its 2 in before the read that settles it: a fall to 2.
    w.net.state.lock().unwrap().get_mut(&a).unwrap().1 = 2 * DASH;
    w.net.tick();
    assert_eq!(incoming_total(&w.activity()), 5 * DASH);
    assert_eq!(w.shielded_row(1).state, PlatformActivityState::Done);
    w.net.fund(a, 2 * DASH);
    assert_eq!(
        incoming_total(&w.activity()),
        7 * DASH,
        "a later payment: incoming"
    );
}

/// A read that does not compare the record's address (a fund apply runs
/// there) does not end its allowance: the first read that compares it
/// takes the credit, no `incoming` row.
#[test]
fn from_shielded_done_allowance_waits_for_a_held_address() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    let to = bech32m(&a, Network::Regtest);
    w.arm_unshield(1, a, DASH).unwrap();
    let hold = FundHold::new(&w.f.s.platform.fund_holds, w.f.wallet, to);
    w.settle_unshield(1, ArtifactStatus::Executed);
    w.net.tick();
    assert_eq!(w.shielded_row(1).state, PlatformActivityState::Done);
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), 0, "held: not compared");
    drop(hold);
    assert_eq!(incoming_total(&w.activity()), 0, "its credit, once");
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), DASH);
}

/// A rise a failed record had read as its own goes only to a record of
/// its address armed before that read: a later unshield's record never
/// claims it, so it is `incoming` again, and the later record still takes
/// its own credit.
#[test]
fn a_later_unshield_never_claims_a_rise_read_before_it_was_armed() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.arm_unshield(1, a, DASH).unwrap();
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), 0, "the first record's");
    w.arm_unshield(2, a, DASH).unwrap();
    w.settle_unshield(1, ArtifactStatus::NotExecuted);
    assert_eq!(
        incoming_total(&w.activity()),
        DASH,
        "read before the second was armed: incoming"
    );
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), DASH, "the second's own");
    w.settle_unshield(2, ArtifactStatus::Executed);
    assert_eq!(w.shielded_row(2).state, PlatformActivityState::Done);
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), 2 * DASH);
}

/// A restored fund's armed record read part of a done unshield's credit
/// as its own. Platform reports its lock used: the record goes, and what
/// it absorbed is the unshield's again, not `incoming`, so the credit
/// shows once; a later payment to the address is `incoming`.
#[test]
fn a_removed_restored_funds_give_back_goes_to_the_unshield_it_hid() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    let to = bech32m(&a, Network::Regtest);
    w.arm_unshield(1, a, DASH).unwrap();
    w.settle_unshield(1, ArtifactStatus::Executed);
    let txid = "cd".repeat(32);
    let at = dashcore::OutPoint::new(txid.parse().unwrap(), 0);
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let arming = (w.f.engine)
        .block_on(async move { arm_fund(&s, id, (at, 1_000_000), to, DASH / 100, false).await });
    assert_eq!(arming, Ok(Arming::Made));
    w.net.fund(a, DASH);
    let rows = w.activity();
    assert_eq!(incoming_total(&rows), 0, "{rows:?}");
    assert_eq!(w.shielded_row(1).state, PlatformActivityState::Done);
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    (w.f.engine)
        .block_on(async move { fund_used(&s, id, &txid, DASH / 100).await })
        .unwrap();
    let rows = w.activity();
    assert!(
        rows.iter()
            .all(|r| r.kind != PlatformActivityKind::FromCore),
        "{rows:?}"
    );
    assert_eq!(incoming_total(&rows), 0, "shown once: {rows:?}");
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let again = "cd".repeat(32);
    (w.f.engine)
        .block_on(async move { fund_used(&s, id, &again, DASH / 100).await })
        .unwrap();
    assert_eq!(incoming_total(&w.activity()), 0, "given back once");
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), DASH, "a later payment");
}

/// A settle whose write fails records nothing: the next read settles the
/// record again and gives back what it absorbed once, never twice.
#[test]
fn a_failed_from_shielded_settle_write_gives_back_once() {
    use dw_appdb::dispatch::ArtifactStatus;
    let w = World::new();
    let a = w.address(0);
    w.arm_unshield(1, a, DASH).unwrap();
    w.net.fund(a, DASH / 2);
    assert_eq!(incoming_total(&w.activity()), 0);
    w.settle_unshield(1, ArtifactStatus::NotExecuted);
    w.fail_next_outcome_write();
    let failed = w.call(|net, s, wl, id| async move { activity(&*net, &s, &wl, id).await });
    assert!(failed.is_err(), "{failed:?}");
    for _ in 0..2 {
        assert_eq!(incoming_total(&w.activity()), DASH / 2);
        assert_eq!(w.shielded_row(1).state, PlatformActivityState::Failed);
    }
}

/// A settled record (failed or done) is not armed again: `false`, as for
/// no record.
#[test]
fn a_settled_from_shielded_record_is_not_armed() {
    let w = World::new();
    let a = w.address(0);
    for (n, state) in [
        (1u8, PlatformActivityState::Failed),
        (3, PlatformActivityState::Done),
    ] {
        w.arm_unshield(n, a, DASH).unwrap();
        let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
        let armed = (w.f.engine).block_on(async move {
            set_record_state(&s, id, [n; 32], state).await;
            arm_from_shielded(&s, id, ([n; 32], [n.wrapping_add(1); 32])).await
        });
        assert_eq!(armed, Ok(false), "{state:?}");
    }
}

/// A record that cannot be read is an error, never "no record", so the
/// hook refuses a re-send rather than let it go unattributed. A record
/// whose activity is not the id its key names is left out: no row, and it
/// absorbs nothing.
#[test]
fn an_unreadable_or_malformed_from_shielded_record_claims_nothing() {
    let w = World::new();
    let a = w.address(0);
    let key = |n: u8| format!("{FROM_SHIELDED_PREFIX}{}", hex::encode([n; 32]));
    w.commit(vec![(key(1), Some("{not a record".into()))]);
    let (s, id) = (Arc::clone(&w.f.s), w.f.wallet);
    let armed =
        (w.f.engine).block_on(async move { arm_from_shielded(&s, id, ([1; 32], [2; 32])).await });
    assert!(armed.is_err(), "{armed:?}");

    let record = FromShieldedRecord::staged(
        FromShieldedSource::Unshield,
        ([2; 32], [3; 32]),
        bech32m(&a, Network::Regtest),
        DASH,
    );
    let mut value = serde_json::to_value(&record).unwrap();
    value["armed"] = true.into();
    value["activity"] = "zz".into();
    w.commit(vec![(key(2), Some(value.to_string()))]);
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), DASH);
    assert!(w.shielded_rows().is_empty());
}

#[path = "platform_top_up_tests.rs"]
mod top_up;

#[path = "platform_register_tests.rs"]
mod register;

#[path = "platform_mainnet_flag_tests.rs"]
mod mainnet_flag;

#[path = "platform_held_exit_tests.rs"]
mod held_exit;

// ---- C-funding C4b-2: a Shielded-funded registration's refund ----

impl World {
    fn height(&self) -> u64 {
        self.net.height.load(Ordering::SeqCst)
    }

    fn activity_height(&self) -> u64 {
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        (self.f.engine)
            .block_on(async move { activity_height(&s, id).await })
            .unwrap()
    }

    /// The refund record of create `n` to `to`, at most `cap`, the create
    /// named at activity height `named_at` and proved executed by
    /// `executed_by`.
    fn arm_refund(&self, n: u8, to: PlatformAddress, cap: u64, heights: (u64, u64)) {
        let (s, id) = (Arc::clone(&self.f.s), self.f.wallet);
        let to = bech32m(&to, Network::Regtest);
        (self.f.engine)
            .block_on(async move {
                arm_registration_refund(
                    &s,
                    id,
                    ([n; 32], [n.wrapping_add(1); 32]),
                    (7, to, cap),
                    heights,
                )
                .await
            })
            .unwrap();
    }

    fn refund_rows(&self) -> Vec<PlatformActivity> {
        (self.activity().into_iter())
            .filter(|r| r.kind == PlatformActivityKind::RegistrationRefund)
            .collect()
    }
}

/// F13: the refund of a create Platform ran as its fallback, armed after
/// the proof and before any read showed it, is one `registrationRefund`
/// row of exactly what the first read showed, no `incoming` row; once read,
/// the next rise of the address is `incoming`, and nothing is listed
/// before the read. Arming again changes nothing.
#[test]
fn a_registration_refund_is_one_row_of_what_the_read_shows() {
    let w = World::new();
    let a = w.address(0);
    assert!(w.activity().is_empty());
    let named_at = w.activity_height();
    w.net.fund(a, DASH);
    let executed_by = w.height();
    w.arm_refund(9, a, 2 * DASH, (named_at, executed_by));
    w.arm_refund(9, a, 2 * DASH, (named_at, executed_by));
    let activity = w.activity();
    assert_eq!(incoming_total(&activity), 0);
    let rows: Vec<_> = (activity.iter())
        .filter(|r| r.kind == PlatformActivityKind::RegistrationRefund)
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].state, rows[0].credits, rows[0].may_land),
        (PlatformActivityState::Done, DASH, false)
    );
    assert_eq!(rows[0].address, Some(bech32m(&a, Network::Regtest)));
    w.net.fund(a, DASH);
    assert_eq!(
        incoming_total(&w.activity()),
        DASH,
        "the next rise is incoming"
    );
    assert_eq!(w.refund_rows()[0].credits, DASH);
}

/// F13: a read between the create's execution and the record's arming
/// showed the refund as `incoming`: the record claims nothing and lists no
/// row, and a later rise stays `incoming`. Mutant: absorb whatever the
/// baseline.
#[test]
fn a_read_between_execution_and_arming_absorbs_nothing() {
    let w = World::new();
    let a = w.address(0);
    assert!(w.activity().is_empty());
    let named_at = w.activity_height();
    w.net.fund(a, DASH);
    let executed_by = w.height();
    assert_eq!(incoming_total(&w.activity()), DASH);
    w.arm_refund(9, a, 2 * DASH, (named_at, executed_by));
    assert!(w.refund_rows().is_empty());
    w.net.fund(a, DASH);
    assert_eq!(incoming_total(&w.activity()), 2 * DASH);
    assert!(w.refund_rows().is_empty());
}

/// F13: an unshield's pending record and a refund record on one address
/// share one rise in arming order, the unshield's first, no credit counted
/// twice and none `incoming`.
#[test]
fn two_armed_records_absorb_in_arming_order() {
    let w = World::new();
    let a = w.address(0);
    assert!(w.activity().is_empty());
    let named_at = w.activity_height();
    w.arm_unshield(1, a, DASH).unwrap();
    w.net.fund(a, 2 * DASH);
    w.arm_refund(9, a, 2 * DASH, (named_at, w.height()));
    let activity = w.activity();
    assert_eq!(incoming_total(&activity), 0);
    let unshield = w.shielded_row(1);
    let refund = w.refund_rows();
    assert_eq!(unshield.credits, DASH, "the unshield's first");
    assert_eq!((refund.len(), refund[0].credits), (1, DASH));
}

#[test]
fn fund_record_and_from_shielded_record_pending_dispatch_labels() {
    let at = dashcore::OutPoint::new(dashcore::Txid::from_byte_array([0x42; 32]), 0);
    let mut broadcast_fund = FundRecord::pending((at, 1_000_000), "tdash1qtest".into(), 999);
    broadcast_fund.lock_sent = true;
    assert_eq!(
        broadcast_fund.activity().pending_dispatch,
        Some(PendingDispatch::Broadcast)
    );

    let queued_fund = FundRecord::pending((at, 1_000_000), "tdash1qtest".into(), 999);
    assert_eq!(
        queued_fund.activity().pending_dispatch,
        Some(PendingDispatch::Queued)
    );

    let mut recovered_fund = broadcast_fund.clone();
    recovered_fund.recovered = true;
    assert_eq!(recovered_fund.activity().pending_dispatch, None);

    let mut done_fund = broadcast_fund;
    done_fund.state = PlatformActivityState::Done;
    assert_eq!(done_fund.activity().pending_dispatch, None);

    let unshield = FromShieldedRecord::staged(
        FromShieldedSource::Unshield,
        ([1u8; 32], [2u8; 32]),
        "tdash1qtest".into(),
        DASH,
    );
    assert_eq!(
        unshield.activity().pending_dispatch,
        Some(PendingDispatch::Queued)
    );

    let mut unshield_done = unshield;
    unshield_done.state = PlatformActivityState::Done;
    assert_eq!(unshield_done.activity().pending_dispatch, None);

    let registration = FromShieldedRecord::staged(
        FromShieldedSource::Registration { draft: 7 },
        ([3u8; 32], [4u8; 32]),
        "tdash1qtest".into(),
        DASH,
    );
    assert_eq!(registration.activity().pending_dispatch, None);
}
