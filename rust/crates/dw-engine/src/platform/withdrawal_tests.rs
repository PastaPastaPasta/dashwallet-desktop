//! Tests of withdrawal.rs (DP6-02): Platform's rules at every known
//! protocol version (the range, the fee reserve, the payout), "Max", the
//! destination, and the write on requests_tests.rs's fixture with a
//! scripted Platform ([`FakeWithdrawals`]): the transition's bytes against
//! dpp's own construction, the refusals that keep the grant, rule 5, and a
//! broadcast that failed after signing. The devnet round trip is T2
//! (`/work/scratch/dw-dp6-02/t2`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use dpp::bls::native_bls::NativeBlsModule;
use dpp::serialization::PlatformSerializable;
use dpp::state_transition::StateTransitionHasUserFeeIncrease;
use dpp::state_transition::identity_credit_withdrawal_transition::accessors::IdentityCreditWithdrawalTransitionAccessorsV0;

use super::super::flows::FlowKind;
use super::super::journal_tests::id;
use super::super::money_gate_tests::{FakePlatform, Proof, gate_session};
use super::super::requests::tests::{Fixture, keys};
use super::super::trust::TrustMode;
use super::*;

/// The protocol versions Drive takes withdrawals at.
fn versions() -> impl Iterator<Item = &'static PlatformVersion> {
    (1..=PlatformVersion::latest().protocol_version)
        .map(|v| PlatformVersion::get(v).unwrap())
        .filter(|v| Rules::at(v).is_ok())
}

const DASH: u64 = 100_000_000_000;

#[test]
fn every_version_from_the_first_withdrawals_has_rules() {
    let first = versions().next().unwrap().protocol_version;
    for v in 1..first {
        let refused = Rules::at(PlatformVersion::get(v).unwrap()).unwrap_err();
        assert_eq!(refused.code(), "invalid_argument", "{v}");
    }
    assert_eq!(
        versions().count() as u32,
        PlatformVersion::latest().protocol_version - first + 1
    );
}

/// The range is Drive's structure check's, raised to clear dust; the
/// reserve is dpp's minimum fee, which the metered bound stays under.
#[test]
fn the_rules_are_platforms() {
    for version in versions() {
        let v = version.protocol_version;
        let rules = Rules::at(version).unwrap();
        let limits = &version.system_limits;
        assert_eq!(rules.max, limits.max_withdrawal_amount, "{v}");
        assert!(rules.min >= limits.min_withdrawal_amount, "{v}");
        let min_fee = version.fee_version.state_transition_min_fees.credit_withdrawal;
        assert_eq!(rules.fee, min_fee, "{v}: the reserve is dpp's minimum");
        let metered = withdrawal_metered_bound(version).unwrap();
        assert!(
            metered < min_fee,
            "{v}: the metered bound {metered} is under the minimum fee {min_fee}"
        );
        // A real bound: the document's storage alone and the threshold
        // signing are past 100M credits.
        assert!(metered > version.fee_version.processing.perform_network_threshold_signing);
    }
    let at = |v| Rules::at(PlatformVersion::get(v).unwrap()).unwrap();
    // v12–13: `min_withdrawal_amount`; v14+: plus the 190-byte Core fee.
    assert_eq!(at(13).min, 1_000_000);
    assert_eq!(at(14).min, 1_190_000);
    assert_eq!(at(15).min, 1_190_000);
    assert_eq!(at(15).fee, 400_000_000);
}

/// At the minimum the payout is no dust at either address kind, so Core
/// mines it (a dust payout is `FAILED` and Platform keeps the credits).
#[test]
fn the_least_withdrawal_pays_out_more_than_dust() {
    for version in versions() {
        let min = Rules::at(version).unwrap().min;
        let paid = expected_duffs(min, version).unwrap();
        assert!(paid >= p2pkh_probe().dust_value().to_sat(), "{}", version.protocol_version);
        let p2sh = CoreScript::new_p2sh([1; 20]);
        assert!(paid >= p2sh.dust_value().to_sat());
    }
    // Protocol version 11's floor was 190,000 credits: 190 duffs, dust.
    let v11 = PlatformVersion::get(11).unwrap();
    if Rules::at(v11).is_ok() {
        assert!(v11.system_limits.min_withdrawal_amount < Rules::at(v11).unwrap().min);
    }
}

