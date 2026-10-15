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

use dpp::data_contract::document_type::DocumentPropertyType;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::identity_public_key::v1::IdentityPublicKeyV1;
use dpp::identity::{IdentityPublicKey, KeyID};
use dpp::platform_value::Value;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::dashpay_contract;
use platform_wallet::{ContactProfileEntry, DashPayProfile, ManagedIdentity};

use super::errors::{PlatformError, ProfileError};
use super::flows::{DOCUMENT_10K_CREDITS, FlowKind};
use super::journal_tests::id;
use super::private_details_tests::one_test_at_a_time;
use super::profile::{
    AvatarChange, Profile, ProfileBaseline, ProfileEdit, ProfileField, ProfileLimits,
};
use super::profiles::{
    BUILT_IN_LIMITS, ProfileDocument, ProfileProperties, bundled_schema, contract_limits, limits,
    merged, normalized, update,
};
use super::registration_flow::b58;
use super::requests::tests::{Fixture, keys, wait_until};
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
        baseline: None,
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
        ("corePaymentAddress", Value::Bytes(vec![7; 21])),
    ])
}

fn texts(p: &Profile) -> (Option<&str>, Option<&str>) {
    (p.display_name.as_deref(), p.public_message.as_deref())
}

fn fixture() -> Fixture {
    one_test_at_a_time();
    Fixture::new(u64::MAX / 2)
}

/// `edit` made against `seen`, what `profile` showed.
fn against(edit: ProfileEdit, seen: Option<Profile>) -> ProfileEdit {
    ProfileEdit {
        baseline: Some(ProfileBaseline { profile: seen }),
        ..edit
    }
}

/// The V0 key under `key`.
fn v0(key: &IdentityPublicKey) -> dpp::identity::identity_public_key::v0::IdentityPublicKeyV0 {
    match key {
        IdentityPublicKey::V0(k) => k.clone(),
        IdentityPublicKey::V1(_) => unreachable!("the vault derives V0 keys"),
    }
}

fn bound(key: &IdentityPublicKey, bounds: ContractBounds) -> IdentityPublicKey {
    let mut k = v0(key);
    k.contract_bounds = Some(bounds);
    IdentityPublicKey::V0(k)
}

/// `key` with a budget or an expiry (ms).
fn limited(
    key: &IdentityPublicKey,
    budget: Option<u64>,
    expires_at: Option<u64>,
) -> IdentityPublicKey {
    IdentityPublicKey::V1(IdentityPublicKeyV1::from_v0_with_limits(
        v0(key),
        budget,
        expires_at,
    ))
}

fn to_dashpay(document: &str) -> ContractBounds {
    ContractBounds::SingleContractDocumentType {
        id: dashpay_contract::ID,
        document_type_name: document.into(),
    }
}

/// Far past and far future, in ms.
const EXPIRED: Option<u64> = Some(1);
const LATER: Option<u64> = Some(u64::MAX);

impl Fixture {
    fn update(&self, edit: ProfileEdit, grant: &str) -> Result<(), PlatformError> {
        self.update_as(ME, edit, grant)
    }

