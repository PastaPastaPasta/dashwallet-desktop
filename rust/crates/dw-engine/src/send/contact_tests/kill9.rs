//! H-03 row R5a: a contact payment to Bob killed with SIGKILL in a child
//! process after its gate passed and his address was reserved, before
//! anything marks it sent; after the relaunch the contact is not locked, no
//! lock row is left, and a payment to him prepares and goes out.

use super::*;
use crate::kill9;

const ROW: &str = "contact_payment";
const CHILD: &str = "send::contact_tests::kill9::payment_child";

/// [`fixture`]'s wallet on `root`, made and stored on the first launch (our
/// identity, Bob's paying account and contact, his verified record), opened
/// as stored on the relaunch; one confirmed coin in memory either way.
fn fixture_at(dir: tempfile::TempDir, root: &std::path::Path, first: bool) -> Fixture {
    let (engine, session) = kill9::engine_at(root, PASS, true);
    let wallet = if first {
        engine
            .block_on(session.adopt_signing_window_default())
            .unwrap();
        engine
            .block_on(session.import_wallet(
                Zeroizing::new(PHRASE.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap()
    } else {
        let wallets = session.wallet_infos().unwrap();
        assert_eq!(wallets.len(), 1);
        wallets[0].wallet_id
    };
    let f = Fixture {
        _dir: dir,
        engine,
        session,
        wallet,
        seeded: std::sync::atomic::AtomicBool::new(false),
    };
    if first {
        f.give_me();
        f.establish(BOB, false);
        f.store(f.bobs_account(contact_xpub(BOB)));
        f.store_bobs_contact(request(BOB, ME, 2_000_000), Some(0));
        f.verify_contacts();
    }
    f.credit(1, COIN);
    f
}

/// Run by [`kill9_r5a_payment_killed_after_its_reservation_leaves_bob_payable`]
/// in a child process: a payment to Bob held once his address is reserved.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn payment_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    assert_eq!(child.point, "reserved");
    let f = fixture_at(kill9::scratch(&child), &child.root, true);
    let d = f.draft(BOB);
    let pause = Arc::new(crate::coins::TestPause::default());
    (f.session.platform.money_gate).pause_at(Point::Reserved, Arc::clone(&pause));
    let _prepare = f.spawn_prepare(&d);
    kill9::eventually("the payment's address is reserved", || {
        pause.reached.load(Ordering::SeqCst)
    });
    kill9::reached(&child, "{}");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r5a_payment_killed_after_its_reservation_leaves_bob_payable() {
    let (dir, _) = kill9::killed_at(CHILD, ROW, "reserved");
    let root = dir.path().to_path_buf();
    let f = fixture_at(dir, &root, false);
    assert!(f.lock(BOB).is_none(), "nothing went out: Bob is not locked");
    assert!(f.rows().is_empty(), "no lock row");

    let d = f.draft(BOB);
    Seam::install(&d, Net::Accept);
    let p = f.prepare(&d).unwrap();
    let address = paid(&p);
    f.engine.block_on(d.broadcast(p)).unwrap();

    let fresh = [f.contact_address(BOB, 0), f.contact_address(BOB, 1)];
    assert!(
        fresh.iter().any(|a| a.to_string() == address),
        "one of Bob's first two addresses: {address}"
    );
    assert!(f.lock(BOB).is_none());
    assert!(f.rows().is_empty());
}
