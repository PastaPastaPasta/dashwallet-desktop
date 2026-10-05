// Demo implementations of every M1 service protocol
// (docs/contracts/m1-swift.md §2), backed by `DemoStore`. They behave like
// the real adapters at the seams the view models use (typed errors,
// change streams, prepare-then-broadcast) but never reach an engine.
#if os(macOS)
import Foundation
import PlatformServicesMac
import WalletFeatures
import WalletRuntime

// MARK: Host and lifecycle

final class DemoHost: WalletHosting {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    var activeNetwork: DashNetwork? {
        get async { await store.network }
    }

    func start(network: DashNetwork, options: NetworkOptions) async throws(ServiceError) {}

    func stop() async throws(ServiceError) {}

    func dataDirectory(for network: DashNetwork) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("DashWalletDemo/\(network)", isDirectory: true)
    }
}

final class DemoLifecycle: LifecycleQueueing {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    var transition: LifecycleTransition {
        get async { await store.transition }
    }

    func transitions() -> AsyncStream<LifecycleTransition> {
        store.transitionChanges.stream()
    }

    func start(network: DashNetwork) async throws(ServiceError) {}

    func stop() async throws(ServiceError) {}

    func switchNetwork(to network: DashNetwork) async throws(ServiceError) {
        let from = await store.network
        await store.setTransition(.switchingNetwork(from: from, to: network))
        try? await Task.sleep(for: .milliseconds(600))
        await store.switchNetwork(to: network)
        await store.setTransition(.idle)
    }

    func importWallet(
        mnemonic: any SecretBuffer, bip39Passphrase: any SecretBuffer, options: WalletImportOptions
    ) async throws(ServiceError) -> WalletID {
        let phrase = mnemonic.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
        await store.setTransition(.addingWallet)
        try? await Task.sleep(for: .milliseconds(400))
        let result: Result<WalletID, ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                return .success(try store.importWallet(phrase: phrase, options: options))
            } catch {
                return .failure(error)
            }
        }
        await store.setTransition(.idle)
        return try result.get()
    }

    func removeWallet(_ id: WalletID, grant: AuthGrant) async throws(ServiceError) {
        throw .demo(.notImplemented, "wallet removal is not part of M1")
    }
}

// MARK: Wallet state, sync, auth, settings (main actor)

@MainActor
final class DemoWalletState: WalletStateProviding {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    var wallets: [WalletInfo]? { store.wallets }
    var selectedWalletID: WalletID? { store.selected }
    var balances: WalletBalances? { store.balances }
    func changes() -> AsyncStream<Void> { store.walletChanges.stream() }
    func select(_ id: WalletID) { store.select(id) }
    func rename(_ id: WalletID, to name: String) async throws(ServiceError) { try store.rename(id, to: name) }
}

@MainActor
final class DemoSyncStatus: SyncStatusProviding {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    var status: SyncStatus? { store.sync }

    func changes() -> AsyncStream<SyncStatus> { store.syncChanges.stream(initial: store.sync) }

    func peers() async throws(ServiceError) -> [PeerInfo] {
        (1...8).map { index in
            PeerInfo(
                address: "203.0.113.\(10 + index):19999", userAgent: "/Dash Core:23.1.7/", protocolVersion: 70_235,
                bestHeight: DemoLedger.tipHeight, pingMilliseconds: UInt32(20 + index * 7),
                connectedSince: store.now().addingTimeInterval(-Double(index) * 600), inbound: false)
        }
    }

    func rotatePeers() async throws(ServiceError) {}

    func rescan(from start: RescanStart) async throws(ServiceError) {}
}

@MainActor
final class DemoAuthentication: AuthenticationGating {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    var lockState: VaultLockState? { store.vaultState }

    func lockStateChanges() -> AsyncStream<VaultLockState> { store.lockChanges.stream(initial: store.vaultState) }

    func requirement(for purpose: GrantPurpose) -> CredentialRequirement {
        store.vaultEncrypted ? .passphrase : .none
    }

