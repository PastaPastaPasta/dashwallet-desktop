// Adapters for the M3 masternode calls (m3-swift.md §2.3, owner R3):
// `MasternodeListProviding`, `MasternodeRegistering`, `MasternodeMaintaining`,
// `SharedMasternodeCoordinating`, `MasternodeKeychainProviding`,
// `TrackedMasternodeManaging` and `EvonodeServicing` over `EngineClient`
// (DashKit `EngineClient+M3Masternodes.swift`). One service per network, as
// `M3Services.live(network:…)` builds them. The engine's prepared
// registrations and provider transactions stay here behind the references'
// ids; shared-masternode sessions and the evonode Platform calls answer what
// the engine answers (`not_implemented` until they land).
import DashKit
import Foundation

/// Every masternode protocol over the engine of one network.
public final class MasternodeService: @unchecked Sendable {
    private let client: EngineClient
    private let network: DashNetwork
    // `lock` guards `registrations` and `transactions`.
    private let lock = NSLock()
    private var registrations: [UUID: MNPreparedRegistration] = [:]
    private var transactions: [UUID: MNPreparedProviderTx] = [:]

    public init(client: EngineClient, network: DashNetwork) {
        self.client = client
        self.network = network
    }

    private var kitNetwork: DashKit.DashNetwork { network.kit }

    /// Runs an engine call on this network and maps its error.
    private func call<T>(
        _ body: (EngineClient, DashKit.DashNetwork) async throws(DashKitError) -> T
    ) async throws(ServiceError) -> T {
        let client = client
        let network = kitNetwork
        return try await serviceCall { () async throws(DashKitError) in try await body(client, network) }
    }

    private func requireGrant(_ grant: AuthGrant, _ purpose: GrantPurpose, _ what: String) throws(ServiceError) {
        guard grant.purpose == purpose else {
            throw ServiceError(code: .vaultGrantPurposeMismatch, detail: "\(what) needs a \(purpose) grant")
        }
    }

    private func registration(_ ref: PreparedRegistrationReference) throws(ServiceError) -> MNPreparedRegistration {
        guard let prepared = lock.withLock({ registrations[ref.id] }) else {
            throw ServiceError(code: .invalidArgument, detail: "the prepared registration is gone")
        }
        return prepared
    }

    private func transaction(_ ref: PreparedProviderTransaction) throws(ServiceError) -> MNPreparedProviderTx {
        guard let prepared = lock.withLock({ transactions[ref.id] }) else {
            throw ServiceError(code: .invalidArgument, detail: "the prepared provider transaction is gone")
        }
        return prepared
    }

    private func keep(_ prepared: MNPreparedProviderTx) -> PreparedProviderTransaction {
        let id = UUID()
        lock.withLock { transactions[id] = prepared }
        return PreparedProviderTransaction(id: id, summary: ProviderTransactionSummary(prepared.summary))
    }
}

// MARK: - MasternodeListProviding (QT-118…122, IOS-080)

extension MasternodeService: MasternodeListProviding {
    public func defaults() -> MasternodeNetworkDefaults {
        let d = client.masternodeDefaults(for: kitNetwork)
        return MasternodeNetworkDefaults(
            coreP2PPort: d.coreP2PPort, platformP2PPort: d.platformP2PPort, platformHTTPSPort: d.platformHTTPSPort,
            masternodeCollateral: .duffs(d.masternodeCollateral), evonodeCollateral: .duffs(d.evonodeCollateral),
            shares: Int(d.minShares)...Int(d.maxShares), minimumShareAmount: .duffs(d.minShareAmount),
            maxEarlyPeriodBlocks: d.maxEarlyPeriodBlocks, maxEnvelopeBytes: Int(clamping: d.maxEnvelopeBytes),
            maxOperatorRewardX100: Int(d.maxOperatorRewardX100))
    }

    public func state() async throws(ServiceError) -> MasternodeListState {
        let s = try await call { client, network throws(DashKitError) in try await client.masternodeListState(on: network) }
        return MasternodeListState(
            available: s.available, height: s.height, total: Int(s.total), enabled: Int(s.enabled),
            evoTotal: Int(s.evoTotal), evoEnabled: Int(s.evoEnabled), syncing: s.syncing)
    }

