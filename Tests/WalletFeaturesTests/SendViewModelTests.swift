// Send flow (QT-052…063, IOS-041…052, DESIGN-opus §1.11, review M-7/H-4).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Send view model")
struct SendViewModelTests {
    let world = FakeWorld()

    func makeModel(page: SendPage = .regular) -> SendViewModel {
        SendViewModel(env: world.environment(), network: world.network, page: page)
    }

    /// Fills the first entry with a valid recipient.
    func fillValid(_ model: SendViewModel, amount: String = "1.5", label: String = "") {
        model.entries[0].address = testnetAddress1
        model.entries[0].amountText = amount
        model.entries[0].label = label
    }

    /// Reviews and runs the 3 s countdown to the end.
    func reviewToConfirm(_ model: SendViewModel) async {
        await model.review()
        guard case .confirm = model.phase else {
            Issue.record("expected confirm, got \(model.phase)")
            return
        }
        for _ in 0..<SendViewModel.confirmDelaySeconds {
            await eventually { world.sleeper.pendingCount == 1 }
            world.sleeper.fireNext()
        }
        await eventually { model.canConfirm }
    }

    // MARK: Validation (QT-055, QT-056, QT-067)

    @Test func QT055_invalidAddressAndAmountHighlightFields() async {
        let model = makeModel()
        model.entries[0].address = "not-an-address"
        model.entries[0].amountText = "0"
        await model.review()
        #expect(model.phase == .editing)
        #expect(model.entries[0].addressError == L10n.Send.invalidAddress)
        #expect(model.entries[0].amountError == L10n.Send.invalidAmount)
        #expect(model.entries[0].error == .sendInvalidAddress)
        #expect(world.sender.drafts.current.isEmpty)
    }

    @Test func QT055_dustAmountIsRejected() async {
        let model = makeModel()
        model.entries[0].address = testnetAddress1
        model.entries[0].amountText = "0.00000545"
        await model.review()
        #expect(model.entries[0].amountError == L10n.Send.dustAmount)
        #expect(model.entries[0].error == .sendDustAmount)
    }

    @Test func QT056_commaIsAcceptedAsDecimalMark() async {
        let model = makeModel()
        fillValid(model, amount: "1,25")
        await model.review()
        #expect(world.sender.lastDraft?.state.current.recipients.first?.amount == Amount(duffs: 125_000_000))
    }

    @Test func QT056_amountAbove21MillionIsRejected() async {
        let model = makeModel()
        fillValid(model, amount: "21000000.00000001")
        await model.review()
        #expect(model.entries[0].amountError == L10n.Send.amountTooLarge)
    }

    @Test func wrongNetworkAddressNamesTheNetwork() async {
        let model = makeModel()
        model.entries[0].address = mainnetAddress
        model.entries[0].amountText = "1"
        await model.review()
        #expect(model.entries[0].addressError == L10n.Send.networkMismatch("Testnet"))
    }

    @Test func QT067_platformAddressIsRefused() async {
        let model = makeModel()
        model.entries[0].address = "tdash1platformaddress"
        model.entries[0].amountText = "1"
        await model.review()
        #expect(model.entries[0].error == .sendPlatformAddress)
        #expect(model.entries[0].addressError == L10n.Send.platformAddress)
    }

    @Test func addressWhitespaceAndZeroWidthCharactersAreStripped() async {
        let model = makeModel()
        model.entries[0].address = " \u{200B}" + testnetAddress1 + "\n"
        model.entries[0].amountText = "1"
        await model.review()
        #expect(world.sender.lastDraft?.state.current.recipients.first?.address == testnetAddress1)
    }

    // MARK: Guards (IOS-051)

