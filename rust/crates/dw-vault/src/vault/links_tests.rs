//! The link stash (DP5-01 D4–D19).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashcore::Network;

use super::*;
use crate::types::{Clock, UnlockScope, VaultConfig};

const PAY: &str = "dash:yMsgnH1xKGa85n4bq2imrZbG2KgrmGttAV?amount=1.25&label=PlantedLabel";
const INVITE: &str = "dashpay://invite?du=alice&assetlocktx=00ff&pk=cPlantedSecretKey&islock=00";
const USER: &str = "dashpay://user?id=US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx&username=bob";

struct StepClock(AtomicU64);

impl Clock for StepClock {
    fn now_secs(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    clock: Arc<StepClock>,
    os: Arc<crate::MemoryOsStore>,
}

impl Fixture {
    fn new() -> Self {
        let dir = dw_testutil::private_tempdir();
        Fixture {
            root: dir.path().to_path_buf(),
            _dir: dir,
            clock: Arc::new(StepClock(AtomicU64::new(1_000_000))),
            os: Arc::new(crate::MemoryOsStore::new()),
        }
    }

    /// Opens (again) the vault in the fixture's directory: a restart.
    fn open(&self) -> Vault {
        self.open_in(&self.root)
    }

    fn open_in(&self, root: &Path) -> Vault {
        self.open_with(root, self.os.clone())
    }

    fn open_with(&self, root: &Path, os_store: Arc<crate::MemoryOsStore>) -> Vault {
        let config = VaultConfig {
            kdf: crate::KdfPolicy::Fixed(crate::KdfParams::TEST),
            os_store,
            clock: self.clock.clone(),
            grant_ttl_secs: 120,
        };
        Vault::open(root.join("vault"), Network::Regtest, "regtest", config).unwrap()
    }

    fn advance(&self, secs: u64) {
        self.clock.0.fetch_add(secs, Ordering::SeqCst);
    }

    fn links_dir(&self) -> std::path::PathBuf {
        self.root.join("vault").join(LINKS_DIR)
    }

    /// Every byte the stash directory holds, as text.
    fn disk_text(&self) -> String {
        let mut all = String::new();
        if let Ok(read) = std::fs::read_dir(self.links_dir()) {
            for item in read {
                all.push_str(&String::from_utf8_lossy(
                    &std::fs::read(item.unwrap().path()).unwrap(),
                ));
            }
        }
        all
    }

    /// [`Self::disk_text`] of the regular files only.
    fn disk_text_files(&self) -> String {
        let mut all = String::new();
        for item in std::fs::read_dir(self.links_dir()).unwrap() {
            let path = item.unwrap().path();
            if std::fs::symlink_metadata(&path).unwrap().is_file() {
                all.push_str(&String::from_utf8_lossy(&std::fs::read(path).unwrap()));
            }
        }
        all
    }

    fn entry_files(&self) -> usize {
        std::fs::read_dir(self.links_dir())
            .map(|r| {
                r.filter(|i| {
                    i.as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".dwl")
                })
                .count()
            })
            .unwrap_or(0)
    }
}

fn take(v: &Vault, id: &str) -> Option<(StashedLinkKind, String)> {
    v.take_stashed_link(id, ALL_LINK_KINDS)
        .unwrap()
        .map(|(k, t)| (k, t.as_str().to_owned()))
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// D6: before a vault exists a link is a plaintext entry, owner-only; the
/// vault seals it when it is created (§2.9's move), under the same id.
#[test]
fn a_link_before_the_vault_is_sealed_when_the_vault_is_created() {
    let f = Fixture::new();
    let v = f.open();
    assert_eq!(v.lock_state(), LockState::NoVault);
    let meta = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    assert!(is_link_id(&meta.id));
    assert_eq!(meta.expires_at - meta.stashed_at, 30 * DAY);
    assert!(
        f.disk_text().contains("cPlantedSecretKey"),
        "plaintext before a vault"
    );
    #[cfg(unix)]
    {
        assert_eq!(mode(&f.links_dir()), 0o700);
        assert_eq!(mode(&f.links_dir().join(format!("{}.dwl", meta.id))), 0o600);
    }
    // A restart before onboarding keeps it.
    drop(v);
    let v = f.open();
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);

    v.create(Some(b"pass")).unwrap();
    let text = f.disk_text();
    assert!(
        !text.contains("cPlantedSecretKey"),
        "sealed once the vault exists"
    );
    assert!(!text.contains("dashpay"), "{text}");
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);
    // Locked after a restart: listed, not readable.
    drop(v);
    let v = f.open();
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);
    assert!(matches!(
        v.take_stashed_link(&meta.id, ALL_LINK_KINDS),
        Err(VaultError::Locked)
    ));
    assert_eq!(f.entry_files(), 1, "a locked take keeps the entry");
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert_eq!(
        take(&v, &meta.id),
        Some((StashedLinkKind::Invitation, INVITE.to_owned()))
    );
    // D10: once.
    assert_eq!(take(&v, &meta.id), None);
    assert_eq!(f.entry_files(), 0);
}

