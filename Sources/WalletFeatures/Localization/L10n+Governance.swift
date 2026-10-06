import Foundation
import WalletRuntime

extension L10n {
    /// dash-qt's governance copy (`proposallist.cpp`, `proposalmodel.cpp`,
    /// `proposalvotedialog.cpp`, `proposalcreate.cpp`, `proposalresume.cpp`,
    /// `proposalinfo.ui`; research 02 §11).
    public enum Governance {
        // List (QT-128…130)
        public static let activeProposals = "Active Proposals"
        public static let myProposals = "My Proposals"
        public static let filterByTitle = "Filter by Title"
        public static let proposalCount = "Proposal Count:"
        public static let votesButton = "Votes…"
        public static let createProposal = "Create Proposal"
        public static let resumeProposal = "Resume Proposal"
        public static let createTip = "Creates a new proposal"
        public static let resumeTip = "Resumes an existing proposal"
        public static let syncRequiredTip = "Cannot interact with governance before sync completes"
        public static func insufficientBalanceTip(_ fee: String) -> String {
            "Creating proposals costs \(fee), insufficient balance"
        }
        public static let noActiveProposals = "No active proposals on the network."
        public static let noLocalProposals = "No proposals recorded in wallet file."
        public static let fromPeers = "Governance data comes from peers and may lag behind the network."
        public static let columnStatus = "Status"
        public static let columnTitle = "Title"
        public static let columnAmount = "Amount"
        public static let columnStart = "Start"
        public static let columnEnd = "End"
        public static let columnVotes = "Votes"
        public static let columnMyVotes = "My Votes"
        public static let columnHash = "Hash"
        public static let noVotingKeys = "No voting keys"
        public static func votes(yes: Int, no: Int, abstain: Int, margin: Int) -> String {
            "\(yes)Y, \(no)N, \(abstain)A (\(margin > 0 ? "+" : "")\(margin))"
        }
        public static func votesTooltip(yes: Int, no: Int, abstain: Int, margin: Int) -> String {
            let tail = margin >= 0 ? "passing with \(abs(margin)) votes" : "needs \(abs(margin)) more votes"
            return "\(yes) Yes, \(no) No, \(abstain) Abstain, \(tail)"
        }
        public static func myVotes(_ votes: MyVotes) -> String {
            "\(votes.yes)Y, \(votes.no)N, \(votes.abstain)A / \(votes.unvoted) unvoted"
        }
        public static let myVotesTooltip =
            "Current funding votes from eligible masternodes controlled by this wallet. Counts are weighted votes."

        public static func status(_ status: ProposalStatus) -> String {
            switch status {
            case .funded: "Funded"
            case .lapsed: "Lapsed"
            case .confirming: "Confirming"
            case .pending: "Pending"
            case .passing: "Passing"
            case .failing: "Failing"
            case .voting: "Voting"
            case .unfunded: "Unfunded"
            }
        }

        /// The status column's tooltip (`ProposalModel::data`, ToolTipRole).
        /// `needed` = votes still missing for the threshold.
        public static func statusTooltip(
            _ status: ProposalStatus, yes: Int, needed: Int, confirmations: Int?, requiredConfirmations: Int
        ) -> String {
            switch status {
            case .confirming: "Pending, \(confirmations ?? 0) of \(requiredConfirmations) confirmations"
            case .voting: "Voting, needs \(max(0, needed)) more votes for funding"
            case .passing: "Passing with \(yes) votes"
            case .unfunded: "Passing with \(yes) votes but budget saturated, may not be funded"
            case .failing: "Failed, needed \(max(0, needed)) more votes"
            case .funded: "Funded"
            case .lapsed: "Lapsed, past proposal end date"
            case .pending: "Ready to broadcast, check \"Resume Proposal\" dialog"
            }
        }

        public static func deadline(left: String, blocks: UInt32, block: UInt32) -> String {
            "Voting deadline: ~\(left) left (\(blocks) blocks, block \(block))"
        }
        public static func deadlinePassed(_ block: UInt32) -> String {
            "Voting deadline passed for this cycle (block \(block))"
        }
        public static let deadlineWaiting = "Voting deadline: waiting for sync…"
        public static let deadlineTooltip =
            "Estimated from the remaining blocks and target block time. Votes can still be relayed after the deadline, but may not affect this cycle's payment."
        public static let waitingBlockchain = "Waiting for blockchain sync…"
        public static let waitingGovernance = "Waiting for governance sync…"
        public static func masternodesAvailable(_ n: Int) -> String {
            "\(n) masternode\(n == 1 ? "" : "s") available for voting"
        }
        public static func leftForVoting(_ left: String, _ blocks: UInt32) -> String {
            "~\(left) (\(blocks) blocks) left for voting"
        }
        public static func leftForSuperblock(_ left: String, _ blocks: UInt32) -> String {
            "~\(left) (\(blocks) blocks) left for superblock"
        }
        public static let superblockImminent = "Superblock imminent"
        public static let votingEnded = "Voting period ended"
        public static func budgetCommitted(percent: Int, allocated: String, budget: String) -> String {
            "~\(percent)% of budget committed (\(allocated) / \(budget))"
        }