/// The payout is Platform's conversion: the credits in duffs, less the
/// asset unlock's Core fee from protocol version 14 (stamped documents).
#[test]
fn the_payout_follows_the_protocol_version() {
    let at = |v| PlatformVersion::get(v).unwrap();
    assert_eq!(expected_duffs(DASH, at(13)).unwrap(), 100_000_000);
    assert_eq!(expected_duffs(DASH, at(14)).unwrap(), 100_000_000 - 190);
    assert_eq!(expected_duffs(DASH, at(15)).unwrap(), 100_000_000 - 190);
    // Credits short of a duff are not paid.
    assert_eq!(expected_duffs(1_190_999, at(15)).unwrap(), 1_000);
}

#[test]
fn max_keeps_the_reserve_in_whole_duffs() {
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    for balance in [rules.min + rules.fee, DASH + 123_456, 7 * DASH + 999] {
        let credits = rules.credits(balance, WithdrawAmount::All).unwrap();
        assert_eq!(credits % CREDITS_PER_DUFF, 0);
        assert!(balance - credits >= rules.fee, "the reserve stays");
        assert!(balance - credits < rules.fee + CREDITS_PER_DUFF, "and no more");
    }
    // Capped at Platform's maximum.
    let huge = rules.max + rules.fee + DASH;
    assert_eq!(rules.credits(huge, WithdrawAmount::All).unwrap(), rules.max);
}

#[test]
fn max_below_the_least_withdrawal_is_insufficient_credits() {
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    let balance = rules.min + rules.fee - 1;
    let refused = rules.credits(balance, WithdrawAmount::All).unwrap_err();
    assert_eq!(
        refused,
        CreditsError::Platform(PlatformError::InsufficientCredits {
            needed: rules.min + rules.fee,
            available: balance,
        })
    );
}

#[test]
fn an_amount_is_refused_outside_the_range_or_the_balance() {
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    let range = CreditsError::WithdrawalOutOfRange {
        min: rules.min,
        max: rules.max,
    };
    let ask = |balance, credits| rules.credits(balance, WithdrawAmount::Credits { credits });
    let rich = rules.max * 2;
    assert_eq!(ask(rich, rules.min - 1).unwrap_err(), range);
    assert_eq!(ask(rich, rules.max + 1).unwrap_err(), range);
    assert_eq!(ask(rich, rules.min).unwrap(), rules.min);
    assert_eq!(ask(rich, rules.max).unwrap(), rules.max);
    // The reserve counts against the balance.
    let balance = DASH + rules.fee - 1;
    assert_eq!(
        ask(balance, DASH).unwrap_err(),
        CreditsError::Platform(PlatformError::InsufficientCredits {
            needed: DASH + rules.fee,
            available: balance,
        })
    );
    assert_eq!(ask(DASH + rules.fee, DASH).unwrap(), DASH);
}

fn address(network: Network, script: &CoreScript) -> String {
    Address::from_script(script, network).unwrap().to_string()
}

#[test]
fn the_destination_is_a_p2pkh_or_p2sh_address_of_the_network() {
    let (pkh, sh) = (CoreScript::new_p2pkh([7; 20]), CoreScript::new_p2sh([8; 20]));
    for network in [Network::Testnet, Network::Mainnet, Network::Devnet, Network::Regtest] {
        assert_eq!(destination(network, &address(network, &pkh)).unwrap(), pkh);
        let padded = format!(" {} ", address(network, &sh));
        assert_eq!(destination(network, &padded).unwrap(), sh);
    }
    let refused = |network, to: &str| destination(network, to).unwrap_err().code();
    let mainnet = address(Network::Mainnet, &pkh);
    assert_eq!(refused(Network::Testnet, &mainnet), "invalid_argument");
    assert_eq!(refused(Network::Testnet, "not an address"), "invalid_argument");
    assert_eq!(refused(Network::Testnet, ""), "invalid_argument");
}

