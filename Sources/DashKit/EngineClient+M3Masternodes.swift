import DashWalletCore
import Foundation

// `EngineClient` wrappers of the M3 masternode calls (docs/contracts/m3-engine.md
// §2.3–2.5, owner R3) and the DashKit value types they return. Amounts are
// duffs, proTxHashes and txids display-order hex; `nil` = not known to an SPV
// wallet. Errors keep the engine's `masternode.*` codes (DashKitError+M3.swift).
// Secrets cross as `SecretBytes`; the FFI's `Data` copies are wiped.

// MARK: - List

public enum MNType: Sendable, Hashable { case regular, evo }

public enum MNTypeFilter: Sendable, Hashable { case all, regular, evo, shared }

public enum MNKeyRole: Sendable, Hashable { case owner, voting, `operator`, platformNode, ownerPayout, operatorPayout }

public enum MNOwnedRole: Sendable, Hashable {
    case collateral, owner, voting, `operator`, payout, operatorPayout, platformNode, shareOwner, shareRefund, tracked
}

public enum MNListStatus: Sendable, Hashable {
    case active(sinceHeight: UInt32?)
    case banned(sinceHeight: UInt32?)
    case retired
    case unknown
}

public struct MNRow: Sendable, Hashable {
    public let proTxHash: String
    public let service: String?
    public let type: MNType
    /// `(held, total)` shares of a v24 shared masternode.
    public let sharedHeld: UInt32?
    public let sharedTotal: UInt32?
    public let status: MNListStatus
    public let poseScore: UInt32?
    public let registeredHeight: UInt32?
    public let lastPaidHeight: UInt32?
    public let nextPaymentHeight: UInt32?
    public let operatorRewardX100: UInt16?
    public let operatorPayoutAddress: String?
    public let collateral: OutPoint?
    public let collateralAddress: String?
    public let ownerAddress: String?
    public let votingAddress: String
    public let payoutAddresses: [String]
    public let operatorPublicKey: String
    public let platformNodeID: String?
    public let ownedRoles: [MNOwnedRole]
    public let label: String?
}

public struct MNQuery: Sendable, Hashable {
    public var typeFilter: MNTypeFilter
    public var text: String?
    public var ownedOnly: Bool
    public var hideBanned: Bool

    public init(typeFilter: MNTypeFilter, text: String?, ownedOnly: Bool, hideBanned: Bool) {
        self.typeFilter = typeFilter
        self.text = text
        self.ownedOnly = ownedOnly
        self.hideBanned = hideBanned
    }
}

public struct MNListState: Sendable, Hashable {
    public let available: Bool
    public let height: UInt32?
    public let total: UInt32
    public let enabled: UInt32
    public let evoTotal: UInt32
    public let evoEnabled: UInt32
    public let syncing: Bool
}

public struct MNShare: Sendable, Hashable {
    public let amount: UInt64
    public let ownerAddress: String
    public let payoutAddress: String
    public let refundAddress: String
    public let mine: Bool
}

public struct MNDetail: Sendable, Hashable {
    public let row: MNRow
    public let consecutivePayments: UInt32?
    public let poseBanHeight: UInt32?
    public let poseRevivedHeight: UInt32?
    public let networkAddresses: [String]
    public let platformP2PAddresses: [String]
    public let platformHTTPSAddresses: [String]
    public let shares: [MNShare]
    public let earlyPeriodEnd: UInt32?
    public let earlyExitPenalty: UInt64?
    public let hasStandbyDissolution: Bool
    public let revocationReason: UInt16?
    public let walletTransactions: UInt32
}

public struct MNNetworkDefaults: Sendable, Hashable {
    public let coreP2PPort: UInt16
    public let platformP2PPort: UInt16
    public let platformHTTPSPort: UInt16
    public let masternodeCollateral: UInt64
    public let evonodeCollateral: UInt64
    public let minShares: UInt32
    public let maxShares: UInt32
    public let minShareAmount: UInt64
    public let maxEarlyPeriodBlocks: UInt32
    public let maxEnvelopeBytes: UInt64
    public let maxOperatorRewardX100: UInt16
}

// MARK: - Provider transactions

public enum MNCollateralChoice: Sendable, Hashable {
    case fundNew
    case existingUTXO(OutPoint)
    case external(OutPoint)
}

public enum MNOperatorKeyChoice: Sendable, Hashable {
    case generate
    case existing(publicKeyHex: String)
}

public struct MNPlatformFields: Sendable, Hashable {
    public var nodeIDHex: String
    public var p2pAddresses: [String]
    public var httpsAddresses: [String]

    public init(nodeIDHex: String, p2pAddresses: [String], httpsAddresses: [String]) {
        self.nodeIDHex = nodeIDHex
        self.p2pAddresses = p2pAddresses
        self.httpsAddresses = httpsAddresses
    }
}

public enum MNFeeSource: Sendable, Hashable {
    case automatic
    case address(String)
}

