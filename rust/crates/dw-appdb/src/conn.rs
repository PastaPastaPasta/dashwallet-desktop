//! The app database's SQLite connections, opened and closed one at a time
//! in this process.
//!
//! SQLite 3.51.0 and 3.51.1 can deadlock when one thread closes the last
//! connection to a WAL database while another opens or closes the same file
//! (#105). Every connection here opens through [`AppConn::open`] and closes
//! in its `Drop`, both under one process-wide lock. The SQLite linked now
//! is 3.51.2 or later, which fixes the deadlock; the lock stays until a
//! follow-up removes it on its own.

use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OpenFlags};

static OPEN_CLOSE: Mutex<()> = Mutex::new(());

thread_local! {
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

/// Held across one SQLite open or close. Taken again on a thread that
/// already holds it, it passes through: no other thread can open or close
/// meanwhile, and waiting on itself would never end.
struct OpenClose(Option<MutexGuard<'static, ()>>);

impl OpenClose {
    fn enter() -> Self {
        if HOLDING.get() {
            return Self(None);
        }
        let guard = OPEN_CLOSE.lock().unwrap_or_else(PoisonError::into_inner);
        HOLDING.set(true);
        Self(Some(guard))
    }
}

impl Drop for OpenClose {
    fn drop(&mut self) {
        if let Some(guard) = self.0.take() {
            HOLDING.set(false);
            drop(guard);
        }
    }
}

/// A connection to the app database. Only the open and the close are
/// serialized; statements run without the lock.
pub(crate) struct AppConn(Option<Connection>);

impl AppConn {
    pub(crate) fn open(path: &Path, flags: OpenFlags) -> rusqlite::Result<Self> {
        let _held = OpenClose::enter();
        Connection::open_with_flags(path, flags).map(|conn| Self(Some(conn)))
    }

    pub(crate) fn open_in_memory() -> rusqlite::Result<Self> {
        let _held = OpenClose::enter();
        Connection::open_in_memory().map(|conn| Self(Some(conn)))
    }
}

impl Deref for AppConn {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.0.as_ref().expect("taken only by drop")
    }
}

impl DerefMut for AppConn {
    fn deref_mut(&mut self) -> &mut Connection {
        self.0.as_mut().expect("taken only by drop")
    }
}

impl Drop for AppConn {
    fn drop(&mut self) {
        if let Some(conn) = self.0.take() {
            let _held = OpenClose::enter();
            drop(conn);
        }
    }
}

#[cfg(test)]
mod tests;
