//! Ends the signing window when the machine is about to sleep.
//!
//! Linux listens for logind's `PrepareForSleep` on the system bus, through
//! zbus, which this crate already uses. `tao`'s `Suspended` event, and
//! Tauri's wrapper of it, are built for Android and iOS only and are
//! documented as unsupported on macOS, Windows and Linux, so those desktops
//! have no power notification here without a new dependency. macOS and
//! Windows therefore end the window from the one-second backstop, which
//! also runs on Linux: a tick whose monotonic clock jumps, or whose wall
//! clock moves away from it either way, is treated as a sleep. Every other
//! tick is a heartbeat to the vault, which refuses window grants once the
//! last one is more than three seconds old, so a grant asked for after a
//! wake and before the next tick is refused too. The vault also compares
//! its own clocks at the next grant.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use tauri::{AppHandle, Manager, Runtime};

use crate::state::AppState;

const TICK: Duration = Duration::from_secs(1);
/// Larger than a normal scheduling delay, smaller than a nap.
const GAP: Duration = Duration::from_secs(2);

/// The desktop app runs this watcher: every vault it opens then needs
/// the watcher's beats before its window admits a payment
/// ([`require_beats`]).
static WATCHED: AtomicBool = AtomicBool::new(false);

/// Starts the backstop, and on Linux the logind listener. A bus that is
/// not there does not stop the process. From here every vault the app
/// opens needs the watcher's beats, so if the watcher cannot start the
/// signing window admits nothing and every payment asks.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    WATCHED.store(true, Ordering::SeqCst);
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("signing-window-sleep".into())
        .spawn(move || watch(app))
    {
        eprintln!(
            "dw-app: the sleep watcher did not start ({e}); every payment asks for the passphrase"
        );
    }
}

/// Makes `session`'s vault need the watcher's beats when this process runs
/// the watcher. Called for every session the app opens, before commands
/// can reach it.
pub(crate) fn require_beats(session: &dw_engine::NetworkSession) {
    if WATCHED.load(Ordering::SeqCst) {
        session.vault().require_signing_window_heartbeat();
    }
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
        let mono_dt = Instant::now().saturating_duration_since(mono);
        if gap_is_sleep(wall, SystemTime::now(), mono_dt, TICK) {
            end_window(&app);
        } else {
            heartbeat(&app);
        }
    }
}

/// A measured tick was a sleep: the wall clock moved away from the
/// monotonic clock, forward or back, or the monotonic clock jumped further
/// than the tick allows.
pub(crate) fn gap_is_sleep(
    wall_from: SystemTime,
    wall_to: SystemTime,
    mono_dt: Duration,
    tick: Duration,
) -> bool {
    let apart = match wall_to.duration_since(wall_from) {
        Ok(wall_dt) => wall_dt.abs_diff(mono_dt),
        Err(back) => back.duration().saturating_add(mono_dt),
    };
    apart > GAP || mono_dt > tick.saturating_add(GAP)
}

/// Tells the active session's vault the watcher is running. Skipped while
/// a network opens or closes; the vault then ends the window if that takes
/// more than its heartbeat allowance.
fn heartbeat<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if let Some(session) = state.session_now() {
        session.vault().signing_window_heartbeat();
    }
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
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ms = Duration::from_millis;
        let forward = |wall: u64, mono: u64| gap_is_sleep(start, start + ms(wall), ms(mono), tick);
        let back = |wall: u64, mono: u64| gap_is_sleep(start, start - ms(wall), ms(mono), tick);
        assert!(!forward(1_000, 1_000));
        assert!(!forward(1_500, 1_400));
        assert!(forward(10_000, 1_000));
        assert!(forward(8_000, 8_000));
        assert!(!back(500, 1_000));
        assert!(back(3_000, 1_000));
        assert!(back(3_600_000, 1_000));
    }
}
