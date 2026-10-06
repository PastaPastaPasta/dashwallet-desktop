import DashKit
import Foundation
import Testing
@testable import WalletRuntime

@MainActor
@Suite struct LifecycleTests {
    @Test func startOpensSessionThenObserversThenSPV() async throws {
        let h = Harness()
        try await h.start()

        let calls = h.engine.calls
        let order = ["open regtest", "vaultStatus", "walletInfos", "syncSnapshot", "startSPV regtest"].map {
            calls.firstIndex(of: $0)
        }
        #expect(order.allSatisfy { $0 != nil }, "calls: \(calls)")
        #expect(order.compactMap { $0 } == order.compactMap { $0 }.sorted(), "calls: \(calls)")
        #expect(await h.services.host.activeNetwork == .regtest)
        #expect(h.services.settings.lastNetwork == .regtest)
        #expect(await h.services.lifecycle.transition == .idle)
    }

    @Test func stopRunsInReverseAndClearsState() async throws {
        let h = Harness {
            $0.with { s in
                s.vault = Fixtures.vault(.locked)
                s.walletInfos = .success([Fixtures.info(Fixtures.walletA, name: "A", confirmed: 5)])
            }
        }
        try await h.start()
        #expect(h.services.auth.lockState == .locked)
        #expect(h.services.walletState.wallets?.count == 1)

        try await h.services.lifecycle.stop()
        let calls = h.engine.calls
        let stopSPV = try #require(calls.firstIndex(of: "stopSPV regtest"))
        let close = try #require(calls.firstIndex(of: "close regtest"))
        #expect(stopSPV < close)
        #expect(await h.services.host.activeNetwork == nil)
        #expect(h.services.auth.lockState == nil)
        #expect(h.services.walletState.wallets == nil)
        #expect(h.services.walletState.balances == nil)
        #expect(h.services.sync.status == nil)
    }

    @Test func switchNetworkClosesTheOldSessionFirst() async throws {
        let h = Harness()
        try await h.start()
        try await h.services.lifecycle.switchNetwork(to: .testnet)
        let calls = h.engine.calls
        let close = try #require(calls.firstIndex(of: "close regtest"))
        let open = try #require(calls.firstIndex(of: "open testnet"))
        #expect(close < open)
        #expect(await h.services.host.activeNetwork == .testnet)
        #expect(h.services.settings.lastNetwork == .testnet)
    }

    @Test func queuedOperationsDoNotInterleave() async throws {
        let h = Harness()
        let lifecycle = h.services.lifecycle
        async let first: Void = lifecycle.start(network: .regtest)
        async let second: Void = lifecycle.switchNetwork(to: .testnet)
        async let third: Void = lifecycle.stop()
        _ = try await (first, second, third)
        // Every open is followed by its SPV start before any close begins.
        var openNetworks = 0
        for call in h.engine.calls {
            if call.hasPrefix("open ") { openNetworks += 1 }
            if call.hasPrefix("close ") { openNetworks -= 1 }
            #expect((0...1).contains(openNetworks), "two sessions open at once: \(h.engine.calls)")
        }
    }

    @Test func walletOperationsNeedAnOpenNetwork() async throws {
        let h = Harness()
        let vault = h.services.vault
        do {
            _ = try await h.services.lifecycle.importWallet(
                mnemonic: vault.makeSecret(utf8: "x"), bip39Passphrase: vault.makeSecret(utf8: ""),
                options: WalletImportOptions())
            Issue.record("import without a session must fail")
        } catch {
            #expect(error.code == .networkNotOpen)
        }
    }

    @Test func importReloadsTheWalletList() async throws {
        let h = Harness {
            $0.with { $0.walletInfos = .success([]) }
        }
        try await h.start()
        #expect(h.services.walletState.wallets == [])
        h.engine.with { $0.walletInfos = .success([Fixtures.info(Fixtures.walletA, name: "A", confirmed: 0)]) }
        let vault = h.services.vault
        let id = try await h.services.lifecycle.importWallet(
            mnemonic: vault.makeSecret(utf8: "phrase"), bip39Passphrase: vault.makeSecret(utf8: ""),
            options: WalletImportOptions())
        #expect(id.hex.count == 64)
        #expect(h.services.walletState.wallets?.map(\.id.hex) == [Fixtures.walletA.hex])
        #expect(h.services.walletState.selectedWalletID?.hex == Fixtures.walletA.hex)
    }

