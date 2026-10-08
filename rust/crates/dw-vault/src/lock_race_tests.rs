//! `lock()` against signer calls already running (review DW-E0-03 B1/m1,
//! r2 M2): once `lock()` has returned, no signature, shared secret,
//! ciphertext or exported key of the old epoch is still being made, and none
//! is released.
//!
//! The barrier tests pause an operation inside the gate with the
//! [`test_hook`] (`Opened`, once it passed its epoch check). The stress tests
//! check the vault's own log of releases and epoch changes
//! ([`GateEvent`]), kept in the order of the mutex both hold, instead of
//! timing.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
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
use crate::vault::GateEvent;
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

/// The signers of one round, all issued in the same epoch.
struct Round {
    crypto: VaultSigner,
    identity: VaultSigner,
    spend: VaultSigner,
    /// The mixing signer; on a locked vault, which has none, the spend
    /// signer (it signs the same CoinJoin path).
    mixing: VaultSigner,
    scan: ScanKey,
    key_data: [u8; 33],
}

/// The vault states the stress test runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Unencrypted: the data key comes from the OS store.
    Unencrypted,
    /// Encrypted and unlocked with scope Full before each round.
    Unlocked,
    /// Encrypted and locked: every signer carries its passphrase grant's
    /// own copy of the data key.
    LockedOwnKey,
}

impl Mode {
    fn vault(self, dir: &tempfile::TempDir) -> Vault {
        let v = vault(dir, (self != Mode::Unencrypted).then_some(PASS));
        if self == Mode::LockedOwnKey {
            v.lock();
        }
        v
    }

    /// The signers of a new round (the vault was locked by the last one).
    fn round(self, v: &Vault) -> Round {
        let credential = match self {
            Mode::Unencrypted => Credential::None,
            Mode::Unlocked => {
                v.unlock(PASS, UnlockScope::Full).unwrap();
                Credential::None
            }
            Mode::LockedOwnKey => Credential::Passphrase(PASS),
        };
        let identity = platform_signer(v, SignerScope::PlatformIdentity, credential);
        let spend = v
            .signer(
                &W,
                &token(v, GrantPurpose::Spend { max_duffs: 1 }, credential),
            )
            .unwrap();
        let (crypto, mixing) = match self {
            Mode::LockedOwnKey => (
                platform_signer(v, SignerScope::DashPayCrypto, credential),
                spend.clone(),
            ),
            _ => (
                v.dashpay_crypto_signer(&W).unwrap(),
                v.mixing_signer(&W).unwrap(),
            ),
        };
        Round {
            key_data: identity_public_key(&identity),
            crypto,
            identity,
            spend,
            mixing,
            scan: v
                .scan_key(&W, &token(v, GrantPurpose::PlatformOp, credential))
                .unwrap(),
        }
    }
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

/// What [`check_gate_log`] counted.
#[derive(Debug, Default)]
struct GateStats {
    released: usize,
    /// Results released between a `lock()` call and its epoch change: the
    /// operations that lock had to wait for.
    released_while_lock_waited: usize,
    locks: usize,
}

/// Checks the release rule on the vault's own log, which records every
/// release and every epoch change in the order of the mutex both hold
/// (vault module doc, "Release"): the epoch is rebuilt from the change
/// events alone, and every result must be released under the epoch it
/// started in, before the change that ends it.
fn check_gate_log(log: &[GateEvent]) -> GateStats {
    let Some(&GateEvent::Epoch(mut epoch)) = log.first() else {
        panic!("the log starts with the epoch");
    };
    let mut stats = GateStats::default();
    let mut lock_waiting = false;
    for (i, event) in log.iter().enumerate().skip(1) {
        match *event {
            GateEvent::Epoch(next) => {
                assert!(next > epoch, "event {i}: the epoch went back");
                epoch = next;
                lock_waiting = false;
            }
            GateEvent::Opened(started) => {
                assert_eq!(started, epoch, "event {i}: opened under an ended epoch");
            }
            GateEvent::Released(started) => {
                assert_eq!(
                    started, epoch,
                    "event {i}: a result of epoch {started} was released in epoch {epoch}"
                );
                stats.released += 1;
                stats.released_while_lock_waited += usize::from(lock_waiting);
            }
            GateEvent::LockCalled => {
                lock_waiting = true;
                stats.locks += 1;
            }
        }
    }
    stats
}

/// Many threads call every kind of signer operation while `lock()` runs
/// round after round (review DW-E0-03 r2 M2). The vault logs each release
/// and epoch change in the order of the mutex both take; no result may be
/// released under an epoch a lock has ended, and every result a caller got
/// is one of those releases.
fn no_result_is_released_after_its_epoch(mode: Mode) {
    const ROUNDS: usize = 300;
    const WORKERS: usize = 16;

    let dir = tempfile::tempdir().unwrap();
    let v = mode.vault(&dir);
    let current: Arc<Mutex<Option<Arc<Round>>>> = Arc::default();
    let barrier = Arc::new(Barrier::new(WORKERS + 1));
    let results = Arc::new(AtomicUsize::new(0));
    let refused = Arc::new(AtomicUsize::new(0));

    let workers: Vec<_> = (0..WORKERS)
        .map(|w| {
            let (current, barrier, results, refused) = (
                current.clone(),
                barrier.clone(),
                results.clone(),
                refused.clone(),
            );
            thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                for _ in 0..ROUNDS {
                    barrier.wait();
                    let round = current.lock().unwrap().clone().unwrap();
                    for n in w.. {
                        match call(&rt, &round, n) {
                            Ok(()) => results.fetch_add(1, SeqCst),
                            Err(SignerError::Locked) => {
                                refused.fetch_add(1, SeqCst);
                                break;
                            }
                            Err(e) => panic!("{mode:?}, call {n}: {e}"),
                        };
                    }
                    barrier.wait();
                }
            })
        })
        .collect();

    v.start_gate_log();
    for index in 0..ROUNDS {
        *current.lock().unwrap() = Some(Arc::new(mode.round(&v)));
        barrier.wait();
        thread::sleep(Duration::from_micros(50 + (index as u64 * 37) % 400));
        v.lock();
        barrier.wait();
    }
    for w in workers {
        w.join().unwrap();
    }

    let stats = check_gate_log(&v.take_gate_log());
    let (results, refused) = (results.load(SeqCst), refused.load(SeqCst));
    eprintln!(
        "{mode:?}: {WORKERS} workers, {} locks: {results} results, {refused} refused Locked, \
         {} released ({} while lock() waited for them), 0 released under an ended epoch",
        stats.locks, stats.released, stats.released_while_lock_waited
    );
    assert_eq!(stats.locks, ROUNDS);
    // Each round's setup releases one result (the identity public key).
    assert_eq!(
        stats.released,
        results + ROUNDS,
        "every result a caller got is a logged release"
    );
    assert_eq!(
        refused,
        ROUNDS * WORKERS,
        "every worker ends its round Locked"
    );
    assert!(
        stats.released_while_lock_waited >= ROUNDS / 10,
        "only {} results were released while a lock waited: the race was not exercised",
        stats.released_while_lock_waited
    );
}

