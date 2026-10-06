// M3 service contracts: governance (engine governance.rs; m3-swift.md §2.2,
// m3-engine.md §2.2). Owner of the adapter: R2. Values not synced yet are
// `nil`, never estimated.
import Foundation

/// Governance constants of a network (engine `governance_params`).
public struct GovernanceParameters: Sendable, Hashable {
    public let superblockStartHeight: UInt32
    public let superblockCycle: UInt32
    public let maturityWindow: UInt32
    public let minQuorum: Int
    public let proposalFee: Amount
    public let feeConfirmations: Int
    public let maxNameLength: Int
    public let maxPayloadBytes: Int
    public let maxPayments: Int
    public let evonodeVoteWeight: Int
    public let voteUpdateMinimum: Duration
    public let targetSpacing: Duration

    public init(
        superblockStartHeight: UInt32, superblockCycle: UInt32, maturityWindow: UInt32, minQuorum: Int,
        proposalFee: Amount, feeConfirmations: Int, maxNameLength: Int, maxPayloadBytes: Int, maxPayments: Int,
        evonodeVoteWeight: Int, voteUpdateMinimum: Duration, targetSpacing: Duration
    ) {
        self.superblockStartHeight = superblockStartHeight
        self.superblockCycle = superblockCycle
        self.maturityWindow = maturityWindow
        self.minQuorum = minQuorum
        self.proposalFee = proposalFee
        self.feeConfirmations = feeConfirmations
        self.maxNameLength = maxNameLength
        self.maxPayloadBytes = maxPayloadBytes
        self.maxPayments = maxPayments
        self.evonodeVoteWeight = evonodeVoteWeight
        self.voteUpdateMinimum = voteUpdateMinimum
        self.targetSpacing = targetSpacing
    }
}

public enum GovernanceSyncPhase: Sendable, Hashable {
    case disabled
    /// "waiting for sync…": chain or masternode list not synced, or no peers.
    case waiting
    case syncingObjects
    case syncingVotes
    case synced
    case failed
}

public struct GovernanceSyncState: Sendable, Hashable {
    public let phase: GovernanceSyncPhase
    public let objects: Int
    public let votes: Int
    public let peers: Int
    public let bytesReceived: UInt64
    public let lastSyncedAt: Date?

    public init(
        phase: GovernanceSyncPhase, objects: Int, votes: Int, peers: Int, bytesReceived: UInt64, lastSyncedAt: Date?
    ) {
        self.phase = phase
        self.objects = objects
        self.votes = votes
        self.peers = peers
        self.bytesReceived = bytesReceived
        self.lastSyncedAt = lastSyncedAt
    }
}

/// dash-qt `ProposalStatus` (research 02 §11.1).
public enum ProposalStatus: Sendable, Hashable, CaseIterable {
    case funded, lapsed, confirming, pending, passing, failing, voting, unfunded
}

public enum VoteOutcome: Sendable, Hashable, CaseIterable {
    case yes, no, abstain
}

public enum ProposalSource: Sendable, Hashable {
    /// "Active Proposals".
    case active
    /// "My Proposals" (the Votes column is hidden).
    case mine(WalletID)
}

public struct ProposalQuery: Sendable, Hashable {
    public var source: ProposalSource
    /// Case-insensitive, title only.
    public var titleFilter: String?

    public init(source: ProposalSource, titleFilter: String? = nil) {
        self.source = source
        self.titleFilter = titleFilter
    }
}

/// "My Votes" (weighted); `nil` on a row = "No voting keys".
public struct MyVotes: Sendable, Hashable {
    public let yes: Int
    public let no: Int
    public let abstain: Int
    public let unvoted: Int

    public init(yes: Int, no: Int, abstain: Int, unvoted: Int) {
        self.yes = yes
        self.no = no
        self.abstain = abstain
        self.unvoted = unvoted
    }
}

/// One proposal list row (QT-129).
public struct ProposalRow: Sendable, Hashable, Identifiable {
    public let hash: String
    public let name: String
    public let url: String
    public let paymentAddress: String
    public let paymentAmount: Amount
    public let start: Date
    public let end: Date
    public let status: ProposalStatus
    public let collateralConfirmations: Int?
    public let yes: Int
    public let no: Int
    public let abstain: Int
    /// `(yes − no) − threshold`.
    public let margin: Int
    public let myVotes: MyVotes?

    public var id: String { hash }

    public init(
        hash: String, name: String, url: String, paymentAddress: String, paymentAmount: Amount, start: Date, end: Date,
        status: ProposalStatus, collateralConfirmations: Int?, yes: Int, no: Int, abstain: Int, margin: Int,
        myVotes: MyVotes?
    ) {
        self.hash = hash
        self.name = name
        self.url = url
        self.paymentAddress = paymentAddress
        self.paymentAmount = paymentAmount
        self.start = start
        self.end = end
        self.status = status
        self.collateralConfirmations = collateralConfirmations
        self.yes = yes
        self.no = no
        self.abstain = abstain
        self.margin = margin
        self.myVotes = myVotes
    }
}

