//! The Identities page's reads (DP6-01) over a regtest session whose
//! Platform answers are scripted ([`FakeNet`]): the detail's keys, a
//! refresh's fold (keys and revision, never the balance; not from an older
//! answer; under the write lock), the refresh of all, and the read-only
//! lookup by id and by name.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{
    Identity, IdentityPublicKey, KeyType as DppKeyType, Purpose, SecurityLevel as DppSecurityLevel,
};
use dpp::platform_value::BinaryData;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use platform_wallet::changeset::PlatformWalletChangeSet;

use crate::PlatformChange;
use crate::platform::balances::Refreshed;
use crate::platform::dashpay::DashPay;
use crate::platform::errors::PlatformError;
use crate::platform::identity::{FoundBy, IdentityKeyInfo, KeyPurpose, KeyType, SecurityLevel};
use crate::platform::identity_reads::IdentityNet;
use crate::platform::identity_writes::identity_locks;
use crate::platform::runtime::guard;
use crate::platform::topup_tests::{Fixture, fixture, options};
use crate::platform::trust::{self, BoxFuture};

const MINE: u8 = 1;
const SECOND: u8 = 2;
const STRANGER: u8 = 9;
/// The DashPay contract's id as the keys bind it here.
const CONTRACT: u8 = 0xdc;

fn id(n: u8) -> Identifier {
    Identifier::new([n; 32])
}

fn b58(n: u8) -> String {
    id(n).to_string(Encoding::Base58)
}

fn key(id: u32, purpose: Purpose, level: DppSecurityLevel) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id,
        purpose,
        security_level: level,
        contract_bounds: None,
        key_type: DppKeyType::ECDSA_SECP256K1,
        read_only: false,
        data: BinaryData::new(vec![2; 33]),
        disabled_at: None,
    })
}

fn keys(list: Vec<IdentityPublicKey>) -> BTreeMap<u32, IdentityPublicKey> {
    list.into_iter().map(|k| (k.id(), k)).collect()
}

/// Keys 0 (master), 1 (high), and 2, an encryption key bound to the
/// DashPay contract's `contactRequest`, read-only and disabled.
fn base_keys() -> BTreeMap<u32, IdentityPublicKey> {
    let mut bound = key(2, Purpose::ENCRYPTION, DppSecurityLevel::MEDIUM);
    if let IdentityPublicKey::V0(k) = &mut bound {
        k.contract_bounds = Some(ContractBounds::SingleContractDocumentType {
            id: id(CONTRACT),
            document_type_name: "contactRequest".into(),
        });
        k.read_only = true;
        k.disabled_at = Some(1_700_000_000_000);
        k.key_type = DppKeyType::BLS12_381;
    }
    keys(vec![
        key(0, Purpose::AUTHENTICATION, DppSecurityLevel::MASTER),
        key(1, Purpose::AUTHENTICATION, DppSecurityLevel::HIGH),
        bound,
    ])
}

fn identity(
    n: u8,
    keys: BTreeMap<u32, IdentityPublicKey>,
    balance: u64,
    revision: u64,
) -> Identity {
    Identity::V0(IdentityV0 {
        id: id(n),
        public_keys: keys,
        balance,
        revision,
    })
}

/// Gives the wallet `MINE` (index 0) and `SECOND` (index 1), each with
/// [`base_keys`], balance 1 000 and revision 1.
fn give_identities(f: &Fixture) {
    let manager = f.session.manager().unwrap();
    let wallet = f.wallet;
    f.engine.block_on(async move {
        let w = manager.get_wallet(&wallet.0).await.unwrap();
        let mut state = w.state_mut().await;
        for (index, n) in [(0, MINE), (1, SECOND)] {
            (state.identity_manager)
                .add_identity(
                    identity(n, base_keys(), 1_000, 1),
                    index,
                    wallet.0,
                    w.persister(),
                )
                .unwrap();
        }
    });
}

/// The library's copy of the wallet's identity `n`.
fn held(f: &Fixture, n: u8) -> Identity {
    let manager = f.session.manager().unwrap();
    let wallet = f.wallet;
    f.engine.block_on(async move {
        let w = manager.get_wallet(&wallet.0).await.unwrap();
        let state = w.state().await;
        (state.identity_manager.wallet_identity(&wallet.0, &id(n)))
            .unwrap()
            .identity
            .clone()
    })
}

