// Tools ▸ Information ▸ Governance (QT-134, dash-qt `proposalinfo.ui`):
// cycles, superblocks, voting cutoff, participation, threshold, what this
// wallet controls, proposal counts and the budget. "N/A" until synced.
import Foundation
import Observation
import WalletRuntime

public struct InfoSection: Sendable, Hashable, Identifiable {
    public var id: String { title }
    public let title: String
    public let lines: [DetailLine]
}

@MainActor
@Observable
public final class GovernanceInfoViewModel {
    public private(set) var info: GovernanceInfo?
    public private(set) var available = true
    public private(set) var errorMessage: String?
    public let parameters: GovernanceParameters

    /// 0–1 of the budget allocated (the donut chart); `nil` until synced.
    public var budgetFraction: Double? {
        guard let info, let available = info.budgetAvailable, let allocated = info.budgetAllocated,
            available > .zero
        else { return nil }
        return min(1, Double(allocated.duffs) / Double(available.duffs))
    }

    public var syncText: String? { info.map { L10n.Governance.syncPhase($0.sync.phase) } }

    public var sections: [InfoSection] {
        typealias G = L10n.Governance
        let na = G.notAvailable
        let info = self.info
        let spacing = parameters.targetSpacing
        func date(_ d: Date?) -> String? { d.map { M3Dates.dateTime($0, timing: timing) } }
        let cycle = info?.superblockCycle ?? parameters.superblockCycle
        var general = [
            DetailLine(G.votingCycles, G.cycleBlocks(cycle, L10n.Durations.blocks(cycle, spacing: spacing))),
            DetailLine(G.lastSuperblock, info?.lastSuperblock.map { "\($0)" } ?? na),
            DetailLine(
                G.nextSuperblock,
                info?.nextSuperblock.map { G.superblockAt($0, date(info?.nextSuperblockDate)) } ?? na),
        ]
        general.append(DetailLine(G.votingCutoff, info?.votingCutoff.map { "\($0)" } ?? na))
        func participation(_ voting: Int?, _ eligible: Int?) -> String {
            guard let voting, let eligible else { return na }
            return G.participationValue(voting, eligible: eligible)
        }
        let participationLines = [
            DetailLine(G.masternodesVoting, participation(info?.masternodesVoting, info?.masternodesEligible)),
            DetailLine(G.evonodesVoting, participation(info?.evonodesVoting, info?.evonodesEligible)),
            DetailLine(G.passingThreshold, info?.passingThreshold.map { "\($0)" } ?? na),
        ]
        let node = [
            DetailLine(G.masternodesControlled, info.map { "\($0.masternodesControlled)" } ?? na),
            DetailLine(G.votesControlled, info.map { "\($0.votesControlled)" } ?? na),
        ]
        var budget = na
        if let available = info?.budgetAvailable, let allocated = info?.budgetAllocated {
            budget = G.budgetValue(
                allocated: amount(allocated), available: amount(available),
                percent: Int((budgetFraction ?? 0) * 100))
        }
        let proposals = [
            DetailLine(G.proposalCountTitle, info?.proposalCount.map { "\($0)" } ?? na),
            DetailLine(G.budgetAllocated, budget),
            DetailLine(G.passingProposals, info?.passing.map { "\($0)" } ?? na),
            DetailLine(
                G.unfundedProposals,
                info?.unfunded.map { G.unfundedValue($0, short: info?.unfundedShort.map(amount)) } ?? na),
            DetailLine(G.failingProposals, info?.failing.map { "\($0)" } ?? na),
        ]
        return [
            InfoSection(title: G.infoGeneral, lines: general),
            InfoSection(title: G.participation, lines: participationLines),
            InfoSection(title: G.node, lines: node),
            InfoSection(title: G.proposalsSection, lines: proposals),
        ]
    }

    private let governance: any GovernanceProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let timing: Timing
    private var task: Task<Void, Never>?

    public init(
        governance: any GovernanceProviding, amounts: any AmountFormatting, settings: any SettingsProviding,
        timing: Timing
    ) {
        self.governance = governance
        self.amounts = amounts
        self.settings = settings
        self.timing = timing
        parameters = governance.parameters()
    }

    public convenience init(env: AppEnvironment, m3: M3Services) {
        self.init(governance: m3.governance, amounts: env.amounts, settings: env.settings, timing: env.timing)
    }

    private func amount(_ value: Amount) -> String { AmountText(amounts: amounts, settings: settings)(value) }

    public func start() async {
        stop()
        await reload()
        let changes = governance.changes()
        task = Task { [weak self] in
            for await _ in changes { await self?.reload() }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    public func reload() async {
        do {
            info = try await governance.info()
            available = true
        } catch {
            if error.isNotImplemented {
                available = false
            } else {
                errorMessage = ErrorText.m3(error, amount: amount)
            }
        }
    }
}
