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
//! answered: the withdrawal's document, whose id the transition fixes
//! (Drive's entropy is the nonce and the output script), decides. A
//! refusal Platform answered is that refusal; any other failure with no
//! document read back is `platform.broadcast_unknown` naming the document,
//! and nothing retries it (a retry signs a new nonce: a second
//! withdrawal).

use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use dash_sdk::drive::query::{OrderClause, WhereClause, WhereOperator};
use dash_sdk::platform::transition::broadcast::BroadcastStateTransition;
use dash_sdk::platform::{DocumentQuery, Fetch, FetchMany};
use dash_sdk::query_types::IdentityBalance;
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::dashcore::{Address, Network, ScriptBuf};
use dpp::data_contract::DataContract;
use dpp::document::{Document, DocumentV0, DocumentV0Getters};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::core_script::CoreScript;
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
pub(super) const CORE_FEE_PER_BYTE: u32 = MIN_CORE_FEE_PER_BYTE;
/// The withdrawals a `withdrawals` read returns at most, newest first.
pub(super) const RECENT_WITHDRAWALS: u32 = 20;

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
    /// The network's protocol version, as the SDK last proved it.
    fn version(&self) -> &'static PlatformVersion;
    /// The identity's credits, proved; `None` when Platform does not know it.
    fn balance(&self, id: Identifier)
    -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
    /// The identity's next nonce (the SDK's cached one, bumped).
    fn nonce(&self, id: Identifier) -> impl Future<Output = Result<u64, PlatformError>> + Send;
    /// Broadcasts `transition` and waits for its result: the identity's new
    /// balance, proved.
    fn broadcast(
        &self,
        transition: StateTransition,
    ) -> impl Future<Output = Result<u64, Unsent>> + Send;
    /// The withdrawal document `id`, proved; `None` when it is not there.
    fn withdrawal(
        &self,
        id: Identifier,
    ) -> impl Future<Output = Result<Option<Document>, PlatformError>> + Send;
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

    async fn nonce(&self, id: Identifier) -> Result<u64, PlatformError> {
        Ok(self.0.get_identity_nonce(id, true, None).await?)
    }

    async fn broadcast(&self, transition: StateTransition) -> Result<u64, Unsent> {
        let result = transition
            .broadcast_and_wait_for_affected_state::<StateTransitionProofResult>(&self.0, None)
            .await;
        match result {
            Ok(StateTransitionProofResult::VerifiedPartialIdentity(identity)) => {
                identity.balance.ok_or_else(|| {
                    Unsent::Unknown(internal("the proved identity has no balance"))
                })
            }
            Ok(_) => Err(Unsent::Unknown(internal("the proof is not of an identity"))),
            Err(e @ dash_sdk::Error::StateTransitionBroadcastError(_)) => {
                Err(Unsent::Refused(e.into()))
            }
            Err(e) => Err(Unsent::Unknown(e.into())),
        }
    }

    async fn withdrawal(&self, id: Identifier) -> Result<Option<Document>, PlatformError> {
        let query = DocumentQuery::new(withdrawals_contract_v()?, withdrawal::NAME)
            .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
            .with_document_id(&id);
        Ok(Document::fetch(&self.0, query).await?)
    }

    async fn withdrawals(
        &self,
        owner: Identifier,
        limit: u32,
    ) -> Result<Vec<Document>, PlatformError> {
        let mut query = DocumentQuery::new(withdrawals_contract_v()?, withdrawal::NAME)
            .map_err(|e| PlatformError::from(dash_sdk::Error::from(e)))?
            .with_where(WhereClause {
                field: withdrawal::properties::OWNER_ID.into(),
                operator: WhereOperator::Equal,
                value: Value::Identifier(owner.to_buffer()),
            })
            .with_order_by(OrderClause {
                field: withdrawal::properties::UPDATED_AT.into(),
                ascending: false,
            });
        query.limit = limit;
        let documents = Document::fetch_many(&self.0, query).await?;
        Ok(documents.into_values().flatten().collect())
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
        let structure = (version.drive_abci.validation_and_processing.state_transitions)
            .identity_credit_withdrawal_state_transition
            .basic_structure;
        let platform_min = match structure {
            Some(1) => limits.min_withdrawal_amount,
            Some(2) => min_withdrawal_amount_with_core_fee(CORE_FEE_PER_BYTE, version),
            _ => {
                return Err(PlatformError::InvalidArgument {
                    detail: format!(
                        "protocol version {} takes no withdrawals",
                        version.protocol_version
                    ),
                });
            }
        };
        let min_fee = IdentityCreditWithdrawalTransition::V1(
            IdentityCreditWithdrawalTransitionV1::default(),
        )
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
    pub(crate) fn credits(&self, balance: u64, amount: WithdrawAmount) -> Result<u64, CreditsError> {
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
    let transform = (version.drive_abci.validation_and_processing.state_transitions)
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
        contract_version: (transform >= 1).then_some(u32::from(version.system_data_contracts.withdrawals)),
        properties: properties.into_btree_string_map().map_err(internal)?,
        revision: Some(1),
        ..DocumentV0::default()
    }))
}

