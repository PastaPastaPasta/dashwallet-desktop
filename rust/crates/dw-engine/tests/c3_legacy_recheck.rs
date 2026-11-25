//! C3 against stores the published fork builds wrote (tests/fixtures/c3-legacy,
//! see each pin's PROVENANCE.md): the v0.3.0 release's data with its wallet
//! store replaced by one whose open at the pin dropped claims into
//! `core_dropped_unsigned_spends`. The first open judges each dropped claim
//! again by the store repair's rules, before the persister upgrades the
//! store, so no session offers a coin the rules hold.

mod store_upgrade_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_engine::{DashNetwork, Engine, EngineError, NetworkSession, WalletBalances, WalletId};
use sha2::{Digest, Sha256};
use store_upgrade_common::{Fixture, engine, exec, fixture, local_opts};

const PINS: [&str; 2] = ["c5917e6bd", "v030-373342cb3"];

const WALLET_A: &str = "0ff35f248c69653efce8336ce14b78e29cd3bbaf4162c42246259850af2c2add";
const WALLET_B: &str = "da8b3e81edb50926c70d3cd3c5de4102fa7c1419b0559fe19d06813c64dcca30";

/// Each claimed coin pays this.
const COIN: u64 = 500_000;

/// A dropped claim of the fixtures: its wallet, txid (display order) and
/// the outpoint blob of the coin it spends.
struct Claim {
    wallet: &'static str,
    txid: &'static str,
    coin: &'static str,
}

const A: Claim = Claim {
    wallet: WALLET_A,
    txid: "f22699f7d1c7a3fa119a6c734e64fc0643c18da9d326d0fbc2254d3dfe8e99fc",
    coin: "2001A5C50F8EB6A91DD97BC167F3381BAE9D5916FCFB2E595570547D2A66B6CE9100",
};
const B: Claim = Claim {
    wallet: WALLET_A,
    txid: "b9ee9599b9e3fe2d4e7bf7997fbbdb1e7f52edecfaab9adba98e7043c66eabdd",
    coin: "204185550E700E883D8B7FF2625778CAB104C4027029F8F35B7CE65136E17BC5B300",
};
const C: Claim = Claim {
    wallet: WALLET_B,
    txid: "00958be2e62ef41a80a6d56cc4c8295e979cc491df36b758a49e0c08b8535374",
    coin: "20A2502EA8AB1BB94B4E32328B58FA7A9B302F14FF9731B2217AEE111BB761CBC700",
};
const D: Claim = Claim {
    wallet: WALLET_A,
    txid: "2779809a779ef0ac770de30cea61bb66bcd561fea07953aaa9d9f48efffb94bc",
    coin: "20ABC4162ACB3F98BFCD00B4032599420A3B7DD37EF7D42B752DAB9617B45741A900",
};
/// The mined transaction that spends (d)'s coin after the fork gave it back.
const D_SPENDER: &str = "8623fd8588d1e357a7eba0122668dccb0cd6a78f043ffa4cc6fdc7363b95ec0c";
const G: Claim = Claim {
    wallet: WALLET_A,
    txid: "651d12af56e37477d019b9c29136560ebdb82ca137c98cf01ae4667c784f2178",
    coin: "20C2106C7530C4E46B2CB4C32DA2D450EE225DE28AA674137548BABBEC7A47B11F00",
};

/// `file` of `pin`'s fixtures, its sha256 checked against SHA256SUMS.
fn verified(pin: &str, file: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/c3-legacy");
    let name = format!("{pin}/{file}");
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
    let want = (sums.lines())
        .find_map(|l| l.strip_suffix(&format!("  {name}")))
        .unwrap_or_else(|| panic!("{name} isn't in SHA256SUMS"));
    let path = dir.join(&name);
    let got = hex::encode(Sha256::digest(std::fs::read(&path).unwrap()));
    assert_eq!(got, want, "{name}'s sha256");
    path
}

