use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const T0: u64 = 1_793_835_763;

type Fut<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;

/// Answers every request with `answer` (after `gate`, when set), counting
/// the requests and the URLs asked for.
struct Feed {
    answer: Result<Vec<u8>, String>,
    gate: Option<Arc<Notify>>,
    started: Notify,
    calls: AtomicUsize,
    urls: Mutex<Vec<String>>,
}

impl Feed {
    fn new(answer: Result<&str, &str>) -> Arc<Self> {
        Arc::new(Self {
            answer: answer.map(|b| b.as_bytes().to_vec()).map_err(str::to_owned),
            gate: None,
            started: Notify::new(),
            calls: AtomicUsize::new(0),
            urls: Mutex::new(Vec::new()),
        })
    }

    fn gated(answer: &str, gate: Arc<Notify>) -> Arc<Self> {
        let mut feed = Arc::into_inner(Self::new(Ok(answer))).unwrap();
        feed.gate = Some(gate);
        Arc::new(feed)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HttpGet for Feed {
    fn get<'a>(&'a self, url: &'a str) -> Fut<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.urls.lock().unwrap().push(url.to_owned());
        Box::pin(async move {
            self.started.notify_one();
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            self.answer.clone()
        })
    }
}

fn hub(dir: &Path, feed: &Arc<Feed>) -> Arc<UpdatesHub> {
    let http: Arc<dyn HttpGet> = feed.clone();
    UpdatesHub::new(dir, Some(http), "0.2.0", Arc::new(|| T0))
}

const NEWER: &str =
    r#"{"version":"0.3.0","pub_date":"2026-11-04T23:42:43Z","notes_url":"https://evil.example/x"}"#;

#[test]
fn release_versions_are_plain_x_y_z() {
    assert_eq!(release_version("0.2.0"), Some((0, 2, 0)));
    assert_eq!(release_version("v1.10.3"), Some((1, 10, 3)));
    for not in [
        "0.2.0-rc.1",
        "0.2.0-dev.3+abc1234",
        "0.2.0+abc",
        "0.2",
        "0.2.0.1",
        "",
        "v",
        "+1.2.3",
        "1..3",
        " 1.2.3",
        "1.2.3 ",
        "1234567890.0.0",
    ] {
        assert_eq!(release_version(not), None, "{not:?}");
    }
}

#[test]
fn a_newer_full_release_is_available_on_a_fixed_url() {
    let got = parse_feed(NEWER.as_bytes(), "0.2.0").unwrap().unwrap();
    assert_eq!(
        got,
        Release {
            version: "0.3.0".into(),
            published_at: Some(1_793_835_763),
            notes_url: "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/tag/v0.3.0"
                .into(),
        }
    );
    let minor = parse_feed(br#"{"version":"v0.10.0"}"#, "0.9.9")
        .unwrap()
        .unwrap();
    assert_eq!(
        (minor.version.as_str(), minor.published_at),
        ("0.10.0", None)
    );
    let odd_date = parse_feed(br#"{"version":"1.0.0","pub_date":"yesterday"}"#, "0.2.0");
    assert_eq!(odd_date.unwrap().unwrap().published_at, None);
}

#[test]
fn the_same_an_older_or_a_test_release_is_not_an_update() {
    for v in [
        "0.2.0",
        "0.1.9",
        "0.0.1",
        "0.3.0-rc.1",
        "0.3.0-dev.4+abc",
        "v0.3.0-rc.2",
    ] {
        let body = format!(r#"{{"version":"{v}"}}"#);
        assert_eq!(parse_feed(body.as_bytes(), "0.2.0"), Ok(None), "{v}");
    }
}

#[test]
fn a_body_that_is_not_a_feed_is_an_error() {
    for body in [&b""[..], b"<html>", b"{}", br#"{"version":3}"#, b"[]"] {
        assert!(parse_feed(body, "0.2.0").is_err(), "{body:?}");
    }
    for v in [
        "latest",
        "",
        "0.3",
        "1.2.3.4",
        "0.3.0-",
        "0.3.0+abc",
        "x.3.0-rc.1",
        "-rc.1",
    ] {
        let body = format!(r#"{{"version":"{v}"}}"#);
        assert!(parse_feed(body.as_bytes(), "0.2.0").is_err(), "{v:?}");
    }
}

#[test]
fn a_release_candidate_is_offered_its_final_and_a_dev_build_never_its_own() {
    let offered = |feed: &str, build: &str| {
        let body = format!(r#"{{"version":"{feed}"}}"#);
        parse_feed(body.as_bytes(), build)
            .unwrap()
            .map(|r| r.version)
    };
    assert_eq!(offered("0.2.0", "0.2.0-rc.1"), Some("0.2.0".into()));
    assert_eq!(offered("0.3.0", "0.2.0-rc.1"), Some("0.3.0".into()));
    assert_eq!(offered("0.1.9", "0.2.0-rc.1"), None);
    assert_eq!(offered("0.2.0", "0.2.0-dev"), None);
    assert_eq!(offered("0.2.0", "0.2.0-dev.57.g1a2b3c4"), None);
    assert_eq!(offered("0.3.0", "0.2.0-dev"), Some("0.3.0".into()));
    assert_eq!(offered("0.1.0", "0.2.0-dev"), None);
    assert_eq!(offered("0.2.0", "0.2.0"), None);
    assert!(parse_feed(br#"{"version":"0.2.0"}"#, "0.2.0rc1").is_err());
}

#[test]
fn only_github_release_hosts_over_https_are_pinned() {
    let ok = |u: &str| pinned(&url::Url::parse(u).unwrap());
    assert!(ok(FEED_URL));
    assert!(ok(
        "https://objects.githubusercontent.com/github-production-release-asset/1"
    ));
    assert!(ok("https://release-assets.githubusercontent.com/x"));
    for bad in [
        "http://github.com/x",
        "https://github.com:8443/x",
        "https://evil.example/x",
        "https://github.com.evil.example/x",
        "https://api.github.com/x",
        "https://raw.githubusercontent.com/x",
    ] {
        assert!(!ok(bad), "{bad}");
    }
}

#[test]
fn the_production_client_builds() {
    assert!(client().is_ok());
}

#[tokio::test]
async fn off_by_default_makes_no_request() {
    let dir = dw_testutil::private_tempdir();
    let feed = Feed::new(Ok(NEWER));
    let hub = hub(dir.path(), &feed);
    assert_eq!(
        hub.view(),
        UpdatesView {
            enabled: false,
            state: UpdateState::Off,
            checked_at: None
        }
    );
    assert_eq!(hub.check().await, Err(UpdatesError::Off));
    assert_eq!(feed.calls(), 0);
    assert!(!dir.path().join(UPDATES_FILE).exists());
}

#[tokio::test]
async fn on_checks_the_feed_and_remembers_the_setting() {
    let dir = dw_testutil::private_tempdir();
    let feed = Feed::new(Ok(NEWER));
    let hub = hub(dir.path(), &feed);
    assert_eq!(
        hub.set_enabled(true).unwrap().state,
        UpdateState::NotChecked
    );
    assert_eq!(feed.calls(), 0, "turning it on makes no request by itself");
    let view = hub.check().await.unwrap();
    assert!(matches!(&view.state, UpdateState::Available(r) if r.version == "0.3.0"));
    assert_eq!(view.checked_at, Some(T0));
    assert_eq!(*feed.urls.lock().unwrap(), [FEED_URL]);

    let again = self::hub(dir.path(), &Feed::new(Ok(NEWER)));
    assert!(again.view().enabled);
    assert_eq!(again.view().state, UpdateState::NotChecked);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(UPDATES_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    hub.set_enabled(false).unwrap();
    assert_eq!(self::hub(dir.path(), &feed).view().state, UpdateState::Off);
}

#[tokio::test]
async fn current_and_failed_checks() {
    let dir = dw_testutil::private_tempdir();
    let current = Feed::new(Ok(r#"{"version":"0.2.0"}"#));
    let hub = hub(dir.path(), &current);
    hub.set_enabled(true).unwrap();
    assert_eq!(hub.check().await.unwrap().state, UpdateState::Current);

    for answer in [Err("github.com: HTTP 404"), Ok("not json")] {
        let feed = Feed::new(answer);
        let hub = self::hub(dir.path(), &feed);
        assert_eq!(hub.check().await.unwrap().state, UpdateState::Failed);
        assert_eq!(feed.calls(), 1);
    }
}

#[tokio::test]
async fn turning_it_off_aborts_the_check_in_flight_and_keeps_nothing() {
    let dir = dw_testutil::private_tempdir();
    let gate = Arc::new(Notify::new());
    let feed = Feed::gated(NEWER, Arc::clone(&gate));
    let hub = hub(dir.path(), &feed);
    hub.set_enabled(true).unwrap();
    let running = tokio::spawn({
        let hub = Arc::clone(&hub);
        async move { hub.check().await }
    });
    feed.started.notified().await;
    assert_eq!(hub.set_enabled(false).unwrap().state, UpdateState::Off);
    assert_eq!(running.await.unwrap(), Err(UpdatesError::Off));
    gate.notify_waiters();
    assert_eq!(hub.view().state, UpdateState::Off);
    assert_eq!(hub.view().checked_at, None);

    // On again, the old answer is not there; a new check is needed.
    assert_eq!(
        hub.set_enabled(true).unwrap().state,
        UpdateState::NotChecked
    );
}

#[tokio::test]
async fn off_forgets_the_last_result() {
    let dir = dw_testutil::private_tempdir();
    let feed = Feed::new(Ok(NEWER));
    let hub = hub(dir.path(), &feed);
    hub.set_enabled(true).unwrap();
    hub.check().await.unwrap();
    hub.set_enabled(false).unwrap();
    let view = hub.set_enabled(true).unwrap();
    assert_eq!(
        (view.state, view.checked_at),
        (UpdateState::NotChecked, None)
    );
}

#[tokio::test]
async fn on_is_refused_when_the_setting_cannot_be_saved() {
    let dir = dw_testutil::private_tempdir();
    let feed = Feed::new(Ok(NEWER));
    let hub = UpdatesHub::new(
        &dir.path().join("missing"),
        Some(feed.clone() as _),
        "0.2.0",
        Arc::new(|| T0),
    );
    assert!(matches!(
        hub.set_enabled(true),
        Err(UpdatesError::Storage(_))
    ));
    assert_eq!(hub.view().state, UpdateState::Off);
    assert_eq!(hub.check().await, Err(UpdatesError::Off));
    assert_eq!(feed.calls(), 0);
}
