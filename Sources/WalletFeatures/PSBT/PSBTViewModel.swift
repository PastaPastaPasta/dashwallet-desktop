// PSBT controls (QT-076…079): Create Unsigned from a send draft, load from a
// file or the clipboard, and dash-qt's PSBT Operations dialog (sign,
// broadcast, copy, save).
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// The PSBT Operations flow.
public enum PSBTStep: Sendable, Hashable {
    case empty
    case loading
    case ready
    /// Signing waits for the wallet passphrase.
    case needsPassphrase
    case signing
    case broadcasting
    case broadcast(txid: String)
}

@MainActor
@Observable
public final class PSBTViewModel {
    public private(set) var step: PSBTStep = .empty
    public private(set) var reference: PSBTReference?
    public private(set) var analysis: PSBTAnalysis?
    /// dash-qt's result line after sign, broadcast, copy or save.
    public private(set) var message: String?
    public private(set) var errorMessage: String?

    /// The dialog's description lines (dash-qt `renderTransaction`): one per
    /// output with amounts in DASH, the fee, the total in the display unit
    /// and the unsigned-input count.
    public var descriptionLines: [String] {
        guard let analysis else { return [] }
        var lines = analysis.outputs.map { output in
            var line = L10n.PSBT.sends(
                format(output.amount, unit: .dash), to: output.address ?? L10n.PSBT.unknownAddress)
            if output.isMine { line += " (\(L10n.PSBT.ownAddress))" }
            return line
        }
        if let fee = analysis.fee {
            lines.append(L10n.PSBT.paysFee(format(fee, unit: .dash)))
            if let total = analysis.total {
                lines.append("\(L10n.PSBT.totalAmount): \(format(total, unit: settings.display.unit))")
            }
        } else {
            lines.append(L10n.PSBT.feeUnknown)
        }
        if analysis.unsignedInputs > 0 { lines.append(L10n.PSBT.unsignedInputs(analysis.unsignedInputs)) }
        return lines
    }

    /// dash-qt's status line for the PSBT's role and this wallet.
    public var statusLine: String? {
        guard let analysis else { return nil }
        switch analysis.status {
        case .missingInputInfo:
            return L10n.PSBT.missingInputInfo
        case .needsSignatures:
            switch analysis.signability {
            case .noWallet: return "\(L10n.PSBT.needsSignatures) \(L10n.PSBT.noWallet)"
            case .watchOnly: return "\(L10n.PSBT.needsSignatures) \(L10n.PSBT.cannotSign)"
            case .noMatchingKeys: return "\(L10n.PSBT.needsSignatures) \(L10n.PSBT.noMatchingKeys)"
            case .canSign: return L10n.PSBT.needsSignatures
            }
        case .complete:
            return L10n.PSBT.complete
        }
    }

    /// "Sign Tx" is enabled while this wallet can add signatures.
    public var canSign: Bool {
        guard step == .ready, let analysis else { return false }
        return analysis.status == .needsSignatures && analysis.signability == .canSign
    }

    /// "Broadcast Tx" is enabled once fully signed.
    public var canBroadcast: Bool { step == .ready && analysis?.status == .complete }

    /// dash-qt's Save suggestion: `<address>-<amount>` per paid output, `.psbt`.
    public var suggestedFileName: String {
        let paid = (analysis?.outputs ?? []).filter { !$0.isMine }
        let parts = paid.map { "\($0.address ?? L10n.PSBT.unknownAddress)-\(format($0.amount, unit: settings.display.unit))" }
        return (parts.isEmpty ? "transaction" : parts.joined(separator: " - ")) + ".psbt"
    }

    private let psbt: any PSBTHandling
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let walletState: any WalletStateProviding
    private let clipboard: any ClipboardProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding

    public init(
        psbt: any PSBTHandling, auth: any AuthenticationGating, vault: any VaultProviding,
        walletState: any WalletStateProviding, clipboard: any ClipboardProviding, amounts: any AmountFormatting,
        settings: any SettingsProviding
    ) {
        self.psbt = psbt
        self.auth = auth
        self.vault = vault
        self.walletState = walletState
        self.clipboard = clipboard
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(
            psbt: m2.psbt, auth: env.auth, vault: env.vault, walletState: env.walletState, clipboard: m2.clipboard,
            amounts: env.amounts, settings: env.settings)
    }

    // MARK: Create and load

