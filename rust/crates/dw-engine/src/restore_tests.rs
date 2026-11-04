//! A wallet's coins come back from its store in the funds account that
//! derives their address (platform-wallet-storage's rehydrate through
//! key-wallet `restore_utxos`), including CoinJoin coins past the stored
//! pool rows (0..99), and a replay of their blocks (a rescan, an abandon's
//! rescan, a crash catch-up) counts each once.

use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{Address, BlockHash, OutPoint, Transaction, TxIn, TxOut, Txid};
use key_wallet::Utxo;
use key_wallet::Wallet;
use key_wallet::account::AccountType;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::transaction_checking::{BlockInfo, TransactionContext, WalletTransactionChecker};
use key_wallet::wallet::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

use crate::session::NetworkSession;
use crate::{DashNetwork, Engine, WalletId};

/// A 1.00001 DASH CoinJoin denomination.
const VALUE: u64 = 100_001_000;
const HEIGHT: u32 = 1_000;

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

struct Silent;

impl crate::EventSink for Silent {
    fn emit(&self, _: crate::EngineEvent) {}
}

fn cj() -> AccountType {
    AccountType::CoinJoin { index: 0 }
}

fn bip44() -> AccountType {
    AccountType::Standard {
        index: 0,
        standard_account_type: key_wallet::account::StandardAccountType::BIP44Account,
    }
}

/// Unreachable SPV peer, DAPI and quorum service: no network.
fn offline() -> crate::SessionOptions {
    crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

/// A real engine on `data_root` and its offline regtest session.
fn open_engine(data_root: std::path::PathBuf) -> (Engine, Arc<NetworkSession>) {
    use crate::EngineConfig;
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
    let engine = Engine::new(
        EngineConfig {
            data_root,
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(Silent),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .unwrap();
    (engine, session)
}

/// A real engine and offline regtest session with one imported wallet.
fn engine_wallet() -> (tempfile::TempDir, Engine, Arc<NetworkSession>, WalletId) {
    use crate::ImportOptions;
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap();
    (dir, engine, session, id)
}

/// The CoinJoin account's external address at `index`, derived on a copy of
/// its pool (the wallet's own pool is left as it is).
fn coinjoin_address(wallet: &Wallet, info: &ManagedWalletInfo, index: u32) -> Address {
    let mut pool = (info.accounts.funds_account(&cj()).unwrap())
        .managed_account_type()
        .address_pools()[0]
        .clone();
    let have = pool.highest_generated.unwrap_or(0);
    if index > have {
        pool.generate_addresses(index - have, &wallet.key_source_for_account(cj()), true)
            .unwrap();
    }
    pool.address_at_index(index).unwrap()
}

/// The CoinJoin addresses at `indexes`, from wallet `id` as the session holds it.
fn coinjoin_addresses(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    indexes: &[u32],
) -> Vec<Address> {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        (indexes.iter())
            .map(|i| coinjoin_address(wallet, info, *i))
            .collect()
    })
}

fn pays(n: u8, to: &Address) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: VALUE,
            script_pubkey: to.script_pubkey(),
        }],
        special_transaction_payload: None,
    }
}

fn confirmed_coin(tx: &Transaction, to: &Address) -> Utxo {
    let mut coin = Utxo::new(
        OutPoint::new(tx.txid(), 0),
        tx.output[0].clone(),
        to.clone(),
        HEIGHT,
        false,
    );
    coin.is_confirmed = true;
    coin
}

/// Writes `unspent` and `spent` to wallet `id`'s store as SPV would: no pool
/// row for any of their addresses (the persister never writes the rows of
/// addresses SPV derives).
fn store_coins(session: &NetworkSession, id: WalletId, unspent: Vec<Utxo>, spent: Vec<Utxo>) {
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let live = session.live().unwrap();
    let mut new_utxos = unspent;
    new_utxos.extend(spent.iter().cloned());
    let core = CoreChangeSet {
        new_utxos,
        spent_utxos: spent,
        last_processed_height: Some(HEIGHT + 300),
        synced_height: Some(HEIGHT + 300),
        ..Default::default()
    };
    live.persister
        .store(
            id.0,
            PlatformWalletChangeSet {
                core: Some(core),
                ..Default::default()
            },
        )
        .unwrap();
    live.persister.flush(id.0).unwrap();
}

/// Closes the network and opens it again, offline: the wallet is loaded
/// from its store.
fn reopen(engine: &Engine, session: Arc<NetworkSession>) -> Arc<NetworkSession> {
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .unwrap()
}

fn holders(info: &ManagedWalletInfo, outpoint: &OutPoint) -> Vec<AccountType> {
    (info.accounts.all_funding_accounts().into_iter())
        .filter(|a| a.utxos.contains_key(outpoint))
        .map(|a| a.managed_account_type().to_account_type())
        .collect()
}

