//! Profiles behind the facade calls of `profile.rs` (DP4-01,
//! `docs/design/DP4-01-profile.md`). Not part of the facade.
//!
//! - The limits are the DashPay contract's (`limits`).
//! - `profile` reads what `identities()` and the contacts read model show:
//!   own identities from the identity read, other users from the contacts
//!   model ([`NetworkSession::profile_of`]).
//! - `update_profile` replaces Platform's document with the whole profile
//!   the edit describes ([`update`]): the library's own update merges and
//!   cannot clear a field (D1). It refuses what it can before it redeems a
//!   grant, as every DashPay write (`requests.rs`).
//! - The library refreshes a contact's profile at most once an hour; the
//!   engine expires that cache at session open and after every pass that
//!   fetched, so each pass fetches the contacts' profiles again (D12,
//!   [`NetworkSession::expire_contact_profiles`]).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use dpp::data_contract::accessors::v0::DataContractV0Getters;
use dpp::data_contract::document_type::DocumentPropertyType;
use dpp::data_contract::document_type::accessors::DocumentTypeV0Getters;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey};
use dpp::platform_value::Value;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use platform_wallet::PlatformWallet;
use platform_wallet::wallet::identity::DashPayProfile;

use super::contacts_model::identifier;
use super::errors::{ContactError, IdentityError, PlatformError, ProfileError};
use super::flows::{DOCUMENT_10K_CREDITS, FlowKind};
use super::identity::KeyPurpose;
use super::identity_writes::identity_locks;
use super::lease::FlowOutcome;
use super::profile::{AvatarChange, Profile, ProfileEdit, ProfileField, ProfileLimits};
use super::registration_flow::dashpay_contract;
use super::requests::{RequestNet, leased, not_found, prepare_signer};
use crate::session::Manager;
use crate::{NetworkSession, WalletId};

/// A profile document's properties, as Platform holds them.
pub(crate) type ProfileProperties = BTreeMap<String, Value>;

/// An identity's `profile` document on Platform.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProfileDocument {
    pub(crate) id: Identifier,
    pub(crate) revision: u64,
    pub(crate) properties: ProfileProperties,
}

/// The DashPay contract's profile document type.
pub(super) const PROFILE: &str = "profile";
const DISPLAY_NAME: &str = "displayName";
const PUBLIC_MESSAGE: &str = "publicMessage";
const AVATAR_URL: &str = "avatarUrl";
const AVATAR_HASH: &str = "avatarHash";
const AVATAR_FINGERPRINT: &str = "avatarFingerprint";
/// The avatar fields, all or none (the contract's `dependentRequired`).
const AVATAR: [&str; 3] = [AVATAR_URL, AVATAR_HASH, AVATAR_FINGERPRINT];
/// The longest avatar URL the library caches (its `MAX_AVATAR_URL_LEN`).
const MAX_AVATAR_URL_LEN: usize = 2048;

/// The limits if the contract lacked them (D3): DIP-15's.
pub(super) const BUILT_IN_LIMITS: ProfileLimits = ProfileLimits {
    display_name_max: 25,
    public_message_max: 140,
};

/// The `maxLength` of the profile's display name and public message in the
/// DashPay contract the library bundles (D3), read once.
pub(super) fn limits() -> ProfileLimits {
    static LIMITS: OnceLock<ProfileLimits> = OnceLock::new();
    *LIMITS.get_or_init(|| {
        contract_limits().unwrap_or_else(|e| {
            tracing::warn!("the DashPay contract gives no profile limits ({e}); using 25 / 140");
            BUILT_IN_LIMITS
        })
    })
}

pub(super) fn contract_limits() -> Result<ProfileLimits, String> {
    let contract = dashpay_contract().map_err(|e| e.to_string())?;
    let profile = (contract.document_type_for_name(PROFILE)).map_err(|e| e.to_string())?;
    let max = |field: &str| match profile.properties().get(field).map(|p| &p.property_type) {
        Some(DocumentPropertyType::String(sizes)) => {
            (sizes.max_length.map(u32::from)).ok_or_else(|| format!("{field} has no maxLength"))
        }
        _ => Err(format!("{field} is not a string property")),
    };
    Ok(ProfileLimits {
        display_name_max: max(DISPLAY_NAME)?,
        public_message_max: max(PUBLIC_MESSAGE)?,
    })
}

