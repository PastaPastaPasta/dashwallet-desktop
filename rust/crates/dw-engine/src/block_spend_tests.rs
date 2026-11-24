//! The durable block-spend rows (#268, #400) at the engine level: the load
//! restores claims through the platform wrapper so a block's spend of a
//! claimed coin is stored, a fork drops the rows above it in either order,
//! and the load-time check drops a row whose block the header chain no
//! longer names.

use std::collections::BTreeMap;
use std::sync::Arc;

use dashcore::{Address, BlockHash, OutPoint, Transaction};
use platform_wallet::changeset::BlockSpend;

use super::own_spend_disk_tests::{
    COIN, COINS, FUNDED_AT, TIP, abandon, chain_lock, coins, eventually, foreign, fresh_address,
    funding,
    header_chain, send, spendable, wait_stored,
};
use super::{block_with, elsewhere, engine_wallet, mempool, mined, reopen};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{Engine, WalletId};

/// The wallet store's block-spend rows of `id`.
fn stored_rows(session: &NetworkSession, id: WalletId) -> BTreeMap<OutPoint, BlockSpend> {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    persister.block_spent_outpoints(&id.0).unwrap()
}

pub(super) struct Claimed {
    pub _dir: tempfile::TempDir,
    pub engine: Engine,
    pub session: Arc<NetworkSession>,
    pub id: WalletId,
    pub a: OutPoint,
    pub p: Transaction,
}

impl Claimed {
    fn held(&self) -> bool {
        coins(&self.engine, &self.session, self.id)
            .iter()
            .any(|(o, _)| *o == self.a)
    }

    fn reopen(mut self) -> Self {
        self.session = reopen(&self.engine, self.session);
        self
    }

    fn check(&self) -> bool {
        let s = Arc::clone(&self.session);
        let id = self.id;
        (self.engine)
            .block_on(self.session.on_runtime(async move {
                let manager = s.manager()?;
                s.check_block_spends(&manager, id).await
            }))
            .unwrap()
    }
}

/// [`COINS`] coins of one DASH, A among them; P pays from A and stays
/// unconfirmed over a restart, so the load holds A only by P's restored
/// claim: key-wallet keeps no record of P.
pub(super) fn claimed() -> Claimed {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    mined(&engine, &session, id, FUNDED_AT, vec![funded.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let a = OutPoint::new(funded.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let session = reopen(&engine, session);
    Claimed {
        _dir: dir,
        engine,
        session,
        id,
        a,
        p,
    }
}

/// Waits for the wallet store to hold a row for `a` (`true`) or none: the
/// persister hears the capture through its own event bridge.
fn row_stored(fx: &Claimed, held: bool) -> Option<BlockSpend> {
    eventually("the store's block-spend row", || {
        stored_rows(&fx.session, fx.id).contains_key(&fx.a) == held
    });
    stored_rows(&fx.session, fx.id).get(&fx.a).copied()
}

/// W, a spend of `a` paying only someone else.
fn w_of(a: OutPoint) -> Transaction {
    super::spend_of(a, vec![(elsewhere(), COIN - 1_000)])
}

/// (g) The block of a foreign spend of a coin only P's restored claim
/// holds is stored as a row: the load restores the claim through the
/// platform wrapper, which captures the spend.
#[test]
fn a_block_spend_of_a_coin_only_a_restored_claim_holds_is_stored() {
    let fx = claimed();
    assert!(!fx.held(), "P's claim is restored");
    let w = w_of(fx.a);
    let hash = block_with(vec![w.clone()]).block_hash();
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![w.clone()]);
    let expected = BlockSpend {
        outpoint: fx.a,
        height: TIP + 1,
        block_hash: hash,
        spender_txid: w.txid(),
    };
    assert_eq!(row_stored(&fx, true), Some(expected));
    assert_eq!(
        fx.session.hub.block_spends.rows(&fx.id).get(&fx.a),
        Some(&expected),
        "the session holds the row too"
    );
    let fx = fx.reopen();
    assert_eq!(
        fx.session.hub.block_spends.rows(&fx.id).get(&fx.a),
        Some(&expected),
        "the load reads it back"
    );
}

/// (i) W's block, then a fork that replaces it with a block not spending
/// A: the row goes from the store and the session, and abandoning P then
/// gives A back.
#[test]
fn a_fork_replacing_the_spenders_block_drops_its_row() {
    let fx = claimed();
    let w = w_of(fx.a);
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![w]);
    row_stored(&fx, true);
    super::deliver(&fx.engine, &fx.session, fx.id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.truncate_above(TIP).unwrap();
        })
    });
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![foreign()]);
    row_stored(&fx, false);
    assert!(
        !fx.session.hub.block_spends.has_row(&fx.id, &fx.a),
        "the session drops it"
    );
    abandon(&fx.engine, &fx.session, fx.id, fx.p.txid());
    assert_eq!(
        *fx.session.hub.last_release.lock().unwrap(),
        Some(vec![fx.a]),
        "A is released: no block on the best chain spends it"
    );
}

