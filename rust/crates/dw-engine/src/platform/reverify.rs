//! Re-fetching unverified Platform data once SPV has synced (DASHPAY §2.2,
//! DEC-125; E0-10b design §5). Once the fallback latch is set, an entity the
//! read models show is unverified until one of its fetches verifies against
//! SPV-held quorum keys. This task re-fetches the identities, profiles and
//! DPNS labels the library holds, each in its own trust scope, and records
//! those whose fetch verified and matches what the library holds. A
//! verified fetch that differs records nothing: the library's own sync
//! replaces the data, and a later run records it.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dpp::document::{Document, DocumentV0Getters};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyID};
use dpp::platform_value::Value;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::{SystemDataContract, load_system_data_contract};
use dpp::util::strings::convert_to_homograph_safe_chars;
use platform_wallet::wallet::identity::DashPayProfile;

use super::errors::PlatformError;
use super::provenance::{Provenance, kind};
use super::recovery::owned_names;
use super::trust::{self, BoxFuture};
use crate::{EngineError, NetworkSession, WalletId};

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
    fn of(p: &DashPayProfile) -> Self {
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
    /// `label` is homograph-normalized.
    Label {
        label: String,
        owner: Identifier,
    },
}

impl Held {
    /// The `dp_trust_verified` record a verified match writes.
    fn record(&self) -> (&'static str, String) {
        match self {
            Self::Identity { id, .. } => (kind::IDENTITY, id.to_string(Encoding::Base58)),
            Self::Profile { id, .. } => (kind::PROFILE, id.to_string(Encoding::Base58)),
            Self::Label { label, .. } => (kind::DPNS_LABEL, label.clone()),
        }
    }
}

/// Platform's current answers, proof-verified by the SDK.
pub(super) trait Refetcher: Send + Sync {
    fn identity_keys(
        &self,
        id: Identifier,
    ) -> BoxFuture<Result<Option<BTreeMap<KeyID, IdentityPublicKey>>, PlatformError>>;
    fn profile(&self, id: Identifier) -> BoxFuture<Result<Option<ShownProfile>, PlatformError>>;
    fn dpns_owner(&self, label: String) -> BoxFuture<Result<Option<Identifier>, PlatformError>>;
}

/// Re-fetches each entity in its own scope. Returns the records to write:
/// the fetch used only SPV-verified quorum keys and matches the library.
pub(super) async fn reverify(held: &[Held], fetch: &dyn Refetcher) -> Vec<(&'static str, String)> {
    let mut records = Vec::new();
    for h in held {
        let (matches, tally) = trust::scoped(async {
            match h {
                Held::Identity { id, keys } => {
                    fetch.identity_keys(*id).await.ok().flatten().as_ref() == Some(keys)
                }
                Held::Profile { id, shown } => {
                    fetch.profile(*id).await.ok().flatten().as_ref() == Some(shown)
                }
                Held::Label { label, owner } => {
                    fetch.dpns_owner(label.clone()).await.ok().flatten() == Some(*owner)
                }
            }
        })
        .await;
        if matches && tally.verified() {
            records.push(h.record());
        }
    }
    records
}

/// The entities of `info` the read models would show as unverified
/// (`recovery::summaries`), each once.
fn unverified_of(
    info: &platform_wallet::PlatformWalletInfo,
    id: &WalletId,
    provenance: &Provenance,
    seen: &mut HashSet<(&'static str, String)>,
) -> Vec<Held> {
    let mut held = Vec::new();
    let mut push = |h: Held| {
        let (k, key) = h.record();
        if provenance.is_unverified(k, &key) && seen.insert((k, key)) {
            held.push(h);
        }
    };
    for m in info.identity_manager.wallet_managed_identities(&id.0) {
        let identity = m.identity.id();
        push(Held::Identity {
            id: identity,
            keys: m.identity.public_keys().clone(),
        });
        if let Some(p) = &m.dashpay().profile {
            push(Held::Profile {
                id: identity,
                shown: ShownProfile::of(p),
            });
        }
        for (label, _) in owned_names(identity, &m.dpns_names, &info.dpns_name_states) {
            push(Held::Label {
                label: convert_to_homograph_safe_chars(&label),
                owner: identity,
            });
        }
    }
    held
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

    fn dpns_owner(&self, label: String) -> BoxFuture<Result<Option<Identifier>, PlatformError>> {
        let sdk = Arc::clone(&self.0);
        Box::pin(async move { Ok(sdk.resolve_dpns_name(&label).await?) })
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

    /// One run. Returns how many entities are still unverified.
    async fn reverify_once(&self) -> Result<usize, EngineError> {
        let _op = self.enter().await?;
        let live = self.live()?;
        let held = {
            let wm = live.manager.wallet_manager_arc();
            let wm = wm.read().await;
            let mut seen = HashSet::new();
            let mut held = Vec::new();
            for id in wm.list_wallets() {
                let id = WalletId(*id);
                if let Some(info) = wm.get_wallet_info(&id.0) {
                    held.extend(unverified_of(info, &id, &live.provenance, &mut seen));
                }
            }
            held
        };
        if held.is_empty() {
            return Ok(0);
        }
        let records = reverify(&held, &SdkRefetcher(live.manager.sdk_arc())).await;
        let written = records.len();
        let provenance = Arc::clone(&live.provenance);
        tokio::task::spawn_blocking(move || {
            for (k, key) in records {
                if let Err(e) = provenance.record_verified(k, &key) {
                    tracing::warn!(error = %e, kind = k, "could not record a verified fetch");
                }
            }
        })
        .await?;
        Ok(held.len() - written)
    }
}
