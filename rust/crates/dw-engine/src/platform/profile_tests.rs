//! Tests of profiles.rs (DP4-01) on requests_tests.rs's fixture: a session
//! with no network and a scripted Platform ([`FakeRequests`]) holding the
//! `profile` documents. The contract's limits, the edit's checks, the
//! document a write creates or replaces (fields cleared, fields it does
//! not own carried), the refusals before a grant is redeemed, a failed
//! write's read-back, the reads of own and contacts' profiles, and the
//! contact-profile expiry the tap drives.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use dpp::platform_value::Value;
use platform_wallet::{ContactProfileEntry, DashPayProfile, ManagedIdentity};

use super::errors::{PlatformError, ProfileError};
use super::flows::{DOCUMENT_10K_CREDITS, FlowKind};
use super::journal_tests::id;
use super::private_details_tests::one_test_at_a_time;
use super::profile::{AvatarChange, Profile, ProfileEdit, ProfileField, ProfileLimits};
use super::profiles::{
    BUILT_IN_LIMITS, ProfileDocument, ProfileProperties, contract_limits, limits, merged,
    normalized, update,
};
use super::registration_flow::b58;
use super::requests::tests::{Fixture, wait_until};
use crate::{EngineEvent, PlatformChange};

const ME: u8 = 1;
/// A contact of ours.
const ERIN: u8 = 5;
/// Unknown to the wallet.
const DAVE: u8 = 4;

fn edit(display_name: Option<&str>, public_message: Option<&str>) -> ProfileEdit {
    ProfileEdit {
        display_name: display_name.map(str::to_owned),
        public_message: public_message.map(str::to_owned),
        avatar: AvatarChange::Keep,
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn properties<const N: usize>(fields: [(&str, Value); N]) -> ProfileProperties {
    fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// A published profile with an avatar and a v2 field the edit does not own.
fn full() -> ProfileProperties {
    properties([
        ("displayName", text("Me")),
        ("publicMessage", text("hello")),
        ("avatarUrl", text("https://example.org/me.png")),
        ("avatarHash", Value::Bytes32([1; 32])),
        ("avatarFingerprint", Value::Bytes(vec![2; 8])),
        (
            "corePaymentAddress",
            text("yP8A3cbdxRtLRduy5mXDsBnJtMzHWs6ZXr"),
        ),
    ])
}

fn fixture() -> Fixture {
    one_test_at_a_time();
    Fixture::new(u64::MAX / 2)
}

impl Fixture {
    fn update(&self, edit: ProfileEdit, grant: &str) -> Result<(), PlatformError> {
        let (net, wallet_id, grant) = (Arc::clone(&self.net), self.wallet, grant.to_owned());
        let dp = self.s.dashpay(self.wallet);
        self.engine
            .block_on(dp.on_wallet(move |s, _, w| async move {
                update(&*net, &s, &w, (wallet_id, &b58(&id(ME))), edit, grant).await
            }))
    }

    fn profile(&self, n: u8) -> Result<Option<Profile>, PlatformError> {
        self.s.dashpay(self.wallet).profile(b58(&id(n)))
    }

    /// Our `profile` document on Platform.
    fn document(&self) -> Option<ProfileDocument> {
        self.net.profiles.lock().unwrap().get(&id(ME)).cloned()
    }

    fn publish(&self, properties: ProfileProperties, revision: u64) {
        let doc = ProfileDocument {
            id: id(9),
            revision,
            properties,
        };
        self.net.profiles.lock().unwrap().insert(id(ME), doc);
    }

    fn profile_lease(&self) -> String {
        self.flow(
            FlowKind::ProfileEdit,
            vec![self.grant(DOCUMENT_10K_CREDITS)],
        )
    }

    fn identities_signals(&self) -> usize {
        (self.events.0.lock().unwrap().iter())
            .filter(|e| {
                matches!(e, EngineEvent::Platform { change, .. } if *change == PlatformChange::Identities)
            })
            .count()
    }

    /// Runs `f` on our identity as the library holds it.
    fn with_me<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut ManagedIdentity, &platform_wallet::WalletPersister) -> T + Send + 'static,
    ) -> T {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            f(
                (state.identity_manager.managed_identity_mut(&id(ME))).unwrap(),
                wallet.persister(),
            )
        })
    }

    fn checked_at(&self, contact: u8) -> u64 {
        self.with_me(move |m, _| m.dashpay().contact_profiles[&id(contact)].checked_at_ms)
    }
}

/// The limits are the bundled DashPay contract's: DIP-15's 25 and 140.
#[test]
fn the_limits_are_the_contracts() {
    let want = ProfileLimits {
        display_name_max: 25,
        public_message_max: 140,
    };
    assert_eq!(contract_limits(), Ok(want));
    assert_eq!((limits(), BUILT_IN_LIMITS), (want, want));
}

