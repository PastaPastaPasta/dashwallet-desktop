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
            Color.role.canvas.ignoresSafeArea()
            VStack(spacing: DashSpacing.l) {
                // The wordmark carries the name (testnet has its own wordmark).
                DashIconImage(.token(.dashLogo))
                    .scaledToFit()
                    .frame(height: 32)
                    .accessibilityLabel(L10n.Navigation.appName)
                DashProgressBar(value: startup.progress)
                    .frame(width: 240)
                    .accessibilityIdentifier("splash.progress")
                Text(startup.statusText)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .accessibilityIdentifier("splash.status")
                Text(L10n.Shell.pressQToQuit)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textTertiary)
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
            Color.role.overlay.ignoresSafeArea()
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                SystemMessageView(
                    title: L10n.Shell.settingsUnreadable, subtitle: L10n.Shell.settingsResetQuestion,
                    icon: .token(.messageWarning), backgroundColor: Color.role.dangerTint)
                ForEach(files, id: \.self) { file in
                    // A path: technical text.
                    Text(file.path)
                        .font(.system(size: DesignTokens.DashTextStyle.caption1.size, design: .monospaced))
                        .textSelection(.enabled)
                        .foregroundStyle(Color.role.textSecondary)
                }
                HStack {
                    Spacer()
                    Button(MacStrings.Shell.abort) { startup.abortSettings() }
                        .buttonStyle(.dash(.tintedGray, .medium))
                        .keyboardShortcut(.cancelAction)
                        .accessibilityIdentifier("settingsUnreadable.abort")
                    Button(MacStrings.Shell.reset) { startup.resetSettings() }
                        .buttonStyle(.dash(.filledBlue, .medium))
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier("settingsUnreadable.reset")
                }
            }
            .padding(DashSpacing.xl)
            .frame(width: DashLayout.sheetWidthSmall)
            .dashCard(padding: nil, elevation: .floating)
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
            DashIconImage(.token(.dashLogo))
                .scaledToFit()
                .frame(height: 28)
                .accessibilityLabel(L10n.Navigation.appName)
            PageTitle(title: L10n.Shell.welcome, subtitle: L10n.Shell.welcomeDetail)
            VStack(alignment: .leading, spacing: DashSpacing.m) {
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
                    // A path: technical text.
                    .font(.system(size: DesignTokens.DashTextStyle.footnote.size, design: .monospaced))
                    .foregroundStyle(Color.role.textPrimary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, DashSpacing.m)
                    .frame(height: 36)
                    .background(
                        RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous).fill(Color.role.fieldFill))
                    .accessibilityIdentifier("datadir.path")
                Button(MacStrings.Common.choose) {
                    Task {
                        if let url = await MacOpenPanel.chooseDirectory(title: L10n.Shell.useCustomDirectory) {
                            await chooser.setCustomDirectory(url)
                        }
                    }
                }
                .buttonStyle(.dash(.tintedBlue, .medium))
                .disabled(chooser.choice != .custom)
            }
            }
            .dashCard(padding: DashLayout.cardPadding, elevation: .menuCard)
            if let status = chooser.statusText {
                Text(status)
                    .dashFont(.footnote)
                    .foregroundStyle(chooser.canAccept ? Color.role.textSecondary : Color.role.danger)
                    .accessibilityIdentifier("datadir.status")
            }
            if let space = chooser.freeSpaceText {
                Text(space).dashFont(.footnote).foregroundStyle(Color.role.textSecondary)
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
                .buttonStyle(.dash(.tintedGray, .medium))
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
                .buttonStyle(.dash(.filledBlue, .medium))
                .keyboardShortcut(.defaultAction)
                .disabled(!chooser.canAccept)
                .accessibilityIdentifier("datadir.ok")
            }
        }
        .padding(DashSpacing.xxl)
        .frame(minWidth: 640, minHeight: 420)
        .dashCanvas()
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
            DashIconImage(.token(.dashLogo))
                .scaledToFit()
                .frame(height: 24)
                .accessibilityHidden(true)
            ProgressView().controlSize(.small)
            Text(shutdown.title)
                .dashFont(.subheadMedium)
                .foregroundStyle(Color.role.textPrimary)
            Text(shutdown.message)
                .dashFont(.footnote)
                .foregroundStyle(Color.role.textSecondary)
                .multilineTextAlignment(.center)
        }
        .padding(DashSpacing.xxl)
        .frame(width: 380)
        .dashCanvas()
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
                Text(MacApplication.versionString).foregroundStyle(Color.role.textSecondary)
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
            DashIconImage(.token(.dashLogo))
                .scaledToFit()
                .frame(height: 32)
                .accessibilityLabel(L10n.Navigation.appName)
            Text(about.versionText ?? L10n.HomeM2.version(MacApplication.versionString))
                .dashFont(.subhead)
                .foregroundStyle(Color.role.textSecondary)
                .accessibilityIdentifier("about.version")
            Grid(alignment: .leading, horizontalSpacing: DashSpacing.m, verticalSpacing: DashSpacing.xs) {
                GridRow {
                    Text(L10n.HomeM2.network).foregroundStyle(Color.role.textSecondary)
                    Text(about.networkName ?? L10n.Tools.none)
                }
                GridRow {
                    Text(L10n.HomeM2.dataDirectory).foregroundStyle(Color.role.textSecondary)
                    Text(about.dataDirectory?.path ?? L10n.Tools.none)
                        .textSelection(.enabled)
                        .lineLimit(2)
                        .truncationMode(.middle)
                }
            }
            .dashFont(.footnote)
            .foregroundStyle(Color.role.textPrimary)
            .frame(width: 400, alignment: .leading)
            .dashCard(padding: DashLayout.cardPadding)
            HStack(spacing: DashSpacing.s) {
                Link(L10n.HomeM2.github, destination: AboutViewModel.githubURL)
                    .buttonStyle(.dash(.plainBlue, .small))
                Link(L10n.HomeM2.support, destination: AboutViewModel.supportURL)
                    .buttonStyle(.dash(.plainBlue, .small))
                Button(L10n.HomeM2.exportLogs) { Task { await exportLogs() } }
                    .buttonStyle(.dash(.tintedBlue, .small))
                    .disabled(about.logExport == .exporting)
                    .accessibilityIdentifier("about.exportLogs")
            }
            if let message = exportMessage ?? about.errorMessage {
                Text(message).dashFont(.footnote).foregroundStyle(Color.role.textSecondary)
            }
            ScrollView {
                Text(about.licenseText)
                    .dashFont(.caption1)
                    .foregroundStyle(Color.role.textSecondary)
                    .multilineTextAlignment(.leading)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(width: 400, height: 90)
            .dashCard(radius: DashRadius.standard, padding: DashSpacing.m, elevation: nil, fill: Color.role.cardRaised)
        }
        .padding(DashSpacing.xxl)
        .dashCanvas()
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
            Text("\(L10n.Navigation.appName) \(about.versionText ?? MacApplication.versionString)")
                .dashFont(.headline)
                .foregroundStyle(Color.role.textPrimary)
            Text(L10n.HomeM2.commandLineUsage)
                .font(.system(size: DesignTokens.DashTextStyle.footnote.size, design: .monospaced))
                .foregroundStyle(Color.role.textSecondary)
            ScrollView {
                Grid(alignment: .leading, horizontalSpacing: DashSpacing.l, verticalSpacing: DashSpacing.s) {
                    ForEach(about.commandLineOptions) { option in
                        GridRow(alignment: .firstTextBaseline) {
                            Text(option.name)
                                .font(.system(size: DesignTokens.DashTextStyle.footnote.size, design: .monospaced))
                                .textSelection(.enabled)
                            Text(option.text).dashFont(.footnote)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .foregroundStyle(Color.role.textPrimary)
            }
            .frame(height: 280)
            .dashCard(padding: DashLayout.cardPadding)
        }
        .padding(DashSpacing.xl)
        .frame(width: 560)
        .dashCanvas()
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
        SheetScaffold(title: L10n.Shell.unlockWallet.replacingOccurrences(of: "…", with: ""), onClose: onClose) {
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                Text(L10n.Lock.prompt)
                    .dashFont(.subhead)
                    .foregroundStyle(Color.role.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                SecureField(MacStrings.Common.passphrase, text: $passphrase)
                    .modifier(DashFieldModifier(isError: lock.message != nil))
                    .onSubmit(unlock)
                    .accessibilityIdentifier("unlock.passphrase")
                if let message = lock.message {
                    Text(message).dashFont(.footnote).foregroundStyle(Color.role.danger)
                }
            }
        } footer: {
            Button(MacStrings.Common.cancel, action: onClose)
                .buttonStyle(.dash(.tintedGray, .large, fillsWidth: true))
                .keyboardShortcut(.cancelAction)
            Button(MacStrings.Common.ok, action: unlock)
                .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
                .keyboardShortcut(.defaultAction)
                .disabled(passphrase.isEmpty)
        }
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
        SheetScaffold(title: L10n.Shell.backupWallet.replacingOccurrences(of: "…", with: ""), onClose: onClose) {
            VStack(alignment: .leading, spacing: DashSpacing.m) {
                Text(walletName)
                    .dashFont(.subheadMedium)
                    .foregroundStyle(Color.role.textPrimary)
                Text(MacStrings.Wallets.backupPassphraseHelp)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                PassphraseField(label: MacStrings.Wallets.backupPassphrase, text: $passphrase)
                PassphraseField(label: MacStrings.Common.repeatPassphrase, text: $confirmation)
                if !passphrase.isEmpty, passphrase != confirmation {
                    Text(MacStrings.Wallets.passphrasesDiffer).dashFont(.footnote).foregroundStyle(Color.role.danger)
                }
            }
        } footer: {
            Button(MacStrings.Common.cancel, action: onClose)
                .buttonStyle(.dash(.tintedGray, .large, fillsWidth: true))
                .keyboardShortcut(.cancelAction)
            Button(MacStrings.Common.saveEllipsis) {
                Task { await backup() }
            }
            .buttonStyle(.dash(.filledBlue, .large, fillsWidth: true))
            .keyboardShortcut(.defaultAction)
            .disabled(walletID == nil || passphrase != confirmation)
            .accessibilityIdentifier("backup.save")
        }
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
