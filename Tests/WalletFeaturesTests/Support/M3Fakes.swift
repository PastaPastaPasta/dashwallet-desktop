// In-memory fakes of the M3 WalletRuntime contracts (docs/contracts/
// m3-swift.md §2). Like the M1/M2 fakes they record calls and answer from
// configured state; an unconfigured call throws not_implemented, as the
// engine stubs do. The checks the engine already makes (CoinJoin ranges,
// the salt, the 2 MiB envelope limit, ≤ 100 keys, the last-4 gate before
// submit, the 1-hour vote rule) are made here too, with the engine's codes.
import Foundation
import WalletFeatures
import WalletRuntime

// MARK: CoinJoin (§2.1, §2.4)

final class FakeCoinJoin: CoinJoinControlling, MixedCoinsMoving, NetworkStatisticsProviding, @unchecked Sendable {
    let settingsValue = Locked<CoinJoinSettings?>(nil)
    let statuses = Locked<[WalletID: CoinJoinStatus]>([:])
    let starts = Locked<[WalletID]>([])
    let stops = Locked<[WalletID]>([])
    let settingsWrites = Locked<[CoinJoinSettings]>([])
    let report = Locked<CoinJoinRecoveryReport?>(nil)
    let sweepPlan = Locked<MixedCoinsSweepPlan?>(nil)
    let moveResult = Locked<MixedCoinsSweepResult?>(nil)
    let moveGrants = Locked<[AuthGrant]>([])
    let networkStatistics = Locked<NetworkStatistics?>(nil)
    let errors = FakeErrors()
    let changes = Broadcast<WalletID>()
    /// `start` refuses with `coinjoin.vault_locked` while this says the vault is locked.
    let vaultLocked = Locked<@Sendable () -> Bool>({ false })

    func limits() -> CoinJoinLimits { M3Defaults.coinJoinLimits }

    func settings() async throws(ServiceError) -> CoinJoinSettings {
        try errors.check("settings")
        guard let value = settingsValue.current else { throw notConfigured("settings") }
        return value
    }

    func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) {
        if let field = M3Defaults.invalidCoinJoinField(settings) {
            throw ServiceError(code: .invalidArgument, detail: field)
        }
        guard settingsValue.current != nil else { throw notConfigured("setSettings") }
        settingsWrites.withLock { $0.append(settings) }
        settingsValue.withLock { $0 = settings }
    }

    func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus {
        try errors.check("status")
        guard let status = statuses.current[wallet] else { throw notConfigured("status") }
        return status
    }

    func statusChanges() -> AsyncStream<WalletID> { changes.stream() }

    func start(wallet: WalletID) async throws(ServiceError) {
        try errors.check("start")
        if vaultLocked.current() { throw ServiceError(code: .coinjoinVaultLocked) }
        guard let status = statuses.current[wallet] else { throw notConfigured("start") }
        starts.withLock { $0.append(wallet) }
        statuses.withLock { $0[wallet] = status.with(state: .mixing) }
        changes.send(wallet)
    }

    func stop(wallet: WalletID) async throws(ServiceError) {
        guard let status = statuses.current[wallet] else { throw notConfigured("stop") }
        stops.withLock { $0.append(wallet) }
        statuses.withLock { $0[wallet] = status.with(state: .idle, stopReason: .userRequested) }
        changes.send(wallet)
    }

    func salt(wallet: WalletID) async throws(ServiceError) -> String { throw notConfigured("salt") }

    func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) {
        guard M3Defaults.isValidSalt(salt) else { throw ServiceError(code: .invalidArgument, detail: "salt") }
        throw notConfigured("setSalt")
    }

    func generateSalt(wallet: WalletID) async throws(ServiceError) -> String { throw notConfigured("generateSalt") }

    func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        guard let report = report.current else { throw notConfigured("recoveryScan") }
        return report
    }

    func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError) -> MixedCoinsSweepPlan {
        if destination == .shielded { throw ServiceError(code: .notImplemented, detail: "move_mixed_coins.shielded") }
        guard let plan = sweepPlan.current else { throw notConfigured("plan") }
        return plan
    }

    func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant) async throws(ServiceError)
        -> MixedCoinsSweepResult
    {
        guard let plan = sweepPlan.current, let result = moveResult.current else { throw notConfigured("move") }
        guard case .spend(let max) = grant.purpose, max >= plan.total else { throw ServiceError(code: .coinjoinGrantInvalid) }
        moveGrants.withLock { $0.append(grant) }
        return result
    }

    func statistics() async throws(ServiceError) -> NetworkStatistics {
        guard let value = networkStatistics.current else { throw notConfigured("statistics") }
        return value
    }
}

