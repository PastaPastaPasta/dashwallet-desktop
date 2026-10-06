import DashWalletCore
import Foundation

// `EngineClient` wrappers of the M3 governance calls (docs/contracts/m3-engine.md
// §2.2, owner R2) and the DashKit value types they return. Amounts are duffs,
// times UNIX seconds, hashes display-order hex; `nil` = not known (not synced).
// Errors keep the engine's `governance.*` codes (DashKitError+M3.swift).

public struct GovParameters: Sendable, Hashable {
    public let superblockStartHeight: UInt32
    public let superblockCycle: UInt32
    public let maturityWindow: UInt32
    public let minQuorum: UInt32
    public let proposalFee: UInt64
    public let feeConfirmations: UInt32
    public let maxNameLength: UInt32
    public let maxPayloadBytes: UInt32
    public let maxPayments: UInt32
    public let evonodeVoteWeight: UInt32
    public let voteUpdateMinSecs: UInt64
    public let targetSpacingSecs: UInt32
}

public enum GovSyncPhase: Sendable, Hashable {
    case disabled, waiting, syncingObjects, syncingVotes, synced, failed
}

public struct GovSyncState: Sendable, Hashable {
    public let phase: GovSyncPhase
    public let objects: UInt32
    public let votes: UInt64
    public let peers: UInt32
    public let bytesReceived: UInt64
    public let lastSyncedAt: UInt64?
}

public enum GovProposalStatus: Sendable, Hashable {
    case funded, lapsed, confirming, pending, passing, failing, voting, unfunded
}

public enum GovVoteOutcome: Sendable, Hashable {
    case yes, no, abstain
}

public struct GovMyVotes: Sendable, Hashable {
    public let yes: UInt32
    public let no: UInt32
    public let abstain: UInt32
    public let unvoted: UInt32
}

public struct GovProposalRow: Sendable, Hashable {
    public let hash: String
    public let name: String
    public let url: String
    public let paymentAddress: String
    public let paymentAmount: UInt64
    public let startEpoch: UInt64
    public let endEpoch: UInt64
    public let status: GovProposalStatus
    public let collateralConfirmations: UInt32?
    public let yes: UInt32
    public let no: UInt32
    public let abstain: UInt32
    public let margin: Int64
    public let myVotes: GovMyVotes?
}

public struct GovProposalDetail: Sendable, Hashable {
    public let row: GovProposalRow
    public let parentHash: String
    public let collateralTxid: String
    public let createdAt: UInt64
    public let payments: UInt32
    public let rawJSON: String
}

public struct GovVotingMasternode: Sendable, Hashable {
    public let proTxHash: String
    public let collateral: String
    public let votingAddress: String
    public let weight: UInt32
    public let currentVote: GovVoteOutcome?
    public let voteTime: UInt64?
    public let nextVoteAt: UInt64?
    public let label: String?
}

public struct GovVoteResult: Sendable, Hashable {
    public let proTxHash: String
    /// `nil` = relayed; else a `governance.*` code.
    public let errorCode: String?
    public let detail: String?
}

public struct GovSuperblockDate: Sendable, Hashable {
    public let height: UInt32
    public let estimatedTime: UInt64
}

public struct GovProposalDraft: Sendable, Hashable {
    public var name: String
    public var url: String
    public var paymentAddress: String
    public var paymentAmount: UInt64
    public var paymentCount: UInt32
    public var firstSuperblockHeight: UInt32

