//! BL-153: `DashPay::status`. The rules on [`derive_status`] alone, then a
//! real offline session: what each input on file does to the status, and
//! the `Platform{Startup}` signal of a changed bring-up outcome.

use std::collections::BTreeSet;
use std::sync::Arc;

use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{ContestWatch, NewRegistration, RegistrationRow};
use platform_wallet::DpnsNameInfo;
use zeroize::Zeroizing;

use super::banner::{MainIdentity, StatusInputs, derive_status};
use super::bringup::NO_IDENTITY_KEY;
use super::recovery_tests::engine;
use super::registration_rows::Funding;
use super::startup::{DashPayStatus, NoIdentityReason, StartupStatus};
use super::startup_status::DashPayStartup;
use crate::{
    DashNetwork, Engine, ImportOptions, NetworkSession, PlatformChange, SessionOptions, WalletId,
};

const ALICE: &str = "Alice111111111111111111111111111111111111111";
const BOB: &str = "Bob11111111111111111111111111111111111111111";

fn inputs() -> StatusInputs {
    StatusInputs {
        read_only: false,
        startup: StartupStatus::NotRun,
        startup_identity: None,
        main: None,
        live: true,
        registrations: Vec::new(),
        contests: Vec::new(),
        absence_on_file: false,
        caught_up: true,
    }
}

fn main_identity(identity: &str, names: &[&str], temporary: Option<&str>) -> Option<MainIdentity> {
    Some(MainIdentity {
        identity: identity.into(),
        names: names.iter().map(|n| (*n).into()).collect(),
        temporary: temporary.map(Into::into),
        // The library still lists the contest of `row`'s label.
        open_contests: vec!["Alice".into()],
    })
}

/// A readable row (Core-balance funding) at `phase`.
fn row(id: i64, phase: &str, identity: Option<&str>) -> RegistrationRow {
    RegistrationRow {
        id,
        identity_index: Some(0),
        identity: identity.map(Into::into),
        label: "alice".into(),
        temp_label: None,
        funding: Funding::CoreBalance { lock_duffs: 1_000 }.encode(),
        initial_profile: None,
        asset_lock_outpoint: None,
        phase: phase.into(),
        error: None,
        retryable: false,
        name_error: None,
        profile_error: None,
        owner: 0,
        created_at: 1,
        updated_at: 1,
    }
}

fn failed(mut r: RegistrationRow, retryable: bool) -> RegistrationRow {
    r.error = Some("registration.funding_insufficient".into());
    r.retryable = retryable;
    r
}

fn watch(identity: &str, label: &str, ends_at: u64, state: Option<&str>) -> ContestWatch {
    ContestWatch {
        identity: identity.into(),
        label: label.into(),
        ends_at,
        last_state: state.map(Into::into),
    }
}

fn no_identity(reason: Option<NoIdentityReason>) -> DashPayStatus {
    DashPayStatus::NoIdentity { reason }
}

fn registering(draft: i64) -> DashPayStatus {
    DashPayStatus::Registering {
        draft: draft.to_string(),
    }
}

fn ready(main: &str) -> DashPayStatus {
    DashPayStatus::Ready { main: main.into() }
}

fn contest(label: &str, ends_at: Option<u64>) -> DashPayStatus {
    DashPayStatus::ContestPending {
        identity: ALICE.into(),
        label: label.into(),
        ends_at,
    }
}

// ---- the rules ----

/// D2, D3, D7: without an identity, the bring-up's answer or a marker says
/// there is none; otherwise the bring-up has not settled.
#[test]
fn rules_without_an_identity_need_an_established_absence() {
    let status = |f: &dyn Fn(&mut StatusInputs)| {
        let mut i = inputs();
        f(&mut i);
        derive_status(&i)
    };
    for startup in [
        StartupStatus::NotRun,
        StartupStatus::Starting,
        StartupStatus::PartialNoIdentity,
        StartupStatus::DiscoveryFailed,
        StartupStatus::IdentityUnsettled,
    ] {
        assert_eq!(
            status(&|i| i.startup = startup),
            DashPayStatus::StartupIncomplete { startup }
        );
    }
    assert_eq!(
        status(&|i| i.startup = StartupStatus::NoIdentity),
        no_identity(None)
    );
    assert_eq!(status(&|i| i.absence_on_file = true), no_identity(None));
    assert_eq!(
        status(&|i| {
            i.absence_on_file = true;
            i.caught_up = false;
        }),
        no_identity(Some(NoIdentityReason::WaitingForSync))
    );
    assert_eq!(
        status(&|i| i.read_only = true),
        no_identity(Some(NoIdentityReason::WatchOnly))
    );
}