    /// Engine `Masternodes` events of this network, and `resynchronize`.
    public func changes() -> AsyncStream<Void> {
        let subscription = client.events.subscribe()
        let network = kitNetwork
        let (stream, continuation) = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let pump = Task {
            for await event in subscription {
                if Task.isCancelled { break }
                switch event {
                case .masternodesChanged(let n) where n == network:
                    continuation.yield()
                case .resynchronize:
                    continuation.yield()
                default:
                    continue
                }
            }
            continuation.finish()
        }
        continuation.onTermination = { _ in pump.cancel() }
        return stream
    }

    public func list(_ query: MasternodeQuery) async throws(ServiceError) -> [MasternodeRow] {
        let kitQuery = MNQuery(
            typeFilter: query.typeFilter.kit, text: query.text, ownedOnly: query.ownedOnly, hideBanned: query.hideBanned)
        let rows = try await call { client, network throws(DashKitError) in
            try await client.masternodes(on: network, query: kitQuery)
        }
        return rows.map(MasternodeRow.init)
    }

    public func detail(proTxHash: String) async throws(ServiceError) -> MasternodeDetail {
        let d = try await call { client, network throws(DashKitError) in
            try await client.masternodeDetail(on: network, proTxHash: proTxHash)
        }
        return MasternodeDetail(
            row: MasternodeRow(d.row), consecutivePayments: d.consecutivePayments.map(Int.init),
            poseBanHeight: d.poseBanHeight, poseRevivedHeight: d.poseRevivedHeight,
            networkAddresses: d.networkAddresses, platformP2PAddresses: d.platformP2PAddresses,
            platformHTTPSAddresses: d.platformHTTPSAddresses,
            shares: d.shares.map {
                MasternodeShare(
                    amount: .duffs($0.amount), ownerAddress: $0.ownerAddress, payoutAddress: $0.payoutAddress,
                    refundAddress: $0.refundAddress, mine: $0.mine)
            }, earlyPeriodEnd: d.earlyPeriodEnd, earlyExitPenalty: d.earlyExitPenalty.map(Amount.duffs),
            hasStandbyDissolution: d.hasStandbyDissolution, revocationReason: d.revocationReason.map(Int.init),
            walletTransactions: Int(d.walletTransactions))
    }
}

// MARK: - MasternodeRegistering (QT-123, QT-124)

extension MasternodeService: MasternodeRegistering {
    public func collateralCandidates(wallet: WalletID, type: MasternodeType) async throws(ServiceError)
        -> [CollateralCandidate]
    {
        let id = try wallet.kit
        let rows = try await call { client, network throws(DashKitError) in
            try await client.collateralCandidates(on: network, wallet: id, type: type.kit)
        }
        return rows.map {
            CollateralCandidate(
                outpoint: OutPoint($0.outpoint), address: $0.address, amount: .duffs($0.amount),
                confirmations: Int($0.confirmations), refusal: $0.refusal.map(CollateralRefusal.init))
        }
    }

    public func feeSourceCandidates(wallet: WalletID) async throws(ServiceError) -> [FeeSourceCandidate] {
        let id = try wallet.kit
        let rows = try await call { client, network throws(DashKitError) in
            try await client.feeSourceCandidates(on: network, wallet: id)
        }
        return rows.map { FeeSourceCandidate(address: $0.address, spendable: .duffs($0.spendable), label: $0.label) }
    }