        public static func syncPhase(_ phase: GovernanceSyncPhase) -> String {
            switch phase {
            case .disabled: "Governance sync is off."
            case .waiting: "Waiting for blockchain sync…"
            case .syncingObjects: "Downloading proposals from peers…"
            case .syncingVotes: "Downloading votes from peers…"
            case .synced: "Governance data synced from peers."
            case .failed: "Governance sync failed. It retries with other peers."
            }
        }

        // Context menu (QT-130)
        public static let copyRawJSON = "Copy Raw JSON"
        public static let openURL = "Open Proposal URL…"
        public static let voteYes = "Vote Yes"
        public static let voteNo = "Vote No"
        public static let voteAbstain = "Vote Abstain"
        public static let urlInvalid = "Cannot validate URL, potentially malformed or unknown protocol."
        public static let externalLinkTitle = "External Link Warning"
        public static func externalLinkMessage(_ url: String) -> String {
            "You are about to open the following URL in your default browser\n\n\(url)\n\nThis content was submitted by a user. It may not match what is described in the title.\n\nDo you wish to continue?"
        }
        public static func detailsTitle(_ name: String) -> String { "Details for \(name)" }
        public static let fieldTitle = "Title"
        public static let fieldURL = "URL"
        public static let fieldDestination = "Destination Address"
        public static let fieldPaymentAmount = "Payment Amount"
        public static let fieldPaymentsRequested = "Payments Requested"
        public static let fieldPaymentStart = "Payment Start"
        public static let fieldPaymentEnd = "Payment End"
        public static let fieldObjectHash = "Object Hash"
        public static let fieldParentHash = "Parent Hash"
        public static let fieldCollateralDate = "Collateral Date"
        public static let fieldCollateralHash = "Collateral Hash"

        // Vote dialog (QT-131)
        public static let voteDialogTitle = "Proposal Votes"
        public static let voteInstructions =
            "Select the masternodes to vote with. Current funding votes are shown below. Changing a vote replaces that masternode's previous vote; the network limits updates to once per hour."
        public static func outcome(_ outcome: VoteOutcome?) -> String {
            switch outcome {
            case .yes: "Yes"
            case .no: "No"
            case .abstain: "Abstain"
            case nil: "Not voted"
            }
        }
        public static let columnMasternode = "Masternode"
        public static let columnVotingAddress = "Voting Address"
        public static let columnWeight = "Weight"
        public static let columnCurrentVote = "Current Vote"
        public static let columnVoteTime = "Vote Time"
        public static let columnProTxHash = "ProTx Hash"
        public static let selectAll = "Select All"
        public static let clearSelection = "Clear Selection"
        public static func selectionSummary(count: Int, weight: Int) -> String {
            "Masternodes selected: \(count) · Vote weight: \(weight)"
        }
        public static func voteButton(_ outcome: VoteOutcome) -> String { "Vote \(L10n.Governance.outcome(outcome))" }
        public static func nextVoteAt(_ time: String) -> String { "Can change its vote after \(time)" }
        public static let resultsTitle = "Voting Results"
        public static func votedSuccessfully(_ n: Int) -> String { "Voted successfully \(n) time\(n == 1 ? "" : "s")" }
        public static func failedToVote(_ n: Int) -> String { "Failed to vote \(n) time\(n == 1 ? "" : "s")" }
        public static let errorsHeader = "Errors:"
        public static func masternodeError(_ hash: String, _ text: String) -> String { "Masternode \(hash): \(text)" }
        public static let votingFailed = "Voting Failed"
        public static let selectProposal = "Please select a proposal to vote on."

