// PSBT operations (QT-076…079) and the Tools window: Information, Console,
// Peers, Repair (QT-040, QT-117, QT-143, QT-145, QT-147, QT-148, IOS-034, IOS-113).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

func psbtAnalysis(
    status: PSBTStatus, signability: PSBTSignability = .canSign, unsigned: Int = 1, external: Int64? = 100_000_000
) -> PSBTAnalysis {
    PSBTAnalysis(
        outputs: [
            PSBTOutput(address: testnetAddress2, amount: Amount(duffs: 100_000_000), isMine: false),
            PSBTOutput(address: testnetAddress1, amount: Amount(duffs: 50_000), isMine: true),
        ],
        fee: Amount(duffs: 374), total: Amount(duffs: 100_050_374), unsignedInputs: unsigned, status: status,
        signability: signability, externalSent: external.map { Amount(duffs: $0) })
}

@MainActor
@Suite("PSBT view model")
struct PSBTViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    func make() -> PSBTViewModel { PSBTViewModel(env: world.environment(), m2: m2.services) }

    @Test func QT077_createUnsignedCopiesBase64AndOffersSave() async {
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .needsSignatures) }
        let model = make()
        await model.createUnsigned(draft: FakeDraft(addresses: world.uri))
        #expect(model.step == .ready)
        #expect(m2.clipboard.text.current?.hasPrefix("cHNidP8B") == true)
        #expect(model.message == "The PSBT has been copied to the clipboard. You can also save it.")
        #expect(model.suggestedFileName.hasSuffix(".psbt"))
        #expect(model.suggestedFileName.hasPrefix(testnetAddress2 + "-"))
    }

    @Test func QT078_loadFromClipboardNeedsBase64() async {
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .needsSignatures) }
        let model = make()
        m2.clipboard.text.withLock { $0 = "not base64 !!" }
        await model.loadFromClipboard()
        #expect(model.errorMessage == "Unable to decode PSBT from clipboard (invalid base64)")
        m2.clipboard.text.withLock { $0 = "cHNidP8BAAA=" }
        await model.loadFromClipboard()
        #expect(model.step == .ready)
    }

    @Test func QT078_tooLargeFileIsRefusedByTheEngineRule() async {
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .needsSignatures) }
        let model = make()
        await model.load(data: Data(count: FakePSBT.maxSize + 1))
        #expect(model.step == .empty)
        #expect(model.errorMessage == "Failed to load transaction: PSBT file must be smaller than 100 MiB")
    }

    @Test func QT079_dialogLinesAndStatusFollowDashQt() async {
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .needsSignatures, signability: .watchOnly, unsigned: 2) }
        let model = make()
        await model.load(data: Data("cHNidP8B".utf8))
        #expect(model.descriptionLines[0] == " * Sends 1.00000000 tDASH to \(testnetAddress2)")
        #expect(model.descriptionLines[1] == " * Sends 0.00050000 tDASH to \(testnetAddress1) (own address)")
        #expect(model.descriptionLines.contains("Pays transaction fee: 0.00000374 tDASH"))
        #expect(model.descriptionLines.last == "Transaction has 2 unsigned inputs.")
        #expect(model.statusLine == "Transaction still needs signature(s). (But this wallet cannot sign transactions.)")
        #expect(!model.canSign && !model.canBroadcast)
    }

    @Test func QT079_signNeedsASpendGrantCoveringExternalSent() async {
        world.auth.lockState = .unlocked
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .needsSignatures) }
        m2.psbt.signedAnalysis.withLock { $0 = psbtAnalysis(status: .complete, unsigned: 0) }
        let model = make()
        await model.load(data: Data("cHNidP8B".utf8))
        #expect(model.canSign)
        await model.sign()
        #expect(model.step == .needsPassphrase)
        await model.sign(passphrase: "secret")
        #expect(world.auth.authorizeCalls.last?.purpose == .spend(max: Amount(duffs: 100_000_000)))
        #expect(world.auth.authorizeCalls.last?.passphrase == "secret")
        #expect(model.message == "Signed transaction successfully. Transaction is ready to broadcast.")
        #expect(model.canBroadcast)
        #expect(m2.psbt.released.current.count == 1)
    }

    @Test func QT079_broadcastReportsTheTxidOrTheFailure() async {
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .complete, unsigned: 0) }
        let model = make()
        await model.load(data: Data("cHNidP8B".utf8))
        m2.psbt.broadcastResult.withLock { $0 = .failure(ServiceError(code: .psbtFeeRateTooHigh)) }
        await model.broadcast()
        #expect(model.errorMessage == "Transaction broadcast failed: The fee rate is above the broadcast limit of 0.1 DASH/kB.")
        #expect(model.step == .ready)
        m2.psbt.broadcastResult.withLock { $0 = .success(txid(5)) }
        await model.broadcast()
        #expect(model.step == .broadcast(txid: txid(5)))
        #expect(model.message == "Transaction broadcast successfully! Transaction ID: \(txid(5))")
    }

    @Test func QT079_copyAndClose() async {
        m2.psbt.loadAnalysis.withLock { $0 = psbtAnalysis(status: .complete, unsigned: 0) }
        let model = make()
        await model.load(data: Data("cHNidP8B".utf8))
        model.copy()
        #expect(model.message == "PSBT copied to clipboard.")
        model.close()
        #expect(model.step == .empty && m2.psbt.released.current.count == 1)
    }

    @Test func QT076_notImplementedCreateLeavesNoEmptySuccess() async {
        let model = make()
        await model.createUnsigned(draft: FakeDraft(addresses: world.uri))
        #expect(model.step == .empty)
        #expect(model.errorMessage == L10n.Common.notAvailableYet)
        #expect(m2.clipboard.text.current == nil)
    }
}

