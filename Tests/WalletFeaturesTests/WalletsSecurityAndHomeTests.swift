// Wallet management (QT-101, QT-106…110, QT-114, QT-116, IOS-009, IOS-110,
// IOS-111), security (IOS-011, IOS-014…016, IOS-108, IOS-109) and the home
// additions (IOS-005, IOS-025, IOS-117, IOS-121).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Wallet management view model")
struct WalletManagementViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    func make() -> WalletManagementViewModel { WalletManagementViewModel(env: world.environment(), m2: m2.services) }

    let report = WalletImportReport(
        walletID: walletB, labelsImported: 3, keysNotImported: 2, scriptsNotImported: 1, coreCompatibleSeed: true)

    @Test func QT101_closeKeepsDataAndLeavesLoadOnStartup() async {
        m2.walletLifecycle.states.withLock {
            $0 = [WalletLoadState(walletID: walletA, name: "Main", loaded: true, loadOnStartup: true, watchOnly: false)]
        }
        let model = make()
        await model.load()
        await model.close(walletA)
        #expect(model.wallets.first?.loaded == false && model.wallets.first?.loadOnStartup == false)
        await model.open(walletA)
        #expect(model.wallets.first?.loaded == true && model.wallets.first?.loadOnStartup == true)
        #expect(model.flow == .idle)
    }

    @Test func IOS110_listFallsBackToLoadedWalletsWhileLoadStatesAreNotImplemented() async {
        let model = make()
        await model.load()
        #expect(model.loadStatesUnavailable)
        #expect(model.wallets.map(\.walletID) == [walletA])
        #expect(model.errorMessage == nil)
        await model.rename(walletA, to: "Spending")
        #expect(world.walletState.renames.first?.1 == "Spending")
    }

    @Test func IOS110_removeNeedsAWipeGrant() async {
        world.auth.lockState = .unlocked
        world.lifecycle.removeResult.withLock { $0 = .success(()) }
        let model = make()
        await model.load()
        model.requestRemove(walletA)
        #expect(model.flow == .confirmingRemove(walletA, name: "Main"))
        await model.confirmRemove()
        #expect(model.flow == .needsVaultPassphrase(.removing))
        await model.provideVaultPassphrase("pw")
        #expect(world.auth.authorizeCalls.last?.purpose == .wipe)
        #expect(world.lifecycle.removals.current.first?.0 == walletA)
        #expect(model.flow == .idle)
    }

    @Test func QT107_dumpwalletImportReportsKeysNotImported() async {
        let file = URL(fileURLWithPath: "/tmp/dump.txt")
        m2.importer.kinds.withLock {
            $0[file] = .dumpWallet(
                network: .testnet, hasMnemonic: true, hasHDSeed: true, hasXprv: true, looseKeyCount: 2, scriptCount: 1,
                labelCount: 3)
        }
        m2.importer.report.withLock { $0 = report }
        let model = make()
        await model.importFile(file)
        #expect(model.flow == .imported(report))
        #expect(model.importSummary == [
            "The wallet was imported with 3 labels.",
            "2 loose keys and 1 scripts were not imported. Sweep their funds into this wallet instead.",
        ])
    }

    @Test func QT106_encryptedWalletDatAsksForItsPassphrase() async {
        let file = URL(fileURLWithPath: "/tmp/wallet.dat")
        let kind = WalletFileKind.walletDatSQLite(encrypted: true, hasMnemonic: true)
        m2.importer.kinds.withLock { $0[file] = kind }
        m2.importer.walletDatPassphrase.withLock { $0 = "core-pass" }
        m2.importer.report.withLock { $0 = report }
        let model = make()
        await model.importFile(file)
        #expect(model.flow == .needsFilePassphrase(file, kind))
        await model.provideFilePassphrase("wrong")
        #expect(model.flow == .failed(L10n.Common.wrongPassphrase))
        await model.importFile(file)
        await model.provideFilePassphrase("core-pass")
        #expect(model.flow == .imported(report))
    }

    @Test func QT106_berkeleyDBSaysWhatToDoInstead() async {
        let file = URL(fileURLWithPath: "/tmp/old.dat")
        m2.importer.kinds.withLock { $0[file] = .walletDatBerkeleyDB(encrypted: nil) }
        let model = make()
        await model.importFile(file)
        #expect(model.flow == .failed(L10n.Wallets.berkeleyDBUnavailable))
    }

    @Test func QT078_psbtFileIsHandedToThePSBTDialog() async {
        let file = URL(fileURLWithPath: "/tmp/tx.psbt")
        m2.importer.kinds.withLock { $0[file] = .psbt }
        let model = make()
        await model.importFile(file)
        #expect(model.flow == .openPSBT(file))
    }

    @Test func QT108_hdSeedHexGoesIntoAZeroingBuffer() async {
        m2.importer.report.withLock { $0 = report }
        let model = make()
        await model.importKeyMaterial(.hdSeed, text: "zz")
        #expect(model.flow == .failed(L10n.Wallets.invalidSeedHex))
        await model.importKeyMaterial(.hdSeed, text: String(repeating: "ab", count: 32))
        #expect(m2.importer.materials.current == ["seed:32"])
        #expect(model.flow == .imported(report))
    }

    @Test func QT114_watchOnlyWalletFromAnXpub() async {
        m2.walletLifecycle.states.withLock { $0 = [] }
        let model = make()
        await model.addWatchOnly(xpub: "garbage", name: "Cold")
        #expect(model.flow == .failed(L10n.M2Errors.invalidXpub))
        await model.addWatchOnly(xpub: "tpub" + String(repeating: "D", count: 107), name: "Cold")
        #expect(model.flow == .idle)
        #expect(model.wallets.first?.watchOnly == true && model.wallets.first?.name == "Cold")
    }

    @Test func QT110_backupOfAnUnencryptedVaultNeedsAPassphrase() async {
        let file = URL(fileURLWithPath: "/tmp/w.dwbackup")
        let backup = WalletBackup(file: file, walletID: walletA, createdAt: Date(), sizeBytes: 4096, automatic: false)
        m2.backups.backupResult.withLock { $0 = backup }
        let model = make()
        await model.backup(walletA, to: file, passphrase: nil)
        #expect(model.flow == .failed(L10n.M2Errors.backupPassphraseRequired))
        await model.backup(walletA, to: file, passphrase: "bk")
        #expect(model.flow == .backedUp(backup))
        #expect(m2.backups.backupPassphrases.current == ["bk"])
        // An encrypted vault uses its own passphrase slot.
        world.auth.lockState = .unlocked
        await model.backup(walletA, to: file, passphrase: "ignored")
        #expect(m2.backups.backupPassphrases.current.last == .some(nil))
    }

    @Test func QT110_restoringABackupAsksForItsPassphrase() async {
        let file = URL(fileURLWithPath: "/tmp/w.dwbackup")
        let kind = WalletFileKind.dwBackup(network: .testnet, walletCount: 1, createdAt: Date(), formatVersion: 1)
        m2.importer.kinds.withLock { $0[file] = kind }
        m2.backups.restorePassphrase.withLock { $0 = "bk" }
        m2.backups.restoreIDs.withLock { $0 = [walletB] }
        let model = make()
        await model.importFile(file)
        #expect(model.flow == .needsFilePassphrase(file, kind))
        await model.provideFilePassphrase("bk")
        #expect(model.flow == .restored([walletB]))
    }

    @Test func QT116_automaticBackupsAndFolder() async {
        let folder = URL(fileURLWithPath: "/data/backups")
        let auto = WalletBackup(
            file: folder.appendingPathComponent("Main.2026-10-06-10-00.dwbackup"), walletID: walletA,
            createdAt: Date(), sizeBytes: 1, automatic: true)
        m2.backups.automatic.withLock { $0 = [auto] }
        m2.backups.policyValue.withLock { $0 = BackupPolicy(keep: 10, directory: folder) }
        let model = make()
        await model.load()
        #expect(model.automaticBackups == [auto])
        model.showBackupsFolder()
        #expect(m2.fileRevealer.revealed.current == [folder])
    }

    @Test func QT109_exportForCoreNeedsARevealGrantAndReportsWarnings() async {
        let file = URL(fileURLWithPath: "/tmp/dump.txt")
        m2.exporter.report.withLock {
            $0 = CoreExportReport(file: file, format: .dumpWallet, keyCount: 42, warnings: [.coinJoinAccountNotScannedByLegacyCore])
        }
        m2.exporter.compatible.withLock { $0 = (true, []) }
        let model = make()
        await model.exportForCore(walletA, format: .dumpWallet, to: file)
        #expect(world.auth.authorizeCalls.last?.purpose == .revealSecret)
        guard case .exported(_, let compatible) = model.flow else {
            Issue.record("export did not finish")
            return
        }
        #expect(compatible)
        #expect(model.exportSummary == ["42 keys were exported.", L10n.Wallets.coinJoinNotScanned])
        // The export holds the phrase: the backup reminder stops.
        #expect(m2.desktopPreferences.desktop.backupReminders[walletA.hex]?.backedUp == true)
    }

    @Test func IOS111_xpubWithQRCode() async {
        let key = AccountXpub(account: 0, derivationPath: "m/44'/1'/0'", xpub: "tpubDemo")
        m2.walletLifecycle.xpubs.withLock { $0[walletA] = key }
        let model = make()
        await model.loadXpub(walletA)
        #expect(model.xpub == key)
        #expect(model.xpubQR != nil)
        #expect(world.uri.qrRequests.current == ["tpubDemo"])
    }

    @Test func IOS009_existingDataPromptAndDeleteAllWithTypedSentence() async {
        m2.walletLifecycle.networks.withLock {
            $0 = [NetworkDataInfo(
                network: .testnet, directory: URL(fileURLWithPath: "/data/testnet"), hasWalletState: true, hasVault: true,
                hasOSStoreKey: true)]
        }
        m2.walletLifecycle.states.withLock {
            $0 = [
                WalletLoadState(walletID: walletA, name: "Main", loaded: true, loadOnStartup: true, watchOnly: false),
                WalletLoadState(walletID: walletB, name: "Old", loaded: false, loadOnStartup: false, watchOnly: false),
            ]
        }
        let removed = Locked<Set<WalletID>>([])
        world.lifecycle.removeResult.withLock { $0 = .success(()) }
        world.lifecycle.onRemove.withLock { $0 = { id in removed.withLock { _ = $0.insert(id) } } }
        m2.recovery.remainingWallets.withLock { $0 = { 2 - removed.current.count } }
        let model = make()
        await model.loadExistingData()
        #expect(model.showsExistingDataPrompt)
        await model.deleteAll(acceptance: "I accept")
        #expect(model.flow == .failed(L10n.Wallets.acceptPhraseMismatch))
        await model.deleteAll(acceptance: L10n.Wallets.wipeAcceptPhrase)
        #expect(model.flow == .deletedAll)
        // The unloaded wallet was loaded first so the engine could remove it.
        #expect(removed.current == [walletA, walletB])
        #expect(m2.recovery.destroyed.current == 1)
        #expect(!model.showsExistingDataPrompt)
    }
}

