import DashKit
import Foundation
import Testing
@testable import WalletRuntime

private typealias Grant = WalletRuntime.AuthGrant

private func grant(_ purpose: WalletRuntime.GrantPurpose, id: String = "g1") -> Grant {
    Grant(id: id, purpose: purpose, expiresAt: Date().addingTimeInterval(60), singleUse: true)
}

/// A one-shot latch a fake engine call can wait on.
private final class Latch: @unchecked Sendable {
    private let lock = NSLock()
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            let resume = lock.withLock {
                if opened { return true }
                waiters.append(c)
                return false
            }
            if resume { c.resume() }
        }
    }

    func open() {
        let all = lock.withLock {
            opened = true
            defer { waiters = [] }
            return waiters
        }
        all.forEach { $0.resume() }
    }
}

@MainActor
@Suite struct AuthenticationGateTests {
    @Test func lockStateLoadsAtSessionStartAndFollowsVaultCalls() async throws {
        let h = Harness()
        try await h.start()
        let auth = h.services.auth
        #expect(auth.lockState == .noVault)

        let changes = auth.lockStateChanges()
        _ = try await h.services.vault.create(passphrase: h.services.vault.makeSecret(utf8: "correct"))
        // The vault service forwards the returned status before returning.
        #expect(auth.lockState == .unlocked)
        var iterator = changes.makeAsyncIterator()
        #expect(await iterator.next() == .unlocked)

        try await auth.lock()
        #expect(auth.lockState == .locked)
    }

    @Test func requirementFollowsLockStateAndThePaymentSetting() async throws {
        let h = Harness {
            $0.with { $0.vault = Fixtures.vault(.unlocked) }
        }
        try await h.start()
        let auth = h.services.auth
        let spend = WalletRuntime.GrantPurpose.spend(max: WalletRuntime.Amount(duffs: 1))
        #expect(auth.requirement(for: spend) == .passphrase)
        try h.services.settings.setRequireAuthenticationForEveryPayment(false)
        #expect(auth.requirement(for: spend) == .none)
        #expect(auth.requirement(for: .signMessage) == .none)
        // The vault refuses these without the passphrase whenever it is
        // encrypted, even while unlocked (dw-vault `Vault::authorize`).
        #expect(auth.requirement(for: .revealSecret) == .passphrase)
        #expect(auth.requirement(for: .wipe) == .passphrase)
        #expect(auth.requirement(for: .changeCredential) == .passphrase)

        try await auth.lock()
        #expect(auth.requirement(for: spend) == .passphrase)
        try await auth.unlock(passphrase: h.services.vault.makeSecret(utf8: "correct"), scope: .mixingOnly)
        #expect(auth.requirement(for: .revealSecret) == .passphrase)
        #expect(auth.requirement(for: spend) == .passphrase)

        // Unencrypted (no passphrase slot): nothing to ask for.
        let unencrypted = Harness {
            $0.with { $0.vault = Fixtures.vault(.unencrypted, encrypted: false) }
        }
        try await unencrypted.start()
        #expect(unencrypted.services.auth.requirement(for: .revealSecret) == .none)
        #expect(unencrypted.services.auth.requirement(for: .changeCredential) == .none)
        #expect(unencrypted.services.auth.requirement(for: .wipe) == .none)
    }

    @Test func unlockReportsAttemptsAndUpdatesState() async throws {
        let h = Harness {
            $0.with { $0.vault = Fixtures.vault(.locked) }
        }
        try await h.start()
        let auth = h.services.auth
        let vault = h.services.vault
        do {
            try await auth.unlock(passphrase: vault.makeSecret(utf8: "wrong"), scope: .full)
            Issue.record("wrong passphrase must fail")
        } catch {
            #expect(error.code == .vaultWrongPassphrase)
            #expect(error.parameters["failed_attempts"] == 1)
        }
        #expect(auth.lockState == .locked)
        try await auth.unlock(passphrase: vault.makeSecret(utf8: "correct"), scope: .mixingOnly)
        #expect(auth.lockState == .unlockedMixingOnly)
    }

