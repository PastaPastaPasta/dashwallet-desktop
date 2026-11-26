//! App Connect v2 logins (DashPay Connect, DASHPAY F24; dips#181): a
//! `dashpay://connect` request read against Platform, the sheet the user
//! decides on, and the approval that adds the app's keys to the identity
//! and publishes the sealed response the app polls for.
//!
//! Nothing is derived or signed before the approval: the sheet and the
//! quote read Platform only. An approval runs under an `AppConnect` grant
//! bound to the request and to the digest of everything the quote showed;
//! it reads Platform again and refuses when anything differs. The vault
//! derives and seals the application keys (dw-vault `app_connect`); this
//! side sees their public keys and the sealed payload.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use dpp::ProtocolError;
use dpp::address_funds::AddressWitness;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::accessors::v1::IdentityPublicKeyGettersV1;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::BinaryData;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract;
use dpp::version::PlatformVersion;
use dw_uri::connect::{AppConnectRequest, Bounds, ConnectUri, EncPurpose, Level};
use dw_vault::app_connect::AppKey;
use dw_vault::{GrantKind, GrantPurpose, SignerError, SignerScope, VaultSigner};
use platform_wallet::PlatformWallet;
use serde::{Deserialize, Serialize};

use super::app_connect_net::{ConnectNet, Response, SdkConnect};
use super::app_connect_plan::{
    self as plan, Commitment, ConnectRefusal, ContractFacts, EncPlan, GroupFacts, MAX_FUNDING,
    MIN_PROTOCOL_VERSION, PROFILE_TYPE,
};
use super::dashpay::DashPay;
use super::errors::{ConnectError, PlatformError};
use super::flows::{BudgetPurpose, FlowKind};
use super::identity_writes::identity_locks;
use super::keys_policy::{key_path, missing_dashpay_purposes};
use super::lease::session::HeldKey;
use super::lease::{Lease, SignatureCount};
use super::requests::{leased, write_permit};
use super::signers::VaultIdentitySigner;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

/// What the IdentityUpdate adding the keys may cost, in credits.
pub(crate) const CONNECT_UPDATE_CREDITS: u64 = 300_000_000;
/// What the response document may cost, in credits.
pub(crate) const CONNECT_RESPONSE_CREDITS: u64 = 150_000_000;
/// What one login costs at most: the update and the response.
const CONNECT_FEE_CREDITS: u64 = CONNECT_UPDATE_CREDITS + CONNECT_RESPONSE_CREDITS;
/// Responses one grant pays for: the first and two "Retry sending" (a send
/// that may have gone out keeps its reservation).
const CONNECT_SENDS: u64 = 3;
/// The grant's credit bound.
const CONNECT_MAX_CREDITS: u64 = CONNECT_UPDATE_CREDITS + CONNECT_SENDS * CONNECT_RESPONSE_CREDITS;
/// Platform's most keys added, and disabled, in one IdentityUpdate.
const MAX_ADDED: usize = 6;
const MAX_DISABLED: usize = 10;
/// The confirmation reads after the update: three, 2 s then 4 s apart.
const CONFIRM_WAITS_MS: [u64; 3] = [0, 2_000, 4_000];
/// The App Connect records kept per wallet.
const MAX_RECORDS: usize = 64;
const RECORDS_SETTING: &str = "appconnect.grants";
/// Responses kept for "Retry sending" in one session.
const MAX_PENDING: usize = 8;
/// Disabling one key updates its existing entry (`disabled_at`): bounded
/// conservatively at 512 bytes of storage/processing per key.
const DISCONNECT_KEY_BYTES: u64 = 512;
/// The highest Platform credit fee Disconnect may spend (0.001 DASH).
/// A live fee above this bound is refused rather than clamped.
pub(crate) const DISCONNECT_FEE_BOUND: u64 = 100_000_000;

/// Live Platform credit fee to disable `key_count` keys at `protocol_version`.
pub(crate) fn disconnect_fee_at(
    protocol_version: u32,
    key_count: usize,
) -> Result<u64, PlatformError> {
    if key_count == 0 {
        return Ok(0);
    }
    let version = PlatformVersion::get(protocol_version).map_err(|e| PlatformError::Internal {
        detail: format!("protocol version {protocol_version}: {e}"),
    })?;
    let fees = &version.fee_version;
    let per_byte = fees
        .storage
        .storage_disk_usage_credit_per_byte
        .saturating_add(fees.storage.storage_processing_credit_per_byte);
    Ok(fees
        .state_transition_min_fees
        .identity_update
        .saturating_add(
            (key_count as u64)
                .saturating_mul(DISCONNECT_KEY_BYTES)
                .saturating_mul(per_byte),
        ))
}

/// Whether App Connect runs on `network`: not on mainnet in this build,
/// until its keys ruling; there every call is `connect.mainnet_off`.
pub(crate) fn enabled_on(network: &DashNetwork) -> bool {
    *network != DashNetwork::Mainnet
}

/// Why an App Connect login stopped after it was approved, from a fixed
/// list: the host shows its own words for each, never Platform's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectFailure {
    /// A key this login derives is on the identity with other limits.
    KeyConflict,
    /// Platform did not show every added key in time. The request stays
    /// open: approving it again re-reads the identity, reusing what landed.
    NotConfirmed,
    /// The identity has no key the wallet holds that may sign the response.
    NoResponseKey,
    /// No unsent response of this request is kept (a lock ended it).
    NothingToSend,
    /// A group or encryption key changed after the keys were added: the
    /// response is not sent.
    ScopeChanged,
    /// A key recorded for this app could not be verified on Platform as an
    /// app-added session key of this grant (for example an older grant with
    /// no recorded key digest, or a key whose public data or role does not
    /// match).
    UnverifiedKey,
    /// The live Platform credit fee to disable this app's keys exceeds the
    /// wallet's fee bound.
    FeeTooHigh,
}

/// What the user sets: one funding limit for the connection, one expiry
/// for its keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectTerms {
    /// Credits.
    pub funding_limit: u64,
    pub expires_at_ms: u64,
}

/// What the user picked on the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectChoice {
    /// Base58.
    pub identity: String,
    /// The auth bindings granted, by their index in the request; the
    /// primary (0) always.
    pub selection: Vec<u8>,
    pub terms: ConnectTerms,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectLevel {
    Critical,
    High,
}

/// What a key of one binding may sign for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectScope {
    /// One contract of the app's owner. `tokens`: it has tokens, or an
    /// update could add some.
    Contract { contract: String, tokens: bool },
    /// One document type of a contract of the app's owner.
    DocumentType {
        contract: String,
        document_type: String,
    },
    /// The user's DashPay profile.
    Profile,
    /// A contract group of the app's owner, as Platform proved it now: it
    /// may grow, and the key covers what joins it.
    Group(ConnectGroup),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectGroup {
    pub group: String,
    pub owner: String,
    pub admins: Vec<String>,
    pub contracts: Vec<String>,
    pub document_types: Vec<ConnectDocumentType>,
    pub tokens: Vec<ConnectToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectDocumentType {
    pub contract: String,
    pub document_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectToken {
    pub contract: String,
    pub position: u16,
}

/// One auth binding of the request, as the sheet lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectBinding {
    pub index: u8,
    /// The primary binding, which cannot be turned off.
    pub primary: bool,
    pub level: ConnectLevel,
    pub scope: ConnectScope,
}

/// One of the wallet's identities on the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectIdentity {
    pub identity: String,
    pub username: String,
    /// Credits, as the wallet last read them.
    pub balance: Option<u64>,
}

/// What the sheet shows before the user decides. The label is the app's
/// claim: nothing verifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectSheet {
    /// `hash160(appEphPub)`, hex.
    pub request: String,
    pub app_id: String,
    pub label: String,
    pub app_owner: String,
    /// The identities with a username; the hinted one alone when the
    /// request names one.
    pub identities: Vec<ConnectIdentity>,
    pub bindings: Vec<ConnectBinding>,
    /// An encryption key for the app's contract is asked for.
    pub encryption: bool,
    pub terms: ConnectTerms,
    /// The highest funding limit an approval may set, in credits.
    pub max_funding_limit: u64,
    /// An approval of this request went to Platform from this wallet and
    /// no response to it is published: its keys may be on the identity.
    /// Approving it again with this choice reuses them.
    pub earlier_approval: Option<ConnectChoice>,
    pub block_time_ms: u64,
}

/// What an approval of `choice` costs and the grant it needs: an
/// `AppConnect` grant naming `request` and `plan`, with `max_credits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectQuote {
    pub request: [u8; 20],
    /// The digest of what this quote read.
    pub plan: [u8; 32],
    /// The grant's credit bound: the update and up to three sends.
    pub max_credits: u64,
    /// What the login costs at most: the update and one send.
    pub fee: u64,
    /// Each selected binding's budget, in selection order.
    pub budgets: Vec<u64>,
    pub balance: u64,
}

