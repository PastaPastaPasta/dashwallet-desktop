//! Credit withdrawal (DP6-02; design `docs/design/DP6-02-withdraw-credits.md`):
//! an IdentityCreditWithdrawal from one of the wallet's identities to a
//! Core address, its quote, and the identity's recent withdrawals, behind
//! the facade calls of `credits.rs`. Not part of the facade.
//!
//! Every figure is Platform's at the network's protocol version (D2–D5):
//!
//! - **The range.** Drive's basic structure checks (`identity_credit_withdrawal/structure`):
//!   from `min_withdrawal_amount` (plus the asset unlock's Core fee from
//!   structure v2, `min_withdrawal_amount_with_core_fee`) up to
//!   `max_withdrawal_amount`, raised where needed so the payout clears
//!   Core's dust threshold: a payout below it is signed and never mined,
//!   and Platform keeps its credits (`WithdrawalStatus::FAILED`).
//! - **The fee reserve.** Drive refuses a withdrawal whose identity holds
//!   less than the amount plus dpp's `calculate_min_required_fee`, then
//!   meters the fee and refuses it if it exceeds what the amount leaves.
//!   The reserve is the larger of that minimum and a bound on the metered
//!   fee (`credit_costs::withdrawal_metered_bound`), so "Max" is the
//!   balance less the reserve, in whole duffs.
//! - **The payout.** Platform's own conversion of the withdrawal document
//!   Drive would write (`try_into_asset_unlock_base_transaction_info`): the
//!   credits in duffs, less the Core fee where the version carves it out.
//!
//! A withdrawal signs with a CRITICAL ECDSA TRANSFER key
//! (`requests::flow_keys`), never the OWNER key (Platform refuses an OWNER
//! key paying to an address of the user's choice), at Core fee rate 1 and
//! pooling `Never` (the SDK's choices; Drive accepts no other pooling). It
//! runs under the identity's write lock, from before the grant is redeemed,
//! after the money gate (rule 5, DP3-04), in a `Withdraw` lease charged the
//! amount plus the reserve.
//!
//! Once the transition is signed it may have landed whatever the broadcast
//! answered (D21, D24). It counts as executed only on proved evidence: the
//! identity's proved balance down by at least the amount from the one read
//! under the lock, or the withdrawal's document, whose id the transition
//! fixes (Drive's entropy is the nonce and the output script), read back
//! with the amount. A refusal Platform answered with no such document is
//! that refusal; any other answer is `platform.broadcast_unknown` naming
//! the document, and nothing retries it. The nonce is never the SDK's
//! cache's ([`WithdrawalNonces`]): a later withdrawal of the session first
//! reads back the unconfirmed one's documents (one there executed, and the
//! new one is refused as `credits.withdrawal_landed`), then signs at its
//! nonce while Platform would still take it, so at most one of them
//! executes.

use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use dash_sdk::drive::query::{OrderClause, WhereClause, WhereOperator};
use dash_sdk::platform::transition::broadcast::BroadcastStateTransition;
use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dash_sdk::query_types::{IdentityBalance, IdentityNonceFetcher};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::dashcore::{Address, Network, ScriptBuf};
use dpp::data_contract::DataContract;
use dpp::document::{Document, DocumentV0, DocumentV0Getters};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_nonce::{
    IDENTITY_NONCE_VALUE_FILTER, validate_identity_nonce_update, validate_new_identity_nonce,
};
use dpp::identity::signer::Signer;
use dpp::identity::{Identity, IdentityPublicKey};
use dpp::platform_value::btreemap_extensions::BTreeValueMapHelper;
use dpp::platform_value::string_encoding::Encoding;
use dpp::platform_value::{Value, platform_value};
use dpp::prelude::Identifier;
use dpp::state_transition::StateTransition;
use dpp::state_transition::StateTransitionEstimatedFeeValidation;
use dpp::state_transition::identity_credit_withdrawal_transition::IdentityCreditWithdrawalTransition;
use dpp::state_transition::identity_credit_withdrawal_transition::MIN_CORE_FEE_PER_BYTE;
use dpp::state_transition::identity_credit_withdrawal_transition::methods::{
    IdentityCreditWithdrawalTransitionMethodsV0, PreferredKeyPurposeForSigningWithdrawal,
};
use dpp::state_transition::identity_credit_withdrawal_transition::v1::IdentityCreditWithdrawalTransitionV1;
use dpp::state_transition::proof_result::StateTransitionProofResult;
use dpp::system_data_contracts::withdrawals_contract::v1::document_types::withdrawal;
use dpp::system_data_contracts::withdrawals_contract::{self, WithdrawalStatus as DocStatus};
use dpp::system_data_contracts::{SystemDataContract, load_system_data_contract};
use dpp::version::PlatformVersion;
use dpp::withdrawal::{Pooling, min_withdrawal_amount_with_core_fee};
use platform_wallet::PlatformWallet;

