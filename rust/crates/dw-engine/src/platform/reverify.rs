//! Re-fetching unverified Platform data once SPV has synced (DASHPAY §2.2,
//! DEC-125; E0-10b design §5). Once the fallback latch is set, an entity the
//! read models show is unverified until one of its fetches verifies against
//! SPV-held quorum keys. This task re-fetches the identities, profiles,
//! DPNS labels and contact requests the library holds, and the contacts'
//! profiles and shown labels, each in its own trust scope, and records
//! those whose fetch verified and matches what the library holds. A
//! verified fetch that differs records nothing: the library's own sync
//! replaces the data, and a later run records it. The record is written
//! under the wallet manager's read lock, and only for an entity the library
//! still holds unchanged then.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dashcore::hashes::{Hash, sha256};
use dpp::document::{Document, DocumentV0Getters};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyID};
use dpp::platform_value::Value;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::serialization::PlatformSerializable;
use dpp::system_data_contracts::{SystemDataContract, load_system_data_contract};
use dpp::util::strings::convert_to_homograph_safe_chars;
use platform_wallet::key_wallet_manager::WalletManager;
use platform_wallet::wallet::identity::DashPayProfile;
use platform_wallet::{ContactRequest, ManagedIdentity, PlatformWalletInfo};

use super::errors::PlatformError;
use super::provenance::{Entity, Provenance, contact_request_key, kind};
use super::recovery::owned_names;
use super::trust::{self, BoxFuture};
use crate::{EngineError, NetworkSession, WalletId};

/// The most `contactRequest` documents read for one sender and recipient.
const CONTACT_REQUESTS_PER_PAIR: u32 = 100;
/// How often the task looks again after a run.
const RETRY_EVERY: Duration = Duration::from_secs(600);
/// platform-wallet drops an avatar URL it would not fetch (`profile.rs`,
/// `is_valid_avatar_url`); the comparison does the same.
const MAX_AVATAR_URL_LEN: usize = 2048;

/// The profile fields the read models show (`owned_identities`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ShownProfile {
    pub(super) display_name: Option<String>,
    pub(super) public_message: Option<String>,
    pub(super) avatar_url: Option<String>,
    pub(super) avatar_hash: Option<[u8; 32]>,
    pub(super) avatar_fingerprint: Option<[u8; 8]>,
}

impl ShownProfile {
    pub(super) fn of(p: &DashPayProfile) -> Self {
        Self {
            display_name: p.display_name.clone(),
            public_message: p.public_message.clone().or_else(|| p.bio.clone()),
            avatar_url: p.avatar_url.clone(),
            avatar_hash: p.avatar_hash,
            avatar_fingerprint: p.avatar_fingerprint,
        }
    }

    /// A `profile` document's fields as platform-wallet caches them. `None`
    /// when no field is set: the library stores no profile then.
    fn from_document(doc: &Document) -> Option<Self> {
        let props = doc.properties();
        let text = |key: &str| {
            props
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        };
        let bytes = |key: &str| props.get(key).and_then(|v| v.as_bytes_slice().ok());
        let shown = Self {
            display_name: text("displayName"),
            public_message: text("publicMessage"),
            avatar_url: text("avatarUrl")
                .filter(|u| u.len() <= MAX_AVATAR_URL_LEN && u.starts_with("https://")),
            avatar_hash: bytes("avatarHash").and_then(|b| b.try_into().ok()),
            avatar_fingerprint: bytes("avatarFingerprint").and_then(|b| b.try_into().ok()),
        };
        (shown != Self::default()).then_some(shown)
    }
}

/// An entity the read models show as unverified, with what the library
/// holds for it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Held {
    Identity {
        id: Identifier,
        keys: BTreeMap<KeyID, IdentityPublicKey>,
    },
    Profile {
        id: Identifier,
        shown: ShownProfile,
    },
    /// A DPNS name shown as `spelling` for `owner`; `label` is its
    /// homograph-normalized form, the record and query key. The digest binds
    /// the spelling and the owner (Sol r1 R1: `Alice` and `Al1ce` share a
    /// key, not a digest). Build it with [`Held::label`].
    Label {
        label: String,
        spelling: String,
        owner: Identifier,
    },
    /// A sent or received request, as the library holds it.
    ContactRequest(ContactRequest),
}

