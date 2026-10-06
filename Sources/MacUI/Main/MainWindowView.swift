// Main window (DESIGN-opus §1.10): sidebar, page, toolbar and dash-qt
// status bar, with onboarding, lock screen and lifecycle overlays.
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures
import WalletRuntime

struct MainWindowView: View {
    @Bindable var model: MacAppModel
    @Bindable var main: MainViewModel

    var body: some View {
        ZStack {
            if main.needsOnboarding, let onboarding = main.onboarding {
                OnboardingView(model: onboarding)
                    .transition(.opacity)
            } else {
                walletView
            }
            if main.showsSyncOverlay, let status = main.home?.sync {
                SyncOverlayView(
                    status: status, rates: main.syncRates, syncText: main.home?.syncText ?? "",
                    hide: { main.hideSyncOverlay() })
                .transition(.opacity)
            }
            if main.showsLockScreen {
                LockScreenView(lock: main.lock, receive: main.receive, network: main.network)
                    .transition(.opacity)
            }
            if main.showsTransitionOverlay {
                TransitionOverlay(transition: main.transition)
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if let error = model.launchError {
                LaunchErrorBanner(error: error, retry: { Task { await model.retryLaunch() } })
            }
        }
        .frame(minWidth: DashLayout.windowMinWidth, minHeight: DashLayout.windowMinHeight)
        .animation(.easeInOut(duration: DashMotion.overlay), value: main.showsLockScreen)
        .sheet(item: Binding(get: { main.sheet.map(SheetItem.init) }, set: { main.sheet = $0?.route })) { item in
            SecuritySheet(
                route: item.route, settings: main.settings, screenCapture: model.env?.screenCapture,
                onClose: { main.sheet = nil })
        }
        .sheet(isPresented: Binding(get: { model.isOpenURIPresented }, set: { model.isOpenURIPresented = $0 })) {
            OpenURISheet(model: model)
        }
        .sheet(isPresented: Binding(get: { model.isPeersPresented }, set: { model.isPeersPresented = $0 })) {
            if let sync = model.env?.sync { PeersSheet(sync: sync) }
        }
        .sheet(isPresented: $model.isUnlockPresented) {
            UnlockSheet(lock: main.lock, onClose: { model.isUnlockPresented = false })
        }
        .sheet(isPresented: $model.isBackupPresented) {
            if let wallets = model.features?.wallets {
                BackupWalletSheet(
                    wallets: wallets, walletID: main.selectedWalletID,
                    walletName: main.wallets?.first { $0.id == main.selectedWalletID }?.name ?? "",
                    onClose: { model.isBackupPresented = false })
            }
        }
        .sheet(isPresented: existingDataBinding) {
            if let wallets = model.features?.wallets { ExistingDataSheet(wallets: wallets) }
        }
        .task(id: main.needsOnboarding) {
            // IOS-009: wallet data from an earlier install while no wallet is open.
            if main.needsOnboarding { await model.features?.wallets.loadExistingData() }
        }
        .modifier(ShellAlerts(model: model))
        .onChange(of: main.home?.sync) { _, status in
            if let status { main.syncRates.record(status) }
        }
        .alert(
            MacStrings.Common.error,
            isPresented: Binding(get: { model.uriError != nil }, set: { if !$0 { model.uriError = nil } })
        ) {
            Button(MacStrings.Common.ok) { model.uriError = nil }
        } message: {
            Text(model.uriError ?? "")
        }
        // Drop a dash: URI on the window to pay it (QT-019).
        .dropDestination(for: String.self) { items, _ in
            guard let text = items.first(where: { $0.lowercased().hasPrefix("dash:") }) else { return false }
            Task { await model.open(uri: text) }
            return true
        }
    }

    private var existingDataBinding: Binding<Bool> {
        Binding(
            get: { main.needsOnboarding && (model.features?.wallets.showsExistingDataPrompt ?? false) },
            set: { if !$0 { model.features?.wallets.keepExistingData() } })
    }

    private var walletView: some View {
        NavigationSplitView {
            Sidebar(main: main)
                .navigationSplitViewColumnWidth(
                    min: DashLayout.sidebarMinWidth, ideal: DashLayout.sidebarIdealWidth, max: DashLayout.sidebarMaxWidth)
        } detail: {
            page
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .dashCanvas()
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            WalletStatusBar(model: model, main: main)
        }
        .toolbar { MainToolbar(model: model, main: main) }
        .onChange(of: main.home?.route) { _, route in
            guard let route else { return }
            main.home?.route = nil
            Task { await main.navigate(route) }
        }
        .onChange(of: main.send?.route) { _, route in
            guard let route else { return }
            // After an unknown broadcast outcome the Send page explains it
            // first and follows the route itself.
            if case .broadcastUnknown = main.send?.phase { return }
            main.send?.route = nil
            Task {
                await main.navigate(route)
                await main.send?.dismiss()
            }
        }
    }

    @ViewBuilder
    private var page: some View {
        switch main.selection {
        case .overview:
            if let home = main.home {
                OverviewPage(model: model, main: main, home: home)
            } else {
                LoadingPage()
            }
        case .send:
            if let send = main.send { SendView(model: model, send: send) } else { LoadingPage() }
        case .receive:
            if let receive = main.receive { ReceiveView(receive: receive, unitName: model.unitName) } else { LoadingPage() }
        case .transactions:
            if let transactions = main.transactions {
                TransactionsView(
                    transactions: transactions, unitName: model.unitName, formatAmount: model.formatAmount,
                    unit: main.settings.display.unit)
            } else {
                LoadingPage()
            }
        case .coinJoin, .masternodes, .governance, .contacts, .explore:
            // Hidden in M1 (FeatureFlags.m1); not reachable from the sidebar.
            LoadingPage()
        }
    }
}

/// Overview with the M2 backup reminder and shortcut bar above it.
struct OverviewPage: View {
    let model: MacAppModel
    let main: MainViewModel
    let home: HomeViewModel
    @Environment(\.openURL) private var openURL

