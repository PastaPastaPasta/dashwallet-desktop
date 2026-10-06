// v24 shared masternodes (QT-126, QT-127): the coordinator's terms and the
// participants' contributions, exchanged as JSON envelopes over the
// clipboard or files (there is no network transport). Shows the session code
// and the `XXXX-XXXX` fingerprint to compare out of band, routes pasted text
// (envelopes to their session, standby-dissolution text to broadcast),
// protects closing a session that holds reserved coins, and starts the
// multi-party key rotation and "dissolve together".
import Foundation
import Observation
import PlatformServices
import WalletRuntime

/// One share row of the coordinator's terms form.
public struct ShareDraft: Sendable, Hashable, Identifiable {
    public let id: UUID
    public var amountText: String
    public var label: String
    public var mine: Bool

    public init(amountText: String, label: String = "", mine: Bool = false) {
        id = UUID()
        self.amountText = amountText
        self.label = label
        self.mine = mine
    }
}

/// What the dialog shows.
public enum SharedMasternodeMode: Sendable, Hashable {
    case sessions
    case creating
    case session(id: String)
    /// Pasted standby-dissolution text, waiting for "Broadcast?".
    case standby(proTxHash: String?, transactionsHex: [String])
    case unavailable
}

@MainActor
@Observable
public final class SharedMasternodeViewModel {
    // Coordinator terms (QT-126)
    public var shares: [ShareDraft] = [ShareDraft(amountText: "500", mine: true), ShareDraft(amountText: "500")]
    public var earlyPeriodText = "0"
    public var penaltyText = "0"
    public var serviceText = ""
    public var operatorKeyMode: OperatorKeyMode = .generate
    public var operatorPublicKey = ""
    public var operatorRewardText = "0"

    // Participant contribution
    public var contributionShare = 0
    public var contributionInputs: Set<OutPoint> = []
    public var contributionOwner = ""
    public var contributionPayout = ""
    public var contributionRefund = ""

    public private(set) var mode: SharedMasternodeMode = .sessions
    public private(set) var sessions: [SharedSessionInfo] = []
    public private(set) var envelope: SharedEnvelope?
    public private(set) var coins: [Utxo] = []
    public private(set) var needsPassphrase = false
    public private(set) var isWorking = false
    /// Close protection: the session holds reserved coins.
    public private(set) var closeProtection = false
    public private(set) var errorMessage: String?
    public private(set) var message: String?

    public let defaults: MasternodeNetworkDefaults

    public var session: SharedSessionInfo? {
        guard case .session(let id) = mode else { return nil }
        return sessions.first { $0.id == id }
    }

    public var sessionHeader: String? {
        guard let session else { return nil }
        return "\(L10n.Masternodes.sessionCode(session.sessionCode)) · \(L10n.Masternodes.stage(session.stage))"
    }

    public var fingerprintText: String? {
        (envelope?.fingerprint ?? session?.fingerprint).map(L10n.Masternodes.fingerprint)
    }

    /// Which actions the session's stage and the wallet's role allow.
    public var canContribute: Bool { session.map { $0.role == .participant && $0.stage == .details } ?? false }
    public var canApprove: Bool { session.map { $0.stage == .lockedTerms || $0.stage == .approvals } ?? false }
    public var canSign: Bool { session.map { $0.stage == .signingRequest } ?? false }
    public var canBroadcast: Bool {
        session.map { $0.role == .coordinator && $0.stage == .signedContributions } ?? false
    }

    /// The shares rule (2–8, ≥ 100 DASH each, exactly 1000 DASH).
    public var termsError: String? {
        typealias M = L10n.Masternodes
        guard defaults.shares.contains(shares.count) else { return M.shareCountWrong }
        let amounts = shares.map { parse($0.amountText) }
        guard amounts.allSatisfy({ $0 != nil }) else { return M.sharesTotalWrong }
        let values = amounts.compactMap { $0 }
        if values.contains(where: { $0 < defaults.minimumShareAmount }) { return M.shareTooSmall }
        guard values.reduce(0, { $0 + $1.duffs }) == defaults.masternodeCollateral.duffs else {
            return M.sharesTotalWrong
        }
        guard let early = UInt32(earlyPeriodText), early <= defaults.maxEarlyPeriodBlocks else {
            return M.earlyPeriodTooLong
        }
        guard let penalty = parse(penaltyText), let smallest = values.min(), penalty < smallest else {
            return M.penaltyTooLarge
        }
        if M3Validation.list(serviceText).contains(where: { !M3Validation.isService($0) }) { return M.enterService }
        if operatorKeyMode == .existing,
            !M3Validation.isHex(operatorPublicKey.trimmingCharacters(in: .whitespaces), count: 96)
        {
            return M.enterOperatorKey
        }
        guard M3Validation.rewardX100(operatorRewardText) != nil else { return M.rewardRange }
        return nil
    }

    private let shared: any SharedMasternodeCoordinating
    private let maintenance: any MasternodeMaintaining
    private let coinControl: any CoinControlProviding
    private let walletState: any WalletStateProviding
    private let grants: GrantRequester
    private let clipboard: any ClipboardProviding
    private let amounts: any AmountFormatting
    private let settings: any SettingsProviding

