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
            // The macOS settings toolbar: icon over label per tab (UX-SPEC
            // §4.12). Each tab is a button named by its title.
            HStack(spacing: DashSpacing.xxs) {
                tabButton(.general, MacStrings.Options.general, "gearshape")
                ForEach(options.tabs, id: \.self) { item in
                    tabButton(.options(item), item.title, Self.symbol(item))
                }
                tabButton(.security, MacStrings.Options.security, "lock.shield")
            }
            .padding(.horizontal, DashSpacing.m)
            .padding(.vertical, DashSpacing.s)
            .frame(maxWidth: .infinity)
            .background(Color.role.card)
            .overlay(alignment: .bottom) { Rectangle().fill(Color.role.separator).frame(height: 0.5) }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("options.tabs")
            ScrollView {
                Group {
                    switch tab {
                    case .general: GeneralOptionsTab(model: model, settings: settings)
                    case .options(let item): content(item)
                    case .security:
                        SecurityOptionsTab(security: security, settings: settings, screenCapture: model.env?.screenCapture)
                    }
                }
                .padding(DashLayout.pagePaddingH)
            }
            .frame(height: 470)
            footer
        }
        .frame(width: 640)
        .dashCanvas()
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

    private func tabButton(_ value: MacOptionsTab, _ title: String, _ symbol: String) -> some View {
        let selected = tab == value
        return Button {
            tab = value
        } label: {
            VStack(spacing: DashSpacing.xxs) {
                Image(systemName: symbol)
                    .font(.system(size: 17, weight: .medium))
                    .frame(height: 20)
                Text(title)
                    .dashFont(.caption1Medium)
                    .lineLimit(1)
            }
            .foregroundStyle(selected ? Color.role.accent : Color.role.textSecondary)
            .padding(.horizontal, DashSpacing.s)
            .padding(.vertical, DashSpacing.xs)
            .frame(minWidth: 72)
            .background(
                RoundedRectangle(cornerRadius: DashRadius.small + 2, style: .continuous)
                    .fill(selected ? Color.role.accentTint : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(title))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    static func symbol(_ tab: OptionsTab) -> String {
        switch tab {
        case .main: "slider.horizontal.3"
        case .wallet: "wallet.pass"
        case .network: "network"
        case .display: "textformat"
        case .appearance: "circle.lefthalf.filled"
        case .notifications: "bell"
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: DashSpacing.s) {
            if options.restartRequired {
                Text(L10n.Options.restartRequired)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.role.warning)
            }
            if let error = options.errorMessage {
                Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger)
                    .accessibilityIdentifier("options.error")
            }
            if case .failed(let reason) = options.resetFlow {
                Text(reason).dashFont(.footnote).foregroundStyle(Color.role.danger)
            }
            HStack(spacing: DashSpacing.s) {
                Button(L10n.Options.resetOptions) { options.requestReset() }
                    .buttonStyle(.dash(.plainRed, .medium))
                    .accessibilityIdentifier("options.reset")
                Spacer()
                Button(MacStrings.Common.cancel) {
                    options.discard()
                    close()
                }
                .buttonStyle(.dash(.tintedGray, .medium))
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
                .buttonStyle(.dash(.filledBlue, .medium))
                .keyboardShortcut(.defaultAction)
                .disabled(options.proxyError != nil)
                .accessibilityIdentifier("options.ok")
            }
        }
        .padding(.horizontal, DashLayout.pagePaddingH)
        .padding(.vertical, DashSpacing.m)
        .background(Color.role.card)
        .overlay(alignment: .top) { Rectangle().fill(Color.role.separator).frame(height: 0.5) }
    }

    private func close() {
        dismissWindow(id: SceneID.options)
    }
}

// MARK: Tabs

/// A toggle on a menu row; the toggle keeps the row title as its name.
private struct ToggleRow: View {
    var icon: DashIconSource?
    let title: String
    var help: String?
    @Binding var isOn: Bool

    var body: some View {
        MenuRow(icon: icon, title: title, help: help) {
            Toggle(title, isOn: $isOn)
                .toggleStyle(.switch)
                .labelsHidden()
        }
    }
}

/// A picker on a menu row: the value and a pop-up menu.
private struct PickerRow<Value: Hashable, Options: View>: View {
    var icon: DashIconSource?
    let title: String
    var help: String?
    @Binding var selection: Value
    @ViewBuilder let options: () -> Options

    var body: some View {
        MenuRow(icon: icon, title: title, help: help) {
            Picker(title, selection: $selection) { options() }
                .labelsHidden()
                .fixedSize()
        }
    }
}

/// A stepper on a menu row: the value then − / +.
private struct StepperRow: View {
    var icon: DashIconSource?
    let title: String
    var help: String?
    @Binding var value: Int
    let range: ClosedRange<Int>

