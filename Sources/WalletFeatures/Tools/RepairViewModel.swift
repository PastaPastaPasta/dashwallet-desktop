// Tools ▸ Repair and sync info (QT-117, QT-148, IOS-034, IOS-113): rescan
// from the wallet birthday or genesis with progress and cancel, reset chain
// data (with a confirmation dash-qt omits), drop unconfirmed transactions
// and edit the birth height.
import Foundation
import Observation
import WalletRuntime

/// A destructive Repair action waiting for Yes / Cancel.
public enum RepairConfirmation: Sendable, Hashable {
    case resetChainData
    case dropUnconfirmed

    public var message: String {
        switch self {
        case .resetChainData: L10n.Tools.resetChainDataQuestion
        case .dropUnconfirmed: L10n.Tools.dropUnconfirmedQuestion
        }
    }
}

/// The Repair tab's flow.
public enum RepairState: Sendable, Hashable {
    case idle
    case confirming(RepairConfirmation)
    case working
    case done(String)
    case failed(String)
}

@MainActor
@Observable
public final class RepairViewModel {
    public private(set) var state: RepairState = .idle
    /// `nil` while no rescan runs.
    public private(set) var progress: RescanProgress?

    public var isRescanning: Bool { progress != nil }

    /// "Rescanning… current / target"; `nil` while idle or unknown.
    public var progressText: String? {
        guard let progress, let current = progress.currentHeight, let target = progress.targetHeight else { return nil }
        return L10n.Tools.rescanProgress(current: current, target: target)
    }

    /// 0...1 within the rescan; `nil` when unknown.
    public var progressFraction: Double? {
        guard let progress, let current = progress.currentHeight, let target = progress.targetHeight,
            target > progress.fromHeight
        else { return nil }
        let done = Double(current.saturatingSubtract(progress.fromHeight))
        return min(1, done / Double(target - progress.fromHeight))
    }

    private let sync: any SyncStatusProviding
    private let repair: any RepairProviding
    private let actions: any TransactionActing
    private let walletState: any WalletStateProviding
    private var task: Task<Void, Never>?

    public init(
        sync: any SyncStatusProviding, repair: any RepairProviding, actions: any TransactionActing,
        walletState: any WalletStateProviding
    ) {
        self.sync = sync
        self.repair = repair
        self.actions = actions
        self.walletState = walletState
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(sync: env.sync, repair: m2.repair, actions: m2.transactionActions, walletState: env.walletState)
    }

    /// Refreshes the progress on every sync change until `stop()`.
    public func start() {
        stop()
        let changes = sync.changes()
        task = Task { [weak self] in
            for await _ in changes {
                await self?.refresh()
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    public func refresh() async {
        do {
            progress = try await repair.rescanProgress()
        } catch {
            progress = nil
            if error.code != .notImplemented { state = .failed(ErrorText.m2(error.code)) }
        }
    }

    /// "Rescan Chain" (`.walletBirth`) / "Rescan Chain (full)" (`.genesis`).
    public func rescan(_ start: RescanStart) async {
        guard state != .working else { return }
        state = .working
        do {
            try await sync.rescan(from: start)
            state = .idle
        } catch {
            state = .failed(
                error.code == .syncRescanInProgress
                    ? "\(L10n.Tools.rescanUnavailable): \(L10n.M2Errors.rescanInProgress)" : ErrorText.m2(error.code))
        }
        await refresh()
    }

    /// `abortrescan`: stops and keeps what was found.
    public func cancelRescan() async {
        do {
            _ = try await repair.cancelRescan()
        } catch {
            state = .failed(ErrorText.m2(error.code))
        }
        await refresh()
    }

    public func requestResetChainData() {
        guard state != .working else { return }
        state = .confirming(.resetChainData)
    }

    /// IOS-034 bulk "remove unconfirmed and rescan", for every wallet.
    public func requestDropUnconfirmed() {
        guard state != .working else { return }
        state = .confirming(.dropUnconfirmed)
    }

    public func cancelConfirmation() {
        if case .confirming = state { state = .idle }
    }

    public func confirm() async {
        guard case .confirming(let confirmation) = state else { return }
        state = .working
        do {
            switch confirmation {
            case .resetChainData:
                try await repair.resetChainData()
                state = .idle
            case .dropUnconfirmed:
                let count = try await actions.dropUnconfirmed(wallet: nil)
                state = .done(L10n.Tools.droppedUnconfirmed(count))
            }
        } catch {
            state = .failed(ErrorText.m2(error.code))
        }
        await refresh()
    }

    /// IOS-113: a lower height schedules a rescan from it.
    public func setBirthHeight(_ text: String) async {
        guard let wallet = walletState.selectedWalletID else { return }
        guard let height = UInt32(text.trimmingCharacters(in: .whitespaces)) else {
            state = .failed(L10n.Tools.birthHeightInvalid)
            return
        }
        state = .working
        do {
            try await repair.setBirthHeight(wallet: wallet, height: height)
            state = .idle
        } catch {
            state = .failed(ErrorText.m2(error.code))
        }
    }

    public func dismissResult() {
        switch state {
        case .done, .failed: state = .idle
        default: break
        }
    }
}

extension UInt32 {
    fileprivate func saturatingSubtract(_ other: UInt32) -> UInt32 { self > other ? self - other : 0 }
}
