//! The one error every command rejects with (E0-13 D-row "errors").
//!
//! `AppError { code, message, data }`:
//!
//! - `code` is a stable string the UI picks its copy by. In TypeScript it is
//!   a union of string literals ([`ErrorCode`]), so a `switch` over it is
//!   checked and a misspelt code does not compile. The strings are the
//!   engine's contract codes (m1–m4: `wallet_not_found`, `vault.locked`,
//!   `send.dust_amount`, `platform.grant_exceeded`, …) plus `app.*` for the
//!   binding's own refusals.
//! - `message` is diagnostic text for logs, never shown as copy.
//! - `data` carries the fields some codes have ([`ErrorData`]).
//!
//! Each engine error type maps here through one `From` with an exhaustive
//! `match`, so a new engine variant fails to compile until it has a code.

mod engine;
mod governance;
mod masternodes;
mod nested;
mod platform;
mod rates;

use serde::Serialize;
use ts_rs::TS;

use crate::dto::governance::{GovernanceSyncPhaseName, ProposalFieldName};
use crate::dto::platform::{
    BudgetPurposeName, DashPayProfile, DispatchStateName, EligibilityName, KeyPurposeName,
    ProfileFieldName, RevokeCauseName, UsernameRuleName,
};

pub use engine::{EngineCode, VaultCode};
pub use governance::GovernanceCode;
pub use masternodes::MasternodesCode;
pub use nested::{
    BackupCode, CoinJoinCode, CompatCode, ConsoleCode, LabelsCode, MasternodeCode, PsbtCode,
    SendCode, SignerCode, UriCode,
};
pub use platform::{
    AvatarCode, ContactCode, CreditsCode, IdentityCode, InvitationCode, NameCode, PlatformCode,
    ProfileCode, RegistrationCode, ShieldedCode,
};
pub use rates::RatesCode;

/// Error returned by every command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{}: {message}", code.as_str())]
pub struct AppError {
    pub code: ErrorCode,
    /// Diagnostic detail for logs; the UI's copy comes from `code`.
    pub message: String,
    pub data: Option<ErrorData>,
}

/// Every code a command can reject with, by domain. Serialised as the bare
/// code string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(untagged)]
pub enum ErrorCode {
    App(AppCode),
    Engine(EngineCode),
    Vault(VaultCode),
    Signer(SignerCode),
    Send(SendCode),
    Labels(LabelsCode),
    Compat(CompatCode),
    Backup(BackupCode),
    Psbt(PsbtCode),
    CoinJoin(CoinJoinCode),
    Masternode(MasternodeCode),
    Uri(UriCode),
    Console(ConsoleCode),
    Platform(PlatformCode),
    Identity(IdentityCode),
    Profile(ProfileCode),
    Registration(RegistrationCode),
    Name(NameCode),
    Contact(ContactCode),
    Invitation(InvitationCode),
    Avatar(AvatarCode),
    Credits(CreditsCode),
    Shielded(ShieldedCode),
    Governance(GovernanceCode),
    Masternodes(MasternodesCode),
    Rates(RatesCode),
}

/// Declares a code enum: each variant serialises as its string, and
/// `as_str` returns it.
macro_rules! codes {
    ($(#[$meta:meta])* $name:ident { $($(#[doc = $doc:literal])* $variant:ident = $code:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, ts_rs::TS)]
        pub enum $name {
            $($(#[doc = $doc])* #[serde(rename = $code)] $variant),+
        }

        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $code),+
                }
            }
        }
    };
}
pub(crate) use codes;

