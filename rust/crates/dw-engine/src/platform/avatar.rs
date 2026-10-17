//! The session's avatar pipeline (DASHPAY §3.8; DP4-02 design): the read
//! behind `DashPay::avatar` (profile, cache, fetch, verdict), the sources
//! behind `prepare_avatar`, the candidates `update_profile` publishes, and
//! the "Load contact pictures" toggle. The guard, the decoder and the store
//! are `avatar_fetch.rs`, `avatar_image.rs` and `avatar_cache.rs`. Not part
//! of the facade.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{AppDb, AvatarRow};
use reqwest::Url;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use super::avatar_cache::{AvatarStore, ThumbDir, stem};
use super::avatar_fetch::{
    FetchError, FetchPolicy, Fetcher, MAX_BYTES, MAX_URL_LEN, NameResolver, SystemResolver,
};
use super::avatar_image::{self, Crop, Ingested};
use super::contacts_model::identifier;
use super::errors::{AvatarError, PlatformError};
use super::profile::{AvatarCandidate, AvatarImage, AvatarSize, AvatarSource, Profile};
use super::requests::RequestNet;
use crate::events::unix_now;
use crate::{NetworkSession, WalletId};

/// The toggle's key in app.sqlite (global scope; D17): on unless `"0"`.
pub(super) const LOAD_PICTURES_KEY: &str = "dashpay.load_contact_pictures";
/// A failed URL, or one whose content another profile asked for, is
/// fetched again after this (D11).
const RECHECK: Duration = Duration::from_secs(600);
/// Fetches at once per session, and decodes.
const FETCHES: usize = 4;
const DECODES: usize = 2;
/// Candidates kept, and for how long (design: "Sources").
const CANDIDATES: usize = 16;
const CANDIDATE_LIFE: Duration = Duration::from_secs(30 * 60);
/// Profiles of identities the wallet does not hold (D14), and the
/// Platform reads of them at once (D35).
const REMOTE_PROFILES: usize = 256;
const REMOTE_PROFILE_LIFE: Duration = Duration::from_secs(600);
const REMOTE_READS: usize = 4;
/// A failed Platform read is the answer for this long, so the reads queued
/// behind it share it rather than each asking again (D35).
const REMOTE_FAILURE_LIFE: Duration = Duration::from_secs(5);

type RemoteRead = Result<Option<Profile>, AvatarError>;

/// How long a kept Platform read answers.
fn remote_life(read: &RemoteRead) -> Duration {
    if read.is_ok() {
        REMOTE_PROFILE_LIFE
    } else {
        REMOTE_FAILURE_LIFE
    }
}
/// Android's crop parameter (D15).
const ZOOM_PARAM: &str = "dashpay-profile-pic-zoom";

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

type Locks<K> = Mutex<HashMap<K, Weak<tokio::sync::Mutex<()>>>>;

/// The lock for `key` in `locks`, shared by every holder while one lives
/// (dead entries are dropped as new ones come).
fn keyed_lock<K: Hash + Eq + Clone>(locks: &Locks<K>, key: &K) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = guard(locks);
    locks.retain(|_, l| l.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key.clone(), Arc::downgrade(&lock));
    lock
}

/// What a read saw of "Load contact pictures" when it began (D33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ticket {
    on: bool,
    epoch: u64,
}

