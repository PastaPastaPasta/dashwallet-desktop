//! Generates the V3 recovery-record fixture: a store a fork V1 build left
//! with unsigned pending claims (two of them InstantSend-locked), opened by
//! this build's `SqlitePersister::open`, which migrates it to fork V3 and
//! runs `drop_unauthorized_pending_spends`.
//!
//! Run: `V3FIXTURE_OUT=/path/wallet.sqlite cargo test -p platform-wallet-storage
//! --lib -- --ignored --exact
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

#[test]
#[ignore = "fixture generator; run explicitly with V3FIXTURE_OUT"]
fn generate_v3_dropped_unsigned_spends_fixture() {
    let out = std::path::PathBuf::from(
        std::env::var("V3FIXTURE_OUT").expect("V3FIXTURE_OUT names the fixture output path"),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.db");

    let wa = [0xA1u8; 32];
    let wb = [0xB2u8; 32];
    let mut conn = wallet_db_in(rusqlite::Connection::open(&path).unwrap(), &wa);
    conn.execute(
        "INSERT INTO wallets (wallet_id, network, birth_height) VALUES (?1, 'testnet', 0)",
        params![&wb[..]],
    )
    .unwrap();

    let (a_locked_coin, a_locked_change, a_locked) =
        claim_in_store(&mut conn, &wa, 0x61, 0x99, locked);
    let (a_pending_coin, _, a_pending) = claim_in_store(&mut conn, &wa, 0x62, 0x99, pending);
    let (b_locked_coin, b_locked_change, b_locked) =
        claim_in_store(&mut conn, &wb, 0x64, 0x99, locked);
    let (_, b_signed_change, b_signed) = claim_in_store(&mut conn, &wb, 0x65, 0x31, pending);

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
    if wal || shm {
        let plain = rusqlite::Connection::open(&path).unwrap();
        let (busy, log, ckpt): (i64, i64, i64) = plain
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        println!("V3FIXTURE wal_checkpoint(TRUNCATE): busy={busy} log={log} checkpointed={ckpt}");
        drop(plain);
        let (wal, shm) = sidecars(&path);
        println!("V3FIXTURE after plain checkpoint+close: -wal exists={wal} -shm exists={shm}");
        assert!(!wal && !shm);
    }
    std::fs::copy(&path, &out).unwrap();
    println!("V3FIXTURE written to {}", out.display());

    let conn = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut a_expected = vec![a_locked_coin.outpoint, a_pending_coin.outpoint];
    a_expected.sort();
    assert_eq!(unspent(&conn, &wa), a_expected);
    let mut b_expected = vec![b_locked_coin.outpoint, b_signed_change.outpoint];
    b_expected.sort();
    assert_eq!(unspent(&conn, &wb), b_expected);
    assert!(!has_record(&conn, &wa, a_locked.txid()));
    assert!(!has_record(&conn, &wa, a_pending.txid()));
    assert!(!has_record(&conn, &wb, b_locked.txid()));
    assert!(has_record(&conn, &wb, b_signed.txid()));

    let kept_a = kept(&conn, &wa);
    let kept_b = kept(&conn, &wb);
    assert_eq!(
        kept_a.iter().map(|k| k.0).collect::<std::collections::BTreeSet<_>>(),
        [a_locked.txid(), a_pending.txid()].into_iter().collect()
    );
    assert_eq!(kept_b.iter().map(|k| k.0).collect::<Vec<_>>(), vec![b_locked.txid()]);
    for (txid, coin, change) in [
        (a_locked.txid(), &a_locked_coin, &a_locked_change),
        (b_locked.txid(), &b_locked_coin, &b_locked_change),
    ] {
        let w = if txid == a_locked.txid() { &wa } else { &wb };
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
        "V3FIXTURE ok: A dropped {} (locked {}, pending {}), B dropped {} (locked {}), B kept signed {}",
        kept_a.len(),
        a_locked.txid(),
        a_pending.txid(),
        kept_b.len(),
        b_locked.txid(),
        b_signed.txid()
    );
}