/// D5: without an identity, any flow short of `Done` that has not failed
/// for good is a registration in progress; the newest one is named.
#[test]
fn rules_a_flow_in_progress_is_registering_until_done_or_failed_for_good() {
    let status = |rows: Vec<RegistrationRow>| {
        derive_status(&StatusInputs {
            registrations: rows,
            startup: StartupStatus::NoIdentity,
            ..inputs()
        })
    };
    for phase in ["draft", "funding_sent", "name_requested", "contested"] {
        assert_eq!(status(vec![row(3, phase, None)]), registering(3), "{phase}");
    }
    assert_eq!(
        status(vec![failed(row(3, "draft", None), true)]),
        registering(3)
    );
    assert_eq!(
        status(vec![failed(row(3, "draft", None), false)]),
        no_identity(None)
    );
    assert_eq!(status(vec![row(3, "done", None)]), no_identity(None));
    // A row this build cannot read is a failure for good (DEC-171).
    let mut unreadable = row(3, "draft", None);
    unreadable.funding = "{\"v\":99}".into();
    assert_eq!(status(vec![unreadable]), no_identity(None));
    assert_eq!(
        status(vec![
            row(2, "draft", None),
            row(5, "funding_sent", None),
            row(9, "done", None)
        ]),
        registering(5)
    );
    // A watch-only wallet cannot have one; a registration outranks the
    // bring-up's unsettled outcome.
    assert_eq!(
        derive_status(&StatusInputs {
            registrations: vec![row(3, "draft", None)],
            ..inputs()
        }),
        registering(3)
    );
}

/// D2, D4, D5: an identity on file is the main one whatever the bring-up
/// said; only its own registration before the name step ends hides it.
#[test]
fn rules_an_identity_on_file_is_ready_unless_its_name_step_is_running() {
    let with = |main: Option<MainIdentity>, rows: Vec<RegistrationRow>| {
        derive_status(&StatusInputs {
            main,
            registrations: rows,
            startup: StartupStatus::PartialNoIdentity,
            ..inputs()
        })
    };
    assert_eq!(
        with(main_identity(ALICE, &["alice"], None), vec![]),
        ready(ALICE)
    );
    for phase in ["identity_registered", "name_requested"] {
        assert_eq!(
            with(
                main_identity(ALICE, &[], None),
                vec![row(4, phase, Some(ALICE))]
            ),
            registering(4),
            "{phase}"
        );
    }
    for phase in ["name_registered", "profile_created", "done"] {
        assert_eq!(
            with(
                main_identity(ALICE, &["alice"], None),
                vec![row(4, phase, Some(ALICE))]
            ),
            ready(ALICE),
            "{phase}"
        );
    }
    // A second identity's registration, or a flow failed for good.
    assert_eq!(
        with(
            main_identity(ALICE, &["alice"], None),
            vec![row(4, "draft", None)]
        ),
        ready(ALICE)
    );
    assert_eq!(
        with(
            main_identity(ALICE, &[], None),
            vec![failed(row(4, "name_requested", Some(ALICE)), false)]
        ),
        ready(ALICE)
    );
    assert_eq!(
        with(
            main_identity(ALICE, &[], None),
            vec![failed(row(4, "name_requested", Some(ALICE)), true)]
        ),
        registering(4)
    );
    // A flow adding a name to an identity it did not create, or one whose
    // name step failed: the identity is not registering.
    let mut existing = row(4, "name_requested", Some(ALICE));
    existing.funding = Funding::ExistingIdentity.encode();
    assert_eq!(
        with(main_identity(ALICE, &["bob"], None), vec![existing]),
        ready(ALICE)
    );
    let mut nameless = row(4, "identity_registered", Some(ALICE));
    nameless.name_error = Some("names.taken".into());
    assert_eq!(
        with(main_identity(ALICE, &[], None), vec![nameless]),
        ready(ALICE)
    );
    // A watch-only wallet with an identity on file still has it.
    assert_eq!(
        derive_status(&StatusInputs {
            read_only: true,
            main: main_identity(ALICE, &["alice"], None),
            ..inputs()
        }),
        ready(ALICE)
    );
}

