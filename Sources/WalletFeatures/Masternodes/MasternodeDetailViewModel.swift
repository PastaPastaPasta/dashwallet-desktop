// "Details for Masternode <hash>" (QT-122) and the iOS masternode detail
// (IOS-080): every §10.1 field, the shares table of a shared masternode, and
// an EvoNode's Platform status (claimable credits, blocks this epoch), which
// stays visible as "Not available yet" while the engine answers
// `not_implemented` (Platform work, M4). Also the Unban entry (IOS-081).
import Foundation
import Observation
import WalletRuntime

public struct MasternodeShareRow: Sendable, Hashable, Identifiable {
    public var id: Int { number }
    public let number: Int
    public let amount: String
    public let ownerAddress: String
    public let payoutAddress: String
    public let refundAddress: String
    public let mine: Bool
}

public enum EvonodeStatusState: Sendable, Hashable {
    case notEvonode
    case loading
    case loaded(EvonodePlatformStatus)
    case unavailable
    case failed(String)
}

@MainActor
@Observable
public final class MasternodeDetailViewModel {
    public let proTxHash: String
    public private(set) var detail: MasternodeDetail?
    public private(set) var evonode: EvonodeStatusState = .notEvonode
    public private(set) var errorMessage: String?
    public private(set) var available = true

    public var title: String { L10n.Masternodes.detailsTitle(proTxHash) }

    /// Banned: offer Unban (a service update, IOS-081).
    public var canUnban: Bool {
        guard let detail, case .banned = detail.row.status else { return false }
        return true
    }

    public var lines: [DetailLine] {
        guard let detail else { return [] }
        let row = detail.row
        typealias M = L10n.Masternodes
        let dash = M.dash
        let fullNode = M.fullNodeOnly
        func optional(_ title: String, _ value: String?) -> DetailLine {
            DetailLine(title, value ?? dash, tooltip: value == nil ? fullNode : nil)
        }
        var lines = [
            DetailLine(M.fieldProTxHash, row.proTxHash),
            DetailLine(M.fieldOperatorKey, row.operatorPublicKey),
            optional(M.fieldOwner, row.ownerAddress),
            optional(M.fieldPayout, row.payoutAddresses.isEmpty ? nil : row.payoutAddresses.joined(separator: ", ")),
            DetailLine(M.fieldVoting, row.votingAddress),
            optional(M.fieldCollateralAddress, row.collateralAddress),
            optional(M.fieldCollateralHash, row.collateral?.txid),
            optional(M.fieldCollateralIndex, row.collateral.map { "\($0.vout)" }),
            DetailLine(M.fieldType, typeText(row)),
            optional(M.fieldRegistered, row.registeredHeight.map { "\($0)" }),
            optional(M.fieldLastPaid, row.lastPaidHeight.map { "\($0)" }),
            optional(M.fieldConsecutivePayments, detail.consecutivePayments.map { "\($0)" }),
            optional(M.fieldOperatorReward, row.operatorReward.map { M3Validation.percent($0.percentX100) + "%" }),
            DetailLine(M.fieldService, detail.networkAddresses.isEmpty ? dash : detail.networkAddresses.joined(separator: ", ")),
        ]
        if row.type == .evo {
            lines += [
                DetailLine(M.fieldPlatformNodeID, row.platformNodeID ?? dash),
                DetailLine(M.fieldPlatformP2P, detail.platformP2PAddresses.isEmpty
                    ? dash : detail.platformP2PAddresses.joined(separator: ", ")),
                DetailLine(M.fieldPlatformHTTPS, detail.platformHTTPSAddresses.isEmpty
                    ? dash : detail.platformHTTPSAddresses.joined(separator: ", ")),
            ]
        }
        lines += [
            optional(M.fieldPoSePenalty, row.poseScore.map { "\($0)" }),
            optional(M.fieldPoSeBan, detail.poseBanHeight.map { "\($0)" }),
            optional(M.fieldPoSeRevived, detail.poseRevivedHeight.map { "\($0)" }),
        ]
        if row.shared != nil {
            lines += [
                optional(M.fieldEarlyPeriod, detail.earlyPeriodEnd.map { "\($0)" }),
                optional(M.fieldEarlyPenalty, detail.earlyExitPenalty.map(amount)),
                DetailLine(M.fieldStandby, detail.hasStandbyDissolution ? M.standbySaved : M.standbyNone),
            ]
        }
        if let reason = detail.revocationReason, reason >= 0, reason < RevocationReason.allCases.count {
            lines.append(DetailLine(M.fieldRevocation, M.revocationReason(RevocationReason.allCases[reason])))
        }
        if !row.ownedRoles.isEmpty {
            lines.append(DetailLine(
                M.fieldOwnedBecause,
                OwnedRole.allCases.filter(row.ownedRoles.contains).map(M.ownedRole).joined(separator: ", ")))
        }
        lines.append(DetailLine(M.fieldWalletTransactions, "\(detail.walletTransactions)"))
        return lines
    }

