//! Quitting (issue 399). `RunEvent::Exit` waits for the engine's shutdown on
//! the event loop's thread, which can take a while (a Shielded pass being
//! drained). Nothing is drawn meanwhile, so a window still shown there stays
//! on screen, frozen, until the process ends. [`hide_for_exit`] takes every
//! window and the tray icon off the screen first.

use tauri::{AppHandle, Manager, Runtime};

/// Hides every window and the tray icon now, on the event loop's thread,
/// before the exit blocks it. On Linux the GTK window is hidden directly and
/// the display flushed: Tauri's own `hide` there is queued for an event loop
/// turn that never comes. The window-state plugin saved the geometry before.
pub fn hide_for_exit<R: Runtime>(app: &AppHandle<R>) {
    for window in app.webview_windows().values() {
        #[cfg(target_os = "linux")]
        if let Ok(gtk_window) = window.gtk_window() {
            use gtk::prelude::WidgetExt;
            gtk_window.hide();
            continue;
        }
        let _ = window.hide();
    }
    #[cfg(desktop)]
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_visible(false);
    }
    #[cfg(target_os = "linux")]
    if let Some(display) = gtk::gdk::Display::default() {
        display.flush();
    }
}
