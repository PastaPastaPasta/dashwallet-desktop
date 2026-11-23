//! A payment the user abandoned that went through after all, whose change a
//! locked payment had already spent (#302). Once the payment comes back the
//! locked spend is passed to the wallet again, as dash-spv's mempool manager
//! does, so the wallet records it: the change stays spent across a restart,
//! and comes back, at once and after a restart, when a ChainLocked conflict
//! beats the spend.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::bls_sig_utils::BLSSignature;
use dashcore::hashes::Hash as _;
use dashcore::{Address, ChainLock, InstantLock, OutPoint, Transaction, TxIn, Txid};
use platform_wallet::key_wallet_manager::WalletInterface;

use super::own_spend_disk_tests::{
    COIN, COINS, FUNDED_AT, TIP, abandon, change_of, coins, foreign, fresh_address, funding,
    own_scripts, send, spendable, wait_stored,
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
    winner: Txid,
    before: u64,
}

fn total(engine: &Engine, session: &NetworkSession, id: WalletId) -> u64 {
    coins(engine, session, id)
        .iter()
        .map(|(_, value)| value)
        .sum()
}

fn records(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    persister
        .get_core_tx_record(id.0, &txid)
        .is_ok_and(|r| r.is_some())
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
    Beaten {
        _dir,
        engine,
        session,
        id,
        change,
        winner: winner_txid,
        before,
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
        winner,
        before,
    } = beaten_spend(false, lock);
    let value = coins(&engine, &session, id)
        .into_iter()
        .find(|(coin, _)| *coin == change)
        .expect("D beat C")
        .1;
    assert_eq!(total(&engine, &session, id), before + value);
    assert!(spendable(&engine, &session, id, change));
    assert!(!records(&session, id, winner), "D is not the wallet's");

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&change),
        "after a restart"
    );
    assert_eq!(total(&engine, &session, id), before + value);
    assert!(spendable(&engine, &session, id, change));
    assert!(!records(&session, id, winner));
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
    use crate::ImportOptions;
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

    let wipe = (session.vault())
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    engine.block_on(session.remove_wallet(id, wipe)).unwrap();
    let session = reopen(&engine, session);
    let id = engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(super::PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(super::BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
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
        "E is in a block"
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
        "E is in a block, after a restart"
    );
}