    /// The shares table (QT-122, shared masternodes).
    public var shares: [MasternodeShareRow] {
        (detail?.shares ?? []).enumerated().map { index, share in
            MasternodeShareRow(
                number: index + 1, amount: amount(share.amount), ownerAddress: share.ownerAddress,
                payoutAddress: share.payoutAddress, refundAddress: share.refundAddress, mine: share.mine)
        }
    }

    /// The EvoNode Platform rows: values, or "Not available yet".
    public var platformLines: [DetailLine] {
        typealias M = L10n.Masternodes
        switch evonode {
        case .notEvonode:
            return []
        case .loading:
            return [DetailLine(M.platformStatus, "…")]
        case .unavailable:
            return [DetailLine(M.claimableCredits, M.notAvailableYet), DetailLine(M.epochBlocks, M.notAvailableYet)]
        case .failed(let text):
            return [DetailLine(M.platformStatus, text)]
        case .loaded(let status):
            return [
                DetailLine(M.claimableCredits, status.claimableCredits.map { "\($0)" } ?? M.dash),
                DetailLine(M.epochBlocks, status.epochProposedBlocks.map { "\($0)" } ?? M.dash),
            ]
        }
    }

    private let masternodes: any MasternodeListProviding
    private let evonodes: any EvonodeServicing
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding

    public init(
        proTxHash: String, masternodes: any MasternodeListProviding, evonodes: any EvonodeServicing,
        amounts: any AmountFormatting, settings: any SettingsProviding
    ) {
        self.proTxHash = proTxHash
        self.masternodes = masternodes
        self.evonodes = evonodes
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(proTxHash: String, env: AppEnvironment, m3: M3Services) {
        self.init(
            proTxHash: proTxHash, masternodes: m3.masternodes, evonodes: m3.evonodes, amounts: env.amounts,
            settings: env.settings)
    }

    private func amount(_ value: Amount) -> String { AmountText(amounts: amounts, settings: settings)(value) }

    private func typeText(_ row: MasternodeRow) -> String {
        if let shared = row.shared { return L10n.Masternodes.sharedHolding(shared.heldShares, of: shared.totalShares) }
        return row.type == .evo ? L10n.Masternodes.evo : L10n.Masternodes.regular
    }

    public func load() async {
        do {
            detail = try await masternodes.detail(proTxHash: proTxHash)
            available = true
        } catch {
            if error.isNotImplemented {
                available = false
            } else {
                errorMessage = ErrorText.m3(error, amount: amount)
            }
            return
        }
        guard detail?.row.type == .evo else {
            evonode = .notEvonode
            return
        }
        evonode = .loading
        do {
            evonode = .loaded(try await evonodes.status(proTxHash: proTxHash))
        } catch {
            evonode = error.isNotImplemented ? .unavailable : .failed(ErrorText.m3(error, amount: amount))
        }
    }
}
