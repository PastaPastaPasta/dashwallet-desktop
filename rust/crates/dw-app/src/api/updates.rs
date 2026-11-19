//! The update check (PK-03a): notify only, off by default. Inside Flatpak,
//! which updates the app itself, there is no check: `supported` is false
//! and no request is made whatever the setting says (D-N4).

use dw_engine::updates as u;
use serde::Serialize;
use ts_rs::TS;

use crate::error::{AppCode, AppError};
use crate::ipc::commands;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// A newer full release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    /// `X.Y.Z`.
    pub version: String,
    /// UNIX seconds, when the feed says.
    pub published_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", content = "release", rename_all = "camelCase")]
pub enum UpdateStateName {
    /// Off, or not supported here: no request is made.
    Off,
    NotChecked,
    Current,
    Available(ReleaseInfo),
    Failed,
}

/// The setting and the last check's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesView {
    /// False inside Flatpak: the setting is not offered.
    pub supported: bool,
    pub enabled: bool,
    pub state: UpdateStateName,
    /// When the last check finished (UNIX seconds).
    pub checked_at: Option<u64>,
}

impl UpdatesView {
    fn new(v: u::UpdatesView, supported: bool) -> Self {
        let u::UpdatesView {
            enabled,
            state,
            checked_at,
        } = v;
        let state = match state {
            _ if !supported => UpdateStateName::Off,
            u::UpdateState::Off => UpdateStateName::Off,
            u::UpdateState::NotChecked => UpdateStateName::NotChecked,
            u::UpdateState::Current => UpdateStateName::Current,
            u::UpdateState::Available(r) => UpdateStateName::Available(ReleaseInfo {
                version: r.version,
                published_at: r.published_at,
            }),
            u::UpdateState::Failed => UpdateStateName::Failed,
        };
        Self {
            supported,
            enabled,
            state,
            checked_at: checked_at.filter(|_| supported),
        }
    }
}

fn view(v: u::UpdatesView) -> UpdatesView {
    UpdatesView::new(v, !dw_desktop::in_flatpak())
}

commands! {
    /// The setting and the last check's result.
    #[facade("Engine::updates", "Updates::status")]
    fn update_status(state) -> UpdatesView {
        Ok(view(state.engine.updates().status()))
    }

    /// Saves the setting; off stops a check in flight and forgets the last
    /// result. Makes no request by itself.
    #[facade("Engine::updates", "Updates::set_enabled")]
    fn set_update_check(state, enabled: bool) -> UpdatesView {
        Ok(view(state.engine.updates().set_enabled(enabled)?))
    }

    /// One request for the latest full release: `updates.off` while the
    /// check is off or when it is turned off before the answer. Inside
    /// Flatpak, the status, with no request.
    #[facade("Engine::updates", "Updates::check")]
    fn check_for_update(state) -> UpdatesView {
        let updates = state.engine.updates();
        if dw_desktop::in_flatpak() {
            return Ok(view(updates.status()));
        }
        Ok(view(updates.check().await?))
    }

    /// Opens the available release's page in the system browser: a URL the
    /// engine builds from the version, never one the renderer or the feed
    /// gives. `app.release_notes_open` when no update is available or the
    /// browser could not be opened.
    #[facade()]
    fn open_release_notes(state) -> () {
        let u::UpdateState::Available(release) = state.engine.updates().status().state else {
            return Err(AppError::app(AppCode::ReleaseNotesOpen, "no update is available"));
        };
        let url = release.notes_url;
        tauri::async_runtime::spawn_blocking(move || open::that_detached(&url))
            .await
            .map_err(|e| AppError::app(AppCode::ReleaseNotesOpen, e.to_string()))?
            .map_err(|e| AppError::app(AppCode::ReleaseNotesOpen, e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_reads_as_off_whatever_the_engine_holds() {
        let release = u::Release {
            version: "0.3.0".into(),
            published_at: Some(5),
            notes_url: "https://example.invalid".into(),
        };
        let engine = u::UpdatesView {
            enabled: true,
            state: u::UpdateState::Available(release),
            checked_at: Some(9),
        };
        assert_eq!(
            UpdatesView::new(engine.clone(), true),
            UpdatesView {
                supported: true,
                enabled: true,
                state: UpdateStateName::Available(ReleaseInfo {
                    version: "0.3.0".into(),
                    published_at: Some(5),
                }),
                checked_at: Some(9),
            }
        );
        assert_eq!(
            UpdatesView::new(engine, false),
            UpdatesView {
                supported: false,
                enabled: true,
                state: UpdateStateName::Off,
                checked_at: None,
            }
        );
    }
}
