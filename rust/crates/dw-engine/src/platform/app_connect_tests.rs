//! App Connect against a scripted Platform: what is read, what is refused
//! before anything is signed, the keys an approval adds and the response it
//! publishes.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use dpp::identity::accessors::{IdentityGettersV0, IdentitySettersV0};
use dpp::identity::identity_public_key::accessors::v0::{
    IdentityPublicKeyGettersV0, IdentityPublicKeySettersV0,
};
use dpp::identity::identity_public_key::accessors::v1::IdentityPublicKeyGettersV1;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::prelude::Identifier;
use dpp::secp256k1::{PublicKey, SecretKey};
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
    /// The nonce each IdentityUpdate was signed at.
    update_nonces: Mutex<Vec<u64>>,
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
    /// Simulate an SDK cache refresh failure while direct identity fetches
    /// (`refresh` / `identity`) still succeed (`r4712172979`).
    fail_cache_refresh: AtomicBool,
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
            update_nonces: Mutex::default(),
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
            fail_cache_refresh: AtomicBool::new(false),
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

fn merge_fake_nonce(cur: u64, nonce: u64) -> u64 {
    let cur_tip = cur & IDENTITY_NONCE_VALUE_FILTER;
    if nonce > cur_tip {
        let gap = nonce - cur_tip;
        let shifted = (cur & !IDENTITY_NONCE_VALUE_FILTER)
            .checked_shl(gap as u32)
            .unwrap_or(0);
        let mut missing = shifted;
        for offset in 1..gap {
            missing |= 1u64 << (offset - 1 + 40);
        }
        nonce | missing
    } else if nonce < cur_tip {
        let bit = 1u64 << (cur_tip - nonce - 1 + 40);
        cur & !bit
    } else {
        cur
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

    async fn refresh_cache(
        &self,
        wallet: &PlatformWallet,
        id: Identifier,
    ) -> Result<Identity, PlatformError> {
        if self.fail_cache_refresh.load(Ordering::SeqCst) {
            return Err(PlatformError::Unavailable);
        }
        self.refresh(wallet, id).await.map(|(identity, _)| identity)
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
        identity: &Identity,
        master_key_id: KeyID,
        add: Vec<IdentityPublicKey>,
        disable: Vec<KeyID>,
        nonce: u64,
        signer: &ConnectUpdateSigner,
    ) -> Result<(), PlatformError> {
        let id = identity.id();
        let master = identity.public_keys()[&master_key_id].clone();
        assert_eq!(master.security_level(), SecurityLevel::MASTER);
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
        self.update_nonces.lock().unwrap().push(nonce);
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
                    nonces.insert(id, merge_fake_nonce(cur_nonce, nonce));
                }
                return Err(PlatformError::Internal {
                    detail: format!(
                        "wrong revision: expected {}, got {target_revision}",
                        me.revision() + 1
                    ),
                });
            }
            me.set_revision(target_revision);
            nonces.insert(id, merge_fake_nonce(cur_nonce, nonce));
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
        nonces.insert(owner, merge_fake_nonce(cur, nonce));
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

    fn restart(&mut self) {
        self.engine
            .block_on(self.engine.close_network(DashNetwork::Regtest))
            .unwrap();
        self.s = self
            .engine
            .block_on(self.engine.open_network(
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

/// Mainnet runs App Connect like any other network: the sheet, the quote
/// and the approval go through, and the app gets bounded AUTHENTICATION
/// session keys only.
#[test]
fn mainnet_sheet_and_approval_run_with_bounded_authentication_keys() {
    let f = Fixture::on(true);
    let main = link(request(), NetworkSel::Mainnet);
    let sheet = f.sheet(&main).unwrap();
    assert_eq!(sheet.terms.funding_limit, 51_000_000_000);
    assert_eq!(sheet.max_funding_limit, MAX_FUNDING);
    let choice = Fixture::choice();
    let grant = f.quoted_grant(&main, &choice);
    let done = f.approve(&main, &choice, &grant).unwrap();
    assert_eq!(done.keys_added, vec![6, 7, 8]);
    assert!(!f.unused(&grant));
    let updates = f.net.updates.lock().unwrap().clone();
    assert_eq!(updates.len(), 1);
    let (add, disable) = &updates[0];
    assert!(disable.is_empty());
    for k in &add[..2] {
        assert_eq!(k.purpose(), Purpose::AUTHENTICATION);
        assert_ne!(k.security_level(), SecurityLevel::MASTER);
        assert!(k.total_budget().is_some());
        assert!(k.expires_at().is_some());
    }
    assert_eq!(add[2].purpose(), Purpose::ENCRYPTION);
}

#[test]
fn a_mainnet_wallet_refuses_a_test_network_request_before_anything_is_read() {
    let f = Fixture::on(true);
    for network in [NetworkSel::Testnet, NetworkSel::Devnet] {
        let wrong = link(request(), network);
        assert_eq!(f.sheet(&wrong), Err(ConnectError::Network));
        assert_eq!(
            f.quote(&wrong, &Fixture::choice()),
            Err(ConnectError::Network)
        );
    }
    assert_eq!(f.net.reads(), 0);
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

/// The quote names the group facts it read: those the sheet showed when
/// nothing changed between the two reads, others when the group did.
#[test]
fn the_quote_names_the_group_facts_it_read() {
    let f = Fixture::new();
    let l = devnet(request());
    let sheet = f.sheet(&l).unwrap();
    let quote = f.quote(&l, &Fixture::choice()).unwrap();
    assert_eq!(quote.groups, sheet.groups);
    f.net
        .groups
        .lock()
        .unwrap()
        .get_mut(&group())
        .unwrap()
        .admins
        .push(id(51));
    let changed = f.quote(&l, &Fixture::choice()).unwrap();
    assert_ne!(changed.groups, sheet.groups);
    assert_ne!(changed.plan, quote.plan);
    let refreshed = f.sheet(&l).unwrap();
    assert_eq!(refreshed.groups, changed.groups);
    let ConnectScope::Group(g) = &refreshed.bindings[0].scope else {
        panic!("expected a group binding, got {:?}", refreshed.bindings[0]);
    };
    assert_eq!(g.admins, vec![b58(&id(43)), b58(&id(51))]);
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

fn renew(f: &Fixture, mut request: AppConnectRequest, choice: &ConnectChoice) -> Vec<KeyID> {
    request.app_eph_pub =
        PublicKey::from_secret_key(&SecretKey::from_secret_bytes([8u8; 32]).unwrap()).serialize();
    let l = devnet(request);
    let grant = f.quoted_grant(&l, choice);
    f.approve(&l, choice, &grant).unwrap();
    f.net.updates.lock().unwrap().last().unwrap().1.clone()
}

fn first_record(f: &Fixture) -> ConnectRecord {
    f.engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .remove(0)
}

/// Every session key an approval adds, for each kind of binding, passes
/// the check that lets a renewal or Disconnect disable it.
#[test]
fn every_added_session_key_passes_the_disconnect_check() {
    let f = Fixture::new();
    let mut r = request();
    r.auth_bindings.push(AuthBinding {
        bounds: Bounds::Contract(app().to_buffer()),
        level: Level::High,
        budget: Some(1_000_000_000),
        expires_at_ms: Some(NOW + 90 * DAY_MS),
    });
    let choice = ConnectChoice {
        selection: vec![0, 1, 2],
        terms: ConnectTerms {
            funding_limit: 52_000_000_000,
            ..Fixture::choice().terms
        },
        ..Fixture::choice()
    };
    let l = devnet(r);
    let grant = f.quoted_grant(&l, &choice);
    f.approve(&l, &choice, &grant).unwrap();
    let record = first_record(&f);
    let me = f.net.me();
    let mut kinds = BTreeSet::new();
    for key_id in &record.session_keys {
        let key = &me.public_keys()[key_id];
        assert_eq!(check_disconnect_key(&record, key), Ok(()), "key {key_id}");
        kinds.insert(match key.contract_bounds().unwrap() {
            ContractBounds::SingleContract { .. } => "contract",
            ContractBounds::SingleContractDocumentType { .. } => "documentType",
            ContractBounds::ContractGroup { .. } => "group",
        });
    }
    assert_eq!(record.session_keys.len(), 3);
    assert_eq!(kinds.len(), 3, "{kinds:?}");
}

/// A renewal disables only what Disconnect would: keys its record added,
/// whatever other ids the local record names, even with their key data.
#[test]
fn a_renewal_disables_only_keys_its_record_added() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.approve(&l, &Fixture::choice(), &grant).unwrap();
    let elsewhere = || Some(ContractBounds::SingleContract { id: id(77) });
    let key = |key_id, purpose, level, key_type, bounds, with_limits| {
        make_session_key_v1(
            key_id,
            purpose,
            level,
            key_type,
            bounds,
            vec![key_id as u8; 20],
            with_limits,
        )
    };
    use KeyType::{ECDSA_HASH160, ECDSA_SECP256K1};
    use Purpose::{AUTHENTICATION, ENCRYPTION};
    use SecurityLevel::{HIGH, MEDIUM};
    let foreign = [
        key(20, AUTHENTICATION, HIGH, ECDSA_HASH160, elsewhere(), false),
        key(21, AUTHENTICATION, HIGH, ECDSA_HASH160, None, true),
        key(
            22,
            AUTHENTICATION,
            HIGH,
            ECDSA_HASH160,
            Some(ContractBounds::SingleContract {
                id: dashpay_contract::ID,
            }),
            true,
        ),
        key(23, AUTHENTICATION, MEDIUM, ECDSA_HASH160, elsewhere(), true),
        key(24, AUTHENTICATION, HIGH, ECDSA_SECP256K1, elsewhere(), true),
        key(25, ENCRYPTION, HIGH, ECDSA_HASH160, elsewhere(), true),
    ];
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        for k in foreign {
            me.public_keys_mut().insert(k.id(), k);
        }
    }
    let mut record = first_record(&f);
    assert_eq!(record.session_keys, vec![6, 7]);
    let others = [0, 8, 20, 21, 22, 23, 24, 25];
    let me = f.net.me();
    for key_id in others {
        record.session_keys.push(key_id);
        let data = hex::encode(me.public_keys()[&key_id].data().as_slice());
        record.session_key_data.insert(key_id, data);
    }
    f.put_record(record);
    assert_eq!(renew(&f, request(), &Fixture::choice()), vec![6, 7]);
    let me = f.net.me();
    for key_id in others {
        assert!(!me.public_keys()[&key_id].is_disabled(), "key {key_id}");
    }
}

/// A renewal that asks for less still disables every key of the login it
/// replaces.
#[test]
fn a_renewal_that_narrows_its_bindings_disables_the_keys_it_drops() {
    let f = Fixture::new();
    let l = devnet(request());
    let grant = f.quoted_grant(&l, &Fixture::choice());
    f.approve(&l, &Fixture::choice(), &grant).unwrap();
    let mut narrowed = request();
    narrowed.auth_bindings.truncate(1);
    let choice = ConnectChoice {
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 50_000_000_000,
            ..Fixture::choice().terms
        },
        ..Fixture::choice()
    };
    assert_eq!(renew(&f, narrowed, &choice), vec![6, 7]);
    assert!(f.net.me().public_keys()[&7].is_disabled());
}

/// A recorded key whose data no longer matches the key on Platform under
/// that id, or a record with no key data, is left enabled.
#[test]
fn a_renewal_skips_a_recorded_key_it_cannot_match() {
    for (corrupt, expected) in [
        (
            (|r: &mut ConnectRecord| {
                r.session_key_data.insert(6, "00".repeat(20));
            }) as fn(&mut ConnectRecord),
            vec![7],
        ),
        (
            (|r: &mut ConnectRecord| r.session_key_data.clear()) as fn(&mut ConnectRecord),
            vec![],
        ),
    ] {
        let f = Fixture::new();
        let l = devnet(request());
        let grant = f.quoted_grant(&l, &Fixture::choice());
        f.approve(&l, &Fixture::choice(), &grant).unwrap();
        let mut record = first_record(&f);
        assert_eq!(record.session_key_data.len(), 2);
        corrupt(&mut record);
        f.put_record(record);
        assert_eq!(renew(&f, request(), &Fixture::choice()), expected);
        let me = f.net.me();
        for key_id in [6, 7] {
            assert_eq!(
                me.public_keys()[&key_id].is_disabled(),
                expected.contains(&key_id),
                "key {key_id}"
            );
        }
    }
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
fn list_quote_and_disconnect_run_on_mainnet() {
    let f = Fixture::on(true);
    let me_b58 = b58(&id(ME));
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());

    let main = link(request(), NetworkSel::Mainnet);
    let choice = Fixture::choice();
    let grant = f.quoted_grant(&main, &choice);
    f.approve(&main, &choice, &grant).unwrap();

    let listed = f.list_apps(&me_b58).unwrap();
    assert_eq!(listed.apps.len(), 1);
    let req = hex::encode(main.request.request_hash());
    assert_eq!(listed.apps[0].request, req);
    assert_eq!(listed.apps[0].keys.len(), 2);

    let q = f.disconnect_quote(&me_b58, &req).unwrap();
    assert_eq!(q.keys.len(), 2);
    assert!(q.fee > 0 && q.fee <= DISCONNECT_FEE_BOUND);
    let g = f.disconnect_grant(q.request, q.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req, &g).unwrap();
    assert_eq!(done.keys_disabled, q.keys);
    assert!(
        f.net.signed().ends_with(&[0]),
        "MASTER key 0 signed the update"
    );
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
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
    //     WITHOUT auto-resigning, invalidates the pre-conflict grant so a re-quote is
    //     required (r4712282286), and returns DisconnectConflict.
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
    assert!(
        !f.disconnect_grant_unused(&g2_try2),
        "conflict retirement must invalidate the pre-conflict grant"
    );
    assert_eq!(
        f.disconnect(&me_b58, &req2, &g2_try2),
        Err(ConnectError::Platform(PlatformError::GrantInvalid))
    );
    assert_eq!(
        f.net.signed_disables.lock().unwrap().len(),
        signed_after_try1
    );
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
    let broadcasts_before_reuse = f.net.broadcast_disables.lock().unwrap().len();
    assert!(
        !f.disconnect_grant_unused(&g2),
        "conflict retirement must invalidate the pre-conflict grant so the next attempt re-quotes"
    );
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g2),
        Err(ConnectError::Platform(PlatformError::GrantInvalid))
    );
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    assert_eq!(
        f.net.broadcast_disables.lock().unwrap().len(),
        broadcasts_before_reuse
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

/// An unresolved transfer at `tip + 1` keeps approve from signing at that
/// nonce, so the transfer is never reported as landed on approve's nonce;
/// approve signs at `tip + 1` of its own read once the transfer resolves,
/// and refuses on a read behind a nonce this session saw used.
#[test]
fn approve_refuses_while_a_transfer_holds_the_next_nonce() {
    use crate::platform::errors::CreditsError;

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let amount = 100_000_000;
    let tq = f.transfer_quote(&me_b58, amount).unwrap();
    let w_grant1 = f.withdraw_grant(amount + tq.fee_credits);
    f.net.transfer_timeout.store(true, Ordering::SeqCst);
    let t1_hash = match f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant1) {
        Err(CreditsError::Platform(PlatformError::BroadcastUnknown { artifact })) => artifact,
        other => panic!("expected BroadcastUnknown, got {other:?}"),
    };
    f.net.transfer_timeout.store(false, Ordering::SeqCst);

    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    assert_eq!(
        f.approve(&l, &c, &g),
        Err(ConnectError::Failed {
            failure: ConnectFailure::PendingWrite,
        })
    );
    assert!(f.net.signed().is_empty());
    assert!(f.net.update_nonces.lock().unwrap().is_empty());
    assert!(f.unused(&g));
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());

    let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
        Err(CreditsError::MovePending {
            id: t1_hash.clone(),
        })
    );

    f.net.nonces.lock().unwrap().insert(id(ME), 1);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
        Err(CreditsError::WithdrawalLanded { artifact: t1_hash })
    );
    f.net.reads.lock().unwrap().clear();
    f.approve(&l, &c, &g).unwrap();
    assert_eq!(*f.net.update_nonces.lock().unwrap(), [2]);
    let nonce_reads = f
        .net
        .reads
        .lock()
        .unwrap()
        .iter()
        .filter(|r| **r == "identity_nonce")
        .count();
    assert_eq!(nonce_reads, 1);

    f.s.withdrawal_nonces.advance_floor(id(ME), 4);
    let l2 = devnet(second_app_request());
    let c2 = ConnectChoice {
        identity: me_b58,
        selection: vec![0],
        terms: ConnectTerms {
            funding_limit: 5_000_000_000,
            expires_at_ms: NOW + 30 * DAY_MS,
        },
    };
    let g2 = f.quoted_grant(&l2, &c2);
    assert_eq!(
        f.approve(&l2, &c2, &g2),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    assert_eq!(f.net.update_nonces.lock().unwrap().len(), 1);
    assert!(f.unused(&g2));
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

/// Reviewer F / CodeRabbit 4711025633 test (a) + r4712282279:
/// Post-rebroadcast revision conflict with `k` still admissible returns
/// `DisconnectConflict`, invalidates the pre-conflict grant, keeps `k` reserved
/// (`last_conflict.nonce == Some(k)`), refuses `Unavailable` on lagging reads
/// (`k > tip`), lets other apps' Disconnects and transfers skip past `k`
/// (`>= k + 1`, never signing at `k`), never rebroadcasts dead bytes, then
/// drops the reservation once the dead bytes consume `k` as a paid bump so a
/// timed-out transfer never falsely reports `WithdrawalLanded`.
#[test]
fn disconnect_post_rebroadcast_revision_conflict_reserves_nonce_until_spent_and_prevents_false_landed()
 {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
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

    // Start at tip = 29 so Disconnect signs at k = 30.
    f.net.nonces.lock().unwrap().insert(id(ME), 29);
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
    assert_eq!(f.net.signed_disables.lock().unwrap()[0].1, 30);
    let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

    // Retry rebroadcasts (still lost), and during `confirm_disabled` another
    // device advances identity revision while k = 30 stays admissible (tip = 29).
    *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
        let mut all = net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }));
    let g1_retry = f.disconnect_grant(q1.request, q1.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req1, &g1_retry),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );
    assert!(!f.disconnect_grant_unused(&g1_retry));
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let broadcasts_after_retire = f.net.broadcast_disables.lock().unwrap().len();
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);

    let rec = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req1)
        .unwrap();
    assert!(rec.pending_disconnect.is_none());
    let conflict = rec.last_conflict.expect("last_conflict persisted");
    assert_eq!(
        conflict.nonce,
        Some(30),
        "post-rebroadcast revision retire with admissible k must reserve k"
    );

    // On a lagging read (`tip = 5`, `k > tip`), both transfers and other apps'
    // Disconnects MUST refuse with Unavailable without signing.
    let amount = 100_000_000;
    let tq = f.transfer_quote(&me_b58, amount).unwrap();
    let w_grant = f.withdraw_grant(amount + tq.fee_credits);
    *f.net.stale_nonce.lock().unwrap() = Some(5);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
        Err(CreditsError::Platform(PlatformError::Unavailable))
    );
    let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
    let g2 = f.disconnect_grant(q2.request, q2.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req2, &g2),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    *f.net.stale_nonce.lock().unwrap() = None;
    assert!(f.net.transfers_sent.lock().unwrap().is_empty());
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);

    // On a fresh read (`tip = 29`, `k = 30` reserved), App B's Disconnect skips
    // `k = 30` and signs at `>= 31` (`31`), disabling App B's keys cleanly while
    // never rebroadcasting App A's dead bytes!
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done2 = f.disconnect(&me_b58, &req2, &g2).unwrap();
    assert_eq!(done2.keys_disabled, q2.keys);
    assert_eq!(f.net.signed_disables.lock().unwrap()[1].1, 31);
    let broadcasts = f.net.broadcast_disables.lock().unwrap();
    assert_eq!(broadcasts.len(), broadcasts_after_retire + 1);
    assert_ne!(
        broadcasts[broadcasts_after_retire], dead_bytes,
        "dead bytes must never be rebroadcast"
    );
    drop(broadcasts);

    // Transfer while k = 30 is still admissible (missing bit under tip = 31)
    // skips k = 30 and signs at >= 32 (32), then times out.
    f.net.transfer_timeout.store(true, Ordering::SeqCst);
    let t_err = f
        .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
        .unwrap_err();
    let t_hash = match t_err {
        CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
        other => panic!("expected BroadcastUnknown at nonce >= 32, got {other:?}"),
    };
    f.net.transfer_timeout.store(false, Ordering::SeqCst);

    let sent = f.net.transfers_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
    else {
        panic!("expected transfer transition");
    };
    assert_eq!(
        t.nonce(),
        32,
        "transfer must skip reserved k=30 and sign at exact nonce 32"
    );
    drop(sent);

    // Now the dead bytes are included as a paid BumpIdentityNonceAction consuming k = 30.
    *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
    let pw = f.wallet();
    assert!(
        f.engine
            .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
            .is_err()
    );
    assert!(!crate::platform::withdrawal::takes(
        *f.net.nonces.lock().unwrap().get(&id(ME)).unwrap(),
        30,
        id(ME)
    ));

    // Next transfer retry drops the spent reservation (`nonce == None`), keeps
    // conflict disclosure, and reports MovePending for the timed-out transfer —
    // NEVER falsely reporting WithdrawalLanded!
    let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
        Err(CreditsError::MovePending { id: t_hash })
    );
    let rec_after = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req1)
        .unwrap();
    let conflict_after = rec_after
        .last_conflict
        .expect("conflict disclosure must remain until Disconnect succeeds");
    assert_eq!(
        conflict_after.nonce, None,
        "spent reservation must be dropped after proved read shows k <= tip && !takes"
    );
}

