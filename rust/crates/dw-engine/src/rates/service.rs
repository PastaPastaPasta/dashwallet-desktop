//! The rates service: one per [`crate::Engine`], beside its sessions,
//! since prices belong to no network. A task fetches every 5 minutes while
//! prices are on, the disclosure was shown and a mainnet network is open;
//! otherwise it makes no request. A minute's tick re-reads the state, so a
//! price turns stale or expires with no fetch. Every change of the view is
//! announced with [`EngineEvent::Rates`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::Notify;

use super::RatesError;
use super::sources::{HttpGet, RateSource, ReqwestGet};
use super::state::{Stored, is_code};
use super::types::{CurrencyRate, RateState, RatesSettings, RatesView};
use crate::events::{EngineEvent, EventSink};

/// `<data_root>/rates.json`.
pub(crate) const RATES_FILE: &str = "rates.json";

/// The schedule (production values in [`RatesConfig::production`]).
#[derive(Clone)]
pub(crate) struct RatesConfig {
    pub cadence: Duration,
    /// A fraction of `cadence` each wait is moved by at random, up or down.
    pub jitter: f64,
    /// Waits after a failed attempt, the last repeated (capped by `cadence`).
    pub backoff: Vec<Duration>,
    /// How often the state is re-read without a fetch.
    pub tick: Duration,
    /// `refresh` is refused this soon after an attempt.
    pub refresh_min: Duration,
    pub sources: Vec<Arc<dyn RateSource>>,
    pub http: Option<Arc<dyn HttpGet>>,
    /// UNIX seconds.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl RatesConfig {
    pub(crate) fn production() -> Self {
        Self {
            cadence: Duration::from_secs(5 * 60),
            jitter: 0.2,
            backoff: [60, 120, 240].map(Duration::from_secs).to_vec(),
            tick: Duration::from_secs(60),
            refresh_min: Duration::from_secs(30),
            sources: super::sources::production_sources(),
            http: None,
            clock: Arc::new(unix_now),
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

struct Inner {
    stored: Stored,
    /// Open mainnet sessions (0 or 1 per engine).
    mainnet: usize,
    /// Failed attempts in a row.
    failures: usize,
    /// When the next fetch is due (UNIX seconds); `None`: as soon as
    /// fetching is allowed.
    next_due: Option<u64>,
    /// The view last announced.
    announced: Option<RatesView>,
    /// Raised each time prices are turned off: an attempt started before
    /// keeps nothing it brings, even if prices are on again when it lands.
    off_epoch: u64,
}

pub(crate) struct RatesHub {
    path: PathBuf,
    cfg: RatesConfig,
    http: Option<Arc<dyn HttpGet>>,
    sink: Arc<dyn EventSink>,
    inner: Mutex<Inner>,
    /// One attempt at a time.
    attempt: tokio::sync::Mutex<()>,
    wake: Arc<Notify>,
    /// Notified when prices are turned off: the attempt in flight stops.
    off: Notify,
}

impl RatesHub {
    /// Reads `<data_root>/rates.json` (a missing or unreadable file is
    /// "nothing fetched yet") and starts the task on `handle`.
    pub(crate) fn start(
        data_root: &Path,
        cfg: RatesConfig,
        sink: Arc<dyn EventSink>,
        handle: &Handle,
    ) -> Arc<Self> {
        let hub = Self::new(data_root, cfg, sink);
        let task = Arc::downgrade(&hub);
        handle.spawn(async move { run(task).await });
        hub
    }

    pub(crate) fn new(data_root: &Path, cfg: RatesConfig, sink: Arc<dyn EventSink>) -> Arc<Self> {
        let path = data_root.join(RATES_FILE);
        let stored = std::fs::read_to_string(&path)
            .map(|t| Stored::from_json(&t))
            .unwrap_or_default();
        let http = cfg.http.clone().or_else(|| {
            ReqwestGet::new()
                .inspect_err(|e| tracing::warn!(error = %e, "rates HTTP client unavailable"))
                .ok()
                .map(|c| Arc::new(c) as Arc<dyn HttpGet>)
        });
        Arc::new(Self {
            path,
            cfg,
            http,
            sink,
            inner: Mutex::new(Inner {
                stored,
                mainnet: 0,
                failures: 0,
                next_due: None,
                announced: None,
                off_epoch: 0,
            }),
            attempt: tokio::sync::Mutex::new(()),
            wake: Arc::new(Notify::new()),
            off: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now(&self) -> u64 {
        (self.cfg.clock)()
    }

    pub(crate) fn mainnet_opened(&self) {
        self.lock().mainnet += 1;
        self.changed();
    }

    pub(crate) fn mainnet_closed(&self) {
        {
            let mut inner = self.lock();
            inner.mainnet = inner.mainnet.saturating_sub(1);
        }
        self.changed();
    }

    pub(crate) fn view(&self) -> RatesView {
        let inner = self.lock();
        inner.stored.view(self.now(), inner.mainnet > 0)
    }

    /// The price a transaction first seen now is stamped with: the chosen
    /// currency's, only while it is fresh (rates-engine §6).
    pub(crate) fn fresh_rate(&self) -> Option<super::types::FiatRate> {
        let view = self.view();
        match view.state {
            RateState::Fresh => view.rate,
            _ => None,
        }
    }

    /// Counts the times prices were turned off: read before a rate is
    /// taken for stamping, and given back to [`Self::stamps_still_allowed`].
    pub(crate) fn off_epoch(&self) -> u64 {
        self.lock().off_epoch
    }

    /// Stamps read under `epoch` may still be written: prices are on and
    /// allowed, and were not turned off since (rates-engine §6).
    pub(crate) fn stamps_still_allowed(&self, epoch: u64) -> bool {
        let inner = self.lock();
        inner.off_epoch == epoch
            && !matches!(
                inner.stored.view(self.now(), inner.mainnet > 0).state,
                RateState::Off(_)
            )
    }

    pub(crate) fn currencies(&self) -> Vec<CurrencyRate> {
        let inner = self.lock();
        let view = inner.stored.view(self.now(), inner.mainnet > 0);
        if matches!(view.state, RateState::Off(_)) {
            return Vec::new();
        }
        inner.stored.currencies(self.now())
    }

    /// Stores the settings. Turning prices off stops the attempt in
    /// flight, so nothing it brings is kept, and clears every cached price
    /// and attempt, here and in `rates.json` (rates-engine §1); on again,
    /// the state is `unavailable(neverFetched)` until a new fetch. The
    /// transactions' price stamps (§6) are history and stay.
    ///
    /// Off takes effect even when `rates.json` cannot be written: the file
    /// is then removed, so a restart cannot bring the old prices back (it
    /// reads as never disclosed), and the storage error is returned.
    pub(crate) fn set_settings(&self, settings: RatesSettings) -> Result<RatesView, RatesError> {
        if let Some(c) = settings.currency.as_deref()
            && !is_code(c)
        {
            return Err(RatesError::InvalidCurrency(c.to_owned()));
        }
        let off = !settings.enabled;
        let saved = {
            let mut inner = self.lock();
            if off {
                inner.stored = Stored {
                    settings,
                    ..Stored::default()
                };
                inner.off_epoch += 1;
                inner.failures = 0;
                inner.next_due = None;
                let saved = self.write(&inner.stored);
                if saved.is_err() {
                    let _ = std::fs::remove_file(&self.path);
                }
                saved
            } else {
                let next = Stored {
                    settings,
                    ..inner.stored.clone()
                };
                self.write(&next)?;
                inner.stored = next;
                Ok(())
            }
        };
        if off {
            self.off.notify_waiters();
        }
        self.changed();
        saved?;
        Ok(self.view())
    }

    /// An attempt now, unless prices are off or one was made within
    /// `refresh_min`.
    pub(crate) async fn refresh(&self) -> Result<RatesView, RatesError> {
        let view = self.view();
        if let RateState::Off(why) = view.state {
            return Err(RatesError::Off(why));
        }
        let now = self.now();
        if view
            .last_attempt_at
            .is_some_and(|at| now.saturating_sub(at) < self.cfg.refresh_min.as_secs())
        {
            return Err(RatesError::Throttled);
        }
        self.attempt().await;
        Ok(self.view())
    }

    /// Whether a request may be made now.
    fn may_fetch(&self) -> bool {
        !matches!(self.view().state, RateState::Off(_))
    }

    /// Asks the sources in order until one answers, then stores what it
    /// brought. Makes no request when prices are off.
    /// Turning prices off stops it: the request is dropped, and nothing is
    /// stored.
    pub(crate) async fn attempt(&self) {
        let _one = self.attempt.lock().await;
        // Listening before the epoch is read: an Off after this is heard.
        let off = self.off.notified();
        tokio::pin!(off);
        off.as_mut().enable();
        let epoch = self.lock().off_epoch;
        if !self.may_fetch() {
            return;
        }
        let Some(http) = self.http.clone() else {
            self.finish(None, epoch);
            return;
        };
        let fetch = async {
            for source in &self.cfg.sources {
                match source.fetch(http.as_ref(), self.now()).await {
                    Ok(rates) => return Some(rates),
                    Err(e) => {
                        tracing::info!(source = ?source.id(), error = %e, "rates source failed");
                    }
                }
            }
            None
        };
        // An Off already heard wins over an answer in the same poll.
        let got = tokio::select! {
            biased;
            () = &mut off => return,
            got = fetch => got,
        };
        // Not allowed any more (not disclosed, no mainnet): nothing it brought is kept.
        if !self.may_fetch() {
            return;
        }
        self.finish(got, epoch);
    }

    /// Stores what the attempt started at `epoch` brought, unless prices
    /// were turned off since.
    fn finish(&self, got: Option<super::sources::SourceRates>, epoch: u64) {
        let now = self.now();
        {
            let mut inner = self.lock();
            if inner.off_epoch != epoch {
                return;
            }
            let mut next = inner.stored.clone();
            let ok = got.is_some();
            match got {
                Some(rates) => {
                    for (code, why) in next.take(rates, now) {
                        tracing::info!(%code, ?why, "rates: refused a new price");
                    }
                }
                None => next.note_failure(now),
            }
            if let Err(e) = self.write(&next) {
                tracing::warn!(error = %e, "rates: cannot save");
            }
            inner.stored = next;
            inner.failures = if ok { 0 } else { inner.failures + 1 };
            // Every wait is jittered, so no fetch falls on a fixed mark.
            let wait = if ok {
                self.cfg.cadence
            } else {
                let i = (inner.failures - 1).min(self.cfg.backoff.len().saturating_sub(1));
                self.cfg
                    .backoff
                    .get(i)
                    .copied()
                    .unwrap_or(self.cfg.cadence)
                    .min(self.cfg.cadence)
            };
            let wait = jittered(wait, self.cfg.jitter);
            inner.next_due = Some(now + wait.as_secs().max(1));
        }
        self.changed();
    }

    fn write(&self, stored: &Stored) -> Result<(), RatesError> {
        let json =
            serde_json::to_vec_pretty(stored).map_err(|e| RatesError::Storage(e.to_string()))?;
        write_private(&self.path, &json)
            .map_err(|e| RatesError::Storage(format!("{RATES_FILE}: {e}")))
    }

    /// Wakes the task and announces the view when it differs from the one
    /// last announced.
    fn changed(&self) {
        self.wake.notify_one();
        self.announce();
    }

    /// Announces the view when it differs from the one last announced.
    fn announce(&self) {
        let view = self.view();
        let mut inner = self.lock();
        if inner.announced.as_ref() != Some(&view) {
            inner.announced = Some(view);
            drop(inner);
            self.sink.emit(EngineEvent::Rates);
        }
    }

    /// How long the task sleeps before it looks again.
    fn next_wait(&self) -> (Duration, bool) {
        let now = self.now();
        if !self.may_fetch() {
            return (self.cfg.tick, false);
        }
        let mut inner = self.lock();
        // After a restart the last attempt's time sets the next one.
        let first = self.first_due(&inner.stored, now);
        let due = *inner.next_due.get_or_insert(first);
        if due <= now {
            return (Duration::ZERO, true);
        }
        (Duration::from_secs(due - now).min(self.cfg.tick), false)
    }
}

async fn run(hub: std::sync::Weak<RatesHub>) {
    loop {
        let Some(h) = hub.upgrade() else { return };
        let (wait, due) = h.next_wait();
        if due {
            h.attempt().await;
            continue;
        }
        let wake = Arc::clone(&h.wake);
        drop(h);
        let woke = tokio::select! {
            () = tokio::time::sleep(wait) => false,
            () = wake.notified() => true,
        };
        let Some(h) = hub.upgrade() else { return };
        if !woke {
            // The tick: a price may have turned stale or expired.
            h.announce();
        }
    }
}

impl RatesHub {
    /// When the first attempt of this run is due: now, or after a restart
    /// a jittered cadence after the last attempt.
    fn first_due(&self, stored: &Stored, now: u64) -> u64 {
        stored.last_attempt_at.map_or(now, |at| {
            at + jittered(self.cfg.cadence, self.cfg.jitter).as_secs()
        })
    }
}

fn jittered(base: Duration, jitter: f64) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let unit = f64::from(nanos % 1_000_000) / 1_000_000.0; // 0..1
    let factor = 1.0 + jitter * (2.0 * unit - 1.0);
    base.mul_f64(factor.max(0.0))
}

/// Replaces `path` with `bytes` through a temporary file created 0600.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    // `create_new` (O_EXCL) never follows a symlink planted in its place.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
pub(super) async fn run_for_tests(hub: std::sync::Weak<RatesHub>) {
    run(hub).await;
}

#[cfg(test)]
impl RatesHub {
    /// Seconds until the next fetch is due.
    pub(super) fn due_in_for_tests(&self) -> u64 {
        let now = self.now();
        self.lock().next_due.map_or(0, |d| d.saturating_sub(now))
    }

    /// Seconds from now to the first attempt of a run started now.
    pub(super) fn first_due_in_for_tests(&self) -> u64 {
        let now = self.now();
        let stored = self.lock().stored.clone();
        self.first_due(&stored, now).saturating_sub(now)
    }
}