/// Platform as a test scripts it: identities, domains (normalized label →
/// spelling, identity), names per identity, whether each fetch's proofs
/// verify, and what was asked.
#[derive(Default)]
struct FakeNet {
    identities: Mutex<HashMap<Identifier, Identity>>,
    domains: HashMap<String, (String, Identifier)>,
    names: HashMap<Identifier, Vec<String>>,
    names_fail: bool,
    fail: Mutex<HashMap<Identifier, PlatformError>>,
    verified: bool,
    /// The names read's proofs, when not `verified`'s.
    names_verified: Option<bool>,
    /// Each identity fetched, and whether its write lock was held then.
    fetched: Mutex<Vec<(Identifier, bool)>>,
    /// The wallet whose locks the fetch checks.
    wallet: Option<crate::WalletId>,
}

impl IdentityNet for FakeNet {
    fn identity(&self, id: Identifier) -> BoxFuture<Result<Option<Identity>, PlatformError>> {
        let held = self
            .wallet
            .is_some_and(|w| identity_locks(w, id).write.try_lock().is_err());
        self.fetched.lock().unwrap().push((id, held));
        let answer = match self.fail.lock().unwrap().remove(&id) {
            Some(e) => Err(e),
            None => Ok(self.identities.lock().unwrap().get(&id).cloned()),
        };
        noted(self.verified, answer)
    }

    fn domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        let found = self.domains.get(&label).cloned();
        noted(self.verified, Ok(found))
    }

    fn names_of(
        &self,
        id: Identifier,
        limit: u32,
    ) -> BoxFuture<Result<Vec<String>, PlatformError>> {
        let names = match self.names_fail {
            true => Err(PlatformError::Unavailable),
            false => Ok(
                (self.names.get(&id).cloned().unwrap_or_default().into_iter())
                    .take(limit as usize)
                    .collect(),
            ),
        };
        noted(self.names_verified.unwrap_or(self.verified), names)
    }
}

/// `answer`, its proof lookup noted when the future runs, as the SDK's are:
/// inside the caller's `trust::scoped`.
fn noted<T: Send + 'static>(verified: bool, answer: T) -> BoxFuture<T> {
    Box::pin(async move {
        trust::note_lookup(verified);
        answer
    })
}

fn install(f: &Fixture, net: FakeNet) -> Arc<FakeNet> {
    let net = Arc::new(FakeNet {
        wallet: Some(f.wallet),
        ..net
    });
    *guard(&f.session.platform.identity_net) = Some(net.clone());
    net
}

/// Balance refreshes answer `balance` at the refresh's moment, and are
/// counted.
fn balance_hook(f: &Fixture, balance: u64) -> Arc<Mutex<Vec<Identifier>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    *f.session.platform.balances.test_refresh.lock().unwrap() =
        Some(Arc::new(move |_, identity| {
            s.lock().unwrap().push(identity);
            Ok(Some(Refreshed {
                balance,
                as_of: Some(crate::platform::balances::now_ms()),
            }))
        }));
    seen
}

fn dp(f: &Fixture) -> Arc<DashPay> {
    f.session.dashpay(f.wallet)
}

#[test]
fn the_detail_lists_each_key_with_its_purpose_level_type_and_bounds() {
    let f = fixture(false);
    give_identities(&f);
    let detail = f
        .engine
        .block_on(dp(&f).identity_detail(b58(MINE)))
        .unwrap();
    assert_eq!(detail.summary.identity, b58(MINE));
    assert_eq!(detail.summary.index, 0);
    assert_eq!(detail.revision, Some(1));
    let bound = IdentityKeyInfo {
        id: 2,
        purpose: KeyPurpose::Encryption,
        security_level: SecurityLevel::Medium,
        key_type: KeyType::Bls12381,
        public_key: "02".repeat(33),
        read_only: true,
        disabled_at: Some(1_700_000_000_000),
        contract_bound: Some(b58(CONTRACT)),
        contract_bound_document_type: Some("contactRequest".into()),
    };
    let ids: Vec<_> = detail
        .public_keys
        .iter()
        .map(|k| (k.id, k.purpose, k.security_level))
        .collect();
    assert_eq!(
        ids,
        vec![
            (0, KeyPurpose::Authentication, SecurityLevel::Master),
            (1, KeyPurpose::Authentication, SecurityLevel::High),
            (2, KeyPurpose::Encryption, SecurityLevel::Medium),
        ]
    );
    assert_eq!(detail.public_keys[2], bound);
    assert_eq!(detail.public_keys[0].key_type, KeyType::EcdsaSecp256k1);
    assert!(!detail.public_keys[0].read_only && detail.public_keys[0].disabled_at.is_none());
}

