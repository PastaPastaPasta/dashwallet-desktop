//! A wallet's coins come back from its store in the funds account that
//! derives their address: platform-wallet-storage's rehydrate (key-wallet
//! `restore_utxos`) within the default gaps, the engine's restore
//! (`crate::restore`) past them, through a stored or raised lookahead, a
//! spent coin's stored address or the search window. A replay of their
//! blocks (a rescan, an abandon's rescan, a crash catch-up) counts each once,
//! and the duplicate reconcile after block events stays as the safety net.

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
use crate::{DashNetwork, Engine, EngineError, WalletId};

/// A 1.00001 DASH CoinJoin denomination.
const VALUE: u64 = 100_001_000;
const HEIGHT: u32 = 1_000;

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// For tests that make the engine fail for one wallet id: no other test
/// imports it.
const HOOK_PHRASE: &[u8] =
    b"legal winner thank year wave sausage worth useful legal winner thank yellow";

/// As [`HOOK_PHRASE`], for the test whose marker cannot be read at open.
const MARKER_READ_PHRASE: &[u8] =
    b"jelly better achieve collect unaware mountain thought cargo oxygen act hood bridge";

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
    open_engine_with(data_root, offline())
}

fn open_engine_with(
    data_root: std::path::PathBuf,
    options: crate::SessionOptions,
) -> (Engine, Arc<NetworkSession>) {
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
        .block_on(engine.open_network(DashNetwork::Regtest, options))
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
    coinjoin_pool_address(wallet, info, 0, index)
}

fn coinjoin_pool_address(
    wallet: &Wallet,
    info: &ManagedWalletInfo,
    pool_index: usize,
    index: u32,
) -> Address {
    let mut pool = (info.accounts.funds_account(&cj()).unwrap())
        .managed_account_type()
        .address_pools()[pool_index]
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
    replay_at(engine, session, id, txs, HEIGHT);
}

/// [`replay`] in a block at `height`.
fn replay_at(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    txs: &[Transaction],
    height: u32,
) {
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
                    height,
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

/// A restored wallet finds CoinJoin coins at index 100 and past it, on both
/// chains, and so does the same wallet reopened. The default gap derives
/// only through index 99, so those keys are outside it until the import and
/// the load widen the account.
#[test]
fn an_imported_and_reopened_wallet_finds_coinjoin_coins_at_index_100_and_beyond() {
    let (_dir, engine, session, id) = engine_wallet();
    let external = coinjoin_addresses(&engine, &session, id, &[100, 240]);
    let internal = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        coinjoin_pool_address(wallet, info, 1, 100)
    });
    let txs = [
        pays(1, &external[0]),
        pays(2, &external[1]),
        pays(3, &internal),
    ];
    replay(&engine, &session, id, &txs);
    let coins = [
        (OutPoint::new(txs[0].txid(), 0), external[0].clone()),
        (OutPoint::new(txs[1].txid(), 0), external[1].clone()),
        (OutPoint::new(txs[2].txid(), 0), internal.clone()),
    ];
    assert_coins(&engine, &session, id, &coins, cj(), 3 * VALUE);

    let session = reopen(&engine, session);
    let further = coinjoin_addresses(&engine, &session, id, &[400]).remove(0);
    let later = pays(4, &further);
    replay(&engine, &session, id, std::slice::from_ref(&later));
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(later.txid(), 0), further)],
        cj(),
        VALUE,
    );
}

/// An offline regtest session without Platform, whose SPV client can run.
fn open_spv_engine(data_root: std::path::PathBuf) -> (Engine, Arc<NetworkSession>) {
    open_engine_with(
        data_root,
        crate::SessionOptions {
            no_platform: true,
            ..offline()
        },
    )
}

fn create_vault(engine: &Engine, session: &Arc<NetworkSession>) {
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
}

fn import_from_genesis(engine: &Engine, session: &Arc<NetworkSession>) -> WalletId {
    engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap()
}

fn start_offline_spv(engine: &Engine, session: &Arc<NetworkSession>) {
    engine.block_on(session.start_spv()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while session.spv_state().unwrap() != crate::SpvState::Running {
        assert!(std::time::Instant::now() < deadline, "SPV did not start");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The BIP158 filter of a block holding `tx` alone, and the block's hash.
fn block_filter(nonce: u32, tx: &Transaction) -> (BlockHash, dashcore::bip158::BlockFilter) {
    let block = dashcore::Block {
        header: dashcore::block::Header {
            version: dashcore::block::Version::ONE,
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: dashcore::TxMerkleNode::all_zeros(),
            time: 1_793_598_712,
            bits: dashcore::CompactTarget::from_consensus(0x207f_ffff),
            nonce,
        },
        txdata: vec![tx.clone()],
    };
    let mut raw = Vec::new();
    let mut writer = dashcore::bip158::BlockFilterWriter::new(&mut raw, &block);
    writer.add_output_scripts();
    writer.finish().unwrap();
    (block.block_hash(), dashcore::bip158::BlockFilter::new(&raw))
}

/// Whether SPV, querying with `scripts`, downloads the block of `filter`.
fn matches_filter(
    (hash, filter): &(BlockHash, dashcore::bip158::BlockFilter),
    scripts: &[dashcore::ScriptBuf],
) -> bool {
    let mut query = dashcore::bip158::FilterQuery::new();
    for script in scripts {
        query.push(script.as_bytes());
    }
    filter.match_any(hash, &query).unwrap()
}

/// What wallet `id` gives SPV to match filters with.
fn scan_scripts(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> Vec<dashcore::ScriptBuf> {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        (wm.get_wallet_info(&id.0).unwrap().core_wallet).scan_script_pubkeys()
    })
}

fn synced_height(engine: &Engine, session: &NetworkSession, id: WalletId) -> u32 {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        (wm.get_wallet_info(&id.0).unwrap().core_wallet).synced_height()
    })
}

/// A batch SPV commits up to `height` for wallet `id`.
fn commit_batch(engine: &Engine, session: &NetworkSession, id: WalletId, height: u32) {
    use platform_wallet::key_wallet_manager::WalletInterface;
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        wm.update_wallet_synced_height(&id.0, height);
        wm.update_wallet_last_processed_height(&id.0, height);
    });
}

/// Puts wallet `id`'s CoinJoin account back to the pools a fresh wallet
/// publishes: gap 100, indexes 0 to 99.
fn narrow_coinjoin(engine: &Engine, session: &NetworkSession, id: WalletId) {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
        let mut fresh = key_wallet::managed_account::ManagedCoreFundsAccount::from_account(
            wallet.accounts.account_of_type(cj()).unwrap(),
        );
        let accounts = &mut info.core_wallet.accounts.coinjoin_accounts;
        for _ in 0..=accounts[&0].monitor_revision() {
            fresh.bump_monitor_revision();
        }
        *accounts.get_mut(&0).unwrap() = fresh;
    });
    assert_eq!(gap(engine, session, id, cj()), 100);
}

/// The filters of two blocks paying CoinJoin index 100, on the receive and
/// on the change chain, of the wallet of `PHRASE`.
fn filters_past_99() -> [(BlockHash, dashcore::bip158::BlockFilter); 2] {
    let mnemonic =
        key_wallet::mnemonic::Mnemonic::from_phrase(std::str::from_utf8(PHRASE).unwrap()).unwrap();
    let wallet = Wallet::from_mnemonic(
        mnemonic,
        dashcore::Network::Regtest,
        key_wallet::wallet::initialization::WalletAccountCreationOptions::Default,
    )
    .unwrap();
    let info = ManagedWalletInfo::from_wallet(&wallet, 0);
    [0, 1].map(|pool| {
        let to = coinjoin_pool_address(&wallet, &info, pool, 100);
        block_filter(pool as u32, &pays(20 + pool as u8, &to))
    })
}

/// The public rescan widens a CoinJoin pool left at the default gap before
/// it rewinds: the filters of blocks paying index 100, on either chain, then
/// match the scan's scripts, and the coins they deliver are counted.
#[test]
fn a_public_rescan_matches_filters_paying_coinjoin_index_100() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_from_genesis(&engine, &session);
    start_offline_spv(&engine, &session);
    let (external, internal) = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        (
            coinjoin_pool_address(wallet, info, 0, 100),
            coinjoin_pool_address(wallet, info, 1, 100),
        )
    });
    narrow_coinjoin(&engine, &session, id);
    commit_batch(&engine, &session, id, HEIGHT + 100);
    let txs = [pays(11, &external), pays(12, &internal)];
    let filters = [block_filter(11, &txs[0]), block_filter(12, &txs[1])];
    let before = scan_scripts(&engine, &session, id);
    assert!(filters.iter().all(|f| !matches_filter(f, &before)));

    engine
        .block_on(session.rescan(crate::RescanFrom::Genesis))
        .unwrap();

    assert_eq!(synced_height(&engine, &session, id), 0);
    let after = scan_scripts(&engine, &session, id);
    assert!(filters.iter().all(|f| matches_filter(f, &after)));
    replay(&engine, &session, id, &txs);
    assert_coins(
        &engine,
        &session,
        id,
        &[
            (OutPoint::new(txs[0].txid(), 0), external),
            (OutPoint::new(txs[1].txid(), 0), internal),
        ],
        cj(),
        2 * VALUE,
    );
}

/// platform-wallet publishes an imported wallet to SPV with the default
/// CoinJoin gap. A batch SPV commits before the import widens it matched
/// the narrow pools, so the import rewinds the wallet to its birth, with a
/// replay record, and the next scan matches index 100.
#[test]
fn an_import_rescans_a_batch_spv_committed_before_the_coinjoin_gap_was_widened() {
    use platform_wallet::key_wallet_manager::WalletInterface;
    use std::sync::atomic::{AtomicBool, Ordering};
    let filters = filters_past_99();
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let matched_when_published = Arc::new(AtomicBool::new(true));
    {
        let live = session.live().unwrap();
        let wm = live.manager.wallet_manager_arc();
        let matched = Arc::clone(&matched_when_published);
        let filters = filters.clone();
        *live.store.on_register.lock().unwrap() = Some(Box::new(move |wallet| {
            let mut wm = wm.try_write().expect("registered, the manager unlocked");
            let scripts = (wm.get_wallet_info(&wallet).unwrap().core_wallet).scan_script_pubkeys();
            matched.store(
                filters.iter().any(|f| matches_filter(f, &scripts)),
                Ordering::SeqCst,
            );
            wm.update_wallet_synced_height(&wallet, HEIGHT);
            wm.update_wallet_last_processed_height(&wallet, HEIGHT);
        }));
    }

    let id = import_from_genesis(&engine, &session);

    assert!(!matched_when_published.load(Ordering::SeqCst));
    assert_eq!(synced_height(&engine, &session, id), 0);
    let scripts = scan_scripts(&engine, &session, id);
    assert!(filters.iter().all(|f| matches_filter(f, &scripts)));
    let record = engine
        .block_on(session.live().unwrap().replay.get(id))
        .unwrap()
        .unwrap();
    assert_eq!((record.reached, record.target), (0, HEIGHT));
}

/// The same for a wallet opened again: `load_from_persistor` publishes it
/// with the default CoinJoin gap, and a batch SPV commits before the load
/// widens it is rescanned from the checkpoint the wallet was loaded with.
#[test]
fn a_load_rescans_a_batch_spv_committed_before_the_coinjoin_gap_was_widened() {
    let filters = filters_past_99();
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(session.unload_wallet(id)).unwrap();
    let live = session.live().unwrap();
    live.store.set_unloaded(id.0, false);
    engine.block_on(live.manager.load_from_persistor()).unwrap();
    let loaded_at = live.store.loaded_checkpoint(&id.0).unwrap();
    assert_eq!(synced_height(&engine, &session, id), loaded_at);
    narrow_coinjoin(&engine, &session, id);
    commit_batch(&engine, &session, id, loaded_at + 50);
    let before = scan_scripts(&engine, &session, id);
    assert!(filters.iter().all(|f| !matches_filter(f, &before)));

    engine
        .block_on(session.restore_loaded_wallet(&live.manager, id))
        .unwrap();

    assert_eq!(synced_height(&engine, &session, id), loaded_at);
    let after = scan_scripts(&engine, &session, id);
    assert!(filters.iter().all(|f| matches_filter(f, &after)));
    let record = engine.block_on(live.replay.get(id)).unwrap().unwrap();
    assert_eq!((record.reached, record.target), (loaded_at, loaded_at + 50));
}

/// A rescan waits for an unload in flight, then widens and rewinds the
/// wallets still open.
#[test]
fn a_rescan_waits_for_an_unload_and_widens_the_wallets_left() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let w1 = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    let w2 = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    start_offline_spv(&engine, &session);
    narrow_coinjoin(&engine, &session, w2);

    engine.block_on(async {
        let live = session.live().unwrap();
        let wm = live.manager.wallet_manager_arc();
        let hold = wm.read().await;
        let unload = {
            let s = Arc::clone(&session);
            session.rt.spawn(async move { s.unload_wallet(w1).await })
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        while !live.store.is_unloaded(&w1.0) {
            assert!(tokio::time::Instant::now() < deadline, "the unload began");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let rescan = {
            let s = Arc::clone(&session);
            (session.rt).spawn(async move { s.rescan(crate::RescanFrom::Genesis).await })
        };
        while session.wallet_lifecycle.waiting() == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the rescan waits for the unload"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        drop(hold);
        unload.await.unwrap().unwrap();
        rescan.await.unwrap().unwrap();
    });
    assert!(
        engine
            .block_on(session.manager().unwrap().get_wallet(&w1.0))
            .is_none()
    );
    assert_eq!(
        gap(&engine, &session, w2, cj()),
        crate::keys::COINJOIN_SCAN_GAP
    );
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
        // The spent coin's address stays used: never handed out again.
        let spent = coinjoin_address(wallet, info, 60);
        let pools = info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type();
        assert!(
            (pools.address_pools().iter())
                .any(|p| p.address_info(&spent).is_some_and(|a| a.is_used())),
            "CoinJoin 60 is used"
        );
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
/// its account (CoinJoin index 1,500: past the default gap and the search
/// window past it) into CoinJoin, so the replay counts it once.
#[test]
fn a_lookahead_raised_after_the_load_restores_a_coin_before_its_replay() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = coinjoin_addresses(&engine, &session, id, &[1_500]).remove(0);
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
            "out of the default gap and the search"
        );
    });

    raise(&engine, &session, id);

    let coin = [(outpoint, to)];
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
    replay(&engine, &session, id, &[tx]);
    assert_coins(&engine, &session, id, &coin, cj(), VALUE);
}