codes! {
    /// The binding's own refusals (`app.*`).
    AppCode {
        /// A self-test command in a normal launch.
        NotSelftest = "app.not_selftest",
        /// A developer-only command in a release build.
        NotDeveloperBuild = "app.not_developer_build",
        /// The self-test report could not be written.
        Selftest = "app.selftest",
        /// The self-test's passphrase file cannot be read, or is not the
        /// user's alone (on Unix: a regular file the user owns, mode 0600 or
        /// stricter, its final component not a symlink).
        SelftestPassphraseFile = "app.selftest_passphrase_file",
        /// The data root cannot be found (the platform's app-data directory
        /// is unknown), created or locked.
        DataDir = "app.data_dir",
        /// Another process holds the data root's instance lock: the wallet
        /// is not opened twice (U-01 D2).
        AlreadyRunning = "app.already_running",
        /// A handle id that is not (or no longer) open.
        UnknownHandle = "app.unknown_handle",
        /// More than `SendHandles::MAX` handles of a kind are open.
        TooManyHandles = "app.too_many_handles",
        /// The window's content protection change could not run (no main
        /// window, the event loop is gone; `shell_sync`, U-02 D27): the
        /// renderer keeps secrets hidden.
        ScreenProtection = "app.screen_protection",
        /// No file dialog could be shown, or the chosen path is not UTF-8
        /// (U-05 D2).
        FileDialog = "app.file_dialog",
        /// The chosen file is at or above its purpose's limit (a PSBT: 100 MiB).
        FileTooLarge = "app.file_too_large",
        /// The chosen file could not be read, or is not a regular file.
        FileRead = "app.file_read",
        /// The file could not be written where the user chose.
        FileWrite = "app.file_write",
        /// Not a PNG, JPEG or BMP image the QR decoder reads, or too large
        /// (DP2-08 D9).
        ImageUnreadable = "app.image_unreadable",
        /// The image holds no QR code that decodes.
        NoQrCode = "app.no_qr_code",
        /// This OS or desktop session offers no screen-region picker (macOS;
        /// Linux without the desktop portal; DP2-08 D10).
        ScreenCaptureUnavailable = "app.screen_capture_unavailable",
        /// The screen capture failed.
        ScreenCapture = "app.screen_capture",
        /// `switch_network` refused: closing the network would cut what
        /// runs on it ([`ErrorData::NetworkBusy`] names it). Nothing changed.
        NetworkBusy = "app.network_busy",
        /// `switch_network` could not open the chosen network
        /// ([`ErrorData::NetworkSwitch`]): the window is back on the
        /// network it left, or, when that reopen failed too, on none (the
        /// splash's Retry opens it). The choice was not saved.
        NetworkSwitchFailed = "app.network_switch_failed",
        /// The system browser could not be opened for an Insight transaction
        /// (`open_tx_in_explorer`).
        ExplorerOpen = "app.explorer_open",
        /// The system browser could not be opened (`open_releases_page`).
        BrowserOpen = "app.browser_open",
        /// No automatic backup has made the backup folder yet
        /// (`open_backup_directory`).
        BackupFolderMissing = "app.backup_folder_missing",
        /// The backup folder exists but no file manager opened it
        /// (`open_backup_directory`).
        FolderOpen = "app.folder_open",
        /// This copy cannot update itself (`UpdateAvailability`: a build
        /// without the updater key, a Flatpak, a package-manager install).
        UpdatesUnavailable = "app.updates_unavailable",
        /// The update feed could not be reached or read, or did not answer
        /// within 30 s.
        UpdateCheck = "app.update_check",
        /// The Latest release publishes no update feed (`latest.json`).
        UpdateNotPublished = "app.update_not_published",
        /// The update could not be downloaded, or no data came for 60 s.
        UpdateDownload = "app.update_download",
        /// The update's signature does not verify against the release key,
        /// or names another version than the feed announced: nothing was
        /// installed.
        UpdateSignature = "app.update_signature",
        /// The verified update could not be put in place.
        UpdateInstall = "app.update_install",
        /// The step does not fit the update's state: no update (or another
        /// version) on offer, one already being installed, or a restart
        /// before the install.
        UpdateState = "app.update_state",
        /// Install or Restart would close the engine while something runs
        /// ([`ErrorData::UpdateBusy`] names it). Nothing changed.
        UpdateBusy = "app.update_busy",
        /// A shared-masternode send on a network this release doesn't send
        /// them on ([`ErrorData::SharedSendNetwork`]): mainnet. Nothing was
        /// signed or sent.
        SharedSendOff = "app.shared_send_off",
        /// `mn_shared_send_maintenance` on a session kind this release
        /// doesn't send ([`ErrorData::SharedUnsupported`]): a key change.
        /// Nothing was signed or sent.
        SharedSendUnsupported = "app.shared_send_unsupported",
    }
}

