//! H-03 rows R2 and R3: a contact request sent, or Frank's accepted, killed
//! with SIGKILL in a child process before Platform has our request, once
//! Platform accepted it, or as a failed write is read back; done again after
//! the relaunch, it ends with one request of ours on Platform, which the
//! wallet shows as pending (R2) or as an established contact (R3).
//!
//! R4: a profile edit killed before Platform has the document or once it
//! accepted it, before the wallet remembers it; made again after the
//! relaunch, Platform holds the edit once and the wallet shows it.
//!
//! "Before Platform has it" and "once Platform accepted it" kill the child
//! at the same moment, during the write (nothing local is written between
//! Platform accepting and the write returning); they differ in whether the
//! fake Platform kept the write for the relaunch.

use std::sync::Arc;

use zeroize::Zeroizing;

use super::super::super::journal_tests::ABANDON_12;
use super::super::super::profile::{AvatarChange, ProfileEdit};
use super::super::super::profiles::{merged, update};
use super::*;
use crate::ImportOptions;
use crate::kill9;

const ROW: &str = "contact_request";
const CHILD: &str = "platform::requests::tests::kill9::request_child";
const PROFILE_ROW: &str = "profile";
const PROFILE_CHILD: &str = "platform::requests::tests::kill9::profile_child";
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
    let f = fixture_at(kill9::scratch(&child), &child.root, true, false);
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
    // One request on Platform: a write after the relaunch is needed if it
    // had not landed, and refused as a duplicate if it had.
    let writes = f.net.writes();
    assert!(
        if landed { writes <= 1 } else { writes == 1 },
        "{writes} writes after the relaunch (landed: {landed})"
    );
    assert_eq!(f.eligibility(contact).unwrap(), after);
    let writes = f.net.writes();
    let again = write(&f, op);
    assert!(again.is_err(), "{again:?}");
    assert_eq!(f.net.writes(), writes, "refused before writing: {again:?}");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r2_send_killed_before_platform_has_it_sends_one_request() {
    killed("send", "before_write");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r2_send_killed_once_platform_accepted_it_keeps_one_request() {
    killed("send", "accepted");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r2_send_killed_reading_back_its_failed_write_keeps_one_request() {
    killed("send", "before_remember");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r3_accept_killed_before_platform_has_it_establishes_once() {
    killed("accept", "before_write");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r3_accept_killed_once_platform_accepted_it_establishes_once() {
    killed("accept", "accepted");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r3_accept_killed_reading_back_its_failed_write_establishes_once() {
    killed("accept", "before_remember");
}

/// R4's edit: a name and a message on a wallet with no profile yet.
fn edit() -> ProfileEdit {
    ProfileEdit {
        display_name: Some("Alice".into()),
        public_message: Some("hi".into()),
        avatar: AvatarChange::Keep,
        baseline: None,
    }
}

fn update_profile(f: &Fixture) -> Result<(), PlatformError> {
    let (net, wallet, grant) = (Arc::clone(&f.net), f.wallet, grant(&f.s, f.wallet));
    let me = b58(ME);
    let dp = f.s.dashpay(f.wallet);
    f.engine.block_on(dp.on_wallet(move |s, _, w| async move {
        update(&*net, &s, &w, (wallet, &me), edit(), grant).await
    }))
}

/// Run by [`profile_killed`] in a child process: [`edit`] taken to the
/// point, then reported with whether Platform has the document.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn profile_child() {
    let Some(child) = kill9::child(PROFILE_ROW) else {
        return;
    };
    let f = fixture_at(kill9::scratch(&child), &child.root, true, false);
    let landed = match child.point.as_str() {
        "before_write" => false,
        "accepted" => true,
        other => panic!("no profile kill point {other:?}"),
    };
    let report = serde_json::json!({ "landed": landed }).to_string();
    let child = Arc::new(child);
    *f.net.during.lock().unwrap() = Some(Box::new(move || kill9::reached(&child, &report)));
    let r = update_profile(&f);
    panic!("the edit ended before its kill point: {r:?}");
}

/// R4 at `point`: killed, relaunched, made again.
fn profile_killed(point: &str) {
    let (dir, report) = kill9::killed_at(PROFILE_CHILD, PROFILE_ROW, point);
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    let landed = report["landed"].as_bool().unwrap();
    let root = dir.path().to_path_buf();
    let f = fixture_at(dir, &root, false, landed);
    let wanted = merged(None, &edit());
    if landed {
        let doc = ProfileDocument {
            id: Identifier::from([9; 32]),
            revision: 1,
            properties: wanted.clone(),
        };
        f.net.profiles.lock().unwrap().insert(id(ME), doc);
    }
    let shown = || f.s.dashpay(f.wallet).profile(b58(ME)).unwrap();
    assert_eq!(shown(), None, "the killed edit remembered nothing");

    update_profile(&f).unwrap();

    assert_eq!(f.net.writes(), u32::from(!landed), "written once in all");
    let doc = f.net.profiles.lock().unwrap()[&id(ME)].clone();
    assert_eq!((doc.revision, doc.properties), (1, wanted));
    let shown = shown().unwrap();
    assert_eq!(
        (
            shown.display_name.as_deref(),
            shown.public_message.as_deref()
        ),
        (Some("Alice"), Some("hi"))
    );
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r4_profile_edit_killed_before_platform_has_it_publishes_once() {
    profile_killed("before_write");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r4_profile_edit_killed_once_platform_accepted_it_shows_it() {
    profile_killed("accepted");
}
