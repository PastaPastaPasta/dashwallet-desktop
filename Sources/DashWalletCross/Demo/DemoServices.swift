// The WalletRuntime service protocols over `DemoWorld` (--demo only).
import Foundation
import WalletFeatures
import WalletRuntime

// MARK: Host and lifecycle

final class DemoHost: WalletHosting {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    var activeNetwork: DashNetwork? {
        get async { await world.network }
    }

    func start(network: DashNetwork, options: NetworkOptions) async throws(ServiceError) {}
    func stop() async throws(ServiceError) {}

    /// A scratch directory: the demo stores nothing.
    func dataDirectory(for network: DashNetwork) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("dashwallet-demo/\(network.description)")
    }
}

final class DemoLifecycle: LifecycleQueueing {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    var transition: LifecycleTransition {
        get async { await world.transition }
    }

    func transitions() -> AsyncStream<LifecycleTransition> {
        world.transitionChanges.stream(initial: .idle)
    }

    func start(network: DashNetwork) async throws(ServiceError) {}
    func stop() async throws(ServiceError) {}

    func switchNetwork(to network: DashNetwork) async throws(ServiceError) {
        let from = await world.network
        try await run(.switchingNetwork(from: from, to: network)) { world in
            world.networkBox.network = network
            if world.states[network] == nil { world.states[network] = DemoNetworkState() }
            world.walletChanges.yield(())
            world.lockChanges.yield(world.vault.state)
            world.syncChanges.yield(world.syncStatus)
        }
    }

    func importWallet(mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer, options: WalletImportOptions)
        async throws(ServiceError) -> WalletID
    {
        let phrase = mnemonic.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
        let state = await world.vault.state
        guard state != .noVault else { throw ServiceError(code: .vaultNoVault, detail: "demo") }
        guard state != .locked else { throw ServiceError(code: .vaultLocked, detail: "demo") }
        return try await run(.addingWallet) { world throws(ServiceError) in
            try world.addWallet(phrase: phrase, name: options.name)
        }
    }

    func removeWallet(_ id: WalletID, grant: AuthGrant) async throws(ServiceError) {
        guard grant.purpose == .wipe else { throw ServiceError(code: .vaultGrantInvalid, detail: "demo") }
        try await run(.removingWallet(id)) { $0.removeWallet(id) }
    }

    /// Shows `transition` briefly (so the overlay is visible), runs `body`, returns to idle.
    @MainActor
    private func run<T: Sendable>(
        _ transition: LifecycleTransition, _ body: @MainActor (DemoWorld) throws(ServiceError) -> T
    )
        async throws(ServiceError) -> T
    {
        world.transition = transition
        world.transitionChanges.yield(transition)
        try? await Task.sleep(for: .milliseconds(400))
        defer {
            world.transition = .idle
            world.transitionChanges.yield(.idle)
        }
        return try body(world)
    }
}

// MARK: State holders

@MainActor
final class DemoWalletState: WalletStateProviding {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    var wallets: [WalletInfo]? { world.current.wallets }
    var selectedWalletID: WalletID? { world.current.selected }
    var balances: WalletBalances? { selectedWalletID.map(world.balances(of:)) }

    func changes() -> AsyncStream<Void> { world.walletChanges.stream() }

    func select(_ id: WalletID) {
        guard world.current.wallets.contains(where: { $0.id == id }) else { return }
        world.current.selected = id
        world.walletChanges.yield(())
    }

    func rename(_ id: WalletID, to name: String) async throws(ServiceError) {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        guard (1...64).contains(trimmed.count) else { throw ServiceError(code: .walletNameRejected, detail: "demo") }
        guard let index = world.current.wallets.firstIndex(where: { $0.id == id }) else {
            throw ServiceError(code: .walletNotFound, detail: "demo")
        }
        let info = world.current.wallets[index]
        world.current.wallets[index] = WalletInfo(
            id: info.id, name: trimmed, watchOnly: info.watchOnly, hasMnemonic: info.hasMnemonic, hd: info.hd,
            birthHeight: info.birthHeight, createdAt: info.createdAt, balances: info.balances)
        world.walletChanges.yield(())
    }
}

