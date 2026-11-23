// Adapters for message signing, `dash:` URIs / QR and amount formatting.
// Verification, URIs, QR and units are the engine's pure functions
// (`CoreFunctions`). In the live runtime they use the open network, else the
// last open one, else the fallback the composition root chose; hosts without
// an engine session (demo mode) pass their own network (`EngineFunctions`).
import DashKit
import Foundation

/// `MessageSigning`: signing through the open session's vault, verification
/// through the engine's pure `verify_message` (QT-099/100).
public final class MessageService: MessageSigning {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func sign(wallet: WalletID, address: String, message: String, grant: AuthGrant) async throws(ServiceError)
        -> String
    {
        guard case .signMessage = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "signing a message needs a signMessage grant")
        }
        let id = try wallet.kit
        let grantID = grant.id
        return try await context.call { engine, network throws(DashKitError) in
            try await engine.signMessage(on: network, wallet: id, address: address, message: message, grantID: grantID)
        }
    }

    public func verify(address: String, message: String, signature: String) throws(ServiceError) {
        try EngineFunctions.verifyMessage(
            address: address, message: message, signature: signature, naming: context.namingNetwork)
    }
}

/// `URIHandling` over the engine's dw-uri functions.
public final class URIService: URIHandling {
    private let namingNetwork: @Sendable () -> DashKit.DashNetwork

    init(context: EngineContext) {
        namingNetwork = { context.namingNetwork }
    }

    init(namingNetwork: @escaping @Sendable () -> DashKit.DashNetwork) {
        self.namingNetwork = namingNetwork
    }

    public func parsePaymentURI(_ text: String) throws(ServiceError) -> PaymentURI {
        let network = namingNetwork()
        return PaymentURI(try serviceCall { () throws(DashKitError) in
            try CoreFunctions.parsePaymentURI(text, network: network)
        })
    }

    public func buildPaymentURI(address: String, amount: Amount?, label: String?, message: String?)
        throws(ServiceError) -> String
    {
        let kitAmount = amount?.kit
        return try serviceCall { () throws(DashKitError) in
            try CoreFunctions.buildPaymentURI(address: address, amount: kitAmount, label: label, message: message)
        }
    }

    public func classifyAddress(_ text: String) -> AddressClass {
        AddressClass(CoreFunctions.classifyAddress(text, network: namingNetwork()))
    }

    public func qrMatrix(for text: String) throws(ServiceError) -> QRMatrix {
        let matrix = try serviceCall { () throws(DashKitError) in try CoreFunctions.qrMatrix(for: text) }
        return QRMatrix(size: matrix.size, modules: matrix.modules)
    }
}

/// `AmountFormatting` over the engine's dw-units functions (dash-qt
/// `BitcoinUnits`), so Swift and Rust format amounts identically.
public final class EngineAmountFormatter: AmountFormatting {
    private let namingNetwork: @Sendable () -> DashKit.DashNetwork

    init(context: EngineContext) {
        namingNetwork = { context.namingNetwork }
    }

    init(namingNetwork: @escaping @Sendable () -> DashKit.DashNetwork) {
        self.namingNetwork = namingNetwork
    }

    /// dw-units rejects only out-of-range digit counts, which the style
    /// conversion clamps to 0...8. Should the engine still refuse, the amount
    /// is shown exactly, in duffs, rather than as an empty string.
    public func format(_ amount: Amount, unit: DisplayUnit, style: AmountStyle) -> String {
        let network = namingNetwork()
        do {
            return try CoreFunctions.formatAmount(amount.kit, unit: unit.kit, network: network, style: style.kit)
        } catch {
            return "\(amount.duffs) \(CoreFunctions.unitName(.duffs, network: network))"
        }
    }

    public func parse(_ text: String, unit: DisplayUnit) throws(ServiceError) -> Amount {
        Amount(try serviceCall { () throws(DashKitError) in try CoreFunctions.parseAmount(text, unit: unit.kit) })
    }

    public func unitName(_ unit: DisplayUnit) -> String {
        CoreFunctions.unitName(unit.kit, network: namingNetwork())
    }
}

/// The engine's pure functions for hosts that have no engine session: the
/// demo services (WalletDemo) format amounts, parse and build `dash:` URIs,
/// draw QR codes, verify signatures and check mnemonics exactly as the live
/// app does, so demo mode accepts and refuses the same input.
public enum EngineFunctions {
    /// `URIHandling` that checks addresses against `network()` at each call.
    public static func uriHandler(network: @escaping @Sendable () -> DashNetwork) -> URIService {
        URIService(namingNetwork: { network().kit })
    }

    /// `AmountFormatting` that names units for `network()` at each call (`tDASH` off mainnet).
    public static func amountFormatter(network: @escaping @Sendable () -> DashNetwork) -> EngineAmountFormatter {
        EngineAmountFormatter(namingNetwork: { network().kit })
    }

    /// The engine's `verify_message`: returns normally for a valid signature,
    /// else throws its `message.*` code.
    public static func verifyMessage(
        address: String, message: String, signature: String, network: DashNetwork
    ) throws(ServiceError) {
        try verifyMessage(address: address, message: message, signature: signature, naming: network.kit)
    }

    static func verifyMessage(
        address: String, message: String, signature: String, naming network: DashKit.DashNetwork
    ) throws(ServiceError) {
        try serviceCall { () throws(DashKitError) in
            try CoreFunctions.verifyMessage(address: address, message: message, signature: signature, network: network)
        }
    }

    /// The engine's `check_mnemonic` (word list and BIP39 checksum).
    public static func checkMnemonic(_ phrase: any SecretBuffer) throws(ServiceError) -> MnemonicCheck {
        let secret = secretBytes(phrase)
        return MnemonicCheck(try serviceCall { () throws(DashKitError) in try CoreFunctions.checkMnemonic(secret) })
    }
}
