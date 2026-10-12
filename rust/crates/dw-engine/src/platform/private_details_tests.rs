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

use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use platform_wallet::wallet::identity::crypto::contact_info::{
    MAX_PLAINTEXT_LEN, encode_private_data,
};
use platform_wallet::{ContactCryptoProvider, EstablishedContact, ManagedIdentity};

use super::contacts::{PrivateDetails, PublishState};
use super::errors::{ContactError, PlatformError};
use super::flows::{DOCUMENT_10K_CREDITS, FlowKind};
use super::identity_writes::identity_locks;
use super::journal_tests::{id, request};
use super::keys_policy::key_path;
use super::private_details::{BoxedFuture, InfoDocument, InfoSource, digest, publish_state, set};

use super::registration_flow::b58;
use super::requests::tests::{Fixture, keys, wait_until};
use super::runtime::guard;
use super::signers::VaultContactCrypto;
use super::startup::StartupStatus;
use crate::coins::TestPause;
use crate::{DashNetwork, EngineEvent, PlatformChange};
use dw_vault::{Credential, GrantPurpose, SeedDerivation, WalletSecret};
use zeroize::Zeroizing;

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
        let plaintext = encode_private_data(&d.into());
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

    pub(crate) fn remove(&self, owner: Identifier, contact: Identifier) {
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

/// Another seed than the wallet's: keys that do not bind it.
pub(crate) fn other_seed() -> WalletSecret {
    WalletSecret {
        mnemonic: Zeroizing::new(Vec::new()),
        mnemonic_passphrase: Zeroizing::new(Vec::new()),
        seed: Zeroizing::new([7; 64]),
        derivation: SeedDerivation::RawSeed,
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
    fn with_identity<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut ManagedIdentity) -> T + Send + 'static,
    ) -> T {
        let manager = self.s.manager().unwrap();
        let wallet = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            f((state.identity_manager.managed_identity_mut(&id(ME))).unwrap())
        })
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
        self.with_identity(move |m| (&m.dashpay().established_contacts()[&id(contact)]).into())
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

    fn lease(&self) -> String {
        self.flow(
            FlowKind::PrivateDetails,
            vec![self.grant(DOCUMENT_10K_CREDITS)],
        )
    }

    /// Applies `d` to the contact as the library's DashPay pass applies a
    /// `contactInfo` (no settle, no record).
    fn apply(&self, contact: u8, d: &PrivateDetails) {
        let (manager, wallet, d) = (self.s.manager().unwrap(), self.wallet, d.clone());
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let m = (state.identity_manager.managed_identity_mut(&id(ME))).unwrap();
            let saved = m.set_contact_metadata(&id(contact), (&d).into(), wallet.persister());
            assert!(saved.unwrap());
        });
    }

    /// Replaces the wallet's seed in the vault with [`other_seed`].
    fn swap_seed(&self) {
        let (vault, wallet) = (self.s.vault(), self.wallet);
        assert!(vault.delete_wallet_secret(&wallet.0).unwrap());
        vault.store_wallet_secret(&wallet.0, &other_seed()).unwrap();
    }

    /// `contact`'s `publish_state` as `DashPay::contact` reports it.
    fn reported(&self, contact: u8) -> PublishState {
        let dp = self.s.dashpay(self.wallet);
        let detail = dp
            .contact(b58(&id(ME)), b58(&id(contact)))
            .unwrap()
            .unwrap();
        detail.publish_state
    }

    /// The wallet's DashPay startup status.
    fn startup(&self) -> Option<StartupStatus> {
        self.s.platform.startup_of(&self.wallet).map(|s| s.startup)
    }

    /// [`Self::settle`] on the runtime, not waited for.
    fn settle_in_background(&self) -> tokio::task::JoinHandle<bool> {
        let (s, wallet) = (Arc::clone(&self.s), self.wallet);
        self.s.rt.spawn(async move {
            let crypto = s.prompt_free_crypto(wallet).await.unwrap();
            let w = s.manager().unwrap().get_wallet(&wallet.0).await.unwrap();
            s.settle_contact_infos(&w, wallet, &crypto, None).await
        })
    }

    /// Removes the wallet, as `remove_wallet` with a wipe grant.
    fn remove_wallet(&self) {
        let (vault, wallet) = (self.s.vault(), self.wallet);
        let wipe = (vault.authorize(GrantPurpose::Wipe, Some(&wallet.0), Credential::None))
            .unwrap()
            .id;
        (self.engine)
            .block_on(self.s.remove_wallet(wallet, wipe))
            .unwrap();
    }

    /// Nothing of the removed wallet's private details stays: no
    /// `dp_contact_info` row, nothing in memory.
    fn nothing_of_the_wallet(&self) {
        let w = self.wallet.to_string();
        let rows = self.s.appdb_op(move |db| db.contact_info_digests(&w));
        assert_eq!(self.engine.block_on(rows).unwrap(), vec![]);
        assert!(!self.s.platform.contacts.digests.holds(&self.wallet));
    }

    /// The opens reads have begun.
    fn opens(&self) -> usize {
        guard(&self.s.platform.contacts.read_hooks).opens
    }

    /// Waits until a read is opening documents.
    fn reading(&self) {
        wait_until("a read's first open", Duration::from_secs(20), || {
            self.opens() > 0
        });
    }

    /// The `Platform{Contacts}` signals for our identity so far.
    fn contact_signals(&self) -> usize {
        let ours = PlatformChange::Contacts {
            identity: id(ME).to_string(Encoding::Base58),
        };
        let events = self.events.0.lock().unwrap();
        (events.iter())
            .filter(|e| matches!(e, EngineEvent::Platform { change, .. } if *change == ours))
            .count()
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

/// A write that failed before signing sent nothing: the quote goes back,
/// and the details return to what Platform holds (the library saved the
/// edit first); with nothing recorded the edit stays, `Local`.
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
    assert_eq!(
        Fixture::code(f.set(BOB, details("Bob"), Some(f.lease()))),
        "platform.unavailable"
    );
    assert_eq!(
        (f.shown(BOB), f.state(BOB)),
        (details("Bob"), PublishState::Local)
    );
}

