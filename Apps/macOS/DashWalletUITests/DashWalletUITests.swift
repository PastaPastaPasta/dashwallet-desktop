// Critical macOS flows in demo mode (DESIGN-opus §4.1): onboarding create,
// unlock, send review/cancel, receive address, transaction filter.
import XCTest

@MainActor
final class DashWalletUITests: XCTestCase {
    /// A testnet-shaped address the demo URI handler accepts (34 base58 characters, `y` prefix).
    static let recipient = "yAb3Cd4Ef5Gh6Jk7Lm8Np9Qr1St2Uv3Wx4"
    static let timeout: TimeInterval = 10

    override func setUp() async throws {
        continueAfterFailure = false
    }

    private func launch(_ scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--demo-scenario", scenario, "--no-menu-bar-extra", "--appearance", "light",
            // A closed Window scene is not reopened when state is restored.
            "-ApplePersistenceIgnoreState", "YES",
        ]
        app.launch()
        app.activate()
        return app
    }

    private func element(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
        app.descendants(matching: .any)[identifier].firstMatch
    }

    private func text(of element: XCUIElement) -> String {
        if let value = element.value as? String, !value.isEmpty { return value }
        return element.label
    }

    /// Waits until `predicate` holds for `element`.
    private func wait(_ element: XCUIElement, _ predicate: NSPredicate, timeout: TimeInterval = DashWalletUITests.timeout) -> Bool {
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: element)
        return XCTWaiter().wait(for: [expectation], timeout: timeout) == .completed
    }

    private func attachScreenshot(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.windows.firstMatch.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    // MARK: Flows

    func testOnboardingCreate() throws {
        let app = launch("fresh")
        let create = element(app, "onboarding.create")
        XCTAssertTrue(create.waitForExistence(timeout: Self.timeout), app.debugDescription)
        create.click()

        let firstWord = app.staticTexts["onboarding.word.0"]
        XCTAssertTrue(firstWord.waitForExistence(timeout: Self.timeout))
        var words: [String] = []
        for index in 0..<12 {
            words.append(text(of: app.staticTexts["onboarding.word.\(index)"]))
        }
        XCTAssertEqual(words.count, 12)
        XCTAssertFalse(words.contains(""))
        element(app, "onboarding.writtenDown").click()

        // Pick the chip for each requested word position (IOS-004).
        for _ in 0..<4 {
            let current = app.staticTexts["onboarding.challenge.current"]
            guard current.waitForExistence(timeout: 2) else { break }
            let label = text(of: current)
            let position = try XCTUnwrap(Int(label.replacingOccurrences(of: "Word #", with: ""))) - 1
            app.buttons["onboarding.chip.\(words[position])"].click()
        }

        let passphrase = app.secureTextFields.element(boundBy: 0)
        XCTAssertTrue(passphrase.waitForExistence(timeout: Self.timeout))
        passphrase.click()
        passphrase.typeText("correct horse battery")
        let confirmation = app.secureTextFields.element(boundBy: 1)
        confirmation.click()
        confirmation.typeText("correct horse battery")
        element(app, "onboarding.encrypt").click()

        XCTAssertTrue(element(app, "overview").waitForExistence(timeout: Self.timeout))
        attachScreenshot(app, "onboarding-done")
    }

    func testUnlock() {
        let app = launch("locked")
        let field = app.secureTextFields["lock.passphrase"]
        XCTAssertTrue(field.waitForExistence(timeout: Self.timeout))
        field.click()
        field.typeText("wrong\r")
        XCTAssertTrue(app.staticTexts["lock.message"].waitForExistence(timeout: Self.timeout))

        field.click()
        field.typeText("demo\r")
        XCTAssertTrue(element(app, "overview").waitForExistence(timeout: Self.timeout))
        XCTAssertTrue(wait(field, NSPredicate(format: "exists == false")))
    }

    func testSendReviewCancel() {
        let app = launch("funded")
        let sidebar = element(app, "sidebar.send")
        XCTAssertTrue(sidebar.waitForExistence(timeout: Self.timeout))
        sidebar.click()

        let address = app.textFields["send.address.0"]
        XCTAssertTrue(address.waitForExistence(timeout: Self.timeout))
        address.click()
        address.typeText(Self.recipient)
        let amount = element(app, "send.amount.0").textFields.firstMatch
        amount.click()
        amount.typeText("0.1")
        element(app, "send.review").click()

        // Confirm dialog with the 3 s countdown: Send starts disabled (QT-059).
        let send = app.buttons["send.confirm.send"]
        XCTAssertTrue(send.waitForExistence(timeout: Self.timeout))
        XCTAssertFalse(send.isEnabled)
        attachScreenshot(app, "send-confirm")
        app.buttons["send.confirm.cancel"].click()

        XCTAssertTrue(wait(send, NSPredicate(format: "exists == false")))
        XCTAssertTrue(element(app, "send.review").isEnabled)
        XCTAssertEqual(text(of: address), Self.recipient)
    }

    func testReceiveShowsAddress() {
        let app = launch("funded")
        let sidebar = element(app, "sidebar.receive")
        XCTAssertTrue(sidebar.waitForExistence(timeout: Self.timeout))
        sidebar.click()

        let address = app.staticTexts["receive.address"]
        XCTAssertTrue(address.waitForExistence(timeout: Self.timeout))
        let shown = text(of: address)
        XCTAssertTrue(shown.hasPrefix("y"), "testnet address expected, got \(shown)")
        XCTAssertEqual(shown.count, 34)
        XCTAssertTrue(element(app, "receive.qr").exists)
    }

    func testTransactionsFilter() {
        let app = launch("funded")
        let sidebar = element(app, "sidebar.transactions")
        XCTAssertTrue(sidebar.waitForExistence(timeout: Self.timeout))
        sidebar.click()
        // The page opens in the iOS list; the dash-qt types are in the table.
        let table = element(app, "transactions.layout.table")
        XCTAssertTrue(table.waitForExistence(timeout: Self.timeout))
        table.click()

        let count = app.staticTexts["transactions.count"]
        XCTAssertTrue(count.waitForExistence(timeout: Self.timeout))
        let before = text(of: count)

        let filter = app.popUpButtons["transactions.typeFilter"]
        XCTAssertTrue(filter.waitForExistence(timeout: Self.timeout))
        filter.click()
        app.menuItems["Sent to"].click()

        XCTAssertTrue(wait(count, NSPredicate(format: "value != %@ AND label != %@", before, before)))
        let types = app.staticTexts.matching(identifier: "transactions.type").allElementsBoundByIndex.map(text(of:))
        XCTAssertFalse(types.isEmpty)
        XCTAssertTrue(types.allSatisfy { $0 == "Sent to" }, "rows: \(types)")
    }
}