impl Held {
    /// `spelling` shown as `owner`'s DPNS name.
    pub(super) fn label(spelling: &str, owner: Identifier) -> Self {
        Self::Label {
            label: convert_to_homograph_safe_chars(spelling),
            spelling: spelling.to_string(),
            owner,
        }
    }

    /// The entity as provenance sees it (DEC-183): its record and the digest
    /// of this content.
    pub(super) fn entity(&self) -> Entity {
        let (kind, key) = self.record();
        Entity {
            kind,
            key,
            digest: self.digest(kind),
        }
    }

    /// The content digest, `v2:` and the hex SHA-256 of [`Canon`] bytes.
    /// `None` if an identity key does not serialize.
    fn digest(&self, kind: &str) -> Option<String> {
        let mut c = Canon::new(kind);
        match self {
            Self::Identity { id, keys } => {
                c.id(id).u64(keys.len() as u64);
                for (key_id, key) in keys {
                    c.u64((*key_id).into())
                        .bytes(&key.serialize_to_bytes().ok()?);
                }
            }
            Self::Profile { id, shown } => {
                // Every field named: a new one fails to build until encoded.
                let ShownProfile {
                    display_name,
                    public_message,
                    avatar_url,
                    avatar_hash,
                    avatar_fingerprint,
                } = shown;
                c.id(id)
                    .opt(display_name.as_deref().map(str::as_bytes))
                    .opt(public_message.as_deref().map(str::as_bytes))
                    .opt(avatar_url.as_deref().map(str::as_bytes))
                    .opt(avatar_hash.as_ref().map(|h| &h[..]))
                    .opt(avatar_fingerprint.as_ref().map(|f| &f[..]));
            }
            Self::Label {
                label,
                spelling,
                owner,
            } => {
                c.bytes(label.as_bytes())
                    .bytes(spelling.as_bytes())
                    .id(owner);
            }
            Self::ContactRequest(r) => {
                let ContactRequest {
                    sender_id,
                    recipient_id,
                    sender_key_index,
                    recipient_key_index,
                    account_reference,
                    encrypted_account_label,
                    encrypted_public_key,
                    auto_accept_proof,
                    core_height_created_at,
                    created_at,
                } = r;
                c.id(sender_id)
                    .id(recipient_id)
                    .u64((*sender_key_index).into())
                    .u64((*recipient_key_index).into())
                    .u64((*account_reference).into())
                    .opt(encrypted_account_label.as_deref())
                    .bytes(encrypted_public_key)
                    .opt(auto_accept_proof.as_deref())
                    .u64((*core_height_created_at).into())
                    .u64(*created_at);
            }
        }
        Some(c.finish())
    }

    /// The `dp_trust_verified` record a verified match writes.
    fn record(&self) -> (&'static str, String) {
        match self {
            Self::Identity { id, .. } => (kind::IDENTITY, id.to_string(Encoding::Base58)),
            Self::Profile { id, .. } => (kind::PROFILE, id.to_string(Encoding::Base58)),
            Self::Label { label, .. } => (kind::DPNS_LABEL, label.clone()),
            Self::ContactRequest(r) => (
                kind::CONTACT_REQUEST,
                contact_request_key(&r.sender_id, &r.recipient_id, r.created_at),
            ),
        }
    }
}

/// The digest encoding (DEC-183); a change to [`Canon`] or to a field set
/// takes the next version, and records of the old one read as unverified.
/// Identity keys are encoded by dpp: a dpp release that changes their
/// bincode layout has the same effect (fails closed until re-fetched).
/// v2 (Sol r1 R1): a DPNS label's spelling joined its fields; every v1
/// record (label records bound only the normalized label) reads as
/// unverified until a re-fetch verifies it again.
const DIGEST_VERSION: &str = "v2";

/// The canonical bytes of an entity: a domain tag and the kind, then each
/// field in order. Byte strings are length-prefixed (u64 LE), integers u64
/// LE, an option is a 0 or 1 byte then its value. Identity keys are their
/// platform serialization (unversioned bincode).
struct Canon(Vec<u8>);

