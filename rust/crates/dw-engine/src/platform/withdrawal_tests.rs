//! Tests of withdrawal.rs (DP6-02): Platform's rules at every known
//! protocol version (the range, the fee reserve, the payout), "Max", the
//! destination, and the write on requests_tests.rs's fixture with a
//! scripted Platform ([`FakeWithdrawals`]): the transition's bytes against
//! dpp's own construction, the refusals that keep the grant, rule 5, and a
//! broadcast that failed after signing. The devnet round trip is T2
//! (`/work/scratch/dw-dp6-02/t2`).

use std::collections::{BTreeMap, BTreeSet};

use dpp::identity::identity_nonce::IDENTITY_NONCE_VALUE_FILTER_MAX_BYTES;
use std::sync::Mutex;

use dpp::identity::identity_public_key::accessors::v0::{
    IdentityPublicKeyGettersV0, IdentityPublicKeySettersV0,
};
use dpp::identity::{KeyType, Purpose, SecurityLevel};
use dpp::serialization::{PlatformSerializable, Signable};
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
        let min_fee = version
            .fee_version
            .state_transition_min_fees
            .credit_withdrawal;
        assert_eq!(rules.fee, min_fee, "{v}: the reserve is dpp's minimum");
        let metered = withdrawal_metered_bound(version).unwrap();
        assert!(
            metered < min_fee,
            "{v}: the metered bound {metered} is under the minimum fee {min_fee}"
        );
        // A real bound: the document's storage alone and the threshold
        // signing are past 100M credits.
        assert!(
            metered
                > version
                    .fee_version
                    .processing
                    .perform_network_threshold_signing
        );
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
        assert!(
            paid >= p2pkh_probe().dust_value().to_sat(),
            "{}",
            version.protocol_version
        );
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
        assert!(
            balance - credits < rules.fee + CREDITS_PER_DUFF,
            "and no more"
        );
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
    let (pkh, sh) = (
        CoreScript::new_p2pkh([7; 20]),
        CoreScript::new_p2sh([8; 20]),
    );
    for network in [
        Network::Testnet,
        Network::Mainnet,
        Network::Devnet,
        Network::Regtest,
    ] {
        assert_eq!(destination(network, &address(network, &pkh)).unwrap(), pkh);
        let padded = format!(" {} ", address(network, &sh));
        assert_eq!(destination(network, &padded).unwrap(), sh);
    }
    let refused = |network, to: &str| destination(network, to).unwrap_err().code();
    let mainnet = address(Network::Mainnet, &pkh);
    assert_eq!(refused(Network::Testnet, &mainnet), "invalid_argument");
    assert_eq!(
        refused(Network::Testnet, "not an address"),
        "invalid_argument"
    );
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
    assert_ne!(
        document_id(&id(1), 6, &script),
        drive,
        "a new nonce, a new withdrawal"
    );
}

// ---- The write, on a scripted Platform --------------------------------------

/// What `broadcast` answers.
enum Answer {
    /// It executes: Platform's proved new balance, the balance less the
    /// amount and `fee`; Platform's refusal when Drive refuses it.
    Applied { fee: u64 },
    /// Refused at execution: the nonce is spent.
    Refused,
    /// No answer, and it never executes.
    Unknown,
    /// No answer, and it sits in a mempool until `execute_pending`.
    Pending,
    /// No answer, though the withdrawal executed.
    LandedUnanswered,
    /// A node that says refused, though the withdrawal executed.
    LandedRefused,
    /// A proved balance, unchanged, and nothing executed.
    LyingSuccess,
    /// A proved balance from before it executed.
    StaleSuccess,
    /// No answer; the transitions pending meanwhile execute, and this one
    /// never does.
    EarlierLands,
}

/// What a withdrawal Drive executes is charged on top of its amount.
const FEE: u64 = 210_000_000;