#[cfg(test)]
pub(super) type Hook = Box<
    dyn Fn(&'static str) -> Option<std::pin::Pin<Box<dyn Future<Output = ()> + Send>>>
        + Send
        + Sync,
>;

/// "Load contact pictures" for the session (D17). Every transition moves
/// the epoch, so a read that began before one can tell (D33), and
/// transitions are serialized across their durable write, so memory is
/// always the last value written (D34).
pub(super) struct Pictures {
    state: tokio::sync::watch::Sender<(bool, u64)>,
    transitions: tokio::sync::Mutex<()>,
    /// Test pauses at named points of a read.
    #[cfg(test)]
    hook: Mutex<Option<Hook>>,
}

impl Pictures {
    pub(super) fn new(on: bool) -> Self {
        Self {
            state: tokio::sync::watch::Sender::new((on, 0)),
            transitions: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            hook: Mutex::default(),
        }
    }

    pub(super) fn on(&self) -> bool {
        self.state.borrow().0
    }

    /// The toggle as a read beginning now sees it.
    pub(super) fn ticket(&self) -> Ticket {
        let (on, epoch) = *self.state.borrow();
        Ticket { on, epoch }
    }

    /// Whether a read holding `ticket` may still use the network: it began
    /// with the toggle on, and nothing has moved it since.
    pub(super) fn allows(&self, ticket: Ticket) -> bool {
        ticket.on && *self.state.borrow() == (true, ticket.epoch)
    }

    /// Sets the toggle now, without a durable write (at open, and tests).
    pub(super) fn set(&self, on: bool) {
        self.state.send_if_modified(|(now, epoch)| {
            let moved = *now != on;
            if moved {
                (*now, *epoch) = (on, *epoch + 1);
            }
            moved
        });
    }

    /// A transition: `write` (the durable one) and then memory, one
    /// transition at a time (D34). Memory stays as it was if `write` fails.
    pub(super) async fn transition<E>(
        &self,
        on: bool,
        write: impl Future<Output = Result<(), E>>,
    ) -> Result<(), E> {
        let _one = self.transitions.lock().await;
        write.await?;
        self.set(on);
        Ok(())
    }

    /// `work`, unless `ticket` stops allowing the network first: then
    /// `None`, and `work` is dropped where it stands (D33).
    pub(super) async fn unless_revoked<T>(
        &self,
        ticket: Ticket,
        work: impl Future<Output = T>,
    ) -> Option<T> {
        if !self.allows(ticket) {
            return None;
        }
        let mut state = self.state.subscribe();
        let revoked = state.wait_for(|&(on, epoch)| !(on && epoch == ticket.epoch));
        tokio::select! {
            biased;
            _ = revoked => None,
            done = work => self.allows(ticket).then_some(done),
        }
    }

    /// A named point of a read, where a test may pause it.
    async fn at(&self, point: &'static str) {
        #[cfg(test)]
        {
            let pause = guard(&self.hook).as_ref().and_then(|hook| hook(point));
            if let Some(pause) = pause {
                pause.await;
            }
        }
        #[cfg(not(test))]
        let _ = point;
    }

    #[cfg(test)]
    pub(super) fn set_hook(&self, hook: Hook) {
        *guard(&self.hook) = Some(hook);
    }
}

/// Whose avatar a read is for (D17). While "Load contact pictures" is
/// off, an own identity's comes from the cache only and another user's is
/// not read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Whose {
    Own,
    Other,
}

/// An avatar URL as published, and what is fetched for it.
#[derive(Debug, Clone)]
pub(super) struct Target {
    /// SHA-256 of the URL as published, hex: the index key.
    pub(super) url_sha: String,
    /// The URL fetched: the published one without Android's crop.
    pub(super) fetch: Url,
    pub(super) crop: Option<Crop>,
}

impl Target {
    pub(super) fn parse(published: &str) -> Result<Self, FetchError> {
        let mut fetch =
            Url::parse(published).map_err(|e| FetchError::Form(format!("not a URL: {e}")))?;
        // Only the zoom parameter's segments go; the rest of the query is
        // kept byte for byte (a re-encoding could change what is fetched).
        let mut crop = None;
        if let Some(query) = fetch.query().map(str::to_owned) {
            let (zoom, rest): (Vec<&str>, Vec<&str>) = query.split('&').partition(|segment| {
                (url::form_urlencoded::parse(segment.as_bytes()).next())
                    .is_some_and(|(k, _)| k == ZOOM_PARAM)
            });
            if !zoom.is_empty() {
                crop = (url::form_urlencoded::parse(zoom[0].as_bytes()).next())
                    .and_then(|(_, v)| Crop::from_zoom(&v));
                fetch.set_query((!rest.is_empty()).then(|| rest.join("&")).as_deref());
            }
        }
        Ok(Self {
            url_sha: hex::encode(Sha256::digest(published.as_bytes())),
            fetch,
            crop,
        })
    }
}

