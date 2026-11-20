//! #105: one thread closing the last connection to a WAL database while
//! another opens the same file must not deadlock. Alone in its own test
//! binary: it replaces SQLite's `stat` for the whole process. Unix only: the
//! bug and the `stat` it slows are the unix VFS's.

#![cfg(unix)]

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use dw_appdb::{APP_DB_FILE, AppDb};
use rusqlite::ffi;

const THREADS: usize = 4;
const RUN: Duration = Duration::from_secs(3);
const FINISH: Duration = Duration::from_secs(60);
const STAT_DELAY: Duration = Duration::from_millis(2);
const STEPS: [&str; 4] = ["between cycles", "AppDb::open", "a read", "the drop"];

type Stat = unsafe extern "C" fn(*const c_char, *mut c_void) -> c_int;

static REAL_STAT: OnceLock<Stat> = OnceLock::new();

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
    // is changed here before any thread uses SQLite. `slow_stat` takes the
    // `struct stat *` of the `stat` it replaces as an untyped pointer and
    // only passes it on, which is the same call on the unix ABIs.
    unsafe {
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
    }
}

#[test]
fn opens_and_closes_of_one_app_database_racing_on_many_threads_never_hang() {
    let dir = tempfile::tempdir().unwrap();
    let path = Arc::new(dir.path().join(APP_DB_FILE));
    drop(AppDb::open(&path).unwrap());
    slow_down_sqlite_stat();
    let stop = Arc::new(AtomicBool::new(false));
    let cycles = Arc::new(AtomicU64::new(0));
    let steps: Arc<[AtomicU8; THREADS]> = Arc::new(Default::default());
    let (done, finished) = mpsc::channel();
    for i in 0..THREADS {
        let (path, stop, cycles, steps, done) = (
            Arc::clone(&path),
            Arc::clone(&stop),
            Arc::clone(&cycles),
            Arc::clone(&steps),
            done.clone(),
        );
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                steps[i].store(1, Ordering::Relaxed);
                let db = AppDb::open(&path).unwrap();
                steps[i].store(2, Ordering::Relaxed);
                db.schema_version().unwrap();
                steps[i].store(3, Ordering::Relaxed);
                drop(db);
                steps[i].store(0, Ordering::Relaxed);
                cycles.fetch_add(1, Ordering::Relaxed);
            }
            let _ = done.send(());
        });
    }
    drop(done);
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
            panic!(
                "hung: threads not done {FINISH:?} after the stop, {} cycles; threads in {in_flight:?}",
                cycles.load(Ordering::Relaxed)
            );
        }
    }
    assert!(cycles.load(Ordering::Relaxed) > 0);
}
