// dash-qt coin control (QT-068…074): the Send-page panel values and the Coin
// Selection dialog (list/tree, sort, CoinJoin filter, locks, copy menu),
// custom change address checks and the spent-coin notice.
//
// The selection is the one source of truth for the Send page it is attached
// to (`SendViewModel.attach`, review M3): Send reads `source()` when it builds
// its draft, a user change of the selection ends a review in progress, and
// while the payment is broadcasting (or its outcome is unknown) a change is
// refused with `errorMessage` instead of being dropped silently.
import Foundation
import Observation
import WalletRuntime

/// UTXO context-menu copy entries (QT-070).
public enum CoinCopyField: Sendable, Hashable, CaseIterable {
    case address, label, amount, outpoint
}

/// Panel values a right-click copies (QT-068).
public enum CoinSummaryField: Sendable, Hashable, CaseIterable {
    case quantity, amount, fee, afterFee, bytes, change
}

/// Custom change address problems (QT-073).
public enum CustomChangeWarning: Sendable, Hashable {
    /// "Warning: Invalid Dash address"; the address is not used.
    case invalidAddress
    /// "Warning: Unknown change address"; used only after the user confirms.
    case unknownAddress(confirmed: Bool)

    public var text: String {
        switch self {
        case .invalidAddress: L10n.CoinControl.invalidChangeAddress
        case .unknownAddress: L10n.CoinControl.unknownChangeAddress
        }
    }
}

/// Coins of one address in tree mode.
public struct CoinAddressGroup: Sendable, Hashable, Identifiable {
    public var id: String { address }
    public let address: String
    public let label: String
    public let coins: [Utxo]
    public let total: Amount
}

/// The panel's text values; `≈` marks estimates (dash-qt).
public struct CoinSummaryText: Sendable, Hashable {
    public let quantity: String
    public let amount: String
    public let bytes: String
    public let fee: String
    public let afterFee: String
    public let change: String
    public let tolerance: String
}

@MainActor
@Observable
public final class CoinControlViewModel {
    public static let approximately = "≈"

    public private(set) var mode: CoinControlMode
    public private(set) var sort: CoinSort
    public private(set) var allCoins: [Utxo] = []
    public private(set) var selected: Set<OutPoint> = []
    /// Denominated CoinJoin coins are hidden on the regular page by default.
    public private(set) var showCoinJoinCoins = false
    public private(set) var summary: CoinSelectionSummary?
    public private(set) var customChange: String?
    public private(set) var customChangeWarning: CustomChangeWarning?
    /// "Some coins were unselected because they were spent." (QT-074)
    public private(set) var unselectedNotice = false
    public private(set) var errorMessage: String?
    /// `coin_selection_summary` is not implemented by the engine yet.
    public private(set) var summaryUnavailable = false
    /// The CoinJoin send page: list mode only, all change goes to the fee.
    public let coinJoinPage: Bool
    /// The Send page spending this selection; set by `SendViewModel.attach`.
    @ObservationIgnored weak var send: SendViewModel?

    /// Coins the dialog lists, in the chosen order.
    public var coins: [Utxo] {
        let visible = allCoins.filter { showCoinJoinCoins || coinJoinPage || !$0.coinJoinDenominated }
        return visible.sorted(by: Self.order(sort))
    }

    /// Tree mode: coins grouped by address, groups in the same order.
    public var groups: [CoinAddressGroup] {
        var order: [String] = []
        var byAddress: [String: [Utxo]] = [:]
        for coin in coins {
            if byAddress[coin.address] == nil { order.append(coin.address) }
            byAddress[coin.address, default: []].append(coin)
        }
        return order.map { address in
            let coins = byAddress[address] ?? []
            return CoinAddressGroup(
                address: address, label: label(of: coins[0]), coins: coins,
                total: Amount(duffs: coins.reduce(0) { $0 + $1.amount.duffs }))
        }
    }

