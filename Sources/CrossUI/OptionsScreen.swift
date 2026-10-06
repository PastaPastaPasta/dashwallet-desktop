// dash-qt's Options dialog as a page (QT-033, QT-135…141, IOS-104/105): the
// tabs edit copies; OK writes what changed, Cancel restores the stored
// values, Reset Options backs the settings up, resets them and quits.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct OptionsScreen: View {
    let model: OptionsViewModel
    let state: CrossAppState

    @State var tab: OptionsTab = .main
    @State var dustText = ""
    @State var currencySearch = ""
    @State var applied = false

    var body: some View {
        let model = model
        let state = state
        let tabs = model.tabs
        let current = tabs.contains(tab) ? tab : tabs[0]
        Page(CrossStrings.optionsPage) {
            SegmentedControl(options: tabs.map { PickerOption($0, $0.title) }, selection: current) { tab = $0 }
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            if model.restartRequired {
                Toast(L10n.Options.restartRequired, kind: .warning)
            }
            if applied, model.errorMessage == nil {
                Toast(CrossStrings.optionsSaved, kind: .success)
            }
            DashCard(spacing: Int(DashSpacing.l)) {
                switch current {
                case .main: mainTab(model)
                case .wallet: walletTab(model)
                case .network: networkTab(model)
                case .display: displayTab(model)
                case .appearance: appearanceTab(model)
                case .notifications: notificationsTab(model)
                }
            }
            if current == .network || current == .wallet {
                // SPV-only note at the bottom of the affected tabs (UX-SPEC §4.12).
                Toast(
                    "\(L10n.Options.spvFootnote) \(L10n.Options.spvOnlyOptions.joined(separator: ", "))", kind: .info)
            }
            if model.resetFlow != .idle {
                resetSection(model)
            }
            // dash-qt's button bar: Reset Options, then Cancel and OK.
            HStack(spacing: Int(DashSpacing.s)) {
                if model.resetFlow == .idle {
                    resetSection(model)
                }
                Spacer()
                DashButton(CrossStrings.cancel, style: .tintedGray) {
                    model.discard()
                    dustText = String(model.wallet.dustThreshold)
                    applied = false
                    state.closePage()
                }
                DashButton(CrossStrings.ok, style: .filledBlue) {
                    if model.wallet.dustProtectionEnabled, let value = Int64(dustText) { model.wallet.dustThreshold = value }
                    Task {
                        do throws(ServiceError) {
                            try await model.apply()
                            state.shell.optionsChanged()
                            applied = true
                        } catch {
                            applied = false
                        }
                    }
                }
            }
        }
        .task {
            await model.load()
            dustText = String(model.wallet.dustThreshold)
        }
    }

    // MARK: Tabs

    @ViewBuilder
    private func mainTab(_ model: OptionsViewModel) -> some View {
        if model.showsStartOnLogin {
            DashToggle(L10n.Options.startOnLogin, isOn: bind({ model.main.startOnLogin }, { model.main.startOnLogin = $0 }))
        }
        if model.showsTrayOptions {
            // dw-desktop has no tray backend yet; the values are kept but have
            // no effect until it has one.
            if !state.capabilities.tray {
                Toast(CrossStrings.noTray, kind: .info)
            }
            DashToggle(L10n.Options.showTrayIcon, isOn: bind({ model.main.showTrayIcon }, { model.main.showTrayIcon = $0 }))
                .disabled(!state.capabilities.tray)
            DashToggle(
                L10n.Options.minimizeToTray, isOn: bind({ model.main.minimizeToTray }, { model.main.minimizeToTray = $0 })
            )
            .disabled(!state.capabilities.tray)
            DashToggle(
                L10n.Options.minimizeOnClose, isOn: bind({ model.main.minimizeOnClose }, { model.main.minimizeOnClose = $0 })
            )
            .disabled(!state.capabilities.tray)
        }
    }

    @ViewBuilder
    private func walletTab(_ model: OptionsViewModel) -> some View {
        DashToggle(
            L10n.Options.subtractFeeByDefault,
            isOn: bind({ model.wallet.subtractFeeByDefault }, { model.wallet.subtractFeeByDefault = $0 }))
        DashToggle(L10n.Options.coinControl, isOn: bind({ model.wallet.coinControl }, { model.wallet.coinControl = $0 }))
        DashToggle(L10n.Options.psbtControls, isOn: bind({ model.wallet.psbtControls }, { model.wallet.psbtControls = $0 }))
        DashToggle(
            L10n.Options.keepCustomChangeAddress,
            isOn: bind({ model.wallet.keepCustomChangeAddress }, { model.wallet.keepCustomChangeAddress = $0 }))
        if model.dustProtectionAvailable {
            DashToggle(
                L10n.Options.dustProtection,
                isOn: bind({ model.wallet.dustProtectionEnabled }, { model.wallet.dustProtectionEnabled = $0 }))
            if model.wallet.dustProtectionEnabled {
                DashTextField(
                    L10n.Options.dustThreshold, text: bind({ dustText }, { text in
                        dustText = text
                        if let value = Int64(text) { model.wallet.dustThreshold = value }
                    }), width: 200)
            }
        } else {
            KeyValueRow(L10n.Options.dustProtection, L10n.Options.unavailable)
        }
        if model.automaticBackupsAvailable {
            DashPicker(
                L10n.Options.automaticBackups,
                options: OptionsViewModel.automaticBackupsRange.map { PickerOption($0, "\($0)") },
                selection: bind({ model.wallet.automaticBackups }, { model.wallet.automaticBackups = $0 }))
        } else {
            KeyValueRow(L10n.Options.automaticBackups, L10n.Options.unavailable)
        }
    }

    /// Shown, not editable, until the engine has proxy support; the
    /// numeric-IP check still runs on what is shown.
    @ViewBuilder
    private func networkTab(_ model: OptionsViewModel) -> some View {
        Toast(model.network.disabledReason, kind: .info)
        VStack(alignment: .leading, spacing: Int(DashSpacing.s)) {
            DashToggle(L10n.Options.proxy, isOn: bind({ model.network.proxyEnabled }, { model.network.proxyEnabled = $0 }))
            HStack(spacing: Int(DashSpacing.s)) {
                DashTextField(
                    L10n.Options.proxyIP, text: bind({ model.network.proxyIP }, { model.network.proxyIP = $0 }), width: 200)
                DashTextField(
                    L10n.Options.proxyPort, text: bind({ model.network.proxyPort }, { model.network.proxyPort = $0 }),
                    error: model.proxyError, width: 100)
            }
            DashToggle(
                L10n.Options.onionProxy, isOn: bind({ model.network.onionEnabled }, { model.network.onionEnabled = $0 }))
            HStack(spacing: Int(DashSpacing.s)) {
                DashTextField(
                    L10n.Options.proxyIP, text: bind({ model.network.onionIP }, { model.network.onionIP = $0 }), width: 200)
                DashTextField(
                    L10n.Options.proxyPort, text: bind({ model.network.onionPort }, { model.network.onionPort = $0 }),
                    width: 100)
            }
        }
        .disabled(!model.network.isEditable)
    }

    @ViewBuilder
    private func displayTab(_ model: OptionsViewModel) -> some View {
        let amounts = state.env.amounts
        DashPicker(
            L10n.Options.language,
            options: model.availableLanguages.map { PickerOption($0, L10n.Options.languageName($0)) },
            selection: bind({ model.display.languageCode }, { model.display.languageCode = $0 }))
        DashPicker(
            L10n.Options.unit,
            options: DisplayUnit.allCases.map { PickerOption($0, amounts.unitName($0)) },
            selection: bind({ model.display.unit }, { model.display.unit = $0 }))
        DashPicker(
            L10n.Options.decimalDigits,
            options: OptionsViewModel.decimalDigitsRange.map { PickerOption($0, "\($0)") },
            selection: bind({ model.display.decimalDigits }, { model.display.decimalDigits = $0 }))
        DashToggle(
            L10n.Options.showMasternodesTab,
            isOn: bind({ model.display.showMasternodesTab }, { model.display.showMasternodesTab = $0 }))
        DashToggle(
            L10n.Options.showGovernanceTab,
            isOn: bind({ model.display.showGovernanceTab }, { model.display.showGovernanceTab = $0 }))
        DashToggle(
            L10n.Options.showGovernanceClock,
            isOn: bind({ model.display.showGovernanceClock }, { model.display.showGovernanceClock = $0 }))
        DashTextField(
            L10n.Options.thirdPartyTxURLs, placeholder: CrossStrings.thirdPartyPlaceholder,
            text: bind({ model.display.thirdPartyTxURLs }, { model.display.thirdPartyTxURLs = $0 }))
        HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
            DashTextField(CrossStrings.currencySearch, text: $currencySearch, width: 180)
            DashPicker(
                L10n.Options.localCurrency,
                options: currencyOptions(model),
                selection: bind({ model.display.localCurrency ?? model.defaultCurrency }, { model.display.localCurrency = $0 }))
        }
        Text(CrossStrings.currencyListOnly).dashFont(.caption1).dashForeground(CrossRole.textSecondary)
    }

    private func currencyOptions(_ model: OptionsViewModel) -> [PickerOption<String>] {
        var codes = model.currencies(matching: currencySearch)
        let selected = model.display.localCurrency ?? model.defaultCurrency
        if !codes.contains(selected) { codes.insert(selected, at: 0) }
        return codes.map { PickerOption($0, L10n.Options.currencyName($0)) }
    }

    @ViewBuilder
    private func appearanceTab(_ model: OptionsViewModel) -> some View {
        DashPicker(
            CrossStrings.theme,
            options: [
                PickerOption(AppTheme.system, L10n.Settings.themeSystem),
                PickerOption(.light, L10n.Settings.themeLight), PickerOption(.dark, L10n.Settings.themeDark),
            ],
            selection: bind({ model.appearance }, { model.appearance = $0 }))
    }

    @ViewBuilder
    private func notificationsTab(_ model: OptionsViewModel) -> some View {
        DashToggle(
            L10n.Options.notificationsEnabled,
            isOn: bind({ model.notifications.enabled }, { model.notifications.enabled = $0 }))
        if let status = model.notificationStatusText {
            Text(status).dashFont(.footnote).dashForeground(CrossRole.textSecondary)
        }
        DashToggle(
            L10n.Options.showCoinJoinNotifications,
            isOn: bind(
                { model.notifications.showCoinJoinNotifications }, { model.notifications.showCoinJoinNotifications = $0 }))
    }

    // MARK: Reset (QT-141)

    @ViewBuilder
    private func resetSection(_ model: OptionsViewModel) -> some View {
        let state = state
        switch model.resetFlow {
        case .idle:
            DashButton(L10n.Options.resetOptions, style: .plainRed) { model.requestReset() }
        case .confirming:
            ConfirmationCard(
                title: L10n.Options.resetTitle, message: L10n.Options.resetQuestion, confirmTitle: CrossStrings.yes,
                destructive: true, onConfirm: { model.confirmReset() }, onCancel: { model.cancelReset() })
        case .done(let backups):
            DashCard {
                Text(CrossStrings.resetDone).dashFont(.footnote)
                ForEach(backups, id: \.self) { url in
                    Text(url.path).dashFont(.caption1).textSelectionEnabled()
                }
                DashButton(L10n.Shell.exit, style: .filledBlue, size: .small) { Task { await state.quit() } }
            }
        case .failed(let text):
            Toast(text, kind: .error)
        }
    }
}
