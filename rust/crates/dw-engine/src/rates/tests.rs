//! The rates service against a scripted source and clock: no test here
//! touches the network.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::decimal::Price;
use super::service::{RATES_FILE, RatesConfig, RatesHub};
use super::sources::{HttpGet, RateSource, SourceRates, SourceRow};
use super::state::{EXPIRE_SECS, FRESH_SECS, Stored};
use super::*;
use crate::events::{EngineEvent, EventSink};

const T0: u64 = 1_793_835_763;

#[derive(Default)]
struct Events(Mutex<Vec<EngineEvent>>);

impl EventSink for Events {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Events {
    fn rates(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == EngineEvent::Rates)
            .count()
    }
}

/// Counts every request; the sources below never call it.
#[derive(Default)]
struct CountingHttp(AtomicUsize);

impl HttpGet for CountingHttp {
    fn get<'a>(
        &'a self,
        _url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + 'a>>
    {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err("no network in tests".into()) })
    }
}

type Answer = Result<Vec<(&'static str, &'static str, u64)>, &'static str>;

/// A source that answers from a script, one answer per fetch (the last
/// repeated), and counts its calls.
struct Scripted {
    id: RateSourceId,
    answers: Mutex<VecDeque<Answer>>,
    calls: AtomicUsize,
}