/// `edit` with each text trimmed, an empty one cleared (D4), and refused
/// past its limit, counted in Unicode scalar values as the contract's
/// `maxLength` is.
pub(super) fn normalized(
    edit: ProfileEdit,
    limits: ProfileLimits,
) -> Result<ProfileEdit, ProfileError> {
    let ProfileEdit {
        display_name,
        public_message,
        avatar,
    } = edit;
    let text = |(value, field, max): (Option<String>, ProfileField, u32)| {
        let Some(value) = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) else {
            return Ok(None);
        };
        let length = u32::try_from(value.chars().count()).unwrap_or(u32::MAX);
        if length > max {
            return Err(ProfileError::TooLong { field, length, max });
        }
        Ok(Some(value))
    };
    use ProfileField::{DisplayName, PublicMessage};
    Ok(ProfileEdit {
        display_name: text((display_name, DisplayName, limits.display_name_max))?,
        public_message: text((public_message, PublicMessage, limits.public_message_max))?,
        avatar,
    })
}

/// The document `edit` makes of Platform's `current` properties: the texts
/// set or removed, the avatar kept or removed, and every field the edit
/// does not own carried over (D2).
pub(super) fn merged(current: Option<&ProfileProperties>, edit: &ProfileEdit) -> ProfileProperties {
    let mut properties = current.cloned().unwrap_or_default();
    for (key, value) in [
        (DISPLAY_NAME, &edit.display_name),
        (PUBLIC_MESSAGE, &edit.public_message),
    ] {
        match value {
            Some(text) => properties.insert(key.to_owned(), Value::Text(text.clone())),
            None => properties.remove(key),
        };
    }
    if edit.avatar == AvatarChange::Remove {
        for key in AVATAR {
            properties.remove(key);
        }
    }
    properties
}

