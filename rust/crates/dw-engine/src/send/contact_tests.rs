//! Contact payments (DP3-01) offline: a real session and vault whose
//! wallet holds our identity, its contacts and their DashPay accounts, the
//! broadcast faked behind `TxDraft`'s seam. The estimate's placeholder, the
//! order of `prepare` (nothing consumed before the reservation), the
//! address kept for a re-prepare, the per-contact lock and its evidence,
//! "Accept and pay" and a lock before the hand-off.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, TxOut, Txid};
use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::Utxo;
use key_wallet::account::AccountType;
use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};
use key_wallet::managed_account::ManagedCoreFundsAccount;
use platform_wallet::{ContactRequest, EstablishedContact};
use zeroize::Zeroizing;

use super::*;
use crate::platform::lease::ArtifactId;
use crate::platform::{
    ContactError, DispatchState, Eligibility, FlowKind, LockResolution, PlatformError,
};
use crate::{DashNetwork, Engine, EngineConfig, EventSink, ImportOptions};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const COIN: u64 = 100_000_000;
const AMOUNT: u64 = 10_000_000;
/// Room for the fee of a one-input payment.
const FEE_ROOM: u64 = 10_000;
const PASS: &[u8] = b"pass phrase";
/// Our identity, an established contact, one whose request waits, and a
/// stranger.
const ME: u8 = 1;
const BOB: u8 = 2;
const CAROL: u8 = 3;
const DAVE: u8 = 4;

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

fn id(n: u8) -> Identifier {
    Identifier::from([n; 32])
}

fn b58(n: u8) -> String {
    id(n).to_string(Encoding::Base58)
}

fn request(sender: u8, recipient: u8, created_at: u64) -> ContactRequest {
    ContactRequest::new(
        id(sender),
        id(recipient),
        0,
        0,
        0,
        vec![1; 96],
        1,
        created_at,
    )
}

/// The external account's xpub paying contact `n` (a test key).
fn contact_xpub(n: u8) -> ExtendedPubKey {
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[100 + n; 32]).unwrap();
    ExtendedPubKey::from_priv(&master)
}

/// The library's broadcast, faked: what each attempt returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Net {
    Accept,
    NoPeers,
    Unconfirmed,
    /// Unconfirmed once `gate` is notified.
    Hold,
}

struct Seam {
    mode: std::sync::Mutex<Net>,
    entered: AtomicU32,
    gate: Arc<tokio::sync::Notify>,
}

impl Seam {
    fn install(d: &TxDraft, mode: Net) -> Arc<Self> {
        let seam = Arc::new(Self {
            mode: std::sync::Mutex::new(mode),
            entered: AtomicU32::new(0),
            gate: Arc::new(tokio::sync::Notify::new()),
        });
        *d.broadcaster.lock().unwrap() = Some(seam.clone());
        seam
    }

    fn set(&self, mode: Net) {
        *self.mode.lock().unwrap() = mode;
    }
}