/// A write that failed after signing may still land (Opus r1 #1): the
/// read-back that finds it is `Published`; otherwise the edit stays,
/// `Local`, with the error, and a later read decides: Platform's details
/// back when it did not land, `Published` when it landed late.
#[test]
fn a_failed_write_keeps_the_edit_until_a_read() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);

    let failed = f.set(ERIN, details("Erin2"), Some(f.lease()));
    assert_eq!(Fixture::code(failed), "platform.timeout");
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin2"), PublishState::Local)
    );
    assert!(f.settle(), "it did not land");
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin"), PublishState::Published)
    );

    let failed = f.set(ERIN, details("Erin3"), Some(f.lease()));
    assert_eq!(Fixture::code(failed), "platform.timeout");
    f.publish_elsewhere(ERIN, &details("Erin3"));
    assert!(f.settle(), "it landed late");
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin3"), PublishState::Published)
    );

    f.net.landed.store(true, Ordering::SeqCst);
    assert_eq!(
        f.set(ERIN, details("Erin4"), Some(f.lease())).unwrap(),
        PublishState::Published
    );
    assert_eq!(f.state(ERIN), PublishState::Published);
    assert_eq!(f.net.writes(), 4);
}

/// With a grant, details count as on Platform only while the contact still
/// shows them (Opus r1 #5): after another device's edit the library's pass
/// applied, the details recorded earlier are published again.
#[test]
fn a_stale_record_is_published_again() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    f.publish_elsewhere(ERIN, &details("Other"));
    f.apply(ERIN, &details("Other"));
    assert_eq!(
        f.set(ERIN, details("Erin"), Some(f.lease())).unwrap(),
        PublishState::Published
    );
    assert_eq!(f.net.writes(), 2);
    assert!(f.settle());
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin"), PublishState::Published)
    );
}

/// With nothing recorded (a settle that failed), an edit without a grant
/// reads Platform first (Opus r1 #4): details there need a grant. When the
/// read fails too, the edit is saved, `Local` (the residual of item 42).
#[test]
fn without_a_record_an_edit_reads_platform_first() {
    let f = published();
    f.publish_elsewhere(ERIN, &details("Other"));
    f.apply(ERIN, &details("Other"));
    assert_eq!(f.recorded(ERIN), None);
    assert_eq!(
        Fixture::code(f.set(ERIN, details("Erin"), None)),
        "platform.needs_grant"
    );
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Other"), PublishState::Published)
    );

    f.publish_elsewhere(BOB, &details("Other"));
    f.net.infos.down.store(true, Ordering::SeqCst);
    assert_eq!(
        f.set(BOB, details("Bob"), None).unwrap(),
        PublishState::Local
    );
    assert_eq!(f.shown(BOB), details("Bob"));
}