    func authorize(_ purpose: GrantPurpose, credential: Credential) async throws(ServiceError) -> AuthGrant {
        switch credential {
        case .unencrypted:
            if store.vaultEncrypted { throw .demo(.vaultLocked) }
        case .passphrase(let secret):
            try store.checkPassphrase(Self.text(secret))
        }
        return AuthGrant(
            id: UUID().uuidString, purpose: purpose, expiresAt: store.now().addingTimeInterval(60), singleUse: true)
    }

    func unlock(passphrase: any SecretBuffer, scope: UnlockScope) async throws(ServiceError) {
        try store.checkPassphrase(Self.text(passphrase))
        store.setLockState(scope == .full ? .unlocked : .unlockedMixingOnly)
    }

    func lock() async throws(ServiceError) {
        guard store.vaultEncrypted else { throw .demo(EngineCodes.vaultNotEncrypted) }
        store.setLockState(.locked)
    }

    nonisolated static func text(_ secret: any SecretBuffer) -> String {
        secret.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
    }
}

@MainActor
final class DemoSettings: SettingsProviding, UIPreferencesStoring {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    var display: DisplaySettings { store.display }
    var lastNetwork: DashNetwork? { store.network }
    func update(_ display: DisplaySettings) throws(ServiceError) { store.updateDisplay(display) }
    func changes() -> AsyncStream<DisplaySettings> { store.displayChanges.stream() }

    var preferences: UIPreferences { store.preferences }
    func update(_ preferences: UIPreferences) throws(ServiceError) { store.preferences = preferences }
}

// MARK: Vault

final class DemoVault: VaultProviding {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    func status() async throws(ServiceError) -> VaultStatus { await store.vaultStatus }

    func create(passphrase: (any SecretBuffer)?) async throws(ServiceError) -> VaultStatus {
        let text = passphrase.map(DemoAuthentication.text)
        return try await run { (store: DemoStore) throws(ServiceError) -> VaultStatus in try store.createVault(passphrase: text) }
    }

    func encrypt(newPassphrase: any SecretBuffer, grant: AuthGrant) async throws(ServiceError) -> VaultStatus {
        let text = DemoAuthentication.text(newPassphrase)
        return try await run { (store: DemoStore) throws(ServiceError) -> VaultStatus in try store.encrypt(newPassphrase: text) }
    }

    func changePassphrase(old: any SecretBuffer, new: any SecretBuffer) async throws(ServiceError) -> VaultStatus {
        let oldText = DemoAuthentication.text(old)
        let newText = DemoAuthentication.text(new)
        return try await run { (store: DemoStore) throws(ServiceError) -> VaultStatus in
            try store.changePassphrase(old: oldText, new: newText)
        }
    }

    func revealMnemonic(wallet: WalletID, grant: AuthGrant) async throws(ServiceError) -> RevealedMnemonic {
        guard grant.purpose == .revealSecret else { throw .demo(.vaultGrantInvalid) }
        guard let phrase = await store.phrase(of: wallet) else { throw .demo(.vaultNoSecret) }
        return RevealedMnemonic(phrase: DemoSecret(utf8: phrase), bip39Passphrase: DemoSecret(utf8: ""))
    }

    func generateMnemonic(wordCount: Int, language: MnemonicLanguage) async throws(ServiceError) -> any SecretBuffer {
        guard wordCount == 12 || wordCount == 24 else { throw .demo(.walletUnsupportedWordCount) }
        return DemoSecret(utf8: DemoStore.phrase.prefix(wordCount).joined(separator: " "))
    }

    /// Word-list check only: demo mode does not compute the BIP39 checksum,
    /// so any phrase of known words with a valid length reports `.valid`.
    func checkMnemonic(_ phrase: any SecretBuffer) async throws(ServiceError) -> MnemonicCheck {
        let words = DemoAuthentication.text(phrase).split(separator: " ").map(String.init)
        let unknown = words.indices.filter { !Self.isWord(words[$0]) }
        let validLength = MnemonicWords.restoreCounts.contains(words.count)
        return MnemonicCheck(
            wordCount: words.count, unknownWordIndices: unknown, language: unknown.isEmpty ? .english : nil,
            checksum: unknown.isEmpty && validLength ? .valid : .invalid)
    }

    func makeSecret(utf8 text: String) -> any SecretBuffer { DemoSecret(utf8: text) }

