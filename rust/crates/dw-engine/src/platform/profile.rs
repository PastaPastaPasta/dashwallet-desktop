//! Profiles and avatars (DP4-01, DP4-02). The avatar pipeline itself
//! (fetch guard, verification, re-encoding, cache) goes in `avatar.rs`.

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::{AvatarError, PlatformError};
use super::profiles;
use super::requests::SdkRequests;

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
    /// `profile.empty`. The avatar is kept or removed; `Set` waits for
    /// DP4-02. An edit that changes nothing is `Ok` with nothing sent.
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

#[expect(unused_variables, reason = "stubs until DP4-02")]
impl DashPay {
    pub async fn prepare_avatar(&self, src: AvatarSource) -> Result<AvatarCandidate, AvatarError> {
        stub("DashPay.prepare_avatar")
    }

    pub fn avatar_upload_available(&self) -> Result<bool, AvatarError> {
        stub("DashPay.avatar_upload_available")
    }

    pub async fn upload_avatar(&self, candidate: String) -> Result<String, AvatarError> {
        stub("DashPay.upload_avatar")
    }

    pub async fn avatar(
        &self,
        identity: String,
        size: AvatarSize,
    ) -> Result<Option<AvatarImage>, AvatarError> {
        stub("DashPay.avatar")
    }
}
