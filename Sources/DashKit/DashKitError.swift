import DashWalletCore
import Foundation

/// Errors from the engine. `detail` is diagnostic text for logs; UI copy is
/// chosen from `code` (docs/contracts/m1-engine.md §4).
public enum DashKitError: Error, Sendable, Equatable {
    case invalidConfig(detail: String)
    case invalidArgument(detail: String)
    case networkNotOpen(detail: String)
    case storageInUse(detail: String)
    case storage(detail: String)
    case walletNotFound(detail: String)
    case invalidMnemonic(detail: String)
    case walletAlreadyExists(detail: String)
    case wallet(detail: String)
    case sdk(detail: String)
    case spv(detail: String)
    case io(detail: String)
    /// The engine side of a call has not landed; `detail` names the call.
    case notImplemented(detail: String)
    case `internal`(detail: String)
    /// Any other M1 domain error; `code` is the engine's stable code.
    case domain(code: String, detail: String)
    /// A send error about one recipient (QT-055): `index` into the recipients.
    case recipient(code: String, index: Int)
    /// A vault passphrase attempt failed or is throttled (IOS-012).
    case vaultAttempt(code: String, failedAttempts: UInt32?, retryAfterSeconds: UInt64?)
    /// A domain error whose engine variant carries numbers the UI shows, such
    /// as the fee in dash-qt's "amount with fee exceeds balance" text
    /// (review M-5). Keys are the engine's field names.
    case parameterized(code: String, parameters: [String: Int64])

    /// Stable machine-readable code: the engine's `code()` strings
    /// (docs/contracts/m1-engine.md §4). The legacy `EngineError` variants
    /// `InvalidMnemonic` and `WalletAlreadyExists` report the `wallet.*` code
    /// of the same condition, so the UI has one code per condition.
    public var code: String {
        switch self {
        case .invalidConfig: "invalid_config"
        case .invalidArgument: "invalid_argument"
        case .networkNotOpen: "network_not_open"
        case .storageInUse: "storage_in_use"
        case .storage: "storage"
        case .walletNotFound: "wallet_not_found"
        case .invalidMnemonic: "wallet.invalid_mnemonic"
        case .walletAlreadyExists: "wallet.already_exists"
        case .wallet: "wallet"
        case .sdk: "sdk"
        case .spv: "spv"
        case .io: "io"
        case .notImplemented: "not_implemented"
        case .internal: "internal"
        case .domain(let code, _), .recipient(let code, _), .vaultAttempt(let code, _, _),
             .parameterized(let code, _):
            code
        }
    }

    /// Numbers the error carries, keyed by the engine's field names: `index`
    /// (recipient), `failed_attempts`, `retry_after_secs`, `fee`, `available`,
    /// `max_duffs`, `height`. Empty when there are none.
    public var parameters: [String: Int64] {
        switch self {
        case .recipient(_, let index):
            return ["index": Int64(index)]
        case .vaultAttempt(_, let failed, let retry):
            var values: [String: Int64] = [:]
            if let failed { values["failed_attempts"] = Int64(failed) }
            if let retry { values["retry_after_secs"] = Int64(clamping: retry) }
            return values
        case .parameterized(_, let parameters):
            return parameters
        default:
            return [:]
        }
    }

