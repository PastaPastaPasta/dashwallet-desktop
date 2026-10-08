//! Records and enums of the DashPay facade (DASHPAY §3.6). The contract is
//! `docs/contracts/m4-dashpay-engine.md`; its §3 listing is generated from
//! this file and checked by `tests/m4_dashpay_contract.rs`.
//!
//! Conventions (m1-engine.md §1): identity ids are Base58, txids lower-case
//! hex, duffs and credits `u64`, times UNIX seconds, and an unknown value is
//! `None`. Data-carrying enums serialize with a `kind` tag.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Status and sync
// ---------------------------------------------------------------------------

/// The banner state shared by the Home card, the identity chip and the
/// Contacts empty state (F1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DashPayStatus {
    /// No identity yet. `reason` is why joining is not offered now; `None`
    /// means the user can join.
    NoIdentity { reason: Option<NoIdentityReason> },
    /// A registration flow is running; `draft` is its id.
    Registering { draft: String },
    /// The main identity's username is in a contest.
    ContestPending {
        identity: String,
        label: String,
        ends_at: Option<u64>,
    },
    /// `main` is the main identity.
    Ready { main: String },
    /// The bring-up did not settle (§3.2).
    StartupIncomplete { startup: StartupStatus },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoIdentityReason {
    /// The wallet has no keys (DashPay is read-only).
    WatchOnly,
    /// Core or masternode sync has not finished (§2.2 rule 5, §3.7).
    WaitingForSync,
}

/// The bring-up outcome: the library's seven `WalletStartupStatus` values
/// plus our own `NotRun`, `Starting` and `IdentityUnsettled` (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupStatus {
    NotRun,
    Starting,
    Ready,
    NoIdentity,
    PartialNoIdentity,
    DiscoveryFailed,
    PartialAccountsPending,
    SeedBindingUnverified,
    IdentityScanIncomplete,
    /// The bring-up ran with a locked vault and no signers; it runs again at
    /// the first unlock.
    IdentityUnsettled,
}

