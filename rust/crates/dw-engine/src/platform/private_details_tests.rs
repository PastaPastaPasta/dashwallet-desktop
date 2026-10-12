//! Tests of private_details.rs (DP2-04) on requests_tests.rs's fixture: a
//! session with no network, a scripted Platform ([`FakeRequests`]) and its
//! `contactInfo` documents ([`FakeInfos`]), sealed and opened with the
//! session vault's real DIP-15 keys. The deferral, the publish and its
//! digest, the refusals before a grant is redeemed, a failed write's
//! rollback or read-back, and the settle that gives a restore its details
//! back.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::prelude::Identifier;
use platform_wallet::wallet::identity::crypto::contact_info::{
    ContactInfoPrivateData, MAX_PLAINTEXT_LEN, encode_private_data,
};
use platform_wallet::{ContactCryptoProvider, EstablishedContact, ManagedIdentity};

use super::contacts::{PrivateDetails, PublishState};
use super::errors::{ContactError, PlatformError};
use super::flows::{BudgetPurpose, DOCUMENT_10K_CREDITS, FlowKind};
use super::identity_writes::identity_locks;
use super::journal_tests::{id, request};
use super::keys_policy::key_path;
use super::private_details::{BoxedFuture, InfoDocument, InfoSource, digest, publish_state, set};
use super::requests::tests::{Fixture, keys};
use super::runtime::guard;
use super::signers::VaultContactCrypto;

const ME: u8 = 1;
/// A second contact, made in [`two_contacts`].
const BOB: u8 = 2;
/// Not on Platform, no contact.
const DAVE: u8 = 4;
/// The fixture's contact.
const ERIN: u8 = 5;
/// The identity's encryption key (`keys(true)`): DIP-15's root.
const ROOT: u32 = 4;
/// The test sessions' network (`journal_tests::session`).
const NETWORK: key_wallet::Network = key_wallet::Network::Regtest;

/// The details as the library holds them with a contact.
pub(crate) fn sealed_data(d: &PrivateDetails) -> ContactInfoPrivateData {
    ContactInfoPrivateData {
        alias_name: d.alias.clone(),
        note: d.note.clone(),
        display_hidden: d.hidden,
        accepted_accounts: Vec::new(),
    }
}

/// An identity's `contactInfo` documents on Platform, by (owner, contact).
#[derive(Default)]
pub(crate) struct FakeInfos {
    docs: Mutex<BTreeMap<(Identifier, Identifier), InfoDocument>>,
    /// Reads fail: `platform.unavailable`.
    pub(crate) down: AtomicBool,
    pub(crate) reads: AtomicU32,
    clock: AtomicU64,
}

impl FakeInfos {
    /// Publishes `d` as `owner`'s (identity `index`) details of `contact`,
    /// as the library does: sealed under the identity's encryption key, the
    /// contact's existing document updated, else the next derivation index.
    pub(crate) async fn seal(
        &self,
        crypto: &VaultContactCrypto,
        (owner, index, contact): (Identifier, u32, Identifier),
        d: &PrivateDetails,
    ) {
        let derivation = {
            let docs = self.docs.lock().unwrap();
            match docs.get(&(owner, contact)) {
                Some(doc) => doc.derivation,
                None => docs.keys().filter(|(o, _)| *o == owner).count() as u32,
            }
        };
        let root = key_path(NETWORK, index, ROOT).unwrap();
        let plaintext = encode_private_data(&sealed_data(d));
        let sealed = crypto
            .contact_info_seal(
                &root,
                derivation,
                &contact.to_buffer(),
                &plaintext,
                &[7; 16],
            )
            .await
            .unwrap();
        self.put(
            owner,
            contact,
            InfoDocument {
                root: ROOT,
                derivation,
                enc_to_user_id: sealed.enc_to_user_id,
                private_data: sealed.private_data,
                updated_at: 0,
            },
        );
    }