    /// Whether `word` is in the BIP39 English list. Every BIP39 word has at
    /// least three letters, so the prefix without its last letter lists it.
    static func isWord(_ word: String) -> Bool {
        guard word.count >= 3 else { return false }
        return MnemonicWords.suggestions(for: String(word.dropLast()), limit: 2048).contains(word)
    }

    private func run<T: Sendable>(
        _ body: @MainActor @Sendable (DemoStore) throws(ServiceError) -> T
    ) async throws(ServiceError) -> T {
        let result: Result<T, ServiceError> = await MainActor.run {
            do throws(ServiceError) { return .success(try body(store)) } catch { return .failure(error) }
        }
        return try result.get()
    }
}

// MARK: Sending

final class DemoSender: TransactionSending {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    func makeDraft(wallet: WalletID) async throws(ServiceError) -> any TransactionDrafting {
        _ = try await ledger(wallet)
        return DemoDraft(store: store, wallet: wallet)
    }

    func maxSpendable(wallet: WalletID, source: CoinSourceChoice, fee: FeeChoice) async throws(ServiceError) -> Amount {
        let balance = try await ledger(wallet).balances.confirmed
        return Amount(duffs: max(0, balance.duffs - DemoDraft.fee(inputs: 3, outputs: 1, fee: fee).duffs))
    }

    private func ledger(_ wallet: WalletID) async throws(ServiceError) -> DemoLedger {
        let result: Result<DemoLedger, ServiceError> = await MainActor.run {
            do throws(ServiceError) { return .success(try store.ledger(wallet)) } catch { return .failure(error) }
        }
        return try result.get()
    }
}

/// A payment being edited. `prepare` reserves nothing real and never
/// broadcasts; `broadcast` adds the transaction to the demo history.
actor DemoDraft: TransactionDrafting {
    let store: DemoStore
    let wallet: WalletID
    private var recipients: [PaymentRecipient] = []
    private var fee: FeeChoice = .recommended(targetBlocks: 6)
    private var prepared: [UUID: PreparedTxSummary] = [:]

    init(store: DemoStore, wallet: WalletID) {
        self.store = store
        self.wallet = wallet
    }

    func setRecipients(_ recipients: [PaymentRecipient]) throws(ServiceError) {
        guard !recipients.isEmpty else { throw .demo(.sendNoRecipients) }
        self.recipients = recipients
    }

    func setSource(_ source: CoinSourceChoice) throws(ServiceError) {}

    func setFee(_ fee: FeeChoice) throws(ServiceError) {
        self.fee = fee
    }

    func setChange(_ change: ChangeChoice) throws(ServiceError) {}

    func estimate() async throws(ServiceError) -> TxEstimate {
        let total = recipients.reduce(Int64(0)) { $0 + $1.amount.duffs }
        let inputs = UInt32(min(4, 1 + recipients.count))
        let fee = Self.fee(inputs: Int(inputs), outputs: recipients.count + 1, fee: fee)
        let wallet = self.wallet
        let available = await MainActor.run { (try? store.ledger(wallet))?.balances.confirmed.duffs ?? 0 }
        let subtracts = recipients.contains(where: \.subtractFeeFromAmount)
        let debit = total + (subtracts ? 0 : fee.duffs)
        if total > available { throw .demo(.sendAmountExceedsBalance) }
        if debit > available { throw .demo(.sendAmountWithFeeExceedsBalance) }
        return TxEstimate(
            fee: fee, sizeBytes: Self.size(inputs: Int(inputs), outputs: recipients.count + 1), inputCount: inputs,
            change: Amount(duffs: available - debit), totalSent: Amount(duffs: total - (subtracts ? fee.duffs : 0)))
    }

    func prepare(grant: AuthGrant) async throws(ServiceError) -> PreparedTransaction {
        guard case .spend(let limit) = grant.purpose else { throw .demo(.vaultGrantInvalid) }
        let estimate = try await estimate()
        let debit = estimate.totalSent.duffs + estimate.fee.duffs
        if debit > limit.duffs { throw .demo(.sendGrantExceeded) }
        var rng = DemoRandom(text: recipients.map(\.address).joined() + String(debit))
        let outputs = recipients.map {
            PreparedOutput(address: $0.address, amount: $0.amount, isChange: false, label: $0.label)
        } + [PreparedOutput(address: rng.testnetAddress(), amount: estimate.change ?? .zero, isChange: true, label: nil)]
        let rate = Amount(duffs: estimate.fee.duffs * 1000 / Int64(max(1, estimate.sizeBytes)))
        let summary = PreparedTxSummary(
            txid: rng.hex(bytes: 32), fee: estimate.fee, feeRatePerKilobyte: rate, sizeBytes: estimate.sizeBytes,
            inputCount: Int(estimate.inputCount), outputs: outputs, totalSent: estimate.totalSent,
            totalDebit: Amount(duffs: debit))
        let id = UUID()
        prepared[id] = summary
        return PreparedTransaction(id: id, summary: summary)
    }

    func broadcast(_ transaction: PreparedTransaction) async throws(ServiceError) -> BroadcastResult {
        guard let summary = prepared.removeValue(forKey: transaction.id) else {
            throw .demo(ServiceErrorCode(rawValue: "send.prepared_tx_unknown"))
        }
        let recipient = summary.outputs.first { !$0.isChange }
        let wallet = self.wallet
        let result: Result<Void, ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                let now = store.now()
                try store.updateLedger(wallet) { ledger in
                    ledger.addRecord(
                        txid: summary.txid, type: .sendToAddress, date: now, confirmations: 0, instantLocked: true,
                        duffs: -summary.totalDebit.duffs, address: recipient?.address, label: recipient?.label,
                        counterparty: recipient?.address ?? "", index: ledger.records.count)
                    ledger.records.sort { ($0.date ?? .distantPast) > ($1.date ?? .distantPast) }
                }
                store.notifyHistory(wallet, txids: [summary.txid])
                return .success(())
            } catch {
                return .failure(error)
            }
        }
        try result.get()
        return BroadcastResult(txid: summary.txid, peersAnnounced: 8)
    }

    func abandon(_ transaction: PreparedTransaction) throws(ServiceError) {
        prepared[transaction.id] = nil
    }

    static func size(inputs: Int, outputs: Int) -> UInt32 {
        UInt32(10 + inputs * 148 + outputs * 34)
    }

    static func fee(inputs: Int, outputs: Int, fee: FeeChoice) -> Amount {
        let rate: Int64
        switch fee {
        case .recommended(let blocks): rate = blocks <= 2 ? 2000 : 1000
        case .perKilobyte(let amount): rate = amount.duffs
        }
        return Amount(duffs: Int64(size(inputs: inputs, outputs: outputs)) * rate / 1000)
    }
}

