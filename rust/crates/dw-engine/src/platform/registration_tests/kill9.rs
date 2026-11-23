//! H-03 rows R1: a DashPay registration killed with SIGKILL at a scripted
//! call ([`super::When`]), in a child process; the relaunched flow, with
//! Platform as the child left it, ends with one lock, one identity, one name
//! and one profile.

use std::str::FromStr;

use super::*;
use crate::kill9;

const ROW: &str = "registration";
const CHILD: &str = "platform::registration_tests::kill9::registration_child";

/// The kill points of R1, by name.
fn point(name: &str) -> (Call, When) {
    match name {
        "fund_tracked" => (Call::Fund, When::Tracked),
        "register_identity_after" => (Call::RegisterIdentity, When::After),
        "create_profile_after" => (Call::CreateProfile, When::After),
        _ => panic!("no registration kill point {name:?}"),
    }
}

fn status_name(s: LockStatus) -> &'static str {
    match s {
        LockStatus::Built => "built",
        LockStatus::Broadcast => "broadcast",
        LockStatus::InstantSendLocked => "is",
        LockStatus::ChainLocked => "cl",
        LockStatus::Consumed => "consumed",
        LockStatus::RecoveredFromChain => "recovered",
    }
}

fn status_of(name: &str) -> LockStatus {
    [
        LockStatus::Built,
        LockStatus::Broadcast,
        LockStatus::InstantSendLocked,
        LockStatus::ChainLocked,
        LockStatus::Consumed,
        LockStatus::RecoveredFromChain,
    ]
    .into_iter()
    .find(|s| status_name(*s) == name)
    .unwrap()
}

/// What the scripted Platform holds, for the relaunch's fake.
fn platform_state(lib: &Fake) -> serde_json::Value {
    let w = lib.w();
    serde_json::json!({
        "locks": w.locks.iter().map(|l| serde_json::json!({
            "outpoint": l.outpoint.to_string(),
            "registration": l.registration,
            "index": l.identity_index,
            "account": l.account,
            "amount": l.amount,
            "status": status_name(l.status),
        })).collect::<Vec<_>>(),
        "identities": w.identities,
        "names": w.names,
        "profiles": w.profiles,
        "funds": w.funds,
        "identity_creates": w.identity_creates,
    })
}

fn restore_platform(lib: &Fake, state: &serde_json::Value) {
    let mut w = lib.w();
    w.locks = (state["locks"].as_array().unwrap().iter())
        .map(|l| LockInfo {
            outpoint: OutPoint::from_str(l["outpoint"].as_str().unwrap()).unwrap(),
            registration: l["registration"].as_bool().unwrap(),
            identity_index: l["index"].as_u64().unwrap() as u32,
            account: l["account"].as_u64().unwrap() as u32,
            amount: l["amount"].as_u64().unwrap(),
            status: status_of(l["status"].as_str().unwrap()),
        })
        .collect();
    w.identities = serde_json::from_value(state["identities"].clone()).unwrap();
    w.names = serde_json::from_value(state["names"].clone()).unwrap();
    w.profiles = serde_json::from_value(state["profiles"].clone()).unwrap();
    w.funds = state["funds"].as_u64().unwrap() as u32;
    w.identity_creates = state["identity_creates"].as_u64().unwrap() as u32;
}

fn fixture_at(dir: tempfile::TempDir, root: &std::path::Path, import: bool) -> Fixture {
    let (engine, session) = kill9::engine_at(root, PASS, false);
    let wallet = if import {
        engine
            .block_on(session.import_wallet(
                Zeroizing::new(PHRASE.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap()
    } else {
        let ids = session.wallet_infos().unwrap();
        assert_eq!(ids.len(), 1);
        ids[0].wallet_id
    };
    assert!(engine.block_on(session.leases.refresh_funding(wallet)));
    Fixture {
        _dir: dir,
        engine,
        session,
        wallet,
    }
}

/// Run by [`killed`] in a child process: a registration of [`PLAIN`] taken
/// to the point, then reported.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn registration_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    let (call, when) = point(&child.point);
    let f = fixture_at(kill9::scratch(&child), &child.root, true);
    let lib = f.fake();
    lib.hang_at(call, when);
    let draft = f.insert(core_row(PLAIN));
    f.spawn(&lib, draft, Some(f.lease()));
    kill9::eventually("the scripted call is reached", || {
        lib.hung.load(Ordering::SeqCst)
    });
    let report = serde_json::json!({ "draft": draft, "platform": platform_state(&lib) });
    kill9::reached(&child, &report.to_string());
}

/// R1 at `name`: killed, relaunched, run again to the end.
fn killed(name: &str) {
    let (dir, report) = kill9::killed_at(CHILD, ROW, name);
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    let root = dir.path().to_path_buf();
    let f = fixture_at(dir, &root, false);
    let draft = report["draft"].as_i64().unwrap();
    let killed_row = f.row(draft);
    if name == "fund_tracked" {
        // Restore rule 3: the outpoint went to the row before the broadcast.
        assert!(killed_row.asset_lock_outpoint.is_some());
        assert_eq!(phase(&killed_row), Phase::FundingSent);
    }
    let lib = f.fake();
    restore_platform(&lib, &report["platform"]);

    f.drive(&lib, draft, Some(f.lease()), true);

    let row = f.row(draft);
    assert_eq!((phase(&row), row.error.as_deref()), (Phase::Done, None));
    assert_eq!(
        (row.name_error.as_deref(), row.profile_error.as_deref()),
        (None, None)
    );
    let w = lib.w();
    assert_eq!((w.funds, w.identity_creates), (1, 1));
    assert_eq!(w.names.len(), 1);
    assert_eq!(w.profiles.len(), 1);
    assert_eq!(row.identity.as_deref(), Some(w.identities[0].1.as_str()));
    assert_eq!(f.rows().len(), 1, "one registration row");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r1_registration_killed_with_its_lock_tracked_resumes_to_one_registration() {
    killed("fund_tracked");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r1_registration_killed_after_its_identity_resumes_to_one_registration() {
    killed("register_identity_after");
}

#[test]
#[ignore = "H-03 kill -9 matrix (nightly): cargo test -p dw-engine --lib kill9 -- --ignored --skip kill9_finding"]
fn kill9_r1_registration_killed_after_its_profile_resumes_to_one_registration() {
    killed("create_profile_after");
}