    public var lockedCount: Int { allCoins.filter(\.userLocked).count }
    public var lockedText: String { L10n.CoinControl.lockedCount(lockedCount) }
    public var coinJoinToggleTitle: String {
        showCoinJoinCoins ? L10n.CoinControl.hideCoinJoinCoins : L10n.CoinControl.showAllCoins
    }
    /// "automatically selected" while nothing is picked.
    public var isAutomatic: Bool { selected.isEmpty }
    public var insufficientFunds: Bool { summary?.insufficientFunds ?? false }

    public var summaryText: CoinSummaryText? {
        guard let summary else { return nil }
        let unit = settings.display.unit
        func money(_ amount: Amount) -> String {
            amounts.format(amount, unit: unit, style: .withUnit(plusSign: false, separators: .standard))
        }
        let estimate = summary.fee.duffs > 0 ? Self.approximately : ""
        let changeEstimate = summary.fee.duffs > 0 && summary.change.duffs > 0 ? Self.approximately : ""
        return CoinSummaryText(
            quantity: String(summary.quantity), amount: money(summary.amount),
            bytes: (summary.bytes > 0 ? Self.approximately : "") + String(summary.bytes),
            fee: estimate + money(summary.fee), afterFee: estimate + money(summary.afterFee),
            change: changeEstimate + money(summary.change),
            tolerance: L10n.CoinControl.tolerance(summary.feeTolerancePerInput.duffs))
    }

    private var payAmounts: [Amount] = []
    private var fee: FeeChoice = .recommended(targetBlocks: 6)
    private let walletState: any WalletStateProviding
    private let coinControl: any CoinControlProviding
    private let fees: any FeeAndCoinSelectionProviding
    private let receive: any ReceiveProviding
    private let uri: any URIHandling
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let desktopPreferences: any DesktopPreferencesStoring

    public init(
        walletState: any WalletStateProviding, coinControl: any CoinControlProviding,
        fees: any FeeAndCoinSelectionProviding, receive: any ReceiveProviding, uri: any URIHandling,
        amounts: any AmountFormatting, settings: any SettingsProviding,
        desktopPreferences: any DesktopPreferencesStoring, coinJoinPage: Bool = false
    ) {
        self.walletState = walletState
        self.coinControl = coinControl
        self.fees = fees
        self.receive = receive
        self.uri = uri
        self.amounts = amounts
        self.settings = settings
        self.desktopPreferences = desktopPreferences
        self.coinJoinPage = coinJoinPage
        let stored = desktopPreferences.desktop
        mode = coinJoinPage ? .list : stored.coinControlMode
        sort = stored.coinSort
        if stored.options.keepCustomChangeAddress, !coinJoinPage {
            customChange = stored.options.customChangeAddress
        }
    }

    public convenience init(env: AppEnvironment, m2: M2Services, coinJoinPage: Bool = false) {
        self.init(
            walletState: env.walletState, coinControl: env.coinControl, fees: m2.fees, receive: env.receive,
            uri: env.uri, amounts: env.amounts, settings: env.settings, desktopPreferences: m2.desktopPreferences,
            coinJoinPage: coinJoinPage)
    }

    // MARK: Loading

    /// Lists the wallet's coins; selections that are gone are dropped with
    /// the QT-074 notice.
    public func load() async {
        guard let wallet = walletState.selectedWalletID else {
            allCoins = []
            selected = []
            return
        }
        do {
            allCoins = try await coinControl.utxos(wallet: wallet, filter: UtxoFilter(includeLocked: true))
            errorMessage = nil
        } catch {
            errorMessage = ErrorText.m2(error.code)
            return
        }
        let present = Set(allCoins.filter(isSelectable).map(\.outpoint))
        let gone = selected.subtracting(present)
        if !gone.isEmpty {
            selected.subtract(gone)
            unselectedNotice = true
        }
        if let customChange { await validateCustomChange(customChange, keepConfirmation: true) }
        await refreshSummary()
    }

    /// The Send page's recipients and fee, for the panel values.
    public func updatePayment(amounts: [Amount], fee: FeeChoice) async {
        payAmounts = amounts
        self.fee = fee
        await refreshSummary()
    }

    // MARK: Selection