// MARK: History, receive, coins, address book

final class DemoHistory: HistoryProviding {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    func page(wallet: WalletID, query: HistoryQuery) async throws(ServiceError) -> HistoryPage {
        try await read(wallet) { $0.page(query) }
    }

    func detail(wallet: WalletID, txid: String) async throws(ServiceError) -> TransactionDetail {
        guard let detail = try await read(wallet, { $0.details[txid] }) else { throw .demo(.historyTxNotFound) }
        return detail
    }

    func setLabel(wallet: WalletID, txid: String, label: String?) async throws(ServiceError) {
        let result: Result<Void, ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                try store.updateLedger(wallet) { ledger in
                    guard let index = ledger.records.firstIndex(where: { $0.id.txid == txid }) else { return }
                    let old = ledger.records[index]
                    ledger.records[index] = TxRecord(
                        id: old.id, type: old.type, category: old.category, status: old.status, date: old.date,
                        blockHeight: old.blockHeight, amount: old.amount, fee: old.fee, address: old.address,
                        label: label, countsTowardBalance: old.countsTowardBalance,
                        involvesWatchOnly: old.involvesWatchOnly)
                    if let detail = ledger.details[txid] {
                        ledger.details[txid] = TransactionDetail(
                            txid: detail.txid, records: [ledger.records[index]], status: detail.status,
                            date: detail.date, blockHeight: detail.blockHeight, blockHash: detail.blockHash,
                            fee: detail.fee, sizeBytes: detail.sizeBytes, inputs: detail.inputs,
                            outputs: detail.outputs, message: detail.message, label: label, rawHex: detail.rawHex)
                    }
                }
                return .success(())
            } catch {
                return .failure(error)
            }
        }
        try result.get()
    }

    func changes(wallet: WalletID) -> AsyncStream<[String]> {
        let source = store.historyChanges.stream()
        return AsyncStream { continuation in
            let task = Task {
                for await (changed, txids) in source where changed == wallet { continuation.yield(txids) }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func read<T: Sendable>(
        _ wallet: WalletID, _ body: @MainActor @Sendable (DemoLedger) -> T
    ) async throws(ServiceError) -> T {
        let result: Result<T, ServiceError> = await MainActor.run {
            do throws(ServiceError) { return .success(body(try store.ledger(wallet))) } catch { return .failure(error) }
        }
        return try result.get()
    }
}

final class DemoReceive: ReceiveProviding {
    let store: DemoStore
    let uri: DemoURIHandler

    init(store: DemoStore, uri: DemoURIHandler) {
        self.store = store
        self.uri = uri
    }

    func currentAddress(wallet: WalletID) async throws(ServiceError) -> AddressInfo {
        try await mutate(wallet) { ledger in
            ledger.addressInfo(receivingIndex: ledger.nextReceiveIndex)
        }
    }

    func nextAddress(wallet: WalletID, label: String?) async throws(ServiceError) -> AddressInfo {
        try await mutate(wallet) { ledger in
            ledger.nextReceiveIndex += 1
            return ledger.addressInfo(receivingIndex: ledger.nextReceiveIndex)
        }
    }

    func addresses(wallet: WalletID, filter: AddressFilter) async throws(ServiceError) -> [AddressInfo] {
        try await mutate(wallet) { ledger in
            (0...ledger.nextReceiveIndex).map { ledger.addressInfo(receivingIndex: $0) }
                .filter { filter.used == nil || $0.used == filter.used }
        }
    }

    func createRequest(
        wallet: WalletID, amount: Amount?, label: String?, message: String?
    ) async throws(ServiceError) -> ReceiveRequest {
        let address = try await nextAddress(wallet: wallet, label: label).address
        let text = try uri.buildPaymentURI(address: address, amount: amount, label: label, message: message)
        let created = await MainActor.run { store.now() }
        return try await mutate(wallet) { ledger in
            let request = ReceiveRequest(
                id: ledger.nextRequestID, createdAt: created, address: address, amount: amount, label: label,
                message: message, uri: text)
            ledger.nextRequestID += 1
            ledger.requests.append(request)
            return request
        }
    }

    func requests(wallet: WalletID) async throws(ServiceError) -> [ReceiveRequest] {
        try await mutate(wallet) { $0.requests }
    }

    func deleteRequest(wallet: WalletID, id: UInt64) async throws(ServiceError) {
        let found = try await mutate(wallet) { ledger in
            let before = ledger.requests.count
            ledger.requests.removeAll { $0.id == id }
            return ledger.requests.count != before
        }
        if !found { throw .demo(EngineCodes.receiveRequestNotFound) }
    }

    private func mutate<T: Sendable>(
        _ wallet: WalletID, _ body: @MainActor @Sendable (inout DemoLedger) -> T
    ) async throws(ServiceError) -> T {
        let result: Result<T, ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                var value: T?
                try store.updateLedger(wallet) { value = body(&$0) }
                return .success(value!)
            } catch {
                return .failure(error)
            }
        }
        return try result.get()
    }
}

