// The Masternode Keys view model (IOS-083) against the M3 fakes.
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Masternode view models")
struct MasternodeViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()
    let m3 = FakeM3World()

    @Test func IOS083_keychainPagesKeysAndRevealsBehindAGrant() async {
        m3.keychain.keyInfos.withLock {
            $0 = (0..<25).map { index in
                MasternodeKeyInfo(
                    role: .voting, index: UInt32(index), derivationPath: "m/9'/1'/3'/1'/\(index)", address: "yVote\(index)",
                    publicKeyHex: "02ab", legacyPublicKeyHex: nil, platformNodeID: nil)
            }
        }
        world.auth.lockState = .unlocked
        let model = MasternodeKeychainViewModel(env: world.environment(), m2: m2.services, m3: m3.services)
        #expect(model.roles == [.owner, .voting, .operator, .platformNode])
        await model.select(.voting)
        #expect(model.keys.count == 20)
        await model.loadMore()
        #expect(model.keys.count == 25)
        await model.reveal(model.keys[2])
        #expect(model.pendingReveal?.index == 2)
        await model.reveal(model.keys[2], passphrase: "pw")
        #expect(model.revealed["m/9'/1'/3'/1'/2"]?.privateKeyHex.testString == "priv-voting-2")
        #expect(world.auth.authorizeCalls.last?.purpose == .revealSecret)
        await model.select(.owner)
        #expect(model.revealed.isEmpty)
    }

    @Test func IOS083_withoutAnAdapterTheToolIsNotAvailable() async {
        world.auth.lockState = .unlocked
        let model = MasternodeKeychainViewModel(
            env: world.environment(), m2: m2.services, m3: M3Services.unavailable())
        await model.select(.owner)
        #expect(!model.available)
        #expect(model.keys.isEmpty && model.errorMessage == nil)
    }
}
