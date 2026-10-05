// Window content: onboarding, lock screen or the sidebar + page layout, with
// the lifecycle overlay and the status row (QT-011…014, QT-024, IOS-013, IOS-018).
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

public struct WalletRootView: View {
    let state: CrossAppState

    public init(state: CrossAppState) {
        self.state = state
    }

    public var body: some View {
        let main = state.main
        VStack(spacing: 0) {
            content(main)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            StatusRow(state: state)
        }
        .background(DashColor.primaryBackground.color)
        .preferredColorScheme(colorScheme(main.settings.theme))
        .task { await main.start() }
    }

    @ViewBuilder
    private func content(_ main: MainViewModel) -> some View {
        if main.showsTransitionOverlay {
            CenteredMessage(text: Format.transition(main.transition), busy: true)
        } else if main.wallets == nil {
            CenteredMessage(text: main.errorMessage ?? CrossStrings.loading, busy: main.errorMessage == nil)
        } else if main.needsOnboarding, let onboarding = main.onboarding {
            OnboardingScreen(model: onboarding)
        } else if main.showsLockScreen {
            LockScreen(model: main.lock)
        } else {
            MainSplitView(state: state)
        }
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

    @ViewBuilder
    private func detail(_ main: MainViewModel) -> some View {
        if let sheet = main.sheet {
            toolPage(ToolPage(sheet))
        } else {
            switch main.selection {
            case .overview:
                if let home = main.home { OverviewScreen(model: home, state: state) }
            case .send:
                if let send = main.send { SendScreen(model: send, state: state) }
            case .receive:
                if let receive = main.receive { ReceiveScreen(model: receive) }
            case .transactions:
                if let transactions = main.transactions { TransactionsScreen(model: transactions, state: state) }
            case .coinJoin, .masternodes, .governance, .contacts, .explore:
                Page(main.selection.title) { Text(L10n.Common.notAvailableYet) }
            }
        }
    }

    @ViewBuilder
    private func toolPage(_ page: ToolPage) -> some View {
        switch page {
        case .addressBook(let purpose):
            if let model = state.addressBook(purpose: purpose) { AddressBookScreen(model: model) }
        case .signVerify:
            if let model = state.signVerify() { SignVerifyScreen(model: model) }
        case .settings:
            SettingsScreen(model: state.main.settings, state: state)
        }
    }
}

struct Sidebar: View {
    let state: CrossAppState

    var body: some View {
        let main = state.main
        let items = main.visibleSidebarItems
        let selection = bind(
            { main.sheet == nil ? Optional(main.selection.id) : nil },
            { (id: String?) in
                guard let id, let item = SidebarItem(rawValue: id) else { return }
                main.sheet = nil
                main.selection = item
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
            }
            .frame(height: Double(44 * items.count))
            SectionHeader(CrossStrings.tools, style: .footnoteMedium)
            toolButton(CrossStrings.addressBook, .sendingAddresses)
            toolButton(CrossStrings.signVerify, .signMessage)
            toolButton(CrossStrings.settings, .settings)
            if main.lockState == .unlocked || main.lockState == .unlockedMixingOnly {
                DashButton(CrossStrings.lockWallet, style: .plainBlue, size: .small) {
                    Task { await main.lock.lock() }
                }
            }
            Spacer()
        }
        .padding(Int(DashSpacing.m))
    }

    private func toolButton(_ title: String, _ sheet: SheetRoute) -> some View {
        let main = state.main
        let active = main.sheet.map { ToolPage($0) } == ToolPage(sheet)
        return DashButton(title, style: active ? .tintedBlue : .plainBlue, size: .small) {
            main.sheet = sheet
        }
    }
}

/// The bottom status row: sync, network, peers, lock state, notice.
struct StatusRow: View {
    let state: CrossAppState

    var body: some View {
        let main = state.main
        let home = main.home
        var items: [StatusBarItem] = []
        if let notice = state.notice { items.append(StatusBarItem(id: "notice", text: notice)) }
        items.append(StatusBarItem(id: "network", text: Format.network(main.network)))
        if let sync = home?.sync {
            items.append(StatusBarItem(id: "peers", text: "\(sync.connectedPeers) \(CrossStrings.peers)"))
            if let height = sync.tipHeight {
                items.append(
                    StatusBarItem(id: "height", text: "#\(height)", help: sync.tipDate.map { Format.date($0) }))
            }
        }
        items.append(StatusBarItem(id: "lock", text: Format.lockState(main.lockState)))
        let progress = home?.sync.flatMap { $0.isDone ? nil : $0.progress }
        return StatusBarView(syncText: home?.syncText ?? L10n.Home.notConnected, progress: progress, items: items)
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
