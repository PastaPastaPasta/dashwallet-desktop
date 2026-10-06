// Window content: onboarding, lock screen, the sync overlay or the sidebar +
// page layout, with the lifecycle overlay, the shutdown page and the status
// row (QT-008, QT-011…014, QT-020…024, QT-027, QT-040, IOS-013, IOS-018).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

public struct WalletRootView: View {
    let state: CrossAppState

    @Environment(\.chooseFile) var chooseFile

    public init(state: CrossAppState) {
        self.state = state
    }

    public var body: some View {
        let state = state
        let main = state.main
        VStack(spacing: 0) {
            if showsMainWindow(main) {
                ShellBanners(state: state)
            }
            content(main)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            StatusRow(state: state)
        }
        .background(DashColor.primaryBackground.color)
        .preferredColorScheme(colorScheme(main.settings.theme))
        .onChange(of: main.home?.sync, initial: true) {
            if let status = main.home?.sync { main.syncRates.record(status) }
        }
        .onChange(of: main.network) {
            // The shell reads the network only on refresh (window title, menus).
            Task { await state.shell.refresh() }
        }
        .onChange(of: main.sheet) {
            // A route an M1 view model raised replaces the M2 page.
            if main.sheet != nil { state.page = nil }
        }
        .task {
            let choose = chooseFile
            state.chooseFile = { title in await choose(title: title, defaultButtonLabel: CrossStrings.open) }
            await main.start()
            await state.shell.start()
            state.information.start()
            await state.information.load()
        }
    }

    @ViewBuilder
    private func content(_ main: MainViewModel) -> some View {
        if state.shutdown.isVisible {
            ShutdownScreen(model: state.shutdown)
        } else if main.showsTransitionOverlay {
            CenteredMessage(text: Format.transition(main.transition), busy: true)
        } else if main.wallets == nil {
            CenteredMessage(text: main.errorMessage ?? CrossStrings.loading, busy: main.errorMessage == nil)
        } else if main.needsOnboarding, let onboarding = main.onboarding {
            OnboardingScreen(model: onboarding)
        } else if main.showsLockScreen {
            LockScreen(model: main.lock)
        } else if main.showsSyncOverlay, let status = main.home?.sync {
            SyncOverlayScreen(state: state, status: status)
        } else {
            MainSplitView(state: state)
        }
    }

    /// The sidebar and pages are shown (not onboarding, lock, overlays).
    private func showsMainWindow(_ main: MainViewModel) -> Bool {
        !state.shutdown.isVisible && !main.showsTransitionOverlay && main.wallets != nil && !main.needsOnboarding
            && !main.showsLockScreen && !main.showsSyncOverlay
    }

    private func colorScheme(_ theme: AppTheme) -> ColorScheme? {
        switch theme {
        case .light: .light
        case .dark: .dark
        case .system: nil
        }
    }
}

/// Sidebar (sections, tools, wallet selector) and the selected page.
struct MainSplitView: View {
    let state: CrossAppState

