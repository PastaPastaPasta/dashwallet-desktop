//! The Identities page's reads (DP6-01): an identity's keys, Platform's
//! copy of them folded into the library's, and a read-only lookup of any
//! identity by id or DPNS name.
//!
//! - `identity_detail` reads the library's memory after its read lock, so a
//!   sync pass's write is waited for rather than answered from the snapshot.
//! - A refresh holds the identity's write lock (`identity_writes.rs`), so it
//!   never moves the revision under a key upgrade or a name write. It folds
//!   Platform's keys and revision only, and only from an answer at least as
//!   new as the library's revision: a node a block behind could otherwise
//!   re-enable a disabled key. The balance is DP1-06's ([`Balances`]),
//!   proved with its block time, never the fetched identity's, which carries
//!   none. The library's own `refresh_identity` replaces the whole identity
//!   and persists no keys, so it is not used. The keys are stored before the
//!   revision: a failed store leaves the old revision in memory and on disk,
//!   so the next refresh folds the answer again (fail closed, D16).
//! - A lookup adds nothing to the library and records nothing as verified:
//!   it shows what Platform answered, marked `unverified` as the user search
//!   marks its hits (m4 §5).
//!
//! [`Balances`]: super::balances::Balances

use std::collections::BTreeMap;
use std::sync::Arc;

use dash_sdk::platform::Fetch;
use dash_sdk::platform::dpns_usernames::is_valid_username;
use dpp::identity::accessors::{IdentityGettersV0, IdentitySettersV0};
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::{
    Identity, IdentityPublicKey, KeyID, KeyType as DppKeyType, Purpose,
    SecurityLevel as DppSecurityLevel,
};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_uri::deeplink::dpns_label;
use platform_wallet::changeset::changeset::{IdentityChangeSet, IdentityEntry};
use platform_wallet::changeset::{PlatformWalletChangeSet, PlatformWalletPersistence};

use super::errors::{IdentityError, PlatformError};
use super::identity::{
    FoundBy, FoundIdentity, IdentityDetail, IdentityKeyInfo, IdentityRefresh, KeyPurpose, KeyType,
    SecurityLevel,
};
use super::identity_writes::identity_locks;
use super::registration_flow::parse_identity;
use super::reverify::{Held, Refetcher, SdkRefetcher};
use super::trust::{self, BoxFuture};
use crate::{NetworkSession, WalletId};

/// The names a lookup shows at most.
const FOUND_NAMES: u32 = 10;

/// Platform's answers for the identity reads: the SDK, or a test's script.
pub(crate) trait IdentityNet: Send + Sync {
    /// The identity `id`, proved; `None` when Platform has none.
    fn identity(&self, id: Identifier) -> BoxFuture<Result<Option<Identity>, PlatformError>>;
    /// The domain of the normalized `label`: its label as spelled and its
    /// `records.identity` (DP2-03's read).
    fn domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>>;
    /// The labels, as spelled, of at most `limit` domains whose
    /// `records.identity` is `id`.
    fn names_of(&self, id: Identifier, limit: u32)
    -> BoxFuture<Result<Vec<String>, PlatformError>>;
}

struct SdkIdentities(SdkRefetcher);

impl IdentityNet for SdkIdentities {
    fn identity(&self, id: Identifier) -> BoxFuture<Result<Option<Identity>, PlatformError>> {
        let sdk = Arc::clone(&self.0.0);
        Box::pin(async move { Ok(Identity::fetch(&sdk, id).await?) })
    }

    fn domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        self.0.dpns_domain(label)
    }

    fn names_of(
        &self,
        id: Identifier,
        limit: u32,
    ) -> BoxFuture<Result<Vec<String>, PlatformError>> {
        let sdk = Arc::clone(&self.0.0);
        Box::pin(async move {
            let names = sdk.get_dpns_usernames_by_identity(id, Some(limit)).await?;
            Ok(names.into_iter().map(|n| n.label).collect())
        })
    }
}

/// The keys as the page lists them, by key id.
fn key_infos(keys: &BTreeMap<KeyID, IdentityPublicKey>) -> Vec<IdentityKeyInfo> {
    keys.values().map(key_info).collect()
}

