// Settings (QT-020, QT-036, QT-039, QT-111, QT-113, IOS-104/106), the main
// window model and sidebar routing (QT-011…014, QT-019).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Settings view model")
struct SettingsViewModelTests {
    let world = FakeWorld()

    func makeModel(developerMode: Bool = false) -> SettingsViewModel {
        SettingsViewModel(env: world.environment(developerMode: developerMode))
    }

    @Test func IOS106_switchNetworkGoesThroughTheLifecycleQueue() async {
        let model = makeModel()
        await model.load()
        #expect(model.network == .testnet)
        #expect(model.availableNetworks == [.mainnet, .testnet])
        await model.switchNetwork(to: .regtest)
        #expect(world.lifecycle.switches.current.isEmpty)
        await model.switchNetwork(to: .mainnet)
        #expect(world.lifecycle.switches.current == [.mainnet])
        #expect(model.network == .mainnet)
        #expect(makeModel(developerMode: true).availableNetworks == [.mainnet, .testnet, .regtest])
    }

    @Test func IOS018_followsLifecycleTransitions() async {
        let model = makeModel()
        model.start()
        world.lifecycle.publish(.switchingNetwork(from: .testnet, to: .mainnet))
        await eventually { model.isSwitchingNetwork }
        world.lifecycle.publish(.idle)
        await eventually { !model.isSwitchingNetwork }
        model.stop()
    }

    @Test func switchFailureIsShown() async {
        world.lifecycle.switchError.withLock { $0 = ServiceError(code: .storage) }
        let model = makeModel()
        await model.load()
        await model.switchNetwork(to: .mainnet)
        #expect(model.network == .testnet)
        #expect(model.errorMessage == L10n.Common.storage)
    }

    @Test func QT020_QT036_QT039_displaySettingsPersistAndClamp() {
        let model = makeModel()
        model.setUnit(.milliDash)
        model.setDecimalDigits(12)
        #expect(model.display.decimalDigits == 8)
        model.setDecimalDigits(0)
        #expect(model.display.decimalDigits == 2)
        model.setDiscreet(true)
        #expect(world.settings.display == DisplaySettings(unit: .milliDash, decimalDigits: 2, hideBalances: true))
        world.settings.updateError = ServiceError(code: .settingsWriteFailed)
        model.setUnit(.duffs)
        #expect(model.display.unit == .milliDash)
        #expect(model.errorMessage == L10n.Settings.settingsNotSaved)
    }

    @Test func themeAndLanguage() {
        let model = makeModel()
        model.setTheme(.dark)
        #expect(world.preferences.preferences.theme == .dark)
        #expect(model.theme == .dark)
        model.setLanguage("fr")
        #expect(model.languageCode == nil)
        model.setLanguage("en")
        #expect(world.preferences.preferences.languageCode == "en")
    }

    @Test func QT111_encryptWallet() async {
        let model = makeModel()
        await model.encryptWallet(passphrase: "a", confirmation: "b")
        #expect(model.errorMessage == L10n.Settings.passphraseMismatch)
        #expect(world.vault.state.current.encrypts.isEmpty)
        await model.encryptWallet(passphrase: "new pass", confirmation: "new pass")
        #expect(world.auth.authorizeCalls.last?.purpose == .changeCredential)
        #expect(world.vault.state.current.encrypts.first?.passphrase == "new pass")
        #expect(world.vault.state.current.encrypts.first?.grant.purpose == .changeCredential)
        #expect(model.vault?.encrypted == true)
        #expect(model.infoMessage == L10n.Settings.walletEncrypted)
        world.vault.setError(ServiceError(code: .init(rawValue: "vault.already_encrypted")), for: "encrypt")
        await model.encryptWallet(passphrase: "x", confirmation: "x")
        #expect(model.errorMessage == L10n.Settings.alreadyEncrypted)
    }

