import Foundation
import WalletRuntime

extension L10n {
    /// Copy for the M3 error codes (m3-engine.md §4), chosen by code and the
    /// numeric `parameters`, never from `ServiceError.detail`. The only
    /// exception is a Core reject reason, which is matched against dash-qt's
    /// table of known reasons (`rejectExplanation`), never shown raw.
    public enum M3Errors {
        public static let coinJoinDisabled = "CoinJoin is disabled in the options."
        public static let coinJoinWatchOnly = "Watch-only wallets cannot mix."
        public static func coinJoinMinimum(_ amount: String) -> String { "CoinJoin requires at least \(amount) to use." }
        public static let coinJoinNothingToMove = "There are no mixed coins to move."
        public static let spvNotRunning = "Sync is not running. Start it and try again."
        public static let noPeers = "No peers are connected. Try again when the wallet is connected to the network."
        public static let broadcastRejected = "The network rejected the transaction."
        public static let grantInvalid = "The authorization expired. Please try again."
        public static let watchOnly = "Watch-only wallets have no private keys."

        public static let governanceSyncDisabled = "Governance data is not being downloaded."
        public static let governanceNotSynced = "Waiting for governance sync…"
        public static let proposalNotFound = "The proposal could not be found."
        public static func invalidProposal(_ field: ProposalField?) -> String {
            guard let field else { return "The proposal is not valid." }
            return "The proposal is not valid: \(L10n.Governance.fieldError(field))"
        }
        public static let noVotingKeys = "No masternode voting keys found in wallet."
        public static let voteTooOften = "Masternode voting too often. Try again later."
        public static func insufficientFunds(needed: String, available: String) -> String {
            "Insufficient funds: \(needed) needed, \(available) available."
        }
        public static let insufficientFundsShort = "Insufficient funds."
        public static func collateralUnconfirmed(_ confirmations: Int64?) -> String {
            "The collateral transaction needs at least 1 confirmation (it has \(confirmations ?? 0))."
        }
        public static let proposalExpired = "This proposal has expired."

        public static let listUnavailable = "The masternode list is not available yet."
        public static let masternodeNotFound = "Masternode not found"
        public static func keyNotInWallet(_ role: MasternodeKeyRole?) -> String {
            guard let role else { return "The required key is not in this wallet." }
            return "This wallet does not hold the \(L10n.Masternodes.roleName(role).lowercased()) key."
        }
        public static let invalidService = "Enter valid service addresses (IP:port), separated by commas or spaces."
        public static func invalidKey(_ role: MasternodeKeyRole?) -> String {
            guard let role else { return "The key is not valid." }
            return "The \(L10n.Masternodes.roleName(role).lowercased()) key is not valid."
        }
        public static let invalidPayout = "Enter a valid payout address (P2PKH or P2SH)."
        public static let duplicateAddress =
            "The owner, voting, payout and collateral addresses must be different where required."
        public static func collateralUnavailable(_ refusal: CollateralRefusal?) -> String {
            guard let refusal else { return "The collateral cannot be used." }
            return L10n.Masternodes.refusal(refusal)
        }
        public static let operatorSecretMismatch = "The operator secret key does not match this masternode."
        public static let operatorSecretUnconfirmed =
            "Confirm you saved the operator secret key by typing its last 4 characters."
        public static let collateralSignatureInvalid =
            "The signature does not match the collateral key. Sign the message above with the collateral address."
        public static let unsupportedEntry = "This masternode cannot be changed with this transaction."
        public static let sharedEnvelopeInvalid = "This is not a valid shared masternode message."
        public static func sharedEnvelopeTooLarge(_ bytes: Int64?) -> String {
            "The message is too large (\(bytes.map { "\($0) bytes" } ?? "over 2 MiB"); the limit is 2 MiB)."
        }
        public static let sharedNetworkMismatch = "This message belongs to another network."
        public static let sharedSessionNotFound = "This message is about a session that is not on this computer."
        public static let sharedInputsRefused =
            "The transaction asks this wallet to sign inputs it did not offer or pays it less than agreed."
        public static let sharedCoinSpent = "A coin reserved for this session was spent elsewhere."
        public static let alreadyTracked = "This masternode is already tracked."
        public static let platformUnavailable = "Dash Platform could not be reached."

        /// dash-qt's explanations of Core reject reasons
        /// (`MasternodeOperationRunner`), keyed by the reason's first word.
        static let rejectExplanations: [String: String] = [
            "bad-protx-dup-key":
                "One of the chosen keys is already in use by a registered masternode or share. Every owner and voting key may be used only once network-wide.",
            "bad-protx-dup-addr": "The chosen service address is already in use by a registered masternode.",
            "bad-protx-shares-payee-reuse":
                "A refund or reward address may not double as a share owner or voting address. Use distinct addresses for payouts and keys.",
            "bad-protx-shares-sig":
                "A participant's consent signature does not match the final transaction. This happens when the terms or the funding transaction changed after signing; collect fresh signatures.",
            "bad-protx-version":
                "The network does not accept this transaction version yet. Wait for the network upgrade that introduces it to activate.",
            "too-early":
                "This feature is not active on the network yet. Wait for the network upgrade that introduces it to activate.",
            "bad-prodis-dup": "A dissolution for this masternode is already pending.",
        ]