/// Why a network's close refused itself ([`ErrorData::NetworkCloseRefused`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CloseRefusalReason {
    /// A wallet is still opening.
    WalletOpening,
    /// A shielded spend's result is not known yet.
    ShieldedSpendUnresolved,
}

/// Why the data folder is not private ([`ErrorData::NotPrivate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NotPrivateReason {
    /// Its group can write to it.
    GroupWritable,
    /// Every account can write to it.
    OtherWritable,
    /// Another account owns it.
    OtherOwner,
}

impl From<dw_fs::InsecureReason> for NotPrivateReason {
    fn from(r: dw_fs::InsecureReason) -> Self {
        match r {
            dw_fs::InsecureReason::GroupWritable => Self::GroupWritable,
            dw_fs::InsecureReason::OtherWritable => Self::OtherWritable,
            dw_fs::InsecureReason::OtherOwner => Self::OtherOwner,
        }
    }
}

/// What `app.update_busy` waits for ([`ErrorData::UpdateBusy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UpdateBusyReason {
    /// The engine is still starting.
    Startup,
    /// A payment's broadcast has not returned.
    Payment,
    /// A wallet mixes.
    Mixing,
    /// A Dash Platform flow holds a lease.
    Platform,
}

/// Fields some codes carry. Credit amounts are decimal strings (see
/// `dto::credits`); duff amounts are numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ErrorData {
    /// `vault.throttled`: when to allow the next try.
    RetryAfter { secs: u64 },
    /// `app.network_busy`: what runs on the network the window would leave.
    NetworkBusy {
        /// The Platform flows holding a lease.
        flows: Vec<crate::dto::platform::FlowKindName>,
        /// Every one of `flows` waits for a proof or a grant; locking the
        /// wallet ends their leases.
        stalled: bool,
        /// A wallet mixes.
        mixing: bool,
        /// A broadcast has not returned.
        broadcast: bool,
        /// The network's operations already running did not finish in
        /// time; nothing else is named.
        draining: bool,
    },
    /// `app.update_busy`: what Install or Restart waits for.
    UpdateBusy { reason: UpdateBusyReason },
    /// `wallet` or `shielded_spend_unresolved` from `switch_network`: the
    /// network's close refused itself. Nothing changed.
    NetworkCloseRefused { reason: CloseRefusalReason },
    /// `app.network_switch_failed`: the network that did not open, and
    /// whether the window is back on the one it left.
    NetworkSwitch {
        network: crate::dto::NetworkInfo,
        rolled_back: bool,
    },
    /// `vault.wrong_passphrase`: failed attempts so far, and the wait before
    /// the next try once the vault throttles.
    WrongPassphrase {
        failed_attempts: u32,
        retry_after_secs: Option<u64>,
    },
    /// `console.rpc_error`: Core's error code and message for the same
    /// failure, shown as "message (code N)".
    Rpc { rpc_code: i32, rpc_message: String },
    /// `console.authorization_required`: the grant to ask for (`authorize`),
    /// and the wallet it is for; then run the line again with it.
    Authorize {
        purpose: crate::api::vault::GrantPurposeSpec,
        wallet_id: Option<String>,
    },
    /// A duff shortfall (`*.funding_insufficient`, `send.amount_exceeds_balance`,
    /// `send.insufficient_mixed_funds`, …).
    Duffs { needed: Option<u64>, available: u64 },
    /// `platform.insufficient_credits`: a credit shortfall.
    Credits {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        needed: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        available: u64,
    },
    /// `credits.withdrawal_out_of_range`: what a withdrawal may take, in credits.
    CreditRange {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        min: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        max: u64,
    },
    /// `credits.send_out_of_range`: the least a Platform send may pay, in credits.
    CreditMinimum {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        min: u64,
    },
    /// `insufficient_credits`: the identity (Base58) whose balance is short.
    IdentityCredits {
        identity: String,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        needed: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        available: u64,
    },
    /// `credits.below_minimum`, `vault.quick_unlock_limit_exceeded`,
    /// `psbt.grant_exceeded` (the grant's cap), `coinjoin.insufficient_funds`
    /// (the mixing minimum): a limit in duffs.
    Limit { duffs: u64 },
    /// `send.absurd_fee`, `send.amount_with_fee_exceeds_balance`, `psbt.absurd_fee`.
    Fee { fee: u64, available: Option<u64> },
    /// `send.grant_exceeded`: the payment's outflow against the grant's cap, in duffs.
    SpendCap { max_duffs: u64, outflow: u64 },
    /// `send.*` refusals of one recipient (its index in the draft).
    Recipient { index: u32 },
    /// `platform.grant_exceeded`: amounts in the budget's unit (duffs or credits), as strings.
    Budget {
        purpose: BudgetPurposeName,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        needed: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        remaining: u64,
    },
    /// `platform.needs_grant`.
    NeedsGrant { purpose: BudgetPurposeName },
    /// `platform.lease_revoked`.
    Revoked { cause: RevokeCauseName },
    /// `platform.broadcast_unknown`, `platform.will_be_sent`: what to ask
    /// `dispatch_status` about.
    Artifact { artifact: String },
    /// `app.browser_open` from `open_project_link`: the page that did not
    /// open, for the user to copy.
    Link { url: String },
    /// `contact.payment_locked`, `credits.top_up_pending` (the lock to
    /// resume), and a txid elsewhere.
    Txid { txid: String },
    /// `registration.recoverable`: the draft to resume;
    /// `registration.in_progress`: the draft that blocks (or
    /// `registration/<n>/funding` for a funding step whose row is gone).
    Draft { draft: String },
    /// `registration.funding_unknown_recent`: the earlier registration
    /// whose funding may still appear, and how long until a start may pass
    /// it, when known.
    DraftWait {
        draft: String,
        wait_secs: Option<u64>,
    },
    /// `registration.shielded_not_ready`: the first readiness check that
    /// does not hold, and when the notes have rested (`rested`).
    ShieldedNotReady {
        reason: crate::dto::platform::ShieldedCheckName,
        ready_at: Option<u64>,
    },
    /// `name.invalid`: the rules the label breaks.
    UsernameRules { rules: Vec<UsernameRuleName> },
    /// `contact.ineligible`: the eligibility answer that refused.
    Ineligible { reason: EligibilityName },
    /// `identity.keys_missing`.
    KeysMissing { purpose: KeyPurposeName },
    /// `profile.too_long`: the text, its length and the contract's limit, in characters.
    ProfileTooLong {
        field: ProfileFieldName,
        length: u32,
        max: u32,
    },
    /// `profile.conflict`: the profile on Platform now, as `dashpayProfile` shows it.
    ProfileConflict {
        current: Option<Box<DashPayProfile>>,
    },
    /// `platform.feature_off`.
    Feature { feature: String },
    /// `governance.not_synced`: the phase sync is in (no run reached
    /// `synced` yet, or no tip for the clock).
    GovernancePhase { phase: GovernanceSyncPhaseName },
    /// `governance.proposal_not_found`: the hash asked for.
    ProposalHash { hash: String },
    /// `governance.invalid_proposal`: every draft field that failed, in
    /// wizard order.
    ProposalFields { fields: Vec<ProposalFieldName> },
    /// `governance.contest_not_found`, `governance.contest_closed`: the
    /// normalized DPNS label (MG-07 D13).
    ContestLabel { label: String },
    /// `governance.not_a_contender`: the identity id (base58) the poll does
    /// not list (MG-07 D13).
    Contender { identity: String },
    /// `masternodes.key_mismatch`, `masternodes.key_exists`: the role the key
    /// was given for.
    MasternodeKey {
        role: crate::dto::masternodes::AttachedKeyRoleName,
    },
    /// `masternodes.operation_state`: the operation's state now.
    OperationState { state: String },
    /// `masternodes.keys_exhausted`: the keychain role with no unused key.
    KeyRole { role: String },
    /// `masternodes.update_pending`, `masternodes.update_superseded`: the
    /// other operation's id.
    Operation { operation: String },
    /// `masternodes.shared_message`: why a message was refused (never its
    /// text), and the share whose approval doesn't verify.
    SharedMessage { reason: String, share: Option<u16> },
    /// `masternodes.shared_stage`: the stage that refused the call.
    SharedStage { stage: String },
    /// `masternodes.shared_funding_mismatch`: the check that failed, and the
    /// least fee in duffs for `fee_low`.
    SharedFunding { reason: String, needed: Option<u64> },
    /// `masternodes.shared_actor_fee`: the fee in duffs the request takes
    /// from this wallet's share.
    SharedActorFee { fee: u64 },
    /// `app.shared_send_off`: the network the send was refused on, by its
    /// directory name (`mainnet`).
    SharedSendNetwork { network: String },
    /// `app.shared_send_unsupported`: the session kind whose send was refused.
    SharedUnsupported {
        session_kind: crate::dto::masternodes::SharedKindName,
    },
    /// `masternodes.unsupported`: what the call needs that the libraries
    /// lack, and the field that needs it.
    Unsupported {
        feature: String,
        field: Option<String>,
    },
    /// `platform.not_implemented`, `governance.not_implemented`,
    /// `masternodes.not_implemented`, `not_implemented`: the call that is not.
    Call { call: String },
    /// `send.broadcast_rejected`, `send.broadcast_unknown` and their `psbt.*` and
    /// `coinjoin.broadcast_rejected` twins: the peer's or library's reason;
    /// `governance.vote_rejected`: the check that failed;
    /// `governance.submit_refused`: why (`public`, `expired`, `fee_failed`,
    /// `in_flight`)
    /// (mg-engine §4); `shielded.not_synced`: what Shielded is still
    /// syncing (`account`, `tree`, `journal`, `core`, `startup`; m4 §2.13);
    /// `credits.recipient_invalid`: why the address cannot be paid (m4 §2.12).
    Reason { reason: String },
    /// `tx_action_refused`: why the abandon or resend was refused, and for
    /// `recordUnreadable` the wallet whose saved transaction didn't read.
    TxAction {
        refusal: TxActionRefusalName,
        wallet_id: Option<String>,
    },
    /// `request_not_found`, `height_out_of_range`, `backup.unsupported_version`,
    /// `psbt.too_large` (bytes), `shielded.too_many_inputs` (the coins the
    /// shield would spend; at most 50 fit): the number named.
    Number { value: u64 },
    /// `contact.details_too_long`: the details' DIP-15 encoding and its cap,
    /// in bytes.
    Size { bytes: u32, max: u32 },
    /// `send.outpoint_unavailable`, `outpoint_not_found`: the output (txid hex and index).
    Outpoint { txid: String, vout: u32 },
    /// `backup.already_exists`: the wallet (lower-case hex id) the backup would duplicate.
    Wallet { wallet_id: String },
    /// `compat.network_mismatch`: the keys are mainnet keys, or else test-network keys.
    KeysNetwork { mainnet: bool },
    /// `psbt.fee_rate_too_high`: the offending fee rate in duffs per kB.
    FeeRate { duffs_per_kb: u64 },
    /// `shielded.not_active`: the network's protocol version and the one
    /// the shielded pool needs.
    Protocol {
        protocol_version: u32,
        activation_version: u32,
    },
    /// `shielded.pool_too_small`: the notes the network's shielded pool
    /// holds and the number withdrawals need.
    PoolNotes { notes: u64, required: u64 },
    /// `stuck_funding`: why Repair refused (E0-04 §6.5), and whether the
    /// same Repair may succeed later with nothing done by the user.
    StuckFunding {
        refusal: StuckFundingRefusalName,
        retry_later: bool,
    },
    /// `storage_refused`: the open's store-upgrade check refused the
    /// network's wallet data and changed nothing in it. `screen` is the
    /// store upgrade's own screen with `{folder}` filled in; `folder` is the
    /// network directory; `cause` names the refusal for tests and logs only.
    StoreRefused {
        screen: Box<StoreScreen>,
        folder: String,
        cause: String,
    },
    /// `storage_not_private`: the open refused the data root before reading
    /// or writing anything below it. `folder` is the directory someone other
    /// than the user or root can change (the root, one above it, or one on a
    /// symlinked network directory's path), left as it is; trying again opens
    /// only once its permissions change.
    NotPrivate {
        folder: String,
        reason: NotPrivateReason,
    },
    /// `storage_in_use`: whether trying again in this process may succeed;
    /// false means only a restart clears it.
    StorageInUse { retryable: bool },
    /// `io` from the open's store-upgrade step: the machine failed (disk
    /// full, a read error), not the data; trying again may succeed.
    Io { retryable: bool },
}