    public func prepare(_ request: RegistrationRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedRegistrationReference
    {
        try requireGrant(grant, .masternodeOperation, "registering a masternode")
        guard let reward = UInt16(exactly: request.operatorRewardX100) else {
            throw ServiceError(code: .invalidArgument, detail: "operator reward out of range")
        }
        let collateral: MNCollateralChoice =
            switch request.collateral {
            case .fundNew: .fundNew
            case .existingUTXO(let o): .existingUTXO(o.kit)
            case .external(let o): .external(o.kit)
            }
        let kitRequest = MNRegistrationRequest(
            wallet: try request.wallet.kit, type: request.type.kit, collateral: collateral,
            serviceAddresses: request.serviceAddresses, ownerAddress: request.ownerAddress,
            votingAddress: request.votingAddress, operatorKey: request.operatorKey.kit,
            payoutAddress: request.payoutAddress, operatorRewardX100: reward, platform: request.platform?.kit,
            feeSource: request.feeSource.kit)
        let grantID = grant.id
        let prepared = try await call { client, network throws(DashKitError) in
            try await client.prepareRegistration(on: network, kitRequest, grantID: grantID)
        }
        let id = UUID()
        lock.withLock { registrations[id] = prepared }
        return PreparedRegistrationReference(id: id, summary: RegistrationSummary(prepared.summary))
    }

    public func operatorSecret(_ registration: PreparedRegistrationReference) async throws(ServiceError)
        -> OperatorSecret
    {
        let prepared = try self.registration(registration)
        let secret = try serviceCall { () throws(DashKitError) in try prepared.operatorSecret() }
        return OperatorSecret(secretHex: secret.secretHex, configLine: secret.configLine)
    }

    public func confirmOperatorSecret(_ registration: PreparedRegistrationReference, last4: String)
        async throws(ServiceError) -> Bool
    {
        let prepared = try self.registration(registration)
        return try serviceCall { () throws(DashKitError) in try prepared.confirmOperatorSecret(last4: last4) }
    }

    public func submit(_ registration: PreparedRegistrationReference, collateralSignature: String?)
        async throws(ServiceError) -> String
    {
        let prepared = try self.registration(registration)
        let hash = try await serviceCall { () async throws(DashKitError) in
            try await prepared.submit(collateralSignature: collateralSignature)
        }
        lock.withLock { registrations[registration.id] = nil }
        return hash
    }

    /// Releases the inputs; a registration the engine no longer holds is
    /// already released.
    public func abandon(_ registration: PreparedRegistrationReference) async {
        guard let prepared = lock.withLock({ registrations.removeValue(forKey: registration.id) }) else { return }
        try? await prepared.abandon()
    }
}

// MARK: - MasternodeMaintaining (QT-125, QT-127, IOS-081)

extension MasternodeService: MasternodeMaintaining {
    public func prepareUpdateService(_ request: UpdateServiceRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        try requireGrant(grant, .masternodeOperation, "Update Service")
        let feeWallet = try request.feeWallet.kit
        let secret = request.operatorSecret.map(secretBytes)
        let grantID = grant.id
        let platform = request.platform?.kit
        let feeSource = request.feeSource.kit
        let prepared = try await call { client, network throws(DashKitError) in
            try await client.prepareUpdateService(
                on: network, proTxHash: request.proTxHash, serviceAddresses: request.serviceAddresses,
                operatorSecret: secret, platform: platform, operatorPayoutAddress: request.operatorPayoutAddress,
                feeSource: feeSource, feeWallet: feeWallet, grantID: grantID)
        }
        return keep(prepared)
    }

    public func prepareUpdateRegistrar(_ request: UpdateRegistrarRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        try requireGrant(grant, .masternodeOperation, "Update Registrar")
        let feeWallet = try request.feeWallet.kit
        let grantID = grant.id
        let feeSource = request.feeSource.kit
        let prepared = try await call { client, network throws(DashKitError) in
            try await client.prepareUpdateRegistrar(
                on: network, proTxHash: request.proTxHash, operatorPublicKey: request.operatorPublicKey,
                votingAddress: request.votingAddress, payoutAddress: request.payoutAddress, feeSource: feeSource,
                feeWallet: feeWallet, grantID: grantID)
        }
        return keep(prepared)
    }

    public func prepareRevoke(_ request: RevokeRequest, grant: AuthGrant) async throws(ServiceError)
        -> PreparedProviderTransaction
    {
        try requireGrant(grant, .masternodeOperation, "Revoke")
        let feeWallet = try request.feeWallet.kit
        let secret = request.operatorSecret.map(secretBytes)
        let grantID = grant.id
        let reason = request.reason.kit
        let feeSource = request.feeSource.kit
        let prepared = try await call { client, network throws(DashKitError) in
            try await client.prepareRevoke(
                on: network, proTxHash: request.proTxHash, operatorSecret: secret, reason: reason,
                feeSource: feeSource, feeWallet: feeWallet, grantID: grantID)
        }
        return keep(prepared)
    }