/// The CoinJoin recovery scan's raise before its rescan
/// ([`NetworkSession::raise_for_recovery`]): wallet `id`'s lookahead to the
/// most, then the hub reads the wallet.
fn recover(engine: &Engine, session: &NetworkSession, id: WalletId) -> Result<(), EngineError> {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        session.raise_for_recovery(&manager, id).await
    })
}

/// [`recover`], which must succeed.
fn raise(engine: &Engine, session: &NetworkSession, id: WalletId) {
    recover(engine, session, id).unwrap();
}

/// [`recover`] with its restore failing ([`crate::restore::FAIL_RESTORE`]).
fn recover_failing_restore(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> Result<(), EngineError> {
    engine.block_on(crate::restore::FAIL_RESTORE.scope((), async {
        let manager = session.manager().unwrap();
        session.raise_for_recovery(&manager, id).await
    }))
}

/// Adds BIP44 account 1 to wallet `id`'s managed accounts only: the wallet
/// has no such account, so raising its lookahead fails, between BIP44 0 and
/// CoinJoin 0, which are raised.
fn insert_unraisable_account(engine: &Engine, session: &NetworkSession, id: WalletId) {
    use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[9; 64]).unwrap();
    let managed =
        key_wallet::managed_account::ManagedCoreFundsAccount::from_account(&key_wallet::Account {
            parent_wallet_id: Some(id.0),
            account_type: AccountType::Standard {
                index: 1,
                standard_account_type: key_wallet::account::StandardAccountType::BIP44Account,
            },
            network: dashcore::Network::Regtest,
            account_xpub: ExtendedPubKey::from_priv(&master),
            is_watch_only: true,
        });
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (_, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
        (info.core_wallet.accounts)
            .insert_funds_bearing_account(managed)
            .unwrap();
    });
}

/// Moves `outpoint` from CoinJoin to BIP44 in the managed wallet, where a
/// load that missed its address leaves it.
fn misplace(engine: &Engine, session: &NetworkSession, id: WalletId, outpoint: &OutPoint) {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = &mut wm.get_wallet_and_info_mut(&id.0).unwrap().1.core_wallet;
        let utxo = (info.accounts.funds_account_mut(&cj()).unwrap())
            .utxos
            .remove(outpoint)
            .unwrap();
        (info.accounts.funds_account_mut(&bip44()).unwrap())
            .utxos
            .insert(*outpoint, utxo);
        info.update_balance();
        assert_eq!(holders(info, outpoint), [bip44()]);
    });
}

/// Zeroes the hub's balance of wallet `id`, so only a read of the wallet
/// shows it again.
fn stale_hub(session: &NetworkSession, id: WalletId) {
    (session.hub).update_wallet_state(id, |s| s.balance = Default::default());
    assert_eq!(session.hub.wallet_state(&id).unwrap().balance.total(), 0);
}

/// The gap limit of `account`'s first pool in wallet `id`.
fn gap(engine: &Engine, session: &NetworkSession, id: WalletId, account: AccountType) -> u32 {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let funds = info.accounts.funds_account(&account).unwrap();
        funds.managed_account_type().address_pools()[0].gap_limit
    })
}

/// Sol r1 M1: a CoinJoin coin at index 1,500 whose address a scan with a
/// 1,000 lookahead reached only through the spent coin at 600 (stored, no
/// pool row for either). Returns the coin's outpoint and address, and the
/// replay of their history: the bridge's funding, the coin's, the bridge's
/// spend.
fn store_spent_bridge(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> ((OutPoint, Address), Vec<Transaction>) {
    let addrs = coinjoin_addresses(engine, session, id, &[600, 1_500]);
    let (bridge, deep) = (pays(1, &addrs[0]), pays(2, &addrs[1]));
    store_coins(
        session,
        id,
        vec![confirmed_coin(&deep, &addrs[1])],
        vec![confirmed_coin(&bridge, &addrs[0])],
    );
    let spend = spends(&bridge);
    let coin = (OutPoint::new(deep.txid(), 0), addrs[1].clone());
    (coin, vec![bridge, deep, spend])
}

/// A transaction spending output 0 of `prev`, less a fee, to `foreign()`.
fn spends(prev: &Transaction) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(prev.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: VALUE - 1_000,
            script_pubkey: foreign().script_pubkey(),
        }],
        special_transaction_payload: None,
    }
}

/// Asserts the one coin is CoinJoin's, counted once, at `VALUE`.
fn assert_home(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    coin: &(OutPoint, Address),
) {
    assert_coins(engine, session, id, std::slice::from_ref(coin), cj(), VALUE);
}

/// An address of no account of the wallet's.
fn foreign() -> Address {
    Address::p2pkh(
        &dashcore::PublicKey::from_slice(&[2; 33]).unwrap(),
        dashcore::Network::Regtest,
    )
}

/// Sol r1 M1, session reopen: with the stored lookahead applied, the coin
/// past the spent bridge comes back in CoinJoin before anything replays it,
/// and the replay of its whole history (the bridge's funding and spend
/// included) leaves one copy and the exact balance.
#[test]
fn a_reopened_session_restores_a_coin_past_a_spent_bridge_and_a_replay_counts_it_once() {
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(session.store_lookahead(id, crate::keys::MAX_LOOKAHEAD));
    let (coin, history) = store_spent_bridge(&engine, &session, id);

    let session = reopen(&engine, session);

    assert_home(&engine, &session, id, &coin);
    replay(&engine, &session, id, &history);
    assert_home(&engine, &session, id, &coin);
}

/// Sol r1 M1, through `unload_wallet` and `load_wallet`.
#[test]
fn a_reloaded_wallet_restores_a_coin_past_a_spent_bridge_and_a_replay_counts_it_once() {
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(session.store_lookahead(id, crate::keys::MAX_LOOKAHEAD));
    let (coin, history) = store_spent_bridge(&engine, &session, id);
    engine.block_on(session.unload_wallet(id)).unwrap();

    engine.block_on(session.load_wallet(id)).unwrap();

    assert_home(&engine, &session, id, &coin);
    replay(&engine, &session, id, &history);
    assert_home(&engine, &session, id, &coin);
}

/// Sol r1 M1, with no stored lookahead: the load's search finds the spent
/// bridge's stored address and, past its gap, the coin; the recovery scan's
/// raise keeps it there, and the replay counts it once.
#[test]
fn a_raised_lookahead_keeps_a_coin_past_a_spent_bridge_counted_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let (coin, history) = store_spent_bridge(&engine, &session, id);
    let session = reopen(&engine, session);
    assert_home(&engine, &session, id, &coin);

    raise(&engine, &session, id);

    assert_home(&engine, &session, id, &coin);
    replay(&engine, &session, id, &history);
    assert_home(&engine, &session, id, &coin);
}

/// Sol r1 M1 through the recovery scan's raise, partially failing: the coin
/// past the spent bridge, left in BIP44, and an account the raise can't
/// reach. The other accounts are raised, the restore still takes the coin
/// home, the hub reads the wallet before the error (naming that account)
/// returns, and a replay counts the coin once.
#[test]
fn a_partial_recovery_raise_still_takes_a_coin_past_a_spent_bridge_home_counted_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let (coin, history) = store_spent_bridge(&engine, &session, id);
    let session = reopen(&engine, session);
    misplace(&engine, &session, id, &coin.0);
    insert_unraisable_account(&engine, &session, id);
    stale_hub(&session, id);

    let error = recover(&engine, &session, id).unwrap_err().to_string();

    assert!(error.contains("BIP44 #1"), "{error}");
    let raised = crate::coinjoin::RECOVERY_LOOKAHEAD;
    assert_eq!(gap(&engine, &session, id, cj()), raised);
    assert_eq!(gap(&engine, &session, id, bip44()), raised);
    assert_home(&engine, &session, id, &coin);
    replay(&engine, &session, id, &history);
    assert_home(&engine, &session, id, &coin);
}

/// Sol r1 M1 through the recovery scan's raise, its restore failing: the
/// coin past the spent bridge stays where the load left it (BIP44), counted
/// once; the hub reads the wallet before the error returns. A replay copies
/// it into CoinJoin; the replay's block event starts
/// the reconcile, which leaves one copy, at home. The next load keeps it
/// there.
#[test]
fn a_failed_recovery_restore_keeps_a_coin_past_a_spent_bridge_counted_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let (coin, history) = store_spent_bridge(&engine, &session, id);
    let session = reopen(&engine, session);
    misplace(&engine, &session, id, &coin.0);
    stale_hub(&session, id);

    let error = recover_failing_restore(&engine, &session, id)
        .unwrap_err()
        .to_string();

    assert!(error.contains("restoring coins past the gap"), "{error}");
    assert!(!error.contains("raising the lookahead"), "{error}");
    assert_eq!(
        session.hub.wallet_state(&id).unwrap().balance.total(),
        VALUE
    );
    // The replay copies the coin into CoinJoin; its block event starts the
    // reconcile, which keeps that copy only.
    replay(&engine, &session, id, &history);
    let pw = engine
        .block_on(session.manager().unwrap().get_wallet(&id.0))
        .unwrap();
    deliver(&session, &pw, id, 2 * VALUE);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while session.hub.wallet_state(&id).unwrap().balance.total() != VALUE {
        assert!(std::time::Instant::now() < deadline, "no reconcile ran");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_home(&engine, &session, id, &coin);
    drop(pw);
    let session = reopen(&engine, session);
    assert_home(&engine, &session, id, &coin);
}

/// The recovery scan's raise with both its raise and its restore failing:
/// one error names both, the restore's first.
#[test]
fn a_recovery_raise_failing_twice_names_both_failures() {
    let (_dir, engine, session, id) = engine_wallet();
    insert_unraisable_account(&engine, &session, id);
    stale_hub(&session, id);

    let error = recover_failing_restore(&engine, &session, id)
        .unwrap_err()
        .to_string();

    assert!(error.contains("restoring coins past the gap"), "{error}");
    assert!(error.contains("raising the lookahead to 1000"), "{error}");
    assert!(error.contains("BIP44 #1"), "{error}");
}

/// Signs, through the session, a PSBT spending `outpoint` of `prev`: whether
/// the wallet signed it (it has the coin's key and derivation path), or
/// why it refused.
fn signs(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    prev: &Transaction,
) -> Result<bool, EngineError> {
    use dw_vault::{Credential, GrantPurpose};
    let unsigned = dw_psbt::create_unsigned(
        spends(prev),
        vec![dw_psbt::InputData {
            prev_tx: prev.clone(),
            derivation: None,
        }],
        vec![None],
    )
    .unwrap();
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend { max_duffs: VALUE },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    (engine.block_on(session.sign_psbt(id, unsigned, grant)))
        .map(|mut signed| dw_psbt::finalize(&mut signed))
}

/// Sol r1 M2 (the deleted `a_real_load_marks_a_sparse_bip44_coin_used`): a
/// BIP44 coin at external index 79, written by the real persister with no
/// pool row and no used address near it, no lookahead stored. After
/// `unload_wallet` and `load_wallet` its address is derived and used, the gap
/// is kept past it, it counts once, and the wallet signs a spend of it once
/// the replay the load started from the birth has caught up.
#[test]
fn a_real_load_restores_a_sparse_bip44_coin_spendable() {
    let (_dir, engine, session, id) = engine_wallet();
    let (to, index) = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = wm.get_wallet_info(&id.0).unwrap();
        let mut pool = (info.core_wallet.accounts.funds_account(&bip44()).unwrap())
            .managed_account_type()
            .address_pools()[0]
            .clone();
        let index = pool.highest_generated.unwrap() + 50;
        pool.generate_addresses(50, &wallet.key_source_for_account(bip44()), true)
            .unwrap();
        (pool.address_at_index(index).unwrap(), index)
    });
    assert_eq!(index, 79);
    let tx = pays(1, &to);
    store_coins(&session, id, vec![confirmed_coin(&tx, &to)], Vec::new());
    engine.block_on(session.unload_wallet(id)).unwrap();

    engine.block_on(session.load_wallet(id)).unwrap();

    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let pool = &info
            .accounts
            .funds_account(&bip44())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        let at = pool.address_info(&to).expect("the load derives it");
        assert!(at.is_used(), "highest_used={:?}", pool.highest_used);
        assert!(
            pool.highest_generated.unwrap() >= index + pool.gap_limit,
            "the gap is kept past it"
        );
    });
    let coin = [(OutPoint::new(tx.txid(), 0), to)];
    assert_coins(&engine, &session, id, &coin, bip44(), VALUE);
    assert!(
        matches!(
            signs(&engine, &session, id, &tx),
            Err(EngineError::RescanInProgress)
        ),
        "signing waits for the load's replay"
    );
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    assert!(
        signs(&engine, &session, id, &tx).unwrap(),
        "the wallet signs a spend of it"
    );
}

/// Opus r1 M2: a watch-only wallet imported with a lookahead keeps it: it is
/// stored, and raised again at every load on the wallet's own BIP44 account
/// (here account 3), so a coin only that gap reaches (external index 1,100:
/// past the default 30 and the search window past it, within 200 and the
/// window) comes back in that account, counted once.
#[test]
fn a_watch_only_lookahead_is_stored_and_raised_on_its_account_at_load() {
    use key_wallet::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let (_dir, engine, session, _) = engine_wallet();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[7; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/3'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = engine
        .block_on(session.import_watch_only(
            xpub.to_string(),
            crate::WatchOnlyOptions {
                lookahead: Some(200),
                ..Default::default()
            },
        ))
        .unwrap();
    let child = xpub
        .derive_pub(&[
            ChildNumber::Normal { index: 0 },
            ChildNumber::Normal { index: 1_100 },
        ])
        .unwrap();
    let to = Address::p2pkh(
        &dashcore::PublicKey::new(child.public_key),
        dashcore::Network::Regtest,
    );
    let tx = pays(1, &to);
    store_coins(&session, id, vec![confirmed_coin(&tx, &to)], Vec::new());

    let session = reopen(&engine, session);

    let account = AccountType::Standard {
        index: 3,
        standard_account_type: key_wallet::account::StandardAccountType::BIP44Account,
    };
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let pools = info
            .accounts
            .funds_account(&account)
            .unwrap()
            .managed_account_type();
        assert_eq!(
            pools.address_pools()[0].gap_limit,
            200,
            "the stored lookahead"
        );
    });
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(tx.txid(), 0), to)],
        account,
        VALUE,
    );
}

