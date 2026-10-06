// The live `M3Services`: the R1–R3 adapters over the engine
// (`WalletRuntime/M3/<Domain>Adapters.swift`). A domain whose adapter has not
// landed is served by `UnavailableM3Service`, which answers every call with
// `not_implemented` like the engine stubs (m3-engine.md intro), so its
// screens say "Not available yet" instead of showing an empty success state.
import Foundation
import WalletRuntime

extension M3Services {
    /// The M3 services of a live app run. Pass each adapter as it lands; the
    /// rest answer `not_implemented`.
    public static func live(
        network: DashNetwork, coinJoin: (any CoinJoinControlling)? = nil, mixedCoins: (any MixedCoinsMoving)? = nil,
        networkStatistics: (any NetworkStatisticsProviding)? = nil, governance: (any GovernanceProviding)? = nil,
        voting: (any GovernanceVoting)? = nil, proposals: (any ProposalCreating)? = nil,
        masternodes: (any MasternodeListProviding)? = nil, registration: (any MasternodeRegistering)? = nil,
        maintenance: (any MasternodeMaintaining)? = nil, shared: (any SharedMasternodeCoordinating)? = nil,
        keychain: (any MasternodeKeychainProviding)? = nil, tracked: (any TrackedMasternodeManaging)? = nil,
        evonodes: (any EvonodeServicing)? = nil
    ) -> M3Services {
        let missing = UnavailableM3Service(network: network)
        return M3Services(
            coinJoin: coinJoin ?? missing, mixedCoins: mixedCoins ?? missing,
            networkStatistics: networkStatistics ?? missing, governance: governance ?? missing,
            voting: voting ?? missing, proposals: proposals ?? missing, masternodes: masternodes ?? missing,
            registration: registration ?? missing, maintenance: maintenance ?? missing, shared: shared ?? missing,
            keychain: keychain ?? missing, tracked: tracked ?? missing, evonodes: evonodes ?? missing)
    }

    /// Every M3 domain answering `not_implemented`.
    public static func unavailable(network: DashNetwork) -> M3Services {
        live(network: network)
    }
}

/// Stands in for an M3 adapter that has not landed. The constant calls return
/// Core's values (`M3Defaults`, as the engine's working calls do); every other
/// call throws `not_implemented` naming the engine call; streams end at once.
public final class UnavailableM3Service: Sendable {
    public let network: DashNetwork

    public init(network: DashNetwork) {
        self.network = network
    }

    private func missing(_ call: String) -> ServiceError {
        ServiceError(code: .notImplemented, detail: call)
    }

    private func ended<T>() -> AsyncStream<T> {
        AsyncStream { $0.finish() }
    }
}

extension UnavailableM3Service: CoinJoinControlling, MixedCoinsMoving, NetworkStatisticsProviding {
    public func limits() -> CoinJoinLimits { M3Defaults.coinJoinLimits }
    public func settings() async throws(ServiceError) -> CoinJoinSettings { throw missing("coinjoin_settings") }
    public func setSettings(_ settings: CoinJoinSettings) async throws(ServiceError) {
        if M3Defaults.invalidCoinJoinField(settings) != nil {
            throw ServiceError(code: .invalidArgument, detail: "coinjoin settings")
        }
        throw missing("set_coinjoin_settings")
    }
    public func status(wallet: WalletID) async throws(ServiceError) -> CoinJoinStatus { throw missing("coinjoin_status") }
    public func statusChanges() -> AsyncStream<WalletID> { ended() }
    public func start(wallet: WalletID) async throws(ServiceError) { throw missing("start_mixing") }
    public func stop(wallet: WalletID) async throws(ServiceError) { throw missing("stop_mixing") }
    public func salt(wallet: WalletID) async throws(ServiceError) -> String { throw missing("coinjoin_salt") }
    public func setSalt(_ salt: String, wallet: WalletID) async throws(ServiceError) {
        guard M3Defaults.isValidSalt(salt) else { throw ServiceError(code: .invalidArgument, detail: "salt") }
        throw missing("set_coinjoin_salt")
    }
    public func generateSalt(wallet: WalletID) async throws(ServiceError) -> String {
        throw missing("generate_coinjoin_salt")
    }
    public func recoveryScan(wallet: WalletID) async throws(ServiceError) -> CoinJoinRecoveryReport {
        throw missing("coinjoin_recovery_scan")
    }
    public func plan(wallet: WalletID, destination: MixedCoinsDestination) async throws(ServiceError)
        -> MixedCoinsSweepPlan
    {
        throw missing("mixed_coins_sweep_plan")
    }
    public func move(wallet: WalletID, destination: MixedCoinsDestination, grant: AuthGrant)
        async throws(ServiceError) -> MixedCoinsSweepResult
    {
        throw missing("move_mixed_coins")
    }
    public func statistics() async throws(ServiceError) -> NetworkStatistics { throw missing("network_stats") }
}

