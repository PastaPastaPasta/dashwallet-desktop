// Coin Selection window (QT-068…074): dash-qt's coin control dialog with
// list and tree modes, (un)select all, (un)lock all, the CoinJoin coin
// toggle, per-coin context menu and the summary labels; plus the Send page's
// "Coin Control Features" panel that opens it. The window edits the main
// model's `coinControl`, whose selection Send reads when it builds its draft
// (review M3), so Send pays from what is ticked even while this is open.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct CoinControlWindow: View {
    let model: MacAppModel
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        if model.features != nil, let coinControl = model.main?.coinControl {
            CoinControlView(coinControl: coinControl, formatAmount: model.formatAmount, onDone: {
                dismissWindow(id: SceneID.coinControl)
            })
        } else {
            Text(model.unavailableReason ?? L10n.Options.unavailable).padding(DashSpacing.xl)
        }
    }
}

struct CoinControlView: View {
    let coinControl: CoinControlViewModel
    /// A tree group's total in the display unit.
    let formatAmount: (Amount) -> String
    let onDone: () -> Void
    @State private var tableSelection: Set<OutPoint> = []

    var body: some View {
        // UX-SPEC §4.14: the summary on a raised strip, the toolbar, the
        // technical table on a card, OK in the footer.
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            CoinSummaryGrid(coinControl: coinControl)
                .frame(maxWidth: .infinity, alignment: .leading)
                .dashCard(radius: DashRadius.standard, padding: DashSpacing.m, elevation: nil, fill: Color.role.cardRaised)
            toolbar
            if coinControl.unselectedNotice {
                HStack {
                    SystemNotice(text: L10n.CoinControl.coinsUnselected, tone: .warning)
                    Button(MacStrings.Common.ok) { coinControl.dismissUnselectedNotice() }
                        .buttonStyle(.dash(.tintedGray, .small))
                }
            }
            if let error = coinControl.errorMessage {
                SystemNotice(text: error, tone: .error)
            }
            Group {
                switch coinControl.mode {
                case .list: listMode
                case .tree: treeMode
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: DashRadius.group, style: .continuous))
            .dashCard(radius: DashRadius.group, padding: nil)
            .frame(maxHeight: .infinity)
            HStack {
                Spacer()
                Button(MacStrings.Common.ok, action: onDone)
                    .buttonStyle(.dash(.filledBlue, .medium))
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("coinControl.ok")
            }
        }
        .padding(DashLayout.pagePaddingH)
        .frame(minWidth: 860, minHeight: 520)
        .dashCanvas()
        .task { await coinControl.load() }
        .accessibilityIdentifier("coinControl")
    }

    private var toolbar: some View {
        HStack(spacing: DashSpacing.m) {
            Button(L10n.CoinControl.selectAll) { Task { await coinControl.selectAll() } }
                .buttonStyle(.dash(.tintedGray, .small))
                .accessibilityIdentifier("coinControl.selectAll")
            Button(L10n.CoinControl.lockAll) { Task { await coinControl.lockAll() } }
                .buttonStyle(.dash(.tintedGray, .small))
                .accessibilityIdentifier("coinControl.lockAll")
            Text(coinControl.lockedText)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
                .accessibilityIdentifier("coinControl.locked")
            Spacer()
            if !coinControl.coinJoinPage {
                Button(coinControl.coinJoinToggleTitle) {
                    Task { await coinControl.setShowCoinJoinCoins(!coinControl.showCoinJoinCoins) }
                }
                .buttonStyle(.dash(.plainBlue, .small))
                DashSegmentedControl(
                    [(CoinControlMode.tree, L10n.CoinControl.treeMode), (CoinControlMode.list, L10n.CoinControl.listMode)],
                    selection: Binding(get: { coinControl.mode }, set: { coinControl.setMode($0) }))
                .accessibilityIdentifier("coinControl.mode")
            }
        }
    }

    // MARK: List mode

    private var listMode: some View {
        DataTable(
            rows: coinControl.coins, columns: columns, selection: $tableSelection,
            sortOrder: Binding(
                get: { DataTableSortOrder(columnID: coinControl.sort.column.rawValue, ascending: coinControl.sort.ascending) },
                set: { order in
                    if let order, let column = CoinColumn(rawValue: order.columnID) { coinControl.sort(by: column) }
                }),
            emptyText: MacStrings.CoinControl.empty,
            onActivate: { id in Task { await coinControl.toggle(id) } },
            contextMenu: { ids in contextMenu(ids.first) })
        .accessibilityIdentifier("coinControl.list")
    }

    private var columns: [DataTableColumn<Utxo>] {
        var columns: [DataTableColumn<Utxo>] = [
            DataTableColumn("", id: "check", width: .fixed(30), alignment: .center) { coin in
                CoinCheckbox(coinControl: coinControl, coin: coin)
            },
            DataTableColumn(
                L10n.CoinControl.columnAmount, id: CoinColumn.amount.rawValue, width: .fixed(150), alignment: .trailing,
                sortBy: { $0.amount < $1.amount }
            ) { coin in
                Text(coinControl.amountText(coin)).dashFont(.footnote).monospacedDigit()
            },
            DataTableColumn(
                L10n.CoinControl.columnLabel, id: CoinColumn.label.rawValue, width: .flexible(min: 120),
                value: { coinControl.label(of: $0) }),
            DataTableColumn(
                L10n.CoinControl.columnAddress, id: CoinColumn.address.rawValue, width: .flexible(min: 200),
                value: \.address),
        ]
        if coinControl.showCoinJoinCoins || coinControl.coinJoinPage {
            columns.append(DataTableColumn(
                L10n.CoinControl.columnMixingRounds, id: CoinColumn.mixingRounds.rawValue, width: .fixed(110),
                alignment: .trailing, sortBy: { ($0.coinJoinRounds ?? 0) < ($1.coinJoinRounds ?? 0) }
            ) { coin in
                Text(coin.coinJoinRounds.map(String.init) ?? L10n.Tools.none).dashFont(.footnote)
            })
        }
        columns += [
            DataTableColumn(
                L10n.CoinControl.columnDate, id: CoinColumn.date.rawValue, width: .fixed(140),
                sortBy: { ($0.date ?? .distantPast) < ($1.date ?? .distantPast) }
            ) { coin in
                Text(coin.date?.formatted(date: .numeric, time: .shortened) ?? "").dashFont(.footnote).monospacedDigit()
            },
            DataTableColumn(
                L10n.CoinControl.columnConfirmations, id: CoinColumn.confirmations.rawValue, width: .fixed(100),
                alignment: .trailing, sortBy: { $0.confirmations < $1.confirmations }
            ) { coin in
                Text("\(coin.confirmations)").dashFont(.footnote).monospacedDigit()
            },
        ]
        return columns
    }

    // MARK: Tree mode

    private var treeMode: some View {
        List {
            ForEach(coinControl.groups) { group in
                DisclosureGroup {
                    ForEach(group.coins) { coin in
                        HStack(spacing: DashSpacing.m) {
                            CoinCheckbox(coinControl: coinControl, coin: coin)
                            Text(coinControl.amountText(coin))
                                .dashFont(.footnote)
                                .monospacedDigit()
                                .frame(width: 150, alignment: .trailing)
                            // An outpoint (txid:vout) is a hash: technical text.
                            Text("\(coin.outpoint.txid.prefix(16))…:\(coin.outpoint.vout)")
                                .font(.system(size: DesignTokens.DashTextStyle.caption1.size, design: .monospaced))
                                .foregroundStyle(Color.role.textSecondary)
                            Spacer()
                            Text(coin.date?.formatted(date: .numeric, time: .shortened) ?? "").dashFont(.footnote)
                            Text("\(coin.confirmations)").dashFont(.footnote).frame(width: 60, alignment: .trailing)
                        }
                        .contextMenu { contextMenuItems(coin.outpoint) }
                    }
                } label: {
                    HStack {
                        Text(group.label).dashFont(.subheadMedium)
                        Text(group.address)
                            .dashFont(.footnote)
                            .foregroundStyle(Color.role.textSecondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer()
                        Text(formatAmount(group.total))
                            .dashFont(.footnote)
                            .monospacedDigit()
                    }
                }
            }
        }
        .listStyle(.inset)
        .scrollContentBackground(.hidden)
        .accessibilityIdentifier("coinControl.tree")
    }

    // MARK: Context menu

    private func contextMenu(_ outpoint: OutPoint?) -> [DataTableMenuAction] {
        guard let outpoint, let coin = coinControl.allCoins.first(where: { $0.outpoint == outpoint }) else { return [] }
        return [
            DataTableMenuAction(title: L10n.CoinControl.copyAmount) { MacPasteboard.copy(coinControl.copy(.amount, of: outpoint)) },
            DataTableMenuAction(title: L10n.CoinControl.copyLabel) { MacPasteboard.copy(coinControl.copy(.label, of: outpoint)) },
            DataTableMenuAction(title: L10n.CoinControl.copyAddress) {
                MacPasteboard.copy(coinControl.copy(.address, of: outpoint))
            },
            DataTableMenuAction(title: L10n.CoinControl.copyOutpoint) {
                MacPasteboard.copy(coinControl.copy(.outpoint, of: outpoint))
            },
            DataTableMenuAction(title: L10n.CoinControl.lockUnspent, isEnabled: !coin.userLocked) {
                Task { await coinControl.lock(outpoint) }
            },
            DataTableMenuAction(title: L10n.CoinControl.unlockUnspent, isEnabled: coin.userLocked) {
                Task { await coinControl.unlock(outpoint) }
            },
        ]
    }

    @ViewBuilder
    private func contextMenuItems(_ outpoint: OutPoint) -> some View {
        ForEach(contextMenu(outpoint)) { action in
            Button(action.title, action: action.action).disabled(!action.isEnabled)
        }
    }
}