/// D5: a link that arrives while the vault is locked is sealed at once to
/// the stash key `stash.pub` names; only the unlocked vault opens it.
#[test]
fn a_link_while_locked_is_sealed_with_the_public_key() {
    let f = Fixture::new();
    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    v.lock();
    let meta = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    assert_eq!(meta.expires_at - meta.stashed_at, 7 * DAY);
    let text = f.disk_text();
    assert!(
        !text.contains("PlantedLabel") && !text.contains("yMsgnH1x"),
        "{text}"
    );
    assert!(matches!(
        v.take_stashed_link(&meta.id, ALL_LINK_KINDS),
        Err(VaultError::Locked)
    ));
    // A mixing-only unlock reads links: that signs nothing (D11).
    v.unlock(b"pass", UnlockScope::MixingOnly).unwrap();
    assert_eq!(
        take(&v, &meta.id),
        Some((StashedLinkKind::Payment, PAY.to_owned()))
    );
}

/// An unencrypted vault seals with the key from the OS store.
#[test]
fn an_unencrypted_vault_seals_at_once() {
    let f = Fixture::new();
    let v = f.open();
    v.create(None).unwrap();
    let meta = v.stash_link(StashedLinkKind::Contact, USER).unwrap();
    assert!(!f.disk_text().contains("username=bob"));
    drop(v);
    let v = f.open();
    assert_eq!(
        take(&v, &meta.id),
        Some((StashedLinkKind::Contact, USER.to_owned()))
    );
}

/// D9: the same link of the same kind keeps one entry, in every state.
#[test]
fn the_same_link_keeps_one_entry() {
    let f = Fixture::new();
    let v = f.open();
    let a = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    assert_eq!(
        v.stash_link(StashedLinkKind::Payment, PAY).unwrap(),
        a,
        "plaintext"
    );
    assert_eq!(
        v.stash_link(StashedLinkKind::Payment, &format!(" {PAY}\n"))
            .unwrap(),
        a,
        "trimmed"
    );
    assert_ne!(
        v.stash_link(StashedLinkKind::Contact, PAY).unwrap().id,
        a.id,
        "kind counts"
    );
    v.create(Some(b"pass")).unwrap();
    assert_eq!(
        v.stash_link(StashedLinkKind::Payment, PAY).unwrap(),
        a,
        "unlocked"
    );
    v.lock();
    let b = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    assert_eq!(
        v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap(),
        b,
        "locked, this process"
    );
    // Another process while locked cannot compare: a second entry, which
    // the next unlock collapses (the older one stays).
    drop(v);
    let v = f.open();
    f.advance(5);
    let c = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    assert_ne!(c.id, b.id);
    assert_eq!(v.stashed_links().unwrap().len(), 4);
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    let left: Vec<String> = v
        .stashed_links()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(left.len(), 3);
    assert!(left.contains(&b.id) && !left.contains(&c.id));
}

/// D8: entries expire by kind.
#[test]
fn entries_expire() {
    let f = Fixture::new();
    let v = f.open();
    let pay = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    let invite = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    f.advance(7 * DAY);
    assert_eq!(v.stashed_links().unwrap(), vec![invite.clone()]);
    assert_eq!(take(&v, &pay.id), None);
    f.advance(23 * DAY - 1);
    assert_eq!(v.stashed_links().unwrap(), vec![invite.clone()]);
    f.advance(1);
    assert_eq!(v.stashed_links().unwrap(), vec![]);
    assert_eq!(f.entry_files(), 0);
}

