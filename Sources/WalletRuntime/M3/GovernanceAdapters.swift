// Adapters for the M3 governance calls (m3-swift.md §2.2, owner R2):
// `GovernanceProviding`, `GovernanceVoting` and `ProposalCreating` over
// `EngineClient` (DashKit `EngineClient+M3Governance.swift`). One service per
// network, as `M3Services.live(network:…)` builds them; values not synced yet
// stay `nil`.
import DashKit
import Foundation

/// The three governance protocols over the engine of one network.
public final class GovernanceService: GovernanceProviding, GovernanceVoting, ProposalCreating {
    private let client: EngineClient
    private let network: DashNetwork

    public init(client: EngineClient, network: DashNetwork) {
        self.client = client
        self.network = network
    }

    private var kitNetwork: DashKit.DashNetwork { network.kit }

    // MARK: GovernanceProviding

    public func parameters() -> GovernanceParameters {
        let p = client.governanceParameters(for: kitNetwork)
        return GovernanceParameters(
            superblockStartHeight: p.superblockStartHeight, superblockCycle: p.superblockCycle,
            maturityWindow: p.maturityWindow, minQuorum: Int(p.minQuorum),
            proposalFee: Amount(duffs: Int64(clamping: p.proposalFee)), feeConfirmations: Int(p.feeConfirmations),
            maxNameLength: Int(p.maxNameLength), maxPayloadBytes: Int(p.maxPayloadBytes),
            maxPayments: Int(p.maxPayments), evonodeVoteWeight: Int(p.evonodeVoteWeight),
            voteUpdateMinimum: .seconds(Int64(clamping: p.voteUpdateMinSecs)),
            targetSpacing: .seconds(Int64(p.targetSpacingSecs)))
    }

    public func syncState() async throws(ServiceError) -> GovernanceSyncState {
        let client = client
        let network = kitNetwork
        return GovernanceSyncState(
            try await serviceCall { () async throws(DashKitError) in
                try await client.governanceSyncState(on: network)
            })
    }

    public func setSyncEnabled(_ enabled: Bool) async throws(ServiceError) {
        let client = client
        let network = kitNetwork
        try await serviceCall { () async throws(DashKitError) in
            try await client.setGovernanceSyncEnabled(on: network, enabled)
        }
    }

    /// Engine `Governance` events of this network, and `resynchronize`.
    public func changes() -> AsyncStream<Void> {
        let subscription = client.events.subscribe()
        let network = kitNetwork
        let (stream, continuation) = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let pump = Task {
            for await event in subscription {
                if Task.isCancelled { break }
                switch event {
                case .governanceChanged(let n) where n == network:
                    continuation.yield()
                case .resynchronize:
                    continuation.yield()
                default:
                    continue
                }
            }
            continuation.finish()
        }
        continuation.onTermination = { _ in pump.cancel() }
        return stream
    }

    public func proposals(_ query: ProposalQuery) async throws(ServiceError) -> [ProposalRow] {
        let source: GovProposalSource
        switch query.source {
        case .active: source = .active
        case .mine(let wallet): source = .mine(try wallet.kit)
        }
        let client = client
        let network = kitNetwork
        let filter = query.titleFilter
        let rows = try await serviceCall { () async throws(DashKitError) in
            try await client.proposals(on: network, source: source, titleFilter: filter)
        }
        return rows.map(ProposalRow.init)
    }

    public func detail(hash: String) async throws(ServiceError) -> ProposalDetail {
        let client = client
        let network = kitNetwork
        let d = try await serviceCall { () async throws(DashKitError) in
            try await client.proposalDetail(on: network, hash: hash)
        }
        return ProposalDetail(
            row: ProposalRow(d.row), parentHash: d.parentHash, collateralTxid: d.collateralTxid,
            createdAt: Date(unix: d.createdAt), payments: Int(d.payments), rawJSON: d.rawJSON)
    }