impl Scripted {
    fn new(id: RateSourceId, answers: Vec<Answer>) -> Arc<Self> {
        Arc::new(Self {
            id,
            answers: Mutex::new(answers.into()),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl RateSource for Scripted {
    fn id(&self) -> RateSourceId {
        self.id
    }

    fn fetch<'a>(
        &'a self,
        http: &'a dyn HttpGet,
        _now: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SourceRates, String>> + Send + 'a>>
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Each source makes one request, as a real one would.
        drop(http.get("https://example.invalid/"));
        let answer = {
            let mut a = self.answers.lock().unwrap();
            if a.len() > 1 {
                a.pop_front().unwrap()
            } else {
                a.front().cloned().unwrap()
            }
        };
        let id = self.id;
        Box::pin(async move {
            let rows = answer.map_err(str::to_owned)?;
            Ok(SourceRates {
                source: id,
                rows: rows
                    .into_iter()
                    .map(|(c, p, at)| SourceRow {
                        currency: c.into(),
                        price: Price::parse(p).unwrap(),
                        retrieved_at: at,
                    })
                    .collect(),
            })
        })
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    hub: Arc<RatesHub>,
    clock: Arc<AtomicU64>,
    http: Arc<CountingHttp>,
    events: Arc<Events>,
    primary: Arc<Scripted>,
    fallback: Arc<Scripted>,
}

impl Rig {
    fn new(primary: Vec<Answer>, fallback: Vec<Answer>) -> Rig {
        Self::with(primary, fallback, |c| c)
    }

    fn with(
        primary: Vec<Answer>,
        fallback: Vec<Answer>,
        tune: impl FnOnce(RatesConfig) -> RatesConfig,
    ) -> Rig {
        let dir = dw_testutil::private_tempdir();
        let root = dir.path().to_path_buf();
        Self::at(dir, root, primary, fallback, tune)
    }

    fn at(
        dir: tempfile::TempDir,
        root: std::path::PathBuf,
        primary: Vec<Answer>,
        fallback: Vec<Answer>,
        tune: impl FnOnce(RatesConfig) -> RatesConfig,
    ) -> Rig {
        let clock = Arc::new(AtomicU64::new(T0));
        let http = Arc::new(CountingHttp::default());
        let events = Arc::new(Events::default());
        let primary = Scripted::new(RateSourceId::Ctx, primary);
        let fallback = Scripted::new(RateSourceId::BitPayDashCentral, fallback);
        let c = Arc::clone(&clock);
        let cfg = tune(RatesConfig {
            sources: vec![
                Arc::clone(&primary) as Arc<dyn RateSource>,
                Arc::clone(&fallback) as _,
            ],
            http: Some(Arc::clone(&http) as Arc<dyn HttpGet>),
            clock: Arc::new(move || c.load(Ordering::SeqCst)),
            ..RatesConfig::production()
        });
        let hub = RatesHub::new(&root, cfg, Arc::clone(&events) as Arc<dyn EventSink>);
        Rig {
            _dir: dir,
            root,
            hub,
            clock,
            http,
            events,
            primary,
            fallback,
        }
    }

    fn rates(&self) -> Rates {
        Rates {
            hub: Arc::clone(&self.hub),
        }
    }

    /// Prices on, disclosed, USD, a mainnet network open.
    fn on(&self) -> &Self {
        self.rates()
            .set_settings(settings(true, Some("USD"), true))
            .unwrap();
        self.hub.mainnet_opened();
        self
    }

    fn advance(&self, secs: u64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    fn requests(&self) -> usize {
        self.http.0.load(Ordering::SeqCst)
    }
}

fn settings(enabled: bool, currency: Option<&str>, disclosed: bool) -> RatesSettings {
    RatesSettings {
        enabled,
        currency: currency.map(str::to_owned),
        disclosed,
    }
}

fn usd(price: &'static str, at: u64) -> Answer {
    Ok(vec![("USD", price, at), ("EUR", "44", at)])
}

/// Polls `ready` until it returns `Ok`; panics after 10 s with `what` and
/// the last thing `ready` saw.
fn eventually(what: &str, mut ready: impl FnMut() -> Result<(), String>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match ready() {
            Ok(()) => return,
            Err(seen) => assert!(
                std::time::Instant::now() < deadline,
                "{what}; last seen: {seen}"
            ),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn a_fresh_price_shows_and_ages_to_stale_then_expires_with_no_fetch() {
    let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
    rig.on();
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Fresh);
    let rate = v.rate.unwrap();
    assert_eq!(
        (rate.currency.as_str(), rate.price.as_str(), rate.rate_at),
        ("USD", "49.0967", T0)
    );
    assert_eq!(rate.source, RateSourceId::Ctx);
    assert_eq!(v.last_success_at, Some(T0));

    rig.advance(FRESH_SECS);
    assert_eq!(
        rig.rates().status().state,
        RateState::Fresh,
        "30:00 is still fresh"
    );
    rig.advance(1);
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Stale);
    assert!(v.rate.is_some(), "a stale price is shown, with its warning");
    rig.advance(EXPIRE_SECS - FRESH_SECS - 1);
    assert_eq!(
        rig.rates().status().state,
        RateState::Stale,
        "24 h is still stale"
    );
    rig.advance(1);
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Unavailable(RatesUnavailable::Expired));
    assert_eq!(v.rate, None, "never a number past 24 h");
    assert_eq!(rig.primary.calls(), 1, "ageing makes no request");
}

#[test]
fn the_age_is_the_earlier_of_retrieval_and_fetch() {
    let rig = Rig::new(vec![usd("49", T0 - FRESH_SECS)], vec![Err("unused")]);
    rig.on();
    block_on(rig.hub.attempt());
    assert_eq!(rig.rates().status().state, RateState::Fresh);
    rig.advance(1);
    assert_eq!(
        rig.rates().status().state,
        RateState::Stale,
        "the provider took it 30 min ago"
    );
}

#[test]
fn a_failed_fetch_never_shows_a_number() {
    let rig = Rig::new(vec![Err("ctx down")], vec![Err("bitpay down")]);
    // Before any attempt.
    rig.on();
    let v = rig.rates().status();
    assert_eq!(
        v.state,
        RateState::Unavailable(RatesUnavailable::NeverFetched)
    );
    assert_eq!(v.rate, None);
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Unavailable(RatesUnavailable::Failed));
    assert_eq!(v.rate, None);
    assert!(v.last_attempt_failed);
    assert_eq!(
        (rig.primary.calls(), rig.fallback.calls()),
        (1, 1),
        "the fallback was asked too"
    );
    // Every state without a number has no rate, whatever is cached.
    for state in [
        settings(false, Some("USD"), true),
        settings(true, Some("USD"), false),
        settings(true, None, true),
        settings(true, Some("JPY"), true),
    ] {
        let v = rig.rates().set_settings(state).unwrap();
        assert!(
            matches!(v.state, RateState::Off(_) | RateState::Unavailable(_)),
            "{v:?}"
        );
        assert_eq!(v.rate, None, "{v:?}");
    }
}

#[test]
fn the_fallback_answers_when_ctx_fails_and_a_later_failure_keeps_the_cache() {
    let rig = Rig::new(
        vec![Err("ctx down")],
        vec![usd("49.5", T0), Err("bitpay down")],
    );
    rig.on();
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Fresh);
    assert_eq!(
        v.rate.as_ref().unwrap().source,
        RateSourceId::BitPayDashCentral
    );
    assert!(!v.last_attempt_failed);
    rig.advance(60);
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Fresh, "the cached price is still fresh");
    assert!(v.last_attempt_failed);
    assert_eq!(v.rate.unwrap().price, "49.5");
}

#[test]
fn a_missing_currency_is_unavailable_not_zero() {
    let rig = Rig::new(vec![usd("49", T0)], vec![Err("unused")]);
    rig.on();
    block_on(rig.hub.attempt());
    let v = rig
        .rates()
        .set_settings(settings(true, Some("JPY"), true))
        .unwrap();
    assert_eq!(
        v.state,
        RateState::Unavailable(RatesUnavailable::CurrencyMissing)
    );
    assert_eq!(v.rate, None);
    let v = rig
        .rates()
        .set_settings(settings(true, None, true))
        .unwrap();
    assert_eq!(
        v.state,
        RateState::Unavailable(RatesUnavailable::NoCurrency)
    );
    let codes: Vec<String> = rig
        .rates()
        .currencies()
        .into_iter()
        .map(|c| c.code)
        .collect();
    assert_eq!(codes, ["EUR", "USD"]);
}

#[test]
fn volatile_flags_a_move_of_more_than_half_either_way() {
    let case = |second: &'static str, gap: u64| {
        let rig = Rig::new(
            vec![usd("100", T0), usd(second, T0 + gap)],
            vec![Err("unused")],
        );
        rig.on();
        block_on(rig.hub.attempt());
        rig.advance(gap);
        block_on(rig.hub.attempt());
        let v = rig.rates().status();
        assert_eq!(v.rate.as_ref().map(|r| r.price.as_str()), Some(second));
        v.volatile
    };
    assert!(case("160", 300), "+60 %");
    assert!(case("40", 300), "−60 %: Android only flags rises");
    assert!(!case("149", 300), "+49 %");
    assert!(!case("51", 300), "−49 %");
    assert!(
        !case("160", 8 * 24 * 3600),
        "the previous price is over 7 days old"
    );
}

