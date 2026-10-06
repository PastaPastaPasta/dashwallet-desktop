// UX-SPEC §5 presentation helpers: compact amounts, amount splitting, list
// titles and chips, day headers, shortened addresses.
import Foundation
import Testing
import WalletRuntime

@testable import WalletFeatures

@Suite("Presentation helpers")
struct PresentationTests {
    private func status(_ kind: TxStatusKind, instant: Bool = false, chain: Bool = false) -> TxStatus {
        TxStatus(kind: kind, confirmations: 0, instantLocked: instant, chainLocked: chain, maturesIn: nil)
    }

    @Test("compact keeps every duff, trims zeros to two decimals and never rounds")
    func compactAmounts() {
        #expect(CompactAmount.format(Amount(duffs: 20_000_000), unit: .dash, signed: false) == "0.20")
        #expect(CompactAmount.format(Amount(duffs: 3_039_698_287), unit: .dash, signed: false) == "30.39698287")
        #expect(CompactAmount.format(Amount(duffs: -50_000_226), unit: .dash, signed: true) == "-0.50000226")
        #expect(CompactAmount.format(Amount(duffs: 125_000_000), unit: .dash, signed: true) == "+1.25")
        #expect(CompactAmount.format(Amount(duffs: 100_000_000), unit: .dash, signed: true) == "+1.00")
        #expect(CompactAmount.format(.zero, unit: .dash, signed: true) == "0.00")
        #expect(CompactAmount.format(Amount(duffs: 1), unit: .dash, signed: false) == "0.00000001")
        // dash-qt grouping above four integer digits, thin space U+2009.
        #expect(CompactAmount.format(Amount(duffs: 1_234_567 * 100_000_000), unit: .dash, signed: false)
            == "1\u{2009}234\u{2009}567.00")
        #expect(CompactAmount.format(Amount(duffs: 12_345), unit: .milliDash, signed: false) == "0.12345")
        #expect(CompactAmount.format(Amount(duffs: 12_345), unit: .duffs, signed: false) == "12\u{2009}345")
        #expect(CompactAmount.trim("5.10000") == "5.10")
        #expect(CompactAmount.trim("5") == "5")
    }

    @Test("split separates sign and number from the unit name")
    func splitAmounts() {
        let received = AmountParts.split("+0.20000000 tDASH")
        #expect(received.number == "+0.20000000" && received.unit == "tDASH" && received.isDash && !received.bracketed)
        let bracketed = AmountParts.split("[-1.5 DASH]")
        #expect(bracketed.number == "-1.5" && bracketed.unit == "DASH" && bracketed.bracketed)
        let milli = AmountParts.split("250.00374 mtDASH")
        #expect(milli.unit == "mtDASH" && !milli.isDash)
        let grouped = AmountParts.split("30\u{2009}391.69 tDASH")
        #expect(grouped.number == "30\u{2009}391.69")
        let masked = AmountParts.split("###.## tDASH")
        #expect(masked.number == "###.##")
        #expect(AmountParts.split("Unknown").unit == nil)
    }

    @Test("titles prefer the label, never the address; chips are quiet when settled")
    func titlesAndChips() {
        #expect(TxPresentation.title(type: .recvWithAddress, amount: Amount(duffs: 5), label: "Coffee") == "Coffee")
        #expect(TxPresentation.title(type: .recvWithAddress, amount: Amount(duffs: 5), label: "") == "Received")
        #expect(TxPresentation.title(type: .sendToAddress, amount: Amount(duffs: -5), label: nil) == "Sent")
        #expect(TxPresentation.title(type: .sendToSelf, amount: .zero, label: nil) == "Sent to yourself")
        #expect(TxPresentation.title(type: .generated, amount: Amount(duffs: 5), label: nil) == "Mined")
        #expect(TxPresentation.title(type: .recvWithAddress, category: .reward, amount: Amount(duffs: 5), label: nil)
            == "Masternode reward")
        #expect(TxPresentation.chip(status(.unconfirmed)) == "Pending")
        #expect(TxPresentation.chip(status(.unconfirmed, instant: true)) == "InstantSend")
        #expect(TxPresentation.chip(status(.confirming, instant: true)) == "InstantSend")
        #expect(TxPresentation.chip(status(.confirming, instant: true, chain: true)) == nil)
        #expect(TxPresentation.chip(status(.confirmed)) == nil)
        #expect(TxPresentation.chip(status(.conflicted)) == "Conflicted")
        #expect(TxPresentation.chipIsProblem(status(.abandoned)))
        #expect(TxPresentation.trailingStatus(status(.immature)) == "Locked")
        #expect(TxPresentation.icon(type: .recvWithAddress, status: status(.confirmed), amount: Amount(duffs: 1)) == .received)
        #expect(TxPresentation.icon(type: .sendToAddress, status: status(.abandoned), amount: Amount(duffs: -1)) == .error)
        #expect(TxPresentation.icon(type: .coinJoinMixing, status: status(.confirmed), amount: .zero) == .mixing)
    }

    @Test("day headers and grouping")
    func days() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try #require(TimeZone(identifier: "UTC"))
        calendar.locale = Locale(identifier: "en_US")
        let now = try #require(calendar.date(from: DateComponents(year: 2026, month: 10, day: 6, hour: 12)))
        let yesterday = now.addingTimeInterval(-86_400)
        let earlier = now.addingTimeInterval(-2 * 86_400)
        #expect(HistoryDay.title(now, calendar: calendar, now: now) == "Today")
        #expect(HistoryDay.title(yesterday, calendar: calendar, now: now) == "Yesterday")
        #expect(HistoryDay.title(earlier, calendar: calendar, now: now).contains("2026"))
        let groups = HistoryDay.group([now, earlier, now.addingTimeInterval(-60), yesterday], calendar: calendar) { $0 }
        #expect(groups.count == 3)
        #expect(groups.first?.items.count == 2)
        #expect(groups.last?.day == calendar.startOfDay(for: earlier))
    }

    @Test("addresses shorten to 12…12 above 24 characters")
    func addresses() {
        #expect(AddressText.shortened("yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98") == "yPgfYhP6PwdZ…27nL6kLpvh98")
        #expect(AddressText.shortened("yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98").count == 25)
        #expect(AddressText.shortened("short") == "short")
    }
}
