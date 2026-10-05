// In-memory state behind `--demo`: sample wallets, history, addresses and a
// vault with the passphrase "demo". Nothing touches the network or disk, and
// "sent" transactions only exist in this process. The status row says so.
import Foundation
import WalletFeatures
import WalletRuntime

/// How the demo starts.
enum DemoScenario: Sendable, Hashable {
    /// A funded wallet, unlocked.
    case funded
    /// A funded wallet behind the lock screen.
    case locked
    /// No wallet yet: the onboarding flow.
    case onboarding
}

/// Thread-safe fan-out of values to `AsyncStream` subscribers.
final class Broadcast<Element: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<Element>.Continuation] = [:]

    /// A new subscription, starting with `initial` when given.
    func stream(initial: Element? = nil) -> AsyncStream<Element> {
        let (stream, continuation) = AsyncStream.makeStream(of: Element.self, bufferingPolicy: .bufferingNewest(32))
        if let initial { continuation.yield(initial) }
        let id = UUID()
        lock.withLock { continuations[id] = continuation }
        continuation.onTermination = { [weak self] _ in
            guard let self else { return }
            _ = self.lock.withLock { self.continuations.removeValue(forKey: id) }
        }
        return stream
    }

    func yield(_ value: Element) {
        let targets = lock.withLock { Array(continuations.values) }
        for continuation in targets { continuation.yield(value) }
    }
}

/// The active network, shared by the per-network pure services.
final class NetworkBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: DashNetwork

    init(_ network: DashNetwork) { value = network }

    var network: DashNetwork {
        get { lock.withLock { value } }
        set { lock.withLock { value = newValue } }
    }
}

/// A zeroing secret buffer (the live runtime wraps DashKit `SecretBytes`).
final class DemoSecret: SecretBuffer, @unchecked Sendable {
    private var bytes: [UInt8]

    init(_ bytes: [UInt8]) { self.bytes = bytes }
    convenience init(utf8 text: String) { self.init(Array(text.utf8)) }

    var count: Int { bytes.count }

    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try bytes.withUnsafeBytes(body)
    }

    var utf8String: String { String(decoding: bytes, as: UTF8.self) }

    deinit {
        bytes.withUnsafeMutableBytes { _ = $0.initializeMemory(as: UInt8.self, repeating: 0) }
    }
}

/// One network's wallets and their data.
struct DemoNetworkState {
    var wallets: [WalletInfo] = []
    var selected: WalletID?
    var records: [WalletID: [TxRecord]] = [:]
    var txLabels: [String: String] = [:]
    var addresses: [WalletID: [AddressInfo]] = [:]
    var issuedAddresses: [WalletID: Int] = [:]
    var requests: [WalletID: [ReceiveRequest]] = [:]
    var book: [WalletID: [AddressBookEntry]] = [:]
    var lockedOutpoints: [WalletID: Set<OutPoint>] = [:]
}

@MainActor
final class DemoWorld {
    /// Valid testnet P2PKH addresses (base58check, version 140) used as sample data.
    nonisolated static let ownAddresses = [
        "yLYprFXH7G1JokbWvHUJ2Sm2cgAR7cfHj2", "yLeWeiEuMfuxVB7sxAUtmfCyeYhGedG4mw",
        "yMSGUgRUN8cvqLawxWkrWpmcvp2uJFcsyG", "yN8QNovgxsrytHNQPRVDdWyykZM8xwno6B",
        "yNPw9ieC332AeLrovudK1UKE5EBd2DYaNU", "yNZpqbukFJzBUJSca4Q1Ny2YD8qwkBp874",
    ]
    nonisolated static let counterparties = [
        "yPgfYhP6PwdZd8xn1TKDps27nL6kLpvh98", "yPwGGhgGeWFbyVPBkBCgVGwA5o3Hqh9Uxx",
        "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n", "yQYY2kVZWi66s2Vi6hLLw7Vt8CtCnDLApB",
        "yRK85RUPqe6cZ5UChcn93JwP6T9Amnprgy", "yRS8rJBYLbgjkPMR54y6Z1oRywub5eu9Uy",
    ]
    nonisolated static let counterpartyLabels = ["Alice", "Bob's shop", "Exchange deposit"]
    /// BIP39 test vectors; every demo "new wallet" gets one of these.
    nonisolated static let phrase12 = Array(repeating: "abandon", count: 11) + ["about"]
    nonisolated static let phrase24 = Array(repeating: "abandon", count: 23) + ["art"]
    nonisolated static let passphrase = "demo"
    nonisolated static let tipHeight: UInt32 = 1_234_567

