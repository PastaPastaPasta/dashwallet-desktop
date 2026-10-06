// S1 security and notification services: quick unlock (IOS-011/016),
// recovery and wipe (IOS-014/109), auto-lock (IOS-015), transaction
// notifications (QT-031…033), log export (IOS-112), QR images (IOS-043).
import DashKit
import Foundation
import PlatformServices
import Testing
@testable import WalletRuntime

private typealias RGrant = WalletRuntime.AuthGrant

private func grant(_ purpose: WalletRuntime.GrantPurpose) -> RGrant {
    RGrant(id: "g1", purpose: purpose, expiresAt: Date().addingTimeInterval(60), singleUse: true)
}

private func policy(enrolled: Bool, limit: Int64 = 50_000_000) -> DashKit.QuickUnlockPolicy {
    DashKit.QuickUnlockPolicy(
        enrolled: enrolled, spendLimit: DashKit.Amount(duffs: limit), passphraseMaxAgeSeconds: 604_800,
        lastPassphraseAt: nil)
}

@MainActor
private func desktop(_ h: Harness, platform: DesktopOSServices = .fake()) -> DesktopRuntimeServices {
    DesktopRuntimeServices(runtime: h.services, platform: platform, clock: h.clock, onQuit: {})
}

@MainActor
@Suite struct QuickUnlockServiceTests {
    @Test func IOS011_enrolStoresTheVaultKeyAndCredentialReturnsIt() async throws {
        let keys = FakeKeyStore()
        let h = Harness {
            $0.with {
                $0.vault = Fixtures.vault(.unlocked)
                $0.enrollKey = .success(Array(repeating: 7, count: 32))
                $0.quickUnlockPolicy = .success(policy(enrolled: true))
            }
        }
        try await h.start()
        let services = desktop(h, platform: .fake(keys: keys))
        let quickUnlock = services.quickUnlock
        #expect(quickUnlock.provider == .touchID)

        await #expect(throws: ServiceError(code: .vaultGrantPurposeMismatch, detail: "enrolling needs a changeCredential grant")) {
            _ = try await quickUnlock.enroll(grant: grant(.revealSecret))
        }
        let result = try await quickUnlock.enroll(grant: grant(.changeCredential))
        #expect(result.enrolled)
        #expect(result.spendLimit == QuickUnlockPolicy.defaultSpendLimit)
        #expect(keys.item("regtest") == Array(repeating: 7, count: 32))
        #expect(h.engine.calls.contains("enrollQuickUnlock g1"))