extension CoinJoinStatus {
    func with(state: CoinJoinState, stopReason: CoinJoinStopReason? = nil) -> CoinJoinStatus {
        CoinJoinStatus(
            wallet: wallet, state: state, stopReason: stopReason, unavailable: unavailable, balances: balances,
            progress: progress, amountAndRounds: amountAndRounds, submittedDenominations: submittedDenominations,
            sessions: sessions, status: state == .mixing ? .mixingInProgress : .idle, queueSize: queueSize,
            keysLeft: keysLeft)
    }
}

func coinJoinStatus(
    _ wallet: WalletID = walletA, state: CoinJoinState = .idle, unavailable: CoinJoinUnavailable? = nil,
    anonymizable: Int64 = 5_00000000, fullyMixed: Int64 = 1_50000000, denominated: Int64 = 3_00000000,
    amount: Int64 = 1000_00000000, insufficientInputs: Bool = false, keysLeft: Int? = nil,
    status: CoinJoinStatusCode = .idle, stopReason: CoinJoinStopReason? = nil
) -> CoinJoinStatus {
    CoinJoinStatus(
        wallet: wallet, state: state, stopReason: stopReason, unavailable: unavailable,
        balances: CoinJoinBalances(
            anonymizable: Amount(duffs: anonymizable), denominated: Amount(duffs: denominated),
            normalizedAnonymized: Amount(duffs: fullyMixed), fullyMixed: Amount(duffs: fullyMixed)),
        progress: CoinJoinProgress(overall: 42.5, denominated: 60, partiallyMixed: 30, mixed: 25, averageRounds: 1.75),
        amountAndRounds: CoinJoinAmountAndRounds(
            amount: Amount(duffs: amount), rounds: 4, insufficientInputs: insufficientInputs),
        submittedDenominations: [], sessions: [], status: status, queueSize: 0, keysLeft: keysLeft)
}

// MARK: Governance (§2.2)

final class FakeGovernance: GovernanceProviding, GovernanceVoting, ProposalCreating, @unchecked Sendable {
    let network: DashNetwork
    let sync = Locked<GovernanceSyncState?>(nil)
    let syncSwitches = Locked<[Bool]>([])
    let rows = Locked<[ProposalRow]?>(nil)
    let queries = Locked<[ProposalQuery]>([])
    let details = Locked<[String: ProposalDetail]>([:])
    let infoValue = Locked<GovernanceInfo?>(nil)
    let clockValue = Locked<GovernanceClock?>(nil)
    let voters = Locked<[VotingMasternode]?>(nil)
    let casts = Locked<[(VoteOutcome, String, [String], AuthGrant)]>([])
    let superblocks = Locked<[SuperblockDate]?>(nil)
    let created = Locked<[(ProposalDraft, AuthGrant)]>([])
    let pendingList = Locked<[PendingProposal]?>(nil)
    let submits = Locked<[String]>([])
    let errors = FakeErrors()
    let changeBroadcast = Broadcast<Void>()
    let now: @Sendable () -> Date

    init(network: DashNetwork = .testnet, now: @escaping @Sendable () -> Date = { Date() }) {
        self.network = network
        self.now = now
    }

    func parameters() -> GovernanceParameters { M3Defaults.governanceParameters(network) }

    func syncState() async throws(ServiceError) -> GovernanceSyncState {
        try errors.check("syncState")
        guard let state = sync.current else { throw notConfigured("syncState") }
        return state
    }

    func setSyncEnabled(_ enabled: Bool) async throws(ServiceError) {
        try errors.check("setSyncEnabled")
        syncSwitches.withLock { $0.append(enabled) }
    }

