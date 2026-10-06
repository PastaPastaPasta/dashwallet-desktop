// Governance and masternode rules of the M3 demo (m3-engine.md §2.2–2.5).
import Foundation
import WalletFeatures
import WalletRuntime

extension DemoM3World {
    // MARK: Governance

    var syncState: GovernanceSyncState {
        GovernanceSyncState(
            phase: governanceSyncOn ? .synced : .disabled, objects: governanceSyncOn ? proposals.count : 0,
            votes: governanceSyncOn ? proposals.reduce(0) { $0 + $1.row.yes + $1.row.no + $1.row.abstain } : 0,
            peers: Int(world.sync.connectedPeers), bytesReceived: governanceSyncOn ? 2_400_000 : 0,
            lastSyncedAt: governanceSyncOn ? startedAt : nil)
    }

    func setGovernanceSync(_ on: Bool) {
        guard on != governanceSyncOn else { return }
        governanceSyncOn = on
        governanceChanges.send(())
    }

    private var nextSuperblock: UInt32 {
        let p = parameters
        guard tip >= p.superblockStartHeight else { return p.superblockStartHeight }
        return ((tip - p.superblockStartHeight) / p.superblockCycle + 1) * p.superblockCycle + p.superblockStartHeight
    }

    /// The masternodes the selected wallet votes with, weighted.
    private var walletVoters: [DemoMasternode] { masternodes.filter(\.votingKeyInWallet) }

    private func myVotes(_ hash: String) -> MyVotes? {
        guard !walletVoters.isEmpty else { return nil }
        var yes = 0, no = 0, abstain = 0, unvoted = 0
        for node in walletVoters {
            let weight = node.row.type == .evo ? parameters.evonodeVoteWeight : 1
            switch votes[hash]?[node.row.proTxHash]?.outcome {
            case .yes: yes += weight
            case .no: no += weight
            case .abstain: abstain += weight
            case nil: unvoted += weight
            }
        }
        return MyVotes(yes: yes, no: no, abstain: abstain, unvoted: unvoted)
    }

    private func row(_ proposal: DemoProposal) -> ProposalRow {
        let r = proposal.row
        return ProposalRow(
            hash: r.hash, name: r.name, url: r.url, paymentAddress: r.paymentAddress, paymentAmount: r.paymentAmount,
            start: r.start, end: r.end, status: r.status, collateralConfirmations: r.collateralConfirmations, yes: r.yes,
            no: r.no, abstain: r.abstain, margin: r.margin, myVotes: myVotes(r.hash))
    }

    /// `proposals(query)`: Active needs sync on; Mine works without.
    func proposals(_ query: ProposalQuery) throws(ServiceError) -> [ProposalRow] {
        let filter = query.titleFilter?.lowercased()
        let list: [DemoProposal]
        switch query.source {
        case .active:
            guard governanceSyncOn else { throw .demo(.governanceSyncDisabled) }
            list = proposals.filter { $0.row.status != .lapsed }
        case .mine(let wallet):
            _ = try self.wallet(wallet)
            list = proposals.filter(\.mine)
        }
        let order: [ProposalStatus] = [.funded, .passing, .unfunded, .voting, .confirming, .pending, .failing, .lapsed]
        return list.filter { filter == nil || $0.row.name.lowercased().contains(filter!) }
            .sorted { (order.firstIndex(of: $0.row.status) ?? 0, -$0.row.margin) < (order.firstIndex(of: $1.row.status) ?? 0, -$1.row.margin) }
            .map(row)
    }

    func proposalDetail(_ hash: String) throws(ServiceError) -> ProposalDetail {
        guard let proposal = proposals.first(where: { $0.row.hash == hash }) else {
            throw .demo(.governanceProposalNotFound)
        }
        let r = proposal.row
        let json = """
            {"name":"\(r.name)","payment_address":"\(r.paymentAddress)","payment_amount":\(Self.dash(r.paymentAmount)),\
            "url":"\(r.url)","start_epoch":\(Int(r.start.timeIntervalSince1970)),"end_epoch":\(Int(r.end.timeIntervalSince1970)),"type":1}
            """
        return ProposalDetail(
            row: row(proposal), parentHash: proposal.parentHash, collateralTxid: proposal.collateralTxid,
            createdAt: proposal.createdAt, payments: proposal.payments, rawJSON: json)
    }

    private static func dash(_ amount: Amount) -> String {
        let whole = amount.duffs / Amount.duffsPerDash, fraction = amount.duffs % Amount.duffsPerDash
        guard fraction > 0 else { return "\(whole)" }
        return "\(whole)." + String(format: "%08lld", fraction).replacingOccurrences(of: "0+$", with: "", options: .regularExpression)
    }

