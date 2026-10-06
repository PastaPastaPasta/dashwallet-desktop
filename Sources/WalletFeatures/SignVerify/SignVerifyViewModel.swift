// Sign / Verify message (QT-099/100).
import Foundation
import Observation
import WalletRuntime

public enum SignVerifyResult: Sendable, Equatable {
    case signed
    case verified
    case failed(ServiceErrorCode, message: String)

    /// dash-qt shows success in green and failures in red.
    public var isSuccess: Bool {
        if case .failed = self { return false }
        return true
    }

    public var text: String {
        switch self {
        case .signed: L10n.SignVerify.signed
        case .verified: L10n.SignVerify.verified
        case .failed(_, let message): message
        }
    }
}

@MainActor
@Observable
public final class SignVerifyViewModel {
    // Sign page.
    public var address = ""
    public var message = ""
    public private(set) var signature = ""
    /// Sign needs the wallet passphrase; call `sign(passphrase:)` with it.
    public private(set) var needsPassphrase = false
    public private(set) var signResult: SignVerifyResult?

    // Verify page.
    public var verifyAddress = ""
    public var verifyMessage = ""
    public var verifySignature = ""
    public private(set) var verifyResult: SignVerifyResult?

    /// The latest result of either page.
    public var result: SignVerifyResult? { verifyResult ?? signResult }

    private let walletState: any WalletStateProviding
    private let messages: any MessageSigning
    private let auth: any AuthenticationGating
    private let vault: any VaultProviding
    private let uri: any URIHandling
    private let network: DashNetwork

    public init(
        walletState: any WalletStateProviding, messages: any MessageSigning, auth: any AuthenticationGating,
        vault: any VaultProviding, uri: any URIHandling, network: DashNetwork
    ) {
        self.walletState = walletState
        self.messages = messages
        self.auth = auth
        self.vault = vault
        self.uri = uri
        self.network = network
    }

    public convenience init(env: AppEnvironment, network: DashNetwork) {
        self.init(
            walletState: env.walletState, messages: env.messages, auth: env.auth, vault: env.vault, uri: env.uri,
            network: network)
    }

    /// Signs `message` with the key of `address` (P2PKH only). Needs a full
    /// unlock: without `passphrase` on an encrypted wallet it sets
    /// `needsPassphrase` and returns.
    public func sign(passphrase: String? = nil) async {
        verifyResult = nil
        signature = ""
        let cleaned = AddressInput.clean(address)
        if let problem = Self.p2pkhProblem(cleaned, uri: uri, network: network) {
            signResult = problem
            return
        }
        guard let wallet = walletState.selectedWalletID else {
            signResult = .failed(.walletNotFound, message: L10n.Common.noWallet)
            return
        }
        let credential: Credential
        switch auth.requirement(for: .signMessage) {
        case .none:
            credential = .unencrypted
        case .passphrase, .quickUnlockOrPassphrase:
            guard let passphrase else {
                needsPassphrase = true
                return
            }
            credential = .passphrase(vault.makeSecret(utf8: passphrase))
        }
        needsPassphrase = false
        do {
            let grant = try await auth.authorize(.signMessage, wallet: wallet, credential: credential)
            signature = try await messages.sign(wallet: wallet, address: cleaned, message: message, grant: grant)
            signResult = .signed
        } catch {
            signResult = .failed(error.code, message: Self.signText(error.code))
        }
    }

    /// Passphrase prompt dismissed without unlocking.
    public func cancelPassphrase() {
        needsPassphrase = false
        signResult = .failed(.vaultLocked, message: L10n.Common.unlockCancelled)
    }

    /// Verifies; no unlock needed (QT-100).
    public func verify() {
        signResult = nil
        let cleaned = AddressInput.clean(verifyAddress)
        if let problem = Self.p2pkhProblem(cleaned, uri: uri, network: network) {
            verifyResult = problem
            return
        }
        do {
            try messages.verify(address: cleaned, message: verifyMessage, signature: verifySignature)
            verifyResult = .verified
        } catch {
            verifyResult = .failed(error.code, message: Self.verifyText(error.code))
        }
    }

    /// The status line clears when a field gets focus (dash-qt).
    public func clearStatus() {
        signResult = nil
        verifyResult = nil
    }

    public func clearSign() {
        address = ""
        message = ""
        signature = ""
        signResult = nil
        needsPassphrase = false
    }

    public func clearVerify() {
        verifyAddress = ""
        verifyMessage = ""
        verifySignature = ""
        verifyResult = nil
    }

    // MARK: Private

    private static func p2pkhProblem(_ address: String, uri: any URIHandling, network: DashNetwork) -> SignVerifyResult? {
        switch AddressInput.checkCore(address, uri: uri, network: network) {
        case .success(.core(scriptHash: false)):
            return nil
        case .success:
            return .failed(EngineCode.messageAddressNoKey, message: L10n.SignVerify.addressNoKey)
        case .failure:
            return .failed(EngineCode.messageInvalidAddress, message: L10n.SignVerify.invalidAddress)
        }
    }

    static func signText(_ code: ServiceErrorCode) -> String {
        switch code {
        case EngineCode.messageInvalidAddress: L10n.SignVerify.invalidAddress
        case EngineCode.messageAddressNoKey: L10n.SignVerify.addressNoKey
        case EngineCode.messageAddressNotMine, EngineCode.messageWatchOnly, .vaultNoSecret:
            L10n.SignVerify.privateKeyUnavailable
        case .vaultWrongPassphrase: L10n.Common.wrongPassphrase
        case .vaultLocked, .vaultMixingOnly, .vaultGrantInvalid, EngineCode.messageVaultLocked,
            EngineCode.messageGrantInvalid:
            L10n.Common.unlockCancelled
        case .vaultThrottled, .notImplemented, .networkNotOpen, .walletNotFound: ErrorText.common(code)
        default: L10n.SignVerify.signingFailed
        }
    }

    static func verifyText(_ code: ServiceErrorCode) -> String {
        switch code {
        case EngineCode.messageInvalidAddress: L10n.SignVerify.invalidAddress
        case EngineCode.messageAddressNoKey: L10n.SignVerify.addressNoKey
        case EngineCode.messageMalformedSignature: L10n.SignVerify.malformedSignature
        case EngineCode.messagePubkeyNotRecovered: L10n.SignVerify.digestMismatch
        case .notImplemented: ErrorText.common(code)
        default: L10n.SignVerify.verificationFailed
        }
    }
}
