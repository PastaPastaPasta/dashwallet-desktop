//! Opt-in updates from the signed GitHub Releases feed (PK-03). The build,
//! the install and the user each have to allow them: docs/release/updater.md.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use semver::Version;
use serde::Serialize;
use serde_json::Value;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;
use ts_rs::TS;

use crate::error::{AppCode, AppError};

/// The stable channel's feed: the `latest.json` of the newest full release.
/// GitHub's "latest" is never a pre-release or a draft.
pub const FEED_URL: &str =
    "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/latest/download/latest.json";

/// `plugins.updater.pubkey` in `tauri.conf.json` until the release key
/// exists (docs/release/updater.md): a build with it has no updater.
pub const PUBKEY_PLACEHOLDER: &str =
    "UPDATER-DISABLED: replace with the release public key (docs/release/updater.md)";

/// How long a check may take, and a download may go without data.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_IDLE: Duration = Duration::from_secs(60);

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
    /// A macOS app run from its disk image or a translocated path, which it
    /// cannot replace: it has to be moved to Applications first.
    MoveToApplications,
}

/// What [`availability`] needs to know about the running copy.
#[derive(Debug, Clone)]
pub struct Install {
    pub flatpak: bool,
    pub bundle: Option<BundleType>,
    /// Run as an AppImage (`$APPIMAGE` names the file the update replaces).
    pub appimage: bool,
    /// A macOS app that cannot replace itself where it runs ([`movable_only`]).
    pub read_only: bool,
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
        let read_only = cfg!(target_os = "macos")
            && std::env::current_exe().is_ok_and(|exe| movable_only(&exe));
        Install {
            flatpak: is_flatpak(),
            bundle: tauri::utils::platform::bundle_type(),
            appimage,
            read_only,
        }
    }
}

/// Flatpak puts `/.flatpak-info` in every sandbox and sets `FLATPAK_ID`.
fn is_flatpak() -> bool {
    cfg!(target_os = "linux")
        && (std::path::Path::new("/.flatpak-info").exists()
            || std::env::var_os("FLATPAK_ID").is_some())
}

/// A macOS executable that Gatekeeper translocated, or that runs from a
/// mounted volume such as the disk image.
pub fn movable_only(exe: &Path) -> bool {
    let path = exe.to_string_lossy();
    path.contains("/AppTranslocation/") || path.starts_with("/Volumes/")
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
        Some(BundleType::App) if install.read_only => A::MoveToApplications,
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
    /// The last check found nothing newer.
    UpToDate,
    /// A newer stable release, not downloaded.
    Found {
        version: String,
    },
    /// Downloaded and verified, kept while Install waited for something
    /// running that closing the engine would cut (Windows).
    Downloaded {
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
    /// The engine was shut down for the Windows installer, which then failed
    /// to start: only a restart brings the wallets back.
    RestartRequired {
        version: String,
    },
}

/// `update_status`: what Settings shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub availability: UpdateAvailability,
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
    /// Shows the main window again, which the Windows installer's hook hid.
    fn reveal(&self);
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
    engine_closed: AtomicBool,
    closes_engine: bool,
    check_timeout: Duration,
    download_idle: Duration,
}

