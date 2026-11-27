//! A data root written by the desktop's previous main (fork-pinned platform
//! and rust-dashcore) opens on this build with the same balances, the same
//! coins, the same coin-control locks and the same held coins, and its
//! wallet store loads with nothing tolerated: no `AccountRecordInvalid`,
//! no `PoolRefused` (which storage reports as `AccountRecordInvalid`), no
//! other load degradation.
//!
//! `fixtures/main-store-f057bee77/` holds the data root and what that build
//! showed for it after reopening it offline (`main-observed.json`); its
//! README says how both were made.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use dashcore::Txid;
use dw_engine::{
    CoinFilter, DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, NetworkSession,
    SessionOptions, WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use platform_wallet::changeset::PlatformWalletPersistence;
use platform_wallet_storage::{LoadPolicy, SqlitePersister, SqlitePersisterConfig};
use serde_json::{Value, json};

struct Silent;

impl EventSink for Silent {
    fn emit(&self, _: EngineEvent) {}
}

fn fixture_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/main-store-f057bee77"
    ))
}

fn copy_private(from: &Path, to: &Path) {
    dw_testutil::private_dir(to);
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_private(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }
}

fn offline() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    }
}

fn engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(4),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Silent),
    )
    .unwrap()
}

/// What the engine shows for `w`, in the shape the writer recorded.
fn observe(engine: &Engine, s: &Arc<NetworkSession>, w: WalletId, txids: &[Txid]) -> Value {
    let b = s.balances(&w).unwrap().expect("balances known");
    let mut coins: Vec<Value> = engine
        .block_on(s.utxos(
            w,
            CoinFilter {
                include_locked: true,
                fully_mixed_only: false,
                min_confirmations: None,
            },
        ))
        .unwrap()
        .into_iter()
        .map(|c| {
            json!({
                "outpoint": c.outpoint.to_string(),
                "address": c.address,
                "amount": c.amount,
                "confirmations": c.confirmations,
                "block_height": c.block_height,
                "instant_locked": c.instant_locked,
                "chain_locked": c.chain_locked,
                "user_locked": c.user_locked,
                "reserved": c.reserved,
                "is_change": c.is_change,
                "spendable": c.spendable,
                "excluded": c.excluded.map(|e| format!("{e:?}")),
            })
        })
        .collect();
    coins.sort_by_key(|c| c["outpoint"].as_str().unwrap().to_owned());
    let mut locked: Vec<String> = (engine.block_on(s.locked_outpoints(w)).unwrap().iter())
        .map(|o| o.to_string())
        .collect();
    locked.sort();
    let txs: Vec<Value> = (txids.iter())
        .map(|t| {
            let x = engine.block_on(s.tx_detail_extras(w, *t)).unwrap();
            let mut withheld: Vec<String> =
                x.withheld_outputs.iter().map(|o| o.to_string()).collect();
            withheld.sort();
            json!({
                "txid": x.txid,
                "abandoned": x.abandoned,
                "can_abandon": x.can_abandon,
                "withheld_outputs": withheld,
                "checking_spent_coins": x.checking_spent_coins,
                "total_credit": x.total_credit,
                "total_debit": x.total_debit,
            })
        })
        .collect();
    json!({
        "wallet_id": w.to_string(),
        "balances": {
            "confirmed": b.confirmed, "unconfirmed": b.unconfirmed, "immature": b.immature,
            "locked": b.locked, "total": b.total, "coinjoin": b.coinjoin, "withheld": b.withheld,
            "catching_up": b.catching_up, "held_unfinished": b.held_unfinished,
            "held_unvalued": b.held_unvalued, "held_awaiting": b.held_awaiting,
        },
        "coins": coins,
        "locked_outpoints": locked,
        "txs": txs,
    })
}

/// The wallet's stored rescan-hold marks, read from a copy of `app.sqlite`.
fn hold_marks(network_dir: &Path, w: &WalletId) -> Value {
    let tmp = dw_testutil::private_tempdir();
    for f in ["app.sqlite", "app.sqlite-wal", "app.sqlite-shm"] {
        if network_dir.join(f).exists() {
            fs::copy(network_dir.join(f), tmp.path().join(f)).unwrap();
        }
    }
    let db = rusqlite::Connection::open(tmp.path().join("app.sqlite")).unwrap();
    let mut st = db
        .prepare(
            "SELECT key, value FROM settings_kv \
             WHERE scope = ?1 AND key LIKE 'rescan_hold:%' ORDER BY key",
        )
        .unwrap();
    let marks: Vec<Value> = st
        .query_map([w.to_string()], |r| {
            Ok(json!({"key": r.get::<_, String>(0)?, "value": r.get::<_, Option<String>>(1)?}))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    Value::Array(marks)
}

#[test]
fn a_store_main_wrote_opens_with_the_same_balances_locks_and_held_coins() {
    let main: Value = serde_json::from_str(
        &fs::read_to_string(fixture_dir().join("main-observed.json")).unwrap(),
    )
    .unwrap();
    let want = &main["reopened_offline"];
    let w: WalletId = want["wallet_id"].as_str().unwrap().parse().unwrap();
    let txids: Vec<Txid> = (want["txs"].as_array().unwrap().iter())
        .map(|t| t["txid"].as_str().unwrap().parse().unwrap())
        .collect();

    let tmp = dw_testutil::private_tempdir();
    let root = tmp.path().join("data");
    copy_private(&fixture_dir().join("data"), &root);
    let net = root.join(main["network_dir"].as_str().unwrap());

    let engine = engine(&root);
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .expect("the store main wrote opens");
    let ids: Vec<WalletId> = (s.wallet_infos().unwrap().into_iter())
        .map(|i| i.wallet_id)
        .collect();
    assert_eq!(ids, vec![w], "the one wallet main wrote loads");
    let got = observe(&engine, &s, w, &txids);
    eprintln!("main: {want:#}\nthis build: {got:#}");
    for key in ["balances", "coins", "locked_outpoints", "txs"] {
        assert_eq!(got[key], want[key], "{key}");
    }
    drop(s);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);
    assert_eq!(
        hold_marks(&net, &w),
        main["rescan_hold_marks_final"],
        "the stored rescan holds"
    );

    let persister = SqlitePersister::open(
        SqlitePersisterConfig::new(net.join("wallet.sqlite"))
            .with_load_policy(LoadPolicy::Recovery),
    )
    .expect("the wallet store opens");
    let state = persister.load().expect("the wallet store loads");
    assert!(
        state.wallets.contains_key(&w.0),
        "the wallet is in the load"
    );
    let degradation = persister.last_load_degradation();
    eprintln!("load degradation: {degradation:?}");
    assert!(!persister.is_degraded(), "{degradation:?}");
    assert_eq!(degradation.total, 0, "{degradation:?}");
    assert!(degradation.by_site.is_empty(), "{degradation:?}");
    assert!(degradation.wallets_degraded.is_empty(), "{degradation:?}");
}