use super::credit_costs::withdrawal_metered_bound;
use super::credits::{
    WithdrawAmount, WithdrawOutcome, WithdrawQuote, Withdrawal, WithdrawalStatus,
};
use super::errors::{ContactError, CreditsError, IdentityError, PlatformError};
use super::flows::{FlowKind, GrantRequest};
use super::identity::KeyPurpose;
use super::identity_writes::identity_locks;
use super::money_gate::Subject;
use super::registration_flow::parse_identity;
use super::requests::{flow_signer, leased, write_lease, write_permit};
use crate::{NetworkSession, WalletId};

/// The Core fee rate the withdrawal asks for, in duffs per byte: Platform's
/// minimum, as the SDK's default (D5).
const CORE_FEE_PER_BYTE: u32 = MIN_CORE_FEE_PER_BYTE;
/// The withdrawals a `withdrawals` read returns at most, newest first.
const RECENT_WITHDRAWALS: u32 = 20;

/// Documents per page of [`SdkWithdrawals`]'s read.
const WITHDRAWALS_PAGE: u32 = 100;

/// Why a signed withdrawal's broadcast failed.
#[derive(Debug)]
pub(crate) enum Unsent {
    /// Platform answered with a refusal (its check or its execution): the
    /// withdrawal did not happen.
    Refused(PlatformError),
    /// No answer that says: it may have landed.
    Unknown(PlatformError),
}