/// Delivers `balance` to platform-wallet's balance mirror of wallet `id`
/// and to the hub, as a block event does.
fn deliver(
    session: &NetworkSession,
    pw: &Arc<platform_wallet::PlatformWallet>,
    id: WalletId,
    balance: u64,
) {
    use dash_spv::EventHandler;
    use key_wallet::wallet::balance::WalletCoreBalance;
    use std::collections::BTreeMap;
    let mirrors = Arc::new(Default::default());
    let handler = platform_wallet::wallet::core::BalanceUpdateHandler::new(Arc::clone(&mirrors));
    mirrors.store(Arc::new(BTreeMap::from([(id.0, Arc::clone(pw))])));
    let event = platform_wallet::events::WalletEvent::BlockProcessed {
        wallet_id: id.0,
        height: HEIGHT,
        chain_lock: None,
        inserted: vec![],
        updated: vec![],
        matured: vec![],
        balance: WalletCoreBalance::new(balance, 0, 0, 0),
        account_balances: BTreeMap::new(),
        addresses_derived: vec![],
    };
    handler.on_wallet_event(&event);
    session.hub.apply_wallet_event(&event);
}

/// A copy of a CoinJoin coin in BIP44 too, as a replay of a coin no restore
/// placed leaves it: the balance counts it twice.
fn duplicate(engine: &Engine, session: &NetworkSession, id: WalletId) -> OutPoint {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
        let info = &mut info.core_wallet;
        let to = coinjoin_address(wallet, info, 5);
        let tx = pays(1, &to);
        let coin = confirmed_coin(&tx, &to);
        for account in [bip44(), cj()] {
            (info.accounts.funds_account_mut(&account).unwrap().utxos)
                .insert(coin.outpoint, coin.clone());
        }
        info.update_balance();
        assert_eq!(info.balance.total(), 2 * VALUE);
        coin.outpoint
    })
}

/// A replay's duplicate, then a block event through the hub: the attached
/// reconcile runs on its own, drops the copy the holder can't derive and
/// publishes the deduplicated balance.
#[test]
fn a_block_event_starts_the_attached_reconcile() {
    let (_dir, engine, session, id) = engine_wallet();
    let op = duplicate(&engine, &session, id);
    let pw = engine
        .block_on(session.manager().unwrap().get_wallet(&id.0))
        .unwrap();

    deliver(&session, &pw, id, 2 * VALUE);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while session.hub.wallet_state(&id).unwrap().balance.total() != VALUE {
        assert!(std::time::Instant::now() < deadline, "no reconcile ran");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        assert_eq!(holders(info, &op), [cj()]);
    });
}

/// A replay's doubled balance reaches platform-wallet's mirror; the
/// reconcile corrects key-wallet and the hub; a later refresh (a birth
/// height change, a cancelled rescan, a send) must not bring the doubled
/// total back.
#[test]
fn a_refresh_after_the_reconcile_keeps_the_deduplicated_total() {
    let (_dir, engine, session, id) = engine_wallet();
    duplicate(&engine, &session, id);
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let pw = manager.get_wallet(&id.0).await.unwrap();
        deliver(&session, &pw, id, 2 * VALUE);
        session.reconcile_duplicate_coins(id).await;
        assert_eq!(
            session.hub.wallet_state(&id).unwrap().balance.total(),
            VALUE
        );

        session.refresh_wallet_state(&manager, id).await;

        assert_eq!(
            session.hub.wallet_state(&id).unwrap().balance.total(),
            VALUE
        );
    });
}

/// A reconcile that finds no duplicate takes no write lock (it would block
/// SPV's block processing on every height step), and still publishes
/// key-wallet's balance.
#[test]
fn a_reconcile_without_duplicates_needs_no_write_lock() {
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let reader = wm.read().await;

        let ran = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            session.reconcile_duplicate_coins(id),
        )
        .await;

        drop(reader);
        assert!(ran.is_ok(), "it waited for the write lock");
        assert_eq!(session.hub.wallet_state(&id).unwrap().balance.total(), 0);
    });
}

fn wide_scan(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
) -> Option<crate::keys::WideScan> {
    engine.block_on(session.wide_scan(id)).unwrap()
}

fn set_wide_scan(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    marker: Option<crate::keys::WideScan>,
) {
    engine.block_on(session.set_wide_scan(id, marker));
}

/// Stores wallet `id`'s filter checkpoint at `height`, as a committed SPV
/// batch does, and moves the wallet in memory there too.
fn store_checkpoint(engine: &Engine, session: &NetworkSession, id: WalletId, height: u32) {
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    commit_batch(engine, session, id, height);
    (session.hub).apply_wallet_event(&platform_wallet::events::WalletEvent::SyncHeightAdvanced {
        wallet_id: id.0,
        height,
    });
    let live = session.live().unwrap();
    let core = CoreChangeSet {
        last_processed_height: Some(height),
        synced_height: Some(height),
        ..Default::default()
    };
    let changeset = PlatformWalletChangeSet {
        core: Some(core),
        ..Default::default()
    };
    live.store.store(id.0, changeset).unwrap();
    live.store.flush(id.0).unwrap();
    engine.block_on(live.replay.settled());
}

/// Stores wallet `id`'s checkpoint in wallet.sqlite alone, as a rewind
/// that no committed batch has stored yet leaves it.
fn store_synced_only(session: &NetworkSession, id: WalletId, height: u32) {
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let live = session.live().unwrap();
    let core = CoreChangeSet {
        last_processed_height: Some(height),
        synced_height: Some(height),
        ..Default::default()
    };
    let changeset = PlatformWalletChangeSet {
        core: Some(core),
        ..Default::default()
    };
    live.store.store(id.0, changeset).unwrap();
    live.store.flush(id.0).unwrap();
}

fn replay_record(engine: &Engine, session: &NetworkSession, id: WalletId) -> Option<(u32, u32)> {
    let live = session.live().unwrap();
    (engine.block_on(live.replay.get(id)).unwrap()).map(|r| (r.reached, r.target))
}

/// An import marks the wallet before platform-wallet publishes it, and
/// marks it scanned with wide pools once they are widened.
#[test]
fn an_import_marks_the_wallet_before_it_is_published_and_done_once_widened() {
    use std::sync::Mutex;
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let seen = Arc::new(Mutex::new(None));
    {
        let live = session.live().unwrap();
        let appdb = Arc::clone(&live.appdb);
        let seen = Arc::clone(&seen);
        *live.store.on_register.lock().unwrap() = Some(Box::new(move |wallet| {
            let scope = dw_appdb::local_scope(&WalletId(wallet).to_string());
            *seen.lock().unwrap() = appdb
                .setting(&scope, crate::keys::WIDE_SCAN_SETTING)
                .unwrap();
        }));
    }

    let id = import_from_genesis(&engine, &session);

    assert_eq!(seen.lock().unwrap().as_deref(), Some("2:birth"));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// The app quit after SPV stored a batch matched with an import's narrow
/// pools and before the import rewound: the marker the import left makes
/// the next open replay from the wallet's birth.
#[test]
fn a_quit_mid_import_replays_from_the_birth_at_the_next_open() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Birth));

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(replay_record(&engine, &session, id), Some((0, HEIGHT)));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// An unload leaves the wallet's checkpoint as its marker. The app quit
/// after the next load published it and SPV stored a batch above that
/// checkpoint, before the load rewound: the next open replays from it.
#[test]
fn a_quit_mid_load_replays_from_the_checkpoint_the_wallet_was_closed_at() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine.block_on(session.unload_wallet(id)).unwrap();
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(HEIGHT))
    );
    let live = session.live().unwrap();
    live.store.set_unloaded(id.0, false);
    engine.block_on(live.manager.load_from_persistor()).unwrap();
    store_checkpoint(&engine, &session, id, HEIGHT + 50);
    drop(live);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), HEIGHT);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((HEIGHT, HEIGHT + 50))
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A wallet an older build restored and scanned has no marker. Its CoinJoin
/// account was used (a spent coin at index 5), and a coin at index 100 below
/// its checkpoint was never matched. Its first open replays once from its
/// birth, which finds the coin; once that replay is done, the next open
/// replays nothing.
#[test]
fn a_wallet_an_older_build_scanned_is_replayed_once_from_its_birth() {
    let (_dir, engine, session, id) = engine_wallet();
    let [used, to] = coinjoin_addresses(&engine, &session, id, &[5, 100])
        .try_into()
        .unwrap();
    let mixed = pays(2, &used);
    store_coins(
        &session,
        id,
        Vec::new(),
        vec![confirmed_coin(&mixed, &used)],
    );
    let tx = pays(1, &to);
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    set_wide_scan(&engine, &session, id, None);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 300))
    );
    replay(&engine, &session, id, std::slice::from_ref(&tx));
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(tx.txid(), 0), to)],
        cj(),
        VALUE,
    );
    store_checkpoint(&engine, &session, id, HEIGHT + 301);
    assert_eq!(replay_record(&engine, &session, id), None);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), HEIGHT + 301);
    assert_eq!(replay_record(&engine, &session, id), None);
}

/// A wallet created here, with no marker, is replayed once from its birth:
/// the same seed may have been mixed by another app, past the narrow pool.
#[test]
fn a_wallet_created_here_without_a_marker_is_replayed_from_its_birth() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, None);
    let birth = birth_height(&engine, &session, id).saturating_sub(1);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), birth);
    assert_eq!(replay_record(&engine, &session, id), Some((birth, HEIGHT)));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A plain reopen leaves the checkpoint where it was and stores no replay
/// record: no rescan at every launch.
#[test]
fn a_plain_reopen_leaves_the_checkpoint_alone() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT);

    let session = reopen(&engine, session);
    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), HEIGHT);
    assert_eq!(replay_record(&engine, &session, id), None);
}

/// Through `load_wallet`: a batch SPV commits after the load published the
/// wallet with narrow pools, before it widens them, is rescanned from the
/// checkpoint the wallet was loaded with.
#[test]
fn load_wallet_rescans_a_batch_spv_committed_before_it_widened_the_pools() {
    use platform_wallet::key_wallet_manager::WalletInterface;
    use std::sync::atomic::{AtomicBool, Ordering};
    let filters = filters_past_99();
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine.block_on(session.unload_wallet(id)).unwrap();
    let matched_when_published = Arc::new(AtomicBool::new(true));
    {
        let live = session.live().unwrap();
        let wm = live.manager.wallet_manager_arc();
        let matched = Arc::clone(&matched_when_published);
        let filters = filters.clone();
        *session.hub.on_published.lock().unwrap() = Some(Box::new(move |wallet| {
            let mut wm = wm.try_write().expect("published, the manager unlocked");
            let scripts =
                (wm.get_wallet_info(&wallet.0).unwrap().core_wallet).scan_script_pubkeys();
            matched.store(
                filters.iter().any(|f| matches_filter(f, &scripts)),
                Ordering::SeqCst,
            );
            wm.update_wallet_synced_height(&wallet.0, HEIGHT + 50);
            wm.update_wallet_last_processed_height(&wallet.0, HEIGHT + 50);
            false
        }));
    }

    engine.block_on(session.load_wallet(id)).unwrap();

    assert!(!matched_when_published.load(Ordering::SeqCst));
    assert_eq!(synced_height(&engine, &session, id), HEIGHT);
    assert!((filters.iter()).all(|f| matches_filter(f, &scan_scripts(&engine, &session, id))));
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((HEIGHT, HEIGHT + 50))
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A watch-only wallet's lookahead is raised after the load published it:
/// a batch SPV committed in between matched the default gap, so the import
/// rewinds to the checkpoint it was published with.
#[test]
fn a_watch_only_import_rescans_a_batch_committed_before_its_lookahead_was_raised() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    use platform_wallet::key_wallet_manager::WalletInterface;
    let (_dir, engine, session, _) = engine_wallet();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[9; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    {
        let live = session.live().unwrap();
        let wm = live.manager.wallet_manager_arc();
        *session.hub.on_published.lock().unwrap() = Some(Box::new(move |wallet| {
            let mut wm = wm.try_write().expect("published, the manager unlocked");
            wm.update_wallet_synced_height(&wallet.0, HEIGHT);
            wm.update_wallet_last_processed_height(&wallet.0, HEIGHT);
            false
        }));
    }

    let id = engine
        .block_on(session.import_watch_only(
            xpub.to_string(),
            crate::WatchOnlyOptions {
                lookahead: Some(200),
                ..Default::default()
            },
        ))
        .unwrap();

    let published_at = session
        .live()
        .unwrap()
        .store
        .loaded_checkpoint(&id.0)
        .unwrap();
    assert!(published_at < HEIGHT);
    assert_eq!(synced_height(&engine, &session, id), published_at);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((published_at, HEIGHT))
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A watch-only wallet born at `HEIGHT` synced past its birth. A payment to
/// receive index 500, outside its pool, sits in block 10, below that birth.
/// A Dash Core file of its seed attaches the keys with Core's range, which
/// reaches that index, rewinds to the start of Core's import (genesis) and
/// lowers the birth to block 1 first: dash-spv scans from the birth up, and its
/// commit certifies a wallet whose checkpoint is just below a batch's start,
/// so the checkpoint must not sit below the birth. The payment counts.
#[test]
fn a_wallet_dat_attached_to_a_synced_watch_only_wallet_recovers_a_payment_past_its_pool() {
    const PAID_AT: u32 = 10;
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    let to = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let mut pool = (info.accounts.funds_account(&bip44()).unwrap())
            .managed_account_type()
            .address_pools()[0]
            .clone();
        assert!(pool.highest_generated.unwrap_or(0) < 500);
        let have = pool.highest_generated.unwrap_or(0);
        pool.generate_addresses(500 - have, &wallet.key_source_for_account(bip44()), true)
            .unwrap();
        pool.address_at_index(500).unwrap()
    });
    let txs = [pays(31, &to)];
    let filter = block_filter(31, &txs[0]);
    commit_batch(&engine, &session, id, HEIGHT + 100);
    assert!(!matches_filter(
        &filter,
        &scan_scripts(&engine, &session, id)
    ));

    let attached = engine
        .block_on(
            session.import_wallet_dat(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../testdata/compat/walletdat/bdb_wide_range.dat"),
                None,
                crate::ImportOptions::default(),
            ),
        )
        .unwrap();

    assert_eq!(attached.wallet_id, id);
    assert!(session.vault().has_wallet_secret(&id.0));
    let synced = synced_height(&engine, &session, id);
    let birth = birth_height(&engine, &session, id);
    assert_eq!((synced, birth), (0, 1));
    assert!(
        synced + 1 >= birth,
        "dash-spv restarts at the birth, above the checkpoint"
    );
    assert!(birth <= PAID_AT);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 100))
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    assert!(matches_filter(
        &filter,
        &scan_scripts(&engine, &session, id)
    ));
    replay_at(&engine, &session, id, &txs, PAID_AT);
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(txs[0].txid(), 0), to)],
        bip44(),
        VALUE,
    );
    let session = reopen(&engine, session);
    assert_eq!(
        birth_height(&engine, &session, id),
        1,
        "the lowered birth is stored"
    );
}

