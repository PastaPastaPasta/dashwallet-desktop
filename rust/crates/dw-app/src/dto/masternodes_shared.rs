//! The shared-masternode records (`docs/contracts/mn-engine.md` §5, MG-04b
//! §8) as the renderer sees them (MG-10 §3.3): the capability, the creation
//! requests, a session with its message, and what an import did. No record
//! holds a secret; a standby's signed bytes never leave Rust (MG-10 D6).

use dw_engine::masternodes as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::mirror_enum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ActivationName {
    Active,
    Inactive,
    Unknown,
}

mirror_enum!(engine::Activation => ActivationName { Active, Inactive, Unknown });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedCapability {
    /// `active` once the wallet saw a version-3 entry, else `unknown`.
    pub v24: ActivationName,
    /// Always `inactive`.
    pub evonode_shares: ActivationName,
    pub can_send: bool,
}

mirror_struct!(engine::SharedCapability => SharedCapability {
    v24: into, evonode_shares: into, can_send,
});

/// The operator's BLS public key: given, the keychain's, or the next unused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OperatorChoice {
    /// 96 hex characters, basic serialization.
    Given {
        public_key: String,
    },
    Keychain {
        index: u32,
    },
    NextUnused,
}

impl From<OperatorChoice> for engine::OperatorChoice {
    fn from(c: OperatorChoice) -> Self {
        match c {
            OperatorChoice::Given { public_key } => Self::Given { public_key },
            OperatorChoice::Keychain { index } => Self::Keychain { index },
            OperatorChoice::NextUnused => Self::NextUnused,
        }
    }
}

/// The voting key: the keychain's, another wallet's address, or the next
/// unused keychain key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum VotingChoice {
    Keychain { index: u32 },
    Address { address: String },
    NextUnused,
}

impl From<VotingChoice> for engine::VotingChoice {
    fn from(c: VotingChoice) -> Self {
        match c {
            VotingChoice::Keychain { index } => Self::Keychain { index },
            VotingChoice::Address { address } => Self::Address { address },
            VotingChoice::NextUnused => Self::NextUnused,
        }
    }
}

/// Where a share's rewards go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RewardChoice {
    Refund,
    NewAddress,
    Address { address: String },
}

impl From<RewardChoice> for engine::RewardChoice {
    fn from(c: RewardChoice) -> Self {
        match c {
            RewardChoice::Refund => Self::Refund,
            RewardChoice::NewAddress => Self::NewAddress,
            RewardChoice::Address { address } => Self::Address { address },
        }
    }
}

/// The coordinator's masternode-wide terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShareTerms {
    /// `a.b.c.d:port`.
    pub service: String,
    pub operator: OperatorChoice,
    /// Basis points.
    pub operator_reward: u16,
    pub voting: VotingChoice,
    pub early_period_blocks: u32,
    /// Duffs.
    pub early_penalty: u64,
}