    fn update_as(&self, n: u8, edit: ProfileEdit, grant: &str) -> Result<(), PlatformError> {
        let (net, wallet_id, grant) = (Arc::clone(&self.net), self.wallet, grant.to_owned());
        let dp = self.s.dashpay(self.wallet);
        self.engine
            .block_on(dp.on_wallet(move |s, _, w| async move {
                update(&*net, &s, &w, (wallet_id, &b58(&id(n))), edit, grant).await
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
            ("corePaymentAddress", Value::Bytes(vec![7; 21])),
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
    assert_eq!(texts(&shown), (Some("Alice"), Some("hi")));
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
    assert_eq!(texts(&shown), (None, Some("bye")));
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

    // The cache is what the library's own fetch makes of the document: an
    // avatar URL it would not trust dropped, no profile without a field.
    let mut http = full();
    http.insert("avatarUrl".into(), text("http://example.org/me.png"));
    f.publish(http, 3);
    f.update(edit(Some("Me"), Some("hello")), &grant).unwrap();
    let shown = f.profile(ME).unwrap().unwrap();
    assert_eq!(
        (texts(&shown), shown.avatar_url.as_deref()),
        ((Some("Me"), Some("hello")), None)
    );
    let core_only = properties([("corePaymentAddress", Value::Bytes(vec![7; 21]))]);
    f.publish(core_only, 3);
    f.update(edit(None, None), &grant).unwrap();
    assert_eq!(f.profile(ME), Ok(None));
    assert!(f.unused(&grant));
}

/// Each refusal comes before the grant is redeemed: a text too long, an
/// avatar candidate (DP4-02), Platform unreachable, an identity the wallet
/// lacks, a grant below the quote, a lease of another flow, too few
/// credits.
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
    let unknown = f.update_as(DAVE, edit(Some("Al"), None), &grant);
    assert_eq!(unknown.unwrap_err().code(), "identity.not_found");
    let short = f.grant(DOCUMENT_10K_CREDITS - 1);
    let exceeded = f.update(edit(Some("Al"), None), &short);
    assert_eq!(exceeded.unwrap_err().code(), "platform.grant_exceeded");
    let other = f.flow(
        FlowKind::PrivateDetails,
        vec![f.grant(DOCUMENT_10K_CREDITS)],
    );
    let invalid = f.update(edit(Some("Al"), None), &other);
    assert_eq!(invalid.unwrap_err().code(), "platform.grant_invalid");
    assert!(f.unused(&grant) && f.unused(&short));
    assert_eq!(f.net.writes(), 0);

    drop(f);
    let poor = Fixture::new(DOCUMENT_10K_CREDITS - 1);
    let grant = poor.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        poor.update(edit(Some("Al"), None), &grant)
            .unwrap_err()
            .code(),
        "platform.insufficient_credits"
    );
    assert!(poor.unused(&grant));
}

/// Without the document key in the vault the edit is
/// `identity.keys_missing`, the grant unused.
#[test]
fn a_profile_without_a_held_key_keeps_the_grant() {
    one_test_at_a_time();
    let f = Fixture::foreign(keys(true), u64::MAX / 2);
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let refused = f.update(edit(Some("Al"), None), &grant);
    assert_eq!(refused.unwrap_err().code(), "identity.keys_missing");
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);
}

/// A write that signed nothing spends nothing; one that failed after
/// signing is decided by Platform's document: kept when it landed, the
/// error (the quote spent) when it did not, the cache unchanged.
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
    let lease = f.profile_lease();
    let failed = f.update(edit(Some("Al"), None), &lease);
    assert_eq!(failed.unwrap_err().code(), "platform.timeout");
    assert_eq!(f.credits_spent(&lease), DOCUMENT_10K_CREDITS);
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
/// is `invalid_argument`.
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
    assert_eq!(texts(&shown), (Some("Erin"), Some("hey")));
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

/// The profile is signed with a key that may sign it now, as the library
/// picks one (D22): a lower-id key bound to another contract or to another
/// DashPay document type, or expired, is passed over; a key with no limits
/// comes before one with a budget or an expiry; a key bound to DashPay or
/// to its profiles may sign.
#[test]
fn a_profile_signs_with_a_key_that_may_sign_it() {
    let elsewhere = ContractBounds::SingleContract {
        id: Identifier::from([0x0E; 32]),
    };
    type Adjust = fn(&IdentityPublicKey, &ContractBounds) -> IdentityPublicKey;
    let cases: [(&str, Adjust, KeyID); 7] = [
        ("bound elsewhere", |k, e| bound(k, e.clone()), 2),
        (
            "bound to contact requests",
            |k, _| bound(k, to_dashpay("contactRequest")),
            2,
        ),
        ("expired", |k, _| limited(k, None, EXPIRED), 2),
        ("expiring later", |k, _| limited(k, None, LATER), 2),
        ("with a budget", |k, _| limited(k, Some(1_000), None), 2),
        (
            "bound to profiles",
            |k, _| bound(k, to_dashpay("profile")),
            1,
        ),
        (
            "bound to DashPay",
            |k, _| {
                bound(
                    k,
                    ContractBounds::SingleContract {
                        id: dashpay_contract::ID,
                    },
                )
            },
            1,
        ),
    ];
    one_test_at_a_time();
    for (case, adjust, signs) in cases {
        let f = Fixture::with_adjusted_keys(u64::MAX / 2, |keys| {
            let first = adjust(&keys[&1], &elsewhere);
            keys.insert(1, first);
        });
        f.update(edit(Some("Al"), None), &f.profile_lease())
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(*f.net.profile_keys.lock().unwrap(), [signs], "{case}");
    }
    // Only a limited key: it signs.
    let f = Fixture::with_adjusted_keys(u64::MAX / 2, |keys| {
        let (first, second) = (limited(&keys[&1], None, LATER), bound(&keys[&2], elsewhere));
        keys.extend([(1, first), (2, second)]);
    });
    f.update(edit(Some("Al"), None), &f.profile_lease())
        .unwrap();
    assert_eq!(*f.net.profile_keys.lock().unwrap(), [1]);
}

/// With no key that may sign the profile the edit is
/// `identity.keys_missing` before the grant is redeemed.
#[test]
fn no_key_that_may_sign_keeps_the_grant() {
    one_test_at_a_time();
    let f = Fixture::with_adjusted_keys(u64::MAX / 2, |keys| {
        let first = bound(&keys[&1], to_dashpay("contactInfo"));
        let second = limited(&keys[&2], None, EXPIRED);
        keys.extend([(1, first), (2, second)]);
    });
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let refused = f.update(edit(Some("Al"), None), &grant);
    assert_eq!(refused.unwrap_err().code(), "identity.keys_missing");
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);
}