fn key_info(k: &IdentityPublicKey) -> IdentityKeyInfo {
    let (contract_bound, contract_bound_document_type) = match k.contract_bounds() {
        None => (None, None),
        Some(ContractBounds::SingleContract { id } | ContractBounds::ContractGroup { id }) => {
            (Some(*id), None)
        }
        Some(ContractBounds::SingleContractDocumentType {
            id,
            document_type_name,
        }) => (Some(*id), Some(document_type_name.clone())),
    };
    IdentityKeyInfo {
        id: k.id(),
        purpose: match k.purpose() {
            Purpose::AUTHENTICATION => KeyPurpose::Authentication,
            Purpose::ENCRYPTION => KeyPurpose::Encryption,
            Purpose::DECRYPTION => KeyPurpose::Decryption,
            Purpose::TRANSFER => KeyPurpose::Transfer,
            Purpose::SYSTEM => KeyPurpose::System,
            Purpose::VOTING => KeyPurpose::Voting,
            Purpose::OWNER => KeyPurpose::Owner,
        },
        security_level: match k.security_level() {
            DppSecurityLevel::MASTER => SecurityLevel::Master,
            DppSecurityLevel::CRITICAL => SecurityLevel::Critical,
            DppSecurityLevel::HIGH => SecurityLevel::High,
            DppSecurityLevel::MEDIUM => SecurityLevel::Medium,
        },
        key_type: match k.key_type() {
            DppKeyType::ECDSA_SECP256K1 => KeyType::EcdsaSecp256k1,
            DppKeyType::BLS12_381 => KeyType::Bls12381,
            DppKeyType::ECDSA_HASH160 => KeyType::EcdsaHash160,
            DppKeyType::BIP13_SCRIPT_HASH => KeyType::Bip13ScriptHash,
            DppKeyType::EDDSA_25519_HASH160 => KeyType::EddsaHash160,
        },
        public_key: hex::encode(k.data().as_slice()),
        read_only: k.read_only(),
        disabled_at: k.disabled_at(),
        contract_bound: contract_bound.map(|id| id.to_string(Encoding::Base58)),
        contract_bound_document_type,
    }
}

/// What a lookup was asked for.
enum Query {
    Id(Identifier),
    /// A DPNS label as typed, without `.dash`.
    Name(String),
}

fn query(text: &str) -> Result<Query, PlatformError> {
    let text = text.trim();
    if let Ok(id) = Identifier::from_string(text, Encoding::Base58) {
        return Ok(Query::Id(id));
    }
    let label = dpns_label(text);
    if is_valid_username(label) {
        return Ok(Query::Name(label.to_string()));
    }
    Err(PlatformError::InvalidArgument {
        detail: "not an identity id or a username".into(),
    })
}

impl NetworkSession {
    fn identity_net(&self) -> Result<Arc<dyn IdentityNet>, PlatformError> {
        #[cfg(test)]
        if let Some(net) = super::runtime::guard(&self.platform.identity_net).clone() {
            return Ok(net);
        }
        let manager = self.manager()?;
        Ok(Arc::new(SdkIdentities(SdkRefetcher(manager.sdk_arc()))))
    }

