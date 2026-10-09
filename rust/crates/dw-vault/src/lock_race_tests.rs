//! `lock()` against signer calls already running (review DW-E0-03 B1/m1,
//! r2 M2): once `lock()` has returned, no signature, shared secret,
//! ciphertext or exported key of the old epoch is still being made, and none
//! is released.
//!
//! The barrier tests pause an operation inside the gate with the
//! [`test_hook`], once it passed its epoch check. The stress tests
//! check the vault's own log of releases and epoch changes
//! ([`GateEvent`]), kept in the order of the mutex both hold, instead of
//! timing.
//!
//! What each covers (review DW-E0-03 r3 n1, checked by mutation): the
//! stress tests verify the gate and the log reconstruction. With the gate
//! in place the release check never has a stale result to refuse, so they
//! cannot tell whether it works; `a_result_is_not_released_in_a_later_epoch`
//! covers the check on its own, by changing the epoch past the gate.
//!
//! The stress rounds meet at an abortable rendezvous ([`stress_rounds`],
//! review DW-E0-03 r4 n1): a worker that fails ends the run with its panic
//! message instead of leaving the others waiting at a barrier.

use std::any::Any;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use dashcore::secp256k1::{PublicKey, Secp256k1, SecretKey};
use key_wallet::bip32::DerivationPath;
use key_wallet::{Network, Signer};
use zeroize::Zeroizing;

use crate::os_store::OsSecretStore;
use crate::signer::KeyUse;
use crate::signer::test_hook;
use crate::vault::GateEvent;
use crate::{
    Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantPurpose, GrantToken, KdfParams, KdfPolicy,
    MemoryOsStore, QUICK_UNLOCK_SPEND_LIMITS, ScanKey, SeedDerivation, SignerError, SignerScope,
    SystemClock, UnlockScope, Vault, VaultConfig, VaultError, VaultSigner, WalletSecret,
    WalletSigner,
};

const W: [u8; 32] = [1; 32];
const PASS: &[u8] = b"pw";
const IDENTITY_KEY: &str = "m/9'/1'/5'/0'/0'/0'/0'";
const PLATFORM_OP: GrantPurpose = GrantPurpose::PlatformOp {
    max_duffs: 1,
    max_credits: 1,
};

fn path(s: &str) -> DerivationPath {
    DerivationPath::from_str(s).unwrap()
}

fn vault(dir: &tempfile::TempDir, passphrase: Option<&[u8]>) -> Vault {
    vault_with_store(dir, passphrase, Arc::new(MemoryOsStore::new()))
}

