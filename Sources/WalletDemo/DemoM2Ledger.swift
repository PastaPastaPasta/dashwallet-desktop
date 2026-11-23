// M2 transaction rules over the demo ledger: dash-qt's abandon / resend
// conditions (m2-engine.md §2.2) and the details extras.
import Foundation
import WalletRuntime

extension DemoLedger {
    func transaction(_ txid: String) -> DemoTransaction? {
        transactions.first { $0.txid == txid }
    }

    /// `tx_detail_extras`. SPV sees no mempool: `inMempool` is `nil`, except
    /// for incoming transactions, which a peer announced.
    func extras(txid: String) -> TransactionDetailExtras? {
        guard let tx = transaction(txid) else { return nil }
        let credit = tx.outputs.filter(\.isMine).reduce(Int64(0)) { $0 + $1.amount.duffs }
        let debit = tx.inputs.filter(\.isMine).reduce(Int64(0)) { $0 + ($1.amount?.duffs ?? 0) }
        let sentByWallet = tx.inputs.contains(where: \.isMine)
        return TransactionDetailExtras(
            txid: txid, isCoinbase: tx.type == .generated, totalCredit: Amount(duffs: credit),
            totalDebit: sentByWallet ? Amount(duffs: debit) : .zero, net: Amount(duffs: tx.net), maturesIn: nil,
            inMempool: sentByWallet || tx.confirmations > 0 ? nil : true, abandoned: abandoned.contains(txid),
            canAbandon: abandonRefusal(txid) == nil, canResend: resendRefusal(txid) == nil, dustLockedOutputs: [],
            lastAnnouncedAt: nil)
    }

    /// dash-qt Abandon: unconfirmed, not abandoned, not InstantSend-locked,
    /// not announced by a peer (incoming transactions were).
    func abandonRefusal(_ txid: String) -> TransactionActionRefusal? {
        guard let tx = transaction(txid) else { return nil }
        if abandoned.contains(txid) { return .alreadyAbandoned }
        if tx.type == .generated { return .coinbase }
        if tx.confirmations > 0 { return .confirmed }
        if tx.instantLocked { return .instantLocked }
        if !tx.inputs.contains(where: \.isMine) { return .inMempool }
        return nil
    }

    /// dash-qt Resend: unconfirmed, not abandoned, not coinbase, not
    /// InstantSend-locked and funded by the wallet.
    func resendRefusal(_ txid: String) -> TransactionActionRefusal? {
        guard let tx = transaction(txid) else { return nil }
        if abandoned.contains(txid) { return .alreadyAbandoned }
        if tx.type == .generated { return .coinbase }
        if tx.confirmations > 0 { return .confirmed }
        if tx.instantLocked { return .instantLocked }
        if !tx.inputs.contains(where: \.isMine) { return .notSentByWallet }
        return nil
    }

    /// Marks the transaction abandoned: its outputs leave the coins and the
    /// wallet's inputs become spendable again.
    mutating func abandon(_ txid: String) {
        guard let tx = transaction(txid), !abandoned.contains(txid) else { return }
        abandoned.insert(txid)
        coins.removeAll { $0.outpoint.txid == txid }
        for input in tx.inputs where input.isMine {
            guard let address = input.address, let amount = input.amount,
                !coins.contains(where: { $0.outpoint == input.previousOutput })
            else { continue }
            coins.append(DemoCoin(
                outpoint: input.previousOutput, address: address, amount: amount.duffs, confirmations: 1,
                date: tx.date, instantLocked: false, isChange: changeAddresses.contains(address)))
        }
    }
}
