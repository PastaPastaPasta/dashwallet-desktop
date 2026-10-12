//! Tests of requests.rs (DP2-02) on a session with no network and a
//! scripted Platform ([`FakeRequests`]): eligibility, ignore and unignore,
//! the refusals before a grant is redeemed, the keys each flow signs with,
//! "Accept and pay" across a slow accept, the key upgrade, and a write that
//! failed but landed. The testnet round trip is `t2_testnet_requests`
//! (ignored).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::identity::accessors::{IdentityGettersV0, IdentitySettersV0};
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::signer::Signer;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_vault::{Credential, GrantKind, GrantPurpose};
use platform_wallet::{
    AutoAcceptProofSource, ContactInfoPublishOutcome, ContactRequest, EstablishedContact,
    PlatformWallet,
};

use super::super::contacts::{Eligibility, PrivateDetails, RequestOutcome};
use super::super::errors::{ContactError, PlatformError};
use super::super::flows::{BudgetPurpose, CONTACT_REQUEST_CREDITS, FlowKind, KEY_UPDATE_CREDITS};
use super::super::identity_writes::identity_locks;
use super::super::journal_tests::{
    Recorder, b58, engine, grant_unused, id, platform_grant, request, session,
};
use super::super::keys_policy::registration_specs;
use super::super::private_details_tests::FakeInfos;
use super::super::signers::{VaultContactCrypto, VaultIdentitySigner};
use super::super::startup::SyncLoop;
use super::*;
use crate::{Engine, EventSink, NetworkSession, WalletId};

const ME: u8 = 1;
/// Can receive requests.
const BOB: u8 = 2;
/// On Platform without DashPay keys.
const CAROL: u8 = 3;
/// Not on Platform.
const DAVE: u8 = 4;
/// A contact of ours.
const ERIN: u8 = 5;
/// Asked us.
const FRANK: u8 = 6;
/// We asked them.
const GRACE: u8 = 7;

fn key(id: KeyID, purpose: Purpose, security_level: SecurityLevel) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id,
        purpose,
        security_level,
        contract_bounds: None,
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: vec![2; 33].into(),
        disabled_at: None,
    })
}

/// Keys 0–3 (iOS's base set), and with `dashpay` the pair 4–5. Placeholder
/// key data: the fake network signs nothing.
pub(crate) fn keys(dashpay: bool) -> BTreeMap<KeyID, IdentityPublicKey> {
    let mut keys = vec![
        key(0, Purpose::AUTHENTICATION, SecurityLevel::MASTER),
        key(1, Purpose::AUTHENTICATION, SecurityLevel::CRITICAL),
        key(2, Purpose::AUTHENTICATION, SecurityLevel::HIGH),
        key(3, Purpose::TRANSFER, SecurityLevel::CRITICAL),
    ];
    if dashpay {
        keys.push(key(4, Purpose::ENCRYPTION, SecurityLevel::MEDIUM));
        keys.push(key(5, Purpose::DECRYPTION, SecurityLevel::MEDIUM));
    }
    keys.into_iter().map(|k| (k.id(), k)).collect()
}

/// The keys of `keys` as the session's vault derives them for its identity
/// at index 0 (`registration_specs`): the keys a flow's signer can sign
/// with.
pub(crate) fn vault_keys(
    engine: &Engine,
    s: &Arc<NetworkSession>,
    wallet: WalletId,
    keys: BTreeMap<KeyID, IdentityPublicKey>,
) -> BTreeMap<KeyID, IdentityPublicKey> {
    let grants = vec![platform_grant(s, wallet, 1)];
    let lease_id =
        (engine.block_on(s.begin_flow(wallet, FlowKind::EnableDashPayKeys, grants))).unwrap();
    let lease = s.lease_for(&wallet, &lease_id).unwrap().unwrap();
    let signer = lease.identity_signer(&[0]).unwrap();
    let real = (engine.block_on(signer.derive_public_keys(0, &registration_specs()))).unwrap();
    lease.end();
    keys.into_keys().map(|k| (k, real[&k].clone())).collect()
}

pub(crate) fn identity(n: u8, keys: BTreeMap<KeyID, IdentityPublicKey>, balance: u64) -> Identity {
    Identity::V0(IdentityV0 {
        id: id(n),
        public_keys: keys,
        balance,
        revision: 0,
    })
}

/// Platform as the writes see it: identities by id, writes recorded, and
/// a write that fails on demand.
#[derive(Default)]
pub(crate) struct FakeRequests {
    platform: Mutex<HashMap<Identifier, Identity>>,
    /// Reads fail: `platform.unavailable`.
    down: AtomicBool,
    /// Writes fail with this error, having landed or not as `landed` says.
    pub(crate) fails: Mutex<Option<PlatformError>>,
    /// Writes fail with this error before they sign anything.
    pub(crate) fails_unsigned: Mutex<Option<PlatformError>>,
    pub(crate) landed: AtomicBool,
    writes: AtomicU32,
    /// Runs inside each write, before it lands.
    pub(crate) during: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// What a key upgrade costs.
    fee: u64,
    /// The `contactInfo` documents a write publishes.
    pub(crate) infos: Arc<FakeInfos>,
    /// A `contactInfo` write takes their reads down as it lands (or not).
    pub(crate) infos_down_after: AtomicBool,
}

impl FakeRequests {
    pub(crate) fn with(identities: impl IntoIterator<Item = Identity>) -> Self {
        Self {
            platform: Mutex::new(identities.into_iter().map(|i| (i.id(), i)).collect()),
            fee: 20_000_000,
            ..Self::default()
        }
    }

    fn up(&self) -> Result<(), PlatformError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        Ok(())
    }

    /// A write: the hook, then the scripted failure, if any.
    fn write(&self) -> Result<(), PlatformError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if let Some(during) = &*self.during.lock().unwrap() {
            during();
        }
        match self.fails.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Signs as rs-sdk does before it broadcasts a transition: with the
    /// first key of `id` the signer signs with, so a write that goes out has
    /// released a signature.
    async fn sign(
        &self,
        id: Identifier,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        if let Some(e) = self.fails_unsigned.lock().unwrap().clone() {
            return Err(e);
        }
        let key = (self.platform.lock().unwrap()[&id].public_keys().values())
            .find(|k| signer.can_sign_with(k))
            .cloned()
            .expect("a key the flow signs with");
        signer
            .sign(&key, b"transition")
            .await
            .map_err(|_| PlatformError::SignerUnavailable)?;
        Ok(())
    }

    pub(crate) fn writes(&self) -> u32 {
        self.writes.load(Ordering::SeqCst)
    }

    fn platform_copy(&self, id: Identifier) -> Option<Identity> {
        self.platform.lock().unwrap().get(&id).cloned()
    }

    /// As the library records a request it sent (an incoming one from the
    /// recipient establishes the contact).
    async fn record_sent(wallet: &PlatformWallet, sent: &ContactRequest) {
        let mut state = wallet.state_mut().await;
        let managed = (state.identity_manager)
            .managed_identity_mut(&sent.sender_id)
            .unwrap();
        managed
            .add_sent_contact_request(sent.clone(), wallet.persister())
            .unwrap();
    }
}

