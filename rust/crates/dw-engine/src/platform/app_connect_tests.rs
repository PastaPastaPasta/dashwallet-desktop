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
use super::super::app_connect_plan::{
    ContractFacts, DAY_MS, GroupFacts, GroupInfoFacts, MAX_FUNDING,
};
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
    /// Optional override for the live disconnect fee, for fee-bound tests.
    disconnect_fee_override: Mutex<Option<u64>>,
    /// Per-identity nonce state (low 40 bits: tip; upper 24 bits: window).
    nonces: Mutex<HashMap<Identifier, u64>>,
    /// Optional lagging nonce state override for split-node retry tests.
    stale_nonce: Mutex<Option<u64>>,
    /// Each disable-only transition signed: `(target_revision, nonce, disable_keys)`.
    signed_disables: Mutex<Vec<(u64, u64, Vec<KeyID>)>>,
    /// Each raw disable-only transition payload broadcast.
    broadcast_disables: Mutex<Vec<Vec<u8>>>,
    /// Proved block height reported by identity / refresh reads.
    identity_height: Mutex<u64>,
    /// Proved block height reported by identity_nonce reads.
    nonce_height: Mutex<u64>,
    /// Optional override for serialized ContractGroupInfo byte lengths.
    group_info_bytes: Mutex<HashMap<Identifier, usize>>,
    /// Optional error returned by `publish` instead of `Timeout`.
    fail_publish_err: Mutex<Option<PlatformError>>,
    /// When non-zero, a revision-conflict disable broadcast debits this fee
    /// and consumes its nonce (mirroring Drive's paid revision conflict).
    paid_conflict_fee: Mutex<u64>,
    /// Simulate a group member pagination failure in `group_all` (E3).
    fail_group_all: AtomicBool,
    /// Simulate a `group_info` lookup failure for specific groups (E3).
    fail_group_info: Mutex<BTreeSet<Identifier>>,
    /// Simulate a timed-out credit transfer broadcast (`BroadcastUnknown`, E1).
    transfer_timeout: AtomicBool,
    /// Every credit transfer transition broadcast via `WithdrawNet`.
    transfers_sent: Mutex<Vec<dpp::state_transition::StateTransition>>,
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
                serialized_bytes: 4_096,
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
                serialized_bytes: 8_192,
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
            disconnect_fee_override: Mutex::default(),
            nonces: Mutex::default(),
            stale_nonce: Mutex::default(),
            signed_disables: Mutex::default(),
            broadcast_disables: Mutex::default(),
            identity_height: Mutex::new(100),
            nonce_height: Mutex::new(100),
            group_info_bytes: Mutex::default(),
            fail_publish_err: Mutex::default(),
            paid_conflict_fee: Mutex::new(0),
            fail_group_all: AtomicBool::new(false),
            fail_group_info: Mutex::default(),
            transfer_timeout: AtomicBool::new(false),
            transfers_sent: Mutex::default(),
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

    async fn identity(
        &self,
        id: Identifier,
    ) -> Result<(Option<Identity>, u64, u64), PlatformError> {
        self.read("identity");
        let stale = self.stale.lock().unwrap().clone();
        // A lagging node (`stale`) serves an older proved block height than the tip.
        let height = if stale.is_some() {
            self.identity_height.lock().unwrap().saturating_sub(1)
        } else {
            *self.identity_height.lock().unwrap()
        };
        let found = stale.or_else(|| self.identities.lock().unwrap().get(&id).cloned());
        Ok((found, *self.block_time.lock().unwrap(), height))
    }

    async fn contract(&self, id: Identifier) -> Result<Option<ContractFacts>, PlatformError> {
        self.read("contract");
        Ok(self.contracts.lock().unwrap().get(&id).cloned())
    }

    async fn group(&self, id: Identifier) -> Result<Option<GroupFacts>, PlatformError> {
        self.read("group");
        // Mirror SdkConnect::members(..., Some(MAX_GROUP_MEMBERS)): once earlier
        // categories cross MAX_GROUP_MEMBERS, later categories (tokens) are not
        // fetched because approval refuses any group over MAX_GROUP_MEMBERS.
        Ok(self.groups.lock().unwrap().get(&id).cloned().map(|mut g| {
            if g.contracts.len() > plan::MAX_GROUP_MEMBERS {
                g.document_types.clear();
                g.tokens.clear();
            } else if g.contracts.len() + g.document_types.len() > plan::MAX_GROUP_MEMBERS {
                g.tokens.clear();
            }
            g
        }))
    }

    async fn group_all(&self, id: Identifier) -> Result<Option<GroupFacts>, PlatformError> {
        self.read("group_all");
        if self.fail_group_all.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        Ok(self.groups.lock().unwrap().get(&id).cloned())
    }

    async fn group_info(&self, id: Identifier) -> Result<Option<GroupInfoFacts>, PlatformError> {
        self.read("group_info");
        if self.fail_group_info.lock().unwrap().contains(&id) {
            return Err(PlatformError::Unavailable);
        }
        let bytes_override = self.group_info_bytes.lock().unwrap().get(&id).copied();
        Ok(self.groups.lock().unwrap().get(&id).cloned().map(|g| {
            let serialized_bytes = bytes_override.unwrap_or_else(|| 64 + 32 * g.admins.len());
            GroupInfoFacts {
                owner: g.owner,
                admins: g.admins,
                serialized_bytes,
            }
        }))
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

    async fn refresh(
        &self,
        _: &PlatformWallet,
        id: Identifier,
    ) -> Result<(Identity, u64), PlatformError> {
        self.read("refresh");
        self.refreshed.store(true, Ordering::SeqCst);
        let stale = self.stale.lock().unwrap().clone();
        // A lagging node (`stale`) serves an older proved block height than the tip.
        let height = if stale.is_some() {
            self.identity_height.lock().unwrap().saturating_sub(1)
        } else {
            *self.identity_height.lock().unwrap()
        };
        let found = stale.or_else(|| self.identities.lock().unwrap().get(&id).cloned());
        found
            .map(|id| (id, height))
            .ok_or(PlatformError::Unavailable)
    }

    async fn update_identity(
        &self,
        _: &PlatformWallet,
        id: Identifier,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        signer: &ConnectUpdateSigner,
    ) -> Result<(), PlatformError> {
        let master = self
            .identities
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .public_keys()
            .values()
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

    async fn identity_nonce(&self, id: Identifier) -> Result<(u64, u64), PlatformError> {
        self.read("identity_nonce");
        let height = *self.nonce_height.lock().unwrap();
        if let Some(stale) = *self.stale_nonce.lock().unwrap() {
            return Ok((stale, height));
        }
        Ok((*self.nonces.lock().unwrap().get(&id).unwrap_or(&0), height))
    }

    async fn sign_disable_keys(
        &self,
        identity: &Identity,
        master_key_id: KeyID,
        disable: Vec<KeyID>,
        nonce: u64,
        signer: &VaultIdentitySigner,
    ) -> Result<Vec<u8>, PlatformError> {
        let master = identity
            .public_keys()
            .get(&master_key_id)
            .cloned()
            .ok_or(PlatformError::SignerUnavailable)?;
        signer
            .sign(&master, b"disable")
            .await
            .map_err(|_| PlatformError::SignerUnavailable)?;
        self.signed.lock().unwrap().push(master.id());
        if !self.refreshed.swap(false, Ordering::SeqCst) {
            return Err(PlatformError::Internal {
                detail: "Failed to broadcast identity update".into(),
            });
        }
        let target_revision = identity.revision() + 1;
        self.signed_disables
            .lock()
            .unwrap()
            .push((target_revision, nonce, disable.clone()));
        serde_json::to_vec(&(target_revision, nonce, disable)).map_err(|e| {
            PlatformError::Internal {
                detail: format!("encode fake disable: {e}"),
            }
        })
    }

    async fn broadcast_disable_keys(
        &self,
        _: &PlatformWallet,
        id: Identifier,
        transition_bytes: &[u8],
    ) -> Result<(), PlatformError> {
        self.broadcast_disables
            .lock()
            .unwrap()
            .push(transition_bytes.to_vec());
        if let Some(e) = self.refuse_update.lock().unwrap().take() {
            return Err(e);
        }
        let (target_revision, nonce, disable): (u64, u64, Vec<KeyID>) =
            serde_json::from_slice(transition_bytes).map_err(|e| PlatformError::Internal {
                detail: format!("decode fake disable: {e}"),
            })?;
        if !self.lose_updates.load(Ordering::SeqCst) {
            let mut nonces = self.nonces.lock().unwrap();
            let cur_nonce = *nonces.get(&id).unwrap_or(&0);
            if !super::super::withdrawal::takes(cur_nonce, nonce, id) {
                // Mirror CheckTx rejecting a duplicate/used nonce for free.
                return Err(PlatformError::Unavailable);
            }
            let mut all = self.identities.lock().unwrap();
            let me = all.get_mut(&id).unwrap();
            if me.revision() + 1 != target_revision {
                let conflict_fee = *self.paid_conflict_fee.lock().unwrap();
                if conflict_fee > 0 {
                    // Mirror Drive charging a processing fee & spending the nonce
                    // on a paid revision conflict without changing keys or revision.
                    let bal = me.balance().saturating_sub(conflict_fee);
                    me.set_balance(bal);
                    nonces.insert(id, nonce);
                }
                return Err(PlatformError::Internal {
                    detail: format!(
                        "wrong revision: expected {}, got {target_revision}",
                        me.revision() + 1
                    ),
                });
            }
            me.set_revision(target_revision);
            nonces.insert(id, nonce);
            for d in &disable {
                if let Some(k) = me.public_keys_mut().get_mut(d) {
                    k.set_disabled_at(NOW);
                }
            }
            self.updates.lock().unwrap().push((Vec::new(), disable));
        }
        if let Some(after) = self.after_update.lock().unwrap().take() {
            after(self);
        }
        Ok(())
    }

    async fn disconnect_fee(&self, specs: &[DisconnectKeyFeeSpec]) -> Result<u64, PlatformError> {
        self.read("disconnect_fee");
        if let Some(fee) = *self.disconnect_fee_override.lock().unwrap() {
            return Ok(fee);
        }
        disconnect_fee_at(self.protocol.load(Ordering::SeqCst), specs)
    }

    async fn publish(
        &self,
        _: &PlatformWallet,
        owner: Identifier,
        response: &Response,
        signer: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        let key = self
            .identities
            .lock()
            .unwrap()
            .get(&owner)
            .unwrap()
            .public_keys()
            .values()
            .find(|k| signer.can_sign_with(k))
            .cloned()
            .expect("a key the response signs with");
        signer
            .sign(&key, b"response")
            .await
            .map_err(|_| PlatformError::SignerUnavailable)?;
        self.signed.lock().unwrap().push(key.id());
        if let Some(err) = self.fail_publish_err.lock().unwrap().take() {
            return Err(err);
        }
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

impl crate::platform::withdrawal::WithdrawNet for FakeConnect {
    fn version(&self) -> &'static dpp::version::PlatformVersion {
        dpp::version::PlatformVersion::get(self.protocol.load(Ordering::SeqCst))
            .unwrap_or_else(|_| dpp::version::PlatformVersion::latest())
    }

    async fn balance(&self, id: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(self
            .identities
            .lock()
            .unwrap()
            .get(&id)
            .map(IdentityGettersV0::balance))
    }

    async fn nonce_state(&self, id: Identifier) -> Result<(u64, u64), PlatformError> {
        let state = (*self.stale_nonce.lock().unwrap())
            .unwrap_or_else(|| *self.nonces.lock().unwrap().get(&id).unwrap_or(&0));
        Ok((state, *self.nonce_height.lock().unwrap()))
    }

    async fn broadcast(
        &self,
        transition: dpp::state_transition::StateTransition,
    ) -> Result<u64, crate::platform::withdrawal::Unsent> {
        use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;
        self.transfers_sent.lock().unwrap().push(transition.clone());
        if self.transfer_timeout.load(Ordering::SeqCst) {
            return Err(crate::platform::withdrawal::Unsent::Unknown(
                PlatformError::Timeout,
            ));
        }
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) =
            transition
        else {
            return Err(crate::platform::withdrawal::Unsent::Refused(
                PlatformError::Internal {
                    detail: "unexpected transition in FakeConnect::WithdrawNet".into(),
                },
            ));
        };
        let owner = t.identity_id();
        let nonce = t.nonce();
        let mut nonces = self.nonces.lock().unwrap();
        let cur = *nonces.get(&owner).unwrap_or(&0);
        if !crate::platform::withdrawal::takes(cur, nonce, owner) {
            return Err(crate::platform::withdrawal::Unsent::Refused(
                PlatformError::Unavailable,
            ));
        }
        let cur_tip = cur & IDENTITY_NONCE_VALUE_FILTER;
        nonces.insert(owner, cur_tip.max(nonce));
        drop(nonces);
        let spent: u64 = t.recipient_addresses().values().copied().sum();
        let mut all = self.identities.lock().unwrap();
        let me = all.get_mut(&owner).unwrap();
        let remaining = me.balance().saturating_sub(spent + 10_000_000);
        me.set_balance(remaining);
        Ok(remaining)
    }

    async fn withdrawal(
        &self,
        _id: Identifier,
    ) -> Result<(Option<dpp::document::Document>, u64), PlatformError> {
        Ok((None, *self.nonce_height.lock().unwrap()))
    }

    async fn withdrawals(
        &self,
        _owner: Identifier,
        _limit: u32,
    ) -> Result<Vec<dpp::document::Document>, PlatformError> {
        Ok(Vec::new())
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

    fn list_apps(&self, identity: &str) -> Result<ConnectedApps, ConnectError> {
        let (net, s, w, id) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet,
            identity.to_owned(),
        );
        self.engine
            .block_on(async move { list(&*net, &s, w, &id).await })
    }

    fn disconnect_quote(
        &self,
        identity: &str,
        request: &str,
    ) -> Result<DisconnectQuote, ConnectError> {
        let (net, s, w) = (Arc::clone(&self.net), Arc::clone(&self.s), self.wallet);
        let (identity, request) = (identity.to_owned(), request.to_owned());
        self.engine
            .block_on(async move { disconnect_quote(&*net, &s, w, &identity, &request).await })
    }

    fn disconnect_grant(&self, request: [u8; 20], max_credits: u64) -> String {
        self.disconnect_grant_for(id(ME).to_buffer(), request, max_credits)
    }

    fn disconnect_grant_for(
        &self,
        identity: [u8; 32],
        request: [u8; 20],
        max_credits: u64,
    ) -> String {
        let purpose = GrantPurpose::AppDisconnect {
            max_credits,
            identity,
            request,
        };
        (self
            .s
            .vault
            .authorize(purpose, Some(&self.wallet.0), Credential::None))
        .unwrap()
        .id
    }

    fn withdraw_grant(&self, max_credits: u64) -> String {
        let purpose = GrantPurpose::PlatformOp {
            max_duffs: 0,
            max_credits,
        };
        (self
            .s
            .vault
            .authorize(purpose, Some(&self.wallet.0), Credential::None))
        .unwrap()
        .id
    }

    fn transfer_quote(
        &self,
        identity: &str,
        credits: u64,
    ) -> Result<
        crate::platform::credits::WithdrawToPlatformQuote,
        crate::platform::errors::CreditsError,
    > {
        let (net, s, pw, id_str) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet(),
            identity.to_owned(),
        );
        self.engine.block_on(async move {
            crate::platform::credit_transfer::quote(
                &*net,
                &s,
                &pw,
                s.vault.network(),
                &id_str,
                crate::platform::credits::WithdrawAmount::Credits { credits },
            )
            .await
        })
    }

    fn transfer_to_platform(
        &self,
        identity: &str,
        to: &str,
        credits: u64,
        grant: &str,
    ) -> Result<
        crate::platform::credits::WithdrawToPlatformOutcome,
        crate::platform::errors::CreditsError,
    > {
        let (net, s, pw, w) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet(),
            self.wallet,
        );
        let (id_str, to, grant) = (identity.to_owned(), to.to_owned(), grant.to_owned());
        self.engine.block_on(async move {
            crate::platform::credit_transfer::transfer(
                &*net,
                &s,
                &pw,
                (w, &id_str),
                (
                    &to,
                    crate::platform::credits::WithdrawAmount::Credits { credits },
                ),
                &grant,
            )
            .await
        })
    }

    /// Registers a second identity in this wallet (index 1, username "bob").
    fn add_second_identity(&self, id_byte: u8) -> Identifier {
        let second_id = id(id_byte);
        let grant = self
            .s
            .vault
            .authorize(
                GrantPurpose::PlatformOp {
                    max_duffs: 0,
                    max_credits: 1,
                },
                Some(&self.wallet.0),
                Credential::None,
            )
            .unwrap()
            .id;
        let lease_id = self
            .engine
            .block_on(self.s.begin_flow(
                self.wallet,
                FlowKind::EnableDashPayKeys,
                vec![grant],
                None,
            ))
            .unwrap();
        let lease = self.s.lease_for(&self.wallet, &lease_id).unwrap().unwrap();
        let signer = lease.identity_signer(&[1]).unwrap();
        let real = self
            .engine
            .block_on(
                signer.derive_public_keys(1, &crate::platform::keys_policy::registration_specs()),
            )
            .unwrap();
        lease.end();
        let derived: BTreeMap<KeyID, IdentityPublicKey> = keys(true)
            .into_keys()
            .map(|k| (k, real[&k].clone()))
            .collect();
        let second = identity(id_byte, derived, BALANCE);
        self.net
            .identities
            .lock()
            .unwrap()
            .insert(second_id, second.clone());
        let manager = self.s.manager().unwrap();
        let w = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&w.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let persister = wallet.persister();
            (state.identity_manager)
                .add_identity(second, 1, wallet.wallet_id(), persister)
                .unwrap();
            let names = vec![DpnsNameInfo {
                label: "bob".into(),
                acquired_at: Some(1),
            }];
            (state.identity_manager.managed_identity_mut(&second_id))
                .unwrap()
                .set_dpns_names(names, persister);
        });
        second_id
    }

    fn disconnect_grant_unused(&self, grant: &str) -> bool {
        (self.s.vault)
            .check_grant(grant, GrantKind::AppDisconnect, Some(&self.wallet.0))
            .is_ok()
    }

    fn disconnect(
        &self,
        identity: &str,
        request: &str,
        grant: &str,
    ) -> Result<DisconnectDone, ConnectError> {
        let (net, s, w, pw) = (
            Arc::clone(&self.net),
            Arc::clone(&self.s),
            self.wallet,
            self.wallet(),
        );
        let (identity, request, grant) =
            (identity.to_owned(), request.to_owned(), grant.to_owned());
        self.engine.block_on(async move {
            disconnect(&*net, &s, &pw, w, &identity, &request, grant).await
        })
    }

    fn put_record(&self, record: ConnectRecord) {
        let (s, w) = (Arc::clone(&self.s), self.wallet);
        self.engine
            .block_on(async move { put_record(&s, w, record).await.unwrap() });
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
    type Edit = fn(&mut Identity);
    let cases: [(&str, EncPurpose, Edit); 3] = [
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
        (f.net.contracts.lock().unwrap().get_mut(&app()).unwrap()).decryption_requirement = Some(2);
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

/// A current app key that holds the lowest id of its purpose is the key
/// platform-wallet picks for contact requests: it is not shared. Behind a
/// lower-id key of its purpose, it is.
#[test]
fn an_app_key_first_in_line_for_contact_requests_is_not_shared() {
    for (keep_dashpay_key, expected) in [
        (false, EncPlan::Omit),
        (
            true,
            EncPlan::Share {
                key_id: 6,
                rotation: None,
            },
        ),
    ] {
        let f = Fixture::new();
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            if !keep_dashpay_key {
                me.public_keys_mut()
                    .retain(|_, k| k.purpose() != Purpose::ENCRYPTION);
            }
            let mut app_key = master_key(me).unwrap().clone();
            app_key.set_id(6);
            app_key.set_purpose(Purpose::ENCRYPTION);
            app_key.set_security_level(SecurityLevel::MEDIUM);
            if let IdentityPublicKey::V0(v) = &mut app_key {
                v.contract_bounds = Some(ContractBounds::SingleContract { id: app() });
            }
            me.public_keys_mut().insert(6, app_key);
        }
        (f.net.current.lock().unwrap()).insert((id(ME), app(), Purpose::ENCRYPTION), 6);
        let plans = f
            .engine
            .block_on(encryption_plans(&*f.net, &request(), &f.net.me()))
            .unwrap();
        assert_eq!(
            plans,
            vec![expected],
            "DashPay key kept: {keep_dashpay_key}"
        );
    }
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
                .take_for(f.wallet, id(ME), l.request.request_hash()),
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

fn devnet_with_callback(request: AppConnectRequest, cb: &str) -> ConnectUri {
    let body = dw_uri::connect::base64url_encode(&request.encode());
    let uri = format!("dashpay://connect?v=2&n=d&r={body}&cb={cb}");
    dw_uri::connect::parse(&uri).unwrap()
}

fn second_app_request() -> AppConnectRequest {
    let sk = SecretKey::from_secret_bytes([11u8; 32]).unwrap();
    let eph = PublicKey::from_secret_key(&sk).serialize();
    let mut r = request();
    r.app_eph_pub = eph;
    r.label = "SecondApp".into();
    r.connection_id = Some([12; 16]);
    r.auth_bindings = vec![AuthBinding {
        bounds: Bounds::DocumentType(app().to_buffer(), "post".into()),
        level: Level::High,
        budget: Some(10_000_000_000),
        expires_at_ms: Some(NOW + 30 * DAY_MS),
    }];
    r.enc_bindings = Vec::new();
    r
}

fn make_session_key_v1(
    key_id: KeyID,
    purpose: Purpose,
    security_level: SecurityLevel,
    key_type: KeyType,
    bounds: Option<ContractBounds>,
    data: Vec<u8>,
    with_limits: bool,
) -> IdentityPublicKey {
    use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
    use dpp::identity::identity_public_key::v1::IdentityPublicKeyV1;
    let v0 = IdentityPublicKeyV0 {
        id: key_id,
        purpose,
        security_level,
        contract_bounds: bounds,
        key_type,
        read_only: false,
        data: dpp::platform_value::BinaryData::new(data),
        disabled_at: None,
    };
    if with_limits {
        IdentityPublicKey::V1(IdentityPublicKeyV1::from_v0_with_limits(
            v0,
            Some(10_000_000_000),
            Some(NOW + 30 * DAY_MS),
        ))
    } else {
        IdentityPublicKey::V0(v0)
    }
}

#[test]
fn list_after_approve_shows_app_details_and_only_enabled_keys() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let empty = f.list_apps(&me_b58).unwrap();
    assert!(empty.apps.is_empty());

    let l = devnet_with_callback(request(), "https://yappr.example/callback");
    let choice = Fixture::choice();
    let grant = f.quoted_grant(&l, &choice);
    f.approve(&l, &choice, &grant).unwrap();

    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 1);
    let item = &listed.apps[0];
    assert_eq!(item.request, hex::encode(l.request.request_hash()));
    assert_eq!(item.app_id, b58(&app()));
    assert_eq!(item.label, "Yappr");
    assert_eq!(item.returns_to.as_deref(), Some("yappr.example"));
    assert_eq!(item.funding_limit, choice.terms.funding_limit);
    assert_eq!(item.expires_at_ms, Some(choice.terms.expires_at_ms));
    assert!(item.approved_at_ms > 0);
    assert_eq!(
        item.keys.len(),
        2,
        "only the 2 session keys, never encryption"
    );
    assert_eq!(item.keys[0].level, ConnectLevel::Critical);
    assert!(matches!(item.keys[0].scope, ConnectScope::Group(_)));
    assert_eq!(item.keys[1].level, ConnectLevel::High);
    assert_eq!(item.keys[1].scope, ConnectScope::Profile);

    // Disable one key directly on Platform: only the remaining enabled key is shown.
    let first_key = item.keys[0].key_id;
    let second_key = item.keys[1].key_id;
    {
        let mut all = f.net.identities.lock().unwrap();
        all.get_mut(&id(ME))
            .unwrap()
            .public_keys_mut()
            .get_mut(&first_key)
            .unwrap()
            .set_disabled_at(NOW);
    }
    let after_one = f.list_apps(&me_b58).unwrap();
    assert_eq!(after_one.apps.len(), 1);
    assert_eq!(after_one.apps[0].keys.len(), 1);
    assert_eq!(after_one.apps[0].keys[0].key_id, second_key);

    // Disable the remaining key: the app leaves the list.
    {
        let mut all = f.net.identities.lock().unwrap();
        all.get_mut(&id(ME))
            .unwrap()
            .public_keys_mut()
            .get_mut(&second_key)
            .unwrap()
            .set_disabled_at(NOW);
    }
    let after_all = f.list_apps(&me_b58).unwrap();
    assert!(after_all.apps.is_empty());
}

