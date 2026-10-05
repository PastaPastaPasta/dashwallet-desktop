// Sending (iOS `SwiftDashSDKTransactionSender`, DESIGN-opus §1.12).
//
// The flow the send view model drives is auth → prepare → confirm →
// broadcast: `AuthenticationGating.authorize(.spend)` issues the grant,
// `TransactionDraft.prepare(grant:)` signs and reserves inputs without
// broadcasting (iOS rule 4), and only `broadcast` sends.
import DashKit
import Foundation

/// `TransactionSending` over engine `new_tx_draft` / `max_spendable`.
public final class TransactionSender: TransactionSending {
    private let context: EngineContext

    init(context: EngineContext) {
        self.context = context
    }

    public func makeDraft(wallet: WalletID) async throws(ServiceError) -> any TransactionDrafting {
        let id = try wallet.kit
        let handle = try await context.call { engine, network throws(DashKitError) in
            try await engine.newTxDraft(on: network, wallet: id)
        }
        return TransactionDraft(handle: handle)
    }

    public func maxSpendable(wallet: WalletID, source: CoinSourceChoice, fee: FeeChoice) async throws(ServiceError)
        -> Amount
    {
        let id = try wallet.kit
        let kitSource = source.kit
        let kitFee = fee.kit
        return Amount(try await context.call { engine, network throws(DashKitError) in
            try await engine.maxSpendable(on: network, wallet: id, source: kitSource, fee: kitFee)
        })
    }
}

/// One payment being edited (engine `TxDraft`). It keeps the engine's
/// prepared transactions under the `PreparedTransaction.id` it hands out,
/// so a view model only ever holds value types.
///
/// Rules on top of the engine (review M-7):
/// - Any setter first abandons the prepared transactions that were never
///   broadcast, so an edit after "Review" never leaves inputs reserved for a
///   transaction the user will not confirm.
/// - A broadcast that fails after it may have reached a peer (any failure
///   except `send.no_peers`, `send.prepared_tx_spent` and argument/session
///   errors) marks the transaction "outcome unknown". `abandon` then refuses
///   with `send.broadcast_outcome_unknown` and keeps the inputs reserved;
///   `broadcast` may be retried (same signed transaction, same txid).
/// - After a successful broadcast the transaction is forgotten: `abandon` is
///   a no-op and `broadcast` fails with `send.prepared_tx_unknown`.
public actor TransactionDraft: TransactionDrafting {
    private enum State {
        case ready
        case outcomeUnknown
    }

    private struct Entry {
        let handle: DashKit.PreparedTxHandle
        var state: State
    }

    private let handle: any DashKit.TxDraftHandle
    private var prepared: [UUID: Entry] = [:]

    init(handle: any DashKit.TxDraftHandle) {
        self.handle = handle
    }

    public func setRecipients(_ recipients: [PaymentRecipient]) async throws(ServiceError) {
        await abandonUnsent()
        let rows = recipients.map(\.kit)
        try await serviceCall { () async throws(DashKitError) in try await handle.setRecipients(rows) }
    }

    public func setSource(_ source: CoinSourceChoice) async throws(ServiceError) {
        await abandonUnsent()
        let kit = source.kit
        try await serviceCall { () async throws(DashKitError) in try await handle.setSource(kit) }
    }

    public func setFee(_ fee: FeeChoice) async throws(ServiceError) {
        await abandonUnsent()
        let kit = fee.kit
        try await serviceCall { () async throws(DashKitError) in try await handle.setFee(kit) }
    }

    public func setChange(_ change: ChangeChoice) async throws(ServiceError) {
        await abandonUnsent()
        let kit = change.kit
        try await serviceCall { () async throws(DashKitError) in try await handle.setChange(kit) }
    }

    public func estimate() async throws(ServiceError) -> TxEstimate {
        TxEstimate(try await serviceCall { () async throws(DashKitError) in try await handle.estimate() })
    }

    /// Signs and reserves inputs; never broadcasts. Needs a `.spend` grant.
    public func prepare(grant: AuthGrant) async throws(ServiceError) -> PreparedTransaction {
        guard case .spend = grant.purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "prepare needs a spend grant")
        }
        let grantID = grant.id
        let tx = try await serviceCall { () async throws(DashKitError) in try await handle.prepare(grantID: grantID) }
        let id = UUID()
        prepared[id] = Entry(handle: tx, state: .ready)
        return PreparedTransaction(id: id, summary: PreparedTxSummary(tx.summary))
    }

    public func broadcast(_ transaction: PreparedTransaction) async throws(ServiceError) -> BroadcastResult {
        guard let entry = prepared[transaction.id] else {
            throw ServiceError(code: .sendPreparedTxUnknown, detail: "not held by this draft")
        }
        let tx = entry.handle
        do {
            let outcome = try await serviceCall { () async throws(DashKitError) in try await handle.broadcast(tx) }
            prepared[transaction.id] = nil
            return BroadcastResult(txid: outcome.txid, peersAnnounced: outcome.peersAnnounced)
        } catch {
            switch error.code {
            case .sendNoPeers, .invalidArgument, .networkNotOpen, .notImplemented, .walletNotFound:
                // Nothing left the engine; the transaction stays ready.
                break
            case .sendPreparedTxSpent:
                prepared[transaction.id] = nil
            default:
                prepared[transaction.id]?.state = .outcomeUnknown
            }
            throw error
        }
    }

    public func abandon(_ transaction: PreparedTransaction) async throws(ServiceError) {
        guard let entry = prepared[transaction.id] else { return }
        if case .outcomeUnknown = entry.state {
            throw ServiceError(
                code: .sendBroadcastOutcomeUnknown, detail: "the broadcast may have reached a peer; inputs stay reserved")
        }
        let tx = entry.handle
        try await serviceCall { () async throws(DashKitError) in try await handle.abandon(tx) }
        prepared[transaction.id] = nil
    }

    /// Abandons every prepared transaction that was never broadcast. A
    /// failure leaves it held, so a later `abandon` can retry it.
    private func abandonUnsent() async {
        for (id, entry) in prepared {
            guard case .ready = entry.state else { continue }
            let tx = entry.handle
            do {
                try await serviceCall { () async throws(DashKitError) in try await handle.abandon(tx) }
                prepared[id] = nil
            } catch {
                continue
            }
        }
    }
}
