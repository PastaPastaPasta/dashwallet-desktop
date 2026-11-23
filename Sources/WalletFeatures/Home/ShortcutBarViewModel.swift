// iOS shortcut bar (IOS-025), backup reminder (IOS-005) and the testnet web
// faucet shortcut (IOS-121).
import Foundation
import Observation
import WalletRuntime

/// iOS `ShortcutActionType` entries a desktop wallet can show. Raw values
/// are persisted in `DesktopPreferences.shortcuts`.
public enum ShortcutAction: String, Sendable, Hashable, CaseIterable, Codable {
    case backup, receive, send, scanQR, sendToAddress, buySell, explore, spend, atm, coinbase, uphold, topper,
        dashDEX, crowdNode, testnetFaucet, switchWallet, nodes

    public var title: String {
        switch self {
        case .backup: L10n.HomeM2.backup
        case .receive: L10n.HomeM2.receive
        case .send: L10n.HomeM2.send
        case .scanQR: L10n.HomeM2.scanQR
        case .sendToAddress: L10n.HomeM2.sendToAddress
        case .buySell: L10n.HomeM2.buySell
        case .explore: L10n.HomeM2.explore
        case .spend: L10n.HomeM2.spend
        case .atm: L10n.HomeM2.atm
        case .coinbase: L10n.HomeM2.coinbase
        case .uphold: L10n.HomeM2.uphold
        case .topper: L10n.HomeM2.topper
        case .dashDEX: L10n.HomeM2.dashDEX
        case .crowdNode: L10n.HomeM2.crowdNode
        case .testnetFaucet: L10n.HomeM2.testDash
        case .switchWallet: L10n.HomeM2.switchWallet
        case .nodes: L10n.HomeM2.nodes
        }
    }

    /// Integrations (M5) and masternode tooling (M3) are shown disabled.
    var arrivesLater: Bool {
        switch self {
        case .buySell, .explore, .spend, .atm, .coinbase, .uphold, .topper, .dashDEX, .crowdNode, .nodes: true
        case .backup, .receive, .send, .scanQR, .sendToAddress, .testnetFaucet, .switchWallet: false
        }
    }
}

/// What a shortcut opens; the UI performs it.
public enum ShortcutRoute: Sendable, Hashable {
    case section(SidebarItem)
    case shell(ShellCommand)
    /// Reads a QR code from an image file or the clipboard (IOS-043).
    case scanQR
    case openURL(URL)
    case switchWallet
}

public struct ShortcutSlot: Sendable, Hashable, Identifiable {
    public var id: Int { index }
    public let index: Int
    public let action: ShortcutAction
    public let isEnabled: Bool
    /// Why the action is disabled.
    public let helpText: String?
}

/// The testnet web faucet (IOS-121). The in-app PoW faucet is M5 AppServices work.
public enum FaucetShortcut {
    public static let webFaucet = URL(string: "https://faucet.testnet.networks.dash.org/")!
    public static let inAppFaucetAvailable = false

    /// The faucet page on testnet; `nil` on every other network.
    public static func url(for network: DashNetwork?) -> URL? {
        network == .testnet ? webFaucet : nil
    }
}

@MainActor
@Observable
public final class ShortcutBarViewModel {
    public static let slotCount = 4

    public let network: DashNetwork
    public private(set) var slots: [ShortcutSlot] = []
    public private(set) var errorMessage: String?

    /// Actions the customisation picker offers (iOS `customizableActions`).
    public var customizableActions: [ShortcutAction] {
        var actions: [ShortcutAction] = [
            .buySell, .explore, .spend, .atm, .receive, .send, .scanQR, .sendToAddress, .coinbase, .uphold, .topper,
        ]
        if network == .testnet { actions.insert(.testnetFaucet, at: 0) }
        if walletCount > 1 { actions.append(.switchWallet) }
        return actions
    }