    /// Stores `doc` as `owner`'s about `contact`, updated now.
    pub(crate) fn put(&self, owner: Identifier, contact: Identifier, mut doc: InfoDocument) {
        doc.updated_at = self.clock.fetch_add(1, Ordering::SeqCst);
        self.docs.lock().unwrap().insert((owner, contact), doc);
    }

    fn remove(&self, owner: Identifier, contact: Identifier) {
        self.docs.lock().unwrap().remove(&(owner, contact));
    }

    fn count(&self) -> usize {
        self.docs.lock().unwrap().len()
    }
}

impl InfoSource for FakeInfos {
    fn documents(
        &self,
        owner: Identifier,
    ) -> BoxedFuture<'_, Result<Vec<InfoDocument>, PlatformError>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.down.load(Ordering::SeqCst) {
                return Err(PlatformError::Unavailable);
            }
            let docs = self.docs.lock().unwrap();
            let mut mine: Vec<InfoDocument> = (docs.iter())
                .filter(|((o, _), _)| *o == owner)
                .map(|(_, doc)| doc.clone())
                .collect();
            mine.sort_by_key(|doc| doc.updated_at);
            Ok(mine)
        })
    }
}

fn details(alias: &str) -> PrivateDetails {
    PrivateDetails {
        alias: Some(alias.into()),
        note: Some(format!("note on {alias}")),
        hidden: false,
    }
}

/// The fixture with `infos` as the session's `contactInfo` source.
fn fixture(f: Fixture) -> Fixture {
    *guard(&f.s.platform.contacts.info_source) =
        Some(Arc::clone(&f.net.infos) as Arc<dyn InfoSource>);
    f
}

/// The fixture with Bob a contact too: two, so the details publish.
fn two_contacts(f: Fixture) -> Fixture {
    f.with_identity(|m| {
        m.apply_established_contact(EstablishedContact::new(
            id(BOB),
            request(ME, BOB, 5_000),
            request(BOB, ME, 6_000),
        ));
    });
    f
}

impl Fixture {
    /// Runs `edit` on our identity as the library holds it.
    fn with_identity(&self, edit: impl FnOnce(&mut ManagedIdentity) + Send + 'static) {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let m = (state.identity_manager.managed_identity_mut(&id(ME))).unwrap();
            edit(m);
        });
    }

    fn set(
        &self,
        contact: u8,
        d: PrivateDetails,
        grant: Option<String>,
    ) -> Result<PublishState, ContactError> {
        let pair = self.pair(contact);
        self.call(move |net, s, w| async move { set(&*net, &s, &w, pair, d, grant).await })
    }

    /// The contact's details as the library holds them.
    fn shown(&self, contact: u8) -> PrivateDetails {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let state = wallet.state().await;
            let m = (state.identity_manager.managed_identity(&id(ME))).unwrap();
            let c = &m.dashpay().established_contacts()[&id(contact)];
            PrivateDetails {
                alias: c.alias.clone(),
                note: c.note.clone(),
                hidden: c.is_hidden,
            }
        })
    }

    fn recorded(&self, contact: u8) -> Option<[u8; 32]> {
        let pair = self.pair(contact);
        let s = Arc::clone(&self.s);
        self.engine
            .block_on(async move { s.recorded_digest(&pair).await })
            .unwrap()
    }

    /// What the read model reports for the contact (`contact_detail`'s
    /// `publish_state`), with two contacts.
    fn state(&self, contact: u8) -> PublishState {
        publish_state(2, &self.shown(contact), self.recorded(contact))
    }

    /// Settles our identity's details as a bring-up does.
    fn settle(&self) -> bool {
        let (s, wallet) = (Arc::clone(&self.s), self.wallet);
        self.engine.block_on(async move {
            let crypto = s
                .prompt_free_crypto(wallet)
                .await
                .expect("an unlocked vault");
            let w = s.manager().unwrap().get_wallet(&wallet.0).await.unwrap();
            s.settle_contact_infos(&w, wallet, &crypto, None).await
        })
    }

    /// Publishes `d` for `contact` as another device of the wallet would.
    fn publish_elsewhere(&self, contact: u8, d: &PrivateDetails) {
        let (s, wallet, infos, d) = (
            Arc::clone(&self.s),
            self.wallet,
            Arc::clone(&self.net.infos),
            d.clone(),
        );
        self.engine.block_on(async move {
            let crypto = s.prompt_free_crypto(wallet).await.unwrap();
            infos.seal(&crypto, (id(ME), 0, id(contact)), &d).await;
        });
    }

    fn credits_spent(&self, lease: &str) -> u64 {
        let lease = self.s.lease_for(&self.wallet, lease).unwrap().unwrap();
        let view = lease.view().unwrap();
        (view.budgets.iter())
            .find(|b| b.purpose == BudgetPurpose::Credits)
            .unwrap()
            .spent
    }

    fn lease(&self) -> String {
        self.flow(
            FlowKind::PrivateDetails,
            vec![self.grant(DOCUMENT_10K_CREDITS)],
        )
    }
}