/// What a fresh row that does not verify answers: its fetch's failure, or
/// a mismatch when the fetch succeeded.
fn refusal(status: &str) -> AvatarError {
    match status {
        "ok" => AvatarError::HashMismatch,
        "too_large" => AvatarError::TooLarge,
        "unsupported" => AvatarError::Unsupported,
        _ => AvatarError::FetchFailed,
    }
}

fn status_of(e: &AvatarError) -> &'static str {
    match e {
        AvatarError::TooLarge => "too_large",
        AvatarError::Unsupported => "unsupported",
        _ => "fetch_failed",
    }
}

fn fetch_error(e: FetchError) -> AvatarError {
    match e {
        FetchError::TooLarge => AvatarError::TooLarge,
        _ => AvatarError::FetchFailed,
    }
}

/// Whether `row` was fetched within [`RECHECK`]; a time ahead of the clock
/// is not fresh.
fn fresh(row: &AvatarRow, now: u64) -> bool {
    now.checked_sub(row.fetched_at)
        .is_some_and(|age| age < RECHECK.as_secs())
}

/// Fetch, decode, cache: what a session shares between its reads.
pub(super) struct Pipeline {
    fetcher: Fetcher,
    store: Arc<AvatarStore>,
    /// One fetch per URL at a time (D12), by `url_sha`.
    url_locks: Locks<String>,
    fetches: Semaphore,
    decodes: Arc<Semaphore>,
    /// "Load contact pictures", shared with the runtime (D17, D33).
    pictures: Arc<Pictures>,
}

impl Pipeline {
    pub(super) fn new(
        policy: FetchPolicy,
        resolver: Arc<dyn NameResolver>,
        dir: Option<ThumbDir>,
        db: Arc<AppDb>,
        pictures: Arc<Pictures>,
    ) -> Self {
        Self {
            fetcher: Fetcher::new(policy, resolver),
            store: Arc::new(AvatarStore::new(dir, db)),
            url_locks: Mutex::default(),
            fetches: Semaphore::new(FETCHES),
            decodes: Arc::new(Semaphore::new(DECODES)),
            pictures,
        }
    }