#[test]
fn volatile_is_never_set_on_a_stale_price() {
    let rig = Rig::new(
        vec![usd("100", T0), usd("160", T0 + 60)],
        vec![Err("unused")],
    );
    rig.on();
    block_on(rig.hub.attempt());
    rig.advance(60);
    block_on(rig.hub.attempt());
    assert!(rig.rates().status().volatile);
    rig.advance(FRESH_SECS + 1);
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Stale);
    assert!(!v.volatile);
}

#[test]
fn refuses_prices_from_the_future_older_than_cached_or_absurd() {
    let rig = Rig::new(
        vec![
            usd("50", T0),
            // From 6 minutes ahead.
            Ok(vec![("USD", "51", T0 + 60 + 360)]),
            // Older than the cached one.
            Ok(vec![("USD", "52", T0 - 1)]),
            // Over tenfold up, then over tenfold down.
            Ok(vec![("USD", "500.01", T0 + 60)]),
            Ok(vec![("USD", "4.99", T0 + 60)]),
            // Tenfold exactly, with EUR fine beside it.
            Ok(vec![("USD", "500", T0 + 60), ("EUR", "45", T0 + 60)]),
        ],
        vec![Err("unused")],
    );
    rig.on();
    block_on(rig.hub.attempt());
    // A price from the future or absurd is a failed attempt for its
    // currency; one older than the cached price is not, the cached being
    // the newer.
    for (refused, failed) in [(0, true), (1, false), (2, true), (3, true)] {
        rig.advance(15);
        block_on(rig.hub.attempt());
        let v = rig.rates().status();
        assert_eq!(v.rate.unwrap().price, "50", "attempt {refused}");
        assert_eq!(v.last_attempt_failed, failed, "attempt {refused}");
        assert_eq!(v.state, RateState::Fresh);
    }
    rig.advance(1);
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(v.rate.unwrap().price, "500");
    assert!(!v.last_attempt_failed);
    // Within the skew allowance, a provider time ahead is taken.
    let rig = Rig::new(vec![usd("50", T0 + 300)], vec![Err("unused")]);
    rig.on();
    block_on(rig.hub.attempt());
    assert_eq!(rig.rates().status().rate.unwrap().rate_at, T0);
}

#[test]
fn an_expired_cache_does_not_bound_the_next_price() {
    let rig = Rig::new(
        vec![usd("50", T0), usd("600", T0 + EXPIRE_SECS + 10)],
        vec![Err("unused")],
    );
    rig.on();
    block_on(rig.hub.attempt());
    rig.advance(EXPIRE_SECS + 10);
    block_on(rig.hub.attempt());
    assert_eq!(rig.rates().status().rate.unwrap().price, "600");
}

#[test]
fn off_undisclosed_or_not_mainnet_makes_no_request() {
    let rig = Rig::with(vec![usd("49", T0)], vec![usd("49", T0)], |c| RatesConfig {
        tick: Duration::from_millis(5),
        cadence: Duration::from_secs(1),
        ..c
    });
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let run = Arc::downgrade(&rig.hub);
    rt.spawn(super::service::run_for_tests(run));
    let wait = || std::thread::sleep(Duration::from_millis(80));
    let rates = rig.rates();
    // The defaults: on, but the disclosure was not shown.
    assert_eq!(rates.status().state, RateState::Off(RatesOff::NotDisclosed));
    rig.hub.mainnet_opened();
    wait();
    assert_eq!(rig.requests(), 0, "no request before the disclosure");
    rates
        .set_settings(settings(false, Some("USD"), true))
        .unwrap();
    wait();
    assert_eq!(rig.requests(), 0, "no request while off");
    assert_eq!(
        rt.block_on(rates.refresh()),
        Err(RatesError::Off(RatesOff::Disabled)),
        "Update now makes none either"
    );
    rig.hub.mainnet_closed();
    rates
        .set_settings(settings(true, Some("USD"), true))
        .unwrap();
    wait();
    assert_eq!(rig.requests(), 0, "no request without a mainnet network");
    assert_eq!(rates.status().state, RateState::Off(RatesOff::NotMainnet));
    assert_eq!(rates.currencies(), Vec::new());
    // Mainnet opens: the task asks at once.
    rig.hub.mainnet_opened();
    eventually("the first answer never showed", || {
        match rates.status().state {
            RateState::Fresh => Ok(()),
            state => Err(format!("{state:?} after {} request(s)", rig.requests())),
        }
    });
    assert_eq!(rig.requests(), 1);
    // Off again: nothing more.
    rates
        .set_settings(settings(false, Some("USD"), true))
        .unwrap();
    rig.advance(3600);
    wait();
    assert_eq!(rig.requests(), 1);
    drop(rt);
}