impl RequestNet for FakeRequests {
    async fn identity(&self, id: Identifier) -> Result<Option<Identity>, PlatformError> {
        self.up()?;
        Ok(self.platform_copy(id))
    }

    async fn balance(&self, id: Identifier) -> Result<Option<u64>, PlatformError> {
        self.up()?;
        Ok(self.platform_copy(id).map(|i| i.balance()))
    }

    async fn refresh(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> Result<Identity, PlatformError> {
        self.up()?;
        let copy = self.platform_copy(id).ok_or(PlatformError::Unavailable)?;
        let mut state = wallet.state_mut().await;
        state
            .identity_manager
            .managed_identity_mut(&id)
            .unwrap()
            .identity = copy.clone();
        Ok(copy)
    }

    async fn sent_to(
        &self,
        _: &PlatformWallet,
        sender: Identifier,
        recipient: Identifier,
    ) -> Result<Option<ContactRequest>, PlatformError> {
        self.up()?;
        let landed = self.landed.load(Ordering::SeqCst);
        Ok(landed.then(|| ContactRequest::new(sender, recipient, 0, 0, 0, vec![1; 96], 1, 9_000)))
    }

    async fn send(
        &self,
        wallet: &PlatformWallet,
        sender: Identifier,
        recipient: Identifier,
        _: AutoAcceptProofSource,
        signer: &VaultIdentitySigner,
        _: &VaultContactCrypto,
    ) -> Result<ContactRequest, PlatformError> {
        self.sign(sender, signer).await?;
        self.write()?;
        let sent = ContactRequest::new(sender, recipient, 4, 5, 0, vec![1; 96], 1, 8_000);
        Self::record_sent(wallet, &sent).await;
        Ok(sent)
    }

    async fn accept(
        &self,
        wallet: &PlatformWallet,
        request: &ContactRequest,
        signer: &VaultIdentitySigner,
        _: &VaultContactCrypto,
    ) -> Result<ContactRequest, PlatformError> {
        self.sign(request.recipient_id, signer).await?;
        self.write()?;
        let ours = ContactRequest::new(
            request.recipient_id,
            request.sender_id,
            4,
            5,
            0,
            vec![1; 96],
            1,
            8_500,
        );
        Self::record_sent(wallet, &ours).await;
        Ok(ours)
    }

    async fn add_keys(
        &self,
        _: &PlatformWallet,
        id: Identifier,
        keys: Vec<IdentityPublicKey>,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        self.sign(id, signer).await?;
        let landed = self.write();
        if landed.is_ok() || self.landed.load(Ordering::SeqCst) {
            let mut platform = self.platform.lock().unwrap();
            let copy = platform.get_mut(&id).unwrap();
            for k in keys {
                copy.add_public_key(k);
            }
            copy.set_balance(copy.balance() - self.fee);
        }
        landed
    }

    /// As platform-wallet: the details saved with the contact first, the
    /// publish deferred below two contacts and skipped without an index;
    /// then signed and written, sealed into [`FakeInfos`] once landed.
    async fn set_contact_info(
        &self,
        wallet: &PlatformWallet,
        owner: Identifier,
        contact: Identifier,
        details: &PrivateDetails,
        signer: &VaultIdentitySigner,
        crypto: &VaultContactCrypto,
    ) -> Result<ContactInfoPublishOutcome, PlatformError> {
        let (contacts, index) = {
            let mut state = wallet.state_mut().await;
            let m = (state.identity_manager.managed_identity_mut(&owner)).unwrap();
            let saved = m.set_contact_metadata(&contact, details.into(), wallet.persister());
            assert!(saved.unwrap(), "an established contact");
            (m.dashpay().established_contacts().len(), m.identity_index)
        };
        if contacts < 2 {
            return Ok(ContactInfoPublishOutcome::DeferredUntilTwoContacts);
        }
        let Some(index) = index else {
            return Ok(ContactInfoPublishOutcome::SkippedWatchOnly);
        };
        self.sign(owner, signer).await?;
        let landed = self.write();
        if landed.is_ok() || self.landed.load(Ordering::SeqCst) {
            let at = (owner, index, contact);
            self.infos.seal(crypto, at, details).await;
        }
        if self.infos_down_after.load(Ordering::SeqCst) {
            self.infos.down.store(true, Ordering::SeqCst);
        }
        landed.map(|()| ContactInfoPublishOutcome::Published)
    }
}

pub(crate) struct Fixture {
    pub(crate) engine: Engine,
    pub(crate) s: Arc<NetworkSession>,
    pub(crate) wallet: WalletId,
    pub(crate) net: Arc<FakeRequests>,
    /// What the engine emitted.
    pub(crate) events: Arc<Recorder>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// Our identity at index 0 with its DashPay keys and `balance` credits,
    /// Erin a contact, Frank asking, Grace asked; Bob, Carol and Dave as
    /// their consts say.
    pub(crate) fn new(balance: u64) -> Self {
        Self::with_keys(keys(true), balance)
    }

    pub(crate) fn with_keys(mine: BTreeMap<KeyID, IdentityPublicKey>, balance: u64) -> Self {
        Self::build(mine, balance, true)
    }

    /// Our identity with `mine` as they are: keys the vault does not hold.
    pub(crate) fn foreign(mine: BTreeMap<KeyID, IdentityPublicKey>, balance: u64) -> Self {
        Self::build(mine, balance, false)
    }

