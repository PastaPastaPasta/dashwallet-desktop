// Options window (QT-135…141, IOS-104/105, QT-033): dash-qt's tabs over
// OptionsViewModel with OK / Cancel / Reset Options, plus the macOS General
// tab (network, menu bar companion) and the Security tab (IOS-011/015/016).
// Node options an SPV wallet does not have are listed in a footnote
// (DESIGN-opus §1.14), never shown as controls.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

/// The tabs of the macOS Options window: the app's General and Security
/// tabs around dash-qt's.
enum MacOptionsTab: Hashable {
    case general
    case options(OptionsTab)
    case security
}

struct OptionsWindow: View {
    let model: MacAppModel

    var body: some View {
        if let features = model.features, let main = model.main {
            OptionsView(
                model: model, options: features.options, settings: main.settings, security: features.security,
                onDone: { features.shell.optionsChanged() })
        } else {
            Text(model.unavailableReason ?? L10n.Options.unavailable).padding(DashSpacing.xl)
        }
    }
}

struct OptionsView: View {
    let model: MacAppModel
    @Bindable var options: OptionsViewModel
    let settings: SettingsViewModel
    let security: SecurityViewModel
    var onDone: () -> Void = {}
    @State var tab: MacOptionsTab = .general
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        VStack(spacing: 0) {
            // dash-qt's tab strip as a segmented control (it also renders
            // offscreen for the screenshots and is reachable in UI tests).
            Picker("", selection: $tab) {
                Text(MacStrings.Options.general).tag(MacOptionsTab.general)
                ForEach(options.tabs, id: \.self) { item in
                    Text(item.title).tag(MacOptionsTab.options(item))
                }
                Text(MacStrings.Options.security).tag(MacOptionsTab.security)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(DashSpacing.m)
            .accessibilityIdentifier("options.tabs")
            Group {
                switch tab {
                case .general: GeneralOptionsTab(model: model, settings: settings)
                case .options(let item): content(item)
                case .security:
                    SecurityOptionsTab(security: security, settings: settings, screenCapture: model.env?.screenCapture)
                }
            }
            .frame(height: 430)
            Divider()
            footer
        }
        .frame(width: 600)
        .task { await options.load() }
        .alert(L10n.Options.resetTitle, isPresented: Binding(
            get: { options.resetFlow == .confirming }, set: { if !$0 { options.cancelReset() } }
        )) {
            Button(MacStrings.Common.cancel, role: .cancel) { options.cancelReset() }
            Button(MacStrings.Common.ok, role: .destructive) { options.confirmReset() }
        } message: {
            Text(L10n.Options.resetQuestion)
        }
        .onChange(of: options.resetFlow) { _, flow in
            // dash-qt shuts the client down after a reset; it does not restart.
            if case .done = flow { MacApplication.terminate() }
        }
        .accessibilityIdentifier("options")
    }

    @ViewBuilder
    private func content(_ item: OptionsTab) -> some View {
        switch item {
        case .main: MainOptionsTab(options: options)
        case .wallet: WalletOptionsTab(options: options)
        case .network: NetworkOptionsTab(options: options)
        case .display: DisplayOptionsTab(model: model, options: options)
        case .appearance: AppearanceOptionsTab(options: options)
        case .notifications: NotificationOptionsTab(options: options)
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            if options.restartRequired {
                Text(L10n.Options.restartRequired)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.orange)
            }
            if let error = options.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
                    .accessibilityIdentifier("options.error")
            }
            if case .failed(let reason) = options.resetFlow {
                Text(reason).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
            }
            HStack {
                Button(L10n.Options.resetOptions) { options.requestReset() }
                    .accessibilityIdentifier("options.reset")
                Spacer()
                Button(MacStrings.Common.cancel) {
                    options.discard()
                    close()
                }
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("options.cancel")
                Button(MacStrings.Common.ok) {
                    Task {
                        do {
                            try await options.apply()
                            onDone()
                            close()
                        } catch {
                            // `errorMessage` says why; the window stays open.
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(options.proxyError != nil)
                .accessibilityIdentifier("options.ok")
            }
        }
        .padding(DashSpacing.l)
    }

    private func close() {
        dismissWindow(id: SceneID.options)
    }
}

// MARK: Tabs

/// macOS: the network (applies at once, as in M1) and the menu bar companion.
private struct GeneralOptionsTab: View {
    let model: MacAppModel
    let settings: SettingsViewModel

