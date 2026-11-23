import DashKit
import Testing

@Suite struct AmountTests {
    @Test func parsesDecimalDashExactly() {
        #expect(Amount(dashString: "1")?.duffs == 100_000_000)
        #expect(Amount(dashString: "1.5")?.duffs == 150_000_000)
        #expect(Amount(dashString: "0.00000001")?.duffs == 1)
        #expect(Amount(dashString: ".5")?.duffs == 50_000_000)
        #expect(Amount(dashString: "-2.25")?.duffs == -225_000_000)
        #expect(Amount(dashString: "21000000")?.duffs == 2_100_000_000_000_000)
    }

    @Test(arguments: ["", "-", "1.", "1.000000001", "1e8", " 1", "1,5", "abc", "١"])
    func rejectsMalformed(_ text: String) {
        #expect(Amount(dashString: text) == nil)
    }

    @Test func rejectsOverflow() {
        #expect(Amount(dashString: "92233720368.54775808") == nil)
        #expect(Amount(dashString: "92233720368.54775807")?.duffs == .max)
    }

    @Test func formatsWithTruncation() {
        let a = Amount(duffs: 123_456_789)
        #expect(a.dashString() == "1.23456789")
        #expect(a.dashString(fractionDigits: 2) == "1.23")
        #expect(a.dashString(fractionDigits: 0) == "1")
        #expect(Amount(duffs: -1).dashString() == "-0.00000001")
        #expect(Amount(duffs: 5).description == "0.00000005 DASH")
    }

    @Test func creditsConversion() {
        #expect(Amount(duffs: 2).credits == 2_000)
        #expect(Amount(creditsRoundingDown: 2_999).duffs == 2)
        #expect(Amount(duffs: .max).credits == nil)
    }

    @Test func checkedArithmetic() {
        #expect(Amount(duffs: 1).adding(Amount(duffs: 2)) == Amount(duffs: 3))
        #expect(Amount(duffs: .max).adding(Amount(duffs: 1)) == nil)
        #expect(Amount(duffs: .min).subtracting(Amount(duffs: 1)) == nil)
        #expect(Amount(exactly: UInt64.max) == nil)
        #expect(Amount(exactly: 7)?.duffs == 7)
    }
}
