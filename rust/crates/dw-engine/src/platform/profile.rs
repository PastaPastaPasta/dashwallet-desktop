//! Profiles and avatars (DP4-01, DP4-02). The avatar pipeline itself
//! (fetch guard, verification, re-encoding, cache) is `avatar.rs` and its
//! siblings.

use serde::{Deserialize, Serialize};

use super::avatar::LOAD_PICTURES_KEY;
use super::dashpay::DashPay;
use super::errors::{AvatarError, PlatformError};
use super::profiles;
use super::requests::{RequestNet, SdkRequests};
use crate::NetworkSession;
use crate::session::Manager;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_url: Option<String>,
    /// Hex SHA-256 of the image bytes, as published.
    pub avatar_hash: Option<String>,
    /// Hex dHash, as published.
    pub avatar_fingerprint: Option<String>,
    pub updated_at: Option<u64>,
}

/// Character limits from the DashPay contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLimits {
    pub display_name_max: u32,
    pub public_message_max: u32,
}

/// A profile text the contract limits (`profile.too_long`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileField {
    DisplayName,
    PublicMessage,
}

/// The whole new profile; `None` clears a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileEdit {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar: AvatarChange,
    /// What the edit was made against (D23): with one, an edit of a profile
    /// that has changed on Platform since is `profile.conflict`; without
    /// one it replaces whatever Platform holds. Absent in JSON is `None`.
    pub baseline: Option<ProfileBaseline>,
}

/// The profile an edit was made against: what `profile` showed (`None`:
/// no profile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBaseline {
    pub profile: Option<Profile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarChange {
    Keep,
    Remove,
    /// A candidate from `prepare_avatar`.
    Set {
        candidate: String,
    },
}

/// Input only: it never serializes, and `Debug` shows the byte count and
/// hides the e-mail address (§3.8: Gravatar e-mails are never stored).
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarSource {
    File {
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
        crop: Option<CropRect>,
    },
    Url {
        url: String,
    },
    Gravatar {
        email: String,
    },
}

impl std::fmt::Debug for AvatarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File { bytes, crop } => f
                .debug_struct("File")
                .field("bytes", &bytes.len())
                .field("crop", crop)
                .finish(),
            Self::Url { url } => f.debug_struct("Url").field("url", url).finish(),
            Self::Gravatar { .. } => f.write_str("Gravatar { .. }"),
        }
    }
}

/// Pixels of the source image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarCandidate {
    pub id: String,
    pub preview: AvatarImage,
    /// Set for `Url` and `Gravatar` sources, and after an upload.
    pub url: Option<String>,
    /// A `File` source needs `upload_avatar` before it can be published.
    pub needs_upload: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarSize {
    /// 128 px.
    Small,
    /// 256 px.
    Large,
}

/// An engine-re-encoded PNG thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarImage {
    #[serde(with = "base64_bytes")]
    pub png: Vec<u8>,
    pub size: AvatarSize,
}

/// Byte payloads as standard base64 strings, not JSON number arrays.
mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

impl DashPay {
    /// The profile of `identity` (DP4-01): an own identity's as
    /// `identities()` shows it, another user's as the contacts read model
    /// holds it, `None` when the library has none. From memory; a stale read
    /// with nothing to answer from is `platform.unavailable`.
    pub fn profile(&self, identity: String) -> Result<Option<Profile>, PlatformError> {
        self.session.profile_of(self.wallet_id(), &identity)
    }

    /// The display name's and public message's `maxLength` in the DashPay
    /// contract the library bundles. Reads no wallet.
    pub fn profile_limits(&self) -> Result<ProfileLimits, PlatformError> {
        Ok(profiles::limits())
    }

    /// Publishes `edit` as the identity's whole profile (DP4-01): texts are
    /// trimmed, an empty one clears its field, one past its limit is
    /// `profile.too_long`, and a profile left with no field at all is
    /// `profile.empty`. The avatar is kept, removed, or set from a
    /// `prepare_avatar` candidate with a URL (DP4-02). An edit that changes
    /// nothing is `Ok` with nothing sent.
    /// `grant` is a `PlatformOp` grant (or a `ProfileEdit` lease).
    pub async fn update_profile(
        &self,
        identity: String,
        edit: ProfileEdit,
        grant: String,
    ) -> Result<(), PlatformError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, manager, wallet| async move {
            let net = SdkRequests(manager.sdk_arc());
            profiles::update(&net, &session, &wallet, (wallet_id, &identity), edit, grant).await
        })
        .await
    }
}

