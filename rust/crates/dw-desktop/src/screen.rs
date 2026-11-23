//! A region of the screen as an image, for scanning a QR code shown in
//! another window (IOS-043, DP2-08 D10). The OS's own picker selects the
//! region, so the app asks no new permission:
//!
//! - Linux: the XDG desktop portal's `Screenshot` with `interactive`; the
//!   portal answers with the image's file, which is read and left where the
//!   portal put it. A session without the portal is `Unsupported`.
//! - Windows: the Snipping Tool's region capture (`ms-screenclip:`), then
//!   the image it puts on the clipboard (`CF_DIB`). UNVERIFIED: not run on
//!   Windows yet.
//! - macOS: `Unsupported`. Capturing from an app needs the Screen Recording
//!   permission; the host points to the system's own copy-a-region shortcut
//!   instead.

use std::time::{Duration, Instant};

use crate::DesktopError;

/// How a host offers a screen region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionCapture {
    /// [`capture_region`] lets the user pick a region.
    Picker,
    /// No capture here: the user copies a region with the OS's shortcut
    /// and pastes the image.
    CopyShortcut,
}

/// What this OS offers ([`capture_region`] may still find no portal).
pub fn region_capture() -> RegionCapture {
    if cfg!(any(target_os = "linux", windows)) {
        RegionCapture::Picker
    } else {
        RegionCapture::CopyShortcut
    }
}

/// Largest image read back from a capture, in bytes (as [`crate::qr`]).
const MAX_CAPTURE_BYTES: u64 = crate::qr::MAX_IMAGE_BYTES as u64;

/// How often a wait looks at `cancelled` and the deadline.
const POLL: Duration = Duration::from_millis(200);

/// Lets the user pick a region of the screen and returns it as an encoded
/// image (PNG or BMP). `Ok(None)` when the user cancelled, `cancelled`
/// turned true, or `deadline` passed first. Blocks: call it off the async
/// runtime.
pub fn capture_region(
    deadline: Instant,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, DesktopError> {
    #[cfg(target_os = "linux")]
    {
        let conn = zbus::blocking::Connection::session().map_err(|e| {
            DesktopError::Unsupported(format!("screen capture without a D-Bus session ({e})"))
        })?;
        portal::capture(&conn, deadline, cancelled)
    }
    #[cfg(windows)]
    {
        snip::capture(deadline, cancelled)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (deadline, cancelled);
        Err(DesktopError::Unsupported(
            "screen-region capture on this OS".into(),
        ))
    }
}

/// The file a `file://` URI names (RFC 8089: an empty or `localhost`
/// authority, the path percent-decoded). Any other URI is `None`.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn file_uri_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(std::ffi::OsString::from_vec(out).into())
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(out).ok().map(Into::into)
    }
}

/// The regular file at `path`, at most [`MAX_CAPTURE_BYTES`]. The path is
/// checked before it is opened (Windows refuses to open a directory as a
/// file, so every OS says `ImageUnreadable` for one, BL-164), and the open
/// file again, so no bytes are read from anything else put there in between.
/// That second check validates what was opened; it cannot make the open
/// itself race-free (a FIFO swapped in after the first check blocks it).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn read_image(path: &std::path::Path) -> Result<Vec<u8>, DesktopError> {
    use std::io::Read as _;
    let os = |e: std::io::Error| DesktopError::OsError(format!("reading the capture: {e}"));
    let regular = |meta: &std::fs::Metadata| {
        if meta.is_file() && meta.len() <= MAX_CAPTURE_BYTES {
            return Ok(());
        }
        Err(DesktopError::ImageUnreadable(
            "the capture is not a regular file of at most 32 MiB".into(),
        ))
    };
    regular(&std::fs::metadata(path).map_err(os)?)?;
    let file = std::fs::File::open(path).map_err(os)?;
    let meta = file.metadata().map_err(os)?;
    regular(&meta)?;
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_CAPTURE_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(os)?;
    if data.len() as u64 > MAX_CAPTURE_BYTES {
        return Err(DesktopError::ImageUnreadable(
            "the capture grew past 32 MiB".into(),
        ));
    }
    Ok(data)
}

