//! The update check (PK-03a): notify only. One HTTPS GET of the latest
//! full release's `latest.json`, which `ci/github/release-publish.sh`
//! writes for `vX.Y.Z` tags only, and a semver comparison with the build's
//! own version (`BUILD_VERSION`: an rc is offered its final, a dev build
//! never its own `X.Y.Z`). Nothing is downloaded or installed.
//!
//! Off by default (D-N1): the request shows GitHub the user's IP address.
//! No request is made while it is off, and turning it off stops a request
//! in flight; nothing it brings is kept. The setting is app data, in
//! `<data_root>/updates.json`; the result lives in memory only.
//!
//! The request carries nothing of the wallet or the version: a fixed URL,
//! no cookie, the rates' User-Agent. Redirects are followed only over
//! HTTPS to GitHub's release hosts. Release notes open on a URL built here
//! from the version, never one the feed names.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::rates::{HttpGet, ReqwestGet, parse_rfc3339_utc};

/// The repository releases are published from.
const RELEASES: &str = "https://github.com/PastaPastaPasta/dashwallet-desktop/releases";
/// GitHub answers with the latest full release's asset (never a prerelease).
pub(crate) const FEED_URL: &str =
    "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/latest/download/latest.json";
/// The only hosts a request may reach: the feed's and its redirects'.
pub(crate) const HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 5;
/// `<data_root>/updates.json`.
pub(crate) const UPDATES_FILE: &str = "updates.json";

/// A newer full release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// `X.Y.Z`.
    pub version: String,
    /// When it was published (UNIX seconds), when the feed says.
    pub published_at: Option<u64>,
    /// Its release page.
    pub notes_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    /// The check is off: no request is made.
    Off,
    /// On, and no check finished since the app started.
    NotChecked,
    /// The latest full release is not newer than this one.
    Current,
    Available(Release),
    /// The last check failed (network, HTTP status, a feed that does not
    /// parse).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatesView {
    pub enabled: bool,
    pub state: UpdateState,
    /// When the last check finished (UNIX seconds).
    pub checked_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdatesError {
    /// A check while the check is off, or turned off while it ran.
    #[error("the update check is off")]
    Off,
    /// `updates.json` could not be written.
    #[error("storage: {0}")]
    Storage(String),
}

impl UpdatesError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Off => "updates.off",
            Self::Storage(_) => "updates.storage",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stored {
    enabled: bool,
}

/// `X.Y.Z` (with an optional leading `v`) as numbers; `None` for anything
/// else, a prerelease (`-rc.1`, `-dev.3`) or build suffix included.
type Triple = (u64, u64, u64);

pub(crate) fn release_version(v: &str) -> Option<Triple> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.split('.').map(|p| {
        (!p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    });
    let triple = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(triple)
}

/// `X.Y.Z[-<suffix>]` as its numbers and the suffix; `None` when the
/// `X.Y.Z` part does not parse or the suffix is empty.
fn split_version(v: &str) -> Option<(Triple, Option<&str>)> {
    let (core, suffix) = match v.split_once('-') {
        Some((_, "")) => return None,
        Some((core, suffix)) => (core, Some(suffix)),
        None => (v, None),
    };
    Some((release_version(core)?, suffix))
}

/// Where a build stands among releases: `X.Y.Z-rc.N` before `X.Y.Z`, a dev
/// build (any other suffix) level with its `X.Y.Z`, so it is never offered
/// the release it came from.
fn build_rank(current: &str) -> Option<(Triple, u8)> {
    let (triple, suffix) = split_version(current)?;
    let rc = suffix.is_some_and(|s| s.starts_with("rc."));
    Some((triple, if rc { 0 } else { 1 }))
}

/// What a feed body means for a build of `current` (`BUILD_VERSION`): a
/// newer full release, or `None` (not newer, or a prerelease). An error for
/// a body that is not a feed or a version that is malformed, so a broken
/// publish shows as a failed check, never as "up to date".
pub(crate) fn parse_feed(body: &[u8], current: &str) -> Result<Option<Release>, String> {
    #[derive(Deserialize)]
    struct Feed {
        version: String,
        pub_date: Option<String>,
    }
    let feed: Feed = serde_json::from_slice(body).map_err(|e| format!("latest.json: {e}"))?;
    let (latest, suffix) = split_version(&feed.version)
        .ok_or_else(|| format!("latest.json: not a version: {:?}", feed.version))?;
    if suffix.is_some() {
        return Ok(None);
    }
    let ours = build_rank(current).ok_or_else(|| format!("not a version: {current}"))?;
    if (latest, 1) <= ours {
        return Ok(None);
    }
    let version = format!("{}.{}.{}", latest.0, latest.1, latest.2);
    Ok(Some(Release {
        notes_url: format!("{RELEASES}/tag/v{version}"),
        published_at: feed.pub_date.as_deref().and_then(parse_rfc3339_utc),
        version,
    }))
}

/// Whether a request may go to `url`: HTTPS, the default port, a pinned host.
pub(crate) fn pinned(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.host_str().is_some_and(|h| HOSTS.contains(&h))
}

fn client() -> Result<ReqwestGet, String> {
    ReqwestGet::with_redirects(reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if pinned(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("a redirect off GitHub")
        }
    }))
}

