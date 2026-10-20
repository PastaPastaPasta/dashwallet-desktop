//! The wallet's own proposals (MG-06 §5): the state table, the fee paid
//! under a "Proposal" lease and saved before its broadcast, and the
//! submission pace against Core's postponed-object rules (D6, D19, D20).

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::hashes::Hash;
use dashcore::{BlockHash, Transaction, Txid};
use dw_appdb::{AppDb, ProposalRow};
use dw_governance::object::{GovernanceObject, display_hex};
use dw_governance::sync::Phase;
use dw_vault::{Credential, GrantPurpose};
use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
use platform_wallet::PlatformWalletError;

use super::super::reads::now;
use super::super::{
    Governance, GovernanceError, MyProposalState, ProposalDraft, ProposalField, SubmitRefusal,
    SubmitWait,
};
use super::{ECHO_WAIT, FETCH_WAIT, Facts, PROPAGATION_WAIT, derive_state};
use crate::EngineError;
use crate::history::TxEntry;
use crate::platform::lease::session_tests::{Fixture, fixture};
use crate::send::SendFailure;

const COIN: u64 = 100_000_000;
const TIP: u32 = 2_000;
/// The first regtest superblock after [`TIP`] (start 1500, cycle 20).
const SUPERBLOCK: u32 = 2_020;
const PAYEE: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

fn peer(n: u8) -> SocketAddr {
    SocketAddr::from(([10, 0, 0, n], 19899))
}

#[derive(Clone, Copy)]
enum Mode {
    Accept,
    Reject,
    Unconfirmed,
    /// Waits past the permit's deadline before the library.
    Late,
}

/// The library's broadcast of the fee, recording whether the proposal was
/// saved when it was entered.
struct Lib {
    appdb: Arc<AppDb>,
    wallet: String,
    mode: Mode,
    entered: AtomicU32,
    saved_at_entry: Mutex<Vec<bool>>,
    sent: Mutex<Vec<Transaction>>,
}

impl crate::send::TestBroadcaster for Lib {
    fn before_entry(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let wait = match self.mode {
            Mode::Late => {
                crate::platform::lease::LeaseConfig::default().permit_ttl + Duration::from_secs(1)
            }
            _ => Duration::ZERO,
        };
        Box::pin(tokio::time::sleep(wait))
    }

    fn broadcast(
        &self,
        tx: &Transaction,
        _first: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Txid, PlatformWalletError>> + Send>> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let txid = tx.txid();
        let saved = (self.appdb.proposals(Some(&self.wallet)).unwrap())
            .iter()
            .any(|r| r.fee_txid == txid.to_byte_array() && r.fee_sent_at.is_none());
        self.saved_at_entry.lock().unwrap().push(saved);
        self.sent.lock().unwrap().push(tx.clone());
        let mode = self.mode;
        Box::pin(async move {
            match mode {
                Mode::Accept | Mode::Late => Ok(txid),
                Mode::Reject => Err(PlatformWalletError::TransactionBroadcast(
                    "bad-txns-inputs-missingorspent".into(),
                )),
                Mode::Unconfirmed => Err(PlatformWalletError::TransactionBroadcastUnconfirmed(
                    "no verdict yet".into(),
                )),
            }
        })
    }
}

struct Props {
    f: Fixture,
    gov: Governance,
    lib: Arc<Lib>,
}

/// A synced regtest session at [`TIP`] whose wallet holds 3 DASH, with the
/// fee's broadcast faked as `mode`.
fn props(mode: Mode) -> Props {
    let f = fixture(false);
    f.credit(1, 3 * COIN);
    f.session.hub.tracker().set_tip_height(TIP);
    let rt = &f.session.governance;
    rt.set_enabled_flag(true);
    {
        let mut p = rt.shared.progress();
        p.phase = Phase::Synced;
        p.last_synced_at = Some(1);
    }
    let lib = Arc::new(Lib {
        appdb: f.session.live().unwrap().appdb,
        wallet: f.wallet.to_string(),
        mode,
        entered: AtomicU32::new(0),
        saved_at_entry: Mutex::default(),
        sent: Mutex::default(),
    });
    *rt.fee_broadcaster.lock().unwrap() = Some(lib.clone());
    let gov = f.session.governance();
    Props { f, gov, lib }
}