@MainActor
@Suite("Tools view models")
struct ToolsViewModelTests {
    let world = FakeWorld()
    let m2 = FakeM2World()

    // MARK: Information (QT-143, QT-040)

    @Test func QT143_unknownAndFullNodeOnlyRowsShowADash() async {
        m2.nodeInformation.info.withLock { $0 = FakeNodeInformation.sample() }
        let model = InformationViewModel(env: world.environment(), m2: m2.services)
        await model.load()
        let rows = model.sections.flatMap(\.rows)
        #expect(rows.first { $0.title == "Number of connections" }?.value == "8 (In: 0 / Out: 8)")
        #expect(rows.first { $0.title == "Name" }?.value == "test")
        let mempool = rows.first { $0.title == "Current number of transactions" }
        #expect(mempool?.value == "—" && mempool?.note == "Requires full-node data source")
        #expect(rows.first { $0.title == "Masternodes" }?.value == "Total: 3000 (Enabled: 2900)")
        #expect(rows.first { $0.title == "EvoNodes" }?.value == "—")
    }

    @Test func QT040_bannerShowsTheMostSevereWarning() async {
        m2.nodeInformation.info.withLock { $0 = FakeNodeInformation.sample() }
        m2.nodeInformation.warningList.withLock { $0 = [.clockSkew, .syncStalled] }
        let model = InformationViewModel(env: world.environment(), m2: m2.services)
        await model.load()
        #expect(model.bannerText == L10n.Tools.clockSkew)
    }

    // MARK: Console (QT-145)

    func console() -> ConsoleViewModel { ConsoleViewModel(env: world.environment(), m2: m2.services) }

    @Test func QT145_welcomeHasTheAntiScamWarningAndClearKeepsIt() {
        let model = console()
        #expect(model.entries.map(\.kind) == [.welcome, .warning])
        #expect(model.entries[1].text.hasPrefix("WARNING: Scammers have been active"))
        model.clear()
        #expect(model.entries.count == 2)
    }

    @Test func QT145_sensitiveLinesAreRedactedInEchoAndHistory() async {
        let model = console()
        await model.run(line: "walletpassphrase \"hunter2\" 60")
        #expect(model.history == ["walletpassphrase(…)"])
        #expect(model.entries.contains { $0.kind == .command && $0.text == "walletpassphrase(…)" })
        #expect(!model.entries.contains { $0.text.contains("hunter2") })
    }

    @Test func QT145_historyKeepsFiftyAndBrowsesWithArrows() async {
        let model = console()
        for n in 0..<55 { await model.run(line: "getblockcount \(n)") }
        #expect(model.history.count == 50)
        #expect(model.historyUp() == "getblockcount 54")
        #expect(model.historyUp() == "getblockcount 53")
        #expect(model.historyDown() == "getblockcount 54")
        #expect(model.historyDown() == "")
    }

    @Test func QT145_parseErrorsAreNotKeptAndRPCErrorsPrintCoreLine() async {
        let model = console()
        await model.run(line: "getblock \"abc")
        #expect(model.errorMessage == "Error: Invalid command line")
        #expect(model.history.isEmpty)
        await model.run(line: "getblock abc")
        #expect(model.entries.last?.text == "Block not found (code -5)")
        await model.run(line: "getmempoolinfo")
        #expect(model.entries.last?.text == "Not available in SPV mode.")
    }

    @Test func QT145_authorizationRequiredAsksAndRunsTheLineAgain() async {
        world.auth.lockState = .unlocked
        let model = console()
        await model.load()
        await model.run(line: "sendtoaddress \(testnetAddress2) 1")
        #expect(model.state == .awaitingPassphrase(.spend(max: Amount(duffs: 100_000_000)), wallet: walletA))
        await model.authorize(passphrase: "pw")
        #expect(model.state == .idle)
        #expect(model.entries.last?.text == txid(9))
        #expect(m2.console.executed.current.last?.2 != nil)
        #expect(world.auth.authorizeCalls.last?.passphrase == "pw")
    }

    @Test func QT145_unencryptedVaultAuthorizesWithoutAPrompt() async {
        let model = console()
        await model.load()
        await model.run(line: "sendtoaddress \(testnetAddress2) 1")
        #expect(model.state == .idle)
        #expect(model.entries.last?.text == txid(9))
    }

