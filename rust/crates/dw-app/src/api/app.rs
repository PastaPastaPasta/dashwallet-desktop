//! The binding's own commands: launch facts, the start-up marks (G-02's U5),
//! the renderer's display settings (U-04 D2) and the refusal probe. The
//! self-test's commands are in `selftest`.

use crate::bridge::RecentNotice;
use crate::dto::LaunchInfo;
use crate::ipc::commands;
use crate::settings::{self, UiSettings};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    /// How the app was launched: the self-test, the forced theme and the
    /// network `openNetwork` opens by default.
    #[facade()]
    fn launch_info(state) -> LaunchInfo {
        Ok(LaunchInfo::new(&state.launch))
    }

    /// The last 64 engine notices, oldest first, kept apart from the event
    /// queue: after `eventsOverflowed`, the notices it may have dropped.
    #[facade()]
    fn recent_notices(state) -> Vec<RecentNotice> {
        Ok(state.memory.recent_notices())
    }

    /// A start-up milestone of the renderer (`shell`: the window's first
    /// frame; `first-paint`: the first Overview frame with data). Returns
    /// milliseconds since the top of `main` and logs them (U5 cold start).
    #[facade()]
    fn startup_mark(state, mark: String) -> u64 {
        let ms = state.started.elapsed().as_millis() as u64;
        // Escaped: the harness reads these lines, so a mark cannot forge one.
        eprintln!("dw-app: startup {} {ms}", mark.escape_debug());
        Ok(ms)
    }

    /// The display settings in `<data root>/settings.json` (U-04 D2); the
    /// defaults when there is no readable file.
    #[facade()]
    fn ui_settings(state) -> UiSettings {
        Ok(settings::read(&state.data_root))
    }

    /// Checks and saves the display settings (decimal digits 2-8, auto lock
    /// 1 s to 24 hours; `invalid_argument` otherwise), and gives the main
    /// window the chosen appearance unless `--theme` forced one (U-04 D3).
    #[facade()]
    fn set_ui_settings(state, settings: UiSettings) -> UiSettings {
        settings::write(&state.data_root, &settings)?;
        if state.launch.theme.is_none() {
            (state.apply_theme)(settings.theme);
        }
        Ok(settings)
    }

    /// Registered but granted by no capability: the U6 probe that the ACL
    /// refuses an unlisted command (`api::UNGRANTED`).
    #[facade()]
    fn probe_unlisted(state) -> String {
        Ok("reached".into())
    }
}