fn draft() -> ProposalDraft {
    ProposalDraft {
        name: "dw-proposal".into(),
        url: "https://example.org/p".into(),
        payment_address: PAYEE.into(),
        payment_amount: 5 * COIN,
        payment_count: 2,
        first_superblock_height: SUPERBLOCK,
        coins: Vec::new(),
    }
}

impl Props {
    fn spend(&self, max_duffs: u64) -> String {
        self.f
            .grant(GrantPurpose::Spend { max_duffs }, Credential::None)
    }

    fn create(&self, grant: String) -> Result<super::MyProposal, GovernanceError> {
        self.f
            .engine
            .block_on(self.gov.create_proposal(self.f.wallet, draft(), grant))
    }

    /// Creates the draft's proposal and returns its hash.
    fn created(&self) -> [u8; 32] {
        let p = self.create(self.spend(2 * COIN)).unwrap();
        dw_governance::object::parse_display_hex(&p.hash).unwrap()
    }

    fn rows(&self) -> Vec<ProposalRow> {
        self.lib.appdb.proposals(None).unwrap()
    }

    fn row(&self, hash: &[u8; 32]) -> ProposalRow {
        self.rows().into_iter().find(|r| &r.hash == hash).unwrap()
    }

    fn fee(&self, hash: &[u8; 32]) -> Transaction {
        dashcore::consensus::deserialize(&self.row(hash).fee_tx).unwrap()
    }

    /// Puts the fee `hash` paid in the wallet's history at `depth`
    /// confirmations (0: in the mempool).
    fn confirm(&self, hash: &[u8; 32], depth: u32) {
        self.confirm_tx(self.fee(hash), depth);
    }

    fn confirm_tx(&self, tx: Transaction, depth: u32) {
        let context = match depth {
            0 => TransactionContext::Mempool,
            d => {
                TransactionContext::InBlock(BlockInfo::new(TIP - d + 1, BlockHash::all_zeros(), 1))
            }
        };
        let txid = tx.txid();
        self.f.session.hub.history.with_wallet(self.f.wallet, |h| {
            h.txs.insert(
                txid,
                TxEntry {
                    tx,
                    context,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs: Default::default(),
                    abandoned: false,
                },
            )
        });
    }

    fn tick(&self, at: Instant) {
        self.f.engine.block_on(self.gov.tick_at(at));
    }

    /// Ticks at `at`, when the fee's confirmation is first seen, and again
    /// once it may be announced (D21); returns that second instant.
    fn announce(&self, at: Instant) -> Instant {
        self.tick(at);
        let t = at + PROPAGATION_WAIT;
        self.tick(t);
        t
    }

    /// Starts a new synced run.
    fn new_run(&self, n: u64) {
        let mut p = self.f.session.governance.shared.progress();
        p.last_synced_at = Some(n);
    }

    fn state(&self, hash: &[u8; 32]) -> MyProposalState {
        let mine = self.gov.my_proposals(self.f.wallet).unwrap();
        (mine.into_iter())
            .find(|p| p.hash == display_hex(hash))
            .unwrap()
            .state
    }

    fn queued(&self, hash: &[u8; 32]) -> bool {
        self.f.session.governance.shared.is_queued(hash)
    }

    fn fetch(&self, hash: &[u8; 32], by: u8) {
        (self.f.session.governance.shared).note_object_fetch(hash, peer(by));
    }

    fn resyncs(&self) -> usize {
        self.f.session.governance.resyncs.load(Ordering::SeqCst)
    }