    @Test func QT145_walletSelectorWithTwoWalletsAndFontSize() async {
        world.walletState.wallets = [walletInfo(walletA), walletInfo(walletB, name: "Savings")]
        let model = console()
        #expect(model.showsWalletSelector)
        #expect(model.selectedWalletID == walletA)
        model.selectWallet(walletB)
        #expect(model.entries.last?.text == "Executing command using \"Savings\" wallet")
        model.selectWallet(nil)
        #expect(model.entries.last?.text == "Executing command without any wallet")
        await model.run(line: "getbalance")
        #expect(model.entries.last?.text == L10n.Tools.walletRequired)
        for _ in 0..<50 { model.increaseFontSize() }
        #expect(model.fontSize == 40)
        #expect(m2.desktopPreferences.desktop.consoleFontSize == 40)
        await model.load()
        #expect(model.completions(for: "get") == ["getbalance", "getblockcount", "getnetworkinfo"])
    }

    // MARK: Peers (QT-147)

    @Test func QT147_disconnectBanAndUnban() async {
        m2.peers.connected.withLock { $0 = ["203.0.113.1:19999"] }
        let model = PeersViewModel(sync: world.sync, moderation: m2.peers)
        await model.load()
        #expect(model.canModerate && !model.showsBannedList)
        await model.ban("203.0.113.1:19999", for: .day)
        #expect(m2.peers.calls.current == ["ban 203.0.113.1:19999 86400"])
        #expect(model.showsBannedList)
        await model.unban("203.0.113.1:19999/32")
        #expect(model.banned.isEmpty)
        await model.disconnect("198.51.100.9:19999")
        #expect(model.error == L10n.M2Errors.peerNotFound)
        #expect(BanDuration.allCases.map(\.title) == ["Ban for 1 hour", "Ban for 1 day", "Ban for 1 week", "Ban for 1 year"])
    }

    @Test func QT147_moderationNotImplementedSaysSo() async {
        let model = PeersViewModel(sync: world.sync, moderation: m2.peers)
        await model.load()
        #expect(model.error == nil)
        await model.disconnect("a")
        #expect(model.error == L10n.Common.notAvailableYet)
    }

    // MARK: Repair (QT-117, QT-148, IOS-034, IOS-113)

    func repair() -> RepairViewModel { RepairViewModel(env: world.environment(), m2: m2.services) }

    @Test func QT117_rescanWithProgressAndCancel() async {
        m2.repair.configured.withLock { $0 = true }
        world.sync.rescanResult = .success(())
        let model = repair()
        m2.repair.progress.withLock {
            $0 = RescanProgress(fromHeight: 0, currentHeight: 250, targetHeight: 1000, startedAt: Date())
        }
        await model.rescan(.genesis)
        #expect(world.sync.rescans == [.genesis])
        #expect(model.progressText == "Rescanning… 250 / 1000")
        #expect(model.progressFraction == 0.25)
        await model.cancelRescan()
        #expect(!model.isRescanning)
    }

    @Test func QT117_rescanWhileRescanningSaysWalletIsRescanning() async {
        m2.repair.configured.withLock { $0 = true }
        world.sync.rescanResult = .failure(ServiceError(code: .syncRescanInProgress))
        let model = repair()
        await model.rescan(.walletBirth)
        #expect(model.state == .failed("Rescan unavailable: Wallet is currently rescanning. Abort existing rescan or wait."))
    }

    @Test func QT148_resetChainDataAsksFirst() async {
        m2.repair.configured.withLock { $0 = true }
        let model = repair()
        model.requestResetChainData()
        #expect(model.state == .confirming(.resetChainData))
        model.cancelConfirmation()
        #expect(m2.repair.resets.current == 0)
        model.requestResetChainData()
        await model.confirm()
        #expect(m2.repair.resets.current == 1)
        #expect(model.state == .idle)
    }

    @Test func QT148_resetWhileSPVRunsReportsTheEngineCode() async {
        m2.repair.configured.withLock { $0 = true }
        m2.repair.errors.set(ServiceError(code: .syncSpvRunning), for: "resetChainData")
        let model = repair()
        model.requestResetChainData()
        await model.confirm()
        #expect(model.state == .failed(L10n.M2Errors.spvRunning))
    }

    @Test func IOS034_dropUnconfirmedCountsAndSchedulesRescan() async {
        m2.actions.dropCount.withLock { $0 = 2 }
        let model = repair()
        model.requestDropUnconfirmed()
        await model.confirm()
        #expect(model.state == .done("2 transactions were removed; a rescan was scheduled."))
        #expect(m2.actions.dropCalls.current.count == 1)
    }

    @Test func IOS113_birthHeightAboveTipIsRefused() async {
        m2.repair.configured.withLock { $0 = true }
        let model = repair()
        await model.setBirthHeight("abc")
        #expect(model.state == .failed(L10n.Tools.birthHeightInvalid))
        await model.setBirthHeight("2000000")
        #expect(model.state == .failed(L10n.M2Errors.heightOutOfRange))
        await model.setBirthHeight("900000")
        #expect(model.state == .idle)
        #expect(m2.repair.birthHeights.current.first?.1 == 900_000)
    }
}