/// A store refusal's screen, as `dw_engine::store_upgrade::copy::screen_for` gives
/// it: catalog keys with their English, `{folder}` filled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StoreScreen {
    pub title_key: String,
    pub title: String,
    pub text_key: String,
    pub text: String,
    pub buttons: Vec<StoreButton>,
    /// A line under the buttons.
    pub hint: Option<StoreHint>,
}

/// One button of a [`StoreScreen`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StoreButton {
    pub key: String,
    pub label: String,
    pub destructive: bool,
    pub default_focus: bool,
}

/// The line under a [`StoreScreen`]'s buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StoreHint {
    pub key: String,
    pub text: String,
}

impl StoreScreen {
    pub fn new(screen: &dw_engine::store_upgrade::copy::ScreenCopy, folder: &str) -> Self {
        Self {
            title_key: screen.title_key.into(),
            title: screen.title.into(),
            text_key: screen.text_key.into(),
            text: screen.text.replace("{folder}", folder),
            buttons: (screen.buttons.iter())
                .map(|b| StoreButton {
                    key: b.key.into(),
                    label: b.label.into(),
                    destructive: b.destructive,
                    default_focus: b.default_focus,
                })
                .collect(),
            hint: screen.hint.map(|(key, text)| StoreHint {
                key: key.into(),
                text: text.into(),
            }),
        }
    }
}