    /// Diagnostic text for logs.
    public var detail: String {
        switch self {
        case .invalidConfig(let d), .invalidArgument(let d), .networkNotOpen(let d), .storageInUse(let d),
             .storage(let d), .walletNotFound(let d), .invalidMnemonic(let d), .walletAlreadyExists(let d),
             .wallet(let d), .sdk(let d), .spv(let d), .io(let d), .notImplemented(let d), .internal(let d),
             .domain(_, let d):
            d
        case .recipient(_, let index): "recipient \(index)"
        case .vaultAttempt(_, let failed, let retry):
            "failed_attempts=\(failed.map(String.init) ?? "?") retry_after=\(retry.map(String.init) ?? "-")"
        case .parameterized(_, let parameters):
            parameters.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: " ")
        }
    }

    /// The recipient a send error concerns, if any.
    public var recipientIndex: Int? {
        if case .recipient(_, let index) = self { return index }
        return nil
    }

    /// Seconds until the vault accepts another passphrase attempt, if throttled.
    public var retryAfterSeconds: UInt64? {
        if case .vaultAttempt(_, _, let retry) = self { return retry }
        return nil
    }

    init(_ e: DashWalletCore.EngineError) {
        switch e {
        case .InvalidConfig(let d): self = .invalidConfig(detail: d)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .StorageInUse(let d): self = .storageInUse(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .InvalidMnemonic(let d): self = .invalidMnemonic(detail: d)
        case .WalletAlreadyExists(let d): self = .walletAlreadyExists(detail: d)
        case .Wallet(let d): self = .wallet(detail: d)
        case .Sdk(let d): self = .sdk(detail: d)
        case .Spv(let d): self = .spv(detail: d)
        case .Io(let d): self = .io(detail: d)
        case .NotImplemented(let d): self = .notImplemented(detail: d)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.WalletError) {
        switch e {
        case .InvalidMnemonic(let d): self = .invalidMnemonic(detail: d)
        case .UnsupportedWordCount(let n): self = .domain(code: "wallet.unsupported_word_count", detail: "\(n)")
        case .AlreadyExists(let d): self = .walletAlreadyExists(detail: d)
        case .WatchOnlyExists(let id): self = .domain(code: "wallet.watch_only_exists", detail: id)
        case .NoVault: self = .domain(code: "wallet.no_vault", detail: "")
        case .VaultLocked: self = .domain(code: "wallet.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "wallet.grant_invalid", detail: "")
        case .NameRejected(let d): self = .domain(code: "wallet.name_rejected", detail: d)
        case .InvalidXpub(let d): self = .domain(code: "wallet.invalid_xpub", detail: d)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.VaultError) {
        switch e {
        case .NoVault: self = .domain(code: "vault.no_vault", detail: "")
        case .AlreadyExists: self = .domain(code: "vault.already_exists", detail: "")
        case .Locked: self = .domain(code: "vault.locked", detail: "")
        case .WrongPassphrase(let failed, let retry):
            self = .vaultAttempt(code: "vault.wrong_passphrase", failedAttempts: failed, retryAfterSeconds: retry)
        case .Throttled(let retry):
            self = .vaultAttempt(code: "vault.throttled", failedAttempts: nil, retryAfterSeconds: retry)
        case .PassphraseRejected(let d): self = .domain(code: "vault.passphrase_rejected", detail: d)
        case .NotEncrypted: self = .domain(code: "vault.not_encrypted", detail: "")
        case .AlreadyEncrypted: self = .domain(code: "vault.already_encrypted", detail: "")
        case .GrantInvalid: self = .domain(code: "vault.grant_invalid", detail: "")
        case .GrantPurposeMismatch: self = .domain(code: "vault.grant_purpose_mismatch", detail: "")
        case .MixingOnly: self = .domain(code: "vault.mixing_only", detail: "")
        case .CredentialRequired: self = .domain(code: "vault.credential_required", detail: "")
        case .NoSecret: self = .domain(code: "vault.no_secret", detail: "")
        case .QuickUnlockUnavailable: self = .domain(code: "vault.quick_unlock_unavailable", detail: "")
        case .OsStoreUnavailable(let d): self = .domain(code: "vault.os_store_unavailable", detail: d)
        case .Corrupt(let d): self = .domain(code: "vault.corrupt", detail: d)
        case .QuickUnlockLimitExceeded(let limit):
            self = .parameterized(
                code: "vault.quick_unlock_limit_exceeded", parameters: ["limit_duffs": Int64(clamping: limit)])
        case .PassphraseStale: self = .domain(code: "vault.passphrase_stale", detail: "")
        case .NotEmpty: self = .domain(code: "vault.not_empty", detail: "")
        case .RecoveryMismatch: self = .domain(code: "vault.recovery_mismatch", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.SyncError) {
        switch e {
        case .SpvNotRunning: self = .domain(code: "sync.spv_not_running", detail: "")
        case .HeightOutOfRange(let h):
            self = .parameterized(code: "sync.height_out_of_range", parameters: ["height": Int64(h)])
        case .Spv(let d): self = .domain(code: "sync.spv", detail: d)
        case .SpvRunning: self = .domain(code: "sync.spv_running", detail: "")
        case .RescanInProgress: self = .domain(code: "sync.rescan_in_progress", detail: "")
        case .PeerNotFound(let address): self = .domain(code: "sync.peer_not_found", detail: address)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.HistoryError) {
        switch e {
        case .InvalidQuery(let d): self = .domain(code: "history.invalid_query", detail: d)
        case .StaleCursor: self = .domain(code: "history.stale_cursor", detail: "")
        case .TxNotFound(let txid): self = .domain(code: "history.tx_not_found", detail: txid)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.ReceiveError) {
        switch e {
        case .RequestNotFound(let id): self = .domain(code: "receive.request_not_found", detail: "\(id)")
        case .GapLimit: self = .domain(code: "receive.gap_limit", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.SendError) {
        switch e {
        case .NoRecipients: self = .domain(code: "send.no_recipients", detail: "")
        case .InvalidAddress(let i): self = .recipient(code: "send.invalid_address", index: Int(i))
        case .PlatformAddress(let i): self = .recipient(code: "send.platform_address", index: Int(i))
        case .InvalidAmount(let i): self = .recipient(code: "send.invalid_amount", index: Int(i))
        case .DustAmount(let i): self = .recipient(code: "send.dust_amount", index: Int(i))
        case .DuplicateAddress(let i): self = .recipient(code: "send.duplicate_address", index: Int(i))
        case .AmountExceedsBalance(let available):
            self = .parameterized(
                code: "send.amount_exceeds_balance", parameters: ["available": Int64(clamping: available)])
        case .AmountWithFeeExceedsBalance(let fee, let available):
            self = .parameterized(
                code: "send.amount_with_fee_exceeds_balance",
                parameters: ["fee": Int64(clamping: fee), "available": Int64(clamping: available)])
        case .InsufficientMixedFunds(let available):
            self = .parameterized(
                code: "send.insufficient_mixed_funds", parameters: ["available": Int64(clamping: available)])
        case .OutpointUnavailable(let o):
            self = .domain(code: "send.outpoint_unavailable", detail: "\(o.txid):\(o.vout)")
        case .AbsurdFee(let fee):
            self = .parameterized(code: "send.absurd_fee", parameters: ["fee": Int64(clamping: fee)])
        case .TxTooLarge: self = .domain(code: "send.tx_too_large", detail: "")
        case .InvalidChangeAddress: self = .domain(code: "send.invalid_change_address", detail: "")
        case .WatchOnly: self = .domain(code: "send.watch_only", detail: "")
        case .VaultLocked: self = .domain(code: "send.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "send.grant_invalid", detail: "")
        case .GrantExceeded(let max):
            self = .parameterized(code: "send.grant_exceeded", parameters: ["max_duffs": Int64(clamping: max)])
        case .PreparedTxSpent: self = .domain(code: "send.prepared_tx_spent", detail: "")
        case .NoPeers: self = .domain(code: "send.no_peers", detail: "")
        case .BroadcastRejected(let reason): self = .domain(code: "send.broadcast_rejected", detail: reason)
        case .AmountTooSmallAfterFee(let i): self = .recipient(code: "send.amount_too_small_after_fee", index: Int(i))
        case .BroadcastUnknown(let reason): self = .domain(code: "send.broadcast_unknown", detail: reason)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.CoinsError) {
        switch e {
        case .OutpointNotFound(let o): self = .domain(code: "coins.outpoint_not_found", detail: "\(o.txid):\(o.vout)")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.LabelsError) {
        switch e {
        case .InvalidAddress: self = .domain(code: "labels.invalid_address", detail: "")
        case .DuplicateAddress: self = .domain(code: "labels.duplicate_address", detail: "")
        case .OwnAddress: self = .domain(code: "labels.own_address", detail: "")
        case .EntryNotFound: self = .domain(code: "labels.entry_not_found", detail: "")
        case .ReceiveEntryNotDeletable: self = .domain(code: "labels.receive_entry_not_deletable", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.MessageError) {
        switch e {
        case .InvalidAddress: self = .domain(code: "message.invalid_address", detail: "")
        case .AddressNoKey: self = .domain(code: "message.address_no_key", detail: "")
        case .MalformedSignature: self = .domain(code: "message.malformed_signature", detail: "")
        case .PubkeyNotRecovered: self = .domain(code: "message.pubkey_not_recovered", detail: "")
        case .NotSigned: self = .domain(code: "message.not_signed", detail: "")
        case .AddressNotMine: self = .domain(code: "message.address_not_mine", detail: "")
        case .WatchOnly: self = .domain(code: "message.watch_only", detail: "")
        case .VaultLocked: self = .domain(code: "message.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "message.grant_invalid", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.UriError) {
        switch e {
        case .DoubleSlash: self = .domain(code: "uri.double_slash", detail: "")
        case .NotDashUri: self = .domain(code: "uri.not_dash_uri", detail: "")
        case .Unparsable: self = .domain(code: "uri.unparsable", detail: "")
        case .Bip70Unsupported: self = .domain(code: "uri.bip70_unsupported", detail: "")
        case .InvalidAddress(let problem): self = .domain(code: "uri.invalid_address", detail: "\(problem)")
        case .InvalidAmount: self = .domain(code: "uri.invalid_amount", detail: "")
        case .TooLongForQr: self = .domain(code: "uri.too_long_for_qr", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        }
    }

    init(_ e: DashWalletCore.UnitsError) {
        switch e {
        case .Unparsable: self = .domain(code: "units.unparsable", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        }
    }

    /// Maps any error thrown by a generated binding call.
    static func from(_ error: any Error) -> DashKitError {
        switch error {
        case let e as DashKitError: e
        case let e as DashWalletCore.EngineError: DashKitError(e)
        case let e as DashWalletCore.WalletError: DashKitError(e)
        case let e as DashWalletCore.VaultError: DashKitError(e)
        case let e as DashWalletCore.SyncError: DashKitError(e)
        case let e as DashWalletCore.HistoryError: DashKitError(e)
        case let e as DashWalletCore.ReceiveError: DashKitError(e)
        case let e as DashWalletCore.SendError: DashKitError(e)
        case let e as DashWalletCore.CoinsError: DashKitError(e)
        case let e as DashWalletCore.LabelsError: DashKitError(e)
        case let e as DashWalletCore.MessageError: DashKitError(e)
        case let e as DashWalletCore.UriError: DashKitError(e)
        case let e as DashWalletCore.UnitsError: DashKitError(e)
        case let e as DashWalletCore.TxActionError: DashKitError(e)
        case let e as DashWalletCore.ConsoleError: DashKitError(e)
        case let e as DashWalletCore.CompatError: DashKitError(e)
        case let e as DashWalletCore.BackupError: DashKitError(e)
        case let e as DashWalletCore.PsbtError: DashKitError(e)
        case let e as DashWalletCore.DesktopError: DashKitError(e)
        default: .internal(detail: String(describing: error))
        }
    }
}

/// Runs a throwing binding call and maps its error to `DashKitError`.
@inline(__always)
func mapped<T>(_ body: () throws -> T) throws(DashKitError) -> T {
    do { return try body() } catch { throw DashKitError.from(error) }
}

/// Async variant of `mapped`.
@inline(__always)
func mapped<T>(_ body: () async throws -> T) async throws(DashKitError) -> T {
    do { return try await body() } catch { throw DashKitError.from(error) }
}
