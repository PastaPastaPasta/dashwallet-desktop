// The Overview CoinJoin panel (QT-041…044, QT-047…050, QT-112): status,
// balance, "Amount and Rounds", progress with dash-qt's tooltip, the advanced
// fields, Core's session status, and Start/Stop with dash-qt's prompts.
import Foundation
import Observation
import WalletRuntime

/// What the panel can show.
public enum CoinJoinPanelPhase: Sendable, Hashable {
    case loading
    /// The engine does not offer CoinJoin yet ("Not available yet").
    case unavailable
    case ready
    case failed(String)
}

/// The dialogs of dash-qt's `toggleCoinJoin`, in its order.
public enum CoinJoinPanelPrompt: Sendable, Hashable {
    /// First use: suggest the "Most Common" filter. OK continues the toggle.
    case firstUseHint
    /// "CoinJoin requires at least %2 to use."
    case minimumBalance(String)
    /// "Unlock wallet for mixing only": a passphrase, then the start retries.
    case unlockForMixing
    /// "Wallet is locked and user declined to unlock. Disabling CoinJoin."
    case declinedToUnlock

    public var title: String {
        switch self {
        case .unlockForMixing: L10n.CoinJoin.unlockForMixingTitle
        default: L10n.CoinJoin.name
        }
    }

    public var message: String {
        switch self {
        case .firstUseHint: L10n.CoinJoin.mostCommonHint
        case .minimumBalance(let amount): L10n.CoinJoin.minimumBalance(amount)
        case .unlockForMixing: L10n.CoinJoin.unlockForMixingMessage
        case .declinedToUnlock: L10n.CoinJoin.declinedToUnlock
        }
    }
}

@MainActor
@Observable
public final class CoinJoinPanelViewModel {
    public private(set) var phase: CoinJoinPanelPhase = .loading
    public private(set) var status: CoinJoinStatus?
    public private(set) var settings: CoinJoinSettings?
    public private(set) var prompt: CoinJoinPanelPrompt?
    public private(set) var isWorking = false
    public private(set) var errorMessage: String?
    /// The wallet the panel shows; mixing is per wallet (QT-049).
    public private(set) var wallet: WalletID?

    /// Options ▸ CoinJoin ▸ Enable advanced interface (`fShowAdvancedCJUI`).
    public var showAdvanced: Bool { desktopPreferences.desktop.m3.showAdvancedCoinJoinUI }

    /// dash-qt hides the panel while CoinJoin features are disabled.
    public var isVisible: Bool { phase == .unavailable || settings?.enabled == true }

    public var isMixing: Bool { status?.state == .mixing }

    /// "Start CoinJoin" / "Stop CoinJoin" / "(Disabled)".
    public var buttonTitle: String {
        guard let status else { return L10n.CoinJoin.disabledButton }
        if status.state != .idle { return L10n.CoinJoin.stop }
        return status.unavailable == nil ? L10n.CoinJoin.start : L10n.CoinJoin.disabledButton
    }

    /// Stop is always offered while mixing; Start not while unavailable.
    /// Insufficient funds keeps the button so dash-qt's warning can show.
    public var buttonEnabled: Bool {
        guard let status, phase == .ready, !isWorking else { return false }
        if status.state != .idle { return true }
        switch status.unavailable {
        case nil, .insufficientFunds: return true
        case .disabled, .watchOnly: return false
        }
    }

    /// Why the button is disabled.
    public var buttonTooltip: String? {
        switch status?.unavailable {
        case .watchOnly: L10n.CoinJoin.watchOnlyTooltip
        case .disabled: L10n.CoinJoin.disabledTooltip
        case .insufficientFunds(let minimum): L10n.CoinJoin.minimumBalance(amountText(minimum))
        case nil: nil
        }
    }

    /// "Enabled"/"Disabled", plus ", keys left: N" in advanced mode when the
    /// wallet has a keypool (never for our HD wallets).
    public var statusText: String {
        let base = isMixing ? L10n.CoinJoin.enabled : L10n.CoinJoin.disabled
        guard showAdvanced, let keys = status?.keysLeft else { return base }
        return base + ", " + L10n.CoinJoin.keysLeft(keys)
    }

    /// dash-qt shows "keys left" in red below 100.
    public var statusIsWarning: Bool { (status?.keysLeft ?? .max) < 100 }

    /// "CoinJoin Balance": the fully mixed balance (discreet mode hides it).
    public var balanceText: String {
        amountText.privacy(status?.balances.fullyMixed ?? .zero)
    }

    /// "1000 DASH / 4 Rounds", or "~X / N Rounds" (red) without enough
    /// compatible inputs; "No inputs detected" on an empty wallet.
    public var amountAndRoundsText: String {
        guard let status, let settings else { return "" }
        let rounds = L10n.CoinJoin.rounds(settings.rounds)
        if hideBalances { return "#### \(amounts.unitName(self.settingsStore.display.unit)) / \(L10n.CoinJoin.rounds(0))" }
        if walletIsEmpty { return "\(amountText.whole(target(settings))) / \(rounds)" }
        if status.amountAndRounds.insufficientInputs {
            let tilde = self.settingsStore.display.unit == .duffs ? "" : "~"
            return "\(tilde)\(amountText.whole(status.amountAndRounds.amount)) / \(rounds)"
        }
        return "\(amountText.whole(status.amountAndRounds.amount)) / \(rounds)"
    }