    fn build(mine: BTreeMap<KeyID, IdentityPublicKey>, balance: u64, held: bool) -> Self {
        let dir = dw_testutil::private_tempdir();
        let events = Arc::new(Recorder::default());
        let engine = engine(dir.path(), Arc::clone(&events) as Arc<dyn EventSink>);
        let (s, wallet) = session(&engine, true);
        let mine = if held {
            vault_keys(&engine, &s, wallet, mine)
        } else {
            mine
        };
        let me = identity(ME, mine, balance);
        let net = Arc::new(FakeRequests::with([
            me.clone(),
            identity(BOB, keys(true), 0),
            identity(CAROL, keys(false), 0),
        ]));
        let manager = s.manager().unwrap();
        engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let persister = wallet.persister();
            (state.identity_manager)
                .add_identity(me, 0, wallet.wallet_id(), persister)
                .unwrap();
            let m = state
                .identity_manager
                .managed_identity_mut(&id(ME))
                .unwrap();
            m.apply_established_contact(EstablishedContact::new(
                id(ERIN),
                request(ME, ERIN, 1_000),
                request(ERIN, ME, 2_000),
            ));
            m.add_incoming_contact_request(request(FRANK, ME, 3_000), persister)
                .unwrap();
            m.add_sent_contact_request(request(ME, GRACE, 4_000), persister)
                .unwrap();
        });
        Self {
            engine,
            s,
            wallet,
            net,
            events,
            _dir: dir,
        }
    }

    pub(crate) fn pair(&self, contact: u8) -> Pair {
        Pair {
            wallet: self.wallet,
            identity: id(ME),
            contact: id(contact),
        }
    }

    /// Runs `body` as a facade call of the wallet does.
    pub(crate) fn call<T: Send + 'static, Fut>(
        &self,
        body: impl FnOnce(Arc<FakeRequests>, Arc<NetworkSession>, Arc<PlatformWallet>) -> Fut
        + Send
        + 'static,
    ) -> Result<T, ContactError>
    where
        Fut: std::future::Future<Output = Result<T, ContactError>> + Send + 'static,
    {
        let net = Arc::clone(&self.net);
        let dp = self.s.dashpay(self.wallet);
        self.engine
            .block_on(dp.on_wallet(move |session, _, wallet| body(net, session, wallet)))
    }

    pub(crate) fn code<T: std::fmt::Debug>(r: Result<T, ContactError>) -> &'static str {
        r.unwrap_err().code()
    }

    fn eligibility(&self, contact: u8) -> Result<Eligibility, ContactError> {
        let pair = self.pair(contact);
        self.call(move |net, _, w| async move { eligibility(&*net, &w, pair).await })
    }

    fn send(&self, to: u8, grant: &str) -> Result<RequestOutcome, ContactError> {
        let (wallet_id, grant) = (self.wallet, grant.to_owned());
        self.call(move |net, s, w| async move {
            let ask = (b58(ME), b58(to), None);
            send(&*net, &s, &w, wallet_id, ask, grant).await
        })
    }

    fn accept(&self, from: u8, grant: &str) -> Result<RequestOutcome, ContactError> {
        let (pair, grant) = (self.pair(from), grant.to_owned());
        self.call(move |net, s, w| async move { accept(&*net, &s, &w, pair, grant).await })
    }

    fn enable_keys(&self, grant: &str) -> Result<(), ContactError> {
        let (wallet_id, grant) = (self.wallet, grant.to_owned());
        self.call(move |net, s, w| async move {
            enable_keys(&*net, &s, &w, wallet_id, id(ME), grant).await
        })
    }

    pub(crate) fn grant(&self, max_credits: u64) -> String {
        platform_grant(&self.s, self.wallet, max_credits)
    }

    fn spend_grant(&self, max_duffs: u64) -> String {
        self.s
            .vault
            .authorize(
                GrantPurpose::Spend { max_duffs },
                Some(&self.wallet.0),
                Credential::None,
            )
            .unwrap()
            .id
    }

    /// What the lease `lease` has spent of its credits budget.
    pub(crate) fn credits_spent(&self, lease: &str) -> u64 {
        let lease = self.s.lease_for(&self.wallet, lease).unwrap().unwrap();
        let view = lease.view().unwrap();
        (view.budgets.iter())
            .find(|b| b.purpose == BudgetPurpose::Credits)
            .unwrap()
            .spent
    }

    pub(crate) fn unused(&self, grant: &str) -> bool {
        grant_unused(&self.s, self.wallet, grant)
    }

    pub(crate) fn flow(&self, flow: FlowKind, grants: Vec<String>) -> String {
        self.engine
            .block_on(self.s.begin_flow(self.wallet, flow, grants))
            .unwrap()
    }

    /// Our identity as the library holds it.
    fn me(&self) -> Identity {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let state = wallet.state().await;
            state
                .identity_manager
                .managed_identity(&id(ME))
                .unwrap()
                .identity
                .clone()
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.engine.block_on(self.engine.shutdown());
    }
}

/// The wallet's own state answers first, without Platform; then the
/// contact's keys on Platform, and an unknown Platform answer is an error,
/// never `Ok`.
#[test]
fn eligibility_asks_the_wallet_then_platform() {
    let f = Fixture::new(0);
    let answer = |n| f.eligibility(n).unwrap();
    f.net.down.store(true, Ordering::SeqCst);
    assert_eq!(answer(ME), Eligibility::IsSelf);
    assert_eq!(answer(ERIN), Eligibility::AlreadyContact);
    assert_eq!(answer(GRACE), Eligibility::PendingOutgoing);
    assert_eq!(answer(FRANK), Eligibility::PendingIncoming);
    assert_eq!(Fixture::code(f.eligibility(BOB)), "platform.unavailable");
    f.net.down.store(false, Ordering::SeqCst);
    assert_eq!(answer(BOB), Eligibility::Ok);
    assert_eq!(answer(CAROL), Eligibility::ContactCannotReceive);
    assert_eq!(Fixture::code(f.eligibility(DAVE)), "identity.not_found");

    // Another wallet's identity, or none.
    let pair = Pair {
        identity: id(BOB),
        ..f.pair(ERIN)
    };
    let theirs = f.call(move |net, _, w| async move { eligibility(&*net, &w, pair).await });
    assert_eq!(Fixture::code(theirs), "identity.not_found");
    assert_eq!(
        Pair::parse(f.wallet, &b58(ME), "not base58!")
            .unwrap_err()
            .code(),
        "invalid_argument"
    );

    // Without its own DashPay keys our identity is told so before Platform
    // is asked about anyone.
    let g = Fixture::with_keys(keys(false), 0);
    g.net.down.store(true, Ordering::SeqCst);
    assert_eq!(g.eligibility(BOB).unwrap(), Eligibility::NoDashPayKeys);
    assert_eq!(g.eligibility(ERIN).unwrap(), Eligibility::AlreadyContact);
}