    /// "Create Unsigned" (QT-077): the base64 PSBT goes to the clipboard at
    /// once; the dialog then offers Save.
    public func createUnsigned(draft: any TransactionDrafting) async {
        await open { () async throws(ServiceError) -> PSBTReference in
            try await psbt.createUnsigned(from: draft)
        }
        guard let reference, step == .ready else { return }
        do {
            clipboard.setString(try psbt.base64(reference))
            message = L10n.PSBT.unsignedCopied
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// File ▸ Load PSBT from file: binary or base64, under 100 MiB.
    public func load(file url: URL) async {
        let data: Data
        do {
            data = try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            errorMessage = L10n.PSBT.loadFailed(L10n.M2Errors.fileUnreadable)
            return
        }
        await load(data: data)
    }

    /// Bytes from a file or drag and drop.
    public func load(data: Data) async {
        await open { () throws(ServiceError) -> PSBTReference in try psbt.load(data) }
    }

    /// File ▸ Load PSBT from clipboard: base64 text only.
    public func loadFromClipboard() async {
        guard let text = clipboard.string()?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty,
            Data(base64Encoded: text) != nil
        else {
            errorMessage = L10n.PSBT.clipboardInvalid
            return
        }
        await load(data: Data(text.utf8))
    }

    // MARK: Operations

    /// "Sign Tx" with a `.spend(max: total)` grant: what leaves the wallet,
    /// fee included (the engine caps that, review H1). An encrypted vault
    /// needs `passphrase`; without it the step becomes `.needsPassphrase`.
    public func sign(passphrase: String? = nil) async {
        guard step == .ready || step == .needsPassphrase, let reference, let analysis,
            let wallet = walletState.selectedWalletID
        else { return }
        // `nil` while the fee is unknown; the engine refuses to sign then.
        guard let outflow = analysis.total else {
            errorMessage = L10n.PSBT.noExternalAmount
            return
        }
        let purpose = GrantPurpose.spend(max: outflow)
        let credential: Credential
        switch auth.requirement(for: purpose) {
        case .none:
            credential = .unencrypted
        case .passphrase, .quickUnlockOrPassphrase:
            guard let passphrase else {
                step = .needsPassphrase
                return
            }
            credential = .passphrase(vault.makeSecret(utf8: passphrase))
        }
        step = .signing
        errorMessage = nil
        do {
            let grant = try await auth.authorize(purpose, wallet: wallet, credential: credential)
            let signed = try await psbt.sign(reference, wallet: wallet, grant: grant)
            let after = try await psbt.analyze(signed, wallet: wallet)
            psbt.release(reference)
            self.reference = signed
            self.analysis = after
            if after.status == .complete {
                message = L10n.PSBT.signedComplete
            } else if after.unsignedInputs < analysis.unsignedInputs {
                message = L10n.PSBT.signedPartially(analysis.unsignedInputs - after.unsignedInputs)
            } else {
                message = L10n.PSBT.couldNotSign
            }
            step = .ready
        } catch {
            errorMessage = ErrorText.m2(error.code)
            step = error.code == .vaultWrongPassphrase ? .needsPassphrase : .ready
        }
    }

    public func cancelPassphrase() {
        if step == .needsPassphrase { step = .ready }
    }

    /// "Broadcast Tx" (rates above 0.1 DASH/kB are refused by the engine).
    public func broadcast() async {
        guard canBroadcast, let reference else { return }
        step = .broadcasting
        errorMessage = nil
        do {
            let txid = try await psbt.broadcast(reference)
            message = L10n.PSBT.broadcastSucceeded(txid)
            step = .broadcast(txid: txid)
        } catch {
            message = nil
            errorMessage = L10n.PSBT.broadcastFailed(ErrorText.m2(error.code))
            step = .ready
        }
    }

    /// "Copy to Clipboard": base64.
    public func copy() {
        guard let reference else { return }
        do {
            clipboard.setString(try psbt.base64(reference))
            message = L10n.PSBT.copied
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// "Save…": the binary PSBT, never replacing an existing file.
    public func save(to url: URL) {
        guard let reference else { return }
        do {
            let data = try psbt.bytes(reference)
            do {
                try PrivateFileSystem.writeFile(data, to: url, replacing: false)
                message = L10n.PSBT.saved
            } catch {
                errorMessage = L10n.M2Errors.destinationUnwritable
            }
        } catch {
            errorMessage = ErrorText.m2(error.code)
        }
    }

    /// "Close": drops the engine object.
    public func close() {
        if let reference { psbt.release(reference) }
        reference = nil
        analysis = nil
        message = nil
        errorMessage = nil
        step = .empty
    }

    // MARK: Private

    private func open(_ make: () async throws(ServiceError) -> PSBTReference) async {
        let previous = reference
        step = .loading
        message = nil
        errorMessage = nil
        let next: PSBTReference
        do {
            next = try await make()
        } catch {
            fail(error, previous: previous)
            return
        }
        do {
            let result = try await psbt.analyze(next, wallet: walletState.selectedWalletID)
            if let previous { psbt.release(previous) }
            reference = next
            analysis = result
            step = .ready
        } catch {
            psbt.release(next)
            fail(error, previous: previous)
        }
    }

    private func fail(_ error: ServiceError, previous: PSBTReference?) {
        errorMessage = error.code == .psbtTooLarge || error.code == .psbtInvalid
            ? L10n.PSBT.loadFailed(ErrorText.m2(error.code)) : ErrorText.m2(error.code)
        step = previous == nil ? .empty : .ready
    }

    private func format(_ amount: Amount, unit: DisplayUnit) -> String {
        amounts.format(amount, unit: unit, style: .withUnit(plusSign: false, separators: .standard))
    }
}
