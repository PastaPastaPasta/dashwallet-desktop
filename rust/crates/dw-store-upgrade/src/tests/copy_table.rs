//! MG's final refusal copy (`storeUpgrade.refused.*`, with the
//! damaged-store keys): every cause × every
//! proved state gives an allowed way out with its text assembled exactly,
//! "previous" only for a file proved not migrated (MG (A), (E)), and every
//! cause, made by the engine on a real fixture, leaves the data root
//! byte-identical (MG (B)).

use std::collections::BTreeSet;

use super::*;
use crate::copy::{
    BackupState, ProvedState, RefusedCause, RefusedScreen, ScreenCopy, WayOut, migrated,
    restore_screen_for, retry_copy_for, screen_for,
};
use crate::error::StoreUpgradeError;
use crate::precheck::{Precheck, precheck_locked_with, precheck_with};
use crate::restore::{IncompleteCause, RefusalCause};

const FOLDER: &str = "/home/u/.local/share/Dash Wallet/regtest";

/// MG's strings, verbatim from the final copy.
const TITLE: &str = "Dash Wallet can't open this wallet data safely";
const INTRO: &str = "This version of Dash Wallet can't safely open the wallet data in {folder}";
const UNCHANGED: &str = "Dash Wallet didn't change anything in it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.";
const CHANGED: &str = "Dash Wallet had already started upgrading it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.";
const KEEP: &str = "Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.";
const HINT: &str = "Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.";
const EARLIER: &str =
    "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded.";
const L9: &str = "If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.";

fn detail(cause: RefusedCause) -> (&'static str, &'static str) {
    use RefusedCause::*;
    match cause {
        ForkUnsupported => (
            "forkUnsupported",
            "it was written by a version of Dash Wallet that this one can't upgrade from.",
        ),
        ShapeMismatch => (
            "shapeMismatch",
            "its layout doesn't match the version it says it is, so another program or a test build may have changed it.",
        ),
        RehearsalFailed => (
            "rehearsalFailed",
            "a trial upgrade on a copy of it failed, so Dash Wallet left the real data as it was.",
        ),
        DevBuild => (
            "devBuild",
            "it was written by a development build of Dash Wallet.",
        ),
        TooNew => (
            "tooNew",
            "it was written by a newer version of Dash Wallet.",
        ),
        HistoryMismatch => (
            "historyMismatch",
            "its upgrade history doesn't match any released version of Dash Wallet, so this version can't tell what it contains.",
        ),
        ContactsTable => (
            "contactsTable",
            "it holds contact data that this version can't read yet.",
        ),
        SelfCheck => (
            "selfCheck",
            "a check Dash Wallet runs on itself before it opens any wallet data didn't pass.",
        ),
        Damaged => (
            "damaged",
            "it can't be read as wallet data, so it may be damaged.",
        ),
        Unexpected => ("unexpected", "it's in a state this version doesn't expect."),
    }
}

fn way_out(w: WayOut) -> (&'static str, String) {
    match w {
        WayOut::Previous => (
            "previous",
            format!(
                "Close this version and keep using, or reinstall, the version of Dash Wallet you used before. {L9}"
            ),
        ),
        WayOut::Newer => (
            "newer",
            "Install that version of Dash Wallet, or a later one, to open it. Don't open it with an earlier version.".into(),
        ),
        WayOut::Restore => (
            "restore",
            format!(
                "{EARLIER} To go back to your wallet data from before the upgrade, choose Restore earlier data. Otherwise, choose Report a problem."
            ),
        ),
        WayOut::Unverified => (
            "unverified",
            format!(
                "{EARLIER} A backup from before the upgrade exists, but it doesn't check out, so Dash Wallet won't restore from it. Choose Report a problem."
            ),
        ),
        WayOut::Support => ("support", "Choose Report a problem.".into()),
        WayOut::SupportMigrated => (
            "supportMigrated",
            format!("{EARLIER} Choose Report a problem."),
        ),
        WayOut::SelfCheckNotMigrated => (
            "selfCheckNotMigrated",
            format!(
                "Quit Dash Wallet and open it again. If you see this again, free up some disk space and try once more. If it still happens, install the latest version of Dash Wallet, or keep using the version you used before. {L9}"
            ),
        ),
        WayOut::SelfCheck => (
            "selfCheck",
            "Quit Dash Wallet and open it again. If you see this again, free up some disk space and try once more. If it still happens, install the latest version of Dash Wallet. Until then, don't open this wallet data with an earlier version.".into(),
        ),
        WayOut::Damaged => (
            "damaged",
            "To set up a wallet again from its recovery phrase, choose Set up from recovery phrase. Dash Wallet starts with a new data folder and leaves {folder} as it is, without deleting anything. Setting up again rebuilds the wallet from the network, so anything saved only in this wallet data, such as labels and notes, isn't brought across. If you don't have the recovery phrase, choose Report a problem.".into(),
        ),
    }
}

/// MG's cause → allowed ways out. A cause whose table entry is `previous`
/// alone also allows `support` / `supportMigrated` for a state not proved
/// unmigrated (MG (A)). A failed self check has its own two ways out
/// (Q98 98-3). A part-done upgrade is migrated whatever the state says.
fn allowed(cause: RefusedCause) -> BTreeSet<WayOut> {
    use RefusedCause::*;
    use WayOut::*;
    match cause {
        ForkUnsupported | RehearsalFailed => [Previous, Support, SupportMigrated].into(),
        ShapeMismatch | DevBuild | HistoryMismatch | Unexpected => {
            [Restore, Unverified, Support, SupportMigrated].into()
        }
        TooNew => [Newer].into(),
        ContactsTable => [Previous, Restore, Unverified, Support, SupportMigrated].into(),
        RefusedCause::SelfCheck => [WayOut::SelfCheck, WayOut::SelfCheckNotMigrated].into(),
        RefusedCause::Damaged => [WayOut::Damaged].into(),
    }
}

