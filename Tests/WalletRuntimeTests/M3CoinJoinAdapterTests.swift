// The CoinJoin adapter's conversions between DashKit and the WalletRuntime
// contract (m3-swift.md §2.1). No engine needed.
import DashKit
import Foundation
import Testing
@testable import WalletRuntime

struct M3CoinJoinAdapterTests {
    /// Both enums list Core's `PoolMessage` in wire order, so the position
    /// mapping keeps every message (QT-050 status text).
    @Test func QT050_poolMessagesKeepTheirMeaning() {
        #expect(CoinJoinMessage.allCases.count == CoinJoinPoolMessage.allCases.count)
        #expect(CoinJoinPoolMessage(CoinJoinMessage.alreadyHave) == .alreadyHave)
        #expect(CoinJoinPoolMessage(CoinJoinMessage.queueFull) == .queueFull)
        #expect(CoinJoinPoolMessage(CoinJoinMessage.success) == .success)
        #expect(CoinJoinPoolMessage(CoinJoinMessage.sizeMismatch) == .sizeMismatch)
        #expect(CoinJoinStatusCode(CoinJoinCode.masternode(.entriesAdded)) == .masternode(.entriesAdded))
        #expect(CoinJoinStatusCode(CoinJoinCode.syncInProgress) == .syncInProgress)
    }

    /// Options round-trip; a negative value is refused before the engine
    /// (QT-046).
    @Test func QT046_optionsConvertBothWays() throws {
        let s = CoinJoinSettings.dashQtDefaults
        let kit = try s.kit
        #expect(kit.rounds == 4 && kit.targetAmountDash == 1000 && kit.denomsHardCap == 300)
        #expect(CoinJoinSettings(kit) == s)
        var bad = s
        bad.rounds = -1
        do throws(ServiceError) {
            _ = try bad.kit
            Issue.record("a negative rounds value must be refused")
        } catch {
            #expect(error.code == .invalidArgument)
        }
    }

    @Test func IOS057_destinationsMapOneToOne() {
        #expect(MixedCoinsDestination.wallet.kit == .wallet)
        #expect(MixedCoinsDestination.shielded.kit == .shielded)
    }
}