    @Test func removeWalletNeedsAWipeGrant() async throws {
        let h = Harness()
        try await h.start()
        let grant = WalletRuntime.AuthGrant(id: "g", purpose: .signMessage, expiresAt: Date(), singleUse: true)
        let wallet = WalletRuntime.WalletID(Fixtures.walletA)
        await #expect(throws: ServiceError(code: .vaultGrantPurposeMismatch, detail: "removing a wallet needs a wipe grant")) {
            try await h.services.lifecycle.removeWallet(wallet, grant: grant)
        }
        #expect(!h.engine.calls.contains { $0.hasPrefix("removeWallet") })
    }

    @Test func shutdownStopsAndReleasesTheEngine() async throws {
        let h = Harness()
        try await h.start()
        try await h.services.shutdown()
        let calls = h.engine.calls
        let close = try #require(calls.firstIndex(of: "close regtest"))
        let shutdown = try #require(calls.firstIndex(of: "shutdown"))
        #expect(close < shutdown)
        #expect(h.engine.events.subscriberCount == 0)
    }

    @Test func launchOpensTheLastNetwork() async throws {
        let h = Harness()
        try await h.services.launch(defaultNetwork: .testnet)
        #expect(await h.services.host.activeNetwork == .testnet)
        try await h.services.lifecycle.stop()
        // A new store reading the same files remembers it.
        let reloaded = SettingsStore(directory: h.dir.url)
        #expect(reloaded.lastNetwork == .testnet)
    }
}

@MainActor
@Suite struct WalletStateTests {
    @Test func unknownUntilTheEngineListsWallets() async throws {
        let h = Harness()
        try await h.start()
        let state = h.services.walletState
        #expect(state.wallets == nil)
        #expect(state.balances == nil)
        #expect(state.lastError?.code == .notImplemented)
    }

    @Test func selectsTheFirstWalletAndFollowsBalanceEvents() async throws {
        let h = Harness {
            $0.with { s in
                s.walletInfos = .success([
                    Fixtures.info(Fixtures.walletA, name: "A", confirmed: 100),
                    Fixtures.info(Fixtures.walletB, name: "B", confirmed: 200),
                ])
                s.balances = [Fixtures.walletA: Fixtures.balances(150), Fixtures.walletB: Fixtures.balances(250)]
            }
        }
        try await h.start()
        let state = h.services.walletState
        #expect(state.selectedWalletID?.hex == Fixtures.walletA.hex)
        #expect(state.balances?.confirmed.duffs == 100)

        h.engine.events.publish(.balancesChanged(.regtest, Fixtures.walletA))
        #expect(await eventually { state.balances?.confirmed.duffs == 150 })
        #expect(state.wallets?.first?.balances?.confirmed.duffs == 150)

        state.select(WalletRuntime.WalletID(Fixtures.walletB))
        #expect(state.balances?.confirmed.duffs == 200)
        await state.settle()
        #expect(state.balances?.confirmed.duffs == 250)
    }

    @Test func reloadsOnLifecycleEventsAndKeepsTheSelection() async throws {
        let h = Harness {
            $0.with { $0.walletInfos = .success([Fixtures.info(Fixtures.walletA, name: "A", confirmed: 1)]) }
        }
        try await h.start()
        let state = h.services.walletState
        let changes = state.changes()
        h.engine.with {
            $0.walletInfos = .success([
                Fixtures.info(Fixtures.walletB, name: "B", confirmed: 2),
                Fixtures.info(Fixtures.walletA, name: "A", confirmed: 1),
            ])
        }
        h.engine.events.publish(.walletCreated(.regtest, Fixtures.walletB))
        var iterator = changes.makeAsyncIterator()
        _ = await iterator.next()
        #expect(await eventually { state.wallets?.count == 2 })
        #expect(state.selectedWalletID?.hex == Fixtures.walletA.hex)

        h.engine.with { $0.walletInfos = .success([Fixtures.info(Fixtures.walletB, name: "B", confirmed: 2)]) }
        h.engine.events.publish(.walletRemoved(.regtest, Fixtures.walletA))
        #expect(await eventually { state.wallets?.count == 1 })
        #expect(state.selectedWalletID?.hex == Fixtures.walletB.hex)
        #expect(state.balances?.confirmed.duffs == 2)
    }

