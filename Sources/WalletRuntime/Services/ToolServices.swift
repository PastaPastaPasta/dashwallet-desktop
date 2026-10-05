// Adapters for message signing, `dash:` URIs / QR and amount formatting.
// Verification, URIs, QR and units are the engine's pure functions
// (`CoreFunctions`); they use the open network, else the last open one,
// else the fallback the composition root chose.
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
        let network = context.namingNetwork
        try serviceCall { () throws(DashKitError) in
            try CoreFunctions.verifyMessage(address: address, message: message, signature: signature, network: network)
        }
    }
}

/// `URIHandling` over the engine's dw-uri functions.
public final class URIService: URIHandling {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func parsePaymentURI(_ text: String) throws(ServiceError) -> PaymentURI {
        let network = context.namingNetwork
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
        AddressClass(CoreFunctions.classifyAddress(text, network: context.namingNetwork))
    }

    public func qrMatrix(for text: String) throws(ServiceError) -> QRMatrix {
        let matrix = try serviceCall { () throws(DashKitError) in try CoreFunctions.qrMatrix(for: text) }
        return QRMatrix(size: matrix.size, modules: matrix.modules)
    }
}

/// `AmountFormatting` over the engine's dw-units functions (dash-qt
/// `BitcoinUnits`), so Swift and Rust format amounts identically.
public final class EngineAmountFormatter: AmountFormatting {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    /// dw-units rejects only out-of-range digit counts, which the style
    /// conversion clamps to 0...8. Should the engine still refuse, the amount
    /// is shown exactly, in duffs, rather than as an empty string.
    public func format(_ amount: Amount, unit: DisplayUnit, style: AmountStyle) -> String {
        let network = context.namingNetwork
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
        CoreFunctions.unitName(unit.kit, network: context.namingNetwork)
    }
}
