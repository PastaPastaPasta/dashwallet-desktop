//! `lock()` against signer calls already running (review DW-E0-03 B1/m1):
//! once `lock()` has returned, no signature, shared secret, ciphertext or
//! exported key of the old epoch is still being made.
//!
//! The [`test_hook`] stamps each operation when it passes its epoch check
//! (`Opened`) and just before it releases the operation gate (`Closing`),
//! by which point its result exists.

use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use dashcore::secp256k1::{PublicKey, Secp256k1, SecretKey};
use key_wallet::bip32::DerivationPath;
use key_wallet::{Network, Signer};
use zeroize::Zeroizing;

use crate::signer::KeyUse;
use crate::signer::test_hook::{self, OpPoint};
use crate::{
    Credential, GrantPurpose, GrantToken, KdfParams, KdfPolicy, MemoryOsStore, ScanKey,
    SeedDerivation, SignerError, SignerScope, SystemClock, UnlockScope, Vault, VaultConfig,
    VaultSigner, WalletSecret, WalletSigner,
};

const W: [u8; 32] = [1; 32];
const PASS: &[u8] = b"pw";
const IDENTITY_KEY: &str = "m/9'/1'/5'/0'/0'/0'/0'";

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

fn vault(dir: &tempfile::TempDir, passphrase: Option<&[u8]>) -> Vault {
    let config = VaultConfig {
        kdf: KdfPolicy::Fixed(KdfParams::TEST),
        os_store: Arc::new(MemoryOsStore::new()),
        clock: Arc::new(SystemClock),
        grant_ttl_secs: 120,
    };
    let v = Vault::open(
        dir.path().join("vault"),
        Network::Regtest,
        "regtest",
        config,
    )
    .unwrap();
    v.create(passphrase).unwrap();
    let secret = WalletSecret {
        mnemonic: Zeroizing::new(b"unused".to_vec()),
        mnemonic_passphrase: Zeroizing::new(Vec::new()),
        seed: Zeroizing::new([0x5a; 64]),
        derivation: SeedDerivation::Bip39,
    };
    v.store_wallet_secret(&W, &secret).unwrap();
    v
}

/// A redeemed grant of `purpose` for the test wallet.
fn token(v: &Vault, purpose: GrantPurpose, credential: Credential<'_>) -> GrantToken {
    let grant = v.authorize(purpose, Some(&W), credential).unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&W)).unwrap()
}

fn platform_signer(v: &Vault, scope: SignerScope, credential: Credential<'_>) -> VaultSigner {
    let token = token(v, GrantPurpose::PlatformOp, credential);
    v.platform_signer(&W, &token, scope).unwrap()
}

fn spend_signer(v: &Vault) -> VaultSigner {
    let purpose = GrantPurpose::Spend { max_duffs: 1 };
    v.signer(&W, &token(v, purpose, Credential::None)).unwrap()
}

fn peer() -> PublicKey {
    PublicKey::from_secret_key(
        &Secp256k1::new(),
        &SecretKey::from_slice(&[0x42; 32]).unwrap(),
    )
}

/// The compressed public key of [`IDENTITY_KEY`] (its on-chain key data).
fn identity_public_key(identity: &VaultSigner) -> [u8; 33] {
    identity
        .with_key(&path(IDENTITY_KEY), KeyUse::PublicKey, |secp, x| {
            PublicKey::from_secret_key(secp, &x.private_key).serialize()
        })
        .unwrap()
}

/// Pauses the operation `call` on another thread inside the gate, runs the
/// epoch change `change` (a lock, a scope change) and checks that it does
/// not return until that operation has made its result, which is `Ok`.
fn epoch_change_waits_for<E: std::fmt::Debug + Send + 'static>(
    v: &Vault,
    change: impl FnOnce(&Vault) + Send + 'static,
    call: impl FnOnce() -> Result<(), E> + Send + 'static,
) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let worker = {
        thread::spawn(move || {
            test_hook::set(move |point| {
                if point == OpPoint::Opened {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
            });
            call()
        })
    };
    entered_rx.recv().unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changer = {
        let (v, changed) = (v.clone(), changed.clone());
        thread::spawn(move || {
            change(&v);
            changed.store(true, SeqCst);
        })
    };
    thread::sleep(Duration::from_millis(200));
    assert!(
        !changed.load(SeqCst),
        "the epoch changed while an operation was still inside the gate"
    );
    release_tx.send(()).unwrap();
    worker
        .join()
        .unwrap()
        .expect("the operation began before the epoch change");
    changer.join().unwrap();
    assert!(changed.load(SeqCst));
}

fn lock(v: &Vault) {
    v.lock();
}