impl TestBroadcaster for Seam {
    fn before_entry(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn broadcast(
        &self,
        tx: &Transaction,
        _first: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Txid, PlatformWalletError>> + Send>> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let txid = tx.txid();
        let mode = *self.mode.lock().unwrap();
        let gate = Arc::clone(&self.gate);
        Box::pin(async move {
            if mode == Net::Hold {
                gate.notified().await;
            }
            match mode {
                Net::Accept => Ok(txid),
                Net::NoPeers => Err(PlatformWalletError::TransactionBroadcast(
                    "not connected".into(),
                )),
                Net::Unconfirmed | Net::Hold => Err(
                    PlatformWalletError::TransactionBroadcastUnconfirmed("no verdict yet".into()),
                ),
            }
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

/// Our identity with Bob established (his paying account held) and
/// Carol's request waiting; one confirmed coin.
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
                no_platform: true,
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
    let f = Fixture {
        _dir: dir,
        engine,
        session,
        wallet,
    };
    f.give_me();
    f.establish(BOB, false);
    f.with_wallet(|w| async move {
        let mut state = w.state_mut().await;
        let me = state
            .identity_manager
            .managed_identity_mut(&id(ME))
            .unwrap();
        me.apply_incoming_contact_request(request(CAROL, ME, 3_000_000));
    });
    f.credit(1, COIN);
    f
}

impl Fixture {
    /// Runs `f` on the session's runtime with the wallet.
    fn with_wallet<R, Fut>(&self, f: impl FnOnce(Arc<PlatformWallet>) -> Fut + Send + 'static) -> R
    where
        R: Send + 'static,
        Fut: Future<Output = R> + Send,
    {
        let (session, id) = (Arc::clone(&self.session), self.wallet);
        let inner = Arc::clone(&session);
        self.engine
            .block_on(session.on_runtime(async move { Ok(f(inner.wallet(&id).await?).await) }))
            .unwrap()
    }

    fn give_me(&self) {
        let id_ = self.wallet;
        self.with_wallet(move |w| async move {
            (w.state_mut().await.identity_manager)
                .add_identity(
                    Identity::V0(IdentityV0 {
                        id: id(ME),
                        public_keys: Default::default(),
                        balance: 0,
                        revision: 0,
                    }),
                    0,
                    id_.0,
                    w.persister(),
                )
                .unwrap();
        });
    }

    /// Contact `n` established with its paying account, as the library
    /// leaves an accept: the request pair, the immutable account carrying
    /// the contact's xpub and its managed pool. `broken`: its channel.
    fn establish(&self, n: u8, broken: bool) {
        let manager = self.session.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (w, info) = wm.get_wallet_mut_and_info_mut(&wallet.0).unwrap();
            let account_type = AccountType::DashpayExternalAccount {
                index: 0,
                user_identity_id: [ME; 32],
                friend_identity_id: [n; 32],
            };
            let xpub = contact_xpub(n);
            w.add_account(account_type, Some(xpub)).unwrap();
            let managed = ManagedCoreFundsAccount::from_account(&key_wallet::Account {
                parent_wallet_id: Some(wallet.0),
                account_type,
                network: dashcore::Network::Regtest,
                account_xpub: xpub,
                is_watch_only: true,
            });
            (info.core_wallet.accounts)
                .insert_funds_bearing_account(managed)
                .unwrap();
            let me = (info.identity_manager)
                .managed_identity_mut(&id(ME))
                .unwrap();
            let mut c = EstablishedContact::new(
                id(n),
                request(ME, n, 1_000_000),
                request(n, ME, 2_000_000),
            );
            c.external_account_reference = Some(c.incoming_request.account_reference);
            c.payment_channel_broken = broken;
            me.apply_established_contact(c);
        });
    }

    /// Contact `n`'s DIP-15 address at `index` (the external account's
    /// derivation).
    fn contact_address(&self, n: u8, index: u32) -> Address {
        let child = (contact_xpub(n))
            .derive_pub(&[key_wallet::bip32::ChildNumber::Normal { index }])
            .unwrap();
        Address::p2pkh(
            &dashcore::PublicKey::new(child.public_key),
            dashcore::Network::Regtest,
        )
    }

    fn credit(&self, seed: u8, value: u64) -> OutPoint {
        self.with_wallet(move |w| async move {
            let address = w.core().next_receive_address_for_account(0).await.unwrap();
            let outpoint = OutPoint::new(Txid::from_byte_array([seed; 32]), 0);
            let txout = TxOut {
                value,
                script_pubkey: address.script_pubkey(),
            };
            let mut utxo = Utxo::new(outpoint, txout, address, 1, false);
            utxo.is_confirmed = true;
            (w.state_mut()
                .await
                .core_wallet
                .accounts
                .standard_bip44_accounts)
                .get_mut(&0)
                .expect("BIP44 account 0")
                .utxos
                .insert(outpoint, utxo);
            outpoint
        })
    }

    /// Removes a coin from key-wallet's unspent set, as when the wallet
    /// sees a transaction spend it.
    fn see_spent(&self, outpoint: OutPoint) {
        self.with_wallet(move |w| async move {
            (w.state_mut()
                .await
                .core_wallet
                .accounts
                .standard_bip44_accounts)
                .get_mut(&0)
                .expect("BIP44 account 0")
                .utxos
                .remove(&outpoint);
        });
    }

    fn grant(&self, purpose: GrantPurpose) -> String {
        (self.session.vault())
            .authorize(purpose, Some(&self.wallet.0), Credential::None)
            .unwrap()
            .id
    }

    fn spend(&self, max_duffs: u64) -> String {
        self.grant(GrantPurpose::Spend { max_duffs })
    }

    fn pay(&self, to: u8, amount: u64) -> ContactRecipient {
        ContactRecipient {
            identity: b58(ME),
            contact: b58(to),
            amount,
            subtract_fee: false,
            note: Some("lunch".into()),
        }
    }

    fn draft(&self, to: u8) -> Arc<TxDraft> {
        let d = self.session.new_tx_draft(self.wallet).unwrap();
        d.set_contact_recipient(self.pay(to, AMOUNT)).unwrap();
        d
    }

    /// The address reserved for `to` that no broadcast handed out.
    fn reserved(&self, to: u8) -> Option<Address> {
        (self.session.platform.payments).reserved(self.wallet, (id(ME), id(to)))
    }

    fn lock(&self, to: u8) -> Option<crate::platform::PaymentLock> {
        (self.session)
            .payment_lock_of(self.wallet, &b58(ME), &b58(to))
            .unwrap()
    }

    fn resolve(&self, to: u8) -> LockResolution {
        (self.engine)
            .block_on(
                self.session
                    .resolve_payment_lock_of(self.wallet, b58(ME), b58(to)),
            )
            .unwrap()
    }
}

/// The output paying the contact.
fn paid(p: &PreparedTx) -> String {
    let out = p.summary().outputs.iter().find(|o| !o.is_change).unwrap();
    out.address.clone().unwrap()
}

fn send_failure<T: std::fmt::Debug>(r: Result<T, EngineError>) -> SendFailure {
    match r {
        Err(EngineError::Send(f)) => f,
        other => panic!("expected a send failure, got {other:?}"),
    }
}

fn contact_error<T: std::fmt::Debug>(r: Result<T, EngineError>) -> ContactError {
    match r {
        Err(EngineError::Contact(e)) => e,
        other => panic!("expected a contact error, got {other:?}"),
    }
}

/// D2: the setter's offline checks.
#[test]
fn the_setter_validates_offline() {
    let f = fixture(false);
    let d = f.session.new_tx_draft(f.wallet).unwrap();
    let set = |to: u8, amount: u64| d.set_contact_recipient(f.pay(to, amount));
    assert!(matches!(
        contact_error(set(ME, AMOUNT)),
        ContactError::IsSelf
    ));
    assert!(matches!(
        contact_error(set(DAVE, AMOUNT)),
        ContactError::Platform(PlatformError::InvalidArgument { .. })
    ));
    assert_eq!(
        send_failure(set(BOB, 100)),
        SendFailure::DustAmount { index: 0 }
    );
    assert_eq!(
        send_failure(set(BOB, 0)),
        SendFailure::InvalidAmount { index: 0 }
    );
    let bad = d.set_contact_recipient(ContactRecipient {
        contact: "not base58!".into(),
        ..f.pay(BOB, AMOUNT)
    });
    assert!(matches!(
        contact_error(bad),
        ContactError::Platform(PlatformError::InvalidArgument { .. })
    ));
    let unknown = d.set_contact_recipient(ContactRecipient {
        identity: b58(9),
        ..f.pay(BOB, AMOUNT)
    });
    assert!(
        matches!(
            contact_error(unknown),
            ContactError::Platform(PlatformError::Identity(_))
        ),
        "an identity of another wallet"
    );
    // A pending incoming request passes the setter ("Accept and pay").
    set(CAROL, AMOUNT).unwrap();
    set(BOB, AMOUNT).unwrap();
    // `set_recipients` replaces the contact.
    d.set_recipients(vec![Recipient {
        address: f.contact_address(BOB, 7).to_string(),
        amount: AMOUNT,
        subtract_fee_from_amount: false,
        label: None,
        message: None,
    }])
    .unwrap();
    assert_eq!(d.state().contact, None);

    f.establish(DAVE, true);
    assert!(matches!(
        contact_error(set(DAVE, AMOUNT)),
        ContactError::ChannelBroken
    ));
}

/// D3/D4: the estimate's fee is the payment's; the payment pays the
/// contact's next address, counted as leaving the wallet; the note is the
/// transaction's message and the address book stays empty (D10).
#[test]
fn the_estimate_is_the_payment() {
    let f = fixture(false);
    let d = f.draft(BOB);
    let estimate = f.engine.block_on(d.estimate()).unwrap();
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let s = p.summary();
    // The planned size, so the fee; the signed size varies with the
    // signatures' encodings.
    assert_eq!(s.fee, estimate.fee);
    assert_eq!(paid(&p), f.contact_address(BOB, 0).to_string());
    let out = s.outputs.iter().find(|o| !o.is_change).unwrap();
    assert!(!out.is_mine, "a contact payment leaves the wallet");
    assert_eq!(s.total_debit, AMOUNT + s.fee);

    let seam = Seam::install(&d, Net::Accept);
    f.engine.block_on(d.broadcast(Arc::clone(&p))).unwrap();
    assert_eq!(seam.entered.load(Ordering::SeqCst), 1);
    // The wallet sees its coin spent; another funds the next payment.
    f.see_spent(s.inputs[0].outpoint);
    f.credit(2, COIN);
    let (w, txid, address) = (f.wallet.to_string(), s.txid.clone(), paid(&p));
    let (message, book) = f
        .engine
        .block_on(
            f.session
                .appdb_op(move |db| Ok((db.tx_message(&w, &txid)?, db.book_entry(&w, &address)?))),
        )
        .unwrap();
    assert_eq!(message.as_deref(), Some("lunch"));
    assert!(book.is_none(), "no address-book entry for a DIP-15 address");
    // Sent: the next payment pays the next address.
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    assert_eq!(paid(&p), f.contact_address(BOB, 1).to_string());
}

/// D4: a refusal before the reservation consumes no address: a grant too
/// small, one of another purpose, a pending request.
#[test]
fn a_refusal_before_the_reservation_takes_no_address() {
    let f = fixture(false);
    let d = f.draft(BOB);
    assert!(matches!(
        send_failure(f.engine.block_on(d.prepare(f.spend(AMOUNT)))),
        SendFailure::GrantExceeded { .. }
    ));
    let platform = f.grant(GrantPurpose::PlatformOp {
        max_duffs: COIN,
        max_credits: 0,
    });
    assert_eq!(
        send_failure(f.engine.block_on(d.prepare(platform))),
        SendFailure::GrantInvalid
    );
    let carol = f.draft(CAROL);
    assert!(matches!(
        contact_error(f.engine.block_on(carol.prepare(f.spend(COIN)))),
        ContactError::Ineligible {
            reason: Eligibility::PendingIncoming
        }
    ));
    assert!(f.reserved(BOB).is_none());
    // A new draft gets the first address: none was taken.
    let p = (f.engine)
        .block_on(f.draft(BOB).prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    assert_eq!(paid(&p), f.contact_address(BOB, 0).to_string());
}

/// D7: `send.no_peers` releases the payment; a re-prepare with a new grant
/// pays the same address. A second payment of the draft prepared before
/// the first went out is spent once the first took the address.
#[test]
fn a_no_peers_re_prepare_reuses_the_address() {
    let f = fixture(false);
    let d = f.draft(BOB);
    let seam = Seam::install(&d, Net::NoPeers);
    let first = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(d.broadcast(Arc::clone(&first)))),
        SendFailure::NoPeers
    );
    let again = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    assert_eq!(paid(&again), paid(&first));
    assert_eq!(paid(&again), f.contact_address(BOB, 0).to_string());

    f.credit(2, COIN);
    let twin = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    assert_eq!(paid(&twin), paid(&again));
    seam.set(Net::Accept);
    f.engine.block_on(d.broadcast(again)).unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(d.broadcast(twin))),
        SendFailure::PreparedTxSpent,
        "an address goes out once"
    );
}

