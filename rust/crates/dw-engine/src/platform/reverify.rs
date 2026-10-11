//! Re-fetching unverified Platform data once SPV has synced (DASHPAY §2.2,
//! DEC-125; E0-10b design §5). Once the fallback latch is set, an entity the
//! read models show is unverified until one of its fetches verifies against
//! SPV-held quorum keys. This task re-fetches the identities, profiles and
//! DPNS labels the library holds, each in its own trust scope, and records
//! those whose fetch verified and matches what the library holds. A
//! verified fetch that differs records nothing: the library's own sync
//! replaces the data, and a later run records it. The record is written
//! under the wallet manager's read lock, and only for an entity the library
//! still holds unchanged then.

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
use platform_wallet::PlatformWalletInfo;
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
            Held::Label { label, owner } => {
                ok(fetch.dpns_owner(label.clone()).await) == Some(*owner)
            }
        }
    })
    .await;
    matches && tally.verified()
}

/// The records to write: entities whose fetch verified and that the
/// library still holds, unchanged and unverified (`current`).
pub(super) fn confirmed(verified: &[&Held], current: &[Held]) -> Vec<(&'static str, String)> {
    verified
        .iter()
        .filter(|h| current.contains(h))
        .map(|h| h.record())
        .collect()
}

/// The entities of `info` the read models would show as unverified
/// (`recovery::summaries`), each once.
fn unverified_of(
    info: &PlatformWalletInfo,
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

    /// The owner of `label.dash`, by its domain document (as the SDK's
    /// `resolve_dpns_name` does, without its re-normalization: `label` is
    /// normalized already).
    fn dpns_owner(&self, label: String) -> BoxFuture<Result<Option<Identifier>, PlatformError>> {
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
            Ok(docs.into_iter().find_map(|(_, doc)| {
                let records = doc?.properties().get("records")?.to_map().ok()?.clone();
                let identity = records
                    .iter()
                    .find(|(k, _)| k.as_text() == Some("identity"))?;
                identity.1.to_identifier().ok()
            }))
        })
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
            let wallets = wm
                .list_wallets()
                .into_iter()
                .filter_map(|id| Some((WalletId(*id), wm.get_wallet_info(id)?)));
            (
                held_unverified(wallets, &live.provenance),
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
        let wallets = wm
            .list_wallets()
            .into_iter()
            .filter_map(|id| Some((WalletId(*id), wm.get_wallet_info(id)?)));
        let records = confirmed(&verified, &held_unverified(wallets, &live.provenance));
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
        drop(wm);
        Ok(held.len() - written)
    }
}

/// Every entity of `wallets` the read models show as unverified.
fn held_unverified<'a>(
    wallets: impl IntoIterator<Item = (WalletId, &'a PlatformWalletInfo)>,
    provenance: &Provenance,
) -> Vec<Held> {
    let mut seen = HashSet::new();
    let mut held = Vec::new();
    for (id, info) in wallets {
        held.extend(unverified_of(info, &id, provenance, &mut seen));
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