    var body: some View {
        let main = state.main
        NavigationSplitView {
            Sidebar(state: state)
                .frame(minWidth: 200)
        } detail: {
            detail(main)
                .frame(minWidth: 560, maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
    }

    /// The page itself, without a wrapping stack: every SwiftCrossUI layer is
    /// another GTK container in the accessibility tree (ADR 0002 gap A6).
    @ViewBuilder
    private func detail(_ main: MainViewModel) -> some View {
        if let page = state.currentPage {
            toolPage(page)
        } else {
            switch main.selection {
            case .overview:
                if let home = main.home { OverviewScreen(model: home, state: state) }
            case .send:
                if let send = main.send { SendScreen(model: send, state: state) }
            case .receive:
                if let receive = main.receive { ReceiveScreen(model: receive) }
            case .transactions:
                if let transactions = state.transactions() { TransactionsScreen(model: transactions, state: state) }
            case .coinJoin, .masternodes, .governance, .contacts, .explore:
                Page(main.selection.title) { Text(L10n.Common.notAvailableYet) }
            }
        }
    }

    @ViewBuilder
    private func toolPage(_ page: ToolPage) -> some View {
        switch page {
        case .addressBook(let purpose):
            if let model = state.addressBook(purpose: purpose) { AddressBookScreen(model: model, state: state) }
        case .signVerify:
            if let model = state.signVerify() { SignVerifyScreen(model: model) }
        case .settings:
            SettingsScreen(model: state.main.settings, state: state)
        case .options:
            OptionsScreen(model: state.options, state: state)
        case .coinSelection:
            if let model = state.coinControl() { CoinSelectionScreen(model: model, state: state) }
        case .psbt:
            PSBTScreen(model: state.psbt, state: state)
        case .tools(let tab):
            ToolsWindowScreen(state: state, tab: tab)
        case .wallets:
            WalletsScreen(model: state.wallets, state: state)
        case .security:
            SecurityScreen(model: state.security, state: state)
        case .about:
            AboutScreen(model: state.about, state: state, showsOptions: false)
        case .commandLineOptions:
            AboutScreen(model: state.about, state: state, showsOptions: true)
        case .openURI:
            OpenURIScreen(state: state)
        case .createWallet:
            CreateWalletScreen(state: state)
        }
    }
}

struct Sidebar: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let main = state.main
        let items = state.shell.sections
        let selection = bind(
            { state.currentPage == nil ? Optional(main.selection.id) : nil },
            { (id: String?) in
                guard let id, let item = SidebarItem(rawValue: id) else { return }
                state.closePage()
                main.selection = item
                Task { await state.shell.perform(.section(item)) }
            })
        VStack(alignment: .leading, spacing: Int(DashSpacing.m)) {
            SectionHeader(L10n.Navigation.appName, style: .headline)
            if main.showsWalletSelector, let wallets = main.wallets {
                DashPicker(
                    CrossStrings.wallet,
                    options: wallets.map { PickerOption($0.id, $0.name.isEmpty ? String($0.id.hex.prefix(8)) : $0.name) },
                    selection: bind(
                        { main.selectedWalletID ?? wallets[0].id },
                        { id in Task { await main.selectWallet(id) } }))
            }
            // ADR 0002 rule until fork patch P5: every List sits in a ScrollView.
            ScrollView {
                List(items, selection: selection) { item in
                    Text(item.title).lineLimit(1)
                }
                .accessibleRowNames(items.map(\.title))
            }
            .frame(height: Double(44 * items.count))
            SectionHeader(CrossStrings.tools, style: .footnoteMedium)
            VStack(alignment: .leading, spacing: Int(DashSpacing.xxs)) {
                toolButton(CrossStrings.addressBook, .addressBook(.send)) {
                    if case .addressBook = $0 { true } else { false }
                }
                toolButton(CrossStrings.signVerify, .signVerify)
                toolButton(CrossStrings.toolsWindow, .tools(.information)) {
                    if case .tools = $0 { true } else { false }
                }
                toolButton(L10n.PSBT.dialogTitle, .psbt)
                toolButton(CrossStrings.walletsPage, .wallets)
                toolButton(CrossStrings.optionsPage, .options)
                toolButton(CrossStrings.securityPage, .security)
                toolButton(CrossStrings.settings, .settings)
                toolButton(CrossStrings.aboutPage, .about)
            }
            if main.lockState == .unlocked || main.lockState == .unlockedMixingOnly {
                DashButton(CrossStrings.lockWallet, style: .plainBlue, size: .small) {
                    Task { await state.perform(.lockWallet) }
                }
            }
            Spacer()
        }
        .padding(Int(DashSpacing.m))
    }

    private func toolButton(
        _ title: String, _ page: ToolPage, matches: ((ToolPage) -> Bool)? = nil
    ) -> some View {
        let state = state
        let active = state.currentPage.map { matches?($0) ?? ($0 == page) } ?? false
        return DashButton(title, style: active ? .tintedBlue : .plainBlue, size: .small) {
            state.open(page)
        }
    }
}