    public init(
        name: String, url: String, paymentAddress: String, paymentAmount: UInt64, paymentCount: UInt32,
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

/// `ProposalField`, in the engine's order.
public enum GovProposalField: Int, Sendable, Hashable, CaseIterable {
    case name, url, paymentAddress, paymentAmount, paymentCount, firstPayment, payload
}

public enum GovCollateralStatus: Sendable, Hashable {
    case unknown, pending, ready
}

public struct GovPendingProposal: Sendable, Hashable {
    public let hash: String
    public let name: String
    public let url: String
    public let paymentAmount: UInt64
    public let paymentCount: UInt32
    public let collateralTxid: String
    public let collateralStatus: GovCollateralStatus
    public let confirmations: UInt32
    public let createdAt: UInt64
    public let endEpoch: UInt64
}

public struct GovInfo: Sendable, Hashable {
    public let sync: GovSyncState
    public let superblockCycle: UInt32
    public let lastSuperblock: UInt32?
    public let nextSuperblock: UInt32?
    public let nextSuperblockEta: UInt64?
    public let votingCutoff: UInt32?
    public let masternodesVoting: UInt32?
    public let masternodesEligible: UInt32?
    public let evonodesVoting: UInt32?
    public let evonodesEligible: UInt32?
    public let passingThreshold: UInt32?
    public let masternodesControlled: UInt32
    public let votesControlled: UInt32
    public let proposalCount: UInt32?
    public let passing: UInt32?
    public let failing: UInt32?
    public let unfunded: UInt32?
    public let unfundedShort: UInt64?
    public let budgetAvailable: UInt64?
    public let budgetAllocated: UInt64?
}

public struct GovClock: Sendable, Hashable {
    public let cycleProgress: Double
    public let nextSuperblock: UInt32
    public let blocksToSuperblock: UInt32
    public let superblockEta: UInt64
    public let votingCutoff: UInt32
    public let votingOpen: Bool
    public let budgetCommitted: Double?
}

public enum GovProposalSource: Sendable, Hashable {
    case active
    case mine(WalletID)
}

// MARK: - Conversions from the generated bindings

extension GovSyncState {
    init(_ s: DashWalletCore.GovernanceSyncState) {
        let phase: GovSyncPhase =
            switch s.phase {
            case .disabled: .disabled
            case .waiting: .waiting
            case .syncingObjects: .syncingObjects
            case .syncingVotes: .syncingVotes
            case .synced: .synced
            case .failed: .failed
            }
        self.init(
            phase: phase, objects: s.objects, votes: s.votes, peers: s.peers, bytesReceived: s.bytesReceived,
            lastSyncedAt: s.lastSyncedAt)
    }
}

extension GovVoteOutcome {
    init(_ o: DashWalletCore.VoteOutcome) {
        switch o {
        case .yes: self = .yes
        case .no: self = .no
        case .abstain: self = .abstain
        }
    }

    var ffi: DashWalletCore.VoteOutcome {
        switch self {
        case .yes: .yes
        case .no: .no
        case .abstain: .abstain
        }
    }
}

extension GovProposalRow {
    init(_ r: DashWalletCore.ProposalRow) {
        let status: GovProposalStatus =
            switch r.status {
            case .funded: .funded
            case .lapsed: .lapsed
            case .confirming: .confirming
            case .pending: .pending
            case .passing: .passing
            case .failing: .failing
            case .voting: .voting
            case .unfunded: .unfunded
            }
        self.init(
            hash: r.hash, name: r.name, url: r.url, paymentAddress: r.paymentAddress,
            paymentAmount: r.paymentAmount, startEpoch: r.startEpoch, endEpoch: r.endEpoch, status: status,
            collateralConfirmations: r.collateralConfirmations, yes: r.yes, no: r.no, abstain: r.abstain,
            margin: r.margin,
            myVotes: r.myVotes.map { GovMyVotes(yes: $0.yes, no: $0.no, abstain: $0.abstain, unvoted: $0.unvoted) })
    }
}

extension GovProposalDraft {
    var ffi: DashWalletCore.ProposalDraft {
        DashWalletCore.ProposalDraft(
            name: name, url: url, paymentAddress: paymentAddress, paymentAmount: paymentAmount,
            paymentCount: paymentCount, firstSuperblockHeight: firstSuperblockHeight)
    }
}

extension GovProposalField {
    init(_ f: DashWalletCore.ProposalField) {
        self = GovProposalField(rawValue: Int(f.index)) ?? .payload
    }
}

extension GovPendingProposal {
    init(_ p: DashWalletCore.PendingProposal) {
        let status: GovCollateralStatus =
            switch p.collateralStatus {
            case .unknown: .unknown
            case .pending: .pending
            case .ready: .ready
            }
        self.init(
            hash: p.hash, name: p.name, url: p.url, paymentAmount: p.paymentAmount, paymentCount: p.paymentCount,
            collateralTxid: p.collateralTxid, collateralStatus: status, confirmations: p.confirmations,
            createdAt: p.createdAt, endEpoch: p.endEpoch)
    }
}

// MARK: - Calls

extension EngineClient {
    /// `governance_params` (constants; no session needed).
    public nonisolated func governanceParameters(for network: DashNetwork) -> GovParameters {
        let p = DashWalletCore.governanceParams(network: network.ffi)
        return GovParameters(
            superblockStartHeight: p.superblockStartHeight, superblockCycle: p.superblockCycle,
            maturityWindow: p.maturityWindow, minQuorum: p.minQuorum, proposalFee: p.proposalFee,
            feeConfirmations: p.feeConfirmations, maxNameLength: p.maxNameLen, maxPayloadBytes: p.maxPayloadBytes,
            maxPayments: p.maxPayments, evonodeVoteWeight: p.evonodeVoteWeight,
            voteUpdateMinSecs: p.voteUpdateMinSecs, targetSpacingSecs: p.targetSpacingSecs)
    }

    public func governanceSyncState(on network: DashNetwork) throws(DashKitError) -> GovSyncState {
        let session = try session(network)
        return GovSyncState(try mapped { try session.governanceSyncState() })
    }

    public func setGovernanceSyncEnabled(on network: DashNetwork, _ enabled: Bool) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.setGovernanceSyncEnabled(enabled: enabled) }
    }