/// Reviewer F / CodeRabbit 4711025633 test (b):
/// After a revision conflict with `k` still admissible (including when another
/// transition moved `tip` past `k` leaving `k` in Drive's missing-nonce bitset,
/// so `k != tip + 1`), the next user-initiated re-quoted Disconnect signs AT `k`
/// (never `tip + 1`) and never rebroadcasts the dead bytes.
#[test]
fn disconnect_requoted_after_revision_conflict_signs_at_reserved_nonce_k() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Start at tip = 4 so first Disconnect signs at k = 5.
    f.net.nonces.lock().unwrap().insert(id(ME), 4);
    let q1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    assert_eq!(f.net.signed_disables.lock().unwrap()[0].1, 5);
    let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

    // A foreign transition executes at nonce 6 (advancing revision AND moving
    // nonce state to tip = 6 with missing-nonce bit for k = 5 set, so
    // `takes(state, 5, id) == true` while `tip + 1 == 7 != 5`).
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }
    let state_with_missing_5 = merge_fake_nonce(4, 6);
    assert!(crate::platform::withdrawal::takes(
        state_with_missing_5,
        5,
        id(ME)
    ));
    assert_eq!(state_with_missing_5 & IDENTITY_NONCE_VALUE_FILTER, 6);
    f.net
        .nonces
        .lock()
        .unwrap()
        .insert(id(ME), state_with_missing_5);

    // Retry hits step-2 revision retire: returns DisconnectConflict, keeps k = 5
    // reserved, and does NOT rebroadcast the dead bytes.
    let broadcasts_before = f.net.broadcast_disables.lock().unwrap().len();
    let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_retire),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );
    assert_eq!(
        f.net.broadcast_disables.lock().unwrap().len(),
        broadcasts_before,
        "step-2 revision retire must never rebroadcast dead bytes"
    );

    // Re-quote discloses prior conflict, and the new Disconnect MUST sign AT k = 5
    // (NOT tip + 1 = 7) and disable the keys cleanly.
    let q2 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert!(q2.prior_attempt_conflict);
    let g2 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &g2).unwrap();
    assert_eq!(done.keys_disabled, q2.keys);

    let signed = f.net.signed_disables.lock().unwrap();
    assert_eq!(signed.len(), 2);
    assert_eq!(
        signed[1].1, 5,
        "re-quoted Disconnect with admissible reserved k must sign at k (5), not tip+1 (7)"
    );
    let broadcasts = f.net.broadcast_disables.lock().unwrap();
    assert_eq!(broadcasts.len(), 2);
    assert_ne!(
        broadcasts[1], dead_bytes,
        "new broadcast must be the fresh transition at k, never the dead bytes"
    );
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