    fn url_lock(&self, url_sha: &str) -> Arc<tokio::sync::Mutex<()>> {
        keyed_lock(&self.url_locks, &url_sha.to_owned())
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&AvatarStore) -> T + Send + 'static,
    ) -> Result<T, AvatarError> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| {
                AvatarError::from(PlatformError::Internal {
                    detail: format!("avatar store task: {e}"),
                })
            })
    }

    /// Runs `work` (a decode) on a blocking thread, at most [`DECODES`] at
    /// once; a panic is `avatar.unsupported`.
    async fn decoding<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, AvatarError> + Send + 'static,
    ) -> Result<T, AvatarError> {
        // The permit goes with the work: a dropped caller does not free it
        // while the decode still runs.
        let permit = (Arc::clone(&self.decodes).acquire_owned().await)
            .map_err(|_| AvatarError::Unsupported)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .unwrap_or(Err(AvatarError::Unsupported))
    }

    /// Fetches and ingests `target` (with the fingerprint for a candidate),
    /// and records the outcome in the index.
    async fn fetch(&self, target: &Target, fingerprint: bool) -> Result<Ingested, AvatarError> {
        let fetched = {
            let _permit = self.fetches.acquire().await;
            self.fetcher.fetch(&target.fetch).await
        };
        let crop = target.crop;
        let ingested = match fetched {
            Ok(bytes) => {
                self.decoding(move || avatar_image::ingest(&bytes, crop, fingerprint))
                    .await
            }
            Err(e) => {
                tracing::debug!(error = %e, "avatar fetch failed");
                Err(fetch_error(e))
            }
        };
        let (url_sha, now) = (target.url_sha.clone(), unix_now());
        let recorded = match &ingested {
            Ok(i) => {
                let (i, stem) = (i.clone(), stem(&i.sha, target.crop.as_ref()));
                self.blocking(move |s| s.record_ok(&url_sha, &stem, &i, now))
                    .await
            }
            Err(e) => {
                let status = status_of(e);
                self.blocking(move |s| s.record_failure(&url_sha, status, now))
                    .await
            }
        };
        // The store failing costs only the cache: the caller judges the
        // ingested picture itself.
        if let Err(e) = recorded {
            tracing::warn!(error = %e, "avatar not recorded");
        }
        ingested
    }

    /// The thumbnail at `size` of `profile`'s avatar, verified against it
    /// (design: "The read"). `None` when the profile has no avatar, or
    /// while "Load contact pictures" is off, for another user's or for an
    /// own identity's with nothing verified cached. A profile whose hash or
    /// fingerprint could match nothing is a mismatch without a fetch.
    /// `ticket` is the toggle as the read began (D33): the network is used
    /// only while it allows it, a fetch it stops allowing is abandoned, and
    /// another user's read it stops allowing answers `None` whatever it
    /// found.
    pub(super) async fn for_profile(
        &self,
        profile: &Profile,
        size: AvatarSize,
        whose: Whose,
        ticket: Ticket,
    ) -> Result<Option<Vec<u8>>, AvatarError> {
        let read = self.read(profile, size, whose, ticket).await;
        if whose == Whose::Other && !self.pictures.allows(ticket) {
            return Ok(None);
        }
        read
    }

    async fn read(
        &self,
        profile: &Profile,
        size: AvatarSize,
        whose: Whose,
        ticket: Ticket,
    ) -> Result<Option<Vec<u8>>, AvatarError> {
        let Some(published) = profile.avatar_url.as_deref() else {
            return Ok(None);
        };
        let network = || self.pictures.allows(ticket);
        if whose == Whose::Other && !network() {
            return Ok(None);
        }
        if !avatar_image::verifiable(profile) {
            return Err(AvatarError::HashMismatch);
        }
        let target = Target::parse(published).map_err(|_| AvatarError::FetchFailed)?;
        let lock = self.url_lock(&target.url_sha);
        let _one = lock.lock().await;
        if whose == Whose::Other && !network() {
            return Ok(None);
        }
        let key = target.url_sha.clone();
        let row = self.blocking(move |s| s.row(&key)).await?;
        self.pictures.at("row").await;
        if let Some(row) = row {
            let verified = row.status == "ok"
                && avatar_image::matches(row.content_sha.as_deref().unwrap_or(""), profile);
            if verified {
                if let Some(png) = self.blocking(move |s| s.thumbnail(&row, size)).await? {
                    self.pictures.at("thumbnail").await;
                    return Ok(Some(png));
                }
            } else if !network() {
                return Ok(None);
            } else if fresh(&row, unix_now()) {
                return Err(refusal(&row.status));
            }
        }
        let fetch = self.fetch(&target, false);
        let Some(ingested) = self.pictures.unless_revoked(ticket, fetch).await else {
            return Ok(None);
        };
        let ingested = ingested?;
        if !avatar_image::matches(&hex::encode(ingested.sha), profile) {
            return Err(AvatarError::HashMismatch);
        }
        Ok(Some(ingested.thumbnail(size).to_vec()))
    }

    /// `prepare_avatar`'s fetch of a URL the user gave: always fetched now,
    /// recorded in the index. Returns the URL to publish (as parsed, so
    /// what is published is what was checked) and what it holds.
    pub(super) async fn prepare_url(&self, url: &str) -> Result<(String, Ingested), AvatarError> {
        let published = Url::parse(url.trim()).map_err(invalid)?.to_string();
        if published.len() > MAX_URL_LEN {
            return Err(invalid(format!("longer than {MAX_URL_LEN} bytes")));
        }
        let target = Target::parse(&published).map_err(invalid)?;
        self.fetcher.check_url(&target.fetch).map_err(|e| match e {
            FetchError::Form(_) => invalid(e),
            _ => AvatarError::FetchFailed,
        })?;
        let lock = self.url_lock(&target.url_sha);
        let _one = lock.lock().await;
        Ok((published, self.fetch(&target, true).await?))
    }
}

fn invalid(e: impl std::fmt::Display) -> AvatarError {
    PlatformError::InvalidArgument {
        detail: format!("avatar URL: {e}"),
    }
    .into()
}

