// The scene graph of the macOS app: the main window, dash-qt's secondary
// windows (address book, sign/verify, Options, Tools, Coin Selection, PSBT
// operations, wallets, command-line help, About, shutdown) and the menu bar
// companion. The app's `@main` builds a `MacAppModel` and returns
// `DashWalletScenes(model:)`.
#if os(macOS)
import DashUIMac
import DesignTokens
import SwiftUI
import WalletFeatures

public struct DashWalletScenes: Scene {
    @Bindable private var model: MacAppModel

    public init(model: MacAppModel) {
        self.model = model
    }

    public var body: some Scene {
        Window(L10n.Navigation.appName, id: SceneID.main) {
            RootView(model: model)
        }
        .defaultSize(width: 1120, height: 740)
        .commands { WalletCommands(model: model) }

        Window(MacStrings.AddressBook.windowTitle, id: SceneID.addressBook) {
            AddressBookWindow(model: model)
                .modifier(WindowChrome(model: model, geometry: .addressBook))
        }
        .defaultSize(width: 640, height: 460)

        Window(L10n.SignVerify.windowTitle, id: SceneID.signVerify) {
            SignVerifyWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .defaultSize(width: 640, height: 520)

        Window(L10n.HomeM2.aboutTitle, id: SceneID.about) {
            AboutWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .windowResizability(.contentSize)

        Window(L10n.Shell.options.replacingOccurrences(of: "…", with: ""), id: SceneID.options) {
            OptionsWindow(model: model)
                .modifier(WindowChrome(model: model, geometry: .options))
        }
        .windowResizability(.contentSize)

        Window(L10n.Tools.windowTitle, id: SceneID.tools) {
            ToolsWindow(model: model)
                .modifier(WindowChrome(model: model, geometry: .tools))
        }
        .defaultSize(width: 760, height: 540)

        Window(L10n.CoinControl.title, id: SceneID.coinControl) {
            CoinControlWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .defaultSize(width: 900, height: 560)

        Window(L10n.PSBT.dialogTitle, id: SceneID.psbt) {
            PSBTWindow(model: model)
                .modifier(WindowChrome(model: model, geometry: .psbt))
        }
        .defaultSize(width: 620, height: 460)

        Window(MacStrings.Wallets.windowTitle, id: SceneID.wallets) {
            WalletsWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .defaultSize(width: 720, height: 520)

        Window(L10n.Shell.createWallet.replacingOccurrences(of: "…", with: ""), id: SceneID.addWallet) {
            AddWalletWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .defaultSize(width: 900, height: 640)

        Window(L10n.HomeM2.commandLineTitle, id: SceneID.commandLine) {
            CommandLineOptionsWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .windowResizability(.contentSize)

        Window(L10n.Shell.shuttingDown, id: SceneID.shutdown) {
            ShutdownWindow(model: model)
                .modifier(WindowChrome(model: model))
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)

        MenuBarExtra(isInserted: $model.showsMenuBarExtra) {
            MenuBarContentView(model: model)
                .preferredColorScheme(model.colorScheme)
                .tint(Color.role.accent)
        } label: {
            Label {
                Text(L10n.Navigation.appName)
            } icon: {
                DashIconImage(.token(.dashCurrency), template: true)
            }
        }
        .menuBarExtraStyle(.window)
    }
}

/// Every window: the theme, the window opener for menu and Dock commands,
/// and (for the windows dash-qt remembers) the saved geometry (QT-011).
struct WindowChrome: ViewModifier {
    let model: MacAppModel
    var geometry: ShellWindow?
    @Environment(\.openWindow) private var openWindow

    func body(content: Content) -> some View {
        content
            .preferredColorScheme(model.colorScheme)
            // Dash blue for selection, focus rings, default buttons and toggles,
            // whatever the system accent (UX-SPEC §2.8).
            .tint(Color.role.accent)
            .background(WindowGeometryKeeper(model: model, window: geometry))
            .onAppear {
                if model.windowOpener == nil { model.windowOpener = openWindow }
            }
    }
}

/// The main window's content: the data-directory chooser, the wallet, or
/// why there is none; the splash and the corrupt-settings question over it.
struct RootView: View {
    let model: MacAppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Group {
            if let chooser = model.dataDirectoryChooser {
                DataDirectoryChooserView(model: model, chooser: chooser)
            } else if let main = model.main {
                MainWindowView(model: model, main: main)
            } else {
                RuntimeUnavailableView(reason: model.unavailableReason ?? "")
            }
        }
        .overlay {
            if let startup = model.features?.startup {
                StartupOverlay(startup: startup)
            }
        }
        .preferredColorScheme(model.colorScheme)
        .tint(Color.role.accent)
        .background(WindowGeometryKeeper(model: model, window: .main))
        .navigationTitle(model.windowTitle)
        .onOpenURL { url in
            Task { await model.open(uri: url.absoluteString) }
        }
        .onAppear { model.windowOpener = openWindow }
        .task { await model.start() }
    }
}

struct RuntimeUnavailableView: View {
    let reason: String

    var body: some View {
        VStack(spacing: DashSpacing.m) {
            DashIconImage(.token(.messageWarning))
                .scaledToFit()
                .frame(width: 48, height: 48)
                .accessibilityHidden(true)
            Text(MacStrings.App.runtimeUnavailableTitle)
                .dashFont(.title2)
                .foregroundStyle(Color.role.textPrimary)
            Text(reason)
                .dashFont(.subhead)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
                .foregroundStyle(Color.role.textSecondary)
            Text(MacStrings.App.openDemo)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textTertiary)
        }
        .frame(maxWidth: DashLayout.formMaxWidth)
        .dashCard(padding: DashSpacing.xxxl)
        .frame(minWidth: 640, minHeight: 420)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .dashCanvas()
        .accessibilityIdentifier("runtime.unavailable")
    }
}
#endif