final class DemoCoinControl: CoinControlProviding {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    func utxos(wallet: WalletID, filter: UtxoFilter) async throws(ServiceError) -> [Utxo] {
        let result: Result<[Utxo], ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                return .success(try store.ledger(wallet).utxos(now: store.now()))
            } catch {
                return .failure(error)
            }
        }
        return try result.get().filter { filter.includeLocked || !$0.userLocked }
    }

    func lock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        try await update(wallet) { $0.lockedOutpoints.formUnion(outpoints) }
    }

    func unlock(wallet: WalletID, outpoints: [OutPoint]) async throws(ServiceError) {
        try await update(wallet) { $0.lockedOutpoints.subtract(outpoints) }
    }

    func lockedOutpoints(wallet: WalletID) async throws(ServiceError) -> [OutPoint] {
        let result: Result<[OutPoint], ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                return .success(Array(try store.ledger(wallet).lockedOutpoints))
            } catch {
                return .failure(error)
            }
        }
        return try result.get()
    }

    private func update(
        _ wallet: WalletID, _ body: @MainActor @Sendable @escaping (inout DemoLedger) -> Void
    ) async throws(ServiceError) {
        let result: Result<Void, ServiceError> = await MainActor.run {
            do throws(ServiceError) { try store.updateLedger(wallet, body); return .success(()) } catch {
                return .failure(error)
            }
        }
        try result.get()
    }
}

