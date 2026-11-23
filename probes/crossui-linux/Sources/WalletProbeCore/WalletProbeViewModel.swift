import Foundation
import Observation

/// Sidebar destinations of the probe's wallet window.
public enum SidebarItem: String, CaseIterable, Identifiable, Sendable, CustomStringConvertible {
    case overview = "Overview"
    case send = "Send"
    case receive = "Receive"
    case transactions = "Transactions"

    public var id: Self { self }
    public var description: String { rawValue }
}

/// Direction of a sample transaction relative to the wallet.
public enum TransactionDirection: String, Sendable {
    case received = "Received"
    case sent = "Sent"
}

/// One row of sample transaction data. The probe has no engine; these rows are
/// generated locally and labelled as sample data in the UI.
public struct SampleTransaction: Identifiable, Hashable, Sendable {
    public let id: Int
    public let direction: TransactionDirection
    /// Amount in duffs (1 DASH = 100,000,000 duffs). Always positive; the sign
    /// comes from `direction`.
    public let amountDuffs: Int64
    public let confirmations: Int

    /// Single-line text used for the row label (and therefore the accessible name).
    public var summary: String {
        let sign = direction == .received ? "+" : "-"
        return "\(direction.rawValue) \(sign)\(DashAmountFormatter.string(fromDuffs: amountDuffs)) DASH, "
            + "\(confirmations) confirmations, sample tx \(id)"
    }
}

/// Result of the Send form. The probe never builds or broadcasts a transaction.
public enum SendPhase: Equatable, Sendable {
    /// The user is editing the form.
    case editing
    /// The address field failed the format check.
    case invalidAddress
    /// The form was valid, but sending is not implemented in this probe.
    case notImplemented
}

/// Formats duff amounts as DASH strings with integer arithmetic only.
public enum DashAmountFormatter {
    public static let duffsPerDash: Int64 = 100_000_000

    /// Returns e.g. "1.5" for 150,000,000 duffs and "-0.00000001" for -1.
    public static func string(fromDuffs duffs: Int64) -> String {
        let negative = duffs < 0
        let magnitude = duffs.magnitude
        let whole = magnitude / UInt64(duffsPerDash)
        var fraction = String(magnitude % UInt64(duffsPerDash))
        fraction = String(repeating: "0", count: 8 - fraction.count) + fraction
        while fraction.hasSuffix("0") { fraction.removeLast() }
        let body = fraction.isEmpty ? "\(whole)" : "\(whole).\(fraction)"
        return negative ? "-" + body : body
    }
}

/// View model for the probe's wallet window. Same shape as the real WalletFeatures
/// layer: main-actor isolated, `@Observable`, Foundation + Observation only, and
/// plain value state for the views to render.
@MainActor
@Observable
public final class WalletProbeViewModel {
    public var selectedSidebarItem: SidebarItem? = .overview
    public var selectedTransactionID: Int?
    public var hideBalance = false
    public var mixedFundsOnly = false
    public var sendAddress = "" {
        didSet {
            // Any edit invalidates the previous Send result. Only assign on a real
            // change so observers of `sendPhase` are not notified for no-ops.
            if oldValue != sendAddress, sendPhase != .editing { sendPhase = .editing }
        }
    }
    public private(set) var sendPhase: SendPhase = .editing
    public let transactions: [SampleTransaction]

    public init(transactionCount: Int = 50) {
        transactions = Self.makeSampleTransactions(count: transactionCount)
    }

    /// Sum of the sample transactions, in duffs.
    public var balanceDuffs: Int64 {
        transactions.reduce(0) { total, tx in
            tx.direction == .received ? total + tx.amountDuffs : total - tx.amountDuffs
        }
    }

    /// Balance line shown on the Overview page.
    public var balanceText: String {
        hideBalance
            ? "Balance: hidden"
            : "Balance: \(DashAmountFormatter.string(fromDuffs: balanceDuffs)) DASH (sample data)"
    }

    /// Status line shown under the Send button.
    public var sendStatusText: String {
        switch sendPhase {
        case .editing:
            return "Enter a Dash address."
        case .invalidAddress:
            return "Invalid address: expected 34 Base58 characters starting with X or 7."
        case .notImplemented:
            return "Address format OK. Sending is not implemented in this probe."
        }
    }

    public var detailTitle: String { selectedSidebarItem?.rawValue ?? "Select a page" }

    /// Validates the address field. The probe has no engine, so a well-formed
    /// address ends in `.notImplemented` rather than a sent transaction.
    public func send() {
        sendPhase = Self.isPlausibleMainnetAddress(sendAddress) ? .notImplemented : .invalidAddress
    }

    /// Format-only check for a mainnet P2PKH (X…) or P2SH (7…) address: 34 Base58
    /// characters. It does not verify the Base58Check checksum.
    public nonisolated static func isPlausibleMainnetAddress(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let base58 = Set("123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz")
        guard trimmed.count == 34, let first = trimmed.first, first == "X" || first == "7" else {
            return false
        }
        return trimmed.allSatisfy { base58.contains($0) }
    }

    /// Deterministic sample rows: every third row is a send, amounts and
    /// confirmation counts follow fixed formulas.
    nonisolated static func makeSampleTransactions(count: Int) -> [SampleTransaction] {
        (0..<count).map { index in
            SampleTransaction(
                id: index + 1,
                direction: index % 3 == 2 ? .sent : .received,
                amountDuffs: Int64((index % 7) + 1) * 12_345_678,
                confirmations: (index * 37) % 1000
            )
        }
    }
}
