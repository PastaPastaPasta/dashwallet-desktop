//! T3 (a) and (d): the released v0.3.0 store with records written by the
//! fork's own drop path, once at v0.3.0's pin (`data/fork-v3-dropped-v030`)
//! and once at the nightly builds' (`data/fork-v3-dropped`, each with its
//! PROVENANCE.md), goes through the whole upgrade, a Strict upstream open
//! and a stored changeset with its `core_dropped_unsigned_spends` byte for
//! byte as found. The coins its drops gave back stay unspent and the Strict
//! load returns them as the wallet's (T3 b), and removing one wallet deletes
//! only that wallet's rows (T3 c).
//! A V3 store the upstream load can't read is refused at the rehearsal.
//!
//! Each fixture's V1-era state is the fork's test seeding (its tests'
//! `as_fork_v1_left_it`), not a file a V1 build wrote; the open that
//! re-applied V2 and V3 and ran the drop is the fork's own. Each
//! InstantSend lock is a test vector: an all-zero signature, stored
//! unverified (`islock_verified` 0).

use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::changeset::{
    CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
};
use rusqlite::types::Value;

use super::*;
use crate::error::StoreUpgradeError;
use crate::precheck::Precheck;
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::sql::{open_immutable, sha256_file};

const TABLE: &str = "core_dropped_unsigned_spends";
const SHA256: &str = "d132793137d4fd554245fbd709bf13986225f7f2289b8a99914f1700d335fec8";
const SHA256_V030: &str = "0ab260fdfb032e2731c27e23aeb2fc174ef7f40f53beb179a4d56485bc356c5c";
const NO_ACCOUNTS_SHA256: &str = "c89e5d5a29477ab2ff075f0efd8c97c05d199462a2dc96b67185b2f21948101b";
const WALLET_A: [u8; 32] = id("0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD");
const WALLET_B: [u8; 32] = id("DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30");

const fn id(hex: &str) -> [u8; 32] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            _ => c - b'A' + 10,
        }
    }
    let h = hex.as_bytes();
    let mut out = [0; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(h[2 * i]) << 4) | nibble(h[2 * i + 1]);
        i += 1;
    }
    out
}

/// A data root whose `network` directory holds a copy of `data/{name}`'s
/// `wallet.sqlite`, checked against its recorded sha256.
fn data_fixture(name: &str, sha256: &str, network: &str) -> Fixture {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
        .join("wallet.sqlite");
    assert_eq!(sha256_file(&source).unwrap(), sha256, "{name} as generated");
    let tmp = private_tempdir();
    let root = tmp.path().join("data");
    crate::fsx::create_private_dir(&root).unwrap();
    let net = root.join(network);
    crate::fsx::create_private_dir(&net).unwrap();
    fs::copy(source, net.join("wallet.sqlite")).unwrap();
    Fixture {
        _tmp: tmp,
        root,
        net,
    }
}

