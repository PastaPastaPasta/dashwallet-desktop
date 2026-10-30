//! The records of the shared-masternode calls (MG-04b §8, mn-engine §5),
//! slice 1a's subset: the capability, the creation requests, the session
//! and its message. Same conventions as `records.rs`: display-order hex
//! hashes, `"<txid>-<n>"` outpoints, duffs, Unix seconds. No record holds a
//! secret: a session carries addresses, outpoints, the operator's public
//! key and signatures only, as dash-qt's session file does.

use serde::{Deserialize, Serialize};

use super::protx_records::{OperatorChoice, VotingChoice};

/// Whether a deployment is active, as the wallet can tell (MG-04b D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Active,
    Inactive,
    Unknown,
}

/// What this session can do with shared masternodes (D1, D3, D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedCapability {
    /// `active` once a version-3 list entry or provider transaction was seen
    /// (MG-03's `any_shared_seen`), else `unknown`; never `inactive`.
    pub v24: Activation,
    /// Always `inactive` (D4).
    pub evonode_shares: Activation,
    /// Whether a call may produce bytes the network accepts: `false` until
    /// the pin decodes version-3 ProTxs (BL-187, D1).
    pub can_send: bool,
}

/// The coordinator's masternode-wide terms (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareTerms {
    /// `a.b.c.d:port`, one or more joined by `,` (D5).
    pub service: String,
    /// The operator's BLS public key, given or the keychain's (MG-04 D14).
    pub operator: OperatorChoice,
    /// Basis points.
    pub operator_reward: u16,
    /// The keychain voting key or a given address (D10).
    pub voting: VotingChoice,
    pub early_period_blocks: u32,
    /// Duffs.
    pub early_penalty: u64,
}

/// One share of a new session: its label and amount (duffs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSlot {
    pub label: String,
    pub amount: u64,
}

/// A coordinator's new session (§3.1 step 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSharedRequest {
    /// 2–8 shares; row 0 is the coordinator's.
    pub shares: Vec<ShareSlot>,
    pub terms: ShareTerms,
    /// The registration's network fee, paid by the coordinator (D9).
    pub fee: u64,
    /// The coordinator's own reward.
    pub reward: RewardChoice,
    /// dash-qt's `operatorSecretHolder`: free text naming who runs the node.
    pub node_run_by: Option<String>,
}

/// Where a share's rewards go (D10, D31).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RewardChoice {
    /// To the share's refund address.
    Refund,
    /// A fresh receive address of this wallet.
    NewAddress,
    /// A P2PKH or P2SH address, this wallet's or another's.
    Address { address: String },
}

/// A participant's answer to an Invitation (§3.2 step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinSharedRequest {
    /// The share to take; `None` lets the wallet infer it (the single empty
    /// row, as dash-qt's `inferMyShare`).
    pub share_index: Option<u16>,
    pub reward: RewardChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedRole {
    Coordinator,
    Participant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedKind {
    Registration,
    DissolveTogether,
    KeyChange,
}

/// The wallet's view of a session (§3), derived on every read (§9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedStage {
    /// A participant's imported Invitation, not joined.
    Invited,
    Draft,
    Signing,
    Combined,
    FundingSigned,
    Sent,
    Confirmed {
        height: u32,
    },
    Conflicted,
    /// A key change whose base moved (D25).
    Stale,
    Dead {
        reason: DeadReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadReason {
    /// One of this wallet's reserved inputs was spent elsewhere before
    /// `out_at`.
    InputsSpent,
    /// The masternode left the list (maintenance).
    Delisted,
}

/// What this wallet does next with a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedNext {
    /// Join the imported Invitation (`join_shared`).
    Join,
    /// Send the current message (`shared_message`) to the other parties.
    Copy,
    /// Wait for another party's message.
    Wait,
    /// Lock the terms (`lock_shared_terms`): every row and contribution is
    /// in.
    Lock,
    /// Approve the locked terms (`approve_shared`).
    Approve,
    /// Sign this wallet's funding inputs (`sign_shared_funding`).
    Sign,
    /// Sign the coordinator's inputs and send (slice 1b).
    Send,
    /// A confirmed registration with a share of this wallet that has no
    /// standby: prepare one (`create_standby`, D21).
    CreateStandby,
    Nothing,
}

/// One share of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRow {
    pub index: u16,
    pub label: String,
    /// Duffs.
    pub amount: u64,
    pub owner_address: Option<String>,
    pub refund_address: Option<String>,
    /// `None`: rewards go to the refund address.
    pub reward_address: Option<String>,
    /// This wallet holds the share's owner key and took it in this session.
    pub mine: bool,
    /// The share's contribution is in the session.
    pub contributed: bool,
    /// Its owner's consent signature is in the session.
    pub approved: bool,
    /// Every input of its contribution is signed.
    pub funding_signed: bool,
}

