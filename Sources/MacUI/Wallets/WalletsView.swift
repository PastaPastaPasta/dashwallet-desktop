// Wallets window (IOS-110, IOS-111, QT-101, QT-106…110, QT-114, QT-116):
// every wallet with its load state, open/close, load on startup, rename,
// remove; imports (Dash Core dumpwallet / wallet.dat, .dwbackup, key
// material, watch-only xpub), backups, Dash Core export, the account xpub
// with its QR, and the automatic backups. Also the IOS-009 "Wallets found on
// this device" question.
#if os(macOS)
import DashUIMac
import DesignTokens
import PlatformServicesMac
import SwiftUI
import WalletFeatures
import WalletRuntime

struct WalletsWindow: View {
    let model: MacAppModel

    var body: some View {
        if let wallets = model.features?.wallets {
            WalletsView(wallets: wallets, selectedWalletID: model.main?.selectedWalletID)
        } else {
            Text(model.unavailableReason ?? L10n.Options.unavailable).padding(DashSpacing.xl)
        }
    }
}

/// The sheets the window opens.
private enum WalletsSheet: String, Identifiable {
    case rename, keyMaterial, watchOnly, backup, export, xpub
    var id: String { rawValue }
}

struct WalletsView: View {
    let wallets: WalletManagementViewModel
    var selectedWalletID: WalletID?
    @State private var selection: WalletID?
    @State private var sheet: WalletsSheet?

    private var selected: WalletLoadState? { wallets.wallets.first { $0.walletID == selection } }

    var body: some View {
        VStack(spacing: 0) {
            FlowBanner(wallets: wallets)
            List(selection: $selection) {
                Section(MacStrings.Wallets.walletsHeader) {
                    ForEach(wallets.wallets) { state in
                        WalletRow(state: state, onLoadOnStartup: { value in
                            Task { await wallets.setLoadOnStartup(state.walletID, value) }
                        })
                        .tag(state.walletID)
                    }
                    if wallets.wallets.isEmpty {
                        Text(MacStrings.Wallets.none).foregroundStyle(Color.dash.secondaryText)
                    }
                }
                if !wallets.automaticBackups.isEmpty {
                    Section(L10n.Shell.showAutomaticBackups) {
                        ForEach(wallets.automaticBackups, id: \.file) { backup in
                            HStack {
                                Text(backup.file.lastPathComponent).lineLimit(1).truncationMode(.middle)
                                Spacer()
                                Text(backup.createdAt.formatted(date: .abbreviated, time: .shortened))
                                    .foregroundStyle(Color.dash.secondaryText)
                            }
                            .dashFont(.footnote)
                        }
                    }
                }
            }
            .listStyle(.inset)
            .accessibilityIdentifier("wallets.list")
            if wallets.loadStatesUnavailable {
                Text(MacStrings.Wallets.loadStatesUnavailable)
                    .dashFont(.footnote)
                    .foregroundStyle(Color.dash.secondaryText)
                    .padding(.horizontal, DashSpacing.l)
            }
            Divider()
            actions
        }
        .frame(minWidth: 640, minHeight: 440)
        .task {
            if selection == nil { selection = selectedWalletID }
            wallets.start()
            await wallets.load()
        }
        .onDisappear { wallets.stop() }
        .sheet(item: $sheet) { sheet in
            sheetContent(sheet)
        }
        .alert(L10n.Wallets.removeTitle, isPresented: removeBinding) {
            Button(MacStrings.Common.cancel, role: .cancel) { wallets.dismiss() }
            Button(MacStrings.Wallets.remove, role: .destructive) { Task { await wallets.confirmRemove() } }
        } message: {
            if case .confirmingRemove(_, let name) = wallets.flow { Text(L10n.Wallets.removeQuestion(name)) }
        }
        .accessibilityIdentifier("wallets")
    }

    private var removeBinding: Binding<Bool> {
        Binding(get: { if case .confirmingRemove = wallets.flow { true } else { false } }, set: { _ in })
    }