@MainActor
final class DemoSync: SyncStatusProviding {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    var status: SyncStatus? { world.syncStatus }

    func changes() -> AsyncStream<SyncStatus> { world.syncChanges.stream(initial: world.syncStatus) }

    func peers() async throws(ServiceError) -> [PeerInfo] {
        (1...8).map {
            PeerInfo(
                address: "203.0.113.\($0):19999", userAgent: "/Dash Core:24.0.0/", protocolVersion: 70_237,
                bestHeight: DemoWorld.tipHeight, pingMilliseconds: UInt32(20 + $0 * 7), connectedSince: world.now,
                inbound: false)
        }
    }

    func rotatePeers() async throws(ServiceError) {}
    func rescan(from start: RescanStart) async throws(ServiceError) {}
}

@MainActor
final class DemoSettings: SettingsProviding, UIPreferencesStoring {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    var display: DisplaySettings { world.display }
    var lastNetwork: DashNetwork? { world.network }

    func update(_ display: DisplaySettings) throws(ServiceError) {
        world.display = display
        world.settingsChanges.yield(display)
    }

    func changes() -> AsyncStream<DisplaySettings> { world.settingsChanges.stream() }

    var preferences: UIPreferences { world.preferences }

    func update(_ preferences: UIPreferences) throws(ServiceError) {
        world.preferences = preferences
    }
}

// MARK: Vault and authorization

final class DemoVault: VaultProviding {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    func status() async throws(ServiceError) -> VaultStatus { await world.vaultStatus() }

    func create(passphrase: (any SecretBuffer)?) async throws(ServiceError) -> VaultStatus {
        try await world.createVault(passphrase: passphrase.map(Self.text))
    }

    func encrypt(newPassphrase: any SecretBuffer, grant: AuthGrant) async throws(ServiceError) -> VaultStatus {
        try await world.encryptVault()
    }

    func changePassphrase(old: any SecretBuffer, new: any SecretBuffer) async throws(ServiceError) -> VaultStatus {
        try await world.checkPassphrase(Self.text(old))
        return await world.vaultStatus()
    }

    func revealMnemonic(wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> RevealedMnemonic {
        guard grant.purpose == .revealSecret else { throw ServiceError(code: .vaultGrantInvalid, detail: "demo") }
        return RevealedMnemonic(
            phrase: DemoSecret(utf8: DemoWorld.phrase12.joined(separator: " ")), bip39Passphrase: DemoSecret([]))
    }

    /// Always one of the BIP39 test vectors: the demo must not look like it
    /// made a usable wallet.
    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) async throws(ServiceError) -> any SecretBuffer {
        switch wordCount {
        case 12: DemoSecret(utf8: DemoWorld.phrase12.joined(separator: " "))
        case 24: DemoSecret(utf8: DemoWorld.phrase24.joined(separator: " "))
        default: throw ServiceError(code: .walletUnsupportedWordCount, detail: "demo supports 12 and 24 words")
        }
    }

    /// Word-list check only; the demo does not verify the BIP39 checksum.
    func checkMnemonic(_ phrase: any SecretBuffer) async throws(ServiceError) -> MnemonicCheck {
        let words = Self.text(phrase).split(separator: " ").map(String.init)
        let unknown = words.indices.filter { index in
            let word = words[index]
            return !MnemonicWords.suggestions(for: String(word.prefix(1)), limit: 2048).contains(word)
        }
        return MnemonicCheck(
            wordCount: words.count, unknownWordIndices: unknown, language: unknown.isEmpty ? .english : nil,
            checksum: unknown.isEmpty ? .valid : .invalid)
    }

    func makeSecret(utf8 text: String) -> any SecretBuffer { DemoSecret(utf8: text) }

    static func text(_ secret: any SecretBuffer) -> String {
        secret.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
    }
}