fn published() -> Fixture {
    two_contacts(fixture(Fixture::new(u64::MAX / 2)))
}

/// Below two contacts the details are saved with the contact and the
/// publish deferred: no write, the grant left unused.
#[test]
fn below_two_contacts_the_details_stay_local() {
    let f = fixture(Fixture::new(u64::MAX / 2));
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        f.set(ERIN, details("Erin"), Some(grant.clone())).unwrap(),
        PublishState::DeferredUntilTwoContacts
    );
    assert_eq!(f.shown(ERIN), details("Erin"));
    assert!(f.unused(&grant));
    assert_eq!(
        (f.net.writes(), f.net.infos.count(), f.recorded(ERIN)),
        (0, 0, None)
    );
    assert_eq!(
        f.set(ERIN, details("Erin2"), None).unwrap(),
        PublishState::DeferredUntilTwoContacts
    );
    assert_eq!(f.shown(ERIN), details("Erin2"));
}

/// Without a grant, details Platform holds nothing for are saved locally
/// and read `Local`; with one they publish, `Published` and recorded,
/// signed with a document key against the quote. The same details again
/// are `Published` without a write; blank text is none.
#[test]
fn a_grant_publishes_and_records_the_details() {
    let f = published();
    assert_eq!(
        f.set(ERIN, details("Erin"), None).unwrap(),
        PublishState::Local
    );
    assert_eq!((f.shown(ERIN), f.net.writes()), (details("Erin"), 0));

    let lease = f.lease();
    assert_eq!(
        f.set(ERIN, details("Erin"), Some(lease.clone())).unwrap(),
        PublishState::Published
    );
    assert_eq!(f.net.writes(), 1);
    assert_eq!(f.credits_spent(&lease), DOCUMENT_10K_CREDITS);
    assert_eq!(f.recorded(ERIN), Some(digest(&details("Erin"))));
    assert_eq!(f.state(ERIN), PublishState::Published);
    assert_eq!(
        f.state(BOB),
        PublishState::Local,
        "another contact's record"
    );

    let padded = PrivateDetails {
        alias: Some("  Erin ".into()),
        note: Some("note on Erin\n".into()),
        hidden: false,
    };
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        f.set(ERIN, padded, Some(grant.clone())).unwrap(),
        PublishState::Published
    );
    assert!(f.unused(&grant), "nothing to publish");
    assert_eq!(
        f.set(ERIN, details("Erin"), None).unwrap(),
        PublishState::Published
    );
    assert_eq!(f.net.writes(), 1);

    // What Platform holds opens to the details.
    assert!(f.settle());
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin"), PublishState::Published)
    );

    let blank = PrivateDetails {
        alias: Some(" ".into()),
        note: Some(String::new()),
        hidden: true,
    };
    assert_eq!(
        f.set(BOB, blank, Some(f.lease())).unwrap(),
        PublishState::Published
    );
    let none = PrivateDetails {
        hidden: true,
        ..PrivateDetails::default()
    };
    assert_eq!(f.shown(BOB), none);
}