        // Create Proposal (QT-132)
        public static let createTitle = "New proposal"
        public static let fieldName = "Proposal name"
        public static let fieldDescriptionURL = "Description URL"
        public static let fieldPaymentDate = "Payment date"
        public static let fieldPayments = "Payments"
        public static let fieldPaymentAddress = "Payment address"
        public static let fieldAmount = "Payment amount"
        public static let fieldTotal = "Total amount"
        public static let amountTip = "The amount to request in a single payment"
        public static let viewJSON = "View JSON"
        public static let viewPayload = "View Payload"
        public static let allFieldsMandatory = "All fields are mandatory"
        public static let paymentDateHonoured =
            "Payments start at the chosen superblock. The dates are estimates from the target block time."
        public static func superblockOption(height: UInt32, date: String) -> String { "Block \(height) (~\(date))" }
        public static func fieldError(_ field: ProposalField) -> String {
            switch field {
            case .name: "Name must be 1–40 characters: lowercase letters, digits, - or _."
            case .url: "URL must have at least 4 characters and no spaces."
            case .paymentAddress: "Enter a valid Dash address (P2PKH or P2SH) of this network."
            case .paymentAmount: "Payment amount must be greater than zero."
            case .paymentCount: "Payments must be between 1 and 12."
            case .firstPayment: "Choose one of the next 12 superblocks."
            case .payload: "The proposal is too large (512 bytes at most)."
            }
        }
        public static let confirmTitle = "Confirm Proposal"
        public static let confirmQuestion = "Are you sure you want to create this proposal?"
        public static func confirmFee(_ fee: String) -> String {
            "Creating a proposal pays \(fee) to the network. This fee is non-refundable regardless of outcome."
        }
        public static let createdTitle = "Proposal Created"
        public static func created(fee: String, name: String) -> String {
            "\(fee) successfully sent for your proposal \"\(name)\".\n\nYou will now be redirected to monitor and broadcast your new proposal, you can resume this later by clicking \"Resume Proposal\"."
        }
        public static let creationFailed = "Creation failed"

        // Resume Proposals (QT-133)
        public static let resumeTitle = "Resume Proposals"
        public static let noPending = "No pending proposals to broadcast."
        public static func fundSummary(payments: Int, amount: String, url: String) -> String {
            "For \(payments) payment\(payments == 1 ? "" : "s") of \(amount) to \(url)"
        }
        public static func collateralStatus(_ status: CollateralStatus) -> String {
            switch status {
            case .unknown: "Unknown"
            case .pending: "Pending"
            case .ready: "Ready"
            }
        }
        public static let collateralStatusTitle = "Collateral Status"
        public static let broadcast = "Broadcast"
        public static let broadcastTitle = "Broadcast proposal"
        public static func broadcasted(_ hash: String) -> String {
            "Proposal has been broadcasted to the network with hash \(hash)"
        }
        public static func broadcastFailed(_ reason: String) -> String { "Unable to broadcast proposal, \(reason)" }

        // Information ▸ Governance (QT-134, `proposalinfo.ui`)
        public static let infoGeneral = "General"
        public static let votingCycles = "Voting cycles"
        public static let lastSuperblock = "Last superblock"
        public static let nextSuperblock = "Next superblock"
        public static let votingCutoff = "Voting cutoff block"
        public static let participation = "Participation"
        public static let masternodesVoting = "Masternodes voting"
        public static let evonodesVoting = "EvoNodes voting"
        public static let passingThreshold = "Passing threshold"
        public static let node = "Node"
        public static let masternodesControlled = "Masternodes controlled"
        public static let votesControlled = "Votes controlled"
        public static let proposalsSection = "Proposals"
        public static let proposalCountTitle = "Proposal Count"
        public static let budgetAllocated = "Budget allocated"
        public static let passingProposals = "Passing Proposals"
        public static let unfundedProposals = "Unfunded proposals"
        public static let failingProposals = "Failing Proposals"
        public static let notAvailable = "N/A"
        public static func cycleBlocks(_ blocks: UInt32, _ duration: String) -> String { "\(blocks) blocks (~\(duration))" }
        public static func superblockAt(_ height: UInt32, _ date: String?) -> String {
            date.map { "\(height) (~\($0))" } ?? "\(height)"
        }
        public static func participationValue(_ voting: Int, eligible: Int) -> String { "\(voting) (\(eligible) eligible)" }
        public static func unfundedValue(_ count: Int, short: String?) -> String {
            short.map { "\(count) (short \($0))" } ?? "\(count)"
        }
        public static func budgetValue(allocated: String, available: String, percent: Int) -> String {
            "\(allocated) / \(available) (\(percent)%)"
        }

        // Clock (QT-026)
        public static let clockOpensGovernance = "Click to open the Governance tab."
    }

    /// dash-qt's `GUIUtil::formatBlockDuration`.
    public enum Durations {
        public static func blocks(_ blocks: UInt32, spacing: Duration) -> String {
            guard blocks > 0 else { return "now" }
            let spacingSeconds = Double(spacing.components.seconds)
            let secs = Double(blocks) * spacingSeconds
            let minute = 60.0, hour = 3_600.0, day = 86_400.0, month = 30.44 * 86_400, year = 365.25 * 86_400
            func plural(_ n: Int, _ unit: String) -> String { "\(n) \(unit)\(n == 1 ? "" : "s")" }
            if secs < hour { return plural(Int(secs / minute + 0.5), "minute") }
            if secs < day { return plural(Int(secs / hour + 0.5), "hour") }
            if secs < month { return plural(Int(secs / day + 0.5), "day") }
            if secs < year { return plural(Int(secs / month + 0.5), "month") }
            return plural(Int(secs / year + 0.5), "year")
        }
    }
}