    public init(
        shared: any SharedMasternodeCoordinating, maintenance: any MasternodeMaintaining,
        defaults: MasternodeNetworkDefaults, coinControl: any CoinControlProviding,
        walletState: any WalletStateProviding, auth: any AuthenticationGating, vault: any VaultProviding,
        clipboard: any ClipboardProviding, amounts: any AmountFormatting, settings: any SettingsProviding
    ) {
        self.shared = shared
        self.maintenance = maintenance
        self.defaults = defaults
        self.coinControl = coinControl
        self.walletState = walletState
        grants = GrantRequester(auth: auth, vault: vault)
        self.clipboard = clipboard
        self.amounts = amounts
        self.settings = settings
    }

    public convenience init(env: AppEnvironment, m2: M2Services, m3: M3Services) {
        self.init(
            shared: m3.shared, maintenance: m3.maintenance, defaults: m3.masternodes.defaults(),
            coinControl: env.coinControl, walletState: env.walletState, auth: env.auth, vault: env.vault,
            clipboard: m2.clipboard, amounts: env.amounts, settings: env.settings)
    }

    private func parse(_ text: String) -> Amount? {
        try? amounts.parse(text.trimmingCharacters(in: .whitespaces), unit: .dash)
    }

    private func errorText(_ error: ServiceError) -> String {
        ErrorText.m3(error, amount: AmountText(amounts: amounts, settings: settings).callAsFunction)
    }

    // MARK: Sessions

    /// Open sessions of the wallet, resumable after a restart.
    public func load() async {
        guard let wallet = walletState.selectedWalletID else { return }
        do {
            sessions = try await shared.sessions(wallet: wallet)
        } catch {
            if error.isNotImplemented { mode = .unavailable } else { errorMessage = errorText(error) }
        }
    }

    public func open(_ id: String) async {
        guard sessions.contains(where: { $0.id == id }) else { return }
        mode = .session(id: id)
        await refreshEnvelope()
    }

    public func startCreating() {
        errorMessage = nil
        mode = .creating
    }

    public func addShare() {
        guard shares.count < defaults.shares.upperBound else { return }
        shares.append(ShareDraft(amountText: "100"))
    }

    public func removeShare(_ id: UUID) {
        guard shares.count > defaults.shares.lowerBound else { return }
        shares.removeAll { $0.id == id }
    }

    /// The coordinator starts a session with the terms.
    public func create() async {
        errorMessage = nil
        if let termsError {
            errorMessage = termsError
            return
        }
        guard let wallet = walletState.selectedWalletID else { return }
        let terms = SharedMasternodeTerms(
            shares: shares.map { SharedShareTerms(amount: parse($0.amountText) ?? .zero, label: $0.label.isEmpty ? nil : $0.label, mine: $0.mine) },
            earlyPeriodBlocks: UInt32(earlyPeriodText) ?? 0, earlyExitPenalty: parse(penaltyText) ?? .zero,
            serviceAddresses: M3Validation.list(serviceText),
            operatorKey: operatorKeyMode == .generate
                ? .generate : .existing(publicKeyHex: operatorPublicKey.trimmingCharacters(in: .whitespaces)),
            operatorRewardX100: M3Validation.rewardX100(operatorRewardText) ?? 0)
        await run { () async throws(ServiceError) in
            let info = try await self.shared.create(wallet: wallet, terms: terms)
            await self.adopt(info)
        }
    }

    // MARK: Messages

    /// Paste routing (QT-127): an envelope goes to its session, a standby
    /// dissolution waits for "Broadcast?". Over 2 MiB is refused.
    public func importText(_ text: String) async {
        guard let wallet = walletState.selectedWalletID else { return }
        await run { () async throws(ServiceError) in
            switch try await self.shared.importMessage(text, wallet: wallet) {
            case .envelope(let info):
                await self.adopt(info)
                self.message = L10n.Masternodes.pasteRoutedToSession
            case .standbyDissolution(let hash, let transactions):
                self.mode = .standby(proTxHash: hash, transactionsHex: transactions)
                self.message = L10n.Masternodes.pasteIsStandby
            }
        }
    }

    public func pasteFromClipboard() async {
        guard let text = clipboard.string(), !text.isEmpty else { return }
        await importText(text)
    }

    /// Copies the session's outgoing envelope for the other participants.
    public func copyMessage() {
        guard let envelope else { return }
        clipboard.setString(envelope.json)
        message = L10n.Masternodes.messageCopied
    }

    /// "Save message…": the file name the save panel suggests and its text.
    public var messageFile: (name: String, text: String)? {
        envelope.map { ($0.suggestedFileName, $0.json) }
    }

    // MARK: Participant and signing

    /// The wallet's spendable coins to reserve for a share.
    public func loadCoins() async {
        guard let wallet = walletState.selectedWalletID else { return }
        coins = (try? await coinControl.utxos(wallet: wallet, filter: UtxoFilter(includeLocked: false)))?
            .filter(\.spendable) ?? []
    }