/// m1 §2.7.1: a fee grown past the authorized cap is `send.grant_exceeded`.
#[test]
fn a_grown_fee_is_grant_exceeded() {
    let f = fixture(false);
    let d = f.draft(BOB);
    let estimate = f.engine.block_on(d.estimate()).unwrap();
    let cap = AMOUNT + estimate.fee;
    d.set_fee(FeeMode::PerKb(5_000)).unwrap();
    assert!(matches!(
        send_failure(f.engine.block_on(d.prepare(f.spend(cap)))),
        SendFailure::GrantExceeded { max_duffs, .. } if max_duffs == cap
    ));
    assert!(f.reserved(BOB).is_none());
}

/// D8/D9: an unknown outcome locks the contact, in memory and on disk;
/// every new payment to it is refused; a repeat broadcast of the locking
/// transaction proceeds and its acceptance clears the lock.
#[test]
fn an_unknown_outcome_locks_the_contact() {
    let f = fixture(false);
    let d = f.draft(BOB);
    let seam = Seam::install(&d, Net::Unconfirmed);
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let txid = p.summary().txid.clone();
    assert!(matches!(
        send_failure(f.engine.block_on(d.broadcast(Arc::clone(&p)))),
        SendFailure::BroadcastUnknown { .. }
    ));
    assert_eq!(f.lock(BOB).unwrap().txid, txid);
    assert!(f.reserved(BOB).is_none(), "the address may be out");
    let locked =
        |r| matches!(contact_error(r), ContactError::PaymentLocked { txid: t } if t == txid);
    assert!(locked(
        f.session
            .new_tx_draft(f.wallet)
            .unwrap()
            .set_contact_recipient(f.pay(BOB, AMOUNT))
    ));
    assert!(locked(f.engine.block_on(d.estimate()).map(|_| ())));
    assert!(locked(
        f.engine
            .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
            .map(|_| ())
    ));
    // Other contacts are not locked.
    f.establish(DAVE, false);
    f.draft(DAVE);
    // On disk: a reload keeps it.
    f.session.platform.payments.forget(&f.wallet);
    assert!(f.lock(BOB).is_none());
    f.engine.block_on(f.session.load_payment_locks(f.wallet));
    assert_eq!(f.lock(BOB).unwrap().txid, txid);
    // No evidence either way.
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);
    assert!(f.lock(BOB).is_some());

    seam.set(Net::Accept);
    f.engine.block_on(d.broadcast(p)).unwrap();
    assert!(f.lock(BOB).is_none());
    let w = f.wallet.to_string();
    let rows = (f.engine)
        .block_on(f.session.appdb_op(move |db| db.dp_payment_locks(&w)))
        .unwrap();
    assert!(rows.is_empty());
}

