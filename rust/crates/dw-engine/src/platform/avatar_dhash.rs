//! The `avatarFingerprint` this wallet publishes (BL-190): the mobile
//! wallets' dHash, not the library's. dash-wallet (Android, `CocoaImageDHash`,
//! after CocoaImageHashing's dHash) decodes the picture with `BitmapFactory`
//! (EXIF orientation not applied), scales it to 9 × 9 with
//! `Bitmap.createScaledBitmap(…, filter = true)`, and sets bit `x` of byte
//! `y` where pixel `(x, y)` is brighter than `(x + 1, y)`, rows 0–7, brightness
//! being `(r + g + b) / 3` of the colour over black. The scale is Skia's filtered
//! bitmap draw as Android shipped it (`SkBitmapProcState`): the pixel centre
//! mapped back in floats, 16.16 fixed point less half a pixel, clamped to the
//! picture, and bilinear weights of 4 bits on premultiplied colour,
//! truncated. Of the 43 testnet avatars Android set whose bytes match
//! `avatarHash`, this gives the published fingerprint bit for bit on 38 from
//! the engine's decoder, on 4 more from libjpeg-turbo's pixels (the engine's
//! pure-Rust decoder rounds some up to 2 levels apart), and on the last from
//! libjpeg's fast IDCT, which older Android used
//! (`tests/fixtures/bl190_avatar_fingerprints.json`, DP4-03 D27).
//!
//! The library's dHash (9 × 8 Lanczos3 of luma) differs from those by 12 to
//! 35 bits (DP4-02 D32); reads compare neither.

use image::{DynamicImage, GenericImageView};

/// The grid the picture is scaled to: 9 columns give 8 differences a row.
const GRID: u32 = 9;

/// The fingerprint of `image` as decoded, before any EXIF orientation.
pub(super) fn mobile_dhash(image: &DynamicImage) -> [u8; 8] {
    let (w, h) = image.dimensions();
    dhash_of(w, h, |x, y| image.get_pixel(x, y).0)
}

