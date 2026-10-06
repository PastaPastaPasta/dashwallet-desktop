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
    /// The system's appearance, used when the Theme setting is System.
    @Environment(\.colorScheme) var systemColorScheme

    public init(state: CrossAppState) {
        self.state = state
    }

    public var body: some View {
        let state = state
        let main = state.main
        let scheme = colorScheme(state.appearanceOverride ?? main.settings.theme)
        VStack(spacing: 0) {
            if showsMainWindow(main) {
                ShellBanners(state: state)
            }
            content(main)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let message = state.copyMessage {
                CopyToast(state: state, message: message)
            }
            StatusRow(state: state)
        }
        .background(CrossRole.canvas.color)
        .toolkitThemeFromEnvironment()
        // GtkBackend ignores `preferredColorScheme` (no window override), so
        // the Theme setting reaches the views through the environment too.
        .environment(\.colorScheme, scheme ?? systemColorScheme)
        .preferredColorScheme(scheme)
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
            LockScreen(model: main.lock, network: main.network)
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
                .frame(minWidth: CrossLayout.sidebarWidth - 20)
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
                if let receive = main.receive { ReceiveScreen(model: receive, state: state) }
            case .transactions:
                if let transactions = state.transactions() { TransactionsScreen(model: transactions, state: state) }
            case .coinJoin, .masternodes, .governance, .contacts, .explore:
                Page(main.selection.title) {
                    DashCard { EmptyState(icon: Sidebar.icon(main.selection), title: L10n.Common.notAvailableYet) }
                }
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

/// The sidebar (C1): the Dash wordmark, the wallet selector (two or more
/// wallets), the sections as a selectable list (PNG icon + title; the
/// selected row is the accent pill with white content), then "More" with
/// the tool pages as rows of the same style (UX-SPEC §4.1, Cross only).
struct Sidebar: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let main = state.main
        let items = state.shell.sections
        let current: SidebarItem? = state.currentPage == nil ? main.selection : nil
        let selection = bind(
            { state.currentPage == nil ? Optional(main.selection.id) : nil },
            { (id: String?) in
                guard let id, let item = SidebarItem(rawValue: id) else { return }
                state.closePage()
                main.selection = item
                Task { await state.shell.perform(.section(item)) }
            })
        VStack(alignment: .leading, spacing: Int(DashSpacing.s)) {
            HStack {
                DashIcon(main.network == .mainnet || main.network == nil ? .dashLogo : .dashLogoTestnet, size: 22)
                Spacer()
            }
            .padding(.horizontal, Int(DashSpacing.s))
            .padding(.vertical, Int(DashSpacing.s))
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
                    SidebarRowLabel(
                        title: item.title, icon: Self.icon(item), template: true, selected: item == current,
                        flipped: item == .send)
                }
                .accessibleRowNames(items.map(\.title))
                .listCSSClass("dwd-sidebar")
            }
            .frame(height: Double(Self.rowHeight * items.count + 4))
            Text(CrossStrings.more.uppercased())
                .dashFont(.caption1Medium)
                .dashForeground(CrossRole.textTertiary)
                .padding(.horizontal, Int(DashSpacing.sm))
                .padding(.top, Int(DashSpacing.s))
            VStack(alignment: .leading, spacing: Int(DashSpacing.xxxs)) {
                toolRow(CrossStrings.addressBook, .addressBook, .addressBook(.send)) {
                    if case .addressBook = $0 { true } else { false }
                }
                toolRow(CrossStrings.signVerify, .clipboard, .signVerify)
                toolRow(CrossStrings.toolsWindow, .tools, .tools(.information)) {
                    if case .tools = $0 { true } else { false }
                }
                toolRow(L10n.PSBT.dialogTitle, .file, .psbt)
                toolRow(CrossStrings.walletsPage, .wallet, .wallets)
                toolRow(CrossStrings.optionsPage, .settings, .options)
                toolRow(CrossStrings.securityPage, .security, .security)
                toolRow(CrossStrings.settings, .appearance, .settings)
                toolRow(CrossStrings.aboutPage, .about, .about)
            }
            Spacer()
            if main.lockState == .unlocked || main.lockState == .unlockedMixingOnly {
                DashButton(CrossStrings.lockWallet, style: .tintedGray, size: .small) {
                    Task { await state.perform(.lockWallet) }
                }
            }
        }
        .padding(Int(DashSpacing.m))
        .frame(maxHeight: .infinity, alignment: .topLeading)
        .background(CrossRole.sidebar.color)
    }

    /// Row content height: 20 pt icon row plus 2 × 8 pt padding, and GTK's row spacing.
    static let rowHeight = 38

    /// Section icons (UX-SPEC §2.7; Cross uses the exported PNGs, tinted).
    /// Send is the down arrow drawn upside down.
    static func icon(_ item: SidebarItem) -> DashIconToken {
        switch item {
        case .overview: .tabHome
        case .send: .arrowDown
        case .receive: .arrowDown
        case .transactions: .votingList
        case .coinJoin: .coinjoinShuffle
        case .masternodes: .tabMore
        case .governance: .voting
        case .contacts: .tabContacts
        case .explore: .tabExplore
        }
    }

    private func toolRow(
        _ title: String, _ icon: DashIconToken, _ page: ToolPage, matches: ((ToolPage) -> Bool)? = nil
    ) -> some View {
        let state = state
        let active = state.currentPage.map { matches?($0) ?? ($0 == page) } ?? false
        return SidebarButtonRow(title: title, icon: icon, selected: active) { state.open(page) }
    }
}