/// Wallet `id`'s birth height, as dash-spv reads it.
fn birth_height(engine: &Engine, session: &NetworkSession, id: WalletId) -> u32 {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        (wm.get_wallet_info(&id.0).unwrap().core_wallet).birth_height()
    })
}

/// A load whose widening fails keeps the wallet's marker, so the next load
/// widens and replays again instead of leaving coins hidden.
#[test]
fn a_failed_widening_keeps_the_marker_for_the_next_load() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine.block_on(session.unload_wallet(id)).unwrap();
    crate::keys::FAIL_WIDEN.lock().unwrap().push(id.0);

    let loaded = engine.block_on(session.load_wallet(id));

    crate::keys::FAIL_WIDEN
        .lock()
        .unwrap()
        .retain(|w| *w != id.0);
    loaded.unwrap();
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(HEIGHT))
    );
    engine.block_on(session.unload_wallet(id)).unwrap();
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    assert_eq!(
        gap(&engine, &session, id, cj()),
        crate::keys::COINJOIN_SCAN_GAP
    );
}

/// One wallet whose pools fail to widen does not stop the rescan of the
/// others.
#[test]
fn a_rescan_goes_on_when_one_wallet_fails_to_widen() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let w1 = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    let w2 = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    start_offline_spv(&engine, &session);
    narrow_coinjoin(&engine, &session, w2);
    commit_batch(&engine, &session, w2, HEIGHT);
    crate::keys::FAIL_WIDEN.lock().unwrap().push(w1.0);

    let rescanned = engine.block_on(session.rescan(crate::RescanFrom::Genesis));

    crate::keys::FAIL_WIDEN
        .lock()
        .unwrap()
        .retain(|w| *w != w1.0);
    rescanned.unwrap();
    assert_eq!(
        gap(&engine, &session, w2, cj()),
        crate::keys::COINJOIN_SCAN_GAP
    );
    assert_eq!(synced_height(&engine, &session, w2), 0);
}

/// A removed wallet's loaded checkpoint is forgotten at the next load.
#[test]
fn a_removed_wallets_loaded_checkpoint_is_pruned() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let (_dir, engine, session, kept) = engine_wallet();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[11; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = engine
        .block_on(session.import_watch_only(xpub.to_string(), Default::default()))
        .unwrap();
    let live = session.live().unwrap();
    assert!(live.store.loaded_checkpoint(&id.0).is_some());
    let wipe = (session.vault())
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    engine.block_on(session.remove_wallet(id, wipe)).unwrap();

    engine.block_on(live.manager.load_from_persistor()).unwrap();

    assert_eq!(live.store.loaded_checkpoint(&id.0), None);
    assert!(live.store.loaded_checkpoint(&kept.0).is_some());
}

/// A wallet off the load-on-startup list opens unloaded with no unload to
/// note its checkpoint. A load that published it, saw SPV store a batch
/// above that checkpoint and quit before its rewind is replayed from the
/// checkpoint at the next load.
#[test]
fn a_wallet_opened_unloaded_and_quit_mid_load_is_replayed_at_the_next_load() {
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    use platform_wallet::key_wallet_manager::WalletInterface;
    let (_dir, engine, session, id) = engine_wallet();
    let _stays_on_the_list = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine
        .block_on(session.set_load_on_startup(id, false))
        .unwrap();
    let session = reopen(&engine, session);
    assert!(session.live().unwrap().store.is_unloaded(&id.0));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    {
        let live = session.live().unwrap();
        let wm = live.manager.wallet_manager_arc();
        let store = Arc::clone(&live.store);
        *session.hub.on_published.lock().unwrap() = Some(Box::new(move |wallet| {
            {
                let mut wm = wm.try_write().expect("published, the manager unlocked");
                wm.update_wallet_synced_height(&wallet.0, HEIGHT + 50);
                wm.update_wallet_last_processed_height(&wallet.0, HEIGHT + 50);
            }
            let core = CoreChangeSet {
                last_processed_height: Some(HEIGHT + 50),
                synced_height: Some(HEIGHT + 50),
                ..Default::default()
            };
            let changeset = PlatformWalletChangeSet {
                core: Some(core),
                ..Default::default()
            };
            store.store(wallet.0, changeset).unwrap();
            store.flush(wallet.0).unwrap();
            true
        }));
    }
    assert!(engine.block_on(session.load_wallet(id)).is_err());

    let session = reopen(&engine, session);
    engine.block_on(session.load_wallet(id)).unwrap();

    assert_eq!(synced_height(&engine, &session, id), HEIGHT);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((HEIGHT, HEIGHT + 50))
    );
}

/// A rescan from block 500 that cannot widen a wallet's pools leaves it a
/// marker at checkpoint 499, the one the rescan rewinds to, so the next
/// load widens and replays block 500 too.
#[test]
fn a_rescan_that_cannot_widen_a_wallet_marks_it_for_the_next_load() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let w = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    store_checkpoint(&engine, &session, w, HEIGHT);
    start_offline_spv(&engine, &session);
    crate::keys::FAIL_WIDEN.lock().unwrap().push(w.0);

    let rescanned = engine.block_on(session.rescan(crate::RescanFrom::Height(500)));

    crate::keys::FAIL_WIDEN
        .lock()
        .unwrap()
        .retain(|x| *x != w.0);
    rescanned.unwrap();
    assert_eq!(
        wide_scan(&engine, &session, w),
        Some(crate::keys::WideScan::From(499))
    );
}

/// A marker at or below the wallet's birth checkpoint already names a
/// replay from below the birth: a rescan from the birth that cannot widen
/// the wallet keeps it instead of raising it to the birth.
#[test]
fn a_rescan_from_the_birth_that_cannot_widen_keeps_a_marker_below_the_birth() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let w = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(HOOK_PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                ..Default::default()
            },
        ))
        .unwrap();
    set_wide_scan(&engine, &session, w, Some(crate::keys::WideScan::From(5)));
    start_offline_spv(&engine, &session);
    crate::keys::FAIL_WIDEN.lock().unwrap().push(w.0);

    let rescanned = engine.block_on(session.rescan(crate::RescanFrom::WalletBirth));

    crate::keys::FAIL_WIDEN
        .lock()
        .unwrap()
        .retain(|x| *x != w.0);
    rescanned.unwrap();
    assert_eq!(
        wide_scan(&engine, &session, w),
        Some(crate::keys::WideScan::From(5))
    );
}

/// A watch-only import whose registration fails puts the wallet's marker
/// back as it was before the import wrote its own, as a seed import does.
#[test]
fn a_watch_only_import_whose_registration_fails_puts_the_marker_back() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[9; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, dashcore::Network::Regtest);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    (crate::keys::FAIL_REGISTRATION_IN.lock().unwrap()).push(session.data_dir().to_path_buf());

    let refused = engine
        .block_on(session.import_watch_only(xpub.to_string(), crate::WatchOnlyOptions::default()));

    assert!(refused.is_err());
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A watch-only import with a lookahead whose registration fails puts the
/// stored lookahead back as it was (here none): a retry with the default
/// scans with the default gap after a restart, not the failed request's.
#[test]
fn a_watch_only_import_whose_registration_fails_puts_the_lookahead_back() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[12; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, dashcore::Network::Regtest);
    (crate::keys::FAIL_REGISTRATION_IN.lock().unwrap()).push(session.data_dir().to_path_buf());

    let refused = engine.block_on(session.import_watch_only(
        xpub.to_string(),
        crate::WatchOnlyOptions {
            lookahead: Some(1),
            ..Default::default()
        },
    ));

    assert!(refused.is_err());
    assert_eq!(engine.block_on(session.stored_lookahead(id)).unwrap(), None);
    let imported = engine
        .block_on(session.import_watch_only(xpub.to_string(), crate::WatchOnlyOptions::default()));
    assert_eq!(imported.unwrap(), id);
    let session = reopen(&engine, session);
    assert_eq!(gap(&engine, &session, id, bip44()), 30);
}

/// A watch-only import whose registration fails puts back a lookahead that
/// was stored before it, as well as the absence of one.
#[test]
fn a_watch_only_import_whose_registration_fails_keeps_the_lookahead_stored_before() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[14; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, dashcore::Network::Regtest);
    store_setting(&engine, &session, id, crate::keys::LOOKAHEAD_SETTING, "200");
    (crate::keys::FAIL_REGISTRATION_IN.lock().unwrap()).push(session.data_dir().to_path_buf());

    let refused = engine.block_on(session.import_watch_only(
        xpub.to_string(),
        crate::WatchOnlyOptions {
            lookahead: Some(1),
            ..Default::default()
        },
    ));

    assert!(refused.is_err());
    assert_eq!(
        engine.block_on(session.stored_lookahead(id)).unwrap(),
        Some(200)
    );
}

/// An import of a seed the vault already holds, whose registration fails,
/// puts the wallet's marker back as it was before the import wrote its own.
#[test]
fn an_import_whose_registration_fails_puts_the_marker_back() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let secret = dw_vault::mnemonic::derive_secret(PHRASE, b"", false).unwrap();
    let id = WalletId(
        dw_vault::mnemonic::wallet_id_for_seed(&secret.seed, dashcore::Network::Regtest).unwrap(),
    );
    session.vault().store_wallet_secret(&id.0, &secret).unwrap();
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    (crate::keys::FAIL_REGISTRATION_IN.lock().unwrap()).push(session.data_dir().to_path_buf());

    let refused = engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(PHRASE.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions::default(),
    ));

    assert!(refused.is_err());
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// Removes wallet `id` from a test failure list once `f` has run.
fn failing<T>(list: &std::sync::Mutex<Vec<[u8; 32]>>, id: WalletId, f: impl FnOnce() -> T) -> T {
    list.lock().unwrap().push(id.0);
    let out = f();
    list.lock().unwrap().retain(|w| *w != id.0);
    out
}

fn created(engine: &Engine, session: &Arc<NetworkSession>) -> WalletId {
    engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id
}

/// A session open whose widening fails for a wallet already marked done
/// marks it pending from its checkpoint: SPV then scans it with narrow
/// pools, and the next open replays from there.
#[test]
fn a_session_open_that_cannot_widen_a_done_wallet_marks_it_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));

    let session = failing(&crate::keys::FAIL_WIDEN, id, || reopen(&engine, session));

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(HEIGHT))
    );
    let session = reopen(&engine, session);
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// Review r2 finding 1: a session open that can neither widen a wallet nor
/// store the replay it then owes does not open, so SPV never scans that
/// wallet with narrow pools unmarked. A healthy open after it opens.
#[test]
fn a_session_open_that_cannot_store_an_owed_replay_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );

    let opened = failing(&crate::keys::FAIL_WIDEN, id, || {
        failing(&crate::keys::FAIL_WIDE_SCAN_IO, id, || {
            engine.block_on(engine.open_network(DashNetwork::Regtest, offline()))
        })
    });

    assert!(opened.is_err());
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .unwrap();
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A load whose pending marker cannot be stored is refused before the
/// wallet is published, and the wallet stays closed.
#[test]
fn a_load_that_cannot_store_its_marker_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine.block_on(session.unload_wallet(id)).unwrap();
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));

    let loaded = failing(&crate::keys::FAIL_WIDE_SCAN_IO, id, || {
        engine.block_on(session.load_wallet(id))
    });

    assert!(loaded.is_err());
    let live = session.live().unwrap();
    assert!(live.store.is_unloaded(&id.0));
    assert!(engine.block_on(live.manager.get_wallet(&id.0)).is_none());
    engine.block_on(session.load_wallet(id)).unwrap();
}

/// An unload whose marker cannot be stored is refused, and the wallet stays
/// loaded.
#[test]
fn an_unload_that_cannot_store_its_marker_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);

    let unloaded = failing(&crate::keys::FAIL_WIDE_SCAN_IO, id, || {
        engine.block_on(session.unload_wallet(id))
    });

    assert!(unloaded.is_err());
    let live = session.live().unwrap();
    assert!(!live.store.is_unloaded(&id.0));
    assert!(engine.block_on(live.manager.get_wallet(&id.0)).is_some());
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A session open that cannot read a pending wallet's stored lookahead or
/// range keeps it pending: the replay it starts ran without that range.
#[test]
fn a_session_open_that_cannot_read_the_restore_state_keeps_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    engine.block_on(session.store_lookahead(id, 200));
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Birth));

    let session = failing(&crate::keys::FAIL_RESTORE_READ, id, || {
        reopen(&engine, session)
    });

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Birth)
    );
    let session = reopen(&engine, session);
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    assert_eq!(gap(&engine, &session, id, bip44()), 200);
}

