// Provider transactions of the M3 demo (QT-125, QT-127, IOS-081): prepare
// checks the grant and holds the transaction; broadcast applies it to the
// sample list (a service update revives a banned node, an operator-key
// change bans it, a revoke retires it, a dissolution removes it).
import Foundation
import WalletFeatures
import WalletRuntime

/// What a prepared provider transaction changes once broadcast.
enum DemoProviderEffect: Sendable {
    case service([String])
    case registrar(operatorKey: String?, voting: String?, payout: String?)
    case revoke(RevocationReason)
    case shareReward
    case dissolve
}

extension DemoM3World {
    func prepareProvider(
        _ kind: ProviderTransactionKind, hash: String, wallet: WalletID, grant: AuthGrant, effect: DemoProviderEffect,
        penalty: Amount? = nil, bans: Bool = false
    ) throws(ServiceError) -> PreparedProviderTransaction {
        _ = try masternodeDetail(hash)
        let info = try self.wallet(wallet)
        guard !info.watchOnly else { throw .demo(.masternodeWatchOnly) }
        try world.check(grant, .masternodeOperation, wallet: wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        try world.redeem(grant, .masternodeOperation, wallet: wallet, refuse: .masternode)
        let id = UUID()
        let summary = ProviderTransactionSummary(
            kind: kind, proTxHash: hash, txid: DemoM3Sample.hash("protx|\(id)"), fee: Amount(duffs: 1_000),
            penalty: penalty, bansMasternode: bans)
        providerTransactions[id] = (summary, effect)
        return PreparedProviderTransaction(id: id, summary: summary)
    }

    func abandonProvider(_ id: UUID) {
        providerTransactions[id] = nil
    }

    func broadcastProvider(_ id: UUID) throws(ServiceError) -> String {
        guard let (summary, effect) = providerTransactions[id] else {
            throw .demo(.invalidArgument, "unknown prepared transaction")
        }
        guard world.sync.connectedPeers > 0 else { throw .demo(.masternodeNoPeers) }
        providerTransactions[id] = nil
        guard let index = masternodes.firstIndex(where: { $0.row.proTxHash == summary.proTxHash }) else {
            throw .demo(.masternodeNotFound)
        }
        let r = masternodes[index].row
        var service = r.service, status = r.status, voting = r.votingAddress, payout = r.payoutAddresses
        var operatorKey = r.operatorPublicKey
        var addresses = masternodes[index].detail.networkAddresses
        var reason: Int?
        switch effect {
        case .service(let list):
            service = list.first ?? service
            if !list.isEmpty { addresses = list }
            if case .banned = status { status = .active(sinceHeight: tip) }
        case .registrar(let key, let newVoting, let newPayout):
            if let key {
                operatorKey = key
                status = .banned(sinceHeight: tip)
            }
            if let newVoting { voting = newVoting }
            if let newPayout { payout = [newPayout] }
        case .revoke(let why):
            status = .retired
            reason = RevocationReason.allCases.firstIndex(of: why)
        case .shareReward:
            break
        case .dissolve:
            masternodes.remove(at: index)
            masternodeChanges.send(())
            return summary.txid
        }
        let row = MasternodeRow(
            proTxHash: r.proTxHash, service: service, type: r.type, shared: r.shared, status: status,
            poseScore: r.poseScore, registeredHeight: r.registeredHeight, lastPaidHeight: r.lastPaidHeight,
            nextPaymentHeight: r.nextPaymentHeight, operatorReward: r.operatorReward, collateral: r.collateral,
            collateralAddress: r.collateralAddress, ownerAddress: r.ownerAddress, votingAddress: voting,
            payoutAddresses: payout, operatorPublicKey: operatorKey, platformNodeID: r.platformNodeID,
            ownedRoles: r.ownedRoles, label: r.label)
        let d = masternodes[index].detail
        masternodes[index].row = row
        masternodes[index].detail = MasternodeDetail(
            row: row, consecutivePayments: d.consecutivePayments, poseBanHeight: d.poseBanHeight,
            poseRevivedHeight: d.poseRevivedHeight, networkAddresses: addresses,
            platformP2PAddresses: d.platformP2PAddresses, platformHTTPSAddresses: d.platformHTTPSAddresses,
            shares: d.shares, earlyPeriodEnd: d.earlyPeriodEnd, earlyExitPenalty: d.earlyExitPenalty,
            hasStandbyDissolution: d.hasStandbyDissolution, revocationReason: reason ?? d.revocationReason,
            walletTransactions: d.walletTransactions + 1)
        masternodeChanges.send(())
        return summary.txid
    }
}
