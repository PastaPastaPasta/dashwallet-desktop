//! Ends the signing window when the machine is about to sleep.
//!
//! Linux listens for logind's `PrepareForSleep` on the system bus, through
//! zbus, which this crate already uses. `tao`'s `Suspended` event, and
//! Tauri's wrapper of it, are built for Android and iOS only and are
//! documented as unsupported on macOS, Windows and Linux, so those desktops
//! have no power notification here without a new dependency. macOS and
//! Windows therefore end the window from the one-second backstop, which
//! also runs on Linux: a tick whose monotonic clock jumps, or whose wall
//! clock runs ahead of it, is treated as a sleep. The vault still applies
//! the wall-clock check at the next grant, so a missed tick does not leave
//! the window open.

use std::time::{Duration, Instant, SystemTime};

use tauri::{AppHandle, Manager, Runtime};

use crate::state::AppState;

const TICK: Duration = Duration::from_secs(1);
/// Larger than a normal scheduling delay, smaller than a nap.
const GAP: Duration = Duration::from_secs(2);

/// Starts the backstop, and on Linux the logind listener. A bus that is
/// not there does not stop the process.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("signing-window-sleep".into())
        .spawn(move || watch(app));
}

fn watch<R: Runtime>(app: AppHandle<R>) {
    #[cfg(target_os = "linux")]
    {
        let logind_app = app.clone();
        let _ = std::thread::Builder::new()
            .name("signing-window-logind".into())
            .spawn(move || listen_logind(logind_app));
    }
    loop {
        let wall = SystemTime::now();
        let mono = Instant::now();
        std::thread::sleep(TICK);
        let wall_dt = SystemTime::now().duration_since(wall).unwrap_or_default();
        let mono_dt = Instant::now().saturating_duration_since(mono);
        if gap_is_sleep(wall_dt, mono_dt, TICK) {
            end_window(&app);
        }
    }
}

/// A measured tick was a sleep: the wall clock ran ahead of the monotonic
/// clock, or the monotonic clock jumped further than the tick allows.
pub(crate) fn gap_is_sleep(wall_dt: Duration, mono_dt: Duration, tick: Duration) -> bool {
    wall_dt > mono_dt.saturating_add(GAP) || mono_dt > tick.saturating_add(GAP)
}

fn end_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Ok(session) = tauri::async_runtime::block_on(state.session()) else {
        return;
    };
    session.vault().end_signing_window();
}

/// logind's `PrepareForSleep`: `start` is true as the machine goes to sleep.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn apply_prepare_for_sleep<R: Runtime>(start: bool, app: &AppHandle<R>) {
    if start {
        end_window(app);
    }
}

#[cfg(target_os = "linux")]
fn listen_logind<R: Runtime>(app: AppHandle<R>) {
    let Ok(conn) = zbus::blocking::Connection::system() else {
        return;
    };
    let Ok(proxy) = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    ) else {
        return;
    };
    let Ok(signals) = proxy.receive_signal("PrepareForSleep") else {
        return;
    };
    for message in signals {
        let Ok(start) = message.body().deserialize::<bool>() else {
            continue;
        };
        apply_prepare_for_sleep(start, &app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stall_or_a_wall_jump_is_a_sleep_and_a_normal_tick_is_not() {
        let tick = Duration::from_secs(1);
        assert!(!gap_is_sleep(tick, tick, tick));
        assert!(gap_is_sleep(
            Duration::from_secs(10),
            Duration::from_secs(1),
            tick
        ));
        assert!(gap_is_sleep(
            Duration::from_secs(8),
            Duration::from_secs(8),
            tick
        ));
        assert!(!gap_is_sleep(
            Duration::from_millis(1500),
            Duration::from_millis(1400),
            tick
        ));
    }
}