    var body: some View {
        MenuRow(icon: icon, title: title, help: help) {
            HStack(spacing: DashSpacing.s) {
                Text("\(value)")
                    .dashFont(.subhead)
                    .monospacedDigit()
                    .foregroundStyle(Color.role.textPrimary)
                Stepper(title, value: $value, in: range).labelsHidden()
            }
        }
    }
}

/// macOS: the network (applies at once, as in M1) and the menu bar companion.
private struct GeneralOptionsTab: View {
    let model: MacAppModel
    let settings: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
            MenuCard {
                MenuRow(icon: .token(.connections), title: MacStrings.Settings.network, help: MacStrings.Settings.networkHelp) {
                    HStack(spacing: DashSpacing.s) {
                        if settings.isSwitchingNetwork { ProgressView().controlSize(.small) }
                        Picker(MacStrings.Settings.network, selection: Binding(
                            get: { settings.network ?? .mainnet },
                            set: { network in Task { await settings.switchNetwork(to: network) } }
                        )) {
                            ForEach(settings.availableNetworks, id: \.self) { network in
                                Text(L10n.Settings.networkName(network)).tag(network)
                            }
                        }
                        .labelsHidden()
                        .fixedSize()
                        .disabled(settings.isSwitchingNetwork)
                        .accessibilityIdentifier("settings.network")
                    }
                }
                ToggleRow(
                    icon: .token(.settings), title: MacStrings.Settings.menuBar,
                    isOn: Binding(get: { model.showsMenuBarExtra }, set: { model.showsMenuBarExtra = $0 }))
                .accessibilityIdentifier("settings.menuBar")
            }
            if let error = model.preferencesError {
                SystemNotice(text: error, tone: .error)
            }
            if let error = settings.errorMessage {
                SystemNotice(text: error, tone: .error)
            }
        }
    }
}

/// dash-qt Main tab: start on login and the tray settings (Windows/Linux).
private struct MainOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
            if options.showsStartOnLogin || options.showsTrayOptions {
                MenuCard {
                    if options.showsStartOnLogin {
                        ToggleRow(icon: .token(.settings), title: L10n.Options.startOnLogin, isOn: $options.main.startOnLogin)
                    }
                    if options.showsTrayOptions {
                        ToggleRow(title: L10n.Options.showTrayIcon, isOn: $options.main.showTrayIcon)
                        ToggleRow(title: L10n.Options.minimizeToTray, isOn: $options.main.minimizeToTray)
                            .disabled(!options.main.showTrayIcon)
                        ToggleRow(title: L10n.Options.minimizeOnClose, isOn: $options.main.minimizeOnClose)
                    }
                }
            }
            SPVFootnote()
        }
    }
}

private struct WalletOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
            MenuCard {
                ToggleRow(icon: .token(.send), title: L10n.Options.subtractFeeByDefault, isOn: $options.wallet.subtractFeeByDefault)
                    .accessibilityIdentifier("options.subtractFee")
                ToggleRow(icon: .token(.tools), title: L10n.Options.coinControl, isOn: $options.wallet.coinControl)
                    .accessibilityIdentifier("options.coinControl")
                ToggleRow(icon: .token(.file), title: L10n.Options.psbtControls, isOn: $options.wallet.psbtControls)
                    .accessibilityIdentifier("options.psbtControls")
                ToggleRow(
                    icon: .token(.transfer), title: L10n.Options.keepCustomChangeAddress,
                    isOn: $options.wallet.keepCustomChangeAddress)
                .disabled(!options.wallet.coinControl)
            }
            MenuCard {
                ToggleRow(icon: .token(.shield), title: L10n.Options.dustProtection, isOn: $options.wallet.dustProtectionEnabled)
                    .disabled(!options.dustProtectionAvailable)
                MenuRow(
                    title: L10n.Options.dustThreshold,
                    help: !options.dustProtectionAvailable ? L10n.Options.unavailable : nil
                ) {
                    TextField(L10n.Options.dustThreshold, value: $options.wallet.dustThreshold, format: .number)
                        .textFieldStyle(.dash(isError: options.dustProtectionAvailable
                            && !OptionsViewModel.dustThresholdRange.contains(options.wallet.dustThreshold)))
                        .multilineTextAlignment(.trailing)
                        .monospacedDigit()
                        .frame(width: 130)
                        .accessibilityIdentifier("options.dustThreshold")
                }
                .disabled(!options.dustProtectionAvailable || !options.wallet.dustProtectionEnabled)
                if options.dustProtectionAvailable,
                    !OptionsViewModel.dustThresholdRange.contains(options.wallet.dustThreshold)
                {
                    Text(L10n.Options.dustThresholdInvalid)
                        .dashFont(.footnote)
                        .foregroundStyle(Color.role.danger)
                        .padding(.horizontal, DashSpacing.sm)
                }
            }
            MenuCard {
                StepperRow(
                    icon: .token(.backup), title: L10n.Options.automaticBackups,
                    help: options.automaticBackupsAvailable ? nil : L10n.Options.unavailable,
                    value: $options.wallet.automaticBackups, range: OptionsViewModel.automaticBackupsRange)
                .disabled(!options.automaticBackupsAvailable)
            }
        }
    }
}

