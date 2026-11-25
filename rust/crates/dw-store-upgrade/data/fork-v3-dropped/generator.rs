//! Generates the V3 recovery-record fixture from a copy of the released
//! v0.3.0 `wallet.sqlite` (real wallets with their account registrations):
//! unsigned pending claims seeded through `core_state::apply`, the store
//! taken back to how a fork V1 build left it, then this build's
//! `SqlitePersister::open`, which re-applies fork V2 and V3 and runs
//! `drop_unauthorized_pending_spends`.
//!
//! Each claimed coin pays one of the wallet's own unused BIP44 receive
//! scripts, so the wallet owns it and loads it once it is released; the
//! claim carries a signature and key that don't match that script, so it
//! is unsigned. Nothing is written but through `apply`, `as_fork_v1_left_it`
//! and the open.
//!
//! Run: `V3FIXTURE_SRC=<v0.3.0>/wallet.sqlite V3FIXTURE_OUT=<out>/wallet.sqlite
//! cargo test -p platform-wallet-storage --lib -- --ignored --exact --nocapture
//! sqlite::schema::core_state::tests::unauthorized_pending_spends::v3fixture_gen::generate_v3_dropped_unsigned_spends_fixture`

use super::*;

fn sidecars(path: &std::path::Path) -> (bool, bool) {
    let side = |suffix: &str| {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        std::path::PathBuf::from(p).exists()
    };
    (side("-wal"), side("-shm"))
}