final class DemoAddressBook: AddressBookProviding {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    func entries(wallet: WalletID, purpose: AddressPurpose?, search: String?) async throws(ServiceError)
        -> [AddressBookEntry]
    {
        let result: Result<[AddressBookEntry], ServiceError> = await MainActor.run {
            do throws(ServiceError) { return .success(try store.ledger(wallet).addressBook) } catch {
                return .failure(error)
            }
        }
        let needle = search?.lowercased() ?? ""
        return try result.get().filter {
            (purpose == nil || $0.purpose == purpose)
                && (needle.isEmpty || $0.label.lowercased().contains(needle) || $0.address.lowercased().contains(needle))
        }
    }

    func save(
        wallet: WalletID, address: String, label: String, purpose: AddressPurpose, replace: Bool
    ) async throws(ServiceError) -> AddressBookEntry {
        let result: Result<AddressBookEntry, ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                let now = store.now()
                var outcome: Result<AddressBookEntry, ServiceError> = .failure(.demo(.internal))
                try store.updateLedger(wallet) { ledger in
                    if let index = ledger.addressBook.firstIndex(where: { $0.address == address }) {
                        guard replace else {
                            outcome = .failure(.demo(.labelsDuplicateAddress))
                            return
                        }
                        let old = ledger.addressBook[index]
                        let entry = AddressBookEntry(
                            address: address, label: label, purpose: old.purpose, createdAt: old.createdAt)
                        ledger.addressBook[index] = entry
                        outcome = .success(entry)
                    } else {
                        let entry = AddressBookEntry(address: address, label: label, purpose: purpose, createdAt: now)
                        ledger.addressBook.append(entry)
                        outcome = .success(entry)
                    }
                }
                return outcome
            } catch {
                return .failure(error)
            }
        }
        return try result.get()
    }

    func delete(wallet: WalletID, address: String) async throws(ServiceError) {
        let result: Result<Void, ServiceError> = await MainActor.run {
            do throws(ServiceError) {
                var found = false
                try store.updateLedger(wallet) { ledger in
                    found = ledger.addressBook.contains { $0.address == address }
                    ledger.addressBook.removeAll { $0.address == address }
                }
                return found ? .success(()) : .failure(.demo(EngineCodes.labelsEntryNotFound))
            } catch {
                return .failure(error)
            }
        }
        try result.get()
    }
}

// MARK: Messages, URIs, amounts

final class DemoMessages: MessageSigning {
    let store: DemoStore

    init(store: DemoStore) {
        self.store = store
    }

    func sign(wallet: WalletID, address: String, message: String, grant: AuthGrant) async throws(ServiceError)
        -> String
    {
        guard grant.purpose == .signMessage else { throw .demo(.vaultGrantInvalid) }
        let own = await MainActor.run {
            (try? store.ledger(wallet)).map { $0.receiveAddresses.contains(address) } ?? false
        }
        guard own else { throw .demo(EngineCodes.messageAddressNotMine) }
        return Self.signature(address: address, message: message)
    }

    func verify(address: String, message: String, signature: String) throws(ServiceError) {
        guard let data = Data(base64Encoded: signature), data.count == 65 else {
            throw .demo(EngineCodes.messageMalformedSignature)
        }
        guard signature == Self.signature(address: address, message: message) else {
            throw .demo(.messageNotSigned)
        }
    }

    /// A deterministic 65-byte compact signature shape; not a real signature.
    static func signature(address: String, message: String) -> String {
        var rng = DemoRandom(text: address + "\n" + message)
        return Data([0x1f] + rng.bytes(64)).base64EncodedString()
    }
}

final class DemoURIHandler: URIHandling {
    let network: DemoLocked<DashNetwork>