#[test]
fn disconnect_yappr_disables_both_bounded_keys_and_leaves_all_other_keys_untouched() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    // Add a non-app DashPay encryption key to make sure Disconnect never touches it.
    let dashpay_enc_id = 4;
    let dashpay_dec_id = 5;
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        me.public_keys_mut().insert(
            dashpay_enc_id,
            make_session_key_v1(
                dashpay_enc_id,
                Purpose::ENCRYPTION,
                SecurityLevel::MEDIUM,
                KeyType::ECDSA_SECP256K1,
                Some(ContractBounds::SingleContract {
                    id: dashpay_contract::ID,
                }),
                vec![0x02; 33],
                false,
            ),
        );
        me.public_keys_mut().insert(
            dashpay_dec_id,
            make_session_key_v1(
                dashpay_dec_id,
                Purpose::DECRYPTION,
                SecurityLevel::MEDIUM,
                KeyType::ECDSA_SECP256K1,
                Some(ContractBounds::SingleContract {
                    id: dashpay_contract::ID,
                }),
                vec![0x03; 33],
                false,
            ),
        );
    }

    // Approve Yappr (both group-bounded CRITICAL key + dashpay/profile HIGH key + encryption key).
    let l1 = devnet_with_callback(request(), "https://yappr.example/cb");
    let c1 = Fixture::choice();
    let g1 = f.quoted_grant(&l1, &c1);
    f.approve(&l1, &c1, &g1).unwrap();

    // Approve a second app.
    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &g2).unwrap();

    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 2);

    let req1 = hex::encode(l1.request.request_hash());
    let q = f.disconnect_quote(&me_b58, &req1).unwrap();
    assert_eq!(q.keys.len(), 2);
    assert!(q.fee > 0 && q.fee <= DISCONNECT_FEE_BOUND);
    assert_eq!(q.max_credits, q.fee);

    let grant = f.disconnect_grant(q.request, q.max_credits);
    let updates_before = f.net.updates.lock().unwrap().len();
    // Clear the refresh marker left by approval so Disconnect must perform its own refresh.
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req1, &grant).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before + 1);
    assert!(
        f.net.signed().ends_with(&[0]),
        "MASTER key 0 signed the update"
    );

    // Yappr left the list; SecondApp stays.
    let remaining = f.list_apps(&me_b58).unwrap();
    assert_eq!(remaining.apps.len(), 1);
    assert_eq!(remaining.apps[0].label, "SecondApp");

    // Only Yappr's 2 session keys are disabled; every non-app key, DashPay key,
    // Yappr's encryption key, and SecondApp's key stay enabled.
    let me = f.net.me();
    for (kid, k) in me.public_keys() {
        if q.keys.contains(kid) {
            assert!(k.is_disabled(), "Yappr key {kid} must be disabled");
        } else {
            assert!(!k.is_disabled(), "non-target key {kid} must stay enabled");
        }
    }
    // platform-wallet's contact-key selectors pick the lowest-id enabled key of
    // each purpose regardless of bounds: DashPay keys 4 & 5 must stay lowest.
    let lowest_enc = me
        .public_keys()
        .values()
        .filter(|k| k.purpose() == Purpose::ENCRYPTION && !k.is_disabled())
        .map(|k| k.id())
        .min();
    let lowest_dec = me
        .public_keys()
        .values()
        .filter(|k| k.purpose() == Purpose::DECRYPTION && !k.is_disabled())
        .map(|k| k.id())
        .min();
    assert_eq!(lowest_enc, Some(dashpay_enc_id));
    assert_eq!(lowest_dec, Some(dashpay_dec_id));
}

