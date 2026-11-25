//! App Connect against a scripted Platform: what is read, what is refused
//! before anything is signed, the keys an approval adds and the response it
//! publishes.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::identity::accessors::{IdentityGettersV0, IdentitySettersV0};
use dpp::identity::identity_public_key::accessors::v0::{
    IdentityPublicKeyGettersV0, IdentityPublicKeySettersV0,
};
use dpp::identity::identity_public_key::accessors::v1::IdentityPublicKeyGettersV1;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract;
use dw_uri::connect::{
    AppConnectRequest, AuthBinding, Bounds, Carrier, ConnectUri, EncBinding, EncPurpose, Level,
    NetworkSel,
};
use dw_vault::{Credential, GrantKind, GrantPurpose};
use platform_wallet::{DpnsNameInfo, PlatformWallet};

use super::super::app_connect_net::{ConnectNet, Response};
use super::super::app_connect_plan::{ContractFacts, DAY_MS, GroupFacts, MAX_FUNDING};
use super::super::errors::{ConnectError, PlatformError};
use super::super::journal_tests::{engine, id, session};
use super::super::requests::tests::{identity, keys, vault_keys};
use super::super::signers::VaultIdentitySigner;
use super::*;
use crate::{DashNetwork, Engine, EventSink, NetworkSession, WalletId};

const ME: u8 = 1;
const OTHER: u8 = 9;
const NOW: u64 = 1_800_000_000_000;
const BALANCE: u64 = 20_000_000_000;

fn app() -> Identifier {
    id(40)
}
fn group() -> Identifier {
    id(41)
}
fn app_owner() -> Identifier {
    id(42)
}

fn app_eph() -> (SecretKey, [u8; 33]) {
    let sk = SecretKey::from_secret_bytes([7u8; 32]).unwrap();
    (sk, PublicKey::from_secret_key(&sk).serialize())
}

/// Yappr's login: its group CRITICAL, the DashPay profile HIGH, an
/// encryption key on its contract, a connection id.
fn request() -> AppConnectRequest {
    AppConnectRequest {
        app_eph_pub: app_eph().1,
        app_id: app().to_buffer(),
        label: "Yappr".into(),
        identity_hint: None,
        auth_bindings: vec![
            AuthBinding {
                bounds: Bounds::Group(group().to_buffer()),
                level: Level::Critical,
                budget: Some(50_000_000_000),
                expires_at_ms: Some(NOW + 90 * DAY_MS),
            },
            AuthBinding {
                bounds: Bounds::DocumentType(dashpay_contract::ID.to_buffer(), "profile".into()),
                level: Level::High,
                budget: Some(1_000_000_000),
                expires_at_ms: Some(NOW + 90 * DAY_MS),
            },
        ],
        connection_id: Some([9; 16]),
        enc_bindings: vec![EncBinding {
            contract: app().to_buffer(),
            purpose: EncPurpose::Encryption,
        }],
        renew: false,
        created_at_ms: NOW - 1_000,
    }
}

fn link(request: AppConnectRequest, network: NetworkSel) -> ConnectUri {
    ConnectUri {
        carrier: Carrier::DashPayConnect,
        network,
        request,
        callback: None,
    }
}

fn devnet(request: AppConnectRequest) -> ConnectUri {
    link(request, NetworkSel::Devnet)
}

fn yappr_group() -> GroupFacts {
    GroupFacts {
        owner: app_owner(),
        admins: vec![id(43)],
        contracts: vec![app()],
        document_types: Vec::new(),
        tokens: Vec::new(),
    }
}

type AfterUpdate = Box<dyn FnOnce(&FakeConnect) + Send>;

/// Platform as App Connect reads and writes it.
pub(crate) struct FakeConnect {
    protocol: AtomicU32,
    block_time: Mutex<u64>,
    identities: Mutex<HashMap<Identifier, Identity>>,
    contracts: Mutex<HashMap<Identifier, ContractFacts>>,
    groups: Mutex<HashMap<Identifier, GroupFacts>>,
    current: Mutex<HashMap<(Identifier, Identifier, Purpose), KeyID>>,
    responses: Mutex<BTreeSet<([u8; 20], Identifier)>>,
    /// Every read, in order.
    reads: Mutex<Vec<&'static str>>,
    /// Each update: the keys added and disabled.
    updates: Mutex<Vec<(Vec<IdentityPublicKey>, Vec<KeyID>)>>,
    /// Each response published, with the key that signed it.
    published: Mutex<Vec<(Identifier, Response, KeyID)>>,
    /// Signatures released, by key id.
    signed: Mutex<Vec<KeyID>>,
    fail_publish: AtomicBool,
    /// Runs once, right after the first update lands.
    after_update: Mutex<Option<AfterUpdate>>,
    /// Every identity read returns this, as a lagging node would.
    stale: Mutex<Option<Identity>>,
    /// Updates are accepted but never land.
    lose_updates: AtomicBool,
    refuse_update: Mutex<Option<PlatformError>>,
    /// The library builds an update from its cached identity. Platform
    /// refuses one built before the last refresh, as its revision is old.
    refreshed: AtomicBool,
}

impl FakeConnect {
    fn new(me: Identity) -> Self {
        let mut contracts = HashMap::new();
        contracts.insert(
            app(),
            ContractFacts {
                owner: app_owner(),
                document_types: ["post".to_string()].into(),
                encryption_requirement: Some(2),
                decryption_requirement: None,
                has_tokens: true,
                readonly: false,
            },
        );
        contracts.insert(
            dashpay_contract::ID,
            ContractFacts {
                owner: Identifier::default(),
                document_types: ["profile".to_string(), "contactRequest".to_string()].into(),
                encryption_requirement: None,
                decryption_requirement: None,
                has_tokens: false,
                readonly: true,
            },
        );
        Self {
            protocol: AtomicU32::new(14),
            block_time: Mutex::new(NOW),
            identities: Mutex::new([(me.id(), me)].into()),
            contracts: Mutex::new(contracts),
            groups: Mutex::new([(group(), yappr_group())].into()),
            current: Mutex::default(),
            responses: Mutex::default(),
            reads: Mutex::default(),
            updates: Mutex::default(),
            published: Mutex::default(),
            signed: Mutex::default(),
            fail_publish: AtomicBool::new(false),
            after_update: Mutex::default(),
            stale: Mutex::default(),
            lose_updates: AtomicBool::new(false),
            refuse_update: Mutex::default(),
            refreshed: AtomicBool::new(false),
        }
    }

