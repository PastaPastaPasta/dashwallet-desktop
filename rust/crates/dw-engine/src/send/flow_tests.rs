//! Offline send-flow tests: a real session (PlatformWalletManager +
//! SqlitePersister + vault) whose BIP44 account is credited with coins
//! directly in key-wallet state, so prepare can select, sign through the
//! vault and reserve without a network. Broadcasting needs SPV and is
//! covered by the regtest suite (`regtest/harness/tests/test_l1_send.py`).

use std::sync::Arc;
use std::time::Duration;

use dashcore::hashes::Hash;
use dashcore::{OutPoint, TxOut, Txid};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Utxo;
use zeroize::Zeroizing;

use super::psbt::{PsbtFailure, PsbtSignability};
use super::*;
use crate::{
    BookPurpose, CoinFilter, DashNetwork, Engine, EngineConfig, EventSink, ImportOptions,
    LabelsFailure,
};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
/// Someone else's regtest address (dashd `signmessagewithprivkey` vector).
const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";
const COIN: u64 = 100_000_000;

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

fn fixture(encrypted: bool) -> Fixture {
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    let pass: Option<&'static [u8]> = encrypted.then_some(b"pass phrase".as_slice());
    engine
        .block_on(session.vault_op(move |v| v.create(pass)))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    Fixture {
        _dir: dir,
        engine,
        session,
        wallet,
    }
}

impl Fixture {
    /// Credits a confirmed coin of `value` to a fresh receive address.
    fn credit(&self, seed: u8, value: u64) -> OutPoint {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine.block_on(async move {
            session
                .on_runtime(async move {
                    let wallet = inner.wallet(&id).await?;
                    let address = wallet.core().next_receive_address_for_account(0).await?;
                    let outpoint = OutPoint::new(Txid::from_byte_array([seed; 32]), 0);
                    let mut utxo = Utxo::new(
                        outpoint,
                        TxOut {
                            value,
                            script_pubkey: address.script_pubkey(),
                        },
                        address,
                        1,
                        false,
                    );
                    utxo.is_confirmed = true;
                    let mut state = wallet.state_mut().await;
                    state
                        .core_wallet
                        .accounts
                        .standard_bip44_accounts
                        .get_mut(&0)
                        .expect("BIP44 account 0")
                        .utxos
                        .insert(outpoint, utxo);
                    Ok(outpoint)
                })
                .await
                .unwrap()
        })
    }

    /// Removes a coin from key-wallet's unspent set, as when the wallet sees
    /// a transaction spending it (SPV feeds a broadcast back into the
    /// wallet's mempool view, or a block confirms it).
    fn see_spent(&self, outpoint: OutPoint) {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine
            .block_on(async move {
                session
                    .on_runtime(async move {
                        let wallet = inner.wallet(&id).await?;
                        let mut state = wallet.state_mut().await;
                        state
                            .core_wallet
                            .accounts
                            .standard_bip44_accounts
                            .get_mut(&0)
                            .expect("BIP44 account 0")
                            .utxos
                            .remove(&outpoint);
                        Ok(())
                    })
                    .await
            })
            .unwrap();
    }

    /// Tracks asset lock `seed` spending `inputs` with `status` (`None`
    /// untracks it). key-wallet's coins stay unspent, as for a lock the
    /// wallet never saw sent.
    fn track_lock(
        &self,
        seed: u8,
        inputs: &[OutPoint],
        status: Option<platform_wallet::AssetLockStatus>,
    ) -> Txid {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        let txid = Txid::from_byte_array([seed; 32]);
        let out_point = OutPoint::new(txid, 0);
        let input = inputs
            .iter()
            .map(|o| dashcore::TxIn {
                previous_output: *o,
                ..Default::default()
            })
            .collect();
        self.engine
            .block_on(async move {
                session
                    .on_runtime(async move {
                        let wallet = inner.wallet(&id).await?;
                        let mut state = wallet.state_mut().await;
                        let Some(status) = status else {
                            state.tracked_asset_locks.remove(&out_point);
                            return Ok(());
                        };
                        state.tracked_asset_locks.insert(
                            out_point,
                            platform_wallet::TrackedAssetLock {
                                out_point,
                                transaction: Transaction {
                                    version: 3,
                                    lock_time: 0,
                                    input,
                                    output: Vec::new(),
                                    special_transaction_payload: None,
                                },
                                account_index: 0,
                                funding_type:
                                    platform_wallet::AssetLockFundingType::IdentityRegistration,
                                identity_index: 0,
                                amount: COIN,
                                status,
                                proof: None,
                            },
                        );
                        Ok(())
                    })
                    .await
            })
            .unwrap();
        txid
    }

    /// A fresh receive address of the wallet.
    fn own_address(&self) -> String {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine
            .block_on(async move {
                session
                    .on_runtime(async move {
                        let w = inner.wallet(&id).await?;
                        Ok(w.core().next_receive_address_for_account(0).await?)
                    })
                    .await
            })
            .unwrap()
            .to_string()
    }

    fn spend_grant(&self, max_duffs: u64) -> String {
        self.session
            .vault()
            .authorize(
                GrantPurpose::Spend { max_duffs },
                Some(&self.wallet.0),
                Credential::None,
            )
            .unwrap()
            .id
    }

    fn draft(&self, recipients: Vec<Recipient>) -> Arc<TxDraft> {
        let d = self.session.new_tx_draft(self.wallet).unwrap();
        d.set_recipients(recipients).unwrap();
        d
    }

    fn reserved(&self) -> Vec<OutPoint> {
        let coins = self
            .engine
            .block_on(self.session.utxos(
                self.wallet,
                CoinFilter {
                    include_locked: true,
                    ..Default::default()
                },
            ))
            .unwrap();
        coins
            .into_iter()
            .filter(|c| c.reserved)
            .map(|c| c.outpoint)
            .collect()
    }
}

/// A second address the wallet does not own.
fn other_foreign() -> String {
    Address::new(
        dashcore::Network::Regtest,
        Payload::PubkeyHash(PubkeyHash::from_byte_array([5; 20])),
    )
    .to_string()
}

fn pay(address: &str, amount: u64) -> Recipient {
    Recipient {
        address: address.into(),
        amount,
        subtract_fee_from_amount: false,
        label: None,
        message: None,
    }
}

/// Room for the fee in a grant whose test is not about the cap: a few
/// inputs at the default rate cost well under this.
const FEE_ROOM: u64 = 10_000;

fn send_failure<T: std::fmt::Debug>(r: Result<T, EngineError>) -> SendFailure {
    match r {
        Err(EngineError::Send(f)) => f,
        other => panic!("expected a send failure, got {other:?}"),
    }
}

#[test]
fn prepare_signs_reserves_and_abandon_releases() {
    let f = fixture(false);
    let big = f.credit(1, COIN);
    let small = f.credit(2, COIN / 2);

    let draft = f.draft(vec![pay(FOREIGN, 30_000_000)]);
    let estimate = f.engine.block_on(draft.estimate()).unwrap();
    assert_eq!(estimate.input_count, 1);
    assert_eq!(estimate.total_sent, 30_000_000);
    assert_eq!(estimate.fee, 226);

    let prepared = f
        .engine
        .block_on(draft.prepare(f.spend_grant(30_000_000 + estimate.fee)))
        .unwrap();
    let s = prepared.summary().clone();
    assert_eq!(s.fee, estimate.fee);
    assert_eq!(s.total_sent, 30_000_000);
    assert_eq!(s.external_sent, 30_000_000);
    assert_eq!(s.total_debit, 30_000_000 + s.fee);
    assert_eq!(s.inputs.len(), 1);
    assert_eq!(s.inputs[0].outpoint, big);
    assert_eq!(s.outputs.len(), 2);
    let change = s.outputs.iter().find(|o| o.is_change).unwrap();
    assert!(change.is_mine);
    assert_eq!(change.amount, COIN - 30_000_000 - s.fee);
    // Signed: every input carries a scriptSig (signature + pubkey).
    let raw = prepared.raw().unwrap();
    let tx: Transaction = dashcore::consensus::deserialize(&raw).unwrap();
    assert!(tx.input.iter().all(|i| i.script_sig.len() > 100));
    assert_eq!(tx.txid().to_string(), s.txid);
    assert_eq!(f.reserved(), vec![big]);

    // A second payment cannot take the reserved coin.
    let second = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let p2 = f
        .engine
        .block_on(second.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    assert_eq!(p2.summary().inputs[0].outpoint, small);

    f.engine
        .block_on(draft.abandon(Arc::clone(&prepared)))
        .unwrap();
    f.engine
        .block_on(draft.abandon(Arc::clone(&prepared)))
        .unwrap();
    assert_eq!(f.reserved(), vec![small]);
    assert_eq!(
        send_failure(f.engine.block_on(draft.broadcast(Arc::clone(&prepared)))),
        SendFailure::PreparedTxSpent
    );
    // A prepared transaction belongs to the draft that made it.
    let r = f.engine.block_on(draft.abandon(Arc::clone(&p2)));
    assert!(matches!(r, Err(EngineError::InvalidArgument(_))), "{r:?}");
}

/// The grant caps what leaves the wallet, fee included (fix-review L4):
/// the recipients' amounts alone are not enough.
#[test]
fn grants_cap_what_leaves_the_wallet() {
    let f = fixture(false);
    f.credit(1, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 30_000_000)]);
    let fee = f.engine.block_on(draft.estimate()).unwrap().fee;
    let r = f.engine.block_on(draft.prepare(f.spend_grant(30_000_000)));
    assert_eq!(
        send_failure(r),
        SendFailure::GrantExceeded {
            max_duffs: 30_000_000,
            outflow: 30_000_000 + fee
        }
    );
    let r = f
        .engine
        .block_on(draft.prepare(f.spend_grant(30_000_000 + fee - 1)));
    assert!(matches!(send_failure(r), SendFailure::GrantExceeded { .. }));
    // The grant was single-use.
    let grant = f.spend_grant(30_000_000 + fee);
    f.engine.block_on(draft.prepare(grant.clone())).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.prepare(grant))),
        SendFailure::GrantInvalid
    );
    let r = f.engine.block_on(draft.prepare("no-such-grant".into()));
    assert_eq!(send_failure(r), SendFailure::GrantInvalid);
}

#[test]
fn foreign_change_counts_against_the_cap() {
    let f = fixture(false);
    f.credit(1, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 30_000_000)]);
    draft
        .set_change(ChangePolicy::Address(other_foreign()))
        .unwrap();
    let estimate = f.engine.block_on(draft.estimate()).unwrap();
    let change = estimate.change.unwrap();
    let r = f
        .engine
        .block_on(draft.prepare(f.spend_grant(30_000_000 + estimate.fee)));
    assert_eq!(
        send_failure(r),
        SendFailure::GrantExceeded {
            max_duffs: 30_000_000 + estimate.fee,
            outflow: COIN
        }
    );
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(30_000_000 + change + estimate.fee)))
        .unwrap();
    let out = p.summary().outputs.iter().find(|o| o.is_change).unwrap();
    assert!(!out.is_mine);
    assert_eq!(p.summary().total_debit, COIN);
}

#[test]
fn subtract_fee_and_coin_control() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let b = f.credit(2, COIN / 2);
    let c = f.credit(3, COIN / 4);

    // Coin control spends exactly the chosen coins, both of them.
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    draft.set_source(CoinSource::Outpoints(vec![b, c])).unwrap();
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    let mut spent: Vec<OutPoint> = p.summary().inputs.iter().map(|i| i.outpoint).collect();
    spent.sort();
    let mut want = vec![b, c];
    want.sort();
    assert_eq!(spent, want);
    // Reserved coins are no longer available to coin control.
    let other = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    other.set_source(CoinSource::Outpoints(vec![b])).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(other.estimate())),
        SendFailure::OutpointUnavailable(b)
    );
    f.engine.block_on(draft.abandon(p)).unwrap();

    // Subtract fee: the recipient pays the fee; external_sent excludes it.
    let mut r = pay(FOREIGN, COIN);
    r.subtract_fee_from_amount = true;
    let draft = f.draft(vec![r]);
    draft.set_source(CoinSource::Outpoints(vec![a])).unwrap();
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(COIN)))
        .unwrap();
    let s = p.summary();
    assert_eq!(s.outputs.len(), 1);
    assert_eq!(s.total_sent, COIN - s.fee);
    assert_eq!(s.external_sent, COIN - s.fee);
    assert_eq!(s.total_debit, COIN);
}

#[test]
fn locked_coins_are_skipped_and_reported() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let b = f.credit(2, COIN / 2);
    f.engine
        .block_on(f.session.lock_outpoints(f.wallet, vec![a]))
        .unwrap();
    assert_eq!(
        f.engine
            .block_on(f.session.locked_outpoints(f.wallet))
            .unwrap(),
        vec![a]
    );
    let listed = f
        .engine
        .block_on(f.session.utxos(f.wallet, CoinFilter::default()))
        .unwrap();
    assert_eq!(
        listed.iter().map(|c| c.outpoint).collect::<Vec<_>>(),
        vec![b]
    );
    let all = f
        .engine
        .block_on(f.session.utxos(
            f.wallet,
            CoinFilter {
                include_locked: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(
        all.iter()
            .any(|c| c.outpoint == a && c.user_locked && !c.spendable)
    );
    let max = f
        .engine
        .block_on(f.session.max_spendable(
            f.wallet,
            CoinSource::Any,
            FeeMode::Recommended { target_blocks: 6 },
        ))
        .unwrap();
    assert_eq!(max, COIN / 2);

    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    assert_eq!(p.summary().inputs[0].outpoint, b);
    f.engine.block_on(draft.abandon(p)).unwrap();
    draft.set_source(CoinSource::Outpoints(vec![a])).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.estimate())),
        SendFailure::OutpointUnavailable(a)
    );
    // More than the unlocked coins hold.
    draft.set_source(CoinSource::Any).unwrap();
    draft.set_recipients(vec![pay(FOREIGN, COIN)]).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.estimate())),
        SendFailure::AmountExceedsBalance {
            available: COIN / 2
        }
    );

    f.engine
        .block_on(f.session.unlock_outpoints(f.wallet, vec![a]))
        .unwrap();
    assert!(
        f.engine
            .block_on(f.session.locked_outpoints(f.wallet))
            .unwrap()
            .is_empty()
    );
    let unknown = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
    let r = f
        .engine
        .block_on(f.session.lock_outpoints(f.wallet, vec![unknown]));
    assert!(matches!(r, Err(EngineError::OutpointNotFound(_))), "{r:?}");
}

/// E0-04 §2a.3's F7 mitigation (H6 part 1 in Mode B): the inputs of a
/// tracked `Built` or `Broadcast` asset lock are reserved, for automatic
/// selection, coin control, `max_spendable` and the coin list alike, until
/// the lock reaches a proof, is consumed or is untracked. §12 item 9's
/// "SPV slow to start" shape: SPV never starts here, so no resume pass ran,
/// and the exclusion already holds.
#[test]
fn tracked_asset_lock_inputs_are_not_selectable() {
    use platform_wallet::AssetLockStatus as S;
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let b = f.credit(2, COIN / 2);
    let lock = f.track_lock(50, &[a], Some(S::Built));
    // No pass ran: SPV never started, and a trigger does not start one.
    f.session.leases.request_resume(Some(f.wallet));
    assert!(!f.session.leases.pass_running(f.wallet));
    assert!(!f.session.leases.resumed(f.wallet, lock));

    let max = |source: CoinSource| {
        f.engine
            .block_on(f.session.max_spendable(
                f.wallet,
                source,
                FeeMode::Recommended { target_blocks: 6 },
            ))
            .unwrap()
    };
    let excluded = |status: &str| {
        let coins = f
            .engine
            .block_on(f.session.utxos(
                f.wallet,
                CoinFilter {
                    include_locked: true,
                    ..Default::default()
                },
            ))
            .unwrap();
        let coin = coins.iter().find(|c| c.outpoint == a).unwrap();
        assert!(coin.reserved && !coin.spendable, "{status}: {coin:?}");
        assert!(!coin.user_locked);
        assert_eq!(max(CoinSource::Any), COIN / 2, "{status}");
        // Never picked automatically, even when it alone would do.
        let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
        let p = f
            .engine
            .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
            .unwrap();
        assert_eq!(
            p.summary()
                .inputs
                .iter()
                .map(|i| i.outpoint)
                .collect::<Vec<_>>(),
            vec![b],
            "{status}"
        );
        f.engine.block_on(draft.abandon(p)).unwrap();
        let big = f.draft(vec![pay(FOREIGN, COIN * 3 / 4)]);
        assert_eq!(
            send_failure(f.engine.block_on(big.estimate())),
            SendFailure::AmountExceedsBalance {
                available: COIN / 2
            },
            "{status}"
        );
        // Refused when chosen by hand.
        draft.set_source(CoinSource::Outpoints(vec![a])).unwrap();
        assert_eq!(
            send_failure(f.engine.block_on(draft.estimate())),
            SendFailure::OutpointUnavailable(a),
            "{status}"
        );
        assert_eq!(
            send_failure(f.engine.block_on(draft.prepare(f.spend_grant(COIN)))),
            SendFailure::OutpointUnavailable(a),
            "{status}"
        );
    };
    excluded("Built");
    f.track_lock(50, &[a], Some(S::Broadcast));
    excluded("Broadcast");

    let selectable = |status: &str| {
        let coins = f
            .engine
            .block_on(f.session.utxos(f.wallet, CoinFilter::default()))
            .unwrap();
        let coin = coins.iter().find(|c| c.outpoint == a).unwrap();
        assert!(!coin.reserved && coin.spendable, "{status}: {coin:?}");
        assert_eq!(max(CoinSource::Outpoints(vec![a])), COIN, "{status}");
        let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
        draft.set_source(CoinSource::Outpoints(vec![a])).unwrap();
        let p = f
            .engine
            .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
            .unwrap();
        assert_eq!(p.summary().inputs[0].outpoint, a, "{status}");
        f.engine.block_on(draft.abandon(p)).unwrap();
    };
    f.track_lock(50, &[a], Some(S::InstantSendLocked));
    selectable("InstantSendLocked");
    f.track_lock(50, &[a], Some(S::ChainLocked));
    selectable("ChainLocked");
    f.track_lock(50, &[a], Some(S::RecoveredFromChain));
    selectable("RecoveredFromChain");
    f.track_lock(50, &[a], Some(S::Consumed));
    selectable("Consumed");
    f.track_lock(50, &[a], Some(S::Built));
    excluded("Built again");
    f.track_lock(50, &[a], None);
    selectable("untracked");
}

/// DEC-179: a `PreparedTx` releases its own claim on its inputs, never the
/// coin: a coin another owner (a registration lock, a CoinJoin session)
/// also holds stays reserved through the abandon and the drop.
#[test]
fn a_prepared_tx_releases_only_its_own_claim() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let other = SpendOwner::mixing_session();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let prepare = || {
        f.engine
            .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
            .unwrap()
    };

    let p = prepare();
    let mine = SpendOwner::Tx(p.summary().txid.parse().unwrap());
    assert_eq!(f.session.spends.held_by(&f.wallet, mine), [a].into());
    f.session.spends.reserve(f.wallet, other, [a]);
    f.engine.block_on(draft.abandon(Arc::clone(&p))).unwrap();
    assert!(f.session.spends.held_by(&f.wallet, mine).is_empty());
    assert_eq!(f.reserved(), vec![a]);
    assert_eq!(
        send_failure(f.engine.block_on(draft.estimate())),
        SendFailure::AmountExceedsBalance { available: 0 }
    );

    f.session.spends.release(&f.wallet, other);
    assert!(f.reserved().is_empty());
    let p = prepare();
    f.session.spends.reserve(f.wallet, other, [a]);
    drop(p);
    assert_eq!(f.reserved(), vec![a]);
    f.session.spends.release(&f.wallet, other);
    assert!(f.reserved().is_empty());
}

#[test]
fn broadcast_needs_spv_and_drop_releases() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    let r = f.engine.block_on(draft.broadcast(Arc::clone(&p)));
    assert_eq!(send_failure(r), SendFailure::NoPeers);
    // Never sent: released, and spent for any further broadcast.
    assert!(f.reserved().is_empty());
    f.engine.block_on(draft.abandon(Arc::clone(&p))).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.broadcast(p))),
        SendFailure::PreparedTxSpent
    );

    // Dropping a pending transaction releases its inputs.
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    assert_eq!(f.reserved(), vec![a]);
    drop(p);
    assert!(f.reserved().is_empty());
    // key-wallet's reservation is released by a task the drop spawned.
    let mut ok = false;
    for _ in 0..50 {
        let d = f.draft(vec![pay(FOREIGN, 10_000_000)]);
        if f.engine.block_on(d.estimate()).is_ok()
            && f.engine
                .block_on(d.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
                .is_ok()
        {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ok,
        "the dropped transaction's input never became spendable again"
    );
}

#[test]
fn locked_vault_and_validation_errors() {
    let f = fixture(true);
    f.credit(1, COIN);
    f.session.lock_vault_sync().unwrap();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    assert_eq!(
        send_failure(f.engine.block_on(draft.prepare("g".into()))),
        SendFailure::VaultLocked
    );
    let empty = f.session.new_tx_draft(f.wallet).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(empty.estimate())),
        SendFailure::NoRecipients
    );
    // QT-051: the CoinJoin source is accepted; the wallet holds no fully
    // mixed coins, so nothing can be paid from it.
    let mixed = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    mixed.set_source(CoinSource::FullyMixedOnly).unwrap();
    assert!(matches!(
        send_failure(f.engine.block_on(mixed.estimate())),
        SendFailure::InsufficientMixedFunds { available: 0 }
    ));
    assert_eq!(
        f.engine
            .block_on(f.session.max_spendable(
                f.wallet,
                CoinSource::FullyMixedOnly,
                FeeMode::PerKb(1_000)
            ))
            .unwrap(),
        0
    );
    // Only the automatic change policy goes with it (change is paid as fee).
    mixed
        .set_change(ChangePolicy::Address(FOREIGN.into()))
        .unwrap();
    assert!(matches!(
        f.engine.block_on(mixed.estimate()),
        Err(EngineError::InvalidArgument(_))
    ));
    assert!(matches!(
        empty.set_change(ChangePolicy::Address("bogus".into())),
        Err(EngineError::Send(SendFailure::InvalidChangeAddress))
    ));
    assert!(matches!(
        empty.set_fee(FeeMode::PerKb(10)),
        Err(EngineError::InvalidArgument(_))
    ));
    let r = f.session.new_tx_draft(WalletId([7; 32]));
    assert!(matches!(r, Err(EngineError::WalletNotFound(_))), "{r:?}");
}

/// Review M4 and M-6 through the send path: a passphrase grant signs on a
/// locked vault and leaves it locked; a grant for another wallet is refused
/// and stays usable for its own wallet.
#[test]
fn passphrase_grant_signs_on_a_locked_vault_and_is_bound_to_its_wallet() {
    let f = fixture(true);
    f.credit(1, COIN);
    f.session.lock_vault_sync().unwrap();
    let vault = f.session.vault();
    let spend = GrantPurpose::Spend {
        max_duffs: 10_000_000 + FEE_ROOM,
    };

    let other = vault
        .authorize(
            spend,
            Some(&[7; 32]),
            Credential::Passphrase(b"pass phrase"),
        )
        .unwrap();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    assert_eq!(
        send_failure(f.engine.block_on(draft.prepare(other.id.clone()))),
        SendFailure::GrantInvalid
    );

    let grant = vault
        .authorize(
            spend,
            Some(&f.wallet.0),
            Credential::Passphrase(b"pass phrase"),
        )
        .unwrap();
    assert_eq!(vault.lock_state(), dw_vault::LockState::Locked);
    let prepared = f.engine.block_on(draft.prepare(grant.id)).unwrap();
    let tx: Transaction = dashcore::consensus::deserialize(&prepared.raw().unwrap()).unwrap();
    assert!(tx.input.iter().all(|i| i.script_sig.len() > 100));
    assert_eq!(vault.lock_state(), dw_vault::LockState::Locked);
    // The refused grant was not consumed.
    vault
        .check_grant(&other.id, dw_vault::GrantKind::Spend, Some(&[7; 32]))
        .unwrap();
}

