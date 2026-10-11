//! Lease tests against a real session and vault (E0-04 §12 [A+B], P2a):
//! the facade lease handle, `TxDraft` under a lease, the locks, the
//! background lease, epoch wiring and the key window. Real time.

use std::sync::Arc;
use std::time::Duration;

use dashcore::hashes::Hash;
use dashcore::{OutPoint, TxOut, Txid};
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, LockState, MemoryOsStore, UnlockScope,
    VaultConfig, VaultError,
};
use key_wallet::Utxo;
use zeroize::Zeroizing;

use super::session::issue;
use super::table::LeaseState;
use super::tests::{Recorder, art, status, table_with, with_journal};
use super::*;
use crate::platform::PlatformError;
use crate::platform::flows::{
    BudgetPurpose, DispatchResolution, DispatchState, FlowKind, LeaseStateView, RevokeCause,
};
use crate::send::{CoinSource, Recipient, SendFailure};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";
const COIN: u64 = 100_000_000;
const PASS: &[u8] = b"pass phrase";

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    rec: Arc<Recorder>,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

fn fixture(encrypted: bool) -> Fixture {
    let dir = dw_testutil::private_tempdir();
    let rec = Arc::new(Recorder::default());
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
        rec.clone(),
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
    let pass: Option<&'static [u8]> = encrypted.then_some(PASS);
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
        rec,
        session,
        wallet,
    }
}

impl Fixture {
    fn grant(&self, purpose: GrantPurpose, credential: Credential<'_>) -> String {
        self.session
            .vault()
            .authorize(purpose, Some(&self.wallet.0), credential)
            .unwrap()
            .id
    }

    fn platform_op(&self, max_duffs: u64, max_credits: u64) -> String {
        self.grant(
            GrantPurpose::PlatformOp {
                max_duffs,
                max_credits,
            },
            Credential::None,
        )
    }

    fn lease(&self, flow: FlowKind, grants: &[String]) -> Result<Lease, LeaseError> {
        self.engine
            .block_on(self.session.begin_lease(self.wallet, flow, grants))
    }

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
                    wallet
                        .state_mut()
                        .await
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

    /// Tracks asset lock `seed` of identity `identity_index` spending
    /// `inputs`, with `status`. key-wallet's coins stay unspent, as for a
    /// lock the wallet never saw sent.
    fn track_lock(
        &self,
        seed: u8,
        identity_index: u32,
        inputs: &[OutPoint],
        status: platform_wallet::AssetLockStatus,
    ) -> Txid {
        let input = inputs
            .iter()
            .map(|o| dashcore::TxIn {
                previous_output: *o,
                ..Default::default()
            })
            .collect();
        let tx = dashcore::Transaction {
            version: 3,
            lock_time: 0,
            input,
            output: Vec::new(),
            special_transaction_payload: None,
        };
        self.track_lock_tx(
            Txid::from_byte_array([seed; 32]),
            tx,
            identity_index,
            status,
        )
    }

    /// [`Self::track_lock`] of transaction `tx`, under `txid`.
    fn track_lock_tx(
        &self,
        txid: Txid,
        tx: dashcore::Transaction,
        identity_index: u32,
        status: platform_wallet::AssetLockStatus,
    ) -> Txid {
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let out_point = OutPoint::new(txid, 0);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                wallet.state_mut().await.tracked_asset_locks.insert(
                    out_point,
                    platform_wallet::TrackedAssetLock {
                        out_point,
                        transaction: tx,
                        account_index: 0,
                        funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
                        identity_index,
                        amount: COIN,
                        status,
                        proof: None,
                    },
                );
                Ok(())
            }))
            .unwrap();
        txid
    }

    /// Records `tx` in BIP44 account 0 as key-wallet would, with
    /// `context`; `spends`: as spending one of the wallet's coins.
    fn record_tx(
        &self,
        tx: dashcore::Transaction,
        context: key_wallet::transaction_checking::TransactionContext,
        spends: bool,
    ) {
        use key_wallet::account::{AccountType, StandardAccountType};
        use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
        use key_wallet::managed_account::transaction_record::{
            InputDetail, TransactionDirection, TransactionRecord,
        };
        let session = Arc::clone(&self.session);
        let id = self.wallet;
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move {
                let wallet = inner.wallet(&id).await?;
                let address = wallet.core().next_receive_address_for_account(0).await?;
                let input_details = if spends {
                    vec![InputDetail {
                        index: 0,
                        value: COIN,
                        address,
                    }]
                } else {
                    Vec::new()
                };
                let record = TransactionRecord::new(
                    tx,
                    AccountType::Standard {
                        index: 0,
                        standard_account_type: StandardAccountType::BIP44Account,
                    },
                    context,
                    key_wallet::transaction_checking::TransactionType::Standard,
                    TransactionDirection::Internal,
                    input_details,
                    Vec::new(),
                    0,
                );
                wallet
                    .state_mut()
                    .await
                    .core_wallet
                    .accounts
                    .standard_bip44_accounts
                    .get_mut(&0)
                    .expect("BIP44 account 0")
                    .transactions_mut()
                    .insert(record.txid, record);
                Ok(())
            }))
            .unwrap();
    }

    /// The wallet's coin fence (`coins.rs`).
    fn fence(&self) -> crate::CoinFence {
        self.engine
            .block_on(self.session.coin_fence(self.wallet))
            .unwrap()
    }

    /// Funding `step` of identity 0 after a call that released a signature
    /// and failed: `MaybeSent`, its marker standing. `step` is the funding
    /// step of the wallet's first draft, inserted here at index 0.
    fn stuck(&self, step: &str) {
        self.stuck_with(step, None);
    }

    /// [`Self::stuck`], the call having signed asset lock `seed` spending
    /// `inputs` (§2a.6): the step's own lock, recorded as the library's
    /// store of it would. Its row is not tracked.
    fn stuck_signing(&self, step: &str, seed: u8, inputs: &[OutPoint]) {
        self.stuck_with(step, Some((seed, inputs)));
    }

    fn stuck_with(&self, step: &str, lock: Option<(u8, &[OutPoint])>) {
        let draft = self.registration_draft(0);
        assert_eq!(step, format!("registration/{draft}/funding"));
        let l = self
            .lease(FlowKind::Registration, &[self.platform_op(10_000_000, 0)])
            .unwrap();
        let permit = self
            .engine
            .block_on(self.session.funding_call_permit(
                &l,
                "create_funded_asset_lock_proof",
                1_000_000,
                step.into(),
                FundingKey {
                    funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
                    identity_index: 0,
                    account: 0,
                },
                self.fence(),
            ))
            .unwrap();
        match lock {
            None => permit.signatures().note_released(),
            Some((seed, inputs)) => {
                let sighash = [seed; 32];
                permit.signatures().note_signed(sighash);
                self.session.leases.record_signed_locks(
                    self.wallet,
                    &[StoredLock {
                        txid: Txid::from_byte_array([seed; 32]),
                        inputs: inputs.to_vec(),
                        sighashes: Some(vec![sighash]),
                    }],
                );
            }
        }
        assert_eq!(permit.finish(false), FlowOutcome::MaybeSent);
    }

    /// Inserts a `dp_registration` draft at `identity_index` (DP1-02's
    /// table) and returns its id: its funding step's own key
    /// (`IdentityRegistration`, `identity_index`, account 0).
    fn registration_draft(&self, identity_index: u32) -> i64 {
        use dw_appdb::{SqlValue, TableRows};
        let w = self.wallet.to_string();
        self.engine
            .block_on(self.session.appdb_op(move |db| {
                let before = db.registration_identity_indices(&w)?;
                let text = |t: &str| SqlValue::Text(t.to_owned());
                db.import_wallet_rows(
                    &w,
                    &[TableRows {
                        table: "dp_registration".into(),
                        columns: [
                            "wallet_id",
                            "identity_index",
                            "label",
                            "funding",
                            "phase",
                            "created_at",
                            "updated_at",
                        ]
                        .map(String::from)
                        .to_vec(),
                        rows: vec![vec![
                            text(&w),
                            SqlValue::Integer(i64::from(identity_index)),
                            text("alice"),
                            text("{}"),
                            text("draft"),
                            SqlValue::Integer(1),
                            SqlValue::Integer(1),
                        ]],
                    }],
                )?;
                let after = db.registration_identity_indices(&w)?;
                Ok(after
                    .into_iter()
                    .find(|d| !before.contains(d))
                    .expect("inserted")
                    .0)
            }))
            .unwrap()
    }

    fn reserved(&self) -> usize {
        self.engine
            .block_on(self.session.utxos(
                self.wallet,
                crate::CoinFilter {
                    include_locked: true,
                    ..Default::default()
                },
            ))
            .unwrap()
            .iter()
            .filter(|c| c.reserved)
            .count()
    }

    fn draft(&self, amount: u64) -> Arc<crate::send::TxDraft> {
        let d = self.session.new_tx_draft(self.wallet).unwrap();
        d.set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
        d
    }

    /// Waits up to 5 s for `f`.
    fn eventually(&self, what: &str, mut f: impl FnMut() -> bool) {
        for _ in 0..250 {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }
}