    /// Two peers announce the proposal back: the store holds it
    /// confirmed (MG-02 D3).
    fn echo(&self, hash: &[u8; 32]) {
        let row = self.row(hash);
        let mut object = GovernanceObject::new_proposal(row.revision, row.time, row.data);
        object.collateral_hash = row.fee_txid;
        let mut store = self.f.session.governance.shared.store();
        store.set_confirmations(2);
        store
            .add_object_announced(object, Some(peer(1)), &[peer(2)], now())
            .unwrap();
    }

    fn submit(&self, hash: &[u8; 32]) -> Result<super::MyProposal, GovernanceError> {
        self.f
            .engine
            .block_on(self.gov.submit_proposal(self.f.wallet, display_hex(hash)))
    }
}

fn facts() -> Facts {
    Facts {
        depth: Some(1),
        sent: true,
        conflicted: false,
        submitted: false,
        public: None,
        expired: false,
        wait: None,
    }
}

/// MG-06 §2.3, in its precedence order.
#[test]
fn the_state_is_derived_in_section_2_3_order() {
    use MyProposalState as S;
    let cases = [
        (
            Facts {
                public: Some(None),
                expired: true,
                conflicted: true,
                ..facts()
            },
            S::Public { status: None },
        ),
        (
            Facts {
                expired: true,
                conflicted: true,
                ..facts()
            },
            S::Expired,
        ),
        (
            Facts {
                conflicted: true,
                depth: None,
                sent: false,
                ..facts()
            },
            S::FeeFailed,
        ),
        (
            Facts {
                depth: None,
                sent: false,
                ..facts()
            },
            S::FeeUnsent,
        ),
        (
            Facts {
                depth: None,
                ..facts()
            },
            S::FeePaid { confirmations: 0 },
        ),
        (
            Facts {
                depth: Some(0),
                submitted: true,
                wait: Some(SubmitWait::NoPeerAccepted),
                ..facts()
            },
            S::FeePaid { confirmations: 0 },
        ),
        (
            Facts {
                depth: Some(3),
                submitted: true,
                wait: Some(SubmitWait::NotSynced),
                ..facts()
            },
            S::Confirming { confirmations: 3 },
        ),
        (
            Facts {
                wait: Some(SubmitWait::NoPeerAccepted),
                ..facts()
            },
            S::ReadyToSubmit {
                reason: SubmitWait::NoPeerAccepted,
            },
        ),
        (facts(), S::FeePaid { confirmations: 1 }),
    ];
    for (f, want) in cases {
        assert_eq!(derive_state(f), want, "{f:?}");
    }
}

/// D4/D18: the proposal and its signed fee are saved before the fee is
/// handed to the network; the fee is Core's `GenBudgetSystemCollateralTx`
/// shape, P2PKH change only (D1).
#[test]
fn the_proposal_is_saved_before_its_fee_is_broadcast() {
    let p = props(Mode::Accept);
    let mine = p.create(p.spend(2 * COIN)).unwrap();
    assert_eq!(*p.lib.saved_at_entry.lock().unwrap(), [true]);
    assert_eq!(mine.state, MyProposalState::FeePaid { confirmations: 0 });
    assert_eq!(mine.name, "dw-proposal");
    assert_eq!(mine.payment_count, 2);

    let hash = dw_governance::object::parse_display_hex(&mine.hash).unwrap();
    let row = p.row(&hash);
    assert!(row.fee_sent_at.is_some() && row.submitted_at.is_none());
    let fee = p.fee(&hash);
    assert_eq!(p.lib.sent.lock().unwrap()[0], fee);
    assert_eq!(mine.fee_txid, fee.txid().to_string());
    let data: Vec<_> = fee
        .output
        .iter()
        .filter(|o| o.script_pubkey.is_op_return())
        .collect();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0].value, COIN);
    assert_eq!(
        data[0].script_pubkey,
        dashcore::ScriptBuf::new_op_return(&hash)
    );
    assert!(
        (fee.output.iter())
            .filter(|o| !o.script_pubkey.is_op_return())
            .all(|o| o.script_pubkey.is_p2pkh())
    );
    // The saved object is the hash the OP_RETURN commits to (Core's
    // `IsCollateralValid`).
    let mut object = GovernanceObject::new_proposal(row.revision, row.time, row.data.clone());
    object.collateral_hash = row.fee_txid;
    assert_eq!(object.hash(), hash);
    assert!(
        !p.queued(&hash),
        "a fee with no confirmation is not announced"
    );
}

