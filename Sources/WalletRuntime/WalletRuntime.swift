// Adapter layer (WS-08): Contracts/ holds the protocols and value types view
// models see; Host/, State/ and Services/ implement them on DashKit;
// Composition/WalletRuntimeServices wires one engine to all of them.
import Foundation

public enum WalletRuntimeModule {
    public static let name = "WalletRuntime"
}