fn spend_of(lease: &Lease) -> Option<u64> {
    lease
        .view()?
        .budgets
        .iter()
        .find(|b| b.purpose == BudgetPurpose::Spend)
        .map(|b| b.spent)
}

fn send_failure<T: std::fmt::Debug>(r: Result<T, EngineError>) -> SendFailure {
    match r {
        Err(EngineError::Send(f)) => f,
        other => panic!("expected a send failure, got {other:?}"),
    }
}

#[test]
fn the_facade_lease_handle_begins_ends_and_checks_its_grants() {
    let f = fixture(false);
    let s = &f.session;
    let g = f.platform_op(0, 1_000);
    let id = f
        .engine
        .block_on(s.begin_flow(f.wallet, FlowKind::ContactRequest, vec![g.clone()]))
        .unwrap();
    assert!(parse_lease(&id).is_some(), "{id}");
    let views = s.leases().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].state, LeaseStateView::Active);
    assert_eq!(views[0].flow, FlowKind::ContactRequest);
    // The grant was consumed.
    assert_eq!(
        f.engine
            .block_on(s.begin_flow(f.wallet, FlowKind::ContactRequest, vec![g])),
        Err(PlatformError::GrantInvalid)
    );
    // The session owns it (§4.1): a facade lookup, dropped, ends nothing.
    drop(s.lease_for(&f.wallet, &id).unwrap().unwrap());
    assert_eq!(s.leases().unwrap()[0].state, LeaseStateView::Active);
    s.end_flow(id.clone()).unwrap();
    s.end_flow(id).unwrap();
    assert_eq!(s.leases().unwrap()[0].state, LeaseStateView::Ended);
    assert_eq!(
        s.end_flow("lease-zz".into()),
        Err(PlatformError::GrantInvalid)
    );

    // A Spend grant belongs to "Accept and pay" only; one grant per kind.
    let spend = f.grant(GrantPurpose::Spend { max_duffs: 1 }, Credential::None);
    assert_eq!(
        f.lease(FlowKind::ContactRequest, std::slice::from_ref(&spend))
            .unwrap_err(),
        LeaseError::Invalid
    );
    let two = [f.platform_op(0, 1), f.platform_op(0, 1)];
    assert_eq!(
        f.lease(FlowKind::Accept, &two).unwrap_err(),
        LeaseError::Invalid
    );
    let both = [f.platform_op(0, 1_000), spend];
    let l = f.lease(FlowKind::AcceptAndPay, &both).unwrap();
    assert_eq!(spend_of(&l), Some(0));
    // Another wallet's facade call cannot use it.
    assert!(matches!(
        f.session.lease_for(&WalletId([9; 32]), &l.id_string()),
        Some(Err(LeaseError::Invalid))
    ));
}

#[test]
fn a_txdraft_under_a_lease_is_charged_fenced_and_refunded() {
    let f = fixture(false);
    f.credit(1, COIN);
    let grants = [
        f.platform_op(0, 1_000),
        f.grant(
            GrantPurpose::Spend {
                max_duffs: 5_000_000,
            },
            Credential::None,
        ),
    ];
    let l = f.lease(FlowKind::AcceptAndPay, &grants).unwrap();
    let lease = l.id_string();

    // A BIP44 spend above the cap is refused before signing.
    let d = f.draft(10_000_000);
    assert!(matches!(
        send_failure(f.engine.block_on(d.prepare(lease.clone()))),
        SendFailure::GrantExceeded { .. }
    ));
    assert_eq!(spend_of(&l), Some(0));
    assert_eq!(f.reserved(), 0);

    // Within the cap: charged at prepare, refunded by abandon.
    let d = f.draft(1_000_000);
    let p = f.engine.block_on(d.prepare(lease.clone())).unwrap();
    assert!(spend_of(&l).unwrap() > 1_000_000);
    f.engine.block_on(d.abandon(p)).unwrap();
    assert_eq!(spend_of(&l), Some(0));
    assert_eq!(f.reserved(), 0);

    // SPV is not running: a definite NotSent settles, releases, refunds.
    let p = f.engine.block_on(d.prepare(lease.clone())).unwrap();
    let txid = ArtifactId::parse(&p.summary().txid).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(d.broadcast(Arc::clone(&p)))),
        SendFailure::NoPeers
    );
    assert_eq!(spend_of(&l), Some(0));
    assert_eq!(f.reserved(), 0);
    assert_eq!(
        f.session.leases.dispatch_status(f.wallet, txid),
        Some(DispatchState::NotSent)
    );

    // Prepared, then a lock before the hand-off: send.cancelled, released.
    let p = f.engine.block_on(d.prepare(lease.clone())).unwrap();
    assert!(spend_of(&l).unwrap() > 0);
    f.session.lock_vault_sync().unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(d.broadcast(p))),
        SendFailure::Cancelled
    );
    assert_eq!(spend_of(&l), Some(0));
    assert_eq!(f.reserved(), 0);
    // The revoked lease prepares nothing more.
    assert_eq!(
        send_failure(f.engine.block_on(d.prepare(lease))),
        SendFailure::Cancelled
    );
    // A plain grant still prepares (Unleased(Send) until P5).
    let g = f.grant(
        GrantPurpose::Spend {
            max_duffs: 5_000_000,
        },
        Credential::None,
    );
    f.engine.block_on(d.prepare(g)).unwrap();
}

#[test]
fn the_synchronous_lock_returns_after_its_gate_and_done_carries_the_report() {
    let f = fixture(true);
    let l = f
        .lease(FlowKind::Registration, &[f.platform_op(1_000, 1_000)])
        .unwrap();
    let status = f.session.lock_vault_sync().unwrap();
    assert_eq!(status.state, LockState::Locked);
    assert_eq!(l.state(), Some(LeaseState::Revoked(RevokeCause::Lock)));
    f.eventually("LockProgress::Done", || !f.rec.done().is_empty());
    let report = f.rec.done().pop().unwrap();
    assert_eq!(report.status.state, LockState::Locked);
    assert_eq!(report.flows.len(), 1);
    assert_eq!(report.flows[0].outcome, FlowOutcome::Cancelled);

    // On a Locked vault: an own-key lease (a passphrase grant) holds the
    // key until Vault.lock() revokes it.
    let g = f.grant(
        GrantPurpose::PlatformOp {
            max_duffs: 1_000,
            max_credits: 0,
        },
        Credential::Passphrase(PASS),
    );
    let own = f.lease(FlowKind::TopUp, &[g]).unwrap();
    let view = own.view().unwrap();
    assert!(view.own_key);
    assert!(view.key_expires_in_secs.is_some());
    own.funding_signer().unwrap();
    // The async lock returns the report.
    let report = f.engine.block_on(f.session.lock_vault()).unwrap();
    assert_eq!(own.state(), Some(LeaseState::Revoked(RevokeCause::Lock)));
    assert!(report.flows.iter().any(|r| r.lease == own.id_string()));
    assert_eq!(own.view().unwrap().key_expires_in_secs, None, "key dropped");
}

#[test]
fn the_background_lease_follows_the_vault_state() {
    let f = fixture(true);
    let s = &f.session;
    f.engine.block_on(s.ensure_background());
    assert!(s.leases.background(&f.wallet).is_some(), "Unlocked");
    s.lock_vault_sync().unwrap();
    assert!(
        s.leases.background(&f.wallet).is_none(),
        "dropped in the freeze"
    );
    f.engine
        .block_on(s.vault_op(|v| v.unlock(PASS, UnlockScope::MixingOnly)))
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(s.leases.background(&f.wallet).is_none(), "MixingOnly");
    f.engine
        .block_on(s.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    f.eventually("the background lease", || {
        s.leases.background(&f.wallet).is_some()
    });
    // A revoking call drops it and re-creates it.
    f.engine
        .block_on(s.revoking_vault_op(RevokeCause::PassphraseChange, |v| {
            v.change_passphrase(PASS, b"new pass phrase")
        }))
        .unwrap();
    assert!(s.leases.background(&f.wallet).is_some());
    // The lazy path: absent, then created on demand.
    s.lock_vault_sync().unwrap();
    assert!(f.engine.block_on(s.background_crypto(f.wallet)).is_none());

    let u = fixture(false);
    assert!(
        u.engine
            .block_on(u.session.background_crypto(u.wallet))
            .is_some(),
        "Unencrypted"
    );
}

/// Polls `fut` on this thread, outside any tokio runtime, as UniFFI's
/// foreign executor polls dw-ffi's futures (the crate docs' contract).
fn block_on_foreign<F: std::future::Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Arc::new(Unpark(std::thread::current())).into();
    let mut cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let std::task::Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}

/// An unlock through `vault_op` polled off the engine's runtime (dw-ffi's
/// `Vault`) re-creates the background lease. Its key read used the
/// caller's runtime and panicked with "there is no reactor running".
#[test]
fn an_unlock_polled_off_the_runtime_recreates_the_background_lease() {
    let f = fixture(true);
    let s = &f.session;
    s.lock_vault_sync().unwrap();
    assert!(s.leases.background(&f.wallet).is_none());
    block_on_foreign(s.vault_op(|v| v.unlock(PASS, UnlockScope::Full))).unwrap();
    assert!(s.leases.background(&f.wallet).is_some());
}

