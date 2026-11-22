//! H-03, the kill -9 matrix (DESIGN G5) for a plain Core send, through a
//! real process kill. A child process (this test binary, running
//! [`core_send_child`]) funds a wallet offline, prepares a payment and
//! stops at one named point of it; the parent sends it SIGKILL, opens the
//! same data directory again and checks the state it finds within 60 s.
//! The hand-off goes through the draft's test broadcaster: no network.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::{Address, OutPoint, Transaction, Txid};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

use super::own_spend_disk_tests::{
    COIN, COINS, FOREIGN, FUNDED_AT, coins, fresh_address, funding, wait_stored,
};
use super::{BIRTH, PHRASE, Recorder, balances, mempool, mined, open, rows};
use crate::send::{CoinSource, Recipient};
use crate::{Engine, EngineConfig, NetworkSession, WalletId};

const ROOT: &str = "DWD_H03_KILL9_ROOT";
const POINT: &str = "DWD_H03_KILL9_POINT";
const CHILD: &str = "mempool_sync_tests::h03_kill9_core_tests::core_send_child";
const PASS: &[u8] = b"h03 kill9 passphrase";
const AMOUNT: u64 = COIN * 5 / 2;
/// ROADMAP H-03's acceptance: equal state at most 60 s after the restart.
const WITHIN: Duration = Duration::from_secs(60);
/// How long the child may take to reach its point (a cold debug build
/// opens the engine slowly).
const REACH: Duration = Duration::from_secs(180);

/// Where the child stops, in the order a payment passes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    /// Signed and reserved by `prepare`; no broadcast asked for.
    Signed,
    /// `broadcast` wrote the sent mark (#88), the hand-off not entered.
    Marked,
    /// Handed to the network; the wallet has not stored it.
    Handed,
    /// Handed, and the wallet stored it as dash-spv's relay delivers it;
    /// the hand-off's outcome not stored.
    Stored,
}

impl Point {
    fn name(self) -> &'static str {
        match self {
            Point::Signed => "signed",
            Point::Marked => "marked",
            Point::Handed => "handed",
            Point::Stored => "stored",
        }
    }

    fn parse(s: &str) -> Self {
        [Point::Signed, Point::Marked, Point::Handed, Point::Stored]
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or_else(|| panic!("no kill point {s:?}"))
    }
}

fn engine_at(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(Recorder::default()),
    )
    .unwrap()
}

/// The broadcaster of the child's payment: it parks the hand-off at the
/// child's point, so the process dies there.
struct Hang {
    point: Point,
    reached: AtomicBool,
    handed: Mutex<Option<Transaction>>,
}

impl crate::send::TestBroadcaster for Hang {
    fn before_entry(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let park = self.point == Point::Marked;
        if park {
            self.reached.store(true, Ordering::SeqCst);
        }
        Box::pin(async move {
            if park {
                std::future::pending::<()>().await;
            }
        })
    }