#[test]
fn the_detail_refuses_another_wallets_identity_and_a_malformed_id() {
    let f = fixture(false);
    give_identities(&f);
    let err = f
        .engine
        .block_on(dp(&f).identity_detail(b58(STRANGER)))
        .unwrap_err();
    assert_eq!(err.code(), "identity.not_found");
    let err = f
        .engine
        .block_on(dp(&f).identity_detail("nope".into()))
        .unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
}

/// Platform's newer key set and revision reach the library and the
/// detail; the balance is the balance refresh's, not the fetched one's.
#[test]
fn a_refresh_folds_platforms_keys_and_revision_and_refreshes_the_balance_apart() {
    let f = fixture(false);
    give_identities(&f);
    let mut newer = base_keys();
    newer.insert(3, key(3, Purpose::TRANSFER, DppSecurityLevel::CRITICAL));
    if let Some(IdentityPublicKey::V0(k)) = newer.get_mut(&1) {
        k.disabled_at = Some(1_800_000_000_000);
    }
    let net = install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(MINE),
                identity(MINE, newer.clone(), 999_999, 4),
            )])),
            ..FakeNet::default()
        },
    );
    let balances = balance_hook(&f, 5_000);
    let detail = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap();
    assert_eq!(detail.revision, Some(4));
    assert_eq!(detail.public_keys.len(), 4);
    assert_eq!(detail.public_keys[1].disabled_at, Some(1_800_000_000_000));
    assert_eq!(detail.public_keys[3].purpose, KeyPurpose::Transfer);
    let lib = held(&f, MINE);
    assert_eq!(
        (lib.revision(), lib.public_keys(), lib.balance()),
        (4, &newer, 1_000)
    );
    assert_eq!(
        *balances.lock().unwrap(),
        vec![id(MINE)],
        "the balance refresh ran once"
    );
    assert_eq!(
        *net.fetched.lock().unwrap(),
        vec![(id(MINE), true)],
        "fetched under the write lock"
    );
    // The other identity is untouched.
    assert_eq!(held(&f, SECOND).revision(), 1);
}

/// An answer older than the library's revision changes nothing (a node a
/// block behind would re-enable a disabled key).
#[test]
fn a_refresh_ignores_an_answer_older_than_the_librarys_revision() {
    let f = fixture(false);
    give_identities(&f);
    let mut older = base_keys();
    if let Some(IdentityPublicKey::V0(k)) = older.get_mut(&2) {
        k.disabled_at = None;
    }
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(id(MINE), identity(MINE, older, 1_000, 0))])),
            ..FakeNet::default()
        },
    );
    balance_hook(&f, 1_000);
    let detail = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap();
    assert_eq!(detail.revision, Some(1));
    assert_eq!(detail.public_keys[2].disabled_at, Some(1_700_000_000_000));
    assert_eq!(held(&f, MINE).public_keys(), &base_keys());
}

#[test]
fn a_refresh_refuses_an_identity_not_the_wallets_or_not_on_platform() {
    let f = fixture(false);
    give_identities(&f);
    let net = install(&f, FakeNet::default());
    let err = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(STRANGER)))
        .unwrap_err();
    assert_eq!(err.code(), "identity.not_found");
    assert!(
        net.fetched.lock().unwrap().is_empty(),
        "nothing asked for a stranger"
    );
    let err = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap_err();
    assert_eq!(err.code(), "identity.not_found");
    // A refused proof reaches the caller typed.
    net.fail
        .lock()
        .unwrap()
        .insert(id(MINE), PlatformError::TrustMismatch);
    let err = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap_err();
    assert_eq!(err.code(), "platform.trust_mismatch");
}

#[test]
fn refreshing_all_reports_each_identity_in_index_order() {
    let f = fixture(false);
    give_identities(&f);
    let net = install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(MINE),
                identity(MINE, base_keys(), 0, 2),
            )])),
            ..FakeNet::default()
        },
    );
    net.fail
        .lock()
        .unwrap()
        .insert(id(SECOND), PlatformError::Unavailable);
    balance_hook(&f, 1_000);
    let outcomes = f.engine.block_on(dp(&f).refresh_identities()).unwrap();
    let got: Vec<_> = outcomes
        .iter()
        .map(|o| (o.identity.clone(), o.failure_code.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (b58(MINE), None),
            (b58(SECOND), Some("platform.unavailable".to_string())),
        ]
    );
    assert_eq!(held(&f, MINE).revision(), 2);
}