    /// Space / checkbox. Locked and reserved coins cannot be picked.
    public func toggle(_ outpoint: OutPoint) async {
        guard let coin = allCoins.first(where: { $0.outpoint == outpoint }), isSelectable(coin) else { return }
        guard editSelection({ selected in
            if selected.contains(outpoint) { selected.remove(outpoint) } else { selected.insert(outpoint) }
        }) else { return }
        await refreshSummary()
    }

    /// "(un)select all": clears a full selection, else selects every listed coin.
    public func selectAll() async {
        let selectable = Set(coins.filter(isSelectable).map(\.outpoint))
        guard editSelection({ selected in
            if !selectable.isEmpty, selectable.isSubset(of: selected) {
                selected.subtract(selectable)
            } else {
                selected.formUnion(selectable)
            }
        }) else { return }
        await refreshSummary()
    }

    /// Empties the selection (dash-qt `UnSelectAll`): Send's Clear All and a
    /// sent payment. Not a user edit of a review.
    public func clearSelection() {
        selected = []
        summary = nil
        unselectedNotice = false
    }

    /// "(un)lock all": flips the lock of every listed coin; newly locked ones
    /// are unselected (dash-qt).
    public func lockAll() async {
        guard let wallet = walletState.selectedWalletID else { return }
        let listed = coins
        let toUnlock = listed.filter(\.userLocked).map(\.outpoint)
        let toLock = listed.filter { !$0.userLocked }.map(\.outpoint)
        do {
            if !toUnlock.isEmpty { try await coinControl.unlock(wallet: wallet, outpoints: toUnlock) }
            if !toLock.isEmpty { try await coinControl.lock(wallet: wallet, outpoints: toLock) }
            noteSelectionEdit { $0.subtract(toLock) }
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
        await load()
    }

    public func lock(_ outpoint: OutPoint) async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            try await coinControl.lock(wallet: wallet, outpoints: [outpoint])
            noteSelectionEdit { $0.remove(outpoint) }
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
        await load()
    }

