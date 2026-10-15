//! QR codes the user shows the app (IOS-043, DP2-08 D9, D10): an image file
//! chosen in the OS's dialog, a dropped or pasted image, or a region of the
//! screen. Each is decoded here, by dw-desktop's `rqrr` decoder, and the
//! codes' texts go back in reading order; the renderer decides what they
//! are (a contact code goes to `dashpay_verify_scanned`). A text may carry a
//! bearer key (`dapk`), so none is logged.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppCode, AppError};
use crate::ipc::commands;
use crate::shell::files::FilePurpose;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// How long `scan_qr_screen` waits for the user's region.
const SCREEN_WAIT: Duration = Duration::from_secs(120);

/// Bumped by `cancel_screen_scan`: a capture begun under an older value
/// stops waiting.
static SCREEN_SCAN: AtomicU64 = AtomicU64::new(0);

/// How this OS lets the user scan a region of the screen (DP2-08 D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ScreenScanMode {
    /// `scan_qr_screen` shows the OS's region picker (Linux's desktop
    /// portal, Windows' Snipping Tool).
    Picker,
    /// No capture from the app (macOS: it would need the Screen Recording
    /// permission): the user copies a region with the system's shortcut and
    /// pastes the image.
    CopyShortcut,
}

commands! {
    /// How this OS lets the user scan a region of the screen.
    #[facade()]
    fn screen_scan_mode(state) -> ScreenScanMode {
        Ok(match dw_desktop::screen::region_capture() {
            dw_desktop::screen::RegionCapture::Picker => ScreenScanMode::Picker,
            dw_desktop::screen::RegionCapture::CopyShortcut => ScreenScanMode::CopyShortcut,
        })
    }

    /// Shows the open dialog for a PNG, JPEG or BMP image and returns the
    /// texts of its QR codes in reading order; `null` when the user
    /// cancelled. `app.no_qr_code` when none decodes,
    /// `app.image_unreadable` for another format or an image over 40 MP;
    /// the file codes as `open_file`'s.
    #[facade()]
    fn scan_qr_file(state) -> Option<Vec<String>> {
        let Some(file) = crate::api::files::open(&state, FilePurpose::QrImage).await? else {
            return Ok(None);
        };
        decode(file.data).await.map(Some)
    }

    /// The texts of the QR codes in `data` (a PNG, JPEG or BMP image,
    /// base64): a dropped or pasted image. Codes as `scan_qr_file`'s.
    #[facade()]
    fn scan_qr_image(state, data: String) -> Vec<String> {
        use ::base64::Engine as _;
        let data = ::base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| AppError::invalid_argument(format!("data: {e}")))?;
        decode(data).await
    }

    /// Lets the user pick a region of the screen (`screen_scan_mode`
    /// `picker`) and returns the texts of its QR codes; `null` when the user
    /// cancelled, `cancel_screen_scan` ran, or two minutes passed.
    /// `app.screen_capture_unavailable` where the OS or the session offers
    /// no picker (macOS; Linux without the desktop portal),
    /// `app.screen_capture` when the capture failed; then the image codes
    /// as `scan_qr_image`'s.
    #[facade()]
    fn scan_qr_screen(state) -> Option<Vec<String>> {
        let generation = SCREEN_SCAN.load(Ordering::SeqCst);
        let deadline = Instant::now() + SCREEN_WAIT;
        let image = tauri::async_runtime::spawn_blocking(move || {
            dw_desktop::screen::capture_region(deadline, &|| {
                SCREEN_SCAN.load(Ordering::SeqCst) != generation
            })
        })
        .await
        .map_err(|e| AppError::app(AppCode::ScreenCapture, e.to_string()))?
        .map_err(desktop_error)?;
        match image {
            Some(image) => decode(image).await.map(Some),
            None => Ok(None),
        }
    }

    /// Stops a running `scan_qr_screen` (it answers `null`).
    #[facade()]
    fn cancel_screen_scan(state) -> () {
        SCREEN_SCAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The QR texts in `image`, decoded off the async runtime.
async fn decode(image: Vec<u8>) -> Result<Vec<String>, AppError> {
    tauri::async_runtime::spawn_blocking(move || dw_desktop::qr::decode(&image))
        .await
        .map_err(|e| AppError::app(AppCode::ImageUnreadable, e.to_string()))?
        .map_err(desktop_error)
}

fn desktop_error(e: dw_desktop::DesktopError) -> AppError {
    use dw_desktop::DesktopError as D;
    let code = match &e {
        D::NoQrCode => AppCode::NoQrCode,
        D::ImageUnreadable(_) => AppCode::ImageUnreadable,
        D::Unsupported(_) => AppCode::ScreenCaptureUnavailable,
        D::OsError(_) | D::InvalidArgument(_) => AppCode::ScreenCapture,
    };
    AppError::app(code, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` as a QR code in a PNG (8 px modules, 4-module quiet zone).
    fn qr_png(text: &str) -> Vec<u8> {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let (n, scale, quiet) = (code.width() as u32, 8u32, 4u32);
        let side = (n + 2 * quiet) * scale;
        let colors = code.to_colors();
        let img = image::GrayImage::from_fn(side, side, |x, y| {
            let (mx, my) = (x / scale, y / scale);
            let dark = mx >= quiet
                && my >= quiet
                && mx < n + quiet
                && my < n + quiet
                && colors[((my - quiet) * n + (mx - quiet)) as usize] == qrcode::Color::Dark;
            image::Luma([if dark { 0 } else { 255 }])
        });
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn images_decode_to_their_codes() {
        let link = "dashpay://user?id=4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF&username=alice";
        let got = tauri::async_runtime::block_on(decode(qr_png(link))).unwrap();
        assert_eq!(got, [link]);
    }

    #[test]
    fn image_errors_keep_their_codes() {
        let blank = {
            let mut png = Vec::new();
            image::GrayImage::from_pixel(64, 64, image::Luma([255]))
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            png
        };
        let e = tauri::async_runtime::block_on(decode(blank)).unwrap_err();
        assert_eq!(e.code_str(), AppCode::NoQrCode.as_str());
        let e = tauri::async_runtime::block_on(decode(b"not an image".to_vec())).unwrap_err();
        assert_eq!(e.code_str(), AppCode::ImageUnreadable.as_str());
        let e = desktop_error(dw_desktop::DesktopError::Unsupported("x".into()));
        assert_eq!(e.code_str(), AppCode::ScreenCaptureUnavailable.as_str());
        let e = desktop_error(dw_desktop::DesktopError::OsError("x".into()));
        assert_eq!(e.code_str(), AppCode::ScreenCapture.as_str());
    }

    #[test]
    fn the_image_dialog_takes_what_the_decoder_reads() {
        let spec = FilePurpose::QrImage.spec();
        assert_eq!(spec.extensions, ["png", "jpg", "jpeg", "bmp"]);
        // `open_file` refuses at `max_bytes`; the decoder takes up to its own limit.
        assert_eq!(spec.max_bytes, dw_desktop::qr::MAX_IMAGE_BYTES as u64 + 1);
    }
}
