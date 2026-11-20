//! The spending limit's policy generation, committed outside the vault
//! file (`Vault::attach_policy_commitments`): a vault file put back from an
//! older copy cannot bring back an older, higher limit.

mod common;

use common::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dw_vault::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantPurpose, PolicyCommitments, UnlockScope,
    Vault, VaultError,
};

const HIGH: u64 = 500_000_000;

#[derive(Default)]
struct Store {
    rows: Mutex<HashMap<Vec<u8>, u64>>,
    refuse: AtomicBool,
}

impl PolicyCommitments for Store {
    fn committed(&self, vault_id: &[u8]) -> Option<u64> {
        self.rows.lock().unwrap().get(vault_id).copied()
    }

    fn commit(&self, vault_id: &[u8], generation: u64) -> Result<(), String> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err("refused".into());
        }
        let mut rows = self.rows.lock().unwrap();
        let row = rows.entry(vault_id.to_vec()).or_default();
        *row = (*row).max(generation);
        Ok(())
    }

    fn forget(&self, vault_id: &[u8]) -> Result<(), String> {
        self.rows.lock().unwrap().remove(vault_id);
        Ok(())
    }
}

fn open(fx: &Fixture, store: Option<&Arc<Store>>) -> Vault {
    let v = fx.open();
    if let Some(store) = store {
        v.attach_policy_commitments(store.clone());
    }
    v
}

fn created(fx: &Fixture, store: Option<&Arc<Store>>) -> Vault {
    let v = open(fx, store);
    v.create(Some(PASS)).unwrap();
    allow_window(&v);
    v.store_wallet_secret(&wallet(1), &secret(1)).unwrap();
    v
}

fn set_limit(v: &Vault, limit: u64) -> Result<(), VaultError> {
    let change = v
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(PASS),
        )
        .unwrap();
    v.set_quick_unlock_spend_limit(&change.id, limit).map(drop)
}

/// The vault opened again (an app restart) and unlocked with the
/// passphrase; returns the limit in force.
fn reopened(fx: &Fixture, store: Option<&Arc<Store>>) -> (Vault, u64) {
    let v = open(fx, store);
    allow_window(&v);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let limit = v.quick_unlock_policy().spend_limit_duffs;
    (v, limit)
}

fn spend(max_duffs: u64) -> GrantPurpose {
    GrantPurpose::Spend { max_duffs }
}

/// A whole older vault file, its manifest and its higher limit together,
/// counts as limit 0 after a reopen: no window grant above 0, and setting
/// the limit again works.
#[test]
fn an_older_manifest_and_limit_put_back_together_count_as_0() {
    let fx = Fixture::new();
    let store = Arc::new(Store::default());
    let v = created(&fx, Some(&store));
    set_limit(&v, HIGH).unwrap();
    let older = fx.raw();
    set_limit(&v, 0).unwrap();
    drop(v);
    fx.restore_raw(&older);

    let (v, limit) = reopened(&fx, Some(&store));
    assert_eq!(limit, 0);
    assert_eq!(v.signing_window_remaining(), Some(0));
    assert_eq!(
        v.authorize(spend(1), Some(&wallet(1)), Credential::None)
            .map(drop),
        Err(VaultError::CredentialRequired)
    );
    set_limit(&v, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT).unwrap();
    v.unlock(PASS, UnlockScope::Full).unwrap();
    v.authorize(spend(1), Some(&wallet(1)), Credential::None)
        .unwrap();
    drop(v);
    assert_eq!(
        reopened(&fx, Some(&store)).1,
        DEFAULT_QUICK_UNLOCK_SPEND_LIMIT
    );
}

/// A file whose manifest names no policy generation (written before one
/// existed), put back where a commitment exists, counts as limit 0, not
/// the default.
#[test]
fn a_file_without_a_policy_generation_put_back_counts_as_0() {
    let fx = Fixture::new();
    let store = Arc::new(Store::default());
    let v = created(&fx, Some(&store));
    let before = fx.raw();
    set_limit(&v, 10_000_000).unwrap();
    drop(v);
    fx.restore_raw(&before);
    assert_eq!(reopened(&fx, Some(&store)).1, 0);
}

