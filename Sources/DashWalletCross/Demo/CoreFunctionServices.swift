// URI/QR, amount formatting and message verification over the engine's pure
// functions (dw-uri, dw-units, message.rs; m1-engine.md §2.10–2.12, all
// "works"). The --demo environment uses these real implementations, so
// amounts, `dash:` URIs, address checks and QR codes in the demo are the
// engine's.
//
// TODO(C): WalletRuntime's adapters provide URIHandling/AmountFormatting for
// the live app; switch the demo to them once they are on main.
import DashWalletCore
import Foundation
import WalletRuntime

/// `URIHandling` for the network in `box`.
final class CoreURIHandling: URIHandling {
    let box: NetworkBox

    init(box: NetworkBox) {
        self.box = box
    }

    var network: WalletRuntime.DashNetwork { box.network }

    func parsePaymentURI(_ text: String) throws(ServiceError) -> PaymentURI {
        do {
            let parsed = try DashWalletCore.parsePaymentUri(network: network.core, text: text)
            return PaymentURI(
                address: parsed.address, amount: parsed.amount.map { Amount(duffs: Int64(clamping: $0)) },
                label: parsed.label, message: parsed.message)
        } catch {
            throw CoreErrors.map(error)
        }
    }

    func buildPaymentURI(address: String, amount: Amount?, label: String?, message: String?) throws(ServiceError)
        -> String
    {
        do {
            let duffs = try amount.map { amount throws(ServiceError) -> UInt64 in
                guard amount.duffs >= 0 else {
                    throw ServiceError(code: ServiceErrorCode(rawValue: "uri.invalid_amount"), detail: "negative amount")
                }
                return UInt64(amount.duffs)
            }
            return try DashWalletCore.buildPaymentUri(address: address, amount: duffs, label: label, message: message)
        } catch let error as ServiceError {
            throw error
        } catch {
            throw CoreErrors.map(error)
        }
    }

    func classifyAddress(_ text: String) -> WalletRuntime.AddressClass {
        switch DashWalletCore.classifyAddress(network: network.core, text: text) {
        case .core(let scriptHash): .core(scriptHash: scriptHash)
        case .platform: .platform
        case .shielded: .shielded
        case .invalid(let problem): .invalid(problem.runtime)
        }
    }

    func qrMatrix(for text: String) throws(ServiceError) -> QRMatrix {
        do {
            let matrix = try DashWalletCore.qrMatrix(text: text)
            return QRMatrix(size: Int(matrix.size), modules: matrix.modules)
        } catch {
            throw CoreErrors.map(error)
        }
    }
}

/// `AmountFormatting` for the network in `box` (`tDASH` names off mainnet).
final class CoreAmountFormatting: AmountFormatting {
    let box: NetworkBox

    init(box: NetworkBox) {
        self.box = box
    }

    var network: WalletRuntime.DashNetwork { box.network }

    func format(_ amount: Amount, unit: WalletRuntime.DisplayUnit, style: WalletRuntime.AmountStyle) -> String {
        do {
            return try DashWalletCore.formatAmount(
                amount: amount.duffs, unit: unit.core, network: network.core, style: style.core)
        } catch {
            // Only `floored` with more than 8 digits fails; show the plain number.
            return (try? DashWalletCore.formatAmount(
                amount: amount.duffs, unit: unit.core, network: network.core,
                style: .plain(plusSign: false, separators: .standard))) ?? String(amount.duffs)
        }
    }

    func parse(_ text: String, unit: WalletRuntime.DisplayUnit) throws(ServiceError) -> Amount {
        do {
            return Amount(duffs: try DashWalletCore.parseAmount(text: text, unit: unit.core))
        } catch {
            throw CoreErrors.map(error)
        }
    }

    func unitName(_ unit: WalletRuntime.DisplayUnit) -> String {
        DashWalletCore.unitName(unit: unit.core, network: network.core)
    }
}

/// Message verification is the engine's. Signing needs the vault-backed
/// `sign_message` and the wallet's keys; the demo's sample wallets have no
/// keys in any vault, so the demo reports signing as not implemented instead
/// of inventing a signature. The live app signs through WalletRuntime.
final class CoreMessageSigning: MessageSigning {
    let box: NetworkBox

    init(box: NetworkBox) {
        self.box = box
    }

    var network: WalletRuntime.DashNetwork { box.network }