/// A finished login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectDone {
    pub identity: String,
    /// The key ids added to the identity.
    pub keys_added: Vec<KeyID>,
}

/// One still-enabled AUTHENTICATION session key of a connected app, as
/// Platform proves it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedAppKey {
    pub key_id: KeyID,
    pub level: ConnectLevel,
    pub scope: ConnectScope,
    /// Credits, as Platform proves it.
    pub budget: Option<u64>,
    pub expires_at_ms: Option<u64>,
}

/// One app this wallet approved for an identity that still has enabled
/// session keys on Platform. `label` is the app's own claim: nothing
/// verifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedApp {
    /// `hash160(appEphPub)`, hex: identifies this approval for Disconnect.
    pub request: String,
    pub app_id: String,
    pub label: String,
    pub returns_to: Option<String>,
    pub identity: String,
    pub keys: Vec<ConnectedAppKey>,
    /// Credits.
    pub funding_limit: u64,
    pub expires_at_ms: u64,
    pub approved_at_ms: u64,
}

/// The connected apps of one identity, with the proved Platform block time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedApps {
    pub identity: String,
    pub apps: Vec<ConnectedApp>,
    pub block_time_ms: u64,
}

/// What disconnecting one connected app costs and the `AppDisconnect` grant
/// it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectQuote {
    pub request: [u8; 20],
    pub identity: String,
    /// The verified session key ids that will be disabled.
    pub keys: Vec<KeyID>,
    /// The grant's credit bound, equal to `fee` (never clamped).
    pub max_credits: u64,
    /// Live Platform credit fee to disable `keys`.
    pub fee: u64,
    pub balance: u64,
}

/// A finished disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectDone {
    pub identity: String,
    /// The key ids disabled on Platform (`empty` when already disabled).
    pub keys_disabled: Vec<KeyID>,
}

impl From<ConnectRefusal> for ConnectError {
    fn from(r: ConnectRefusal) -> Self {
        match r {
            ConnectRefusal::Unsupported => ConnectError::Unsupported,
            ConnectRefusal::Invalid => ConnectError::Invalid,
            ConnectRefusal::Owner => ConnectError::Owner,
        }
    }
}