/// Platform as a withdrawal sees it.
struct FakeWithdrawals {
    /// What `version` answers: the seed until a proved read.
    version: Mutex<&'static PlatformVersion>,
    /// The network's protocol version, which a proved read ratchets to.
    proved: &'static PlatformVersion,
    balance: Mutex<Option<u64>>,
    /// The identity's nonces Drive took.
    used: Mutex<BTreeSet<u64>>,
    /// The nonce state a node behind the chain answers, if one does.
    lag: Mutex<Option<u64>>,
    answer: Mutex<Answer>,
    /// The transitions broadcast.
    sent: Mutex<Vec<StateTransition>>,
    /// The transitions in a mempool.
    pending: Mutex<Vec<StateTransition>>,
    /// The withdrawal documents on Platform.
    documents: Mutex<Vec<Document>>,
    /// The document reads fail.
    reads_fail: Mutex<bool>,
}

fn withdrawal_of(transition: &StateTransition) -> &IdentityCreditWithdrawalTransition {
    let StateTransition::IdentityCreditWithdrawal(w) = transition else {
        panic!("not a withdrawal");
    };
    w
}

fn nonce_of(transition: &StateTransition) -> u64 {
    withdrawal_of(transition).nonce()
}

impl FakeWithdrawals {
    fn new(balance: u64) -> Arc<Self> {
        Arc::new(Self {
            version: Mutex::new(PlatformVersion::latest()),
            proved: PlatformVersion::latest(),
            balance: Mutex::new(Some(balance)),
            used: Mutex::new((1..=4).collect()),
            lag: Mutex::default(),
            answer: Mutex::new(Answer::Applied { fee: FEE }),
            sent: Mutex::default(),
            pending: Mutex::default(),
            documents: Mutex::default(),
            reads_fail: Mutex::new(false),
        })
    }

    /// A proved read: the SDK takes the network's version.
    fn ratchet(&self) {
        *self.version.lock().unwrap() = self.proved;
    }

    /// The SDK back at its regtest seed, protocol 13, as a fresh process
    /// starts.
    fn reseed(&self) {
        *self.version.lock().unwrap() = PlatformVersion::get(13).unwrap();
    }

    fn sent(&self) -> Vec<StateTransition> {
        self.sent.lock().unwrap().clone()
    }

    fn answer(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
    }

    /// A node `state` behind the chain answers the nonce reads.
    fn lag(&self, state: u64) {
        *self.lag.lock().unwrap() = Some(state);
    }

    /// The withdrawal documents on Platform.
    fn landed(&self) -> usize {
        self.documents.lock().unwrap().len()
    }

    /// The nonce as Drive stores it: the newest, and a bit for each of the
    /// 24 below it not yet used (`validate_identity_nonce_update`).
    fn state(&self) -> u64 {
        let used = self.used.lock().unwrap();
        let tip = used.last().copied().unwrap_or(0);
        (1..=24u64)
            .filter(|&p| tip > p && !used.contains(&(tip - p)))
            .fold(tip, |state, p| {
                state | 1 << (p - 1 + IDENTITY_NONCE_VALUE_FILTER_MAX_BYTES)
            })
    }

    /// Another identity-nonce write (a key update) executes at `nonce`.
    fn other_write(&self, nonce: u64) {
        assert!(validate_identity_nonce_update(self.state(), nonce, id(ME)).is_valid());
        self.used.lock().unwrap().insert(nonce);
    }

    /// Drive executes `transition` by its nonce rule: the nonce taken, the
    /// credits and `fee` charged, the document written. False when Drive
    /// refuses it.
    fn execute(&self, transition: &StateTransition, fee: u64) -> bool {
        let w = withdrawal_of(transition);
        if !validate_identity_nonce_update(self.state(), w.nonce(), w.identity_id()).is_valid() {
            return false;
        }
        self.used.lock().unwrap().insert(w.nonce());
        let mut balance = self.balance.lock().unwrap();
        let Some(after) = balance.unwrap().checked_sub(w.amount() + fee) else {
            return false;
        };
        *balance = Some(after);
        let script = w.output_script().unwrap();
        let mut document = drive_document(w.amount(), &script, self.proved).unwrap();
        let Document::V0(d) = &mut document;
        d.id = document_id(&w.identity_id(), w.nonce(), &script);
        d.owner_id = w.identity_id();
        self.documents.lock().unwrap().push(document);
        true
    }

