// App shell screens: the splash and corrupt-settings question (QT-005,
// QT-007), the data-directory chooser (QT-004), the shutdown window
// (QT-008), remembered window geometry (QT-011), About and the
// command-line help (QT-153, IOS-107), File ▸ Create Wallet, Settings ▸
// Unlock Wallet, File ▸ Backup Wallet and the close-wallet questions.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

// MARK: Window geometry (QT-011)

/// Restores and saves `window`'s frame through the shell's settings.
struct WindowGeometryKeeper: View {
    let model: MacAppModel
    let window: ShellWindow?

    var body: some View {
        if let window, let shell = model.shell {
            WindowFrameAccessor(initialFrame: shell.geometry(for: window).map(Self.frame)) { frame in
                shell.saveGeometry(
                    WindowGeometry(x: frame.x, y: frame.y, width: frame.width, height: frame.height), for: window)
            }
            .frame(width: 0, height: 0)
        }
    }

    static func frame(_ geometry: WindowGeometry) -> WindowFrame {
        WindowFrame(x: geometry.x, y: geometry.y, width: geometry.width, height: geometry.height)
    }
}

// MARK: Splash and corrupt settings (QT-005, QT-007)

/// Over the main window while the network opens: dash-qt's splash with the
/// phase text and a bar that never moves back; Q quits. The corrupt-settings
/// question comes first when a settings file had to be set aside.
struct StartupOverlay: View {
    let startup: StartupViewModel

    var body: some View {
        Group {
            switch startup.stage {
            case .starting where startup.showsSplash:
                SplashView(startup: startup)
                    .transition(.opacity)
            case .settingsUnreadable(let files):
                SettingsUnreadableView(startup: startup, files: files)
            case .quitting:
                Color.clear.onAppear { MacApplication.terminate() }
            default:
                EmptyView()
            }
        }
        .animation(.easeOut(duration: 0.25), value: startup.stage)
    }
}

struct SplashView: View {
    let startup: StartupViewModel

    var body: some View {
        ZStack {
            Color.dash.primaryBackground.ignoresSafeArea()
            VStack(spacing: DashSpacing.l) {
                Image(systemName: "d.circle.fill")
                    .font(.system(size: 72))
                    .foregroundStyle(Color.dash.blue)
                    .accessibilityHidden(true)
                Text(L10n.Navigation.appName).dashFont(.title1)
                ProgressView(value: startup.progress)
                    .frame(width: 280)
                    .accessibilityIdentifier("splash.progress")
                Text(startup.statusText)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
                    .accessibilityIdentifier("splash.status")
                Text(L10n.Shell.pressQToQuit)
                    .dashFont(.caption1)
                    .foregroundStyle(Color.dash.tertiaryText)
            }
        }
        .focusable()
        .onKeyPress("q") {
            startup.quit()
            return .handled
        }
        .accessibilityIdentifier("splash")
    }
}

/// dash-qt `InitSettings`: Reset (defaults; the old files stay as `.bak`)
/// or Abort (quit without writing).
struct SettingsUnreadableView: View {
    let startup: StartupViewModel
    let files: [URL]

    var body: some View {
        ZStack {
            Color.dash.backgroundOverlay.ignoresSafeArea()
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                Text(L10n.Shell.settingsUnreadable).dashFont(.title3)
                Text(L10n.Shell.settingsResetQuestion).dashFont(.body)
                ForEach(files, id: \.self) { file in
                    Text(file.path)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .foregroundStyle(Color.dash.secondaryText)
                }
                HStack {
                    Spacer()
                    Button(MacStrings.Shell.abort) { startup.abortSettings() }
                        .keyboardShortcut(.cancelAction)
                        .accessibilityIdentifier("settingsUnreadable.abort")
                    Button(MacStrings.Shell.reset) { startup.resetSettings() }
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier("settingsUnreadable.reset")
                }
            }
            .padding(DashSpacing.xl)
            .frame(width: 460)
            .background(RoundedRectangle(cornerRadius: DashRadius.card).fill(Color.dash.secondaryBackground))
        }
        .accessibilityIdentifier("settingsUnreadable")
    }
}

// MARK: Data directory chooser (QT-004)