fn b58(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

fn signer_error(e: &SignerError) -> PlatformError {
    match e {
        SignerError::Locked => PlatformError::SignerUnavailable,
        _ => PlatformError::Internal {
            detail: "app connect key".into(),
        },
    }
}

/// The request's application keys under one call permit: public keys
/// freely, each proof of possession and the seal counted as one released
/// signature.
pub(crate) struct AppKeys {
    signer: VaultSigner,
    count: Arc<SignatureCount>,
}

impl AppKeys {
    pub(crate) fn new(
        signer: VaultSigner,
        count: Arc<SignatureCount>,
    ) -> Result<Self, EngineError> {
        if !matches!(signer.scope(), SignerScope::AppConnect { .. }) {
            return Err(EngineError::InvalidArgument(
                "an AppConnect signer is required".into(),
            ));
        }
        Ok(Self { signer, count })
    }

    fn public(
        &self,
        identity: [u8; 32],
        app: &[u8; 33],
        key: &AppKey,
    ) -> Result<[u8; 33], PlatformError> {
        self.signer
            .app_public_key(identity, app, key)
            .map_err(|e| signer_error(&e))
    }

    fn counted<T>(
        &self,
        op: impl FnOnce(&VaultSigner) -> Result<T, SignerError>,
    ) -> Result<T, PlatformError> {
        let signing = self.count.begin().ok_or(PlatformError::SignerUnavailable)?;
        match op(&self.signer) {
            Ok(out) => {
                signing.released();
                Ok(out)
            }
            Err(e) => {
                self.count.observe_err(&e);
                Err(signer_error(&e))
            }
        }
    }

    fn sign(
        &self,
        identity: [u8; 32],
        app: &[u8; 33],
        key: &AppKey,
        key_data: &[u8],
        data: &[u8],
    ) -> Result<[u8; 65], PlatformError> {
        self.counted(|s| s.sign_app_key(identity, app, key, key_data, data))
    }

    fn seal(
        &self,
        owner: [u8; 32],
        app: &[u8; 33],
        keys: &[AppKey],
    ) -> Result<Response, PlatformError> {
        let sealed = self.counted(|s| s.app_connect_seal(owner, app, keys))?;
        Ok(Response {
            request_hash: dw_uri::connect::request_hash(app),
            wallet_ephemeral_pub_key: sealed.wallet_ephemeral_pub_key,
            payload: sealed.payload,
        })
    }
}

/// The IdentityUpdate's signer: the MASTER key for the transition, and
/// each new encryption key for its own proof of possession. Session keys
/// are `ECDSA_HASH160` and sign nothing.
pub(crate) struct ConnectUpdateSigner {
    master: VaultIdentitySigner,
    app: AppKeys,
    identity: [u8; 32],
    app_eph_pub: [u8; 33],
    added: BTreeMap<KeyID, (AppKey, Vec<u8>)>,
}

impl std::fmt::Debug for ConnectUpdateSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectUpdateSigner")
            .field("added", &self.added.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Signer<IdentityPublicKey> for ConnectUpdateSigner {
    async fn sign(
        &self,
        key: &IdentityPublicKey,
        data: &[u8],
    ) -> Result<BinaryData, ProtocolError> {
        let Some((app_key, key_data)) = self.added.get(&key.id()) else {
            return self.master.sign(key, data).await;
        };
        if key.data().as_slice() != key_data.as_slice() {
            return Err(ProtocolError::ExternalSignerError(format!(
                "key {} is not the key this login adds",
                key.id()
            )));
        }
        let signature = (self.app)
            .sign(self.identity, &self.app_eph_pub, app_key, key_data, data)
            .map_err(|e| ProtocolError::ExternalSignerError(e.to_string()))?;
        Ok(signature.to_vec().into())
    }

    async fn sign_create_witness(
        &self,
        key: &IdentityPublicKey,
        _data: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        Err(ProtocolError::InvalidIdentityPublicKeyTypeError(
            dpp::state_transition::errors::InvalidIdentityPublicKeyTypeError::new(key.key_type()),
        ))
    }

    fn can_sign_with(&self, key: &IdentityPublicKey) -> bool {
        match self.added.get(&key.id()) {
            Some((_, data)) => {
                key.key_type() == KeyType::ECDSA_SECP256K1
                    && key.data().as_slice() == data.as_slice()
            }
            None => self.master.can_sign_with(key),
        }
    }
}

/// One login this wallet approved, kept in the app database: what a
/// disconnect, a later login of the same connection and "Retry sending"
/// read. Public keys and ids only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConnectRecord {
    pub(crate) request: String,
    pub(crate) app_id: String,
    pub(crate) label: String,
    pub(crate) identity: String,
    pub(crate) connection_id: Option<String>,
    pub(crate) session_keys: Vec<KeyID>,
    pub(crate) encryption_keys: Vec<KeyID>,
    /// The bindings approved, by index.
    #[serde(default)]
    pub(crate) selection: Vec<u8>,
    pub(crate) funding_limit: u64,
    pub(crate) expires_at_ms: u64,
    pub(crate) state: RecordState,
    pub(crate) created_ms: u64,
    /// The callback host from the sign-in link, if any.
    #[serde(default)]
    pub(crate) returns_to: Option<String>,
    /// Hex-encoded on-chain `data` (20-byte `ECDSA_HASH160`) of each key in
    /// `session_keys`, recorded at approval so Disconnect verifies both key id
    /// and public key data before disabling.
    #[serde(default)]
    pub(crate) session_key_data: BTreeMap<KeyID, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordState {
    /// The update may be out.
    Submitted,
    /// Every key was seen on Platform.
    Confirmed,
    /// The response is out.
    Published,
    /// Every session key of this grant was disabled on Platform.
    Disconnected,
}

/// The responses an approval sealed and could not publish, for "Retry
/// sending": the sealed payload (nothing secret) and the lease that signs
/// the document, which a lock ends.
#[derive(Default)]
pub(crate) struct PendingSends(Mutex<HashMap<(WalletId, [u8; 20]), PendingSend>>);

struct PendingSend {
    lease: Lease,
    identity: Identifier,
    /// The groups the user approved, compared again before each send.
    groups: BTreeMap<Identifier, Option<GroupFacts>>,
    index: u32,
    doc_key: KeyID,
    response: Response,
}

impl PendingSends {
    fn put(&self, wallet: WalletId, send: PendingSend) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= MAX_PENDING {
            map.clear();
        }
        map.insert((wallet, send.response.request_hash), send);
    }

    fn take(&self, wallet: WalletId, request: [u8; 20]) -> Option<PendingSend> {
        (self.0.lock().unwrap_or_else(PoisonError::into_inner)).remove(&(wallet, request))
    }
}

/// The identity of the wallet at its DIP-13 index, with a username.
struct Candidate {
    id: Identifier,
    index: u32,
    username: String,
    balance: Option<u64>,
}

fn candidates(session: &NetworkSession, wallet: WalletId) -> Result<Vec<Candidate>, PlatformError> {
    let summaries = session.identity_summaries(wallet)?;
    Ok(summaries
        .into_iter()
        .filter_map(|s| {
            let username = s.main_name.clone().or_else(|| s.names.first().cloned())?;
            Some(Candidate {
                id: Identifier::from_string(&s.identity, Encoding::Base58).ok()?,
                index: s.index,
                username,
                balance: s.balance,
            })
        })
        .collect())
}

/// The checks that need no read: the network, the flag, what this wallet
/// supports.
fn check_link(network: &DashNetwork, link: &ConnectUri) -> Result<(), ConnectError> {
    if !link.network.matches(network.core_network()) {
        return Err(ConnectError::Network);
    }
    if !enabled_on(network) {
        return Err(ConnectError::MainnetOff);
    }
    link.request
        .check_supported()
        .map_err(|_| ConnectError::Unsupported)?;
    Ok(())
}

/// What Platform says of the request as a whole: protocol, contracts and
/// groups, and the owners the DIP requires.
struct RequestFacts {
    app_owner: Identifier,
    contracts: BTreeMap<Identifier, Option<ContractFacts>>,
    groups: BTreeMap<Identifier, Option<GroupFacts>>,
}

async fn request_facts(
    net: &impl ConnectNet,
    request: &AppConnectRequest,
) -> Result<RequestFacts, ConnectError> {
    if net.protocol_version().await? < MIN_PROTOCOL_VERSION {
        return Err(ConnectError::Protocol);
    }
    let mut contracts = BTreeMap::new();
    for c in plan::named_contracts(request)
        .into_iter()
        .chain(request.enc_bindings.iter().map(|b| plan::id(&b.contract)))
    {
        if let std::collections::btree_map::Entry::Vacant(e) = contracts.entry(c) {
            e.insert(net.contract(c).await?);
        }
    }
    let mut groups = BTreeMap::new();
    for g in plan::named_groups(request) {
        groups.insert(g, net.group(g).await?);
    }
    plan::check_ownership(request, &contracts, &groups)?;
    let app_owner = (contracts.get(&plan::id(&request.app_id)).cloned().flatten())
        .ok_or(ConnectError::Invalid)?
        .owner;
    Ok(RequestFacts {
        app_owner,
        contracts,
        groups,
    })
}

fn scope(bounds: &Bounds, facts: &RequestFacts) -> ConnectScope {
    match bounds {
        b if plan::is_profile(b) => ConnectScope::Profile,
        Bounds::Contract(c) => ConnectScope::Contract {
            contract: b58(&plan::id(c)),
            tokens: (facts.contracts.get(&plan::id(c)).cloned().flatten())
                .is_none_or(|f| f.has_tokens || !f.readonly),
        },
        Bounds::DocumentType(c, name) => ConnectScope::DocumentType {
            contract: b58(&plan::id(c)),
            document_type: name.clone(),
        },
        Bounds::Group(g) => {
            let facts = (facts.groups)
                .get(&plan::id(g))
                .cloned()
                .flatten()
                .unwrap_or_default();
            ConnectScope::Group(ConnectGroup {
                group: b58(&plan::id(g)),
                owner: b58(&facts.owner),
                admins: facts.admins.iter().map(b58).collect(),
                contracts: facts.contracts.iter().map(b58).collect(),
                document_types: (facts.document_types.iter())
                    .map(|(c, t)| ConnectDocumentType {
                        contract: b58(c),
                        document_type: t.clone(),
                    })
                    .collect(),
                tokens: (facts.tokens.iter())
                    .map(|(c, p)| ConnectToken {
                        contract: b58(c),
                        position: *p,
                    })
                    .collect(),
            })
        }
    }
}

/// The request's age against the proved block time.
fn check_age(request: &AppConnectRequest, block_time_ms: u64) -> Result<(), ConnectError> {
    match request.age(block_time_ms) {
        dw_uri::connect::Age::Fresh => Ok(()),
        _ => Err(ConnectError::Stale),
    }
}

pub(crate) async fn sheet(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    link: &ConnectUri,
) -> Result<ConnectSheet, ConnectError> {
    check_link(&session.network, link)?;
    let request = &link.request;
    let mut identities = candidates(session, wallet)?;
    if let Some(hint) = request.identity_hint {
        if identities.is_empty() {
            return Err(ConnectError::NoIdentity);
        }
        identities.retain(|c| c.id == plan::id(&hint));
        if identities.is_empty() {
            return Err(ConnectError::Hint);
        }
    }
    let first = identities.first().ok_or(ConnectError::NoIdentity)?;
    let earlier_approval = (earlier_approval(net, session, wallet, request).await?).filter(|c| {
        parse_identity(&c.identity).is_ok_and(|id| identities.iter().any(|i| i.id == id))
    });
    let facts = request_facts(net, request).await?;
    let (_, block_time_ms) = net.identity(first.id).await?;
    check_age(request, block_time_ms)?;
    let bindings = (request.auth_bindings.iter().enumerate())
        .map(|(i, b)| ConnectBinding {
            index: i as u8,
            primary: i == 0,
            level: match b.level {
                Level::Critical => ConnectLevel::Critical,
                Level::High => ConnectLevel::High,
            },
            scope: scope(&b.bounds, &facts),
        })
        .collect();
    Ok(ConnectSheet {
        request: hex::encode(request.request_hash()),
        app_id: b58(&plan::id(&request.app_id)),
        label: request.label.clone(),
        app_owner: b58(&facts.app_owner),
        identities: (identities.into_iter())
            .map(|c| ConnectIdentity {
                identity: b58(&c.id),
                username: c.username,
                balance: c.balance,
            })
            .collect(),
        bindings,
        encryption: !request.enc_bindings.is_empty(),
        terms: plan::default_terms(request, block_time_ms),
        max_funding_limit: MAX_FUNDING,
        earlier_approval,
        block_time_ms,
    })
}

/// Everything an approval of `choice` rests on, read now.
struct Prepared {
    identity: Identity,
    index: u32,
    selection: Vec<u8>,
    terms: ConnectTerms,
    budgets: Vec<u64>,
    groups: BTreeMap<Identifier, Option<GroupFacts>>,
    encryption: Vec<EncPlan>,
    digest: [u8; 32],
}

/// The choice of this wallet's last submitted approval of `request`; one
/// whose response Platform holds answered it (`connect.answered`).
async fn earlier_approval(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    request: &AppConnectRequest,
) -> Result<Option<ConnectChoice>, ConnectError> {
    let hash = request.request_hash();
    let this = hex::encode(hash);
    let mut found = None;
    for r in records(session, wallet).await? {
        if r.request != this {
            continue;
        }
        if net
            .response_exists(hash, parse_identity(&r.identity)?)
            .await?
        {
            return Err(ConnectError::Answered);
        }
        if r.selection.is_empty() {
            continue;
        }
        found = Some(ConnectChoice {
            identity: r.identity,
            selection: r.selection,
            terms: ConnectTerms {
                funding_limit: r.funding_limit,
                expires_at_ms: r.expires_at_ms,
            },
        });
    }
    Ok(found)
}

/// An update error that proves nothing was sent. Any other, the library's
/// flattened broadcast failures included, may follow an accepted update,
/// so its keys may still land.
fn refused_unsent(e: &PlatformError) -> bool {
    matches!(
        e,
        PlatformError::SignerUnavailable
            | PlatformError::SeedMismatch
            | PlatformError::InsufficientCredits { .. }
            | PlatformError::InvalidArgument { .. }
            | PlatformError::GrantInvalid
            | PlatformError::GrantExceeded { .. }
            | PlatformError::NeedsGrant { .. }
            | PlatformError::Cancelled
            | PlatformError::LeaseRevoked { .. }
            | PlatformError::LeaseExpired
            | PlatformError::WalletNotFound
            | PlatformError::Identity(_)
    )
}

fn parse_identity(text: &str) -> Result<Identifier, ConnectError> {
    Identifier::from_string(text, Encoding::Base58).map_err(|_| {
        PlatformError::InvalidArgument {
            detail: "identity".into(),
        }
        .into()
    })
}

/// The identity's keys of `purpose` bound to `contract` alone, in any
/// state.
fn contract_keys(
    identity: &Identity,
    contract: Identifier,
    purpose: Purpose,
) -> impl Iterator<Item = &IdentityPublicKey> {
    use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
    identity.public_keys().values().filter(move |k| {
        k.purpose() == purpose
            && matches!(k.contract_bounds(), Some(ContractBounds::SingleContract { id }) if *id == contract)
    })
}

/// What each encryption binding gets. No app key goes on the DashPay contract,
/// and none is added or shared unless a lower-id key of its purpose comes
/// first: platform-wallet picks contact-request keys by purpose and id, not
/// by bounds.
async fn encryption_plans(
    net: &impl ConnectNet,
    request: &AppConnectRequest,
    identity: &Identity,
) -> Result<Vec<EncPlan>, ConnectError> {
    let mut out = Vec::with_capacity(request.enc_bindings.len());
    for binding in &request.enc_bindings {
        let contract = plan::id(&binding.contract);
        let (purpose, _) = plan::enc_purpose(binding.purpose);
        let current = net
            .current_contract_key(identity.id(), contract, purpose)
            .await?;
        let current =
            current.and_then(|id| identity.public_keys().get(&id).filter(|k| !k.is_disabled()));
        out.push(match current {
            _ if contract == dashpay_contract::ID => EncPlan::Omit,
            Some(key)
                if key.key_type() == KeyType::ECDSA_SECP256K1 && picked_first(identity, key) =>
            {
                EncPlan::Share {
                    key_id: key.id(),
                    rotation: None,
                }
            }
            Some(_) => EncPlan::Omit,
            None if contract_keys(identity, contract, purpose).next().is_some()
                || missing_dashpay_purposes(identity.public_keys().values()).contains(&purpose) =>
            {
                EncPlan::Omit
            }
            None => EncPlan::Register,
        });
    }
    Ok(out)
}

/// Whether an enabled ECDSA key of `key`'s purpose has a lower id than `key`,
/// so platform-wallet's contact-request selectors take that one, not `key`.
fn picked_first(identity: &Identity, key: &IdentityPublicKey) -> bool {
    identity.public_keys().values().any(|k| {
        k.id() < key.id()
            && k.purpose() == key.purpose()
            && k.key_type() == KeyType::ECDSA_SECP256K1
            && !k.is_disabled()
    })
}

/// The doc key a response is signed with: an enabled ECDSA AUTHENTICATION
/// key at HIGH or CRITICAL with no bounds, budget or expiry, that `usable`.
fn response_key(identity: &Identity, usable: impl Fn(&IdentityPublicKey) -> bool) -> Option<KeyID> {
    identity
        .public_keys()
        .values()
        .find(|k| {
            k.purpose() == Purpose::AUTHENTICATION
                && matches!(
                    k.security_level(),
                    SecurityLevel::HIGH | SecurityLevel::CRITICAL
                )
                && k.key_type() == KeyType::ECDSA_SECP256K1
                && k.contract_bounds().is_none()
                && !k.has_limits()
                && !k.is_disabled()
                && usable(k)
        })
        .map(|k| k.id())
}

fn master_key(identity: &Identity) -> Option<&IdentityPublicKey> {
    identity.public_keys().values().find(|k| {
        k.purpose() == Purpose::AUTHENTICATION
            && k.security_level() == SecurityLevel::MASTER
            && k.key_type() == KeyType::ECDSA_SECP256K1
            && !k.is_disabled()
    })
}

async fn prepare(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    link: &ConnectUri,
    choice: &ConnectChoice,
) -> Result<Prepared, ConnectError> {
    check_link(&session.network, link)?;
    let request = &link.request;
    let id = parse_identity(&choice.identity)?;
    let candidate = (candidates(session, wallet)?.into_iter())
        .find(|c| c.id == id)
        .ok_or(ConnectError::NoIdentity)?;
    if request.identity_hint.is_some_and(|h| plan::id(&h) != id) {
        return Err(ConnectError::Hint);
    }
    let mut selection = choice.selection.clone();
    selection.sort_unstable();
    selection.dedup();
    if selection.first() != Some(&0)
        || selection
            .iter()
            .any(|&i| usize::from(i) >= request.auth_bindings.len())
    {
        return Err(PlatformError::InvalidArgument {
            detail: "selection".into(),
        }
        .into());
    }
    let facts = request_facts(net, request).await?;
    let (identity, block_time_ms) = net.identity(id).await?;
    let identity = identity.ok_or(ConnectError::NoIdentity)?;
    check_age(request, block_time_ms)?;
    if !plan::terms_allowed(&choice.terms, block_time_ms) {
        return Err(PlatformError::InvalidArgument {
            detail: "terms".into(),
        }
        .into());
    }
    if net.response_exists(request.request_hash(), id).await? {
        return Err(ConnectError::Answered);
    }
    let budgets = plan::key_budgets(request, &selection, choice.terms.funding_limit).ok_or(
        PlatformError::InvalidArgument {
            detail: "funding limit".into(),
        },
    )?;
    let encryption = encryption_plans(net, request, &identity).await?;
    if identity.balance() < CONNECT_FEE_CREDITS {
        return Err(ConnectError::Credits {
            needed: CONNECT_FEE_CREDITS,
            available: identity.balance(),
        });
    }
    let body = request.encode();
    let digest = Commitment {
        request: &body,
        identity: id,
        selection: &selection,
        terms: choice.terms,
        budgets: &budgets,
        app_owner: facts.app_owner,
        groups: &facts.groups,
        encryption: &encryption,
    }
    .digest()
    .ok_or(PlatformError::Internal {
        detail: "app connect plan".into(),
    })?;
    Ok(Prepared {
        identity,
        index: candidate.index,
        selection,
        terms: choice.terms,
        budgets,
        groups: facts.groups,
        encryption,
        digest,
    })
}

pub(crate) async fn quote(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    link: &ConnectUri,
    choice: &ConnectChoice,
) -> Result<ConnectQuote, ConnectError> {
    let p = prepare(net, session, wallet, link, choice).await?;
    Ok(ConnectQuote {
        request: link.request.request_hash(),
        plan: p.digest,
        max_credits: CONNECT_MAX_CREDITS,
        fee: CONNECT_FEE_CREDITS,
        budgets: p.budgets,
        balance: p.identity.balance(),
    })
}

/// Refuses a grant that is not this wallet's `AppConnect` grant for this
/// request and plan, or whose cap is below the quote, before it is
/// redeemed: a refusal leaves it for another try.
async fn check_grant(
    session: &NetworkSession,
    wallet: WalletId,
    grant: &str,
    request: [u8; 20],
    digest: [u8; 32],
) -> Result<(), ConnectError> {
    let (vault, g) = (session.vault.clone(), grant.to_owned());
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&g, GrantKind::AppConnect, Some(&wallet.0))
    })
    .await
    .map_err(|e| PlatformError::from(EngineError::from(e)))?
    .map_err(PlatformError::from)?;
    let GrantPurpose::AppConnect {
        max_credits,
        request: granted,
        plan,
    } = purpose
    else {
        return Err(PlatformError::GrantInvalid.into());
    };
    if granted != request {
        return Err(PlatformError::GrantInvalid.into());
    }
    if plan != digest {
        return Err(ConnectError::PlanChanged);
    }
    if max_credits < CONNECT_MAX_CREDITS {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: CONNECT_MAX_CREDITS,
            remaining: max_credits,
        }
        .into());
    }
    Ok(())
}