impl Canon {
    fn new(kind: &str) -> Self {
        let mut c = Self(Vec::new());
        c.bytes(b"dw-trust-digest")
            .bytes(DIGEST_VERSION.as_bytes())
            .bytes(kind.as_bytes());
        c
    }

    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u64(b.len() as u64);
        self.0.extend_from_slice(b);
        self
    }

    fn id(&mut self, id: &Identifier) -> &mut Self {
        self.bytes(&id.to_buffer())
    }

    fn opt(&mut self, b: Option<&[u8]>) -> &mut Self {
        match b {
            Some(b) => {
                self.0.push(1);
                self.bytes(b)
            }
            None => {
                self.0.push(0);
                self
            }
        }
    }

    fn finish(&self) -> String {
        let digest = sha256::Hash::hash(&self.0).to_byte_array();
        format!("{DIGEST_VERSION}:{}", hex::encode(digest))
    }
}

/// Platform's current answers, proof-verified by the SDK.
pub(super) trait Refetcher: Send + Sync {
    fn identity_keys(
        &self,
        id: Identifier,
    ) -> BoxFuture<Result<Option<BTreeMap<KeyID, IdentityPublicKey>>, PlatformError>>;
    fn profile(&self, id: Identifier) -> BoxFuture<Result<Option<ShownProfile>, PlatformError>>;
    /// The DPNS domain of the homograph-normalized `label`: its `label` as
    /// spelled and its `records.identity`.
    fn dpns_domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>>;
    /// The `contactRequest` documents from `sender` to `recipient` created at
    /// `created_at`, read as the library reads them.
    fn contact_requests(
        &self,
        sender: Identifier,
        recipient: Identifier,
        created_at: u64,
    ) -> BoxFuture<Result<Vec<ContactRequest>, PlatformError>>;
}

/// Re-fetches one entity in its own scope: whether the fetch used only
/// SPV-verified quorum keys and matches what the library holds.
pub(super) async fn reverify(h: &Held, fetch: &dyn Refetcher) -> bool {
    fn ok<T>(r: Result<Option<T>, PlatformError>) -> Option<T> {
        r.inspect_err(|e| tracing::debug!(error = %e, "re-fetch failed"))
            .ok()
            .flatten()
    }
    let (matches, tally) = trust::scoped(async {
        match h {
            Held::Identity { id, keys } => {
                ok(fetch.identity_keys(*id).await).as_ref() == Some(keys)
            }
            Held::Profile { id, shown } => ok(fetch.profile(*id).await).as_ref() == Some(shown),
            Held::Label {
                label,
                spelling,
                owner,
            } => ok(fetch.dpns_domain(label.clone()).await) == Some((spelling.clone(), *owner)),
            Held::ContactRequest(r) => {
                let fetched = fetch.contact_requests(r.sender_id, r.recipient_id, r.created_at);
                ok(fetched.await.map(Some)).is_some_and(|all| all.contains(r))
            }
        }
    })
    .await;
    matches && tally.verified()
}

/// The records to write: entities whose fetch verified and that the
/// library still holds, unchanged and unverified (`current`).
pub(super) fn confirmed(verified: &[&Held], current: &[Held]) -> Vec<Entity> {
    verified
        .iter()
        .filter(|h| current.contains(h))
        .map(|h| h.entity())
        .collect()
}

/// What an identity summary shows of `m` (`recovery::summaries`): the
/// identity and its keys, its profile, the names it owns.
pub(super) fn held_of(m: &ManagedIdentity, info: &PlatformWalletInfo) -> Vec<Held> {
    let identity = m.identity.id();
    let mut held = vec![Held::Identity {
        id: identity,
        keys: m.identity.public_keys().clone(),
    }];
    if let Some(p) = &m.dashpay().profile {
        held.push(Held::Profile {
            id: identity,
            shown: ShownProfile::of(p),
        });
    }
    for (label, _) in owned_names(identity, &m.dpns_names, &info.dpns_name_states) {
        held.push(Held::label(&label, identity));
    }
    held
}

/// The contact requests of `m`: sent, received and established.
fn requests_of(m: &ManagedIdentity) -> impl Iterator<Item = Held> + '_ {
    let dp = m.dashpay();
    let established = dp.established_contacts().values();
    (dp.sent_contact_requests().values())
        .chain(dp.incoming_contact_requests().values())
        .chain(established.flat_map(|c| [&c.outgoing_request, &c.incoming_request]))
        .map(|r| Held::ContactRequest(r.clone()))
}