struct DataDirectoryChooserView: View {
    let model: MacAppModel
    let chooser: DataDirectoryChooserViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.l) {
            Text(L10n.Shell.welcome).dashFont(.title2)
            Text(L10n.Shell.welcomeDetail).dashFont(.body)
            Picker("", selection: Binding(
                get: { chooser.choice },
                set: { choice in Task { await chooser.choose(choice) } }
            )) {
                Text(L10n.Shell.useDefaultDirectory).tag(DataDirectoryChooserViewModel.Choice.defaultDirectory)
                Text(L10n.Shell.useCustomDirectory).tag(DataDirectoryChooserViewModel.Choice.custom)
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
            .accessibilityIdentifier("datadir.choice")
            HStack {
                Text(chooser.selectedDirectory?.path ?? chooser.defaultDirectory.path)
                    .font(.system(.footnote, design: .monospaced))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(DashSpacing.s)
                    .background(RoundedRectangle(cornerRadius: DashRadius.standard).fill(Color.dash.secondaryBackground))
                    .accessibilityIdentifier("datadir.path")
                Button(MacStrings.Common.choose) {
                    Task {
                        if let url = await MacOpenPanel.chooseDirectory(title: L10n.Shell.useCustomDirectory) {
                            await chooser.setCustomDirectory(url)
                        }
                    }
                }
                .disabled(chooser.choice != .custom)
            }
            if let status = chooser.statusText {
                Text(status)
                    .dashFont(.footnote)
                    .foregroundStyle(chooser.canAccept ? Color.dash.secondaryText : Color.dash.errorText)
                    .accessibilityIdentifier("datadir.status")
            }
            if let space = chooser.freeSpaceText {
                Text(space).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
            }
            if let error = chooser.errorMessage {
                SystemNotice(text: error, tone: .error)
            }
            Spacer()
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel) {
                    chooser.cancel()
                    MacApplication.terminate()
                }
                .keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok) {
                    Task {
                        await chooser.accept()
                        if case .accepted(let directory) = chooser.outcome {
                            await model.acceptDataDirectory(directory) { url in
                                UserDefaults.standard.set(url.path, forKey: MacAppComposition.dataDirectoryDefaultsKey)
                            }
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!chooser.canAccept)
                .accessibilityIdentifier("datadir.ok")
            }
        }
        .padding(DashSpacing.xxl)
        .frame(minWidth: 640, minHeight: 420)
        .task { await chooser.load() }
        .accessibilityIdentifier("datadir")
    }
}

// MARK: Shutdown (QT-008)

/// "Dash Wallet is shutting down…": open while the engine stops.
struct ShutdownWindow: View {
    let model: MacAppModel

    var body: some View {
        if let shutdown = model.features?.shutdown {
            ShutdownView(shutdown: shutdown)
        }
    }
}

struct ShutdownView: View {
    let shutdown: ShutdownViewModel

    var body: some View {
        VStack(spacing: DashSpacing.m) {
            ProgressView().controlSize(.large)
            Text(shutdown.title).dashFont(.headline)
            Text(shutdown.message)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
                .multilineTextAlignment(.center)
        }
        .padding(DashSpacing.xxl)
        .frame(width: 380)
        .interactiveDismissDisabled(!shutdown.canClose)
        .accessibilityIdentifier("shutdown")
    }
}

// MARK: About and command-line help (QT-153, IOS-107)

struct AboutWindow: View {
    let model: MacAppModel

    var body: some View {
        if let about = model.features?.about {
            AboutView(about: about)
        } else {
            VStack(spacing: DashSpacing.s) {
                Text(L10n.HomeM2.aboutTitle).dashFont(.title2)
                Text(MacApplication.versionString).foregroundStyle(Color.dash.secondaryText)
                Text(L10n.HomeM2.license).dashFont(.footnote).frame(width: 340)
            }
            .padding(DashSpacing.xxl)
        }
    }
}

struct AboutView: View {
    let about: AboutViewModel
    @State private var exportMessage: String?

    var body: some View {
        VStack(spacing: DashSpacing.m) {
            Image(systemName: "d.circle.fill")
                .font(.system(size: 56))
                .foregroundStyle(Color.dash.blue)
                .accessibilityHidden(true)
            Text(L10n.Navigation.appName).dashFont(.title1)
            Text(about.versionText ?? L10n.HomeM2.version(MacApplication.versionString))
                .foregroundStyle(Color.dash.secondaryText)
                .accessibilityIdentifier("about.version")
            Grid(alignment: .leading, horizontalSpacing: DashSpacing.m, verticalSpacing: DashSpacing.xs) {
                GridRow {
                    Text(L10n.HomeM2.network).foregroundStyle(Color.dash.secondaryText)
                    Text(about.networkName ?? L10n.Tools.none)
                }
                GridRow {
                    Text(L10n.HomeM2.dataDirectory).foregroundStyle(Color.dash.secondaryText)
                    Text(about.dataDirectory?.path ?? L10n.Tools.none)
                        .textSelection(.enabled)
                        .lineLimit(2)
                        .truncationMode(.middle)
                }
            }
            .dashFont(.footnote)
            .frame(width: 400, alignment: .leading)
            HStack(spacing: DashSpacing.m) {
                Link(L10n.HomeM2.github, destination: AboutViewModel.githubURL)
                Link(L10n.HomeM2.support, destination: AboutViewModel.supportURL)
                Button(L10n.HomeM2.exportLogs) { Task { await exportLogs() } }
                    .disabled(about.logExport == .exporting)
                    .accessibilityIdentifier("about.exportLogs")
            }
            if let message = exportMessage ?? about.errorMessage {
                Text(message).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
            }
            Text(about.licenseText)
                .dashFont(.caption1)
                .foregroundStyle(Color.dash.secondaryText)
                .multilineTextAlignment(.center)
                .frame(width: 400)
        }
        .padding(DashSpacing.xxl)
        .task { await about.load() }
        .accessibilityIdentifier("about")
    }

