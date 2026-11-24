//! Each load of a wallet from the store checks two statuses an older build
//! stored on the word of an InstantSend lock it never verified (#177, item
//! 2). The store now restores a lock only when it was stored as verified, but
//! these two statuses were stored separately:
//!
//! - a tracked asset lock `InstantSendLocked`, with the lock's proof cached:
//!   resuming it hands Platform that proof, and the identity funding reads as
//!   ready;
//! - a DashPay sent payment `Confirmed`: the library's reconcile looks only
//!   at `Pending` entries, so it is never looked at again.
//!
//! What the store holds for the transaction decides:
//!
//! - its record, with a verified lock: an asset lock keeps its status only
//!   when the cached proof carries that same lock; the payment stays;
//! - its record, mined: the asset lock goes back (its proof is from no
//!   verified lock the store holds; the ChainLock gives a new one); the
//!   payment stays;
//! - its record, unmined and without a verified lock, or the transaction
//!   dropped at open as an unsigned spend (`core_dropped_unsigned_spends`):
//!   both go back;
//! - no record and not dropped: both stay, as nothing shows them wrong;
//! - unreadable: the asset lock goes back; the payment, shown only, stays.
//!
//! Going back means `Broadcast` without the proof for an asset lock, which
//! the resume then proves again from verified evidence, and `Pending` for a
//! payment, which the reconcile confirms again on a verified lock or a block.
//!
//! It runs on the start state the store hands the wallet manager, for each
//! wallet the manager does not hold yet: its first load in the session, or
//! the first after an unload. Nothing has read the old statuses then, and no
//! live event races it. A reload hands back every open wallet as well, but
//! the manager keeps its live state for those, so they are skipped: writing a
//! snapshot's correction under a live wallet would part the store from
//! memory. The corrections are written back through the store. It keeps no
//! marker: once the statuses agree with the store a run changes nothing, and
//! a run at each load leaves nothing a lost write could make disagree with
//! the store.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use dashcore::{InstantLock, OutPoint, Txid};
use dpp::prelude::{AssetLockProof, Identifier};
use key_wallet::managed_account::transaction_record::TransactionRecord;
use key_wallet::transaction_checking::TransactionContext;
use platform_wallet::changeset::{
    AssetLockChangeSet, AssetLockEntry, ClientStartState, PlatformWalletChangeSet,
    PlatformWalletPersistence,
};
use platform_wallet::wallet::asset_lock::tracked::AssetLockStatus;
use platform_wallet::wallet::identity::{PaymentDirection, PaymentEntry, PaymentStatus};
use platform_wallet_storage::SqlitePersister;

type RawWalletId = [u8; 32];

/// What the store holds for a transaction.
pub(crate) enum Stored {
    Record(Box<TransactionRecord>),
    /// Dropped at open as an unsigned spend.
    Dropped,
    Missing,
    Unreadable,
}

fn verified_lock(record: &TransactionRecord) -> Option<&InstantLock> {
    match &record.context {
        TransactionContext::InstantSend(lock) => Some(lock),
        _ => None,
    }
}

/// Whether an `InstantSendLocked` asset lock with `proof` keeps its status.
pub(crate) fn lock_stays(stored: &Stored, proof: Option<&AssetLockProof>) -> bool {
    match stored {
        Stored::Missing => true,
        Stored::Dropped | Stored::Unreadable => false,
        Stored::Record(record) => match (verified_lock(record), proof) {
            (Some(verified), Some(AssetLockProof::Instant(p))) => p.instant_lock() == verified,
            _ => false,
        },
    }
}

/// Whether a `Confirmed` sent payment goes back to `Pending`.
pub(crate) fn payment_unproven(stored: &Stored) -> bool {
    match stored {
        Stored::Dropped => true,
        Stored::Record(r) => r.block_info().is_none() && verified_lock(r).is_none(),
        Stored::Missing | Stored::Unreadable => false,
    }
}

/// The transactions of wallet `wallet` dropped at open; `None` if they
/// cannot be read.
fn dropped_txids(db_path: &Path, wallet: &RawWalletId) -> Option<HashSet<Txid>> {
    use dashcore::hashes::Hash;
    let read = || -> Result<HashSet<Txid>, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut stmt =
            conn.prepare("SELECT txid FROM core_dropped_unsigned_spends WHERE wallet_id = ?1")?;
        let rows = stmt.query_map([&wallet[..]], |r| r.get::<_, Vec<u8>>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            if let Ok(txid) = Txid::from_slice(&row?) {
                out.insert(txid);
            }
        }
        Ok(out)
    };
    read()
        .map_err(|e| {
            tracing::warn!(error = %e, "could not read the transactions dropped at open");
        })
        .ok()
}