    @Test func QT111_changePassphrase() async {
        let model = makeModel()
        await model.changePassphrase(old: "old", new: "", confirmation: "")
        #expect(model.errorMessage == L10n.Settings.passphraseEmpty)
        await model.changePassphrase(old: "old", new: "new", confirmation: "new")
        #expect(world.vault.state.current.passphraseChanges.first?.old == "old")
        #expect(world.vault.state.current.passphraseChanges.first?.new == "new")
        #expect(model.infoMessage == L10n.Settings.passphraseChanged)
        world.vault.setError(ServiceError(code: .vaultWrongPassphrase), for: "changePassphrase")
        await model.changePassphrase(old: "bad", new: "n", confirmation: "n")
        #expect(model.errorMessage == L10n.Common.wrongPassphrase)
    }

    @Test func QT113_revealNeedsAuthorizationFirst() async {
        world.auth.lockState = .locked
        let model = makeModel()
        #expect(await model.revealPhrase() == nil)
        #expect(model.needsPassphrase)
        #expect(world.vault.state.current.reveals.isEmpty)
        let revealed = await model.revealPhrase(passphrase: "pw")
        #expect(revealed?.phrase.testString == world.vault.state.current.generated)
        #expect(world.auth.authorizeCalls.last?.purpose == .revealSecret)
        #expect(world.vault.state.current.reveals.first?.1.purpose == .revealSecret)
        #expect(!model.needsPassphrase)
        world.vault.setError(ServiceError(code: .vaultNoSecret), for: "revealMnemonic")
        #expect(await model.revealPhrase(passphrase: "pw") == nil)
        #expect(model.errorMessage == L10n.Settings.noRecoveryPhrase)
    }
}

@MainActor
@Suite("Navigation and main view model")
struct MainViewModelTests {
    let world = FakeWorld()

    @Test func QT012_sidebarShowsM1SectionsWithRenumberedShortcuts() {
        #expect(SidebarItem.visible(with: .m1) == [.overview, .send, .receive, .transactions])
        #expect(SidebarItem.transactions.shortcutNumber(with: .m1) == 4)
        #expect(SidebarItem.coinJoin.shortcutNumber(with: .m1) == nil)
        let withCoinJoin = FeatureFlags(coinJoin: true, contacts: true)
        #expect(SidebarItem.visible(with: withCoinJoin) == [.overview, .send, .receive, .transactions, .coinJoin, .contacts])
        #expect(SidebarItem.contacts.shortcutNumber(with: withCoinJoin) == 6)
        #expect(SidebarItem.overview.title == "Overview")
    }

    @Test func QT011_windowTitleTagsEveryNonMainnetNetwork() async {
        let model = MainViewModel(env: world.environment())
        await model.start()
        #expect(model.network == .testnet)
        #expect(model.windowTitle == "Dash Wallet - Main - [testnet]")
        #expect(MainViewModel.networkTag(.devnet(name: "ouzo")) == "[devnet: ouzo]")
        #expect(MainViewModel.networkTag(.regtest) == "[regtest]")
        #expect(MainViewModel.networkTag(.mainnet) == nil)
        model.stop()
    }

    @Test func QT013_noWalletStartsOnboarding() async {
        world.walletState.wallets = []
        world.walletState.selectedWalletID = nil
        let model = MainViewModel(env: world.environment())
        await model.start()
        #expect(model.needsOnboarding)
        #expect(model.onboarding != nil)
        // A wallet was added: the main window takes over.
        world.walletState.wallets = [walletInfo(walletA)]
        world.walletState.selectedWalletID = walletA
        world.walletState.notify()
        await eventually { !model.needsOnboarding }
        model.stop()
    }

    @Test func QT014_walletSelectorWithTwoWallets() async {
        world.walletState.wallets = [walletInfo(walletA), walletInfo(walletB, name: "Savings")]
        let model = MainViewModel(env: world.environment())
        await model.start()
        #expect(model.showsWalletSelector)
        await model.selectWallet(walletB)
        #expect(model.selectedWalletID == walletB)
        #expect(model.windowTitle == "Dash Wallet - Savings - [testnet]")
        model.stop()
    }