/// The proxy fields stay visible and disabled until the engine supports a
/// proxy (OptionsViewModel.network.isEditable).
private struct NetworkOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
            MenuCard {
                ToggleRow(icon: .token(.connections), title: L10n.Options.proxy, isOn: $options.network.proxyEnabled)
                proxyFields(ip: $options.network.proxyIP, port: $options.network.proxyPort)
                ToggleRow(icon: .token(.shield), title: L10n.Options.onionProxy, isOn: $options.network.onionEnabled)
                proxyFields(ip: $options.network.onionIP, port: $options.network.onionPort)
                if let error = options.proxyError {
                    Text(error).dashFont(.footnote).foregroundStyle(Color.role.danger).padding(.horizontal, DashSpacing.sm)
                }
            }
            .disabled(!options.network.isEditable)
            SystemNotice(text: options.network.disabledReason, tone: .info)
                .accessibilityIdentifier("options.proxyUnavailable")
            SPVFootnote()
        }
    }

    private func proxyFields(ip: Binding<String>, port: Binding<String>) -> some View {
        HStack(spacing: DashSpacing.s) {
            TextField(L10n.Options.proxyIP, text: ip).textFieldStyle(.dash)
            TextField(L10n.Options.proxyPort, text: port).textFieldStyle(.dash).frame(width: 120)
        }
        .padding(.horizontal, DashSpacing.sm)
        .padding(.bottom, DashSpacing.s)
    }
}

private struct DisplayOptionsTab: View {
    let model: MacAppModel
    @Bindable var options: OptionsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: DashLayout.sectionGap) {
            MenuCard {
                PickerRow(icon: .token(.localCurrency), title: L10n.Options.language, selection: $options.display.languageCode) {
                    ForEach(options.availableLanguages, id: \.self) { code in
                        Text(L10n.Options.languageName(code)).tag(code)
                    }
                }
                PickerRow(icon: .token(.dashCurrency), title: L10n.Options.unit, selection: $options.display.unit) {
                    ForEach(DisplayUnit.allCases, id: \.self) { unit in
                        Text(model.env?.amounts.unitName(unit) ?? "").tag(unit)
                    }
                }
                .accessibilityIdentifier("options.unit")
                StepperRow(
                    title: L10n.Options.decimalDigits, value: $options.display.decimalDigits,
                    range: OptionsViewModel.decimalDigitsRange)
                PickerRow(
                    title: L10n.Options.localCurrency,
                    selection: Binding(
                        get: { options.display.localCurrency ?? options.defaultCurrency },
                        set: { options.display.localCurrency = $0 })
                ) {
                    ForEach(options.currencies(), id: \.self) { code in
                        Text(L10n.Options.currencyName(code)).tag(code)
                    }
                }
            }
            MenuCard(title: L10n.Options.thirdPartyTxURLs, footer: MacStrings.Options.thirdPartyHelp) {
                TextField(L10n.Options.thirdPartyTxURLs, text: $options.display.thirdPartyTxURLs)
                    .textFieldStyle(.dash)
                    .padding(DashSpacing.xs)
                    .accessibilityIdentifier("options.thirdPartyURLs")
            }
        }
    }
}

private struct AppearanceOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        MenuCard {
            MenuRow(icon: .token(.appearance), title: MacStrings.Settings.theme) {
                DashSegmentedControl(
                    [(AppTheme.system, L10n.Settings.themeSystem), (AppTheme.light, L10n.Settings.themeLight),
                     (AppTheme.dark, L10n.Settings.themeDark)],
                    selection: $options.appearance)
            }
            .accessibilityIdentifier("options.theme")
        }
    }
}

private struct NotificationOptionsTab: View {
    @Bindable var options: OptionsViewModel

    var body: some View {
        MenuCard {
            ToggleRow(
                icon: .token(.notifications), title: L10n.Options.notificationsEnabled,
                help: options.notificationStatusText, isOn: $options.notifications.enabled)
            .accessibilityIdentifier("options.notifications")
            ToggleRow(
                icon: .token(.coinjoinMixing), title: L10n.Options.showCoinJoinNotifications,
                isOn: $options.notifications.showCoinJoinNotifications)
            .disabled(!options.notifications.enabled)
        }
    }
}

/// dash-qt node options this SPV wallet does not have (DESIGN-opus §1.14).
private struct SPVFootnote: View {
    var body: some View {
        SystemMessageView(
            title: L10n.Options.spvFootnote, subtitle: L10n.Options.spvOnlyOptions.joined(separator: ", "),
            icon: .token(.messageInfo), backgroundColor: Color.role.accentTint)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("options.spvFootnote")
    }
}
#endif