/// The selection checkbox; locked and reserved coins show a lock instead.
private struct CoinCheckbox: View {
    let coinControl: CoinControlViewModel
    let coin: Utxo

    var body: some View {
        if coinControl.isSelectable(coin) {
            Toggle("", isOn: Binding(
                get: { coinControl.selected.contains(coin.outpoint) },
                set: { _ in Task { await coinControl.toggle(coin.outpoint) } }))
            .toggleStyle(.checkbox)
            .labelsHidden()
            .accessibilityIdentifier("coinControl.check.\(coin.outpoint.vout).\(coin.outpoint.txid.prefix(8))")
        } else {
            Image(systemName: "lock.fill")
                .foregroundStyle(Color.role.textSecondary)
                .help(coin.reserved ? MacStrings.CoinControl.reserved : L10n.CoinControl.unlockUnspent)
                .accessibilityLabel(MacStrings.CoinControl.locked)
        }
    }
}

/// dash-qt's summary labels: Quantity, Bytes, Amount, Fee, After Fee,
/// Change; each copies its value from the context menu.
struct CoinSummaryGrid: View {
    let coinControl: CoinControlViewModel

    var body: some View {
        if let text = coinControl.summaryText {
            Grid(alignment: .leading, horizontalSpacing: DashSpacing.l, verticalSpacing: DashSpacing.xxxs) {
                GridRow {
                    cell(L10n.CoinControl.quantity, text.quantity, .quantity)
                    cell(L10n.CoinControl.amount, text.amount, .amount)
                    cell(L10n.CoinControl.fee, text.fee, .fee)
                }
                GridRow {
                    cell(L10n.CoinControl.bytes, text.bytes, .bytes)
                    cell(L10n.CoinControl.afterFee, text.afterFee, .afterFee)
                    cell(L10n.CoinControl.change, text.change, .change)
                }
            }
            .dashFont(.footnote)
            .help(text.tolerance)
            .accessibilityIdentifier("coinControl.summary")
            if coinControl.insufficientFunds {
                Text(L10n.CoinControl.insufficientFunds)
                    .dashFont(.footnoteMedium)
                    .foregroundStyle(Color.role.danger)
            }
        } else if coinControl.summaryUnavailable {
            Text(L10n.CoinControl.summaryUnavailable)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
        } else {
            Text(L10n.CoinControl.automaticallySelected)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
        }
    }