    init(network: DemoLocked<DashNetwork>) {
        self.network = network
    }

    func parsePaymentURI(_ text: String) throws(ServiceError) -> PaymentURI {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.lowercased().hasPrefix("dash:") else { throw .demo(ServiceErrorCode(rawValue: "uri.not_dash_uri")) }
        var rest = String(trimmed.dropFirst(5))
        if rest.hasPrefix("//") { rest.removeFirst(2) }
        let parts = rest.split(separator: "?", maxSplits: 1).map(String.init)
        let address = parts.first ?? ""
        guard case .core = classifyAddress(address) else { throw .demo(.uriInvalidAddress) }
        var amount: Amount?
        var label: String?
        var message: String?
        if parts.count == 2 {
            for pair in parts[1].split(separator: "&") {
                let kv = pair.split(separator: "=", maxSplits: 1).map(String.init)
                guard kv.count == 2 else { continue }
                let value = kv[1].removingPercentEncoding ?? kv[1]
                switch kv[0] {
                case "amount":
                    guard let duffs = AmountFormatter.parseDuffs(value, unit: .dash), duffs >= 0 else {
                        throw .demo(ServiceErrorCode(rawValue: "uri.invalid_amount"))
                    }
                    amount = Amount(duffs: duffs)
                case "label": label = value
                case "message": message = value
                default: break
                }
            }
        }
        return PaymentURI(address: address, amount: amount, label: label, message: message)
    }

    func buildPaymentURI(address: String, amount: Amount?, label: String?, message: String?) throws(ServiceError)
        -> String
    {
        guard case .core = classifyAddress(address) else { throw .demo(.uriInvalidAddress) }
        var query: [String] = []
        if let amount {
            var text = AmountFormatter.plain(amount.duffs, unit: .dash, plusSign: false, separators: .never, justify: false)
            while text.hasSuffix("0") { text.removeLast() }
            if text.hasSuffix(".") { text.removeLast() }
            query.append("amount=\(text)")
        }
        let allowed = CharacterSet.urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&=?#+"))
        if let label, !label.isEmpty { query.append("label=\(label.addingPercentEncoding(withAllowedCharacters: allowed) ?? label)") }
        if let message, !message.isEmpty {
            query.append("message=\(message.addingPercentEncoding(withAllowedCharacters: allowed) ?? message)")
        }
        return "dash:\(address)" + (query.isEmpty ? "" : "?" + query.joined(separator: "&"))
    }

    /// Prefix, length and alphabet only; demo mode does not check base58 checksums.
    func classifyAddress(_ text: String) -> AddressClass {
        let base58 = Set(DemoRandom.base58)
        guard (26...35).contains(text.count), text.allSatisfy(base58.contains) else {
            return .invalid(.invalidBase58ChecksumOrLength)
        }
        let mainnet = network.current == .mainnet
        switch text.first {
        case "X" where mainnet, "y" where !mainnet: return .core(scriptHash: false)
        case "7" where mainnet, "8" where !mainnet, "9" where !mainnet: return .core(scriptHash: true)
        default: return .invalid(.invalidBase58Prefix)
        }
    }

    func qrMatrix(for text: String) throws(ServiceError) -> QRMatrix {
        // dash-qt's MAX_URI_LENGTH (QT-084).
        guard text.count <= 255 else { throw .demo(EngineCodes.uriTooLongForQR) }
        guard let (size, modules) = MacQRCodeEncoder.modules(for: text) else { throw .demo(.internal, "qr") }
        return QRMatrix(size: size, modules: modules)
    }
}

/// dash-qt formatting for whichever network demo mode is on.
final class DemoAmounts: AmountFormatting {
    let network: DemoLocked<DashNetwork>

    init(network: DemoLocked<DashNetwork>) {
        self.network = network
    }

    func format(_ amount: Amount, unit: DisplayUnit, style: AmountStyle) -> String {
        AmountFormatter(network: network.current).format(amount, unit: unit, style: style)
    }

    func parse(_ text: String, unit: DisplayUnit) throws(ServiceError) -> Amount {
        try AmountFormatter(network: network.current).parse(text, unit: unit)
    }

    func unitName(_ unit: DisplayUnit) -> String { unit.name(on: network.current) }
}
#endif
