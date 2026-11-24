//! #268: a payment P of the wallet's coin A, unconfirmed, and W, a foreign
//! spend of A paying only someone else, mined and ChainLocked. After a
//! restart between P and W, key-wallet holds no record of P, so W's block
//! does not sweep it; it only notes W's spend of A, and the ChainLock prunes
//! that note. The block's spend of A is stored as a row, so removing or
//! abandoning P leaves A spent, through restarts, Remove unconfirmed again,
//! Rescan from birth, Reset chain data and a restore from the recovery
//! phrase. A coin only the removed payment spent still comes back. Blocks
//! reach the wallet as dash-spv's filter scan hands them: only above its
//! checkpoint, and only when the block's BIP158 filter matches its scan
//! scripts. W's block matches through A's spent script alone.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use dashcore::bip158::{BlockFilter, FilterQuery};
use dashcore::hashes::Hash as _;
use dashcore::{Address, Block, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use platform_wallet::key_wallet_manager::WalletInterface;

use super::own_spend_disk_tests::{
    Accept, COIN, COINS, FUNDED_AT, TIP, chain_lock, coins, eventually, foreign, fresh_address,
    funding, prepare, send, spendable, wait_stored,
};
use super::{BIRTH, balances, block_with, elsewhere, engine_wallet, mempool, reopen};
use crate::send::{CoinSource, SendFailure};
use crate::session::NetworkSession;
use crate::{EngineError, WalletId};

/// The coinbase of the block at `height`, paying someone else.
fn coinbase(height: u32) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from(height.to_le_bytes().to_vec()),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 500 * COIN,
            script_pubkey: elsewhere(),
        }],
        special_transaction_payload: None,
    }
}

/// The block at `height`: its coinbase, then `txs`.
fn block_at(height: u32, txs: &[Transaction]) -> Block {
    let mut all = vec![coinbase(height)];
    all.extend_from_slice(txs);
    block_with(all)
}

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