    var body: some View {
        OverviewView.make(model: model, main: main, home: home, perform: { route in model.perform(route, openURL: openURL) })
    }
}

extension OverviewView {
    /// The Overview wired to the app: shortcut card and backup reminder (M2
    /// services), unit menu, sync overlay and the Transactions page.
    @MainActor
    static func make(
        model: MacAppModel, main: MainViewModel, home: HomeViewModel, perform: @escaping (ShortcutRoute) -> Void
    ) -> OverviewView {
        let features = model.features
        return OverviewView(
            home: home,
            toggleDiscreet: { main.settings.setDiscreet(!main.settings.display.hideBalances) },
            unit: main.settings.display.unit,
            unitName: model.unitName,
            shortcuts: main.network.flatMap { features?.shortcutBar(for: $0) },
            reminder: features?.backupReminder,
            perform: perform,
            backUp: { model.perform(.showRecoveryPhrase) },
            setUnit: { main.settings.setUnit($0) },
            unitTitle: { model.env?.amounts.unitName($0) ?? "" },
            showSyncDetails: { main.syncOverlayRequested = true },
            seeAll: { main.selection = .transactions })
    }
}

/// The shell's questions (Close Wallet, Close All Wallets) and errors.
struct ShellAlerts: ViewModifier {
    let model: MacAppModel
    /// The shell keeps its last error until the next command; the alert
    /// shows each message once.
    @State private var dismissedError: String?

    func body(content: Content) -> some View {
        content
            .alert(
                model.shell?.confirmation?.title ?? "",
                isPresented: Binding(
                    get: { model.shell?.confirmation != nil }, set: { if !$0 { model.shell?.cancelConfirmation() } })
            ) {
                Button(MacStrings.Common.no, role: .cancel) { model.shell?.cancelConfirmation() }
                Button(MacStrings.Common.yes) { Task { await model.shell?.confirm() } }
                    .accessibilityIdentifier("shell.confirm.yes")
            } message: {
                Text(model.shell?.confirmation?.message ?? "")
            }
            .alert(
                MacStrings.Common.error,
                isPresented: Binding(
                    get: {
                        guard let error = model.shell?.errorMessage else { return false }
                        return error != dismissedError && model.shell?.confirmation == nil
                    },
                    set: { if !$0 { dismissedError = model.shell?.errorMessage } })
            ) {
                Button(MacStrings.Common.ok) { dismissedError = model.shell?.errorMessage }
            } message: {
                Text(model.shell?.errorMessage ?? "")
            }
    }
}

/// Opening the network failed: the engine's code and a retry button.
struct LaunchErrorBanner: View {
    let error: ServiceError
    let retry: () -> Void

    var body: some View {
        HStack(spacing: DashSpacing.m) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(Color.role.warning)
                .accessibilityHidden(true)
            Text(MacStrings.App.launchFailed(error.code.rawValue))
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textPrimary)
                .textSelection(.enabled)
            Spacer()
            Button(MacStrings.App.retry, action: retry)
                .controlSize(.small)
        }
        .padding(.horizontal, DashSpacing.l)
        .padding(.vertical, DashSpacing.s)
        .background(Color.role.warningTint)
        .accessibilityIdentifier("banner.launchError")
    }
}

/// `SheetRoute` as a sheet item.
struct SheetItem: Identifiable {
    let route: SheetRoute
    var id: SheetRoute { route }
}