/// The keys an approval adds and the slots it seals, in the DIP's order.
#[derive(Default)]
struct KeyPlan {
    add: Vec<IdentityPublicKey>,
    /// New encryption keys by id, for their proofs of possession.
    proofs: BTreeMap<KeyID, (AppKey, Vec<u8>)>,
    slots: Vec<AppKey>,
    session_ids: Vec<KeyID>,
    /// Hex-encoded `ECDSA_HASH160` data for every id in `session_ids`.
    session_data: BTreeMap<KeyID, String>,
    encryption_ids: Vec<KeyID>,
}

fn app_encryption(contract: &[u8; 32], purpose: EncPurpose, rotation: Option<u32>) -> AppKey {
    AppKey::Encryption {
        contract: *contract,
        purpose: plan::enc_purpose(purpose).1,
        rotation,
    }
}

/// A session key derived earlier that is on the identity: reused only when
/// it is enabled and equal to what this approval grants (DIP "Session
/// Keys"); else `connect.failed{key_conflict}`.
fn key_plan(
    keys: &AppKeys,
    p: &Prepared,
    request: &AppConnectRequest,
) -> Result<KeyPlan, ConnectError> {
    let owner = p.identity.id().to_buffer();
    let app = &request.app_eph_pub;
    let mut next = p.identity.public_keys().keys().max().map_or(0, |m| m + 1);
    let mut out = KeyPlan::default();
    for (&index, &budget) in p.selection.iter().zip(&p.budgets) {
        let key = AppKey::Session {
            binding_index: index,
        };
        let public = keys.public(owner, app, &key)?;
        let wanted =
            plan::session_key(request, index, next, &public, budget, p.terms.expires_at_ms)
                .ok_or(ConnectError::Invalid)?;
        let found = (p.identity.public_keys().values()).find(|k| k.data() == wanted.data());
        let data_hex = hex::encode(wanted.data().as_slice());
        match found {
            Some(k) => {
                let same = !k.is_disabled()
                    && k.purpose() == wanted.purpose()
                    && k.security_level() == wanted.security_level()
                    && k.key_type() == wanted.key_type()
                    && k.contract_bounds() == wanted.contract_bounds()
                    && k.total_budget() == wanted.total_budget()
                    && k.expires_at() == wanted.expires_at();
                if !same {
                    return Err(ConnectError::Failed {
                        failure: ConnectFailure::KeyConflict,
                    });
                }
                out.session_ids.push(k.id());
                out.session_data.insert(k.id(), data_hex);
            }
            None => {
                out.session_ids.push(next);
                out.session_data.insert(next, data_hex);
                out.add.push(wanted);
                next += 1;
            }
        }
        out.slots.push(key);
    }
    for (binding, enc) in request.enc_bindings.iter().zip(&p.encryption) {
        match *enc {
            EncPlan::Omit => {}
            EncPlan::Share { key_id, .. } => {
                let on_chain = p
                    .identity
                    .public_keys()
                    .get(&key_id)
                    .map(|k| k.data().to_vec());
                for rotation in [Some(0), None] {
                    let key = app_encryption(&binding.contract, binding.purpose, rotation);
                    let Ok(public) = keys.public(owner, app, &key) else {
                        continue;
                    };
                    if on_chain.as_deref() == Some(&public[..]) {
                        out.slots.push(key);
                        out.encryption_ids.push(key_id);
                        break;
                    }
                }
            }
            EncPlan::Register => {
                let key = app_encryption(&binding.contract, binding.purpose, Some(0));
                let public = keys.public(owner, app, &key)?;
                out.add.push(plan::encryption_key(
                    &binding.contract,
                    binding.purpose,
                    next,
                    &public,
                ));
                out.proofs.insert(next, (key, public.to_vec()));
                out.encryption_ids.push(next);
                out.slots.push(key);
                next += 1;
            }
        }
    }
    if out.add.len() > MAX_ADDED {
        return Err(ConnectError::Unsupported);
    }
    Ok(out)
}

