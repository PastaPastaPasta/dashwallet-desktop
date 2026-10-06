// Pure presentation helpers shared by MacUI and CrossUI (UX-SPEC §5): the
// compact amount style, the split of a formatted amount into sign, number
// and unit, list-mode transaction titles and status chips, day headers and
// shortened addresses. Foundation only; no view model state.
import Foundation
import WalletRuntime

/// A formatted amount split for display: the view draws `number` and then the
/// Dash glyph (`unit == nil` and `isDash`) or the unit name.
public struct AmountParts: Sendable, Hashable {
    /// The number with its sign, as the formatter wrote it ("+0.20", "###.##", "30 391.69").
    public let number: String
    /// The unit name the formatter appended ("tDASH", "mDASH"), or nil when it wrote none.
    public let unit: String?
    /// `[…]`: dash-qt's brackets around an amount that does not count toward the balance.
    public let bracketed: Bool

    public init(number: String, unit: String?, bracketed: Bool = false) {
        self.number = number
        self.unit = unit
        self.bracketed = bracketed
    }

    /// The unit is DASH or tDASH, which draw as the Dash glyph (UX-SPEC §5.1). mDASH, μDASH and
    /// duffs have no glyph and keep their name.
    public var isDash: Bool { unit == "DASH" || unit == "tDASH" }

    /// Splits "+0.20000000 tDASH" or "[1.5 DASH]" at the last space.
    public static func split(_ formatted: String) -> AmountParts {
        var text = formatted.trimmingCharacters(in: .whitespaces)
        var bracketed = false
        if text.hasPrefix("["), text.hasSuffix("]") {
            bracketed = true
            text = String(text.dropFirst().dropLast())
        }
        guard let space = text.lastIndex(of: " ") else {
            return AmountParts(number: text, unit: nil, bracketed: bracketed)
        }
        let unit = String(text[text.index(after: space)...])
        // A unit name has a letter; "30 391.69" split at a grouping space is still a number.
        guard unit.contains(where: \.isLetter) else { return AmountParts(number: text, unit: nil, bracketed: bracketed) }
        return AmountParts(
            number: String(text[..<space]).trimmingCharacters(in: .whitespaces), unit: unit, bracketed: bracketed)
    }
}

/// UX-SPEC §5.2 "compact": the exact value with trailing zeros removed down to
/// a minimum of two decimals. It never rounds, so every significant duff stays.
public enum CompactAmount {
    /// `duffs` in `unit`, dash-qt grouping (thin spaces when the integer part
    /// has more than four digits), "." decimal mark, and a sign as asked.
    public static func format(_ amount: Amount, unit: DisplayUnit, signed: Bool) -> String {
        trim(AmountFormatter.plain(amount.duffs, unit: unit, plusSign: signed, separators: .standard, justify: false))
    }

    /// Removes trailing zeros after the decimal mark, keeping at least two decimals.
    public static func trim(_ number: String) -> String {
        guard let dot = number.firstIndex(of: ".") else { return number }
        var text = number
        let minimumEnd = text.index(dot, offsetBy: 3, limitedBy: text.endIndex) ?? text.endIndex
        while text.endIndex > minimumEnd, text.last == "0" {
            text.removeLast()
        }
        return text
    }
}

/// The leading icon of a transaction row (DashUIKit transaction icons).
public enum TxIconKind: Sendable, Hashable {
    case received, sent, internalTransfer, mixing, mining, error
}

/// List-mode transaction titles and chips (UX-SPEC §5.4, §4.9).
public enum TxPresentation {
    /// Label first, then the type title. Never the raw address: it belongs to
    /// the detail sheet and table mode.
    public static func title(type: TxType, category: TxCategory? = nil, amount: Amount, label: String?) -> String {
        if let label, !label.isEmpty { return label }
        return typeTitle(type, category: category, amount: amount)
    }

    public static func typeTitle(_ type: TxType, category: TxCategory? = nil, amount: Amount) -> String {
        if category == .reward, type != .generated { return L10n.TxTitle.masternodeReward }
        return switch type {
        case .recvWithAddress, .recvFromOther, .dustReceive: L10n.TxTitle.received
        case .sendToAddress, .sendToOther: L10n.TxTitle.sent
        case .sendToSelf: L10n.TxTitle.sentToYourself
        case .recvWithCoinJoin, .coinJoinMixing, .coinJoinCollateralPayment, .coinJoinMakeCollaterals,
            .coinJoinCreateDenominations:
            L10n.TxTitle.mixing
        case .coinJoinSend: L10n.TxTitle.coinJoinSend
        case .generated: L10n.TxTitle.mined
        case .masternodeRegistration, .masternodeUpdate: L10n.TxTitle.providerTransaction
        case .assetLock: L10n.TxTitle.assetLock
        case .platformTransfer: L10n.TxTitle.internalTransfer
        case .dataTransaction, .other: amount.duffs >= 0 ? L10n.TxTitle.received : L10n.TxTitle.sent
        }
    }

