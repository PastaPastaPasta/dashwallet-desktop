// Menu bar / tray companion additions (IOS-117): a receive request with an
// amount (address, `dash:` URI and QR) and "pay from clipboard". The balance
// and last transaction stay on `HomeViewModel` (M1).
import Foundation
import Observation
import PlatformServices
import WalletRuntime

@MainActor
@Observable
public final class MenuBarCompanionViewModel {
    public private(set) var address: String?
    public private(set) var requestAmountText = ""
    public private(set) var requestAmountError: String?
    /// `dash:` URI of the address with the requested amount.
    public private(set) var requestURI: String?
    public private(set) var qr: QRMatrix?
    /// Set by `payFromClipboard`; the UI opens Send with it.
    public private(set) var route: AppRoute?
    public private(set) var errorMessage: String?

    private var requestAmount: Amount?
    private let walletState: any WalletStateProviding
    private let receive: any ReceiveProviding
    private let uri: any URIHandling
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let clipboard: any ClipboardProviding

    public init(
        walletState: any WalletStateProviding, receive: any ReceiveProviding, uri: any URIHandling,
        amounts: any AmountFormatting, settings: any SettingsProviding, clipboard: any ClipboardProviding
    ) {
        self.walletState = walletState
        self.receive = receive
        self.uri = uri
        self.amounts = amounts
        self.settings = settings
        self.clipboard = clipboard
    }

    public convenience init(env: AppEnvironment, m2: M2Services) {
        self.init(
            walletState: env.walletState, receive: env.receive, uri: env.uri, amounts: env.amounts,
            settings: env.settings, clipboard: m2.clipboard)
    }

    /// The current receiving address and its request.
    public func load() async {
        guard let wallet = walletState.selectedWalletID else {
            address = nil
            requestURI = nil
            qr = nil
            return
        }
        do {
            address = try await receive.currentAddress(wallet: wallet).address
            errorMessage = nil
        } catch {
            address = nil
            errorMessage = ErrorText.m2(error.code)
        }
        rebuild()
    }

    /// Amount in the display unit; empty = no amount.
    public func setRequestAmount(_ text: String) {
        requestAmountText = text
        switch AmountInput.parse(text, unit: settings.display.unit, formatter: amounts) {
        case .success(let amount) where (amount?.duffs ?? 1) > 0:
            requestAmount = amount
            requestAmountError = nil
        case .success, .failure:
            requestAmount = nil
            requestAmountError = L10n.Receive.invalidAmount
        }
        rebuild()
    }

    /// A `dash:` URI or a plain address from the clipboard opens Send.
    public func payFromClipboard() {
        guard let text = clipboard.string()?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
            errorMessage = L10n.Send.invalidAddress
            return
        }
        if let payment = try? uri.parsePaymentURI(text) {
            route = .send(payment)
        } else if case .core = uri.classifyAddress(text) {
            route = .send(PaymentURI(address: text, amount: nil, label: nil, message: nil))
        } else {
            errorMessage = L10n.Send.invalidAddress
            return
        }
        errorMessage = nil
    }

    public func routeHandled() {
        route = nil
    }

    private func rebuild() {
        guard let address, requestAmountError == nil else {
            requestURI = nil
            qr = nil
            return
        }
        do {
            let text = try uri.buildPaymentURI(address: address, amount: requestAmount, label: nil, message: nil)
            requestURI = text
            qr = try uri.qrMatrix(for: text)
        } catch {
            requestURI = nil
            qr = nil
            errorMessage = ErrorText.m2(error.code)
        }
    }
}
