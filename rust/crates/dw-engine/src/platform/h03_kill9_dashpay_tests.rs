//! H-03, the kill -9 matrix (DESIGN G5) for what a DashPay write leaves in
//! the library's store once Platform took it: an identity, a sent request
//! (send), an incoming one answered by ours (accept) and our profile
//! (update). A child process (this test binary, running
//! [`dashpay_record_child`]) stores them through the library's own calls,
//! as a landed write records them, and is killed with SIGKILL; the parent
//! opens the same data directory offline and reads them back within 60 s,
//! before any Platform re-sync could have run.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use platform_wallet::{ContactRequest, DashPayProfile};
use zeroize::Zeroizing;

use super::contacts::{ContactQuery, ContactSort, ContactsPage};
use super::journal_tests::Recorder;
use super::profile::Profile;
use crate::{
    DashNetwork, Engine, EngineConfig, ImportOptions, NetworkSession, SessionOptions, WalletId,
};

const ROOT: &str = "DWD_H03_KILL9_DASHPAY_ROOT";
const CHILD: &str = "platform::h03_kill9_dashpay_tests::dashpay_record_child";
const PASS: &[u8] = b"h03 kill9 dashpay passphrase";
const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const WITHIN: Duration = Duration::from_secs(60);
const REACH: Duration = Duration::from_secs(180);

const ME: u8 = 1;
/// We asked them (send).
const GRACE: u8 = 7;
/// They asked us, and we answered (accept).
const ERIN: u8 = 5;
/// They asked us; not answered.
const FRANK: u8 = 6;

fn id(n: u8) -> Identifier {
    Identifier::from([n; 32])
}

fn b58(n: u8) -> String {
    id(n).to_string(Encoding::Base58)
}

fn request(sender: u8, recipient: u8, created_at: u64) -> ContactRequest {
    ContactRequest::new(
        id(sender),
        id(recipient),
        0,
        0,
        0,
        vec![1; 96],
        1,
        created_at,
    )
}

fn engine_at(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Recorder::default()),
    )
    .unwrap()
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform: true,
                ..Default::default()
            },
        ))
        .unwrap()
}

/// What the wallet shows of our identity: its contacts page and profile.
fn shown(
    s: &Arc<NetworkSession>,
    wallet: WalletId,
) -> (
    Result<ContactsPage, &'static str>,
    Result<Option<Profile>, &'static str>,
) {
    let dp = s.dashpay(wallet);
    let page = dp.contacts(
        b58(ME),
        ContactQuery {
            sections: Vec::new(),
            sort: ContactSort::DateAdded,
            text: None,
        },
    );
    (
        page.map_err(|e| e.code()),
        dp.profile(b58(ME)).map_err(|e| e.code()),
    )
}

/// Run by [`dashpay_records_survive_a_kill`] in a child process: the
/// records of a landed send, accept and profile update, stored as the
/// library stores them; then it reports what it shows and waits for its
/// SIGKILL.
#[test]
#[ignore = "the child process of the H-03 DashPay kill matrix"]
fn dashpay_record_child() {
    let Some(root) = std::env::var_os(ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    let engine = engine_at(&root);
    let s = open(&engine);
    engine
        .block_on(s.vault_op(|v| v.create(Some(PASS))))
        .unwrap();
    let wallet = engine
        .block_on(s.import_wallet(
            Zeroizing::new(PHRASE.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let manager = s.manager().unwrap();
    engine.block_on(async move {
        let wallet = manager.get_wallet(&wallet.0).await.unwrap();
        let mut state = wallet.state_mut().await;
        let p = wallet.persister();
        let me = Identity::V0(IdentityV0 {
            id: id(ME),
            public_keys: super::requests::tests::keys(true),
            balance: 1_000_000,
            revision: 0,
        });
        (state.identity_manager)
            .add_identity(me, 0, wallet.wallet_id(), p)
            .unwrap();
        let m = (state.identity_manager.managed_identity_mut(&id(ME))).unwrap();
        m.add_sent_contact_request(request(ME, GRACE, 4_000), p)
            .unwrap();
        m.add_incoming_contact_request(request(ERIN, ME, 2_000), p)
            .unwrap();
        m.add_sent_contact_request(request(ME, ERIN, 3_000), p)
            .unwrap();
        m.add_incoming_contact_request(request(FRANK, ME, 5_000), p)
            .unwrap();
        m.set_dashpay_profile(
            Some(DashPayProfile {
                display_name: Some("Me after the edit".into()),
                public_message: Some("hello".into()),
                bio: Some("hello".into()),
                ..Default::default()
            }),
            p,
        );
    });
    let (page, profile) = shown(&s, wallet);
    let (page, profile) = (page.unwrap(), profile.unwrap());
    assert!(!page.sections.is_empty(), "{page:?}");
    assert!(profile.is_some());
    let report = format!(
        "{wallet}\n{}\n",
        serde_json::to_string(&(page, profile)).unwrap()
    );
    let tmp = root.join("reached.tmp");
    std::fs::write(&tmp, report).unwrap();
    std::fs::rename(&tmp, root.join("reached")).unwrap();
    loop {
        std::thread::park();
    }
}

/// After the record, before any confirmation of it is stored or any
/// re-sync runs: what the library stored for a landed send, accept and
/// profile update reads the same after a kill -9, offline.
#[test]
fn dashpay_records_survive_a_kill() {
    let dir = dw_testutil::private_tempdir();
    let log = std::fs::File::create(dir.path().join("child.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([CHILD, "--exact", "--ignored", "--test-threads=1"])
        .env(ROOT, dir.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let reached = dir.path().join("reached");
    let start = Instant::now();
    while !reached.exists() {
        let exited = child.try_wait().unwrap();
        if exited.is_some() || start.elapsed() > REACH {
            let _ = child.kill();
            let _ = child.wait();
            let log = std::fs::read_to_string(dir.path().join("child.log")).unwrap_or_default();
            panic!("the child never stored its records ({exited:?}): {log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    }
    let report = std::fs::read_to_string(&reached).unwrap();
    let (wallet, before) = report.split_once('\n').unwrap();
    let wallet: WalletId = wallet.parse().unwrap();
    let before: (ContactsPage, Option<Profile>) = serde_json::from_str(before.trim()).unwrap();

    let engine = engine_at(dir.path());
    let s = open(&engine);
    let deadline = Instant::now() + WITHIN;
    let after = loop {
        let after = shown(&s, wallet);
        let equal =
            matches!(&after, (Ok(p), Ok(f)) if !p.updating && (p, f) == (&before.0, &before.1));
        if equal || Instant::now() >= deadline {
            break after;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(after.0, Ok(before.0), "the contacts page after the kill");
    assert_eq!(after.1, Ok(before.1), "the profile after the kill");
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
}
