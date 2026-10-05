// Address and amount input rules shared by Send, Receive, Address book and
// Sign/Verify (QT-053, QT-055, QT-056, QT-067).
import Foundation
import WalletRuntime

/// Why typed address text cannot be used as a Core payment address.
public enum AddressInputProblem: Error, Sendable, Hashable {
    case empty
    case invalid
    /// A valid-looking address of another Dash network (mainnet `X`/`7`
    /// versus testnet, devnet and regtest `y`/`8`/`9`).
    case wrongNetwork
    /// DIP-18 Platform address (QT-067).
    case platformAddress
    /// Orchard address; shielded sending arrives with M4.
    case shieldedAddress
}

public enum AddressInput {
    /// Characters dash-qt strips besides whitespace: zero-width space, joiners,
    /// word joiner and BOM.
    private static let invisible: Set<Unicode.Scalar> = ["\u{200B}", "\u{200C}", "\u{200D}", "\u{2060}", "\u{FEFF}"]

    /// The text with whitespace and zero-width characters removed.
    public static func clean(_ text: String) -> String {
        String(String.UnicodeScalarView(text.unicodeScalars.filter {
            !$0.properties.isWhitespace && !invisible.contains($0)
        }))
    }

    /// Classifies cleaned text for `network`. Returns the class for Core
    /// addresses, or the problem.
    public static func checkCore(
        _ text: String, uri: any URIHandling, network: DashNetwork
    ) -> Result<AddressClass, AddressInputProblem> {
        let address = clean(text)
        guard !address.isEmpty else { return .failure(.empty) }
        let addressClass = uri.classifyAddress(address)
        switch addressClass {
        case .core:
            return .success(addressClass)
        case .platform:
            return .failure(.platformAddress)
        case .shielded:
            return .failure(.shieldedAddress)
        case .invalid(.platformAddress):
            return .failure(.platformAddress)
        case .invalid(.invalidBase58Prefix):
            return .failure(looksLikeOtherNetwork(address, network: network) ? .wrongNetwork : .invalid)
        case .invalid:
            return .failure(.invalid)
        }
    }

    private static func looksLikeOtherNetwork(_ address: String, network: DashNetwork) -> Bool {
        guard let first = address.first else { return false }
        let otherPrefixes: Set<Character> = network == .mainnet ? ["y", "8", "9"] : ["X", "7"]
        return otherPrefixes.contains(first)
    }
}

/// Why typed amount text is not a usable amount.
public enum AmountInputProblem: Error, Sendable, Hashable {
    case unparsable
    /// Zero or negative where a payment needs a positive amount.
    case notPositive
    /// Above 21 million DASH.
    case outOfRange
    /// Below the dust threshold of a P2PKH output (QT-055).
    case dust
}

public enum AmountInput {
    /// Smallest non-dust P2PKH output at dash-qt's 3000 duff/kB dust relay fee.
    public static let dustThreshold = Amount(duffs: 546)

    /// Parses amount-field text in `unit`; "," is accepted as "." (QT-056).
    /// Empty text is `nil` (no amount).
    public static func parse(
        _ text: String, unit: DisplayUnit, formatter: any AmountFormatting
    ) -> Result<Amount?, AmountInputProblem> {
        let normalized = text.replacingOccurrences(of: ",", with: ".")
            .trimmingCharacters(in: .whitespaces)
        guard !normalized.isEmpty else { return .success(nil) }
        do {
            let amount = try formatter.parse(normalized, unit: unit)
            guard amount.duffs.magnitude <= UInt64(AmountFormatter.maxMoney.duffs) else {
                return .failure(.outOfRange)
            }
            return .success(amount)
        } catch {
            return .failure(.unparsable)
        }
    }

    /// Parses a payment amount: required, positive, within range, not dust.
    public static func parsePayment(
        _ text: String, unit: DisplayUnit, formatter: any AmountFormatting
    ) -> Result<Amount, AmountInputProblem> {
        switch parse(text, unit: unit, formatter: formatter) {
        case .failure(let problem):
            return .failure(problem)
        case .success(nil):
            return .failure(.notPositive)
        case .success(let amount?):
            if amount.duffs <= 0 { return .failure(.notPositive) }
            if amount < dustThreshold { return .failure(.dust) }
            return .success(amount)
        }
    }
}
