//! Usernames: rules, availability, registration, the own contest and user
//! search (DP1-03, DP1-04, DP2-03).

use serde::{Deserialize, Serialize};

use super::contacts::Relation;
use super::dashpay::{DashPay, stub};
use super::errors::NameError;

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

/// Validates a username label offline (F4). Pure; needs no session.
#[expect(unused_variables, reason = "stub until DP1-03")]
pub fn check_username(label: &str) -> Result<UsernameCheck, NameError> {
    stub("check_username")
}

#[expect(unused_variables, reason = "stubs until DP1-03 and DP2-03")]
impl DashPay {
    pub async fn name_availability(&self, label: String) -> Result<NameAvailability, NameError> {
        stub("DashPay.name_availability")
    }

    pub async fn register_name(
        &self,
        identity: String,
        label: String,
        grant: String,
    ) -> Result<NameOutcome, NameError> {
        stub("DashPay.register_name")
    }

    pub async fn contest_status(
        &self,
        identity: String,
        label: String,
    ) -> Result<ContestStatus, NameError> {
        stub("DashPay.contest_status")
    }

    pub async fn search_users(
        &self,
        prefix: String,
        limit: u32,
    ) -> Result<Vec<UserHit>, NameError> {
        stub("DashPay.search_users")
    }

    pub async fn resolve_user(&self, username: String) -> Result<Option<UserHit>, NameError> {
        stub("DashPay.resolve_user")
    }
}