struct LoadingPage: View {
    var body: some View {
        ProgressView()
            .controlSize(.small)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: Sidebar

struct Sidebar: View {
    @Bindable var main: MainViewModel

    var body: some View {
        List(selection: Binding<SidebarItem?>(get: { main.selection }, set: { if let item = $0 { main.selection = item } })) {
            ForEach(main.visibleSidebarItems) { item in
                SidebarRow(title: item.title, icon: Self.icon(item))
                    .tag(item)
                    .accessibilityIdentifier("sidebar.\(item.rawValue)")
                    .help(Self.shortcutHelp(item, main: main))
            }
        }
        .listStyle(.sidebar)
    }

    static func icon(_ item: SidebarItem) -> DashIconSource {
        switch item {
        case .overview: .system("house")
        case .send: .system("arrow.up.right")
        case .receive: .system("arrow.down.left")
        case .transactions: .system("list.bullet.rectangle")
        case .coinJoin: .system("shuffle")
        case .masternodes: .system("server.rack")
        case .governance: .system("checkmark.seal")
        case .contacts: .system("person.2")
        case .explore: .system("map")
        }
    }

    static func shortcutHelp(_ item: SidebarItem, main: MainViewModel) -> String {
        guard let number = item.shortcutNumber(with: main.features) else { return item.title }
        return "\(item.title) (⌘\(number))"
    }
}

// MARK: Toolbar

struct MainToolbar: ToolbarContent {
    let model: MacAppModel
    @Bindable var main: MainViewModel

    var body: some ToolbarContent {
        // The network and demo tags are in the window title only; the
        // status bar carries the Demo badge (UX-SPEC §4.1).
        ToolbarItemGroup(placement: .primaryAction) {
            // Wallet selector only with two or more wallets (QT-014).
            if main.showsWalletSelector, let wallets = main.wallets {
                Picker(MacStrings.Toolbar.wallet, selection: Binding(
                    get: { main.selectedWalletID },
                    set: { id in if let id { Task { await main.selectWallet(id) } } }
                )) {
                    ForEach(wallets) { wallet in
                        Text(wallet.name).tag(Optional(wallet.id))
                    }
                }
                .pickerStyle(.menu)
                .accessibilityIdentifier("toolbar.walletSelector")
            }
            let hidden = main.settings.display.hideBalances
            Button {
                main.settings.setDiscreet(!hidden)
            } label: {
                Label(
                    hidden ? MacStrings.Toolbar.showBalances : MacStrings.Toolbar.hideBalances,
                    systemImage: hidden ? "eye.slash" : "eye")
            }
            .help(hidden ? MacStrings.Toolbar.showBalances : MacStrings.Toolbar.hideBalances)
            .accessibilityIdentifier("toolbar.discreet")
            if main.lockState == .unlocked || main.lockState == .unlockedMixingOnly {
                Button {
                    Task { await main.lock.lock() }
                } label: {
                    Label(MacStrings.Toolbar.lock, systemImage: "lock")
                }
                .help(MacStrings.Toolbar.lock)
                .accessibilityIdentifier("toolbar.lock")
            }
        }
    }
}

// MARK: Overlays

struct TransitionOverlay: View {
    let transition: LifecycleTransition

    var body: some View {
        ZStack {
            Color.role.overlay.ignoresSafeArea()
            VStack(spacing: DashSpacing.m) {
                ProgressView()
                Text(Self.text(transition))
                    .dashFont(.subheadMedium)
                    .foregroundStyle(Color.role.textPrimary)
            }
            .padding(DashSpacing.xxl)
            .dashCard(padding: nil)
        }
        .accessibilityIdentifier("overlay.transition")
    }

    static func text(_ transition: LifecycleTransition) -> String {
        switch transition {
        case .idle: ""
        case .starting(let network): MacStrings.Transition.starting(L10n.Settings.networkName(network))
        case .stopping(let network): MacStrings.Transition.stopping(L10n.Settings.networkName(network))
        case .switchingNetwork(_, let to): MacStrings.Transition.switching(L10n.Settings.networkName(to))
        case .addingWallet: MacStrings.Transition.addingWallet
        case .removingWallet: MacStrings.Transition.removingWallet
        }
    }
}

/// File ▸ Open URI (QT-015).
struct OpenURISheet: View {
    let model: MacAppModel
    @State private var text = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Menu.openURIPrompt)
                .dashFont(.headline)
            TextField("dash:", text: $text)
                .textFieldStyle(.dash)
                .frame(width: 420)
                .accessibilityIdentifier("openURI.field")
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel) { dismiss() }
                    .buttonStyle(.dash(.tintedGray, .medium))
                    .keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok) {
                    let uri = text
                    dismiss()
                    Task { await model.open(uri: uri) }
                }
                .buttonStyle(.dash(.filledBlue, .medium))
                .keyboardShortcut(.defaultAction)
                .disabled(text.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(DashSpacing.xl)
    }
}
#endif