/// D7: at most 16 entries; a full stash drops the oldest non-invitation,
/// then the oldest invitation. Over-long and empty links are refused.
#[test]
fn a_full_stash_makes_room_and_keeps_invitations_longest() {
    let f = Fixture::new();
    let v = f.open();
    let first = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    let mut pays = Vec::new();
    for i in 1..MAX_STASHED_LINKS {
        f.advance(1);
        pays.push(
            v.stash_link(StashedLinkKind::Payment, &format!("{PAY}&n={i}"))
                .unwrap(),
        );
    }
    f.advance(1);
    v.stash_link(StashedLinkKind::Payment, &format!("{PAY}&n=last"))
        .unwrap();
    let ids: Vec<String> = v
        .stashed_links()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids.len(), MAX_STASHED_LINKS);
    assert!(ids.contains(&first.id), "the invitation stays");
    assert!(!ids.contains(&pays[0].id), "the oldest payment made room");
    // Only invitations: the oldest goes.
    for i in 0..MAX_STASHED_LINKS {
        f.advance(1);
        v.stash_link(StashedLinkKind::Invitation, &format!("{INVITE}&n={i}"))
            .unwrap();
    }
    let ids: Vec<String> = v
        .stashed_links()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids.len(), MAX_STASHED_LINKS);
    assert!(!ids.contains(&first.id));
    for bad in [
        String::new(),
        "  ".into(),
        "x".repeat(MAX_STASHED_LINK_BYTES + 1),
        "dash:y?label=a\nb".into(),
        "dash:y\u{7}".into(),
    ] {
        assert!(matches!(
            v.stash_link(StashedLinkKind::Payment, &bad),
            Err(VaultError::InvalidArgument(_))
        ));
    }
    v.stash_link(
        StashedLinkKind::Payment,
        &"x".repeat(MAX_STASHED_LINK_BYTES),
    )
    .unwrap();
}

/// D23: the reseal after an unlock opens no sealed entry: a tampered one
/// outlives it, and the next gated stash call deletes it.
#[test]
fn the_reseal_after_unlock_opens_nothing() {
    let f = Fixture::new();
    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    v.lock();
    let meta = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    let path = f.links_dir().join(format!("{}.dwl", meta.id));
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let ct = json["sealed"]["ct"].as_str().unwrap().to_owned();
    let flipped = if ct.starts_with('0') { "1" } else { "0" };
    json["sealed"]["ct"] = format!("{flipped}{}", &ct[1..]).into();
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert!(path.exists(), "not opened by the reseal");
    assert_eq!(v.stashed_links().unwrap(), vec![]);
    assert!(!path.exists());
}

/// The associated data binds the entry's fields: an edited time or kind,
/// or a flipped ciphertext bit, never opens; such an entry is deleted.
#[test]
fn a_tampered_entry_never_opens() {
    let f = Fixture::new();
    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    for edit in ["expires", "kind", "ct"] {
        let meta = v
            .stash_link(StashedLinkKind::Payment, &format!("{PAY}&{edit}"))
            .unwrap();
        let path = f.links_dir().join(format!("{}.dwl", meta.id));
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match edit {
            "expires" => json["expires_at"] = (meta.expires_at - 1).into(),
            "kind" => json["kind"] = "contact".into(),
            _ => {
                let ct = json["sealed"]["ct"].as_str().unwrap().to_owned();
                let flipped = if ct.starts_with('0') { "1" } else { "0" };
                json["sealed"]["ct"] = format!("{flipped}{}", &ct[1..]).into();
            }
        }
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(take(&v, &meta.id), None, "{edit}");
        assert!(!path.exists(), "{edit}: deleted");
    }
    // A file that is no entry is left alone; a malformed entry is deleted.
    std::fs::write(f.links_dir().join("notes.txt"), b"x").unwrap();
    let junk = f.links_dir().join(format!("{}.dwl", "a".repeat(32)));
    std::fs::write(&junk, b"{}").unwrap();
    assert_eq!(v.stashed_links().unwrap(), vec![]);
    assert!(f.links_dir().join("notes.txt").exists());
    assert!(!junk.exists());
}