    let networkBox: NetworkBox
    let walletChanges = Broadcast<Void>()
    let syncChanges = Broadcast<SyncStatus>()
    let lockChanges = Broadcast<VaultLockState>()
    let settingsChanges = Broadcast<DisplaySettings>()
    let historyChanges = Broadcast<(WalletID, [String])>()
    let transitionChanges = Broadcast<LifecycleTransition>()

    var states: [DashNetwork: DemoNetworkState] = [:]
    var vaults: [DashNetwork: (encrypted: Bool, state: VaultLockState)] = [:]
    var failedAttempts: UInt32 = 0
    var transition: LifecycleTransition = .idle
    var display = DisplaySettings()
    var preferences = UIPreferences()
    let now: Date

    var network: DashNetwork { networkBox.network }

    init(network: DashNetwork, scenario: DemoScenario, now: Date = Date()) {
        self.networkBox = NetworkBox(network)
        self.now = now
        switch scenario {
        case .onboarding:
            states[network] = DemoNetworkState()
            vaults[network] = (false, .noVault)
        case .funded, .locked:
            var state = DemoNetworkState()
            let id = Self.walletID(for: Self.phrase12.joined(separator: " "), network: network)
            Self.populate(&state, wallet: id, name: "Demo wallet", now: now)
            states[network] = state
            vaults[network] = (true, scenario == .locked ? .locked : .unlocked)
        }
        refreshWallets()
    }

    // MARK: Current network

    var current: DemoNetworkState {
        get { states[network] ?? DemoNetworkState() }
        set { states[network] = newValue }
    }

    var vault: (encrypted: Bool, state: VaultLockState) {
        get { vaults[network] ?? (false, .noVault) }
        set {
            vaults[network] = newValue
            lockChanges.yield(newValue.state)
        }
    }

    var syncStatus: SyncStatus {
        let tipDate = now.addingTimeInterval(-90)
        let phases = SyncPhase.allCases.map {
            SyncPhaseProgress(phase: $0, currentHeight: Self.tipHeight, targetHeight: Self.tipHeight, done: true)
        }
        return SyncStatus(
            running: true, phases: phases, activePhase: nil, tipHeight: Self.tipHeight, tipDate: tipDate,
            chainLockHeight: Self.tipHeight, connectedPeers: 8, progress: 1, isDone: true, isStalled: false)
    }

    func balances(of wallet: WalletID) -> WalletBalances {
        let records = current.records[wallet] ?? []
        var confirmed: Int64 = 0
        var unconfirmed: Int64 = 0
        for record in records where record.countsTowardBalance {
            if record.status.kind == .confirmed || record.status.instantLocked {
                confirmed += record.amount.duffs
            } else {
                unconfirmed += record.amount.duffs
            }
        }
        return WalletBalances(
            confirmed: Amount(duffs: confirmed), unconfirmed: Amount(duffs: unconfirmed), immature: .zero,
            locked: .zero, total: Amount(duffs: confirmed + unconfirmed))
    }

    /// Re-derives each wallet's balances after its history changed.
    func refreshWallets() {
        var state = current
        state.wallets = state.wallets.map { info in
            WalletInfo(
                id: info.id, name: info.name, watchOnly: info.watchOnly, hasMnemonic: info.hasMnemonic, hd: info.hd,
                birthHeight: info.birthHeight, createdAt: info.createdAt, balances: balances(of: info.id))
        }
        current = state
        walletChanges.yield(())
    }

    // MARK: Wallets

    func addWallet(phrase: String, name: String?) throws(ServiceError) -> WalletID {
        let id = Self.walletID(for: phrase, network: network)
        guard !current.wallets.contains(where: { $0.id == id }) else {
            throw ServiceError(code: .walletAlreadyExists, detail: "demo wallet exists")
        }
        var state = current
        let isSample = phrase == Self.phrase12.joined(separator: " ")
        if isSample {
            Self.populate(&state, wallet: id, name: name ?? "Demo wallet", now: now)
        } else {
            let info = WalletInfo(
                id: id, name: name ?? "Wallet \(state.wallets.count + 1)", watchOnly: false, hasMnemonic: true,
                hd: true, birthHeight: 0, createdAt: now, balances: Self.emptyBalances)
            state.wallets.append(info)
            state.addresses[id] = Self.addressInfos(Self.ownAddresses)
            state.issuedAddresses[id] = 1
        }
        state.selected = id
        current = state
        refreshWallets()
        return id
    }

    func removeWallet(_ id: WalletID) {
        var state = current
        state.wallets.removeAll { $0.id == id }
        if state.selected == id { state.selected = state.wallets.first?.id }
        current = state
        walletChanges.yield(())
    }

    // MARK: Sample data

    static let emptyBalances = WalletBalances(
        confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero)

