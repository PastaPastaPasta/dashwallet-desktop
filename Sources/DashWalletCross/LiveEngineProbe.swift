// Live start: opens the real engine on the data directory and reads the
// network's wallets, so the app reports what is on disk even while the
// WalletRuntime adapters it needs for the M1 screens are missing.
import DashKit
import Foundation

/// What opening the engine found.
struct LiveEngineReport: Sendable {
    let dataRoot: URL
    let networkName: String
    let coreVersion: String
    /// `nil` when the engine or the network could not be opened.
    let walletCount: Int?
    /// The DashKit error code when opening failed.
    let failure: String?
}

enum LiveEngineProbe {
    /// Opens the engine, opens `network`, counts its wallets and shuts the
    /// engine down again (so its lock files are released).
    static func run(dataRoot: URL, network: DashKit.DashNetwork) async -> LiveEngineReport {
        func report(_ count: Int?, _ failure: String?) -> LiveEngineReport {
            LiveEngineReport(
                dataRoot: dataRoot, networkName: network.description, coreVersion: EngineClient.coreVersion,
                walletCount: count, failure: failure)
        }
        let engine: EngineClient
        do {
            engine = try EngineClient(dataRoot: dataRoot)
        } catch {
            return report(nil, "\(error.code): \(error)")
        }
        do {
            try await engine.open(network)
            let count = try await engine.wallets(on: network).count
            try await engine.shutdown()
            return report(count, nil)
        } catch {
            try? await engine.shutdown()
            return report(nil, "\(error.code): \(error)")
        }
    }

    static func network(named name: String) -> DashKit.DashNetwork? {
        switch name {
        case "mainnet": .mainnet
        case "testnet": .testnet
        case "regtest": .regtest
        default: name.hasPrefix("devnet-") ? .devnet(name: String(name.dropFirst("devnet-".count))) : nil
        }
    }
}
