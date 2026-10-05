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

use super::*;
use crate::{BookPurpose, CoinFilter, LabelsFailure, DashNetwork, Engine, EngineConfig, EventSink, ImportOptions};

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
    let dir = tempfile::tempdir().unwrap();
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

    fn spend_grant(&self, max_duffs: u64) -> String {
        self.session
            .vault()
            .authorize(GrantPurpose::Spend { max_duffs }, Credential::None)
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
        coins.into_iter().filter(|c| c.reserved).map(|c| c.outpoint).collect()
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
        .block_on(draft.prepare(f.spend_grant(30_000_000)))
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
        .block_on(second.prepare(f.spend_grant(10_000_000)))
        .unwrap();
    assert_eq!(p2.summary().inputs[0].outpoint, small);

    f.engine.block_on(draft.abandon(Arc::clone(&prepared))).unwrap();
    f.engine.block_on(draft.abandon(Arc::clone(&prepared))).unwrap();
    assert_eq!(f.reserved(), vec![small]);
    assert_eq!(
        send_failure(f.engine.block_on(draft.broadcast(Arc::clone(&prepared)))),
        SendFailure::PreparedTxSpent
    );
    // A prepared transaction belongs to the draft that made it.
    let r = f.engine.block_on(draft.abandon(Arc::clone(&p2)));
    assert!(matches!(r, Err(EngineError::InvalidArgument(_))), "{r:?}");
}

#[test]
fn grants_cap_what_leaves_the_wallet() {
    let f = fixture(false);
    f.credit(1, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 30_000_000)]);
    let r = f.engine.block_on(draft.prepare(f.spend_grant(29_999_999)));
    assert_eq!(
        send_failure(r),
        SendFailure::GrantExceeded {
            max_duffs: 29_999_999,
            external_sent: 30_000_000
        }
    );
    // The grant was single-use.
    let grant = f.spend_grant(30_000_000);
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
    let r = f.engine.block_on(draft.prepare(f.spend_grant(30_000_000)));
    assert_eq!(
        send_failure(r),
        SendFailure::GrantExceeded {
            max_duffs: 30_000_000,
            external_sent: 30_000_000 + change
        }
    );
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(30_000_000 + change)))
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
        .block_on(draft.prepare(f.spend_grant(10_000_000)))
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
    let p = f.engine.block_on(draft.prepare(f.spend_grant(COIN))).unwrap();
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
        f.engine.block_on(f.session.locked_outpoints(f.wallet)).unwrap(),
        vec![a]
    );
    let listed = f
        .engine
        .block_on(f.session.utxos(f.wallet, CoinFilter::default()))
        .unwrap();
    assert_eq!(listed.iter().map(|c| c.outpoint).collect::<Vec<_>>(), vec![b]);
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
    assert!(all.iter().any(|c| c.outpoint == a && c.user_locked && !c.spendable));
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
        .block_on(draft.prepare(f.spend_grant(10_000_000)))
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
        SendFailure::AmountExceedsBalance { available: COIN / 2 }
    );

    f.engine
        .block_on(f.session.unlock_outpoints(f.wallet, vec![a]))
        .unwrap();
    assert!(f.engine.block_on(f.session.locked_outpoints(f.wallet)).unwrap().is_empty());
    let unknown = OutPoint::new(Txid::from_byte_array([9; 32]), 0);
    let r = f.engine.block_on(f.session.lock_outpoints(f.wallet, vec![unknown]));
    assert!(matches!(r, Err(EngineError::OutpointNotFound(_))), "{r:?}");
}

#[test]
fn broadcast_needs_spv_and_drop_releases() {
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let draft = f.draft(vec![pay(FOREIGN, 10_000_000)]);
    let p = f
        .engine
        .block_on(draft.prepare(f.spend_grant(10_000_000)))
        .unwrap();
    let r = f.engine.block_on(draft.broadcast(Arc::clone(&p)));
    assert!(matches!(r, Err(EngineError::SpvNotRunning)), "{r:?}");
    // Not sent: still pending and still reserved.
    assert_eq!(f.reserved(), vec![a]);
    drop(p);
    assert!(f.reserved().is_empty());
    // key-wallet's reservation is released by a task the drop spawned.
    let mut ok = false;
    for _ in 0..50 {
        let d = f.draft(vec![pay(FOREIGN, 10_000_000)]);
        if f.engine.block_on(d.estimate()).is_ok()
            && f.engine.block_on(d.prepare(f.spend_grant(10_000_000))).is_ok()
        {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok, "the dropped transaction's input never became spendable again");
}

#[test]
fn locked_vault_and_validation_errors() {
    let f = fixture(true);
    f.credit(1, COIN);
    f.session.lock_vault().unwrap();
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
    assert!(matches!(
        empty.set_source(CoinSource::FullyMixedOnly),
        Err(EngineError::NotImplemented(_))
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
    assert_eq!(save(FOREIGN, "Bob", BookPurpose::Send, true).unwrap().label, "Bob");
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
        .block_on(f.session.address_book(f.wallet, Some(BookPurpose::Send), Some("b*b".into())))
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(matches!(
        f.engine.block_on(f.session.delete_address_book_entry(f.wallet, own.clone())),
        Err(EngineError::Labels(LabelsFailure::ReceiveEntryNotDeletable))
    ));
    f.engine
        .block_on(f.session.delete_address_book_entry(f.wallet, FOREIGN.into()))
        .unwrap();
    assert!(matches!(
        f.engine.block_on(f.session.delete_address_book_entry(f.wallet, FOREIGN.into())),
        Err(EngineError::Labels(LabelsFailure::EntryNotFound))
    ));

    let txid = "ab".repeat(32);
    f.engine
        .block_on(f.session.set_tx_label(f.wallet, txid.clone(), Some("rent".into())))
        .unwrap();
    assert_eq!(
        f.engine.block_on(f.session.tx_label(f.wallet, txid.clone())).unwrap(),
        Some("rent".into())
    );
    f.engine
        .block_on(f.session.set_tx_label(f.wallet, txid.clone(), None))
        .unwrap();
    assert_eq!(f.engine.block_on(f.session.tx_label(f.wallet, txid)).unwrap(), None);
}