    private func exportLogs() async {
        guard let url = await MacSavePanel.chooseDestination(
            suggestedName: MacStrings.About.logsFileName, title: L10n.HomeM2.exportLogs)
        else { return }
        await about.exportLogs(to: url)
        switch about.logExport {
        case .exported(let file): exportMessage = "\(L10n.HomeM2.logsExported) \(file.lastPathComponent)"
        case .failed(let reason): exportMessage = reason
        case .idle, .exporting: exportMessage = nil
        }
    }
}

struct CommandLineOptionsWindow: View {
    let model: MacAppModel

    var body: some View {
        if let about = model.features?.about {
            CommandLineOptionsView(about: about)
        }
    }
}

/// Help ▸ Command-line options (dash-qt `HelpMessageDialog`).
struct CommandLineOptionsView: View {
    let about: AboutViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text("\(L10n.Navigation.appName) \(about.versionText ?? MacApplication.versionString)").dashFont(.headline)
            Text(L10n.HomeM2.commandLineUsage)
                .font(.system(.footnote, design: .monospaced))
            Divider()
            ScrollView {
                Grid(alignment: .leading, horizontalSpacing: DashSpacing.l, verticalSpacing: DashSpacing.s) {
                    ForEach(about.commandLineOptions) { option in
                        GridRow(alignment: .firstTextBaseline) {
                            Text(option.name).font(.system(.footnote, design: .monospaced))
                            Text(option.text).dashFont(.footnote)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(height: 280)
        }
        .padding(DashSpacing.xl)
        .frame(width: 560)
        .accessibilityIdentifier("commandLine")
    }
}

// MARK: File ▸ Create Wallet

/// The onboarding flow over the existing vault: a second wallet.
struct AddWalletWindow: View {
    let model: MacAppModel
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        Group {
            if let onboarding = model.addWallet {
                OnboardingView(model: onboarding)
                    .onChange(of: onboarding.step) { _, step in
                        if case .done = step {
                            model.addWalletClosed()
                            dismissWindow(id: SceneID.addWallet)
                        }
                    }
            } else {
                Text(L10n.Shell.createWalletTip).padding(DashSpacing.xxl)
            }
        }
        .onDisappear { model.addWalletClosed() }
    }
}

// MARK: Settings ▸ Unlock Wallet (QT-016)

struct UnlockSheet: View {
    let lock: LockViewModel
    let onClose: () -> Void
    @State private var passphrase = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(L10n.Shell.unlockWallet.replacingOccurrences(of: "…", with: "")).dashFont(.title3)
            Text(L10n.Lock.prompt).dashFont(.subhead)
            SecureField(MacStrings.Common.passphrase, text: $passphrase)
                .textFieldStyle(.roundedBorder)
                .onSubmit(unlock)
                .accessibilityIdentifier("unlock.passphrase")
            if let message = lock.message {
                Text(message).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.ok, action: unlock)
                    .keyboardShortcut(.defaultAction)
                    .disabled(passphrase.isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 420)
    }

    private func unlock() {
        let text = passphrase
        passphrase = ""
        Task {
            await lock.unlock(passphrase: text, mixingOnly: false)
            if lock.lockState == .unlocked { onClose() }
        }
    }
}

// MARK: File ▸ Backup Wallet (QT-110)

/// An optional passphrase for the `.dwbackup` file, then the destination.
struct BackupWalletSheet: View {
    let wallets: WalletManagementViewModel
    let walletID: WalletID?
    let walletName: String
    let onClose: () -> Void
    @State private var passphrase = ""
    @State private var confirmation = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(L10n.Shell.backupWallet.replacingOccurrences(of: "…", with: "")).dashFont(.title3)
            Text(walletName).dashFont(.subheadMedium)
            Text(MacStrings.Wallets.backupPassphraseHelp).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
            PassphraseField(label: MacStrings.Wallets.backupPassphrase, text: $passphrase)
            PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
            if !passphrase.isEmpty, passphrase != confirmation {
                Text(MacStrings.Wallets.passphrasesDiffer).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.saveEllipsis) {
                    Task { await backup() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(walletID == nil || passphrase != confirmation)
                .accessibilityIdentifier("backup.save")
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 440)
    }

    private func backup() async {
        guard let walletID,
            let url = await MacSavePanel.chooseDestination(
                suggestedName: "\(walletName.isEmpty ? "wallet" : walletName).dwbackup",
                title: L10n.Shell.backupWalletTip)
        else { return }
        let secret = passphrase.isEmpty ? nil : passphrase
        passphrase = ""
        confirmation = ""
        onClose()
        await wallets.backup(walletID, to: url, passphrase: secret)
    }
}
#endif