/// Waits on `rx` until it answers, `cancelled` turns true or `deadline`
/// passes (both `None`).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn wait<T>(
    rx: &std::sync::mpsc::Receiver<T>,
    deadline: Instant,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<T> {
    loop {
        if cancelled() {
            return None;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match rx.recv_timeout(left.min(POLL)) {
            Ok(v) => return Some(v),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

#[cfg(target_os = "linux")]
mod portal {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::Instant;

    use zbus::blocking::{Connection, MessageIterator};
    use zbus::message::Type;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    use super::{file_uri_path, read_image, wait};
    use crate::DesktopError;

    const DEST: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const SCREENSHOT: &str = "org.freedesktop.portal.Screenshot";
    const REQUEST: &str = "org.freedesktop.portal.Request";

    /// Portal workers still running (the tests check a cancel ends its own).
    static WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// What the portal's `Response` said.
    enum Answer {
        Uri(String),
        Cancelled,
    }

    /// One interactive screenshot through the portal on `conn`.
    pub(super) fn capture(
        conn: &Connection,
        deadline: Instant,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Vec<u8>>, DesktopError> {
        let (tx, rx) = mpsc::channel();
        let (handle_tx, handle_rx) = mpsc::channel();
        let worker = conn.clone();
        // The portal waits for the user; the worker waits for the portal,
        // so a cancel or the deadline never waits for either.
        WORKERS.fetch_add(1, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("dw-desktop-portal".into())
            .spawn(move || {
                let _ = tx.send(screenshot(&worker, &handle_tx));
                WORKERS.fetch_sub(1, Ordering::SeqCst);
            })
            .map_err(|e| {
                WORKERS.fetch_sub(1, Ordering::SeqCst);
                DesktopError::OsError(e.to_string())
            })?;
        let Some(answer) = wait(&rx, deadline, cancelled) else {
            // Dismiss the portal's picker; it may already be gone.
            if let Ok(handle) = handle_rx.try_recv() {
                close(conn, &handle);
            }
            // The portal sends no `Response` after `Close`: closing the
            // connection ends the worker (and its bus socket) instead.
            let _ = conn.clone().close();
            return Ok(None);
        };
        match answer? {
            Answer::Cancelled => Ok(None),
            Answer::Uri(uri) => {
                let path = file_uri_path(&uri).ok_or_else(|| {
                    DesktopError::OsError("the portal answered with no local file".into())
                })?;
                read_image(&path).map(Some)
            }
        }
    }

    /// Asks the portal to close the request `handle`, without waiting for its
    /// reply: a portal that never answers must not hold the cancel.
    fn close(conn: &Connection, handle: &OwnedObjectPath) {
        let msg = zbus::Message::method_call(handle.as_str(), "Close")
            .and_then(|b| b.destination(DEST))
            .and_then(|b| b.interface(REQUEST))
            .and_then(|b| b.with_flags(zbus::message::Flags::NoReplyExpected))
            .and_then(|b| b.build(&()));
        if let Ok(msg) = msg {
            let _ = conn.send(&msg);
        }
    }

    /// Calls `Screenshot` and waits for its request's `Response`, sending the
    /// request's handle on `handle_tx` once known.
    fn screenshot(
        conn: &Connection,
        handle_tx: &mpsc::Sender<OwnedObjectPath>,
    ) -> Result<Answer, DesktopError> {
        let dbus = |e: zbus::Error| DesktopError::OsError(format!("screen capture: {e}"));
        // Listen before calling: the response may come before the reply.
        let rule = zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(REQUEST)
            .and_then(|b| b.member("Response"))
            .map_err(dbus)?
            .build();
        let signals = MessageIterator::for_match_rule(rule, conn, Some(16)).map_err(dbus)?;
        let token = format!("dw{}", std::process::id());
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(token.as_str()));
        options.insert("interactive", Value::from(true));
        options.insert("modal", Value::from(true));
        let reply = conn
            .call_method(
                Some(DEST),
                PATH,
                Some(SCREENSHOT),
                "Screenshot",
                &("", options),
            )
            .map_err(|e| match &e {
                // No portal, or one without a screenshot backend.
                zbus::Error::MethodError(name, _, _)
                    if [
                        "org.freedesktop.DBus.Error.ServiceUnknown",
                        "org.freedesktop.DBus.Error.UnknownMethod",
                        "org.freedesktop.DBus.Error.UnknownInterface",
                        "org.freedesktop.DBus.Error.UnknownObject",
                        "org.freedesktop.DBus.Error.NameHasNoOwner",
                    ]
                    .contains(&name.as_str()) =>
                {
                    DesktopError::Unsupported(format!("screen capture: {e}"))
                }
                _ => dbus(e),
            })?;
        let handle: OwnedObjectPath = reply.body().deserialize().map_err(dbus)?;
        let _ = handle_tx.send(handle.clone());
        // On a bus any client may signal on the handle's path: only the
        // portal that answered counts (a peer connection has no senders).
        let portal = reply.header().sender().map(|s| s.as_str().to_owned());
        for msg in signals {
            let msg = msg.map_err(dbus)?;
            let header = msg.header();
            if header.path().map(|p| p.as_str()) != Some(handle.as_str())
                || header.sender().map(|s| s.as_str()) != portal.as_deref()
            {
                continue;
            }
            let (response, results): (u32, HashMap<String, OwnedValue>) =
                msg.body().deserialize().map_err(dbus)?;
            return match response {
                0 => {
                    let uri = results
                        .get("uri")
                        .and_then(|v| <&str>::try_from(v).ok())
                        .ok_or_else(|| DesktopError::OsError("the portal sent no uri".into()))?;
                    Ok(Answer::Uri(uri.to_owned()))
                }
                1 => Ok(Answer::Cancelled),
                n => Err(DesktopError::OsError(format!("the portal ended with {n}"))),
            };
        }
        Err(DesktopError::OsError("the portal connection closed".into()))
    }

    #[cfg(test)]
    mod tests {
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        use zbus::blocking::connection::Builder;

        use super::*;

        /// A portal on the other end of a socket pair: it answers the first
        /// `Screenshot` with `response` and `uri`, sending the `Response`
        /// signal before the method's reply (the order the client must
        /// survive), or never answers when `response` is `None`.
        fn fake_portal(response: Option<u32>, uri: Option<String>) -> Connection {
            let (server, client) = UnixStream::pair().unwrap();
            let guid = zbus::Guid::generate();
            std::thread::spawn(move || {
                let conn = Builder::async_io_unix_stream(server)
                    .server(guid)
                    .unwrap()
                    .p2p()
                    .build()
                    .unwrap();
                for msg in MessageIterator::from(&conn) {
                    let Ok(msg) = msg else { return };
                    let header = msg.header();
                    if header.member().map(|m| m.as_str()) != Some("Screenshot") {
                        continue;
                    }
                    let (parent, options): (String, HashMap<String, OwnedValue>) =
                        msg.body().deserialize().unwrap();
                    assert_eq!(parent, "");
                    assert!(bool::try_from(&options["interactive"]).unwrap());
                    let token = <&str>::try_from(&options["handle_token"])
                        .unwrap()
                        .to_owned();
                    let handle = format!("/org/freedesktop/portal/desktop/request/1_1/{token}");
                    let Some(response) = response else {
                        conn.reply(&header, &OwnedObjectPath::try_from(handle).unwrap())
                            .unwrap();
                        continue;
                    };
                    let mut results: HashMap<&str, Value<'_>> = HashMap::new();
                    if let Some(uri) = &uri {
                        results.insert("uri", Value::from(uri.as_str()));
                    }
                    // Another request's response first: it must be ignored.
                    conn.emit_signal(
                        None::<()>,
                        "/org/freedesktop/portal/desktop/request/1_1/other",
                        REQUEST,
                        "Response",
                        &(1u32, HashMap::<&str, Value<'_>>::new()),
                    )
                    .unwrap();
                    conn.emit_signal(
                        None::<()>,
                        handle.as_str(),
                        REQUEST,
                        "Response",
                        &(response, results),
                    )
                    .unwrap();
                    conn.reply(&header, &OwnedObjectPath::try_from(handle).unwrap())
                        .unwrap();
                }
            });
            Builder::async_io_unix_stream(client).p2p().build().unwrap()
        }

        fn soon() -> Instant {
            Instant::now() + Duration::from_secs(30)
        }

        #[test]
        fn test_IOS_043_the_portal_s_file_is_read() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("Screenshot from now.png");
            std::fs::write(&path, b"\x89PNG fake").unwrap();
            let uri = format!("file://{}", path.to_str().unwrap().replace(' ', "%20"));
            let conn = fake_portal(Some(0), Some(uri));
            let got = capture(&conn, soon(), &|| false).unwrap();
            assert_eq!(got.as_deref(), Some(&b"\x89PNG fake"[..]));
            // The portal's file stays where it put it.
            assert!(path.exists());
        }

        #[test]
        fn test_IOS_043_a_cancelled_pick_is_none() {
            let conn = fake_portal(Some(1), None);
            assert_eq!(capture(&conn, soon(), &|| false).unwrap(), None);
        }

        #[test]
        fn test_IOS_043_a_failed_or_odd_answer_is_an_os_error() {
            let conn = fake_portal(Some(2), None);
            assert!(matches!(
                capture(&conn, soon(), &|| false),
                Err(DesktopError::OsError(_))
            ));
            let conn = fake_portal(Some(0), Some("https://example.com/a.png".into()));
            assert!(matches!(
                capture(&conn, soon(), &|| false),
                Err(DesktopError::OsError(_))
            ));
        }

        #[test]
        fn test_IOS_043_cancel_and_deadline_stop_waiting_for_the_user() {
            let conn = fake_portal(None, None);
            let started = Instant::now();
            assert_eq!(
                capture(&conn, soon(), &|| started.elapsed()
                    > Duration::from_millis(300))
                .unwrap(),
                None
            );
            let conn = fake_portal(None, None);
            let deadline = Instant::now() + Duration::from_millis(300);
            assert_eq!(capture(&conn, deadline, &|| false).unwrap(), None);
            // Neither left its worker waiting for a `Response` (the other
            // tests' workers end at once).
            let until = Instant::now() + Duration::from_secs(20);
            while WORKERS.load(Ordering::SeqCst) > 0 {
                assert!(
                    Instant::now() < until,
                    "a portal worker outlived its cancel"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[cfg(windows)]
mod snip {
    use std::time::Instant;

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
        OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_DIB;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::{MAX_CAPTURE_BYTES, POLL, dib_to_bmp};
    use crate::DesktopError;

    /// Starts the Snipping Tool's region capture and waits for the clipboard
    /// to change; a new image there is the region. UNVERIFIED: not run on
    /// Windows yet.
    pub(super) fn capture(
        deadline: Instant,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Vec<u8>>, DesktopError> {
        // SAFETY: no arguments.
        let before = unsafe { GetClipboardSequenceNumber() };
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (verb, target) = (wide("open"), wide("ms-screenclip:"));
        // ShellExecuteW may hand the protocol to a COM handler: the thread
        // (a blocking-pool one) joins an apartment for the call, as its
        // documentation asks, unless it is in one already.
        // SAFETY: the reserved pointer is null; a successful init (S_OK or
        // S_FALSE) is balanced below, a failed one (another mode) is not.
        let com = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        } >= 0;
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let started = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if com {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        // Values above 32 mean success (ShellExecuteW's contract).
        if started as usize <= 32 {
            return Err(DesktopError::Unsupported(
                "the Snipping Tool (ms-screenclip:)".into(),
            ));
        }
        loop {
            if cancelled() || Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(POLL);
            // SAFETY: no arguments.
            if unsafe { GetClipboardSequenceNumber() } == before {
                continue;
            }
            if let Some(image) = clipboard_dib()? {
                return Ok(Some(image));
            }
        }
    }

    /// The clipboard's `CF_DIB` image as a BMP file, `None` when it holds none.
    fn clipboard_dib() -> Result<Option<Vec<u8>>, DesktopError> {
        // SAFETY: plain Win32 calls; the clipboard is closed on every path
        // and the global memory unlocked before it.
        unsafe {
            if IsClipboardFormatAvailable(u32::from(CF_DIB)) == 0 {
                return Ok(None);
            }
            if OpenClipboard(0 as HWND) == 0 {
                return Ok(None);
            }
            let result = (|| {
                let handle = GetClipboardData(u32::from(CF_DIB));
                if handle.is_null() {
                    return Ok(None);
                }
                let size = GlobalSize(handle as _);
                if size as u64 > MAX_CAPTURE_BYTES {
                    return Err(DesktopError::ImageUnreadable(
                        "the capture is over 32 MiB".into(),
                    ));
                }
                let ptr = GlobalLock(handle as _) as *const u8;
                if ptr.is_null() {
                    return Ok(None);
                }
                let dib = std::slice::from_raw_parts(ptr, size).to_vec();
                GlobalUnlock(handle as _);
                Ok(dib_to_bmp(&dib))
            })();
            CloseClipboard();
            result
        }
    }
}

/// A packed device-independent bitmap (the clipboard's `CF_DIB`: a
/// `BITMAPINFOHEADER` or later header, its colour masks and table, then the
/// pixels) as a BMP file, by putting the 14-byte file header in front.
/// `None` for a header too short to be one.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |i: usize| {
        dib.get(i..i + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let header = u32_at(0)? as usize;
    if header < 40 || dib.len() < header {
        return None;
    }
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let used = u32_at(32)? as usize;
    // BI_BITFIELDS (3) with a 40-byte header: three masks follow it.
    let masks = if compression == 3 && header == 40 {
        12
    } else {
        0
    };
    let colours = match (used, bits) {
        (0, 1..=8) => 1usize << bits,
        (n, _) => n,
    };
    let offset = 14 + header + masks + colours * 4;
    let size = u32::try_from(14 + dib.len()).ok()?;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_IOS_043_file_uris_name_local_files_only() {
        assert_eq!(
            file_uri_path("file:///home/a/Screenshot%20from%202026.png").unwrap(),
            std::path::Path::new("/home/a/Screenshot from 2026.png")
        );
        assert_eq!(
            file_uri_path("file://localhost/tmp/x.png").unwrap(),
            std::path::Path::new("/tmp/x.png")
        );
        assert_eq!(file_uri_path("file://host/tmp/x.png"), None);
        assert_eq!(file_uri_path("https://example.com/x.png"), None);
        assert_eq!(file_uri_path("file:///tmp/%zz"), None);
        assert_eq!(file_uri_path("file:///tmp/a%00b"), None);
    }

    #[test]
    fn test_IOS_043_captures_are_regular_files_of_at_most_32_mib() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is refused as unreadable on every OS (Windows cannot open
        // one as a file, BL-164).
        assert!(matches!(
            read_image(dir.path()),
            Err(DesktopError::ImageUnreadable(_))
        ));
        assert!(matches!(
            read_image(&dir.path().join("missing")),
            Err(DesktopError::OsError(_))
        ));
        let big = dir.path().join("big.png");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_CAPTURE_BYTES + 1).unwrap();
        assert!(matches!(
            read_image(&big),
            Err(DesktopError::ImageUnreadable(_))
        ));
    }

    #[test]
    fn test_IOS_043_a_clipboard_dib_becomes_a_bmp_the_decoder_reads() {
        // A 2×1 24-bit DIB, then a QR code's grey image through the same path.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[0, 0, 0, 255, 255, 255, 0, 0]);
        let bmp = dib_to_bmp(&dib).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).unwrap();
        assert_eq!((img.width(), img.height()), (2, 1));
        // An 8-bit DIB carries a 256-colour table before its pixels.
        let mut dib8 = dib.clone();
        dib8[14..16].copy_from_slice(&8u16.to_le_bytes());
        let bmp8 = dib_to_bmp(&dib8).unwrap();
        assert_eq!(
            u32::from_le_bytes(bmp8[10..14].try_into().unwrap()),
            54 + 1024
        );
        assert_eq!(dib_to_bmp(&dib[..20]), None);
    }

    #[test]
    fn test_IOS_043_waits_end_on_cancel_or_deadline() {
        let (_tx, rx) = std::sync::mpsc::channel::<()>();
        assert_eq!(
            wait(&rx, Instant::now() + Duration::from_secs(30), &|| true),
            None
        );
        assert_eq!(wait(&rx, Instant::now(), &|| false), None);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(7).unwrap();
        assert_eq!(
            wait(&rx, Instant::now() + Duration::from_secs(30), &|| false),
            Some(7)
        );
    }
}