/// The lookahead `bip44.lookahead` of the wallet platform-wallet registers,
/// as stored at that moment.
fn lookahead_at_registration(session: &NetworkSession) -> Arc<std::sync::Mutex<Option<String>>> {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let live = session.live().unwrap();
    let appdb = Arc::clone(&live.appdb);
    let out = Arc::clone(&seen);
    *live.store.on_register.lock().unwrap() = Some(Box::new(move |wallet| {
        *out.lock().unwrap() = appdb
            .setting(
                &WalletId(wallet).to_string(),
                crate::keys::LOOKAHEAD_SETTING,
            )
            .unwrap();
    }));
    seen
}

/// A seed import stores its lookahead before the wallet is registered: a
/// crash after SPV saw it still replays with that range.
#[test]
fn an_import_stores_its_lookahead_before_the_wallet_is_registered() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let seen = lookahead_at_registration(&session);

    engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(0),
                lookahead: Some(200),
                ..Default::default()
            },
        ))
        .unwrap();

    assert_eq!(seen.lock().unwrap().as_deref(), Some("200"));
}

/// A watch-only import stores its lookahead before the wallet is
/// registered.
#[test]
fn a_watch_only_import_stores_its_lookahead_before_the_wallet_is_registered() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let seen = lookahead_at_registration(&session);
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[11; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());

    engine
        .block_on(session.import_watch_only(
            xpub.to_string(),
            crate::WatchOnlyOptions {
                lookahead: Some(200),
                ..Default::default()
            },
        ))
        .unwrap();

    assert_eq!(seen.lock().unwrap().as_deref(), Some("200"));
}

/// Keys attached to a watch-only wallet with a lookahead and no Core range
/// raise and store that lookahead, and replay from the wallet's birth with
/// it before the wallet is marked done.
#[test]
fn keys_attached_with_a_lookahead_raise_it_and_replay_from_the_birth() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |lookahead| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                lookahead,
                ..Default::default()
            },
        ))
    };
    let id = import(None).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    commit_batch(&engine, &session, id, HEIGHT + 100);

    assert_eq!(import(Some(200)).unwrap(), id);

    assert_eq!(gap(&engine, &session, id, bip44()), 200);
    assert_eq!(
        engine.block_on(session.stored_lookahead(id)).unwrap(),
        Some(200)
    );
    let synced = synced_height(&engine, &session, id);
    assert!(
        synced < HEIGHT,
        "rewound to the birth checkpoint, at {synced}"
    );
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((synced, HEIGHT + 100))
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// Keys attached with a lookahead smaller than the one the wallet has keep
/// the larger: an address the wallet watched at index 500 is still derived
/// after a restart.
#[test]
fn keys_attached_with_a_smaller_lookahead_keep_the_wallets_larger_one() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |lookahead| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                lookahead,
                ..Default::default()
            },
        ))
    };
    let id = import(Some(1_000)).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());

    assert_eq!(import(Some(200)).unwrap(), id);

    assert_eq!(
        engine.block_on(session.stored_lookahead(id)).unwrap(),
        Some(1_000)
    );
    let session = reopen(&engine, session);
    assert_eq!(gap(&engine, &session, id, bip44()), 1_000);
    let highest = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let funds = info.accounts.funds_account(&bip44()).unwrap();
        funds.managed_account_type().address_pools()[0].highest_generated
    });
    assert!(highest.is_some_and(|h| h >= 500), "{highest:?}");
}

/// A restored backup's lookahead is stored and raised before the wallet is
/// registered, not only once its settings rows land.
#[test]
fn a_restored_backup_raises_its_lookahead_before_the_wallet_is_registered() {
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("w.dwbackup");
    let pass = || Some(zeroize::Zeroizing::new(b"bk".to_vec()));
    let id = {
        let (engine, session) = open_engine(dir.path().join("a"));
        create_vault(&engine, &session);
        let id = created(&engine, &session);
        engine.block_on(session.store_lookahead(id, 200));
        engine
            .block_on(session.backup_wallet(id, backup.clone(), pass()))
            .unwrap();
        id
    };
    let (engine, session) = open_engine(dir.path().join("b"));
    create_vault(&engine, &session);
    let seen = lookahead_at_registration(&session);

    assert_eq!(
        engine
            .block_on(session.restore_backup(backup, pass()))
            .unwrap(),
        vec![id]
    );

    assert_eq!(seen.lock().unwrap().as_deref(), Some("200"));
    assert_eq!(gap(&engine, &session, id, bip44()), 200);
}

/// CoinJoin Recovery stores the replay it owes from the birth and its
/// larger lookahead in one write: when that write fails, neither is stored,
/// so no replay is owed without the range it needs. Before, the replay was
/// stored first, and a failure or a quit before the lookahead left it owed
/// with the narrow BIP44 range.
#[test]
fn a_recovery_scan_whose_write_fails_stores_neither_its_replay_nor_its_lookahead() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    start_offline_spv(&engine, &session);
    let marker = wide_scan(&engine, &session, id);

    let recovered = failing(&crate::keys::FAIL_LOOKAHEAD_WRITE, id, || {
        engine.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(20),
                session.coinjoin_recovery_scan(id),
            )
            .await
        })
    });

    assert!(
        matches!(&recovered, Ok(Err(e)) if e.to_string().contains("test: lookahead not stored")),
        "refused at the write: {recovered:?}"
    );
    assert_eq!(engine.block_on(session.stored_lookahead(id)).unwrap(), None);
    assert_eq!(wide_scan(&engine, &session, id), marker);
    assert_eq!(replay_record(&engine, &session, id), None);
}

/// CoinJoin Recovery that stops right after its write, as a quit there
/// would: the next open raises the BIP44 range to the stored lookahead and
/// replays from the wallet's birth, which finds a payment to BIP44 index
/// 150 below the old checkpoint.
#[test]
fn a_recovery_scan_that_quits_after_its_write_replays_with_its_lookahead() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    start_offline_spv(&engine, &session);
    let birth = birth_height(&engine, &session, id).saturating_sub(1);
    let to = bip44_address(&engine, &session, id, 150);
    let tx = pays(1, &to);
    let data = session.data_dir().to_path_buf();
    (crate::coinjoin::QUIT_AFTER_RECOVERY_WRITE_IN
        .lock()
        .unwrap())
    .push(data);

    let recovered = engine.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            session.coinjoin_recovery_scan(id),
        )
        .await
    });
    assert!(
        matches!(&recovered, Ok(Err(e)) if e.to_string().contains("test: quit after the write")),
        "{recovered:?}"
    );
    assert!(gap(&engine, &session, id, bip44()) < crate::coinjoin::RECOVERY_LOOKAHEAD);

    let session = reopen(&engine, session);

    assert_eq!(
        engine.block_on(session.stored_lookahead(id)).unwrap(),
        Some(crate::coinjoin::RECOVERY_LOOKAHEAD)
    );
    assert_eq!(
        gap(&engine, &session, id, bip44()),
        crate::coinjoin::RECOVERY_LOOKAHEAD
    );
    assert_eq!(synced_height(&engine, &session, id), birth);
    assert_eq!(replay_record(&engine, &session, id), Some((birth, HEIGHT)));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    replay(&engine, &session, id, std::slice::from_ref(&tx));
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(tx.txid(), 0), to)],
        bip44(),
        VALUE,
    );
}

/// Review r7 finding 2: a wallet scanned to 1,000 holds a cancelled replay
/// at 800, and CoinJoin Recovery stored a wide replay owed from block 0 with
/// its lookahead before the app quit, ahead of its rescan. The next open
/// leaves the cancelled replay as it was and keeps the replay from 0 owed:
/// done is not written over it.
#[test]
fn a_cancelled_replay_above_an_owed_start_keeps_that_start_owed() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine.block_on(session.store_lookahead(id, 1_000));
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::From(0)));
    store_setting(&engine, &session, id, "replay", "2:1000:800:1:0");

    let session = reopen(&engine, session);

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(0))
    );
    let live = session.live().unwrap();
    let record = engine.block_on(live.replay.get(id)).unwrap().unwrap();
    assert!(record.cancelled && record.reached == 800, "{record:?}");
}

/// A cancelled replay below the birth holds no lower than the birth
/// checkpoint once resumed, so a start owed above its own start but below
/// that hold stays owed: a rescan from the birth would clear the hold
/// without scanning it.
#[test]
fn a_cancelled_replay_below_the_birth_keeps_a_start_owed_below_its_hold() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    set_wide_scan(
        &engine,
        &session,
        id,
        Some(crate::keys::WideScan::From(199)),
    );
    store_setting(&engine, &session, id, "replay", "2:1000:99:1:0");

    let session = reopen(&engine, session);

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(199))
    );
}

/// A rescan that cannot widen a wallet without a marker (an older build's,
/// or one whose done mark was not stored) stores what it owes, from the
/// rescan's start and no later than the birth, before it scans narrow.
#[test]
fn a_rescan_that_cannot_widen_a_wallet_without_a_marker_marks_it_owed() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    set_wide_scan(&engine, &session, id, None);
    start_offline_spv(&engine, &session);

    let rescanned = failing(&crate::keys::FAIL_WIDEN, id, || {
        engine.block_on(session.rescan(crate::RescanFrom::WalletBirth))
    });

    rescanned.unwrap();
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(699))
    );
}

/// A recovery scan of a wallet that is not open is refused before anything
/// is stored for it: its marker stays as its unload left it, not owed from
/// block 0, which would lower its birth to block 1 at the next load.
#[test]
fn a_recovery_scan_of_an_unloaded_wallet_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    let before = wide_scan(&engine, &session, id);
    assert_ne!(before, Some(crate::keys::WideScan::From(0)));
    start_offline_spv(&engine, &session);

    let recovered = engine.block_on(session.coinjoin_recovery_scan(id));

    assert!(
        matches!(recovered, Err(EngineError::WalletNotFound(_))),
        "{recovered:?}"
    );
    assert_eq!(wide_scan(&engine, &session, id), before);
}

/// A recovery scan of a wallet id never registered here is refused with
/// nothing stored for it.
#[test]
fn a_recovery_scan_of_an_unknown_wallet_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    start_offline_spv(&engine, &session);
    let id = WalletId([7; 32]);

    let recovered = engine.block_on(session.coinjoin_recovery_scan(id));

    assert!(
        matches!(recovered, Err(EngineError::WalletNotFound(_))),
        "{recovered:?}"
    );
    assert_eq!(wide_scan(&engine, &session, id), None);
}

/// A wallet born at block 700 and scanned to 1,000.
fn born_at_700(engine: &Engine, session: &Arc<NetworkSession>) -> WalletId {
    let id = created(engine, session);
    engine.block_on(session.set_birth_height(id, 700)).unwrap();
    store_checkpoint(engine, session, id, HEIGHT);
    assert_eq!(birth_height(engine, session, id), 700);
    id
}

/// A replay stored from below the birth, by a lower birth height or a
/// rescan below it that quit before the birth was stored, lowers the birth
/// to its start when it is resumed: dash-spv scans no block below the
/// earliest birth and never certifies a wallet whose checkpoint is below
/// it, so the replay would never finish and spends would wait for good.
#[test]
fn a_replay_resumed_from_below_the_birth_lowers_the_birth_to_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    store_setting(&engine, &session, id, "replay", "2:1000:99:0:0");

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 99);
    assert_eq!(birth_height(&engine, &session, id), 100);
    assert_eq!(stored_birth(&session, id), Some(100));
    assert_eq!(replay_record(&engine, &session, id), Some((99, HEIGHT)));
}

/// The birth height wallet.sqlite holds for wallet `id`.
fn stored_birth(session: &NetworkSession, id: WalletId) -> Option<u32> {
    let db_path = session.data_dir().join(crate::session::WALLET_DB_FILE);
    crate::store::stored_heights(&db_path, &id.0).unwrap().1
}

/// A replay from below the birth whose lower birth cannot be stored is not
/// resumed: the checkpoint stays, the record is kept, spends are held, and
/// a rescan from the birth lifts the hold.
#[test]
fn a_replay_below_the_birth_whose_lower_birth_cannot_be_stored_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    store_setting(&engine, &session, id, "replay", "2:1000:99:0:0");

    let session = failing(&crate::tools::FAIL_BIRTH_STORE, id, || {
        drop(session);
        assert!(
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        let options = crate::SessionOptions {
            no_platform: true,
            ..offline()
        };
        (engine.block_on(engine.open_network(DashNetwork::Regtest, options))).unwrap()
    });

    assert_eq!(birth_height(&engine, &session, id), 700);
    assert_eq!(stored_birth(&session, id), Some(700));
    assert_eq!(synced_height(&engine, &session, id), HEIGHT);
    assert_eq!(replay_record(&engine, &session, id), Some((99, HEIGHT)));
    let guard = engine.block_on(session.replay_guard(id, 0));
    assert!(
        matches!(guard, Err(EngineError::RescanInterrupted)),
        "{guard:?}"
    );
    start_offline_spv(&engine, &session);
    (engine.block_on(session.rescan(crate::RescanFrom::WalletBirth))).unwrap();
    let guard = engine.block_on(session.replay_guard(id, 0));
    assert!(
        matches!(guard, Err(EngineError::RescanInProgress)),
        "{guard:?}"
    );
}

/// A session open whose widening fails under a replay from below the birth
/// owes the wide scan from the replay's start, where the resume rescans
/// from, not from the old birth: the next open replays it from there.
#[test]
fn a_failed_widening_under_a_replay_below_the_birth_owes_from_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    store_setting(&engine, &session, id, "replay", "2:1000:99:0:0");

    let session = failing(&crate::keys::FAIL_WIDEN, id, || reopen(&engine, session));

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(99))
    );
    let session = reopen(&engine, session);
    assert_eq!(synced_height(&engine, &session, id), 99);
    assert_eq!(replay_record(&engine, &session, id), Some((99, HEIGHT)));
}

fn fail_replay_writes(session: &NetworkSession, fail: bool) {
    (session.live().unwrap().replay.fail_writes).store(fail, std::sync::atomic::Ordering::SeqCst);
}