/// The content of a sidebar row: 20 pt icon and `subhead` title; white on
/// the accent pill when selected. `template` icons are tinted (accent, or
/// white when selected); the filled `settings-*` tiles keep their colours.
struct SidebarRowLabel: View {
    let title: String
    let icon: DashIconToken
    let template: Bool
    let selected: Bool
    var flipped = false

    var body: some View {
        let tint: IconTint = template ? .color(selected ? CrossRole.white : CrossRole.accent) : .original
        HStack(spacing: Int(DashSpacing.m)) {
            DashIcon(icon, size: 20, width: 20, tint: tint, flipped: flipped)
            Text(title)
                .dashFont(selected ? .subheadMedium : .subhead)
                .dashForeground(selected ? CrossRole.white : CrossRole.textPrimary)
                .lineLimit(1)
            Spacer()
        }
        .padding(.horizontal, Int(DashSpacing.sm))
        .padding(.vertical, Int(DashSpacing.s))
    }
}

/// A "More" row: a button that looks like a sidebar row (hover tint,
/// accent pill when its page is open). Its title is its accessible name.
struct SidebarButtonRow: View {
    let title: String
    let icon: DashIconToken
    let selected: Bool
    let action: @MainActor @Sendable () -> Void

    @State var hovering = false

    var body: some View {
        Button(action: action) {
            SidebarRowLabel(title: title, icon: icon, template: false, selected: selected)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            if selected {
                RoundedRectangle(cornerRadius: Double(CrossLayout.sidebarRowRadius)).fill(CrossRole.accent.color)
            } else if hovering {
                RoundedRectangle(cornerRadius: Double(CrossLayout.sidebarRowRadius)).fill(CrossRole.accentTint.color)
            }
        }
        .onHover { hovering = $0 }
    }
}

/// Above every page: the shell's question (Close wallet / Close all
/// wallets), its error and the node warning banner (QT-040).
struct ShellBanners: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let shell = state.shell
        let empty = shell.confirmation == nil && shell.errorMessage == nil && state.information.bannerText == nil
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
        }
        .padding(.horizontal, CrossLayout.pagePaddingH)
        .padding(.vertical, Int(DashSpacing.s))
    }
}