/// The cached profiles of `m`'s contacts (and request senders). A
/// confirmed-absent profile shows nothing and needs no record. A contact's
/// identity has no class of its own: the library holds no keys for it, and
/// the verified contact request binds the key indices and the encrypted
/// xpub its channel uses (E0-10b design §5).
fn contact_profiles_of(m: &ManagedIdentity) -> impl Iterator<Item = Held> + '_ {
    (m.dashpay().contact_profiles.iter()).filter_map(|(id, entry)| {
        Some(Held::Profile {
            id: *id,
            shown: ShownProfile::of(entry.profile.as_ref()?),
        })
    })
}

/// The entities of `info` the read models would show as unverified, each
/// content once. Two copies of one record key with different content (two
/// wallets' caches of one contact's profile) are both re-fetched; at most
/// the one Platform holds verifies.
fn unverified_of(
    info: &PlatformWalletInfo,
    id: &WalletId,
    provenance: &Provenance,
    seen: &mut HashSet<Entity>,
) -> Vec<Held> {
    let mut held = Vec::new();
    for m in info.identity_manager.wallet_managed_identities(&id.0) {
        let all = held_of(m, info).into_iter().chain(requests_of(m));
        for h in all.chain(contact_profiles_of(m)) {
            let e = h.entity();
            if provenance.is_unverified(&e) && seen.insert(e) {
                held.push(h);
            }
        }
    }
    held
}

/// A DPNS `domain` document's `label` as spelled and its `records.identity`
/// (not `normalizedLabel`, not `$ownerId`); `None` if either is missing or
/// malformed.
fn domain_of(doc: &Document) -> Option<(String, Identifier)> {
    let props = doc.properties();
    let spelling = props.get("label")?.as_str()?.to_string();
    let records = props.get("records")?.to_map().ok()?;
    let (_, identity) = (records.iter()).find(|(k, _)| k.as_text() == Some("identity"))?;
    Some((spelling, identity.to_identifier().ok()?))
}

/// A `contactRequest` document as platform-wallet reads it
/// (`parse_contact_request_doc`); `None` without a required field.
fn request_from_document(doc: &Document) -> Option<ContactRequest> {
    let props = doc.properties();
    let int = |key: &str| props.get(key).and_then(|v| v.to_integer::<u32>().ok());
    let bytes = |key: &str| props.get(key).and_then(|v| v.to_binary_bytes().ok());
    let recipient = props.get("toUserId")?.to_identifier().ok()?;
    let mut request = ContactRequest::new(
        doc.owner_id(),
        recipient,
        int("senderKeyIndex")?,
        int("recipientKeyIndex")?,
        int("accountReference")?,
        bytes("encryptedPublicKey")?,
        doc.created_at_core_block_height().unwrap_or(0),
        doc.created_at().unwrap_or(0),
    );
    request.auto_accept_proof = bytes("autoAcceptProof");
    request.encrypted_account_label = bytes("encryptedAccountLabel");
    Some(request)
}

/// The SDK's fetches.
struct SdkRefetcher(Arc<dash_sdk::Sdk>);

impl Refetcher for SdkRefetcher {
    fn identity_keys(
        &self,
        id: Identifier,
    ) -> BoxFuture<Result<Option<BTreeMap<KeyID, IdentityPublicKey>>, PlatformError>> {
        let sdk = Arc::clone(&self.0);
        Box::pin(async move {
            Ok(Identity::fetch(&sdk, id)
                .await?
                .map(|i| i.public_keys().clone()))
        })
    }

    fn profile(&self, id: Identifier) -> BoxFuture<Result<Option<ShownProfile>, PlatformError>> {
        let sdk = Arc::clone(&self.0);
        Box::pin(async move {
            let contract = load_system_data_contract(SystemDataContract::Dashpay, sdk.version())
                .map_err(|e| PlatformError::Internal {
                    detail: format!("DashPay contract: {e}"),
                })?;
            let mut query = DocumentQuery::new(Arc::new(contract), "profile")
                .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
                .with_where(dash_sdk::drive::query::WhereClause {
                    field: "$ownerId".to_string(),
                    operator: dash_sdk::drive::query::WhereOperator::Equal,
                    value: Value::Identifier(id.to_buffer()),
                });
            query.limit = 1;
            let docs = Document::fetch_many(&sdk, query).await?;
            Ok(docs
                .into_iter()
                .filter_map(|(_, doc)| doc)
                .find(|doc| doc.owner_id() == id)
                .and_then(|doc| ShownProfile::from_document(&doc)))
        })
    }