/// Review r7 finding 4: a birth height lowered below the checkpoint stores
/// its rescan before the birth. When the rescan cannot be stored, the birth
/// stays, so no open trusts the old checkpoint under the lower birth.
#[test]
fn a_lower_birth_height_whose_rescan_cannot_be_stored_is_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);

    fail_replay_writes(&session, true);
    let set = engine.block_on(session.set_birth_height(id, 100));
    fail_replay_writes(&session, false);

    assert!(set.is_err());
    assert_eq!(birth_height(&engine, &session, id), 700);
    let session = reopen(&engine, session);
    assert_eq!(birth_height(&engine, &session, id), 700);
}

/// Review r7 finding 4: a lower birth height asked for while a rescan runs
/// is refused before anything is stored.
#[test]
fn a_lower_birth_height_during_a_rescan_is_refused_before_it_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    start_offline_spv(&engine, &session);
    engine
        .block_on(session.rescan(crate::RescanFrom::Height(900)))
        .unwrap();

    let set = engine.block_on(session.set_birth_height(id, 100));

    assert!(matches!(set, Err(EngineError::RescanInProgress)), "{set:?}");
    assert_eq!(birth_height(&engine, &session, id), 700);
}

/// Review r7 finding 4: a rescan from below the birth stores its replay
/// before the lower birth; when the replay cannot be stored, the birth
/// stays.
#[test]
fn a_rescan_below_the_birth_whose_replay_cannot_be_stored_keeps_the_birth() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = born_at_700(&engine, &session);
    start_offline_spv(&engine, &session);

    fail_replay_writes(&session, true);
    let rescanned = engine.block_on(session.rescan(crate::RescanFrom::Height(100)));
    fail_replay_writes(&session, false);

    assert!(rescanned.is_err());
    assert_eq!(birth_height(&engine, &session, id), 700);
}

/// Review r7 finding 5a: a session open that cannot read a wallet's marker
/// treats it as owed: the wallet is replayed from its birth, though its
/// widening succeeds, and the marker it could not read is not overwritten
/// with done. Here that marker owed a replay from block 100.
#[test]
fn a_marker_that_cannot_be_read_at_open_is_owed_from_the_birth() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_phrase(&engine, &session, MARKER_READ_PHRASE, 0).unwrap();
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(
        &engine,
        &session,
        id,
        Some(crate::keys::WideScan::From(100)),
    );

    let session = failing(&crate::keys::FAIL_WIDE_SCAN_READ, id, || {
        reopen(&engine, session)
    });

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(replay_record(&engine, &session, id), Some((0, HEIGHT)));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(100))
    );
}

/// Review r7 finding 5b: a backup whose lookahead is present but malformed
/// is refused as corrupt before its wallet is registered, as one with a
/// malformed legacy range is: it is not restored as having none.
#[test]
fn a_backup_with_a_malformed_lookahead_is_refused_before_registration() {
    let dir = tempfile::tempdir().unwrap();
    let backup = dir.path().join("w.dwbackup");
    let pass = || Some(zeroize::Zeroizing::new(b"bk".to_vec()));
    let id = {
        let (engine, session) = open_engine(dir.path().join("a"));
        create_vault(&engine, &session);
        let id = created(&engine, &session);
        store_setting(
            &engine,
            &session,
            id,
            crate::keys::LOOKAHEAD_SETTING,
            "two hundred",
        );
        engine
            .block_on(session.backup_wallet(id, backup.clone(), pass()))
            .unwrap();
        id
    };
    let (engine, session) = open_engine(dir.path().join("b"));
    create_vault(&engine, &session);

    let restored = engine.block_on(session.restore_backup(backup, pass()));

    assert!(restored.is_err(), "{restored:?}");
    assert!(
        engine
            .block_on(session.manager().unwrap().get_wallet(&id.0))
            .is_none()
    );
    assert!(!session.vault().has_wallet_secret(&id.0));
}

/// A rescan that can neither widen a wallet's pools nor store its marker is
/// refused before anything is rewound narrow.
#[test]
fn a_rescan_that_cannot_mark_a_wallet_it_cannot_widen_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let w = created(&engine, &session);
    store_checkpoint(&engine, &session, w, HEIGHT);
    start_offline_spv(&engine, &session);

    let rescanned = failing(&crate::keys::FAIL_WIDEN, w, || {
        failing(&crate::keys::FAIL_WIDE_SCAN_IO, w, || {
            engine.block_on(session.rescan(crate::RescanFrom::Height(500)))
        })
    });

    assert!(rescanned.is_err());
    assert_eq!(synced_height(&engine, &session, w), HEIGHT);
}

/// A wallet an older build restored and scanned, whose only CoinJoin use
/// is a payment to index 100 in a block that matched nothing else of it:
/// nothing on disk shows that use, and its first open still replays it once
/// from its birth, which finds the coin.
#[test]
fn an_older_build_wallet_whose_only_coinjoin_payment_left_no_trace_is_replayed() {
    let (_dir, engine, session, id) = engine_wallet();
    let to = coinjoin_addresses(&engine, &session, id, &[100]).remove(0);
    let tx = pays(1, &to);
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    set_wide_scan(&engine, &session, id, None);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 300))
    );
    replay(&engine, &session, id, std::slice::from_ref(&tx));
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(tx.txid(), 0), to)],
        cj(),
        VALUE,
    );
}

/// A watch-only wallet an older build scanned, with no CoinJoin account
/// and no stored lookahead, has no pool a narrow scan could have missed:
/// it is marked done without a replay.
#[test]
fn an_older_build_watch_only_wallet_without_coinjoin_or_lookahead_is_not_replayed() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let (_dir, engine, session, _) = engine_wallet();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[9; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = engine
        .block_on(session.import_watch_only(xpub.to_string(), Default::default()))
        .unwrap();
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    set_wide_scan(&engine, &session, id, None);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), HEIGHT + 300);
    assert_eq!(replay_record(&engine, &session, id), None);
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// A wallet an older build scanned, left off the start-up list, is loaded;
/// SPV commits a batch with its narrow pools and the app quits before they
/// are widened. The next open replays it from its birth, below that batch.
#[test]
fn an_older_build_wallet_loaded_then_quit_before_widening_is_replayed_from_its_birth() {
    use platform_wallet::key_wallet_manager::WalletInterface;
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine.block_on(session.unload_wallet(id)).unwrap();
    set_wide_scan(&engine, &session, id, None);
    {
        let live = session.live().unwrap();
        let wm = live.manager.wallet_manager_arc();
        *session.hub.on_published.lock().unwrap() = Some(Box::new(move |wallet| {
            let mut wm = wm.try_write().expect("published, the manager unlocked");
            wm.update_wallet_synced_height(&wallet.0, HEIGHT + 50);
            wm.update_wallet_last_processed_height(&wallet.0, HEIGHT + 50);
            true
        }));
    }

    let quit = engine.block_on(session.load_wallet(id)).unwrap_err();
    assert!(
        quit.to_string().contains("quit after the publish"),
        "{quit}"
    );
    store_synced_only(&session, id, HEIGHT + 50);
    let session = reopen(&engine, session);
    if session.live().unwrap().store.is_unloaded(&id.0) {
        engine.block_on(session.load_wallet(id)).unwrap();
    }

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(replay_record(&engine, &session, id), Some((0, HEIGHT + 50)));
}

/// #172: 0.3.0 could store `done` for a wallet an older build scanned
/// without replaying it. That marker reads as none: the open replays the
/// wallet once from its birth and stores this build's `done`.
#[test]
fn a_done_marker_an_older_build_stored_is_decided_again() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting(&scope, crate::keys::WIDE_SCAN_SETTING, Some("done"))
        }))
        .unwrap();
    assert_eq!(wide_scan(&engine, &session, id), None);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 300))
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

fn store_marker(engine: &Engine, session: &NetworkSession, id: WalletId, value: &'static str) {
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting(&scope, crate::keys::WIDE_SCAN_SETTING, Some(value))
        }))
        .unwrap();
}

/// #172: an unload turned 0.3.0's `done` into a bare height above the
/// birth. That height is owed from no higher than the birth.
#[test]
fn a_height_marker_an_older_build_stored_is_owed_from_the_birth() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    store_marker(&engine, &session, id, "1200");
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Legacy(1200))
    );

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 300))
    );
}

/// #172: a load notes the wallet's narrow start over an older build's bare
/// height without trusting it: under a cancelled replay holding at 800,
/// the wallet stays owed from its birth, not marked done.
#[test]
fn a_load_keeps_an_older_build_height_marker_owed_from_the_birth() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    store_setting(&engine, &session, id, "replay", "2:1300:800:1:0");
    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    store_marker(&engine, &session, id, "1200");

    engine.block_on(session.load_wallet(id)).unwrap();

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Legacy(800))
    );
}

/// #172: an older-build wallet owed a replay from its birth, under a
/// cancelled replay that holds at 800, stays owed from the birth when its
/// pools cannot widen, not from the checkpoint.
#[test]
fn a_failed_widening_under_a_cancelled_hold_keeps_the_birth_owed() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, None);
    store_setting(&engine, &session, id, "replay", "2:1000:800:1:0");
    let birth = birth_height(&engine, &session, id).saturating_sub(1);
    assert!(birth < 800, "{birth}");

    let session = failing(&crate::keys::FAIL_WIDEN, id, || reopen(&engine, session));

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(birth))
    );
}

/// Wallets the hook tests below import: no other test imports them.
const ATTACH_PHRASE: &[u8] = b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
const IMPORT_READ_PHRASE: &[u8] = b"all all all all all all all all all all all all";

fn import_phrase(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    phrase: &[u8],
    birth: u32,
) -> Result<WalletId, EngineError> {
    engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(phrase.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(birth),
            ..Default::default()
        },
    ))
}

/// The BIP44 account's receive address at `index`, derived on a copy of its
/// pool.
fn bip44_address(engine: &Engine, session: &NetworkSession, id: WalletId, index: u32) -> Address {
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let wallet = wm.get_wallet(&id.0).unwrap();
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let mut pool = (info.accounts.funds_account(&bip44()).unwrap())
            .managed_account_type()
            .address_pools()[0]
            .clone();
        let have = pool.highest_generated.unwrap_or(0);
        if index > have {
            pool.generate_addresses(index - have, &wallet.key_source_for_account(bip44()), true)
                .unwrap();
        }
        pool.address_at_index(index).unwrap()
    })
}

/// Review r2 finding 1: a replay from 0 to 1,000 is recorded and the wallet
/// marked done, and the app quits before the replay reaches block 500, which
/// holds a payment past the narrow pool. The next open cannot widen: it
/// stores the replay's start, not the loaded checkpoint, before SPV may
/// finish that replay with narrow pools. The open after that, healthy,
/// replays from there and finds the payment.
#[test]
fn a_start_that_cannot_widen_keeps_an_unfinished_replays_start() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    let to = coinjoin_addresses(&engine, &session, id, &[100]).remove(0);
    let tx = pays(1, &to);
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Birth));
    let session = reopen(&engine, session);
    assert_eq!(replay_record(&engine, &session, id), Some((0, HEIGHT)));
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    // The app quits before SPV commits a batch of the replay: the store
    // still reads 1,000, and the record still starts at 0.
    store_synced_only(&session, id, HEIGHT);
    assert_eq!(replay_record(&engine, &session, id), Some((0, HEIGHT)));

    let session = failing(&crate::keys::FAIL_WIDEN, id, || reopen(&engine, session));

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(0))
    );
    // SPV finishes the replay with the narrow pools.
    store_checkpoint(&engine, &session, id, HEIGHT + 1);
    assert_eq!(replay_record(&engine, &session, id), None);
    let session = reopen(&engine, session);
    assert_eq!(synced_height(&engine, &session, id), 0);
    replay_at(&engine, &session, id, std::slice::from_ref(&tx), 500);
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(tx.txid(), 0), to)],
        cj(),
        VALUE,
    );
}

/// Review r2 finding 2: keys attached to a watch-only wallet marked done,
/// with a lookahead of 200, whose replay obligation cannot be stored, store
/// no lookahead either: the done marker never stands beside the wider
/// range. Attached again, with a start above the wallet's birth, the replay
/// starts at the birth, the earlier, and finds a payment to receive index
/// 150 in block 500.
#[test]
fn an_attach_stores_its_replay_before_its_lookahead() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_phrase(&engine, &session, ATTACH_PHRASE, 100).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    let to = bip44_address(&engine, &session, id, 150);
    let tx = pays(4, &to);
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    let attach = || {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(ATTACH_PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(800),
                lookahead: Some(200),
                ..Default::default()
            },
        ))
    };

    let refused = failing(&crate::keys::FAIL_WIDE_SCAN_IO, id, attach);

    assert!(refused.is_err());
    assert_eq!(engine.block_on(session.stored_lookahead(id)).unwrap(), None);
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    assert_eq!(attach().unwrap(), id);
    assert!(synced_height(&engine, &session, id) < 500);
    assert_eq!(gap(&engine, &session, id, bip44()), 200);
    replay_at(&engine, &session, id, std::slice::from_ref(&tx), 500);
    assert_coins(
        &engine,
        &session,
        id,
        &[(OutPoint::new(tx.txid(), 0), to)],
        bip44(),
        VALUE,
    );
}

/// CoinJoin Recovery whose wide-scan marker cannot be read is refused
/// before it stores anything.
#[test]
fn a_recovery_scan_whose_marker_cannot_be_read_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    start_offline_spv(&engine, &session);

    let recovered = failing(&crate::keys::FAIL_WIDE_SCAN_IO, id, || {
        engine.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(20),
                session.coinjoin_recovery_scan(id),
            )
            .await
        })
    });

    assert!(
        matches!(&recovered, Ok(Err(e)) if e.to_string().contains("test: marker not read")),
        "refused at the read: {recovered:?}"
    );
    assert_eq!(engine.block_on(session.stored_lookahead(id)).unwrap(), None);
    assert_eq!(replay_record(&engine, &session, id), None);
}