/// A lookup by name (any case, with `.dash`) or by id shows the identity,
/// its names and keys, and whether it is the wallet's; it adds nothing to
/// the library.
#[test]
fn find_reads_any_identity_by_name_or_id_and_adds_nothing() {
    let f = fixture(false);
    give_identities(&f);
    let stranger = identity(STRANGER, base_keys(), 42_000, 7);
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([
                (id(STRANGER), stranger),
                (id(SECOND), identity(SECOND, base_keys(), 1_000, 1)),
            ])),
            domains: HashMap::from([("a11ce".to_string(), ("Alice".to_string(), id(STRANGER)))]),
            names: HashMap::from([(
                id(STRANGER),
                vec!["Alice".to_string(), "alice-2".to_string()],
            )]),
            verified: true,
            ..FakeNet::default()
        },
    );
    let found = f
        .engine
        .block_on(dp(&f).find_identity(" ALICE.dash ".into()))
        .unwrap()
        .unwrap();
    assert_eq!(found.identity, b58(STRANGER));
    assert_eq!(
        found.found_by,
        FoundBy::Name {
            label: "Alice".into()
        }
    );
    assert_eq!(found.names, vec!["Alice", "alice-2"]);
    assert_eq!((found.balance, found.revision), (Some(42_000), Some(7)));
    assert_eq!(found.public_keys.len(), 3);
    assert!(!found.own && !found.unverified);

    let by_id = f
        .engine
        .block_on(dp(&f).find_identity(b58(STRANGER)))
        .unwrap()
        .unwrap();
    assert_eq!(by_id.found_by, FoundBy::Id);
    let own = f
        .engine
        .block_on(dp(&f).find_identity(b58(SECOND)))
        .unwrap()
        .unwrap();
    assert!(own.own && own.names.is_empty());

    let summaries = f.session.identity_summaries(f.wallet).unwrap();
    assert_eq!(summaries.len(), 2, "the lookup watched nothing");
    let manager = f.session.manager().unwrap();
    let wallet = f.wallet;
    let observed = f.engine.block_on(async move {
        let w = manager.get_wallet(&wallet.0).await.unwrap();
        let state = w.state().await;
        state.identity_manager.identity(&id(STRANGER)).is_some()
    });
    assert!(!observed, "the stranger is not in the library");
}

#[test]
fn find_answers_none_for_unknown_names_and_ids_and_refuses_other_text() {
    let f = fixture(false);
    give_identities(&f);
    install(
        &f,
        FakeNet {
            verified: true,
            ..FakeNet::default()
        },
    );
    assert_eq!(
        f.engine
            .block_on(dp(&f).find_identity("nobody".into()))
            .unwrap(),
        None
    );
    assert_eq!(
        f.engine
            .block_on(dp(&f).find_identity(b58(STRANGER)))
            .unwrap(),
        None
    );
    for bad in ["", "-bad", "a b", "x"] {
        let err = f
            .engine
            .block_on(dp(&f).find_identity(bad.into()))
            .unwrap_err();
        assert_eq!(err.code(), "invalid_argument", "{bad:?}");
    }
}

/// A failed names read leaves the names empty; once the fallback served a
/// read, an answer whose proofs did not verify against SPV is unverified.
#[test]
fn find_survives_a_failed_names_read_and_marks_an_unverified_answer() {
    let f = fixture(false);
    give_identities(&f);
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(STRANGER),
                identity(STRANGER, base_keys(), 1, 1),
            )])),
            names_fail: true,
            verified: false,
            ..FakeNet::default()
        },
    );
    let found = f
        .engine
        .block_on(dp(&f).find_identity(b58(STRANGER)))
        .unwrap()
        .unwrap();
    assert!(
        found.names.is_empty() && !found.unverified,
        "no fallback latched yet"
    );
    f.session
        .live()
        .unwrap()
        .provenance
        .fallback_in_use()
        .unwrap();
    let found = f
        .engine
        .block_on(dp(&f).find_identity(b58(STRANGER)))
        .unwrap()
        .unwrap();
    assert!(found.unverified);
}