#[test]
fn an_epoch_change_through_vault_op_needs_a_grant_and_rebind_restores() {
    let f = fixture(true);
    let s = &f.session;
    s.lock_vault_sync().unwrap();
    let g = f.grant(
        GrantPurpose::PlatformOp {
            max_duffs: 0,
            max_credits: 1_000,
        },
        Credential::Passphrase(PASS),
    );
    let l = f.lease(FlowKind::ProfileEdit, &[g]).unwrap();
    assert!(l.view().unwrap().own_key);
    f.engine
        .block_on(s.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert_eq!(l.state(), Some(LeaseState::NeedsGrant));
    assert_eq!(l.view().unwrap().key_expires_in_secs, None, "key dropped");
    assert_eq!(
        l.identity_signer(&[0]).unwrap_err(),
        LeaseError::NeedsGrant(BudgetPurpose::Credits)
    );
    f.eventually("the background lease", || {
        s.leases.background(&f.wallet).is_some()
    });
    let fresh = f.platform_op(0, 5_000);
    f.engine.block_on(s.rebind_lease(&l, &[fresh])).unwrap();
    assert_eq!(l.state(), Some(LeaseState::Active));
    assert!(!l.view().unwrap().own_key);
    l.identity_signer(&[0]).unwrap();
}

#[test]
fn a_wrong_old_passphrase_revokes_no_lease_and_a_correct_one_revokes_every_lease() {
    let f = fixture(true);
    let s = &f.session;
    let l = f
        .lease(FlowKind::ProfileEdit, &[f.platform_op(0, 5_000)])
        .unwrap();
    let pending = f.platform_op(0, 1_000);
    let change = |old: &[u8], new: &[u8]| {
        f.engine.block_on(
            s.change_passphrase(Zeroizing::new(old.to_vec()), Zeroizing::new(new.to_vec())),
        )
    };
    // DEC-134: a wrong old passphrase, or a rejected new one, is refused
    // before the freeze: every lease and grant stays usable.
    assert!(
        matches!(
            change(b"wrong", b"other"),
            Err(EngineError::Vault(VaultError::WrongPassphrase { .. }))
        ),
        "wrong old passphrase"
    );
    assert!(matches!(
        change(PASS, b""),
        Err(EngineError::Vault(VaultError::PassphraseRejected(_)))
    ));
    assert_eq!(l.state(), Some(LeaseState::Active));
    l.identity_signer(&[0]).unwrap();
    let second = f.lease(FlowKind::ProfileEdit, &[pending]).unwrap();
    assert_eq!(second.state(), Some(LeaseState::Active));

    // A correct one revokes every lease, as before.
    change(PASS, b"new pass phrase").unwrap();
    for lease in [&l, &second] {
        assert_eq!(
            lease.state(),
            Some(LeaseState::Revoked(RevokeCause::PassphraseChange))
        );
    }
    assert_eq!(
        l.identity_signer(&[0]).unwrap_err(),
        LeaseError::Revoked(RevokeCause::PassphraseChange)
    );
    s.vault()
        .unlock(b"new pass phrase", UnlockScope::Full)
        .unwrap();
}

#[test]
fn the_key_window_parks_at_key_until_and_follows_the_proof_wait() {
    let f = fixture(true);
    f.session.lock_vault_sync().unwrap();
    let vault = f.session.vault().clone();
    let wallet = f.wallet;
    let grant = |f: &Fixture| {
        f.grant(
            GrantPurpose::PlatformOp {
                max_duffs: 1_000,
                max_credits: 0,
            },
            Credential::Passphrase(PASS),
        )
    };
    let ttl = Duration::from_millis(300);
    f.engine.block_on(async {
        let (t, _) = table_with(LeaseConfig {
            key_ttl: ttl,
            ..LeaseConfig::default()
        });
        with_journal(&t);
        t.observe_epoch(vault.epoch());
        let begin = |g: String| {
            let (vault, epoch) = (vault.clone(), vault.clone());
            t.begin(
                wallet,
                FlowKind::Registration,
                move || issue(&vault, &wallet, FlowKind::Registration, &[g]),
                move || epoch.epoch(),
            )
        };
        // Without funding: the key drops at key_until and the lease parks.
        let a = begin(grant(&f)).await.unwrap();
        a.funding_signer().unwrap();
        tokio::time::sleep(ttl + Duration::from_millis(150)).await;
        assert_eq!(a.state(), Some(LeaseState::Parked));
        assert_eq!(
            a.funding_signer().unwrap_err(),
            LeaseError::Parked(BudgetPurpose::Funding)
        );
        assert!(t.inspect(|i| i.leases[&a.id()].key.is_none()));

        // The proof wait moves key_until to its own timeout.
        let b = begin(grant(&f)).await.unwrap();
        let t2 = Arc::clone(&t);
        b.scope(async move { t2.register(wallet, art(1), 500, Vec::new()).await })
            .await
            .unwrap();
        t.proof_wait_started(wallet, art(1), ttl * 3);
        assert_eq!(b.state(), Some(LeaseState::AwaitingProof));
        tokio::time::sleep(ttl + Duration::from_millis(150)).await;
        b.funding_signer().unwrap();
        tokio::time::sleep(ttl * 2).await;
        assert_eq!(b.state(), Some(LeaseState::Parked));

        // park drops it at once.
        let c = begin(grant(&f)).await.unwrap();
        c.park();
        assert!(t.inspect(|i| i.leases[&c.id()].key.is_none()));
        let _ = status();
    });
}

#[test]
fn close_revokes_every_lease_and_closes_the_journal() {
    let f = fixture(false);
    let l = f
        .lease(FlowKind::Withdraw, &[f.platform_op(0, 1_000)])
        .unwrap();
    f.engine
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(l.state(), Some(LeaseState::Revoked(RevokeCause::Close)));
    assert!(l.table.journal.get().is_none());
}

// Review P2a r1 regressions (F3, F5).

/// What the fake library does once entered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Library {
    Accept,
    Unconfirmed,
    Hang,
}

/// The library's broadcast behind `TxDraft`'s test seam.
struct FakeLibrary {
    before: Duration,
    mode: Library,
    entered: std::sync::atomic::AtomicU32,
}

impl FakeLibrary {
    fn install(d: &crate::send::TxDraft, before: Duration, mode: Library) -> Arc<Self> {
        let lib = Arc::new(Self {
            before,
            mode,
            entered: Default::default(),
        });
        *d.broadcaster.lock().unwrap() = Some(lib.clone());
        lib
    }

    fn entered(&self) -> u32 {
        self.entered.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl crate::send::TestBroadcaster for FakeLibrary {
    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(self.before))
    }

    fn broadcast(
        &self,
        tx: &dashcore::Transaction,
        first: bool,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Txid, platform_wallet::PlatformWalletError>> + Send>,
    > {
        assert!(first, "a leased First enters the library as first");
        self.entered
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let txid = tx.txid();
        let mode = self.mode;
        Box::pin(async move {
            match mode {
                Library::Accept => Ok(txid),
                Library::Unconfirmed => Err(
                    platform_wallet::PlatformWalletError::TransactionBroadcastUnconfirmed(
                        "no verdict yet".into(),
                    ),
                ),
                Library::Hang => std::future::pending().await,
            }
        })
    }
}

fn accept_and_pay(f: &Fixture, cap: u64) -> Lease {
    let grants = [
        f.platform_op(0, 1_000),
        f.grant(GrantPurpose::Spend { max_duffs: cap }, Credential::None),
    ];
    f.lease(FlowKind::AcceptAndPay, &grants).unwrap()
}

/// F3: a leased First is handed off end to end and accepted: Sent.
#[test]
fn r1_f3_a_leased_first_hands_off_end_to_end() {
    let f = fixture(false);
    f.credit(1, COIN);
    let l = accept_and_pay(&f, 5_000_000);

    let d = f.draft(1_000_000);
    let lib = FakeLibrary::install(&d, Duration::ZERO, Library::Accept);
    let p = f.engine.block_on(d.prepare(l.id_string())).unwrap();
    let txid = ArtifactId::parse(&p.summary().txid).unwrap();
    f.engine.block_on(d.broadcast(p)).unwrap();
    assert_eq!(lib.entered(), 1);
    assert_eq!(
        f.session.leases.dispatch_status(f.wallet, txid),
        Some(DispatchState::Sent)
    );
    assert!(
        spend_of(&l).unwrap() > 1_000_000,
        "a sent payment stays charged"
    );
}

/// F3: an unconfirmed hand-off of a leased First is MaybeSent, charged,
/// its inputs held.
#[test]
fn r1_f3_an_unconfirmed_leased_first_is_maybe_sent() {
    let f = fixture(false);
    f.credit(1, COIN);
    let l = accept_and_pay(&f, 5_000_000);
    let d = f.draft(1_000_000);
    let lib = FakeLibrary::install(&d, Duration::ZERO, Library::Unconfirmed);
    let p = f.engine.block_on(d.prepare(l.id_string())).unwrap();
    let txid = ArtifactId::parse(&p.summary().txid).unwrap();
    assert!(matches!(
        send_failure(f.engine.block_on(d.broadcast(Arc::clone(&p)))),
        SendFailure::BroadcastUnknown { .. }
    ));
    assert_eq!(lib.entered(), 1);
    assert_eq!(
        f.session.leases.dispatch_status(f.wallet, txid),
        Some(DispatchState::MaybeSent)
    );
    assert!(
        spend_of(&l).unwrap() > 1_000_000,
        "MaybeSent keeps its charge"
    );
    assert!(f.reserved() > 0, "its inputs stay held");
    assert_eq!(
        send_failure(f.engine.block_on(d.abandon(p))),
        SendFailure::PreparedTxSpent
    );
}

/// F3: a First whose wait before the library outlives the permit never
/// enters it: definitely unsent, released and refunded.
#[test]
fn r1_f3_a_first_past_its_deadline_never_enters_the_library() {
    let f = fixture(false);
    f.credit(1, COIN);
    let l = accept_and_pay(&f, 5_000_000);
    let d = f.draft(1_000_000);
    let ttl = super::LeaseConfig::default().permit_ttl;
    let lib = FakeLibrary::install(&d, ttl + Duration::from_secs(1), Library::Accept);
    let p = f.engine.block_on(d.prepare(l.id_string())).unwrap();
    let txid = ArtifactId::parse(&p.summary().txid).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(d.broadcast(p))),
        SendFailure::Cancelled
    );
    assert_eq!(lib.entered(), 0, "never entered past the deadline");
    assert_eq!(
        f.session.leases.dispatch_status(f.wallet, txid),
        Some(DispatchState::NotSent)
    );
    assert_eq!(spend_of(&l), Some(0));
    assert_eq!(f.reserved(), 0);
}