/// The v0.3.0 release's data root with `pin`'s `file` as its wallet store.
fn store(pin: &str, file: &str) -> Fixture {
    let f = fixture("v0.3.0");
    for extra in ["wallet.sqlite-wal", "wallet.sqlite-shm"] {
        let _ = std::fs::remove_file(f.net.join(extra));
    }
    std::fs::copy(verified(pin, file), f.wallet()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.wallet(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    f
}

fn open(f: &Fixture) -> (Engine, Arc<NetworkSession>) {
    let engine = engine(&f.root);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let session = loop {
        match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
            Ok(s) => break s,
            Err(EngineError::StorageInUse { .. }) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("open: {e:?}"),
        }
    };
    (engine, session)
}

fn close(engine: Engine, session: Arc<NetworkSession>) {
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
}

/// The balances of each fixture wallet, once its state is loaded.
fn balances(session: &NetworkSession) -> [WalletBalances; 2] {
    [WALLET_A, WALLET_B].map(|w| {
        let id: WalletId = w.parse().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            if let Some(b) = session.balances(&id).unwrap() {
                break b;
            }
            assert!(std::time::Instant::now() < deadline, "{w} loads");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    })
}

/// A txid in display order, as the store keys it.
fn key(display: &str) -> Vec<u8> {
    let mut bytes = hex::decode(display).unwrap();
    bytes.reverse();
    bytes
}

fn conn(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(f.wallet(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
}

/// The fork's rows for `c`.
fn fork_rows(f: &Fixture, c: &Claim) -> i64 {
    conn(f)
        .query_row(
            "SELECT count(*) FROM core_dropped_unsigned_spends WHERE wallet_id = ?1 AND txid = ?2",
            rusqlite::params![hex::decode(c.wallet).unwrap(), key(c.txid)],
            |r| r.get(0),
        )
        .unwrap()
}

/// `c`'s row in its wallet: height, as JSON; `None` when not stored.
fn transaction_row(f: &Fixture, wallet: &str, txid: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    conn(f)
        .query_row(
            "SELECT json_object('height', height, 'block_hash', hex(block_hash), \
             'block_time', block_time, 'finalized', finalized, 'record', hex(record_blob)) \
             FROM core_transactions WHERE wallet_id = ?1 AND txid = ?2",
            rusqlite::params![hex::decode(wallet).unwrap(), key(txid)],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}

/// `c`'s coin in its wallet: spent, and the hex of its `spent_in_txid`.
fn coin(f: &Fixture, c: &Claim) -> (i64, Option<String>) {
    conn(f)
        .query_row(
            "SELECT spent, CASE WHEN spent_in_txid IS NULL THEN NULL ELSE hex(spent_in_txid) END \
             FROM core_utxos WHERE wallet_id = ?1 AND outpoint = ?2",
            rusqlite::params![hex::decode(c.wallet).unwrap(), hex::decode(c.coin).unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

/// The lock rows stored for `c`: `verified` of each.
fn lock_rows(f: &Fixture, c: &Claim) -> Vec<i64> {
    let conn = conn(f);
    let mut stmt = conn
        .prepare("SELECT verified FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2")
        .unwrap();
    stmt.query_map(
        rusqlite::params![hex::decode(c.wallet).unwrap(), key(c.txid)],
        |r| r.get(0),
    )
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// The re-check's record of `c`, from the closed network's app database.
fn record(f: &Fixture, c: &Claim) -> Option<String> {
    dw_appdb::AppDb::open(&f.net.join(dw_appdb::APP_DB_FILE))
        .unwrap()
        .setting(c.wallet, &format!("store_restored:{}", c.txid))
        .unwrap()
}

/// What the re-check may change of `claims`, for the idempotence check.
fn state(f: &Fixture, claims: &[&Claim]) -> Vec<String> {
    (claims.iter())
        .map(|c| {
            format!(
                "{} {} {:?} {:?} {:?} {:?}",
                c.txid,
                fork_rows(f, c),
                transaction_row(f, c.wallet, c.txid),
                coin(f, c),
                lock_rows(f, c),
                record(f, c)
            )
        })
        .collect()
}

/// The first open of `pin`'s `file`, and of the same store with the
/// fork's rows emptied first (as a build without the re-check opens it).
/// `before` sees the fixture unopened. Each is opened once more, which
/// changes nothing. Returns both, with their balances.
fn first_open(
    pin: &str,
    file: &str,
    claims: &[&Claim],
    before: impl FnOnce(&Fixture),
) -> ((Fixture, [u64; 2]), (Fixture, [u64; 2])) {
    let opened = |f: Fixture| {
        let (engine, session) = open(&f);
        let totals = balances(&session).map(|b| b.total);
        close(engine, session);
        let once = state(&f, claims);
        let (engine, session) = open(&f);
        let again = balances(&session).map(|b| b.total);
        close(engine, session);
        assert_eq!(again, totals, "{pin}: a second open changes nothing");
        assert_eq!(
            state(&f, claims),
            once,
            "{pin}: a second open changes nothing"
        );
        (f, totals)
    };
    let f = store(pin, file);
    before(&f);
    let base = store(pin, file);
    exec(&base.wallet(), "DELETE FROM core_dropped_unsigned_spends");
    (opened(f), opened(base))
}

/// A claim put back pending in its wallet with its coin held for it.
fn assert_put_back(f: &Fixture, pin: &str, c: &Claim, reason: &str) -> String {
    let row = transaction_row(f, c.wallet, c.txid)
        .unwrap_or_else(|| panic!("{pin} {}: put back", c.txid));
    assert!(row.contains("\"height\":null"), "{pin}: pending: {row}");
    assert_eq!(fork_rows(f, c), 0, "{pin} {}: its fork row goes", c.txid);
    let (spent, spent_in) = coin(f, c);
    assert_eq!(spent, 1, "{pin} {}: its coin is held", c.txid);
    assert_eq!(spent_in, None, "{pin} {}", c.txid);
    let raw = record(f, c).unwrap_or_else(|| panic!("{pin} {}: recorded", c.txid));
    assert!(raw.contains("\"state\":\"done\""), "{raw}");
    assert!(raw.contains(reason), "{pin}: {raw}");
    raw
}

/// (a)–(d), at the first open: the claim whose input script isn't
/// push-only (a) and the one settled in the other wallet (b) come back
/// pending with their coins held, so the first session already doesn't
/// offer them; the push-only unsigned claim settled nowhere (c) stays
/// dropped; (d)'s coin, spent since by a mined transaction, isn't held for
/// its claim again whatever its verdict, and its spender doesn't change.
/// The balances are those of a build without the re-check, less (a)'s and
/// (b)'s coins.
#[test]
fn the_fork_builds_dropped_claims_are_judged_again_at_the_first_open() {
    for pin in PINS {
        let claims = [&A, &B, &C, &D];
        let ((f, after), (base, before)) = first_open(pin, "wallet.sqlite", &claims, |f| {
            assert_eq!(claims.map(|c| fork_rows(f, c)), [1; 4], "{pin}");
            assert_eq!(
                coin(f, &A),
                (0, None),
                "{pin}: the fork gave (a)'s coin back"
            );
            assert_eq!(
                coin(f, &D),
                (1, None),
                "{pin}: (d)'s coin, as the fork left it"
            );
        });

        assert_put_back(&f, pin, &A, "\"verdict\":\"keep\"");
        assert_put_back(&f, pin, &B, "of its tree is settled");

        assert_eq!(transaction_row(&f, C.wallet, C.txid), None, "{pin}: (c)");
        assert_eq!(fork_rows(&f, &C), 1, "{pin}: (c)'s fork row stays");
        assert_eq!(coin(&f, &C), (0, None), "{pin}: (c)'s coin stays offered");
        assert_eq!(record(&f, &C), None);

        let (spent, spent_in) = coin(&f, &D);
        assert_eq!(spent, 1, "{pin}: (d)'s coin stays spent");
        assert_ne!(
            spent_in,
            Some(hex::encode_upper(key(D.txid))),
            "{pin}: (d)'s coin isn't bound to its claim"
        );
        let spender = transaction_row(&base, WALLET_A, D_SPENDER).expect("(d)'s spender is stored");
        assert!(!spender.contains("\"height\":null"), "{spender}");
        assert_eq!(
            transaction_row(&f, WALLET_A, D_SPENDER).as_ref(),
            Some(&spender),
            "{pin}: (d)'s spender is as without the re-check"
        );
        let d = record(&f, &D);
        eprintln!(
            "{pin}: (d)'s verdict: {}",
            match &d {
                None => format!("dropped (fork rows {})", fork_rows(&f, &D)),
                Some(raw) => raw.clone(),
            }
        );
        if let Some(raw) = &d {
            assert!(raw.contains(&format!("\"not_held\":[\"{}\"]", D.coin.to_lowercase())));
            assert!(raw.contains("\"held_coins\":[]"), "{raw}");
        }

        assert_eq!(
            before[0] - after[0],
            2 * COIN,
            "{pin}: A holds (a)'s and (b)'s coins, and (d)'s balance is unchanged: \
             {before:?} → {after:?}"
        );
        assert_eq!(before[1], after[1], "{pin}: B is unchanged");
    }
}

/// (f) The same store with (c)'s record unreadable: it can't go back, so
/// the coin the fork released for it is held again, its fork row goes,
/// and the put-back is recorded as a doubt. The others are judged as with
/// readable records.
#[test]
fn a_fork_row_whose_record_doesnt_read_has_its_released_coin_held_again() {
    for pin in PINS {
        let claims = [&A, &B, &C, &D];
        let ((f, after), (_base, before)) =
            first_open(pin, "wallet-corrupt-blob.sqlite", &claims, |f| {
                assert_eq!(
                    coin(f, &C),
                    (0, None),
                    "{pin}: the fork gave (c)'s coin back"
                );
            });
        assert_put_back(&f, pin, &A, "\"verdict\":\"keep\"");
        assert_put_back(&f, pin, &B, "of its tree is settled");
        assert_eq!(transaction_row(&f, C.wallet, C.txid), None, "{pin}: (c)");
        assert_eq!(fork_rows(&f, &C), 0, "{pin}: (c)'s fork row goes");
        assert_eq!(coin(&f, &C), (1, None), "{pin}: (c)'s coin is held");
        let raw = record(&f, &C).expect("recorded");
        assert!(raw.contains("\"verdict\":\"doubt\""), "{raw}");
        assert!(raw.contains("\"restored_transaction\":false"), "{raw}");
        assert!(raw.contains("\"state\":\"done\""), "{raw}");
        assert_eq!(before[0] - after[0], 2 * COIN, "{pin}");
        assert_eq!(before[1] - after[1], COIN, "{pin}");
    }
}

/// (g) A fork row with a lock (`islock_blob` set, verified 0, not
/// quorum-signed), the claim neither mined nor locked anywhere the store
/// holds: the lock it had keeps it (L1a) unverified and unread, so it
/// comes back pending at the first open with its coin held and its lock
/// row back, unverified.
#[test]
fn a_fork_row_with_a_lock_is_put_back_pending_with_its_coin_held() {
    for pin in PINS {
        let ((f, after), (_base, before)) = first_open(pin, "wallet-islock.sqlite", &[&G], |f| {
            assert!(
                lock_rows(f, &G).is_empty(),
                "{pin}: no lock row the store holds"
            );
            assert_eq!(transaction_row(f, G.wallet, G.txid), None);
            assert_eq!(coin(f, &G), (0, None), "{pin}: the fork gave its coin back");
        });
        let raw = assert_put_back(&f, pin, &G, "it had a lock row");
        assert!(raw.contains("\"restored_lock\":true"), "{raw}");
        assert_eq!(lock_rows(&f, &G), vec![0], "{pin}: its lock row is back");
        assert_eq!(before[0] - after[0], COIN, "{pin}");
        assert_eq!(before[1], after[1], "{pin}");
    }
}