    /// `DashPay::identity_detail`.
    pub(super) async fn identity_detail_of(
        &self,
        wallet: WalletId,
        identity: String,
    ) -> Result<IdentityDetail, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&wallet)?;
        self.detail(wallet, parse_identity(&identity)?).await
    }

    /// The identity's summary, revision and keys: the library's after its
    /// read lock.
    async fn detail(
        &self,
        wallet: WalletId,
        id: Identifier,
    ) -> Result<IdentityDetail, PlatformError> {
        let (revision, public_keys) = {
            let manager = self.manager()?;
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let m = (wm.get_wallet_info(&wallet.0))
                .and_then(|info| info.identity_manager.wallet_identity(&wallet.0, &id))
                .ok_or(IdentityError::NotFound)?;
            (m.identity.revision(), key_infos(m.identity.public_keys()))
        };
        let b58 = id.to_string(Encoding::Base58);
        let summary = (self.identity_summaries(wallet)?.into_iter())
            .find(|s| s.identity == b58)
            .ok_or(IdentityError::NotFound)?;
        Ok(IdentityDetail {
            summary,
            revision: Some(revision),
            public_keys,
        })
    }

    /// `DashPay::refresh_identity`.
    pub(super) async fn refresh_identity_of(
        &self,
        wallet: WalletId,
        identity: String,
    ) -> Result<IdentityDetail, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&wallet)?;
        let id = parse_identity(&identity)?;
        self.refresh_one(wallet, id).await?;
        self.detail(wallet, id).await
    }

    /// `DashPay::refresh_identities`: each identity in index order, one at a
    /// time.
    pub(super) async fn refresh_identities_of(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<IdentityRefresh>, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&wallet)?;
        let mut ids = {
            let manager = self.manager()?;
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            (wm.get_wallet_info(&wallet.0))
                .map(|info| {
                    (info.identity_manager.wallet_managed_identities(&wallet.0))
                        .map(|m| (m.identity_index, m.identity.id()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        ids.sort();
        let mut out = Vec::with_capacity(ids.len());
        for (_, id) in ids {
            let refreshed = self.refresh_one(wallet, id).await;
            if let Err(e) = &refreshed {
                // The wallet or the network gone fails the whole run (contract §2.1).
                if matches!(
                    e,
                    PlatformError::WalletNotFound | PlatformError::NetworkNotOpen
                ) {
                    return Err(refreshed.unwrap_err());
                }
                tracing::debug!(identity = %id, error = %e, "an identity refresh failed");
            }
            out.push(IdentityRefresh {
                identity: id.to_string(Encoding::Base58),
                failure_code: refreshed.err().map(|e| e.code().to_string()),
            });
        }
        Ok(out)
    }

    /// Folds Platform's keys and revision of the wallet's identity `id` into
    /// the library's, then refreshes its balance (module doc).
    async fn refresh_one(&self, wallet: WalletId, id: Identifier) -> Result<(), PlatformError> {
        let manager = self.manager()?;
        let w = (manager.get_wallet(&wallet.0).await).ok_or(PlatformError::WalletNotFound)?;
        if (w.state().await.identity_manager)
            .wallet_identity(&wallet.0, &id)
            .is_none()
        {
            return Err(IdentityError::NotFound.into());
        }
        let net = self.identity_net()?;
        let store = self.live()?.store;
        {
            let locks = identity_locks(wallet, id);
            let _write = locks.write.lock().await;
            let fetched = (net.identity(id).await?).ok_or(IdentityError::NotFound)?;
            let mut state = w.state_mut().await;
            let m = (state.identity_manager)
                .wallet_identity_mut(&wallet.0, &id)
                .ok_or(IdentityError::NotFound)?;
            fold(m, &fetched, w.persister(), (store.as_ref(), wallet))?;
        }
        self.refresh_balance_of(wallet, id).await?;
        Ok(())
    }

    /// `DashPay::find_identity`.
    pub(super) async fn find_identity_of(
        &self,
        wallet: WalletId,
        text: String,
    ) -> Result<Option<FoundIdentity>, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&wallet)?;
        let query = query(&text)?;
        let net = self.identity_net()?;
        let (id, found_by, label_shown) = match query {
            Query::Id(id) => (id, FoundBy::Id, None),
            Query::Name(label) => {
                let normalized = convert_to_homograph_safe_chars(&label);
                let (domain, scope) = trust::scoped(net.domain(normalized)).await;
                let Some((spelling, owner)) = domain? else {
                    return Ok(None);
                };
                let shown = (Held::label(&spelling, owner), scope.verified());
                (owner, FoundBy::Name { label: spelling }, Some(shown))
            }
        };
        let (identity, scope) = trust::scoped(net.identity(id)).await;
        let Some(identity) = identity? else {
            return Ok(None);
        };
        let keys_shown = (
            Held::Identity {
                id,
                keys: identity.public_keys().clone(),
            },
            scope.verified(),
        );
        let (names, names_scope) = trust::scoped(net.names_of(id, FOUND_NAMES)).await;
        let names = names
            .inspect_err(|e| tracing::debug!(error = %e, "could not read the identity's names"))
            .unwrap_or_default();
        // Each name shown counts, as in `IdentitySummary` (m4 §5).
        let names_shown = (names.iter()).map(|n| (Held::label(n, id), names_scope.verified()));
        let b58 = id.to_string(Encoding::Base58);
        let own = (self.identity_summaries(wallet)?.iter()).any(|s| s.identity == b58);
        let provenance = self.live()?.provenance;
        let unverified = [Some(keys_shown), label_shown]
            .into_iter()
            .flatten()
            .chain(names_shown)
            .any(|(held, verified)| !verified && provenance.is_unverified(&held.entity()));
        Ok(Some(FoundIdentity {
            identity: b58,
            found_by,
            names,
            balance: Some(identity.balance()),
            revision: Some(identity.revision()),
            public_keys: key_infos(identity.public_keys()),
            own,
            unverified,
        }))
    }
}

/// Folds `fetched`'s keys and revision into `m` and persists them, when
/// `fetched` is not older than `m` (module doc). The balance is left alone.
///
/// Each changed key is stored first, under the old revision; a key whose
/// store fails is put back as it was, so memory never runs ahead of the
/// store (a failure the store keeps for retry may land on disk later, still
/// Platform's answer, and the next refresh folds it into memory again).
/// The new revision is stored last, through `store` so that its failure is
/// `storage` too, and only once every key is.
fn fold(
    m: &mut platform_wallet::ManagedIdentity,
    fetched: &Identity,
    persister: &platform_wallet::WalletPersister,
    store: (&dyn PlatformWalletPersistence, WalletId),
) -> Result<(), PlatformError> {
    if fetched.revision() < m.identity.revision() {
        return Ok(());
    }
    let changed: Vec<IdentityPublicKey> = (fetched.public_keys().values())
        .filter(|k| m.identity.public_keys().get(&k.id()) != Some(*k))
        .cloned()
        .collect();
    for key in changed {
        let id = key.id();
        let was = m.identity.public_keys().get(&id).cloned();
        if let Err(e) = m.replace_key(key, persister) {
            let mut keys = m.identity.public_keys().clone();
            match was {
                Some(was) => keys.insert(id, was),
                None => keys.remove(&id),
            };
            m.identity.set_public_keys(keys);
            return Err(not_stored("identity keys", e));
        }
    }
    let before = m.identity.revision();
    if fetched.revision() != before {
        m.identity.set_revision(fetched.revision());
        let mut snapshot = IdentityChangeSet::default();
        (snapshot.identities).insert(m.id(), IdentityEntry::from_managed(m));
        let cs = PlatformWalletChangeSet {
            identities: Some(snapshot),
            ..Default::default()
        };
        if let Err(e) = store.0.store(store.1.0, cs) {
            m.identity.set_revision(before);
            return Err(not_stored("identity revision", e));
        }
    }
    Ok(())
}

fn not_stored(what: &str, e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Storage {
        detail: format!("{what} not stored: {e}"),
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
