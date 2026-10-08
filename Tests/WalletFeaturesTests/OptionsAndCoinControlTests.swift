// Options dialog (QT-135…141, QT-033, QT-075, QT-116, IOS-104/105) and coin
// control (QT-068…074).
import Foundation
import PlatformServices
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Options view model")
struct OptionsViewModelTests {
    let world = FakeWorld()

    func make(_ m2: FakeM2World) -> OptionsViewModel {
        OptionsViewModel(env: world.environment(), m2: m2.services)
    }

    @Test func QT135_okAppliesChangedValuesAndCancelReverts() async throws {
        let m2 = FakeM2World()
        let model = make(m2)
        await model.load()
        #expect(!model.hasChanges)
        model.wallet.coinControl = true
        model.display.unit = .milliDash
        model.appearance = .dark
        #expect(model.hasChanges)
        model.discard()
        #expect(!model.wallet.coinControl && model.display.unit == .dash && !model.hasChanges)
        model.wallet.coinControl = true
        model.display.unit = .milliDash
        model.appearance = .dark
        try await model.apply()
        #expect(m2.desktopPreferences.desktop.options.coinControl)
        #expect(world.settings.display.unit == .milliDash)
        #expect(world.preferences.preferences.theme == .dark)
        #expect(!model.hasChanges)
    }

    @Test func QT136_mainTabIsHiddenOnMacOSAndAutostartUsesMinAndNetwork() async throws {
        #expect(!make(FakeM2World(platform: .macOS)).tabs.contains(.main))
        let m2 = FakeM2World(platform: .linux)
        let model = make(m2)
        await model.load()
        #expect(model.tabs.first == .main)
        model.main.startOnLogin = true
        model.main.minimizeToTray = true
        try await model.apply()
        #expect(m2.launchAtLogin.calls.current.first?.1 == ["--min", "--testnet"])
        #expect(m2.shellSettings.shell.minimizeToTray)
    }

    @Test func QT137_walletDefaultsAreDashQts() async {
        let model = make(FakeM2World())
        await model.load()
        #expect(!model.wallet.subtractFeeByDefault && !model.wallet.coinControl && !model.wallet.psbtControls)
        #expect(!model.wallet.keepCustomChangeAddress)
        #expect(model.wallet.dustThreshold == 10_000)
    }

    @Test func QT075_dustProtectionRangeAndEngineCall() async throws {
        let m2 = FakeM2World()
        m2.dustProtection.configured.withLock { $0 = true }
        let model = make(m2)
        await model.load()
        #expect(model.dustProtectionAvailable && !model.wallet.dustProtectionEnabled)
        model.wallet.dustProtectionEnabled = true
        model.wallet.dustThreshold = 1_000_001
        await #expect(throws: ServiceError.self) { try await model.apply() }
        #expect(model.errorMessage == L10n.Options.dustThresholdInvalid)
        model.wallet.dustThreshold = 5000
        try await model.apply()
        #expect(m2.dustProtection.sets.current == [Amount(duffs: 5000)])
    }

    @Test func QT075_dustProtectionNotImplementedIsShownUnavailable() async throws {
        let m2 = FakeM2World()
        let model = make(m2)
        await model.load()
        #expect(!model.dustProtectionAvailable)
        #expect(model.errorMessage == nil)
        model.wallet.subtractFeeByDefault = true
        try await model.apply()
        #expect(m2.dustProtection.sets.current.isEmpty)
    }

    @Test func QT116_automaticBackupsKeepIsSentToTheEngine() async throws {
        let m2 = FakeM2World()
        m2.backups.policyValue.withLock { $0 = BackupPolicy(keep: 10, directory: URL(fileURLWithPath: "/b")) }
        let model = make(m2)
        await model.load()
        #expect(model.wallet.automaticBackups == 10)
        model.wallet.automaticBackups = 11
        await #expect(throws: ServiceError.self) { try await model.apply() }
        model.wallet.automaticBackups = 3
        try await model.apply()
        #expect(m2.backups.keeps.current == [3])
    }

    @Test func QT138_proxyAcceptsNumericAddressesOnly() {
        #expect(OptionsViewModel.validateProxy(ip: "127.0.0.1", port: "9050"))
        #expect(OptionsViewModel.validateProxy(ip: "::1", port: "9050"))
        #expect(OptionsViewModel.validateProxy(ip: "[2001:db8::1]", port: "1"))
        #expect(!OptionsViewModel.validateProxy(ip: "localhost", port: "9050"))
        #expect(!OptionsViewModel.validateProxy(ip: "256.0.0.1", port: "9050"))
        #expect(!OptionsViewModel.validateProxy(ip: "127.0.0.1", port: "65536"))
        let model = make(FakeM2World())
        #expect(!model.network.isEditable)
        model.network.proxyEnabled = true
        model.network.proxyIP = "proxy.example"
        #expect(model.proxyError == "The supplied proxy address is invalid.")
    }

