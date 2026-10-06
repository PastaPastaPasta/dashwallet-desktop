// M3 service contracts: the Masternodes tab, ProTx flows, shared
// masternodes, the masternode keychain, tracked masternodes and evonode
// tools (engine masternode.rs, protx.rs, masternode_keys.rs; m3-swift.md
// §2.3, m3-engine.md §2.3–2.5). Owner of the adapters: R3. Full-node-only
// columns (PoSe, last paid, next payment) are `nil` on SPV.
import Foundation

public enum MasternodeType: Sendable, Hashable {
    case regular
    case evo
}

/// The list's type combo (QT-118).
public enum MasternodeTypeFilter: Sendable, Hashable, CaseIterable {
    case all, regular, evo, shared
}

public enum MasternodeKeyRole: Sendable, Hashable, CaseIterable {
    case owner, voting, `operator`, platformNode, ownerPayout, operatorPayout
}

/// Why the wallets count a masternode as theirs (QT-120).
public enum OwnedRole: Sendable, Hashable, CaseIterable {
    case collateral, owner, voting, `operator`, payout, operatorPayout, platformNode, shareOwner, shareRefund, tracked
}

public enum MasternodeListStatus: Sendable, Hashable {
    /// `sinceHeight` is unknown on SPV ("Active for X" needs it).
    case active(sinceHeight: UInt32?)
    case banned(sinceHeight: UInt32?)
    case retired
    case unknown
}

/// "NONE", "x.xx% to <addr>", or "…but not claimed" without an address.
public struct OperatorReward: Sendable, Hashable {
    /// Hundredths of a percent.
    public let percentX100: Int
    public let payoutAddress: String?

    public init(percentX100: Int, payoutAddress: String?) {
        self.percentX100 = percentX100
        self.payoutAddress = payoutAddress
    }
}

/// "Shared (you hold %1 of %2)".
public struct SharedHolding: Sendable, Hashable {
    public let heldShares: Int
    public let totalShares: Int

    public init(heldShares: Int, totalShares: Int) {
        self.heldShares = heldShares
        self.totalShares = totalShares
    }
}

/// One list row (QT-119); `nil` = not known to an SPV wallet.
public struct MasternodeRow: Sendable, Hashable, Identifiable {
    public let proTxHash: String
    public let service: String?
    public let type: MasternodeType
    public let shared: SharedHolding?
    public let status: MasternodeListStatus
    public let poseScore: Int?
    public let registeredHeight: UInt32?
    public let lastPaidHeight: UInt32?
    public let nextPaymentHeight: UInt32?
    public let operatorReward: OperatorReward?
    public let collateral: OutPoint?
    public let collateralAddress: String?
    public let ownerAddress: String?
    public let votingAddress: String
    public let payoutAddresses: [String]
    public let operatorPublicKey: String
    public let platformNodeID: String?
    /// Empty = not owned.
    public let ownedRoles: Set<OwnedRole>
    public let label: String?

    public var id: String { proTxHash }

    public init(
        proTxHash: String, service: String?, type: MasternodeType, shared: SharedHolding?,
        status: MasternodeListStatus, poseScore: Int?, registeredHeight: UInt32?, lastPaidHeight: UInt32?,
        nextPaymentHeight: UInt32?, operatorReward: OperatorReward?, collateral: OutPoint?, collateralAddress: String?,
        ownerAddress: String?, votingAddress: String, payoutAddresses: [String], operatorPublicKey: String,
        platformNodeID: String?, ownedRoles: Set<OwnedRole>, label: String?
    ) {
        self.proTxHash = proTxHash
        self.service = service
        self.type = type
        self.shared = shared
        self.status = status
        self.poseScore = poseScore
        self.registeredHeight = registeredHeight
        self.lastPaidHeight = lastPaidHeight
        self.nextPaymentHeight = nextPaymentHeight
        self.operatorReward = operatorReward
        self.collateral = collateral
        self.collateralAddress = collateralAddress
        self.ownerAddress = ownerAddress
        self.votingAddress = votingAddress
        self.payoutAddresses = payoutAddresses
        self.operatorPublicKey = operatorPublicKey
        self.platformNodeID = platformNodeID
        self.ownedRoles = ownedRoles
        self.label = label
    }
}