/// Reviewer F / CodeRabbit 4711025633 test (c):
/// Both the pre-broadcast step-2 revision retire AND the fresh-path
/// post-broadcast revision retire keep `k` reserved while admissible, never
/// rebroadcast dead bytes, block transfers from signing at `k`, and drop the
/// reservation once the dead bytes consume `k` so the next transfer signs at
/// `>= k + 1` and never falsely reports `WithdrawalLanded`.
#[test]
fn disconnect_step2_and_fresh_post_broadcast_revision_conflict_reserve_nonce_until_spent() {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    // --- Arm 1: pre-broadcast step-2 revision retire ---
    {
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
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

        // Revision advances while k = 30 stays admissible (tip = 29).
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }

        let g_retire = f.disconnect_grant(q.request, q.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            1,
            "step-2 retire must never rebroadcast dead bytes"
        );
        assert!(
            !f.disconnect_grant_unused(&g_retire),
            "step-2 conflict retire must invalidate the pre-conflict grant"
        );

        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        let conflict = rec.last_conflict.expect("last_conflict recorded");
        assert_eq!(
            conflict.nonce,
            Some(30),
            "step-2 revision retire must reserve admissible nonce k"
        );

        // Stale read (`tip = 5`, `k = 30 > tip`) refuses Unavailable without signing.
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        *f.net.stale_nonce.lock().unwrap() = Some(5);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable))
        );
        *f.net.stale_nonce.lock().unwrap() = None;
        assert!(f.net.transfers_sent.lock().unwrap().is_empty());

        // On a fresh read (`tip = 29`, `k = 30` reserved), transfer skips `k = 30`
        // and signs at `>= 31` (never colliding with `k = 30`); even if it times out
        // and dead bytes then consume `k = 30`, the next transfer drops the spent
        // reservation and reports MovePending — never WithdrawalLanded.
        f.net.transfer_timeout.store(true, Ordering::SeqCst);
        let t_err = f
            .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
            .unwrap_err();
        let t_hash = match t_err {
            CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
            other => panic!("expected BroadcastUnknown, got {other:?}"),
        };
        f.net.transfer_timeout.store(false, Ordering::SeqCst);
        let sent = f.net.transfers_sent.lock().unwrap();
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
        else {
            panic!("expected transfer transition");
        };
        assert_eq!(
            t.nonce(),
            31,
            "transfer must skip reserved k=30 and sign at exact nonce 31"
        );
        drop(sent);

        // Dead bytes consume k = 30 as a paid bump; next transfer drops the
        // reservation and reports MovePending — NEVER WithdrawalLanded.
        *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
        let pw = f.wallet();
        assert!(
            f.engine
                .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
                .is_err()
        );
        let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
            Err(CreditsError::MovePending { id: t_hash })
        );
    }

    // --- Arm 2: fresh-path post-broadcast revision retire ---
    {
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
        *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
            let mut all = net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }));
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert!(rec.pending_disconnect.is_none());
        let conflict = rec.last_conflict.expect("last_conflict recorded");
        assert_eq!(
            conflict.nonce,
            Some(30),
            "fresh-path post-broadcast revision retire must reserve admissible nonce k"
        );

        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        *f.net.stale_nonce.lock().unwrap() = Some(5);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable))
        );
        *f.net.stale_nonce.lock().unwrap() = None;
        assert!(f.net.transfers_sent.lock().unwrap().is_empty());

        f.net.transfer_timeout.store(true, Ordering::SeqCst);
        let t_err = f
            .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
            .unwrap_err();
        let t_hash = match t_err {
            CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
            other => panic!("expected BroadcastUnknown, got {other:?}"),
        };
        f.net.transfer_timeout.store(false, Ordering::SeqCst);
        let sent = f.net.transfers_sent.lock().unwrap();
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
        else {
            panic!("expected transfer transition");
        };
        assert_eq!(
            t.nonce(),
            31,
            "transfer must skip reserved k=30 and sign at exact nonce 31"
        );
        drop(sent);

        *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
        let pw = f.wallet();
        assert!(
            f.engine
                .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
                .is_err()
        );
        let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
            Err(CreditsError::MovePending { id: t_hash })
        );
    }
}

/// Reviewer F / CodeRabbit 4711025633 test (d):
/// Once a reserved conflict nonce `k` is proven spent at `id_height >= nonce_height`
/// (`k <= tip && !takes`), a lagging read does NOT drop it (`Unavailable`), and
/// a consistent read drops the reservation and signs the re-quoted Disconnect at
/// `tip + 1`.
#[test]
fn disconnect_requoted_after_spent_conflict_reservation_drops_reservation_and_signs_at_tip_plus_one()
 {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // Start at tip = 29 so first Disconnect signs at k = 30.
    f.net.nonces.lock().unwrap().insert(id(ME), 29);
    let q1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);
    let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

    // Revision advances while k = 30 stays admissible -> step-2 retire reserves k = 30.
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }
    let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_retire),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );
    let rec = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap();
    assert_eq!(rec.last_conflict.as_ref().and_then(|c| c.nonce), Some(30));

    // Stale read (`tip = 5`, `k = 30 > tip`) MUST keep the reservation and refuse Unavailable.
    *f.net.stale_nonce.lock().unwrap() = Some(5);
    *f.net.identity_height.lock().unwrap() = 105;
    *f.net.nonce_height.lock().unwrap() = 100;
    let q_stale = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g_stale = f.disconnect_grant(q_stale.request, q_stale.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_stale),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    assert!(f.disconnect_grant_unused(&g_stale));
    assert_eq!(f.net.signed_disables.lock().unwrap().len(), 1);
    *f.net.stale_nonce.lock().unwrap() = None;

    // Now dead bytes consume k = 30 (tip = 30), and a consistent read
    // (`id_height >= nonce_height`, `30 <= 30 && !takes`) drops the reservation
    // and signs the re-quoted Disconnect at tip + 1 = 31.
    *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
    let pw = f.wallet();
    assert!(
        f.engine
            .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
            .is_err()
    );
    assert_eq!(*f.net.nonces.lock().unwrap().get(&id(ME)).unwrap(), 30);

    let q2 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    assert!(q2.prior_attempt_conflict);
    let g2 = f.disconnect_grant(q2.request, q2.max_credits);
    f.net.refreshed.store(false, Ordering::SeqCst);
    let done = f.disconnect(&me_b58, &req_hex, &g2).unwrap();
    assert_eq!(done.keys_disabled, q2.keys);
    let signed = f.net.signed_disables.lock().unwrap();
    assert_eq!(signed.len(), 2);
    assert_eq!(
        signed[1].1, 31,
        "after spent reservation is proved, re-quoted Disconnect must sign at tip + 1 (31)"
    );
    assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
}