    @Test func QT139_displayTabOptionsAndLanguageRestart() async throws {
        let m2 = FakeM2World()
        let model = make(m2)
        await model.load()
        model.display.thirdPartyTxURLs = "https://example.com/tx/%s"
        model.display.languageCode = "en"
        try await model.apply()
        #expect(m2.desktopPreferences.desktop.options.thirdPartyTxURLs == "https://example.com/tx/%s")
        #expect(model.restartRequired)
        #expect(L10n.Options.languageName(nil) == "(Default)")
        #expect(L10n.Options.spvOnlyOptions.contains("Prune block storage"))
    }

    @Test func IOS104_localCurrencyListIsSearchable() {
        let model = make(FakeM2World())
        #expect(model.currencies().contains("USD"))
        #expect(model.currencies(matching: "euro") == ["EUR"])
    }

    @Test func IOS105_enablingNotificationsAsksTheOSOnce() async throws {
        let m2 = FakeM2World()
        let model = make(m2)
        await model.load()
        model.notifications.enabled = false
        try await model.apply()
        model.notifications.enabled = true
        try await model.apply()
        #expect(m2.notifier.requests.current == 1)
        #expect(model.notificationStatusText == nil)
        m2.notifier.state.withLock { $0 = .denied }
        await model.load()
        #expect(model.notificationStatusText == "Turned off in system settings")
    }

    @Test func QT033_coinJoinPopupsOptionIsKeptInShellSettings() async throws {
        let m2 = FakeM2World()
        let model = make(m2)
        await model.load()
        #expect(model.notifications.showCoinJoinNotifications)
        model.notifications.showCoinJoinNotifications = false
        try await model.apply()
        #expect(!m2.shellSettings.shell.showCoinJoinNotifications)
    }

    @Test func QT141_resetOptionsConfirmsBacksUpAndQuits() {
        let m2 = FakeM2World(platform: .linux)
        m2.launchAtLogin.enabled.withLock { $0 = true }
        let model = make(m2)
        model.confirmReset()
        #expect(model.resetFlow == .idle)
        model.requestReset()
        #expect(model.resetFlow == .confirming)
        model.confirmReset()
        guard case .done(let backups) = model.resetFlow else {
            Issue.record("reset did not finish")
            return
        }
        #expect(backups.count == 2)
        #expect(!m2.launchAtLogin.enabled.current)
    }
}