        /// The explanation of a known Core reject reason, or `nil`.
        public static func rejectExplanation(_ reason: String) -> String? {
            let key = reason.split(whereSeparator: { $0 == " " || $0 == ":" || $0 == "," }).first.map(String.init) ?? ""
            return rejectExplanations[key]
        }
    }
}

extension ErrorText {
    /// Copy for an M3 error (any domain), falling back to the M2 and common
    /// tables. `amount` formats the duff parameters in the display unit.
    public static func m3(_ error: ServiceError, amount: (Amount) -> String) -> String {
        let p = error.parameters
        func duffs(_ key: String) -> String? { p[key].map { amount(Amount(duffs: $0)) } }
        func index<T: CaseIterable>(_ key: String, _ type: T.Type) -> T? where T.AllCases: RandomAccessCollection,
            T.AllCases.Index == Int
        {
            guard let raw = p[key], raw >= 0, raw < Int64(T.allCases.count) else { return nil }
            return T.allCases[Int(raw)]
        }
        typealias E = L10n.M3Errors
        switch error.code {
        case .coinjoinDisabled: return E.coinJoinDisabled
        case .coinjoinWatchOnly: return E.coinJoinWatchOnly
        case .coinjoinInsufficientFunds:
            return E.coinJoinMinimum(duffs("min_duffs") ?? amount(M3Defaults.coinJoinLimits.minimumMixingBalance))
        case .coinjoinVaultLocked, .governanceVaultLocked, .masternodeVaultLocked: return L10n.Common.vaultLocked
        case .coinjoinGrantInvalid, .governanceGrantInvalid, .masternodeGrantInvalid: return E.grantInvalid
        case .coinjoinNothingToMove: return E.coinJoinNothingToMove
        case .coinjoinSpvNotRunning: return E.spvNotRunning
        case .coinjoinNoPeers, .governanceNoPeers, .masternodeNoPeers: return E.noPeers
        case .coinjoinBroadcastRejected, .governanceBroadcastRejected, .masternodeBroadcastRejected:
            return E.rejectExplanation(error.detail).map { "\(E.broadcastRejected) \($0)" } ?? E.broadcastRejected
        case .governanceSyncDisabled: return E.governanceSyncDisabled
        case .governanceNotSynced: return E.governanceNotSynced
        case .governanceProposalNotFound: return E.proposalNotFound
        case .governanceInvalidProposal: return E.invalidProposal(index("field", ProposalField.self))
        case .governanceNoVotingKeys: return E.noVotingKeys
        case .governanceVoteTooOften: return E.voteTooOften
        case .governanceInsufficientFunds, .masternodeInsufficientFunds:
            if let needed = duffs("needed"), let available = duffs("available") {
                return E.insufficientFunds(needed: needed, available: available)
            }
            return E.insufficientFundsShort
        case .governanceCollateralUnconfirmed: return E.collateralUnconfirmed(p["confirmations"])
        case .governanceProposalExpired: return E.proposalExpired
        case .governanceWatchOnly, .masternodeWatchOnly: return E.watchOnly
        case .masternodeListUnavailable: return E.listUnavailable
        case .masternodeNotFound: return E.masternodeNotFound
        case .masternodeKeyNotInWallet: return E.keyNotInWallet(index("role", MasternodeKeyRole.self))
        case .masternodeInvalidService: return E.invalidService
        case .masternodeInvalidKey: return E.invalidKey(index("role", MasternodeKeyRole.self))
        case .masternodeInvalidPayout: return E.invalidPayout
        case .masternodeDuplicateAddress: return E.duplicateAddress
        case .masternodeCollateralUnavailable:
            return E.collateralUnavailable(index("refusal", CollateralRefusal.self))
        case .masternodeOperatorSecretMismatch: return E.operatorSecretMismatch
        case .masternodeOperatorSecretUnconfirmed: return E.operatorSecretUnconfirmed
        case .masternodeCollateralSignatureInvalid: return E.collateralSignatureInvalid
        case .masternodeUnsupportedEntry: return E.unsupportedEntry
        case .masternodeSharedEnvelopeInvalid: return E.sharedEnvelopeInvalid
        case .masternodeSharedEnvelopeTooLarge: return E.sharedEnvelopeTooLarge(p["size_bytes"])
        case .masternodeSharedNetworkMismatch: return E.sharedNetworkMismatch
        case .masternodeSharedSessionNotFound: return E.sharedSessionNotFound
        case .masternodeSharedInputsRefused: return E.sharedInputsRefused
        case .masternodeSharedCoinSpent: return E.sharedCoinSpent
        case .masternodeAlreadyTracked: return E.alreadyTracked
        case .masternodePlatformUnavailable: return E.platformUnavailable
        default: return m2(error.code)
        }
    }
}