    fn broadcast(
        &self,
        tx: &Transaction,
        _first: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Txid, platform_wallet::PlatformWalletError>> + Send>>
    {
        *self.handed.lock().unwrap() = Some(tx.clone());
        self.reached.store(true, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

/// What the child left: the payment's txid and the coins it signed.
struct Report {
    wallet: WalletId,
    txid: Txid,
    inputs: BTreeSet<OutPoint>,
    funded: BTreeSet<OutPoint>,
}

fn outpoints(line: &str) -> BTreeSet<OutPoint> {
    (line.split(',').filter(|s| !s.is_empty()))
        .map(|s| OutPoint::from_str(s).unwrap())
        .collect()
}

fn joined(coins: &BTreeSet<OutPoint>) -> String {
    (coins.iter().map(OutPoint::to_string))
        .collect::<Vec<_>>()
        .join(",")
}

/// [`COINS`] coins of one DASH each, mined at [`FUNDED_AT`].
fn fund(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> BTreeSet<OutPoint> {
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(engine, session, id))
        .collect();
    let paid = funding(7, &to, COIN);
    mined(engine, session, id, FUNDED_AT, vec![paid.clone()]);
    (0..COINS as u32)
        .map(|v| OutPoint::new(paid.txid(), v))
        .collect()
}

/// Run by [`killed_at`] in a child process: a wallet funded offline, a
/// payment of [`AMOUNT`] taken to `DWD_H03_KILL9_POINT`; then it reports
/// and waits for its SIGKILL.
#[test]
#[ignore = "the child process of the H-03 Core send kill matrix"]
fn core_send_child() {
    let (Some(root), Ok(point)) = (std::env::var_os(ROOT), std::env::var(POINT)) else {
        return;
    };
    let (root, point) = (PathBuf::from(root), Point::parse(&point));
    let engine = engine_at(&root);
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.create(Some(PASS))))
        .unwrap();
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    let funded = fund(&engine, &session, id);
    assert_eq!(balances(&session, id).total, COINS as u64 * COIN);

    let hang = Arc::new(Hang {
        point,
        reached: AtomicBool::new(false),
        handed: Mutex::new(None),
    });
    let draft = session.new_tx_draft(id).unwrap();
    *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&hang) as _);
    draft
        .set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount: AMOUNT,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
    draft.set_source(CoinSource::Any).unwrap();
    let grant = (session.vault())
        .authorize(
            GrantPurpose::Spend {
                max_duffs: AMOUNT + COIN,
            },
            Some(&id.0),
            Credential::Passphrase(PASS),
        )
        .unwrap()
        .id;
    let prepared = engine.block_on(draft.prepare(grant)).unwrap();
    let txid = prepared.summary().txid.clone();
    let inputs: BTreeSet<OutPoint> = (prepared.summary().inputs.iter())
        .map(|i| i.outpoint)
        .collect();
    if point != Point::Signed {
        let (draft, prepared) = (Arc::clone(&draft), Arc::clone(&prepared));
        session.rt.spawn(async move {
            let _ = draft.broadcast(prepared).await;
        });
        let start = Instant::now();
        while !hang.reached.load(Ordering::SeqCst) {
            assert!(start.elapsed() < REACH, "never reached {point:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if point == Point::Stored {
        let tx = hang.handed.lock().unwrap().clone().unwrap();
        mempool(&engine, &session, id, &tx);
        wait_stored(&session, id, &[tx.txid()]);
    }
    let report = format!("{id}\n{txid}\n{}\n{}\n", joined(&inputs), joined(&funded));
    let tmp = root.join("reached.tmp");
    std::fs::write(&tmp, report).unwrap();
    std::fs::rename(&tmp, root.join("reached")).unwrap();
    loop {
        std::thread::park();
    }
}

/// Runs [`core_send_child`] to `point` in a child process, kills it with
/// SIGKILL once it reports, and returns its data root and report.
fn killed_at(point: Point) -> (tempfile::TempDir, Report) {
    let dir = dw_testutil::private_tempdir();
    let log = std::fs::File::create(dir.path().join("child.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([CHILD, "--exact", "--ignored", "--test-threads=1"])
        .env(ROOT, dir.path())
        .env(POINT, point.name())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let reached = dir.path().join("reached");
    let start = Instant::now();
    while !reached.exists() {
        let exited = child.try_wait().unwrap();
        if exited.is_some() || start.elapsed() > REACH {
            let _ = child.kill();
            let _ = child.wait();
            let log = std::fs::read_to_string(dir.path().join("child.log")).unwrap_or_default();
            panic!("the child never reached {point:?} ({exited:?}): {log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // `Child::kill` is SIGKILL on Unix: no destructor, close or flush runs.
    child.kill().unwrap();
    let status = child.wait().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    }
    let report = std::fs::read_to_string(&reached).unwrap();
    let lines: Vec<&str> = report.lines().collect();
    let report = Report {
        wallet: lines[0].parse().unwrap(),
        txid: Txid::from_str(lines[1]).unwrap(),
        inputs: outpoints(lines[2]),
        funded: outpoints(lines[3]),
    };
    assert!(!report.inputs.is_empty() && report.inputs.is_subset(&report.funded));
    (dir, report)
}

/// The engine of the next launch on the killed child's data root.
fn relaunch(dir: &Path) -> (Engine, Arc<NetworkSession>) {
    let engine = engine_at(dir);
    let session = open(&engine);
    (engine, session)
}

/// The wallet's free coins, its history's txids and its total.
fn state(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (BTreeSet<OutPoint>, BTreeSet<String>, u64) {
    let free = coins(engine, session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    let txids = (rows(engine, session, id).into_iter())
        .map(|r| r.0)
        .collect();
    (free, txids, balances(session, id).total)
}

/// Waits up to [`WITHIN`] for `equal`; the last state read otherwise.
fn within_60s<S: std::fmt::Debug>(mut read: impl FnMut() -> S, equal: impl Fn(&S) -> bool) -> S {
    let deadline = Instant::now() + WITHIN;
    loop {
        let s = read();
        if equal(&s) || Instant::now() >= deadline {
            return s;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn sent_marks(session: &NetworkSession, id: WalletId) -> Vec<(String, String)> {
    (session.live().unwrap().appdb)
        .settings_with_prefix(&id.to_string(), crate::tx_actions::SENT_PREFIX)
        .unwrap()
}

/// A kill before anything of the payment was handed off leaves the
/// wallet as it was before the payment: every coin free, nothing in the
/// history, the balance whole and, once the load ran, no sent mark.
fn nothing_left(point: Point) {
    let (dir, r) = killed_at(point);
    let (engine, session) = relaunch(dir.path());
    let whole = COINS as u64 * COIN;
    let funding_tx = r.funded.iter().next().unwrap().txid.to_string();
    let found = within_60s(
        || {
            (
                state(&engine, &session, r.wallet),
                sent_marks(&session, r.wallet),
            )
        },
        |((free, txids, total), marks)| {
            *free == r.funded
                && *txids == BTreeSet::from([funding_tx.clone()])
                && *total == whole
                && marks.is_empty()
        },
    );
    let ((free, txids, total), marks) = found;
    assert_eq!(free, r.funded, "{point:?}: every coin free again");
    assert_eq!(txids, BTreeSet::from([funding_tx]), "{point:?}");
    assert_eq!(total, whole, "{point:?}");
    assert_eq!(marks, Vec::new(), "{point:?}: the mark goes at the load");
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
}

/// After signing, before any durable record: the reservation was memory
/// only, so the next launch offers every coin again and nothing was sent.
#[test]
fn h03_core_send_killed_after_signing_leaves_nothing() {
    nothing_left(Point::Signed);
}

/// After the sent mark, before the hand-off: nothing reached the network,
/// but the next launch cannot tell that from a kill just after it (#170
/// case 1), so the payment's mark keeps it pending and its coins out until
/// it is resent or abandoned.
#[test]
fn h03_core_send_killed_before_the_hand_off_keeps_its_coins_spent() {
    the_payment_holds_its_coins(Point::Marked);
}

/// A kill once the payment may have left: the next launch must not offer
/// or count the coins it spent, must show it pending with Resend and
/// Abandon, and keep its mark; the same after another launch. Abandoning
/// it frees the coins once the scan passes their block again, and a launch
/// after that loads them.
fn the_payment_holds_its_coins(point: Point) {
    let (dir, r) = killed_at(point);
    let txid = r.txid.to_string();
    let whole = COINS as u64 * COIN;
    for launch in ["relaunched", "relaunched again"] {
        let (engine, session) = relaunch(dir.path());
        let (free, txids, total) = within_60s(
            || state(&engine, &session, r.wallet),
            |(free, txids, _)| free.is_disjoint(&r.inputs) && txids.contains(&txid),
        );
        assert!(
            free.is_disjoint(&r.inputs),
            "{point:?}, {launch}: the coins the payment spent are offered again: {:?}",
            free.intersection(&r.inputs).collect::<Vec<_>>()
        );
        assert!(
            txids.contains(&txid),
            "{point:?}, {launch}: the payment is not shown"
        );
        assert!(total < whole, "{point:?}, {launch}: {total}");
        let pending = (session.hub.history.get(&r.wallet, &r.txid))
            .is_some_and(|e| e.context.block_info().is_none() && !e.is_abandoned());
        assert!(pending, "{point:?}, {launch}: not pending");
        let extras = engine
            .block_on(session.tx_detail_extras(r.wallet, r.txid))
            .unwrap();
        assert!(
            extras.can_resend && extras.can_abandon,
            "{point:?}, {launch}: {extras:?}"
        );
        assert!(
            (sent_marks(&session, r.wallet).iter()).any(|(k, _)| k.ends_with(&txid)),
            "{point:?}, {launch}: the mark went"
        );
        drop(session);
        engine.block_on(engine.shutdown()).unwrap();
    }

    let (engine, session) = relaunch(dir.path());
    let funding_txid = r.funded.iter().next().unwrap().txid;
    let funding_tx = session
        .hub
        .history
        .get(&r.wallet, &funding_txid)
        .unwrap()
        .tx;
    let (s, id, root) = (Arc::clone(&session), r.wallet, r.txid);
    engine
        .block_on(session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, root).await
        }))
        .unwrap();
    mined(&engine, &session, r.wallet, FUNDED_AT, vec![funding_tx]);
    let (free, _, _) = state(&engine, &session, r.wallet);
    assert!(
        r.inputs.is_subset(&free),
        "{point:?}: abandoned, its coins stay out"
    );
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    let (engine, session) = relaunch(dir.path());
    let (free, _, _) = state(&engine, &session, r.wallet);
    assert!(
        r.inputs.is_subset(&free),
        "{point:?}: abandoned, its coins load spent"
    );
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
}

/// Handed off and stored by the wallet (dash-spv's relay), killed before
/// the outcome: the stored record and the sent mark keep its coins spent.
#[test]
fn h03_core_send_killed_after_the_wallet_stored_it_keeps_its_coins_spent() {
    the_payment_holds_its_coins(Point::Stored);
}

/// Handed off, killed before the wallet stored it: the network may hold
/// the payment, so its mark, which holds the signed transaction, keeps it
/// pending and its coins spent (H-03 defect D1, #170 case 1).
#[test]
fn h03_core_send_killed_after_the_hand_off_keeps_its_coins_spent() {
    the_payment_holds_its_coins(Point::Handed);
}