    @Test func engineLockEventsRefreshTheState() async throws {
        let h = Harness {
            $0.with { $0.vault = Fixtures.vault(.unlocked) }
        }
        try await h.start()
        h.engine.with { $0.vault = Fixtures.vault(.locked) }
        h.engine.events.publish(.lockStateChanged(.regtest))
        #expect(await eventually { h.services.auth.lockState == .locked })
    }

    @Test func authorizeIssuesTheEngineGrant() async throws {
        let h = Harness {
            $0.with { $0.vault = Fixtures.vault(.unlocked) }
        }
        try await h.start()
        let issued = try await h.services.auth.authorize(.signMessage, credential: .unencrypted)
        #expect(issued.purpose == .signMessage)
        #expect(h.engine.with { $0.issuedGrants } == [issued.id])
    }

    @Test func watchdogTimesOutAndRevokesTheLateGrant() async throws {
        let latch = Latch()
        let h = Harness(authorizeTimeout: .seconds(30)) {
            $0.with { $0.vault = Fixtures.vault(.unlocked) }
            $0.authorizeHandler = { purpose in
                await latch.wait()
                return .success(DashKit.AuthGrant(id: "late", purpose: purpose, expiresAt: Date(), singleUse: true))
            }
        }
        try await h.start()
        let auth = h.services.auth
        let attempt = Task { @MainActor () -> ServiceErrorCode? in
            do throws(ServiceError) {
                _ = try await auth.authorize(.revealSecret, credential: .unencrypted)
                return nil
            } catch {
                return error.code
            }
        }
        #expect(await eventuallyAsync { h.clock.sleeperCount > 0 })
        h.clock.advance(by: .seconds(30))
        #expect(await attempt.value == .authTimedOut)
        latch.open()
        #expect(await eventuallyAsync { h.engine.with { $0.revokedGrants } == ["late"] })
    }

    @Test func callsNeedAnOpenNetwork() async throws {
        let h = Harness()
        do {
            _ = try await h.services.auth.authorize(.signMessage, credential: .unencrypted)
            Issue.record("authorize without a session must fail")
        } catch {
            #expect(error.code == .networkNotOpen)
        }
    }
}

@MainActor
@Suite struct VaultServiceTests {
    @Test func grantPurposesAreCheckedBeforeTheEngine() async throws {
        let h = Harness()
        try await h.start()
        let vault = h.services.vault
        let wallet = WalletRuntime.WalletID(Fixtures.walletA)
        do {
            _ = try await vault.revealMnemonic(wallet: wallet, grant: grant(.signMessage))
            Issue.record("reveal with a signMessage grant must fail")
        } catch {
            #expect(error.code == .vaultGrantPurposeMismatch)
        }
        #expect(!h.engine.calls.contains("revealMnemonic"))
        let revealed = try await vault.revealMnemonic(wallet: wallet, grant: grant(.revealSecret))
        #expect(revealed.phrase.count == 6)
    }

    @Test func wordCountsOutsideUInt8AreTypedErrors() async throws {
        let h = Harness()
        do {
            _ = try await h.services.vault.generateMnemonic(wordCount: 300, language: .english)
            Issue.record("300 words must fail")
        } catch {
            #expect(error.code == .walletUnsupportedWordCount)
        }
        do {
            _ = try await h.services.vault.generateMnemonic(wordCount: -1, language: .english)
            Issue.record("-1 words must fail")
        } catch {
            #expect(error.code == .walletUnsupportedWordCount)
        }
    }

    @Test func secretsAreZeroingBuffers() {
        let h = Harness()
        let secret = h.services.vault.makeSecret(utf8: "pass phrase")
        #expect(secret is DashKit.SecretBytes)
        #expect(secret.count == 11)
        #expect(!String(describing: secret).contains("pass"))
    }
}

@MainActor
@Suite struct TransactionSenderTests {
    private func draft(_ h: Harness) async throws -> (any TransactionDrafting, FakeTxDraft) {
        try await h.start()
        let fake = FakeTxDraft(walletID: Fixtures.walletA)
        h.engine.with { $0.nextDraft = fake }
        let draft = try await h.services.sender.makeDraft(wallet: WalletRuntime.WalletID(Fixtures.walletA))
        try await draft.setRecipients([
            PaymentRecipient(address: "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n", amount: WalletRuntime.Amount(duffs: 1000))
        ])
        return (draft, fake)
    }