/// D9: the fence's word settles a lock: a sighting is `Sent`.
#[test]
fn resolve_reads_the_fence() {
    let f = fixture(false);
    let d = f.draft(BOB);
    Seam::install(&d, Net::Unconfirmed);
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let txid = p.summary().txid.clone();
    let _ = f.engine.block_on(d.broadcast(p));
    let artifact = ArtifactId::parse(&txid).unwrap();
    assert_eq!(
        f.session.leases.dispatch_status(f.wallet, artifact),
        Some(DispatchState::MaybeSent)
    );
    f.session.leases.note_seen(f.wallet, artifact);
    assert_eq!(f.resolve(BOB), LockResolution::Sent);
    assert!(f.lock(BOB).is_none());
    assert!(matches!(
        f.engine.block_on(
            f.session
                .resolve_payment_lock_of(f.wallet, b58(ME), b58(BOB))
        ),
        Err(ContactError::Platform(
            PlatformError::InvalidArgument { .. }
        ))
    ));
}

/// D20: while one payment's first broadcast is under way, no other
/// payment to the contact starts: a second payment prepared before it (the
/// same reserved address) is refused, and so is a new one; the first's
/// unknown outcome then locks the contact behind it.
#[test]
fn a_payment_under_way_holds_the_contact() {
    let f = fixture(false);
    let d = f.draft(BOB);
    let seam = Seam::install(&d, Net::Hold);
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let txid = p.summary().txid.clone();
    f.credit(2, COIN);
    let other = f.draft(BOB);
    let twin_seam = Seam::install(&other, Net::Accept);
    let twin = (f.engine)
        .block_on(other.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    assert_eq!(paid(&twin), paid(&p), "one reserved address per contact");

    let broadcast = f.session.rt.spawn({
        let (d, p) = (Arc::clone(&d), Arc::clone(&p));
        async move { d.broadcast(p).await }
    });
    let start = std::time::Instant::now();
    while seam.entered.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < std::time::Duration::from_secs(30));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let held = |r| matches!(contact_error(r), ContactError::PaymentLocked { txid: t } if t == txid);
    assert!(held(
        f.engine
            .block_on(other.broadcast(Arc::clone(&twin)))
            .map(|_| ())
    ));
    assert!(held(
        f.session
            .new_tx_draft(f.wallet)
            .unwrap()
            .set_contact_recipient(f.pay(BOB, AMOUNT))
    ));
    assert!(f.lock(BOB).is_none(), "under way, not locked");

    seam.gate.notify_one();
    let outcome = f.engine.block_on(broadcast).unwrap();
    assert!(matches!(
        send_failure(outcome),
        SendFailure::BroadcastUnknown { .. }
    ));
    assert_eq!(f.lock(BOB).unwrap().txid, txid);
    assert!(held(f.engine.block_on(other.broadcast(twin)).map(|_| ())));
    assert_eq!(twin_seam.entered.load(Ordering::SeqCst), 0);
}

