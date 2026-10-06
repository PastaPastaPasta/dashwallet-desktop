// M1 service contracts: the one error type view models handle.
import Foundation

/// Stable error code. Values are the engine's codes from
/// docs/contracts/m1-engine.md §4 ("Error codes"); view models choose UI copy
/// by code, never from `ServiceError.detail`. `engineCodes` lists every engine
/// code and a test compares it with the contract table (review M-9).
public struct ServiceErrorCode: RawRepresentable, Sendable, Hashable, Codable, CustomStringConvertible {
    public let rawValue: String

    public init(rawValue: String) {
        self.rawValue = rawValue
    }

    public var description: String { rawValue }

    // Common to every domain.
    public static let invalidArgument = Self(rawValue: "invalid_argument")
    public static let networkNotOpen = Self(rawValue: "network_not_open")
    public static let walletNotFound = Self(rawValue: "wallet_not_found")
    public static let storage = Self(rawValue: "storage")
    public static let notImplemented = Self(rawValue: "not_implemented")
    public static let `internal` = Self(rawValue: "internal")

    // Legacy `EngineError` (M0 calls).
    public static let invalidConfig = Self(rawValue: "invalid_config")
    public static let storageInUse = Self(rawValue: "storage_in_use")
    public static let wallet = Self(rawValue: "wallet")
    public static let sdk = Self(rawValue: "sdk")
    public static let spv = Self(rawValue: "spv")
    public static let io = Self(rawValue: "io")

    // Vault.
    public static let vaultNoVault = Self(rawValue: "vault.no_vault")
    public static let vaultAlreadyExists = Self(rawValue: "vault.already_exists")
    public static let vaultLocked = Self(rawValue: "vault.locked")
    public static let vaultWrongPassphrase = Self(rawValue: "vault.wrong_passphrase")
    public static let vaultThrottled = Self(rawValue: "vault.throttled")
    public static let vaultPassphraseRejected = Self(rawValue: "vault.passphrase_rejected")
    public static let vaultNotEncrypted = Self(rawValue: "vault.not_encrypted")
    public static let vaultAlreadyEncrypted = Self(rawValue: "vault.already_encrypted")
    public static let vaultGrantInvalid = Self(rawValue: "vault.grant_invalid")
    public static let vaultGrantPurposeMismatch = Self(rawValue: "vault.grant_purpose_mismatch")
    public static let vaultMixingOnly = Self(rawValue: "vault.mixing_only")
    public static let vaultCredentialRequired = Self(rawValue: "vault.credential_required")
    public static let vaultNoSecret = Self(rawValue: "vault.no_secret")
    public static let vaultQuickUnlockUnavailable = Self(rawValue: "vault.quick_unlock_unavailable")
    public static let vaultOSStoreUnavailable = Self(rawValue: "vault.os_store_unavailable")
    public static let vaultCorrupt = Self(rawValue: "vault.corrupt")

    // Wallet.
    public static let walletInvalidMnemonic = Self(rawValue: "wallet.invalid_mnemonic")
    public static let walletUnsupportedWordCount = Self(rawValue: "wallet.unsupported_word_count")
    public static let walletAlreadyExists = Self(rawValue: "wallet.already_exists")
    public static let walletWatchOnlyExists = Self(rawValue: "wallet.watch_only_exists")
    public static let walletNoVault = Self(rawValue: "wallet.no_vault")
    public static let walletVaultLocked = Self(rawValue: "wallet.vault_locked")
    public static let walletGrantInvalid = Self(rawValue: "wallet.grant_invalid")
    public static let walletNameRejected = Self(rawValue: "wallet.name_rejected")

    // Sync.
    public static let syncSpvNotRunning = Self(rawValue: "sync.spv_not_running")
    public static let syncHeightOutOfRange = Self(rawValue: "sync.height_out_of_range")
    public static let syncSpv = Self(rawValue: "sync.spv")

    // History.
    public static let historyInvalidQuery = Self(rawValue: "history.invalid_query")
    public static let historyStaleCursor = Self(rawValue: "history.stale_cursor")
    public static let historyTxNotFound = Self(rawValue: "history.tx_not_found")

    // Receive.
    public static let receiveRequestNotFound = Self(rawValue: "receive.request_not_found")
    public static let receiveGapLimit = Self(rawValue: "receive.gap_limit")