/// What a withdrawal asks of Platform, behind a seam (as `RequestNet`):
/// [`SdkWithdrawals`] asks the SDK; `withdrawal_tests.rs` scripts it.
pub(crate) trait WithdrawNet: Send + Sync {
    /// The network's protocol version, as the SDK last proved it: the
    /// SDK's seed until a proved read ratchets it, so read it after one.
    fn version(&self) -> &'static PlatformVersion;
    /// The identity's credits, proved; `None` when Platform does not know it.
    fn balance(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
    /// The identity's nonce as Drive stores it (the newest nonce and the
    /// missing-revision bits above it), proved, and the block height of
    /// the proof; 0 when Platform has none. Never the SDK's cache (D24).
    fn nonce_state(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(u64, u64), PlatformError>> + Send;
    /// Broadcasts `transition` and waits for its result: the identity's new
    /// balance, proved.
    fn broadcast(
        &self,
        transition: StateTransition,
    ) -> impl Future<Output = Result<u64, Unsent>> + Send;
    /// The withdrawal document `id`, proved, `None` when it is not there,
    /// and the block height of the proof.
    fn withdrawal(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<(Option<Document>, u64), PlatformError>> + Send;
    /// `owner`'s withdrawal documents, the most recently updated first, at
    /// most `limit`.
    fn withdrawals(
        &self,
        owner: Identifier,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Document>, PlatformError>> + Send;
}

pub(crate) struct SdkWithdrawals(pub(crate) Arc<dash_sdk::Sdk>);

impl WithdrawNet for SdkWithdrawals {
    fn version(&self) -> &'static PlatformVersion {
        self.0.version()
    }

    async fn balance(&self, id: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(IdentityBalance::fetch(&self.0, id).await?)
    }

    async fn nonce_state(&self, id: Identifier) -> Result<(u64, u64), PlatformError> {
        let (state, at) = IdentityNonceFetcher::fetch_with_metadata(&self.0, id, None).await?;
        Ok((state.map_or(0, |IdentityNonceFetcher(n)| n), at.height))
    }

    async fn broadcast(&self, transition: StateTransition) -> Result<u64, Unsent> {
        let owner = transition.owner_id();
        let result = transition
            .broadcast_and_wait_for_affected_state::<StateTransitionProofResult>(&self.0, None)
            .await;
        // Every other identity-nonce write takes the SDK's cache: re-read
        // it from Platform and step it, so they take a later nonce (D24).
        if let Some(owner) = owner {
            self.0.refresh_identity_nonce(&owner).await;
            if let Err(e) = self.0.get_identity_nonce(owner, true, None).await {
                tracing::info!("advancing the SDK's nonce cache: {e}");
            }
        }
        settled(result)
    }

    async fn withdrawal(&self, id: Identifier) -> Result<(Option<Document>, u64), PlatformError> {
        let query = DocumentQuery::new(withdrawals_contract_v()?, withdrawal::NAME)
            .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
            .with_document_id(&id);
        let (document, at) = Document::fetch_with_metadata(&self.0, query, None).await?;
        Ok((document, at.height))
    }

    /// Reads every page oldest first and keeps the newest `limit`: at the
    /// pin a descending query's proof does not verify ("inverted op in a
    /// left-to-right walk", T2 D16).
    async fn withdrawals(
        &self,
        owner: Identifier,
        limit: u32,
    ) -> Result<Vec<Document>, PlatformError> {
        use dash_sdk::dapi_grpc::platform::v0::get_documents_request::get_documents_request_v0::Start;
        let contract = withdrawals_contract_v()?;
        let sdk = &self.0;
        newest(limit, |start: Option<Identifier>| {
            let contract = Arc::clone(&contract);
            async move {
                let mut query = DocumentQuery::new(contract, withdrawal::NAME)
                    .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
                    .with_where(WhereClause {
                        field: withdrawal::properties::OWNER_ID.into(),
                        operator: WhereOperator::Equal,
                        value: Value::Identifier(owner.to_buffer()),
                    })
                    .with_order_by(OrderClause {
                        field: withdrawal::properties::UPDATED_AT.into(),
                        ascending: true,
                    });
                query.limit = WITHDRAWALS_PAGE;
                query.start = start.map(|id| Start::StartAfter(id.to_buffer().to_vec()));
                Ok(Document::fetch_many(sdk, query)
                    .await?
                    .into_iter()
                    .collect())
            }
        })
        .await
    }
}

/// What a broadcast's result says (D21): only a refusal that carries
/// Platform's consensus error says the withdrawal did not happen. DAPI
/// answers a failed wait (its own timeout, a dropped Tenderdash
/// subscription) with a broadcast error too, without a cause, while the
/// transition may sit in the mempool and land: that is `Unknown`. So is a
/// consensus error from the broadcast step: the SDK resends to other nodes,
/// and a later send's refusal (the nonce used, the credits gone) can follow
/// a first send Platform took.
fn settled(result: Result<StateTransitionProofResult, dash_sdk::Error>) -> Result<u64, Unsent> {
    match result {
        Ok(StateTransitionProofResult::VerifiedPartialIdentity(identity)) => identity
            .balance
            .ok_or_else(|| Unsent::Unknown(internal("the proved identity has no balance"))),
        Ok(_) => Err(Unsent::Unknown(internal("the proof is not of an identity"))),
        Err(
            e @ dash_sdk::Error::StateTransitionBroadcastError(
                dash_sdk::error::StateTransitionBroadcastError { cause: Some(_), .. },
            ),
        ) => Err(Unsent::Refused(e.into())),
        Err(e) => Err(Unsent::Unknown(e.into())),
    }
}

/// What the session knows of its identities' nonces past Platform's proved
/// state (D24), one entry per identity it withdrew from. Not kept across
/// a restart.
#[derive(Default)]
pub(crate) struct WithdrawalNonces(Mutex<HashMap<Identifier, Nonces>>);

#[derive(Debug, Default, Clone)]
struct Nonces {
    /// The nonce of the last withdrawals signed and not seen executed, and
    /// each one signed at it.
    unconfirmed: Option<(u64, Vec<Signed>)>,
    /// Past every withdrawal seen executed: a node behind it never hands
    /// back a nonce this session saw used.
    floor: u64,
}

/// A withdrawal signed: its document and amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Signed {
    document: Identifier,
    credits: u64,
}

impl WithdrawalNonces {
    /// The nonce `id`'s next withdrawal signs at, given its proved nonce
    /// `state`: the unconfirmed one's while Drive's rule would still take
    /// it (unused, within 24 of the newest, and past the floor), so at most
    /// one of them executes; else one past the newest, Platform's or the
    /// session's. A `state` too far behind the unconfirmed nonce to take it
    /// decides nothing (`platform.unavailable`): it is from before that
    /// withdrawal was signed.
    fn next(&self, id: Identifier, state: u64) -> Result<u64, PlatformError> {
        let n = self.get(id);
        let tip = state & IDENTITY_NONCE_VALUE_FILTER;
        if let Some((k, _)) = n.unconfirmed
            && k >= n.floor
        {
            if takes(state, k, id) {
                return Ok(k);
            }
            if k > tip {
                return Err(PlatformError::Unavailable);
            }
        }
        Ok((tip + 1).max(n.floor))
    }

    /// The withdrawals signed at `id`'s unconfirmed nonce, if it has one.
    fn unconfirmed(&self, id: Identifier) -> Option<(u64, Vec<Signed>)> {
        self.get(id).unconfirmed
    }

    /// A withdrawal at `nonce` is signed: it may execute from now on.
    fn signed(&self, id: Identifier, nonce: u64, signed: Signed) {
        self.update(id, |n| match &mut n.unconfirmed {
            Some((k, all)) if *k == nonce => all.push(signed),
            unconfirmed => *unconfirmed = Some((nonce, vec![signed])),
        });
    }

    /// A withdrawal at `nonce` executed.
    fn executed(&self, id: Identifier, nonce: u64) {
        self.update(id, |n| {
            n.floor = n.floor.max(nonce + 1);
            if n.unconfirmed.as_ref().is_some_and(|(k, _)| *k == nonce) {
                n.unconfirmed = None;
            }
        });
    }

    fn get(&self, id: Identifier) -> Nonces {
        let map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(&id).cloned().unwrap_or_default()
    }

    fn update(&self, id: Identifier, change: impl FnOnce(&mut Nonces)) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        change(map.entry(id).or_default());
    }
}

/// Whether Drive takes nonce `k` from an identity whose stored nonce is
/// `state` (0: none stored), by its own rule.
fn takes(state: u64, k: u64, id: Identifier) -> bool {
    if state == 0 {
        validate_new_identity_nonce(k, id).is_valid()
    } else {
        validate_identity_nonce_update(state, k, id).is_valid()
    }
}

/// Reads of a withdrawal's document before one recent enough decides: the
/// SDK asks another node each time, and one a block behind is common.
const READ_BACK_TRIES: usize = 3;

/// Which of `signed` executed, by its document read back with its amount;
/// `None` when none did. Each read is at least as recent as `height`, so a
/// node behind cannot hide one; when [`READ_BACK_TRIES`] reads all come
/// from further back, nothing is decided (`platform.unavailable`).
async fn landed(
    net: &impl WithdrawNet,
    signed: &[Signed],
    height: u64,
) -> Result<Option<Signed>, PlatformError> {
    for s in signed {
        let mut tries = 0;
        let document = loop {
            let (document, at) = net.withdrawal(s.document).await?;
            if at >= height {
                break document;
            }
            tries += 1;
            if tries == READ_BACK_TRIES {
                return Err(PlatformError::Unavailable);
            }
        };
        if document.is_some_and(|d| amount_of(&d) == Some(s.credits)) {
            return Ok(Some(*s));
        }
    }
    Ok(None)
}

/// `credits.withdrawal_landed` naming `s`'s document.
fn landed_error(s: Signed) -> CreditsError {
    CreditsError::WithdrawalLanded {
        artifact: s.document.to_string(Encoding::Base58),
    }
}

/// The nonce a withdrawal of `id` signs at (D24), read before its grant is
/// redeemed. An unconfirmed withdrawal's documents are read back first, at
/// least as recent as the nonce: one there executed, so this withdrawal is
/// `credits.withdrawal_landed` naming it, and nothing is signed.
async fn nonce_for(
    net: &impl WithdrawNet,
    nonces: &WithdrawalNonces,
    id: Identifier,
) -> Result<u64, CreditsError> {
    let (state, height) = net.nonce_state(id).await?;
    if let Some((k, signed)) = nonces.unconfirmed(id)
        && let Some(s) = landed(net, &signed, height).await?
    {
        nonces.executed(id, k);
        return Err(landed_error(s));
    }
    Ok(nonces.next(id, state)?)
}

/// The newest `limit` documents of pages read oldest first: `page(start)`
/// reads up to [`WITHDRAWALS_PAGE`] documents after `start`, and a shorter
/// page is the last. Only the owner creates its withdrawals, so the pages
/// end.
async fn newest<F, Fut>(limit: u32, mut page: F) -> Result<Vec<Document>, PlatformError>
where
    F: FnMut(Option<Identifier>) -> Fut,
    Fut: Future<Output = Result<Vec<(Identifier, Option<Document>)>, PlatformError>>,
{
    let mut newest = std::collections::VecDeque::new();
    let mut start = None;
    loop {
        let read = page(start.take()).await?;
        let (len, last) = (read.len(), read.last().map(|(id, _)| *id));
        for document in read.into_iter().filter_map(|(_, d)| d) {
            if newest.len() == limit as usize {
                newest.pop_front();
            }
            newest.push_back(document);
        }
        match last {
            Some(id) if len >= WITHDRAWALS_PAGE as usize => start = Some(id),
            _ => return Ok(newest.into_iter().rev().collect()),
        }
    }
}

/// The withdrawals contract, as dpp holds it (a system contract: the
/// network's is this one).
fn withdrawals_contract_v() -> Result<Arc<DataContract>, PlatformError> {
    static CONTRACT: std::sync::OnceLock<Arc<DataContract>> = std::sync::OnceLock::new();
    if let Some(contract) = CONTRACT.get() {
        return Ok(Arc::clone(contract));
    }
    let contract =
        load_system_data_contract(SystemDataContract::Withdrawals, PlatformVersion::latest())
            .map_err(|e| internal(format!("the withdrawals contract: {e}")))?;
    Ok(Arc::clone(CONTRACT.get_or_init(|| Arc::new(contract))))
}

/// Platform's withdrawal rules at one protocol version (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rules {
    /// The least a withdrawal takes, in credits.
    pub(crate) min: u64,
    /// The most a withdrawal takes, in credits.
    pub(crate) max: u64,
    /// The fee reserve, in credits.
    pub(crate) fee: u64,
}