    public var amountAndRoundsIsWarning: Bool {
        !hideBalances && !walletIsEmpty && (status?.amountAndRounds.insufficientInputs ?? false)
    }

    public var amountAndRoundsTooltip: String? {
        guard let status, let settings, !hideBalances else { return nil }
        if walletIsEmpty { return L10n.CoinJoin.noInputsDetected }
        if status.amountAndRounds.insufficientInputs {
            return L10n.CoinJoin.notEnoughInputs(
                target: amountText(target(settings)), instead: amountText(status.amountAndRounds.amount))
        }
        return L10n.CoinJoin.enoughInputs(amountText(target(settings)))
    }

    /// "Completion" (advanced mode), 0–100.
    public var progressPercent: Int {
        guard let status, !walletIsEmpty else { return 0 }
        return Int(min(100, max(0, status.progress.overall)).rounded(.down))
    }

    /// The progress bar's tooltip lines (dash-qt `updateCoinJoinProgress`).
    public var progressTooltipLines: [String] {
        guard let status, let settings else { return [] }
        if walletIsEmpty { return [L10n.CoinJoin.noInputsDetected] }
        let p = status.progress
        return [
            "\(L10n.CoinJoin.overallProgress): \(Self.percent(p.overall))%",
            "\(L10n.CoinJoin.denominated): \(Self.percent(p.denominated))%",
            "\(L10n.CoinJoin.partiallyMixed): \(Self.percent(p.partiallyMixed))%",
            "\(L10n.CoinJoin.mixed): \(Self.percent(p.mixed))%",
            L10n.CoinJoin.averageRounds(String(format: "%.2f", p.averageRounds), of: settings.rounds),
        ]
    }

    /// "Submitted Denom" (advanced mode): `1.00001; 0.100001;` or "n/a".
    public var submittedDenominationsText: String {
        guard let denominations = status?.submittedDenominations, !denominations.isEmpty else {
            return L10n.CoinJoin.notApplicable
        }
        return denominations.map {
            amounts.format($0, unit: .dash, style: .plain(plusSign: false, separators: .never))
                .replacingOccurrences(of: "0+$", with: "", options: .regularExpression) + ";"
        }.joined(separator: " ")
    }

    /// Core's `coinjoin status` line (QT-050, beyond dash-qt's GUI).
    public var sessionStatusText: String? {
        guard let status, status.state != .idle else { return nil }
        return L10n.CoinJoin.status(status.status)
    }

    /// One line per open session: masternode, denomination, state, entries.
    public var sessionLines: [String] {
        (status?.sessions ?? []).map { session in
            var parts: [String] = []
            if let service = session.service { parts.append(service) }
            if let denomination = session.denomination {
                parts.append(amounts.format(denomination, unit: .dash, style: .withUnit(plusSign: false, separators: .never)))
            }
            parts.append(L10n.CoinJoin.poolState(session.state))
            if let message = session.lastMessage { parts.append(L10n.CoinJoin.poolMessage(message)) }
            return parts.joined(separator: " · ")
        }
    }

    public var queueSizeText: String? {
        guard let status, status.state != .idle else { return nil }
        return "\(L10n.CoinJoin.queueSize): \(status.queueSize)"
    }

    /// A stop the user did not ask for, e.g. the vault locked (QT-112).
    public var stopNotice: String? {
        guard let status, status.state == .idle, status.stopReason == .vaultLocked else { return nil }
        return L10n.CoinJoin.mixingStoppedLocked
    }

    private let coinJoin: any CoinJoinControlling
    private let walletState: any WalletStateProviding
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let amounts: any AmountFormatting
    private let settingsStore: any SettingsProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private var tasks: [Task<Void, Never>] = []
    /// The toggle waits for the first-use hint to be acknowledged.
    private var resumeAfterHint = false

