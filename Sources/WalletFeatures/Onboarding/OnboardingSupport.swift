// Pure helpers for onboarding: passphrase strength, word suggestions and
// birth-height estimation.
import Foundation
import WalletRuntime

/// A rough strength meter for the encryption passphrase (QT-111). It only
/// guides the user; the vault's Argon2id cost is the protection.
public enum PassphraseStrength: Int, Sendable, Hashable, Comparable, CaseIterable {
    case none, weak, fair, good, strong

    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }

    /// Scores length and character variety: under 8 characters is weak;
    /// each extra class (lower, upper, digit, other) and each 4 characters
    /// beyond 8 raise the score.
    public static func evaluate(_ passphrase: String) -> PassphraseStrength {
        let length = passphrase.count
        guard length > 0 else { return .none }
        guard length >= 8 else { return .weak }
        var classes = 0
        if passphrase.contains(where: \.isLowercase) { classes += 1 }
        if passphrase.contains(where: \.isUppercase) { classes += 1 }
        if passphrase.contains(where: \.isNumber) { classes += 1 }
        if passphrase.contains(where: { !$0.isLetter && !$0.isNumber }) { classes += 1 }
        let score = classes + min(3, (length - 8) / 4)
        switch score {
        case ...1: return .weak
        case 2...3: return .fair
        case 4: return .good
        default: return .strong
        }
    }

    public var title: String {
        switch self {
        case .none: ""
        case .weak: L10n.PassphraseStrength.weak
        case .fair: L10n.PassphraseStrength.fair
        case .good: L10n.PassphraseStrength.good
        case .strong: L10n.PassphraseStrength.strong
        }
    }
}

public enum MnemonicWords {
    /// Word counts a recovery phrase may have (IOS-007, QT-104).
    public static let restoreCounts: Set<Int> = [12, 15, 18, 21, 24]
    /// Word counts offered when creating a wallet (IOS-002).
    public static let createCounts = [12, 24]

    /// English BIP39 words starting with `prefix` (case-insensitive), at most
    /// `limit`. Empty for an empty prefix or an exact single match.
    public static func suggestions(for prefix: String, limit: Int = 5) -> [String] {
        let lower = prefix.lowercased()
        guard !lower.isEmpty else { return [] }
        let matches = BIP39EnglishWords.all.lazy.filter { $0.hasPrefix(lower) }.prefix(limit)
        let result = Array(matches)
        return result == [lower] ? [] : result
    }

    /// Lowercases and joins the words with single spaces.
    public static func normalize(_ text: String) -> String {
        text.lowercased().split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }
}

/// Converts a wallet creation date into a conservative scan start height.
///
/// The estimate assumes 200 s per block (the target is 157.5 s), so it lands
/// below the real height, and then subtracts one more week of blocks. Starting
/// early only costs scan time; starting late would miss transactions. Devnet
/// and regtest always start at 0.
public enum BirthHeightEstimator {
    static let secondsPerBlock: TimeInterval = 200
    static let safetyMargin: TimeInterval = 7 * 24 * 3600
    /// Genesis block times (Dash Core chainparams).
    static let mainnetGenesis = Date(timeIntervalSince1970: 1_390_095_618)
    static let testnetGenesis = Date(timeIntervalSince1970: 1_390_666_206)

    public static func height(for date: Date, network: DashNetwork) -> UInt32 {
        let genesis: Date
        switch network {
        case .mainnet: genesis = mainnetGenesis
        case .testnet: genesis = testnetGenesis
        case .devnet, .regtest: return 0
        }
        let elapsed = date.timeIntervalSince(genesis) - safetyMargin
        guard elapsed > 0 else { return 0 }
        return UInt32(min(Double(UInt32.max), (elapsed / secondsPerBlock).rounded(.down)))
    }
}

/// Type-erased generator so a seeded one can be injected in tests.
struct AnyRandomNumberGenerator: RandomNumberGenerator {
    var base: any RandomNumberGenerator

    mutating func next() -> UInt64 { base.next() }
}