    var body: some View {
        Form {
            Picker(MacStrings.Settings.network, selection: Binding(
                get: { settings.network ?? .mainnet },
                set: { network in Task { await settings.switchNetwork(to: network) } }
            )) {
                ForEach(settings.availableNetworks, id: \.self) { network in
                    Text(L10n.Settings.networkName(network)).tag(network)
                }
            }
            .disabled(settings.isSwitchingNetwork)
            .accessibilityIdentifier("settings.network")
            Text(MacStrings.Settings.networkHelp)
                .font(.footnote)
                .foregroundStyle(.secondary)
            if settings.isSwitchingNetwork {
                ProgressView().controlSize(.small)
            }
            Toggle(MacStrings.Settings.menuBar, isOn: Binding(
                get: { model.showsMenuBarExtra }, set: { model.showsMenuBarExtra = $0 }))
            .accessibilityIdentifier("settings.menuBar")
            if let error = model.preferencesError {
                Text(error).foregroundStyle(Color.dash.errorText)
            }
            if let error = settings.errorMessage {
                Text(error).foregroundStyle(Color.dash.errorText)
            }
        }
        .formStyle(.grouped)
    }
}

/// dash-qt Main tab: start on login and the tray settings (Windows/Linux).
private struct MainOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        Form {
            if options.showsStartOnLogin {
                Toggle(L10n.Options.startOnLogin, isOn: $options.main.startOnLogin)
            }
            if options.showsTrayOptions {
                Toggle(L10n.Options.showTrayIcon, isOn: $options.main.showTrayIcon)
                Toggle(L10n.Options.minimizeToTray, isOn: $options.main.minimizeToTray)
                    .disabled(!options.main.showTrayIcon)
                Toggle(L10n.Options.minimizeOnClose, isOn: $options.main.minimizeOnClose)
            }
            SPVFootnote()
        }
        .formStyle(.grouped)
    }
}

private struct WalletOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        Form {
            Toggle(L10n.Options.subtractFeeByDefault, isOn: $options.wallet.subtractFeeByDefault)
                .accessibilityIdentifier("options.subtractFee")
            Toggle(L10n.Options.coinControl, isOn: $options.wallet.coinControl)
                .accessibilityIdentifier("options.coinControl")
            Toggle(L10n.Options.psbtControls, isOn: $options.wallet.psbtControls)
                .accessibilityIdentifier("options.psbtControls")
            Toggle(L10n.Options.keepCustomChangeAddress, isOn: $options.wallet.keepCustomChangeAddress)
                .disabled(!options.wallet.coinControl)
            Section {
                Toggle(L10n.Options.dustProtection, isOn: $options.wallet.dustProtectionEnabled)
                    .disabled(!options.dustProtectionAvailable)
                TextField(L10n.Options.dustThreshold, value: $options.wallet.dustThreshold, format: .number)
                    .disabled(!options.dustProtectionAvailable || !options.wallet.dustProtectionEnabled)
                    .accessibilityIdentifier("options.dustThreshold")
                if !options.dustProtectionAvailable {
                    Text(L10n.Options.unavailable).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
                } else if !OptionsViewModel.dustThresholdRange.contains(options.wallet.dustThreshold) {
                    Text(L10n.Options.dustThresholdInvalid).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
                }
            }
            Section {
                Stepper(value: $options.wallet.automaticBackups, in: OptionsViewModel.automaticBackupsRange) {
                    LabeledContent(L10n.Options.automaticBackups, value: "\(options.wallet.automaticBackups)")
                }
                .disabled(!options.automaticBackupsAvailable)
                if !options.automaticBackupsAvailable {
                    Text(L10n.Options.unavailable).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
                }
            }
        }
        .formStyle(.grouped)
    }
}

