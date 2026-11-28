use std::io::{Read as _, Write as _};
use std::sync::atomic::AtomicUsize;

use base64::Engine as _;
use serde_json::json;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;

use super::*;

fn conf(value: &serde_json::Value) -> tauri::Config {
    serde_json::from_value(value.clone()).unwrap()
}

fn shipped() -> serde_json::Value {
    serde_json::from_str(include_str!("../tauri.conf.json")).unwrap()
}

fn overlay() -> serde_json::Value {
    serde_json::from_str(include_str!("../tauri.updater.conf.json")).unwrap()
}

/// JSON merge patch (RFC 7396), as `tauri build --config` merges a file.
fn merge(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// The shipped config as a build with the signing key has it: the overlay
/// merged, and `pubkey` in place of the release key.
fn keyed(pubkey: &str) -> serde_json::Value {
    let mut merged = shipped();
    merge(&mut merged, &overlay());
    merged["plugins"]["updater"]["pubkey"] = json!(pubkey);
    merged
}

fn installed(bundle: Option<BundleType>) -> Install {
    Install {
        flatpak: false,
        bundle,
        appimage: true,
        read_only: false,
    }
}

const EVERY_BUNDLE: [Option<BundleType>; 8] = [
    None,
    Some(BundleType::Deb),
    Some(BundleType::Rpm),
    Some(BundleType::AppImage),
    Some(BundleType::Msi),
    Some(BundleType::Nsis),
    Some(BundleType::App),
    Some(BundleType::Dmg),
];

/// A throwaway key pair, made for this run and never written anywhere.
struct TestKey(minisign::KeyPair);

impl TestKey {
    fn new() -> Self {
        TestKey(minisign::KeyPair::generate_unencrypted_keypair().unwrap())
    }

    /// `plugins.updater.pubkey`: the public key file, base64, as
    /// `tauri signer generate` prints it.
    fn pubkey(&self) -> String {
        let text = self.0.pk.to_box().unwrap().to_string();
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    /// A feed entry's `signature`: the .sig file, base64, with the trusted
    /// comment the Tauri CLI writes (`version` `None`: an older CLI's, which
    /// names no version).
    fn sign_as(&self, bytes: &[u8], version: Option<&str>) -> String {
        let comment = match version {
            Some(v) => format!("timestamp:1790000000\tfile:payload\tversion:{v}"),
            None => "timestamp:1790000000\tfile:payload".into(),
        };
        let sig = minisign::sign(
            Some(&self.0.pk),
            &self.0.sk,
            std::io::Cursor::new(bytes),
            Some(&comment),
            None,
        )
        .unwrap();
        base64::engine::general_purpose::STANDARD.encode(sig.to_string())
    }

    fn sign(&self, bytes: &[u8], version: &str) -> String {
        self.sign_as(bytes, Some(version))
    }
}

/// The release public key, minisign key ID ED322B963B527B21.
const RELEASE_KEY_ID: &str = "ED322B963B527B21";
const RELEASE_KEY: &str = "RWQhe1I7lisy7VSCxCb7l68xDmzM20krIOD6L6lNybwEJAZuzX4aFEFS";

/// The shipped config carries the release public key but no feed: a build
/// without the signing key (main, pull requests, rc, dev) has no updater,
/// whatever the public key.
#[test]
fn the_shipped_build_has_no_updater() {
    let shipped = shipped();
    let pubkey = shipped["plugins"]["updater"]["pubkey"].as_str().unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(pubkey)
        .unwrap();
    let decoded = String::from_utf8(decoded).unwrap();
    assert_eq!(
        decoded,
        format!("untrusted comment: minisign public key: {RELEASE_KEY_ID}\n{RELEASE_KEY}\n")
    );
    let key = base64::engine::general_purpose::STANDARD
        .decode(RELEASE_KEY)
        .unwrap();
    assert_eq!((key.len(), &key[..2]), (42, &b"Ed"[..]));
    let id: String = key[2..10]
        .iter()
        .rev()
        .map(|b| format!("{b:02X}"))
        .collect();
    assert_eq!(id, RELEASE_KEY_ID);
    assert_eq!(shipped["plugins"]["updater"]["endpoints"], json!([]));
    assert!(shipped["bundle"].get("createUpdaterArtifacts").is_none());
    for pubkey in [json!(pubkey), json!(TestKey::new().pubkey())] {
        let mut c = shipped.clone();
        c["plugins"]["updater"]["pubkey"] = pubkey;
        let config = conf(&c);
        assert!(!build_has_updater(&config));
        for bundle in EVERY_BUNDLE {
            assert_eq!(
                availability(&config, &installed(bundle)),
                UpdateAvailability::NotInThisBuild
            );
        }
    }
}

#[test]
fn the_key_overlay_adds_the_stable_feed_and_the_artifacts() {
    let overlay = overlay();
    assert_eq!(overlay["bundle"]["createUpdaterArtifacts"], true);
    assert_eq!(
        overlay["plugins"]["updater"]["endpoints"],
        json!([FEED_URL])
    );
    assert!(overlay["plugins"]["updater"].get("pubkey").is_none());
    assert!(!build_has_updater(&conf(&keyed(""))));
    assert!(build_has_updater(&conf(&keyed(&TestKey::new().pubkey()))));
    let release_key = shipped()["plugins"]["updater"]["pubkey"].clone();
    assert!(build_has_updater(&conf(&keyed(
        release_key.as_str().unwrap()
    ))));
}

/// The flag the plugin itself reads from the shipped config with the
/// overlay: signatures must name the version the feed announces.
#[test]
fn a_keyed_build_requires_signed_versions() {
    let keyed = keyed(&TestKey::new().pubkey());
    let config: tauri_plugin_updater::Config =
        serde_json::from_value(keyed["plugins"]["updater"].clone()).unwrap();
    assert!(config.require_signed_version);
    assert!(!config.dangerous_insecure_transport_protocol);
    assert_eq!(config.endpoints.len(), 1);
    assert_eq!(config.endpoints[0].as_str(), FEED_URL);
}

#[test]
fn only_the_official_feed_with_signed_versions_counts() {
    let merged = keyed(&TestKey::new().pubkey());
    let with = |path: &str, value: serde_json::Value| {
        let mut c = merged.clone();
        c["plugins"]["updater"][path] = value;
        build_has_updater(&conf(&c))
    };
    assert!(!with(
        "endpoints",
        json!(["https://example.com/latest.json"])
    ));
    assert!(!with(
        "endpoints",
        json!([FEED_URL, "https://example.com/latest.json"])
    ));
    assert!(!with("requireSignedVersion", json!(false)));
    assert!(!with("pubkey", json!(" ")));
    let mut dropped = merged.clone();
    dropped["plugins"]["updater"]
        .as_object_mut()
        .unwrap()
        .remove("requireSignedVersion");
    assert!(!build_has_updater(&conf(&dropped)));
    let mut no_plugin = merged.clone();
    no_plugin["plugins"]
        .as_object_mut()
        .unwrap()
        .remove("updater");
    assert!(!build_has_updater(&conf(&no_plugin)));
}

#[test]
fn the_install_decides_in_a_build_with_the_updater() {
    let config = conf(&keyed(&TestKey::new().pubkey()));
    let of = |install: Install| availability(&config, &install);
    use UpdateAvailability as A;
    assert_eq!(of(installed(Some(BundleType::AppImage))), A::Available);
    assert_eq!(of(installed(Some(BundleType::Nsis))), A::Available);
    assert_eq!(of(installed(Some(BundleType::App))), A::Available);
    assert_eq!(of(installed(Some(BundleType::Deb))), A::PackageManager);
    assert_eq!(of(installed(Some(BundleType::Rpm))), A::PackageManager);
    assert_eq!(of(installed(None)), A::NotInstalled);
    assert_eq!(of(installed(Some(BundleType::Msi))), A::NotInstalled);
    assert_eq!(
        of(Install {
            appimage: false,
            ..installed(Some(BundleType::AppImage))
        }),
        A::NotInstalled
    );
    assert_eq!(
        of(Install {
            read_only: true,
            ..installed(Some(BundleType::App))
        }),
        A::MoveToApplications
    );
}

#[test]
fn a_mac_app_on_the_disk_image_or_translocated_cannot_replace_itself() {
    for exe in [
        "/Volumes/DashPay Desktop/DashPay Desktop.app/Contents/MacOS/dash-wallet",
        "/private/var/folders/x1/abc/T/AppTranslocation/0A1B2C/d/DashPay Desktop.app/Contents/MacOS/dash-wallet",
    ] {
        assert!(movable_only(Path::new(exe)), "{exe}");
    }
    for exe in [
        "/Applications/DashPay Desktop.app/Contents/MacOS/dash-wallet",
        "/Users/me/Applications/DashPay Desktop.app/Contents/MacOS/dash-wallet",
    ] {
        assert!(!movable_only(Path::new(exe)), "{exe}");
    }
}

#[test]
fn a_flatpak_never_updates_itself() {
    for config in [conf(&keyed(&TestKey::new().pubkey())), conf(&shipped())] {
        for bundle in EVERY_BUNDLE {
            let install = Install {
                flatpak: true,
                ..installed(bundle)
            };
            assert_eq!(availability(&config, &install), UpdateAvailability::Flatpak);
        }
    }
}

#[test]
fn only_newer_stable_releases_are_offered() {
    let v = |s: &str| Version::parse(s).unwrap();
    let current = v("0.6.0");
    assert!(is_offered(&current, &v("0.6.1")));
    assert!(is_offered(&current, &v("1.0.0")));
    assert!(!is_offered(&current, &v("0.6.0")));
    assert!(!is_offered(&current, &v("0.5.9")));
    assert!(!is_offered(&current, &v("0.7.0-rc.1")));
    assert!(!is_offered(&current, &v("0.7.0-dev.3")));
    assert!(!is_offered(&current, &v("0.7.0+build.1")));
    assert!(is_offered(&v("0.7.0-rc.1"), &v("0.7.0")));
}

/// The app with the updater plugin, configured as the shipped config with
/// the overlay, the test key in place of the release key.
fn app_with_updater(pubkey: &str) -> tauri::App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        keyed(pubkey)["plugins"]["updater"].clone(),
    );
    assert!(build_has_updater(context.config()));
    mock_builder().plugin(plugin()).build(context).expect("app")
}