/// Opus B1 / Reviewer F G5 test (1):
/// Whenever a revision-conflict retire proves `k <= tip && !takes` (`reserved == None`),
/// all three revision-retire arms (step-2, post-rebroadcast, post-fresh-broadcast)
/// MUST advance the session nonce floor past `k`, so a subsequent lagging nonce read
/// (`tip = 29`) cannot hand `k = 30` to a timed-out transfer and later report a false
/// `WithdrawalLanded`.
#[test]
fn disconnect_spent_retire_raises_nonce_floor_preventing_false_landed_on_lagging_read() {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let assert_lagging_transfer_never_false_lands = |f: &Fixture, me_b58: &str, arm: &str| {
        let amount = 100_000_000;
        let tq = f.transfer_quote(me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        f.net.transfer_timeout.store(true, Ordering::SeqCst);
        let t_err = f
            .transfer_to_platform(me_b58, &tq.to, amount, &w_grant)
            .unwrap_err();
        let t_hash = match t_err {
            CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
            other => panic!("{arm}: expected BroadcastUnknown, got {other:?}"),
        };
        *f.net.stale_nonce.lock().unwrap() = None;
        f.net.transfer_timeout.store(false, Ordering::SeqCst);

        let sent = f.net.transfers_sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "{arm}: expected one signed transfer");
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
        else {
            panic!("{arm}: expected transfer transition");
        };
        assert_eq!(
            t.nonce(),
            31,
            "{arm}: lagging read after proven-spent retire must sign transfer at exact nonce 31"
        );
        drop(sent);

        let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(me_b58, &tq.to, amount, &w_grant2),
            Err(CreditsError::MovePending { id: t_hash }),
            "{arm}: timed-out transfer must report MovePending, never WithdrawalLanded"
        );
    };

    // --- Arm 1: step-2 revision retire with dead bytes already consumed as paid bump ---
    {
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
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

        // Revision bumps, then dead bytes are included as a paid conflict consuming k = 30.
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
        let pw = f.wallet();
        assert!(
            f.engine
                .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
                .is_err()
        );
        assert_eq!(*f.net.nonces.lock().unwrap().get(&id(ME)).unwrap(), 30);

        let g_retire = f.disconnect_grant(q.request, q.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert_eq!(rec.last_conflict.as_ref().and_then(|c| c.nonce), None);

        assert_lagging_transfer_never_false_lands(&f, &me_b58, "step2");
    }

    // --- Arm 2: post-rebroadcast revision retire with k already spent ---
    {
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
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );

        *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
            let mut all = net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
            net.nonces.lock().unwrap().insert(id(ME), 30);
        }));
        let g_retry = f.disconnect_grant(q.request, q.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g_retry),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert_eq!(rec.last_conflict.as_ref().and_then(|c| c.nonce), None);

        assert_lagging_transfer_never_false_lands(&f, &me_b58, "post-rebroadcast");
    }

    // --- Arm 3: post-fresh-broadcast revision retire with k already spent ---
    {
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
        *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
            let mut all = net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
            net.nonces.lock().unwrap().insert(id(ME), 30);
        }));
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert_eq!(rec.last_conflict.as_ref().and_then(|c| c.nonce), None);

        assert_lagging_transfer_never_false_lands(&f, &me_b58, "post-fresh");
    }
}

/// Opus B1 / Reviewer F G5 test (2):
/// Step-3 spent-nonce retire (`rev < pending.revision && !takes(k) && k <= tip`) MUST
/// advance the session nonce floor past `k` in BOTH:
/// - Arm 1 (re-quote variant): re-quoted Disconnect signs fresh N at reserved `k = 30`,
///   times out, dead bytes D win `k = 30` as a paid bump, next Disconnect retires N at
///   step 3 (`nonce: None`);
/// - Arm 2 (direct step-3 retire with no prior `last_conflict`): first Disconnect signs
///   at `k = 30`, times out, another identity-nonce transition consumes `k = 30` without
///   advancing identity revision, next Disconnect retires at step 3 (`nonce: None`).
///
/// In both arms, a subsequent lagging read (`tip = 29`) + transfer timeout MUST sign at
/// `>= 31` and the next transfer MUST report `MovePending`, never `WithdrawalLanded`.
#[test]
fn disconnect_step3_spent_nonce_retire_raises_nonce_floor_preventing_false_landed_on_lagging_read()
{
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let assert_lagging_transfer_never_false_lands = |f: &Fixture, me_b58: &str, arm: &str| {
        let amount = 100_000_000;
        let tq = f.transfer_quote(me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        f.net.transfer_timeout.store(true, Ordering::SeqCst);
        let t_err = f
            .transfer_to_platform(me_b58, &tq.to, amount, &w_grant)
            .unwrap_err();
        let t_hash = match t_err {
            CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
            other => panic!("{arm}: expected BroadcastUnknown, got {other:?}"),
        };
        *f.net.stale_nonce.lock().unwrap() = None;
        f.net.transfer_timeout.store(false, Ordering::SeqCst);

        let sent = f.net.transfers_sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "{arm}: expected one signed transfer");
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
        else {
            panic!("{arm}: expected transfer transition");
        };
        assert_eq!(
            t.nonce(),
            31,
            "{arm}: lagging read after step-3 spent-nonce retire must sign transfer at exact nonce 31"
        );
        drop(sent);

        let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(me_b58, &tq.to, amount, &w_grant2),
            Err(CreditsError::MovePending { id: t_hash }),
            "{arm}: timed-out transfer must report MovePending, never WithdrawalLanded"
        );
    };

    // --- Arm 1: re-quote variant (D wins k = 30 while N @ 30 is pending) ---
    {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l = devnet(request());
        let c = Fixture::choice();
        let g = f.quoted_grant(&l, &c);
        f.approve(&l, &c, &g).unwrap();
        let req_hex = hex::encode(l.request.request_hash());

        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
        let g1 = f.disconnect_grant(q1.request, q1.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );

        let q2 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
        let g2 = f.disconnect_grant(q2.request, q2.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g2),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        assert_eq!(f.net.signed_disables.lock().unwrap()[1].1, 30);

        *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
        let pw = f.wallet();
        assert!(
            f.engine
                .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
                .is_err()
        );
        assert_eq!(*f.net.nonces.lock().unwrap().get(&id(ME)).unwrap(), 30);

        let g3 = f.disconnect_grant(q2.request, q2.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g3),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert!(rec.pending_disconnect.is_none());
        assert_eq!(rec.last_conflict.as_ref().and_then(|c| c.nonce), None);

        assert_lagging_transfer_never_false_lands(&f, &me_b58, "step3-requote");
    }

    // --- Arm 2: direct step-3 spent-nonce retire (no prior last_conflict reservation) ---
    {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l = devnet(request());
        let c = Fixture::choice();
        let g = f.quoted_grant(&l, &c);
        f.approve(&l, &c, &g).unwrap();
        let req_hex = hex::encode(l.request.request_hash());

        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
        let g1 = f.disconnect_grant(q1.request, q1.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);

        // Another identity-nonce transition consumes k = 30 without advancing identity revision.
        f.net.nonces.lock().unwrap().insert(id(ME), 30);

        let g2 = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &g2),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert!(rec.pending_disconnect.is_none());
        assert_eq!(rec.last_conflict.as_ref().and_then(|c| c.nonce), None);

        assert_lagging_transfer_never_false_lands(&f, &me_b58, "step3-direct");
    }
}

/// Opus fail-first test (3):
/// When `verified.to_disable.is_empty()` runs while `last_conflict` holds an unresolved
/// nonce reservation `Some(k)`, the nothing-to-disable branch MUST keep `last_conflict`
/// intact so transfers continue refusing `MovePending` instead of colliding on `k`.
#[test]
fn disconnect_nothing_to_disable_preserves_unresolved_conflict_nonce_reservation() {
    use crate::platform::errors::CreditsError;

    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l = devnet(request());
    let c = Fixture::choice();
    let g = f.quoted_grant(&l, &c);
    f.approve(&l, &c, &g).unwrap();
    let req_hex = hex::encode(l.request.request_hash());

    // 1. First Disconnect signs D at k = 30, times out -> pending saved.
    f.net.nonces.lock().unwrap().insert(id(ME), 29);
    let q1 = f.disconnect_quote(&me_b58, &req_hex).unwrap();
    let g1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);

    // 2. Foreign update bumps revision while k = 30 stays admissible -> step-2 retire reserves 30.
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }
    let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req_hex, &g_retire),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );
    let conflict_hash = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap()
        .last_conflict
        .expect("last_conflict recorded")
        .transition_hash;

    // 3. Another foreign update disables all of this app's keys while leaving k = 30
    //    admissible (tip = 31 with missing-nonce bit for 30 set).
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        for &kid in &q1.keys {
            me.public_keys_mut()
                .get_mut(&kid)
                .unwrap()
                .set_disabled_at(NOW);
        }
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }
    let state_with_missing_30 = merge_fake_nonce(29, 31);
    assert!(crate::platform::withdrawal::takes(
        state_with_missing_30,
        30,
        id(ME)
    ));
    f.net
        .nonces
        .lock()
        .unwrap()
        .insert(id(ME), state_with_missing_30);

    // 4. Next Disconnect hits `verified.to_disable.is_empty()`: marks record Disconnected
    //    AND MUST keep `last_conflict` with `nonce == Some(30)`.
    let g_done = f.disconnect_grant(q1.request, q1.max_credits);
    let done = f.disconnect(&me_b58, &req_hex, &g_done).unwrap();
    assert!(done.keys_disabled.is_empty());

    let rec = f
        .engine
        .block_on(records(&f.s, f.wallet))
        .unwrap()
        .into_iter()
        .find(|r| r.request == req_hex)
        .unwrap();
    assert_eq!(rec.state, RecordState::Disconnected);
    assert!(rec.pending_disconnect.is_none());
    assert_eq!(
        rec.last_conflict.as_ref().and_then(|c| c.nonce),
        Some(30),
        "nothing-to-disable branch must preserve unresolved conflict nonce reservation"
    );

    // 5. The preserved reservation STILL refuses Unavailable on a stale read (`tip = 5`,
    //    `k = 30 > tip`), proving it stayed intact, while on a fresh read (`tip = 31` with
    //    missing bit 30) a transfer skips k = 30 and signs at >= 32 (never MovePending).
    let _ = conflict_hash;
    let amount = 100_000_000;
    let tq = f.transfer_quote(&me_b58, amount).unwrap();
    let w_grant = f.withdraw_grant(amount + tq.fee_credits);
    *f.net.stale_nonce.lock().unwrap() = Some(5);
    assert_eq!(
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
        Err(CreditsError::Platform(PlatformError::Unavailable))
    );
    *f.net.stale_nonce.lock().unwrap() = None;
    assert!(f.net.transfers_sent.lock().unwrap().is_empty());
}

