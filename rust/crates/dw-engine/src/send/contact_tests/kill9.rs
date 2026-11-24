//! H-03 rows R5a and R5b: a contact payment to Bob killed in a child
//! process.
//!
//! R5a: after its gate passed and his address was reserved, before anything
//! marks it sent; after the relaunch the contact is not locked, no lock row
//! is left, and a payment to him prepares and goes out.
//!
//! R5b (#170 case 1): once its lock row is stored, at three points: before
//! its sent mark (`recorded`), after it and before the library is entered
//! (`marked`), and handed to the library with its outcome awaited
//! (`handed`).

use super::*;
use crate::kill9;

const ROW: &str = "contact_payment";
const CHILD: &str = "send::contact_tests::kill9::payment_child";

/// [`fixture`]'s wallet on `root`, made and stored on the first launch (our
/// identity, Bob's paying account and contact, his verified record), opened
/// as stored on the relaunch; one confirmed coin in memory either way.
fn fixture_at(dir: tempfile::TempDir, root: &std::path::Path, first: bool) -> Fixture {
    let (engine, session) = kill9::engine_at(root, PASS, true);
    let wallet = if first {
        engine
            .block_on(session.adopt_signing_window_default())
            .unwrap();
        engine
            .block_on(session.import_wallet(
                Zeroizing::new(PHRASE.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap()
    } else {
        let wallets = session.wallet_infos().unwrap();
        assert_eq!(wallets.len(), 1);
        wallets[0].wallet_id
    };
    let f = Fixture {
        _dir: Some(dir),
        engine,
        session,
        wallet,
        seeded: std::sync::atomic::AtomicBool::new(false),
    };
    if first {
        f.give_me();
        f.establish(BOB, false);
        f.store(f.bobs_account(contact_xpub(BOB)));
        f.store_bobs_contact(request(BOB, ME, 2_000_000), Some(0));
        f.verify_contacts();
    }
    f.credit(1, COIN);
    f
}

/// Run by [`kill9_r5a_payment_killed_after_its_reservation_leaves_bob_payable`]
/// in a child process: a payment to Bob held once his address is reserved.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn payment_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    assert_eq!(child.point, "reserved");
    let f = fixture_at(kill9::scratch(&child), &child.root, true);
    let d = f.draft(BOB);
    let pause = Arc::new(crate::coins::TestPause::default());
    (f.session.platform.money_gate).pause_at(Point::Reserved, Arc::clone(&pause));
    let _prepare = f.spawn_prepare(&d);
    kill9::eventually("the payment's address is reserved", || {
        pause.reached.load(Ordering::SeqCst)
    });
    kill9::reached(&child, "{}");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r5a_payment_killed_after_its_reservation_leaves_bob_payable() {
    let (dir, _) = kill9::killed_at(CHILD, ROW, "reserved");
    let root = dir.path().to_path_buf();
    let f = fixture_at(dir, &root, false);
    assert!(f.lock(BOB).is_none(), "nothing went out: Bob is not locked");
    assert!(f.rows().is_empty(), "no lock row");

    let d = f.draft(BOB);
    Seam::install(&d, Net::Accept);
    let p = f.prepare(&d).unwrap();
    let address = paid(&p);
    f.engine.block_on(d.broadcast(p)).unwrap();

    let fresh = [f.contact_address(BOB, 0), f.contact_address(BOB, 1)];
    assert!(
        fresh.iter().any(|a| a.to_string() == address),
        "one of Bob's first two addresses: {address}"
    );
    assert!(f.lock(BOB).is_none());
    assert!(f.rows().is_empty());
}

const SENT_ROW: &str = "contact_payment_sent";
const SENT_CHILD: &str = "send::contact_tests::kill9::sent_payment_child";

/// The points a payment passes, in order, each named for its seam.
const POINTS: [&str; 3] = ["recorded", "marked", "handed"];

/// The child's broadcaster: it notes each seam the payment passes and
/// parks it at the child's point.
struct Park {
    point: String,
    passed: std::sync::Mutex<Vec<&'static str>>,
}

impl Park {
    fn pass(&self, point: &'static str) -> bool {
        self.passed.lock().unwrap().push(point);
        self.point == point
    }

    fn park_at(&self, point: &'static str) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        if self.pass(point) {
            Box::pin(std::future::pending())
        } else {
            Box::pin(async {})
        }
    }

    fn reached(&self) -> bool {
        self.passed.lock().unwrap().last() == Some(&self.point.as_str())
    }
}

impl TestBroadcaster for Park {
    fn after_record(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        self.park_at("recorded")
    }

    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        self.park_at("marked")
    }

    fn broadcast(
        &self,
        _tx: &Transaction,
        _first: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Txid, PlatformWalletError>> + Send>> {
        assert!(self.pass("handed"), "handed off before {}", self.point);
        Box::pin(std::future::pending())
    }
}

/// Whether `wallet`'s sent mark for `txid` is stored.
fn marked(f: &Fixture, txid: &str) -> bool {
    (f.session.live().unwrap().appdb)
        .settings_with_prefix(&f.wallet.to_string(), crate::tx_actions::SENT_PREFIX)
        .unwrap()
        .iter()
        .any(|(k, _)| k.ends_with(txid))
}

/// Run by [`sent_payment`] in a child process: the fixture's coin stored, a
/// payment to Bob under way to the child's point; then it reports.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn sent_payment_child() {
    let Some(child) = kill9::child(SENT_ROW) else {
        return;
    };
    let f = super::fixture_at(&child.root, true);
    let utxos = f.with_wallet(|w| async move {
        let state = w.state().await;
        let account = state.core_wallet.accounts.standard_bip44_accounts.get(&0);
        (account.unwrap().utxos.values().cloned()).collect::<Vec<_>>()
    });
    f.store(PlatformWalletChangeSet {
        core: Some(platform_wallet::changeset::CoreChangeSet {
            new_utxos: utxos,
            ..Default::default()
        }),
        ..Default::default()
    });
    let d = f.draft(BOB);
    let at = POINTS.iter().position(|p| *p == child.point).unwrap();
    let park = Arc::new(Park {
        point: child.point.clone(),
        passed: Default::default(),
    });
    *d.broadcaster.lock().unwrap() = Some(park.clone());
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let txid = p.summary().txid.clone();
    let inputs: Vec<String> = (p.summary().inputs.iter())
        .map(|i| i.outpoint.to_string())
        .collect();
    let sending = f.session.rt.spawn({
        let (d, p) = (Arc::clone(&d), Arc::clone(&p));
        async move { d.broadcast(p).await.map(drop) }
    });
    kill9::eventually("the payment reached its point", || {
        assert!(!sending.is_finished(), "the payment ended before its point");
        park.reached()
    });
    assert_eq!(*park.passed.lock().unwrap(), POINTS[..=at]);
    assert_eq!(f.rows(), vec![(b58(BOB), txid.clone())]);
    assert_eq!(
        marked(&f, &txid),
        at > 0,
        "the sent mark at {}",
        child.point
    );
    kill9::reached(
        &child,
        &format!("{}\n{txid}\n{}\n", f.wallet, inputs.join(",")),
    );
}

/// A contact payment to Bob killed at `point` in a child process; the next
/// launch, with the payment's txid and the coins it signed.
fn sent_payment(point: &str) -> (Fixture, Txid, std::collections::BTreeSet<OutPoint>) {
    use std::str::FromStr;
    let (dir, report) = kill9::killed_at(SENT_CHILD, SENT_ROW, point);
    let lines: Vec<&str> = report.lines().collect();
    let wallet: WalletId = lines[0].parse().unwrap();
    let txid: Txid = lines[1].parse().unwrap();
    let inputs = (lines[2].split(','))
        .map(|o| OutPoint::from_str(o).unwrap())
        .collect();
    let (engine, session) = kill9::engine_at(dir.path(), PASS, true);
    // The child's fixture recorded the contacts as verified (stored, so
    // kept across the kill).
    let f = Fixture {
        _dir: Some(dir),
        engine,
        session,
        wallet,
        seeded: std::sync::atomic::AtomicBool::new(true),
    };
    (f, txid, inputs)
}

/// The wallet's coins, those reserved included.
fn coins(f: &Fixture) -> Vec<crate::CoinInfo> {
    let filter = crate::CoinFilter {
        include_locked: true,
        ..Default::default()
    };
    (f.engine)
        .block_on(f.session.utxos(f.wallet, filter))
        .unwrap()
}

/// The outpoints of [`coins`].
fn listed(f: &Fixture) -> std::collections::BTreeSet<OutPoint> {
    coins(f).into_iter().map(|c| c.outpoint).collect()
}

impl Fixture {
    /// The scan processes the block at `height` holding `tx`, and commits.
    fn mine(&self, height: u32, tx: Transaction) -> dashcore::BlockHash {
        let mut block = dashcore::Block {
            header: dashcore::blockdata::block::Header {
                version: dashcore::blockdata::block::Version::from_consensus(1),
                prev_blockhash: dashcore::BlockHash::all_zeros(),
                merkle_root: dashcore::TxMerkleNode::all_zeros(),
                time: 1_793_598_712,
                bits: dashcore::CompactTarget::from_consensus(0x207f_ffff),
                nonce: 0,
            },
            txdata: vec![tx],
        };
        block.header.merkle_root = block.compute_merkle_root().unwrap();
        let (hash, id) = (block.block_hash(), self.wallet);
        self.deliver(move |wm| {
            Box::pin(async move {
                use platform_wallet::key_wallet_manager::WalletInterface;
                let ids = std::collections::BTreeSet::from([id.0]);
                wm.process_block_for_wallets(&block, hash, height, &ids)
                    .await;
                wm.update_wallet_synced_height(&id.0, height);
            })
        });
        hash
    }

    fn chain_lock(&self, height: u32, hash: dashcore::BlockHash) {
        self.deliver(move |wm| {
            Box::pin(async move {
                use platform_wallet::key_wallet_manager::WalletInterface;
                wm.apply_chain_lock(dashcore::ChainLock {
                    block_height: height,
                    block_hash: hash,
                    signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
                });
            })
        });
    }
}

/// R5b at `recorded`: Bob's lock row stored, no sent mark, nothing handed
/// off. The next launch offers the coin and counts it, lists no payment,
/// holds no mark or reserved address, and keeps Bob locked behind the
/// payment, its outcome unknown. The user's own payment of the coin, once
/// ChainLocked, proves it unsent: Check again unlocks him.
#[test]
fn kill9_r5b_contact_payment_killed_before_its_sent_mark_frees_its_coin() {
    let (f, txid, inputs) = sent_payment("recorded");
    let txid_s = txid.to_string();
    let coin = *inputs.iter().next().unwrap();
    assert!(
        coins(&f).iter().any(|c| c.outpoint == coin && c.spendable),
        "coin control offers the coin"
    );
    let total = f.session.balances(&f.wallet).unwrap().unwrap().total;
    assert_eq!(total, COIN, "the coin counts");
    assert!(
        f.session.hub.history.get(&f.wallet, &txid).is_none(),
        "a payment never handed off is listed"
    );
    assert!(!marked(&f, &txid_s), "no sent mark");
    assert!(f.reserved(BOB).is_none(), "no address reserved");
    assert_eq!(f.lock(BOB).map(|l| l.txid), Some(txid_s.clone()));
    assert_eq!(f.rows(), vec![(b58(BOB), txid_s)], "the lock's row");
    // #485: Bob stays locked though nothing went out; this pins today's
    // behaviour until that issue's fix lands.
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);

    let self_send = own_payment_of(&f, coin);
    assert_eq!(
        f.resolve(BOB),
        LockResolution::Unknown,
        "a self-send unmined"
    );
    let hash = f.mine(10, self_send);
    assert_eq!(
        f.resolve(BOB),
        LockResolution::Unknown,
        "a self-send in a block not ChainLocked"
    );
    assert!(f.lock(BOB).is_some());
    f.chain_lock(10, hash);
    assert_eq!(f.resolve(BOB), LockResolution::NotSent, "Check again");
    assert!(f.lock(BOB).is_none(), "Bob is unlocked");
    assert!(f.rows().is_empty(), "the lock's row went");
}