/// Review r2 finding 4: a birth height that cannot be stored leaves the
/// wallet's birth in memory as it was.
#[test]
fn a_birth_height_that_cannot_be_stored_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    let before = birth_height(&engine, &session, id);

    let set = failing(&crate::tools::FAIL_BIRTH_STORE, id, || {
        engine.block_on(session.set_birth_height(id, before + 500))
    });

    assert!(set.is_err());
    assert_eq!(birth_height(&engine, &session, id), before);
    let session = reopen(&engine, session);
    assert_eq!(birth_height(&engine, &session, id), before);
}

/// Review r2 finding 5: a rescan from block 500 of a wallet whose stored
/// lookahead cannot be applied, marked from 1,000, lowers its marker to 499
/// though its CoinJoin pools widen: the rescan scans 500 to 1,000 with a
/// narrow BIP44 pool.
#[test]
fn a_rescan_with_a_range_it_cannot_apply_marks_the_wallet_from_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    engine.block_on(session.store_lookahead(id, 200));
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(
        &engine,
        &session,
        id,
        Some(crate::keys::WideScan::From(HEIGHT)),
    );
    start_offline_spv(&engine, &session);

    let rescanned = failing(&crate::keys::FAIL_RESTORE_READ, id, || {
        engine.block_on(session.rescan(crate::RescanFrom::Height(500)))
    });

    rescanned.unwrap();
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(499))
    );
}

/// Review r2 finding 5: a rescan refused because one runs changes no
/// marker.
#[test]
fn a_refused_second_rescan_changes_no_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    start_offline_spv(&engine, &session);
    engine
        .block_on(session.rescan(crate::RescanFrom::Height(900)))
        .unwrap();

    let second = failing(&crate::keys::FAIL_WIDEN, id, || {
        engine.block_on(session.rescan(crate::RescanFrom::Height(500)))
    });

    assert!(
        matches!(second, Err(EngineError::RescanInProgress)),
        "{second:?}"
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// Review r2 finding 6: a seed import that cannot read the marker already
/// stored for its wallet is refused, and registers nothing.
#[test]
fn an_import_that_cannot_read_the_previous_marker_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let secret = dw_vault::mnemonic::derive_secret(IMPORT_READ_PHRASE, b"", false).unwrap();
    let id = WalletId(
        dw_vault::mnemonic::wallet_id_for_seed(&secret.seed, dashcore::Network::Regtest).unwrap(),
    );
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::From(5)));

    let imported = failing(&crate::keys::FAIL_WIDE_SCAN_READ, id, || {
        import_phrase(&engine, &session, IMPORT_READ_PHRASE, 0)
    });

    assert!(imported.is_err());
    assert!(!session.vault().has_wallet_secret(&id.0));
    assert!(
        engine
            .block_on(session.manager().unwrap().get_wallet(&id.0))
            .is_none()
    );
}

/// Review r2 finding 6: as above, for a watch-only import.
#[test]
fn a_watch_only_import_that_cannot_read_the_previous_marker_is_refused() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[13; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, dashcore::Network::Regtest);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::From(5)));

    let imported = failing(&crate::keys::FAIL_WIDE_SCAN_READ, id, || {
        engine.block_on(
            session.import_watch_only(xpub.to_string(), crate::WatchOnlyOptions::default()),
        )
    });

    assert!(imported.is_err());
    assert!(
        engine
            .block_on(session.manager().unwrap().get_wallet(&id.0))
            .is_none()
    );
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(5))
    );
}

fn store_setting(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    key: &'static str,
    value: &'static str,
) {
    let scope = id.to_string();
    engine
        .block_on(session.appdb_op(move |db| db.set_setting(&scope, key, Some(value))))
        .unwrap();
}

/// Review r2 finding 6: a malformed stored lookahead is not "none stored":
/// the wallet's pending marker stays.
#[test]
fn a_malformed_stored_lookahead_keeps_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_setting(
        &engine,
        &session,
        id,
        crate::keys::LOOKAHEAD_SETTING,
        "two hundred",
    );
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Birth));

    let session = reopen(&engine, session);

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Birth)
    );
}

/// Review r2 finding 6: a malformed stored legacy range is not "none
/// stored" either.
#[test]
fn a_malformed_stored_range_keeps_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    store_setting(&engine, &session, id, crate::compat::ISSUED_SETTING, "12;x");
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Birth));

    let session = reopen(&engine, session);

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Birth)
    );
}

/// The upgrade rule: a wallet an older build scanned whose stored CoinJoin
/// pool has an address marked used is replayed once from its birth.
#[test]
fn an_older_build_wallet_with_a_used_coinjoin_pool_row_is_replayed() {
    let (_dir, engine, session, id) = engine_wallet();
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    set_wide_scan(&engine, &session, id, None);
    let db_path = session.data_dir().join(crate::session::WALLET_DB_FILE);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let db = rusqlite::Connection::open(&db_path).unwrap();
    let marked = db
        .execute(
            "UPDATE core_address_pool SET used = 1 WHERE wallet_id = ?1 \
             AND account_type = 'coinjoin' AND pool_type = 0 AND address_index = 3",
            [&id.0[..]],
        )
        .unwrap();
    assert_eq!(marked, 1, "the stored pool row");
    drop(db);

    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, offline()))
        .unwrap();

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 300))
    );
}

/// The upgrade rule: a wallet an older build scanned with a raised
/// lookahead is replayed once from its birth, though its CoinJoin account
/// shows no use.
#[test]
fn an_older_build_wallet_with_a_raised_lookahead_is_replayed() {
    let (_dir, engine, session, id) = engine_wallet();
    engine.block_on(session.store_lookahead(id, 200));
    store_checkpoint(&engine, &session, id, HEIGHT + 300);
    set_wide_scan(&engine, &session, id, None);

    let session = reopen(&engine, session);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((0, HEIGHT + 300))
    );
}

/// Review r2 finding 1, on a load: a wallet opened unloaded whose replay
/// from 0 is unfinished, and whose store still reads 1,000, is marked from
/// the replay's start before the load publishes it. A quit right after the
/// publish leaves that mark.
#[test]
fn a_load_notes_an_unfinished_replays_start_before_it_publishes() {
    let (_dir, engine, session, id) = engine_wallet();
    let _stays_on_the_list = created(&engine, &session);
    store_checkpoint(&engine, &session, id, HEIGHT);
    engine
        .block_on(session.set_load_on_startup(id, false))
        .unwrap();
    let session = reopen(&engine, session);
    assert!(session.live().unwrap().store.is_unloaded(&id.0));
    let live = session.live().unwrap();
    engine
        .block_on(live.replay.note_rewind(id, 0, HEIGHT, 0))
        .unwrap();
    *session.hub.on_published.lock().unwrap() = Some(Box::new(|_| true));

    assert!(engine.block_on(session.load_wallet(id)).is_err());

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(0))
    );
}

/// PHRASE's wallet id, as an import derives it.
fn phrase_id() -> WalletId {
    let secret = dw_vault::mnemonic::derive_secret(PHRASE, b"", false).unwrap();
    WalletId(
        dw_vault::mnemonic::wallet_id_for_seed(&secret.seed, dashcore::Network::Regtest).unwrap(),
    )
}

/// F-B: keys attached while the vault is unlocked for mixing only are
/// refused, and the marker and lookahead they replaced are back: a reopen
/// neither lowers the birth nor replays.
#[test]
fn an_attach_the_vault_refuses_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    engine
        .block_on(session.vault_op(|v| v.create(Some(b"pw"))))
        .unwrap();
    let id = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    store_checkpoint(&engine, &session, id, HEIGHT + 100);
    set_wide_scan(&engine, &session, id, Some(crate::keys::WideScan::Done));
    session.vault().lock();
    (session.vault())
        .unlock(b"pw", dw_vault::UnlockScope::MixingOnly)
        .unwrap();

    let refused = engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(PHRASE.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(0),
            lookahead: Some(200),
            ..Default::default()
        },
    ));

    assert!(refused.is_err());
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    assert_eq!(engine.block_on(session.stored_lookahead(id)).unwrap(), None);
    let session = reopen(&engine, session);
    assert_eq!(birth_height(&engine, &session, id), HEIGHT);
    assert_eq!(replay_record(&engine, &session, id), None);
}

/// F-A: a seed import with the default lookahead deletes a row an earlier
/// attempt left before registration: the next load scans with the default.
#[test]
fn an_import_with_the_default_lookahead_drops_a_row_left_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = phrase_id();
    store_setting(&engine, &session, id, crate::keys::LOOKAHEAD_SETTING, "1");

    assert_eq!(import_from_genesis(&engine, &session), id);

    let session = reopen(&engine, session);
    assert_eq!(gap(&engine, &session, id, bip44()), 30);
}

/// F-A: as a seed import, a watch-only import with the default lookahead
/// deletes a row an earlier attempt left.
#[test]
fn a_watch_only_import_with_the_default_lookahead_drops_a_row_left_before_it() {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[15; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    let xpub = ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap());
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, dashcore::Network::Regtest);
    store_setting(&engine, &session, id, crate::keys::LOOKAHEAD_SETTING, "1");

    let imported = engine
        .block_on(session.import_watch_only(xpub.to_string(), crate::WatchOnlyOptions::default()));

    assert_eq!(imported.unwrap(), id);
    let session = reopen(&engine, session);
    assert_eq!(gap(&engine, &session, id, bip44()), 30);
}

/// F-G: keys attached with a lookahead below the default, none stored,
/// keep the default.
#[test]
fn keys_attached_with_a_lookahead_below_the_default_keep_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());

    let attached = engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(PHRASE.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(HEIGHT),
            lookahead: Some(1),
            ..Default::default()
        },
    ));

    assert_eq!(attached.unwrap(), id);
    let session = reopen(&engine, session);
    assert_eq!(gap(&engine, &session, id, bip44()), 30);
}

/// F-F: keys attached to a wallet whose stored lookahead does not parse
/// read it as none and go on.
#[test]
fn keys_attached_over_a_malformed_stored_lookahead_go_on() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    store_setting(&engine, &session, id, crate::keys::LOOKAHEAD_SETTING, "x");

    let attached = engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(PHRASE.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(HEIGHT),
            lookahead: Some(200),
            ..Default::default()
        },
    ));

    assert_eq!(attached.unwrap(), id);
    assert_eq!(
        engine.block_on(session.stored_lookahead(id)).unwrap(),
        Some(200)
    );
}

/// F-H: a marker that does not decode owes a replay from genesis, not from
/// the birth.
#[test]
fn an_undecodable_marker_is_owed_from_genesis() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);

    store_marker(&engine, &session, id, "2:x");
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(0))
    );
    store_marker(&engine, &session, id, "x");
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Legacy(0))
    );
}

/// Round 4, 2: keys attached with a lookahead below the wallet's live
/// CoinJoin gap, refused by the vault, leave the live gap as it was.
#[test]
fn an_attach_the_vault_refuses_keeps_the_live_coinjoin_gap() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    engine
        .block_on(session.vault_op(|v| v.create(Some(b"pw"))))
        .unwrap();
    let id = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    let manager = session.manager().unwrap();
    engine
        .block_on(crate::keys::ensure_coinjoin_scan_gap(&manager, id))
        .unwrap();
    let wide = gap(&engine, &session, id, cj());
    assert!(wide > 200, "{wide}");
    session.vault().lock();
    (session.vault())
        .unlock(b"pw", dw_vault::UnlockScope::MixingOnly)
        .unwrap();

    let refused = engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(PHRASE.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(HEIGHT),
            lookahead: Some(200),
            ..Default::default()
        },
    ));

    assert!(refused.is_err());
    assert_eq!(gap(&engine, &session, id, cj()), wide);
}

/// Round 4, 3: a removal that fails to delete the wallet from its store
/// leaves its lookahead row; an import of it again with the default
/// lookahead keeps that row up to its registration, so an exit there
/// reopens the stored wallet with its own gap.
#[test]
fn a_default_reimport_keeps_the_lookahead_of_a_wallet_still_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |lookahead| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                lookahead,
                ..Default::default()
            },
        ))
    };
    let id = import(Some(200)).unwrap();
    let wipe = (session.vault())
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    (crate::wallets::FAIL_DELETE_IN.lock().unwrap()).push(session.data_dir().to_path_buf());
    assert!(engine.block_on(session.remove_wallet(id, wipe)).is_err());
    let seen = lookahead_at_registration(&session);

    let _ = import(None);

    assert_eq!(seen.lock().unwrap().as_deref(), Some("200"));
}

/// Writes a `.dwbackup` of `wallets` sealed by `session`'s vault (backup
/// passphrase `bk`), each bundle born at 0 and carrying the lookahead row
/// given.
fn craft_backup(
    session: &NetworkSession,
    path: &std::path::Path,
    wallets: &[(WalletId, &[(&str, &str)])],
) {
    let header = serde_json::json!({
        "format": "dwbackup", "format_version": 1, "network": "regtest", "created_at": 1,
        "wallet_ids": wallets.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>(),
        "automatic": false, "app_version": "test",
    });
    let header_line = serde_json::to_vec(&header).unwrap();
    let bundles: Vec<_> = (wallets.iter())
        .map(|(id, settings)| {
            let rows: Vec<_> = (settings.iter())
                .map(|(k, v)| serde_json::json!([{ "t": id.to_string() }, { "t": k }, { "t": v }]))
                .collect();
            let payload = serde_json::json!({
                "name": "A", "birth_height": 0, "created_at": null,
                "app_rows": [{ "table": "settings_kv", "columns": ["scope", "key", "value"], "rows": rows }],
            });
            (session.vault())
                .backup_bundle(&id.0, Some(b"bk"), &serde_json::to_vec(&payload).unwrap(), &header_line)
                .unwrap()
        })
        .collect();
    let mut out = b"DWBACKUP 1\n".to_vec();
    out.extend_from_slice(&header_line);
    out.push(b'\n');
    out.extend(serde_json::to_vec(&serde_json::json!({ "bundles": bundles })).unwrap());
    out.push(b'\n');
    std::fs::write(path, out).unwrap();
}

