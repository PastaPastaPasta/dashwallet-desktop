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
//! - extra names, paid from the identity's credits ([`DashPay::register_name`]),
//!   with the fees and any contest fund budgeted against the grant and the
//!   balance before anything is spent ([`name_cost`]);
//! - the temporary-name policy ([`check_temporary_name`]): a non-contested
//!   name registered next to a contested request, used as the main name until
//!   the contest resolves (F5);
//! - the main name: the user's pick in `dp_prefs`, which no sync rewrites
//!   (platform #4978's rule), resolved against the names the identity owns
//!   ([`resolve_main_name`], the one rule: DP1-05's `identities()` applies
//!   it too). The rows are written and read only through DP1-05's
//!   `NetworkSession::set_main_name`, `set_name_pref` and `name_prefs`.
//!
//! Counterparts: `rs-platform-wallet-ffi` registers names through
//! `IdentityWallet::register_name_with_external_signer`, as here; the rules
//! are `rs-sdk-ffi/src/dpns/helpers.rs` (`dash_sdk_dpns_is_valid_username`,
//! `dash_sdk_dpns_get_validation_message`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use dash_sdk::drive::config::DEFAULT_QUERY_LIMIT;
use dash_sdk::platform::dpns_usernames::{
    convert_to_homograph_safe_chars, is_contested_username, is_valid_username,
};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::{KeyType, Purpose, SecurityLevel};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::SystemDataContract;
use dpp::version::PlatformVersion;
use dpp::voting::vote_info_storage::contested_document_vote_poll_winner_info::ContestedDocumentVotePollWinnerInfo;
use dpp::voting::vote_polls::contested_document_resource_vote_poll::required_vote_resolution_fund_to_join;
use dw_vault::{GrantKind, GrantPurpose, SignerScope};
use platform_wallet::{DpnsNameInfo, ManagedIdentity, PlatformWallet, WalletPersister};
use serde::{Deserialize, Serialize};

use super::contacts::Relation;
use super::dashpay::{DashPay, stub};
use super::errors::{IdentityError, NameError, PlatformError};
use super::flows::{BudgetPurpose, GrantRequest};
use super::identity::KeyPurpose;
use super::recovery::{MAIN_NAME_PREF, owned_names, row_owned};
use super::signers::VaultIdentitySigner;
use crate::session::Manager;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

const MIN_LENGTH: usize = 3;
/// The desktop's cap; DPNS allows 63 (DASHPAY §2.9).
const MAX_LENGTH: usize = 23;

// `dp_prefs` keys besides the pick ([`MAIN_NAME_PREF`], the label as the
// identity owns it).
/// The temporary name registered while the identity's contest was open.
const PREF_TEMPORARY_NAME: &str = "temporary_name";
/// The label the identity last contended for.
const PREF_CONTESTED_NAME: &str = "contested_name";
/// Labels whose write may be in flight, one per line (a valid label has no
/// line break): an intent record only, added just before a write and
/// removed by the first definitive Platform answer about it. A pending
/// label is never an owned name ([`own_names`]); Platform alone says what
/// became of it.
const PREF_PENDING_NAME: &str = "pending_name";
/// DPNS's contested document type.
const DPNS_DOMAIN: &str = "domain";