    @ViewBuilder
    private func cell(_ title: String, _ value: String, _ field: CoinSummaryField) -> some View {
        Text(title).foregroundStyle(Color.role.textSecondary)
        Text(value)
            .monospacedDigit()
            .contextMenu {
                Button(MacStrings.Common.copy) { MacPasteboard.copy(coinControl.copy(field)) }
            }
    }
}

/// The Send page's "Coin Control Features" box (QT-068): Inputs… opens the
/// Coin Selection window; the labels follow the selection.
struct SendCoinControlPanel: View {
    let coinControl: CoinControlViewModel
    let send: SendViewModel
    let openInputs: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            HStack {
                Text(L10n.CoinControl.featuresHeader).dashFont(.subheadMedium)
                Spacer()
                Button(L10n.CoinControl.inputs, action: openInputs)
                    .disabled(!send.isEditable)
                    .accessibilityIdentifier("send.coinControl.inputs")
            }
            if coinControl.isAutomatic {
                Text(L10n.CoinControl.automaticallySelected)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textSecondary)
                    .accessibilityIdentifier("send.coinControl.automatic")
            } else {
                CoinSummaryGrid(coinControl: coinControl)
            }
            // Send has no custom change address yet: change always returns
            // to the wallet, so the option is shown disabled with the reason.
            Toggle(L10n.CoinControl.customChange, isOn: .constant(false))
                .disabled(true)
                .help(MacStrings.CoinControl.customChangeUnavailable)
            Text(MacStrings.CoinControl.customChangeUnavailable)
                .dashFont(.caption1)
                .foregroundStyle(Color.role.textSecondary)
        }
        .padding(DashSpacing.l)
        .dashCard(padding: nil)
        .accessibilityIdentifier("send.coinControl")
    }
}
#endif