/// Above every page: the shell's question (Close wallet / Close all
/// wallets), its error, the node warning banner (QT-040) and the outcome of
/// the last copy action.
struct ShellBanners: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let shell = state.shell
        let empty = shell.confirmation == nil && shell.errorMessage == nil && state.information.bannerText == nil
            && state.copyMessage == nil
        if !empty {
            banners(state, shell)
        }
    }

    private func banners(_ state: CrossAppState, _ shell: ShellModel) -> some View {
        VStack(alignment: .leading, spacing: Int(DashSpacing.xs)) {
            if let confirmation = shell.confirmation {
                ConfirmationCard(
                    title: confirmation.title, message: confirmation.message, confirmTitle: CrossStrings.yes,
                    onConfirm: { Task { await shell.confirm() } }, onCancel: { shell.cancelConfirmation() })
            }
            if let error = shell.errorMessage {
                Toast(error, kind: .error)
            }
            if let banner = state.information.bannerText {
                Toast(banner, kind: .warning)
            }
            if let message = state.copyMessage {
                Toast(message, kind: .info, actionTitle: CrossStrings.dismiss) { state.copyMessage = nil }
            }
        }
        .padding(.horizontal, Int(DashSpacing.xl))
        .padding(.top, Int(DashSpacing.s))
    }
}

/// An inline Yes/Cancel question (dash-qt's message boxes).
struct ConfirmationCard: View {
    let title: String
    let message: String
    let confirmTitle: String
    var destructive = false
    let onConfirm: @MainActor @Sendable () -> Void
    let onCancel: @MainActor @Sendable () -> Void

    var body: some View {
        DashCard {
            SectionHeader(title, style: .subheadMedium)
            Text(message).dashFont(.footnote).dashForeground(.primaryText)
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(confirmTitle, style: destructive ? .filledRed : .filledBlue, size: .small, action: onConfirm)
                DashButton(CrossStrings.cancel, style: .strokeGray, size: .small, action: onCancel)
            }
        }
    }
}

/// The bottom status row: sync, network, height, HD and lock state, notice,
/// then dash-qt's unit selector (QT-020), the peers button (QT-024) and,
/// while syncing, the sync details button (QT-027).
struct StatusRow: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let main = state.main
        let home = main.home
        let amounts = state.env.amounts
        var items: [StatusBarItem] = []
        if let notice = state.notice { items.append(StatusBarItem(id: "notice", text: notice)) }
        items.append(StatusBarItem(id: "network", text: Format.network(main.network)))
        if let sync = home?.sync, let height = sync.tipHeight {
            items.append(StatusBarItem(id: "height", text: "#\(height)", help: sync.tipDate.map { Format.date($0) }))
        }
        if state.shell.hdIconVisible {
            items.append(StatusBarItem(id: "hd", text: CrossStrings.hd, help: state.shell.hdTooltip))
        }
        items.append(StatusBarItem(
            id: "lock", text: Format.lockState(main.lockState), help: state.shell.lockIcon?.tooltip))
        let progress = home?.sync.flatMap { $0.isDone ? nil : $0.progress }
        return StatusBarView(syncText: home?.syncText ?? L10n.Home.notConnected, progress: progress, items: items) {
            if let sync = home?.sync {
                if !sync.isDone {
                    DashButton(CrossStrings.syncDetails, style: .plainBlue, size: .small, help: L10n.SyncOverlay.show) {
                        main.syncOverlayRequested = true
                    }
                }
                DashButton(
                    "\(sync.connectedPeers) \(CrossStrings.peers)", style: .plainBlue, size: .small,
                    help: L10n.Peers.show
                ) {
                    state.open(.tools(.peers))
                }
            }
            if main.network != nil {
                DashPicker(
                    nil, accessibleName: CrossStrings.unit,
                    options: DisplayUnit.allCases.map { PickerOption($0, amounts.unitName($0)) },
                    selection: bind({ main.settings.display.unit }, { main.settings.setUnit($0) }))
            }
        }
    }
}

struct CenteredMessage: View {
    let text: String
    let busy: Bool

    var body: some View {
        VStack(spacing: Int(DashSpacing.m)) {
            Spacer()
            if busy { ProgressView() }
            Text(text).dashFont(.callout).dashForeground(.primaryText)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}