/// An edit made against a profile that has changed on Platform since is
/// `profile.conflict` with what `profile` now shows, before the grant is
/// redeemed, and the cache follows Platform (D23); made against the
/// current one, or with no baseline, it is written.
#[test]
fn an_edit_of_a_changed_profile_is_a_conflict() {
    let f = fixture();
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    // Nothing shown, but one published since.
    f.publish(full(), 3);
    let conflict = f.update(against(edit(Some("Al"), None), None), &grant);
    let shown = f.profile(ME).unwrap().unwrap();
    assert_eq!(
        texts(&shown),
        (Some("Me"), Some("hello")),
        "the cache follows"
    );
    assert_eq!(
        conflict,
        Err(ProfileError::Conflict {
            current: Some(Box::new(shown.clone())),
        }
        .into())
    );
    assert_eq!(conflict.unwrap_err().code(), "profile.conflict");
    // A stale one.
    let stale = Profile {
        display_name: Some("Old".into()),
        ..shown.clone()
    };
    let conflict = f.update(against(edit(Some("Al"), None), Some(stale)), &grant);
    assert_eq!(conflict.unwrap_err().code(), "profile.conflict");
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);

    // Against what is shown: written (`updated_at` is never compared).
    let seen = Profile {
        updated_at: Some(5),
        ..shown
    };
    f.update(against(edit(Some("Al"), None), Some(seen)), &grant)
        .unwrap();
    assert_eq!(f.document().unwrap().revision, 4);
    // Removed since: the conflict shows none.
    f.net.profiles.lock().unwrap().clear();
    let seen = f.profile(ME).unwrap();
    let conflict = f.update(
        against(edit(Some("Bo"), None), seen),
        &f.grant(DOCUMENT_10K_CREDITS),
    );
    assert_eq!(
        conflict,
        Err(ProfileError::Conflict { current: None }.into())
    );
    assert_eq!(f.profile(ME), Ok(None));
    // Without a baseline (dwcli's --force): written over whatever is there.
    f.publish(full(), 7);
    f.update(edit(Some("Cy"), None), &f.grant(DOCUMENT_10K_CREDITS))
        .unwrap();
    assert_eq!(f.document().unwrap().revision, 8);
    // An edit that matches Platform already sends nothing, whatever it was
    // made against.
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    f.update(against(edit(Some("Cy"), None), None), &grant)
        .unwrap();
    assert!(f.unused(&grant));
}

/// When Platform's contract has a profile property the bundled one lacks
/// or types otherwise, no profile is written (D21):
/// `profile.unsupported_contract` before the grant is redeemed. A property
/// only the bundled contract has is no loss.
#[test]
fn a_profile_platform_would_shape_otherwise_is_not_written() {
    let f = fixture();
    f.publish(full(), 3);
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let bundled = bundled_schema().clone();
    assert!(bundled.contains_key("displayName") && bundled.contains_key("avatarUrl"));
    let mut added = bundled.clone();
    added.insert("pronouns".into(), DocumentPropertyType::Boolean);
    let mut retyped = bundled.clone();
    retyped.insert("publicMessage".into(), DocumentPropertyType::Boolean);
    for live in [Some(added), Some(retyped), None] {
        *f.net.profile_schema.lock().unwrap() = Some(live);
        let refused = f.update(edit(Some("Al"), None), &grant);
        assert_eq!(refused.unwrap_err().code(), "profile.unsupported_contract");
    }
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);

    let mut fewer = bundled.clone();
    fewer.remove("avatarFingerprint");
    *f.net.profile_schema.lock().unwrap() = Some(Some(fewer));
    f.update(edit(Some("Al"), None), &grant).unwrap();
    assert_eq!(f.document().unwrap().revision, 4);
}