fn states() -> Vec<ProvedState> {
    let mut out = Vec::new();
    for migrated in [Some(false), Some(true), None] {
        for backup in [
            Some(BackupState::Verified),
            Some(BackupState::Unverified),
            Some(BackupState::None),
            None,
        ] {
            for unchanged in [Some(true), Some(false), None] {
                out.push(ProvedState {
                    migrated,
                    backup,
                    unchanged,
                });
            }
        }
    }
    out
}

fn labels(s: &RefusedScreen) -> Vec<(&'static str, &'static str)> {
    s.buttons.iter().map(|b| (b.key, b.label)).collect()
}

#[test]
fn every_cause_and_proved_state_gives_an_allowed_way_out_with_exact_text() {
    let mut reached: BTreeSet<(RefusedCause, WayOut)> = BTreeSet::new();
    for cause in RefusedCause::ALL {
        for state in states() {
            let s = RefusedScreen::new(cause, state);
            let w = s.way_out;
            let at = format!("{cause:?} {state:?} -> {w:?}");
            assert!(allowed(cause).contains(&w), "{at}");
            reached.insert((cause, w));

            match w {
                WayOut::Previous => assert_eq!(state.migrated, Some(false), "{at}"),
                WayOut::Newer => assert_eq!(cause, RefusedCause::TooNew, "{at}"),
                WayOut::Restore => assert_eq!(
                    (state.migrated, state.backup),
                    (Some(true), Some(BackupState::Verified)),
                    "{at}"
                ),
                WayOut::Unverified => assert_eq!(
                    (state.migrated, state.backup),
                    (Some(true), Some(BackupState::Unverified)),
                    "{at}"
                ),
                WayOut::SupportMigrated => assert_eq!(state.migrated, Some(true), "{at}"),
                WayOut::Support => assert_ne!(state.migrated, Some(true), "{at}"),
                WayOut::SelfCheck => {
                    assert_eq!(cause, RefusedCause::SelfCheck, "{at}");
                    assert_ne!(state.migrated, Some(false), "{at}");
                }
                WayOut::SelfCheckNotMigrated => assert_eq!(
                    (cause, state.migrated),
                    (RefusedCause::SelfCheck, Some(false)),
                    "{at}"
                ),
                WayOut::Damaged => assert_eq!(cause, RefusedCause::Damaged, "{at}"),
            }
            if allowed(cause).contains(&WayOut::Previous) && state.migrated == Some(false) {
                assert_eq!(w, WayOut::Previous, "{at}: the earlier version opens it");
            }
            if allowed(cause).contains(&WayOut::Restore)
                && state.migrated == Some(true)
                && state.backup == Some(BackupState::Verified)
            {
                assert_eq!(w, WayOut::Restore, "{at}");
            }

            let (detail_key, detail_text) = detail(cause);
            let (way_key, way_text) = way_out(w);
            assert_eq!(s.title.key, "storeUpgrade.refused.title");
            assert_eq!(s.intro.key, "storeUpgrade.refused.intro");
            let (unchanged_key, unchanged_text) = match state.unchanged {
                Some(true) => ("unchanged", UNCHANGED),
                Some(false) => ("changedByUpgrade", CHANGED),
                None => ("keep", KEEP),
            };
            assert_eq!(
                s.unchanged.key,
                format!("storeUpgrade.refused.{unchanged_key}")
            );
            assert_eq!(
                s.detail.key,
                format!("storeUpgrade.refused.detail.{detail_key}")
            );
            assert_eq!(
                s.way_out_line.key,
                format!("storeUpgrade.refused.wayOut.{way_key}")
            );
            let text = format!("{INTRO}: {detail_text} {unchanged_text} {way_text}")
                .replace("{folder}", FOLDER);
            assert_eq!(s.text(FOLDER), text, "{at}");

            let reports = !matches!(w, WayOut::Previous | WayOut::Newer);
            let mut want = Vec::new();
            if w == WayOut::Restore {
                want.push(("storeUpgrade.refused.restore", "Restore earlier data"));
            }
            if w == WayOut::Damaged {
                want.push((
                    "storeUpgrade.refused.recover",
                    "Set up from recovery phrase",
                ));
            }
            if reports {
                want.push(("storeUpgrade.refused.report", "Report a problem"));
            }
            want.push(("storeUpgrade.refused.quit", "Quit"));
            assert_eq!(labels(&s), want, "{at}");
            assert!(s.buttons.iter().all(|b| !b.destructive), "{at}");
            assert_eq!(
                s.hint.map(|h| (h.key, h.text)),
                reports.then_some(("settings.about.links.issuesHint", HINT)),
                "{at}"
            );

            let mut shown = format!("{TITLE}\n\n{text}\n");
            for (_, label) in &want {
                shown += &format!("[{label}]");
            }
            if reports {
                shown += &format!("\n{HINT}");
            }
            assert_eq!(s.render(FOLDER), shown, "{at}");
            assert!(!shown.contains("{folder}"));
            assert!(!shown.contains("held on the network"));
        }
    }
    let every: BTreeSet<(RefusedCause, WayOut)> = RefusedCause::ALL
        .into_iter()
        .flat_map(|c| allowed(c).into_iter().map(move |w| (c, w)))
        .collect();
    assert_eq!(reached, every, "every allowed way out is reachable");
}