/// A `latest.json` with one entry for this system, its package at `url`.
fn feed_at(version: &str, signature: &str, url: &str) -> serde_json::Value {
    let target = tauri_plugin_updater::target().expect("an updater target");
    json!({
        "version": version,
        "notes": "See the release page.",
        "pub_date": "2026-11-29T00:00:00Z",
        "platforms": { target: { "url": url, "signature": signature } }
    })
}

fn feed(version: &str, signature: &str) -> serde_json::Value {
    let url = format!(
        "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/v{version}/payload"
    );
    feed_at(version, signature, &url)
}

/// The feed entry as the plugin reads it, with no request made.
fn offered(
    app: &tauri::App<MockRuntime>,
    feed: serde_json::Value,
) -> Option<tauri_plugin_updater::Update> {
    app.updater_builder()
        .configure_client(|_| panic!("no request in tests"))
        .build()
        .unwrap()
        .restore_update(feed)
        .unwrap()
}

/// A loopback HTTP server that answers each path `files` gives (with the
/// server's base URL) with its body, and anything else with 404: the real
/// check and download, off the network.
fn serve(files: impl FnOnce(&str) -> Vec<(&'static str, Vec<u8>)>) -> String {
    serve_or("404 Not Found", files)
}

/// [`serve`], answering a path it does not have with `missing`.
fn serve_or(
    missing: &'static str,
    files: impl FnOnce(&str) -> Vec<(&'static str, Vec<u8>)>,
) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let files = files(&base);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let line = String::from_utf8_lossy(&request);
            let path = line.split_whitespace().nth(1).unwrap_or_default();
            let (status, body) = match files.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => ("200 OK", body.clone()),
                None => (missing, Vec::new()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    base
}

/// The app's check of a feed at `base`, as a keyed build makes it, then
/// the download of what it offers.
fn check_and_download(
    app: &tauri::App<MockRuntime>,
    base: &str,
) -> Result<Vec<u8>, tauri_plugin_updater::Error> {
    let endpoint = format!("{base}/latest.json").parse().unwrap();
    let update = run(app
        .updater_builder()
        .endpoints(vec![endpoint])?
        .no_proxy()
        .build()?
        .check())?
    .expect("the feed offers an update");
    run(update.download(|_, _| {}, || {}))
}

const PAYLOAD: &[u8] = b"the 0.2.0 AppImage";

#[test]
fn a_feed_entry_signed_by_the_release_key_verifies() {
    let key = TestKey::new();
    let app = app_with_updater(&key.pubkey());
    assert_eq!(app.package_info().version.to_string(), "0.1.0");
    let update =
        offered(&app, feed("0.2.0", &key.sign(PAYLOAD, "0.2.0"))).expect("0.2.0 is offered");
    assert_eq!(update.version, "0.2.0");
    update.verify(PAYLOAD).expect("the signature verifies");
}

#[test]
fn a_bad_signature_is_refused() {
    let key = TestKey::new();
    let app = app_with_updater(&key.pubkey());
    let code = |e| plugin_error(e, AppCode::UpdateDownload).code_str();
    // Other bytes than the ones signed.
    let update = offered(&app, feed("0.2.0", &key.sign(PAYLOAD, "0.2.0"))).unwrap();
    let e = update.verify(b"a tampered AppImage").unwrap_err();
    assert_eq!(code(e), "app.update_signature");
    // Signed by another key.
    let stranger = TestKey::new();
    let update = offered(&app, feed("0.2.0", &stranger.sign(PAYLOAD, "0.2.0"))).unwrap();
    let e = update.verify(PAYLOAD).unwrap_err();
    assert_eq!(code(e), "app.update_signature");
    // Not a signature at all.
    let update = offered(&app, feed("0.2.0", "bm90IGEgc2lnbmF0dXJl")).unwrap();
    let e = update.verify(PAYLOAD).unwrap_err();
    assert_eq!(code(e), "app.update_signature");
}

/// Through the app's own check and download, configured as shipped: a
/// signature of the announced version installs; an old signed build
/// relabelled as new, or a signature that names no version, does not.
#[test]
fn the_configured_check_binds_the_signed_version() {
    let key = TestKey::new();
    let app = app_with_updater(&key.pubkey());
    let at = |signature: String| {
        let base = serve(|base| {
            let latest = feed_at("0.2.0", &signature, &format!("{base}/payload"));
            vec![
                ("/latest.json", latest.to_string().into_bytes()),
                ("/payload", PAYLOAD.to_vec()),
            ]
        });
        check_and_download(&app, &base)
    };
    assert_eq!(at(key.sign(PAYLOAD, "0.2.0")).unwrap(), PAYLOAD);
    let relabelled = at(key.sign(PAYLOAD, "0.1.0")).unwrap_err();
    assert!(
        matches!(
            relabelled,
            tauri_plugin_updater::Error::SignedVersionMismatch { .. }
        ),
        "{relabelled}"
    );
    let unversioned = at(key.sign_as(PAYLOAD, None)).unwrap_err();
    assert!(
        matches!(
            unversioned,
            tauri_plugin_updater::Error::MissingSignedVersion
        ),
        "{unversioned}"
    );
    for e in [relabelled, unversioned] {
        assert_eq!(
            plugin_error(e, AppCode::UpdateInstall).code_str(),
            "app.update_signature"
        );
    }
}

/// A Latest release without `latest.json` answers 404: "not published".
/// Any other failed answer stays "could not reach the update server".
#[test]
fn only_a_feed_that_answers_404_is_not_published() {
    let app = app_with_updater(&TestKey::new().pubkey());
    for (missing, code) in [
        ("404 Not Found", "app.update_not_published"),
        ("500 Internal Server Error", "app.update_check"),
        ("403 Forbidden", "app.update_check"),
    ] {
        let url = format!("{}/latest.json", serve_or(missing, |_| vec![]));
        let updater = app
            .updater_builder()
            .endpoints(vec![url.parse().unwrap()])
            .unwrap()
            .no_proxy()
            .build()
            .unwrap();
        let Err(e) = run(check_feed(&updater, &url)) else {
            panic!("{missing}: the feed offered an update");
        };
        assert_eq!(e.code_str(), code, "{missing}");
    }
}

#[test]
fn a_downgrade_or_a_pre_release_is_not_offered() {
    let key = TestKey::new();
    let app = app_with_updater(&key.pubkey());
    for version in ["0.1.0", "0.0.9", "0.2.0-rc.1", "0.2.0-dev.7"] {
        let entry = feed(version, &key.sign(PAYLOAD, version));
        assert!(offered(&app, entry).is_none(), "{version} was offered");
    }
}

type Answer = Result<Option<Arc<dyn Release>>, AppError>;

/// A feed that answers from a script and counts what it is asked; with no
/// answer left it never answers.
#[derive(Default)]
struct FakeFeed {
    answers: Mutex<Vec<Answer>>,
    checks: AtomicUsize,
    restarts: Arc<AtomicUsize>,
    reveals: AtomicUsize,
}

impl Feed for Arc<FakeFeed> {
    fn check(&self) -> BoxFuture<'_, Answer> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let mut answers = self.answers.lock().unwrap();
        if answers.is_empty() {
            return Box::pin(std::future::pending());
        }
        let answer = answers.remove(0);
        Box::pin(async move { answer })
    }

    fn restart(&self) {
        self.restarts.fetch_add(1, Ordering::SeqCst);
    }

    fn reveal(&self) {
        self.reveals.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeRelease {
    version: &'static str,
    download: Option<AppCode>,
    stalls: bool,
    install: Option<AppCode>,
    installs: AtomicUsize,
    downloads: AtomicUsize,
    /// Runs once, while the download is under way.
    during_download: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl FakeRelease {
    fn new(version: &'static str) -> Arc<Self> {
        Arc::new(FakeRelease {
            version,
            ..Default::default()
        })
    }
}

impl Release for FakeRelease {
    fn version(&self) -> String {
        self.version.into()
    }

    fn download<'a>(&'a self, progress: Progress<'a>) -> BoxFuture<'a, Result<Vec<u8>, AppError>> {
        Box::pin(async move {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            progress(5, Some(10));
            if let Some(during) = self.during_download.lock().unwrap().take() {
                during();
            }
            if self.stalls {
                std::future::pending::<()>().await;
            }
            progress(10, Some(10));
            match self.download {
                None => Ok(PAYLOAD.to_vec()),
                Some(code) => Err(AppError::app(code, "refused")),
            }
        })
    }

    fn install(&self, bytes: &[u8]) -> Result<(), AppError> {
        assert_eq!(bytes, PAYLOAD);
        self.installs.fetch_add(1, Ordering::SeqCst);
        match self.install {
            None => Ok(()),
            Some(code) => Err(AppError::app(code, "the installer did not start")),
        }
    }
}

fn session(answers: Vec<Answer>) -> (Updates, Arc<FakeFeed>) {
    let fake = Arc::new(FakeFeed {
        answers: Mutex::new(answers),
        ..Default::default()
    });
    let updates = Updates::new(
        UpdateAvailability::Available,
        "0.1.0".into(),
        Some(Box::new(Arc::clone(&fake))),
    )
    .with_timeouts(Duration::from_millis(200), Duration::from_millis(200));
    (updates, fake)
}

/// Updates whose next check offers `release`.
fn offering_later(release: &Arc<FakeRelease>) -> (Updates, Arc<FakeFeed>) {
    session(vec![Ok(Some(release.clone() as Arc<dyn Release>))])
}

fn offering(release: &Arc<FakeRelease>) -> (Updates, Arc<FakeFeed>) {
    let (updates, fake) = session(vec![Ok(Some(release.clone() as Arc<dyn Release>))]);
    run(updates.check()).unwrap();
    (updates, fake)
}

fn found(version: &str) -> UpdatePhase {
    UpdatePhase::Found {
        version: version.into(),
    }
}

fn run<T>(f: impl Future<Output = T>) -> T {
    tauri::async_runtime::block_on(f)
}

/// An Install whose engine-closing step finds nothing running.
fn ready() -> std::future::Ready<Result<(), AppError>> {
    std::future::ready(Ok(()))
}

#[test]
fn an_unavailable_copy_fetches_nothing() {
    for availability in [
        UpdateAvailability::NotInThisBuild,
        UpdateAvailability::Flatpak,
        UpdateAvailability::PackageManager,
        UpdateAvailability::NotInstalled,
        UpdateAvailability::MoveToApplications,
    ] {
        let fake = Arc::new(FakeFeed::default());
        let updates = Updates::new(
            availability,
            "0.1.0".into(),
            Some(Box::new(Arc::clone(&fake))),
        );
        assert_eq!(updates.status().availability, availability);
        assert_eq!(updates.status().phase, UpdatePhase::Idle);
        for e in [
            run(updates.check()).unwrap_err(),
            run(updates.install("0.2.0", ready)).unwrap_err(),
            updates.restart().unwrap_err(),
        ] {
            assert_eq!(e.code_str(), "app.updates_unavailable");
        }
        assert_eq!(fake.checks.load(Ordering::SeqCst), 0);
        assert_eq!(fake.restarts.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn a_found_update_waits_for_install_then_for_restart() {
    let release = FakeRelease::new("0.2.0");
    let (updates, fake) = session(vec![Ok(Some(release.clone() as Arc<dyn Release>))]);
    assert_eq!(
        updates.restart().unwrap_err().code_str(),
        "app.update_state"
    );
    assert_eq!(run(updates.check()).unwrap().phase, found("0.2.0"));
    assert_eq!(release.installs.load(Ordering::SeqCst), 0);
    assert_eq!(
        updates.restart().unwrap_err().code_str(),
        "app.update_state"
    );
    // Not the version the user saw.
    assert_eq!(
        run(updates.install("0.3.0", ready)).unwrap_err().code_str(),
        "app.update_state"
    );
    // Installing does not close the engine here (macOS, Linux): nothing is
    // asked about what runs.
    let asked = AtomicUsize::new(0);
    let busy = || {
        asked.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Err(AppError::app(AppCode::UpdateBusy, "Mixing")))
    };
    let status = run(updates.install("0.2.0", busy)).unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    assert_eq!(
        status.phase,
        UpdatePhase::Installed {
            version: "0.2.0".into()
        }
    );
    assert_eq!(release.installs.load(Ordering::SeqCst), 1);
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 0);
    // Installed: no second check, no second install.
    assert_eq!(run(updates.check()).unwrap().phase, status.phase);
    assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
    assert_eq!(
        run(updates.install("0.2.0", ready)).unwrap_err().code_str(),
        "app.update_state"
    );
    updates.restart().unwrap();
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_refused_signature_installs_nothing_and_keeps_the_offer() {
    let release = Arc::new(FakeRelease {
        version: "0.2.0",
        download: Some(AppCode::UpdateSignature),
        ..Default::default()
    });
    let (updates, _) = offering(&release);
    let e = run(updates.install("0.2.0", ready)).unwrap_err();
    assert_eq!(e.code_str(), "app.update_signature");
    assert_eq!(release.installs.load(Ordering::SeqCst), 0);
    assert_eq!(updates.status().phase, found("0.2.0"));
}

#[test]
fn up_to_date_and_a_failed_check_keep_what_was_known() {
    let (updates, _) = session(vec![
        Ok(None),
        Ok(Some(FakeRelease::new("0.2.0") as Arc<dyn Release>)),
        Err(AppError::app(AppCode::UpdateCheck, "offline")),
    ]);
    assert_eq!(run(updates.check()).unwrap().phase, UpdatePhase::UpToDate);
    run(updates.check()).unwrap();
    let e = run(updates.check()).unwrap_err();
    assert_eq!(e.code_str(), "app.update_check");
    assert_eq!(updates.status().phase, found("0.2.0"));
    run(updates.install("0.2.0", ready)).unwrap();
}

/// A stalled check or download gives up, and the next check and the
/// install button work again.
#[test]
fn a_stalled_check_or_download_gives_up() {
    let stalls = Arc::new(FakeRelease {
        version: "0.2.0",
        stalls: true,
        ..Default::default()
    });
    let (updates, fake) = session(vec![]);
    let e = run(updates.check()).unwrap_err();
    assert_eq!(e.code_str(), "app.update_check");
    assert_eq!(updates.status().phase, UpdatePhase::Idle);
    fake.answers
        .lock()
        .unwrap()
        .push(Ok(Some(stalls.clone() as Arc<dyn Release>)));
    assert_eq!(run(updates.check()).unwrap().phase, found("0.2.0"));
    let e = run(updates.install("0.2.0", ready)).unwrap_err();
    assert_eq!(e.code_str(), "app.update_download");
    assert_eq!(updates.status().phase, found("0.2.0"));
    assert_eq!(stalls.installs.load(Ordering::SeqCst), 0);
}

/// The Windows installer failing after its hook shut the engine down: the
/// update is not offered again, a restart is.
#[test]
fn a_failed_install_after_the_engine_closed_asks_for_a_restart() {
    let fails = || {
        Arc::new(FakeRelease {
            version: "0.2.0",
            install: Some(AppCode::UpdateInstall),
            ..Default::default()
        })
    };
    let (updates, fake) = offering(&fails());
    let e = run(updates.install("0.2.0", ready)).unwrap_err();
    assert_eq!(e.code_str(), "app.update_install");
    assert_eq!(updates.status().phase, found("0.2.0"));
    assert_eq!(fake.reveals.load(Ordering::SeqCst), 0);

    let (updates, fake) = offering(&fails());
    updates.engine_closed();
    let e = run(updates.install("0.2.0", ready)).unwrap_err();
    assert_eq!(e.code_str(), "app.update_install");
    // The hook hid the window before the installer ran: it is shown again.
    assert_eq!(fake.reveals.load(Ordering::SeqCst), 1);
    let restart = UpdatePhase::RestartRequired {
        version: "0.2.0".into(),
    };
    assert_eq!(updates.status().phase, restart);
    assert_eq!(run(updates.check()).unwrap().phase, restart);
    assert_eq!(
        run(updates.install("0.2.0", ready)).unwrap_err().code_str(),
        "app.update_state"
    );
    updates.restart().unwrap();
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
}

/// Where installing closes the engine, what runs is asked again once the
/// download is done; when something does, the download is kept for the
/// next Install.
#[test]
fn a_busy_engine_after_the_download_keeps_it_ready_to_install() {
    let release = FakeRelease::new("0.2.0");
    let (updates, fake) = session(vec![Ok(Some(release.clone() as Arc<dyn Release>))]);
    let updates = updates.closing_engine();
    run(updates.check()).unwrap();
    let busy = || std::future::ready(Err(AppError::app(AppCode::UpdateBusy, "Payment")));
    let e = run(updates.install("0.2.0", busy)).unwrap_err();
    assert_eq!(e.code_str(), "app.update_busy");
    let downloaded = UpdatePhase::Downloaded {
        version: "0.2.0".into(),
    };
    assert_eq!(updates.status().phase, downloaded);
    assert_eq!(release.installs.load(Ordering::SeqCst), 0);
    assert_eq!(run(updates.check()).unwrap().phase, downloaded);
    assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
    assert_eq!(
        run(updates.install("0.3.0", ready)).unwrap_err().code_str(),
        "app.update_state"
    );
    run(updates.install("0.2.0", ready)).unwrap();
    assert_eq!(release.downloads.load(Ordering::SeqCst), 1);
    assert_eq!(release.installs.load(Ordering::SeqCst), 1);
}

#[test]
fn release_notes_open_the_page_of_the_release_on_offer() {
    let (updates, _) = session(vec![]);
    assert_eq!(
        updates.notes_url().unwrap_err().code_str(),
        "app.update_state"
    );
    let (updates, _) = offering(&FakeRelease::new("0.2.0"));
    assert_eq!(
        updates.notes_url().unwrap(),
        "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/tag/v0.2.0"
    );
}

#[test]
fn the_app_without_the_updater_reports_it_over_ipc() {
    let demo = crate::ipc_tests::Demo::new();
    let status = demo.ok("update_status", json!({}));
    assert_eq!(
        status,
        json!({ "availability": "notInThisBuild", "phase": { "kind": "idle" } })
    );
    demo.err("check_for_update", json!({}), "app.updates_unavailable");
    demo.err(
        "install_update",
        json!({ "version": "9.9.9" }),
        "app.updates_unavailable",
    );
    demo.err("restart_to_update", json!({}), "app.updates_unavailable");
    let settings = demo.ok("ui_settings", json!({}));
    assert_eq!(settings["checkUpdatesAutomatically"], false);
    let launch = demo.ok("launch_info", json!({}));
    let state = demo.handle().state::<crate::state::AppState>();
    assert_eq!(launch["appVersion"], state.updates.current_version());
}

/// A demo app whose updates are `updates` (offering `release`), checked.
fn demo_offering(updates: Updates) -> crate::ipc_tests::Demo {
    crate::state::set_updates(updates);
    let demo = crate::ipc_tests::Demo::new();
    demo.open();
    assert_eq!(
        demo.ok("check_for_update", json!({}))["phase"]["kind"],
        "found"
    );
    demo
}

fn busy_reason(e: &serde_json::Value) -> &serde_json::Value {
    assert_eq!(e["code"], "app.update_busy", "{e}");
    &e["data"]["reason"]
}

/// Where installing closes the engine (Windows), Install waits for a
/// payment before it downloads anything; Restart always does.
#[test]
fn install_and_restart_wait_for_a_payment() {
    let release = FakeRelease::new("0.2.0");
    let (updates, fake) = offering_later(&release);
    let demo = demo_offering(updates.closing_engine());
    let state = demo.handle().state::<crate::state::AppState>();
    let install = json!({ "version": "0.2.0" });

    let sending = state.broadcasting();
    let e = demo.err("install_update", install.clone(), "app.update_busy");
    assert_eq!(busy_reason(&e), "payment");
    assert_eq!(release.downloads.load(Ordering::SeqCst), 0);
    drop(sending);
    let status = demo.ok("install_update", install);
    assert_eq!(status["phase"]["kind"], "installed");

    let sending = state.broadcasting();
    let e = demo.err("restart_to_update", json!({}), "app.update_busy");
    assert_eq!(busy_reason(&e), "payment");
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 0);
    drop(sending);
    demo.ok("restart_to_update", json!({}));
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
}

/// A payment sent while the Windows installer downloads: the engine is not
/// closed under it, and the download waits, ready to install.
#[test]
fn a_payment_started_during_the_download_keeps_the_engine_open() {
    let release = FakeRelease::new("0.2.0");
    let (updates, _) = offering_later(&release);
    let demo = demo_offering(updates.closing_engine());
    let state = demo.handle().state::<crate::state::AppState>();
    let broadcasts = state.broadcasts();
    let sent = Arc::clone(&broadcasts);
    *release.during_download.lock().unwrap() = Some(Box::new(move || {
        sent.fetch_add(1, Ordering::SeqCst);
    }));
    let install = json!({ "version": "0.2.0" });
    let e = demo.err("install_update", install.clone(), "app.update_busy");
    assert_eq!(busy_reason(&e), "payment");
    assert_eq!(
        demo.ok("update_status", json!({}))["phase"],
        json!({ "kind": "downloaded", "version": "0.2.0" })
    );
    assert_eq!(release.installs.load(Ordering::SeqCst), 0);
    broadcasts.fetch_sub(1, Ordering::SeqCst);
    assert_eq!(
        demo.ok("install_update", install)["phase"]["kind"],
        "installed"
    );
    assert_eq!(release.downloads.load(Ordering::SeqCst), 1);
}

/// On macOS and Linux installing leaves the engine running: it does not
/// wait for a payment or mixing, the restart does.
#[test]
fn where_install_keeps_the_engine_only_restart_waits() {
    let release = FakeRelease::new("0.2.0");
    let (updates, fake) = offering_later(&release);
    let demo = demo_offering(updates);
    let state = demo.handle().state::<crate::state::AppState>();
    let sending = state.broadcasting();
    let status = demo.ok("install_update", json!({ "version": "0.2.0" }));
    assert_eq!(status["phase"]["kind"], "installed");
    demo.err("restart_to_update", json!({}), "app.update_busy");
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 0);
    drop(sending);
}

/// After a failed Windows install the engine is closed already: Restart
/// does not wait for what ran before, which the hook already ended.
#[test]
fn a_restart_after_a_failed_windows_install_does_not_wait() {
    let release = Arc::new(FakeRelease {
        version: "0.2.0",
        install: Some(AppCode::UpdateInstall),
        ..Default::default()
    });
    let (updates, fake) = offering_later(&release);
    let demo = demo_offering(updates.closing_engine());
    let state = demo.handle().state::<crate::state::AppState>();
    state.updates.engine_closed();
    demo.err(
        "install_update",
        json!({ "version": "0.2.0" }),
        "app.update_install",
    );
    assert_eq!(
        demo.ok("update_status", json!({}))["phase"]["kind"],
        "restartRequired"
    );
    assert_eq!(fake.reveals.load(Ordering::SeqCst), 1);
    let _sending = state.broadcasting();
    demo.ok("restart_to_update", json!({}));
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
}

/// Install and Restart wait while the engine is still starting.
#[test]
fn install_and_restart_wait_for_the_engine_to_start() {
    let (go, wait) = std::sync::mpsc::channel::<()>();
    crate::state::set_engine_start_hook(move || {
        let _ = wait.recv();
    });
    let demo = crate::ipc_tests::Demo::open_unready(|_, _| {});
    let state = demo.handle().state::<crate::state::AppState>();
    let e = run(crate::api::updates::refuse_while_busy(&state)).unwrap_err();
    assert_eq!(
        serde_json::to_value(&e).unwrap()["data"],
        json!({ "kind": "updateBusy", "reason": "startup" })
    );
    drop(go);
    run(state.engine.wait()).expect("the engine starts");
    run(crate::api::updates::refuse_while_busy(&state)).unwrap();
}
