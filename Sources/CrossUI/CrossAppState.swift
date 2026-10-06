// The state one window of the SwiftCrossUI app holds: the composition's
// environment, the main view model and the view models of the tool pages
// (address book, sign/verify), which MainViewModel does not own.
import Foundation
import Observation
import WalletFeatures
import WalletRuntime

/// Tool pages reached from the sidebar's Tools section and the menus.
public enum ToolPage: Sendable, Hashable {
    case addressBook(AddressPurpose)
    case signVerify
    case settings

    init(_ sheet: SheetRoute) {
        switch sheet {
        case .signMessage, .verifyMessage: self = .signVerify
        case .sendingAddresses: self = .addressBook(.send)
        case .receivingAddresses: self = .addressBook(.receive)
        case .encryptWallet, .changePassphrase, .showRecoveryPhrase, .settings: self = .settings
        }
    }

    var sheet: SheetRoute {
        switch self {
        case .addressBook(.send): .sendingAddresses
        case .addressBook(.receive): .receivingAddresses
        case .signVerify: .signMessage
        case .settings: .settings
        }
    }
}

@MainActor
@Observable
public final class CrossAppState {
    public let env: AppEnvironment
    public let main: MainViewModel
    /// Shown in the status row (for example the demo-mode notice).
    public let notice: String?
    /// The peers page is open (status row, QT-147).
    var showsPeers = false

    @ObservationIgnored private var addressBookModel: (network: DashNetwork, model: AddressBookViewModel)?
    @ObservationIgnored private var signVerifyModel: (network: DashNetwork, model: SignVerifyViewModel)?

    public init(env: AppEnvironment, main: MainViewModel, notice: String? = nil) {
        self.env = env
        self.main = main
        self.notice = notice
    }

    /// The address book of the active network, created on first use with
    /// `purpose`; afterwards the page's own picker sets the purpose.
    func addressBook(purpose: AddressPurpose) -> AddressBookViewModel? {
        guard let network = main.network else { return nil }
        if let cached = addressBookModel, cached.network == network { return cached.model }
        let model = AddressBookViewModel(env: env, network: network, purpose: purpose)
        addressBookModel = (network, model)
        return model
    }

    /// Sign/verify of the active network, created on first use.
    func signVerify() -> SignVerifyViewModel? {
        guard let network = main.network else { return nil }
        if let cached = signVerifyModel, cached.network == network { return cached.model }
        let model = SignVerifyViewModel(env: env, network: network)
        signVerifyModel = (network, model)
        return model
    }

    /// An amount in the display unit, with the unit name (detail panes).
    func format(_ amount: Amount) -> String {
        env.amounts.format(amount, unit: env.settings.display.unit, style: .withUnit(plusSign: false, separators: .always))
    }

    /// Performs a route a page raised and clears it.
    func follow(_ route: AppRoute?) async {
        guard let route else { return }
        main.sheet = nil
        await main.navigate(route)
    }
}
