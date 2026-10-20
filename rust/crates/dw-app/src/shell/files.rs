//! The native file chooser (U-05 D2): the OS's open and save dialogs,
//! shown through `tauri-plugin-dialog`'s Rust API. No capability grants the
//! plugin's own commands, so the renderer cannot open a dialog or name a
//! path by itself: a path comes only from the user's choice in a dialog
//! (`api::files`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use ts_rs::TS;

/// What a file is for: the dialog's title and filter, and the largest file
/// `open_file` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FilePurpose {
    /// A partially signed transaction (Tools ▸ PSBT, QT-077, QT-078).
    Psbt,
    /// The address book's export (QT-095).
    Csv,
    /// A `.dwbackup` the engine writes (U-04).
    WalletBackup,
    /// A wallet file to import: `.dwbackup`, `dumpwallet`, `wallet.dat` (U-04).
    WalletFile,
    /// A Dash Core export: `dumpwallet` text or `importdescriptors` JSON (U-04).
    CoreExport,
    /// An image holding a QR code (DP2-08 D9): what dw-desktop's decoder reads.
    QrImage,
    /// A profile picture (DP4-03): the formats the avatar pipeline decodes (DP4-02 D8).
    AvatarImage,
}

/// A dialog's words and filter.
pub struct PurposeSpec {
    pub open_title: &'static str,
    pub save_title: &'static str,
    pub filter: &'static str,
    pub extensions: &'static [&'static str],
    /// `open_file` refuses a file of this many bytes or more.
    pub max_bytes: u64,
}

/// Files other than PSBTs that `open_file` reads: far above any real one.
const MAX_OTHER_BYTES: u64 = 16 * 1024 * 1024;

/// The largest picture `prepare_avatar` takes from a file (DP4-02: 5 MiB, `avatar.too_large`
/// above it), so a larger one is refused before it is read.
pub const MAX_AVATAR_FILE_BYTES: u64 = 5 << 20;

impl FilePurpose {
    pub fn spec(self) -> PurposeSpec {
        // dash-qt's titles and filters where it has the dialog.
        match self {
            FilePurpose::Psbt => PurposeSpec {
                open_title: "Load Transaction Data",
                save_title: "Save Transaction Data",
                filter: "Partially Signed Transaction (Binary)",
                extensions: &["psbt"],
                max_bytes: dw_psbt::MAX_PSBT_BYTES as u64,
            },
            FilePurpose::Csv => PurposeSpec {
                open_title: "Open Address List",
                save_title: "Export Address List",
                filter: "Comma separated file",
                extensions: &["csv"],
                max_bytes: MAX_OTHER_BYTES,
            },
            FilePurpose::WalletBackup => PurposeSpec {
                open_title: "Open Wallet Backup",
                save_title: "Back Up Wallet",
                filter: "Wallet backup",
                extensions: &["dwbackup"],
                max_bytes: MAX_OTHER_BYTES,
            },
            FilePurpose::WalletFile => PurposeSpec {
                open_title: "Import Wallet File",
                save_title: "Save Wallet File",
                filter: "Wallet files",
                extensions: &["dwbackup", "txt", "dat"],
                max_bytes: MAX_OTHER_BYTES,
            },
            FilePurpose::CoreExport => PurposeSpec {
                open_title: "Open Dash Core Export",
                save_title: "Export for Dash Core",
                filter: "Dash Core export",
                extensions: &["txt", "json"],
                max_bytes: MAX_OTHER_BYTES,
            },
            FilePurpose::QrImage => PurposeSpec {
                open_title: "Open QR Code Image",
                save_title: "Save Image",
                filter: "Images",
                extensions: &["png", "jpg", "jpeg", "bmp"],
                max_bytes: dw_desktop::qr::MAX_IMAGE_BYTES as u64 + 1,
            },
            FilePurpose::AvatarImage => PurposeSpec {
                open_title: "Choose a Profile Picture",
                save_title: "Save Image",
                filter: "Images",
                extensions: &["png", "jpg", "jpeg", "gif"],
                max_bytes: MAX_AVATAR_FILE_BYTES + 1,
            },
        }
    }
}

/// One dialog to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRequest {
    pub purpose: FilePurpose,
    /// A save dialog (with the OS's question before replacing a file), else an open one.
    pub save: bool,
    pub suggested_name: Option<String>,
}

/// Shows a dialog and answers with the chosen path, `None` when cancelled.
pub type ChooseFn =
    Box<dyn Fn(FileRequest) -> tokio::sync::oneshot::Receiver<Option<PathBuf>> + Send + Sync>;

/// Installs the dialogs `Shell::choose` shows, parented to the main window.
/// `run` adds the plugin; tests install a fake chooser instead.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_dialog::DialogExt as _;
    let Some(shell) = super::shell(app) else {
        return;
    };
    let app = app.clone();
    shell.on_choose(Box::new(move |request| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let spec = request.purpose.spec();
        let mut dialog = app
            .dialog()
            .file()
            .set_title(if request.save {
                spec.save_title
            } else {
                spec.open_title
            })
            .add_filter(spec.filter, spec.extensions);
        if let Some(name) = &request.suggested_name {
            dialog = dialog.set_file_name(name);
        }
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        // A path the platform cannot name as a file (a mobile content URL)
        // counts as no choice.
        let reply = move |path: Option<tauri_plugin_dialog::FilePath>| {
            let _ = tx.send(path.and_then(|p| p.into_path().ok()));
        };
        if request.save {
            dialog.save_file(reply);
        } else {
            dialog.pick_file(reply);
        }
        rx
    }));
}