/// Bounds on the fees of a name's two document transitions, in credits: a
/// conservative constant per transition type until DP1-06's cost table
/// lands (E0-04 design §4.2, Q7). Platform states only a minimum per
/// transition (`document_batch_sub_transition`, 100,000); the real fee
/// follows the storage written. On testnet (2026-10-09, `gasUsed` of the
/// explorer's `/identity/<id>/transactions`) preorders cost 23.6M–32.3M and
/// plain 11-character domains 34.0M–41.3M; the largest contested-domain
/// batches 79.7M–107.4M. Each bound is about three times the largest.
const PREORDER_FEE_BOUND: u64 = 100_000_000;
const DOMAIN_FEE_BOUND: u64 = 300_000_000;
/// How long past a contest's join deadline its closing is taken as final,
/// seconds: DAPI nodes lag, and the deadline is read against this clock.
const CLOSED_MARGIN: u64 = 10 * 60;
/// What a name's preorder and domain may cost in fees, together.
pub(crate) const NAME_FEE_BOUND: u64 = PREORDER_FEE_BOUND + DOMAIN_FEE_BOUND;

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
/// 4. the name the identity got first, by Platform's acquisition time
///    ([`owned_names`]); names with no time come last, ties in list order
///    (DP1-05's default).
///
/// The one rule: `DashPay::main_name` and DP1-05's `identities()` both
/// apply it, over [`evident_names`].
///
/// `owned` is the identity's names with their acquisition times; a label it
/// is still contending for (`open_contests`) is not owned yet.
pub(crate) fn resolve_main_name(
    owned: &[(String, Option<u64>)],
    open_contests: &[String],
    prefs: &MainNamePrefs,
) -> Option<String> {
    let owned_as = |label: &String| owned_spelling(owned, open_contests, label);
    let first = || {
        owned_labels(owned, open_contests)
            .into_iter()
            .enumerate()
            .min_by_key(|(i, (_, at))| (at.is_none(), *at, *i))
            .map(|(_, (n, _))| n.to_string())
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
        .or_else(first)
}

/// `label` as the identity spells it, if it owns it (not in `open_contests`).
fn owned_spelling(
    owned: &[(String, Option<u64>)],
    open_contests: &[String],
    label: &str,
) -> Option<String> {
    owned_labels(owned, open_contests)
        .iter()
        .find(|(n, _)| same_name(n, label))
        .map(|(n, _)| n.to_string())
}

/// The names of `owned` that are not in `open_contests`.
fn owned_labels<'a>(
    owned: &'a [(String, Option<u64>)],
    open_contests: &[String],
) -> Vec<&'a (String, Option<u64>)> {
    owned
        .iter()
        .filter(|(label, _)| !open_contests.iter().any(|c| same_name(c, label)))
        .collect()
}

/// The names of `owned` ([`owned_names`]) that Platform evidence shows the
/// identity owns: a `pending` label (its write may be in flight) only if a
/// marketplace row says so (`row_owned`). The library lists a label once
/// its write returns, a contested one too, so a pending label there may be
/// a running contest or a write cut short.
pub(crate) fn evident_names(
    mut owned: Vec<(String, Option<u64>)>,
    pending: &[String],
    row_owned: impl Fn(&str) -> bool,
) -> Vec<(String, Option<u64>)> {
    owned.retain(|(label, _)| !pending.iter().any(|p| same_name(p, label)) || row_owned(label));
    owned
}

/// The labels an identity shows as its names: the evident ones it is not
/// still contending for.
pub(crate) fn shown_names(
    owned: &[(String, Option<u64>)],
    open_contests: &[String],
) -> Vec<String> {
    owned_labels(owned, open_contests)
        .into_iter()
        .map(|(label, _)| label.clone())
        .collect()
}

/// An identity's `dp_prefs` for its main name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MainNamePrefs {
    /// The user's pick, as the identity spells it.
    pub(crate) pick: Option<String>,
    /// The name registered while a contest of the identity was open.
    pub(crate) temporary: Option<String>,
    /// The label the identity last contended for.
    pub(crate) contested: Option<String>,
    /// Labels whose write may be in flight ([`PREF_PENDING_NAME`]).
    pub(crate) pending: Vec<String>,
}

impl MainNamePrefs {
    pub(crate) const KEYS: [&str; 4] = [
        MAIN_NAME_PREF,
        PREF_TEMPORARY_NAME,
        PREF_CONTESTED_NAME,
        PREF_PENDING_NAME,
    ];