/// Reviewer F / bot r4712172979 fail-first test:
/// `approve` signs its `update_identity` at the refreshed cached identity's
/// next revision. When the cache refresh fails while
/// a direct height-bearing `refresh` fetch still succeeds, `approve` MUST refuse
/// before `update_identity`, leaving nothing signed and the grant unused.
#[test]
fn approve_refuses_when_cache_refresh_fails_even_if_direct_fetch_succeeds() {
    let f = Fixture::new();
    let l = devnet(request());
    let c = Fixture::choice();
    let grant = f.quoted_grant(&l, &c);

    // Cache refresh fails while direct identity fetch (`refresh`) still succeeds.
    f.net.fail_cache_refresh.store(true, Ordering::SeqCst);
    let pw = f.wallet();
    assert!(
        f.engine.block_on(f.net.refresh(&pw, id(ME))).is_ok(),
        "direct fetch must still succeed to prove approve requires mandatory cache refresh"
    );

    assert_eq!(
        f.approve(&l, &c, &grant),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    assert!(
        f.net.signed().is_empty(),
        "approve must sign nothing when cache refresh fails"
    );
    assert!(
        f.net.updates.lock().unwrap().is_empty(),
        "approve must not call update_identity when cache refresh fails"
    );
    assert!(
        f.unused(&grant),
        "approve must leave the grant unused when cache refresh fails"
    );
}

/// Reviewer F / bot r4712172990 fail-first test:
/// When two identities (A and B) both have parked sign-in responses for the same
/// unhinted request and A has a refused-Disconnect hold, `retry_send` without an
/// exact hint match MUST refuse (`NothingToSend`), publish nothing, and leave
/// both entries and A's hold untouched. An unmatched hint must also refuse
/// without touching either entry or hold.
#[test]
fn retry_send_refuses_ambiguous_multi_identity_parked_responses_without_touching_holds() {
    let f = Fixture::new();
    let id_a = id(ME);
    let id_b = f.add_second_identity(2);
    let id_a_b58 = b58(&id_a);
    let id_b_b58 = b58(&id_b);

    let l = devnet(request());
    let req_hash = l.request.request_hash();
    let req_hex = hex::encode(req_hash);
    let c_a = Fixture::choice();
    let c_b = ConnectChoice {
        identity: id_b_b58.clone(),
        ..Fixture::choice()
    };
    let g_a = f.quoted_grant(&l, &c_a);
    let g_b = f.quoted_grant(&l, &c_b);

    // Both A and B approve the same unhinted request while publish fails -> both park responses.
    f.net.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(f.approve(&l, &c_a, &g_a), Err(ConnectError::SendFailed));
    assert_eq!(f.approve(&l, &c_b, &g_b), Err(ConnectError::SendFailed));
    f.net.fail_publish.store(false, Ordering::SeqCst);

    // User attempts to Disconnect A and it is refused before submission -> A's parked response is held.
    let q_a = f.disconnect_quote(&id_a_b58, &req_hex).unwrap();
    *f.net.disconnect_fee_override.lock().unwrap() = Some(q_a.fee + 5_000_000);
    let gd_a = f.disconnect_grant_for(q_a.identity_id, q_a.request, q_a.max_credits);
    assert!(matches!(
        f.disconnect(&id_a_b58, &req_hex, &gd_a),
        Err(ConnectError::Platform(PlatformError::GrantExceeded { .. }))
    ));
    *f.net.disconnect_fee_override.lock().unwrap() = None;
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_a, req_hash),
        Some(true),
        "A's parked response must be held after refused Disconnect"
    );
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_b, req_hash),
        Some(false)
    );

    // Unhinted retry while both A and B are parked MUST refuse NothingToSend,
    // publish nothing, and leave A's response + hold and B's response untouched.
    assert_eq!(
        f.retry(&l),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
    assert!(
        f.net.published.lock().unwrap().is_empty(),
        "ambiguous retry must publish nothing"
    );
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_a, req_hash),
        Some(true),
        "ambiguous retry must leave A's parked response and Disconnect hold unchanged"
    );
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_b, req_hash),
        Some(false),
        "ambiguous retry must leave B's parked response unchanged"
    );

    // A hint that matches neither A nor B MUST also refuse without touching either entry or hold.
    let mut unmatched_req = request();
    unmatched_req.identity_hint = Some(id(OTHER).to_buffer());
    assert_eq!(
        f.retry(&devnet(unmatched_req)),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NothingToSend,
        })
    );
    assert!(f.net.published.lock().unwrap().is_empty());
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_a, req_hash),
        Some(true)
    );
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_b, req_hash),
        Some(false)
    );

    // An exact hint for B publishes ONLY B's response while A's response and hold stay untouched.
    let mut hinted_b = request();
    hinted_b.identity_hint = Some(id_b.to_buffer());
    let done_b = f.retry(&devnet(hinted_b)).unwrap();
    assert_eq!(done_b.identity, id_b_b58);
    let published = f.net.published.lock().unwrap().clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0, id_b);
    assert_eq!(
        f.s.platform.app_connect.is_held(f.wallet, id_a, req_hash),
        Some(true),
        "retrying B via exact hint must keep A's response and hold intact"
    );
}

/// Reviewer F / CodeRabbit r4712282279 fail-first test:
/// A retired conflict reservation (`last_conflict.nonce = Some(k)` with no
/// `pending_disconnect`) forbids signing AT `k` only and must NEVER block
/// transfers or other apps' Disconnects at other nonces, even after app A is
/// unlisted via the nothing-to-disable path:
/// - Arm 1 (nothing-to-disable state: A unlisted, tip = 31 with missing bit 30):
///   a credit transfer signs at `>= 32` (never `MovePending`), times out, A's
///   dead bytes then land at `k = 30`, and the next transfer drops the spent
///   reservation and reports `MovePending` — never `WithdrawalLanded`;
/// - Arm 2 (same nothing-to-disable state): app B's Disconnect signs at `>= 32`
///   (never `PendingWrite`) and disables B's keys;
/// - Arm 3 (active A at tip = 29, k = 30 admissible): another write skips `k`
///   by computing the max locally without calling `advance_floor(id, k)`, so
///   app A's own re-quote is NOT wedged by `NonceOwner::Behind` and still signs
///   at `k = 30`.
#[test]
fn disconnect_nothing_to_disable_reservation_never_blocks_transfer_or_other_app_disconnect() {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let setup_nothing_to_disable = || {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
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

        // App A starts Disconnect at k = 30 (from tip = 29), times out, hits
        // step-2 revision conflict -> reserves k = 30.
        f.net.nonces.lock().unwrap().insert(id(ME), 29);
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
        let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();

        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );

        // Foreign update disables all of A's keys and moves nonce to tip = 31
        // with missing bit 30 still admissible (`takes(state, 30) == true`).
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            for &kid in &q1.keys {
                me.public_keys_mut()
                    .get_mut(&kid)
                    .unwrap()
                    .set_disabled_at(NOW);
            }
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        let state_with_missing_30 = merge_fake_nonce(29, 31);
        assert!(crate::platform::withdrawal::takes(
            state_with_missing_30,
            30,
            id(ME)
        ));
        f.net
            .nonces
            .lock()
            .unwrap()
            .insert(id(ME), state_with_missing_30);

        // App A hits nothing-to-disable -> A is unlisted (`Disconnected`), with
        // `last_conflict.nonce == Some(30)` preserved.
        let g_done = f.disconnect_grant(q1.request, q1.max_credits);
        let done = f.disconnect(&me_b58, &req1, &g_done).unwrap();
        assert!(done.keys_disabled.is_empty());
        let listed = f.list_apps(&me_b58).unwrap();
        assert_eq!(listed.apps.len(), 1);
        assert_eq!(listed.apps[0].request, req2);

        (f, me_b58, req1, req2, dead_bytes)
    };

    // --- Arm 1: Transfer signs at >= 32, times out, dead bytes land at 30, never Landed ---
    {
        let (f, me_b58, req1, _req2, dead_bytes) = setup_nothing_to_disable();
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        f.net.transfer_timeout.store(true, Ordering::SeqCst);
        let t_err = f
            .transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
            .unwrap_err();
        let t_hash = match t_err {
            CreditsError::Platform(PlatformError::BroadcastUnknown { artifact }) => artifact,
            other => panic!("transfer must sign at >= 32 (not MovePending), got {other:?}"),
        };
        f.net.transfer_timeout.store(false, Ordering::SeqCst);

        let sent = f.net.transfers_sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
        else {
            panic!("expected transfer transition");
        };
        assert_eq!(
            t.nonce(),
            32,
            "transfer in nothing-to-disable state (tip 31, k=30) must sign at exact nonce 32"
        );
        drop(sent);

        // A's dead bytes now land at k = 30 as a paid bump; transfer's outcome is
        // still decided by its own nonce (>= 32), so retry reports MovePending —
        // NEVER WithdrawalLanded!
        *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
        let pw = f.wallet();
        assert!(
            f.engine
                .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
                .is_err()
        );
        let w_grant2 = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant2),
            Err(CreditsError::MovePending { id: t_hash })
        );
        let rec_a = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        assert_eq!(rec_a.last_conflict.and_then(|c| c.nonce), None);
    }

    // --- Arm 2: App B's Disconnect in the same state signs at >= 32 and lands ---
    {
        let (f, me_b58, _req1, req2, _dead_bytes) = setup_nothing_to_disable();
        let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
        let g2 = f.disconnect_grant(q2.request, q2.max_credits);
        f.net.refreshed.store(false, Ordering::SeqCst);
        let done2 = f.disconnect(&me_b58, &req2, &g2).unwrap();
        assert_eq!(done2.keys_disabled, q2.keys);
        let signed = f.net.signed_disables.lock().unwrap();
        assert_eq!(signed.len(), 2);
        assert_eq!(
            signed[1].1, 32,
            "app B Disconnect in nothing-to-disable state (tip 31, k=30) must sign at exact nonce 32"
        );
        assert!(f.list_apps(&me_b58).unwrap().apps.is_empty());
    }

    // --- Arm 3: Skipping k = tip + 1 (29 -> 31) does NOT raise withdrawal_nonces.floor,
    //            so app A's own re-quote still sees Free and signs AT k = 30 ---
    {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l1 = devnet(request());
        let c1 = Fixture::choice();
        let g1_app = f.quoted_grant(&l1, &c1);
        f.approve(&l1, &c1, &g1_app).unwrap();
        let req1 = hex::encode(l1.request.request_hash());

        f.net.nonces.lock().unwrap().insert(id(ME), 29);
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
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        // Transfer fails before signing (tiny grant) after reading nonce_for -> must NOT
        // have raised withdrawal_nonces.floor past 30.
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let tiny_w = f.withdraw_grant(amount);
        assert!(matches!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &tiny_w),
            Err(CreditsError::Platform(PlatformError::GrantExceeded { .. }))
        ));
        assert_eq!(
            f.s.withdrawal_nonces.floor(id(ME)),
            0,
            "skipping reserved k must compute max locally, never calling advance_floor(k)"
        );
        // App A's own re-quote still signs AT k = 30.
        let q_re = f.disconnect_quote(&me_b58, &req1).unwrap();
        let g_re = f.disconnect_grant(q_re.request, q_re.max_credits);
        f.net.refreshed.store(false, Ordering::SeqCst);
        let done = f.disconnect(&me_b58, &req1, &g_re).unwrap();
        assert_eq!(done.keys_disabled, q_re.keys);
        assert_eq!(f.net.signed_disables.lock().unwrap()[1].1, 30);
    }
}