    private var actions: some View {
        HStack(spacing: DashSpacing.s) {
            Menu(MacStrings.Wallets.add) {
                Button(MacStrings.Wallets.importFile) { Task { await importFile() } }
                Button(MacStrings.Wallets.importKeyMaterial) { sheet = .keyMaterial }
                Button(MacStrings.Wallets.addWatchOnly) { sheet = .watchOnly }
            }
            .fixedSize()
            .accessibilityIdentifier("wallets.add")
            if let selected {
                Button(selected.loaded ? MacStrings.Wallets.close : MacStrings.Wallets.open) {
                    Task {
                        if selected.loaded { await wallets.close(selected.walletID) } else { await wallets.open(selected.walletID) }
                    }
                }
                .accessibilityIdentifier("wallets.openClose")
                Menu(MacStrings.Wallets.more) {
                    Button(MacStrings.Wallets.rename) { sheet = .rename }
                    Button(L10n.Shell.backupWallet) { sheet = .backup }
                    Button(MacStrings.Wallets.exportForCore) { sheet = .export }
                    Button(L10n.Wallets.xpubTitle) {
                        sheet = .xpub
                        Task { await wallets.loadXpub(selected.walletID) }
                    }
                    Divider()
                    Button(MacStrings.Wallets.remove, role: .destructive) { wallets.requestRemove(selected.walletID) }
                }
                .fixedSize()
            }
            Spacer()
            Button(L10n.Shell.showAutomaticBackups) { wallets.showBackupsFolder() }
            Button(MacStrings.Wallets.closeAll) { Task { await wallets.closeAll() } }
                .disabled(!wallets.wallets.contains(where: \.loaded))
        }
        .padding(DashSpacing.m)
    }

    @ViewBuilder
    private func sheetContent(_ sheet: WalletsSheet) -> some View {
        let close = { self.sheet = nil }
        switch sheet {
        case .rename:
            if let selected {
                TextPromptSheet(
                    title: MacStrings.Wallets.rename, label: MacStrings.Wallets.name, initial: selected.name,
                    onCancel: close
                ) { name in
                    close()
                    Task { await wallets.rename(selected.walletID, to: name) }
                }
            }
        case .keyMaterial:
            KeyMaterialSheet(wallets: wallets, onClose: close)
        case .watchOnly:
            WatchOnlySheet(wallets: wallets, onClose: close)
        case .backup:
            BackupWalletSheet(
                wallets: wallets, walletID: selected?.walletID, walletName: selected?.name ?? "", onClose: close)
        case .export:
            if let selected { CoreExportSheet(wallets: wallets, wallet: selected, onClose: close) }
        case .xpub:
            XpubSheet(wallets: wallets, onClose: close)
        }
    }

    private func importFile() async {
        guard let url = await MacOpenPanel.chooseFile(title: MacStrings.Wallets.importFile) else { return }
        await wallets.importFile(url)
    }
}

private struct WalletRow: View {
    let state: WalletLoadState
    let onLoadOnStartup: (Bool) -> Void

    var body: some View {
        HStack(spacing: DashSpacing.m) {
            Image(systemName: state.watchOnly ? "eye" : "wallet.pass")
                .foregroundStyle(Color.dash.secondaryText)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                Text(state.name.isEmpty ? MacStrings.Wallets.unnamed : state.name).dashFont(.subheadMedium)
                Text(state.walletID.hex.prefix(16) + "…")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(Color.dash.secondaryText)
            }
            Spacer()
            if state.watchOnly { Badge(MacStrings.Wallets.watchOnly, tone: .info) }
            Badge(state.loaded ? MacStrings.Wallets.loaded : MacStrings.Wallets.notLoaded,
                  tone: state.loaded ? .success : .neutral)
            Toggle(MacStrings.Wallets.loadOnStartup, isOn: Binding(get: { state.loadOnStartup }, set: onLoadOnStartup))
                .toggleStyle(.checkbox)
        }
        .padding(.vertical, DashSpacing.xxxs)
        .accessibilityIdentifier("wallets.row.\(state.name)")
    }
}

/// The running operation, its result, or what it waits for.
private struct FlowBanner: View {
    let wallets: WalletManagementViewModel
    @State private var passphrase = ""