public struct MasternodeQuery: Sendable, Hashable {
    public var typeFilter: MasternodeTypeFilter
    /// dash-qt's literal, case-insensitive search.
    public var text: String?
    public var ownedOnly: Bool
    public var hideBanned: Bool

    public init(typeFilter: MasternodeTypeFilter = .all, text: String? = nil, ownedOnly: Bool = false, hideBanned: Bool = false) {
        self.typeFilter = typeFilter
        self.text = text
        self.ownedOnly = ownedOnly
        self.hideBanned = hideBanned
    }
}

public struct MasternodeListState: Sendable, Hashable {
    public let available: Bool
    public let height: UInt32?
    public let total: Int
    public let enabled: Int
    public let evoTotal: Int
    public let evoEnabled: Int
    public let syncing: Bool

    public init(available: Bool, height: UInt32?, total: Int, enabled: Int, evoTotal: Int, evoEnabled: Int, syncing: Bool) {
        self.available = available
        self.height = height
        self.total = total
        self.enabled = enabled
        self.evoTotal = evoTotal
        self.evoEnabled = evoEnabled
        self.syncing = syncing
    }
}

public struct MasternodeShare: Sendable, Hashable {
    public let amount: Amount
    public let ownerAddress: String
    public let payoutAddress: String
    public let refundAddress: String
    public let mine: Bool

    public init(amount: Amount, ownerAddress: String, payoutAddress: String, refundAddress: String, mine: Bool) {
        self.amount = amount
        self.ownerAddress = ownerAddress
        self.payoutAddress = payoutAddress
        self.refundAddress = refundAddress
        self.mine = mine
    }
}

/// Details dialog (QT-122, IOS-080).
public struct MasternodeDetail: Sendable, Hashable {
    public let row: MasternodeRow
    public let consecutivePayments: Int?
    public let poseBanHeight: UInt32?
    public let poseRevivedHeight: UInt32?
    public let networkAddresses: [String]
    public let platformP2PAddresses: [String]
    public let platformHTTPSAddresses: [String]
    public let shares: [MasternodeShare]
    public let earlyPeriodEnd: UInt32?
    public let earlyExitPenalty: Amount?
    public let hasStandbyDissolution: Bool
    public let revocationReason: Int?
    public let walletTransactions: Int

    public init(
        row: MasternodeRow, consecutivePayments: Int?, poseBanHeight: UInt32?, poseRevivedHeight: UInt32?,
        networkAddresses: [String], platformP2PAddresses: [String], platformHTTPSAddresses: [String],
        shares: [MasternodeShare], earlyPeriodEnd: UInt32?, earlyExitPenalty: Amount?, hasStandbyDissolution: Bool,
        revocationReason: Int?, walletTransactions: Int
    ) {
        self.row = row
        self.consecutivePayments = consecutivePayments
        self.poseBanHeight = poseBanHeight
        self.poseRevivedHeight = poseRevivedHeight
        self.networkAddresses = networkAddresses
        self.platformP2PAddresses = platformP2PAddresses
        self.platformHTTPSAddresses = platformHTTPSAddresses
        self.shares = shares
        self.earlyPeriodEnd = earlyPeriodEnd
        self.earlyExitPenalty = earlyExitPenalty
        self.hasStandbyDissolution = hasStandbyDissolution
        self.revocationReason = revocationReason
        self.walletTransactions = walletTransactions
    }
}

/// Engine `masternode_network_defaults`.
public struct MasternodeNetworkDefaults: Sendable, Hashable {
    public let coreP2PPort: UInt16
    public let platformP2PPort: UInt16
    public let platformHTTPSPort: UInt16
    public let masternodeCollateral: Amount
    public let evonodeCollateral: Amount
    public let shares: ClosedRange<Int>
    public let minimumShareAmount: Amount
    public let maxEarlyPeriodBlocks: UInt32
    public let maxEnvelopeBytes: Int
    public let maxOperatorRewardX100: Int

