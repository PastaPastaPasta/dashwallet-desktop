//! H-03 row R4, the avatar cache: a contact's picture killed with SIGKILL
//! in a child process once its thumbnails are renamed into place, before
//! the `dp_avatar` row naming them; after the relaunch no thumbnail is left
//! on disk without a row.

use super::super::avatar_cache::{AFTER_FILES, AvatarStore, stem};
use super::*;
use crate::kill9;

const ROW: &str = "avatar";
const CHILD: &str = "platform::avatar_tests::kill9::avatar_child";
const URL_SHA: &str = "aa";

/// The cache on `root` as a session opens it: the index, then the
/// directory (its leftovers swept).
fn store_at(root: &Path) -> AvatarStore {
    let dir = root.join("avatars");
    if !dir.exists() {
        std::fs::create_dir(&dir).unwrap();
        set_mode(&dir, 0o700);
    }
    let db = Arc::new(AppDb::open(&root.join("app.sqlite")).unwrap());
    AvatarStore::new(Some(ThumbDir::open(&dir).unwrap()), db)
}

/// Run by the row in a child process: a picture recorded up to its row.
#[test]
#[ignore = "a child process of the H-03 kill -9 matrix"]
fn avatar_child() {
    let Some(child) = kill9::child(ROW) else {
        return;
    };
    assert_eq!(child.point, "after_files");
    let store = store_at(&child.root);
    let ingested = avatar_image::ingest(&png(64, 64), None, false).unwrap();
    let stem = stem(&ingested.sha, None);
    let report = stem.clone();
    *AFTER_FILES.lock().unwrap() = Some(Box::new(move || kill9::reached(&child, &report)));
    store.record_ok(URL_SHA, &stem, &ingested, 1);
    panic!("the record ended before its kill point");
}

#[test]
#[ignore = "finding #358"]
fn kill9_finding_358_avatar_killed_between_its_files_and_row_leaves_no_orphan() {
    let (dir, stem) = kill9::killed_at(CHILD, ROW, "after_files");
    let store = store_at(dir.path());
    let row = store.row(URL_SHA);
    let thumbnails: Vec<String> = (std::fs::read_dir(dir.path().join("avatars")).unwrap())
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        thumbnails.is_empty() || row.and_then(|r| r.file) == Some(stem),
        "thumbnails no row names: {thumbnails:?}"
    );
}