/// A prepared avatar (`prepare_avatar`), kept for `update_profile`.
#[derive(Debug, Clone)]
pub(super) struct Candidate {
    pub(super) wallet: WalletId,
    pub(super) url: Option<String>,
    pub(super) sha: [u8; 32],
    pub(super) dhash: [u8; 8],
    /// A `File` source's PNG to upload (D18).
    pub(super) upload: Option<Vec<u8>>,
    made: Instant,
}

impl Candidate {
    pub(super) fn new(
        wallet: WalletId,
        url: Option<String>,
        sha: [u8; 32],
        dhash: [u8; 8],
        upload: Option<Vec<u8>>,
    ) -> Self {
        Self {
            wallet,
            url,
            sha,
            dhash,
            upload,
            made: Instant::now(),
        }
    }
}

/// What `update_profile` publishes for a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Published {
    pub(super) url: String,
    pub(super) sha: [u8; 32],
    pub(super) dhash: [u8; 8],
}

/// The avatar state of a session's Platform runtime.
pub(crate) struct AvatarRuntime {
    pipeline: Mutex<Option<Arc<Pipeline>>>,
    pictures: Arc<Pictures>,
    candidates: Mutex<VecDeque<(String, Candidate)>>,
    remote: Mutex<HashMap<Identifier, (Instant, RemoteRead)>>,
    /// One Platform read per unknown identity at a time, and at most
    /// [`REMOTE_READS`] at once (D35).
    remote_locks: Locks<Identifier>,
    remote_reads: Semaphore,
}

impl Default for AvatarRuntime {
    fn default() -> Self {
        Self {
            pipeline: Mutex::default(),
            pictures: Arc::new(Pictures::new(true)),
            candidates: Mutex::default(),
            remote: Mutex::default(),
            remote_locks: Mutex::default(),
            remote_reads: Semaphore::new(REMOTE_READS),
        }
    }
}

impl AvatarRuntime {
    /// Installs the session's pipeline (at open; tests replace it).
    pub(super) fn install(&self, pipeline: Pipeline) {
        *guard(&self.pipeline) = Some(Arc::new(pipeline));
    }

    /// The toggle the pipeline reads, to build one.
    pub(super) fn toggle(&self) -> Arc<Pictures> {
        Arc::clone(&self.pictures)
    }

    fn pipeline(&self) -> Result<Arc<Pipeline>, AvatarError> {
        (guard(&self.pipeline).clone()).ok_or(AvatarError::Platform(PlatformError::Unavailable))
    }

    pub(super) fn pictures(&self) -> bool {
        self.pictures.on()
    }

    /// Keeps `candidate` and returns its id, dropping expired ones and the
    /// wallet's oldest past [`CANDIDATES`] (another wallet's stay).
    pub(super) fn insert(&self, candidate: Candidate) -> String {
        let id = format!("av-{:016x}", rand::random::<u64>());
        let mut all = guard(&self.candidates);
        all.retain(|(_, c)| c.made.elapsed() < CANDIDATE_LIFE);
        let wallet = candidate.wallet;
        while all.iter().filter(|(_, c)| c.wallet == wallet).count() >= CANDIDATES {
            let oldest = all.iter().position(|(_, c)| c.wallet == wallet);
            oldest.map(|i| all.remove(i));
        }
        all.push_back((id.clone(), candidate));
        id
    }

    /// `f` of this wallet's live candidate `id`.
    fn with_candidate<T>(
        &self,
        wallet: WalletId,
        id: &str,
        f: impl FnOnce(&Candidate) -> T,
    ) -> Result<T, PlatformError> {
        let all = guard(&self.candidates);
        (all.iter())
            .find(|(i, c)| i == id && c.wallet == wallet && c.made.elapsed() < CANDIDATE_LIFE)
            .map(|(_, c)| f(c))
            .ok_or_else(|| PlatformError::InvalidArgument {
                detail: "unknown avatar candidate".into(),
            })
    }