    /// Applies one row of [`Self::KEYS`] as stored (`None`: unset).
    pub(crate) fn set(&mut self, key: &str, value: Option<String>) {
        match key {
            MAIN_NAME_PREF => self.pick = value,
            PREF_TEMPORARY_NAME => self.temporary = value,
            PREF_CONTESTED_NAME => self.contested = value,
            PREF_PENDING_NAME => {
                self.pending = value
                    .map(|v| {
                        v.lines()
                            .filter(|l| username_check(l).valid)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            _ => {}
        }
    }

    fn is_pending(&self, label: &str) -> bool {
        self.pending.iter().any(|l| same_name(l, label))
    }

    /// The [`PREF_PENDING_NAME`] value; `None` when nothing is pending.
    fn pending_value(&self) -> Option<String> {
        (!self.pending.is_empty()).then(|| self.pending.join("\n"))
    }
}

/// What a registration is for the identity, derived on each call from the
/// label and the identity's open contests as Platform last showed them,
/// never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameKind {
    /// A contested label: the identity contends, then owns it once won.
    Contested,
    /// A non-contested label while the identity contends for another: its
    /// temporary name (F5).
    Temporary,
    /// Any other non-contested label.
    Extra,
}

impl NameKind {
    fn of(check: &UsernameCheck, open_contests: &[String]) -> Self {
        let other_contests = open_contests
            .iter()
            .any(|c| !same_name(c, &check.normalized));
        match (check.contested, other_contests) {
            (true, _) => Self::Contested,
            (false, true) => Self::Temporary,
            (false, false) => Self::Extra,
        }
    }
}

/// What the network says about a valid label.
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// known. A new contender needs it: with no known end the join may be past
/// its window, and the preorder's fee would be spent on a domain Platform
/// refuses, so it is `platform.unavailable` (review DP1-03 R2). A contest
/// whose contenders fill a vote-state page cannot be priced or searched for
/// the identity, so it is refused like a closed one.
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
                return Ok(Some(NameOutcome::ContestStarted { ends_at: *ends_at }));
            }
            if found.contenders_truncated() {
                return Err(NameError::ContestOpen);
            }
            match join_until {
                None => Err(PlatformError::Unavailable.into()),
                Some(until) if until <= now => Err(NameError::ContestOpen),
                Some(_) => Ok(None),
            }
        }
        NameAvailability::Unknown => Err(PlatformError::Unavailable.into()),
        NameAvailability::Invalid { rules } => Err(NameError::Invalid {
            rules: rules.clone(),
        }),
    }
}

/// What registering a label costs in credits, at most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NameCost {
    /// A contested label's fund to join a contest of `contenders` (rs-dpp
    /// `required_vote_resolution_fund_to_join`: the contest fund, doubled
    /// from 250 contenders on); `None` for a plain label.
    pub(crate) fund: Option<u64>,
    /// The fund and [`NAME_FEE_BOUND`]: what the grant must cover and the
    /// identity's balance hold before the preorder goes out.
    pub(crate) total: u64,
}

pub(crate) fn name_cost(
    check: &UsernameCheck,
    contenders: usize,
    version: &PlatformVersion,
) -> NameCost {
    let fund = check.contested.then(|| {
        required_vote_resolution_fund_to_join(
            &SystemDataContract::DPNS.id(),
            DPNS_DOMAIN,
            u16::try_from(contenders).unwrap_or(u16::MAX),
            version,
        )
    });
    NameCost {
        fund,
        total: NAME_FEE_BOUND.saturating_add(fund.unwrap_or(0)),
    }
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
    /// With their acquisition times ([`owned_names`]).
    owned: Vec<(String, Option<u64>)>,
    open_contests: Vec<String>,
    /// It has a key that may sign a DPNS document: a HIGH or CRITICAL
    /// ECDSA authentication key (platform-wallet `register_name_with_*`).
    can_sign_documents: bool,
}

/// The identity's names as Platform evidence shows them ([`evident_names`]):
/// the library's list (DPNS documents, and a write the library saw
/// confirmed), without a `pending` label unless a marketplace row says the
/// identity owns it.
async fn own_names(
    wallet: &PlatformWallet,
    identity: &Identifier,
    pending: &[String],
) -> Result<OwnNames, NameError> {
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
    let rows = row_owned(*identity, &state.dpns_name_states);
    let owned = evident_names(
        owned_names(*identity, &managed.dpns_names, &state.dpns_name_states),
        pending,
        |label| rows.contains(&convert_to_homograph_safe_chars(label)),
    );
    Ok(OwnNames {
        index: managed.identity_index,
        owned,
        open_contests: managed.contested_dpns_names.clone(),
        can_sign_documents,
    })
}

impl OwnNames {
    fn owns(&self, label: &str) -> bool {
        owned_spelling(&self.owned, &self.open_contests, label).is_some()
    }
}

/// Refuses a grant that is not this wallet's `PlatformOp` or whose credit
/// cap is below `needed` (`platform.grant_exceeded`; a zero-credit grant
/// too), without consuming it: a refused registration leaves the grant for
/// the next try.
async fn check_credit_cap(
    session: &NetworkSession,
    wallet_id: WalletId,
    grant: &str,
    needed: u64,
) -> Result<(), NameError> {
    let vault = session.vault.clone();
    let grant = grant.to_string();
    let purpose = tokio::task::spawn_blocking(move || {
        vault.grant_purpose(&grant, GrantKind::PlatformOp, Some(&wallet_id.0))
    })
    .await
    .map_err(|e| engine(e.into()))?
    .map_err(PlatformError::from)?;
    let GrantPurpose::PlatformOp {
        max_credits: remaining,
        ..
    } = purpose
    else {
        return Err(PlatformError::GrantInvalid.into());
    };
    if needed > remaining {
        return Err(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed,
            remaining,
        }
        .into());
    }
    Ok(())
}

