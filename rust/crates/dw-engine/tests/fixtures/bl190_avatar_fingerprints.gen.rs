//! Builds `tests/fixtures/bl190_avatar_fingerprints.json` (BL-190, DP4-03 D27) from a
//! corpus of testnet avatars: `index.tsv` (owner, URL, published `avatarHash` and
//! `avatarFingerprint` in hex, HTTP status, SHA-256 of the bytes fetched, file, updatedAt)
//! and the fetched bytes. Kept beside the fixture as `bl190_avatar_fingerprints.gen.rs`;
//! to run it, copy it to `examples/` and:
//!
//! ```sh
//! cargo run -p dw-engine --example bl190_vectors -- <corpus dir> > tests/fixtures/bl190_avatar_fingerprints.json
//! ```
//!
//! For each avatar whose bytes match `avatarHash` it records the pixels the fingerprint's
//! grid reads (decoded as the engine decodes, before EXIF orientation), the published
//! fingerprint, who made it, and how many bits the engine's fingerprint differs by. The
//! grid's taps are `avatar_dhash.rs`'s, repeated here.

use image::GenericImageView;
use platform_wallet::wallet::identity::calculate_dhash_fingerprint;

/// Owners whose fingerprint the engine's grid gives bit for bit from libjpeg-turbo's decode
/// (`djpeg -dct int`, as Android's `BitmapFactory`), but not from the engine's pure-Rust
/// decoder (zune-jpeg), which rounds a pixel up to 2 levels apart: checked by hand (D27).
const LIBJPEG: &[&str] = &[
    "3sze1Jtr3PSXtXr3wcfA2KABC1W7fmcEufYrC1iZ2wGg",
    "BfTACNHZe3b31p61BejuTWnn5ZeD1x1ZDFpQsf69gV5H",
    "EEfj96h8qqHozKWj2yZU6fsQ8ffahppsSeRtLMKgvSQy",
    "EfPggELWzpqfw7EXQi42CEBKL7udAoWwEHcysEEuTGzb",
];
/// The same, with libjpeg's fast integer IDCT (`djpeg -dct fast`), which older Android
/// decoders used.
const IFAST: &[&str] = &["62ozXKF27zh55c3bpvgWn4h9Fkt2be1MzJgqP8PvqduW"];

fn taps(n: u32) -> Vec<(u32, u32, u32)> {
    (0..9)
        .map(|i| {
            let inverse = 1.0f32 / (9.0f32 / n as f32);
            let centre = (i as f32 + 0.5) * inverse;
            let f = (centre * 65536.0) as i32 - 0x8000;
            let last = n as i32 - 1;
            (
                (f >> 16).clamp(0, last) as u32,
                ((f + 0x10000) >> 16).clamp(0, last) as u32,
                ((f >> 12) & 0xF) as u32,
            )
        })
        .collect()
}

fn dhash(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> [u8; 8] {
    let pm = |x, y| {
        let [r, g, b, a] = px(x, y);
        let a = u32::from(a);
        let t = |c: u8| {
            let p = u32::from(c) * a + 128;
            (p + (p >> 8)) >> 8
        };
        [t(r), t(g), t(b), a]
    };
    let (xs, ys) = (taps(w), taps(h));
    let mut grey = [[0u32; 9]; 9];
    for (gy, &(y0, y1, sy)) in ys.iter().enumerate() {
        for (gx, &(x0, x1, sx)) in xs.iter().enumerate() {
            let xy = sx * sy;
            let w4 = [256 + xy - 16 * sy - 16 * sx, 16 * sx - xy, 16 * sy - xy, xy];
            let p4 = [pm(x0, y0), pm(x1, y0), pm(x0, y1), pm(x1, y1)];
            let c: Vec<u32> = (0..4)
                .map(|ch| p4.iter().zip(w4).map(|(p, w)| p[ch] * w).sum::<u32>() >> 8)
                .collect();
            let a = c[3];
            let ch = |v: u32| if a == 0 { 0 } else { ((v * 255 + a / 2) / a).min(255) * a / 255 };
            grey[gy][gx] = ch(c[0]) + ch(c[1]) + ch(c[2]);
        }
    }
    let mut out = [0u8; 8];
    for y in 0..8 {
        for x in 0..8 {
            if grey[y][x] > grey[y][x + 1] {
                out[y] |= 1 << x;
            }
        }
    }
    out
}

fn main() {
    let dir = std::env::args().nth(1).expect("corpus dir");
    let index = std::fs::read_to_string(format!("{dir}/index.tsv")).unwrap();
    let mut out = Vec::new();
    for line in index.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f[4] != "200" || f[2] != f[5] {
            continue;
        }
        let bytes = std::fs::read(f[6]).unwrap();
        let published = hex::decode(f[3]).unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        let (w, h) = image.dimensions();
        let ours = dhash(w, h, |x, y| image.get_pixel(x, y).0);
        let distance: u32 = ours.iter().zip(&published).map(|(a, b)| (a ^ b).count_ones()).sum();
        let maker = if calculate_dhash_fingerprint(&bytes).unwrap().as_slice() == published {
            "library"
        } else if distance == 0 {
            "android"
        } else if LIBJPEG.contains(&f[0]) {
            "android-libjpeg"
        } else if IFAST.contains(&f[0]) {
            "android-ifast"
        } else {
            "other"
        };
        // The grid reads the pixels at these columns and rows, row by row.
        let axis = |n| {
            let mut v: Vec<u32> = taps(n).iter().flat_map(|&(a, b, _)| [a, b]).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (xs, ys) = (axis(w), axis(h));
        let rgba: String = (ys.iter())
            .flat_map(|&y| xs.iter().map(move |&x| (x, y)))
            .map(|(x, y)| hex::encode(image.get_pixel(x, y).0))
            .collect();
        out.push(serde_json::json!({
            "owner": f[0],
            "url": f[1],
            "avatarHash": f[2],
            "updatedAt": f[7].parse::<u64>().ok(),
            "width": w,
            "height": h,
            "published": f[3],
            "maker": maker,
            "distance": distance,
            "xs": xs,
            "ys": ys,
            "rgba": rgba,
        }));
    }
    // One avatar a line.
    let lines: Vec<String> = out.iter().map(|v| serde_json::to_string(v).unwrap()).collect();
    println!("[\n{}\n]", lines.join(",\n"));
}