    // Send (dash-qt SendCoinsReturn statuses, QT-062).
    public static let sendNoRecipients = Self(rawValue: "send.no_recipients")
    public static let sendInvalidAddress = Self(rawValue: "send.invalid_address")
    public static let sendPlatformAddress = Self(rawValue: "send.platform_address")
    public static let sendInvalidAmount = Self(rawValue: "send.invalid_amount")
    public static let sendDustAmount = Self(rawValue: "send.dust_amount")
    public static let sendDuplicateAddress = Self(rawValue: "send.duplicate_address")
    public static let sendAmountExceedsBalance = Self(rawValue: "send.amount_exceeds_balance")
    public static let sendAmountWithFeeExceedsBalance = Self(rawValue: "send.amount_with_fee_exceeds_balance")
    public static let sendInsufficientMixedFunds = Self(rawValue: "send.insufficient_mixed_funds")
    public static let sendOutpointUnavailable = Self(rawValue: "send.outpoint_unavailable")
    public static let sendAbsurdFee = Self(rawValue: "send.absurd_fee")
    public static let sendTxTooLarge = Self(rawValue: "send.tx_too_large")
    public static let sendInvalidChangeAddress = Self(rawValue: "send.invalid_change_address")
    public static let sendWatchOnly = Self(rawValue: "send.watch_only")
    public static let sendVaultLocked = Self(rawValue: "send.vault_locked")
    public static let sendGrantInvalid = Self(rawValue: "send.grant_invalid")
    public static let sendGrantExceeded = Self(rawValue: "send.grant_exceeded")
    public static let sendPreparedTxSpent = Self(rawValue: "send.prepared_tx_spent")
    public static let sendNoPeers = Self(rawValue: "send.no_peers")
    public static let sendBroadcastRejected = Self(rawValue: "send.broadcast_rejected")
    public static let sendAmountTooSmallAfterFee = Self(rawValue: "send.amount_too_small_after_fee")
    /// Handed to the network without an acceptance verdict: never abandon (m1-engine.md §2.7).
    public static let sendBroadcastUnknown = Self(rawValue: "send.broadcast_unknown")

    // Coins.
    public static let coinsOutpointNotFound = Self(rawValue: "coins.outpoint_not_found")

    // Labels.
    public static let labelsInvalidAddress = Self(rawValue: "labels.invalid_address")
    public static let labelsDuplicateAddress = Self(rawValue: "labels.duplicate_address")
    public static let labelsOwnAddress = Self(rawValue: "labels.own_address")
    public static let labelsEntryNotFound = Self(rawValue: "labels.entry_not_found")
    public static let labelsReceiveEntryNotDeletable = Self(rawValue: "labels.receive_entry_not_deletable")

    // Message.
    public static let messageInvalidAddress = Self(rawValue: "message.invalid_address")
    public static let messageAddressNoKey = Self(rawValue: "message.address_no_key")
    public static let messageMalformedSignature = Self(rawValue: "message.malformed_signature")
    public static let messagePubkeyNotRecovered = Self(rawValue: "message.pubkey_not_recovered")
    public static let messageNotSigned = Self(rawValue: "message.not_signed")
    public static let messageAddressNotMine = Self(rawValue: "message.address_not_mine")
    public static let messageWatchOnly = Self(rawValue: "message.watch_only")
    public static let messageVaultLocked = Self(rawValue: "message.vault_locked")
    public static let messageGrantInvalid = Self(rawValue: "message.grant_invalid")

    // URI.
    public static let uriDoubleSlash = Self(rawValue: "uri.double_slash")
    public static let uriNotDashURI = Self(rawValue: "uri.not_dash_uri")
    public static let uriUnparsable = Self(rawValue: "uri.unparsable")
    public static let uriBip70Unsupported = Self(rawValue: "uri.bip70_unsupported")
    public static let uriInvalidAddress = Self(rawValue: "uri.invalid_address")
    public static let uriInvalidAmount = Self(rawValue: "uri.invalid_amount")
    public static let uriTooLongForQR = Self(rawValue: "uri.too_long_for_qr")

    // Units.
    public static let unitsUnparsable = Self(rawValue: "units.unparsable")