    @Test func IOS051_refusesWhileSyncing() async {
        world.sync.status = FakeSync.syncing(phase: .headers, progress: 0.5)
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.phase == .failed(SendFailure(code: .syncSpvNotRunning, message: L10n.Send.syncing)))
    }

    @Test func IOS051_refusesWithoutPeers() async {
        world.sync.status = FakeSync.synced(peers: 0)
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.phase == .failed(SendFailure(code: .sendNoPeers, message: L10n.Send.offline)))
    }

    // MARK: Duplicates (QT-060)

    /// "Yes" merges the entries that pay the same address, because the
    /// engine refuses an address twice (review H3).
    @Test func QT060_duplicateRecipientsAskFirstThenMerge() async {
        let model = makeModel()
        fillValid(model, amount: "1.5", label: "")
        model.entries[0].message = "first"
        model.addRecipient()
        model.entries[1].address = testnetAddress2
        model.entries[1].amountText = "0.5"
        model.addRecipient()
        model.entries[2].address = " " + testnetAddress1
        model.entries[2].amountText = "2,25"
        model.entries[2].subtractFee = true
        model.entries[2].label = "Shop"
        model.entries[2].message = "second"
        let firstID = model.entries[0].id
        await model.review()
        #expect(model.phase == .confirmDuplicates)
        #expect(world.sender.drafts.current.isEmpty)

        await model.acknowledgeDuplicates()
        guard case .confirm = model.phase else {
            Issue.record("expected confirm after acknowledging, got \(model.phase)")
            return
        }
        #expect(model.entries.count == 2)
        let merged = model.entries[0]
        #expect(merged.id == firstID)
        #expect(merged.address == testnetAddress1)
        #expect(merged.amountText == "3.75000000")
        #expect(merged.subtractFee)
        #expect(merged.label == "Shop")
        #expect(merged.message == "first\nsecond")
        #expect(model.entries[1].address == testnetAddress2 && model.entries[1].amountText == "0.5")
        let sent = world.sender.lastDraft?.state.current.recipients
        #expect(sent?.map(\.address) == [testnetAddress1, testnetAddress2])
        #expect(sent?.map(\.amount.duffs) == [375_000_000, 50_000_000])
        #expect(sent?.first?.subtractFeeFromAmount == true)
        // The merge is the view model's own edit: nothing was abandoned.
        #expect(world.sender.lastDraft?.state.current.abandoned.isEmpty == true)
    }

    @Test func QT060_mergeKeepsTheFirstLabel() async {
        let model = makeModel()
        fillValid(model, amount: "1", label: "Alice")
        model.addRecipient()
        model.entries[1].address = testnetAddress1
        model.entries[1].amountText = "1"
        model.entries[1].label = "Bob"
        await model.review()
        await model.acknowledgeDuplicates()
        #expect(model.entries.map(\.label) == ["Alice"])
        #expect(model.entries[0].amountText == "2.00000000")
        #expect(!model.entries[0].subtractFee)
    }

    @Test func QT060_mergedAmountAbove21MillionIsAnAmountError() async {
        let model = makeModel()
        fillValid(model, amount: "20000000")
        model.addRecipient()
        model.entries[1].address = testnetAddress1
        model.entries[1].amountText = "2000000"
        await model.review()
        await model.acknowledgeDuplicates()
        #expect(model.phase == .editing)
        #expect(model.entries.count == 1)
        #expect(model.entries[0].amountError == L10n.Send.amountTooLarge)
        #expect(world.sender.drafts.current.isEmpty)
    }

    @Test func QT060_noOnTheQuestionKeepsTheEntries() async {
        let model = makeModel()
        fillValid(model)
        model.addRecipient()
        model.entries[1].address = testnetAddress1
        model.entries[1].amountText = "2"
        await model.review()
        await model.cancel()
        #expect(model.phase == .editing)
        #expect(model.entries.count == 2)
    }

    /// The fake refuses duplicates like the engine's `validate_recipients`,
    /// so a view model that sent them would fail here as it does live.
    @Test func H3_fakeDraftRefusesDuplicatesLikeTheEngine() async throws {
        let draft = try await world.sender.makeDraft(wallet: walletA)
        do {
            try await draft.setRecipients([
                PaymentRecipient(address: testnetAddress1, amount: Amount(duffs: 1000)),
                PaymentRecipient(address: testnetAddress2, amount: Amount(duffs: 1000)),
                PaymentRecipient(address: testnetAddress1, amount: Amount(duffs: 2000)),
            ])
            Issue.record("duplicates must be refused")
        } catch {
            #expect(error.code == .sendDuplicateAddress)
            #expect(error.recipientIndex == 2)
            #expect(error.parameters["index"] == 2)
        }
        #expect(world.sender.lastDraft?.state.current.recipients.isEmpty == true)
    }

    // MARK: Happy path (QT-059, QT-063, IOS rule 4)

    @Test func QT059_confirmWaitsThreeSecondsBeforeBroadcasting() async {
        let model = makeModel()
        fillValid(model)
        await model.review()
        guard case .confirm(let summary) = model.phase else {
            Issue.record("expected confirm, got \(model.phase)")
            return
        }
        #expect(summary.totalSent == Amount(duffs: 150_000_000))
        #expect(model.confirmCountdown == 3)
        #expect(!model.canConfirm)
        #expect(model.sendButtonTitle == "Send (3)")
        // Confirm is refused during the countdown: nothing is broadcast.
        await model.confirm()
        #expect(world.sender.lastDraft?.state.current.broadcasts.isEmpty == true)
        for remaining in [2, 1, 0] {
            await eventually { world.sleeper.pendingCount == 1 }
            world.sleeper.fireNext()
            await eventually { model.confirmCountdown == remaining }
        }
        #expect(world.sleeper.requested.current.allSatisfy { $0 == .seconds(1) })
        #expect(model.canConfirm)
        #expect(model.sendButtonTitle == L10n.Send.send)
    }

    @Test func IOSRule4_prepareNeverBroadcasts() async {
        let model = makeModel()
        fillValid(model)
        await model.review()
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.prepareGrants.count == 1)
        #expect(draft.broadcasts.isEmpty)
    }

    @Test func QT063_broadcastClearsFormAndRoutesToTheTransaction() async {
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        let txid = String(repeating: "f", count: 64)
        #expect(model.phase == .done(txid: txid))
        #expect(model.route == .transaction(txid: txid))
        #expect(model.entries.count == 1 && model.entries[0].isBlank)
        #expect(world.sender.lastDraft?.state.current.broadcasts.count == 1)
        #expect(world.sender.lastDraft?.state.current.abandoned.isEmpty == true)
    }

    /// The engine's broadcast records every recipient, labelled or not, in
    /// the address book (m1-engine.md §2.7.1, review M7/L3): the view model
    /// hands the recipients to the draft and writes no entries itself.
    @Test(arguments: ["", "Shop"])
    func L3_addressBookEntriesAreLeftToTheEngine(label: String) async {
        let model = makeModel()
        fillValid(model, label: label)
        await reviewToConfirm(model)
        await model.confirm()
        let recipient = world.sender.lastDraft?.state.current.recipients.first
        #expect(recipient?.address == testnetAddress1)
        #expect((recipient?.label ?? "") == label)
        #expect(world.addressBook.saves.current.isEmpty)
    }

    @Test func QT059_confirmLinesFollowDashQt() async {
        let model = makeModel()
        fillValid(model, label: "Shop")
        await model.review()
        let lines = model.confirmLines
        #expect(lines.first == L10n.Send.confirmQuestion)
        #expect(lines.contains("1.50000000 tDASH to 'Shop' (\(testnetAddress1))"))
        #expect(lines.contains(L10n.Send.usingAnyFunds))
        #expect(lines.contains("Transaction fee: 0.00000226 tDASH"))
        #expect(lines.contains("Transaction size: 0.226 kB, fee rate: 0.00001000 tDASH/kB"))
        #expect(lines.last == "Total Amount: 1.50000226 tDASH")
    }

    @Test func QT059_confirmListsAtMostTenRecipients() async {
        let model = makeModel()
        model.entries = (0..<12).map { i in
            RecipientEntry(address: "yRecipientAddress0000000000000\(String(format: "%04d", i))", amountText: "1")
        }
        await model.review()
        #expect(model.confirmLines.contains(L10n.Send.entriesDisplayed(10, of: 12)))
    }

    // MARK: Authorization (QT-061, H-4)

    /// The engine caps what leaves the wallet, fee included, at the grant's
    /// `max_duffs`, so the cap is the sum of the amounts plus the estimated
    /// fee, without other slack (fix-review L4: the quick-unlock limit means
    /// the same in Send and PSBT).
    @Test func IOS016_spendGrantCapsTheAmountsPlusTheFee() async {
        let model = makeModel()
        fillValid(model, amount: "1")
        model.addRecipient()
        model.entries[1].address = testnetAddress2
        model.entries[1].amountText = "2"
        model.entries[1].subtractFee = true
        await model.review()
        let call = world.auth.authorizeCalls.last
        #expect(call?.purpose == .spend(max: Amount(duffs: 300_000_226)))
        #expect(call?.passphrase == nil)
    }

    @Test func QT061_encryptedWalletAsksForThePassphrase() async {
        world.auth.lockState = .locked
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.phase == .authorizing)
        #expect(world.auth.authorizeCalls.isEmpty)
        await model.authorize(passphrase: "secret")
        #expect(world.auth.authorizeCalls.last?.passphrase == "secret")
        guard case .confirm = model.phase else {
            Issue.record("expected confirm, got \(model.phase)")
            return
        }
        #expect(world.sender.lastDraft?.state.current.prepareGrants.first?.id == "grant-1")
    }

    /// A second passphrase submission while the first is being checked is
    /// ignored: one grant, one prepare.
    @Test func QT061_secondAuthorizeWhileCheckingIsIgnored() async {
        world.auth.lockState = .locked
        let gate = Gate()
        world.auth.authorizeGate = gate
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.phase == .authorizing)
        let first = Task { await model.authorize(passphrase: "secret") }
        await eventually { gate.waiterCount == 1 }
        #expect(model.phase == .preparing)
        await model.authorize(passphrase: "secret")
        gate.open()
        await first.value
        #expect(world.auth.authorizeCalls.count == 1)
        #expect(world.sender.lastDraft?.state.current.prepareGrants.count == 1)
        guard case .confirm = model.phase else {
            Issue.record("expected confirm, got \(model.phase)")
            return
        }
    }

    @Test func QT061_wrongPassphraseFails() async {
        world.auth.lockState = .locked
        world.auth.authorizeErrors = [ServiceError(code: .vaultWrongPassphrase)]
        let model = makeModel()
        fillValid(model)
        await model.review()
        await model.authorize(passphrase: "bad")
        #expect(model.phase == .failed(SendFailure(code: .vaultWrongPassphrase, message: L10n.Common.wrongPassphrase)))
    }

    // MARK: Cancel and edits (review M-7)

    @Test func M7_cancelInConfirmAbandonsThePreparedTransaction() async {
        let model = makeModel()
        fillValid(model)
        await model.review()
        await model.cancel()
        #expect(model.phase == .editing)
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.abandoned.count == 1)
        #expect(draft.broadcasts.isEmpty)
        #expect(model.entries[0].address == testnetAddress1)  // the form is kept
    }

    @Test func M7_editingInConfirmAbandonsAndReturnsToEditing() async {
        let model = makeModel()
        fillValid(model)
        await model.review()
        model.entries[0].amountText = "2"
        #expect(model.phase == .editing)
        #expect(model.confirmCountdown == 0)
        await eventually { world.sender.lastDraft?.state.current.abandoned.count == 1 }
        await model.confirm()
        #expect(world.sender.lastDraft?.state.current.broadcasts.isEmpty == true)
    }

    @Test func M7_feeChangeInConfirmAbandons() async {
        let model = makeModel()
        fillValid(model)
        await model.review()
        model.setFee(.recommended(targetBlocks: 2))
        #expect(model.phase == .editing)
        await eventually { world.sender.lastDraft?.state.current.abandoned.count == 1 }
    }

    @Test func M7_validationMessagesDoNotCountAsEdits() async {
        let model = makeModel()
        model.entries[0].address = testnetAddress1
        model.entries[0].amountText = "abc"
        await model.review()
        #expect(model.entries[0].amountError == L10n.Send.unparsableAmount)
        #expect(model.phase == .editing)
    }

    @Test func M7_prepareFinishingAfterAnEditIsAbandoned() async {
        let gate = Gate()
        world.sender.configure.withLock { $0 = { draft in draft.prepareGate.withLock { $0 = gate } } }
        let model = makeModel()
        fillValid(model)
        let review = Task { await model.review() }
        await eventually { gate.waiterCount == 1 }
        #expect(model.phase == .preparing)
        model.entries[0].label = "changed"
        #expect(model.phase == .editing)
        #expect(world.auth.revoked.map(\.id) == ["grant-1"])
        gate.open()
        await review.value
        #expect(model.phase == .editing)
        #expect(world.sender.lastDraft?.state.current.abandoned.count == 1)
    }

    @Test func M7_failedPrepareRevokesTheUnusedGrant() async {
        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.prepareError = ServiceError(code: .sendAbsurdFee) } }
        }
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.phase == .failed(SendFailure(code: .sendAbsurdFee, message: L10n.Send.absurdFee("0.10000000 tDASH"))))
        #expect(world.auth.revoked.map(\.id) == ["grant-1"])
    }

    @Test func M7_cancelWhileAuthorizingIssuesNoGrant() async {
        world.auth.lockState = .locked
        let model = makeModel()
        fillValid(model)
        await model.review()
        await model.cancel()
        #expect(model.phase == .editing)
        await model.authorize(passphrase: "late")
        #expect(world.auth.authorizeCalls.isEmpty)
    }

    @Test func M7_unknownBroadcastOutcomeIsNeverAbandoned() async {
        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.broadcastError = ServiceError(code: .internal, detail: "timeout") } }
        }
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        let txid = String(repeating: "f", count: 64)
        guard case .broadcastUnknown(let unknownTxid, _) = model.phase else {
            Issue.record("expected broadcastUnknown, got \(model.phase)")
            return
        }
        #expect(unknownTxid == txid)
        #expect(model.route == .transaction(txid: txid))
        #expect(!model.isEditable)
        #expect(model.canBroadcastAgain)
        await model.cancel()
        model.addRecipient()
        #expect(model.entries.count == 1)
        await model.dismiss()
        #expect(model.phase == .editing)
        #expect(model.entries[0].isBlank)
        #expect(!model.canBroadcastAgain)
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.abandoned.isEmpty)
        #expect(draft.released.isEmpty)
    }

    /// After an unknown outcome the prepared transaction is kept and can be
    /// sent again, same txid, without a new grant (review M3).
    @Test func M3_unknownOutcomeCanBeBroadcastAgain() async {
        world.sender.configure.withLock {
            $0 = { draft in
                draft.state.withLock { $0.broadcastErrors = [ServiceError(code: .sendBroadcastUnknown, detail: "no verdict")] }
            }
        }
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        guard case .broadcastUnknown = model.phase else {
            Issue.record("expected broadcastUnknown, got \(model.phase)")
            return
        }
        await model.broadcastAgain()
        let txid = String(repeating: "f", count: 64)
        #expect(model.phase == .done(txid: txid))
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.broadcasts.count == 2)
        #expect(draft.broadcasts.allSatisfy { $0.summary.txid == txid })
        #expect(draft.prepareGrants.count == 1)
        #expect(world.auth.authorizeCalls.count == 1)
        #expect(model.entries[0].isBlank)
    }

    /// A failed second attempt does not settle the first one: the phase
    /// stays `.broadcastUnknown` and the same transaction can be broadcast
    /// again. The engine never releases it; a repeat the network refused is
    /// reported as `send.broadcast_unknown`.
    @Test func M3_failedSecondAttemptKeepsTheOutcomeUnknown() async {
        world.sender.configure.withLock {
            $0 = { draft in
                draft.state.withLock {
                    $0.broadcastErrors = [
                        ServiceError(code: .sendBroadcastUnknown), ServiceError(code: .networkNotOpen),
                        ServiceError(code: .sendNoPeers),
                    ]
                }
            }
        }
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        await model.broadcastAgain()
        guard case .broadcastUnknown(_, let failure) = model.phase else {
            Issue.record("expected broadcastUnknown, got \(model.phase)")
            return
        }
        #expect(failure.code == .networkNotOpen)
        #expect(model.canBroadcastAgain)
        await model.broadcastAgain()
        guard case .broadcastUnknown(_, let second) = model.phase else {
            Issue.record("expected broadcastUnknown, got \(model.phase)")
            return
        }
        #expect(second.code == .sendBroadcastUnknown)
        #expect(model.canBroadcastAgain)
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.broadcasts.count == 3)
        #expect(draft.released.isEmpty)
        await model.broadcastAgain()
        #expect(model.phase == .done(txid: String(repeating: "f", count: 64)))
    }

    @Test func M7_rejectedBroadcastIsReleasedByTheEngine() async {
        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.broadcastError = ServiceError(code: .sendBroadcastRejected) } }
        }
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        #expect(model.phase == .failed(SendFailure(code: .sendBroadcastRejected, message: L10n.Send.broadcastRejected)))
        #expect(!model.canBroadcastAgain)
        await model.dismiss()
        #expect(model.phase == .editing)
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.released.count == 1)
        #expect(draft.abandoned.isEmpty)
    }

    /// Errors the engine raises before it hands a first broadcast to the
    /// network leave the transaction pending (m1-engine.md §2.7.1 "not
    /// dispatched"): a definite failure, abandoned on dismiss, never
    /// "outcome unknown" (review L5).
    @Test(arguments: [ServiceErrorCode.wallet, .storage, .spv, .io, .walletNotFound, .invalidArgument])
    func L5_preDispatchErrorIsADefiniteFailure(code: ServiceErrorCode) async {
        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.broadcastErrors = [ServiceError(code: code)] } }
        }
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        guard case .failed(let failure) = model.phase else {
            Issue.record("expected failed, got \(model.phase)")
            return
        }
        #expect(failure.code == code)
        #expect(model.route == nil)
        #expect(!model.canBroadcastAgain)
        await model.dismiss()
        #expect(model.phase == .editing)
        let draft = world.sender.lastDraft!.state.current
        #expect(draft.abandoned.count == 1)
        #expect(draft.released.count == 1)
    }

    /// While a broadcast is in flight or its outcome is unknown the form is
    /// read-only: edits through `entries` are undone and the editing methods
    /// do nothing, so "Broadcast again" sends what was reviewed (review L6).
    @Test func L6_formIsReadOnlyWhileBroadcastingAndWhileTheOutcomeIsUnknown() async {
        let gate = Gate()
        world.sender.configure.withLock {
            $0 = { draft in
                draft.broadcastGate.withLock { $0 = gate }
                draft.state.withLock { $0.broadcastErrors = [ServiceError(code: .sendBroadcastUnknown)] }
            }
        }
        let model = makeModel()
        fillValid(model, label: "Shop")
        await reviewToConfirm(model)
        let reviewed = model.entries
        let broadcast = Task { await model.confirm() }
        await eventually { gate.waiterCount == 1 }
        #expect(model.phase == .broadcasting)
        #expect(!model.isEditable)
        model.entries[0].address = testnetAddress2
        model.entries[0].label = "Other"
        model.addRecipient()
        #expect(model.entries == reviewed)
        gate.open()
        await broadcast.value
        guard case .broadcastUnknown = model.phase else {
            Issue.record("expected broadcastUnknown, got \(model.phase)")
            return
        }
        #expect(!model.isEditable)
        model.entries[0].amountText = "2"
        model.paste(testnetAddress2)
        model.clearAll()
        #expect(model.entries == reviewed)
        // The gate is open now: the repeat goes through at once.
        await model.broadcastAgain()
        #expect(model.phase == .done(txid: String(repeating: "f", count: 64)))
        #expect(model.isEditable)
    }

    @Test func M7_sessionErrorOnBroadcastIsAbandonedOnDismiss() async {
        world.sender.configure.withLock {
            $0 = { draft in draft.state.withLock { $0.broadcastErrors = [ServiceError(code: .networkNotOpen)] } }
        }
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        await model.confirm()
        guard case .failed(let failure) = model.phase else {
            Issue.record("expected failed, got \(model.phase)")
            return
        }
        #expect(failure.code == .networkNotOpen)
        await model.dismiss()
        #expect(world.sender.lastDraft?.state.current.abandoned.count == 1)
    }

    /// `send.no_peers` releases the inputs in the engine and spends the
    /// prepared transaction, so the user goes back to the form and Review
    /// runs a new grant and prepare (review M1).
    @Test func M1_noPeersReturnsToReviewWithANewGrantAndPrepare() async {
        let model = makeModel()
        fillValid(model)
        await reviewToConfirm(model)
        world.sender.lastDraft?.state.withLock { $0.broadcastErrors = [ServiceError(code: .sendNoPeers)] }
        await model.confirm()
        #expect(model.phase == .failed(SendFailure(code: .sendNoPeers, message: L10n.Send.noPeers)))
        let first = world.sender.lastDraft!
        #expect(first.state.current.released.count == 1)
        await model.dismiss()
        #expect(model.phase == .editing)
        #expect(model.entries[0].address == testnetAddress1)
        #expect(first.state.current.abandoned.isEmpty)

        await reviewToConfirm(model)
        await model.confirm()
        #expect(model.phase == .done(txid: String(repeating: "f", count: 64)))
        #expect(world.sender.drafts.current.count == 2)
        #expect(world.auth.authorizeCalls.count == 2)
        #expect(first.state.current.broadcasts.count == 1)
        #expect(world.sender.lastDraft?.state.current.prepareGrants.map(\.id) == ["grant-2"])
    }

    // MARK: Engine errors (QT-062)

    @Test func QT062_recipientErrorsGoBackToTheEntry() async {
        world.sender.configure.withLock {
            $0 = { draft in
                draft.state.withLock {
                    $0.estimateError = ServiceError(code: .sendAmountExceedsBalance, recipientIndex: 0)
                }
            }
        }
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.phase == .editing)
        #expect(model.entries[0].error == .sendAmountExceedsBalance)
        #expect(model.entries[0].amountError == L10n.Send.amountExceedsBalance)
    }

    /// The engine reports the fee with the error; `estimate()` failed, so
    /// there is no estimate to read it from (review M6).
    @Test func M6_amountWithFeeExceedsBalanceShowsTheEngineFee() async {
        world.sender.configure.withLock {
            $0 = { draft in
                draft.state.withLock {
                    $0.estimateError = ServiceError(
                        code: .sendAmountWithFeeExceedsBalance, parameters: ["fee": 1234, "available": 150_000_000])
                }
            }
        }
        let model = makeModel()
        fillValid(model)
        await model.review()
        #expect(model.estimate == nil)
        #expect(model.phase == .failed(SendFailure(
            code: .sendAmountWithFeeExceedsBalance,
            message: L10n.Send.amountWithFeeExceedsBalance("0.00001234 tDASH"))))
    }

    @Test func amountTooSmallAfterFeeMarksTheAmount() async {
        world.sender.configure.withLock {
            $0 = { draft in
                draft.state.withLock {
                    $0.estimateError = ServiceError(
                        code: .sendAmountTooSmallAfterFee, recipientIndex: 0, parameters: ["index": 0])
                }
            }
        }
        let model = makeModel()
        fillValid(model, amount: "0.00001")
        model.entries[0].subtractFee = true
        await model.review()
        #expect(model.phase == .editing)
        #expect(model.entries[0].error == .sendAmountTooSmallAfterFee)
        #expect(model.entries[0].amountError == L10n.Send.amountTooSmallAfterFee)
        #expect(model.entries[0].addressError == nil)
    }

    @Test func QT062_insufficientMixedFundsText() async {
        world.sender.configure.withLock {
            $0 = { draft in
                draft.state.withLock {
                    $0.estimateError = ServiceError(code: .init(rawValue: "send.insufficient_mixed_funds"))
                }
            }
        }
        let model = makeModel(page: .coinJoin)
        fillValid(model)
        await model.review()
        #expect(model.phase == .failed(SendFailure(
            code: .init(rawValue: "send.insufficient_mixed_funds"), message: L10n.Send.insufficientMixedFunds)))
    }

    @Test func unknownSendCodesShowCreationFailed() {
        let model = makeModel()
        #expect(model.failure(for: ServiceError(code: .init(rawValue: "send.something_new"))).message == L10n.Send.creationFailed)
        #expect(model.failure(for: ServiceError(code: .notImplemented)).message == L10n.Common.notAvailableYet)
    }

    // MARK: Entries, paste, max, fee, source (QT-051…054, QT-057)

    @Test func QT052_addRemoveAndClearAll() throws {
        let model = makeModel()
        model.addRecipient()
        #expect(model.entries.count == 2)
        model.removeRecipient(model.entries[0].id)
        #expect(model.entries.count == 1)
        model.removeRecipient(model.entries[0].id)
        #expect(model.entries.count == 1 && model.entries[0].isBlank)
        fillValid(model)
        try model.setSource(.outpoints([OutPoint(txid: txid(1), vout: 0)]))
        model.clearAll()
        #expect(model.entries[0].isBlank)
        #expect(model.source == .any)
    }

    @Test func QT054_pastingAURIFillsTheEntry() {
        let model = makeModel()
        model.paste("dash:\(testnetAddress1)?amount=0.25&label=Cafe&message=Thanks")
        let entry = model.entries[0]
        #expect(entry.address == testnetAddress1)
        #expect(entry.amountText == "0.25000000")
        #expect(entry.label == "Cafe")
        #expect(entry.message == "Thanks")
    }

    @Test func QT054_invalidURIMarksTheAddress() {
        let model = makeModel()
        model.paste("dash:nonsense")
        #expect(model.entries[0].addressError == L10n.Send.invalidAddress)
    }

    @Test func QT054_pasteGoesToTheFirstBlankEntryOrANewOne() {
        let model = makeModel()
        fillValid(model)
        model.paste(testnetAddress2)
        #expect(model.entries.count == 2)
        #expect(model.entries[1].address == testnetAddress2)
    }

    @Test func QT053_useAvailableBalanceSubtractsOtherEntries() async {
        world.sender.maxSpendableValue.withLock { $0 = .success(Amount(duffs: 500_000_000)) }
        let model = makeModel()
        fillValid(model, amount: "1")
        model.addRecipient()
        model.entries[1].address = testnetAddress2
        await model.useMax(for: model.entries[1].id)
        #expect(model.entries[1].amountText == "4.00000000")
        #expect(model.entries[1].subtractFee)
    }

    @Test func QT057_customFeeIsRaisedToTheMinimum() {
        let model = makeModel()
        model.setFee(.perKilobyte(Amount(duffs: 10)))
        #expect(model.fee == .perKilobyte(SendViewModel.minimumFeePerKilobyte))
        #expect(model.customFeeWarning)
        model.setFee(.recommended(targetBlocks: 2))
        #expect(!model.customFeeWarning)
        #expect(ConfirmationTarget.all.map(\.blocks) == [2, 4, 6, 12, 24, 48, 144, 504, 1008])
    }

    @Test func QT051_coinJoinPageSpendsFullyMixedCoinsOnly() async {
        let model = makeModel(page: .coinJoin)
        #expect(model.source == .fullyMixed)
        #expect(throws: SendSourceRefusal.coinJoinPage) { try model.setSource(.any) }
        #expect(model.source == .fullyMixed)
        #expect(model.sendButtonTitle == L10n.Send.sendMixedFunds)
        fillValid(model)
        await model.review()
        #expect(world.sender.lastDraft?.state.current.source == .fullyMixed)
        #expect(model.confirmLines.contains(L10n.Send.usingCoinJoinFunds))
        #expect(model.confirmLines.contains(L10n.Send.inputCount(1)))
    }
}