/// A settle that moves only a digest (the library's pass applied the
/// details already) signals `Contacts`, so a host re-reads `publish_state`
/// (Opus r1 #2).
#[test]
fn a_digest_change_signals_contacts() {
    let f = published();
    f.apply(ERIN, &details("Erin"));
    f.publish_elsewhere(ERIN, &details("Erin"));
    wait_until("the apply's signal", Duration::from_secs(20), || {
        f.contact_signals() > 0
    });
    std::thread::sleep(Duration::from_millis(300));
    let before = f.contact_signals();
    assert!(f.settle());
    assert_eq!(f.state(ERIN), PublishState::Published);
    wait_until("the digest's signal", Duration::from_secs(20), || {
        f.contact_signals() > before
    });
}

/// When the read-back fails too, the edit stays, `Local`, with its error;
/// a later read that finds it on Platform makes it `Published` without
/// another write (Q5).
#[test]
fn an_unknown_write_is_published_by_a_later_read() {
    let f = published();
    f.net.landed.store(true, Ordering::SeqCst);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    f.net.infos_down_after.store(true, Ordering::SeqCst);
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

/// The failed read-back's own retry (`RESETTLE_AFTER`, shortened in tests)
/// reads again on its own: once Platform answers, the edit is `Published`.
#[test]
fn a_failed_read_back_is_retried() {
    let f = published();
    f.net.landed.store(true, Ordering::SeqCst);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    f.net.infos_down_after.store(true, Ordering::SeqCst);
    let failed = f.set(ERIN, details("Erin"), Some(f.lease()));
    assert_eq!(Fixture::code(failed), "platform.timeout");
    let reads = f.net.infos.reads.load(Ordering::SeqCst);
    wait_until("a retry", Duration::from_secs(20), || {
        f.net.infos.reads.load(Ordering::SeqCst) > reads
    });
    assert_eq!(f.state(ERIN), PublishState::Local);
    f.net.infos.down.store(false, Ordering::SeqCst);
    wait_until("the retry's read", Duration::from_secs(20), || {
        f.state(ERIN) == PublishState::Published
    });
    assert_eq!(f.net.writes(), 1);
}

/// Refused before the grant is redeemed, with nothing written or saved:
/// details over DIP-15's plaintext cap, ourselves, no contact, an identity
/// without DashPay keys, credits below the quote, no document key the vault
/// holds, one the wallet only watches, and a lease of another flow.
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

    let poor = two_contacts(fixture(Fixture::new(DOCUMENT_10K_CREDITS - 1)));
    let grant = poor.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        Fixture::code(poor.set(ERIN, details("Erin"), Some(grant.clone()))),
        "platform.insufficient_credits"
    );
    assert!(poor.unused(&grant));

    let unheld = two_contacts(fixture(Fixture::foreign(keys(true), u64::MAX / 2)));
    let grant = unheld.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        Fixture::code(unheld.set(ERIN, details("Erin"), Some(grant.clone()))),
        "identity.keys_missing"
    );
    assert!(unheld.unused(&grant));

    f.with_identity(|m| m.identity_index = None);
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    let code = Fixture::code(f.set(ERIN, details("Erin"), Some(grant.clone())));
    assert_eq!(code, "invalid_argument");
    assert!(f.unused(&grant));
    let writes = [&f, &g, &poor, &unheld].map(|x| x.net.writes());
    assert_eq!(writes, [0; 4]);
}

/// The settle (bring-up, unlock, a failed read-back's retry) makes the
/// wallet agree with Platform: its details applied and recorded, a record
/// Platform no longer backs forgotten, and a document that does not open
/// with the wallet's keys skipped (Opus r1 #3). Documents of which none
/// opens (a seed that does not bind the wallet) fail the read, changing
/// nothing (Opus r2 M1).
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
    assert!(f.settle());
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin3"), PublishState::Published)
    );

    f.net.infos.remove(id(ME), id(ERIN));
    assert!(!f.settle());
    assert_eq!(
        (f.shown(ERIN), f.state(ERIN)),
        (details("Erin3"), PublishState::Published)
    );
}

