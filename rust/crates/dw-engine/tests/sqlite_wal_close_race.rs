//! #105 against the SQLite the workspace links, with no lock of ours around
//! it: connections that open, read and close one WAL database on many
//! threads must never hang, raw `rusqlite` ones and the app database's.
//! SQLite 3.51.0 and 3.51.1 can deadlock when one thread closes the last
//! connection to a WAL database while another opens the same file; 3.51.2
//! fixes it. Alone in its own test binary: it replaces SQLite's `stat` for
//! the whole process. Unix only: the bug and the `stat` it slows are the unix
//! VFS's.

#![cfg(unix)]

use std::ffi::{c_char, c_int, c_void};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Once, OnceLock, mpsc};
use std::time::{Duration, Instant};

use dw_appdb::{AppDb, GLOBAL_SCOPE};
use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig};
use rusqlite::{Connection, ffi};

const THREADS: usize = 4;
const RUN: Duration = Duration::from_secs(3);
const FINISH: Duration = Duration::from_secs(60);
const READY: Duration = Duration::from_secs(30);
const STAT_DELAY: Duration = Duration::from_millis(2);
const STEPS: [&str; 4] = ["between cycles", "the open", "a read", "the drop"];

type Stat = unsafe extern "C" fn(*const c_char, *mut c_void) -> c_int;

static REAL_STAT: OnceLock<Stat> = OnceLock::new();
static SLOW_STAT: Once = Once::new();

unsafe extern "C" fn slow_stat(path: *const c_char, buf: *mut c_void) -> c_int {
    std::thread::sleep(STAT_DELAY);
    let real = REAL_STAT.get().expect("set before the override");
    // SAFETY: the arguments SQLite passed, to the function it called before.
    unsafe { real(path, buf) }
}

/// SQLite's open stats the file while it holds the unix VFS's global mutex
/// (`findReusableFd`); a slower `stat` widens the window in which a close
/// can take the file's own mutex and then wait for the global one.
fn slow_down_sqlite_stat() {
    // SAFETY: the default VFS lives for the process; its system-call table
    // is changed here once, before any connection is open: every test of
    // this binary that opens one calls this first.
    // `slow_stat` takes the `struct stat *` of the `stat` it replaces as an
    // untyped pointer and only passes it on, which is the same call on the
    // unix ABIs.
    SLOW_STAT.call_once(|| unsafe {
        let vfs = ffi::sqlite3_vfs_find(std::ptr::null());
        assert!(!vfs.is_null());
        let get = (*vfs).xGetSystemCall.expect("unix VFS");
        let set = (*vfs).xSetSystemCall.expect("unix VFS");
        let real = get(vfs, c"stat".as_ptr()).expect("stat is a unix VFS system call");
        REAL_STAT
            .set(std::mem::transmute::<unsafe extern "C" fn(), Stat>(real))
            .expect("once");
        let slow = std::mem::transmute::<Stat, unsafe extern "C" fn()>(slow_stat);
        assert_eq!(set(vfs, c"stat".as_ptr(), Some(slow)), ffi::SQLITE_OK);
    });
}

fn journal_mode(path: &Path) -> String {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap()
}

/// Steps of one cycle on `path`: the open, a read, the drop; `at` marks each.
type Cycle = fn(&Path, &dyn Fn(u8));