/// The previous grant's session keys for the same identity, app and
/// connection id that are still enabled: disabled in the same update. Only
/// a confirmed record's ids are known to hold its keys; a submitted one's
/// may since belong to another key.
fn superseded(
    records: &[ConnectRecord],
    identity: &Identity,
    request: &AppConnectRequest,
) -> Vec<KeyID> {
    let Some(connection) = request.connection_id.map(hex::encode) else {
        return Vec::new();
    };
    let (app, owner) = (b58(&plan::id(&request.app_id)), b58(&identity.id()));
    let this = hex::encode(request.request_hash());
    (records.iter())
        .filter(|r| {
            r.app_id == app
                && r.identity == owner
                && r.connection_id.as_deref() == Some(&connection)
                && r.request != this
                && r.state != RecordState::Submitted
        })
        .flat_map(|r| r.session_keys.iter().copied())
        .filter(|id| {
            identity
                .public_keys()
                .get(id)
                .is_some_and(|k| !k.is_disabled())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_DISABLED)
        .collect()
}

async fn records(
    session: &NetworkSession,
    wallet: WalletId,
) -> Result<Vec<ConnectRecord>, PlatformError> {
    let scope = dw_appdb::local_scope(&wallet.to_string());
    let raw = session
        .appdb_op(move |db| db.setting(&scope, RECORDS_SETTING))
        .await?;
    raw.map_or(Ok(Vec::new()), |r| {
        serde_json::from_str(&r).map_err(|_| PlatformError::Internal {
            detail: "app connect records".into(),
        })
    })
}

async fn put_record(
    session: &NetworkSession,
    wallet: WalletId,
    record: ConnectRecord,
) -> Result<(), PlatformError> {
    static WRITE: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(Default::default);
    let _write = WRITE.lock().await;
    let mut all = records(session, wallet).await?;
    all.retain(|r| r.request != record.request || r.identity != record.identity);
    all.push(record);
    let excess = all.len().saturating_sub(MAX_RECORDS);
    all.drain(..excess);
    let value = serde_json::to_string(&all).map_err(|e| PlatformError::Internal {
        detail: format!("app connect record: {e}"),
    })?;
    let scope = dw_appdb::local_scope(&wallet.to_string());
    session
        .appdb_op(move |db| db.set_setting_durable(&scope, RECORDS_SETTING, Some(&value)))
        .await?;
    Ok(())
}

/// Whether every key of `add` is on `identity`, enabled.
fn landed(identity: &Identity, add: &[IdentityPublicKey], disable: &[KeyID]) -> bool {
    let keys = identity.public_keys();
    add.iter().all(|k| {
        keys.get(&k.id())
            .is_some_and(|on| on.data() == k.data() && !on.is_disabled())
    }) && disable
        .iter()
        .all(|id| keys.get(id).is_none_or(|on| on.is_disabled()))
}

/// The groups as Platform has them now, against the approval's.
async fn same_groups(
    net: &impl ConnectNet,
    approved: &BTreeMap<Identifier, Option<GroupFacts>>,
) -> Result<(), ConnectError> {
    for (id, facts) in approved {
        if net.group(*id).await? != *facts {
            return Err(ConnectError::PlanChanged);
        }
    }
    Ok(())
}

/// The encryption keys sealed are still the identity's current keys.
async fn same_encryption(
    net: &impl ConnectNet,
    request: &AppConnectRequest,
    identity: Identifier,
    sealed: &[KeyID],
) -> Result<(), ConnectError> {
    let mut sealed = sealed.iter().peekable();
    for binding in &request.enc_bindings {
        let (purpose, _) = plan::enc_purpose(binding.purpose);
        let current = net
            .current_contract_key(identity, plan::id(&binding.contract), purpose)
            .await?;
        if let Some(current) = current {
            sealed.next_if_eq(&&current);
        }
    }
    if sealed.next().is_some() {
        return Err(ConnectError::PlanChanged);
    }
    Ok(())
}

pub(crate) async fn approve(
    net: &impl ConnectNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    link: &ConnectUri,
    choice: &ConnectChoice,
    grant: String,
) -> Result<ConnectDone, ConnectError> {
    let request = &link.request;
    let request_hash = request.request_hash();
    check_link(&session.network, link)?;
    let id = parse_identity(&choice.identity)?;
    let _refresh = session.after_spend(wallet_id, id);
    let locks = identity_locks(wallet_id, id);
    let _write = locks.write.lock().await;
    let p = prepare(net, session, wallet_id, link, choice).await?;
    check_grant(session, wallet_id, &grant, request_hash, p.digest).await?;
    let master = master_key(&p.identity).ok_or(PlatformError::from(
        super::errors::IdentityError::KeysMissing {
            purpose: super::identity::KeyPurpose::Authentication,
        },
    ))?;
    let held: Vec<HeldKey> = key_path(session.vault.network(), p.index, master.id())
        .map(|path| vec![(path, master.data().to_vec())])
        .unwrap_or_default();
    let lease = session
        .begin_lease_holding(wallet_id, FlowKind::AppConnect, &[grant], held)
        .await
        .map_err(PlatformError::from)?;
    if response_signer_key(&lease, &p.identity, p.index)?.is_none() {
        return Err(ConnectError::Failed {
            failure: ConnectFailure::NoResponseKey,
        });
    }
    let owner = p.identity.id().to_buffer();
    let update = write_permit(&lease, "app_connect_update", CONNECT_UPDATE_CREDITS)?;
    let keys = update.app_connect_keys().map_err(PlatformError::from)?;
    let plan = match key_plan(&keys, &p, request) {
        Ok(plan) => plan,
        Err(e) => {
            update.finish(false);
            return Err(e);
        }
    };
    let disable: Vec<KeyID> = superseded(&records(session, wallet_id).await?, &p.identity, request)
        .into_iter()
        .filter(|k| !plan.session_ids.contains(k))
        .collect();
    let mut record = ConnectRecord {
        request: hex::encode(request_hash),
        app_id: b58(&plan::id(&request.app_id)),
        label: request.label.clone(),
        identity: b58(&id),
        connection_id: request.connection_id.map(hex::encode),
        session_keys: plan.session_ids.clone(),
        encryption_keys: plan.encryption_ids.clone(),
        selection: p.selection.clone(),
        funding_limit: p.terms.funding_limit,
        expires_at_ms: p.terms.expires_at_ms,
        state: RecordState::Submitted,
        created_ms: super::balances::now_ms(),
        returns_to: link.callback.as_ref().map(|c| c.host().to_owned()),
        session_key_data: plan.session_data.clone(),
    };
    if plan.add.is_empty() && disable.is_empty() {
        update.finish(false);
    } else {
        if let Err(e) = same_groups(net, &p.groups).await {
            update.finish(false);
            return Err(e);
        }
        if let Err(e) = net.refresh(wallet, id).await {
            update.finish(false);
            return Err(e.into());
        }
        put_record(session, wallet_id, record.clone()).await?;
        let signer = ConnectUpdateSigner {
            master: update
                .identity_signer(&[p.index])
                .map_err(PlatformError::from)?
                .only_keys([master.id()]),
            app: keys,
            identity: owner,
            app_eph_pub: request.app_eph_pub,
            added: plan.proofs.clone(),
        };
        let written = leased(
            &lease,
            net.update_identity(wallet, id, plan.add.clone(), disable.clone(), &signer),
        )
        .await;
        drop(signer);
        update.finish(written.is_ok());
        if let Err(e) = &written {
            tracing::info!("app connect update: {}", e.code());
        }
        if !confirm(net, wallet, id, &plan.add, &disable).await {
            return Err(match written {
                Err(e) if refused_unsent(&e) => e.into(),
                _ => ConnectError::Failed {
                    failure: ConnectFailure::NotConfirmed,
                },
            });
        }
    }
    record.state = RecordState::Confirmed;
    put_record(session, wallet_id, record.clone()).await?;
    let response = {
        let seal = write_permit(&lease, "app_connect_seal", 0)?;
        let keys = seal.app_connect_keys().map_err(PlatformError::from)?;
        let sealed = keys.seal(owner, &request.app_eph_pub, &plan.slots);
        seal.finish(sealed.is_ok());
        sealed?
    };
    let fresh = net.refresh(wallet, id).await?;
    let doc_key = response_signer_key(&lease, &fresh, p.index)?.ok_or(ConnectError::Failed {
        failure: ConnectFailure::NoResponseKey,
    })?;
    let pending = PendingSend {
        lease,
        identity: id,
        groups: p.groups.clone(),
        index: p.index,
        doc_key,
        response,
    };
    let published = publish(net, wallet, &pending, request, &plan.encryption_ids).await;
    match published {
        Ok(()) => {
            record.state = RecordState::Published;
            if let Err(e) = put_record(session, wallet_id, record).await {
                tracing::warn!("app connect record after publish: {}", e.code());
            }
            Ok(ConnectDone {
                identity: b58(&id),
                keys_added: plan.add.iter().map(|k| k.id()).collect(),
            })
        }
        Err(ConnectError::Platform(e)) => Err(park(session, wallet_id, pending, &e)),
        Err(e) => Err(e),
    }
}

/// The key of `identity` the lease can sign the response with, if any.
fn response_signer_key(
    lease: &Lease,
    identity: &Identity,
    index: u32,
) -> Result<Option<KeyID>, PlatformError> {
    let probe = lease.call_permit("app_connect_probe", None)?;
    let found = probe
        .identity_signer(&[index])
        .map(|signer| response_key(identity, |k| signer.can_sign_with(k)));
    probe.finish(false);
    Ok(found?)
}

/// Keeps a response Platform did not take for "Retry sending".
fn park(
    session: &NetworkSession,
    wallet_id: WalletId,
    pending: PendingSend,
    e: &PlatformError,
) -> ConnectError {
    tracing::info!("app connect response: {}", e.code());
    session.platform.app_connect.put(wallet_id, pending);
    ConnectError::SendFailed
}

/// Reads the identity until every key of `add` is on it (three reads, 2 s
/// then 4 s apart).
async fn confirm(
    net: &impl ConnectNet,
    wallet: &PlatformWallet,
    id: Identifier,
    add: &[IdentityPublicKey],
    disable: &[KeyID],
) -> bool {
    for wait in CONFIRM_WAITS_MS {
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        if net
            .refresh(wallet, id)
            .await
            .is_ok_and(|i| landed(&i, add, disable))
        {
            return true;
        }
    }
    false
}

/// Sends the sealed response once the approved groups and the sealed
/// encryption keys are still Platform's: a change after the keys were added
/// is `connect.failed{scope_changed}`.
async fn publish(
    net: &impl ConnectNet,
    wallet: &PlatformWallet,
    pending: &PendingSend,
    request: &AppConnectRequest,
    encryption: &[KeyID],
) -> Result<(), ConnectError> {
    let scope_changed = |e| match e {
        ConnectError::PlanChanged => ConnectError::Failed {
            failure: ConnectFailure::ScopeChanged,
        },
        e => e,
    };
    same_groups(net, &pending.groups)
        .await
        .map_err(scope_changed)?;
    same_encryption(net, request, pending.identity, encryption)
        .await
        .map_err(scope_changed)?;
    let permit = write_permit(
        &pending.lease,
        "app_connect_response",
        CONNECT_RESPONSE_CREDITS,
    )?;
    let signer = permit
        .identity_signer(&[pending.index])
        .map_err(PlatformError::from)?
        .only_keys([pending.doc_key]);
    let written = leased(
        &pending.lease,
        net.publish(wallet, pending.identity, &pending.response, &signer),
    )
    .await;
    drop(signer);
    permit.finish(written.is_ok());
    Ok(written?)
}

pub(crate) async fn retry_send(
    net: &impl ConnectNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    link: &ConnectUri,
) -> Result<ConnectDone, ConnectError> {
    check_link(&session.network, link)?;
    let request = &link.request;
    let nothing = || ConnectError::Failed {
        failure: ConnectFailure::NothingToSend,
    };
    let pending = (session.platform.app_connect)
        .take(wallet_id, request.request_hash())
        .ok_or_else(nothing)?;
    let _refresh = session.after_spend(wallet_id, pending.identity);
    let locks = identity_locks(wallet_id, pending.identity);
    let _write = locks.write.lock().await;
    let sent = async {
        let record = (records(session, wallet_id).await?.into_iter()).find(|r| {
            r.request == hex::encode(request.request_hash()) && r.identity == b58(&pending.identity)
        });
        let record = record.ok_or_else(nothing)?;
        if !net
            .response_exists(request.request_hash(), pending.identity)
            .await?
        {
            publish(net, wallet, &pending, request, &record.encryption_keys).await?;
        }
        Ok(record)
    }
    .await;
    match sent {
        Ok(mut record) => {
            record.state = RecordState::Published;
            if let Err(e) = put_record(session, wallet_id, record.clone()).await {
                tracing::warn!("app connect record after publish: {}", e.code());
            }
            Ok(ConnectDone {
                identity: record.identity,
                keys_added: Vec::new(),
            })
        }
        Err(ConnectError::Platform(
            PlatformError::LeaseRevoked { .. }
            | PlatformError::LeaseExpired
            | PlatformError::Cancelled
            | PlatformError::NeedsGrant { .. }
            | PlatformError::GrantExceeded { .. }
            | PlatformError::GrantInvalid,
        )) => Err(nothing()),
        Err(ConnectError::Platform(e)) => Err(park(session, wallet_id, pending, &e)),
        Err(e) => Err(e),
    }
}

fn check_enabled(network: &DashNetwork) -> Result<(), ConnectError> {
    if !enabled_on(network) {
        return Err(ConnectError::MainnetOff);
    }
    Ok(())
}

fn parse_request_hash(text: &str) -> Result<[u8; 20], ConnectError> {
    let invalid = || {
        ConnectError::from(PlatformError::InvalidArgument {
            detail: "request".into(),
        })
    };
    let bytes = hex::decode(text).map_err(|_| invalid())?;
    if bytes.len() != 20 || text != hex::encode(&bytes) {
        return Err(invalid());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn wallet_identity_index(
    session: &NetworkSession,
    wallet: WalletId,
    id: Identifier,
) -> Result<u32, ConnectError> {
    let target = b58(&id);
    let summaries = session
        .identity_summaries(wallet)
        .map_err(PlatformError::from)?;
    summaries
        .into_iter()
        .find(|s| s.identity == target)
        .map(|s| s.index)
        .ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound).into())
}

fn is_forbidden_dashpay_bounds(bounds: &ContractBounds) -> bool {
    match bounds {
        ContractBounds::SingleContract { id } => *id == dashpay_contract::ID,
        ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        } => *id == dashpay_contract::ID && document_type_name != PROFILE_TYPE,
        ContractBounds::ContractGroup { .. } => false,
    }
}