/// A `PlatformIdentity` signer for the identity at `index`, from a
/// `PlatformOp` grant [`check_credit_cap`] has passed. The hold, if any,
/// must outlive the signer's use (dw-vault `platform_signer_held`).
async fn identity_signer(
    session: &NetworkSession,
    wallet_id: WalletId,
    grant: String,
    index: u32,
) -> Result<(VaultIdentitySigner, Option<dw_vault::KeyHold>), NameError> {
    let vault = session.vault.clone();
    let (signer, hold) = tokio::task::spawn_blocking(move || {
        let mut token = vault.redeem_grant(&grant, GrantKind::PlatformOp, Some(&wallet_id.0))?;
        let hold = vault.hold_key(std::slice::from_mut(&mut token))?;
        let scope = SignerScope::PlatformIdentity;
        let signer = match &hold {
            Some(hold) => vault.platform_signer_held(&wallet_id.0, hold, &token, scope)?,
            None => vault.platform_signer(&wallet_id.0, &token, scope)?,
        };
        Ok::<_, PlatformError>((signer, hold))
    })
    .await
    .map_err(|e| engine(e.into()))??;
    let signer = VaultIdentitySigner::new(signer, [index]).map_err(engine)?;
    Ok((signer, hold))
}

/// One identity's locks.
#[derive(Default)]
struct IdentityLocks {
    /// Held for a whole registration: two calls for one label would each
    /// pay a preorder, and a call's [`NameKind`] reads the open contests
    /// another call is changing.
    serial: tokio::sync::Mutex<()>,
    /// Held, never across the network, around each change to the pending
    /// labels and the wallet's names that goes with it, and around a read
    /// of both, so a read never pairs one's old state with the other's new.
    state: tokio::sync::Mutex<()>,
}