    public func prepareShareRewardUpdate(
        proTxHash: String, shareIndex: Int, payoutAddress: String, feeWallet: WalletID, grant: AuthGrant
    ) async throws(ServiceError) -> PreparedProviderTransaction {
        try requireGrant(grant, .masternodeOperation, "Change Reward Address")
        guard let index = UInt32(exactly: shareIndex) else {
            throw ServiceError(code: .invalidArgument, detail: "share index out of range")
        }
        let wallet = try feeWallet.kit
        let grantID = grant.id
        let prepared = try await call { client, network throws(DashKitError) in
            try await client.prepareShareRewardUpdate(
                on: network, proTxHash: proTxHash, shareIndex: index, payoutAddress: payoutAddress, feeWallet: wallet,
                grantID: grantID)
        }
        return keep(prepared)
    }

    public func prepareDissolveNow(proTxHash: String, acceptPenalty: Bool, feeWallet: WalletID, grant: AuthGrant)
        async throws(ServiceError) -> PreparedProviderTransaction
    {
        try requireGrant(grant, .masternodeOperation, "Dissolve Now")
        let wallet = try feeWallet.kit
        let grantID = grant.id
        let prepared = try await call { client, network throws(DashKitError) in
            try await client.prepareDissolveNow(
                on: network, proTxHash: proTxHash, acceptPenalty: acceptPenalty, feeWallet: wallet, grantID: grantID)
        }
        return keep(prepared)
    }

    public func broadcast(_ transaction: PreparedProviderTransaction) async throws(ServiceError) -> String {
        let prepared = try self.transaction(transaction)
        let txid = try await serviceCall { () async throws(DashKitError) in try await prepared.broadcast() }
        lock.withLock { transactions[transaction.id] = nil }
        return txid
    }

    public func abandon(_ transaction: PreparedProviderTransaction) async {
        guard let prepared = lock.withLock({ transactions.removeValue(forKey: transaction.id) }) else { return }
        try? await prepared.abandon()
    }

    public func createStandbyDissolution(wallet: WalletID, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError) -> StandbyDissolution
    {
        try requireGrant(grant, .masternodeOperation, "Create Standby Dissolution")
        let id = try wallet.kit
        let grantID = grant.id
        let s = try await call { client, network throws(DashKitError) in
            try await client.createStandbyDissolution(on: network, wallet: id, proTxHash: proTxHash, grantID: grantID)
        }
        return StandbyDissolution(
            proTxHash: s.proTxHash, transactionsHex: s.transactionsHex, suggestedFileName: s.suggestedFileName)
    }

    public func broadcastStandbyDissolution(_ transactionsHex: [String]) async throws(ServiceError) -> [String] {
        try await call { client, network throws(DashKitError) in
            try await client.broadcastStandbyDissolution(on: network, transactionsHex: transactionsHex)
        }
    }
}

// MARK: - SharedMasternodeCoordinating (QT-126, QT-127)

extension MasternodeService: SharedMasternodeCoordinating {
    public func create(wallet: WalletID, terms: SharedMasternodeTerms) async throws(ServiceError) -> SharedSessionInfo {
        let id = try wallet.kit
        guard let reward = UInt16(exactly: terms.operatorRewardX100),
            let penalty = UInt64(exactly: terms.earlyExitPenalty.duffs),
            case let shares = terms.shares.compactMap({ share in
                UInt64(exactly: share.amount.duffs).map { MNSharedShareTerms(amount: $0, label: share.label, mine: share.mine) }
            }), shares.count == terms.shares.count
        else {
            throw ServiceError(code: .invalidArgument, detail: "shared masternode terms out of range")
        }
        let kitTerms = MNSharedTerms(
            shares: shares, earlyPeriodBlocks: terms.earlyPeriodBlocks, earlyExitPenalty: penalty,
            serviceAddresses: terms.serviceAddresses, operatorKey: terms.operatorKey.kit, operatorRewardX100: reward)
        return SharedSessionInfo(
            try await call { client, network throws(DashKitError) in
                try await client.createSharedSession(on: network, wallet: id, terms: kitTerms)
            })
    }

    public func importMessage(_ text: String, wallet: WalletID) async throws(ServiceError) -> SharedMessageKind {
        let id = try wallet.kit
        let kind = try await call { client, network throws(DashKitError) in
            try await client.importSharedMessage(on: network, wallet: id, text: text)
        }
        switch kind {
        case .envelope(let s): return .envelope(SharedSessionInfo(s))
        case .standbyDissolution(let hash, let txs): return .standbyDissolution(proTxHash: hash, transactionsHex: txs)
        }
    }