    /// Reserves the chosen coins (persistent locks) and fills the share.
    public func contribute() async {
        guard let session, canContribute else { return }
        func trimmed(_ text: String) -> String? {
            let value = text.trimmingCharacters(in: .whitespaces)
            return value.isEmpty ? nil : value
        }
        let contribution = ShareContribution(
            shareIndex: contributionShare, inputs: Array(contributionInputs).sorted { ($0.txid, $0.vout) < ($1.txid, $1.vout) },
            ownerAddress: trimmed(contributionOwner), payoutAddress: trimmed(contributionPayout),
            refundAddress: trimmed(contributionRefund))
        await run { () async throws(ServiceError) in
            await self.adopt(try await self.shared.contribute(contribution, session: session.id))
        }
    }

    public func approve(passphrase: String? = nil) async {
        guard let session, canApprove else { return }
        await authorized(passphrase) { grant async throws(ServiceError) in
            await self.adopt(try await self.shared.approve(session: session.id, grant: grant))
        }
    }

    public func sign(passphrase: String? = nil) async {
        guard let session, canSign else { return }
        await authorized(passphrase) { grant async throws(ServiceError) in
            await self.adopt(try await self.shared.sign(session: session.id, grant: grant))
        }
    }

    /// Coordinator: combines and broadcasts; the txid is the proTxHash.
    public func broadcast() async {
        guard let session, canBroadcast else { return }
        await run { () async throws(ServiceError) in
            let txid = try await self.shared.broadcast(session: session.id)
            self.message = L10n.Masternodes.sent(txid)
            await self.load()
        }
    }

    public func cancelPassphrase() {
        needsPassphrase = false
    }

    // MARK: Maintenance sessions (QT-127)

    /// Rotate Keys…: a new operator key and/or voting address that every
    /// owner approves.
    public func startKeyRotation(proTxHash: String, operatorKey: OperatorKeyChoice?, votingAddress: String?) async {
        guard let wallet = walletState.selectedWalletID else { return }
        await run { () async throws(ServiceError) in
            await self.adopt(try await self.shared.startKeyRotation(
                wallet: wallet, proTxHash: proTxHash, operatorKey: operatorKey, votingAddress: votingAddress))
        }
    }

    /// Dissolve ▸ Together: unanimous, returns every principal.
    public func startDissolveTogether(proTxHash: String) async {
        guard let wallet = walletState.selectedWalletID else { return }
        await run { () async throws(ServiceError) in
            await self.adopt(try await self.shared.startDissolveTogether(wallet: wallet, proTxHash: proTxHash))
        }
    }

    /// "Broadcast this standby dissolution?" Yes.
    public func broadcastStandby() async {
        guard case .standby(_, let transactions) = mode else { return }
        await run { () async throws(ServiceError) in
            let txids = try await self.maintenance.broadcastStandbyDissolution(transactions)
            self.message = L10n.Masternodes.standbySent(txids)
            self.mode = .sessions
        }
    }

    // MARK: Close protection

    /// Closing the dialog: a session holding reserved coins asks first.
    public func requestClose() -> Bool {
        guard let session, !session.reservedInputs.isEmpty,
            session.stage != .completed, session.stage != .abandoned
        else { return true }
        closeProtection = true
        return false
    }

    /// "Save and close": the session stays for later.
    public func keepAndClose() {
        closeProtection = false
        mode = .sessions
    }

    /// "Release coins": leaves the session and unlocks its coins.
    public func releaseAndClose() async {
        closeProtection = false
        guard let session else { return }
        await run { () async throws(ServiceError) in
            try await self.shared.abandon(session: session.id)
            self.mode = .sessions
            await self.load()
        }
    }

    // MARK: Private

    private func adopt(_ info: SharedSessionInfo) async {
        if let index = sessions.firstIndex(where: { $0.id == info.id }) {
            sessions[index] = info
        } else {
            sessions.append(info)
        }
        mode = .session(id: info.id)
        if info.reservedCoinSpent { errorMessage = L10n.Masternodes.reservedCoinSpent }
        await refreshEnvelope()
    }

    private func refreshEnvelope() async {
        guard let session else {
            envelope = nil
            return
        }
        envelope = try? await shared.message(session: session.id)
    }

    private func run(_ body: () async throws(ServiceError) -> Void) async {
        errorMessage = nil
        isWorking = true
        defer { isWorking = false }
        do {
            try await body()
        } catch {
            // One step the engine does not offer yet: the session stays open.
            errorMessage = error.isNotImplemented ? L10n.Masternodes.notAvailableYet : errorText(error)
        }
    }

    private func authorized(_ passphrase: String?, _ body: (AuthGrant) async throws(ServiceError) -> Void) async {
        guard let wallet = walletState.selectedWalletID else { return }
        await run { () async throws(ServiceError) in
            let done: Void? = try await self.grants.with(.masternodeOperation, wallet: wallet, passphrase: passphrase, body)
            self.needsPassphrase = done == nil
        }
    }
}
