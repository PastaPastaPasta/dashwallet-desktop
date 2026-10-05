// Receive (QT-081…085, IOS-053…055).
import Foundation
import Observation
import WalletRuntime

@MainActor
@Observable
public final class ReceiveViewModel {
    /// The address shown: first unused receiving address (rotates once paid).
    public private(set) var address: AddressInfo?
    /// QR of `uri` when a request is shown, else of `dash:<address>`.
    public private(set) var qr: QRMatrix?
    public var requestAmountText = ""
    public var label = ""
    public var message = ""
    /// The URI of the request being shown (`createRequest`), or the plain
    /// address URI.
    public private(set) var uri: String?
    /// The request just created (dash-qt "Request payment" dialog, QT-082).
    public private(set) var shownRequest: ReceiveRequest?
    public private(set) var requests: [ReceiveRequest] = []
    public private(set) var errorMessage: String?
    public private(set) var amountError: String?

    private let walletState: any WalletStateProviding
    private let receive: any ReceiveProviding
    private let history: any HistoryProviding
    private let uriHandler: any URIHandling
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private var historyTask: Task<Void, Never>?

    public init(
        walletState: any WalletStateProviding, receive: any ReceiveProviding, history: any HistoryProviding,
        uri: any URIHandling, amounts: any AmountFormatting, settings: any SettingsProviding
    ) {
        self.walletState = walletState
        self.receive = receive
        self.history = history
        self.uriHandler = uri
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(env: AppEnvironment) {
        self.init(
            walletState: env.walletState, receive: env.receive, history: env.history, uri: env.uri,
            amounts: env.amounts, settings: env.settings)
    }

    /// Loads the current address and the stored requests.
    public func load() async {
        guard let wallet = walletState.selectedWalletID else {
            errorMessage = L10n.Common.noWallet
            return
        }
        do {
            let current = try await receive.currentAddress(wallet: wallet)
            requests = try await receive.requests(wallet: wallet).sorted { $0.createdAt > $1.createdAt }
            show(address: current)
            errorMessage = nil
        } catch {
            errorMessage = text(for: error)
        }
    }

    /// Re-queries the current address after history changes so a paid
    /// address rotates (IOS-054).
    public func start() {
        stop()
        guard let wallet = walletState.selectedWalletID else { return }
        let changes = history.changes(wallet: wallet)
        historyTask = Task { [weak self] in
            for await _ in changes {
                guard let self else { return }
                await self.refreshCurrentAddress()
            }
        }
    }

    public func stop() {
        historyTask?.cancel()
        historyTask = nil
    }

    /// A fresh address ("Receive another", IOS-054).
    public func newAddress() async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            let next = try await receive.nextAddress(wallet: wallet, label: label.isEmpty ? nil : label)
            show(address: next)
            errorMessage = nil
        } catch {
            errorMessage = text(for: error)
        }
    }

    /// dash-qt "Create new receiving address" with the form's amount, label
    /// and message (QT-081); shows the request and clears the form.
    public func createRequest() async {
        guard let wallet = walletState.selectedWalletID else { return }
        amountError = nil
        let amount: Amount?
        switch AmountInput.parse(requestAmountText, unit: settings.display.unit, formatter: amounts) {
        case .success(let value):
            // 0 or empty means "no amount" (dash-qt).
            amount = (value?.duffs ?? 0) > 0 ? value : nil
            if let value, value.duffs < 0 {
                amountError = L10n.Receive.invalidAmount
                return
            }
        case .failure:
            amountError = L10n.Receive.invalidAmount
            return
        }
        do {
            let request = try await receive.createRequest(
                wallet: wallet, amount: amount, label: label.isEmpty ? nil : label,
                message: message.isEmpty ? nil : message)
            requests.insert(request, at: 0)
            shownRequest = request
            uri = request.uri
            renderQR(request.uri)
            clearForm()
        } catch {
            errorMessage = text(for: error)
        }
    }

    /// Shows a stored request again (QT-083 "Show").
    public func show(_ request: ReceiveRequest) {
        shownRequest = request
        uri = request.uri
        renderQR(request.uri)
    }

    /// Closes the request view and returns to the plain address.
    public func dismissRequest() {
        shownRequest = nil
        if let address { show(address: address) }
    }

    public func deleteRequest(_ id: ReceiveRequest.ID) async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            try await receive.deleteRequest(wallet: wallet, id: id)
            requests.removeAll { $0.id == id }
            if shownRequest?.id == id { dismissRequest() }
        } catch {
            errorMessage = text(for: error)
        }
    }

    public func clearForm() {
        requestAmountText = ""
        label = ""
        message = ""
        amountError = nil
    }

    public func copyURI() -> String? { uri }

    public func copyAddress() -> String? { shownRequest?.address ?? address?.address }

    /// Requests-table cells with dash-qt's placeholders (QT-083).
    public func amountText(of request: ReceiveRequest) -> String {
        guard let amount = request.amount else { return L10n.Receive.noAmount }
        return amounts.format(amount, unit: settings.display.unit, style: .plain(plusSign: false, separators: .standard))
    }

    // MARK: Private

    private func refreshCurrentAddress() async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            let current = try await receive.currentAddress(wallet: wallet)
            if current.address != address?.address {
                address = current
                if shownRequest == nil { show(address: current) }
            }
        } catch {
            errorMessage = text(for: error)
        }
    }

    /// QR of `text`; on failure (URI over 255 characters, QT-084) the
    /// error is shown and `qr` is `nil`.
    private func renderQR(_ text: String) {
        do {
            qr = try uriHandler.qrMatrix(for: text)
            errorMessage = nil
        } catch {
            qr = nil
            errorMessage = self.text(for: error)
        }
    }

    private func show(address info: AddressInfo) {
        address = info
        guard shownRequest == nil else { return }
        do {
            let plain = try uriHandler.buildPaymentURI(address: info.address, amount: nil, label: nil, message: nil)
            uri = plain
            qr = try uriHandler.qrMatrix(for: plain)
        } catch {
            uri = nil
            qr = nil
            errorMessage = text(for: error)
        }
    }

    private func text(for error: ServiceError) -> String {
        switch error.code {
        case EngineCode.uriTooLongForQR: L10n.Receive.uriTooLong
        case EngineCode.receiveGapLimit: L10n.Receive.couldNotGenerate
        case .vaultLocked, EngineCode.walletVaultLocked: L10n.Receive.couldNotUnlock
        default: ErrorText.common(error.code)
        }
    }
}