@MainActor
final class DemoAuth: AuthenticationGating {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    var lockState: VaultLockState? { world.vault.state }

    func lockStateChanges() -> AsyncStream<VaultLockState> { world.lockChanges.stream(initial: world.vault.state) }

    func requirement(for purpose: GrantPurpose) -> CredentialRequirement {
        world.vault.encrypted ? .passphrase : .none
    }

    func authorize(_ purpose: GrantPurpose, credential: Credential) async throws(ServiceError) -> AuthGrant {
        switch credential {
        case .unencrypted:
            guard !world.vault.encrypted else { throw ServiceError(code: .vaultWrongPassphrase, detail: "demo") }
        case .passphrase(let secret):
            try world.checkPassphrase(DemoVault.text(secret))
        }
        return AuthGrant(id: UUID().uuidString, purpose: purpose, expiresAt: Date().addingTimeInterval(60), singleUse: true)
    }

    func unlock(passphrase: any SecretBuffer, scope: UnlockScope) async throws(ServiceError) {
        try world.checkPassphrase(DemoVault.text(passphrase))
        world.vault.state = scope == .full ? .unlocked : .unlockedMixingOnly
    }

    func lock() async throws(ServiceError) {
        guard world.vault.encrypted else { return }
        world.vault.state = .locked
    }
}

extension DemoWorld {
    func vaultStatus() -> VaultStatus {
        VaultStatus(
            state: vault.state, encrypted: vault.encrypted, quickUnlockEnrolled: false, failedAttempts: failedAttempts,
            retryAfterSeconds: nil, walletsWithSecrets: current.wallets.map(\.id))
    }

    func createVault(passphrase: String?) throws(ServiceError) -> VaultStatus {
        guard vault.state == .noVault else { throw ServiceError(code: .vaultAlreadyExists, detail: "demo") }
        vault = passphrase == nil ? (false, .unencrypted) : (true, .unlocked)
        return vaultStatus()
    }

    func encryptVault() throws(ServiceError) -> VaultStatus {
        guard !vault.encrypted else {
            throw ServiceError(code: ServiceErrorCode(rawValue: "vault.already_encrypted"), detail: "demo")
        }
        vault = (true, .unlocked)
        return vaultStatus()
    }

    /// The demo vault's passphrase is "demo" (any passphrase chosen during
    /// demo onboarding is not kept).
    func checkPassphrase(_ text: String) throws(ServiceError) {
        guard text == Self.passphrase else {
            failedAttempts += 1
            throw ServiceError(code: .vaultWrongPassphrase, detail: "demo")
        }
        failedAttempts = 0
    }
}

// MARK: Sending

final class DemoSender: TransactionSending {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    func makeDraft(wallet: WalletID) async throws(ServiceError) -> any TransactionDrafting {
        await DemoDraft(world: world, wallet: wallet)
    }

    func maxSpendable(wallet: WalletID, source: CoinSourceChoice, fee: FeeChoice) async throws(ServiceError) -> Amount {
        let available = await world.balances(of: wallet).confirmed.duffs
        let estimate = DemoDraft.fee(inputs: 2, outputs: 1, fee: fee)
        return Amount(duffs: max(0, available - estimate))
    }
}

/// Draft state is only touched from the main actor.
@MainActor
final class DemoDraft: TransactionDrafting {
    let world: DemoWorld
    let wallet: WalletID
    private var recipients: [PaymentRecipient] = []
    private var fee: FeeChoice = .recommended(targetBlocks: 6)
    private var prepared: Set<UUID> = []

    init(world: DemoWorld, wallet: WalletID) {
        self.world = world
        self.wallet = wallet
    }

    func setRecipients(_ recipients: [PaymentRecipient]) async throws(ServiceError) {
        guard !recipients.isEmpty else { throw ServiceError(code: .sendNoRecipients, detail: "demo") }
        for (index, recipient) in recipients.enumerated() where recipient.amount.duffs < 546 {
            throw ServiceError(code: .sendDustAmount, detail: "demo", recipientIndex: index)
        }
        self.recipients = recipients
    }