    fn read(&self, what: &'static str) {
        self.reads.lock().unwrap().push(what);
    }

    fn reads(&self) -> usize {
        self.reads.lock().unwrap().len()
    }

    fn me(&self) -> Identity {
        self.identities.lock().unwrap()[&id(ME)].clone()
    }

    fn signed(&self) -> Vec<KeyID> {
        self.signed.lock().unwrap().clone()
    }
}

impl ConnectNet for FakeConnect {
    async fn protocol_version(&self) -> Result<u32, PlatformError> {
        self.read("protocol");
        Ok(self.protocol.load(Ordering::SeqCst))
    }

    async fn identity(&self, id: Identifier) -> Result<(Option<Identity>, u64), PlatformError> {
        self.read("identity");
        let found = (self.stale.lock().unwrap().clone())
            .or_else(|| self.identities.lock().unwrap().get(&id).cloned());
        Ok((found, *self.block_time.lock().unwrap()))
    }

    async fn contract(&self, id: Identifier) -> Result<Option<ContractFacts>, PlatformError> {
        self.read("contract");
        Ok(self.contracts.lock().unwrap().get(&id).cloned())
    }

    async fn group(&self, id: Identifier) -> Result<Option<GroupFacts>, PlatformError> {
        self.read("group");
        Ok(self.groups.lock().unwrap().get(&id).cloned())
    }

    async fn current_contract_key(
        &self,
        identity: Identifier,
        contract: Identifier,
        purpose: Purpose,
    ) -> Result<Option<KeyID>, PlatformError> {
        self.read("current");
        Ok(self
            .current
            .lock()
            .unwrap()
            .get(&(identity, contract, purpose))
            .copied())
    }

    async fn response_exists(
        &self,
        request_hash: [u8; 20],
        owner: Identifier,
    ) -> Result<bool, PlatformError> {
        self.read("response");
        Ok(self
            .responses
            .lock()
            .unwrap()
            .contains(&(request_hash, owner)))
    }

    async fn refresh(&self, _: &PlatformWallet, id: Identifier) -> Result<Identity, PlatformError> {
        self.read("refresh");
        self.refreshed.store(true, Ordering::SeqCst);
        let found = (self.stale.lock().unwrap().clone())
            .or_else(|| self.identities.lock().unwrap().get(&id).cloned());
        found.ok_or(PlatformError::Unavailable)
    }

    async fn update_identity(
        &self,
        _: &PlatformWallet,
        id: Identifier,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        signer: &ConnectUpdateSigner,
    ) -> Result<(), PlatformError> {
        let master = (self.me().public_keys().values())
            .find(|k| k.security_level() == SecurityLevel::MASTER)
            .cloned()
            .unwrap();
        // As dpp: each new unique-type key proves possession, then MASTER.
        for k in add
            .iter()
            .filter(|k| k.key_type() == KeyType::ECDSA_SECP256K1)
        {
            signer
                .sign(k, b"update")
                .await
                .map_err(|_| PlatformError::SignerUnavailable)?;
            self.signed.lock().unwrap().push(k.id());
        }
        signer
            .sign(&master, b"update")
            .await
            .map_err(|_| PlatformError::SignerUnavailable)?;
        self.signed.lock().unwrap().push(master.id());
        if !self.refreshed.swap(false, Ordering::SeqCst) {
            return Err(PlatformError::Internal {
                detail: "Failed to broadcast identity update".into(),
            });
        }
        if let Some(e) = self.refuse_update.lock().unwrap().take() {
            return Err(e);
        }
        if !self.lose_updates.load(Ordering::SeqCst) {
            let mut all = self.identities.lock().unwrap();
            let me = all.get_mut(&id).unwrap();
            for k in &add {
                me.public_keys_mut().insert(k.id(), k.clone());
                if k.purpose() == Purpose::ENCRYPTION {
                    self.current
                        .lock()
                        .unwrap()
                        .insert((id, app(), Purpose::ENCRYPTION), k.id());
                }
            }
            for d in &disable {
                if let Some(k) = me.public_keys_mut().get_mut(d) {
                    k.set_disabled_at(NOW);
                }
            }
        }
        self.updates.lock().unwrap().push((add, disable));
        if let Some(after) = self.after_update.lock().unwrap().take() {
            after(self);
        }
        Ok(())
    }

    async fn publish(
        &self,
        _: &PlatformWallet,
        owner: Identifier,
        response: &Response,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        let key = (self.me().public_keys().values())
            .find(|k| signer.can_sign_with(k))
            .cloned()
            .expect("a key the response signs with");
        signer
            .sign(&key, b"response")
            .await
            .map_err(|_| PlatformError::SignerUnavailable)?;
        self.signed.lock().unwrap().push(key.id());
        if self.fail_publish.load(Ordering::SeqCst) {
            return Err(PlatformError::Timeout);
        }
        self.responses
            .lock()
            .unwrap()
            .insert((response.request_hash, owner));
        self.published
            .lock()
            .unwrap()
            .push((owner, response.clone(), key.id()));
        Ok(())
    }
}