#[test]
fn refresh_is_refused_within_thirty_seconds_and_failures_back_off() {
    let rig = Rig::new(vec![Err("down")], vec![Err("down")]);
    rig.on();
    let rates = rig.rates();
    block_on(rates.refresh()).unwrap();
    rig.advance(29);
    assert_eq!(block_on(rates.refresh()), Err(RatesError::Throttled));
    rig.advance(1);
    block_on(rates.refresh()).unwrap();
    assert_eq!(rig.primary.calls(), 2);
    // Backoff after failures in a row: 1, 2, 4 minutes, then the cadence.
    let rig = Rig::new(vec![Err("down")], vec![Err("down")]);
    rig.on();
    let mut waits = Vec::new();
    for _ in 0..5 {
        block_on(rig.hub.attempt());
        waits.push(rig.hub.due_in_for_tests());
    }
    // Each ± 20 %.
    for (wait, base) in waits.iter().zip([60, 120, 240, 240, 240]) {
        assert!((base * 4 / 5..=base * 6 / 5).contains(wait), "{waits:?}");
    }
}

/// Samples `f` 24 times, a little apart, since the jitter's entropy is the
/// clock's sub-millisecond part.
fn samples(mut f: impl FnMut() -> u64) -> std::collections::BTreeSet<u64> {
    (0..24)
        .map(|_| {
            std::thread::sleep(Duration::from_micros(173));
            f()
        })
        .collect()
}

/// Review nit 2: the backoffs and the first attempt after a restart are
/// jittered too, so no fetch falls on a fixed mark.
#[test]
fn backoffs_and_the_first_wait_after_a_restart_are_jittered() {
    let rig = Rig::new(vec![Err("down")], vec![Err("down")]);
    rig.on();
    for _ in 0..3 {
        block_on(rig.hub.attempt());
    }
    let backoffs = samples(|| {
        block_on(rig.hub.attempt());
        rig.hub.due_in_for_tests()
    });
    assert!(
        backoffs.iter().all(|w| (192..=288).contains(w)),
        "{backoffs:?}"
    );
    assert!(backoffs.len() > 1, "never one fixed wait: {backoffs:?}");
    // After a restart: the cadence after the last attempt, ± 20 %.
    let firsts = samples(|| rig.hub.first_due_in_for_tests());
    assert!(firsts.iter().all(|w| (240..=360).contains(w)), "{firsts:?}");
    assert!(firsts.len() > 1, "never one fixed wait: {firsts:?}");
}

#[test]
fn a_success_waits_the_cadence_with_jitter() {
    let rig = Rig::new(vec![usd("49", T0)], vec![Err("unused")]);
    rig.on();
    block_on(rig.hub.attempt());
    let wait = rig.hub.due_in_for_tests();
    assert!((240..=360).contains(&wait), "{wait}");
}

#[test]
fn rates_json_is_private_and_survives_a_restart() {
    let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
    rig.on();
    block_on(rig.hub.attempt());
    let path = rig.root.join(RATES_FILE);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let text = std::fs::read_to_string(&path).unwrap();
    for absent in ["wallet", "address", "balance", "txid"] {
        assert!(!text.to_lowercase().contains(absent), "{absent}");
    }
    let Rig { _dir, root, .. } = rig;
    let again = Rig::at(_dir, root, vec![Err("down")], vec![Err("down")], |c| c);
    again.hub.mainnet_opened();
    let v = again.rates().status();
    assert_eq!(v.settings, settings(true, Some("USD"), true));
    assert_eq!(v.rate.unwrap().price, "49.0967");
}

#[test]
fn a_corrupt_rates_json_reads_as_nothing_fetched() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join(RATES_FILE), "{not json").unwrap();
    let rig = Rig::at(
        dir,
        root.clone(),
        vec![Err("down")],
        vec![Err("down")],
        |c| c,
    );
    assert_eq!(rig.rates().status().settings, RatesSettings::default());
    assert_eq!(
        std::fs::read_to_string(root.join(RATES_FILE)).unwrap(),
        "{not json",
        "left alone until a change is written"
    );
    // A price that is not a plain decimal makes the file unreadable too.
    let mut bad = Stored::default();
    bad.rates.insert(
        "USD".into(),
        super::state::Cached {
            price: "1e3".into(),
            retrieved_at: T0,
            fetched_at: T0,
            source: RateSourceId::Ctx,
            prev: None,
        },
    );
    assert_eq!(
        Stored::from_json(&serde_json::to_string(&bad).unwrap()),
        Stored::default()
    );
}

#[test]
fn invalid_currency_codes_are_refused() {
    let rig = Rig::new(vec![Err("down")], vec![Err("down")]);
    for bad in ["usd", "US", "USDT", "", "U$D"] {
        assert_eq!(
            rig.rates().set_settings(settings(true, Some(bad), true)),
            Err(RatesError::InvalidCurrency(bad.into()))
        );
    }
    assert_eq!(RatesError::Throttled.code(), "rates.throttled");
}

#[test]
fn every_change_of_the_view_is_announced_once() {
    let rig = Rig::new(vec![usd("49", T0)], vec![Err("unused")]);
    let base = rig.events.rates();
    rig.on();
    let after_on = rig.events.rates();
    assert!(after_on > base);
    rig.rates()
        .set_settings(settings(true, Some("USD"), true))
        .unwrap();
    assert_eq!(rig.events.rates(), after_on, "no change, no event");
    block_on(rig.hub.attempt());
    assert_eq!(rig.events.rates(), after_on + 1);
}

