//! #268: a payment P of the wallet's coin A, unconfirmed, and W, a foreign
//! spend of A paying only someone else, mined and ChainLocked. After a
//! restart between P and W, key-wallet holds no record of P, so W's block
//! does not sweep it; it only notes W's spend of A, and the ChainLock prunes
//! that note. Removing or abandoning P then releases A, which the store
//! keeps unspent: after a restart A is counted and listed in coin control.
//! Sends stay refused while the remove's rescan is owed, and that rescan,
//! once it passes W's block again, spends A for good. Reset chain data and
//! a restore from the recovery phrase do the same; Remove unconfirmed again
//! does not. A compact-filter rescan fetches W's block for the wallet,
//! since BIP158 filters hold the script of every output a block spends.
//! Nothing here changes the engine: the root fix waits on #268.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{Address, OutPoint, Transaction, Txid};

use super::own_spend_disk_tests::{
    Accept, COIN, COINS, FUNDED_AT, TIP, chain_lock, coins, eventually, foreign, fresh_address,
    funding, prepare, send, spendable, wait_stored,
};
use super::{balances, block_with, elsewhere, engine_wallet, mempool, mined, reopen};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{EngineError, WalletId};

/// Installs the SPV client Remove unconfirmed, Abandon and a rescan need,
/// without its run loop.
fn start_spv(fx: &Fx) {
    let s = Arc::clone(&fx.session);
    (fx.engine)
        .block_on(fx.session.on_runtime(async move {
            let config = s.spv_config()?;
            let dir = std::path::Path::new(crate::session::SPV_DIR);
            crate::fsutil::create_owned_dir(s.data_dir(), dir)?;
            s.manager()?.spv().start(config).await?;
            Ok(())
        }))
        .unwrap();
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

/// Whether the wallet store holds `txid`'s record.
fn records(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    persister
        .get_core_tx_record(id.0, &txid)
        .expect("read the core transaction record")
        .is_some()
}

#[derive(Clone, Copy, Debug)]
enum Removal {
    /// Tools ▸ Repair ▸ Remove unconfirmed transactions.
    Remove,
    Abandon,
}

struct Fx {
    _dir: tempfile::TempDir,
    engine: crate::Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    a: OutPoint,
    funded: Transaction,
    p: Transaction,
    w: Transaction,
}

impl Fx {
    fn reopen(mut self) -> Self {
        self.session = reopen(&self.engine, self.session);
        self
    }

    /// The blocks a rescan from the wallet's birth passes: the funding
    /// block, an empty one, and W's.
    fn replay(&self) {
        let (engine, session, id) = (&self.engine, &self.session, self.id);
        mined(engine, session, id, FUNDED_AT, vec![self.funded.clone()]);
        mined(engine, session, id, TIP, vec![foreign()]);
        mined(engine, session, id, TIP + 1, vec![self.w.clone()]);
    }

    fn held(&self) -> bool {
        coins(&self.engine, &self.session, self.id)
            .iter()
            .any(|(o, _)| *o == self.a)
    }

    fn listed(&self) -> bool {
        let filter = crate::CoinFilter {
            include_locked: true,
            ..Default::default()
        };
        (self.engine.block_on(self.session.utxos(self.id, filter)))
            .unwrap()
            .iter()
            .any(|c| c.outpoint == self.a)
    }

    fn total(&self) -> u64 {
        balances(&self.session, self.id).total
    }

    /// What preparing a payment from A alone gives.
    fn prepare_from_a(&self) -> Result<(), EngineError> {
        let (_draft, prepared) = prepare(
            &self.engine,
            &self.session,
            self.id,
            CoinSource::Outpoints(vec![self.a]),
            COIN / 2,
            false,
            &Arc::new(Accept::default()),
        );
        prepared.map(drop)
    }

    /// A is held, stored unspent, listed in coin control and counted.
    fn assert_back(&self, state: &str) {
        assert!(self.held(), "{state}: A is held");
        assert!(
            stored_coins(&self.session, self.id).contains(&self.a),
            "{state}: A is stored unspent"
        );
        assert!(self.listed(), "{state}: coin control lists A");
        assert_eq!(self.total(), COINS as u64 * COIN, "{state}: A is counted");
        assert!(!records(&self.session, self.id, self.w.txid()));
    }

    /// A is spent by W, whose record the store holds: A is not held, not
    /// stored, not listed, not counted, and neither a payment from A alone
    /// nor automatic selection for more than the other coins hold can use
    /// it.
    fn assert_gone(&self, state: &str) {
        assert!(!self.held(), "{state}: A is held");
        wait_stored(&self.session, self.id, &[self.w.txid()]);
        eventually(&format!("{state}: A leaves the store"), || {
            !stored_coins(&self.session, self.id).contains(&self.a)
        });
        assert!(!self.listed(), "{state}: coin control lists A");
        let rest = (COINS as u64 - 1) * COIN;
        assert_eq!(self.total(), rest, "{state}: A is counted");
        assert!(
            !spendable(&self.engine, &self.session, self.id, self.a),
            "{state}: A is offered"
        );
        let (_draft, prepared) = prepare(
            &self.engine,
            &self.session,
            self.id,
            CoinSource::Any,
            rest + COIN / 2,
            false,
            &Arc::new(Accept::default()),
        );
        use crate::send::SendFailure;
        assert!(
            matches!(
                prepared,
                Err(EngineError::Send(
                    SendFailure::AmountExceedsBalance { .. }
                        | SendFailure::AmountWithFeeExceedsBalance { .. }
                ))
            ),
            "{state}: automatic selection funds a payment only A covers: {:?}",
            prepared.map(|p| p.summary().inputs.clone())
        );
    }
}

/// The wallet holds [`COINS`] coins of one DASH, A among them; it sends P
/// from A, unconfirmed, and Dash Wallet restarts. W, a foreign spend of A
/// paying only someone else, is mined and ChainLocked; with
/// `restart_after_lock` Dash Wallet restarts again. The user then removes
/// P `how`, and Dash Wallet restarts before the remove's rescan passes W's
/// block again. Returns that session.
fn brought_back(restart_after_lock: bool, how: Removal) -> Fx {
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
    let w = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &to[0],
        super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
    );
    let fx = Fx {
        _dir: dir,
        engine,
        session,
        id,
        a,
        funded,
        p,
        w,
    }
    .reopen();
    mined(&fx.engine, &fx.session, id, TIP + 1, vec![fx.w.clone()]);
    let hash = block_with(vec![fx.w.clone()]).block_hash();
    chain_lock(&fx.engine, &fx.session, id, TIP + 1, hash);
    assert!(!fx.held(), "W's block spends A");
    assert!(
        (fx.session.hub.history.get(&id, &fx.p.txid())).is_some_and(|e| !e.abandoned),
        "P stays: key-wallet held no record of it to sweep"
    );
    let fx = if restart_after_lock { fx.reopen() } else { fx };
    start_spv(&fx);
    match how {
        Removal::Remove => assert_eq!(
            (fx.engine)
                .block_on(fx.session.drop_unconfirmed(Some(id)))
                .unwrap(),
            1
        ),
        Removal::Abandon => (fx.engine)
            .block_on(fx.session.abandon_transaction(id, fx.p.txid()))
            .unwrap(),
    }
    assert!(!fx.held(), "{how:?}: A stays out until the restart");
    fx.reopen()
}