    public func sessions(wallet: WalletID) async throws(ServiceError) -> [SharedSessionInfo] {
        let id = try wallet.kit
        return try await call { client, network throws(DashKitError) in
            try await client.sharedSessions(on: network, wallet: id)
        }.map(SharedSessionInfo.init)
    }

    public func message(session: String) async throws(ServiceError) -> SharedEnvelope {
        let e = try await call { client, network throws(DashKitError) in
            try await client.sharedSessionMessage(on: network, sessionID: session)
        }
        return SharedEnvelope(json: e.json, fingerprint: e.fingerprint, suggestedFileName: e.suggestedFileName)
    }

    public func contribute(_ contribution: ShareContribution, session: String) async throws(ServiceError)
        -> SharedSessionInfo
    {
        guard let index = UInt32(exactly: contribution.shareIndex) else {
            throw ServiceError(code: .invalidArgument, detail: "share index out of range")
        }
        let kit = MNShareContribution(
            shareIndex: index, inputs: contribution.inputs.map(\.kit), ownerAddress: contribution.ownerAddress,
            payoutAddress: contribution.payoutAddress, refundAddress: contribution.refundAddress)
        return SharedSessionInfo(
            try await call { client, network throws(DashKitError) in
                try await client.sharedSessionContribute(on: network, sessionID: session, contribution: kit)
            })
    }

    public func approve(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        try requireGrant(grant, .masternodeOperation, "approving shared terms")
        let grantID = grant.id
        return SharedSessionInfo(
            try await call { client, network throws(DashKitError) in
                try await client.sharedSessionApprove(on: network, sessionID: session, grantID: grantID)
            })
    }

    public func sign(session: String, grant: AuthGrant) async throws(ServiceError) -> SharedSessionInfo {
        try requireGrant(grant, .masternodeOperation, "signing a shared transaction")
        let grantID = grant.id
        return SharedSessionInfo(
            try await call { client, network throws(DashKitError) in
                try await client.sharedSessionSign(on: network, sessionID: session, grantID: grantID)
            })
    }

    public func broadcast(session: String) async throws(ServiceError) -> String {
        try await call { client, network throws(DashKitError) in
            try await client.sharedSessionBroadcast(on: network, sessionID: session)
        }
    }

    public func abandon(session: String) async throws(ServiceError) {
        try await call { client, network throws(DashKitError) in
            try await client.sharedSessionAbandon(on: network, sessionID: session)
        }
    }

    public func startKeyRotation(
        wallet: WalletID, proTxHash: String, operatorKey: OperatorKeyChoice?, votingAddress: String?
    ) async throws(ServiceError) -> SharedSessionInfo {
        let id = try wallet.kit
        let key = operatorKey?.kit
        return SharedSessionInfo(
            try await call { client, network throws(DashKitError) in
                try await client.startSharedKeyRotation(
                    on: network, wallet: id, proTxHash: proTxHash, operatorKey: key, votingAddress: votingAddress)
            })
    }

    public func startDissolveTogether(wallet: WalletID, proTxHash: String) async throws(ServiceError)
        -> SharedSessionInfo
    {
        let id = try wallet.kit
        return SharedSessionInfo(
            try await call { client, network throws(DashKitError) in
                try await client.startDissolveTogether(on: network, wallet: id, proTxHash: proTxHash)
            })
    }
}

// MARK: - MasternodeKeychainProviding (IOS-083)

extension MasternodeService: MasternodeKeychainProviding {
    public func keys(wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) async throws(ServiceError)
        -> [MasternodeKeyInfo]
    {
        let id = try wallet.kit
        let kitRole = role.kit
        let rows = try await call { client, network throws(DashKitError) in
            try await client.masternodeKeys(
                on: network, wallet: id, role: kitRole, start: range.lowerBound, count: UInt32(range.count))
        }
        return rows.map {
            MasternodeKeyInfo(
                role: MasternodeKeyRole($0.role), index: $0.index, derivationPath: $0.derivationPath,
                address: $0.address, publicKeyHex: $0.publicKeyHex, legacyPublicKeyHex: $0.legacyPublicKeyHex,
                platformNodeID: $0.platformNodeID,
                usedBy: $0.usedBy.map {
                    MasternodeKeyUsage(proTxHash: $0.proTxHash, service: $0.service, revoked: $0.revoked)
                })
        }
    }