    func changes() -> AsyncStream<Void> { changeBroadcast.stream() }

    func proposals(_ query: ProposalQuery) async throws(ServiceError) -> [ProposalRow] {
        try errors.check("proposals")
        guard let rows = rows.current else { throw notConfigured("proposals") }
        queries.withLock { $0.append(query) }
        guard let filter = query.titleFilter?.lowercased() else { return rows }
        return rows.filter { $0.name.lowercased().contains(filter) }
    }

    func detail(hash: String) async throws(ServiceError) -> ProposalDetail {
        guard let detail = details.current[hash] else { throw ServiceError(code: .governanceProposalNotFound) }
        return detail
    }

    func info() async throws(ServiceError) -> GovernanceInfo {
        guard let info = infoValue.current else { throw notConfigured("info") }
        return info
    }

    func clock() async throws(ServiceError) -> GovernanceClock {
        guard let clock = clockValue.current else { throw notConfigured("clock") }
        return clock
    }

    func votingMasternodes(proposal hash: String, wallet: WalletID?) async throws(ServiceError) -> [VotingMasternode] {
        guard let voters = voters.current else { throw notConfigured("votingMasternodes") }
        return voters
    }

    /// The engine's 1-hour rule: a masternode whose `nextVoteAt` is ahead
    /// gets `governance.vote_too_often` without being sent.
    func cast(_ outcome: VoteOutcome, on hash: String, with proTxHashes: [String], grant: AuthGrant)
        async throws(ServiceError) -> [VoteResult]
    {
        guard grant.purpose == .governance else { throw ServiceError(code: .governanceGrantInvalid) }
        guard let voters = voters.current else { throw notConfigured("cast") }
        casts.withLock { $0.append((outcome, hash, proTxHashes, grant)) }
        let time = now()
        return proTxHashes.map { hash in
            let voter = voters.first { $0.proTxHash == hash }
            if let next = voter?.nextVoteAt, next > time {
                return VoteResult(proTxHash: hash, errorCode: .governanceVoteTooOften)
            }
            return VoteResult(proTxHash: hash, errorCode: voter == nil ? .masternodeNotFound : nil)
        }
    }

    func superblockDates(count: Int) async throws(ServiceError) -> [SuperblockDate] {
        guard let dates = superblocks.current else { throw notConfigured("superblockDates") }
        return Array(dates.prefix(count))
    }

    /// The engine's field rules (m3-engine.md §2.2).
    func validate(_ draft: ProposalDraft) async throws(ServiceError) -> [ProposalField] {
        var failing: [ProposalField] = []
        let name = draft.name
        if name.isEmpty || name.count > 40
            || !name.unicodeScalars.allSatisfy({ "abcdefghijklmnopqrstuvwxyz0123456789-_".unicodeScalars.contains($0) })
        {
            failing.append(.name)
        }
        if draft.url.count < 4 || draft.url.contains(" ") { failing.append(.url) }
        if !(draft.paymentAddress.hasPrefix("y") || draft.paymentAddress.hasPrefix("8")) || draft.paymentAddress.count != 34 {
            failing.append(.paymentAddress)
        }
        if draft.paymentAmount <= .zero { failing.append(.paymentAmount) }
        if !(1...12).contains(draft.paymentCount) { failing.append(.paymentCount) }
        if !(superblocks.current ?? []).contains(where: { $0.height == draft.firstSuperblockHeight }) {
            failing.append(.firstPayment)
        }
        return failing
    }

    func json(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        "{\"name\":\"\(draft.name)\",\"payment_address\":\"\(draft.paymentAddress)\"}"
    }

    func payloadHex(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        Array(try await json(draft).utf8).map { String(format: "%02x", $0) }.joined()
    }

    func create(wallet: WalletID, draft: ProposalDraft, grant: AuthGrant) async throws(ServiceError) -> PendingProposal {
        try errors.check("create")
        guard case .spend(let max) = grant.purpose, max.duffs >= 100_000_000 else {
            throw ServiceError(code: .governanceGrantInvalid)
        }
        created.withLock { $0.append((draft, grant)) }
        let proposal = PendingProposal(
            hash: txid(77), name: draft.name, url: draft.url, paymentAmount: draft.paymentAmount,
            paymentCount: draft.paymentCount, collateralTxid: txid(78), collateralStatus: .pending, confirmations: 0,
            createdAt: now(), end: now().addingTimeInterval(86_400 * 30))
        pendingList.withLock { $0 = ($0 ?? []) + [proposal] }
        return proposal
    }

