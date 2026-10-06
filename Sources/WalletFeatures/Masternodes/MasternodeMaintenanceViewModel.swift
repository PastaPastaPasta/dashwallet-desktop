// The single-party provider transactions (QT-125, QT-127, IOS-081): Update
// Service (also "Unban", which revives a PoSe-banned node, with its pending
// state), Update Registrar (owner key; changing the operator key PoSe-bans),
// Revoke (reason), Change Reward Address of a share, Dissolve now, and the
// standby dissolution to save. Each is prepare → review → broadcast; the
// typed operator secret becomes a zeroing buffer at prepare.
import Foundation
import Observation
import WalletRuntime

public enum MaintenanceKind: Sendable, Hashable {
    case updateService
    /// Update Service of a banned masternode (IOS-081).
    case unban
    case updateRegistrar
    case revoke
    case changeRewardAddress
    case dissolveNow
    case standbyDissolution

    public var title: String {
        typealias M = L10n.Masternodes
        switch self {
        case .updateService: return M.updateServiceTitle
        case .unban: return M.unbanTitle
        case .updateRegistrar: return M.updateRegistrarTitle
        case .revoke: return M.revokeTitle
        case .changeRewardAddress: return String(M.changeRewardAddress.dropLast())
        case .dissolveNow: return M.dissolveNow
        case .standbyDissolution: return String(M.createStandby.dropLast())
        }
    }
}

public enum MaintenanceStep: Sendable, Hashable {
    case loading
    case unavailable
    case editing
    case needsPassphrase
    case preparing
    case review(ProviderTransactionSummary)
    case broadcasting
    case sent(txid: String)
    /// The standby dissolution is ready to save as `.txt`.
    case standbyReady(StandbyDissolution)
}

@MainActor
@Observable
public final class MasternodeMaintenanceViewModel {
    public let kind: MaintenanceKind
    public let proTxHash: String

    // Fields
    public var serviceText = ""
    /// Typed each time (dash-qt); empty = a key the wallet or a tracked
    /// masternode holds. Cleared once it became a secret buffer.
    public var operatorSecretText = ""
    public var platformNodeID = ""
    public var platformP2PText = ""
    public var platformHTTPSText = ""
    public var operatorPayoutAddress = ""
    public var operatorPublicKey = ""
    public var votingAddress = ""
    public var payoutAddress = ""
    public var reason: RevocationReason = .notSpecified
    public var shareIndex = 0
    public var acceptPenalty = false
    /// `nil` = Automatic (recommended).
    public var feeSource: String?

    public private(set) var step: MaintenanceStep = .loading
    public private(set) var detail: MasternodeDetail?
    public private(set) var feeSources: [FeeSourceCandidate] = []
    public private(set) var errorMessage: String?
    public private(set) var message: String?

    public var isEvo: Bool { detail?.row.type == .evo }
    /// The operator payout field shows only with an operator reward.
    public var showsOperatorPayout: Bool { (detail?.row.operatorReward?.percentX100 ?? 0) > 0 }
    /// Several payout addresses: the payout field is disabled ("use the RPC").
    public var payoutLocked: Bool { (detail?.row.payoutAddresses.count ?? 0) > 1 }
    public var payoutNote: String? { payoutLocked ? L10n.Masternodes.multiplePayoutsNote : nil }
    public var isBanned: Bool {
        if case .banned = detail?.row.status { return true }
        return false
    }

    /// The early period is running: Dissolve now pays the penalty.
    public var penalty: Amount? {
        guard kind == .dissolveNow, let detail, let end = detail.earlyPeriodEnd, let height = tipHeight, height < end
        else { return nil }
        return detail.earlyExitPenalty
    }

    public var warning: String? {
        switch kind {
        case .updateService, .unban: return isBanned ? L10n.Masternodes.revivesBanned : nil
        case .updateRegistrar:
            let current = detail?.row.operatorPublicKey ?? ""
            let typed = operatorPublicKey.trimmingCharacters(in: .whitespaces)
            return !typed.isEmpty && typed.lowercased() != current.lowercased() ? L10n.Masternodes.bansWarning : nil
        case .revoke: return L10n.Masternodes.revokeWarning
        default: return nil
        }
    }

    public var reviewLines: [String] {
        guard case .review(let summary) = step else { return [] }
        var lines = [L10n.Masternodes.reviewFee(amount(summary.fee))]
        if let penalty = summary.penalty { lines.append(L10n.Masternodes.reviewPenalty(amount(penalty))) }
        if summary.bansMasternode { lines.append(L10n.Masternodes.bansWarning) }
        return lines
    }