#[test]
fn disconnect_retry_after_unknown_result_does_not_double_spend_or_disable_other_keys() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));

    let l1 = devnet(request());
    let c1 = Fixture::choice();
    let g1 = f.quoted_grant(&l1, &c1);
    f.approve(&l1, &c1, &g1).unwrap();

    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &g2).unwrap();

    let req1 = hex::encode(l1.request.request_hash());
    let q = f.disconnect_quote(&me_b58, &req1).unwrap();

    // Case 1: update landed on Platform, but lagging confirmation read hid it (unknown result).
    let before_disable = f.net.me();
    *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
        *net.stale.lock().unwrap() = Some(before_disable);
    }));
    let updates_before = f.net.updates.lock().unwrap().len();
    let g_try1 = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req1, &g_try1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before + 1);

    // Lag clears: retry refreshes Platform first, sees keys already disabled,
    // sends NO update, leaves grant unused, spends 0 credits, touches no other keys.
    *f.net.stale.lock().unwrap() = None;
    let g_try2 = f.disconnect_grant(q.request, q.max_credits);
    let retry_done = f.disconnect(&me_b58, &req1, &g_try2).unwrap();
    assert!(retry_done.keys_disabled.is_empty());
    assert_eq!(
        f.net.updates.lock().unwrap().len(),
        updates_before + 1,
        "retry must not send a second update or double-spend credits"
    );
    assert!(f.disconnect_grant_unused(&g_try2));
    let remaining = f.list_apps(&me_b58).unwrap();
    assert_eq!(remaining.apps.len(), 1);
    assert_eq!(remaining.apps[0].label, "SecondApp");

    // Case 2: update was truly lost on try 1, then succeeds on try 2.
    let req2 = hex::encode(l2.request.request_hash());
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    f.net.lose_updates.store(true, Ordering::SeqCst);
    let g2_try1 = f.disconnect_grant(q2.request, q2.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req2, &g2_try1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let g2_try2 = f.disconnect_grant(q2.request, q2.max_credits);
    let done2 = f.disconnect(&me_b58, &req2, &g2_try2).unwrap();
    assert_eq!(done2.keys_disabled, q2.keys);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn list_quote_and_disconnect_refused_on_mainnet() {
    let f = Fixture::on(true);
    let me_b58 = b58(&id(ME));
    let req = "ab".repeat(20);

    assert_eq!(f.list_apps(&me_b58), Err(ConnectError::MainnetOff));
    assert_eq!(
        f.disconnect_quote(&me_b58, &req),
        Err(ConnectError::MainnetOff)
    );
    assert_eq!(
        f.disconnect(&me_b58, &req, "g"),
        Err(ConnectError::MainnetOff)
    );
    assert_eq!(f.net.reads(), 0, "mainnet refuses before any Platform read");
}

#[test]
fn disconnect_refuses_wrong_passphrase_and_wrong_grant() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();

    let req_hash = l.request.request_hash();
    let req_hex = hex::encode(req_hash);
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();

    // Encrypt vault through NetworkSession so the lease table tracks the new epoch.
    let enc_grant =
        f.s.vault
            .authorize(GrantPurpose::ChangeCredential, None, Credential::None)
            .unwrap()
            .id;
    f.engine
        .block_on(f.s.revoking_vault_op(
            super::super::flows::RevokeCause::PassphraseChange,
            move |v| v.encrypt(b"secret-pin", &enc_grant),
        ))
        .unwrap();
    f.engine
        .block_on(f.s.vault_op(|v| v.unlock(b"secret-pin", dw_vault::UnlockScope::Full)))
        .unwrap();
    let purpose = GrantPurpose::AppDisconnect {
        max_credits: q.max_credits,
        identity: id(ME).to_buffer(),
        request: req_hash,
    };
    assert!(matches!(
        f.s.vault.authorize(
            purpose,
            Some(&f.wallet.0),
            Credential::Passphrase(b"wrong-pin")
        ),
        Err(dw_vault::VaultError::WrongPassphrase { .. })
    ));
    assert!(matches!(
        f.s.vault
            .authorize(purpose, Some(&f.wallet.0), Credential::None),
        Err(dw_vault::VaultError::CredentialRequired)
    ));

    // Wrong grant kind (PlatformOp, AppConnect) or wrong request hash are refused.
    let platform_op =
        f.s.vault
            .authorize(
                GrantPurpose::PlatformOp {
                    max_duffs: 0,
                    max_credits: DISCONNECT_FEE_BOUND,
                },
                Some(&f.wallet.0),
                Credential::Passphrase(b"secret-pin"),
            )
            .unwrap()
            .id;
    let app_connect =
        f.s.vault
            .authorize(
                GrantPurpose::AppConnect {
                    max_credits: CONNECT_MAX_CREDITS,
                    request: req_hash,
                    plan: [1; 32],
                },
                Some(&f.wallet.0),
                Credential::Passphrase(b"secret-pin"),
            )
            .unwrap()
            .id;
    let other_req =
        f.s.vault
            .authorize(
                GrantPurpose::AppDisconnect {
                    max_credits: q.max_credits,
                    identity: id(ME).to_buffer(),
                    request: [0xee; 20],
                },
                Some(&f.wallet.0),
                Credential::Passphrase(b"secret-pin"),
            )
            .unwrap()
            .id;

    let updates_before = f.net.updates.lock().unwrap().len();
    for bad in [platform_op, app_connect, other_req] {
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &bad),
            Err(ConnectError::Platform(PlatformError::GrantInvalid))
        );
    }
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);

    // Correct passphrase succeeds.
    let ok_grant =
        f.s.vault
            .authorize(
                purpose,
                Some(&f.wallet.0),
                Credential::Passphrase(b"secret-pin"),
            )
            .unwrap()
            .id;
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &ok_grant).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
}

#[test]
fn older_grant_without_key_data_lists_but_refuses_disconnect() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let key_id = 10;
    let k = make_session_key_v1(
        key_id,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContractDocumentType {
            id: app(),
            document_type_name: "post".into(),
        }),
        vec![0xaa; 20],
        true,
    );
    f.net
        .identities
        .lock()
        .unwrap()
        .get_mut(&id(ME))
        .unwrap()
        .public_keys_mut()
        .insert(key_id, k);

    let req_bytes = [0x42u8; 20];
    let req_hex = hex::encode(req_bytes);
    f.put_record(ConnectRecord {
        request: req_hex.clone(),
        app_id: b58(&app()),
        label: "LegacyApp".into(),
        identity: me_b58.clone(),
        connection_id: None,
        session_keys: vec![key_id],
        encryption_keys: Vec::new(),
        selection: vec![0],
        funding_limit: 5_000_000_000,
        expires_at_ms: NOW + 30 * DAY_MS,
        state: RecordState::Published,
        created_ms: NOW - 10_000,
        returns_to: None,
        session_key_data: BTreeMap::new(),
        pending_disconnect: None,
        response_delivery: ResponseDelivery::Delivered,
        last_conflict: None,
    });

    // Still listed with whatever the older record holds.
    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 1);
    assert_eq!(listed.apps[0].label, "LegacyApp");
    assert_eq!(listed.apps[0].app_id, b58(&app()));
    assert_eq!(listed.apps[0].returns_to, None);
    assert_eq!(listed.apps[0].keys.len(), 1);
    assert_eq!(listed.apps[0].keys[0].key_id, key_id);

    // Disconnect refuses because key public data cannot be proved.
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );
    let grant = f.disconnect_grant(req_bytes, DISCONNECT_FEE_BOUND);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &grant),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );
    assert!(f.net.updates.lock().unwrap().is_empty());
    assert!(!f.net.me().public_keys()[&key_id].is_disabled());
}

fn assert_disconnect_refused_for_key(f: &Fixture, key: IdentityPublicKey, recorded_hex: String) {
    let me_b58 = b58(&id(ME));
    let key_id = key.id();
    f.net
        .identities
        .lock()
        .unwrap()
        .get_mut(&id(ME))
        .unwrap()
        .public_keys_mut()
        .insert(key_id, key);
    let req_bytes = [0x55u8; 20];
    let req_hex = hex::encode(req_bytes);
    f.put_record(ConnectRecord {
        request: req_hex.clone(),
        app_id: b58(&app()),
        label: "BadKeyApp".into(),
        identity: me_b58.clone(),
        connection_id: None,
        session_keys: vec![key_id],
        encryption_keys: Vec::new(),
        selection: vec![0],
        funding_limit: 5_000_000_000,
        expires_at_ms: NOW + 30 * DAY_MS,
        state: RecordState::Published,
        created_ms: NOW - 10_000,
        returns_to: None,
        session_key_data: [(key_id, recorded_hex)].into_iter().collect(),
        pending_disconnect: None,
        response_delivery: ResponseDelivery::Delivered,
        last_conflict: None,
    });
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );
    let grant = f.disconnect_grant(req_bytes, DISCONNECT_FEE_BOUND);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &grant),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );
    assert!(f.net.updates.lock().unwrap().is_empty());
    assert!(!f.net.me().public_keys()[&key_id].is_disabled());
}

#[test]
fn disconnect_refuses_mismatched_key_data() {
    let f = Fixture::new();
    let k = make_session_key_v1(
        10,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContract { id: app() }),
        vec![0xaa; 20],
        true,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode([0xbb; 20]));
}

#[test]
fn disconnect_refuses_master_key() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    // Synthetic MASTER-level key with otherwise session-like fields, so removing
    // the non-MASTER security-level guard alone fails this test.
    let synthetic_data = vec![0x66; 20];
    let synthetic_master = make_session_key_v1(
        17,
        Purpose::AUTHENTICATION,
        SecurityLevel::MASTER,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContract { id: app() }),
        synthetic_data.clone(),
        true,
    );
    assert_disconnect_refused_for_key(&f, synthetic_master, hex::encode(&synthetic_data));

    // Also test the identity's real MASTER key 0.
    let master = f.net.me().public_keys()[&0].clone();
    let req_bytes = [0x56u8; 20];
    let req_hex = hex::encode(req_bytes);
    f.put_record(ConnectRecord {
        request: req_hex.clone(),
        app_id: b58(&app()),
        label: "MasterAttempt".into(),
        identity: me_b58.clone(),
        connection_id: None,
        session_keys: vec![0],
        encryption_keys: Vec::new(),
        selection: vec![0],
        funding_limit: 5_000_000_000,
        expires_at_ms: NOW + 30 * DAY_MS,
        state: RecordState::Published,
        created_ms: NOW - 10_000,
        returns_to: None,
        session_key_data: [(0, hex::encode(master.data().as_slice()))]
            .into_iter()
            .collect(),
        pending_disconnect: None,
        response_delivery: ResponseDelivery::Delivered,
        last_conflict: None,
    });
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );
    let grant = f.disconnect_grant(req_bytes, DISCONNECT_FEE_BOUND);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &grant),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );
    assert!(f.net.updates.lock().unwrap().is_empty());
    assert!(!f.net.me().public_keys()[&0].is_disabled());
}

#[test]
fn disconnect_refuses_unbounded_critical_key() {
    let f = Fixture::new();
    let data = vec![0x11; 20];
    let k = make_session_key_v1(
        11,
        Purpose::AUTHENTICATION,
        SecurityLevel::CRITICAL,
        KeyType::ECDSA_HASH160,
        None,
        data.clone(),
        true,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode(&data));
}

#[test]
fn disconnect_refuses_non_authentication_key() {
    let f = Fixture::new();
    let data = vec![0x22; 20];
    let k = make_session_key_v1(
        12,
        Purpose::ENCRYPTION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContract { id: app() }),
        data.clone(),
        true,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode(&data));
}