fn identity_locks(wallet: WalletId, identity: Identifier) -> Arc<IdentityLocks> {
    type Locks = HashMap<(WalletId, Identifier), Weak<IdentityLocks>>;
    static LOCKS: LazyLock<Mutex<Locks>> = LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    let key = (wallet, identity);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(IdentityLocks::default());
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

/// One `register_name` call.
struct Registration {
    wallet_id: WalletId,
    identity: Identifier,
    label: String,
    check: UsernameCheck,
    /// A write of the label is this call's or was pending: only then may a
    /// Platform answer about it set the temporary or the won name (a name
    /// the identity got some other way is neither).
    ours: bool,
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
        session
            .name_prefs(wallet_id, identity.to_string(Encoding::Base58))
            .await
            .map_err(engine)
    }

    /// Stores (or with `None` deletes) one main-name pref of the identity,
    /// through DP1-05's writer, which keeps `identities()` in step.
    async fn set_main_name_pref(
        session: &NetworkSession,
        wallet_id: WalletId,
        identity: &Identifier,
        key: &'static str,
        value: Option<String>,
    ) -> Result<(), NameError> {
        let identity = identity.to_string(Encoding::Base58);
        let stored = match key {
            MAIN_NAME_PREF => session.set_main_name(wallet_id, identity, value).await,
            _ => session.set_name_pref(wallet_id, identity, key, value).await,
        };
        stored.map_err(engine)
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
    /// Refused before the grant is redeemed or anything is sent: a bad label
    /// (`name.invalid`), an identity with no key that may sign a DPNS
    /// document (`identity.keys_missing`), another owner (`name.taken`), a
    /// locked label (`name.locked`), a contest past its join deadline or too
    /// large to price (`name.contest_open`), an unknown contest state or,
    /// for a new contender, an unknown join deadline (`platform.unavailable`),
    /// a credit balance below [`name_cost`] (`platform.insufficient_credits`),
    /// and a grant that is not this wallet's `PlatformOp` or whose
    /// `max_credits` is below that cost (`platform.grant_exceeded`). These
    /// refusals leave the grant usable. The cost is budgeted, not enforced:
    /// the fees' bound ([`NAME_FEE_BOUND`]) is an estimate until DP1-06's
    /// cost table and E0-04's budgets, plus for a contested label the fund
    /// to join, which does cap what the contest takes.
    /// `grant_request(RegisterName)` quotes it. Calls for one identity run
    /// one at a time.
    ///
    /// Platform is the only source of what became of a registration. The
    /// label is stored as pending just before the write, as a record that a
    /// write may be in flight; while pending it is no owned name, so no
    /// read, pick or retry takes it for one ([`own_names`]). A retry, and
    /// any later registration of the identity (for its other pending
    /// labels), asks Platform and records the answer ([`DashPay::conclude`]):
    /// a name the identity owns is in its names, a contest it contends in
    /// is in its open contests, and a refusal for good (another owner, a
    /// lock, a closed contest it is not in) clears the label and any copy
    /// the library listed. A non-contested name registered while the
    /// identity contends for another is its temporary name.
    ///
    /// When the write fails, the label is looked up once more, with the
    /// same reading. Until E0-04's dispatch fence lands (P2a), any other
    /// failure after the hand-off is the library's error, not
    /// `platform.broadcast_unknown`, and the label stays pending.
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
            let net = SdkNet(manager.sdk_arc());
            let ask = Registration {
                wallet_id,
                identity: identity_id,
                label,
                check,
                ours: false,
            };
            Self::register(&net, &session, &wallet, ask, grant).await
        })
        .await
    }

    async fn register(
        net: &impl NameNet,
        session: &NetworkSession,
        wallet: &PlatformWallet,
        mut ask: Registration,
        grant: String,
    ) -> Result<NameOutcome, NameError> {
        let locks = identity_locks(ask.wallet_id, ask.identity);
        let _serial = locks.serial.lock().await;
        Self::conclude_others(net, session, wallet, &locks, &ask).await?;
        let (names, prefs) = Self::local_view(session, wallet, &locks, &ask).await?;
        ask.ours = prefs.is_pending(&ask.label);
        let kind = NameKind::of(&ask.check, &names.open_contests);
        if names.owns(&ask.label) {
            return Self::settle(session, wallet, &locks, &ask, kind, NameOutcome::Registered)
                .await;
        }
        let index = names.index.ok_or(PlatformError::SignerUnavailable)?;
        if !names.can_sign_documents {
            return Err(PlatformError::Identity(IdentityError::KeysMissing {
                purpose: KeyPurpose::Authentication,
            })
            .into());
        }
        let found = net.lookup(&ask.check).await?;
        let join_until = Self::join_until(session, net, &found);
        let now = crate::events::unix_now();
        if let Some(outcome) =
            Self::conclude(session, wallet, &locks, &ask, kind, &found, join_until, now).await?
        {
            return Ok(outcome);
        }
        let cost = name_cost(&ask.check, found.contenders.len(), net.version());
        let available = net
            .balance(ask.identity)
            .await?
            .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
        if available < cost.total {
            return Err(PlatformError::InsufficientCredits {
                needed: cost.total,
                available,
            }
            .into());
        }
        check_credit_cap(session, ask.wallet_id, &grant, cost.total).await?;

        // Before the grant is redeemed: a failure here spends nothing.
        {
            let _state = locks.state.lock().await;
            Self::update_pending(session, &ask, true).await?;
        }
        ask.ours = true;
        let (signer, hold) = identity_signer(session, ask.wallet_id, grant, index).await?;
        // The fund priced above caps what the contest may take.
        let written = net
            .submit(wallet, ask.identity, &ask.label, cost.fund, &signer)
            .await;
        drop((signer, hold));
        let outcome = match written {
            Ok(()) if ask.check.contested => NameOutcome::ContestStarted {
                ends_at: net.contest_end(&ask.check.normalized).await,
            },
            Ok(()) => NameOutcome::Registered,
            Err(e) => {
                tracing::info!(label = %ask.check.normalized, "name registration failed: {e}");
                // platform-wallet flattens the SDK's errors into text; the
                // label's state says whether the write landed or lost a race.
                let Ok(again) = net.lookup(&ask.check).await else {
                    return Err(e.into());
                };
                return match Self::conclude(session, wallet, &locks, &ask, kind, &again, None, now)
                    .await
                {
                    Ok(Some(outcome)) => Ok(outcome),
                    Err(refused @ (NameError::Taken | NameError::Locked)) => Err(refused),
                    Ok(None) => Err(e.into()),
                    Err(other) => {
                        tracing::info!(label = %ask.check.normalized, "after the failure: {other}");
                        Err(e.into())
                    }
                };
            }
        };
        Self::settle(session, wallet, &locks, &ask, kind, outcome).await
    }

    /// The running contest's join deadline in `found`, when its end is known.
    fn join_until(session: &NetworkSession, net: &impl NameNet, found: &Lookup) -> Option<u64> {
        match &found.availability {
            NameAvailability::ContestOpen { ends_at, .. } => {
                ends_at.map(|end| join_deadline(end, &session.network, net.version()))
            }
            _ => None,
        }
    }

    /// Asks Platform about the identity's pending labels other than `ask`'s
    /// and records each definitive answer, so a contest whose write was cut
    /// short shows before `ask` is classified. A label Platform cannot place
    /// yet stays pending. A failed lookup, or for a non-contested `ask` a
    /// contested label Platform cannot place (its vote state unknown, or a
    /// running contest whose page or deadline cannot show whether the
    /// identity joined), refuses the registration: `ask` could be the
    /// temporary name or not.
    async fn conclude_others(
        net: &impl NameNet,
        session: &NetworkSession,
        wallet: &PlatformWallet,
        locks: &IdentityLocks,
        ask: &Registration,
    ) -> Result<(), NameError> {
        let (_, prefs) = Self::local_view(session, wallet, locks, ask).await?;
        let others = prefs
            .pending
            .into_iter()
            .filter(|label| !same_name(label, &ask.label));
        for label in others {
            let other = Registration {
                wallet_id: ask.wallet_id,
                identity: ask.identity,
                check: username_check(&label),
                label,
                ours: true,
            };
            // Each answer may open a contest the next label's kind reads.
            let (names, _) = Self::local_view(session, wallet, locks, ask).await?;
            let kind = NameKind::of(&other.check, &names.open_contests);
            let found = net.lookup(&other.check).await?;
            let join_until = Self::join_until(session, net, &found);
            let unplaced = match &found.availability {
                NameAvailability::Unknown => true,
                NameAvailability::ContestOpen { .. } => {
                    !found.contenders.contains(&ask.identity)
                        && (found.contenders_truncated() || join_until.is_none())
                }
                _ => false,
            };
            if unplaced && other.check.contested && !ask.check.contested {
                return Err(PlatformError::Unavailable.into());
            }
            let now = crate::events::unix_now();
            // Refusals were recorded; they say nothing about `ask`.
            let concluded = Self::conclude(
                session, wallet, locks, &other, kind, &found, join_until, now,
            )
            .await;
            if let Err(NameError::Platform(e)) = concluded
                && e != PlatformError::Unavailable
            {
                return Err(e.into());
            }
        }
        Ok(())
    }

    /// What `found` proves about `ask`'s label, recorded: the identity owns
    /// it or contends for it (settled, `Ok(Some)`), Platform refuses it for
    /// good (the pending label and any listed copy forgotten, the refusal
    /// returned), or neither yet (`Ok(None)` to write, or a refusal that
    /// rests on incomplete evidence, such as a vote-state page too full to
    /// show every contender or an unknown deadline, with the label kept
    /// pending).
    #[expect(clippy::too_many_arguments, reason = "one registration's context")]
    async fn conclude(
        session: &NetworkSession,
        wallet: &PlatformWallet,
        locks: &IdentityLocks,
        ask: &Registration,
        kind: NameKind,
        found: &Lookup,
        join_until: Option<u64>,
        now: u64,
    ) -> Result<Option<NameOutcome>, NameError> {
        match registration_step(found, &ask.identity, join_until, now) {
            Ok(Some(outcome)) => Ok(Some(
                Self::settle(session, wallet, locks, ask, kind, outcome).await?,
            )),
            Ok(None) => Ok(None),
            Err(e) => {
                let for_good = match &e {
                    NameError::Taken | NameError::Locked => true,
                    // Closed well before now (DAPI nodes and clocks lag), and
                    // the identity is not among every contender.
                    NameError::ContestOpen => {
                        !found.contenders_truncated()
                            && join_until.is_some_and(|until| now >= until + CLOSED_MARGIN)
                    }
                    _ => false,
                };
                if for_good {
                    Self::forget(session, wallet, locks, ask).await?;
                }
                Err(e)
            }
        }
    }

    /// Records a Platform answer that the identity owns `ask`'s label
    /// (`Registered`) or contends for it (`ContestStarted`), and clears it
    /// from the pending labels. `Registered`: the label is in the
    /// identity's names (a confirmation that failed after the domain
    /// landed leaves it out of the library's list) and out of its open
    /// contests. If the write was the engine's ([`Registration::ours`]), a
    /// non-contested one asked for while the identity contends for another
    /// is its temporary name and a contested one its won contest.
    /// `ContestStarted`: it is in the open contests ([`record_contest`]) and
    /// is the contested name.
    /// The user's pick is never written.
    async fn settle(
        session: &NetworkSession,
        wallet: &PlatformWallet,
        locks: &IdentityLocks,
        ask: &Registration,
        kind: NameKind,
        outcome: NameOutcome,
    ) -> Result<NameOutcome, NameError> {
        let label = ask.label.as_str();
        let _state = locks.state.lock().await;
        Self::edit_names(wallet, ask, |managed, persister| match outcome {
            NameOutcome::Registered => {
                if !managed
                    .dpns_names
                    .iter()
                    .any(|n| same_name(&n.label, label))
                {
                    let mut names = managed.dpns_names.clone();
                    // No time: a later fetch or marketplace row gives it
                    // one; until then it sorts after timed names.
                    names.push(DpnsNameInfo {
                        label: label.to_string(),
                        acquired_at: None,
                    });
                    managed.set_dpns_names(names, persister);
                }
                Self::drop_contest(managed, label, persister);
            }
            NameOutcome::ContestStarted { .. } => record_contest(managed, label, persister),
        })
        .await?;
        let key = match (kind, &outcome) {
            (NameKind::Contested, NameOutcome::ContestStarted { .. }) => Some(PREF_CONTESTED_NAME),
            (NameKind::Contested, NameOutcome::Registered) if ask.ours => Some(PREF_CONTESTED_NAME),
            (NameKind::Temporary, NameOutcome::Registered) if ask.ours => Some(PREF_TEMPORARY_NAME),
            _ => None,
        };
        if let Some(key) = key {
            Self::set_name_pref(session, ask, key, Some(label.to_string())).await?;
        }
        Self::update_pending(session, ask, false).await?;
        Ok(outcome)
    }

    /// Records a refusal for good of `ask`'s label: the identity neither
    /// owns it nor contends for it, so it leaves the open contests and, if
    /// it was pending, the pending labels and the copy the library listed
    /// after its write.
    async fn forget(
        session: &NetworkSession,
        wallet: &PlatformWallet,
        locks: &IdentityLocks,
        ask: &Registration,
    ) -> Result<(), NameError> {
        let label = ask.label.as_str();
        let _state = locks.state.lock().await;
        let pending = Self::main_name_prefs(session, ask.wallet_id, &ask.identity)
            .await?
            .is_pending(label);
        Self::edit_names(wallet, ask, |managed, persister| {
            if pending
                && managed
                    .dpns_names
                    .iter()
                    .any(|n| same_name(&n.label, label))
            {
                let names = managed
                    .dpns_names
                    .iter()
                    .filter(|n| !same_name(&n.label, label))
                    .cloned()
                    .collect();
                managed.set_dpns_names(names, persister);
            }
            Self::drop_contest(managed, label, persister);
        })
        .await?;
        if pending {
            Self::update_pending(session, ask, false).await?;
        }
        Ok(())
    }

    /// Runs `edit` on `ask`'s identity under the wallet's write lock.
    async fn edit_names(
        wallet: &PlatformWallet,
        ask: &Registration,
        edit: impl FnOnce(&mut ManagedIdentity, &WalletPersister),
    ) -> Result<(), NameError> {
        let mut state = wallet.state_mut().await;
        let managed = state
            .identity_manager
            .managed_identity_mut(&ask.identity)
            .ok_or(PlatformError::Identity(IdentityError::NotFound))?;
        edit(managed, wallet.persister());
        Ok(())
    }

    fn drop_contest(managed: &mut ManagedIdentity, label: &str, persister: &WalletPersister) {
        if managed
            .contested_dpns_names
            .iter()
            .any(|c| same_name(c, label))
        {
            let open = managed
                .contested_dpns_names
                .iter()
                .filter(|c| !same_name(c, label))
                .cloned()
                .collect();
            managed.set_contested_dpns_names(open, persister);
        }
    }

    /// The identity's names and prefs, read together under the state lock.
    async fn local_view(
        session: &NetworkSession,
        wallet: &PlatformWallet,
        locks: &IdentityLocks,
        ask: &Registration,
    ) -> Result<(OwnNames, MainNamePrefs), NameError> {
        let _state = locks.state.lock().await;
        Self::view(session, wallet, ask.wallet_id, ask.identity).await
    }

    /// The identity's prefs and names; the caller holds the state lock.
    async fn view(
        session: &NetworkSession,
        wallet: &PlatformWallet,
        wallet_id: WalletId,
        identity: Identifier,
    ) -> Result<(OwnNames, MainNamePrefs), NameError> {
        let prefs = Self::main_name_prefs(session, wallet_id, &identity).await?;
        let names = own_names(wallet, &identity, &prefs.pending).await?;
        Ok((names, prefs))
    }

    /// Adds `ask`'s label to the pending labels, or removes it; the caller
    /// holds the state lock.
    async fn update_pending(
        session: &NetworkSession,
        ask: &Registration,
        pending: bool,
    ) -> Result<(), NameError> {
        let mut prefs = Self::main_name_prefs(session, ask.wallet_id, &ask.identity).await?;
        prefs.pending.retain(|l| !same_name(l, &ask.label));
        if pending {
            prefs.pending.push(ask.label.clone());
        }
        Self::set_name_pref(session, ask, PREF_PENDING_NAME, prefs.pending_value()).await
    }

    async fn set_name_pref(
        session: &NetworkSession,
        ask: &Registration,
        key: &'static str,
        value: Option<String>,
    ) -> Result<(), NameError> {
        Self::set_main_name_pref(session, ask.wallet_id, &ask.identity, key, value).await
    }

    /// The grant a registration of `label` needs (`GrantAction::RegisterName`):
    /// no duffs, and credits for [`name_cost`] at the contest's size now. A
    /// contest that grows before the write costs more; `register_name` then
    /// refuses with `platform.grant_exceeded` and the grant survives.
    pub(crate) async fn register_name_grant(
        &self,
        label: String,
    ) -> Result<GrantRequest, NameError> {
        let check = valid_label(&label)?;
        self.on_wallet(move |_, manager, _| async move {
            Self::name_grant(&SdkNet(manager.sdk_arc()), &check).await
        })
        .await
    }

    async fn name_grant(
        net: &impl NameNet,
        check: &UsernameCheck,
    ) -> Result<GrantRequest, NameError> {
        let contenders = if check.contested {
            net.lookup(check).await?.contenders.len()
        } else {
            0
        };
        Ok(GrantRequest {
            max_duffs: 0,
            max_credits: name_cost(check, contenders, net.version()).total,
        })
    }

    /// Picks `label` as the identity's main name, or clears the pick with
    /// `None`. The label must be one the identity owns now, by Platform's
    /// evidence (`invalid_argument` otherwise: a pending label is not
    /// owned); the pick is stored as the identity spells it. No sync
    /// rewrites it ([`resolve_main_name`]).
    pub async fn set_main_name(
        &self,
        identity: String,
        label: Option<String>,
    ) -> Result<(), NameError> {
        let identity_id = parse_identity(&identity)?;
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, _, wallet| async move {
            let locks = identity_locks(wallet_id, identity_id);
            let _state = locks.state.lock().await;
            let (names, _) = Self::view(&session, &wallet, wallet_id, identity_id).await?;
            let pick = label
                .map(|label| {
                    owned_spelling(&names.owned, &names.open_contests, &label).ok_or_else(|| {
                        PlatformError::InvalidArgument {
                            detail: "not a name this identity owns".into(),
                        }
                    })
                })
                .transpose()?;
            // The only write of the pick: DP1-05's, which also updates the
            // cache `identities()` reads (review DP1-03 R5).
            Self::set_main_name_pref(&session, wallet_id, &identity_id, MAIN_NAME_PREF, pick).await
        })
        .await
    }

    /// The identity's main name ([`resolve_main_name`]) over the names
    /// Platform evidence shows it owns ([`own_names`]); `None` while it owns
    /// none. A read records nothing.
    pub async fn main_name(&self, identity: String) -> Result<Option<String>, NameError> {
        let identity_id = parse_identity(&identity)?;
        let wallet_id = self.wallet_id;
        self.on_wallet(move |session, _, wallet| async move {
            let locks = identity_locks(wallet_id, identity_id);
            let _state = locks.state.lock().await;
            let (names, prefs) = Self::view(&session, &wallet, wallet_id, identity_id).await?;
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

#[path = "names_net.rs"]
mod net;
use net::{NameNet, SdkNet};

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
