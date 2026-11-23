// M1 service contracts: wallets and balances (iOS `SwiftDashSDKWalletState`).
import Foundation

/// Core balance buckets (QT-034).
public struct WalletBalances: Sendable, Hashable {
    public let confirmed: Amount
    public let unconfirmed: Amount
    public let immature: Amount
    public let locked: Amount
    public let total: Amount
    /// Spendable balance of the CoinJoin accounts.
    public let coinjoin: Amount

    public init(
        confirmed: Amount, unconfirmed: Amount, immature: Amount, locked: Amount, total: Amount,
        coinjoin: Amount = .zero
    ) {
        self.confirmed = confirmed
        self.unconfirmed = unconfirmed
        self.immature = immature
        self.locked = locked
        self.total = total
        self.coinjoin = coinjoin
    }
}

/// One registered wallet (engine `WalletInfo`).
public struct WalletInfo: Sendable, Hashable, Identifiable {
    public let id: WalletID
    public let name: String
    public let watchOnly: Bool
    public let hasMnemonic: Bool
    public let hd: Bool
    public let birthHeight: UInt32?
    public let createdAt: Date?
    /// `nil` until the wallet's scan has passed its birth height: show
    /// "unknown", never zero.
    public let balances: WalletBalances?

    public init(
        id: WalletID, name: String, watchOnly: Bool, hasMnemonic: Bool, hd: Bool,
        birthHeight: UInt32?, createdAt: Date?, balances: WalletBalances?
    ) {
        self.id = id
        self.name = name
        self.watchOnly = watchOnly
        self.hasMnemonic = hasMnemonic
        self.hd = hd
        self.birthHeight = birthHeight
        self.createdAt = createdAt
        self.balances = balances
    }
}

/// Wallet list, selection and balances of the active network. Values are
/// `nil` until the first load; view models show "unknown", never zero.
@MainActor
public protocol WalletStateProviding: AnyObject {
    var wallets: [WalletInfo]? { get }
    var selectedWalletID: WalletID? { get }
    /// Balances of the selected wallet.
    var balances: WalletBalances? { get }
    /// Yields after any of the properties above changed (coalesced).
    func changes() -> AsyncStream<Void>
    func select(_ id: WalletID)
    func rename(_ id: WalletID, to name: String) async throws(ServiceError)
}