/// What reaches `script` on Core for a withdrawal document: Platform's own
/// conversion into its asset unlock (module doc).
fn payout(document: &Document, version: &PlatformVersion) -> Result<u64, PlatformError> {
    let info = (document.try_into_asset_unlock_base_transaction_info(0, version))
        .map_err(internal)?;
    (info.output.first())
        .map(|o| o.value)
        .ok_or_else(|| internal("the asset unlock pays nothing"))
}

/// What a withdrawal of `credits` pays out on Core, in duffs. The same for
/// either address kind: only the dust threshold differs.
pub(crate) fn expected_duffs(credits: u64, version: &PlatformVersion) -> Result<u64, PlatformError> {
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
    let index = m.identity_index.ok_or_else(|| PlatformError::InvalidArgument {
        detail: "the wallet only watches this identity; it cannot withdraw".into(),
    })?;
    Ok((index, m.identity.clone()))
}

/// The plan both the quote and the withdrawal check: the credits taken and
/// the rules they were taken by, at the network's version.
async fn plan(
    net: &impl WithdrawNet,
    id: Identifier,
    amount: WithdrawAmount,
) -> Result<(u64, Rules), CreditsError> {
    let rules = Rules::at(net.version())?;
    let balance = (net.balance(id).await?).ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    Ok((rules.credits(balance, amount)?, rules))
}

pub(crate) async fn quote(
    net: &impl WithdrawNet,
    wallet: &PlatformWallet,
    identity: &str,
    amount: WithdrawAmount,
) -> Result<WithdrawQuote, CreditsError> {
    let id = parse_identity(identity)?;
    own(wallet, id).await?;
    let (credits, rules) = plan(net, id, amount).await?;
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
    let (credits, rules) = plan(net, id, amount).await?;
    let version = net.version();
    let expected = expected_duffs(credits, version)?;
    let quote = credits + rules.fee;
    let signs = (index, &me);
    let lease = write_lease(session, wallet_id, grant, (&[FlowKind::Withdraw], quote), signs, || {
        Ok(())
    })
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
    let sent = leased(&lease, async {
        let nonce = net.nonce(id).await.map_err(Unsent::Refused)?;
        let signed = transition(&me, (script.clone(), credits, nonce), &key, signer.clone(), version)
            .await
            .map_err(Unsent::Refused)?;
        Ok((nonce, net.broadcast(signed).await))
    })
    .await;
    drop(signer);
    permit.finish(matches!(sent, Ok((_, Ok(_)))));
    let (nonce, broadcast) = match sent {
        Ok(sent) => sent,
        // Nothing signed, so nothing went out.
        Err(Unsent::Refused(e) | Unsent::Unknown(e)) => return Err(e.into()),
    };
    let doc = document_id(&id, nonce, &script);
    let done = |remaining| WithdrawOutcome {
        credits,
        expected_duffs: Some(expected),
        remaining_credits: remaining,
        withdrawal: doc.to_string(Encoding::Base58),
    };
    let e = match broadcast {
        Ok(balance) => return Ok(done(Some(balance))),
        Err(Unsent::Refused(e)) => return Err(e.into()),
        Err(Unsent::Unknown(e)) => e,
    };
    // Signed, so it may have landed: the withdrawal's document decides, and
    // a failed read decides nothing.
    tracing::info!(identity = %identity, "a withdrawal's broadcast failed: {e}");
    match net.withdrawal(doc).await {
        Ok(Some(_)) => Ok(done(None)),
        read => {
            if let Err(again) = read {
                tracing::info!("reading the withdrawal back: {again}");
            }
            Err(PlatformError::BroadcastUnknown {
                artifact: doc.to_string(Encoding::Base58),
            }
            .into())
        }
    }
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
    let version = net.version();
    let mut documents = net.withdrawals(id, RECENT_WITHDRAWALS).await?;
    documents.retain(|d| d.owner_id() == id);
    documents.sort_by_key(|d| std::cmp::Reverse((d.updated_at(), d.created_at())));
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
        credits: p.get_integer(withdrawal::properties::AMOUNT).unwrap_or(0),
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