/// D9/D20: the lock keeps its payment's inputs on disk, so after a restart
/// that lost the wallet's record of the payment, a ChainLocked spend of
/// one of them still proves it unsent.
#[test]
fn a_conflict_proves_a_lock_unsent_after_a_restart() {
    use key_wallet::account::StandardAccountType;
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};

    let f = fixture(false);
    let d = f.draft(BOB);
    Seam::install(&d, Net::Unconfirmed);
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let coin = p.summary().inputs[0].outpoint;
    let _ = f.engine.block_on(d.broadcast(Arc::clone(&p)));
    assert!(f.lock(BOB).is_some());
    drop((d, p));

    f.session.platform.payments.restart();
    f.engine.block_on(f.session.load_payment_locks(f.wallet));
    assert!(f.lock(BOB).is_some());
    assert_eq!(f.resolve(BOB), LockResolution::Unknown);

    let conflict = Transaction {
        version: 2,
        lock_time: 7,
        input: vec![dashcore::TxIn {
            previous_output: coin,
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: None,
    };
    let block = BlockInfo::new(2, dashcore::BlockHash::all_zeros(), 0);
    let record = TransactionRecord::new(
        conflict,
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        TransactionContext::InChainLockedBlock(block),
        TransactionType::Standard,
        TransactionDirection::Internal,
        Vec::new(),
        Vec::new(),
        0,
    );
    (f.session.hub.history).with_wallet(f.wallet, |h| h.upsert(&record, None));
    assert_eq!(f.resolve(BOB), LockResolution::NotSent);
    assert!(f.lock(BOB).is_none());
}

/// D12: the CoinJoin source applies to a contact payment (the wallet has
/// no fully mixed coins here), and subtract-fee pays the fee from the
/// amount, the cap still `amount`.
#[test]
fn coin_sources_and_subtract_fee_apply() {
    let f = fixture(false);
    let d = f.draft(BOB);
    d.set_source(CoinSource::FullyMixedOnly).unwrap();
    assert!(matches!(
        send_failure(f.engine.block_on(d.estimate())),
        SendFailure::InsufficientMixedFunds { available: 0 }
    ));
    d.set_source(CoinSource::Any).unwrap();
    d.set_contact_recipient(ContactRecipient {
        subtract_fee: true,
        ..f.pay(BOB, AMOUNT)
    })
    .unwrap();
    let p = f.engine.block_on(d.prepare(f.spend(AMOUNT))).unwrap();
    assert_eq!(p.summary().total_debit, AMOUNT);
    assert_eq!(p.summary().total_sent, AMOUNT - p.summary().fee);
}

/// D14: "Accept and pay" under one `AcceptAndPay` lease: refused before
/// the accept, with nothing charged or reserved; once the accept
/// established the contact, the same lease pays it.
#[test]
fn accept_and_pay_spends_only_after_the_accept() {
    let f = fixture(false);
    let lease = (f.engine)
        .block_on(f.session.begin_lease(
            f.wallet,
            FlowKind::AcceptAndPay,
            &[
                f.grant(GrantPurpose::PlatformOp {
                    max_duffs: 0,
                    max_credits: 1_000,
                }),
                f.spend(AMOUNT + FEE_ROOM),
            ],
        ))
        .unwrap();
    let d = f.draft(CAROL);
    assert!(matches!(
        contact_error(f.engine.block_on(d.prepare(lease.id_string()))),
        ContactError::Ineligible {
            reason: Eligibility::PendingIncoming
        }
    ));
    assert!(f.reserved(CAROL).is_none());
    // The accept, as the library leaves it.
    f.establish(CAROL, false);
    let p = f.engine.block_on(d.prepare(lease.id_string())).unwrap();
    assert_eq!(paid(&p), f.contact_address(CAROL, 0).to_string());
    Seam::install(&d, Net::Accept);
    f.engine.block_on(d.broadcast(p)).unwrap();

    // Another flow's lease cannot pay a contact.
    let send = (f.engine)
        .block_on(f.session.begin_lease(
            f.wallet,
            FlowKind::Registration,
            &[f.grant(GrantPurpose::PlatformOp {
                max_duffs: COIN,
                max_credits: 0,
            })],
        ))
        .unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(f.draft(BOB).prepare(send.id_string()))),
        SendFailure::GrantInvalid
    );
}