/// D4: a snapshot read (a pass holds the library) proves no absence; while
/// it lists no identity, the bring-up's identity or the newest finished
/// flow's stands in, else the status waits.
#[test]
fn rules_a_snapshot_read_falls_back_to_identities_on_file() {
    let snapshot = |f: &dyn Fn(&mut StatusInputs)| {
        let mut i = StatusInputs {
            live: false,
            ..inputs()
        };
        f(&mut i);
        derive_status(&i)
    };
    assert_eq!(
        snapshot(&|i| i.startup_identity = Some(BOB.into())),
        ready(BOB)
    );
    assert_eq!(
        snapshot(
            &|i| i.registrations = vec![row(2, "done", Some(ALICE)), row(3, "done", Some(BOB))]
        ),
        ready(BOB)
    );
    assert_eq!(
        snapshot(&|i| {
            i.startup_identity = Some(BOB.into());
            i.registrations = vec![row(3, "done", Some(ALICE))];
        }),
        ready(BOB)
    );
    // Failed or unfinished flows name nothing; neither marker nor the
    // bring-up's absence counts on a snapshot.
    let not_done = [
        failed(row(3, "done", Some(ALICE)), false),
        row(4, "draft", None),
    ];
    assert_eq!(
        snapshot(&|i| {
            i.registrations = vec![not_done[0].clone()];
            i.absence_on_file = true;
        }),
        DashPayStatus::StartupIncomplete {
            startup: StartupStatus::NotRun
        }
    );
    assert_eq!(
        snapshot(&|i| i.startup = StartupStatus::NoIdentity),
        DashPayStatus::StartupIncomplete {
            startup: StartupStatus::NoIdentity
        }
    );
    assert_eq!(
        snapshot(&|i| i.registrations = vec![not_done[1].clone()]),
        registering(4)
    );
    // A name added to an existing identity, or a flow whose name step
    // failed, means an identity exists: not a registration.
    let mut existing = row(5, "name_requested", Some(ALICE));
    existing.funding = Funding::ExistingIdentity.encode();
    let mut nameless = row(6, "profile_created", Some(ALICE));
    nameless.name_error = Some("names.taken".into());
    for r in [existing, nameless] {
        assert_eq!(
            snapshot(&|i| i.registrations = vec![r.clone()]),
            DashPayStatus::StartupIncomplete {
                startup: StartupStatus::NotRun
            }
        );
    }
    // A live empty list does not take the bring-up's identity.
    assert_eq!(
        derive_status(&StatusInputs {
            startup_identity: Some(BOB.into()),
            ..inputs()
        }),
        DashPayStatus::StartupIncomplete {
            startup: StartupStatus::NotRun
        }
    );
}

