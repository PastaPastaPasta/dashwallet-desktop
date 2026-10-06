// dash-qt's Tools window as a page with its tabs (research 02 §14):
// Information (QT-143), Console (QT-145), Network Traffic (not available,
// QT-144), Peers (QT-024, QT-146/147) and Repair (QT-117, QT-148, IOS-034,
// IOS-113).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct ToolsWindowScreen: View {
    let state: CrossAppState
    let tab: ToolsTab

    var body: some View {
        let state = state
        Page(L10n.Tools.windowTitle, width: .full) {
            HStack(spacing: Int(DashSpacing.s)) {
                SegmentedControl(
                    options: ToolsTab.allCases.map { PickerOption($0, $0.title) }, selection: tab,
                    disabled: [.networkTraffic], help: [.networkTraffic: L10n.Shell.networkTrafficUnavailable]
                ) { state.open(.tools($0)) }
                Spacer()
                DashButton(CrossStrings.close, style: .tintedGray, size: .small) { state.closePage() }
            }
            switch tab {
            case .information: InformationTab(model: state.information)
            case .console: ConsoleTab(model: state.console)
            case .networkTraffic:
                DashCard { EmptyState(icon: .networkMonitor, title: L10n.Shell.networkTrafficUnavailable) }
            case .peers: PeersTab(model: state.peers)
            case .repair: RepairTab(model: state.repair)
            }
        }
    }
}

// MARK: Information

/// General, Network, Block chain, Memory Pool and Masternodes rows; rows an
/// SPV wallet cannot fill say "Requires full-node data source".
struct InformationTab: View {
    let model: InformationViewModel

    var body: some View {
        let model = model
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            ForEach(model.warnings, id: \.self) { warning in
                Toast(warning.text, kind: .warning)
            }
            if model.sections.isEmpty, model.errorMessage == nil {
                LoadingState(CrossStrings.loading)
            }
            ForEach(model.sections) { section in
                DashCard(spacing: Int(DashSpacing.xs)) {
                    SectionHeader(section.title, style: .headline)
                    ForEach(section.rows) { row in
                        KeyValueRow(
                            row.title, row.note.map { "\(row.value)  (\($0))" } ?? row.value, help: row.note,
                            monospaced: InformationTab.isTechnical(row.title))
                    }
                }
            }
        }
        .task { await model.load() }
    }

    /// Hashes are technical values (UX-SPEC §5.6): monospaced.
    static func isTechnical(_ title: String) -> Bool {
        title.localizedCaseInsensitiveContains("hash")
    }
}

// MARK: Console

/// The RPC console: wallet selector (two or more wallets), output with the
/// welcome text and anti-scam warning, history (Up/Down buttons; SwiftCrossUI
/// 0.10 has no key handlers on text fields), completions, font size, and the
/// passphrase prompt for commands that spend, sign or reveal.
struct ConsoleTab: View {
    let model: ConsoleViewModel

    @State var line = ""
    @State var passphrase = ""