/// A source that answers USD only once released, and says whether its
/// fetch was dropped before it answered (the request aborted).
struct Gated {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    aborted: std::sync::atomic::AtomicBool,
}

impl Gated {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            aborted: std::sync::atomic::AtomicBool::new(false),
        })
    }
}

struct AbortFlag<'a>(&'a std::sync::atomic::AtomicBool, bool);

impl Drop for AbortFlag<'_> {
    fn drop(&mut self) {
        if !self.1 {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

impl RateSource for Gated {
    fn id(&self) -> RateSourceId {
        RateSourceId::Ctx
    }

    fn fetch<'a>(
        &'a self,
        _http: &'a dyn HttpGet,
        _now: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SourceRates, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut flag = AbortFlag(&self.aborted, false);
            self.started.notify_one();
            self.release.notified().await;
            flag.1 = true;
            Ok(SourceRates {
                source: RateSourceId::Ctx,
                rows: vec![SourceRow {
                    currency: "USD".into(),
                    price: Price::parse("49.0967").unwrap(),
                    retrieved_at: T0,
                }],
            })
        })
    }
}

/// A rig whose only source is `gate`.
fn gated_rig(gate: &Arc<Gated>) -> Rig {
    let g = Arc::clone(gate);
    Rig::with(Vec::new(), Vec::new(), move |c| RatesConfig {
        sources: vec![g as Arc<dyn RateSource>],
        ..c
    })
}

fn off(rig: &Rig) {
    rig.rates()
        .set_settings(settings(false, Some("USD"), true))
        .unwrap();
}

/// A Mainnet session's stamper reading `rig`'s prices (rates-engine §6).
fn stamper(rig: &Rig) -> crate::events::SessionHub {
    let hub = crate::events::SessionHub::new(crate::DashNetwork::Mainnet, Arc::new(NoSink));
    hub.set_rates(Arc::downgrade(&rig.hub));
    hub
}

struct NoSink;

impl EventSink for NoSink {
    fn emit(&self, _: EngineEvent) {}
}

fn stored_file(rig: &Rig) -> Stored {
    Stored::from_json(&std::fs::read_to_string(rig.root.join(RATES_FILE)).unwrap())
}

/// `rates.json`, or `None` when there is none.
fn stored_file_if_any(rig: &Rig) -> Option<Stored> {
    match std::fs::read_to_string(rig.root.join(RATES_FILE)) {
        Ok(text) => Some(Stored::from_json(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("{RATES_FILE}: {e}"),
    }
}

/// Amendment 4 (i): Off while a fetch is out stops it, and nothing it
/// would have brought is stored; the view stays off.
#[test]
fn off_during_a_fetch_aborts_it_and_keeps_nothing() {
    let gate = Gated::new();
    let rig = gated_rig(&gate);
    let stamps = stamper(&rig);
    rig.on();
    block_on(async {
        let hub = Arc::clone(&rig.hub);
        let task = tokio::spawn(async move { hub.attempt().await });
        gate.started.notified().await;
        off(&rig);
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the attempt stops without its answer")
            .unwrap();
        // Were the request still out, this is the answer it would get.
        gate.release.notify_waiters();
        tokio::task::yield_now().await;
    });
    assert!(
        gate.aborted.load(Ordering::SeqCst),
        "the request was dropped"
    );
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Off(RatesOff::Disabled));
    assert_eq!(v.rate, None);
    assert!(stored_file(&rig).rates.is_empty());
    assert_eq!(rig.hub.view().last_attempt_at, None, "no attempt recorded");
    assert!(stamps.stamp_rate().is_none(), "prices off stamp nothing");
    // On again: the dropped answer was never kept, so there is nothing to stamp with.
    rig.on();
    assert_eq!(
        rig.rates().status().state,
        RateState::Unavailable(RatesUnavailable::NeverFetched)
    );
    assert!(stamps.stamp_rate().is_none());
}

/// A source that turns prices off and on again in the poll that answers:
/// its result lands after an Off, with prices on again.
#[derive(Default)]
struct FlipsOff(std::sync::OnceLock<std::sync::Weak<RatesHub>>);

impl RateSource for FlipsOff {
    fn id(&self) -> RateSourceId {
        RateSourceId::Ctx
    }

    fn fetch<'a>(
        &'a self,
        _http: &'a dyn HttpGet,
        _now: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SourceRates, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let hub = self.0.get().and_then(std::sync::Weak::upgrade).unwrap();
            hub.set_settings(settings(false, Some("USD"), true))
                .unwrap();
            hub.set_settings(settings(true, Some("USD"), true)).unwrap();
            Ok(SourceRates {
                source: RateSourceId::Ctx,
                rows: vec![SourceRow {
                    currency: "USD".into(),
                    price: Price::parse("49.0967").unwrap(),
                    retrieved_at: T0,
                }],
            })
        })
    }
}