/// D6: a contest is pending while the main identity shows no name but its
/// temporary one, on a live read.
#[test]
fn rules_a_contest_is_pending_until_the_identity_shows_a_name() {
    let with = |main: Option<MainIdentity>, contests: Vec<ContestWatch>, rows| {
        derive_status(&StatusInputs {
            main,
            contests,
            registrations: rows,
            ..inputs()
        })
    };
    let open = || vec![watch(ALICE, "alice", 1_700, Some("open"))];
    assert_eq!(
        with(main_identity(ALICE, &[], None), open(), vec![]),
        contest("alice", Some(1_700))
    );
    assert_eq!(
        with(
            main_identity(ALICE, &["alice007"], Some("alice007")),
            open(),
            vec![]
        ),
        contest("alice", Some(1_700))
    );
    // An end not read yet; a row not looked at yet; an unplaced one.
    for state in [None, Some("unplaced:5")] {
        assert_eq!(
            with(
                main_identity(ALICE, &[], None),
                vec![watch(ALICE, "alice", 0, state)],
                vec![]
            ),
            contest("alice", None)
        );
    }
    // Another name owned, a won contest, another identity's watch, a
    // snapshot read: Ready.
    assert_eq!(
        with(
            main_identity(ALICE, &["bob"], Some("alice007")),
            open(),
            vec![]
        ),
        ready(ALICE)
    );
    assert_eq!(
        with(
            main_identity(ALICE, &[], None),
            vec![watch(ALICE, "alice", 1_700, Some("won:1:2"))],
            vec![]
        ),
        ready(ALICE)
    );
    assert_eq!(
        with(
            main_identity(ALICE, &[], None),
            vec![watch(BOB, "alice", 1_700, None)],
            vec![]
        ),
        ready(ALICE)
    );
    assert_eq!(
        derive_status(&StatusInputs {
            main: main_identity(ALICE, &[], None),
            contests: open(),
            live: false,
            ..inputs()
        }),
        ready(ALICE)
    );
    // The registration's contested label outranks other watches; the
    // watch ending first otherwise.
    let watches = vec![
        watch(ALICE, "carol", 0, None),
        watch(ALICE, "dave", 2_000, None),
        watch(ALICE, "alice", 3_000, Some("open")),
    ];
    assert_eq!(
        with(main_identity(ALICE, &[], None), watches.clone(), vec![]),
        contest("dave", Some(2_000))
    );
    assert_eq!(
        with(
            main_identity(ALICE, &[], None),
            watches,
            vec![row(6, "contested", Some(ALICE))]
        ),
        contest("alice", Some(3_000))
    );
    // A contested registration before its watch row exists.
    assert_eq!(
        with(
            main_identity(ALICE, &[], None),
            vec![],
            vec![row(6, "contested", Some(ALICE))]
        ),
        contest("alice", None)
    );
    // D20: its contest ended (the library no longer lists it) while its
    // profile step is parked: no longer pending.
    let mut ended = main_identity(ALICE, &[], None);
    ended.as_mut().unwrap().open_contests.clear();
    assert_eq!(
        with(ended, vec![], vec![row(6, "contested", Some(ALICE))]),
        ready(ALICE)
    );
}

// ---- a real session ----

const ABANDON_12: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn open_offline(engine: &Engine) -> Arc<NetworkSession> {
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    s
}

fn status(s: &Arc<NetworkSession>, id: WalletId) -> DashPayStatus {
    s.dashpay(id).status().unwrap()
}

/// Adds an identity at index 0 to the wallet's library state, with `names`.
fn add_identity(engine: &Engine, s: &NetworkSession, id: WalletId, raw: [u8; 32], names: &[&str]) {
    engine.block_on(async {
        let manager = s.manager().unwrap();
        let wallet = manager.get_wallet(&id.0).await.unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        let identity = Identity::V0(IdentityV0 {
            id: raw.into(),
            public_keys: Default::default(),
            balance: 1_000,
            revision: 0,
        });
        (info.identity_manager)
            .add_identity(identity, 0, id.0, wallet.persister())
            .unwrap();
        let names = (names.iter())
            .map(|n| DpnsNameInfo {
                label: (*n).into(),
                acquired_at: Some(1),
            })
            .collect();
        (info
            .identity_manager
            .managed_identity_mut(&Identifier::from(raw)))
        .unwrap()
        .set_dpns_names(names, wallet.persister());
    });
}

