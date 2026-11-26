//! A payment the user abandoned that went through after all, whose change a
//! locked payment had already spent (#302). Once the payment comes back the
//! locked spend is passed to the wallet again, as dash-spv's mempool manager
//! does, so the wallet records it: the change stays spent across a restart,
//! and comes back, at once and after a restart, when a ChainLocked conflict
//! beats the spend. A restart before that conflict's ChainLock is pinned as
//! it is: the spend stays until the wallet is restored from its phrase.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dashcore::bls_sig_utils::BLSSignature;
use dashcore::{Address, ChainLock, InstantLock, OutPoint, Transaction, TxIn, Txid};
use platform_wallet::key_wallet_manager::WalletInterface;

use super::own_spend_disk_tests::{
    COIN, COINS, FUNDED_AT, TIP, abandon, change_of, coins, foreign, fresh_address, funding,
    own_scripts, send, spendable, start_offline_spv, wait_stored,
};
use super::{block_with, deliver, elsewhere, engine_wallet, mempool, mined, reopen};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{Engine, WalletId};

fn lock_for(tx: &Transaction) -> InstantLock {
    InstantLock {
        txid: tx.txid(),
        inputs: tx.input.iter().map(|input| input.previous_output).collect(),
        ..InstantLock::default()
    }
}

fn held(engine: &Engine, session: &NetworkSession, id: WalletId) -> BTreeSet<OutPoint> {
    coins(engine, session, id)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

struct Abandoned {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    funding: Transaction,
    payment: Transaction,
    child: Transaction,
}

/// A wallet holding [`COINS`] confirmed coins from `funding`; a payment P
/// with change that the user abandoned; and C, a spend of P's change and of
/// `extra` inputs, signed by another device of the wallet, paying someone
/// else.
fn abandoned_change_and_its_spend(extra: Vec<OutPoint>) -> Abandoned {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let payment = send(&engine, &session, id, CoinSource::Any, COIN / 4, false);
    mempool(&engine, &session, id, &payment);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    abandon(&engine, &session, id, payment.txid());
    assert!(!held(&engine, &session, id).contains(&change));

    let owner = Address::from_script(
        &payment.output[change.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let mut child = super::spend_of(
        change,
        vec![(
            elsewhere(),
            payment.output[change.vout as usize].value - 1_000,
        )],
    );
    child
        .input
        .extend(extra.into_iter().map(|previous_output| TxIn {
            previous_output,
            ..Default::default()
        }));
    let child = super::signed_by_wallet(&engine, &session, id, &owner, child);
    Abandoned {
        _dir: dir,
        engine,
        session,
        id,
        funding: funded,
        payment,
        child,
    }
}

/// C arrives with its lock and matches nothing, since P's change is
/// unknown; P then comes back, by its lock or a block, and C is passed again
/// right after it.
fn spend_first_then_payment(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    payment: &Transaction,
    child: &Transaction,
    by_lock: bool,
) {
    let (payment, child) = (payment.clone(), child.clone());
    deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            let first = wm
                .process_mempool_transaction(&child, Some(lock_for(&child)))
                .await;
            assert!(!first.is_relevant, "P's change is unknown");
            if by_lock {
                wm.process_mempool_transaction(&payment, Some(lock_for(&payment)))
                    .await;
            } else {
                let block = block_with(vec![payment.clone()]);
                wm.process_block_for_wallets(
                    &block,
                    block.block_hash(),
                    TIP + 1,
                    &BTreeSet::from([id.0]),
                )
                .await;
                wm.update_wallet_synced_height(&id.0, TIP + 1);
            }
            let again = wm
                .process_mempool_transaction(&child, Some(lock_for(&child)))
                .await;
            assert!(again.is_relevant, "C spends P's change");
        })
    });
}

#[test]
fn a_locked_spend_of_abandoned_change_keeps_it_spent_after_a_restart_once_its_payment_is_locked() {
    locked_spend_keeps_abandoned_change_spent_after_a_restart(true);
}

#[test]
fn a_locked_spend_of_abandoned_change_keeps_it_spent_after_a_restart_once_its_payment_is_mined() {
    locked_spend_keeps_abandoned_change_spent_after_a_restart(false);
}

/// C's record keeps P's change spent, at once and after a restart.
fn locked_spend_keeps_abandoned_change_spent_after_a_restart(by_lock: bool) {
    let Abandoned {
        _dir,
        engine,
        session,
        id,
        payment,
        child,
        ..
    } = abandoned_change_and_its_spend(vec![]);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    spend_first_then_payment(&engine, &session, id, &payment, &child, by_lock);
    assert!(
        !held(&engine, &session, id).contains(&change),
        "C spends it"
    );
    wait_stored(&session, id, &[payment.txid(), child.txid()]);

    let session = reopen(&engine, session);
    assert!(
        !held(&engine, &session, id).contains(&change),
        "C spends it after a restart"
    );
    assert!(!spendable(&engine, &session, id, change));
}

/// When D's ChainLock arrives, if at all.
#[derive(Clone, Copy, PartialEq)]
enum Lock {
    BeforeBlock,
    AfterBlock,
    Never,
}

/// What the wallet holds once D, which spends C's other input but not P's
/// change, is mined: either D also pays the wallet, so the wallet fetches
/// its block itself, or only another wallet loaded here fetches it.
struct Beaten {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    change: OutPoint,
    /// P's change output's value, from P itself.
    change_value: u64,
    winner: Txid,
    before: u64,
    /// The wallet's coins before D.
    coins_before: BTreeMap<OutPoint, u64>,
}