/// Platform's details come back over a local-only edit on the library's
/// next pass: changing published details needs a grant, and the details
/// stay as they were.
#[test]
fn a_published_contact_needs_a_grant_to_change() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    let mut hide = details("Erin");
    hide.hidden = true;
    assert_eq!(
        Fixture::code(f.set(ERIN, hide.clone(), None)),
        "platform.needs_grant"
    );
    assert_eq!(f.shown(ERIN), details("Erin"));
    assert_eq!(
        f.set(ERIN, hide.clone(), Some(f.lease())).unwrap(),
        PublishState::Published
    );
    assert_eq!(f.shown(ERIN), hide);
    assert_eq!(f.net.writes(), 2);
}

/// A write that failed before signing sent nothing: the quote goes back
/// and the details return to what they were (the library saved the edit
/// first).
#[test]
fn a_write_that_signed_nothing_rolls_back() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    let lease = f.lease();
    *f.net.fails_unsigned.lock().unwrap() = Some(PlatformError::Unavailable);
    assert_eq!(
        Fixture::code(f.set(ERIN, details("Erin2"), Some(lease.clone()))),
        "platform.unavailable"
    );
    assert_eq!(f.credits_spent(&lease), 0);
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin"), PublishState::Published)
    );
}

/// A write that failed after signing is read back: the details Platform
/// holds decide. Not landed: the error, the details back as they were;
/// landed: `Published` and recorded.
#[test]
fn a_failed_write_is_read_back() {
    let f = published();
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    assert_eq!(
        Fixture::code(f.set(ERIN, details("Erin"), Some(f.lease()))),
        "platform.timeout"
    );
    assert_eq!(
        (f.shown(ERIN), f.recorded(ERIN)),
        (PrivateDetails::default(), None)
    );

    f.net.landed.store(true, Ordering::SeqCst);
    assert_eq!(
        f.set(ERIN, details("Erin"), Some(f.lease())).unwrap(),
        PublishState::Published
    );
    assert_eq!(f.state(ERIN), PublishState::Published);
}

/// When the read-back fails too, the edit stays, `Local`, with its error;
/// a later read that finds it on Platform makes it `Published` without
/// another write (Q5).
#[test]
fn an_unknown_write_is_published_by_a_later_read() {
    let f = published();
    f.net.landed.store(true, Ordering::SeqCst);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    f.net.infos.down.store(true, Ordering::SeqCst);
    assert_eq!(
        Fixture::code(f.set(ERIN, details("Erin"), Some(f.lease()))),
        "platform.timeout"
    );
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin"), PublishState::Local)
    );
    assert!(!f.settle(), "a failed read changes nothing");
    assert_eq!(f.state(ERIN), PublishState::Local);

    f.net.infos.down.store(false, Ordering::SeqCst);
    assert!(f.settle());
    assert_eq!(f.state(ERIN), PublishState::Published);
    assert_eq!(f.net.writes(), 1);
}