    /// The fields `update_profile` sets for the candidate `id`: one of
    /// this wallet's, with a URL.
    pub(super) fn published(&self, wallet: WalletId, id: &str) -> Result<Published, PlatformError> {
        let published = self.with_candidate(wallet, id, |c| {
            c.url.clone().map(|url| Published {
                url,
                sha: c.sha,
                dhash: c.dhash,
            })
        })?;
        published.ok_or_else(|| PlatformError::InvalidArgument {
            detail: "the avatar candidate has no URL; upload it first".into(),
        })
    }

    /// `upload_avatar` (D2): no upload host until B1.
    pub(super) fn upload(&self, wallet: WalletId, id: &str) -> Result<String, AvatarError> {
        if !self.with_candidate(wallet, id, |c| c.upload.is_some())? {
            return Err(PlatformError::InvalidArgument {
                detail: "the avatar candidate already has a URL".into(),
            }
            .into());
        }
        Err(AvatarError::UploadUnconfigured)
    }

    /// `prepare_avatar(src)` for `wallet`.
    pub(super) async fn prepare(
        &self,
        wallet: WalletId,
        src: AvatarSource,
    ) -> Result<AvatarCandidate, AvatarError> {
        let pipeline = self.pipeline()?;
        let (url, upload, ingested) = match src {
            AvatarSource::Url { url } => {
                let (url, ingested) = pipeline.prepare_url(&url).await?;
                (Some(url), None, ingested)
            }
            AvatarSource::Gravatar { email } => {
                let url = gravatar_url(&Zeroizing::new(email))?;
                let (url, ingested) = pipeline.prepare_url(&url).await?;
                (Some(url), None, ingested)
            }
            AvatarSource::File { bytes, crop } => {
                if bytes.len() > MAX_BYTES {
                    return Err(AvatarError::TooLarge);
                }
                let (upload, ingested) = (pipeline)
                    .decoding(move || avatar_image::ingest_file(&bytes, crop))
                    .await?;
                (None, Some(upload), ingested)
            }
        };
        let preview = AvatarImage {
            png: ingested.large.clone(),
            size: AvatarSize::Large,
        };
        let needs_upload = upload.is_some();
        let id = self.insert(Candidate::new(
            wallet,
            url.clone(),
            ingested.sha,
            ingested.dhash.ok_or(AvatarError::Unsupported)?,
            upload,
        ));
        Ok(AvatarCandidate {
            id,
            preview,
            url,
            needs_upload,
        })
    }

    /// The profile of an identity the wallet holds nothing of, from
    /// Platform, kept [`REMOTE_PROFILE_LIFE`] (D14). Reads of one identity
    /// share one Platform read, its failure too (kept
    /// [`REMOTE_FAILURE_LIFE`]), and at most [`REMOTE_READS`] run at once
    /// (D35). None starts or completes once `ticket` no longer allows the
    /// network (D33): then `None`, and nothing is kept.
    async fn remote_profile(
        &self,
        net: &impl RequestNet,
        user: Identifier,
        ticket: Ticket,
    ) -> RemoteRead {
        if let Some(kept) = self.kept_remote(&user) {
            return kept;
        }
        let lock = keyed_lock(&self.remote_locks, &user);
        let pictures = &self.pictures;
        let Some(_one) = pictures.unless_revoked(ticket, lock.lock()).await else {
            return Ok(None);
        };
        if let Some(kept) = self.kept_remote(&user) {
            return kept;
        }
        let Some(_permit) = pictures
            .unless_revoked(ticket, self.remote_reads.acquire())
            .await
        else {
            return Ok(None);
        };
        pictures.at("platform").await;
        let Some(doc) = pictures
            .unless_revoked(ticket, net.profile_document(user))
            .await
        else {
            return Ok(None);
        };
        let read = doc
            .map(|doc| doc.and_then(|d| super::profiles::shown(&d.properties)))
            .map_err(AvatarError::from);
        let mut remote = guard(&self.remote);
        remote.retain(|_, (at, kept)| at.elapsed() < remote_life(kept));
        if remote.len() >= REMOTE_PROFILES
            && let Some(oldest) = (remote.iter())
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| *k)
        {
            remote.remove(&oldest);
        }
        remote.insert(user, (Instant::now(), read.clone()));
        read
    }

    fn kept_remote(&self, user: &Identifier) -> Option<RemoteRead> {
        (guard(&self.remote).get(user))
            .filter(|(at, kept)| at.elapsed() < remote_life(kept))
            .map(|(_, kept)| kept.clone())
    }
}