/// Ignore and unignore are local, need no grant, and are idempotent.
#[test]
fn ignore_and_unignore_stay_local() {
    let f = Fixture::new(0);
    let set = |n: u8, on: bool| {
        let pair = f.pair(n);
        f.call(move |_, _, w| async move { set_ignored(&w, pair, on).await })
    };
    f.net.down.store(true, Ordering::SeqCst);
    set(FRANK, true).unwrap();
    set(FRANK, true).unwrap();
    // Frank's request is gone and he is a stranger to Platform's check.
    f.net.down.store(false, Ordering::SeqCst);
    assert_eq!(
        f.eligibility(FRANK).unwrap_err().code(),
        "identity.not_found"
    );
    set(FRANK, false).unwrap();
    set(BOB, false).unwrap();
    assert_eq!(Fixture::code(set(ME, true)), "contact.self");
    assert_eq!(f.net.writes(), 0);
}

/// Every refusal comes before the grant is redeemed or anything is sent,
/// and the quote is the line: one credit short of it refuses, the quote
/// itself passes.
#[test]
fn a_send_refuses_before_it_redeems_the_grant() {
    let quote = CONTACT_REQUEST_CREDITS;
    let f = Fixture::new(quote);
    let grant = f.grant(quote);
    for (to, code) in [
        (ME, "contact.ineligible"),
        (ERIN, "contact.ineligible"),
        (FRANK, "contact.ineligible"),
        (CAROL, "contact.ineligible"),
        (DAVE, "identity.not_found"),
    ] {
        assert_eq!(Fixture::code(f.send(to, &grant)), code, "to {to}");
    }
    assert_eq!(
        f.send(CAROL, &grant).unwrap_err(),
        ContactError::Ineligible {
            reason: Eligibility::ContactCannotReceive
        }
    );
    let short = f.grant(quote - 1);
    assert_eq!(
        Fixture::code(f.send(BOB, &short)),
        "platform.grant_exceeded"
    );
    assert!(f.unused(&short) && f.unused(&grant));
    assert_eq!(f.net.writes(), 0);

    let g = Fixture::new(quote - 1);
    let poor = g.grant(quote);
    assert_eq!(
        Fixture::code(g.send(BOB, &poor)),
        "platform.insufficient_credits"
    );
    assert!(g.unused(&poor));

    // At the quote: sent, Pending, the grant spent.
    let outcome = f.send(BOB, &grant).unwrap();
    assert_eq!(
        outcome,
        RequestOutcome::Pending {
            request: "8000".into()
        }
    );
    assert!(!f.unused(&grant));
    assert_eq!(f.eligibility(BOB).unwrap(), Eligibility::PendingOutgoing);
    assert_eq!(
        Fixture::code(f.send(BOB, &f.grant(quote))),
        "contact.ineligible"
    );
    assert_eq!(f.net.writes(), 1);
}

/// A failed write is read back: on Platform it is the request (Q6), else
/// the write's own error.
#[test]
fn a_failed_send_that_landed_is_pending() {
    let f = Fixture::new(u64::MAX / 2);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    assert_eq!(
        Fixture::code(f.send(BOB, &f.grant(u64::MAX))),
        "platform.timeout"
    );
    f.net.landed.store(true, Ordering::SeqCst);
    assert_eq!(
        f.send(BOB, &f.grant(u64::MAX)).unwrap(),
        RequestOutcome::Pending {
            request: "9000".into()
        }
    );
    // Recorded as the library records a request it sent.
    assert_eq!(f.eligibility(BOB).unwrap(), Eligibility::PendingOutgoing);
}

/// The quote is charged to a lease id's Credits budget by the write's call
/// permit and kept once the write has signed, whatever its outcome: a
/// lease id carrying one quote pays for one write, failed or not.
#[test]
fn one_lease_pays_for_one_write() {
    let quote = CONTACT_REQUEST_CREDITS;
    let f = Fixture::new(u64::MAX / 2);
    let lease = f.flow(FlowKind::ContactRequest, vec![f.grant(quote)]);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    assert_eq!(Fixture::code(f.send(BOB, &lease)), "platform.timeout");
    *f.net.fails.lock().unwrap() = None;
    assert_eq!(
        Fixture::code(f.send(BOB, &lease)),
        "platform.grant_exceeded"
    );
    assert_eq!(f.net.writes(), 1);
    let twice = f.flow(FlowKind::ContactRequest, vec![f.grant(2 * quote)]);
    assert!(f.send(BOB, &twice).is_ok());
}

/// A permit's identity signer gives public keys and signatures only (DP2-02
/// Sol r5): it derives a key upgrade's public keys while unnarrowed and for
/// its own identity; narrowed to a flow's keys it derives none; once the
/// permit ends it signs nothing. No raw vault signer can be had from it
/// (`signers::tests::no_vault_signer_leaves_the_module`).
#[test]
fn a_permits_signer_hands_out_public_keys_only() {
    let f = Fixture::with_keys(keys(false), u64::MAX / 2);
    let id = f.flow(
        FlowKind::EnableDashPayKeys,
        vec![f.grant(KEY_UPDATE_CREDITS)],
    );
    let lease = f.s.lease_for(&f.wallet, &id).unwrap().unwrap();
    let permit = lease.call_permit("enable_dashpay_keys", None).unwrap();
    let signer = permit.identity_signer(&[0]).unwrap();
    let derive = |signer: &VaultIdentitySigner, index| {
        f.engine
            .block_on(signer.derive_public_keys(index, &registration_specs()))
    };
    let real = derive(&signer, 0).unwrap();
    assert_eq!(real, vault_keys(&f.engine, &f.s, f.wallet, keys(true)));
    assert!(matches!(
        derive(&signer, 1),
        Err(crate::EngineError::InvalidArgument(_))
    ));
    let narrowed = signer.clone().only_keys([0]);
    assert!(matches!(
        derive(&narrowed, 0),
        Err(crate::EngineError::Internal(_))
    ));

    let master = &real[&0];
    f.engine.block_on(signer.sign(master, b"before")).unwrap();
    permit.finish(true);
    assert!(!signer.can_sign_with(master));
    assert!(f.engine.block_on(signer.sign(master, b"after")).is_err());
}