/// Round 4, 6: a restore whose first bundle, carrying a lookahead, attaches
/// keys to a watch-only wallet and whose second fails (a wallet with keys
/// already) leaves the first wallet's scan as it was: a reopen keeps its
/// birth and marker and owes no replay.
#[test]
fn a_failed_restore_leaves_the_scan_of_a_wallet_it_attached_keys_to() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let a = born_at_700(&engine, &session);
    let b = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    let backup = dir.path().join("two.dwbackup");
    craft_backup(
        &session,
        &backup,
        &[(a, &[(crate::keys::LOOKAHEAD_SETTING, "200")]), (b, &[])],
    );
    assert!(session.vault().delete_wallet_secret(&a.0).unwrap());
    let marker = wide_scan(&engine, &session, a);

    let restored = engine
        .block_on(session.restore_backup(backup, Some(zeroize::Zeroizing::new(b"bk".to_vec()))));

    assert!(restored.is_err(), "{restored:?}");
    let session = reopen(&engine, session);
    assert_eq!(birth_height(&engine, &session, a), 700);
    assert_eq!(replay_record(&engine, &session, a), None);
    assert_eq!(wide_scan(&engine, &session, a), marker);
}

/// Round 4, 7: keys attached with only a lookahead to a wallet whose stored
/// legacy range start-up could not apply apply that range before the
/// replay: its addresses are derived, and the wallet is marked done.
#[test]
fn keys_attached_with_a_lookahead_apply_a_stored_range_start_up_left() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    store_setting(
        &engine,
        &session,
        id,
        crate::compat::ISSUED_SETTING,
        "1001,2",
    );
    let data = session.data_dir().to_path_buf();
    (crate::keys::FAIL_RANGES_IN.lock().unwrap()).push(data);
    let session = reopen(&engine, session);

    let attached = engine.block_on(session.import_wallet(
        zeroize::Zeroizing::new(PHRASE.to_vec()),
        zeroize::Zeroizing::new(Vec::new()),
        crate::ImportOptions {
            birth_height: Some(HEIGHT),
            lookahead: Some(200),
            ..Default::default()
        },
    ));

    assert_eq!(attached.unwrap(), id);
    let highest = engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let funds = info.accounts.funds_account(&bip44()).unwrap();
        funds.managed_account_type().address_pools()[0].highest_generated
    });
    assert!(highest.is_some_and(|h| h >= 1001), "{highest:?}");
}

/// Round 5, B: a fresh import with a lookahead of 1 runs with 1, and so
/// does a reopen of it, as on main: only an attach keeps a wider pool.
#[test]
fn a_fresh_import_and_a_reopen_with_a_lookahead_of_one_run_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);

    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                lookahead: Some(1),
                ..Default::default()
            },
        ))
        .unwrap();

    assert_eq!(gap(&engine, &session, id, bip44()), 1, "fresh import");
    let session = reopen(&engine, session);
    assert_eq!(gap(&engine, &session, id, bip44()), 1, "reopen");
}

/// Round 5, 1: a backup restored onto a watch-only wallet whose stored
/// lookahead start-up could not apply, with the wallet's legacy counters
/// and no lookahead, applies that lookahead before it marks the wallet
/// scanned.
#[test]
fn a_range_only_attach_applies_the_stored_lookahead_start_up_left() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = import_phrase(&engine, &session, PHRASE, HEIGHT).unwrap();
    engine.block_on(session.store_lookahead(id, 1000));
    let backup = dir.path().join("one.dwbackup");
    craft_backup(
        &session,
        &backup,
        &[(id, &[(crate::compat::ISSUED_SETTING, "5,2")])],
    );
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    let data = session.data_dir().to_path_buf();
    (crate::keys::FAIL_RANGES_IN.lock().unwrap()).push(data);
    let session = reopen(&engine, session);
    assert!(gap(&engine, &session, id, bip44()) < 1000);

    engine
        .block_on(session.restore_backup(backup, Some(zeroize::Zeroizing::new(b"bk".to_vec()))))
        .unwrap();

    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
    assert_eq!(gap(&engine, &session, id, bip44()), 1000);
}

/// Round 5, 2: a rescan of a wallet whose live BIP44 gap is wider than
/// its stored lookahead (a raise whose row was not written) keeps the
/// live gap.
#[test]
fn a_rescan_keeps_a_live_gap_wider_than_the_stored_one() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_spv_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let id = created(&engine, &session);
    engine.block_on(session.store_lookahead(id, 50));
    let manager = session.manager().unwrap();
    (engine.block_on(session.raise_lookahead(&manager, id, 1000))).unwrap();
    store_checkpoint(&engine, &session, id, HEIGHT);
    start_offline_spv(&engine, &session);

    (engine.block_on(session.rescan(crate::RescanFrom::Height(500)))).unwrap();

    assert_eq!(gap(&engine, &session, id, bip44()), 1000);
}

/// Review N2: keys attached to a synced watch-only wallet show the replay
/// they start as a rescan before any reopen or block: spends wait for it,
/// so it reports progress and can be cancelled.
#[test]
fn keys_attached_to_a_synced_wallet_show_their_replay() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |lookahead| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                lookahead,
                ..Default::default()
            },
        ))
    };
    let id = import(None).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    commit_batch(&engine, &session, id, HEIGHT + 100);

    assert_eq!(import(Some(200)).unwrap(), id);

    let synced = synced_height(&engine, &session, id);
    let progress = session.rescan_progress().unwrap();
    assert_eq!(progress.map(|p| p.from_height), Some(synced + 1));
    // The replay's own target, with no network tip known.
    let progress = session.rescan_progress().unwrap();
    assert_eq!(progress.and_then(|p| p.target_height), Some(HEIGHT + 100));
    let guard = engine.block_on(session.replay_guard(id, 0));
    assert!(
        matches!(guard, Err(EngineError::RescanInProgress)),
        "{guard:?}"
    );
    assert!(engine.block_on(session.cancel_rescan()).unwrap());
    assert_eq!(synced_height(&engine, &session, id), HEIGHT + 100);
    let record = engine.block_on(session.live().unwrap().replay.get(id));
    assert!(record.unwrap().is_some_and(|r| r.cancelled));
    let guard = engine.block_on(session.replay_guard(id, 0));
    assert!(
        matches!(guard, Err(EngineError::RescanInterrupted)),
        "{guard:?}"
    );
}

/// Review #208, 2: keys attached to a synced wallet whose replay SPV
/// finishes before the attach publishes it show no rescan.
#[test]
fn keys_attached_show_no_replay_spv_already_finished() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |lookahead| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(HEIGHT),
                lookahead,
                ..Default::default()
            },
        ))
    };
    let id = import(None).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    commit_batch(&engine, &session, id, HEIGHT + 100);
    let data = session.data_dir().to_path_buf();
    (crate::keys::REACH_BEFORE_PUBLISH_IN.lock().unwrap()).push(data);

    assert_eq!(import(Some(200)).unwrap(), id);

    assert_eq!(synced_height(&engine, &session, id), HEIGHT + 100);
    assert_eq!(
        session.rescan_progress().unwrap().map(|p| p.from_height),
        None
    );
}

/// Keys attached alone (no Dash Core range, no lookahead) with a birth
/// height below a synced watch-only wallet's lower its birth and replay
/// from there.
#[test]
fn keys_attached_alone_with_an_earlier_birth_replay_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |birth| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(birth),
                ..Default::default()
            },
        ))
    };
    let id = import(HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    commit_batch(&engine, &session, id, HEIGHT + 100);

    assert_eq!(import(HEIGHT - 500).unwrap(), id);

    assert_eq!(birth_height(&engine, &session, id), HEIGHT - 500);
    assert_eq!(stored_birth(&session, id), Some(HEIGHT - 500));
    assert_eq!(synced_height(&engine, &session, id), HEIGHT - 501);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((HEIGHT - 501, HEIGHT + 100))
    );
}

/// Keys attached with an earlier birth to a watch-only wallet whose
/// checkpoint is already at or below the attach's start need no rewind, and
/// still lower the birth: dash-spv scans nothing below it.
#[test]
fn keys_attached_with_an_earlier_birth_below_no_checkpoint_still_lower_the_birth() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |birth, lookahead| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(birth),
                lookahead,
                ..Default::default()
            },
        ))
    };
    let id = import(HEIGHT, None).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    store_synced_only(&session, id, 0);
    engine.block_on(async {
        let manager = session.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        info.core_wallet.update_synced_height(0);
    });
    assert_eq!(synced_height(&engine, &session, id), 0);

    assert_eq!(import(HEIGHT - 500, Some(200)).unwrap(), id);

    assert_eq!(synced_height(&engine, &session, id), 0);
    assert_eq!(birth_height(&engine, &session, id), HEIGHT - 500);
    assert_eq!(stored_birth(&session, id), Some(HEIGHT - 500));
    assert_eq!(replay_record(&engine, &session, id), None, "no rewind");
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::Done)
    );
}

/// Keys attached alone with an earlier birth whose seed cannot be stored
/// are refused, and the birth and the marker the attach changed are put
/// back.
#[test]
fn keys_attached_alone_with_an_earlier_birth_that_are_refused_put_the_birth_back() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |birth| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(birth),
                ..Default::default()
            },
        ))
    };
    let id = import(HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    commit_batch(&engine, &session, id, HEIGHT + 100);
    let marker = wide_scan(&engine, &session, id);
    let appdb = session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::After;
    dw_appdb::fault::arm(&appdb, crate::keys::IMPORT_ROWS, stage);

    assert!(import(HEIGHT - 500).is_err());

    dw_appdb::fault::disarm(&appdb);
    assert_eq!(birth_height(&engine, &session, id), HEIGHT);
    assert_eq!(stored_birth(&session, id), Some(HEIGHT));
    assert_eq!(wide_scan(&engine, &session, id), marker);
    assert_eq!(synced_height(&engine, &session, id), HEIGHT + 100);
    assert_eq!(replay_record(&engine, &session, id), None);
}

/// Wallets the two tests below import, each its own: their birth-store
/// faults go by wallet id.
const LOWERING_PHRASE: &[u8] =
    b"afford alter spike radar gate glance object seek swamp infant panel yellow";
const PUT_BACK_PHRASE: &[u8] =
    b"turtle front uncle idea crush write shrug there lottery flower risk shell";

/// Keys alone with an earlier birth whose lowering cannot be stored are
/// refused with the replay from that birth still owed: the next load lowers
/// the birth and replays.
#[test]
fn keys_attached_alone_whose_lower_birth_cannot_be_stored_leave_the_replay_owed() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, session) = open_engine(dir.path().join("data"));
    create_vault(&engine, &session);
    let import = |birth| {
        engine.block_on(session.import_wallet(
            zeroize::Zeroizing::new(LOWERING_PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(birth),
                ..Default::default()
            },
        ))
    };
    let id = import(HEIGHT).unwrap();
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    store_checkpoint(&engine, &session, id, HEIGHT + 100);

    let refused = failing(&crate::tools::FAIL_BIRTH_STORE, id, || import(HEIGHT - 500));

    assert!(refused.is_err());
    assert_eq!(birth_height(&engine, &session, id), HEIGHT);
    assert_eq!(
        wide_scan(&engine, &session, id),
        Some(crate::keys::WideScan::From(HEIGHT - 501))
    );
    let session = reopen(&engine, session);
    assert_eq!(birth_height(&engine, &session, id), HEIGHT - 500);
    assert_eq!(synced_height(&engine, &session, id), HEIGHT - 501);
    assert_eq!(
        replay_record(&engine, &session, id),
        Some((HEIGHT - 501, HEIGHT + 100))
    );
}

/// A refused attach that cannot put its lower birth back keeps the replay
/// owed rather than marking the lower birth scanned: the next load
/// replays from it. That is also what a quit between the lowering and the
/// keys leaves, and attaching the keys again, alone or with a lookahead,
/// stores them and keeps that replay.
#[test]
fn keys_attached_alone_whose_birth_cannot_be_put_back_leave_the_replay_owed() {
    for retry_lookahead in [None, Some(200)] {
        let dir = tempfile::tempdir().unwrap();
        let (engine, session) = open_engine(dir.path().join("data"));
        create_vault(&engine, &session);
        let import = |session: &Arc<NetworkSession>, birth, lookahead| {
            engine.block_on(session.import_wallet(
                zeroize::Zeroizing::new(PUT_BACK_PHRASE.to_vec()),
                zeroize::Zeroizing::new(Vec::new()),
                crate::ImportOptions {
                    birth_height: Some(birth),
                    lookahead,
                    ..Default::default()
                },
            ))
        };
        let id = import(&session, HEIGHT, None).unwrap();
        assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
        store_checkpoint(&engine, &session, id, HEIGHT + 100);
        let appdb = session.data_dir().join(dw_appdb::APP_DB_FILE);
        dw_appdb::fault::arm(
            &appdb,
            crate::keys::IMPORT_ROWS,
            dw_appdb::fault::Stage::After,
        );

        let refused = failing(&crate::keys::FAIL_BIRTH_PUT_BACK, id, || {
            import(&session, HEIGHT - 500, None)
        });

        dw_appdb::fault::disarm(&appdb);
        assert!(refused.is_err());
        assert_eq!(birth_height(&engine, &session, id), HEIGHT - 500);
        assert_eq!(
            wide_scan(&engine, &session, id),
            Some(crate::keys::WideScan::From(HEIGHT - 501))
        );
        let session = reopen(&engine, session);
        assert_eq!(synced_height(&engine, &session, id), HEIGHT - 501);
        assert_eq!(
            replay_record(&engine, &session, id),
            Some((HEIGHT - 501, HEIGHT + 100))
        );

        let retried = import(&session, HEIGHT - 500, retry_lookahead);

        assert_eq!(retried.unwrap(), id, "retry with {retry_lookahead:?}");
        assert!(session.vault().has_wallet_secret(&id.0));
        assert_eq!(birth_height(&engine, &session, id), HEIGHT - 500);
        assert_eq!(stored_birth(&session, id), Some(HEIGHT - 500));
        assert_eq!(synced_height(&engine, &session, id), HEIGHT - 501);
        assert_eq!(
            replay_record(&engine, &session, id),
            Some((HEIGHT - 501, HEIGHT + 100)),
            "retry with {retry_lookahead:?}"
        );
        if let Some(lookahead) = retry_lookahead {
            assert_eq!(gap(&engine, &session, id, bip44()), lookahead);
        }
    }
}
