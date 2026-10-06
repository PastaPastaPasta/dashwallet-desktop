// dash-qt's Register Masternode / Register EvoNode wizard (QT-123, QT-124):
// Type → Collateral → Service → Keys → Payout → Platform (Evo) → Fee →
// Review → Save operator key → Prove ownership (external) → Complete, with
// "Step %1 of %2 · %3", default ports from the network, dash-qt's field
// checks, the operator-secret gate (type its last 4 characters) before any
// broadcast, and dash-qt's explanations of Core reject reasons.
import Foundation
import Observation
import PlatformServices
import WalletRuntime

public enum RegisterPage: Sendable, Hashable, CaseIterable {
    case type, collateral, service, keys, payout, platform, fee, review, saveKey, sign, complete

    public var title: String {
        typealias M = L10n.Masternodes
        switch self {
        case .type: return M.pageType
        case .collateral: return M.pageCollateral
        case .service: return M.pageService
        case .keys: return M.pageKeys
        case .payout: return M.pagePayout
        case .platform: return M.pagePlatform
        case .fee: return M.pageFee
        case .review: return M.pageReview
        case .saveKey: return M.pageSaveKey
        case .sign: return M.pageSign
        case .complete: return M.pageComplete
        }
    }
}

public enum CollateralMode: Sendable, Hashable, CaseIterable {
    case fundNew, existing, external
}

public enum OperatorKeyMode: Sendable, Hashable, CaseIterable {
    case generate, existing
}

@MainActor
@Observable
public final class RegisterMasternodeWizardViewModel {
    // Answers
    public var type: MasternodeType = .regular
    public var collateralMode: CollateralMode = .fundNew
    public var selectedCollateral: OutPoint?
    public var externalTxid = ""
    public var externalVout = "0"
    public var serviceText = ""
    /// Empty = a fresh wallet address.
    public var ownerAddress = ""
    /// Empty = the owner address.
    public var votingAddress = ""
    public var operatorKeyMode: OperatorKeyMode = .generate
    public var operatorPublicKey = ""
    public var payoutAddress = ""
    /// Percent, two decimals ("12.5").
    public var operatorRewardText = "0"
    public var platformNodeID = ""
    public var platformP2PText = ""
    public var platformHTTPSText = ""
    public var feeSource: String?
    /// The user's typed last 4 characters (QT-124).
    public var last4 = ""
    /// The base64 `signmessage` signature for external collateral.
    public var collateralSignature = ""

    // State
    public private(set) var page: RegisterPage = .type
    public private(set) var collateralCandidates: [CollateralCandidate] = []
    public private(set) var feeSources: [FeeSourceCandidate] = []
    public private(set) var prepared: PreparedRegistrationReference?
    /// The generated operator secret while the Save page shows it.
    public private(set) var operatorSecret: OperatorSecret?
    public private(set) var gateOpen = false
    public private(set) var proTxHash: String?
    public private(set) var needsPassphrase = false
    public private(set) var isWorking = false
    public private(set) var errorMessage: String?
    public private(set) var message: String?
    public private(set) var unavailable = false

    public let defaults: MasternodeNetworkDefaults

    public var windowTitle: String {
        type == .evo ? L10n.Masternodes.registerEvoNode : L10n.Masternodes.registerMasternode
    }

    /// The pages of this registration, in order.
    public var pages: [RegisterPage] {
        RegisterPage.allCases.filter { page in
            switch page {
            case .platform: type == .evo
            case .saveKey: operatorKeyMode == .generate
            case .sign: collateralMode == .external
            default: true
            }
        }
    }

    /// "Step %1 of %2 · %3"; "Complete" at the end.
    public var progressText: String {
        guard page != .complete, let index = pages.firstIndex(of: page) else { return L10n.Masternodes.pageComplete }
        return L10n.Masternodes.step(index + 1, of: pages.count, page.title)
    }

    public var collateralAmount: Amount {
        type == .evo ? defaults.evonodeCollateral : defaults.masternodeCollateral
    }

    public var defaultPortText: String { L10n.Masternodes.defaultPort(defaults.coreP2PPort) }

    /// dash-qt's Next button: Next / Register / Prepare / Continue / Submit / Finish.
    public var nextTitle: String {
        typealias M = L10n.Masternodes
        switch page {
        case .review:
            if operatorKeyMode == .generate { return M.continueTitle }
            return collateralMode == .external ? M.prepare : M.register
        case .saveKey: return collateralMode == .external ? M.continueTitle : M.register
        case .sign: return M.submit
        case .complete: return M.finish
        default: return M.next
        }
    }