/// Which of the three blocks (funding, empty, W's) a scan fetched.
const FUNDING_AND_W: [bool; 3] = [true, false, true];

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

    /// The chain: the funding block, a block of someone else's payment, and
    /// W's block.
    fn chain(&self) -> [(u32, Vec<Transaction>); 3] {
        [
            (FUNDED_AT, vec![self.funded.clone()]),
            (TIP, vec![foreign()]),
            (TIP + 1, vec![self.w.clone()]),
        ]
    }

    /// The BIP158 basic filter of `block`, with the scripts of the funding
    /// coins it spends (`spent_known`) or a foreign script in their place.
    fn filter(&self, block: &Block, spent_known: bool) -> BlockFilter {
        let prevouts: HashMap<OutPoint, ScriptBuf> = (self.funded.output.iter().enumerate())
            .map(|(v, o)| {
                (
                    OutPoint::new(self.funded.txid(), v as u32),
                    o.script_pubkey.clone(),
                )
            })
            .filter(|_| spent_known)
            .collect();
        BlockFilter::new_script_filter(block, |o| {
            Ok::<_, dashcore::bip158::Error>(prevouts.get(o).cloned().unwrap_or_else(elsewhere))
        })
        .unwrap()
    }

    /// Whether the loaded wallet's scan scripts match `filter` of `block`.
    fn matches(&self, block: &Block, filter: &BlockFilter) -> bool {
        let manager = self.session.manager().unwrap();
        let id = self.id;
        let (scripts, elements) = self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            (
                wm.scan_script_pubkeys_for(&id.0),
                wm.monitored_filter_elements_for(&id.0),
            )
        });
        let mut query: FilterQuery = scripts.iter().map(|s| s.as_bytes()).collect();
        for element in &elements {
            query.push(element);
        }
        filter.match_any(&block.block_hash(), &query).unwrap()
    }

    /// key-wallet's filter checkpoint for the wallet.
    fn synced(&self) -> u32 {
        super::own_spend_disk_tests::synced(&self.engine, &self.session, self.id)
    }

    /// The outpoints key-wallet noted spent in a block, with its height.
    fn noted_spends(&self) -> std::collections::BTreeMap<OutPoint, u32> {
        let manager = self.session.manager().unwrap();
        let id = self.id;
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = wm.get_wallet_info(&id.0).unwrap();
            info.core_wallet.observed_spent_outpoints().clone()
        })
    }

    /// The filter scan passes the block at `height` holding `txs`, as
    /// dash-spv does: a block at or below the checkpoint is skipped; above
    /// it, the block is fetched and processed only when its filter matches
    /// the wallet's scan scripts, and the checkpoint moves to `height`
    /// either way. Returns whether it was fetched.
    fn scan(&self, height: u32, txs: &[Transaction]) -> bool {
        if height <= self.synced() {
            return false;
        }
        let block = block_at(height, txs);
        let fetched = self.matches(&block, &self.filter(&block, true));
        let id = self.id;
        super::deliver(&self.engine, &self.session, id, move |wm| {
            Box::pin(async move {
                if fetched {
                    wm.process_block_for_wallets(
                        &block,
                        block.block_hash(),
                        height,
                        &BTreeSet::from([id.0]),
                    )
                    .await;
                }
                wm.update_wallet_synced_height(&id.0, height);
            })
        });
        fetched
    }

    /// The filter scan over [`Self::chain`]; which blocks it fetched.
    fn replay(&self) -> [bool; 3] {
        self.chain().map(|(height, txs)| self.scan(height, &txs))
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

    /// Prepares a payment of `amount` from `source`; its inputs.
    fn prepare(
        &self,
        source: CoinSource,
        amount: u64,
        subtract_fee: bool,
    ) -> Result<BTreeSet<OutPoint>, EngineError> {
        let (_draft, prepared) = prepare(
            &self.engine,
            &self.session,
            self.id,
            source,
            amount,
            subtract_fee,
            &Arc::new(Accept::default()),
        );
        prepared.map(|p| (p.summary().inputs.iter()).map(|i| i.outpoint).collect())
    }

    fn prepare_from_a(&self) -> Result<BTreeSet<OutPoint>, EngineError> {
        self.prepare(CoinSource::Outpoints(vec![self.a]), COIN / 2, false)
    }

    /// The funding coins other than A.
    fn others(&self) -> BTreeSet<OutPoint> {
        (1..COINS as u32)
            .map(|v| OutPoint::new(self.funded.txid(), v))
            .collect()
    }

    /// A is held, stored unspent, listed in coin control and counted, and
    /// the scan has not passed W's block since the remove.
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
    /// stored, not listed and not counted. A payment from A alone is refused
    /// as unavailable; automatic selection sees exactly the other coins, and
    /// a payment of all of them uses each of them and never A.
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
        let from_a = self.prepare_from_a();
        assert!(
            matches!(
                from_a,
                Err(EngineError::Send(SendFailure::OutpointUnavailable(o))) if o == self.a
            ),
            "{state}: a payment from A alone: {from_a:?}"
        );
        let over = self.prepare(CoinSource::Any, rest + COIN / 2, false);
        assert!(
            matches!(
                over,
                Err(EngineError::Send(
                    SendFailure::AmountExceedsBalance { available }
                        | SendFailure::AmountWithFeeExceedsBalance { available, .. }
                )) if available == rest
            ),
            "{state}: a payment above the other coins: {over:?}"
        );
        let all = self.prepare(CoinSource::Any, rest, true);
        assert_eq!(
            all.as_ref().ok(),
            Some(&self.others()),
            "{state}: a payment of everything: {all:?}"
        );
    }

    /// A stays spent by W although the store holds no record of W: it is
    /// not held, not stored unspent, not listed, not counted and not
    /// offered, and no payment can be prepared from it.
    fn assert_still_spent(&self, state: &str) {
        assert!(!self.held(), "{state}: A is held");
        assert!(
            !stored_coins(&self.session, self.id).contains(&self.a),
            "{state}: A is stored unspent"
        );
        assert!(!self.listed(), "{state}: coin control lists A");
        assert_eq!(
            self.total(),
            (COINS as u64 - 1) * COIN,
            "{state}: A is counted"
        );
        assert!(
            !spendable(&self.engine, &self.session, self.id, self.a),
            "{state}: A is offered"
        );
        let from_a = self.prepare_from_a();
        assert!(
            from_a.is_err(),
            "{state}: a payment from A alone: {from_a:?}"
        );
    }
}