/// Every order of the story: the ChainLock in the remove's session or
/// before a restart, removed by Remove unconfirmed or Abandon.
fn each_story(check: impl Fn(Fx, &str)) {
    for restart_after_lock in [false, true] {
        for how in [Removal::Remove, Removal::Abandon] {
            let state = format!("{how:?}, restart after the lock: {restart_after_lock}");
            check(brought_back(restart_after_lock, how), &state);
        }
    }
}

/// The known gap of #268: after the restart A is back, counted and listed
/// in coin control. No payment can be prepared from it while the remove's
/// rescan is owed (`replay_guard`), so it is never sent yet.
#[test]
fn known_gap_268_a_removed_payment_brings_back_a_coin_a_chain_locked_block_spent() {
    each_story(|fx, state| {
        fx.assert_back(state);
        assert!(
            matches!(fx.prepare_from_a(), Err(EngineError::RescanInProgress)),
            "{state}: the remove's rescan is owed"
        );
        assert!(
            fx.session.rescan_progress().unwrap().is_some(),
            "{state}: the remove's rescan resumes after the restart"
        );
    });
}

/// The way out that needs nothing of the user: the remove's own rescan,
/// resumed after the restart, passes W's block again; key-wallet then
/// records W's spend of A, which the store keeps, after another restart
/// too. The user's Rescan from birth is refused while it runs.
#[test]
fn the_removes_rescan_passing_the_spends_block_spends_the_coin_brought_back() {
    each_story(|fx, state| {
        start_spv(&fx);
        assert!(
            matches!(
                (fx.engine).block_on(fx.session.rescan(crate::RescanFrom::WalletBirth)),
                Err(EngineError::RescanInProgress)
            ),
            "{state}: Rescan from birth is refused"
        );
        fx.replay();
        fx.assert_gone(&format!("{state}, the rescan done"));
        let fx = fx.reopen();
        fx.assert_gone(&format!("{state}, the rescan done, a restart"));
    });
}

