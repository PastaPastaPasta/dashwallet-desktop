// Small pieces the M3 view models share: asking the authentication gate for
// a grant with an optional passphrase, display-unit amount text, and the
// "Not available yet" rule for `not_implemented` answers.
import Foundation
import WalletRuntime

/// Issues grants for the M3 flows. An encrypted vault needs the passphrase
/// (or the gate's other credential); without one the flow asks for it.
@MainActor
struct GrantRequester {
    let auth: any AuthenticationGating
    let vault: any VaultProviding

    /// Whether `purpose` needs a passphrase from the user right now.
    func needsPassphrase(_ purpose: GrantPurpose) -> Bool {
        auth.requirement(for: purpose) != .none
    }

    /// A grant for `purpose` on `wallet`, or `nil` when a passphrase is
    /// needed and `passphrase` is empty.
    func authorize(_ purpose: GrantPurpose, wallet: WalletID, passphrase: String?) async throws(ServiceError)
        -> AuthGrant?
    {
        switch auth.requirement(for: purpose) {
        case .none:
            return try await auth.authorize(purpose, wallet: wallet, credential: .unencrypted)
        case .passphrase, .quickUnlockOrPassphrase:
            guard let passphrase, !passphrase.isEmpty else { return nil }
            return try await auth.authorize(
                purpose, wallet: wallet, credential: .passphrase(vault.makeSecret(utf8: passphrase)))
        }
    }
}

/// Amounts in the display unit (dash-qt `formatWithUnit`).
@MainActor
struct AmountText {
    let amounts: any AmountFormatting
    let settings: any SettingsProviding

    func callAsFunction(_ amount: Amount) -> String {
        amounts.format(amount, unit: settings.display.unit, style: .withUnit(plusSign: false, separators: .standard))
    }

    /// Without decimals (dash-qt strips them from the CoinJoin target).
    func whole(_ amount: Amount) -> String {
        let unit = settings.display.unit
        return amounts.format(amount, unit: unit, style: .gui(signed: false, truncate: 0)) + " "
            + amounts.unitName(unit)
    }

    /// Hidden in discreet mode (dash-qt `#` placeholders).
    func privacy(_ amount: Amount) -> String {
        amounts.format(
            amount, unit: settings.display.unit,
            style: .privacy(separators: .standard, hidden: settings.display.hideBalances))
    }
}

extension ServiceError {
    /// The engine stub (or a missing adapter) answered: the feature is shown
    /// as "Not available yet", not as an error.
    var isNotImplemented: Bool { code == .notImplemented }
}

/// dash-qt's date format for tables (`GUIUtil::dateTimeStr`).
enum M3Dates {
    static func dateTime(_ date: Date, timing: Timing) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timing.timeZone
        formatter.calendar = timing.calendar
        formatter.dateFormat = "yyyy-MM-dd HH:mm"
        return formatter.string(from: date)
    }

    static func date(_ date: Date, timing: Timing) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timing.timeZone
        formatter.calendar = timing.calendar
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: date)
    }
}

/// Text checks the M3 forms run before calling the engine (the engine checks
/// again and has the final word).
enum M3Validation {
    static func isHex(_ text: String, count: Int) -> Bool {
        text.count == count && text.allSatisfy(\.isHexDigit)
    }

    /// Comma- or space-separated entries, empty ones dropped.
    static func list(_ text: String) -> [String] {
        text.split(whereSeparator: { $0 == "," || $0 == " " || $0 == "\n" || $0 == "\t" }).map(String.init)
    }

    /// `host:port` with a numeric port 1–65535 (IPv6 in brackets).
    static func isService(_ entry: String) -> Bool {
        guard let colon = entry.lastIndex(of: ":") else { return false }
        let host = entry[..<colon]
        guard let port = Int(entry[entry.index(after: colon)...]), (1...65_535).contains(port), !host.isEmpty else {
            return false
        }
        if host.hasPrefix("[") { return host.hasSuffix("]") && host.count > 2 }
        return OptionsViewModel.isIPv4(String(host)) || host.allSatisfy { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" }
    }

    /// The operator reward text "12.34" as hundredths of a percent.
    static func rewardX100(_ text: String) -> Int? {
        let trimmed = text.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "%", with: "")
        guard !trimmed.isEmpty, let value = Decimal(string: trimmed, locale: Locale(identifier: "en_US_POSIX")) else {
            return trimmed.isEmpty ? 0 : nil
        }
        let scaled = NSDecimalNumber(decimal: value * 100)
        guard scaled.doubleValue == scaled.doubleValue.rounded() else { return nil }
        let result = scaled.intValue
        return (0...10_000).contains(result) ? result : nil
    }

    static func percent(_ x100: Int) -> String {
        String(format: "%d.%02d", x100 / 100, x100 % 100)
    }
}