    // M2 additions to M1 domains (docs/contracts/m2-engine.md §4).
    public static let vaultQuickUnlockLimitExceeded = Self(rawValue: "vault.quick_unlock_limit_exceeded")
    public static let vaultPassphraseStale = Self(rawValue: "vault.passphrase_stale")
    public static let vaultNotEmpty = Self(rawValue: "vault.not_empty")
    public static let vaultRecoveryMismatch = Self(rawValue: "vault.recovery_mismatch")
    public static let walletInvalidXpub = Self(rawValue: "wallet.invalid_xpub")
    public static let syncSpvRunning = Self(rawValue: "sync.spv_running")
    public static let syncRescanInProgress = Self(rawValue: "sync.rescan_in_progress")
    public static let syncPeerNotFound = Self(rawValue: "sync.peer_not_found")

    // Transaction actions (M2).
    public static let txActionTxNotFound = Self(rawValue: "tx_action.tx_not_found")
    /// `parameters["refusal"]` is a `TransactionActionRefusal.rawValue`.
    public static let txActionRefused = Self(rawValue: "tx_action.refused")
    public static let txActionSpvNotRunning = Self(rawValue: "tx_action.spv_not_running")
    public static let txActionNoPeers = Self(rawValue: "tx_action.no_peers")

    // Console (M2).
    public static let consoleParseError = Self(rawValue: "console.parse_error")
    /// `detail` is Core's "message (code N)" line, printed as RPC output.
    public static let consoleRPCError = Self(rawValue: "console.rpc_error")
    public static let consoleNotAvailable = Self(rawValue: "console.not_available")
    public static let consoleAuthorizationRequired = Self(rawValue: "console.authorization_required")
    public static let consoleWalletRequired = Self(rawValue: "console.wallet_required")

    // dash-qt compatibility (M2).
    public static let compatFileUnreadable = Self(rawValue: "compat.file_unreadable")
    public static let compatUnsupportedFormat = Self(rawValue: "compat.unsupported_format")
    public static let compatCorrupt = Self(rawValue: "compat.corrupt")
    public static let compatPassphraseRequired = Self(rawValue: "compat.passphrase_required")
    public static let compatWrongPassphrase = Self(rawValue: "compat.wrong_passphrase")
    public static let compatNoHDChain = Self(rawValue: "compat.no_hd_chain")
    public static let compatNetworkMismatch = Self(rawValue: "compat.network_mismatch")
    public static let compatInvalidKeyMaterial = Self(rawValue: "compat.invalid_key_material")
    public static let compatAlreadyExists = Self(rawValue: "compat.already_exists")
    public static let compatNoVault = Self(rawValue: "compat.no_vault")
    public static let compatVaultLocked = Self(rawValue: "compat.vault_locked")
    public static let compatGrantInvalid = Self(rawValue: "compat.grant_invalid")
    public static let compatWatchOnly = Self(rawValue: "compat.watch_only")
    public static let compatDestinationUnwritable = Self(rawValue: "compat.destination_unwritable")

    // Backups (M2).
    public static let backupVaultLocked = Self(rawValue: "backup.vault_locked")
    public static let backupPassphraseRequired = Self(rawValue: "backup.passphrase_required")
    public static let backupWrongPassphrase = Self(rawValue: "backup.wrong_passphrase")
    public static let backupCorrupt = Self(rawValue: "backup.corrupt")
    public static let backupUnsupportedVersion = Self(rawValue: "backup.unsupported_version")
    public static let backupNetworkMismatch = Self(rawValue: "backup.network_mismatch")
    public static let backupAlreadyExists = Self(rawValue: "backup.already_exists")
    public static let backupDestinationUnwritable = Self(rawValue: "backup.destination_unwritable")

    // PSBT (M2).
    public static let psbtInvalid = Self(rawValue: "psbt.invalid")
    public static let psbtTooLarge = Self(rawValue: "psbt.too_large")
    public static let psbtNetworkMismatch = Self(rawValue: "psbt.network_mismatch")
    public static let psbtNotComplete = Self(rawValue: "psbt.not_complete")
    public static let psbtFeeRateTooHigh = Self(rawValue: "psbt.fee_rate_too_high")
    public static let psbtFeeUnknown = Self(rawValue: "psbt.fee_unknown")
    public static let psbtAbsurdFee = Self(rawValue: "psbt.absurd_fee")
    public static let psbtWatchOnly = Self(rawValue: "psbt.watch_only")
    public static let psbtVaultLocked = Self(rawValue: "psbt.vault_locked")
    public static let psbtGrantInvalid = Self(rawValue: "psbt.grant_invalid")
    public static let psbtGrantExceeded = Self(rawValue: "psbt.grant_exceeded")
    public static let psbtNoPeers = Self(rawValue: "psbt.no_peers")
    public static let psbtBroadcastRejected = Self(rawValue: "psbt.broadcast_rejected")
    public static let psbtBroadcastUnknown = Self(rawValue: "psbt.broadcast_unknown")

