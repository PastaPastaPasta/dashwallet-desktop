//! The embedded data, the copy model, and source-level rules.

use std::collections::BTreeSet;

use super::*;
use crate::copy::{self, CantOpenCause};

#[test]
fn published_pins_give_the_accepted_sets() {
    assert_eq!(crate::fork::published_pin_count(), 17);
    let want: BTreeSet<BTreeSet<u32>> = [vec![1], vec![1, 2, 3]]
        .into_iter()
        .map(|v| v.into_iter().collect())
        .collect();
    assert_eq!(crate::fork::published_sets(), want);
}

#[test]
fn fork_sql_reproduces_the_recorded_checksums() {
    for (known, sql) in crate::fork::KNOWN.iter().zip([
        crate::fork::V1_SQL,
        crate::fork::V2_SQL,
        crate::fork::V3_SQL,
    ]) {
        let name = format!("V{}__{}", known.version, known.name);
        let migration = refinery::Migration::unapplied(&name, sql).unwrap();
        assert_eq!(migration.checksum().to_string(), known.checksum, "{name}");
    }
}

#[test]
fn probe_template_is_pinned_scrubbed_and_fork_shaped() {
    use sha2::Digest;
    let template = crate::adopters::PROBE_TEMPLATE;
    assert_eq!(
        crate::sql::hex(&sha2::Sha256::digest(template)),
        crate::adopters::PROBE_TEMPLATE_SHA256
    );
    let dir = private_tempdir();
    let path = dir.path().join("t.sqlite");
    fs::write(&path, template).unwrap();
    let conn = crate::sql::open_immutable(&path).unwrap();
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT count(*) FROM wallets"), 1);
    assert_eq!(count("SELECT count(*) FROM shielded_viewing_keys"), 0);
    let schema = |c: &Connection| -> Vec<(String, String, Option<String>)> {
        c.prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let f = fixture("v0.1.0");
    let fixture_conn = crate::sql::open_immutable(&f.wallet()).unwrap();
    assert_eq!(schema(&conn), schema(&fixture_conn));
}

#[test]
fn crate_source_never_calls_wallet_removal() {
    let needles: Vec<String> = ["remove", "unregister", "purge"]
        .iter()
        .map(|verb| [verb, "_wallet"].concat())
        .collect();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 10);
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for needle in &needles {
            assert!(
                !text.contains(needle.as_str()),
                "{} calls {needle}",
                file.display()
            );
        }
    }
}

#[test]
fn cant_open_renders_for_both_causes_with_and_without_a_backup() {
    let folder = "/home/u/.local/share/Dash Wallet/regtest";
    for cause in [
        CantOpenCause::RetiredPoolWithoutP9a,
        CantOpenCause::P9aBroken,
    ] {
        let with = copy::cant_open(cause, true);
        let shown = with.render(folder);
        assert!(shown.starts_with("Dash Wallet can't open this wallet data\n"));
        assert!(shown.contains(&format!(
            "the wallet data in {folder}. Nothing in it was changed, and"
        )));
        assert!(shown.contains(&format!("upgrade-backups folder inside {folder}.")));
        assert!(shown.ends_with("[Quit]"));
        assert_eq!(with.text_key, "storeUpgrade.cantOpen.text");
        let without = copy::cant_open(cause, false);
        assert_eq!(without.text_key, "storeUpgrade.cantOpen.textWithoutBackup");
        assert!(!without.render(folder).contains("upgrade-backups"));
        assert!(
            without
                .render(folder)
                .contains("Nothing in it was changed. Your funds")
        );
    }
    let err = crate::StoreUpgradeError::RetiredPoolWithoutAdopter { rows: 1 };
    assert_eq!(
        CantOpenCause::of(&err),
        Some(CantOpenCause::RetiredPoolWithoutP9a)
    );
    let err = crate::StoreUpgradeError::P9aBroken { detail: "x".into() };
    assert_eq!(
        copy::screen_for(&err, &crate::snapshot::SnapshotState::None),
        Some(copy::cant_open(CantOpenCause::P9aBroken, false))
    );
}

#[test]
fn backup_sentence_follows_snapshot_presence() {
    let f = fixture("v0.1.0");
    assert!(!crate::snapshot::snapshot_present(&f.net).unwrap());
    let snapshot = crate::snapshot::take_snapshot(&f.net, now()).unwrap();
    assert!(crate::snapshot::snapshot_present(&f.net).unwrap());
    fs::remove_file(snapshot.dir().join("wallet.sqlite")).unwrap();
    assert!(!crate::snapshot::snapshot_present(&f.net).unwrap());
}

#[test]
fn copy_names_the_folder_the_snapshot_writes() {
    assert_eq!(copy::COPY_FOLDER_NAME, "upgrade-backups");
    let text = copy::cant_open(CantOpenCause::P9aBroken, true).text;
    assert!(text.contains(&format!(
        "the {} folder inside {{folder}}",
        crate::snapshot::UPGRADE_BACKUPS_DIR
    )));
}

#[test]
fn opening_a_store_sets_the_process_gate() {
    crate::token::reset_opened_for_test();
    assert!(!crate::token::stores_opened_in_process());
    let f = fixture("v0.1.0");
    let refused = crate::token::PersisterOpened::open(config(&f.net.join("missing")));
    assert!(refused.is_err());
    assert!(crate::token::stores_opened_in_process());
    crate::token::reset_opened_for_test();
    crate::token::note_stores_opening();
    assert!(crate::token::stores_opened_in_process());
}
