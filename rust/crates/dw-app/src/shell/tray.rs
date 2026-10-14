//! The tray icon (QT-028..030): Show or Hide, Pay, Receive, Activity, Lock
//! and Quit. A left click shows or hides the window on Windows and macOS;
//! Linux trays only open the menu. Closing the window still quits: hiding
//! to the tray on close or minimise is an Options choice (U-04, QT-030).

use tauri::menu::{Menu, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Runtime};

use super::menu::{self, Action, menu_item};

/// Adds the tray icon. On Linux it needs an AppIndicator library, which a
/// desktop without a tray may lack: the tray is then left out, as Tauri
/// would otherwise abort.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if !indicator_available() {
        eprintln!("dw-app: no AppIndicator library; the tray icon is off");
        return Ok(());
    }
    let it = |id| menu_item(app, id);
    let lock = it("tray.lock")?;
    lock.set_enabled(false)?;
    let menu = Menu::with_items(
        app,
        &[
            &it("tray.toggle")?,
            &PredefinedMenuItem::separator(app)?,
            &it("tray.pay")?,
            &it("tray.receive")?,
            &it("tray.activity")?,
            &PredefinedMenuItem::separator(app)?,
            &lock,
            &PredefinedMenuItem::separator(app)?,
            &it("tray.quit")?,
        ],
    )?;
    if let Some(shell) = super::shell(app) {
        shell.on_sync(Box::new(move |s| {
            let _ = lock.set_enabled(s.can_lock);
        }));
    }
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Dash Wallet")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                menu::run(tray.app_handle(), Action::ToggleWindow);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Whether libappindicator-sys will find a library: it panics when it
/// loads none of these (its `LIBRARY_NAMES`).
#[cfg(target_os = "linux")]
fn indicator_available() -> bool {
    [
        "libayatana-appindicator3.so.1",
        "libappindicator3.so.1",
        "libayatana-appindicator3.so",
        "libappindicator3.so",
    ]
    .iter()
    // SAFETY: loading a system library runs its initialisers; these are the
    // libraries the tray would load anyway.
    .any(|name| unsafe { libloading::Library::new(name) }.is_ok())
}

#[cfg(not(target_os = "linux"))]
fn indicator_available() -> bool {
    true
}