    func setSource(_ source: CoinSourceChoice) async throws(ServiceError) {}
    func setFee(_ fee: FeeChoice) async throws(ServiceError) { self.fee = fee }
    func setChange(_ change: ChangeChoice) async throws(ServiceError) {}

    func estimate() async throws(ServiceError) -> TxEstimate {
        let fee = Self.fee(inputs: 2, outputs: recipients.count + 1, fee: self.fee)
        let sent = recipients.reduce(Int64(0)) { $0 + $1.amount.duffs }
        let subtracts = recipients.contains { $0.subtractFeeFromAmount }
        let debit = sent + (subtracts ? 0 : fee)
        let available = world.balances(of: wallet).confirmed.duffs
        guard debit <= available else {
            throw ServiceError(
                code: sent <= available ? .sendAmountWithFeeExceedsBalance : .sendAmountExceedsBalance, detail: "demo")
        }
        return TxEstimate(
            fee: Amount(duffs: fee), sizeBytes: UInt32(Self.size(inputs: 2, outputs: recipients.count + 1)),
            inputCount: 2, change: Amount(duffs: available - debit), totalSent: Amount(duffs: sent - (subtracts ? fee : 0)))
    }

    func prepare(grant: AuthGrant) async throws(ServiceError) -> PreparedTransaction {
        let estimate = try await estimate()
        guard case .spend(let max) = grant.purpose, max.duffs >= estimate.totalSent.duffs else {
            throw ServiceError(code: .sendGrantExceeded, detail: "demo")
        }
        var subtractDone = false
        let outputs = recipients.map { recipient -> PreparedOutput in
            var amount = recipient.amount.duffs
            if recipient.subtractFeeFromAmount, !subtractDone {
                amount -= estimate.fee.duffs
                subtractDone = true
            }
            return PreparedOutput(address: recipient.address, amount: Amount(duffs: amount), isChange: false, label: recipient.label)
        } + [PreparedOutput(address: DemoWorld.ownAddresses[5], amount: estimate.change ?? .zero, isChange: true, label: nil)]
        let id = UUID()
        prepared.insert(id)
        let summary = PreparedTxSummary(
            txid: DemoWorld.hex64(seed: UInt64(truncatingIfNeeded: id.hashValue)), fee: estimate.fee,
            feeRatePerKilobyte: Amount(duffs: Self.rate(fee)), sizeBytes: estimate.sizeBytes, inputCount: 2,
            outputs: outputs, totalSent: estimate.totalSent,
            totalDebit: Amount(duffs: estimate.totalSent.duffs + estimate.fee.duffs))
        return PreparedTransaction(id: id, summary: summary)
    }

    /// Records the payment in the demo history; nothing leaves the process.
    func broadcast(_ prepared: PreparedTransaction) async throws(ServiceError) -> BroadcastResult {
        guard self.prepared.remove(prepared.id) != nil else {
            throw ServiceError(code: ServiceErrorCode(rawValue: "send.prepared_tx_spent"), detail: "demo")
        }
        let summary = prepared.summary
        let first = summary.outputs.first { !$0.isChange }
        let record = TxRecord(
            id: .init(txid: summary.txid, recordIndex: 0), type: .sendToAddress, category: .sent,
            status: TxStatus(kind: .unconfirmed, confirmations: 0, instantLocked: true, chainLocked: false, maturesIn: nil),
            date: Date(), blockHeight: nil, amount: Amount(duffs: -summary.totalDebit.duffs), fee: summary.fee,
            address: first?.address, label: first?.label, countsTowardBalance: true, involvesWatchOnly: false)
        world.current.records[wallet, default: []].append(record)
        world.refreshWallets()
        world.historyChanges.yield((wallet, [summary.txid]))
        return BroadcastResult(txid: summary.txid, peersAnnounced: 8)
    }