/// Verifies that `key` on Platform is an enabled app-added AUTHENTICATION
/// session key belonging to `record`:
/// - its key id is in `record.session_keys`;
/// - `record.session_key_data` holds the expected key digest AND matches
///   `key.data()` (older grants without key data are refused, never matched
///   by purpose/level alone);
/// - `key.purpose() == Purpose::AUTHENTICATION`;
/// - `key.security_level()` is `HIGH` or `CRITICAL` (never `MASTER`);
/// - `key.key_type() == KeyType::ECDSA_HASH160`;
/// - `key.contract_bounds()` is `Some(...)` (never unbounded, including
///   unbounded `CRITICAL`);
/// - `key.has_limits()`;
/// - not a whole-contract DashPay key (only `dashpay/profile` doc-type is
///   allowed on the DashPay contract).
fn check_disconnect_key(
    record: &ConnectRecord,
    key: &IdentityPublicKey,
) -> Result<(), ConnectError> {
    let unverified = || ConnectError::Failed {
        failure: ConnectFailure::UnverifiedKey,
    };
    if !record.session_keys.contains(&key.id()) {
        return Err(unverified());
    }
    let Some(expected_hex) = record.session_key_data.get(&key.id()) else {
        return Err(unverified());
    };
    let Ok(expected_bytes) = hex::decode(expected_hex) else {
        return Err(unverified());
    };
    if expected_bytes.is_empty() || key.data().as_slice() != expected_bytes.as_slice() {
        return Err(unverified());
    }
    if key.purpose() != Purpose::AUTHENTICATION {
        return Err(unverified());
    }
    if !matches!(
        key.security_level(),
        SecurityLevel::HIGH | SecurityLevel::CRITICAL
    ) {
        return Err(unverified());
    }
    if key.key_type() != KeyType::ECDSA_HASH160 {
        return Err(unverified());
    }
    let Some(bounds) = key.contract_bounds() else {
        return Err(unverified());
    };
    if !key.has_limits() {
        return Err(unverified());
    }
    if is_forbidden_dashpay_bounds(bounds) {
        return Err(unverified());
    }
    Ok(())
}

