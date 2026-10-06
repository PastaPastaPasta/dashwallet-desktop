// Shared fixtures: a fake environment, records and polling helpers.
import Foundation
import Testing
import WalletFeatures
import WalletRuntime

let walletA = WalletID(hex: String(repeating: "aa", count: 32))!
let walletB = WalletID(hex: String(repeating: "bb", count: 32))!
/// Valid testnet P2PKH / P2SH addresses for `FakeURI` (34 characters).
let testnetAddress1 = "yRecipientAddress00000000000000001"
let testnetAddress2 = "yRecipientAddress00000000000000002"
let testnetScriptAddress = "8RecipientScript000000000000000001"
let mainnetAddress = "XMainnetAddress0000000000000000001"

func walletInfo(_ id: WalletID, name: String = "Main", watchOnly: Bool = false) -> WalletInfo {
    WalletInfo(
        id: id, name: name, watchOnly: watchOnly, hasMnemonic: !watchOnly, hd: true, birthHeight: 0, createdAt: nil,
        balances: WalletBalances(confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero))
}

func balances(confirmed: Int64, unconfirmed: Int64 = 0, immature: Int64 = 0) -> WalletBalances {
    WalletBalances(
        confirmed: Amount(duffs: confirmed), unconfirmed: Amount(duffs: unconfirmed),
        immature: Amount(duffs: immature), locked: .zero, total: Amount(duffs: confirmed + unconfirmed + immature))
}

func record(
    _ txid: String, type: TxType = .recvWithAddress, amount: Int64, index: UInt32 = 0, date: Date? = nil,
    label: String? = nil, address: String? = testnetAddress1, counts: Bool = true, watchOnly: Bool = false,
    status: TxStatus = TxStatus(kind: .confirmed, confirmations: 10, instantLocked: false, chainLocked: true, maturesIn: nil)
) -> TxRecord {
    TxRecord(
        id: TxRecord.ID(txid: txid, recordIndex: index), type: type, category: amount < 0 ? .sent : .received,
        status: status, date: date, blockHeight: 100, amount: Amount(duffs: amount), fee: nil, address: address,
        label: label, countsTowardBalance: counts, involvesWatchOnly: watchOnly)
}

func txid(_ n: Int) -> String {
    let hex = String(n, radix: 16)
    return String(repeating: "0", count: 64 - hex.count) + hex
}

/// Records sleeps and lets the test decide when each one returns.
final class ManualSleeper: @unchecked Sendable {
    private struct Pending {
        let id: UUID
        let continuation: CheckedContinuation<Void, any Error>
    }

    private let pending = Locked<[Pending]>([])
    let requested = Locked<[Duration]>([])

    /// Suspends until `fireNext()`; a cancelled task returns with `CancellationError`.
    var sleeper: Sleeper {
        { [self] duration in
            requested.withLock { $0.append(duration) }
            let id = UUID()
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    pending.withLock { $0.append(Pending(id: id, continuation: continuation)) }
                    if Task.isCancelled { resume(id, throwing: CancellationError()) }
                }
            } onCancel: {
                resume(id, throwing: CancellationError())
            }
        }
    }

    private func resume(_ id: UUID, throwing error: any Error) {
        let match = pending.withLock { list -> Pending? in
            guard let index = list.firstIndex(where: { $0.id == id }) else { return nil }
            return list.remove(at: index)
        }
        match?.continuation.resume(throwing: error)
    }

    var pendingCount: Int { pending.current.count }

    /// Returns from the oldest pending sleep.
    func fireNext() {
        let next = pending.withLock { $0.isEmpty ? nil : $0.removeFirst() }
        next?.continuation.resume()
    }
}

/// A clock the test moves by hand.
final class TestClock: @unchecked Sendable {
    let date = Locked(Date(timeIntervalSince1970: 1_760_000_000))

    func advance(_ seconds: TimeInterval) {
        date.withLock { $0 = $0.addingTimeInterval(seconds) }
    }

    var now: Date { date.current }
}

/// Polls `condition` on the main actor, yielding between checks, until it
/// holds or about two seconds passed.
@MainActor
func eventually(_ condition: @MainActor () -> Bool, sourceLocation: SourceLocation = #_sourceLocation) async {
    for _ in 0..<2000 {
        if condition() { return }
        try? await Task.sleep(for: .milliseconds(1))
    }
    Issue.record("condition not met in time", sourceLocation: sourceLocation)
}

/// Every fake, wired into an `AppEnvironment`.
@MainActor
final class FakeWorld {
    let clock = TestClock()
    let sleeper = ManualSleeper()
    let host = FakeHost()
    let lifecycle: FakeLifecycle
    let walletState: FakeWalletState
    let sync: FakeSync
    let vault = FakeVault()
    let auth: FakeAuth
    let sender: FakeSender
    let history = FakeHistory()
    let receive = FakeReceive()
    let coinControl = FakeCoinControl()
    let addressBook = FakeAddressBook()
    let messages = FakeMessages()
    let uri: FakeURI
    let amounts: AmountFormatter
    let settings = FakeSettings()
    let preferences = FakePreferences()
    let screenCapture = FakeScreenCapture()
    let network: DashNetwork

    init(network: DashNetwork = .testnet, wallets: [WalletInfo]? = [walletInfo(walletA)], selected: WalletID? = walletA) {
        self.network = network
        host.network.withLock { $0 = network }
        lifecycle = FakeLifecycle(host: host)
        walletState = FakeWalletState(wallets: wallets, selected: selected, balances: nil)
        sync = FakeSync(status: FakeSync.synced())
        auth = FakeAuth(lockState: .unencrypted)
        uri = FakeURI(network: network)
        sender = FakeSender(addresses: uri)
        amounts = AmountFormatter(network: network)
    }

    var timing: Timing {
        let clock = clock
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return Timing(
            now: { clock.now }, sleep: sleeper.sleeper, calendar: calendar, timeZone: TimeZone(identifier: "UTC")!)
    }

    func environment(developerMode: Bool = false) -> AppEnvironment {
        AppEnvironment(
            host: host, lifecycle: lifecycle, walletState: walletState, sync: sync, vault: vault, auth: auth,
            sender: sender, history: history, receive: receive, coinControl: coinControl, addressBook: addressBook,
            messages: messages, uri: uri, amounts: amounts, settings: settings, preferences: preferences,
            screenCapture: screenCapture, timing: timing, developerMode: developerMode)
    }
}