    func pending(wallet: WalletID) async throws(ServiceError) -> [PendingProposal] {
        guard let list = pendingList.current else { throw notConfigured("pending") }
        return list
    }

    func submit(wallet: WalletID, hash: String) async throws(ServiceError) -> String {
        guard let proposal = pendingList.current?.first(where: { $0.hash == hash }) else {
            throw ServiceError(code: .governanceProposalNotFound)
        }
        guard proposal.confirmations >= 1 else {
            throw ServiceError(
                code: .governanceCollateralUnconfirmed, parameters: ["confirmations": Int64(proposal.confirmations)])
        }
        submits.withLock { $0.append(hash) }
        pendingList.withLock { $0?.removeAll { $0.hash == hash } }
        return hash
    }
}

func proposalRow(
    _ n: Int, name: String = "proposal", status: ProposalStatus = .voting, yes: Int = 10, no: Int = 2, abstain: Int = 1,
    margin: Int = -3, myVotes: MyVotes? = nil, url: String = "https://dashcentral.org/p/1"
) -> ProposalRow {
    ProposalRow(
        hash: txid(n), name: name, url: url, paymentAddress: testnetAddress1, paymentAmount: Amount(duffs: 50_00000000),
        start: Date(timeIntervalSince1970: 1_760_000_000), end: Date(timeIntervalSince1970: 1_765_000_000),
        status: status, collateralConfirmations: 10, yes: yes, no: no, abstain: abstain, margin: margin,
        myVotes: myVotes)
}

// MARK: Masternodes (§2.3)