extension UnavailableM3Service: GovernanceProviding, GovernanceVoting, ProposalCreating {
    public func parameters() -> GovernanceParameters { M3Defaults.governanceParameters(network) }
    public func syncState() async throws(ServiceError) -> GovernanceSyncState { throw missing("governance_sync_state") }
    public func setSyncEnabled(_ enabled: Bool) async throws(ServiceError) {
        throw missing("set_governance_sync_enabled")
    }
    public func changes() -> AsyncStream<Void> { ended() }
    public func proposals(_ query: ProposalQuery) async throws(ServiceError) -> [ProposalRow] {
        throw missing("proposals")
    }
    public func detail(hash: String) async throws(ServiceError) -> ProposalDetail { throw missing("proposal_detail") }
    public func info() async throws(ServiceError) -> GovernanceInfo { throw missing("governance_info") }
    public func clock() async throws(ServiceError) -> GovernanceClock { throw missing("governance_clock") }
    public func votingMasternodes(proposal hash: String, wallet: WalletID?) async throws(ServiceError)
        -> [VotingMasternode]
    {
        throw missing("voting_masternodes")
    }
    public func cast(_ outcome: VoteOutcome, on hash: String, with proTxHashes: [String], grant: AuthGrant)
        async throws(ServiceError) -> [VoteResult]
    {
        throw missing("cast_votes")
    }
    public func superblockDates(count: Int) async throws(ServiceError) -> [SuperblockDate] {
        throw missing("superblock_dates")
    }
    public func validate(_ draft: ProposalDraft) async throws(ServiceError) -> [ProposalField] {
        throw missing("validate_proposal")
    }
    public func json(_ draft: ProposalDraft) async throws(ServiceError) -> String { throw missing("proposal_json") }
    public func payloadHex(_ draft: ProposalDraft) async throws(ServiceError) -> String {
        throw missing("proposal_payload_hex")
    }
    public func create(wallet: WalletID, draft: ProposalDraft, grant: AuthGrant) async throws(ServiceError)
        -> PendingProposal
    {
        throw missing("create_proposal")
    }
    public func pending(wallet: WalletID) async throws(ServiceError) -> [PendingProposal] {
        throw missing("pending_proposals")
    }
    public func submit(wallet: WalletID, hash: String) async throws(ServiceError) -> String {
        throw missing("submit_proposal")
    }
}