public struct MNRegistrationRequest: Sendable, Hashable {
    public var wallet: WalletID
    public var type: MNType
    public var collateral: MNCollateralChoice
    public var serviceAddresses: [String]
    public var ownerAddress: String?
    public var votingAddress: String?
    public var operatorKey: MNOperatorKeyChoice
    public var payoutAddress: String
    public var operatorRewardX100: UInt16
    public var platform: MNPlatformFields?
    public var feeSource: MNFeeSource

    public init(
        wallet: WalletID, type: MNType, collateral: MNCollateralChoice, serviceAddresses: [String],
        ownerAddress: String?, votingAddress: String?, operatorKey: MNOperatorKeyChoice, payoutAddress: String,
        operatorRewardX100: UInt16, platform: MNPlatformFields?, feeSource: MNFeeSource
    ) {
        self.wallet = wallet
        self.type = type
        self.collateral = collateral
        self.serviceAddresses = serviceAddresses
        self.ownerAddress = ownerAddress
        self.votingAddress = votingAddress
        self.operatorKey = operatorKey
        self.payoutAddress = payoutAddress
        self.operatorRewardX100 = operatorRewardX100
        self.platform = platform
        self.feeSource = feeSource
    }
}

public enum MNCollateralRefusal: Sendable, Hashable {
    case wrongAmount, unconfirmed, notP2PKH, locked, alreadyCollateral, notFound
}

public struct MNCollateralCandidate: Sendable, Hashable {
    public let outpoint: OutPoint
    public let address: String
    public let amount: UInt64
    public let confirmations: UInt32
    public let refusal: MNCollateralRefusal?
}

public struct MNFeeSourceCandidate: Sendable, Hashable {
    public let address: String
    public let spendable: UInt64
    public let label: String?
}

public struct MNRegistrationSummary: Sendable, Hashable {
    public let type: MNType
    public let proTxHash: String
    public let collateral: OutPoint
    public let collateralAddress: String
    public let ownerAddress: String
    public let votingAddress: String
    public let payoutAddress: String
    public let operatorPublicKey: String
    public let operatorRewardX100: UInt16
    public let serviceAddresses: [String]
    public let platform: MNPlatformFields?
    public let fee: UInt64
    public let totalSpent: UInt64
    public let operatorSecretRequired: Bool
    public let collateralSignMessage: String?
}

/// An engine `PreparedRegistration` (signed, inputs reserved).
public final class MNPreparedRegistration: Sendable {
    let inner: DashWalletCore.PreparedRegistration
    public let summary: MNRegistrationSummary

    init(_ inner: DashWalletCore.PreparedRegistration, summary: MNRegistrationSummary) {
        self.inner = inner
        self.summary = summary
    }

    /// The generated operator secret and its `masternodeblsprivkey=` line.
    public func operatorSecret() throws(DashKitError) -> (secretHex: SecretBytes, configLine: SecretBytes) {
        let inner = inner
        let secret = try mapped { try inner.operatorSecret() }
        var hexData = secret.secretHex
        var lineData = secret.configLine
        return (SecretBytes(consuming: &hexData), SecretBytes(consuming: &lineData))
    }

    public func confirmOperatorSecret(last4: String) throws(DashKitError) -> Bool {
        let inner = inner
        return try mapped { try inner.confirmOperatorSecret(last4: last4) }
    }

    /// Broadcasts; returns the proTxHash.
    public func submit(collateralSignature: String?) async throws(DashKitError) -> String {
        let inner = inner
        return try await mapped { try await inner.submit(collateralSignature: collateralSignature) }
    }

    public func abandon() async throws(DashKitError) {
        let inner = inner
        try await mapped { try await inner.abandon() }
    }
}

public enum MNRevocationReason: Sendable, Hashable { case notSpecified, terminationOfService, compromisedKeys, changeOfKeys }

public enum MNProviderTxKind: Sendable, Hashable {
    case updateService, updateRegistrar, revoke, updateShare, updateSharedRegistrar, dissolve
}

public struct MNProviderTxSummary: Sendable, Hashable {
    public let kind: MNProviderTxKind
    public let proTxHash: String
    public let txid: String
    public let fee: UInt64
    public let penalty: UInt64?
    public let bansMasternode: Bool
}

/// An engine `PreparedProviderTx` (signed, inputs reserved).
public final class MNPreparedProviderTx: Sendable {
    let inner: DashWalletCore.PreparedProviderTx
    public let summary: MNProviderTxSummary

    init(_ inner: DashWalletCore.PreparedProviderTx) throws(DashKitError) {
        self.inner = inner
        summary = MNProviderTxSummary(try mapped { try inner.summary() })
    }

    /// Returns the txid.
    public func broadcast() async throws(DashKitError) -> String {
        let inner = inner
        return try await mapped { try await inner.broadcast() }
    }

    public func abandon() async throws(DashKitError) {
        let inner = inner
        try await mapped { try await inner.abandon() }
    }
}

public struct MNStandbyDissolution: Sendable, Hashable {
    public let proTxHash: String
    public let transactionsHex: [String]
    public let suggestedFileName: String
}

// MARK: - Shared masternodes

public struct MNSharedShareTerms: Sendable, Hashable {
    public var amount: UInt64
    public var label: String?
    public var mine: Bool

    public init(amount: UInt64, label: String?, mine: Bool) {
        self.amount = amount
        self.label = label
        self.mine = mine
    }
}