struct Fixture {
    engine: Engine,
    s: Arc<NetworkSession>,
    wallet: WalletId,
    net: Arc<FakeConnect>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::on(false)
    }

    /// `mainnet`: a mainnet session (no Platform).
    fn on(mainnet: bool) -> Self {
        let dir = dw_testutil::private_tempdir();
        let engine = engine(
            dir.path(),
            Arc::new(super::super::journal_tests::Recorder::default()) as Arc<dyn EventSink>,
        );
        let (s, wallet) = if mainnet {
            mainnet_session(&engine)
        } else {
            session(&engine, true)
        };
        let mine = vault_keys(&engine, &s, wallet, keys(true));
        let me = identity(ME, mine, BALANCE);
        let manager = s.manager().unwrap();
        let added = me.clone();
        engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let persister = wallet.persister();
            (state.identity_manager)
                .add_identity(added, 0, wallet.wallet_id(), persister)
                .unwrap();
            let names = vec![DpnsNameInfo {
                label: "alice".into(),
                acquired_at: Some(1),
            }];
            (state.identity_manager.managed_identity_mut(&id(ME)))
                .unwrap()
                .set_dpns_names(names, persister);
        });
        let net = Arc::new(FakeConnect::new(me));
        Self {
            engine,
            s,
            wallet,
            net,
            _dir: dir,
        }
    }

    fn choice() -> ConnectChoice {
        ConnectChoice {
            identity: id(ME).to_string(dpp::platform_value::string_encoding::Encoding::Base58),
            selection: vec![0, 1],
            terms: ConnectTerms {
                funding_limit: 51_000_000_000,
                expires_at_ms: NOW + 90 * DAY_MS,
            },
        }
    }

    fn sheet(&self, link: &ConnectUri) -> Result<ConnectSheet, ConnectError> {
        let (net, s, w, link) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet,
            link.clone(),
        );
        self.engine
            .block_on(async move { sheet(&*net, &s, w, &link).await })
    }

    fn quote(
        &self,
        link: &ConnectUri,
        choice: &ConnectChoice,
    ) -> Result<ConnectQuote, ConnectError> {
        let (net, s, w) = (Arc::clone(&self.net), Arc::clone(&self.s), self.wallet);
        let (link, choice) = (link.clone(), choice.clone());
        self.engine
            .block_on(async move { quote(&*net, &s, w, &link, &choice).await })
    }

    fn grant(&self, request: [u8; 20], plan: [u8; 32], max_credits: u64) -> String {
        let purpose = GrantPurpose::AppConnect {
            max_credits,
            request,
            plan,
        };
        (self
            .s
            .vault
            .authorize(purpose, Some(&self.wallet.0), Credential::None))
        .unwrap()
        .id
    }

    /// A grant for what `quote` read.
    fn quoted_grant(&self, link: &ConnectUri, choice: &ConnectChoice) -> String {
        let q = self.quote(link, choice).unwrap();
        self.grant(link.request.request_hash(), q.plan, q.max_credits)
    }

    fn unused(&self, grant: &str) -> bool {
        (self.s.vault)
            .check_grant(grant, GrantKind::AppConnect, Some(&self.wallet.0))
            .is_ok()
    }

    fn wallet(&self) -> Arc<PlatformWallet> {
        let manager = self.s.manager().unwrap();
        let w = self.wallet;
        self.engine
            .block_on(async move { manager.get_wallet(&w.0).await.unwrap() })
    }

    fn approve(
        &self,
        link: &ConnectUri,
        choice: &ConnectChoice,
        grant: &str,
    ) -> Result<ConnectDone, ConnectError> {
        let (net, s, w, pw) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet,
            self.wallet(),
        );
        let (link, choice, grant) = (link.clone(), choice.clone(), grant.to_owned());
        self.engine
            .block_on(async move { approve(&*net, &s, &pw, w, &link, &choice, grant).await })
    }

    fn retry(&self, link: &ConnectUri) -> Result<ConnectDone, ConnectError> {
        let (net, s, w, pw) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet,
            self.wallet(),
        );
        let link = link.clone();
        self.engine
            .block_on(async move { retry_send(&*net, &s, &pw, w, &link).await })
    }
}

fn mainnet_session(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    use crate::SessionOptions;
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Mainnet,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform: true,
                ..Default::default()
            },
        ))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let phrase = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let wallet = engine
        .block_on(s.import_wallet(
            zeroize::Zeroizing::new(phrase.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            Default::default(),
        ))
        .unwrap();
    (s, wallet)
}

#[test]
fn another_network_is_refused_before_anything_is_read() {
    let f = Fixture::new();
    let wrong = link(request(), NetworkSel::Testnet);
    assert_eq!(f.sheet(&wrong), Err(ConnectError::Network));
    assert_eq!(
        f.quote(&wrong, &Fixture::choice()),
        Err(ConnectError::Network)
    );
    let grant = f.grant(request().request_hash(), [0; 32], u64::MAX / 2);
    assert_eq!(
        f.approve(&wrong, &Fixture::choice(), &grant),
        Err(ConnectError::Network)
    );
    assert_eq!(f.net.reads(), 0);
    assert!(f.unused(&grant));
    assert!(f.net.signed().is_empty());
}

/// Mainnet is off by the flag: the sheet has no Approve path, and an
/// approval called directly is refused with the grant unused and nothing
/// read or signed.
#[test]
fn mainnet_approval_is_refused_even_when_called_directly() {
    let f = Fixture::on(true);
    let main = link(request(), NetworkSel::Mainnet);
    assert_eq!(f.sheet(&main), Err(ConnectError::MainnetOff));
    assert_eq!(
        f.quote(&main, &Fixture::choice()),
        Err(ConnectError::MainnetOff)
    );
    let grant = f.grant(request().request_hash(), [0; 32], u64::MAX / 2);
    assert_eq!(
        f.approve(&main, &Fixture::choice(), &grant),
        Err(ConnectError::MainnetOff)
    );
    assert_eq!(f.retry(&main), Err(ConnectError::MainnetOff));
    assert_eq!(f.net.reads(), 0);
    assert!(f.unused(&grant));
    assert!(f.net.signed().is_empty());
    assert!(!enabled_on(&DashNetwork::Mainnet));
    assert!(enabled_on(&DashNetwork::Testnet));
}

#[test]
fn the_sheet_shows_the_group_and_signs_nothing() {
    let f = Fixture::new();
    let l = devnet(request());
    let leases = f.s.leases().unwrap().len();
    let sheet = f.sheet(&l).unwrap();
    assert_eq!(sheet.label, "Yappr");
    assert_eq!(sheet.app_owner, b58(&app_owner()));
    assert_eq!(sheet.identities.len(), 1);
    assert_eq!(sheet.identities[0].username, "alice");
    assert!(sheet.encryption);
    let ConnectScope::Group(g) = &sheet.bindings[0].scope else {
        panic!("{:?}", sheet.bindings[0]);
    };
    assert_eq!(g.owner, b58(&app_owner()));
    assert_eq!(g.admins, vec![b58(&id(43))]);
    assert_eq!(g.contracts, vec![b58(&app())]);
    assert!(sheet.bindings[0].primary);
    assert_eq!(sheet.bindings[1].scope, ConnectScope::Profile);
    assert_eq!(sheet.terms.funding_limit, 51_000_000_000);
    assert_eq!(sheet.max_funding_limit, MAX_FUNDING);
    assert_eq!(sheet.earlier_approval, None);
    f.quote(&l, &Fixture::choice()).unwrap();
    assert!(f.net.signed().is_empty());
    assert!(f.net.updates.lock().unwrap().is_empty());
    assert_eq!(f.s.leases().unwrap().len(), leases);
}

