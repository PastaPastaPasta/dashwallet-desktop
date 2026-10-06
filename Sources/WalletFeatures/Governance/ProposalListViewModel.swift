// The Governance tab (QT-128…130): Active / My Proposals, the title filter,
// dash-qt's columns and tooltips, the voting deadline and the info button's
// tooltip, the context menu (Copy Raw JSON, Open URL with the External Link
// Warning, Vote Yes/No/Abstain) and the details view. Governance data comes
// from peers (govsync) and may lag; the tab says so.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

public enum ProposalListSource: String, Sendable, Hashable, CaseIterable {
    case active, mine

    public var title: String {
        switch self {
        case .active: L10n.Governance.activeProposals
        case .mine: L10n.Governance.myProposals
        }
    }
}

public enum ProposalColumn: Sendable, Hashable, CaseIterable {
    case status, title, amount, start, end, votes, myVotes, hash

    public var title: String {
        switch self {
        case .status: L10n.Governance.columnStatus
        case .title: L10n.Governance.columnTitle
        case .amount: L10n.Governance.columnAmount
        case .start: L10n.Governance.columnStart
        case .end: L10n.Governance.columnEnd
        case .votes: L10n.Governance.columnVotes
        case .myVotes: L10n.Governance.columnMyVotes
        case .hash: L10n.Governance.columnHash
        }
    }
}

public enum GovernanceListPhase: Sendable, Hashable {
    case loading
    case unavailable
    case ready
    case failed(String)
}

/// The "External Link Warning" box (defaults to No).
public struct ExternalLinkConfirmation: Sendable, Hashable {
    public let url: URL
    public var title: String { L10n.Governance.externalLinkTitle }
    public var message: String { L10n.Governance.externalLinkMessage(url.absoluteString) }
}

/// One line of the details view.
public struct DetailLine: Sendable, Hashable, Identifiable {
    public var id: String { title }
    public let title: String
    public let value: String
    /// Shown as "—" with this tooltip (full-node-only data).
    public let tooltip: String?

    public init(_ title: String, _ value: String, tooltip: String? = nil) {
        self.title = title
        self.value = value
        self.tooltip = tooltip
    }
}

@MainActor
@Observable
public final class ProposalListViewModel {
    public private(set) var phase: GovernanceListPhase = .loading
    public private(set) var rows: [ProposalRow] = []
    public private(set) var syncState: GovernanceSyncState?
    public private(set) var clock: GovernanceClock?
    public private(set) var info: GovernanceInfo?
    public private(set) var source: ProposalListSource = .active
    public private(set) var titleFilter = ""
    public private(set) var errorMessage: String?
    /// A confirmation the UI copies to the clipboard or shows.
    public private(set) var message: String?
    public private(set) var externalLink: ExternalLinkConfirmation?
    /// A URL the user confirmed; the UI opens it and calls `urlOpened()`.
    public private(set) var urlToOpen: URL?
    public private(set) var detail: ProposalDetail?
    /// The open vote dialog (Votes… or a Vote context action).
    public private(set) var vote: ProposalVoteViewModel?
    public var selection: String?

    public let parameters: GovernanceParameters

    public var columns: [ProposalColumn] {
        ProposalColumn.allCases.filter { column in
            switch column {
            case .votes: source == .active
            case .myVotes: walletState.selectedWalletID != nil
            default: true
            }
        }
    }

    public var countText: String { "\(rows.count)" }

    public var emptyText: String? {
        guard phase == .ready, rows.isEmpty else { return nil }
        return source == .active ? L10n.Governance.noActiveProposals : L10n.Governance.noLocalProposals
    }

    public var isSynced: Bool { syncState?.phase == .synced }

    /// The sync line ("from peers, may lag").
    public var syncText: String? {
        guard let syncState else { return nil }
        return syncState.phase == .synced ? L10n.Governance.fromPeers : L10n.Governance.syncPhase(syncState.phase)
    }

    /// "Voting deadline: ~%1 left (%2 blocks, block %3)" / passed / waiting.
    public var deadlineText: String {
        guard isSynced, let clock else { return L10n.Governance.deadlineWaiting }
        let height = clock.nextSuperblock - min(clock.blocksToSuperblock, clock.nextSuperblock)
        guard height < clock.votingCutoff else { return L10n.Governance.deadlinePassed(clock.votingCutoff) }
        let blocks = clock.votingCutoff - height
        return L10n.Governance.deadline(
            left: L10n.Durations.blocks(blocks, spacing: parameters.targetSpacing), blocks: blocks,
            block: clock.votingCutoff)
    }