fn total(engine: &Engine, session: &NetworkSession, id: WalletId) -> u64 {
    coins(engine, session, id)
        .iter()
        .map(|(_, value)| value)
        .sum()
}

/// Whether the wallet store holds `txid`'s record. A read error fails the
/// test, so it never passes for an absent record.
fn records(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    persister
        .get_core_tx_record(id.0, &txid)
        .expect("read the core transaction record")
        .is_some()
}

/// The wallet's coins as the wallet store holds them.
fn stored_coins(session: &NetworkSession, id: WalletId) -> BTreeSet<OutPoint> {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let store = Arc::clone(&session.live().unwrap().store);
    let stored = store.load().unwrap();
    (stored.wallets.get(&id.0).into_iter())
        .flat_map(|w| w.wallet_info.accounts.all_funding_accounts())
        .flat_map(|a| a.utxos.keys().copied().collect::<Vec<_>>())
        .collect()
}

/// Waits for the wallet store to hold the wallet's sync height at `height`
/// or above. The store takes the wallet's events in the order they were
/// emitted, and a block's events end with its sync height, so every event
/// before it is in the store too.
fn wait_synced(session: &NetworkSession, id: WalletId, height: u32) {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let store = Arc::clone(&session.live().unwrap().store);
    super::own_spend_disk_tests::eventually("the store reaches the block's height", || {
        (store.load().unwrap().wallets.get(&id.0))
            .is_some_and(|w| w.wallet_info.metadata.synced_height >= height)
    });
}

/// The wallet's sync height as key-wallet holds it.
fn synced_height(engine: &Engine, session: &NetworkSession, id: WalletId) -> u32 {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        wm.get_wallet_info(&id.0)
            .unwrap()
            .core_wallet
            .synced_height()
    })
}

fn beaten_spend(fetched_by_wallet: bool, lock: Lock) -> Beaten {
    let other = OutPoint::new(Txid::from_byte_array([0x58; 32]), 0);
    let Abandoned {
        _dir,
        engine,
        session,
        id,
        payment,
        child,
        ..
    } = abandoned_change_and_its_spend(vec![other]);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    spend_first_then_payment(&engine, &session, id, &payment, &child, true);
    assert!(
        !held(&engine, &session, id).contains(&change),
        "C spends it"
    );
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    let before = total(&engine, &session, id);
    let coins_before: BTreeMap<OutPoint, u64> = coins(&engine, &session, id).into_iter().collect();
    let change_value = payment.output[change.vout as usize].value;

    let mut outputs = vec![(elsewhere(), 500_000)];
    let fetched_for = if fetched_by_wallet {
        outputs.push((fresh_address(&engine, &session, id).script_pubkey(), 10_000));
        id.0
    } else {
        engine
            .block_on(session.create_wallet(12))
            .unwrap()
            .wallet_id
            .0
    };
    let winner = super::spend_of(other, outputs);
    let winner_txid = winner.txid();
    let height = TIP + 2;
    deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            let block = block_with(vec![winner]);
            let chain_lock = ChainLock {
                block_height: height,
                block_hash: block.block_hash(),
                signature: BLSSignature::from([0; 96]),
            };
            if lock == Lock::BeforeBlock {
                wm.apply_chain_lock(chain_lock.clone());
            }
            wm.process_block_for_wallets(
                &block,
                block.block_hash(),
                height,
                &BTreeSet::from([fetched_for]),
            )
            .await;
            // The filter batch covers every wallet, fetched for or not.
            wm.update_wallet_synced_height(&fetched_for, height);
            wm.update_wallet_synced_height(&id.0, height);
            if lock == Lock::AfterBlock {
                wm.apply_chain_lock(chain_lock);
            }
        })
    });
    wait_synced(&session, id, height);
    if lock != Lock::Never {
        // The sweep that gives the change back is the last event.
        super::own_spend_disk_tests::eventually("the store holds the change again", || {
            stored_coins(&session, id).contains(&change)
        });
    }
    Beaten {
        _dir,
        engine,
        session,
        id,
        change,
        change_value,
        winner: winner_txid,
        before,
        coins_before,
    }
}

/// D is ChainLocked, so C can never confirm: P's change is offered again,
/// at once and after a restart, whichever comes first, D's block or its
/// lock. D also pays the wallet, so its block is one the wallet's own filter
/// fetches.
#[test]
fn a_chain_locked_conflict_beating_the_spend_gives_back_the_abandoned_change_lock_before_block() {
    a_chain_locked_conflict_beating_the_spend_gives_back_the_abandoned_change(Lock::BeforeBlock);
}

#[test]
fn a_chain_locked_conflict_beating_the_spend_gives_back_the_abandoned_change_lock_after_block() {
    a_chain_locked_conflict_beating_the_spend_gives_back_the_abandoned_change(Lock::AfterBlock);
}

fn a_chain_locked_conflict_beating_the_spend_gives_back_the_abandoned_change(lock: Lock) {
    let Beaten {
        _dir,
        engine,
        session,
        id,
        change,
        ..
    } = beaten_spend(true, lock);
    assert!(held(&engine, &session, id).contains(&change), "D beat C");
    assert!(spendable(&engine, &session, id, change));

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&change),
        "D beat C, after a restart"
    );
    assert!(spendable(&engine, &session, id, change));
}