    @Test func eventsOfAnotherNetworkAreIgnored() async throws {
        let h = Harness {
            $0.with { $0.walletInfos = .success([]) }
        }
        try await h.start()
        let before = h.engine.calls.filter { $0 == "walletInfos" }.count
        h.engine.events.publish(.walletCreated(.testnet, Fixtures.walletA))
        try await Task.sleep(for: .milliseconds(50))
        await h.services.walletState.settle()
        #expect(h.engine.calls.filter { $0 == "walletInfos" }.count == before)
    }
}

@MainActor
@Suite struct SPVCoordinatorTests {
    @Test func statusStaysUnknownWhileTheEngineHasNoSnapshot() async throws {
        let h = Harness()
        try await h.start()
        #expect(h.services.sync.status == nil)
        #expect(h.services.sync.lastError?.code == .notImplemented)
    }

    @Test func progressMovesAtMostTenPercentPerSnapshot() async throws {
        let h = Harness {
            $0.with { $0.snapshot = .success(Fixtures.snapshot(headers: (0, 100))) }
        }
        try await h.start()
        let sync = h.services.sync
        #expect(sync.status?.progress == 0)
        // Let the refresh the SPV-start event triggers finish first.
        let refreshes = { (h.engine.calls.filter { $0 == "syncSnapshot" }).count }
        try await Task.sleep(for: .milliseconds(50))
        await sync.settle()

        h.engine.with { $0.snapshot = .success(Fixtures.snapshot(headers: (80, 100))) }
        var before = refreshes()
        h.engine.events.publish(.syncChanged(.regtest))
        #expect(await eventually { refreshes() > before })
        await sync.settle()
        #expect(sync.status?.progress == 0.1)

        before = refreshes()
        h.engine.events.publish(.syncChanged(.regtest))
        #expect(await eventually { refreshes() > before })
        await sync.settle()
        #expect(abs((sync.status?.progress ?? 0) - 0.2) < 1e-9)
        #expect(sync.status?.isDone == false)
    }

    @Test func doneOnlyAfterThePeakDelay() async throws {
        let h = Harness {
            $0.with { $0.snapshot = .success(Fixtures.snapshot(headers: (100, 100), caughtUp: true)) }
        }
        try await h.start()
        let sync = h.services.sync
        #expect(sync.status?.isDone == false)
        #expect(await eventuallyAsync { h.clock.sleeperCount > 0 })
        h.clock.advance(by: .milliseconds(3000))
        try await Task.sleep(for: .milliseconds(20))
        #expect(sync.status?.isDone == false)
        h.clock.advance(by: .milliseconds(250))
        #expect(await eventually { sync.status?.isDone == true })
        #expect(sync.status?.progress == 1)
    }

    @Test func stalledAfterFortyFiveQuietSeconds() async throws {
        let h = Harness {
            $0.with { $0.snapshot = .success(Fixtures.snapshot(headers: (10, 100), secondsSinceProgress: 45)) }
        }
        try await h.start()
        #expect(h.services.sync.status?.isStalled == true)
    }

    @Test func peerCallsPassEngineErrorsThrough() async throws {
        let h = Harness()
        try await h.start()
        await #expect(throws: ServiceError.self) { try await h.services.sync.rotatePeers() }
        do {
            _ = try await h.services.sync.peers()
        } catch {
            #expect(error.code == .notImplemented)
        }
    }
}