    private let masternodes: any MasternodeListProviding
    private let maintenance: any MasternodeMaintaining
    private let registration: any MasternodeRegistering
    private let walletState: any WalletStateProviding
    private let sync: any SyncStatusProviding
    private let vault: any VaultProviding
    private let grants: GrantRequester
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding
    private let desktopPreferences: any DesktopPreferencesStoring
    private let timing: Timing
    private var prepared: PreparedProviderTransaction?
    private var tipHeight: UInt32? { sync.status?.tipHeight }

    public init(
        kind: MaintenanceKind, proTxHash: String, masternodes: any MasternodeListProviding,
        maintenance: any MasternodeMaintaining, registration: any MasternodeRegistering,
        walletState: any WalletStateProviding, sync: any SyncStatusProviding, auth: any AuthenticationGating,
        vault: any VaultProviding, amounts: any AmountFormatting, settings: any SettingsProviding,
        desktopPreferences: any DesktopPreferencesStoring, timing: Timing
    ) {
        self.kind = kind
        self.proTxHash = proTxHash
        self.masternodes = masternodes
        self.maintenance = maintenance
        self.registration = registration
        self.walletState = walletState
        self.sync = sync
        self.vault = vault
        grants = GrantRequester(auth: auth, vault: vault)
        self.amounts = amounts
        self.settings = settings
        self.desktopPreferences = desktopPreferences
        self.timing = timing
    }

    public convenience init(
        kind: MaintenanceKind, proTxHash: String, env: AppEnvironment, m2: M2Services, m3: M3Services
    ) {
        self.init(
            kind: kind, proTxHash: proTxHash, masternodes: m3.masternodes, maintenance: m3.maintenance,
            registration: m3.registration, walletState: env.walletState, sync: env.sync, auth: env.auth,
            vault: env.vault, amounts: env.amounts, settings: env.settings,
            desktopPreferences: m2.desktopPreferences, timing: env.timing)
    }

    private func amount(_ value: Amount) -> String { AmountText(amounts: amounts, settings: settings)(value) }
    private func errorText(_ error: ServiceError) -> String { ErrorText.m3(error, amount: amount) }

    /// Reads the masternode to prefill the fields, and the fee sources.
    public func load() async {
        step = .loading
        do {
            let detail = try await masternodes.detail(proTxHash: proTxHash)
            self.detail = detail
            serviceText = detail.networkAddresses.joined(separator: ", ")
            platformNodeID = detail.row.platformNodeID ?? ""
            platformP2PText = detail.platformP2PAddresses.joined(separator: ", ")
            platformHTTPSText = detail.platformHTTPSAddresses.joined(separator: ", ")
            operatorPayoutAddress = detail.row.operatorReward?.payoutAddress ?? ""
            operatorPublicKey = detail.row.operatorPublicKey
            votingAddress = detail.row.votingAddress
            payoutAddress = detail.row.payoutAddresses.first ?? ""
            shareIndex = detail.shares.firstIndex(where: \.mine) ?? 0
        } catch {
            if error.isNotImplemented {
                step = .unavailable
            } else {
                errorMessage = errorText(error)
                step = .editing
            }
            return
        }
        if let wallet = walletState.selectedWalletID {
            feeSources = (try? await registration.feeSourceCandidates(wallet: wallet)) ?? []
        }
        step = .editing
    }

    /// Prepares the transaction with a `.masternodeOperation` grant (nothing
    /// is sent); Review shows the fee. An encrypted vault needs `passphrase`.
    public func prepare(passphrase: String? = nil) async {
        guard step == .editing || step == .needsPassphrase, let wallet = walletState.selectedWalletID else { return }
        errorMessage = nil
        if kind == .dissolveNow, penalty != nil, !acceptPenalty {
            errorMessage = L10n.Masternodes.acceptPenalty
            return
        }
        if (kind == .updateService || kind == .unban),
            M3Validation.list(serviceText).contains(where: { !M3Validation.isService($0) })
        {
            errorMessage = L10n.Masternodes.enterService
            return
        }
        if kind == .updateRegistrar {
            let changes = registrarChanges
            guard changes.operatorKey != nil || changes.voting != nil || changes.payout != nil else {
                errorMessage = L10n.Masternodes.noChanges
                return
            }
        }
        step = .preparing
        let grant: AuthGrant
        do {
            guard let issued = try await grants.authorize(.masternodeOperation, wallet: wallet, passphrase: passphrase)
            else {
                step = .needsPassphrase
                return
            }
            grant = issued
        } catch {
            errorMessage = errorText(error)
            step = error.code == .vaultWrongPassphrase ? .needsPassphrase : .editing
            return
        }
        do {
            switch kind {
            case .standbyDissolution:
                let standby = try await maintenance.createStandbyDissolution(
                    wallet: wallet, proTxHash: proTxHash, grant: grant)
                let now = timing.now()
                desktopPreferences.updateM3 { $0.standbyDissolutions[self.proTxHash] = now }
                step = .standbyReady(standby)
                return
            default:
                let transaction = try await prepareTransaction(wallet: wallet, grant: grant)
                prepared = transaction
                step = .review(transaction.summary)
            }
        } catch {
            errorMessage = errorText(error)
            step = error.isNotImplemented ? .unavailable : .editing
        }
    }