    /// Back is refused once the operator secret was shown or the
    /// registration sent.
    public var canGoBack: Bool {
        guard !isWorking, let index = pages.firstIndex(of: page), index > 0 else { return false }
        return page != .saveKey && page != .sign && page != .complete
    }

    public var backTooltip: String? {
        page == .saveKey || page == .sign ? L10n.Masternodes.backUnavailable : nil
    }

    public var canGoNext: Bool {
        guard !isWorking, !unavailable else { return false }
        if page == .saveKey { return !last4.isEmpty }
        return true
    }

    public var nextTooltip: String? {
        page == .saveKey && !gateOpen ? L10n.Masternodes.typeLast4 : nil
    }

    public var feeSourceHint: String {
        collateralMode == .fundNew
            ? L10n.Masternodes.feeSourceFundHint(amount(collateralAmount)) : L10n.Masternodes.feeSourceHint
    }

    public var rewardWarning: String? {
        (M3Validation.rewardX100(operatorRewardText) ?? 0) > 0 ? L10n.Masternodes.operatorRewardWarning : nil
    }

    /// The review page: summary rows from the prepared registration.
    public var reviewLines: [DetailLine] {
        guard let summary = prepared?.summary else { return [] }
        typealias M = L10n.Masternodes
        var lines = [
            DetailLine(M.columnType, summary.type == .evo ? M.evo : M.regular),
            DetailLine(M.columnService, summary.serviceAddresses.isEmpty ? M.dash : summary.serviceAddresses.joined(separator: ", ")),
            DetailLine(M.fieldCollateralHash, "\(summary.collateral.txid)-\(summary.collateral.vout)"),
            DetailLine(M.fieldCollateralAddress, summary.collateralAddress),
            DetailLine(M.fieldOwner, summary.ownerAddress),
            DetailLine(M.fieldVoting, summary.votingAddress),
            DetailLine(M.fieldOperatorKey, summary.operatorPublicKey),
            DetailLine(M.fieldPayout, summary.payoutAddress),
            DetailLine(M.operatorRewardTitle, M3Validation.percent(summary.operatorRewardX100) + "%"),
        ]
        if let platform = summary.platform {
            lines += [
                DetailLine(M.fieldPlatformNodeID, platform.nodeIDHex),
                DetailLine(M.platformP2P, platform.p2pAddresses.joined(separator: ", ")),
                DetailLine(M.platformHTTPS, platform.httpsAddresses.joined(separator: ", ")),
            ]
        }
        lines += [
            DetailLine(M.reviewNetworkFee, amount(summary.fee)),
            DetailLine(M.reviewTotal, amount(summary.totalSpent)),
        ]
        return lines
    }

    /// The message to sign for external collateral.
    public var collateralSignMessage: String? { prepared?.summary.collateralSignMessage }

    private let registration: any MasternodeRegistering
    private let walletState: any WalletStateProviding
    private let grants: GrantRequester
    private let clipboard: any ClipboardProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding

