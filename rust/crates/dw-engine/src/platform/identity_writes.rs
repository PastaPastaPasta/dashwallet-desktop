//! The locks of one wallet identity's Platform writes (DP1-03, shared since
//! DP2-02). Every write that spends the identity's credits or moves its
//! revision or nonce (a name registration, a contact request or accept, a
//! key upgrade) holds [`IdentityLocks::write`] for its whole run, whichever
//! feature makes it: two writes never race on the nonce, the revision or a
//! unique index, and a balance read before and after a write measures that
//! write alone. A write that spends credits binds `balances.rs`'s
//! `AfterSpend` just before it, so the balance is refreshed once the lock
//! is released.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use dpp::prelude::Identifier;

use crate::WalletId;

/// One identity's locks.
#[derive(Default)]
pub(crate) struct IdentityLocks {
    /// Held for a whole Platform write of the identity (module doc). A
    /// registration holds it throughout: two calls for one label would each
    /// pay a preorder, and a call's `NameKind` reads the open contests
    /// another call is changing.
    pub(crate) write: tokio::sync::Mutex<()>,
    /// Held, never across the network, around each change to the pending
    /// labels and the wallet's names that goes with it, and around a read
    /// of both, so a read never pairs one's old state with the other's new
    /// (names.rs).
    pub(crate) state: tokio::sync::Mutex<()>,
}

/// The locks of `identity` in `wallet`, shared by every caller while any
/// holds them.
pub(crate) fn identity_locks(wallet: WalletId, identity: Identifier) -> Arc<IdentityLocks> {
    type Locks = HashMap<(WalletId, Identifier), Weak<IdentityLocks>>;
    static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    let key = (wallet, identity);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(IdentityLocks::default());
    locks.insert(key, Arc::downgrade(&lock));
    lock
}