fn vault_with_store(
    dir: &tempfile::TempDir,
    passphrase: Option<&[u8]>,
    os_store: Arc<dyn OsSecretStore>,
) -> Vault {
    let config = VaultConfig {
        kdf: KdfPolicy::Fixed(KdfParams::TEST),
        os_store,
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
    let token = token(v, PLATFORM_OP, credential);
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
    let worker = thread::spawn(move || {
        test_hook::set(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        call()
    });
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
/// vault (`reveal_mnemonic`, `open_backup_bundle`) and with a passphrase
/// grant's own key on a locked one (`with_revealed_seed`,
/// `export_wallet_secret`).
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

    // Opening this vault's own backup bundle with its key, under a reveal
    // grant (review DW-E0-03 r3). Writing the bundle needs the vault
    // unlocked.
    v.unlock(PASS, UnlockScope::Full).unwrap();
    let bundle = v.backup_bundle(&W, None, b"payload", b"header").unwrap();
    let (id, w) = (reveal_grant(&v), v.clone());
    epoch_change_waits_for(&v, lock, move || {
        w.open_backup_bundle(&bundle, None, Some(&id), b"header")
            .map(drop)
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
                .scan_key(&W, &token(v, GrantPurpose::IdentityScan, credential))
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
    refused: usize,
    /// Results released between a `lock()` call and its epoch change: the
    /// operations that lock had to wait for.
    released_while_lock_waited: usize,
    locks: usize,
}

/// Checks the release rule on the vault's own log, which records every
/// release decision and every epoch change in the order of the mutex both
/// hold (vault module doc, "Release"): the epoch is rebuilt from the change
/// events alone; every result released must have started in that epoch
/// (released before the change that ends it), and every result refused
/// must have started in an earlier one.
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
            GateEvent::Refused(started) => {
                assert!(
                    started < epoch,
                    "event {i}: a result of the current epoch {epoch} was refused"
                );
                stats.refused += 1;
            }
            GateEvent::LockCalled => {
                lock_waiting = true;
                stats.locks += 1;
            }
        }
    }
    stats
}

/// How long a stress participant waits at a rendezvous for the others. A
/// round takes well under a millisecond; this only bounds a hang.
const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(60);

/// The round rendezvous of [`stress_rounds`] (review DW-E0-03 r3/r4 n1).
/// Unlike [`std::sync::Barrier`], one participant's failure releases every
/// other: [`Rendezvous::abort`] (called by a panicking participant's
/// [`AbortOnPanic`] guard, or by a wait that outlasts
/// [`RENDEZVOUS_TIMEOUT`]) wakes every waiter, and every wait from then on
/// returns the reason as an error.
struct Rendezvous {
    parties: usize,
    state: Mutex<RendezvousState>,
    arrived: Condvar,
}

#[derive(Default)]
struct RendezvousState {
    waiting: usize,
    generation: u64,
    aborted: Option<String>,
}

impl Rendezvous {
    fn new(parties: usize) -> Self {
        Self {
            parties,
            state: Mutex::default(),
            arrived: Condvar::new(),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, RendezvousState> {
        // A participant that panicked holds no guard across a panic point,
        // so the state is consistent even when the mutex is poisoned.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until every party arrived, or returns why the rendezvous was
    /// aborted.
    fn wait(&self) -> Result<(), String> {
        let mut state = self.lock_state();
        if let Some(why) = &state.aborted {
            return Err(why.clone());
        }
        state.waiting += 1;
        if state.waiting == self.parties {
            state.waiting = 0;
            state.generation += 1;
            self.arrived.notify_all();
            return Ok(());
        }
        let generation = state.generation;
        let deadline = Instant::now() + RENDEZVOUS_TIMEOUT;
        loop {
            if state.generation != generation {
                return Ok(());
            }
            if let Some(why) = &state.aborted {
                return Err(why.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                drop(state);
                let why = format!(
                    "a stress participant waited {RENDEZVOUS_TIMEOUT:?} at a rendezvous \
                     for the others"
                );
                self.abort(why.clone());
                return Err(why);
            }
            state = self
                .arrived
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Ends the rendezvous for every party: current and later waits return
    /// `Err`. The first reason is kept.
    fn abort(&self, why: String) {
        self.lock_state().aborted.get_or_insert(why);
        self.arrived.notify_all();
    }
}

/// Aborts the rendezvous if its thread unwinds while holding it.
struct AbortOnPanic<'a>(&'a Rendezvous, &'static str);

impl Drop for AbortOnPanic<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.abort(format!("the {} panicked", self.1));
        }
    }
}

/// The text of a panic payload.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a non-text panic".to_owned())
}

/// Runs `rounds` rounds of `workers` threads against a coordinator. Each
/// round: the coordinator calls `setup(index)`; every worker then runs its
/// round body (made once per worker by `make_worker(w)`) while the
/// coordinator calls `during(index)`; the round ends when all of them have
/// returned.
///
/// A participant that fails cannot hang the others (review DW-E0-03 r4 n1):
/// a panic aborts the [`Rendezvous`], which releases every waiter, and the
/// test then panics with the reason and each worker's panic message. A
/// participant stuck elsewhere aborts it after [`RENDEZVOUS_TIMEOUT`].
///
/// The join is bounded too (review m-4): a worker stuck inside its round
/// body never reaches a rendezvous, so after the run the harness waits at
/// most `join_grace` for every worker to finish, then panics naming the
/// ones still running instead of joining them.
fn stress_rounds<F>(
    join_grace: Duration,
    workers: usize,
    rounds: usize,
    make_worker: impl Fn(usize) -> F,
    mut setup: impl FnMut(usize),
    mut during: impl FnMut(usize),
) where
    F: FnMut() + Send + 'static,
{
    let rendezvous = Arc::new(Rendezvous::new(workers + 1));
    let handles: Vec<_> = (0..workers)
        .map(|w| {
            let rendezvous = rendezvous.clone();
            let mut body = make_worker(w);
            thread::spawn(move || {
                let _abort = AbortOnPanic(&rendezvous, "stress worker");
                for _ in 0..rounds {
                    if rendezvous.wait().is_err() {
                        return;
                    }
                    body();
                    if rendezvous.wait().is_err() {
                        return;
                    }
                }
            })
        })
        .collect();

    let coordinated = {
        let _abort = AbortOnPanic(&rendezvous, "stress coordinator");
        (0..rounds).try_for_each(|index| {
            setup(index);
            rendezvous.wait()?;
            during(index);
            rendezvous.wait()
        })
    };
    let deadline = Instant::now() + join_grace;
    while handles.iter().any(|h| !h.is_finished()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let mut worker_panics = Vec::new();
    let mut still_running = Vec::new();
    for (w, h) in handles.into_iter().enumerate() {
        if !h.is_finished() {
            // Left detached: joining it would hang the test.
            still_running.push(w);
        } else if let Err(payload) = h.join() {
            worker_panics.push(format!("worker {w}: {}", panic_text(&*payload)));
        }
    }
    let outcome = format!(
        "worker panics: {worker_panics:?}; workers still running after {join_grace:?}: \
         {still_running:?}"
    );
    if let Err(why) = coordinated {
        panic!("stress rounds aborted: {why}; {outcome}");
    }
    assert!(
        worker_panics.is_empty() && still_running.is_empty(),
        "stress workers failed: {outcome}"
    );
}

/// How long [`stress_rounds`] waits for its workers to finish after the
/// last round or an abort.
const JOIN_GRACE: Duration = Duration::from_secs(10);

/// The harness fails fast when a worker fails, in the first round or a
/// later one (review DW-E0-03 r4 n1): the stress run ends with a panic
/// that names the worker's failure, rather than leaving the other
/// participants waiting at a barrier forever. A worker stuck inside its
/// round body (review m-4) does not hang the join either: the run panics
/// after the join grace, naming it.
#[test]
fn a_failing_stress_worker_fails_the_run_instead_of_hanging() {
    let release = Arc::new(AtomicBool::new(false));
    for (failing_round, stuck_worker) in [(0, None), (3, None), (2, Some(3))] {
        let (sender, outcome) = mpsc::channel();
        let release = release.clone();
        thread::spawn(move || {
            let run = std::panic::catch_unwind(|| {
                stress_rounds(
                    Duration::from_millis(200),
                    4,
                    10,
                    |w| {
                        let (mut round, release) = (0, release.clone());
                        move || {
                            if w == 2 && round == failing_round {
                                panic!("injected failure in round {round}");
                            }
                            if Some(w) == stuck_worker && round == failing_round {
                                while !release.load(SeqCst) {
                                    thread::sleep(Duration::from_millis(1));
                                }
                            }
                            round += 1;
                        }
                    },
                    |_| {},
                    |_| thread::sleep(Duration::from_millis(1)),
                )
            });
            let _ = sender.send(run.map_err(|payload| panic_text(&*payload)));
        });
        let run = outcome
            .recv_timeout(Duration::from_secs(10))
            .expect("the stress harness hung after a worker failed");
        let message = run.expect_err("a failing worker must fail the run");
        assert!(
            message.contains(&format!(
                "worker 2: injected failure in round {failing_round}"
            )),
            "{message}"
        );
        assert!(message.contains("the stress worker panicked"), "{message}");
        let still_running = match stuck_worker {
            Some(w) => format!("still running after 200ms: [{w}]"),
            None => "still running after 200ms: []".to_owned(),
        };
        assert!(message.contains(&still_running), "{message}");
    }
    // Let the stuck worker end.
    release.store(true, SeqCst);
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
    let results = Arc::new(AtomicUsize::new(0));
    let refused = Arc::new(AtomicUsize::new(0));

    v.start_gate_log();
    stress_rounds(
        JOIN_GRACE,
        WORKERS,
        ROUNDS,
        |w| {
            let (current, results, refused) = (current.clone(), results.clone(), refused.clone());
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            move || {
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
            }
        },
        |_| *current.lock().unwrap() = Some(Arc::new(mode.round(&v))),
        |index| {
            thread::sleep(Duration::from_micros(50 + (index as u64 * 37) % 400));
            v.lock();
        },
    );

    let stats = check_gate_log(&v.take_gate_log());
    let (results, refused) = (results.load(SeqCst), refused.load(SeqCst));
    eprintln!(
        "{mode:?}: {WORKERS} workers, {} locks: {results} results, {refused} refused Locked, \
         {} released ({} while lock() waited for them), 0 released under an ended epoch, \
         {} refused by the release check",
        stats.locks, stats.released, stats.released_while_lock_waited, stats.refused
    );
    assert_eq!(stats.locks, ROUNDS);
    // The gate makes lock() wait for running operations, so the release
    // check never has a stale result to refuse.
    assert_eq!(stats.refused, 0);
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
    let stats = check_gate_log(&v.take_gate_log());
    assert_eq!((stats.released, stats.refused), (0, 2));
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

/// How long a test waits for a thread that should not block before it calls
/// that a hang.
const NO_HANG: Duration = Duration::from_secs(10);

/// One token operation of review DW-E0-03 r3 m2, set up on a fresh vault:
/// the call, and whether the vault still shows it undone.
struct TokenOp {
    name: &'static str,
    _dir: tempfile::TempDir,
    vault: Vault,
    call: Box<dyn FnOnce() -> Result<(), VaultError> + Send>,
    undone: fn(&Vault) -> bool,
}

/// The four operations that use a grant token's key outside a signer
/// (wipe, encrypt, quick-unlock enrolment and its spending limit), each
/// with the vault's key and, where the vault can be locked, with a
/// passphrase grant's own key on a locked vault.
fn token_ops() -> Vec<TokenOp> {
    let limit = *QUICK_UNLOCK_SPEND_LIMITS
        .iter()
        .find(|&&l| l != DEFAULT_QUICK_UNLOCK_SPEND_LIMIT)
        .unwrap();
    let change_credential = |v: &Vault, credential| {
        v.authorize(GrantPurpose::ChangeCredential, None, credential)
            .unwrap()
            .id
    };
    let mut ops = Vec::new();
    for locked in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let v = vault(&dir, Some(PASS));
        if locked {
            v.lock();
        }
        let t = token(&v, GrantPurpose::Wipe, Credential::Passphrase(PASS));
        let w = v.clone();
        ops.push(TokenOp {
            _dir: dir,
            name: if locked {
                "wipe, grant key"
            } else {
                "wipe, vault key"
            },
            vault: v,
            call: Box::new(move || w.wipe_wallet_secret(&W, &t).map(drop)),
            undone: |v| v.has_wallet_secret(&W),
        });

        let dir = tempfile::tempdir().unwrap();
        let v = vault(&dir, Some(PASS));
        if locked {
            v.lock();
        }
        let (id, w) = (
            change_credential(&v, Credential::Passphrase(PASS)),
            v.clone(),
        );
        ops.push(TokenOp {
            _dir: dir,
            name: if locked {
                "enroll, grant key"
            } else {
                "enroll, vault key"
            },
            vault: v,
            call: Box::new(move || w.enroll_quick_unlock(&id).map(drop)),
            undone: |v| !v.quick_unlock_policy().enrolled,
        });

        let dir = tempfile::tempdir().unwrap();
        let v = vault(&dir, Some(PASS));
        if locked {
            v.lock();
        }
        let (id, w) = (
            change_credential(&v, Credential::Passphrase(PASS)),
            v.clone(),
        );
        ops.push(TokenOp {
            _dir: dir,
            name: if locked {
                "spend limit, grant key"
            } else {
                "spend limit, vault key"
            },
            vault: v,
            call: Box::new(move || w.set_quick_unlock_spend_limit(&id, limit).map(drop)),
            undone: |v| {
                v.quick_unlock_policy().spend_limit_duffs == DEFAULT_QUICK_UNLOCK_SPEND_LIMIT
            },
        });
    }
    // Unencrypted (the data key reloaded from the OS store): wipe and
    // encrypt, which needs an unencrypted vault.
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, None);
    let t = token(&v, GrantPurpose::Wipe, Credential::None);
    let w = v.clone();
    ops.push(TokenOp {
        _dir: dir,
        name: "wipe, unencrypted",
        vault: v,
        call: Box::new(move || w.wipe_wallet_secret(&W, &t).map(drop)),
        undone: |v| v.has_wallet_secret(&W),
    });
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, None);
    let (id, w) = (change_credential(&v, Credential::None), v.clone());
    ops.push(TokenOp {
        _dir: dir,
        name: "encrypt",
        vault: v,
        call: Box::new(move || w.encrypt(b"new passphrase", &id).map(drop)),
        undone: |v| !v.status().encrypted,
    });
    ops
}

/// Review DW-E0-03 r3 m2: a `lock()` that lands after a token operation
/// checked (or redeemed) its token and before it loaded the key returns at
/// once, and the operation is then refused `Locked` with nothing changed.
/// Before the fix the wipe had checked its token at this point, the lock
/// returned, and the wipe still deleted the wallet's records; so only the
/// wipe cases fail on the old code. Encrypt, enrolment and the spending
/// limit pause before any token check here, which the old code also made
/// later; what they lacked (the gate) is what
/// `lock_waits_for_a_token_operation_already_running` covers.
#[test]
fn a_lock_before_the_key_load_refuses_each_token_operation() {
    for op in token_ops() {
        assert!((op.undone)(&op.vault), "{}: set up undone", op.name);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let call = op.call;
        let worker = thread::spawn(move || {
            test_hook::set_checked(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            call()
        });
        entered_rx
            .recv_timeout(NO_HANG)
            .unwrap_or_else(|_| panic!("{}: the token was not checked", op.name));
        let (locked_tx, locked_rx) = mpsc::channel();
        let locker = {
            let v = op.vault.clone();
            thread::spawn(move || {
                v.lock();
                locked_tx.send(()).unwrap();
            })
        };
        let lock_returned = locked_rx.recv_timeout(NO_HANG).is_ok();
        release_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        locker.join().unwrap();
        assert!(lock_returned, "{}: lock() waited before the gate", op.name);
        assert_eq!(result, Err(VaultError::Locked), "{}", op.name);
        assert!((op.undone)(&op.vault), "{}: changed after lock()", op.name);
    }
}

/// Review DW-E0-03 r3 m2: a `lock()` that lands while a token operation is
/// inside the gate (its key loaded) waits until the operation is done, so
/// it is never still running once `lock()` has returned.
#[test]
fn lock_waits_for_a_token_operation_already_running() {
    for op in token_ops() {
        epoch_change_waits_for(&op.vault, lock, op.call);
        assert!(!(op.undone)(&op.vault), "{}: done before lock()", op.name);
    }
}

/// The gated reads that use the vault's own key, which an unencrypted vault
/// loads from the OS store first.
type Read = fn(&Vault) -> Result<(), VaultError>;
const KEY_LOADING_READS: [(&str, Read); 2] = [
    ("seed_derivation", |v| v.seed_derivation(&W).map(drop)),
    ("core_mnemonic_check", |v| {
        v.core_mnemonic_check(&W).map(drop)
    }),
];

/// An OS store whose next `get` blocks until the test releases it, as a
/// keyring waiting on its unlock prompt would.
#[derive(Default)]
struct SlowStore {
    inner: MemoryOsStore,
    armed: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl SlowStore {
    /// Makes the next `get` block; returns (entered, release).
    fn arm(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *self.armed.lock().unwrap() = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }
}

impl OsSecretStore for SlowStore {
    fn put(&self, service: &[u8; 32], label: &str, secret: &[u8]) -> Result<(), VaultError> {
        self.inner.put(service, label, secret)
    }

    fn get(
        &self,
        service: &[u8; 32],
        label: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        let armed = self.armed.lock().unwrap().take();
        if let Some((entered, release)) = armed {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        self.inner.get(service, label)
    }

    fn delete(&self, service: &[u8; 32], label: &str) -> Result<bool, VaultError> {
        self.inner.delete(service, label)
    }

    fn name(&self) -> &'static str {
        "slow"
    }
}

/// Review DW-E0-03 r3 m1: the gated reads that use an unencrypted vault's
/// own key load it from the OS store before they enter the gate, so a
/// `lock()` does not wait on a keyring that blocks. Before the fix it waited
/// for as long as the read was held.
#[test]
fn lock_does_not_wait_for_an_os_store_read() {
    for (name, read) in KEY_LOADING_READS {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SlowStore::default());
        let v = vault_with_store(&dir, None, store.clone());
        // Drops the cached key, so the next read goes to the OS store.
        v.lock();
        let (entered, release) = store.arm();
        let worker = {
            let v = v.clone();
            thread::spawn(move || read(&v))
        };
        entered
            .recv_timeout(NO_HANG)
            .unwrap_or_else(|_| panic!("{name}: the key was not read from the OS store"));
        let (locked_tx, locked_rx) = mpsc::channel();
        let locker = {
            let v = v.clone();
            thread::spawn(move || {
                v.lock();
                locked_tx.send(()).unwrap();
            })
        };
        let lock_returned = locked_rx.recv_timeout(NO_HANG).is_ok();
        release.send(()).unwrap();
        let result = worker.join().unwrap();
        locker.join().unwrap();
        assert!(lock_returned, "{name}: lock() waited for the OS store");
        // An unencrypted vault reloads its key on demand; the read then
        // runs in the new epoch.
        assert_eq!(result, Ok(()), "{name}");
    }
}

/// Review DW-E0-03 r3 m1 (follow-up): a `lock()` between the OS-store key
/// load and the gate drops the key again. An unencrypted vault's read then
/// loads it again and succeeds, rather than failing with a spurious
/// `Locked` the caller could do nothing about.
#[test]
fn a_lock_between_the_key_load_and_the_gate_reloads_the_key() {
    for (name, read) in KEY_LOADING_READS {
        let dir = tempfile::tempdir().unwrap();
        let v = vault(&dir, None);
        v.lock();
        let (loaded_tx, loaded_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let worker = {
            let v = v.clone();
            thread::spawn(move || {
                let mut first = true;
                test_hook::set_checked(move || {
                    if std::mem::take(&mut first) {
                        loaded_tx.send(()).unwrap();
                        go_rx.recv().unwrap();
                    }
                });
                read(&v)
            })
        };
        loaded_rx
            .recv_timeout(NO_HANG)
            .unwrap_or_else(|_| panic!("{name}: the key was not loaded"));
        v.lock();
        go_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Ok(()), "{name}");
    }
}

/// A redeemed `PlatformOp` token of a passphrase grant on the locked vault
/// `v`, its key moved into a hold, and an identity signer on that hold
/// (E0-04 design §3.5).
fn held_identity(v: &Vault) -> (crate::KeyHold, VaultSigner) {
    let mut tokens = vec![token(v, PLATFORM_OP, Credential::Passphrase(PASS))];
    let hold = v.hold_key(&mut tokens).unwrap();
    let signer = v
        .platform_signer_held(&W, &hold, &tokens[0], SignerScope::PlatformIdentity)
        .unwrap();
    (hold, signer)
}

/// Dropping a `KeyHold` erases the key while signer clones are still held
/// (no strong reference is left), and an operation already under way
/// finishes with the copy it took inside the gate.
#[test]
fn dropping_a_key_hold_erases_the_key_and_lets_a_running_operation_finish() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, Some(PASS));
    v.lock();
    let (hold, identity) = held_identity(&v);
    let key_data = identity_public_key(&identity);
    let weak = Arc::downgrade(&hold.key);
    let clones: Vec<_> = (0..4).map(|_| identity.clone()).collect();

    // Pause a signature inside the gate, drop the hold, then let it finish.
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let running = clones[0].clone();
    let worker = thread::spawn(move || {
        test_hook::set(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        running
            .sign_identity(&path(IDENTITY_KEY), &key_data, b"transition")
            .map(drop)
    });
    entered_rx.recv().unwrap();
    drop(hold);
    assert_eq!(
        weak.strong_count(),
        0,
        "the key outlived its hold: a signer kept a strong reference"
    );
    release_tx.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap(),
        Ok(()),
        "the running operation finishes"
    );

    for s in clones.iter().chain([&identity]) {
        assert_eq!(
            s.sign_identity(&path(IDENTITY_KEY), &key_data, b"transition")
                .map(drop),
            Err(SignerError::Locked)
        );
    }
}

/// 16 threads sign with clones of one held signer while the coordinator
/// drops the hold, round after round (300 drops). No call that began after
/// the drop returned succeeds, and every worker ends its round `Locked`.
#[test]
fn no_signature_begins_after_its_key_hold_dropped() {
    const ROUNDS: usize = 300;
    const WORKERS: usize = 16;

    struct HeldRound {
        hold: Mutex<Option<crate::KeyHold>>,
        identity: VaultSigner,
        key_data: [u8; 33],
        dropped: AtomicBool,
    }

    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir, Some(PASS));
    v.lock();
    let current: Arc<Mutex<Option<Arc<HeldRound>>>> = Arc::default();
    let results = Arc::new(AtomicUsize::new(0));
    let across_drop = Arc::new(AtomicUsize::new(0));
    let refused = Arc::new(AtomicUsize::new(0));

    stress_rounds(
        JOIN_GRACE,
        WORKERS,
        ROUNDS,
        |_| {
            let (current, results, across_drop, refused) = (
                current.clone(),
                results.clone(),
                across_drop.clone(),
                refused.clone(),
            );
            move || {
                let round = current.lock().unwrap().clone().unwrap();
                // Each worker signs with its own clone of the held signer.
                let signer = round.identity.clone();
                loop {
                    let dropped_before = round.dropped.load(SeqCst);
                    match signer.sign_identity(&path(IDENTITY_KEY), &round.key_data, b"transition")
                    {
                        Ok(_) => {
                            assert!(
                                !dropped_before,
                                "a signature began after its hold had dropped"
                            );
                            results.fetch_add(1, SeqCst);
                            if round.dropped.load(SeqCst) {
                                across_drop.fetch_add(1, SeqCst);
                            }
                        }
                        Err(SignerError::Locked) => {
                            refused.fetch_add(1, SeqCst);
                            break;
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
            }
        },
        |_| {
            let (hold, identity) = held_identity(&v);
            *current.lock().unwrap() = Some(Arc::new(HeldRound {
                hold: Mutex::new(Some(hold)),
                key_data: identity_public_key(&identity),
                identity,
                dropped: AtomicBool::new(false),
            }));
        },
        |index| {
            thread::sleep(Duration::from_micros(50 + (index as u64 * 37) % 400));
            let round = current.lock().unwrap().clone().unwrap();
            drop(round.hold.lock().unwrap().take());
            round.dropped.store(true, SeqCst);
        },
    );

    let (results, across_drop, refused) = (
        results.load(SeqCst),
        across_drop.load(SeqCst),
        refused.load(SeqCst),
    );
    eprintln!(
        "{WORKERS} workers, {ROUNDS} hold drops: {results} signatures \
         ({across_drop} finished after their hold dropped), {refused} refused Locked"
    );
    assert_eq!(
        refused,
        ROUNDS * WORKERS,
        "every worker ends its round Locked"
    );
    assert!(
        results > 0,
        "no signature was made: the race was not exercised"
    );
}