/// Why Repair refused a stuck funding step (E0-04 §6.5; the engine's
/// `StuckFundingRefusal`). Txids are display hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StuckFundingRefusalName {
    /// The wallet's tracked asset locks were not read yet this session.
    RowsUnknown,
    /// The funding is not stuck: `status` is its dispatch state (`null`:
    /// it never started).
    NotStuck {
        status: Option<DispatchStateName>,
    },
    /// No marker of the funding step stands.
    NoMarker,
    /// A call of the funding step is running.
    CallRunning,
    JournalUnavailable,
    /// This transaction spends the wallet's coins and is not ChainLocked
    /// yet: wait for its ChainLock, or abandon it.
    SpendUnsettled {
        txid: String,
    },
    /// Every coin this asset lock spends is already spent.
    LockInputsSpent {
        txid: String,
    },
    /// The self-spend spends no input of this signed asset lock.
    MissesSignedLock {
        txid: String,
    },
    /// This tracked asset lock may be the funding's; its identity is not
    /// known yet.
    UnattributedLock {
        txid: String,
    },
    /// No asset lock the funding signed is known.
    NoKnownLock,
}

impl From<dw_engine::platform::lease::StuckFundingRefusal> for StuckFundingRefusalName {
    fn from(r: dw_engine::platform::lease::StuckFundingRefusal) -> Self {
        use dw_engine::platform::lease::StuckFundingRefusal as R;
        let txid = |t: dashcore::Txid| t.to_string();
        match r {
            R::RowsUnknown => Self::RowsUnknown,
            R::NotStuck(status) => Self::NotStuck {
                status: status.map(Into::into),
            },
            R::NoMarker => Self::NoMarker,
            R::CallRunning => Self::CallRunning,
            R::JournalUnavailable => Self::JournalUnavailable,
            R::SpendUnsettled(t) => Self::SpendUnsettled { txid: txid(t) },
            R::LockInputsSpent(t) => Self::LockInputsSpent { txid: txid(t) },
            R::MissesSignedLock(t) => Self::MissesSignedLock { txid: txid(t) },
            R::UnattributedLock(t) => Self::UnattributedLock { txid: txid(t) },
            R::NoKnownLock => Self::NoKnownLock,
        }
    }
}