    public init(
        coinJoin: any CoinJoinControlling, walletState: any WalletStateProviding, auth: any AuthenticationGating,
        vault: any VaultProviding, amounts: any AmountFormatting, settings: any SettingsProviding,
        desktopPreferences: any DesktopPreferencesStoring
    ) {
        self.coinJoin = coinJoin
        self.walletState = walletState
        self.auth = auth
        self.vault = vault
        self.amounts = amounts
        self.settingsStore = settings
        self.desktopPreferences = desktopPreferences
        wallet = walletState.selectedWalletID
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            coinJoin: m3.coinJoin, walletState: env.walletState, auth: env.auth, vault: env.vault,
            amounts: env.amounts, settings: env.settings, desktopPreferences: m2.desktopPreferences)
    }

    private var amountText: AmountText { AmountText(amounts: amounts, settings: settingsStore) }
    private var hideBalances: Bool { settingsStore.display.hideBalances }
    /// dash-qt's "balance == 0" branch: every CoinJoin balance is zero.
    private var walletIsEmpty: Bool {
        guard let b = status?.balances else { return true }
        return b.anonymizable == .zero && b.fullyMixed == .zero && b.denominated == .zero
    }

    private func target(_ settings: CoinJoinSettings) -> Amount {
        Amount(duffs: Int64(settings.targetAmountDash) * Amount.duffsPerDash)
    }

    private static func percent(_ value: Double) -> String {
        String(format: "%.2f", min(100, max(0, value)))
    }

    // MARK: Observation

    /// Loads, then follows the wallet's CoinJoin events (other wallets'
    /// events are ignored, QT-049) and wallet switches until `stop()`.
    public func start() async {
        stop()
        await reload()
        let changes = coinJoin.statusChanges()
        let walletChanges = walletState.changes()
        tasks.append(Task { [weak self] in
            for await id in changes {
                guard let self, id == self.wallet else { continue }
                await self.reloadStatus()
            }
        })
        tasks.append(Task { [weak self] in
            for await _ in walletChanges {
                guard let self, self.walletState.selectedWalletID != self.wallet else { continue }
                self.prompt = nil
                await self.reload()
            }
        })
    }

    public func stop() {
        tasks.forEach { $0.cancel() }
        tasks = []
    }

    /// Reads the options and the selected wallet's status.
    public func reload() async {
        wallet = walletState.selectedWalletID
        do {
            settings = try await coinJoin.settings()
        } catch {
            fail(error)
            return
        }
        await reloadStatus()
    }

    private func reloadStatus() async {
        guard let wallet else {
            status = nil
            phase = .ready
            return
        }
        do {
            let next = try await coinJoin.status(wallet: wallet)
            guard next.wallet == self.wallet else { return }
            status = next
            phase = .ready
        } catch {
            fail(error)
        }
    }

    private func fail(_ error: ServiceError) {
        if error.isNotImplemented {
            phase = .unavailable
        } else {
            phase = .failed(ErrorText.m3(error, amount: amountText.callAsFunction))
        }
    }

    // MARK: Start / Stop (QT-044)

    /// The Start/Stop button: dash-qt's `toggleCoinJoin`.
    public func toggle() async {
        guard buttonEnabled, let status else { return }
        errorMessage = nil
        if !desktopPreferences.desktop.m3.coinJoinHintShown {
            desktopPreferences.updateM3 { $0.coinJoinHintShown = true }
            resumeAfterHint = true
            prompt = .firstUseHint
            return
        }
        if status.state != .idle {
            await stopMixing()
        } else {
            await startMixing()
        }
    }

    /// OK on the first-use hint: the toggle continues.
    public func acknowledgeHint() async {
        guard prompt == .firstUseHint else { return }
        prompt = nil
        guard resumeAfterHint else { return }
        resumeAfterHint = false
        await toggle()
    }

    /// Closes a warning prompt.
    public func dismissPrompt() {
        switch prompt {
        case .unlockForMixing:
            declineUnlock()
        case .firstUseHint:
            prompt = nil
            resumeAfterHint = false
        default:
            prompt = nil
        }
    }

    /// Cancel on "Unlock wallet for mixing only".
    public func declineUnlock() {
        guard prompt == .unlockForMixing else { return }
        prompt = .declinedToUnlock
    }

    /// The passphrase for "Unlock wallet for mixing only" (QT-112); the
    /// start retries once unlocked. A wrong passphrase keeps the prompt.
    public func unlockForMixing(passphrase: String) async {
        guard prompt == .unlockForMixing else { return }
        guard !passphrase.isEmpty else {
            errorMessage = L10n.Common.passphraseRequired
            return
        }
        isWorking = true
        defer { isWorking = false }
        do {
            try await auth.unlock(passphrase: vault.makeSecret(utf8: passphrase), scope: .mixingOnly)
        } catch {
            errorMessage = ErrorText.common(error.code)
            return
        }
        prompt = nil
        errorMessage = nil
        isWorking = false
        await startMixing()
    }

    private func startMixing() async {
        guard let wallet, let status else { return }
        let minimum = coinJoin.limits().minimumMixingBalance
        if case .insufficientFunds(let required) = status.unavailable {
            prompt = .minimumBalance(amountText(required))
            return
        }
        if let balance = walletState.balances?.confirmed, balance < minimum {
            prompt = .minimumBalance(amountText(minimum))
            return
        }
        if auth.lockState == .locked {
            prompt = .unlockForMixing
            return
        }
        isWorking = true
        defer { isWorking = false }
        do {
            try await coinJoin.start(wallet: wallet)
        } catch {
            switch error.code {
            case .coinjoinVaultLocked:
                prompt = .unlockForMixing
            case .coinjoinInsufficientFunds:
                let required = error.parameters["min_duffs"].map { Amount(duffs: $0) } ?? minimum
                prompt = .minimumBalance(amountText(required))
            default:
                errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
            }
            return
        }
        await reloadStatus()
    }

    /// Stop: Core resets the pool, then stops (the engine's `stop_mixing`).
    private func stopMixing() async {
        guard let wallet else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            try await coinJoin.stop(wallet: wallet)
        } catch {
            errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
            return
        }
        await reloadStatus()
    }
}