/// The names a lookup shows count in `unverified` as an `IdentitySummary`'s
/// do (m4 §5): an identity whose proofs verified, shown with names whose
/// read did not, is unverified (review F1).
#[test]
fn find_counts_the_names_it_shows_in_unverified() {
    let f = fixture(false);
    give_identities(&f);
    let net = FakeNet {
        identities: Mutex::new(HashMap::from([(
            id(STRANGER),
            identity(STRANGER, base_keys(), 1, 1),
        )])),
        names: HashMap::from([(id(STRANGER), vec!["Bob".to_string()])]),
        verified: true,
        names_verified: Some(false),
        ..FakeNet::default()
    };
    install(&f, net);
    f.session
        .live()
        .unwrap()
        .provenance
        .fallback_in_use()
        .unwrap();
    let found = f
        .engine
        .block_on(dp(&f).find_identity(b58(STRANGER)))
        .unwrap()
        .unwrap();
    assert_eq!(found.names, vec!["Bob"]);
    assert!(found.unverified, "the names read did not verify");

    // The same names read verified: nothing is unverified.
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(STRANGER),
                identity(STRANGER, base_keys(), 1, 1),
            )])),
            names: HashMap::from([(id(STRANGER), vec!["Bob".to_string()])]),
            verified: true,
            ..FakeNet::default()
        },
    );
    let found = f
        .engine
        .block_on(dp(&f).find_identity(b58(STRANGER)))
        .unwrap()
        .unwrap();
    assert!(!found.unverified);
}

/// An answer at the library's own revision still folds its keys: the
/// library stamps a local key disable with its own clock, and Platform's
/// block time replaces it on the next refresh (review F3, D16).
#[test]
fn an_answer_at_the_same_revision_folds_platforms_disable_time() {
    let f = fixture(false);
    give_identities(&f);
    let mut same = base_keys();
    if let Some(IdentityPublicKey::V0(k)) = same.get_mut(&2) {
        k.disabled_at = Some(1_699_999_990_000);
    }
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(MINE),
                identity(MINE, same.clone(), 1_000, 1),
            )])),
            ..FakeNet::default()
        },
    );
    balance_hook(&f, 1_000);
    let detail = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap();
    assert_eq!(detail.revision, Some(1));
    assert_eq!(detail.public_keys[2].disabled_at, Some(1_699_999_990_000));
    assert_eq!(held(&f, MINE).public_keys(), &same);
}

/// Fails the wallet store's next stores that `fails` matches.
fn fail_stores(f: &Fixture, fails: impl Fn(&PlatformWalletChangeSet) -> bool + Send + 'static) {
    *f.session.live().unwrap().store.fail_store.lock().unwrap() = Some(Box::new(fails));
}

fn heal_stores(f: &Fixture) {
    *f.session.live().unwrap().store.fail_store.lock().unwrap() = None;
}

fn reopen(f: &mut Fixture) {
    f.engine
        .block_on(f.engine.close_network(crate::DashNetwork::Regtest))
        .unwrap();
    f.session = f
        .engine
        .block_on(
            f.engine
                .open_network(crate::DashNetwork::Regtest, options()),
        )
        .unwrap();
}

/// A key whose store fails leaves the old revision, in memory and on disk,
/// and the key as it was; the next refresh folds the answer again and
/// stores it (review F2, D16).
#[test]
fn a_failed_key_store_keeps_the_old_revision_and_a_retry_stores_the_key() {
    let mut f = fixture(false);
    give_identities(&f);
    let mut newer = base_keys();
    if let Some(IdentityPublicKey::V0(k)) = newer.get_mut(&1) {
        k.disabled_at = Some(1_800_000_000_000);
    }
    newer.insert(3, key(3, Purpose::TRANSFER, DppSecurityLevel::CRITICAL));
    let platform = || FakeNet {
        identities: Mutex::new(HashMap::from([(
            id(MINE),
            identity(MINE, newer.clone(), 1_000, 2),
        )])),
        ..FakeNet::default()
    };
    install(&f, platform());
    balance_hook(&f, 1_000);
    // Key 1 stores; key 3 does not.
    fail_stores(&f, |cs| {
        (cs.identity_keys.as_ref()).is_some_and(|k| k.upserts.keys().any(|(_, key)| *key == 3))
    });
    let err = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap_err();
    assert_eq!(err.code(), "storage");
    let mut stored = base_keys();
    stored.insert(1, newer[&1].clone());
    let lib = held(&f, MINE);
    assert_eq!((lib.revision(), lib.public_keys()), (1, &stored), "memory");
    heal_stores(&f);
    reopen(&mut f);
    let lib = held(&f, MINE);
    assert_eq!((lib.revision(), lib.public_keys()), (1, &stored), "disk");

    install(&f, platform());
    balance_hook(&f, 1_000);
    f.engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap();
    reopen(&mut f);
    let lib = held(&f, MINE);
    assert_eq!((lib.revision(), lib.public_keys()), (2, &newer), "retried");
}

