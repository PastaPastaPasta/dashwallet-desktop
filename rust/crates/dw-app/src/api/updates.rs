//! Updates (PK-03): whether this copy can update itself, the check, the
//! install and the restart, all on the user's request (`crate::updates`).

use crate::error::{AppCode, AppError};
use crate::ipc::commands;
use crate::updates::UpdateStatus;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    /// Whether this copy can update itself, its version, and where this
    /// run is in finding and installing an update. Fetches nothing.
    #[facade()]
    fn update_status(state) -> UpdateStatus {
        Ok(state.updates.status())
    }

    /// Asks the stable feed for a newer release: the user's "Check for
    /// updates", or the automatic check they turned on. The request carries
    /// no wallet data and no identifier. `app.updates_unavailable` when this
    /// copy cannot update itself, `app.update_check` when the feed cannot be
    /// read (the status stays as it was). While an install is under way it
    /// fetches nothing.
    #[facade()]
    fn check_for_update(state) -> UpdateStatus {
        state.updates.check().await
    }

    /// Downloads the offered `version`, checks its signature and puts it in
    /// place; the app runs it after `restart_to_update`. On Windows the
    /// installer replaces the running app: the engine is shut down as on
    /// quit, the app closes and the installer opens the new one.
    /// `app.update_state` when `version` is not the one on offer;
    /// `app.update_signature` when the signature does not verify (nothing is
    /// installed); `app.update_download` or `app.update_install` otherwise.
    #[facade()]
    fn install_update(state, version: String) -> UpdateStatus {
        state.updates.install(&version).await
    }

    /// Quits through the normal exit path (the engine flushes and closes)
    /// and starts the installed update. `app.update_state` before an update
    /// is installed.
    #[facade()]
    fn restart_to_update(state) -> () {
        state.updates.restart()
    }

    /// Opens the release notes of the update on offer in the system browser.
    /// Only that release's page on this repository may be opened.
    #[facade()]
    fn open_update_notes(state) -> () {
        let url = state.updates.notes_url()?;
        tauri::async_runtime::spawn_blocking(move || open::that_detached(url))
            .await
            .map_err(|e| AppError::app(AppCode::BrowserOpen, e.to_string()))?
            .map_err(|e| AppError::app(AppCode::BrowserOpen, e.to_string()))?;
        Ok(())
    }
}