/// Drive's id for the document (its transformer's entropy).
#[test]
fn the_document_id_is_drives() {
    let script = CoreScript::new_p2pkh([7; 20]);
    let mut entropy = 5u64.to_be_bytes().to_vec();
    entropy.extend_from_slice(script.as_bytes());
    let drive = Document::generate_document_id_v0(
        &withdrawals_contract::ID,
        &id(1),
        withdrawal::NAME,
        &entropy,
    );
    assert_eq!(document_id(&id(1), 5, &script), drive);
    assert_ne!(document_id(&id(1), 6, &script), drive, "a new nonce, a new withdrawal");
}

// ---- The write, on a scripted Platform --------------------------------------

/// What `broadcast` answers.
enum Answer {
    /// Platform's proved new balance: the balance less the amount and `fee`.
    Applied { fee: u64 },
    Refused,
    Unknown,
    /// No answer, though the withdrawal landed.
    LandedUnanswered,
}

/// Platform as a withdrawal sees it.
struct FakeWithdrawals {
    version: &'static PlatformVersion,
    balance: Mutex<Option<u64>>,
    nonce: AtomicU64,
    answer: Mutex<Answer>,
    /// The transitions broadcast.
    sent: Mutex<Vec<StateTransition>>,
    /// The withdrawal documents on Platform.
    documents: Mutex<Vec<Document>>,
    /// The document reads fail.
    reads_fail: Mutex<bool>,
}

impl FakeWithdrawals {
    fn new(balance: u64) -> Arc<Self> {
        Arc::new(Self {
            version: PlatformVersion::latest(),
            balance: Mutex::new(Some(balance)),
            nonce: AtomicU64::new(4),
            answer: Mutex::new(Answer::Applied { fee: 210_000_000 }),
            sent: Mutex::default(),
            documents: Mutex::default(),
            reads_fail: Mutex::new(false),
        })
    }

    fn sent(&self) -> Vec<StateTransition> {
        self.sent.lock().unwrap().clone()
    }

    /// The document Drive writes for `transition`, as it lands.
    fn land(&self, transition: &StateTransition) {
        let StateTransition::IdentityCreditWithdrawal(w) = transition else {
            panic!("not a withdrawal");
        };
        let script = w.output_script().unwrap();
        let mut document = drive_document(w.amount(), &script, self.version).unwrap();
        let Document::V0(d) = &mut document;
        d.id = document_id(&w.identity_id(), w.nonce(), &script);
        d.owner_id = w.identity_id();
        self.documents.lock().unwrap().push(document);
    }
}

impl WithdrawNet for FakeWithdrawals {
    fn version(&self) -> &'static PlatformVersion {
        self.version
    }

    async fn balance(&self, _: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(*self.balance.lock().unwrap())
    }

    async fn nonce(&self, _: Identifier) -> Result<u64, PlatformError> {
        Ok(self.nonce.fetch_add(1, Ordering::SeqCst) + 1)
    }

    async fn broadcast(&self, transition: StateTransition) -> Result<u64, Unsent> {
        self.sent.lock().unwrap().push(transition.clone());
        let StateTransition::IdentityCreditWithdrawal(w) = &transition else {
            panic!("not a withdrawal");
        };
        match *self.answer.lock().unwrap() {
            Answer::Applied { fee } => {
                self.land(&transition);
                let mut balance = self.balance.lock().unwrap();
                let after = balance.unwrap() - w.amount() - fee;
                *balance = Some(after);
                Ok(after)
            }
            Answer::Refused => Err(Unsent::Refused(PlatformError::InsufficientCredits {
                needed: 1,
                available: 0,
            })),
            Answer::Unknown => Err(Unsent::Unknown(PlatformError::Timeout)),
            Answer::LandedUnanswered => {
                self.land(&transition);
                Err(Unsent::Unknown(PlatformError::Timeout))
            }
        }
    }

    async fn withdrawal(&self, id: Identifier) -> Result<Option<Document>, PlatformError> {
        if *self.reads_fail.lock().unwrap() {
            return Err(PlatformError::Unavailable);
        }
        Ok((self.documents.lock().unwrap().iter())
            .find(|d| d.id() == id)
            .cloned())
    }

    async fn withdrawals(&self, owner: Identifier, limit: u32) -> Result<Vec<Document>, PlatformError> {
        let documents = self.documents.lock().unwrap();
        Ok((documents.iter().rev())
            .filter(|d| d.owner_id() == owner)
            .take(limit as usize)
            .cloned()
            .collect())
    }
}