struct Inner {
    stored: Stored,
    state: UpdateState,
    checked_at: Option<u64>,
    /// Raised each time the check is turned off: a check started before
    /// keeps nothing it brings.
    off_epoch: u64,
}

pub(crate) struct UpdatesHub {
    path: PathBuf,
    http: Option<Arc<dyn HttpGet>>,
    current: &'static str,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    inner: Mutex<Inner>,
    /// One check at a time.
    check: tokio::sync::Mutex<()>,
    /// Notified when the check is turned off: the request in flight stops.
    off: Notify,
}

impl UpdatesHub {
    /// Reads `<data_root>/updates.json`; a missing or unreadable file is off.
    pub(crate) fn new(
        data_root: &Path,
        http: Option<Arc<dyn HttpGet>>,
        current: &'static str,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Arc<Self> {
        let path = data_root.join(UPDATES_FILE);
        let stored: Stored = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let http = http.or_else(|| {
            client()
                .inspect_err(|e| tracing::warn!(error = %e, "update check HTTP client unavailable"))
                .ok()
                .map(|c| Arc::new(c) as Arc<dyn HttpGet>)
        });
        Arc::new(Self {
            path,
            http,
            current,
            clock,
            inner: Mutex::new(Inner {
                state: if stored.enabled {
                    UpdateState::NotChecked
                } else {
                    UpdateState::Off
                },
                stored,
                checked_at: None,
                off_epoch: 0,
            }),
            check: tokio::sync::Mutex::new(()),
            off: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn view(&self) -> UpdatesView {
        let inner = self.lock();
        UpdatesView {
            enabled: inner.stored.enabled,
            state: inner.state.clone(),
            checked_at: inner.checked_at,
        }
    }

    /// Saves the setting. Off stops the check in flight and forgets the
    /// last result, even when `updates.json` cannot be written (the error
    /// is returned then; a restart may read the check as on again).
    pub(crate) fn set_enabled(&self, enabled: bool) -> Result<UpdatesView, UpdatesError> {
        let saved = {
            let mut inner = self.lock();
            let saved = write_private(&self.path, &Stored { enabled });
            if enabled {
                saved.clone()?;
            }
            if enabled != inner.stored.enabled {
                inner.stored.enabled = enabled;
                inner.checked_at = None;
                inner.state = if enabled {
                    UpdateState::NotChecked
                } else {
                    inner.off_epoch += 1;
                    UpdateState::Off
                };
            }
            saved
        };
        if !enabled {
            self.off.notify_waiters();
        }
        saved.map(|()| self.view())
    }

    /// One check now. `Off` while the check is off or when it is turned
    /// off before the answer is in.
    pub(crate) async fn check(&self) -> Result<UpdatesView, UpdatesError> {
        let _one = self.check.lock().await;
        // Listening before the epoch is read: an Off after this is heard.
        let off = self.off.notified();
        tokio::pin!(off);
        off.as_mut().enable();
        let epoch = {
            let inner = self.lock();
            if !inner.stored.enabled {
                return Err(UpdatesError::Off);
            }
            inner.off_epoch
        };
        let fetch = async {
            let http = self.http.as_ref().ok_or("no HTTP client")?;
            let body = http.get(FEED_URL).await?;
            parse_feed(&body, self.current)
        };
        let got = tokio::select! {
            biased;
            () = &mut off => return Err(UpdatesError::Off),
            got = fetch => got,
        };
        let mut inner = self.lock();
        if inner.off_epoch != epoch || !inner.stored.enabled {
            return Err(UpdatesError::Off);
        }
        inner.state = match got {
            Ok(Some(release)) => UpdateState::Available(release),
            Ok(None) => UpdateState::Current,
            Err(e) => {
                tracing::info!(error = %e, "update check failed");
                UpdateState::Failed
            }
        };
        inner.checked_at = Some((self.clock)());
        drop(inner);
        Ok(self.view())
    }
}

fn write_private(path: &Path, stored: &Stored) -> Result<(), UpdatesError> {
    let json = serde_json::to_vec(stored).map_err(|e| UpdatesError::Storage(e.to_string()))?;
    crate::rates::write_private(path, &json)
        .map_err(|e| UpdatesError::Storage(format!("{UPDATES_FILE}: {e}")))
}

/// The engine's update check. A cheap handle on the engine's one hub.
#[derive(Clone)]
pub struct Updates {
    hub: Arc<UpdatesHub>,
}

impl crate::Engine {
    /// The update check.
    pub fn updates(&self) -> Updates {
        Updates {
            hub: self.updates_hub(),
        }
    }
}

impl Updates {
    /// The setting and the last check's result.
    pub fn status(&self) -> UpdatesView {
        self.hub.view()
    }

    /// Saves the setting (`updates.json`); off stops a check in flight and
    /// forgets the last result.
    pub fn set_enabled(&self, enabled: bool) -> Result<UpdatesView, UpdatesError> {
        self.hub.set_enabled(enabled)
    }

    /// One request for the latest full release; `Off` while the check is off.
    pub async fn check(&self) -> Result<UpdatesView, UpdatesError> {
        self.hub.check().await
    }
}
