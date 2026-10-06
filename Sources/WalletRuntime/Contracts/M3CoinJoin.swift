// M3 service contracts: CoinJoin mixing, its options, and IOS-057 "move
// mixed coins" (engine coinjoin.rs; m3-swift.md §2.1, m3-engine.md §2.1).
// Owner of the adapter: R1. The CoinJoin send page stays
// `TransactionSending` with `CoinSourceChoice.fullyMixed` (M1).
import Foundation

/// Options → CoinJoin (QT-046). Global for the network; mixing state is per
/// wallet (QT-049). The UI-only options (advanced interface, low-keys
/// warning, popups) are host settings, as dash-qt keeps them in QSettings.
public struct CoinJoinSettings: Sendable, Hashable {
    public var enabled: Bool
    public var multiSession: Bool
    public var maxSessions: Int
    public var rounds: Int
    /// Whole DASH (`coinjoinamount`).
    public var targetAmountDash: Int
    public var denomsGoal: Int
    public var denomsHardCap: Int

    public init(
        enabled: Bool, multiSession: Bool, maxSessions: Int, rounds: Int, targetAmountDash: Int, denomsGoal: Int,
        denomsHardCap: Int
    ) {
        self.enabled = enabled
        self.multiSession = multiSession
        self.maxSessions = maxSessions
        self.rounds = rounds
        self.targetAmountDash = targetAmountDash
        self.denomsGoal = denomsGoal
        self.denomsHardCap = denomsHardCap
    }

    /// dash-qt's defaults (Dash Core `coinjoin/options.h`).
    public static let dashQtDefaults = CoinJoinSettings(
        enabled: true, multiSession: false, maxSessions: 4, rounds: 4, targetAmountDash: 1000, denomsGoal: 50,
        denomsHardCap: 300)
}

/// Fixed CoinJoin values (engine `coinjoin_limits`).
public struct CoinJoinLimits: Sendable, Hashable {
    /// Largest first.
    public let denominations: [Amount]
    /// 0.00140001 DASH ("CoinJoin requires at least %2 to use.").
    public let minimumMixingBalance: Amount
    public let rounds: ClosedRange<Int>
    public let sessions: ClosedRange<Int>
    public let targetAmountDash: ClosedRange<Int>
    public let denoms: ClosedRange<Int>
    public let defaults: CoinJoinSettings

    public init(
        denominations: [Amount], minimumMixingBalance: Amount, rounds: ClosedRange<Int>, sessions: ClosedRange<Int>,
        targetAmountDash: ClosedRange<Int>, denoms: ClosedRange<Int>, defaults: CoinJoinSettings
    ) {
        self.denominations = denominations
        self.minimumMixingBalance = minimumMixingBalance
        self.rounds = rounds
        self.sessions = sessions
        self.targetAmountDash = targetAmountDash
        self.denoms = denoms
        self.defaults = defaults
    }
}

public enum CoinJoinState: Sendable, Hashable {
    case idle
    case mixing
    case stopping
}

/// Why mixing stopped without the user asking.
public enum CoinJoinStopReason: Sendable, Hashable {
    case userRequested
    /// The vault locked while mixing; with a cancelled unlock prompt the
    /// panel says "Wallet is locked and user declined to unlock. Disabling
    /// CoinJoin."
    case vaultLocked
    case walletUnloaded
    case sessionClosed
    case disabled
}

/// Why "Start CoinJoin" is not offered ("(Disabled)"). A locked vault is not
/// here: the view model asks to unlock for mixing only.
public enum CoinJoinUnavailable: Sendable, Hashable {
    case disabled
    case watchOnly
    case insufficientFunds(minimum: Amount)
}

/// Core `PoolState`.
public enum CoinJoinPoolState: Sendable, Hashable {
    case idle
    case queue
    case acceptingEntries
    case signing
    case error
}

/// Core `PoolMessage` (a masternode's reply); the view model holds Core's
/// English texts (research 02 §9.4).
public enum CoinJoinPoolMessage: Sendable, Hashable, CaseIterable {
    case alreadyHave, denom, entriesFull, existingTx, fees, invalidCollateral, invalidInput, invalidScript, invalidTx
    case maximum, mnList, mode, nonStandardPubkey, notAMasternode, queueFull, recent, session, missingTx, version
    case noError, success, entriesAdded, sizeMismatch
}

