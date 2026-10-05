import DashWalletCore
import Foundation

/// Errors from the engine. `detail` is diagnostic text for logs; UI copy is
/// chosen from the case (or `code`).
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
    case notImplemented(detail: String)
    case `internal`(detail: String)
    /// An M1 domain error DashKit has no dedicated case for yet; `code` is the
    /// engine's stable code (docs/contracts/m1-engine.md).
    case domain(code: String, detail: String)

    /// Stable machine-readable code (matches dw-engine `EngineError::code`).
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
        case .domain(let code, _): code
        }
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
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    /// Maps any error thrown by a generated binding call.
    static func from(_ error: any Error) -> DashKitError {
        if let e = error as? DashWalletCore.EngineError { return DashKitError(e) }
        if let e = error as? DashWalletCore.WalletError { return DashKitError(e) }
        if let e = error as? DashKitError { return e }
        return .internal(detail: String(describing: error))
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