public struct MNSharedTerms: Sendable, Hashable {
    public var shares: [MNSharedShareTerms]
    public var earlyPeriodBlocks: UInt32
    public var earlyExitPenalty: UInt64
    public var serviceAddresses: [String]
    public var operatorKey: MNOperatorKeyChoice
    public var operatorRewardX100: UInt16

    public init(
        shares: [MNSharedShareTerms], earlyPeriodBlocks: UInt32, earlyExitPenalty: UInt64,
        serviceAddresses: [String], operatorKey: MNOperatorKeyChoice, operatorRewardX100: UInt16
    ) {
        self.shares = shares
        self.earlyPeriodBlocks = earlyPeriodBlocks
        self.earlyExitPenalty = earlyExitPenalty
        self.serviceAddresses = serviceAddresses
        self.operatorKey = operatorKey
        self.operatorRewardX100 = operatorRewardX100
    }
}

public enum MNSharedRole: Sendable, Hashable { case coordinator, participant }

public enum MNSharedStage: Sendable, Hashable {
    case invitation, details, lockedTerms, approvals, signingRequest, signedContributions, broadcast, completed
    case abandoned
}

public enum MNSharedPurpose: Sendable, Hashable { case register, rotateKeys, dissolveTogether }

public struct MNSharedSession: Sendable, Hashable {
    public let sessionID: String
    public let sessionCode: String
    public let purpose: MNSharedPurpose
    public let role: MNSharedRole
    public let stage: MNSharedStage
    public let revision: UInt32
    public let fingerprint: String
    public let wallet: WalletID
    public let myShareIndexes: [UInt32]
    public let reservedInputs: [OutPoint]
    public let reservedCoinSpent: Bool
    public let proTxHash: String?
}

public struct MNSharedEnvelope: Sendable, Hashable {
    public let json: String
    public let fingerprint: String
    public let suggestedFileName: String
}

public enum MNSharedMessageKind: Sendable, Hashable {
    case envelope(MNSharedSession)
    case standbyDissolution(proTxHash: String?, transactionsHex: [String])
}

public struct MNShareContribution: Sendable, Hashable {
    public var shareIndex: UInt32
    public var inputs: [OutPoint]
    public var ownerAddress: String?
    public var payoutAddress: String?
    public var refundAddress: String?

    public init(
        shareIndex: UInt32, inputs: [OutPoint], ownerAddress: String?, payoutAddress: String?, refundAddress: String?
    ) {
        self.shareIndex = shareIndex
        self.inputs = inputs
        self.ownerAddress = ownerAddress
        self.payoutAddress = payoutAddress
        self.refundAddress = refundAddress
    }
}

// MARK: - Keychain, tracked masternodes, evonodes

public struct MNKeyUsage: Sendable, Hashable {
    public let proTxHash: String
    public let service: String?
    public let revoked: Bool
}

public struct MNKeyInfo: Sendable, Hashable {
    public let role: MNKeyRole
    public let index: UInt32
    public let derivationPath: String
    public let address: String?
    public let publicKeyHex: String
    public let legacyPublicKeyHex: String?
    public let platformNodeID: String?
    public let usedBy: [MNKeyUsage]
}

public struct MNRevealedKey: Sendable {
    public let privateKeyHex: SecretBytes
    public let wif: SecretBytes?
    public let tenderdashKey: SecretBytes?
}

public struct MNTrackedCapabilities: Sendable, Hashable {
    public let canWithdraw: Bool
    public let canUpdateService: Bool
    public let canUpdateRegistrar: Bool
    public let canVote: Bool
}

public struct MNTracked: Sendable, Hashable {
    public let row: MNRow
    public let label: String?
    public let attachedRoles: [MNKeyRole]
    public let capabilities: MNTrackedCapabilities
}

public struct MNEvonodeStatus: Sendable, Hashable {
    public let proTxHash: String
    public let claimableCredits: UInt64?
    public let epochProposedBlocks: UInt32?
    public let epochIndex: UInt32?
}

public enum MNCreditDestination: Sendable, Hashable {
    case payoutAddress
    case address(String)
}

// MARK: - FFI conversions

extension MNType {
    init(_ t: DashWalletCore.MasternodeType) {
        self = t == .evo ? .evo : .regular
    }

    var ffi: DashWalletCore.MasternodeType { self == .evo ? .evo : .regular }
}

extension MNKeyRole {
    init(_ r: DashWalletCore.MasternodeKeyRole) {
        switch r {
        case .owner: self = .owner
        case .voting: self = .voting
        case .operator: self = .operator
        case .platformNode: self = .platformNode
        case .ownerPayout: self = .ownerPayout
        case .operatorPayout: self = .operatorPayout
        }
    }