/// A vault whose store has no commitment yet (an upgrade, or a file written
/// before the store was attached): the first passphrase check records the
/// file's generation and keeps the limit; an older copy put back afterwards
/// counts as 0, and the check does not lower the commitment to it.
#[test]
fn the_first_check_records_the_commitment_and_a_later_rollback_is_caught() {
    let fx = Fixture::new();
    let v = created(&fx, None);
    set_limit(&v, HIGH).unwrap();
    let older = fx.raw();
    set_limit(&v, 10_000_000).unwrap();
    let id = v.vault_id().unwrap();
    drop(v);

    let store = Arc::new(Store::default());
    let v = open(&fx, Some(&store));
    assert_eq!(store.committed(&id), None);
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let recorded = store
        .committed(&id)
        .expect("recorded at the passphrase check");
    assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 10_000_000);
    drop(v);

    fx.restore_raw(&older);
    assert_eq!(reopened(&fx, Some(&store)).1, 0);
    assert_eq!(store.committed(&id), Some(recorded));
}

/// The commitment is written before the file. A crash between the two
/// leaves the commitment ahead of the file on disk: limit 0 until set
/// again. A commitment that fails stops the change before the file is
/// written: the old limit stays, on disk and after a reopen.
#[test]
fn a_crash_between_the_commitment_and_the_file_leaves_limit_0() {
    let fx = Fixture::new();
    let store = Arc::new(Store::default());
    let v = created(&fx, Some(&store));
    set_limit(&v, HIGH).unwrap();
    let on_disk = fx.raw();
    set_limit(&v, 10_000_000).unwrap();
    drop(v);
    fx.restore_raw(&on_disk);
    let (v, limit) = reopened(&fx, Some(&store));
    assert_eq!(limit, 0);
    set_limit(&v, 10_000_000).unwrap();
    drop(v);
    assert_eq!(reopened(&fx, Some(&store)).1, 10_000_000);

    let (v, _) = reopened(&fx, Some(&store));
    let on_disk = fx.raw();
    store.refuse.store(true, Ordering::SeqCst);
    assert!(matches!(set_limit(&v, HIGH), Err(VaultError::Storage(_))));
    assert_eq!(fx.raw(), on_disk);
    assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 10_000_000);
    drop(v);
    store.refuse.store(false, Ordering::SeqCst);
    assert_eq!(reopened(&fx, Some(&store)).1, 10_000_000);
}

/// Without a store (tests, tools) the vault keeps its own file's check
/// only, as before: a whole older file brings back its limit.
#[test]
fn without_a_store_an_older_file_keeps_its_limit() {
    let fx = Fixture::new();
    let v = created(&fx, None);
    set_limit(&v, HIGH).unwrap();
    let older = fx.raw();
    set_limit(&v, 0).unwrap();
    drop(v);
    fx.restore_raw(&older);
    assert_eq!(reopened(&fx, None).1, HIGH);
}

/// Destroying the vault drops its commitment; a new vault starts clean.
#[test]
fn a_destroyed_vault_takes_its_commitment_with_it() {
    let fx = Fixture::new();
    let store = Arc::new(Store::default());
    let v = open(&fx, Some(&store));
    v.create(Some(PASS)).unwrap();
    set_limit(&v, HIGH).unwrap();
    let id = v.vault_id().unwrap();
    assert!(store.committed(&id).is_some());
    v.destroy(Credential::Passphrase(PASS)).unwrap();
    assert_eq!(store.committed(&id), None);
    v.create(Some(PASS)).unwrap();
    assert_eq!(
        v.quick_unlock_policy().spend_limit_duffs,
        DEFAULT_QUICK_UNLOCK_SPEND_LIMIT
    );
}
