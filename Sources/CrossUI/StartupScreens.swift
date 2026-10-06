// Startup and shutdown windows as window content (QT-004, QT-005, QT-007,
// QT-008): the data-directory chooser, the splash with its progress and
// Quit, the unreadable-settings question and "shutting down".
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

/// dash-qt's Intro dialog: the default or a custom data directory, its
/// status and free space; OK creates it.
public struct DataDirectoryChooserScreen: View {
    let model: DataDirectoryChooserViewModel
    let onDone: @MainActor (DataDirectoryChooserViewModel.Outcome) -> Void

    @Environment(\.chooseFile) var chooseFile
    @State var customPath = ""

    public init(
        model: DataDirectoryChooserViewModel, onDone: @escaping @MainActor (DataDirectoryChooserViewModel.Outcome) -> Void
    ) {
        self.model = model
        self.onDone = onDone
    }

    public var body: some View {
        let model = model
        Page(L10n.Shell.welcomeTitle) {
            Text(L10n.Shell.welcome).dashFont(.headline)
            Text(L10n.Shell.welcomeDetail).dashFont(.footnote).dashForeground(.secondaryText)
            DashCard {
                DashPicker(
                    CrossStrings.dataDirectory,
                    options: [
                        PickerOption(DataDirectoryChooserViewModel.Choice.defaultDirectory, L10n.Shell.useDefaultDirectory),
                        PickerOption(.custom, L10n.Shell.useCustomDirectory),
                    ],
                    selection: bind({ model.choice }, { choice in Task { await model.choose(choice) } }))
                Text(model.defaultDirectory.path).dashFont(.caption1).textSelectionEnabled()
                if model.choice == .custom {
                    HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                        DashTextField(CrossStrings.customDirectory, text: $customPath)
                        DashButton(CrossStrings.use, style: .tintedGray, size: .small, isEnabled: !customPath.isEmpty) {
                            let url = URL(fileURLWithPath: (customPath as NSString).expandingTildeInPath, isDirectory: true)
                            Task { await model.setCustomDirectory(url) }
                        }
                        DashButton(CrossStrings.browse, style: .tintedGray, size: .small) {
                            let choose = chooseFile
                            Task {
                                if let url = await choose(
                                    title: L10n.Shell.useCustomDirectory, defaultButtonLabel: CrossStrings.choose,
                                    allowSelectingFiles: false, allowSelectingDirectories: true)
                                {
                                    customPath = url.path
                                    await model.setCustomDirectory(url)
                                }
                            }
                        }
                    }
                }
                if let status = model.statusText {
                    Text(status).dashFont(.footnote).dashForeground(model.canAccept ? .primaryText : .red)
                }
                if let space = model.freeSpaceText {
                    Text(space).dashFont(.footnote).dashForeground(.secondaryText)
                }
                if let error = model.errorMessage {
                    Toast(error, kind: .error)
                }
            }
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(CrossStrings.ok, style: .filledBlue, size: .small, isEnabled: model.canAccept) {
                    Task {
                        await model.accept()
                        if case .accepted = model.outcome { onDone(model.outcome) }
                    }
                }
                DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                    model.cancel()
                    onDone(model.outcome)
                }
            }
        }
        .task { await model.load() }
    }
}

/// The splash (QT-005): the phase text, a bar that never moves back, and
/// Quit (dash-qt's Q key; SwiftCrossUI 0.10 has no key handler here).
public struct SplashScreen: View {
    let model: StartupViewModel

    public init(model: StartupViewModel) {
        self.model = model
    }

    public var body: some View {
        let model = model
        VStack(spacing: Int(DashSpacing.m)) {
            Spacer()
            Text(L10n.Navigation.appName).dashFont(.title1).dashForeground(.primaryText)
            Text(model.statusText).dashFont(.callout).dashForeground(.secondaryText)
            ProgressView(value: model.progress).frame(width: 320)
            DashButton(CrossStrings.quit, style: .strokeGray, size: .small, help: L10n.Shell.pressQToQuit) { model.quit() }
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}

/// QT-007: the settings file could not be read; Reset keeps the defaults
/// (the old file stays as `.bak`), Abort quits without writing.
public struct SettingsUnreadableScreen: View {
    let model: StartupViewModel
    let files: [URL]
    let onReset: @MainActor () -> Void
    let onAbort: @MainActor () -> Void

    public init(
        model: StartupViewModel, files: [URL], onReset: @escaping @MainActor () -> Void,
        onAbort: @escaping @MainActor () -> Void
    ) {
        self.model = model
        self.files = files
        self.onReset = onReset
        self.onAbort = onAbort
    }

    public var body: some View {
        let model = model
        let onReset = onReset
        let onAbort = onAbort
        Page(L10n.Shell.settingsUnreadable) {
            Text(L10n.Shell.settingsResetQuestion).dashFont(.footnote)
            ForEach(files, id: \.self) { url in
                Text(url.path).dashFont(.caption1).textSelectionEnabled()
            }
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(CrossStrings.reset, style: .filledBlue, size: .small) {
                    model.resetSettings()
                    onReset()
                }
                DashButton(CrossStrings.abort, style: .strokeGray, size: .small) {
                    model.abortSettings()
                    onAbort()
                }
            }
        }
    }
}

/// QT-008: shown while the engine stops; it has no close button.
struct ShutdownScreen: View {
    let model: ShutdownViewModel

    var body: some View {
        VStack(spacing: Int(DashSpacing.m)) {
            Spacer()
            ProgressView()
            Text(model.title).dashFont(.headline).dashForeground(.primaryText)
            Text(model.message).dashFont(.footnote).dashForeground(.secondaryText)
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}