/// Details view and "Copy Raw JSON" (QT-130).
public struct ProposalDetail: Sendable, Hashable {
    public let row: ProposalRow
    public let parentHash: String
    public let collateralTxid: String
    public let createdAt: Date
    public let payments: Int
    public let rawJSON: String

    public init(row: ProposalRow, parentHash: String, collateralTxid: String, createdAt: Date, payments: Int, rawJSON: String) {
        self.row = row
        self.parentHash = parentHash
        self.collateralTxid = collateralTxid
        self.createdAt = createdAt
        self.payments = payments
        self.rawJSON = rawJSON
    }
}

/// A masternode a wallet can vote with (QT-131 table).
public struct VotingMasternode: Sendable, Hashable, Identifiable {
    public let proTxHash: String
    /// `txid-n`.
    public let collateral: String
    public let votingAddress: String
    public let weight: Int
    public let currentVote: VoteOutcome?
    public let voteTime: Date?
    /// The 1-hour rule: a changed vote is accepted from then on.
    public let nextVoteAt: Date?
    public let label: String?

    public var id: String { proTxHash }

    public init(
        proTxHash: String, collateral: String, votingAddress: String, weight: Int, currentVote: VoteOutcome?,
        voteTime: Date?, nextVoteAt: Date?, label: String?
    ) {
        self.proTxHash = proTxHash
        self.collateral = collateral
        self.votingAddress = votingAddress
        self.weight = weight
        self.currentVote = currentVote
        self.voteTime = voteTime
        self.nextVoteAt = nextVoteAt
        self.label = label
    }
}

/// One masternode's vote outcome ("Voted successfully %n time(s)").
public struct VoteResult: Sendable, Hashable {
    public let proTxHash: String
    /// `nil` = relayed.
    public let errorCode: ServiceErrorCode?

    public init(proTxHash: String, errorCode: ServiceErrorCode?) {
        self.proTxHash = proTxHash
        self.errorCode = errorCode
    }
}

public struct SuperblockDate: Sendable, Hashable {
    public let height: UInt32
    public let estimatedDate: Date

    public init(height: UInt32, estimatedDate: Date) {
        self.height = height
        self.estimatedDate = estimatedDate
    }
}

/// Create Proposal input (QT-132). The chosen superblock is honoured.
public struct ProposalDraft: Sendable, Hashable {
    public var name: String
    public var url: String
    public var paymentAddress: String
    public var paymentAmount: Amount
    public var paymentCount: Int
    public var firstSuperblockHeight: UInt32

    public init(
        name: String, url: String, paymentAddress: String, paymentAmount: Amount, paymentCount: Int,
        firstSuperblockHeight: UInt32
    ) {
        self.name = name
        self.url = url
        self.paymentAddress = paymentAddress
        self.paymentAmount = paymentAmount
        self.paymentCount = paymentCount
        self.firstSuperblockHeight = firstSuperblockHeight
    }
}

public enum ProposalField: Sendable, Hashable, CaseIterable {
    case name, url, paymentAddress, paymentAmount, paymentCount, firstPayment, payload
}

public enum CollateralStatus: Sendable, Hashable {
    case unknown
    case pending
    /// ≥ 1 confirmation: "Broadcast" enabled.
    case ready
}

/// A created, not yet submitted proposal (QT-133).
public struct PendingProposal: Sendable, Hashable, Identifiable {
    public let hash: String
    public let name: String
    public let url: String
    public let paymentAmount: Amount
    public let paymentCount: Int
    public let collateralTxid: String
    public let collateralStatus: CollateralStatus
    public let confirmations: Int
    public let createdAt: Date
    public let end: Date

    public var id: String { hash }

    public init(
        hash: String, name: String, url: String, paymentAmount: Amount, paymentCount: Int, collateralTxid: String,
        collateralStatus: CollateralStatus, confirmations: Int, createdAt: Date, end: Date
    ) {
        self.hash = hash
        self.name = name
        self.url = url
        self.paymentAmount = paymentAmount
        self.paymentCount = paymentCount
        self.collateralTxid = collateralTxid
        self.collateralStatus = collateralStatus
        self.confirmations = confirmations
        self.createdAt = createdAt
        self.end = end
    }
}

/// Governance info panel (QT-134); `nil` = not synced.
public struct GovernanceInfo: Sendable, Hashable {
    public let sync: GovernanceSyncState
    public let superblockCycle: UInt32
    public let lastSuperblock: UInt32?
    public let nextSuperblock: UInt32?
    public let nextSuperblockDate: Date?
    public let votingCutoff: UInt32?
    public let masternodesVoting: Int?
    public let masternodesEligible: Int?
    public let evonodesVoting: Int?
    public let evonodesEligible: Int?
    public let passingThreshold: Int?
    public let masternodesControlled: Int
    public let votesControlled: Int
    public let proposalCount: Int?
    public let passing: Int?
    public let failing: Int?
    public let unfunded: Int?
    public let unfundedShort: Amount?
    public let budgetAvailable: Amount?
    public let budgetAllocated: Amount?