/// Whether `account` can sign for `address`: a build that selects the coin
/// finds its derivation path there.
fn derives(info: &ManagedWalletInfo, account: &AccountType, address: &Address) -> bool {
    (info.accounts.funds_account(account)).is_some_and(|a| {
        a.managed_account_type()
            .get_address_derivation_path(address)
            .is_some()
    })
}

/// Replays `txs` in a block, as a rescan, an abandon's rescan or a crash
/// catch-up does, then reads the wallet into the hub.
fn replay(engine: &Engine, session: &NetworkSession, id: WalletId, txs: &[Transaction]) {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let mut wallet = wallet.clone();
            let info = &mut info.core_wallet;
            for tx in txs {
                let context = TransactionContext::InChainLockedBlock(BlockInfo::new(
                    HEIGHT,
                    BlockHash::from_byte_array([1; 32]),
                    1_793_598_712,
                ));
                info.check_core_transaction(tx, context, &mut wallet, true, true)
                    .await;
            }
            info.update_balance();
        }
        session.refresh_wallet_state(&manager, id).await;
    });
}

/// Checks wallet `id`'s coins: each `(outpoint, address)` held once, by
/// `account`, which signs for it; the wallet and the hub count `total`.
fn assert_coins(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    coins: &[(OutPoint, Address)],
    account: AccountType,
    total: u64,
) {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        for (outpoint, address) in coins {
            assert_eq!(holders(info, outpoint), [account], "{address}");
            assert!(derives(info, &account, address), "{address} is signable");
        }
        assert_eq!(info.balance.total(), total);
    });
    assert_eq!(
        session.hub.wallet_state(&id).unwrap().balance.total(),
        total
    );
}

/// A CoinJoin coin past index 99, stored with no pool row next to the spent
/// coin whose address made SPV derive that far, comes back in CoinJoin on a
/// session reopen; replaying both blocks counts it once.
#[test]
fn a_reopened_session_restores_a_coinjoin_coin_past_99_and_a_replay_counts_it_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let addrs = coinjoin_addresses(&engine, &session, id, &[60, 150]);
    let (anchor, deep) = (pays(1, &addrs[0]), pays(2, &addrs[1]));
    store_coins(
        &session,
        id,
        vec![confirmed_coin(&deep, &addrs[1])],
        vec![confirmed_coin(&anchor, &addrs[0])],
    );

    let session = reopen(&engine, session);

    let coin = [(OutPoint::new(deep.txid(), 0), addrs[1].clone())];
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
    replay(&engine, &session, id, &[deep]);
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
}

/// The same through `unload_wallet` and `load_wallet`.
#[test]
fn a_reloaded_wallet_restores_coinjoin_coins_past_99_and_a_replay_counts_them_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let addrs = coinjoin_addresses(&engine, &session, id, &[60, 150, 240]);
    let txs: Vec<Transaction> = (addrs.iter().enumerate())
        .map(|(n, a)| pays(n as u8 + 1, a))
        .collect();
    store_coins(
        &session,
        id,
        (txs.iter().zip(&addrs))
            .map(|(tx, a)| confirmed_coin(tx, a))
            .collect(),
        Vec::new(),
    );
    engine.block_on(session.unload_wallet(id)).unwrap();

    engine.block_on(session.load_wallet(id)).unwrap();

    let coins: Vec<(OutPoint, Address)> = (txs.iter().zip(&addrs))
        .map(|(tx, a)| (OutPoint::new(tx.txid(), 0), a.clone()))
        .collect();
    assert_coins(&engine, &session, id, &coins, cj(), 3 * VALUE);
    replay(&engine, &session, id, &txs);
    assert_coins(&engine, &session, id, &coins, cj(), 3 * VALUE);
}

/// A coin only a stored lookahead's gap reaches (CoinJoin index 500, with no
/// used address below it) is restored into CoinJoin once the lookahead
/// applies, so a replay with that gap counts it once.
#[test]
fn a_stored_lookahead_restores_a_coin_only_its_gap_reaches() {
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(session.store_lookahead(id, crate::keys::MAX_LOOKAHEAD));
    let to = coinjoin_addresses(&engine, &session, id, &[500]).remove(0);
    let tx = pays(1, &to);
    store_coins(&session, id, vec![confirmed_coin(&tx, &to)], Vec::new());

    let session = reopen(&engine, session);

    let coin = [(OutPoint::new(tx.txid(), 0), to)];
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
    replay(&engine, &session, id, &[tx]);
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
}