#[test]
fn lock_waits_for_the_operation_already_running() {
    // The background crypto signer of an unlocked vault: ECDH.
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, Some(PASS));
    let crypto = v.dashpay_crypto_signer(&W).unwrap();
    let ecdh = move || {
        crypto
            .ecdh_shared_secret(&path(IDENTITY_KEY), &peer())
            .map(drop)
    };
    epoch_change_waits_for(&v, lock, ecdh.clone());
    assert_eq!(ecdh(), Err(SignerError::Locked));

    // A passphrase grant on a locked vault, whose signer holds the grant's
    // own copy of the data key: an identity signature.
    let identity = platform_signer(
        &v,
        SignerScope::PlatformIdentity,
        Credential::Passphrase(PASS),
    );
    let key_data = identity_public_key(&identity);
    let sign = move || {
        identity
            .sign_identity(&path(IDENTITY_KEY), &key_data, b"transition")
            .map(drop)
    };
    epoch_change_waits_for(&v, lock, sign.clone());
    assert_eq!(sign(), Err(SignerError::Locked));
}

/// The secret reads of the reveal paths hold the gate too: on an unlocked
/// vault (`reveal_mnemonic`) and with a passphrase grant's own key on a
/// locked one (`with_revealed_seed`, `export_wallet_secret`).
#[test]
fn lock_waits_for_a_secret_read_already_running() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, Some(PASS));
    let reveal_grant = |v: &Vault| {
        v.authorize(
            GrantPurpose::RevealSecret,
            Some(&W),
            Credential::Passphrase(PASS),
        )
        .unwrap()
        .id
    };

    let (id, w) = (reveal_grant(&v), v.clone());
    epoch_change_waits_for(&v, lock, move || w.reveal_mnemonic(&W, &id).map(drop));

    let (id, w) = (reveal_grant(&v), v.clone());
    epoch_change_waits_for(&v, lock, move || {
        w.with_revealed_seed(&W, &id, |seed| seed[0]).map(drop)
    });

    let token = token(&v, GrantPurpose::RevealSecret, Credential::Passphrase(PASS));
    let w = v.clone();
    epoch_change_waits_for(&v, lock, move || {
        w.export_wallet_secret(&W, &token).map(drop)
    });
}

/// One call a worker made: its round, its gate stamps (`None` when it never
/// passed the epoch check) and whether it returned a result.
struct Call {
    round: usize,
    stamps: Option<(u64, u64)>,
    ok: bool,
}

/// The signers of one round, all issued in the same epoch.
struct Round {
    index: usize,
    crypto: VaultSigner,
    identity: VaultSigner,
    spend: VaultSigner,
    mixing: VaultSigner,
    scan: ScanKey,
    key_data: [u8; 33],
}

/// One call of kind `n` on the round's signers.
fn call(rt: &tokio::runtime::Runtime, r: &Round, n: usize) -> Result<(), SignerError> {
    let identity_key = path(IDENTITY_KEY);
    let root = path("m/9'/1'/5'/0'/0'/0'/2'");
    match n % 8 {
        0 => r
            .crypto
            .ecdh_shared_secret(&identity_key, &peer())
            .map(drop),
        1 => r
            .identity
            .sign_identity(&identity_key, &r.key_data, b"transition")
            .map(drop),
        2 => r
            .crypto
            .account_reference(&identity_key, &[7; 69], 3, 0)
            .map(drop),
        3 => r
            .crypto
            .contact_info_seal(&root, 1, &[3; 32], b"alias", &[1; 16])
            .map(drop),
        4 => r
            .crypto
            .export_auto_accept_key(&path("m/9'/1'/16'/1900000000'"))
            .map(drop),
        5 => rt
            .block_on(r.spend.sign_message(&path("m/44'/1'/0'/0/0"), b"m"))
            .map(drop),
        6 => rt
            .block_on(r.mixing.sign_ecdsa(&path("m/9'/1'/4'/0'/0/1"), [9; 32]))
            .map(drop),
        _ => r.scan.master_key().map(drop),
    }
}

