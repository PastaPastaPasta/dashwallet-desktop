//! The records of the shared-masternode calls (MG-04b §8, mn-engine §5),
//! slice 1a's subset: the capability, the creation requests, the session
//! and its message. Same conventions as `records.rs`: display-order hex
//! hashes, `"<txid>-<n>"` outpoints, duffs, Unix seconds. No record holds a
//! secret: a session carries addresses, outpoints, the operator's public
//! key and signatures only, as dash-qt's session file does.

use serde::{Deserialize, Serialize};

use super::protx_records::{KeepOr, OperatorChoice, ProTxOperation, VotingChoice};

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

/// A stored session whose envelope no longer parses (D57), read from its
/// row alone. `shared_sessions` skips it, yet it may hold reserved coins:
/// `discard_shared_session` releases them while `discardable`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableSession {
    /// 64 lower-case hex.
    pub id: String,
    /// The id's first six characters, upper-case.
    pub code: String,
    pub kind: SharedKind,
    pub role: SharedRole,
    /// What `SharedSession.discardable` reads: nothing of it may be out.
    pub discardable: bool,
    /// `abandon_unreadable_shared_session` would spend one of its reserved
    /// coins back to the wallet as of this read: out, not a
    /// dissolve-together session, not settled by its stored columns, and
    /// a reserved coin unspent and free of other pending spends.
    pub abandonable: bool,
    /// The outpoints locked for it, `"txid:vout"`.
    pub reserved: Vec<String>,
    pub updated_at: u64,
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
    /// The funder abandoned its key change before sending it: its fee
    /// inputs are released and it never sends the request (MG-04b D102).
    Abandoned,
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
    /// A key change this wallet funds whose base the list has left: its
    /// signatures stay usable until a fee input is spent, so abandon it
    /// (`abandon_shared_session`, D25).
    Abandon,
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

/// A key change session's keys (MG-04b §3.3, D25): each key as the synced
/// list holds it now and as the request sets it ("unchanged" where they
/// are equal, dash-qt's `sharedmndialogs.cpp:1960-1971`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChangeView {
    /// The keys the request writes (a kept key copied from the list when
    /// it was prepared). The operator key is 96 hex, basic serialization.
    pub requested: KeyBase,
    /// The synced list's keys now; `None` while the masternode isn't in it.
    pub current: Option<KeyBase>,
    /// This wallet funds the request: it alone may send it (D99).
    pub funder: bool,
}

/// A request to change a shared masternode's registrar keys together
/// (`start_key_change`, §8): each key kept (copied from the synced list)
/// or set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChangeRequest {
    pub pro_tx_hash: String,
    pub operator: KeepOr<OperatorChoice>,
    pub voting: KeepOr<VotingChoice>,
}

/// What `send_maintenance` handed off (D95, D99): a dissolution as an
/// MG-04 operation of kind `dissolve`, a key change as the session itself,
/// which holds its fee inputs and their locks (§9.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaintenanceSent {
    Operation { operation: Box<ProTxOperation> },
    Session { session: Box<SharedSession> },
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
    /// `discard_shared_session` would delete it as of this read (D17),
    /// computed by the rule that call refuses by. A registration or key
    /// change reads `false` once `out_at` is set; a dissolve-together
    /// session follows its own rule. The call can still refuse on a newer
    /// state or while another call holds the session.
    pub discardable: bool,
    /// `abandon_shared_session` would spend (or, for a key change not yet
    /// out, release) as of this read, by the rule it refuses by: a
    /// registration out and not `confirmed` or `conflicted`; a key change
    /// of this wallet's funding, not `confirmed`, `conflicted` or `dead`;
    /// once out, each with a reserved coin unspent and free of other
    /// pending spends. A dissolve-together session never is.
    pub abandonable: bool,
    pub warnings: Vec<SharedWarning>,
    /// The terms this wallet last approved (M-1 case 2's diff base).
    pub approved_terms: Option<ShareTermsView>,
    /// A key change's base (D25).
    pub base_keys: Option<KeyBase>,
    /// A key change's keys, requested and current (D25).
    pub key_change: Option<KeyChangeView>,
    pub actor_fee: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: Option<String>,
    /// What the wallet can do to get a session's coins back, and what it
    /// knows of where they are (MG-10 §4.4 EP 4).
    pub recovery: SharedRecovery,
}

/// A session's recovery (MG-10 §4.4 EP 4), computed with the predicates
/// the recovery calls themselves run. `actions` is a safe subset of what
/// the engine accepts now: each listed action passes its call's guards
/// with its charge as the grant cap.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedRecovery {
    pub out_at: Option<u64>,
    /// Advisory: out, the list current with the tip, and three hours of
    /// tip time since `out_at` (MG-10 §4.2).
    pub overdue: bool,
    pub settlement: Settlement,
    pub handoff_error: Option<HandoffError>,
    /// The latest abandon spend; present exactly when one was signed.
    pub abandon_spend: Option<AbandonSpend>,
    /// Every abandon spend's txid the session signed, oldest first.
    pub attributed: Vec<String>,
    pub blocked: Option<Blocked>,
    pub actions: Vec<RecoveryAction>,
}