/// A settle requires a verified seed binding (Sol r1 S1): with keys that
/// do not bind the wallet, a read that finds nothing changes nothing, the
/// recorded digests included.
#[test]
fn a_settle_needs_a_verified_seed_binding() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    f.set(BOB, details("Bob"), Some(f.lease())).unwrap();
    f.net.infos.remove(id(ME), id(ERIN));
    f.net.infos.remove(id(ME), id(BOB));
    f.swap_seed();
    assert!(!f.settle());
    assert_eq!(
        (f.state(ERIN), f.state(BOB)),
        (PublishState::Published, PublishState::Published)
    );
    assert_eq!(f.startup(), Some(StartupStatus::SeedBindingUnverified));
}

/// The unlock's settle checks the binding itself, after a contact-crypto
/// drain that had nothing queued and so checked nothing (Sol r1 S1): the
/// records stay, and the wallet reports `SeedBindingUnverified`.
#[test]
fn an_unlock_with_nothing_queued_still_checks_the_binding() {
    let f = published();
    f.set(BOB, details("Bob"), Some(f.lease())).unwrap();
    f.net.infos.remove(id(ME), id(BOB));
    f.swap_seed();
    let (s, wallet) = (Arc::clone(&f.s), f.wallet);
    let queued = f.engine.block_on(async move {
        let manager = s.manager().unwrap();
        let w = manager.get_wallet(&wallet.0).await.unwrap();
        let queued = (w.identity().dashpay())
            .drainable_contact_crypto_count()
            .await;
        s.after_unlock(&manager, wallet).await;
        queued
    });
    assert_eq!(queued, 0);
    wait_until("the settle's check", Duration::from_secs(20), || {
        f.startup() == Some(StartupStatus::SeedBindingUnverified)
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.state(BOB), PublishState::Published);
}

/// An edit's read-first is a settle too, with the same check: it leaves
/// other contacts' records alone under keys that do not bind the wallet.
#[test]
fn the_read_first_needs_a_verified_seed_binding() {
    let f = published();
    f.set(BOB, details("Bob"), Some(f.lease())).unwrap();
    f.net.infos.remove(id(ME), id(BOB));
    f.swap_seed();
    assert_eq!(f.recorded(ERIN), None);
    assert_eq!(
        f.set(ERIN, details("Erin"), None).unwrap(),
        PublishState::Local
    );
    assert_eq!(f.state(BOB), PublishState::Published);
    assert_eq!(f.startup(), Some(StartupStatus::SeedBindingUnverified));
}

/// One deadline bounds a settle, opens included, checked between opens
/// (Sol r1 F4): a slow vault ends an edit's read-first at the deadline,
/// which releases the identity's write lock.
#[test]
fn a_slow_read_ends_at_its_deadline() {
    let f = published();
    for n in 0..100 {
        f.publish_elsewhere(100 + n, &details("x"));
    }
    {
        let mut hooks = guard(&f.s.platform.contacts.read_hooks);
        hooks.budget = Some(Duration::from_secs(2));
        hooks.open_delay = Some(Duration::from_millis(100));
    }
    let locks = identity_locks(f.wallet, id(ME));
    let s = Arc::clone(&f.s);
    let pair = f.pair(ERIN);
    let manager = f.s.manager().unwrap();
    let net = Arc::clone(&f.net);
    let editing = f.s.rt.spawn(async move {
        let wallet = manager.get_wallet(&pair.wallet.0).await.unwrap();
        set(&*net, &s, &wallet, pair, details("Erin"), None).await
    });
    f.reading();
    // Waits for the edit's read, which holds the lock.
    let started = std::time::Instant::now();
    drop(f.engine.block_on(locks.write.lock()));
    let took = started.elapsed();
    // 100 opens would take 10 s.
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(f.opens() < 100);
    let edited = f.engine.block_on(editing).unwrap();
    assert_eq!(edited.unwrap(), PublishState::Local);
}

/// A read ends at the session's close, between opens, and releases the
/// identity's lock (Sol r1 F4).
#[test]
fn a_read_ends_at_the_sessions_close() {
    let f = published();
    for n in 0..100 {
        f.publish_elsewhere(100 + n, &details("x"));
    }
    guard(&f.s.platform.contacts.read_hooks).open_delay = Some(Duration::from_millis(100));
    let s = Arc::clone(&f.s);
    let pair = f.pair(ERIN);
    let manager = f.s.manager().unwrap();
    let net = Arc::clone(&f.net);
    // Admitted, as the facade's call is: the close waits for it.
    let editing = f.s.rt.spawn(async move {
        let _op = s.enter().await.unwrap();
        let wallet = manager.get_wallet(&pair.wallet.0).await.unwrap();
        let _ = set(&*net, &s, &wallet, pair, details("Erin"), None).await;
    });
    f.reading();
    let closing = std::time::Instant::now();
    (f.engine)
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    // 100 opens would take 10 s.
    let took = closing.elapsed();
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(f.opens() < 100);
    f.engine.block_on(editing).unwrap();
}

/// The deadline holds after the last open too (Sol r2 F4): a slow final
/// open fails the settle with the timeout and applies nothing.
#[test]
fn a_slow_last_open_ends_the_settle() {
    let f = published();
    f.publish_elsewhere(BOB, &details("Bobby"));
    {
        let mut hooks = guard(&f.s.platform.contacts.read_hooks);
        hooks.budget = Some(Duration::from_secs(2));
        hooks.open_delay = Some(Duration::from_secs(4));
    }
    let shown = f.shown(BOB);
    assert!(!f.settle());
    assert_eq!(f.opens(), 1);
    assert_eq!((f.shown(BOB), f.recorded(BOB)), (shown, None));
}

/// A write's read-back holds the deadline after its last open too: past
/// it the read-back finds nothing, and the edit stays `Local`.
#[test]
fn a_late_read_back_finds_nothing() {
    let f = published();
    f.net.landed.store(true, Ordering::SeqCst);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    {
        let mut hooks = guard(&f.s.platform.contacts.read_hooks);
        hooks.budget = Some(Duration::from_secs(2));
        hooks.open_delay = Some(Duration::from_secs(4));
    }
    assert_eq!(
        Fixture::code(f.set(ERIN, details("Erin"), Some(f.lease()))),
        "platform.timeout"
    );
    assert!(f.opens() >= 1);
    assert_eq!((f.shown(ERIN), f.recorded(ERIN)), (details("Erin"), None));
}

/// A settle past its deadline when its apply phase would start applies
/// nothing.
#[test]
fn a_settle_past_its_deadline_applies_nothing() {
    let f = published();
    f.publish_elsewhere(BOB, &details("Bobby"));
    guard(&f.s.platform.contacts.read_hooks).budget = Some(Duration::from_secs(1));
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.settle_pause) = Some(Arc::clone(&pause));
    let shown = f.shown(BOB);
    let settling = f.settle_in_background();
    wait_until("the settle's snapshot", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    std::thread::sleep(Duration::from_millis(1200));
    pause.release.notify_one();
    assert!(!f.engine.block_on(settling).unwrap());
    assert_eq!((f.shown(BOB), f.recorded(BOB)), (shown, None));
}

/// A settle that read the wallet before an unload writes nothing once it
/// is loaded again: what it read is not the wallet loaded.
#[test]
fn a_settle_across_an_unload_writes_nothing() {
    let f = published();
    f.publish_elsewhere(BOB, &details("Bobby"));
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.settle_pause) = Some(Arc::clone(&pause));
    let settling = f.settle_in_background();
    wait_until("the settle's snapshot", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    f.engine.block_on(f.s.unload_wallet(f.wallet)).unwrap();
    f.engine.block_on(f.s.load_wallet(f.wallet)).unwrap();
    pause.release.notify_one();
    assert!(!f.engine.block_on(settling).unwrap());
}

/// A settle that read its contacts before the wallet's removal writes
/// nothing of the wallet afterwards: no row, nothing in memory, no
/// details.
#[test]
fn a_settle_after_the_wallet_left_writes_nothing() {
    let f = published();
    f.publish_elsewhere(BOB, &details("Bobby"));
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.settle_pause) = Some(Arc::clone(&pause));
    let settling = f.settle_in_background();
    wait_until("the settle's snapshot", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    f.remove_wallet();
    pause.release.notify_one();
    assert!(!f.engine.block_on(settling).unwrap());
    f.nothing_of_the_wallet();
}

/// A publish that ends as the wallet is removed records nothing of it.
#[test]
fn a_publish_after_the_wallet_left_records_nothing() {
    let f = published();
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.publish_pause) = Some(Arc::clone(&pause));
    let (s, pair, lease) = (Arc::clone(&f.s), f.pair(ERIN), f.lease());
    let (manager, net) = (f.s.manager().unwrap(), Arc::clone(&f.net));
    let publishing = f.s.rt.spawn(async move {
        let wallet = manager.get_wallet(&pair.wallet.0).await.unwrap();
        set(&*net, &s, &wallet, pair, details("Erin"), Some(lease)).await
    });
    wait_until("the publish", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    f.remove_wallet();
    pause.release.notify_one();
    let published = f.engine.block_on(publishing).unwrap();
    assert_eq!(Fixture::code(published), "wallet_not_found");
    f.nothing_of_the_wallet();
}

/// A digest load that read before the wallet's removal lands nothing.
#[test]
fn a_load_after_the_wallet_left_lands_nothing() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    f.s.platform.contacts.digests.forget(&f.wallet);
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.load_pause) = Some(Arc::clone(&pause));
    f.reported(ERIN);
    wait_until("the load's read", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    f.remove_wallet();
    pause.release.notify_one();
    std::thread::sleep(Duration::from_millis(300));
    f.nothing_of_the_wallet();
}

