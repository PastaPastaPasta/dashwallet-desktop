//! H-03 rows R2 and R3: a contact request sent, or Frank's accepted, killed
//! with SIGKILL in a child process before Platform has our request, once
//! Platform accepted it, or as a failed write is read back; done again after
//! the relaunch, it ends with one request of ours on Platform, which the
//! wallet shows as pending (R2) or as an established contact (R3).

use std::sync::Arc;

use zeroize::Zeroizing;

use super::super::super::journal_tests::ABANDON_12;
use super::*;
use crate::ImportOptions;
use crate::kill9;

const ROW: &str = "contact_request";
const CHILD: &str = "platform::requests::tests::kill9::request_child";
/// Encrypted: an unencrypted vault's key is in an OS store the relaunch
/// would not have.
const PASS: &[u8] = b"kill9 requests";

/// A grant for the sends' Platform writes.
fn grant(s: &NetworkSession, wallet: WalletId) -> String {
    let purpose = GrantPurpose::PlatformOp {
        max_duffs: 0,
        max_credits: u64::MAX,
    };
    (s.vault
        .authorize(purpose, Some(&wallet.0), Credential::Passphrase(PASS)))
    .unwrap()
    .id
}

/// [`Fixture`] on `root`: the wallet imported, our identity and Frank's
/// request added on the first launch, opened as stored on the relaunch;
/// Platform holding our request if `landed`.
fn fixture_at(
    dir: tempfile::TempDir,
    root: &std::path::Path,
    first: bool,
    landed: bool,
) -> Fixture {
    let (engine, s) = kill9::engine_at(root, PASS, true);
    let wallet = if first {
        let phrase = Zeroizing::new(ABANDON_12.to_vec());
        let options = ImportOptions::default();
        (engine.block_on(s.import_wallet(phrase, Zeroizing::new(Vec::new()), options))).unwrap()
    } else {
        let wallets = s.wallet_infos().unwrap();
        assert_eq!(wallets.len(), 1);
        wallets[0].wallet_id
    };
    let grants = vec![grant(&s, wallet)];
    let flow = engine.block_on(s.begin_flow(wallet, FlowKind::EnableDashPayKeys, grants, None));
    let lease = s.lease_for(&wallet, &flow.unwrap()).unwrap().unwrap();
    let signer = lease.identity_signer(&[0]).unwrap();
    let real = (engine.block_on(signer.derive_public_keys(0, &registration_specs()))).unwrap();
    lease.end();
    let mine = (keys(true).into_keys())
        .map(|k| (k, real[&k].clone()))
        .collect();
    let me = identity(ME, mine, u64::MAX / 2);
    let net = Arc::new(FakeRequests::with([
        me.clone(),
        identity(BOB, keys(true), 0),
    ]));
    if landed {
        net.landed.store(true, Ordering::SeqCst);
        // As Platform's unique index refuses a second request.
        *net.fails.lock().unwrap() = Some(PlatformError::Internal {
            detail: "a contact request to this identity exists".into(),
        });
    }
    if first {
        let manager = s.manager().unwrap();
        engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let persister = wallet.persister();
            (state.identity_manager)
                .add_identity(me, 0, wallet.wallet_id(), persister)
                .unwrap();
            let m = (state.identity_manager.managed_identity_mut(&id(ME))).unwrap();
            m.add_incoming_contact_request(request(FRANK, ME, 3_000), persister)
                .unwrap();
        });
    }
    Fixture {
        engine,
        s,
        wallet,
        net,
        events: Arc::new(Recorder::default()),
        _dir: dir,
    }
}

/// The write killed: a send to Bob (R2) or the accept of Frank's request
/// (R3).
fn write(f: &Fixture, op: &str) -> Result<RequestOutcome, ContactError> {
    let grant = grant(&f.s, f.wallet);
    match op {
        "send" => f.send(BOB, &grant),
        "accept" => f.accept(FRANK, &grant),
        _ => panic!("no contact request write {op:?}"),
    }
}

/// Run by [`killed`] in a child process: the write taken to the point
/// (`op/point`), then reported with whether Platform has our request.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn request_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    let f = fixture_at(dw_testutil::private_tempdir(), &child.root, true, false);
    let child = Arc::new(child);
    let park = |landed: bool| -> Box<dyn Fn() + Send + Sync> {
        let child = Arc::clone(&child);
        let report = serde_json::json!({ "landed": landed }).to_string();
        Box::new(move || kill9::reached(&child, &report))
    };
    let (op, point) = child.point.split_once('/').unwrap();
    match point {
        "before_write" => *f.net.during.lock().unwrap() = Some(park(false)),
        "accepted" => *f.net.during.lock().unwrap() = Some(park(true)),
        "before_remember" => {
            *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
            f.net.landed.store(true, Ordering::SeqCst);
            *f.net.reading.lock().unwrap() = Some(park(true));
        }
        other => panic!("no contact request kill point {other:?}"),
    }
    let r = write(&f, op);
    panic!("the write ended before its kill point: {r:?}");
}

/// R2 or R3 at `op/point`: killed, relaunched, done again.
fn killed(op: &str, point: &str) {
    let (dir, report) = kill9::killed_at(CHILD, ROW, &format!("{op}/{point}"));
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    let landed = report["landed"].as_bool().unwrap();
    let root = dir.path().to_path_buf();
    let f = fixture_at(dir, &root, false, landed);
    let (contact, before, after) = match op {
        "send" => (BOB, Eligibility::Ok, Eligibility::PendingOutgoing),
        _ => (
            FRANK,
            Eligibility::PendingIncoming,
            Eligibility::AlreadyContact,
        ),
    };
    assert_eq!(
        f.eligibility(contact).unwrap(),
        before,
        "the killed write recorded nothing"
    );

    let done = write(&f, op);

    match op {
        "send" => assert!(
            matches!(done, Ok(RequestOutcome::Pending { .. })),
            "{done:?}"
        ),
        _ => assert!(
            matches!(done, Ok(RequestOutcome::Established { .. })),
            "{done:?}"
        ),
    }
    let new_on_platform = if landed { 0 } else { f.net.writes() };
    assert_eq!(
        u32::from(landed) + new_on_platform,
        1,
        "one request on Platform"
    );
    assert_eq!(f.eligibility(contact).unwrap(), after);
    let writes = f.net.writes();
    let again = write(&f, op);
    assert!(again.is_err(), "{again:?}");
    assert_eq!(f.net.writes(), writes, "refused before writing: {again:?}");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored"]
fn kill9_r2_send_killed_before_platform_has_it_sends_one_request() {
    killed("send", "before_write");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored"]
fn kill9_r2_send_killed_once_platform_accepted_it_keeps_one_request() {
    killed("send", "accepted");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored"]
fn kill9_r2_send_killed_reading_back_its_failed_write_keeps_one_request() {
    killed("send", "before_remember");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored"]
fn kill9_r3_accept_killed_before_platform_has_it_establishes_once() {
    killed("accept", "before_write");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored"]
fn kill9_r3_accept_killed_once_platform_accepted_it_establishes_once() {
    killed("accept", "accepted");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored"]
fn kill9_r3_accept_killed_reading_back_its_failed_write_establishes_once() {
    killed("accept", "before_remember");
}