    static func walletID(for phrase: String, network: DashNetwork) -> WalletID {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in (network.description + "|" + phrase).utf8 {
            hash = (hash ^ UInt64(byte)) &* 0x0000_0100_0000_01B3
        }
        return WalletID(hex: hex64(seed: hash))!
    }

    /// 64 lowercase hex characters derived from `seed` (sample txids and ids).
    nonisolated static func hex64(seed: UInt64) -> String {
        var state = seed
        var out = ""
        for _ in 0..<4 {
            state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            out += String(format: "%016llx", state)
        }
        return out
    }

    static func addressInfos(_ addresses: [String]) -> [AddressInfo] {
        addresses.enumerated().map { index, address in
            AddressInfo(
                address: address, chain: .receiving, index: UInt32(index),
                derivationPath: "m/44'/1'/0'/0/\(index)", used: false, label: nil, balance: nil, txCount: 0)
        }
    }

    private static func populate(_ state: inout DemoNetworkState, wallet id: WalletID, name: String, now: Date) {
        var records: [TxRecord] = []
        var balance: Int64 = 0
        let day: TimeInterval = 86_400
        // Oldest first: an opening deposit, then a mix of payments.
        let plan: [(TxType, Int64, Int)] = [
            (.recvWithAddress, 25_0000_0000, 0), (.sendToAddress, -1_2500_0000, 1), (.recvWithAddress, 3_7500_0000, 2),
            (.sendToAddress, -2500_0000, 0), (.sendToSelf, -2260, 0), (.recvWithAddress, 1_0000_0000, 1),
            (.sendToAddress, -4_0000_0000, 2), (.recvWithAddress, 5000_0000, 0), (.sendToAddress, -1234_5678, 1),
            (.recvWithAddress, 2_0000_0000, 2), (.sendToAddress, -7500_0000, 0), (.recvWithAddress, 1500_0000, 1),
        ]
        let count = 36
        for index in 0..<count {
            let (type, amount, party) = plan[index % plan.count]
            let isLatest = index == count - 1
            let date = now.addingTimeInterval(-Double(count - index) * 1.7 * day + (isLatest ? 1.6 * day : 0))
            let height = tipHeight - UInt32((count - index) * 720)
            let confirmations = isLatest ? 0 : tipHeight - height + 1
            let fee: Int64? = amount < 0 ? 2260 : nil
            let signed = amount < 0 ? amount - (type == .sendToSelf ? 0 : 2260) : amount
            guard balance + signed >= 0 else { continue }
            balance += signed
            let address: String =
                switch type {
                case .recvWithAddress: ownAddresses[index % ownAddresses.count]
                case .sendToSelf: ownAddresses[(index + 1) % ownAddresses.count]
                default: counterparties[party]
                }
            let category: TxCategory =
                switch type {
                case .recvWithAddress: .received
                case .sendToSelf: .internalTransfer
                default: .sent
                }
            let status = TxStatus(
                kind: isLatest ? .unconfirmed : .confirmed, confirmations: confirmations, instantLocked: !isLatest,
                chainLocked: !isLatest, maturesIn: nil)
            records.append(
                TxRecord(
                    id: .init(txid: hex64(seed: UInt64(index) &+ 0xD45), recordIndex: 0), type: type,
                    category: category, status: status, date: date, blockHeight: isLatest ? nil : height,
                    amount: Amount(duffs: signed), fee: fee.map { Amount(duffs: $0) }, address: address,
                    label: type == .sendToAddress ? counterpartyLabels[party] : nil, countsTowardBalance: true,
                    involvesWatchOnly: false))
        }
        state.wallets.append(
            WalletInfo(
                id: id, name: name, watchOnly: false, hasMnemonic: true, hd: true, birthHeight: tipHeight - 40_000,
                createdAt: now.addingTimeInterval(-70 * day), balances: emptyBalances))
        state.selected = id
        state.records[id] = records
        var addresses = addressInfos(ownAddresses)
        for index in addresses.indices.prefix(3) {
            let info = addresses[index]
            addresses[index] = AddressInfo(
                address: info.address, chain: info.chain, index: info.index, derivationPath: info.derivationPath,
                used: true, label: index == 0 ? "Savings" : nil, balance: nil, txCount: 4)
        }
        state.addresses[id] = addresses
        state.issuedAddresses[id] = 4
        state.book[id] =
            zip(counterparties, counterpartyLabels).map {
                AddressBookEntry(address: $0.0, label: $0.1, purpose: .send, createdAt: now.addingTimeInterval(-30 * day))
            } + [AddressBookEntry(address: ownAddresses[0], label: "Savings", purpose: .receive, createdAt: nil)]
    }
}
