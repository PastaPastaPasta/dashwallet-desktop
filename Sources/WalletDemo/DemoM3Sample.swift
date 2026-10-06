// Deterministic M3 sample data for demo mode: a masternode list and the
// current cycle's proposals. Values an SPV wallet cannot know about other
// people's masternodes (PoSe, payments, owner/payout addresses) stay `nil`,
// as the engine reports them (m3-engine.md §7).
import Foundation
import WalletFeatures
import WalletRuntime

struct DemoM3Sample {
    let masternodes: [DemoMasternode]
    let proposals: [DemoProposal]

    static func hash(_ text: String) -> String {
        var rng = DemoRandom(text: text)
        return rng.hex(bytes: 32)
    }

    init(network: DashNetwork, now: Date) {
        var rng = DemoRandom(text: "m3-sample|" + network.description)
        let port = M3Defaults.masternodeDefaults(network).coreP2PPort
        let tip = DemoLedger.tipHeight
        var nodes: [DemoMasternode] = []
        for index in 0..<14 {
            let type: MasternodeType = [1, 5, 9, 12].contains(index) ? .evo : .regular
            let shared: SharedHolding? = index == 2 ? SharedHolding(heldShares: 1, totalShares: 3) : nil
            let owned: Set<OwnedRole> =
                switch index {
                case 0: [.collateral, .owner, .voting, .payout]
                case 1: [.owner, .voting, .operator]
                case 2: [.shareOwner, .shareRefund]
                default: []
                }
            let mine = !owned.isEmpty
            let status: MasternodeListStatus =
                index == 7 || index == 11 ? .banned(sinceHeight: tip - UInt32(40 + index)) : .active(sinceHeight: tip - UInt32(2_000 + index * 977))
            let service = "\(34 + index % 9).\(rng.next() % 200 + 20).\(rng.next() % 250).\(rng.next() % 250):\(port)"
            let proTxHash = rng.hex(bytes: 32)
            let collateral = mine ? OutPoint(txid: rng.hex(bytes: 32), vout: UInt32(index % 2)) : nil
            let payout = mine ? [rng.address(on: network)] : []
            let row = MasternodeRow(
                proTxHash: proTxHash, service: service, type: type, shared: shared, status: status, poseScore: nil,
                registeredHeight: mine ? tip - UInt32(30_000 + index * 1_000) : nil, lastPaidHeight: nil,
                nextPaymentHeight: nil,
                operatorReward: mine ? OperatorReward(percentX100: index == 1 ? 1_000 : 0, payoutAddress: nil) : nil,
                collateral: collateral, collateralAddress: mine ? rng.address(on: network) : nil,
                ownerAddress: owned.contains(.owner) ? rng.address(on: network) : nil, votingAddress: rng.address(on: network),
                payoutAddresses: payout, operatorPublicKey: rng.hex(bytes: 48),
                platformNodeID: type == .evo ? rng.hex(bytes: 20) : nil, ownedRoles: owned,
                label: index == 0 ? "Home node" : nil)
            let shares: [MasternodeShare] = shared == nil ? [] : [
                MasternodeShare(
                    amount: Amount(duffs: 400 * Amount.duffsPerDash), ownerAddress: rng.address(on: network),
                    payoutAddress: rng.address(on: network), refundAddress: rng.address(on: network), mine: true),
                MasternodeShare(
                    amount: Amount(duffs: 300 * Amount.duffsPerDash), ownerAddress: rng.address(on: network),
                    payoutAddress: rng.address(on: network), refundAddress: rng.address(on: network), mine: false),
                MasternodeShare(
                    amount: Amount(duffs: 300 * Amount.duffsPerDash), ownerAddress: rng.address(on: network),
                    payoutAddress: rng.address(on: network), refundAddress: rng.address(on: network), mine: false),
            ]
            let detail = MasternodeDetail(
                row: row, consecutivePayments: nil, poseBanHeight: nil, poseRevivedHeight: nil,
                networkAddresses: [service],
                platformP2PAddresses: type == .evo ? [service.replacingOccurrences(of: ":\(port)", with: ":22000")] : [],
                platformHTTPSAddresses: type == .evo ? [service.replacingOccurrences(of: ":\(port)", with: ":22001")] : [],
                shares: shares, earlyPeriodEnd: shared == nil ? nil : tip + 12_000,
                earlyExitPenalty: shared == nil ? nil : Amount(duffs: 25 * Amount.duffsPerDash),
                hasStandbyDissolution: false, revocationReason: nil, walletTransactions: mine ? 1 : 0)
            nodes.append(DemoMasternode(row: row, detail: detail, votingKeyInWallet: owned.contains(.voting)))
        }
        masternodes = nodes

        let threshold = 30
        struct Script {
            let name: String
            let status: ProposalStatus
            let yes: Int, no: Int, abstain: Int
            let dash: Int64
            let payments: Int
            var mine = false
        }
        let scripts = [
            Script(name: "dash-core-group-q4", status: .funded, yes: 212, no: 12, abstain: 3, dash: 3_000, payments: 3),
            Script(name: "dashpay-ux-research", status: .passing, yes: 96, no: 20, abstain: 4, dash: 450, payments: 2),
            Script(name: "evonode-docs-refresh", status: .voting, yes: 31, no: 9, abstain: 0, dash: 120, payments: 1),
            Script(name: "conference-booth-2026", status: .unfunded, yes: 70, no: 25, abstain: 2, dash: 900, payments: 1),
            Script(name: "merch-giveaway", status: .failing, yes: 8, no: 41, abstain: 6, dash: 75, payments: 1),
            Script(name: "translations-sprint", status: .confirming, yes: 0, no: 0, abstain: 0, dash: 60, payments: 2),
            Script(name: "demo-wallet-docs", status: .voting, yes: 18, no: 2, abstain: 1, dash: 40, payments: 1, mine: true),
        ]
        let cycle = TimeInterval(M3Defaults.governanceParameters(network).superblockCycle) * DemoLedger.blockSeconds
        proposals = scripts.enumerated().map { index, script in
            let hash = Self.hash("proposal|\(network.description)|\(script.name)")
            let start = now.addingTimeInterval(-cycle / 2 - TimeInterval(index) * 3_600)
            let row = ProposalRow(
                hash: hash, name: script.name, url: "https://www.dashcentral.org/p/\(script.name)",
                paymentAddress: rng.address(on: network), paymentAmount: Amount(duffs: script.dash * Amount.duffsPerDash),
                start: start, end: start.addingTimeInterval(cycle * Double(script.payments)), status: script.status,
                collateralConfirmations: script.status == .confirming ? 3 : 2_000, yes: script.yes, no: script.no,
                abstain: script.abstain, margin: script.yes - script.no - threshold, myVotes: nil)
            return DemoProposal(
                row: row, parentHash: "0", collateralTxid: Self.hash("collateral|\(hash)"),
                createdAt: start.addingTimeInterval(-86_400), payments: script.payments, mine: script.mine)
        }
    }
}