    func governanceInfo() -> GovernanceInfo {
        let p = parameters
        let next = nextSuperblock
        let synced = governanceSyncOn
        let rows = proposals.map(\.row)
        let budget = Amount(duffs: 5_000 * Amount.duffsPerDash)
        let allocated = Amount(duffs: rows.filter { $0.status == .funded || $0.status == .passing }
            .reduce(0) { $0 + $1.paymentAmount.duffs })
        let enabled = masternodes.filter { if case .active = $0.row.status { return true } else { return false } }
        let evo = enabled.filter { $0.row.type == .evo }.count
        let weighted = (enabled.count - evo) + evo * p.evonodeVoteWeight
        return GovernanceInfo(
            sync: syncState, superblockCycle: p.superblockCycle, lastSuperblock: next - p.superblockCycle,
            nextSuperblock: next,
            nextSuperblockDate: world.now().addingTimeInterval(Double(next - tip) * DemoLedger.blockSeconds),
            votingCutoff: next - p.maturityWindow, masternodesVoting: synced ? enabled.count - 3 : nil,
            masternodesEligible: synced ? enabled.count - evo : nil, evonodesVoting: synced ? max(0, evo - 1) : nil,
            evonodesEligible: synced ? evo : nil, passingThreshold: synced ? max(p.minQuorum, weighted / 10) : nil,
            masternodesControlled: walletVoters.count,
            votesControlled: walletVoters.reduce(0) { $0 + ($1.row.type == .evo ? p.evonodeVoteWeight : 1) },
            proposalCount: synced ? rows.count : nil, passing: synced ? rows.filter { $0.status == .passing }.count : nil,
            failing: synced ? rows.filter { $0.status == .failing }.count : nil,
            unfunded: synced ? rows.filter { $0.status == .unfunded }.count : nil,
            unfundedShort: synced ? Amount(duffs: 400 * Amount.duffsPerDash) : nil,
            budgetAvailable: synced ? budget : nil, budgetAllocated: synced ? allocated : nil)
    }

    func governanceClock() -> GovernanceClock {
        let p = parameters
        let next = nextSuperblock
        let left = next - tip
        let cutoff = next - p.maturityWindow
        let info = governanceInfo()
        let committed = info.budgetAvailable.flatMap { available in
            info.budgetAllocated.map { Double($0.duffs) / Double(available.duffs) }
        }
        return GovernanceClock(
            cycleProgress: 1 - Double(left) / Double(p.superblockCycle), nextSuperblock: next, blocksToSuperblock: left,
            superblockDate: world.now().addingTimeInterval(Double(left) * DemoLedger.blockSeconds), votingCutoff: cutoff,
            votingOpen: tip < cutoff, budgetCommitted: committed)
    }

    func votingMasternodes(_ hash: String) throws(ServiceError) -> [VotingMasternode] {
        guard proposals.contains(where: { $0.row.hash == hash }) else { throw .demo(.governanceProposalNotFound) }
        return walletVoters.map { node in
            let vote = votes[hash]?[node.row.proTxHash]
            return VotingMasternode(
                proTxHash: node.row.proTxHash,
                collateral: node.row.collateral.map { "\($0.txid)-\($0.vout)" } ?? node.row.proTxHash,
                votingAddress: node.row.votingAddress, weight: node.row.type == .evo ? parameters.evonodeVoteWeight : 1,
                currentVote: vote?.outcome, voteTime: vote?.time,
                nextVoteAt: vote.map { $0.time.addingTimeInterval(Double(parameters.voteUpdateMinimum.components.seconds)) },
                label: node.row.label)
        }
    }

    /// `cast_votes`: a `.governance` grant; one result per masternode; the
    /// 1-hour rule refuses a change without sending it.
    func cast(_ outcome: VoteOutcome, _ hash: String, _ hashes: [String], grant: AuthGrant, wallet: WalletID)
        throws(ServiceError) -> [VoteResult]
    {
        guard governanceSyncOn else { throw .demo(.governanceNotSynced) }
        guard world.sync.connectedPeers > 0 else { throw .demo(.governanceNoPeers) }
        try world.check(grant, .governance, wallet: wallet, refuse: .governance, locked: .governanceVaultLocked)
        try world.redeem(grant, .governance, wallet: wallet, refuse: .governance)
        guard let index = proposals.firstIndex(where: { $0.row.hash == hash }) else {
            throw .demo(.governanceProposalNotFound)
        }
        let now = world.now()
        let window = Double(parameters.voteUpdateMinimum.components.seconds)
        var results: [VoteResult] = []
        for proTxHash in hashes {
            guard let node = walletVoters.first(where: { $0.row.proTxHash == proTxHash }) else {
                results.append(VoteResult(proTxHash: proTxHash, errorCode: .governanceNoVotingKeys))
                continue
            }
            if let previous = votes[hash]?[proTxHash], now.timeIntervalSince(previous.time) < window {
                results.append(VoteResult(proTxHash: proTxHash, errorCode: .governanceVoteTooOften))
                continue
            }
            let weight = node.row.type == .evo ? parameters.evonodeVoteWeight : 1
            var r = proposals[index].row
            var yes = r.yes, no = r.no, abstain = r.abstain
            switch votes[hash]?[proTxHash]?.outcome {
            case .yes: yes -= weight
            case .no: no -= weight
            case .abstain: abstain -= weight
            case nil: break
            }
            switch outcome {
            case .yes: yes += weight
            case .no: no += weight
            case .abstain: abstain += weight
            }
            r = ProposalRow(
                hash: r.hash, name: r.name, url: r.url, paymentAddress: r.paymentAddress, paymentAmount: r.paymentAmount,
                start: r.start, end: r.end, status: r.status, collateralConfirmations: r.collateralConfirmations,
                yes: yes, no: no, abstain: abstain, margin: r.margin + (yes - no) - (r.yes - r.no), myVotes: nil)
            proposals[index].row = r
            votes[hash, default: [:]][proTxHash] = DemoVote(outcome: outcome, time: now)
            results.append(VoteResult(proTxHash: proTxHash, errorCode: nil))
        }
        governanceChanges.send(())
        return results
    }

    func superblockDates(_ count: Int) -> [SuperblockDate] {
        let p = parameters
        return (0..<min(max(count, 0), p.maxPayments)).map { k in
            let height = nextSuperblock + UInt32(k) * p.superblockCycle
            return SuperblockDate(
                height: height, estimatedDate: world.now().addingTimeInterval(Double(height - tip) * DemoLedger.blockSeconds))
        }
    }