/// `coinjoin status` (Core `strAutoDenomResult`), QT-050.
public enum CoinJoinStatusCode: Sendable, Hashable {
    case idle, syncInProgress, walletLocked, mixingInProgress, noMasternodes, notEnoughFunds, unconfirmedDenominated
    case noCompatibleMasternode, noCompatibleInputs, tryingToConnect, noQueueToJoin, noRandomMasternode
    case failedToStartQueue, waitingInQueue, signing
    /// "Masternode: <message>".
    case masternode(CoinJoinPoolMessage)
}

/// Balances of dash-qt's panel and progress formula.
public struct CoinJoinBalances: Sendable, Hashable {
    public let anonymizable: Amount
    public let denominated: Amount
    public let normalizedAnonymized: Amount
    /// "CoinJoin Balance"; what the CoinJoin send page may spend (QT-043).
    public let fullyMixed: Amount

    public init(anonymizable: Amount, denominated: Amount, normalizedAnonymized: Amount, fullyMixed: Amount) {
        self.anonymizable = anonymizable
        self.denominated = denominated
        self.normalizedAnonymized = normalizedAnonymized
        self.fullyMixed = fullyMixed
    }
}

/// dash-qt's progress (QT-042), each 0–100, and the tooltip's average.
public struct CoinJoinProgress: Sendable, Hashable {
    public let overall: Double
    public let denominated: Double
    public let partiallyMixed: Double
    public let mixed: Double
    public let averageRounds: Double

    public init(overall: Double, denominated: Double, partiallyMixed: Double, mixed: Double, averageRounds: Double) {
        self.overall = overall
        self.denominated = denominated
        self.partiallyMixed = partiallyMixed
        self.mixed = mixed
        self.averageRounds = averageRounds
    }
}

/// "Amount and Rounds" (`~amount` in red when `insufficientInputs`).
public struct CoinJoinAmountAndRounds: Sendable, Hashable {
    public let amount: Amount
    public let rounds: Int
    public let insufficientInputs: Bool

    public init(amount: Amount, rounds: Int, insufficientInputs: Bool) {
        self.amount = amount
        self.rounds = rounds
        self.insufficientInputs = insufficientInputs
    }
}

/// One open mixing session.
public struct CoinJoinSessionInfo: Sendable, Hashable {
    public let proTxHash: String?
    public let service: String?
    public let denomination: Amount?
    public let state: CoinJoinPoolState
    public let entries: Int
    public let lastMessage: CoinJoinPoolMessage?

    public init(
        proTxHash: String?, service: String?, denomination: Amount?, state: CoinJoinPoolState, entries: Int,
        lastMessage: CoinJoinPoolMessage?
    ) {
        self.proTxHash = proTxHash
        self.service = service
        self.denomination = denomination
        self.state = state
        self.entries = entries
        self.lastMessage = lastMessage
    }
}

/// Everything the Overview CoinJoin panel shows for one wallet (QT-041…050).
public struct CoinJoinStatus: Sendable, Hashable {
    public let wallet: WalletID
    public let state: CoinJoinState
    public let stopReason: CoinJoinStopReason?
    public let unavailable: CoinJoinUnavailable?
    public let balances: CoinJoinBalances
    public let progress: CoinJoinProgress
    public let amountAndRounds: CoinJoinAmountAndRounds
    /// "Submitted Denom" (advanced mode); empty = "n/a".
    public let submittedDenominations: [Amount]
    public let sessions: [CoinJoinSessionInfo]
    public let status: CoinJoinStatusCode
    public let queueSize: Int
    /// `nil` for HD wallets (no keypool): no "keys left" text.
    public let keysLeft: Int?