/// Amendment 4: a result that lands after Off is never stored, even with
/// prices on again by then.
#[test]
fn a_result_landing_after_off_then_on_is_discarded() {
    let source = Arc::new(FlipsOff::default());
    let s = Arc::clone(&source);
    let rig = Rig::with(Vec::new(), Vec::new(), move |c| RatesConfig {
        sources: vec![s as Arc<dyn RateSource>],
        ..c
    });
    source.0.set(Arc::downgrade(&rig.hub)).unwrap();
    let stamps = stamper(&rig);
    rig.on();
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(
        v.state,
        RateState::Unavailable(RatesUnavailable::NeverFetched)
    );
    assert_eq!(v.rate, None);
    assert!(stored_file(&rig).rates.is_empty());
    assert!(
        stamps.stamp_rate().is_none(),
        "a discarded result never reaches the stamper"
    );
}

/// Amendment 4 (ii) and (iii): Off clears every cached price and attempt,
/// in memory and in rates.json; on again, nothing shows until a new fetch.
#[test]
fn off_clears_the_cache_and_on_starts_from_nothing() {
    let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
    let stamps = stamper(&rig);
    rig.on();
    block_on(rig.hub.attempt());
    assert_eq!(rig.rates().status().state, RateState::Fresh);
    assert!(!stored_file(&rig).rates.is_empty());
    assert!(stamps.stamp_rate().is_some());

    off(&rig);
    assert!(stamps.stamp_rate().is_none(), "Off stops new stamps");
    let file = stored_file(&rig);
    assert!(file.rates.is_empty(), "rates.json holds no rate rows");
    assert_eq!(file.last_attempt_at, None);
    assert_eq!(
        file.settings,
        settings(false, Some("USD"), true),
        "the settings stay"
    );

    rig.on();
    let v = rig.rates().status();
    assert_eq!(
        v.state,
        RateState::Unavailable(RatesUnavailable::NeverFetched)
    );
    assert_eq!(
        (v.rate, v.last_success_at, v.last_attempt_at),
        (None, None, None)
    );
    assert!(
        rig.rates().currencies().is_empty(),
        "no stale number from before"
    );
    assert!(
        stamps.stamp_rate().is_none(),
        "nothing to stamp with until a new fetch"
    );
    // A restart reads the same nothing.
    let again = RatesHub::new(
        &rig.root,
        RatesConfig::production(),
        Arc::new(Events::default()),
    );
    again.mainnet_opened();
    assert_eq!(
        again.view().state,
        RateState::Unavailable(RatesUnavailable::NeverFetched)
    );
    drop(again);
    // A new fetch stamps again.
    block_on(rig.hub.attempt());
    assert_eq!(rig.rates().status().state, RateState::Fresh);
    assert!(stamps.stamp_rate().is_some());
}

/// Amendment 2: while CTX answers, the fallback is never asked.
#[test]
fn the_fallback_is_asked_only_after_ctx_fails() {
    let rig = Rig::new(vec![usd("49.0967", T0)], vec![usd("1", T0)]);
    rig.on();
    for _ in 0..3 {
        block_on(rig.hub.attempt());
        rig.advance(60);
    }
    assert_eq!((rig.primary.calls(), rig.fallback.calls()), (3, 0));
    assert_eq!(rig.requests(), 3, "one request per attempt, CTX's");
    assert_eq!(rig.rates().status().rate.unwrap().source, RateSourceId::Ctx);
}

/// Review m1: Off takes effect in memory even when rates.json cannot be
/// written. The fetch in flight stops, the cache is cleared and nothing
/// more is stamped; the storage error comes back after, and the file is
/// removed, so a restart reads prices as never disclosed.
#[test]
fn off_takes_effect_when_rates_json_cannot_be_written() {
    let gate = Gated::new();
    let rig = gated_rig(&gate);
    let stamps = stamper(&rig);
    rig.on();
    block_on(async {
        let hub = Arc::clone(&rig.hub);
        let task = tokio::spawn(async move { hub.attempt().await });
        gate.started.notified().await;
        gate.release.notify_waiters();
        task.await.unwrap();
    });
    assert_eq!(rig.rates().status().state, RateState::Fresh);
    let check = stamps.stamp_check();
    assert!(stamps.stamp_rate().is_some());
    // The temporary file's place taken: no write can succeed.
    std::fs::create_dir(rig.root.join("rates.json.tmp")).unwrap();
    let res = block_on(async {
        let hub = Arc::clone(&rig.hub);
        let task = tokio::spawn(async move { hub.attempt().await });
        gate.started.notified().await;
        let res = rig.rates().set_settings(settings(false, Some("USD"), true));
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the attempt stops without its answer")
            .unwrap();
        res
    });
    assert_eq!(res.map_err(|e| e.code()), Err("rates.storage"));
    assert!(
        gate.aborted.load(Ordering::SeqCst),
        "the request was dropped"
    );
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Off(RatesOff::Disabled));
    assert_eq!((v.rate, v.last_attempt_at), (None, None));
    assert!(rig.rates().currencies().is_empty());
    assert!(stamps.stamp_rate().is_none(), "no stamps after Off");
    assert!(!check(), "stamps read before Off are not written");
    assert!(!rig.root.join(RATES_FILE).exists());
    let again = RatesHub::new(
        &rig.root,
        RatesConfig::production(),
        Arc::new(Events::default()),
    );
    again.mainnet_opened();
    let v = again.view();
    assert_eq!(v.state, RateState::Off(RatesOff::NotDisclosed));
    assert_eq!(v.rate, None);
}