#[test]
fn disconnect_refuses_whole_contract_dashpay_key() {
    let f = Fixture::new();
    let data = vec![0x33; 20];
    let k = make_session_key_v1(
        13,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContract {
            id: dashpay_contract::ID,
        }),
        data.clone(),
        true,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode(&data));
}

#[test]
fn disconnect_refuses_non_profile_dashpay_doctype_key() {
    let f = Fixture::new();
    let data = vec![0x34; 20];
    let k = make_session_key_v1(
        14,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContractDocumentType {
            id: dashpay_contract::ID,
            document_type_name: "contactRequest".into(),
        }),
        data.clone(),
        true,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode(&data));
}

#[test]
fn disconnect_refuses_unlimited_key() {
    let f = Fixture::new();
    let data = vec![0x44; 20];
    let k = make_session_key_v1(
        15,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContract { id: app() }),
        data.clone(),
        false,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode(&data));
}

#[test]
fn disconnect_refuses_non_hash160_key() {
    let f = Fixture::new();
    let data = vec![0x02; 33];
    let k = make_session_key_v1(
        16,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_SECP256K1,
        Some(ContractBounds::SingleContract { id: app() }),
        data.clone(),
        true,
    );
    assert_disconnect_refused_for_key(&f, k, hex::encode(&data));
}

#[test]
fn disconnect_refuses_key_outside_grant() {
    let data = vec![0x77; 20];
    let key = make_session_key_v1(
        20,
        Purpose::AUTHENTICATION,
        SecurityLevel::HIGH,
        KeyType::ECDSA_HASH160,
        Some(ContractBounds::SingleContract { id: app() }),
        data.clone(),
        true,
    );
    let record = ConnectRecord {
        request: "aa".repeat(20),
        app_id: b58(&app()),
        label: "App".into(),
        identity: b58(&id(ME)),
        connection_id: None,
        session_keys: vec![21],
        encryption_keys: Vec::new(),
        selection: vec![0],
        funding_limit: 5_000_000_000,
        expires_at_ms: NOW + 30 * DAY_MS,
        state: RecordState::Published,
        created_ms: NOW,
        returns_to: None,
        session_key_data: [(20, hex::encode(&data))].into_iter().collect(),
        pending_disconnect: None,
        response_delivery: ResponseDelivery::Delivered,
        last_conflict: None,
    };
    assert_eq!(
        check_disconnect_key(&record, &key),
        Err(ConnectError::Failed {
            failure: ConnectFailure::UnverifiedKey,
        })
    );

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let unknown_req = "cc".repeat(20);
    assert!(matches!(
        f.disconnect_quote(&me_b58, &unknown_req),
        Err(ConnectError::Platform(
            PlatformError::InvalidArgument { .. }
        ))
    ));
}

#[test]
fn disconnect_fee_bound_covers_known_versions_and_refuses_over_bound_without_clamping() {
    let typical_specs = [
        DisconnectKeyFeeSpec::Group {
            serialized_bytes: 256,
        },
        DisconnectKeyFeeSpec::Contract {
            serialized_bytes: 8_192,
            readonly: false,
        },
    ];
    for v in MIN_PROTOCOL_VERSION..=PlatformVersion::latest().protocol_version {
        let fee = disconnect_fee_at(v, &typical_specs).unwrap();
        assert!(
            fee > 0 && fee <= DISCONNECT_FEE_BOUND,
            "v{v}: {fee} vs {DISCONNECT_FEE_BOUND}"
        );
    }

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Live fee above bound at quote time: refuse rather than clamp.
    *f.net.disconnect_fee_override.lock().unwrap() = Some(DISCONNECT_FEE_BOUND + 1);
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Failed {
            failure: ConnectFailure::FeeTooHigh,
        })
    );

    // Quote normally, then live fee rises above bound before signing: refuse rather than clamp.
    *f.net.disconnect_fee_override.lock().unwrap() = None;
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g1 = f.disconnect_grant(q.request, q.max_credits);
    let updates_before = f.net.updates.lock().unwrap().len();
    *f.net.disconnect_fee_override.lock().unwrap() = Some(DISCONNECT_FEE_BOUND + 1);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::FeeTooHigh,
        })
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);

    // Live fee rises above grant cap (still within bound) before signing: refuse GrantExceeded.
    *f.net.disconnect_fee_override.lock().unwrap() = Some(q.fee + 1);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Platform(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: q.fee + 1,
            remaining: q.max_credits,
        }))
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);

    // Balance below live fee: refuse ConnectError::Credits.
    *f.net.disconnect_fee_override.lock().unwrap() = None;
    f.net
        .identities
        .lock()
        .unwrap()
        .get_mut(&id(ME))
        .unwrap()
        .set_balance(q.fee - 1);
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Credits {
            needed: q.fee,
            available: q.fee - 1,
        })
    );
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Credits {
            needed: q.fee,
            available: q.fee - 1,
        })
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);
}

#[test]
fn put_record_never_evicts_active_records_past_max_records() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));

    // Approve app #1 (the oldest active record).
    let l1 = devnet_with_callback(request(), "https://yappr.example/cb");
    let c1 = Fixture::choice();
    let g1 = f.quoted_grant(&l1, &c1);
    f.approve(&l1, &c1, &g1).unwrap();
    let req1 = hex::encode(l1.request.request_hash());

    // Record 63 more active (Published) grants across multiple identities so
    // the wallet holds MAX_RECORDS (64) active records.
    let other_id_b58 = b58(&id(2));
    for i in 0..(MAX_RECORDS - 1) {
        let rec = ConnectRecord {
            request: format!("{i:040x}"),
            app_id: b58(&app()),
            label: format!("App {i}"),
            identity: if i % 2 == 0 {
                me_b58.clone()
            } else {
                other_id_b58.clone()
            },
            connection_id: None,
            session_keys: vec![],
            encryption_keys: vec![],
            selection: vec![0],
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
            state: RecordState::Published,
            created_ms: NOW + i as u64,
            returns_to: None,
            session_key_data: BTreeMap::new(),
            pending_disconnect: None,
            response_delivery: ResponseDelivery::Delivered,
            last_conflict: None,
        };
        f.engine.block_on(put_record(&f.s, f.wallet, rec)).unwrap();
    }

    // Approve a 65th app — even if it stops at Submitted (NotConfirmed), its
    // Submitted write must NOT evict app #1.
    let l65 = devnet(second_app_request());
    let c65 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g65 = f.quoted_grant(&l65, &c65);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    assert_eq!(
        f.approve(&l65, &c65, &g65),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);

    // App #1 must still appear in Connected apps and remain disconnectable.
    let listed = f.list_apps(&me_b58).unwrap();
    assert!(
        listed.apps.iter().any(|a| a.request == req1),
        "oldest active app must not be evicted by the 65th approval"
    );
    let q = f.disconnect_quote(&me_b58, &req1).unwrap();
    assert_eq!(q.keys.len(), 2);
    let g_disc = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req1, &g_disc).unwrap();
    assert_eq!(done.keys_disabled, q.keys);

    // Inactive (Disconnected) history IS bounded to MAX_RECORDS without touching active records.
    for i in 0..(MAX_RECORDS + 5) {
        let rec = ConnectRecord {
            request: format!("{:040x}", 0x10_000 + i),
            app_id: b58(&app()),
            label: format!("Old {i}"),
            identity: me_b58.clone(),
            connection_id: None,
            session_keys: vec![],
            encryption_keys: vec![],
            selection: vec![0],
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
            state: RecordState::Disconnected,
            created_ms: NOW + 1_000 + i as u64,
            returns_to: None,
            session_key_data: BTreeMap::new(),
            pending_disconnect: None,
            response_delivery: ResponseDelivery::Delivered,
            last_conflict: None,
        };
        f.engine.block_on(put_record(&f.s, f.wallet, rec)).unwrap();
    }
    let all = f.engine.block_on(records(&f.s, f.wallet)).unwrap();
    let inactive = all
        .iter()
        .filter(|r| r.state == RecordState::Disconnected)
        .count();
    let active = all
        .iter()
        .filter(|r| r.state != RecordState::Disconnected)
        .count();
    assert_eq!(inactive, MAX_RECORDS, "inactive history stays bounded");
    assert_eq!(
        active, MAX_RECORDS,
        "all 64 remaining active records (63 Published + 1 Submitted) stay intact"
    );
}

#[test]
fn retry_send_aborts_if_disconnected_while_waiting_for_write_lock() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c, &g), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    let req_hash = l.request.request_hash();
    let req_hex = hex::encode(req_hash);
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g_disc = f.disconnect_grant(q.request, q.max_credits);

    // Case 1: retry_send peeks the identity and waits on locks.write while
    // disconnect holds locks.write; disconnect cancels the parked response and
    // disables the keys, so when retry_send enters the lock it aborts without
    // publishing or overwriting Disconnected with Published.
    let pw = f.wallet();
    let (res_disc, res_retry) = f.engine.block_on(async {
        let locks = identity_locks(f.wallet, id(ME));
        let hold = locks.write.lock().await;
        let retry_fut = {
            let (net, s, pw, w, link) = (
                Arc::clone(&f.net),
                Arc::clone(&f.s),
                Arc::clone(&pw),
                f.wallet,
                l.clone(),
            );
            tokio::spawn(async move { retry_send(&*net, &s, &pw, w, &link).await })
        };
        // Yield so retry_send peeks the identity and blocks on locks.write;
        // pending MUST remain in app_connect until the lock is held.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert!(
            !f.s.platform.app_connect.0.lock().unwrap().is_empty(),
            "retry_send must not take pending before acquiring the identity write lock"
        );
        // Simulate disconnect running under locks.write: cancel pending,
        // disable keys, mark Disconnected, then release the lock.
        let _ =
            f.s.platform
                .app_connect
                .take_for(f.wallet, id(ME), req_hash);
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            for &kid in &q.keys {
                me.public_keys_mut()
                    .get_mut(&kid)
                    .unwrap()
                    .set_disabled_at(NOW);
            }
        }
        let mut rec = records(&f.s, f.wallet)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        rec.state = RecordState::Disconnected;
        put_record(&f.s, f.wallet, rec).await.unwrap();
        drop(hold);
        let r = retry_fut.await.unwrap();
        let d = disconnect(&*f.net, &f.s, &pw, f.wallet, &me_b58, &req_hex, g_disc).await;
        (d, r)
    });
    assert!(res_disc.is_ok());
    assert_eq!(
        res_retry,
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
    assert!(
        f.net.published.lock().unwrap().is_empty(),
        "retry_send must not publish after disconnect"
    );
    let rec = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap();
    assert_eq!(
        rec.state,
        RecordState::Disconnected,
        "retry_send must not overwrite Disconnected with Published"
    );

    // Case 2: even if disconnect's put_record failed so the stored record stays
    // Confirmed while Platform disabled the keys, retry_send still aborts.
    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l2, &c2, &g2), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);
    let req2 = hex::encode(l2.request.request_hash());
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        for &kid in &q2.keys {
            me.public_keys_mut()
                .get_mut(&kid)
                .unwrap()
                .set_disabled_at(NOW);
        }
    }
    assert_eq!(
        f.retry(&l2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
    assert!(
        f.net.published.lock().unwrap().is_empty(),
        "stale Confirmed record with disabled Platform keys must not publish"
    );
    let rec2 = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req2)
        .unwrap();
    assert_eq!(rec2.state, RecordState::Confirmed);
}

#[test]
fn list_enumerates_tokens_even_when_approved_group_grows_past_200_members() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));

    // Approve Yappr while the group is within MAX_GROUP_MEMBERS.
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();

    // After approval, the group owner adds contracts past MAX_GROUP_MEMBERS AND a token.
    let token_contract = Identifier::from([0x77; 32]);
    let mut expected_contracts: BTreeSet<String> = [b58(&app())].into();
    {
        let mut groups = f.net.groups.lock().unwrap();
        let gf = groups.get_mut(&group()).unwrap();
        for i in 0..(plan::MAX_GROUP_MEMBERS + 5) {
            let mut buf = [0u8; 32];
            buf[0] = 0x55;
            buf[1..5].copy_from_slice(&(i as u32).to_be_bytes());
            let cid = Identifier::from(buf);
            gf.contracts.push(cid);
            expected_contracts.insert(b58(&cid));
        }
        gf.tokens.push((token_contract, 2));
    }

    // New approvals of this over-limit group are still refused as Unsupported.
    let mut fresh_req = request();
    fresh_req.app_eph_pub =
        PublicKey::from_secret_key(&SecretKey::from_secret_bytes([9u8; 32]).unwrap()).serialize();
    assert_eq!(f.sheet(&devnet(fresh_req)), Err(ConnectError::Unsupported));

    // Connected apps pages through all members and shows the exact full contract set AND token.
    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 1);
    let group_scope = listed.apps[0]
        .keys
        .iter()
        .find_map(|k| match &k.scope {
            ConnectScope::Group(grp) => Some(grp),
            _ => None,
        })
        .expect("group key scope");
    let actual_contracts: BTreeSet<String> = group_scope.contracts.iter().cloned().collect();
    assert_eq!(
        group_scope.contracts.len(),
        expected_contracts.len(),
        "must enumerate all {} contracts without duplicates or truncation",
        expected_contracts.len()
    );
    assert_eq!(
        actual_contracts, expected_contracts,
        "exact complete set of all 206 contracts must be enumerated"
    );
    assert_eq!(
        group_scope.tokens,
        vec![ConnectToken {
            contract: b58(&token_contract),
            position: 2,
        }],
        "group tokens must never be silently omitted when the group exceeds 200 members"
    );
}