    func abandon(_ prepared: PreparedTransaction) async throws(ServiceError) {
        self.prepared.remove(prepared.id)
    }

    nonisolated static func rate(_ fee: FeeChoice) -> Int64 {
        switch fee {
        case .recommended: 1000
        case .perKilobyte(let amount): amount.duffs
        }
    }

    nonisolated static func size(inputs: Int, outputs: Int) -> Int { 10 + 148 * inputs + 34 * outputs }

    nonisolated static func fee(inputs: Int, outputs: Int, fee: FeeChoice) -> Int64 {
        max(1, rate(fee) * Int64(size(inputs: inputs, outputs: outputs)) / 1000)
    }
}

// MARK: History, receive, coins, address book

final class DemoHistory: HistoryProviding {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    func page(wallet: WalletID, query: HistoryQuery) async throws(ServiceError) -> HistoryPage {
        let all = await world.filteredRecords(wallet: wallet, filter: query.filter, sort: query.sort)
        let start = query.cursor.flatMap(Int.init) ?? 0
        guard start <= all.count else { throw ServiceError(code: .historyStaleCursor, detail: "demo") }
        let end = min(all.count, start + max(1, query.limit))
        return HistoryPage(
            records: Array(all[start..<end]), nextCursor: end < all.count ? String(end) : nil, totalMatching: all.count)
    }