@MainActor
@Suite("Coin control view model")
struct CoinControlViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    func make(coins: [Utxo]) -> CoinControlViewModel {
        world.coinControl.coins.withLock { $0 = coins }
        m2.fees.coins.withLock { $0 = Dictionary(uniqueKeysWithValues: coins.map { ($0.outpoint, $0.amount) }) }
        return CoinControlViewModel(env: world.environment(), m2: m2.services)
    }

    @Test func QT069_listModeDefaultSortedByAmountDescendingAndPersisted() async {
        let model = make(coins: [utxo(1, amount: 100_000), utxo(2, amount: 900_000), utxo(3, amount: 500_000)])
        await model.load()
        #expect(model.mode == .list)
        #expect(model.coins.map(\.amount.duffs) == [900_000, 500_000, 100_000])
        model.sort(by: .amount)
        #expect(model.coins.map(\.amount.duffs) == [100_000, 500_000, 900_000])
        model.setMode(.tree)
        #expect(m2.desktopPreferences.desktop.coinControlMode == .tree)
        #expect(m2.desktopPreferences.desktop.coinSort == CoinSort(column: .amount, ascending: true))
        #expect(L10n.CoinControl.columnTitle(.label) == "Received with label")
    }

    @Test func QT069_treeModeGroupsByAddressAndLabelsChange() async {
        let model = make(coins: [
            utxo(1, amount: 100, address: testnetAddress1, label: "Salary"),
            utxo(2, amount: 200, address: testnetAddress1, label: "Salary"),
            utxo(3, amount: 300, address: testnetAddress2, change: true),
        ])
        await model.load()
        let groups = model.groups
        #expect(groups.count == 2)
        #expect(groups.first { $0.address == testnetAddress1 }?.total == Amount(duffs: 300))
        #expect(model.label(of: model.coins.first { $0.isChange }!) == "(change)")
        #expect(model.label(of: utxo(9, amount: 1)) == "(no label)")
    }

    @Test func QT070_copyMenuAndLocks() async {
        let model = make(coins: [utxo(1, amount: 123_456_789, label: "Rent")])
        await model.load()
        let outpoint = OutPoint(txid: txid(1), vout: 0)
        #expect(model.copy(.outpoint, of: outpoint) == "\(txid(1)):0")
        #expect(model.copy(.amount, of: outpoint) == "1.23456789")
        #expect(model.copy(.label, of: outpoint) == "Rent")
        await model.toggle(outpoint)
        await model.lock(outpoint)
        #expect(model.lockedCount == 1 && model.lockedText == "(1 locked)")
        #expect(model.selected.isEmpty)
        await model.toggle(outpoint)
        #expect(model.selected.isEmpty)  // locked coins cannot be picked
        await model.unlock(outpoint)
        #expect(model.lockedCount == 0)
    }

    @Test func QT070_unlockAllFlipsEveryLock() async {
        let model = make(coins: [utxo(1, amount: 1000), utxo(2, amount: 2000)])
        world.coinControl.locked.withLock { $0 = [OutPoint(txid: txid(1), vout: 0)] }
        await model.load()
        await model.lockAll()
        #expect(world.coinControl.locked.current == [OutPoint(txid: txid(2), vout: 0)])
    }

    @Test func QT071_coinJoinCoinsAreHiddenByDefault() async {
        let model = make(coins: [utxo(1, amount: 1000), utxo(2, amount: 100_001, denominated: true, rounds: 4)])
        await model.load()
        #expect(model.coins.count == 1)
        #expect(model.coinJoinToggleTitle == "Show all coins")
        await model.setShowCoinJoinCoins(true)
        #expect(model.coins.count == 2)
        #expect(model.coinJoinToggleTitle == "Hide CoinJoin coins")
    }

    @Test func QT072_summaryUsesTheEngineFormulaWithApproximatelyPrefix() async {
        let model = make(coins: [utxo(1, amount: 10_000_000), utxo(2, amount: 5_000_000)])
        await model.load()
        #expect(model.isAutomatic)
        await model.updatePayment(amounts: [Amount(duffs: 1_000_000)], fee: .recommended(targetBlocks: 6))
        await model.selectAll()
        // 148·2 + 34·(1+1) + 10 = 374 bytes at 1000 duff/kB.
        #expect(model.summary?.bytes == 374)
        #expect(model.summaryText?.bytes == "≈374")
        #expect(model.summaryText?.fee.hasPrefix("≈") == true)
        #expect(model.copy(.bytes) == "374")
        #expect(model.copy(.fee) == "0.00000374")
        #expect(model.summaryText?.tolerance == "Can vary +/- 2 duffs per input.")
        #expect(m2.fees.summaries.current.last?.count == 2)
    }

    @Test func QT072_summaryNotImplementedIsUnavailableNotZero() async {
        let model = make(coins: [utxo(1, amount: 10_000_000)])
        m2.fees.coins.withLock { $0 = nil }
        await model.load()
        await model.toggle(OutPoint(txid: txid(1), vout: 0))
        #expect(model.summary == nil)
        #expect(model.summaryUnavailable)
        #expect(model.errorMessage == nil)
    }

    @Test func QT074_spentSelectionsAreUnselectedWithTheNotice() async {
        m2.desktopPreferences.desktop.options.coinControl = true
        let model = make(coins: [utxo(1, amount: 10_000_000), utxo(2, amount: 5_000_000)])
        await model.load()
        await model.selectAll()
        // Coin 2 is spent elsewhere: the engine reports it unavailable.
        m2.fees.coins.withLock { $0?[OutPoint(txid: txid(2), vout: 0)] = nil }
        await model.updatePayment(amounts: [Amount(duffs: 1_000_000)], fee: .recommended(targetBlocks: 6))
        #expect(model.selected == [OutPoint(txid: txid(1), vout: 0)])
        #expect(model.unselectedNotice)
        #expect(L10n.CoinControl.coinsUnselected == "Some coins were unselected because they were spent.")
        #expect(model.source() == .outpoints([OutPoint(txid: txid(1), vout: 0)]))
    }

    @Test func QT073_customChangeAddressChecks() async {
        let model = make(coins: [])
        await model.load()
        await model.setCustomChange("not an address")
        #expect(model.customChangeWarning == .invalidAddress)
        #expect(model.customChangeWarning?.text == "Warning: Invalid Dash address")
        #expect(model.change() == .automatic)
        await model.setCustomChange(testnetAddress2)
        #expect(model.customChangeWarning == .unknownAddress(confirmed: false))
        #expect(model.change() == .automatic)
        model.confirmCustomChange()
        #expect(model.change() == .address(testnetAddress2))
        world.receive.state.withLock { $0.others = [FakeReceive.info(testnetAddress1, index: 3)] }
        await model.setCustomChange(testnetAddress1)
        #expect(model.customChangeWarning == nil)
        #expect(model.change() == .address(testnetAddress1))
    }

    @Test func QT073_keepCustomChangeAddressRemembersIt() async {
        m2.desktopPreferences.desktop.options.keepCustomChangeAddress = true
        world.receive.state.withLock { $0.others = [FakeReceive.info(testnetAddress1, index: 3)] }
        let model = make(coins: [])
        await model.setCustomChange(testnetAddress1)
        #expect(m2.desktopPreferences.desktop.options.customChangeAddress == testnetAddress1)
        let next = CoinControlViewModel(env: world.environment(), m2: m2.services)
        #expect(next.customChange == testnetAddress1)
    }
}
