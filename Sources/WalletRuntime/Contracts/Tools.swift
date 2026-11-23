// M1 service contracts: message signing, URIs/QR, amount formatting and
// display settings (engine message.rs, uri.rs, units.rs; Swift settings).
import Foundation

/// Sign / verify message (QT-099/100).
public protocol MessageSigning: AnyObject, Sendable {
    /// Base64 compact signature. Needs a `.signMessage` grant.
    func sign(wallet: WalletID, address: String, message: String, grant: AuthGrant) async throws(ServiceError)
        -> String
    /// Returns normally when the signature is valid; otherwise throws a
    /// `message.*` code that selects dash-qt's result text.
    func verify(address: String, message: String, signature: String) throws(ServiceError)
}

public enum AddressProblem: Sendable, Hashable {
    case invalidBase58Length, invalidBase58Prefix, notBech32mOrBase58, invalidBase58ChecksumOrLength
    case platformAddress
    case bech32
}

public enum AddressClass: Sendable, Hashable {
    case core(scriptHash: Bool)
    case platform
    case shielded
    case invalid(AddressProblem)
}

public struct PaymentURI: Sendable, Hashable {
    public let address: String
    public let amount: Amount?
    public let label: String?
    public let message: String?

    public init(address: String, amount: Amount?, label: String?, message: String?) {
        self.address = address
        self.amount = amount
        self.label = label
        self.message = message
    }
}

/// QR modules, row-major (`modules[y * size + x]`, `true` = dark).
public struct QRMatrix: Sendable, Hashable {
    public let size: Int
    public let modules: [Bool]

    public init(size: Int, modules: [Bool]) {
        self.size = size
        self.modules = modules
    }
}

/// `dash:` URIs, address classification and QR matrices for the active
/// network (QT-054, QT-084/085, QT-149, IOS-042).
public protocol URIHandling: AnyObject, Sendable {
    func parsePaymentURI(_ text: String) throws(ServiceError) -> PaymentURI
    func buildPaymentURI(address: String, amount: Amount?, label: String?, message: String?) throws(ServiceError)
        -> String
    func classifyAddress(_ text: String) -> AddressClass
    func qrMatrix(for text: String) throws(ServiceError) -> QRMatrix
}

public enum DisplayUnit: Sendable, Hashable, CaseIterable, Codable {
    case dash, milliDash, microDash, duffs
}

public enum AmountSeparators: Sendable, Hashable {
    case never, standard, always
}

/// dash-qt formatter to apply (engine `AmountStyle`).
public enum AmountStyle: Sendable, Hashable {
    case plain(plusSign: Bool, separators: AmountSeparators)
    case withUnit(plusSign: Bool, separators: AmountSeparators)
    case floored(plusSign: Bool, separators: AmountSeparators, digits: Int)
    case privacy(separators: AmountSeparators, hidden: Bool)
    case gui(signed: Bool, truncate: Int?)
}

/// dash-qt amount text for the active network (`tDASH` off mainnet).
public protocol AmountFormatting: AnyObject, Sendable {
    func format(_ amount: Amount, unit: DisplayUnit, style: AmountStyle) -> String
    func parse(_ text: String, unit: DisplayUnit) throws(ServiceError) -> Amount
    func unitName(_ unit: DisplayUnit) -> String
}

/// Display preferences from the Swift settings store (`settings.json`).
public struct DisplaySettings: Sendable, Hashable, Codable {
    public var unit: DisplayUnit
    /// dash-qt "decimal digits" (2...8).
    public var decimalDigits: Int
    /// Discreet mode (QT-039, IOS-020).
    public var hideBalances: Bool

    public init(unit: DisplayUnit = .dash, decimalDigits: Int = 8, hideBalances: Bool = false) {
        self.unit = unit
        self.decimalDigits = decimalDigits
        self.hideBalances = hideBalances
    }
}

@MainActor
public protocol SettingsProviding: AnyObject {
    var display: DisplaySettings { get }
    var lastNetwork: DashNetwork? { get }
    func update(_ display: DisplaySettings) throws(ServiceError)
    func changes() -> AsyncStream<DisplaySettings>
}
