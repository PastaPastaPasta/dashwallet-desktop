// The Masternodes tab (QT-118…121, IOS-080): the persisted filters (type,
// text, owned, hide banned), dash-qt's columns with sorting, owned
// detection, the context menu and the toolbar buttons. Columns an SPV wallet
// cannot fill show "—" with "Requires full-node data source". The list is
// browsable without a wallet.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

public enum MasternodeColumn: Sendable, Hashable, CaseIterable {
    case status, service, type, poseScore, registered, lastPaid, nextPayment, operatorReward, proTxHash

    public var title: String {
        typealias M = L10n.Masternodes
        switch self {
        case .status: return M.columnStatus
        case .service: return M.columnService
        case .type: return M.columnType
        case .poseScore: return M.columnPoSe
        case .registered: return M.columnRegistered
        case .lastPaid: return M.columnLastPaid
        case .nextPayment: return M.columnNextPayment
        case .operatorReward: return M.columnOperatorReward
        case .proTxHash: return M.columnProTxHash
        }
    }
}

public struct MasternodeSort: Sendable, Hashable {
    public var column: MasternodeColumn
    public var ascending: Bool

    public init(column: MasternodeColumn = .service, ascending: Bool = true) {
        self.column = column
        self.ascending = ascending
    }
}

/// A context-menu entry (QT-121).
public enum MasternodeAction: Sendable, Hashable {
    case copyProTxHash, copyCollateralOutpoint
    case updateService, updateRegistrar, revoke
    case changeRewardAddress, rotateKeys, dissolve, createStandbyDissolution
    case filterByCollateral, filterByPayout, filterByOwner, filterByVoting
    case showDetails, track
}

public struct MasternodeMenuItem: Sendable, Hashable, Identifiable {
    public var id: MasternodeAction { action }
    public let action: MasternodeAction
    public let title: String
    public let isEnabled: Bool
    public let tooltip: String?
    /// Filter entries sit in the "Filter by" submenu.
    public let inFilterMenu: Bool
}

/// A dialog the list asks the UI to open.
public enum MasternodePresentation: Sendable, Hashable {
    case details(proTxHash: String)
    case updateService(proTxHash: String)
    case updateRegistrar(proTxHash: String)
    case revoke(proTxHash: String)
    case changeRewardAddress(proTxHash: String)
    case rotateKeys(proTxHash: String)
    case dissolve(proTxHash: String)
    case createStandbyDissolution(proTxHash: String)
    case register
    case shared
}

public enum MasternodeListPhase: Sendable, Hashable {
    case loading
    case unavailable
    case ready
    case failed(String)
}

@MainActor
@Observable
public final class MasternodeListViewModel {
    public private(set) var phase: MasternodeListPhase = .loading
    public private(set) var state: MasternodeListState?
    public private(set) var rows: [MasternodeRow] = []
    public private(set) var query: MasternodeQuery
    public private(set) var sort = MasternodeSort()
    public private(set) var errorMessage: String?
    /// A copy/filter confirmation for the status line.
    public private(set) var message: String?
    /// A dialog to open; the UI clears it with `presentationHandled()`.
    public private(set) var presentation: MasternodePresentation?
    public var selection: String?

    public let defaults: MasternodeNetworkDefaults

    /// dash-qt hides Type for the Regular and Evo filters; ProTx Hash is a
    /// hidden column.
    public var columns: [MasternodeColumn] {
        MasternodeColumn.allCases.filter { column in
            switch column {
            case .type: query.typeFilter != .regular && query.typeFilter != .evo
            case .proTxHash: false
            default: true
            }
        }
    }

    /// "Node Count:" — the filtered count.
    public var nodeCountText: String { "\(rows.count)" }

    public var syncNotice: String? {
        guard let state else { return nil }
        if !state.available { return L10n.Masternodes.listUnavailable }
        return state.syncing ? L10n.Masternodes.listSyncing : nil
    }

    private var selectedWallet: WalletInfo? {
        walletState.wallets?.first { $0.id == walletState.selectedWalletID }
    }

    /// "Register Masternode…": a wallet with private keys.
    public var canRegister: Bool { selectedWallet.map { !$0.watchOnly } ?? false }

    public var registerTooltip: String {
        guard let wallet = selectedWallet else { return L10n.Masternodes.registerNeedsWallet }
        return wallet.watchOnly ? L10n.Masternodes.registerNeedsKeys : L10n.Masternodes.registerTip
    }

    public var canManageShared: Bool { canRegister }