    public func reveal(wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant)
        async throws(ServiceError) -> RevealedMasternodeKey
    {
        try requireGrant(grant, .revealSecret, "revealing a key")
        let id = try wallet.kit
        let kitRole = role.kit
        let grantID = grant.id
        return RevealedMasternodeKey(
            try await call { client, network throws(DashKitError) in
                try await client.revealMasternodeKey(
                    on: network, wallet: id, proTxHash: nil, role: kitRole, index: index, grantID: grantID)
            })
    }
}

// MARK: - TrackedMasternodeManaging (IOS-082)

extension MasternodeService: TrackedMasternodeManaging {
    public func locate(_ query: String) async throws(ServiceError) -> [MasternodeRow] {
        try await call { client, network throws(DashKitError) in
            try await client.locateMasternodes(on: network, query: query)
        }.map(MasternodeRow.init)
    }

    public func tracked() async throws(ServiceError) -> [TrackedMasternode] {
        try await call { client, network throws(DashKitError) in
            try await client.trackedMasternodes(on: network)
        }.map(TrackedMasternode.init)
    }

    public func track(proTxHash: String, label: String?) async throws(ServiceError) -> TrackedMasternode {
        TrackedMasternode(
            try await call { client, network throws(DashKitError) in
                try await client.trackMasternode(on: network, proTxHash: proTxHash, label: label)
            })
    }

    public func untrack(proTxHash: String) async throws(ServiceError) -> Bool {
        try await call { client, network throws(DashKitError) in
            try await client.untrackMasternode(on: network, proTxHash: proTxHash)
        }
    }

    public func setLabel(_ label: String?, proTxHash: String) async throws(ServiceError) {
        try await call { client, network throws(DashKitError) in
            try await client.setTrackedMasternodeLabel(on: network, proTxHash: proTxHash, label: label)
        }
    }

    public func attach(_ key: any SecretBuffer, role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant)
        async throws(ServiceError)
    {
        try requireGrant(grant, .masternodeOperation, "attaching a key")
        let bytes = secretBytes(key)
        let kitRole = role.kit
        let grantID = grant.id
        try await call { client, network throws(DashKitError) in
            try await client.attachMasternodeKey(
                on: network, proTxHash: proTxHash, role: kitRole, key: bytes, grantID: grantID)
        }
    }

    public func detach(role: MasternodeKeyRole, proTxHash: String) async throws(ServiceError) {
        let kitRole = role.kit
        try await call { client, network throws(DashKitError) in
            try await client.detachMasternodeKey(on: network, proTxHash: proTxHash, role: kitRole)
        }
    }

    public func reveal(role: MasternodeKeyRole, proTxHash: String, grant: AuthGrant) async throws(ServiceError)
        -> RevealedMasternodeKey
    {
        try requireGrant(grant, .revealSecret, "revealing a key")
        let kitRole = role.kit
        let grantID = grant.id
        return RevealedMasternodeKey(
            try await call { client, network throws(DashKitError) in
                try await client.revealMasternodeKey(
                    on: network, wallet: nil, proTxHash: proTxHash, role: kitRole, index: 0, grantID: grantID)
            })
    }
}

// MARK: - EvonodeServicing (IOS-080, IOS-081)

extension MasternodeService: EvonodeServicing {
    public func status(proTxHash: String) async throws(ServiceError) -> EvonodePlatformStatus {
        let s = try await call { client, network throws(DashKitError) in
            try await client.evonodeStatus(on: network, proTxHash: proTxHash)
        }
        return EvonodePlatformStatus(
            proTxHash: s.proTxHash, claimableCredits: s.claimableCredits,
            epochProposedBlocks: s.epochProposedBlocks.map(Int.init), epochIndex: s.epochIndex.map(Int.init))
    }