    @Test func prepareNeedsASpendGrantAndNeverBroadcasts() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        do {
            _ = try await draft.prepare(grant: grant(.signMessage))
            Issue.record("prepare with a signMessage grant must fail")
        } catch {
            #expect(error.code == .vaultGrantPurposeMismatch)
        }
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 1226))))
        #expect(prepared.summary.totalDebit.duffs == 1226)
        #expect(!fake.calls.contains { $0.hasPrefix("broadcast") })

        let result = try await draft.broadcast(prepared)
        #expect(result.txid == prepared.summary.txid)
        // Forgotten after a successful broadcast.
        try await draft.abandon(prepared)
        #expect(fake.with { $0.abandoned }.isEmpty)
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("a second broadcast must fail")
        } catch {
            #expect(error.code == .sendPreparedTxUnknown)
        }
    }

    @Test func uncertainBroadcastKeepsInputsReserved() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000))))
        fake.with { $0.broadcastErrors = [.domain(code: "send.broadcast_unknown", detail: "timeout")] }
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("broadcast must fail")
        } catch {
            #expect(error.code == .sendBroadcastUnknown)
        }
        do {
            try await draft.abandon(prepared)
            Issue.record("abandon after an uncertain broadcast must refuse")
        } catch {
            #expect(error.code == .sendBroadcastOutcomeUnknown)
        }
        // An edit does not release it either.
        try await draft.setFee(.recommended(targetBlocks: 2))
        #expect(fake.with { $0.abandoned }.isEmpty)
        // Retrying the same signed transaction is allowed.
        let result = try await draft.broadcast(prepared)
        #expect(result.txid == prepared.summary.txid)
    }

    /// The engine releases the inputs on `send.no_peers` and spends its
    /// `PreparedTx` (review M1): the adapter forgets it, so neither a retry
    /// nor an abandon reaches the engine; sending needs a new prepare.
    @Test func noPeersReleasesTheTransaction() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000))))
        fake.with { $0.broadcastErrors = [.domain(code: "send.no_peers", detail: "")] }
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("broadcast must fail")
        } catch {
            #expect(error.code == .sendNoPeers)
        }
        #expect(fake.phase(of: prepared.summary.txid) == .released)
        #expect(fake.with { $0.released } == [prepared.summary.txid])
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("a retry after send.no_peers must fail")
        } catch {
            #expect(error.code == .sendPreparedTxUnknown)
        }
        try await draft.abandon(prepared)
        #expect(fake.with { $0.abandoned }.isEmpty)
        #expect(fake.calls.filter { $0.hasPrefix("broadcast") }.count == 1)

        // A new prepare can be sent.
        let again = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000)), id: "g2"))
        let result = try await draft.broadcast(again)
        #expect(result.txid == again.summary.txid)
    }

    /// After an unknown outcome the engine still holds the transaction; a
    /// second broadcast that finds no peers releases it there, and the
    /// adapter forgets it too.
    @Test func noPeersAfterAnUnknownOutcomeForgetsTheTransaction() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000))))
        fake.with {
            $0.broadcastErrors = [
                .domain(code: "send.broadcast_unknown", detail: "timeout"), .domain(code: "send.no_peers", detail: ""),
            ]
        }
        await #expect(throws: ServiceError.self) { _ = try await draft.broadcast(prepared) }
        #expect(fake.phase(of: prepared.summary.txid) == .unknown)
        await #expect(throws: ServiceError.self) { _ = try await draft.broadcast(prepared) }
        #expect(fake.phase(of: prepared.summary.txid) == .released)
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("the forgotten transaction must not be broadcast")
        } catch {
            #expect(error.code == .sendPreparedTxUnknown)
        }
    }

    @Test func sessionErrorsKeepTheTransactionPending() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000))))
        fake.with { $0.broadcastErrors = [.networkNotOpen(detail: "closed")] }
        await #expect(throws: ServiceError.self) { _ = try await draft.broadcast(prepared) }
        #expect(fake.phase(of: prepared.summary.txid) == .pending)
        try await draft.abandon(prepared)
        #expect(fake.with { $0.abandoned } == [prepared.summary.txid])
        // Idempotent.
        try await draft.abandon(prepared)
        #expect(fake.with { $0.abandoned }.count == 1)
    }

    @Test func duplicateRecipientsAreRefusedWithTheirIndex() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let address = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n"
        do {
            try await draft.setRecipients([
                PaymentRecipient(address: address, amount: WalletRuntime.Amount(duffs: 1000)),
                PaymentRecipient(address: address, amount: WalletRuntime.Amount(duffs: 2000)),
            ])
            Issue.record("duplicates must be refused")
        } catch {
            #expect(error.code == .sendDuplicateAddress)
            #expect(error.recipientIndex == 1)
        }
        // The earlier recipients stay.
        #expect(fake.with { $0.recipients.map(\.amount.duffs) } == [1000])
    }

    @Test func inFlightBroadcastIsNeverAbandoned() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000))))
        let latch = Latch()
        fake.with { $0.broadcastHold = { await latch.wait() } }
        let sending = Task { try await draft.broadcast(prepared) }
        #expect(await eventuallyAsync { fake.calls.contains { $0.hasPrefix("broadcast") } })

        // An edit and an explicit abandon while the engine call is pending.
        try await draft.setFee(.recommended(targetBlocks: 2))
        do {
            try await draft.abandon(prepared)
            Issue.record("abandon during a broadcast must refuse")
        } catch {
            #expect(error.code == .sendBroadcastOutcomeUnknown)
        }
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("a second concurrent broadcast must refuse")
        } catch {
            #expect(error.code == .sendBroadcastOutcomeUnknown)
        }
        #expect(fake.with { $0.abandoned }.isEmpty)
        latch.open()
        let result = try await sending.value
        #expect(result.txid == prepared.summary.txid)
    }

    @Test func editingAfterPrepareAbandonsTheUnsentTransaction() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        let prepared = try await draft.prepare(grant: grant(.spend(max: WalletRuntime.Amount(duffs: 2000))))
        try await draft.setSource(.any)
        #expect(fake.with { $0.abandoned } == [prepared.summary.txid])
        do {
            _ = try await draft.broadcast(prepared)
            Issue.record("broadcast after an edit must fail")
        } catch {
            #expect(error.code == .sendPreparedTxUnknown)
        }
    }

    /// The adapter hands signed amounts to DashKit unchanged; DashKit's FFI
    /// conversion turns a negative one into `send.invalid_amount{index}`
    /// (DashKitTests.ExactConversionTests), as the engine does for zero, and
    /// the index survives the mapping to `ServiceError`.
    @Test func negativeAmountsReachDashKitUnconvertedAndMapToRecipientErrors() async throws {
        let h = Harness()
        let (draft, fake) = try await draft(h)
        do {
            try await draft.setRecipients([
                PaymentRecipient(address: "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n", amount: WalletRuntime.Amount(duffs: 1000)),
                PaymentRecipient(address: "ybt3gVM6cM9WprG7bRTMst1YR2GnAbWGLr", amount: WalletRuntime.Amount(duffs: -1)),
            ])
            Issue.record("a negative amount must be refused")
        } catch {
            #expect(error.code == .sendInvalidAmount)
            #expect(error.recipientIndex == 1)
            #expect(error.parameters["index"] == 1)
        }
        #expect(fake.with { $0.submitted.last?.map(\.amount.duffs) } == [1000, -1])
    }

    @Test func balanceErrorsCarryTheirNumbers() {
        let error = ServiceError(
            DashKitError.parameterized(
                code: "send.amount_with_fee_exceeds_balance", parameters: ["fee": 226, "available": 1000]))
        #expect(error.code == .sendAmountWithFeeExceedsBalance)
        #expect(error.parameters == ["fee": 226, "available": 1000])
    }

    @Test func maxSpendablePassesTheEngineStubThrough() async throws {
        let h = Harness()
        try await h.start()
        do {
            _ = try await h.services.sender.maxSpendable(
                wallet: WalletRuntime.WalletID(Fixtures.walletA), source: .any, fee: .recommended(targetBlocks: 6))
            Issue.record("stub must fail")
        } catch {
            #expect(error.code == .notImplemented)
        }
    }
}