/// D5/D15: a lock before the hand-off cancels the payment (the one-payment
/// lease is revoked); its address stays reserved, so the payment made
/// after an unlock pays it.
#[test]
fn a_lock_before_the_hand_off_cancels() {
    let f = fixture(true);
    let d = f.draft(BOB);
    let seam = Seam::install(&d, Net::Accept);
    let p = (f.engine)
        .block_on(d.prepare(f.spend(AMOUNT + FEE_ROOM)))
        .unwrap();
    let address = paid(&p);
    f.session.lock_vault_sync().unwrap();
    assert_eq!(
        send_failure(f.engine.block_on(d.broadcast(p))),
        SendFailure::Cancelled
    );
    assert_eq!(seam.entered.load(Ordering::SeqCst), 0);
    assert!(f.lock(BOB).is_none(), "definitely unsent: no lock");
    assert_eq!(f.reserved(BOB).map(|a| a.to_string()), Some(address));
    // Locked: refused before any grant is looked at.
    assert_eq!(
        send_failure(f.engine.block_on(d.prepare("g".into()))),
        SendFailure::VaultLocked
    );
}

/// T2 on the testnet fixture (`DW_DP301_TESTNET_FIXTURE`, wallets A and B,
/// established contacts), both restored from their phrases. The first run
/// pays B as A's contact (100,000 duffs): estimate, a `Spend` grant of
/// amount plus fee, prepare, abandon, prepare again with a new grant (the
/// same address), then broadcast; the txid is recorded in `DP3-01.json`
/// before the broadcast. Every run then checks that A sent it and B
/// received it, and that no lock stands.
#[test]
#[ignore = "public testnet; set DW_DP301_TESTNET_FIXTURE"]
fn t2_testnet_pays_a_contact() {
    use std::time::{Duration, Instant};

    use crate::{HistoryFilter, HistoryQuery, HistorySort};

    const PAY: u64 = 100_000;
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP301_TESTNET_FIXTURE").expect("DW_DP301_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let identity = |w: &str| meta[w]["identity_id"].as_str().unwrap().to_owned();
    let (a_id, b_id) = (identity("A"), identity("B"));
    let record = fixture.join("DP3-01.json");

    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(4),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap();
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, crate::SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let import = |w: &str| {
        let phrase = std::fs::read_to_string(fixture.join(format!("{w}.mnemonic"))).unwrap();
        engine
            .block_on(s.import_wallet(
                Zeroizing::new(phrase.trim().as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(birth),
                    ..ImportOptions::default()
                },
            ))
            .unwrap()
    };
    let t0 = Instant::now();
    let (a, b) = (import("A"), import("B"));
    engine.block_on(s.start_spv()).unwrap();
    let wait = |what: &str, secs: u64, done: &mut dyn FnMut() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_secs(1));
        }
        eprintln!("{what} after {:?}", t0.elapsed());
    };
    wait("the wallets at the tip", 3600, &mut || {
        let snap = s.sync_snapshot().unwrap();
        snap.caught_up
            && snap.tip_height.is_some_and(|tip| {
                [a, b]
                    .iter()
                    .all(|w| s.wallet_scan_height(w).is_some_and(|h| h >= tip))
            })
    });
    wait("B as A's contact, its channel ready", 900, &mut || {
        let detail = s.dashpay(a).contact(a_id.clone(), b_id.clone());
        detail.ok().flatten().is_some_and(|d| {
            d.summary.relation == crate::platform::Relation::Contact
                && d.summary.channel == crate::platform::ChannelState::Ready
        })
    });
    let row = |w: WalletId, txid: &str| {
        let q = HistoryQuery {
            filter: HistoryFilter::default(),
            sort: HistorySort::NewestFirst,
            cursor: None,
            limit: 500,
        };
        let page = engine.block_on(s.history_page(w, q)).unwrap();
        page.records.into_iter().find(|r| r.txid == txid)
    };
    let grant = |max_duffs: u64| {
        (s.vault())
            .authorize(
                GrantPurpose::Spend { max_duffs },
                Some(&a.0),
                Credential::None,
            )
            .unwrap()
            .id
    };

    let sent = match std::fs::read(&record) {
        Ok(bytes) => {
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let txid = json["sent_txid"].as_str().unwrap().to_owned();
            let broadcast = json["broadcast"].as_str().unwrap_or_default();
            assert!(
                !broadcast.starts_with("failed"),
                "{txid} was refused ({broadcast}); delete {} to pay again",
                record.display()
            );
            txid
        }
        Err(_) => {
            let draft = s.new_tx_draft(a).unwrap();
            draft
                .set_contact_recipient(ContactRecipient {
                    identity: a_id.clone(),
                    contact: b_id.clone(),
                    amount: PAY,
                    subtract_fee: false,
                    note: Some("DP3-01 T2".into()),
                })
                .unwrap();
            let estimate = engine.block_on(draft.estimate()).unwrap();
            let cap = PAY + estimate.fee;
            let first = engine.block_on(draft.prepare(grant(cap))).unwrap();
            assert_eq!(first.summary().fee, estimate.fee, "same-size estimate");
            let address = paid(&first);
            engine.block_on(draft.abandon(first)).unwrap();
            let prepared = engine.block_on(draft.prepare(grant(cap))).unwrap();
            assert_eq!(paid(&prepared), address, "a re-prepare reuses the address");
            let txid = prepared.summary().txid.clone();
            eprintln!("A pays B's DashPay address {address} in {txid}");
            // Recorded first, and only by one run: a rerun after an unknown
            // outcome never pays twice.
            let write = |broadcast: &str, new: bool| {
                let json = serde_json::json!({
                    "sent_txid": txid, "duffs": PAY, "fee": estimate.fee, "from": "A", "to": "B",
                    "address": address, "broadcast": broadcast,
                });
                let mut file = (std::fs::OpenOptions::new().write(true).truncate(true))
                    .create_new(new)
                    .open(&record)
                    .unwrap();
                std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(&json).unwrap())
                    .unwrap();
            };
            write("pending", true);
            match engine.block_on(draft.broadcast(prepared)) {
                Ok(_) => write("accepted", false),
                Err(EngineError::Send(SendFailure::BroadcastUnknown { reason })) => {
                    write(&format!("unknown: {reason}"), false);
                    let lock = s.dashpay(a).payment_lock(a_id.clone(), b_id.clone());
                    assert_eq!(lock.unwrap().map(|l| l.txid), Some(txid.clone()));
                }
                Err(e) => {
                    write(&format!("failed: {e}"), false);
                    panic!("the payment was refused: {e}");
                }
            }
            txid
        }
    };
    wait("A's payment to B in A's history", 1800, &mut || {
        row(a, &sent).is_some()
    });
    let mut got = None;
    wait("A's payment in B's history", 1800, &mut || {
        got = row(b, &sent);
        got.is_some()
    });
    assert_eq!(got.unwrap().amount, i64::try_from(PAY).unwrap());
    // Settled: the evidence clears any lock an unknown outcome set.
    let dp = s.dashpay(a);
    if dp
        .payment_lock(a_id.clone(), b_id.clone())
        .unwrap()
        .is_some()
    {
        let resolved = engine.block_on(dp.resolve_payment_lock(a_id.clone(), b_id.clone()));
        eprintln!("resolve_payment_lock: {resolved:?}");
    }
    wait("no lock on B", 1800, &mut || {
        dp.payment_lock(a_id.clone(), b_id.clone())
            .unwrap()
            .is_none()
            || engine
                .block_on(dp.resolve_payment_lock(a_id.clone(), b_id.clone()))
                .is_ok_and(|r| r != LockResolution::Unknown)
    });
    // A new payment to B estimates (nothing locks it).
    let next = s.new_tx_draft(a).unwrap();
    next.set_contact_recipient(ContactRecipient {
        identity: a_id,
        contact: b_id,
        amount: PAY,
        subtract_fee: false,
        note: None,
    })
    .unwrap();
    engine.block_on(next.estimate()).unwrap();
    eprintln!("T2 done: {sent}");
    engine.block_on(engine.shutdown()).unwrap();
}