/// [`mobile_dhash`] of a `w` × `h` picture whose straight (not
/// premultiplied) RGBA `pixel` gives.
pub(super) fn dhash_of(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> [u8; 8] {
    let xs: Vec<Tap> = (0..GRID).map(|i| Tap::of(i, w)).collect();
    let ys: Vec<Tap> = (0..GRID).map(|i| Tap::of(i, h)).collect();
    let premultiplied = |x, y| premultiply(pixel(x, y));
    let mut grey = [[0u32; GRID as usize]; GRID as usize];
    for (row, ty) in grey.iter_mut().zip(&ys) {
        for (cell, tx) in row.iter_mut().zip(&xs) {
            *cell = grey_of(filter(
                tx.sub,
                ty.sub,
                [
                    premultiplied(tx.near, ty.near),
                    premultiplied(tx.far, ty.near),
                    premultiplied(tx.near, ty.far),
                    premultiplied(tx.far, ty.far),
                ],
            ));
        }
    }
    let mut out = [0u8; 8];
    for (byte, row) in out.iter_mut().zip(&grey) {
        for x in 0..8 {
            if row[x] > row[x + 1] {
                *byte |= 1 << x;
            }
        }
    }
    out
}

/// The two source pixels a grid column (or row) blends, and the far one's
/// weight in sixteenths (`ClampX_ClampY_filter_scale`'s packing).
struct Tap {
    near: u32,
    far: u32,
    sub: u32,
}

impl Tap {
    fn of(i: u32, n: u32) -> Self {
        // The inverse of the draw's scale, as `SkMatrix::invert` makes it.
        let inverse = 1.0f32 / (GRID as f32 / n as f32);
        let centre = (i as f32 + 0.5) * inverse;
        // `SkScalarToFixed` truncates; a picture is at most 4096 pixels a
        // side, so this stays far inside `i32`.
        let f = (centre * 65536.0) as i32 - 0x8000;
        let last = n as i32 - 1;
        Self {
            near: (f >> 16).clamp(0, last) as u32,
            far: ((f + 0x10000) >> 16).clamp(0, last) as u32,
            sub: ((f >> 12) & 0xF) as u32,
        }
    }
}

/// `SkPremultiplyARGBInline`: each colour channel times alpha / 255, rounded.
fn premultiply([r, g, b, a]: [u8; 4]) -> [u32; 4] {
    let a = u32::from(a);
    let times = |c: u8| {
        let product = u32::from(c) * a + 128;
        (product + (product >> 8)) >> 8
    };
    [times(r), times(g), times(b), a]
}

/// `Filter_32_opaque`: the four pixels (near-near, far-near, near-far,
/// far-far, as x-y) weighted in 256ths and truncated.
fn filter(x: u32, y: u32, px: [[u32; 4]; 4]) -> [u32; 4] {
    let xy = x * y;
    let weights = [256 + xy - 16 * y - 16 * x, 16 * x - xy, 16 * y - xy, xy];
    let mut out = [0u32; 4];
    for (channel, value) in out.iter_mut().enumerate() {
        let sum: u32 = px.iter().zip(weights).map(|(p, w)| p[channel] * w).sum();
        *value = sum >> 8;
    }
    out
}

/// `getPixel`'s straight colour, then `CocoaImageDHash.greyscale` times 3:
/// each channel times alpha / 255, truncated, summed.
fn grey_of([r, g, b, a]: [u32; 4]) -> u32 {
    if a == 0 {
        return 0;
    }
    let channel = |c: u32| ((c * 255 + a / 2) / a).min(255) * a / 255;
    channel(r) + channel(g) + channel(b)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde::Deserialize;

    use super::*;

    /// One testnet profile (`tests/fixtures/bl190_avatar_fingerprints.json`).
    #[derive(Deserialize)]
    struct Vector {
        owner: String,
        width: u32,
        height: u32,
        /// `avatarFingerprint` as published, hex.
        published: String,
        /// Who made it: `android` (equal), `android-libjpeg` (equal from
        /// libjpeg-turbo's pixels, not the engine decoder's), `android-ifast`
        /// (equal from libjpeg's fast IDCT), `library`, `other` (D27).
        maker: String,
        /// The bits the engine's fingerprint differs by.
        distance: u32,
        /// The columns and rows the grid reads, and their straight RGBA
        /// pixels, row by row, in hex.
        xs: Vec<u32>,
        ys: Vec<u32>,
        rgba: String,
    }

    impl Vector {
        fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
            let at = |axis: &[u32], v| axis.iter().position(|&a| a == v);
            let (Some(i), Some(j)) = (at(&self.xs, x), at(&self.ys, y)) else {
                panic!("{}: ({x}, {y}) is not a pixel the grid reads", self.owner);
            };
            let k = (j * self.xs.len() + i) * 8;
            hex::decode(&self.rgba[k..k + 8])
                .unwrap()
                .try_into()
                .unwrap()
        }
    }

    fn vectors() -> Vec<Vector> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/bl190_avatar_fingerprints.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn distance(a: [u8; 8], b: &[u8]) -> u32 {
        a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
    }

    /// D27: every byte-exact testnet avatar. Android's are equal bit for bit
    /// but for five JPEGs decoded otherwise (1 to 3 bits); the library's and
    /// one other client's are not Android's algorithm.
    #[test]
    fn equals_android_on_the_testnet_avatars_android_set() {
        let vectors = vectors();
        assert_eq!(vectors.len(), 53, "the byte-exact testnet avatars");
        for v in &vectors {
            let ours = dhash_of(v.width, v.height, |x, y| v.pixel(x, y));
            let published = hex::decode(&v.published).unwrap();
            assert_eq!(
                distance(ours, &published),
                v.distance,
                "{} ({})",
                v.owner,
                v.maker
            );
            match v.maker.as_str() {
                "android" => assert_eq!(hex::encode(ours), v.published, "{}", v.owner),
                "android-libjpeg" | "android-ifast" => assert!(v.distance <= 3, "{}", v.owner),
                "library" | "other" => assert!(v.distance >= 12, "{}", v.owner),
                m => panic!("{}: maker {m}", v.owner),
            }
        }
        let count = |m: &str| vectors.iter().filter(|v| v.maker == m).count();
        let makers = [
            "android",
            "android-libjpeg",
            "android-ifast",
            "library",
            "other",
        ];
        assert_eq!(makers.map(count), [38, 4, 1, 9, 1]);
    }

    #[test]
    fn a_uniform_picture_has_no_differences_and_a_ramp_all() {
        let flat = DynamicImage::new_rgb8(30, 20);
        assert_eq!(mobile_dhash(&flat), [0; 8]);
        // Darker to the right: every column is greyer than the next.
        let ramp = image::RgbImage::from_fn(90, 90, |x, _| image::Rgb([255 - x as u8 * 2; 3]));
        assert_eq!(mobile_dhash(&ramp.into()), [0xff; 8]);
    }

    #[test]
    fn transparent_pixels_count_as_black() {
        let half = image::RgbaImage::from_fn(18, 18, |x, _| {
            image::Rgba([200, 200, 200, if x < 9 { 255 } else { 0 }])
        });
        let opaque_black = image::RgbaImage::from_fn(18, 18, |x, _| {
            if x < 9 {
                image::Rgba([200, 200, 200, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        });
        assert_eq!(
            mobile_dhash(&half.into()),
            mobile_dhash(&opaque_black.into())
        );
    }

    #[test]
    fn a_one_pixel_picture_reads_only_that_pixel() {
        let one = image::RgbImage::from_pixel(1, 1, image::Rgb([9, 9, 9]));
        assert_eq!(mobile_dhash(&one.into()), [0; 8]);
        let taps = (0..GRID).map(|i| Tap::of(i, 4096));
        assert!(taps.into_iter().all(|t| t.near <= t.far && t.far <= 4095));
    }
}