#[test]
fn approval_adds_bounded_keys_and_publishes_the_sealed_response() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    let done = f.approve(&l, &Fixture::choice(), &grant).unwrap();
    assert_eq!(done.keys_added, vec![6, 7, 8]);
    assert!(!f.unused(&grant));
    let updates = f.net.updates.lock().unwrap().clone();
    assert_eq!(updates.len(), 1);
    let (add, disable) = &updates[0];
    assert!(disable.is_empty());
    let (group_key, profile_key, enc_key) = (&add[0], &add[1], &add[2]);
    for (k, level, budget) in [
        (group_key, SecurityLevel::CRITICAL, 50_800_000_000),
        (profile_key, SecurityLevel::HIGH, 200_000_000),
    ] {
        assert_eq!(k.purpose(), Purpose::AUTHENTICATION);
        assert_eq!(k.security_level(), level);
        assert_eq!(k.key_type(), KeyType::ECDSA_HASH160);
        assert_eq!(k.expires_at(), Some(NOW + 90 * DAY_MS));
        assert_eq!(k.total_budget(), Some(budget));
    }
    assert_eq!(
        group_key.contract_bounds(),
        Some(&ContractBounds::ContractGroup { id: group() })
    );
    assert_eq!(
        group_key.total_budget().unwrap() + profile_key.total_budget().unwrap(),
        51_000_000_000
    );
    assert_eq!(enc_key.purpose(), Purpose::ENCRYPTION);
    assert_eq!(enc_key.security_level(), SecurityLevel::MEDIUM);
    assert_eq!(
        enc_key.contract_bounds(),
        Some(&ContractBounds::SingleContract { id: app() })
    );
    // The encryption key proved possession, MASTER signed the update, an
    // unbound CRITICAL key with no limits signed the response.
    assert_eq!(f.net.signed(), vec![8, 0, 1]);
    let published = f.net.published.lock().unwrap().clone();
    assert_eq!(published.len(), 1);
    let (owner, response, _) = &published[0];
    assert_eq!(*owner, id(ME));
    assert_eq!(response.request_hash, request().request_hash());
    assert_eq!(response.payload.len(), 28 + 32 * 3);
    let records = f.engine.block_on(records(&f.s, f.wallet)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, RecordState::Published);
    assert_eq!(records[0].session_keys, vec![6, 7]);
    assert_eq!(records[0].encryption_keys, vec![8]);
}

#[test]
fn an_unselected_binding_gets_no_key_and_no_slot() {
    let f = Fixture::new();
    let l = devnet(request());
    let choice = ConnectChoice {
        selection: vec![0],
        ..Fixture::choice()
    };
    let grant = f.quoted_grant(&l, &choice);
    f.approve(&l, &choice, &grant).unwrap();
    let (add, _) = f.net.updates.lock().unwrap()[0].clone();
    assert_eq!(add.len(), 2);
    assert_eq!(add[0].total_budget(), Some(51_000_000_000));
    let published = f.net.published.lock().unwrap().clone();
    assert_eq!(published[0].1.payload.len(), 28 + 32 * 2);
}

#[test]
fn the_primary_binding_cannot_be_declined() {
    let f = Fixture::new();
    let choice = ConnectChoice {
        selection: vec![1],
        ..Fixture::choice()
    };
    assert!(matches!(
        f.quote(&devnet(request()), &choice),
        Err(ConnectError::Platform(
            PlatformError::InvalidArgument { .. }
        ))
    ));
}

#[test]
fn a_changed_group_refuses_the_approval_with_the_grant_unused() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.net
        .groups
        .lock()
        .unwrap()
        .get_mut(&group())
        .unwrap()
        .contracts
        .push(id(50));
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant),
        Err(ConnectError::PlanChanged)
    );
    assert!(f.unused(&grant));
    assert!(f.net.signed().is_empty());
}

#[test]
fn changed_terms_refuse_the_approval_with_the_grant_unused() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    let more = ConnectChoice {
        terms: ConnectTerms {
            funding_limit: 500_000_000_000,
            ..Fixture::choice().terms
        },
        ..Fixture::choice()
    };
    assert_eq!(f.approve(&l, &more, &grant), Err(ConnectError::PlanChanged));
    assert!(f.unused(&grant));
    assert!(f.net.signed().is_empty());
}

#[test]
fn a_grant_for_another_request_or_a_platform_grant_is_refused() {
    let f = Fixture::new();
    let l = devnet(request());
    let q = f.quote(&l, &Fixture::choice()).unwrap();
    let plan = q.plan;
    let other = f.grant([1; 20], plan, q.max_credits);
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &other),
        Err(ConnectError::Platform(PlatformError::GrantInvalid))
    );
    assert!(f.unused(&other));
    let op = super::super::journal_tests::platform_grant(&f.s, f.wallet, q.max_credits);
    assert!(f.approve(&l, &Fixture::choice(), &op).is_err());
    assert!(super::super::journal_tests::grant_unused(
        &f.s, f.wallet, &op
    ));
    assert!(f.net.signed().is_empty());
}

/// Replay: a request this identity answered is refused before anything is
/// signed, whether the response is on Platform from another device or
/// from this approval.
#[test]
fn an_answered_request_is_refused() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.approve(&l, &Fixture::choice(), &grant).unwrap();
    let signed = f.net.signed().len();
    assert_eq!(f.quote(&l, &Fixture::choice()), Err(ConnectError::Answered));
    let again = f.grant(request().request_hash(), [0; 32], u64::MAX / 2);
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &again),
        Err(ConnectError::Answered)
    );
    assert!(f.unused(&again));
    assert_eq!(f.net.signed().len(), signed);
}

