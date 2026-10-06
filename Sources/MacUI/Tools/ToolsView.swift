// Tools window (QT-143…148, QT-040, IOS-113): Information, Console, Network
// Traffic (not available yet), Peers with disconnect/ban/unban, and Repair
// (rescan, cancel, reset chain data, remove unconfirmed, birth height).
// Rows an SPV wallet cannot know show "—" with "Requires full-node data
// source" (DESIGN-opus §1.14).
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct ToolsWindow: View {
    @Bindable var model: MacAppModel

    var body: some View {
        if let features = model.features {
            ToolsView(features: features, tab: $model.toolsTab)
        } else {
            Text(model.unavailableReason ?? L10n.Options.unavailable).padding(DashSpacing.xl)
        }
    }
}

struct ToolsView: View {
    let features: MacFeatureModels
    @Binding var tab: ToolsTab

    var body: some View {
        VStack(spacing: 0) {
            DashSegmentedControl(ToolsTab.allCases.map { ($0, $0.title) }, selection: $tab)
                .padding(DashSpacing.m)
                .accessibilityIdentifier("tools.tabs")
            Group {
                switch tab {
                case .information: InformationView(information: features.information)
                case .console: ConsoleView(console: features.console)
                case .networkTraffic: NetworkTrafficUnavailableView()
                case .peers: PeersToolView(peers: features.peers)
                case .repair: RepairView(repair: features.repair)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 700, minHeight: 480)
        .dashCanvas()
        .accessibilityIdentifier("tools")
    }
}

// MARK: Information (QT-143, QT-040)

struct InformationView: View {
    let information: InformationViewModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DashSpacing.l) {
                if let banner = information.bannerText {
                    SystemNotice(text: banner, tone: .warning)
                }
                if let error = information.errorMessage {
                    SystemNotice(text: error, tone: .error)
                }
                // Sections as cards (UX-SPEC §4.16); hashes in the technical face.
                ForEach(information.sections) { section in
                    VStack(alignment: .leading, spacing: DashSpacing.s) {
                        Text(section.title)
                            .dashFont(.headline)
                            .foregroundStyle(Color.role.textPrimary)
                        Grid(alignment: .leading, horizontalSpacing: DashSpacing.l, verticalSpacing: DashSpacing.s) {
                            ForEach(section.rows) { row in
                                GridRow(alignment: .firstTextBaseline) {
                                    Text(row.title)
                                        .foregroundStyle(Color.role.textSecondary)
                                        .frame(width: 200, alignment: .leading)
                                    VStack(alignment: .leading, spacing: 0) {
                                        Text(row.value)
                                            .font(Self.isHash(row.value)
                                                ? .system(size: DesignTokens.DashTextStyle.footnote.size, design: .monospaced)
                                                : DesignTokens.DashTextStyle.footnote.font)
                                            .monospacedDigit()
                                            .foregroundStyle(Color.role.textPrimary)
                                            .textSelection(.enabled)
                                            .lineLimit(2)
                                            .truncationMode(.middle)
                                            .help(row.note ?? row.value)
                                        if let note = row.note {
                                            Text(note)
                                                .dashFont(.caption1)
                                                .foregroundStyle(Color.role.textTertiary)
                                        }
                                    }
                                }
                                .accessibilityElement(children: .combine)
                                .accessibilityIdentifier("information.\(row.title)")
                            }
                        }
                        .dashFont(.footnote)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .dashCard(padding: DashLayout.cardPadding)
                }
                if information.sections.isEmpty, information.errorMessage == nil {
                    ProgressView().frame(maxWidth: .infinity)
                }
            }
            .padding(DashSpacing.l)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .task {
            await information.load()
            information.start()
        }
        .onDisappear { information.stop() }
        .accessibilityIdentifier("tools.information")
    }

    /// A block or transaction hash: 64 hexadecimal characters.
    static func isHash(_ value: String) -> Bool {
        value.count == 64 && value.allSatisfy(\.isHexDigit)
    }
}

// MARK: Console (QT-145)

struct ConsoleView: View {
    let console: ConsoleViewModel
    @State private var line = ""
    @State private var passphrase = ""
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(spacing: DashSpacing.s) {
            HStack {
                if console.showsWalletSelector {
                    Picker(MacStrings.Toolbar.wallet, selection: Binding(
                        get: { console.selectedWalletID }, set: { console.selectWallet($0) }
                    )) {
                        Text(L10n.Tools.noWalletSelection).tag(WalletID?.none)
                        ForEach(console.wallets) { wallet in Text(wallet.name).tag(Optional(wallet.id)) }
                    }
                    .frame(width: 240)
                    .accessibilityIdentifier("console.wallet")
                }
                Spacer()
                Button { console.decreaseFontSize() } label: { Image(systemName: "textformat.size.smaller") }
                    .buttonStyle(.dash(.tintedGray, .small))
                    .keyboardShortcut("-", modifiers: .command)
                    .help(MacStrings.Console.smaller)
                    .accessibilityLabel(MacStrings.Console.smaller)
                Button { console.increaseFontSize() } label: { Image(systemName: "textformat.size.larger") }
                    .buttonStyle(.dash(.tintedGray, .small))
                    .keyboardShortcut("+", modifiers: .command)
                    .help(MacStrings.Console.bigger)
                    .accessibilityLabel(MacStrings.Console.bigger)
                Button { console.clear() } label: { Image(systemName: "trash") }
                    .buttonStyle(.dash(.tintedGray, .small))
                    .keyboardShortcut("l", modifiers: .command)
                    .help(MacStrings.Console.clear)
                    .accessibilityLabel(MacStrings.Console.clear)
                    .accessibilityIdentifier("console.clear")
            }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: DashSpacing.xs) {
                        ForEach(console.entries) { entry in
                            ConsoleEntryRow(entry: entry, fontSize: CGFloat(console.fontSize))
                                .id(entry.id)
                        }
                    }
                    .padding(DashSpacing.s)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .dashCard(radius: DashRadius.standard, padding: nil)
                .onChange(of: console.entries.count) { _, _ in
                    if let last = console.entries.last { proxy.scrollTo(last.id, anchor: .bottom) }
                }
            }
            .accessibilityIdentifier("console.output")
            if let error = console.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("console.error")
            }
            if case .awaitingPassphrase = console.state {
                HStack {
                    Text(L10n.Lock.prompt).dashFont(.footnote)
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.dash)
                        .onSubmit(authorize)
                        .accessibilityIdentifier("console.passphrase")
                    Button(MacStrings.Common.cancel) { console.cancelAuthorization() }
                        .buttonStyle(.dash(.tintedGray, .small))
                    Button(MacStrings.Common.ok, action: authorize)
                        .buttonStyle(.dash(.filledBlue, .small))
                        .disabled(passphrase.isEmpty)
                }
            }
            HStack {
                Text(">")
                    .font(.system(size: CGFloat(console.fontSize), design: .monospaced))
                    .foregroundStyle(Color.role.textLink)
                TextField(MacStrings.Console.placeholder, text: $line)
                    .textFieldStyle(.plain)
                    .font(.system(size: CGFloat(console.fontSize), design: .monospaced))
                    .padding(.horizontal, DashSpacing.m)
                    .frame(height: 34)
                    .background(RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous).fill(Color.role.fieldFill))
                    .focused($inputFocused)
                    .onSubmit(run)
                    .onKeyPress(.upArrow) {
                        if let previous = console.historyUp() { line = previous }
                        return .handled
                    }
                    .onKeyPress(.downArrow) {
                        line = console.historyDown() ?? ""
                        return .handled
                    }
                    .onKeyPress(.tab) {
                        let matches = console.completions(for: line)
                        if matches.count == 1, let match = matches.first { line = match + " " }
                        return .handled
                    }
                    .disabled(console.state != .idle)
                    .accessibilityIdentifier("console.input")
                if console.state == .executing { ProgressView().controlSize(.small) }
            }
        }
        .padding(DashLayout.pagePaddingH)
        .task {
            if console.entries.isEmpty { console.clear() }
            await console.load()
            inputFocused = true
        }
        .accessibilityIdentifier("tools.console")
    }

    private func run() {
        let text = line
        line = ""
        Task { await console.run(line: text) }
    }

    private func authorize() {
        let text = passphrase
        passphrase = ""
        Task { await console.authorize(passphrase: text) }
    }
}

