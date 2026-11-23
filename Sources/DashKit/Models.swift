import DashWalletCore
import Foundation

/// A Dash network. Each has its own data directory and wallets.
public enum DashNetwork: Hashable, Sendable, CustomStringConvertible {
    case mainnet
    case testnet
    case devnet(name: String)
    case regtest

    public var description: String {
        switch self {
        case .mainnet: "mainnet"
        case .testnet: "testnet"
        case .devnet(let name): "devnet-\(name)"
        case .regtest: "regtest"
        }
    }

    var ffi: DashWalletCore.DashNetwork {
        switch self {
        case .mainnet: .mainnet
        case .testnet: .testnet
        case .devnet(let name): .devnet(name: name)
        case .regtest: .regtest
        }
    }

    init(_ ffi: DashWalletCore.DashNetwork) {
        switch ffi {
        case .mainnet: self = .mainnet
        case .testnet: self = .testnet
        case .devnet(let name): self = .devnet(name: name)
        case .regtest: self = .regtest
        }
    }
}

/// Network-scoped wallet identifier (64 lowercase hex characters).
public struct WalletID: Hashable, Sendable, CustomStringConvertible {
    public let hex: String

    public init?(hex: String) {
        let lower = hex.lowercased()
        guard lower.count == 64, lower.allSatisfy({ $0.isHexDigit && $0.isASCII }) else { return nil }
        self.hex = lower
    }

    /// Engine-produced ids are always valid hex; used only on values from Rust.
    init(engine hex: String) {
        self.hex = hex
    }

    public var description: String { hex }
}

/// Core balance buckets.
public struct WalletBalances: Hashable, Sendable {
    public let confirmed: Amount
    public let unconfirmed: Amount
    public let immature: Amount
    public let locked: Amount
    public let total: Amount
    /// Spendable balance of the CoinJoin accounts (no "fully mixed" rule yet).
    public let coinjoin: Amount

    public static let zero = WalletBalances(
        confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero, coinjoin: .zero)

    public init(
        confirmed: Amount, unconfirmed: Amount, immature: Amount, locked: Amount, total: Amount,
        coinjoin: Amount = .zero
    ) {
        self.confirmed = confirmed
        self.unconfirmed = unconfirmed
        self.immature = immature
        self.locked = locked
        self.total = total
        self.coinjoin = coinjoin
    }

    init(_ ffi: DashWalletCore.WalletBalances) throws(DashKitError) {
        func amount(_ v: UInt64) throws(DashKitError) -> Amount {
            guard let a = Amount(exactly: v) else {
                throw .internal(detail: "balance \(v) exceeds Int64 range")
            }
            return a
        }
        self.init(
            confirmed: try amount(ffi.confirmed),
            unconfirmed: try amount(ffi.unconfirmed),
            immature: try amount(ffi.immature),
            locked: try amount(ffi.locked),
            total: try amount(ffi.total),
            coinjoin: try amount(ffi.coinjoin))
    }
}

/// How a session reaches the network. Empty lists mean network defaults
/// (devnet/regtest have none).
public struct SessionOptions: Sendable, Equatable {
    public var dapiAddresses: [String]
    public var quorumURL: String?
    public var spvPeers: [String]

    public init(dapiAddresses: [String] = [], quorumURL: String? = nil, spvPeers: [String] = []) {
        self.dapiAddresses = dapiAddresses
        self.quorumURL = quorumURL
        self.spvPeers = spvPeers
    }

    var ffi: DashWalletCore.SessionOptions {
        .init(dapiAddresses: dapiAddresses, quorumUrl: quorumURL, spvPeers: spvPeers)
    }
}

public enum NoticeCode: Sendable, Equatable {
    case platformContextUnavailable
    case spvError
    case uncleanShutdown
    case syncStalled
    case backupFailed
    /// `removeWallet` removed the wallet but its seed is still in the vault;
    /// `detail` names the wallet id (engine review M8).
    case walletSecretNotDeleted

    init(_ ffi: DashWalletCore.NoticeCode) {
        switch ffi {
        case .platformContextUnavailable: self = .platformContextUnavailable
        case .spvError: self = .spvError
        case .uncleanShutdown: self = .uncleanShutdown
        case .syncStalled: self = .syncStalled
        case .backupFailed: self = .backupFailed
        case .walletSecretNotDeleted: self = .walletSecretNotDeleted
        }
    }
}