    /// The owner of `label.dash`, by its domain document (as the SDK's
    /// `resolve_dpns_name` does, without its re-normalization: `label` is
    /// normalized already).
    fn dpns_domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        let sdk = Arc::clone(&self.0);
        Box::pin(async move {
            let contract = load_system_data_contract(SystemDataContract::DPNS, sdk.version())
                .map_err(|e| PlatformError::Internal {
                    detail: format!("DPNS contract: {e}"),
                })?;
            let equal = |field: &str, value: &str| dash_sdk::drive::query::WhereClause {
                field: field.to_string(),
                operator: dash_sdk::drive::query::WhereOperator::Equal,
                value: Value::Text(value.to_string()),
            };
            let mut query = DocumentQuery::new(Arc::new(contract), "domain")
                .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
                .with_where(equal("normalizedParentDomainName", "dash"))
                .with_where(equal("normalizedLabel", &label));
            query.limit = 1;
            let docs = Document::fetch_many(&sdk, query).await?;
            Ok(docs.into_values().find_map(|doc| domain_of(&doc?)))
        })
    }

    fn contact_requests(
        &self,
        sender: Identifier,
        recipient: Identifier,
        created_at: u64,
    ) -> BoxFuture<Result<Vec<ContactRequest>, PlatformError>> {
        let this = SdkRefetcher(Arc::clone(&self.0));
        Box::pin(async move {
            let docs = this.pair_documents(sender, recipient).await?;
            Ok(docs
                .iter()
                .filter(|doc| doc.created_at() == Some(created_at))
                .filter_map(request_from_document)
                .collect())
        })
    }
}

impl SdkRefetcher {
    /// The `contactRequest` documents from `sender` to `recipient`
    /// (`ownerIdUserId` index; a pair holds one per account reference).
    async fn pair_documents(
        &self,
        sender: Identifier,
        recipient: Identifier,
    ) -> Result<Vec<Document>, PlatformError> {
        let contract = load_system_data_contract(SystemDataContract::Dashpay, self.0.version())
            .map_err(|e| PlatformError::Internal {
                detail: format!("DashPay contract: {e}"),
            })?;
        let equal = |field: &str, id: Identifier| dash_sdk::drive::query::WhereClause {
            field: field.to_string(),
            operator: dash_sdk::drive::query::WhereOperator::Equal,
            value: Value::Identifier(id.to_buffer()),
        };
        let mut query = DocumentQuery::new(Arc::new(contract), "contactRequest")
            .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
            .with_where(equal("$ownerId", sender))
            .with_where(equal("toUserId", recipient));
        query.limit = CONTACT_REQUESTS_PER_PAIR;
        let docs = Document::fetch_many(&self.0, query).await?;
        Ok(docs.into_values().flatten().collect())
    }
}

impl NetworkSession {
    /// Starts the re-fetch task: after masternode sync, then every
    /// [`RETRY_EVERY`] while the session is open.
    pub(crate) fn spawn_reverify(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let mut synced = self.trust.subscribe_synced();
        self.rt.spawn(async move {
            if synced.wait_for(|s| *s).await.is_err() {
                return;
            }
            loop {
                let Some(this) = weak.upgrade() else { return };
                match this.reverify_once().await {
                    Ok(left) if left > 0 => {
                        tracing::info!(left, "Platform data still unverified; re-fetching later");
                    }
                    Ok(_) => {}
                    // Closed or closing.
                    Err(_) => return,
                }
                drop(this);
                tokio::time::sleep(RETRY_EVERY).await;
            }
        });
    }