    func detail(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetail {
        try await world.detail(wallet: wallet, txid: txid)
    }

    func setLabel(wallet: WalletID, txid: String, label: String?) async throws(ServiceError) {
        await world.setLabel(wallet: wallet, txid: txid, label: label)
    }

    func changes(wallet: WalletID) -> AsyncStream<[String]> {
        let source = world.historyChanges.stream()
        return AsyncStream { continuation in
            let task = Task {
                for await (changed, txids) in source where changed == wallet { continuation.yield(txids) }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

extension DemoWorld {
    func records(of wallet: WalletID) -> [TxRecord] {
        let labels = current.txLabels
        return (current.records[wallet] ?? []).map { record in
            guard let label = labels[record.id.txid] else { return record }
            return TxRecord(
                id: record.id, type: record.type, category: record.category, status: record.status, date: record.date,
                blockHeight: record.blockHeight, amount: record.amount, fee: record.fee, address: record.address,
                label: label.isEmpty ? nil : label, countsTowardBalance: record.countsTowardBalance,
                involvesWatchOnly: record.involvesWatchOnly)
        }
    }

    func filteredRecords(wallet: WalletID, filter: HistoryFilter, sort: HistorySort) -> [TxRecord] {
        let text = filter.text?.lowercased()
        let matching = records(of: wallet).filter { record in
            if !filter.types.isEmpty, !filter.types.contains(record.type) { return false }
            if !filter.categories.isEmpty, !filter.categories.contains(record.category) { return false }
            if !filter.statuses.isEmpty, !filter.statuses.contains(record.status.kind) { return false }
            if let from = filter.from, (record.date ?? .distantFuture) < from { return false }
            if let until = filter.until, (record.date ?? .distantFuture) >= until { return false }
            if let minimum = filter.minimumAmount, abs(record.amount.duffs) < minimum.duffs { return false }
            if let text, !text.isEmpty {
                let haystack = [record.id.txid, record.address ?? "", record.label ?? ""].joined(separator: " ").lowercased()
                if !haystack.contains(text) { return false }
            }
            return true
        }
        switch sort {
        case .newestFirst: return matching.sorted { ($0.date ?? .distantFuture) > ($1.date ?? .distantFuture) }
        case .oldestFirst: return matching.sorted { ($0.date ?? .distantFuture) < ($1.date ?? .distantFuture) }
        case .amountDescending: return matching.sorted { $0.amount > $1.amount }
        case .amountAscending: return matching.sorted { $0.amount < $1.amount }
        }
    }

    func detail(wallet: WalletID, txid: String) throws(ServiceError) -> TransactionDetail {
        let records = records(of: wallet).filter { $0.id.txid == txid }
        guard let record = records.first else { throw ServiceError(code: .historyTxNotFound, detail: "demo") }
        let incoming = record.amount.duffs > 0
        let value = Amount(duffs: abs(record.amount.duffs) - (record.fee?.duffs ?? 0))
        var outputs = [
            TxOutput(vout: 0, address: record.address, amount: incoming ? record.amount : value, isMine: incoming, isChange: false, dataHex: nil)
        ]
        if !incoming {
            outputs.append(
                TxOutput(vout: 1, address: Self.ownAddresses[5], amount: Amount(duffs: 1_0000_0000), isMine: true, isChange: true, dataHex: nil))
        }
        let input = TxInput(
            previousOutput: OutPoint(txid: Self.hex64(seed: UInt64(txid.count) &+ 7), vout: 0), address: nil,
            amount: nil, isMine: !incoming)
        return TransactionDetail(
            txid: txid, records: records, status: record.status, date: record.date, blockHeight: record.blockHeight,
            blockHash: record.blockHeight.map { Self.hex64(seed: UInt64($0)) }, fee: record.fee,
            sizeBytes: incoming ? 226 : 374, inputs: [input], outputs: outputs, message: nil, label: record.label,
            rawHex: "")
    }

    func setLabel(wallet: WalletID, txid: String, label: String?) {
        current.txLabels[txid] = label ?? ""
        historyChanges.yield((wallet, [txid]))
    }
}

final class DemoReceive: ReceiveProviding {
    let world: DemoWorld
    let uri: any URIHandling

    init(world: DemoWorld, uri: any URIHandling) {
        self.world = world
        self.uri = uri
    }

    func currentAddress(wallet: WalletID) async throws(ServiceError) -> AddressInfo {
        try await world.address(wallet: wallet, advance: false)
    }

    func nextAddress(wallet: WalletID, label: String?) async throws(ServiceError) -> AddressInfo {
        try await world.address(wallet: wallet, advance: true)
    }

    func addresses(wallet: WalletID, filter: AddressFilter) async throws(ServiceError) -> [AddressInfo] {
        let all = await world.current.addresses[wallet] ?? []
        return all.filter { info in
            (filter.chain == nil || filter.chain == info.chain) && (filter.used == nil || filter.used == info.used)
        }
    }

    func createRequest(wallet: WalletID, amount: Amount?, label: String?, message: String?) async throws(ServiceError)
        -> ReceiveRequest
    {
        let address = try await world.address(wallet: wallet, advance: true).address
        let text = try uri.buildPaymentURI(address: address, amount: amount, label: label, message: message)
        return await world.addRequest(
            wallet: wallet, address: address, amount: amount, label: label, message: message, uri: text)
    }

    func requests(wallet: WalletID) async throws(ServiceError) -> [ReceiveRequest] {
        let requests = await world.current.requests[wallet]
        return requests ?? []
    }

    func deleteRequest(wallet: WalletID, id: UInt64) async throws(ServiceError) {
        await world.deleteRequest(wallet: wallet, id: id)
    }
}

extension DemoWorld {
    func address(wallet: WalletID, advance: Bool) throws(ServiceError) -> AddressInfo {
        let addresses = current.addresses[wallet] ?? []
        var issued = current.issuedAddresses[wallet] ?? 1
        if advance { issued += 1 }
        guard issued <= addresses.count, issued > 0 else {
            throw ServiceError(code: ServiceErrorCode(rawValue: "receive.gap_limit"), detail: "demo has 6 addresses")
        }
        current.issuedAddresses[wallet] = issued
        return addresses[issued - 1]
    }

    func addRequest(
        wallet: WalletID, address: String, amount: Amount?, label: String?, message: String?, uri: String
    ) -> ReceiveRequest {
        let id = UInt64((current.requests[wallet]?.map(\.id).max() ?? 0) + 1)
        let request = ReceiveRequest(
            id: id, createdAt: Date(), address: address, amount: amount, label: label, message: message, uri: uri)
        current.requests[wallet, default: []].append(request)
        return request
    }

    func deleteRequest(wallet: WalletID, id: UInt64) {
        current.requests[wallet]?.removeAll { $0.id == id }
    }
}

final class DemoCoinControl: CoinControlProviding {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    func utxos(wallet: WalletID, filter: UtxoFilter) async throws(ServiceError) -> [Utxo] {
        let all = await world.records(of: wallet)
        let records = all.filter { $0.amount.duffs > 0 }.suffix(4)
        let locked = await world.current.lockedOutpoints[wallet] ?? []
        return records.map { record in
            let outpoint = OutPoint(txid: record.id.txid, vout: 0)
            return Utxo(
                outpoint: outpoint, address: record.address ?? "", amount: record.amount,
                confirmations: record.status.confirmations, date: record.date, instantLocked: record.status.instantLocked,
                chainLocked: record.status.chainLocked, userLocked: locked.contains(outpoint), reserved: false,
                label: record.label, isChange: false, coinJoinDenominated: false, coinJoinRounds: nil, spendable: true)
        }.filter { filter.includeLocked || !$0.userLocked }
    }

    func lock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        await world.updateLocks(wallet: wallet) { $0.formUnion(outpoints) }
    }

    func unlock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        await world.updateLocks(wallet: wallet) { $0.subtract(outpoints) }
    }

    func lockedOutpoints(wallet: WalletID) async throws(ServiceError) -> [OutPoint] {
        let locked = await world.current.lockedOutpoints[wallet]
        return Array(locked ?? [])
    }
}

extension DemoWorld {
    func updateLocks(wallet: WalletID, _ change: (inout Set<OutPoint>) -> Void) {
        change(&current.lockedOutpoints[wallet, default: []])
    }
}

final class DemoAddressBook: AddressBookProviding {
    let world: DemoWorld

    init(world: DemoWorld) { self.world = world }

    func entries(wallet: WalletID, purpose: AddressPurpose?, search: String?) async throws(ServiceError) -> [AddressBookEntry] {
        let book = await world.current.book[wallet]
        let all = book ?? []
        let needle = search?.lowercased() ?? ""
        return all.filter { entry in
            (purpose == nil || entry.purpose == purpose)
                && (needle.isEmpty || entry.address.lowercased().contains(needle) || entry.label.lowercased().contains(needle))
        }
    }

    func save(wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool)
        async throws(ServiceError) -> AddressBookEntry
    {
        try await world.saveEntry(wallet: wallet, address: address, label: label, purpose: purpose, replace: replace)
    }

    func delete(wallet: WalletID, address: String) async throws(ServiceError) {
        try await world.deleteEntry(wallet: wallet, address: address)
    }
}

extension DemoWorld {
    func saveEntry(wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool)
        throws(ServiceError) -> AddressBookEntry
    {
        var book = current.book[wallet] ?? []
        if let index = book.firstIndex(where: { $0.address == address }) {
            guard replace else { throw ServiceError(code: .labelsDuplicateAddress, detail: "demo") }
            book[index] = AddressBookEntry(address: address, label: label, purpose: book[index].purpose, createdAt: book[index].createdAt)
            current.book[wallet] = book
            return book[index]
        }
        let entry = AddressBookEntry(address: address, label: label, purpose: purpose, createdAt: Date())
        book.append(entry)
        current.book[wallet] = book
        return entry
    }

    func deleteEntry(wallet: WalletID, address: String) throws(ServiceError) {
        guard let entry = current.book[wallet]?.first(where: { $0.address == address }) else {
            throw ServiceError(code: ServiceErrorCode(rawValue: "labels.entry_not_found"), detail: "demo")
        }
        guard entry.purpose == .send else {
            throw ServiceError(code: ServiceErrorCode(rawValue: "labels.receive_entry_not_deletable"), detail: "demo")
        }
        current.book[wallet]?.removeAll { $0.address == address }
    }
}