#[test]
fn address_book_follows_dash_qt_rules() {
    let f = fixture(false);
    let own = {
        let session = Arc::clone(&f.session);
        let id = f.wallet;
        f.engine
            .block_on(async move {
                let inner = Arc::clone(&session);
                session
                    .on_runtime(async move {
                        let w = inner.wallet(&id).await?;
                        Ok(w.core().next_receive_address_for_account(0).await?)
                    })
                    .await
            })
            .unwrap()
            .to_string()
    };
    let save = |address: &str, label: &str, purpose, replace| {
        f.engine.block_on(f.session.save_address_book_entry(
            f.wallet,
            address.into(),
            label.into(),
            purpose,
            replace,
        ))
    };
    let e = save(FOREIGN, "Alice", BookPurpose::Send, false).unwrap();
    assert_eq!(e.label, "Alice");
    assert!(matches!(
        save(FOREIGN, "Bob", BookPurpose::Send, false),
        Err(EngineError::Labels(LabelsFailure::DuplicateAddress))
    ));
    assert_eq!(
        save(FOREIGN, "Bob", BookPurpose::Send, true).unwrap().label,
        "Bob"
    );
    assert!(matches!(
        save(&own, "mine", BookPurpose::Send, false),
        Err(EngineError::Labels(LabelsFailure::OwnAddress))
    ));
    save(&own, "Savings", BookPurpose::Receive, false).unwrap();
    assert!(matches!(
        save("not an address", "x", BookPurpose::Send, false),
        Err(EngineError::Labels(LabelsFailure::InvalidAddress))
    ));
    let book = f
        .engine
        .block_on(f.session.address_book(f.wallet, None, None))
        .unwrap();
    assert_eq!(
        book.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
        vec!["Bob", "Savings"]
    );
    let found = f
        .engine
        .block_on(
            f.session
                .address_book(f.wallet, Some(BookPurpose::Send), Some("b*b".into())),
        )
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(matches!(
        f.engine
            .block_on(f.session.delete_address_book_entry(f.wallet, own.clone())),
        Err(EngineError::Labels(LabelsFailure::ReceiveEntryNotDeletable))
    ));
    f.engine
        .block_on(
            f.session
                .delete_address_book_entry(f.wallet, FOREIGN.into()),
        )
        .unwrap();
    assert!(matches!(
        f.engine.block_on(
            f.session
                .delete_address_book_entry(f.wallet, FOREIGN.into())
        ),
        Err(EngineError::Labels(LabelsFailure::EntryNotFound))
    ));

    let txid = "ab".repeat(32);
    f.engine
        .block_on(
            f.session
                .set_tx_label(f.wallet, txid.clone(), Some("rent".into())),
        )
        .unwrap();
    assert_eq!(
        f.engine
            .block_on(f.session.tx_label(f.wallet, txid.clone()))
            .unwrap(),
        Some("rent".into())
    );
    f.engine
        .block_on(f.session.set_tx_label(f.wallet, txid.clone(), None))
        .unwrap();
    assert_eq!(
        f.engine
            .block_on(f.session.tx_label(f.wallet, txid))
            .unwrap(),
        None
    );
}

#[test]
fn close_waits_for_in_flight_prepare_and_broadcast() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.credit(2, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let prepared = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    let grant = f.spend_grant(10_000_000 + FEE_ROOM);
    let wallet = {
        let session = Arc::clone(&f.session);
        let inner = Arc::clone(&session);
        let id = f.wallet;
        f.engine
            .block_on(session.on_runtime(async move { inner.wallet(&id).await }))
            .unwrap()
    };
    f.engine.block_on(async {
        // Holding the wallet manager's write lock parks both calls once they
        // are admitted: prepare reading the coins, broadcast (SPV stopped:
        // never sent) releasing key-wallet's reservation.
        let held = wallet.state_mut().await;
        let prepare = tokio::spawn({
            let draft = Arc::clone(&draft);
            async move { draft.prepare(grant).await }
        });
        let broadcast = tokio::spawn({
            let draft = Arc::clone(&draft);
            async move { draft.broadcast(prepared).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut close = Box::pin(f.engine.close_network(DashNetwork::Regtest));
        assert!(
            tokio::time::timeout(Duration::from_millis(500), &mut close)
                .await
                .is_err(),
            "close finished while a prepare and a broadcast were in flight"
        );
        assert!(!prepare.is_finished() && !broadcast.is_finished());
        drop(held);

        // Both complete against the open session, then the close does.
        let second = prepare.await.unwrap().expect("admitted before the close");
        assert_eq!(send_failure(broadcast.await.unwrap()), SendFailure::NoPeers);
        assert!(close.await.unwrap());
        assert!(!f.session.is_open());
        // Calls after the close are refused.
        assert!(matches!(
            draft.broadcast(second).await,
            Err(EngineError::NetworkNotOpen(_))
        ));
    });
}

#[test]
fn unknown_outcome_stays_reserved_and_rebroadcastable() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        .unwrap();
    // What a dispatch without an acceptance verdict leaves behind; the
    // regtest suite (test_l1_send.py) drives a real one.
    *p.phase() = Phase::Unknown;

    // A repeat with SPV stopped reaches nobody, but the first dispatch may
    // have: the outcome is still unknown and the coin stays reserved.
    for _ in 0..2 {
        match send_failure(f.engine.block_on(draft.broadcast(Arc::clone(&p)))) {
            SendFailure::BroadcastUnknown { reason } => {
                assert!(reason.contains("SPV"), "{reason}")
            }
            other => panic!("expected broadcast_unknown, got {other:?}"),
        }
        assert_eq!(f.reserved(), vec![a]);
    }
    assert_eq!(
        send_failure(f.engine.block_on(draft.abandon(Arc::clone(&p)))),
        SendFailure::PreparedTxSpent
    );
    // Dropping the handle releases nothing, and selection still skips it.
    drop(p);
    assert_eq!(f.reserved(), vec![a]);
    let other = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    assert_eq!(
        send_failure(f.engine.block_on(other.estimate())),
        SendFailure::AmountExceedsBalance { available: 0 }
    );

    // Once the wallet sees the coin spent, the reservation goes with it.
    f.see_spent(a);
    assert!(f.reserved().is_empty());
    assert!(f.session.spends.snapshot(&f.wallet).is_empty());
}

#[test]
fn coin_control_choice_the_builder_would_drop_fails_before_the_grant() {
    let f = fixture(false);
    let big = f.credit(1, COIN);
    // At 10 duff/byte an input costs 1,480 duffs × 1000 / 1000 = 1.48M
    // duffs; this coin is worth less, so key-wallet's selector covers the
    // payment without it.
    let tiny = f.credit(2, 1_000_000);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    draft
        .set_source(CoinSource::Outpoints(vec![big, tiny]))
        .unwrap();
    draft.set_fee(FeeMode::PerKb(MAX_FEE_PER_KB)).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.estimate())),
        SendFailure::OutpointUnavailable(tiny)
    );
    // At this rate the fee can be up to the fee bound.
    let grant = f.spend_grant(10_000_000 + MAX_TX_FEE);
    assert_eq!(
        send_failure(f.engine.block_on(draft.prepare(grant.clone()))),
        SendFailure::OutpointUnavailable(tiny)
    );
    assert!(f.reserved().is_empty());
    // The grant was not redeemed: it pays for the payment without the coin.
    draft.set_source(CoinSource::Outpoints(vec![big])).unwrap();
    let p = f.engine.block_on(draft.prepare(grant)).unwrap();
    assert_eq!(p.summary().inputs.len(), 1);
}

#[test]
fn send_metadata_is_written_only_for_a_sent_payment_and_never_relabels() {
    let f = fixture(false);
    f.credit(1, COIN);
    let own = f.own_address();
    let listed_unlabelled = other_foreign();
    let new = Address::new(
        dashcore::Network::Regtest,
        Payload::PubkeyHash(PubkeyHash::from_byte_array([6; 20])),
    )
    .to_string();
    let save = |address: &str, label: &str, purpose| {
        f.engine
            .block_on(f.session.save_address_book_entry(
                f.wallet,
                address.into(),
                label.into(),
                purpose,
                false,
            ))
            .unwrap();
    };
    save(FOREIGN, "Alice", BookPurpose::Send);
    save(&own, "Savings", BookPurpose::Receive);
    save(&listed_unlabelled, "", BookPurpose::Send);
    let labelled = |address: &str, label: &str| Recipient {
        label: Some(label.into()),
        message: Some("rent".into()),
        ..pay(address, 1_000_000)
    };
    let draft = f.draft(vec![
        labelled(FOREIGN, "Bob"),
        labelled(&own, "Mine"),
        labelled(&listed_unlabelled, "Carol"),
        pay(&new, 1_000_000),
    ]);
    let book = || {
        let mut entries: Vec<(String, String, BookPurpose)> = f
            .engine
            .block_on(f.session.address_book(f.wallet, None, None))
            .unwrap()
            .into_iter()
            .map(|e| (e.address, e.label, e.purpose))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    };
    let before = book();

    // Never sent (SPV stopped): nothing is written.
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(3_000_000 + FEE_ROOM)))
        .unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.broadcast(Arc::clone(&p)))),
        SendFailure::NoPeers
    );
    assert_eq!(book(), before);
    let txid = p.summary().txid.clone();
    let message = || {
        f.engine
            .block_on(f.session.tx_message(f.wallet, txid.clone()))
            .unwrap()
    };
    assert_eq!(message(), None);

    // What an accepted (or unknown) broadcast writes.
    f.engine
        .block_on(f.session.record_send_metadata(f.wallet, &p))
        .unwrap();
    let mut want = vec![
        (FOREIGN.to_string(), "Alice".to_string(), BookPurpose::Send),
        (own.clone(), "Savings".to_string(), BookPurpose::Receive),
        (
            listed_unlabelled.clone(),
            "Carol".to_string(),
            BookPurpose::Send,
        ),
        (new.clone(), String::new(), BookPurpose::Send),
    ];
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(book(), want);
    assert_eq!(message().as_deref(), Some("rent\nrent\nrent"));
    // Writing it again (a repeated broadcast) changes nothing.
    f.engine
        .block_on(f.session.record_send_metadata(f.wallet, &p))
        .unwrap();
    assert_eq!(book(), want);
}

// ---- PSBT signing caps (review H1) ----

impl Fixture {
    /// Credits a confirmed coin of `value` to a fresh receive address and
    /// returns the transaction that created it (output 0), so a PSBT can
    /// carry it as the verified `non_witness_utxo`.
    fn credit_with_prev(&self, seed: u8, value: u64) -> Transaction {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine.block_on(async move {
            session
                .on_runtime(async move {
                    let wallet = inner.wallet(&id).await?;
                    let address = wallet.core().next_receive_address_for_account(0).await?;
                    let prev = Transaction {
                        version: 2,
                        lock_time: 0,
                        input: vec![dashcore::TxIn {
                            previous_output: OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
                            script_sig: dashcore::ScriptBuf::new(),
                            sequence: u32::MAX,
                            witness: dashcore::Witness::default(),
                        }],
                        output: vec![TxOut {
                            value,
                            script_pubkey: address.script_pubkey(),
                        }],
                        special_transaction_payload: None,
                    };
                    let outpoint = OutPoint::new(prev.txid(), 0);
                    let mut utxo = Utxo::new(outpoint, prev.output[0].clone(), address, 1, false);
                    utxo.is_confirmed = true;
                    let mut state = wallet.state_mut().await;
                    state
                        .core_wallet
                        .accounts
                        .standard_bip44_accounts
                        .get_mut(&0)
                        .expect("BIP44 account 0")
                        .utxos
                        .insert(outpoint, utxo);
                    Ok(prev)
                })
                .await
                .unwrap()
        })
    }

    fn sign_psbt(
        &self,
        psbt: &dw_psbt::PartiallySignedTransaction,
        grant: String,
    ) -> Result<dw_psbt::PartiallySignedTransaction, EngineError> {
        self.engine
            .block_on(self.session.sign_psbt(self.wallet, psbt.clone(), grant))
    }
}

fn script_of(address: &str) -> dashcore::ScriptBuf {
    Address::from_str(address)
        .unwrap()
        .assume_checked()
        .script_pubkey()
}

/// An unsigned PSBT spending output 0 of `prev` (its `non_witness_utxo`),
/// paying `outputs` (address, amount).
fn psbt_spending(
    prev: &Transaction,
    outputs: &[(&str, u64)],
) -> dw_psbt::PartiallySignedTransaction {
    let tx = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(prev.txid(), 0),
            script_sig: dashcore::ScriptBuf::new(),
            sequence: u32::MAX,
            witness: dashcore::Witness::default(),
        }],
        output: outputs
            .iter()
            .map(|(a, v)| TxOut {
                value: *v,
                script_pubkey: script_of(a),
            })
            .collect(),
        special_transaction_payload: None,
    };
    let mut psbt = dw_psbt::PartiallySignedTransaction::from_unsigned_tx(tx).unwrap();
    psbt.inputs[0].non_witness_utxo = Some(prev.clone());
    psbt
}

fn psbt_failure<T: std::fmt::Debug>(r: Result<T, EngineError>) -> PsbtFailure {
    match r {
        Err(EngineError::Psbt(f)) => f,
        other => panic!("expected a PSBT failure, got {other:?}"),
    }
}

/// MG-05 D19 (review r1 F2): a DashPay contact's address is not the
/// wallet's in a PSBT either. An output paying it gets no derivation
/// record (this wallet's fingerprint on the contact's key), and an input
/// spending from it is not reported signable.
#[test]
fn psbt_a_contacts_address_is_not_the_wallets() {
    let f = fixture(false);
    let own = Address::from_str(&f.own_address())
        .unwrap()
        .assume_checked();
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    let inner = Arc::clone(&session);
    let (contact, derivations) = f
        .engine
        .block_on(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            let mut state = wallet.state_mut().await;
            let info = &mut state.core_wallet;
            let contact = crate::coins::insert_contact_account(info, id.0);
            let fp = Some(key_wallet::bip32::Fingerprint::default());
            let derivation = |a| super::psbt::derivation_of(info, a, fp).is_some();
            let derivations = (derivation(&contact), derivation(&own));
            Ok::<_, EngineError>((contact, derivations))
        }))
        .unwrap();
    assert_eq!(derivations, (false, true), "(contact, own)");

    let prev = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([9; 32]), 0),
            script_sig: dashcore::ScriptBuf::new(),
            sequence: u32::MAX,
            witness: dashcore::Witness::default(),
        }],
        output: vec![TxOut {
            value: COIN,
            script_pubkey: contact.script_pubkey(),
        }],
        special_transaction_payload: None,
    };
    let psbt = psbt_spending(
        &prev,
        &[(FOREIGN, COIN / 2), (&contact.to_string(), COIN / 4)],
    );
    let a = f
        .engine
        .block_on(f.session.analyze_psbt(Some(f.wallet), psbt))
        .unwrap();
    assert_eq!(a.signability, PsbtSignability::NoMatchingKeys);
    assert!(a.outputs.iter().all(|o| !o.is_mine), "{:?}", a.outputs);
    // The same PSBT from a coin of the wallet's own can be signed.
    let mine = f.credit_with_prev(1, COIN);
    let psbt = psbt_spending(&mine, &[(&contact.to_string(), COIN / 2)]);
    let a = f
        .engine
        .block_on(f.session.analyze_psbt(Some(f.wallet), psbt))
        .unwrap();
    assert_eq!(a.signability, PsbtSignability::CanSign);
}

/// MG-04 D50 (validator r1 m-1): a coin paid to one of the wallet's ProTx
/// owner keys is not the wallet's to sign in a PSBT, so a PSBT spending it
/// is reported unsignable and leaves that input unsigned, as another
/// wallet's would be, instead of failing at the vault.
#[test]
fn psbt_an_owner_keys_coin_is_not_signed() {
    let f = fixture(false);
    let owner = f
        .engine
        .block_on(f.session.derive_masternode_keys(
            f.wallet,
            crate::masternode_keys::MasternodeKeyRole::Owner,
            0,
            1,
        ))
        .unwrap()
        .remove(0)
        .address
        .unwrap();
    let prev = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([8; 32]), 0),
            script_sig: dashcore::ScriptBuf::new(),
            sequence: u32::MAX,
            witness: dashcore::Witness::default(),
        }],
        output: vec![TxOut {
            value: COIN,
            script_pubkey: script_of(&owner),
        }],
        special_transaction_payload: None,
    };
    let psbt = psbt_spending(&prev, &[(FOREIGN, COIN - 10_000)]);
    let a = f
        .engine
        .block_on(f.session.analyze_psbt(Some(f.wallet), psbt.clone()))
        .unwrap();
    assert_eq!(a.signability, PsbtSignability::NoMatchingKeys);
    let signed = f.sign_psbt(&psbt, f.spend_grant(COIN)).unwrap();
    assert!(signed.inputs[0].partial_sigs.is_empty());
}

/// MG-04 D55 (review r2 m-1): a payment to one of the wallet's own
/// provider-key addresses (owner, voting) leaves its funds: key-wallet
/// tracks no coin there and the wallet signs none at an owner key. So the
/// grant caps it, as a recipient or as custom change; the send and a PSBT
/// show it going out, an unsigned PSBT records no derivation for it, and
/// the address book takes it as a Send entry only.
#[test]
fn a_payment_to_an_own_provider_key_leaves_the_wallet() {
    use crate::masternode_keys::MasternodeKeyRole::{Owner, Voting};
    let f = fixture(false);
    let key = |role| {
        f.engine
            .block_on(f.session.derive_masternode_keys(f.wallet, role, 0, 1))
            .unwrap()
            .remove(0)
            .address
            .unwrap()
    };
    let (owner, voting) = (key(Owner), key(Voting));
    let session = Arc::clone(&f.session);
    let (id, inner) = (f.wallet, Arc::clone(&session));
    let (o, v) = (owner.clone(), voting.clone());
    let listed = f
        .engine
        .block_on(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            let state = wallet.state().await;
            let fp = Some(key_wallet::bip32::Fingerprint::default());
            let seen = |a: &str| {
                let a = Address::from_str(a).unwrap().assume_checked();
                let owned = crate::coins::wallet_owns(&state.core_wallet, &a);
                let derived = super::psbt::derivation_of(&state.core_wallet, &a, fp).is_some();
                (owned, derived)
            };
            Ok::<_, EngineError>((seen(&o), seen(&v)))
        }))
        .unwrap();
    assert_eq!(
        listed,
        ((true, false), (true, false)),
        "the wallet lists both keys; no derivation record for either"
    );

    f.credit(1, COIN);
    let draft = f.draft(vec![pay(&owner, 30_000_000), pay(&voting, 20_000_000)]);
    let fee = f.engine.block_on(draft.estimate()).unwrap().fee;
    let r = f.engine.block_on(draft.prepare(f.spend_grant(fee)));
    assert_eq!(
        send_failure(r),
        SendFailure::GrantExceeded {
            max_duffs: fee,
            outflow: 50_000_000 + fee
        }
    );
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(50_000_000 + fee)))
        .unwrap();
    let s = p.summary();
    assert_eq!(s.external_sent, 50_000_000);
    let paid: Vec<_> = s.outputs.iter().filter(|o| !o.is_change).collect();
    assert_eq!(paid.len(), 2);
    assert!(paid.iter().all(|o| !o.is_mine), "{paid:?}");

    // Custom change to the owner key leaves the wallet too.
    f.credit(3, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 30_000_000)]);
    draft
        .set_change(ChangePolicy::Address(owner.clone()))
        .unwrap();
    let estimate = f.engine.block_on(draft.estimate()).unwrap();
    let r = f
        .engine
        .block_on(draft.prepare(f.spend_grant(30_000_000 + estimate.fee)));
    assert_eq!(
        send_failure(r),
        SendFailure::GrantExceeded {
            max_duffs: 30_000_000 + estimate.fee,
            outflow: COIN
        }
    );

    // The address book: a Send entry, never a Receive one.
    let save = |purpose| {
        f.engine.block_on(f.session.save_address_book_entry(
            f.wallet,
            owner.clone(),
            "owner".into(),
            purpose,
            false,
        ))
    };
    let r = save(dw_appdb::BookPurpose::Receive);
    assert!(matches!(r, Err(EngineError::InvalidArgument(_))), "{r:?}");
    save(dw_appdb::BookPurpose::Send).unwrap();

    let prev = f.credit_with_prev(2, COIN);
    let psbt = psbt_spending(&prev, &[(&owner, COIN / 2), (&voting, COIN / 2 - 10_000)]);
    let a = f
        .engine
        .block_on(f.session.analyze_psbt(Some(f.wallet), psbt.clone()))
        .unwrap();
    assert!(a.outputs.iter().all(|o| !o.is_mine), "{:?}", a.outputs);
    assert_eq!(a.external_sent, Some(COIN - 10_000));
    assert_eq!(
        psbt_failure(f.sign_psbt(&psbt, f.spend_grant(COIN / 2))),
        PsbtFailure::GrantExceeded {
            max_duffs: COIN / 2
        }
    );
}

/// MG-04 D55 (review r3 m-1), across every account the wallet derives: a
/// payment to any keys-only account's address (identity, asset-lock,
/// provider) leaves the wallet, so a fee-only grant refuses it; one to a
/// funds account's address (BIP44/BIP32, CoinJoin, DashPay receiving) stays,
/// so the same grant takes it and the output shows as the wallet's.
#[test]
fn only_a_funds_account_keeps_a_payment_in_the_wallet() {
    use key_wallet::account::AccountType;
    use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};
    let f = fixture(false);
    let (id, inner) = (f.wallet, Arc::clone(&f.session));
    let accounts = f
        .engine
        .block_on(f.session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            // The fixture derives no DashPay account: add a receiving one
            // (a contact pays the wallet there).
            let network = dashcore::Network::Regtest;
            let master = ExtendedPrivKey::new_master(network, &[7; 32]).unwrap();
            let receiving = key_wallet::managed_account::ManagedCoreFundsAccount::from_account(
                &key_wallet::Account {
                    parent_wallet_id: None,
                    account_type: AccountType::DashpayReceivingFunds {
                        index: 0,
                        user_identity_id: [1; 32],
                        friend_identity_id: [2; 32],
                    },
                    network,
                    account_xpub: ExtendedPubKey::from_priv(&master),
                    is_watch_only: true,
                },
            );
            (wallet.state_mut().await.core_wallet.accounts)
                .insert_funds_bearing_account(receiving)
                .unwrap();
            let state = wallet.state().await;
            let mut accounts = Vec::new();
            for account in state.core_wallet.all_managed_accounts() {
                let ty = account.managed_account_type();
                if ty.is_contact_owned() {
                    continue;
                }
                // The variant's name without its `Keys` suffix: the
                // operator-signing guard (dw-protx `no_operator_signing`)
                // refuses the provider operator variant's full name here.
                let name = format!("{ty:?}");
                let name = name.split([' ', '{', '(']).next().unwrap();
                let name = name.trim_end_matches("Keys").to_string();
                let address = ty
                    .address_pools()
                    .into_iter()
                    .flat_map(|pool| pool.addresses.values())
                    .map(|info| info.address.clone())
                    .next()
                    .unwrap_or_else(|| panic!("{name} has no address"));
                assert!(
                    crate::coins::wallet_owns(&state.core_wallet, &address),
                    "{name}"
                );
                let funds = crate::coins::wallet_funds_address(&state.core_wallet, &address);
                accounts.push((name, address.to_string(), funds));
            }
            Ok::<_, EngineError>(accounts)
        }))
        .unwrap();
    let names = |funds: bool| {
        let mut n: Vec<_> = accounts
            .iter()
            .filter(|(_, _, f)| *f == funds)
            .map(|(name, _, _)| name.as_str())
            .collect();
        n.sort_unstable();
        n.dedup();
        n
    };
    assert_eq!(
        names(false),
        [
            "AssetLockAddressTopUp",
            "AssetLockShieldedAddressTopUp",
            "IdentityInvitation",
            "IdentityRegistration",
            "IdentityTopUpNotBoundToIdentity",
            "ProviderOperator",
            "ProviderOwner",
            "ProviderPlatform",
            "ProviderVoting",
        ]
    );
    assert_eq!(
        names(true),
        ["CoinJoin", "DashpayReceivingFunds", "Standard"]
    );

    for (seed, (name, address, funds)) in (10u8..).zip(&accounts) {
        f.credit(seed, COIN);
        let draft = f.draft(vec![pay(address, 10_000_000)]);
        let fee = f.engine.block_on(draft.estimate()).unwrap().fee;
        let r = f.engine.block_on(draft.prepare(f.spend_grant(fee)));
        if *funds {
            let p = r.unwrap_or_else(|e| panic!("{name}: a self-transfer is capped: {e:?}"));
            let s = p.summary();
            assert_eq!(s.external_sent, 0, "{name}");
            assert!(
                s.outputs.iter().all(|o| o.is_mine),
                "{name}: {:?}",
                s.outputs
            );
        } else {
            assert_eq!(
                send_failure(r),
                SendFailure::GrantExceeded {
                    max_duffs: fee,
                    outflow: 10_000_000 + fee
                },
                "{name}"
            );
        }
    }
}