    private var walletCount: Int { walletState.wallets?.count ?? 0 }
    private let walletState: any WalletStateProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private var task: Task<Void, Never>?

    public init(
        walletState: any WalletStateProviding, desktopPreferences: any DesktopPreferencesStoring, network: DashNetwork
    ) {
        self.walletState = walletState
        self.desktopPreferences = desktopPreferences
        self.network = network
        reload()
    }

    public convenience init(env: AppEnvironment, m2: M2Services, network: DashNetwork) {
        self.init(walletState: env.walletState, desktopPreferences: m2.desktopPreferences, network: network)
    }

    /// Rebuilds the bar on wallet and balance changes until `stop()`.
    public func start() {
        stop()
        let changes = walletState.changes()
        task = Task { [weak self] in
            for await _ in changes {
                self?.reload()
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    /// The saved bar (degraded where its actions do not apply), else iOS's
    /// state-dependent defaults.
    public func reload() {
        let actions: [ShortcutAction]
        if let saved = desktopPreferences.desktop.shortcuts, saved.count == Self.slotCount {
            actions = saved.map(degrade)
        } else {
            actions = defaults()
        }
        slots = actions.enumerated().map { index, action in
            let reason = disabledReason(action)
            return ShortcutSlot(index: index, action: action, isEnabled: reason == nil, helpText: reason)
        }
    }

    /// Long-press customisation: puts `action` into slot `index`.
    public func setSlot(_ index: Int, to action: ShortcutAction) {
        guard slots.indices.contains(index), customizableActions.contains(action) else { return }
        var actions = slots.map(\.action)
        actions[index] = action
        save(actions)
    }

    public func resetToDefaults() {
        save(nil)
    }

    /// The route of an enabled slot; `nil` for a disabled one.
    public func route(for slot: ShortcutSlot) -> ShortcutRoute? {
        guard slot.isEnabled else { return nil }
        switch slot.action {
        case .backup: return .shell(.showRecoveryPhrase)
        case .receive: return .section(.receive)
        case .send, .sendToAddress: return .section(.send)
        case .scanQR: return .scanQR
        case .testnetFaucet: return FaucetShortcut.url(for: network).map(ShortcutRoute.openURL)
        case .switchWallet: return .switchWallet
        default: return nil
        }
    }

    // MARK: Private

    /// iOS `applyDefaultShortcuts`: four states from balance and backup.
    private func defaults() -> [ShortcutAction] {
        let lastSlot: ShortcutAction = network == .testnet ? .testnetFaucet : .spend
        let hasBalance = (walletState.balances?.total.duffs ?? 0) > 0
        let needsBackup = walletNeedsBackup
        switch (hasBalance, needsBackup) {
        case (false, true): return [.backup, .receive, .buySell, lastSlot]
        case (false, false): return [.receive, .send, .buySell, lastSlot]
        case (true, false): return [.receive, .send, .scanQR, lastSlot]
        case (true, true): return [.backup, .receive, .send, lastSlot]
        }
    }

    /// iOS degrade rules for saved actions that no longer apply; the saved
    /// configuration itself is kept.
    private func degrade(_ action: ShortcutAction) -> ShortcutAction {
        switch action {
        case .testnetFaucet where network != .testnet: .spend
        case .switchWallet where walletCount <= 1: .receive
        case .dashDEX where network != .mainnet: .spend
        case .nodes: network == .testnet ? .testnetFaucet : .spend
        default: action
        }
    }

    private var walletNeedsBackup: Bool {
        guard let id = walletState.selectedWalletID else { return false }
        return BackupReminderViewModel.needsBackup(desktopPreferences.desktop.backupReminders[id.hex])
    }

    private func disabledReason(_ action: ShortcutAction) -> String? {
        if action.arrivesLater { return L10n.HomeM2.laterRelease }
        if action == .backup,
            let id = walletState.selectedWalletID,
            walletState.wallets?.first(where: { $0.id == id })?.hasMnemonic != true
        {
            return L10n.Shell.noRecoveryPhrase
        }
        if action == .testnetFaucet, network != .testnet { return L10n.HomeM2.laterRelease }
        return nil
    }

    private func save(_ actions: [ShortcutAction]?) {
        var stored = desktopPreferences.desktop
        stored.shortcuts = actions
        do {
            try desktopPreferences.update(stored)
            errorMessage = nil
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
        reload()
    }
}

/// The 24-hour backup reminder (IOS-005, iOS `shouldShowWalletBackupReminder`):
/// due once, 24 h after a wallet created here first held funds, while its
/// phrase was never revealed or exported.
@MainActor
@Observable
public final class BackupReminderViewModel {
    public static let delay: TimeInterval = 24 * 60 * 60

    /// The reminder for the selected wallet is due.
    public private(set) var isDue = false

    private let walletState: any WalletStateProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private let timing: Timing
    private var task: Task<Void, Never>?

    public init(walletState: any WalletStateProviding, desktopPreferences: any DesktopPreferencesStoring, timing: Timing) {
        self.walletState = walletState
        self.desktopPreferences = desktopPreferences
        self.timing = timing
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(walletState: env.walletState, desktopPreferences: m2.desktopPreferences, timing: env.timing)
    }

    /// `true` while a tracked wallet's phrase is not backed up.
    nonisolated static func needsBackup(_ state: BackupReminderState?) -> Bool {
        guard let state else { return false }
        return !state.backedUp
    }

    /// Records that `id`'s phrase was shown or exported, from flows that do
    /// not hold this view model (Show Recovery Phrase, export for dash-qt).
    static func recordBackup(_ id: WalletID, in store: any DesktopPreferencesStoring) {
        var stored = store.desktop
        var state = stored.backupReminders[id.hex] ?? BackupReminderState()
        guard !state.backedUp else { return }
        state.backedUp = true
        stored.backupReminders[id.hex] = state
        try? store.update(stored)
    }

    /// Follows balance changes until `stop()`.
    public func start() {
        stop()
        evaluate()
        let changes = walletState.changes()
        task = Task { [weak self] in
            for await _ in changes {
                self?.evaluate()
            }
        }
    }

    public func stop() {
        task?.cancel()
        task = nil
    }

    /// A wallet created with a new phrase (not restored) starts unbacked, as
    /// iOS sets `walletNeedsBackup` on create only.
    public func walletCreated(_ id: WalletID) {
        update(id) { $0 = BackupReminderState() }
    }

    /// The phrase was shown or exported: no reminder, no Backup default.
    public func markBackedUp(_ id: WalletID) {
        update(id) { $0.backedUp = true }
    }

    /// The reminder was shown; it is not shown again.
    public func markShown() {
        guard let id = walletState.selectedWalletID else { return }
        update(id) { $0.reminderShown = true }
    }

    /// Records the first funds and recomputes `isDue` (call after time passes).
    public func evaluate() {
        guard let id = walletState.selectedWalletID,
            var state = desktopPreferences.desktop.backupReminders[id.hex]
        else {
            isDue = false
            return
        }
        if !state.backedUp, state.firstFundsAt == nil, (walletState.balances?.total.duffs ?? 0) > 0 {
            state.firstFundsAt = timing.now()
            update(id) { $0 = state }
        }
        guard !state.backedUp, !state.reminderShown, let first = state.firstFundsAt else {
            isDue = false
            return
        }
        isDue = timing.now().timeIntervalSince(first) > Self.delay
    }

    private func update(_ id: WalletID, _ change: (inout BackupReminderState) -> Void) {
        var stored = desktopPreferences.desktop
        var state = stored.backupReminders[id.hex] ?? BackupReminderState()
        change(&state)
        stored.backupReminders[id.hex] = state
        try? desktopPreferences.update(stored)
        if id == walletState.selectedWalletID, state.backedUp || state.reminderShown { isDue = false }
    }
}