@MainActor
@Suite("Security view model")
struct SecurityViewModelTests {
    let world = FakeWorld()

    func make(_ m2: FakeM2World) -> SecurityViewModel { SecurityViewModel(env: world.environment(), m2: m2.services) }

    func encryptedVault() {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .unlocked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil,
                walletsWithSecrets: [walletA])
        }
        world.auth.lockState = .unlocked
    }

    @Test func IOS011_quickUnlockEnrollsWithAChangeCredentialGrantAndDefaultLimit() async {
        encryptedVault()
        let m2 = FakeM2World()
        m2.quickUnlock.current.withLock {
            $0 = QuickUnlockPolicy(enrolled: false, spendLimit: .zero, passphraseMaxAge: .seconds(604_800), lastPassphraseEntry: nil)
        }
        let model = make(m2)
        await model.load()
        #expect(model.showsQuickUnlock && model.providerName == "Touch ID")
        await model.enableQuickUnlock()
        #expect(model.quickUnlockFlow == .needsPassphrase(.enroll))
        await model.provideQuickUnlockPassphrase("pw")
        #expect(world.auth.authorizeCalls.last?.purpose == .changeCredential)
        #expect(world.auth.authorizeCalls.last?.wallet == nil)
        #expect(model.policy?.enrolled == true)
        #expect(model.policy?.spendLimit == Amount(duffs: 50_000_000))
    }

    @Test func IOS011_quickUnlockIsHiddenWithoutBiometricsOrEncryption() async {
        let m2 = FakeM2World(quickUnlockProvider: .unavailable)
        encryptedVault()
        let model = make(m2)
        await model.load()
        #expect(!model.showsQuickUnlock)
        let unencrypted = make(FakeM2World())
        world.vault.state.withLock { $0.status = VaultStatus(state: .unencrypted, encrypted: false, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSeconds: nil, walletsWithSecrets: []) }
        await unencrypted.load()
        #expect(!unencrypted.showsQuickUnlock)
    }

    @Test func IOS016_spendLimitOptionsAreTheIOSOnes() async {
        encryptedVault()
        let m2 = FakeM2World()
        m2.quickUnlock.current.withLock {
            $0 = QuickUnlockPolicy(enrolled: true, spendLimit: QuickUnlockPolicy.defaultSpendLimit, passphraseMaxAge: .seconds(604_800), lastPassphraseEntry: nil)
        }
        let model = make(m2)
        #expect(model.spendLimitOptions.map { model.spendLimitText($0) } == [
            "0.00000000 tDASH", "0.10000000 tDASH", "0.50000000 tDASH", "1.00000000 tDASH", "5.00000000 tDASH",
        ])
        await model.setSpendLimit(Amount(duffs: 123))
        #expect(model.quickUnlockFlow == .idle && m2.quickUnlock.grants.current.isEmpty)
        await model.setSpendLimit(Amount(duffs: 100_000_000), passphrase: "pw")
        #expect(model.policy?.spendLimit == Amount(duffs: 100_000_000))
    }

    @Test func IOS015_autoLockDefaultsToNeverAndIsSet() {
        let m2 = FakeM2World()
        let model = make(m2)
        #expect(model.autoLockInterval == .never)
        model.setAutoLock(.fiveMinutes)
        #expect(m2.autoLock.interval == .fiveMinutes)
        #expect(L10n.Security.autoLockName(.oneDay) == "24 hours")
    }

    @Test func IOS016_requireAuthenticationAndAutohideBalance() {
        let m2 = FakeM2World()
        let model = make(m2)
        #expect(model.requireAuthenticationForEveryPayment)
        model.setRequireAuthenticationForEveryPayment(false)
        #expect(!m2.paymentAuthentication.requireAuthenticationForEveryPayment)
        model.setAutohideBalance(true)
        #expect(world.settings.display.hideBalances)
    }

    @Test func IOS108_revealPhraseOfAChosenWalletRecordsTheBackup() async {
        encryptedVault()
        let m2 = FakeM2World()
        let model = make(m2)
        #expect(await model.revealPhrase(wallet: walletA) == nil)
        #expect(model.revealNeedsPassphrase)
        let revealed = await model.revealPhrase(wallet: walletA, passphrase: "pw")
        #expect(revealed?.phrase.testString.hasPrefix("abandon") == true)
        #expect(m2.desktopPreferences.desktop.backupReminders[walletA.hex]?.backedUp == true)
    }

    @Test func IOS014_forgotPassphraseChecksThePhraseThenReplacesTheVault() async {
        let m2 = FakeM2World()
        let phrase = "abandon ability able about above absent absorb abstract absurd abuse access accident"
        m2.recovery.phrases.withLock { $0 = [walletA: phrase, walletB: "other words"] }
        world.walletState.wallets = [walletInfo(walletA), walletInfo(walletB, name: "Savings")]
        let model = make(m2)
        model.startForgotPassphrase()
        world.vault.state.withLock {
            $0.checks["bad phrase"] = MnemonicCheck(wordCount: 2, unknownWordIndices: [1], language: nil, checksum: .invalid)
        }
        await model.submitRecoveryPhrase("bad phrase", wallet: walletA)
        #expect(model.errorMessage == L10n.Security.invalidPhrase)
        await model.submitRecoveryPhrase("  Abandon ability able about above absent absorb abstract absurd abuse access accident ", wallet: walletA)
        #expect(model.forgotPassphrase == .choosingPassphrase(wallet: walletA))
        await model.chooseNewPassphrase("new", confirmation: "nope")
        #expect(model.errorMessage == L10n.Settings.passphraseMismatch)
        await model.chooseNewPassphrase("new", confirmation: "new")
        guard case .done = model.forgotPassphrase else {
            Issue.record("recovery did not finish: \(model.forgotPassphrase)")
            return
        }
        #expect(m2.recovery.recoveries.current.first?.1 == "new")
        #expect(model.walletsWithoutSecretsText == L10n.Security.walletsWithoutSecrets(["Savings"]))
    }

    @Test func IOS014_aPhraseOfAnotherWalletIsAMismatch() async {
        let m2 = FakeM2World()
        m2.recovery.phrases.withLock { $0 = [walletA: "one two three"] }
        let model = make(m2)
        model.startForgotPassphrase()
        await model.submitRecoveryPhrase("four five six", wallet: walletA)
        await model.chooseNewPassphrase("new", confirmation: "new")
        #expect(model.forgotPassphrase == .failed(L10n.M2Errors.recoveryMismatch))
    }

    @Test func IOS109_wipeNeedsTheSentenceThenRemovesEveryWalletAndDestroysTheVault() async {
        let m2 = FakeM2World()
        let removed = Locked(0)
        world.lifecycle.removeResult.withLock { $0 = .success(()) }
        world.lifecycle.onRemove.withLock { $0 = { _ in removed.withLock { $0 += 1 } } }
        m2.recovery.remainingWallets.withLock { $0 = { 1 - removed.current } }
        let model = make(m2)
        await model.wipe(confirmation: L10n.Wallets.wipeAcceptPhrase)
        #expect(model.wipeStep == .idle)  // not requested
        model.requestWipe()
        await model.wipe(confirmation: "yes")
        #expect(model.errorMessage == L10n.Wallets.acceptPhraseMismatch)
        await model.wipe(confirmation: L10n.Wallets.wipeAcceptPhrase)
        #expect(model.wipeStep == .done)
        #expect(world.lifecycle.removals.current.map(\.0) == [walletA])
        #expect(model.vaultStatus?.state == .noVault)
    }

    @Test func IOS109_aFailedRemovalStopsBeforeTheVaultIsDestroyed() async {
        let m2 = FakeM2World()
        world.lifecycle.removeResult.withLock { $0 = .failure(ServiceError(code: .walletNotFound)) }
        let model = make(m2)
        model.requestWipe()
        await model.wipe(confirmation: L10n.Wallets.wipeAcceptPhrase)
        #expect(model.wipeStep == .failed(L10n.Common.walletNotFound))
        #expect(m2.recovery.destroyed.current == 0)
    }
}