    public var deadlineTooltip: String { L10n.Governance.deadlineTooltip }

    /// The ⓘ button's tooltip lines.
    public var infoTooltipLines: [String] {
        guard let syncState else { return [] }
        guard syncState.phase != .waiting else { return [L10n.Governance.waitingBlockchain] }
        guard syncState.phase == .synced, let clock else { return [L10n.Governance.waitingGovernance] }
        var lines = [L10n.Governance.masternodesAvailable(info?.masternodesControlled ?? 0)]
        lines.append(GovernanceClockViewModel.cycleLine(clock, parameters: parameters))
        if let line = GovernanceClockViewModel.budgetLine(clock, info: info, amount: amountText) { lines.append(line) }
        return lines
    }

    /// Create Proposal: synced and the wallet holds the 1 DASH fee.
    public var canCreate: Bool { isSynced && createTooltip == L10n.Governance.createTip }

    public var createTooltip: String {
        guard isSynced else { return L10n.Governance.syncRequiredTip }
        guard let balance = walletState.balances?.confirmed, balance >= parameters.proposalFee else {
            return L10n.Governance.insufficientBalanceTip(amountText(parameters.proposalFee))
        }
        return L10n.Governance.createTip
    }

    public var canResume: Bool { isSynced && walletState.selectedWalletID != nil }
    public var resumeTooltip: String { isSynced ? L10n.Governance.resumeTip : L10n.Governance.syncRequiredTip }

    /// Votes…: a synced list, a wallet and a broadcast proposal selected.
    public var canVote: Bool {
        guard isSynced, walletState.selectedWalletID != nil, let row = selectedRow else { return false }
        return row.status != .pending
    }

    public var selectedRow: ProposalRow? { rows.first { $0.id == selection } }

    private let governance: any GovernanceProviding
    private let voting: any GovernanceVoting
    private let walletState: any WalletStateProviding
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let clipboard: any ClipboardProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let timing: Timing
    private var tasks: [Task<Void, Never>] = []