@MainActor
@Suite struct QueryServiceTests {
    @Test func everyServiceNeedsAnOpenNetwork() async throws {
        let h = Harness()
        let wallet = WalletRuntime.WalletID(Fixtures.walletA)
        let s = h.services
        var codes: [ServiceErrorCode] = []
        func capture(_ body: () async throws -> Void) async {
            do { try await body() } catch { codes.append((error as? ServiceError)?.code ?? .internal) }
        }
        await capture { _ = try await s.history.page(wallet: wallet, query: WalletRuntime.HistoryQuery()) }
        await capture { _ = try await s.receive.currentAddress(wallet: wallet) }
        await capture { _ = try await s.coinControl.lockedOutpoints(wallet: wallet) }
        await capture { _ = try await s.addressBook.entries(wallet: wallet, purpose: nil, search: nil) }
        await capture { _ = try await s.sender.makeDraft(wallet: wallet) }
        await capture { _ = try await s.messages.sign(wallet: wallet, address: "a", message: "m", grant: grant(.signMessage)) }
        await capture { _ = try await s.vault.status() }
        #expect(codes == Array(repeating: .networkNotOpen, count: 7))
    }

    @Test func notImplementedEngineCallsSurfaceAsTypedErrors() async throws {
        let h = Harness()
        try await h.start()
        let wallet = WalletRuntime.WalletID(Fixtures.walletA)
        let s = h.services
        var codes: [ServiceErrorCode] = []
        func capture(_ body: () async throws -> Void) async {
            do { try await body() } catch { codes.append((error as? ServiceError)?.code ?? .internal) }
        }
        await capture { _ = try await s.history.page(wallet: wallet, query: WalletRuntime.HistoryQuery()) }
        await capture { _ = try await s.history.detail(wallet: wallet, txid: "t") }
        await capture { _ = try await s.receive.nextAddress(wallet: wallet, label: nil) }
        await capture { _ = try await s.receive.requests(wallet: wallet) }
        await capture { _ = try await s.coinControl.utxos(wallet: wallet, filter: WalletRuntime.UtxoFilter()) }
        await capture { try await s.coinControl.lock(wallet: wallet, outpoints: []) }
        await capture { _ = try await s.addressBook.save(wallet: wallet, address: "a", label: "l", purpose: .send, replace: false) }
        await capture { try await s.addressBook.delete(wallet: wallet, address: "a") }
        #expect(codes == Array(repeating: .notImplemented, count: 8))
    }