/// Engine signal. Consumers re-query state when one arrives. The M1 events
/// (`syncChanged`, `balancesChanged`, `historyChanged`, `walletRemoved`,
/// `lockStateChanged`) carry no payload here: the data is pulled again.
public enum EngineEvent: Sendable, Hashable {
    case sessionOpened(DashNetwork)
    case sessionClosed(DashNetwork)
    case walletCreated(DashNetwork, WalletID)
    case spvStateChanged(DashNetwork, running: Bool)
    case notice(DashNetwork?, NoticeCode, detail: String)
    case syncChanged(DashNetwork)
    case balancesChanged(DashNetwork, WalletID)
    /// `txids` empty = reload everything.
    case historyChanged(DashNetwork, WalletID, txids: [String])
    case walletRemoved(DashNetwork, WalletID)
    case lockStateChanged(DashNetwork)
    /// M2: transactions seen for the first time (m2-engine.md §3). Carries
    /// the txids because notification rows cannot be re-derived from a
    /// later re-query; `catchUp` = SPV was not caught up.
    case newTransactions(DashNetwork, WalletID, txids: [String], catchUp: Bool)
    /// M2: a wallet was loaded (opened) or unloaded (closed) without removal.
    case walletLoadChanged(DashNetwork, WalletID, loaded: Bool)
    /// M3: the wallet's mixing status changed; re-query `coinjoin_status`
    /// (m3-engine.md §3).
    case coinJoinChanged(DashNetwork, WalletID)
    /// Not from the engine: `EventBus` dropped signals for a slow consumer.
    /// Re-query everything (sync, balances, history, wallets).
    case resynchronize

    init(_ ffi: DashWalletCore.EngineEvent) {
        switch ffi {
        case .sessionOpened(let n): self = .sessionOpened(DashNetwork(n))
        case .sessionClosed(let n): self = .sessionClosed(DashNetwork(n))
        case .walletCreated(let n, let id): self = .walletCreated(DashNetwork(n), WalletID(engine: id))
        case .spvStateChanged(let n, let running): self = .spvStateChanged(DashNetwork(n), running: running)
        case .notice(let n, let code, let detail):
            self = .notice(n.map(DashNetwork.init), NoticeCode(code), detail: detail)
        case .sync(let n, _): self = .syncChanged(DashNetwork(n))
        case .balances(let n, let id, _): self = .balancesChanged(DashNetwork(n), WalletID(engine: id))
        case .historyChanged(let n, let id, let txids):
            self = .historyChanged(DashNetwork(n), WalletID(engine: id), txids: txids)
        case .walletRemoved(let n, let id): self = .walletRemoved(DashNetwork(n), WalletID(engine: id))
        case .lockState(let n, _): self = .lockStateChanged(DashNetwork(n))
        case .newTransactions(let n, let id, let txids, let catchUp):
            self = .newTransactions(DashNetwork(n), WalletID(engine: id), txids: txids, catchUp: catchUp)
        case .walletLoadChanged(let n, let id, let loaded):
            self = .walletLoadChanged(DashNetwork(n), WalletID(engine: id), loaded: loaded)
        case .coinJoin(let n, let id): self = .coinJoinChanged(DashNetwork(n), WalletID(engine: id))
        }
    }

    /// The network the event concerns, if any.
    public var network: DashNetwork? {
        switch self {
        case .sessionOpened(let n), .sessionClosed(let n), .walletCreated(let n, _),
             .spvStateChanged(let n, _), .syncChanged(let n), .balancesChanged(let n, _), .historyChanged(let n, _, _),
             .walletRemoved(let n, _), .lockStateChanged(let n), .newTransactions(let n, _, _, _),
             .walletLoadChanged(let n, _, _), .coinJoinChanged(let n, _):
            n
        case .notice(let n, _, _):
            n
        case .resynchronize:
            nil
        }
    }

    /// Lifecycle events are never dropped or merged by `EventBus`; the rest
    /// are "re-query" signals. `newTransactions` counts as lifecycle because
    /// its txids cannot be re-queried.
    public var isLifecycle: Bool {
        switch self {
        case .sessionOpened, .sessionClosed, .walletCreated, .walletRemoved, .spvStateChanged, .lockStateChanged,
             .notice, .newTransactions, .walletLoadChanged:
            true
        case .syncChanged, .balancesChanged, .historyChanged, .coinJoinChanged, .resynchronize:
            false
        }
    }
}