    public init(
        coreP2PPort: UInt16, platformP2PPort: UInt16, platformHTTPSPort: UInt16, masternodeCollateral: Amount,
        evonodeCollateral: Amount, shares: ClosedRange<Int>, minimumShareAmount: Amount, maxEarlyPeriodBlocks: UInt32,
        maxEnvelopeBytes: Int, maxOperatorRewardX100: Int
    ) {
        self.coreP2PPort = coreP2PPort
        self.platformP2PPort = platformP2PPort
        self.platformHTTPSPort = platformHTTPSPort
        self.masternodeCollateral = masternodeCollateral
        self.evonodeCollateral = evonodeCollateral
        self.shares = shares
        self.minimumShareAmount = minimumShareAmount
        self.maxEarlyPeriodBlocks = maxEarlyPeriodBlocks
        self.maxEnvelopeBytes = maxEnvelopeBytes
        self.maxOperatorRewardX100 = maxOperatorRewardX100
    }
}

/// The Masternodes tab (QT-118…122, IOS-080). Errors: `masternode.*`.
public protocol MasternodeListProviding: AnyObject, Sendable {
    func defaults() -> MasternodeNetworkDefaults
    func state() async throws(ServiceError) -> MasternodeListState
    /// Engine `Masternodes` events (≤ every 3 s; 30 s while syncing).
    func changes() -> AsyncStream<Void>
    func list(_ query: MasternodeQuery) async throws(ServiceError) -> [MasternodeRow]
    func detail(proTxHash: String) async throws(ServiceError) -> MasternodeDetail
}

// MARK: - Provider transactions (QT-123…127, IOS-081)

public enum CollateralChoice: Sendable, Hashable {
    case fundNew
    case existingUTXO(OutPoint)
    /// Signed externally (`collateralSignMessage`).
    case external(OutPoint)
}

public enum OperatorKeyChoice: Sendable, Hashable {
    /// The secret is shown once (QT-124) and never stored.
    case generate
    case existing(publicKeyHex: String)
}

public struct PlatformFields: Sendable, Hashable {
    public var nodeIDHex: String
    public var p2pAddresses: [String]
    public var httpsAddresses: [String]

    public init(nodeIDHex: String, p2pAddresses: [String], httpsAddresses: [String]) {
        self.nodeIDHex = nodeIDHex
        self.p2pAddresses = p2pAddresses
        self.httpsAddresses = httpsAddresses
    }
}

public enum FeeSourceChoice: Sendable, Hashable {
    case automatic
    case address(String)
}

/// The Register wizard's answers (QT-123).
public struct RegistrationRequest: Sendable, Hashable {
    public var wallet: WalletID
    public var type: MasternodeType
    public var collateral: CollateralChoice
    public var serviceAddresses: [String]
    /// `nil` = a fresh wallet address.
    public var ownerAddress: String?
    /// `nil` = the owner address.
    public var votingAddress: String?
    public var operatorKey: OperatorKeyChoice
    public var payoutAddress: String
    public var operatorRewardX100: Int
    public var platform: PlatformFields?
    public var feeSource: FeeSourceChoice

    public init(
        wallet: WalletID, type: MasternodeType, collateral: CollateralChoice, serviceAddresses: [String],
        ownerAddress: String?, votingAddress: String?, operatorKey: OperatorKeyChoice, payoutAddress: String,
        operatorRewardX100: Int, platform: PlatformFields?, feeSource: FeeSourceChoice
    ) {
        self.wallet = wallet
        self.type = type
        self.collateral = collateral
        self.serviceAddresses = serviceAddresses
        self.ownerAddress = ownerAddress
        self.votingAddress = votingAddress
        self.operatorKey = operatorKey
        self.payoutAddress = payoutAddress
        self.operatorRewardX100 = operatorRewardX100
        self.platform = platform
        self.feeSource = feeSource
    }
}

public enum CollateralRefusal: Sendable, Hashable, CaseIterable {
    case wrongAmount, unconfirmed, notP2PKH, locked, alreadyCollateral, notFound
}