    // Desktop OS services (M2; also `PlatformServiceError.code`).
    public static let desktopUnsupported = Self(rawValue: "desktop.unsupported")
    public static let desktopOSError = Self(rawValue: "desktop.os_error")
    public static let desktopNoQRCode = Self(rawValue: "desktop.no_qr_code")
    public static let desktopImageUnreadable = Self(rawValue: "desktop.image_unreadable")

    // Swift-side codes (not from the engine).
    /// The authentication gate's watchdog fired before the vault answered.
    public static let authTimedOut = Self(rawValue: "auth.timed_out")
    /// The prepared transaction is not one this draft holds (already
    /// broadcast or abandoned, or from another draft).
    public static let sendPreparedTxUnknown = Self(rawValue: "send.prepared_tx_unknown")
    /// A broadcast of this prepared transaction failed after it may have
    /// reached a peer. Its inputs stay reserved: abandoning them could let the
    /// next send double-spend a transaction already in the mempool (review M-7).
    public static let sendBroadcastOutcomeUnknown = Self(rawValue: "send.broadcast_outcome_unknown")
    /// The settings file could not be written.
    public static let settingsWriteFailed = Self(rawValue: "settings.write_failed")
    /// M2 command line (`LaunchArgumentsParsing`, QT-006).
    public static let launchUnknownOption = Self(rawValue: "launch.unknown_option")
    public static let launchInvalidValue = Self(rawValue: "launch.invalid_value")
    public static let launchOptionAfterURI = Self(rawValue: "launch.option_after_uri")
    /// M2 OS services: the user or the OS refused (permission), or the user
    /// dismissed a prompt (`PlatformServiceError`).
    public static let platformDenied = Self(rawValue: "platform.denied")
    public static let platformCancelled = Self(rawValue: "platform.cancelled")

    /// Every engine code of docs/contracts/m1-engine.md §4, common codes
    /// first. The legacy `EngineError` codes `invalid_mnemonic` and
    /// `wallet_already_exists` are absent: DashKit reports them as
    /// `wallet.invalid_mnemonic` / `wallet.already_exists`.
    public static let engineCodes: [ServiceErrorCode] = [
        .invalidArgument, .networkNotOpen, .walletNotFound, .storage, .notImplemented, .internal,
        .invalidConfig, .storageInUse, .wallet, .sdk, .spv, .io,
        .vaultNoVault, .vaultAlreadyExists, .vaultLocked, .vaultWrongPassphrase, .vaultThrottled,
        .vaultPassphraseRejected, .vaultNotEncrypted, .vaultAlreadyEncrypted, .vaultGrantInvalid,
        .vaultGrantPurposeMismatch, .vaultMixingOnly, .vaultCredentialRequired, .vaultNoSecret, .vaultQuickUnlockUnavailable,
        .vaultOSStoreUnavailable, .vaultCorrupt,
        .walletInvalidMnemonic, .walletUnsupportedWordCount, .walletAlreadyExists, .walletWatchOnlyExists,
        .walletNoVault, .walletVaultLocked, .walletGrantInvalid, .walletNameRejected,
        .syncSpvNotRunning, .syncHeightOutOfRange, .syncSpv,
        .historyInvalidQuery, .historyStaleCursor, .historyTxNotFound,
        .receiveRequestNotFound, .receiveGapLimit,
        .sendNoRecipients, .sendInvalidAddress, .sendPlatformAddress, .sendInvalidAmount, .sendDustAmount,
        .sendDuplicateAddress, .sendAmountExceedsBalance, .sendAmountWithFeeExceedsBalance,
        .sendAmountTooSmallAfterFee, .sendInsufficientMixedFunds, .sendOutpointUnavailable, .sendAbsurdFee, .sendTxTooLarge,
        .sendInvalidChangeAddress, .sendWatchOnly, .sendVaultLocked, .sendGrantInvalid, .sendGrantExceeded,
        .sendPreparedTxSpent, .sendNoPeers, .sendBroadcastRejected, .sendBroadcastUnknown,
        .coinsOutpointNotFound,
        .labelsInvalidAddress, .labelsDuplicateAddress, .labelsOwnAddress, .labelsEntryNotFound,
        .labelsReceiveEntryNotDeletable,
        .messageInvalidAddress, .messageAddressNoKey, .messageMalformedSignature, .messagePubkeyNotRecovered,
        .messageNotSigned, .messageAddressNotMine, .messageWatchOnly, .messageVaultLocked, .messageGrantInvalid,
        .uriDoubleSlash, .uriNotDashURI, .uriUnparsable, .uriBip70Unsupported, .uriInvalidAddress,
        .uriInvalidAmount, .uriTooLongForQR,
        .unitsUnparsable,
    ]