impl Rules {
    /// The rules at `version`, computed once per version. A version whose
    /// Drive does not take withdrawals is `invalid_argument`.
    pub(crate) fn at(version: &PlatformVersion) -> Result<Self, PlatformError> {
        static RULES: LazyLock<Mutex<HashMap<u32, Rules>>> = LazyLock::new(Mutex::default);
        let mut rules = RULES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = rules.get(&version.protocol_version) {
            return Ok(*r);
        }
        let r = Self::compute(version)?;
        rules.insert(version.protocol_version, r);
        Ok(r)
    }

    fn compute(version: &PlatformVersion) -> Result<Self, PlatformError> {
        let limits = &version.system_limits;
        let structure = (version
            .drive_abci
            .validation_and_processing
            .state_transitions)
            .identity_credit_withdrawal_state_transition
            .basic_structure;
        let platform_min = match structure {
            Some(1) => limits.min_withdrawal_amount,
            Some(2) => min_withdrawal_amount_with_core_fee(CORE_FEE_PER_BYTE, version),
            _ => {
                return Err(PlatformError::InvalidArgument {
                    detail: format!(
                        "no withdrawal rules known for protocol version {}",
                        version.protocol_version
                    ),
                });
            }
        };
        let min_fee =
            IdentityCreditWithdrawalTransition::V1(IdentityCreditWithdrawalTransitionV1::default())
                .calculate_min_required_fee(version)
                .map_err(internal)?;
        Ok(Self {
            min: platform_min.max(dust_floor(version)?),
            max: limits.max_withdrawal_amount,
            fee: min_fee.max(withdrawal_metered_bound(version)?),
        })
    }

