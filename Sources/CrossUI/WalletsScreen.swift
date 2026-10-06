// Wallet management (QT-101, QT-106…110, QT-114, QT-116, IOS-110, IOS-111):
// open/close/load on startup, rename, remove, imports from Dash Core files
// and key material, watch-only wallets, backup and restore, exports for Dash
// Core, automatic backups and the account xpub. The first-run "Wallets found
// on this device" question (IOS-009) belongs before onboarding and is not
// shown here: on this page the open network always has data.
import DashUICross
import DesignTokens
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

struct WalletsScreen: View {
    let model: WalletManagementViewModel
    let state: CrossAppState

    @Environment(\.chooseFile) var chooseFile
    @Environment(\.chooseFileSaveDestination) var chooseFileSaveDestination
    @State var renameText = ""
    @State var renaming: WalletID?
    @State var passphrase = ""
    @State var backupPassphrase = ""
    @State var keyKind: KeyMaterialKind = .hdSeed
    @State var keyText = ""
    @State var xpubText = ""
    @State var watchName = ""
    @State var watchBirthHeight = ""
    @State var exportFormat: CoreExportFormat = .dumpWallet

    var body: some View {
        let model = model
        Page(CrossStrings.walletsPage) {
            flowPanel(model)
            if let error = model.errorMessage {
                Toast(error, kind: .error)
            }
            walletList(model)
            if let xpub = model.xpub {
                DashCard {
                    HStack {
                        SectionHeader(L10n.Wallets.xpubTitle, style: .subheadMedium)
                        Spacer()
                        DashButton(CrossStrings.copyXpub, style: .plainBlue, size: .small) {
                            state.copy(xpub.xpub, what: L10n.Wallets.xpubTitle)
                        }
                    }
                    KeyValueRow(CrossStrings.derivationPath, xpub.derivationPath)
                    Text(xpub.xpub).dashFont(.footnote).textSelectionEnabled()
                    if let qr = model.xpubQR {
                        QRCodeView(size: qr.size, modules: qr.modules)
                    }
                }
            }
            imports(model)
            backups(model)
        }
        .task {
            model.start()
            await model.load()
        }
    }

    // MARK: Flow