    /// Every domain code of docs/contracts/m2-engine.md §4 (the M2 domains and
    /// the M2 additions to M1 domains); a test compares it with the table.
    public static let m2EngineCodes: [ServiceErrorCode] = [
        .txActionTxNotFound, .txActionRefused, .txActionSpvNotRunning, .txActionNoPeers,
        .consoleParseError, .consoleRPCError, .consoleNotAvailable, .consoleAuthorizationRequired,
        .consoleWalletRequired,
        .compatFileUnreadable, .compatUnsupportedFormat, .compatCorrupt, .compatPassphraseRequired,
        .compatWrongPassphrase, .compatNoHDChain, .compatNetworkMismatch, .compatInvalidKeyMaterial,
        .compatAlreadyExists, .compatNoVault, .compatVaultLocked, .compatGrantInvalid, .compatWatchOnly,
        .compatDestinationUnwritable,
        .backupVaultLocked, .backupPassphraseRequired, .backupWrongPassphrase, .backupCorrupt,
        .backupUnsupportedVersion, .backupNetworkMismatch, .backupAlreadyExists, .backupDestinationUnwritable,
        .psbtInvalid, .psbtTooLarge, .psbtNetworkMismatch, .psbtNotComplete, .psbtFeeRateTooHigh, .psbtFeeUnknown,
        .psbtAbsurdFee, .psbtWatchOnly,
        .psbtVaultLocked, .psbtGrantInvalid, .psbtGrantExceeded, .psbtNoPeers, .psbtBroadcastRejected,
        .psbtBroadcastUnknown,
        .desktopUnsupported, .desktopOSError, .desktopNoQRCode, .desktopImageUnreadable,
        .vaultQuickUnlockLimitExceeded, .vaultPassphraseStale, .vaultNotEmpty, .vaultRecoveryMismatch,
        .walletInvalidXpub,
        .syncSpvRunning, .syncRescanInProgress, .syncPeerNotFound,
    ]
}

/// An error from any M1 service.
public struct ServiceError: Error, Sendable, Equatable {
    public let code: ServiceErrorCode
    /// Diagnostic text for logs. Never shown to the user.
    public let detail: String
    /// Send errors that concern one recipient carry its index (QT-055).
    public let recipientIndex: Int?
    /// Vault throttling: seconds until the next attempt is accepted (IOS-012).
    public let retryAfterSeconds: UInt64?
    /// Numbers the UI copy needs, keyed by the engine's field names (review
    /// M-5): `fee` and `available` (duffs) for the balance errors, `max_duffs`
    /// for `send.grant_exceeded`, `failed_attempts` for vault attempts,
    /// `height` for `sync.height_out_of_range`, `index` for recipient errors;
    /// M2: `limit_duffs`, `size_bytes`, `duffs_per_kb`, `version`, `refusal`.
    public let parameters: [String: Int64]

    public init(
        code: ServiceErrorCode, detail: String = "", recipientIndex: Int? = nil, retryAfterSeconds: UInt64? = nil,
        parameters: [String: Int64] = [:]
    ) {
        self.code = code
        self.detail = detail
        self.recipientIndex = recipientIndex
        self.retryAfterSeconds = retryAfterSeconds
        self.parameters = parameters
    }
}