/// Gravatar's URL for `email` as Android builds it (D13): the address
/// trimmed and lower-cased, its MD5 in hex. The address is never kept.
pub(super) fn gravatar_url(email: &str) -> Result<String, AvatarError> {
    use md5::Digest as _;
    let email = Zeroizing::new(email.trim().to_lowercase());
    let plausible = email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if !plausible {
        return Err(PlatformError::InvalidArgument {
            detail: "not an e-mail address".into(),
        }
        .into());
    }
    let digest = md5::Md5::digest(email.as_bytes());
    Ok(format!(
        "https://www.gravatar.com/avatar/{}?s=200&d=404&rating=g",
        hex::encode(digest)
    ))
}

impl NetworkSession {
    /// At open: the toggle from app.sqlite, the cache directory opened once
    /// (when `avatars_dir()` exists), and the pipeline installed.
    pub(crate) async fn open_avatars(&self, db: Arc<AppDb>) {
        let dir = self.avatars_dir();
        let blocking_db = Arc::clone(&db);
        let opened = tokio::task::spawn_blocking(move || {
            // Unreadable, the toggle is off for the session: a user who
            // turned it off must not see fetches.
            let pictures = match blocking_db.setting(dw_appdb::GLOBAL_SCOPE, LOAD_PICTURES_KEY) {
                Ok(v) => v.as_deref() != Some("0"),
                Err(e) => {
                    tracing::warn!(error = %e, "could not read the contact-pictures setting; it is off");
                    false
                }
            };
            let dir = dir.and_then(|path| {
                ThumbDir::open(&path)
                    .inspect_err(|e| {
                        tracing::warn!(error = %e, "avatar cache directory refused; thumbnails are not kept")
                    })
                    .ok()
            });
            (pictures, dir)
        })
        .await;
        let (pictures, dir) = opened.unwrap_or((false, None));
        let avatars = &self.platform.avatars;
        avatars.pictures.set(pictures);
        avatars.install(Pipeline::new(
            FetchPolicy::STRICT,
            Arc::new(SystemResolver),
            dir,
            db,
            avatars.toggle(),
        ));
    }

    /// `DashPay::avatar` (design: "The read").
    pub(super) async fn avatar_of(
        self: &Arc<Self>,
        net: &impl RequestNet,
        wallet_id: WalletId,
        identity: &str,
        size: AvatarSize,
    ) -> Result<Option<AvatarImage>, AvatarError> {
        let user = identifier(identity).map_err(|_| PlatformError::InvalidArgument {
            detail: "not a Base58 identity id".into(),
        })?;
        let avatars = &self.platform.avatars;
        // The toggle as this read began: checked again at every network
        // step and before answering (D33).
        let ticket = avatars.pictures.ticket();
        let read = self.identity_read(wallet_id).map_err(PlatformError::from)?;
        let b58 = user.to_string(Encoding::Base58);
        let (profile, whose) = match read.summaries.iter().find(|s| s.identity == b58) {
            Some(own) => (own.profile.clone(), Whose::Own),
            None if !avatars.pictures.allows(ticket) => return Ok(None),
            None => {
                let mut owners: Vec<_> = read.summaries.iter().collect();
                owners.sort_by_key(|s| !s.is_main);
                let owners: Vec<Identifier> = (owners.into_iter())
                    .filter_map(|s| Identifier::from_string(&s.identity, Encoding::Base58).ok())
                    .collect();
                let known = self
                    .known_profile(wallet_id, &user, &owners)
                    .map_err(PlatformError::from)?;
                let profile = match known {
                    Some(profile) => profile,
                    None => avatars.remote_profile(net, user, ticket).await?,
                };
                (profile, Whose::Other)
            }
        };
        let Some(profile) = profile else {
            return Ok(None);
        };
        let png = avatars
            .pipeline()?
            .for_profile(&profile, size, whose, ticket)
            .await?;
        Ok(png.map(|png| AvatarImage { png, size }))
    }
}