/// Why `abandon_transaction` or `resend_transaction` refused (QT-105).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TxActionRefusalName {
    Confirmed,
    InstantLocked,
    AlreadyAbandoned,
    Coinbase,
    InMempool,
    NotSentByWallet,
    /// Abandon can go through after a restart and a rescan from the
    /// wallet's birth (the message says so).
    RescanFirst,
    /// A later transaction that spends from it is already locked or
    /// confirmed, so this one will confirm too.
    SettledDescendant,
    /// A saved transaction of the loaded wallet `walletId` names didn't
    /// read at its load, so what descends from this one is unknown.
    RecordUnreadable,
}

impl From<dw_engine::TxActionRefusal> for TxActionRefusalName {
    fn from(r: dw_engine::TxActionRefusal) -> Self {
        use dw_engine::TxActionRefusal as R;
        match r {
            R::Confirmed => Self::Confirmed,
            R::InstantLocked => Self::InstantLocked,
            R::AlreadyAbandoned => Self::AlreadyAbandoned,
            R::Coinbase => Self::Coinbase,
            R::InMempool => Self::InMempool,
            R::NotSentByWallet => Self::NotSentByWallet,
            R::RescanFirst => Self::RescanFirst,
            R::SettledDescendant => Self::SettledDescendant,
            R::RecordUnreadable(_) => Self::RecordUnreadable,
        }
    }
}

