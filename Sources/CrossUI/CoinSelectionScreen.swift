// dash-qt's Coin Selection dialog as a page (QT-068…074): list and tree
// mode, sorting by dash-qt's columns, the CoinJoin filter, (un)select all,
// (un)lock all, per-coin lock and copy, the summary values with copy, and
// the spent-coin notice. OK hands the selection to the Send page.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct CoinSelectionScreen: View {
    let model: CoinControlViewModel
    let state: CrossAppState

    var body: some View {
        let model = model
        let state = state
        Page(L10n.CoinControl.title) {
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashButton(L10n.CoinControl.selectAll, style: .tintedGray, size: .small) {
                    Task { await model.selectAll() }
                }
                DashButton(L10n.CoinControl.lockAll, style: .tintedGray, size: .small) {
                    Task { await model.lockAll() }
                }
                if !model.coinJoinPage {
                    DashPicker(
                        nil, accessibleName: CrossStrings.coinMode,
                        options: [
                            PickerOption(CoinControlMode.list, L10n.CoinControl.listMode),
                            PickerOption(.tree, L10n.CoinControl.treeMode),
                        ],
                        selection: bind({ model.mode }, { model.setMode($0) }))
                    DashButton(model.coinJoinToggleTitle, style: .plainBlue, size: .small) {
                        let show = !model.showCoinJoinCoins
                        Task { await model.setShowCoinJoinCoins(show) }
                    }
                }
                Text(model.lockedText).dashFont(.footnote).dashForeground(.secondaryText)
            }
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if model.unselectedNotice {
                Toast(L10n.CoinControl.coinsUnselected, kind: .warning, actionTitle: CrossStrings.dismiss) {
                    model.dismissUnselectedNotice()
                }
            }
            SummaryPanel(model: model, state: state)
            sortHeader(model)
            ScrollView {
                VStack(alignment: .leading, spacing: Int(DashSpacing.xs)) {
                    if model.coins.isEmpty {
                        Text(CrossStrings.noCoins).dashFont(.footnote).dashForeground(.secondaryText)
                    }
                    if model.mode == .tree {
                        ForEach(model.groups) { group in
                            VStack(alignment: .leading, spacing: Int(DashSpacing.xxs)) {
                                Text(CrossStrings.coinGroup(
                                    address: group.address, label: group.label, count: group.coins.count,
                                    total: state.format(group.total)))
                                    .dashFont(.footnoteMedium).dashForeground(.primaryText)
                                ForEach(group.coins) { coin in
                                    CoinRow(model: model, state: state, coin: coin).padding(.leading, Int(DashSpacing.l))
                                }
                            }
                        }
                    } else {
                        ForEach(model.coins) { coin in
                            CoinRow(model: model, state: state, coin: coin)
                        }
                    }
                }
            }
            .frame(height: 340)
            customChange(model)
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(CrossStrings.ok, style: .filledBlue, size: .small) {
                    state.main.send?.setSource(model.source())
                    state.closePage()
                    state.main.selection = .send
                }
                DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                    state.closePage()
                    state.main.selection = .send
                }
            }
        }
        .task {
            await model.load()
            await model.updatePayment(amounts: payAmounts(), fee: state.main.send?.fee ?? .recommended(targetBlocks: 6))
        }
    }

    /// Column buttons in dash-qt's order; the active one shows the direction.
    private func sortHeader(_ model: CoinControlViewModel) -> some View {
        HStack(spacing: Int(DashSpacing.xxs)) {
            Text(CrossStrings.sortBy).dashFont(.footnote).dashForeground(.secondaryText)
            ForEach(CoinColumn.allCases, id: \.self) { column in
                let active = model.sort.column == column
                DashButton(
                    L10n.CoinControl.columnTitle(column) + (active ? (model.sort.ascending ? " ▲" : " ▼") : ""),
                    style: active ? .tintedBlue : .plainBlue, size: .extraSmall
                ) { model.sort(by: column) }
            }
        }
    }

    /// QT-073: the Send page does not pass a change address to its draft yet
    /// (SendViewModel has no change setting), so the field is shown but off.
    @ViewBuilder
    private func customChange(_ model: CoinControlViewModel) -> some View {
        if !model.coinJoinPage {
            VStack(alignment: .leading, spacing: Int(DashSpacing.xxs)) {
                DashTextField(L10n.CoinControl.customChange, text: bind({ model.customChange ?? "" }, { _ in }))
                    .disabled(true)
                Text(CrossStrings.customChangeUnavailable).dashFont(.caption1).dashForeground(.secondaryText)
            }
        }
    }

    /// The Send page's amounts in the display unit; unparsable ones count as 0.
    private func payAmounts() -> [Amount] {
        guard let send = state.main.send else { return [] }
        let unit = state.env.settings.display.unit
        return send.entries.compactMap { entry in
            entry.amountText.isEmpty ? nil : try? state.env.amounts.parse(entry.amountText, unit: unit)
        }
    }
}

/// One coin: a switch named after it (selects it), its columns, Copy and
/// Lock/Unlock. Locked and reserved coins cannot be selected.
struct CoinRow: View {
    let model: CoinControlViewModel
    let state: CrossAppState
    let coin: Utxo

