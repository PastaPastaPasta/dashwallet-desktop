//! Updates from the signed GitHub Releases feed (PK-03), off unless the
//! build, the install and the user all allow them.
//!
//! - The build: only a build made with the updater signing key has the feed.
//!   release.yml merges `tauri.updater.conf.json` (the feed URL, and
//!   `bundle.createUpdaterArtifacts`) only when the key secret exists, and
//!   the public key in `tauri.conf.json` is [`PUBKEY_PLACEHOLDER`] until the
//!   release key exists ([`build_has_updater`]). Without both, the plugin is
//!   not even registered.
//! - The install: a Flatpak updates through Flatpak, and a .deb or .rpm
//!   through the system's package manager ([`availability`]).
//! - The user: nothing is fetched until the renderer asks, either from
//!   "Check for updates" or from the automatic check the user turned on in
//!   Settings (`UiSettings::check_updates_automatically`, off by default).
//!
//! A found update is only shown. It is downloaded, its signature checked
//! and installed when the user clicks Install, and the app restarts when the
//! user clicks Restart, through the quit path (`RunEvent::Exit` flushes and
//! closes the engine). On Windows the installer has to replace the running
//! app, so Install closes it, after the same engine shutdown.
//!
//! Only stable releases are offered, and only newer ones ([`is_offered`]);
//! the signature must name the version the feed announced
//! (`requireSignedVersion`), so an old signed build cannot be relabelled.
//! The request carries no wallet data and no identifier: the static feed
//! URL, the plugin's user agent and an `Accept` header.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use semver::Version;
use serde::Serialize;
use serde_json::Value;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;
use ts_rs::TS;

use crate::api::app::RELEASES_URL;
use crate::error::{AppCode, AppError};

/// The stable channel's feed: the `latest.json` of the newest full release.
/// GitHub's "latest" is never a pre-release or a draft.
pub const FEED_URL: &str =
    "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/latest/download/latest.json";

/// `plugins.updater.pubkey` in `tauri.conf.json` until the release key
/// exists (docs/release/updater.md): a build with it has no updater.
pub const PUBKEY_PLACEHOLDER: &str =
    "UPDATER-DISABLED: replace with the release public key (docs/release/updater.md)";

/// Whether this copy of the app can update itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UpdateAvailability {
    Available,
    /// Built without the updater signing key: no feed, no public key.
    NotInThisBuild,
    /// Installed with Flatpak, which updates it.
    Flatpak,
    /// Installed from a .deb or .rpm: the system's package manager updates it.
    PackageManager,
    /// Not run from an installed bundle (a development build, an extracted
    /// AppImage).
    NotInstalled,
}

/// What [`availability`] needs to know about the running copy.
#[derive(Debug, Clone)]
pub struct Install {
    pub flatpak: bool,
    pub bundle: Option<BundleType>,
    /// Run as an AppImage (`$APPIMAGE` names the file the update replaces).
    pub appimage: bool,
}

impl Install {
    pub fn current<R: Runtime>(app: &AppHandle<R>) -> Self {
        #[cfg(target_os = "linux")]
        let appimage = app.env().appimage.is_some();
        #[cfg(not(target_os = "linux"))]
        let appimage = {
            let _ = app;
            false
        };
        Install {
            flatpak: is_flatpak(),
            bundle: tauri::utils::platform::bundle_type(),
            appimage,
        }
    }
}

/// Flatpak puts `/.flatpak-info` in every sandbox and sets `FLATPAK_ID`.
fn is_flatpak() -> bool {
    cfg!(target_os = "linux")
        && (std::path::Path::new("/.flatpak-info").exists()
            || std::env::var_os("FLATPAK_ID").is_some())
}

/// The build has the updater: a real public key, the official feed as its
/// only endpoint, and signatures that must name their version.
pub fn build_has_updater(config: &tauri::Config) -> bool {
    let Some(updater) = config.plugins.0.get("updater") else {
        return false;
    };
    let pubkey = updater
        .get("pubkey")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let endpoints = updater
        .get("endpoints")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    !pubkey.trim().is_empty()
        && pubkey != PUBKEY_PLACEHOLDER
        && !endpoints.is_empty()
        && endpoints.iter().all(|e| e.as_str() == Some(FEED_URL))
        && updater.get("requireSignedVersion") == Some(&Value::Bool(true))
}

pub fn availability(config: &tauri::Config, install: &Install) -> UpdateAvailability {
    use UpdateAvailability as A;
    if install.flatpak {
        return A::Flatpak;
    }
    if !build_has_updater(config) {
        return A::NotInThisBuild;
    }
    match install.bundle {
        Some(BundleType::Deb | BundleType::Rpm) => A::PackageManager,
        Some(BundleType::AppImage) if install.appimage => A::Available,
        Some(BundleType::Nsis | BundleType::App) => A::Available,
        _ => A::NotInstalled,
    }
}

/// A release is offered when it is stable and newer than this build.
pub fn is_offered(current: &Version, release: &Version) -> bool {
    release.pre.is_empty() && release.build.is_empty() && release > current
}

