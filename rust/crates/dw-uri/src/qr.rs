//! QR module matrices for payment URIs and addresses (QT-084, IOS-053).
//!
//! dash-qt encodes with libqrencode as `QRcode_encodeString(uri, 0,
//! QR_ECLEVEL_L, QR_MODE_8, 1)`: error correction level L, the whole string as
//! one 8-bit (byte) segment, the smallest version that fits. This module
//! encodes the same way with the `qrcode` crate and picks the mask with
//! libqrencode's penalty function (`mask.c`), whose scoring differs from the
//! `qrcode` crate's. The result is module-for-module what dash-qt draws
//! (golden test: `testdata/qr_cases.json`). The host draws the modules
//! itself; no image is produced here.

use qrcode::bits::Bits;
use qrcode::canvas::{Canvas, MaskPattern};
use qrcode::{Color, EcLevel, Version};

/// dash-qt `MAX_URI_LENGTH`: longer text is refused before encoding
/// ("Resulting URI too long, try to reduce the text for label / message.").
pub const MAX_QR_TEXT_LENGTH: usize = 255;

/// Dark/light modules of a QR symbol, row-major, without a quiet zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    /// Modules per side.
    pub size: u32,
    /// `modules[y * size + x]`, `true` = dark.
    pub modules: Vec<bool>,
}