/// D17: a link sealed under another vault's key never opens here and is
/// dropped at the next sync; D19: a swapped `stash.pub` is healed at unlock.
#[test]
fn another_vaults_key_is_dropped_and_a_swapped_public_key_is_healed() {
    let f = Fixture::new();
    let other_root = f.root.join("other");
    let other = f.open_in(&other_root);
    other.create(Some(b"pass")).unwrap();
    let foreign = other
        .stash_link(StashedLinkKind::Invitation, INVITE)
        .unwrap();

    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    let ours = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    std::fs::copy(
        other_root
            .join("vault/links")
            .join(format!("{}.dwl", foreign.id)),
        f.links_dir().join(format!("{}.dwl", foreign.id)),
    )
    .unwrap();
    assert_eq!(v.stashed_links().unwrap(), vec![ours.clone()]);

    // Swap stash.pub for the other vault's (as an attacker with write
    // access to the directory could), then stash while locked.
    v.lock();
    let mut pubfile: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.links_dir().join(PUB_FILE)).unwrap()).unwrap();
    let theirs: serde_json::Value = serde_json::from_slice(
        &std::fs::read(other_root.join("vault/links").join(PUB_FILE)).unwrap(),
    )
    .unwrap();
    pubfile["public"] = theirs["public"].clone();
    std::fs::write(
        f.links_dir().join(PUB_FILE),
        serde_json::to_vec(&pubfile).unwrap(),
    )
    .unwrap();
    let lost = v.stash_link(StashedLinkKind::Contact, USER).unwrap();
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert_eq!(
        v.stashed_links().unwrap(),
        vec![ours.clone()],
        "sealed to the wrong key"
    );
    let healed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.links_dir().join(PUB_FILE)).unwrap()).unwrap();
    assert_ne!(healed["public"], theirs["public"]);
    assert_eq!(take(&v, &lost.id), None);
    // A stash.pub of another vault id is not used at all: with no key to
    // seal to, the link is refused, never written in plaintext (D26).
    v.lock();
    let mut wrong_id = healed.clone();
    wrong_id["vault_id"] = "00".repeat(16).into();
    std::fs::write(
        f.links_dir().join(PUB_FILE),
        serde_json::to_vec(&wrong_id).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        v.stash_link(StashedLinkKind::Contact, USER),
        Err(VaultError::Locked)
    ));
    assert!(!f.disk_text().contains("username=bob"));
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert!(!f.disk_text().contains("username=bob"));
}

/// `forget` deletes once; a take or forget limited to other kinds leaves
/// the entry unopened (review r1 #6); ids are checked before any path is
/// built.
#[test]
fn forget_and_id_checks() {
    use StashedLinkKind::{Contact, Invitation, Payment};
    let f = Fixture::new();
    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    let meta = v.stash_link(Invitation, INVITE).unwrap();
    v.lock();
    assert!(matches!(
        v.take_stashed_link(&meta.id, &[Payment, Contact]),
        Err(VaultError::InvalidArgument(d)) if d == OTHER_KIND
    ));
    assert_eq!(
        v.forget_stashed_link(&meta.id, &[Payment]).unwrap(),
        Some(Invitation)
    );
    assert_eq!(f.entry_files(), 1, "another kind is left in place");
    // Forgetting works locked: nothing is opened (review r1 #7).
    assert_eq!(
        v.forget_stashed_link(&meta.id, ALL_LINK_KINDS).unwrap(),
        Some(Invitation)
    );
    assert_eq!(
        v.forget_stashed_link(&meta.id, ALL_LINK_KINDS).unwrap(),
        None
    );
    assert_eq!(f.entry_files(), 0);
    assert_eq!(take(&v, &"b".repeat(32)), None);
    for bad in [
        "../vault.dwv",
        "",
        "ABCDEF0123456789abcdef0123456789",
        "stash.pub",
    ] {
        assert!(matches!(
            v.forget_stashed_link(bad, ALL_LINK_KINDS),
            Err(VaultError::InvalidArgument(_))
        ));
        assert!(matches!(
            v.take_stashed_link(bad, ALL_LINK_KINDS),
            Err(VaultError::InvalidArgument(_))
        ));
    }
}

/// `destroy` removes the stash with everything in it, a stray directory
/// included; without a vault it removes the plaintext entries kept before
/// one (review r1 #4).
#[test]
fn destroy_removes_the_stash() {
    let f = Fixture::new();
    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    std::fs::create_dir(f.links_dir().join("stray")).unwrap();
    v.destroy(crate::Credential::Passphrase(b"pass")).unwrap();
    assert!(!f.links_dir().exists());
    assert_eq!(v.stashed_links().unwrap(), vec![]);

    v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    assert!(f.links_dir().exists());
    v.destroy(crate::Credential::None).unwrap();
    assert!(!f.links_dir().exists());
}