    var body: some View {
        let model = model
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                if model.showsWalletSelector {
                    DashPicker(
                        CrossStrings.wallet,
                        options: [PickerOption(Optional<WalletID>.none, L10n.Tools.noWalletSelection)]
                            + model.wallets.map { PickerOption(Optional($0.id), $0.name) },
                        selection: bind({ model.selectedWalletID }, { model.selectWallet($0) }))
                }
                Spacer()
                DashButton(CrossStrings.fontSmaller, style: .tintedGray, size: .small) { model.decreaseFontSize() }
                DashButton(CrossStrings.fontBigger, style: .tintedGray, size: .small) { model.increaseFontSize() }
                DashButton(CrossStrings.clearConsole, style: .tintedGray, size: .small) { model.clear() }
            }
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            // Console output: the technical monospaced view (UX-SPEC §4.16, §5.6).
            ScrollView {
                VStack(alignment: .leading, spacing: Int(DashSpacing.xs)) {
                    ForEach(model.entries) { entry in
                        Text(Self.prefix(entry.kind) + entry.text)
                            .font(.system(size: Double(model.fontSize)).monospaced())
                            .foregroundColor(Self.color(entry.kind).color)
                            .textSelectionEnabled()
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(Int(DashSpacing.m))
            }
            .frame(height: 300)
            .cardBackground(radius: Int(DashRadius.standard))
            if case .awaitingPassphrase = model.state {
                DashCard {
                    Text(CrossStrings.consoleNeedsPassphrase).dashFont(.footnote)
                    DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                    HStack(spacing: Int(DashSpacing.s)) {
                        DashButton(CrossStrings.authorize, style: .filledBlue, size: .small) {
                            let text = passphrase
                            passphrase = ""
                            Task { await model.authorize(passphrase: text) }
                        }
                        DashButton(CrossStrings.cancel, style: .tintedGray, size: .small) {
                            passphrase = ""
                            model.cancelAuthorization()
                        }
                    }
                }
            }
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(CrossStrings.consoleCommand, placeholder: CrossStrings.consolePlaceholder, text: $line)
                    .onSubmit { run(model) }
                DashButton(CrossStrings.run, style: .filledBlue, size: .small, isEnabled: model.state == .idle) {
                    run(model)
                }
                DashButton(CrossStrings.historyUp, style: .tintedGray, size: .small) {
                    if let previous = model.historyUp() { line = previous }
                }
                DashButton(CrossStrings.historyDown, style: .tintedGray, size: .small) {
                    if let next = model.historyDown() { line = next }
                }
                DashButton(CrossStrings.complete, style: .tintedGray, size: .small) {
                    let matches = model.completions(for: line)
                    if matches.count == 1 { line = matches[0] }
                }
            }
            if model.state == .executing {
                Text(L10n.Tools.executing).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
            }
            let matches = line.isEmpty || line.contains(" ") ? [] : model.completions(for: line)
            if matches.count > 1 {
                Text(matches.prefix(12).joined(separator: "  ")).dashFont(.caption1).dashForeground(CrossRole.textSecondary)
            }
        }
        .task { await model.load() }
    }

    private func run(_ model: ConsoleViewModel) {
        let text = line
        line = ""
        Task { await model.run(line: text) }
    }

    static func prefix(_ kind: ConsoleEntryKind) -> String {
        switch kind {
        case .command: "> "
        default: ""
        }
    }

    static func color(_ kind: ConsoleEntryKind) -> DashColor {
        switch kind {
        case .warning, .error: CrossRole.danger
        case .command: CrossRole.textLink
        case .welcome, .info: CrossRole.textSecondary
        case .reply: CrossRole.textPrimary
        }
    }
}

// MARK: Peers

/// Connected peers with Change Peers, Disconnect and Ban, and the banned
/// list with Unban (dash-qt shows it only when not empty). The engine
/// answers `not_implemented` for moderation until upstream U2; the error
/// says so.
struct PeersTab: View {
    let model: PeersViewModel

    @State var selected: String?
    @State var banDuration: BanDuration = .day

    var body: some View {
        let model = model
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(
                    L10n.Peers.changePeers, style: .tintedBlue, size: .small, isEnabled: !model.rotating,
                    help: L10n.Peers.changePeersHelp
                ) { Task { await model.rotate() } }
            }
            if let error = model.error {
                Toast(error, kind: .error)
            }
            if let peers = model.peers {
                if peers.isEmpty {
                    Text(L10n.Peers.none).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
                } else {
                    let names = peers.map(Self.rowName)
                    // ADR 0002: the list scrolls inside a fixed-height ScrollView.
                    ScrollView {
                        List(peers.map(PeerRow.init), selection: bind({ selected }, { selected = $0 })) { row in
                            MenuItem(icon: .connections, title: row.peer.address, subtitle: Self.details(row.peer))
                                .padding(.horizontal, Int(DashSpacing.m))
                        }
                        .accessibleRowNames(names)
                    }
                    .frame(height: 300)
                    .cardBackground(radius: CrossLayout.groupRadius)
                    if model.canModerate, let address = selected {
                        HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                            Text(address).dashFont(.footnoteMedium)
                            DashButton(L10n.Tools.disconnect, style: .tintedGray, size: .small) {
                                Task { await model.disconnect(address) }
                            }
                            DashPicker(
                                nil, accessibleName: CrossStrings.banDuration,
                                options: BanDuration.allCases.map { PickerOption($0, $0.title) },
                                selection: $banDuration)
                            DashButton(CrossStrings.ban, style: .plainRed, size: .small) {
                                let duration = banDuration
                                Task { await model.ban(address, for: duration) }
                            }
                        }
                    }
                }
            } else if model.error == nil {
                LoadingState(CrossStrings.loading)
            }
            if model.showsBannedList {
                DashCard {
                    SectionHeader(L10n.Tools.bannedPeers, style: .headline)
                    ForEach(model.banned, id: \.subnet) { peer in
                        HStack(spacing: Int(DashSpacing.s)) {
                            MenuItem(
                                title: peer.subnet,
                                subtitle: "\(L10n.Tools.bannedUntil): \(Format.date(peer.bannedUntil))")
                            DashButton(L10n.Tools.unban, style: .plainBlue, size: .small) {
                                Task { await model.unban(peer.subnet) }
                            }
                        }
                    }
                }
            }
        }
        .task { await model.load() }
    }

    /// User agent, best height, ping and direction; unknown fields say so
    /// (dash-spv reports only addresses today).
    static func details(_ peer: PeerInfo) -> String {
        [
            "\(L10n.Peers.userAgent): \(peer.userAgent ?? L10n.Common.unknown)",
            "\(L10n.Peers.height): \(peer.bestHeight.map(String.init) ?? L10n.Common.unknown)",
            "\(L10n.Peers.ping): \(peer.pingMilliseconds.map { "\($0) ms" } ?? L10n.Common.unknown)",
            peer.inbound ? L10n.Peers.inbound : L10n.Peers.outbound,
        ].joined(separator: "  ")
    }

    static func rowName(_ peer: PeerInfo) -> String {
        "\(peer.address), \(details(peer))"
    }
}

