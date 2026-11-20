//! Seed inputs for each target, made from the repository's test vectors
//! (`testdata/`, dw-uri's link corpus): the starting corpus of a fuzz run
//! and the inputs `tests/replay.rs` runs on stable.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, GrayImage, ImageFormat, Luma, Rgba, RgbaImage};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn testdata(rel: &str) -> PathBuf {
    repo().join("testdata").join(rel)
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn json(rel: &str) -> serde_json::Value {
    serde_json::from_slice(&read(&testdata(rel))).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Every string in `value`, at most 1 KiB each.
fn strings(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(s) if s.len() <= 1024 => {
            out.insert(s.clone());
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

/// The files of dw-uri's link corpus (one link per file).
fn link_corpus() -> Vec<Vec<u8>> {
    let dir = repo().join("rust/crates/dw-uri/testdata/link-corpus");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths.iter().map(|p| read(p)).collect()
}

fn texts(files: &[&str]) -> Vec<Vec<u8>> {
    let mut all = BTreeSet::new();
    for file in files {
        strings(&json(file), &mut all);
    }
    all.into_iter().map(String::into_bytes).collect()
}

fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    image.write_to(&mut Cursor::new(&mut out), format).unwrap();
    out
}

/// The QR symbols of `testdata/qr_cases.json` (libqrencode's matrices),
/// 4 px a module with a 4-module quiet zone, as PNG, JPEG and BMP.
fn qr_images() -> Vec<Vec<u8>> {
    let cases = json("qr_cases.json");
    let cases = cases["cases"].as_array().expect("qr_cases.json cases");
    let mut out = Vec::new();
    for case in cases.iter().take(6) {
        let rows: Vec<&str> = case["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|r| r.as_str().expect("row"))
            .collect();
        let (scale, quiet) = (4u32, 4u32);
        let side = (rows.len() as u32 + 2 * quiet) * scale;
        let grey = GrayImage::from_fn(side, side, |x, y| {
            let (mx, my) = (x / scale, y / scale);
            let dark = mx >= quiet
                && my >= quiet
                && rows
                    .get((my - quiet) as usize)
                    .and_then(|r| r.as_bytes().get((mx - quiet) as usize))
                    == Some(&b'1');
            Luma([if dark { 0 } else { 255 }])
        });
        let image = DynamicImage::ImageLuma8(grey);
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Bmp] {
            out.push(encode(&image, format));
        }
    }
    out
}

/// The PSBT vectors of dashd (`testdata/compat/psbt`), as base64 text and as
/// binary.
fn psbts() -> Vec<Vec<u8>> {
    use base64::Engine as _;
    let mut out = Vec::new();
    for name in ["unsigned.b64", "signed.b64"] {
        let text = read(&testdata(&format!("compat/psbt/{name}")));
        let binary = base64::engine::general_purpose::STANDARD
            .decode(String::from_utf8_lossy(&text).trim())
            .expect("PSBT vector is base64");
        out.push(text);
        out.push(binary);
    }
    out
}

fn dumps() -> Vec<Vec<u8>> {
    let mut paths: Vec<_> = std::fs::read_dir(testdata("dumpwallet"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    paths.sort();
    paths.iter().map(|p| read(p)).collect()
}

fn listdescriptors() -> Vec<Vec<u8>> {
    let file = read(&testdata("compat/listdescriptors_plain.json"));
    let value: serde_json::Value = serde_json::from_slice(&file).unwrap();
    let mut out = vec![file, value["descriptors"].to_string().into_bytes()];
    for d in value["descriptors"].as_array().expect("descriptors") {
        out.push(d["desc"].as_str().expect("desc").as_bytes().to_vec());
    }
    out
}

/// A `compactSize`-prefixed field, as [`crate::walletdat`] reads it.
pub fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    match bytes.len() {
        n @ 0..0xfd => out.push(n as u8),
        n @ 0xfd..=0xffff => {
            out.push(0xfd);
            out.extend_from_slice(&(n as u16).to_le_bytes());
        }
        n => {
            out.push(0xfe);
            out.extend_from_slice(&(n as u32).to_le_bytes());
        }
    }
    out.extend_from_slice(bytes);
}

/// The records of the SQLite wallet.dat vectors, each with its wallet
/// passphrase (`testdata/compat/manifest.json`).
fn walletdats() -> Vec<Vec<u8>> {
    let manifest = json("compat/manifest.json");
    let mut out = Vec::new();
    for wallet in manifest["wallets"].as_array().expect("wallets") {
        let path = testdata(&format!("compat/{}", wallet["file"].as_str().unwrap()));
        let passphrase = wallet["wallet_passphrase"].as_str().unwrap_or("");
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM main").unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .unwrap();
        let mut seed = Vec::new();
        field(&mut seed, passphrase.as_bytes());
        for row in rows {
            let (key, value) = row.unwrap();
            field(&mut seed, &key);
            field(&mut seed, &value);
        }
        out.push(seed);
    }
    out
}

/// The Berkeley DB wallet.dat vectors, each with its wallet passphrase
/// (empty for a plain wallet).
fn walletdat_bdbs() -> Vec<Vec<u8>> {
    let dir = testdata("compat/walletdat");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("bdb_"))
        })
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let name = path.file_name().unwrap().to_str().unwrap();
        let passphrase: &[u8] = match name {
            "bdb_hd_encrypted.dat" => b"bdb-pass",
            "bdb_core24_hd_encrypted.dat" => b"core-wallet-pass",
            _ => b"",
        };
        let mut seed = Vec::new();
        field(&mut seed, passphrase);
        seed.extend_from_slice(&read(&path));
        out.push(seed);
    }
    out
}