/// Reviewer F / CodeRabbit 4716732189 fail-first test:
/// A session floor raised by another write landing at `k + 1` (e.g. a transfer
/// skipping reservation `k = 30` and landing at `31`, setting `floor = 32`)
/// proves only that `31` was used — NOT that `30` was spent. `disconnect()`
/// must NEVER clear `last_conflict.nonce = Some(30)` just because
/// `reserved_k < session_floor`:
/// - Arm 1: if A's retry is refused before signing (`GrantExceeded`), the
///   persisted record MUST keep `last_conflict.nonce == Some(30)`; after a
///   restart (fresh session floor = 0, record reloaded from disk) and a lagging
///   read at tip 29, a transfer MUST return `Unavailable`, NEVER sign at 30,
///   and NEVER report a false `WithdrawalLanded` even after A's dead bytes
///   consume 30.
/// - Arm 2: after the transfer lands at 31 (`floor == 32`), A's own re-quoted
///   retry MUST sign at exact nonce `30`, not `32`.
#[test]
fn disconnect_reservation_survives_higher_floor_write_and_blocks_post_restart_lagging_transfer() {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let setup_reserved_30_with_floor_32 = || {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l1 = devnet(request());
        let c1 = Fixture::choice();
        let g1_app = f.quoted_grant(&l1, &c1);
        f.approve(&l1, &c1, &g1_app).unwrap();
        let req1 = hex::encode(l1.request.request_hash());

        // 1. Reservation at k = 30, tip = 29.
        f.net.nonces.lock().unwrap().insert(id(ME), 29);
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
        let dead_bytes = f.net.broadcast_disables.lock().unwrap()[0].clone();
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );

        // 2. A transfer skipping reservation 30 signs at exact nonce 31 and
        //    lands, raising session floor to 32 while 30 remains admissible.
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
            .unwrap();
        {
            let sent = f.net.transfers_sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) =
                &sent[0]
            else {
                panic!("expected credit transfer transition");
            };
            assert_eq!(
                t.nonce(),
                31,
                "transfer skipping reservation 30 at tip 29 must sign at exact nonce 31"
            );
        }
        assert_eq!(
            f.s.withdrawal_nonces.floor(id(ME)),
            32,
            "landing transfer at 31 must raise session floor to 32"
        );
        let live_state = *f.net.nonces.lock().unwrap().get(&id(ME)).unwrap();
        assert_eq!(live_state, merge_fake_nonce(29, 31));
        assert!(crate::platform::withdrawal::takes(live_state, 30, id(ME)));

        (f, me_b58, req1, dead_bytes)
    };

    // --- Arm 1: pre-sign refusal keeps persisted Some(30), and after restart a
    //            lagging read at tip 29 refuses Unavailable and never signs 30 ---
    {
        let (mut f, me_b58, req1, dead_bytes) = setup_reserved_30_with_floor_32();

        // 3. A's retry is refused before signing (cap below fee -> GrantExceeded).
        let q_re = f.disconnect_quote(&me_b58, &req1).unwrap();
        assert!(q_re.max_credits > 1);
        let g_small = f.disconnect_grant(q_re.request, 1);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert!(matches!(
            f.disconnect(&me_b58, &req1, &g_small),
            Err(ConnectError::Platform(PlatformError::GrantExceeded { .. }))
        ));
        assert_eq!(
            f.net.signed_disables.lock().unwrap().len(),
            1,
            "GrantExceeded refusal must not sign a new disconnect transition"
        );
        let stored_before_restart = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        assert_eq!(
            stored_before_restart.last_conflict.and_then(|c| c.nonce),
            Some(30),
            "higher session floor (32) from landing 31 must NOT clear persisted reservation 30"
        );

        // 4. Restart: fresh session floor (0), record reloaded from disk.
        f.restart();
        assert_eq!(
            f.s.withdrawal_nonces.floor(id(ME)),
            0,
            "restarted session must start with a fresh nonce floor"
        );
        let stored_after_restart = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        assert_eq!(
            stored_after_restart.last_conflict.and_then(|c| c.nonce),
            Some(30),
            "reloaded record after restart must still hold reservation 30"
        );

        // 5. Even if A's dead bytes land at 30 on live Platform while a lagging
        //    node still reads tip 29, the transfer skips reserved 30 -> attempts
        //    31 (already spent) -> Unavailable, NEVER signs at 30, and NEVER
        //    reports WithdrawalLanded.
        *f.net.paid_conflict_fee.lock().unwrap() = 500_000;
        let pw = f.wallet();
        assert!(
            f.engine
                .block_on(f.net.broadcast_disable_keys(&pw, id(ME), &dead_bytes))
                .is_err()
        );
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        let amount = 100_000_000;
        let tq2 = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant2 = f.withdraw_grant(amount + tq2.fee_credits);
        let res2 = f.transfer_to_platform(&me_b58, &tq2.to, amount, &w_grant2);
        assert_eq!(
            res2,
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "lagging read at tip 29 after restart must refuse Unavailable instead of reusing 30"
        );
        assert!(
            !matches!(res2, Err(CreditsError::WithdrawalLanded { .. })),
            "lagging read at tip 29 must never report WithdrawalLanded"
        );
        for st in f.net.transfers_sent.lock().unwrap().iter() {
            if let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = st
            {
                assert_ne!(
                    t.nonce(),
                    30,
                    "transfer must NEVER sign at reserved nonce 30"
                );
            }
        }
    }

    // --- Arm 2: after step 2 (floor == 32), A's own retry signs at exact 30, not 32 ---
    {
        let (f, me_b58, req1, _dead_bytes) = setup_reserved_30_with_floor_32();
        let q_re = f.disconnect_quote(&me_b58, &req1).unwrap();
        let g_re = f.disconnect_grant(q_re.request, q_re.max_credits);
        f.net.refreshed.store(false, Ordering::SeqCst);
        let done = f.disconnect(&me_b58, &req1, &g_re).unwrap();
        assert_eq!(done.keys_disabled, q_re.keys);
        let signed = f.net.signed_disables.lock().unwrap();
        assert_eq!(signed.len(), 2);
        assert_eq!(
            signed[1].1, 30,
            "App A retry with floor=32 must still sign at its own admissible reservation 30, not 32"
        );
    }
}

/// Bot r4712173000 / G1 fail-first test:
/// Approving a DIFFERENT app on the same identity MUST refuse with
/// `PendingWrite` (leaving nothing signed and the grant unused) while another
/// app's live `pending_disconnect` holds an admissible nonce OR a retired
/// conflict reservation sits ahead of `tip` (`k > tip`, where the SDK's
/// `tip + 1` writer would collide with `k`), and `Unavailable` on a stale read
/// (`k > tip && !takes`). Once `k` is spent (or sits below `tip` as a missing
/// bit under a retired reservation), approving the other app succeeds cleanly.
#[test]
fn approve_different_app_refuses_while_disconnect_or_ahead_reservation_owns_nonce() {
    let f = Fixture::new();
    let me_b58 = b58(&id(ME));
    let l1 = devnet(request());
    let c1 = Fixture::choice();
    let g1 = f.quoted_grant(&l1, &c1);
    f.approve(&l1, &c1, &g1).unwrap();
    let req1 = hex::encode(l1.request.request_hash());

    // 1. Live pending_disconnect for App 1 at k = 30 (from tip = 29).
    f.net.nonces.lock().unwrap().insert(id(ME), 29);
    let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
    let gd1 = f.disconnect_grant(q1.request, q1.max_credits);
    f.net.lose_updates.store(true, Ordering::SeqCst);
    f.net.refreshed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.disconnect(&me_b58, &req1, &gd1),
        Err(ConnectError::Failed {
            failure: ConnectFailure::NotConfirmed,
        })
    );
    f.net.lose_updates.store(false, Ordering::SeqCst);

    // Approving a DIFFERENT app (App 2) on the same identity MUST refuse
    // PendingWrite, sign nothing, and leave App 2's grant unused.
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
    let signed_before = f.net.signed().len();
    let updates_before = f.net.updates.lock().unwrap().len();
    assert_eq!(
        f.approve(&l2, &c2, &g2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::PendingWrite,
        })
    );
    assert_eq!(f.net.signed().len(), signed_before);
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);
    assert!(f.unused(&g2));

    // 2. Retire App 1's pending_disconnect on revision conflict -> reservation
    //    `last_conflict.nonce = Some(30)` while tip = 29 (k = 30 > tip).
    {
        let mut all = f.net.identities.lock().unwrap();
        let me = all.get_mut(&id(ME)).unwrap();
        let next_rev = me.revision() + 1;
        me.set_revision(next_rev);
    }
    let gd_retire = f.disconnect_grant(q1.request, q1.max_credits);
    assert_eq!(
        f.disconnect(&me_b58, &req1, &gd_retire),
        Err(ConnectError::Failed {
            failure: ConnectFailure::DisconnectConflict,
        })
    );

    // While reserved k = 30 is ahead of tip = 29, approving App 2 MUST still
    // refuse PendingWrite (and Unavailable on a stale read tip = 5).
    assert_eq!(
        f.approve(&l2, &c2, &g2),
        Err(ConnectError::Failed {
            failure: ConnectFailure::PendingWrite,
        })
    );
    *f.net.stale_nonce.lock().unwrap() = Some(5);
    assert_eq!(
        f.approve(&l2, &c2, &g2),
        Err(ConnectError::Platform(PlatformError::Unavailable))
    );
    *f.net.stale_nonce.lock().unwrap() = None;
    assert_eq!(f.net.signed().len(), signed_before);
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);
    assert!(f.unused(&g2));

    // 3. Once tip moves to 31 with missing bit 30 (k = 30 <= tip and
    //    tip >= observed_tip 29), approving App 2 succeeds cleanly.
    let state_with_missing_30 = merge_fake_nonce(29, 31);
    f.net
        .nonces
        .lock()
        .unwrap()
        .insert(id(ME), state_with_missing_30);
    let g2_fresh = f.quoted_grant(&l2, &c2);
    f.approve(&l2, &c2, &g2_fresh).unwrap();
    assert_eq!(f.net.updates.lock().unwrap().len(), updates_before + 1);
    assert_eq!(f.net.update_nonces.lock().unwrap().last(), Some(&32));
}

