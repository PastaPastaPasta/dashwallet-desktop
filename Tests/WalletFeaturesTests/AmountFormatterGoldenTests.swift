// Checks the Swift AmountFormatter against every vector in
// testdata/amount_format.json (generated from dash-qt's BitcoinUnits and
// GUIUtil::formatAmount by testdata/oracle/qt), the same file dw-units is
// tested against (QT-020, QT-036, QT-039, QT-152).
import Foundation
import Testing
import WalletFeatures
import WalletRuntime

struct AmountVectors {
    let doc: [String: Any]

    static let shared: AmountVectors = {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // WalletFeaturesTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // repo root
            .appendingPathComponent("testdata/amount_format.json")
        let data = try! Data(contentsOf: url)
        return AmountVectors(doc: try! JSONSerialization.jsonObject(with: data) as! [String: Any])
    }()

    func cases(_ key: String) -> [[String: Any]] {
        doc[key] as? [[String: Any]] ?? []
    }

    static func unit(_ c: [String: Any]) -> DisplayUnit {
        switch c["unit"] as? String {
        case "DASH": .dash
        case "mDASH": .milliDash
        case "uDASH": .microDash
        case "duffs": .duffs
        default: fatalError("unknown unit \(String(describing: c["unit"]))")
        }
    }

    static func separators(_ c: [String: Any]) -> AmountSeparators {
        switch c["separators"] as? String {
        case "never": .never
        case "standard": .standard
        case "always": .always
        default: fatalError("unknown separators")
        }
    }

    static func network(_ c: [String: Any]) -> DashNetwork {
        c["network"] as? String == "main" ? .mainnet : .testnet
    }

    /// Integers above 2^53 are written as strings by the oracle.
    static func int(_ c: [String: Any], _ key: String) -> Int64 {
        if let text = c[key] as? String { return Int64(text)! }
        return (c[key] as! NSNumber).int64Value
    }

    static func bool(_ c: [String: Any], _ key: String) -> Bool { (c[key] as! NSNumber).boolValue }
}

@Suite("Amount formatting golden vectors (QT-020, QT-036, QT-039)")
struct AmountFormatterGoldenTests {
    let vectors = AmountVectors.shared

    @Test func vectorFileIsComplete() {
        for key in ["format", "format_with_unit", "floor_with_unit", "format_with_privacy", "format_amount", "parse"] {
            #expect(!vectors.cases(key).isEmpty, "missing \(key)")
        }
        #expect(vectors.doc["thin_space"] as? String == String(AmountFormatter.thinSpace))
    }

    @Test func formatMatchesDashQt() {
        for c in vectors.cases("format") {
            let got = AmountFormatter.plain(
                AmountVectors.int(c, "amount"), unit: AmountVectors.unit(c), plusSign: AmountVectors.bool(c, "plus"),
                separators: AmountVectors.separators(c), justify: AmountVectors.bool(c, "justify"))
            #expect(got == c["out"] as? String, "\(c)")
        }
    }

    @Test func formatWithUnitMatchesDashQt() {
        for c in vectors.cases("format_with_unit") {
            let formatter = AmountFormatter(network: AmountVectors.network(c))
            let got = formatter.format(
                Amount(duffs: AmountVectors.int(c, "amount")), unit: AmountVectors.unit(c),
                style: .withUnit(plusSign: AmountVectors.bool(c, "plus"), separators: AmountVectors.separators(c)))
            #expect(got == c["out"] as? String, "\(c)")
        }
    }

    @Test func floorWithUnitMatchesDashQt() {
        for c in vectors.cases("floor_with_unit") {
            let formatter = AmountFormatter(network: AmountVectors.network(c))
            let got = formatter.format(
                Amount(duffs: AmountVectors.int(c, "amount")), unit: AmountVectors.unit(c),
                style: .floored(
                    plusSign: AmountVectors.bool(c, "plus"), separators: AmountVectors.separators(c),
                    digits: Int(AmountVectors.int(c, "digits"))))
            #expect(got == c["out"] as? String, "\(c)")
        }
    }

    @Test func formatWithPrivacyMatchesDashQt() {
        for c in vectors.cases("format_with_privacy") {
            let formatter = AmountFormatter(network: AmountVectors.network(c))
            let got = formatter.format(
                Amount(duffs: AmountVectors.int(c, "amount")), unit: AmountVectors.unit(c),
                style: .privacy(separators: AmountVectors.separators(c), hidden: AmountVectors.bool(c, "privacy")))
            #expect(got == c["out"] as? String, "\(c)")
        }
    }

    @Test func guiFormatAmountMatchesDashQt() {
        for c in vectors.cases("format_amount") {
            let formatter = AmountFormatter(network: AmountVectors.network(c))
            let truncate = c["truncate"] is NSNull || c["truncate"] == nil ? nil : Int(AmountVectors.int(c, "truncate"))
            let got = formatter.format(
                Amount(duffs: AmountVectors.int(c, "amount")), unit: AmountVectors.unit(c),
                style: .gui(signed: AmountVectors.bool(c, "signed"), truncate: truncate))
            #expect(got == c["out"] as? String, "\(c)")
        }
    }

    @Test func parseMatchesDashQt() {
        let formatter = AmountFormatter(network: .mainnet)
        for c in vectors.cases("parse") {
            let input = c["input"] as! String
            let unit = AmountVectors.unit(c)
            let parsed = try? formatter.parse(input, unit: unit)
            if AmountVectors.bool(c, "ok") {
                #expect(parsed?.duffs == AmountVectors.int(c, "value"), "\(c)")
            } else {
                #expect(parsed == nil, "\(c)")
            }
        }
    }

    @Test func unitNamesFollowNetwork() {
        #expect(AmountFormatter(network: .mainnet).unitName(.dash) == "DASH")
        #expect(AmountFormatter(network: .testnet).unitName(.dash) == "tDASH")
        #expect(AmountFormatter(network: .regtest).unitName(.microDash) == "\u{3bc}tDASH")
        #expect(AmountFormatter(network: .devnet(name: "x")).unitName(.duffs) == "tduffs")
    }
}
