// Address book (QT-095…098) and Sign / Verify message (QT-099/100).
import Foundation
import Testing
@testable import WalletFeatures
import WalletRuntime

@MainActor
@Suite("Address book view model")
struct AddressBookViewModelTests {
    let world = FakeWorld()

    init() {
        world.addressBook.entries.withLock {
            $0 = [
                AddressBookEntry(address: testnetAddress2, label: "bob", purpose: .send, createdAt: nil),
                AddressBookEntry(address: testnetAddress1, label: "Alice", purpose: .send, createdAt: nil),
                AddressBookEntry(address: testnetScriptAddress, label: "", purpose: .receive, createdAt: nil),
            ]
        }
    }

    func makeModel(purpose: AddressPurpose = .send, selectionMode: Bool = false) async -> AddressBookViewModel {
        let model = AddressBookViewModel(
            env: world.environment(), network: world.network, purpose: purpose, selectionMode: selectionMode)
        await model.load()
        return model
    }

    @Test func QT095_sendingListSortedByLabel() async {
        let model = await makeModel()
        #expect(model.entries.map(\.label) == ["Alice", "bob"])
        #expect(model.header == L10n.AddressBook.sendingHeader)
        #expect(model.canCreate && model.canDelete)
    }

    @Test func QT095_wildcardSearchOnAddressOrLabel() async {
        let model = await makeModel()
        model.setSearch("al?ce")
        #expect(model.entries.map(\.label) == ["Alice"])
        model.setSearch("B?B")
        #expect(model.entries.map(\.label) == ["bob"])
        model.setSearch("00002")
        #expect(model.entries.map(\.label) == ["bob"])
        model.setSearch("")
        #expect(model.entries.count == 2)
        #expect(WildcardMatcher(pattern: "a*c").matches("xxABCxx"))
        #expect(!WildcardMatcher(pattern: "a?c").matches("abbc"))
    }

    @Test func QT095_newEntryIsValidatedAndSaved() async {
        let model = await makeModel()
        let fresh = "yFreshAddress000000000000000000009"
        #expect(await model.save(address: " \(fresh) ", label: "Carol", replace: false))
        #expect(world.addressBook.saves.current.last?.address == fresh)
        #expect(model.entries.map(\.label) == ["Alice", "bob", "Carol"])
        #expect(!(await model.save(address: "garbage", label: "x", replace: false)))
        #expect(model.errorMessage == L10n.AddressBook.invalidAddress("garbage"))
    }

    @Test func QT098_duplicateMessagesMatchDashQt() async {
        let model = await makeModel()
        #expect(!(await model.save(address: testnetAddress1, label: "again", replace: false)))
        #expect(model.errorMessage == L10n.AddressBook.alreadyInBook(testnetAddress1, label: "Alice"))
        #expect(!(await model.save(address: testnetScriptAddress, label: "mine", replace: false)))
        #expect(model.errorMessage == L10n.AddressBook.existsAsReceiving(testnetScriptAddress, label: ""))
        #expect(await model.save(address: testnetAddress1, label: "Alice B.", replace: true))
        #expect(world.addressBook.saves.current.last?.replace == true)
    }

    @Test func QT096_receivingEntriesEditLabelOnlyNoDelete() async {
        let model = await makeModel(purpose: .receive)
        #expect(model.entries.map(\.address) == [testnetScriptAddress])
        #expect(!model.canCreate && !model.canDelete)
        #expect(model.labelText(for: model.entries[0]) == L10n.AddressBook.noLabel)
        #expect(await model.save(address: testnetScriptAddress, label: "Savings", replace: true))
        #expect(!(await model.save(address: "yNotInBook0000000000000000000000001", label: "x", replace: true)))
        #expect(model.errorMessage == L10n.AddressBook.entryNotFound)
        await model.delete(address: testnetScriptAddress)
        #expect(model.errorMessage == L10n.AddressBook.receivingNotDeletable)
        #expect(world.addressBook.deletes.current.isEmpty)
    }

    @Test func QT095_deleteSendingEntry() async {
        let model = await makeModel()
        await model.delete(address: testnetAddress2)
        #expect(world.addressBook.deletes.current == [testnetAddress2])
        #expect(model.entries.map(\.label) == ["Alice"])
    }

    @Test func QT095_exportCSVHasLabelAndAddress() async {
        let model = await makeModel()
        #expect(model.exportCSV() == "\"Label\",\"Address\"\n\"Alice\",\"\(testnetAddress1)\"\n\"bob\",\"\(testnetAddress2)\"\n")
    }