/// Review round 2, m1: when neither the Off's write nor the removal of
/// the old file succeeds, the failure is logged and the Off is saved on a
/// later pass of the task, once the folder can be written again.
#[cfg(unix)]
#[test]
fn an_off_that_can_be_neither_saved_nor_removed_is_saved_on_a_later_tick() {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::with(vec![usd("49.0967", T0)], vec![Err("unused")], |c| {
        RatesConfig {
            tick: Duration::from_millis(5),
            ..c
        }
    });
    rig.on();
    block_on(rig.hub.attempt());
    assert!(stored_file(&rig).settings.enabled);
    let mode =
        |m: u32| std::fs::set_permissions(&rig.root, std::fs::Permissions::from_mode(m)).unwrap();
    // Read-only: no temporary file, no rename, no removal.
    mode(0o500);
    if std::fs::write(rig.root.join("probe"), b"").is_ok() {
        mode(0o700);
        eprintln!("skipped: the folder stays writable (run as root)");
        return;
    }
    let res = rig.rates().set_settings(settings(false, Some("USD"), true));
    assert_eq!(res.map_err(|e| e.code()), Err("rates.storage"));
    assert_eq!(
        rig.rates().status().state,
        RateState::Off(RatesOff::Disabled)
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.spawn(super::service::run_for_tests(Arc::downgrade(&rig.hub)));
    std::thread::sleep(Duration::from_millis(100));
    // The gap while the folder stays read-only: the old file is still there.
    assert!(
        stored_file(&rig).settings.enabled,
        "not saved while read-only"
    );
    mode(0o700);
    // Saved either way: written with prices off, or the file removed.
    eventually("the Off was never saved", || {
        match stored_file_if_any(&rig) {
            Some(file) if file.settings.enabled => {
                Err(format!("{RATES_FILE} holds {:?}", file.settings))
            }
            _ => Ok(()),
        }
    });
    if let Some(file) = stored_file_if_any(&rig) {
        assert!(file.rates.is_empty(), "no cached price");
        assert_eq!(file.settings, settings(false, Some("USD"), true));
    }
    assert_eq!(rig.requests(), 1, "no request since the Off");
    let again = RatesHub::new(
        &rig.root,
        RatesConfig::production(),
        Arc::new(Events::default()),
    );
    again.mainnet_opened();
    let v = again.view();
    assert!(matches!(v.state, RateState::Off(_)), "{v:?}");
    assert_eq!(v.rate, None);
}

/// Review nit 5: after the fallback answered, a CTX row with an earlier
/// provider time is refused, the fallback's being newer, but the attempt
/// did not fail.
#[test]
fn a_ctx_row_older_than_the_fallbacks_is_no_failure() {
    let rig = Rig::new(
        vec![Err("down"), usd("49.5", T0 + 30)],
        vec![usd("49", T0 + 60)],
    );
    rig.on();
    block_on(rig.hub.attempt());
    assert_eq!(
        rig.rates().status().rate.unwrap().source,
        RateSourceId::BitPayDashCentral
    );
    rig.advance(90);
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(rig.primary.calls(), 2);
    assert_eq!(v.rate.unwrap().price, "49", "the newer price is kept");
    assert!(!v.last_attempt_failed, "CTX answered: no failure");
    assert_eq!(v.state, RateState::Fresh);
}

/// Review round 2, m2: a price taken while the clock was a day ahead
/// (the fallback's rows carry the fetch time) counts as absent once the
/// clock is set back: no number and nothing to stamp with, and CTX's
/// next price is taken rather than refused as older.
#[test]
fn a_price_dated_ahead_of_a_corrected_clock_counts_as_absent() {
    let day = 24 * 60 * 60;
    let rig = Rig::new(vec![Err("down"), usd("50", T0)], vec![usd("49", T0 + day)]);
    let stamps = stamper(&rig);
    rig.advance(day);
    rig.on();
    block_on(rig.hub.attempt());
    assert_eq!(rig.rates().status().rate.unwrap().price, "49");
    rig.clock.store(T0, Ordering::SeqCst);
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Unavailable(RatesUnavailable::Failed));
    assert_eq!(v.rate, None, "never a frozen price that reads fresh");
    assert!(rig.rates().currencies().is_empty());
    assert!(stamps.stamp_rate().is_none());
    block_on(rig.hub.attempt());
    let v = rig.rates().status();
    assert_eq!(v.state, RateState::Fresh);
    let rate = v.rate.unwrap();
    assert_eq!(
        (rate.price.as_str(), rate.source),
        ("50", RateSourceId::Ctx)
    );
    assert!(!v.last_attempt_failed);
    assert!(!v.volatile, "nothing to compare with");
}

