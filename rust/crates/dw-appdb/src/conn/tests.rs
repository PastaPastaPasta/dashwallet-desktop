use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::{APP_DB_FILE, AppDb};

const FINISH: Duration = Duration::from_secs(60);

fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("still running after {limit:?}"))
}

#[test]
fn an_open_and_a_close_on_a_thread_already_holding_the_lock_pass_through() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(APP_DB_FILE);
    within(FINISH, move || {
        let held = OpenClose::enter();
        drop(AppConn::open(&path, OpenFlags::default()).unwrap());
        drop(held);
        assert!(!HOLDING.get());
    });
}

#[test]
fn a_panic_while_holding_the_lock_leaves_later_opens_working() {
    let panicked = std::thread::spawn(|| {
        let _held = OpenClose::enter();
        panic!("while holding the open/close lock");
    })
    .join();
    assert!(panicked.is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(APP_DB_FILE);
    within(FINISH, move || {
        drop(AppDb::open(&path).unwrap());
        assert!(!HOLDING.get());
    });
}