    func sign(wallet: WalletID, address: String, message: String, grant: WalletRuntime.AuthGrant) async throws(ServiceError)
        -> String
    {
        throw ServiceError(code: .notImplemented, detail: "the demo wallets have no keys to sign with")
    }

    func verify(address: String, message: String, signature: String) throws(ServiceError) {
        do {
            try DashWalletCore.verifyMessage(
                network: network.core, address: address, message: message, signature: signature)
        } catch {
            throw CoreErrors.map(error)
        }
    }
}

/// Engine errors → `ServiceError` codes (m1-engine.md §4).
enum CoreErrors {
    static func map(_ error: any Error) -> ServiceError {
        let detail = String(describing: error)
        switch error {
        case let error as UriError:
            let code: String
            switch error {
            case .DoubleSlash: code = "uri.double_slash"
            case .NotDashUri: code = "uri.not_dash_uri"
            case .Unparsable: code = "uri.unparsable"
            case .Bip70Unsupported: code = "uri.bip70_unsupported"
            case .InvalidAddress: code = "uri.invalid_address"
            case .InvalidAmount: code = "uri.invalid_amount"
            case .TooLongForQr: code = "uri.too_long_for_qr"
            case .InvalidArgument: code = "invalid_argument"
            case .NotImplemented: code = "not_implemented"
            }
            return ServiceError(code: ServiceErrorCode(rawValue: code), detail: detail)
        case let error as UnitsError:
            switch error {
            case .Unparsable: return ServiceError(code: .unitsUnparsable, detail: detail)
            case .InvalidArgument: return ServiceError(code: .invalidArgument, detail: detail)
            case .NotImplemented: return ServiceError(code: .notImplemented, detail: detail)
            }
        case let error as MessageError:
            let code: String
            switch error {
            case .InvalidAddress: code = "message.invalid_address"
            case .AddressNoKey: code = "message.address_no_key"
            case .MalformedSignature: code = "message.malformed_signature"
            case .PubkeyNotRecovered: code = "message.pubkey_not_recovered"
            case .NotSigned: code = "message.not_signed"
            case .AddressNotMine: code = "message.address_not_mine"
            case .WatchOnly: code = "message.watch_only"
            case .VaultLocked: code = "message.vault_locked"
            case .GrantInvalid: code = "message.grant_invalid"
            case .InvalidArgument: code = "invalid_argument"
            case .NetworkNotOpen: code = "network_not_open"
            default: code = "internal"
            }
            return ServiceError(code: ServiceErrorCode(rawValue: code), detail: detail)
        default:
            return ServiceError(code: .internal, detail: detail)
        }
    }
}

extension WalletRuntime.DashNetwork {
    var core: DashWalletCore.DashNetwork {
        switch self {
        case .mainnet: .mainnet
        case .testnet: .testnet
        case .devnet(let name): .devnet(name: name)
        case .regtest: .regtest
        }
    }
}

extension WalletRuntime.DisplayUnit {
    var core: DashWalletCore.DisplayUnit {
        switch self {
        case .dash: .dash
        case .milliDash: .milliDash
        case .microDash: .microDash
        case .duffs: .duffs
        }
    }
}

extension WalletRuntime.AmountSeparators {
    var core: Separators {
        switch self {
        case .never: .never
        case .standard: .standard
        case .always: .always
        }
    }
}

extension WalletRuntime.AmountStyle {
    var core: DashWalletCore.AmountStyle {
        switch self {
        case .plain(let plusSign, let separators): .plain(plusSign: plusSign, separators: separators.core)
        case .withUnit(let plusSign, let separators): .withUnit(plusSign: plusSign, separators: separators.core)
        case .floored(let plusSign, let separators, let digits):
            .floored(plusSign: plusSign, separators: separators.core, digits: UInt8(clamping: digits))
        case .privacy(let separators, let hidden): .privacy(separators: separators.core, hidden: hidden)
        case .gui(let signed, let truncate): .gui(signed: signed, truncate: truncate.map { UInt8(clamping: $0) })
        }
    }
}

extension DashWalletCore.AddressProblem {
    var runtime: WalletRuntime.AddressProblem {
        switch self {
        case .invalidBase58Length: .invalidBase58Length
        case .invalidBase58Prefix: .invalidBase58Prefix
        case .notBech32mOrBase58: .notBech32mOrBase58
        case .invalidBase58ChecksumOrLength: .invalidBase58ChecksumOrLength
        case .platformAddress: .platformAddress
        case .bech32: .bech32
        }
    }
}