#[test]
fn disconnect_refuses_lagging_pre_approval_snapshot_with_absent_keys() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    // Snapshot taken BEFORE approval: keys 6 & 7 are absent.
    let pre_approval = f.net.me();

    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Inventory sees the 2 newly added keys.
    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 1);
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert_eq!(q.keys.len(), 2);

    // Case 1: Refresh returns the lagging pre-approval snapshot (keys absent).
    // Disconnect MUST NOT treat absent keys as disabled, MUST NOT return Ok,
    // MUST NOT mark Disconnected, and MUST leave the grant unused.
    *f.net.stale.lock().unwrap() = Some(pre_approval.clone());
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    let g_disc = f.disconnect_grant(q.request, q.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_disc),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    assert!(
        f.disconnect_grant_unused(&g_disc),
        "lagging snapshot refusal must leave grant unused"
    );
    *f.net.stale.lock().unwrap() = None;
    assert_eq!(
        f.list_apps(&me_b58).unwrap().apps.len(),
        1,
        "app must NOT vanish after lagging pre-approval disconnect attempt"
    );

    // Case 2: Post-write confirm receives a lagging pre-approval snapshot
    // (absent keys) when the update was actually lost: must return NotConfirmed,
    // NEVER Ok.
    f.net.lose_updates.store(true, Ordering::SeqCst);
    *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
        *net.stale.lock().unwrap() = Some(pre_approval);
    }));
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_disc),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    *f.net.stale.lock().unwrap() = None;
    f.net.lose_updates.store(false, Ordering::SeqCst);

    // Once lag clears, retrying re-broadcasts the signed transition and disables both keys.
    let g_retry = f.disconnect_grant(q.request, q.max_credits);
    let done = f.disconnect(&me_b58, &req_hex, &g_retry).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn disconnect_grant_bound_to_identity_refuses_other_identity_in_same_wallet() {
    let f = Fixture::new();
    let id1 = id(ME);
    let id2 = f.add_second_identity(2);
    let id1_b58 = b58(&id1);
    let id2_b58 = b58(&id2);

    // Both identities in the same wallet approve the exact same request.
    let l = devnet(request());
    let c1 = Fixture::choice();
    let g1 = f.quoted_grant(&l, &c1);
    f.approve(&l, &c1, &g1).unwrap();

    let c2 = ConnectChoice {
        identity: id2_b58.clone(),
        ..Fixture::choice()
    };
    let g2 = f.quoted_grant(&l, &c2);
    f.approve(&l, &c2, &g2).unwrap();

    let req_hex = hex::encode(l.request.request_hash());
    let q1 = f.disconnect_quote(&id1_b58, &req_hex).unwrap();
    assert_eq!(q1.identity_id, id1.to_buffer());

    // Grant authorized for id1 MUST be refused when disconnecting id2,
    // leaving the grant unused and sending no update.
    let grant_for_id1 = f.disconnect_grant_for(q1.identity_id, q1.request, q1.max_credits);
    let updates_before = f.net.updates.lock().unwrap().len();
    assert_eq!(
        f.disconnect(&id2_b58, &req_hex, &grant_for_id1),
        Err(ConnectError::Platform(PlatformError::GrantInvalid))
    );
    assert!(
        f.disconnect_grant_unused(&grant_for_id1),
        "mismatched-identity refusal must leave grant unused"
    );
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);
    assert_eq!(
        f.list_apps(&id2_b58).unwrap().apps.len(),
        1,
        "id2's keys must remain untouched"
    );

    // The same grant succeeds on the identity it was bound to (id1).
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&id1_b58, &req_hex, &grant_for_id1).unwrap();
    assert_eq!(done.keys_disabled, q1.keys);
    assert!(f.list_apps(&id1_b58).unwrap().apps.is_empty());
    assert_eq!(f.list_apps(&id2_b58).unwrap().apps.len(), 1);
}

#[test]
fn disconnect_not_confirmed_cancels_parked_response_preventing_decryption_key_delivery() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c, &g), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    let req_hex = hex::encode(l.request.request_hash());
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g_disc = f.disconnect_grant(q.request, q.max_credits);
    // Update lands on Platform, but confirmation read lags -> Disconnect returns NotConfirmed.
    let before = f.net.me();
    *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
        *net.stale.lock().unwrap() = Some(before);
    }));
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_disc),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    // Even while stale hides the disabled keys, retry_send MUST be cancelled
    // and MUST NOT publish the sealed payload (which contains the decryption key).
    assert_eq!(
        f.retry(&l),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
    assert!(
        f.net.published.lock().unwrap().is_empty(),
        "NotConfirmed disconnect must cancel parked response so no decryption key leaks"
    );
}

#[test]
fn disconnect_retry_rebroadcasts_same_signed_transition_without_double_charging() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();

    // Case 1: Try 1 lands on Platform (revision 1, nonce 1), but a lagging node
    // STILL shows revision 0 with enabled keys when the user retries!
    let pre_disable = f.net.me();
    let pre_disable_for_retry = pre_disable.clone();
    *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
        *net.stale.lock().unwrap() = Some(pre_disable);
    }));
    let g1 = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), 1);

    // Retry while the lagging node STILL returns pre_disable (revision 0, enabled keys)
    // at an older block height (99 < nonce_height 100) while nonce is already 1 on Platform:
    // Disconnect MUST NOT retire pending, MUST NOT sign a second transition,
    // MUST NOT consume g2, and MUST return NotConfirmed!
    *f.net.stale.lock().unwrap() = Some(pre_disable_for_retry.clone());
    *f.net.identity_height.lock().unwrap() = 99;
    *f.net.nonce_height.lock().unwrap() = 100;
    let g2 = f.disconnect_grant(q.request, q.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        1,
        "retry while lagging must NOT sign a second transition or bump the nonce"
    );
    assert!(
        f.disconnect_grant_unused(&g2),
        "retry of an unresolved transition must leave the new grant unused"
    );

    // Also test lagging nonce (stale_nonce = 0) + lagging identity: re-broadcasts
    // the exact same signed transition bytes (same revision & nonce), and once
    // lag clears on confirm, finishes cleanly with zero second fee!
    *f.net.stale_nonce.lock().unwrap() = Some(0);
    *f.net.identity_height.lock().unwrap() = 100;
    *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
        *net.stale.lock().unwrap() = None;
        *net.stale_nonce.lock().unwrap() = None;
    }));
    // Clear stale right after re-broadcast runs (CheckTx returns duplicate-nonce
    // error, so clear stale via a fresh check right after verifying re-broadcast).
    *f.net.stale.lock().unwrap() = Some(pre_disable_for_retry);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    let broadcasts = f.net.broadcast_disables.lock().unwrap().clone();
    assert_eq!(broadcasts.len(), 2, "same transition re-broadcast once");
    assert_eq!(
        broadcasts[0], broadcasts[1],
        "re-broadcast MUST reuse the exact same signed transition bytes (same revision & nonce)"
    );
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert!(f.disconnect_grant_unused(&g2));

    // Lag clears: retry sees keys already disabled, finishes cleanly with 0 updates & grant unused.
    *f.net.stale.lock().unwrap() = None;
    *f.net.stale_nonce.lock().unwrap() = None;
    let done = f.disconnect(&me_b58, &req_hex, &g2).unwrap();
    assert!(done.keys_disabled.is_empty());
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert!(f.disconnect_grant_unused(&g2));

    // Case 2 (A1 + D1/D2): A non-revision-bumping transition (e.g. credit transfer)
    // spends pending.nonce while identity revision stays < pending.revision and keys stay enabled.
    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g_app2 = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &g_app2).unwrap();
    let req2 = hex::encode(l2.request.request_hash());
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    let g2_try1 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req2, &g2_try1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let signed_after_try1 = f.net.signed_disables.lock().unwrap().len();
    // Another transition (e.g. transfer) consumes pending.nonce WITHOUT bumping revision.
    {
        let mut nonces = f.net.nonces.lock().unwrap();
        let cur = *nonces.get(&id(ME)).unwrap_or(&0);
        nonces.insert(id(ME), cur + 1);
    }
    // 2a. Lagging identity read (id_height < nonce_height) MUST NOT retire or re-sign!
    *f.net.identity_height.lock().unwrap() = 100;
    *f.net.nonce_height.lock().unwrap() = 101;
    let g2_try2 = f.disconnect_grant(q2.request, q2.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req2, &g2_try2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        signed_after_try1,
        "lagging read must never re-sign"
    );
    assert!(f.disconnect_grant_unused(&g2_try2));

    // 2b. Consistent height (id_height >= nonce_height): retires dead pending_disconnect
    //     WITHOUT auto-resigning, leaves grant unused, and returns DisconnectConflict.
    *f.net.identity_height.lock().unwrap() = 101;
    assert_eq!(
        f.disconnect(&me_b58, &req2, &g2_try2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        signed_after_try1,
        "spent-nonce conflict must retire pending and NEVER auto-resign"
    );
    assert!(f.disconnect_grant_unused(&g2_try2));
    let rec2 = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req2)
        .unwrap();
    assert!(
        rec2.pending_disconnect.is_none(),
        "pending_disconnect must be retired once consistent height proves nonce spent"
    );

    // 2c. User starts a new, re-quoted Disconnect: discloses prior conflict (possible fee),
    //     signs once at next nonce and succeeds!
    let q2_fresh = f.disconnect_quote(&me_b58, &req2).unwrap();
    assert!(
        q2_fresh.prior_attempt_conflict,
        "re-quoted Disconnect after conflict must disclose the prior attempt"
    );
    assert_eq!(q2_fresh.prior_attempt_debit, None);
    let g2_fresh = f.disconnect_grant(q2_fresh.request, q2_fresh.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done2 = f.disconnect(&me_b58, &req2, &g2_fresh).unwrap();
    assert_eq!(done2.keys_disabled, q2_fresh.keys);
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        signed_after_try1 + 1
    );
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn disconnect_fee_scales_with_bound_contract_and_group_size_and_refuses_over_bound() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    let small_quote = f.disconnect_quote(&me_b58, &req_hex).unwrap();

    // Growing the bound readonly contract's serialized bytes increases the concrete fee quote.
    f.net
        .contracts
        .lock()
        .unwrap()
        .get_mut(&dashpay_contract::ID)
        .unwrap()
        .serialized_bytes = 65_536;
    let larger_contract_quote = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert!(
        larger_contract_quote.fee > small_quote.fee,
        "larger bound contract must produce a higher fee bound ({} vs {})",
        larger_contract_quote.fee,
        small_quote.fee
    );

    // Growing the bound group metadata above the protocol worst-case floor also increases the fee.
    f.net
        .group_info_bytes
        .lock()
        .unwrap()
        .insert(group(), 16_384);
    let larger_group_quote = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert!(
        larger_group_quote.fee > larger_contract_quote.fee,
        "larger bound group info must produce a higher fee bound ({} vs {})",
        larger_group_quote.fee,
        larger_contract_quote.fee
    );

    // A single key whose metered lookup pushes its fee past DISCONNECT_FEE_BOUND
    // is refused with FeeTooHigh rather than clamped or under-quoted.
    // (Set BOTH bound contract & group oversized so neither key fits under DISCONNECT_FEE_BOUND.)
    f.net
        .contracts
        .lock()
        .unwrap()
        .get_mut(&dashpay_contract::ID)
        .unwrap()
        .serialized_bytes = 3_000_000;
    f.net
        .group_info_bytes
        .lock()
        .unwrap()
        .insert(group(), 6_000_000);
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Failed {
            failure: ConnectFailure::FeeTooHigh,
        })
    );
}

#[test]
fn list_reports_proved_key_budgets_for_profile_only_approval() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    // Profile-only request asking for 0.05 DASH; plan::budgets caps the
    // on-chain profile key budget to PROFILE_SHARE (0.002 DASH).
    let mut r = second_app_request();
    r.auth_bindings = vec![AuthBinding {
        bounds: Bounds::DocumentType(dashpay_contract::ID.to_buffer(), "profile".into()),
        level: Level::High,
        budget: Some(5_000_000_000),
        expires_at_ms: Some(NOW + 30 * DAY_MS),
    }];
    let l = devnet(r);
    let c = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();

    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 1);
    assert_eq!(
        listed.apps[0].funding_limit,
        plan::PROFILE_SHARE,
        "Connected apps must report the proved on-chain key budget, not the higher requested limit"
    );
}