        let credential = try await quickUnlock.credential(reason: "Send 0.1 DASH")
        guard case .quickUnlock(let key) = credential else {
            Issue.record("expected a quick-unlock credential")
            return
        }
        #expect(key.withUnsafeBytes { Array($0) } == Array(repeating: 7, count: 32))
        #expect(keys.prompts == ["Send 0.1 DASH"])
    }

    @Test func IOS011_aRefusedKeychainRemovesTheNewSlot() async throws {
        let keys = FakeKeyStore()
        keys.storeError = PlatformServiceError(code: "desktop.os_error", detail: "missing entitlement")
        let h = Harness {
            $0.with {
                $0.vault = Fixtures.vault(.unlocked)
                $0.enrollKey = .success(Array(repeating: 1, count: 32))
                $0.removeQuickUnlock = .success(Fixtures.vault(.unlocked))
            }
        }
        try await h.start()
        let quickUnlock = desktop(h, platform: .fake(keys: keys)).quickUnlock
        await #expect(throws: ServiceError(code: ServiceErrorCode(rawValue: "desktop.os_error"), detail: "missing entitlement")) {
            _ = try await quickUnlock.enroll(grant: grant(.changeCredential))
        }
        #expect(h.engine.calls.contains("removeQuickUnlock"))
    }

    @Test func IOS011_noBiometricsMeansUnavailable() async throws {
        let h = Harness { $0.with { $0.vault = Fixtures.vault(.unlocked) } }
        try await h.start()
        let quickUnlock = desktop(h, platform: .fake(keys: FakeKeyStore(kind: .none))).quickUnlock
        #expect(quickUnlock.provider == .unavailable)
        await #expect(throws: ServiceError.self) {
            _ = try await quickUnlock.enroll(grant: grant(.changeCredential))
        }
        #expect(!h.engine.calls.contains { $0.hasPrefix("enrollQuickUnlock") })
    }

    @Test func IOS016_spendLimitOptionsOnly() async throws {
        let h = Harness {
            $0.with {
                $0.vault = Fixtures.vault(.unlocked)
                $0.quickUnlockPolicy = .success(policy(enrolled: true))
            }
        }
        try await h.start()
        let quickUnlock = desktop(h).quickUnlock
        await #expect(throws: ServiceError.self) {
            _ = try await quickUnlock.setSpendLimit(Amount(duffs: 12_345), grant: grant(.changeCredential))
        }
        let updated = try await quickUnlock.setSpendLimit(Amount(duffs: 100_000_000), grant: grant(.changeCredential))
        #expect(updated.spendLimit == Amount(duffs: 100_000_000))
        #expect(h.engine.with { $0.spendLimits } == [DashKit.Amount(duffs: 100_000_000)])
    }

    @Test func IOS011_cancelledPromptIsTyped() async throws {
        let keys = FakeKeyStore()
        keys.retrieveError = PlatformServiceError(code: "platform.cancelled")
        let h = Harness { $0.with { $0.vault = Fixtures.vault(.locked) } }
        try await h.start()
        let quickUnlock = desktop(h, platform: .fake(keys: keys)).quickUnlock
        do {
            _ = try await quickUnlock.credential(reason: "x")
            Issue.record("no error")
        } catch {
            #expect(error.code == .platformCancelled)
        }
    }

    @Test func IOS014_IOS109_recoveryAndDestroyDropTheStoredKey() async throws {
        let keys = FakeKeyStore()
        try keys.store(BiometricKey(copying: UnsafeRawBufferPointer(start: nil, count: 0)), network: "regtest")
        let h = Harness {
            $0.with {
                $0.vault = Fixtures.vault(.unlocked)
                $0.recovery = .success(
                    DashKit.VaultRecovery(status: Fixtures.vault(.unlocked), walletsWithoutSecrets: [Fixtures.walletB]))
                $0.destroy = .success(Fixtures.vault(.noVault, encrypted: false))
            }
        }
        try await h.start()
        let services = desktop(h, platform: .fake(keys: keys))
        let secret = h.services.vault.makeSecret(utf8: "abandon")
        let result = try await services.vaultRecovery.recover(
            wallet: WalletID(Fixtures.walletA), mnemonic: secret, bip39Passphrase: h.services.vault.makeSecret(utf8: ""),
            newPassphrase: h.services.vault.makeSecret(utf8: "new"))
        #expect(result.walletsWithoutSecrets == [WalletID(Fixtures.walletB)])
        #expect(keys.item("regtest") == nil)

        let status = try await services.vaultRecovery.destroy(credential: .passphrase(h.services.vault.makeSecret(utf8: "new")))
        #expect(status.state == .noVault)
        #expect(h.services.auth.lockState == .noVault)
        #expect(h.engine.with { $0.destroyCredentials } == ["passphrase"])
    }
}

@MainActor
@Suite struct AutoLockTests {
    @Test func IOS015_defaultIsNeverAndTheIntervalPersists() throws {
        let h = Harness()
        let services = desktop(h)
        #expect(services.autoLock.interval == .never)
        try services.autoLock.setInterval(.fiveMinutes)
        let again = AutoLockController(settings: h.services.settings, idle: nil, clock: h.clock, lock: {})
        #expect(again.interval == .fiveMinutes)
    }

    @Test func IOS015_inactivityLocksOnceUntilActivity() async throws {
        let clock = ManualClock()
        let dir = TempDir()
        var locks = 0
        let controller = AutoLockController(
            settings: SettingsStore(directory: dir.url), idle: nil, clock: clock, lock: { locks += 1 })
        try controller.setInterval(.oneMinute)
        #expect(await eventually { clock.sleeperCount == 1 })
        clock.advance(by: .seconds(30))
        controller.noteActivity()
        clock.advance(by: .seconds(31))
        // Activity moved the deadline: no lock yet.
        #expect(await eventually { clock.sleeperCount == 1 })
        #expect(locks == 0)
        clock.advance(by: .seconds(30))
        #expect(await eventually { locks == 1 })
        // No further lock without activity.
        #expect(await eventually { clock.sleeperCount == 1 })
        clock.advance(by: .seconds(120))
        #expect(await eventually { clock.sleeperCount == 1 })
        #expect(locks == 1)
        controller.noteActivity()
        clock.advance(by: .seconds(60))
        #expect(await eventually { locks == 2 })
        controller.stop()
    }