/// MG-05 r2 N2: the same claim through `create_unsigned` itself: a draft
/// paying a contact's address gets no derivation record for that output,
/// while its own input and change get theirs.
#[test]
fn psbt_create_unsigned_records_no_derivation_for_a_contacts_address() {
    let f = fixture(false);
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    let inner = Arc::clone(&session);
    let contact = f
        .engine
        .block_on(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            let mut state = wallet.state_mut().await;
            Ok::<_, EngineError>(crate::coins::insert_contact_account(
                &mut state.core_wallet,
                id.0,
            ))
        }))
        .unwrap();
    let prev = f.credit_with_prev(1, COIN);
    f.session.hub.history.with_wallet(f.wallet, |h| {
        h.txs.insert(
            prev.txid(),
            crate::history::TxEntry {
                tx: prev.clone(),
                context: key_wallet::transaction_checking::TransactionContext::Mempool,
                instant_locked: false,
                first_seen: None,
                own_inputs: Default::default(),
                abandoned: false,
            },
        )
    });
    (f.session.live().unwrap().appdb)
        .set_setting(
            &f.wallet.to_string(),
            super::psbt::FINGERPRINT_SETTING,
            Some("01020304"),
        )
        .unwrap();
    let d = f.draft(vec![pay(&contact.to_string(), COIN / 2)]);
    let psbt = f.engine.block_on(d.create_unsigned()).unwrap();
    let to_contact = contact.script_pubkey();
    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2, "the payment and change");
    for (o, out) in tx.output.iter().zip(&psbt.outputs) {
        assert_eq!(
            out.bip32_derivation.is_empty(),
            o.script_pubkey == to_contact,
            "{o:?}"
        );
    }
    assert!(!psbt.inputs[0].bip32_derivation.is_empty());
}

/// Before H1 the cap covered only the outputs to others, so a PSBT could
/// burn the rest of a coin as fee under a small grant.
#[test]
fn psbt_signing_caps_the_wallet_outflow_including_the_fee() {
    let f = fixture(false);
    let prev = f.credit_with_prev(1, COIN);
    let change = f.own_address();
    // 0.5 to someone else, 0.45 back, 0.05 DASH fee (under MAX_TX_FEE).
    let psbt = psbt_spending(&prev, &[(FOREIGN, COIN / 2), (&change, 45_000_000)]);
    let a = f
        .engine
        .block_on(f.session.analyze_psbt(Some(f.wallet), psbt.clone()))
        .unwrap();
    assert_eq!(a.external_sent, Some(COIN / 2));
    assert_eq!(a.fee, Some(5_000_000));
    assert_eq!(a.total, Some(55_000_000));
    // A grant covering only the external outputs is not enough.
    assert_eq!(
        psbt_failure(f.sign_psbt(&psbt, f.spend_grant(COIN / 2))),
        PsbtFailure::GrantExceeded {
            max_duffs: COIN / 2
        }
    );
    let signed = f.sign_psbt(&psbt, f.spend_grant(55_000_000)).unwrap();
    assert_eq!(signed.inputs[0].partial_sigs.len(), 1);

    // Burning the coin as fee is refused whatever the grant.
    let burn = psbt_spending(&prev, &[(FOREIGN, COIN / 100)]);
    assert_eq!(
        psbt_failure(f.sign_psbt(&burn, f.spend_grant(COIN))),
        PsbtFailure::AbsurdFee {
            fee: COIN - COIN / 100
        }
    );
    // The refusal came before the grant was redeemed.
    let grant = f.spend_grant(COIN);
    psbt_failure(f.sign_psbt(&burn, grant.clone()));
    f.session
        .vault()
        .check_grant(&grant, dw_vault::GrantKind::Spend, Some(&f.wallet.0))
        .unwrap();
}

/// U-05 D40: the analysis names the payment by its unsigned txid, which
/// signatures and extra PSBT fields leave alone and another payment changes.
#[test]
fn psbt_analysis_names_the_unsigned_transaction() {
    let f = fixture(false);
    let prev = f.credit_with_prev(1, COIN);
    let change = f.own_address();
    let psbt = psbt_spending(&prev, &[(FOREIGN, COIN / 2), (&change, 45_000_000)]);
    let analyze = |p: &dw_psbt::PartiallySignedTransaction| {
        f.engine
            .block_on(f.session.analyze_psbt(Some(f.wallet), p.clone()))
            .unwrap()
            .unsigned_txid
    };
    let txid = analyze(&psbt);
    assert_eq!(txid, psbt.unsigned_tx.txid());

    let mut annotated = psbt.clone();
    annotated.unknown.insert(
        dw_psbt::psbt::raw::Key {
            type_value: 0xf0,
            key: vec![1, 2, 3],
        },
        vec![9],
    );
    assert_eq!(analyze(&annotated), txid);
    let signed = f.sign_psbt(&psbt, f.spend_grant(55_000_000)).unwrap();
    assert_eq!(analyze(&signed), txid);
    // No wallet: the same name.
    let unloaded = f
        .engine
        .block_on(f.session.analyze_psbt(None, signed))
        .unwrap();
    assert_eq!(unloaded.unsigned_txid, txid);

    let other = psbt_spending(&prev, &[(FOREIGN, COIN / 2), (&change, 44_000_000)]);
    assert_ne!(analyze(&other), txid);
}

/// `witness_utxo` (no segwit on Dash; nothing commits to its value) never
/// stands in for the previous transaction: the fee is unknown and nothing
/// is signed.
#[test]
fn psbt_witness_only_inputs_are_refused() {
    let f = fixture(false);
    let prev = f.credit_with_prev(1, COIN);
    let mut psbt = psbt_spending(&prev, &[(FOREIGN, COIN / 100)]);
    // Claims our script with a small value, so the fee would look normal.
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: COIN / 100 + 1_000,
        script_pubkey: prev.output[0].script_pubkey.clone(),
    });
    psbt.inputs[0].non_witness_utxo = None;
    let a = f
        .engine
        .block_on(f.session.analyze_psbt(Some(f.wallet), psbt.clone()))
        .unwrap();
    assert_eq!(a.fee, None);
    assert_eq!(a.total, None);
    assert_eq!(a.signability, PsbtSignability::NoMatchingKeys);
    assert_eq!(
        psbt_failure(f.sign_psbt(&psbt, f.spend_grant(COIN * 2))),
        PsbtFailure::FeeUnknown
    );

    // A second, verified input of ours does not make the unknown one count.
    let other = f.credit_with_prev(2, COIN);
    let mut two = psbt_spending(&other, &[(FOREIGN, COIN / 100)]);
    two.unsigned_tx
        .input
        .push(psbt.unsigned_tx.input[0].clone());
    two.inputs.push(psbt.inputs[0].clone());
    assert_eq!(
        psbt_failure(f.sign_psbt(&two, f.spend_grant(COIN * 2))),
        PsbtFailure::FeeUnknown
    );
}

/// IOS-016 end to end: a quick-unlock grant (0.5 DASH limit) signs a PSBT
/// whose outflow, fee included, is under the limit and is refused above it
/// even when the outputs to others alone are under it.
#[test]
fn psbt_quick_unlock_grant_is_capped_by_the_outflow() {
    let f = fixture(true);
    let prev = f.credit_with_prev(1, COIN);
    let change = f.own_address();
    let vault = f.session.vault();
    let change_grant = vault
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(b"pass phrase"),
        )
        .unwrap();
    let key = vault.enroll_quick_unlock(&change_grant.id).unwrap();
    f.session.lock_vault_sync().unwrap();
    let limit = dw_vault::DEFAULT_QUICK_UNLOCK_SPEND_LIMIT;
    assert_eq!(limit, COIN / 2);
    let quick = || {
        vault
            .authorize(
                GrantPurpose::Spend { max_duffs: limit },
                Some(&f.wallet.0),
                Credential::QuickUnlock(&key),
            )
            .unwrap()
            .id
    };

    // 0.48 out + 0.03 fee = 0.51 DASH leaves the wallet.
    let over = psbt_spending(&prev, &[(FOREIGN, 48_000_000), (&change, 49_000_000)]);
    assert_eq!(
        psbt_failure(f.sign_psbt(&over, quick())),
        PsbtFailure::GrantExceeded { max_duffs: limit }
    );
    // 0.4 out + 0.001 fee.
    let under = psbt_spending(&prev, &[(FOREIGN, 40_000_000), (&change, 59_900_000)]);
    let mut signed = f.sign_psbt(&under, quick()).unwrap();
    assert!(dw_psbt::finalize(&mut signed));
    assert_eq!(vault.lock_state(), dw_vault::LockState::Locked);
}

/// Fix-review L4: the quick-unlock spending limit caps what leaves the
/// wallet in Send as it does in `sign_psbt`, fee included: a payment of
/// exactly the limit is refused, the limit minus the fee goes through.
#[test]
#[allow(non_snake_case)]
fn test_IOS_016_send_quick_unlock_limit_includes_the_fee() {
    let f = fixture(true);
    f.credit(1, COIN);
    let vault = f.session.vault();
    let change_grant = vault
        .authorize(
            GrantPurpose::ChangeCredential,
            None,
            Credential::Passphrase(b"pass phrase"),
        )
        .unwrap();
    let key = vault.enroll_quick_unlock(&change_grant.id).unwrap();
    f.session.lock_vault_sync().unwrap();
    let limit = dw_vault::DEFAULT_QUICK_UNLOCK_SPEND_LIMIT;
    let quick = || {
        vault
            .authorize(
                GrantPurpose::Spend { max_duffs: limit },
                Some(&f.wallet.0),
                Credential::QuickUnlock(&key),
            )
            .unwrap()
            .id
    };

    let over = f.draft(vec![pay(FOREIGN, limit)]);
    let fee = f.engine.block_on(over.estimate()).unwrap().fee;
    assert!(fee > 0);
    assert_eq!(
        send_failure(f.engine.block_on(over.prepare(quick()))),
        SendFailure::GrantExceeded {
            max_duffs: limit,
            outflow: limit + fee
        }
    );
    let under = f.draft(vec![pay(FOREIGN, limit - fee)]);
    assert_eq!(f.engine.block_on(under.estimate()).unwrap().fee, fee);
    let p = f.engine.block_on(under.prepare(quick())).unwrap();
    assert_eq!(p.summary().total_debit, limit);
    assert_eq!(vault.lock_state(), dw_vault::LockState::Locked);
}

// --- The coin fence (E0-04 §2a.6, `coins.rs`) ------------------------------

/// Waits up to 5 s for `f`.
fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..250 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// A coin locked after a prepare read the coins is not signed by it: the
/// lock waits for the prepare's fence, which is held from the coin read
/// through the signing and the reservation, and the signed inputs are the
/// read's. Locked first, the coin is never selected or chosen.
#[test]
fn a_coin_locked_after_prepare_read_the_coins_is_not_signed_by_it() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let b = f.credit(2, COIN / 2);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    draft.set_source(CoinSource::Outpoints(vec![a])).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.coin_fences.pause_prepare.lock().unwrap() = Some(Arc::clone(&pause));
    let grant = f.spend_grant(10_000_000 + FEE_ROOM);
    let prepare = f.session.rt.spawn({
        let draft = Arc::clone(&draft);
        async move { draft.prepare(grant).await }
    });
    eventually("the prepare's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let lock = f.session.rt.spawn({
        let s = Arc::clone(&f.session);
        let w = f.wallet;
        async move { s.lock_outpoints(w, vec![a]).await }
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !lock.is_finished(),
        "the lock waits for the prepare's fence"
    );
    // Read without the fence (the coin list would wait for it).
    assert!(
        f.session.spends.snapshot(&f.wallet).is_empty(),
        "nothing reserved yet"
    );
    pause.release.notify_one();
    let prepared = f.engine.block_on(prepare).unwrap().unwrap();
    f.engine.block_on(lock).unwrap().unwrap();
    assert_eq!(
        prepared
            .summary()
            .inputs
            .iter()
            .map(|i| i.outpoint)
            .collect::<Vec<_>>(),
        vec![a],
        "the inputs of the coin read"
    );
    let raw = prepared.raw().unwrap();
    let tx: Transaction = dashcore::consensus::deserialize(&raw).unwrap();
    assert_eq!(
        tx.input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>(),
        vec![a]
    );
    assert_eq!(f.reserved(), vec![a], "reserved before the lock returned");
    f.engine.block_on(draft.abandon(prepared)).unwrap();

    // The reverse order: locked first, neither chosen nor selected.
    f.engine
        .block_on(f.session.lock_outpoints(f.wallet, vec![b]))
        .unwrap();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    draft.set_source(CoinSource::Outpoints(vec![b])).unwrap();
    assert_eq!(
        send_failure(
            f.engine
                .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        ),
        SendFailure::OutpointUnavailable(b)
    );
    draft.set_source(CoinSource::Any).unwrap();
    assert!(matches!(
        send_failure(
            f.engine
                .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
        ),
        SendFailure::AmountExceedsBalance { available: 0 }
    ));
}

/// A held coin fence stalls nothing that does not read the wallet's
/// coins: the CoinJoin status refresh skips the wallet (its cached status
/// stays), and locking the vault and unloading the wallet finish; the
/// unloaded wallet's held fence is kept until released (`coins.rs`).
#[test]
fn a_held_coin_fence_stalls_no_status_refresh_vault_lock_or_unload() {
    let f = fixture(false);
    f.credit(1, COIN);
    let s = &f.session;
    let fence = f.engine.block_on(s.coin_fence(f.wallet)).unwrap();
    f.engine.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            s.refresh_coinjoin_status(f.wallet).await;
            s.lock_vault().await.unwrap();
            s.unload_wallet(f.wallet).await.unwrap();
        })
        .await
        .expect("nothing here waits for the coin fence")
    });
    assert!(
        s.coin_fences.try_acquire(f.wallet).is_none(),
        "the unloaded wallet's held fence is kept"
    );
    drop(fence);
    drop(s.coin_fences.try_acquire(f.wallet).expect("released"));
}

/// Dust locks are applied only under the coin fence: a coin read while
/// another holder has the wallet's fence waits for it, and so does its
/// dust lock.
#[test]
fn dust_locks_are_applied_only_under_the_fence() {
    let f = fixture(false);
    f.engine
        .block_on(f.session.set_dust_protection(Some(10_000)))
        .unwrap();
    let dust = f.credit(1, 5_000);
    let fence = f.engine.block_on(f.session.coin_fence(f.wallet)).unwrap();
    let listing = f.session.rt.spawn({
        let s = Arc::clone(&f.session);
        let w = f.wallet;
        async move {
            s.utxos(
                w,
                CoinFilter {
                    include_locked: true,
                    ..Default::default()
                },
            )
            .await
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!listing.is_finished(), "the coin read waits for the fence");
    let locked = || {
        f.engine
            .block_on(f.session.locked_outpoints(f.wallet))
            .unwrap()
    };
    assert!(locked().is_empty(), "no dust lock outside the fence");
    drop(fence);
    let coins = f.engine.block_on(listing).unwrap().unwrap();
    assert!(coins.iter().any(|c| c.outpoint == dust && c.user_locked));
    assert_eq!(locked(), vec![dust]);

    // The funding policy check names each held coin: the dust lock apart
    // from a user's lock (E0-04 §2a.6).
    let mine = f.credit(2, 50_000);
    f.engine
        .block_on(f.session.lock_outpoints(f.wallet, vec![mine]))
        .unwrap();
    let fence = f.engine.block_on(f.session.coin_fence(f.wallet)).unwrap();
    let snapshot = f.engine.block_on(f.session.fenced_coins(&fence)).unwrap();
    let mut held: Vec<_> = snapshot
        .held_funding_coins()
        .map(|h| (h.outpoint, h.hold))
        .collect();
    held.sort();
    let mut expect = vec![
        (dust, crate::coins::CoinHold::DustLock),
        (mine, crate::coins::CoinHold::UserLock),
    ];
    expect.sort();
    assert_eq!(held, expect);
}

/// DEC-210, a facade writer: "Lock unspent" held in its app.sqlite job
/// past the wallet's removal, its caller's task cancelled, leaves no lock.
#[test]
fn a_lock_held_past_a_removal_leaves_nothing() {
    use crate::held_writes::{a_write_held_past_the_removal, hold_first_job};

    let f = fixture(false);
    let a = f.credit(1, COIN);
    let wipe = f
        .session
        .vault()
        .authorize(GrantPurpose::Wipe, Some(&f.wallet.0), Credential::None)
        .unwrap()
        .id;
    let (s, w) = (Arc::clone(&f.session), f.wallet);
    let (pause, task) = hold_first_job(&f.session, async move {
        let _ = s.lock_outpoints(w, vec![a]).await;
    });
    let db = f.session.live().unwrap().appdb;
    a_write_held_past_the_removal(&f.engine, &f.session, (w, wipe), &pause, Some(task), || {
        db.locks(&w.to_string()).unwrap().is_empty()
    });
}

/// Core's `RANDOM_CHANGE_POSITION` (review r1 nit): the change of a data
/// transaction lands before or after the data output, by chance. 64
/// prepares all alike would fail this once in 2^63.
#[test]
fn a_data_transactions_change_lands_at_a_random_position() {
    let f = fixture(false);
    f.credit(1, 3 * COIN);
    let data = TxOut {
        value: COIN,
        script_pubkey: dashcore::ScriptBuf::new_op_return(&[7u8; 32]),
    };
    let draft = f.session.new_tx_draft(f.wallet).unwrap();
    draft.set_data_output(data.clone()).unwrap();
    let mut seen = [false; 2];
    for _ in 0..64 {
        let prepared = f
            .engine
            .block_on(draft.prepare(f.spend_grant(2 * COIN)))
            .unwrap();
        let tx: dashcore::Transaction =
            dashcore::consensus::deserialize(&prepared.raw().unwrap()).unwrap();
        assert_eq!(tx.output.len(), 2);
        seen[tx.output.iter().position(|o| *o == data).unwrap()] = true;
        f.engine.block_on(draft.abandon(prepared)).unwrap();
        if seen == [true; 2] {
            return;
        }
    }
    panic!("the change always at one position: {seen:?}");
}

/// MG-06 D1: a proposal fee's change is automatic (a fresh BIP44 P2PKH
/// address) and it takes no CoinJoin source, so every output but its data
/// is P2PKH, as Core's collateral check needs.
#[test]
fn a_data_output_takes_only_automatic_change() {
    let f = fixture(false);
    let data = TxOut {
        value: 100_000_000,
        script_pubkey: dashcore::ScriptBuf::new_op_return(&[7u8; 32]),
    };
    let draft = f.session.new_tx_draft(f.wallet).unwrap();
    draft.set_data_output(data.clone()).unwrap();
    draft
        .set_change(ChangePolicy::Address(FOREIGN.into()))
        .unwrap();
    assert!(matches!(
        f.engine.block_on(draft.estimate()),
        Err(EngineError::InvalidArgument(_))
    ));
    let mixed = f.session.new_tx_draft(f.wallet).unwrap();
    mixed.set_data_output(data).unwrap();
    mixed.set_source(CoinSource::FullyMixedOnly).unwrap();
    assert!(matches!(
        f.engine.block_on(mixed.estimate()),
        Err(EngineError::InvalidArgument(_))
    ));
}

// ---- 0.1.x-bal §1: the replay window ----------------------------------------

impl Fixture {
    /// Sets key-wallet's filter checkpoint, as a rewind does
    /// (`reconcile_dashpay_rescan`, `spv_rescan_filters_blocking`), or a
    /// filter commit, which raises the processed height with it.
    fn checkpoint(&self, height: u32) {
        let (session, id) = (Arc::clone(&self.session), self.wallet);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                crate::events::set_checkpoint(&mut wallet.state_mut().await.core_wallet, height);
                Ok(())
            }))
            .unwrap();
    }

    /// dash-spv committed a filter batch up to `height`: the checkpoint
    /// advances and the hub hears of it.
    fn advance(&self, height: u32) {
        self.checkpoint(height);
        (self.session.hub).apply_wallet_event(
            &platform_wallet::events::WalletEvent::SyncHeightAdvanced {
                wallet_id: self.wallet.0,
                height,
            },
        );
    }

    /// The wallet's own transaction `seed`, spending `inputs`, in `context`.
    fn own_spend(
        &self,
        seed: u8,
        inputs: &[OutPoint],
        context: key_wallet::transaction_checking::TransactionContext,
    ) -> Txid {
        let tx = Transaction {
            version: 1,
            lock_time: u32::from(seed),
            input: (inputs.iter())
                .map(|o| dashcore::TxIn {
                    previous_output: *o,
                    ..Default::default()
                })
                .collect(),
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let txid = tx.txid();
        self.session.hub.history.with_wallet(self.wallet, |h| {
            h.txs.insert(
                txid,
                crate::history::TxEntry {
                    tx,
                    context,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs: Default::default(),
                    abandoned: false,
                },
            )
        });
        txid
    }

    fn max(&self) -> u64 {
        self.engine
            .block_on(self.session.max_spendable(
                self.wallet,
                CoinSource::Any,
                FeeMode::Recommended { target_blocks: 6 },
            ))
            .unwrap()
    }
}

fn replaying<T: std::fmt::Debug>(r: Result<T, EngineError>) -> bool {
    matches!(r, Err(EngineError::RescanInProgress))
}

/// A rewind of the filter checkpoint (here platform-wallet's DIP-15 backfill,
/// which no event announces) refuses a payment and an unsigned PSBT at once,
/// until the replay is back at the highest checkpoint seen; the estimate and
/// "Use available balance" still answer.
#[test]
fn a_replay_refuses_spends_until_it_reaches_its_target() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let grant = || f.spend_grant(10_000_000 + FEE_ROOM);
    f.checkpoint(500);
    assert!(replaying(f.engine.block_on(draft.prepare(grant()))));
    assert!(replaying(f.engine.block_on(draft.create_unsigned())));
    assert_eq!(f.engine.block_on(draft.estimate()).unwrap().input_count, 1);
    assert_eq!(f.max(), COIN);
    f.advance(999);
    assert!(replaying(f.engine.block_on(draft.prepare(grant()))));
    f.advance(1_000);
    // (The unsigned PSBT then wants the coin's funding transaction.)
    assert!(!replaying(f.engine.block_on(draft.create_unsigned())));
    f.engine.block_on(draft.prepare(grant())).unwrap();
}

/// The prepare of `draft` on the runtime, for a test to interleave with.
fn spawn_prepare(
    f: &Fixture,
    draft: &Arc<TxDraft>,
) -> tokio::task::JoinHandle<Result<Arc<PreparedTx>, EngineError>> {
    let (draft, grant) = (Arc::clone(draft), f.spend_grant(10_000_000 + FEE_ROOM));
    f.session
        .rt
        .spawn(async move { draft.prepare(grant).await })
}

/// A rewind while the payment waits for the coin fence (another spender
/// holds it) is seen: the guard reads the checkpoint under the fence.
#[test]
fn a_rewind_while_a_payment_waits_for_the_fence_refuses_it() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let fence = f.engine.block_on(f.session.coin_fence(f.wallet)).unwrap();
    let prepare = spawn_prepare(&f, &draft);
    eventually("the prepare waits for the fence", || {
        f.session.coin_fences.users(f.wallet) == 2
    });
    f.checkpoint(500);
    drop(fence);
    assert!(replaying(f.engine.block_on(prepare).unwrap()));
}

/// A rewind after the payment read its coins, before it signs them: the
/// guard's second reading refuses it.
#[test]
fn a_rewind_after_the_coin_read_refuses_the_payment() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.coin_fences.pause_prepare.lock().unwrap() = Some(Arc::clone(&pause));
    let prepare = spawn_prepare(&f, &draft);
    eventually("the prepare's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    f.checkpoint(500);
    pause.release.notify_one();
    assert!(replaying(f.engine.block_on(prepare).unwrap()));
    assert!(
        f.session.spends.snapshot(&f.wallet).is_empty(),
        "nothing reserved"
    );
}