/// A session's masternode-wide terms as the envelope carries them
/// (compared exactly, as dash-qt's `sameTerms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareTermsView {
    /// `coreP2PAddrs`.
    pub service: String,
    /// What `service` registers, when Core reads an entry of it other than
    /// as written (an octal or short IPv4 form, an IPv4-mapped IPv6 address,
    /// a `+` port): each entry's reading, joined by ", ". `None` when every
    /// entry reads as written or one can't be read (MG-04b D78). Missing
    /// from an `approved_terms` row written before it, read as `None`.
    pub service_read: Option<String>,
    /// 96 hex, basic serialization.
    pub operator_public_key: String,
    pub voting_address: String,
    /// Basis points, as received (an out-of-range value fails Core's rules).
    pub operator_reward: i32,
    pub early_period_blocks: u32,
    /// Duffs, as received.
    pub early_penalty: i64,
}

/// A key change's base (D25): the list's keys when it was prepared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBase {
    pub operator_public_key: String,
    pub voting_address: String,
}

/// A shared-masternode session of the wallet (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSession {
    /// 64 lower-case hex (D28).
    pub id: String,
    /// The session code: the id's first six characters, upper-case.
    pub code: String,
    /// The current envelope's fingerprint, `XXXX-XXXX` (§2.2).
    pub message_code: String,
    pub kind: SharedKind,
    pub role: SharedRole,
    pub stage: SharedStage,
    /// The envelope's `revision`: what `lock_shared_terms` takes back from
    /// the coordinator that reviewed this table (D56).
    pub revision: u32,
    /// The frozen registration's consent digest as the envelope shows it
    /// (`consentHash`, display order), from the locked terms on: what
    /// `approve_shared` takes back from the participant that reviewed it
    /// (D56). `None` in a draft.
    pub consent_hash: Option<String>,
    pub rows: Vec<ShareRow>,
    pub terms: ShareTermsView,
    /// `operatorSecretHolder`: who runs the node, as the coordinator wrote it.
    pub node_run_by: String,
    /// The registration fee this wallet reserved: the coordinator's (D9).
    pub fee: Option<u64>,
    /// What this wallet's funding signature debits it, once its
    /// contribution is recorded: its inputs less its change, so its share,
    /// the coordinator's fee and any dust change it gave up (D9). A
    /// `sign_shared_funding` or `send_shared_registration` grant must
    /// cover it.
    pub debit: Option<u64>,
    /// Once every input is signed: the registration's txid.
    pub pro_tx_hash: Option<String>,
    /// The txid of this wallet's signed copy, once it signed its inputs.
    pub txid: Option<String>,
    /// This wallet's locked outpoints.
    pub reserved: Vec<String>,
    pub next: SharedNext,
    pub warnings: Vec<SharedWarning>,
    /// The terms this wallet last approved (M-1 case 2's diff base).
    pub approved_terms: Option<ShareTermsView>,
    pub base_keys: Option<KeyBase>,
    pub actor_fee: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: Option<String>,
}

/// The message to copy or save now (§2.3's names by stage).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMessage {
    /// The envelope, UniValue's `write(2)` rendering.
    pub text: String,
    /// Its message code, `XXXX-XXXX`.
    pub code: String,
    /// `Invitation`, `Details`, `Locked Terms`, `Approval`, `Signing
    /// Request`, `Signed Contribution` or `Final Record`.
    pub name: String,
    /// Core's file name, `shared-mn-session-<CODE>.json`.
    pub file_name: String,
}

/// A creation step's result: the session and the message it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedOutcome {
    pub session: SharedSession,
    pub message: SharedMessage,
}