/// D4: a proposal the app database refuses pays nothing.
#[test]
fn a_proposal_that_cannot_be_saved_broadcasts_nothing() {
    let p = props(Mode::Accept);
    p.lib.appdb.delete_wallet(&p.f.wallet.to_string()).unwrap();
    assert!(p.create(p.spend(2 * COIN)).is_err());
    assert_eq!(p.lib.entered.load(Ordering::SeqCst), 0);
    assert!(p.rows().is_empty());
    assert_eq!(p.f.reserved(), 0, "the fee's coins are released");
}

/// D4: a fee the network refused leaves no proposal behind.
#[test]
fn a_refused_fee_deletes_its_proposal() {
    let p = props(Mode::Reject);
    let e = p.create(p.spend(2 * COIN)).unwrap_err();
    assert!(
        matches!(
            e.engine(),
            Some(EngineError::Send(SendFailure::BroadcastRejected { .. }))
        ),
        "{e:?}"
    );
    assert_eq!(p.lib.entered.load(Ordering::SeqCst), 1);
    assert!(p.rows().is_empty());
}

/// D4: a hand-off cut before the library (a lock, the permit's deadline)
/// is definitely unsent: the proposal goes, the coins are released.
#[test]
fn a_hand_off_cut_before_the_library_deletes_its_proposal() {
    let p = props(Mode::Late);
    let e = p.create(p.spend(2 * COIN)).unwrap_err();
    assert!(
        matches!(e.engine(), Some(EngineError::Send(SendFailure::Cancelled))),
        "{e:?}"
    );
    assert_eq!(p.lib.entered.load(Ordering::SeqCst), 0);
    assert!(p.rows().is_empty());
    assert_eq!(p.f.reserved(), 0);
}

/// D10: a fee that may be out keeps its proposal, `fee_unsent` with the
/// failure until the wallet sees the fee.
#[test]
fn a_fee_that_may_be_out_keeps_its_proposal_unsent() {
    let p = props(Mode::Unconfirmed);
    let mine = p.create(p.spend(2 * COIN)).unwrap();
    assert_eq!(mine.state, MyProposalState::FeeUnsent);
    let hash = dw_governance::object::parse_display_hex(&mine.hash).unwrap();
    let row = p.row(&hash);
    assert!(row.fee_sent_at.is_none() && row.last_error.is_some());
    p.confirm(&hash, 1);
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 1 }
    );
}

/// D2: the fee and its network fee are charged to a Spend grant; a cap
/// below them, or another grant kind, pays nothing.
#[test]
fn the_fee_needs_a_spend_grant_covering_it() {
    let p = props(Mode::Accept);
    let e = p.create(p.spend(COIN)).unwrap_err();
    assert!(
        matches!(
            e.engine(),
            Some(EngineError::Send(SendFailure::GrantExceeded { .. }))
        ),
        "{e:?}"
    );
    let governance = p.f.grant(GrantPurpose::Governance, Credential::None);
    assert!(
        p.create(governance)
            .unwrap_err()
            .code()
            .starts_with("platform.")
    );
    assert_eq!(p.lib.entered.load(Ordering::SeqCst), 0);
    assert!(p.rows().is_empty());
    assert_eq!(p.f.reserved(), 0);
}