    var body: some View {
        let model = model
        let state = state
        let coin = coin
        let selectable = model.isSelectable(coin)
        HStack(alignment: .center, spacing: Int(DashSpacing.s)) {
            DashToggle(
                CrossStrings.selectCoin(model.amountText(coin), coin.address),
                isOn: bind({ model.selected.contains(coin.outpoint) }, { _ in Task { await model.toggle(coin.outpoint) } })
            )
            .disabled(!selectable)
            VStack(alignment: .leading, spacing: 2) {
                Text("\(model.amountText(coin))   \(model.label(of: coin))\(coin.userLocked ? "   " + CrossStrings.lockedCoin : "")")
                    .dashFont(.footnoteMedium).dashForeground(.primaryText).lineLimit(1)
                Text(Self.columns(coin)).dashFont(.caption1).dashForeground(.secondaryText).lineLimit(1)
            }
            Spacer()
            Menu(CrossStrings.copyMenu) {
                Button(L10n.CoinControl.copyAmount) { state.copy(model.copy(.amount, of: coin.outpoint), what: CrossStrings.amountWord) }
                Button(L10n.CoinControl.copyLabel) { state.copy(model.copy(.label, of: coin.outpoint), what: CrossStrings.labelWord) }
                Button(L10n.CoinControl.copyAddress) { state.copy(model.copy(.address, of: coin.outpoint), what: CrossStrings.addressWord) }
                Button(L10n.CoinControl.copyOutpoint) { state.copy(model.copy(.outpoint, of: coin.outpoint), what: CrossStrings.outpointWord) }
            }
            if coin.userLocked {
                DashButton(L10n.CoinControl.unlockUnspent, style: .plainBlue, size: .extraSmall) {
                    Task { await model.unlock(coin.outpoint) }
                }
            } else {
                DashButton(L10n.CoinControl.lockUnspent, style: .plainBlue, size: .extraSmall, isEnabled: !coin.reserved) {
                    Task { await model.lock(coin.outpoint) }
                }
            }
        }
    }

    /// Address, mixing rounds, date and confirmations (dash-qt's other columns).
    static func columns(_ coin: Utxo) -> String {
        [
            coin.address,
            "\(L10n.CoinControl.columnMixingRounds): \(coin.coinJoinRounds.map(String.init) ?? "n/a")",
            Format.date(coin.date),
            "\(L10n.CoinControl.columnConfirmations): \(coin.confirmations)",
        ].joined(separator: "   ")
    }
}

/// Quantity, Amount, Fee, After Fee, Bytes and Change ("≈" marks
/// estimates); "automatically selected" while nothing is picked.
struct SummaryPanel: View {
    let model: CoinControlViewModel
    let state: CrossAppState

    var body: some View {
        let model = model
        let state = state
        DashCard {
            if model.isAutomatic {
                Text(L10n.CoinControl.automaticallySelected).dashFont(.footnote).dashForeground(.secondaryText)
            } else if let text = model.summaryText {
                HStack(spacing: Int(DashSpacing.l)) {
                    KeyValueRow(L10n.CoinControl.quantity, text.quantity)
                    KeyValueRow(L10n.CoinControl.amount, text.amount)
                    KeyValueRow(L10n.CoinControl.fee, text.fee)
                }
                HStack(spacing: Int(DashSpacing.l)) {
                    KeyValueRow(L10n.CoinControl.afterFee, text.afterFee)
                    KeyValueRow(L10n.CoinControl.bytes, text.bytes)
                    KeyValueRow(L10n.CoinControl.change, text.change)
                }
                Text(text.tolerance).dashFont(.caption1).dashForeground(.secondaryText)
                if model.insufficientFunds {
                    Text(L10n.CoinControl.insufficientFunds).dashFont(.footnoteMedium).dashForeground(.red)
                }
                Menu(CrossStrings.copyMenu) {
                    Button(L10n.CoinControl.copyQuantity) { state.copy(model.copy(.quantity), what: L10n.CoinControl.quantity) }
                    Button(L10n.CoinControl.copyAmount) { state.copy(model.copy(.amount), what: L10n.CoinControl.amount) }
                    Button(L10n.CoinControl.copyFee) { state.copy(model.copy(.fee), what: L10n.CoinControl.fee) }
                    Button(L10n.CoinControl.copyAfterFee) { state.copy(model.copy(.afterFee), what: L10n.CoinControl.afterFee) }
                    Button(L10n.CoinControl.copyBytes) { state.copy(model.copy(.bytes), what: L10n.CoinControl.bytes) }
                    Button(L10n.CoinControl.copyChange) { state.copy(model.copy(.change), what: L10n.CoinControl.change) }
                }
            } else if model.summaryUnavailable {
                Text(CrossStrings.summaryUnavailable).dashFont(.footnote).dashForeground(.secondaryText)
            } else {
                Text(CrossStrings.loading).dashFont(.footnote).dashForeground(.secondaryText)
            }
        }
    }
}

/// The Send page's coin control panel (Options ▸ Wallet ▸ Enable coin
/// control features): Inputs… opens the Coin Selection page.
struct CoinControlPanel: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        if let model = state.coinControl() {
            DashCard {
                SectionHeader(L10n.CoinControl.featuresHeader, style: .subheadMedium)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.CoinControl.inputs, style: .tintedBlue, size: .small) { state.open(.coinSelection) }
                    if model.isAutomatic {
                        Text(L10n.CoinControl.automaticallySelected).dashFont(.footnote).dashForeground(.secondaryText)
                    } else if let text = model.summaryText {
                        Text("\(L10n.CoinControl.quantity) \(text.quantity)   \(L10n.CoinControl.amount) \(text.amount)")
                            .dashFont(.footnote)
                    }
                }
                if model.insufficientFunds {
                    Text(L10n.CoinControl.insufficientFunds).dashFont(.footnoteMedium).dashForeground(.red)
                }
            }
        }
    }
}