/// The user's payment of `coin` to this wallet, handed off.
fn own_payment_of(f: &Fixture, coin: OutPoint) -> Transaction {
    let own = (f.engine)
        .block_on(f.session.current_receive_address(f.wallet))
        .unwrap()
        .address;
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    d.set_recipients(vec![Recipient {
        address: own,
        amount: COIN,
        subtract_fee_from_amount: true,
        label: None,
        message: None,
    }])
    .unwrap();
    d.set_source(CoinSource::Outpoints(vec![coin])).unwrap();
    Seam::install(&d, Net::Accept);
    let grant = (f.session.vault())
        .authorize(
            GrantPurpose::Spend { max_duffs: COIN },
            Some(&f.wallet.0),
            Credential::Passphrase(PASS),
        )
        .unwrap()
        .id;
    let p = f.engine.block_on(d.prepare(grant)).unwrap();
    let tx = p.transaction().unwrap();
    f.engine.block_on(d.broadcast(p)).unwrap();
    let seen = tx.clone();
    f.deliver(move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.process_mempool_transaction(&seen, None).await;
        })
    });
    let entry = f.session.hub.history.get(&f.wallet, &tx.txid());
    assert!(
        matches!(
            entry.map(|e| e.context),
            Some(key_wallet::transaction_checking::TransactionContext::Mempool)
        ),
        "the wallet sees its own payment in the mempool"
    );
    tx
}