/// The hub hears of a checkpoint late (a lagging event subscriber loses
/// events): the guard raises the target to the checkpoint it reads, so a
/// rewind below that is a replay.
#[test]
fn the_guard_raises_the_target_to_a_checkpoint_the_hub_missed() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.checkpoint(1_200);
    let guard = |floor| f.engine.block_on(f.session.replay_guard(f.wallet, floor));
    assert_eq!(guard(0).unwrap(), 1_200);
    assert_eq!(
        f.session.hub.wallet_state(&f.wallet).unwrap().scan_target,
        1_200
    );
    f.checkpoint(1_100);
    assert!(replaying(guard(0)));
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    assert!(replaying(
        f.engine
            .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
    ));
    f.checkpoint(1_200);
    assert!(replaying(guard(1_300)), "below the floor");
}

/// A user rescan holds spends. Cancelling it puts the checkpoint back but
/// leaves what the partial replay re-credited above where it stopped, so
/// spends stay held until a rewind to there or below has replayed back to the
/// target; a later replay from above that point does not release them. Each
/// replay below ends in one filter batch, so the hub hears only its end.
#[test]
fn a_user_rescan_holds_spends_and_its_cancel_keeps_them_held() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    let rescan = |from| {
        let (s, id) = (Arc::clone(&f.session), f.wallet);
        f.engine
            .block_on(
                (f.session)
                    .on_runtime(async move { s.start_rescan(vec![id], from, |_| from).await }),
            )
            .unwrap();
    };
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    // The refusal's code: the stopped rescan's (interrupted) is held until
    // a rescan runs again; a running one is a replay (in progress).
    let refusal = || {
        let prepared = f
            .engine
            .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)));
        prepared.err().map(|e| e.code())
    };
    let (running, stopped) = (Some("rescan_in_progress"), Some("rescan_interrupted"));
    // Cancelled right after its start: a batch may have applied blocks it
    // had not committed yet.
    rescan(400);
    assert!(f.engine.block_on(f.session.cancel_rescan()).unwrap());
    assert_eq!(refusal(), stopped, "cancelled before any batch committed");
    rescan(400);
    assert_eq!(refusal(), running);
    f.advance(700);
    assert!(f.engine.block_on(f.session.cancel_rescan()).unwrap());
    assert_eq!(refusal(), stopped, "blocks 701-1000 are not replayed");
    rescan(800);
    f.advance(1_000);
    assert_eq!(refusal(), stopped, "blocks 701-799 are still not replayed");
    // A second cancel, stopped further on, keeps the first one's point.
    rescan(800);
    f.advance(900);
    assert!(f.engine.block_on(f.session.cancel_rescan()).unwrap());
    rescan(850);
    assert_eq!(refusal(), stopped, "a rescan from above the hold runs");
    f.advance(1_000);
    assert_eq!(refusal(), stopped, "blocks 701-849 are still not replayed");
    rescan(701);
    assert_eq!(refusal(), running);
    f.advance(1_000);
    (f.engine
        .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM))))
    .unwrap();
}

/// After a cancel, a rewind the hub never hears of (a fork, DIP-15 or an
/// abandon) to at or below where the rescan stopped replays what it
/// skipped: spends wait for that replay, then go through without a rescan.
#[test]
fn a_hidden_rewind_below_a_cancelled_rescan_ends_its_hold() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    let (s, id) = (Arc::clone(&f.session), f.wallet);
    f.engine
        .block_on(
            (f.session).on_runtime(async move { s.start_rescan(vec![id], 400, |_| 400).await }),
        )
        .unwrap();
    f.advance(700);
    assert!(f.engine.block_on(f.session.cancel_rescan()).unwrap());
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let prepare = || {
        f.engine
            .block_on(draft.prepare(f.spend_grant(10_000_000 + FEE_ROOM)))
    };
    let refusal = || prepare().err().map(|e| e.code());
    let (running, stopped) = (Some("rescan_in_progress"), Some("rescan_interrupted"));
    f.checkpoint(701);
    assert_eq!(refusal(), stopped, "a replay from 702 skips block 701");
    f.checkpoint(1_000);
    assert_eq!(refusal(), stopped, "block 701 is still not replayed");
    f.checkpoint(700);
    assert_eq!(refusal(), running, "a replay from 701 covers it");
    f.checkpoint(999);
    assert_eq!(refusal(), running);
    f.checkpoint(1_000);
    prepare().unwrap();
}

/// The §1 residual: a replay re-credits a coin whose spend was stored while
/// the spender was unconfirmed or InstantSend-locked at close, and no block
/// spends it again until the spender is mined. Dash Core counts it spent
/// (`CWallet::IsSpent`): selection skips it and an asset-lock funding is
/// held back; an abandoned or conflicted spender frees it.
#[test]
fn a_coin_our_unconfirmed_transaction_spends_is_held() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    let other = f.credit(2, COIN / 2);
    assert_eq!(f.max(), COIN + COIN / 2);
    let spender = f.own_spend(0x51, &[coin], TransactionContext::Mempool);
    assert_eq!(f.max(), COIN / 2);
    assert_eq!(f.reserved(), vec![coin]);
    let held = f.engine.block_on(async {
        let fence = f.session.coin_fence(f.wallet).await.unwrap();
        let snapshot = f.session.fenced_coins(&fence).await.unwrap();
        snapshot
            .held_funding_coins()
            .map(|h| h.outpoint)
            .collect::<Vec<_>>()
    });
    assert_eq!(held, vec![coin], "the asset-lock builder can't exclude it");
    let p = f
        .engine
        .block_on(
            f.draft(vec![pay(FOREIGN, 10_000_000)])
                .prepare(f.spend_grant(10_000_000 + FEE_ROOM)),
        )
        .unwrap();
    assert_eq!(p.summary().inputs[0].outpoint, other);

    let entry = |change: &dyn Fn(&mut crate::history::TxEntry)| {
        (f.session.hub.history).with_wallet(f.wallet, |h| change(h.txs.get_mut(&spender).unwrap()))
    };
    // InstantSend-locked, then abandoned: the lock wins (`status_of`).
    entry(&|e| (e.instant_locked, e.abandoned) = (true, true));
    assert_eq!(f.reserved(), vec![coin, other]);
    entry(&|e| e.instant_locked = false);
    assert_eq!(f.reserved(), vec![other], "an abandoned spender frees it");
    entry(&|e| e.abandoned = false);
    // Conflicted: a transaction in a block spends another of its inputs.
    let rival = OutPoint::new(Txid::from_byte_array([0x52; 32]), 0);
    entry(&|e| {
        e.tx.input.push(dashcore::TxIn {
            previous_output: rival,
            ..Default::default()
        })
    });
    let block = TransactionContext::InBlock(BlockInfo::new(7, dashcore::BlockHash::all_zeros(), 9));
    f.own_spend(0x53, &[rival], block.clone());
    assert_eq!(f.reserved(), vec![other], "a conflicted spender frees it");
    // A spender in a block holds it, whatever the abandon flag says.
    entry(&|e| (e.context, e.abandoned) = (block.clone(), true));
    assert_eq!(f.reserved(), vec![coin, other]);
}

/// Core marks the descendants of a conflicted transaction conflicted too
/// (`RecursiveUpdateTxState`): A spends P, B spends A's output and our coin
/// Q, both out of a block; then R spends P in a block. A and B are only in
/// dw's stored history (key-wallet holds neither, so no sweep removes
/// them): Q is free to spend and to fund an asset lock.
#[test]
fn a_conflicted_transactions_descendant_frees_its_other_inputs() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let f = fixture(false);
    let q = f.credit(1, COIN);
    let p = OutPoint::new(Txid::from_byte_array([0x61; 32]), 0);
    let a = f.own_spend(0x62, &[p], TransactionContext::Mempool);
    f.own_spend(0x63, &[OutPoint::new(a, 0), q], TransactionContext::Mempool);
    assert_eq!(f.reserved(), vec![q], "B holds Q while A stands");
    let block = TransactionContext::InBlock(BlockInfo::new(7, dashcore::BlockHash::all_zeros(), 9));
    f.own_spend(0x64, &[p], block);
    assert_eq!(f.reserved(), Vec::<OutPoint>::new());
    let held = f.engine.block_on(async {
        let fence = f.session.coin_fence(f.wallet).await.unwrap();
        let snapshot = f.session.fenced_coins(&fence).await.unwrap();
        snapshot.held_funding_coins().count()
    });
    assert_eq!(held, 0, "an asset lock may fund from Q");
    let prepared = f
        .engine
        .block_on(
            f.draft(vec![pay(FOREIGN, 10_000_000)])
                .prepare(f.spend_grant(10_000_000 + FEE_ROOM)),
        )
        .unwrap();
    assert_eq!(prepared.summary().inputs[0].outpoint, q);
}

impl Fixture {
    /// The persister commits a store whose checkpoint is `height`, through
    /// the store platform-wallet writes to.
    fn commit(&self, height: u32) -> Result<(), platform_wallet::changeset::PersistenceError> {
        use platform_wallet::changeset::{
            CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
        };
        let (store, id) = (Arc::clone(&self.session.live().unwrap().store), self.wallet);
        self.engine
            .block_on(self.session.on_runtime(async move {
                let changeset = PlatformWalletChangeSet {
                    core: Some(CoreChangeSet {
                        synced_height: Some(height),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                Ok(tokio::task::spawn_blocking(move || store.store(id.0, changeset)).await?)
            }))
            .unwrap()
    }

    fn rescan(&self, from: u32) -> Result<(), EngineError> {
        let (s, id) = (Arc::clone(&self.session), self.wallet);
        self.engine.block_on(
            (self.session)
                .on_runtime(async move { s.start_rescan(vec![id], from, |_| from).await }),
        )
    }

    fn synced(&self) -> u32 {
        use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
        let (s, id) = (Arc::clone(&self.session), self.wallet);
        self.engine
            .block_on(self.session.on_runtime(async move {
                let wallet = s.wallet(&id).await?;
                Ok(wallet.state().await.core_wallet.synced_height())
            }))
            .unwrap()
    }

    fn guard(&self) -> Result<u32, EngineError> {
        self.engine
            .block_on(self.session.replay_guard(self.wallet, 0))
    }

    /// Closes the session and opens it again, as a quit and a relaunch.
    fn reopen(self) -> Fixture {
        let Fixture {
            _dir,
            engine,
            session,
            wallet,
        } = self;
        drop(session);
        assert!(
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        let session = engine
            .block_on(engine.open_network(
                DashNetwork::Regtest,
                crate::SessionOptions {
                    dapi_addresses: vec!["http://127.0.0.1:1".into()],
                    quorum_url: Some("http://127.0.0.1:1".into()),
                    spv_peers: vec!["127.0.0.1:1".into()],
                    ..Default::default()
                },
            ))
            .unwrap();
        Fixture {
            _dir,
            engine,
            session,
            wallet,
        }
    }
}

/// The replay record's seams.
impl Fixture {
    /// The wallet's replay record once the stores so far are applied.
    fn record(&self) -> Option<crate::replay_record::ReplayRecord> {
        let (replay, id) = (
            Arc::clone(&self.session.live().unwrap().replay),
            self.wallet,
        );
        self.engine
            .block_on(self.session.on_runtime(async move {
                replay.settled().await;
                replay.get(id).await
            }))
            .unwrap()
    }

    /// Waits until the stores so far are applied to the record.
    fn settle(&self) {
        let replay = Arc::clone(&self.session.live().unwrap().replay);
        self.engine
            .block_on(self.session.on_runtime(async move {
                replay.settled().await;
                Ok(())
            }))
            .unwrap();
    }

    /// The next rewind stops once its record is stored, as a quit there.
    fn quit_after_the_record(&self) {
        (self.session.live().unwrap().replay)
            .crash_after_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The next close clears no record, as a crash.
    fn crash_on_close(&self) {
        (self.session.live().unwrap().replay)
            .crash_on_close
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// `rescan`'s outcome when it quit after the record.
    fn assert_quit_before_the_rewind(&self, rescan: Result<(), EngineError>) {
        assert!(rescan.is_err());
        assert_eq!(self.synced(), 1_000, "the quit comes before the rewind");
    }
}

/// The app quits while a rescan replays below the stored checkpoint. The
/// reopened session must resume that replay, and hold spends until it is
/// back at the checkpoint the wallet had reached before the rescan.
#[test]
fn a_restart_mid_rescan_resumes_the_replay_and_holds_spends() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    let (session, id) = (Arc::clone(&f.session), f.wallet);
    let persister = Arc::clone(&session.live().unwrap().persister);
    f.engine
        .block_on(session.on_runtime(async move {
            tokio::task::spawn_blocking(move || {
                use platform_wallet::changeset::PlatformWalletPersistence;
                let core = platform_wallet::changeset::CoreChangeSet {
                    synced_height: Some(1_000),
                    ..Default::default()
                };
                let changeset = platform_wallet::changeset::PlatformWalletChangeSet {
                    core: Some(core),
                    ..Default::default()
                };
                persister.store(id.0, changeset).unwrap();
                persister.flush(id.0).unwrap();
            })
            .await
            .unwrap();
            Ok::<_, EngineError>(())
        }))
        .unwrap();
    let (s, id) = (Arc::clone(&f.session), f.wallet);
    f.engine
        .block_on(
            (f.session).on_runtime(async move { s.start_rescan(vec![id], 400, |_| 400).await }),
        )
        .unwrap();
    assert!(replaying(
        f.engine.block_on(f.session.replay_guard(f.wallet, 0))
    ));
    drop(session);

    assert!(
        f.engine
            .block_on(f.engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let reopened = f
        .engine
        .block_on(f.engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    let synced = f
        .engine
        .block_on({
            let (s, id) = (Arc::clone(&reopened), f.wallet);
            reopened.on_runtime(async move {
                use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
                let wallet = s.wallet(&id).await?;
                Ok::<_, EngineError>(wallet.state().await.core_wallet.synced_height())
            })
        })
        .unwrap();
    let guard = f.engine.block_on(reopened.replay_guard(f.wallet, 0));
    let progress = reopened.rescan_progress().unwrap();
    eprintln!(
        "ROW58 desktop restart mid-rescan: filter checkpoint {synced}, replay guard {guard:?}, \
         rescan progress {progress:?}"
    );
    assert!(
        synced < 400,
        "the reopened wallet resumes the filter scan at {}, past the rescan's start 400",
        synced + 1
    );
    assert!(
        replaying(guard),
        "spends go through after a restart mid-rescan"
    );
}

/// A cancelled replay is not resumed after a restart: its hold comes back
/// and refuses spends, until a rewind to where it began replays what the
/// cancel skipped.
#[test]
fn a_cancelled_replay_stays_held_across_a_restart() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.advance(600);
    assert!(f.engine.block_on(f.session.cancel_rescan()).unwrap());
    assert!(matches!(f.guard(), Err(EngineError::RescanInterrupted)));
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!("ROW58 cancel then restart: filter checkpoint {synced}, replay guard {guard:?}");
    assert_eq!(synced, 1_000, "a cancelled replay is not resumed");
    assert!(
        matches!(guard, Err(EngineError::RescanInterrupted)),
        "the cancel's hold is gone after a restart: {guard:?}"
    );
    f.rescan(400).unwrap();
    assert!(
        replaying(f.guard()),
        "a rescan from its start clears the hold"
    );
}

/// A quit after the record is stored and before the rewind: the resume
/// replays the stored range once, and the record goes when it is back.
#[test]
fn a_quit_between_the_record_and_the_rewind_costs_only_a_replay() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.quit_after_the_record();
    let rescan = f.rescan(400);
    f.assert_quit_before_the_rewind(rescan);
    f.crash_on_close();
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!(
        "ROW58 quit between record and rewind: filter checkpoint {synced}, replay guard {guard:?}"
    );
    assert!(
        synced < 400,
        "the replay is not resumed: checkpoint {synced}"
    );
    assert!(replaying(guard));
    f.advance(1_001);
    f.commit(1_001).unwrap();
    assert_eq!(f.record(), None, "the replay is back and stored");
    assert_eq!(f.guard().unwrap(), 1_001);
    let f = f.reopen();
    assert_eq!(f.synced(), 1_001);
    assert_eq!(f.guard().unwrap(), 1_001);
}

/// The replay runs past its target in memory while the persister commits
/// none of it, and a store made before the rewind commits after it: a
/// restart then resumes at or below what is committed.
#[test]
fn a_resume_starts_at_or_below_the_committed_replay() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    let store = Arc::clone(&f.session.live().unwrap().store);
    *store.fail_store.lock().unwrap() = Some(Box::new(|cs| cs.core.is_some()));
    f.advance(1_200);
    assert!(
        f.commit(1_200).is_err(),
        "the replay's batch is not flushed"
    );
    *store.fail_store.lock().unwrap() = None;
    drop(store);
    f.commit(1_000).unwrap();
    f.settle();
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!("ROW58 reached vs commit: filter checkpoint {synced}, replay guard {guard:?}");
    assert!(
        synced < 400,
        "resumed at {}, above the committed replay (nothing above 399)",
        synced + 1
    );
    assert!(replaying(guard));
}

/// A second rewind while the first's replay is not stored: the record
/// replays from the lower start to the higher target.
#[test]
fn overlapping_rewinds_merge_their_replays() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.advance(1_200);
    f.rescan(800).unwrap();
    let f = f.reopen();
    let synced = f.synced();
    f.advance(1_100);
    let guard = f.guard();
    eprintln!(
        "ROW58 overlapping rewinds: filter checkpoint {synced}, replay guard at 1100 {guard:?}"
    );
    assert!(
        synced < 400,
        "resumed at {}, past the first start",
        synced + 1
    );
    assert!(replaying(guard), "done below the second rewind's target");
    f.advance(1_200);
    assert_eq!(f.guard().unwrap(), 1_200);
}

/// A rewind no event announces (platform-wallet's DashPay backfill), once
/// the guard reads it, is resumed after a restart too.
#[test]
fn a_hidden_rewind_the_guard_saw_is_resumed_after_a_restart() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.checkpoint(500);
    assert!(replaying(f.guard()));
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!(
        "ROW58 hidden rewind then restart: filter checkpoint {synced}, replay guard {guard:?}"
    );
    assert!(synced <= 500, "resumed at {}, past the rewind", synced + 1);
    assert!(replaying(guard));
}

/// The record goes once a store above its target is committed with the
/// checkpoint back there, not before; a fork lowers its target to the new
/// chain; removing the wallet deletes it.
#[test]
fn a_replay_record_ends_with_its_stored_replay() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    let started = f.record().expect("recorded before the rewind");
    assert_eq!((started.target, started.reached), (1_000, 399));
    f.commit(1_001).unwrap();
    assert!(f.record().is_some(), "the checkpoint is not back yet");
    f.advance(1_000);
    f.commit(1_000).unwrap();
    assert!(
        f.record().is_some(),
        "a store at the target can predate the rewind"
    );
    f.advance(1_001);
    f.commit(1_001).unwrap();
    assert_eq!(f.record(), None);
    let f = f.reopen();
    assert_eq!(f.synced(), 1_001);
    assert_eq!(f.guard().unwrap(), 1_001);

    f.rescan(400).unwrap();
    (f.session.hub).apply_wallet_event(&platform_wallet::events::WalletEvent::ChainTruncated {
        wallet_id: f.wallet.0,
        height: 900,
        txids: Vec::new(),
        unconfirmed_txids: Vec::new(),
        restored_outpoints: Vec::new(),
        balance: key_wallet::wallet::balance::WalletCoreBalance::new(COIN, 0, 0, 0),
        account_balances: Default::default(),
    });
    let truncated = f.record().expect("still replaying");
    assert_eq!((truncated.target, truncated.reached), (900, 399));

    let wipe = (f.session.vault)
        .authorize(GrantPurpose::Wipe, Some(&f.wallet.0), Credential::None)
        .unwrap()
        .id;
    let appdb = Arc::clone(&f.session.live().unwrap().appdb);
    f.engine
        .block_on(f.session.remove_wallet(f.wallet, wipe))
        .unwrap();
    assert_eq!(
        appdb.setting(&f.wallet.to_string(), "replay").unwrap(),
        None
    );
}

/// A clean close with the replay back and a store at its target committed
/// ends the record: the persistence adapter has committed every event by
/// then. A crash there keeps it, which costs one replay.
#[test]
fn a_clean_close_after_the_replay_ends_its_record() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    assert!(
        f.record().is_some(),
        "a store at the target can predate the rewind"
    );
    f.crash_on_close();
    let f = f.reopen();
    let crashed = f.synced();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    let f = f.reopen();
    let (synced, guard, record) = (f.synced(), f.guard(), f.record());
    eprintln!(
        "ROW58 clean close after the replay: after a crash {crashed}, after a clean close \
         {synced}, replay guard {guard:?}, record {record:?}"
    );
    assert!(crashed < 400, "a crash keeps the record: {crashed}");
    assert_eq!(synced, 1_000, "a clean close ends the record");
    assert_eq!(guard.unwrap(), 1_000);
    assert_eq!(record, None);
}

/// Seams for coins the store holds.
impl Fixture {
    /// A coin of `value` funded at `height` to a fresh receive address.
    fn late_coin(&self, seed: u8, value: u64, height: u32) -> Utxo {
        let (session, id) = (Arc::clone(&self.session), self.wallet);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                let address = wallet.core().next_receive_address_for_account(0).await?;
                let mut utxo = Utxo::new(
                    OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
                    TxOut {
                        value,
                        script_pubkey: address.script_pubkey(),
                    },
                    address,
                    height,
                    false,
                );
                utxo.is_confirmed = true;
                Ok(utxo)
            }))
            .unwrap()
    }

    /// Puts `utxo` in key-wallet's unspent set, as a replay of its funding
    /// block does.
    fn put_coin(&self, utxo: Utxo) {
        let (session, id) = (Arc::clone(&self.session), self.wallet);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                let mut state = wallet.state_mut().await;
                (state.core_wallet.accounts.standard_bip44_accounts)
                    .get_mut(&0)
                    .expect("BIP44 account 0")
                    .utxos
                    .insert(utxo.outpoint, utxo);
                Ok(())
            }))
            .unwrap();
    }

    /// The persister commits `core` for the wallet, through the store
    /// platform-wallet writes to.
    fn store_core(&self, core: platform_wallet::changeset::CoreChangeSet) {
        use platform_wallet::changeset::{PlatformWalletChangeSet, PlatformWalletPersistence};
        let (store, id) = (Arc::clone(&self.session.live().unwrap().store), self.wallet);
        self.engine
            .block_on(self.session.on_runtime(async move {
                let changeset = PlatformWalletChangeSet {
                    core: Some(core),
                    ..Default::default()
                };
                tokio::task::spawn_blocking(move || store.store(id.0, changeset))
                    .await?
                    .map_err(|e| EngineError::Storage(e.to_string()))
            }))
            .unwrap();
    }

    /// Whether the coin list offers `outpoint`.
    fn offers(&self, outpoint: OutPoint) -> bool {
        let coins = (self.engine)
            .block_on(self.session.utxos(
                self.wallet,
                CoinFilter {
                    include_locked: true,
                    ..Default::default()
                },
            ))
            .unwrap();
        coins.iter().any(|c| c.outpoint == outpoint)
    }

    fn pay_from_the_draft(&self, draft: &Arc<TxDraft>) -> Result<Arc<PreparedTx>, EngineError> {
        (self.engine).block_on(draft.prepare(self.spend_grant(10_000_000 + FEE_ROOM)))
    }

    fn records(&self) -> Arc<crate::replay_record::ReplayRecords> {
        Arc::clone(&self.session.live().unwrap().replay)
    }
}

fn interrupted<T: std::fmt::Debug>(r: &Result<T, EngineError>) -> bool {
    matches!(r, Err(EngineError::RescanInterrupted))
}