    /// The engine's field rules, in field order.
    func validate(_ draft: ProposalDraft) -> [ProposalField] {
        let allowed = Set("abcdefghijklmnopqrstuvwxyz0123456789-_")
        var failing: [ProposalField] = []
        let name = draft.name.lowercased()
        if name.isEmpty || name.count > parameters.maxNameLength || !name.allSatisfy(allowed.contains) {
            failing.append(.name)
        }
        if draft.url.count < 4 || draft.url.contains(where: \.isWhitespace) { failing.append(.url) }
        if !DemoM3World.isAddress(draft.paymentAddress, network: network) { failing.append(.paymentAddress) }
        if draft.paymentAmount <= .zero { failing.append(.paymentAmount) }
        if !(1...parameters.maxPayments).contains(draft.paymentCount) { failing.append(.paymentCount) }
        if !superblockDates(parameters.maxPayments).contains(where: { $0.height == draft.firstSuperblockHeight }) {
            failing.append(.firstPayment)
        }
        if failing.isEmpty, json(draft).utf8.count > parameters.maxPayloadBytes { failing.append(.payload) }
        return failing
    }

    /// "View JSON": dash-qt's key order; the chosen first superblock sets
    /// the epochs (start − cycle/2, last payment + cycle/2).
    func json(_ draft: ProposalDraft) -> String {
        let dates = superblockDates(parameters.maxPayments)
        let half = Double(parameters.superblockCycle) * DemoLedger.blockSeconds / 2
        let first = dates.first { $0.height == draft.firstSuperblockHeight }?.estimatedDate ?? world.now()
        let last = first.addingTimeInterval(Double(max(draft.paymentCount - 1, 0)) * half * 2)
        return """
            {"name":"\(draft.name)","payment_address":"\(draft.paymentAddress)","payment_amount":\(Self.dash(draft.paymentAmount)),\
            "url":"\(draft.url)","start_epoch":\(Int(first.timeIntervalSince1970 - half)),"end_epoch":\(Int(last.timeIntervalSince1970 + half)),"type":1}
            """
    }

    func payloadHex(_ draft: ProposalDraft) -> String {
        json(draft).utf8.map { String(format: "%02x", $0) }.joined()
    }

    nonisolated static func isAddress(_ text: String, network: DashNetwork) -> Bool {
        guard (25...35).contains(text.count), let first = text.first else { return false }
        let alphabet = Set(String(decoding: DemoAddress.alphabet, as: UTF8.self))
        guard text.allSatisfy(alphabet.contains) else { return false }
        return network == .mainnet ? (first == "X" || first == "7") : (first == "y" || first == "8")
    }

    /// `create_proposal`: pays the 1 DASH collateral (an OP_RETURN output)
    /// from the wallet's coins with a `.spend` grant ≥ 1 DASH + fee.
    func createProposal(_ wallet: WalletID, _ draft: ProposalDraft, grant: AuthGrant) throws(ServiceError)
        -> PendingProposal
    {
        if let field = validate(draft).first {
            throw .demo(.governanceInvalidProposal, parameters: ["field": Int64(ProposalField.allCases.firstIndex(of: field)!)])
        }
        let info = try self.wallet(wallet)
        guard !info.watchOnly else { throw .demo(.governanceWatchOnly) }
        guard world.sync.connectedPeers > 0 else { throw .demo(.governanceNoPeers) }
        try world.check(grant, .spend, wallet: wallet, refuse: .governance, locked: .governanceVaultLocked)
        let fee = DemoLedger.scriptFee
        let needed = parameters.proposalFee.duffs + fee
        let issued = try world.redeem(grant, .spend, wallet: wallet, refuse: .governance)
        guard case .spend(let cap) = issued.grant.purpose, cap.duffs >= needed else { throw .demo(.governanceGrantInvalid) }
        let now = world.now()
        let burn = DemoAddress.p2pkh(hash: Array(repeating: 0, count: 20), network: network)
        let txid = try world.update(wallet) { (ledger: inout DemoLedger) throws(ServiceError) -> String? in
            ledger.spend(
                paying: [(burn, parameters.proposalFee.duffs)], fee: fee, type: .sendToAddress, date: now,
                confirmations: 0, instantLocked: true, message: "Proposal collateral: \(draft.name)")
        }
        guard let txid else {
            throw .demo(.governanceInsufficientFunds, parameters: [
                "needed": needed, "available": info.balances?.confirmed.duffs ?? 0,
            ])
        }
        world.notifyHistory(wallet, txids: [txid])
        let hash = DemoM3Sample.hash("gobject|\(txid)")
        let pendingProposal = PendingProposal(
            hash: hash, name: draft.name, url: draft.url, paymentAmount: draft.paymentAmount,
            paymentCount: draft.paymentCount, collateralTxid: txid, collateralStatus: .pending, confirmations: 0,
            createdAt: now, end: now.addingTimeInterval(Double(draft.paymentCount) * Double(parameters.superblockCycle) * DemoLedger.blockSeconds))
        pending[wallet, default: []].append(DemoPendingProposal(proposal: pendingProposal, draft: draft, createdAt: now))
        governanceChanges.send(())
        return pendingProposal
    }

