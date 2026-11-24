//! The binding's own commands: launch facts, the start-up marks (G-02's U5),
//! the renderer's display settings (U-04 D2) and the refusal probe. The
//! self-test's commands are in `selftest`.

use crate::bridge::RecentNotice;
use crate::dto::LaunchInfo;
use crate::error::{AppCode, AppError, ErrorData};
use crate::ipc::commands;
use crate::settings::{self, UiSettings};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// The repository's GitHub releases page, built in Rust so the renderer has
/// no opener or shell grant (`open_releases_page`).
pub(crate) const RELEASES_URL: &str = "https://github.com/PastaPastaPasta/dashwallet-desktop/releases";

/// The project pages Settings ▸ About links to. The renderer names one; the
/// URL is fixed here, so it cannot open anything else (`open_project_link`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum ProjectLinkName {
    /// The source code.
    SourceCode,
    /// Where problems are reported (support).
    Issues,
    /// Dash's website.
    Website,
}

impl ProjectLinkName {
    pub(crate) fn url(self) -> &'static str {
        match self {
            Self::SourceCode => "https://github.com/PastaPastaPasta/dashwallet-desktop",
            Self::Issues => "https://github.com/PastaPastaPasta/dashwallet-desktop/issues",
            Self::Website => "https://www.dash.org",
        }
    }
}

/// Opens `url`, a page under [`RELEASES_URL`], in the system browser.
pub(crate) async fn open_official(url: String) -> Result<(), AppError> {
    if url != RELEASES_URL && !url.starts_with(&format!("{RELEASES_URL}/")) {
        return Err(AppError::app(AppCode::BrowserOpen, format!("not a release page: {url}")));
    }
    tauri::async_runtime::spawn_blocking(move || open::that_detached(url))
        .await
        .map_err(|e| AppError::app(AppCode::BrowserOpen, e.to_string()))?
        .map_err(|e| AppError::app(AppCode::BrowserOpen, e.to_string()))
}

commands! {
    /// How the app was launched: the self-test, the forced theme, and the
    /// network `openNetwork` opens by default: the window's, which
    /// `switchNetwork` changes.
    #[facade()]
    fn launch_info(state) -> LaunchInfo {
        Ok(LaunchInfo::new(
            &state.launch,
            &state.window_network(),
            state.updates.current_version(),
        ))
    }

    /// Of the last 64 engine notices, oldest first, those of the window's
    /// network and those of none, kept apart from the event queue: after
    /// `eventsOverflowed`, the notices it may have dropped. A network the
    /// window left (`switchNetwork`) keeps its notices out, as its events.
    #[facade()]
    fn recent_notices(state) -> Vec<RecentNotice> {
        let window = state.window_network().dir_name();
        let mut notices = state.memory.recent_notices();
        notices.retain(|n| n.network.as_deref().is_none_or(|network| network == window));
        Ok(notices)
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
    /// 1 s to 24 hours, a BCP 47 language tag; `invalid_argument`
    /// otherwise), and gives the main
    /// window the chosen appearance unless `--theme` forced one (U-04 D3).
    #[facade()]
    fn set_ui_settings(state, settings: UiSettings) -> UiSettings {
        settings::write(&state.data_root, &settings)?;
        if state.launch.theme.is_none() {
            (state.apply_theme)(settings.theme);
        }
        Ok(settings)
    }

    /// Opens this repository's GitHub releases page in the system browser
    /// (the protocol-compatibility advisory banner). Only [`RELEASES_URL`]
    /// may be opened.
    #[facade()]
    fn open_releases_page(state) -> () {
        let _ = state;
        open_official(RELEASES_URL.into()).await
    }

    /// Opens one of the project pages About links to, in the system browser.
    /// The renderer names the page; its URL is fixed in Rust.
    #[facade()]
    fn open_project_link(state, link: ProjectLinkName) -> () {
        let _ = state;
        let url = link.url();
        let failed = |e: String| AppError::app(AppCode::BrowserOpen, e).with(ErrorData::Link { url: url.into() });
        tauri::async_runtime::spawn_blocking(move || open::that_detached(url))
            .await
            .map_err(|e| failed(e.to_string()))?
            .map_err(|e| failed(e.to_string()))
    }

    /// Registered but granted by no capability: the U6 probe that the ACL
    /// refuses an unlisted command (`api::UNGRANTED`).
    #[facade()]
    fn probe_unlisted(state) -> String {
        Ok("reached".into())
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectLinkName;

    /// The renderer's fixture answers a failed open with these URLs; they must stay this table's.
    #[test]
    fn the_fixture_names_the_same_project_urls() {
        let fixture = include_str!("../../../../../apps/desktop/src/fixture/handlers.ts");
        for (name, link) in [
            ("sourceCode", ProjectLinkName::SourceCode),
            ("issues", ProjectLinkName::Issues),
            ("website", ProjectLinkName::Website),
        ] {
            let line = format!("{name}: \"{}\",", link.url());
            assert!(fixture.contains(&line), "fixture PROJECT_LINKS is missing `{line}`");
        }
    }
}