    public func unlock(_ outpoint: OutPoint) async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            try await coinControl.unlock(wallet: wallet, outpoints: [outpoint])
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
        await load()
    }

    public func dismissUnselectedNotice() {
        unselectedNotice = false
    }

    // MARK: Display settings

    public func setMode(_ mode: CoinControlMode) {
        guard !coinJoinPage else { return }
        self.mode = mode
        persist { $0.coinControlMode = mode }
    }

    /// Header click: the same column flips the order; another starts descending
    /// for amounts and ascending otherwise.
    public func sort(by column: CoinColumn) {
        if sort.column == column {
            sort.ascending.toggle()
        } else {
            sort = CoinSort(column: column, ascending: column != .amount)
        }
        let sort = sort
        persist { $0.coinSort = sort }
    }

    /// Hiding CoinJoin coins unselects them, so it is refused like any
    /// selection change while the payment cannot change.
    public func setShowCoinJoinCoins(_ show: Bool) async {
        let hidden = Set(allCoins.filter(\.coinJoinDenominated).map(\.outpoint))
        if !show, !selected.isDisjoint(with: hidden) {
            guard editSelection({ $0.subtract(hidden) }) else { return }
            showCoinJoinCoins = show
            await refreshSummary()
        } else {
            showCoinJoinCoins = show
        }
    }

    // MARK: Copy

    /// Context-menu text for a coin: amount without unit or separators,
    /// outpoint as `txid:vout`.
    public func copy(_ field: CoinCopyField, of outpoint: OutPoint) -> String {
        guard let coin = allCoins.first(where: { $0.outpoint == outpoint }) else { return "" }
        switch field {
        case .address: return coin.address
        case .label: return coin.label ?? ""
        case .amount:
            return amounts.format(coin.amount, unit: settings.display.unit, style: .plain(plusSign: false, separators: .never))
        case .outpoint: return "\(coin.outpoint.txid):\(coin.outpoint.vout)"
        }
    }

    /// A panel value as dash-qt copies it: the number, without `≈` or unit.
    public func copy(_ field: CoinSummaryField) -> String {
        guard let summary else { return "" }
        let unit = settings.display.unit
        func plain(_ amount: Amount) -> String {
            amounts.format(amount, unit: unit, style: .plain(plusSign: false, separators: .never))
        }
        switch field {
        case .quantity: return String(summary.quantity)
        case .amount: return plain(summary.amount)
        case .fee: return plain(summary.fee)
        case .afterFee: return plain(summary.afterFee)
        case .bytes: return String(summary.bytes)
        case .change: return plain(summary.change)
        }
    }

    // MARK: Presentation

    public func amountText(_ coin: Utxo) -> String {
        amounts.format(coin.amount, unit: settings.display.unit, style: .plain(plusSign: false, separators: .standard))
    }

    /// The label column: "(change)" for change, else the label or "(no label)".
    public func label(of coin: Utxo) -> String {
        if coin.isChange { return L10n.CoinControl.changeLabel }
        guard let label = coin.label, !label.isEmpty else { return L10n.CoinControl.noLabel }
        return label
    }

    public func isSelectable(_ coin: Utxo) -> Bool { !coin.userLocked && !coin.reserved }

    // MARK: Custom change (QT-073)

    /// Checks the typed address: invalid ones are refused; one this wallet
    /// does not own needs `confirmCustomChange()`.
    public func setCustomChange(_ text: String?) async {
        let trimmed = text?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let trimmed, !trimmed.isEmpty else {
            rejectCustomChange()
            return
        }
        customChange = trimmed
        await validateCustomChange(trimmed, keepConfirmation: false)
    }

    /// "Yes" to "Confirm custom change address".
    public func confirmCustomChange() {
        guard case .unknownAddress(confirmed: false) = customChangeWarning else { return }
        customChangeWarning = .unknownAddress(confirmed: true)
        rememberCustomChange(customChange)
    }

    /// "Cancel": the address is cleared.
    public func rejectCustomChange() {
        customChange = nil
        customChangeWarning = nil
        rememberCustomChange(nil)
    }

    /// Whether the selection is used: Options ▸ Wallet ▸ "Enable coin
    /// control features" (dash-qt ignores the selection while it is off).
    public var isEnabled: Bool { desktopPreferences.desktop.options.coinControl }

    /// The option was applied (dash-qt `coinControlFeatureChanged`): turning
    /// it off empties the selection, so it cannot come back into effect when
    /// the option is turned on again, and ends a review that spent it, as a
    /// selection edit does. A payment already broadcasting keeps its built
    /// transaction; only the selection is emptied.
    public func coinControlFeatureChanged() {
        guard !isEnabled else { return }
        let hadSelection = !selected.isEmpty
        clearSelection()
        if hadSelection { send?.coinSelectionEdited() }
    }

    /// What the draft spends: the picked coins, or any coins (fully mixed
    /// ones on the CoinJoin page) while nothing is picked or coin control
    /// features are off.
    public func source() -> CoinSourceChoice {
        let automatic: CoinSourceChoice = coinJoinPage ? .fullyMixed : .any
        guard isEnabled, !selected.isEmpty else { return automatic }
        return .outpoints(sortedSelection)
    }

    /// Where change goes: a valid (and, if foreign, confirmed) custom
    /// address, else automatic.
    public func change() -> ChangeChoice {
        guard !coinJoinPage, let customChange else { return .automatic }
        switch customChangeWarning {
        case nil, .unknownAddress(confirmed: true): return .address(customChange)
        case .invalidAddress, .unknownAddress(confirmed: false): return .automatic
        }
    }

    // MARK: Private

    /// Applies a user change of the selection. While the Send page cannot
    /// change (broadcasting, outcome unknown) nothing changes and
    /// `errorMessage` says why; returns whether the change was applied.
    private func editSelection(_ change: (inout Set<OutPoint>) -> Void) -> Bool {
        if let send, !send.isEditable {
            var proposed = selected
            change(&proposed)
            if proposed != selected { errorMessage = L10n.CoinControl.selectionLockedWhileSending }
            return false
        }
        if errorMessage == L10n.CoinControl.selectionLockedWhileSending { errorMessage = nil }
        noteSelectionEdit(change)
        return true
    }

    /// A selection change the user caused (a pick, or a lock that unselects):
    /// the Send page treats it as an edit.
    private func noteSelectionEdit(_ change: (inout Set<OutPoint>) -> Void) {
        let before = selected
        change(&selected)
        if selected != before { send?.coinSelectionEdited() }
    }

    private func validateCustomChange(_ address: String, keepConfirmation: Bool) async {
        guard case .core = uri.classifyAddress(address) else {
            customChangeWarning = .invalidAddress
            return
        }
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            let own = try await receive.addresses(wallet: wallet, filter: AddressFilter())
            guard customChange == address else { return }
            if own.contains(where: { $0.address == address }) {
                customChangeWarning = nil
                rememberCustomChange(address)
            } else {
                let confirmed: Bool
                if keepConfirmation, case .unknownAddress(true) = customChangeWarning {
                    confirmed = true
                } else if keepConfirmation, customChangeWarning == nil {
                    // A remembered address was confirmed before it was stored.
                    confirmed = desktopPreferences.desktop.options.customChangeAddress == address
                } else {
                    confirmed = false
                }
                customChangeWarning = .unknownAddress(confirmed: confirmed)
            }
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// The selected outpoints in a stable order (txid, then vout).
    private var sortedSelection: [OutPoint] {
        selected.sorted { ($0.txid, $0.vout) < ($1.txid, $1.vout) }
    }

    private func rememberCustomChange(_ address: String?) {
        guard desktopPreferences.desktop.options.keepCustomChangeAddress, !coinJoinPage else { return }
        persist { $0.options.customChangeAddress = address }
    }

    private func refreshSummary() async {
        guard let wallet = walletState.selectedWalletID, !selected.isEmpty else {
            summary = nil
            return
        }
        for _ in 0..<2 {
            do {
                let result = try await fees.summary(
                    wallet: wallet, outpoints: sortedSelection, payAmounts: payAmounts, fee: fee,
                    allChangeToFee: coinJoinPage)
                summaryUnavailable = false
                guard !result.unavailable.isEmpty else {
                    summary = result
                    return
                }
                selected.subtract(result.unavailable)
                unselectedNotice = true
                if selected.isEmpty {
                    summary = nil
                    return
                }
            } catch {
                summary = nil
                if error.code == .notImplemented {
                    summaryUnavailable = true
                } else {
                    errorMessage = ErrorText.m2(error.code)
                }
                return
            }
        }
        // Coins kept vanishing between two answers: show no stale values.
        summary = nil
    }

    private func persist(_ change: (inout DesktopPreferences) -> Void) {
        var stored = desktopPreferences.desktop
        change(&stored)
        do {
            try desktopPreferences.update(stored)
        } catch {
            errorMessage = L10n.Settings.settingsNotSaved
        }
    }

    private static func order(_ sort: CoinSort) -> (Utxo, Utxo) -> Bool {
        { lhs, rhs in
            let ascending: Bool
            switch sort.column {
            case .amount:
                if lhs.amount == rhs.amount { return tie(lhs, rhs) }
                ascending = lhs.amount < rhs.amount
            case .label:
                let l = lhs.label ?? "", r = rhs.label ?? ""
                if l == r { return tie(lhs, rhs) }
                ascending = l.localizedCaseInsensitiveCompare(r) == .orderedAscending
            case .address:
                if lhs.address == rhs.address { return tie(lhs, rhs) }
                ascending = lhs.address < rhs.address
            case .mixingRounds:
                let l = lhs.coinJoinRounds.map(Int.init) ?? -1, r = rhs.coinJoinRounds.map(Int.init) ?? -1
                if l == r { return tie(lhs, rhs) }
                ascending = l < r
            case .date:
                let l = lhs.date ?? .distantFuture, r = rhs.date ?? .distantFuture
                if l == r { return tie(lhs, rhs) }
                ascending = l < r
            case .confirmations:
                if lhs.confirmations == rhs.confirmations { return tie(lhs, rhs) }
                ascending = lhs.confirmations < rhs.confirmations
            }
            return sort.ascending ? ascending : !ascending
        }
    }

    /// Stable order for equal keys.
    private static func tie(_ lhs: Utxo, _ rhs: Utxo) -> Bool {
        (lhs.outpoint.txid, lhs.outpoint.vout) < (rhs.outpoint.txid, rhs.outpoint.vout)
    }
}