/// A removal waits for a settle already writing, then removes what it
/// wrote.
#[test]
fn a_removal_waits_for_a_settle_writing() {
    let f = published();
    f.publish_elsewhere(BOB, &details("Bobby"));
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.apply_pause) = Some(Arc::clone(&pause));
    let settling = f.settle_in_background();
    wait_until("the settle's writes", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    std::thread::scope(|scope| {
        let removal = scope.spawn(|| f.remove_wallet());
        std::thread::sleep(Duration::from_millis(300));
        assert!(!removal.is_finished());
        pause.release.notify_one();
        removal.join().unwrap();
    });
    assert!(f.engine.block_on(settling).unwrap());
    f.nothing_of_the_wallet();
}

/// A digest load overtaken by a write reads again (Sol r2 R6): what it
/// read first lacks the write.
#[test]
fn a_load_overtaken_by_a_write_reads_again() {
    let f = published();
    f.s.platform.contacts.digests.forget(&f.wallet);
    let pause = Arc::new(TestPause::default());
    *guard(&f.s.platform.contacts.load_pause) = Some(Arc::clone(&pause));
    assert_eq!(f.reported(ERIN), PublishState::Local);
    wait_until("the load's read", Duration::from_secs(20), || {
        pause.reached.load(Ordering::SeqCst)
    });
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    pause.release.notify_one();
    wait_until("the second read", Duration::from_secs(20), || {
        f.reported(ERIN) == PublishState::Published
    });
}