/// D1, D3, D5, D6 against what the session has on file: a new wallet, a
/// restored one, the proven-absence marker, a registration row, an
/// identity, a contest watch, the temporary name and a watch-only wallet.
#[test]
fn a_session_reads_the_status_from_what_is_on_file() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let s = open_offline(&engine);
    let db = s.live().unwrap().appdb;

    // Created here: no identity, and Join waits for sync (SPV is stopped).
    let created = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    assert_eq!(
        status(&s, created),
        no_identity(Some(NoIdentityReason::WaitingForSync))
    );

    // A restore before any bring-up has not settled whether it has one,
    // until Platform's proof of none is on file.
    let restored = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let wallet = restored.to_string();
    assert_eq!(
        status(&s, restored),
        DashPayStatus::StartupIncomplete {
            startup: StartupStatus::NotRun
        }
    );
    let scope = dw_appdb::local_scope(&wallet);
    let now = crate::events::unix_now().to_string();
    db.set_setting(&scope, NO_IDENTITY_KEY, Some(&now)).unwrap();
    assert_eq!(
        status(&s, restored),
        no_identity(Some(NoIdentityReason::WaitingForSync))
    );
    // Past its 7-day TTL the proof no longer counts.
    db.set_setting(&scope, NO_IDENTITY_KEY, Some("1")).unwrap();
    assert_eq!(
        status(&s, restored),
        DashPayStatus::StartupIncomplete {
            startup: StartupStatus::NotRun
        }
    );

    // A registration in progress.
    let draft = db
        .insert_registration(
            &wallet,
            &NewRegistration {
                identity_index: Some(0),
                identity: None,
                label: "alice".into(),
                temp_label: Some("alice007".into()),
                funding: Funding::CoreBalance { lock_duffs: 1_000 }.encode(),
                initial_profile: None,
                phase: "funding_sent".into(),
            },
            1,
        )
        .unwrap();
    assert_eq!(status(&s, restored), registering(draft));

    // Its identity in the library: Ready (the row names no identity, so it
    // is another identity's flow as far as the status can tell).
    let raw = [7; 32];
    let alice = Identifier::from(raw).to_string(Encoding::Base58);
    add_identity(&engine, &s, restored, raw, &["alice007"]);
    assert_eq!(status(&s, restored), ready(&alice));

    // Its contest watched, the temporary name its only one.
    db.watch_contest(&wallet, &alice, "alice", 1_900_000_000)
        .unwrap();
    assert_eq!(
        status(&s, restored),
        ready(&alice),
        "alice007 is not temporary yet"
    );
    db.set_dp_pref(&wallet, &alice, "temporary_name", Some("alice007"))
        .unwrap();
    assert_eq!(
        status(&s, restored),
        DashPayStatus::ContestPending {
            identity: alice.clone(),
            label: "alice".into(),
            ends_at: Some(1_900_000_000),
        }
    );

    // A watch-only wallet.
    let xpub = engine.block_on(s.account_xpub(created, 0)).unwrap().xpub;
    let watch_only = engine
        .block_on(s.import_watch_only(xpub, Default::default()))
        .unwrap();
    assert_eq!(
        status(&s, watch_only),
        no_identity(Some(NoIdentityReason::WatchOnly))
    );

    // An unknown wallet.
    assert_eq!(
        s.dashpay(WalletId([9; 32])).status().unwrap_err().code(),
        "wallet_not_found"
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// D8: a changed bring-up outcome (or named identity) signals
/// `Platform{Startup}` for its wallet; a write that changes nothing does
/// not.
#[test]
fn startup_changes_are_signalled_once() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let s = open_offline(&engine);
    let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    *s.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
    let marks = || -> Vec<(WalletId, BTreeSet<PlatformChange>)> {
        std::mem::take(s.hub.pump.platform_marks.lock().unwrap().as_mut().unwrap())
    };
    let startup = BTreeSet::from([PlatformChange::Startup]);

    s.platform
        .record(id, DashPayStartup::new(StartupStatus::NoIdentity, false));
    assert_eq!(marks(), vec![(id, startup.clone())]);
    s.platform
        .record(id, DashPayStartup::new(StartupStatus::NoIdentity, false));
    assert_eq!(marks(), vec![], "nothing changed");
    let mut named = DashPayStartup::new(StartupStatus::NoIdentity, false);
    named.identity = Some(ALICE.into());
    s.platform.record(id, named);
    assert_eq!(marks(), vec![(id, startup.clone())], "the identity changed");
    s.platform.set_status(id, StartupStatus::Starting);
    assert_eq!(marks(), vec![(id, startup.clone())]);
    s.platform.write_startup().remove(&id);
    assert_eq!(marks(), vec![(id, startup)], "removed");
    engine.block_on(engine.shutdown()).unwrap();
}

/// A restored wallet (no identity, not created here) whose status rests
/// on a proof of no identity that holds `secs` more seconds.
fn restored_with_a_proof(engine: &Engine, s: &Arc<NetworkSession>, secs: u64) -> WalletId {
    let restored = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    prove_absence(s, restored, secs);
    restored
}

/// Platform's proof that `id` owns no identity, holding `secs` more seconds.
fn prove_absence(s: &NetworkSession, id: WalletId, secs: u64) -> u64 {
    let ttl = super::bringup::NO_IDENTITY_TTL.as_secs();
    let at = crate::events::unix_now() + secs - ttl;
    let scope = dw_appdb::local_scope(&id.to_string());
    let db = s.live().unwrap().appdb;
    db.set_setting(&scope, NO_IDENTITY_KEY, Some(&at.to_string()))
        .unwrap();
    at + ttl
}