#[test]
fn previous_carries_the_l9_sentence_verbatim() {
    assert!(WayOut::Previous.line().text.ends_with(L9));
    assert!(WayOut::SelfCheckNotMigrated.line().text.ends_with(L9));
    for w in WayOut::ALL {
        if w != WayOut::Previous {
            assert!(
                !w.line()
                    .text
                    .contains("version of Dash Wallet you used before")
            );
        }
    }
}

#[test]
fn errors_map_to_their_causes_and_retryable_ones_get_no_screen() {
    use RefusedCause::*;
    use StoreUpgradeError as E;
    let cases: Vec<(E, Option<RefusedCause>)> = vec![
        (
            E::ForkStoreUnsupported {
                versions: [1, 6].into(),
            },
            Some(ForkUnsupported),
        ),
        (
            E::ForkStoreShapeMismatch { object: "x".into() },
            Some(ShapeMismatch),
        ),
        (
            E::UpgradeRehearsalFailed { detail: "x".into() },
            Some(RehearsalFailed),
        ),
        (E::PreHistoryFork, Some(DevBuild)),
        (
            E::SchemaTooNew {
                found: 99,
                supported: 22,
            },
            Some(TooNew),
        ),
        (
            E::SchemaHistoryMismatch {
                version: 1,
                detail: "x".into(),
            },
            Some(HistoryMismatch),
        ),
        (
            E::RetiredPoolWithoutAdopter { rows: 1 },
            Some(ContactsTable),
        ),
        (E::P9aBroken { detail: "x".into() }, Some(SelfCheck)),
        (
            E::WalletFileUnsupported { detail: "x".into() },
            Some(Unexpected),
        ),
        (
            E::UpgradeCopyUnwritable { detail: "x".into() },
            Some(Unexpected),
        ),
        (
            E::SnapshotSourceDamaged {
                store: "x".into(),
                detail: "x".into(),
            },
            Some(Damaged),
        ),
        (
            E::RestoreRefused {
                cause: RefusalCause::MissingBackup,
            },
            None,
        ),
        (E::RestoreNoSpace { needed: None }, None),
        (
            E::RestoreIncomplete {
                cause: IncompleteCause::Other,
                journal_written: false,
                detail: String::new(),
            },
            None,
        ),
        (E::RestoreAfterOpen, None),
        (
            E::NonStrictOpen {
                policy: "Recovery".into(),
            },
            Some(Unexpected),
        ),
        (E::ProbeNotRun, Some(Unexpected)),
        (
            E::OpenConfigUnsupported { detail: "x".into() },
            Some(Unexpected),
        ),
        (
            E::Io {
                context: "x",
                unchanged: true,
                source: std::io::Error::other("disk full"),
            },
            None,
        ),
        (
            E::Io {
                context: "x",
                unchanged: false,
                source: std::io::Error::other("x"),
            },
            None,
        ),
        (
            E::RestorePending {
                dir: PathBuf::from("x"),
            },
            None,
        ),
        (
            E::Sqlite {
                context: "x",
                unchanged: false,
                source: rusqlite::Error::InvalidQuery,
            },
            None,
        ),
        (
            E::Sqlite {
                context: "x",
                unchanged: true,
                source: rusqlite::Error::InvalidQuery,
            },
            None,
        ),
        (
            E::StoreHeld {
                path: PathBuf::from("x"),
            },
            None,
        ),
        (E::SnapshotFailed { detail: "x".into() }, None),
    ];
    for (e, want) in cases {
        assert_eq!(RefusedCause::of(&e), want, "{e:?}");
        assert_eq!(
            screen_for(&e, ProvedState::default()).map(|s| s.cause),
            want,
            "{e:?}"
        );
        if want.is_some() {
            assert!(e.file_unchanged(), "{e:?}");
        }
    }
}

