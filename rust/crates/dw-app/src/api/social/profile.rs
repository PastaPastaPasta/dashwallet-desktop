//! Profiles and avatars.

use dw_engine::platform::{
    AvatarCandidate, AvatarChange, AvatarImage, AvatarSize, AvatarSource, CropRect, Profile,
    ProfileEdit, ProfileLimits,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPayProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_url: Option<String>,
    /// Hex SHA-256 of the image bytes, as published.
    pub avatar_hash: Option<String>,
    /// Hex dHash, as published.
    pub avatar_fingerprint: Option<String>,
    pub updated_at: Option<u64>,
}

impl From<Profile> for DashPayProfile {
    fn from(p: Profile) -> Self {
        let Profile {
            display_name,
            public_message,
            avatar_url,
            avatar_hash,
            avatar_fingerprint,
            updated_at,
        } = p;
        DashPayProfile {
            display_name,
            public_message,
            avatar_url,
            avatar_hash,
            avatar_fingerprint,
            updated_at,
        }
    }
}

/// Character limits from the DashPay contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPayProfileLimits {
    pub display_name_max: u32,
    pub public_message_max: u32,
}

impl From<ProfileLimits> for DashPayProfileLimits {
    fn from(l: ProfileLimits) -> Self {
        let ProfileLimits {
            display_name_max,
            public_message_max,
        } = l;
        DashPayProfileLimits {
            display_name_max,
            public_message_max,
        }
    }
}

/// What `dashpayUpdateProfile` does to the avatar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AvatarChangeArgs {
    Keep,
    Remove,
    /// A candidate from `dashpayPrepareAvatar`.
    Set { candidate: String },
}

impl From<AvatarChangeArgs> for AvatarChange {
    fn from(c: AvatarChangeArgs) -> Self {
        match c {
            AvatarChangeArgs::Keep => AvatarChange::Keep,
            AvatarChangeArgs::Remove => AvatarChange::Remove,
            AvatarChangeArgs::Set { candidate } => AvatarChange::Set { candidate },
        }
    }
}

/// The whole new profile; `null` clears a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEditArgs {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar: AvatarChangeArgs,
}

impl From<ProfileEditArgs> for ProfileEdit {
    fn from(e: ProfileEditArgs) -> Self {
        let ProfileEditArgs {
            display_name,
            public_message,
            avatar,
        } = e;
        ProfileEdit {
            display_name,
            public_message,
            avatar: avatar.into(),
        }
    }
}

/// Pixels of the source image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CropArgs {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl From<CropArgs> for CropRect {
    fn from(c: CropArgs) -> Self {
        let CropArgs {
            x,
            y,
            width,
            height,
        } = c;
        CropRect {
            x,
            y,
            width,
            height,
        }
    }
}

/// Where a new avatar comes from. `Debug` shows a file's size and hides a
/// Gravatar e-mail address.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AvatarSourceArgs {
    /// Image bytes (base64); the engine bounds their size and re-encodes them.
    File {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::base64")]
        bytes: Vec<u8>,
        crop: Option<CropArgs>,
    },
    Url {
        url: String,
    },
    Gravatar {
        email: String,
    },
}

impl std::fmt::Debug for AvatarSourceArgs {
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

impl From<AvatarSourceArgs> for AvatarSource {
    fn from(s: AvatarSourceArgs) -> Self {
        match s {
            AvatarSourceArgs::File { bytes, crop } => AvatarSource::File {
                bytes,
                crop: crop.map(Into::into),
            },
            AvatarSourceArgs::Url { url } => AvatarSource::Url { url },
            AvatarSourceArgs::Gravatar { email } => AvatarSource::Gravatar { email },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AvatarSizeName {
    /// 128 px.
    Small,
    /// 256 px.
    Large,
}

mirror_enum!(AvatarSize => AvatarSizeName { Small, Large });
mirror_enum!(AvatarSizeName => AvatarSize { Small, Large });

/// An engine-re-encoded PNG thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AvatarThumbnail {
    /// PNG bytes (base64).
    #[ts(type = "string")]
    #[serde(with = "crate::dto::base64")]
    pub png: Vec<u8>,
    pub size: AvatarSizeName,
}

impl From<AvatarImage> for AvatarThumbnail {
    fn from(i: AvatarImage) -> Self {
        let AvatarImage { png, size } = i;
        AvatarThumbnail {
            png,
            size: size.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AvatarCandidateInfo {
    /// Pass it to `dashpayUploadAvatar` or as `avatar.candidate` of an edit.
    pub id: String,
    pub preview: AvatarThumbnail,
    /// Set for `url` and `gravatar` sources, and after an upload.
    pub url: Option<String>,
    /// A `file` source needs `dashpayUploadAvatar` before it can be published.
    pub needs_upload: bool,
}

impl From<AvatarCandidate> for AvatarCandidateInfo {
    fn from(c: AvatarCandidate) -> Self {
        let AvatarCandidate {
            id,
            preview,
            url,
            needs_upload,
        } = c;
        AvatarCandidateInfo {
            id,
            preview: preview.into(),
            url,
            needs_upload,
        }
    }
}