    public init(
        wallet: WalletID, state: CoinJoinState, stopReason: CoinJoinStopReason?, unavailable: CoinJoinUnavailable?,
        balances: CoinJoinBalances, progress: CoinJoinProgress, amountAndRounds: CoinJoinAmountAndRounds,
        submittedDenominations: [Amount], sessions: [CoinJoinSessionInfo], status: CoinJoinStatusCode,
        queueSize: Int, keysLeft: Int?
    ) {
        self.wallet = wallet
        self.state = state
        self.stopReason = stopReason
        self.unavailable = unavailable
        self.balances = balances
        self.progress = progress
        self.amountAndRounds = amountAndRounds
        self.submittedDenominations = submittedDenominations
        self.sessions = sessions
        self.status = status
        self.queueSize = queueSize
        self.keysLeft = keysLeft
    }
}

/// Mixing control (QT-041…050, QT-112). Errors: `coinjoin.*`.
public protocol CoinJoinControlling: AnyObject, Sendable {
    func limits() -> CoinJoinLimits
    func settings() async throws(ServiceError) -> CoinJoinSettings
    /// Applied live; `invalid_argument` outside `limits()`.
    func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError)
    func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus
    /// Wallets whose status changed (engine `CoinJoin`, ≤ 1 Hz each).
    func statusChanges() -> AsyncStream<WalletID>
    /// `coinjoin.vault_locked`: ask to unlock for mixing only, then retry.
    func start(wallet: WalletID) async throws(ServiceError)
    func stop(wallet: WalletID) async throws(ServiceError)
    /// `coinjoinsalt get/set/generate` (64 hex).
    func salt(wallet: WalletID) async throws(ServiceError) -> String
    func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError)
    func generateSalt(wallet: WalletID) async throws(ServiceError) -> String
}

public struct CoinJoinRecoveryReport: Sendable, Hashable {
    public let coinJoinAddressesScanned: Int
    public let bip44AddressesScanned: Int
    public let coinJoinBalance: Amount
    public let newTransactions: Int

    public init(coinJoinAddressesScanned: Int, bip44AddressesScanned: Int, coinJoinBalance: Amount, newTransactions: Int) {
        self.coinJoinAddressesScanned = coinJoinAddressesScanned
        self.bip44AddressesScanned = bip44AddressesScanned
        self.coinJoinBalance = coinJoinBalance
        self.newTransactions = newTransactions
    }
}

public enum MixedCoinsDestination: Sendable, Hashable {
    /// A fresh receive address of the same wallet.
    case wallet
    /// The shielded pool (M4; `not_implemented` until then).
    case shielded
}

public struct MixedCoinsChunk: Sendable, Hashable {
    public let inputs: Int
    public let amount: Amount
    public let fee: Amount

    public init(inputs: Int, amount: Amount, fee: Amount) {
        self.inputs = inputs
        self.amount = amount
        self.fee = fee
    }
}

public struct MixedCoinsSweepPlan: Sendable, Hashable {
    public let destination: MixedCoinsDestination
    public let total: Amount
    public let chunks: [MixedCoinsChunk]

    public init(destination: MixedCoinsDestination, total: Amount, chunks: [MixedCoinsChunk]) {
        self.destination = destination
        self.total = total
        self.chunks = chunks
    }
}

/// A sweep may stop part-way (iOS "may partially succeed").
public struct MixedCoinsSweepResult: Sendable, Hashable {
    public let txids: [String]
    public let moved: Amount
    public let remaining: Amount
    /// Engine code of the error that stopped the sweep; `nil` = complete.
    public let failureCode: ServiceErrorCode?

    public init(txids: [String], moved: Amount, remaining: Amount, failureCode: ServiceErrorCode?) {
        self.txids = txids
        self.moved = moved
        self.remaining = remaining
        self.failureCode = failureCode
    }
}

/// IOS-057: recovery scan and "Move mixed coins". Errors: `coinjoin.*`.
public protocol MixedCoinsMoving: AnyObject, Sendable {
    /// Runs as a rescan (`RepairProviding.rescanProgress`).
    func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport
    func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError) -> MixedCoinsSweepPlan
    /// `grant`: `.spend(max: ≥ plan.total)`.
    func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant) async throws(ServiceError)
        -> MixedCoinsSweepResult
}