    public var sharedTooltip: String {
        guard let wallet = selectedWallet else { return L10n.Masternodes.sharedNeedsWallet }
        return wallet.watchOnly ? L10n.Masternodes.sharedNeedsKeys : L10n.Masternodes.sharedTip
    }

    private let masternodes: any MasternodeListProviding
    private let tracked: any TrackedMasternodeManaging
    private let walletState: any WalletStateProviding
    private let clipboard: any ClipboardProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private let timing: Timing
    private var tasks: [Task<Void, Never>] = []
    private var loaded: [MasternodeRow] = []

    public init(
        masternodes: any MasternodeListProviding, tracked: any TrackedMasternodeManaging,
        walletState: any WalletStateProviding, clipboard: any ClipboardProviding,
        desktopPreferences: any DesktopPreferencesStoring, timing: Timing
    ) {
        self.masternodes = masternodes
        self.tracked = tracked
        self.walletState = walletState
        self.clipboard = clipboard
        self.desktopPreferences = desktopPreferences
        self.timing = timing
        defaults = masternodes.defaults()
        let stored = desktopPreferences.desktop.m3
        query = MasternodeQuery(
            typeFilter: MasternodeTypeFilter(storedName: stored.masternodeTypeFilter),
            text: stored.masternodeFilterText.isEmpty ? nil : stored.masternodeFilterText,
            ownedOnly: stored.masternodeOwnedOnly, hideBanned: stored.masternodeHideBanned)
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            masternodes: m3.masternodes, tracked: m3.tracked, walletState: env.walletState, clipboard: m2.clipboard,
            desktopPreferences: m2.desktopPreferences, timing: env.timing)
    }

    // MARK: Observation

    /// Loads and re-queries on `Masternodes` events (the engine coalesces
    /// them to every 3 s, 30 s while syncing) and wallet changes.
    public func start() async {
        stop()
        await reload()
        let changes = masternodes.changes()
        let walletChanges = walletState.changes()
        tasks.append(Task { [weak self] in
            for await _ in changes { await self?.reload() }
        })
        tasks.append(Task { [weak self] in
            for await _ in walletChanges { await self?.reload() }
        })
    }

    public func stop() {
        tasks.forEach { $0.cancel() }
        tasks = []
    }

    public func reload() async {
        do {
            state = try await masternodes.state()
            loaded = try await masternodes.list(query)
            phase = .ready
        } catch {
            if error.isNotImplemented {
                phase = .unavailable
            } else {
                phase = .failed(ErrorText.m3(error, amount: { "\($0.duffs)" }))
            }
            return
        }
        applySort()
        if let selection, !rows.contains(where: { $0.id == selection }) { self.selection = nil }
    }

    // MARK: Filters (persisted)

    public func setTypeFilter(_ filter: MasternodeTypeFilter) async {
        query.typeFilter = filter
        desktopPreferences.updateM3 { $0.masternodeTypeFilter = filter.storedName }
        await reload()
    }

    public func setFilterText(_ text: String) async {
        query.text = text.isEmpty ? nil : text
        desktopPreferences.updateM3 { $0.masternodeFilterText = text }
        await reload()
    }

    public func setOwnedOnly(_ on: Bool) async {
        query.ownedOnly = on
        desktopPreferences.updateM3 { $0.masternodeOwnedOnly = on }
        await reload()
    }

    public func setHideBanned(_ on: Bool) async {
        query.hideBanned = on
        desktopPreferences.updateM3 { $0.masternodeHideBanned = on }
        await reload()
    }

    /// A header click: the same column flips the order.
    public func sort(by column: MasternodeColumn) {
        if sort.column == column {
            sort.ascending.toggle()
        } else {
            sort = MasternodeSort(column: column, ascending: true)
        }
        applySort()
    }

    private func applySort() {
        let ascending = sort.ascending
        let key = sortKey(sort.column)
        rows = loaded.sorted { lhs, rhs in
            let l = key(lhs), r = key(rhs)
            if l == r { return lhs.proTxHash < rhs.proTxHash }
            // Unknown values sort last either way.
            switch (l, r) {
            case (.none, _): return false
            case (_, .none): return true
            case (.some(let a), .some(let b)): return ascending ? a < b : a > b
            }
        }
    }

    private enum SortValue: Comparable {
        case number(Int64)
        case text(String)
    }

    private func sortKey(_ column: MasternodeColumn) -> (MasternodeRow) -> SortValue? {
        switch column {
        case .status:
            return { row in
                switch row.status {
                case .active: .number(0)
                case .banned: .number(1)
                case .retired: .number(2)
                case .unknown: nil
                }
            }
        case .service: return { $0.service.map { .text($0) } }
        case .type: return { .text(self.typeText($0)) }
        case .poseScore: return { $0.poseScore.map { .number(Int64($0)) } }
        case .registered: return { $0.registeredHeight.map { .number(Int64($0)) } }
        case .lastPaid: return { $0.lastPaidHeight.map { .number(Int64($0)) } }
        case .nextPayment: return { $0.nextPaymentHeight.map { .number(Int64($0)) } }
        case .operatorReward: return { $0.operatorReward.map { .number(Int64($0.percentX100)) } }
        case .proTxHash: return { .text($0.proTxHash) }
        }
    }

    // MARK: Cells (QT-119)

    public func statusText(_ row: MasternodeRow) -> String {
        switch row.status {
        case .active: L10n.Masternodes.active
        case .banned: L10n.Masternodes.banned
        case .retired: L10n.Masternodes.retired
        case .unknown: L10n.Masternodes.statusUnknown
        }
    }

    /// "Active for X" / "Banned for X"; "—" reasons when the height is unknown.
    public func statusTooltip(_ row: MasternodeRow) -> String? {
        func since(_ height: UInt32?) -> String? {
            guard let height, let tip = state?.height, tip >= height else { return nil }
            return L10n.Durations.blocks(tip - height, spacing: .seconds(150))
        }
        switch row.status {
        case .active(let height): return since(height).map(L10n.Masternodes.activeFor)
        case .banned(let height): return since(height).map(L10n.Masternodes.bannedFor)
        case .retired, .unknown: return nil
        }
    }

    public func typeText(_ row: MasternodeRow) -> String {
        if let shared = row.shared { return L10n.Masternodes.sharedHolding(shared.heldShares, of: shared.totalShares) }
        return row.type == .evo ? L10n.Masternodes.evo : L10n.Masternodes.regular
    }

    /// The text of a cell; `nil` values show "—".
    public func text(_ column: MasternodeColumn, _ row: MasternodeRow) -> String {
        let dash = L10n.Masternodes.dash
        switch column {
        case .status: return statusText(row)
        case .service: return row.service ?? dash
        case .type: return typeText(row)
        case .poseScore: return row.poseScore.map { "\($0)" } ?? dash
        case .registered: return row.registeredHeight.map { "\($0)" } ?? dash
        case .lastPaid: return row.lastPaidHeight.map { "\($0)" } ?? dash
        case .nextPayment: return row.nextPaymentHeight.map { "\($0)" } ?? dash
        case .operatorReward: return operatorRewardText(row)
        case .proTxHash: return row.proTxHash
        }
    }

    /// "Requires full-node data source" for the "—" an SPV wallet shows.
    public func tooltip(_ column: MasternodeColumn, _ row: MasternodeRow) -> String? {
        let unknown: Bool
        switch column {
        case .status: return statusTooltip(row)
        case .poseScore: unknown = row.poseScore == nil
        case .registered: unknown = row.registeredHeight == nil
        case .lastPaid: unknown = row.lastPaidHeight == nil
        case .nextPayment: unknown = row.nextPaymentHeight == nil
        case .operatorReward: unknown = row.operatorReward == nil
        case .service, .type, .proTxHash: return nil
        }
        return unknown ? L10n.Masternodes.fullNodeOnly : nil
    }

    /// "NONE", "x.xx% to <addr>", or "x.xx% but not claimed".
    public func operatorRewardText(_ row: MasternodeRow) -> String {
        guard let reward = row.operatorReward else { return L10n.Masternodes.dash }
        guard reward.percentX100 > 0 else { return L10n.Masternodes.operatorRewardNone }
        return L10n.Masternodes.operatorReward(M3Validation.percent(reward.percentX100), to: reward.payoutAddress)
    }

    /// Why the wallets count the masternode as theirs (QT-120).
    public func ownedText(_ row: MasternodeRow) -> String? {
        guard !row.ownedRoles.isEmpty else { return nil }
        return OwnedRole.allCases.filter(row.ownedRoles.contains).map(L10n.Masternodes.ownedRole)
            .joined(separator: ", ")
    }

    // MARK: Context menu (QT-121)

    public func menu(for row: MasternodeRow) -> [MasternodeMenuItem] {
        typealias M = L10n.Masternodes
        let signing = canRegister
        let owner = row.ownedRoles.contains(.owner)
        let shareOwner = row.ownedRoles.contains(.shareOwner)
        let isShared = row.shared != nil
        func item(_ action: MasternodeAction, _ title: String, _ enabled: Bool = true, _ tip: String? = nil,
                  filter: Bool = false) -> MasternodeMenuItem {
            MasternodeMenuItem(
                action: action, title: title, isEnabled: enabled, tooltip: enabled ? nil : tip, inFilterMenu: filter)
        }
        var items = [
            item(.copyProTxHash, M.copyProTxHash),
            item(.copyCollateralOutpoint, M.copyCollateralOutpoint, row.collateral != nil, M.fullNodeOnly),
            item(.updateService, M.updateService, signing, M.needsSigningWallet),
        ]
        if !isShared {
            items.append(item(.updateRegistrar, M.updateRegistrar, signing && owner, M.needsOwnerKey))
        } else {
            let saved = desktopPreferences.desktop.m3.standbyDissolutions[row.proTxHash]
            items += [
                item(.changeRewardAddress, M.changeRewardAddress, shareOwner, M.needsShareOwnerKey),
                item(.rotateKeys, M.rotateKeys, shareOwner, M.needsShareOwnerKey),
                item(.dissolve, M.dissolve, shareOwner, M.needsShareOwnerKey),
                MasternodeMenuItem(
                    action: .createStandbyDissolution, title: M.createStandby, isEnabled: shareOwner,
                    tooltip: shareOwner
                        ? saved.map { M.standbySaved(M3Dates.dateTime($0, timing: timing)) } ?? M.standbyNotSaved
                        : M.needsShareOwnerKey,
                    inFilterMenu: false),
            ]
        }
        items.append(item(.revoke, M.revoke, signing, M.needsSigningWallet))
        items += [
            item(.filterByCollateral, M.collateralAddress, row.collateralAddress != nil, M.fullNodeOnly, filter: true),
            item(.filterByPayout, M.payoutAddress, !row.payoutAddresses.isEmpty, M.fullNodeOnly, filter: true),
            MasternodeMenuItem(
                action: .filterByOwner, title: M.ownerAddress, isEnabled: row.ownerAddress != nil,
                tooltip: isShared ? M.filterOwnerShared : (row.ownerAddress == nil ? M.fullNodeOnly : nil),
                inFilterMenu: true),
            item(.filterByVoting, M.votingAddress, filter: true),
            item(.showDetails, M.showDetails),
        ]
        if !row.ownedRoles.contains(.tracked) { items.append(item(.track, M.track)) }
        return items
    }

    public func perform(_ action: MasternodeAction, on row: MasternodeRow) async {
        errorMessage = nil
        message = nil
        guard let item = menu(for: row).first(where: { $0.action == action }), item.isEnabled else { return }
        let hash = row.proTxHash
        switch action {
        case .copyProTxHash:
            clipboard.setString(hash)
        case .copyCollateralOutpoint:
            if let collateral = row.collateral { clipboard.setString("\(collateral.txid)-\(collateral.vout)") }
        case .filterByCollateral:
            if let address = row.collateralAddress { await setFilterText(address) }
        case .filterByPayout:
            if let address = row.payoutAddresses.first { await setFilterText(address) }
        case .filterByOwner:
            if let address = row.ownerAddress { await setFilterText(address) }
        case .filterByVoting:
            await setFilterText(row.votingAddress)
        case .updateService: presentation = .updateService(proTxHash: hash)
        case .updateRegistrar: presentation = .updateRegistrar(proTxHash: hash)
        case .revoke: presentation = .revoke(proTxHash: hash)
        case .changeRewardAddress: presentation = .changeRewardAddress(proTxHash: hash)
        case .rotateKeys: presentation = .rotateKeys(proTxHash: hash)
        case .dissolve: presentation = .dissolve(proTxHash: hash)
        case .createStandbyDissolution: presentation = .createStandbyDissolution(proTxHash: hash)
        case .showDetails: presentation = .details(proTxHash: hash)
        case .track:
            do {
                _ = try await tracked.track(proTxHash: hash, label: nil)
                await reload()
            } catch {
                errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" })
            }
        }
    }

    public func openRegister() {
        guard canRegister else { return }
        presentation = .register
    }

    public func openShared() {
        guard canManageShared else { return }
        presentation = .shared
    }

    public func presentationHandled() {
        presentation = nil
    }
}