final class FakeMasternodes: MasternodeListProviding, MasternodeRegistering, MasternodeMaintaining,
    SharedMasternodeCoordinating, MasternodeKeychainProviding, TrackedMasternodeManaging, EvonodeServicing,
    @unchecked Sendable
{
    let network: DashNetwork
    let listState = Locked<MasternodeListState?>(nil)
    let rows = Locked<[MasternodeRow]?>(nil)
    let queries = Locked<[MasternodeQuery]>([])
    let details = Locked<[String: MasternodeDetail]>([:])
    let changeBroadcast = Broadcast<Void>()
    let candidates = Locked<[CollateralCandidate]?>(nil)
    let feeSources = Locked<[FeeSourceCandidate]?>(nil)
    let registrations = Locked<[RegistrationRequest]>([])
    let secret = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd1f"
    let gate = Locked<Set<UUID>>([])
    let abandoned = Locked<[UUID]>([])
    let submitted = Locked<[(UUID, String?)]>([])
    let providerRequests = Locked<[String]>([])
    let serviceRequests = Locked<[UpdateServiceRequest]>([])
    let registrarRequests = Locked<[UpdateRegistrarRequest]>([])
    let broadcasts = Locked<[UUID]>([])
    let sharedSessions = Locked<[SharedSessionInfo]>([])
    let abandonedSessions = Locked<[String]>([])
    let keyInfos = Locked<[MasternodeKeyInfo]?>(nil)
    let trackedList = Locked<[TrackedMasternode]?>(nil)
    let attached = Locked<[(String, MasternodeKeyRole, String)]>([])
    let errors = FakeErrors()

    init(network: DashNetwork = .testnet) {
        self.network = network
    }

    // List
    func defaults() -> MasternodeNetworkDefaults { M3Defaults.masternodeDefaults(network) }

    func state() async throws(ServiceError) -> MasternodeListState {
        guard let state = listState.current else { throw notConfigured("state") }
        return state
    }

    func changes() -> AsyncStream<Void> { changeBroadcast.stream() }

    /// dash-qt's filters: type, literal case-insensitive text, owned, banned.
    func list(_ query: MasternodeQuery) async throws(ServiceError) -> [MasternodeRow] {
        guard let rows = rows.current else { throw notConfigured("list") }
        queries.withLock { $0.append(query) }
        return rows.filter { row in
            switch query.typeFilter {
            case .all: break
            case .regular: if row.type != .regular || row.shared != nil { return false }
            case .evo: if row.type != .evo { return false }
            case .shared: if row.shared == nil { return false }
            }
            if query.ownedOnly, row.ownedRoles.isEmpty { return false }
            if query.hideBanned, case .banned = row.status { return false }
            if let text = query.text?.lowercased(), !text.isEmpty {
                let fields = [row.proTxHash, row.service ?? "", row.votingAddress, row.ownerAddress ?? "",
                              row.collateralAddress ?? ""] + row.payoutAddresses
                return fields.contains { $0.lowercased().contains(text) }
            }
            return true
        }
    }

    func detail(proTxHash: String) async throws(ServiceError) -> MasternodeDetail {
        guard let detail = details.current[proTxHash] else { throw ServiceError(code: .masternodeNotFound) }
        return detail
    }

    // Registration
    func collateralCandidates(wallet: WalletID, type: MasternodeType) async throws(ServiceError) -> [CollateralCandidate] {
        guard let candidates = candidates.current else { throw notConfigured("collateralCandidates") }
        return candidates
    }

    func feeSourceCandidates(wallet: WalletID) async throws(ServiceError) -> [FeeSourceCandidate] {
        guard let sources = feeSources.current else { throw notConfigured("feeSourceCandidates") }
        return sources
    }

    func prepare(_ request: RegistrationRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedRegistrationReference
    {
        try errors.check("prepare")
        guard grant.purpose == .masternodeOperation else { throw ServiceError(code: .masternodeGrantInvalid) }
        registrations.withLock { $0.append(request) }
        let external: OutPoint?
        if case .external(let outpoint) = request.collateral { external = outpoint } else { external = nil }
        let summary = RegistrationSummary(
            type: request.type, proTxHash: txid(500), collateral: external ?? OutPoint(txid: txid(501), vout: 1),
            collateralAddress: testnetAddress2, ownerAddress: request.ownerAddress ?? "yOwner",
            votingAddress: request.votingAddress ?? request.ownerAddress ?? "yOwner", payoutAddress: request.payoutAddress,
            operatorPublicKey: String(repeating: "ab", count: 48), operatorRewardX100: request.operatorRewardX100,
            serviceAddresses: request.serviceAddresses, platform: request.platform, fee: Amount(duffs: 1_000),
            totalSpent: Amount(duffs: 1_000), operatorSecretRequired: request.operatorKey == .generate,
            collateralSignMessage: external == nil ? nil : "payout|0|owner|voting|hash")
        return PreparedRegistrationReference(id: UUID(), summary: summary)
    }

    func operatorSecret(_ registration: PreparedRegistrationReference) async throws(ServiceError) -> OperatorSecret {
        guard registration.summary.operatorSecretRequired else {
            throw ServiceError(code: .invalidArgument, detail: "key not generated")
        }
        return OperatorSecret(secretHex: FakeSecret(secret), configLine: FakeSecret("masternodeblsprivkey=\(secret)"))
    }

    func confirmOperatorSecret(_ registration: PreparedRegistrationReference, last4: String) async throws(ServiceError)
        -> Bool
    {
        guard last4.lowercased() == String(secret.suffix(4)) else { return false }
        gate.withLock { _ = $0.insert(registration.id) }
        return true
    }

    func submit(_ registration: PreparedRegistrationReference, collateralSignature: String?) async throws(ServiceError)
        -> String
    {
        try errors.check("submit")
        if registration.summary.operatorSecretRequired, !gate.current.contains(registration.id) {
            throw ServiceError(code: .masternodeOperatorSecretUnconfirmed)
        }
        if registration.summary.collateralSignMessage != nil, collateralSignature?.isEmpty ?? true {
            throw ServiceError(code: .masternodeCollateralSignatureInvalid)
        }
        submitted.withLock { $0.append((registration.id, collateralSignature)) }
        return registration.summary.proTxHash
    }

    func abandon(_ registration: PreparedRegistrationReference) async {
        abandoned.withLock { $0.append(registration.id) }
    }

    // Maintenance
    private func prepared(_ kind: ProviderTransactionKind, _ hash: String, bans: Bool = false, penalty: Amount? = nil)
        -> PreparedProviderTransaction
    {
        PreparedProviderTransaction(
            id: UUID(),
            summary: ProviderTransactionSummary(
                kind: kind, proTxHash: hash, txid: txid(600), fee: Amount(duffs: 500), penalty: penalty,
                bansMasternode: bans))
    }

    func prepareUpdateService(_ request: UpdateServiceRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        try errors.check("prepareUpdateService")
        serviceRequests.withLock { $0.append(request) }
        return prepared(.updateService, request.proTxHash)
    }

    func prepareUpdateRegistrar(_ request: UpdateRegistrarRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        registrarRequests.withLock { $0.append(request) }
        return prepared(.updateRegistrar, request.proTxHash, bans: request.operatorPublicKey != nil)
    }

    func prepareRevoke(_ request: RevokeRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        providerRequests.withLock { $0.append("revoke:\(request.reason)") }
        return prepared(.revoke, request.proTxHash)
    }

    func prepareShareRewardUpdate(
        proTxHash: String, shareIndex: Int, payoutAddress: String, feeWallet: WalletID, grant: AuthGrant
    ) async throws(ServiceError) -> PreparedProviderTransaction {
        providerRequests.withLock { $0.append("share:\(shareIndex):\(payoutAddress)") }
        return prepared(.updateShare, proTxHash)
    }

    func prepareDissolveNow(proTxHash: String, acceptPenalty: Bool, feeWallet: WalletID, grant: AuthGrant)
        async throws(ServiceError) -> PreparedProviderTransaction
    {
        providerRequests.withLock { $0.append("dissolve:\(acceptPenalty)") }
        return prepared(.dissolve, proTxHash, penalty: acceptPenalty ? Amount(duffs: 10_00000000) : nil)
    }

    func broadcast(_ transaction: PreparedProviderTransaction) async throws(ServiceError) -> String {
        try errors.check("broadcast")
        broadcasts.withLock { $0.append(transaction.id) }
        return transaction.summary.txid
    }

    func abandon(_ transaction: PreparedProviderTransaction) async {
        abandoned.withLock { $0.append(transaction.id) }
    }

    func createStandbyDissolution(wallet: WalletID, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> StandbyDissolution
    {
        StandbyDissolution(proTxHash: proTxHash, transactionsHex: ["0100aa", "0100bb"], suggestedFileName: "standby.txt")
    }

    func broadcastStandbyDissolution(_ transactionsHex: [String]) async throws(ServiceError) -> [String] {
        transactionsHex.indices.map { txid(700 + $0) }
    }

    // Shared
    func create(wallet: WalletID, terms: SharedMasternodeTerms) async throws(ServiceError) -> SharedSessionInfo {
        let info = session("s1", role: .coordinator, stage: .invitation, wallet: wallet)
        sharedSessions.withLock { $0.append(info) }
        return info
    }

    /// ≤ 2 MiB, then a session envelope or standby text.
    func importMessage(_ text: String, wallet: WalletID) async throws(ServiceError) -> SharedMessageKind {
        let size = text.utf8.count
        guard size <= defaults().maxEnvelopeBytes else {
            throw ServiceError(code: .masternodeSharedEnvelopeTooLarge, parameters: ["size_bytes": Int64(size)])
        }
        if text.hasPrefix("standby:") {
            return .standbyDissolution(proTxHash: nil, transactionsHex: ["0100aa", "0100bb"])
        }
        guard text.contains("dash-shared-mn-session") else { throw ServiceError(code: .masternodeSharedEnvelopeInvalid) }
        let info = session("s2", role: .participant, stage: .details, wallet: wallet)
        sharedSessions.withLock { $0.append(info) }
        return .envelope(info)
    }

    func sessions(wallet: WalletID) async throws(ServiceError) -> [SharedSessionInfo] { sharedSessions.current }

    func message(session: String) async throws(ServiceError) -> SharedEnvelope {
        SharedEnvelope(json: "{\"type\":\"dash-shared-mn-session\"}", fingerprint: "ABCD-1234", suggestedFileName: "\(session).json")
    }

    func contribute(_ contribution: ShareContribution, session: String) async throws(ServiceError) -> SharedSessionInfo {
        try update(session) { self.session($0.id, role: $0.role, stage: .lockedTerms, wallet: $0.wallet, reserved: contribution.inputs) }
    }

    func approve(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        try update(session) { self.session($0.id, role: $0.role, stage: .signingRequest, wallet: $0.wallet, reserved: $0.reservedInputs) }
    }

    func sign(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        try update(session) { self.session($0.id, role: $0.role, stage: .signedContributions, wallet: $0.wallet, reserved: $0.reservedInputs) }
    }

    func broadcast(session: String) async throws(ServiceError) -> String {
        _ = try update(session) { self.session($0.id, role: $0.role, stage: .completed, wallet: $0.wallet) }
        return txid(800)
    }

    func abandon(session: String) async throws(ServiceError) {
        abandonedSessions.withLock { $0.append(session) }
        sharedSessions.withLock { $0.removeAll { $0.id == session } }
    }

    func startKeyRotation(wallet: WalletID, proTxHash: String, operatorKey: OperatorKeyChoice?, votingAddress: String?)
        async throws(ServiceError) -> SharedSessionInfo
    {
        let info = session("rot", role: .coordinator, stage: .lockedTerms, wallet: wallet, purpose: .rotateKeys)
        sharedSessions.withLock { $0.append(info) }
        return info
    }

    func startDissolveTogether(wallet: WalletID, proTxHash: String) async throws(ServiceError) -> SharedSessionInfo {
        let info = session("dis", role: .coordinator, stage: .lockedTerms, wallet: wallet, purpose: .dissolveTogether)
        sharedSessions.withLock { $0.append(info) }
        return info
    }

    private func session(
        _ id: String, role: SharedRole, stage: SharedStage, wallet: WalletID, reserved: [OutPoint] = [],
        purpose: SharedSessionPurpose = .register
    ) -> SharedSessionInfo {
        SharedSessionInfo(
            id: id, sessionCode: String(id.prefix(6)), purpose: purpose, role: role, stage: stage, revision: 1,
            fingerprint: "ABCD-1234", wallet: wallet, myShareIndexes: [0], reservedInputs: reserved,
            reservedCoinSpent: false, proTxHash: nil)
    }

    private func update(_ id: String, _ change: (SharedSessionInfo) -> SharedSessionInfo) throws(ServiceError)
        -> SharedSessionInfo
    {
        guard let current = sharedSessions.current.first(where: { $0.id == id }) else {
            throw ServiceError(code: .masternodeSharedSessionNotFound)
        }
        let next = change(current)
        sharedSessions.withLock { list in
            if let index = list.firstIndex(where: { $0.id == id }) { list[index] = next }
        }
        return next
    }

    // Keychain
    func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        guard range.count <= 100 else { throw ServiceError(code: .invalidArgument, detail: "count > 100") }
        guard let infos = keyInfos.current else { throw notConfigured("keys") }
        return infos.filter { $0.role == role && range.contains($0.index) }
    }

    func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        guard grant.purpose == .revealSecret else { throw ServiceError(code: .masternodeGrantInvalid) }
        return RevealedMasternodeKey(privateKeyHex: FakeSecret("priv-\(role)-\(index)"), wif: nil, tenderdashKey: nil)
    }

    // Tracked
    func locate(_ query: String) async throws(ServiceError) -> [MasternodeRow] {
        guard let rows = rows.current else { throw notConfigured("locate") }
        return rows.filter { ($0.service ?? "").hasPrefix(query) || $0.proTxHash == query }
    }

    func tracked() async throws(ServiceError) -> [TrackedMasternode] {
        guard let list = trackedList.current else { throw notConfigured("tracked") }
        return list
    }

    func track(proTxHash: String, label: String?) async throws(ServiceError) -> TrackedMasternode {
        if trackedList.current?.contains(where: { $0.id == proTxHash }) == true {
            throw ServiceError(code: .masternodeAlreadyTracked)
        }
        guard let row = rows.current?.first(where: { $0.proTxHash == proTxHash }) else {
            throw ServiceError(code: .masternodeNotFound)
        }
        let node = TrackedMasternode(
            row: row, label: label, attachedRoles: [],
            capabilities: TrackedCapabilities(canWithdraw: false, canUpdateService: false, canUpdateRegistrar: false, canVote: false))
        trackedList.withLock { $0 = ($0 ?? []) + [node] }
        return node
    }

    func untrack(proTxHash: String) async throws(ServiceError) -> Bool {
        trackedList.withLock { $0?.removeAll { $0.id == proTxHash } }
        return true
    }

    func setLabel(_ label: String?, proTxHash: String) async throws(ServiceError) {}

    func attach(_ key: any SecretBuffer, role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError)
    {
        guard grant.purpose == .masternodeOperation else { throw ServiceError(code: .masternodeGrantInvalid) }
        attached.withLock { $0.append((proTxHash, role, key.testString)) }
    }

    func detach(role: MasternodeKeyRole, proTxHash: String) async throws(ServiceError) {}

    func reveal(role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        RevealedMasternodeKey(privateKeyHex: FakeSecret("attached-\(role)"), wif: nil, tenderdashKey: nil)
    }

    // Evonode: Platform work, M4 (the engine's `.platform` stubs).
    func status(proTxHash: String) async throws(ServiceError) -> EvonodePlatformStatus {
        throw ServiceError(code: .notImplemented, detail: "NetworkSession.evonode_status.platform")
    }

    func withdraw(proTxHash: String, credits: UInt64, destination: CreditWithdrawalDestination, grant: AuthGrant)
        async throws(ServiceError) -> String
    {
        throw ServiceError(code: .notImplemented, detail: "NetworkSession.withdraw_evonode_credits.platform")
    }
}

