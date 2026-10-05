// The scene graph of the macOS app: main window, address book, sign/verify,
// about, Settings and the menu bar companion. The app's `@main` builds a
// `MacAppModel` and returns `DashWalletScenes(model:)`.
#if os(macOS)
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
                .preferredColorScheme(model.colorScheme)
        }
        .defaultSize(width: 640, height: 460)

        Window(L10n.SignVerify.windowTitle, id: SceneID.signVerify) {
            SignVerifyWindow(model: model)
                .preferredColorScheme(model.colorScheme)
        }
        .defaultSize(width: 640, height: 520)

        Window(MacStrings.App.aboutTitle, id: SceneID.about) {
            AboutView()
                .preferredColorScheme(model.colorScheme)
        }
        .windowResizability(.contentSize)

        Settings {
            SettingsView(model: model)
                .preferredColorScheme(model.colorScheme)
        }

        MenuBarExtra(isInserted: $model.showsMenuBarExtra) {
            MenuBarContentView(model: model)
                .preferredColorScheme(model.colorScheme)
        } label: {
            Label(L10n.Navigation.appName, systemImage: "d.circle.fill")
        }
        .menuBarExtraStyle(.window)
    }
}

/// The main window's content: the wallet, or why there is none.
struct RootView: View {
    let model: MacAppModel

    var body: some View {
        Group {
            if let main = model.main {
                MainWindowView(model: model, main: main)
            } else {
                RuntimeUnavailableView(reason: model.unavailableReason ?? "")
            }
        }
        .preferredColorScheme(model.colorScheme)
        .navigationTitle(model.windowTitle)
        .onOpenURL { url in
            Task { await model.open(uri: url.absoluteString) }
        }
        .task { await model.start() }
    }
}

struct RuntimeUnavailableView: View {
    let reason: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 40))
                .foregroundStyle(.orange)
            Text(MacStrings.App.runtimeUnavailableTitle)
                .font(.title2.bold())
            Text(reason)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
                .foregroundStyle(.secondary)
            Text(MacStrings.App.openDemo)
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .padding(40)
        .frame(minWidth: 640, minHeight: 420)
        .accessibilityIdentifier("runtime.unavailable")
    }
}

struct AboutView: View {
    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "d.circle.fill")
                .font(.system(size: 56))
                .foregroundStyle(Color.accentColor)
            Text(L10n.Navigation.appName)
                .font(.title.bold())
            Text(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")
                .foregroundStyle(.secondary)
            Text(MacStrings.App.aboutBody)
                .multilineTextAlignment(.center)
                .frame(width: 320)
                .font(.callout)
        }
        .padding(28)
    }
}
#endif