    @ViewBuilder
    private func flowPanel(_ model: WalletManagementViewModel) -> some View {
        switch model.flow {
        case .idle:
            EmptyView()
        case .working(let operation):
            Text(CrossStrings.walletOperation(operation)).dashFont(.footnote).dashForeground(.secondaryText)
        case .needsFilePassphrase(let url, _):
            DashCard {
                Text(CrossStrings.filePassphrasePrompt(url.lastPathComponent)).dashFont(.footnote)
                DashSecureField(CrossStrings.filePassphrase, text: $passphrase)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.continueTitle, style: .filledBlue, size: .small) {
                        let text = passphrase
                        passphrase = ""
                        Task { await model.provideFilePassphrase(text) }
                    }
                    DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                        passphrase = ""
                        model.dismiss()
                    }
                }
            }
        case .needsVaultPassphrase:
            DashCard {
                Text(L10n.Common.passphraseRequired).dashFont(.footnote)
                DashSecureField(CrossStrings.passphrase, placeholder: CrossStrings.walletPassphrase, text: $passphrase)
                HStack(spacing: Int(DashSpacing.s)) {
                    DashButton(CrossStrings.continueTitle, style: .filledBlue, size: .small) {
                        let text = passphrase
                        passphrase = ""
                        Task { await model.provideVaultPassphrase(text) }
                    }
                    DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) {
                        passphrase = ""
                        model.dismiss()
                    }
                }
            }
        case .confirmingRemove(_, let name):
            ConfirmationCard(
                title: L10n.Wallets.removeTitle, message: L10n.Wallets.removeQuestion(name),
                confirmTitle: CrossStrings.remove, destructive: true,
                onConfirm: { Task { await model.confirmRemove() } }, onCancel: { model.dismiss() })
        case .imported:
            DashCard {
                ForEach(model.importSummary, id: \.self) { line in Text(line).dashFont(.footnote) }
                DashButton(CrossStrings.done, style: .tintedBlue, size: .small) { model.dismiss() }
            }
        case .restored:
            Toast(L10n.Wallets.walletRestored, kind: .success, actionTitle: CrossStrings.dismiss) { model.dismiss() }
        case .backedUp(let backup):
            Toast(
                "\(L10n.Wallets.backupSuccessful): \(L10n.Wallets.backupSaved(backup.file.path))", kind: .success,
                actionTitle: CrossStrings.dismiss
            ) { model.dismiss() }
        case .exported:
            DashCard {
                ForEach(model.exportSummary, id: \.self) { line in Text(line).dashFont(.footnote) }
                DashButton(CrossStrings.done, style: .tintedBlue, size: .small) { model.dismiss() }
            }
        case .openPSBT(let url):
            Toast(L10n.Wallets.psbtFile, kind: .info, actionTitle: L10n.PSBT.dialogTitle) {
                model.dismiss()
                state.open(.psbt)
                Task { await state.psbt.load(file: url) }
            }
        case .deletedAll:
            Toast(L10n.Security.wiped, kind: .success, actionTitle: CrossStrings.dismiss) { model.dismiss() }
        case .failed(let text):
            Toast(text, kind: .error, actionTitle: CrossStrings.dismiss) { model.dismiss() }
        }
    }

    // MARK: Wallets

    @ViewBuilder
    private func walletList(_ model: WalletManagementViewModel) -> some View {
        DashCard {
            HStack {
                SectionHeader(CrossStrings.walletsPage, style: .subheadMedium)
                Spacer()
                DashButton(L10n.Shell.closeAllWallets, style: .plainBlue, size: .small, isEnabled: model.wallets.contains(where: \.loaded)) {
                    Task { await model.closeAll() }
                }
            }
            if model.loadStatesUnavailable {
                Text(CrossStrings.loadStatesUnavailable).dashFont(.caption1).dashForeground(.secondaryText)
            }
            if model.wallets.isEmpty {
                Text(L10n.Shell.noWalletsAvailable).dashFont(.footnote).dashForeground(.secondaryText)
            }
            ForEach(model.wallets) { wallet in
                VStack(alignment: .leading, spacing: Int(DashSpacing.xxs)) {
                    HStack(spacing: Int(DashSpacing.s)) {
                        MenuItem(
                            title: wallet.name.isEmpty ? String(wallet.walletID.hex.prefix(8)) : wallet.name,
                            subtitle: CrossStrings.walletState(loaded: wallet.loaded, watchOnly: wallet.watchOnly))
                        if wallet.loaded {
                            DashButton(CrossStrings.closeWallet, style: .plainBlue, size: .small) {
                                Task { await model.close(wallet.walletID) }
                            }
                        } else {
                            DashButton(CrossStrings.openWallet, style: .plainBlue, size: .small) {
                                Task { await model.open(wallet.walletID) }
                            }
                        }
                        DashButton(CrossStrings.rename, style: .plainBlue, size: .small) {
                            renaming = wallet.walletID
                            renameText = wallet.name
                        }
                        DashButton(CrossStrings.xpub, style: .plainBlue, size: .small, isEnabled: wallet.loaded) {
                            Task { await model.loadXpub(wallet.walletID) }
                        }
                        DashButton(CrossStrings.remove, style: .plainRed, size: .small) {
                            model.requestRemove(wallet.walletID)
                        }
                    }
                    DashToggle(
                        CrossStrings.loadOnStartup(wallet.name),
                        isOn: bind({ wallet.loadOnStartup }, { value in
                            Task { await model.setLoadOnStartup(wallet.walletID, value) }
                        }))
                    if renaming == wallet.walletID {
                        HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                            DashTextField(CrossStrings.walletName, text: $renameText, width: 260)
                            DashButton(CrossStrings.save, style: .tintedBlue, size: .small, isEnabled: !renameText.isEmpty) {
                                let name = renameText
                                renaming = nil
                                Task { await model.rename(wallet.walletID, to: name) }
                            }
                            DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) { renaming = nil }
                        }
                    }
                }
            }
            DashButton(L10n.Shell.createWallet, style: .tintedBlue, size: .small) { state.open(.createWallet) }
        }
    }

    // MARK: Imports (QT-106…108, QT-110, QT-114)

    @ViewBuilder
    private func imports(_ model: WalletManagementViewModel) -> some View {
        DashCard {
            SectionHeader(CrossStrings.importSection, style: .subheadMedium)
            Text(CrossStrings.importFileHelp).dashFont(.caption1).dashForeground(.secondaryText)
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(CrossStrings.importFile, style: .tintedBlue, size: .small) {
                    let choose = chooseFile
                    Task {
                        if let url = await choose(title: CrossStrings.importFile, defaultButtonLabel: CrossStrings.open) {
                            await model.importFile(url)
                        }
                    }
                }
                DashButton(L10n.Shell.restoreWallet, style: .tintedBlue, size: .small) {
                    let choose = chooseFile
                    Task {
                        if let url = await choose(title: L10n.Shell.restoreWallet, defaultButtonLabel: CrossStrings.open) {
                            await model.restore(url, passphrase: nil)
                        }
                    }
                }
            }
            SectionHeader(CrossStrings.keyMaterial, style: .footnoteMedium)
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashPicker(
                    CrossStrings.keyMaterialKind,
                    options: KeyMaterialKind.allCases.map { PickerOption($0, CrossStrings.keyMaterialName($0)) },
                    selection: $keyKind)
                DashSecureField(CrossStrings.keyMaterial, text: $keyText)
                DashButton(CrossStrings.importTitle, style: .tintedBlue, size: .small, isEnabled: !keyText.isEmpty) {
                    let text = keyText
                    let kind = keyKind
                    keyText = ""
                    Task { await model.importKeyMaterial(kind, text: text) }
                }
            }
            SectionHeader(CrossStrings.watchOnlyWallet, style: .footnoteMedium)
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashTextField(CrossStrings.xpubField, text: $xpubText)
                DashTextField(CrossStrings.walletName, text: $watchName, width: 160)
                DashTextField(CrossStrings.birthHeightField, text: $watchBirthHeight, width: 140)
                DashButton(CrossStrings.add, style: .tintedBlue, size: .small, isEnabled: !xpubText.isEmpty) {
                    let (xpub, name, height) = (xpubText, watchName, UInt32(watchBirthHeight))
                    xpubText = ""
                    watchName = ""
                    watchBirthHeight = ""
                    Task { await model.addWatchOnly(xpub: xpub, name: name, birthHeight: height) }
                }
            }
        }
    }

    // MARK: Backups and exports (QT-109, QT-110, QT-116)

    @ViewBuilder
    private func backups(_ model: WalletManagementViewModel) -> some View {
        let selected = state.main.selectedWalletID
        DashCard {
            SectionHeader(L10n.Shell.backupWallet, style: .subheadMedium)
            if state.main.lockState == .unencrypted {
                DashSecureField(CrossStrings.backupPassphrase, text: $backupPassphrase)
            }
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(L10n.Shell.backupWallet, style: .tintedBlue, size: .small, isEnabled: selected != nil) {
                    guard let id = selected else { return }
                    let choose = chooseFileSaveDestination
                    let secret = backupPassphrase
                    backupPassphrase = ""
                    Task {
                        if let url = await choose(
                            title: L10n.Shell.backupWallet, defaultButtonLabel: CrossStrings.save,
                            defaultFileName: "wallet.dwbackup")
                        {
                            await model.backup(id, to: url, passphrase: secret.isEmpty ? nil : secret)
                        }
                    }
                }
                DashButton(L10n.Shell.showAutomaticBackups, style: .plainBlue, size: .small, isEnabled: model.backupDirectory != nil) {
                    model.showBackupsFolder()
                }
            }
            if !model.automaticBackups.isEmpty {
                SectionHeader(CrossStrings.automaticBackups, style: .footnoteMedium)
                ForEach(model.automaticBackups, id: \.self) { backup in
                    Text("\(Format.date(backup.createdAt))  \(backup.file.lastPathComponent)").dashFont(.caption1)
                }
            }
            SectionHeader(CrossStrings.exportForCore, style: .footnoteMedium)
            HStack(alignment: .bottom, spacing: Int(DashSpacing.s)) {
                DashPicker(
                    CrossStrings.exportFormat,
                    options: CoreExportFormat.allCases.map { PickerOption($0, CrossStrings.exportFormatName($0)) },
                    selection: $exportFormat)
                DashButton(CrossStrings.exportTitle, style: .tintedBlue, size: .small, isEnabled: selected != nil) {
                    guard let id = selected else { return }
                    let choose = chooseFileSaveDestination
                    let format = exportFormat
                    Task {
                        if let url = await choose(
                            title: CrossStrings.exportForCore, defaultButtonLabel: CrossStrings.save,
                            defaultFileName: format == .dumpWallet ? "wallet-dump.txt" : "descriptors.json")
                        {
                            await model.exportForCore(id, format: format, to: url)
                        }
                    }
                }
            }
        }
    }
}

