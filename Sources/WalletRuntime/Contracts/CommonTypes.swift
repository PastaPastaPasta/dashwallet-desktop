// M1 service contracts: shared value types (docs/contracts/m1-swift.md).
//
// These types are declared in WalletRuntime, not taken from DashKit, so view
// models in WalletFeatures (which may not import DashKit) can use them. They
// shadow the same-named DashKit types inside WalletRuntime; adapter code that
// needs the DashKit type writes `DashKit.Amount`, `DashKit.WalletID`, ….
import Foundation

/// A Dash amount in duffs (1 DASH = 100,000,000 duffs). Formatting and
/// parsing go through `AmountFormatting` (Rust dw-units).
public struct Amount: Sendable, Hashable, Comparable, Codable {
    public static let duffsPerDash: Int64 = 100_000_000
    public static let zero = Amount(duffs: 0)

    public let duffs: Int64

    public init(duffs: Int64) {
        self.duffs = duffs
    }

    public static func < (lhs: Amount, rhs: Amount) -> Bool { lhs.duffs < rhs.duffs }
}

/// Network-scoped wallet id: 64 lowercase hex characters.
public struct WalletID: Sendable, Hashable, Codable, CustomStringConvertible {
    public let hex: String

    public init?(hex: String) {
        let lower = hex.lowercased()
        guard lower.count == 64, lower.allSatisfy({ $0.isASCII && $0.isHexDigit }) else { return nil }
        self.hex = lower
    }

    public var description: String { hex }
}

/// A Dash network. Each has its own data directory, wallets and vault.
public enum DashNetwork: Sendable, Hashable, Codable, CustomStringConvertible {
    case mainnet
    case testnet
    case devnet(name: String)
    case regtest

    public var description: String {
        switch self {
        case .mainnet: "mainnet"
        case .testnet: "testnet"
        case .devnet(let name): "devnet-\(name)"
        case .regtest: "regtest"
        }
    }
}

/// A transaction output reference. `txid` is 64 lowercase hex characters.
public struct OutPoint: Sendable, Hashable, Codable {
    public let txid: String
    public let vout: UInt32

    public init(txid: String, vout: UInt32) {
        self.txid = txid
        self.vout = vout
    }
}

/// Secret material held transiently (recovery phrase, passphrase). The
/// runtime's conforming type zeroes its storage on deinit (DashKit
/// `SecretBytes`). View models never copy the bytes into a `String` they keep.
public protocol SecretBuffer: AnyObject, Sendable {
    var count: Int { get }
    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R
}