    /// Confirmations follow the demo's block clock (one per 150 s).
    func pendingProposals(_ wallet: WalletID) throws(ServiceError) -> [PendingProposal] {
        _ = try self.wallet(wallet)
        let now = world.now()
        return (pending[wallet] ?? []).map { entry in
            let confirmations = Int(max(0, now.timeIntervalSince(entry.createdAt)) / DemoLedger.blockSeconds)
            let p = entry.proposal
            return PendingProposal(
                hash: p.hash, name: p.name, url: p.url, paymentAmount: p.paymentAmount, paymentCount: p.paymentCount,
                collateralTxid: p.collateralTxid, collateralStatus: confirmations >= 1 ? .ready : .pending,
                confirmations: confirmations, createdAt: p.createdAt, end: p.end)
        }
    }

    /// `gobject submit` at ≥ 1 confirmation; the proposal then lists as
    /// Confirming (until 6) under My Proposals and Active.
    func submitProposal(_ wallet: WalletID, _ hash: String) throws(ServiceError) -> String {
        guard let entry = try pendingProposals(wallet).first(where: { $0.hash == hash }),
            let stored = pending[wallet]?.first(where: { $0.proposal.hash == hash })
        else { throw .demo(.governanceProposalNotFound) }
        guard entry.confirmations >= 1 else {
            throw .demo(.governanceCollateralUnconfirmed, parameters: ["confirmations": Int64(entry.confirmations)])
        }
        guard world.sync.connectedPeers > 0 else { throw .demo(.governanceNoPeers) }
        pending[wallet]?.removeAll { $0.proposal.hash == hash }
        let draft = stored.draft
        let first = superblockDates(parameters.maxPayments).first { $0.height == draft.firstSuperblockHeight }
        let start = first?.estimatedDate ?? world.now()
        proposals.append(DemoProposal(
            row: ProposalRow(
                hash: hash, name: draft.name, url: draft.url, paymentAddress: draft.paymentAddress,
                paymentAmount: draft.paymentAmount, start: start, end: entry.end, status: .confirming,
                collateralConfirmations: entry.confirmations, yes: 0, no: 0, abstain: 0, margin: -30, myVotes: nil),
            parentHash: "0", collateralTxid: entry.collateralTxid, createdAt: entry.createdAt,
            payments: draft.paymentCount, mine: true))
        governanceChanges.send(())
        return hash
    }

    // MARK: Masternodes

    var listState: MasternodeListState {
        let rows = masternodes.map(\.row)
        func active(_ row: MasternodeRow) -> Bool { if case .active = row.status { true } else { false } }
        return MasternodeListState(
            available: true, height: tip, total: rows.count, enabled: rows.filter(active).count,
            evoTotal: rows.filter { $0.type == .evo }.count,
            evoEnabled: rows.filter { $0.type == .evo && active($0) }.count, syncing: !world.sync.isDone)
    }

    private func withTracked(_ row: MasternodeRow) -> MasternodeRow {
        guard tracked[row.proTxHash] != nil, !row.ownedRoles.contains(.tracked) else { return row }
        var roles = row.ownedRoles
        roles.insert(.tracked)
        return MasternodeRow(
            proTxHash: row.proTxHash, service: row.service, type: row.type, shared: row.shared, status: row.status,
            poseScore: row.poseScore, registeredHeight: row.registeredHeight, lastPaidHeight: row.lastPaidHeight,
            nextPaymentHeight: row.nextPaymentHeight, operatorReward: row.operatorReward, collateral: row.collateral,
            collateralAddress: row.collateralAddress, ownerAddress: row.ownerAddress, votingAddress: row.votingAddress,
            payoutAddresses: row.payoutAddresses, operatorPublicKey: row.operatorPublicKey,
            platformNodeID: row.platformNodeID, ownedRoles: roles, label: tracked[row.proTxHash]?.label ?? row.label)
    }

    /// dash-qt's literal, case-insensitive search over the §10.1 fields.
    func list(_ query: MasternodeQuery) -> [MasternodeRow] {
        let text = query.text?.lowercased() ?? ""
        return masternodes.map { withTracked($0.row) }.filter { row in
            switch query.typeFilter {
            case .all: break
            case .regular: if row.type != .regular || row.shared != nil { return false }
            case .evo: if row.type != .evo { return false }
            case .shared: if row.shared == nil { return false }
            }
            if query.ownedOnly, row.ownedRoles.isEmpty { return false }
            if query.hideBanned, case .banned = row.status { return false }
            guard !text.isEmpty else { return true }
            let fields = [row.proTxHash, row.service ?? "", row.type == .evo ? "evo" : "regular",
                          row.collateralAddress ?? "", row.ownerAddress ?? "", row.votingAddress,
                          row.registeredHeight.map { "\($0)" } ?? ""] + row.payoutAddresses
            return fields.contains { $0.lowercased().contains(text) }
        }
    }

    func masternodeDetail(_ hash: String) throws(ServiceError) -> MasternodeDetail {
        guard let node = masternodes.first(where: { $0.row.proTxHash == hash }) else { throw .demo(.masternodeNotFound) }
        let d = node.detail
        return MasternodeDetail(
            row: withTracked(node.row), consecutivePayments: d.consecutivePayments, poseBanHeight: d.poseBanHeight,
            poseRevivedHeight: d.poseRevivedHeight, networkAddresses: d.networkAddresses,
            platformP2PAddresses: d.platformP2PAddresses, platformHTTPSAddresses: d.platformHTTPSAddresses,
            shares: d.shares, earlyPeriodEnd: d.earlyPeriodEnd, earlyExitPenalty: d.earlyExitPenalty,
            hasStandbyDissolution: d.hasStandbyDissolution, revocationReason: d.revocationReason,
            walletTransactions: d.walletTransactions)
    }