/// The drop's two forms of a kept row's absent spender: v0.3.0's pin wrote
/// `hex(NULL)`, an empty string; the nightly builds' pin writes JSON null.
/// A reader of these rows accepts both.
const V3_FIXTURES: [(&str, &str, &str); 2] = [
    ("fork-v3-dropped-v030", SHA256_V030, r#""spent_in_txid":"""#),
    ("fork-v3-dropped", SHA256, r#""spent_in_txid":null"#),
];

/// Each fixture, its kept rows' absent spender in its own form.
fn v3_fixtures() -> Vec<Fixture> {
    V3_FIXTURES
        .iter()
        .map(|(name, sha256, form)| {
            let f = data_fixture(name, sha256, "regtest");
            let conn = open_immutable(&f.wallet()).unwrap();
            let kept: Vec<String> = conn
                .prepare(&format!(
                    "SELECT deleted_outputs FROM {TABLE} UNION ALL SELECT released_coins FROM {TABLE}"
                ))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let spenders: usize = kept.iter().map(|t| t.matches(r#""spent_in_txid":"#).count()).sum();
            let own: usize = kept.iter().map(|t| t.matches(form).count()).sum();
            assert!(spenders > 0 && own == spenders, "{name}: {kept:?}");
            drop(conn);
            f
        })
        .collect()
}

/// The table's and its index's SQL, and `SELECT *` of every row in id order.
#[derive(Debug, PartialEq)]
struct Dump {
    schema: Vec<(String, String, Option<String>)>,
    rows: Vec<Vec<Value>>,
}

/// The immutable reads below see the main file only: no log may hold rows.
fn no_log(path: &Path) {
    for suffix in ["-wal", "-journal"] {
        let log = PathBuf::from(format!("{}{suffix}", path.display()));
        assert!(
            fs::metadata(&log).map_or(true, |m| m.len() == 0),
            "{} holds unread writes",
            log.display()
        );
    }
}

fn dump(path: &Path) -> Dump {
    no_log(path);
    let conn = open_immutable(path).unwrap();
    let schema = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master WHERE tbl_name = ?1 ORDER BY type, name",
        )
        .unwrap()
        .query_map([TABLE], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {TABLE} ORDER BY id"))
        .unwrap();
    let width = stmt.column_count();
    let rows = stmt
        .query_map([], |r| (0..width).map(|i| r.get::<_, Value>(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    Dump { schema, rows }
}

/// Each released coin as (drop id, wallet, outpoint, spent), found in
/// `core_utxos` through the drop's `released_coins` JSON.
fn released(path: &Path) -> Vec<(i64, Vec<u8>, Vec<u8>, i64)> {
    no_log(path);
    open_immutable(path)
        .unwrap()
        .prepare(&format!(
            "SELECT d.id, u.wallet_id, u.outpoint, u.spent \
             FROM {TABLE} d, json_each(d.released_coins) j \
             JOIN core_utxos u ON u.wallet_id = d.wallet_id \
              AND hex(u.outpoint) = json_extract(j.value, '$.outpoint') \
             ORDER BY d.id, u.outpoint"
        ))
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Accept, R1's drop, a Strict open and load that stores and flushes a
/// changeset, close, then the retire on the next start and a second open.
fn upgrade_with_a_persist_cycle(f: &Fixture) {
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .expect("fork V3 is accepted")
        .snapshot
        .unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    let (persister, token) = open_and_load(&f.net);
    let changeset = PlatformWalletChangeSet {
        core: Some(CoreChangeSet {
            last_processed_height: Some(200),
            synced_height: Some(200),
            ..Default::default()
        }),
        ..Default::default()
    };
    persister.store(WALLET_A, changeset).unwrap();
    persister.flush(WALLET_A).unwrap();
    drop(persister);
    drop(token);
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Retired { .. }
    ));
    drop(open_and_load(&f.net));
}

#[test]
fn the_forks_own_v3_records_survive_the_upgrade_and_a_persist_cycle_byte_for_byte() {
    for f in v3_fixtures() {
        records_survive(&f);
    }
}

fn records_survive(f: &Fixture) {
    let before = dump(&f.wallet());
    let islock = before.rows.iter().any(|row| {
        matches!(&row[4], Value::Blob(b) if !b.is_empty())
            && matches!(&row[7], Value::Text(t) if !t.is_empty() && t != "[]")
    });
    assert!(
        islock,
        "a record with an InstantSend lock and released coins"
    );
    let coins = released(&f.wallet());
    assert_eq!(
        coins.iter().map(|c| c.0).collect::<Vec<_>>(),
        [1, 2, 3],
        "one released coin per record"
    );
    assert!(coins.iter().all(|c| c.3 == 0), "released coins are unspent");

    upgrade_with_a_persist_cycle(f);

    assert_eq!(dump(&f.wallet()), before);
    assert_eq!(released(&f.wallet()), coins, "still unspent");
    let (persister, token) = PersisterOpened::open(config(&f.net)).unwrap();
    let state = persister.load().expect("Strict load");
    for (wallet, funding) in [(WALLET_A, "61"), (WALLET_A, "62"), (WALLET_B, "64")] {
        let outpoint = format!("{}:0", funding.repeat(32));
        assert!(
            state.wallets[&wallet]
                .wallet_info
                .utxos()
                .iter()
                .any(|u| u.outpoint.to_string() == outpoint),
            "the released coin {outpoint} loads as the wallet's"
        );
    }
    drop(persister);
    drop(token);
    let conn = open_immutable(&f.wallet()).unwrap();
    let height: i64 = conn
        .query_row(
            "SELECT synced_height FROM core_sync_state WHERE wallet_id = ?1",
            [WALLET_A.as_slice()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(height, 200, "the stored changeset is durable");
    let retired: Vec<i64> = conn
        .prepare("SELECT version FROM dw_fork_schema_history_retired ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(retired, [1, 2, 3]);
}

#[test]
fn removing_wallet_a_deletes_only_its_v3_records() {
    for f in v3_fixtures() {
        removing_a_keeps_b(&f);
    }
}

fn removing_a_keeps_b(f: &Fixture) {
    upgrade_with_a_persist_cycle(f);
    let before = dump(&f.wallet());
    let wallet = |row: &Vec<Value>| match &row[1] {
        Value::Blob(b) => b.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        before.rows.iter().map(wallet).collect::<Vec<_>>(),
        [WALLET_A.to_vec(), WALLET_A.to_vec(), WALLET_B.to_vec()]
    );
    let b_coins: Vec<_> = released(&f.wallet())
        .into_iter()
        .filter(|c| c.1 == WALLET_B)
        .collect();
    assert_eq!(b_coins.len(), 1);

    let (persister, token) = open_and_load(&f.net);
    persister.delete_wallet(WALLET_A).unwrap();
    drop(persister);
    drop(token);

    let after = dump(&f.wallet());
    assert_eq!(after.schema, before.schema);
    let kept: Vec<Vec<Value>> = before
        .rows
        .into_iter()
        .filter(|row| wallet(row) != WALLET_A)
        .collect();
    assert_eq!(after.rows, kept, "B's record byte for byte, A's gone");
    assert_eq!(
        released(&f.wallet()),
        b_coins,
        "B's released coin untouched"
    );
    drop(open_and_load(&f.net));
}

#[test]
fn a_v3_store_the_upstream_load_cant_read_is_refused_at_the_rehearsal() {
    let f = data_fixture("fork-v3-no-accounts", NO_ACCOUNTS_SHA256, "testnet");
    let before = tree(&f.root);
    let prepared = crate::prepare_with(&f.net, build(), now()).unwrap();
    assert!(prepared.snapshot.is_none(), "a refusal takes no snapshot");
    let Precheck::Refuse { reason } = prepared.precheck else {
        panic!("{:?}", prepared.precheck)
    };
    assert!(
        matches!(&reason, StoreUpgradeError::UpgradeRehearsalFailed { detail }
            if detail.contains("required account information is missing")),
        "{reason:?}"
    );
    assert!(reason.file_unchanged() && reason.is_refusal());
    assert_eq!(tree_diff(&tree(&f.root), &before), Vec::<String>::new());
}

/// The no-accounts V3 store (its PROVENANCE.md) serves the rehearsal refusal
/// above and nothing else: no other code in the crate names it.
#[test]
fn the_no_accounts_store_serves_only_the_rehearsal_refusal() {
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let name = concat!("fork-v3-", "no-accounts");
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&crate_dir.join("src"), &mut files);
    let mut uses = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        for (at, _) in text.match_indices(name) {
            uses.push((path.clone(), at, text.clone()));
        }
    }
    let [(path, at, text)] = uses.as_slice() else {
        panic!("{} uses of {name}", uses.len())
    };
    assert!(path.ends_with("tests/v3_dropped.rs"), "{}", path.display());
    let start = text
        .find("fn a_v3_store_the_upstream_load_cant_read_is_refused_at_the_rehearsal()")
        .unwrap();
    let end = start + text[start..].find("\n}\n").unwrap();
    assert!((start..end).contains(at), "used outside the refusal test");
}