    public static func icon(type: TxType, status: TxStatus, amount: Amount) -> TxIconKind {
        switch status.kind {
        case .conflicted, .abandoned, .notAccepted: return .error
        default: break
        }
        switch type {
        case .generated: return .mining
        case .sendToSelf, .platformTransfer: return .internalTransfer
        case .recvWithCoinJoin, .coinJoinMixing, .coinJoinCollateralPayment, .coinJoinMakeCollaterals,
            .coinJoinCreateDenominations:
            return .mixing
        default: return amount.duffs >= 0 ? .received : .sent
        }
    }

    /// One status chip, or nil when the transaction is settled ("quiet when fine").
    public static func chip(_ status: TxStatus) -> String? {
        switch status.kind {
        case .conflicted: return L10n.TxTitle.chipConflicted
        case .abandoned: return L10n.TxTitle.chipAbandoned
        case .notAccepted: return L10n.TxTitle.chipNotAccepted
        case .immature, .confirmed: return nil
        case .unconfirmed:
            return status.instantLocked ? L10n.TxTitle.chipInstantSend : L10n.TxTitle.chipPending
        case .confirming:
            if status.chainLocked { return nil }
            return status.instantLocked ? L10n.TxTitle.chipInstantSend : nil
        }
    }

    /// The chip marks a failure (drawn in the danger tone).
    public static func chipIsProblem(_ status: TxStatus) -> Bool {
        [.conflicted, .abandoned, .notAccepted].contains(status.kind)
    }

    /// Orange trailing status before the amount: an immature coinbase (iOS "Locked").
    public static func trailingStatus(_ status: TxStatus) -> String? {
        status.kind == .immature ? L10n.TxTitle.locked : nil
    }

    /// The row's amount has no direction: a payment to yourself or an internal move.
    public static func isInternal(_ type: TxType) -> Bool {
        type == .sendToSelf || type == .platformTransfer
    }
}

/// Day headers of the grouped history (UX-SPEC §5.7).
public enum HistoryDay {
    /// "Today", "Yesterday", else the long date ("October 4, 2026") in the user's locale.
    public static func title(_ day: Date, calendar: Calendar = .current, now: Date = Date()) -> String {
        if calendar.isDate(day, inSameDayAs: now) { return L10n.TxTitle.today }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(day, inSameDayAs: yesterday) {
            return L10n.TxTitle.yesterday
        }
        return day.formatted(Date.FormatStyle(date: .long, time: .omitted).locale(calendar.locale ?? .current))
    }

    /// The weekday on the right of a day header ("Tuesday").
    public static func weekday(_ day: Date, calendar: Calendar = .current) -> String {
        day.formatted(Date.FormatStyle().weekday(.wide).locale(calendar.locale ?? .current))
    }

    /// The row subtitle: the local time only; the date is in the day header.
    public static func time(_ date: Date, calendar: Calendar = .current) -> String {
        date.formatted(Date.FormatStyle(date: .omitted, time: .shortened).locale(calendar.locale ?? .current))
    }

    /// Groups `items` by calendar day, newest day first, keeping the order inside a day.
    /// Items without a date go last under `nil`.
    public static func group<Item>(_ items: [Item], calendar: Calendar = .current, date: (Item) -> Date?)
        -> [HistoryDayBucket<Item>]
    {
        var order: [Date?] = []
        var byDay: [Date?: [Item]] = [:]
        for item in items {
            let day = date(item).map { calendar.startOfDay(for: $0) }
            if byDay[day] == nil { order.append(day) }
            byDay[day, default: []].append(item)
        }
        let sorted = order.sorted { lhs, rhs in
            switch (lhs, rhs) {
            case (let l?, let r?): l > r
            case (.some, nil): true
            case (nil, _): false
            }
        }
        return sorted.map { HistoryDayBucket(day: $0, items: byDay[$0] ?? []) }
    }
}

/// One day of `HistoryDay.group`.
public struct HistoryDayBucket<Item>: Identifiable {
    public let day: Date?
    public let items: [Item]
    public var id: Date { day ?? .distantPast }
}

/// Addresses as fixed strings (UX-SPEC §5.5): the first and last twelve
/// characters around an ellipsis when longer than 24 characters.
public enum AddressText {
    public static func shortened(_ address: String) -> String {
        guard address.count > 24 else { return address }
        return "\(address.prefix(12))…\(address.suffix(12))"
    }
}