#[test]
fn no_result_is_released_after_its_epoch_unencrypted() {
    no_result_is_released_after_its_epoch(Mode::Unencrypted);
}

#[test]
fn no_result_is_released_after_its_epoch_unlocked() {
    no_result_is_released_after_its_epoch(Mode::Unlocked);
}

#[test]
fn no_result_is_released_after_its_epoch_with_grant_keys() {
    no_result_is_released_after_its_epoch(Mode::LockedOwnKey);
}

/// The release check refuses on its own: an epoch change that skips the gate
/// (which no production code does) between an operation's start and its
/// release drops the result, for a signer and a gated secret read.
#[test]
fn a_result_is_not_released_in_a_later_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, None);
    let crypto = v.dashpay_crypto_signer(&W).unwrap();
    let key = path(IDENTITY_KEY);
    v.start_gate_log();
    let r = crypto.run(&[(KeyUse::Agreement, &key)], |op| {
        let secret = op.with_key(&key, KeyUse::Agreement, |_, x| {
            crate::dip15::ecdh(&x.private_key, &peer())
        })?;
        v.bump_epoch_ungated();
        Ok(secret)
    });
    assert_eq!(r.map(drop), Err(SignerError::Locked));
    let r = v.gated(crate::VaultError::Locked, |_| {
        v.bump_epoch_ungated();
        Ok(())
    });
    assert_eq!(r, Err(crate::VaultError::Locked));
    let log = v.take_gate_log();
    assert!(
        !log.iter().any(|e| matches!(e, GateEvent::Released(_))),
        "{log:?}"
    );
    // The signer belongs to the ended epoch; a new one, and the gated read,
    // release when nothing changes the epoch.
    assert_eq!(
        crypto.ecdh_shared_secret(&key, &peer()).map(drop),
        Err(SignerError::Locked)
    );
    let crypto = v.dashpay_crypto_signer(&W).unwrap();
    v.start_gate_log();
    crypto.ecdh_shared_secret(&key, &peer()).unwrap();
    v.gated(crate::VaultError::Locked, |_| Ok(())).unwrap();
    assert_eq!(check_gate_log(&v.take_gate_log()).released, 2);
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