/// File ▸ Create Wallet: the onboarding flow for one more wallet; Close
/// returns to the wallet.
struct CreateWalletScreen: View {
    let state: CrossAppState

    var body: some View {
        let state = state
        let model = state.createWalletFlow()
        VStack(alignment: .leading, spacing: Int(DashSpacing.s)) {
            HStack {
                Spacer()
                DashButton(CrossStrings.close, style: .strokeGray, size: .small) { state.closePage() }
            }
            .padding(.horizontal, Int(DashSpacing.xl))
            OnboardingScreen(model: model)
        }
    }
}

/// File ▸ Open URI (QT-019): a `dash:` URI opens the Send page pre-filled.
struct OpenURIScreen: View {
    let state: CrossAppState

    @State var uri = ""

    var body: some View {
        let state = state
        Page(L10n.Shell.openURI) {
            DashTextField(CrossStrings.uri, placeholder: CrossStrings.uriPlaceholder, text: $uri)
            if let error = state.main.errorMessage {
                Toast(error, kind: .error)
            }
            HStack(spacing: Int(DashSpacing.s)) {
                DashButton(CrossStrings.ok, style: .filledBlue, size: .small, isEnabled: !uri.isEmpty) {
                    let text = uri.trimmingCharacters(in: .whitespacesAndNewlines)
                    Task {
                        state.closePage()
                        await state.main.open(uri: text)
                    }
                }
                DashButton(CrossStrings.cancel, style: .strokeGray, size: .small) { state.closePage() }
            }
        }
    }
}
