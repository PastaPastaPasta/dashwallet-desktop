//! Usernames: rules, availability, registration, the own contest and user
//! search (DP1-03, DP1-04, DP2-03).
//!
//! DP1-03's part:
//! - [`check_username`]: dash-platform-queries' DPNS rules (`dpns_usernames.rs`
//!   at the pin, re-exported by dash-sdk), with the desktop's 23-character cap
//!   (DASHPAY §2.9; iOS `DW_MAX_USERNAME_LENGTH`). The normalized label is
//!   DPNS's homograph folding (`o`→`0`, `i` and `l`→`1`, lower case), the key
//!   that makes `alice` and `A11CE` one name.
//! - availability and the contest precheck: iOS `checkIfBlocked` and
//!   `contestPrecheck`. A free domain is not enough for a contested label: a
//!   past vote may have locked it, or a running vote holds it.
//! - extra names, paid from the identity's credits ([`DashPay::register_name`]);
//! - the temporary-name policy ([`check_temporary_name`]): a non-contested
//!   name registered next to a contested request, used as the main name until
//!   the contest resolves (F5);
//! - the main name: the user's pick in `dp_prefs`, which no sync rewrites
//!   (platform #4978's rule), resolved against the names the identity owns.
//!
//! Counterparts: `rs-platform-wallet-ffi` registers names through
//! `IdentityWallet::register_name_with_external_signer`, as here; the rules
//! are `rs-sdk-ffi/src/dpns/helpers.rs` (`dash_sdk_dpns_is_valid_username`,
//! `dash_sdk_dpns_get_validation_message`).

use std::future::Future;
use std::sync::Arc;

use dash_sdk::drive::config::DEFAULT_QUERY_LIMIT;
use dash_sdk::platform::Fetch;
use dash_sdk::platform::dpns_usernames::{
    convert_to_homograph_safe_chars, is_contested_username, is_valid_username,
};
use dash_sdk::query_types::IdentityBalance;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::{KeyType, Purpose, SecurityLevel};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::SystemDataContract;
use dpp::version::PlatformVersion;
use dpp::voting::vote_info_storage::contested_document_vote_poll_winner_info::ContestedDocumentVotePollWinnerInfo;
use dpp::voting::vote_polls::contested_document_resource_vote_poll::required_vote_resolution_fund_to_join;
use dw_vault::{GrantKind, SignerScope};
use platform_wallet::{DpnsNameInfo, ManagedIdentity, PlatformWallet, WalletPersister};
use serde::{Deserialize, Serialize};

use super::contacts::Relation;
use super::dashpay::{DashPay, stub};
use super::errors::{IdentityError, NameError, PlatformError};
use super::flows::BudgetPurpose;
use super::identity::KeyPurpose;
use super::signers::VaultIdentitySigner;
use crate::session::Manager;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

const MIN_LENGTH: usize = 3;
/// The desktop's cap; DPNS allows 63 (DASHPAY §2.9).
const MAX_LENGTH: usize = 23;

/// `dp_prefs` keys. The pick is the label as the identity owns it.
const PREF_MAIN_NAME: &str = "main_name";
/// The temporary name registered while the identity's contest was open.
const PREF_TEMPORARY_NAME: &str = "temporary_name";
/// The label the identity last contended for.
const PREF_CONTESTED_NAME: &str = "contested_name";
/// DPNS's contested document type.
const DPNS_DOMAIN: &str = "domain";