/// D11: every failing field, before any lease or fee.
#[test]
fn an_invalid_draft_names_every_failing_field() {
    let p = props(Mode::Accept);
    let bad = ProposalDraft {
        name: String::new(),
        payment_count: 0,
        first_superblock_height: SUPERBLOCK + 1,
        ..draft()
    };
    let e =
        p.f.engine
            .block_on(p.gov.create_proposal(p.f.wallet, bad, p.spend(2 * COIN)))
            .unwrap_err();
    let GovernanceError::InvalidProposal { fields } = e else {
        panic!("{e:?}")
    };
    assert_eq!(
        fields,
        [
            ProposalField::Name,
            ProposalField::PaymentCount,
            ProposalField::FirstPayment
        ]
    );
    assert_eq!(p.lib.entered.load(Ordering::SeqCst), 0);
}

/// D6/D21: announced only once a caught-up wallet has seen the fee's
/// first confirmation for the propagation wait, and withdrawn when a reorg
/// takes it back.
#[test]
fn the_proposal_is_announced_only_while_its_fee_is_mined() {
    let p = props(Mode::Accept);
    let hash = p.created();
    let t0 = Instant::now();
    p.confirm(&hash, 0);
    p.tick(t0);
    p.tick(t0 + PROPAGATION_WAIT);
    assert!(!p.queued(&hash));
    p.confirm(&hash, 1);
    p.tick(t0);
    p.tick(t0 + PROPAGATION_WAIT - Duration::from_secs(1));
    assert!(!p.queued(&hash), "peers may not have the block yet");
    p.tick(t0 + PROPAGATION_WAIT);
    assert!(p.queued(&hash));
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 1 }
    );
    p.confirm(&hash, 0);
    let t1 = t0 + PROPAGATION_WAIT;
    p.tick(t1);
    assert!(!p.queued(&hash));
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 0 }
    );

    // Not caught up: the wallet's view of the chain may lead its peers'.
    p.f.session.hub.tracker().set_running(false);
    p.confirm(&hash, 1);
    p.tick(t1);
    p.tick(t1 + PROPAGATION_WAIT);
    assert!(!p.queued(&hash));
    {
        let mut t = p.f.session.hub.tracker();
        t.set_running(true);
        t.set_tip_height(TIP);
    }
    p.tick(t1 + PROPAGATION_WAIT);
    assert!(!p.queued(&hash), "the wait starts once caught up");
    p.tick(t1 + PROPAGATION_WAIT * 2);
    assert!(p.queued(&hash));
}

/// D6/D7: a peer's fetch is "submitted"; no fetch within a minute is
/// `no_peer_accepted`, retried by a new run with a doubling delay.
#[test]
fn an_unfetched_proposal_is_retried_by_new_runs() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    let t0 = p.announce(Instant::now());
    p.tick(t0 + FETCH_WAIT - Duration::from_secs(1));
    assert_eq!(p.resyncs(), 0);
    p.tick(t0 + FETCH_WAIT);
    assert_eq!(
        p.state(&hash),
        MyProposalState::ReadyToSubmit {
            reason: SubmitWait::NoPeerAccepted
        }
    );
    assert_eq!(p.resyncs(), 1);
    // The next retry is a minute on, the one after that two.
    p.tick(t0 + FETCH_WAIT + Duration::from_secs(59));
    assert_eq!(p.resyncs(), 1);
    p.tick(t0 + FETCH_WAIT + Duration::from_secs(60));
    assert_eq!(p.resyncs(), 2);
    p.tick(t0 + FETCH_WAIT + Duration::from_secs(179));
    assert_eq!(p.resyncs(), 2);
    p.tick(t0 + FETCH_WAIT + Duration::from_secs(180));
    assert_eq!(p.resyncs(), 3);

    p.fetch(&hash, 1);
    p.tick(t0 + Duration::from_secs(600));
    assert_eq!(
        p.state(&hash),
        MyProposalState::Confirming { confirmations: 1 }
    );
    assert!(p.row(&hash).submitted_at.is_some());
}