    @Test func historyChangesFollowTheWalletAndMerge() async throws {
        let h = Harness()
        try await h.start()
        let stream = h.services.history.changes(wallet: WalletRuntime.WalletID(Fixtures.walletA))
        var iterator = stream.makeAsyncIterator()
        try await Task.sleep(for: .milliseconds(20))
        h.engine.events.publish(.historyChanged(.regtest, Fixtures.walletB, txids: ["other"]))
        h.engine.events.publish(.historyChanged(.testnet, Fixtures.walletA, txids: ["wrong-network"]))
        h.engine.events.publish(.historyChanged(.regtest, Fixtures.walletA, txids: ["t1"]))
        let first = await iterator.next()
        #expect(first == ["t1"])
        h.engine.events.publish(.historyChanged(.regtest, Fixtures.walletA, txids: ["t2"]))
        h.engine.events.publish(.historyChanged(.regtest, Fixtures.walletA, txids: ["t3", "t2"]))
        try await Task.sleep(for: .milliseconds(20))
        #expect(await iterator.next() == ["t2", "t3"])
        h.engine.events.publish(.resynchronize)
        #expect(await iterator.next() == [])
    }

    @Test func messageSigningChecksTheGrant() async throws {
        let h = Harness {
            $0.with { $0.signature = .success("SIG") }
        }
        try await h.start()
        let wallet = WalletRuntime.WalletID(Fixtures.walletA)
        do {
            _ = try await h.services.messages.sign(wallet: wallet, address: "a", message: "m", grant: grant(.revealSecret))
            Issue.record("sign with a reveal grant must fail")
        } catch {
            #expect(error.code == .vaultGrantPurposeMismatch)
        }
        let signature = try await h.services.messages.sign(
            wallet: wallet, address: "a", message: "m", grant: grant(.signMessage, id: "gs"))
        #expect(signature == "SIG")
        #expect(h.engine.calls.contains("signMessage gs"))
    }
}
