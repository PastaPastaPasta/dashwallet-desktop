//! H-03 row R10: a top-up killed with SIGKILL in a child process once its
//! asset lock is signed and tracked, before its broadcast; after the
//! relaunch nothing is funded twice: the step is listed as stuck funding,
//! which Repair resolves, and a new top-up is refused on it until then.

use super::*;
use crate::kill9;

const ROW: &str = "topup";
const CHILD: &str = "platform::topup_tests::kill9::topup_child";

fn fixture_at(dir: tempfile::TempDir, root: &std::path::Path, first: bool) -> Fixture {
    let (engine, session) = kill9::engine_at(root, PASS, false);
    let wallet = if first {
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
    assert!(engine.block_on(session.leases.refresh_funding(wallet)));
    Fixture {
        dir,
        engine,
        session,
        wallet,
        encrypted: true,
    }
}

/// Run by the row in a child process: a top-up held once its lock is
/// tracked, reported with what Platform and the library's calls saw.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn topup_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    assert_eq!(child.point, "fund_tracked");
    let f = fixture_at(kill9::scratch(&child), &child.root, true);
    let lib = f.stored_fake(None);
    lib.w().hang = Some((Call::Fund, When::Tracked));
    let grant = f.grant(DUFFS + f.fee_bound());
    let (session, wallet, run) = (Arc::clone(&f.session), f.wallet, Arc::clone(&lib));
    let _task = f.session.rt.spawn(async move {
        topup::top_up(&session, wallet, &*run, &b58(identity()), DUFFS, &grant).await
    });
    kill9::eventually("the top-up's lock is tracked", || {
        lib.hung.load(Ordering::SeqCst)
    });
    let w = lib.w();
    assert!(w.consumed.is_empty());
    let report = serde_json::json!({ "funds": w.funds, "applies": w.applies });
    drop(w);
    kill9::reached(&child, &report.to_string());
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r10_topup_killed_with_its_lock_tracked_funds_once() {
    let (dir, report) = kill9::killed_at(CHILD, ROW, "fund_tracked");
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    let root = dir.path().to_path_buf();
    let f = fixture_at(dir, &root, false);
    let lib = f.stored_fake(None);
    lib.w().funds = report["funds"].as_u64().unwrap() as u32;
    let first = top_up_step(0, 0);
    // The lock was signed, never broadcast: the wallet's store never held
    // it (no `Built` lock is stored while its funding signs), and the
    // dispatch journal's signature makes the step's outcome unknown.
    assert!(lib.rt_locks().is_empty());
    assert!(f.pending(&lib).is_empty());
    assert_eq!(f.status(&first), Some(DispatchState::MaybeSent));
    assert_eq!(f.signed(&first).len(), 1);
    // Repair lists it ("Resolve stuck funding" is the way out).
    let stuck = (f.engine.block_on(topup::stuck(&f.session, f.wallet, &*lib))).unwrap();
    assert_eq!(
        stuck,
        vec![StuckTopUpFunding {
            step: first.clone(),
            identity_index: 0,
            identity: Some(b58(identity())),
            state: DispatchState::MaybeSent,
        }]
    );
    // Never funded again while it may be out.
    for _ in 0..2 {
        match f.top_up(&lib, DUFFS) {
            Err(CreditsError::Platform(PlatformError::BroadcastUnknown { artifact })) => {
                assert_eq!(artifact, first)
            }
            other => panic!("not broadcast_unknown: {other:?}"),
        }
    }
    assert_eq!(lib.counts(), (1, 0));
    assert!(f.signed(&top_up_step(0, 1)).is_empty());
    assert_eq!(lib.balance(), START_BALANCE);
}