/// The outcome of the last copy action as the bottom toast (C21): "…
/// copied" disappears after 3 s; the no-clipboard text (which carries the
/// value to copy by hand) stays until dismissed.
struct CopyToast: View {
    let state: CrossAppState
    let message: String

    var body: some View {
        let state = state
        let copied = state.capabilities.clipboard
        HStack {
            Spacer()
            ToastPill(message, icon: copied ? .toastCopied : .toastWarning, actionTitle: CrossStrings.dismiss) {
                state.copyMessage = nil
            }
            Spacer()
        }
        .padding(.vertical, Int(DashSpacing.s))
        .task(id: message) {
            guard copied else { return }
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            if state.copyMessage == message { state.copyMessage = nil }
        }
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
            SectionHeader(title, style: .headline)
            Text(message).dashFont(.subhead).dashForeground(CrossRole.textSecondary)
            HStack(spacing: Int(DashSpacing.s)) {
                Spacer()
                DashButton(CrossStrings.cancel, style: .tintedGray, size: .small, action: onCancel)
                DashButton(confirmTitle, style: destructive ? .filledRed : .filledBlue, size: .small, action: onConfirm)
            }
        }
    }
}

/// The bottom status row (C3, UX-SPEC §4.1): the demo badge, sync text and
/// bar on the left; on the right, in dash-qt's order, the unit selector
/// (QT-020), HD (QT-021), the lock state (QT-022), the peers button
/// (QT-024) and the sync state, with Sync details while syncing (QT-027).
/// The network is in the window title and the hero capsule, not here.
struct StatusRow: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let main = state.main
        let home = main.home
        let amounts = state.env.amounts
        var items: [StatusBarItem] = []
        if state.shell.hdIconVisible {
            items.append(StatusBarItem(id: "hd", text: CrossStrings.hd, help: state.shell.hdTooltip, tone: .success))
        }
        if let lock = state.shell.lockIcon {
            let tone: BadgeTone =
                switch lock {
                case .locked: .success
                case .unlocked: .danger
                case .unlockedMixingOnly: .warning
                }
            items.append(StatusBarItem(id: "lock", text: Format.lockState(main.lockState), help: lock.tooltip, tone: tone))
        } else if main.lockState == .unencrypted {
            // dash-qt's red open lock for an unencrypted wallet; no item without a vault or keys.
            items.append(StatusBarItem(id: "lock", text: Format.lockState(main.lockState), tone: .danger))
        }
        let progress = home?.sync.flatMap { $0.isDone ? nil : $0.progress }
        let tipHelp = home?.sync.flatMap { sync in sync.tipHeight.map { "#\($0) · \(Format.date(sync.tipDate))" } }
        return StatusBarView(
            syncText: home?.syncText ?? L10n.Home.notConnected, syncHelp: tipHelp, progress: progress, items: items
        ) {
            if let notice = state.notice {
                DashBadge(CrossStrings.demo, tone: .neutral, help: notice)
            }
        } accessory: {
            if main.network != nil {
                DashPicker(
                    nil, accessibleName: CrossStrings.unit,
                    options: DisplayUnit.allCases.map { PickerOption($0, amounts.unitName($0)) },
                    selection: bind({ main.settings.display.unit }, { main.settings.setUnit($0) }))
            }
            if let sync = home?.sync {
                DashButton(
                    "\(sync.connectedPeers) \(CrossStrings.peers)", style: .plainBlue, size: .extraSmall,
                    help: L10n.Peers.show
                ) {
                    state.open(.tools(.peers))
                }
                if sync.isDone {
                    StatusBarText(item: StatusBarItem(id: "sync", text: CrossStrings.synced, tone: .success))
                } else {
                    DashButton(CrossStrings.syncDetails, style: .plainBlue, size: .extraSmall, help: L10n.SyncOverlay.show) {
                        main.syncOverlayRequested = true
                    }
                }
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
            Text(text).dashFont(.subhead).dashForeground(CrossRole.textSecondary)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CrossRole.canvas.color)
    }
}