    var body: some View {
        Group {
            switch wallets.flow {
            case .idle, .confirmingRemove:
                if let error = wallets.errorMessage { banner(SystemNotice(text: error, tone: .error)) }
            case .working:
                banner(HStack { ProgressView().controlSize(.small); Text(MacStrings.Wallets.working) })
            case .needsFilePassphrase, .needsVaultPassphrase:
                banner(HStack {
                    SecureField(MacStrings.Common.passphrase, text: $passphrase)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("wallets.passphrase")
                    Button(MacStrings.Common.cancel) { wallets.dismiss() }
                    Button(MacStrings.Common.ok) {
                        let text = passphrase
                        passphrase = ""
                        Task {
                            if case .needsFilePassphrase = wallets.flow {
                                await wallets.provideFilePassphrase(text)
                            } else {
                                await wallets.provideVaultPassphrase(text)
                            }
                        }
                    }
                    .disabled(passphrase.isEmpty)
                })
            case .imported:
                result(wallets.importSummary.joined(separator: "\n"), tone: .info)
            case .restored:
                result(L10n.Wallets.walletRestored, tone: .info)
            case .backedUp(let backup):
                result(L10n.Wallets.backupSaved(backup.file.path), tone: .info)
            case .exported:
                result(wallets.exportSummary.joined(separator: "\n"), tone: .info)
            case .openPSBT:
                result(L10n.Wallets.psbtFile, tone: .info)
            case .deletedAll:
                result(L10n.Security.wiped, tone: .info)
            case .failed(let reason):
                result(reason, tone: .error)
            }
        }
        .accessibilityIdentifier("wallets.flow")
    }

    private func banner(_ content: some View) -> some View {
        content.padding(DashSpacing.m).frame(maxWidth: .infinity, alignment: .leading)
    }

    private func result(_ text: String, tone: DashTone) -> some View {
        banner(HStack(alignment: .top) {
            SystemNotice(text: text, tone: tone)
            Button(MacStrings.Common.ok) { wallets.dismiss() }
        })
    }
}

/// A one-field prompt (rename).
struct TextPromptSheet: View {
    let title: String
    let label: String
    let initial: String
    let onCancel: () -> Void
    let onSave: (String) -> Void
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(title).dashFont(.title3)
            TextField(label, text: $text).textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onCancel).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.save) { onSave(text) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(text.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 420)
        .onAppear { text = initial }
    }
}

/// HD seed hex, xprv or descriptors pasted from Dash Core (QT-108).
private struct KeyMaterialSheet: View {
    let wallets: WalletManagementViewModel
    let onClose: () -> Void
    @State private var kind: KeyMaterialKind = .hdSeed
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Wallets.importKeyMaterial).dashFont(.title3)
            Picker(MacStrings.Wallets.kind, selection: $kind) {
                Text(MacStrings.Wallets.hdSeed).tag(KeyMaterialKind.hdSeed)
                Text(MacStrings.Wallets.xprv).tag(KeyMaterialKind.xprv)
                Text(MacStrings.Wallets.descriptors).tag(KeyMaterialKind.descriptors)
            }
            .pickerStyle(.segmented)
            TextEditor(text: $text)
                .font(.system(.footnote, design: .monospaced))
                .frame(height: 100)
                .border(Color.dash.gray300Alpha40)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel) {
                    text = ""
                    onClose()
                }
                .keyboardShortcut(.cancelAction)
                Button(MacStrings.Wallets.importAction) {
                    let material = text
                    text = ""
                    onClose()
                    Task { await wallets.importKeyMaterial(kind, text: material) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(text.isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 520)
    }
}

/// IOS-111 / QT-114: a watch-only wallet from an account xpub.
private struct WatchOnlySheet: View {
    let wallets: WalletManagementViewModel
    let onClose: () -> Void
    @State private var xpub = ""
    @State private var name = ""
    @State private var birthHeight = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Wallets.addWatchOnly).dashFont(.title3)
            TextField(L10n.Wallets.xpubTitle, text: $xpub)
                .textFieldStyle(.roundedBorder)
                .font(.system(.footnote, design: .monospaced))
            TextField(MacStrings.Wallets.name, text: $name).textFieldStyle(.roundedBorder)
            TextField(L10n.Tools.birthHeight, text: $birthHeight).textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Wallets.importAction) {
                    let (key, label, height) = (xpub, name, UInt32(birthHeight))
                    onClose()
                    Task { await wallets.addWatchOnly(xpub: key, name: label.isEmpty ? nil : label, birthHeight: height) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(xpub.isEmpty)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 520)
    }
}