struct PeerRow: Identifiable, Hashable {
    let peer: PeerInfo
    var id: String { peer.address }

    static func == (lhs: PeerRow, rhs: PeerRow) -> Bool { lhs.peer.address == rhs.peer.address }
    func hash(into hasher: inout Hasher) { hasher.combine(peer.address) }
}

// MARK: Repair

/// Rescan from the wallet birthday or genesis with progress and Cancel,
/// reset chain data and drop unconfirmed transactions (each asks first), and
/// the wallet birth height.
struct RepairTab: View {
    let model: RepairViewModel

    @State var birthHeight = ""

    var body: some View {
        let model = model
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            switch model.state {
            case .confirming(let confirmation):
                ConfirmationCard(
                    title: confirmation.title, message: confirmation.message, confirmTitle: CrossStrings.yes,
                    destructive: true, onConfirm: { Task { await model.confirm() } },
                    onCancel: { model.cancelConfirmation() })
            case .working:
                LoadingState(CrossStrings.working)
            case .done(let text):
                Toast(text, kind: .success, actionTitle: CrossStrings.dismiss) { model.dismissResult() }
            case .failed(let text):
                Toast(text, kind: .error, actionTitle: CrossStrings.dismiss) { model.dismissResult() }
            case .idle:
                EmptyView()
            }
            DashCard {
                MenuItem(icon: .rescanBlockchain, title: L10n.Tools.rescan)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.Tools.rescan, style: .tintedBlue, size: .small, isEnabled: !model.isRescanning) {
                        Task { await model.rescan(.walletBirth) }
                    }
                    DashButton(L10n.Tools.rescanFull, style: .tintedBlue, size: .small, isEnabled: !model.isRescanning) {
                        Task { await model.rescan(.genesis) }
                    }
                    if model.isRescanning {
                        DashButton(L10n.Tools.cancelRescan, style: .tintedGray, size: .small) {
                            Task { await model.cancelRescan() }
                        }
                    }
                }
                if model.isRescanning {
                    if let fraction = model.progressFraction {
                        ProgressView(value: fraction)
                    }
                    Text(model.progressText ?? CrossStrings.rescanning).dashFont(.footnote)
                }
            }
            DashCard {
                MenuItem(icon: .resetWallet, title: L10n.Tools.resetChainData, destructive: true)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(L10n.Tools.resetChainData, style: .plainRed, size: .small) {
                        model.requestResetChainData()
                    }
                    DashButton(L10n.Tools.dropUnconfirmed, style: .plainRed, size: .small) {
                        model.requestDropUnconfirmed()
                    }
                }
            }
            DashCard {
                MenuItem(icon: .settings, title: L10n.Tools.birthHeight)
                HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                    DashTextField(L10n.Tools.birthHeight, placeholder: CrossStrings.blockHeight, text: $birthHeight, width: 200)
                    DashButton(CrossStrings.save, style: .tintedBlue, size: .small, isEnabled: !birthHeight.isEmpty) {
                        let text = birthHeight
                        Task { await model.setBirthHeight(text) }
                    }
                }
            }
        }
        .task {
            model.start()
            await model.refresh()
        }
    }
}
