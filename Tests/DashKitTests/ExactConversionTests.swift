@testable import DashKit
import DashWalletCore
import Foundation
import Testing

/// Review M-8: Swift → FFI integer conversions fail with typed errors, never
/// trap; review M-5: errors keep the numbers the UI needs.
@Suite struct ExactConversionTests {
    @Test func negativeRecipientAmountIsARecipientError() {
        let recipient = Recipient(address: "a", amount: Amount(duffs: -1))
        #expect(throws: DashKitError.recipient(code: "send.invalid_amount", index: 3)) {
            _ = try recipient.ffi(index: 3)
        }
    }

    @Test func negativeAmountsAreInvalidArguments() {
        #expect(throws: DashKitError.self) { _ = try GrantPurpose.spend(max: Amount(duffs: -5)).ffi() }
        #expect(throws: DashKitError.self) { _ = try FeeMode.perKilobyte(Amount(duffs: -1)).ffi() }
        #expect(throws: DashKitError.self) {
            _ = try CoreFunctions.buildPaymentURI(address: "a", amount: Amount(duffs: -1), label: nil, message: nil)
        }
    }

    @Test func historyLimitAndDatesAreChecked() {
        #expect(throws: DashKitError.self) { _ = try HistoryQuery(limit: 0).ffi() }
        #expect(throws: DashKitError.self) { _ = try HistoryQuery(limit: 501).ffi() }
        let farFuture = HistoryFilter(until: Date(timeIntervalSince1970: 1e30))
        #expect(throws: DashKitError.self) { _ = try farFuture.ffi() }
        // Before 1970 clamps to the epoch.
        let early = try? HistoryFilter(from: Date(timeIntervalSince1970: -10)).ffi()
        #expect(early?.dateFrom == 0)
    }

    @Test func wordCountOutsideUInt8IsTyped() {
        for count in [-1, 256, Int.max] {
            #expect(throws: DashKitError.domain(code: "wallet.unsupported_word_count", detail: "\(count)")) {
                _ = try CoreFunctions.generateMnemonic(wordCount: count, language: .english)
            }
        }
    }

    @Test func parameterizedErrorsExposeTheirNumbers() {
        let error = DashKitError.parameterized(
            code: "send.amount_with_fee_exceeds_balance", parameters: ["fee": 226, "available": 1_000])
        #expect(error.code == "send.amount_with_fee_exceeds_balance")
        #expect(error.parameters == ["fee": 226, "available": 1_000])
        #expect(error.detail == "available=1000 fee=226")
        let attempt = DashKitError.vaultAttempt(code: "vault.wrong_passphrase", failedAttempts: 4, retryAfterSeconds: 60)
        #expect(attempt.parameters == ["failed_attempts": 4, "retry_after_secs": 60])
        #expect(DashKitError.recipient(code: "send.dust_amount", index: 2).parameters == ["index": 2])
        #expect(DashKitError.storage(detail: "x").parameters.isEmpty)
    }
}