/// The plugin, for a build that [`build_has_updater`]. Its JavaScript
/// commands are granted to no window.
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new()
        .default_version_comparator(|current, release| is_offered(&current, &release.version))
        .build()
}

/// The page of a release's notes.
pub fn notes_url(version: &str) -> String {
    format!("{RELEASES_URL}/tag/v{version}")
}

/// Where this run is in finding and installing an update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdatePhase {
    /// Nothing checked yet in this run.
    Idle,
    Checking,
    /// The last check found nothing newer, at `checked_at` (UNIX seconds).
    UpToDate {
        checked_at: u64,
    },
    /// A newer stable release, not downloaded.
    Found {
        version: String,
    },
    /// `received` of `total` bytes (`null` while the size is unknown).
    Downloading {
        version: String,
        received: u64,
        total: Option<u64>,
    },
    /// Downloaded and verified, being put in place.
    Installing {
        version: String,
    },
    /// In place: it runs after a restart.
    Installed {
        version: String,
    },
}

/// `update_status`: what Settings shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub availability: UpdateAvailability,
    pub current_version: String,
    pub phase: UpdatePhase,
}

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Called with the bytes received so far and the total, when known.
pub(crate) type Progress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

/// Where updates come from: the plugin in the app, a fake in tests.
pub(crate) trait Feed: Send + Sync {
    /// The newest offered release, if any.
    fn check(&self) -> BoxFuture<'_, Result<Option<Arc<dyn Release>>, AppError>>;
    /// Ends the app through its quit path and starts the installed one.
    fn restart(&self);
}

/// An offered release.
pub(crate) trait Release: Send + Sync {
    fn version(&self) -> String;
    /// Downloads the update and checks its signature.
    fn download<'a>(&'a self, progress: Progress<'a>) -> BoxFuture<'a, Result<Vec<u8>, AppError>>;
    /// Checks the signature again and puts the update in place (blocking).
    /// On Windows it does not return: the installer replaces the app.
    fn install(&self, bytes: &[u8]) -> Result<(), AppError>;
}

/// The update of this run (one per `AppState`).
pub struct Updates {
    availability: UpdateAvailability,
    current_version: String,
    feed: Option<Box<dyn Feed>>,
    state: Mutex<Session>,
}

struct Session {
    phase: UpdatePhase,
    release: Option<Arc<dyn Release>>,
}

impl Updates {
    pub(crate) fn new(
        availability: UpdateAvailability,
        current_version: String,
        feed: Option<Box<dyn Feed>>,
    ) -> Self {
        let feed = feed.filter(|_| availability == UpdateAvailability::Available);
        Updates {
            availability,
            current_version,
            feed,
            state: Mutex::new(Session {
                phase: UpdatePhase::Idle,
                release: None,
            }),
        }
    }

    /// The app's: the plugin's feed when this copy can update itself.
    pub fn for_app<R: Runtime>(app: &AppHandle<R>) -> Self {
        let availability = availability(app.config(), &Install::current(app));
        let feed = (availability == UpdateAvailability::Available)
            .then(|| Box::new(PluginFeed { app: app.clone() }) as Box<dyn Feed>);
        Self::new(availability, app.package_info().version.to_string(), feed)
    }

    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> UpdateStatus {
        UpdateStatus {
            availability: self.availability,
            current_version: self.current_version.clone(),
            phase: self.session().phase.clone(),
        }
    }

    fn feed(&self) -> Result<&dyn Feed, AppError> {
        self.feed.as_deref().ok_or_else(|| {
            AppError::app(
                AppCode::UpdatesUnavailable,
                format!("{:?}", self.availability),
            )
        })
    }

    /// Asks the feed once. While a check, download or install is under way,
    /// or once an update is installed, it fetches nothing and returns the
    /// status. A failed check leaves the status as it was.
    pub async fn check(&self) -> Result<UpdateStatus, AppError> {
        let feed = self.feed()?;
        let (prev_phase, prev_release) = {
            let mut s = self.session();
            match s.phase {
                UpdatePhase::Idle | UpdatePhase::UpToDate { .. } | UpdatePhase::Found { .. } => {}
                _ => return Ok(self.status_of(&s)),
            }
            let prev = std::mem::replace(&mut s.phase, UpdatePhase::Checking);
            (prev, s.release.clone())
        };
        let found = feed.check().await;
        let mut s = self.session();
        match found {
            Ok(Some(release)) => {
                s.phase = UpdatePhase::Found {
                    version: release.version(),
                };
                s.release = Some(release);
            }
            Ok(None) => {
                s.phase = UpdatePhase::UpToDate {
                    checked_at: unix_now(),
                };
                s.release = None;
            }
            Err(e) => {
                s.phase = prev_phase;
                s.release = prev_release;
                return Err(e);
            }
        }
        Ok(self.status_of(&s))
    }

    fn status_of(&self, s: &Session) -> UpdateStatus {
        UpdateStatus {
            availability: self.availability,
            current_version: self.current_version.clone(),
            phase: s.phase.clone(),
        }
    }