    /// The credits `amount` takes from an identity holding `balance`:
    /// - "Max": the balance less the reserve, in whole duffs (the rest
    ///   stays), at most [`Self::max`]; one below [`Self::min`] is
    ///   `platform.insufficient_credits` for the least withdrawal;
    /// - an amount: outside the range `credits.withdrawal_out_of_range`,
    ///   above the balance less the reserve `platform.insufficient_credits`.
    pub(crate) fn credits(
        &self,
        balance: u64,
        amount: WithdrawAmount,
    ) -> Result<u64, CreditsError> {
        let insufficient = |credits: u64| PlatformError::InsufficientCredits {
            needed: credits.saturating_add(self.fee),
            available: balance,
        };
        match amount {
            WithdrawAmount::All => {
                let free = balance.saturating_sub(self.fee);
                let credits = (free - free % CREDITS_PER_DUFF).min(self.max);
                if credits < self.min {
                    return Err(insufficient(self.min).into());
                }
                Ok(credits)
            }
            WithdrawAmount::Credits { credits } => {
                if credits < self.min || credits > self.max {
                    return Err(CreditsError::WithdrawalOutOfRange {
                        min: self.min,
                        max: self.max,
                    });
                }
                if balance < credits.saturating_add(self.fee) {
                    return Err(insufficient(credits).into());
                }
                Ok(credits)
            }
        }
    }
}