/// D19: a peer holds the proposal postponed until the fee's sixth
/// confirmation, so a fetched proposal waits there without retries; past
/// six, no echo within two minutes looks again with a new run (D20).
#[test]
fn a_fetched_proposal_waits_for_six_confirmations_without_retrying() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    let t0 = p.announce(Instant::now());
    p.fetch(&hash, 1);
    for (depth, at) in [(1, 0), (3, 3_600), (5, 7_200)] {
        p.confirm(&hash, depth);
        p.tick(t0 + Duration::from_secs(at));
        assert_eq!(
            p.state(&hash),
            MyProposalState::Confirming {
                confirmations: depth
            }
        );
    }
    assert_eq!(p.resyncs(), 0, "no retry before six confirmations");

    p.confirm(&hash, 6);
    let t6 = t0 + Duration::from_secs(8_000);
    p.tick(t6);
    p.tick(t6 + ECHO_WAIT - Duration::from_secs(1));
    assert_eq!(p.resyncs(), 0);
    p.tick(t6 + ECHO_WAIT);
    assert_eq!(p.resyncs(), 1);
    assert_eq!(
        p.state(&hash),
        MyProposalState::Confirming { confirmations: 6 }
    );

    p.echo(&hash);
    assert!(matches!(p.state(&hash), MyProposalState::Public { .. }));
    p.tick(t6 + ECHO_WAIT * 10);
    assert!(!p.queued(&hash), "a public proposal is no longer announced");
    assert_eq!(p.resyncs(), 1);
    assert_eq!(
        p.submit(&hash).unwrap_err().code(),
        "governance.submit_refused"
    );
}

/// D20: with no echo six confirmations on, the looks for it back off from
/// 30 minutes, doubling, to a day apart, and the state stays `confirming`
/// with no error until the proposal ends.
#[test]
fn echo_retries_back_off_to_a_day() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 6);
    let t = p.announce(Instant::now());
    p.fetch(&hash, 1);
    p.tick(t);
    p.tick(t + ECHO_WAIT);
    assert_eq!(p.resyncs(), 1);
    let mut at = t + ECHO_WAIT;
    // Literal, not the constants: the test pins the policy (D20).
    let day = Duration::from_secs(24 * 60 * 60);
    let mut delay = Duration::from_secs(30 * 60);
    for n in 2..=10 {
        p.tick(at + delay - Duration::from_secs(1));
        assert_eq!(p.resyncs(), n - 1, "not before {delay:?}");
        at += delay;
        p.tick(at);
        assert_eq!(p.resyncs(), n, "after {delay:?}");
        delay = (delay * 2).min(day);
    }
    assert_eq!(delay, day, "capped at a day");
    let mine = &p.gov.my_proposals(p.f.wallet).unwrap()[0];
    assert_eq!(mine.state, MyProposalState::Confirming { confirmations: 6 });
}

/// D6: a reorg that takes the fee's confirmation back makes the peers
/// drop their postponed copies: confirmed again, a fetch is expected again.
#[test]
fn a_reorged_fee_expects_a_new_fetch() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 2);
    let t = p.announce(Instant::now());
    p.fetch(&hash, 1);
    p.tick(t);
    assert_eq!(
        p.state(&hash),
        MyProposalState::Confirming { confirmations: 2 }
    );
    p.confirm(&hash, 0);
    p.tick(t);
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 0 }
    );
    p.confirm(&hash, 1);
    let t0 = p.announce(t);
    assert!(p.queued(&hash), "queued again, a new round");
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 1 }
    );
    p.tick(t0 + FETCH_WAIT);
    assert_eq!(p.resyncs(), 1);
    p.fetch(&hash, 2);
    p.tick(t0 + FETCH_WAIT);
    assert_eq!(
        p.state(&hash),
        MyProposalState::Confirming { confirmations: 1 }
    );
}

