import DashWalletCore
import Foundation

// Maps the M3 FFI error domains (docs/contracts/m3-engine.md §4) to
// `DashKitError` with the engine's codes. Numbers the UI shows go into
// `parameters` (review M-5 rule).
extension DashKitError {
    init(_ e: DashWalletCore.CoinJoinError) {
        switch e {
        case .Disabled: self = .domain(code: "coinjoin.disabled", detail: "")
        case .WatchOnly: self = .domain(code: "coinjoin.watch_only", detail: "")
        case .InsufficientFunds(let min):
            self = .parameterized(code: "coinjoin.insufficient_funds", parameters: ["min_duffs": Int64(clamping: min)])
        case .VaultLocked: self = .domain(code: "coinjoin.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "coinjoin.grant_invalid", detail: "")
        case .NothingToMove: self = .domain(code: "coinjoin.nothing_to_move", detail: "")
        case .SpvNotRunning: self = .domain(code: "coinjoin.spv_not_running", detail: "")
        case .NoPeers: self = .domain(code: "coinjoin.no_peers", detail: "")
        case .BroadcastRejected(let reason): self = .domain(code: "coinjoin.broadcast_rejected", detail: reason)
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }

    init(_ e: DashWalletCore.MasternodeError) {
        switch e {
        case .WatchOnly: self = .domain(code: "masternode.watch_only", detail: "")
        case .VaultLocked: self = .domain(code: "masternode.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "masternode.grant_invalid", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }
}