/// Texts are trimmed and an empty one cleared; a text past its limit, in
/// characters, is `profile.too_long` with the field, its length and the
/// limit.
#[test]
fn an_edit_is_trimmed_and_held_to_the_limits() {
    let limits = limits();
    let trimmed = normalized(edit(Some("  Al \n"), Some(" ")), limits).unwrap();
    assert_eq!(trimmed, edit(Some("Al"), None));

    let name = "é".repeat(25);
    let message = "ж".repeat(140);
    let at_limit = edit(Some(&name), Some(&message));
    assert_eq!(normalized(at_limit.clone(), limits), Ok(at_limit));

    let long = normalized(edit(Some(&format!("{name}x")), None), limits);
    assert_eq!(
        long,
        Err(ProfileError::TooLong {
            field: ProfileField::DisplayName,
            length: 26,
            max: 25
        })
    );
    let long = normalized(edit(None, Some(&format!(" {message}! "))), limits).unwrap_err();
    assert_eq!(
        (PlatformError::from(long.clone()).code(), long),
        (
            "profile.too_long",
            ProfileError::TooLong {
                field: ProfileField::PublicMessage,
                length: 141,
                max: 140
            }
        )
    );
}

/// The document an edit makes: its texts set or removed, the avatar kept
/// or removed whole, every other field carried.
#[test]
fn an_edit_replaces_only_what_it_owns() {
    let current = full();
    let kept = merged(Some(&current), &edit(None, Some("new")));
    let mut want = full();
    want.remove("displayName");
    want.insert("publicMessage".into(), text("new"));
    assert_eq!(kept, want);

    let removed = merged(
        Some(&current),
        &ProfileEdit {
            avatar: AvatarChange::Remove,
            ..edit(Some("Me"), None)
        },
    );
    assert_eq!(
        removed,
        properties([
            ("displayName", text("Me")),
            (
                "corePaymentAddress",
                text("yP8A3cbdxRtLRduy5mXDsBnJtMzHWs6ZXr")
            ),
        ])
    );
    assert_eq!(
        merged(None, &edit(Some("Al"), None)),
        properties([("displayName", text("Al"))])
    );
}

/// A first edit creates the document, signed with one document key under
/// the quote; the identity read shows it at once, with an `Identities`
/// signal. A later edit replaces the document at its next revision.
#[test]
fn an_edit_creates_then_replaces_the_document() {
    let f = fixture();
    assert_eq!(f.profile(ME), Ok(None));
    let lease = f.profile_lease();
    let signals = f.identities_signals();
    f.update(edit(Some(" Alice "), Some("hi")), &lease).unwrap();
    let doc = f.document().unwrap();
    assert_eq!(
        (doc.revision, &doc.properties),
        (
            1,
            &properties([
                ("displayName", text("Alice")),
                ("publicMessage", text("hi"))
            ])
        )
    );
    assert_eq!(f.credits_spent(&lease), DOCUMENT_10K_CREDITS);
    let signed = f.net.profile_keys.lock().unwrap().clone();
    assert!(matches!(signed[..], [1 | 2]), "a document key: {signed:?}");
    let shown = f.profile(ME).unwrap().unwrap();
    assert_eq!(
        (
            shown.display_name.as_deref(),
            shown.public_message.as_deref()
        ),
        (Some("Alice"), Some("hi"))
    );
    wait_until("the Identities signal", Duration::from_secs(10), || {
        f.identities_signals() > signals
    });

    f.update(edit(None, Some("bye")), &f.grant(DOCUMENT_10K_CREDITS))
        .unwrap();
    let doc = f.document().unwrap();
    assert_eq!(
        (doc.id, doc.revision, &doc.properties),
        (id(9), 2, &properties([("publicMessage", text("bye"))]))
    );
    let shown = f.profile(ME).unwrap().unwrap();
    assert_eq!(
        (shown.display_name, shown.public_message.as_deref()),
        (None, Some("bye"))
    );
    assert_eq!(f.net.writes(), 2);
}

/// An edit that changes nothing sends nothing and leaves the grant
/// unused; it still brings the cache to Platform's document. An empty
/// edit with no document is nothing too; one that would empty a document
/// is `profile.empty`.
#[test]
fn an_edit_that_changes_nothing_sends_nothing() {
    let f = fixture();
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    f.update(edit(None, Some("  ")), &grant).unwrap();
    assert!(f.unused(&grant));

    f.publish(full(), 3);
    f.update(edit(Some("Me"), Some("hello ")), &grant).unwrap();
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);
    let shown = f.profile(ME).unwrap().unwrap();
    assert_eq!(
        (shown.display_name.as_deref(), shown.avatar_url.as_deref()),
        (Some("Me"), Some("https://example.org/me.png"))
    );
    assert_eq!(shown.avatar_hash, Some(hex::encode([1; 32])));

    f.publish(properties([("displayName", text("Me"))]), 3);
    let emptied = f.update(edit(None, None), &grant);
    assert_eq!(emptied.unwrap_err().code(), "profile.empty");
    assert!(f.unused(&grant));
}