    /// Outputs of exactly the collateral amount (the sample wallet has none).
    func collateralCandidates(_ wallet: WalletID, _ type: MasternodeType) throws(ServiceError) -> [CollateralCandidate] {
        let ledger = try world.ledger(wallet)
        let defaults = M3Defaults.masternodeDefaults(network)
        let amount = type == .evo ? defaults.evonodeCollateral : defaults.masternodeCollateral
        return ledger.coins.filter { $0.amount == amount.duffs }.map { coin in
            CollateralCandidate(
                outpoint: coin.outpoint, address: coin.address, amount: Amount(duffs: coin.amount),
                confirmations: Int(coin.confirmations), refusal: coin.confirmations == 0 ? .unconfirmed : nil)
        }
    }

    func feeSources(_ wallet: WalletID) throws(ServiceError) -> [FeeSourceCandidate] {
        let ledger = try world.ledger(wallet)
        var byAddress: [String: Int64] = [:]
        for coin in ledger.spendableCoins { byAddress[coin.address, default: 0] += coin.amount }
        let labels = Dictionary(ledger.addressBook.map { ($0.address, $0.label) }, uniquingKeysWith: { a, _ in a })
        return byAddress.sorted { $0.value > $1.value }.map {
            FeeSourceCandidate(address: $0.key, spendable: Amount(duffs: $0.value), label: labels[$0.key])
        }
    }