/// The library's publish reads every document the identity holds, with no
/// bound: over the engine's cap (every document, opened or not) a publish
/// is refused before the grant is redeemed (Sol r1 F3). (The SDK source
/// counts its raw page entries, parsed or not.)
#[test]
fn a_publish_over_the_document_cap_is_refused() {
    let f = published();
    let junk = |n: u32| {
        let mut contact = [9; 32];
        contact[..4].copy_from_slice(&n.to_le_bytes());
        Identifier::from(contact)
    };
    let doc = InfoDocument {
        root: ROOT,
        derivation: 0,
        enc_to_user_id: [3; 32],
        private_data: vec![5; 48],
        updated_at: 0,
    };
    for n in 0..=1000 {
        f.net.infos.put(id(ME), junk(n), doc.clone());
    }
    let grant = f.grant(DOCUMENT_10K_CREDITS);
    assert_eq!(
        Fixture::code(f.set(ERIN, details("Erin"), Some(grant.clone()))),
        "platform.not_implemented"
    );
    assert!(f.unused(&grant));
    assert_eq!(f.net.writes(), 0);
    f.net.infos.remove(id(ME), junk(1000));
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    assert_eq!(f.net.writes(), 1);
}

/// `contact` reads the digests from memory (m4 §1; Sol r1 F2): loaded once
/// in the background, then written through by each record and forget.
#[test]
fn contact_reads_digests_from_memory() {
    let f = published();
    f.set(ERIN, details("Erin"), Some(f.lease())).unwrap();
    wait_until("the digests' load", Duration::from_secs(20), || {
        f.reported(ERIN) == PublishState::Published
    });
    // Gone from app.sqlite behind the engine: the read answers from memory.
    let (w, i, c) = (f.wallet.to_string(), b58(&id(ME)), b58(&id(ERIN)));
    let forgot =
        f.s.appdb_op(move |db| db.forget_contact_info_digest(&w, &i, &c));
    assert!(f.engine.block_on(forgot).unwrap());
    assert_eq!(f.reported(ERIN), PublishState::Published);
    // A forget through the engine lands in memory at once.
    f.net.infos.remove(id(ME), id(ERIN));
    f.publish_elsewhere(BOB, &details("Bob"));
    assert!(f.settle());
    assert_eq!(f.reported(ERIN), PublishState::Local);
    assert_eq!(f.reported(BOB), PublishState::Published);
}