/// Each refusal comes before the grant is redeemed: a text too long, an
/// avatar candidate (DP4-02), Platform unreachable, too few credits.
#[test]
fn refusals_come_before_the_grant() {
    let f = fixture();
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let long = f.update(edit(Some(&"a".repeat(26)), None), &grant);
    assert_eq!(long.unwrap_err().code(), "profile.too_long");
    let set = ProfileEdit {
        avatar: AvatarChange::Set {
            candidate: "c1".into(),
        },
        ..edit(Some("Al"), None)
    };
    assert_eq!(
        f.update(set, &grant).unwrap_err().code(),
        "invalid_argument"
    );
    f.net.down.store(true, Ordering::SeqCst);
    assert_eq!(
        f.update(edit(Some("Al"), None), &grant).unwrap_err().code(),
        "platform.unavailable"
    );
    f.net.down.store(false, Ordering::SeqCst);
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);

    let poor = {
        drop(f);
        one_test_at_a_time();
        Fixture::new(DOCUMENT_10K_CREDITS - 1)
    };
    let grant = poor.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        poor.update(edit(Some("Al"), None), &grant)
            .unwrap_err()
            .code(),
        "platform.insufficient_credits"
    );
    assert!(poor.unused(&grant));
}

/// A write that signed nothing spends nothing; one that failed after
/// signing is decided by Platform's document: kept when it landed, the
/// error when it did not, the cache unchanged.
#[test]
fn a_failed_write_is_read_back() {
    let f = fixture();
    f.publish(full(), 3);
    let lease = f.profile_lease();
    *f.net.fails_unsigned.lock().unwrap() = Some(PlatformError::Unavailable);
    let failed = f.update(edit(Some("Al"), None), &lease);
    assert_eq!(failed.unwrap_err().code(), "platform.unavailable");
    assert_eq!(f.credits_spent(&lease), 0);
    *f.net.fails_unsigned.lock().unwrap() = None;

    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    let failed = f.update(edit(Some("Al"), None), &f.profile_lease());
    assert_eq!(failed.unwrap_err().code(), "platform.timeout");
    assert_eq!(f.document().unwrap().revision, 3);
    assert_eq!(f.profile(ME), Ok(None), "nothing cached");

    f.net.landed.store(true, Ordering::SeqCst);
    f.update(edit(Some("Al"), None), &f.profile_lease())
        .unwrap();
    assert_eq!(f.document().unwrap().revision, 4);
    let shown = f.profile(ME).unwrap().unwrap();
    assert_eq!(shown.display_name.as_deref(), Some("Al"));
}

/// Another user's profile is the contacts model's (the library's cache of
/// it); one the wallet knows nothing of is `None`; an id that is not one
/// is `platform.invalid_argument`.
#[test]
fn a_contacts_profile_is_the_cached_one() {
    let f = fixture();
    assert_eq!(f.profile(ERIN), Ok(None));
    f.with_me(|m, _| {
        let profile = DashPayProfile {
            display_name: Some("Erin".into()),
            public_message: Some("hey".into()),
            ..Default::default()
        };
        m.dashpay_contact_profiles_mut().insert(
            id(ERIN),
            ContactProfileEntry {
                profile: Some(profile),
                checked_at_ms: 0,
            },
        );
    });
    let shown = f.profile(ERIN).unwrap().unwrap();
    assert_eq!(
        (
            shown.display_name.as_deref(),
            shown.public_message.as_deref()
        ),
        (Some("Erin"), Some("hey"))
    );
    assert_eq!(f.profile(DAVE), Ok(None));
    let bad = f.s.dashpay(f.wallet).profile("not an id".into());
    assert_eq!(bad.unwrap_err().code(), "invalid_argument");
}

/// A store carrying contact profiles the library checked (its pass) has the
/// session expire them, in memory, so the next pass fetches them again.
#[test]
fn a_pass_that_checked_contact_profiles_expires_them() {
    let f = fixture();
    f.with_me(|m, persister| {
        m.dashpay_contact_profiles_mut().insert(
            id(ERIN),
            ContactProfileEntry {
                profile: None,
                checked_at_ms: 1_000,
            },
        );
        m.set_dashpay_profile(None, persister);
    });
    wait_until("the expiry", Duration::from_secs(10), || {
        f.checked_at(ERIN) == 0
    });
}
