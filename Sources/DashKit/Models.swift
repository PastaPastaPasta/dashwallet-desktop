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

    public static let zero = WalletBalances(
        confirmed: .zero, unconfirmed: .zero, immature: .zero, locked: .zero, total: .zero)

    public init(confirmed: Amount, unconfirmed: Amount, immature: Amount, locked: Amount, total: Amount) {
        self.confirmed = confirmed
        self.unconfirmed = unconfirmed
        self.immature = immature
        self.locked = locked
        self.total = total
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
            total: try amount(ffi.total))
    }
}

/// M0 wallet list row (superseded by `WalletInfo`).
public struct WalletSummary: Equatable, Sendable {
    public let walletID: WalletID
    public let balances: WalletBalances
}

/// A wallet from the M0 `createWallet` call. The phrase crosses the FFI as a
/// `String` there (review M5); B removes the call in favour of
/// `generateMnemonic` + `importWallet`, which use bytes only.
public struct CreatedWallet: Sendable {
    public let walletID: WalletID
    public let mnemonic: SecretBytes
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

    init(_ ffi: DashWalletCore.NoticeCode) {
        switch ffi {
        case .platformContextUnavailable: self = .platformContextUnavailable
        case .spvError: self = .spvError
        case .uncleanShutdown: self = .uncleanShutdown
        case .syncStalled: self = .syncStalled
        case .backupFailed: self = .backupFailed
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
    case walletChanged(DashNetwork, WalletID)
    case spvStateChanged(DashNetwork, running: Bool)
    case syncProgress(DashNetwork, headerTipHeight: UInt32?, synced: Bool)
    case peersChanged(DashNetwork, connected: UInt32)
    case notice(DashNetwork?, NoticeCode, detail: String)
    case syncChanged(DashNetwork)
    case balancesChanged(DashNetwork, WalletID)
    /// `txids` empty = reload everything.
    case historyChanged(DashNetwork, WalletID, txids: [String])
    case walletRemoved(DashNetwork, WalletID)
    case lockStateChanged(DashNetwork)
    /// Not from the engine: `EventBus` dropped signals for a slow consumer.
    /// Re-query everything (sync, balances, history, wallets).
    case resynchronize

    init(_ ffi: DashWalletCore.EngineEvent) {
        switch ffi {
        case .sessionOpened(let n): self = .sessionOpened(DashNetwork(n))
        case .sessionClosed(let n): self = .sessionClosed(DashNetwork(n))
        case .walletCreated(let n, let id): self = .walletCreated(DashNetwork(n), WalletID(engine: id))
        case .walletChanged(let n, let id): self = .walletChanged(DashNetwork(n), WalletID(engine: id))
        case .spvStateChanged(let n, let running): self = .spvStateChanged(DashNetwork(n), running: running)
        case .syncProgress(let n, let tip, let synced):
            self = .syncProgress(DashNetwork(n), headerTipHeight: tip, synced: synced)
        case .peersChanged(let n, let connected): self = .peersChanged(DashNetwork(n), connected: connected)
        case .notice(let n, let code, let detail):
            self = .notice(n.map(DashNetwork.init), NoticeCode(code), detail: detail)
        case .sync(let n, _): self = .syncChanged(DashNetwork(n))
        case .balances(let n, let id, _): self = .balancesChanged(DashNetwork(n), WalletID(engine: id))
        case .historyChanged(let n, let id, let txids):
            self = .historyChanged(DashNetwork(n), WalletID(engine: id), txids: txids)
        case .walletRemoved(let n, let id): self = .walletRemoved(DashNetwork(n), WalletID(engine: id))
        case .lockState(let n, _): self = .lockStateChanged(DashNetwork(n))
        }
    }

    /// The network the event concerns, if any.
    public var network: DashNetwork? {
        switch self {
        case .sessionOpened(let n), .sessionClosed(let n), .walletCreated(let n, _), .walletChanged(let n, _),
             .spvStateChanged(let n, _), .syncProgress(let n, _, _), .peersChanged(let n, _),
             .syncChanged(let n), .balancesChanged(let n, _), .historyChanged(let n, _, _),
             .walletRemoved(let n, _), .lockStateChanged(let n):
            n
        case .notice(let n, _, _):
            n
        case .resynchronize:
            nil
        }
    }

    /// Lifecycle events are never dropped or merged by `EventBus`; the rest
    /// are "re-query" signals.
    public var isLifecycle: Bool {
        switch self {
        case .sessionOpened, .sessionClosed, .walletCreated, .walletRemoved, .spvStateChanged, .lockStateChanged,
             .notice:
            true
        case .walletChanged, .syncProgress, .peersChanged, .syncChanged, .balancesChanged, .historyChanged,
             .resynchronize:
            false
        }
    }
}
