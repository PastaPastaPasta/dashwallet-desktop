import DashWalletCore
import Foundation

/// The engine's pure functions (units, URIs, QR, message verification,
/// mnemonics). They need no session and run on the caller's thread.
public enum CoreFunctions {
    // MARK: Units (QT-020, QT-036, QT-039, QT-152)

    public static func formatAmount(
        _ amount: Amount, unit: DisplayUnit, network: DashNetwork, style: AmountStyle
    ) throws(DashKitError) -> String {
        try mapped { try DashWalletCore.formatAmount(amount: amount.duffs, unit: unit.ffi, network: network.ffi, style: style.ffi) }
    }

    public static func parseAmount(_ text: String, unit: DisplayUnit) throws(DashKitError) -> Amount {
        Amount(duffs: try mapped { try DashWalletCore.parseAmount(text: text, unit: unit.ffi) })
    }

    public static func unitName(_ unit: DisplayUnit, network: DashNetwork) -> String {
        DashWalletCore.unitName(unit: unit.ffi, network: network.ffi)
    }

    // MARK: URIs and QR (QT-054, QT-084/085, QT-149)

    public static func parsePaymentURI(_ text: String, network: DashNetwork) throws(DashKitError) -> PaymentURI {
        let uri = try mapped { try DashWalletCore.parsePaymentUri(network: network.ffi, text: text) }
        return PaymentURI(
            address: uri.address, amount: try uri.amount.engineAmount(), label: uri.label, message: uri.message)
    }

    public static func buildPaymentURI(
        address: String, amount: Amount?, label: String?, message: String?
    ) throws(DashKitError) -> String {
        let duffs = try amount.map { (a) throws(DashKitError) in try a.engineDuffs() }
        return try mapped { try DashWalletCore.buildPaymentUri(address: address, amount: duffs, label: label, message: message) }
    }

    public static func classifyAddress(_ text: String, network: DashNetwork) -> AddressClass {
        AddressClass(DashWalletCore.classifyAddress(network: network.ffi, text: text))
    }

    public static func qrMatrix(for text: String) throws(DashKitError) -> QRMatrix {
        let matrix = try mapped { try DashWalletCore.qrMatrix(text: text) }
        return QRMatrix(size: Int(matrix.size), modules: matrix.modules)
    }

    // MARK: Messages (QT-100)

    /// Returns normally when `signature` is a valid signature of `message`
    /// by `address`; otherwise throws a `message.*` code.
    public static func verifyMessage(
        address: String, message: String, signature: String, network: DashNetwork
    ) throws(DashKitError) {
        try mapped {
            try DashWalletCore.verifyMessage(network: network.ffi, address: address, message: message, signature: signature)
        }
    }

    // MARK: Mnemonics (QT-102…104, IOS-002…007)

    /// A fresh phrase as zeroing bytes. Stores nothing.
    public static func generateMnemonic(wordCount: Int, language: MnemonicLanguage) throws(DashKitError) -> SecretBytes {
        guard let count = UInt8(exactly: wordCount) else {
            throw .domain(code: "wallet.unsupported_word_count", detail: "\(wordCount)")
        }
        var data = try mapped { try DashWalletCore.generateMnemonic(wordCount: count, language: language.ffi) }
        return SecretBytes(consuming: &data)
    }

    public static func checkMnemonic(_ phrase: SecretBytes) throws(DashKitError) -> MnemonicCheck {
        var data = phrase.withUnsafeBytes { Data($0) }
        defer { data.withUnsafeMutableBytes { secureZero($0) } }
        let check = try mapped { try DashWalletCore.checkMnemonic(phrase: data) }
        return MnemonicCheck(check)
    }
}