/// Tools ▸ Information's DashPay card (F23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPaySyncStatus {
    pub startup: StartupStatus,
    pub last_pass: Option<SyncPassReport>,
    /// Contacts waiting for an unlock to finish setting up.
    pub pending_contact_crypto: u32,
    pub loops: Vec<SyncLoopStatus>,
    pub quorum_source: QuorumSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPassReport {
    pub started_at: u64,
    pub finished_at: u64,
    pub new_requests: u32,
    pub new_contacts: u32,
    pub new_payments: u32,
    /// The error code of a failed pass.
    pub failure_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLoopStatus {
    pub sync_loop: SyncLoop,
    pub running: bool,
    pub last_run_at: Option<u64>,
    pub next_run_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncLoop {
    IdentitySync,
    DashPaySync,
    DpnsSync,
    PlatformAddressSync,
}

/// Where Platform proofs get their quorum keys (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuorumSource {
    /// dash-spv's masternode state.
    Spv,
    /// The trusted service, before masternode sync; results are unverified.
    TrustedFallback,
    /// The trusted service only: the developer toggle or the degraded mode.
    Trusted,
}

// ---------------------------------------------------------------------------
// Identities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySummary {
    pub identity: String,
    /// `i` in `m/9'/c'/5'/0'/0'/i'`.
    pub index: u32,
    pub names: Vec<String>,
    pub main_name: Option<String>,
    pub is_main: bool,
    /// Credits.
    pub balance: Option<u64>,
    pub has_dashpay_keys: bool,
    pub profile: Option<Profile>,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityDetail {
    pub summary: IdentitySummary,
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityKeyInfo {
    pub id: u32,
    pub purpose: KeyPurpose,
    pub security_level: SecurityLevel,
    pub key_type: KeyType,
    /// Hex.
    pub public_key: String,
    pub read_only: bool,
    pub disabled_at: Option<u64>,
    /// Base58 id of the contract the key is bound to.
    pub contract_bound: Option<String>,
    /// The document type within `contract_bound` (`contactRequest`).
    pub contract_bound_document_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPurpose {
    Authentication,
    Encryption,
    Decryption,
    Transfer,
    System,
    Voting,
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityLevel {
    Master,
    Critical,
    High,
    Medium,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    EcdsaSecp256k1,
    Bls12381,
    EcdsaHash160,
    Bip13ScriptHash,
    EddsaHash160,
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Input only: it can carry a bearer secret, so it does not serialize.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RegistrationRequest {
    pub label: String,
    pub temporary_label: Option<String>,
    pub funding: RegistrationFunding,
    pub initial_profile: Option<InitialProfile>,
}

/// The profile entered at Draft. Kept in `dp_registration.initial_profile`
/// until `ProfileCreated`, across restarts and restores, so the engine
/// resolves `avatar_candidate` to its URL, hash and fingerprint when the
/// draft is created; the row never holds a candidate id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_candidate: Option<String>,
}

/// Persisted as `dp_registration.funding` in DP1-02's versioned encoding
/// (§3.4), never in this type's serde form.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegistrationFunding {
    CoreBalance,
    Invitation {
        link_id: String,
    },
    ExistingIdentity {
        identity: String,
    },
    /// Developer builds only. The key goes to the vault, never to the row.
    FaucetAssetLock {
        outpoint: String,
        private_key: BearerSecret,
    },
}

/// A bearer credential handed in by the host: a faucet asset-lock key in
/// WIF, an invitation link, a `dapk` payload (§3.8). `Debug` never shows
/// it, it never serializes, and it is zeroed on drop.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub struct BearerSecret(Zeroizing<String>);

impl BearerSecret {
    pub fn new(secret: String) -> Self {
        Self(Zeroizing::new(secret))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for BearerSecret {
    fn from(secret: String) -> Self {
        Self::new(secret)
    }
}

impl std::fmt::Debug for BearerSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BearerSecret(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationQuote {
    pub contested: bool,
    /// Duffs locked into the asset lock.
    pub lock_duffs: u64,
    pub fee_duffs: u64,
    /// `lock_duffs + fee_duffs`: what the grant must cover.
    pub total_duffs: u64,
    /// Credits the identity keeps after the name and profile.
    pub remaining_credits: u64,
}

/// One `dp_registration` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationStatus {
    pub draft: String,
    pub phase: RegistrationPhase,
    pub label: String,
    pub temporary_label: Option<String>,
    pub identity: Option<String>,
    pub txid: Option<String>,
    /// While `phase` is `ProofWaiting`.
    pub proof: Option<ProofKind>,
    /// While `phase` is `Contested`.
    pub contest_ends_at: Option<u64>,
    /// The phase a `Failed` flow stopped in.
    pub failed_phase: Option<RegistrationPhase>,
    /// The error code of a `Failed` flow.
    pub error: Option<String>,
    pub retryable: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

/// The §3.4 state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationPhase {
    Draft,
    KeysPrepared,
    FundingSent,
    ProofWaiting,
    IdentityRegistered,
    NameRequested,
    NameRegistered,
    Contested,
    ProfileCreated,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    InstantSend,
    ChainLock,
}

/// Tools ▸ Repair "Finish transfers".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReport {
    pub resumed: u32,
    pub completed: u32,
    pub still_pending: u32,
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

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

/// How a user relates to one of the wallet's identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Stranger,
    Contact,
    PendingOutgoing,
    PendingIncoming,
    Ignored,
    IsSelf,
}

// ---------------------------------------------------------------------------
// Contacts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactQuery {
    /// Empty means every section.
    pub sections: Vec<ContactSection>,
    pub sort: ContactSort,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSection {
    Requests,
    Contacts,
    Pending,
    Hidden,
}

/// Android's four orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSort {
    DisplayName,
    Username,
    DateAdded,
    LastActivity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsPage {
    pub sections: Vec<ContactSectionPage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSectionPage {
    pub section: ContactSection,
    pub contacts: Vec<ContactSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSummary {
    pub contact: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub alias: Option<String>,
    pub relation: Relation,
    pub hidden: bool,
    pub channel: ChannelState,
    pub since: Option<u64>,
    pub last_activity_at: Option<u64>,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

/// The DIP-15 payment channel to a contact (F13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelState {
    NotEstablished,
    /// Waiting for an unlock to finish the contact crypto.
    SetupPending,
    Ready,
    /// `payment_channel_broken`.
    Broken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactDetail {
    pub summary: ContactSummary,
    pub profile: Option<Profile>,
    pub private_details: PrivateDetails,
    pub publish_state: PublishState,
    pub request_sent_at: Option<u64>,
    pub request_received_at: Option<u64>,
    pub payment_lock: Option<PaymentLock>,
}

/// "Only visible to you" (F12).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrivateDetails {
    pub alias: Option<String>,
    pub note: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishState {
    Local,
    Published,
    DeferredUntilTwoContacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    Ok,
    NoDashPayKeys,
    IsSelf,
    AlreadyContact,
    PendingOutgoing,
    PendingIncoming,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestOutcome {
    /// Sent; waiting for the other side.
    Pending { request: String },
    /// Both directions exist.
    Established { request: String },
}

/// A scanned or pasted contact, verified against Platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedContact {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    /// Set when the payload carried a `dapk` auto-accept proof: an id for
    /// the proof, which the engine holds in memory for `send_request`.
    pub scan: Option<String>,
    pub relation: Relation,
    /// Verified only through the trusted fallback (§2.2 rule 2).
    pub unverified: bool,
}

// ---------------------------------------------------------------------------
// Payments and activity
// ---------------------------------------------------------------------------

/// A per-contact send lock after an ambiguous broadcast (`dp_payment_lock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentLock {
    pub txid: String,
    pub since: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockResolution {
    /// The payment was found; the lock is cleared.
    Sent,
    /// The payment was never sent; the lock is cleared.
    NotSent,
    /// Still unknown; the lock stays.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityFilter {
    All,
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPage {
    pub items: Vec<ActivityItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    pub txid: String,
    pub direction: ActivityDirection,
    /// Duffs.
    pub amount: u64,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDirection {
    Sent,
    Received,
}

/// The other side of a contact payment; history and transaction records
/// gain it as `counterparty` in DP3-02.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counterparty {
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPage {
    /// Incoming requests waiting for an answer.
    pub pending: Vec<ContactSummary>,
    /// Unread events, newest first.
    pub new: Vec<DashPayEvent>,
    /// Read events, newest first.
    pub earlier: Vec<DashPayEvent>,
    pub next_cursor: Option<u64>,
}

/// One `dp_events` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPayEvent {
    pub id: u64,
    pub kind: EventKind,
    pub contact: Option<String>,
    /// A txid or a contact-request id.
    pub reference: Option<String>,
    pub at: u64,
    pub read_at: Option<u64>,
}

/// The journal kinds of §3.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    UsernameRegistered,
    ContestWon,
    ContestLost,
    RequestReceived,
    RequestAccepted,
    ContactEstablished,
    PaymentReceived,
}

// ---------------------------------------------------------------------------
// Profile and avatars
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_url: Option<String>,
    /// Hex SHA-256 of the image bytes, as published.
    pub avatar_hash: Option<String>,
    /// Hex dHash, as published.
    pub avatar_fingerprint: Option<String>,
    pub updated_at: Option<u64>,
}

/// Character limits from the DashPay contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLimits {
    pub display_name_max: u32,
    pub public_message_max: u32,
}

/// The whole new profile; `None` clears a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileEdit {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar: AvatarChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarChange {
    Keep,
    Remove,
    /// A candidate from `prepare_avatar`.
    Set {
        candidate: String,
    },
}

/// `Debug` shows the byte count and hides the e-mail address.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarSource {
    File {
        bytes: Vec<u8>,
        crop: Option<CropRect>,
    },
    Url {
        url: String,
    },
    Gravatar {
        email: String,
    },
}

impl std::fmt::Debug for AvatarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File { bytes, crop } => f
                .debug_struct("File")
                .field("bytes", &bytes.len())
                .field("crop", crop)
                .finish(),
            Self::Url { url } => f.debug_struct("Url").field("url", url).finish(),
            Self::Gravatar { .. } => f.write_str("Gravatar { .. }"),
        }
    }
}