func masternodeRow(
    _ n: Int, type: MasternodeType = .regular, status: MasternodeListStatus = .active(sinceHeight: 1_000),
    service: String? = "1.2.3.4:19999", owned: Set<OwnedRole> = [], shared: SharedHolding? = nil,
    collateral: OutPoint? = nil, reward: OperatorReward? = OperatorReward(percentX100: 0, payoutAddress: nil)
) -> MasternodeRow {
    MasternodeRow(
        proTxHash: txid(n), service: service, type: type, shared: shared, status: status, poseScore: nil,
        registeredHeight: 900, lastPaidHeight: nil, nextPaymentHeight: nil, operatorReward: reward,
        collateral: collateral, collateralAddress: collateral.map { _ in testnetAddress2 },
        ownerAddress: owned.contains(.owner) ? "yOwnerAddress" : nil, votingAddress: "yVotingAddress\(n)",
        payoutAddresses: [], operatorPublicKey: String(repeating: "cd", count: 48), platformNodeID: nil,
        ownedRoles: owned, label: nil)
}

func masternodeDetail(_ row: MasternodeRow, shares: [MasternodeShare] = []) -> MasternodeDetail {
    MasternodeDetail(
        row: row, consecutivePayments: nil, poseBanHeight: nil, poseRevivedHeight: nil,
        networkAddresses: row.service.map { [$0] } ?? [], platformP2PAddresses: [], platformHTTPSAddresses: [],
        shares: shares, earlyPeriodEnd: nil, earlyExitPenalty: nil, hasStandbyDissolution: false, revocationReason: nil,
        walletTransactions: 1)
}

// MARK: World

@MainActor
final class FakeM3World {
    let coinJoin = FakeCoinJoin()
    let governance: FakeGovernance
    let masternodes: FakeMasternodes

    init(network: DashNetwork = .testnet, now: @escaping @Sendable () -> Date = { Date() }) {
        governance = FakeGovernance(network: network, now: now)
        masternodes = FakeMasternodes(network: network)
    }

    var services: M3Services {
        M3Services(
            coinJoin: coinJoin, mixedCoins: coinJoin, networkStatistics: coinJoin, governance: governance,
            voting: governance, proposals: governance, masternodes: masternodes, registration: masternodes,
            maintenance: masternodes, shared: masternodes, keychain: masternodes, tracked: masternodes,
            evonodes: masternodes)
    }
}