    public func withdraw(proTxHash: String, credits: UInt64, destination: CreditWithdrawalDestination, grant: AuthGrant)
        async throws(ServiceError) -> String
    {
        try requireGrant(grant, .masternodeOperation, "withdrawing credits")
        let kitDestination: MNCreditDestination =
            switch destination {
            case .payoutAddress: .payoutAddress
            case .address(let a): .address(a)
            }
        let grantID = grant.id
        return try await call { client, network throws(DashKitError) in
            try await client.withdrawEvonodeCredits(
                on: network, proTxHash: proTxHash, amount: credits, destination: kitDestination, grantID: grantID)
        }
    }
}

// MARK: - Conversions

extension Amount {
    fileprivate static func duffs(_ value: UInt64) -> Amount { Amount(duffs: Int64(clamping: value)) }
}

extension MasternodeType {
    init(_ kit: MNType) { self = kit == .evo ? .evo : .regular }
    var kit: MNType { self == .evo ? .evo : .regular }
}

extension MasternodeTypeFilter {
    var kit: MNTypeFilter {
        switch self {
        case .all: .all
        case .regular: .regular
        case .evo: .evo
        case .shared: .shared
        }
    }
}

extension MasternodeKeyRole {
    init(_ kit: MNKeyRole) {
        switch kit {
        case .owner: self = .owner
        case .voting: self = .voting
        case .operator: self = .operator
        case .platformNode: self = .platformNode
        case .ownerPayout: self = .ownerPayout
        case .operatorPayout: self = .operatorPayout
        }
    }

    var kit: MNKeyRole {
        switch self {
        case .owner: .owner
        case .voting: .voting
        case .operator: .operator
        case .platformNode: .platformNode
        case .ownerPayout: .ownerPayout
        case .operatorPayout: .operatorPayout
        }
    }
}

extension OwnedRole {
    init(_ kit: MNOwnedRole) {
        switch kit {
        case .collateral: self = .collateral
        case .owner: self = .owner
        case .voting: self = .voting
        case .operator: self = .operator
        case .payout: self = .payout
        case .operatorPayout: self = .operatorPayout
        case .platformNode: self = .platformNode
        case .shareOwner: self = .shareOwner
        case .shareRefund: self = .shareRefund
        case .tracked: self = .tracked
        }
    }
}

extension MasternodeRow {
    init(_ kit: MNRow) {
        let status: MasternodeListStatus =
            switch kit.status {
            case .active(let since): .active(sinceHeight: since)
            case .banned(let since): .banned(sinceHeight: since)
            case .retired: .retired
            case .unknown: .unknown
            }
        let shared: SharedHolding? =
            if let held = kit.sharedHeld, let total = kit.sharedTotal {
                SharedHolding(heldShares: Int(held), totalShares: Int(total))
            } else {
                nil
            }
        self.init(
            proTxHash: kit.proTxHash, service: kit.service, type: MasternodeType(kit.type), shared: shared,
            status: status, poseScore: kit.poseScore.map(Int.init), registeredHeight: kit.registeredHeight,
            lastPaidHeight: kit.lastPaidHeight, nextPaymentHeight: kit.nextPaymentHeight,
            operatorReward: kit.operatorRewardX100.map {
                OperatorReward(percentX100: Int($0), payoutAddress: kit.operatorPayoutAddress)
            }, collateral: kit.collateral.map(OutPoint.init), collateralAddress: kit.collateralAddress,
            ownerAddress: kit.ownerAddress, votingAddress: kit.votingAddress, payoutAddresses: kit.payoutAddresses,
            operatorPublicKey: kit.operatorPublicKey, platformNodeID: kit.platformNodeID,
            ownedRoles: Set(kit.ownedRoles.map(OwnedRole.init)), label: kit.label)
    }
}

extension CollateralRefusal {
    init(_ kit: MNCollateralRefusal) {
        switch kit {
        case .wrongAmount: self = .wrongAmount
        case .unconfirmed: self = .unconfirmed
        case .notP2PKH: self = .notP2PKH
        case .locked: self = .locked
        case .alreadyCollateral: self = .alreadyCollateral
        case .notFound: self = .notFound
        }
    }
}

extension OperatorKeyChoice {
    var kit: MNOperatorKeyChoice {
        switch self {
        case .generate: .generate
        case .existing(let key): .existing(publicKeyHex: key)
        }
    }
}

extension PlatformFields {
    init(_ kit: MNPlatformFields) {
        self.init(nodeIDHex: kit.nodeIDHex, p2pAddresses: kit.p2pAddresses, httpsAddresses: kit.httpsAddresses)
    }

