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
/// - `send.no_peers`, `send.broadcast_rejected` and `send.prepared_tx_spent`
///   mean the engine released the inputs and spent its `PreparedTx`
///   (m1-engine.md §2.7): the transaction is forgotten, `abandon` is a no-op
///   and `broadcast` fails with `send.prepared_tx_unknown`. Sending again
///   needs a new `prepare` and a new grant.
/// - Argument and session errors (`invalid_argument`, `network_not_open`,
///   `wallet_not_found`, `not_implemented`) mean nothing was dispatched: the
///   transaction keeps the state it had before the call.
/// - Any other failure may have reached a peer and marks the transaction
///   "outcome unknown". `abandon` then refuses with
///   `send.broadcast_outcome_unknown` and keeps the inputs reserved;
///   `broadcast` may be called again (same signed transaction, same txid).
/// - While a broadcast is in flight the transaction is neither abandoned (by
///   `abandon` or an edit) nor broadcast again: the actor is reentrant across
///   the engine call, and releasing inputs of a transaction that may be in
///   the mempool would allow a double spend. Both fail with
///   `send.broadcast_outcome_unknown` (edits simply leave it reserved).
/// - After a successful broadcast the transaction is forgotten: `abandon` is
///   a no-op and `broadcast` fails with `send.prepared_tx_unknown`.
public actor TransactionDraft: TransactionDrafting {
    private enum State {
        case ready
        case broadcasting
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
        if case .broadcasting = entry.state {
            throw ServiceError(code: .sendBroadcastOutcomeUnknown, detail: "a broadcast of this transaction is in flight")
        }
        let tx = entry.handle
        prepared[transaction.id]?.state = .broadcasting
        do {
            let outcome = try await serviceCall { () async throws(DashKitError) in try await handle.broadcast(tx) }
            prepared[transaction.id] = nil
            return BroadcastResult(txid: outcome.txid, peersAnnounced: outcome.peersAnnounced)
        } catch {
            switch error.code {
            case .invalidArgument, .networkNotOpen, .notImplemented, .walletNotFound:
                // Not dispatched. A transaction that was uncertain before
                // this call stays uncertain.
                prepared[transaction.id]?.state = entry.state
            case .sendNoPeers, .sendPreparedTxSpent, .sendBroadcastRejected:
                // The engine released the inputs and spent the PreparedTx
                // (m1-engine.md §2.7): nothing is left to abandon or retry.
                prepared[transaction.id] = nil
            default:
                prepared[transaction.id]?.state = .outcomeUnknown
            }
            throw error
        }
    }

    public func abandon(_ transaction: PreparedTransaction) async throws(ServiceError) {
        guard let entry = prepared[transaction.id] else { return }
        guard case .ready = entry.state else {
            throw ServiceError(
                code: .sendBroadcastOutcomeUnknown, detail: "the broadcast may have reached a peer; inputs stay reserved")
        }
        try await release(transaction.id, entry)
    }

    /// Abandons every prepared transaction that was never broadcast. A
    /// failure leaves it held, so a later `abandon` can retry it.
    private func abandonUnsent() async {
        for id in Array(prepared.keys) {
            // Re-read: a broadcast may have started while an earlier release awaited.
            guard let entry = prepared[id], case .ready = entry.state else { continue }
            try? await release(id, entry)
        }
    }

    /// Abandons `entry` in the engine. It leaves `prepared` first, so a
    /// broadcast that arrives while the engine releases the inputs fails with
    /// `send.prepared_tx_unknown`; on failure it is put back.
    private func release(_ id: UUID, _ entry: Entry) async throws(ServiceError) {
        prepared[id] = nil
        let tx = entry.handle
        do {
            try await serviceCall { () async throws(DashKitError) in try await handle.abandon(tx) }
        } catch {
            prepared[id] = entry
            throw error
        }
    }
}