/// An [`crate::avatar`] input: flags, crop rectangle, zoom text, image.
pub fn avatar_input(fingerprint: bool, rect: [u32; 4], zoom: &str, image: &[u8]) -> Vec<u8> {
    let mut out = vec![u8::from(fingerprint)];
    for v in rect {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(u8::try_from(zoom.len()).expect("zoom text under 256 bytes"));
    out.extend_from_slice(zoom.as_bytes());
    out.extend_from_slice(image);
    out
}

/// Small pictures in each accepted format (D8): PNG (8-bit RGBA, 16-bit
/// grey), JPEG and GIF, under each kind of crop.
fn avatars() -> Vec<Vec<u8>> {
    let rgba = DynamicImage::ImageRgba8(RgbaImage::from_fn(24, 16, |x, y| {
        Rgba([
            (x * 10) as u8,
            (y * 15) as u8,
            ((x + y) * 5) as u8,
            255 - x as u8,
        ])
    }));
    let grey16 = DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(9, 13, |x, y| {
        Luma([(x * 7000 + y * 300) as u16])
    }));
    let images = [
        encode(&rgba, ImageFormat::Png),
        encode(&grey16, ImageFormat::Png),
        encode(&DynamicImage::ImageRgb8(rgba.to_rgb8()), ImageFormat::Jpeg),
        encode(&rgba, ImageFormat::Gif),
    ];
    let mut out = Vec::new();
    for image in &images {
        out.push(avatar_input(false, [0, 0, 0, 0], "", image));
        out.push(avatar_input(true, [2, 3, 8, 8], "0.1,0.2,0.9,0.8", image));
    }
    out
}

/// The seeds of target `name`.
pub fn seeds(name: &str) -> Vec<Vec<u8>> {
    let mut seeds = match name {
        "link" => {
            let mut s = link_corpus();
            s.extend(texts(&[
                "uri_cases.json",
                "uri_ext_cases.json",
                "dashpay_user_links.json",
            ]));
            s
        }
        "send_input" => texts(&[
            "uri_cases.json",
            "uri_ext_cases.json",
            "address_cases.json",
            "amount_format.json",
            "core/key_io_valid.json",
            "core/key_io_invalid.json",
        ]),
        "qr_image" => qr_images(),
        "psbt" => psbts(),
        "dumpwallet" => dumps(),
        "listdescriptors" => listdescriptors(),
        "walletdat" => walletdats(),
        "walletdat_bdb" => walletdat_bdbs(),
        "avatar" => avatars(),
        "invitation" => link_corpus(),
        _ => panic!("no fuzz target {name:?}"),
    };
    seeds.dedup();
    seeds
}
