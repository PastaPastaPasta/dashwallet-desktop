// The status-bar governance clock (QT-026): the moon phase of the voting
// cycle, its tooltip (time left for voting or for the superblock, budget
// committed) and a click that opens the Governance tab. Shown with Display ▸
// Show Governance Tab and Show governance clock; while either the tab or the
// clock is enabled, governance sync is on (m3-swift.md §3).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class GovernanceClockViewModel {
    public private(set) var clock: GovernanceClock?
    public private(set) var syncState: GovernanceSyncState?
    public private(set) var info: GovernanceInfo?
    /// `false` once the engine answered `not_implemented`.
    public private(set) var available = true
    /// Raised by a click; the UI performs it and clears it.
    public var route: AppRoute?

    public let parameters: GovernanceParameters
    public let features: FeatureFlags

    /// dash-qt: governance enabled, peers connected, both options on.
    public var isVisible: Bool {
        let options = desktopPreferences.desktop.options
        return available && features.governance && options.showGovernanceTab && options.showGovernanceClock
            && (syncState?.peers ?? 0) > 0
    }

    /// Animated until governance synced.
    public var isAnimating: Bool { syncState?.phase != .synced }

    /// 0–1 through the cycle (the moon phase); `nil` until known.
    public var cycleProgress: Double? { isAnimating ? nil : clock?.cycleProgress }

    /// Blue in dash-qt: voting closed, waiting for the superblock.
    public var awaitingSuperblock: Bool { clock.map { !$0.votingOpen } ?? false }

    public var tooltipLines: [String] {
        guard let syncState else { return [] }
        guard syncState.phase != .waiting else { return [L10n.Governance.waitingBlockchain] }
        guard syncState.phase == .synced, let clock else { return [L10n.Governance.waitingGovernance] }
        var lines = [Self.cycleLine(clock, parameters: parameters)]
        if let line = Self.budgetLine(clock, info: info, amount: AmountText(amounts: amounts, settings: settings)) {
            lines.append(line)
        }
        lines.append(L10n.Governance.clockOpensGovernance)
        return lines
    }

    private let governance: any GovernanceProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private var task: Task<Void, Never>?
    private var syncOn: Bool?

    public init(
        governance: any GovernanceProviding, desktopPreferences: any DesktopPreferencesStoring,
        amounts: any AmountFormatting, settings: any SettingsProviding, features: FeatureFlags
    ) {
        self.governance = governance
        self.desktopPreferences = desktopPreferences
        self.amounts = amounts
        self.settings = settings
        self.features = features
        parameters = governance.parameters()
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services, features: FeatureFlags) {
        self.init(
            governance: m3.governance, desktopPreferences: m2.desktopPreferences, amounts: env.amounts,
            settings: env.settings, features: features)
    }

    /// Applies the sync switch, loads, and follows `Governance` events.
    public func start() async {
        stop()
        await optionsChanged()
        let changes = governance.changes()
        task = Task { [weak self] in
            for await _ in changes { await self?.reload() }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    /// Display options changed: govsync runs while the Governance tab or the
    /// clock is enabled, so a wallet that never shows governance never
    /// downloads it.
    public func optionsChanged() async {
        let options = desktopPreferences.desktop.options
        let wanted = features.governance && (options.showGovernanceTab || options.showGovernanceClock)
        if wanted != syncOn {
            do {
                try await governance.setSyncEnabled(wanted)
                syncOn = wanted
            } catch {
                if error.isNotImplemented { available = false }
            }
        }
        await reload()
    }

    public func reload() async {
        do {
            syncState = try await governance.syncState()
        } catch {
            if error.isNotImplemented { available = false }
            return
        }
        clock = try? await governance.clock()
        info = try? await governance.info()
    }

    /// Click: open the Governance tab.
    public func activate() {
        route = .section(.governance)
    }

    // MARK: Tooltip lines shared with the proposal list's ⓘ button

    /// "~%1 (%2 blocks) left for voting" / "…left for superblock" /
    /// "Superblock imminent" / "Voting period ended".
    nonisolated static func cycleLine(_ clock: GovernanceClock, parameters: GovernanceParameters) -> String {
        let remaining = clock.blocksToSuperblock
        let awaiting = !clock.votingOpen
        if remaining == 0 { return awaiting ? L10n.Governance.superblockImminent : L10n.Governance.votingEnded }
        if awaiting {
            return L10n.Governance.leftForSuperblock(
                L10n.Durations.blocks(remaining, spacing: parameters.targetSpacing), remaining)
        }
        let voting = remaining > parameters.maturityWindow ? remaining - parameters.maturityWindow : 0
        return L10n.Governance.leftForVoting(L10n.Durations.blocks(voting, spacing: parameters.targetSpacing), voting)
    }

    /// "~%1% of budget committed (%2 / %3)"; `nil` until governance synced.
    @MainActor
    static func budgetLine(_ clock: GovernanceClock, info: GovernanceInfo?, amount: AmountText) -> String? {
        guard let committed = clock.budgetCommitted, let available = info?.budgetAvailable,
            let allocated = info?.budgetAllocated
        else { return nil }
        return L10n.Governance.budgetCommitted(
            percent: Int(committed * 100), allocated: amount(allocated), budget: amount(available))
    }
}