    @Test func QT097_selectionMode() async {
        let picker = await makeModel(selectionMode: true)
        picker.choose(picker.entries[0])
        #expect(picker.chosen?.label == "Alice")
        let plain = await makeModel()
        plain.choose(plain.entries[0])
        #expect(plain.chosen == nil)
    }

    @Test func engineDuplicateCodeMapsToDashQtText() async {
        world.addressBook.saveError.withLock { $0 = ServiceError(code: .labelsDuplicateAddress) }
        let model = await makeModel()
        #expect(!(await model.save(address: "yFreshAddress000000000000000000009", label: "x", replace: false)))
        #expect(model.errorMessage == L10n.AddressBook.alreadyInBook("yFreshAddress000000000000000000009", label: ""))
    }
}

@MainActor
@Suite("Sign / verify view model")
struct SignVerifyViewModelTests {
    let world = FakeWorld()

    func makeModel() -> SignVerifyViewModel {
        SignVerifyViewModel(env: world.environment(), network: world.network)
    }

    @Test func QT099_signWithUnencryptedWallet() async {
        let model = makeModel()
        model.address = testnetAddress1
        model.message = "hello"
        await model.sign()
        #expect(model.signature == "H+signature==")
        #expect(model.signResult == .signed)
        #expect(model.result?.text == L10n.SignVerify.signed)
        #expect(world.auth.authorizeCalls.first?.purpose == .signMessage)
        let call = world.messages.signCalls.current.first
        #expect(call?.0 == testnetAddress1 && call?.1 == "hello" && call?.2.purpose == .signMessage)
    }

    @Test func QT099_encryptedWalletNeedsThePassphrase() async {
        world.auth.defaultRequirement = .passphrase
        let model = makeModel()
        model.address = testnetAddress1
        await model.sign()
        #expect(model.needsPassphrase)
        #expect(world.messages.signCalls.current.isEmpty)
        await model.sign(passphrase: "pw")
        #expect(!model.needsPassphrase)
        #expect(world.auth.authorizeCalls.first?.passphrase == "pw")
        #expect(model.signResult == .signed)
        model.cancelPassphrase()
        #expect(model.signResult == .failed(.vaultLocked, message: L10n.Common.unlockCancelled))
    }

    @Test func QT099_addressChecksUseDashQtTexts() async {
        let model = makeModel()
        model.address = "nonsense"
        await model.sign()
        #expect(model.signResult?.text == L10n.SignVerify.invalidAddress)
        model.address = testnetScriptAddress
        await model.sign()
        #expect(model.signResult?.text == L10n.SignVerify.addressNoKey)
        #expect(world.auth.authorizeCalls.isEmpty)
    }

    @Test func QT099_engineErrorsMapToDashQtTexts() async {
        world.messages.signResult.withLock { $0 = .failure(ServiceError(code: .init(rawValue: "message.address_not_mine"))) }
        let model = makeModel()
        model.address = testnetAddress1
        await model.sign()
        #expect(model.signResult?.text == L10n.SignVerify.privateKeyUnavailable)
        #expect(model.signResult?.isSuccess == false)
        #expect(SignVerifyViewModel.signText(.init(rawValue: "message.something")) == L10n.SignVerify.signingFailed)
    }

    @Test func QT100_verify() {
        let model = makeModel()
        model.verifyAddress = testnetAddress1
        model.verifyMessage = "hello"
        model.verifySignature = "sig"
        model.verify()
        #expect(model.verifyResult == .verified)
        #expect(model.result?.isSuccess == true)
        world.messages.verifyError.withLock { $0 = ServiceError(code: .init(rawValue: "message.malformed_signature")) }
        model.verify()
        #expect(model.verifyResult?.text == L10n.SignVerify.malformedSignature)
        world.messages.verifyError.withLock { $0 = ServiceError(code: .messageNotSigned) }
        model.verify()
        #expect(model.verifyResult?.text == L10n.SignVerify.verificationFailed)
        world.messages.verifyError.withLock { $0 = ServiceError(code: .init(rawValue: "message.pubkey_not_recovered")) }
        model.verify()
        #expect(model.verifyResult?.text == L10n.SignVerify.digestMismatch)
        model.verifyAddress = "bad"
        model.verify()
        #expect(model.verifyResult?.text == L10n.SignVerify.invalidAddress)
        model.clearVerify()
        #expect(model.verifyResult == nil && model.verifyAddress.isEmpty)
    }
}