/// D7: [Submit] on `no_peer_accepted` looks again at once.
#[test]
fn submit_retries_a_proposal_no_peer_took_at_once() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    let t = p.announce(Instant::now() - FETCH_WAIT - PROPAGATION_WAIT);
    p.tick(t + FETCH_WAIT);
    assert_eq!(p.resyncs(), 1);
    let mine = p.submit(&hash).unwrap();
    assert_eq!(p.resyncs(), 2);
    assert_eq!(mine.state, MyProposalState::FeePaid { confirmations: 1 });
}

/// D5: sync off reads `ready_to_submit{sync_disabled}`; [Submit] says so.
#[test]
fn sync_off_waits_and_submit_says_why() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    p.f.session.governance.set_enabled_flag(false);
    assert_eq!(
        p.state(&hash),
        MyProposalState::ReadyToSubmit {
            reason: SubmitWait::SyncDisabled
        }
    );
    assert_eq!(
        p.submit(&hash).unwrap_err().code(),
        "governance.sync_disabled"
    );
}

/// D5: another confirmed transaction spending the fee's coins fails it.
#[test]
fn a_double_spent_fee_fails_and_cannot_be_submitted() {
    let p = props(Mode::Accept);
    let hash = p.created();
    let fee = p.fee(&hash);
    let mut rival = fee.clone();
    rival.output.truncate(1);
    rival.output[0].script_pubkey = dashcore::ScriptBuf::new();
    p.confirm_tx(rival, 1);
    assert_eq!(p.state(&hash), MyProposalState::FeeFailed);
    let e = p.submit(&hash).unwrap_err();
    assert!(matches!(
        e,
        GovernanceError::SubmitRefused {
            reason: SubmitRefusal::FeeFailed
        }
    ));
    p.tick(Instant::now());
    assert!(!p.queued(&hash));
}

/// M1: the waits belong to a synced run. A new run's peers are new
/// connections, so its fetch wait starts again, and a run that is not
/// synced counts nothing.
#[test]
fn the_waits_restart_with_each_synced_run() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    let t0 = p.announce(Instant::now());
    p.tick(t0 + FETCH_WAIT - Duration::from_secs(1));
    p.new_run(2);
    p.tick(t0 + FETCH_WAIT);
    assert_eq!(p.resyncs(), 0, "the new run's wait just began");
    let t1 = t0 + FETCH_WAIT;
    p.tick(t1 + FETCH_WAIT - Duration::from_secs(1));
    assert_eq!(p.resyncs(), 0);
    // Not synced: no wait counts, and syncing again starts it over.
    p.f.session.governance.shared.progress().phase = Phase::SyncingObjects;
    p.tick(t1 + FETCH_WAIT);
    assert_eq!(p.resyncs(), 0);
    p.f.session.governance.shared.progress().phase = Phase::Synced;
    let t2 = t1 + FETCH_WAIT * 2;
    p.tick(t2);
    p.tick(t2 + FETCH_WAIT - Duration::from_secs(1));
    assert_eq!(p.resyncs(), 0);
    p.tick(t2 + FETCH_WAIT);
    assert_eq!(p.resyncs(), 1);
}

/// M3: a fee missing from history (a stale read) withdraws the proposal
/// but does not mark it lost; only a fee back in the mempool does.
#[test]
fn a_fee_missing_from_history_is_not_a_reorg() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 2);
    let t = p.announce(Instant::now());
    p.fetch(&hash, 1);
    p.tick(t);
    let txid = p.fee(&hash).txid();
    p.f.session
        .hub
        .history
        .with_wallet(p.f.wallet, |h| h.txs.remove(&txid));
    p.tick(t);
    assert!(!p.queued(&hash));
    p.confirm(&hash, 2);
    let t = p.announce(t);
    assert_eq!(
        p.state(&hash),
        MyProposalState::Confirming { confirmations: 2 }
    );
    p.tick(t + FETCH_WAIT);
    assert_eq!(p.resyncs(), 0, "no new fetch is expected");
}

