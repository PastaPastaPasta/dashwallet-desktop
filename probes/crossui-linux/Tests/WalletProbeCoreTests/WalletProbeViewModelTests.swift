import Foundation
import Observation
import Testing

@testable import WalletProbeCore

/// Thread-safe change counter for `withObservationTracking`'s `@Sendable` onChange.
/// NSLock keeps it usable on macOS 14 (Synchronization.Mutex needs macOS 15).
final class ChangeCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func increment() { lock.withLock { count += 1 } }
    var value: Int { lock.withLock { count } }
}

@MainActor
struct WalletProbeViewModelTests {
    static let validAddress = "XpESxaUmonkq8RaLLp46Brx2K39ggQe226"

    @Test func initialState() {
        let model = WalletProbeViewModel()
        #expect(model.selectedSidebarItem == .overview)
        #expect(model.detailTitle == "Overview")
        #expect(model.sendPhase == .editing)
        #expect(model.hideBalance == false)
        #expect(model.mixedFundsOnly == false)
        #expect(model.selectedTransactionID == nil)
    }

    @Test func sidebarHasTheFourWalletPagesInOrder() {
        #expect(SidebarItem.allCases.map(\.description) == ["Overview", "Send", "Receive", "Transactions"])
    }

    @Test func fiftyDeterministicSampleTransactionsWithUniqueIDs() {
        let first = WalletProbeViewModel()
        let second = WalletProbeViewModel()
        #expect(first.transactions.count == 50)
        #expect(Set(first.transactions.map(\.id)).count == 50)
        #expect(first.transactions == second.transactions)
        #expect(first.transactions[2].direction == .sent)
        #expect(first.transactions[0].summary == "Received +0.12345678 DASH, 0 confirmations, sample tx 1")
    }

    @Test func balanceIsReceivedMinusSent() {
        let model = WalletProbeViewModel(transactionCount: 3)
        // rows: +12_345_678, +24_691_356, -37_037_034
        #expect(model.balanceDuffs == 0)
        #expect(model.balanceText == "Balance: 0 DASH (sample data)")
    }

    @Test func hideBalanceMasksTheBalanceText() {
        let model = WalletProbeViewModel()
        #expect(model.balanceText.hasSuffix("DASH (sample data)"))
        model.hideBalance = true
        #expect(model.balanceText == "Balance: hidden")
    }

    @Test(arguments: [
        (Int64(0), "0"),
        (Int64(1), "0.00000001"),
        (Int64(150_000_000), "1.5"),
        (Int64(-12_345_678), "-0.12345678"),
        (Int64(2_100_000_000_000_000), "21000000"),
    ])
    func amountFormatting(duffs: Int64, expected: String) {
        #expect(DashAmountFormatter.string(fromDuffs: duffs) == expected)
    }

    @Test func amountFormattingHandlesInt64Min() {
        #expect(DashAmountFormatter.string(fromDuffs: .min) == "-92233720368.54775808")
    }

    @Test(arguments: [
        ("", false),
        ("XpESxaUmonkq8RaLLp46Brx2K39ggQe22", false),   // 33 chars
        ("YpESxaUmonkq8RaLLp46Brx2K39ggQe226", false),  // testnet-style prefix
        ("XpESxaUmonkq8RaLLp46Brx2K39ggQe2I0", false),  // 'I' and '0' are not Base58
        ("XpESxaUmonkq8RaLLp46Brx2K39ggQe226", true),
        ("  XpESxaUmonkq8RaLLp46Brx2K39ggQe226\n", true),
        ("7gnwGHt17heGpG9Crfeh4KGpYNFugPhJdh", true),
    ])
    func addressFormatCheck(text: String, expected: Bool) {
        #expect(WalletProbeViewModel.isPlausibleMainnetAddress(text) == expected)
    }

    @Test func sendWithInvalidAddressReportsInvalid() {
        let model = WalletProbeViewModel()
        model.sendAddress = "not an address"
        model.send()
        #expect(model.sendPhase == .invalidAddress)
        #expect(model.sendStatusText.hasPrefix("Invalid address"))
    }

    @Test func sendWithValidAddressReportsNotImplementedNeverSuccess() {
        let model = WalletProbeViewModel()
        model.sendAddress = Self.validAddress
        model.send()
        #expect(model.sendPhase == .notImplemented)
        #expect(model.sendStatusText.contains("not implemented"))
    }

    @Test func editingTheAddressResetsTheSendPhase() {
        let model = WalletProbeViewModel()
        model.sendAddress = "bad"
        model.send()
        #expect(model.sendPhase == .invalidAddress)
        model.sendAddress = "bad!"
        #expect(model.sendPhase == .editing)
    }

    @Test func observationFiresWhenTheAddressChanges() {
        let model = WalletProbeViewModel()
        let changes = ChangeCounter()
        withObservationTracking {
            _ = model.sendStatusText
        } onChange: {
            changes.increment()
        }
        model.sendAddress = Self.validAddress
        #expect(changes.value == 0)  // phase was already .editing, so the tracked status is unchanged
        model.send()
        #expect(changes.value == 1)
    }

    @Test func observationFiresWhenTheSidebarSelectionChanges() {
        let model = WalletProbeViewModel()
        let changes = ChangeCounter()
        withObservationTracking {
            _ = model.detailTitle
        } onChange: {
            changes.increment()
        }
        model.selectedSidebarItem = .transactions
        #expect(changes.value == 1)
        #expect(model.detailTitle == "Transactions")
    }
}
