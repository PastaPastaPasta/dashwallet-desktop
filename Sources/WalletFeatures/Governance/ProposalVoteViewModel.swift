// dash-qt's "Proposal Votes" dialog (QT-131): the outcome, a checkable table
// of the masternodes whose voting key the wallets hold, Select All / Clear
// Selection, the weight summary, "Vote %1", and the results box with each
// failing masternode's error. A masternode inside the network's 1-hour
// window shows when it may change its vote.
import Foundation
import Observation
import WalletRuntime

public enum ProposalVotePhase: Sendable, Hashable {
    case loading
    case unavailable
    case ready
    /// Voting waits for the wallet passphrase (a full unlock).
    case needsPassphrase
    case voting
    case done
    case failed(String)
}

@MainActor
@Observable
public final class ProposalVoteViewModel {
    public let proposal: ProposalRow
    public var outcome: VoteOutcome
    public private(set) var phase: ProposalVotePhase = .loading
    public private(set) var masternodes: [VotingMasternode] = []
    public private(set) var selected: Set<String> = []
    public private(set) var results: [VoteResult]?
    public private(set) var errorMessage: String?

    public var title: String { L10n.Governance.voteDialogTitle }
    public var instructions: String { L10n.Governance.voteInstructions }
    public var voteButtonTitle: String { L10n.Governance.voteButton(outcome) }

    /// The list's "Voting deadline: …" line, repeated in the dialog.
    public let deadlineText: String?

    /// "Masternodes selected: %1 · Vote weight: %2".
    public var summary: String {
        let weight = masternodes.filter { selected.contains($0.proTxHash) }.reduce(0) { $0 + $1.weight }
        return L10n.Governance.selectionSummary(count: selected.count, weight: weight)
    }

    public var canVote: Bool { (phase == .ready || phase == .needsPassphrase) && !selected.isEmpty }

    /// "Voted successfully %n time(s)" / "Failed to vote %n time(s)" and the
    /// per-masternode errors.
    public var resultLines: [String] {
        guard let results else { return [] }
        let failed = results.filter { $0.errorCode != nil }
        var lines: [String] = []
        let succeeded = results.count - failed.count
        if succeeded > 0 { lines.append(L10n.Governance.votedSuccessfully(succeeded)) }
        if !failed.isEmpty {
            lines.append(L10n.Governance.failedToVote(failed.count))
            lines.append(L10n.Governance.errorsHeader)
            for result in failed {
                lines.append(L10n.Governance.masternodeError(result.proTxHash, failureText(result)))
            }
        }
        return lines
    }

    private let voting: any GovernanceVoting
    private let walletState: any WalletStateProviding
    private let grants: GrantRequester
    private let timing: Timing

    public init(
        proposal: ProposalRow, outcome: VoteOutcome, voting: any GovernanceVoting,
        walletState: any WalletStateProviding, auth: any AuthenticationGating, vault: any VaultProviding,
        deadlineText: String? = nil, timing: Timing
    ) {
        self.deadlineText = deadlineText
        self.proposal = proposal
        self.outcome = outcome
        self.voting = voting
        self.walletState = walletState
        grants = GrantRequester(auth: auth, vault: vault)
        self.timing = timing
    }

    // MARK: Table

    public func outcomeText(_ masternode: VotingMasternode) -> String { L10n.Governance.outcome(masternode.currentVote) }

    public func voteTimeText(_ masternode: VotingMasternode) -> String {
        masternode.voteTime.map { M3Dates.dateTime($0, timing: timing) } ?? ""
    }

    public func name(_ masternode: VotingMasternode) -> String {
        masternode.label ?? masternode.collateral
    }

    /// "Can change its vote after …" while the 1-hour window is open.
    public func waitText(_ masternode: VotingMasternode) -> String? {
        guard let next = masternode.nextVoteAt, next > timing.now() else { return nil }
        return L10n.Governance.nextVoteAt(M3Dates.dateTime(next, timing: timing))
    }

    /// Loads the masternodes; every one is selected except those inside the
    /// 1-hour window.
    public func load() async {
        phase = .loading
        do {
            masternodes = try await voting.votingMasternodes(proposal: proposal.hash, wallet: walletState.selectedWalletID)
        } catch {
            phase = error.isNotImplemented ? .unavailable : .failed(ErrorText.m3(error, amount: { "\($0.duffs)" }))
            return
        }
        if masternodes.isEmpty {
            phase = .failed(L10n.M3Errors.noVotingKeys)
            return
        }
        let now = timing.now()
        selected = Set(masternodes.filter { ($0.nextVoteAt ?? .distantPast) <= now }.map(\.proTxHash))
        phase = .ready
    }

    public func toggle(_ proTxHash: String) {
        guard masternodes.contains(where: { $0.proTxHash == proTxHash }) else { return }
        if selected.contains(proTxHash) { selected.remove(proTxHash) } else { selected.insert(proTxHash) }
    }

    public func selectAll() { selected = Set(masternodes.map(\.proTxHash)) }
    public func clearSelection() { selected = [] }

    // MARK: Vote

    /// Signs and relays one vote per selected masternode with a
    /// `.governance` grant; an encrypted vault needs `passphrase`.
    public func vote(passphrase: String? = nil) async {
        guard canVote, let wallet = walletState.selectedWalletID else { return }
        errorMessage = nil
        let grant: AuthGrant
        do {
            guard let issued = try await grants.authorize(.governance, wallet: wallet, passphrase: passphrase) else {
                phase = .needsPassphrase
                return
            }
            grant = issued
        } catch {
            errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" })
            phase = error.code == .vaultWrongPassphrase ? .needsPassphrase : .ready
            return
        }
        phase = .voting
        let hashes = masternodes.map(\.proTxHash).filter { selected.contains($0) }
        do {
            results = try await voting.cast(outcome, on: proposal.hash, with: hashes, grant: grant)
            phase = .done
        } catch {
            grants.revoke(grant)
            errorMessage = ErrorText.m3(error, amount: { "\($0.duffs)" })
            phase = .ready
        }
    }

    public func cancelPassphrase() {
        if phase == .needsPassphrase { phase = .ready }
    }

    private func failureText(_ result: VoteResult) -> String {
        guard let code = result.errorCode else { return "" }
        if code == .governanceVoteTooOften,
            let masternode = masternodes.first(where: { $0.proTxHash == result.proTxHash }),
            let wait = waitText(masternode)
        {
            return "\(L10n.M3Errors.voteTooOften) \(wait)"
        }
        return ErrorText.m3(ServiceError(code: code), amount: { "\($0.duffs)" })
    }
}
