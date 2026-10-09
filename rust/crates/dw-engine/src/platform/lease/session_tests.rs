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
use crate::platform::flows::{BudgetPurpose, DispatchState, FlowKind, LeaseStateView, RevokeCause};
use crate::send::{Recipient, SendFailure};
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