    public func proposals(on network: DashNetwork, source: GovProposalSource, titleFilter: String?)
        async throws(DashKitError) -> [GovProposalRow]
    {
        let session = try session(network)
        let ffiSource: DashWalletCore.ProposalSource =
            switch source {
            case .active: .active
            case .mine(let wallet): .mine(walletId: wallet.hex)
            }
        let query = DashWalletCore.ProposalQuery(source: ffiSource, titleFilter: titleFilter)
        return try await mapped { try await session.proposals(query: query) }.map(GovProposalRow.init)
    }

    public func proposalDetail(on network: DashNetwork, hash: String) async throws(DashKitError) -> GovProposalDetail {
        let session = try session(network)
        let d = try await mapped { try await session.proposalDetail(hash: hash) }
        return GovProposalDetail(
            row: GovProposalRow(d.row), parentHash: d.parentHash, collateralTxid: d.collateralTxid,
            createdAt: d.createdAt, payments: d.payments, rawJSON: d.rawJson)
    }

    public func votingMasternodes(on network: DashNetwork, hash: String, wallet: WalletID?)
        async throws(DashKitError) -> [GovVotingMasternode]
    {
        let session = try session(network)
        return try await mapped { try await session.votingMasternodes(hash: hash, walletId: wallet?.hex) }.map {
            GovVotingMasternode(
                proTxHash: $0.proTxHash, collateral: $0.collateral, votingAddress: $0.votingAddress,
                weight: $0.weight, currentVote: $0.currentVote.map(GovVoteOutcome.init), voteTime: $0.voteTime,
                nextVoteAt: $0.nextVoteAt, label: $0.label)
        }
    }

