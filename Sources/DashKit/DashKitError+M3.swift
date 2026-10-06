import DashWalletCore
import Foundation

// Maps the M3 FFI error domains (docs/contracts/m3-engine.md §4) to
// `DashKitError` with the engine's codes. Numbers the UI shows go into
// `parameters` (review M-5 rule); enum parameters are their position in the
// engine enum, which the WalletRuntime enums of the same name follow.
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

    init(_ e: DashWalletCore.GovernanceError) {
        switch e {
        case .SyncDisabled: self = .domain(code: "governance.sync_disabled", detail: "")
        case .NotSynced: self = .domain(code: "governance.not_synced", detail: "")
        case .ProposalNotFound(let hash): self = .domain(code: "governance.proposal_not_found", detail: hash)
        case .InvalidProposal(let field):
            self = .parameterized(code: "governance.invalid_proposal", parameters: ["field": field.index])
        case .NoVotingKeys: self = .domain(code: "governance.no_voting_keys", detail: "")
        case .VoteTooOften(let secs):
            self = .parameterized(
                code: "governance.vote_too_often", parameters: ["retry_after_secs": Int64(clamping: secs)])
        case .InsufficientFunds(let needed, let available):
            self = .parameterized(
                code: "governance.insufficient_funds",
                parameters: ["needed": Int64(clamping: needed), "available": Int64(clamping: available)])
        case .CollateralUnconfirmed(let confirmations):
            self = .parameterized(
                code: "governance.collateral_unconfirmed", parameters: ["confirmations": Int64(confirmations)])
        case .ProposalExpired: self = .domain(code: "governance.proposal_expired", detail: "")
        case .WatchOnly: self = .domain(code: "governance.watch_only", detail: "")
        case .VaultLocked: self = .domain(code: "governance.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "governance.grant_invalid", detail: "")
        case .NoPeers: self = .domain(code: "governance.no_peers", detail: "")
        case .BroadcastRejected(let reason): self = .domain(code: "governance.broadcast_rejected", detail: reason)
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
        case .ListUnavailable: self = .domain(code: "masternode.list_unavailable", detail: "")
        case .NotFound(let hash): self = .domain(code: "masternode.not_found", detail: hash)
        case .KeyNotInWallet(let role):
            self = .parameterized(code: "masternode.key_not_in_wallet", parameters: ["role": role.index])
        case .InvalidService(let d): self = .domain(code: "masternode.invalid_service", detail: d)
        case .InvalidKey(let role, _):
            self = .parameterized(code: "masternode.invalid_key", parameters: ["role": role.index])
        case .InvalidPayout(let d): self = .domain(code: "masternode.invalid_payout", detail: d)
        case .DuplicateAddress(let d): self = .domain(code: "masternode.duplicate_address", detail: d)
        case .CollateralUnavailable(let refusal):
            self = .parameterized(code: "masternode.collateral_unavailable", parameters: ["refusal": refusal.index])
        case .InsufficientFunds(let needed, let available):
            self = .parameterized(
                code: "masternode.insufficient_funds",
                parameters: ["needed": Int64(clamping: needed), "available": Int64(clamping: available)])
        case .OperatorSecretMismatch: self = .domain(code: "masternode.operator_secret_mismatch", detail: "")
        case .OperatorSecretUnconfirmed: self = .domain(code: "masternode.operator_secret_unconfirmed", detail: "")
        case .CollateralSignatureInvalid: self = .domain(code: "masternode.collateral_signature_invalid", detail: "")
        case .UnsupportedEntry(let d): self = .domain(code: "masternode.unsupported_entry", detail: d)
        case .WatchOnly: self = .domain(code: "masternode.watch_only", detail: "")
        case .VaultLocked: self = .domain(code: "masternode.vault_locked", detail: "")
        case .GrantInvalid: self = .domain(code: "masternode.grant_invalid", detail: "")
        case .NoPeers: self = .domain(code: "masternode.no_peers", detail: "")
        case .BroadcastRejected(let reason): self = .domain(code: "masternode.broadcast_rejected", detail: reason)
        case .SharedEnvelopeInvalid(let d): self = .domain(code: "masternode.shared_envelope_invalid", detail: d)
        case .SharedEnvelopeTooLarge(let size):
            self = .parameterized(
                code: "masternode.shared_envelope_too_large", parameters: ["size_bytes": Int64(clamping: size)])
        case .SharedNetworkMismatch: self = .domain(code: "masternode.shared_network_mismatch", detail: "")
        case .SharedSessionNotFound(let id): self = .domain(code: "masternode.shared_session_not_found", detail: id)
        case .SharedInputsRefused(let d): self = .domain(code: "masternode.shared_inputs_refused", detail: d)
        case .SharedCoinSpent(let outpoint):
            self = .domain(code: "masternode.shared_coin_spent", detail: "\(outpoint.txid)-\(outpoint.vout)")
        case .AlreadyTracked(let hash): self = .domain(code: "masternode.already_tracked", detail: hash)
        case .PlatformUnavailable: self = .domain(code: "masternode.platform_unavailable", detail: "")
        case .InvalidArgument(let d): self = .invalidArgument(detail: d)
        case .NetworkNotOpen(let d): self = .networkNotOpen(detail: d)
        case .WalletNotFound(let d): self = .walletNotFound(detail: d)
        case .Storage(let d): self = .storage(detail: d)
        case .NotImplemented(let call): self = .notImplemented(detail: call)
        case .Internal(let d): self = .internal(detail: d)
        }
    }
}

extension DashWalletCore.ProposalField {
    /// Position in the engine enum (WalletRuntime `ProposalField.allCases`).
    var index: Int64 {
        switch self {
        case .name: 0
        case .url: 1
        case .paymentAddress: 2
        case .paymentAmount: 3
        case .paymentCount: 4
        case .firstPayment: 5
        case .payload: 6
        }
    }
}

extension DashWalletCore.MasternodeKeyRole {
    /// Position in the engine enum (WalletRuntime `MasternodeKeyRole.allCases`).
    var index: Int64 {
        switch self {
        case .owner: 0
        case .voting: 1
        case .operator: 2
        case .platformNode: 3
        case .ownerPayout: 4
        case .operatorPayout: 5
        }
    }
}

extension DashWalletCore.CollateralRefusal {
    /// Position in the engine enum (WalletRuntime `CollateralRefusal.allCases`).
    var index: Int64 {
        switch self {
        case .wrongAmount: 0
        case .unconfirmed: 1
        case .notP2pkh: 2
        case .locked: 3
        case .alreadyCollateral: 4
        case .notFound: 5
        }
    }
}