/// (i) The other order: a block not spending A, then a fork that replaces
/// it with W's block. The row names the new branch's block.
#[test]
fn a_fork_onto_the_spenders_block_stores_its_row() {
    let fx = claimed();
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![foreign()]);
    super::deliver(&fx.engine, &fx.session, fx.id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.truncate_above(TIP).unwrap();
        })
    });
    let w = w_of(fx.a);
    let hash = block_with(vec![w.clone()]).block_hash();
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![w]);
    assert_eq!(row_stored(&fx, true).map(|r| r.block_hash), Some(hash));
    abandon(&fx.engine, &fx.session, fx.id, fx.p.txid());
    assert_eq!(*fx.session.hub.last_release.lock().unwrap(), Some(vec![]));
    let fx = fx.reopen();
    assert!(!fx.held(), "A stays spent after a restart");
    assert!(!spendable(&fx.engine, &fx.session, fx.id, fx.a));
}

/// (i), C2: a fork found while catching up after a restart. W's row is
/// above the wallet's ChainLock; the header chain read after the restart
/// names another block at its height, so the load-time check drops it and
/// an abandon of P then gives A back. A header still unknown keeps the row.
#[test]
fn a_fork_found_after_a_restart_drops_a_row_above_the_chain_lock() {
    let fx = claimed();
    let w = w_of(fx.a);
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![w]);
    row_stored(&fx, true);
    let fx = fx.reopen();
    header_chain(&fx.session, &[]);
    assert!(!fx.check(), "an unknown header keeps the row");
    assert!(stored_rows(&fx.session, fx.id).contains_key(&fx.a));
    let other = block_with(vec![foreign()]).block_hash();
    header_chain(&fx.session, &[(TIP + 1, other)]);
    assert!(fx.check(), "another block at its height drops it");
    assert!(!stored_rows(&fx.session, fx.id).contains_key(&fx.a));
    assert!(!fx.session.hub.block_spends.has_row(&fx.id, &fx.a));
    abandon(&fx.engine, &fx.session, fx.id, fx.p.txid());
    assert_eq!(
        *fx.session.hub.last_release.lock().unwrap(),
        Some(vec![fx.a])
    );
}

/// C2: the check leaves a row the header chain confirms, and never looks
/// at a row at or below the wallet's ChainLock.
#[test]
fn the_load_check_keeps_a_confirmed_row_and_a_chain_locked_one() {
    let fx = claimed();
    let w = w_of(fx.a);
    let hash = block_with(vec![w.clone()]).block_hash();
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![w]);
    row_stored(&fx, true);
    header_chain(&fx.session, &[(TIP + 1, hash)]);
    assert!(!fx.check());
    assert!(stored_rows(&fx.session, fx.id).contains_key(&fx.a));
    chain_lock(&fx.engine, &fx.session, fx.id, TIP + 1, hash);
    let other: BlockHash = block_with(vec![foreign()]).block_hash();
    header_chain(&fx.session, &[(TIP + 1, other)]);
    assert!(!fx.check(), "a ChainLocked row is final");
    assert!(stored_rows(&fx.session, fx.id).contains_key(&fx.a));
}

/// C4: the persister the engine hands platform-wallet keeps block spends.
#[test]
fn the_wallet_store_attests_block_spends() {
    use platform_wallet::changeset::{PersistenceCapabilities, PlatformWalletPersistence};
    let (_dir, _engine, session, _id) = engine_wallet();
    let live = session.live().unwrap();
    assert!(
        (live.store.persistence_capabilities()).contains(PersistenceCapabilities::CORE_BLOCK_SPENDS)
    );
}

/// B1(d): after the ChainLock prunes key-wallet's note of W's spend, the
/// abandon's own release skips A for its row alone, and owes no rescan
/// for it.
#[test]
fn a_release_after_the_chain_lock_prune_skips_a_row_spent_coin() {
    let fx = claimed();
    let w = w_of(fx.a);
    let hash = block_with(vec![w.clone()]).block_hash();
    mined(&fx.engine, &fx.session, fx.id, TIP + 1, vec![w]);
    row_stored(&fx, true);
    chain_lock(&fx.engine, &fx.session, fx.id, TIP + 1, hash);
    let noted = {
        let manager = fx.session.manager().unwrap();
        let id = fx.id;
        fx.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = wm.get_wallet_info(&id.0).unwrap();
            info.core_wallet.observed_spent_outpoints().clone()
        })
    };
    assert!(!noted.contains_key(&fx.a), "the ChainLock pruned the note");
    abandon(&fx.engine, &fx.session, fx.id, fx.p.txid());
    assert_eq!(*fx.session.hub.last_release.lock().unwrap(), Some(vec![]));
    assert!(
        fx.session.rescan_progress().unwrap().is_none(),
        "no rescan is owed for A"
    );
    assert!(!fx.held());
}