/// Many threads call every kind of signer operation while `lock()` runs
/// round after round. Every call that returned a result must have made it
/// (reached `Closing`) before the `lock()` ending its round returned.
#[test]
fn no_result_is_made_after_lock_returns() {
    const ROUNDS: usize = 300;
    const WORKERS: usize = 8;

    let dir = tempfile::tempdir().unwrap();
    // Unencrypted: each round's signers come without a KDF run.
    let v = vault(&dir, None);
    let clock = Arc::new(AtomicU64::new(0));
    let current: Arc<Mutex<Option<Arc<Round>>>> = Arc::default();
    let barrier = Arc::new(Barrier::new(WORKERS + 1));
    let calls: Arc<Mutex<Vec<Call>>> = Arc::default();

    let workers: Vec<_> = (0..WORKERS)
        .map(|w| {
            let (clock, current, barrier, calls) = (
                clock.clone(),
                current.clone(),
                barrier.clone(),
                calls.clone(),
            );
            thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                let stamps: Rc<RefCell<(Option<u64>, Option<u64>)>> = Rc::default();
                {
                    let (stamps, clock) = (stamps.clone(), clock.clone());
                    test_hook::set(move |point| {
                        let now = clock.fetch_add(1, SeqCst);
                        let mut s = stamps.borrow_mut();
                        match point {
                            OpPoint::Opened => s.0 = Some(now),
                            OpPoint::Closing => s.1 = Some(now),
                        }
                    });
                }
                let mut mine = Vec::new();
                for _ in 0..ROUNDS {
                    barrier.wait();
                    let round = current.lock().unwrap().clone().unwrap();
                    for n in w.. {
                        *stamps.borrow_mut() = (None, None);
                        let result = call(&rt, &round, n);
                        let (opened, closing) = *stamps.borrow();
                        mine.push(Call {
                            round: round.index,
                            stamps: opened.zip(closing),
                            ok: result.is_ok(),
                        });
                        match result {
                            Ok(()) => {}
                            Err(SignerError::Locked) => break,
                            Err(e) => panic!("call {n}: {e}"),
                        }
                    }
                    barrier.wait();
                }
                calls.lock().unwrap().extend(mine);
            })
        })
        .collect();

    let mut lock_calls = Vec::with_capacity(ROUNDS);
    let mut lock_returns = Vec::with_capacity(ROUNDS);
    for index in 0..ROUNDS {
        let identity = platform_signer(&v, SignerScope::PlatformIdentity, Credential::None);
        let key_data = identity_public_key(&identity);
        *current.lock().unwrap() = Some(Arc::new(Round {
            index,
            crypto: v.dashpay_crypto_signer(&W).unwrap(),
            identity,
            spend: spend_signer(&v),
            mixing: v.mixing_signer(&W).unwrap(),
            scan: v
                .scan_key(&W, &token(&v, GrantPurpose::PlatformOp, Credential::None))
                .unwrap(),
            key_data,
        }));
        barrier.wait();
        thread::sleep(Duration::from_micros(50 + (index as u64 * 37) % 400));
        lock_calls.push(clock.fetch_add(1, SeqCst));
        v.lock();
        lock_returns.push(clock.fetch_add(1, SeqCst));
        barrier.wait();
    }
    for w in workers {
        w.join().unwrap();
    }

    let calls = calls.lock().unwrap();
    let ok: Vec<_> = calls.iter().filter(|c| c.ok).collect();
    let late: Vec<_> = ok
        .iter()
        .filter(|c| c.stamps.is_none_or(|(_, end)| end > lock_returns[c.round]))
        .collect();
    // Calls that had passed their epoch check when lock() was called and
    // were still running: the ones lock() had to wait for.
    let raced = ok
        .iter()
        .filter(|c| {
            c.stamps.is_some_and(|(start, end)| {
                start < lock_calls[c.round] && end > lock_calls[c.round]
            })
        })
        .count();
    let refused = calls.iter().filter(|c| !c.ok).count();
    eprintln!(
        "{} calls: {} results, {} refused Locked, {raced} in flight when lock() was called, \
         {} results made after lock() returned",
        calls.len(),
        ok.len(),
        refused,
        late.len()
    );
    assert!(
        late.is_empty(),
        "{} results outlived their lock",
        late.len()
    );
    assert_eq!(
        refused,
        ROUNDS * WORKERS,
        "every worker ends its round Locked"
    );
    assert!(
        raced >= ROUNDS / 10,
        "only {raced} calls were in flight at a lock: the race was not exercised"
    );
}

/// A scope change (full unlock → mixing-only) ends the epoch as a lock does,
/// and also waits for the operation already running.
#[test]
fn a_scope_change_waits_for_the_operation_already_running() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, Some(PASS));
    let crypto = v.dashpay_crypto_signer(&W).unwrap();
    let ecdh = move || {
        crypto
            .ecdh_shared_secret(&path(IDENTITY_KEY), &peer())
            .map(drop)
    };
    epoch_change_waits_for(
        &v,
        |v| v.unlock(PASS, UnlockScope::MixingOnly).map(drop).unwrap(),
        ecdh.clone(),
    );
    assert_eq!(ecdh(), Err(SignerError::Locked));
}