    public func info() async throws(ServiceError) -> GovernanceInfo {
        let client = client
        let network = kitNetwork
        let i = try await serviceCall { () async throws(DashKitError) in try await client.governanceInfo(on: network) }
        return GovernanceInfo(
            sync: GovernanceSyncState(i.sync), superblockCycle: i.superblockCycle, lastSuperblock: i.lastSuperblock,
            nextSuperblock: i.nextSuperblock, nextSuperblockDate: i.nextSuperblockEta.map(Date.init(unix:)),
            votingCutoff: i.votingCutoff, masternodesVoting: i.masternodesVoting.map(Int.init),
            masternodesEligible: i.masternodesEligible.map(Int.init), evonodesVoting: i.evonodesVoting.map(Int.init),
            evonodesEligible: i.evonodesEligible.map(Int.init), passingThreshold: i.passingThreshold.map(Int.init),
            masternodesControlled: Int(i.masternodesControlled), votesControlled: Int(i.votesControlled),
            proposalCount: i.proposalCount.map(Int.init), passing: i.passing.map(Int.init),
            failing: i.failing.map(Int.init), unfunded: i.unfunded.map(Int.init),
            unfundedShort: i.unfundedShort.map(Amount.init(engine:)),
            budgetAvailable: i.budgetAvailable.map(Amount.init(engine:)),
            budgetAllocated: i.budgetAllocated.map(Amount.init(engine:)))
    }

    public func clock() async throws(ServiceError) -> GovernanceClock {
        let client = client
        let network = kitNetwork
        let c = try await serviceCall { () async throws(DashKitError) in try await client.governanceClock(on: network) }
        return GovernanceClock(
            cycleProgress: c.cycleProgress, nextSuperblock: c.nextSuperblock, blocksToSuperblock: c.blocksToSuperblock,
            superblockDate: Date(unix: c.superblockEta), votingCutoff: c.votingCutoff, votingOpen: c.votingOpen,
            budgetCommitted: c.budgetCommitted)
    }

    // MARK: GovernanceVoting

    public func votingMasternodes(proposal hash: String, wallet: WalletID?) async throws(ServiceError)
        -> [VotingMasternode]
    {
        let kitWallet = try wallet.map { (w: WalletID) throws(ServiceError) in try w.kit }
        let client = client
        let network = kitNetwork
        let rows = try await serviceCall { () async throws(DashKitError) in
            try await client.votingMasternodes(on: network, hash: hash, wallet: kitWallet)
        }
        return rows.map {
            VotingMasternode(
                proTxHash: $0.proTxHash, collateral: $0.collateral, votingAddress: $0.votingAddress,
                weight: Int($0.weight), currentVote: $0.currentVote.map(VoteOutcome.init),
                voteTime: $0.voteTime.map(Date.init(unix:)), nextVoteAt: $0.nextVoteAt.map(Date.init(unix:)),
                label: $0.label)
        }
    }

    public func cast(_ outcome: VoteOutcome, on hash: String, with proTxHashes: [String], grant: AuthGrant)
        async throws(ServiceError) -> [VoteResult]
    {
        let client = client
        let network = kitNetwork
        let kitOutcome = outcome.kit
        let results = try await serviceCall { () async throws(DashKitError) in
            try await client.castVotes(
                on: network, hash: hash, outcome: kitOutcome, proTxHashes: proTxHashes, grantID: grant.id)
        }
        return results.map {
            VoteResult(proTxHash: $0.proTxHash, errorCode: $0.errorCode.map(ServiceErrorCode.init(rawValue:)))
        }
    }

    // MARK: ProposalCreating

    public func superblockDates(count: Int) async throws(ServiceError) -> [SuperblockDate] {
        let client = client
        let network = kitNetwork
        let n = UInt32(clamping: count)
        let dates = try await serviceCall { () async throws(DashKitError) in
            try await client.superblockDates(on: network, count: n)
        }
        return dates.map { SuperblockDate(height: $0.height, estimatedDate: Date(unix: $0.estimatedTime)) }
    }

    public func validate(_ draft: ProposalDraft) async throws(ServiceError) -> [ProposalField] {
        let client = client
        let network = kitNetwork
        let kitDraft = try draft.kit
        let fields = try await serviceCall { () async throws(DashKitError) in
            try await client.validateProposal(on: network, kitDraft)
        }
        return fields.map { ProposalField.allCases[$0.rawValue] }
    }

    public func json(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        let client = client
        let network = kitNetwork
        let kitDraft = try draft.kit
        return try await serviceCall { () async throws(DashKitError) in try await client.proposalJSON(on: network, kitDraft) }
    }

    public func payloadHex(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        let client = client
        let network = kitNetwork
        let kitDraft = try draft.kit
        return try await serviceCall { () async throws(DashKitError) in
            try await client.proposalPayloadHex(on: network, kitDraft)
        }
    }

