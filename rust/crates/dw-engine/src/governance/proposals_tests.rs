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
use dashcore::{BlockHash, OutPoint, Transaction, Txid};
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
    mode: Mutex<Mode>,
    entered: AtomicU32,
    saved_at_entry: Mutex<Vec<bool>>,
    sent: Mutex<Vec<Transaction>>,
}

impl crate::send::TestBroadcaster for Lib {
    fn before_entry(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let wait = match *self.mode.lock().unwrap() {
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
        let mode = *self.mode.lock().unwrap();
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
    rt.manual_ticks.store(true, Ordering::SeqCst);
    rt.set_enabled_flag(true);
    {
        let mut p = rt.shared.progress();
        p.phase = Phase::Synced;
        p.last_synced_at = Some(1);
    }
    let lib = Arc::new(Lib {
        appdb: f.session.live().unwrap().appdb,
        wallet: f.wallet.to_string(),
        mode: Mutex::new(mode),
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

    /// Creates the draft's proposal with its fee unconfirmed and drops the
    /// fee's claims, as a restart does; returns its hash and fee.
    fn restarted(&self) -> ([u8; 32], Transaction) {
        let hash = self.created();
        let fee = self.fee(&hash);
        let owner = crate::send::SpendOwner::Tx(fee.txid());
        self.f.session.spends.release(&self.f.wallet, owner);
        (hash, fee)
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

    /// `peers` announce the proposal back, the store needing two.
    fn echo_from(&self, hash: &[u8; 32], peers: &[u8]) {
        let row = self.row(hash);
        let mut object = GovernanceObject::new_proposal(row.revision, row.time, row.data);
        object.collateral_hash = row.fee_txid;
        let mut store = self.f.session.governance.shared.store();
        store.set_confirmations(2);
        let others: Vec<SocketAddr> = peers[1..].iter().map(|n| peer(*n)).collect();
        store
            .add_object_announced(object, Some(peer(peers[0])), &others, now())
            .unwrap();
    }

    fn create_with(
        &self,
        draft: ProposalDraft,
        grant: String,
    ) -> Result<super::MyProposal, GovernanceError> {
        self.f
            .engine
            .block_on(self.gov.create_proposal(self.f.wallet, draft, grant))
    }

    /// Saves a proposal whose end is one second past, with a fee mined 10
    /// blocks deep; returns its hash.
    fn expired(&self) -> [u8; 32] {
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
        self.lib
            .appdb
            .insert_proposal(&ProposalRow {
                wallet_id: self.f.wallet.to_string(),
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
        self.confirm_tx(fee, 10);
        hash
    }

    fn fee_scans(&self) -> usize {
        self.f.session.governance.fee_scans.load(Ordering::SeqCst)
    }

    fn submit(&self, hash: &[u8; 32]) -> Result<super::MyProposal, GovernanceError> {
        self.f
            .engine
            .block_on(self.gov.submit_proposal(self.f.wallet, display_hex(hash)))
    }
}

/// Asserts `e` refuses the resend naming `coin` (`send.outpoint_unavailable`).
fn assert_unavailable(e: &GovernanceError, coin: OutPoint) {
    assert!(
        matches!(e.engine(), Some(EngineError::Send(SendFailure::OutpointUnavailable(o))) if *o == coin),
        "{e:?}"
    );
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
    // A masternode transaction's grant does not pay a proposal either.
    let masternode = p.f.grant(
        GrantPurpose::MasternodeOp {
            max_duffs: 2 * COIN,
        },
        Credential::None,
    );
    let e = p.create(masternode).unwrap_err();
    assert!(e.code().starts_with("platform."), "{e:?}");
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
    p.lib.appdb.delete_wallet(&p.f.wallet.to_string()).unwrap();
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
    let fee = p.fee(&hash);
    // The wallet has the fee, so it reads the fee's coins spent: by the fee.
    for input in &fee.input {
        p.f.spend_coin(input.previous_output);
    }
    let mine = p.submit(&hash).unwrap();
    assert_eq!(mine.state, MyProposalState::FeePaid { confirmations: 0 });
    assert_eq!(*p.lib.sent.lock().unwrap(), vec![fee.clone(), fee]);
}

/// Review r1 m-1: a resend records the hand-off before the bytes go out,
/// keeps its failure as the row's `last_error` (shown, m-3), and clears it
/// once a resend goes out.
#[test]
fn a_resend_records_the_hand_off_and_its_failure() {
    let p = props(Mode::Unconfirmed);
    let mine = p.create(p.spend(2 * COIN)).unwrap();
    assert_eq!(mine.state, MyProposalState::FeeUnsent);
    assert!(mine.last_error.is_some());
    let hash = dw_governance::object::parse_display_hex(&mine.hash).unwrap();
    let e = p.submit(&hash).unwrap_err();
    assert!(
        matches!(
            e.engine(),
            Some(EngineError::Send(SendFailure::BroadcastUnknown { .. }))
        ),
        "{e:?}"
    );
    let row = p.row(&hash);
    assert!(row.fee_sent_at.is_some(), "recorded before the hand-off");
    assert_eq!(row.last_error, Some(e.engine().unwrap().to_string()));
    // Not paid as far as anyone knows (validator r1 Minor-2).
    assert_eq!(p.state(&hash), MyProposalState::FeeUnsent);

    *p.lib.mode.lock().unwrap() = Mode::Accept;
    let mine = p.submit(&hash).unwrap();
    assert_eq!(mine.last_error, None);
    assert_eq!(mine.state, MyProposalState::FeePaid { confirmations: 0 });
    assert_eq!(p.row(&hash).fee_sent_at, row.fee_sent_at, "set once");
    // Unsent at the create's entry; marked sent at each resend's.
    assert_eq!(*p.lib.saved_at_entry.lock().unwrap(), [true, false, false]);
}

/// The library's broadcast of a fee: the first is held until released, its
/// entry signalled, and every one answers a definite rejection (review r1
/// M-1).
struct HeldLib {
    entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    release: Arc<std::sync::atomic::AtomicBool>,
}

impl crate::send::TestBroadcaster for HeldLib {
    fn before_entry(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn broadcast(
        &self,
        _tx: &Transaction,
        _first: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Txid, PlatformWalletError>> + Send>> {
        let first = self.entered.lock().unwrap().take();
        let held = first.is_some();
        if let Some(tx) = first {
            let _ = tx.send(());
        }
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            while held && !release.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(PlatformWalletError::TransactionBroadcast(
                "txn-already-in-mempool".into(),
            ))
        })
    }
}

/// Review r1 M-1 (the reviewer's race test, adapted): a [Submit] while the
/// create still hands the fee off is refused `in_flight`, sends nothing, and
/// the create's definite `NotSent` then takes the row with nothing out. On
/// 1af1515d the [Submit] resent the fee and the row was deleted anyway.
#[test]
fn a_submit_during_the_create_hand_off_is_refused_in_flight() {
    let p = props(Mode::Accept);
    let (tx, rx) = std::sync::mpsc::channel();
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let held = Arc::new(HeldLib {
        entered: Mutex::new(Some(tx)),
        release: Arc::clone(&release),
    });
    *p.f.session.governance.fee_broadcaster.lock().unwrap() = Some(held);
    let gov = p.f.session.governance();
    let (wallet, grant) = (p.f.wallet, p.spend(2 * COIN));
    let create =
        (p.f.session.rt).spawn(async move { gov.create_proposal(wallet, draft(), grant).await });
    rx.recv_timeout(Duration::from_secs(120))
        .expect("the hand-off is entered");
    let row = p.rows().pop().expect("saved before the hand-off");
    assert_eq!(p.state(&row.hash), MyProposalState::FeeUnsent);
    assert!(matches!(
        p.submit(&row.hash).unwrap_err(),
        GovernanceError::SubmitRefused {
            reason: SubmitRefusal::InFlight
        }
    ));
    assert!(p.row(&row.hash).fee_sent_at.is_none(), "nothing resent");
    release.store(true, Ordering::SeqCst);
    let created = p.f.engine.block_on(create).unwrap();
    assert!(created.is_err(), "{created:?}");
    assert!(p.rows().is_empty());
    // The mark goes with the call: a [Submit] now finds no proposal.
    assert_eq!(
        p.submit(&row.hash).unwrap_err().code(),
        "governance.proposal_not_found"
    );
}

/// Review r1 M-2: after a restart the fee's claims are gone. A payment
/// prepared since on one of its coins holds it: the resend is refused
/// naming the coin, takes no claim, and the payment still goes out. With
/// the coin free again, the resend claims the fee's coins.
#[test]
fn a_resend_refuses_a_coin_another_payment_holds() {
    let p = props(Mode::Unconfirmed);
    let (hash, fee) = p.restarted();
    let (wallet, spends) = (p.f.wallet, &p.f.session.spends);
    let owner = crate::send::SpendOwner::Tx(fee.txid());
    let coin = fee.input[0].previous_output;
    let other = p.f.draft_to(PAYEE.into(), COIN / 2);
    other
        .set_source(crate::send::CoinSource::Outpoints(vec![coin]))
        .unwrap();
    *other.broadcaster.lock().unwrap() = Some(p.lib.clone());
    let prepared = p.f.engine.block_on(other.prepare(p.spend(COIN))).unwrap();
    assert_unavailable(&p.submit(&hash).unwrap_err(), coin);
    assert!(spends.held_by(&wallet, owner).is_empty());
    assert!(p.row(&hash).fee_sent_at.is_none(), "nothing handed off");
    *p.lib.mode.lock().unwrap() = Mode::Accept;
    p.f.engine.block_on(other.broadcast(prepared)).unwrap();
    assert_eq!(
        p.lib.sent.lock().unwrap().len(),
        2,
        "the fee once, the payment"
    );

    // The payment seen, its claim goes with the coin: the resend would
    // double-spend it (validator r1 Minor-1).
    spends.release(
        &wallet,
        crate::send::SpendOwner::Tx(p.lib.sent.lock().unwrap()[1].txid()),
    );
    p.f.spend_coin(coin);
    assert_unavailable(&p.submit(&hash).unwrap_err(), coin);
    assert_eq!(p.lib.sent.lock().unwrap().len(), 2, "no resend");
}

/// D10: after a restart a resend claims the fee's coins again, so no
/// payment prepared since can take them.
#[test]
fn a_resend_after_a_restart_claims_the_fees_coins() {
    let p = props(Mode::Accept);
    let (hash, fee) = p.restarted();
    let (wallet, owner) = (p.f.wallet, crate::send::SpendOwner::Tx(fee.txid()));
    assert!(p.f.session.spends.held_by(&wallet, owner).is_empty());
    p.submit(&hash).unwrap();
    let coins: std::collections::HashSet<_> = fee.input.iter().map(|i| i.previous_output).collect();
    assert_eq!(p.f.session.spends.held_by(&wallet, owner), coins);
}

/// Validator r1 Major-1: an asset lock's hold outlives a restart, so after
/// one a resend refuses a coin a tracked `Built` or `Broadcast` lock spends.
#[test]
fn a_resend_refuses_a_coin_an_asset_lock_holds() {
    let p = props(Mode::Unconfirmed);
    let (hash, fee) = p.restarted();
    let (wallet, spends) = (p.f.wallet, &p.f.session.spends);
    let owner = crate::send::SpendOwner::Tx(fee.txid());
    let coin = fee.input[0].previous_output;
    for status in [
        platform_wallet::AssetLockStatus::Built,
        platform_wallet::AssetLockStatus::Broadcast,
    ] {
        p.f.track_lock(9, 0, &[coin], status);
        assert_unavailable(&p.submit(&hash).unwrap_err(), coin);
        assert!(spends.held_by(&wallet, owner).is_empty());
        assert!(p.row(&hash).fee_sent_at.is_none(), "nothing handed off");
    }
    assert_eq!(p.lib.sent.lock().unwrap().len(), 1, "the create's only");
}

/// Validator r1 Major-1: the inputs of a lock a funding call signed, whose
/// step is unresolved and its row gone, are held durably too: a resend
/// refuses them.
#[test]
fn a_resend_refuses_a_coin_an_unresolved_lock_signed() {
    let p = props(Mode::Unconfirmed);
    let (hash, fee) = p.restarted();
    let coin = fee.input[0].previous_output;
    p.f.stuck_signing("registration/1/funding", 70, &[coin]);
    p.f.refresh();
    assert_unavailable(&p.submit(&hash).unwrap_err(), coin);
    assert!(p.row(&hash).fee_sent_at.is_none(), "nothing handed off");
    assert_eq!(p.lib.sent.lock().unwrap().len(), 1, "the create's only");
}

/// D5: past its end the proposal is expired, never announced.
#[test]
fn an_expired_proposal_is_settled() {
    let p = props(Mode::Accept);
    let hash = p.expired();
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

/// Core's `IsExpired`: the end itself is past (review r1 nit).
#[test]
fn a_proposal_expires_at_its_end() {
    assert!(super::expired(Some(100), 100));
    assert!(super::expired(Some(100), 101));
    assert!(!super::expired(Some(100), 99));
    assert!(!super::expired(None, i64::MAX));
}

/// Review r1 m-1: a run that is not synced counts no wait. Ticks a fetch
/// wait apart start no run, and neither do ticks an echo wait apart once
/// it is fetched at six confirmations.
#[test]
fn no_wait_counts_while_not_synced() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 1);
    let t = p.announce(Instant::now());
    p.f.session.governance.shared.progress().phase = Phase::SyncingObjects;
    p.tick(t);
    p.tick(t + FETCH_WAIT);
    p.tick(t + FETCH_WAIT * 3);
    assert_eq!(p.resyncs(), 0);
    p.f.session.governance.shared.progress().phase = Phase::Synced;
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 1 },
        "no peer refused it yet"
    );
    p.f.session.governance.shared.progress().phase = Phase::SyncingObjects;
    p.fetch(&hash, 1);
    p.confirm(&hash, 6);
    let t = t + FETCH_WAIT * 3;
    p.tick(t);
    p.tick(t + ECHO_WAIT);
    p.tick(t + ECHO_WAIT * 3);
    assert_eq!(p.resyncs(), 0);
}

/// D5 (review r1 m-1): a rival spend still in the mempool does not fail the
/// fee; only a confirmed one does.
#[test]
fn a_rival_in_the_mempool_does_not_fail_the_fee() {
    let p = props(Mode::Accept);
    let hash = p.created();
    let mut rival = p.fee(&hash);
    rival.output.truncate(1);
    rival.output[0].script_pubkey = dashcore::ScriptBuf::new();
    p.confirm_tx(rival, 0);
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 0 }
    );
}

/// MG-02 D3 (review r1 m-1): one announcing peer does not make the
/// proposal public; a second does.
#[test]
fn one_announcing_peer_does_not_make_it_public() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.confirm(&hash, 6);
    p.echo_from(&hash, &[1]);
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 6 }
    );
    p.echo_from(&hash, &[2, 3]);
    assert!(matches!(p.state(&hash), MyProposalState::Public { .. }));
}

