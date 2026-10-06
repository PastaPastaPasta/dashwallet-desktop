// S1 shell services: command line (QT-006), startup phases (QT-005),
// shutdown (QT-008), settings recovery (QT-007), shell settings and the
// single-instance URI hand-off (QT-001, QT-019).
import DashKit
import Foundation
import PlatformServices
import Testing
@testable import WalletRuntime

struct LaunchArgumentsParserTests {
    let parser = LaunchArgumentsParser(currentDirectory: URL(fileURLWithPath: "/work", isDirectory: true))

    @Test func QT006_dashQtFlagsAndValues() throws {
        let options = try parser.parse([
            "-min", "--splash=0", "-resetguisettings", "-lang=de_DE", "-windowtitle=Test Wallet",
            "-datadir=data", "-testnet",
        ])
        #expect(options.startMinimized)
        #expect(!options.showSplash)
        #expect(options.resetGUISettings)
        #expect(options.language == "de_DE")
        #expect(options.windowTitleSuffix == "Test Wallet")
        #expect(options.dataDirectory?.path == "/work/data")
        #expect(options.network == WalletRuntime.DashNetwork.testnet)
        #expect(options.uris.isEmpty)
    }

    @Test func QT006_defaultsNegationAndNetworks() throws {
        let empty = try parser.parse([])
        #expect(!empty.startMinimized && empty.showSplash && empty.network == nil)
        #expect(try !parser.parse(["-min", "-nomin"]).startMinimized)
        #expect(try parser.parse(["-min=0"]).startMinimized == false)
        #expect(try parser.parse(["-regtest"]).network == WalletRuntime.DashNetwork.regtest)
        #expect(try parser.parse(["-chain=main"]).network == WalletRuntime.DashNetwork.mainnet)
        #expect(try parser.parse(["-devnet=mobile"]).network == WalletRuntime.DashNetwork.devnet(name: "mobile"))
        #expect(
            try parser.parse(["-chain=devnet", "-devnet=mobile"]).network
                == WalletRuntime.DashNetwork.devnet(name: "mobile"))
        #expect(try parser.parse(["-datadir=/abs/dir"]).dataDirectory?.path == "/abs/dir")
        #expect(try parser.parse(["--help"]).showHelp)
        #expect(try parser.parse(["-choosedatadir"]).chooseDataDirectory)
    }

    @Test func QT006_urisComeLastAndMacLaunchNoiseIsSkipped() throws {
        let options = try parser.parse([
            "-psn_0_1234", "-NSDocumentRevisionsDebugMode", "YES", "-min", "dash:yX?amount=1", "pay:yY",
        ])
        #expect(options.uris == ["dash:yX?amount=1", "pay:yY"])
        #expect(options.startMinimized)
        #expect(throws: ServiceError(code: .launchOptionAfterURI, detail: "-min")) {
            try parser.parse(["dash:yX", "-min"])
        }
    }

    @Test func QT006_unknownAndInvalidOptionsAreTyped() {
        for (arguments, code) in [
            (["-server"], ServiceErrorCode.launchUnknownOption),
            (["-prune=550"], .launchUnknownOption),
            (["-nolang"], .launchUnknownOption),
            (["-lang"], .launchInvalidValue),
            (["-lang=de;rm"], .launchInvalidValue),
            (["-splash=maybe"], .launchInvalidValue),
            (["-chain=moon"], .launchInvalidValue),
            (["-chain=devnet"], .launchInvalidValue),
            (["-testnet", "-regtest"], .launchInvalidValue),
            (["-testnet", "-chain=main"], .launchInvalidValue),
            (["-devnet=a/b"], .launchInvalidValue),
            (["-datadir="], .launchInvalidValue),
        ] {
            do {
                _ = try parser.parse(arguments)
                Issue.record("\(arguments) parsed")
            } catch {
                #expect(error.code == code, "\(arguments)")
            }
        }
        #expect(parser.optionNames.contains("-min"))
        #expect(parser.optionNames.contains("-windowtitle=<name>"))
    }
}

@MainActor
@Suite struct StartupAndShutdownTests {
    @Test func QT005_phasesFollowTheEngineAndNeverGoBack() async throws {
        let h = Harness()
        let progress = StartupProgress(events: h.engine.events, onEmergencyQuit: {})
        let changes = progress.changes()
        try await progress.run(network: .regtest) { () async throws(ServiceError) in
            try await h.services.lifecycle.start(network: .regtest)
        }
        #expect(progress.phase == .ready)
        #expect(progress.progress == 1)
        var seen: [StartupPhase] = []
        for await phase in changes {
            seen.append(phase)
            if phase == .ready { break }
        }
        // The stream keeps the newest value for a slow reader, so phases may
        // be skipped but never repeated or reordered.
        let order: [StartupPhase] = [
            .loadingSettings, .openingNetwork(.regtest), .loadingWallets, .startingSync, .ready,
        ]
        let ranks = seen.compactMap { order.firstIndex(of: $0) }
        #expect(ranks.count == seen.count)
        #expect(ranks == ranks.sorted() && Set(ranks).count == ranks.count)
        #expect(seen.last == .ready)

        progress.advance(to: .loadingWallets)
        #expect(progress.phase == .ready)
    }