    var kit: MNPlatformFields {
        MNPlatformFields(nodeIDHex: nodeIDHex, p2pAddresses: p2pAddresses, httpsAddresses: httpsAddresses)
    }
}

extension FeeSourceChoice {
    var kit: MNFeeSource {
        switch self {
        case .automatic: .automatic
        case .address(let a): .address(a)
        }
    }
}

extension RevocationReason {
    var kit: MNRevocationReason {
        switch self {
        case .notSpecified: .notSpecified
        case .terminationOfService: .terminationOfService
        case .compromisedKeys: .compromisedKeys
        case .changeOfKeys: .changeOfKeys
        }
    }
}

extension RegistrationSummary {
    init(_ kit: MNRegistrationSummary) {
        self.init(
            type: MasternodeType(kit.type), proTxHash: kit.proTxHash, collateral: OutPoint(kit.collateral),
            collateralAddress: kit.collateralAddress, ownerAddress: kit.ownerAddress,
            votingAddress: kit.votingAddress, payoutAddress: kit.payoutAddress,
            operatorPublicKey: kit.operatorPublicKey, operatorRewardX100: Int(kit.operatorRewardX100),
            serviceAddresses: kit.serviceAddresses, platform: kit.platform.map(PlatformFields.init),
            fee: .duffs(kit.fee), totalSpent: .duffs(kit.totalSpent),
            operatorSecretRequired: kit.operatorSecretRequired, collateralSignMessage: kit.collateralSignMessage)
    }
}

extension ProviderTransactionSummary {
    init(_ kit: MNProviderTxSummary) {
        let kind: ProviderTransactionKind =
            switch kit.kind {
            case .updateService: .updateService
            case .updateRegistrar: .updateRegistrar
            case .revoke: .revoke
            case .updateShare: .updateShare
            case .updateSharedRegistrar: .updateSharedRegistrar
            case .dissolve: .dissolve
            }
        self.init(
            kind: kind, proTxHash: kit.proTxHash, txid: kit.txid, fee: .duffs(kit.fee),
            penalty: kit.penalty.map(Amount.duffs), bansMasternode: kit.bansMasternode)
    }
}

extension SharedSessionInfo {
    init(_ kit: MNSharedSession) {
        let purpose: SharedSessionPurpose =
            switch kit.purpose {
            case .register: .register
            case .rotateKeys: .rotateKeys
            case .dissolveTogether: .dissolveTogether
            }
        let stage: SharedStage =
            switch kit.stage {
            case .invitation: .invitation
            case .details: .details
            case .lockedTerms: .lockedTerms
            case .approvals: .approvals
            case .signingRequest: .signingRequest
            case .signedContributions: .signedContributions
            case .broadcast: .broadcast
            case .completed: .completed
            case .abandoned: .abandoned
            }
        self.init(
            id: kit.sessionID, sessionCode: kit.sessionCode, purpose: purpose,
            role: kit.role == .coordinator ? .coordinator : .participant, stage: stage, revision: Int(kit.revision),
            fingerprint: kit.fingerprint, wallet: WalletID(kit.wallet), myShareIndexes: kit.myShareIndexes.map(Int.init),
            reservedInputs: kit.reservedInputs.map(OutPoint.init), reservedCoinSpent: kit.reservedCoinSpent,
            proTxHash: kit.proTxHash)
    }
}

extension TrackedMasternode {
    init(_ kit: MNTracked) {
        self.init(
            row: MasternodeRow(kit.row), label: kit.label, attachedRoles: Set(kit.attachedRoles.map(MasternodeKeyRole.init)),
            capabilities: TrackedCapabilities(
                canWithdraw: kit.capabilities.canWithdraw, canUpdateService: kit.capabilities.canUpdateService,
                canUpdateRegistrar: kit.capabilities.canUpdateRegistrar, canVote: kit.capabilities.canVote))
    }
}

extension RevealedMasternodeKey {
    init(_ kit: MNRevealedKey) {
        self.init(privateKeyHex: kit.privateKeyHex, wif: kit.wif, tenderdashKey: kit.tenderdashKey)
    }
}