/// mg-engine §2.5 (review r1 m-1): the draft's coins, as `<txid>-<n>`, are
/// the fee's only inputs.
#[test]
fn the_fee_spends_the_chosen_coins() {
    let p = props(Mode::Accept);
    let chosen = p.f.credit(2, 2 * COIN);
    let d = ProposalDraft {
        coins: vec![format!("{}-{}", chosen.txid, chosen.vout)],
        ..draft()
    };
    let mine = p.create_with(d, p.spend(2 * COIN)).unwrap();
    let hash = dw_governance::object::parse_display_hex(&mine.hash).unwrap();
    let inputs: Vec<_> = (p.fee(&hash).input.iter())
        .map(|i| i.previous_output)
        .collect();
    assert_eq!(inputs, vec![chosen]);
}

/// D18 (plan item 8): a `.dwbackup` round trip carries the row, and the
/// restored wallet, its history found again, reports the same proposal.
#[test]
fn a_backup_round_trip_restores_the_proposal_as_it_was() {
    let p = props(Mode::Unconfirmed);
    let hash = p.created();
    p.confirm(&hash, 2);
    let before = p.gov.my_proposals(p.f.wallet).unwrap();
    assert!(before[0].last_error.is_some());
    let (s, wallet) = (&p.f.session, p.f.wallet);
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("proposal.dwbackup");
    let pass = || Some(zeroize::Zeroizing::new(b"bk".to_vec()));
    (p.f.engine)
        .block_on(s.backup_wallet(wallet, backup.clone(), pass()))
        .unwrap();
    let wipe = p.f.grant(GrantPurpose::Wipe, Credential::None);
    p.f.engine.block_on(s.remove_wallet(wallet, wipe)).unwrap();
    // Only the backup brings it back: nothing kept past the removal.
    p.lib
        .appdb
        .keep_proposals(&wallet.to_string(), &[])
        .unwrap();
    assert_eq!(
        p.f.engine
            .block_on(s.restore_backup(backup, pass()))
            .unwrap(),
        vec![wallet]
    );
    p.confirm(&hash, 2);
    assert_eq!(p.gov.my_proposals(wallet).unwrap(), before);
}

