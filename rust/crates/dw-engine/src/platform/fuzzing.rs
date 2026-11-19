//! The engine's parsers of untrusted input, for dw-fuzz (H-02); built only
//! with the `fuzzing` feature.

use super::avatar_image::{self, Crop};
use super::profile::{AvatarSize, CropRect};

/// Fetched avatar bytes through [`avatar_image::ingest`], with Android's
/// zoom crop when `zoom` parses, and picked file bytes through
/// [`avatar_image::ingest_file`] with the pixel crop `rect`. Panics when a
/// thumbnail is not a square PNG of its size.
pub fn avatar(bytes: &[u8], zoom: &str, rect: [u32; 4], fingerprint: bool) {
    let crop = Crop::from_zoom(zoom);
    if let Ok(ingested) = avatar_image::ingest(bytes, crop, fingerprint) {
        assert_eq!(ingested.dhash.is_some(), fingerprint);
        for size in [AvatarSize::Small, AvatarSize::Large] {
            assert!(avatar_image::is_thumbnail(
                ingested.thumbnail(size),
                size.px()
            ));
        }
    }
    let [x, y, width, height] = rect;
    let rect = CropRect {
        x,
        y,
        width,
        height,
    };
    if let Ok((upload, _)) = avatar_image::ingest_file(bytes, Some(rect)) {
        assert_eq!(avatar_image::sniff(&upload), Some(image::ImageFormat::Png));
    }
}

/// An invitation link through the claim's parser: DP5-01's normalization,
/// then the library's `parse_invitation_uri` (the voucher key, the funding
/// transaction, the inviter).
pub fn invitation(text: &str) -> bool {
    super::invitation_check::parse(text).is_some()
}