    @Test func QT019_openURIRoutesToSendAndFillsIt() async {
        let model = MainViewModel(env: world.environment())
        await model.start()
        await model.open(uri: "dash:\(testnetAddress1)?amount=2")
        #expect(model.selection == .send)
        #expect(model.send?.entries.first?.address == testnetAddress1)
        #expect(model.send?.entries.first?.amountText == "2.00000000")
        await model.open(uri: "dash:bad")
        #expect(model.errorMessage == L10n.Send.invalidAddress)
        model.stop()
    }

    @Test func routesAndShortcuts() async {
        world.history.state.withLock { $0.records = [record(txid(3), amount: 1)] }
        let model = MainViewModel(env: world.environment())
        await model.start()
        model.selectShortcut(3)
        #expect(model.selection == .receive)
        model.selectShortcut(5)
        #expect(model.selection == .receive)
        await model.navigate(.section(.coinJoin))
        #expect(model.selection == .receive)
        await model.navigate(.transaction(txid: txid(3)))
        #expect(model.selection == .transactions)
        #expect(model.transactions?.selection == [TxRecord.ID(txid: txid(3), recordIndex: 0)])
        model.stop()
    }

    /// The sync overlay shows by itself once the tip is more than 25
    /// minutes old, re-checked on a timer without a new sync status; hiding
    /// it lasts until another network opens (QT-027).
    @Test func QT027_syncOverlayFollowsTheTipAgeAndResetsOnNetworkChange() async {
        let tip = world.clock.now.addingTimeInterval(-20 * 60)
        world.sync.status = SyncStatus(
            running: true, phases: [], activePhase: .filters, tipHeight: 100, tipDate: tip, chainLockHeight: nil,
            connectedPeers: 8, progress: 0.5, isDone: false, isStalled: false)
        let model = MainViewModel(env: world.environment())
        await model.start()
        #expect(!model.showsSyncOverlay)
        await eventually { world.sleeper.requested.current.contains(MainViewModel.syncOverlayRecheck) }
        world.clock.advance(6 * 60)
        world.sleeper.fireNext()
        await eventually { model.showsSyncOverlay }
        model.hideSyncOverlay()
        #expect(!model.showsSyncOverlay)

        world.host.network.withLock { $0 = .mainnet }
        world.lifecycle.publish(.idle)
        await eventually { model.network == .mainnet }
        #expect(!model.syncOverlayHidden)
        #expect(model.showsSyncOverlay)
        model.stop()
    }

    @Test func IOS013_IOS018_lockAndTransitionOverlays() async {
        let model = MainViewModel(env: world.environment())
        await model.start()
        #expect(!model.showsLockScreen)
        world.auth.publish(.locked)
        await eventually { model.showsLockScreen }
        world.lifecycle.publish(.starting(.testnet))
        await eventually { model.showsTransitionOverlay }
        world.lifecycle.publish(.idle)
        await eventually { !model.showsTransitionOverlay }
        model.stop()
    }
}

@Suite("Input validation")
struct InputValidationTests {
    @Test func cleanRemovesWhitespaceAndInvisibleCharacters() {
        #expect(AddressInput.clean(" a\u{200B}b\u{FEFF}c\n\t") == "abc")
    }

    @Test func QT056_amountParsing() {
        let formatter = AmountFormatter(network: .testnet)
        #expect(AmountInput.parse("", unit: .dash, formatter: formatter) == .success(nil))
        #expect(AmountInput.parse("1,5", unit: .dash, formatter: formatter) == .success(Amount(duffs: 150_000_000)))
        #expect(AmountInput.parse("1.123456789", unit: .dash, formatter: formatter) == .failure(.unparsable))
        #expect(AmountInput.parse("21000001", unit: .dash, formatter: formatter) == .failure(.outOfRange))
        #expect(AmountInput.parsePayment("546", unit: .duffs, formatter: formatter) == .success(Amount(duffs: 546)))
        #expect(AmountInput.parsePayment("545", unit: .duffs, formatter: formatter) == .failure(.dust))
        #expect(AmountInput.parsePayment("-1", unit: .dash, formatter: formatter) == .failure(.notPositive))
        #expect(AmountInput.parsePayment("", unit: .dash, formatter: formatter) == .failure(.notPositive))
    }
}