impl DashPay {
    /// Turns a picture into a candidate for `update_profile` (DP4-02): a
    /// `Url` or `Gravatar` source is fetched through the guard now, a
    /// `File` decoded and cropped; each is re-encoded, and its hash and
    /// fingerprint taken. The candidate lives in the session for 30
    /// minutes.
    pub async fn prepare_avatar(&self, src: AvatarSource) -> Result<AvatarCandidate, AvatarError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, _, _| async move {
            session.platform.avatars.prepare(wallet_id, src).await
        })
        .await
    }

    /// Whether `upload_avatar` can run: `false` until an Imgur client id
    /// is configured (B1, DP4-02 D2).
    pub fn avatar_upload_available(&self) -> Result<bool, AvatarError> {
        Ok(false)
    }

    /// Uploads a `File` candidate (DP4-02 D2): `avatar.upload_unconfigured`
    /// until B1; an unknown candidate, or one with a URL already, is
    /// `invalid_argument`.
    pub async fn upload_avatar(&self, candidate: String) -> Result<String, AvatarError> {
        let wallet_id = self.wallet_id();
        self.on_wallet(move |session, _, _| async move {
            session.platform.avatars.upload(wallet_id, &candidate)
        })
        .await
    }

    /// The thumbnail of any identity's avatar, verified against its
    /// profile's hash (DP4-02 D32): `None` without an avatar,
    /// or for another user's while "Load contact pictures" is off; a
    /// picture that does not match is `avatar.hash_mismatch`.
    pub async fn avatar(
        &self,
        identity: String,
        size: AvatarSize,
    ) -> Result<Option<AvatarImage>, AvatarError> {
        let net = |manager: &Manager| Box::new(SdkRequests(manager.sdk_arc()));
        self.avatar_through(net, identity, size).await
    }

    /// [`Self::avatar`] reading Platform through `net` (tests: a fake).
    /// Delivered here, once the read's task has joined: another user's
    /// answer read before an off that has since been published is `None`
    /// (DP4-02 D40).
    pub(super) async fn avatar_through<N>(
        &self,
        net: impl FnOnce(&Manager) -> N + Send + 'static,
        identity: String,
        size: AvatarSize,
    ) -> Result<Option<AvatarImage>, AvatarError>
    where
        N: std::ops::Deref<Target: RequestNet + Sized> + Send + Sync + 'static,
    {
        let wallet_id = self.wallet_id();
        let answer = self
            .on_wallet(move |session, manager, _| async move {
                let net = net(&manager);
                Ok::<_, AvatarError>(session.avatar_of(&*net, wallet_id, &identity, size).await)
            })
            .await?;
        let pictures = self.session.platform.avatars.toggle();
        pictures.at("delivery").await;
        answer.deliver(&pictures)
    }
}

impl NetworkSession {
    /// "Load contact pictures" (DP4-02 D17): on unless turned off. From
    /// memory.
    pub fn load_contact_pictures(&self) -> Result<bool, PlatformError> {
        let _op = self.try_enter().map_err(PlatformError::from)?;
        Ok(self.platform.avatars.pictures())
    }

    /// Turns "Load contact pictures" on or off for this network: stored in
    /// app.sqlite and applied at once. Off, no other user's avatar is
    /// fetched or shown from the moment this returns: reads already running
    /// have ended (DP4-02 D33, D36). Calls at once apply in turn, and the
    /// session holds what was stored last (D34), a call dropped mid-way
    /// included: once its turn came, it runs to its end (D37).
    pub async fn set_load_contact_pictures(&self, on: bool) -> Result<(), PlatformError> {
        let _op = self.enter().await.map_err(PlatformError::from)?;
        let db = self.live().map_err(PlatformError::from)?.appdb;
        let value = (!on).then_some("0");
        let write = crate::coins::appdb_job(db, move |db| {
            db.set_setting(dw_appdb::GLOBAL_SCOPE, LOAD_PICTURES_KEY, value)
        });
        let turn = self.platform.avatars.toggle().begin_transition().await;
        self.on_runtime(turn.apply(on, write)).await?;
        Ok(())
    }
}
