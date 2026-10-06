import DashWalletCore
import Foundation

// Maps the M2 FFI error domains (docs/contracts/m2-engine.md §4) to
// `DashKitError` with the engine's codes. Numbers the UI shows go into
// `parameters` (review M-5 rule).
extension DashKitError {
    init(_ e: DashWalletCore.TxActionError) {
        switch e {
        case .TxNotFound(let txid): self = .domain(code: "tx_action.tx_not_found", detail: txid)
        case .Refused(let refusal):
            self = .parameterized(code: "tx_action.refused", parameters: ["refusal": refusal.index])
        case .SpvNotRunning: self = .domain(code: "tx_action.spv_not_running", detail: "")
        case .NoPeers: self = .domain(code: "tx_action.no_peers", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.ConsoleError) {
        switch e {
        case .ParseError(let d): self = .domain(code: "console.parse_error", detail: d)
        // Core's own "message (code N)" line; the console prints it as RPC output.
        case .RpcError(let code, let message):
            self = .domain(code: "console.rpc_error", detail: "\(message) (code \(code))")
        case .NotAvailable(let command): self = .domain(code: "console.not_available", detail: command)
        // The console adapter turns this case into `ConsoleResult.authorizationRequired`
        // before it is mapped; reaching here means it was not handled.
        case .AuthorizationRequired: self = .domain(code: "console.authorization_required", detail: "")
        case .WalletRequired: self = .domain(code: "console.wallet_required", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.CompatError) {
        switch e {
        case .FileUnreadable(let d): self = .domain(code: "compat.file_unreadable", detail: d)
        case .UnsupportedFormat(let d): self = .domain(code: "compat.unsupported_format", detail: d)
        case .Corrupt(let d): self = .domain(code: "compat.corrupt", detail: d)
        case .PassphraseRequired: self = .domain(code: "compat.passphrase_required", detail: "")
        case .WrongPassphrase: self = .domain(code: "compat.wrong_passphrase", detail: "")
        case .NoHdChain: self = .domain(code: "compat.no_hd_chain", detail: "")
        case .NetworkMismatch(let found):
            self = .domain(code: "compat.network_mismatch", detail: String(describing: found))
        case .InvalidKeyMaterial(let d): self = .domain(code: "compat.invalid_key_material", detail: d)
        case .AlreadyExists(let id): self = .domain(code: "compat.already_exists", detail: id)
        case .NoVault: self = .domain(code: "compat.no_vault", detail: "")
        case .VaultLocked: self = .domain(code: "compat.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "compat.grant_invalid", detail: "")
        case .WatchOnly: self = .domain(code: "compat.watch_only", detail: "")
        case .DestinationUnwritable(let d): self = .domain(code: "compat.destination_unwritable", detail: d)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.BackupError) {
        switch e {
        case .VaultLocked: self = .domain(code: "backup.vault_locked", detail: "")
        case .PassphraseRequired: self = .domain(code: "backup.passphrase_required", detail: "")
        case .WrongPassphrase: self = .domain(code: "backup.wrong_passphrase", detail: "")
        case .Corrupt(let d): self = .domain(code: "backup.corrupt", detail: d)
        case .UnsupportedVersion(let version):
            self = .parameterized(code: "backup.unsupported_version", parameters: ["version": Int64(version)])
        case .NetworkMismatch: self = .domain(code: "backup.network_mismatch", detail: "")
        case .AlreadyExists(let id): self = .domain(code: "backup.already_exists", detail: id)
        case .DestinationUnwritable(let d): self = .domain(code: "backup.destination_unwritable", detail: d)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.PsbtError) {
        switch e {
        case .Invalid(let d): self = .domain(code: "psbt.invalid", detail: d)
        case .TooLarge(let size):
            self = .parameterized(code: "psbt.too_large", parameters: ["size_bytes": Int64(clamping: size)])
        case .NetworkMismatch: self = .domain(code: "psbt.network_mismatch", detail: "")
        case .NotComplete: self = .domain(code: "psbt.not_complete", detail: "")
        case .FeeRateTooHigh(let rate):
            self = .parameterized(code: "psbt.fee_rate_too_high", parameters: ["duffs_per_kb": Int64(clamping: rate)])
        case .WatchOnly: self = .domain(code: "psbt.watch_only", detail: "")
        case .VaultLocked: self = .domain(code: "psbt.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "psbt.grant_invalid", detail: "")
        case .GrantExceeded(let max):
            self = .parameterized(code: "psbt.grant_exceeded", parameters: ["max_duffs": Int64(clamping: max)])
        case .NoPeers: self = .domain(code: "psbt.no_peers", detail: "")
        case .BroadcastRejected(let reason): self = .domain(code: "psbt.broadcast_rejected", detail: reason)
        case .BroadcastUnknown(let reason): self = .domain(code: "psbt.broadcast_unknown", detail: reason)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.DesktopError) {
        switch e {
        case .Unsupported(let feature): self = .domain(code: "desktop.unsupported", detail: feature)
        case .OsError(let d): self = .domain(code: "desktop.os_error", detail: d)
        case .NoQrCode: self = .domain(code: "desktop.no_qr_code", detail: "")
        case .ImageUnreadable(let d): self = .domain(code: "desktop.image_unreadable", detail: d)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }
}

extension DashWalletCore.TxActionRefusal {
    /// Position in the engine enum; WalletRuntime's
    /// `TransactionActionRefusal.rawValue` uses the same order.
    var index: Int64 {
        switch self {
        case .confirmed: 0
        case .instantLocked: 1
        case .alreadyAbandoned: 2
        case .coinbase: 3
        case .inMempool: 4
        case .notSentByWallet: 5
        }
    }
}