/// rates-engine §6: a Mainnet session stamps a transaction with the fresh
/// price only, and only one whose time (first sight or block) is at most 3 h ago.
pub(crate) mod stamps {
    use dashcore::hashes::Hash;
    use dashcore::{BlockHash, OutPoint, Transaction, TxIn, Txid};
    use key_wallet::account::{AccountType, StandardAccountType};
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};

    use super::*;
    use crate::events::SessionHub;
    use crate::{DashNetwork, WalletId};

    const W: WalletId = WalletId([7; 32]);
    const HOURS_3: u64 = 3 * 3600;

    fn add(hub: &SessionHub, n: u8, context: TransactionContext) -> Txid {
        add_seen(hub, n, context, Some(T0))
    }

    fn add_seen(hub: &SessionHub, n: u8, context: TransactionContext, seen: Option<u64>) -> Txid {
        add_seen_to(hub, W, n, context, seen)
    }

    /// A transaction of `wallet` in `hub`'s history, first seen at `seen`.
    pub(crate) fn add_seen_to(
        hub: &SessionHub,
        wallet: WalletId,
        n: u8,
        context: TransactionContext,
        seen: Option<u64>,
    ) -> Txid {
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let r = TransactionRecord::new(
            tx,
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            context,
            TransactionType::Standard,
            TransactionDirection::Incoming,
            Vec::new(),
            Vec::new(),
            1_000,
        );
        hub.history
            .with_wallet(wallet, |h| assert!(h.upsert(&r, seen)));
        r.txid
    }

    fn block_at(time: u64) -> TransactionContext {
        TransactionContext::InBlock(BlockInfo::new(1, BlockHash::all_zeros(), time as u32))
    }

    #[test]
    fn a_fresh_price_stamps_new_and_recent_transactions_only() {
        let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
        rig.on();
        block_on(rig.hub.attempt());
        let hub = stamper(&rig);
        let now = T0;
        let mempool = add(&hub, 1, TransactionContext::Mempool);
        let recent = add(&hub, 2, block_at(now - HOURS_3));
        let old = add(&hub, 3, block_at(now - HOURS_3 - 1));
        // Seen over 3 h ago while no price was fresh, mined just now: too late to stamp.
        let late = add_seen(&hub, 4, block_at(now), Some(now - HOURS_3 - 1));
        let late_mempool = add_seen(
            &hub,
            5,
            TransactionContext::Mempool,
            Some(now - HOURS_3 - 1),
        );
        let unseen = add_seen(&hub, 6, TransactionContext::Mempool, None);
        let unknown = Txid::from_byte_array([9; 32]);

        let rate = hub.stamp_rate().expect("a fresh price stamps");
        let all = [mempool, recent, old, late, late_mempool, unseen, unknown];
        let rows = hub.price_stamps(&rate, &W, &all, now);
        let stamped: Vec<&str> = rows.iter().map(|r| r.txid.as_str()).collect();
        assert_eq!(
            stamped,
            [mempool.to_string(), recent.to_string(), unseen.to_string()]
        );
        let r = &rows[0];
        assert_eq!(
            (
                r.currency.as_str(),
                r.price.as_str(),
                r.rate_at,
                r.source.as_str()
            ),
            ("USD", "49.0967", T0, "ctx")
        );
    }

    #[test]
    fn a_stale_price_or_another_network_never_stamps() {
        let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
        rig.on();
        block_on(rig.hub.attempt());
        let testnet = SessionHub::new(DashNetwork::Testnet, Arc::new(NoSink));
        testnet.set_rates(Arc::downgrade(&rig.hub));
        assert!(testnet.stamp_rate().is_none(), "test coins have no price");
        assert!(
            SessionHub::new(DashNetwork::Mainnet, Arc::new(NoSink))
                .stamp_rate()
                .is_none(),
            "no prices attached"
        );

        let hub = stamper(&rig);
        assert!(hub.stamp_rate().is_some());
        rig.advance(FRESH_SECS + 1);
        assert_eq!(rig.rates().status().state, RateState::Stale);
        assert!(
            hub.stamp_rate().is_none(),
            "a stale price is shown, never stamped"
        );
    }

    #[test]
    fn prices_off_never_stamp() {
        let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
        rig.on();
        block_on(rig.hub.attempt());
        let hub = stamper(&rig);
        rig.rates()
            .set_settings(settings(false, Some("USD"), true))
            .unwrap();
        assert!(hub.stamp_rate().is_none());
    }

    /// Review nit 1: the stamp write re-checks, so an Off between reading
    /// the rate and writing the stamps writes none.
    #[test]
    fn an_off_between_reading_the_rate_and_writing_stamps_writes_none() {
        let rig = Rig::new(vec![usd("49.0967", T0)], vec![Err("unused")]);
        rig.on();
        block_on(rig.hub.attempt());
        let hub = stamper(&rig);
        let check = hub.stamp_check();
        assert!(hub.stamp_rate().is_some());
        assert!(check(), "nothing changed: the stamps are written");
        rig.rates()
            .set_settings(settings(false, Some("USD"), true))
            .unwrap();
        assert!(!check(), "turned off since the rate was read");
        rig.on();
        block_on(rig.hub.attempt());
        assert!(!check(), "on again since: the rate read then is not used");
        assert!(hub.stamp_check()(), "a rate read now is");
        let detached = SessionHub::new(DashNetwork::Mainnet, Arc::new(NoSink));
        assert!(!detached.stamp_check()(), "no prices attached");
    }

    #[test]
    fn stored_source_names_round_trip() {
        for id in [RateSourceId::Ctx, RateSourceId::BitPayDashCentral] {
            assert_eq!(RateSourceId::from_stored(id.stored()), Some(id));
        }
        assert_eq!(
            RateSourceId::stored(RateSourceId::BitPayDashCentral),
            "bitPayDashCentral"
        );
        assert_eq!(RateSourceId::from_stored("coingecko"), None);
    }
}