const ME: u8 = 1;

struct World {
    f: Fixture,
    net: Arc<FakeWithdrawals>,
    /// A P2PKH address of the session's network.
    to: String,
    script: CoreScript,
}

impl World {
    fn new(balance: u64) -> Self {
        Self::of(Fixture::new(balance), balance)
    }

    fn of(f: Fixture, balance: u64) -> Self {
        let script = CoreScript::new_p2pkh([7; 20]);
        let to = address(f.s.vault.network(), &script);
        Self {
            f,
            net: FakeWithdrawals::new(balance),
            to,
            script,
        }
    }

    fn call<T: Send + 'static, Fut>(
        &self,
        body: impl FnOnce(Arc<FakeWithdrawals>, Arc<NetworkSession>, Arc<PlatformWallet>) -> Fut
        + Send
        + 'static,
    ) -> Result<T, CreditsError>
    where
        Fut: Future<Output = Result<T, CreditsError>> + Send + 'static,
    {
        let net = Arc::clone(&self.net);
        let dp = self.f.s.dashpay(self.f.wallet);
        (self.f.engine).block_on(dp.on_wallet(move |s, _, w| body(net, s, w)))
    }

    fn quote(&self, amount: WithdrawAmount) -> Result<WithdrawQuote, CreditsError> {
        let me = id(ME).to_string(Encoding::Base58);
        self.call(move |net, _, w| async move { quote(&*net, &w, &me, amount).await })
    }

    fn withdraw_to(
        &self,
        to: &str,
        amount: WithdrawAmount,
        grant: &str,
    ) -> Result<WithdrawOutcome, CreditsError> {
        let (wallet, to, grant) = (self.f.wallet, to.to_owned(), grant.to_owned());
        let me = id(ME).to_string(Encoding::Base58);
        self.call(move |net, s, w| async move {
            withdraw(&*net, &s, &w, (wallet, &me), (&to, amount), &grant).await
        })
    }

    fn withdraw(&self, amount: WithdrawAmount, grant: &str) -> Result<WithdrawOutcome, CreditsError> {
        self.withdraw_to(&self.to.clone(), amount, grant)
    }

    fn recent(&self) -> Result<Vec<Withdrawal>, CreditsError> {
        let me = id(ME).to_string(Encoding::Base58);
        let network = self.f.s.vault.network();
        self.call(move |net, _, w| async move { recent(&*net, &w, network, &me).await })
    }

    /// Our identity as the library holds it.
    fn me(&self) -> Identity {
        let manager = self.f.s.manager().unwrap();
        let wallet = self.f.wallet;
        self.f.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet.0).await.unwrap();
            let state = wallet.state().await;
            state.identity_manager.managed_identity(&id(ME)).unwrap().identity.clone()
        })
    }
}

fn rules() -> Rules {
    Rules::at(PlatformVersion::latest()).unwrap()
}

#[test]
fn the_quote_is_the_plan_and_its_grant() {
    let balance = 3 * DASH + 5;
    let w = World::new(balance);
    let q = w.quote(WithdrawAmount::All).unwrap();
    let rules = rules();
    assert_eq!(q.credits, (balance - rules.fee) / 1000 * 1000);
    assert_eq!(q.fee_credits, rules.fee);
    assert_eq!(q.expected_duffs, q.credits / 1000 - 190);
    assert_eq!(q.grant, GrantRequest {
        max_duffs: 0,
        max_credits: q.credits + q.fee_credits,
    });
    let q = w.quote(WithdrawAmount::Credits { credits: DASH }).unwrap();
    assert_eq!((q.credits, q.expected_duffs), (DASH, 100_000_000 - 190));
    let refused = w.quote(WithdrawAmount::Credits { credits: 3 * DASH }).unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");
    let refused = w.quote(WithdrawAmount::Credits { credits: 1 }).unwrap_err();
    assert_eq!(refused.code(), "credits.withdrawal_out_of_range");
}