/// Makes the stash directory read-only, so no file can be written in it;
/// `false` where that does not hold (a root user), and the caller skips.
#[cfg(unix)]
fn read_only(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let probe = dir.join("probe");
    if std::fs::write(&probe, b"").is_ok() {
        std::fs::remove_file(&probe).unwrap();
        writable(dir);
        return false;
    }
    true
}

#[cfg(unix)]
fn writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Review r1 #3, Sol r1 H1 (D26): sealing a plaintext entry replaces it
/// atomically. A seal that fails leaves the plaintext entry whole (it used
/// to be zeroed first), and the vault created all the same reports it
/// (`unsealed_links`); the next call with the key seals it, and only once
/// the sealed entry is in place is the old file zeroed.
#[cfg(unix)]
#[test]
fn a_failed_seal_keeps_the_entry_and_is_reported_until_sealed() {
    let f = Fixture::new();
    let v = f.open();
    let meta = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    let entry = f.links_dir().join(format!("{}.dwl", meta.id));
    if !read_only(&f.links_dir()) {
        eprintln!("skipped: the stash directory stays writable (root)");
        return;
    }
    let status = v.create(Some(b"pass")).unwrap();
    assert_eq!(status.state, LockState::Unlocked);
    assert!(status.unsealed_links, "the failed seal is reported");
    assert!(v.status().unsealed_links);
    let kept = String::from_utf8(std::fs::read(&entry).unwrap()).unwrap();
    assert!(kept.contains("cPlantedSecretKey"), "still whole: {kept}");
    // Restarted and still locked: still reported, and with no key and no
    // `stash.pub` a new link is refused rather than kept in plaintext.
    writable(&f.links_dir());
    drop(v);
    let v = f.open();
    assert!(v.status().unsealed_links);
    assert!(matches!(
        v.stash_link(StashedLinkKind::Payment, PAY),
        Err(VaultError::Locked)
    ));
    assert_eq!(f.entry_files(), 1);

    let old = f.root.join("old-entry");
    std::fs::hard_link(&entry, &old).unwrap();
    let status = v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert!(!status.unsealed_links, "the unlock seals it");
    assert!(!v.status().unsealed_links);
    assert!(!f.disk_text().contains("cPlantedSecretKey"));
    let scrubbed = std::fs::read(&old).unwrap();
    assert!(!scrubbed.is_empty() && scrubbed.iter().all(|b| *b == 0));
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);
    assert_eq!(
        take(&v, &meta.id),
        Some((StashedLinkKind::Invitation, INVITE.to_owned()))
    );
}

/// Sol r1 H1 (D28): a temporary name taken by a directory (with files
/// in it) or a symbolic link no longer blocks the seal; it is cleared,
/// never followed.
#[test]
fn a_taken_temporary_name_is_cleared() {
    let f = Fixture::new();
    let v = f.open();
    let invite = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    // Listed oldest first: one second apart, so the order is fixed.
    f.advance(1);
    let pay = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    let dir = f.links_dir().join(format!("{}.tmp", invite.id));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("inner"), b"x").unwrap();
    let outside = f.root.join("outside");
    std::fs::write(&outside, b"keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, f.links_dir().join(format!("{}.tmp", pay.id))).unwrap();
    let status = v.create(Some(b"pass")).unwrap();
    assert!(!status.unsealed_links);
    let text = f.disk_text_files();
    assert!(!text.contains("cPlantedSecretKey") && !text.contains("PlantedLabel"));
    assert!(!dir.exists());
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    assert_eq!(v.stashed_links().unwrap(), vec![invite, pay]);
}