/// D23 (review r1 nit): removing the wallet keeps a paid proposal that has
/// not expired (and logs it, as a removal keeps a possibly sent asset
/// lock's records), drops an expired one, and importing the wallet again
/// brings the kept one back.
#[test]
fn a_removal_keeps_a_paid_proposal_for_the_wallets_return() {
    let p = props(Mode::Accept);
    let hash = p.created();
    p.expired();
    assert_eq!(p.rows().len(), 2);
    let (s, wallet) = (&p.f.session, p.f.wallet);
    let wipe = p.f.grant(GrantPurpose::Wipe, Credential::None);
    p.f.engine.block_on(s.remove_wallet(wallet, wipe)).unwrap();
    assert!(p.rows().is_empty());
    let again =
        p.f.engine
            .block_on(
                s.import_wallet(
                    zeroize::Zeroizing::new(
                        crate::platform::lease::session_tests::PHRASE
                            .as_bytes()
                            .to_vec(),
                    ),
                    zeroize::Zeroizing::new(Vec::new()),
                    crate::ImportOptions {
                        birth_height: Some(0),
                        core_compat: false,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
    assert_eq!(again, wallet);
    let hashes: Vec<_> = p.rows().into_iter().map(|r| r.hash).collect();
    assert_eq!(hashes, vec![hash]);
    assert_eq!(
        p.state(&hash),
        MyProposalState::FeePaid { confirmations: 0 }
    );
}

/// Review r1 m-2: a tick reads each wallet's history once for all its live
/// proposals, and not at all for settled (expired, public) ones.
#[test]
fn a_tick_reads_the_history_once_and_never_for_settled_proposals() {
    let p = props(Mode::Accept);
    let a = p.created();
    p.f.credit(2, 3 * COIN);
    let other = ProposalDraft {
        name: "dw-proposal-2".into(),
        ..draft()
    };
    let b = p.create_with(other, p.spend(2 * COIN)).unwrap();
    let b = dw_governance::object::parse_display_hex(&b.hash).unwrap();
    assert_ne!(a, b);
    p.expired();
    let n = p.fee_scans();
    p.tick(Instant::now());
    assert_eq!(p.fee_scans(), n + 1);
    p.echo(&a);
    p.echo(&b);
    let n = p.fee_scans();
    p.tick(Instant::now());
    assert_eq!(p.fee_scans(), n);
}
