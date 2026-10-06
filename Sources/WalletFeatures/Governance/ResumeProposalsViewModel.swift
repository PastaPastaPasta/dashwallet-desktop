// dash-qt's Resume Proposals dialog (QT-133): the wallet's created, not yet
// broadcast proposals with their collateral status; "Broadcast" at ≥ 1
// confirmation; re-queried on `Governance` events (confirmations arrive).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class ResumeProposalsViewModel {
    public private(set) var pending: [PendingProposal] = []
    public private(set) var available = true
    public private(set) var isLoading = false
    public private(set) var broadcasting: String?
    /// "Proposal has been broadcasted to the network with hash %1".
    public private(set) var message: String?
    public private(set) var errorMessage: String?

    public var title: String { L10n.Governance.resumeTitle }
    public var emptyText: String? { available && pending.isEmpty && !isLoading ? L10n.Governance.noPending : nil }

    public func canBroadcast(_ proposal: PendingProposal) -> Bool {
        proposal.collateralStatus == .ready && broadcasting == nil
    }

    /// "For %1 payment(s) of %2 to %3".
    public func fundSummary(_ proposal: PendingProposal) -> String {
        L10n.Governance.fundSummary(
            payments: proposal.paymentCount, amount: AmountText(amounts: amounts, settings: settings)(proposal.paymentAmount),
            url: proposal.url)
    }

    public func collateralStatusText(_ proposal: PendingProposal) -> String {
        L10n.Governance.collateralStatus(proposal.collateralStatus)
    }

    private let proposals: any ProposalCreating
    private let governance: any GovernanceProviding
    private let walletState: any WalletStateProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private var task: Task<Void, Never>?

    public init(
        proposals: any ProposalCreating, governance: any GovernanceProviding, walletState: any WalletStateProviding,
        amounts: any AmountFormatting, settings: any SettingsProviding
    ) {
        self.proposals = proposals
        self.governance = governance
        self.walletState = walletState
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(env: AppEnvironment, m3: M3Services) {
        self.init(
            proposals: m3.proposals, governance: m3.governance, walletState: env.walletState, amounts: env.amounts,
            settings: env.settings)
    }

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
        guard let wallet = walletState.selectedWalletID else {
            pending = []
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            pending = try await proposals.pending(wallet: wallet)
            available = true
        } catch {
            if error.isNotImplemented {
                available = false
            } else {
                errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" })
            }
        }
    }

    /// `gobject submit`; the proposal then shows as Confirming in the list.
    public func broadcast(_ hash: String) async {
        guard let wallet = walletState.selectedWalletID, let proposal = pending.first(where: { $0.hash == hash }),
            canBroadcast(proposal)
        else { return }
        broadcasting = hash
        message = nil
        errorMessage = nil
        defer { broadcasting = nil }
        do {
            let objectHash = try await proposals.submit(wallet: wallet, hash: hash)
            message = L10n.Governance.broadcasted(objectHash)
        } catch {
            errorMessage = L10n.Governance.broadcastFailed(
                ErrorText.m3(error, amount: AmountText(amounts: amounts, settings: settings).callAsFunction))
        }
        await reload()
    }
}