    /// Downloads, verifies and installs the offered `version`: the one the
    /// user saw. Refuses `app.update_state` when no update, or another
    /// version, is on offer, or one is already being installed.
    pub async fn install(&self, version: &str) -> Result<UpdateStatus, AppError> {
        self.feed()?;
        let release = {
            let mut s = self.session();
            match (&s.phase, &s.release) {
                (UpdatePhase::Found { version: v }, Some(release)) if v == version => {
                    let release = Arc::clone(release);
                    s.phase = UpdatePhase::Downloading {
                        version: version.into(),
                        received: 0,
                        total: None,
                    };
                    release
                }
                (phase, _) => return Err(state_error(format!("install {version} in {phase:?}"))),
            }
        };
        let back_to_found = |e: AppError| {
            self.session().phase = UpdatePhase::Found {
                version: version.into(),
            };
            e
        };
        let progress = |received: u64, total: Option<u64>| {
            let mut s = self.session();
            if let UpdatePhase::Downloading { .. } = s.phase {
                s.phase = UpdatePhase::Downloading {
                    version: version.into(),
                    received,
                    total,
                };
            }
        };
        let bytes = release.download(&progress).await.map_err(back_to_found)?;
        self.session().phase = UpdatePhase::Installing {
            version: version.into(),
        };
        let installing = Arc::clone(&release);
        tauri::async_runtime::spawn_blocking(move || installing.install(&bytes))
            .await
            .map_err(|e| AppError::app(AppCode::UpdateInstall, e.to_string()))
            .and_then(|r| r)
            .map_err(back_to_found)?;
        let mut s = self.session();
        s.phase = UpdatePhase::Installed {
            version: version.into(),
        };
        Ok(self.status_of(&s))
    }

    /// Restarts into the installed update; `app.update_state` before one is.
    pub fn restart(&self) -> Result<(), AppError> {
        let feed = self.feed()?;
        let phase = self.session().phase.clone();
        match phase {
            UpdatePhase::Installed { .. } => {
                feed.restart();
                Ok(())
            }
            phase => Err(state_error(format!("restart in {phase:?}"))),
        }
    }

    /// The notes page of the update on offer or installed.
    pub fn notes_url(&self) -> Result<String, AppError> {
        match &self.session().phase {
            UpdatePhase::Found { version }
            | UpdatePhase::Downloading { version, .. }
            | UpdatePhase::Installing { version }
            | UpdatePhase::Installed { version } => Ok(notes_url(version)),
            phase => Err(state_error(format!("notes in {phase:?}"))),
        }
    }
}

fn state_error(message: String) -> AppError {
    AppError::app(AppCode::UpdateState, message)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The feed as the plugin reads it.
struct PluginFeed<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginFeed<R> {
    fn updater(&self) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
        let app = self.app.clone();
        self.app
            .updater_builder()
            .on_before_exit(move || {
                if let Some(state) = app.try_state::<crate::state::AppState>()
                    && let Err(e) = tauri::async_runtime::block_on(state.shutdown_engine())
                {
                    eprintln!("dash-wallet: engine shutdown before the update: {e}");
                }
                app.cleanup_before_exit();
            })
            .build()
    }
}

impl<R: Runtime> Feed for PluginFeed<R> {
    fn check(&self) -> BoxFuture<'_, Result<Option<Arc<dyn Release>>, AppError>> {
        Box::pin(async move {
            let check =
                |e: tauri_plugin_updater::Error| AppError::app(AppCode::UpdateCheck, e.to_string());
            let update = self
                .updater()
                .map_err(check)?
                .check()
                .await
                .map_err(check)?;
            Ok(update.map(|u| Arc::new(u) as Arc<dyn Release>))
        })
    }

    fn restart(&self) {
        self.app.request_restart();
    }
}

/// A signature that does not verify, or names another version, is
/// `app.update_signature`; anything else is `otherwise`.
pub(crate) fn plugin_error(e: tauri_plugin_updater::Error, otherwise: AppCode) -> AppError {
    use tauri_plugin_updater::Error as E;
    let code = match e {
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => AppCode::UpdateSignature,
        _ => otherwise,
    };
    AppError::app(code, e.to_string())
}

impl Release for tauri_plugin_updater::Update {
    fn version(&self) -> String {
        self.version.clone()
    }

    fn download<'a>(&'a self, progress: Progress<'a>) -> BoxFuture<'a, Result<Vec<u8>, AppError>> {
        Box::pin(async move {
            let mut received = 0u64;
            tauri_plugin_updater::Update::download(
                self,
                |chunk, total| {
                    received += chunk as u64;
                    progress(received, total);
                },
                || {},
            )
            .await
            .map_err(|e| plugin_error(e, AppCode::UpdateDownload))
        })
    }

    fn install(&self, bytes: &[u8]) -> Result<(), AppError> {
        tauri_plugin_updater::Update::install(self, bytes)
            .map_err(|e| plugin_error(e, AppCode::UpdateInstall))
    }
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
