// How the Cross screens present runtime values (UX-SPEC §5): amounts as
// number + unit for `AmountText`, transaction titles, icons and status chips
// for list mode, and the locale's dates.
import DashUICross
import DesignTokens
import Foundation
import WalletFeatures
import WalletRuntime

/// Amount text for `AmountText`: the number without the unit, and how the
/// unit is shown (the Dash glyph for DASH/tDASH, the name otherwise).
@MainActor
struct AmountPresenter {
    let amounts: any AmountFormatting
    let unit: DisplayUnit

    init(env: AppEnvironment) {
        amounts = env.amounts
        unit = env.settings.display.unit
    }

    var unitDisplay: AmountUnitDisplay {
        unit == .dash ? .glyph : .name(amounts.unitName(unit))
    }

    /// The unit name, for accessible names and tooltips ("0.25 tDASH").
    var unitName: String { amounts.unitName(unit) }

    /// Rows, toasts: exact value, trailing zeros trimmed (§5.2 `.compact`),
    /// signed when `signed`.
    func compact(_ amount: Amount, signed: Bool = true) -> String {
        AmountTextRules.compact(amounts.format(amount, unit: unit, style: .plain(plusSign: signed, separators: .always)))
    }

    /// Details, confirmations: every decimal of the unit.
    func full(_ amount: Amount, signed: Bool = false) -> String {
        amounts.format(amount, unit: unit, style: .plain(plusSign: signed, separators: .always))
    }

    /// A view model's formatted text with its trailing " unit" removed (the
    /// hero and breakdown take the VM's floored or masked text as it is).
    func number(fromFormatted text: String) -> String {
        let suffix = " " + unitName
        return text.hasSuffix(suffix) ? String(text.dropLast(suffix.count)) : text
    }
}

/// List-mode presentation of a history record (UX-SPEC §5.4): label, else a
/// type title; never the raw address. dash-qt's "Received with" + address
/// phrasing stays in the details and the accessible row names.
enum TxPresentation {
    static func title(_ record: TxRecord) -> String {
        if let label = record.label, !label.isEmpty { return label }
        return typeTitle(record)
    }

    static func typeTitle(_ record: TxRecord) -> String {
        switch record.type {
        case .generated: record.category == .masternode ? CrossStrings.TxTitle.masternodeReward : CrossStrings.TxTitle.mined
        case .sendToAddress, .sendToOther, .coinJoinSend: CrossStrings.TxTitle.sent
        case .recvWithAddress, .recvFromOther, .recvWithCoinJoin, .dustReceive: CrossStrings.TxTitle.received
        case .sendToSelf: CrossStrings.TxTitle.sentToYourself
        case .coinJoinMixing, .coinJoinCollateralPayment, .coinJoinMakeCollaterals, .coinJoinCreateDenominations:
            CrossStrings.TxTitle.mixing
        case .masternodeRegistration, .masternodeUpdate: CrossStrings.TxTitle.providerTransaction
        case .assetLock, .platformTransfer: CrossStrings.TxTitle.assetLock
        case .other, .dataTransaction:
            record.category == .internalTransfer
                ? CrossStrings.TxTitle.internalTransfer
                : (record.amount.duffs < 0 ? CrossStrings.TxTitle.sent : CrossStrings.TxTitle.received)
        }
    }

    static func direction(_ record: TxRecord) -> TransactionDirection {
        switch record.status.kind {
        case .conflicted, .abandoned, .notAccepted: return .failed
        default: break
        }
        switch record.type {
        case .generated: return .mined
        case .sendToSelf: return .internalTransfer
        case .coinJoinMixing, .coinJoinCollateralPayment, .coinJoinMakeCollaterals, .coinJoinCreateDenominations:
            return .mixing
        default:
            if record.category == .internalTransfer { return .internalTransfer }
            return record.amount.duffs < 0 ? .outgoing : .incoming
        }
    }

    /// One chip for the status; confirmed and ChainLocked records have none
    /// (quiet when fine).
    static func chip(_ status: TxStatus) -> TransactionChip? {
        switch status.kind {
        case .abandoned: TransactionChip(CrossStrings.TxChip.abandoned, tone: .danger)
        case .conflicted: TransactionChip(CrossStrings.TxChip.conflicted, tone: .danger)
        case .notAccepted: TransactionChip(CrossStrings.TxChip.notAccepted, tone: .danger)
        case .unconfirmed, .confirming:
            status.instantLocked
                ? TransactionChip(CrossStrings.TxChip.instantSend) : TransactionChip(CrossStrings.TxChip.pending)
        case .immature, .confirmed: nil
        }
    }

    /// iOS's orange trailing "Locked" for immature coinbase outputs.
    static func trailingStatus(_ status: TxStatus) -> String? {
        status.kind == .immature ? CrossStrings.TxChip.locked : nil
    }

    static func dimmed(_ status: TxStatus) -> Bool {
        status.kind == .abandoned || status.kind == .conflicted || status.kind == .notAccepted
    }
}

extension Format {
    /// "Oct 6, 2026 at 1:39 AM" in the user's locale (UX-SPEC §5.7).
    static let localDateTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        return formatter
    }()

    /// Row time ("01:40" / "1:40 AM"); the date lives in the day header.
    static let localTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateStyle = .none
        formatter.timeStyle = .short
        return formatter
    }()

    /// "Today", "Yesterday", else the long date.
    static let dayTitle: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateStyle = .long
        formatter.timeStyle = .none
        formatter.doesRelativeDateFormatting = true
        return formatter
    }()

    static let weekdayName: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.setLocalizedDateFormatFromTemplate("EEEE")
        return formatter
    }()

    static func time(_ date: Date?) -> String {
        date.map { localTime.string(from: $0) } ?? ""
    }

    static func weekday(_ date: Date?) -> String? {
        date.map { weekdayName.string(from: $0) }
    }
}