@MainActor
@Suite("Home additions")
struct HomeM2Tests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    // MARK: Shortcut bar (IOS-025, IOS-121)

    func bar(network: DashNetwork = .testnet) -> ShortcutBarViewModel {
        ShortcutBarViewModel(env: world.environment(), m2: m2.services, network: network)
    }

    @Test func IOS025_defaultsFollowBalanceAndBackupState() {
        // Zero balance, backed up / untracked.
        #expect(bar().slots.map(\.action) == [.receive, .send, .buySell, .testnetFaucet])
        // Zero balance, needs backup.
        m2.desktopPreferences.desktop.backupReminders[walletA.hex] = BackupReminderState()
        #expect(bar().slots.map(\.action) == [.backup, .receive, .buySell, .testnetFaucet])
        // Balance, needs backup; mainnet ends with Spend.
        world.walletState.balances = balances(confirmed: 1000)
        #expect(bar(network: .mainnet).slots.map(\.action) == [.backup, .receive, .send, .spend])
        // Balance, backed up.
        m2.desktopPreferences.desktop.backupReminders[walletA.hex]?.backedUp = true
        #expect(bar().slots.map(\.action) == [.receive, .send, .scanQR, .testnetFaucet])
    }

    @Test func IOS025_laterReleaseActionsAreDisabledWithAReason() {
        let model = bar(network: .mainnet)
        let spend = model.slots.first { $0.action == .buySell }!
        #expect(!spend.isEnabled && spend.helpText == L10n.HomeM2.laterRelease)
        #expect(model.route(for: spend) == nil)
        #expect(model.route(for: model.slots[0]) == .section(.receive))
    }

    @Test func IOS025_customisationIsSavedAndDegradesOnOtherNetworks() {
        let model = bar()
        model.setSlot(3, to: .scanQR)
        #expect(m2.desktopPreferences.desktop.shortcuts == [.receive, .send, .buySell, .scanQR])
        model.setSlot(0, to: .testnetFaucet)
        // The faucet saved on testnet becomes Spend on mainnet; the saved bar is kept.
        #expect(bar(network: .mainnet).slots[0].action == .spend)
        #expect(m2.desktopPreferences.desktop.shortcuts?[0] == .testnetFaucet)
        model.resetToDefaults()
        #expect(m2.desktopPreferences.desktop.shortcuts == nil)
        #expect(!model.customizableActions.contains(.switchWallet))
    }

    @Test func IOS121_faucetOpensTheWebFaucetOnTestnetOnly() {
        let model = bar()
        let faucet = model.slots.first { $0.action == .testnetFaucet }!
        #expect(model.route(for: faucet) == .openURL(URL(string: "https://faucet.testnet.networks.dash.org/")!))
        #expect(FaucetShortcut.url(for: .mainnet) == nil)
        #expect(!FaucetShortcut.inAppFaucetAvailable)
    }

    // MARK: Backup reminder (IOS-005)

    @Test func IOS005_dueOnce24HoursAfterFirstFundsWhileUnbacked() {
        let reminder = BackupReminderViewModel(env: world.environment(), m2: m2.services)
        reminder.walletCreated(walletA)
        reminder.evaluate()
        #expect(!reminder.isDue)
        world.walletState.balances = balances(confirmed: 5000)
        reminder.evaluate()
        #expect(m2.desktopPreferences.desktop.backupReminders[walletA.hex]?.firstFundsAt == world.clock.now)
        world.clock.advance(23 * 3600)
        reminder.evaluate()
        #expect(!reminder.isDue)
        world.clock.advance(2 * 3600)
        reminder.evaluate()
        #expect(reminder.isDue)
        reminder.markShown()
        reminder.evaluate()
        #expect(!reminder.isDue)
    }

    @Test func IOS005_restoredWalletsAndBackedUpPhrasesGetNoReminder() {
        let reminder = BackupReminderViewModel(env: world.environment(), m2: m2.services)
        world.walletState.balances = balances(confirmed: 5000)
        world.clock.advance(-200_000)
        reminder.evaluate()
        world.clock.advance(400_000)
        reminder.evaluate()
        #expect(!reminder.isDue)  // untracked = restored
        reminder.walletCreated(walletA)
        reminder.markBackedUp(walletA)
        reminder.evaluate()
        #expect(!reminder.isDue)
    }

    // MARK: Tray companion (IOS-117)

    @Test func IOS117_requestAmountBuildsAURIAndPayFromClipboardRoutesToSend() async {
        let model = MenuBarCompanionViewModel(env: world.environment(), m2: m2.services)
        await model.load()
        #expect(model.requestURI == "dash:XcurrentAddress000000000000000001")
        model.setRequestAmount("1.5")
        #expect(model.requestURI == "dash:XcurrentAddress000000000000000001?amount=1.5")
        #expect(model.qr != nil)
        model.setRequestAmount("abc")
        #expect(model.requestAmountError == L10n.Receive.invalidAmount && model.requestURI == nil)
        m2.clipboard.text.withLock { $0 = "dash:\(testnetAddress2)?amount=2" }
        model.payFromClipboard()
        #expect(model.route == .send(PaymentURI(address: testnetAddress2, amount: Amount(duffs: 200_000_000), label: nil, message: nil)))
        m2.clipboard.text.withLock { $0 = "hello" }
        model.routeHandled()
        model.payFromClipboard()
        #expect(model.route == nil && model.errorMessage == L10n.Send.invalidAddress)
    }
}