#[test]
fn an_old_or_future_request_is_stale() {
    let f = Fixture::new();
    *f.net.block_time.lock().unwrap() = NOW + 11 * 60 * 1000;
    assert_eq!(f.sheet(&devnet(request())), Err(ConnectError::Stale));
    *f.net.block_time.lock().unwrap() = NOW - 4 * 60 * 1000;
    assert_eq!(f.sheet(&devnet(request())), Err(ConnectError::Stale));
}

#[test]
fn an_old_protocol_is_refused() {
    let f = Fixture::new();
    f.net.protocol.store(13, Ordering::SeqCst);
    assert_eq!(f.sheet(&devnet(request())), Err(ConnectError::Protocol));
}

#[test]
fn a_group_of_another_owner_is_refused() {
    let f = Fixture::new();
    f.net
        .groups
        .lock()
        .unwrap()
        .get_mut(&group())
        .unwrap()
        .owner = id(44);
    assert_eq!(f.sheet(&devnet(request())), Err(ConnectError::Owner));
}

#[test]
fn a_hint_for_an_identity_not_in_the_wallet_is_refused() {
    let f = Fixture::new();
    let mut r = request();
    r.identity_hint = Some(id(OTHER).to_buffer());
    assert_eq!(f.sheet(&devnet(r.clone())), Err(ConnectError::Hint));
    assert_eq!(
        f.quote(&devnet(r), &Fixture::choice()),
        Err(ConnectError::Hint)
    );
    let mut mine = request();
    mine.identity_hint = Some(id(ME).to_buffer());
    assert_eq!(f.sheet(&devnet(mine)).unwrap().identities.len(), 1);
}

#[test]
fn a_balance_below_the_fees_is_refused() {
    let f = Fixture::new();
    f.net
        .identities
        .lock()
        .unwrap()
        .get_mut(&id(ME))
        .unwrap()
        .set_balance(1_000);
    assert!(matches!(
        f.quote(&devnet(request()), &Fixture::choice()),
        Err(ConnectError::Credits { .. })
    ));
}

/// Platform-wallet picks the key a contact request is encrypted to by
/// purpose and id, not bounds: an app key of a DashPay purpose the identity
/// has no DashPay key for would become its contact key. No such key is added,
/// none on the DashPay contract either, and the sign-in still finishes.
#[test]
fn an_app_key_never_stands_in_for_a_dashpay_key() {
    fn other_contract(k: &mut IdentityPublicKey) {
        if let IdentityPublicKey::V0(v) = k {
            v.contract_bounds = Some(ContractBounds::SingleContract { id: id(77) });
        }
    }
    let cases: [(&str, EncPurpose, fn(&mut Identity)); 3] = [
        ("no DECRYPTION key", EncPurpose::Decryption, |me| {
            me.public_keys_mut()
                .retain(|_, k| k.purpose() != Purpose::DECRYPTION);
        }),
        (
            "ENCRYPTION key on another contract",
            EncPurpose::Encryption,
            |me| {
                for k in me.public_keys_mut().values_mut() {
                    if k.purpose() == Purpose::ENCRYPTION {
                        other_contract(k);
                    }
                }
            },
        ),
        ("disabled ENCRYPTION key", EncPurpose::Encryption, |me| {
            for k in me.public_keys_mut().values_mut() {
                if k.purpose() == Purpose::ENCRYPTION {
                    k.set_disabled_at(1);
                }
            }
        }),
    ];
    for (case, purpose, edit) in cases {
        let f = Fixture::new();
        edit(f.net.identities.lock().unwrap().get_mut(&id(ME)).unwrap());
        let mut r = request();
        r.enc_bindings = vec![EncBinding {
            contract: app().to_buffer(),
            purpose,
        }];
        let l = devnet(r);
        let grant = f.quoted_grant(&l, &Fixture::choice());
        f.approve(&l, &Fixture::choice(), &grant).unwrap();
        let updates = f.net.updates.lock().unwrap().clone();
        assert!(
            updates[0]
                .0
                .iter()
                .all(|k| k.purpose() == Purpose::AUTHENTICATION),
            "{case}: {:?}",
            updates[0].0
        );
        let records = f.engine.block_on(records(&f.s, f.wallet)).unwrap();
        assert!(records[0].encryption_keys.is_empty(), "{case}");
    }
    let f = Fixture::new();
    let mut r = request();
    r.enc_bindings = [EncPurpose::Encryption, EncPurpose::Decryption]
        .map(|purpose| EncBinding {
            contract: dashpay_contract::ID.to_buffer(),
            purpose,
        })
        .into();
    let plans = f
        .engine
        .block_on(encryption_plans(&*f.net, &r, &f.net.me()))
        .unwrap();
    assert_eq!(plans, vec![EncPlan::Omit; 2], "the DashPay contract");
}

#[test]
fn an_existing_encryption_key_is_shared_not_registered() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.approve(&l, &Fixture::choice(), &grant).unwrap();
    let mut second = request();
    second.app_eph_pub =
        PublicKey::from_secret_key(&SecretKey::from_secret_bytes([8u8; 32]).unwrap()).serialize();
    let l2 = devnet(second);
    let grant = f.quoted_grant(&l2, &Fixture::choice());
    f.approve(&l2, &Fixture::choice(), &grant).unwrap();
    let updates = f.net.updates.lock().unwrap().clone();
    let (add, disable) = &updates[1];
    assert_eq!(add.len(), 2, "no second encryption key");
    assert!(add.iter().all(|k| k.purpose() == Purpose::AUTHENTICATION));
    // The same connection's previous session keys are disabled.
    assert_eq!(disable, &vec![6, 7]);
    let published = f.net.published.lock().unwrap().clone();
    assert_eq!(published[1].1.payload.len(), 28 + 32 * 3);
}

#[test]
fn a_failed_send_keeps_the_response_for_retry() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant),
        Err(ConnectError::SendFailed)
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), 1);
    assert!(f.net.published.lock().unwrap().is_empty());
    assert_eq!(
        f.retry(&l),
        Err(ConnectError::SendFailed),
        "signed, not sent"
    );
    f.net.fail_publish.store(false, Ordering::SeqCst);
    f.retry(&l).unwrap();
    assert_eq!(
        f.net.updates.lock().unwrap().len(),
        1,
        "keys are added once"
    );
    assert_eq!(f.net.published.lock().unwrap().len(), 1);
    assert_eq!(
        f.retry(&l),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend
        })
    );
}