/// Pixels of the source image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarCandidate {
    pub id: String,
    pub preview: AvatarImage,
    /// Set for `Url` and `Gravatar` sources, and after an upload.
    pub url: Option<String>,
    /// A `File` source needs `upload_avatar` before it can be published.
    pub needs_upload: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarSize {
    /// 128 px.
    Small,
    /// 256 px.
    Large,
}

/// An engine-re-encoded PNG thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarImage {
    pub png: Vec<u8>,
    pub size: AvatarSize,
}

// ---------------------------------------------------------------------------
// Credits
// ---------------------------------------------------------------------------

/// Credit costs of the DashPay actions, for "≈ N contact requests" and the
/// low-credit warnings (F8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostTable {
    pub contact_request: u64,
    pub profile_update: u64,
    pub contact_info: u64,
    pub enable_dashpay_keys: u64,
    pub credits_per_duff: u64,
    pub top_up_min_duffs: u64,
    /// Below this balance the UI warns.
    pub low_credits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpOutcome {
    pub txid: String,
    pub credits_added: Option<u64>,
    pub balance: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WithdrawAmount {
    All,
    Credits { credits: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawOutcome {
    pub credits: u64,
    pub expected_duffs: Option<u64>,
    pub remaining_credits: Option<u64>,
}

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvitationStatus {
    Valid {
        inviter: Option<Counterparty>,
        funding_duffs: u64,
        contested_allowed: bool,
        expires_at: Option<u64>,
    },
    Claimed,
    Invalid {
        reason: InvitationInvalidReason,
    },
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationInvalidReason {
    Malformed,
    WrongNetwork,
    AssetLockNotFound,
}
