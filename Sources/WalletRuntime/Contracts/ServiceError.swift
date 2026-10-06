// M1 service contracts: the one error type view models handle.
import Foundation

/// Stable error code. Values are the engine's codes from
/// docs/contracts/m1-engine.md ("Error codes"); view models choose UI copy by
/// code, never from `ServiceError.detail`.
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

    // Vault.
    public static let vaultNoVault = Self(rawValue: "vault.no_vault")
    public static let vaultAlreadyExists = Self(rawValue: "vault.already_exists")
    public static let vaultLocked = Self(rawValue: "vault.locked")
    public static let vaultWrongPassphrase = Self(rawValue: "vault.wrong_passphrase")
    public static let vaultThrottled = Self(rawValue: "vault.throttled")
    public static let vaultPassphraseRejected = Self(rawValue: "vault.passphrase_rejected")
    public static let vaultGrantInvalid = Self(rawValue: "vault.grant_invalid")
    public static let vaultMixingOnly = Self(rawValue: "vault.mixing_only")
    public static let vaultNoSecret = Self(rawValue: "vault.no_secret")
    public static let vaultGrantPurposeMismatch = Self(rawValue: "vault.grant_purpose_mismatch")
    public static let vaultNotEncrypted = Self(rawValue: "vault.not_encrypted")

    // Wallet.
    public static let walletInvalidMnemonic = Self(rawValue: "wallet.invalid_mnemonic")
    public static let walletUnsupportedWordCount = Self(rawValue: "wallet.unsupported_word_count")
    public static let walletAlreadyExists = Self(rawValue: "wallet.already_exists")
    public static let walletWatchOnlyExists = Self(rawValue: "wallet.watch_only_exists")
    public static let walletNameRejected = Self(rawValue: "wallet.name_rejected")

    // Sync.
    public static let syncSpvNotRunning = Self(rawValue: "sync.spv_not_running")
    public static let syncHeightOutOfRange = Self(rawValue: "sync.height_out_of_range")

    // History.
    public static let historyStaleCursor = Self(rawValue: "history.stale_cursor")
    public static let historyTxNotFound = Self(rawValue: "history.tx_not_found")

    // Send (dash-qt SendCoinsReturn statuses, QT-062).
    public static let sendNoRecipients = Self(rawValue: "send.no_recipients")
    public static let sendInvalidAddress = Self(rawValue: "send.invalid_address")
    public static let sendPlatformAddress = Self(rawValue: "send.platform_address")
    public static let sendInvalidAmount = Self(rawValue: "send.invalid_amount")
    public static let sendDustAmount = Self(rawValue: "send.dust_amount")
    public static let sendDuplicateAddress = Self(rawValue: "send.duplicate_address")
    public static let sendAmountExceedsBalance = Self(rawValue: "send.amount_exceeds_balance")
    public static let sendAmountWithFeeExceedsBalance = Self(rawValue: "send.amount_with_fee_exceeds_balance")
    public static let sendAbsurdFee = Self(rawValue: "send.absurd_fee")
    public static let sendGrantExceeded = Self(rawValue: "send.grant_exceeded")
    public static let sendBroadcastRejected = Self(rawValue: "send.broadcast_rejected")
    public static let sendPreparedTxSpent = Self(rawValue: "send.prepared_tx_spent")
    public static let sendNoPeers = Self(rawValue: "send.no_peers")
    public static let sendAmountTooSmallAfterFee = Self(rawValue: "send.amount_too_small_after_fee")
    /// Handed to the network without an acceptance verdict: never abandon (m1-engine.md §2.7).
    public static let sendBroadcastUnknown = Self(rawValue: "send.broadcast_unknown")

    // URI / units / message / labels.
    public static let uriUnparsable = Self(rawValue: "uri.unparsable")
    public static let uriInvalidAddress = Self(rawValue: "uri.invalid_address")
    public static let unitsUnparsable = Self(rawValue: "units.unparsable")
    public static let messageNotSigned = Self(rawValue: "message.not_signed")
    public static let labelsDuplicateAddress = Self(rawValue: "labels.duplicate_address")

    // Swift-side codes (not from the engine).
    /// The authentication gate's watchdog fired before the vault answered.
    public static let authTimedOut = Self(rawValue: "auth.timed_out")
    /// The prepared transaction is not one this draft holds (already
    /// broadcast or abandoned, or from another draft).
    public static let sendPreparedTxUnknown = Self(rawValue: "send.prepared_tx_unknown")
    /// The settings file could not be written.
    public static let settingsWriteFailed = Self(rawValue: "settings.write_failed")
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

    public init(code: ServiceErrorCode, detail: String = "", recipientIndex: Int? = nil, retryAfterSeconds: UInt64? = nil) {
        self.code = code
        self.detail = detail
        self.recipientIndex = recipientIndex
        self.retryAfterSeconds = retryAfterSeconds
    }
}