/// The transition the write broadcasts is, byte for byte, dpp's own
/// construction with the SDK's choices (`withdraw_with_metadata`: pooling
/// `Never`, rate 1, no fee increase, `TransferPreferred` with no key
/// given), signed with the TRANSFER key, and its signature verifies.
#[test]
fn the_transition_is_platforms_own_construction() {
    let balance = 2 * DASH;
    let w = World::new(balance);
    let q = w.quote(WithdrawAmount::All).unwrap();
    let grant = w.f.grant(q.grant.max_credits);
    let out = w.withdraw(WithdrawAmount::All, &grant).unwrap();
    let sent = w.net.sent();
    let [signed] = sent.as_slice() else {
        panic!("one broadcast: {sent:?}");
    };

    let me = w.me();
    let key = &me.public_keys()[&3];
    assert_eq!(signed.signature_public_key_id(), Some(3), "the TRANSFER key");
    let StateTransition::IdentityCreditWithdrawal(t) = signed else {
        panic!("a withdrawal");
    };
    assert_eq!(t.amount(), q.credits);
    assert_eq!(t.output_script(), Some(w.script.clone()));
    assert_eq!(t.pooling(), Pooling::Never);
    assert_eq!(t.core_fee_per_byte(), 1);
    assert_eq!(t.user_fee_increase(), 0);
    signed
        .verify_identity_signed_signature(key, &NativeBlsModule, PlatformVersion::latest())
        .expect("the signature verifies with the TRANSFER key");

    // dpp's construction, the SDK's call, by a signer of the same vault.
    let lease_id = w.f.flow(FlowKind::Withdraw, vec![w.f.grant(1)]);
    let lease = w.f.s.lease_for(&w.f.wallet, &lease_id).unwrap().unwrap();
    let signer = lease.identity_signer(&[0]).unwrap();
    let (script, nonce) = (w.script.clone(), t.nonce());
    let platforms = w.f.engine.block_on(async move {
        IdentityCreditWithdrawalTransition::try_from_identity(
            &me,
            Some(script),
            q.credits,
            Pooling::Never,
            1,
            0,
            signer,
            None,
            PreferredKeyPurposeForSigningWithdrawal::TransferPreferred,
            nonce,
            PlatformVersion::latest(),
            None,
        )
        .await
        .unwrap()
    });
    lease.end();
    assert_eq!(
        signed.serialize_to_bytes().unwrap(),
        platforms.serialize_to_bytes().unwrap()
    );

    let rules = rules();
    assert_eq!(out.credits, q.credits);
    assert_eq!(out.expected_duffs, Some(q.expected_duffs));
    assert_eq!(out.remaining_credits, Some(balance - q.credits - 210_000_000));
    assert!(out.remaining_credits.unwrap() >= rules.fee - 210_000_000);
    let doc = document_id(&id(ME), nonce, &w.script);
    assert_eq!(out.withdrawal, doc.to_string(Encoding::Base58));
}

/// Every refusal before the signature leaves the grant unused and sends
/// nothing.
#[test]
fn refusals_keep_the_grant() {
    let balance = 2 * DASH;
    let w = World::new(balance);
    let fee = rules().fee;
    let grant = w.f.grant(balance);
    let code = |r: Result<WithdrawOutcome, CreditsError>| r.unwrap_err().code();
    let ask = |credits| WithdrawAmount::Credits { credits };
    assert_eq!(code(w.withdraw(ask(1), &grant)), "credits.withdrawal_out_of_range");
    assert_eq!(code(w.withdraw(ask(balance), &grant)), "platform.insufficient_credits");
    let mainnet = address(Network::Mainnet, &w.script);
    assert_eq!(code(w.withdraw_to(&mainnet, ask(DASH), &grant)), "invalid_argument");
    assert!(w.f.unused(&grant));
    // A grant short of the amount plus the reserve.
    let small = w.f.grant(DASH + fee - 1);
    assert_eq!(code(w.withdraw(ask(DASH), &small)), "platform.grant_exceeded");
    assert!(w.f.unused(&small));
    // A lease of another flow.
    let other = w.f.flow(FlowKind::ProfileEdit, vec![w.f.grant(balance)]);
    assert_eq!(code(w.withdraw(ask(DASH), &other)), "platform.grant_invalid");
    assert!(w.net.sent().is_empty());
    assert!(w.f.unused(&grant));
    // Then the same grant withdraws.
    w.withdraw(ask(DASH), &grant).unwrap();
    assert_eq!(w.net.sent().len(), 1);
}

