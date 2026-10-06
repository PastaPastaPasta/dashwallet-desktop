// The M3 service protocols (docs/contracts/m3-swift.md §2) over
// `DemoM3World`. Calls whose engine work builds transactions the demo does
// not make (move mixed coins, the multi-party shared broadcast, standby
// dissolutions) answer `not_implemented` after the engine's argument checks,
// and the Platform calls answer `not_implemented` like the engine's
// `.platform` stubs (M4).
import Foundation
import WalletFeatures
import WalletRuntime

private func notInDemo(_ call: String) -> ServiceError {
    .demo(.notImplemented, "demo: \(call) builds no transactions")
}

// MARK: CoinJoin (§2.1, §2.4)

final class DemoCoinJoin: CoinJoinControlling, MixedCoinsMoving, NetworkStatisticsProviding {
    let m3: DemoM3World

    init(m3: DemoM3World) {
        self.m3 = m3
    }

    func limits() -> CoinJoinLimits { M3Defaults.coinJoinLimits }
    func settings() async throws(ServiceError) -> CoinJoinSettings { await m3.settings }
    func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) { try await m3.setSettings(settings) }
    func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus { try await m3.status(wallet) }
    func statusChanges() -> AsyncStream<WalletID> { m3.coinJoinChanges.stream() }
    func start(wallet: WalletID) async throws(ServiceError) { try await m3.start(wallet) }
    func stop(wallet: WalletID) async throws(ServiceError) { try await m3.stopMixing(wallet) }
    func salt(wallet: WalletID) async throws(ServiceError) -> String { try await m3.salt(wallet) }
    func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) { try await m3.setSalt(salt, wallet) }
    func generateSalt(wallet: WalletID) async throws(ServiceError) -> String { try await m3.generateSalt(wallet) }

    func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        let status = try await m3.status(wallet)
        return CoinJoinRecoveryReport(
            coinJoinAddressesScanned: 1_000, bip44AddressesScanned: 2_000, coinJoinBalance: status.balances.fullyMixed,
            newTransactions: 0)
    }

    func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError) -> MixedCoinsSweepPlan {
        try await m3.sweepPlan(wallet, destination: destination)
    }

    func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant) async throws(ServiceError)
        -> MixedCoinsSweepResult
    {
        _ = try await m3.sweepPlan(wallet, destination: destination)
        throw notInDemo("move_mixed_coins")
    }

    func statistics() async throws(ServiceError) -> NetworkStatistics { await m3.statistics() }
}

// MARK: Governance (§2.2)

final class DemoGovernance: GovernanceProviding, GovernanceVoting, ProposalCreating {
    let m3: DemoM3World

    init(m3: DemoM3World) {
        self.m3 = m3
    }

    func parameters() -> GovernanceParameters { M3Defaults.governanceParameters(m3.network) }
    func syncState() async throws(ServiceError) -> GovernanceSyncState { await m3.syncState }
    func setSyncEnabled(_ enabled: Bool) async throws(ServiceError) { await m3.setGovernanceSync(enabled) }
    func changes() -> AsyncStream<Void> { m3.governanceChanges.stream() }
    func proposals(_ query: ProposalQuery) async throws(ServiceError) -> [ProposalRow] { try await m3.proposals(query) }
    func detail(hash: String) async throws(ServiceError) -> ProposalDetail { try await m3.proposalDetail(hash) }
    func info() async throws(ServiceError) -> GovernanceInfo { await m3.governanceInfo() }
    func clock() async throws(ServiceError) -> GovernanceClock { await m3.governanceClock() }

    func votingMasternodes(proposal hash: String, wallet: WalletID?) async throws(ServiceError) -> [VotingMasternode] {
        try await m3.votingMasternodes(hash)
    }

    func cast(_ outcome: VoteOutcome, on hash: String, with proTxHashes: [String], grant: AuthGrant)
        async throws(ServiceError) -> [VoteResult]
    {
        guard let wallet = await m3.world.current.selected else { throw .demo(.walletNotFound) }
        return try await m3.cast(outcome, hash, proTxHashes, grant: grant, wallet: wallet)
    }