fn wallet_id(hex: &str) -> [u8; 32] {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

/// The wallet's `skip`-th unused BIP44 account 0 receive script.
fn receive_script(conn: &rusqlite::Connection, w: &[u8; 32], skip: i64) -> dashcore::ScriptBuf {
    let script: Vec<u8> = conn
        .query_row(
            "SELECT script FROM core_address_pool WHERE wallet_id = ?1 \
             AND account_type = 'standard_bip44' AND account_index = 0 AND pool_type = 0 \
             AND used = 0 ORDER BY address_index LIMIT 1 OFFSET ?2",
            params![&w[..], skip],
            |r| r.get(0),
        )
        .unwrap();
    dashcore::ScriptBuf::from_bytes(script)
}

/// `claim_in_store` for a coin paying `script`: the coin, stored through
/// `apply`, then an unsigned spend of it in `context` with change, the coin
/// marked spent.
fn owned_claim_in_store(
    conn: &mut rusqlite::Connection,
    w: &[u8; 32],
    funding: u8,
    script: dashcore::ScriptBuf,
    context: impl Fn(&Transaction) -> TransactionContext,
) -> (Utxo, Utxo, Transaction) {
    let held = Utxo {
        outpoint: OutPoint {
            txid: Txid::from_byte_array([funding; 32]),
            vout: 0,
        },
        txout: TxOut {
            value: 500_000,
            script_pubkey: script.clone(),
        },
        address: dashcore::Address::from_script(&script, dashcore::Network::Regtest).unwrap(),
        height: 50,
        is_coinbase: false,
        is_confirmed: true,
        is_instantlocked: false,
        is_locked: false,
        is_trusted: false,
    };
    store(
        conn,
        w,
        &CoreChangeSet {
            new_utxos: vec![held.clone()],
            ..Default::default()
        },
    );
    let (change, claim) = claim_with_change(&held, 0x99);
    let mut cs = CoreChangeSet {
        records: vec![spend_record(claim.clone(), context(&claim))],
        new_utxos: vec![change.clone()],
        spent_utxos: vec![held.clone()],
        ..Default::default()
    };
    if let TransactionContext::InstantSend(lock) = context(&claim) {
        cs.instant_locks_for_non_final_records
            .insert(claim.txid(), lock);
    }
    store(conn, w, &cs);
    (held, change, claim)
}

fn unspent_on(conn: &rusqlite::Connection, w: &[u8; 32]) -> Vec<OutPoint> {
    let (loaded, _) = load_state(conn, w, dashcore::Network::Regtest, &LoadCtx::strict()).unwrap();
    let mut outpoints: Vec<_> = loaded.new_utxos.iter().map(|u| u.outpoint).collect();
    outpoints.sort();
    outpoints
}

#[test]
#[ignore = "fixture generator; run explicitly with V3FIXTURE_SRC and V3FIXTURE_OUT"]
fn generate_v3_dropped_unsigned_spends_fixture() {
    let src = std::path::PathBuf::from(
        std::env::var("V3FIXTURE_SRC").expect("V3FIXTURE_SRC names a copy of v0.3.0 wallet.sqlite"),
    );
    let out = std::path::PathBuf::from(
        std::env::var("V3FIXTURE_OUT").expect("V3FIXTURE_OUT names the fixture output path"),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.sqlite");
    std::fs::copy(&src, &path).unwrap();
    assert_eq!(sidecars(&src), (false, false), "the source has no log beside it");

    let wa = wallet_id("0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD");
    let wb = wallet_id("DA8B3E81EDB50926C70D3CD3C5DE4102FA7C1419B0559FE19D06813C64DCCA30");
    let mut conn = rusqlite::Connection::open(&path).unwrap();
    crate::sqlite::migrations::run(&mut conn).unwrap();
    let a_before = unspent_on(&conn, &wa);
    let b_before = unspent_on(&conn, &wb);

    let scripts = [
        receive_script(&conn, &wa, 0),
        receive_script(&conn, &wa, 1),
        receive_script(&conn, &wb, 0),
    ];
    let [a1, a2, b1] = scripts;
    let (a_locked_coin, a_locked_change, a_locked) =
        owned_claim_in_store(&mut conn, &wa, 0x61, a1, locked);
    let (a_pending_coin, _, a_pending) = owned_claim_in_store(&mut conn, &wa, 0x62, a2, pending);
    let (b_locked_coin, b_locked_change, b_locked) =
        owned_claim_in_store(&mut conn, &wb, 0x64, b1, locked);

    let locks_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM core_instant_locks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(locks_before, 2, "both locked claims stored their lock");
    as_fork_v1_left_it(&conn);
    drop(conn);

    let persister = crate::sqlite::persister::SqlitePersister::open(
        crate::sqlite::config::SqlitePersisterConfig::new(&path),
    )
    .unwrap();
    drop(persister);

    let (wal, shm) = sidecars(&path);
    println!("V3FIXTURE after persister drop: -wal exists={wal} -shm exists={shm}");
    assert!(!wal && !shm, "the persister closed the store cleanly");
    std::fs::copy(&path, &out).unwrap();
    println!("V3FIXTURE written to {}", out.display());

    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut a_expected = [a_before, vec![a_locked_coin.outpoint, a_pending_coin.outpoint]].concat();
    a_expected.sort();
    assert_eq!(unspent_on(&conn, &wa), a_expected);
    let mut b_expected = [b_before, vec![b_locked_coin.outpoint]].concat();
    b_expected.sort();
    assert_eq!(unspent_on(&conn, &wb), b_expected);
    assert!(!has_record(&conn, &wa, a_locked.txid()));
    assert!(!has_record(&conn, &wa, a_pending.txid()));
    assert!(!has_record(&conn, &wb, b_locked.txid()));

    let kept_a = kept(&conn, &wa);
    let kept_b = kept(&conn, &wb);
    assert_eq!(
        kept_a.iter().map(|k| k.0).collect::<std::collections::BTreeSet<_>>(),
        [a_locked.txid(), a_pending.txid()].into_iter().collect()
    );
    assert_eq!(kept_b.iter().map(|k| k.0).collect::<Vec<_>>(), vec![b_locked.txid()]);
    for (w, txid, coin, change) in [
        (&wa, a_locked.txid(), &a_locked_coin, &a_locked_change),
        (&wb, b_locked.txid(), &b_locked_coin, &b_locked_change),
    ] {
        let (islock_len, verified, deleted, released): (Option<i64>, Option<i64>, String, String) =
            conn.query_row(
                "SELECT length(islock_blob), islock_verified, deleted_outputs, released_coins \
                 FROM core_dropped_unsigned_spends WHERE wallet_id = ?1 AND txid = ?2",
                params![&w[..], AsRef::<[u8]>::as_ref(&txid)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert!(islock_len.unwrap_or(0) > 0, "{txid} kept its lock");
        assert_eq!(verified, Some(0));
        assert!(released.contains(&outpoint_hex(&coin.outpoint)), "{released}");
        assert!(deleted.contains(&outpoint_hex(&change.outpoint)), "{deleted}");
    }
    let pending_lock: Option<i64> = conn
        .query_row(
            "SELECT length(islock_blob) FROM core_dropped_unsigned_spends \
             WHERE wallet_id = ?1 AND txid = ?2",
            params![&wa[..], AsRef::<[u8]>::as_ref(&a_pending.txid())],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending_lock, None, "a claim without a lock row keeps NULL");
    let locks_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM core_instant_locks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(locks_after, 0, "the drop removed both lock rows");
    println!(
        "V3FIXTURE ok: A dropped {} (locked {}, pending {}), B dropped {} (locked {})",
        kept_a.len(),
        a_locked.txid(),
        a_pending.txid(),
        kept_b.len(),
        b_locked.txid()
    );
}