    public init(
        sync: GovernanceSyncState, superblockCycle: UInt32, lastSuperblock: UInt32?, nextSuperblock: UInt32?,
        nextSuperblockDate: Date?, votingCutoff: UInt32?, masternodesVoting: Int?, masternodesEligible: Int?,
        evonodesVoting: Int?, evonodesEligible: Int?, passingThreshold: Int?, masternodesControlled: Int,
        votesControlled: Int, proposalCount: Int?, passing: Int?, failing: Int?, unfunded: Int?,
        unfundedShort: Amount?, budgetAvailable: Amount?, budgetAllocated: Amount?
    ) {
        self.sync = sync
        self.superblockCycle = superblockCycle
        self.lastSuperblock = lastSuperblock
        self.nextSuperblock = nextSuperblock
        self.nextSuperblockDate = nextSuperblockDate
        self.votingCutoff = votingCutoff
        self.masternodesVoting = masternodesVoting
        self.masternodesEligible = masternodesEligible
        self.evonodesVoting = evonodesVoting
        self.evonodesEligible = evonodesEligible
        self.passingThreshold = passingThreshold
        self.masternodesControlled = masternodesControlled
        self.votesControlled = votesControlled
        self.proposalCount = proposalCount
        self.passing = passing
        self.failing = failing
        self.unfunded = unfunded
        self.unfundedShort = unfundedShort
        self.budgetAvailable = budgetAvailable
        self.budgetAllocated = budgetAllocated
    }
}

/// Status-bar governance clock (QT-026).
public struct GovernanceClock: Sendable, Hashable {
    /// 0–1, the moon phase.
    public let cycleProgress: Double
    public let nextSuperblock: UInt32
    public let blocksToSuperblock: UInt32
    public let superblockDate: Date
    public let votingCutoff: UInt32
    public let votingOpen: Bool
    /// 0–1; `nil` until governance synced.
    public let budgetCommitted: Double?

    public init(
        cycleProgress: Double, nextSuperblock: UInt32, blocksToSuperblock: UInt32, superblockDate: Date,
        votingCutoff: UInt32, votingOpen: Bool, budgetCommitted: Double?
    ) {
        self.cycleProgress = cycleProgress
        self.nextSuperblock = nextSuperblock
        self.blocksToSuperblock = blocksToSuperblock
        self.superblockDate = superblockDate
        self.votingCutoff = votingCutoff
        self.votingOpen = votingOpen
        self.budgetCommitted = budgetCommitted
    }
}

/// Proposal list, details, info panel and clock (QT-026, QT-128…130, 134).
/// Errors: `governance.*`.
public protocol GovernanceProviding: AnyObject, Sendable {
    func parameters() -> GovernanceParameters
    func syncState() async throws(ServiceError) -> GovernanceSyncState
    /// On while the Governance tab or the clock is enabled.
    func setSyncEnabled(_ enabled: Bool) async throws(ServiceError)
    /// Engine `Governance` events (≤ 1 Hz): re-query what is shown.
    func changes() -> AsyncStream<Void>
    func proposals(_ query: ProposalQuery) async throws(ServiceError) -> [ProposalRow]
    func detail(hash: String) async throws(ServiceError) -> ProposalDetail
    func info() async throws(ServiceError) -> GovernanceInfo
    func clock() async throws(ServiceError) -> GovernanceClock
}

/// Vote dialog (QT-131). Errors: `governance.*`.
public protocol GovernanceVoting: AnyObject, Sendable {
    /// `wallet == nil`: every wallet's voting keys plus tracked masternodes'.
    func votingMasternodes(proposal hash: String, wallet: WalletID?) async throws(ServiceError) -> [VotingMasternode]
    /// `grant`: `.governance`. One result per masternode.
    func cast(_ outcome: VoteOutcome, on hash: String, with proTxHashes: [String], grant: AuthGrant)
        async throws(ServiceError) -> [VoteResult]
}

/// Create Proposal and Resume Proposals (QT-132, QT-133). Errors:
/// `governance.*`.
public protocol ProposalCreating: AnyObject, Sendable {
    func superblockDates(count: Int) async throws(ServiceError) -> [SuperblockDate]
    /// Failing fields in field order; empty = valid.
    func validate(_ draft: ProposalDraft) async throws(ServiceError) -> [ProposalField]
    func json(_ draft: ProposalDraft) async throws(ServiceError) -> String
    func payloadHex(_ draft: ProposalDraft) async throws(ServiceError) -> String
    /// Pays the 1 DASH collateral; `grant`: `.spend(max: ≥ fee + 1 DASH)`.
    func create(wallet: WalletID, draft: ProposalDraft, grant: AuthGrant) async throws(ServiceError) -> PendingProposal
    func pending(wallet: WalletID) async throws(ServiceError) -> [PendingProposal]
    /// `governance.collateral_unconfirmed` before 1 confirmation.
    func submit(wallet: WalletID, hash: String) async throws(ServiceError) -> String
}