/// The proxy fields stay visible and disabled until the engine supports a
/// proxy (OptionsViewModel.network.isEditable).
private struct NetworkOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        Form {
            Section {
                Toggle(L10n.Options.proxy, isOn: $options.network.proxyEnabled)
                HStack {
                    TextField(L10n.Options.proxyIP, text: $options.network.proxyIP)
                    TextField(L10n.Options.proxyPort, text: $options.network.proxyPort).frame(width: 140)
                }
                Toggle(L10n.Options.onionProxy, isOn: $options.network.onionEnabled)
                HStack {
                    TextField(L10n.Options.proxyIP, text: $options.network.onionIP)
                    TextField(L10n.Options.proxyPort, text: $options.network.onionPort).frame(width: 140)
                }
                if let error = options.proxyError {
                    Text(error).dashFont(.footnote).foregroundStyle(Color.dash.errorText)
                }
            }
            .disabled(!options.network.isEditable)
            Text(options.network.disabledReason)
                .dashFont(.footnote)
                .foregroundStyle(Color.dash.secondaryText)
                .accessibilityIdentifier("options.proxyUnavailable")
            SPVFootnote()
        }
        .formStyle(.grouped)
    }
}

private struct DisplayOptionsTab: View {
    let model: MacAppModel
    @Bindable var options: OptionsViewModel
    @State private var currencySearch = ""

    var body: some View {
        Form {
            Picker(L10n.Options.language, selection: $options.display.languageCode) {
                ForEach(options.availableLanguages, id: \.self) { code in
                    Text(L10n.Options.languageName(code)).tag(code)
                }
            }
            Picker(L10n.Options.unit, selection: $options.display.unit) {
                ForEach(DisplayUnit.allCases, id: \.self) { unit in
                    Text(model.env?.amounts.unitName(unit) ?? "").tag(unit)
                }
            }
            .accessibilityIdentifier("options.unit")
            Stepper(value: $options.display.decimalDigits, in: OptionsViewModel.decimalDigitsRange) {
                LabeledContent(L10n.Options.decimalDigits, value: "\(options.display.decimalDigits)")
            }
            Toggle(L10n.Options.showMasternodesTab, isOn: $options.display.showMasternodesTab)
            Toggle(L10n.Options.showGovernanceTab, isOn: $options.display.showGovernanceTab)
            Toggle(L10n.Options.showGovernanceClock, isOn: $options.display.showGovernanceClock)
                .disabled(!options.display.showGovernanceTab)
            TextField(L10n.Options.thirdPartyTxURLs, text: $options.display.thirdPartyTxURLs)
                .help(MacStrings.Options.thirdPartyHelp)
                .accessibilityIdentifier("options.thirdPartyURLs")
            Picker(L10n.Options.localCurrency, selection: Binding(
                get: { options.display.localCurrency ?? options.defaultCurrency },
                set: { options.display.localCurrency = $0 }
            )) {
                ForEach(options.currencies(), id: \.self) { code in
                    Text(L10n.Options.currencyName(code)).tag(code)
                }
            }
        }
        .formStyle(.grouped)
    }
}

private struct AppearanceOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        Form {
            Picker(MacStrings.Settings.theme, selection: $options.appearance) {
                Text(L10n.Settings.themeSystem).tag(AppTheme.system)
                Text(L10n.Settings.themeLight).tag(AppTheme.light)
                Text(L10n.Settings.themeDark).tag(AppTheme.dark)
            }
            .pickerStyle(.radioGroup)
            .accessibilityIdentifier("options.theme")
        }
        .formStyle(.grouped)
    }
}

private struct NotificationOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        Form {
            Toggle(L10n.Options.notificationsEnabled, isOn: $options.notifications.enabled)
                .accessibilityIdentifier("options.notifications")
            if let status = options.notificationStatusText {
                Text(status).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
            }
            Toggle(L10n.Options.showCoinJoinNotifications, isOn: $options.notifications.showCoinJoinNotifications)
                .disabled(!options.notifications.enabled)
        }
        .formStyle(.grouped)
    }
}

/// dash-qt node options this SPV wallet does not have (DESIGN-opus §1.14).
private struct SPVFootnote: View {
    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
            Text(L10n.Options.spvFootnote).dashFont(.footnoteMedium)
            Text(L10n.Options.spvOnlyOptions.joined(separator: ", "))
                .dashFont(.footnote)
        }
        .foregroundStyle(Color.dash.secondaryText)
        .accessibilityIdentifier("options.spvFootnote")
    }
}
#endif