/// The verified still-enabled session key ids of `record` on `identity`.
/// Any still-enabled key id in `record.session_keys` that fails verification
/// refuses the whole disconnect rather than disabling partially or guessing.
fn verified_disconnect_keys(
    record: &ConnectRecord,
    identity: &Identity,
) -> Result<Vec<KeyID>, ConnectError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for &key_id in &record.session_keys {
        if !seen.insert(key_id) {
            continue;
        }
        let Some(key) = identity.public_keys().get(&key_id) else {
            continue;
        };
        if key.is_disabled() {
            continue;
        }
        check_disconnect_key(record, key)?;
        out.push(key_id);
    }
    if out.len() > MAX_DISABLED {
        return Err(ConnectError::Unsupported);
    }
    Ok(out)
}

/// Checks the live Platform fee against `DISCONNECT_FEE_BOUND`, refusing
/// rather than clamping when the live fee exceeds the bound.
async fn live_disconnect_fee(net: &impl ConnectNet, key_count: usize) -> Result<u64, ConnectError> {
    if key_count == 0 {
        return Ok(0);
    }
    let fee = net.disconnect_fee(key_count).await?;
    if fee > DISCONNECT_FEE_BOUND {
        return Err(ConnectError::Failed {
            failure: ConnectFailure::FeeTooHigh,
        });
    }
    Ok(fee)
}

/// Refuses a grant that is not this wallet's `AppDisconnect` grant for
/// `request`, returning its `max_credits` cap before redemption.
async fn check_disconnect_grant(
    session: &NetworkSession,
    wallet: WalletId,
    grant: &str,
    request: [u8; 20],
) -> Result<u64, ConnectError> {
    let (vault, g) = (session.vault.clone(), grant.to_owned());
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&g, GrantKind::AppDisconnect, Some(&wallet.0))
    })
    .await
    .map_err(|e| PlatformError::from(EngineError::from(e)))?
    .map_err(PlatformError::from)?;
    let GrantPurpose::AppDisconnect {
        max_credits,
        request: granted,
    } = purpose
    else {
        return Err(PlatformError::GrantInvalid.into());
    };
    if granted != request {
        return Err(PlatformError::GrantInvalid.into());
    }
    Ok(max_credits)
}

async fn resolve_key_scope(
    net: &impl ConnectNet,
    bounds: Option<&ContractBounds>,
    fallback_app_id: &str,
    contracts: &mut BTreeMap<Identifier, Option<ContractFacts>>,
    groups: &mut BTreeMap<Identifier, Option<GroupFacts>>,
) -> Result<ConnectScope, ConnectError> {
    match bounds {
        Some(ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        }) if *id == dashpay_contract::ID && document_type_name == PROFILE_TYPE => {
            Ok(ConnectScope::Profile)
        }
        Some(ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        }) => Ok(ConnectScope::DocumentType {
            contract: b58(id),
            document_type: document_type_name.clone(),
        }),
        Some(ContractBounds::SingleContract { id }) => {
            if let std::collections::btree_map::Entry::Vacant(e) = contracts.entry(*id) {
                e.insert(net.contract(*id).await?);
            }
            let tokens = contracts
                .get(id)
                .cloned()
                .flatten()
                .is_none_or(|f| f.has_tokens || !f.readonly);
            Ok(ConnectScope::Contract {
                contract: b58(id),
                tokens,
            })
        }
        Some(ContractBounds::ContractGroup { id }) => {
            if let std::collections::btree_map::Entry::Vacant(e) = groups.entry(*id) {
                e.insert(net.group(*id).await?);
            }
            let facts = groups.get(id).cloned().flatten().unwrap_or_default();
            Ok(ConnectScope::Group(ConnectGroup {
                group: b58(id),
                owner: b58(&facts.owner),
                admins: facts.admins.iter().map(b58).collect(),
                contracts: facts.contracts.iter().map(b58).collect(),
                document_types: facts
                    .document_types
                    .iter()
                    .map(|(c, t)| ConnectDocumentType {
                        contract: b58(c),
                        document_type: t.clone(),
                    })
                    .collect(),
                tokens: facts
                    .tokens
                    .iter()
                    .map(|(c, p)| ConnectToken {
                        contract: b58(c),
                        position: *p,
                    })
                    .collect(),
            }))
        }
        None => Ok(ConnectScope::Contract {
            contract: fallback_app_id.to_owned(),
            tokens: true,
        }),
    }
}

pub(crate) async fn list(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    identity: &str,
) -> Result<ConnectedApps, ConnectError> {
    check_enabled(&session.network)?;
    let id = parse_identity(identity)?;
    let _index = wallet_identity_index(session, wallet, id)?;
    let owner = b58(&id);
    let mut recs: Vec<ConnectRecord> = records(session, wallet)
        .await?
        .into_iter()
        .filter(|r| r.identity == owner && r.state != RecordState::Disconnected)
        .collect();
    recs.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
    if recs.is_empty() {
        return Ok(ConnectedApps {
            identity: owner,
            apps: Vec::new(),
            block_time_ms: 0,
        });
    }
    let (on_chain, block_time_ms) = net.identity(id).await?;
    let on_chain =
        on_chain.ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound))?;
    let mut contracts = BTreeMap::new();
    let mut groups = BTreeMap::new();
    let mut apps = Vec::new();
    for r in recs {
        // An older record with no key digests whose update never confirmed may
        // name ids another key later took: skip it.
        if r.state == RecordState::Submitted && r.session_key_data.is_empty() {
            continue;
        }
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();
        for &key_id in &r.session_keys {
            if !seen.insert(key_id) {
                continue;
            }
            let Some(k) = on_chain.public_keys().get(&key_id) else {
                continue;
            };
            if k.is_disabled() {
                continue;
            }
            if let Some(expected_hex) = r.session_key_data.get(&key_id)
                && hex::encode(k.data().as_slice()) != *expected_hex
            {
                continue;
            }
            let level = match k.security_level() {
                SecurityLevel::CRITICAL => ConnectLevel::Critical,
                _ => ConnectLevel::High,
            };
            let scope = resolve_key_scope(
                net,
                k.contract_bounds(),
                &r.app_id,
                &mut contracts,
                &mut groups,
            )
            .await?;
            keys.push(ConnectedAppKey {
                key_id,
                level,
                scope,
                budget: k.total_budget(),
                expires_at_ms: k.expires_at(),
            });
        }
        if keys.is_empty() {
            continue;
        }
        let expires_at_ms = keys
            .iter()
            .filter_map(|k| k.expires_at_ms)
            .min()
            .unwrap_or(r.expires_at_ms);
        apps.push(ConnectedApp {
            request: r.request,
            app_id: r.app_id,
            label: r.label,
            returns_to: r.returns_to,
            identity: r.identity,
            keys,
            funding_limit: r.funding_limit,
            expires_at_ms,
            approved_at_ms: r.created_ms,
        });
    }
    Ok(ConnectedApps {
        identity: owner,
        apps,
        block_time_ms,
    })
}

pub(crate) async fn disconnect_quote(
    net: &impl ConnectNet,
    session: &NetworkSession,
    wallet: WalletId,
    identity: &str,
    request: &str,
) -> Result<DisconnectQuote, ConnectError> {
    check_enabled(&session.network)?;
    let id = parse_identity(identity)?;
    let request_hash = parse_request_hash(request)?;
    let _index = wallet_identity_index(session, wallet, id)?;
    let owner = b58(&id);
    let record = records(session, wallet)
        .await?
        .into_iter()
        .find(|r| r.identity == owner && r.request == request)
        .ok_or_else(|| PlatformError::InvalidArgument {
            detail: "unknown app connection".into(),
        })?;
    let (on_chain, _) = net.identity(id).await?;
    let on_chain =
        on_chain.ok_or_else(|| PlatformError::from(super::errors::IdentityError::NotFound))?;
    let keys = verified_disconnect_keys(&record, &on_chain)?;
    let fee = live_disconnect_fee(net, keys.len()).await?;
    if on_chain.balance() < fee {
        return Err(ConnectError::Credits {
            needed: fee,
            available: on_chain.balance(),
        });
    }
    Ok(DisconnectQuote {
        request: request_hash,
        identity: owner,
        keys,
        max_credits: fee,
        fee,
        balance: on_chain.balance(),
    })
}