/// Reviewer F / thepastaclaw r4717147458 fail-first test:
/// Skipping past a retired conflict reservation requires a nonce read at least
/// as current as the tip observed when the reservation was created
/// (`tip >= observed_tip`, failing closed when `observed_tip` is `None`).
/// - Disconnect signs at 30; another device's update lands at 31 leaving 30
///   unused; retire at consistent tip 31 persists `{nonce: 30, observed_tip: 31}`.
/// - A transfer, another app's Disconnect, or Approve G1 on a lagging read
///   below `observed_tip` (tip 29, or tip 31 when `observed_tip == 32`) MUST
///   refuse `Unavailable` with ZERO signatures, ZERO broadcasts, and never
///   `WithdrawalLanded`.
/// - On a current read at tip 31, the transfer signs at exact nonce 32.
/// - After restart (fresh floor, reloaded record), a lagging read at tip 29
///   still refuses `Unavailable` with ZERO signatures/broadcasts.
/// - A legacy reservation with `observed_tip == None` fails closed (`Unavailable`).
#[test]
fn disconnect_reservation_observed_tip_blocks_lagging_skip_and_allows_current_read() {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let setup_retire_at_tip_31 = || {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
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

        // Disconnect App 1 at nonce 30 (from tip 29), lost in flight.
        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
        let gd1 = f.disconnect_grant(q1.request, q1.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &gd1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);

        // Another device's update lands at nonce 31 leaving 30 unused, advancing
        // identity revision.
        let state_31_missing_30 = merge_fake_nonce(29, 31);
        f.net
            .nonces
            .lock()
            .unwrap()
            .insert(id(ME), state_31_missing_30);
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        // Retire at consistent tip 31: persists `{nonce: Some(30), observed_tip: Some(31)}`.
        let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        let conflict = rec.last_conflict.expect("last_conflict persisted");
        assert_eq!(conflict.nonce, Some(30));
        assert_eq!(conflict.observed_tip, Some(31));

        (f, me_b58, req1, req2)
    };

    // --- Arm 1: lagging read at tip 29 (no restart) refuses Unavailable with
    //            zero signatures/broadcasts across transfer & other Disconnect;
    //            current read at tip 31 signs transfer at exact nonce 32 ---
    {
        let (f, me_b58, _req1, req2) = setup_retire_at_tip_31();
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        let res = f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant);
        assert_eq!(
            res,
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "lagging read tip 29 < observed_tip 31 must refuse Unavailable"
        );
        assert!(
            f.net.transfers_sent.lock().unwrap().is_empty(),
            "lagging skip refusal must sign and broadcast zero transfers"
        );

        // Other app's Disconnect on lagging tip 29 also refuses Unavailable
        // with zero new signatures or broadcasts.
        let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
        let g2 = f.disconnect_grant(q2.request, q2.max_credits);
        let signed_before = f.net.signed_disables.lock().unwrap().len();
        let bcast_before = f.net.broadcast_disables.lock().unwrap().len();
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req2, &g2),
            Err(ConnectError::Platform(PlatformError::Unavailable)),
            "other app Disconnect on lagging tip 29 < observed_tip 31 must refuse Unavailable"
        );
        assert_eq!(f.net.signed_disables.lock().unwrap().len(), signed_before);
        assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), bcast_before);

        // Current read at tip 31 -> transfer signs at exact nonce 32 and lands.
        *f.net.stale_nonce.lock().unwrap() = None;
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
            .unwrap();
        let sent = f.net.transfers_sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) = &sent[0]
        else {
            panic!("expected credit transfer transition");
        };
        assert_eq!(
            t.nonce(),
            32,
            "current read at tip 31 must skip reserved 30 and sign at exact nonce 32"
        );
    }

    // --- Arm 2: restart + reload + lagging read at tip 29 refuses Unavailable
    //            with zero signatures/broadcasts; Approve G1 <= tip also checks
    //            observed_tip; observed_tip == None fails closed ---
    {
        let (mut f, me_b58, req1, _req2) = setup_retire_at_tip_31();
        f.restart();
        assert_eq!(f.s.withdrawal_nonces.floor(id(ME)), 0);
        let reloaded = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        assert_eq!(
            reloaded.last_conflict.as_ref().and_then(|c| c.nonce),
            Some(30)
        );
        assert_eq!(
            reloaded.last_conflict.as_ref().and_then(|c| c.observed_tip),
            Some(31)
        );

        *f.net.stale_nonce.lock().unwrap() = Some(29);
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "restarted lagging read tip 29 < observed_tip 31 must refuse Unavailable"
        );
        assert!(
            f.net.transfers_sent.lock().unwrap().is_empty(),
            "restarted lagging skip refusal must sign and broadcast zero transfers"
        );

        // Approve G1 <=-tip reservation allowance: when stored observed_tip is
        // 32 and lagging read shows tip 31 (with missing bit 30), Approve must
        // refuse Unavailable instead of proceeding.
        let mut rec_obs32 = reloaded.clone();
        rec_obs32.last_conflict.as_mut().unwrap().observed_tip = Some(32);
        f.put_record(rec_obs32);
        *f.net.stale_nonce.lock().unwrap() = Some(merge_fake_nonce(29, 31));
        let mut req3 = second_app_request();
        let sk3 = SecretKey::from_secret_bytes([88u8; 32]).unwrap();
        req3.app_eph_pub = PublicKey::from_secret_key(&sk3).serialize();
        req3.connection_id = Some([88; 16]);
        let l3 = devnet(req3);
        let c3 = ConnectChoice {
            identity: me_b58.clone(),
            selection: vec![0],
            terms: ConnectTerms {
                funding_limit: 5_000_000_000,
                expires_at_ms: NOW + 30 * DAY_MS,
            },
        };
        let g3 = f.quoted_grant(&l3, &c3);
        let updates_before = f.net.updates.lock().unwrap().len();
        assert_eq!(
            f.approve(&l3, &c3, &g3),
            Err(ConnectError::Platform(PlatformError::Unavailable)),
            "approve G1 must refuse Unavailable when tip 31 < observed_tip 32"
        );
        assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);
        assert!(f.unused(&g3));

        // Legacy reservation with observed_tip == None fails closed on transfer.
        let mut rec_none = reloaded;
        rec_none.last_conflict.as_mut().unwrap().observed_tip = None;
        f.put_record(rec_none);
        *f.net.stale_nonce.lock().unwrap() = None;
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "legacy reservation with observed_tip == None must fail closed"
        );
        assert!(f.net.transfers_sent.lock().unwrap().is_empty());
    }
}

/// Reviewer F / thepastaclaw r4717147468 fail-first test:
/// The pending-Disconnect rebroadcast path must enforce the same funds gate as
/// `begin_issuing` (restore mark, wallet Platform hold, Platform funds off),
/// sending ZERO `broadcast_disable_keys` calls and preserving `pending_disconnect`
/// and the grant on refusal, while still allowing read-only confirmation when
/// the pending transition already landed on Platform.
#[test]
fn disconnect_pending_rebroadcast_enforces_funds_gate_while_allowing_confirmation() {
    let setup_pending = || {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l = devnet(request());
        let c = Fixture::choice();
        let g = f.quoted_grant(&l, &c);
        f.approve(&l, &c, &g).unwrap();
        let req_hex = hex::encode(l.request.request_hash());

        let q = f.disconnect_quote(&me_b58, &req_hex).unwrap();
        let gd = f.disconnect_grant(q.request, q.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &gd),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);
        assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), 1);
        (f, me_b58, req_hex, q)
    };

    // --- Arm 1 & Arm 2: restored mark blocks rebroadcast (0 broadcasts, pending
    //                    and grant kept), then already-landed pending confirms
    //                    to DisconnectDone with 0 broadcasts ---
    {
        let (mut f, me_b58, req_hex, q) = setup_pending();
        let dir = f.s.data_dir().join(dw_store_upgrade::UPGRADE_BACKUPS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("RESTORED"), "restored\nformat 1\n").unwrap();
        f.restart();
        assert!(f.s.restored(), "restored mark must engage funds gate");

        let gd_retry = f.disconnect_grant(q.request, q.max_credits);
        let bcast_before = f.net.broadcast_disables.lock().unwrap().len();
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &gd_retry),
            Err(ConnectError::Platform(PlatformError::Restored)),
            "pending rebroadcast under restore mark must refuse Restored"
        );
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            bcast_before,
            "restored funds gate must emit ZERO broadcast_disable_keys calls"
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert!(
            rec.pending_disconnect.is_some(),
            "pending_disconnect must be preserved when funds gate refuses rebroadcast"
        );
        assert!(
            f.s.vault
                .check_grant(&gd_retry, GrantKind::AppDisconnect, Some(&f.wallet.0))
                .is_ok(),
            "grant must stay valid (not revoked/redeemed) on funds gate refusal"
        );

        // Arm 2: once Platform shows the pending keys disabled, confirmation-only
        // recovery still succeeds under the restored mark with ZERO broadcasts.
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            for &kid in &q.keys {
                if let Some(k) = me.get_public_key_by_id_mut(kid) {
                    k.set_disabled_at(NOW);
                }
            }
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        let done = f.disconnect(&me_b58, &req_hex, &gd_retry).unwrap();
        assert_eq!(done.identity, me_b58);
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            bcast_before,
            "confirmation-only recovery under restored mark must emit ZERO broadcasts"
        );
        let rec_done = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert!(rec_done.pending_disconnect.is_none());
        assert_eq!(rec_done.state, RecordState::Disconnected);
    }

    // --- Arm 3: Platform funds off blocks pending rebroadcast with ZERO
    //            broadcasts and preserves pending + grant ---
    {
        let (f, me_b58, req_hex, q) = setup_pending();
        f.s.platform
            .funds_off_in_tests
            .store(true, Ordering::SeqCst);
        let gd_retry = f.disconnect_grant(q.request, q.max_credits);
        let bcast_before = f.net.broadcast_disables.lock().unwrap().len();
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req_hex, &gd_retry),
            Err(ConnectError::Platform(PlatformError::SessionOff)),
            "pending rebroadcast with Platform funds off must refuse SessionOff"
        );
        assert_eq!(
            f.net.broadcast_disables.lock().unwrap().len(),
            bcast_before,
            "funds-off gate must emit ZERO broadcast_disable_keys calls"
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req_hex)
            .unwrap();
        assert!(rec.pending_disconnect.is_some());
        assert!(
            f.s.vault
                .check_grant(&gd_retry, GrantKind::AppDisconnect, Some(&f.wallet.0))
                .is_ok()
        );
    }
}