/// The `Platform{Startup}` marks for `id` so far.
fn startup_marks(s: &NetworkSession, id: WalletId) -> usize {
    let marks = s.hub.pump.platform_marks.lock().unwrap();
    (marks.as_ref().unwrap().iter())
        .filter(|(w, c)| *w == id && c.contains(&PlatformChange::Startup))
        .count()
}

/// Waits up to 30 s for `n` marks for `id`.
fn wait_for_marks(s: &NetworkSession, id: WalletId, n: usize) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while startup_marks(s, id) < n && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    startup_marks(s, id)
}

/// D21 (Sol r1 finding 3), D23: a status that relied on Platform's proof of
/// no identity is signalled again when the proof expires, so a host showing
/// `NoIdentity` re-reads it, offline too: once per proof, and a fired timer
/// leaves its slot, so a renewed proof is timed again.
#[test]
fn the_expiry_of_a_proven_absence_is_signalled() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let s = open_offline(&engine);
    *s.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
    let restored = restored_with_a_proof(&engine, &s, 2);
    let waiting = no_identity(Some(NoIdentityReason::WaitingForSync));
    assert_eq!(status(&s, restored), waiting);
    status(&s, restored);
    assert_eq!(
        wait_for_marks(&s, restored, 1),
        1,
        "one signal for the two reads"
    );
    assert_eq!(
        status(&s, restored),
        DashPayStatus::StartupIncomplete {
            startup: StartupStatus::NotRun
        }
    );
    assert!(s.platform.absence_expiries.pending().is_empty());

    // A renewed proof: timed again; a newer one replaces it.
    prove_absence(&s, restored, 30);
    assert_eq!(status(&s, restored), waiting);
    let until = prove_absence(&s, restored, 2);
    assert_eq!(status(&s, restored), waiting);
    assert_eq!(
        s.platform.absence_expiries.pending(),
        [(restored, until)].into()
    );
    assert_eq!(wait_for_marks(&s, restored, 2), 2);
    assert!(s.platform.absence_expiries.pending().is_empty());
}

/// D23: only an answer the proof alone gives is timed (not a wallet created
/// here), and removing the wallet aborts its timer: no mark after the
/// deadline.
#[test]
fn a_removed_wallet_keeps_no_expiry_timer() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let s = open_offline(&engine);
    let created = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    prove_absence(&s, created, 60);
    assert_eq!(
        status(&s, created),
        no_identity(Some(NoIdentityReason::WaitingForSync))
    );
    assert!(
        s.platform.absence_expiries.pending().is_empty(),
        "created here"
    );

    let restored = restored_with_a_proof(&engine, &s, 2);
    status(&s, restored);
    assert!(
        s.platform
            .absence_expiries
            .pending()
            .contains_key(&restored)
    );
    let wipe = engine
        .block_on(s.vault_op(move |v| {
            v.authorize(
                dw_vault::GrantPurpose::Wipe,
                Some(&restored.0),
                dw_vault::Credential::None,
            )
        }))
        .unwrap()
        .id;
    engine.block_on(s.remove_wallet(restored, wipe)).unwrap();
    assert!(s.platform.absence_expiries.pending().is_empty());
    *s.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
    std::thread::sleep(std::time::Duration::from_secs(4));
    assert_eq!(startup_marks(&s, restored), 0);
}

/// D23: a timer holds the session's hub weakly, and closing the session
/// aborts it.
#[test]
fn a_closed_session_keeps_no_expiry_timer() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let s = open_offline(&engine);
    let restored = restored_with_a_proof(&engine, &s, 2);
    let hubs = Arc::strong_count(&s.hub);
    status(&s, restored);
    assert_eq!(s.platform.absence_expiries.pending().len(), 1);
    // Once the timer runs, waiting for its deadline.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(
        Arc::strong_count(&s.hub),
        hubs,
        "no strong hub in the timer"
    );
    engine.block_on(s.close());
    assert!(s.platform.absence_expiries.pending().is_empty());
    *s.hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
    std::thread::sleep(std::time::Duration::from_secs(4));
    assert_eq!(startup_marks(&s, restored), 0);
}
