//! The records of an evonode's Platform credit withdrawal (MG-13 P13,
//! mn-engine §2.5). Same conventions as `records.rs`: display-order hex
//! ProTx hashes, credits and duffs as `u64`, Unix seconds. Identities and
//! document ids are Base58, addresses the network's. No record holds a
//! secret or a signed transition's bytes.

use serde::{Deserialize, Serialize};

use crate::platform::WithdrawalStatus;

/// Which kind of key signs (MG-00 D23, MG-13 P2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalKeyChoice {
    /// The owner key: no destination may be given, and Drive pays the
    /// address its window over the identity's TRANSFER keys picks at
    /// execution, the registered payout address.
    Owner,
    /// A payout key: the approved destination, by default the key's own
    /// address.
    Payout,
}

/// Where a held withdrawal key comes from (P2). Public facts only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WithdrawalKeySource {
    /// The wallet's keychain owner key `m/9'/coin'/3'/2'/index`.
    KeychainOwner { index: u32 },
    /// The `owner` key attached to the tracked masternode.
    AttachedOwner,
    /// The wallet's key of a payout address, at `path`.
    WalletPayout { path: String },
    /// The `payout` key attached to the tracked masternode.
    AttachedPayout,
}

/// A key of the wallet that matches one of the identity's enabled keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldWithdrawalKey {
    pub kind: WithdrawalKeyChoice,
    pub source: WithdrawalKeySource,
    /// The identity key it signs as.
    pub identity_key_id: u32,
}

/// Why a destination reads `checking`, with Withdraw off (P4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalCheckingReason {
    /// (a): Platform's proved core height is below SPV's synced header tip.
    /// The quote's wait sends §6's event once it catches up (P4).
    PlatformBehind,
    /// (b): a payout-change barrier of the masternode is pending (P16).
    PayoutChangePending,
    /// (c): the identity read failed rule 5 (P12).
    Unverified,
    /// SPV has no synced header tip to compare with.
    SpvUnsynced,
}

impl WithdrawalCheckingReason {
    /// Its JSON name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PlatformBehind => "platform_behind",
            Self::PayoutChangePending => "payout_change_pending",
            Self::Unverified => "unverified",
            Self::SpvUnsynced => "spv_unsynced",
        }
    }
}

/// Where a withdrawal by one key pays (P4, MG-00 D42).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WithdrawalDestinationCheck {
    /// Withdraw on: (a)–(c) hold.
    Fixed {
        address: String,
        /// The identity proof's Platform height.
        platform_height: u64,
        /// The identity proof's core chain-locked height.
        core_height: u32,
        /// SPV's synced header tip.
        header_tip: u32,
        /// `header_tip − core_height`, 0 when Platform is ahead.
        lag: u32,
    },
    /// Withdraw off.
    Checking { reason: WithdrawalCheckingReason },
}

/// One of the identity's keys, as Platform proved it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalIdentityKey {
    pub id: u32,
    /// dpp's purpose in lower case (`owner`, `transfer`, `voting`, …).
    pub purpose: String,
    pub disabled: bool,
}

/// Platform's side of the masternode, for the detail pane (P13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalPlatformStatus {
    pub identity_found: bool,
    pub keys: Vec<WithdrawalIdentityKey>,
    /// Credits; `None` when Platform has no identity.
    pub balance: Option<u64>,
    pub platform_height: u64,
    pub core_height: u32,
    /// `None` while SPV is unsynced.
    pub header_tip: Option<u32>,
    pub lag: Option<u32>,
}

/// Drive's withdrawal limits at the network's protocol version (P11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalRules {
    pub min_credits: u64,
    pub max_credits: u64,
    /// The fee reserve the balance must hold besides the amount; the grant
    /// covers it.
    pub fee_credits: u64,
}