/// Only another wallet loaded here fetches D's block. Once D is
/// ChainLocked, in either order, the wallet still drops C and gets P's
/// change back, at once and after a restart, and takes nothing else from
/// D: no record, and no balance beyond the change.
#[test]
fn a_chain_locked_conflict_another_wallet_fetches_gives_back_only_the_abandoned_change_lock_before_block()
 {
    a_chain_locked_conflict_another_wallet_fetches_gives_back_only_the_abandoned_change(
        Lock::BeforeBlock,
    );
}

#[test]
fn a_chain_locked_conflict_another_wallet_fetches_gives_back_only_the_abandoned_change_lock_after_block()
 {
    a_chain_locked_conflict_another_wallet_fetches_gives_back_only_the_abandoned_change(
        Lock::AfterBlock,
    );
}

fn a_chain_locked_conflict_another_wallet_fetches_gives_back_only_the_abandoned_change(lock: Lock) {
    let Beaten {
        _dir,
        engine,
        session,
        id,
        change,
        change_value,
        winner,
        coins_before,
        ..
    } = beaten_spend(false, lock);
    let mut expected = coins_before;
    assert!(
        expected.insert(change, change_value).is_none(),
        "C spent the change"
    );
    let after = |session: &Arc<NetworkSession>| {
        let held: BTreeMap<OutPoint, u64> = coins(&engine, session, id).into_iter().collect();
        assert_eq!(held, expected, "D beat C: only P's change comes back");
        assert!(spendable(&engine, session, id, change));
        assert!(!records(session, id, winner), "D is not the wallet's");
    };
    after(&session);
    let session = reopen(&engine, session);
    after(&session);
}

/// A plain block never overturns an InstantSend lock: with D fetched by
/// another wallet and never ChainLocked, C keeps P's change spent, at once
/// and after a restart, and the wallet takes nothing from D.
#[test]
fn a_conflict_another_wallet_fetches_without_a_chain_lock_leaves_the_change_spent() {
    let Beaten {
        _dir,
        engine,
        session,
        id,
        change,
        winner,
        before,
        ..
    } = beaten_spend(false, Lock::Never);
    assert!(!held(&engine, &session, id).contains(&change));
    assert_eq!(total(&engine, &session, id), before);
    assert!(!records(&session, id, winner));

    let session = reopen(&engine, session);
    assert!(!held(&engine, &session, id).contains(&change));
    assert_eq!(total(&engine, &session, id), before);
    assert!(!records(&session, id, winner));
}

/// D spends C's other input, which no wallet loaded here holds, and pays
/// elsewhere, so no wallet fetches its block: P's change stays spent by C.
/// Removing the wallet and restoring it from its recovery phrase frees the
/// change, since the restored wallet's scan finds the funding and P, and
/// never C. Pins the way out for a known gap.
#[test]
fn a_change_a_conflict_no_wallet_fetches_frees_is_spendable_once_the_wallet_is_restored() {
    let other = OutPoint::new(Txid::from_byte_array([0x59; 32]), 0);
    let Abandoned {
        _dir,
        engine,
        session,
        id,
        funding,
        payment,
        child,
    } = abandoned_change_and_its_spend(vec![other]);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    spend_first_then_payment(&engine, &session, id, &payment, &child, true);
    mined(&engine, &session, id, TIP + 1, vec![payment.clone()]);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    assert!(
        !held(&engine, &session, id).contains(&change),
        "D's block is not fetched, so C still spends the change"
    );

    remove(&engine, &session, id);
    let session = reopen(&engine, session);
    let id = restore(&engine, &session, super::PHRASE);
    mined(&engine, &session, id, FUNDED_AT, vec![funding]);
    mined(&engine, &session, id, TIP + 1, vec![payment.clone()]);
    assert!(
        held(&engine, &session, id).contains(&change),
        "the restored wallet never saw C"
    );
    assert!(spendable(&engine, &session, id, change));
    wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&change));
    assert!(spendable(&engine, &session, id, change));
}

/// For a failure's message: which of `coins` key-wallet holds, and what
/// the history and key-wallet hold of each named transaction.
fn known_gap_story(
    engine: &Engine,
    session: &NetworkSession,
    id: WalletId,
    (a, b): (OutPoint, OutPoint),
    named: &[(&str, Txid)],
) -> String {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let held: Vec<&str> = (coins(engine, session, id).iter())
        .filter_map(|(o, _)| (*o == a).then_some("A").or((*o == b).then_some("B")))
        .collect();
    let history = session.hub.history.snapshot(&id);
    let manager = session.manager().unwrap();
    let txids: Vec<Txid> = named.iter().map(|(_, t)| *t).collect();
    let records: Vec<Option<String>> = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&id.0);
        let records = info
            .map(|i| i.core_wallet.transaction_history())
            .unwrap_or_default();
        (txids.iter())
            .map(|t| (records.iter().find(|r| r.txid == *t)).map(|r| format!("{:?}", r.context)))
            .collect()
    });
    let each: Vec<String> = (named.iter().zip(records))
        .map(|((name, t), record)| {
            let shown = (history.get(t)).map(|e| {
                format!(
                    "{:?} locked={} abandoned={}",
                    e.context, e.instant_locked, e.abandoned
                )
            });
            format!("{name} {t}: history {shown:?}, key-wallet {record:?}")
        })
        .collect();
    format!("held {held:?} (A {a}, B {b})\n{}", each.join("\n"))
}

