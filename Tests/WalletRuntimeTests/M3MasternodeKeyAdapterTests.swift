// The live M3 wiring (m3-swift.md §1) and the keychain adapter's checks
// before it reaches the engine (m3-swift.md §2.3).
import DashKit
import Foundation
import Testing
@testable import WalletRuntime

@MainActor
struct M3MasternodeKeyAdapterTests {
    @Test func IOS083_rolesKeepTheirMeaningThroughDashKit() {
        for role in MasternodeKeyRole.allCases {
            #expect(MasternodeKeyRole(role.kit) == role)
        }
    }

    @Test func fakeEnginesHaveNoM3Adapters() {
        #expect(Harness().services.m3 == nil)
    }

    @Test func IOS083_liveAdaptersCheckTheGrantAndTheNetwork() async throws {
        let dir = TempDir()
        let services = try WalletRuntimeServices.live(dataRoot: dir.url, workerThreads: 2)
        let m3 = try #require(services.m3)
        let wallet = WalletRuntime.WalletID(Fixtures.walletA)
        let sign = WalletRuntime.AuthGrant(id: "g", purpose: WalletRuntime.GrantPurpose.signMessage, expiresAt: .distantFuture, singleUse: true)
        do throws(ServiceError) {
            _ = try await m3.keychain.reveal(wallet: wallet, role: .owner, index: 0, grant: sign)
            Issue.record("expected an error")
        } catch {
            #expect(error.code == .vaultGrantPurposeMismatch)
        }
        // No network is open yet.
        do throws(ServiceError) {
            _ = try await m3.keychain.keys(wallet: wallet, role: .owner, range: 0..<1)
            Issue.record("expected an error")
        } catch {
            #expect(error.code == .networkNotOpen)
        }
        try await services.shutdown()
    }
}