#[test]
fn a_group_changed_after_the_update_stops_the_response() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    *f.net.after_update.lock().unwrap() = Some(Box::new(|net: &FakeConnect| {
        net.groups
            .lock()
            .unwrap()
            .get_mut(&group())
            .unwrap()
            .admins
            .push(id(51));
    }));
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant),
        Err(ConnectError::Failed {
            failure: ConnectFailure::ScopeChanged
        })
    );
    assert!(f.net.published.lock().unwrap().is_empty());
}

/// Approving the request again after its keys landed reuses them: they
/// are sealed again and never disabled as a previous grant's.
#[test]
fn a_second_approval_reuses_its_keys_without_disabling_them() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    *f.net.after_update.lock().unwrap() = Some(Box::new(|net: &FakeConnect| {
        net.groups
            .lock()
            .unwrap()
            .get_mut(&group())
            .unwrap()
            .admins
            .push(id(51));
    }));
    assert!(f.approve(&l, &Fixture::choice(), &grant).is_err());
    let added: Vec<KeyID> = f.net.updates.lock().unwrap()[0]
        .0
        .iter()
        .map(|k| k.id())
        .collect();
    let again = f.quoted_grant(&l, &Fixture::choice());
    let done = f.approve(&l, &Fixture::choice(), &again).unwrap();
    assert!(done.keys_added.is_empty());
    assert_eq!(
        f.net.updates.lock().unwrap().len(),
        1,
        "nothing to add or disable"
    );
    let me = f.net.me();
    for k in &added {
        assert!(!me.public_keys()[k].is_disabled(), "key {k}");
    }
    assert_eq!(f.net.published.lock().unwrap().len(), 1);
}

/// A send that went out though it seemed to fail is not sent twice: the
/// retry finds it and finishes.
#[test]
fn a_retry_finds_a_response_that_went_out() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant),
        Err(ConnectError::SendFailed)
    );
    f.net
        .responses
        .lock()
        .unwrap()
        .insert((request().request_hash(), id(ME)));
    let before = f.net.signed().len();
    f.retry(&l).unwrap();
    assert_eq!(f.net.signed().len(), before, "nothing signed again");
}

#[test]
fn a_derived_key_on_the_identity_with_other_limits_fails() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.approve(&l, &Fixture::choice(), &grant).unwrap();
    f.net.responses.lock().unwrap().clear();
    let shorter = ConnectChoice {
        terms: ConnectTerms {
            expires_at_ms: NOW + 30 * DAY_MS,
            ..Fixture::choice().terms
        },
        ..Fixture::choice()
    };
    let grant = f.quoted_grant(&l, &shorter);
    assert_eq!(
        f.approve(&l, &shorter, &grant),
        Err(ConnectError::Failed {
            failure: ConnectFailure::KeyConflict
        })
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), 1);
}

/// No DTO, record, response or error a call returns carries the sealed
/// plaintext; the response's Debug shows its length only.
#[test]
fn nothing_returned_carries_the_payload() {
    let f = Fixture::new();
    let l = devnet(request());
    let sheet = f.sheet(&l).unwrap();
    let quote = f.quote(&l, &Fixture::choice()).unwrap();
    let grant = f.quoted_grant(&l, &Fixture::choice());
    let done = f.approve(&l, &Fixture::choice(), &grant).unwrap();
    let response = f.net.published.lock().unwrap()[0].1.clone();
    let payload = hex::encode(&response.payload);
    let records = f.engine.block_on(records(&f.s, f.wallet)).unwrap();
    let errors = [
        ConnectError::SendFailed,
        ConnectError::Failed {
            failure: ConnectFailure::KeyConflict,
        },
    ];
    for text in [
        format!("{sheet:?}{quote:?}{done:?}{records:?}{response:?}{errors:?}"),
        serde_json::to_string(&(&sheet, &quote, &done, &records)).unwrap(),
    ] {
        assert!(!text.contains(&payload));
        assert!(!text.contains(&payload[24..88]));
    }
    assert!(format!("{response:?}").contains("payload_len"));
}

#[test]
fn statuses_follow_the_dip() {
    use dw_uri::connect::Status;
    assert_eq!(ConnectError::Network.status(), Some(Status::Failed));
    assert_eq!(ConnectError::MainnetOff.status(), Some(Status::Unsupported));
    assert_eq!(ConnectError::Owner.status(), Some(Status::Invalid));
    assert_eq!(ConnectError::Stale.status(), Some(Status::Stale));
    assert_eq!(ConnectError::Hint.status(), Some(Status::Failed));
    assert_eq!(ConnectError::PlanChanged.status(), None);
    let failed = |failure| ConnectError::Failed { failure }.status();
    assert_eq!(failed(ConnectFailure::ScopeChanged), Some(Status::Failed));
    assert_eq!(failed(ConnectFailure::NotConfirmed), None);
    for open in [
        ConnectError::Credits {
            needed: 2,
            available: 1,
        },
        ConnectError::SendFailed,
        ConnectError::Answered,
        ConnectError::Platform(PlatformError::Timeout),
        ConnectError::Platform(PlatformError::InvalidArgument {
            detail: "funding limit".into(),
        }),
    ] {
        assert_eq!(open.status(), None, "{open:?}");
    }
}

#[test]
fn a_host_cannot_begin_an_app_connect_lease() {
    let f = Fixture::new();
    let grant = f.grant(request().request_hash(), [0; 32], u64::MAX / 2);
    let begun = f.engine.block_on(f.s.begin_flow(
        f.wallet,
        super::super::flows::FlowKind::AppConnect,
        vec![grant.clone()],
        None,
    ));
    assert_eq!(begun, Err(PlatformError::GrantInvalid));
    assert!(f.unused(&grant));
}