/// Pins a known gap. L, which the user abandoned, spends coins A and B;
/// another payment E, as another device on the same recovery phrase would
/// make, spends B. L then comes back locked, so E loses to it, and a
/// ChainLocked W beats L on A: B is handed back as free although E spends
/// it, until E is in a block.
#[test]
fn known_gap_a_coin_another_device_spent_shows_again_when_the_abandoned_payment_loses() {
    let (_dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let (a, b) = (
        OutPoint::new(funded.txid(), 0),
        OutPoint::new(funded.txid(), 1),
    );
    let loser = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a, b]),
        COIN + COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &loser);
    abandon(&engine, &session, id, loser.txid());
    let session = reopen(&engine, session);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    assert!(held(&engine, &session, id).is_superset(&BTreeSet::from([a, b])));
    let other_device = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &to[1],
        super::spend_of(b, vec![(elsewhere(), COIN - 1_000)]),
    );
    mempool(&engine, &session, id, &other_device);
    assert!(!held(&engine, &session, id).contains(&b), "E spends B");

    let locked = loser.clone();
    deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&locked, Some(lock_for(&locked)))
                .await;
        })
    });
    let winner = super::spend_of(
        a,
        vec![
            (elsewhere(), COIN / 2),
            (fresh_address(&engine, &session, id).script_pubkey(), 10_000),
        ],
    );
    let named = [
        ("L", loser.txid()),
        ("W", winner.txid()),
        ("E", other_device.txid()),
    ];
    let height = TIP + 1;
    deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            let block = block_with(vec![winner]);
            wm.apply_chain_lock(ChainLock {
                block_height: height,
                block_hash: block.block_hash(),
                signature: BLSSignature::from([0; 96]),
            });
            wm.process_block_for_wallets(
                &block,
                block.block_hash(),
                height,
                &BTreeSet::from([id.0]),
            )
            .await;
            wm.update_wallet_synced_height(&id.0, height);
        })
    });
    assert!(
        !held(&engine, &session, id).contains(&b),
        "not credited in this session"
    );

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&b),
        "known gap: after a restart B is offered although E spends it"
    );
    assert!(spendable(&engine, &session, id, b));

    mined(&engine, &session, id, TIP + 2, vec![other_device]);
    let coins = coins(&engine, &session, id);
    assert!(
        !coins.iter().any(|(coin, _)| *coin == a || *coin == b),
        "E is in a block: {}",
        known_gap_story(&engine, &session, id, (a, b), &named)
    );
    let total: u64 = coins.iter().map(|(_, value)| value).sum();
    assert_eq!(
        total,
        (COINS as u64 - 2) * COIN + 10_000,
        "nothing counted twice"
    );
    let session = reopen(&engine, session);
    assert!(
        !held(&engine, &session, id).contains(&b),
        "E is in a block, after a restart: {}",
        known_gap_story(&engine, &session, id, (a, b), &named)
    );
}

/// Runs `op` on the session's wallet manager, then hands the hub the
/// events it emitted and waits for the store of every wallet in `wallets`.
fn deliver_all<F>(engine: &Engine, session: &NetworkSession, wallets: &[WalletId], op: F)
where
    F: for<'a> FnOnce(
            &'a mut platform_wallet::key_wallet_manager::WalletManager<
                platform_wallet::PlatformWalletInfo,
            >,
        ) -> futures_util::future::BoxFuture<'a, ()>
        + Send
        + 'static,
{
    use platform_wallet::changeset::PlatformWalletPersistence;
    deliver(engine, session, wallets[0], op);
    let persister = Arc::clone(&session.live().unwrap().persister);
    for id in wallets {
        persister.flush(id.0).unwrap();
    }
}

fn block_for(
    engine: &Engine,
    session: &NetworkSession,
    wallets: &[WalletId],
    fetched_for: &[WalletId],
    height: u32,
    txs: Vec<Transaction>,
) {
    let ids: BTreeSet<[u8; 32]> = fetched_for.iter().map(|id| id.0).collect();
    let all: Vec<[u8; 32]> = wallets.iter().map(|id| id.0).collect();
    let advancing: Vec<WalletId> = (wallets.iter().copied())
        .filter(|id| synced_height(engine, session, *id) < height)
        .collect();
    deliver_all(engine, session, wallets, move |wm| {
        Box::pin(async move {
            let block = block_with(txs);
            wm.process_block_for_wallets(&block, block.block_hash(), height, &ids)
                .await;
            for id in &all {
                wm.update_wallet_synced_height(id, height);
            }
        })
    });
    for id in advancing {
        wait_synced(session, id, height);
    }
}

fn remove(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) {
    let wipe = (session.vault())
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    engine.block_on(session.remove_wallet(id, wipe)).unwrap();
}

/// Restores the wallet of `phrase`, born at [`super::BIRTH`].
fn restore(engine: &Engine, session: &Arc<NetworkSession>, phrase: &[u8]) -> WalletId {
    engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(phrase.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(super::BIRTH),
                ..Default::default()
            },
        ))
        .unwrap()
}

#[derive(Clone, Copy, PartialEq)]
enum Restart {
    AfterTheBlock,
    BeforeTheBlock,
}