impl From<ShareTerms> for engine::ShareTerms {
    fn from(t: ShareTerms) -> Self {
        let ShareTerms {
            service,
            operator,
            operator_reward,
            voting,
            early_period_blocks,
            early_penalty,
        } = t;
        Self {
            service,
            operator: operator.into(),
            operator_reward,
            voting: voting.into(),
            early_period_blocks,
            early_penalty,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShareSlot {
    pub label: String,
    /// Duffs.
    pub amount: u64,
}

/// A coordinator's new session: 2–8 shares, row 0 its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StartSharedRequest {
    pub shares: Vec<ShareSlot>,
    pub terms: ShareTerms,
    /// The registration's network fee, paid by the coordinator.
    pub fee: u64,
    pub reward: RewardChoice,
    /// Who runs the server, free text.
    pub node_run_by: Option<String>,
}

impl From<StartSharedRequest> for engine::StartSharedRequest {
    fn from(r: StartSharedRequest) -> Self {
        let StartSharedRequest {
            shares,
            terms,
            fee,
            reward,
            node_run_by,
        } = r;
        Self {
            shares: shares
                .into_iter()
                .map(|s| engine::ShareSlot {
                    label: s.label,
                    amount: s.amount,
                })
                .collect(),
            terms: terms.into(),
            fee,
            reward: reward.into(),
            node_run_by,
        }
    }
}

/// A participant's answer to an Invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JoinSharedRequest {
    /// `null` lets the wallet take the single empty row.
    pub share_index: Option<u16>,
    pub reward: RewardChoice,
}

impl From<JoinSharedRequest> for engine::JoinSharedRequest {
    fn from(r: JoinSharedRequest) -> Self {
        Self {
            share_index: r.share_index,
            reward: r.reward.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SharedRoleName {
    Coordinator,
    Participant,
}

mirror_enum!(engine::SharedRole => SharedRoleName { Coordinator, Participant });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SharedKindName {
    Registration,
    DissolveTogether,
    KeyChange,
}

mirror_enum!(engine::SharedKind => SharedKindName { Registration, DissolveTogether, KeyChange });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeadReasonName {
    InputsSpent,
    Delisted,
    Abandoned,
}

mirror_enum!(engine::DeadReason => DeadReasonName { InputsSpent, Delisted, Abandoned });

/// The wallet's view of a session, derived on every read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedStageName {
    Invited,
    Draft,
    Signing,
    Combined,
    FundingSigned,
    Sent,
    Confirmed { height: u32 },
    Conflicted,
    Stale,
    Dead { reason: DeadReasonName },
}

impl From<engine::SharedStage> for SharedStageName {
    fn from(s: engine::SharedStage) -> Self {
        use engine::SharedStage as S;
        match s {
            S::Invited => Self::Invited,
            S::Draft => Self::Draft,
            S::Signing => Self::Signing,
            S::Combined => Self::Combined,
            S::FundingSigned => Self::FundingSigned,
            S::Sent => Self::Sent,
            S::Confirmed { height } => Self::Confirmed { height },
            S::Conflicted => Self::Conflicted,
            S::Stale => Self::Stale,
            S::Dead { reason } => Self::Dead {
                reason: reason.into(),
            },
        }
    }
}

/// What this wallet does next with a session: the render key (MG-10 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SharedNextName {
    Join,
    Copy,
    Wait,
    Lock,
    Approve,
    Sign,
    Send,
    CreateStandby,
    Abandon,
    Nothing,
}

mirror_enum!(engine::SharedNext => SharedNextName {
    Join, Copy, Wait, Lock, Approve, Sign, Send, CreateStandby, Abandon, Nothing,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShareRow {
    pub index: u16,
    pub label: String,
    /// Duffs.
    pub amount: u64,
    pub owner_address: Option<String>,
    pub refund_address: Option<String>,
    /// `null`: rewards go to the refund address.
    pub reward_address: Option<String>,
    pub mine: bool,
    pub contributed: bool,
    pub approved: bool,
    pub funding_signed: bool,
}

mirror_struct!(engine::ShareRow => ShareRow {
    index, label, amount, owner_address, refund_address, reward_address, mine, contributed,
    approved, funding_signed,
});

/// A session's terms as the envelope carries them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShareTermsView {
    pub service: String,
    /// How Core reads `service` when it differs from what is written.
    pub service_read: Option<String>,
    pub operator_public_key: String,
    pub voting_address: String,
    /// Basis points, as received.
    pub operator_reward: i32,
    pub early_period_blocks: u32,
    /// Duffs, as received.
    pub early_penalty: i64,
}

mirror_struct!(engine::ShareTermsView => ShareTermsView {
    service, service_read, operator_public_key, voting_address, operator_reward,
    early_period_blocks, early_penalty,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyBase {
    pub operator_public_key: String,
    pub voting_address: String,
}

mirror_struct!(engine::KeyBase => KeyBase { operator_public_key, voting_address });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyChangeView {
    pub requested: KeyBase,
    /// `null` while the masternode isn't in the synced list.
    pub current: Option<KeyBase>,
    pub funder: bool,
}

mirror_struct!(engine::KeyChangeView => KeyChangeView { requested: into, current: opt, funder });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SharedWarning {
    ZeroPenalty,
    EditedMessage,
    UnvaluedInputs { shares: Vec<u16>, count: u32 },
    EmptyService,
    ActorOutputDust,
    PenaltyPaidAfterPeriod,
    SkippedSignatures { shares: Vec<u16>, count: u32 },
    NoApprovals,
    OperatorReset,
    UsableByFunder,
    AbandonHint,
}

impl From<engine::SharedWarning> for SharedWarning {
    fn from(w: engine::SharedWarning) -> Self {
        use engine::SharedWarning as W;
        match w {
            W::ZeroPenalty => Self::ZeroPenalty,
            W::EditedMessage => Self::EditedMessage,
            W::UnvaluedInputs { shares, count } => Self::UnvaluedInputs { shares, count },
            W::EmptyService => Self::EmptyService,
            W::ActorOutputDust => Self::ActorOutputDust,
            W::PenaltyPaidAfterPeriod => Self::PenaltyPaidAfterPeriod,
            W::SkippedSignatures { shares, count } => Self::SkippedSignatures { shares, count },
            W::NoApprovals => Self::NoApprovals,
            W::OperatorReset => Self::OperatorReset,
            W::UsableByFunder => Self::UsableByFunder,
            W::AbandonHint => Self::AbandonHint,
        }
    }
}

/// A shared-masternode session of the wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedSession {
    /// 64 lower-case hex.
    pub id: String,
    /// The session code: the id's first six characters, upper-case.
    pub code: String,
    /// The current envelope's check code, `XXXX-XXXX`.
    pub message_code: String,
    pub kind: SharedKindName,
    pub role: SharedRoleName,
    pub stage: SharedStageName,
    /// What `mn_shared_lock` takes back.
    pub revision: u32,
    /// What `mn_shared_approve` takes back; `null` in a draft.
    pub consent_hash: Option<String>,
    pub rows: Vec<ShareRow>,
    pub terms: ShareTermsView,
    pub node_run_by: String,
    /// The coordinator's registration fee, duffs.
    pub fee: Option<u64>,
    /// What this wallet's funding debits it, once its contribution is in.
    pub debit: Option<u64>,
    pub pro_tx_hash: Option<String>,
    pub txid: Option<String>,
    pub reserved: Vec<String>,
    pub next: SharedNextName,
    /// `mn_shared_discard` would delete it now and release its coins.
    pub discardable: bool,
    /// The session may be out, not settled, and holds a reserved coin. The
    /// renderer offers Abandon only while `abandon_charge` is set too.
    pub abandonable: bool,
    pub warnings: Vec<SharedWarning>,
    /// The terms this wallet last approved: the consent diff's base.
    pub approved_terms: Option<ShareTermsView>,
    pub base_keys: Option<KeyBase>,
    pub key_change: Option<KeyChangeView>,
    pub actor_fee: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: Option<String>,
    /// The engine's recovery read lists Abandon: `mn_shared_abandon` spends
    /// every reserved coin back to the wallet now, charged exactly this, its
    /// grant cap. `null` while it isn't listed.
    pub abandon_charge: Option<u64>,
    /// Advisory: out, the list current with the tip, and three hours of tip
    /// time since it went out (MG-10 §4.2).
    pub overdue: bool,
}

impl From<engine::SharedSession> for SharedSession {
    fn from(x: engine::SharedSession) -> Self {
        let engine::SharedSession {
            id,
            code,
            message_code,
            kind,
            role,
            stage,
            revision,
            consent_hash,
            rows,
            terms,
            node_run_by,
            fee,
            debit,
            pro_tx_hash,
            txid,
            reserved,
            next,
            discardable,
            abandonable,
            warnings,
            approved_terms,
            base_keys,
            key_change,
            actor_fee,
            created_at,
            updated_at,
            last_error,
            recovery,
        } = x;
        let abandon_charge = recovery.actions.iter().find_map(|a| match a {
            engine::RecoveryAction::Abandon { charge, .. } => Some(*charge),
            _ => None,
        });
        SharedSession {
            id,
            code,
            message_code,
            kind: kind.into(),
            role: role.into(),
            stage: stage.into(),
            revision,
            consent_hash,
            rows: rows.into_iter().map(Into::into).collect(),
            terms: terms.into(),
            node_run_by,
            fee,
            debit,
            pro_tx_hash,
            txid,
            reserved,
            next: next.into(),
            discardable,
            abandonable,
            warnings: warnings.into_iter().map(Into::into).collect(),
            approved_terms: approved_terms.map(Into::into),
            base_keys: base_keys.map(Into::into),
            key_change: key_change.map(Into::into),
            actor_fee,
            created_at,
            updated_at,
            last_error,
            abandon_charge,
            overdue: recovery.overdue,
        }
    }
}

/// A session whose stored record this wallet can no longer read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UnreadableSession {
    /// 64 lower-case hex.
    pub id: String,
    /// The id's first six characters, upper-case.
    pub code: String,
    pub kind: SharedKindName,
    pub role: SharedRoleName,
    /// `mn_shared_discard` would delete it now: nothing of it may be out.
    pub discardable: bool,
    /// `mn_shared_abandon_unreadable` would spend every reserved coin back
    /// to the wallet now, charged `abandon_charge`.
    pub abandonable: bool,
    /// What that spend charges, exactly its grant cap; `null` while it
    /// isn't `abandonable`.
    pub abandon_charge: Option<u64>,
    pub reserved: Vec<String>,
    pub updated_at: u64,
}

mirror_struct!(engine::UnreadableSession => UnreadableSession {
    id, code, kind: into, role: into, discardable, abandonable, abandon_charge, reserved, updated_at,
});

/// The message to copy or save now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedMessage {
    pub text: String,
    /// `XXXX-XXXX`.
    pub code: String,
    pub name: String,
    /// Core's file name, `shared-mn-session-<CODE>.json`.
    pub file_name: String,
}

mirror_struct!(engine::SharedMessage => SharedMessage { text, code, name, file_name });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SharedOutcome {
    pub session: SharedSession,
    pub message: SharedMessage,
}

mirror_struct!(engine::SharedOutcome => SharedOutcome { session: into, message: into });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StandbyVariantName {
    Full,
    Immediate,
}

mirror_both!(engine::StandbyVariant => StandbyVariantName { Full, Immediate });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StandbySourceName {
    Created,
    Imported,
}

mirror_enum!(engine::StandbySource => StandbySourceName { Created, Imported });

/// A stored standby. Its signed bytes stay in the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Standby {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub fee: u64,
    pub penalty: Option<u64>,
    pub full_valid_from: u32,
    pub full: bool,
    pub immediate: bool,
    pub source: StandbySourceName,
    pub created_at: u64,
}

mirror_struct!(engine::Standby => Standby {
    pro_tx_hash, share_index, fee, penalty, full_valid_from, full, immediate, source: into,
    created_at,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShareReturn {
    pub share_index: u16,
    pub amount: u64,
    pub refund_address: Option<String>,
    pub mine: bool,
}

mirror_struct!(engine::ShareReturn => ShareReturn { share_index, amount, refund_address, mine });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StandbyDescription {
    pub standby: Standby,
    pub variant: StandbyVariantName,
    pub txid: String,
    pub penalty: u64,
    pub returns: Vec<ShareReturn>,
    pub returns_principal: bool,
}

mirror_struct!(engine::StandbyDescription => StandbyDescription {
    standby: into, variant: into, txid, penalty, returns: vec, returns_principal,
});

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SharedImport {
    Session {
        session: Box<SharedSession>,
        changed: bool,
        /// The pasted message's own check code, for the sender to read out.
        received_code: String,
        warnings: Vec<SharedWarning>,
    },
    Standby {
        description: StandbyDescription,
        stored: bool,
    },
}

impl From<engine::SharedImport> for SharedImport {
    fn from(i: engine::SharedImport) -> Self {
        use engine::SharedImport as I;
        match i {
            I::Session {
                session,
                changed,
                received_code,
                warnings,
            } => Self::Session {
                session: Box::new((*session).into()),
                changed,
                received_code,
                warnings: warnings.into_iter().map(Into::into).collect(),
            },
            I::Standby {
                description,
                stored,
            } => Self::Standby {
                description: description.into(),
                stored,
            },
        }
    }
}