/// The replay has passed a coin's funding block, so the coin is credited
/// again and stored unspent, and not yet the block that spent it, when
/// the app quits. The reopened wallet holds the stored coin: spends wait
/// for the resumed replay, which spends it again.
#[test]
fn a_quit_after_a_late_coin_replayed_holds_it_until_its_spend_replays() {
    use platform_wallet::changeset::CoreChangeSet;
    let f = fixture(false);
    let coin = f.late_coin(7, COIN, 150);
    f.store_core(CoreChangeSet {
        new_utxos: vec![coin.clone()],
        spent_utxos: vec![coin.clone()],
        synced_height: Some(1_000),
        ..Default::default()
    });
    f.advance(1_000);
    f.commit(1_000).unwrap();
    assert!(!f.offers(coin.outpoint), "spent at 700");
    f.rescan(100).unwrap();
    f.put_coin(coin.clone());
    f.store_core(CoreChangeSet {
        new_utxos: vec![coin.clone()],
        synced_height: Some(160),
        ..Default::default()
    });
    f.advance(160);
    assert!(f.offers(coin.outpoint), "the replay credits the coin again");
    f.crash_on_close();
    let f = f.reopen();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let (synced, offered, payment) = (
        f.synced(),
        f.offers(coin.outpoint),
        f.pay_from_the_draft(&draft),
    );
    eprintln!(
        "ROW58 late coin quit: filter checkpoint {synced}, stored spent coin offered {offered}, \
         payment {:?}",
        payment.as_ref().map(|p| p.summary().inputs[0].outpoint)
    );
    assert!(offered, "the store keeps the replay's credit");
    assert!(synced < 100, "resumed at {}, past the replay", synced + 1);
    assert!(replaying(payment), "the spent coin pays after a restart");
    f.see_spent(coin.outpoint);
    f.store_core(CoreChangeSet {
        spent_utxos: vec![coin.clone()],
        synced_height: Some(710),
        ..Default::default()
    });
    f.advance(1_001);
    f.commit(1_001).unwrap();
    assert_eq!(f.record(), None, "the replay is back and stored");
    assert!(!f.offers(coin.outpoint));
    assert!(!replaying(f.pay_from_the_draft(&draft)));
}

/// A cancel's hold is kept by a later rewind that starts above it, a
/// rescan or a hidden one (the DashPay backfill on each launch), whatever
/// the guard reads meanwhile, across restarts and a replay past the
/// target; a rescan from the hold ends it.
#[test]
fn a_rewind_above_a_cancel_hold_keeps_it_across_a_restart() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.advance(600);
    assert!(f.engine.block_on(f.session.cancel_rescan()).unwrap());
    f.rescan(800).unwrap();
    assert!(interrupted(&f.guard()));
    f.crash_on_close();
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!(
        "ROW58 rescan above the hold then restart: filter checkpoint {synced}, replay guard {guard:?}"
    );
    assert_eq!(synced, 1_000, "the cancelled replay is resumed");
    assert!(interrupted(&guard), "the hold is gone: {guard:?}");

    f.checkpoint(800);
    assert!(interrupted(&f.guard()));
    f.advance(1_001);
    f.commit(1_001).unwrap();
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!(
        "ROW58 hidden rewind above the hold then restart: filter checkpoint {synced}, replay guard {guard:?}"
    );
    assert_eq!(synced, 1_001, "the cancelled replay is resumed");
    assert!(interrupted(&guard), "the hold is gone: {guard:?}");

    f.rescan(400).unwrap();
    assert!(replaying(f.guard()), "a rescan from the hold ends it");
    f.crash_on_close();
    let f = f.reopen();
    assert!(f.synced() < 400);
    assert!(replaying(f.guard()));
}

/// A cancel whose hold cannot be stored fails, and the replay goes on as
/// if it was not cancelled: a clean close keeps its record.
#[test]
fn a_cancel_whose_hold_is_not_stored_fails_and_the_replay_goes_on() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.advance(600);
    let records = f.records();
    records
        .fail_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let cancel = f.engine.block_on(f.session.cancel_rescan());
    records
        .fail_writes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    drop(records);
    let (synced, guard) = (f.synced(), f.guard());
    let progress = f.session.rescan_progress().unwrap();
    eprintln!(
        "ROW58 cancel not stored: cancel {cancel:?}, filter checkpoint {synced}, replay guard \
         {guard:?}, rescan progress {progress:?}"
    );
    assert!(cancel.is_err(), "the cancel reports success");
    assert_eq!(synced, 600, "the cancel moved the checkpoint");
    assert!(replaying(guard));
    assert!(progress.is_some(), "the rescan stopped");
    let f = f.reopen();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let (synced, record) = (f.synced(), f.record());
    assert!(record.is_some_and(|r| !r.cancelled));
    assert!(synced < 400, "resumed at {}", synced + 1);
    assert!(replaying(f.pay_from_the_draft(&draft)));
}

/// A record whose replay the hub still holds (here a cancel's hold the
/// record does not carry) is not ended by a commit above its target or by
/// a clean close.
#[test]
fn a_replay_the_hub_holds_keeps_its_record() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    (f.session.hub).update_wallet_state(f.wallet, |s| s.replay_cancelled = Some(500));
    f.advance(1_001);
    f.commit(1_001).unwrap();
    assert!(f.record().is_some(), "ended in the session");
    let f = f.reopen();
    assert!(f.synced() < 400, "ended at the clean close");
    assert!(replaying(f.guard()));
}

/// A clean quit with the new chain at 901, below the old chain's height
/// 1000 a fork left stored: the record survives, the reopen replays from
/// `reached` and sends are back at the new tip; the record goes once the
/// chain is stored at the old height.
fn quit_below_then_past_the_old_height(f: Fixture, reached: u32) {
    f.advance(901);
    f.commit(901).unwrap();
    let f = f.reopen();
    assert_eq!(
        f.record().map(|r| (r.target, r.reached, r.forked_from)),
        Some((901, reached, 1_000)),
        "the clean close kept the record"
    );
    assert_eq!(f.synced(), reached);
    f.advance(901);
    assert_eq!(f.guard().unwrap(), 901);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    let f = f.reopen();
    assert_eq!((f.record(), f.synced()), (None, 1_000));
    assert!(f.guard().is_ok());
}

/// A fork lowers the replay's target to the new chain, and a restart keeps
/// it lowered: the replay is done at the shorter chain's tip, though the
/// stored checkpoint stays above it.
#[test]
fn a_fork_lowered_replay_target_stays_lowered_after_a_restart() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    (f.session.hub).apply_wallet_event(&platform_wallet::events::WalletEvent::ChainTruncated {
        wallet_id: f.wallet.0,
        height: 900,
        txids: Vec::new(),
        unconfirmed_txids: Vec::new(),
        restored_outpoints: Vec::new(),
        balance: key_wallet::wallet::balance::WalletCoreBalance::new(COIN, 0, 0, 0),
        account_balances: Default::default(),
    });
    assert_eq!(f.record().map(|r| r.target), Some(900));
    f.crash_on_close();
    let f = f.reopen();
    assert!(f.synced() < 400);
    assert!(replaying(f.guard()));
    f.advance(900);
    let guard = f.guard();
    eprintln!("ROW58 fork then restart: replay guard at the new tip 900 {guard:?}");
    assert_eq!(guard.unwrap(), 900, "the target went back above the chain");
    quit_below_then_past_the_old_height(f, 399);
}

/// Without a fork, a replay resumed after a restart is not done below
/// its target.
#[test]
fn a_resumed_replay_short_of_its_target_still_refuses() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.crash_on_close();
    let f = f.reopen();
    f.advance(900);
    assert!(replaying(f.guard()));
    f.advance(1_000);
    assert_eq!(f.guard().unwrap(), 1_000);
}

/// A wallet opened mid-session is published before its stored replay is
/// resumed: a payment in between is refused, and once resumed the replay
/// holds spends.
#[test]
fn a_wallet_opened_mid_session_refuses_spends_until_its_replay_resumes() {
    use platform_wallet::changeset::CoreChangeSet;
    let f = fixture(false);
    let coin = f.late_coin(7, COIN, 150);
    f.put_coin(coin.clone());
    f.store_core(CoreChangeSet {
        new_utxos: vec![coin.clone()],
        synced_height: Some(1_000),
        ..Default::default()
    });
    f.advance(1_000);
    f.rescan(400).unwrap();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_load.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let (s, id) = (Arc::clone(&f.session), f.wallet);
        f.session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the open's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let (synced, payment) = (f.synced(), f.pay_from_the_draft(&draft));
    eprintln!(
        "ROW58 wallet opened mid-session: filter checkpoint {synced} before the resume, payment {:?}",
        payment.as_ref().map(|p| p.summary().inputs[0].outpoint)
    );
    assert!(replaying(payment), "a payment before the resume");
    pause.release.notify_one();
    f.engine.block_on(load).unwrap().unwrap();
    assert!(f.synced() < 400);
    assert!(replaying(f.pay_from_the_draft(&draft)));
}

/// A stored replay this build cannot read (another format, or a start
/// above its target), or a read that fails, holds spends until a rescan
/// from the birth height.
#[test]
fn an_unreadable_replay_record_holds_spends_until_a_rescan_from_the_birth_height() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 500))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    (f.session.live().unwrap().appdb)
        .set_setting(&f.wallet.to_string(), "replay", Some("1:1000:1001:0"))
        .unwrap();
    f.crash_on_close();
    let f = f.reopen();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!("ROW58 unreadable record: filter checkpoint {synced}, replay guard {guard:?}");
    assert_eq!(synced, 1_000);
    assert!(interrupted(&guard), "{guard:?}");
    f.rescan(500).unwrap();
    assert!(
        replaying(f.guard()),
        "a rescan from the birth height ends the hold"
    );

    f.advance(1_001);
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    let records = f.records();
    records
        .fail_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    f.engine.block_on(f.session.load_wallet(f.wallet)).unwrap();
    let guard = f.guard();
    eprintln!("ROW58 record read failed at open: replay guard {guard:?}");
    assert!(interrupted(&guard), "{guard:?}");
    records
        .fail_reads
        .store(false, std::sync::atomic::Ordering::SeqCst);
    f.rescan(500).unwrap();
    assert!(
        replaying(f.guard()),
        "a rescan from the birth height ends the hold"
    );
}

/// The abandon's replay record is stored before the abandon: when it
/// cannot be, nothing is abandoned.
#[test]
fn an_abandon_whose_replay_is_not_stored_does_not_happen() {
    use key_wallet::transaction_checking::{
        BlockInfo, TransactionContext, WalletTransactionChecker,
    };
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    let funding = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: Vec::new(),
        output: Vec::new(),
        special_transaction_payload: None,
    };
    f.session.hub.history.with_wallet(f.wallet, |h| {
        h.txs.insert(
            coin.txid,
            crate::history::TxEntry {
                tx: funding,
                context: TransactionContext::InBlock(BlockInfo::new(
                    150,
                    dashcore::BlockHash::all_zeros(),
                    9,
                )),
                instant_locked: false,
                first_seen: None,
                own_inputs: Default::default(),
                abandoned: false,
            },
        )
    });
    let spend = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: coin,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: COIN / 2,
            script_pubkey: script_of(FOREIGN),
        }],
        special_transaction_payload: None,
    };
    let txid = spend.txid();
    f.session.hub.history.with_wallet(f.wallet, |h| {
        h.txs.insert(
            txid,
            crate::history::TxEntry {
                tx: spend.clone(),
                context: TransactionContext::Mempool,
                instant_locked: false,
                first_seen: None,
                own_inputs: Default::default(),
                abandoned: false,
            },
        )
    });
    let manager = Arc::clone(&f.session.live().unwrap().manager);
    f.engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm.get_wallet_and_info_mut(&f.wallet.0).unwrap();
        let mut wallet = wallet.clone();
        info.core_wallet
            .check_core_transaction(&spend, TransactionContext::Mempool, &mut wallet, true, true)
            .await;
    });
    let records = f.records();
    records
        .fail_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let abandon = {
        let (s, id, m) = (Arc::clone(&f.session), f.wallet, Arc::clone(&manager));
        f.engine.block_on(
            f.session
                .on_runtime(async move { s.abandon_inner(&m, id, txid).await }),
        )
    };
    records
        .fail_writes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let abandoned = (f.session.hub.history)
        .get(&f.wallet, &txid)
        .is_some_and(|e| e.abandoned);
    let held = f.engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = &wm.get_wallet_info(&f.wallet.0).unwrap().core_wallet;
        (info.transaction_history().iter()).any(|r| r.txid == txid)
    });
    eprintln!(
        "ROW58 abandon not recorded: {abandon:?}, abandoned {abandoned}, key-wallet holds it {held}"
    );
    assert!(abandon.is_err());
    assert!(!abandoned, "abandoned without its replay record");
    assert!(held, "key-wallet dropped it");
}

/// Seams for forks, transactions the wallet sees, and the record store.
impl Fixture {
    /// A fork truncated the chain to `height`: the wallet lowers its
    /// heights to it, and the hub hears of it.
    fn truncate(&self, height: u32) {
        self.truncate_silently(height);
        self.hear_truncation(height);
    }

    /// The hub hears the `ChainTruncated` event of a fork to `height`.
    fn hear_truncation(&self, height: u32) {
        (self.session.hub).apply_wallet_event(
            &platform_wallet::events::WalletEvent::ChainTruncated {
                wallet_id: self.wallet.0,
                height,
                txids: Vec::new(),
                unconfirmed_txids: Vec::new(),
                restored_outpoints: Vec::new(),
                balance: key_wallet::wallet::balance::WalletCoreBalance::new(COIN, 0, 0, 0),
                account_balances: Default::default(),
            },
        );
    }

    /// A fork truncated the chain to `height` with no event the hub hears:
    /// dash-spv truncates with its event monitor stopped
    /// (lifecycle.rs:263-287 at 8e7ad2c1). key-wallet's `truncate_above`,
    /// under the manager's lock.
    fn truncate_silently(&self, height: u32) {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let manager = Arc::clone(&self.session.live().unwrap().manager);
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            wm.write().await.truncate_above(height).unwrap();
        });
    }

    fn cancel(&self) {
        assert!(self.engine.block_on(self.session.cancel_rescan()).unwrap());
    }

    /// The history holds `txid`'s transaction as mined at `height`.
    fn funded_at(&self, txid: Txid, height: u32) {
        use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
        self.funded_in(
            txid,
            TransactionContext::InBlock(BlockInfo::new(
                height,
                dashcore::BlockHash::all_zeros(),
                9,
            )),
        );
    }

    /// The history holds `txid`'s transaction, not in a block.
    fn funded_at_unmined(&self, txid: Txid) {
        self.funded_in(
            txid,
            key_wallet::transaction_checking::TransactionContext::Mempool,
        );
    }

    fn funded_in(&self, txid: Txid, context: key_wallet::transaction_checking::TransactionContext) {
        let funding = dashcore::Transaction {
            version: 2,
            lock_time: 0,
            input: Vec::new(),
            output: Vec::new(),
            special_transaction_payload: None,
        };
        self.session.hub.history.with_wallet(self.wallet, |h| {
            h.txs.insert(
                txid,
                crate::history::TxEntry {
                    tx: funding,
                    context,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs: Default::default(),
                    abandoned: false,
                },
            )
        });
    }

    /// The wallet sees `tx` unconfirmed: key-wallet records it and the
    /// history holds it.
    fn see_tx(&self, tx: &dashcore::Transaction) {
        use key_wallet::transaction_checking::{TransactionContext, WalletTransactionChecker};
        self.session.hub.history.with_wallet(self.wallet, |h| {
            h.txs.insert(
                tx.txid(),
                crate::history::TxEntry {
                    tx: tx.clone(),
                    context: TransactionContext::Mempool,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs: Default::default(),
                    abandoned: false,
                },
            )
        });
        let (manager, id, tx) = (
            Arc::clone(&self.session.live().unwrap().manager),
            self.wallet,
            tx.clone(),
        );
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let mut wallet = wallet.clone();
            info.core_wallet
                .check_core_transaction(&tx, TransactionContext::Mempool, &mut wallet, true, true)
                .await;
        });
    }

    /// Whether the history holds `txid` as abandoned, and whether key-wallet
    /// still holds it.
    fn abandon_state(&self, txid: Txid) -> (bool, bool) {
        use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
        let abandoned = (self.session.hub.history)
            .get(&self.wallet, &txid)
            .is_some_and(|e| e.abandoned);
        let manager = Arc::clone(&self.session.live().unwrap().manager);
        let held = self.engine.block_on(async {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = &wm.get_wallet_info(&self.wallet.0).unwrap().core_wallet;
            (info.transaction_history().iter()).any(|r| r.txid == txid)
        });
        (abandoned, held)
    }

    /// The store holds `value` as the wallet's replay record, as a session
    /// opening now reads it.
    fn store_record(&self, value: &str) {
        (self.session.live().unwrap().appdb)
            .set_setting(&self.wallet.to_string(), "replay", Some(value))
            .unwrap();
        let (replay, id) = (self.records(), self.wallet);
        self.engine
            .block_on(self.session.on_runtime(async move {
                replay.forget(id).await;
                Ok(())
            }))
            .unwrap();
    }
}

/// A fork lowers the running rescan's target with the record's: the
/// rescan is back at the shorter chain's tip; the record ends once the
/// chain is stored at the old height.
#[test]
fn a_rescan_a_fork_shortened_is_back_at_the_new_tip() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.truncate(900);
    f.advance(900);
    let (progress, guard) = (f.session.rescan_progress().unwrap(), f.guard());
    eprintln!(
        "ROW58 fork during a rescan: rescan progress at the new tip 900 {progress:?}, replay guard {guard:?}"
    );
    assert!(
        progress.is_none(),
        "the rescan waits for the old chain's height"
    );
    assert_eq!(guard.unwrap(), 900);
    quit_below_then_past_the_old_height(f, 399);
}

/// After a fork, a cancel leaves the checkpoint at the shorter chain's
/// tip, and a rescan from the birth height then replays to that tip; its
/// record ends once the chain is stored at the old height.
#[test]
fn a_fork_then_a_cancel_and_a_rescan_from_the_birth_height_complete_at_the_shorter_tip() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 300))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.truncate(900);
    f.advance(600);
    f.cancel();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!("ROW58 fork then cancel: filter checkpoint {synced}, replay guard {guard:?}");
    assert_eq!(
        synced, 900,
        "the cancel left the checkpoint above the chain"
    );
    assert!(interrupted(&guard), "{guard:?}");
    f.rescan(300).unwrap();
    assert!(replaying(f.guard()));
    f.advance(900);
    let (guard, record) = (f.guard(), f.record());
    eprintln!(
        "ROW58 fork, cancel, rescan from the birth height: replay guard at the new tip 900 {guard:?}, record {record:?}"
    );
    assert_eq!(guard.unwrap(), 900, "the target went back above the chain");
    quit_below_then_past_the_old_height(f, 299);
}

/// The same after a restart: the stored checkpoint stays the old chain's,
/// above the shorter tip, and the resumed replay is still done at the tip.
#[test]
fn a_fork_then_a_cancel_and_a_rescan_from_the_birth_height_complete_at_the_shorter_tip_after_a_restart()
 {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 300))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.truncate(900);
    f.advance(600);
    f.cancel();
    f.rescan(300).unwrap();
    f.crash_on_close();
    let f = f.reopen();
    assert!(f.synced() < 300, "resumed at {}", f.synced() + 1);
    assert!(replaying(f.guard()));
    f.advance(900);
    let (guard, record) = (f.guard(), f.record());
    eprintln!(
        "ROW58 fork, cancel, rescan from the birth height, restart: replay guard at the new tip 900 \
         {guard:?}, record {record:?}"
    );
    assert_eq!(guard.unwrap(), 900, "the target went back above the chain");
    quit_below_then_past_the_old_height(f, 299);
}

/// A stored checkpoint above the target of a record no fork lowered is
/// the replay's own (a quit after its last commit, before the record
/// went): the resumed replay runs to it, and a coin it credits again pays
/// nothing before its spend there replays.
#[test]
fn a_resumed_replay_runs_to_a_stored_checkpoint_above_its_target() {
    use platform_wallet::changeset::CoreChangeSet;
    let f = fixture(false);
    let coin = f.late_coin(7, COIN, 500);
    f.put_coin(coin.clone());
    f.store_core(CoreChangeSet {
        new_utxos: vec![coin.clone()],
        synced_height: Some(1_000),
        ..Default::default()
    });
    f.advance(1_000);
    f.rescan(400).unwrap();
    f.advance(1_010);
    f.see_spent(coin.outpoint);
    f.store_core(CoreChangeSet {
        spent_utxos: vec![coin.clone()],
        synced_height: Some(1_010),
        ..Default::default()
    });
    f.settle();
    f.store_record("1:1000:399:0");
    f.crash_on_close();
    let f = f.reopen();
    assert!(f.synced() < 400);
    f.put_coin(coin.clone());
    f.advance(1_000);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let (guard, payment) = (f.guard(), f.pay_from_the_draft(&draft));
    eprintln!(
        "ROW58 stored checkpoint 1010 above the target 1000: replay guard at 1000 {guard:?}, payment {:?}",
        payment.as_ref().map(|p| p.summary().inputs[0].outpoint)
    );
    assert!(replaying(guard), "done below the stored checkpoint");
    assert!(replaying(payment), "the coin spent at 1005 pays");
    f.see_spent(coin.outpoint);
    f.advance(1_010);
    assert_eq!(f.guard().unwrap(), 1_010);
    assert!(!replaying(f.pay_from_the_draft(&draft)));
}

/// An unreadable record whose hold cannot be raised to the birth height
/// at open holds there all the same: once writes work again, a rescan
/// from the birth height ends the hold, in memory and stored.
#[test]
fn an_unreadable_record_whose_raised_hold_is_not_stored_ends_with_a_rescan_from_the_birth_height() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 500))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    f.store_record("1:1000:1001:0");
    let records = f.records();
    records
        .fail_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    f.engine.block_on(f.session.load_wallet(f.wallet)).unwrap();
    records
        .fail_writes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    drop(records);
    assert!(interrupted(&f.guard()));
    f.rescan(500).unwrap();
    let (guard, record) = (f.guard(), f.record());
    eprintln!(
        "ROW58 hold not raised at open, then a rescan from the birth height: replay guard {guard:?}, \
         record {record:?}"
    );
    assert!(replaying(guard), "the hold stays");
    assert!(
        record.is_some_and(|r| !r.cancelled),
        "the stored hold stays"
    );
    f.crash_on_close();
    let f = f.reopen();
    assert!(replaying(f.guard()));
    assert!(f.record().is_some_and(|r| !r.cancelled));
}

/// How the hub holds the funding of the descendant's older coin.
enum OlderFunding {
    Mined,
    Withheld,
    Unmined,
}

/// A wallet born at 50 with root R (spending a coin funded at 150) seen,
/// and its descendant D (spending R's change and a coin funded at 100)
/// not yet: D's older funding as `older` says.
fn abandon_fixture(older: OlderFunding) -> (Fixture, Txid, dashcore::Transaction) {
    let f = fixture(false);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 50))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    let (a, b) = (f.late_coin(1, COIN, 150), f.late_coin(2, COIN, 100));
    f.put_coin(a.clone());
    f.put_coin(b.clone());
    f.funded_at(a.outpoint.txid, 150);
    match older {
        OlderFunding::Mined => f.funded_at(b.outpoint.txid, 100),
        OlderFunding::Withheld => {}
        OlderFunding::Unmined => f.funded_at_unmined(b.outpoint.txid),
    }
    let root = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: a.outpoint,
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: COIN / 4,
                script_pubkey: script_of(FOREIGN),
            },
            TxOut {
                value: COIN / 2,
                script_pubkey: a.txout.script_pubkey.clone(),
            },
        ],
        special_transaction_payload: None,
    };
    f.see_tx(&root);
    let descendant = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: vec![
            dashcore::TxIn {
                previous_output: OutPoint::new(root.txid(), 1),
                ..Default::default()
            },
            dashcore::TxIn {
                previous_output: b.outpoint,
                ..Default::default()
            },
        ],
        output: vec![TxOut {
            value: COIN,
            script_pubkey: script_of(FOREIGN),
        }],
        special_transaction_payload: None,
    };
    let root_id = root.txid();
    (f, root_id, descendant)
}

fn abandon(
    f: &Fixture,
    root: Txid,
) -> tokio::task::JoinHandle<Result<std::collections::BTreeSet<Txid>, EngineError>> {
    let (s, id, m) = (
        Arc::clone(&f.session),
        f.wallet,
        Arc::clone(&f.session.live().unwrap().manager),
    );
    f.session
        .rt
        .spawn(async move { s.abandon_inner(&m, id, root).await })
}