impl QrMatrix {
    pub fn is_dark(&self, x: u32, y: u32) -> bool {
        self.modules[(y * self.size + x) as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    /// More than [`MAX_QR_TEXT_LENGTH`] characters, counted in UTF-16 code
    /// units as Qt's `QString::length()` counts them.
    #[error("text is {len} characters, the QR limit is {MAX_QR_TEXT_LENGTH}")]
    TooLong { len: usize },
}

/// libqrencode's mask order (mask number = index).
const MASKS: [MaskPattern; 8] = [
    MaskPattern::Checkerboard,
    MaskPattern::HorizontalLines,
    MaskPattern::VerticalLines,
    MaskPattern::DiagonalLines,
    MaskPattern::LargeCheckerboard,
    MaskPattern::Fields,
    MaskPattern::Diamonds,
    MaskPattern::Meadow,
];

/// Encodes `text` as dash-qt does (byte mode, ECC L, smallest version,
/// libqrencode mask choice).
pub fn qr_matrix(text: &str) -> Result<QrMatrix, QrError> {
    let len = text.encode_utf16().count();
    if len > MAX_QR_TEXT_LENGTH {
        return Err(QrError::TooLong { len });
    }
    // 255 UTF-16 units are at most 765 UTF-8 bytes and version 40-L holds
    // 2953, so some version always fits.
    let (version, data) = (1..=40)
        .find_map(|v| byte_mode_codewords(text.as_bytes(), Version::Normal(v)))
        .expect("255 characters fit in a version-40 L symbol");
    let (data, ec) = qrcode::ec::construct_codewords(&data, version, EcLevel::L)
        .expect("codewords of a filled data stream");
    let mut canvas = Canvas::new(version, EcLevel::L);
    canvas.draw_all_functional_patterns();
    canvas.draw_data(&data, &ec);

    let size = version.width() as usize;
    let modules = MASKS
        .iter()
        .map(|&mask| {
            let mut masked = canvas.clone();
            masked.apply_mask(mask);
            masked
                .into_colors()
                .into_iter()
                .map(|c| c == Color::Dark)
                .collect::<Vec<bool>>()
        })
        // `min_by_key` keeps the first of equal scores: libqrencode only
        // replaces its best mask on a strictly lower demerit.
        .min_by_key(|m| libqrencode_demerit(m, size))
        .expect("eight masks");
    Ok(QrMatrix {
        size: size as u32,
        modules,
    })
}

/// The data codewords of `data` as one byte segment at `version`, or `None`
/// when it does not fit.
fn byte_mode_codewords(data: &[u8], version: Version) -> Option<(Version, Vec<u8>)> {
    let mut bits = Bits::new(version);
    bits.push_byte_data(data).ok()?;
    bits.push_terminator(EcLevel::L).ok()?;
    Some((version, bits.into_bytes()))
}

// libqrencode mask.c weights.
const N1: u32 = 3;
const N2: u32 = 3;
const N3: u32 = 40;
const N4: u32 = 10;

/// `Mask_mask`'s score of a masked symbol (format information included):
/// the dark-ratio term plus `Mask_evaluateSymbol`.
fn libqrencode_demerit(m: &[bool], width: usize) -> u32 {
    let w2 = (width * width) as u32;
    let blacks = m.iter().filter(|&&d| d).count() as u32;
    // (int)(100 * blacks / w2 + 0.5), written as libqrencode writes it.
    let bratio = (200 * blacks + w2) / w2 / 2;
    let mut demerit = (bratio.abs_diff(50) / 5) * N4;

    // N2: every 2x2 block of one colour.
    for y in 1..width {
        for x in 1..width {
            let p = [
                m[y * width + x],
                m[y * width + x - 1],
                m[(y - 1) * width + x],
                m[(y - 1) * width + x - 1],
            ];
            if p.iter().all(|&d| d) || p.iter().all(|&d| !d) {
                demerit += N2;
            }
        }
    }
    for y in 0..width {
        demerit += n1_n3(&run_lengths((0..width).map(|x| m[y * width + x])));
    }
    for x in 0..width {
        demerit += n1_n3(&run_lengths((0..width).map(|y| m[y * width + x])));
    }
    demerit
}

/// `Mask_calcRunLength{H,V}`: alternating light/dark run lengths starting
/// with a light run; a line that starts dark gets a `-1` placeholder first.
fn run_lengths(line: impl Iterator<Item = bool>) -> Vec<i32> {
    let mut runs: Vec<i32> = Vec::new();
    let mut prev = None;
    for dark in line {
        match prev {
            None => {
                if dark {
                    runs.push(-1);
                }
                runs.push(1);
            }
            Some(p) if p == dark => *runs.last_mut().expect("a run") += 1,
            Some(_) => runs.push(1),
        }
        prev = Some(dark);
    }
    runs
}

/// `Mask_calcN1N3`: long runs (N1) and 1:1:3:1:1 finder-like patterns (N3).
///
/// The light run after the pattern is read at `i + 3` (the pattern's last
/// dark run), not `i + 4`: libqrencode 4.1.1 does this, and the mask it then
/// picks is the one dash-qt shows.
fn n1_n3(runs: &[i32]) -> u32 {
    let len = runs.len();
    let mut demerit = 0;
    for i in 0..len {
        if runs[i] >= 5 {
            demerit += N1 + (runs[i] - 5) as u32;
        }
        if i & 1 == 1 && i >= 3 && i + 2 < len && runs[i] % 3 == 0 {
            let fact = runs[i] / 3;
            if runs[i - 2] == fact
                && runs[i - 1] == fact
                && runs[i + 1] == fact
                && runs[i + 2] == fact
                && (i == 3 || runs[i - 3] >= 4 * fact || i + 4 >= len || runs[i + 3] >= 4 * fact)
            {
                demerit += N3;
            }
        }
    }
    demerit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Module count per side for QR version `v`.
    fn width(v: u32) -> u32 {
        17 + 4 * v
    }

    #[test]
    fn picks_the_smallest_byte_mode_version() {
        // Byte-mode capacities at ECC L: v1 = 17, v2 = 32, v3 = 53 bytes.
        assert_eq!(qr_matrix(&"a".repeat(17)).unwrap().size, width(1));
        assert_eq!(qr_matrix(&"a".repeat(18)).unwrap().size, width(2));
        // Digits would fit v1 in numeric mode; dash-qt uses byte mode.
        assert_eq!(qr_matrix(&"1".repeat(32)).unwrap().size, width(2));
        assert_eq!(qr_matrix(&"1".repeat(33)).unwrap().size, width(3));
    }

    #[test]
    fn matrix_has_finder_patterns_and_no_quiet_zone() {
        let m = qr_matrix("dash:XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg").unwrap();
        assert_eq!(m.modules.len(), (m.size * m.size) as usize);
        // Top-left finder: dark 7x7 border, light ring, dark 3x3 core.
        for i in 0..7 {
            assert!(m.is_dark(i, 0) && m.is_dark(0, i) && m.is_dark(i, 6) && m.is_dark(6, i));
        }
        for i in 1..6 {
            assert!(!m.is_dark(i, 1) && !m.is_dark(1, i));
        }
        assert!(m.is_dark(3, 3));
        // Separator column next to the finder is light.
        assert!((0..8).all(|y| !m.is_dark(7, y)));
        // Top-right and bottom-left finders start at the edges.
        assert!(m.is_dark(m.size - 1, 0) && m.is_dark(0, m.size - 1));
    }

    #[test]
    fn length_limit_counts_utf16_units() {
        assert!(qr_matrix(&"a".repeat(255)).is_ok());
        assert_eq!(
            qr_matrix(&"a".repeat(256)),
            Err(QrError::TooLong { len: 256 })
        );
        // U+1F600 is two UTF-16 units (Qt length 2), four UTF-8 bytes.
        let emoji = "\u{1F600}".repeat(128);
        assert_eq!(qr_matrix(&emoji), Err(QrError::TooLong { len: 256 }));
        assert!(qr_matrix(&"\u{1F600}".repeat(127)).is_ok());
    }

    #[test]
    fn run_lengths_follow_libqrencode_layout() {
        assert_eq!(run_lengths([false, false, true].into_iter()), [2, 1]);
        assert_eq!(run_lengths([true, true, false].into_iter()), [-1, 2, 1]);
    }

    #[test]
    fn n3_scores_a_finder_like_run() {
        // light 4, dark 1, light 1, dark 3, light 1, dark 1, light 4
        let runs = [4, 1, 1, 3, 1, 1, 4];
        assert_eq!(n1_n3(&runs), N3);
        assert_eq!(n1_n3(&[1, 1, 1, 2, 1, 1, 1]), 0);
    }
}