/// F3: a library call still running at the deadline ends there as
/// MaybeSent, its inputs held; a lock returns by then.
#[test]
fn r1_f3_a_hanging_hand_off_ends_at_the_deadline_as_maybe_sent() {
    let f = fixture(false);
    f.credit(1, COIN);
    let l = accept_and_pay(&f, 5_000_000);
    let d = f.draft(1_000_000);
    let lib = FakeLibrary::install(&d, Duration::ZERO, Library::Hang);
    let p = f.engine.block_on(d.prepare(l.id_string())).unwrap();
    let txid = ArtifactId::parse(&p.summary().txid).unwrap();
    let ttl = super::LeaseConfig::default().permit_ttl;
    f.engine.block_on(async {
        let started = tokio::time::Instant::now();
        let send = tokio::spawn({
            let (d, p) = (Arc::clone(&d), Arc::clone(&p));
            async move { d.broadcast(p).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while lib.entered() == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        f.session.lock_vault().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), send)
            .await
            .expect("the hand-off ends at the deadline")
            .unwrap();
        assert!(matches!(
            result,
            Err(EngineError::Send(SendFailure::BroadcastUnknown { .. }))
        ));
        assert!(started.elapsed() < ttl + Duration::from_secs(1));
    });
    assert_eq!(
        f.session.leases.dispatch_status(f.wallet, txid),
        Some(DispatchState::MaybeSent)
    );
    assert!(f.reserved() > 0, "its inputs stay held");
}

/// F3 (Sol's witness): with the real SPV client, a First whose library
/// call waits on the wallet manager past the deadline is cut there.
#[test]
fn r1_f3_the_real_spv_dispatch_stops_at_the_permit_deadline() {
    let f = fixture(false);
    f.credit(1, COIN);
    let l = accept_and_pay(&f, 5_000_000);
    let d = f.draft(1_000_000);
    let p = f.engine.block_on(d.prepare(l.id_string())).unwrap();
    let txid = ArtifactId::parse(&p.summary().txid).unwrap();
    f.engine.block_on(async {
        let manager = f.session.manager().unwrap();
        // The actual SPV client, without its network run loop.
        crate::fsutil::create_owned_dir(f.session.data_dir(), std::path::Path::new("spv")).unwrap();
        manager
            .spv()
            .start(f.session.spv_config().unwrap())
            .await
            .unwrap();
        let wallet = f.session.wallet(&f.wallet).await.unwrap();
        let held = wallet.state_mut().await;
        let task = tokio::spawn({
            let d = Arc::clone(&d);
            async move { d.broadcast(p).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while l.view().unwrap().in_flight == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        f.session.lock_vault().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // The library call is dropped at the deadline and the attempt
        // finished; what still waits on the held guard is the engine's
        // send-metadata bookkeeping, which never reaches the transport.
        let attempt_live = l.view().unwrap().in_flight > 0;
        drop(held);
        let result = task.await.unwrap();
        manager.spv().stop().await.unwrap();
        assert!(!attempt_live, "the dispatch outlived its permit");
        assert_eq!(
            f.session.leases.dispatch_status(f.wallet, txid),
            Some(DispatchState::MaybeSent)
        );
        assert!(matches!(
            result,
            Err(EngineError::Send(SendFailure::BroadcastUnknown { .. }))
        ));
    });
}

/// F5: a payment exactly at its Spend cap prepares; one duff over does
/// not.
#[test]
fn r1_f5_a_payment_exactly_at_the_cap_prepares() {
    let f = fixture(false);
    f.credit(1, COIN);
    let d = f.draft(1_000_000);
    let reference = f.grant(
        GrantPurpose::Spend {
            max_duffs: 5_000_000,
        },
        Credential::None,
    );
    let p = f.engine.block_on(d.prepare(reference)).unwrap();
    let debit = p.summary().total_debit;
    f.engine.block_on(d.abandon(p)).unwrap();

    let under = accept_and_pay(&f, debit - 1);
    assert!(matches!(
        send_failure(f.engine.block_on(d.prepare(under.id_string()))),
        SendFailure::GrantExceeded { .. }
    ));
    assert_eq!(spend_of(&under), Some(0));
    let exact = accept_and_pay(&f, debit);
    let p = f.engine.block_on(d.prepare(exact.id_string())).unwrap();
    assert_eq!(spend_of(&exact), Some(debit));
    f.engine.block_on(d.abandon(p)).unwrap();
    assert_eq!(spend_of(&exact), Some(0));
}

/// F7 (Sol's real-vault interleaving): a lock whose caller is dropped
/// still gates and drains; an unlock and a grant meanwhile, then a
/// synchronous lock during the drain locks again and kills that grant.
#[test]
fn r1_f7_a_real_lock_dropped_unlocked_and_locked_again_during_its_drain() {
    let f = fixture(true);
    let l = f
        .lease(FlowKind::Withdraw, &[f.platform_op(0, 1000)])
        .unwrap();
    f.engine.block_on(async {
        let t = Arc::clone(&f.session.leases);
        let permit = super::tests::first(t.admit(super::tests::transition(&l, art(82), 10)).await);
        let k1 = tokio::spawn({
            let s = Arc::clone(&f.session);
            async move { s.lock_vault().await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.session.vault().lock_state() != LockState::Locked
                || t.with_j(|i, _| i.barrier.gates) != 0
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        // The gate and drain outlive their caller.
        k1.abort();
        let unlock = tokio::spawn({
            let s = Arc::clone(&f.session);
            async move { s.vault_op(|v| v.unlock(PASS, UnlockScope::Full)).await }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while f.session.vault().lock_state() != LockState::Unlocked {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let g = f.platform_op(0, 10);
        assert!(t.with_j(|i, _| i.barrier.drains) > 0, "K1 still drains");
        f.session.lock_vault_sync().unwrap();
        assert_eq!(f.session.vault().lock_state(), LockState::Locked);
        assert!(
            f.session
                .vault()
                .check_grant(&g, dw_vault::GrantKind::PlatformOp, Some(&f.wallet.0))
                .is_err(),
            "a grant issued before K2 dies with it"
        );
        permit.finish(Outcome::NotSent);
        unlock.await.unwrap().unwrap();
    });
}

/// E0-04 §2a.3 on a real session: a Mode B funding call's permit is quoted
/// `amount + fee_bound_worst` from the wallet's coins, reads the wallet's
/// tracked rows first, and hands out a funding signer that counts what it
/// releases and a `Locked` refusal; the facade's `dispatch_status` answers
/// the funding step id from the derivation (§2a.5) and a resumable step id
/// from its marker.
#[test]
fn mode_b_a_funding_permit_counts_its_signer_and_the_facade_reads_its_status() {
    use key_wallet::Signer as _;
    use platform_wallet::AssetLockFundingType;
    use std::str::FromStr;

    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    f.credit(1, COIN);
    f.credit(2, COIN);
    assert_eq!(f.registration_draft(0), 1);
    let s = &f.session;
    let fee = f.engine.block_on(s.fee_bound_worst(f.wallet)).unwrap();
    assert_eq!(fee, crate::fees::asset_lock_fee_bound(2));
    let dashpay = s.dashpay(f.wallet);
    assert_eq!(
        f.engine
            .block_on(dashpay.dispatch_status(STEP.into()))
            .unwrap(),
        None,
        "never registered: no marker, no row, no draft"
    );
    let grant = f.platform_op(1_000_000, 0);
    let l = f.lease(FlowKind::Registration, &[grant]).unwrap();
    let key = FundingKey {
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index: 0,
        account: 0,
    };
    let too_much = f.engine.block_on(s.funding_call_permit(
        &l,
        "create_funded_asset_lock_proof",
        1_000_000 - fee + 1,
        STEP.into(),
        key,
        f.fence(),
    ));
    assert_eq!(
        too_much.unwrap_err(),
        PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Funding,
            needed: 1_000_001,
            remaining: 1_000_000,
        }
    );
    let permit = f
        .engine
        .block_on(s.funding_call_permit(
            &l,
            "create_funded_asset_lock_proof",
            500_000,
            STEP.into(),
            key,
            f.fence(),
        ))
        .unwrap();
    let view = l.view().unwrap();
    assert!(view.funds_committed && view.call_running);
    assert_eq!(view.budgets[0].spent, 500_000 + fee, "the sum is charged");

    let signer = permit.funding_signer().unwrap();
    let path = key_wallet::DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
    f.engine.block_on(signer.public_key(&path)).unwrap();
    assert_eq!(
        permit.signatures().released(),
        0,
        "a public key is no signature"
    );
    f.engine
        .block_on(signer.sign_ecdsa(&path, [1; 32]))
        .unwrap();
    assert_eq!(permit.signatures().released(), 1);
    assert_eq!(
        f.engine
            .block_on(dashpay.dispatch_status(STEP.into()))
            .unwrap(),
        Some(DispatchState::MaybeSent)
    );
    // The vault's own lock ends the signer's epoch.
    s.vault().lock();
    assert!(matches!(
        f.engine.block_on(signer.sign_ecdsa(&path, [2; 32])),
        Err(dw_vault::SignerError::Locked)
    ));
    assert!(permit.signatures().locked());
    assert_eq!(permit.signatures().released(), 1);
    assert_eq!(permit.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(
        f.engine
            .block_on(dashpay.dispatch_status(STEP.into()))
            .unwrap(),
        Some(DispatchState::MaybeSent)
    );
    // A resumable step id reads its marker: none here.
    assert_eq!(
        f.engine
            .block_on(dashpay.dispatch_status("withdrawal/w/submit".into()))
            .unwrap(),
        None
    );
    assert!(matches!(
        f.engine
            .block_on(dashpay.dispatch_status("not a step".into())),
        Err(PlatformError::InvalidArgument { .. })
    ));
}

/// Reviews M-3, M-4: a funding permit vouches for its step's key, so it
/// refuses before anything is charged or marked a step that is no funding
/// step, a top-up step (no own key yet), a registration step whose draft
/// has no known key, and a key that is not the draft's own.
#[test]
fn mode_b_a_funding_permit_refuses_a_step_or_key_it_cannot_vouch_for() {
    use platform_wallet::AssetLockFundingType as T;
    let f = fixture(false);
    f.credit(1, COIN);
    let draft = f.registration_draft(4);
    let step = format!("registration/{draft}/funding");
    let s = &f.session;
    let l = f
        .lease(FlowKind::Registration, &[f.platform_op(10_000_000, 0)])
        .unwrap();
    let key = |funding_type, identity_index, account| FundingKey {
        funding_type,
        identity_index,
        account,
    };
    let permit = |step: &str, key: FundingKey| {
        f.engine.block_on(s.funding_call_permit(
            &l,
            "create_funded_asset_lock_proof",
            1_000_000,
            step.into(),
            key,
            f.fence(),
        ))
    };
    let refused = |step: &str, key: FundingKey, detail: &str| match permit(step, key) {
        Err(PlatformError::InvalidArgument { detail: d }) => {
            assert!(d.contains(detail), "{step}: {d}")
        }
        other => panic!("{step}: {other:?}"),
    };
    let own = key(T::IdentityRegistration, 4, 0);
    refused("withdrawal/1/submit", own, "is not a funding step");
    refused("registration/1/2/funding", own, "is not a funding step");
    refused(
        "topup/i/funding",
        key(T::IdentityTopUp, 4, 0),
        "a top-up step has no own funding key yet",
    );
    refused("registration/999/funding", own, "has no funding key");
    for wrong in [
        key(T::IdentityRegistration, 5, 0),
        key(T::IdentityRegistration, 4, 1),
        key(T::IdentityInvitation, 4, 0),
    ] {
        refused(&step, wrong, "is not the funding key");
    }
    let view = l.view().unwrap();
    assert!(!view.call_running && !view.funds_committed);
    assert!(view.budgets.iter().all(|b| b.spent == 0), "nothing charged");
    for step in [step.as_str(), "topup/i/funding", "registration/999/funding"] {
        assert_eq!(s.leases.call_step(f.wallet, step).funding, None, "{step}");
    }
    // The draft's own key: the permit.
    permit(&step, own).unwrap().finish(true);
}

/// Review M-1: a counting signer kept past its permit (a clone the library
/// still holds) signs nothing. A signature being signed when the permit
/// ends counts: the step reads `MaybeSent`, not `NotSent`.
#[test]
fn mode_b_a_signer_that_outlives_its_permit_signs_nothing() {
    use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
    use dpp::identity::signer::Signer as _;
    use dpp::identity::{IdentityPublicKey, KeyType, Purpose, SecurityLevel};
    use key_wallet::Signer as _;
    use std::str::FromStr;

    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    f.credit(1, COIN);
    f.registration_draft(0);
    let s = &f.session;
    let dashpay = s.dashpay(f.wallet);
    let l = f
        .lease(FlowKind::Registration, &[f.platform_op(10_000_000, 5_000)])
        .unwrap();
    let permit = f
        .engine
        .block_on(s.funding_call_permit(
            &l,
            "create_funded_asset_lock_proof",
            1_000_000,
            STEP.into(),
            FundingKey {
                funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
                identity_index: 0,
                account: 0,
            },
            f.fence(),
        ))
        .unwrap();
    let kept = permit.funding_signer().unwrap().clone();
    let identity = permit.identity_signer(&[0]).unwrap().clone();
    let count = permit.signatures();
    // The library is inside `sign_ecdsa` when the permit is dropped.
    let in_flight = count.begin().expect("open");
    drop(permit);
    assert_eq!(
        f.engine
            .block_on(dashpay.dispatch_status(STEP.into()))
            .unwrap(),
        Some(DispatchState::MaybeSent),
        "neither Cancelled nor NotSent"
    );
    drop(in_flight);
    // Used afterwards: refused, nothing signed.
    let path = key_wallet::DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
    assert!(matches!(
        f.engine.block_on(kept.sign_ecdsa(&path, [1; 32])),
        Err(dw_vault::SignerError::Locked)
    ));
    let key = IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id: 0,
        purpose: Purpose::AUTHENTICATION,
        security_level: SecurityLevel::HIGH,
        contract_bounds: None,
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: vec![2; 33].into(),
        disabled_at: None,
    });
    assert!(matches!(
        f.engine.block_on(identity.sign(&key, b"data")),
        Err(dpp::ProtocolError::ExternalSignerError(m)) if m == dw_vault::SignerError::Locked.to_string()
    ));
    assert!(!identity.can_sign_with(&key), "the preflight asks nothing");
    assert_eq!(count.released(), 0);
    assert_eq!(
        f.engine
            .block_on(dashpay.dispatch_status(STEP.into()))
            .unwrap(),
        Some(DispatchState::MaybeSent)
    );
}

/// Review L-3: dpp's preflight (`can_sign_with`) that meets a locked vault
/// reports `Locked` to the call permit, as `sign` does.
#[test]
fn mode_b_a_locked_preflight_counts_as_locked() {
    use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
    use dpp::identity::signer::Signer as _;
    use dpp::identity::{IdentityPublicKey, KeyType, Purpose, SecurityLevel};

    let f = fixture(false);
    let s = &f.session;
    let l = f
        .lease(FlowKind::ProfileEdit, &[f.platform_op(0, 5_000)])
        .unwrap();
    let permit = l
        .call_permit(
            "DashPay.update_profile",
            Some(CallQuote {
                purpose: BudgetPurpose::Credits,
                amount: 100,
            }),
        )
        .unwrap();
    let signer = permit.identity_signer(&[0]).unwrap();
    let key = IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id: 0,
        purpose: Purpose::AUTHENTICATION,
        security_level: SecurityLevel::HIGH,
        contract_bounds: None,
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: vec![2; 33].into(),
        disabled_at: None,
    });
    // Not the wallet's key: false, and no Locked.
    assert!(!signer.can_sign_with(&key));
    assert!(!permit.signatures().locked());
    s.vault().lock();
    assert!(!signer.can_sign_with(&key));
    assert!(permit.signatures().locked());
    assert_eq!(permit.finish(false), FlowOutcome::Cancelled);
}

/// Review L-4: a wallet created, imported or restored gets the refresh
/// and resume pass `load_wallet` gives a reopened one: its funding
/// statuses are read with no `dispatch_status` call.
#[test]
fn mode_b_an_imported_wallet_gets_its_tracked_rows_read() {
    let f = fixture(false);
    f.eventually("the imported wallet's rows read", || {
        f.session.leases.has_tracked_snapshot(f.wallet)
    });
}

/// The library's broadcast for Repair's self-spend: accepts, and records
/// what it was handed and the step's self-spends at that moment.
struct SelfSpendSeam {
    session: Arc<NetworkSession>,
    wallet: WalletId,
    step: &'static str,
    seen: std::sync::Mutex<Option<(dashcore::Transaction, Vec<[u8; 32]>)>>,
}

impl crate::send::TestBroadcaster for SelfSpendSeam {
    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(std::future::ready(()))
    }

    fn broadcast(
        &self,
        tx: &dashcore::Transaction,
        _first: bool,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Txid, platform_wallet::PlatformWalletError>> + Send>,
    > {
        let spends = self
            .session
            .leases
            .call_step(self.wallet, self.step)
            .self_spends
            .iter()
            .map(|s| s.txid)
            .collect();
        *self.seen.lock().unwrap() = Some((tx.clone(), spends));
        let txid = tx.txid();
        Box::pin(async move { Ok(txid) })
    }
}

/// E0-04 §6.5 (C9), commit 7, on a real session: Repair's "Resolve stuck
/// funding" spends every coin of the funding accounts, the inputs of a
/// tracked `Built` lock included, to a fresh address of the wallet. Its
/// txid is in the journal before the library sees the transaction, and the
/// step stays `MaybeSent` (no ChainLock yet). The exemption is the
/// repair's alone: plain coin control still refuses the lock's input, a
/// user-locked coin makes the repair refuse, and a step that is not stuck
/// is refused before anything is signed.
#[test]
fn repair_self_spends_every_coin_recorded_before_its_broadcast() {
    use platform_wallet::{AssetLockFundingType, AssetLockStatus};
    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let b = f.credit(2, COIN / 2);
    assert_eq!(f.registration_draft(0), 1);
    let s = &f.session;
    let dashpay = s.dashpay(f.wallet);
    let status = || {
        f.engine
            .block_on(dashpay.dispatch_status(STEP.into()))
            .unwrap()
    };
    let repair_draft = || f.engine.block_on(s.stuck_funding_draft(f.wallet, STEP));
    assert!(
        matches!(repair_draft(), Err(EngineError::InvalidArgument(_))),
        "never started: nothing is stuck"
    );

    // A funding call that released a signature and failed: MaybeSent.
    let l = f
        .lease(FlowKind::Registration, &[f.platform_op(10_000_000, 0)])
        .unwrap();
    let key = FundingKey {
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index: 0,
        account: 0,
    };
    let permit = f
        .engine
        .block_on(s.funding_call_permit(
            &l,
            "create_funded_asset_lock_proof",
            1_000_000,
            STEP.into(),
            key,
            f.fence(),
        ))
        .unwrap();
    assert!(
        matches!(repair_draft(), Err(EngineError::InvalidArgument(_))),
        "a call runs under the marker"
    );
    // The call signs its lock, which spends `a` (§2a.6), and fails.
    permit.signatures().note_signed([51; 32]);
    s.leases.record_signed_locks(
        f.wallet,
        &[StoredLock {
            txid: Txid::from_byte_array([51; 32]),
            inputs: vec![a],
            sighashes: Some(vec![[51; 32]]),
        }],
    );
    assert_eq!(permit.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(status(), Some(DispatchState::MaybeSent));
    // Another funding's lock, Built and never sent, holds `a` too.
    f.track_lock(50, 3, &[a], AssetLockStatus::Built);
    // The step's own lock, Built and withheld.
    f.track_lock(51, 0, &[a], AssetLockStatus::Built);
    let (leases, w) = (Arc::clone(&s.leases), f.wallet);
    let refreshed = s.on_runtime(async move { Ok(leases.refresh_funding(w).await) });
    assert!(f.engine.block_on(refreshed).unwrap());
    assert_eq!(status(), Some(DispatchState::WillBeSent));
    let plain = f.draft(1_000_000);
    plain.set_source(CoinSource::Outpoints(vec![a])).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(plain.estimate())),
        SendFailure::OutpointUnavailable(a)
    );

    // A coin the user locked is not exempt: the repair refuses it.
    f.engine
        .block_on(s.lock_outpoints(f.wallet, vec![b]))
        .unwrap();
    let draft = repair_draft().unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(draft.estimate())),
        SendFailure::OutpointUnavailable(b)
    );
    f.engine
        .block_on(s.unlock_outpoints(f.wallet, vec![b]))
        .unwrap();

    let draft = repair_draft().unwrap();
    let seam = Arc::new(SelfSpendSeam {
        session: Arc::clone(s),
        wallet: f.wallet,
        step: STEP,
        seen: Default::default(),
    });
    *draft.broadcaster.lock().unwrap() = Some(seam.clone());
    let grant = f.grant(GrantPurpose::Spend { max_duffs: 10_000 }, Credential::None);
    let out = f
        .engine
        .block_on(s.send_self_spend(&draft, STEP, grant))
        .unwrap();
    let txid: Txid = out.txid.parse().unwrap();
    let (tx, spends) = seam.seen.lock().unwrap().take().expect("broadcast");
    assert_eq!(tx.txid(), txid);
    assert_eq!(
        spends,
        vec![ArtifactId::from_txid(&txid).0],
        "recorded before the library saw it"
    );
    let mut inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    inputs.sort();
    let mut every = vec![a, b];
    every.sort();
    assert_eq!(inputs, every, "every coin, the lock's input included");
    assert_eq!(tx.output.len(), 1, "one output, no change");
    let fee = COIN + COIN / 2 - tx.output[0].value;
    assert!(fee > 0 && fee < 10_000, "{fee}");
    let own = f.engine.block_on(s.on_runtime({
        let (s, w, script) = (Arc::clone(s), f.wallet, tx.output[0].script_pubkey.clone());
        async move {
            let wallet = s.wallet(&w).await?;
            let state = wallet.state().await;
            let address =
                dashcore::Address::from_script(&script, dashcore::Network::Regtest).unwrap();
            Ok(crate::coins::wallet_owns(&state.core_wallet, &address))
        }
    }));
    assert!(own.unwrap(), "to the wallet itself");

    // Not ChainLocked: still WillBeSent, the self-spend standing.
    assert_eq!(status(), Some(DispatchState::WillBeSent));
    let spends = s.leases.call_step(f.wallet, STEP).self_spends;
    assert_eq!(spends.len(), 1);
    assert_eq!(spends[0].txid, ArtifactId::from_txid(&txid).0);
    assert_eq!(
        spends[0].conflicts,
        Some(vec![(Txid::from_byte_array([51; 32]), a)]),
        "recorded with the input it takes of the signed lock"
    );
    assert!(
        tx.input.iter().any(|i| i.previous_output == a),
        "an input of the broadcast transaction"
    );
    // The wallet sees it ChainLocked: the step reads NotSent, once.
    f.record_tx(
        tx,
        key_wallet::transaction_checking::TransactionContext::InChainLockedBlock(
            key_wallet::transaction_checking::BlockInfo::new(
                2,
                dashcore::BlockHash::all_zeros(),
                0,
            ),
        ),
        true,
    );
    let (leases, w) = (Arc::clone(&s.leases), f.wallet);
    let refreshed = s.on_runtime(async move { Ok(leases.refresh_funding(w).await) });
    assert!(f.engine.block_on(refreshed).unwrap());
    assert_eq!(status(), Some(DispatchState::NotSent));
    assert_eq!(
        f.rec.resolved(),
        vec![(STEP.to_owned(), DispatchResolution::NotSent)]
    );
}

/// E0-04 §6.5 (C9), review of commit 7: the self-spend beats a lost lock
/// only if no other spend of the wallet's coins is pending and it spends
/// an input of every tracked row of the step. Repair refuses otherwise.
#[test]
fn repair_refuses_a_self_spend_that_could_miss_the_lock() {
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
    use platform_wallet::AssetLockStatus;
    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let s = &f.session;
    let repair = || f.engine.block_on(s.stuck_funding_draft(f.wallet, STEP));
    let refused = |r: StuckFundingRefusal| match repair() {
        Err(EngineError::InvalidArgument(m)) => assert_eq!(m, r.to_string()),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("not refused: {r:?}"),
    };
    // The step's call signed lock 60 (§2a.6), whose row the wallet lost:
    // the self-spend takes `a`, one of its inputs, so it conflicts.
    let gone = OutPoint::new(Txid::from_byte_array([0x77; 32]), 0);
    f.stuck_signing(STEP, 60, &[gone, a]);

    // A spend of the wallet's coins short of its ChainLock (here in the
    // mempool) may be what the lost lock conflicts with.
    let pending = dashcore::Transaction {
        version: 2,
        lock_time: 7,
        input: vec![dashcore::TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([9; 32]), 0),
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: None,
    };
    let pending_id = pending.txid();
    f.record_tx(pending.clone(), TransactionContext::Mempool, true);
    refused(StuckFundingRefusal::SpendUnsettled(pending_id));
    // In a block, not yet ChainLocked: still.
    let block = BlockInfo::new(2, dashcore::BlockHash::all_zeros(), 0);
    f.record_tx(pending.clone(), TransactionContext::InBlock(block), true);
    refused(StuckFundingRefusal::SpendUnsettled(pending_id));
    // An incoming payment spends none of the wallet's coins.
    let incoming = dashcore::Transaction {
        lock_time: 8,
        ..pending.clone()
    };
    f.record_tx(incoming, TransactionContext::Mempool, false);
    f.record_tx(pending, TransactionContext::InChainLockedBlock(block), true);
    assert!(repair().is_ok());
    // A tracked row of the step whose every input the wallet saw spent:
    // the self-spend would not conflict with it.
    let lock = f.track_lock(61, 0, &[gone], AssetLockStatus::Broadcast);
    refused(StuckFundingRefusal::LockInputsSpent(lock));
}

/// E0-04 §2a.6 (review P2b r2 R2-01) on a real session: a tracked lock of
/// the step's key that no call of the step signed, `Built` and so
/// trusted, is no known lock of the step: Repair refuses. One the call
/// signed whose every input is spent, its row lost (the lock may have
/// confirmed), is refused too: the self-spend would not conflict with it.
#[test]
fn repair_needs_a_lock_the_steps_call_signed() {
    use platform_wallet::AssetLockStatus;
    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let s = &f.session;
    let repair = || f.engine.block_on(s.stuck_funding_draft(f.wallet, STEP));
    let refused = |r: StuckFundingRefusal| match repair() {
        Err(EngineError::InvalidArgument(m)) => assert_eq!(m, r.to_string()),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("not refused: {r:?}"),
    };
    let gone = OutPoint::new(Txid::from_byte_array([0x77; 32]), 0);
    f.stuck_signing(STEP, 70, &[gone]);
    f.track_lock(71, 0, &[a], AssetLockStatus::Built);
    // The step's own lock spends only a coin that is gone.
    refused(StuckFundingRefusal::LockInputsSpent(Txid::from_byte_array(
        [70; 32],
    )));
}

/// [`repair_needs_a_lock_the_steps_call_signed`], with no lock signed.
#[test]
fn repair_refuses_a_trusted_row_no_call_of_the_step_signed() {
    use platform_wallet::AssetLockStatus;
    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let s = &f.session;
    f.stuck(STEP);
    f.track_lock(71, 0, &[a], AssetLockStatus::Built);
    let (leases, w) = (Arc::clone(&s.leases), f.wallet);
    let refreshed = s.on_runtime(async move { Ok(leases.refresh_funding(w).await) });
    assert!(f.engine.block_on(refreshed).unwrap());
    match f.engine.block_on(s.stuck_funding_draft(f.wallet, STEP)) {
        Err(EngineError::InvalidArgument(m)) => {
            assert_eq!(m, StuckFundingRefusal::NoKnownLock.to_string())
        }
        other => panic!("{other:?}"),
    }
}

/// E0-04 §6.5 (C9): a self-spend the journal fails to record is released
/// unsent: never broadcast, its inputs free, no row.
#[test]
fn an_unrecorded_self_spend_is_released_unsent() {
    const STEP: &str = "registration/1/funding";
    let f = fixture(false);
    let a = f.credit(1, COIN);
    f.credit(2, COIN / 2);
    let s = &f.session;
    // The step's lock, Built and withheld: a known lock to conflict with.
    // It holds `a`; the self-spend takes the other coin too.
    f.stuck_signing(STEP, 51, &[a]);
    f.track_lock(51, 0, &[a], platform_wallet::AssetLockStatus::Built);
    let draft = f
        .engine
        .block_on(s.stuck_funding_draft(f.wallet, STEP))
        .unwrap();
    let seam = Arc::new(SelfSpendSeam {
        session: Arc::clone(s),
        wallet: f.wallet,
        step: STEP,
        seen: Default::default(),
    });
    *draft.broadcaster.lock().unwrap() = Some(seam.clone());
    let real = s.leases.journal.get();
    let failing = Arc::new(super::tests::FakeJournal::default());
    failing
        .fail_self_spend
        .store(true, std::sync::atomic::Ordering::SeqCst);
    s.leases.journal.set(Some(failing));
    let grant = f.grant(GrantPurpose::Spend { max_duffs: 10_000 }, Credential::None);
    let out = f.engine.block_on(s.send_self_spend(&draft, STEP, grant));
    s.leases.journal.set(real);
    assert!(matches!(out, Err(EngineError::Storage(_))), "{out:?}");
    assert!(seam.seen.lock().unwrap().is_none(), "never broadcast");
    assert_eq!(f.reserved(), 1, "its inputs released: the lock's `a` only");
    assert!(s.leases.call_step(f.wallet, STEP).self_spends.is_empty());
    // The repair runs again once the journal records.
    let grant = f.grant(GrantPurpose::Spend { max_duffs: 10_000 }, Credential::None);
    f.engine
        .block_on(s.send_self_spend(&draft, STEP, grant))
        .unwrap();
    assert!(seam.seen.lock().unwrap().is_some());
}

/// A changeset of one tracked asset-lock row stored with `status` under
/// `(IdentityRegistration, identity_index, 0)`: `Built` as the library
/// stores it after signing and before broadcasting.
fn lock_changeset(
    seed: u8,
    identity_index: u32,
    status: platform_wallet::AssetLockStatus,
) -> platform_wallet::changeset::PlatformWalletChangeSet {
    let tx = dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: Vec::new(),
        output: Vec::new(),
        special_transaction_payload: None,
    };
    lock_changeset_of(
        Txid::from_byte_array([seed; 32]),
        tx,
        identity_index,
        status,
    )
}

/// An asset lock spending `input` (a P2PKH coin of `key`) to `value`
/// duffs, its scriptSig signed by `sign` over its `SIGHASH_ALL` sighash, as
/// key-wallet's builder assembles it.
fn signed_lock_tx(
    input: OutPoint,
    value: u64,
    key: &dashcore::secp256k1::PublicKey,
    sign: impl FnOnce([u8; 32]) -> dashcore::secp256k1::ecdsa::Signature,
) -> dashcore::Transaction {
    use dashcore::blockdata::script::{Builder, PushBytesBuf};
    let mut tx = dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: input,
            ..Default::default()
        }],
        output: vec![TxOut {
            value,
            script_pubkey: dashcore::ScriptBuf::new_op_return(&[]),
        }],
        special_transaction_payload: None,
    };
    let script = dashcore::ScriptBuf::new_p2pkh(&dashcore::PublicKey::new(*key).pubkey_hash());
    let sighash = dashcore::sighash::SighashCache::new(&tx)
        .legacy_signature_hash(0, &script, 1)
        .unwrap()
        .to_byte_array();
    let mut der = sign(sighash).serialize_der().to_vec();
    der.push(0x01);
    tx.input[0].script_sig = Builder::new()
        .push_slice(PushBytesBuf::try_from(der).unwrap())
        .push_slice(key.serialize())
        .into_script();
    tx
}