/// A write's call permit hands its quote back when the write failed
/// before signing anything (`Cancelled`), and keeps it once a signature is
/// out, whatever the write returns (E0-04 P2b, m4 item 41): per flow, on a
/// lease carrying one quote.
#[test]
fn a_quote_goes_back_only_when_nothing_was_signed() {
    type Write = fn(&Fixture, &str) -> Result<(), ContactError>;
    let flows: [(FlowKind, u64, Write); 3] = [
        (FlowKind::ContactRequest, CONTACT_REQUEST_CREDITS, |f, l| {
            f.send(BOB, l).map(drop)
        }),
        (FlowKind::Accept, CONTACT_REQUEST_CREDITS, |f, l| {
            f.accept(FRANK, l).map(drop)
        }),
        (FlowKind::EnableDashPayKeys, KEY_UPDATE_CREDITS, |f, l| {
            f.enable_keys(l)
        }),
    ];
    for (flow, quote, write) in flows {
        let keys = if flow == FlowKind::EnableDashPayKeys {
            keys(false)
        } else {
            keys(true)
        };
        let f = Fixture::with_keys(keys, u64::MAX / 2);
        let lease = f.flow(flow, vec![f.grant(quote)]);
        *f.net.fails_unsigned.lock().unwrap() = Some(PlatformError::Unavailable);
        let refused = write(&f, &lease).unwrap_err();
        assert_eq!(refused.code(), "platform.unavailable", "{flow:?}");
        assert_eq!(
            f.credits_spent(&lease),
            0,
            "{flow:?}: nothing signed, quote not returned"
        );
        *f.net.fails_unsigned.lock().unwrap() = None;

        *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
        assert_eq!(write(&f, &lease).unwrap_err().code(), "platform.timeout");
        assert_eq!(
            f.credits_spent(&lease),
            quote,
            "{flow:?}: signed, quote not kept"
        );
        *f.net.fails.lock().unwrap() = None;
        assert_eq!(
            write(&f, &lease).unwrap_err().code(),
            "platform.grant_exceeded",
            "{flow:?}"
        );
        assert_eq!(f.net.writes(), 1, "{flow:?}");
    }
}

/// A write whose identity has no key its flow signs with is refused before
/// the grant is redeemed: no HIGH or CRITICAL key for a request or accept,
/// no master for a key upgrade.
#[test]
fn missing_flow_keys_refuse_before_the_grant() {
    let quote = CONTACT_REQUEST_CREDITS;
    let no_document_key = keys(true).into_iter().filter(|(k, _)| ![1, 2].contains(k));
    let f = Fixture::with_keys(no_document_key.collect(), u64::MAX / 2);
    let grant = f.grant(quote);
    assert_eq!(Fixture::code(f.send(BOB, &grant)), "identity.keys_missing");
    assert_eq!(
        Fixture::code(f.accept(FRANK, &grant)),
        "identity.keys_missing"
    );
    assert!(f.unused(&grant));

    let no_master = keys(false).into_iter().filter(|(k, _)| *k != 0);
    let g = Fixture::with_keys(no_master.collect(), u64::MAX / 2);
    let grant = g.grant(KEY_UPDATE_CREDITS);
    assert_eq!(
        Fixture::code(g.enable_keys(&grant)),
        "identity.keys_missing"
    );
    assert!(g.unused(&grant));
    assert_eq!(f.net.writes() + g.net.writes(), 0);
}

/// A grant stays unused when the vault holds no key the write's flow signs
/// with, and redeems afterwards (Sol r1 #1).
fn refused_before_redemption(
    f: &Fixture,
    (flow, grant): (FlowKind, String),
    write: Result<impl std::fmt::Debug, ContactError>,
) {
    assert_eq!(Fixture::code(write), "identity.keys_missing");
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);
    f.flow(flow, vec![grant]);
}

/// A send whose document key the vault does not hold.
#[test]
fn a_send_without_a_held_key_keeps_the_grant() {
    let f = Fixture::foreign(keys(true), u64::MAX / 2);
    let grant = f.grant(CONTACT_REQUEST_CREDITS);
    let send = f.send(BOB, &grant);
    refused_before_redemption(&f, (FlowKind::ContactRequest, grant), send);
}

/// An accept whose document key the vault does not hold.
#[test]
fn an_accept_without_a_held_key_keeps_the_grant() {
    let f = Fixture::foreign(keys(true), u64::MAX / 2);
    let grant = f.grant(CONTACT_REQUEST_CREDITS);
    let accept = f.accept(FRANK, &grant);
    refused_before_redemption(&f, (FlowKind::Accept, grant), accept);
}

/// A key upgrade whose master key the vault does not hold.
#[test]
fn a_key_upgrade_without_a_held_master_keeps_the_grant() {
    let f = Fixture::foreign(keys(false), u64::MAX / 2);
    let grant = f.grant(KEY_UPDATE_CREDITS);
    let upgrade = f.enable_keys(&grant);
    refused_before_redemption(&f, (FlowKind::EnableDashPayKeys, grant), upgrade);
}

/// A wallet removal that lands after the key check and before the
/// redemption (Sol r2 #1): the redemption asks again under the vault's
/// record-write guard, so the grant stays unused and nothing is written.
#[test]
fn a_removal_after_the_key_check_keeps_the_grant() {
    let f = Fixture::new(u64::MAX / 2);
    let grant = f.grant(CONTACT_REQUEST_CREDITS);
    let (me, vault, wallet) = (f.me(), f.s.vault.clone(), f.wallet);
    // `ready` runs between the two, as wallets.rs removes a wallet.
    let remove = move || {
        let wipe = (vault.authorize(GrantPurpose::Wipe, Some(&wallet.0), Credential::None))
            .unwrap()
            .id;
        let token = (vault.redeem_grant(&wipe, GrantKind::Wipe, Some(&wallet.0))).unwrap();
        assert!(vault.wipe_wallet_secret(&wallet.0, &token).unwrap());
        Ok(())
    };
    let flows = (&[FlowKind::ContactRequest][..], CONTACT_REQUEST_CREDITS);
    let lease = (f.engine).block_on(write_lease(&f.s, wallet, &grant, flows, (0, &me), remove));
    assert_eq!(Fixture::code(lease.map(drop)), "identity.keys_missing");
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);
}

/// A lease id is not charged for a write the vault cannot sign: its
/// signer's key check comes before the charge.
#[test]
fn a_lease_is_not_charged_without_a_held_key() {
    let f = Fixture::foreign(keys(true), u64::MAX / 2);
    let id = f.flow(
        FlowKind::ContactRequest,
        vec![f.grant(CONTACT_REQUEST_CREDITS)],
    );
    assert_eq!(Fixture::code(f.send(BOB, &id)), "identity.keys_missing");
    let lease = f.s.lease_for(&f.wallet, &id).unwrap().unwrap();
    let view = lease.view().unwrap();
    let credits = (view.budgets.iter())
        .find(|b| b.purpose == BudgetPurpose::Credits)
        .unwrap();
    assert_eq!(
        (credits.ceiling, credits.spent),
        (CONTACT_REQUEST_CREDITS, 0)
    );
    assert_eq!(f.net.writes(), 0);
}