/// The document Drive writes for a withdrawal of `credits` to `script`
/// (`IdentityCreditWithdrawalTransitionAction`), stamped with the contract
/// version where Drive's `transform_into_action` v1 stamps it.
fn drive_document(
    credits: u64,
    script: &CoreScript,
    version: &PlatformVersion,
) -> Result<Document, PlatformError> {
    let transform = (version
        .drive_abci
        .validation_and_processing
        .state_transitions)
        .identity_credit_withdrawal_state_transition
        .transform_into_action;
    let properties = platform_value!({
        withdrawal::properties::AMOUNT: credits,
        withdrawal::properties::CORE_FEE_PER_BYTE: CORE_FEE_PER_BYTE,
        withdrawal::properties::POOLING: Pooling::Never,
        withdrawal::properties::OUTPUT_SCRIPT: script.as_bytes(),
        withdrawal::properties::STATUS: DocStatus::QUEUED,
    });
    Ok(Document::V0(DocumentV0 {
        contract_version: (transform >= 1)
            .then_some(u32::from(version.system_data_contracts.withdrawals)),
        properties: properties.into_btree_string_map().map_err(internal)?,
        revision: Some(1),
        ..DocumentV0::default()
    }))
}

/// What reaches `script` on Core for a withdrawal document: Platform's own
/// conversion into its asset unlock (module doc).
fn payout(document: &Document, version: &PlatformVersion) -> Result<u64, PlatformError> {
    let info =
        (document.try_into_asset_unlock_base_transaction_info(0, version)).map_err(internal)?;
    (info.output.first())
        .map(|o| o.value)
        .ok_or_else(|| internal("the asset unlock pays nothing"))
}

/// What a withdrawal of `credits` pays out on Core, in duffs. The same for
/// either address kind: only the dust threshold differs.
pub(crate) fn expected_duffs(
    credits: u64,
    version: &PlatformVersion,
) -> Result<u64, PlatformError> {
    payout(&drive_document(credits, &p2pkh_probe(), version)?, version)
}

/// A P2PKH script, the address kind with the higher dust threshold.
fn p2pkh_probe() -> CoreScript {
    CoreScript::new_p2pkh([0; 20])
}

/// The least withdrawal whose payout to a P2PKH address is not dust.
fn dust_floor(version: &PlatformVersion) -> Result<u64, PlatformError> {
    let dust = p2pkh_probe().dust_value().to_sat();
    // The Core fee a payout carries is the same at every amount.
    let probe = version.system_limits.max_withdrawal_amount;
    let carved = (probe / CREDITS_PER_DUFF).saturating_sub(expected_duffs(probe, version)?);
    Ok((dust + carved) * CREDITS_PER_DUFF)
}

/// The output script of `to`, a P2PKH or P2SH address of `network`;
/// anything else is `invalid_argument`.
pub(crate) fn destination(network: Network, to: &str) -> Result<CoreScript, PlatformError> {
    let invalid = |detail: &str| PlatformError::InvalidArgument {
        detail: detail.into(),
    };
    let address = Address::from_str(to.trim()).map_err(|_| invalid("not a Dash address"))?;
    let address = (address.require_network(network))
        .map_err(|_| invalid("the address is of another network"))?;
    let script = address.script_pubkey();
    if !script.is_p2pkh() && !script.is_p2sh() {
        return Err(invalid("only P2PKH and P2SH addresses take withdrawals"));
    }
    Ok(CoreScript::new(script))
}

/// The id of the withdrawal document Drive writes for `owner`'s withdrawal
/// at `nonce` to `script` (its entropy: the nonce, then the script).
pub(crate) fn document_id(owner: &Identifier, nonce: u64, script: &CoreScript) -> Identifier {
    let mut entropy = nonce.to_be_bytes().to_vec();
    entropy.extend_from_slice(script.as_bytes());
    Document::generate_document_id_v0(&withdrawals_contract::ID, owner, withdrawal::NAME, &entropy)
}

/// The signed withdrawal: dpp's own construction (the SDK's
/// `withdraw_with_metadata`) with `key`, the one key `signer` signs with.
pub(crate) async fn transition(
    identity: &Identity,
    (script, credits, nonce): (CoreScript, u64, u64),
    key: &IdentityPublicKey,
    signer: impl Signer<IdentityPublicKey>,
    version: &PlatformVersion,
) -> Result<StateTransition, PlatformError> {
    IdentityCreditWithdrawalTransition::try_from_identity(
        identity,
        Some(script),
        credits,
        Pooling::Never,
        CORE_FEE_PER_BYTE,
        0,
        signer,
        Some(key),
        PreferredKeyPurposeForSigningWithdrawal::TransferPreferred,
        nonce,
        version,
        None,
    )
    .await
    .map_err(internal)
}

/// The wallet's identity `id` (its index and keys); one it only watches is
/// `invalid_argument`.
async fn own(wallet: &PlatformWallet, id: Identifier) -> Result<(u32, Identity), PlatformError> {
    let state = wallet.state().await;
    let m = (state.identity_manager.managed_identity(&id))
        .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let index = m
        .identity_index
        .ok_or_else(|| PlatformError::InvalidArgument {
            detail: "the wallet only watches this identity; it cannot withdraw".into(),
        })?;
    Ok((index, m.identity.clone()))
}

