// M2 macOS flows in demo mode: Options (QT-135/136), coin control from the
// Send page (QT-068…074), PSBT load from the clipboard (QT-077) and the
// Tools console (QT-145).
import AppKit
import XCTest

@MainActor
final class M2UITests: XCTestCase {
    static let timeout: TimeInterval = 10

    override func setUp() async throws {
        continueAfterFailure = false
    }

    private func launch(_ scenario: String = "funded") -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "--demo-scenario", scenario, "--no-menu-bar-extra", "--appearance", "light",
            "-ApplePersistenceIgnoreState", "YES",
        ]
        app.launch()
        app.activate()
        return app
    }

    private func element(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
        app.descendants(matching: .any)[identifier].firstMatch
    }

    /// A segment of a segmented control by its title.
    private func segment(_ app: XCUIApplication, _ container: String, _ title: String) -> XCUIElement {
        let control = element(app, container)
        let radio = control.radioButtons[title]
        return radio.exists ? radio : control.buttons[title]
    }

    private func attachScreenshot(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func openMainWindow(_ app: XCUIApplication) {
        XCTAssertTrue(element(app, "overview").waitForExistence(timeout: Self.timeout), app.debugDescription)
    }

    // MARK: Flows

    /// Options ▸ Wallet ▸ "Enable coin control features", OK; the Send page
    /// shows the coin control box; Inputs… opens Coin Selection, a coin is
    /// picked, and the box shows the selection instead of "automatically
    /// selected".
    func testCoinControlFromSend() {
        let app = launch()
        openMainWindow(app)
        app.typeKey(",", modifierFlags: .command)
        XCTAssertTrue(element(app, "options").waitForExistence(timeout: Self.timeout))
        segment(app, "options.tabs", "Wallet").click()
        let toggle = element(app, "options.coinControl")
        XCTAssertTrue(toggle.waitForExistence(timeout: Self.timeout))
        toggle.click()
        element(app, "options.ok").click()
        XCTAssertTrue(element(app, "options").waitForNonExistence(timeout: Self.timeout))

        element(app, "sidebar.send").click()
        let automatic = element(app, "send.coinControl.automatic")
        XCTAssertTrue(automatic.waitForExistence(timeout: Self.timeout))
        element(app, "send.coinControl.inputs").click()

        XCTAssertTrue(element(app, "coinControl").waitForExistence(timeout: Self.timeout))
        let firstCheckbox = app.checkBoxes.matching(NSPredicate(format: "identifier BEGINSWITH 'coinControl.check.'"))
            .firstMatch
        XCTAssertTrue(firstCheckbox.waitForExistence(timeout: Self.timeout))
        firstCheckbox.click()
        XCTAssertTrue(element(app, "coinControl.summary").waitForExistence(timeout: Self.timeout))
        attachScreenshot(app, "coin-selection")
        element(app, "coinControl.ok").click()

        XCTAssertTrue(automatic.waitForNonExistence(timeout: Self.timeout))
        XCTAssertTrue(element(app, "coinControl.summary").exists)
    }

    /// Options: Cancel discards an edited value; the network tab keeps the
    /// proxy fields disabled with the reason; the SPV footnote is shown.
    func testOptionsCancelDiscards() {
        let app = launch()
        openMainWindow(app)
        app.typeKey(",", modifierFlags: .command)
        XCTAssertTrue(element(app, "options").waitForExistence(timeout: Self.timeout))
        segment(app, "options.tabs", "Wallet").click()
        let subtractFee = element(app, "options.subtractFee")
        XCTAssertTrue(subtractFee.waitForExistence(timeout: Self.timeout))
        let before = subtractFee.value as? Int
        subtractFee.click()
        element(app, "options.cancel").click()
        XCTAssertTrue(element(app, "options").waitForNonExistence(timeout: Self.timeout))

        app.typeKey(",", modifierFlags: .command)
        segment(app, "options.tabs", "Wallet").click()
        XCTAssertTrue(subtractFee.waitForExistence(timeout: Self.timeout))
        XCTAssertEqual(subtractFee.value as? Int, before)
        segment(app, "options.tabs", "Network").click()
        XCTAssertTrue(element(app, "options.proxyUnavailable").waitForExistence(timeout: Self.timeout))
        XCTAssertTrue(element(app, "options.spvFootnote").exists)
        attachScreenshot(app, "options-network")
    }

    /// File ▸ Load PSBT from clipboard with text that is not base64: the
    /// PSBT Operations window opens with dash-qt's decode error and Sign /
    /// Broadcast disabled.
    func testLoadPSBTFromClipboard() {
        let app = launch()
        openMainWindow(app)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("this is not a psbt", forType: .string)
        app.menuBars.menuBarItems["File"].click()
        app.menuItems["Load PSBT from clipboard…"].click()
        XCTAssertTrue(element(app, "psbt").waitForExistence(timeout: Self.timeout))
        let error = element(app, "psbt.error")
        XCTAssertTrue(error.waitForExistence(timeout: Self.timeout))
        XCTAssertFalse(element(app, "psbt.sign").isEnabled)
        XCTAssertFalse(element(app, "psbt.broadcast").isEnabled)
        attachScreenshot(app, "psbt-clipboard-error")
        element(app, "psbt.close").click()
    }

    /// Window ▸ Console: the welcome text and scam warning, then a command
    /// and its reply; Up recalls the command.
    func testToolsConsole() {
        let app = launch()
        openMainWindow(app)
        app.menuBars.menuBarItems["Window"].click()
        app.menuItems["Console"].click()
        let input = element(app, "console.input")
        XCTAssertTrue(input.waitForExistence(timeout: Self.timeout))
        XCTAssertTrue(element(app, "console.entry.warning").exists)
        input.click()
        input.typeText("getblockcount\r")
        XCTAssertTrue(element(app, "console.entry.reply").waitForExistence(timeout: Self.timeout))
        input.click()
        input.typeKey(.upArrow, modifierFlags: [])
        let recalled = (input.value as? String) ?? ""
        XCTAssertEqual(recalled, "getblockcount")
        attachScreenshot(app, "tools-console")
    }
}
