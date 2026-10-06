// Glue between the view models' formatted strings and the DashUIMac amount
// and transaction components (UX-SPEC §5).
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures
import WalletRuntime

extension AmountUnitDisplay {
    /// The Dash glyph for DASH and tDASH, the unit name otherwise.
    init(parts: AmountParts) {
        if let unit = parts.unit {
            self = parts.isDash ? .glyph(spoken: unit) : .name(unit)
        } else {
            self = .none
        }
    }

    /// The glyph for the DASH unit, the name for mDASH, μDASH and duffs.
    init(unit: DisplayUnit, name: String) {
        self = unit == .dash ? .glyph(spoken: name) : .name(name)
    }
}

extension AmountText {
    /// Draws a view-model string such as "+0.20000000 tDASH" or "[1.5 DASH]" with the glyph.
    init(
        formatted: String, size: Double = DesignTokens.DashTextStyle.footnote.size, weight: Font.Weight = .medium,
        glyphFactor: Double = 1, help: String? = nil
    ) {
        let parts = AmountParts.split(formatted)
        let number = parts.bracketed ? "[\(parts.number)]" : parts.number
        self.init(
            number, unit: AmountUnitDisplay(parts: parts), size: size, weight: weight, glyphFactor: glyphFactor,
            help: help ?? formatted)
    }
}

extension TxIconKind {
    var icon: DashIconSource {
        switch self {
        case .received: .token(.txReceived)
        case .sent: .token(.txSent)
        case .internalTransfer: .token(.txInternalTransfer)
        case .mixing: .token(.txMixing)
        case .mining: .token(.txMining)
        case .error: .token(.txError)
        }
    }
}

extension DashTransactionRow {
    /// A list-mode row for a history record: label-or-type title, local time and one chip, and the
    /// exact amount in the compact style (never rounded).
    init(
        record: TxRecord, unit: DisplayUnit, unitName: String, isSelected: Bool = false, action: (() -> Void)? = nil
    ) {
        self.init(
            type: record.type, category: record.category, status: record.status, date: record.date,
            amount: record.amount, label: record.label, countsTowardBalance: record.countsTowardBalance, unit: unit,
            unitName: unitName, isSelected: isSelected, action: action)
    }

    /// The same row for an Overview recent transaction.
    init(recent: RecentTransaction, unit: DisplayUnit, unitName: String, action: (() -> Void)? = nil) {
        self.init(
            type: recent.type, category: recent.category, status: recent.status, date: recent.date,
            amount: recent.amount, label: recent.label, countsTowardBalance: recent.countsTowardBalance, unit: unit,
            unitName: unitName, isSelected: false, action: action)
    }

    private init(
        type: TxType, category: TxCategory, status: TxStatus, date: Date?, amount: Amount, label: String?,
        countsTowardBalance: Bool, unit: DisplayUnit, unitName: String, isSelected: Bool, action: (() -> Void)?
    ) {
        let compact = CompactAmount.format(amount, unit: unit, signed: !TxPresentation.isInternal(type))
        let full = AmountFormatter.plain(amount.duffs, unit: unit, plusSign: true, separators: .always, justify: false)
        self.init(
            icon: TxPresentation.icon(type: type, status: status, amount: amount).icon,
            title: TxPresentation.title(type: type, category: category, amount: amount, label: label),
            subtitle: date.map { HistoryDay.time($0) },
            chip: TxPresentation.chip(status).map { Chip(text: $0, isProblem: TxPresentation.chipIsProblem(status)) },
            amount: countsTowardBalance ? compact : "[\(compact)]",
            unit: AmountUnitDisplay(unit: unit, name: unitName),
            amountHelp: "\(full) \(unitName)",
            trailingStatus: TxPresentation.trailingStatus(status),
            isInternal: TxPresentation.isInternal(type),
            isDimmed: TxPresentation.chipIsProblem(status),
            isSelected: isSelected,
            action: action)
    }
}
#endif