/// The refusal from the checks, the locked check when asked.
fn precheck_refusal(f: &Fixture, adopters: &crate::adopters::Adopters) -> StoreUpgradeError {
    let mut found = precheck_with(&f.net, adopters).unwrap();
    if matches!(found, Precheck::NeedsLockedCheck) {
        found = precheck_locked_with(&f.net, adopters).unwrap();
    }
    match found {
        Precheck::Refuse { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// `make` refuses on `f`; the data root is byte-identical across the refusal
/// and the screen's state reads, and the screen is `want`'s.
fn shows_unchanged(
    f: &Fixture,
    want: RefusedCause,
    make: impl FnOnce(&Fixture) -> StoreUpgradeError,
) -> RefusedScreen {
    let before = tree(&f.root);
    let err = make(f);
    let state = ProvedState::read(&f.net, false);
    let screen = screen_for(&err, state).unwrap_or_else(|| panic!("no screen for {err:?}"));
    assert_eq!(screen.cause, want, "{err:?}");
    assert!(err.file_unchanged(), "{err:?}");
    assert_eq!(
        tree_diff(&tree(&f.root), &before),
        Vec::<String>::new(),
        "{want:?} changed the data root"
    );
    screen
}

#[test]
fn every_cause_leaves_the_data_root_byte_identical() {
    use crate::adopters::P9aStatus;
    use RefusedCause::*;
    let mut seen = BTreeSet::new();
    let mut check = |version: &str,
                     sql: &str,
                     want: RefusedCause,
                     make: &dyn Fn(&Fixture) -> StoreUpgradeError| {
        let f = fixture(version);
        if !sql.is_empty() {
            exec(&f.wallet(), sql);
        }
        let s = shows_unchanged(&f, want, make);
        assert_eq!(
            s.unchanged.key, "storeUpgrade.refused.unchanged",
            "{want:?}"
        );
        if allowed(want).contains(&WayOut::Previous) {
            assert_eq!(s.way_out, WayOut::Previous, "{want:?}: a released file");
        }
        if want == SelfCheck {
            assert_eq!(s.way_out, WayOut::SelfCheckNotMigrated, "a released file");
        }
        seen.insert(want);
    };
    let pre = |f: &Fixture| precheck_refusal(f, build());
    check(
        "v0.1.0",
        "INSERT INTO dw_fork_schema_history VALUES (6, 'later', 'x', '1')",
        ForkUnsupported,
        &pre,
    );
    check(
        "v0.1.0",
        "ALTER TABLE core_address_pool_retired ADD COLUMN extra INTEGER",
        ShapeMismatch,
        &pre,
    );
    check(
        "v0.1.0",
        "INSERT INTO core_transactions (wallet_id, txid, finalized, record_blob) \
         SELECT wallet_id, randomblob(32), 0, x'00' FROM wallets LIMIT 1",
        RehearsalFailed,
        &pre,
    );
    check(
        "v0.1.0",
        "DROP TABLE dw_fork_schema_history; \
         INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (19, 'dashpay_external_rotation', 'x', '1')",
        DevBuild,
        &pre,
    );
    check(
        "v0.1.0",
        "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (99, 'future', 'x', '1')",
        TooNew,
        &pre,
    );
    check(
        "v0.1.0",
        "UPDATE refinery_schema_history SET checksum = '1' WHERE version = 1",
        HistoryMismatch,
        &pre,
    );
    check(
        "v0.3.0",
        "INSERT INTO core_address_pool_retired (wallet_id, account_type, account_index, \
           account_xpub, pool_type, address_index, script, used) \
         SELECT wallet_id, 'dashpay_external', 0, x'0488b21e', 2, 0, \
           x'76a914' || randomblob(20) || x'88ac', 1 FROM wallets LIMIT 1",
        ContactsTable,
        &|f| precheck_refusal(f, &build().with_p9a(P9aStatus::Absent)),
    );
    check("v0.3.0", "", SelfCheck, &|f| {
        let broken = build().with_p9a(P9aStatus::Broken {
            detail: "test".into(),
        });
        precheck_refusal(f, &broken)
    });
    check("v0.1.0", "", SelfCheck, &|f| {
        crate::token::PROBE_FOR_OPEN.with(|p| {
            *p.borrow_mut() = Some(Some(P9aStatus::Broken {
                detail: "test".into(),
            }))
        });
        let result = PersisterOpened::open(config(&f.net));
        crate::token::PROBE_FOR_OPEN.with(|p| *p.borrow_mut() = None);
        result.err().expect("refused on a Broken probe")
    });
    check("v0.1.0", "", Unexpected, &|f| {
        PersisterOpened::open(config(&f.net).with_load_policy(LoadPolicy::Recovery))
            .err()
            .expect("Recovery is refused")
    });
    check("v0.1.0", "", Unexpected, &|f| {
        crate::token::PROBE_FOR_OPEN.with(|p| *p.borrow_mut() = Some(None));
        let result = PersisterOpened::open(config(&f.net));
        crate::token::PROBE_FOR_OPEN.with(|p| *p.borrow_mut() = None);
        result.err().expect("refused without the probe")
    });
    let f = fixture("v0.1.0");
    damage_header(&f, "dispatch.sqlite");
    let s = shows_unchanged(&f, Damaged, |f| {
        crate::prepare_with(&f.net, build(), now()).unwrap_err()
    });
    assert_eq!(s.unchanged.key, "storeUpgrade.refused.unchanged");
    assert_eq!(s.way_out, WayOut::Damaged);
    seen.insert(Damaged);
    assert_eq!(seen, RefusedCause::ALL.into_iter().collect());
}

/// "Didn't change anything" only for a file this start left alone that is
/// not part-upgraded; the state read itself writes nothing.
#[test]
fn the_unchanged_sentence_needs_a_file_proved_unchanged_by_this_start() {
    let read = |f: &Fixture, wrote| {
        let before = tree(&f.root);
        let state = ProvedState::read(&f.net, wrote);
        assert_eq!(tree(&f.root), before, "the read writes nothing");
        state
    };
    let shown = |state| {
        let e = StoreUpgradeError::P9aBroken { detail: "x".into() };
        screen_for(&e, state).unwrap().text(FOLDER)
    };
    let unchanged = UNCHANGED.replace("{folder}", FOLDER);
    let changed = CHANGED.replace("{folder}", FOLDER);

    let f = fixture("v0.1.0");
    let released = read(&f, false);
    assert_eq!(
        released,
        ProvedState {
            migrated: Some(false),
            backup: Some(BackupState::None),
            unchanged: Some(true),
        }
    );
    assert!(shown(released).contains(&unchanged));
    assert!(!shown(released).contains(&changed));
    let put_back = read(&f, true);
    assert_eq!(put_back.unchanged, None, "a restore this start put it back");
    assert!(shown(put_back).contains(&KEEP.replace("{folder}", FOLDER)));
    assert!(!shown(put_back).contains(&unchanged) && !shown(put_back).contains(&changed));

    let hidden = fixture("v0.1.0");
    exec(&hidden.wallet(), "PRAGMA wal_checkpoint(TRUNCATE)");
    super::review_fixes::hot_wal(&hidden, "UPDATE core_utxos SET spent = spent");
    let unproved = read(&hidden, false);
    assert_eq!(
        unproved.unchanged, None,
        "a log the immutable read can't see"
    );
    let neutral = shown(unproved);
    assert!(neutral.contains(&KEEP.replace("{folder}", FOLDER)));
    assert!(!neutral.contains(&unchanged) && !neutral.contains(&changed));

    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    assert!(matches!(
        crate::reconcile::reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        crate::reconcile::Outcome::Pending { .. }
    ));
    let part = read(&f, false);
    assert_eq!(
        part,
        ProvedState {
            migrated: Some(true),
            backup: Some(BackupState::Verified),
            unchanged: Some(false),
        },
        "part-upgraded by an earlier start"
    );
    assert!(shown(part).contains(&changed));
    assert!(!shown(part).contains(&unchanged));

    drop(open_and_load(&f.net));
    let done = read(&f, false);
    assert_eq!((done.migrated, done.unchanged), (Some(true), Some(true)));
    assert_eq!(read(&f, true).unchanged, Some(false));
}

#[test]
fn a_migrated_file_never_offers_the_earlier_version() {
    let f = fixture("v0.1.0");
    assert_eq!(migrated(&f.net), Some(false));
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    crate::reconcile::reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    drop(open_and_load(&f.net));
    assert_eq!(migrated(&f.net), Some(true));
    exec(
        &f.wallet(),
        "UPDATE refinery_schema_history SET checksum = '1' WHERE version = 1",
    );
    let s = shows_unchanged(&f, RefusedCause::HistoryMismatch, |f| {
        precheck_refusal(f, build())
    });
    assert_eq!(s.way_out, WayOut::Restore, "migrated, backup verified");
    assert!(
        !s.text(FOLDER)
            .contains("version of Dash Wallet you used before")
    );

    let contacts = RefusedScreen::new(
        RefusedCause::ContactsTable,
        ProvedState {
            migrated: migrated(&f.net),
            backup: Some(BackupState::None),
            unchanged: Some(true),
        },
    );
    assert_eq!(contacts.way_out, WayOut::SupportMigrated);

    let broken = build().with_p9a(crate::adopters::P9aStatus::Broken {
        detail: "test".into(),
    });
    let s = shows_unchanged(&f, RefusedCause::SelfCheck, |f| {
        precheck_refusal(f, &broken)
    });
    assert_eq!(
        s.way_out,
        WayOut::SelfCheck,
        "a migrated file, Broken probe"
    );
}

#[test]
fn a_failed_self_check_offers_the_earlier_version_only_for_a_file_proved_unupgraded() {
    let e = StoreUpgradeError::P9aBroken {
        detail: "disk full".into(),
    };
    for state in states() {
        let s = screen_for(&e, state).unwrap();
        assert_eq!(s.cause, RefusedCause::SelfCheck, "{state:?}");
        let (want, earlier) = match state.migrated {
            Some(false) => (WayOut::SelfCheckNotMigrated, true),
            Some(true) | None => (WayOut::SelfCheck, false),
        };
        assert_eq!(s.way_out, want, "{state:?}");
        assert_eq!(
            s.text(FOLDER)
                .contains("keep using the version you used before"),
            earlier,
            "{state:?}"
        );
        assert_eq!(s.text(FOLDER).ends_with(L9), earlier, "{state:?}");
        assert_eq!(
            labels(&s),
            [
                ("storeUpgrade.refused.report", "Report a problem"),
                ("storeUpgrade.refused.quit", "Quit"),
            ]
        );
        assert_eq!(
            s.hint.map(|h| h.key),
            Some("settings.about.links.issuesHint")
        );
    }
}

/// One value of every [`StoreUpgradeError`] variant (Q98 98-2). [`variant`]
/// matches with no wildcard arm, so a new variant fails to compile until it
/// has an arm there; the count in [`every_refusal_variant_has_a_screen`]
/// must then be raised with a value added here.
fn every_variant() -> Vec<StoreUpgradeError> {
    use StoreUpgradeError as E;
    vec![
        E::ForkStoreUnsupported {
            versions: [1, 6].into(),
        },
        E::ForkStoreShapeMismatch { object: "x".into() },
        E::RetiredPoolWithoutAdopter { rows: 1 },
        E::P9aBroken { detail: "x".into() },
        E::UpgradeRehearsalFailed { detail: "x".into() },
        E::PreHistoryFork,
        E::SchemaTooNew {
            found: 99,
            supported: 22,
        },
        E::SchemaHistoryMismatch {
            version: 1,
            detail: "x".into(),
        },
        E::SnapshotInvalid {
            dir: PathBuf::from("x"),
            detail: "x".into(),
        },
        E::SnapshotFailed { detail: "x".into() },
        E::SnapshotSourceDamaged {
            store: "x".into(),
            detail: "x".into(),
        },
        E::RestorePending {
            dir: PathBuf::from("x"),
        },
        E::StoreHeld {
            path: PathBuf::from("x"),
        },
        E::NonStrictOpen {
            policy: "Recovery".into(),
        },
        E::OpenConfigUnsupported { detail: "x".into() },
        E::ProbeNotRun,
        E::PersisterOpen {
            dir: PathBuf::from("x"),
            source: Box::new(platform_wallet_storage::WalletStorageError::AlreadyOpen {
                path: PathBuf::from("x"),
            }),
        },
        E::Sqlite {
            context: "x",
            unchanged: true,
            source: rusqlite::Error::InvalidQuery,
        },
        E::Io {
            context: "x",
            unchanged: true,
            source: std::io::Error::other("disk full"),
        },
        E::RestoreRefused {
            cause: RefusalCause::MissingBackup,
        },
        E::RestoreNoSpace { needed: Some(1) },
        E::RestoreIncomplete {
            cause: IncompleteCause::Other,
            journal_written: true,
            detail: String::new(),
        },
        E::RestoreAfterOpen,
        E::WalletFileUnsupported { detail: "x".into() },
        E::UpgradeCopyUnwritable { detail: "x".into() },
        E::InjectedFault("x"),
    ]
}

fn variant(e: &StoreUpgradeError) -> &'static str {
    use StoreUpgradeError as E;
    match e {
        E::ForkStoreUnsupported { .. } => "ForkStoreUnsupported",
        E::ForkStoreShapeMismatch { .. } => "ForkStoreShapeMismatch",
        E::RetiredPoolWithoutAdopter { .. } => "RetiredPoolWithoutAdopter",
        E::P9aBroken { .. } => "P9aBroken",
        E::UpgradeRehearsalFailed { .. } => "UpgradeRehearsalFailed",
        E::PreHistoryFork => "PreHistoryFork",
        E::SchemaTooNew { .. } => "SchemaTooNew",
        E::SchemaHistoryMismatch { .. } => "SchemaHistoryMismatch",
        E::SnapshotInvalid { .. } => "SnapshotInvalid",
        E::SnapshotFailed { .. } => "SnapshotFailed",
        E::SnapshotSourceDamaged { .. } => "SnapshotSourceDamaged",
        E::RestorePending { .. } => "RestorePending",
        E::StoreHeld { .. } => "StoreHeld",
        E::NonStrictOpen { .. } => "NonStrictOpen",
        E::OpenConfigUnsupported { .. } => "OpenConfigUnsupported",
        E::ProbeNotRun => "ProbeNotRun",
        E::PersisterOpen { .. } => "PersisterOpen",
        E::Sqlite { .. } => "Sqlite",
        E::Io { .. } => "Io",
        E::RestoreRefused { .. } => "RestoreRefused",
        E::RestoreNoSpace { .. } => "RestoreNoSpace",
        E::RestoreIncomplete { .. } => "RestoreIncomplete",
        E::RestoreAfterOpen => "RestoreAfterOpen",
        E::WalletFileUnsupported { .. } => "WalletFileUnsupported",
        E::UpgradeCopyUnwritable { .. } => "UpgradeCopyUnwritable",
        E::InjectedFault(_) => "InjectedFault",
    }
}

#[test]
fn every_refusal_variant_has_a_screen() {
    let all = every_variant();
    let names: BTreeSet<&str> = all.iter().map(variant).collect();
    assert_eq!(names.len(), all.len(), "each variant listed once");
    assert_eq!(names.len(), 26, "every variant in `variant` is listed");
    let mut refusals = 0;
    let mut unexpected = BTreeSet::new();
    for e in &all {
        let refused = screen_for(e, ProvedState::default());
        let restore = restore_screen_for(e);
        assert!(
            refused.is_none() || restore.is_none(),
            "{}: one screen at most",
            variant(e)
        );
        if e.is_refusal() {
            refusals += 1;
            let s = refused.unwrap_or_else(|| panic!("{} is a refusal with no screen", variant(e)));
            if s.cause == RefusedCause::Unexpected {
                unexpected.insert(variant(e));
            }
            assert_ne!(
                s.way_out,
                WayOut::Previous,
                "{}: unproved state",
                variant(e)
            );
        }
    }
    assert_eq!(refusals, 11);
    assert_eq!(
        unexpected,
        ["UpgradeCopyUnwritable", "WalletFileUnsupported"].into()
    );
    let damaged = all
        .iter()
        .find(|e| variant(e) == "SnapshotSourceDamaged")
        .unwrap();
    assert_eq!(
        screen_for(damaged, ProvedState::default()).map(|s| (s.cause, s.way_out)),
        Some((RefusedCause::Damaged, WayOut::Damaged))
    );
    for name in ["RestoreRefused", "RestoreNoSpace", "RestoreIncomplete"] {
        let e = all.iter().find(|e| variant(e) == name).unwrap();
        assert!(restore_screen_for(e).is_some(), "{name}");
    }
    for name in ["StoreHeld", "SnapshotFailed", "Sqlite", "Io"] {
        let e = all.iter().find(|e| variant(e) == name).unwrap();
        assert!(e.file_unchanged(), "{name}");
        assert!(!e.is_refusal(), "{name} is retried, not refused");
        assert!(screen_for(e, ProvedState::default()).is_none(), "{name}");
        assert!(restore_screen_for(e).is_none(), "{name}");
    }
}

#[test]
fn migrated_is_unproved_without_a_readable_history_or_with_a_hidden_log() {
    let empty = private_tempdir();
    assert_eq!(migrated(empty.path()), None, "no wallet.sqlite");
    let f = fixture("v0.1.0");
    exec(&f.wallet(), "PRAGMA wal_checkpoint(TRUNCATE)");
    super::review_fixes::hot_wal(&f, "UPDATE core_utxos SET spent = spent");
    let before = tree(&f.root);
    assert_eq!(migrated(&f.net), None, "a log the immutable read can't see");
    assert_eq!(tree(&f.root), before);
    let g = fixture("v0.1.0");
    exec(&g.wallet(), "DROP TABLE refinery_schema_history");
    assert_eq!(migrated(&g.net), None);
}

/// R1's drop committed, upstream not migrated yet: no released build opens
/// the file, so no refusal may send the user back to one.
#[test]
fn a_part_upgraded_file_never_offers_the_earlier_version() {
    use crate::adopters::{P4_NAME, P9aStatus};
    let part_upgraded = || {
        let f = fixture("v0.3.0");
        let snapshot = crate::prepare_with(&f.net, build(), now())
            .unwrap()
            .snapshot
            .unwrap();
        assert!(matches!(
            crate::reconcile::reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
            crate::reconcile::Outcome::Pending { .. }
        ));
        assert_eq!(migrated(&f.net), Some(true), "part-upgraded");
        f
    };

    let f = part_upgraded();
    let broken = build().with_p9a(P9aStatus::Broken {
        detail: "test".into(),
    });
    let s = shows_unchanged(&f, RefusedCause::SelfCheck, |f| {
        precheck_refusal(f, &broken)
    });
    assert_eq!(s.way_out, WayOut::SelfCheck);

    let f = part_upgraded();
    add_retired_row(&f.wallet());
    let absent = build().with_p9a(P9aStatus::Absent);
    let s = shows_unchanged(&f, RefusedCause::ContactsTable, |f| {
        precheck_refusal(f, &absent)
    });
    assert_eq!(
        s.way_out,
        WayOut::Unverified,
        "the snapshot lacks the row added since: not this data's backup"
    );

    let f = part_upgraded();
    let mut without_p4 = build().clone();
    without_p4.embedded.retain(|m| m.name != P4_NAME);
    let s = shows_unchanged(&f, RefusedCause::ForkUnsupported, |f| {
        precheck_refusal(f, &without_p4)
    });
    assert_eq!(s.way_out, WayOut::SupportMigrated);
}

/// MG's text for the start that stopped on the machine.
const RETRY_TITLE: &str = "Dash Wallet couldn't finish upgrading your wallet data";
const RETRY_TEXT: &str = "The disk is full, or Dash Wallet couldn't write to it. Free up some space, then quit Dash Wallet and open it again. Until then, don't open this wallet data with an earlier version.";

pub(super) fn assert_retry_copy(s: &ScreenCopy) {
    assert_eq!(
        (s.title_key, s.title),
        ("storeUpgrade.retry.noSpace.title", RETRY_TITLE)
    );
    assert_eq!(
        (s.text_key, s.text),
        ("storeUpgrade.retry.noSpace.text", RETRY_TEXT)
    );
    let labels: Vec<_> = s.buttons.iter().map(|b| (b.key, b.label)).collect();
    assert_eq!(
        labels,
        [
            ("storeUpgrade.refused.report", "Report a problem"),
            ("storeUpgrade.refused.quit", "Quit")
        ]
    );
    assert_eq!(
        s.hint,
        Some((
            "settings.about.links.issuesHint",
            crate::copy::ISSUES_HINT.text
        ))
    );
    assert!(
        !s.render(FOLDER)
            .contains("version of Dash Wallet you used before")
    );
}

#[test]
fn only_a_start_stopped_by_the_machine_gets_the_retry_screen() {
    use StoreUpgradeError as E;
    let sqlite = |code| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
    for e in every_variant() {
        let want = matches!(variant(&e), "Io" | "SnapshotFailed");
        assert_eq!(retry_copy_for(&e).is_some(), want, "{e:?}");
        if want {
            assert!(!e.is_refusal() && screen_for(&e, ProvedState::default()).is_none());
        }
    }
    for (code, want) in [
        (rusqlite::ffi::SQLITE_FULL, true),
        (rusqlite::ffi::SQLITE_IOERR, true),
        (rusqlite::ffi::SQLITE_CANTOPEN, true),
        (rusqlite::ffi::SQLITE_BUSY, false),
        (rusqlite::ffi::SQLITE_CORRUPT, false),
    ] {
        let e = E::Sqlite {
            context: "x",
            unchanged: true,
            source: sqlite(code),
        };
        assert_eq!(retry_copy_for(&e).is_some(), want, "{e:?}");
    }
    let held = E::StoreHeld {
        path: PathBuf::from("x"),
    };
    assert!(retry_copy_for(&held).is_none());
    use platform_wallet_storage::WalletStorageError as W;
    let opened = |source: W| E::PersisterOpen {
        dir: PathBuf::from("x"),
        source: Box::new(source),
    };
    let io = |kind| std::io::Error::from(kind);
    let short_blob = W::ConsensusCodec {
        source: dashcore::consensus::encode::Error::Io(io(std::io::ErrorKind::UnexpectedEof)),
    };
    for (e, want) in [
        (opened(W::Io(io(std::io::ErrorKind::StorageFull))), true),
        (opened(W::Sqlite(sqlite(rusqlite::ffi::SQLITE_FULL))), true),
        (opened(W::Io(io(std::io::ErrorKind::UnexpectedEof))), false),
        (opened(short_blob), false),
        (opened(disk_in_decode()), false),
        (opened(W::BlobDecode { reason: "x" }), false),
    ] {
        assert_eq!(retry_copy_for(&e).is_some(), want, "{e:?}");
    }
    for (e, want) in [
        (W::Io(io(std::io::ErrorKind::StorageFull)), true),
        (W::Io(io(std::io::ErrorKind::UnexpectedEof)), false),
        (disk_in_decode(), false),
    ] {
        assert_eq!(crate::precheck::storage_fault(&e), want, "{e:?}");
    }
    let e = E::Io {
        context: "x",
        unchanged: true,
        source: std::io::Error::other("disk full"),
    };
    assert_retry_copy(&retry_copy_for(&e).unwrap());
}

#[test]
fn the_notices_carry_mg_s_final_text() {
    use crate::reconcile::UpgradeNotice;
    assert_eq!(
        crate::copy::notice_line(UpgradeNotice::SnapshotDamaged),
        crate::copy::Line {
            key: "storeUpgrade.notice.snapshotDamaged",
            text: "Dash Wallet upgraded your wallet data. The copy it saved from before the upgrade doesn't check out, so Dash Wallet won't offer to restore it. Your wallet opened as usual. Don't delete anything in upgrade-backups: support may need it.",
        }
    );
    assert_eq!(
        crate::copy::notice_line(UpgradeNotice::SnapshotOther),
        crate::copy::Line {
            key: "storeUpgrade.notice.snapshotOther",
            text: "Dash Wallet upgraded your wallet data. The copy in upgrade-backups from before the upgrade doesn't match this wallet, so Dash Wallet won't offer to restore it. Your wallet opened as usual. Don't delete anything in upgrade-backups: support may need it.",
        }
    );
    assert!(crate::copy::PENDING_MG.is_empty());
}

/// A decode error stops the search for a machine fault, whatever its
/// sources say.
fn disk_in_decode() -> platform_wallet_storage::WalletStorageError {
    platform_wallet_storage::WalletStorageError::ConsensusCodec {
        source: dashcore::consensus::encode::Error::Io(std::io::Error::from(
            std::io::ErrorKind::StorageFull,
        )),
    }
}

/// Overwrites `name`'s header so SQLite reads it as no database.
pub(super) fn damage_header(f: &Fixture, name: &str) {
    let path = f.net.join(name);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..100].fill(0x5a);
    std::fs::write(&path, bytes).unwrap();
}

/// The damaged-store screen, whatever the proved state: Set up from
/// recovery phrase, Report a problem and Quit, never the earlier version
/// or a restore, and the recovery button on no other screen.
pub(super) fn assert_damaged_screen(e: &StoreUpgradeError, state: ProvedState) {
    let s = screen_for(e, state).unwrap_or_else(|| panic!("no screen for {e:?}"));
    let at = format!("{e:?} {state:?}");
    assert_eq!(
        (s.cause, s.way_out),
        (RefusedCause::Damaged, WayOut::Damaged),
        "{at}"
    );
    assert_eq!(
        s.buttons.iter().map(|b| b.key).collect::<Vec<_>>(),
        [
            "storeUpgrade.refused.recover",
            "storeUpgrade.refused.report",
            "storeUpgrade.refused.quit"
        ],
        "{at}"
    );
    assert_eq!(
        s.hint.map(|h| h.key),
        Some("settings.about.links.issuesHint"),
        "{at}"
    );
    let text = s.text(FOLDER);
    assert!(!text.contains(L9), "{at}");
    assert!(
        !text.contains("version of Dash Wallet you used before"),
        "{at}"
    );
    assert!(!text.contains("Restore earlier data"), "{at}");
    assert!(text.contains("recovery phrase"), "{at}");
}

#[test]
fn a_damaged_store_offers_recovery_and_report_never_the_earlier_version() {
    let e = StoreUpgradeError::SnapshotSourceDamaged {
        store: "dispatch.sqlite".into(),
        detail: "x".into(),
    };
    assert!(e.file_unchanged() && e.is_refusal());
    for state in states() {
        assert_damaged_screen(&e, state);
    }
    for cause in RefusedCause::ALL {
        for state in states() {
            let s = RefusedScreen::new(cause, state);
            let recovers = s
                .buttons
                .iter()
                .any(|b| b.key == "storeUpgrade.refused.recover");
            assert_eq!(
                recovers,
                cause == RefusedCause::Damaged,
                "{cause:?} {state:?}"
            );
            assert_eq!(
                s.way_out == WayOut::Damaged,
                cause == RefusedCause::Damaged,
                "{cause:?} {state:?}"
            );
        }
    }
}

/// No refusal screen carries placeholder copy: MG's final text is in for
/// every key, the damaged-store keys included.
#[test]
fn no_refusal_screen_carries_placeholder_copy() {
    let mut keys = BTreeSet::new();
    for cause in RefusedCause::ALL {
        for state in states() {
            let s = RefusedScreen::new(cause, state);
            for (key, text) in [s.title, s.intro, s.detail, s.unchanged, s.way_out_line]
                .into_iter()
                .map(|l| (l.key, l.text))
                .chain(s.buttons.iter().map(|b| (b.key, b.label)))
            {
                if text.starts_with(crate::copy::PLACEHOLDER) {
                    keys.insert(key);
                }
                assert_eq!(
                    text.contains(crate::copy::PLACEHOLDER.trim_end()),
                    text.starts_with(crate::copy::PLACEHOLDER),
                    "{key}"
                );
            }
        }
    }
    assert_eq!(keys, BTreeSet::<&str>::new());
}