/// The plan both the quote and the withdrawal check: the credits taken,
/// the rules they were taken by, at the network's version, and the proved
/// balance they were taken from.
async fn plan(
    net: &impl WithdrawNet,
    id: Identifier,
    amount: WithdrawAmount,
) -> Result<(u64, Rules, u64), CreditsError> {
    let balance =
        (net.balance(id).await?).ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    // After the proved read, which ratchets the SDK from its seed to the
    // network's protocol version (T2 D17).
    let rules = Rules::at(net.version())?;
    Ok((rules.credits(balance, amount)?, rules, balance))
}

pub(crate) async fn quote(
    net: &impl WithdrawNet,
    wallet: &PlatformWallet,
    identity: &str,
    amount: WithdrawAmount,
) -> Result<WithdrawQuote, CreditsError> {
    let id = parse_identity(identity)?;
    own(wallet, id).await?;
    let (credits, rules, _) = plan(net, id, amount).await?;
    Ok(WithdrawQuote {
        credits,
        fee_credits: rules.fee,
        expected_duffs: expected_duffs(credits, net.version())?,
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: credits + rules.fee,
        },
    })
}

/// A write's refusal from the shared lease helpers as the `PlatformError`
/// it wraps.
fn platform(e: ContactError) -> PlatformError {
    e.platform().cloned().unwrap_or_else(|| internal(e))
}

/// `DashPay::withdraw` (module doc).
pub(crate) async fn withdraw(
    net: &impl WithdrawNet,
    session: &Arc<NetworkSession>,
    wallet: &PlatformWallet,
    (wallet_id, identity): (WalletId, &str),
    (to, amount): (&str, WithdrawAmount),
    grant: &str,
) -> Result<WithdrawOutcome, CreditsError> {
    let id = parse_identity(identity)?;
    let script = destination(session.vault.network(), to)?;
    own(wallet, id).await?;
    // Rule 5 (DP3-04): the identity verified, before its write lock (not
    // held across the wait) and before the grant.
    (session.money_gate(Subject::Identity(wallet_id, id)).await)
        .map_err(|refusal| CreditsError::from(refusal.platform()))?;
    let _refresh = session.after_spend(wallet_id, id);
    let locks = identity_locks(wallet_id, id);
    let _write = locks.write.lock().await;
    // Under the write lock, so the balance is the one this withdrawal
    // takes from, and before the grant, so a refusal keeps it.
    let (index, me) = own(wallet, id).await?;
    let (credits, rules, before) = plan(net, id, amount).await?;
    let version = net.version();
    let expected = expected_duffs(credits, version)?;
    let quote = credits + rules.fee;
    let nonces = &session.withdrawal_nonces;
    let nonce = nonce_for(net, nonces, id).await?;
    let signs = (index, &me);
    let lease = write_lease(
        session,
        wallet_id,
        grant,
        (&[FlowKind::Withdraw], quote),
        signs,
        || Ok(()),
    )
    .await
    .map_err(platform)?;
    let permit = write_permit(&lease, "withdraw", quote)?;
    let signer = flow_signer(&permit, index, &me, &[]).map_err(platform)?;
    let key = (me.public_keys().values())
        .find(|k| signer.can_sign_with(k))
        .cloned()
        .ok_or(PlatformError::Identity(IdentityError::KeysMissing {
            purpose: KeyPurpose::Transfer,
        }))?;
    let doc = document_id(&id, nonce, &script);
    let this = Signed {
        document: doc,
        credits,
    };
    let sent = leased(&lease, async {
        let signed = transition(
            &me,
            (script.clone(), credits, nonce),
            &key,
            signer.clone(),
            version,
        )
        .await?;
        nonces.signed(id, nonce, this);
        Ok::<_, PlatformError>(net.broadcast(signed).await)
    })
    .await;
    drop(signer);
    permit.finish(matches!(sent, Ok(Ok(_))));
    // Nothing signed, so nothing went out.
    let broadcast = sent.map_err(CreditsError::from)?;
    let withdrawal = doc.to_string(Encoding::Base58);
    let done = |remaining| WithdrawOutcome {
        credits,
        expected_duffs: Some(expected),
        remaining_credits: remaining,
        withdrawal: withdrawal.clone(),
    };
    // Every withdrawal signed at this nonce: at most one executes.
    let rivals = (nonces.unconfirmed(id))
        .filter(|(k, _)| *k == nonce)
        .map_or_else(|| vec![this], |(_, all)| all);
    // A proved balance is the identity's state after the wait, not proof
    // that this transition executed (rs-sdk's `wait_for_affected_state`):
    // only a drop by the amount is, and at a nonce signed more than once,
    // where an earlier one's execution drops it too, this document (D24).
    let dropped =
        (broadcast.as_ref().ok().copied()).filter(|b| b.saturating_add(credits) <= before);
    let failure = match broadcast {
        Ok(balance) if dropped.is_some() && rivals.len() == 1 => {
            nonces.executed(id, nonce);
            return Ok(done(Some(balance)));
        }
        Ok(_) if dropped.is_some() => {
            Unsent::Unknown(internal("the nonce was signed more than once"))
        }
        Ok(balance) => Unsent::Unknown(internal(format!(
            "the proved balance {balance} is not {credits} below {before}"
        ))),
        Err(failure) => failure,
    };
    // Signed, so it may have landed: the documents signed at this nonce
    // with their amounts decide, and a failed read decides nothing. A
    // refusal is one node's unproved answer (D21), so it is read back too.
    let (Unsent::Refused(e) | Unsent::Unknown(e)) = &failure;
    tracing::info!(identity = %identity, "a withdrawal's broadcast failed: {e}");
    match landed(net, &rivals, 0).await {
        Ok(Some(s)) => {
            nonces.executed(id, nonce);
            if s == this {
                Ok(done(dropped))
            } else {
                Err(landed_error(s))
            }
        }
        read => {
            if let Err(again) = read {
                tracing::info!("reading the withdrawal back: {again}");
            }
            Err(match failure {
                Unsent::Refused(e) => e,
                Unsent::Unknown(_) => PlatformError::BroadcastUnknown {
                    artifact: withdrawal,
                },
            }
            .into())
        }
    }
}

