//! QR codes in image files and clipboard images (IOS-043). Pure Rust, used
//! on every OS: `image` decodes PNG, JPEG or BMP to greyscale and `rqrr`
//! finds and decodes every grid. The result is text; the host parses it
//! with `parse_payment_uri` like any typed URI.

use std::io::Cursor;

use image::{ImageFormat, ImageReader};

use crate::DesktopError;

/// Largest input accepted, in bytes.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Largest decoded image accepted, in pixels (about 40 MP).
pub const MAX_PIXELS: u64 = 40_000_000;

/// Every QR code in `image`, in reading order (top to bottom, then left to
/// right by each code's top-left corner). Codes that are found but do not
/// decode are skipped; `NoQrCode` when none decodes.
pub fn decode(image: &[u8]) -> Result<Vec<String>, DesktopError> {
    if image.len() > MAX_IMAGE_BYTES {
        return Err(DesktopError::ImageUnreadable(format!(
            "{} bytes; at most {MAX_IMAGE_BYTES}",
            image.len()
        )));
    }
    let reader = ImageReader::new(Cursor::new(image))
        .with_guessed_format()
        .map_err(|e| DesktopError::ImageUnreadable(e.to_string()))?;
    match reader.format() {
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Bmp) => {}
        Some(other) => {
            return Err(DesktopError::ImageUnreadable(format!(
                "{other:?} images are not supported; use PNG, JPEG or BMP"
            )));
        }
        None => return Err(DesktopError::ImageUnreadable("unknown image format".into())),
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| DesktopError::ImageUnreadable(e.to_string()))?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(DesktopError::ImageUnreadable(format!(
            "{width}×{height} pixels; at most {MAX_PIXELS}"
        )));
    }
    let luma = ImageReader::new(Cursor::new(image))
        .with_guessed_format()
        .map_err(|e| DesktopError::ImageUnreadable(e.to_string()))?
        .decode()
        .map_err(|e| DesktopError::ImageUnreadable(e.to_string()))?
        .into_luma8();

    let (w, h) = (luma.width() as usize, luma.height() as usize);
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        luma.get_pixel(x as u32, y as u32)[0]
    });
    let mut found: Vec<((i32, i32), String)> = prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| {
            let corner = grid.bounds[0];
            grid.decode()
                .ok()
                .map(|(_, text)| ((corner.y, corner.x), text))
        })
        .collect();
    if found.is_empty() {
        return Err(DesktopError::NoQrCode);
    }
    found.sort_by_key(|(corner, _)| *corner);
    Ok(found.into_iter().map(|(_, text)| text).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};
    use qrcode::QrCode;

    /// `texts` as QR codes side by side (8 px modules, 4-module quiet zone),
    /// encoded as `format`.
    fn render(texts: &[&str], format: ImageFormat) -> Vec<u8> {
        let codes: Vec<QrCode> = texts
            .iter()
            .map(|t| QrCode::new(t.as_bytes()).unwrap())
            .collect();
        let scale = 8u32;
        let quiet = 4u32;
        let sizes: Vec<u32> = codes.iter().map(|c| c.width() as u32).collect();
        let width: u32 = sizes.iter().map(|s| (s + 2 * quiet) * scale).sum();
        let height = sizes.iter().map(|s| (s + 2 * quiet) * scale).max().unwrap();
        let mut img = GrayImage::from_pixel(width, height, Luma([255]));
        let mut x0 = 0;
        for (code, size) in codes.iter().zip(&sizes) {
            let colors = code.to_colors();
            for (i, c) in colors.iter().enumerate() {
                if *c == qrcode::Color::Dark {
                    let (mx, my) = (i as u32 % size, i as u32 / size);
                    for dy in 0..scale {
                        for dx in 0..scale {
                            img.put_pixel(
                                x0 + (mx + quiet) * scale + dx,
                                (my + quiet) * scale + dy,
                                Luma([0]),
                            );
                        }
                    }
                }
            }
            x0 += (size + 2 * quiet) * scale;
        }
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn test_IOS_043_decodes_png_jpeg_and_bmp() {
        let uri = "dash:XpESxaUmonkq8RaLLp46Brx2K39ggQe226?amount=1.5&label=caf%C3%A9";
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Bmp] {
            assert_eq!(
                decode(&render(&[uri], format)).unwrap(),
                vec![uri],
                "{format:?}"
            );
        }
    }

    #[test]
    fn test_IOS_043_several_codes_come_in_reading_order() {
        let png = render(&["dash:first", "dash:second"], ImageFormat::Png);
        assert_eq!(decode(&png).unwrap(), vec!["dash:first", "dash:second"]);
    }

    #[test]
    fn test_IOS_043_no_code_and_bad_images_are_typed() {
        let blank = {
            let img = GrayImage::from_pixel(64, 64, Luma([255]));
            let mut out = Cursor::new(Vec::new());
            img.write_to(&mut out, ImageFormat::Png).unwrap();
            out.into_inner()
        };
        assert_eq!(decode(&blank), Err(DesktopError::NoQrCode));
        assert!(matches!(
            decode(b"not an image"),
            Err(DesktopError::ImageUnreadable(_))
        ));
        assert!(matches!(decode(&[]), Err(DesktopError::ImageUnreadable(_))));
        // A GIF header: recognised but not one of ours.
        assert!(matches!(
            decode(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;"),
            Err(DesktopError::ImageUnreadable(_))
        ));
        // A PNG header that claims 100 000 × 100 000 pixels.
        let mut huge = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        huge.extend_from_slice(&100_000u32.to_be_bytes());
        huge.extend_from_slice(&100_000u32.to_be_bytes());
        huge.extend_from_slice(&[8, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(matches!(
            decode(&huge),
            Err(DesktopError::ImageUnreadable(_))
        ));
        assert!(matches!(
            decode(&vec![0u8; MAX_IMAGE_BYTES + 1]),
            Err(DesktopError::ImageUnreadable(_))
        ));
    }
}
