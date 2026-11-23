//! H-03 row R9b: a phrase import (onboarding's restore) killed with SIGKILL
//! in a child process inside `import_secret_locked`, once its rows are
//! stored, once its seed is stored, and once its wallet is registered; after
//! the relaunch the wallet opens and is either fully imported or not listed,
//! and importing the phrase again leaves one wallet with its seed.

use std::sync::Arc;

use zeroize::Zeroizing;

use crate::keys::{AT_IMPORT_POINT, IMPORT_REGISTER, IMPORT_ROWS, IMPORT_SECRET};
use crate::kill9;
use crate::{Engine, ImportOptions, NetworkSession, WalletId};

const ROW: &str = "phrase_import";
const CHILD: &str = "kill9::import::import_child";
const PASS: &[u8] = b"kill9 import";
const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn import(engine: &Engine, session: &Arc<NetworkSession>) -> Result<WalletId, crate::EngineError> {
    engine.block_on(session.import_wallet(
        Zeroizing::new(PHRASE.to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            birth_height: Some(0),
            ..Default::default()
        },
    ))
}

/// Run by the R9b rows in a child process: the import, held at its point.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn import_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    let (engine, session) = kill9::engine_at(&child.root, PASS, true);
    engine
        .block_on(session.adopt_signing_window_default())
        .unwrap();
    let point = child.point.clone();
    *AT_IMPORT_POINT.lock().unwrap() = Some(Box::new(move |at| {
        if at == point {
            kill9::reached(&child, at);
        }
    }));
    panic!("not held: {:?}", import(&engine, &session));
}

/// Wallets `session` lists: their ids, each checked to hold its seed.
fn listed(session: &NetworkSession) -> Vec<WalletId> {
    let wallets = session.wallet_infos().unwrap();
    for w in &wallets {
        assert!(!w.watch_only && w.has_mnemonic, "{w:?}");
        assert!(session.vault().has_wallet_secret(&w.wallet_id.0), "{w:?}");
    }
    wallets.iter().map(|w| w.wallet_id).collect()
}

/// Kills the import at `point`, relaunches, and checks that the wallet
/// opens: listed with its seed if `registered`, else not listed, an import
/// again then storing it. Either way a second relaunch lists that one
/// wallet with its seed, and importing it again is refused.
fn killed_at(point: &str, registered: bool) {
    let (dir, report) = kill9::killed_at(CHILD, ROW, point);
    assert_eq!(report, point);
    let id = {
        let (engine, session) = kill9::engine_at(dir.path(), PASS, true);
        let shown = listed(&session);
        if registered {
            assert_eq!(shown.len(), 1, "registered before the kill");
            shown[0]
        } else {
            assert_eq!(shown, vec![], "not registered before the kill");
            let id = import(&engine, &session).unwrap();
            assert_eq!(listed(&session), vec![id]);
            id
        }
    };
    let (engine, session) = kill9::engine_at(dir.path(), PASS, true);
    assert_eq!(listed(&session), vec![id]);
    assert!(matches!(
        import(&engine, &session),
        Err(crate::EngineError::WalletAlreadyExists(w)) if w == id.to_string()
    ));
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9b_import_killed_after_its_rows_imports_again() {
    killed_at(IMPORT_ROWS, false);
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9b_import_killed_after_its_seed_imports_again() {
    killed_at(IMPORT_SECRET, false);
}

/// Its name, creation time and created-here marker are written after the
/// registration: the wallet shows the default name a failed name write
/// leaves (not checked here).
#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9b_import_killed_after_registering_keeps_the_wallet() {
    killed_at(IMPORT_REGISTER, true);
}
