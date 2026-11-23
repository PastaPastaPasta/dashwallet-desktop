// Lock screen and the iOS lockout policy (QT-111, QT-112, IOS-012, IOS-013).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Lock view model")
struct LockViewModelTests {
    let world = FakeWorld()

    init() {
        world.auth.lockState = .locked
    }

    func makeModel() -> LockViewModel {
        LockViewModel(auth: world.auth, vault: world.vault, timing: world.timing)
    }

    func failOnce(_ model: LockViewModel) async {
        world.auth.unlockErrors = [ServiceError(code: .vaultWrongPassphrase)]
        await model.unlock(passphrase: "wrong", mixingOnly: false)
    }

    @Test func IOS012_policyIsThreeFreeThenSixfoldWaitsThenDisabledAtEight() {
        #expect((0..<3).allSatisfy { LockoutPolicy.verdict(failures: $0) == .allowed })
        #expect(LockoutPolicy.verdict(failures: 3) == .wait(seconds: 60))
        #expect(LockoutPolicy.verdict(failures: 4) == .wait(seconds: 360))
        #expect(LockoutPolicy.verdict(failures: 5) == .wait(seconds: 2_160))
        #expect(LockoutPolicy.verdict(failures: 6) == .wait(seconds: 12_960))
        #expect(LockoutPolicy.verdict(failures: 7) == .wait(seconds: 77_760))
        #expect(LockoutPolicy.verdict(failures: 8) == .disabled)
        #expect(LockoutPolicy.verdict(failures: 20) == .disabled)
        #expect(LockoutPolicy.attemptsRemaining(failures: 0) == 8)
        #expect(LockoutPolicy.attemptsRemaining(failures: 9) == 0)
    }

    @Test func QT111_unlockSucceeds() async {
        let model = makeModel()
        #expect(model.isLocked)
        await model.unlock(passphrase: "right", mixingOnly: false)
        #expect(world.auth.unlockCalls.first?.0 == "right")
        #expect(world.auth.unlockCalls.first?.1 == .full)
        #expect(model.lockState == .unlocked)
        #expect(!model.isLocked)
        #expect(model.message == nil)
    }

    @Test func QT112_unlockForMixingOnly() async {
        let model = makeModel()
        await model.unlock(passphrase: "right", mixingOnly: true)
        #expect(world.auth.unlockCalls.first?.1 == .mixingOnly)
        #expect(model.lockState == .unlockedMixingOnly)
        #expect(model.isLocked)
    }

    @Test func emptyPassphraseIsNotSent() async {
        let model = makeModel()
        await model.unlock(passphrase: "", mixingOnly: false)
        #expect(world.auth.unlockCalls.isEmpty)
        #expect(model.message == L10n.Common.passphraseRequired)
    }

    @Test func IOS012_wrongPassphrasesCountDownThenThrottle() async {
        let model = makeModel()
        await failOnce(model)
        #expect(model.failedAttempts == 1)
        #expect(model.message == L10n.Common.wrongPassphrase + " " + L10n.Lock.attemptsRemaining(7))
        #expect(model.retryAfter == nil)
        await failOnce(model)
        await failOnce(model)
        #expect(model.failedAttempts == 3)
        #expect(model.retryAfter == .seconds(60))
        #expect(model.message == L10n.Lock.tryAgainIn("1 minute"))
        // While throttled the passphrase is not even tried.
        await model.unlock(passphrase: "right", mixingOnly: false)
        #expect(world.auth.unlockCalls.count == 3)
        world.clock.advance(30)
        model.refreshCountdown()
        #expect(model.retryAfter == .seconds(30))
        world.clock.advance(31)
        model.refreshCountdown()
        #expect(model.retryAfter == nil)
        #expect(model.message == nil)
        await model.unlock(passphrase: "right", mixingOnly: false)
        #expect(world.auth.unlockCalls.count == 4)
        #expect(model.failedAttempts == 0)
    }

    @Test func IOS012_fourthFailureWaitsSixMinutes() async {
        let model = makeModel()
        for _ in 0..<3 { await failOnce(model) }
        world.clock.advance(61)
        await failOnce(model)
        #expect(model.failedAttempts == 4)
        #expect(model.retryAfter == .seconds(360))
        #expect(model.message == L10n.Lock.tryAgainIn("6 minutes"))
    }

    @Test func IOS012_eightFailuresDisableThePassphrase() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .locked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 7, retryAfterSeconds: nil,
                walletsWithSecrets: [])
        }
        let model = makeModel()
        await model.load()
        #expect(model.failedAttempts == 7)
        #expect(model.retryAfter == .seconds(77_760))
        #expect(model.message == L10n.Lock.tryAgainIn("22 hours"))
        world.clock.advance(77_761)
        await failOnce(model)
        #expect(model.isDisabled)
        #expect(model.message == L10n.Lock.disabled)
        await model.unlock(passphrase: "right", mixingOnly: false)
        #expect(world.auth.unlockCalls.count == 1)
    }

    @Test func IOS012_failureCountSurvivesARestart() async {
        world.vault.state.withLock {
            $0.status = VaultStatus(
                state: .locked, encrypted: true, quickUnlockEnrolled: false, failedAttempts: 3, retryAfterSeconds: nil,
                walletsWithSecrets: [])
        }
        let model = makeModel()
        await model.load()
        #expect(model.retryAfter == .seconds(60))
    }

    @Test func engineThrottleWinsWhenLonger() async {
        world.auth.unlockErrors = [ServiceError(code: .vaultThrottled, retryAfterSeconds: 600)]
        let model = makeModel()
        await model.unlock(passphrase: "x", mixingOnly: false)
        #expect(model.failedAttempts == 0)
        #expect(model.retryAfter == .seconds(600))
        #expect(model.message == L10n.Lock.tryAgainIn("10 minutes"))
    }

    @Test func IOS013_followsLockStateChanges() async {
        let model = makeModel()
        model.start()
        world.auth.publish(.unlocked)
        await eventually { model.lockState == .unlocked }
        world.auth.publish(.locked)
        await eventually { model.lockState == .locked }
        model.stop()
    }

    @Test func QT111_lock() async {
        world.auth.lockState = .unlocked
        let model = makeModel()
        await model.lock()
        #expect(world.auth.lockCount == 1)
        #expect(model.lockState == .locked)
    }

    @Test func durationText() {
        #expect(LockViewModel.describe(.seconds(1)) == "1 minute")
        #expect(LockViewModel.describe(.seconds(360)) == "6 minutes")
        #expect(LockViewModel.describe(.seconds(7_199)) == "120 minutes" || LockViewModel.describe(.seconds(7_199)) == "2 hours")
        #expect(LockViewModel.describe(.seconds(12_960)) == "4 hours")
    }
}