/// With no TRANSFER key the vault holds, `identity.keys_missing{transfer}`
/// before the grant is redeemed; an identity with no TRANSFER key at all
/// the same.
#[test]
fn a_withdrawal_needs_a_held_transfer_key() {
    let balance = 2 * DASH;
    for f in [
        Fixture::foreign(keys(true), balance),
        Fixture::with_adjusted_keys(balance, |k| {
            k.remove(&3);
        }),
    ] {
        let w = World::of(f, balance);
        let grant = w.f.grant(balance);
        let refused = w.withdraw(WithdrawAmount::Credits { credits: DASH }, &grant);
        assert_eq!(
            refused.unwrap_err(),
            CreditsError::Platform(PlatformError::Identity(IdentityError::KeysMissing {
                purpose: KeyPurpose::Transfer,
            }))
        );
        assert!(w.f.unused(&grant));
        assert!(w.net.sent().is_empty());
    }
}

/// Rule 5 (DP3-04): an identity Platform does not verify withdraws
/// nothing, and the grant is kept.
#[test]
fn an_unverified_identity_withdraws_nothing() {
    let w = World::new(2 * DASH);
    gate_session(&w.f.s, TrustMode::Enforcing, FakePlatform::new(Proof::Mismatch));
    let grant = w.f.grant(2 * DASH);
    let refused = w.withdraw(WithdrawAmount::Credits { credits: DASH }, &grant);
    assert_eq!(refused.unwrap_err().code(), "platform.trust_mismatch");
    assert!(w.f.unused(&grant));
    assert!(w.net.sent().is_empty());
}

/// Signed, refused by Platform: that refusal. Signed, unanswered: the
/// document decides; none read back is `platform.broadcast_unknown`
/// naming it, and nothing is sent again.
#[test]
fn a_failed_broadcast_after_signing() {
    let w = World::new(5 * DASH);
    let ask = WithdrawAmount::Credits { credits: DASH };

    *w.net.answer.lock().unwrap() = Answer::Refused;
    let refused = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");

    *w.net.answer.lock().unwrap() = Answer::Unknown;
    *w.net.reads_fail.lock().unwrap() = true;
    let unknown = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap_err();
    let StateTransition::IdentityCreditWithdrawal(t) = &w.net.sent()[1] else {
        panic!("a withdrawal");
    };
    let doc = document_id(&id(ME), t.nonce(), &w.script).to_string(Encoding::Base58);
    assert_eq!(
        unknown,
        CreditsError::Platform(PlatformError::BroadcastUnknown { artifact: doc })
    );

    // Landed after all: read back, with no proved balance.
    *w.net.reads_fail.lock().unwrap() = false;
    *w.net.answer.lock().unwrap() = Answer::LandedUnanswered;
    let out = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!((out.credits, out.remaining_credits), (DASH, None));
    assert_eq!(w.net.sent().len(), 3, "nothing sent again");
}

#[test]
fn recent_withdrawals_are_platforms_documents_newest_first() {
    let w = World::new(5 * DASH);
    assert!(w.recent().unwrap().is_empty());
    let grant = w.f.grant(2 * DASH);
    let first = w.withdraw(WithdrawAmount::Credits { credits: DASH }, &grant).unwrap();
    let grant = w.f.grant(5 * DASH);
    let second = w.withdraw(WithdrawAmount::All, &grant).unwrap();
    // Platform pools the first.
    {
        let Document::V0(d) = &mut w.net.documents.lock().unwrap()[0];
        d.properties.insert(
            withdrawal::properties::STATUS.into(),
            Value::U8(DocStatus::POOLED as u8),
        );
        d.updated_at = Some(2);
    }
    let recent = w.recent().unwrap();
    let ids: Vec<_> = recent.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, [first.withdrawal.as_str(), second.withdrawal.as_str()]);
    assert_eq!(recent[0].status, WithdrawalStatus::Pooled);
    assert_eq!(recent[1].status, WithdrawalStatus::Queued);
    assert_eq!(recent[0].credits, DASH);
    assert_eq!(recent[0].expected_duffs, Some(100_000_000 - 190));
    assert_eq!(recent[1].expected_duffs, second.expected_duffs);
    assert_eq!(recent[0].to.as_deref(), Some(w.to.as_str()));
}
