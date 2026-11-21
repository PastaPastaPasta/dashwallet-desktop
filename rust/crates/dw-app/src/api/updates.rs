//! Updates (PK-03): whether this copy can update itself, the check, the
//! install and the restart, all on the user's request (`crate::updates`).

use crate::error::{AppCode, AppError, ErrorData, UpdateBusyReason};
use crate::ipc::commands;
use crate::state::{AppState, EngineBuildView};
use crate::updates::{UpdatePhase, UpdateStatus};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// `app.update_busy` while the engine starts, a payment is being broadcast,
/// a wallet mixes or a Platform flow runs: installing on Windows and
/// restarting both close the engine, as Quit does.
pub(crate) async fn refuse_while_busy(state: &AppState) -> Result<(), AppError> {
    let reason = if matches!(state.engine.build_view(), EngineBuildView::Pending) {
        Some(UpdateBusyReason::Startup)
    } else {
        let session = state.session_slot().await.clone();
        match session {
            Some(session) => crate::api::network::running(&session, &state.broadcasts())?,
            None => None,
        }
    };
    match reason {
        Some(reason) => Err(AppError::app(AppCode::UpdateBusy, format!("{reason:?}"))
            .with(ErrorData::UpdateBusy { reason })),
        None => Ok(()),
    }
}

commands! {
    /// Whether this copy can update itself, and where this run is in
    /// finding and installing an update. Fetches nothing.
    #[facade()]
    fn update_status(state) -> UpdateStatus {
        Ok(state.updates.status())
    }

    /// Asks the stable feed for a newer release, for at most 30 s: the
    /// user's "Check for updates", or the automatic check they turned on.
    /// The request carries no wallet data and no identifier.
    /// `app.updates_unavailable` when this copy cannot update itself,
    /// `app.update_not_published` when the Latest release has no feed,
    /// `app.update_check` when the feed cannot be reached or read (the
    /// status stays as it was). While an install is under way it fetches
    /// nothing.
    #[facade()]
    fn check_for_update(state) -> UpdateStatus {
        state.updates.check().await
    }

    /// Downloads the offered `version`, checks its signature and puts it in
    /// place; the app runs it after `restart_to_update`. On Windows the
    /// installer replaces the running app: the engine is shut down as on
    /// quit, the app closes and the installer opens the new one; if the
    /// installer then fails, the window is shown again and the phase is
    /// `restartRequired`. Where installing closes the engine (Windows),
    /// `app.update_busy` while something runs that it would cut
    /// ([`refuse_while_busy`]), at the click and again once the download is
    /// done: then the download is kept and the phase is `downloaded`.
    /// `app.update_state` when `version` is not the one on offer;
    /// `app.update_signature` when the signature does not verify (nothing
    /// is installed); `app.update_download` or `app.update_install`
    /// otherwise.
    #[facade()]
    fn install_update(state, version: String) -> UpdateStatus {
        state.updates.feed()?;
        if state.updates.install_closes_engine() {
            refuse_while_busy(&state).await?;
        }
        state.updates.install(&version, || refuse_while_busy(&state)).await
    }

    /// Quits through the normal exit path (the engine flushes and closes)
    /// and starts the installed update, or restarts after a failed Windows
    /// install (`restartRequired`, when the engine is already closed).
    /// `app.update_busy` otherwise while something runs that closing the
    /// engine would cut; `app.update_state` before an update is installed.
    #[facade()]
    fn restart_to_update(state) -> () {
        state.updates.feed()?;
        if !matches!(state.updates.status().phase, UpdatePhase::RestartRequired { .. }) {
            refuse_while_busy(&state).await?;
        }
        state.updates.restart()
    }

    /// Opens the page of the release on offer (`…/releases/tag/vX.Y.Z` on
    /// this repository) in the system browser: its notes and downloads.
    /// `app.update_state` when no release is on offer.
    #[facade()]
    fn open_update_notes(state) -> () {
        state.updates.feed()?;
        crate::api::app::open_official(state.updates.notes_url()?).await
    }
}