    var ffi: DashWalletCore.MasternodeKeyRole {
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

extension MNOwnedRole {
    init(_ r: DashWalletCore.OwnedRole) {
        switch r {
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

extension MNRow {
    init(_ r: DashWalletCore.MasternodeRow) {
        let status: MNListStatus =
            switch r.status {
            case .active(let since): .active(sinceHeight: since)
            case .banned(let since): .banned(sinceHeight: since)
            case .retired: .retired
            case .unknown: .unknown
            }
        self.init(
            proTxHash: r.proTxHash, service: r.service, type: MNType(r.nodeType), sharedHeld: r.shared?.heldShares,
            sharedTotal: r.shared?.totalShares, status: status, poseScore: r.poseScore,
            registeredHeight: r.registeredHeight, lastPaidHeight: r.lastPaidHeight,
            nextPaymentHeight: r.nextPaymentHeight, operatorRewardX100: r.operatorReward?.percentX100,
            operatorPayoutAddress: r.operatorReward?.payoutAddress, collateral: r.collateral.map(OutPoint.init),
            collateralAddress: r.collateralAddress, ownerAddress: r.ownerAddress, votingAddress: r.votingAddress,
            payoutAddresses: r.payoutAddresses, operatorPublicKey: r.operatorPublicKey,
            platformNodeID: r.platformNodeId, ownedRoles: r.ownedRoles.map(MNOwnedRole.init), label: r.label)
    }
}

extension MNPlatformFields {
    init(_ p: DashWalletCore.PlatformFields) {
        self.init(nodeIDHex: p.nodeIdHex, p2pAddresses: p.p2pAddresses, httpsAddresses: p.httpsAddresses)
    }

    var ffi: DashWalletCore.PlatformFields {
        DashWalletCore.PlatformFields(nodeIdHex: nodeIDHex, p2pAddresses: p2pAddresses, httpsAddresses: httpsAddresses)
    }
}

extension MNFeeSource {
    var ffi: DashWalletCore.FeeSourceChoice {
        switch self {
        case .automatic: .automatic
        case .address(let a): .address(address: a)
        }
    }
}

extension MNOperatorKeyChoice {
    var ffi: DashWalletCore.OperatorKeyChoice {
        switch self {
        case .generate: .generate
        case .existing(let key): .existing(publicKeyHex: key)
        }
    }
}

extension MNProviderTxSummary {
    init(_ s: DashWalletCore.ProviderTxSummary) {
        let kind: MNProviderTxKind =
            switch s.kind {
            case .updateService: .updateService
            case .updateRegistrar: .updateRegistrar
            case .revoke: .revoke
            case .updateShare: .updateShare
            case .updateSharedRegistrar: .updateSharedRegistrar
            case .dissolve: .dissolve
            }
        self.init(
            kind: kind, proTxHash: s.proTxHash, txid: s.txid, fee: s.fee, penalty: s.penalty,
            bansMasternode: s.bansMasternode)
    }
}

extension MNSharedSession {
    init(_ s: DashWalletCore.SharedSessionInfo) throws(DashKitError) {
        let purpose: MNSharedPurpose =
            switch s.purpose {
            case .register: .register
            case .rotateKeys: .rotateKeys
            case .dissolveTogether: .dissolveTogether
            }
        let stage: MNSharedStage =
            switch s.stage {
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
            sessionID: s.sessionId, sessionCode: s.sessionCode, purpose: purpose,
            role: s.role == .coordinator ? .coordinator : .participant, stage: stage, revision: s.revision,
            fingerprint: s.fingerprint, wallet: try .engine(s.walletId), myShareIndexes: s.myShareIndexes,
            reservedInputs: s.reservedInputs.map(OutPoint.init), reservedCoinSpent: s.reservedCoinSpent,
            proTxHash: s.proTxHash)
    }
}

extension MNTracked {
    init(_ t: DashWalletCore.TrackedMasternode) {
        self.init(
            row: MNRow(t.row), label: t.label, attachedRoles: t.attachedRoles.map(MNKeyRole.init),
            capabilities: MNTrackedCapabilities(
                canWithdraw: t.capabilities.canWithdraw, canUpdateService: t.capabilities.canUpdateService,
                canUpdateRegistrar: t.capabilities.canUpdateRegistrar, canVote: t.capabilities.canVote))
    }
}

/// Moves engine `Data` secrets into `SecretBytes` and wipes the copies.
private func revealed(_ r: DashWalletCore.RevealedMasternodeKey) -> MNRevealedKey {
    func take(_ data: Data?) -> SecretBytes? {
        guard var data else { return nil }
        return SecretBytes(consuming: &data)
    }
    var key = r.privateKeyHex
    return MNRevealedKey(
        privateKeyHex: SecretBytes(consuming: &key), wif: take(r.wif), tenderdashKey: take(r.tenderdashKey))
}

// MARK: - Calls

extension EngineClient {
    /// `masternode_network_defaults` (constants; no session needed).
    public nonisolated func masternodeDefaults(for network: DashNetwork) -> MNNetworkDefaults {
        let d = DashWalletCore.masternodeNetworkDefaults(network: network.ffi)
        return MNNetworkDefaults(
            coreP2PPort: d.coreP2pPort, platformP2PPort: d.platformP2pPort, platformHTTPSPort: d.platformHttpsPort,
            masternodeCollateral: d.masternodeCollateral, evonodeCollateral: d.evonodeCollateral,
            minShares: d.minShares, maxShares: d.maxShares, minShareAmount: d.minShareAmount,
            maxEarlyPeriodBlocks: d.maxEarlyPeriodBlocks, maxEnvelopeBytes: d.maxEnvelopeBytes,
            maxOperatorRewardX100: d.maxOperatorRewardX100)
    }

    public func masternodeListState(on network: DashNetwork) throws(DashKitError) -> MNListState {
        let session = try session(network)
        let s = try mapped { try session.masternodeListState() }
        return MNListState(
            available: s.available, height: s.height, total: s.total, enabled: s.enabled, evoTotal: s.evoTotal,
            evoEnabled: s.evoEnabled, syncing: s.syncing)
    }

    public func masternodes(on network: DashNetwork, query: MNQuery) async throws(DashKitError) -> [MNRow] {
        let session = try session(network)
        let filter: DashWalletCore.MasternodeTypeFilter =
            switch query.typeFilter {
            case .all: .all
            case .regular: .regular
            case .evo: .evo
            case .shared: .shared
            }
        let ffi = DashWalletCore.MasternodeQuery(
            typeFilter: filter, text: query.text, ownedOnly: query.ownedOnly, hideBanned: query.hideBanned)
        return try await mapped { try await session.masternodes(query: ffi) }.map(MNRow.init)
    }

    public func masternodeDetail(on network: DashNetwork, proTxHash: String) async throws(DashKitError) -> MNDetail {
        let session = try session(network)
        let d = try await mapped { try await session.masternodeDetail(proTxHash: proTxHash) }
        return MNDetail(
            row: MNRow(d.row), consecutivePayments: d.consecutivePayments, poseBanHeight: d.poseBanHeight,
            poseRevivedHeight: d.poseRevivedHeight, networkAddresses: d.networkAddresses,
            platformP2PAddresses: d.platformP2pAddresses, platformHTTPSAddresses: d.platformHttpsAddresses,
            shares: d.shares.map {
                MNShare(
                    amount: $0.amount, ownerAddress: $0.ownerAddress, payoutAddress: $0.payoutAddress,
                    refundAddress: $0.refundAddress, mine: $0.mine)
            }, earlyPeriodEnd: d.earlyPeriodEnd, earlyExitPenalty: d.earlyExitPenalty,
            hasStandbyDissolution: d.hasStandbyDissolution, revocationReason: d.revocationReason,
            walletTransactions: d.walletTransactions)
    }

    public func collateralCandidates(on network: DashNetwork, wallet: WalletID, type: MNType)
        async throws(DashKitError) -> [MNCollateralCandidate]
    {
        let session = try session(network)
        let rows = try await mapped { try await session.collateralCandidates(walletId: wallet.hex, nodeType: type.ffi) }
        return rows.map {
            let refusal: MNCollateralRefusal? =
                switch $0.refusal {
                case nil: nil
                case .wrongAmount: .wrongAmount
                case .unconfirmed: .unconfirmed
                case .notP2pkh: .notP2PKH
                case .locked: .locked
                case .alreadyCollateral: .alreadyCollateral
                case .notFound: .notFound
                }
            return MNCollateralCandidate(
                outpoint: OutPoint($0.outpoint), address: $0.address, amount: $0.amount,
                confirmations: $0.confirmations, refusal: refusal)
        }
    }

    public func feeSourceCandidates(on network: DashNetwork, wallet: WalletID)
        async throws(DashKitError) -> [MNFeeSourceCandidate]
    {
        let session = try session(network)
        return try await mapped { try await session.feeSourceCandidates(walletId: wallet.hex) }.map {
            MNFeeSourceCandidate(address: $0.address, spendable: $0.spendable, label: $0.label)
        }
    }

    public func prepareRegistration(on network: DashNetwork, _ request: MNRegistrationRequest, grantID: String)
        async throws(DashKitError) -> MNPreparedRegistration
    {
        let session = try session(network)
        let collateral: DashWalletCore.CollateralChoice =
            switch request.collateral {
            case .fundNew: .fundNew
            case .existingUTXO(let o): .existingUtxo(outpoint: o.ffi)
            case .external(let o): .external(outpoint: o.ffi)
            }
        let ffi = DashWalletCore.RegistrationRequest(
            walletId: request.wallet.hex, nodeType: request.type.ffi, collateral: collateral,
            serviceAddresses: request.serviceAddresses, ownerAddress: request.ownerAddress,
            votingAddress: request.votingAddress, operatorKey: request.operatorKey.ffi,
            payoutAddress: request.payoutAddress, operatorRewardX100: request.operatorRewardX100,
            platform: request.platform?.ffi, feeSource: request.feeSource.ffi)
        let prepared = try await mapped { try await session.prepareRegistration(request: ffi, grantId: grantID) }
        let s = try mapped { try prepared.summary() }
        let summary = MNRegistrationSummary(
            type: MNType(s.nodeType), proTxHash: s.proTxHash, collateral: OutPoint(s.collateral),
            collateralAddress: s.collateralAddress, ownerAddress: s.ownerAddress, votingAddress: s.votingAddress,
            payoutAddress: s.payoutAddress, operatorPublicKey: s.operatorPublicKey,
            operatorRewardX100: s.operatorRewardX100, serviceAddresses: s.serviceAddresses,
            platform: s.platform.map(MNPlatformFields.init), fee: s.fee, totalSpent: s.totalSpent,
            operatorSecretRequired: s.operatorSecretRequired, collateralSignMessage: s.collateralSignMessage)
        return MNPreparedRegistration(prepared, summary: summary)
    }

    /// Update Service (ProUpServTx). `operatorSecret`: the typed secret, or
    /// `nil` for a wallet-derived or attached key.
    public func prepareUpdateService(
        on network: DashNetwork, proTxHash: String, serviceAddresses: [String], operatorSecret: SecretBytes?,
        platform: MNPlatformFields?, operatorPayoutAddress: String?, feeSource: MNFeeSource, feeWallet: WalletID,
        grantID: String
    ) async throws(DashKitError) -> MNPreparedProviderTx {
        let session = try session(network)
        let prepared: DashWalletCore.PreparedProviderTx
        if let operatorSecret {
            prepared = try await mapped {
                try await operatorSecret.withTemporaryData { data in
                    try await session.prepareUpdateService(
                        request: DashWalletCore.UpdateServiceRequest(
                            proTxHash: proTxHash, serviceAddresses: serviceAddresses, operatorSecret: data,
                            platform: platform?.ffi, operatorPayoutAddress: operatorPayoutAddress,
                            feeSource: feeSource.ffi, feeWalletId: feeWallet.hex),
                        grantId: grantID)
                }
            }
        } else {
            prepared = try await mapped {
                try await session.prepareUpdateService(
                    request: DashWalletCore.UpdateServiceRequest(
                        proTxHash: proTxHash, serviceAddresses: serviceAddresses, operatorSecret: nil,
                        platform: platform?.ffi, operatorPayoutAddress: operatorPayoutAddress,
                        feeSource: feeSource.ffi, feeWalletId: feeWallet.hex),
                    grantId: grantID)
            }
        }
        return try MNPreparedProviderTx(prepared)
    }

    public func prepareUpdateRegistrar(
        on network: DashNetwork, proTxHash: String, operatorPublicKey: String?, votingAddress: String?,
        payoutAddress: String?, feeSource: MNFeeSource, feeWallet: WalletID, grantID: String
    ) async throws(DashKitError) -> MNPreparedProviderTx {
        let session = try session(network)
        let request = DashWalletCore.UpdateRegistrarRequest(
            proTxHash: proTxHash, operatorPublicKey: operatorPublicKey, votingAddress: votingAddress,
            payoutAddress: payoutAddress, feeSource: feeSource.ffi, feeWalletId: feeWallet.hex)
        return try MNPreparedProviderTx(
            try await mapped { try await session.prepareUpdateRegistrar(request: request, grantId: grantID) })
    }

    public func prepareRevoke(
        on network: DashNetwork, proTxHash: String, operatorSecret: SecretBytes?, reason: MNRevocationReason,
        feeSource: MNFeeSource, feeWallet: WalletID, grantID: String
    ) async throws(DashKitError) -> MNPreparedProviderTx {
        let session = try session(network)
        let ffiReason: DashWalletCore.RevocationReason =
            switch reason {
            case .notSpecified: .notSpecified
            case .terminationOfService: .terminationOfService
            case .compromisedKeys: .compromisedKeys
            case .changeOfKeys: .changeOfKeys
            }
        let prepared: DashWalletCore.PreparedProviderTx
        if let operatorSecret {
            prepared = try await mapped {
                try await operatorSecret.withTemporaryData { data in
                    try await session.prepareRevoke(
                        request: DashWalletCore.RevokeRequest(
                            proTxHash: proTxHash, operatorSecret: data, reason: ffiReason, feeSource: feeSource.ffi,
                            feeWalletId: feeWallet.hex),
                        grantId: grantID)
                }
            }
        } else {
            prepared = try await mapped {
                try await session.prepareRevoke(
                    request: DashWalletCore.RevokeRequest(
                        proTxHash: proTxHash, operatorSecret: nil, reason: ffiReason, feeSource: feeSource.ffi,
                        feeWalletId: feeWallet.hex),
                    grantId: grantID)
            }
        }
        return try MNPreparedProviderTx(prepared)
    }

    public func prepareShareRewardUpdate(
        on network: DashNetwork, proTxHash: String, shareIndex: UInt32, payoutAddress: String, feeWallet: WalletID,
        grantID: String
    ) async throws(DashKitError) -> MNPreparedProviderTx {
        let session = try session(network)
        return try MNPreparedProviderTx(
            try await mapped {
                try await session.prepareShareRewardUpdate(
                    proTxHash: proTxHash, shareIndex: shareIndex, payoutAddress: payoutAddress,
                    feeWalletId: feeWallet.hex, grantId: grantID)
            })
    }

    public func prepareDissolveNow(
        on network: DashNetwork, proTxHash: String, acceptPenalty: Bool, feeWallet: WalletID, grantID: String
    ) async throws(DashKitError) -> MNPreparedProviderTx {
        let session = try session(network)
        return try MNPreparedProviderTx(
            try await mapped {
                try await session.prepareDissolveNow(
                    proTxHash: proTxHash, acceptPenalty: acceptPenalty, feeWalletId: feeWallet.hex, grantId: grantID)
            })
    }

    public func createStandbyDissolution(on network: DashNetwork, wallet: WalletID, proTxHash: String, grantID: String)
        async throws(DashKitError) -> MNStandbyDissolution
    {
        let session = try session(network)
        let s = try await mapped {
            try await session.createStandbyDissolution(walletId: wallet.hex, proTxHash: proTxHash, grantId: grantID)
        }
        return MNStandbyDissolution(
            proTxHash: s.proTxHash, transactionsHex: s.transactionsHex, suggestedFileName: s.suggestedFileName)
    }

    public func broadcastStandbyDissolution(on network: DashNetwork, transactionsHex: [String])
        async throws(DashKitError) -> [String]
    {
        let session = try session(network)
        return try await mapped { try await session.broadcastStandbyDissolution(transactionsHex: transactionsHex) }
    }

    // MARK: Shared masternodes

    public func createSharedSession(on network: DashNetwork, wallet: WalletID, terms: MNSharedTerms)
        async throws(DashKitError) -> MNSharedSession
    {
        let session = try session(network)
        let ffi = DashWalletCore.SharedMasternodeTerms(
            shares: terms.shares.map { DashWalletCore.SharedShareTerms(amount: $0.amount, label: $0.label, mine: $0.mine) },
            earlyPeriodBlocks: terms.earlyPeriodBlocks, earlyExitPenalty: terms.earlyExitPenalty,
            serviceAddresses: terms.serviceAddresses, operatorKey: terms.operatorKey.ffi,
            operatorRewardX100: terms.operatorRewardX100)
        return try MNSharedSession(
            try await mapped { try await session.createSharedSession(walletId: wallet.hex, terms: ffi) })
    }

    public func importSharedMessage(on network: DashNetwork, wallet: WalletID, text: String)
        async throws(DashKitError) -> MNSharedMessageKind
    {
        let session = try session(network)
        let kind = try await mapped { try await session.importSharedMessage(walletId: wallet.hex, text: text) }
        switch kind {
        case .envelope(let s): return .envelope(try MNSharedSession(s))
        case .standbyDissolution(let hash, let txs): return .standbyDissolution(proTxHash: hash, transactionsHex: txs)
        }
    }

    public func sharedSessions(on network: DashNetwork, wallet: WalletID) async throws(DashKitError) -> [MNSharedSession] {
        let session = try session(network)
        var out: [MNSharedSession] = []
        for s in try await mapped({ try await session.sharedSessions(walletId: wallet.hex) }) {
            out.append(try MNSharedSession(s))
        }
        return out
    }

    public func sharedSessionMessage(on network: DashNetwork, sessionID: String)
        async throws(DashKitError) -> MNSharedEnvelope
    {
        let session = try session(network)
        let e = try await mapped { try await session.sharedSessionMessage(sessionId: sessionID) }
        return MNSharedEnvelope(json: e.json, fingerprint: e.fingerprint, suggestedFileName: e.suggestedFileName)
    }

    public func sharedSessionContribute(on network: DashNetwork, sessionID: String, contribution: MNShareContribution)
        async throws(DashKitError) -> MNSharedSession
    {
        let session = try session(network)
        let ffi = DashWalletCore.ShareContribution(
            shareIndex: contribution.shareIndex, inputs: contribution.inputs.map(\.ffi),
            ownerAddress: contribution.ownerAddress, payoutAddress: contribution.payoutAddress,
            refundAddress: contribution.refundAddress)
        return try MNSharedSession(
            try await mapped { try await session.sharedSessionContribute(sessionId: sessionID, contribution: ffi) })
    }

    public func sharedSessionApprove(on network: DashNetwork, sessionID: String, grantID: String)
        async throws(DashKitError) -> MNSharedSession
    {
        let session = try session(network)
        return try MNSharedSession(
            try await mapped { try await session.sharedSessionApprove(sessionId: sessionID, grantId: grantID) })
    }

    public func sharedSessionSign(on network: DashNetwork, sessionID: String, grantID: String)
        async throws(DashKitError) -> MNSharedSession
    {
        let session = try session(network)
        return try MNSharedSession(
            try await mapped { try await session.sharedSessionSign(sessionId: sessionID, grantId: grantID) })
    }

    public func sharedSessionBroadcast(on network: DashNetwork, sessionID: String) async throws(DashKitError) -> String {
        let session = try session(network)
        return try await mapped { try await session.sharedSessionBroadcast(sessionId: sessionID) }
    }

    public func sharedSessionAbandon(on network: DashNetwork, sessionID: String) async throws(DashKitError) {
        let session = try session(network)
        try await mapped { try await session.sharedSessionAbandon(sessionId: sessionID) }
    }

    public func startSharedKeyRotation(
        on network: DashNetwork, wallet: WalletID, proTxHash: String, operatorKey: MNOperatorKeyChoice?,
        votingAddress: String?
    ) async throws(DashKitError) -> MNSharedSession {
        let session = try session(network)
        let key = operatorKey?.ffi
        return try MNSharedSession(
            try await mapped {
                try await session.startSharedKeyRotation(
                    walletId: wallet.hex, proTxHash: proTxHash, operatorKey: key, votingAddress: votingAddress)
            })
    }

    public func startDissolveTogether(on network: DashNetwork, wallet: WalletID, proTxHash: String)
        async throws(DashKitError) -> MNSharedSession
    {
        let session = try session(network)
        return try MNSharedSession(
            try await mapped { try await session.startDissolveTogether(walletId: wallet.hex, proTxHash: proTxHash) })
    }

    // MARK: Keychain, tracked masternodes, evonodes

    public func masternodeKeys(on network: DashNetwork, wallet: WalletID, role: MNKeyRole, start: UInt32, count: UInt32)
        async throws(DashKitError) -> [MNKeyInfo]
    {
        let session = try session(network)
        return try await mapped {
            try await session.masternodeKeys(walletId: wallet.hex, role: role.ffi, start: start, count: count)
        }.map {
            MNKeyInfo(
                role: MNKeyRole($0.role), index: $0.index, derivationPath: $0.derivationPath, address: $0.address,
                publicKeyHex: $0.publicKeyHex, legacyPublicKeyHex: $0.legacyPublicKeyHex,
                platformNodeID: $0.platformNodeId,
                usedBy: $0.usedBy.map { MNKeyUsage(proTxHash: $0.proTxHash, service: $0.service, revoked: $0.revoked) })
        }
    }

    /// `Vault.reveal_masternode_key`: exactly one of `wallet` (with `index`)
    /// and `proTxHash`.
    public func revealMasternodeKey(
        on network: DashNetwork, wallet: WalletID?, proTxHash: String?, role: MNKeyRole, index: UInt32, grantID: String
    ) async throws(DashKitError) -> MNRevealedKey {
        let session = try session(network)
        let vault = session.vault()
        let r = try await mapped {
            try await vault.revealMasternodeKey(
                walletId: wallet?.hex, proTxHash: proTxHash, role: role.ffi, index: index, grantId: grantID)
        }
        return revealed(r)
    }

    public func locateMasternodes(on network: DashNetwork, query: String) async throws(DashKitError) -> [MNRow] {
        let session = try session(network)
        return try await mapped { try await session.locateMasternodes(query: query) }.map(MNRow.init)
    }

    public func trackedMasternodes(on network: DashNetwork) async throws(DashKitError) -> [MNTracked] {
        let session = try session(network)
        return try await mapped { try await session.trackedMasternodes() }.map(MNTracked.init)
    }

    public func trackMasternode(on network: DashNetwork, proTxHash: String, label: String?)
        async throws(DashKitError) -> MNTracked
    {
        let session = try session(network)
        return MNTracked(try await mapped { try await session.trackMasternode(proTxHash: proTxHash, label: label) })
    }

    public func untrackMasternode(on network: DashNetwork, proTxHash: String) async throws(DashKitError) -> Bool {
        let session = try session(network)
        return try await mapped { try await session.untrackMasternode(proTxHash: proTxHash) }
    }

    public func setTrackedMasternodeLabel(on network: DashNetwork, proTxHash: String, label: String?)
        async throws(DashKitError)
    {
        let session = try session(network)
        try await mapped { try await session.setTrackedMasternodeLabel(proTxHash: proTxHash, label: label) }
    }

    /// The key bytes go to the engine in a temporary copy that is wiped.
    public func attachMasternodeKey(
        on network: DashNetwork, proTxHash: String, role: MNKeyRole, key: SecretBytes, grantID: String
    ) async throws(DashKitError) {
        let session = try session(network)
        try await mapped {
            try await key.withTemporaryData { data in
                try await session.attachMasternodeKey(proTxHash: proTxHash, role: role.ffi, key: data, grantId: grantID)
            }
        }
    }

    public func detachMasternodeKey(on network: DashNetwork, proTxHash: String, role: MNKeyRole)
        async throws(DashKitError)
    {
        let session = try session(network)
        try await mapped { try await session.detachMasternodeKey(proTxHash: proTxHash, role: role.ffi) }
    }

    public func evonodeStatus(on network: DashNetwork, proTxHash: String) async throws(DashKitError) -> MNEvonodeStatus {
        let session = try session(network)
        let s = try await mapped { try await session.evonodeStatus(proTxHash: proTxHash) }
        return MNEvonodeStatus(
            proTxHash: s.proTxHash, claimableCredits: s.claimableCredits, epochProposedBlocks: s.epochProposedBlocks,
            epochIndex: s.epochIndex)
    }

    public func withdrawEvonodeCredits(
        on network: DashNetwork, proTxHash: String, amount: UInt64, destination: MNCreditDestination, grantID: String
    ) async throws(DashKitError) -> String {
        let session = try session(network)
        let ffi: DashWalletCore.CreditWithdrawalDestination =
            switch destination {
            case .payoutAddress: .payoutAddress
            case .address(let a): .address(address: a)
            }
        return try await mapped {
            try await session.withdrawEvonodeCredits(
                proTxHash: proTxHash, amount: amount, destination: ffi, grantId: grantID)
        }
    }
}