/// An app's expiry under a day is raised to a day, so the default terms can
/// always be approved.
#[test]
fn a_short_app_expiry_defaults_to_a_day() {
    use super::super::app_connect_plan::{default_terms, terms_allowed};
    let mut r = request();
    for b in &mut r.auth_bindings {
        b.expires_at_ms = Some(r.created_at_ms + 60_000);
    }
    let terms = default_terms(&r, r.created_at_ms);
    assert_eq!(terms.expires_at_ms, r.created_at_ms + DAY_MS);
    assert!(terms_allowed(&terms, r.created_at_ms));
}

/// A contract binding says whether its contract can hold tokens: it has
/// some, or it isn't read-only, so an update could add them.
#[test]
fn a_contract_binding_says_whether_tokens_can_move() {
    let f = Fixture::new();
    for (has_tokens, readonly, tokens) in [
        (true, false, true),
        (true, true, true),
        (false, false, true),
        (false, true, false),
    ] {
        {
            let mut contracts = f.net.contracts.lock().unwrap();
            let c = contracts.get_mut(&app()).unwrap();
            (c.has_tokens, c.readonly) = (has_tokens, readonly);
        }
        let mut r = request();
        r.auth_bindings[0].bounds = Bounds::Contract(app().to_buffer());
        let sheet = f.sheet(&devnet(r)).unwrap();
        assert_eq!(
            sheet.bindings[0].scope,
            ConnectScope::Contract {
                contract: b58(&app()),
                tokens
            }
        );
    }
}

/// Keys Platform showed late: the request stays open, and approving it
/// again re-reads the identity, finds them and adds nothing a second time.
#[test]
fn approving_again_after_not_confirmed_reuses_keys_that_landed_late() {
    let f = Fixture::new();
    let l = devnet(request());
    let before = f.net.me();
    *f.net.stale.lock().unwrap() = Some(before.clone());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    let failed = f.approve(&l, &Fixture::choice(), &grant).unwrap_err();
    assert_eq!(
        failed,
        ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed
        }
    );
    assert_eq!(failed.status(), None);
    assert_eq!(
        f.sheet(&l).unwrap().earlier_approval,
        Some(Fixture::choice()),
        "keys may be on the identity"
    );
    let added = f.net.updates.lock().unwrap()[0].0.clone();
    assert!(!added.is_empty());
    *f.net.stale.lock().unwrap() = None;
    let again = f.quoted_grant(&l, &Fixture::choice());
    let done = f.approve(&l, &Fixture::choice(), &again).unwrap();
    assert!(done.keys_added.is_empty());
    assert_eq!(f.net.updates.lock().unwrap().len(), 1, "no second update");
    assert_eq!(
        f.sheet(&l).unwrap_err(),
        ConnectError::Answered,
        "its response is published"
    );
    let me = f.net.me();
    for k in &added {
        let copies = (me.public_keys().values()).filter(|on| on.data() == k.data());
        assert_eq!(copies.count(), 1, "key {}", k.id());
    }
    assert_eq!(f.net.published.lock().unwrap().len(), 1);
}

/// A sheet opened again after `not_confirmed`, with a later block time
/// and so other default terms, offers the earlier choice: approving it
/// reuses the keys that landed instead of conflicting with them.
#[test]
fn a_reopened_sheet_offers_the_earlier_choice() {
    let f = Fixture::new();
    let mut r = request();
    for b in &mut r.auth_bindings {
        b.expires_at_ms = None;
    }
    let l = devnet(r);
    *f.net.stale.lock().unwrap() = Some(f.net.me());
    let first = f.sheet(&l).unwrap().terms;
    let earlier = ConnectChoice {
        terms: first,
        ..Fixture::choice()
    };
    let grant = f.quoted_grant(&l, &earlier);
    assert_eq!(
        f.approve(&l, &earlier, &grant).unwrap_err(),
        ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed
        }
    );
    *f.net.stale.lock().unwrap() = None;
    *f.net.block_time.lock().unwrap() = NOW + 5 * 60 * 1000;
    let sheet = f.sheet(&l).unwrap();
    assert_ne!(sheet.terms, first, "the defaults moved with the block time");
    let defaults = ConnectChoice {
        terms: sheet.terms,
        ..Fixture::choice()
    };
    let conflicting = f.quoted_grant(&l, &defaults);
    assert_eq!(
        f.approve(&l, &defaults, &conflicting).unwrap_err(),
        ConnectError::Failed {
            failure: ConnectFailure::KeyConflict
        },
        "the new defaults conflict with the keys that landed"
    );
    let offered = sheet.earlier_approval.expect("an earlier approval");
    assert_eq!(offered, earlier);
    let again = f.quoted_grant(&l, &offered);
    let done = f.approve(&l, &offered, &again).unwrap();
    assert!(done.keys_added.is_empty());
    assert_eq!(f.net.updates.lock().unwrap().len(), 1, "no second update");
    assert_eq!(f.net.published.lock().unwrap().len(), 1);
}

/// An update that never landed is signed again from a fresh read, once.
#[test]
fn approving_again_after_not_confirmed_adds_keys_that_never_landed() {
    let f = Fixture::new();
    let l = devnet(request());
    f.net.lose_updates.store(true, Ordering::SeqCst);
    let grant = f.quoted_grant(&l, &Fixture::choice());
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant).unwrap_err(),
        ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed
        }
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let again = f.quoted_grant(&l, &Fixture::choice());
    let done = f.approve(&l, &Fixture::choice(), &again).unwrap();
    let updates = f.net.updates.lock().unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].0, updates[1].0, "the same keys, at the same ids");
    assert_eq!(
        done.keys_added,
        updates[1].0.iter().map(|k| k.id()).collect::<Vec<_>>()
    );
    let me = f.net.me();
    for k in &updates[1].0 {
        let copies = (me.public_keys().values()).filter(|on| on.data() == k.data());
        assert_eq!(copies.count(), 1, "key {}", k.id());
    }
}