    public func create(wallet: WalletID, draft: ProposalDraft, grant: AuthGrant) async throws(ServiceError)
        -> PendingProposal
    {
        let client = client
        let network = kitNetwork
        let kitDraft = try draft.kit
        let id = try wallet.kit
        let p = try await serviceCall { () async throws(DashKitError) in
            try await client.createProposal(on: network, wallet: id, draft: kitDraft, grantID: grant.id)
        }
        return PendingProposal(p)
    }

    public func pending(wallet: WalletID) async throws(ServiceError) -> [PendingProposal] {
        let client = client
        let network = kitNetwork
        let id = try wallet.kit
        let rows = try await serviceCall { () async throws(DashKitError) in
            try await client.pendingProposals(on: network, wallet: id)
        }
        return rows.map(PendingProposal.init)
    }

    public func submit(wallet: WalletID, hash: String) async throws(ServiceError) -> String {
        let client = client
        let network = kitNetwork
        let id = try wallet.kit
        return try await serviceCall { () async throws(DashKitError) in
            try await client.submitProposal(on: network, wallet: id, hash: hash)
        }
    }
}

// MARK: - Conversions

extension Date {
    /// A UNIX-seconds engine time.
    init(unix seconds: UInt64) {
        self.init(timeIntervalSince1970: TimeInterval(seconds))
    }
}

extension Amount {
    /// Engine duffs (≤ 21 M DASH, so they fit).
    init(engine duffs: UInt64) {
        self.init(duffs: Int64(clamping: duffs))
    }
}

extension GovernanceSyncState {
    init(_ s: GovSyncState) {
        let phase: GovernanceSyncPhase =
            switch s.phase {
            case .disabled: .disabled
            case .waiting: .waiting
            case .syncingObjects: .syncingObjects
            case .syncingVotes: .syncingVotes
            case .synced: .synced
            case .failed: .failed
            }
        self.init(
            phase: phase, objects: Int(s.objects), votes: Int(clamping: s.votes), peers: Int(s.peers),
            bytesReceived: s.bytesReceived, lastSyncedAt: s.lastSyncedAt.map(Date.init(unix:)))
    }
}

extension VoteOutcome {
    init(_ o: GovVoteOutcome) {
        switch o {
        case .yes: self = .yes
        case .no: self = .no
        case .abstain: self = .abstain
        }
    }

    var kit: GovVoteOutcome {
        switch self {
        case .yes: .yes
        case .no: .no
        case .abstain: .abstain
        }
    }
}

extension ProposalRow {
    init(_ r: GovProposalRow) {
        let status: ProposalStatus =
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
            paymentAmount: Amount(engine: r.paymentAmount), start: Date(unix: r.startEpoch),
            end: Date(unix: r.endEpoch), status: status,
            collateralConfirmations: r.collateralConfirmations.map(Int.init), yes: Int(r.yes), no: Int(r.no),
            abstain: Int(r.abstain), margin: Int(clamping: r.margin),
            myVotes: r.myVotes.map {
                MyVotes(yes: Int($0.yes), no: Int($0.no), abstain: Int($0.abstain), unvoted: Int($0.unvoted))
            })
    }
}

extension ProposalDraft {
    /// The engine draft; amounts and counts outside the engine's integer
    /// ranges are `invalid_argument`.
    var kit: GovProposalDraft {
        get throws(ServiceError) {
            guard paymentAmount.duffs >= 0, let count = UInt32(exactly: paymentCount) else {
                throw ServiceError(code: .invalidArgument, detail: "payment amount or count out of range")
            }
            return GovProposalDraft(
                name: name, url: url, paymentAddress: paymentAddress, paymentAmount: UInt64(paymentAmount.duffs),
                paymentCount: count, firstSuperblockHeight: firstSuperblockHeight)
        }
    }
}

extension PendingProposal {
    init(_ p: GovPendingProposal) {
        let status: CollateralStatus =
            switch p.collateralStatus {
            case .unknown: .unknown
            case .pending: .pending
            case .ready: .ready
            }
        self.init(
            hash: p.hash, name: p.name, url: p.url, paymentAmount: Amount(engine: p.paymentAmount),
            paymentCount: Int(p.paymentCount), collateralTxid: p.collateralTxid, collateralStatus: status,
            confirmations: Int(p.confirmations), createdAt: Date(unix: p.createdAt), end: Date(unix: p.endEpoch))
    }
}