/// Dash Wallet restarts after C, locked, spends P's change and the other
/// wallet's X, and before D's block and its ChainLock are both processed.
/// The wallet store keeps no unconfirmed record in memory across a restart,
/// so nothing can overturn C: Reset chain data and Rescan from the wallet's
/// birth replay D's block under its ChainLock, and C stays, with P's change
/// spent. Removing the wallet and restoring it from its recovery phrase
/// releases the change once P is found again, relayed or mined, at once and
/// after a restart. The other wallet keeps C until it is restored too.
fn restart_before_the_conflicts_chain_lock(restart: Restart) {
    let (_dir, engine, session, id) = engine_wallet();
    let other = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    let both = [id, other];
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    let to_other = fresh_address(&engine, &session, other);
    let funded_other = funding(8, &[to_other], COIN);
    let x = OutPoint::new(funded_other.txid(), 0);
    let funding_block = vec![funded.clone(), funded_other.clone()];
    block_for(
        &engine,
        &session,
        &both,
        &both,
        FUNDED_AT,
        funding_block.clone(),
    );
    block_for(&engine, &session, &both, &[], TIP, vec![foreign()]);
    let payment = send(&engine, &session, id, CoinSource::Any, COIN / 4, false);
    mempool(&engine, &session, id, &payment);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    abandon(&engine, &session, id, payment.txid());
    let session = reopen(&engine, session);
    let owner = Address::from_script(
        &payment.output[change.vout as usize].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let mut child = super::spend_of(
        change,
        vec![(
            elsewhere(),
            payment.output[change.vout as usize].value - 1_000,
        )],
    );
    child.input.push(TxIn {
        previous_output: x,
        ..Default::default()
    });
    let child = super::signed_by_wallet(&engine, &session, id, &owner, child);
    let (back, spend) = (payment.clone(), child.clone());
    deliver_all(&engine, &session, &both, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&spend, Some(lock_for(&spend)))
                .await;
            wm.process_mempool_transaction(&back, Some(lock_for(&back)))
                .await;
            wm.process_mempool_transaction(&spend, Some(lock_for(&spend)))
                .await;
        })
    });
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    wait_stored(&session, other, &[child.txid()]);

    let winner = super::spend_of(x, vec![(elsewhere(), COIN - 1_000)]);
    let height = TIP + 2;
    let chain_lock = ChainLock {
        block_height: height,
        block_hash: block_with(vec![winner.clone()]).block_hash(),
        signature: BLSSignature::from([0; 96]),
    };
    let lock = |session: &NetworkSession| {
        let chain_lock = chain_lock.clone();
        deliver_all(&engine, session, &both, move |wm| {
            Box::pin(async move {
                wm.apply_chain_lock(chain_lock);
            })
        });
    };
    let replay_d = |session: &NetworkSession, wallets: &[WalletId]| {
        block_for(
            &engine,
            session,
            wallets,
            &[other],
            height,
            vec![winner.clone()],
        );
    };
    let session = if restart == Restart::AfterTheBlock {
        replay_d(&session, &both);
        let session = reopen(&engine, session);
        lock(&session);
        session
    } else {
        let session = reopen(&engine, session);
        lock(&session);
        replay_d(&session, &both);
        session
    };
    let overturned_stays = |session: &Arc<NetworkSession>| {
        assert!(!held(&engine, session, id).contains(&change), "C spends it");
        assert!(records(session, id, child.txid()), "C stays");
        assert!(
            records(session, other, child.txid()),
            "the other wallet keeps C too"
        );
        assert!(!held(&engine, session, other).contains(&x), "X stays spent");
    };
    overturned_stays(&session);

    let replay = |session: &NetworkSession| {
        block_for(
            &engine,
            session,
            &both,
            &both,
            FUNDED_AT,
            funding_block.clone(),
        );
        block_for(&engine, session, &both, &[], TIP, vec![foreign()]);
        replay_d(session, &both);
    };
    engine.block_on(session.reset_chain_data()).unwrap();
    replay(&session);
    overturned_stays(&session);
    let session = reopen(&engine, session);
    overturned_stays(&session);

    start_offline_spv(&engine, &session);
    engine
        .block_on(session.rescan(crate::RescanFrom::WalletBirth))
        .unwrap();
    replay(&session);
    overturned_stays(&session);
    let session = reopen(&engine, session);
    overturned_stays(&session);

    remove(&engine, &session, id);
    let session = reopen(&engine, session);
    let id = restore(&engine, &session, super::PHRASE);
    let both = [id, other];
    block_for(&engine, &session, &both, &[id], FUNDED_AT, funding_block);
    block_for(&engine, &session, &both, &[], TIP, vec![foreign()]);
    replay_d(&session, &both);
    assert!(
        !records(&session, id, child.txid()),
        "the restored wallet never saw C"
    );
    assert!(
        !held(&engine, &session, id).contains(&change),
        "P is not found yet"
    );

    let back = payment.clone();
    deliver_all(&engine, &session, &both, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&back, Some(lock_for(&back)))
                .await;
        })
    });
    wait_stored(&session, id, &[payment.txid()]);
    let spent_by_p: Vec<OutPoint> = payment.input.iter().map(|i| i.previous_output).collect();
    let released = |session: &Arc<NetworkSession>| {
        let coins = held(&engine, session, id);
        assert!(coins.contains(&change), "P's change is back");
        assert!(spendable(&engine, session, id, change));
        assert!(spent_by_p.iter().all(|coin| !coins.contains(coin)));
        assert!(!records(session, id, child.txid()));
        assert!(
            records(session, other, child.txid()),
            "until it is restored too"
        );
    };
    released(&session);
    let session = reopen(&engine, session);
    released(&session);

    block_for(
        &engine,
        &session,
        &both,
        &[id],
        height + 1,
        vec![payment.clone()],
    );
    released(&session);
    let session = reopen(&engine, session);
    released(&session);
}