/// A withdrawal document's amount, in credits.
fn amount_of(d: &Document) -> Option<u64> {
    d.properties()
        .get_integer(withdrawal::properties::AMOUNT)
        .ok()
}

/// `owner`'s recent withdrawals (`identityRecent`), newest first.
pub(crate) async fn recent(
    net: &impl WithdrawNet,
    wallet: &PlatformWallet,
    network: Network,
    identity: &str,
) -> Result<Vec<Withdrawal>, CreditsError> {
    let id = parse_identity(identity)?;
    (wallet.state().await.identity_manager.managed_identity(&id))
        .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let mut documents = net.withdrawals(id, RECENT_WITHDRAWALS).await?;
    let version = net.version();
    documents.retain(|d| d.owner_id() == id);
    documents.sort_by_key(|d| std::cmp::Reverse((d.updated_at(), d.created_at())));
    // A document updated between two page reads comes twice: keep its newest.
    let mut seen = std::collections::HashSet::new();
    documents.retain(|d| seen.insert(d.id()));
    Ok((documents.iter())
        .map(|d| record(d, network, version))
        .collect())
}

/// A withdrawal document as the host sees it; a property it cannot read is
/// absent (or `Unknown` for the status), never an error.
fn record(d: &Document, network: Network, version: &PlatformVersion) -> Withdrawal {
    let p = d.properties();
    let script = p.get_bytes(withdrawal::properties::OUTPUT_SCRIPT).ok();
    let to = script.and_then(|s| {
        let address = Address::from_script(&ScriptBuf::from_bytes(s), network).ok()?;
        Some(address.to_string())
    });
    let status = (p.get_integer::<u8>(withdrawal::properties::STATUS).ok())
        .and_then(|s| DocStatus::try_from(s).ok())
        .map_or(WithdrawalStatus::Unknown, |s| match s {
            DocStatus::QUEUED => WithdrawalStatus::Queued,
            DocStatus::POOLED => WithdrawalStatus::Pooled,
            DocStatus::BROADCASTED => WithdrawalStatus::Broadcasted,
            DocStatus::COMPLETE => WithdrawalStatus::Complete,
            DocStatus::EXPIRED => WithdrawalStatus::Expired,
            DocStatus::FAILED => WithdrawalStatus::Failed,
        });
    Withdrawal {
        id: d.id().to_string(Encoding::Base58),
        credits: amount_of(d),
        expected_duffs: payout(d, version).ok(),
        to,
        status,
        created_at: d.created_at(),
        updated_at: d.updated_at(),
    }
}

fn internal(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Internal {
        detail: format!("the withdrawal: {e}"),
    }
}

#[cfg(test)]
#[path = "withdrawal_tests.rs"]
mod tests;