/// The wallet holds [`COINS`] coins of one DASH, A among them; it sends P
/// from A, unconfirmed, and Dash Wallet restarts. W, a foreign spend paying
/// only someone else, of A when `w_spends_a` and of a coin no wallet holds
/// otherwise, is mined and ChainLocked; with `restart_after_lock` Dash
/// Wallet restarts again. The user then removes P `how`, and Dash Wallet
/// restarts before the remove's rescan passes W's block again. Returns that
/// session.
fn removed(restart_after_lock: bool, how: Removal, w_spends_a: bool) -> Fx {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    let a = OutPoint::new(funded.txid(), 0);
    let w = match w_spends_a {
        true => super::signed_by_wallet(
            &engine,
            &session,
            id,
            &to[0],
            super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
        ),
        false => super::spend_of(
            OutPoint::new(Txid::from_byte_array([0x68; 32]), 0),
            vec![(elsewhere(), COIN - 1_000)],
        ),
    };
    let mut fx = Fx {
        _dir: dir,
        engine,
        session,
        id,
        a,
        funded,
        p: foreign(),
        w,
    };
    let [funding_block, other_block, w_block] = fx.chain();
    assert!(fx.scan(funding_block.0, &funding_block.1));
    assert!(!fx.scan(other_block.0, &other_block.1));
    fx.p = send(
        &fx.engine,
        &fx.session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&fx.engine, &fx.session, id, &fx.p);
    wait_stored(&fx.session, id, &[fx.p.txid()]);
    let fx = fx.reopen();

    let block = block_at(w_block.0, &w_block.1);
    assert!(
        !fx.matches(&block, &fx.filter(&block, false)),
        "W's block matches only through A's spent script"
    );
    assert_eq!(
        fx.scan(w_block.0, &w_block.1),
        w_spends_a,
        "the scan fetches W's block only when W spends A"
    );
    if w_spends_a {
        assert_eq!(
            fx.noted_spends().get(&a),
            Some(&(TIP + 1)),
            "key-wallet notes W's spend of A"
        );
    }
    assert!(!fx.held(), "W's block spends A, or P claims it");
    assert!(
        (fx.session.hub.history.get(&id, &fx.p.txid())).is_some_and(|e| !e.abandoned),
        "P stays: key-wallet held no record of it to sweep"
    );
    chain_lock(&fx.engine, &fx.session, id, TIP + 1, block.block_hash());
    assert_eq!(
        fx.noted_spends().get(&a),
        None,
        "the ChainLock prunes the note"
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

/// Every order of the story `build` tells: the ChainLock in the remove's
/// session or before a restart, removed by Remove unconfirmed or Abandon.
fn each_order(build: impl Fn(bool, Removal) -> Fx, check: impl Fn(Fx, &str)) {
    for restart_after_lock in [false, true] {
        for how in [Removal::Remove, Removal::Abandon] {
            let state = format!("{how:?}, restart after the lock: {restart_after_lock}");
            check(build(restart_after_lock, how), &state);
        }
    }
}

/// #268 fixed: the ChainLocked block's spend of A outlives the ChainLock's
/// prune of key-wallet's note, so removing P leaves A spent, in the
/// remove's session and after the restarts that follow.
#[test]
fn a_removed_payment_leaves_spent_a_coin_a_chain_locked_block_spent() {
    each_order(
        |restart_after_lock, how| removed(restart_after_lock, how, true),
        |fx, state| {
            fx.assert_still_spent(state);
            let fx = fx.reopen();
            fx.assert_still_spent(&format!("{state}, another restart"));
        },
    );
}

/// #268 fixed: Remove unconfirmed again finds nothing to remove, and A
/// stays spent, after another restart too.
#[test]
fn remove_unconfirmed_again_leaves_spent_a_coin_a_chain_locked_block_spent() {
    let fx = removed(false, Removal::Remove, true);
    start_spv(&fx);
    assert_eq!(
        (fx.engine)
            .block_on(fx.session.drop_unconfirmed(Some(fx.id)))
            .unwrap(),
        0
    );
    fx.assert_still_spent("removed again");
    let fx = fx.reopen();
    fx.assert_still_spent("removed again, a restart");
}

/// A coin whose only spender was the removed payment comes back: W spends
/// a coin no wallet holds, so its block is never fetched, and removing P
/// frees A for good, once the remove's rescan is done and after another
/// restart.
#[test]
fn a_coin_only_the_removed_payment_spent_comes_back() {
    each_order(
        |restart_after_lock, how| removed(restart_after_lock, how, false),
        |fx, state| {
            fx.assert_back(state);
            start_spv(&fx);
            assert_eq!(fx.replay(), [true, false, false], "{state}");
            fx.assert_back(&format!("{state}, the rescan done"));
            assert!(spendable(&fx.engine, &fx.session, fx.id, fx.a), "{state}");
            assert_eq!(
                fx.prepare_from_a().ok(),
                Some(BTreeSet::from([fx.a])),
                "{state}: a payment from A alone"
            );
            let fx = fx.reopen();
            fx.assert_back(&format!("{state}, the rescan done, a restart"));
            assert!(spendable(&fx.engine, &fx.session, fx.id, fx.a), "{state}");
        },
    );
}

/// The stored block-spend rows of `id`.
fn stored_rows(
    session: &NetworkSession,
    id: WalletId,
) -> std::collections::BTreeMap<OutPoint, platform_wallet::changeset::BlockSpend> {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let persister = Arc::clone(&session.live().unwrap().persister);
    persister.flush(id.0).unwrap();
    persister.block_spent_outpoints(&id.0).unwrap()
}

/// Rescan from birth after the removal: it keeps W's row, its scan fetches
/// W's block again, and A stays spent, after another restart too.
#[test]
fn rescan_from_birth_after_a_removal_keeps_the_coin_spent() {
    each_order(
        |restart_after_lock, how| removed(restart_after_lock, how, true),
        |fx, state| {
            start_spv(&fx);
            (fx.engine)
                .block_on(fx.session.rescan(crate::RescanFrom::WalletBirth))
                .unwrap();
            assert_eq!(fx.synced(), BIRTH - 1, "{state}: the rescan's checkpoint");
            assert!(
                stored_rows(&fx.session, fx.id).contains_key(&fx.a),
                "{state}: the rescan keeps W's row"
            );
            assert_eq!(fx.replay(), FUNDING_AND_W, "{state}");
            fx.assert_still_spent(&format!("{state}, rescanned from birth"));
            let fx = fx.reopen();
            fx.assert_still_spent(&format!("{state}, rescanned from birth, a restart"));
        },
    );
}

/// Reset chain data after the removal: once the scan it owes fetches W's
/// block again, A is spent, after another restart too.
#[test]
fn reset_chain_data_after_a_removal_keeps_the_coin_spent() {
    let fx = removed(false, Removal::Remove, true);
    fx.engine.block_on(fx.session.reset_chain_data()).unwrap();
    assert_eq!(fx.synced(), BIRTH - 1, "Reset rewinds below the birth");
    assert!(
        matches!(fx.prepare_from_a(), Err(EngineError::RescanInProgress)),
        "the reset's scan is owed"
    );
    assert_eq!(fx.replay(), FUNDING_AND_W);
    fx.assert_still_spent("reset");
    let fx = fx.reopen();
    fx.assert_still_spent("reset, a restart");
}

/// Removing the wallet and restoring it from its recovery phrase: the
/// restore's scan fetches W's block, and A is spent, after a restart too.
#[test]
fn restoring_the_wallet_after_a_removal_spends_the_coin() {
    let fx = removed(false, Removal::Remove, true);
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
                birth_height: Some(BIRTH),
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(fx.synced(), BIRTH - 1);
    assert_eq!(fx.replay(), FUNDING_AND_W);
    fx.assert_gone("restored");
    let fx = fx.reopen();
    fx.assert_gone("restored, a restart");
}

/// Without a restart between P and W, key-wallet still holds P's record,
/// and W's block sweeps it at once: nothing is left to remove, and A stays
/// spent after a restart.
#[test]
fn a_payment_beaten_in_its_own_session_goes_with_the_block_and_its_coin_stays_spent() {
    let (dir, engine, session, id) = engine_wallet();
    let to: Vec<Address> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let funded = funding(7, &to, COIN);
    let a = OutPoint::new(funded.txid(), 0);
    let w = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &to[0],
        super::spend_of(a, vec![(elsewhere(), COIN - 1_000)]),
    );
    let mut fx = Fx {
        _dir: dir,
        engine,
        session,
        id,
        a,
        funded,
        p: foreign(),
        w,
    };
    let [funding_block, other_block, w_block] = fx.chain();
    assert!(fx.scan(funding_block.0, &funding_block.1));
    assert!(!fx.scan(other_block.0, &other_block.1));
    fx.p = send(
        &fx.engine,
        &fx.session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&fx.engine, &fx.session, id, &fx.p);
    wait_stored(&fx.session, id, &[fx.p.txid()]);
    assert!(fx.scan(w_block.0, &w_block.1));
    let hash = block_at(w_block.0, &w_block.1).block_hash();
    chain_lock(&fx.engine, &fx.session, id, TIP + 1, hash);
    assert!(fx.session.hub.history.get(&id, &fx.p.txid()).is_none());
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
