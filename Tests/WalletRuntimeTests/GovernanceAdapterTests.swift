// The governance adapter's mapping of DashKit values to the WalletRuntime
// contract (m3-swift.md §2.2): units, times, statuses and the draft check.
import Foundation
import Testing
@testable import DashKit
@testable import WalletRuntime

struct GovernanceAdapterTests {
    @Test func QT129_rowKeepsTalliesMarginAndUnknownConfirmations() {
        let kit = GovProposalRow(
            hash: String(repeating: "ab", count: 32), name: "dwd", url: "https://dash.org", paymentAddress: "yX",
            paymentAmount: 1_250_000_000, startEpoch: 1_700_000_000, endEpoch: 1_700_086_400, status: .unfunded,
            collateralConfirmations: nil, yes: 12, no: 3, abstain: 1, margin: -2,
            myVotes: GovMyVotes(yes: 4, no: 0, abstain: 0, unvoted: 1))
        let row = ProposalRow(kit)
        #expect(row.status == .unfunded)
        #expect(row.paymentAmount == Amount(duffs: 1_250_000_000))
        #expect(row.start == Date(timeIntervalSince1970: 1_700_000_000))
        #expect((row.yes, row.no, row.abstain, row.margin) == (12, 3, 1, -2))
        #expect(row.collateralConfirmations == nil)
        #expect(row.myVotes == MyVotes(yes: 4, no: 0, abstain: 0, unvoted: 1))
    }

    @Test func QT132_draftOutOfEngineRangeIsInvalidArgument() throws {
        let ok = ProposalDraft(
            name: "dwd", url: "https://dash.org", paymentAddress: "yX", paymentAmount: Amount(duffs: 5),
            paymentCount: 2, firstSuperblockHeight: 1_520)
        let kit = try ok.kit
        #expect(kit.paymentAmount == 5 && kit.paymentCount == 2 && kit.firstSuperblockHeight == 1_520)
        var bad = ok
        bad.paymentAmount = Amount(duffs: -1)
        do throws(ServiceError) {
            _ = try bad.kit
            Issue.record("a negative amount must be refused")
        } catch {
            #expect(error.code == .invalidArgument)
        }
    }

    @Test func QT131_outcomesRoundTrip() {
        for outcome in VoteOutcome.allCases {
            #expect(VoteOutcome(outcome.kit) == outcome)
        }
    }

    @Test func QT133_pendingProposalKeepsCollateralState() {
        let p = PendingProposal(
            GovPendingProposal(
                hash: "h", name: "n", url: "u", paymentAmount: 7, paymentCount: 3, collateralTxid: "t",
                collateralStatus: .ready, confirmations: 6, createdAt: 10, endEpoch: 20))
        #expect(p.collateralStatus == .ready)
        #expect(p.confirmations == 6 && p.paymentCount == 3)
        #expect(p.end == Date(timeIntervalSince1970: 20))
    }
}