    private func prepareTransaction(wallet: WalletID, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        let fee: FeeSourceChoice = feeSource.map { .address($0) } ?? .automatic
        let typed = operatorSecretText.trimmingCharacters(in: .whitespaces)
        let secret: (any SecretBuffer)? = typed.isEmpty ? nil : vault.makeSecret(utf8: typed)
        operatorSecretText = ""
        func trimmed(_ text: String) -> String? {
            let value = text.trimmingCharacters(in: .whitespaces)
            return value.isEmpty ? nil : value
        }
        switch kind {
        case .updateService, .unban:
            let platform = isEvo
                ? PlatformFields(
                    nodeIDHex: platformNodeID.trimmingCharacters(in: .whitespaces),
                    p2pAddresses: M3Validation.list(platformP2PText), httpsAddresses: M3Validation.list(platformHTTPSText))
                : nil
            return try await maintenance.prepareUpdateService(
                UpdateServiceRequest(
                    proTxHash: proTxHash, serviceAddresses: M3Validation.list(serviceText), operatorSecret: secret,
                    platform: platform, operatorPayoutAddress: showsOperatorPayout ? trimmed(operatorPayoutAddress) : nil,
                    feeSource: fee, feeWallet: wallet),
                grant: grant)
        case .updateRegistrar:
            let changes = registrarChanges
            return try await maintenance.prepareUpdateRegistrar(
                UpdateRegistrarRequest(
                    proTxHash: proTxHash, operatorPublicKey: changes.operatorKey, votingAddress: changes.voting,
                    payoutAddress: changes.payout, feeSource: fee, feeWallet: wallet),
                grant: grant)
        case .revoke:
            return try await maintenance.prepareRevoke(
                RevokeRequest(proTxHash: proTxHash, operatorSecret: secret, reason: reason, feeSource: fee, feeWallet: wallet),
                grant: grant)
        case .changeRewardAddress:
            return try await maintenance.prepareShareRewardUpdate(
                proTxHash: proTxHash, shareIndex: shareIndex, payoutAddress: payoutAddress.trimmingCharacters(in: .whitespaces),
                feeWallet: wallet, grant: grant)
        case .dissolveNow:
            return try await maintenance.prepareDissolveNow(
                proTxHash: proTxHash, acceptPenalty: acceptPenalty, feeWallet: wallet, grant: grant)
        case .standbyDissolution:
            throw ServiceError(code: .invalidArgument, detail: "standby is not a provider transaction")
        }
    }

    /// Update Registrar sends only the fields that changed.
    private var registrarChanges: (operatorKey: String?, voting: String?, payout: String?) {
        let row = detail?.row
        func changed(_ text: String, from current: String?) -> String? {
            let value = text.trimmingCharacters(in: .whitespaces)
            return value.isEmpty || value.lowercased() == current?.lowercased() ? nil : value
        }
        return (
            changed(operatorPublicKey, from: row?.operatorPublicKey), changed(votingAddress, from: row?.votingAddress),
            payoutLocked ? nil : changed(payoutAddress, from: row?.payoutAddresses.first)
        )
    }

    /// Sends the reviewed transaction.
    public func broadcast() async {
        guard case .review = step, let prepared else { return }
        step = .broadcasting
        do {
            let txid = try await maintenance.broadcast(prepared)
            self.prepared = nil
            message = kind == .unban || (kind == .updateService && isBanned)
                ? L10n.Masternodes.unbanPending : L10n.Masternodes.sent(txid)
            step = .sent(txid: txid)
        } catch {
            errorMessage = errorText(error)
            step = .review(prepared.summary)
        }
    }

    /// Back from Review to the fields: the prepared transaction is released.
    public func backToEditing() async {
        if let prepared {
            self.prepared = nil
            await maintenance.abandon(prepared)
        }
        step = .editing
    }

    public func cancel() async {
        operatorSecretText = ""
        if let prepared {
            self.prepared = nil
            await maintenance.abandon(prepared)
        }
    }

    /// The standby file's text: one raw transaction per line.
    public var standbyFileText: String? {
        guard case .standbyReady(let standby) = step else { return nil }
        return standby.transactionsHex.joined(separator: "\n") + "\n"
    }
}