    /// One run. Returns how many entities are still unverified. The
    /// session is entered only to read the library and to record, not
    /// across the fetches, which hold only the SDK.
    async fn reverify_once(&self) -> Result<usize, EngineError> {
        let (held, sdk) = {
            let _op = self.enter().await?;
            let live = self.live()?;
            let wm = live.manager.wallet_manager_arc();
            let wm = wm.read().await;
            (
                held_unverified(&wm, &live.provenance),
                live.manager.sdk_arc(),
            )
        };
        if held.is_empty() {
            return Ok(0);
        }
        let fetch = SdkRefetcher(sdk);
        let mut verified = Vec::new();
        for h in &held {
            // Closed or closing: stop.
            drop(self.try_enter()?);
            if reverify(h, &fetch).await {
                verified.push(h);
            }
        }
        drop(fetch);
        if verified.is_empty() {
            return Ok(held.len());
        }
        let _op = self.enter().await?;
        let live = self.live()?;
        let wm = live.manager.wallet_manager_arc();
        // Held across the writes: the library cannot change what is recorded.
        let wm = wm.read().await;
        let step = record_step(&wm, &live.provenance, &verified).await?;
        drop(wm);
        tracing::debug!(
            written = step.written,
            left = step.left,
            "recorded verified fetches"
        );
        Ok(step.left)
    }
}

/// What a record step did.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Recorded {
    /// Records that reached app.sqlite.
    pub(super) written: usize,
    /// Entities still unverified afterwards, counted from the library (Sol
    /// r2 R3): a write can replace another copy's record under one key.
    pub(super) left: usize,
}

/// Records the `verified` fetches the library still holds unchanged, then
/// counts what is still unverified. The caller holds the wallet manager's
/// read lock across it.
pub(super) async fn record_step(
    wm: &WalletManager<PlatformWalletInfo>,
    provenance: &Arc<Provenance>,
    verified: &[&Held],
) -> Result<Recorded, tokio::task::JoinError> {
    let records = confirmed(verified, &held_unverified(wm, provenance));
    let writer = Arc::clone(provenance);
    let written = tokio::task::spawn_blocking(move || record_all(&writer, &records)).await?;
    let left = held_unverified(wm, provenance).len();
    Ok(Recorded { written, left })
}

/// Records `records`; how many were written (Sol r1 R2). A failed write is
/// logged and leaves its entity unverified. Blocking (app.sqlite).
pub(super) fn record_all(provenance: &Provenance, records: &[Entity]) -> usize {
    let written = |entity: &&Entity| match provenance.record_verified(entity) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, kind = entity.kind, "could not record a verified fetch");
            false
        }
    };
    (records.iter())
        .filter(|e| e.digest.is_some())
        .filter(written)
        .count()
}