#[test]
fn list_uses_latest_expiry_and_omits_end_date_when_any_key_never_expires() {
    use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
    use dpp::identity::identity_public_key::v1::IdentityPublicKeyV1;

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();

    let listed = f.list_apps(&me_b58).unwrap();
    let key0 = listed.apps[0].keys[0].key_id;
    let key1 = listed.apps[0].keys[1].key_id;

    let replace_expiry = |key_id: KeyID, new_exp: Option<u64>| {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let old = me.public_keys().get(&key_id).unwrap().clone();
        let v0 = IdentityPublicKeyV0 {
            id: old.id(),
            purpose: old.purpose(),
            security_level: old.security_level(),
            contract_bounds: old.contract_bounds().cloned(),
            key_type: old.key_type(),
            read_only: old.read_only(),
            data: old.data().clone(),
            disabled_at: old.disabled_at(),
        };
        let budget = old.total_budget();
        me.public_keys_mut().insert(
            key_id,
            IdentityPublicKey::V1(IdentityPublicKeyV1::from_v0_with_limits(
                v0, budget, new_exp,
            )),
        );
    };

    // Case 1: Key 1 expired in the past, while Key 0 was extended into the future.
    // The app's access has NOT ended — expires_at_ms must be the latest expiry (Key 0).
    let extended = NOW + 180 * DAY_MS;
    replace_expiry(key0, Some(extended));
    replace_expiry(key1, Some(NOW - DAY_MS));
    let listed1 = f.list_apps(&me_b58).unwrap();
    assert_eq!(
        listed1.apps[0].expires_at_ms,
        Some(extended),
        "must use latest expiry so an extended key is never shown as Access ended"
    );

    // Case 2: One enabled key has no expiry on Platform -> expires_at_ms must be None.
    replace_expiry(key0, None);
    let listed2 = f.list_apps(&me_b58).unwrap();
    assert_eq!(
        listed2.apps[0].expires_at_ms, None,
        "must omit end date when any enabled key has no expiry"
    );
}

#[test]
fn disconnect_paid_revision_conflict_retires_pending_without_auto_resigning_or_double_charging() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();

    // Try 1 signs at target_revision = 1, nonce = 1, but network loses the initial broadcast
    // so pending_disconnect stays saved on disk.
    let initial_balance = f.net.me().balance();
    let g1 = f.disconnect_grant(q.request, q.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert_eq!(f.net.me().balance(), initial_balance);

    // Another transition advances identity revision to 1 while keys stay enabled,
    // AND the earlier signed transition arrives and suffers a PAID revision conflict
    // (debiting 500_000 credits without disabling keys or bumping revision further).
    const PAID_CONFLICT_FEE: u64 = 500_000;
    *f.net.paid_conflict_fee.lock().unwrap() = PAID_CONFLICT_FEE;
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }
    let pending_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();
    let pw = f.wallet();
    let conflict_res = f
        .engine
        .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &pending_bytes));
    assert!(conflict_res.is_err());
    assert_eq!(
        f.net.me().balance(),
        initial_balance - PAID_CONFLICT_FEE,
        "paid revision conflict debited fee once"
    );

    // Retry MUST retire pending_disconnect, leave grant unused, return DisconnectConflict,
    // sign NOTHING anew, and NOT debit a second fee!
    let g2 = f.disconnect_grant(q.request, q.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        1,
        "paid revision conflict must NOT auto-resign a second transition"
    );
    assert_eq!(
        f.net.me().balance(),
        initial_balance - PAID_CONFLICT_FEE,
        "balance must reflect only the single conflict debit — never charged twice"
    );
    assert!(
        f.disconnect_grant_unused(&g2),
        "conflict retirement must leave the retry grant unused"
    );
    let rec = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap();
    assert!(rec.pending_disconnect.is_none());

    // User explicitly starts a new, re-quoted Disconnect: discloses the known
    // prior conflict debit (PAID_CONFLICT_FEE), then signs once more and disables keys.
    let q_fresh = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert!(
        q_fresh.prior_attempt_conflict,
        "re-quoted Disconnect after paid conflict must disclose prior attempt"
    );
    assert_eq!(
        q_fresh.prior_attempt_debit,
        Some(PAID_CONFLICT_FEE),
        "re-quoted Disconnect must disclose the observed earlier charge"
    );
    let g_fresh = f.disconnect_grant(q_fresh.request, q_fresh.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &g_fresh).unwrap();
    assert_eq!(done.keys_disabled, q_fresh.keys);
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 2);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
    // Prior conflict metadata is cleared once Disconnect succeeds.
    let q_done = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert!(!q_done.prior_attempt_conflict);
    assert_eq!(q_done.prior_attempt_debit, None);
}

#[test]
fn disconnect_fee_matches_drive_ephemeral_cost_and_splits_oversized_batches() {
    use dpp::contract_group::{ContractGroupInfo, ContractGroupInfoV0, ContractGroupOwner};
    use dpp::serialization::PlatformSerializable;
    use dpp::version::PlatformVersion;

    let pv = PlatformVersion::get(14).unwrap();
    let fees = &pv.fee_version;
    let limits = &pv.system_limits;

    // Verify pinned Drive 9d61fee rates directly from PlatformVersion::get(14):
    assert_eq!(fees.storage.storage_load_credit_per_byte, 20);
    assert_eq!(fees.storage.storage_processing_credit_per_byte, 400);
    assert_eq!(fees.storage.storage_seek_cost, 2_000);
    assert_eq!(
        fees.hashing.blake3_base + fees.hashing.blake3_per_block,
        400
    );
    assert_eq!(fees.storage.storage_disk_usage_credit_per_byte, 27_000);

    // Build an actual worst-case ContractGroupInfo (64 4-byte UTF-8 chars name,
    // 256 4-byte UTF-8 chars description, max_contract_group_admins = 16).
    let admins: BTreeSet<Identifier> = (100u8..(100 + limits.max_contract_group_admins as u8))
        .map(id)
        .collect();
    let worst_group = ContractGroupInfo::V0(ContractGroupInfoV0 {
        owner: ContractGroupOwner::OwnerAndAdmins {
            owner: app_owner(),
            admins,
        },
        name: Some("🚀".repeat(limits.max_contract_group_name_length as usize)),
        description: Some("🚀".repeat(limits.max_contract_group_description_length as usize)),
    });
    let worst_group_bytes = worst_group.serialize_to_bytes().unwrap().len();

    // Independently compute Drive's exact ephemeral_cost (rs-drive/src/fees/op.rs:1895)
    // for a cold populated tree load of 20 KiB contract and worst-case ContractGroupInfo:
    let max_contract_bytes = limits.max_state_transition_size as usize;
    let blake3_hash_node = fees.hashing.blake3_base + fees.hashing.blake3_per_block;
    let one_contract_load_drive = 36 * fees.storage.storage_seek_cost
        + (36 * 192 + max_contract_bytes as u64) * fees.storage.storage_load_credit_per_byte
        + 36 * blake3_hash_node;
    let one_group_load_drive = 36 * fees.storage.storage_seek_cost
        + (36 * 192 + worst_group_bytes as u64) * fees.storage.storage_load_credit_per_byte
        + 36 * blake3_hash_node;

    // Verify per-byte contract load delta in disconnect_fee_at matches 2 * 20 * 1.25 = 50 credits/byte.
    let fee_20k = disconnect_fee_at(
        14,
        &[DisconnectKeyFeeSpec::Contract {
            serialized_bytes: max_contract_bytes,
            readonly: true,
        }],
    )
    .unwrap();
    let fee_30k = disconnect_fee_at(
        14,
        &[DisconnectKeyFeeSpec::Contract {
            serialized_bytes: max_contract_bytes + 10_000,
            readonly: true,
        }],
    )
    .unwrap();
    let expected_10k_delta = (2 * 10_000 * fees.storage.storage_load_credit_per_byte) * 5 / 4;
    assert_eq!(
        fee_30k - fee_20k,
        expected_10k_delta,
        "contract load delta must match Drive's 20 credits/byte * 2 cold loads * 1.25 margin"
    );

    // A small group quote (96 B) must still cover a worst-case ContractGroupInfo load.
    let small_group_fee = disconnect_fee_at(
        14,
        &[DisconnectKeyFeeSpec::Group {
            serialized_bytes: 96,
        }],
    )
    .unwrap();
    let worst_group_fee = disconnect_fee_at(
        14,
        &[DisconnectKeyFeeSpec::Group {
            serialized_bytes: worst_group_bytes,
        }],
    )
    .unwrap();
    assert_eq!(
        small_group_fee, worst_group_fee,
        "group fee bound must cover full protocol ContractGroupInfo metadata ({worst_group_bytes} B)"
    );
    assert!(small_group_fee > one_group_load_drive);
    assert!(fee_20k > 2 * one_contract_load_drive);

    // 6 worst-case 20 KiB mutable contract keys must stay comfortably below DISCONNECT_FEE_BOUND
    // so an app can never make Disconnect impossible.
    let six_worst_mutable = vec![
        DisconnectKeyFeeSpec::Contract {
            serialized_bytes: 1_024,
            readonly: false,
        };
        MAX_ADDED
    ];
    let (raw_proc, raw_disk) = disconnect_metered_operations(pv, &six_worst_mutable).unwrap();
    let six_fee = disconnect_fee_at(14, &six_worst_mutable).unwrap();
    assert!(
        six_fee >= raw_proc + raw_disk && six_fee <= 2 * (raw_proc + raw_disk),
        "margined fee {six_fee} must stay within [1.0x, 2.0x] of raw Drive cost {}",
        raw_proc + raw_disk
    );
    assert!(
        six_fee < DISCONNECT_FEE_BOUND,
        "6 worst-case 20 KiB mutable contract keys ({six_fee}) must fit under DISCONNECT_FEE_BOUND ({DISCONNECT_FEE_BOUND})"
    );

    // Future unaudited Drive method version must be refused rather than guessed.
    let mut future_pv = pv.clone();
    future_pv
        .drive
        .methods
        .identity
        .update
        .disable_identity_keys = 2;
    assert_eq!(
        disconnect_metered_operations(&future_pv, &six_worst_mutable),
        Err(PlatformError::Unavailable)
    );

    // Multi-key batch splitting: when 2 keys together exceed DISCONNECT_FEE_BOUND
    // but each key alone fits under DISCONNECT_FEE_BOUND, Disconnect splits into
    // a 1-key batch first, then a second Disconnect disables the remaining key!
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());
    // Set each key's bound object to ~60M credits so 1 key fits (< 100M) and 2 keys exceed (> 100M).
    f.net
        .group_info_bytes
        .lock()
        .unwrap()
        .insert(group(), 2_200_000);
    f.net
        .contracts
        .lock()
        .unwrap()
        .get_mut(&dashpay_contract::ID)
        .unwrap()
        .serialized_bytes = 1_100_000;

    let q_batch1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert_eq!(
        q_batch1.keys.len(),
        1,
        "multi-key disconnect over DISCONNECT_FEE_BOUND must split into a sub-bound batch"
    );
    assert!(q_batch1.fee <= DISCONNECT_FEE_BOUND);
    let g_batch1 = f.disconnect_grant(q_batch1.request, q_batch1.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done1 = f.disconnect(&me_b58, &req_hex, &g_batch1).unwrap();
    assert_eq!(done1.keys_disabled, q_batch1.keys);
    // Second key is still enabled -> app still listed with 1 remaining key.
    let after1 = f.list_apps(&me_b58).unwrap();
    assert_eq!(after1.apps.len(), 1);
    assert_eq!(after1.apps[0].keys.len(), 1);

    let q_batch2 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert_eq!(q_batch2.keys.len(), 1);
    assert_ne!(q_batch2.keys[0], q_batch1.keys[0]);
    let g_batch2 = f.disconnect_grant(q_batch2.request, q_batch2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done2 = f.disconnect(&me_b58, &req_hex, &g_batch2).unwrap();
    assert_eq!(done2.keys_disabled, q_batch2.keys);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn disconnect_quote_prices_group_bound_key_from_metadata_without_fetching_members() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    f.net.reads.lock().unwrap().clear();
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert_eq!(q.keys.len(), 2);
    let reads = f.net.reads.lock().unwrap().clone();
    assert!(
        reads.contains(&"group_info"),
        "disconnect quote must read group_info metadata"
    );
    assert!(
        !reads.contains(&"group") && !reads.contains(&"group_all"),
        "disconnect quote must NOT paginate or fetch group members: {reads:?}"
    );
}

#[test]
fn two_identities_same_request_parked_responses_stay_isolated_under_disconnect_and_retry() {
    let f = Fixture::new();
    let id1 = id(ME);
    let id2 = f.add_second_identity(2);
    let id1_b58 = b58(&id1);
    let id2_b58 = b58(&id2);

    // Unhinted request approved by BOTH identities while publish fails -> both park responses.
    let l = devnet(request());
    let c1 = Fixture::choice();
    let c2 = ConnectChoice {
        identity: id2_b58.clone(),
        ..Fixture::choice()
    };
    let g1 = f.quoted_grant(&l, &c1);
    let g2 = f.quoted_grant(&l, &c2);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c1, &g1), Err(ConnectError::SendFailed));
    assert_eq!(f.approve(&l, &c2, &g2), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    // Disconnect ONLY identity 2: cancels identity 2's parked response, leaving identity 1's intact!
    let req_hex = hex::encode(l.request.request_hash());
    let q2 = f.disconnect_quote(&id2_b58, &req_hex).unwrap();
    let gd2 = f.disconnect_grant_for(q2.identity_id, q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    f.disconnect(&id2_b58, &req_hex, &gd2).unwrap();

    // retry_send must publish ONLY identity 1's parked response (never identity 2's).
    let retried = f.retry(&l).unwrap();
    assert_eq!(retried.identity, id1_b58);
    let published = f.net.published.lock().unwrap().clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0, id1);

    // A second retry has nothing left to send (identity 2's response stayed cancelled).
    assert_eq!(
        f.retry(&l),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
    assert!(f.list_apps(&id2_b58).unwrap().apps.is_empty());
    assert_eq!(f.list_apps(&id1_b58).unwrap().apps.len(), 1);
}

#[test]
fn disconnect_rebroadcast_enforces_live_fee_limits_preserves_pending_and_reuses_bytes_on_larger_grant()
 {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());
    let q1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();

    // Try 1 signs & broadcasts, update lost -> pending_disconnect saved.
    let g1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), 1);

    // Fee rises above q1.max_credits: rebroadcast with an old-size grant MUST be refused
    // with GrantExceeded, MUST NOT rebroadcast, and MUST KEEP pending_disconnect.
    let higher_fee = q1.fee + 5_000_000;
    *f.net.disconnect_fee_override.lock().unwrap() = Some(higher_fee);
    let g_small = f.disconnect_grant(q1.request, q1.max_credits);
    assert!(matches!(
        f.disconnect(&me_b58, &req_hex, &g_small),
        Err(ConnectError::Platform(PlatformError::GrantExceeded { .. }))
    ));
    assert_eq!(
        f.net.broadcast_disables.lock().unwrap().len(),
        1,
        "refused rebroadcast must not send bytes"
    );
    assert!(
        f.engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap()
            .pending_disconnect
            .is_some(),
        "fee refusal must preserve pending_disconnect"
    );

    // Fee spikes above DISCONNECT_FEE_BOUND: refused with PendingFeeTooHigh while preserving pending_disconnect.
    *f.net.disconnect_fee_override.lock().unwrap() = Some(DISCONNECT_FEE_BOUND + 1);
    let g_big = f.disconnect_grant(q1.request, DISCONNECT_FEE_BOUND);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_big),
        Err(ConnectError::Failed {
            failure: ConnectFailure::PendingFeeTooHigh,
        })
    );
    assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), 1);
    assert!(
        f.engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap()
            .pending_disconnect
            .is_some()
    );

    // Fee settles at higher_fee (under bound): user re-quotes and grants higher_fee ->
    // rebroadcasts the EXACT SAME signed bytes without signing anew and disables keys!
    *f.net.disconnect_fee_override.lock().unwrap() = Some(higher_fee);
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let q2 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert_eq!(q2.fee, higher_fee);
    let g_higher = f.disconnect_grant(q2.request, q2.max_credits);
    let done = f.disconnect(&me_b58, &req_hex, &g_higher).unwrap();
    assert_eq!(done.keys_disabled, q1.keys);
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        1,
        "larger grant for pending disconnect must reuse signed bytes without signing anew"
    );
    let broadcasts = f.net.broadcast_disables.lock().unwrap().clone();
    assert_eq!(broadcasts.len(), 2);
    assert_eq!(broadcasts[0], broadcasts[1]);

    // Also verify confirmation-only recovery succeeds even while live fee exceeds DISCONNECT_FEE_BOUND!
    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    *f.net.disconnect_fee_override.lock().unwrap() = None;
    let ga2 = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &ga2).unwrap();
    let req2 = hex::encode(l2.request.request_hash());
    let q2_app = f.disconnect_quote(&me_b58, &req2).unwrap();
    let pre = f.net.me();
    *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
        *net.stale.lock().unwrap() = Some(pre);
    }));
    let gd_app2 = f.disconnect_grant(q2_app.request, q2_app.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req2, &gd_app2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    // Lag clears (keys ARE disabled on Platform), while live fee spikes above bound:
    // confirmation-only recovery MUST succeed without any fee refusal!
    *f.net.stale.lock().unwrap() = None;
    *f.net.disconnect_fee_override.lock().unwrap() = Some(DISCONNECT_FEE_BOUND + 50_000_000);
    let gd_recover = f.disconnect_grant(q2_app.request, 1);
    let recovered = f.disconnect(&me_b58, &req2, &gd_recover).unwrap();
    assert!(recovered.keys_disabled.is_empty());
}