/// Renewing a connection whose last approval never landed leaves alone a
/// key that has since taken one of its ids.
#[test]
fn a_renewal_keeps_a_key_that_took_an_unlanded_id() {
    let f = Fixture::new();
    let mut first = request();
    first.connection_id = Some([9; 16]);
    let l = devnet(first.clone());
    f.net.lose_updates.store(true, Ordering::SeqCst);
    let grant = f.quoted_grant(&l, &Fixture::choice());
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant).unwrap_err(),
        ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed
        }
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let taken = (f.net.updates.lock().unwrap()[0].0.iter())
        .find(|k| k.purpose() == Purpose::AUTHENTICATION)
        .unwrap()
        .id();
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let mut other = master_key(me).unwrap().clone();
        other.set_id(taken);
        other.set_purpose(Purpose::ENCRYPTION);
        other.set_security_level(SecurityLevel::MEDIUM);
        me.public_keys_mut().insert(taken, other);
    }
    let mut again = first;
    again.app_eph_pub =
        PublicKey::from_secret_key(&SecretKey::from_secret_bytes([8u8; 32]).unwrap()).serialize();
    let l2 = devnet(again);
    let grant = f.quoted_grant(&l2, &Fixture::choice());
    f.approve(&l2, &Fixture::choice(), &grant).unwrap();
    let updates = f.net.updates.lock().unwrap().clone();
    assert!(!updates[1].1.contains(&taken), "{:?}", updates[1].1);
    assert!(!f.net.me().public_keys()[&taken].is_disabled());
}

/// An update Platform refused says so, instead of claiming the keys may
/// still land; the request stays open and approving again succeeds.
#[test]
fn a_refused_update_is_not_reported_as_unconfirmed() {
    let f = Fixture::new();
    let l = devnet(request());
    *f.net.refuse_update.lock().unwrap() = Some(PlatformError::InvalidArgument {
        detail: "revision".into(),
    });
    let grant = f.quoted_grant(&l, &Fixture::choice());
    let refused = f.approve(&l, &Fixture::choice(), &grant).unwrap_err();
    assert!(matches!(refused, ConnectError::Platform(_)), "{refused:?}");
    assert!(refused.keeps_open());
    assert!(f.net.updates.lock().unwrap().is_empty());
    assert!(f.net.published.lock().unwrap().is_empty());
    let again = f.quoted_grant(&l, &Fixture::choice());
    f.approve(&l, &Fixture::choice(), &again).unwrap();
    assert_eq!(f.net.published.lock().unwrap().len(), 1);
}

/// A failed broadcast, which the library reports as an internal error, or
/// a timeout still reads as not confirmed: its keys may land.
#[test]
fn an_unknown_update_outcome_stays_not_confirmed() {
    for e in [
        PlatformError::Timeout,
        PlatformError::Internal {
            detail: "Failed to broadcast identity update".into(),
        },
    ] {
        let f = Fixture::new();
        let l = devnet(request());
        *f.net.refuse_update.lock().unwrap() = Some(e.clone());
        let grant = f.quoted_grant(&l, &Fixture::choice());
        assert_eq!(
            f.approve(&l, &Fixture::choice(), &grant).unwrap_err(),
            ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed
            },
            "{e:?}"
        );
    }
}

/// An identity with no key that may sign the response is refused before
/// any key is added or credit spent.
#[test]
fn no_response_key_is_refused_before_the_update() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let answering: Vec<KeyID> = (me.public_keys().values())
            .filter(|k| {
                k.purpose() == Purpose::AUTHENTICATION
                    && matches!(
                        k.security_level(),
                        SecurityLevel::HIGH | SecurityLevel::CRITICAL
                    )
            })
            .map(|k| k.id())
            .collect();
        assert!(!answering.is_empty());
        for k in answering {
            me.public_keys_mut().remove(&k);
        }
    }
    assert_eq!(
        f.approve(&l, &Fixture::choice(), &grant).unwrap_err(),
        ConnectError::Failed {
            failure: ConnectFailure::NoResponseKey
        }
    );
    assert!(f.net.updates.lock().unwrap().is_empty());
    assert!(f.net.signed().is_empty());
}

/// `nothing_to_send` asks the user to start again from the app. The app's
/// new request has a new ephemeral key, so it derives a second set of
/// session keys: one more update adds them, and disables the unsent set
/// when the app names the same connection, which only this wallet could
/// sign with. The encryption key is shared, not registered again.
#[test]
fn starting_again_after_nothing_to_send_adds_a_second_set_and_disables_the_first() {
    for connection in [Some([9; 16]), None] {
        let f = Fixture::new();
        let mut first = request();
        first.connection_id = connection;
        let l = devnet(first);
        let grant = f.quoted_grant(&l, &Fixture::choice());
        f.net.fail_publish.store(true, Ordering::SeqCst);
        assert_eq!(
            f.approve(&l, &Fixture::choice(), &grant),
            Err(ConnectError::SendFailed)
        );
        f.net.fail_publish.store(false, Ordering::SeqCst);
        drop(
            f.s.platform
                .app_connect
                .take(f.wallet, l.request.request_hash()),
        );
        assert_eq!(
            f.retry(&l),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NothingToSend
            })
        );
        let unsent: Vec<KeyID> = (f.net.updates.lock().unwrap()[0].0.iter())
            .filter(|k| k.purpose() == Purpose::AUTHENTICATION)
            .map(|k| k.id())
            .collect();
        let mut again = request();
        again.connection_id = connection;
        again.app_eph_pub =
            PublicKey::from_secret_key(&SecretKey::from_secret_bytes([8u8; 32]).unwrap())
                .serialize();
        let l2 = devnet(again);
        let grant = f.quoted_grant(&l2, &Fixture::choice());
        let done = f.approve(&l2, &Fixture::choice(), &grant).unwrap();
        let updates = f.net.updates.lock().unwrap().clone();
        assert_eq!(updates.len(), 2, "one more update");
        let (add, disable) = &updates[1];
        assert_eq!(
            add.len(),
            2,
            "a second set of session keys, no second encryption key"
        );
        assert!(add.iter().all(|k| k.purpose() == Purpose::AUTHENTICATION));
        assert!(add.iter().all(|k| !unsent.contains(&k.id())));
        assert_eq!(done.keys_added.len(), 2);
        let me = f.net.me();
        match connection {
            Some(_) => {
                assert_eq!(disable, &unsent);
                assert!(unsent.iter().all(|k| me.public_keys()[k].is_disabled()));
            }
            None => {
                assert!(disable.is_empty());
                assert!(unsent.iter().all(|k| !me.public_keys()[k].is_disabled()));
            }
        }
    }
}