/// A coin no account reaches within its gap (CoinJoin index 5,000, nothing
/// used below it) stays in the first funds account, counted once, and a
/// replay of its block reaches it no further.
#[test]
fn a_coin_no_gap_reaches_stays_counted_once_through_a_replay() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = coinjoin_addresses(&engine, &session, id, &[5_000]).remove(0);
    let tx = pays(1, &to);
    store_coins(&session, id, vec![confirmed_coin(&tx, &to)], Vec::new());

    let session = reopen(&engine, session);
    replay(&engine, &session, id, std::slice::from_ref(&tx));

    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        assert_eq!(holders(info, &OutPoint::new(tx.txid(), 0)), [bip44()]);
        assert_eq!(info.balance.total(), VALUE);
    });
    assert_eq!(
        session.hub.wallet_state(&id).unwrap().balance.total(),
        VALUE
    );
}

/// Copies directory `from` into `to` with private modes (0700 dirs, 0600
/// files), as the storage permission check wants under any umask.
fn copy_private(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_private(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }
}

/// The data root dw 0.1.0 (v0.1.0, 67b6dd4b) wrote for [`PHRASE`] and then
/// reloaded itself: CoinJoin pool rows 0..99 only, a spent coin at CoinJoin
/// index 60, an unspent one at CoinJoin index 150 (no pool row) and one at
/// BIP44 index 0 (`tests/fixtures/README.md`). It loads with no migration:
/// the coin past 99 in CoinJoin, signable from there, each coin counted once
/// before and after a replay of their blocks.
#[test]
fn a_0_1_0_store_loads_its_coinjoin_coin_past_99_into_coinjoin() {
    let dir = dw_testutil::private_tempdir();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0_1_0-regtest");
    copy_private(&fixture, &dir.path().join("data"));
    let (engine, session) = open_engine(dir.path().join("data"));
    let id: WalletId = "0ff35f248c69653efce8336ce14b78e29cd3bbaf4162c42246259850af2c2add"
        .parse()
        .unwrap();

    let (deep_addr, bip_addr) = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let bip = (info.accounts.funds_account(&bip44()).unwrap())
            .managed_account_type()
            .address_pools()[0]
            .address_at_index(0)
            .unwrap();
        (coinjoin_address(wallet, info, 150), bip)
    });
    let (deep, bip) = (pays(2, &deep_addr), pays(3, &bip_addr));
    let check = |session: &NetworkSession| {
        assert_coins(
            &engine,
            session,
            id,
            &[(OutPoint::new(deep.txid(), 0), deep_addr.clone())],
            cj(),
            2 * VALUE,
        );
        assert_coins(
            &engine,
            session,
            id,
            &[(OutPoint::new(bip.txid(), 0), bip_addr.clone())],
            bip44(),
            2 * VALUE,
        );
    };
    check(&session);
    replay(&engine, &session, id, &[deep.clone(), bip.clone()]);
    check(&session);
}

/// The same through `unload_wallet` and `load_wallet`: opening a closed
/// wallet applies its stored lookahead too.
#[test]
fn a_reloaded_wallet_with_a_stored_lookahead_restores_a_coin_only_its_gap_reaches() {
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(session.store_lookahead(id, crate::keys::MAX_LOOKAHEAD));
    let to = coinjoin_addresses(&engine, &session, id, &[500]).remove(0);
    let tx = pays(1, &to);
    store_coins(&session, id, vec![confirmed_coin(&tx, &to)], Vec::new());
    engine.block_on(session.unload_wallet(id)).unwrap();

    engine.block_on(session.load_wallet(id)).unwrap();

    let coin = [(OutPoint::new(tx.txid(), 0), to)];
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
    replay(&engine, &session, id, &[tx]);
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
}

/// A lookahead raised while the wallet is open, as the CoinJoin recovery
/// scan raises it before its rescan, restores a coin the load left outside
/// its account into CoinJoin, so the replay counts it once.
#[test]
fn a_lookahead_raised_after_the_load_restores_a_coin_before_its_replay() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = coinjoin_addresses(&engine, &session, id, &[500]).remove(0);
    let tx = pays(1, &to);
    store_coins(&session, id, vec![confirmed_coin(&tx, &to)], Vec::new());
    let session = reopen(&engine, session);
    let outpoint = OutPoint::new(tx.txid(), 0);
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        assert_eq!(
            holders(info, &outpoint),
            [bip44()],
            "out of the default gap"
        );
    });

    engine.block_on(async {
        let manager = session.manager().unwrap();
        crate::keys::raise_lookahead(&manager, id, crate::keys::MAX_LOOKAHEAD)
            .await
            .unwrap();
        session.refresh_wallet_state(&manager, id).await;
    });

    let coin = [(outpoint, to)];
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
    replay(&engine, &session, id, &[tx]);
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
}