impl AppError {
    pub fn new(code: impl Into<ErrorCode>, message: impl Into<String>) -> Self {
        AppError {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with(mut self, data: ErrorData) -> Self {
        self.data = Some(data);
        self
    }

    pub fn app(code: AppCode, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    /// The engine's `invalid_argument`, for arguments the binding parses
    /// itself (hex, outpoints, enum strings).
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(EngineCode::InvalidArgument, message)
    }

    pub fn code_str(&self) -> &'static str {
        self.code.as_str()
    }
}

macro_rules! into_code {
    ($($variant:ident($ty:ident)),+ $(,)?) => {
        $(impl From<$ty> for ErrorCode {
            fn from(c: $ty) -> Self {
                ErrorCode::$variant(c)
            }
        })+

        impl ErrorCode {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(ErrorCode::$variant(c) => c.as_str(),)+
                }
            }

            /// Every code, in declaration order (the TypeScript union's).
            pub fn all() -> Vec<ErrorCode> {
                let mut all = Vec::new();
                $(all.extend($ty::ALL.iter().map(|c| ErrorCode::$variant(*c)));)+
                all
            }
        }
    };
}

into_code!(
    App(AppCode),
    Engine(EngineCode),
    Vault(VaultCode),
    Signer(SignerCode),
    Send(SendCode),
    Labels(LabelsCode),
    Compat(CompatCode),
    Backup(BackupCode),
    Psbt(PsbtCode),
    CoinJoin(CoinJoinCode),
    Masternode(MasternodeCode),
    Uri(UriCode),
    Console(ConsoleCode),
    Platform(PlatformCode),
    Identity(IdentityCode),
    Profile(ProfileCode),
    Registration(RegistrationCode),
    Name(NameCode),
    Contact(ContactCode),
    Invitation(InvitationCode),
    Avatar(AvatarCode),
    Credits(CreditsCode),
    Shielded(ShieldedCode),
    Governance(GovernanceCode),
    Masternodes(MasternodesCode),
    Rates(RatesCode),
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialise_as_their_strings_and_are_unique() {
        let all = ErrorCode::all();
        let mut seen = std::collections::HashSet::new();
        for code in &all {
            let json = serde_json::to_value(code).unwrap();
            assert_eq!(json, serde_json::Value::String(code.as_str().into()));
            assert!(
                seen.insert(code.as_str()),
                "duplicate code {}",
                code.as_str()
            );
        }
    }

    #[test]
    fn an_error_is_code_message_and_data() {
        let e = AppError::new(VaultCode::Throttled, "slow down")
            .with(ErrorData::RetryAfter { secs: 30 });
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({
                "code": "vault.throttled",
                "message": "slow down",
                "data": { "kind": "retryAfter", "secs": 30 }
            })
        );
        let credits = ErrorData::Credits {
            needed: u64::MAX,
            available: 1,
        };
        assert_eq!(
            serde_json::to_value(&credits).unwrap()["needed"],
            serde_json::Value::String(u64::MAX.to_string())
        );
    }
}
