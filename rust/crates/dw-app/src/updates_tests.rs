use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use serde_json::json;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use tauri_plugin_updater::UpdaterExt;

use super::*;

fn conf(text: &str) -> tauri::Config {
    serde_json::from_str(text).unwrap()
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

fn installed(bundle: Option<BundleType>) -> Install {
    Install {
        flatpak: false,
        bundle,
        appimage: true,
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
    /// comment the Tauri CLI writes.
    fn sign(&self, bytes: &[u8], version: &str) -> String {
        let comment = format!("timestamp:1790000000\tfile:payload\tversion:{version}");
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
}

/// The shipped config, whatever its public key (the placeholder until the
/// release key exists, docs/release/updater.md): without the overlay that
/// only a build with the signing key gets, no feed and no updater.
#[test]
fn the_shipped_build_has_no_updater() {
    let shipped = shipped();
    let pubkey = shipped["plugins"]["updater"]["pubkey"].as_str().unwrap();
    assert!(!pubkey.trim().is_empty());
    assert_eq!(shipped["plugins"]["updater"]["endpoints"], json!([]));
    assert_eq!(shipped["plugins"]["updater"]["requireSignedVersion"], true);
    assert!(shipped["bundle"].get("createUpdaterArtifacts").is_none());
    for pubkey in [
        json!(pubkey),
        json!(PUBKEY_PLACEHOLDER),
        json!(TestKey::new().pubkey()),
    ] {
        let mut c = shipped.clone();
        c["plugins"]["updater"]["pubkey"] = pubkey;
        let config = conf(&c.to_string());
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
    let mut merged = shipped();
    merge(&mut merged, &overlay);
    // The overlay with the placeholder: still off.
    merged["plugins"]["updater"]["pubkey"] = json!(PUBKEY_PLACEHOLDER);
    assert!(!build_has_updater(&conf(&merged.to_string())));
    // With a release key in place of the placeholder, the build has it.
    merged["plugins"]["updater"]["pubkey"] = json!(TestKey::new().pubkey());
    assert!(build_has_updater(&conf(&merged.to_string())));
}

#[test]
fn only_the_official_feed_with_signed_versions_counts() {
    let mut merged = shipped();
    merge(&mut merged, &overlay());
    merged["plugins"]["updater"]["pubkey"] = json!(TestKey::new().pubkey());
    let with = |path: &str, value: serde_json::Value| {
        let mut c = merged.clone();
        c["plugins"]["updater"][path] = value;
        build_has_updater(&conf(&c.to_string()))
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
    let mut no_plugin = merged.clone();
    no_plugin["plugins"]
        .as_object_mut()
        .unwrap()
        .remove("updater");
    assert!(!build_has_updater(&conf(&no_plugin.to_string())));
}

#[test]
fn the_install_decides_in_a_build_with_the_updater() {
    let mut merged = shipped();
    merge(&mut merged, &overlay());
    merged["plugins"]["updater"]["pubkey"] = json!(TestKey::new().pubkey());
    let config = conf(&merged.to_string());
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
}

#[test]
fn a_flatpak_never_updates_itself() {
    let mut merged = shipped();
    merge(&mut merged, &overlay());
    merged["plugins"]["updater"]["pubkey"] = json!(TestKey::new().pubkey());
    for config in [conf(&merged.to_string()), conf(&shipped().to_string())] {
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

#[test]
fn notes_open_the_release_page_of_the_tag() {
    assert_eq!(
        notes_url("0.6.0"),
        "https://github.com/PastaPastaPasta/dashwallet-desktop/releases/tag/v0.6.0"
    );
}

/// The app with the updater plugin, as a build with the release key has
/// it, `pubkey` being the test key's.
fn app_with_updater(pubkey: &str) -> tauri::App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        json!({
            "pubkey": pubkey,
            "endpoints": [FEED_URL],
            "requireSignedVersion": true,
        }),
    );
    assert!(build_has_updater(context.config()));
    mock_builder().plugin(plugin()).build(context).expect("app")
}

/// A `latest.json` with one entry for this system.
fn feed(version: &str, signature: &str) -> serde_json::Value {
    let target = tauri_plugin_updater::target().expect("an updater target");
    json!({
        "version": version,
        "notes": "See the release page.",
        "pub_date": "2026-11-29T00:00:00Z",
        "platforms": {
            target: {
                "url": format!("https://github.com/PastaPastaPasta/dashwallet-desktop/releases/download/v{version}/payload"),
                "signature": signature,
            }
        }
    })
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

#[test]
fn an_old_signed_build_relabelled_as_new_is_refused() {
    let key = TestKey::new();
    let app = app_with_updater(&key.pubkey());
    // A genuine signature of 0.1.0, announced as 0.2.0: a downgrade in disguise.
    let update = offered(&app, feed("0.2.0", &key.sign(PAYLOAD, "0.1.0"))).unwrap();
    let e = update.verify(PAYLOAD).unwrap_err();
    assert!(
        matches!(e, tauri_plugin_updater::Error::SignedVersionMismatch { .. }),
        "{e}"
    );
    assert_eq!(
        plugin_error(e, AppCode::UpdateInstall).code_str(),
        "app.update_signature"
    );
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

/// A feed that answers from a script and counts what it is asked.
#[derive(Default)]
struct FakeFeed {
    answers: Mutex<Vec<Answer>>,
    checks: AtomicUsize,
    restarts: Arc<AtomicUsize>,
}

impl Feed for Arc<FakeFeed> {
    fn check(&self) -> BoxFuture<'_, Answer> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers.lock().unwrap().remove(0);
        Box::pin(async move { answer })
    }

    fn restart(&self) {
        self.restarts.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeRelease {
    version: &'static str,
    download: Result<(), AppCode>,
    installs: AtomicUsize,
}

impl FakeRelease {
    fn new(version: &'static str) -> Arc<Self> {
        Arc::new(FakeRelease {
            version,
            download: Ok(()),
            installs: AtomicUsize::new(0),
        })
    }
}

impl Release for FakeRelease {
    fn version(&self) -> String {
        self.version.into()
    }

    fn download<'a>(&'a self, progress: Progress<'a>) -> BoxFuture<'a, Result<Vec<u8>, AppError>> {
        Box::pin(async move {
            progress(5, Some(10));
            progress(10, Some(10));
            match self.download {
                Ok(()) => Ok(PAYLOAD.to_vec()),
                Err(code) => Err(AppError::app(code, "refused")),
            }
        })
    }

    fn install(&self, bytes: &[u8]) -> Result<(), AppError> {
        assert_eq!(bytes, PAYLOAD);
        self.installs.fetch_add(1, Ordering::SeqCst);
        Ok(())
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
    );
    (updates, fake)
}

fn run<T>(f: impl Future<Output = T>) -> T {
    tauri::async_runtime::block_on(f)
}

#[test]
fn an_unavailable_copy_fetches_nothing() {
    for availability in [
        UpdateAvailability::NotInThisBuild,
        UpdateAvailability::Flatpak,
        UpdateAvailability::PackageManager,
        UpdateAvailability::NotInstalled,
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
            run(updates.install("0.2.0")).unwrap_err(),
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
    let status = run(updates.check()).unwrap();
    assert_eq!(
        status.phase,
        UpdatePhase::Found {
            version: "0.2.0".into()
        }
    );
    assert_eq!(release.installs.load(Ordering::SeqCst), 0);
    assert_eq!(updates.notes_url().unwrap(), notes_url("0.2.0"));
    assert_eq!(
        updates.restart().unwrap_err().code_str(),
        "app.update_state"
    );
    // Not the version the user saw.
    assert_eq!(
        run(updates.install("0.3.0")).unwrap_err().code_str(),
        "app.update_state"
    );
    let status = run(updates.install("0.2.0")).unwrap();
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
        run(updates.install("0.2.0")).unwrap_err().code_str(),
        "app.update_state"
    );
    updates.restart().unwrap();
    assert_eq!(fake.restarts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_refused_signature_installs_nothing_and_keeps_the_offer() {
    let release = Arc::new(FakeRelease {
        version: "0.2.0",
        download: Err(AppCode::UpdateSignature),
        installs: AtomicUsize::new(0),
    });
    let (updates, _) = session(vec![Ok(Some(release.clone() as Arc<dyn Release>))]);
    run(updates.check()).unwrap();
    let e = run(updates.install("0.2.0")).unwrap_err();
    assert_eq!(e.code_str(), "app.update_signature");
    assert_eq!(release.installs.load(Ordering::SeqCst), 0);
    assert_eq!(
        updates.status().phase,
        UpdatePhase::Found {
            version: "0.2.0".into()
        }
    );
}

#[test]
fn up_to_date_and_a_failed_check_keep_what_was_known() {
    let (updates, _) = session(vec![
        Ok(None),
        Ok(Some(FakeRelease::new("0.2.0") as Arc<dyn Release>)),
        Err(AppError::app(AppCode::UpdateCheck, "offline")),
    ]);
    assert!(matches!(
        run(updates.check()).unwrap().phase,
        UpdatePhase::UpToDate { checked_at } if checked_at > 0
    ));
    assert_eq!(
        updates.notes_url().unwrap_err().code_str(),
        "app.update_state"
    );
    run(updates.check()).unwrap();
    let e = run(updates.check()).unwrap_err();
    assert_eq!(e.code_str(), "app.update_check");
    assert_eq!(
        updates.status().phase,
        UpdatePhase::Found {
            version: "0.2.0".into()
        }
    );
    run(updates.install("0.2.0")).unwrap();
}

#[test]
fn the_app_without_the_updater_reports_it_over_ipc() {
    let demo = crate::ipc_tests::Demo::new();
    let status = demo.ok("update_status", json!({}));
    assert_eq!(status["availability"], "notInThisBuild");
    assert_eq!(status["phase"], json!({ "kind": "idle" }));
    demo.err("check_for_update", json!({}), "app.updates_unavailable");
    demo.err(
        "install_update",
        json!({ "version": "9.9.9" }),
        "app.updates_unavailable",
    );
    demo.err("restart_to_update", json!({}), "app.updates_unavailable");
    demo.err("open_update_notes", json!({}), "app.update_state");
    let settings = demo.ok("ui_settings", json!({}));
    assert_eq!(settings["checkUpdatesAutomatically"], false);
}