#[test]
fn a_restart_after_a_conflicts_block_keeps_the_overturned_spend_through_a_rescan_until_a_restore() {
    restart_before_the_conflicts_chain_lock(Restart::AfterTheBlock);
}

#[test]
fn a_restart_before_a_conflicts_block_keeps_the_overturned_spend_through_a_rescan_until_a_restore()
{
    restart_before_the_conflicts_chain_lock(Restart::BeforeTheBlock);
}

struct Overturned {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    c: Transaction,
    back: OutPoint,
    /// Each block's height, transactions and whether the wallet fetches it.
    blocks: Vec<(u32, Vec<Transaction>, bool)>,
}

/// C, locked, spends the wallet's coin A0 and a foreign X and pays the
/// wallet back: C:0 is money C sent to this wallet. D, mined and then
/// ChainLocked, spends A0 when `fetched`, so the wallet fetches its block,
/// and X only otherwise, so no wallet does. With `restart`, Dash Wallet
/// restarts between C and D. Returns the session after D's lock.
fn money_an_overturned_payment_sent(fetched: bool, restart: bool) -> Overturned {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    block_for(
        &engine,
        &session,
        &[id],
        &[id],
        FUNDED_AT,
        vec![funded.clone()],
    );
    block_for(&engine, &session, &[id], &[], TIP, vec![foreign()]);
    let a0 = OutPoint::new(funded.txid(), 0);
    let x = OutPoint::new(Txid::from_byte_array([0x57; 32]), 0);
    let back_to = fresh_address(&engine, &session, id);
    let mut c = super::spend_of(a0, vec![(back_to.script_pubkey(), COIN * 3 / 4)]);
    c.input.push(TxIn {
        previous_output: x,
        ..Default::default()
    });
    let c = super::signed_by_wallet(&engine, &session, id, &to[0], c);
    let back = OutPoint::new(c.txid(), 0);
    let locked = c.clone();
    deliver_all(&engine, &session, &[id], move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&locked, Some(lock_for(&locked)))
                .await;
        })
    });
    wait_stored(&session, id, &[c.txid()]);
    assert!(held(&engine, &session, id).contains(&back));

    let session = if restart {
        reopen(&engine, session)
    } else {
        session
    };
    let d = super::spend_of(if fetched { a0 } else { x }, vec![(elsewhere(), 500_000)]);
    let height = TIP + 2;
    let chain_lock = ChainLock {
        block_height: height,
        block_hash: block_with(vec![d.clone()]).block_hash(),
        signature: BLSSignature::from([0; 96]),
    };
    let fetched_for: &[WalletId] = if fetched { &[id] } else { &[] };
    block_for(
        &engine,
        &session,
        &[id],
        fetched_for,
        height,
        vec![d.clone()],
    );
    deliver_all(&engine, &session, &[id], move |wm| {
        Box::pin(async move {
            wm.apply_chain_lock(chain_lock);
        })
    });
    Overturned {
        _dir: dir,
        engine,
        session,
        id,
        c,
        back,
        blocks: vec![
            (FUNDED_AT, vec![funded], true),
            (TIP, vec![foreign()], false),
            (height, vec![d], fetched),
        ],
    }
}

/// The usual order, D's block and then its ChainLock, in a wallet that
/// fetches the block: C's money to the wallet goes, at once and after a
/// restart. On main it stayed offered for good.
#[test]
fn money_an_overturned_payment_sent_goes_once_the_conflicts_chain_lock_lands() {
    let Overturned {
        _dir,
        engine,
        session,
        id,
        c,
        back,
        ..
    } = money_an_overturned_payment_sent(true, false);
    assert!(
        !held(&engine, &session, id).contains(&back),
        "C:0 never existed"
    );
    assert_eq!(
        total(&engine, &session, id),
        (COINS as u64 - 1) * COIN,
        "D spends A0"
    );
    let session = reopen(&engine, session);
    assert!(!held(&engine, &session, id).contains(&back));
    assert!(!records(&session, id, c.txid()), "C is gone");
}

/// C's money to the wallet stays offered, at once and after a restart.
/// Quitting and reopening Dash Wallet, then removing the wallet and
/// restoring it from its recovery phrase, takes it away for good: the
/// restored wallet's scan never finds C, so A0 is back unless D spent it.
fn money_stays_offered(fetched: bool, restart: bool) {
    let Overturned {
        _dir,
        engine,
        session,
        id,
        c,
        back,
        blocks,
    } = money_an_overturned_payment_sent(fetched, restart);
    let offered = |session: &Arc<NetworkSession>| {
        assert!(
            held(&engine, session, id).contains(&back),
            "known gap: C:0 shows"
        );
        assert!(
            spendable(&engine, session, id, back),
            "known gap: C:0 is offered"
        );
        assert!(records(session, id, c.txid()), "C stays pending");
    };
    offered(&session);
    let session = reopen(&engine, session);
    offered(&session);

    remove(&engine, &session, id);
    let id = restore(&engine, &session, super::PHRASE);
    for (height, txs, fetched) in blocks {
        let fetched_for: &[WalletId] = if fetched { &[id] } else { &[] };
        block_for(&engine, &session, &[id], fetched_for, height, txs);
    }
    let unspent = if fetched { COINS - 1 } else { COINS };
    let gone = |session: &Arc<NetworkSession>| {
        assert!(
            !held(&engine, session, id).contains(&back),
            "the restore takes C:0 away"
        );
        assert!(!records(session, id, c.txid()), "and C");
        assert_eq!(total(&engine, session, id), unspent as u64 * COIN);
    };
    gone(&session);
    let session = reopen(&engine, session);
    gone(&session);
}