/// Cancelling the remove's rescan keeps sends refused as interrupted, and
/// A counted. Rescan from birth then goes through: once it passes W's
/// block, A is spent, after another restart too.
#[test]
fn rescan_from_birth_after_cancelling_the_removes_rescan_spends_the_coin_brought_back() {
    each_story(|fx, state| {
        start_spv(&fx);
        assert!(fx.engine.block_on(fx.session.cancel_rescan()).unwrap());
        fx.assert_back(&format!("{state}, cancelled"));
        assert!(
            matches!(fx.prepare_from_a(), Err(EngineError::RescanInterrupted)),
            "{state}, cancelled: {:?}",
            fx.prepare_from_a()
        );
        (fx.engine)
            .block_on(fx.session.rescan(crate::RescanFrom::WalletBirth))
            .unwrap();
        fx.replay();
        fx.assert_gone(&format!("{state}, rescanned from birth"));
        let fx = fx.reopen();
        fx.assert_gone(&format!("{state}, rescanned from birth, a restart"));
    });
}

/// Remove unconfirmed again finds nothing to remove and leaves A back,
/// after another restart too.
#[test]
fn known_gap_268_remove_unconfirmed_again_leaves_the_coin_brought_back() {
    let fx = brought_back(false, Removal::Remove);
    start_spv(&fx);
    assert_eq!(
        (fx.engine)
            .block_on(fx.session.drop_unconfirmed(Some(fx.id)))
            .unwrap(),
        0
    );
    fx.assert_back("removed again");
    let fx = fx.reopen();
    fx.assert_back("removed again, a restart");
}

/// Reset chain data, then the scan from the wallet's birth it owes: A is
/// spent once W's block is passed again, after another restart too.
#[test]
fn reset_chain_data_spends_the_coin_brought_back() {
    let fx = brought_back(false, Removal::Remove);
    fx.engine.block_on(fx.session.reset_chain_data()).unwrap();
    fx.replay();
    fx.assert_gone("reset");
    let fx = fx.reopen();
    fx.assert_gone("reset, a restart");
}

/// Removing the wallet and restoring it from its recovery phrase: the
/// restore's scan passes W's block, and A is spent, after a restart too.
#[test]
fn restoring_the_wallet_spends_the_coin_brought_back() {
    let fx = brought_back(false, Removal::Remove);
    let wipe = (fx.session.vault())
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&fx.id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    (fx.engine)
        .block_on(fx.session.remove_wallet(fx.id, wipe))
        .unwrap();
    let mut fx = fx.reopen();
    fx.id = (fx.engine)
        .block_on(fx.session.import_wallet(
            zeroize::Zeroizing::new(super::PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(super::BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    fx.replay();
    fx.assert_gone("restored");
    let fx = fx.reopen();
    fx.assert_gone("restored, a restart");
}

/// Without a restart between P and W, key-wallet still holds P's record,
/// and W's block sweeps it at once: nothing is left to remove, and A stays
/// spent after a restart.
#[test]
fn a_payment_beaten_in_its_own_session_goes_with_the_block_and_its_coin_stays_spent() {
    let (_dir, engine, session, id) = engine_wallet();
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
    let w = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &to[0],
        super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
    );
    mined(&engine, &session, id, TIP + 1, vec![w.clone()]);
    let hash = block_with(vec![w.clone()]).block_hash();
    chain_lock(&engine, &session, id, TIP + 1, hash);
    assert!(session.hub.history.get(&id, &p.txid()).is_none());
    let fx = Fx {
        _dir,
        engine,
        session,
        id,
        a,
        funded,
        p,
        w,
    };
    start_spv(&fx);
    assert_eq!(
        (fx.engine)
            .block_on(fx.session.drop_unconfirmed(Some(id)))
            .unwrap(),
        0
    );
    let fx = fx.reopen();
    assert!(!fx.held(), "A stays spent");
    assert!(!fx.listed());
    assert_eq!(fx.total(), (COINS as u64 - 1) * COIN);
}