    public init(
        registration: any MasternodeRegistering, defaults: MasternodeNetworkDefaults,
        walletState: any WalletStateProviding, auth: any AuthenticationGating, vault: any VaultProviding,
        clipboard: any ClipboardProviding, amounts: any AmountFormatting, settings: any SettingsProviding
    ) {
        self.registration = registration
        self.defaults = defaults
        self.walletState = walletState
        grants = GrantRequester(auth: auth, vault: vault)
        self.clipboard = clipboard
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            registration: m3.registration, defaults: m3.masternodes.defaults(), walletState: env.walletState,
            auth: env.auth, vault: env.vault, clipboard: m2.clipboard, amounts: env.amounts, settings: env.settings)
    }

    private func amount(_ value: Amount) -> String { AmountText(amounts: amounts, settings: settings)(value) }
    private func errorText(_ error: ServiceError) -> String { ErrorText.m3(error, amount: amount) }

    // MARK: Navigation

    /// Checks the page and moves on; Fee prepares the registration, Review
    /// shows the secret or submits, Save checks the last 4 characters.
    public func next(passphrase: String? = nil) async {
        guard canGoNext else { return }
        errorMessage = nil
        if let problem = validate(page) {
            errorMessage = problem
            return
        }
        switch page {
        case .type:
            advance()
            await loadCollateralCandidates()
        case .collateral, .service, .keys, .payout, .platform:
            advance()
            if page == .fee { await loadFeeSources() }
        case .fee:
            await prepare(passphrase: passphrase)
        case .review:
            if operatorKeyMode == .generate {
                await showOperatorSecret()
            } else if collateralMode == .external {
                advance()
            } else {
                await submit()
            }
        case .saveKey:
            await confirmLast4()
        case .sign:
            await submit()
        case .complete:
            break
        }
    }

    public func back() async {
        guard canGoBack, let index = pages.firstIndex(of: page) else { return }
        errorMessage = nil
        if page == .review { await abandonPrepared() }
        page = pages[index - 1]
    }

    /// Cancel: releases the prepared registration and forgets the secret.
    public func cancel() async {
        operatorSecret = nil
        if proTxHash == nil { await abandonPrepared() }
    }

    private func advance() {
        guard let index = pages.firstIndex(of: page), index + 1 < pages.count else { return }
        page = pages[index + 1]
    }

    // MARK: Pages

    /// dash-qt's per-page checks (`validateCurrentPage`); the engine checks
    /// again on prepare.
    public func validate(_ page: RegisterPage) -> String? {
        typealias M = L10n.Masternodes
        switch page {
        case .collateral:
            switch collateralMode {
            case .fundNew: return nil
            case .existing:
                guard selectedCollateral != nil else { return M.noCollateralOutput(amount(collateralAmount)) }
                return nil
            case .external:
                guard M3Validation.isHex(externalTxid.trimmingCharacters(in: .whitespaces), count: 64),
                    UInt32(externalVout.trimmingCharacters(in: .whitespaces)) != nil
                else { return M.enterTxid }
                return nil
            }
        case .service:
            let entries = M3Validation.list(serviceText)
            if entries.contains(where: { !M3Validation.isService($0) }) { return M.enterService }
            return nil
        case .keys:
            if operatorKeyMode == .existing,
                !M3Validation.isHex(operatorPublicKey.trimmingCharacters(in: .whitespaces), count: 96)
            {
                return M.enterOperatorKey
            }
            let owner = ownerAddress.trimmingCharacters(in: .whitespaces)
            let voting = votingAddress.trimmingCharacters(in: .whitespaces)
            if let collateral = selectedCollateralAddress, owner == collateral || voting == collateral {
                return M.keysDifferFromCollateral
            }
            return nil
        case .payout:
            let payout = payoutAddress.trimmingCharacters(in: .whitespaces)
            guard !payout.isEmpty else { return M.enterPayout }
            let owner = ownerAddress.trimmingCharacters(in: .whitespaces)
            let voting = votingAddress.trimmingCharacters(in: .whitespaces)
            if !owner.isEmpty && payout == owner || !voting.isEmpty && payout == voting { return M.payoutDiffersFromKeys }
            if payout == selectedCollateralAddress { return M.payoutDiffersFromCollateral }
            guard M3Validation.rewardX100(operatorRewardText) != nil else { return M.rewardRange }
            return nil
        case .platform:
            guard M3Validation.isHex(platformNodeID.trimmingCharacters(in: .whitespaces), count: 40) else {
                return M.enterNodeID
            }
            let p2p = M3Validation.list(platformP2PText)
            let https = M3Validation.list(platformHTTPSText)
            if p2p.isEmpty != https.isEmpty { return M.enterPlatformBoth }
            if !p2p.isEmpty, M3Validation.list(serviceText).isEmpty { return M.enterService }
            if (p2p + https).contains(where: { !M3Validation.isService($0) }) { return M.enterService }
            return nil
        case .fee:
            return feeSource == nil ? M.noFeeSource : nil
        case .sign:
            return collateralSignature.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? M.signatureMissing : nil
        case .type, .review, .saveKey, .complete:
            return nil
        }
    }

    private var selectedCollateralAddress: String? {
        guard collateralMode == .existing, let selectedCollateral else { return nil }
        return collateralCandidates.first { $0.outpoint == selectedCollateral }?.address
    }

    /// Candidates of exactly the collateral amount; refused ones carry the
    /// reason (shown greyed with `refusal`).
    public func loadCollateralCandidates() async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            collateralCandidates = try await registration.collateralCandidates(wallet: wallet, type: type)
            if let selected = selectedCollateral,
                !collateralCandidates.contains(where: { $0.outpoint == selected && $0.refusal == nil })
            {
                selectedCollateral = nil
            }
        } catch {
            if error.isNotImplemented { unavailable = true } else { errorMessage = errorText(error) }
        }
    }

    public var usableCollateral: [CollateralCandidate] { collateralCandidates.filter { $0.refusal == nil } }

    private func loadFeeSources() async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            feeSources = try await registration.feeSourceCandidates(wallet: wallet)
            if feeSource == nil || !feeSources.contains(where: { $0.address == feeSource }) {
                feeSource = feeSources.first?.address
            }
        } catch {
            if error.isNotImplemented { unavailable = true } else { errorMessage = errorText(error) }
        }
    }

    private func request(wallet: WalletID) -> RegistrationRequest? {
        let collateral: CollateralChoice
        switch collateralMode {
        case .fundNew:
            collateral = .fundNew
        case .existing:
            guard let selectedCollateral else { return nil }
            collateral = .existingUTXO(selectedCollateral)
        case .external:
            guard let vout = UInt32(externalVout.trimmingCharacters(in: .whitespaces)) else { return nil }
            collateral = .external(OutPoint(txid: externalTxid.trimmingCharacters(in: .whitespaces).lowercased(), vout: vout))
        }
        let trimmed = { (text: String) -> String? in
            let value = text.trimmingCharacters(in: .whitespaces)
            return value.isEmpty ? nil : value
        }
        let platform = type == .evo
            ? PlatformFields(
                nodeIDHex: platformNodeID.trimmingCharacters(in: .whitespaces).lowercased(),
                p2pAddresses: M3Validation.list(platformP2PText), httpsAddresses: M3Validation.list(platformHTTPSText))
            : nil
        return RegistrationRequest(
            wallet: wallet, type: type, collateral: collateral, serviceAddresses: M3Validation.list(serviceText),
            ownerAddress: trimmed(ownerAddress), votingAddress: trimmed(votingAddress),
            operatorKey: operatorKeyMode == .generate
                ? .generate : .existing(publicKeyHex: operatorPublicKey.trimmingCharacters(in: .whitespaces).lowercased()),
            payoutAddress: payoutAddress.trimmingCharacters(in: .whitespaces),
            operatorRewardX100: M3Validation.rewardX100(operatorRewardText) ?? 0, platform: platform,
            feeSource: feeSource.map { .address($0) } ?? .automatic)
    }

    /// Builds and signs the registration (nothing is sent), then Review.
    private func prepare(passphrase: String?) async {
        guard let wallet = walletState.selectedWalletID, let request = request(wallet: wallet) else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            guard let grant = try await grants.authorize(.masternodeOperation, wallet: wallet, passphrase: passphrase)
            else {
                needsPassphrase = true
                return
            }
            needsPassphrase = false
            prepared = try await registration.prepare(request, grant: grant)
            page = .review
        } catch {
            needsPassphrase = error.code == .vaultWrongPassphrase
            if error.isNotImplemented { unavailable = true }
            errorMessage = errorText(error)
        }
    }

    public func cancelPassphrase() {
        needsPassphrase = false
    }

    private func showOperatorSecret() async {
        guard let prepared else { return }
        do {
            operatorSecret = try await registration.operatorSecret(prepared)
            page = .saveKey
        } catch {
            errorMessage = errorText(error)
        }
    }

    /// The last-4 gate: only a match moves on.
    private func confirmLast4() async {
        guard let prepared else { return }
        do {
            gateOpen = try await registration.confirmOperatorSecret(
                prepared, last4: last4.trimmingCharacters(in: .whitespaces))
        } catch {
            errorMessage = errorText(error)
            return
        }
        guard gateOpen else {
            errorMessage = L10n.Masternodes.last4Mismatch
            return
        }
        if collateralMode == .external {
            page = .sign
        } else {
            await submit()
        }
    }

    /// Copies the secret or the `masternodeblsprivkey=` line.
    public func copySecret(configLine: Bool = false) {
        guard let secret = operatorSecret else { return }
        let buffer = configLine ? secret.configLine : secret.secretHex
        clipboard.setString(buffer.withUnsafeBytes { String(decoding: $0, as: UTF8.self) })
    }

    public func copySignMessage() {
        if let collateralSignMessage { clipboard.setString(collateralSignMessage) }
    }

    private func submit() async {
        guard let prepared else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            let signature = collateralMode == .external
                ? collateralSignature.trimmingCharacters(in: .whitespacesAndNewlines) : nil
            proTxHash = try await registration.submit(prepared, collateralSignature: signature)
            operatorSecret = nil
            page = .complete
        } catch {
            errorMessage = errorText(error)
            if error.code == .masternodeOperatorSecretUnconfirmed { page = .saveKey }
        }
    }

    private func abandonPrepared() async {
        guard let prepared else { return }
        self.prepared = nil
        operatorSecret = nil
        gateOpen = false
        await registration.abandon(prepared)
    }
}
