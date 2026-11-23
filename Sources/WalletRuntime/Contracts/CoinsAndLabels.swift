// M1 service contracts: coin control and address book (engine coins.rs, labels.rs).
import Foundation

public struct Utxo: Sendable, Hashable, Identifiable {
    public var id: OutPoint { outpoint }
    public let outpoint: OutPoint
    public let address: String
    public let amount: Amount
    public let confirmations: UInt32
    public let date: Date?
    public let instantLocked: Bool
    public let chainLocked: Bool
    public let userLocked: Bool
    public let reserved: Bool
    public let label: String?
    public let isChange: Bool
    public let coinJoinDenominated: Bool
    public let coinJoinRounds: UInt32?
    public let spendable: Bool

    public init(
        outpoint: OutPoint, address: String, amount: Amount, confirmations: UInt32, date: Date?,
        instantLocked: Bool, chainLocked: Bool, userLocked: Bool, reserved: Bool, label: String?, isChange: Bool,
        coinJoinDenominated: Bool, coinJoinRounds: UInt32?, spendable: Bool
    ) {
        self.outpoint = outpoint
        self.address = address
        self.amount = amount
        self.confirmations = confirmations
        self.date = date
        self.instantLocked = instantLocked
        self.chainLocked = chainLocked
        self.userLocked = userLocked
        self.reserved = reserved
        self.label = label
        self.isChange = isChange
        self.coinJoinDenominated = coinJoinDenominated
        self.coinJoinRounds = coinJoinRounds
        self.spendable = spendable
    }
}

public struct UtxoFilter: Sendable, Hashable {
    public var includeLocked: Bool
    public var fullyMixedOnly: Bool
    public var minimumConfirmations: UInt32?

    public init(includeLocked: Bool = true, fullyMixedOnly: Bool = false, minimumConfirmations: UInt32? = nil) {
        self.includeLocked = includeLocked
        self.fullyMixedOnly = fullyMixedOnly
        self.minimumConfirmations = minimumConfirmations
    }
}

/// Coin control (QT-068…075).
public protocol CoinControlProviding: AnyObject, Sendable {
    func utxos(wallet: WalletID, filter: UtxoFilter) async throws(ServiceError) -> [Utxo]
    func lock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError)
    func unlock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError)
    func lockedOutpoints(wallet: WalletID) async throws(ServiceError) -> [OutPoint]
}

public enum AddressPurpose: Sendable, Hashable {
    case send, receive
}

public struct AddressBookEntry: Sendable, Hashable, Identifiable {
    public var id: String { address }
    public let address: String
    public let label: String
    public let purpose: AddressPurpose
    public let createdAt: Date?

    public init(address: String, label: String, purpose: AddressPurpose, createdAt: Date?) {
        self.address = address
        self.label = label
        self.purpose = purpose
        self.createdAt = createdAt
    }
}

/// Address book (QT-095…098).
public protocol AddressBookProviding: AnyObject, Sendable {
    func entries(wallet: WalletID, purpose: AddressPurpose?, search: String?) async throws(ServiceError)
        -> [AddressBookEntry]
    /// Adds an entry; `replace` relabels an existing one instead of failing
    /// with `labels.duplicate_address`.
    func save(wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool)
        async throws(ServiceError) -> AddressBookEntry
    func delete(wallet: WalletID, address: String) async throws(ServiceError)
}
