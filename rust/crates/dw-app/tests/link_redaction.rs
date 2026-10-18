//! DP5-01 D14 (DASHPAY §3.8): no log sink carries a link or a part of one.
//!
//! The test runs itself again as a child process, which installs the app's
//! logger (`dw_app::install_logging`, every target at `trace`) and the link
//! trace, then takes planted links in the way the app does: queued as launch
//! arguments, routed and stashed before a vault exists, resealed when the
//! vault is created, listed, taken and forgotten. It also logs one link on
//! purpose, as a careless log line would, as a field, in a panic message,
//! and with a line break inside it, also as a bad launch option (D30). The
//! parent then greps everything
//! the child wrote (stderr, stdout and the trace) for each planted secret:
//! the invitation's key and inviter, the payee address, the amount and
//! label, and the contact's username.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use dw_app::shell::Shell;
use dw_app::shell::links::LinkSource;
use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

/// Set in the child: the directory it works in.
const CHILD: &str = "DW_LINK_REDACTION_CHILD";
const TEST: &str = "no_log_sink_carries_a_link";
const DONE: &str = "link-redaction child done";

const ADDRESS: &str = "yMsgnH1xKGa85n4bq2imrZbG2KgrmGttAV";
const IDENTITY: &str = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";
/// The rest of a link past a line break (D30).
const SUFFIX: &str = "PlantedSuffix8K";
/// What must never appear in a log, one planted value per kind.
const SECRETS: [&str; 8] = [
    "cPlantedInvitationKey7Q", // an invitation's private key (`pk=`)
    "plantedinviter",          // the inviter (`du=`)
    ADDRESS,                   // a payee
    "0.4242",                  // an amount
    "PlantedLabel",            // a label
    "plantedcontact",          // a contact's username
    IDENTITY,                  // a contact's identity
    SUFFIX,                    // a link's rest past a line break
];

fn planted_links() -> Vec<String> {
    let [key, inviter, ..] = SECRETS;
    vec![
        format!("dash:{ADDRESS}?amount=0.4242&label=PlantedLabel"),
        format!("dashpay://user?id={IDENTITY}&username=plantedcontact"),
        format!("dashpay://invite?du={inviter}&pk={key}&assetlocktx=00&islock=00"),
        format!(
            "https://invitations.dashpay.io/applink?du={inviter}&pk={key}&assetlocktx=00&islock=00"
        ),
        format!(
            "https://dashpay.onelink.me/abc?af_dp=dashpay%3A%2F%2Finvite%3Fdu%3D{inviter}%26pk%3D{key}"
        ),
        format!("dashwallet://pay={ADDRESS}&amount=0.4242&sender=plantedcontact"),
    ]
}

#[test]
fn no_log_sink_carries_a_link() {
    if let Some(dir) = std::env::var_os(CHILD) {
        child(Path::new(&dir));
        return;
    }
    let dir = dw_testutil::private_tempdir();
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD, dir.path())
        .env_remove("DW_LOG")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains(DONE),
        "the child failed: {}\n{stdout}\n{}",
        out.status,
        dw_uri::redact::redact_links(&stderr)
    );
    let trace = std::fs::read_to_string(dir.path().join("trace")).unwrap();
    for (sink, text) in [
        ("stderr", &*stderr),
        ("stdout", &*stdout),
        ("trace", &*trace),
    ] {
        for secret in SECRETS {
            assert!(
                !text.contains(secret),
                "{sink} carries the planted {secret:?}"
            );
        }
    }
    // The controls: the child logged at trace level (engine and vault
    // events), the careless line arrived redacted, and the trace has a
    // line per link.
    assert!(
        stderr.contains("TRACE") || stderr.contains("DEBUG"),
        "{stderr}"
    );
    assert!(
        stderr.contains("careless link line: <link>"),
        "no redacted control line in: {stderr}"
    );
    // A link as a field value, and in a panic message (review r1 #1, #2);
    // no colour codes at all.
    assert!(stderr.contains("careless field link=<link>"), "{stderr}");
    assert!(stderr.contains("careless panic: <link>"), "{stderr}");
    // D30: a line break inside a link carries nothing of it on.
    assert!(stderr.contains("careless multiline: <link>"), "{stderr}");
    assert!(
        stderr.contains("dash-wallet: unknown option --bad\n"),
        "{stderr}"
    );
    assert!(!stderr.contains('\x1b'), "{stderr}");
    let queued = planted_links()
        .iter()
        .filter(|l| !l.starts_with("https"))
        .count();
    assert!(stdout.contains(&format!("queued {queued}")), "{stdout}");
    assert_eq!(
        trace
            .lines()
            .filter(|l| l.starts_with("received launch"))
            .count(),
        queued,
        "{trace}"
    );
}

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

fn child(dir: &Path) {
    dw_app::install_logging("trace").unwrap();
    // As `dw_app::run` does.
    dw_uri::redact::redact_panics();
    let shell = Shell::new(Some(dir.join("trace")));
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap();
    let options = SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    };
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, options))
        .unwrap();
    let links = planted_links();
    // `https` links are no launch arguments (the OS opens a browser for
    // them); they arrive through Open URI below.
    let queued = shell.queue(links.clone(), LinkSource::Launch);
    assert_eq!(
        queued,
        links.iter().filter(|l| !l.starts_with("https")).count()
    );
    println!("queued {queued}");
    engine.block_on(async {
        let routes = shell
            .inbox
            .take(Some(Arc::clone(&session)), "regtest")
            .await
            .unwrap();
        assert_eq!(routes.len(), queued);
        // The same links typed into Open URI.
        for link in &links {
            dw_app::shell::links::intake(Some(Arc::clone(&session)), "regtest", link)
                .await
                .unwrap();
        }
        let stashed = session.stashed_links().await.unwrap();
        assert!(stashed.len() >= 3, "{stashed:?}");
        session
            .vault_op(|v| v.create(Some(b"pw".as_slice())))
            .await
            .unwrap();
        for link in session.stashed_links().await.unwrap() {
            if let Some(dw_engine::platform::TakenLink::Invitation) =
                session.take_stashed_link(link.id.clone()).await.unwrap()
            {
                session.forget_invitation(link.id).await.unwrap();
            }
        }
        assert_eq!(session.stashed_links().await.unwrap(), vec![]);
    });
    tracing::error!("careless link line: {}", links[2]);
    tracing::error!(link = %links[0], "careless field");
    let link = links[2].clone();
    std::panic::catch_unwind(move || panic!("careless panic: {link}")).unwrap_err();
    let broken = format!("{}\n{SUFFIX}&x=1", links[2]);
    tracing::error!("careless multiline: {broken}");
    // A bad option whose value holds the link, as `dw_app::run` prints it.
    let e = dw_app::launch::parse([format!("--bad={broken}")]).unwrap_err();
    eprintln!("{}", dw_app::launch_error(&e));
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    println!("{DONE}");
}
