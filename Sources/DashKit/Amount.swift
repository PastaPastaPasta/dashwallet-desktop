import Foundation

/// A Dash amount in duffs (1 DASH = 100,000,000 duffs). Integer-only: no
/// floating point ever touches an amount.
public struct Amount: Sendable, Hashable, Comparable, Codable, CustomStringConvertible {
    public static let duffsPerDash: Int64 = 100_000_000
    /// Platform credits per duff (1 duff = 1000 credits).
    public static let creditsPerDuff: Int64 = 1_000
    public static let zero = Amount(duffs: 0)

    public let duffs: Int64

    public init(duffs: Int64) {
        self.duffs = duffs
    }

    /// Converts an engine `u64` duff count. `nil` if it exceeds `Int64.max`,
    /// which no real balance can (total supply < 2.2e15 duffs).
    public init?(exactly duffs: UInt64) {
        guard let value = Int64(exactly: duffs) else { return nil }
        self.duffs = value
    }

    /// Platform credits rounded down to whole duffs.
    public init(creditsRoundingDown credits: Int64) {
        self.duffs = credits / Amount.creditsPerDuff
    }

    /// The amount in Platform credits, or `nil` on overflow.
    public var credits: Int64? {
        let (value, overflow) = duffs.multipliedReportingOverflow(by: Amount.creditsPerDuff)
        return overflow ? nil : value
    }

    /// Parses a decimal DASH string such as `"1.5"`, `"-0.00000001"` or `"21000000"`.
    /// At most 8 fractional digits; no exponent, grouping or whitespace.
    public init?(dashString: String) {
        var text = Substring(dashString)
        var negative = false
        if text.first == "-" {
            negative = true
            text = text.dropFirst()
        }
        let parts = text.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        let whole = parts[0]
        let fraction = parts.count == 2 ? parts[1] : ""
        guard !whole.isEmpty || !fraction.isEmpty,
              fraction.count <= 8,
              whole.allSatisfy(\.isASCIIDigit),
              fraction.allSatisfy(\.isASCIIDigit),
              !(parts.count == 2 && fraction.isEmpty)
        else { return nil }
        let wholeValue = whole.isEmpty ? 0 : Int64(whole)
        let paddedFraction = fraction + String(repeating: "0", count: 8 - fraction.count)
        guard let wholeValue, let fractionValue = Int64(paddedFraction) else { return nil }
        let (scaled, overflowMul) = wholeValue.multipliedReportingOverflow(by: Amount.duffsPerDash)
        let (total, overflowAdd) = scaled.addingReportingOverflow(fractionValue)
        guard !overflowMul, !overflowAdd else { return nil }
        self.duffs = negative ? -total : total
    }

    /// Plain decimal DASH with exactly `fractionDigits` (0...8) digits after
    /// the point, truncating (never rounding up) extra precision. Locale-free;
    /// user-facing formatting is WalletFeatures' AmountFormatter.
    public func dashString(fractionDigits: Int = 8) -> String {
        precondition((0...8).contains(fractionDigits), "fractionDigits must be 0...8")
        let magnitude = duffs.magnitude
        let unit = UInt64(Amount.duffsPerDash)
        let whole = magnitude / unit
        let fraction = magnitude % unit
        let sign = duffs < 0 ? "-" : ""
        guard fractionDigits > 0 else { return "\(sign)\(whole)" }
        let padded = String(fraction).leftPadded(to: 8)
        return "\(sign)\(whole).\(padded.prefix(fractionDigits))"
    }

    public var description: String { "\(dashString()) DASH" }

    public static func < (lhs: Amount, rhs: Amount) -> Bool { lhs.duffs < rhs.duffs }

    /// Checked addition; `nil` on overflow.
    public func adding(_ other: Amount) -> Amount? {
        let (value, overflow) = duffs.addingReportingOverflow(other.duffs)
        return overflow ? nil : Amount(duffs: value)
    }

    /// Checked subtraction; `nil` on overflow.
    public func subtracting(_ other: Amount) -> Amount? {
        let (value, overflow) = duffs.subtractingReportingOverflow(other.duffs)
        return overflow ? nil : Amount(duffs: value)
    }
}

private extension Character {
    var isASCIIDigit: Bool { isASCII && isNumber }
}

private extension String {
    func leftPadded(to width: Int) -> String {
        count >= width ? self : String(repeating: "0", count: width - count) + self
    }
}