/// A fee missing from history for a while keeps its last depth: back in
/// the mempool after that, it still lost its block and expects a new fetch.
#[test]
fn a_reorg_seen_after_a_missing_fee_still_expects_a_new_fetch() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 2);
    let t = p.announce(Instant::now());
    p.fetch(&hash, 1);
    p.tick(t);
    let txid = p.fee(&hash).txid();
    p.f.session
        .hub
        .history
        .with_wallet(p.f.wallet, |h| h.txs.remove(&txid));
    p.tick(t);
    p.confirm(&hash, 0);
    p.tick(t);
    p.confirm(&hash, 1);
    let t = p.announce(t);
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 1 }
    );
    p.tick(t + FETCH_WAIT);
    assert_eq!(p.resyncs(), 1);
}

/// A proposal whose row is gone (its wallet removed) is announced no more.
#[test]
fn a_proposal_whose_row_is_gone_is_withdrawn() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    let t = p.announce(Instant::now());
    assert!(p.queued(&hash));
    p.lib
        .appdb
        .delete_proposal(&p.f.wallet.to_string(), &hash)
        .unwrap();
    p.tick(t);
    assert!(!p.queued(&hash));
}

/// L3: a fee seen in the mempool is resent from its saved bytes, as one
/// never seen is; sync being off does not stop that.
#[test]
fn submit_resends_an_unmined_fee() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 0);
    p.f.session.governance.set_enabled_flag(false);
    // The fixture has no SPV client: reaching its broadcast is the resend.
    let e = p.submit(&hash).unwrap_err();
    assert!(
        matches!(e.engine(), Some(EngineError::Wallet(m)) if m.contains("broadcast")),
        "{e:?}"
    );
}

/// D5: past its end the proposal is expired, never announced.
#[test]
fn an_expired_proposal_is_settled() {
    let p = props(Mode::Accept);
    let fee = Transaction {
        version: 3,
        lock_time: 0,
        input: Vec::new(),
        output: Vec::new(),
        special_transaction_payload: None,
    };
    let data = format!(
        r#"{{"end_epoch":{},"name":"old","payment_address":"{PAYEE}","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
        now() - 1
    );
    let mut object = GovernanceObject::new_proposal(1, 1, data.into_bytes());
    object.collateral_hash = fee.txid().to_byte_array();
    let hash = object.hash();
    p.lib
        .appdb
        .insert_proposal(&ProposalRow {
            wallet_id: p.f.wallet.to_string(),
            hash,
            revision: 1,
            time: 1,
            data: object.data.clone(),
            fee_txid: object.collateral_hash,
            fee_tx: dashcore::consensus::serialize(&fee),
            created_at: 1,
            fee_sent_at: Some(1),
            submitted_at: None,
            last_error: None,
        })
        .unwrap();
    p.confirm_tx(fee, 10);
    assert_eq!(p.state(&hash), MyProposalState::Expired);
    p.tick(Instant::now());
    assert!(!p.queued(&hash));
    assert!(matches!(
        p.submit(&hash).unwrap_err(),
        GovernanceError::SubmitRefused {
            reason: SubmitRefusal::Expired
        }
    ));
}

/// MG-00 D18: the row travels in the wallet's backup rows.
#[test]
fn the_proposal_is_in_the_wallets_backup_rows() {
    let p = props(Mode::Accept);
    let hash = p.created();
    let tables = p
        .lib
        .appdb
        .export_wallet_rows(&p.f.wallet.to_string())
        .unwrap();
    let t = tables.iter().find(|t| t.table == "mg_proposals").unwrap();
    assert_eq!(t.rows.len(), 1);
    assert!(
        t.rows[0]
            .iter()
            .any(|v| *v == dw_appdb::SqlValue::Blob(hash.to_vec()))
    );
}