extension UnavailableM3Service: MasternodeListProviding, MasternodeRegistering, MasternodeMaintaining {
    public func defaults() -> MasternodeNetworkDefaults { M3Defaults.masternodeDefaults(network) }
    public func state() async throws(ServiceError) -> MasternodeListState { throw missing("masternode_list_state") }
    public func list(_ query: MasternodeQuery) async throws(ServiceError) -> [MasternodeRow] {
        throw missing("masternodes")
    }
    public func detail(proTxHash: String) async throws(ServiceError) -> MasternodeDetail {
        throw missing("masternode_detail")
    }
    public func collateralCandidates(wallet: WalletID, type: MasternodeType) async throws(ServiceError)
        -> [CollateralCandidate]
    {
        throw missing("collateral_candidates")
    }
    public func feeSourceCandidates(wallet: WalletID) async throws(ServiceError) -> [FeeSourceCandidate] {
        throw missing("fee_source_candidates")
    }
    public func prepare(_ request: RegistrationRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedRegistrationReference
    {
        throw missing("prepare_registration")
    }
    public func operatorSecret(_ registration: PreparedRegistrationReference) async throws(ServiceError)
        -> OperatorSecret
    {
        throw missing("PreparedRegistration.operator_secret")
    }
    public func confirmOperatorSecret(_ registration: PreparedRegistrationReference, last4: String)
        async throws(ServiceError) -> Bool
    {
        throw missing("PreparedRegistration.confirm_operator_secret")
    }
    public func submit(_ registration: PreparedRegistrationReference, collateralSignature: String?)
        async throws(ServiceError) -> String
    {
        throw missing("PreparedRegistration.submit")
    }
    public func abandon(_ registration: PreparedRegistrationReference) async {}
    public func prepareUpdateService(_ request: UpdateServiceRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        throw missing("prepare_update_service")
    }
    public func prepareUpdateRegistrar(_ request: UpdateRegistrarRequest, grant: AuthGrant)
        async throws(ServiceError) -> PreparedProviderTransaction
    {
        throw missing("prepare_update_registrar")
    }
    public func prepareRevoke(_ request: RevokeRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        throw missing("prepare_revoke")
    }
    public func prepareShareRewardUpdate(
        proTxHash: String, shareIndex: Int, payoutAddress: String, feeWallet: WalletID, grant: AuthGrant
    ) async throws(ServiceError) -> PreparedProviderTransaction {
        throw missing("prepare_share_reward_update")
    }
    public func prepareDissolveNow(proTxHash: String, acceptPenalty: Bool, feeWallet: WalletID, grant: AuthGrant)
        async throws(ServiceError) -> PreparedProviderTransaction
    {
        throw missing("prepare_dissolve_now")
    }
    public func broadcast(_ transaction: PreparedProviderTransaction) async throws(ServiceError) -> String {
        throw missing("PreparedProviderTx.broadcast")
    }
    public func abandon(_ transaction: PreparedProviderTransaction) async {}
    public func createStandbyDissolution(wallet: WalletID, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError) -> StandbyDissolution
    {
        throw missing("create_standby_dissolution")
    }
    public func broadcastStandbyDissolution(_ transactionsHex: [String]) async throws(ServiceError) -> [String] {
        throw missing("broadcast_standby_dissolution")
    }
}

extension UnavailableM3Service: SharedMasternodeCoordinating {
    public func create(wallet: WalletID, terms: SharedMasternodeTerms) async throws(ServiceError) -> SharedSessionInfo {
        throw missing("create_shared_session")
    }
    public func importMessage(_ text: String, wallet: WalletID) async throws(ServiceError) -> SharedMessageKind {
        let size = text.utf8.count
        if size > M3Defaults.masternodeDefaults(network).maxEnvelopeBytes {
            throw ServiceError(
                code: .masternodeSharedEnvelopeTooLarge, detail: "envelope", parameters: ["size_bytes": Int64(size)])
        }
        throw missing("import_shared_message")
    }
    public func sessions(wallet: WalletID) async throws(ServiceError) -> [SharedSessionInfo] {
        throw missing("shared_sessions")
    }
    public func message(session: String) async throws(ServiceError) -> SharedEnvelope {
        throw missing("shared_session_message")
    }
    public func contribute(_ contribution: ShareContribution, session: String) async throws(ServiceError)
        -> SharedSessionInfo
    {
        throw missing("shared_session_contribute")
    }
    public func approve(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        throw missing("shared_session_approve")
    }
    public func sign(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        throw missing("shared_session_sign")
    }
    public func broadcast(session: String) async throws(ServiceError) -> String {
        throw missing("shared_session_broadcast")
    }
    public func abandon(session: String) async throws(ServiceError) { throw missing("shared_session_abandon") }
    public func startKeyRotation(
        wallet: WalletID, proTxHash: String, operatorKey: OperatorKeyChoice?, votingAddress: String?
    ) async throws(ServiceError) -> SharedSessionInfo {
        throw missing("start_shared_key_rotation")
    }
    public func startDissolveTogether(wallet: WalletID, proTxHash: String) async throws(ServiceError)
        -> SharedSessionInfo
    {
        throw missing("start_dissolve_together")
    }
}

extension UnavailableM3Service: MasternodeKeychainProviding, TrackedMasternodeManaging, EvonodeServicing {
    public func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        guard range.count <= 100 else { throw ServiceError(code: .invalidArgument, detail: "count > 100") }
        throw missing("masternode_keys")
    }
    public func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant)
        async throws(ServiceError) -> RevealedMasternodeKey
    {
        throw missing("Vault.reveal_masternode_key")
    }
    public func locate(_ query: String) async throws(ServiceError) -> [MasternodeRow] {
        throw missing("locate_masternodes")
    }
    public func tracked() async throws(ServiceError) -> [TrackedMasternode] { throw missing("tracked_masternodes") }
    public func track(proTxHash: String, label: String?) async throws(ServiceError) -> TrackedMasternode {
        throw missing("track_masternode")
    }
    public func untrack(proTxHash: String) async throws(ServiceError) -> Bool { throw missing("untrack_masternode") }
    public func setLabel(_ label: String?, proTxHash: String) async throws(ServiceError) {
        throw missing("set_tracked_masternode_label")
    }
    public func attach(_ key: any SecretBuffer, role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError)
    {
        throw missing("attach_masternode_key")
    }
    public func detach(role: MasternodeKeyRole, proTxHash: String) async throws(ServiceError) {
        throw missing("detach_masternode_key")
    }
    public func reveal(role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        throw missing("Vault.reveal_masternode_key")
    }
    public func status(proTxHash: String) async throws(ServiceError) -> EvonodePlatformStatus {
        throw missing("NetworkSession.evonode_status.platform")
    }
    public func withdraw(
        proTxHash: String, credits: UInt64, destination: CreditWithdrawalDestination, grant: AuthGrant
    ) async throws(ServiceError) -> String {
        throw missing("NetworkSession.withdraw_evonode_credits.platform")
    }
}