    public func castVotes(
        on network: DashNetwork, hash: String, outcome: GovVoteOutcome, proTxHashes: [String], grantID: String
    ) async throws(DashKitError) -> [GovVoteResult] {
        let session = try session(network)
        return try await mapped {
            try await session.castVotes(
                hash: hash, outcome: outcome.ffi, proTxHashes: proTxHashes, grantId: grantID)
        }.map { GovVoteResult(proTxHash: $0.proTxHash, errorCode: $0.errorCode, detail: $0.detail) }
    }

    public func superblockDates(on network: DashNetwork, count: UInt32) throws(DashKitError) -> [GovSuperblockDate] {
        let session = try session(network)
        return try mapped { try session.superblockDates(count: count) }.map {
            GovSuperblockDate(height: $0.height, estimatedTime: $0.estimatedTime)
        }
    }

    public func validateProposal(on network: DashNetwork, _ draft: GovProposalDraft) throws(DashKitError)
        -> [GovProposalField]
    {
        let session = try session(network)
        return try mapped { try session.validateProposal(draft: draft.ffi) }.map(GovProposalField.init)
    }

    public func proposalJSON(on network: DashNetwork, _ draft: GovProposalDraft) throws(DashKitError) -> String {
        let session = try session(network)
        return try mapped { try session.proposalJson(draft: draft.ffi) }
    }

    public func proposalPayloadHex(on network: DashNetwork, _ draft: GovProposalDraft) throws(DashKitError) -> String {
        let session = try session(network)
        return try mapped { try session.proposalPayloadHex(draft: draft.ffi) }
    }

    public func createProposal(on network: DashNetwork, wallet: WalletID, draft: GovProposalDraft, grantID: String)
        async throws(DashKitError) -> GovPendingProposal
    {
        let session = try session(network)
        let ffi = draft.ffi
        return GovPendingProposal(
            try await mapped { try await session.createProposal(walletId: wallet.hex, draft: ffi, grantId: grantID) })
    }

    public func pendingProposals(on network: DashNetwork, wallet: WalletID) async throws(DashKitError)
        -> [GovPendingProposal]
    {
        let session = try session(network)
        return try await mapped { try await session.pendingProposals(walletId: wallet.hex) }.map(
            GovPendingProposal.init)
    }

    public func submitProposal(on network: DashNetwork, wallet: WalletID, hash: String) async throws(DashKitError)
        -> String
    {
        let session = try session(network)
        return try await mapped { try await session.submitProposal(walletId: wallet.hex, hash: hash) }
    }

    public func governanceInfo(on network: DashNetwork) async throws(DashKitError) -> GovInfo {
        let session = try session(network)
        let i = try await mapped { try await session.governanceInfo() }
        return GovInfo(
            sync: GovSyncState(i.sync), superblockCycle: i.superblockCycle, lastSuperblock: i.lastSuperblock,
            nextSuperblock: i.nextSuperblock, nextSuperblockEta: i.nextSuperblockEta, votingCutoff: i.votingCutoff,
            masternodesVoting: i.masternodesVoting, masternodesEligible: i.masternodesEligible,
            evonodesVoting: i.evonodesVoting, evonodesEligible: i.evonodesEligible,
            passingThreshold: i.passingThreshold, masternodesControlled: i.masternodesControlled,
            votesControlled: i.votesControlled, proposalCount: i.proposalCount, passing: i.passing,
            failing: i.failing, unfunded: i.unfunded, unfundedShort: i.unfundedShort,
            budgetAvailable: i.budgetAvailable, budgetAllocated: i.budgetAllocated)
    }

    public func governanceClock(on network: DashNetwork) throws(DashKitError) -> GovClock {
        let session = try session(network)
        let c = try mapped { try session.governanceClock() }
        return GovClock(
            cycleProgress: c.cycleProgress, nextSuperblock: c.nextSuperblock,
            blocksToSuperblock: c.blocksToSuperblock, superblockEta: c.superblockEta, votingCutoff: c.votingCutoff,
            votingOpen: c.votingOpen, budgetCommitted: c.budgetCommitted)
    }
}