    @Test func QT005_failureAndEmergencyQuit() async {
        let h = Harness { $0.with { $0.openError = .storage(detail: "disk full") } }
        var quits = 0
        let progress = StartupProgress(events: h.engine.events, onEmergencyQuit: { quits += 1 })
        await #expect(throws: ServiceError.self) {
            try await progress.run(network: .regtest) { () async throws(ServiceError) in
                try await h.services.lifecycle.start(network: .regtest)
            }
        }
        #expect(progress.phase == .failed(.storage))
        progress.advance(to: .ready)
        #expect(progress.phase == .failed(.storage))
        progress.requestEmergencyQuit()
        progress.requestEmergencyQuit()
        #expect(quits == 1)
    }

    @Test func QT008_shutdownRunsOnceAndStopsTheEngine() async throws {
        let h = Harness()
        try await h.start()
        var flushes = 0
        let coordinator = ShutdownCoordinator(
            stop: { () async throws(ServiceError) in try await h.services.shutdown() }, flush: { flushes += 1 })
        async let first: Void = coordinator.shutdown()
        async let second: Void = coordinator.shutdown()
        _ = await (first, second)
        await coordinator.shutdown()
        #expect(flushes == 1)
        #expect(coordinator.finished)
        #expect(!coordinator.isShuttingDown)
        #expect(h.engine.calls.contains("stopSPV regtest"))
        #expect(h.engine.calls.filter { $0 == "shutdown" }.count == 1)
    }
}

@MainActor
@Suite struct ShellSettingsTests {
    @Test func shellSettingsLiveInGlobalJSON() throws {
        let dir = TempDir()
        let settings = SettingsStore(directory: dir.url)
        let store = ShellSettingsStore(settings: settings)
        #expect(store.shell == ShellSettings())
        var next = store.shell
        next.minimizeToTray = true
        next.showCoinJoinNotifications = false
        try store.update(next)

        let reloaded = ShellSettingsStore(settings: SettingsStore(directory: dir.url))
        #expect(reloaded.shell == next)
        let global = try String(contentsOf: dir.url.appendingPathComponent("global.json"), encoding: .utf8)
        #expect(global.contains("\"shell\""))
    }

    @Test func QT007_corruptFilesAreReportedAndResetKeepsBackups() throws {
        let dir = TempDir()
        let settingsURL = dir.url.appendingPathComponent("settings.json")
        try Data("{not json".utf8).write(to: settingsURL)
        let settings = SettingsStore(directory: dir.url)
        #expect(settings.recoveredFromCorruption == [settingsURL])
        // Abort = quit without writing: nothing was written yet.
        #expect(!FileManager.default.fileExists(atPath: settingsURL.path))

        try ShellSettingsStore(settings: settings).update(ShellSettings(minimizeOnClose: true))
        let backups = try settings.resetToDefaults()
        #expect(settings.recoveredFromCorruption.isEmpty)
        #expect(backups.map(\.lastPathComponent) == ["global.json.bak"])
        #expect(FileManager.default.fileExists(atPath: settingsURL.path))
        #expect(ShellSettingsStore(settings: settings).shell == ShellSettings())
        // A reset keeps the last network.
        #expect(SettingsStore(directory: dir.url).recoveredFromCorruption.isEmpty)
    }
}

@MainActor
@Suite struct IncomingURITests {
    @Test func QT001_forwardedLaunchesQueueTheirURIs() async throws {
        let h = Harness()
        let instance = FakeSingleInstance()
        let role = try IncomingURIRouter.claim(instance, network: .testnet, arguments: ["dash:x"])
        #expect(role == .primary)
        #expect(instance.claimedKeys == ["DashWallet-testnet"])

        let router = IncomingURIRouter(uriHandler: h.services.uri)
        router.follow(instance)
        let address = "yNsWkgPLN1u7p5dfWYnasMoS4hvrG2SNBY"
        instance.forward(["-min", "dash:\(address)?amount=1.5"])
        #expect(await eventually { router.activationRequests == 1 })
        // A launch without URIs only raises the window.
        instance.forward([])
        #expect(await eventually { router.activationRequests == 2 })
        #expect(router.pending.count + router.rejected.count == 1)
        router.stop()
    }

    @Test func QT019_droppedTextIsParsedOrRejected() {
        let h = Harness()
        let router = IncomingURIRouter(uriHandler: h.services.uri)
        router.receive("  not a uri  ")
        #expect(router.pending.isEmpty)
        #expect(router.rejected.first?.text == "not a uri")
        router.forwarded(["dash:x", "-min"])
        #expect(router.rejected.last?.error.code == .launchOptionAfterURI)
        router.clearRejected()
        #expect(router.rejected.isEmpty)
        #expect(router.takeNext() == nil)
    }
}