/// The abandon is planned under the lock that applies it: a descendant
/// that arrives first, spending also a coin funded below the root's, is
/// abandoned with it once the replay from that lower block is stored, and
/// not at all when that cannot be.
#[test]
fn an_abandon_stores_the_replay_of_a_descendant_that_arrives_before_it_applies() {
    let (f, root_id, descendant) = abandon_fixture(OlderFunding::Mined);
    let descendant_id = descendant.txid();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_abandon.lock().unwrap() = Some(Arc::clone(&pause));
    let first = abandon(&f, root_id);
    eventually("the abandon's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    f.see_tx(&descendant);
    let records = f.records();
    records
        .fail_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    pause.release.notify_one();
    let first = f.engine.block_on(first).unwrap();
    records
        .fail_writes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    drop(records);
    let (root_state, descendant_state) = (f.abandon_state(root_id), f.abandon_state(descendant_id));
    eprintln!(
        "ROW58 descendant before the abandon applies, its replay not stored: {first:?}, root \
         (abandoned, key-wallet holds it) {root_state:?}, descendant {descendant_state:?}"
    );
    assert!(first.is_err());
    assert_eq!(
        root_state,
        (false, true),
        "the root went without its replay"
    );
    assert_eq!(
        descendant_state,
        (false, true),
        "the descendant went without its replay"
    );

    let second = f.engine.block_on(abandon(&f, root_id)).unwrap().unwrap();
    assert_eq!(second, [root_id, descendant_id].into_iter().collect());
    assert_eq!(f.record().map(|r| r.reached), Some(99));
    assert_eq!(f.abandon_state(descendant_id), (true, false));
}

/// A released coin whose funding the hub does not hold, or holds without
/// a block, is replayed from the birth height.
#[test]
fn an_abandon_releasing_a_coin_of_unknown_funding_height_replays_from_the_birth_height() {
    for older in [OlderFunding::Withheld, OlderFunding::Unmined] {
        let (f, root_id, descendant) = abandon_fixture(older);
        f.see_tx(&descendant);
        let abandoned = f.engine.block_on(abandon(&f, root_id)).unwrap().unwrap();
        let record = f.record();
        eprintln!(
            "ROW58 abandon of a coin of unknown funding height: {abandoned:?}, record {record:?}"
        );
        assert_eq!(abandoned.len(), 2);
        assert_eq!(
            record.map(|r| r.reached),
            Some(49),
            "replayed from above the birth height"
        );
    }
}

/// A wallet born at 50, scanned and stored to 1000.
fn scanned_to_1000() -> Fixture {
    let f = fixture(false);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 50))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f
}

/// A coin of `value` to a fresh receive address, funded by a transaction
/// of its own mined at `height`, which the history holds; the coin is not
/// in key-wallet's unspent set.
fn funded_coin(f: &Fixture, seed: u8, value: u64, height: u32) -> (Utxo, dashcore::Transaction) {
    let mut coin = f.late_coin(seed, value, height);
    let funding = dashcore::Transaction {
        version: 2,
        lock_time: u32::from(seed),
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
            ..Default::default()
        }],
        output: vec![coin.txout.clone()],
        special_transaction_payload: None,
    };
    coin.outpoint = OutPoint::new(funding.txid(), 0);
    f.funded_at(funding.txid(), height);
    (coin, funding)
}

/// An unconfirmed transaction spending `inputs` to a foreign address.
fn unconfirmed_spend(seed: u8, inputs: &[OutPoint]) -> dashcore::Transaction {
    dashcore::Transaction {
        version: 2,
        lock_time: u32::from(seed),
        input: (inputs.iter())
            .map(|o| dashcore::TxIn {
                previous_output: *o,
                ..Default::default()
            })
            .collect(),
        output: vec![TxOut {
            value: COIN / 10,
            script_pubkey: script_of(FOREIGN),
        }],
        special_transaction_payload: None,
    }
}

impl Fixture {
    /// The state a reload leaves for `tx`, recorded unconfirmed by an earlier
    /// session: the history holds it with `own` (input index, coin) as the
    /// wallet's inputs, key-wallet holds no record of it, and each of those
    /// coins is spent with a claim naming it.
    fn recorded_by_an_earlier_session(&self, tx: &dashcore::Transaction, own: &[(u32, &Utxo)]) {
        self.held_with_claims(tx, own);
        assert_eq!(self.abandon_state(tx.txid()), (false, false));
    }

    /// The history holds `tx` with `own` (input index, coin) as the wallet's
    /// inputs, as its stored record names them, and each of those coins is
    /// spent with a claim naming it.
    fn held_with_claims(&self, tx: &dashcore::Transaction, own: &[(u32, &Utxo)]) {
        let txid = tx.txid();
        self.session.hub.history.with_wallet(self.wallet, |h| {
            h.txs.insert(
                txid,
                crate::history::TxEntry {
                    tx: tx.clone(),
                    context: key_wallet::transaction_checking::TransactionContext::Mempool,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs: (own.iter())
                        .map(|(i, c)| (*i, (c.txout.value, c.address.clone())))
                        .collect(),
                    abandoned: false,
                },
            )
        });
        let claims: Vec<(OutPoint, Option<Txid>)> =
            own.iter().map(|(_, c)| (c.outpoint, Some(txid))).collect();
        let (session, id) = (Arc::clone(&self.session), self.wallet);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                let mut state = wallet.state_mut().await;
                state.core_wallet.restore_spent_outpoints(&claims);
                Ok(())
            }))
            .unwrap();
    }

    /// The input indexes key-wallet's records of `txid` name as the wallet's.
    fn recorded_own_inputs(&self, txid: Txid) -> Option<Vec<u32>> {
        let manager = Arc::clone(&self.session.live().unwrap().manager);
        self.engine.block_on(async {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = &wm.get_wallet_info(&self.wallet.0).unwrap().core_wallet;
            let records: Vec<_> = (info.accounts.all_accounts().into_iter())
                .filter_map(|a| a.transactions().get(&txid))
                .collect();
            (!records.is_empty()).then(|| {
                (records.iter())
                    .flat_map(|r| r.input_details.iter().map(|i| i.index))
                    .collect()
            })
        })
    }

    /// The filter scan delivers `tx` in the block at `height`.
    fn deliver_in_block(&self, tx: &dashcore::Transaction, height: u32) {
        use key_wallet::transaction_checking::{
            BlockInfo, TransactionContext, WalletTransactionChecker,
        };
        let (manager, id, tx) = (
            Arc::clone(&self.session.live().unwrap().manager),
            self.wallet,
            tx.clone(),
        );
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let mut wallet = wallet.clone();
            let block = BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 9);
            info.core_wallet
                .check_core_transaction(
                    &tx,
                    TransactionContext::InBlock(block),
                    &mut wallet,
                    true,
                    true,
                )
                .await;
        });
    }

    fn confirmed_balance(&self) -> u64 {
        (self.session.balances(&self.wallet).unwrap()).map_or(0, |b| b.confirmed)
    }
}

/// A send an earlier session recorded, abandoned after a reload: key-wallet
/// holds no record of it, so the coin it released comes from the history's
/// record of it. The scan replays from below the coin's funding block, and
/// once the replay passes it the coin is back in the coin list and the
/// balance.
#[test]
fn an_abandon_of_a_send_key_wallet_holds_no_record_of_replays_its_coins_funding_block() {
    let f = scanned_to_1000();
    let (coin, funding) = funded_coin(&f, 1, COIN, 150);
    let send = unconfirmed_spend(7, &[coin.outpoint]);
    f.recorded_by_an_earlier_session(&send, &[(0, &coin)]);
    let before = (f.offers(coin.outpoint), f.confirmed_balance());

    let abandoned = f
        .engine
        .block_on(abandon(&f, send.txid()))
        .unwrap()
        .unwrap();
    let (synced, record) = (f.synced(), f.record());
    f.deliver_in_block(&funding, 150);
    f.advance(1_000);
    let after = (f.offers(coin.outpoint), f.confirmed_balance());
    eprintln!(
        "ROW68 abandon of a send with no key-wallet record: {abandoned:?}, checkpoint {synced}, \
         record {record:?}, coin listed and confirmed balance before {before:?}, after the replay \
         {after:?}"
    );
    assert_eq!(abandoned, [send.txid()].into_iter().collect());
    assert_eq!(synced, 149, "replayed from below the coin's funding block");
    assert_eq!(record.map(|r| r.reached), Some(149));
    assert!(!before.0);
    assert!(after.0, "the coin is back in the coin list");
    assert_eq!(after.1, before.1 + COIN, "and in the balance");
}

/// A send key-wallet holds a record of that names none of the wallet's
/// inputs (it saw the send before the coin), while the history's record
/// names the coin funded at 150: the abandon replays from below that coin's
/// funding block.
#[test]
fn an_abandon_replays_a_coin_only_the_historys_record_names() {
    let f = scanned_to_1000();
    let (coin, _) = funded_coin(&f, 4, COIN, 150);
    let mut send = unconfirmed_spend(10, &[coin.outpoint]);
    send.output.push(TxOut {
        value: COIN / 2,
        script_pubkey: coin.txout.script_pubkey.clone(),
    });
    f.see_tx(&send);
    f.held_with_claims(&send, &[(0, &coin)]);
    let recorded = f.recorded_own_inputs(send.txid());

    let abandoned = f
        .engine
        .block_on(abandon(&f, send.txid()))
        .unwrap()
        .unwrap();
    let (synced, record) = (f.synced(), f.record());
    eprintln!(
        "ROW68 abandon of a send whose key-wallet record names inputs {recorded:?}: \
         {abandoned:?}, checkpoint {synced}, record {record:?}"
    );
    assert_eq!(
        recorded,
        Some(Vec::new()),
        "key-wallet's record names no coin"
    );
    assert_eq!(abandoned, [send.txid()].into_iter().collect());
    assert_eq!(synced, 149, "replayed from below the coin's funding block");
    assert_eq!(record.map(|r| r.reached), Some(149));
}

/// An incoming payment an earlier session recorded unconfirmed, abandoned
/// after a reload: it spends none of the wallet's coins, so nothing is
/// replayed, though the history knows none of its inputs' funding.
#[test]
fn an_abandon_of_an_incoming_payment_key_wallet_holds_no_record_of_replays_nothing() {
    let f = scanned_to_1000();
    let foreign = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
    let (to_us, _) = funded_coin(&f, 2, COIN, 150);
    let mut payment = unconfirmed_spend(8, &[foreign]);
    payment.output[0].script_pubkey = to_us.txout.script_pubkey.clone();
    f.recorded_by_an_earlier_session(&payment, &[]);

    let abandoned = f
        .engine
        .block_on(abandon(&f, payment.txid()))
        .unwrap()
        .unwrap();
    let (synced, record) = (f.synced(), f.record());
    eprintln!(
        "ROW68 abandon of an incoming payment with no key-wallet record: {abandoned:?}, \
         checkpoint {synced}, record {record:?}"
    );
    assert_eq!(abandoned, [payment.txid()].into_iter().collect());
    assert_eq!(synced, 1_000, "no replay");
    assert_eq!(record, None);
}

/// A mixing transaction an earlier session recorded unconfirmed, abandoned
/// after a reload: of its inputs only the wallet's coin (funded at 150) is
/// replayed, not the other participants' (their funding unknown, which would
/// replay from the birth height).
#[test]
fn an_abandon_of_a_mixing_transaction_key_wallet_holds_no_record_of_replays_only_its_own_coin() {
    let f = scanned_to_1000();
    let (coin, _) = funded_coin(&f, 3, COIN, 150);
    let theirs = [
        OutPoint::new(Txid::from_byte_array([10; 32]), 0),
        OutPoint::new(Txid::from_byte_array([11; 32]), 1),
    ];
    let mix = unconfirmed_spend(9, &[theirs[0], coin.outpoint, theirs[1]]);
    f.recorded_by_an_earlier_session(&mix, &[(1, &coin)]);

    let abandoned = f.engine.block_on(abandon(&f, mix.txid())).unwrap().unwrap();
    let (synced, record) = (f.synced(), f.record());
    eprintln!(
        "ROW68 abandon of a mixing transaction with no key-wallet record: {abandoned:?}, \
         checkpoint {synced}, record {record:?}"
    );
    assert_eq!(abandoned, [mix.txid()].into_iter().collect());
    assert_eq!(synced, 149, "replayed from below the wallet's coin only");
    assert_eq!(record.map(|r| r.reached), Some(149));
}

/// A replay whose target a fork lowered, from a checkpoint of 6000 to
/// 5000, then the new chain grows to 5050 and a coin funded at 150 is
/// spent at 5025, committed; after a crash (and, with `second_fork`, a
/// fork back to 5040 first) the resumed replay refuses the coin until its
/// spend replays.
fn a_fork_then_canonical_progress_survives_a_crash(second_fork: Option<u32>) {
    use platform_wallet::changeset::CoreChangeSet;
    let f = fixture(false);
    let coin = f.late_coin(7, COIN, 150);
    f.put_coin(coin.clone());
    f.store_core(CoreChangeSet {
        new_utxos: vec![coin.clone()],
        synced_height: Some(6_000),
        ..Default::default()
    });
    f.advance(6_000);
    f.rescan(100).unwrap();
    f.truncate(5_000);
    f.advance(5_000);
    f.see_spent(coin.outpoint);
    f.store_core(CoreChangeSet {
        spent_utxos: vec![coin.clone()],
        synced_height: Some(5_025),
        ..Default::default()
    });
    f.advance(5_050);
    f.commit(5_050).unwrap();
    if let Some(height) = second_fork {
        f.truncate(height);
    }
    let before = f.record();
    f.crash_on_close();
    let f = f.reopen();
    assert!(f.synced() < 100);
    f.put_coin(coin.clone());
    f.advance(5_000);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let (guard, payment) = (f.guard(), f.pay_from_the_draft(&draft));
    eprintln!(
        "ROW58 fork, growth, spend at 5025, second fork {second_fork:?}, crash: record {before:?}, \
         replay guard at 5000 {guard:?}, payment {:?}",
        payment.as_ref().map(|p| p.summary().inputs[0].outpoint)
    );
    assert!(replaying(guard), "done below the new chain's spend");
    assert!(replaying(payment), "the coin spent at 5025 pays");
    f.see_spent(coin.outpoint);
    f.advance(5_050);
    assert!(!replaying(f.pay_from_the_draft(&draft)));
}

#[test]
fn a_fork_lowered_replay_keeps_the_new_chains_progress_across_a_crash() {
    a_fork_then_canonical_progress_survives_a_crash(None);
}

#[test]
fn a_second_fork_keeps_the_new_chains_progress_below_it_across_a_crash() {
    a_fork_then_canonical_progress_survives_a_crash(Some(5_040));
}

/// ROW73 (pasta's 11-08 payment): a normal payment of everything with
/// Subtract fee, from a wallet that also holds a coin still being mixed and
/// a fully mixed one.
mod normal_payment_beside_mixing {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use dashcore::hashes::Hash;
    use dashcore::{Address, OutPoint, Transaction, TxIn, TxOut, Txid};
    use dw_vault::{Credential, GrantPurpose};
    use key_wallet::Utxo;
    use key_wallet::transaction_checking::TransactionContext;

    use super::{FOREIGN, send_failure};
    use crate::EngineError;
    use crate::history::TxEntry;
    use crate::platform::lease::session_tests::{Fixture, fixture};
    use crate::send::{CoinSource, FeeMode, Recipient, SendFailure};

    /// The unmixed coin: 215.74478465 DASH.
    const UNMIXED: u64 = 21_574_478_465;
    const MIXING: u64 = 100_001_000;
    const MIXED: u64 = 10_000_100;
    const RECOMMENDED: FeeMode = FeeMode::Recommended { target_blocks: 6 };
    /// One P2PKH input, one P2PKH output, no change, at 1000 duff/kB.
    const FEE: u64 = 192;

    /// A denomination `rounds` mixing transactions deep, as the wallet holds
    /// it: the chain in the history, the last output in the CoinJoin account.
    fn coinjoin_coin(f: &Fixture, seed: u8, value: u64, rounds: u32) -> OutPoint {
        let session = Arc::clone(&f.session);
        let id = f.wallet;
        let address: Address = f
            .engine
            .block_on(session.on_runtime({
                let session = Arc::clone(&session);
                async move {
                    let mut a = session.reserve_coinjoin_addresses(id, 1, false).await?;
                    Ok::<_, EngineError>(a.remove(0))
                }
            }))
            .unwrap();
        let pay = |value| TxOut {
            value,
            script_pubkey: address.script_pubkey(),
        };
        let spend = |previous_output| TxIn {
            previous_output,
            ..Default::default()
        };
        // Created beside a non-denominated output: round 0; each spend of it
        // to itself, in full, adds one.
        let mut tx = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![spend(OutPoint::new(Txid::from_byte_array([seed; 32]), 0))],
            output: vec![pay(value), pay(1_234_567)],
            special_transaction_payload: None,
        };
        let mut chain = vec![(tx.clone(), BTreeMap::new())];
        for _ in 0..rounds {
            tx = Transaction {
                input: vec![spend(OutPoint::new(tx.txid(), 0))],
                output: vec![pay(value)],
                ..tx
            };
            chain.push((tx.clone(), BTreeMap::from([(0, (value, address.clone()))])));
        }
        let outpoint = OutPoint::new(tx.txid(), 0);
        (f.session.hub.history).with_wallet(f.wallet, |h| {
            for (tx, own_inputs) in chain {
                let entry = TxEntry {
                    tx,
                    context: TransactionContext::Mempool,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs,
                    abandoned: false,
                };
                h.txs.insert(entry.tx.txid(), entry);
            }
        });
        let mut utxo = Utxo::new(outpoint, pay(value), address.clone(), 1, false);
        utxo.is_confirmed = true;
        f.engine
            .block_on(session.on_runtime({
                let session = Arc::clone(&session);
                async move {
                    let wallet = session.wallet(&id).await?;
                    (wallet
                        .state_mut()
                        .await
                        .core_wallet
                        .accounts
                        .coinjoin_accounts)
                        .get_mut(&0)
                        .expect("CoinJoin account 0")
                        .utxos
                        .insert(outpoint, utxo);
                    Ok::<_, EngineError>(())
                }
            }))
            .unwrap();
        outpoint
    }

    fn max(f: &Fixture, source: CoinSource) -> u64 {
        f.engine
            .block_on(f.session.max_spendable(f.wallet, source, RECOMMENDED))
            .unwrap()
    }

    fn draft(f: &Fixture, amount: u64) -> Arc<crate::send::TxDraft> {
        let d = f.session.new_tx_draft(f.wallet).unwrap();
        d.set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount,
            subtract_fee_from_amount: true,
            label: None,
            message: None,
        }])
        .unwrap();
        d.set_source(CoinSource::Any).unwrap();
        d.set_fee(RECOMMENDED).unwrap();
        d
    }

    #[test]
    fn a_full_balance_subtract_fee_payment_spends_only_the_unmixed_coin() {
        let f = fixture(false);
        let unmixed = f.credit(1, UNMIXED);
        coinjoin_coin(&f, 2, MIXING, 2);
        coinjoin_coin(&f, 3, MIXED, 8);
        // The mixing view tells the two apart: one fully mixed, one not yet.
        assert_eq!(max(&f, CoinSource::FullyMixedOnly), MIXED);
        // A normal payment counts the unmixed coin alone.
        assert_eq!(max(&f, CoinSource::Any), UNMIXED);

        // Part of it keeps its change: no change becomes fee (LIST row 34).
        let half = f
            .engine
            .block_on(draft(&f, UNMIXED / 2).estimate())
            .unwrap();
        assert_eq!(
            (half.input_count, half.change, half.fee, half.total_sent),
            (1, Some(UNMIXED - UNMIXED / 2), 226, UNMIXED / 2 - 226)
        );

        let all = draft(&f, UNMIXED);
        let e = f.engine.block_on(all.estimate()).unwrap();
        assert_eq!(
            (e.input_count, e.change, e.size_bytes, e.fee, e.total_sent),
            (1, None, 192, FEE, UNMIXED - FEE)
        );
        let grant = f.grant(GrantPurpose::Spend { max_duffs: UNMIXED }, Credential::None);
        let prepared = f.engine.block_on(all.prepare(grant)).unwrap();
        let s = prepared.summary().clone();
        assert_eq!(
            s.inputs.iter().map(|i| i.outpoint).collect::<Vec<_>>(),
            [unmixed]
        );
        assert_eq!(s.outputs.len(), 1, "{:?}", s.outputs);
        assert!(!s.outputs[0].is_change && !s.outputs[0].is_mine);
        assert_eq!(
            (s.outputs[0].amount, s.total_sent, s.fee, s.total_debit),
            (UNMIXED - FEE, UNMIXED - FEE, FEE, UNMIXED)
        );
        // The signed transaction pays that fee (inputs − outputs). The fee
        // budgets the largest signature (148-byte input), so the signed size
        // is at most the estimate and the rate it pays at least 1000 duff/kB.
        let tx: Transaction = dashcore::consensus::deserialize(&prepared.raw().unwrap()).unwrap();
        assert_eq!(
            tx.input
                .iter()
                .map(|i| i.previous_output)
                .collect::<Vec<_>>(),
            [unmixed]
        );
        let out: u64 = tx.output.iter().map(|o| o.value).sum();
        assert_eq!(UNMIXED - out, FEE);
        let size = dashcore::consensus::serialize(&tx).len() as u64;
        assert_eq!(size, u64::from(s.size_bytes));
        assert!((190..=192).contains(&size), "signed size {size}");
        assert_eq!(s.fee_rate_per_kb, FEE * 1000 / size);
        assert!(s.fee_rate_per_kb >= 1_000);

        // Normal funds read 0 once it is signed, and once the wallet sees it
        // spent; the same payment is then refused for them, which is what the
        // Pay form showed behind the receipt.
        assert_eq!(max(&f, CoinSource::Any), 0);
        f.spend_coin(unmixed);
        assert_eq!(max(&f, CoinSource::Any), 0);
        assert_eq!(
            send_failure(f.engine.block_on(all.estimate())),
            SendFailure::AmountExceedsBalance { available: 0 }
        );
        // The CoinJoin account's coins are where they were.
        assert_eq!(max(&f, CoinSource::FullyMixedOnly), MIXED);
    }
}

// ---- rows 59 and 65: a fork truncation no event announces -------------------

impl Fixture {
    /// The wallet's transaction `seed` spending `inputs`, paying no one.
    fn spend_of(&self, seed: u8, inputs: &[OutPoint]) -> dashcore::Transaction {
        dashcore::Transaction {
            version: 1,
            lock_time: u32::from(seed),
            input: (inputs.iter())
                .map(|o| dashcore::TxIn {
                    previous_output: *o,
                    ..Default::default()
                })
                .collect(),
            output: Vec::new(),
            special_transaction_payload: None,
        }
    }

    /// The wallet sees `tx` in `context`, and the hub hears key-wallet's
    /// event of it.
    fn see_in(
        &self,
        tx: &dashcore::Transaction,
        context: key_wallet::transaction_checking::TransactionContext,
    ) {
        let event = self.seen_event(tx, context);
        self.session.hub.apply_wallet_event(&event);
    }