private struct ConsoleEntryRow: View {
    let entry: ConsoleEntry
    let fontSize: CGFloat

    var body: some View {
        HStack(alignment: .top, spacing: DashSpacing.xs) {
            Image(systemName: icon)
                .foregroundStyle(color)
                .frame(width: 16)
                .accessibilityHidden(true)
            Text(entry.text)
                .font(entry.kind == .welcome || entry.kind == .warning
                    ? .system(size: fontSize) : .system(size: fontSize, design: .monospaced))
                .foregroundStyle(
                    entry.kind == .warning || entry.kind == .error || entry.kind == .command ? color : Color.role.textPrimary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("console.entry.\(entry.kind)")
    }

    private var icon: String {
        switch entry.kind {
        case .welcome, .info: "info.circle"
        case .warning: "exclamationmark.triangle.fill"
        case .command: "chevron.right"
        case .reply: "arrow.turn.down.left"
        case .error: "xmark.octagon"
        }
    }

    private var color: Color {
        switch entry.kind {
        case .welcome, .info: Color.role.textSecondary
        case .command: Color.role.textLink
        case .warning: Color.role.warning
        case .reply: Color.role.success
        case .error: Color.role.danger
        }
    }
}

// MARK: Network traffic

/// dash-qt's traffic graph needs per-peer byte counters the SPV client does
/// not expose yet; the tab says so instead of drawing an empty graph.
private struct NetworkTrafficUnavailableView: View {
    var body: some View {
        EmptyState(icon: .system("chart.xyaxis.line"), title: L10n.Shell.networkTrafficUnavailable)
            .dashCard(padding: nil)
            .padding(DashLayout.pagePaddingH)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }
}

// MARK: Peers (QT-147)

struct PeersToolView: View {
    let peers: PeersViewModel
    @State private var selection: String?
    @State private var bannedSelection: String?

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            Table(peerRows, selection: $selection) {
                TableColumn(MacStrings.Peers.address) { Text($0.peer.address).monospacedDigit() }
                TableColumn(MacStrings.Peers.userAgent) { Text($0.peer.userAgent ?? L10n.Common.unknown) }
                TableColumn(MacStrings.Peers.height) { row in
                    Text(row.peer.bestHeight.map { "\($0)" } ?? L10n.Common.unknown)
                }
                .width(80)
                TableColumn(MacStrings.Peers.ping) { row in
                    Text(row.peer.pingMilliseconds.map { "\($0) ms" } ?? L10n.Common.unknown)
                }
                .width(70)
                TableColumn(MacStrings.Peers.direction) { row in
                    Text(row.peer.inbound ? MacStrings.Peers.inbound : MacStrings.Peers.outbound)
                }
                .width(80)
            }
            .contextMenu(forSelectionType: String.self) { ids in
                if let address = ids.first {
                    Button(L10n.Tools.copyAddress) { MacPasteboard.copy(address) }
                    if peers.canModerate {
                        Divider()
                        Button(L10n.Tools.disconnect) { Task { await peers.disconnect(address) } }
                        ForEach(BanDuration.allCases, id: \.self) { duration in
                            Button(duration.title) { Task { await peers.ban(address, for: duration) } }
                        }
                    }
                }
            }
            .frame(minHeight: 200)
            .clipShape(RoundedRectangle(cornerRadius: DashRadius.group, style: .continuous))
            .dashCard(radius: DashRadius.group, padding: nil)
            .accessibilityIdentifier("peers.table")
            if peers.showsBannedList {
                Text(L10n.Tools.bannedPeers).dashFont(.headline)
                Table(bannedRows, selection: $bannedSelection) {
                    TableColumn(L10n.Tools.bannedSubnet) { Text($0.peer.subnet).monospacedDigit() }
                    TableColumn(L10n.Tools.bannedUntil) { Text($0.peer.bannedUntil.formatted()) }
                }
                .contextMenu(forSelectionType: String.self) { ids in
                    if let subnet = ids.first {
                        Button(L10n.Tools.copySubnet) { MacPasteboard.copy(subnet) }
                        Button(L10n.Tools.unban) { Task { await peers.unban(subnet) } }
                    }
                }
                .frame(minHeight: 90)
            }
            if let error = peers.error {
                Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
                    .accessibilityIdentifier("peers.error")
            }
            HStack {
                Button(MacStrings.Peers.changePeers) { Task { await peers.rotate() } }
                    .buttonStyle(.dash(.tintedBlue, .small))
                    .disabled(peers.rotating)
                    .help(MacStrings.Peers.changePeersHelp)
                if let selection, peers.canModerate {
                    Button(L10n.Tools.disconnect) { Task { await peers.disconnect(selection) } }
                        .buttonStyle(.dash(.tintedGray, .small))
                    Menu(MacStrings.PeerTools.ban) {
                        ForEach(BanDuration.allCases, id: \.self) { duration in
                            Button(duration.title) { Task { await peers.ban(selection, for: duration) } }
                        }
                    }
                    .fixedSize()
                }
                Spacer()
                Button(MacStrings.Common.refresh) { Task { await peers.load() } }
                    .buttonStyle(.dash(.plainBlue, .small))
            }
        }
        .padding(DashLayout.pagePaddingH)
        .task { await peers.load() }
        .accessibilityIdentifier("tools.peers")
    }