/// The profile the library caches for a document of `properties`, as its
/// own fetch parses one (`fetch_contact_profiles_chunk`): an avatar URL it
/// would not trust dropped, and `None` when no field is left.
fn library_profile(properties: &ProfileProperties) -> Option<DashPayProfile> {
    let text = |key: &str| {
        (properties.get(key).and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let bytes = |key: &str| properties.get(key).and_then(|v| v.as_bytes_slice().ok());
    let public_message = text(PUBLIC_MESSAGE);
    let profile = DashPayProfile {
        display_name: text(DISPLAY_NAME),
        bio: public_message.clone(),
        avatar_url: text(AVATAR_URL)
            .filter(|u| u.len() <= MAX_AVATAR_URL_LEN && u.starts_with("https://")),
        avatar_hash: bytes(AVATAR_HASH).and_then(|b| b.try_into().ok()),
        avatar_fingerprint: bytes(AVATAR_FINGERPRINT).and_then(|b| b.try_into().ok()),
        public_message,
    };
    (profile != DashPayProfile::default()).then_some(profile)
}

/// Sets the library's cached own profile to `properties` (D10): its store
/// signals `Platform{Identities}`. Nothing when it already says so.
async fn remember(wallet: &PlatformWallet, identity: Identifier, properties: &ProfileProperties) {
    let profile = library_profile(properties);
    let mut state = wallet.state_mut().await;
    let Some(m) = state.identity_manager.managed_identity_mut(&identity) else {
        return;
    };
    if m.dashpay().profile != profile {
        m.set_dashpay_profile(profile, wallet.persister());
    }
}

/// A document write's refusal as the `PlatformError` it wraps.
fn platform(e: ContactError) -> PlatformError {
    e.platform()
        .cloned()
        .unwrap_or_else(|| PlatformError::Internal {
            detail: e.to_string(),
        })
}

/// `DashPay::update_profile` (m4 §2.7; the steps of the design's "Update").
pub(super) async fn update(
    net: &impl RequestNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    (wallet_id, identity): (WalletId, &str),
    edit: ProfileEdit,
    grant: String,
) -> Result<(), PlatformError> {
    let identity = identifier(identity).map_err(platform)?;
    let edit = normalized(edit, limits())?;
    if let AvatarChange::Set { .. } = edit.avatar {
        // No candidate exists before DP4-02 (D6).
        return Err(PlatformError::InvalidArgument {
            detail: "unknown avatar candidate".into(),
        });
    }
    // Refreshes the spent balance once the write lock is released.
    let _refresh = session.after_spend(wallet_id, identity);
    let locks = identity_locks(wallet_id, identity);
    let _write = locks.write.lock().await;
    let (index, me) = {
        let state = wallet.state().await;
        let m = (state.identity_manager.managed_identity(&identity))
            .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
            .ok_or_else(not_found)?;
        (m.identity_index, m.identity.clone())
    };
    let index = index.ok_or_else(|| PlatformError::InvalidArgument {
        detail: "the wallet only watches this identity; it cannot publish".into(),
    })?;
    let current = net.profile_document(identity).await?;
    let wanted = merged(current.as_ref().map(|d| &d.properties), &edit);
    // Nothing to send (D7); the cache follows Platform.
    if current.as_ref().is_some_and(|d| d.properties == wanted) {
        remember(wallet, identity, &wanted).await;
        return Ok(());
    }
    if wanted.is_empty() {
        return match current {
            None => Ok(()),
            Some(_) => Err(ProfileError::Empty.into()),
        };
    }
    let (lease, permit, signer) = prepare_signer(
        net,
        session,
        (index, &me),
        (wallet_id, &grant),
        (
            &[FlowKind::ProfileEdit],
            DOCUMENT_10K_CREDITS,
            "put_profile",
        ),
        || Ok(()),
    )
    .await
    .map_err(platform)?;
    let key = signing_key(&me, &signer)?;
    let at = current.as_ref().map(|d| (d.id, d.revision));
    let written = leased(
        &lease,
        net.put_profile(identity, at, wanted.clone(), &key, &signer),
    )
    .await;
    drop(signer);
    let outcome = permit.finish(written.is_ok());
    let e = match written {
        Ok(()) => {
            remember(wallet, identity, &wanted).await;
            return Ok(());
        }
        // Nothing signed, so nothing went out.
        Err(e) if outcome == FlowOutcome::Cancelled => return Err(e),
        Err(e) => e,
    };
    // Signed, so it may have landed (D9): Platform's document decides, and a
    // failed read decides nothing.
    tracing::info!(identity = %identity.to_string(Encoding::Base58), "profile write failed: {e}");
    match net.profile_document(identity).await {
        Ok(Some(d)) if d.properties == wanted => {
            remember(wallet, identity, &wanted).await;
            Ok(())
        }
        read => {
            if let Err(again) = read {
                tracing::info!("after the failure: {again}");
            }
            Err(e)
        }
    }
}

/// The one key `signer` signs the document with (`requests::flow_keys`
/// narrowed it to the flow's key).
fn signing_key(
    identity: &Identity,
    signer: &impl Signer<IdentityPublicKey>,
) -> Result<IdentityPublicKey, PlatformError> {
    (identity.public_keys().values())
        .find(|k| signer.can_sign_with(k))
        .cloned()
        .ok_or_else(|| {
            IdentityError::KeysMissing {
                purpose: KeyPurpose::Authentication,
            }
            .into()
        })
}

impl NetworkSession {
    /// `DashPay::profile` (D11): an own identity's from the identity read,
    /// another user's from the contacts read model under the main identity
    /// first, then the others by index. A stale read with nothing to answer
    /// from is `platform.unavailable`.
    pub(super) fn profile_of(
        &self,
        id: WalletId,
        identity: &str,
    ) -> Result<Option<Profile>, PlatformError> {
        let user = identifier(identity).map_err(platform)?;
        let read = self.identity_read(id)?;
        let user_b58 = user.to_string(Encoding::Base58);
        if let Some(own) = read.summaries.iter().find(|s| s.identity == user_b58) {
            return Ok(own.profile.clone());
        }
        let mut owners: Vec<_> = read.summaries.iter().collect();
        owners.sort_by_key(|s| !s.is_main);
        let owners: Vec<Identifier> = (owners.into_iter())
            .filter_map(|s| Identifier::from_string(&s.identity, Encoding::Base58).ok())
            .collect();
        let _op = self.try_enter()?;
        match self.shown_profile(id, &user, &owners)? {
            Some(profile) if profile.is_some() || read.live || !read.summaries.is_empty() => {
                Ok(profile)
            }
            _ => Err(PlatformError::Unavailable),
        }
    }

    /// Expires every contact profile the library caches for the wallet's
    /// identities, in its memory only, so its next DashPay pass fetches them
    /// all again (D12). Waits for the library's lock. Identities the wallet
    /// only watches keep theirs, as the contacts model shows none of them.
    pub(crate) async fn expire_contact_profiles(&self, manager: &Manager, id: WalletId) {
        let Some(wallet) = manager.get_wallet(&id.0).await else {
            return;
        };
        let mut state = wallet.state_mut().await;
        let owners: Vec<Identifier> = (state.identity_manager.wallet_managed_identities(&id.0))
            .map(|m| m.identity.id())
            .collect();
        for owner in owners {
            if let Some(m) = state.identity_manager.managed_identity_mut(&owner) {
                for entry in m.dashpay_contact_profiles_mut().values_mut() {
                    entry.checked_at_ms = 0;
                }
            }
        }
    }

    /// [`Self::expire_contact_profiles`] in the background: the tap's hook,
    /// called inside the library's store, where its lock is held.
    pub(crate) fn expire_contact_profiles_soon(self: &Arc<Self>, id: WalletId) {
        let weak = Arc::downgrade(self);
        let task = async move {
            let Some(this) = weak.upgrade() else { return };
            let Ok(manager) = this.manager() else { return };
            this.expire_contact_profiles(&manager, id).await;
        };
        self.platform.contacts.spawn(&self.rt, task);
    }
}