/// Pins a known gap (#338): a restart between C and D leaves C's money to
/// the wallet offered until it is restored, although the wallet fetches D's
/// block. The same on main.
#[test]
fn known_gap_after_a_restart_money_an_overturned_payment_sent_stays_offered() {
    money_stays_offered(true, true);
}

/// Pins a known gap (#327): no wallet fetches D's block, so C's money to the
/// wallet stays offered until it is restored. The same on main.
#[test]
fn known_gap_money_an_overturned_payment_sent_stays_offered_when_no_wallet_fetches_the_conflict() {
    money_stays_offered(false, false);
}

/// Pins the same known gap (#327, #338) across a restart between C and D.
#[test]
fn known_gap_money_an_overturned_payment_sent_stays_offered_after_a_restart_when_no_wallet_fetches_the_conflict()
 {
    money_stays_offered(false, true);
}

const OTHER_PHRASE: &[u8] =
    b"legal winner thank year wave sausage worth useful legal winner thank yellow";

/// Whom C2 pays.
#[derive(Clone, Copy, PartialEq)]
enum Payee {
    /// Another wallet B open in Dash Wallet.
    AnotherWallet,
    /// The same wallet's CoinJoin account.
    CoinJoinAccount,
}

/// The wallet's first CoinJoin receive address.
fn coinjoin_address(engine: &Engine, session: &NetworkSession, id: WalletId) -> Address {
    use key_wallet::account::AccountType;
    use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
    let manager = session.manager().unwrap();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
        let account = (info.accounts)
            .funds_account(&AccountType::CoinJoin { index: 0 })
            .unwrap();
        account.managed_account_type().address_pools()[0]
            .address_at_index(0)
            .unwrap()
    })
}

/// Pins a known gap. L, locked, spends the wallet's coins A0 and A1 with
/// change; C2, locked, spends that change and pays `payee`. D, mined and
/// then ChainLocked, spends A1. The wallet's standard account drops L and C2
/// together, but the account C2 pays records only C2, whose input D does
/// not spend, so it keeps C2:0.
///
/// Another wallet keeps C2 pending and C2:0 offered, also after a restart.
/// The same wallet's CoinJoin account counts C2:0 in the balance until a
/// restart; the wallet's store already dropped C2, so no payment is offered
/// C2:0, history shows no C2, and a restart clears it. Either way, quitting
/// and reopening Dash Wallet, then removing that wallet and restoring it
/// from its recovery phrase, leaves C2:0 gone for good.
fn built_on_an_overturned_payment(payee: Payee) {
    let (_dir, engine, session, id) = engine_wallet();
    let (holder, phrase) = match payee {
        Payee::AnotherWallet => (restore(&engine, &session, OTHER_PHRASE), OTHER_PHRASE),
        Payee::CoinJoinAccount => (id, super::PHRASE),
    };
    let wallets = |id: WalletId, holder: WalletId| -> Vec<WalletId> {
        if id == holder {
            vec![id]
        } else {
            vec![id, holder]
        }
    };
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    let (a0, a1) = (
        OutPoint::new(funded.txid(), 0),
        OutPoint::new(funded.txid(), 1),
    );
    let blocks = |session: &NetworkSession, wallets: &[WalletId], fetcher: WalletId| {
        block_for(
            &engine,
            session,
            wallets,
            &[fetcher],
            FUNDED_AT,
            vec![funded.clone()],
        );
        block_for(&engine, session, wallets, &[], TIP, vec![foreign()]);
    };
    let all = wallets(id, holder);
    blocks(&session, &all, id);
    let change_to = fresh_address(&engine, &session, id);
    let mut l = super::spend_of(
        a0,
        vec![(change_to.script_pubkey(), COIN), (elsewhere(), COIN / 2)],
    );
    l.input.push(TxIn {
        previous_output: a1,
        ..Default::default()
    });
    let l = super::signed_by_wallet(&engine, &session, id, &to[0], l);
    let to_holder = match payee {
        Payee::AnotherWallet => fresh_address(&engine, &session, holder),
        Payee::CoinJoinAccount => coinjoin_address(&engine, &session, holder),
    };
    let c2 = super::spend_of(
        OutPoint::new(l.txid(), 0),
        vec![(to_holder.script_pubkey(), COIN - 1_000)],
    );
    let c2 = super::signed_by_wallet(&engine, &session, id, &change_to, c2);
    let paid = OutPoint::new(c2.txid(), 0);
    let (first, second) = (l.clone(), c2.clone());
    deliver_all(&engine, &session, &all, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&first, Some(lock_for(&first)))
                .await;
            wm.process_mempool_transaction(&second, Some(lock_for(&second)))
                .await;
        })
    });
    wait_stored(&session, id, &[l.txid(), c2.txid()]);
    wait_stored(&session, holder, &[c2.txid()]);
    assert!(held(&engine, &session, holder).contains(&paid));

    let d = super::spend_of(a1, vec![(elsewhere(), COIN - 1_000)]);
    let height = TIP + 2;
    let chain_lock = ChainLock {
        block_height: height,
        block_hash: block_with(vec![d.clone()]).block_hash(),
        signature: BLSSignature::from([0; 96]),
    };
    block_for(&engine, &session, &all, &[id], height, vec![d.clone()]);
    deliver_all(&engine, &session, &all, move |wm| {
        Box::pin(async move {
            wm.apply_chain_lock(chain_lock);
        })
    });
    super::own_spend_disk_tests::eventually("D beats L", || !records(&session, id, l.txid()));
    if payee == Payee::AnotherWallet {
        super::own_spend_disk_tests::eventually("C2 goes with L", || {
            !records(&session, id, c2.txid())
        });
    }
    assert!(held(&engine, &session, id).contains(&a0));

    let left = (COINS as u64 - 1) * COIN;
    let session = match payee {
        Payee::AnotherWallet => {
            let offered = |session: &Arc<NetworkSession>| {
                assert!(
                    held(&engine, session, holder).contains(&paid),
                    "known gap: C2:0 shows"
                );
                assert!(
                    spendable(&engine, session, holder, paid),
                    "known gap: C2:0 is offered"
                );
                assert!(records(session, holder, c2.txid()), "C2 stays pending");
            };
            offered(&session);
            let session = reopen(&engine, session);
            offered(&session);
            session
        }
        Payee::CoinJoinAccount => {
            assert!(
                held(&engine, &session, holder).contains(&paid),
                "known gap: the CoinJoin account keeps C2:0"
            );
            let b = super::balances(&session, holder);
            assert_eq!(
                (b.total, b.coinjoin),
                (left + COIN - 1_000, COIN - 1_000),
                "known gap: C2:0 counts in the balance and the CoinJoin balance"
            );
            assert!(
                !spendable(&engine, &session, holder, paid),
                "no payment is offered C2:0"
            );
            super::own_spend_disk_tests::eventually("no history row for C2", || {
                !records(&session, holder, c2.txid())
                    && !(super::rows(&engine, &session, holder).iter())
                        .any(|row| row.0 == c2.txid().to_string())
            });
            let session = reopen(&engine, session);
            assert!(
                !held(&engine, &session, holder).contains(&paid),
                "a restart drops C2:0"
            );
            let b = super::balances(&session, holder);
            assert_eq!((b.total, b.coinjoin), (left, 0));
            session
        }
    };

    remove(&engine, &session, holder);
    let restored = restore(&engine, &session, phrase);
    let id = if payee == Payee::CoinJoinAccount {
        restored
    } else {
        id
    };
    let all = wallets(id, restored);
    blocks(&session, &all, id);
    block_for(&engine, &session, &all, &[id], height, vec![d]);
    let left = match payee {
        Payee::AnotherWallet => 0,
        Payee::CoinJoinAccount => left,
    };
    let gone = |session: &Arc<NetworkSession>| {
        assert!(
            !held(&engine, session, restored).contains(&paid),
            "the restore takes C2:0 away"
        );
        assert!(!records(session, restored, c2.txid()), "and C2");
        assert_eq!(total(&engine, session, restored), left);
    };
    gone(&session);
    let session = reopen(&engine, session);
    gone(&session);
}

