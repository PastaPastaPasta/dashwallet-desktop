//! H-03 rows R9 and R9b: a `.dwbackup` restore (R9) and a phrase import
//! (R9b, onboarding's restore) killed with SIGKILL in a child process inside
//! `import_secret_locked`: once its rows are stored, once its seed is
//! stored, and once its wallet is registered. After the relaunch the wallet
//! opens and is either fully there or not listed; running the operation
//! again then leaves one wallet with its seed and the backup's coin lock.

use std::path::Path;
use std::sync::Arc;

use zeroize::Zeroizing;

use crate::keys::{AT_IMPORT_POINT, IMPORT_REGISTER, IMPORT_ROWS, IMPORT_SECRET};
use crate::kill9;
use crate::{Engine, EngineError, ImportOptions, NetworkSession, WalletId};

const IMPORT_ROW: &str = "phrase_import";
const IMPORT_CHILD: &str = "kill9::import::import_child";
const RESTORE_ROW: &str = "backup_restore";
const RESTORE_CHILD: &str = "kill9::import::restore_child";
const PASS: &[u8] = b"kill9 import";
const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn import(engine: &Engine, session: &Arc<NetworkSession>) -> Result<WalletId, EngineError> {
    engine.block_on(session.import_wallet(
        Zeroizing::new(PHRASE.to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            birth_height: Some(0),
            ..Default::default()
        },
    ))
}

fn backup_file(root: &Path) -> std::path::PathBuf {
    root.join("w.dwbackup")
}

fn restore(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    root: &Path,
) -> Result<WalletId, EngineError> {
    let ids = engine
        .block_on(session.restore_backup(backup_file(root), Some(Zeroizing::new(PASS.to_vec()))))?;
    assert_eq!(ids.len(), 1);
    Ok(ids[0])
}

/// The child's operation, held at its point once `make` is done.
fn held(
    child: kill9::Child,
    run: impl FnOnce(&Engine, &Arc<NetworkSession>) -> Result<WalletId, EngineError>,
) {
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
    panic!("not held: {:?}", run(&engine, &session));
}

/// Run by the R9b rows in a child process: the import, held at its point.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn import_child() {
    let Some(child) = kill9::child(IMPORT_ROW) else {
        return;
    };
    held(child, import);
}

/// Run by the R9 rows in a child process: a backup of the phrase's wallet
/// with one coin lock, made in a data folder of its own, then its restore,
/// held at its point.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn restore_child() {
    let Some(child) = kill9::child(RESTORE_ROW) else {
        return;
    };
    let root = child.root.clone();
    {
        let (engine, session) = kill9::engine_at(&root.join("source"), PASS, true);
        let id = import(&engine, &session).unwrap();
        let scope = id.to_string();
        engine
            .block_on(session.appdb_op(move |db| db.lock_manual(&scope, TXID, 3, 7)))
            .unwrap();
        engine
            .block_on(session.backup_wallet(id, backup_file(&root), None))
            .unwrap();
    }
    held(child, |engine, session| restore(engine, session, &root));
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

fn locks(engine: &Engine, session: &NetworkSession, id: WalletId) -> Vec<(String, u32)> {
    let scope = id.to_string();
    engine
        .block_on(session.appdb_op(move |db| db.locks(&scope)))
        .unwrap()
        .into_iter()
        .map(|l| (l.txid, l.vout))
        .collect()
}

/// Kills `row`'s child at `point`, relaunches, and checks that the wallet
/// opens: listed with its seed if `registered`, else not listed, the
/// operation run again then storing it. Either way a second relaunch lists
/// that one wallet with its seed and `lock`, and running it again is
/// refused.
fn killed_at(child: &str, row: &str, point: &str, registered: bool) {
    let (dir, report) = kill9::killed_at(child, row, point);
    assert_eq!(report, point);
    let root = dir.path();
    let run = |engine: &Engine, session: &Arc<NetworkSession>| match row {
        IMPORT_ROW => import(engine, session),
        _ => restore(engine, session, root),
    };
    let lock = match row {
        IMPORT_ROW => vec![],
        _ => vec![(TXID.to_string(), 3)],
    };
    let id = {
        let (engine, session) = kill9::engine_at(root, PASS, true);
        let shown = listed(&session);
        if registered {
            assert_eq!(shown.len(), 1, "registered before the kill");
            shown[0]
        } else {
            assert_eq!(shown, vec![], "not registered before the kill");
            let id = run(&engine, &session).unwrap();
            assert_eq!(listed(&session), vec![id]);
            id
        }
    };
    let (engine, session) = kill9::engine_at(root, PASS, true);
    assert_eq!(listed(&session), vec![id]);
    assert_eq!(locks(&engine, &session, id), lock);
    let again = run(&engine, &session);
    assert!(
        matches!(
            &again,
            Err(EngineError::WalletAlreadyExists(w)) if *w == id.to_string()
        ) || matches!(
            &again,
            Err(EngineError::Backup(crate::backup::BackupFailure::AlreadyExists(w))) if *w == id
        ),
        "{again:?}"
    );
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9_restore_killed_after_its_rows_restores_again() {
    killed_at(RESTORE_CHILD, RESTORE_ROW, IMPORT_ROWS, false);
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9_restore_killed_after_its_seed_restores_again() {
    killed_at(RESTORE_CHILD, RESTORE_ROW, IMPORT_SECRET, false);
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9_restore_killed_after_registering_keeps_the_wallet() {
    killed_at(RESTORE_CHILD, RESTORE_ROW, IMPORT_REGISTER, true);
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9b_import_killed_after_its_rows_imports_again() {
    killed_at(IMPORT_CHILD, IMPORT_ROW, IMPORT_ROWS, false);
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9b_import_killed_after_its_seed_imports_again() {
    killed_at(IMPORT_CHILD, IMPORT_ROW, IMPORT_SECRET, false);
}

/// Its name, creation time and created-here marker are written after the
/// registration: the wallet shows the default name a failed name write
/// leaves (not checked here).
#[test]
#[ignore = "H-03 kill -9 matrix (nightly)"]
fn kill9_r9b_import_killed_after_registering_keeps_the_wallet() {
    killed_at(IMPORT_CHILD, IMPORT_ROW, IMPORT_REGISTER, true);
}