/// Refused before the grant is redeemed, with nothing written or saved:
/// details over DIP-15's plaintext cap, ourselves, no contact, an identity
/// without DashPay keys, one the wallet only watches, and a lease of
/// another flow.
#[test]
fn refusals_keep_the_grant() {
    let f = published();
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let refuse = |f: &Fixture, contact, d: PrivateDetails| {
        let code = Fixture::code(f.set(contact, d, Some(grant.clone())));
        assert!(f.unused(&grant), "{code}");
        code
    };
    let long = PrivateDetails {
        alias: Some("a".repeat(MAX_PLAINTEXT_LEN)),
        ..PrivateDetails::default()
    };
    match f.set(ERIN, long.clone(), Some(grant.clone())) {
        Err(ContactError::DetailsTooLong { bytes, max }) => {
            assert_eq!(max as usize, MAX_PLAINTEXT_LEN);
            assert!(bytes as usize > MAX_PLAINTEXT_LEN);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(refuse(&f, ERIN, long), "contact.details_too_long");
    assert_eq!(refuse(&f, ME, details("me")), "contact.self");
    assert_eq!(refuse(&f, DAVE, details("Dave")), "invalid_argument");
    assert_eq!(f.shown(ERIN), PrivateDetails::default());

    let profile = f.flow(FlowKind::ProfileEdit, vec![f.grant(DOCUMENT_10K_CREDITS)]);
    let code = Fixture::code(f.set(ERIN, details("Erin"), Some(profile)));
    assert_eq!(code, "platform.grant_invalid");

    let g = two_contacts(fixture(Fixture::with_keys(keys(false), u64::MAX / 2)));
    let grant = g.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        Fixture::code(g.set(ERIN, details("Erin"), Some(grant.clone()))),
        "contact.ineligible"
    );
    assert!(g.unused(&grant));

    f.with_identity(|m| m.identity_index = None);
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let code = Fixture::code(f.set(ERIN, details("Erin"), Some(grant.clone())));
    assert_eq!(code, "invalid_argument");
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes() + g.net.writes(), 0);
}

/// The settle (bring-up, unlock, a failed read-back's retry) makes the
/// wallet agree with Platform: its details applied and recorded, a record
/// Platform no longer backs forgotten, and a document that does not open
/// failing the read, changing nothing.
#[test]
fn the_settle_applies_what_platform_holds() {
    let f = published();
    f.publish_elsewhere(ERIN, &details("Erin"));
    f.publish_elsewhere(BOB, &details("Bob"));
    assert!(f.settle());
    assert_eq!(
        (f.shown(ERIN), f.shown(BOB)),
        (details("Erin"), details("Bob"))
    );
    assert_eq!(
        (f.state(ERIN), f.state(BOB)),
        (PublishState::Published, PublishState::Published)
    );

    // Another device's newer edit wins.
    f.publish_elsewhere(ERIN, &details("Erin2"));
    f.net.infos.remove(id(ME), id(BOB));
    assert!(f.settle());
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin2"), PublishState::Published)
    );
    assert_eq!(
        (f.shown(BOB), f.state(BOB)),
        (details("Bob"), PublishState::Local)
    );

    let foreign = InfoDocument {
        root: ROOT,
        derivation: 9,
        enc_to_user_id: [3; 32],
        private_data: vec![5; 48],
        updated_at: 0,
    };
    f.net.infos.put(id(ME), id(DAVE), foreign);
    f.publish_elsewhere(ERIN, &details("Erin3"));
    assert!(!f.settle());
    assert_eq!(f.shown(ERIN), details("Erin2"));
}