    private var peerRows: [ToolPeerRow] { (peers.peers ?? []).map(ToolPeerRow.init) }
    private var bannedRows: [BannedRow] { peers.banned.map(BannedRow.init) }
}

private struct ToolPeerRow: Identifiable {
    let peer: PeerInfo
    var id: String { peer.address }
}

private struct BannedRow: Identifiable {
    let peer: BannedPeer
    var id: String { peer.subnet }
}

// MARK: Repair (QT-117, QT-148, IOS-034, IOS-113)

struct RepairView: View {
    let repair: RepairViewModel
    @State private var birthHeight = ""

    /// Menu-card rows with the dash-qt actions (UX-SPEC §4.16).
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
                MenuCard {
                    MenuRow(icon: .token(.rescanBlockchain), title: L10n.Tools.rescan) {
                        Button(L10n.Tools.rescan) { Task { await repair.rescan(.walletBirth) } }
                            .buttonStyle(.dash(.tintedBlue, .small))
                            .accessibilityIdentifier("repair.rescan")
                    }
                    MenuRow(icon: .token(.rescanBlockchain), title: L10n.Tools.rescanFull) {
                        Button(L10n.Tools.rescanFull) { Task { await repair.rescan(.genesis) } }
                            .buttonStyle(.dash(.tintedBlue, .small))
                            .accessibilityIdentifier("repair.rescanFull")
                    }
                    if repair.isRescanning {
                        VStack(alignment: .leading, spacing: DashSpacing.xs) {
                            DashProgressBar(value: repair.progressFraction)
                            HStack {
                                if let text = repair.progressText {
                                    Text(text).dashFont(.footnote).foregroundStyle(Color.role.textSecondary)
                                }
                                Spacer()
                                Button(L10n.Tools.cancelRescan) { Task { await repair.cancelRescan() } }
                                    .buttonStyle(.dash(.plainRed, .small))
                            }
                        }
                        .padding(DashSpacing.sm)
                    }
                }
                .disabled(repair.state == .working)
                MenuCard {
                    MenuActionRow(icon: .token(.resetWallet), title: L10n.Tools.resetChainData, isDestructive: true) {
                        repair.requestResetChainData()
                    }
                    .accessibilityIdentifier("repair.reset")
                    MenuActionRow(icon: .token(.txError), title: L10n.Tools.dropUnconfirmed) {
                        repair.requestDropUnconfirmed()
                    }
                    .accessibilityIdentifier("repair.dropUnconfirmed")
                }
                .disabled(repair.state == .working)
                MenuCard(title: L10n.Tools.birthHeight) {
                    HStack(spacing: DashSpacing.s) {
                        TextField(L10n.Tools.birthHeight, text: $birthHeight)
                            .textFieldStyle(.dash)
                            .monospacedDigit()
                            .accessibilityIdentifier("repair.birthHeight")
                        Button(MacStrings.Common.apply) {
                            let text = birthHeight
                            Task { await repair.setBirthHeight(text) }
                        }
                        .buttonStyle(.dash(.tintedBlue, .medium))
                        .disabled(birthHeight.isEmpty)
                    }
                    .padding(DashSpacing.xs)
                }
                switch repair.state {
                case .working:
                    LoadingState(MacStrings.Wallets.working)
                case .done(let text):
                    resultRow(SystemNotice(text: text, tone: .info))
                case .failed(let text):
                    resultRow(SystemNotice(text: text, tone: .error))
                case .idle, .confirming:
                    EmptyView()
                }
            }
            .padding(DashLayout.pagePaddingH)
        }
        .task {
            await repair.refresh()
            repair.start()
        }
        .onDisappear { repair.stop() }
        .alert(L10n.Shell.repair, isPresented: confirmBinding) {
            Button(MacStrings.Common.cancel, role: .cancel) { repair.cancelConfirmation() }
            Button(MacStrings.Common.ok, role: .destructive) { Task { await repair.confirm() } }
        } message: {
            if case .confirming(let confirmation) = repair.state { Text(confirmation.message) }
        }
        .accessibilityIdentifier("tools.repair")
    }

    private var confirmBinding: Binding<Bool> {
        Binding(get: { if case .confirming = repair.state { true } else { false } }, set: { _ in })
    }

    private func resultRow(_ notice: SystemNotice) -> some View {
        HStack(alignment: .top) {
            notice
            Button(MacStrings.Common.ok) { repair.dismissResult() }
                .buttonStyle(.dash(.tintedGray, .small))
        }
    }
}
#endif