public struct CollateralCandidate: Sendable, Hashable {
    public let outpoint: OutPoint
    public let address: String
    public let amount: Amount
    public let confirmations: Int
    public let refusal: CollateralRefusal?

    public init(outpoint: OutPoint, address: String, amount: Amount, confirmations: Int, refusal: CollateralRefusal?) {
        self.outpoint = outpoint
        self.address = address
        self.amount = amount
        self.confirmations = confirmations
        self.refusal = refusal
    }
}

public struct FeeSourceCandidate: Sendable, Hashable {
    public let address: String
    public let spendable: Amount
    public let label: String?

    public init(address: String, spendable: Amount, label: String?) {
        self.address = address
        self.spendable = spendable
        self.label = label
    }
}

/// The review page (QT-123).
public struct RegistrationSummary: Sendable, Hashable {
    public let type: MasternodeType
    public let proTxHash: String
    public let collateral: OutPoint
    public let collateralAddress: String
    public let ownerAddress: String
    public let votingAddress: String
    public let payoutAddress: String
    public let operatorPublicKey: String
    public let operatorRewardX100: Int
    public let serviceAddresses: [String]
    public let platform: PlatformFields?
    public let fee: Amount
    public let totalSpent: Amount
    public let operatorSecretRequired: Bool
    public let collateralSignMessage: String?

    public init(
        type: MasternodeType, proTxHash: String, collateral: OutPoint, collateralAddress: String, ownerAddress: String,
        votingAddress: String, payoutAddress: String, operatorPublicKey: String, operatorRewardX100: Int,
        serviceAddresses: [String], platform: PlatformFields?, fee: Amount, totalSpent: Amount,
        operatorSecretRequired: Bool, collateralSignMessage: String?
    ) {
        self.type = type
        self.proTxHash = proTxHash
        self.collateral = collateral
        self.collateralAddress = collateralAddress
        self.ownerAddress = ownerAddress
        self.votingAddress = votingAddress
        self.payoutAddress = payoutAddress
        self.operatorPublicKey = operatorPublicKey
        self.operatorRewardX100 = operatorRewardX100
        self.serviceAddresses = serviceAddresses
        self.platform = platform
        self.fee = fee
        self.totalSpent = totalSpent
        self.operatorSecretRequired = operatorSecretRequired
        self.collateralSignMessage = collateralSignMessage
    }
}

/// A prepared registration the adapter holds (`PreparedRegistration`), as
/// `PSBTReference` names an engine PSBT (M2).
public struct PreparedRegistrationReference: Sendable, Hashable, Identifiable {
    public let id: UUID
    public let summary: RegistrationSummary

    public init(id: UUID, summary: RegistrationSummary) {
        self.id = id
        self.summary = summary
    }
}

/// The generated operator secret (QT-124), shown once.
public struct OperatorSecret: Sendable {
    /// 64 hex characters.
    public let secretHex: any SecretBuffer
    /// `masternodeblsprivkey=<hex>`.
    public let configLine: any SecretBuffer

    public init(secretHex: any SecretBuffer, configLine: any SecretBuffer) {
        self.secretHex = secretHex
        self.configLine = configLine
    }
}

/// Register Masternode/EvoNode (QT-123, QT-124). Errors: `masternode.*`.
public protocol MasternodeRegistering: AnyObject, Sendable {
    func collateralCandidates(wallet: WalletID, type: MasternodeType) async throws(ServiceError) -> [CollateralCandidate]
    func feeSourceCandidates(wallet: WalletID) async throws(ServiceError) -> [FeeSourceCandidate]
    /// `grant`: `.masternodeOperation` (a `fundNew` registration also
    /// spends the collateral).
    func prepare(_ request: RegistrationRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedRegistrationReference
    func operatorSecret(_ registration: PreparedRegistrationReference) async throws(ServiceError) -> OperatorSecret
    /// The "type the last 4 characters" gate.
    func confirmOperatorSecret(_ registration: PreparedRegistrationReference, last4: String) async throws(ServiceError)
        -> Bool
    /// Returns the proTxHash. `masternode.operator_secret_unconfirmed`
    /// before the gate opened.
    func submit(_ registration: PreparedRegistrationReference, collateralSignature: String?)
        async throws(ServiceError) -> String
    func abandon(_ registration: PreparedRegistrationReference) async
}

public enum RevocationReason: Sendable, Hashable, CaseIterable {
    case notSpecified, terminationOfService, compromisedKeys, changeOfKeys
}

public struct UpdateServiceRequest: Sendable {
    public var proTxHash: String
    public var serviceAddresses: [String]
    /// Typed each time (dash-qt); `nil` = a wallet or tracked key.
    public var operatorSecret: (any SecretBuffer)?
    public var platform: PlatformFields?
    public var operatorPayoutAddress: String?
    public var feeSource: FeeSourceChoice
    public var feeWallet: WalletID