/// R5b at `handed` (#170 case 1): a contact payment to Bob handed off
/// (its outcome awaited, Bob locked behind it), the app killed before the
/// wallet stored it. The next launch keeps its coin out of coin control
/// and the balance, lists the payment, and keeps Bob locked by it; an
/// Abandon keeps him locked, as it may have left. The user's own payment
/// of the freed coin to this wallet, once ChainLocked, proves it unsent:
/// Check again unlocks him.
#[test]
fn kill9_r5b_contact_payment_killed_after_the_hand_off_keeps_its_coin_and_its_lock() {
    let (f, parsed, inputs) = sent_payment("handed");
    let txid = parsed.to_string();
    assert!(marked(&f, &txid), "the sent mark is stored");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !listed(&f).is_disjoint(&inputs) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(listed(&f).is_disjoint(&inputs), "its coin is offered again");
    let total = f.session.balances(&f.wallet).unwrap().unwrap().total;
    assert!(total < COIN, "its coin counts: {total}");
    assert!(
        f.session.hub.history.get(&f.wallet, &parsed).is_some(),
        "not listed"
    );
    assert_eq!(
        f.lock(BOB).map(|l| l.txid),
        Some(txid.clone()),
        "Bob is free"
    );

    f.abandon(parsed);
    assert_eq!(
        f.lock(BOB).map(|l| l.txid),
        Some(txid.clone()),
        "the abandon unlocks Bob"
    );
    assert_eq!(f.rows(), vec![(b58(BOB), txid.clone())], "the lock's row");
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);

    let f = f.reopen();
    assert_eq!(f.lock(BOB).map(|l| l.txid), Some(txid.clone()), "restarted");
    let coin = *inputs.iter().next().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !coins(&f).iter().any(|c| c.outpoint == coin && c.spendable)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(
        coins(&f).iter().any(|c| c.outpoint == coin && c.spendable),
        "coin control offers the freed coin"
    );

    let self_send = own_payment_of(&f, coin);

    let hash = f.mine(10, self_send);
    assert_eq!(
        f.resolve(BOB),
        LockResolution::Unknown,
        "a self-send in a block not ChainLocked"
    );
    assert!(f.lock(BOB).is_some());
    f.chain_lock(10, hash);
    assert_eq!(f.resolve(BOB), LockResolution::NotSent, "Check again");
    assert!(f.lock(BOB).is_none(), "Bob is unlocked");
    assert!(f.rows().is_empty(), "the lock's row went");
}

/// R5b at `marked`: killed before the library was entered: the next launch cannot tell it
/// from a kill after the hand-off, so an Abandon keeps Bob locked too.
#[test]
fn kill9_r5b_contact_payment_killed_before_the_hand_off_keeps_its_lock_after_an_abandon() {
    let (f, txid, _) = sent_payment("marked");
    let txid_s = txid.to_string();
    assert!(marked(&f, &txid_s), "the sent mark is stored");
    assert_eq!(f.lock(BOB).map(|l| l.txid), Some(txid_s.clone()));
    f.abandon(txid);
    assert_eq!(
        f.lock(BOB).map(|l| l.txid),
        Some(txid_s.clone()),
        "the abandon unlocks Bob"
    );
    assert_eq!(f.rows(), vec![(b58(BOB), txid_s)], "the lock's row");
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
}