/// A lease must be one of the write's flows and carry its quote.
#[test]
fn a_lease_of_another_flow_or_too_small_is_refused() {
    let quote = CONTACT_REQUEST_CREDITS;
    let f = Fixture::new(u64::MAX / 2);
    let profile = f.flow(FlowKind::ProfileEdit, vec![f.grant(quote)]);
    assert_eq!(
        Fixture::code(f.send(BOB, &profile)),
        "platform.grant_invalid"
    );
    let small = f.flow(FlowKind::ContactRequest, vec![f.grant(quote - 1)]);
    assert_eq!(
        Fixture::code(f.send(BOB, &small)),
        "platform.grant_exceeded"
    );
    let crypto_only = f.flow(FlowKind::ContactRequest, vec![f.grant(0)]);
    assert_eq!(
        Fixture::code(f.send(BOB, &crypto_only)),
        "platform.needs_grant"
    );
    assert_eq!(f.net.writes(), 0);
    let lease = f.flow(FlowKind::ContactRequest, vec![f.grant(quote)]);
    assert!(f.send(BOB, &lease).is_ok());
}

/// Accept refuses itself, a contact, no request and missing keys; then
/// establishes.
#[test]
fn accept_establishes_from_their_request() {
    let f = Fixture::new(u64::MAX / 2);
    let grant = f.grant(CONTACT_REQUEST_CREDITS);
    assert_eq!(Fixture::code(f.accept(ME, &grant)), "contact.self");
    assert_eq!(
        Fixture::code(f.accept(ERIN, &grant)),
        "contact.already_contact"
    );
    assert_eq!(Fixture::code(f.accept(BOB, &grant)), "invalid_argument");
    let send_lease = f.flow(
        FlowKind::ContactRequest,
        vec![f.grant(CONTACT_REQUEST_CREDITS)],
    );
    assert_eq!(
        Fixture::code(f.accept(FRANK, &send_lease)),
        "platform.grant_invalid"
    );
    assert!(f.unused(&grant));
    assert_eq!(
        f.accept(FRANK, &grant).unwrap(),
        RequestOutcome::Established {
            request: "8500".into()
        }
    );
    assert_eq!(f.eligibility(FRANK).unwrap(), Eligibility::AlreadyContact);

    let g = Fixture::with_keys(keys(false), u64::MAX / 2);
    assert_eq!(
        g.accept(FRANK, &g.grant(CONTACT_REQUEST_CREDITS))
            .unwrap_err(),
        ContactError::Ineligible {
            reason: Eligibility::NoDashPayKeys
        }
    );
}

/// A failed accept is read back as a send is: our request on Platform
/// establishes the contact, else the write's own error.
#[test]
fn a_failed_accept_that_landed_is_established() {
    let f = Fixture::new(u64::MAX / 2);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    assert_eq!(
        Fixture::code(f.accept(FRANK, &f.grant(CONTACT_REQUEST_CREDITS))),
        "platform.timeout"
    );
    assert_eq!(f.eligibility(FRANK).unwrap(), Eligibility::PendingIncoming);
    f.net.landed.store(true, Ordering::SeqCst);
    assert_eq!(
        f.accept(FRANK, &f.grant(CONTACT_REQUEST_CREDITS)).unwrap(),
        RequestOutcome::Established {
            request: "9000".into()
        }
    );
    assert_eq!(f.eligibility(FRANK).unwrap(), Eligibility::AlreadyContact);
}

/// "Accept and pay" (E0-04 §16 test row): the accept runs as a call of the
/// lease, so however long it takes the payment's `prepare` still finds the
/// Spend part.
#[test]
fn accept_and_pay_keeps_its_spend_part_across_the_accept() {
    let f = Fixture::new(u64::MAX / 2);
    let lease_id = f.flow(
        FlowKind::AcceptAndPay,
        vec![f.grant(CONTACT_REQUEST_CREDITS), f.spend_grant(100_000)],
    );
    let running = Arc::new(AtomicBool::new(false));
    let (seen, s) = (Arc::clone(&running), Arc::clone(&f.s));
    *f.net.during.lock().unwrap() = Some(Box::new(move || {
        let views = s.leases().unwrap();
        seen.store(views.iter().any(|v| v.call_running), Ordering::SeqCst);
    }));
    f.accept(FRANK, &lease_id).unwrap();
    assert!(
        running.load(Ordering::SeqCst),
        "the accept ran as a lease call"
    );
    let lease = f.s.lease_for(&f.wallet, &lease_id).unwrap().unwrap();
    assert!(lease.has_spend());
    lease.spend_signer().unwrap();
}

