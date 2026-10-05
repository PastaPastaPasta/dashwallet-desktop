// M1 service contracts: receive addresses and payment requests (engine receive.rs).
import Foundation

public enum AddressChain: Sendable, Hashable {
    case receiving, change
}

public struct AddressInfo: Sendable, Hashable, Identifiable {
    public var id: String { address }
    public let address: String
    public let chain: AddressChain
    public let index: UInt32
    public let derivationPath: String
    public let used: Bool
    public let label: String?
    public let balance: Amount?
    public let txCount: UInt32

    public init(
        address: String, chain: AddressChain, index: UInt32, derivationPath: String, used: Bool, label: String?,
        balance: Amount?, txCount: UInt32
    ) {
        self.address = address
        self.chain = chain
        self.index = index
        self.derivationPath = derivationPath
        self.used = used
        self.label = label
        self.balance = balance
        self.txCount = txCount
    }
}

public struct AddressFilter: Sendable, Hashable {
    public var chain: AddressChain?
    public var used: Bool?

    public init(chain: AddressChain? = nil, used: Bool? = nil) {
        self.chain = chain
        self.used = used
    }
}

/// A stored payment request (QT-081/083, IOS-055).
public struct ReceiveRequest: Sendable, Hashable, Identifiable {
    public let id: UInt64
    public let createdAt: Date
    public let address: String
    public let amount: Amount?
    public let label: String?
    public let message: String?
    public let uri: String

    public init(id: UInt64, createdAt: Date, address: String, amount: Amount?, label: String?, message: String?, uri: String) {
        self.id = id
        self.createdAt = createdAt
        self.address = address
        self.amount = amount
        self.label = label
        self.message = message
        self.uri = uri
    }
}

public protocol ReceiveProviding: AnyObject, Sendable {
    /// First unused receiving address; re-query after history changes so the
    /// shown address rotates once it is paid (IOS-053).
    func currentAddress(wallet: WalletID) async throws(ServiceError) -> AddressInfo
    /// A fresh address (dash-qt "Request payment").
    func nextAddress(wallet: WalletID, label: String?) async throws(ServiceError) -> AddressInfo
    func addresses(wallet: WalletID, filter: AddressFilter) async throws(ServiceError) -> [AddressInfo]
    func createRequest(wallet: WalletID, amount: Amount?, label: String?, message: String?) async throws(ServiceError)
        -> ReceiveRequest
    func requests(wallet: WalletID) async throws(ServiceError) -> [ReceiveRequest]
    func deleteRequest(wallet: WalletID, id: UInt64) async throws(ServiceError)
}
