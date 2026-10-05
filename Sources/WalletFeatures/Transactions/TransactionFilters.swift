// dash-qt transaction filter presets and CSV export (QT-089, QT-093).
import Foundation
import WalletRuntime

extension TypeFilterPreset {
    /// The record types an entry matches; empty means any type.
    public var types: Set<TxType> {
        switch self {
        case .all: []
        case .mostCommon: Set(TxType.allCases).subtracting(HomeViewModel.hiddenRecentTypes)
        case .receivedWith: [.recvWithAddress, .recvFromOther]
        case .sentTo: [.sendToAddress, .sendToOther]
        case .coinJoinSend: [.coinJoinSend]
        case .coinJoinMakeCollaterals: [.coinJoinMakeCollaterals]
        case .coinJoinCreateDenominations: [.coinJoinCreateDenominations]
        case .coinJoinMixing: [.coinJoinMixing]
        case .coinJoinCollateralPayment: [.coinJoinCollateralPayment]
        case .toYourself: [.sendToSelf]
        case .mined: [.generated]
        case .masternode: [.masternodeRegistration, .masternodeUpdate]
        case .platformTransfer: [.platformTransfer]
        case .assetLock: [.assetLock]
        case .dataTransaction: [.dataTransaction]
        case .dustReceive: [.dustReceive]
        case .other: [.other]
        }
    }

    /// Menu entries; the CoinJoin ones only while CoinJoin is enabled.
    public static func menu(coinJoinEnabled: Bool) -> [TypeFilterPreset] {
        allCases.filter { coinJoinEnabled || !$0.isCoinJoin }
    }
}

extension DateFilterPreset {
    /// The `[from, until)` range dash-qt applies for this entry at `now`.
    /// Weeks start on Monday as in dash-qt. `range` uses the given bounds.
    public func bounds(
        now: Date, calendar: Calendar, rangeFrom: Date? = nil, rangeUntil: Date? = nil
    ) -> (from: Date?, until: Date?) {
        let today = calendar.startOfDay(for: now)
        switch self {
        case .all:
            return (nil, nil)
        case .today:
            return (today, nil)
        case .thisWeek:
            // Calendar weekday: 1 = Sunday … 7 = Saturday; Monday is 2.
            let weekday = calendar.component(.weekday, from: today)
            let daysSinceMonday = (weekday + 5) % 7
            return (calendar.date(byAdding: .day, value: -daysSinceMonday, to: today), nil)
        case .thisMonth:
            return (calendar.date(from: calendar.dateComponents([.year, .month], from: today)), nil)
        case .lastMonth:
            let thisMonth = calendar.date(from: calendar.dateComponents([.year, .month], from: today))
            let lastMonth = thisMonth.flatMap { calendar.date(byAdding: .month, value: -1, to: $0) }
            return (lastMonth, thisMonth)
        case .thisYear:
            return (calendar.date(from: calendar.dateComponents([.year], from: today)), nil)
        case .range:
            return (rangeFrom, rangeUntil)
        }
    }
}

/// dash-qt's transaction CSV (`CSVModelWriter`, QT-093): every field quoted
/// with embedded quotes doubled, `,` between fields, `\n` after every line.
public enum TransactionCSV {
    public static func header(amountUnitName: String, watchOnlyColumn: Bool) -> [String] {
        var columns = ["Confirmed"]
        if watchOnlyColumn { columns.append("Watch-only") }
        columns += ["Date", "Type", "Label", "Address", "Amount (\(amountUnitName))", "ID"]
        return columns
    }

    /// One row per record. Dates are ISO `yyyy-MM-ddTHH:mm:ss` in `timeZone`;
    /// amounts are signed, without separators, in `unit`.
    public static func make(
        records: [TxRecord], unit: DisplayUnit, amounts: any AmountFormatting, watchOnlyColumn: Bool,
        timeZone: TimeZone
    ) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        var out = line(header(amountUnitName: amounts.unitName(unit), watchOnlyColumn: watchOnlyColumn))
        for record in records {
            var fields = [record.countsTowardBalance ? "true" : "false"]
            if watchOnlyColumn { fields.append(record.involvesWatchOnly ? "true" : "false") }
            fields += [
                record.date.map(formatter.string(from:)) ?? "",
                L10n.Transactions.typeName(record.type),
                record.label ?? "",
                record.address ?? "",
                amounts.format(record.amount, unit: unit, style: .plain(plusSign: false, separators: .never)),
                record.id.txid,
            ]
            out += line(fields)
        }
        return out
    }

    private static func line(_ fields: [String]) -> String {
        fields.map { "\"" + $0.replacingOccurrences(of: "\"", with: "\"\"") + "\"" }.joined(separator: ",") + "\n"
    }
}
