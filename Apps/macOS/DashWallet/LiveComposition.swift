// Live services for the app: WalletRuntime adapters over the Rust engine.
import Foundation
import WalletFeatures
import WalletRuntime

enum LiveComposition {
    /// The real `AppEnvironment` for `dataDirectory`.
    ///
    /// Throws `not_implemented` until the WalletRuntime adapters land
    /// (docs/contracts/m1-swift.md §2: WalletHost, LifecycleQueue,
    /// SPVCoordinator, WalletState, AuthenticationGate, TransactionSender,
    /// history/receive/coins/labels/message/URI/units adapters and the
    /// settings store). Wire them here, one per `AppEnvironment` field.
    @MainActor
    static func makeEnvironment(dataDirectory: URL) throws(ServiceError) -> AppEnvironment {
        throw ServiceError(
            code: .notImplemented,
            detail: "WalletRuntime adapters are not on this branch; data directory \(dataDirectory.path)")
    }
}