/// A newer revision with the same keys stores the revision alone; its
/// failed store is `storage` and leaves the old revision (review F2).
#[test]
fn a_failed_revision_store_is_storage_and_keeps_the_old_revision() {
    let mut f = fixture(false);
    give_identities(&f);
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(MINE),
                identity(MINE, base_keys(), 1_000, 3),
            )])),
            ..FakeNet::default()
        },
    );
    balance_hook(&f, 1_000);
    fail_stores(&f, |cs| {
        (cs.identities.as_ref()).is_some_and(|i| i.identities.values().any(|e| e.revision == 3))
    });
    let err = f
        .engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap_err();
    assert_eq!(err.code(), "storage");
    assert_eq!(held(&f, MINE).revision(), 1);
    heal_stores(&f);
    f.engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap();
    reopen(&mut f);
    assert_eq!(held(&f, MINE).revision(), 3);
}

/// The wallet gone mid-run fails Refresh all as a whole, not identity by
/// identity (contract §2.1, review N2).
#[test]
fn refreshing_all_fails_as_a_whole_when_the_wallet_goes() {
    let f = fixture(false);
    give_identities(&f);
    let net = install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(MINE),
                identity(MINE, base_keys(), 0, 1),
            )])),
            ..FakeNet::default()
        },
    );
    net.fail
        .lock()
        .unwrap()
        .insert(id(SECOND), PlatformError::WalletNotFound);
    balance_hook(&f, 1_000);
    let err = f.engine.block_on(dp(&f).refresh_identities()).unwrap_err();
    assert_eq!(err.code(), "wallet_not_found");
}

/// A restart reads the refreshed keys and revision back from the wallet's
/// store, and keeps the main identity the user chose.
#[test]
fn the_refreshed_keys_and_the_chosen_main_identity_survive_a_restart() {
    let mut f = fixture(false);
    give_identities(&f);
    let mut newer = base_keys();
    newer.insert(3, key(3, Purpose::TRANSFER, DppSecurityLevel::CRITICAL));
    install(
        &f,
        FakeNet {
            identities: Mutex::new(HashMap::from([(
                id(MINE),
                identity(MINE, newer.clone(), 1, 5),
            )])),
            ..FakeNet::default()
        },
    );
    balance_hook(&f, 1_000);
    f.engine
        .block_on(dp(&f).refresh_identity(b58(MINE)))
        .unwrap();
    f.engine
        .block_on(dp(&f).set_main_identity(b58(SECOND)))
        .unwrap();

    f.engine
        .block_on(f.engine.close_network(crate::DashNetwork::Regtest))
        .unwrap();
    f.session = f
        .engine
        .block_on(
            f.engine
                .open_network(crate::DashNetwork::Regtest, options()),
        )
        .unwrap();
    let lib = held(&f, MINE);
    assert_eq!((lib.revision(), lib.public_keys()), (5, &newer));
    let main: Vec<_> = (f.session.identity_summaries(f.wallet).unwrap().into_iter())
        .map(|s| (s.identity, s.is_main))
        .collect();
    assert_eq!(main, vec![(b58(MINE), false), (b58(SECOND), true)]);
}

/// Set as main writes no changeset, so it signals itself: the list's main
/// mark and `DashPay::status` re-read on it (review B1).
#[test]
fn set_main_signals_the_identities_and_the_status() {
    let f = fixture(false);
    give_identities(&f);
    let pump = &f.session.hub.pump;
    *pump.platform_marks.lock().unwrap() = Some(Vec::new());
    f.engine
        .block_on(dp(&f).set_main_identity(b58(SECOND)))
        .unwrap();
    let marks = pump.platform_marks.lock().unwrap().take().unwrap();
    let both = BTreeSet::from([PlatformChange::Identities, PlatformChange::Startup]);
    assert!(marks.contains(&(f.wallet, both)), "{marks:?}");
}