/// Reviewer F / Opus B1 at 2545d85dc fail-first test:
/// Because `read_consistent_state` reads the nonce before the identity, a
/// retire whose nonce read lagged at height `H - 1` (tip 29) while the
/// identity read at height `H` proved the conflicting revision bump (foreign
/// write at 31) stores `observed_tip = Some(29)` AND
/// `observed_height = Some(max(id_height, nonce_height)) = Some(H)`.
/// Every skip past the reservation MUST compare the current nonce read's
/// height against `observed_height` (failing closed on `None`):
/// - (a) Step-2 retire with split read `(nonce_height = 100 / tip 29, id_height = 101)`
///   persists `{nonce: 30, observed_tip: 29, observed_height: 101}`; a transfer
///   at `nonce_height = 100` (tip 29) refuses `Unavailable` with zero signatures
///   and zero broadcasts (never `WithdrawalLanded`), while a read at `>= 101`
///   (tip 31) signs at exact nonce 32.
/// - (b) Same split read blocks another app's Disconnect at `nonce_height = 100`
///   (signs at exact 32 at `nonce_height = 101`) and blocks Approve G1 at
///   `nonce_height = 100` even when `tip = 31 >= observed_tip = 29` (grant unused).
/// - (c) Restart + reload of (a) still refuses `Unavailable` at `nonce_height = 100`,
///   and legacy `observed_height == None` fails closed.
/// - (d) Post-rebroadcast and post-fresh retire sites with split read persist
///   `observed_height = Some(max(id_height2, nonce_height2))`.
#[test]
fn disconnect_reservation_observed_height_blocks_split_read_lagging_skip() {
    use crate::platform::errors::CreditsError;
    use dpp::state_transition::identity_credit_transfer_to_addresses_transition::accessors::IdentityCreditTransferToAddressesTransitionAccessorsV0;

    let setup_step2_split_retire = || {
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
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

        // App 1 Disconnect signs at nonce 30 (from tip 29), lost in flight.
        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
        let gd1 = f.disconnect_grant(q1.request, q1.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &gd1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );
        f.net.lose_updates.store(false, Ordering::SeqCst);

        // Foreign write lands at nonce 31 in block H = 101 leaving 30 unused,
        // bumping revision. At step-2 retire, nonce read comes from H - 1 = 100
        // (tip 29) while identity read comes from H = 101.
        let state_31_missing_30 = merge_fake_nonce(29, 31);
        f.net
            .nonces
            .lock()
            .unwrap()
            .insert(id(ME), state_31_missing_30);
        {
            let mut all = f.net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
        }
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        *f.net.nonce_height.lock().unwrap() = 100;
        *f.net.identity_height.lock().unwrap() = 101;

        let g_retire = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &g_retire),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        let conflict = rec.last_conflict.expect("last_conflict persisted");
        assert_eq!(conflict.nonce, Some(30));
        assert_eq!(conflict.observed_tip, Some(29));
        assert_eq!(
            conflict.observed_height,
            Some(101),
            "step-2 split retire must persist observed_height = max(id_height=101, nonce_height=100)"
        );

        (f, me_b58, req1, req2)
    };

    // --- (a) & (b): step-2 split retire blocks lagging H-1 transfer, other-app
    //                Disconnect, and Approve G1; current H=101 read signs at 32 ---
    {
        let (f, me_b58, _req1, req2) = setup_step2_split_retire();

        // (a) Transfer at nonce_height = 100 (tip 29) refuses Unavailable with 0 sent.
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        *f.net.nonce_height.lock().unwrap() = 100;
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "transfer at nonce_height 100 < observed_height 101 must refuse Unavailable"
        );
        assert!(
            f.net.transfers_sent.lock().unwrap().is_empty(),
            "lagging height refusal must sign and broadcast zero transfers"
        );

        // (b1) Other app's Disconnect at nonce_height = 100 (even with id_height = 101)
        //      must compare nonce_height (not id_height) and refuse Unavailable.
        let q2 = f.disconnect_quote(&me_b58, &req2).unwrap();
        let g2 = f.disconnect_grant(q2.request, q2.max_credits);
        let signed_before = f.net.signed_disables.lock().unwrap().len();
        let bcast_before = f.net.broadcast_disables.lock().unwrap().len();
        *f.net.stale_nonce.lock().unwrap() = Some(29);
        *f.net.nonce_height.lock().unwrap() = 100;
        *f.net.identity_height.lock().unwrap() = 101;
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req2, &g2),
            Err(ConnectError::Platform(PlatformError::Unavailable)),
            "other app Disconnect at nonce_height 100 < observed_height 101 must refuse Unavailable"
        );
        assert_eq!(f.net.signed_disables.lock().unwrap().len(), signed_before);
        assert_eq!(f.net.broadcast_disables.lock().unwrap().len(), bcast_before);

        // (b2) Approve G1 at nonce_height = 100 with tip 31 (>= observed_tip 29,
        //      missing bit 30 <= tip) MUST refuse Unavailable on observed_height = 101
        //      and keep the grant unused.
        *f.net.stale_nonce.lock().unwrap() = Some(merge_fake_nonce(29, 31));
        *f.net.nonce_height.lock().unwrap() = 100;
        let mut req3 = second_app_request();
        let sk3 = SecretKey::from_secret_bytes([91u8; 32]).unwrap();
        req3.app_eph_pub = PublicKey::from_secret_key(&sk3).serialize();
        req3.connection_id = Some([91; 16]);
        let l3 = devnet(req3);
        let c3 = ConnectChoice {
            identity: me_b58.clone(),
            selection: vec![0],
            terms: ConnectTerms {
                funding_limit: 5_000_000_000,
                expires_at_ms: NOW + 30 * DAY_MS,
            },
        };
        let g3 = f.quoted_grant(&l3, &c3);
        let updates_before = f.net.updates.lock().unwrap().len();
        assert_eq!(
            f.approve(&l3, &c3, &g3),
            Err(ConnectError::Platform(PlatformError::Unavailable)),
            "approve G1 at nonce_height 100 < observed_height 101 must refuse Unavailable"
        );
        assert_eq!(f.net.updates.lock().unwrap().len(), updates_before);
        assert!(f.unused(&g3));

        // Current read at height H = 101 (tip 31): transfer signs at exact nonce 32.
        *f.net.stale_nonce.lock().unwrap() = None;
        *f.net.nonce_height.lock().unwrap() = 101;
        *f.net.identity_height.lock().unwrap() = 101;
        f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant)
            .unwrap();
        {
            let sent = f.net.transfers_sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            let dpp::state_transition::StateTransition::IdentityCreditTransferToAddresses(t) =
                &sent[0]
            else {
                panic!("expected credit transfer transition");
            };
            assert_eq!(
                t.nonce(),
                32,
                "current read at height 101 (tip 31) must skip reserved 30 and sign at exact nonce 32"
            );
        }

        // Other app's Disconnect at height >= 101 now succeeds and signs at >= 33 (past 32).
        f.net.refreshed.store(false, Ordering::SeqCst);
        let done2 = f.disconnect(&me_b58, &req2, &g2).unwrap();
        assert_eq!(done2.keys_disabled, q2.keys);
        let signed = f.net.signed_disables.lock().unwrap();
        assert!(
            signed.last().unwrap().1 >= 32,
            "other app Disconnect at height >= 101 must sign at >= 32"
        );
    }

    // --- (c) Restart + reload of (a): H-1 read is still Unavailable;
    //         legacy observed_height == None fails closed ---
    {
        let (mut f, me_b58, req1, _req2) = setup_step2_split_retire();
        f.restart();
        assert_eq!(f.s.withdrawal_nonces.floor(id(ME)), 0);
        let reloaded = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        assert_eq!(
            reloaded.last_conflict.as_ref().and_then(|c| c.nonce),
            Some(30)
        );
        assert_eq!(
            reloaded.last_conflict.as_ref().and_then(|c| c.observed_tip),
            Some(29)
        );
        assert_eq!(
            reloaded
                .last_conflict
                .as_ref()
                .and_then(|c| c.observed_height),
            Some(101)
        );

        *f.net.stale_nonce.lock().unwrap() = Some(29);
        *f.net.nonce_height.lock().unwrap() = 100;
        let amount = 100_000_000;
        let tq = f.transfer_quote(&me_b58, amount).unwrap();
        let w_grant = f.withdraw_grant(amount + tq.fee_credits);
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "restarted lagging read at height 100 < observed_height 101 must refuse Unavailable"
        );
        assert!(f.net.transfers_sent.lock().unwrap().is_empty());

        // Legacy reservation with observed_height == None fails closed even at height 101.
        let mut rec_none_h = reloaded;
        rec_none_h.last_conflict.as_mut().unwrap().observed_height = None;
        f.put_record(rec_none_h);
        *f.net.stale_nonce.lock().unwrap() = None;
        *f.net.nonce_height.lock().unwrap() = 101;
        assert_eq!(
            f.transfer_to_platform(&me_b58, &tq.to, amount, &w_grant),
            Err(CreditsError::Platform(PlatformError::Unavailable)),
            "legacy reservation with observed_height == None must fail closed"
        );
        assert!(f.net.transfers_sent.lock().unwrap().is_empty());
    }

    // --- (d) Post-rebroadcast and post-fresh retire sites with split read
    //         persist observed_height = Some(max(id_height2, nonce_height2)) ---
    {
        // Post-rebroadcast retire with split read (nonce_height2 = 100, id_height2 = 105).
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l1 = devnet(request());
        let c1 = Fixture::choice();
        let g1_app = f.quoted_grant(&l1, &c1);
        f.approve(&l1, &c1, &g1_app).unwrap();
        let req1 = hex::encode(l1.request.request_hash());

        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
        let gd1 = f.disconnect_grant(q1.request, q1.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &gd1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            })
        );

        *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
            let mut all = net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
            net.nonces
                .lock()
                .unwrap()
                .insert(id(ME), merge_fake_nonce(29, 31));
            *net.stale_nonce.lock().unwrap() = Some(29);
            *net.nonce_height.lock().unwrap() = 100;
            *net.identity_height.lock().unwrap() = 105;
        }));
        let gd_rebroadcast = f.disconnect_grant(q1.request, q1.max_credits);
        assert_eq!(
            f.disconnect(&me_b58, &req1, &gd_rebroadcast),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        let conflict = rec.last_conflict.expect("post-rebroadcast conflict stored");
        assert_eq!(conflict.nonce, Some(30));
        assert_eq!(conflict.observed_tip, Some(29));
        assert_eq!(
            conflict.observed_height,
            Some(105),
            "post-rebroadcast split retire must persist observed_height = 105"
        );
    }
    {
        // Post-fresh retire with split read (nonce_height2 = 100, id_height2 = 107).
        let f = Fixture::new();
        let me_b58 = b58(&id(ME));
        let l1 = devnet(request());
        let c1 = Fixture::choice();
        let g1_app = f.quoted_grant(&l1, &c1);
        f.approve(&l1, &c1, &g1_app).unwrap();
        let req1 = hex::encode(l1.request.request_hash());

        f.net.nonces.lock().unwrap().insert(id(ME), 29);
        let q1 = f.disconnect_quote(&me_b58, &req1).unwrap();
        let gd1 = f.disconnect_grant(q1.request, q1.max_credits);
        f.net.lose_updates.store(true, Ordering::SeqCst);
        f.net.refreshed.store(false, Ordering::SeqCst);
        *f.net.after_update.lock().unwrap() = Some(Box::new(|net| {
            let mut all = net.identities.lock().unwrap();
            let me = all.get_mut(&id(ME)).unwrap();
            let next_rev = me.revision() + 1;
            me.set_revision(next_rev);
            net.nonces
                .lock()
                .unwrap()
                .insert(id(ME), merge_fake_nonce(29, 31));
            *net.stale_nonce.lock().unwrap() = Some(29);
            *net.nonce_height.lock().unwrap() = 100;
            *net.identity_height.lock().unwrap() = 107;
        }));
        assert_eq!(
            f.disconnect(&me_b58, &req1, &gd1),
            Err(ConnectError::Failed {
                failure: ConnectFailure::DisconnectConflict,
            })
        );
        let rec = f
            .engine
            .block_on(records(&f.s, f.wallet))
            .unwrap()
            .into_iter()
            .find(|r| r.request == req1)
            .unwrap();
        let conflict = rec.last_conflict.expect("post-fresh conflict stored");
        assert_eq!(conflict.nonce, Some(30));
        assert_eq!(conflict.observed_tip, Some(29));
        assert_eq!(
            conflict.observed_height,
            Some(107),
            "post-fresh split retire must persist observed_height = 107"
        );
    }
}
