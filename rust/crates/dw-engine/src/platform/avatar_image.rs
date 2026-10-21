//! Avatar bytes to thumbnails (DP4-02 design, "Decoding and re-encoding"):
//! the format sniffed from the bytes, a bounded pure-Rust decode, the
//! library's hash, the mobile wallets' dHash (BL-190), and 128 / 256 px PNG
//! thumbnails. Everything here is synchronous and CPU-bound; `avatar.rs`
//! runs it on a blocking thread through [`ingest`], which also turns a
//! decoder panic into `avatar.unsupported`. Not part of the facade.

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};
use platform_wallet::wallet::identity::calculate_avatar_hash;

use super::avatar_dhash::mobile_dhash;

use super::errors::AvatarError;
use super::profile::{AvatarSize, CropRect, Profile};

/// The largest picture decoded, per side (design: "Decoding").
pub(super) const MAX_SIDE: u32 = 4096;
/// The decoder's allocation budget.
const MAX_ALLOC: u64 = 128 << 20;
/// The largest decoded picture, in bytes: 4096 x 4096 8-bit RGBA. A
/// 16-bit picture that large is refused; so the output of each decode stays
/// within this.
const MAX_DECODED: u64 = 64 << 20;
/// Pictures larger than this per side are shrunk (fast) before the
/// Lanczos3 resize, which is slow on large inputs.
const PRESHRINK: u32 = 1024;
/// The image a `File` candidate would upload, at most this per side (D18).
const UPLOAD_SIDE: u32 = 512;

impl AvatarSize {
    pub(super) fn px(self) -> u32 {
        match self {
            Self::Small => 128,
            Self::Large => 256,
        }
    }
}

/// What to cut from the picture before the square thumbnail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Crop {
    /// A `File` source's rectangle, in pixels of the (oriented) picture.
    Pixels(CropRect),
    /// Android's `dashpay-profile-pic-zoom=l,t,r,b`, fractions (D15).
    Fractions([f64; 4]),
}

impl Crop {
    /// The canonical text of a crop, for the thumbnail's file stem.
    pub(super) fn key(&self) -> String {
        match self {
            Self::Pixels(r) => format!("px:{},{},{},{}", r.x, r.y, r.width, r.height),
            Self::Fractions(f) => format!("zoom:{},{},{},{}", f[0], f[1], f[2], f[3]),
        }
    }

    /// Android's zoom parameter, or `None` when it is malformed: four
    /// finite fractions with `0 <= l < r <= 1` and `0 <= t < b <= 1`.
    pub(super) fn from_zoom(value: &str) -> Option<Self> {
        let parts: Vec<f64> = (value.split(','))
            .map(|p| p.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect::<Option<_>>()?;
        let [l, t, r, b] = <[f64; 4]>::try_from(parts).ok()?;
        let ok = |lo: f64, hi: f64| (0.0..=1.0).contains(&lo) && lo < hi && hi <= 1.0;
        (ok(l, r) && ok(t, b)).then_some(Self::Fractions([l, t, r, b]))
    }
}

/// The formats accepted (D8), from their first bytes.
pub(super) fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else {
        None
    }
}

fn image_error(e: ImageError) -> AvatarError {
    match e {
        ImageError::Limits(_) => AvatarError::TooLarge,
        _ => AvatarError::Unsupported,
    }
}

fn limits(side: u32) -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(side);
    limits.max_image_height = Some(side);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

/// Decodes `bytes` within the limits: PNG, JPEG or GIF (its first frame,
/// D9), JPEG's EXIF orientation applied.
pub(super) fn decode(bytes: &[u8]) -> Result<DynamicImage, AvatarError> {
    let (mut image, orientation) = decode_unoriented(bytes)?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// [`decode`] without applying the EXIF orientation, which it returns.
fn decode_unoriented(bytes: &[u8]) -> Result<(DynamicImage, Orientation), AvatarError> {
    let format = sniff(bytes).ok_or(AvatarError::Unsupported)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits(MAX_SIDE));
    let mut decoder = reader.into_decoder().map_err(image_error)?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 {
        return Err(AvatarError::Unsupported);
    }
    // `from_decoder` does not hold its output buffer to `max_alloc`.
    if decoder.total_bytes() > MAX_DECODED {
        return Err(AvatarError::TooLarge);
    }
    let orientation = decoder.orientation().map_err(image_error)?;
    let image = DynamicImage::from_decoder(decoder).map_err(image_error)?;
    Ok((image, orientation))
}

/// The square thumbnail of `image` at `px`, as 8-bit RGBA PNG.
fn thumbnail(image: &DynamicImage, px: u32) -> Result<Vec<u8>, AvatarError> {
    png(&image.resize_to_fill(px, px, FilterType::Lanczos3))
}

/// `image` as an 8-bit RGBA PNG.
fn png(image: &DynamicImage) -> Result<Vec<u8>, AvatarError> {
    let mut out = Vec::new();
    (image.to_rgba8())
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|_| AvatarError::Unsupported)?;
    Ok(out)
}

