// Help ▸ About Dash Wallet and Help ▸ Command-line options (QT-153,
// IOS-107, IOS-112): version, network, data directory, license, links, log
// export, and the options `dash-wallet` accepts.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct AboutScreen: View {
    let model: AboutViewModel
    let state: CrossAppState
    let showsOptions: Bool

    @Environment(\.openURL) var openURL
    @Environment(\.chooseFileSaveDestination) var chooseFileSaveDestination

    var body: some View {
        let model = model
        let state = state
        Page(showsOptions ? L10n.HomeM2.commandLineTitle : model.title) {
            if showsOptions {
                commandLine(model)
            } else {
                about(model)
            }
            DashButton(CrossStrings.close, style: .strokeGray, size: .small) { state.closePage() }
        }
        .task { await model.load() }
    }

    @ViewBuilder
    private func about(_ model: AboutViewModel) -> some View {
        DashCard {
            SectionHeader(L10n.Navigation.appName, style: .title3)
            KeyValueRow(CrossStrings.versionTitle, model.versionText ?? L10n.Common.unknown)
            KeyValueRow(L10n.HomeM2.network, model.networkName ?? L10n.Common.unknown)
            KeyValueRow(L10n.HomeM2.dataDirectory, model.dataDirectory?.path ?? L10n.Common.unknown)
            Text(model.licenseText).dashFont(.footnote).dashForeground(.secondaryText)
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
        }
        HStack(spacing: Int(DashSpacing.s)) {
            DashButton(L10n.HomeM2.github, style: .plainBlue, size: .small) { openURL(AboutViewModel.githubURL) }
            DashButton(CrossStrings.documentation, style: .plainBlue, size: .small) {
                openURL(AboutViewModel.documentationURL)
            }
            DashButton(L10n.HomeM2.support, style: .plainBlue, size: .small) { openURL(AboutViewModel.supportURL) }
            DashButton(L10n.HomeM2.commandLineTitle, style: .plainBlue, size: .small) {
                state.open(.commandLineOptions)
            }
        }
        DashCard {
            SectionHeader(L10n.HomeM2.exportLogs, style: .subheadMedium)
            DashButton(L10n.HomeM2.exportLogs, style: .tintedBlue, size: .small, isEnabled: model.logExport != .exporting) {
                let choose = chooseFileSaveDestination
                Task {
                    if let url = await choose(
                        title: L10n.HomeM2.exportLogs, defaultButtonLabel: CrossStrings.save,
                        defaultFileName: "dash-wallet-logs.zip")
                    {
                        await model.exportLogs(to: url)
                    }
                }
            }
            switch model.logExport {
            case .idle: EmptyView()
            case .exporting: Text(CrossStrings.working).dashFont(.footnote).dashForeground(.secondaryText)
            case .exported(let url): Toast("\(L10n.HomeM2.logsExported) \(url.path)", kind: .success)
            case .failed(let text): Toast(text, kind: .error)
            }
        }
    }

    /// dash-qt's `-help` options that this wallet keeps, then the options
    /// of the SwiftCrossUI app itself.
    @ViewBuilder
    private func commandLine(_ model: AboutViewModel) -> some View {
        DashCard {
            Text(L10n.HomeM2.commandLineUsage).dashFont(.footnoteMedium)
            ForEach(model.commandLineOptions) { option in
                KeyValueRow(option.name, option.text)
            }
        }
        if !state.appUsage.isEmpty {
            DashCard {
                SectionHeader(CrossStrings.appOptions, style: .subheadMedium)
                Text(state.appUsage).font(.system(size: 12).monospaced()).textSelectionEnabled()
            }
        }
    }
}