    @Test func IOS015_sleepScreenLockAndIdleEvents() async throws {
        let idle = FakeIdle()
        let dir = TempDir()
        var locks = 0
        let controller = AutoLockController(
            settings: SettingsStore(directory: dir.url), idle: idle, clock: ManualClock(), lock: { locks += 1 })
        // Each step waits until the controller handled the events it sent,
        // so no event is judged under the next step's interval.
        func send(_ events: IdleEvent...) async {
            let target = controller.handledIdleEvents + events.count
            for event in events { idle.send(event) }
            for _ in 0..<2000 where controller.handledIdleEvents < target {
                try? await Task.sleep(for: .milliseconds(5))
            }
            #expect(controller.handledIdleEvents == target)
        }
        await send(.systemWillSleep)
        #expect(locks == 0, ".never ignores events")

        try controller.setInterval(.oneHour)
        await send(.userIdle(seconds: 120))
        #expect(locks == 0, "idle shorter than the interval")
        await send(.screenLocked)
        #expect(locks == 1)
        await send(.userIdle(seconds: 3600))
        #expect(locks == 2)

        try controller.setInterval(.immediately)
        await controller.noteBackgrounded()
        #expect(locks == 3)
        controller.stop()
    }
}

private func notice(_ txid: String, amount: Int64, coinJoin: Bool = false, label: String? = nil) -> DashKit.TxNotice {
    DashKit.TxNotice(
        txid: txid, recordIndex: 0, amount: DashKit.Amount(duffs: amount), timestamp: Date(timeIntervalSince1970: 0),
        type: amount < 0 ? .sendToAddress : .recvWithAddress, address: "yAddress", label: label,
        coinJoinInternal: coinJoin)
}

@MainActor
@Suite struct TransactionNotificationTests {
    private func batch(_ rows: [DashKit.TxNotice], catchUp: Bool = false) -> TransactionNoticeBatch {
        TransactionNoticeBatch(
            network: .regtest, wallet: WalletID(Fixtures.walletA), notices: rows.map(TransactionNotice.init),
            catchUp: catchUp)
    }

    @Test func QT031_oneNotificationPerRowWithDashQtLines() async throws {
        let h = Harness()
        let notifier = FakeNotifier()
        let services = desktop(h, platform: .fake(notifier: notifier))
        let presenter = services.notifications
        let rows = presenter.notifications(for: batch([notice("aa", amount: 150_000_000, label: "Rent"), notice("bb", amount: -1)]))
        #expect(rows.count == 2)
        #expect(rows[0].title == "Incoming transaction")
        #expect(rows[1].title == "Sent transaction")
        #expect(rows[0].body.contains("Label: Rent"))
        #expect(!rows[0].body.contains("Address:"))
        #expect(rows[1].body.contains("Address: yAddress"))
        #expect(!rows[0].body.contains("Wallet:"), "one wallet: no Wallet line")
        #expect(rows[0].deepLink == "dashwallet://tx/\(Fixtures.walletA.hex)/aa")
    }

    @Test func QT032_catchUpIsSilentAndHundredRowsSummarize() {
        let h = Harness()
        let presenter = desktop(h).notifications
        #expect(presenter.notifications(for: batch([notice("aa", amount: 1)], catchUp: true)).isEmpty)
        let many = (0..<100).map { notice(String(format: "%02x", $0), amount: $0 % 2 == 0 ? 10 : -5) }
        let summary = presenter.notifications(for: batch(many))
        #expect(summary.count == 1)
        #expect(summary[0].title == "Received and sent multiple transactions")
        #expect(summary[0].body.contains("Sent Amount:"))
        #expect(summary[0].body.contains("Received Amount:"))
        let received = presenter.notifications(for: batch((0..<100).map { notice("r\($0)", amount: 1) }))
        #expect(received[0].title == "Received multiple transactions")
    }

