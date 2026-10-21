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
/// grant caps it and the send and a PSBT show it going out.
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
            let owns = |a: &str| {
                let a = Address::from_str(a).unwrap().assume_checked();
                crate::coins::wallet_owns(&state.core_wallet, &a)
            };
            Ok::<_, EngineError>((owns(&o), owns(&v)))
        }))
        .unwrap();
    assert_eq!(listed, (true, true), "the wallet lists both keys");

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