/// A settle forgets the records of contacts no longer established (Sol r1
/// F5), and signals it.
#[test]
fn a_settle_forgets_contacts_no_longer_established() {
    let f = published();
    assert!(f.settle());
    // That settle's own signals land first.
    std::thread::sleep(Duration::from_secs(1));
    let (w, i, c) = (f.wallet.to_string(), b58(&id(ME)), b58(&id(DAVE)));
    let planted =
        f.s.appdb_op(move |db| db.set_contact_info_digest(&w, &i, &c, &[1; 32], 1));
    assert!(f.engine.block_on(planted).unwrap());
    // Nothing else changes in this settle: the signal is the forget's.
    let before = f.contact_signals();
    assert!(f.settle());
    assert_eq!(f.recorded(DAVE), None);
    wait_until("the signal", Duration::from_secs(20), || {
        f.contact_signals() > before
    });
}

/// A signed write whose read-back cannot reach the document source reports
/// the write's error, and a later read still finds it (Sol r1, M26).
#[test]
fn a_read_back_without_its_source_is_retried() {
    let f = published();
    f.net.landed.store(true, Ordering::SeqCst);
    *f.net.fails.lock().unwrap() = Some(PlatformError::Timeout);
    let s = Arc::clone(&f.s);
    *f.net.during.lock().unwrap() = Some(Box::new(move || {
        guard(&s.platform.contacts.read_hooks).gone = true;
    }));
    let failed = f.set(ERIN, details("Erin"), Some(f.lease()));
    assert_eq!(Fixture::code(failed), "platform.timeout");
    assert_eq!(f.state(ERIN), PublishState::Local);
    *f.net.during.lock().unwrap() = None;
    guard(&f.s.platform.contacts.read_hooks).gone = false;
    wait_until("the retry's read", Duration::from_secs(20), || {
        f.state(ERIN) == PublishState::Published
    });
    assert_eq!(f.net.writes(), 1);
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

    let held = f.engine.block_on(locks.write.lock());
    let reads = f.net.infos.reads.load(Ordering::SeqCst);
    let s = Arc::clone(&f.s);
    let wallet = f.wallet;
    let settling = f.s.rt.spawn(async move {
        let crypto = s.prompt_free_crypto(wallet).await.unwrap();
        let w = s.manager().unwrap().get_wallet(&wallet.0).await.unwrap();
        s.settle_contact_infos(&w, wallet, &crypto, None).await
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        f.net.infos.reads.load(Ordering::SeqCst),
        reads,
        "the settle waits"
    );
    drop(held);
    assert!(f.engine.block_on(settling).unwrap());
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
    use super::requests::tests::passes;
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
        assert!(charged > 0, "{what} charged nothing");
    };
    let eligibility = |dp: &super::DashPay, me: &str, them: &str| {
        engine
            .block_on(dp.eligibility(me.into(), them.into()))
            .unwrap()
    };

    // 1. D becomes C's second contact.
    let first = eligibility(&dp_c, &c_id, &d_id);
    match first {
        Eligibility::AlreadyContact => eprintln!("T2 rerun: C and D are contacts"),
        Eligibility::Ok | Eligibility::PendingIncoming => {
            if first == Eligibility::Ok {
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