    public init(
        proTxHash: String, serviceAddresses: [String], operatorSecret: (any SecretBuffer)?, platform: PlatformFields?,
        operatorPayoutAddress: String?, feeSource: FeeSourceChoice, feeWallet: WalletID
    ) {
        self.proTxHash = proTxHash
        self.serviceAddresses = serviceAddresses
        self.operatorSecret = operatorSecret
        self.platform = platform
        self.operatorPayoutAddress = operatorPayoutAddress
        self.feeSource = feeSource
        self.feeWallet = feeWallet
    }
}

/// Only changed fields are sent.
public struct UpdateRegistrarRequest: Sendable, Hashable {
    public var proTxHash: String
    public var operatorPublicKey: String?
    public var votingAddress: String?
    public var payoutAddress: String?
    public var feeSource: FeeSourceChoice
    public var feeWallet: WalletID

    public init(
        proTxHash: String, operatorPublicKey: String?, votingAddress: String?, payoutAddress: String?,
        feeSource: FeeSourceChoice, feeWallet: WalletID
    ) {
        self.proTxHash = proTxHash
        self.operatorPublicKey = operatorPublicKey
        self.votingAddress = votingAddress
        self.payoutAddress = payoutAddress
        self.feeSource = feeSource
        self.feeWallet = feeWallet
    }
}

public struct RevokeRequest: Sendable {
    public var proTxHash: String
    public var operatorSecret: (any SecretBuffer)?
    public var reason: RevocationReason
    public var feeSource: FeeSourceChoice
    public var feeWallet: WalletID

    public init(
        proTxHash: String, operatorSecret: (any SecretBuffer)?, reason: RevocationReason, feeSource: FeeSourceChoice,
        feeWallet: WalletID
    ) {
        self.proTxHash = proTxHash
        self.operatorSecret = operatorSecret
        self.reason = reason
        self.feeSource = feeSource
        self.feeWallet = feeWallet
    }
}

public enum ProviderTransactionKind: Sendable, Hashable {
    case updateService, updateRegistrar, revoke, updateShare, updateSharedRegistrar, dissolve
}

public struct ProviderTransactionSummary: Sendable, Hashable {
    public let kind: ProviderTransactionKind
    public let proTxHash: String
    public let txid: String
    public let fee: Amount
    public let penalty: Amount?
    /// "Changing the operator key immediately PoSe-bans the masternode…"
    public let bansMasternode: Bool

    public init(
        kind: ProviderTransactionKind, proTxHash: String, txid: String, fee: Amount, penalty: Amount?,
        bansMasternode: Bool
    ) {
        self.kind = kind
        self.proTxHash = proTxHash
        self.txid = txid
        self.fee = fee
        self.penalty = penalty
        self.bansMasternode = bansMasternode
    }
}

/// A signed, not yet broadcast provider transaction the adapter holds.
public struct PreparedProviderTransaction: Sendable, Hashable, Identifiable {
    public let id: UUID
    public let summary: ProviderTransactionSummary

    public init(id: UUID, summary: ProviderTransactionSummary) {
        self.id = id
        self.summary = summary
    }
}

/// Raw transactions to save as `.txt` (QT-127).
public struct StandbyDissolution: Sendable, Hashable {
    public let proTxHash: String
    public let transactionsHex: [String]
    public let suggestedFileName: String