/// Where a session's reserved coins stand once a spend of them may be out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Settlement {
    #[default]
    None,
    /// An abandon spend is signed and no spend of a reserved coin is in a
    /// block.
    Pending { txid: String },
    /// A spend of a reserved coin is in a block a reorg could still undo.
    Confirming {
        by: SettledBy,
        confirmations: u32,
        /// The tracker has seen a ChainLock.
        chainlocked: bool,
    },
    /// That block is ChainLocked or six deep; `released` once the
    /// follower cleared the reservation.
    Settled { by: SettledBy, released: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettledBy {
    /// Any abandon spend the session signed.
    AbandonTx,
    OtherSpend,
}

/// The latest failed hand-off of the session's own transaction or of its
/// latest abandon spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffError {
    pub tx: HandoffTx,
    pub outcome: HandoffErrorOutcome,
    pub code: Option<String>,
    /// The latest resend of the session's own transaction was refused; an
    /// earlier attempt may be out, so `outcome` is `unknown`.
    pub resend_refused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffTx {
    /// The session's own transaction: the registration or the key change.
    Registration,
    Abandon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffErrorOutcome {
    /// A definite refusal of a first hand-off this engine recorded.
    Refused,
    Unknown,
}

/// The latest abandon spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbandonSpend {
    pub txid: String,
    pub signed_at: u64,
    /// Saved by an earlier version: no bytes or inputs were kept.
    pub legacy: bool,
    /// Advisory: three hours of tip time since `signed_at`.
    pub overdue: bool,
    /// Only while the settlement is `pending` and the session isn't
    /// settled.
    pub state: Option<AbandonSpendState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbandonSpendState {
    /// Its first hand-off was definitely refused.
    Refused,
    /// It may be on the network.
    Uncertain,
    /// The wallet recorded it, unconfirmed.
    InHistory,
    /// The wallet recorded it and the user abandoned it.
    AbandonedInActivity,
    /// Saved by an earlier version and not in the wallet's history.
    Legacy,
}

/// Why no abandon exit is listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    pub txid: Option<String>,
    pub reason: BlockedReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    InFlight,
    InstantLocked,
    SpvStopped,
    Rescanning,
    CoinElsewhere,
    NoOwnershipPath,
    TooSmall,
}

/// A recovery call the engine accepts now, with the grant cap it charges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveryAction {
    /// The session's own transaction again (`send_shared_registration`,
    /// `send_maintenance`).
    Resend {
        charge: u64,
    },
    /// A new abandon spend (`abandon_shared_session`).
    Abandon {
        charge: u64,
        spends: bool,
        replaces: Option<String>,
    },
    /// The latest abandon spend's bytes again (`resend_shared_abandon`).
    ResendAbandon {
        charge: u64,
    },
    /// Activity's Abandon of a transaction the wallet recorded.
    AbandonWalletTx {
        txid: String,
    },
    /// Activity's Resend of a transaction the wallet recorded.
    ResendWalletTx {
        txid: String,
    },
    Discard {
        releases: u64,
    },
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
    /// A key change sets a new operator key: Core resets the operator
    /// fields and PoSe-bans the masternode until a ProUpServTx signed by
    /// the new key (rc.3 `specialtxman.cpp:663-673`); the key is written in
    /// the basic scheme (`providertx_service.cpp:1348`) and the mempool
    /// takes one operator change of a masternode at a time
    /// (`txmempool.cpp:1730-1735`) (MG-04b D25, m-12).
    OperatorReset,
    /// A complete key change stays usable by the wallet that funds it until
    /// one of its fee inputs is spent: only that wallet can void it, by
    /// spending one (an abandon once it is out; before, an abandon only
    /// releases them, MG-04b D25, D102).
    UsableByFunder,
    /// A funder's key change has sat at `signing` for the overdue quiet
    /// period without a new signature. Before `out_at` an abandon spends
    /// nothing (D102). The hint does not change `next`.
    AbandonHint,
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
    /// The immediate penalty from registration bytes; None when those bytes
    /// are unavailable or undecodable. Some(0) means no penalty.
    pub penalty: Option<u64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedEligibility {
    Ok,
    UpgradeUnknown,
    ListUnavailable,
    TooEarly,
    Delisted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedShares {
    pub registered_height: u32,
    pub spend_height: u32,
    pub early_period_blocks: u32,
    pub early_penalty: u64,
    pub full_valid_from: u32,
    pub eligibility: SharedEligibility,
    pub full_valid_now: bool,
    pub session: Option<String>,
    pub shares: Vec<ShareFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareFact {
    pub index: u16,
    pub amount: u64,
    pub refund_address: String,
    pub mine: bool,
    pub reward: RewardDestination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardDestination {
    pub at_registration: String,
    pub latest_confirmed_local: Option<RewardUpdate>,
    pub latest_attempt: Option<RewardAttempt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardUpdate {
    pub operation: String,
    pub address: String,
    pub confirmed_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardAttemptState {
    Signed,
    NotSent,
    Sent,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardAttempt {
    pub operation: String,
    pub address: String,
    pub state: RewardAttemptState,
    pub sent_at: Option<u64>,
}
