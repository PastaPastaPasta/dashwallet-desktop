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
/// returns, no lookahead is stored, and a replay counts the coin once.
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
    assert_eq!(engine.block_on(session.stored_lookahead(id)), None);
    assert_home(&engine, &session, id, &coin);
    replay(&engine, &session, id, &history);
    assert_home(&engine, &session, id, &coin);
}

/// Sol r1 M1 through the recovery scan's raise, its restore failing: the
/// coin past the spent bridge stays where the load left it (BIP44), counted
/// once; the hub reads the wallet before the error returns; no lookahead is
/// stored. A replay copies it into CoinJoin; the replay's block event starts
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
    assert_eq!(engine.block_on(session.stored_lookahead(id)), None);
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
    assert_eq!(engine.block_on(session.stored_lookahead(id)), None);
}

/// Signs, through the session, a PSBT spending `outpoint` of `prev`: whether
/// the wallet signed it (it has the coin's key and derivation path).
fn signs(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, prev: &Transaction) -> bool {
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
    match engine.block_on(session.sign_psbt(id, unsigned, grant)) {
        Ok(mut signed) => dw_psbt::finalize(&mut signed),
        Err(_) => false,
    }
}

/// Sol r1 M2 (the deleted `a_real_load_marks_a_sparse_bip44_coin_used`): a
/// BIP44 coin at external index 79, written by the real persister with no
/// pool row and no used address near it, no lookahead stored. After
/// `unload_wallet` and `load_wallet` its address is derived and used, the gap
/// is kept past it, it counts once, and the wallet signs a spend of it.
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
        signs(&engine, &session, id, &tx),
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
            std::time::Duration::from_secs(10),
            session.reconcile_duplicate_coins(id),
        )
        .await;

        drop(reader);
        assert!(ran.is_ok(), "it waited for the write lock");
        assert_eq!(session.hub.wallet_state(&id).unwrap().balance.total(), 0);
    });
}