/// `image` cut to `crop`. A pixel rectangle outside the picture is
/// `invalid_argument`; fractions are clamped to at least one pixel.
fn cropped(image: DynamicImage, crop: Option<Crop>) -> Result<DynamicImage, AvatarError> {
    let (w, h) = (image.width(), image.height());
    let (x, y, cw, ch) = match crop {
        None => return Ok(image),
        Some(Crop::Pixels(r)) => {
            let inside = r.width > 0
                && r.height > 0
                && r.x.checked_add(r.width).is_some_and(|e| e <= w)
                && r.y.checked_add(r.height).is_some_and(|e| e <= h);
            if !inside {
                return Err(AvatarError::Platform(
                    super::errors::PlatformError::InvalidArgument {
                        detail: format!("the crop is outside the {w} x {h} picture"),
                    },
                ));
            }
            (r.x, r.y, r.width, r.height)
        }
        Some(Crop::Fractions([l, t, r, b])) => {
            let at = |f: f64, n: u32| ((f * f64::from(n)).round() as u32).min(n.saturating_sub(1));
            let (x, y) = (at(l, w), at(t, h));
            let cw = (((r - l) * f64::from(w)).round() as u32).clamp(1, w - x);
            let ch = (((b - t) * f64::from(h)).round() as u32).clamp(1, h - y);
            (x, y, cw, ch)
        }
    };
    Ok(image.crop_imm(x, y, cw, ch))
}

/// A large picture shrunk (box filter) to at most [`PRESHRINK`] per side.
fn preshrunk(image: DynamicImage) -> DynamicImage {
    if image.width() > PRESHRINK || image.height() > PRESHRINK {
        image.thumbnail(PRESHRINK, PRESHRINK)
    } else {
        image
    }
}

/// What the pipeline keeps of a picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ingested {
    /// SHA-256 of the bytes, as the library computes `avatarHash`.
    pub(super) sha: [u8; 32],
    /// The mobile wallets' dHash of the bytes (`avatarFingerprint`,
    /// BL-190), made only for a candidate: a read does not compare it (D32).
    pub(super) dhash: Option<[u8; 8]>,
    pub(super) small: Vec<u8>,
    pub(super) large: Vec<u8>,
}

impl Ingested {
    pub(super) fn thumbnail(&self, size: AvatarSize) -> &[u8] {
        match size {
            AvatarSize::Small => &self.small,
            AvatarSize::Large => &self.large,
        }
    }
}

/// Fetched (or file) bytes to their hash, thumbnails and, with
/// `fingerprint` (a candidate's), the mobile wallets' dHash of the whole
/// picture as decoded, before its EXIF orientation and any crop (as Android
/// computes it, BL-190). The bytes are decoded once, within the limits. A
/// panic anywhere is `avatar.unsupported`.
pub(super) fn ingest(
    bytes: &[u8],
    crop: Option<Crop>,
    fingerprint: bool,
) -> Result<Ingested, AvatarError> {
    guarded(|| {
        let (mut image, orientation) = decode_unoriented(bytes)?;
        let dhash = fingerprint.then(|| mobile_dhash(&image));
        image.apply_orientation(orientation);
        let image = preshrunk(cropped(image, crop)?);
        Ok(Ingested {
            sha: calculate_avatar_hash(bytes),
            dhash,
            small: thumbnail(&image, AvatarSize::Small.px())?,
            large: thumbnail(&image, AvatarSize::Large.px())?,
        })
    })
}

/// A `File` source: the PNG to upload (D18) and what [`ingest`] makes of
/// it, so the candidate's hash and fingerprint are those of the upload.
pub(super) fn ingest_file(
    bytes: &[u8],
    crop: Option<CropRect>,
) -> Result<(Vec<u8>, Ingested), AvatarError> {
    let upload = guarded(|| {
        let image = cropped(decode(bytes)?, crop.map(Crop::Pixels))?;
        let image = if image.width() > UPLOAD_SIDE || image.height() > UPLOAD_SIDE {
            image.resize(UPLOAD_SIDE, UPLOAD_SIDE, FilterType::Lanczos3)
        } else {
            image
        };
        png(&image)
    })?;
    let ingested = ingest(&upload, None, true)?;
    Ok((upload, ingested))
}

fn guarded<T>(body: impl FnOnce() -> Result<T, AvatarError>) -> Result<T, AvatarError> {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(Err(AvatarError::Unsupported))
}

/// Whether `png` is a PNG thumbnail of `px` × `px` (a file read back from
/// the cache, D10).
pub(super) fn is_thumbnail(png: &[u8], px: u32) -> bool {
    guarded(|| {
        let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
        reader.limits(limits(px));
        let image = reader.decode().map_err(image_error)?;
        Ok(image.width() == px && image.height() == px)
    })
    .unwrap_or(false)
}

/// Whether `profile` publishes a hash and a fingerprint that could match
/// anything: 64 and 16 hex digits. Without them no fetch can verify.
pub(super) fn verifiable(profile: &Profile) -> bool {
    let hex = |s: &Option<String>, n: usize| {
        s.as_ref()
            .is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    hex(&profile.avatar_hash, 64) && hex(&profile.avatar_fingerprint, 16)
}

/// The verdict on content fetched for `profile` (design step 5): its
/// SHA-256 must be the profile's `avatarHash`. The fingerprint is not
/// compared (D6, D32: the mobile wallets' dHash is another algorithm, up to
/// 35 bits off for the very bytes `avatarHash` names), only required to be
/// well formed ([`verifiable`], D26).
pub(super) fn matches(content_sha: &str, profile: &Profile) -> bool {
    profile
        .avatar_hash
        .as_ref()
        .is_some_and(|hash| hash.len() == 64 && hash.eq_ignore_ascii_case(content_sha))
}