#[test]
fn known_gap_money_a_locked_payment_built_on_an_overturned_one_sent_another_wallet_stays_until_it_is_restored()
 {
    built_on_an_overturned_payment(Payee::AnotherWallet);
}

#[test]
fn known_gap_money_a_locked_payment_built_on_an_overturned_one_sent_the_wallets_coinjoin_account_counts_until_a_restart()
 {
    built_on_an_overturned_payment(Payee::CoinJoinAccount);
}

/// D, which beats C, is mined but its ChainLock has not landed when the
/// user resets the chain data. The reset forgets D: its ChainLock alone then
/// overturns nothing. Once D's block is fetched again, under that lock, C
/// goes and P's change comes back.
#[test]
fn reset_chain_data_forgets_a_conflict_kept_for_its_chain_lock_until_its_block_is_fetched_again() {
    let other = OutPoint::new(Txid::from_byte_array([0x56; 32]), 0);
    let Abandoned {
        _dir,
        engine,
        session,
        id,
        payment,
        child,
        ..
    } = abandoned_change_and_its_spend(vec![other]);
    let change = change_of(&payment, &own_scripts(&engine, &session, id));
    spend_first_then_payment(&engine, &session, id, &payment, &child, true);
    wait_stored(&session, id, &[payment.txid(), child.txid()]);
    let fetcher = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    let both = [id, fetcher];
    let winner = super::spend_of(other, vec![(elsewhere(), 500_000)]);
    let height = TIP + 2;
    block_for(
        &engine,
        &session,
        &both,
        &[fetcher],
        height,
        vec![winner.clone()],
    );
    assert!(
        !held(&engine, &session, id).contains(&change),
        "a plain block overturns no lock"
    );

    engine.block_on(session.reset_chain_data()).unwrap();
    let chain_lock = ChainLock {
        block_height: height,
        block_hash: block_with(vec![winner.clone()]).block_hash(),
        signature: BLSSignature::from([0; 96]),
    };
    deliver_all(&engine, &session, &both, move |wm| {
        Box::pin(async move {
            wm.apply_chain_lock(chain_lock);
        })
    });
    assert!(
        !held(&engine, &session, id).contains(&change),
        "the reset forgot D"
    );
    assert!(records(&session, id, child.txid()));

    block_for(&engine, &session, &both, &[fetcher], height, vec![winner]);
    assert!(held(&engine, &session, id).contains(&change), "D beat C");
    assert!(spendable(&engine, &session, id, change));
}