struct Session {
    phase: UpdatePhase,
    release: Option<Arc<dyn Release>>,
    downloaded: Option<Vec<u8>>,
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
                downloaded: None,
            }),
            engine_closed: AtomicBool::new(false),
            closes_engine: cfg!(windows),
            check_timeout: CHECK_TIMEOUT,
            download_idle: DOWNLOAD_IDLE,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timeouts(mut self, check: Duration, download_idle: Duration) -> Self {
        self.check_timeout = check;
        self.download_idle = download_idle;
        self
    }

    /// As on Windows, whatever the test runs on.
    #[cfg(test)]
    pub(crate) fn closing_engine(mut self) -> Self {
        self.closes_engine = true;
        self
    }

    /// The app's: the plugin's feed when this copy can update itself.
    pub fn for_app<R: Runtime>(app: &AppHandle<R>) -> Self {
        let availability = availability(app.config(), &Install::current(app));
        let feed = PluginFeed { app: app.clone() };
        Self::new(
            availability,
            app.package_info().version.to_string(),
            Some(Box::new(feed)),
        )
    }

    /// Installing closes the engine (Windows: the installer replaces the
    /// running app). Elsewhere only the restart does.
    pub fn install_closes_engine(&self) -> bool {
        self.closes_engine
    }

    /// The version the feed is compared with, which About shows too.
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> UpdateStatus {
        self.status_of(&self.session())
    }

    fn status_of(&self, s: &Session) -> UpdateStatus {
        UpdateStatus {
            availability: self.availability,
            phase: s.phase.clone(),
        }
    }

    /// `app.updates_unavailable` when this copy cannot update itself.
    pub(crate) fn feed(&self) -> Result<&dyn Feed, AppError> {
        self.feed.as_deref().ok_or_else(|| {
            AppError::app(
                AppCode::UpdatesUnavailable,
                format!("{:?}", self.availability),
            )
        })
    }

    /// Asks the feed once, for at most 30 s. While a check, download or
    /// install is under way, or once an update is installed, it fetches
    /// nothing and returns the status. A failed check leaves the status as
    /// it was.
    pub async fn check(&self) -> Result<UpdateStatus, AppError> {
        let feed = self.feed()?;
        let prev = {
            let mut s = self.session();
            match s.phase {
                UpdatePhase::Idle | UpdatePhase::UpToDate | UpdatePhase::Found { .. } => {}
                _ => return Ok(self.status_of(&s)),
            }
            std::mem::replace(&mut s.phase, UpdatePhase::Checking)
        };
        let found = tokio::time::timeout(self.check_timeout, feed.check())
            .await
            .unwrap_or_else(|_| {
                Err(AppError::app(
                    AppCode::UpdateCheck,
                    "the update check timed out",
                ))
            });
        let mut s = self.session();
        match found {
            Ok(Some(release)) => {
                s.phase = UpdatePhase::Found {
                    version: release.version(),
                };
                s.release = Some(release);
            }
            Ok(None) => {
                s.phase = UpdatePhase::UpToDate;
                s.release = None;
            }
            Err(e) => {
                s.phase = prev;
                return Err(e);
            }
        }
        Ok(self.status_of(&s))
    }

    /// Downloads, verifies and installs the offered `version`: the one the
    /// user saw. Refuses `app.update_state` when no update, or another
    /// version, is on offer, or one is already being installed. A download
    /// that receives nothing for 60 s fails with `app.update_download`.
    /// Where installing closes the engine, `before_close` runs once the
    /// download is done: when it refuses, the download is kept and the
    /// phase is `Downloaded`, so a later Install does not fetch it again.
    pub async fn install<F>(
        &self,
        version: &str,
        before_close: impl FnOnce() -> F,
    ) -> Result<UpdateStatus, AppError>
    where
        F: Future<Output = Result<(), AppError>>,
    {
        let feed = self.feed()?;
        let (release, kept) = {
            let mut s = self.session();
            let ready = matches!(&s.phase, UpdatePhase::Downloaded { version: v } if v == version);
            let offered = matches!(&s.phase, UpdatePhase::Found { version: v } if v == version);
            let release = match &s.release {
                Some(release) if ready || offered => Arc::clone(release),
                _ => return Err(state_error(format!("install {version} in {:?}", s.phase))),
            };
            let kept = if ready { s.downloaded.take() } else { None };
            s.phase = UpdatePhase::Downloading {
                version: version.into(),
                received: 0,
                total: None,
            };
            (release, kept)
        };
        let failed = |e: AppError| {
            let version = version.into();
            let closed = self.engine_closed.load(Ordering::SeqCst);
            self.session().phase = if closed {
                UpdatePhase::RestartRequired { version }
            } else {
                UpdatePhase::Found { version }
            };
            if closed {
                feed.reveal();
            }
            e
        };
        let bytes = match kept {
            Some(bytes) => bytes,
            None => {
                let last_data = Mutex::new(Instant::now());
                let progress = |received: u64, total: Option<u64>| {
                    *last_data.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                    self.session().phase = UpdatePhase::Downloading {
                        version: version.into(),
                        received,
                        total,
                    };
                };
                self.unless_idle(release.download(&progress), &last_data)
                    .await
                    .map_err(failed)?
            }
        };
        self.session().phase = UpdatePhase::Installing {
            version: version.into(),
        };
        if self.closes_engine
            && let Err(e) = before_close().await
        {
            let mut s = self.session();
            s.downloaded = Some(bytes);
            s.phase = UpdatePhase::Downloaded {
                version: version.into(),
            };
            return Err(e);
        }
        let installing = Arc::clone(&release);
        tauri::async_runtime::spawn_blocking(move || installing.install(&bytes))
            .await
            .map_err(|e| AppError::app(AppCode::UpdateInstall, e.to_string()))
            .and_then(|r| r)
            .map_err(failed)?;
        let mut s = self.session();
        s.phase = UpdatePhase::Installed {
            version: version.into(),
        };
        Ok(self.status_of(&s))
    }

    /// `download`, unless no data arrives for `download_idle`.
    async fn unless_idle<T>(
        &self,
        download: impl Future<Output = Result<T, AppError>>,
        last_data: &Mutex<Instant>,
    ) -> Result<T, AppError> {
        tokio::pin!(download);
        loop {
            let quiet = last_data
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .elapsed();
            let Some(wait) = self
                .download_idle
                .checked_sub(quiet)
                .filter(|w| !w.is_zero())
            else {
                return Err(AppError::app(
                    AppCode::UpdateDownload,
                    "the download stalled",
                ));
            };
            tokio::select! {
                done = &mut download => return done,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// The Windows installer's hook ran: the engine is shut down.
    pub(crate) fn engine_closed(&self) {
        self.engine_closed.store(true, Ordering::SeqCst);
    }

    /// The page of the release on offer, on this repository: what "Release
    /// notes" opens. `app.update_state` when no release is on offer.
    pub fn notes_url(&self) -> Result<String, AppError> {
        let phase = self.session().phase.clone();
        let version = match &phase {
            UpdatePhase::Found { version }
            | UpdatePhase::Downloaded { version }
            | UpdatePhase::Downloading { version, .. }
            | UpdatePhase::Installing { version }
            | UpdatePhase::Installed { version }
            | UpdatePhase::RestartRequired { version } => Version::parse(version)
                .map_err(|e| state_error(format!("release notes of {version}: {e}")))?,
            _ => return Err(state_error(format!("release notes in {phase:?}"))),
        };
        Ok(format!("{}/tag/v{version}", crate::api::app::RELEASES_URL))
    }

    /// Restarts into the installed update, or after a failed Windows install
    /// shut the engine down; `app.update_state` otherwise.
    pub fn restart(&self) -> Result<(), AppError> {
        let feed = self.feed()?;
        let phase = self.session().phase.clone();
        match phase {
            UpdatePhase::Installed { .. } | UpdatePhase::RestartRequired { .. } => {
                feed.restart();
                Ok(())
            }
            phase => Err(state_error(format!("restart in {phase:?}"))),
        }
    }
}

fn state_error(message: String) -> AppError {
    AppError::app(AppCode::UpdateState, message)
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
            .timeout(CHECK_TIMEOUT)
            .on_before_exit(move || {
                if let Some(state) = app.try_state::<crate::state::AppState>() {
                    state.updates.engine_closed();
                    if let Err(e) = tauri::async_runtime::block_on(state.shutdown_engine()) {
                        eprintln!("dash-wallet: engine shutdown before the update: {e}");
                    }
                }
                app.cleanup_before_exit();
            })
            .build()
    }
}

impl<R: Runtime> Feed for PluginFeed<R> {
    fn check(&self) -> BoxFuture<'_, Result<Option<Arc<dyn Release>>, AppError>> {
        Box::pin(async move {
            let updater = self
                .updater()
                .map_err(|e| plugin_error(e, AppCode::UpdateCheck))?;
            let update = check_feed(&updater, FEED_URL).await?;
            Ok(update.map(|u| Arc::new(u) as Arc<dyn Release>))
        })
    }

    fn restart(&self) {
        self.app.request_restart();
    }

    fn reveal(&self) {
        crate::shell::reveal(&self.app);
    }
}

/// The plugin's check of the feed at `url`. The plugin reports every
/// non-2xx answer as "no release", so `url` is asked once more for its
/// status: a 404 (the Latest release has no `latest.json`) is
/// `app.update_not_published`, anything else `app.update_check`.
pub(crate) async fn check_feed(
    updater: &tauri_plugin_updater::Updater,
    url: &str,
) -> Result<Option<tauri_plugin_updater::Update>, AppError> {
    match updater.check().await {
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => {
            let status = async {
                reqwest::Client::builder()
                    .timeout(CHECK_TIMEOUT)
                    .build()?
                    .get(url)
                    .send()
                    .await
                    .map(|r| r.status())
            };
            let code = match status.await {
                Ok(reqwest::StatusCode::NOT_FOUND) => AppCode::UpdateNotPublished,
                _ => AppCode::UpdateCheck,
            };
            Err(AppError::app(code, "the feed answered without a release"))
        }
        found => found.map_err(|e| plugin_error(e, AppCode::UpdateCheck)),
    }
}

/// A signature that does not verify, or names another version or none, is
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