    /// The mempool's transitions reach a block.
    fn execute_pending(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for t in &pending {
            self.execute(t, FEE);
        }
    }

    fn balance_now(&self) -> u64 {
        self.balance.lock().unwrap().unwrap()
    }
}

impl WithdrawNet for FakeWithdrawals {
    fn version(&self) -> &'static PlatformVersion {
        *self.version.lock().unwrap()
    }

    async fn balance(&self, _: Identifier) -> Result<Option<u64>, PlatformError> {
        self.ratchet();
        Ok(*self.balance.lock().unwrap())
    }

    async fn nonce_state(&self, _: Identifier) -> Result<u64, PlatformError> {
        let lag = *self.lag.lock().unwrap();
        Ok(lag.unwrap_or_else(|| self.state()))
    }

    async fn broadcast(&self, transition: StateTransition) -> Result<u64, Unsent> {
        self.sent.lock().unwrap().push(transition.clone());
        let refused = || {
            Unsent::Refused(PlatformError::InsufficientCredits {
                needed: 1,
                available: 0,
            })
        };
        let unknown = || Unsent::Unknown(PlatformError::Timeout);
        match *self.answer.lock().unwrap() {
            Answer::Applied { fee } if self.execute(&transition, fee) => Ok(self.balance_now()),
            Answer::Applied { .. } => Err(refused()),
            Answer::Refused => {
                self.used
                    .lock()
                    .unwrap()
                    .insert(withdrawal_of(&transition).nonce());
                Err(refused())
            }
            Answer::Unknown => Err(unknown()),
            Answer::Pending => {
                self.pending.lock().unwrap().push(transition);
                Err(unknown())
            }
            Answer::LandedUnanswered => {
                assert!(self.execute(&transition, FEE));
                Err(unknown())
            }
            Answer::LandedRefused => {
                assert!(self.execute(&transition, FEE));
                Err(refused())
            }
            Answer::LyingSuccess => Ok(self.balance_now()),
            Answer::StaleSuccess => {
                let before = self.balance_now();
                assert!(self.execute(&transition, FEE));
                Ok(before)
            }
            Answer::EarlierLands => {
                self.execute_pending();
                Err(unknown())
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

    async fn withdrawals(
        &self,
        owner: Identifier,
        limit: u32,
    ) -> Result<Vec<Document>, PlatformError> {
        self.ratchet();
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

    fn withdraw(
        &self,
        amount: WithdrawAmount,
        grant: &str,
    ) -> Result<WithdrawOutcome, CreditsError> {
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
            state
                .identity_manager
                .managed_identity(&id(ME))
                .unwrap()
                .identity
                .clone()
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
    assert_eq!(
        q.grant,
        GrantRequest {
            max_duffs: 0,
            max_credits: q.credits + q.fee_credits,
        }
    );
    let q = w.quote(WithdrawAmount::Credits { credits: DASH }).unwrap();
    assert_eq!((q.credits, q.expected_duffs), (DASH, 100_000_000 - 190));
    let refused = w
        .quote(WithdrawAmount::Credits { credits: 3 * DASH })
        .unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");
    let refused = w.quote(WithdrawAmount::Credits { credits: 1 }).unwrap_err();
    assert_eq!(refused.code(), "credits.withdrawal_out_of_range");
}

/// T2 D17: until a proved read ratchets it, the SDK answers its seed
/// (protocol 13 on regtest), whose range and payout are not the network's:
/// the rules, the payout and a document's are read after the proved read.
#[test]
fn the_rules_are_the_proved_versions_not_the_seed() {
    let w = World::new(3 * DASH);
    w.net.reseed();
    // Inside protocol 13's range, below the network's.
    let refused = w
        .quote(WithdrawAmount::Credits { credits: 1_100_000 })
        .unwrap_err();
    assert_eq!(refused.code(), "credits.withdrawal_out_of_range");
    w.net.reseed();
    let q = w.quote(WithdrawAmount::Credits { credits: DASH }).unwrap();
    assert_eq!(q.expected_duffs, 100_000_000 - 190);
    let grant = w.f.grant(q.grant.max_credits);
    w.net.reseed();
    let out = w
        .withdraw(WithdrawAmount::Credits { credits: DASH }, &grant)
        .unwrap();
    assert_eq!(out.expected_duffs, Some(100_000_000 - 190));
    w.net.reseed();
    assert_eq!(
        w.recent().unwrap()[0].expected_duffs,
        Some(100_000_000 - 190)
    );
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
    assert_eq!(
        signed.signature_public_key_id(),
        Some(3),
        "the TRANSFER key"
    );
    let StateTransition::IdentityCreditWithdrawal(t) = signed else {
        panic!("a withdrawal");
    };
    assert_eq!(t.amount(), q.credits);
    assert_eq!(t.output_script(), Some(w.script.clone()));
    assert_eq!(t.pooling(), Pooling::Never);
    assert_eq!(t.core_fee_per_byte(), 1);
    assert_eq!(t.user_fee_increase(), 0);
    assert_eq!(key.key_type(), KeyType::ECDSA_SECP256K1);
    assert_eq!(key.purpose(), Purpose::TRANSFER);
    assert_eq!(key.security_level(), SecurityLevel::CRITICAL);
    dashcore::signer::verify_data_signature(
        &signed.signable_bytes().unwrap(),
        signed.signature().expect("signed").as_slice(),
        key.data().as_slice(),
    )
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
    assert_eq!(
        out.remaining_credits,
        Some(balance - q.credits - 210_000_000)
    );
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
    assert_eq!(
        code(w.withdraw(ask(1), &grant)),
        "credits.withdrawal_out_of_range"
    );
    assert_eq!(
        code(w.withdraw(ask(balance), &grant)),
        "platform.insufficient_credits"
    );
    let mainnet = address(Network::Mainnet, &w.script);
    assert_eq!(
        code(w.withdraw_to(&mainnet, ask(DASH), &grant)),
        "invalid_argument"
    );
    assert!(w.f.unused(&grant));
    // A grant short of the amount plus the reserve.
    let small = w.f.grant(DASH + fee - 1);
    assert_eq!(
        code(w.withdraw(ask(DASH), &small)),
        "platform.grant_exceeded"
    );
    assert!(w.f.unused(&small));
    // A lease of another flow.
    let other = w.f.flow(FlowKind::ProfileEdit, vec![w.f.grant(balance)]);
    assert_eq!(
        code(w.withdraw(ask(DASH), &other)),
        "platform.grant_invalid"
    );
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
    gate_session(
        &w.f.s,
        TrustMode::Enforcing,
        FakePlatform::new(Proof::Mismatch),
    );
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

fn unconfirmed(w: &World, nonce: u64) -> CreditsError {
    let artifact = document_id(&id(ME), nonce, &w.script).to_string(Encoding::Base58);
    CreditsError::Platform(PlatformError::BroadcastUnknown { artifact })
}

/// Review r1 m1 (D24): a withdrawal retried in the session after an
/// unknown outcome signs at the first one's nonce while Platform would
/// still take it, even past another identity-nonce write, so at most one
/// of the two executes. At ae89f4de the retry took the SDK cache's next
/// nonce and both executed.
#[test]
fn a_retry_after_an_unknown_outcome_cannot_execute_twice() {
    let balance = 10 * DASH;
    let w = World::new(balance);
    let grant = || w.f.grant(4 * DASH);
    w.net.answer(Answer::Pending);
    let first = w.withdraw(WithdrawAmount::Credits { credits: DASH }, &grant());
    let k = nonce_of(&w.net.sent()[0]);
    // A key update executes at the SDK cache's next nonce meanwhile.
    w.net.other_write(k + 1);
    // The retry, of another amount, still unanswered.
    let again = w.withdraw(WithdrawAmount::Credits { credits: 2 * DASH }, &grant());
    // Both reach a block.
    w.net.execute_pending();
    assert_eq!(w.net.landed(), 1, "one executed");
    assert_eq!(w.net.balance_now(), balance - DASH - FEE);
    assert_eq!(nonce_of(&w.net.sent()[1]), k, "the first one's nonce");
    assert_eq!(first.unwrap_err(), unconfirmed(&w, k));
    assert_eq!(again.unwrap_err(), unconfirmed(&w, k));
    // Seen executed, the nonce is not offered again.
    w.net.answer(Answer::Applied { fee: FEE });
    w.withdraw(WithdrawAmount::Credits { credits: DASH }, &grant())
        .unwrap();
    assert_eq!(nonce_of(&w.net.sent()[2]), k + 2);
    assert_eq!(w.recent().unwrap().len(), 2);
}

/// D24: after a withdrawal seen executed, a node behind the chain answers
/// the nonce from before it; the next withdrawal signs past it anyway, so
/// it is not refused and then mistaken for the first one's document.
#[test]
fn a_node_behind_a_seen_withdrawal_does_not_bring_its_nonce_back() {
    let w = World::new(10 * DASH);
    let ask = WithdrawAmount::Credits { credits: DASH };
    let before = w.net.state();
    let first = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap();
    w.net.lag(before);
    let second = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap();
    assert_ne!(second.withdrawal, first.withdrawal);
    assert_eq!(second.remaining_credits, Some(10 * DASH - 2 * (DASH + FEE)));
    assert_eq!(w.net.landed(), 2);
}

/// Review r1 m2: a proved balance is the identity's state after the wait,
/// not proof the transition executed. One that did not drop by the amount
/// is no success: the document decides, and with none it is unconfirmed
/// and a retry signs at its nonce.
#[test]
fn a_success_without_the_balance_drop_is_unconfirmed() {
    let balance = 10 * DASH;
    let w = World::new(balance);
    let ask = WithdrawAmount::Credits { credits: DASH };
    w.net.answer(Answer::LyingSuccess);
    let lied = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap_err();
    let k = nonce_of(&w.net.sent()[0]);
    assert_eq!(lied, unconfirmed(&w, k));
    assert_eq!(w.net.landed(), 0);
    // The answer's balance is from before it executed: read back.
    w.net.answer(Answer::StaleSuccess);
    let out = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!(nonce_of(&w.net.sent()[1]), k);
    assert_eq!((out.credits, out.remaining_credits), (DASH, None));
    assert_eq!(w.net.balance_now(), balance - DASH - FEE);
}

/// The document read back counts only with this withdrawal's amount: a
/// retry at the same nonce and address of another amount names the same
/// document, and the earlier one's landing is not this one's.
#[test]
fn a_document_of_another_amount_is_not_this_withdrawal() {
    let w = World::new(10 * DASH);
    w.net.answer(Answer::Pending);
    let first = w.withdraw(
        WithdrawAmount::Credits { credits: DASH },
        &w.f.grant(2 * DASH),
    );
    let k = nonce_of(&w.net.sent()[0]);
    assert_eq!(first.unwrap_err(), unconfirmed(&w, k));
    w.net.answer(Answer::EarlierLands);
    let retry = w.withdraw(
        WithdrawAmount::Credits { credits: 2 * DASH },
        &w.f.grant(3 * DASH),
    );
    assert_eq!(retry.unwrap_err(), unconfirmed(&w, k));
    let recent = w.recent().unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].credits, Some(DASH));
}

/// Review r1 m3 (D21): a refusal is one node's unproved answer, so the
/// document is read back; one there is the withdrawal, done. Only a
/// refusal with no document read back is the refusal.
#[test]
fn a_refusal_with_the_document_on_platform_is_done() {
    let w = World::new(10 * DASH);
    let ask = WithdrawAmount::Credits { credits: DASH };
    w.net.answer(Answer::LandedRefused);
    let out = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap();
    assert_eq!((out.credits, out.remaining_credits), (DASH, None));
    w.net.answer(Answer::Refused);
    let refused = w.withdraw(ask, &w.f.grant(2 * DASH)).unwrap_err();
    assert_eq!(refused.code(), "platform.insufficient_credits");
}

/// Review r1 m4: the withdrawal signs with the CRITICAL TRANSFER key and
/// never with an OWNER key or a TRANSFER key of another level, which the
/// vault holds too (keys 1 and 2, relabelled; lower ids than key 3).
#[test]
fn only_a_critical_transfer_key_signs_a_withdrawal() {
    let balance = 2 * DASH;
    let relabel = |k: &mut BTreeMap<u32, IdentityPublicKey>| {
        let mut set = |id, purpose, level| {
            let key = k.get_mut(&id).unwrap();
            key.set_purpose(purpose);
            key.set_security_level(level);
        };
        set(1, Purpose::TRANSFER, SecurityLevel::HIGH);
        set(2, Purpose::OWNER, SecurityLevel::CRITICAL);
    };
    let w = World::of(Fixture::with_adjusted_keys(balance, relabel), balance);
    let grant = w.f.grant(balance);
    w.withdraw(WithdrawAmount::Credits { credits: DASH }, &grant)
        .unwrap();
    assert_eq!(w.net.sent()[0].signature_public_key_id(), Some(3));

    let w = World::of(
        Fixture::with_adjusted_keys(balance, |k| {
            relabel(k);
            k.remove(&3);
        }),
        balance,
    );
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

#[test]
fn recent_withdrawals_are_platforms_documents_newest_first() {
    let w = World::new(5 * DASH);
    assert!(w.recent().unwrap().is_empty());
    let grant = w.f.grant(2 * DASH);
    let first = w
        .withdraw(WithdrawAmount::Credits { credits: DASH }, &grant)
        .unwrap();
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
    assert_eq!(recent[0].credits, Some(DASH));
    assert_eq!(recent[0].expected_duffs, Some(100_000_000 - 190));
    assert_eq!(recent[1].expected_duffs, second.expected_duffs);
    assert_eq!(recent[0].to.as_deref(), Some(w.to.as_str()));
}

/// D21: only a refusal that carries Platform's consensus error means the
/// withdrawal did not happen. DAPI's failed wait (a broadcast error with a
/// code and no cause), a timeout, and a CheckTx consensus error (a resend's
/// refusal can follow a first send Platform took) are all `Unknown`, so the
/// withdrawal is read back and never offered again blindly.
#[test]
fn only_a_consensus_refusal_says_nothing_happened() {
    use dash_sdk::error::StateTransitionBroadcastError as Broadcast;
    use dpp::consensus::ConsensusError;
    use dpp::consensus::codes::ErrorWithCode;
    use dpp::consensus::state::identity::IdentityInsufficientBalanceError;
    use dpp::identity::PartialIdentity;
    let cause: ConsensusError =
        IdentityInsufficientBalanceError::new(Identifier::from([9; 32]), 1, 2).into();
    let refused = |cause: Option<ConsensusError>| {
        dash_sdk::Error::StateTransitionBroadcastError(Broadcast {
            code: cause.as_ref().map_or(13, |c| c.code()),
            message: "x".into(),
            cause,
        })
    };
    let kind = |r: Result<u64, Unsent>| match r {
        Ok(balance) => format!("ok {balance}"),
        Err(Unsent::Refused(_)) => "refused".into(),
        Err(Unsent::Unknown(_)) => "unknown".into(),
    };
    let identity = |balance| {
        StateTransitionProofResult::VerifiedPartialIdentity(PartialIdentity {
            id: Identifier::from([9; 32]),
            loaded_public_keys: Default::default(),
            balance,
            revision: None,
            not_found_public_keys: Default::default(),
        })
    };
    let timeout = dash_sdk::Error::TimeoutReached(std::time::Duration::from_secs(30), "x".into());
    let check_tx =
        dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(Box::new(cause.clone())));
    let cases = [
        (Err(refused(Some(cause))), "refused"),
        (Err(refused(None)), "unknown"),
        (Err(timeout), "unknown"),
        (Err(check_tx), "unknown"),
        (Ok(identity(Some(7))), "ok 7"),
        (Ok(identity(None)), "unknown"),
    ];
    for (i, (result, want)) in cases.into_iter().enumerate() {
        assert_eq!(kind(settled(result)), want, "case {i}");
    }
}

/// D16: the pages are read oldest first, each after the last one's final
/// document, until a short page; the newest `limit` come back newest first.
#[test]
fn the_pages_keep_the_newest() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let page = WITHDRAWALS_PAGE as usize;
    for n in [0, 1, 20, page - 1, page, page + 1, 2 * page, 250] {
        let all: Vec<_> = (0..n as u32)
            .map(|i| {
                let mut id = [0; 32];
                id[..4].copy_from_slice(&i.to_be_bytes());
                Identifier::from(id)
            })
            .collect();
        let starts = Mutex::new(Vec::new());
        let got = rt
            .block_on(newest(RECENT_WITHDRAWALS, |start: Option<Identifier>| {
                starts.lock().unwrap().push(start);
                let from = start.map_or(0, |s| all.iter().position(|&id| id == s).unwrap() + 1);
                let read: Vec<_> = (all[from..].iter().take(page))
                    .map(|&id| {
                        (
                            id,
                            Some(Document::V0(DocumentV0 {
                                id,
                                ..Default::default()
                            })),
                        )
                    })
                    .collect();
                async move { Ok(read) }
            }))
            .unwrap();
        let ids: Vec<_> = got.iter().map(|d| d.id()).collect();
        let want: Vec<_> = all
            .iter()
            .rev()
            .take(RECENT_WITHDRAWALS as usize)
            .copied()
            .collect();
        assert_eq!(ids, want, "{n} documents");
        let starts = starts.into_inner().unwrap();
        assert_eq!(
            starts.len(),
            n / page + 1,
            "{n} documents: one read per page and a short one"
        );
        for (k, start) in starts.iter().enumerate() {
            assert_eq!(
                *start,
                k.checked_sub(1).map(|k| all[(k + 1) * page - 1]),
                "{n} documents, page {k}"
            );
        }
    }
}

/// A document updated between two page reads comes twice; `withdrawals`
/// lists it once, as its newest.
#[test]
fn a_document_read_twice_is_listed_once() {
    let w = World::new(5 * DASH);
    let out = w
        .withdraw(
            WithdrawAmount::Credits { credits: DASH },
            &w.f.grant(2 * DASH),
        )
        .unwrap();
    {
        let mut documents = w.net.documents.lock().unwrap();
        let mut again = documents[0].clone();
        let Document::V0(d) = &mut again;
        d.properties.insert(
            withdrawal::properties::STATUS.into(),
            Value::U8(DocStatus::COMPLETE as u8),
        );
        d.updated_at = Some(u64::MAX);
        documents.push(again);
    }
    let recent = w.recent().unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(
        (recent[0].id.as_str(), recent[0].status),
        (out.withdrawal.as_str(), WithdrawalStatus::Complete)
    );
}

/// Max takes no more than the quote approved: credits that arrive between
/// the quote and the withdrawal grow Max past the quote's grant, which
/// refuses it unredeemed.
#[test]
fn max_stays_within_the_quotes_grant() {
    let w = World::new(2 * DASH);
    let q = w.quote(WithdrawAmount::All).unwrap();
    let grant = w.f.grant(q.grant.max_credits);
    *w.net.balance.lock().unwrap() = Some(2 * DASH + 1_000);
    let refused = w.withdraw(WithdrawAmount::All, &grant).unwrap_err();
    assert_eq!(refused.code(), "platform.grant_exceeded");
    assert!(w.f.unused(&grant));
    assert!(w.net.sent().is_empty());
    // Within the grant again, it withdraws the quote's credits.
    *w.net.balance.lock().unwrap() = Some(2 * DASH);
    assert_eq!(
        w.withdraw(WithdrawAmount::All, &grant).unwrap().credits,
        q.credits
    );
}