/// What an import did (§3.0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedImport {
    Session {
        /// Boxed: a session is several times a standby's description.
        session: Box<SharedSession>,
        /// Whether the session changed.
        changed: bool,
        /// The received object's recomputed fingerprint: the code its
        /// sender reads out (n-2).
        received_code: String,
        warnings: Vec<SharedWarning>,
    },
    /// A pasted standby passed D32's checks (m-19).
    Standby {
        description: StandbyDescription,
        /// Whether it was stored: `false` when the wallet holds it already.
        stored: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedWarning {
    /// The early-exit penalty is 0: an owner can leave at once (Core's
    /// warning).
    ZeroPenalty,
    /// "This message was edited after it was copied."
    EditedMessage,
    /// Contributions this wallet can't value (D26): their shares and input
    /// count.
    UnvaluedInputs {
        shares: Vec<u16>,
        count: u32,
    },
    /// No service: the masternode starts PoSe-banned (D5).
    EmptyService,
    ActorOutputDust,
    PenaltyPaidAfterPeriod,
    /// Signature entries of a sigs message that weren't taken, each with
    /// dash-qt's reason (`addSignatures`, MG-04b §2.5): missing its share
    /// number or signature, out of range, not a 65-byte base64 signature,
    /// not verifying, or a second, different signature of a share. `shares`
    /// are the readable share numbers among them.
    SkippedSignatures {
        shares: Vec<u16>,
        count: u32,
    },
    /// A sigs message whose `signatures` isn't an array: "This message
    /// does not contain any approvals." (`sharedmndialogs.cpp:569-572`);
    /// the rest of it is taken.
    NoApprovals,
}

/// A ProUpShareTx to preview for one of this wallet's shares (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateShareRequest {
    pub pro_tx_hash: String,
    pub share_index: u16,
    /// `Refund` is the share's refund script, never empty (D31).
    pub reward: RewardChoice,
}

/// What `preview_update_share` would build. Issues, locks and signs
/// nothing; a `NewAddress` reward is issued when it is built, so the
/// preview leaves it empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSharePreview {
    pub pro_tx_hash: String,
    pub share_index: u16,
    /// The new reward address; `None` for a `NewAddress` reward.
    pub reward_address: Option<String>,
    /// The reward is the share's refund script (D31).
    pub reward_is_refund: bool,
    /// The transaction's fee, duffs.
    pub fee: u64,
    /// What the wallet would pay: the fee.
    pub total_debit: u64,
}

/// A unilateral dissolution, now, by one of this wallet's shares (§5,
/// §8): the share is the actor, and pays `fee` (duffs, at most Core's
/// 1,000,000) and, during the early period, the masternode's penalty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DissolveRequest {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub fee: u64,
}

/// What one share gets back from a dissolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareReturn {
    pub share_index: u16,
    /// Duffs: the principal, plus a part of the penalty for a non-actor,
    /// less the penalty and the fee for the actor (0: no output).
    pub amount: u64,
    /// The share's refund address; `None` for a script no address reads.
    pub refund_address: Option<String>,
    /// This wallet's share.
    pub mine: bool,
}

/// What `preview_dissolve` would build: Core's `BuildDissolution` output
/// for the share, with the penalty `DissolveShared` picks. Signs nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DissolvePreview {
    /// Mined at the next height, the dissolution is early, so it pays the
    /// penalty.
    pub early: bool,
    pub penalty: u64,
    pub fee: u64,
    /// The first height at which a dissolution pays no penalty: the
    /// registration's height plus the early period.
    pub valid_from_height: u32,
    /// Every share in share order.
    pub returns: Vec<ShareReturn>,
    /// `ActorOutputDust`: this share's output is below the dust threshold,
    /// valid but not relayed.
    pub warnings: Vec<SharedWarning>,
    /// What the wallet's share gives up, what the grant must cover: the
    /// penalty and the fee (§6.3).
    pub total_debit: u64,
}

/// A standby's two variants (§9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandbyVariant {
    /// Penalty 0: valid from `full_valid_from`.
    Full,
    /// The early penalty paid: valid now.
    Immediate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandbySource {
    /// Signed by this wallet (`create_standby`).
    Created,
    /// Pasted (`import_shared_message`).
    Imported,
}

/// A stored standby, by masternode, actor share and fee (§8, §9.4). The
/// signed bytes stay inside: `export_standby` hands them out under a grant
/// (m-22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standby {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub fee: u64,
    /// The first height the full-principal variant is valid at: the
    /// registration's height plus the early period.
    pub full_valid_from: u32,
    /// Which variants are stored. At penalty 0 the two are one transaction,
    /// stored once as the full one (m-20); an import may hold either.
    pub full: bool,
    pub immediate: bool,
    pub source: StandbySource,
    pub created_at: u64,
}

/// A pasted standby, described as dash-qt describes one before it offers
/// to broadcast it (`masternodelist.cpp:300-373`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbyDescription {
    pub standby: Standby,
    pub variant: StandbyVariant,
    pub txid: String,
    /// What the actor pays the others: 0 for the full variant.
    pub penalty: u64,
    /// Every share in share order.
    pub returns: Vec<ShareReturn>,
    /// Every share gets its principal back (Core's note): the full
    /// variant; the immediate one pays the others the penalty.
    pub returns_principal: bool,
}

/// Core's standby file (§9.4, D22): `standby-dissolution-<first 8 of the
/// ProTx hash>-share<k>.txt` and its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbyFile {
    pub file_name: String,
    pub text: String,
}