    @Test func QT033_coinJoinRowsFollowTheOptionAndNotificationsCanBeOff() throws {
        let h = Harness()
        let services = desktop(h)
        let presenter = services.notifications
        let rows = [notice("aa", amount: 1, coinJoin: true), notice("bb", amount: 2)]
        #expect(presenter.notifications(for: batch(rows)).count == 2)
        try services.shellSettings.update(ShellSettings(showCoinJoinNotifications: false))
        #expect(presenter.notifications(for: batch(rows)).count == 1)
        try services.shellSettings.update(ShellSettings(notificationsEnabled: false))
        #expect(presenter.notifications(for: batch(rows)).isEmpty)
    }

    @Test func QT031_engineBatchesReachTheNotifier() async throws {
        let notifier = FakeNotifier()
        let h = Harness { $0.with { $0.txNotices = .success([notice("aa", amount: 5)]) } }
        try await h.start()
        let services = desktop(h, platform: .fake(notifier: notifier))
        services.start()
        // Let the feed subscribe before the event.
        try await Task.sleep(for: .milliseconds(50))
        h.engine.events.publish(.newTransactions(.regtest, Fixtures.walletA, txids: ["aa"], catchUp: false))
        #expect(await eventually { notifier.posted.count == 1 })
        #expect(notifier.posted.first?.title == "Incoming transaction")
        // Another network's event is ignored.
        h.engine.events.publish(.newTransactions(.testnet, Fixtures.walletA, txids: ["aa"], catchUp: false))
        try await Task.sleep(for: .milliseconds(50))
        #expect(notifier.posted.count == 1)
        services.notifications.stop()
    }

    @Test func QT031_unreadableRowsAreNotInvented() async throws {
        let notifier = FakeNotifier()
        let h = Harness()
        try await h.start()
        let services = desktop(h, platform: .fake(notifier: notifier))
        services.start()
        try await Task.sleep(for: .milliseconds(50))
        h.engine.events.publish(.newTransactions(.regtest, Fixtures.walletA, txids: ["aa"], catchUp: false))
        #expect(await eventually { services.transactionFeed.lastError?.code == .notImplemented })
        #expect(notifier.posted.isEmpty)
        services.notifications.stop()
    }
}

@MainActor
@Suite struct LogsAndQRTests {
    @Test func IOS112_logExportAddsTheAppLogs() async throws {
        let h = Harness {
            $0.with {
                $0.logExport = .success(DashKit.LogExport(file: URL(fileURLWithPath: "/tmp/x.zip"), fileCount: 1, sizeBytes: 9))
            }
        }
        let appLog = URL(fileURLWithPath: "/tmp/app.log")
        let service = LogExportService(engine: h.engine, appLogFiles: { [appLog] })
        let url = try await service.exportLogs(to: URL(fileURLWithPath: "/tmp/x.zip"))
        #expect(url.path == "/tmp/x.zip")
        #expect(h.engine.with { $0.exportedExtraFiles } == [appLog])

        let failing = LogExportService(engine: Harness().engine)
        do {
            _ = try await failing.exportLogs(to: URL(fileURLWithPath: "/tmp/y.zip"))
            Issue.record("no error")
        } catch {
            #expect(error.code == .notImplemented)
        }
    }

    @Test func IOS043_qrFromFileAndClipboard() throws {
        let dir = TempDir()
        let file = dir.url.appendingPathComponent("qr.png")
        try Data([1, 2, 3]).write(to: file)
        let reader = QRImageImport(decoder: FakeQRDecoder(codes: ["dash:x"]), clipboard: FakeClipboard(image: Data([1])))
        #expect(try reader.decode(file: file) == ["dash:x"])
        #expect(try reader.decodeClipboard() == ["dash:x"])

        let empty = QRImageImport(decoder: FakeQRDecoder(), clipboard: FakeClipboard(image: nil))
        #expect(throws: ServiceError(code: .desktopImageUnreadable, detail: "the clipboard holds no image")) {
            try empty.decodeClipboard()
        }
        do {
            _ = try empty.decode(file: file)
            Issue.record("no error")
        } catch {
            #expect(error.code == .desktopNoQRCode)
        }
        let none = QRImageImport(decoder: FakeQRDecoder(), clipboard: nil)
        #expect(throws: ServiceError(code: .desktopUnsupported, detail: "clipboard images")) {
            try none.decodeClipboard()
        }
        do {
            _ = try empty.decode(file: dir.url.appendingPathComponent("missing.png"))
            Issue.record("no error")
        } catch {
            #expect(error.code == .desktopImageUnreadable)
        }
    }
}