/// Each flow's signer signs with its transition's keys only: a contact
/// request never with the master key, a key upgrade with the master and
/// the added keys.
#[test]
fn each_flow_signs_with_its_own_keys_only() {
    let f = Fixture::new(0);
    let lease_of = |flow| {
        let id = f.flow(flow, vec![f.grant(KEY_UPDATE_CREDITS)]);
        f.s.lease_for(&f.wallet, &id).unwrap().unwrap()
    };
    let permit = |lease: &Lease| lease.call_permit("test", None).unwrap();
    let request = lease_of(FlowKind::ContactRequest);
    // Our identity's real keys at index 0.
    let unrestricted = request.identity_signer(&[0]).unwrap();
    let real = f
        .engine
        .block_on(unrestricted.derive_public_keys(0, &registration_specs()))
        .unwrap();
    let me = identity(ME, real.clone(), 0);
    let sign = |signer: &VaultIdentitySigner, k: KeyID| {
        f.engine
            .block_on(signer.sign(&real[&k], b"transition"))
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    // The vault could sign with the master key; the flow's signer refuses.
    sign(&unrestricted, 0).unwrap();
    let doc_permit = permit(&request);
    let doc = flow_signer(&doc_permit, 0, &me, &[]).unwrap();
    assert!(!doc.can_sign_with(&real[&0]));
    let refused = sign(&doc, 0).unwrap_err().to_string();
    assert!(
        refused.contains("not a key this flow signs with"),
        "{refused}"
    );
    assert!(doc.can_sign_with(&real[&1]));
    sign(&doc, 1).unwrap();
    assert!(sign(&doc, 4).is_err(), "the DashPay keys only do ECDH");
    let document_key = flow_keys(FlowKind::ContactRequest, &me, &[], |_| true).unwrap();
    for flow in [
        FlowKind::Accept,
        FlowKind::AcceptAndPay,
        FlowKind::PrivateDetails,
    ] {
        let keys = flow_keys(flow, &me, &[], |_| true).unwrap();
        assert_eq!(keys, document_key, "{flow:?}");
    }

    let upgrade_permit = permit(&lease_of(FlowKind::EnableDashPayKeys));
    let upgrade = flow_signer(&upgrade_permit, 0, &me, &[4, 5]).unwrap();
    sign(&upgrade, 0).unwrap();
    sign(&upgrade, 4).unwrap();
    assert!(sign(&upgrade, 1).is_err() && sign(&upgrade, 2).is_err());

    let other = lease_of(FlowKind::ProfileEdit);
    assert_eq!(
        flow_signer(&permit(&other), 0, &me, &[])
            .unwrap_err()
            .code(),
        "platform.grant_invalid"
    );
    // No HIGH or CRITICAL key: nothing to sign a document with.
    let master_only = identity(ME, real.into_iter().filter(|(k, _)| *k == 0).collect(), 0);
    assert_eq!(
        flow_keys(FlowKind::ContactRequest, &master_only, &[], |_| true)
            .unwrap_err()
            .code(),
        "identity.keys_missing"
    );
}

/// The upgrade adds the missing pair after the highest id, against the
/// quote, and is a no-op once the keys are there.
#[test]
fn enable_dashpay_keys_adds_the_missing_pair_once() {
    let quote = KEY_UPDATE_CREDITS;
    let f = Fixture::with_keys(keys(false), quote);
    assert_eq!(f.eligibility(BOB).unwrap(), Eligibility::NoDashPayKeys);
    let short = f.grant(quote - 1);
    assert_eq!(
        Fixture::code(f.enable_keys(&short)),
        "platform.grant_exceeded"
    );
    let wrong = f.flow(FlowKind::ContactRequest, vec![f.grant(quote)]);
    assert_eq!(
        Fixture::code(f.enable_keys(&wrong)),
        "platform.grant_invalid"
    );
    assert!(f.unused(&short));
    assert_eq!(f.net.writes(), 0);

    let grant = f.grant(quote);
    f.enable_keys(&grant).unwrap();
    let me = f.me();
    let added: Vec<(KeyID, Purpose)> = (me.public_keys().values())
        .filter(|k| k.id() >= 4)
        .map(|k| (k.id(), k.purpose()))
        .collect();
    assert_eq!(added, [(4, Purpose::ENCRYPTION), (5, Purpose::DECRYPTION)]);
    // Derived through the permit's signer as registration derives them.
    let registered = vault_keys(&f.engine, &f.s, f.wallet, keys(true));
    for id in [4, 5] {
        assert_eq!(
            me.public_keys()[&id].data(),
            registered[&id].data(),
            "key {id}"
        );
    }
    assert_eq!(
        me.balance(),
        quote - f.net.fee,
        "the library's copy refreshed"
    );
    assert_eq!(f.eligibility(BOB).unwrap(), Eligibility::Ok);

    // Nothing missing: nothing sent, the grant left.
    let again = f.grant(quote);
    f.enable_keys(&again).unwrap();
    assert!(f.unused(&again));
    assert_eq!(f.net.writes(), 1);

    let g = Fixture::with_keys(keys(false), quote - 1);
    let grant = g.grant(quote);
    assert_eq!(
        Fixture::code(g.enable_keys(&grant)),
        "platform.insufficient_credits"
    );
    assert!(g.unused(&grant));
}

/// An upgrade whose write failed but whose keys are on Platform is done;
/// one that did not land is the write's error.
#[test]
fn a_failed_upgrade_is_read_back() {
    let f = Fixture::with_keys(keys(false), u64::MAX / 2);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    assert_eq!(
        Fixture::code(f.enable_keys(&f.grant(u64::MAX))),
        "platform.timeout"
    );
    assert_eq!(f.eligibility(BOB).unwrap(), Eligibility::NoDashPayKeys);
    f.net.landed.store(true, Ordering::SeqCst);
    f.enable_keys(&f.grant(u64::MAX)).unwrap();
    assert_eq!(f.eligibility(BOB).unwrap(), Eligibility::Ok);
}

/// One write of an identity at a time, whatever the feature: a send waits
/// for the identity's write lock, which names.rs takes too.
#[test]
fn a_write_waits_for_the_identity_write_lock() {
    let f = Fixture::new(u64::MAX / 2);
    let grant = f.grant(CONTACT_REQUEST_CREDITS);
    let locks = identity_locks(f.wallet, id(ME));
    let held = f.engine.block_on(locks.write.lock());
    let (net, dp, wallet_id) = (Arc::clone(&f.net), f.s.dashpay(f.wallet), f.wallet);
    let sending = f.s.rt.spawn(async move {
        dp.on_wallet(move |s, _, w| async move {
            send(&*net, &s, &w, wallet_id, (b58(ME), b58(BOB), None), grant).await
        })
        .await
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.net.writes(), 0, "the send waits for the lock");
    drop(held);
    assert!(f.engine.block_on(sending).unwrap().is_ok());
    assert_eq!(f.net.writes(), 1);
}

/// The dashpay sync passes `dp` has seen finish.
pub(crate) fn passes(dp: &crate::platform::DashPay) -> Option<u64> {
    let status = dp.sync_status().ok()?;
    let pass = status
        .loops
        .iter()
        .find(|l| l.sync_loop == SyncLoop::DashPaySync)?;
    pass.last_run_at
}

pub(crate) fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < timeout, "no {what} within {timeout:?}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// DP2-02's acceptance on the public testnet (T2), with wallets A and C of
/// the fixture (DW_DP202_TESTNET_FIXTURE: `A.mnemonic`, `C.mnemonic`).
/// C's identity was registered without DashPay keys: the first run enables
/// them (charged ≤ quote, measured as the proved balance before less after,
/// DEC-195), sends A a request, A ignores and unignores C, accepts, and both
/// see the contact within 2 passes. Later runs find the keys and the
/// contact there: the upgrade is "already enabled: Ok, nothing sent", and
/// the contact is checked both ways. Prints the T2 numbers.
#[test]
#[ignore = "public testnet; set DW_DP202_TESTNET_FIXTURE"]
fn t2_testnet_requests() {
    use super::super::recovery_tests;
    use crate::{DashNetwork, ImportOptions, SessionOptions};
    use zeroize::Zeroizing;

    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP202_TESTNET_FIXTURE").expect("DW_DP202_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let a_id = meta["A"]["identity_id"].as_str().unwrap().to_owned();

    let dir = dw_testutil::private_tempdir();
    let engine = recovery_tests::engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let import = |who: &str| {
        let phrase = Zeroizing::new(
            std::fs::read_to_string(fixture.join(format!("{who}.mnemonic"))).unwrap(),
        );
        let options = ImportOptions {
            birth_height: Some(birth),
            ..ImportOptions::default()
        };
        let words = Zeroizing::new(phrase.trim().as_bytes().to_vec());
        engine
            .block_on(s.import_wallet(words, Zeroizing::new(Vec::new()), options))
            .unwrap()
    };
    let (a, c) = (import("A"), import("C"));
    engine.block_on(s.start_spv()).unwrap();
    let (dp_a, dp_c) = (s.dashpay(a), s.dashpay(c));
    let mut c_id = String::new();
    wait_until("C's identity", Duration::from_secs(180), || {
        let found = dp_c.identities().unwrap_or_default();
        found.first().map(|i| c_id = i.identity.clone()).is_some()
    });
    wait_until("A's identity", Duration::from_secs(180), || {
        dp_a.identities()
            .is_ok_and(|ids| ids.iter().any(|i| i.identity == a_id))
    });
    eprintln!("C is {c_id}");
    // A fresh engine learns each wallet's requests and contacts in its
    // first DashPay pass.
    wait_until(
        "a DashPay pass of A and C",
        Duration::from_secs(180),
        || passes(&dp_a).is_some() && passes(&dp_c).is_some(),
    );

    let grant = |wallet: WalletId, max_credits: u64| platform_grant(&s, wallet, max_credits);
    let unused = |wallet: WalletId, grant: &str| grant_unused(&s, wallet, grant);
    let net = SdkRequests(s.manager().unwrap().sdk_arc());
    let balance = |id: &str| {
        let id = Identifier::from_string(id, Encoding::Base58).unwrap();
        engine.block_on(net.balance(id)).unwrap().unwrap()
    };
    let charged = |what: &str, quote: u64, before: u64, after: u64| {
        let charged = before.checked_sub(after).expect("the balance grew");
        eprintln!("T2 {what}: quote {quote} before {before} after {after} charged {charged}");
        assert!(
            charged <= quote,
            "{what} charged {charged} over its quote {quote}"
        );
        assert!(charged > 0, "{what} charged nothing");
    };
    let eligibility = |dp: &crate::platform::DashPay, me: &str, them: &str| {
        engine
            .block_on(dp.eligibility(me.into(), them.into()))
            .unwrap()
    };

    // 1. The key upgrade, refused before any prompt until it is done.
    if eligibility(&dp_c, &c_id, &a_id) == Eligibility::NoDashPayKeys {
        let before = balance(&c_id);
        let g = grant(c, KEY_UPDATE_CREDITS);
        engine
            .block_on(dp_c.enable_dashpay_keys(c_id.clone(), g.clone()))
            .unwrap();
        charged(
            "enable_dashpay_keys",
            KEY_UPDATE_CREDITS,
            before,
            balance(&c_id),
        );
        assert!(!unused(c, &g));
    }
    let before = balance(&c_id);
    let g = grant(c, KEY_UPDATE_CREDITS);
    engine
        .block_on(dp_c.enable_dashpay_keys(c_id.clone(), g.clone()))
        .unwrap();
    assert!(unused(c, &g), "already enabled: Ok, nothing sent");
    assert_eq!(balance(&c_id), before);
    eprintln!("T2 enable_dashpay_keys again: already enabled, nothing sent");

    match eligibility(&dp_c, &c_id, &a_id) {
        Eligibility::AlreadyContact => {
            assert_eq!(
                eligibility(&dp_a, &a_id, &c_id),
                Eligibility::AlreadyContact
            );
            eprintln!("T2 rerun: C and A are contacts both ways; nothing sent");
            return;
        }
        Eligibility::Ok => {}
        other => panic!("C → A is {other:?}: finish or reset the fixture by hand"),
    }

    // 2. C asks A.
    let before = balance(&c_id);
    let sent = engine
        .block_on(dp_c.send_request(
            c_id.clone(),
            a_id.clone(),
            None,
            grant(c, CONTACT_REQUEST_CREDITS),
        ))
        .unwrap();
    charged(
        "send_request",
        CONTACT_REQUEST_CREDITS,
        before,
        balance(&c_id),
    );
    assert!(matches!(sent, RequestOutcome::Pending { .. }), "{sent:?}");
    assert_eq!(
        eligibility(&dp_c, &c_id, &a_id),
        Eligibility::PendingOutgoing
    );

    // 3. A sees it, ignores C, unignores, and sees it again.
    wait_until("C's request at A", Duration::from_secs(300), || {
        eligibility(&dp_a, &a_id, &c_id) == Eligibility::PendingIncoming
    });
    engine
        .block_on(dp_a.ignore(a_id.clone(), c_id.clone()))
        .unwrap();
    assert_eq!(eligibility(&dp_a, &a_id, &c_id), Eligibility::Ok);
    engine
        .block_on(dp_a.unignore(a_id.clone(), c_id.clone()))
        .unwrap();
    wait_until("C's request at A again", Duration::from_secs(300), || {
        eligibility(&dp_a, &a_id, &c_id) == Eligibility::PendingIncoming
    });

    // 4. A accepts; both see the contact within 2 passes.
    let before = balance(&a_id);
    let accepted = engine
        .block_on(dp_a.accept_request(
            a_id.clone(),
            c_id.clone(),
            grant(a, CONTACT_REQUEST_CREDITS),
        ))
        .unwrap();
    charged(
        "accept_request",
        CONTACT_REQUEST_CREDITS,
        before,
        balance(&a_id),
    );
    assert!(
        matches!(accepted, RequestOutcome::Established { .. }),
        "{accepted:?}"
    );
    let start = (passes(&dp_a), passes(&dp_c));
    let mut seen = (0, 0);
    let mut last = start;
    wait_until("the contact at A and C", Duration::from_secs(300), || {
        let now = (passes(&dp_a), passes(&dp_c));
        seen.0 += usize::from(now.0 != last.0);
        seen.1 += usize::from(now.1 != last.1);
        last = now;
        assert!(
            seen.0 <= 2 && seen.1 <= 2,
            "not established within 2 passes"
        );
        eligibility(&dp_a, &a_id, &c_id) == Eligibility::AlreadyContact
            && eligibility(&dp_c, &c_id, &a_id) == Eligibility::AlreadyContact
    });
    eprintln!("T2 established after {seen:?} passes (A, C)");
    engine.block_on(engine.shutdown()).unwrap();
}