/// Every entity of the wallet manager's wallets, and every contact label
/// shown ([`Provenance::show_label`]), the read models show as unverified.
pub(super) fn held_unverified(
    wm: &WalletManager<PlatformWalletInfo>,
    provenance: &Provenance,
) -> Vec<Held> {
    let mut seen = HashSet::new();
    let mut held = Vec::new();
    for id in wm.list_wallets() {
        if let Some(info) = wm.get_wallet_info(id) {
            held.extend(unverified_of(info, &WalletId(*id), provenance, &mut seen));
        }
    }
    for (owner, spelling) in provenance.shown_labels() {
        let h = Held::label(&spelling, owner);
        let e = h.entity();
        if provenance.is_unverified(&e) && seen.insert(e) {
            held.push(h);
        }
    }
    held
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dpp::document::DocumentV0;

    use super::*;

    fn doc(props: &[(&str, Value)]) -> Document {
        Document::V0(DocumentV0 {
            properties: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        })
    }

    #[test]
    fn a_profile_document_reads_as_the_library_caches_it() {
        let shown = ShownProfile::from_document(&doc(&[
            ("displayName", Value::Text("Alice".into())),
            ("publicMessage", Value::Text(String::new())),
            ("avatarUrl", Value::Text("https://a.example/x.png".into())),
            ("avatarHash", Value::Bytes32([3; 32])),
            ("avatarFingerprint", Value::Bytes(vec![4; 8])),
        ]))
        .unwrap();
        assert_eq!(
            shown,
            ShownProfile {
                display_name: Some("Alice".into()),
                public_message: None,
                avatar_url: Some("https://a.example/x.png".into()),
                avatar_hash: Some([3; 32]),
                avatar_fingerprint: Some([4; 8]),
            }
        );
        let hash_as_bytes =
            ShownProfile::from_document(&doc(&[("avatarHash", Value::Bytes(vec![3; 32]))]));
        assert_eq!(hash_as_bytes.unwrap().avatar_hash, Some([3; 32]));
    }

    /// Sol r2 N1: the spelling is the document's `label`, the owner its
    /// `records.identity`; a missing or malformed field reads as nothing.
    #[test]
    fn a_domain_document_reads_its_spelling_and_records_identity() {
        let (owner, records) = (Identifier::new([5; 32]), Identifier::new([6; 32]));
        let identity = |v: Value| Value::Map(vec![(Value::Text("identity".into()), v)]);
        let domain = |label: Option<Value>, records: Option<Value>| {
            let mut props = vec![("normalizedLabel", Value::Text("a11ce".into()))];
            props.extend(label.map(|l| ("label", l)));
            props.extend(records.map(|r| ("records", r)));
            let mut d = doc(&props);
            let Document::V0(v0) = &mut d;
            v0.owner_id = owner;
            domain_of(&d)
        };
        let alice = || Some(Value::Text("Alice".into()));
        let points_to = |id: Identifier| Some(identity(Value::Identifier(id.to_buffer())));
        assert_eq!(
            domain(alice(), points_to(records)),
            Some(("Alice".into(), records)),
            "the spelling, not normalizedLabel; records.identity, not $ownerId"
        );
        assert_eq!(domain(None, points_to(records)), None, "no label");
        assert_eq!(domain(Some(Value::U32(1)), points_to(records)), None);
        assert_eq!(domain(alice(), None), None, "no records");
        assert_eq!(domain(alice(), Some(Value::Text("x".into()))), None);
        let other = Value::Map(vec![(
            Value::Text("contract".into()),
            Value::Identifier(records.to_buffer()),
        )]);
        assert_eq!(domain(alice(), Some(other)), None, "no records.identity");
        assert_eq!(
            domain(alice(), Some(identity(Value::Bool(true)))),
            None,
            "a malformed identity"
        );
    }

    #[test]
    fn a_contact_request_document_reads_as_the_library_reads_it() {
        let (sender, recipient) = (Identifier::new([7; 32]), Identifier::new([8; 32]));
        let mut d = doc(&[
            ("toUserId", Value::Identifier(recipient.to_buffer())),
            ("senderKeyIndex", Value::U32(1)),
            ("recipientKeyIndex", Value::U32(2)),
            ("accountReference", Value::U32(3)),
            ("encryptedPublicKey", Value::Bytes(vec![4; 96])),
            ("autoAcceptProof", Value::Bytes(vec![5; 38])),
        ]);
        let Document::V0(v0) = &mut d;
        v0.owner_id = sender;
        v0.created_at = Some(1_700_000_000_123);
        v0.created_at_core_block_height = Some(900);
        let mut want = ContactRequest::new(
            sender,
            recipient,
            1,
            2,
            3,
            vec![4; 96],
            900,
            1_700_000_000_123,
        );
        want.auto_accept_proof = Some(vec![5; 38]);
        assert_eq!(request_from_document(&d), Some(want));

        let Document::V0(v0) = &mut d;
        v0.properties.remove("encryptedPublicKey");
        assert_eq!(
            request_from_document(&d),
            None,
            "a required field is missing"
        );
    }

    #[test]
    fn urls_the_library_would_not_fetch_and_empty_profiles_are_dropped() {
        let url = |u: String| {
            ShownProfile::from_document(&doc(&[
                ("displayName", Value::Text("A".into())),
                ("avatarUrl", Value::Text(u)),
            ]))
            .unwrap()
            .avatar_url
        };
        assert_eq!(url("http://a.example/x.png".into()), None);
        let long = format!("https://a.example/{}", "x".repeat(MAX_AVATAR_URL_LEN));
        assert_eq!(url(long), None);
        assert_eq!(
            ShownProfile::from_document(&doc(&[("displayName", Value::Text(String::new()))])),
            None
        );
        assert_eq!(
            ShownProfile::from_document(&doc(&[("avatarFingerprint", Value::Bytes(vec![1; 7]))])),
            None,
            "a fingerprint of the wrong length is not set"
        );
    }
}