    /// `prepare_registration`'s checks; a FundNew registration needs the
    /// collateral plus the fee from the fee source.
    func prepareRegistration(_ request: RegistrationRequest, grant: AuthGrant) throws(ServiceError)
        -> PreparedRegistrationReference
    {
        let info = try wallet(request.wallet)
        guard !info.watchOnly else { throw .demo(.masternodeWatchOnly) }
        try world.check(grant, .masternodeOperation, wallet: request.wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        let defaults = M3Defaults.masternodeDefaults(network)
        if request.serviceAddresses.contains(where: { !$0.contains(":") }) { throw .demo(.masternodeInvalidService) }
        if case .existing(let key) = request.operatorKey, !(key.count == 96 && key.allSatisfy(\.isHexDigit)) {
            throw .demo(.masternodeInvalidKey, parameters: ["role": 2])
        }
        if request.type == .evo, request.platform.map({ $0.nodeIDHex.count == 40 && $0.nodeIDHex.allSatisfy(\.isHexDigit) }) != true {
            throw .demo(.masternodeInvalidKey, parameters: ["role": 3])
        }
        guard Self.isAddress(request.payoutAddress, network: network) else { throw .demo(.masternodeInvalidPayout) }
        guard (0...defaults.maxOperatorRewardX100).contains(request.operatorRewardX100) else {
            throw .demo(.invalidArgument, "operator_reward")
        }
        let collateralAmount = request.type == .evo ? defaults.evonodeCollateral : defaults.masternodeCollateral
        let fee = Amount(duffs: 1_000)
        let collateral: OutPoint
        var signMessage: String?
        var spent = fee
        switch request.collateral {
        case .fundNew:
            let available = try world.ledger(request.wallet).spendableCoins.reduce(Int64(0)) { $0 + $1.amount }
            let needed = collateralAmount.duffs + fee.duffs
            guard available >= needed else {
                throw .demo(.masternodeInsufficientFunds, parameters: ["needed": needed, "available": available])
            }
            collateral = OutPoint(txid: DemoM3Sample.hash("fund|\(UUID().uuidString)"), vout: 0)
            spent = Amount(duffs: needed)
        case .existingUTXO(let outpoint):
            guard try collateralCandidates(request.wallet, request.type).contains(where: { $0.outpoint == outpoint && $0.refusal == nil })
            else { throw .demo(.masternodeCollateralUnavailable, parameters: ["refusal": 5]) }
            collateral = outpoint
        case .external(let outpoint):
            collateral = outpoint
            signMessage = "\(request.payoutAddress)|\(request.operatorRewardX100)|owner|voting|\(DemoM3Sample.hash("payload|\(outpoint.txid)"))"
        }
        try world.redeem(grant, .masternodeOperation, wallet: request.wallet, refuse: .masternode)
        var rng = DemoRandom(text: "registration|\(collateral.txid)")
        let owner = request.ownerAddress ?? rng.address(on: network)
        let generated = request.operatorKey == .generate
        let secret = generated ? rng.hex(bytes: 32) : nil
        let publicKey: String
        switch request.operatorKey {
        case .generate: publicKey = rng.hex(bytes: 48)
        case .existing(let key): publicKey = key
        }
        let summary = RegistrationSummary(
            type: request.type, proTxHash: rng.hex(bytes: 32), collateral: collateral,
            collateralAddress: rng.address(on: network), ownerAddress: owner,
            votingAddress: request.votingAddress ?? owner, payoutAddress: request.payoutAddress,
            operatorPublicKey: publicKey, operatorRewardX100: request.operatorRewardX100,
            serviceAddresses: request.serviceAddresses, platform: request.platform, fee: fee, totalSpent: spent,
            operatorSecretRequired: generated, collateralSignMessage: signMessage)
        let id = UUID()
        registrations[id] = (summary, secret, false, request.wallet)
        return PreparedRegistrationReference(id: id, summary: summary)
    }

    func operatorSecret(_ id: UUID) throws(ServiceError) -> OperatorSecret {
        guard let secret = registrations[id]?.secret else { throw .demo(.invalidArgument, "key not generated") }
        return OperatorSecret(
            secretHex: DemoSecret(utf8: secret), configLine: DemoSecret(utf8: "masternodeblsprivkey=\(secret)"))
    }

    func confirmSecret(_ id: UUID, last4: String) -> Bool {
        guard let secret = registrations[id]?.secret, last4.count == 4,
            secret.suffix(4).lowercased() == last4.lowercased()
        else { return false }
        registrations[id]?.gateOpen = true
        return true
    }

    /// `submit`: refused before the last-4 gate (dash-qt's order). The demo
    /// adds the registered masternode to the list as the wallet's.
    func submitRegistration(_ id: UUID, signature: String?) throws(ServiceError) -> String {
        guard let entry = registrations[id] else { throw .demo(.invalidArgument, "unknown registration") }
        if entry.summary.operatorSecretRequired, !entry.gateOpen { throw .demo(.masternodeOperatorSecretUnconfirmed) }
        if entry.summary.collateralSignMessage != nil, (signature ?? "").count < 20 {
            throw .demo(.masternodeCollateralSignatureInvalid)
        }
        guard world.sync.connectedPeers > 0 else { throw .demo(.masternodeNoPeers) }
        let s = entry.summary
        let row = MasternodeRow(
            proTxHash: s.proTxHash, service: s.serviceAddresses.first, type: s.type, shared: nil,
            status: s.serviceAddresses.isEmpty ? .unknown : .active(sinceHeight: tip), poseScore: nil,
            registeredHeight: tip, lastPaidHeight: nil, nextPaymentHeight: nil,
            operatorReward: OperatorReward(percentX100: s.operatorRewardX100, payoutAddress: nil), collateral: s.collateral,
            collateralAddress: s.collateralAddress, ownerAddress: s.ownerAddress, votingAddress: s.votingAddress,
            payoutAddresses: [s.payoutAddress], operatorPublicKey: s.operatorPublicKey,
            platformNodeID: s.platform?.nodeIDHex, ownedRoles: [.owner, .voting, .payout], label: nil)
        let detail = MasternodeDetail(
            row: row, consecutivePayments: nil, poseBanHeight: nil, poseRevivedHeight: nil,
            networkAddresses: s.serviceAddresses, platformP2PAddresses: s.platform?.p2pAddresses ?? [],
            platformHTTPSAddresses: s.platform?.httpsAddresses ?? [], shares: [], earlyPeriodEnd: nil,
            earlyExitPenalty: nil, hasStandbyDissolution: false, revocationReason: nil, walletTransactions: 1)
        masternodes.insert(DemoMasternode(row: row, detail: detail, votingKeyInWallet: true), at: 0)
        registrations[id] = nil
        masternodeChanges.send(())
        return s.proTxHash
    }

    // MARK: Keychain and tracked

    /// Derived provider keys of the wallet (public data), ≤ 100 per call.
    func keys(_ wallet: WalletID, role: MasternodeKeyRole, range: Range<UInt32>) throws(ServiceError) -> [MasternodeKeyInfo] {
        guard range.count <= 100 else { throw .demo(.invalidArgument, "count > 100") }
        _ = try self.wallet(wallet)
        let coin = network == .mainnet ? 5 : 1
        let account: Int =
            switch role {
            case .voting: 1
            case .owner: 2
            case .operator: 3
            case .platformNode: 4
            case .ownerPayout, .operatorPayout: 0
            }
        let owned = masternodes.filter { !$0.row.ownedRoles.isEmpty && $0.row.shared == nil }
        return range.map { index in
            var rng = DemoRandom(text: "mnkey|\(wallet.hex)|\(role)|\(index)")
            let usedBy = index < owned.count && role != .ownerPayout && role != .operatorPayout
                ? [MasternodeKeyUsage(proTxHash: owned[Int(index)].row.proTxHash, service: owned[Int(index)].row.service, revoked: false)]
                : []
            let isSecp = role == .owner || role == .voting || role == .ownerPayout || role == .operatorPayout
            return MasternodeKeyInfo(
                role: role, index: index,
                derivationPath: account == 0 ? "m/44'/\(coin)'/0'/0/\(index)" : "m/9'/\(coin)'/3'/\(account)'/\(index)",
                address: isSecp ? rng.address(on: network) : nil,
                publicKeyHex: role == .operator ? rng.hex(bytes: 48) : role == .platformNode ? rng.hex(bytes: 32) : "02" + rng.hex(bytes: 32),
                legacyPublicKeyHex: role == .operator ? rng.hex(bytes: 48) : nil,
                platformNodeID: role == .platformNode ? rng.hex(bytes: 20) : nil, usedBy: usedBy)
        }
    }

    func revealKey(_ wallet: WalletID, role: MasternodeKeyRole, index: UInt32, grant: AuthGrant) throws(ServiceError)
        -> RevealedMasternodeKey
    {
        try world.check(grant, .revealSecret, wallet: wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        try world.redeem(grant, .revealSecret, wallet: wallet, refuse: .masternode)
        var rng = DemoRandom(text: "mnpriv|\(wallet.hex)|\(role)|\(index)")
        let key = rng.hex(bytes: 32)
        return RevealedMasternodeKey(
            privateKeyHex: DemoSecret(utf8: key),
            wif: role == .owner || role == .voting ? DemoSecret(utf8: "c" + DemoAddress.base58(rng.bytes(37))) : nil,
            tenderdashKey: role == .platformNode ? DemoSecret(utf8: key + rng.hex(bytes: 32)) : nil)
    }

    func locate(_ query: String) -> [MasternodeRow] {
        let text = query.lowercased().trimmingCharacters(in: .whitespaces)
        return masternodes.map { withTracked($0.row) }.filter { row in
            (row.service ?? "").lowercased().hasPrefix(text) || row.proTxHash == text || row.votingAddress.lowercased() == text
                || row.ownerAddress?.lowercased() == text || row.operatorPublicKey == text
                || row.payoutAddresses.contains { $0.lowercased() == text }
        }
    }

    func trackedNodes() -> [TrackedMasternode] {
        tracked.keys.sorted().compactMap { hash in
            guard let node = masternodes.first(where: { $0.row.proTxHash == hash }), let entry = tracked[hash] else { return nil }
            let roles = Set(entry.attached.keys)
            return TrackedMasternode(
                row: withTracked(node.row), label: entry.label, attachedRoles: roles,
                capabilities: TrackedCapabilities(
                    canWithdraw: node.row.type == .evo && (roles.contains(.owner) || roles.contains(.ownerPayout)),
                    canUpdateService: roles.contains(.operator), canUpdateRegistrar: roles.contains(.owner),
                    canVote: roles.contains(.voting)))
        }
    }

    func track(_ hash: String, label: String?) throws(ServiceError) -> TrackedMasternode {
        guard tracked[hash] == nil else { throw .demo(.masternodeAlreadyTracked) }
        guard masternodes.contains(where: { $0.row.proTxHash == hash }) else { throw .demo(.masternodeNotFound) }
        tracked[hash] = (label, [:])
        masternodeChanges.send(())
        return trackedNodes().first { $0.id == hash }!
    }

    func untrack(_ hash: String) -> Bool {
        let removed = tracked.removeValue(forKey: hash) != nil
        if removed { masternodeChanges.send(()) }
        return removed
    }

    func setLabel(_ label: String?, _ hash: String) throws(ServiceError) {
        guard tracked[hash] != nil else { throw .demo(.masternodeNotFound) }
        tracked[hash]?.label = label
        masternodeChanges.send(())
    }

    /// Key formats per role (m3-engine.md §2.5); stored "in the vault".
    func attach(_ key: String, role: MasternodeKeyRole, hash: String, grant: AuthGrant, wallet: WalletID) throws(ServiceError) {
        guard tracked[hash] != nil else { throw .demo(.masternodeNotFound) }
        try world.check(grant, .masternodeOperation, wallet: wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        let hex = key.allSatisfy(\.isHexDigit)
        let valid: Bool =
            switch role {
            case .operator: hex && key.count == 64
            case .platformNode: (hex && key.count == 64) || Data(base64Encoded: key)?.count == 32
            default: (hex && key.count == 64) || ((51...52).contains(key.count) && !hex)
            }
        guard valid else { throw .demo(.masternodeInvalidKey, parameters: ["role": Int64(MasternodeKeyRole.allCases.firstIndex(of: role)!)]) }
        try world.redeem(grant, .masternodeOperation, wallet: wallet, refuse: .masternode)
        tracked[hash]?.attached[role] = key
        masternodeChanges.send(())
    }

    func detach(_ role: MasternodeKeyRole, _ hash: String) {
        tracked[hash]?.attached[role] = nil
        masternodeChanges.send(())
    }

    func revealAttached(_ role: MasternodeKeyRole, _ hash: String, grant: AuthGrant, wallet: WalletID) throws(ServiceError)
        -> RevealedMasternodeKey
    {
        guard let key = tracked[hash]?.attached[role] else {
            throw .demo(.masternodeKeyNotInWallet, parameters: ["role": Int64(MasternodeKeyRole.allCases.firstIndex(of: role)!)])
        }
        try world.check(grant, .revealSecret, wallet: wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        try world.redeem(grant, .revealSecret, wallet: wallet, refuse: .masternode)
        return RevealedMasternodeKey(privateKeyHex: DemoSecret(utf8: key), wif: nil, tenderdashKey: nil)
    }

    // MARK: Shared sessions

    func createShared(_ wallet: WalletID, _ terms: SharedMasternodeTerms) throws(ServiceError) -> SharedSessionInfo {
        let defaults = M3Defaults.masternodeDefaults(network)
        let amounts = terms.shares.map(\.amount)
        guard defaults.shares.contains(amounts.count), amounts.allSatisfy({ $0 >= defaults.minimumShareAmount }),
            amounts.reduce(0, { $0 + $1.duffs }) == defaults.masternodeCollateral.duffs,
            terms.earlyPeriodBlocks <= defaults.maxEarlyPeriodBlocks,
            let smallest = amounts.min(), terms.earlyExitPenalty < smallest
        else { throw .demo(.invalidArgument, "terms") }
        return newSession(wallet, role: .coordinator, stage: .invitation, purpose: .register, proTxHash: nil)
    }

    func newSession(
        _ wallet: WalletID, role: SharedRole, stage: SharedStage, purpose: SharedSessionPurpose, proTxHash: String?
    ) -> SharedSessionInfo {
        let id = DemoM3Sample.hash("session|\(UUID().uuidString)")
        let fingerprint = String(DemoM3Sample.hash("fp|\(id)").prefix(8)).uppercased()
        let info = SharedSessionInfo(
            id: id, sessionCode: String(id.prefix(6)), purpose: purpose, role: role, stage: stage, revision: 1,
            fingerprint: "\(fingerprint.prefix(4))-\(fingerprint.suffix(4))", wallet: wallet, myShareIndexes: [0],
            reservedInputs: [], reservedCoinSpent: false, proTxHash: proTxHash)
        sharedSessions[id] = info
        return info
    }

    func envelope(_ id: String) throws(ServiceError) -> SharedEnvelope {
        guard let session = sharedSessions[id] else { throw .demo(.masternodeSharedSessionNotFound) }
        let json = """
            {"type":"dash-shared-mn-session","version":1,"network":"\(network.description)","sessionId":"\(id)",\
            "revision":\(session.revision),"stage":"\(session.stage)","fingerprint":"\(session.fingerprint)"}
            """
        return SharedEnvelope(json: json, fingerprint: session.fingerprint, suggestedFileName: "shared-mn-\(session.sessionCode).json")
    }

    /// Paste routing: ≤ 2 MiB; this network's envelopes join their session
    /// (an invitation creates one); raw hex lines are standby text.
    func importShared(_ text: String, wallet: WalletID) throws(ServiceError) -> SharedMessageKind {
        let size = text.utf8.count
        guard size <= M3Defaults.masternodeDefaults(network).maxEnvelopeBytes else {
            throw .demo(.masternodeSharedEnvelopeTooLarge, parameters: ["size_bytes": Int64(size)])
        }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let lines = trimmed.split(whereSeparator: \.isNewline).map(String.init)
        if !lines.isEmpty, lines.allSatisfy({ $0.count > 20 && $0.allSatisfy(\.isHexDigit) }) {
            return .standbyDissolution(proTxHash: nil, transactionsHex: lines)
        }
        guard let data = trimmed.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            object["type"] as? String == "dash-shared-mn-session", object["version"] as? Int == 1,
            let sessionID = object["sessionId"] as? String
        else { throw .demo(.masternodeSharedEnvelopeInvalid) }
        guard object["network"] as? String == network.description else { throw .demo(.masternodeSharedNetworkMismatch) }
        if let existing = sharedSessions[sessionID] { return .envelope(existing) }
        guard object["stage"] as? String == "invitation" else { throw .demo(.masternodeSharedSessionNotFound) }
        var info = newSession(wallet, role: .participant, stage: .details, purpose: .register, proTxHash: nil)
        sharedSessions[info.id] = nil
        info = SharedSessionInfo(
            id: sessionID, sessionCode: String(sessionID.prefix(6)), purpose: info.purpose, role: .participant,
            stage: .details, revision: 1, fingerprint: object["fingerprint"] as? String ?? info.fingerprint,
            wallet: wallet, myShareIndexes: [], reservedInputs: [], reservedCoinSpent: false, proTxHash: nil)
        sharedSessions[sessionID] = info
        return .envelope(info)
    }

    func advance(_ id: String, to stage: SharedStage, reserving inputs: [OutPoint]? = nil, share: Int? = nil)
        throws(ServiceError) -> SharedSessionInfo
    {
        guard let s = sharedSessions[id] else { throw .demo(.masternodeSharedSessionNotFound) }
        let next = SharedSessionInfo(
            id: s.id, sessionCode: s.sessionCode, purpose: s.purpose, role: s.role, stage: stage, revision: s.revision + 1,
            fingerprint: s.fingerprint, wallet: s.wallet, myShareIndexes: share.map { [$0] } ?? s.myShareIndexes,
            reservedInputs: inputs ?? s.reservedInputs, reservedCoinSpent: false, proTxHash: s.proTxHash)
        sharedSessions[id] = next
        return next
    }

    /// Reserves the contribution's coins (spendable wallet coins only).
    func contribute(_ contribution: ShareContribution, session id: String) throws(ServiceError) -> SharedSessionInfo {
        guard let session = sharedSessions[id] else { throw .demo(.masternodeSharedSessionNotFound) }
        let ledger = try world.ledger(session.wallet)
        let spendable = Set(ledger.spendableCoins.map(\.outpoint))
        guard !contribution.inputs.isEmpty, contribution.inputs.allSatisfy(spendable.contains) else {
            throw .demo(.masternodeCollateralUnavailable, parameters: ["refusal": 5])
        }
        try world.update(session.wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            ledger.lockedOutpoints.formUnion(contribution.inputs)
        }
        return try advance(id, to: .lockedTerms, reserving: contribution.inputs, share: contribution.shareIndex)
    }

    func approve(_ id: String, grant: AuthGrant) throws(ServiceError) -> SharedSessionInfo {
        guard let session = sharedSessions[id] else { throw .demo(.masternodeSharedSessionNotFound) }
        try world.check(grant, .masternodeOperation, wallet: session.wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        try world.redeem(grant, .masternodeOperation, wallet: session.wallet, refuse: .masternode)
        return try advance(id, to: session.role == .coordinator ? .signingRequest : .approvals)
    }

    func sign(_ id: String, grant: AuthGrant) throws(ServiceError) -> SharedSessionInfo {
        guard let session = sharedSessions[id] else { throw .demo(.masternodeSharedSessionNotFound) }
        try world.check(grant, .masternodeOperation, wallet: session.wallet, refuse: .masternode, locked: .masternodeVaultLocked)
        try world.redeem(grant, .masternodeOperation, wallet: session.wallet, refuse: .masternode)
        return try advance(id, to: .signedContributions)
    }

    /// Leaves the session and releases its reserved coins (close protection).
    func abandonShared(_ id: String) throws(ServiceError) {
        guard let session = sharedSessions.removeValue(forKey: id) else { throw .demo(.masternodeSharedSessionNotFound) }
        _ = try? world.update(session.wallet) { (ledger: inout DemoLedger) throws(ServiceError) in
            ledger.lockedOutpoints.subtract(session.reservedInputs)
        }
    }
}