pub(crate) async fn disconnect(
    net: &impl ConnectNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    identity: &str,
    request: &str,
    grant: String,
) -> Result<DisconnectDone, ConnectError> {
    check_enabled(&session.network)?;
    let id = parse_identity(identity)?;
    let request_hash = parse_request_hash(request)?;
    let index = wallet_identity_index(session, wallet_id, id)?;
    let owner = b58(&id);
    let _refresh = session.after_spend(wallet_id, id);
    let locks = identity_locks(wallet_id, id);
    let _write = locks.write.lock().await;
    let mut record = records(session, wallet_id)
        .await?
        .into_iter()
        .find(|r| r.identity == owner && r.request == request)
        .ok_or_else(|| PlatformError::InvalidArgument {
            detail: "unknown app connection".into(),
        })?;
    let granted_max_credits =
        check_disconnect_grant(session, wallet_id, &grant, request_hash).await?;
    let fresh = net.refresh(wallet, id).await?;
    let disable = verified_disconnect_keys(&record, &fresh)?;
    if disable.is_empty() {
        let _ = session.platform.app_connect.take(wallet_id, request_hash);
        if record.state != RecordState::Disconnected {
            record.state = RecordState::Disconnected;
            if let Err(e) = put_record(session, wallet_id, record).await {
                tracing::warn!("app connect record after disconnect: {}", e.code());
            }
        }
        return Ok(DisconnectDone {
            identity: owner,
            keys_disabled: Vec::new(),
        });
    }
    let fee = live_disconnect_fee(net, disable.len()).await?;
    if fee > granted_max_credits {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: fee,
            remaining: granted_max_credits,
        }
        .into());
    }
    if fresh.balance() < fee {
        return Err(ConnectError::Credits {
            needed: fee,
            available: fresh.balance(),
        });
    }
    let master = master_key(&fresh).ok_or(PlatformError::from(
        super::errors::IdentityError::KeysMissing {
            purpose: super::identity::KeyPurpose::Authentication,
        },
    ))?;
    // Pre-sign re-check on every key ID going into the IdentityUpdate.
    for &key_id in &disable {
        if key_id == master.id() {
            return Err(ConnectError::Failed {
                failure: ConnectFailure::UnverifiedKey,
            });
        }
        let key = fresh
            .public_keys()
            .get(&key_id)
            .ok_or(ConnectError::Failed {
                failure: ConnectFailure::UnverifiedKey,
            })?;
        check_disconnect_key(&record, key)?;
    }
    let held: Vec<HeldKey> = key_path(session.vault.network(), index, master.id())
        .map(|path| vec![(path, master.data().to_vec())])
        .unwrap_or_default();
    let lease = session
        .begin_lease_holding(wallet_id, FlowKind::AppDisconnect, &[grant], held)
        .await
        .map_err(PlatformError::from)?;
    let permit = write_permit(&lease, "app_connect_disconnect", fee)?;
    let signer = permit
        .identity_signer(&[index])
        .map_err(PlatformError::from)?
        .only_keys([master.id()]);
    let written = leased(
        &lease,
        net.disable_keys(wallet, id, disable.clone(), &signer),
    )
    .await;
    drop(signer);
    permit.finish(written.is_ok());
    drop(lease);
    if let Err(e) = &written {
        tracing::info!("app connect disconnect: {}", e.code());
    }
    if !confirm(net, wallet, id, &[], &disable).await {
        return Err(match written {
            Err(e) if refused_unsent(&e) => e.into(),
            _ => ConnectError::Failed {
                failure: ConnectFailure::NotConfirmed,
            },
        });
    }
    let _ = session.platform.app_connect.take(wallet_id, request_hash);
    record.state = RecordState::Disconnected;
    if let Err(e) = put_record(session, wallet_id, record).await {
        tracing::warn!("app connect record after disconnect: {}", e.code());
    }
    Ok(DisconnectDone {
        identity: owner,
        keys_disabled: disable,
    })
}

impl DashPay {
    /// The approval sheet of an App Connect request (DASHPAY F24): the
    /// app, the wallet's identities that may sign in, each binding with a
    /// group's owner, admins and members as Platform proves them now, and
    /// the default terms. Refused, with nothing read, for another network
    /// (`connect.network`), on mainnet in this build
    /// (`connect.mainnet_off`) and for bindings this wallet does not grant
    /// (`connect.unsupported`); after the reads for an old protocol
    /// (`connect.protocol`), a wrong owner (`connect.owner`), a missing
    /// contract or group (`connect.invalid`), no identity with a username
    /// (`connect.no_identity`), a hint this wallet does not hold
    /// (`connect.hint`) and an old request (`connect.stale`). Derives and
    /// signs nothing.
    pub async fn app_connect_sheet(&self, link: ConnectUri) -> Result<ConnectSheet, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            sheet(&SdkConnect(manager.sdk_arc()), &session, wallet_id, &link).await
        })
        .await
    }

    /// The cost of approving `choice` and the grant it needs: an
    /// `AppConnect` grant naming the quote's `request` and `plan`, the
    /// digest of everything read for it. The sheet's refusals, and
    /// `connect.answered` for a request this identity answered,
    /// `connect.credits` for a balance below the quote. Derives and signs
    /// nothing.
    pub async fn app_connect_quote(
        &self,
        link: ConnectUri,
        choice: ConnectChoice,
    ) -> Result<ConnectQuote, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            quote(
                &SdkConnect(manager.sdk_arc()),
                &session,
                wallet_id,
                &link,
                &choice,
            )
            .await
        })
        .await
    }

    /// Approves `choice` under `grant`: reads everything the quote read
    /// again and refuses `connect.plan_changed` when its digest is not the
    /// grant's; adds the session keys (and an encryption key the identity
    /// lacks) in one MASTER-signed IdentityUpdate, disabling the keys of
    /// the same connection's previous grant; waits for Platform to show
    /// every key; seals the keys to the app and publishes the response,
    /// checking the groups again before the update and before the
    /// response. A response that could not be published is
    /// `connect.send_failed`, kept for [`Self::app_connect_retry_send`].
    pub async fn app_connect_approve(
        &self,
        link: ConnectUri,
        choice: ConnectChoice,
        grant: String,
    ) -> Result<ConnectDone, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkConnect(manager.sdk_arc());
            approve(&net, &session, &wallet, wallet_id, &link, &choice, grant).await
        })
        .await
    }

    /// Publishes the response an approval of `link` sealed and could not
    /// send, under that approval's lease (a lock ends it:
    /// `connect.failed{nothing_to_send}`).
    pub async fn app_connect_retry_send(
        &self,
        link: ConnectUri,
    ) -> Result<ConnectDone, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkConnect(manager.sdk_arc());
            retry_send(&net, &session, &wallet, wallet_id, &link).await
        })
        .await
    }

    /// Lists each app this wallet approved for `identity` that still has
    /// enabled session keys on Platform, combining local approval records
    /// with Platform's proved identity keys and live contract/group scopes.
    /// Refused on mainnet (`connect.mainnet_off`).
    pub async fn app_connect_list(&self, identity: String) -> Result<ConnectedApps, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            list(
                &SdkConnect(manager.sdk_arc()),
                &session,
                wallet_id,
                &identity,
            )
            .await
        })
        .await
    }

    /// The live Platform credit fee and `AppDisconnect` grant to disconnect
    /// one approved app (`request` hex) on `identity`. Verifies every key id
    /// and its public key data against Platform and refuses when a key is
    /// unverified (`connect.failed{unverified_key}`), the live fee exceeds the
    /// fee bound (`connect.failed{fee_too_high}`), credits are short
    /// (`connect.credits`), or on mainnet (`connect.mainnet_off`).
    pub async fn app_connect_disconnect_quote(
        &self,
        identity: String,
        request: String,
    ) -> Result<DisconnectQuote, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, _| async move {
            disconnect_quote(
                &SdkConnect(manager.sdk_arc()),
                &session,
                wallet_id,
                &identity,
                &request,
            )
            .await
        })
        .await
    }

    /// Disables one approved app's verified AUTHENTICATION session keys on
    /// `identity` in a MASTER-signed IdentityUpdate under `grant` (an
    /// `AppDisconnect` grant bound to `request`). Safe to call twice: refreshes
    /// Platform first under the identity write lock and succeeds without
    /// spending credits or sending an update if the app's keys are already
    /// disabled. Refused on mainnet (`connect.mainnet_off`).
    pub async fn app_connect_disconnect(
        &self,
        identity: String,
        request: String,
        grant: String,
    ) -> Result<DisconnectDone, ConnectError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkConnect(manager.sdk_arc());
            disconnect(
                &net, &session, &wallet, wallet_id, &identity, &request, grant,
            )
            .await
        })
        .await
    }
}

#[cfg(test)]
#[path = "app_connect_tests.rs"]
mod tests;