    public init(
        governance: any GovernanceProviding, voting: any GovernanceVoting, walletState: any WalletStateProviding,
        auth: any AuthenticationGating, vault: any VaultProviding, clipboard: any ClipboardProviding,
        amounts: any AmountFormatting, settings: any SettingsProviding, timing: Timing
    ) {
        self.governance = governance
        self.voting = voting
        self.walletState = walletState
        self.auth = auth
        self.vault = vault
        self.clipboard = clipboard
        self.amounts = amounts
        self.settings = settings
        self.timing = timing
        parameters = governance.parameters()
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            governance: m3.governance, voting: m3.voting, walletState: env.walletState, auth: env.auth,
            vault: env.vault, clipboard: m2.clipboard, amounts: env.amounts, settings: env.settings,
            timing: env.timing)
    }

    private var amountText: AmountText { AmountText(amounts: amounts, settings: settings) }

    // MARK: Observation

    /// Turns govsync on (the tab is enabled), loads, and re-queries on every
    /// `Governance` event until `stop()`.
    public func start() async {
        stop()
        do {
            try await governance.setSyncEnabled(true)
        } catch {
            if error.isNotImplemented {
                phase = .unavailable
                return
            }
            errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
        }
        await reload()
        let changes = governance.changes()
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
            syncState = try await governance.syncState()
            rows = try await governance.proposals(query)
            phase = .ready
        } catch {
            fail(error)
            return
        }
        clock = try? await governance.clock()
        info = try? await governance.info()
        if let selection, !rows.contains(where: { $0.id == selection }) { self.selection = nil }
    }

    private var query: ProposalQuery {
        let filter = titleFilter.trimmingCharacters(in: .whitespaces)
        let source: ProposalSource =
            switch self.source {
            case .active: .active
            case .mine: walletState.selectedWalletID.map { .mine($0) } ?? .active
            }
        return ProposalQuery(source: source, titleFilter: filter.isEmpty ? nil : filter)
    }

    public func setSource(_ source: ProposalListSource) async {
        guard source != self.source else { return }
        self.source = source
        await reload()
    }

    public func setTitleFilter(_ text: String) async {
        guard text != titleFilter else { return }
        titleFilter = text
        await reload()
    }

    private func fail(_ error: ServiceError) {
        if error.isNotImplemented {
            phase = .unavailable
        } else {
            phase = .failed(ErrorText.m3(error, amount: amountText.callAsFunction))
        }
    }

    // MARK: Cells

    public func statusText(_ row: ProposalRow) -> String { L10n.Governance.status(row.status) }

    public func statusTooltip(_ row: ProposalRow) -> String {
        L10n.Governance.statusTooltip(
            row.status, yes: row.yes - row.no, needed: -row.margin, confirmations: row.collateralConfirmations,
            requiredConfirmations: parameters.feeConfirmations)
    }

    public func amountCellText(_ row: ProposalRow) -> String { amountText(row.paymentAmount) }
    public func startText(_ row: ProposalRow) -> String { M3Dates.date(row.start, timing: timing) }
    public func endText(_ row: ProposalRow) -> String { M3Dates.date(row.end, timing: timing) }

    public func votesText(_ row: ProposalRow) -> String {
        L10n.Governance.votes(yes: row.yes, no: row.no, abstain: row.abstain, margin: row.margin)
    }

    public func votesTooltip(_ row: ProposalRow) -> String {
        L10n.Governance.votesTooltip(yes: row.yes, no: row.no, abstain: row.abstain, margin: row.margin)
    }

    public func myVotesText(_ row: ProposalRow) -> String {
        row.myVotes.map(L10n.Governance.myVotes) ?? L10n.Governance.noVotingKeys
    }

    // MARK: Context menu (QT-130)

    public func copyRawJSON(_ hash: String) async {
        do {
            clipboard.setString(try await governance.detail(hash: hash).rawJSON)
        } catch {
            errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
        }
    }

    /// "Open Proposal URL…": http and https only, then the External Link
    /// Warning (No by default).
    public func requestOpenURL(_ row: ProposalRow) {
        errorMessage = nil
        guard let url = Self.safeURL(row.url) else {
            errorMessage = L10n.Governance.urlInvalid
            return
        }
        externalLink = ExternalLinkConfirmation(url: url)
    }

    /// A proposal URL the app may open: http/https with a host.
    public nonisolated static func safeURL(_ text: String) -> URL? {
        guard let url = URL(string: text.trimmingCharacters(in: .whitespaces)),
            let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
            let host = url.host, !host.isEmpty
        else { return nil }
        return url
    }

    /// Yes on the warning.
    public func confirmOpenURL() {
        urlToOpen = externalLink?.url
        externalLink = nil
    }

    /// No on the warning (the default).
    public func cancelOpenURL() {
        externalLink = nil
    }

    public func urlOpened() {
        urlToOpen = nil
    }

    /// Vote Yes / No / Abstain, or Votes… (`outcome` Yes).
    public func openVote(_ hash: String, outcome: VoteOutcome = .yes) async {
        guard let row = rows.first(where: { $0.id == hash }) else {
            errorMessage = L10n.Governance.selectProposal
            return
        }
        let dialog = ProposalVoteViewModel(
            proposal: row, outcome: outcome, voting: voting, walletState: walletState, auth: auth, vault: vault,
            deadlineText: deadlineText, timing: timing)
        vote = dialog
        await dialog.load()
    }

    public func closeVote() async {
        let voted = vote?.results != nil
        vote = nil
        if voted { await reload() }
    }

    /// Double-click: the details view.
    public func showDetails(_ hash: String) async {
        do {
            detail = try await governance.detail(hash: hash)
        } catch {
            errorMessage = ErrorText.m3(error, amount: amountText.callAsFunction)
        }
    }

    public func closeDetails() {
        detail = nil
    }

    public var detailTitle: String? { detail.map { L10n.Governance.detailsTitle($0.row.name) } }

    /// dash-qt's details HTML as lines.
    public var detailLines: [DetailLine] {
        guard let detail else { return [] }
        let row = detail.row
        typealias G = L10n.Governance
        var lines = [DetailLine(G.fieldTitle, row.name)]
        if !row.url.isEmpty { lines.append(DetailLine(G.fieldURL, row.url)) }
        lines += [
            DetailLine(G.fieldDestination, row.paymentAddress),
            DetailLine(G.fieldPaymentAmount, amountText(row.paymentAmount)),
            DetailLine(G.fieldPaymentsRequested, "\(detail.payments)"),
            DetailLine(G.fieldPaymentStart, M3Dates.dateTime(row.start, timing: timing)),
            DetailLine(G.fieldPaymentEnd, M3Dates.dateTime(row.end, timing: timing)),
            DetailLine(G.fieldObjectHash, row.hash),
            DetailLine(G.fieldParentHash, detail.parentHash),
            DetailLine(G.fieldCollateralDate, M3Dates.dateTime(detail.createdAt, timing: timing)),
            DetailLine(G.fieldCollateralHash, detail.collateralTxid),
        ]
        return lines
    }
}