/// QT-109: a dumpwallet file or importdescriptors JSON for Dash Core.
private struct CoreExportSheet: View {
    let wallets: WalletManagementViewModel
    let wallet: WalletLoadState
    let onClose: () -> Void
    @State private var format: CoreExportFormat = .dumpWallet

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(MacStrings.Wallets.exportForCore).dashFont(.title3)
            Picker(MacStrings.Wallets.format, selection: $format) {
                Text(MacStrings.Wallets.dumpWallet).tag(CoreExportFormat.dumpWallet)
                Text(MacStrings.Wallets.descriptorsJSON).tag(CoreExportFormat.importDescriptorsJSON)
            }
            .pickerStyle(.radioGroup)
            SystemNotice(text: MacStrings.Wallets.exportWarning, tone: .warning)
            HStack {
                Spacer()
                Button(MacStrings.Common.cancel, action: onClose).keyboardShortcut(.cancelAction)
                Button(MacStrings.Common.saveEllipsis) {
                    Task {
                        let name = format == .dumpWallet ? "\(wallet.name).txt" : "\(wallet.name).json"
                        guard let url = await MacSavePanel.chooseDestination(
                            suggestedName: name, title: MacStrings.Wallets.exportForCore)
                        else { return }
                        onClose()
                        await wallets.exportForCore(wallet.walletID, format: format, to: url)
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 480)
    }
}

/// The account xpub and its QR (IOS-111).
private struct XpubSheet: View {
    let wallets: WalletManagementViewModel
    let onClose: () -> Void

    var body: some View {
        VStack(spacing: DashSpacing.m) {
            Text(L10n.Wallets.xpubTitle).dashFont(.title3)
            if let xpub = wallets.xpub {
                if let qr = wallets.xpubQR {
                    QRView(size: qr.size, modules: qr.modules, accessibilityLabel: L10n.Wallets.xpubTitle)
                        .frame(width: 220, height: 220)
                }
                Text(xpub.derivationPath).dashFont(.footnote).foregroundStyle(Color.dash.secondaryText)
                Text(xpub.xpub)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
                    .multilineTextAlignment(.center)
                Button(MacStrings.Common.copy) { MacPasteboard.copy(xpub.xpub) }
            } else if let error = wallets.errorMessage {
                SystemNotice(text: error, tone: .error)
            } else if case .failed(let reason) = wallets.flow {
                SystemNotice(text: reason, tone: .error)
            } else {
                ProgressView()
            }
            Button(MacStrings.Common.close, action: onClose).keyboardShortcut(.cancelAction)
        }
        .padding(DashSpacing.xl)
        .frame(width: 460)
    }
}

/// IOS-009: wallet data from an earlier install, while no wallet is open.
struct ExistingDataSheet: View {
    let wallets: WalletManagementViewModel
    @State private var confirmingDelete = false
    @State private var sentence = ""
    @State private var passphrase = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DashSpacing.m) {
            Text(L10n.Wallets.existingTitle).dashFont(.title3)
            Text(L10n.Wallets.existingMessage).dashFont(.subhead)
            ForEach(wallets.existingData, id: \.directory) { info in
                Text("\(L10n.Settings.networkName(info.network)): \(info.directory.path)")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(Color.dash.secondaryText)
            }
            if confirmingDelete {
                Text(L10n.Wallets.deleteAllMessage).dashFont(.footnote)
                Text("“\(L10n.Wallets.wipeAcceptPhrase)”").dashFont(.footnoteMedium)
                TextField(L10n.Wallets.typeSentence, text: $sentence, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                SecureField(MacStrings.Common.passphrase, text: $passphrase).textFieldStyle(.roundedBorder)
            }
            if case .failed(let reason) = wallets.flow { SystemNotice(text: reason, tone: .error) }
            HStack {
                Spacer()
                if confirmingDelete {
                    Button(MacStrings.Common.cancel) { confirmingDelete = false }
                    Button(L10n.Wallets.deleteAll, role: .destructive) {
                        let (text, secret) = (sentence, passphrase)
                        passphrase = ""
                        Task { await wallets.deleteAll(acceptance: text, passphrase: secret.isEmpty ? nil : secret) }
                    }
                    .disabled(sentence.isEmpty)
                } else {
                    Button(L10n.Wallets.deleteAll, role: .destructive) { confirmingDelete = true }
                    Button(L10n.Wallets.keepWallets) { wallets.keepExistingData() }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(DashSpacing.xl)
        .frame(width: 520)
        .accessibilityIdentifier("existingData")
    }
}
#endif