#[test]
fn disconnect_reconciles_in_flight_response_that_commits_after_disable() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));

    // Case 1: Sign-in response times out locally (BroadcastUnknown) and then commits on
    // Platform right after Disconnect disables the keys -> Disconnect reconciles and flags
    // `response_may_have_landed = true`.
    let l = devnet(request());
    let req_hash = l.request.request_hash();
    let req_hex = hex::encode(req_hash);
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c, &g), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let gd = f.disconnect_grant(q.request, q.max_credits);
    *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
        // The earlier timed-out sign-in response document lands after the disable!
        net.responses.lock().unwrap().insert((req_hash, id(ME)));
    }));
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &gd).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
    assert!(
        done.response_may_have_landed,
        "Disconnect must warn when an earlier in-flight sign-in response landed or remained unknown"
    );

    // Case 2: Publish failed provably before broadcast (refused_unsent: SignerUnavailable)
    // -> local retry is cancelled and `response_may_have_landed` is false.
    let l2 = devnet(second_app_request());
    let req2 = hex::encode(l2.request.request_hash());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    *f.net.fail_publish_err.lock().unwrap() = Some(PlatformError::SignerUnavailable);
    assert_eq!(f.approve(&l2, &c2, &g2), Err(ConnectError::SendFailed));
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    let gd2 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done2 = f.disconnect(&me_b58, &req2, &gd2).unwrap();
    assert!(
        !done2.response_may_have_landed,
        "provably unsent publish must report response_may_have_landed = false"
    );
}

#[test]
fn retry_send_keeps_parked_response_on_lagging_snapshot_with_absent_keys() {
    let f = Fixture::new();
    let pre_approval = f.net.me();
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c, &g), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    // Lagging Platform read omits the newly added session keys:
    // retry_send MUST return SendFailed (re-parking the response), NEVER NothingToSend!
    *f.net.stale.lock().unwrap() = Some(pre_approval);
    assert_eq!(f.retry(&l), Err(ConnectError::SendFailed));
    assert!(f.net.published.lock().unwrap().is_empty());

    // Once the lag clears, the parked response is still intact and retry_send succeeds!
    *f.net.stale.lock().unwrap() = None;
    let done = f.retry(&l).unwrap();
    assert_eq!(done.identity, b58(&id(ME)));
    assert_eq!(f.net.published.lock().unwrap().len(), 1);
}

#[test]
fn disconnect_refuses_fresh_sign_when_identity_height_lags_nonce_height() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let gd = f.disconnect_grant(q.request, q.max_credits);

    // Identity snapshot lags behind nonce snapshot -> refuse fresh sign with Unavailable,
    // sign nothing, and leave the grant unused.
    *f.net.identity_height.lock().unwrap() = 99;
    *f.net.nonce_height.lock().unwrap() = 100;
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &gd),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    assert!(f.net.signed_disables.lock().unwrap().is_empty());
    assert!(f.disconnect_grant_unused(&gd));

    // Once identity height catches up to nonce height, fresh sign succeeds.
    *f.net.identity_height.lock().unwrap() = 100;
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &gd).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
}

#[test]
fn approve_refuses_while_disconnect_is_pending_and_preserves_pending_disconnect() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g1 = f.quoted_grant(&l, &c);
    // Initial approval adds keys but publish fails -> request still un-answered so user could re-approve.
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c, &g1), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);
    let req_hex = hex::encode(l.request.request_hash());

    // Start Disconnect whose broadcast goes unconfirmed -> pending_disconnect saved.
    let qd = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let gd = f.disconnect_grant(qd.request, qd.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &gd),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let pending_before = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap()
        .pending_disconnect
        .clone();
    assert!(pending_before.is_some());

    // Attempting to re-approve the same (identity, request) while disconnect is pending
    // MUST refuse with NotConfirmed, leave the approval grant unused, sign nothing,
    // and preserve pending_disconnect untouched!
    let g2 = f.quoted_grant(&l, &c);
    let signed_before = f.net.signed().len();
    assert_eq!(
        f.approve(&l, &c, &g2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert!(
        f.unused(&g2),
        "refused re-approval must leave the approval grant unused"
    );
    assert_eq!(f.net.signed().len(), signed_before);
    let pending_after = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap()
        .pending_disconnect;
    assert_eq!(
        pending_after, pending_before,
        "re-approval must NEVER overwrite or drop pending_disconnect"
    );
}

#[test]
fn disconnect_disables_present_verified_keys_when_predicted_reapproval_key_never_landed() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let pre_approval = f.net.me();
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Simulate an unlanded re-approval that recorded a predicted key id (99) that
    // never landed on Platform, while the original approved keys ARE present & enabled.
    let mut rec = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap();
    let present_keys = rec.session_keys.clone();
    assert_eq!(present_keys.len(), 2);
    rec.session_keys.push(99);
    rec.session_key_data.insert(99, "aa".repeat(20));
    f.put_record(rec);

    // Lagging pre-approval snapshot where ALL keys are absent still returns Unavailable.
    *f.net.stale.lock().unwrap() = Some(pre_approval);
    assert_eq!(
        f.disconnect_quote(&me_b58, &req_hex),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    *f.net.stale.lock().unwrap() = None;

    // On the real snapshot (original 2 keys present, predicted key 99 absent):
    // Disconnect quotes & disables the 2 present verified keys, and does NOT mark
    // RecordState::Disconnected while predicted key 99 remains absent.
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert_eq!(q.keys, present_keys);
    let gd = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &gd).unwrap();
    assert_eq!(done.keys_disabled, present_keys);
    let rec_after = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap();
    assert_ne!(
        rec_after.state,
        RecordState::Disconnected,
        "must not mark Disconnected until every recorded key id is present and disabled"
    );
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn e1_transfer_timeout_blocks_disconnect_from_stealing_nonce_and_never_falsely_reports_landed() {
    use crate::platform::errors::CreditsError;

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // 1. Credit transfer times out without reaching Platform (nonce tip stays 0,
    //    session.withdrawal_nonces holds unresolved nonce 1).
    let amount = 100_000_000;
    let tq = f.transfer_quote(&me_b58, amount).unwrap();
    let w_grant1 = f.withdraw_grant(amount + tq.fee_credits);
    f.net.transfer_timeout.store(true, Ordering::SeqCst);
    let t1_err = f
        .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant1)
        .unwrap_err();
    let t1_hash = match t1_err {
        CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
        other => panic!("expected BroadcastUnknown, got {other:?}"),
    };
    f.net.transfer_timeout.store(false, Ordering::SeqCst);
    assert_eq!(*f.net.nonces.lock().unwrap().get(&id(ME)).unwrap_or(&0), 0);

    // 2. Disconnect on the SAME identity MUST refuse (PendingWrite), MUST NOT
    //    sign or consume nonce 1, and MUST leave the disconnect grant unused.
    let dq = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g_disc = f.disconnect_grant(dq.request, dq.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_disc),
        Err(ConnectError::Failed {
            failure: ConnectFailure::PendingWrite,
        })
    );
    assert!(f.net.signed_disables.lock().unwrap().is_empty());
    assert!(f.disconnect_grant_unused(&g_disc));
    assert_eq!(*f.net.nonces.lock().unwrap().get(&id(ME)).unwrap_or(&0), 0);

    // 3. Next transfer MUST see the first transfer as MovePending — NEVER
    //    falsely reporting WithdrawalLanded for a transfer that never landed!
    let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
        Err(CreditsError::MovePending {
            id: t1_hash.clone(),
        })
    );

    // 4. Once the timed-out transfer genuinely lands on Platform (nonce 1 used),
    //    Disconnect proceeds cleanly at nonce 2 without clobbering withdrawal_nonces,
    //    and the next transfer accurately reports WithdrawalLanded ONCE.
    f.net.nonces.lock().unwrap().insert(id(ME), 1);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &g_disc).unwrap();
    assert_eq!(done.keys_disabled, dq.keys);
    assert_eq!(f.net.signed_disables.lock().unwrap()[0].1, 2);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
        Err(CreditsError::WithdrawalLanded { artifact: t1_hash })
    );
    let out = f
        .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2)
        .unwrap();
    assert!(!out.transition.is_empty());
}

#[test]
fn e1_disconnect_timeout_blocks_transfer_from_stranding_pending_disconnect() {
    use crate::platform::errors::CreditsError;

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // 1. Disconnect times out (`NotConfirmed`), persisting pending_disconnect at nonce 1.
    let dq = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g_disc1 = f.disconnect_grant(dq.request, dq.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_disc1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let pending_hash = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap()
        .pending_disconnect
        .expect("pending_disconnect persisted")
        .transition_hash;

    // 2. Credit transfer on the SAME identity MUST refuse (`MovePending` naming
    //    the pending disconnect hash) and MUST NOT steal nonce 1!
    let amount = 100_000_000;
    let tq = f.transfer_quote(&me_b58, amount).unwrap();
    let w_grant = f.withdraw_grant(amount + tq.fee_credits);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
        Err(CreditsError::MovePending { id: pending_hash })
    );
    assert!(
        f.net.transfers_sent.lock().unwrap().is_empty(),
        "transfer must not sign or broadcast while Disconnect holds nonce 1"
    );

    // 3. Pending Disconnect is NOT stranded or wedged: retry rebroadcasts the
    //    exact same signed bytes at nonce 1 and confirms cleanly (no second sign).
    let g_disc2 = f.disconnect_grant(dq.request, dq.max_credits);
    let done = f.disconnect(&me_b58, &req_hex, &g_disc2).unwrap();
    assert_eq!(done.keys_disabled, dq.keys);
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert!(f.disconnect_grant_unused(&g_disc2));

    // 4. Now that Disconnect resolved, transfer succeeds cleanly at nonce 2.
    let out = f
        .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
        .unwrap();
    assert!(!out.transition.is_empty());
    assert_eq!(*f.net.nonces.lock().unwrap().get(&id(ME)).unwrap_or(&0), 2);
}