/// A raw connection: the read of the schema opens its WAL, so the drop
/// closes it.
fn raw_cycle(path: &Path, at: &dyn Fn(u8)) {
    at(1);
    let conn = Connection::open(path).unwrap();
    at(2);
    let tables: i64 = conn
        .query_row("SELECT count(*) FROM main.sqlite_schema", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(tables > 0);
    at(3);
    drop(conn);
}

/// The app database, which opens and closes its connection with no lock of
/// its own since the SQLite it links fixes #105.
fn app_db_cycle(path: &Path, at: &dyn Fn(u8)) {
    at(1);
    let db = AppDb::open(path).unwrap();
    at(2);
    assert_eq!(db.setting(GLOBAL_SCOPE, "race").unwrap(), None);
    at(3);
    drop(db);
}

/// Four threads run `cycle` on `path` over and over for
/// [`RUN`]; every one of them must run at least one cycle and then stop
/// within [`FINISH`]. A hang ends the process: the stuck thread holds
/// SQLite's global VFS mutex, which would block the other tests of this binary
/// with no timeout.
fn race_opens_and_closes(fixture: &str, path: PathBuf, cycle: Cycle) {
    assert_eq!(journal_mode(&path), "wal");
    let path = Arc::new(path);
    let go = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicUsize::new(0));
    let cycles: Arc<[AtomicU64; THREADS]> = Arc::new(Default::default());
    let steps: Arc<[AtomicU8; THREADS]> = Arc::new(Default::default());
    let (done, finished) = mpsc::channel();
    for i in 0..THREADS {
        let (path, go, stop, ready, cycles, steps, done) = (
            Arc::clone(&path),
            Arc::clone(&go),
            Arc::clone(&stop),
            Arc::clone(&ready),
            Arc::clone(&cycles),
            Arc::clone(&steps),
            done.clone(),
        );
        std::thread::spawn(move || {
            ready.fetch_add(1, Ordering::SeqCst);
            while !go.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            while !stop.load(Ordering::Relaxed) {
                cycle(path.as_path(), &|step| {
                    steps[i].store(step, Ordering::Relaxed)
                });
                steps[i].store(0, Ordering::Relaxed);
                cycles[i].fetch_add(1, Ordering::Relaxed);
            }
            let _ = done.send(());
        });
    }
    drop(done);
    let ready_by = Instant::now() + READY;
    while ready.load(Ordering::SeqCst) < THREADS {
        assert!(
            Instant::now() < ready_by,
            "{fixture}: only {} of {THREADS} threads started within {READY:?}",
            ready.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    go.store(true, Ordering::SeqCst);
    std::thread::sleep(RUN);
    stop.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + FINISH;
    for _ in 0..THREADS {
        let left = deadline.saturating_duration_since(Instant::now());
        let ended = finished.recv_timeout(left);
        assert_ne!(
            ended,
            Err(mpsc::RecvTimeoutError::Disconnected),
            "a thread panicked; its message is above"
        );
        if ended.is_err() {
            let in_flight: Vec<_> = steps
                .iter()
                .map(|s| STEPS[usize::from(s.load(Ordering::Relaxed))])
                .collect();
            let per_thread: Vec<_> = cycles.iter().map(|c| c.load(Ordering::Relaxed)).collect();
            let mut err = std::io::stderr().lock();
            let _ = writeln!(
                err,
                "{fixture}: hung on SQLite {}: threads not done {FINISH:?} after the stop; \
                 cycles per thread {per_thread:?}; threads in {in_flight:?}",
                rusqlite::version(),
            );
            let _ = err.flush();
            std::process::exit(101);
        }
    }
    for (i, c) in cycles.iter().enumerate() {
        assert!(
            c.load(Ordering::Relaxed) > 0,
            "{fixture}: thread {i} ran no cycle in {RUN:?}"
        );
    }
}

#[test]
fn the_linked_sqlite_has_the_wal_close_deadlock_fix() {
    assert!(
        rusqlite::version_number() >= 3_051_002,
        "SQLite {} can deadlock closing a WAL database (#105); 3.51.2 fixes it",
        rusqlite::version()
    );
}

#[test]
fn raw_opens_and_closes_of_a_wal_file_racing_on_many_threads_never_hang() {
    slow_down_sqlite_stat();
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("plain.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();
    }
    race_opens_and_closes("plain WAL file", path, raw_cycle);
}

#[test]
fn raw_opens_and_closes_of_a_wallet_store_racing_on_many_threads_never_hang() {
    slow_down_sqlite_stat();
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join("wallet.sqlite");
    drop(SqlitePersister::open(SqlitePersisterConfig::new(path.clone())).unwrap());
    race_opens_and_closes("wallet store", path, raw_cycle);
}

#[test]
fn app_database_opens_and_closes_racing_on_many_threads_never_hang() {
    slow_down_sqlite_stat();
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join(dw_appdb::APP_DB_FILE);
    drop(AppDb::open(&path).unwrap());
    race_opens_and_closes("app database", path, app_db_cycle);
}