fn stored(
    persister: &SqlitePersister,
    wallet: RawWalletId,
    dropped: Option<&HashSet<Txid>>,
    txid: &Txid,
) -> Stored {
    match persister.get_core_tx_record(wallet, txid) {
        Ok(Some(record)) => Stored::Record(Box::new(record)),
        Ok(None) => match dropped {
            Some(d) if d.contains(txid) => Stored::Dropped,
            Some(_) => Stored::Missing,
            None => Stored::Unreadable,
        },
        Err(e) => {
            tracing::warn!(%txid, error = %e, "could not read a stored transaction to check its lock");
            Stored::Unreadable
        }
    }
}

/// What the check put back and stored, by wallet.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Rechecked {
    pub(crate) asset_locks: Vec<OutPoint>,
    pub(crate) payments: Vec<(Identifier, String)>,
}

/// Runs the check (module doc) over the wallets of `state` in `only`;
/// `state` is the start state just read from `persister`, whose file is
/// `db_path`.
pub(crate) fn recheck_start_state(
    persister: &SqlitePersister,
    db_path: &Path,
    state: &mut ClientStartState,
    only: &HashSet<RawWalletId>,
) -> BTreeMap<RawWalletId, Rechecked> {
    let mut done = BTreeMap::new();
    for (wallet, start) in state.wallets.iter_mut() {
        if !only.contains(wallet) {
            continue;
        }
        let has_locks = (start.unused_asset_locks.values())
            .flat_map(|m| m.values())
            .any(|l| l.status == AssetLockStatus::InstantSendLocked);
        let identities = start.identity_manager.wallet_identities.get_mut(wallet);
        let has_payments = identities.as_ref().is_some_and(|ids| {
            ids.values().any(|m| {
                m.dashpay().payments.values().any(|e| {
                    e.direction == PaymentDirection::Sent && e.status == PaymentStatus::Confirmed
                })
            })
        });
        if !has_locks && !has_payments {
            continue;
        }
        let dropped = dropped_txids(db_path, wallet);

        let mut locks = AssetLockChangeSet::default();
        for lock in start
            .unused_asset_locks
            .values_mut()
            .flat_map(|m| m.values_mut())
        {
            if lock.status != AssetLockStatus::InstantSendLocked {
                continue;
            }
            let read = stored(persister, *wallet, dropped.as_ref(), &lock.out_point.txid);
            if lock_stays(&read, lock.proof.as_ref()) {
                continue;
            }
            lock.status = AssetLockStatus::Broadcast;
            lock.proof = None;
            locks
                .asset_locks
                .insert(lock.out_point, AssetLockEntry::from(&*lock));
        }

        let mut payments: BTreeMap<Identifier, BTreeMap<String, PaymentEntry>> = BTreeMap::new();
        for m in identities.into_iter().flat_map(|ids| ids.values_mut()) {
            let owner = m.id();
            for (txid, entry) in m.dashpay_payments_mut().iter_mut() {
                if entry.direction != PaymentDirection::Sent
                    || entry.status != PaymentStatus::Confirmed
                {
                    continue;
                }
                let Ok(t) = txid.parse::<Txid>() else {
                    continue;
                };
                if !payment_unproven(&stored(persister, *wallet, dropped.as_ref(), &t)) {
                    continue;
                }
                entry.status = PaymentStatus::Pending;
                payments
                    .entry(owner)
                    .or_default()
                    .insert(txid.clone(), entry.clone());
            }
        }

        if locks.asset_locks.is_empty() && payments.is_empty() {
            continue;
        }
        let rechecked = Rechecked {
            asset_locks: locks.asset_locks.keys().copied().collect(),
            payments: (payments.iter())
                .flat_map(|(o, by)| by.keys().map(|t| (*o, t.clone())))
                .collect(),
        };
        let changes = PlatformWalletChangeSet {
            asset_locks: (!locks.asset_locks.is_empty()).then_some(locks),
            dashpay_payments_overlay: (!payments.is_empty()).then_some(payments),
            ..Default::default()
        };
        let written = persister
            .store(*wallet, changes)
            .and_then(|()| persister.flush(*wallet));
        match written {
            Ok(()) => {
                tracing::warn!(
                    asset_locks = rechecked.asset_locks.len(),
                    payments = rechecked.payments.len(),
                    "put back statuses stored on an InstantSend lock that is not stored as verified"
                );
                done.insert(*wallet, rechecked);
            }
            Err(e) => {
                // The start state is corrected all the same; the next load
                // checks the store again.
                tracing::warn!(error = %e, "could not store the statuses put back");
            }
        }
    }
    done
}

#[cfg(test)]
#[path = "finality_recheck_tests.rs"]
mod tests;
