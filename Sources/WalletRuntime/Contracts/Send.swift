// M1 service contracts: sending (iOS `SwiftDashSDKTransactionSender`).
import Foundation

public struct PaymentRecipient: Sendable, Hashable {
    public var address: String
    public var amount: Amount
    public var subtractFeeFromAmount: Bool
    public var label: String?
    public var message: String?

    public init(address: String, amount: Amount, subtractFeeFromAmount: Bool = false, label: String? = nil, message: String? = nil) {
        self.address = address
        self.amount = amount
        self.subtractFeeFromAmount = subtractFeeFromAmount
        self.label = label
        self.message = message
    }
}

public enum CoinSourceChoice: Sendable, Hashable {
    case any
    /// CoinJoin send page (QT-051).
    case fullyMixed
    case outpoints([OutPoint])
}

public enum FeeChoice: Sendable, Hashable {
    case recommended(targetBlocks: UInt32)
    case perKilobyte(Amount)
}

public enum ChangeChoice: Sendable, Hashable {
    case automatic
    case address(String)
}

public struct TxEstimate: Sendable, Hashable {
    public let fee: Amount
    public let sizeBytes: UInt32
    public let inputCount: UInt32
    public let change: Amount?
    public let totalSent: Amount

    public init(fee: Amount, sizeBytes: UInt32, inputCount: UInt32, change: Amount?, totalSent: Amount) {
        self.fee = fee
        self.sizeBytes = sizeBytes
        self.inputCount = inputCount
        self.change = change
        self.totalSent = totalSent
    }
}

public struct PreparedOutput: Sendable, Hashable {
    public let address: String?
    public let amount: Amount
    public let isChange: Bool
    public let label: String?
    /// Pays one of the wallet's addresses. A change output with `false` is a
    /// foreign custom change address (QT-073 warning). `nil` when not reported.
    public let isMine: Bool?

    public init(address: String?, amount: Amount, isChange: Bool, label: String?, isMine: Bool? = nil) {
        self.address = address
        self.amount = amount
        self.isChange = isChange
        self.label = label
        self.isMine = isMine
    }
}

/// What the confirm dialog shows (QT-059, IOS-046).
public struct PreparedTxSummary: Sendable, Hashable {
    public let txid: String
    public let fee: Amount
    public let feeRatePerKilobyte: Amount
    public let sizeBytes: UInt32
    public let inputCount: Int
    public let outputs: [PreparedOutput]
    public let totalSent: Amount
    /// Inputs minus outputs back to the wallet (fee included).
    public let totalDebit: Amount
    /// Paid to scripts the wallet does not own, fee excluded: what a Spend
    /// grant caps (m1-engine.md §2.7.1). `nil` when not reported.
    public let externalSent: Amount?

    public init(
        txid: String, fee: Amount, feeRatePerKilobyte: Amount, sizeBytes: UInt32, inputCount: Int,
        outputs: [PreparedOutput], totalSent: Amount, totalDebit: Amount, externalSent: Amount? = nil
    ) {
        self.txid = txid
        self.fee = fee
        self.feeRatePerKilobyte = feeRatePerKilobyte
        self.sizeBytes = sizeBytes
        self.inputCount = inputCount
        self.outputs = outputs
        self.totalSent = totalSent
        self.totalDebit = totalDebit
        self.externalSent = externalSent
    }
}

/// A signed, not yet broadcast transaction. `id` names the engine handle
/// the draft keeps; only `TransactionDrafting.broadcast` sends it.
public struct PreparedTransaction: Sendable, Hashable {
    public let id: UUID
    public let summary: PreparedTxSummary

    public init(id: UUID, summary: PreparedTxSummary) {
        self.id = id
        self.summary = summary
    }
}

public struct BroadcastResult: Sendable, Hashable {
    public let txid: String
    /// `nil`: the engine (dash-spv) does not report it.
    public let peersAnnounced: UInt32?

    public init(txid: String, peersAnnounced: UInt32?) {
        self.txid = txid
        self.peersAnnounced = peersAnnounced
    }
}

/// One editable payment (engine `TxDraft`). Setters validate offline;
/// `prepare` signs and reserves inputs but never broadcasts (iOS rule 4).
public protocol TransactionDrafting: AnyObject, Sendable {
    func setRecipients(_ recipients: [PaymentRecipient]) async throws(ServiceError)
    func setSource(_ source: CoinSourceChoice) async throws(ServiceError)
    func setFee(_ fee: FeeChoice) async throws(ServiceError)
    func setChange(_ change: ChangeChoice) async throws(ServiceError)
    func estimate() async throws(ServiceError) -> TxEstimate
    func prepare(grant: AuthGrant) async throws(ServiceError) -> PreparedTransaction
    func broadcast(_ prepared: PreparedTransaction) async throws(ServiceError) -> BroadcastResult
    /// Releases the reserved inputs. Idempotent.
    func abandon(_ prepared: PreparedTransaction) async throws(ServiceError)
}

public protocol TransactionSending: AnyObject, Sendable {
    func makeDraft(wallet: WalletID) async throws(ServiceError) -> any TransactionDrafting
    /// Largest single-recipient amount with the fee subtracted ("Max").
    func maxSpendable(wallet: WalletID, source: CoinSourceChoice, fee: FeeChoice) async throws(ServiceError) -> Amount
}
