//! Usernames: rules, availability, registration, the own contest and user search.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::platform::{UserRelation, UsernameRuleName};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsernameRuleCheck {
    pub rule: UsernameRuleName,
    pub passed: bool,
}

/// The inline rule checklist of a username label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UsernameCheck {
    pub valid: bool,
    /// The label under DPNS's homograph folding.
    pub normalized: String,
    /// Set only for a valid label.
    pub contested: bool,
    pub rules: Vec<UsernameRuleCheck>,
}

impl From<engine::UsernameCheck> for UsernameCheck {
    fn from(c: engine::UsernameCheck) -> Self {
        let engine::UsernameCheck {
            valid,
            normalized,
            contested,
            rules,
        } = c;
        UsernameCheck {
            valid,
            normalized,
            contested,
            rules: rules
                .into_iter()
                .map(|r| {
                    let engine::UsernameRuleCheck { rule, passed } = r;
                    UsernameRuleCheck {
                        rule: rule.into(),
                        passed,
                    }
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NameAvailability {
    /// `rules` are the rules the label breaks.
    Invalid { rules: Vec<UsernameRuleName> },
    Available { contested: bool },
    Taken { owner: Option<String> },
    ContestOpen { ends_at: Option<u64>, contenders: u32 },
    Locked,
    Unknown,
}

impl From<engine::NameAvailability> for NameAvailability {
    fn from(a: engine::NameAvailability) -> Self {
        use engine::NameAvailability as E;
        match a {
            E::Invalid { rules } => NameAvailability::Invalid {
                rules: rules.into_iter().map(Into::into).collect(),
            },
            E::Available { contested } => NameAvailability::Available { contested },
            E::Taken { owner } => NameAvailability::Taken { owner },
            E::ContestOpen {
                ends_at,
                contenders,
            } => NameAvailability::ContestOpen {
                ends_at,
                contenders,
            },
            E::Locked => NameAvailability::Locked,
            E::Unknown => NameAvailability::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NameOutcome {
    Registered,
    ContestStarted { ends_at: Option<u64> },
}

impl From<engine::NameOutcome> for NameOutcome {
    fn from(o: engine::NameOutcome) -> Self {
        match o {
            engine::NameOutcome::Registered => NameOutcome::Registered,
            engine::NameOutcome::ContestStarted { ends_at } => {
                NameOutcome::ContestStarted { ends_at }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ContestState {
    Open,
    Won,
    Lost { winner: Option<String> },
    Locked,
}

impl From<engine::ContestState> for ContestState {
    fn from(s: engine::ContestState) -> Self {
        match s {
            engine::ContestState::Open => ContestState::Open,
            engine::ContestState::Won => ContestState::Won,
            engine::ContestState::Lost { winner } => ContestState::Lost { winner },
            engine::ContestState::Locked => ContestState::Locked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContestContender {
    pub identity: String,
    pub votes: Option<u32>,
    pub is_self: bool,
}

impl From<engine::ContestContender> for ContestContender {
    fn from(c: engine::ContestContender) -> Self {
        let engine::ContestContender {
            identity,
            votes,
            is_self,
        } = c;
        ContestContender {
            identity,
            votes,
            is_self,
        }
    }
}

/// The own contest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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

impl From<engine::ContestStatus> for ContestStatus {
    fn from(s: engine::ContestStatus) -> Self {
        let engine::ContestStatus {
            label,
            state,
            ends_at,
            contenders,
            lock_votes,
            abstain_votes,
            temporary_name,
        } = s;
        ContestStatus {
            label,
            state: state.into(),
            ends_at,
            contenders: contenders.into_iter().map(Into::into).collect(),
            lock_votes,
            abstain_votes,
            temporary_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UserHit {
    pub identity: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub relation: UserRelation,
    /// Verified only through the trusted fallback.
    pub unverified: bool,
}

impl From<engine::UserHit> for UserHit {
    fn from(h: engine::UserHit) -> Self {
        let engine::UserHit {
            identity,
            username,
            display_name,
            avatar_url,
            relation,
            unverified,
        } = h;
        UserHit {
            identity,
            username,
            display_name,
            avatar_url,
            relation: relation.into(),
            unverified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_is_tagged_in_camel_case() {
        let a = NameAvailability::from(engine::NameAvailability::ContestOpen {
            ends_at: Some(5),
            contenders: 2,
        });
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::json!({"kind": "contestOpen", "endsAt": 5, "contenders": 2})
        );
        let a = NameAvailability::from(engine::NameAvailability::Invalid {
            rules: vec![engine::UsernameRule::NoDoubleHyphen],
        });
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::json!({"kind": "invalid", "rules": ["noDoubleHyphen"]})
        );
    }

    #[test]
    fn check_username_maps_the_checklist() {
        let c = UsernameCheck::from(engine::check_username("a--").unwrap());
        assert!(!c.valid);
        assert!(
            c.rules
                .iter()
                .any(|r| r.rule == UsernameRuleName::NoDoubleHyphen && !r.passed)
        );
        assert_eq!(c.rules.len(), 5);
    }

    #[test]
    fn contest_states_carry_the_winner() {
        let s = ContestState::from(engine::ContestState::Lost {
            winner: Some("w".into()),
        });
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({"kind": "lost", "winner": "w"})
        );
    }
}
