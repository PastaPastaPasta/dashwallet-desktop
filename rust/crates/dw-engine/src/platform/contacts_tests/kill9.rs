//! H-03 row R7: a sync pass's changeset killed with SIGKILL in a child
//! process once wallet.sqlite committed it, before the changeset tap
//! recorded it; after the relaunch the wallet holds it and the journal
//! (`dp_events`) has its row: Dave's request, or a payment to Bob. It does
//! not yet (#381); the control shows the check finds a row that is there.

use super::super::journal_tests::{ABANDON_12, contacts, incoming};
use super::*;
use crate::coins::TestPause;
use crate::kill9;

const ROW: &str = "sync_apply";
const CHILD: &str = "platform::contacts_tests::kill9::sync_child";
const PASS: &[u8] = b"kill9 sync";
const TXID: &str = "aa11";

fn fixture_at(dir: tempfile::TempDir, root: &std::path::Path, first: bool) -> Fixture {
    let (engine, s) = kill9::engine_at(root, PASS, true);
    let wallet = if first {
        let phrase = Zeroizing::new(ABANDON_12.to_vec());
        let options = ImportOptions::default();
        (engine.block_on(s.import_wallet(phrase, Zeroizing::new(Vec::new()), options))).unwrap()
    } else {
        let wallets = s.wallet_infos().unwrap();
        assert_eq!(wallets.len(), 1);
        wallets[0].wallet_id
    };
    let f = Fixture {
        engine,
        s,
        wallet,
        sink: Arc::new(Recorder::default()),
        _dir: dir,
    };
    if first {
        f.give_me();
    }
    f
}

/// What the pass stores: news after the wallet's first bring-up.
fn changeset(point: &str, now_ms: u64) -> PlatformWalletChangeSet {
    match point {
        "request" => contacts(incoming(ME, DAVE, now_ms)),
        "payment" => PlatformWalletChangeSet {
            dashpay_payments_overlay: Some(BTreeMap::from([(
                id(ME),
                BTreeMap::from([(
                    TXID.to_string(),
                    PaymentEntry {
                        counterparty_id: id(BOB),
                        amount_duffs: 1_000,
                        memo: None,
                        direction: PaymentDirection::Received,
                        status: PaymentStatus::Confirmed,
                    },
                )]),
            )])),
            ..Default::default()
        },
        other => panic!("no sync kill point {other:?}"),
    }
}

/// Run by [`killed`] in a child process: the store held after its commit,
/// before the tap's record.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn sync_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    let f = fixture_at(kill9::scratch(&child), &child.root, true);
    let live = f.s.live().unwrap();
    let pause = Arc::new(TestPause::default());
    *live.tap.pause_record.lock().unwrap() = Some(Arc::clone(&pause));
    let cs = changeset(&child.point, crate::events::unix_now() * 1_000);
    let (store, rt, wallet) = (Arc::clone(&live.store), f.s.rt.clone(), f.wallet);
    std::thread::spawn(move || {
        let _rt = rt.enter();
        PlatformWalletPersistence::store(&*store, wallet.0, cs).unwrap();
    });
    kill9::eventually("the store is committed", || {
        pause.reached.load(Ordering::SeqCst)
    });
    kill9::reached(&child, "{}");
}

/// R7 at `point`; with `control`, the same store run to its end, then
/// the engine closed and opened again. With `journal`, its journal row is
/// checked too (#381: a kill before it leaves none).
fn killed(point: &str, control: bool, journal: bool) {
    let f = if control {
        let dir = dw_testutil::private_tempdir();
        let root = dir.path().to_path_buf();
        let first = fixture_at(dw_testutil::private_tempdir(), &root, true);
        let cs = changeset(point, crate::events::unix_now() * 1_000);
        first
            .s
            .live()
            .unwrap()
            .store
            .store(first.wallet.0, cs)
            .unwrap();
        first.engine.block_on(first.engine.shutdown()).unwrap();
        drop(first);
        fixture_at(dir, &root, false)
    } else {
        let (dir, _) = kill9::killed_at(CHILD, ROW, point);
        let root = dir.path().to_path_buf();
        fixture_at(dir, &root, false)
    };
    let held = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen = Arc::clone(&held);
    let request = point == "request";
    f.with_me(move |m, _| {
        *seen.lock().unwrap() = Some(if request {
            m.dashpay()
                .incoming_contact_requests()
                .contains_key(&id(DAVE))
        } else {
            m.dashpay_payments_mut().contains_key(TXID)
        });
    });
    assert_eq!(*held.lock().unwrap(), Some(true), "wallet.sqlite has it");
    if !journal {
        return;
    }
    let reference = if request { b58(DAVE) } else { TXID.into() };
    kill9::eventually("its journal row", || {
        f.journal()
            .iter()
            .any(|r| r.contact == reference || r.reference == reference)
    });
}

#[test]
#[ignore = "finding #381"]
fn kill9_finding_381_request_stored_before_its_journal_row_is_journaled() {
    killed("request", false, true);
}

#[test]
#[ignore = "finding #381"]
fn kill9_finding_381_payment_stored_before_its_journal_row_is_journaled() {
    killed("payment", false, true);
}

/// The rows' check, on stores that were not killed: each makes its row.
#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r7_control_stores_that_finish_are_journaled() {
    killed("request", true, true);
    killed("payment", true, true);
}

/// A request or payment stored before a kill is in the wallet after the
/// relaunch (its journal row is #381's finding).
#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r7_stores_killed_before_their_journal_row_are_in_the_wallet() {
    killed("request", false, false);
    killed("payment", false, false);
}