/// Sol r1 H1 (D26): plaintext beside a vault, as a crash between the
/// vault's commit and the seal leaves it, is sealed to `stash.pub`
/// without the key: when the vault is opened, and by a stash call while
/// locked.
#[test]
fn plaintext_beside_a_vault_is_sealed_with_the_public_key() {
    let f = Fixture::new();
    let other = Fixture::new();
    let plain = other.open();
    let left = plain
        .stash_link(StashedLinkKind::Invitation, INVITE)
        .unwrap();
    let left_file = other.links_dir().join(format!("{}.dwl", left.id));

    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    v.lock();
    std::fs::copy(&left_file, f.links_dir().join(format!("{}.dwl", left.id))).unwrap();
    assert!(f.disk_text().contains("cPlantedSecretKey"));
    drop(v);
    let v = f.open();
    assert!(
        !f.disk_text().contains("cPlantedSecretKey"),
        "sealed at open"
    );
    assert!(!v.status().unsealed_links);
    assert_eq!(v.lock_state(), LockState::Locked);

    // The same while the vault is open, by the next stash call.
    let again = other
        .open()
        .stash_link(StashedLinkKind::Contact, USER)
        .unwrap();
    std::fs::copy(
        other.links_dir().join(format!("{}.dwl", again.id)),
        f.links_dir().join(format!("{}.dwl", again.id)),
    )
    .unwrap();
    let pay = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    let text = f.disk_text();
    assert!(!text.contains("username=bob") && !text.contains("PlantedLabel"));
    assert!(!v.status().unsealed_links);
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert_eq!(
        take(&v, &left.id),
        Some((StashedLinkKind::Invitation, INVITE.to_owned()))
    );
    assert_eq!(
        take(&v, &again.id),
        Some((StashedLinkKind::Contact, USER.to_owned()))
    );
    assert_eq!(
        take(&v, &pay.id),
        Some((StashedLinkKind::Payment, PAY.to_owned()))
    );
}

/// Review (L2): a seal to `stash.pub` that cannot be written leaves the
/// entry plaintext, in memory (still listed, still taken whole) and on
/// disk, and reported; the next call that can write seals it.
#[cfg(unix)]
#[test]
fn a_failed_seal_to_the_public_key_keeps_the_entry() {
    let f = Fixture::new();
    let other = Fixture::new();
    let left = other
        .open()
        .stash_link(StashedLinkKind::Invitation, INVITE)
        .unwrap();
    let v = f.open();
    v.create(Some(b"pass")).unwrap();
    v.lock();
    let entry = f.links_dir().join(format!("{}.dwl", left.id));
    std::fs::copy(other.links_dir().join(format!("{}.dwl", left.id)), &entry).unwrap();
    if !read_only(&f.links_dir()) {
        eprintln!("skipped: the stash directory stays writable (root)");
        return;
    }
    drop(v);
    let v = f.open();
    assert!(v.status().unsealed_links);
    assert_eq!(v.stashed_links().unwrap(), vec![left.clone()]);
    assert!(v.status().unsealed_links);
    assert!(
        std::fs::read_to_string(&entry)
            .unwrap()
            .contains("cPlantedSecretKey")
    );
    writable(&f.links_dir());
    assert_eq!(v.stashed_links().unwrap(), vec![left.clone()]);
    assert!(!v.status().unsealed_links);
    assert!(!f.disk_text().contains("cPlantedSecretKey"));
    v.unlock(b"pass", UnlockScope::Full).unwrap();
    assert_eq!(
        take(&v, &left.id),
        Some((StashedLinkKind::Invitation, INVITE.to_owned()))
    );
}

/// Sol r1 (D29): opening the vault (the app's start) sweeps: a
/// plaintext entry past its time to live goes without any stash call, and
/// so does a crash's temporary file.
#[test]
fn opening_sweeps_expired_entries_and_leftovers() {
    let f = Fixture::new();
    let v = f.open();
    v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    drop(v);
    let tmp = f.links_dir().join(format!("{}.tmp", "e".repeat(32)));
    std::fs::write(&tmp, b"{\"plain\":\"cPlantedPartial\"}").unwrap();
    f.advance(7 * DAY);
    let v = f.open();
    assert!(!tmp.exists());
    assert_eq!(f.entry_files(), 1, "the payment link expired");
    assert!(!f.disk_text().contains("PlantedLabel"));
    drop(v);
    f.advance(23 * DAY);
    let _v = f.open();
    assert_eq!(f.entry_files(), 0, "the invitation expired");
}