/// [`lock_changeset`] of transaction `tx`, under `txid`.
fn lock_changeset_of(
    txid: Txid,
    tx: dashcore::Transaction,
    identity_index: u32,
    status: platform_wallet::AssetLockStatus,
) -> platform_wallet::changeset::PlatformWalletChangeSet {
    use platform_wallet::changeset::{AssetLockChangeSet, AssetLockEntry};
    let out_point = OutPoint::new(txid, 0);
    let mut locks = AssetLockChangeSet::default();
    locks.asset_locks.insert(
        out_point,
        AssetLockEntry {
            out_point,
            transaction: tx,
            account_index: 0,
            funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
            identity_index,
            amount_duffs: COIN,
            status,
            proof: None,
        },
    );
    platform_wallet::changeset::PlatformWalletChangeSet {
        asset_locks: Some(locks),
        ..Default::default()
    }
}

/// E0-04 §2a.6's coin fence in a funding call (DP1-02 Sol r2 R2-04): the
/// fence the policy check read the coins under moves into the permit. A
/// concurrent "Lock unspent" waits until the changeset tap stores the
/// call's own lock after its first signature (not a row of another key,
/// nor one before the call signed), of any status (the persister may fold
/// `Built` into `Broadcast`), or until the call ends. Nothing the library
/// runs inside the call waits for the fence: the tap, Mode B's refresh,
/// the fee bound and `dispatch_status` all finish while the permit holds
/// it. (A row that preceded the marker: `mode_b_tests`; under a draft's
/// own key it would block the gate.) The store records the lock the
/// call's funding signer signed as the step's own, on disk before the
/// persister takes it, and no other lock of its key (review P2b r2 R2-01).
#[test]
fn mode_b_a_funding_permit_holds_the_coin_fence_until_its_lock_is_built() {
    use std::str::FromStr;

    use key_wallet::Signer as _;
    use platform_wallet::changeset::PlatformWalletPersistence as _;
    use platform_wallet::{AssetLockFundingType as T, AssetLockStatus as A};
    let f = fixture(false);
    let a = f.credit(1, COIN);
    let b = f.credit(2, COIN);
    let s = &f.session;
    let draft = f.registration_draft(0);
    let step = format!("registration/{draft}/funding");
    let key = FundingKey {
        funding_type: T::IdentityRegistration,
        identity_index: 0,
        account: 0,
    };
    let l = f
        .lease(FlowKind::Registration, &[f.platform_op(10_000_000, 0)])
        .unwrap();
    // The policy check, under the fence the permit then holds.
    let fence = f.fence();
    let coins = f.engine.block_on(s.fenced_coins(&fence)).unwrap();
    assert_eq!(coins.held_funding_coins().count(), 0);
    let permit = f
        .engine
        .block_on(s.funding_call_permit(
            &l,
            "create_funded_asset_lock_proof",
            1_000_000,
            step.clone(),
            key,
            fence,
        ))
        .unwrap();
    assert!(s.leases.call_holds_fence(f.wallet));
    let lock = |o: OutPoint| {
        let s = Arc::clone(s);
        let w = f.wallet;
        s.rt.clone()
            .spawn(async move { s.lock_outpoints(w, vec![o]).await })
    };
    let lock_a = lock(a);
    // What the library may run inside its call does not wait for it.
    let dashpay = s.dashpay(f.wallet);
    f.engine.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            assert!(s.leases.refresh_funding(f.wallet).await);
            s.leases.request_refresh(f.wallet);
            s.fee_bound_worst(f.wallet).await.unwrap();
            dashpay.dispatch_status(step.clone()).await.unwrap();
        })
        .await
        .expect("nothing the call runs waits for its coin fence")
    });
    let store = Arc::clone(&s.live().unwrap().store);
    let stored = |cs| {
        let store = Arc::clone(&store);
        let w = f.wallet.0;
        f.engine.block_on(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                tokio::task::spawn_blocking(move || store.store(w, cs).unwrap()),
            )
            .await
        })
    };
    // A row of another key, and the call's key before it signed: not its
    // lock.
    stored(lock_changeset(62, 1, A::Built))
        .expect("the tap does not wait")
        .unwrap();
    stored(lock_changeset(61, 0, A::Built))
        .expect("the tap does not wait")
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!lock_a.is_finished(), "the lock waits for the call's fence");
    assert!(s.leases.call_holds_fence(f.wallet));
    // The call's own lock, signed by its funding signer (the vault), its
    // `Built` folded into `Broadcast`: released, the call still runs. The
    // store recorded it as the step's signed lock (§2a.6) before the
    // persister took it, durably.
    let signer = permit.funding_signer().unwrap();
    let path = key_wallet::DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
    let public = f.engine.block_on(signer.public_key(&path)).unwrap();
    // Signed as the library signs it: key-wallet's `TransactionSigner`
    // over a type-8 lock, the coin's own script as its script code.
    let own = {
        use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
        use dashcore::blockdata::transaction::special_transaction::asset_lock::AssetLockPayload;
        use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionSigner;
        let key = dashcore::PublicKey::new(public);
        let address = dashcore::Address::p2pkh(&key, dashcore::Network::Testnet);
        let tx = dashcore::Transaction {
            version: 3,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: a,
                ..Default::default()
            }],
            output: vec![TxOut {
                value: COIN - 1_000,
                script_pubkey: dashcore::ScriptBuf::new_op_return(&[]),
            }],
            special_transaction_payload: Some(TransactionPayload::AssetLockPayloadType(
                AssetLockPayload {
                    version: 1,
                    credit_outputs: vec![TxOut {
                        value: COIN - 1_000,
                        script_pubkey: dashcore::ScriptBuf::new_p2pkh(&key.pubkey_hash()),
                    }],
                },
            )),
        };
        let coin = Utxo::new(
            a,
            TxOut {
                value: COIN,
                script_pubkey: address.script_pubkey(),
            },
            address,
            1,
            false,
        );
        f.engine
            .block_on(signer.sign_tx(tx, vec![coin], |_| Some(path.clone())))
            .unwrap()
    };
    let own_id = own.txid();
    let on_disk = || -> Vec<(Vec<u8>, Vec<u8>)> {
        let db = rusqlite::Connection::open(
            f.session
                .data_dir()
                .join(dw_appdb::dispatch::DISPATCH_DB_FILE),
        )
        .unwrap();
        let mut q = db
            .prepare("SELECT txid, inputs FROM signed_lock WHERE step_id = ?1")
            .unwrap();
        q.query_map([&step], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut input = a.txid.to_byte_array().to_vec();
    input.extend_from_slice(&a.vout.to_le_bytes());
    let row = vec![(own_id.to_byte_array().to_vec(), input)];
    // The persister refuses the lock's store (its database is locked by
    // another writer), which the library logs before it broadcasts the lock
    // all the same: the signed lock is durable, written before the
    // persister was asked, and is then the lock's only record.
    {
        let db =
            rusqlite::Connection::open(f.session.data_dir().join(crate::session::WALLET_DB_FILE))
                .unwrap();
        db.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let store = Arc::clone(&store);
        let (w, cs) = (
            f.wallet.0,
            lock_changeset_of(own_id, own.clone(), 0, A::Built),
        );
        let refused = f
            .engine
            .block_on(async move {
                tokio::time::timeout(
                    Duration::from_secs(60),
                    tokio::task::spawn_blocking(move || store.store(w, cs)),
                )
                .await
            })
            .expect("the persister's busy timeout ends the store")
            .unwrap();
        assert!(refused.is_err(), "the persister refused the store");
        db.execute_batch("ROLLBACK").unwrap();
    }
    assert_eq!(on_disk(), row, "durable though the persister refused");
    assert!(!lock_a.is_finished(), "a refused store releases no fence");
    stored(lock_changeset_of(own_id, own.clone(), 0, A::Broadcast))
        .expect("the tap does not wait")
        .unwrap();
    f.engine.block_on(lock_a).unwrap().unwrap();
    assert!(!s.leases.call_holds_fence(f.wallet));
    assert!(l.view().unwrap().call_running);
    let signed = |w: WalletId, step: &str| {
        s.leases.with_j(|i, _| {
            (i.fence.signed_locks(&w, step).iter())
                .map(|l| (l.txid, l.inputs.clone()))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(signed(f.wallet, &step), vec![(own_id, vec![a])]);
    assert_eq!(on_disk(), row, "written once");
    // A lock of the call's key that the call did not sign is not the
    // step's: another signer's, or the call's signature copied onto other
    // outputs (it no longer verifies).
    let other_key = dashcore::secp256k1::SecretKey::from_secret_bytes([7; 32]).unwrap();
    let other = dashcore::secp256k1::PublicKey::from_secret_key(&other_key);
    let foreign = signed_lock_tx(b, COIN, &other, |sighash| {
        dashcore::secp256k1::ecdsa::sign(
            dashcore::secp256k1::Message::from_digest(sighash),
            &other_key,
        )
    });
    let mut copied = own.clone();
    copied.output[0].value = COIN / 2;
    for tx in [foreign, copied] {
        stored(lock_changeset_of(tx.txid(), tx, 0, A::Built))
            .expect("the tap does not wait")
            .unwrap();
    }
    assert_eq!(signed(f.wallet, &step).len(), 1);
    assert_eq!(permit.finish(false), FlowOutcome::MaybeSent);
    // A call that never tracks a lock: released at its end.
    let draft = f.registration_draft(1);
    let l = f
        .lease(FlowKind::Registration, &[f.platform_op(10_000_000, 0)])
        .unwrap();
    let permit = f
        .engine
        .block_on(s.funding_call_permit(
            &l,
            "create_funded_asset_lock_proof",
            1_000_000,
            format!("registration/{draft}/funding"),
            FundingKey {
                identity_index: 1,
                ..key
            },
            f.fence(),
        ))
        .unwrap();
    let lock_b = lock(b);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!lock_b.is_finished(), "the lock waits for the call's fence");
    drop(permit);
    f.engine.block_on(lock_b).unwrap().unwrap();
    assert!(!s.leases.call_holds_fence(f.wallet));

    // A refused permit releases the fence with its refusal.
    let refused = f.engine.block_on(s.funding_call_permit(
        &l,
        "create_funded_asset_lock_proof",
        1_000_000,
        "withdrawal/1/submit".into(),
        key,
        f.fence(),
    ));
    assert!(matches!(
        refused,
        Err(PlatformError::InvalidArgument { .. })
    ));
    f.engine
        .block_on(s.unlock_outpoints(f.wallet, vec![a, b]))
        .unwrap();

    // The call signed that index-0 lock: once the wallet holds it
    // ChainLocked, the step's `Sent` row is journaled though the row's
    // index is untrusted (review P2b r1 F1, r2 R2-01).
    f.track_lock_tx(own_id, own, 0, A::ChainLocked);
    let (leases, w) = (Arc::clone(&s.leases), f.wallet);
    let refreshed = s.on_runtime(async move { Ok(leases.refresh_funding(w).await) });
    assert!(f.engine.block_on(refreshed).unwrap());
    let id = ArtifactId::of_step(&f.wallet, &step);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while s
        .leases
        .with_j(|i, _| i.fence.evidence_mark(&f.wallet, &id))
        != Some(super::fence::Mark::Durable)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the step's Sent row was never written"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