    /// The wallet sees `tx` in `context`: key-wallet records it. Returns
    /// the event that carries its record, not heard yet.
    fn seen_event(
        &self,
        tx: &dashcore::Transaction,
        context: key_wallet::transaction_checking::TransactionContext,
    ) -> platform_wallet::events::WalletEvent {
        use key_wallet::transaction_checking::WalletTransactionChecker;
        use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
        let (manager, id, tx) = (
            Arc::clone(&self.session.live().unwrap().manager),
            self.wallet,
            tx.clone(),
        );
        let (record, balance, account_balances) = self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let mut wallet = wallet.clone();
            info.core_wallet
                .check_core_transaction(&tx, context, &mut wallet, true, true)
                .await;
            let record = (info.core_wallet.transaction_history().into_iter())
                .find(|r| r.txid == tx.txid())
                .cloned();
            (
                record,
                info.core_wallet.balance(),
                info.core_wallet.account_balances(),
            )
        });
        platform_wallet::events::WalletEvent::TransactionDetected {
            wallet_id: self.wallet.0,
            record: Box::new(record.expect("key-wallet records the transaction")),
            balance,
            account_balances,
            addresses_derived: Vec::new(),
        }
    }

    /// key-wallet applies a ChainLock at `height`.
    fn chain_lock_at(&self, height: u32) {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let manager = Arc::clone(&self.session.live().unwrap().manager);
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            wm.write().await.apply_chain_lock(dashcore::ChainLock {
                block_height: height,
                block_hash: dashcore::BlockHash::all_zeros(),
                signature: dashcore::bls_sig_utils::BLSSignature::from([0; 96]),
            });
        });
    }

    /// Spawns a wallet open, and waits for its pause `pause`.
    fn load_until(
        &self,
        pause: &Arc<crate::coins::TestPause>,
    ) -> tokio::task::JoinHandle<Result<(), EngineError>> {
        let load = {
            let (s, id) = (Arc::clone(&self.session), self.wallet);
            self.session
                .rt
                .spawn(async move { s.load_wallet(id).await })
        };
        eventually("the open's pause", || {
            pause.reached.load(std::sync::atomic::Ordering::SeqCst)
        });
        load
    }

    /// Key-wallet's checkpoint and processed height, and the hub's
    /// heights, at `height`, with no event: no reconcile is scheduled.
    fn quietly_at(&self, height: u32) {
        self.checkpoint(height);
        self.session.hub.update_wallet_state(self.wallet, |s| {
            s.synced_height = height;
            s.processed_height = height;
            s.scan_target = s.scan_target.max(height);
        });
    }

    /// Under one hold of the manager's lock, so nothing checks for the fork
    /// between: key-wallet sees `before`, a fork truncates it to `fork`,
    /// it sees `after` on the new chain, and its checkpoint advances to
    /// `to`.
    fn fork_and_progress(
        &self,
        before: &[(
            &dashcore::Transaction,
            key_wallet::transaction_checking::TransactionContext,
        )],
        fork: u32,
        after: &[(
            &dashcore::Transaction,
            key_wallet::transaction_checking::TransactionContext,
        )],
        to: u32,
    ) {
        use key_wallet::transaction_checking::WalletTransactionChecker;
        use platform_wallet::key_wallet_manager::WalletInterface;
        let (manager, id) = (
            Arc::clone(&self.session.live().unwrap().manager),
            self.wallet,
        );
        type Seen<'a> = (
            &'a dashcore::Transaction,
            key_wallet::transaction_checking::TransactionContext,
        );
        let owned = |l: &[Seen<'_>]| -> Vec<(
            dashcore::Transaction,
            key_wallet::transaction_checking::TransactionContext,
        )> { l.iter().map(|(t, c)| ((*t).clone(), c.clone())).collect() };
        let (before, after) = (owned(before), owned(after));
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            {
                let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
                let mut wallet = wallet.clone();
                for (tx, context) in before {
                    (info.core_wallet)
                        .check_core_transaction(&tx, context, &mut wallet, true, true)
                        .await;
                }
            }
            wm.truncate_above(fork).unwrap();
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let mut wallet = wallet.clone();
            for (tx, context) in after {
                (info.core_wallet)
                    .check_core_transaction(&tx, context, &mut wallet, true, true)
                    .await;
            }
            crate::events::set_checkpoint(&mut info.core_wallet, to);
        });
    }

    /// The hub's synced height, processed height and replay target.
    fn hub_heights(&self) -> (u32, u32, u32) {
        let s = self.session.hub.wallet_state(&self.wallet).unwrap();
        (s.synced_height, s.processed_height, s.scan_target)
    }

    /// The history's entry of `txid`: its block height, and whether it is
    /// held InstantSend-locked out of a block.
    fn entry_of(&self, txid: Txid) -> Option<(Option<u32>, bool)> {
        (self.session.hub.history.get(&self.wallet, &txid)).map(|e| {
            (
                e.context.block_info().map(|b| b.height()),
                e.instant_locked && e.context.block_info().is_none(),
            )
        })
    }

    /// Stores both of key-wallet's heights at `height`, as the persister
    /// does once a block there and its filter batch are stored.
    fn store_heights(&self, synced: u32, processed: u32) {
        self.store_core(platform_wallet::changeset::CoreChangeSet {
            synced_height: Some(synced),
            last_processed_height: Some(processed),
            ..Default::default()
        });
    }
}

/// A fork truncated the wallet to 900 from 1000 and announced nothing,
/// and the new chain is committed to 905: the replay target follows the
/// chain down, so a payment is admitted, and the hub's heights are the new
/// chain's.
#[test]
fn a_silent_truncation_leaves_no_replay_target_above_the_new_chain() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.truncate_silently(900);
    f.advance(905);
    f.commit(905).unwrap();
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let payment = f.pay_from_the_draft(&draft);
    let heights = f.hub_heights();
    eprintln!(
        "ROW5965 silent truncation to 900 from 1000, new chain at 905: payment {:?}, hub synced/processed/target {heights:?}",
        payment.as_ref().map(|p| p.summary().inputs[0].outpoint)
    );
    assert!(payment.is_ok(), "{payment:?}");
    assert!(
        heights.0 <= 905 && heights.1 <= 905 && heights.2 <= 905,
        "{heights:?}"
    );
}

/// The history follows a truncation no event announced: a spend mined in
/// a replaced block, which key-wallet dropped, is gone and its coin, which
/// key-wallet put back, is no longer reserved; an InstantSend-locked
/// transaction mined there reads unconfirmed.
#[test]
fn a_silent_truncation_drops_the_replaced_blocks_spends_from_the_history() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let mined = |height| {
        TransactionContext::InBlock(BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 9))
    };
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    let other = f.credit(2, COIN);
    let dropped = f.spend_of(7, &[coin]);
    let locked = f.spend_of(8, &[other]);
    f.see_in(&dropped, mined(950));
    f.see_in(
        &locked,
        TransactionContext::InstantSend(dashcore::InstantLock::default()),
    );
    f.see_in(&locked, mined(960));
    f.advance(1_000);
    f.truncate_silently(900);
    let guard = f.guard();
    let (gone, unconfirmed, reserved, offered) = (
        f.entry_of(dropped.txid()),
        f.entry_of(locked.txid()),
        f.reserved(),
        f.offers(coin),
    );
    eprintln!(
        "ROW5965 silent truncation to 900: guard {guard:?}, spend mined at 950 {gone:?}, IS-locked at 960 {unconfirmed:?}, reserved {reserved:?}, coin offered {offered}"
    );
    assert_eq!(gone, None, "the replaced spend stays");
    assert_eq!(unconfirmed, Some((None, true)), "still mined at 960");
    assert!(offered, "key-wallet did not put the coin back");
    assert!(!reserved.contains(&coin), "the coin stays reserved");
}

/// A check for a fork waits for one being applied: a replay guard read
/// while a fork, heard or found, has lowered the heights but not yet the
/// history sees the history without the replaced spend.
#[test]
fn a_guard_during_a_forks_application_sees_its_history() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    use std::sync::Barrier;
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    let dropped = f.spend_of(7, &[coin]);
    f.see_in(
        &dropped,
        TransactionContext::InBlock(BlockInfo::new(950, dashcore::BlockHash::all_zeros(), 9)),
    );
    f.advance(1_000);
    // Set before the fork: the first to apply it pauses, the event below or
    // a reconcile the advance scheduled.
    let (reached, release) = (Arc::new(Barrier::new(2)), Arc::new(Barrier::new(2)));
    *f.session.hub.pause_truncation.lock().unwrap() =
        Some((Arc::clone(&reached), Arc::clone(&release)));
    f.truncate_silently(900);
    let (hub, wallet, txid) = (Arc::clone(&f.session.hub), f.wallet, dropped.txid());
    let heard = std::thread::spawn(move || {
        hub.apply_wallet_event(&platform_wallet::events::WalletEvent::ChainTruncated {
            wallet_id: wallet.0,
            height: 900,
            txids: vec![txid],
            unconfirmed_txids: Vec::new(),
            restored_outpoints: Vec::new(),
            balance: key_wallet::wallet::balance::WalletCoreBalance::new(COIN, 0, 0, 0),
            account_balances: Default::default(),
        });
    });
    reached.wait();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        release.wait();
    });
    let guard = f.guard();
    let gone = f.entry_of(dropped.txid());
    heard.join().unwrap();
    releaser.join().unwrap();
    eprintln!(
        "ROW5965 a guard during a heard fork's application: guard {guard:?}, spend mined at 950 {gone:?}"
    );
    assert_eq!(guard.unwrap(), 900);
    assert_eq!(gone, None, "the guard read the history before the fork's");
}

/// Opus m1: a rescan from 400 to 1000, then a fork to 900 no event
/// announced, then a cancel: the checkpoint goes back no higher than the
/// new chain, and a rescan from the birth height ends at its tip.
#[test]
fn a_cancel_after_a_silent_truncation_leaves_the_checkpoint_on_the_new_chain() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 300))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.advance(600);
    f.truncate_silently(900);
    f.cancel();
    let (synced, guard) = (f.synced(), f.guard());
    eprintln!(
        "ROW5965 silent fork then cancel: filter checkpoint {synced}, replay guard {guard:?}"
    );
    assert!(synced <= 900, "the cancel left the checkpoint at {synced}");
    assert!(interrupted(&guard), "{guard:?}");
    f.rescan(300).unwrap();
    f.advance(900);
    let (progress, guard) = (f.session.rescan_progress().unwrap(), f.guard());
    eprintln!(
        "ROW5965 silent fork, cancel, rescan from the birth height at the new tip 900: progress {progress:?}, guard {guard:?}"
    );
    assert!(progress.is_none(), "the rescan waits for the old height");
    assert_eq!(guard.unwrap(), 900);
}

/// Opus m2 route 1: a fork no event announced, with no record, then a
/// rescan from the birth height: the record's target is the new chain's,
/// and after a quit mid-rescan the reopened wallet admits spends at the
/// new tip, though the stored checkpoint is still the old chain's.
#[test]
fn a_rescan_after_a_silent_truncation_ends_at_the_new_tip_across_a_restart() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 300))
        .unwrap();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.truncate_silently(900);
    f.rescan(300).unwrap();
    let record = f.record();
    eprintln!("ROW5965 silent fork, no record, rescan from the birth height: record {record:?}");
    assert!(record.is_some_and(|r| r.target <= 900), "{record:?}");
    f.advance(600);
    let f = f.reopen();
    f.advance(900);
    let (guard, record) = (f.guard(), f.record());
    eprintln!(
        "ROW5965 silent fork, rescan, quit at 600, reopen: replay guard at the new tip 900 {guard:?}, record {record:?}"
    );
    assert_eq!(guard.unwrap(), 900);
}

/// Sol M1: a fork lands while a wallet opens mid-session, between the
/// resume's read of its record and the rewind: the resumed replay ends at
/// the new chain's height, and so does the record after a restart.
#[test]
fn a_fork_while_a_wallet_opens_lowers_the_replay_it_resumes() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.rescan(400).unwrap();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_resume.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let (s, id) = (Arc::clone(&f.session), f.wallet);
        f.session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the resume's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    f.truncate(900);
    f.settle();
    pause.release.notify_one();
    f.engine.block_on(load).unwrap().unwrap();
    let (record, target) = (f.record(), f.hub_heights().2);
    eprintln!(
        "ROW5965 fork during a mid-session open's resume: record {record:?}, hub target {target}"
    );
    assert!(record.is_some_and(|r| r.target <= 900), "{record:?}");
    assert!(target <= 900, "the hub waits for {target}");
    f.crash_on_close();
    let f = f.reopen();
    let record = f.record();
    f.advance(900);
    let guard = f.guard();
    eprintln!("ROW5965 the same after a restart: record {record:?}, replay guard at 900 {guard:?}");
    assert!(record.is_some_and(|r| r.target <= 900), "{record:?}");
    assert_eq!(guard.unwrap(), 900);
}

/// A rewind with no fork is no truncation: key-wallet's processed height
/// stays, so the target does too.
#[test]
fn a_rewind_with_no_fork_keeps_its_target() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.checkpoint(500);
    let (guard, heights, record) = (f.guard(), f.hub_heights(), f.record());
    eprintln!(
        "ROW5965 rewind to 500, no fork: guard {guard:?}, hub {heights:?}, record {record:?}"
    );
    assert!(replaying(guard));
    assert_eq!(heights.2, 1_000);
    assert!(record.is_some_and(|r| r.target == 1_000), "{record:?}");
}

/// A wallet stored with its processed height below its filter checkpoint
/// (platform-wallet stores them apart; dash-spv lifts the first to the
/// second when its filter sync starts) and a replay record above both
/// opens with that record's target: no truncation is read into it.
#[test]
fn a_wallet_stored_with_its_processed_height_behind_keeps_its_replay_target_at_open() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.store_heights(1_000, 900);
    f.store_record("1:1100:399:0");
    f.crash_on_close();
    let f = f.reopen();
    let guard = f.guard();
    let (record, heights) = (f.record(), f.hub_heights());
    eprintln!(
        "ROW5965 stored processed 900 below synced 1000, record target 1100: guard {guard:?}, record {record:?}, hub {heights:?}"
    );
    assert!(replaying(guard));
    assert!(record.is_some_and(|r| r.target == 1_100), "{record:?}");
    assert_eq!(heights.2, 1_100);
}

/// A truncation the hub found is not applied again when its event comes
/// late, after the new chain's progress: nothing is lowered further.
#[test]
fn a_truncation_heard_after_it_was_found_lowers_nothing_further() {
    for rescan in [true, false] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.advance(1_000);
        f.commit(1_000).unwrap();
        if rescan {
            f.rescan(400).unwrap();
        }
        f.truncate_silently(900);
        let _ = f.guard();
        f.advance(905);
        f.commit(905).unwrap();
        let before = (f.hub_heights(), f.record());
        f.hear_truncation(900);
        let after = (f.hub_heights(), f.record());
        eprintln!(
            "ROW5965 the event of a found truncation, late (a rescan's record {rescan}): before {before:?}, after {after:?}"
        );
        assert!(before.1.is_some());
        assert_eq!(before, after);
    }
}

/// A later fork's event, after a truncation the hub found, is applied:
/// below the found height, above it once the new chain passed it, and at
/// it once the found truncation's own event was heard.
#[test]
fn a_later_forks_event_after_a_found_truncation_is_applied() {
    for (heard_late, second) in [(false, 850), (false, 950), (true, 900)] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.advance(1_000);
        f.commit(1_000).unwrap();
        f.truncate_silently(900);
        let _ = f.guard();
        if heard_late {
            f.hear_truncation(900);
        }
        f.advance(1_000);
        f.commit(1_000).unwrap();
        f.truncate(second);
        let heights = f.hub_heights();
        eprintln!(
            "ROW5965 a later fork to {second} after a found one to 900 (its event heard {heard_late}): hub heights {heights:?}"
        );
        assert_eq!(heights, (second, second, second));
    }
}

/// A truncation whose event the hub heard is not found again in
/// key-wallet, before or after the new chain's progress.
#[test]
fn a_truncation_found_after_its_event_was_heard_is_not_applied_again() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.truncate(900);
    let heard = (f.hub_heights(), f.record());
    let _ = f.guard();
    let found = (f.hub_heights(), f.record());
    f.advance(905);
    f.commit(905).unwrap();
    let progressed = (f.hub_heights(), f.record());
    let _ = f.guard();
    let after = (f.hub_heights(), f.record());
    eprintln!(
        "ROW5965 a heard truncation: heard {heard:?}, after a guard {found:?}, at 905 {progressed:?}, after a guard {after:?}"
    );
    assert_eq!(heard, found);
    assert_eq!(progressed, after);
}

/// platform#5380: the store keeps the old chain's checkpoint. A quit after
/// a fork no event announced, before the new chain passes the old height,
/// reopens to a replay from where the fork was found (900, or up to the
/// new chain's 905 when found after its progress), so the new chain's
/// blocks up to the old height are scanned; the record goes once the
/// checkpoint is back at the old one. The fork is found by a replay guard
/// before the new chain's progress, by a guard after it, or with neither
/// by a reconcile or at the latest by the close.
#[test]
fn a_quit_after_a_silent_truncation_below_the_old_height_replays_from_the_fork() {
    for (guard_before, guard_after) in [(true, false), (false, false), (false, true)] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.store_heights(1_000, 1_000);
        f.advance(1_000);
        f.truncate_silently(900);
        if guard_before {
            let _ = f.guard();
        }
        f.advance(905);
        f.commit(905).unwrap();
        if guard_after {
            let _ = f.guard();
        }
        let record = f.record();
        let f = f.reopen();
        let (synced, reopened) = (f.synced(), f.record());
        f.advance(950);
        f.commit(950).unwrap();
        let f = f.reopen();
        let (again, at_950) = (f.synced(), f.record());
        f.advance(1_000);
        f.commit(1_000).unwrap();
        let before_quit = f.record();
        let f = f.reopen();
        let (past, after) = (f.synced(), f.record());
        eprintln!(
            "ROW5965 #5380 quit after a silent fork to 900 from 1000, new chain at 905, guard before {guard_before} after {guard_after}: record at the quit {record:?}; reopen: checkpoint {synced}, record {reopened:?}; at 950 reopen: checkpoint {again}, record {at_950:?}; at 1000 before the quit {before_quit:?}, reopen: checkpoint {past}, record {after:?}"
        );
        let shape = |r: Option<crate::replay_record::ReplayRecord>| {
            r.map(|r| (r.target, r.reached, r.forked_from))
        };
        let found = reopened.map_or(0, |r| r.reached);
        assert!((900..=905).contains(&found), "found at {found}");
        if guard_before {
            assert_eq!(found, 900);
        }
        if guard_before || guard_after {
            assert_eq!(shape(record), Some((905, found, 1_000)));
        }
        assert_eq!(
            (synced, shape(reopened)),
            (found, Some((905, found, 1_000)))
        );
        assert_eq!((again, shape(at_950)), (found, Some((950, found, 1_000))));
        assert_eq!(shape(before_quit), Some((1_000, found, 1_000)));
        assert_eq!((past, after), (1_000, None));
    }
}

/// Row 58's record of a replay a fork lowered survives a clean quit
/// before the new chain passes the old height: the reopen replays from
/// its start, below the fork.
#[test]
fn a_forks_lowered_record_survives_a_clean_quit_below_the_old_height() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.rescan(400).unwrap();
    f.truncate(900);
    f.advance(905);
    f.commit(905).unwrap();
    let record = f.record();
    let f = f.reopen();
    let (synced, reopened) = (f.synced(), f.record());
    eprintln!(
        "ROW5965 a fork lowered a rescan's record, clean quit at 905: record at the quit {record:?}; reopen: checkpoint {synced}, record {reopened:?}"
    );
    assert_eq!(
        reopened.map(|r| (r.target, r.reached, r.forked_from)),
        Some((905, 399, 1_000))
    );
    assert_eq!(synced, 399);
}

/// A store of the old chain whose note comes before a fork's, applied
/// after the fork's record is written, does not raise its target back to
/// the old height.
#[test]
fn an_old_chains_store_heard_after_a_forks_record_leaves_its_target() {
    use crate::replay_record::ReplayNote;
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.truncate_silently(900);
    f.settle();
    let wallet = f.wallet;
    let replay = Arc::clone(&f.session.live().unwrap().replay);
    replay.note(ReplayNote::Committed {
        wallet,
        height: 1_000,
    });
    replay.note(ReplayNote::Truncated {
        wallet,
        height: 900,
        from: 1_000,
    });
    let record = f.record();
    eprintln!("ROW5965 the old chain's store at 1000 heard after a fork to 900: record {record:?}");
    assert_eq!(
        record.map(|r| (r.target, r.reached, r.forked_from)),
        Some((900, 900, 1_000))
    );
}

/// A fork's record goes once the new chain is stored past the old height:
/// a quit then leaves none, and the reopen replays nothing.
#[test]
fn a_chain_past_the_old_height_at_the_quit_leaves_no_record() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.truncate_silently(900);
    let _ = f.guard();
    f.advance(1_001);
    f.commit(1_001).unwrap();
    let record = f.record();
    let f = f.reopen();
    let (synced, reopened) = (f.synced(), f.record());
    eprintln!(
        "ROW5965 silent fork to 900, new chain stored at 1001: record at the quit {record:?}; reopen: checkpoint {synced}, record {reopened:?}"
    );
    assert_eq!((record, reopened, synced), (None, None, 1_001));
}

/// A fork at the highest checkpoint the wallet can have stored, or above
/// it, leaves nothing to replay: no record is written.
#[test]
fn a_fork_at_the_stored_checkpoint_writes_no_record() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.hear_truncation(1_000);
    let record = f.record();
    eprintln!("ROW5965 a fork at the stored checkpoint 1000: record {record:?}");
    assert_eq!(record, None);
}

/// After the reopen of a fork's replay, a send waits for the replay to the
/// new chain's tip only, never for the chain to pass the old height.
#[test]
fn a_send_after_a_forks_reopen_waits_only_for_the_replay_to_the_tip() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.truncate_silently(900);
    let _ = f.guard();
    f.advance(905);
    f.commit(905).unwrap();
    let f = f.reopen();
    let replaying = f.guard();
    f.advance(905);
    let at_tip = f.guard();
    eprintln!(
        "ROW5965 reopen of a fork's replay from 900 to 905, old height 1000: guard while replaying {replaying:?}, at the tip {at_tip:?}"
    );
    assert!(matches!(replaying, Err(EngineError::RescanInProgress)));
    assert_eq!(at_tip.unwrap(), 905);
}

/// The new chain's progress past a fork no event announced, before
/// anything checked for it: the fork is found at 905, above the fork at
/// 900. A spend mined at 902 in a replaced block is gone and its coin no
/// longer reserved; a transaction mined again at 903 stays; an
/// InstantSend-locked one mined at 903 reads unconfirmed; records only
/// read from storage, at 600 and 800, stay.
#[test]
fn a_fork_found_past_its_height_drops_the_replaced_blocks_spends_below_it() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let mined = |height| {
        TransactionContext::InBlock(BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 9))
    };
    let f = fixture(false);
    let (coin, other, third) = (f.credit(1, COIN), f.credit(2, COIN), f.credit(3, COIN));
    f.quietly_at(1_000);
    let (replaced, locked, remined) = (
        f.spend_of(7, &[coin]),
        f.spend_of(8, &[other]),
        f.spend_of(9, &[third]),
    );
    f.see_in(&replaced, mined(902));
    f.see_in(
        &locked,
        TransactionContext::InstantSend(dashcore::InstantLock::default()),
    );
    f.see_in(&locked, mined(903));
    f.see_in(&remined, mined(902));
    let (stored_600, stored_800) = (
        Txid::from_byte_array([60; 32]),
        Txid::from_byte_array([80; 32]),
    );
    f.funded_at(stored_600, 600);
    f.funded_at(stored_800, 800);
    f.fork_and_progress(&[], 900, &[(&remined, mined(903))], 905);
    let guard = f.guard();
    let entries = [
        replaced.txid(),
        locked.txid(),
        remined.txid(),
        stored_600,
        stored_800,
    ]
    .map(|t| f.entry_of(t));
    let (reserved, offered) = (f.reserved(), f.offers(coin));
    eprintln!(
        "ROW5965 fork to 900 found at 905: guard {guard:?}; replaced at 902, IS-locked at 903, mined again at 903, stored at 600 and 800: {entries:?}; reserved {reserved:?}, coin offered {offered}"
    );
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entries[0], None, "the replaced spend stays");
    assert_eq!(entries[1], Some((None, true)), "still mined at 903");
    assert!(entries[2].is_some(), "the transaction mined again is gone");
    assert_eq!(entries[3], Some((Some(600), false)));
    assert_eq!(entries[4], Some((Some(800), false)));
    assert!(offered, "key-wallet did not put the coin back");
    assert!(!reserved.contains(&coin), "the coin stays reserved");
}

/// A transaction seen this session first in a ChainLocked block, which
/// key-wallet finalized at once (it keeps only the txid), stays in the
/// history across a fork found past its height.
#[test]
fn a_fork_found_past_its_height_keeps_a_finalized_transaction() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let info = BlockInfo::new(902, dashcore::BlockHash::all_zeros(), 9);
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    f.quietly_at(1_000);
    let finalized = f.spend_of(7, &[coin]);
    f.funded_at(finalized.txid(), 902);
    f.session.hub.history.with_wallet(f.wallet, |h| {
        h.held.insert(finalized.txid());
    });
    f.fork_and_progress(
        &[(&finalized, TransactionContext::InChainLockedBlock(info))],
        900,
        &[],
        905,
    );
    let guard = f.guard();
    let entry = f.entry_of(finalized.txid());
    eprintln!("ROW5965 fork to 900 found at 905: guard {guard:?}, finalized at 902 {entry:?}");
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entry, Some((Some(902), false)));
}