/// `withdrawal_quote`: what the wallet can withdraw from the masternode's
/// identity, and where to (P13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalQuote {
    pub pro_tx_hash: String,
    pub owner_key: Option<HeldWithdrawalKey>,
    pub payout_keys: Vec<HeldWithdrawalKey>,
    /// The balance less the fee reserve, in whole duffs as credits, at most
    /// `rules.max_credits`; 0 below `rules.min_credits` (P11).
    pub claimable_credits: u64,
    /// What `claimable_credits` pays out on Core.
    pub expected_duffs: Option<u64>,
    pub rules: WithdrawalRules,
    /// The owner key's destination; `None` without an owner key or when
    /// Drive's window pays nowhere.
    pub owner_destination: Option<WithdrawalDestinationCheck>,
    /// The first payout key's own address; `None` without a payout key.
    pub payout_destination: Option<WithdrawalDestinationCheck>,
    pub status: WithdrawalPlatformStatus,
    /// The identity's open withdrawal: Withdraw is blocked while it is.
    pub open: Option<CreditWithdrawal>,
}

/// A record's state (P6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditWithdrawalState {
    /// Stored before any network step; no hand-off started.
    Signed,
    /// Handed to the network, or it may have been.
    Sent,
    /// A CheckTx refusal on the merits (`state_reason`). Not definitive:
    /// it may still execute.
    Refused,
    /// Try again signed a sibling at its nonce; still read.
    Superseded,
    /// The reads stayed undecidable; `mark_withdrawal` applies.
    CouldntConfirm,
    MarkedReceived,
    MarkedNotReceived,
    /// Its proved withdrawal document matches (`proof`).
    Done,
    /// Another record at its nonce is `done`.
    SiblingPaid,
    /// P8's proof that nothing at its nonce paid (`not_executed`).
    NotExecuted,
}

/// The user's mark of a `couldnt_confirm` record (P9, MG-00 D47): kept
/// apart from any proof, never `done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalMark {
    Received,
    NotReceived,
}

// Crate-level conversions, not `From`: a trait impl would be §3 surface.
impl WithdrawalMark {
    /// As the app database stores it.
    pub(crate) fn stored(self) -> dw_appdb::WithdrawalMark {
        match self {
            Self::Received => dw_appdb::WithdrawalMark::Received,
            Self::NotReceived => dw_appdb::WithdrawalMark::NotReceived,
        }
    }

    /// From the app database's.
    pub(crate) fn of_stored(mark: dw_appdb::WithdrawalMark) -> Self {
        match mark {
            dw_appdb::WithdrawalMark::Received => Self::Received,
            dw_appdb::WithdrawalMark::NotReceived => Self::NotReceived,
        }
    }
}

/// The proved document behind `done` (P7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalProofFacts {
    pub document_id: String,
    pub platform_height: u64,
    /// Unix milliseconds; `None` when the proof carried none.
    pub block_time: Option<u64>,
}

/// The heights of P8's four conditions behind `not_executed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotExecutedFacts {
    pub nonce_height: u64,
    pub identity_height: u64,
    pub document_height: u64,
}

/// A masternode withdrawal's record (P5): app-wide, kept by identity,
/// never deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditWithdrawal {
    /// 32 lower-case hex.
    pub id: String,
    pub pro_tx_hash: String,
    pub identity: String,
    /// The wallet that signed it: a label (P5).
    pub created_by: String,
    /// 64 lower-case hex.
    pub transition_hash: String,
    pub key: WithdrawalKeyChoice,
    pub credits: u64,
    pub expected_duffs: u64,
    pub core_fee_per_byte: u32,
    pub nonce: u64,
    /// A payout-key record's signed destination.
    pub destination: Option<String>,
    /// An owner-key record's destination as its signing saw Drive's window.
    pub expected_destination: Option<String>,
    pub state: CreditWithdrawalState,
    /// A refusal's reason, or why a read was undecidable.
    pub state_reason: Option<String>,
    pub signed_at: u64,
    pub sent_at: Option<u64>,
    /// The address the matching document pays (`done`).
    pub paid_to: Option<String>,
    /// The matching document's status, kept current.
    pub document_status: Option<WithdrawalStatus>,
    pub proof: Option<WithdrawalProofFacts>,
    pub not_executed: Option<NotExecutedFacts>,
    pub last_checked: Option<u64>,
    pub mark: Option<WithdrawalMark>,
    pub marked_at: Option<u64>,
    /// A late proof's notice: the later record it points at (P14).
    pub late_proof_link: Option<String>,
}