/// A write and a settle each wait for the identity's write lock.
#[test]
fn private_details_wait_for_the_identity_write_lock() {
    let f = published();
    let lease = f.lease();
    let locks = identity_locks(f.wallet, id(ME));
    let held = f.engine.block_on(locks.write.lock());
    let (net, dp, pair) = (Arc::clone(&f.net), f.s.dashpay(f.wallet), f.pair(ERIN));
    let setting = f.s.rt.spawn(async move {
        dp.on_wallet(move |s, _, w| async move {
            set(&*net, &s, &w, pair, details("Erin"), Some(lease)).await
        })
        .await
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.net.writes(), 0, "the write waits for the lock");
    assert_eq!(f.shown(ERIN), PrivateDetails::default());
    drop(held);
    assert_eq!(
        f.engine.block_on(setting).unwrap().unwrap(),
        PublishState::Published
    );
    assert_eq!(f.net.writes(), 1);
}

/// The details' digest covers every field.
#[test]
fn the_digest_covers_every_field() {
    let d = details("Erin");
    let variants = [
        PrivateDetails {
            alias: None,
            ..d.clone()
        },
        PrivateDetails {
            note: None,
            ..d.clone()
        },
        PrivateDetails {
            hidden: true,
            ..d.clone()
        },
    ];
    for v in variants {
        assert_ne!(digest(&v), digest(&d), "{v:?}");
    }
    assert_eq!(digest(&d), digest(&details("Erin")));
}

/// DP2-04's acceptance on the public testnet (T2; pending tDASH for D).
/// `DW_DP204_TESTNET_FIXTURE` names the fixture directory (DP2-02's:
/// `fixture.json`, `C.mnemonic`) with a wallet `D.mnemonic` whose identity
/// has DashPay keys. A and B stay read-only: the writer is C, whose only
/// contact is A, so D becomes its second (D asks, C accepts). C then
/// publishes details for A and D (charged ≤ quote); the same details again
/// send nothing; and C restored into a fresh engine shows them `Published`.
/// Reruns find the contact and the details there and only check the
/// restore. Prints the T2 numbers.
#[test]
#[ignore = "public testnet; set DW_DP204_TESTNET_FIXTURE"]
fn t2_testnet_private_details() {
    use dpp::platform_value::string_encoding::Encoding;

    use super::contacts::{Eligibility, RequestOutcome};
    use super::flows::CONTACT_REQUEST_CREDITS;
    use super::journal_tests::{grant_unused, platform_grant};
    use super::recovery_tests;
    use super::requests::tests::{passes, wait_until};
    use crate::{DashNetwork, Engine, ImportOptions, NetworkSession, SessionOptions, WalletId};
    use zeroize::Zeroizing;

    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP204_TESTNET_FIXTURE").expect("DW_DP204_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let a_id = meta["A"]["identity_id"].as_str().unwrap().to_owned();
    let ours = |who: &str| PrivateDetails {
        alias: Some(format!("dp204 {who}")),
        note: Some(format!("DP2-04 T2: C's note on {who}")),
        hidden: false,
    };

    let open = |dir: &std::path::Path| -> (Engine, Arc<NetworkSession>) {
        let engine = recovery_tests::engine(dir);
        let s = engine
            .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
            .unwrap();
        engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
        (engine, s)
    };
    let import = |engine: &Engine, s: &Arc<NetworkSession>, who: &str| -> WalletId {
        let phrase = Zeroizing::new(
            std::fs::read_to_string(fixture.join(format!("{who}.mnemonic"))).unwrap(),
        );
        let options = ImportOptions {
            birth_height: Some(birth),
            ..ImportOptions::default()
        };
        let words = Zeroizing::new(phrase.trim().as_bytes().to_vec());
        engine
            .block_on(s.import_wallet(words, Zeroizing::new(Vec::new()), options))
            .unwrap()
    };
    let identity_of = |dp: &super::DashPay, who: &str| {
        let mut found = String::new();
        wait_until(
            &format!("{who}'s identity"),
            Duration::from_secs(180),
            || {
                let ids = dp.identities().unwrap_or_default();
                ids.first().map(|i| found = i.identity.clone()).is_some()
            },
        );
        wait_until(
            &format!("a DashPay pass of {who}"),
            Duration::from_secs(180),
            || passes(dp).is_some(),
        );
        found
    };

    let dir = dw_testutil::private_tempdir();
    let (engine, s) = open(dir.path());
    let (c, d) = (import(&engine, &s, "C"), import(&engine, &s, "D"));
    engine.block_on(s.start_spv()).unwrap();
    let (dp_c, dp_d) = (s.dashpay(c), s.dashpay(d));
    let (c_id, d_id) = (identity_of(&dp_c, "C"), identity_of(&dp_d, "D"));
    eprintln!("C is {c_id}, D is {d_id}");
    let grant = |wallet: WalletId, max_credits: u64| platform_grant(&s, wallet, max_credits);
    let net = super::requests::SdkRequests(s.manager().unwrap().sdk_arc());
    let balance = |id: &str| {
        let id = Identifier::from_string(id, Encoding::Base58).unwrap();
        engine
            .block_on(super::requests::RequestNet::balance(&net, id))
            .unwrap()
            .unwrap()
    };
    let charged = |what: &str, quote: u64, before: u64, after: u64| {
        let charged = before.checked_sub(after).expect("the balance grew");
        eprintln!("T2 {what}: quote {quote} before {before} after {after} charged {charged}");
        assert!(
            charged <= quote,
            "{what} charged {charged} over its quote {quote}"
        );
        charged
    };
    let eligibility = |dp: &super::DashPay, me: &str, them: &str| {
        engine
            .block_on(dp.eligibility(me.into(), them.into()))
            .unwrap()
    };

    // 1. D becomes C's second contact.
    match eligibility(&dp_c, &c_id, &d_id) {
        Eligibility::AlreadyContact => eprintln!("T2 rerun: C and D are contacts"),
        Eligibility::Ok | Eligibility::PendingIncoming => {
            if eligibility(&dp_c, &c_id, &d_id) == Eligibility::Ok {
                let sent = engine.block_on(dp_d.send_request(
                    d_id.clone(),
                    c_id.clone(),
                    None,
                    grant(d, CONTACT_REQUEST_CREDITS),
                ));
                assert!(
                    matches!(sent, Ok(RequestOutcome::Pending { .. })),
                    "{sent:?}"
                );
                wait_until("D's request at C", Duration::from_secs(300), || {
                    eligibility(&dp_c, &c_id, &d_id) == Eligibility::PendingIncoming
                });
            }
            let accepted = engine.block_on(dp_c.accept_request(
                c_id.clone(),
                d_id.clone(),
                grant(c, CONTACT_REQUEST_CREDITS),
            ));
            assert!(
                matches!(accepted, Ok(RequestOutcome::Established { .. })),
                "{accepted:?}"
            );
        }
        other => panic!("C → D is {other:?}: finish or reset the fixture by hand"),
    }

    // 2. C publishes details for A and D; the same again sends nothing.
    for (contact, who) in [(&a_id, "A"), (&d_id, "D")] {
        let set = |g: String| {
            engine
                .block_on(dp_c.set_private_details(
                    c_id.clone(),
                    contact.clone(),
                    ours(who),
                    Some(g),
                ))
                .unwrap()
        };
        let before = balance(&c_id);
        let g = grant(c, DOCUMENT_10K_CREDITS);
        assert_eq!(set(g.clone()), PublishState::Published);
        if grant_unused(&s, c, &g) {
            eprintln!("T2 rerun: C's details of {who} already published");
        } else {
            charged(
                &format!("set_private_details {who}"),
                DOCUMENT_10K_CREDITS,
                before,
                balance(&c_id),
            );
        }
        let g = grant(c, DOCUMENT_10K_CREDITS);
        assert_eq!(set(g.clone()), PublishState::Published);
        assert!(
            grant_unused(&s, c, &g),
            "the same details again: nothing sent"
        );
    }
    engine.block_on(engine.shutdown()).unwrap();

    // 3. C restored into a fresh engine gets them back.
    let fresh = dw_testutil::private_tempdir();
    let (engine, s) = open(fresh.path());
    let c = import(&engine, &s, "C");
    engine.block_on(s.start_spv()).unwrap();
    let dp_c = s.dashpay(c);
    assert_eq!(identity_of(&dp_c, "restored C"), c_id);
    let started = std::time::Instant::now();
    for (contact, who) in [(&a_id, "A"), (&d_id, "D")] {
        wait_until(
            &format!("C's details of {who}"),
            Duration::from_secs(300),
            || {
                dp_c.contact(c_id.clone(), contact.clone())
                    .ok()
                    .flatten()
                    .is_some_and(|detail| {
                        (detail.private_details, detail.publish_state)
                            == (ours(who), PublishState::Published)
                    })
            },
        );
    }
    eprintln!(
        "T2 restore: details back and Published in {:?}",
        started.elapsed()
    );
    engine.block_on(engine.shutdown()).unwrap();
}