/// key-wallet's last ChainLock, at 950, refuses a fork below it. A fork
/// above it, to 960, found at 965: a transaction key-wallet still holds
/// in a block at 940 stays, one it held at 962 that the fork replaced is
/// gone, and one only read from storage at 970, above the found height,
/// is gone.
#[test]
fn a_fork_above_a_chain_lock_drops_what_it_replaced_and_keeps_what_key_wallet_holds() {
    use dashcore::bls_sig_utils::BLSSignature;
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    use platform_wallet::key_wallet_manager::WalletInterface;
    let mined = |height| {
        TransactionContext::InBlock(BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 9))
    };
    let f = fixture(false);
    let (coin, other) = (f.credit(1, COIN), f.credit(2, COIN));
    f.quietly_at(1_000);
    let (kept, replaced) = (f.spend_of(7, &[coin]), f.spend_of(8, &[other]));
    f.see_in(&kept, mined(940));
    f.see_in(&replaced, mined(962));
    let (manager, id) = (Arc::clone(&f.session.live().unwrap().manager), f.wallet);
    let refused = f.engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        info.core_wallet.metadata.last_applied_chain_lock = Some(dashcore::ChainLock {
            block_height: 950,
            block_hash: dashcore::BlockHash::all_zeros(),
            signature: BLSSignature::from([0; 96]),
        });
        wm.truncate_above(940).is_err()
    });
    let stored = Txid::from_byte_array([97; 32]);
    f.funded_at(stored, 970);
    f.fork_and_progress(&[], 960, &[], 965);
    let guard = f.guard();
    let entries = [kept.txid(), replaced.txid(), stored].map(|t| f.entry_of(t));
    eprintln!(
        "ROW5965 ChainLock 950, fork to 960 found at 965: fork to 940 refused {refused}, guard {guard:?}, held at 940, replaced at 962, stored at 970: {entries:?}"
    );
    assert!(refused);
    assert_eq!(guard.unwrap(), 965);
    assert_eq!(entries, [Some((Some(940), false)), None, None]);
}

/// The transactions key-wallet held are the session's: an unload forgets
/// them, an event key-wallet sent before the unload and heard after it
/// brings back neither them nor the wallet's history, and the wallet
/// loaded again starts anew, whatever the set held before.
#[test]
fn the_transactions_key_wallet_held_are_forgotten_at_unload() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let mined = |height| {
        TransactionContext::InBlock(BlockInfo::new(height, dashcore::BlockHash::all_zeros(), 9))
    };
    let f = fixture(false);
    let (coin, other) = (f.credit(1, COIN), f.credit(2, COIN));
    let (tx, late) = (f.spend_of(7, &[coin]), f.spend_of(8, &[other]));
    f.see_in(&tx, mined(902));
    let stale = f.seen_event(&late, mined(903));
    let held = || {
        (f.session.hub.history).read_wallet(&f.wallet, |h| {
            (h.held.contains(&tx.txid()), h.held.contains(&late.txid()))
        })
    };
    let before = held();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    let unloaded = held();
    f.session.hub.apply_wallet_event(&stale);
    let (after_stale, entry) = (held(), f.entry_of(late.txid()));
    f.session.hub.history.with_wallet(f.wallet, |h| {
        h.held.insert(late.txid());
    });
    f.engine.block_on(f.session.load_wallet(f.wallet)).unwrap();
    let loaded = held();
    eprintln!(
        "ROW5965 transactions key-wallet held: held {before:?}, after an unload {unloaded:?}, after a stale event {after_stale:?} (its entry {entry:?}), loaded again {loaded:?}"
    );
    assert_eq!(before, (true, false));
    assert_eq!(unloaded, (false, false));
    assert_eq!((after_stale, entry), ((false, false), None));
    assert_eq!(loaded, (false, false));
}

/// A found truncation's event heard late above its height lowers nothing
/// and ends the wait for it: a later fork's event at the found height is
/// applied.
#[test]
fn a_found_truncations_event_heard_above_it_ends_the_wait_for_it() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.truncate_silently(900);
    let _ = f.guard();
    f.hear_truncation(950);
    let heard = f.hub_heights();
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.truncate(900);
    let heights = f.hub_heights();
    eprintln!(
        "ROW5965 found at 900, its event heard at 950: hub {heard:?}; a later fork to 900: hub {heights:?}"
    );
    assert_eq!(heard, (900, 900, 900));
    assert_eq!(heights, (900, 900, 900));
}

/// A wallet opening mid-session with a cancelled replay {1000, 600}: a
/// fork to 900 between the resume's read of the record and its hold
/// leaves the hold's target at 900, so a rescan from the birth height
/// ends there and spends wait only for it.
#[test]
fn a_fork_while_a_wallet_opens_lowers_its_cancelled_replays_hold() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 300))
        .unwrap();
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    f.store_record("2:1000:600:1:0");
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_resume.lock().unwrap() = Some(Arc::clone(&pause));
    let load = {
        let (s, id) = (Arc::clone(&f.session), f.wallet);
        f.session.rt.spawn(async move { s.load_wallet(id).await })
    };
    eventually("the resume's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    f.truncate(900);
    f.settle();
    pause.release.notify_one();
    f.engine.block_on(load).unwrap().unwrap();
    let (record, target, held) = (f.record(), f.hub_heights().2, f.guard());
    eprintln!(
        "ROW5965 fork during the open of a cancelled replay: record {record:?}, hub target {target}, guard {held:?}"
    );
    assert!(
        record.is_some_and(|r| r.cancelled && r.target <= 900),
        "{record:?}"
    );
    assert!(target <= 900, "the hub waits for {target}");
    assert!(interrupted(&held), "{held:?}");
    f.rescan(300).unwrap();
    f.advance(900);
    let (progress, guard) = (f.session.rescan_progress().unwrap(), f.guard());
    eprintln!(
        "ROW5965 the same, then a rescan from the birth height at the new tip 900: progress {progress:?}, guard {guard:?}"
    );
    assert!(progress.is_none(), "the rescan waits for the old height");
    assert_eq!(guard.unwrap(), 900);
}

/// A fork's replay record that cannot be stored when the fork is applied
/// stays due: a read once writes work stores it, and the reopen replays
/// from below the old checkpoint to the new chain's tip. With no record
/// before, and with a rescan's record the fork lowers.
#[test]
fn a_forks_record_that_failed_to_store_is_stored_by_the_next_read() {
    for rescan in [false, true] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.store_heights(1_000, 1_000);
        f.advance(1_000);
        f.commit(1_000).unwrap();
        if rescan {
            f.rescan(400).unwrap();
        }
        let records = f.records();
        records
            .fail_writes
            .store(true, std::sync::atomic::Ordering::SeqCst);
        f.truncate_silently(900);
        let _ = f.guard();
        f.settle();
        records
            .fail_writes
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let record = f.record();
        let f = f.reopen();
        let (synced, reopened) = (f.synced(), f.record());
        f.advance(900);
        let guard = f.guard();
        eprintln!(
            "ROW5965 fork to 900 whose record failed to store (a rescan's record {rescan}): record after a read {record:?}; reopen: checkpoint {synced}, record {reopened:?}, guard at 900 {guard:?}"
        );
        let start = if rescan { 399 } else { 900 };
        let shape = |r: Option<crate::replay_record::ReplayRecord>| {
            r.map(|r| (r.target, r.reached, r.forked_from))
        };
        assert_eq!(shape(record), Some((900, start, 1_000)));
        assert_eq!(
            (synced, shape(reopened)),
            (start, Some((900, start, 1_000)))
        );
        assert_eq!(guard.unwrap(), 900);
    }
}

/// The old chain's store at 1010, heard after the fork's record was
/// written with the old checkpoint 1000: the record keeps 1010 as the old
/// chain's. A quit with the new chain at 1005 keeps the record, and the
/// reopen replays from the fork; a quit at 905 reopens to spends that
/// wait only for the new chain's tip.
#[test]
fn an_old_chains_store_heard_after_a_forks_record_raises_its_old_checkpoint() {
    for tip in [1_005, 905] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.store_heights(1_000, 1_000);
        f.advance(1_000);
        f.truncate_silently(900);
        let _ = f.guard();
        f.commit(1_010).unwrap();
        f.settle();
        f.advance(tip);
        f.commit(tip).unwrap();
        let record = f.record();
        let f = f.reopen();
        let (synced, reopened) = (f.synced(), f.record());
        f.advance(tip);
        let guard = f.guard();
        eprintln!(
            "ROW5965 fork to 900, the old chain's store at 1010 heard late, quit at {tip}: record {record:?}; reopen: checkpoint {synced}, record {reopened:?}, guard at {tip} {guard:?}"
        );
        let shape = |r: Option<crate::replay_record::ReplayRecord>| {
            r.map(|r| (r.target, r.reached, r.forked_from))
        };
        assert_eq!(shape(record), Some((tip, 900, 1_010)));
        assert_eq!((synced, shape(reopened)), (900, Some((tip, 900, 1_010))));
        assert_eq!(guard.unwrap(), tip);
    }
}

/// A store of the old chain whose note comes before a fork's, with the
/// wallet's checkpoint past the store's (no cap): the record is the
/// fork's, neither raised by that store nor ended by it.
#[test]
fn an_old_chains_store_heard_before_a_forks_note_leaves_its_record() {
    use crate::replay_record::ReplayNote;
    let f = fixture(false);
    f.credit(1, COIN);
    f.quietly_at(1_000);
    f.truncate_silently(900);
    f.checkpoint(1_005);
    f.settle();
    let (wallet, replay) = (f.wallet, f.records());
    replay.note(ReplayNote::Committed {
        wallet,
        height: 1_000,
    });
    replay.note(ReplayNote::Truncated {
        wallet,
        height: 900,
        from: 1_000,
    });
    let record = f.record();
    eprintln!(
        "ROW5965 the old chain's store at 1000 noted before a fork to 900, checkpoint 1005: record {record:?}"
    );
    assert_eq!(
        record.map(|r| (r.target, r.reached, r.forked_from)),
        Some((900, 900, 1_000))
    );
}

/// The header tip falls below one seen before: the hub checks every wallet
/// for a fork, so one no event announced is applied before the new chain
/// passes it.
#[test]
fn a_falling_header_tip_finds_a_fork_no_event_announced() {
    use dash_spv::EventHandler;
    use dash_spv::sync::{BlockHeadersProgress, SyncProgress, SyncState};
    let progress = |tip| {
        let mut headers = BlockHeadersProgress::default();
        headers.update_target_height(tip);
        headers.update_tip_height(tip);
        headers.set_state(SyncState::Synced);
        let mut p = SyncProgress::default();
        p.update_headers(headers);
        p
    };
    let f = fixture(false);
    f.credit(1, COIN);
    f.quietly_at(1_000);
    f.session.hub.on_progress(&progress(1_000));
    f.truncate_silently(900);
    f.session.hub.on_progress(&progress(900));
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while f.hub_heights().1 != 900 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let heights = f.hub_heights();
    eprintln!("ROW5965 header tip 1000 then 900 after a silent fork to 900: hub {heights:?}");
    assert_eq!(heights, (900, 900, 900));
}

/// A cancel that cannot store its hold puts the rescan back no higher than
/// the fork it found.
#[test]
fn a_failed_cancel_after_a_silent_truncation_restores_the_rescan_on_the_new_chain() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.quietly_at(1_000);
    f.rescan(400).unwrap();
    f.truncate_silently(900);
    let records = f.records();
    records
        .fail_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let cancelled = f.engine.block_on(f.session.cancel_rescan());
    records
        .fail_writes
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let resume = (f.session.hub.rescan().as_ref())
        .and_then(|r| r.wallets.get(&f.wallet).map(|w| w.resume_height));
    eprintln!(
        "ROW5965 silent fork to 900, cancel failed: {cancelled:?}, the rescan resumes to {resume:?}"
    );
    assert!(cancelled.is_err());
    assert_eq!(resume, Some(900));
}

fn record_shape(r: Option<crate::replay_record::ReplayRecord>) -> Option<(u32, u32, u32)> {
    r.map(|r| (r.target, r.reached, r.forked_from))
}

fn mined_at(height: u32) -> key_wallet::transaction_checking::TransactionContext {
    key_wallet::transaction_checking::TransactionContext::InBlock(
        key_wallet::transaction_checking::BlockInfo::new(
            height,
            dashcore::BlockHash::all_zeros(),
            9,
        ),
    )
}

/// The new chain's ChainLock at 905, applied after a fork to 900 and
/// before anything checked for it: a spend mined at 902 in a replaced
/// block is gone, and its coin no longer reserved.
#[test]
fn a_new_chains_chain_lock_before_the_check_keeps_no_spend_the_fork_replaced() {
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    f.quietly_at(1_000);
    let replaced = f.spend_of(7, &[coin]);
    f.see_in(&replaced, mined_at(902));
    f.fork_and_progress(&[], 900, &[], 905);
    f.chain_lock_at(905);
    let guard = f.guard();
    let (entry, reserved, offered) = (f.entry_of(replaced.txid()), f.reserved(), f.offers(coin));
    eprintln!(
        "ROW5965 fork to 900, new chain to 905 with its ChainLock at 905, then found: guard {guard:?}, replaced at 902 {entry:?}, reserved {reserved:?}, coin offered {offered}"
    );
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entry, None, "the replaced spend stays");
    assert!(offered, "key-wallet did not put the coin back");
    assert!(!reserved.contains(&coin), "the coin stays reserved");
}

/// The new chain's ChainLock one block above its filters, at 906, and an
/// old chain's transaction at 906, above the found height 905: it is
/// gone.
#[test]
fn an_old_chains_transaction_at_a_chain_lock_above_the_found_height_is_dropped() {
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    f.quietly_at(1_000);
    let old = f.spend_of(7, &[coin]);
    f.see_in(&old, mined_at(906));
    f.fork_and_progress(&[], 900, &[], 905);
    f.chain_lock_at(906);
    let guard = f.guard();
    let entry = f.entry_of(old.txid());
    eprintln!(
        "ROW5965 fork to 900 found at 905, ChainLock at 906: guard {guard:?}, the old chain's at 906 {entry:?}"
    );
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entry, None);
}

/// A transaction the history has in a ChainLocked block at 902, held this
/// session, stays across a fork found past its height: no fork replaces
/// a ChainLocked block.
#[test]
fn a_fork_found_past_its_height_keeps_a_chain_locked_entry() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    let f = fixture(false);
    f.credit(1, COIN);
    f.quietly_at(1_000);
    let locked = Txid::from_byte_array([92; 32]);
    f.funded_in(
        locked,
        TransactionContext::InChainLockedBlock(BlockInfo::new(
            902,
            dashcore::BlockHash::all_zeros(),
            9,
        )),
    );
    f.session.hub.history.with_wallet(f.wallet, |h| {
        h.held.insert(locked);
    });
    f.fork_and_progress(&[], 900, &[], 905);
    let guard = f.guard();
    let entry = f.entry_of(locked);
    eprintln!("ROW5965 fork to 900 found at 905: guard {guard:?}, ChainLocked at 902 {entry:?}");
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entry, Some((Some(902), false)));
}

/// An event key-wallet sent before an unload, for a transaction mined at
/// 800, heard after it: the wallet loaded again at its stored checkpoint
/// 1000 with the transaction's stored record keeps it across a fork to
/// 900 found at 905.
#[test]
fn a_stale_event_from_before_an_unload_leaves_the_reloaded_history() {
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    f.store_heights(1_000, 1_000);
    f.quietly_at(1_000);
    let tx = f.spend_of(7, &[coin]);
    let stale = f.seen_event(&tx, mined_at(800));
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    f.session.hub.apply_wallet_event(&stale);
    f.engine.block_on(f.session.load_wallet(f.wallet)).unwrap();
    f.funded_at(tx.txid(), 800);
    let held = (f.session.hub.history).read_wallet(&f.wallet, |h| h.held.contains(&tx.txid()));
    f.fork_and_progress(&[], 900, &[], 905);
    let guard = f.guard();
    let entry = f.entry_of(tx.txid());
    eprintln!(
        "ROW5965 stale event for 800 heard after an unload, reloaded at 1000, fork to 900 found at 905: held {held}, guard {guard:?}, stored at 800 {entry:?}"
    );
    assert!(!held);
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entry, Some((Some(800), false)));
}

/// A transaction key-wallet holds when the history is read, with no event
/// of it, is this session's: a fork found past its height that replaced
/// it drops it.
#[test]
fn a_record_key_wallet_holds_at_the_history_load_is_this_sessions() {
    use key_wallet::transaction_checking::WalletTransactionChecker;
    let f = fixture(false);
    let coin = f.credit(1, COIN);
    f.quietly_at(1_000);
    let replaced = f.spend_of(7, &[coin]);
    let (manager, id, tx) = (
        Arc::clone(&f.session.live().unwrap().manager),
        f.wallet,
        replaced.clone(),
    );
    f.engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
        let mut wallet = wallet.clone();
        (info.core_wallet)
            .check_core_transaction(&tx, mined_at(902), &mut wallet, true, true)
            .await;
    });
    let s = Arc::clone(&f.session);
    f.engine
        .block_on(
            f.session
                .on_runtime(async move { s.load_history_for(vec![id]).await }),
        )
        .unwrap();
    let loaded = f.entry_of(replaced.txid());
    f.fork_and_progress(&[], 900, &[], 905);
    let guard = f.guard();
    let entry = f.entry_of(replaced.txid());
    eprintln!(
        "ROW5965 held by key-wallet at the history load, fork to 900 found at 905: loaded {loaded:?}, guard {guard:?}, replaced at 902 {entry:?}"
    );
    assert_eq!(loaded, Some((Some(902), false)));
    assert_eq!(guard.unwrap(), 905);
    assert_eq!(entry, None);
}

/// A clean quit while the old chain's store at 1010 is committed and its
/// note not applied: the close lifts the fork's record as the note would,
/// so it stays, and the reopen replays from the fork. At 1005 and 905.
#[test]
fn a_close_before_the_old_chains_store_is_applied_keeps_its_checkpoint() {
    for tip in [1_005, 905] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.store_heights(1_000, 1_000);
        f.advance(1_000);
        f.truncate_silently(900);
        let _ = f.guard();
        f.settle();
        let pause = Arc::new(crate::coins::TestPause::default());
        *f.records().pause_commit.lock().unwrap() = Some(Arc::clone(&pause));
        f.commit(1_010).unwrap();
        eventually("the commit's pause", || {
            pause.reached.load(std::sync::atomic::Ordering::SeqCst)
        });
        f.advance(tip);
        f.commit(tip).unwrap();
        let f = f.reopen();
        pause.release.notify_one();
        let (synced, reopened) = (f.synced(), f.record());
        f.advance(tip);
        let guard = f.guard();
        eprintln!(
            "ROW5965 fork to 900, the old chain's store at 1010 not applied at a quit at {tip}: reopen: checkpoint {synced}, record {reopened:?}, guard at {tip} {guard:?}"
        );
        assert_eq!(
            (synced, record_shape(reopened)),
            (900, Some((tip, 900, 1_010)))
        );
        assert_eq!(guard.unwrap(), tip);
    }
}

/// The old chain's store at 1010 whose record lift cannot be stored: the
/// lift stays due, so a commit of the new chain at 1005 (or a quit
/// there) keeps the record, and the reopen replays from the fork, below
/// the stored checkpoint.
#[test]
fn a_forks_lift_that_failed_to_store_is_stored_by_the_next_read() {
    for quit in [false, true] {
        let f = fixture(false);
        f.credit(1, COIN);
        f.store_heights(1_000, 1_000);
        f.advance(1_000);
        f.truncate_silently(900);
        let _ = f.guard();
        f.settle();
        let records = f.records();
        records
            .fail_writes
            .store(true, std::sync::atomic::Ordering::SeqCst);
        f.commit(1_010).unwrap();
        f.settle();
        records
            .fail_writes
            .store(false, std::sync::atomic::Ordering::SeqCst);
        f.advance(1_005);
        let record = (!quit).then(|| {
            f.commit(1_005).unwrap();
            f.record()
        });
        let f = f.reopen();
        let (synced, reopened) = (f.synced(), f.record());
        eprintln!(
            "ROW5965 fork to 900, the old chain's store at 1010 whose lift failed, new chain at 1005 (commit {}): record {record:?}; reopen: checkpoint {synced}, record {reopened:?}",
            !quit
        );
        if !quit {
            assert_eq!(record_shape(record.flatten()), Some((900, 900, 1_010)));
        }
        assert_eq!(
            (synced, record_shape(reopened)),
            (900, Some((1_005, 900, 1_010)))
        );
    }
}

/// A fork to 900 between a resumed replay's rewind and its publication:
/// the rescan ends at 900, and a new one is not refused.
#[test]
fn a_fork_before_a_resumed_replay_is_published_ends_it_at_the_new_tip() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    f.store_record("2:1000:600:0:0");
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_publish.lock().unwrap() = Some(Arc::clone(&pause));
    let load = f.load_until(&pause);
    f.truncate(900);
    f.settle();
    pause.release.notify_one();
    f.engine.block_on(load).unwrap().unwrap();
    let resume = (f.session.hub.rescan().as_ref())
        .and_then(|r| r.wallets.get(&f.wallet).map(|w| w.resume_height));
    f.advance(900);
    let progress = f.session.rescan_progress().unwrap();
    let again = f.rescan(400);
    eprintln!(
        "ROW5965 fork to 900 before a resume's publication: resumes to {resume:?}; at 900 progress {progress:?}, a new rescan {again:?}"
    );
    assert_eq!(resume, Some(900));
    assert!(progress.is_none(), "the rescan waits for the old height");
    assert!(again.is_ok(), "{again:?}");
}

/// A fork to 900 between a rescan's rewind and its publication: the
/// rescan ends at 900, and a new one is not refused.
#[test]
fn a_fork_before_a_rescan_is_published_ends_it_at_the_new_tip() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_publish.lock().unwrap() = Some(Arc::clone(&pause));
    let rescan = {
        let (s, id) = (Arc::clone(&f.session), f.wallet);
        f.session
            .rt
            .spawn(async move { s.start_rescan(vec![id], 400, |_| 400).await })
    };
    eventually("the rescan's pause", || {
        pause.reached.load(std::sync::atomic::Ordering::SeqCst)
    });
    f.truncate(900);
    f.settle();
    pause.release.notify_one();
    f.engine.block_on(rescan).unwrap().unwrap();
    let resume = (f.session.hub.rescan().as_ref())
        .and_then(|r| r.wallets.get(&f.wallet).map(|w| w.resume_height));
    f.advance(900);
    let progress = f.session.rescan_progress().unwrap();
    let again = f.rescan(400);
    eprintln!(
        "ROW5965 fork to 900 before a rescan's publication: resumes to {resume:?}; at 900 progress {progress:?}, a new rescan {again:?}"
    );
    assert_eq!(resume, Some(900));
    assert!(progress.is_none(), "the rescan waits for the old height");
    assert!(again.is_ok(), "{again:?}");
}

/// A fork to 900 between a resume's read of its record {1000, 600} and a
/// rewind that fails: the hold it publishes waits for 900, so a rescan
/// from the birth height ends there.
#[test]
fn a_fork_before_a_failed_resume_lowers_its_hold() {
    let f = fixture(false);
    f.credit(1, COIN);
    f.engine
        .block_on(f.session.set_birth_height(f.wallet, 300))
        .unwrap();
    f.store_heights(1_000, 1_000);
    f.advance(1_000);
    f.commit(1_000).unwrap();
    f.engine
        .block_on(f.session.unload_wallet(f.wallet))
        .unwrap();
    f.store_record("2:1000:600:0:0");
    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_resume.lock().unwrap() = Some(Arc::clone(&pause));
    let load = f.load_until(&pause);
    f.truncate(900);
    f.settle();
    f.records()
        .crash_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    pause.release.notify_one();
    let loaded = f.engine.block_on(load).unwrap();
    let (target, held) = (f.hub_heights().2, f.guard());
    eprintln!(
        "ROW5965 fork to 900 before a failed resume of {{1000, 600}}: open {loaded:?}, hub target {target}, guard {held:?}"
    );
    assert!(target <= 900, "the hub waits for {target}");
    assert!(interrupted(&held), "{held:?}");
    f.rescan(300).unwrap();
    f.advance(900);
    let (progress, guard) = (f.session.rescan_progress().unwrap(), f.guard());
    eprintln!(
        "ROW5965 the same, then a rescan from the birth height at the new tip 900: progress {progress:?}, guard {guard:?}"
    );
    assert!(progress.is_none(), "the rescan waits for the old height");
    assert_eq!(guard.unwrap(), 900);
}