/// Sol r1 M2 (D27): a huge entry or temporary file (sparse here, so the
/// test is cheap) is deleted without being read or zeroed: no allocation
/// of its length, no gigabytes of zeros written. A small malformed one is
/// still zeroed before it goes. `destroy` does the same.
#[cfg(unix)]
#[test]
fn an_oversized_file_is_deleted_unread() {
    use std::os::unix::fs::MetadataExt;
    const HUGE: u64 = 4 << 30;
    let f = Fixture::new();
    let v = f.open();
    v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    let keep = f.root.join("keep");
    std::fs::create_dir(&keep).unwrap();
    let mut huge = Vec::new();
    for name in [
        format!("{}.dwl", "c".repeat(32)),
        format!("{}.tmp", "d".repeat(32)),
    ] {
        let path = f.links_dir().join(&name);
        std::fs::File::create(&path).unwrap().set_len(HUGE).unwrap();
        std::fs::hard_link(&path, keep.join(&name)).unwrap();
        huge.push((path, keep.join(&name)));
    }
    let small = f.links_dir().join(format!("{}.dwl", "e".repeat(32)));
    std::fs::write(&small, b"not an entry: PlantedGarbage").unwrap();
    std::fs::hard_link(&small, keep.join("small")).unwrap();

    assert_eq!(v.stashed_links().unwrap().len(), 1);
    for (path, kept) in &huge {
        assert!(!path.exists());
        let meta = std::fs::metadata(kept).unwrap();
        assert_eq!(meta.len(), HUGE);
        assert_eq!(meta.blocks(), 0, "never written");
    }
    let zeroed = std::fs::read(keep.join("small")).unwrap();
    assert!(!zeroed.is_empty() && zeroed.iter().all(|b| *b == 0));

    let path = f.links_dir().join(format!("{}.dwl", "f".repeat(32)));
    std::fs::File::create(&path).unwrap().set_len(HUGE).unwrap();
    std::fs::hard_link(&path, keep.join("destroyed")).unwrap();
    v.destroy(crate::Credential::None).unwrap();
    assert!(!f.links_dir().exists());
    assert_eq!(
        std::fs::metadata(keep.join("destroyed")).unwrap().blocks(),
        0
    );
}

/// A directory with an entry's name is no entry: left alone, and the stash
/// works around it.
#[test]
fn a_directory_named_like_an_entry_is_skipped() {
    let f = Fixture::new();
    let v = f.open();
    let meta = v.stash_link(StashedLinkKind::Payment, PAY).unwrap();
    let dir = f.links_dir().join(format!("{}.dwl", "c".repeat(32)));
    std::fs::create_dir(&dir).unwrap();
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);
    v.create(Some(b"pass")).unwrap();
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);
    assert_eq!(take(&v, &"c".repeat(32)), None);
    assert!(dir.is_dir());
}

/// Review r1 #5: an unencrypted vault whose OS store fails still stashes,
/// sealed to `stash.pub`, and lists; reading waits for the key.
#[test]
fn an_unencrypted_vault_without_its_os_store_seals_to_the_public_key() {
    let f = Fixture::new();
    let v = f.open();
    v.create(None).unwrap();
    v.stashed_links().unwrap();
    drop(v);
    let v = f.open_with(&f.root, Arc::new(crate::MemoryOsStore::unavailable()));
    let meta = v.stash_link(StashedLinkKind::Contact, USER).unwrap();
    assert!(!f.disk_text().contains("username=bob"));
    assert_eq!(v.stashed_links().unwrap(), vec![meta.clone()]);
    assert!(matches!(
        v.take_stashed_link(&meta.id, ALL_LINK_KINDS),
        Err(VaultError::Locked)
    ));
    drop(v);
    let v = f.open();
    assert_eq!(
        take(&v, &meta.id),
        Some((StashedLinkKind::Contact, USER.to_owned()))
    );
}

/// Nothing a stash call returns or logs quotes the link.
#[test]
fn errors_and_debug_never_quote_a_link() {
    let f = Fixture::new();
    let v = f.open();
    let meta = v.stash_link(StashedLinkKind::Invitation, INVITE).unwrap();
    assert!(!format!("{meta:?}").contains("Planted"));
    let long = format!("{INVITE}{}", "x".repeat(MAX_STASHED_LINK_BYTES));
    let e = v
        .stash_link(StashedLinkKind::Invitation, &long)
        .unwrap_err();
    assert!(!format!("{e} {e:?}").contains("Planted"));
}