#[test]
fn refused_disconnect_holds_parked_response_without_cancelling_until_submit_or_explicit_retry() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c, &g), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    let req_hash = l.request.request_hash();
    let req_hex = hex::encode(req_hash);
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id(ME), req_hash),
        Some(false),
        "newly parked response is not yet held"
    );

    // Disconnect refused before submission (grant too small) MUST keep the parked
    // response alive AND mark it held so no automatic retry can deliver a key!
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let tiny_grant = f.disconnect_grant(q.request, q.fee.saturating_sub(1));
    assert!(matches!(
        f.disconnect(&me_b58, &req_hex, &tiny_grant),
        Err(ConnectError::Platform(PlatformError::GrantExceeded { .. }))
    ));
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id(ME), req_hash),
        Some(true),
        "refused Disconnect must HOLD the parked response, not cancel it"
    );
    assert!(
        f.s.platform
            .app_connect
            .take_auto_for(f.wallet, id(ME), req_hash)
            .is_none(),
        "held response must never fire automatically"
    );

    // Explicit user-initiated sign-in retry succeeds if the user chooses to retry sign-in.
    let retried = f.retry(&l).unwrap();
    assert_eq!(retried.identity, me_b58);
    assert_eq!(f.net.published.lock().unwrap().len(), 1);

    // Second app: refused Disconnect holds the parked response; completing Disconnect
    // then cancels it permanently so neither auto-retry nor explicit retry can send it.
    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l2, &c2, &g2), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);
    let req2_hash = l2.request.request_hash();
    let req2_hex = hex::encode(req2_hash);

    let q2 = f.disconnect_quote(&me_b58, &req2_hex).unwrap();
    let tiny2 = f.disconnect_grant(q2.request, q2.fee.saturating_sub(1));
    assert!(matches!(
        f.disconnect(&me_b58, &req2_hex, &tiny2),
        Err(ConnectError::Platform(PlatformError::GrantExceeded { .. }))
    ));
    assert_eq!(
        f.s.platform
            .app_connect
            .is_held(f.wallet, id(ME), req2_hash),
        Some(true)
    );

    // Completing Disconnect cancels the held parked response permanently.
    let good2 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    f.disconnect(&me_b58, &req2_hex, &good2).unwrap();
    assert_eq!(
        f.s.platform
            .app_connect
            .is_held(f.wallet, id(ME), req2_hash),
        None
    );
    assert_eq!(
        f.retry(&l2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
}

#[test]
fn group_member_page_failure_does_not_block_list_or_disconnect() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    // Approve App 1 (group-bound) and App 2 (contract-bound, unrelated to group).
    let l1 = devnet(request());
    let c1 = Fixture::choice();
    let g1 = f.quoted_grant(&l1, &c1);
    f.approve(&l1, &c1, &g1).unwrap();
    let req1 = hex::encode(l1.request.request_hash());

    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &g2).unwrap();
    let req2 = hex::encode(l2.request.request_hash());

    // Simulate a group member pagination failure in `group_all`:
    // list_apps MUST still return both apps, and Disconnect for BOTH apps MUST succeed!
    f.net.fail_group_all.store(true, Ordering::SeqCst);
    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(
        listed.apps.len(),
        2,
        "group member page failure must not hide connected apps or block Disconnect"
    );

    // If group_info fails ONLY for App 1's group, App 1's quote refuses while
    // unrelated App 2 still quotes and disconnects cleanly.
    f.net.fail_group_info.lock().unwrap().insert(group());
    assert_eq!(
        f.disconnect_quote(&me_b58, &req1),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    let gd2 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    f.disconnect(&me_b58, &req2, &gd2).unwrap();

    // Once group_info succeeds (even while group_all member paging STILL fails!),
    // App 1 quotes and disconnects cleanly.
    f.net.fail_group_info.lock().unwrap().clear();
    let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
    let gd1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    f.disconnect(&me_b58, &req1, &gd1).unwrap();
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn disconnect_pending_step3_stale_nonce_read_keeps_pending_and_returns_not_confirmed() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Start at tip = 29 so the first Disconnect signs pending.nonce = 30 (> 24).
    f.net.nonces.lock().unwrap().insert(id(ME), 29);
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g1 = f.disconnect_grant(q.request, q.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);

    // Site 1 (pending step 3): stale nonce reads with tip <= pending.nonce - 25 (tip = 5)
    // and state 0 (tip = 0, pending.nonce >= 24) both make `takes(nonce_state, 30, id)`
    // return false via `NonceTooFarInFuture`. Even with `id_height >= nonce_height`,
    // Disconnect MUST keep `pending_disconnect`, leave `last_conflict` unset, never
    // re-sign, add no broadcast, and leave the retry grant unused.
    for stale_state in [5u64, 0u64] {
        let broadcasts_before = f.net.broadcast_disables.lock().unwrap().len();
        *f.net.stale_nonce.lock().unwrap() = Some(stale_state);
        *f.net.identity_height.lock().unwrap() = 105;
        *f.net.nonce_height.lock().unwrap() = 100;
        let g2 = f.disconnect_grant(q.request, q.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g2),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            }),
            "stale nonce_state={stale_state} at step 3 must return NotConfirmed"
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert_eq!(
            rec.pending_disconnect.as_ref().map(|p| p.nonce),
            Some(30),
            "stale nonce_state={stale_state} must keep pending_disconnect"
        );
        assert!(
            rec.last_conflict.is_none(),
            "stale nonce_state={stale_state} must not record last_conflict"
        );
        assert_eq!(
            f.net.signed_disables.lock().unwrap().len(),
            1,
            "stale nonce_state={stale_state} must not sign a new transition"
        );
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            broadcasts_before,
            "stale nonce_state={stale_state} must not rebroadcast or emit a new broadcast"
        );
        assert!(
            f.disconnect_grant_unused(&g2),
            "stale nonce_state={stale_state} must leave retry grant unused"
        );
    }

    // Once a fresh read sees the real tip (29), retrying resolves the pending
    // transition normally without signing a second transition.
    *f.net.stale_nonce.lock().unwrap() = None;
    let g_resolve = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &g_resolve).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn disconnect_post_rebroadcast_stale_nonce_read_keeps_pending_and_returns_not_confirmed() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Start at tip = 29 so the first Disconnect signs pending.nonce = 30 (> 24).
    f.net.nonces.lock().unwrap().insert(id(ME), 29);
    let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g1 = f.disconnect_grant(q.request, q.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), 1);

    // Site 2 (post-rebroadcast re-read): step 3 reads admissible tip = 29 so Disconnect
    // rebroadcasts, then the post-rebroadcast re-read returns stale tip = 5 (<= 30 - 25)
    // or state 0 with `id_height >= nonce_height` and `revision < pending.revision`.
    for (idx, stale_state) in [5u64, 0u64].into_iter().enumerate() {
        *f.net.stale_nonce.lock().unwrap() = None;
        *f.net.identity_height.lock().unwrap() = 100;
        *f.net.nonce_height.lock().unwrap() = 100;
        *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
            *net.stale_nonce.lock().unwrap() = Some(stale_state);
            *net.identity_height.lock().unwrap() = 105;
            *net.nonce_height.lock().unwrap() = 100;
        }));
        let g2 = f.disconnect_grant(q.request, q.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g2),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            }),
            "stale post-rebroadcast nonce_state={stale_state} must return NotConfirmed"
        );
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            idx + 2,
            "retry must have reached rebroadcast once before the stale post-rebroadcast read"
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert_eq!(
            rec.pending_disconnect.as_ref().map(|p| p.nonce),
            Some(30),
            "stale post-rebroadcast nonce_state={stale_state} must keep pending_disconnect"
        );
        assert!(
            rec.last_conflict.is_none(),
            "stale post-rebroadcast nonce_state={stale_state} must not record last_conflict"
        );
        assert_eq!(
            f.net.signed_disables.lock().unwrap().len(),
            1,
            "stale post-rebroadcast nonce_state={stale_state} must not sign a new transition"
        );
        assert!(
            f.disconnect_grant_unused(&g2),
            "stale post-rebroadcast nonce_state={stale_state} must leave retry grant unused"
        );
    }

    // Once a fresh read sees the real tip (29) and delivery lands, the kept pending
    // transition resolves normally with no second signature.
    f.net.lose_updates.store(false, Ordering::SeqCst);
    *f.net.stale_nonce.lock().unwrap() = None;
    let g_resolve = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &g_resolve).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

#[test]
fn disconnect_post_fresh_broadcast_stale_nonce_read_keeps_pending_and_returns_not_confirmed() {
    // Site 3 (post-fresh-broadcast re-read): fresh sign picks next_nonce = 30 (>= 24)
    // from tip = 29; right after broadcast (which is unconfirmed), the re-read returns
    // a stale nonce read with tip = 5 (<= 30 - 25) or state 0 at `id_height >= nonce_height`
    // and `revision < target_revision`.
    for stale_state in [5u64, 0u64] {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l = devnet(request());
        let c = Fixture::choice();
        let g = f.quoted_grant(&l, &c);
        f.approve(&l, &c, &g).unwrap();
        let req_hex = hex::encode(l.request.request_hash());

        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
        let g1 = f.disconnect_grant(q.request, q.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        *f.net.after_update.lock().unwrap() = Some(Box::new(move |net| {
            *net.stale_nonce.lock().unwrap() = Some(stale_state);
            *net.identity_height.lock().unwrap() = 105;
            *net.nonce_height.lock().unwrap() = 100;
        }));
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            }),
            "stale post-fresh-broadcast nonce_state={stale_state} must return NotConfirmed"
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert_eq!(
            rec.pending_disconnect.as_ref().map(|p| p.nonce),
            Some(30),
            "stale post-fresh-broadcast nonce_state={stale_state} must keep pending_disconnect"
        );
        assert!(
            rec.last_conflict.is_none(),
            "stale post-fresh-broadcast nonce_state={stale_state} must not record last_conflict"
        );
        assert_eq!(
            f.net.signed_disables.lock().unwrap().len(),
            1,
            "fresh broadcast must sign exactly once and never retire/resign on stale read"
        );
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            1,
            "fresh broadcast must emit only the single initial broadcast"
        );
        // Follow-up retry under the same stale read also emits no broadcast, leaves its
        // grant unused, and keeps pending.
        let g2 = f.disconnect_grant(q.request, q.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g2),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        assert!(f.disconnect_grant_unused(&g2));
        assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
        assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), 1);

        // Then a fresh read of the real tip (29) resolves the pending transition normally.
        f.net.lose_updates.store(false, Ordering::SeqCst);
        *f.net.stale_nonce.lock().unwrap() = None;
        f.net.refreshed.store(false, Ordering::SeqCst);
        let done = f.disconnect(&me_b58, &req_hex, &g2).unwrap();
        assert_eq!(done.keys_disabled, q.keys);
        assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
        assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
    }
}

#[test]
fn disconnect_keys_disabled_by_foreign_transition_keeps_nonce_floor_at_proved_tip() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));

    // Approve two apps on the same identity.
    let l1 = devnet(request());
    let c1 = Fixture::choice();
    let g1_app = f.quoted_grant(&l1, &c1);
    f.approve(&l1, &c1, &g1_app).unwrap();
    let req1 = hex::encode(l1.request.request_hash());

    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58.clone(),
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2_app = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &g2_app).unwrap();
    let req2 = hex::encode(l2.request.request_hash());

    // Start Disconnect for App 1 at tip = 5 -> persists pending_disconnect with nonce = 6,
    // but the broadcast is lost so Platform's proved tip stays at 5.
    f.net.nonces.lock().unwrap().insert(id(ME), 5);
    let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
    let g1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req1, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);

    // Another transition (e.g. from another device at a lower/different nonce) disables
    // App 1's keys on Platform while our pending.nonce (6) was never used (proved tip stays 5).
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        for key_id in &q1.keys {
            if let Some(k) = me.public_keys_mut().get_mut(key_id) {
                k.set_disabled_at(NOW);
            }
        }
    }

    // Retry Disconnect for App 1: sees all keys already disabled ("nothing to disable"
    // branch) and finishes cleanly. The nonce floor MUST advance only to min(6, 5) = 5
    // (floor <= proved_tip + 1), NOT above the proved tip!
    let g1_retry = f.disconnect_grant(q1.request, q1.max_credits);
    let done1 = f.disconnect(&me_b58, &req1, &g1_retry).unwrap();
    assert!(done1.keys_disabled.is_empty());
    assert!(f.disconnect_grant_unused(&g1_retry));
    assert_eq!(
        f.s.withdrawal_nonces.ownership(id(ME), 5),
        crate::platform::withdrawal::NonceOwner::Free,
        "floor must stay <= proved tip + 1 so ownership is Free, never Behind"
    );

    // A subsequent Disconnect (App 2) must NOT fail with Behind / Unavailable.
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    let g2 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done2 = f.disconnect(&me_b58, &req2, &g2).unwrap();
    assert_eq!(done2.keys_disabled, q2.keys);
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());

    // And a subsequent credit transfer must also succeed cleanly (never Unavailable).
    let amount = 100_000_000;
    let tq = f.transfer_quote(&me_b58, amount).unwrap();
    let w_grant = f.withdraw_grant(amount + tq.fee_credits);
    let out = f
        .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
        .unwrap();
    assert!(!out.transition.is_empty());
}
