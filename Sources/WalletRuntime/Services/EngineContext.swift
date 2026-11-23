// What every query/command adapter needs: the engine and the open network.
import DashKit
import Foundation

/// The engine plus the network its session-scoped calls go to. Adapters
/// read `active` at each call, so a network switch through the lifecycle
/// queue redirects them without rebuilding anything.
struct EngineContext: Sendable {
    let engine: any EngineProtocol
    let active: ActiveNetwork
    /// Network for pure functions (units, URIs, message verification) while
    /// no session has been open yet: they need a network for address
    /// prefixes and unit names, not a session.
    let fallbackNetwork: DashKit.DashNetwork

    /// The open network; `network_not_open` when none is.
    func network() throws(ServiceError) -> DashKit.DashNetwork {
        try active.require()
    }

    /// The open network, else the one open most recently, else the fallback.
    var namingNetwork: DashKit.DashNetwork {
        active.networkOrLast ?? fallbackNetwork
    }

    /// Runs a session-scoped engine call on the open network.
    func call<T>(
        isolation: isolated (any Actor)? = #isolation,
        _ body: (any EngineProtocol, DashKit.DashNetwork) async throws(DashKitError) -> T
    ) async throws(ServiceError) -> T {
        let network = try network()
        let engine = engine
        return try await serviceCall { () async throws(DashKitError) in try await body(engine, network) }
    }
}
