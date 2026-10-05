// Renders every main MacUI screen over the demo services in light and dark
// and checks the demo data reached the view models. With
// DWD_WRITE_SCREENSHOTS=1 the PNGs go to docs/screenshots/m1/.
#if os(macOS)
import AppKit
import DashUIMac
import DesignTokens
import Foundation
import SwiftUI
import Testing
import WalletFeatures
import WalletRuntime

@testable import MacUI

@MainActor
@Suite(.serialized)
struct ScreenTests {
    static let mainSize = CGSize(width: 1180, height: 760)

    // MARK: Screens

    @Test(arguments: [ColorScheme.light, .dark])
    func overview(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let main = try #require(model.main)
        let home = try #require(main.home)
        #expect(home.formattedTotal != nil)
        #expect(home.recent.count == HomeViewModel.recentLimit(coinJoin: false))
        #expect(home.sync?.isDone == true)
        try await Self.capture(Self.chrome(model, OverviewView(home: home, toggleDiscreet: {})), Self.mainSize, scheme, "overview")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func send(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let main = try #require(model.main)
        let send = try #require(main.send)
        main.selection = .send
        send.entries[0].address = "yAb3Cd4Ef5Gh6Jk7Lm8Np9Qr1St2Uv3Wx4"
        send.entries[0].label = "Alice"
        send.entries[0].amountText = "0.25"
        try await Self.capture(Self.chrome(model, SendView(model: model, send: send)), Self.mainSize, scheme, "send")

        await send.review()
        guard case .confirm = send.phase else {
            Issue.record("expected the confirm step, got \(send.phase)")
            return
        }
        #expect(!send.canConfirm, "Send stays disabled during the countdown")
        #expect(send.confirmLines.contains { $0.contains("yAb3Cd4Ef5Gh6Jk7Lm8Np9Qr1St2Uv3Wx4") })
        try await Self.capture(SendConfirmSheet(send: send), CGSize(width: 520, height: 300), scheme, "send-confirm")
        await send.cancel()
        #expect(send.phase == .editing)
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func receive(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let main = try #require(model.main)
        main.selection = .receive
        let receive = try #require(main.receive)
        await receive.load()
        let address = try #require(receive.address)
        #expect(address.address.hasPrefix("y"))
        #expect(receive.qr != nil)
        #expect(receive.uri == "dash:\(address.address)")
        try await Self.capture(
            Self.chrome(model, ReceiveView(receive: receive, unitName: model.unitName)), Self.mainSize, scheme, "receive")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func transactions(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let main = try #require(model.main)
        main.selection = .transactions
        let transactions = try #require(main.transactions)
        await transactions.reload()
        let all = transactions.rows.count
        #expect(all == 25)
        try await Self.capture(
            Self.chrome(
                model,
                TransactionsView(transactions: transactions, unitName: model.unitName, formatAmount: model.formatAmount)),
            Self.mainSize, scheme, "transactions")

        await transactions.setTypePreset(.sentTo)
        #expect(!transactions.rows.isEmpty)
        #expect(transactions.rows.count < all)
        #expect(transactions.rows.allSatisfy { $0.type == .sendToAddress || $0.type == .sendToOther })
        await transactions.setTypePreset(.all)

        let first = try #require(transactions.rows.first)
        await transactions.select(first.id)
        let detail = try #require(transactions.detail)
        try await Self.capture(
            TransactionDetailView(detail: detail, transactions: transactions, formatAmount: model.formatAmount,
                                  onClose: {}),
            CGSize(width: 620, height: 600), scheme, "transaction-detail")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func onboarding(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.fresh, scheme)
        let main = try #require(model.main)
        #expect(main.needsOnboarding)
        let onboarding = try #require(main.onboarding)
        try await Self.capture(OnboardingView(model: onboarding), Self.mainSize, scheme, "onboarding-welcome")
        await onboarding.startCreate()
        #expect(onboarding.step == .showPhrase)
        #expect(onboarding.phraseWords.count == 12)
        try await Self.capture(OnboardingView(model: onboarding), Self.mainSize, scheme, "onboarding-phrase")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func lock(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.locked, scheme)
        let main = try #require(model.main)
        #expect(main.showsLockScreen)
        try await Self.capture(LockScreenView(lock: main.lock, receive: main.receive), Self.mainSize, scheme, "lock")
        await main.lock.unlock(passphrase: "wrong", mixingOnly: false)
        #expect(main.lock.message != nil)
        await main.lock.unlock(passphrase: DemoStore.lockedPassphrase, mixingOnly: false)
        #expect(main.lock.lockState == .unlocked)
        try await Self.settle { main.lockState == .unlocked }
        #expect(!main.showsLockScreen)
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func addressBook(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let book = try #require(model.makeAddressBook(purpose: .send))
        await book.load()
        #expect(book.entries.count == 5)
        try await Self.capture(
            AddressBookView(book: book, onChoose: nil), CGSize(width: 640, height: 460), scheme, "address-book")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func signVerify(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        let signVerify = try #require(model.makeSignVerify())
        let address = try #require(model.main?.receive?.address?.address)
        signVerify.address = address
        signVerify.message = "I own this address."
        await signVerify.sign()
        #expect(signVerify.signResult == .signed)
        signVerify.verifyAddress = address
        signVerify.verifyMessage = signVerify.message
        signVerify.verifySignature = signVerify.signature
        signVerify.verify()
        #expect(signVerify.verifyResult == .verified)
        signVerify.verifyMessage = "Something else."
        signVerify.verify()
        #expect(signVerify.verifyResult?.isSuccess == false)
        try await Self.capture(SignVerifyWindow(model: model), CGSize(width: 640, height: 520), scheme, "sign-verify")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func settings(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        try await Self.capture(SettingsView(model: model), CGSize(width: 520, height: 420), scheme, "settings")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func menuBar(_ scheme: ColorScheme) async throws {
        let model = try await Self.model(.funded, scheme)
        try await Self.capture(MenuBarContentView(model: model), CGSize(width: 280, height: 470), scheme, "menu-bar")
    }

    // MARK: Behaviour without rendering

    @Test func discreetModeMasksOverview() async throws {
        let model = try await Self.model(.funded, .light)
        let main = try #require(model.main)
        let home = try #require(main.home)
        main.settings.setDiscreet(true)
        try await Self.settle { home.recent.isEmpty }
        #expect(home.discreet)
        #expect(home.formattedTotal?.contains("#") == true)
        // HomeViewModel.reloadRecent checks `discreet` before awaiting the
        // history page but not after, so a reload in flight when discreet
        // mode turns on writes the rows back (WalletFeatures WIP, owner D).
        withKnownIssue("HomeViewModel.reloadRecent race refills recent rows in discreet mode", isIntermittent: true) {
            #expect(home.recent.isEmpty)
        }
    }

    @Test func openURIFillsSend() async throws {
        let model = try await Self.model(.funded, .light)
        let main = try #require(model.main)
        await model.open(uri: "dash:yAb3Cd4Ef5Gh6Jk7Lm8Np9Qr1St2Uv3Wx4?amount=1.5&label=Shop")
        #expect(main.selection == .send)
        #expect(main.send?.entries.first?.address == "yAb3Cd4Ef5Gh6Jk7Lm8Np9Qr1St2Uv3Wx4")
        #expect(main.send?.entries.first?.label == "Shop")
        #expect(model.uriError == nil)
        await model.open(uri: "dash:notanaddress")
        #expect(model.uriError != nil)
    }

    @Test func launchOptions() {
        #expect(LaunchOptions.parse(["app"]).demoScenario == nil)
        #expect(LaunchOptions.parse(["app", "--demo"]).demoScenario == .funded)
        #expect(LaunchOptions.parse(["app", "--fixture"]).demoScenario == .funded)
        #expect(LaunchOptions.parse(["app", "--demo-scenario", "locked"]).demoScenario == .locked)
        #expect(LaunchOptions.parse(["app", "-NSDocumentRevisionsDebugMode", "YES"]).demoScenario == nil)
        #expect(LaunchOptions.parse(["app"], environment: ["DWD_DEMO": "1"]).demoScenario == .funded)
        let options = LaunchOptions.parse(["app", "--appearance", "dark", "--no-menu-bar-extra"])
        #expect(options.appearance == .dark)
        #expect(!options.menuBarExtra)
        #expect(AppPaths.dataDirectory().path.hasSuffix("Library/Application Support/org.dashfoundation.DashWallet"))
    }

    @Test func liveCompositionReportsMissingRuntime() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("dwd-macui-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let model = MacAppComposition.makeModel(launch: LaunchOptions(), dataDirectory: directory) { url throws(ServiceError) in
            throw ServiceError(code: .notImplemented, detail: url.path)
        }
        #expect(model.main == nil)
        #expect(model.unavailableReason?.contains(directory.path) == true)
        #expect(FileManager.default.fileExists(atPath: directory.path))
    }

    @Test func demoPhraseUsesBIP39Words() {
        #expect(DemoStore.phrase.count == 24)
        #expect(Set(DemoStore.phrase).count == 24)
        #expect(DemoStore.phrase.allSatisfy(DemoVault.isWord))
        #expect(!DemoVault.isWord("notaword"))
    }

    // MARK: Helpers

    /// Polls `condition` on the main actor for up to two seconds.
    static func settle(_ condition: () -> Bool) async throws {
        for _ in 0..<40 where !condition() {
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    /// The main window around `page`: title strip, sidebar rows and the real
    /// status bar. `NavigationSplitView` does not draw through
    /// `cacheDisplay` offscreen, so the screenshots compose the chrome from
    /// the same components instead.
    static func chrome<Page: View>(_ model: MacAppModel, _ page: Page) -> some View {
        ScreenshotChrome(model: model, main: model.main!, page: page)
    }

    static func model(_ scenario: DemoScenario, _ scheme: ColorScheme) async throws -> MacAppModel {
        let launch = LaunchOptions(demoScenario: scenario, appearance: scheme == .dark ? .dark : .light, menuBarExtra: false)
        let model = MacAppModel(environment: DemoEnvironment.make(scenario: scenario), launch: launch)
        await model.start()
        await model.main?.receive?.load()
        return model
    }

    /// Lays `view` out in an offscreen window with `scheme`'s appearance,
    /// lets pending updates run, and returns its bitmap. Writes the PNG when
    /// DWD_WRITE_SCREENSHOTS=1. The first renders of a test process can come
    /// out empty while AppKit warms up, so an empty bitmap is redrawn for up
    /// to three seconds before the test fails.
    @discardableResult
    static func capture<V: View>(_ view: V, _ size: CGSize, _ scheme: ColorScheme, _ name: String) async throws
        -> NSBitmapImageRep
    {
        _ = NSApplication.shared
        let appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
        // A ScrollView that fills the hosting view draws nothing through
        // cacheDisplay; a 1 pt sibling row above it makes it draw.
        let root = VStack(spacing: 0) {
            Color(nsColor: .windowBackgroundColor).frame(height: 1)
            view
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .environment(\.colorScheme, scheme)
            .frame(width: size.width, height: size.height)
        let host = NSHostingView(rootView: root)
        host.frame = CGRect(origin: .zero, size: size)
        host.wantsLayer = true
        let window = NSWindow(
            contentRect: CGRect(origin: CGPoint(x: -20_000, y: -20_000), size: size),
            styleMask: [.titled, .fullSizeContentView], backing: .buffered, defer: false)
        window.appearance = appearance
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFrontRegardless()
        defer { window.close() }
        var rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        for attempt in 0..<30 {
            // SwiftUI flushes view updates from main run-loop observers, which
            // an async test on the main queue does not turn by itself.
            Self.spinRunLoop(0.05)
            await Task.yield()
            host.layoutSubtreeIfNeeded()
            guard attempt >= 4 else { continue }
            rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            appearance?.performAsCurrentDrawingAppearance {
                host.cacheDisplay(in: host.bounds, to: rep)
            }
            if Self.inkCoverage(rep) > 0.002 { break }
            // Views drawn only by SwiftUI's layer tree (no AppKit control
            // inside) do not draw through cacheDisplay; render the layers.
            if let layered = Self.renderLayers(of: host, appearance: appearance), Self.inkCoverage(layered) > 0.002 {
                rep = layered
                break
            }
        }
        #expect(Self.inkCoverage(rep) > 0.002, "\(name) rendered blank")
        if ProcessInfo.processInfo.environment["DWD_WRITE_SCREENSHOTS"] == "1" {
            let directory = URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent("docs/screenshots/m1", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let data = try #require(rep.representation(using: .png, properties: [:]))
            try data.write(to: directory.appendingPathComponent("\(name)-\(scheme == .dark ? "dark" : "light").png"))
        }
        return rep
    }

    /// Turns the main run loop for `seconds` (synchronous: `RunLoop.run` is
    /// unavailable from async contexts).
    static func spinRunLoop(_ seconds: TimeInterval) {
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    }

    /// `host`'s layer tree drawn with `CALayer.render(in:)` at 2x.
    static func renderLayers(of host: NSView, appearance: NSAppearance?) -> NSBitmapImageRep? {
        guard let layer = host.layer else { return nil }
        let scale = 2
        let width = Int(host.bounds.width) * scale
        let height = Int(host.bounds.height) * scale
        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: width * 4, bitsPerPixel: 32),
            let graphics = NSGraphicsContext(bitmapImageRep: rep)
        else { return nil }
        let context = graphics.cgContext
        context.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
        appearance?.performAsCurrentDrawingAppearance {
            layer.render(in: context)
        }
        return rep
    }

    /// Share of pixels (every second row and column) whose largest channel
    /// differs from the top-left pixel by more than 38/255; a blank render
    /// is close to 0.
    static func inkCoverage(_ rep: NSBitmapImageRep) -> Double {
        // Decoded from PNG: `cgImage` of a cacheDisplay bitmap can be nil.
        guard let data = rep.representation(using: .png, properties: [:]),
            let source = CGImageSourceCreateWithData(data as CFData, nil),
            let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else { return 0 }
        let width = image.width
        let height = image.height
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = pixels.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(
                data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn, width > 0, height > 0 else { return 0 }
        let background = Array(pixels[0..<4])
        var differing = 0
        var total = 0
        for y in stride(from: 0, to: height, by: 2) {
            for x in stride(from: 0, to: width, by: 2) {
                let offset = (y * width + x) * 4
                total += 1
                let delta = (0..<4).map { Int(pixels[offset + $0]) - Int(background[$0]) }.map(abs).max() ?? 0
                if delta > 38 { differing += 1 }
            }
        }
        return Double(differing) / Double(total)
    }
}

private struct ScreenshotChrome<Page: View>: View {
    let model: MacAppModel
    let main: MainViewModel
    let page: Page

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: DashSpacing.s) {
                Text(model.windowTitle).font(.headline)
                if model.isDemo { Badge(MacStrings.App.demoBadge, tone: .warning) }
                if let network = main.network, network != .mainnet {
                    Badge(L10n.Settings.networkName(network), tone: .info)
                }
                Spacer()
            }
            .padding(.horizontal, DashSpacing.l)
            .frame(height: 38)
            .background(Color.dash.secondaryBackground)
            Divider()
            HStack(spacing: 0) {
                VStack(alignment: .leading, spacing: DashSpacing.xxxs) {
                    ForEach(main.visibleSidebarItems) { item in
                        SidebarRow(title: item.title, icon: Sidebar.icon(item), isSelected: item == main.selection)
                    }
                    Spacer()
                }
                .padding(DashSpacing.s)
                .frame(width: 210)
                .background(Color.dash.secondaryBackground)
                Divider()
                page
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color.dash.primaryBackground)
            }
            WalletStatusBar(model: model, main: main)
        }
    }
}
#endif
