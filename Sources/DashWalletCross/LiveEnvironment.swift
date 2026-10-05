// The live AppEnvironment: WalletRuntime's adapters over the engine
// (DESIGN-opus §1.12). This is where the composition root wires them once
// they exist.
import Foundation
import WalletFeatures
import WalletRuntime

enum LiveEnvironment {
    /// Builds the environment for `network` on `dataRoot`.
    ///
    /// Throws `notImplemented` until WalletRuntime provides its adapters
    /// (WalletHost, LifecycleQueue, SPVCoordinator, WalletState,
    /// AuthenticationGate, TransactionSender, …; workstream C). Main has only
    /// the protocols in `Sources/WalletRuntime/Contracts`, and the app must not
    /// run the M1 screens on stand-ins that pretend to work.
    @MainActor
    static func make(dataRoot: URL, network: DashNetwork) throws(ServiceError) -> AppEnvironment {
        throw ServiceError(
            code: .notImplemented,
            detail: "WalletRuntime adapters are not on this branch; the M1 screens need them (workstream C)")
    }
}