    public init(proTxHash: String, transactionsHex: [String], suggestedFileName: String) {
        self.proTxHash = proTxHash
        self.transactionsHex = transactionsHex
        self.suggestedFileName = suggestedFileName
    }
}

/// Update Service / Registrar / Revoke and the single-party shared actions
/// (QT-125, QT-127, IOS-081 unban). Every `grant` is `.masternodeOperation`.
public protocol MasternodeMaintaining: AnyObject, Sendable {
    func prepareUpdateService(_ request: UpdateServiceRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    func prepareUpdateRegistrar(_ request: UpdateRegistrarRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    func prepareRevoke(_ request: RevokeRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    func prepareShareRewardUpdate(
        proTxHash: String, shareIndex: Int, payoutAddress: String, feeWallet: WalletID, grant: AuthGrant
    ) async throws(ServiceError) -> PreparedProviderTransaction
    /// `acceptPenalty` must be true inside the early period.
    func prepareDissolveNow(proTxHash: String, acceptPenalty: Bool, feeWallet: WalletID, grant: AuthGrant)
        async throws(ServiceError) -> PreparedProviderTransaction
    /// Returns the txid.
    func broadcast(_ transaction: PreparedProviderTransaction) async throws(ServiceError) -> String
    func abandon(_ transaction: PreparedProviderTransaction) async
    func createStandbyDissolution(wallet: WalletID, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> StandbyDissolution
    func broadcastStandbyDissolution(_ transactionsHex: [String]) async throws(ServiceError) -> [String]
}

public struct SharedShareTerms: Sendable, Hashable {
    public var amount: Amount
    public var label: String?
    public var mine: Bool

    public init(amount: Amount, label: String?, mine: Bool) {
        self.amount = amount
        self.label = label
        self.mine = mine
    }
}

public struct SharedMasternodeTerms: Sendable, Hashable {
    public var shares: [SharedShareTerms]
    public var earlyPeriodBlocks: UInt32
    public var earlyExitPenalty: Amount
    public var serviceAddresses: [String]
    public var operatorKey: OperatorKeyChoice
    public var operatorRewardX100: Int

    public init(
        shares: [SharedShareTerms], earlyPeriodBlocks: UInt32, earlyExitPenalty: Amount, serviceAddresses: [String],
        operatorKey: OperatorKeyChoice, operatorRewardX100: Int
    ) {
        self.shares = shares
        self.earlyPeriodBlocks = earlyPeriodBlocks
        self.earlyExitPenalty = earlyExitPenalty
        self.serviceAddresses = serviceAddresses
        self.operatorKey = operatorKey
        self.operatorRewardX100 = operatorRewardX100
    }
}

public enum SharedRole: Sendable, Hashable {
    case coordinator, participant
}

public enum SharedStage: Sendable, Hashable, CaseIterable {
    case invitation, details, lockedTerms, approvals, signingRequest, signedContributions, broadcast, completed
    case abandoned
}

public enum SharedSessionPurpose: Sendable, Hashable {
    case register, rotateKeys, dissolveTogether
}

public struct SharedSessionInfo: Sendable, Hashable, Identifiable {
    public let id: String
    /// First 6 hex characters of the session id.
    public let sessionCode: String
    public let purpose: SharedSessionPurpose
    public let role: SharedRole
    public let stage: SharedStage
    public let revision: Int
    /// `XXXX-XXXX`.
    public let fingerprint: String
    public let wallet: WalletID
    public let myShareIndexes: [Int]
    public let reservedInputs: [OutPoint]
    public let reservedCoinSpent: Bool
    public let proTxHash: String?

    public init(
        id: String, sessionCode: String, purpose: SharedSessionPurpose, role: SharedRole, stage: SharedStage,
        revision: Int, fingerprint: String, wallet: WalletID, myShareIndexes: [Int], reservedInputs: [OutPoint],
        reservedCoinSpent: Bool, proTxHash: String?
    ) {
        self.id = id
        self.sessionCode = sessionCode
        self.purpose = purpose
        self.role = role
        self.stage = stage
        self.revision = revision
        self.fingerprint = fingerprint
        self.wallet = wallet
        self.myShareIndexes = myShareIndexes
        self.reservedInputs = reservedInputs
        self.reservedCoinSpent = reservedCoinSpent
        self.proTxHash = proTxHash
    }
}

public struct SharedEnvelope: Sendable, Hashable {
    public let json: String
    public let fingerprint: String
    public let suggestedFileName: String

    public init(json: String, fingerprint: String, suggestedFileName: String) {
        self.json = json
        self.fingerprint = fingerprint
        self.suggestedFileName = suggestedFileName
    }
}

/// What a paste or a dropped file is (QT-127 paste routing).
public enum SharedMessageKind: Sendable, Hashable {
    case envelope(SharedSessionInfo)
    case standbyDissolution(proTxHash: String?, transactionsHex: [String])
}

public struct ShareContribution: Sendable, Hashable {
    public var shareIndex: Int
    public var inputs: [OutPoint]
    public var ownerAddress: String?
    public var payoutAddress: String?
    public var refundAddress: String?

    public init(
        shareIndex: Int, inputs: [OutPoint], ownerAddress: String?, payoutAddress: String?, refundAddress: String?
    ) {
        self.shareIndex = shareIndex
        self.inputs = inputs
        self.ownerAddress = ownerAddress
        self.payoutAddress = payoutAddress
        self.refundAddress = refundAddress
    }
}

/// v24 shared masternodes, multi-party flows over clipboard/files (QT-126,
/// QT-127). Errors: `masternode.*` (`shared_*`).
public protocol SharedMasternodeCoordinating: AnyObject, Sendable {
    func create(wallet: WalletID, terms: SharedMasternodeTerms) async throws(ServiceError) -> SharedSessionInfo
    /// ≤ 2 MiB, this network only; routes envelopes and standby files.
    func importMessage(_ text: String, wallet: WalletID) async throws(ServiceError) -> SharedMessageKind
    func sessions(wallet: WalletID) async throws(ServiceError) -> [SharedSessionInfo]
    func message(session: String) async throws(ServiceError) -> SharedEnvelope
    func contribute(_ contribution: ShareContribution, session: String) async throws(ServiceError) -> SharedSessionInfo
    func approve(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo
    func sign(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo
    /// Coordinator; returns the txid.
    func broadcast(session: String) async throws(ServiceError) -> String
    /// Releases the session's reserved coins.
    func abandon(session: String) async throws(ServiceError)
    func startKeyRotation(
        wallet: WalletID, proTxHash: String, operatorKey: OperatorKeyChoice?, votingAddress: String?
    ) async throws(ServiceError) -> SharedSessionInfo
    func startDissolveTogether(wallet: WalletID, proTxHash: String) async throws(ServiceError) -> SharedSessionInfo
}

// MARK: - Keychain, tracked masternodes, evonode tools (IOS-080…083)

public struct MasternodeKeyUsage: Sendable, Hashable {
    public let proTxHash: String
    public let service: String?
    public let revoked: Bool

    public init(proTxHash: String, service: String?, revoked: Bool) {
        self.proTxHash = proTxHash
        self.service = service
        self.revoked = revoked
    }
}

/// One derived provider key, public data only (IOS-083).
public struct MasternodeKeyInfo: Sendable, Hashable, Identifiable {
    public let role: MasternodeKeyRole
    public let index: UInt32
    public let derivationPath: String
    public let address: String?
    public let publicKeyHex: String
    public let legacyPublicKeyHex: String?
    public let platformNodeID: String?
    public let usedBy: [MasternodeKeyUsage]

    public var id: String { derivationPath }

    public init(
        role: MasternodeKeyRole, index: UInt32, derivationPath: String, address: String?, publicKeyHex: String,
        legacyPublicKeyHex: String?, platformNodeID: String?, usedBy: [MasternodeKeyUsage]
    ) {
        self.role = role
        self.index = index
        self.derivationPath = derivationPath
        self.address = address
        self.publicKeyHex = publicKeyHex
        self.legacyPublicKeyHex = legacyPublicKeyHex
        self.platformNodeID = platformNodeID
        self.usedBy = usedBy
    }
}

/// A revealed provider private key, held transiently.
public struct RevealedMasternodeKey: Sendable {
    public let privateKeyHex: any SecretBuffer
    public let wif: (any SecretBuffer)?
    public let tenderdashKey: (any SecretBuffer)?

    public init(privateKeyHex: any SecretBuffer, wif: (any SecretBuffer)?, tenderdashKey: (any SecretBuffer)?) {
        self.privateKeyHex = privateKeyHex
        self.wif = wif
        self.tenderdashKey = tenderdashKey
    }
}

/// Masternode keychain (IOS-083). Errors: `masternode.*`.
public protocol MasternodeKeychainProviding: AnyObject, Sendable {
    /// `range.count ≤ 100`.
    func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    /// `grant`: `.revealSecret`.
    func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant)
        async throws(ServiceError) -> RevealedMasternodeKey
}

public struct TrackedCapabilities: Sendable, Hashable {
    public let canWithdraw: Bool
    public let canUpdateService: Bool
    public let canUpdateRegistrar: Bool
    public let canVote: Bool

    public init(canWithdraw: Bool, canUpdateService: Bool, canUpdateRegistrar: Bool, canVote: Bool) {
        self.canWithdraw = canWithdraw
        self.canUpdateService = canUpdateService
        self.canUpdateRegistrar = canUpdateRegistrar
        self.canVote = canVote
    }
}

public struct TrackedMasternode: Sendable, Hashable, Identifiable {
    public let row: MasternodeRow
    public let label: String?
    public let attachedRoles: Set<MasternodeKeyRole>
    public let capabilities: TrackedCapabilities

    public var id: String { row.proTxHash }

    public init(row: MasternodeRow, label: String?, attachedRoles: Set<MasternodeKeyRole>, capabilities: TrackedCapabilities) {
        self.row = row
        self.label = label
        self.attachedRoles = attachedRoles
        self.capabilities = capabilities
    }
}

/// Track any masternode and attach its keys (IOS-082). Errors:
/// `masternode.*`.
public protocol TrackedMasternodeManaging: AnyObject, Sendable {
    /// By IP, `IP:port`, proTxHash, address or operator key.
    func locate(_ query: String) async throws(ServiceError) -> [MasternodeRow]
    func tracked() async throws(ServiceError) -> [TrackedMasternode]
    func track(proTxHash: String, label: String?) async throws(ServiceError) -> TrackedMasternode
    func untrack(proTxHash: String) async throws(ServiceError) -> Bool
    func setLabel(_ label: String?, proTxHash: String) async throws(ServiceError)
    /// Stored in the vault; `grant`: `.masternodeOperation`.
    func attach(_ key: any SecretBuffer, role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError)
    func detach(role: MasternodeKeyRole, proTxHash: String) async throws(ServiceError)
    /// `grant`: `.revealSecret`.
    func reveal(role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
}

public struct EvonodePlatformStatus: Sendable, Hashable {
    public let proTxHash: String
    public let claimableCredits: UInt64?
    public let epochProposedBlocks: Int?
    public let epochIndex: Int?

    public init(proTxHash: String, claimableCredits: UInt64?, epochProposedBlocks: Int?, epochIndex: Int?) {
        self.proTxHash = proTxHash
        self.claimableCredits = claimableCredits
        self.epochProposedBlocks = epochProposedBlocks
        self.epochIndex = epochIndex
    }
}

public enum CreditWithdrawalDestination: Sendable, Hashable {
    case payoutAddress
    case address(String)
}

/// Evonode Platform tools (IOS-080/081). Platform work: may answer
/// `not_implemented` until M4.
public protocol EvonodeServicing: AnyObject, Sendable {
    func status(proTxHash: String) async throws(ServiceError) -> EvonodePlatformStatus
    /// Credits in Platform units; `grant`: `.masternodeOperation`. Returns
    /// the state-transition id.
    func withdraw(proTxHash: String, credits: UInt64, destination: CreditWithdrawalDestination, grant: AuthGrant)
        async throws(ServiceError) -> String
}