    func superblockDates(count: Int) async throws(ServiceError) -> [SuperblockDate] { await m3.superblockDates(count) }
    func validate(_ draft: ProposalDraft) async throws(ServiceError) -> [ProposalField] { await m3.validate(draft) }

    func json(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        if let field = await m3.validate(draft).first {
            throw .demo(.governanceInvalidProposal, parameters: ["field": Int64(ProposalField.allCases.firstIndex(of: field)!)])
        }
        return await m3.json(draft)
    }

    func payloadHex(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        _ = try await json(draft)
        return await m3.payloadHex(draft)
    }

    func create(wallet: WalletID, draft: ProposalDraft, grant: AuthGrant) async throws(ServiceError) -> PendingProposal {
        try await m3.createProposal(wallet, draft, grant: grant)
    }

    func pending(wallet: WalletID) async throws(ServiceError) -> [PendingProposal] {
        try await m3.pendingProposals(wallet)
    }

    func submit(wallet: WalletID, hash: String) async throws(ServiceError) -> String {
        try await m3.submitProposal(wallet, hash)
    }
}

// MARK: Masternodes (§2.3)

final class DemoMasternodes: MasternodeListProviding, MasternodeRegistering, MasternodeMaintaining,
    SharedMasternodeCoordinating, MasternodeKeychainProviding, TrackedMasternodeManaging, EvonodeServicing
{
    let m3: DemoM3World

    init(m3: DemoM3World) {
        self.m3 = m3
    }

    private func selectedWallet() async throws(ServiceError) -> WalletID {
        guard let wallet = await m3.world.current.selected else { throw .demo(.walletNotFound) }
        return wallet
    }

    // List
    func defaults() -> MasternodeNetworkDefaults { M3Defaults.masternodeDefaults(m3.network) }
    func state() async throws(ServiceError) -> MasternodeListState { await m3.listState }
    func changes() -> AsyncStream<Void> { m3.masternodeChanges.stream() }
    func list(_ query: MasternodeQuery) async throws(ServiceError) -> [MasternodeRow] { await m3.list(query) }
    func detail(proTxHash: String) async throws(ServiceError) -> MasternodeDetail { try await m3.masternodeDetail(proTxHash) }

    // Registration
    func collateralCandidates(wallet: WalletID, type: MasternodeType) async throws(ServiceError) -> [CollateralCandidate] {
        try await m3.collateralCandidates(wallet, type)
    }

    func feeSourceCandidates(wallet: WalletID) async throws(ServiceError) -> [FeeSourceCandidate] {
        try await m3.feeSources(wallet)
    }

    func prepare(_ request: RegistrationRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedRegistrationReference
    {
        try await m3.prepareRegistration(request, grant: grant)
    }

    func operatorSecret(_ registration: PreparedRegistrationReference) async throws(ServiceError) -> OperatorSecret {
        try await m3.operatorSecret(registration.id)
    }

    func confirmOperatorSecret(_ registration: PreparedRegistrationReference, last4: String) async throws(ServiceError)
        -> Bool
    {
        await m3.confirmSecret(registration.id, last4: last4)
    }

    func submit(_ registration: PreparedRegistrationReference, collateralSignature: String?) async throws(ServiceError)
        -> String
    {
        try await m3.submitRegistration(registration.id, signature: collateralSignature)
    }

    func abandon(_ registration: PreparedRegistrationReference) async {
        await MainActor.run { _ = m3.registrations.removeValue(forKey: registration.id) }
    }

    // Maintenance: prepare holds the transaction, broadcast applies it.
    func prepareUpdateService(_ request: UpdateServiceRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        if request.serviceAddresses.contains(where: { !$0.contains(":") }) { throw .demo(.masternodeInvalidService) }
        if let secret = request.operatorSecret, DemoSecret.text(secret).count != 64 {
            throw .demo(.masternodeOperatorSecretMismatch)
        }
        return try await m3.prepareProvider(
            .updateService, hash: request.proTxHash, wallet: request.feeWallet, grant: grant,
            effect: .service(request.serviceAddresses))
    }

    func prepareUpdateRegistrar(_ request: UpdateRegistrarRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        let detail = try await m3.masternodeDetail(request.proTxHash)
        if detail.row.shared != nil { throw .demo(.masternodeUnsupportedEntry) }
        if !detail.row.ownedRoles.contains(.owner) {
            throw .demo(.masternodeKeyNotInWallet, parameters: ["role": 0])
        }
        if let key = request.operatorPublicKey, !(key.count == 96 && key.allSatisfy(\.isHexDigit)) {
            throw .demo(.masternodeInvalidKey, parameters: ["role": 2])
        }
        return try await m3.prepareProvider(
            .updateRegistrar, hash: request.proTxHash, wallet: request.feeWallet, grant: grant,
            effect: .registrar(
                operatorKey: request.operatorPublicKey, voting: request.votingAddress, payout: request.payoutAddress),
            bans: request.operatorPublicKey != nil)
    }

    func prepareRevoke(_ request: RevokeRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        if let secret = request.operatorSecret, DemoSecret.text(secret).count != 64 {
            throw .demo(.masternodeOperatorSecretMismatch)
        }
        return try await m3.prepareProvider(
            .revoke, hash: request.proTxHash, wallet: request.feeWallet, grant: grant, effect: .revoke(request.reason))
    }

    func prepareShareRewardUpdate(
        proTxHash: String, shareIndex: Int, payoutAddress: String, feeWallet: WalletID, grant: AuthGrant
    ) async throws(ServiceError) -> PreparedProviderTransaction {
        let detail = try await m3.masternodeDetail(proTxHash)
        guard detail.shares.indices.contains(shareIndex), detail.shares[shareIndex].mine else {
            throw .demo(.masternodeKeyNotInWallet, parameters: ["role": 0])
        }
        guard DemoM3World.isAddress(payoutAddress, network: m3.network) else { throw .demo(.masternodeInvalidPayout) }
        return try await m3.prepareProvider(
            .updateShare, hash: proTxHash, wallet: feeWallet, grant: grant, effect: .shareReward)
    }

    func prepareDissolveNow(proTxHash: String, acceptPenalty: Bool, feeWallet: WalletID, grant: AuthGrant)
        async throws(ServiceError) -> PreparedProviderTransaction
    {
        let detail = try await m3.masternodeDetail(proTxHash)
        let tip = await m3.tip
        let early = detail.earlyPeriodEnd.map { tip < $0 } ?? false
        if early, !acceptPenalty { throw .demo(.invalidArgument, "accept_penalty") }
        return try await m3.prepareProvider(
            .dissolve, hash: proTxHash, wallet: feeWallet, grant: grant, effect: .dissolve,
            penalty: early ? detail.earlyExitPenalty : nil)
    }

    func broadcast(_ transaction: PreparedProviderTransaction) async throws(ServiceError) -> String {
        try await m3.broadcastProvider(transaction.id)
    }

    func abandon(_ transaction: PreparedProviderTransaction) async {
        await m3.abandonProvider(transaction.id)
    }

    private func checkTarget(_ hash: String) async throws(ServiceError) {
        _ = try await m3.masternodeDetail(hash)
    }

    func createStandbyDissolution(wallet: WalletID, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> StandbyDissolution
    {
        try await checkTarget(proTxHash)
        throw notInDemo("create_standby_dissolution")
    }

    func broadcastStandbyDissolution(_ transactionsHex: [String]) async throws(ServiceError) -> [String] {
        throw notInDemo("broadcast_standby_dissolution")
    }

    // Shared
    func create(wallet: WalletID, terms: SharedMasternodeTerms) async throws(ServiceError) -> SharedSessionInfo {
        try await m3.createShared(wallet, terms)
    }

    func importMessage(_ text: String, wallet: WalletID) async throws(ServiceError) -> SharedMessageKind {
        try await m3.importShared(text, wallet: wallet)
    }

    func sessions(wallet: WalletID) async throws(ServiceError) -> [SharedSessionInfo] {
        await MainActor.run { m3.sharedSessions.values.filter { $0.wallet == wallet }.sorted { $0.id < $1.id } }
    }

    func message(session: String) async throws(ServiceError) -> SharedEnvelope { try await m3.envelope(session) }

    func contribute(_ contribution: ShareContribution, session: String) async throws(ServiceError) -> SharedSessionInfo {
        try await m3.contribute(contribution, session: session)
    }

    func approve(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        try await m3.approve(session, grant: grant)
    }

    func sign(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        try await m3.sign(session, grant: grant)
    }

    func broadcast(session: String) async throws(ServiceError) -> String {
        _ = try await m3.envelope(session)
        throw notInDemo("shared_session_broadcast")
    }

    func abandon(session: String) async throws(ServiceError) { try await m3.abandonShared(session) }

    func startKeyRotation(wallet: WalletID, proTxHash: String, operatorKey: OperatorKeyChoice?, votingAddress: String?)
        async throws(ServiceError) -> SharedSessionInfo
    {
        let detail = try await m3.masternodeDetail(proTxHash)
        guard detail.row.shared != nil else { throw .demo(.masternodeUnsupportedEntry) }
        return await m3.newSession(wallet, role: .coordinator, stage: .lockedTerms, purpose: .rotateKeys, proTxHash: proTxHash)
    }

    func startDissolveTogether(wallet: WalletID, proTxHash: String) async throws(ServiceError) -> SharedSessionInfo {
        let detail = try await m3.masternodeDetail(proTxHash)
        guard detail.row.shared != nil else { throw .demo(.masternodeUnsupportedEntry) }
        return await m3.newSession(
            wallet, role: .coordinator, stage: .lockedTerms, purpose: .dissolveTogether, proTxHash: proTxHash)
    }

    // Keychain
    func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        try await m3.keys(wallet, role: role, range: range)
    }

    func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        try await m3.revealKey(wallet, role: role, index: index, grant: grant)
    }

    // Tracked
    func locate(_ query: String) async throws(ServiceError) -> [MasternodeRow] { await m3.locate(query) }
    func tracked() async throws(ServiceError) -> [TrackedMasternode] { await m3.trackedNodes() }

    func track(proTxHash: String, label: String?) async throws(ServiceError) -> TrackedMasternode {
        try await m3.track(proTxHash, label: label)
    }

    func untrack(proTxHash: String) async throws(ServiceError) -> Bool { await m3.untrack(proTxHash) }

    func setLabel(_ label: String?, proTxHash: String) async throws(ServiceError) {
        try await m3.setLabel(label, proTxHash)
    }

    func attach(_ key: any SecretBuffer, role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError)
    {
        let text = DemoSecret.text(key)
        let wallet = try await selectedWallet()
        try await m3.attach(text, role: role, hash: proTxHash, grant: grant, wallet: wallet)
    }

    func detach(role: MasternodeKeyRole, proTxHash: String) async throws(ServiceError) {
        await m3.detach(role, proTxHash)
    }

    func reveal(role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        let wallet = try await selectedWallet()
        return try await m3.revealAttached(role, proTxHash, grant: grant, wallet: wallet)
    }

    // Evonode: Platform work (M4), as the engine's `.platform` stubs.
    func status(proTxHash: String) async throws(ServiceError) -> EvonodePlatformStatus {
        _ = try await m3.masternodeDetail(proTxHash)
        throw .demo(.notImplemented, "NetworkSession.evonode_status.platform")
    }

    func withdraw(proTxHash: String, credits: UInt64, destination: CreditWithdrawalDestination, grant: AuthGrant)
        async throws(ServiceError) -> String
    {
        _ = try await m3.masternodeDetail(proTxHash)
        throw .demo(.notImplemented, "NetworkSession.withdraw_evonode_credits.platform")
    }
}