/// `check_username`'s verdict: the inline rule checklist (F4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsernameCheck {
    pub valid: bool,
    pub normalized: String,
    pub contested: bool,
    pub rules: Vec<UsernameRuleCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsernameRuleCheck {
    pub rule: UsernameRule,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsernameRule {
    /// At least 3 characters.
    MinLength,
    /// At most 23 characters.
    MaxLength,
    /// Only `[A-Za-z0-9-]`.
    AllowedCharacters,
    /// No hyphen at either end.
    NoEdgeHyphen,
    /// No two hyphens in a row (dash-platform-queries' `is_valid_username`).
    NoDoubleHyphen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NameAvailability {
    /// `rules` are the rules the label breaks.
    Invalid {
        rules: Vec<UsernameRule>,
    },
    Available {
        contested: bool,
    },
    Taken {
        owner: Option<String>,
    },
    ContestOpen {
        ends_at: Option<u64>,
        contenders: u32,
    },
    Locked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NameOutcome {
    Registered,
    ContestStarted { ends_at: Option<u64> },
}

/// The own contest (F5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestStatus {
    pub label: String,
    pub state: ContestState,
    pub ends_at: Option<u64>,
    pub contenders: Vec<ContestContender>,
    pub lock_votes: Option<u32>,
    pub abstain_votes: Option<u32>,
    /// The name used until the contest resolves.
    pub temporary_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContestState {
    Open,
    Won,
    Lost { winner: Option<String> },
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestContender {
    pub identity: String,
    pub votes: Option<u32>,
    pub is_self: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserHit {
    pub identity: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub relation: Relation,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

/// Validates a username label offline (F4). Pure; needs no session. The
/// label is checked as given: the host trims it. `contested` is set only for
/// a valid label.
pub fn check_username(label: &str) -> Result<UsernameCheck, NameError> {
    Ok(username_check(label))
}

fn username_check(label: &str) -> UsernameCheck {
    let length = label.chars().count();
    let rules: Vec<UsernameRuleCheck> = [
        (UsernameRule::MinLength, length >= MIN_LENGTH),
        (UsernameRule::MaxLength, length <= MAX_LENGTH),
        (
            UsernameRule::AllowedCharacters,
            label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        ),
        (
            UsernameRule::NoEdgeHyphen,
            !label.starts_with('-') && !label.ends_with('-'),
        ),
        (UsernameRule::NoDoubleHyphen, !label.contains("--")),
    ]
    .into_iter()
    .map(|(rule, passed)| UsernameRuleCheck { rule, passed })
    .collect();
    let valid = rules.iter().all(|r| r.passed);
    // The rules are `is_valid_username`'s with a lower cap, so the engine
    // never accepts a label DPNS refuses.
    debug_assert!(!valid || is_valid_username(label));
    UsernameCheck {
        valid,
        normalized: convert_to_homograph_safe_chars(label),
        contested: valid && is_contested_username(label),
        rules,
    }
}

fn broken_rules(check: &UsernameCheck) -> Vec<UsernameRule> {
    check
        .rules
        .iter()
        .filter(|r| !r.passed)
        .map(|r| r.rule)
        .collect()
}

/// `label`'s check, or `name.invalid` with the rules it breaks.
fn valid_label(label: &str) -> Result<UsernameCheck, NameError> {
    let check = username_check(label);
    if check.valid {
        Ok(check)
    } else {
        Err(NameError::Invalid {
            rules: broken_rules(&check),
        })
    }
}

/// Whether two labels are one DPNS name (homograph folding).
fn same_name(a: &str, b: &str) -> bool {
    convert_to_homograph_safe_chars(a) == convert_to_homograph_safe_chars(b)
}

/// The temporary-name policy (F5; iOS `TemporaryUsernameFieldModel`, Android's
/// "instant" name). A temporary name is registered at once next to a
/// contested request, so it must be valid, must not itself be contested, and
/// must not be the requested name under homograph folding. iOS suggests
/// `<requested>2`: any digit from 2 to 9 takes a label out of the contested
/// set. DP1-02's `registration_quote` applies this to `temporary_label`.
#[cfg_attr(not(test), expect(dead_code, reason = "DP1-02 calls it"))]
pub(crate) fn check_temporary_name(requested: &str, temporary: &str) -> Result<(), NameError> {
    let check = valid_label(temporary)?;
    let refuse = |detail: &str| {
        Err(PlatformError::InvalidArgument {
            detail: detail.into(),
        }
        .into())
    };
    if check.contested {
        return refuse("a temporary name must not be contested");
    }
    if same_name(requested, temporary) {
        return refuse("a temporary name must differ from the requested name");
    }
    Ok(())
}

/// The identity's main name, in this order:
/// 1. the user's pick, while the identity owns it. A pick the identity no
///    longer owns stays stored and counts again if the name comes back: no
///    sync rewrites it (platform #4978, `PersistentIdentity.ownedMainDpnsName`);
/// 2. the temporary name, while a contest of the identity is open (F5);
/// 3. the label the identity contended for, once it owns it (a won contest);
/// 4. the oldest owned name.
///
/// `owned` is the identity's DPNS list in acquisition order; a label it is
/// still contending for (`open_contests`) is not owned yet.
pub(crate) fn resolve_main_name(
    owned: &[DpnsNameInfo],
    open_contests: &[String],
    prefs: &MainNamePrefs,
) -> Option<String> {
    let owned = owned_labels(owned, open_contests);
    let owned_as = |label: &String| {
        owned
            .iter()
            .find(|n| same_name(n, label))
            .map(|n| n.to_string())
    };
    let temporary = || {
        prefs
            .temporary
            .as_ref()
            .filter(|_| !open_contests.is_empty())
            .and_then(owned_as)
    };
    prefs
        .pick
        .as_ref()
        .and_then(owned_as)
        .or_else(temporary)
        .or_else(|| prefs.contested.as_ref().and_then(owned_as))
        .or_else(|| owned.first().map(|n| n.to_string()))
}

/// The labels of `owned` that are not in `open_contests`.
fn owned_labels<'a>(owned: &'a [DpnsNameInfo], open_contests: &[String]) -> Vec<&'a str> {
    owned
        .iter()
        .map(|n| n.label.as_str())
        .filter(|label| !open_contests.iter().any(|c| same_name(c, label)))
        .collect()
}

/// An identity's `dp_prefs` for its main name.
#[derive(Debug, Default)]
pub(crate) struct MainNamePrefs {
    /// The user's pick, as the identity spells it.
    pub(crate) pick: Option<String>,
    /// The name registered while a contest of the identity was open.
    pub(crate) temporary: Option<String>,
    /// The label the identity last contended for.
    pub(crate) contested: Option<String>,
}

impl MainNamePrefs {
    const KEYS: [&str; 3] = [PREF_MAIN_NAME, PREF_TEMPORARY_NAME, PREF_CONTESTED_NAME];

    fn from_values(values: Vec<Option<String>>) -> Self {
        let mut values = values.into_iter();
        Self {
            pick: values.next().flatten(),
            temporary: values.next().flatten(),
            contested: values.next().flatten(),
        }
    }
}

/// What the network says about a valid label.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Lookup {
    pub(crate) availability: NameAvailability,
    /// A running contest's contenders, as far as one page of the vote state
    /// lists them.
    pub(crate) contenders: Vec<Identifier>,
}

impl Lookup {
    fn verdict(availability: NameAvailability) -> Self {
        Self {
            availability,
            contenders: Vec::new(),
        }
    }

    /// Whether `contenders` may miss some: one vote-state page is full.
    fn contenders_truncated(&self) -> bool {
        self.contenders.len() >= usize::from(DEFAULT_QUERY_LIMIT)
    }
}

/// The verdict of a contested label's vote state when no domain exists for
/// it (iOS `checkIfBlocked`): a winner makes it `Taken`, a lock `Locked`; no
/// contenders leave it `Available`, and contenders mean a running contest
/// (`ends_at` still unknown).
pub(crate) fn vote_verdict(
    winner: Option<ContestedDocumentVotePollWinnerInfo>,
    contenders: Vec<Identifier>,
) -> Lookup {
    match winner {
        Some(ContestedDocumentVotePollWinnerInfo::WonByIdentity(id)) => {
            return Lookup::verdict(NameAvailability::Taken {
                owner: Some(id.to_string(Encoding::Base58)),
            });
        }
        Some(ContestedDocumentVotePollWinnerInfo::Locked) => {
            return Lookup::verdict(NameAvailability::Locked);
        }
        Some(ContestedDocumentVotePollWinnerInfo::NoWinner) | None => {}
    }
    if contenders.is_empty() {
        return Lookup::verdict(NameAvailability::Available { contested: true });
    }
    Lookup {
        availability: NameAvailability::ContestOpen {
            ends_at: None,
            contenders: u32::try_from(contenders.len()).unwrap_or(u32::MAX),
        },
        contenders,
    }
}

/// The availability of a valid label and, for a contested one, the contest
/// precheck (iOS `checkIfBlocked`, `contestPrecheck`): a domain document
/// makes it `Taken`; a contested label with no domain is then judged by its
/// vote state ([`vote_verdict`]). A failed domain query is an error; a
/// failed vote-state query only leaves the contest `Unknown`, as iOS does.
async fn lookup(sdk: &dash_sdk::Sdk, check: &UsernameCheck) -> Result<Lookup, PlatformError> {
    let normalized = &check.normalized;
    if let Some(owner) = sdk.resolve_dpns_name(normalized).await? {
        return Ok(Lookup::verdict(NameAvailability::Taken {
            owner: Some(owner.to_string(Encoding::Base58)),
        }));
    }
    if !check.contested {
        return Ok(Lookup::verdict(NameAvailability::Available {
            contested: false,
        }));
    }
    // The vote poll is keyed by the normalized label.
    let state = match sdk.get_contested_dpns_vote_state(normalized, None).await {
        Ok(state) => state,
        Err(e) => {
            tracing::info!(label = %normalized, "contest precheck unavailable: {e}");
            return Ok(Lookup::verdict(NameAvailability::Unknown));
        }
    };
    let winner = state.winner.map(|(winner, _)| winner);
    let mut found = vote_verdict(winner, state.contenders.into_keys().collect());
    if let NameAvailability::ContestOpen { ends_at, .. } = &mut found.availability {
        *ends_at = contest_end(sdk, normalized).await;
    }
    Ok(found)
}

/// When the running contest for `normalized` ends, UNIX seconds. The vote
/// state carries no deadline; the list of current contests does. Best
/// effort: `None` when that query fails or does not list the label.
async fn contest_end(sdk: &dash_sdk::Sdk, normalized: &str) -> Option<u64> {
    let now_ms = crate::events::unix_now().saturating_mul(1000);
    match sdk
        .get_current_dpns_contests(Some(now_ms), None, None)
        .await
    {
        Ok(contests) => contests.get(normalized).map(|ms| ms / 1000),
        Err(e) => {
            tracing::info!(label = %normalized, "contest end unavailable: {e}");
            None
        }
    }
}

/// Until when a contest ending at `ends_at` takes new contenders, UNIX
/// seconds: `allow_other_contenders_time` after the poll starts, which is
/// the poll duration before its end (rs-platform-version
/// `VotingValidationVersions`, `DPPVotingVersions`; iOS
/// `contenderJoinDeadline`). Mainnet uses the mainnet values, every other
/// network the test-network ones.
fn join_deadline(ends_at: u64, network: &DashNetwork, version: &PlatformVersion) -> u64 {
    let poll = &version.dpp.voting_versions;
    let join = &version.dpp.validation.voting;
    let (poll_ms, join_ms) = match network {
        DashNetwork::Mainnet => (
            poll.default_vote_poll_time_duration_mainnet_ms,
            join.allow_other_contenders_time_mainnet_ms,
        ),
        _ => (
            poll.default_vote_poll_time_duration_test_network_ms,
            join.allow_other_contenders_time_testing_ms,
        ),
    };
    ends_at.saturating_sub(poll_ms.saturating_sub(join_ms) / 1000)
}

/// What `register_name` makes of a [`lookup`] for `identity`: an answer
/// without a write (`Ok(Some)`), a refusal, or `Ok(None)` to register.
/// `join_until` is the running contest's join deadline, when its end is
/// known; with no known end the join is attempted. A contest whose
/// contenders fill a vote-state page cannot be priced or searched for the
/// identity, so it is refused like a closed one.
pub(crate) fn registration_step(
    found: &Lookup,
    identity: &Identifier,
    join_until: Option<u64>,
    now: u64,
) -> Result<Option<NameOutcome>, NameError> {
    match &found.availability {
        NameAvailability::Available { .. } => Ok(None),
        NameAvailability::Taken { owner } => {
            if owner.as_deref() == Some(identity.to_string(Encoding::Base58).as_str()) {
                Ok(Some(NameOutcome::Registered))
            } else {
                Err(NameError::Taken)
            }
        }
        NameAvailability::Locked => Err(NameError::Locked),
        NameAvailability::ContestOpen { ends_at, .. } => {
            if found.contenders.contains(identity) {
                Ok(Some(NameOutcome::ContestStarted { ends_at: *ends_at }))
            } else if found.contenders_truncated() || join_until.is_some_and(|until| until <= now) {
                Err(NameError::ContestOpen)
            } else {
                Ok(None)
            }
        }
        NameAvailability::Unknown => Err(PlatformError::Unavailable.into()),
        NameAvailability::Invalid { rules } => Err(NameError::Invalid {
            rules: rules.clone(),
        }),
    }
}

/// The fund a contested request pays to join a contest of `contenders`
/// (rs-dpp `required_vote_resolution_fund_to_join`: the contest fund,
/// doubled from 250 contenders on), checked against the identity's credits.
async fn contest_fund(
    sdk: &dash_sdk::Sdk,
    identity: Identifier,
    contenders: usize,
) -> Result<u64, NameError> {
    let fund = required_vote_resolution_fund_to_join(
        &SystemDataContract::DPNS.id(),
        DPNS_DOMAIN,
        u16::try_from(contenders).unwrap_or(u16::MAX),
        sdk.version(),
    );
    let available = IdentityBalance::fetch(sdk, identity)
        .await
        .map_err(PlatformError::from)?
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    if available < fund {
        return Err(PlatformError::InsufficientCredits {
            needed: fund,
            available,
        }
        .into());
    }
    Ok(fund)
}

/// Records a contested registration: platform-wallet appends the label to
/// the identity's names, but its domain exists only once the vote is won,
/// so it moves to the identity's open contests (normalized, as the library's
/// contest sweep keeps them).
pub(crate) fn record_contest(
    managed: &mut ManagedIdentity,
    label: &str,
    persister: &WalletPersister,
) {
    let names = managed
        .dpns_names
        .iter()
        .filter(|n| !same_name(&n.label, label))
        .cloned()
        .collect();
    managed.set_dpns_names(names, persister);
    managed.add_contested_dpns_name(convert_to_homograph_safe_chars(label), persister);
}

fn parse_identity(identity: &str) -> Result<Identifier, NameError> {
    Identifier::from_string(identity, Encoding::Base58).map_err(|_| {
        PlatformError::InvalidArgument {
            detail: "identity is not a base58 identifier".into(),
        }
        .into()
    })
}

fn engine(e: EngineError) -> NameError {
    PlatformError::from(e).into()
}

async fn platform_wallet(
    manager: &Manager,
    wallet_id: WalletId,
) -> Result<Arc<PlatformWallet>, NameError> {
    manager
        .get_wallet(&wallet_id.0)
        .await
        .ok_or_else(|| PlatformError::WalletNotFound.into())
}

/// One identity of the wallet as platform-wallet holds it in memory.
struct OwnNames {
    index: Option<u32>,
    owned: Vec<DpnsNameInfo>,
    open_contests: Vec<String>,
    /// It has a key that may sign a DPNS document: a HIGH or CRITICAL
    /// ECDSA authentication key (platform-wallet `register_name_with_*`).
    can_sign_documents: bool,
}

async fn own_names(wallet: &PlatformWallet, identity: &Identifier) -> Result<OwnNames, NameError> {
    let state = wallet.state().await;
    let managed = state
        .identity_manager
        .managed_identity(identity)
        .filter(|m| m.wallet_id == Some(wallet.wallet_id()))
        .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
    let can_sign_documents = managed
        .identity
        .get_first_public_key_matching(
            Purpose::AUTHENTICATION,
            [SecurityLevel::HIGH, SecurityLevel::CRITICAL].into(),
            [KeyType::ECDSA_SECP256K1].into(),
            false,
        )
        .is_some();
    Ok(OwnNames {
        index: managed.identity_index,
        owned: managed.dpns_names.clone(),
        open_contests: managed.contested_dpns_names.clone(),
        can_sign_documents,
    })
}

/// A `PlatformIdentity` signer for the identity at `index`, from a
/// `PlatformOp` grant whose credit cap covers `needed`. The hold, if any,
/// must outlive the signer's use (dw-vault `platform_signer_held`).
async fn identity_signer(
    session: &NetworkSession,
    wallet_id: WalletId,
    grant: String,
    index: u32,
    needed: u64,
) -> Result<(VaultIdentitySigner, Option<dw_vault::KeyHold>), NameError> {
    let vault = session.vault.clone();
    let (signer, hold) = tokio::task::spawn_blocking(move || {
        let mut token = vault.redeem_grant(&grant, GrantKind::PlatformOp, Some(&wallet_id.0))?;
        let remaining = token.max_credits().unwrap_or(0);
        if needed > remaining {
            return Err(PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Credits,
                needed,
                remaining,
            });
        }
        let hold = vault.hold_key(std::slice::from_mut(&mut token))?;
        let scope = SignerScope::PlatformIdentity;
        let signer = match &hold {
            Some(hold) => vault.platform_signer_held(&wallet_id.0, hold, &token, scope)?,
            None => vault.platform_signer(&wallet_id.0, &token, scope)?,
        };
        Ok((signer, hold))
    })
    .await
    .map_err(|e| engine(e.into()))??;
    let signer = VaultIdentitySigner::new(signer, [index]).map_err(engine)?;
    Ok((signer, hold))
}

impl DashPay {
    /// Runs `body` on the engine runtime (m4 §1: any executor may poll a
    /// call) inside a session operation, with the session, its manager and
    /// this wallet's platform wallet (`wallet_not_found` otherwise).
    async fn on_wallet<T, F, Fut>(&self, body: F) -> Result<T, NameError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<NetworkSession>, Arc<Manager>, Arc<PlatformWallet>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, NameError>> + Send + 'static,
    {
        let session = Arc::clone(&self.session);
        let wallet_id = self.wallet_id;
        self.session
            .rt
            .spawn(async move {
                let _op = session.enter().await.map_err(engine)?;
                let manager = session.manager().map_err(engine)?;
                let wallet = platform_wallet(&manager, wallet_id).await?;
                body(Arc::clone(&session), manager, wallet).await
            })
            .await
            .map_err(|e| engine(e.into()))?
    }

    /// The identity's main-name prefs (keyed by its canonical base58).
    async fn main_name_prefs(
        session: &NetworkSession,
        wallet_id: WalletId,
        identity: &Identifier,
    ) -> Result<MainNamePrefs, NameError> {
        let (wallet_hex, identity) = (wallet_id.to_string(), identity.to_string(Encoding::Base58));
        let values = session
            .appdb_op(move |db| db.identity_dp_prefs(&wallet_hex, &identity, &MainNamePrefs::KEYS))
            .await
            .map_err(engine)?;
        Ok(MainNamePrefs::from_values(values))
    }

    /// Stores (or with `None` deletes) one main-name pref of the identity.
    async fn set_main_name_pref(
        session: &NetworkSession,
        wallet_id: WalletId,
        identity: &Identifier,
        key: &'static str,
        value: Option<String>,
    ) -> Result<(), NameError> {
        let (wallet_hex, identity) = (wallet_id.to_string(), identity.to_string(Encoding::Base58));
        session
            .appdb_op(move |db| db.set_dp_pref(&wallet_hex, &identity, key, value.as_deref()))
            .await
            .map_err(engine)
    }

    /// The label's checklist verdict, then the network's (F4): see
    /// [`lookup`]. Another identity's open contest is `ContestOpen`; whether
    /// it still takes contenders is for `register_name` to say.
    pub async fn name_availability(&self, label: String) -> Result<NameAvailability, NameError> {
        let check = username_check(&label);
        if !check.valid {
            return Ok(NameAvailability::Invalid {
                rules: broken_rules(&check),
            });
        }
        self.on_wallet(move |_, manager, _| async move {
            Ok(lookup(&manager.sdk_arc(), &check).await?.availability)
        })
        .await
    }

    /// Registers `label` for `identity`, paid from its credits: an extra
    /// name, a temporary name next to the identity's open contest, or the
    /// name of a registration that parked before it. Idempotent: a name the
    /// identity already owns is `Registered`, and a contest it already
    /// contends in is `ContestStarted`; neither redeems the grant.
    ///
    /// Refused before the grant is redeemed: a bad label (`name.invalid`),
    /// an identity with no key that may sign a DPNS document
    /// (`identity.keys_missing`), another owner (`name.taken`), a locked
    /// label (`name.locked`), a contest past its join deadline or too large
    /// to price (`name.contest_open`), an unknown contest state
    /// (`platform.unavailable`), and for a contested label a credit balance
    /// below the fund to join (`platform.insufficient_credits`). The grant
    /// is a `PlatformOp` whose `max_credits` covers that fund
    /// (`platform.grant_exceeded`); the registration fee is not budgeted.
    ///
    /// A registered contested label moves from the identity's names to its
    /// open contests ([`record_contest`]). A non-contested name registered
    /// while a contest is open is the identity's temporary name.
    ///
    /// When the write fails, the domain is looked up once more: a name that
    /// landed for the identity is `Registered`, one another identity took
    /// is `name.taken`. Until E0-04's dispatch fence lands (P2a), any other
    /// failure after the hand-off is the library's error, not
    /// `platform.broadcast_unknown`.
    pub async fn register_name(
        &self,
        identity: String,
        label: String,
        grant: String,
    ) -> Result<NameOutcome, NameError> {
        let check = valid_label(&label)?;
        let identity_id = parse_identity(&identity)?;
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, manager, wallet| async move {
            let names = own_names(&wallet, &identity_id).await?;
            let owned = owned_labels(&names.owned, &names.open_contests);
            if owned.iter().any(|n| same_name(n, &label)) {
                return Ok(NameOutcome::Registered);
            }
            let index = names.index.ok_or(PlatformError::SignerUnavailable)?;
            if !names.can_sign_documents {
                return Err(PlatformError::Identity(IdentityError::KeysMissing {
                    purpose: KeyPurpose::Authentication,
                })
                .into());
            }
            let sdk = manager.sdk_arc();
            let found = lookup(&sdk, &check).await?;
            let join_until = match &found.availability {
                NameAvailability::ContestOpen { ends_at, .. } => {
                    ends_at.map(|end| join_deadline(end, &session.network, sdk.version()))
                }
                _ => None,
            };
            if let Some(outcome) =
                registration_step(&found, &identity_id, join_until, crate::events::unix_now())?
            {
                return Ok(outcome);
            }
            let fund = match check.contested {
                true => Some(contest_fund(&sdk, identity_id, found.contenders.len()).await?),
                false => None,
            };
            let (signer, hold) =
                identity_signer(&session, wallet_id, grant, index, fund.unwrap_or(0)).await?;
            // The fund priced above caps what the contest may take.
            let registered = wallet
                .identity()
                .register_name_with_external_signer(&identity_id, &label, fund, &signer)
                .await;
            drop((signer, hold));
            if let Err(e) = registered {
                tracing::info!(label = %check.normalized, "name registration failed: {e}");
                // platform-wallet flattens the SDK's errors into text; the
                // domain says whether the write landed or lost a race.
                let owner = sdk
                    .resolve_dpns_name(&check.normalized)
                    .await
                    .ok()
                    .flatten();
                return match owner {
                    Some(owner) if owner == identity_id => Ok(NameOutcome::Registered),
                    Some(_) => Err(NameError::Taken),
                    None => Err(PlatformError::from(e).into()),
                };
            }

            if check.contested {
                {
                    let mut state = wallet.state_mut().await;
                    if let Some(managed) = state.identity_manager.managed_identity_mut(&identity_id)
                    {
                        record_contest(managed, &label, wallet.persister());
                    }
                }
                Self::set_main_name_pref(
                    &session,
                    wallet_id,
                    &identity_id,
                    PREF_CONTESTED_NAME,
                    Some(label),
                )
                .await?;
                return Ok(NameOutcome::ContestStarted {
                    ends_at: contest_end(&sdk, &check.normalized).await,
                });
            }
            if !names.open_contests.is_empty() {
                Self::set_main_name_pref(
                    &session,
                    wallet_id,
                    &identity_id,
                    PREF_TEMPORARY_NAME,
                    Some(label),
                )
                .await?;
            }
            Ok(NameOutcome::Registered)
        })
        .await
    }

    /// Picks `label` as the identity's main name, or clears the pick with
    /// `None`. The label must be one the identity owns now
    /// (`invalid_argument` otherwise); the pick is stored as the identity
    /// spells it. No sync rewrites it ([`resolve_main_name`]).
    pub async fn set_main_name(
        &self,
        identity: String,
        label: Option<String>,
    ) -> Result<(), NameError> {
        let identity_id = parse_identity(&identity)?;
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, _, wallet| async move {
            let names = own_names(&wallet, &identity_id).await?;
            let owned = owned_labels(&names.owned, &names.open_contests);
            let pick = label
                .map(|label| {
                    owned
                        .iter()
                        .find(|n| same_name(n, &label))
                        .map(|n| n.to_string())
                        .ok_or_else(|| PlatformError::InvalidArgument {
                            detail: "not a name this identity owns".into(),
                        })
                })
                .transpose()?;
            Self::set_main_name_pref(&session, wallet_id, &identity_id, PREF_MAIN_NAME, pick).await
        })
        .await
    }

    /// The identity's main name ([`resolve_main_name`]); `None` while it owns
    /// no name.
    pub async fn main_name(&self, identity: String) -> Result<Option<String>, NameError> {
        let identity_id = parse_identity(&identity)?;
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, _, wallet| async move {
            let names = own_names(&wallet, &identity_id).await?;
            let prefs = Self::main_name_prefs(&session, wallet_id, &identity_id).await?;
            Ok(resolve_main_name(
                &names.owned,
                &names.open_contests,
                &prefs,
            ))
        })
        .await
    }

    #[expect(unused_variables, reason = "stub until DP1-04")]
    pub async fn contest_status(
        &self,
        identity: String,
        label: String,
    ) -> Result<ContestStatus, NameError> {
        stub("DashPay.contest_status")
    }

    #[expect(unused_variables, reason = "stub until DP2-03")]
    pub async fn search_users(
        &self,
        prefix: String,
        limit: u32,
    ) -> Result<Vec<UserHit>, NameError> {
        stub("DashPay.search_users")
    }

    #[expect(unused_variables, reason = "stub until DP2-03")]
    pub async fn resolve_user(&self, username: String) -> Result<Option<UserHit>, NameError> {
        stub("DashPay.resolve_user")
    }
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
