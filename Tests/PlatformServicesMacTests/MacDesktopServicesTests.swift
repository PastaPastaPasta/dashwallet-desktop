// macOS OS services. The keychain round trip needs Touch ID hardware and a
// signed app with the data-protection keychain entitlement; it skips
// itself otherwise and says why.
#if os(macOS)
import AppKit
import CoreImage
import Foundation
import PlatformServices
import PlatformServicesMac
import Testing

private func qrPNG(_ texts: [String]) -> Data {
    let tiles = texts.map { text -> CIImage in
        let filter = CIFilter(name: "CIQRCodeGenerator")!
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        return filter.outputImage!.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
    }
    let tile = tiles[0].extent.width + 64
    let size = NSSize(width: tile * CGFloat(tiles.count), height: tile)
    let image = NSImage(size: size)
    image.lockFocus()
    NSColor.white.setFill()
    NSRect(origin: .zero, size: size).fill()
    let context = CIContext(cgContext: NSGraphicsContext.current!.cgContext, options: nil)
    for (i, ci) in tiles.enumerated() {
        context.draw(ci, in: CGRect(x: CGFloat(i) * tile + 32, y: 32, width: ci.extent.width, height: ci.extent.height), from: ci.extent)
    }
    image.unlockFocus()
    let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
    return rep.representation(using: .png, properties: [:])!
}

struct MacDesktopServicesTests {
    @Test func IOS043_visionReadsCodesInReadingOrder() throws {
        let png = qrPNG(["dash:first", "dash:second"])
        #expect(try MacQRImageDecoder().decode(imageData: png) == ["dash:first", "dash:second"])
        let blank = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: 32, pixelsHigh: 32, bitsPerSample: 8, samplesPerPixel: 1, hasAlpha: false,
            isPlanar: false, colorSpaceName: .deviceWhite, bytesPerRow: 0, bitsPerPixel: 0)!
        #expect(throws: PlatformServiceError(code: "desktop.no_qr_code")) {
            try MacQRImageDecoder().decode(imageData: blank.representation(using: .png, properties: [:])!)
        }
        do {
            _ = try MacQRImageDecoder().decode(imageData: Data("x".utf8))
            Issue.record("decoded garbage")
        } catch {
            #expect(error.code == "desktop.image_unreadable")
        }
    }

    @Test func QT001_launchServicesDeliversOpenedURLs() async throws {
        let instance = MacSingleInstance()
        #expect(try instance.claim(key: "DashWallet-mainnet", arguments: []) == .primary)
        // A cold launch by URL arrives before anyone listens.
        instance.deliver(urls: [URL(string: "dash:yX?amount=1")!])
        var iterator = instance.forwardedArguments().makeAsyncIterator()
        #expect(await iterator.next() == ["dash:yX?amount=1"])
        instance.deliver(urls: [URL(fileURLWithPath: "/tmp/wallet.dwbackup")])
        #expect(await iterator.next() == ["/tmp/wallet.dwbackup"])
    }

    @Test func QT009_QT150_macPolicies() {
        #expect(MacURISchemeRegistration().registeredAtInstall)
        #expect(!MacLaunchAtLogin().isSupported)
        #expect(throws: PlatformServiceError.self) { try MacLaunchAtLogin().isEnabled() }
    }

    @Test func IOS105_notificationsOutsideABundleAreUnavailable() async {
        let notifier = MacNotifier(inBundle: false)
        #expect(await notifier.authorization() == .unavailable)
        await #expect(throws: PlatformServiceError.unsupported("notifications outside an app bundle")) {
            try await notifier.post(SystemNotification(id: "x", title: "t", body: "b"))
        }
    }

    /// Store, prompt, read and delete a key. Needs a person at the machine
    /// for the Touch ID prompt, so it also needs `DWD_BIOMETRIC_TEST=1`.
    @Test func IOS011_keychainRoundTripWhenTouchIDExists() async throws {
        // Keychain writes can raise a system dialog on the developer's Mac,
        // so the whole test is opt-in.
        guard ProcessInfo.processInfo.environment["DWD_BIOMETRIC_TEST"] == "1" else {
            print("skipped: set DWD_BIOMETRIC_TEST=1 to run the Keychain and Touch ID round trip")
            return
        }
        let store = MacBiometricKeyStore(service: "org.dashfoundation.DashWallet.tests.\(UUID().uuidString)")
        guard store.kind == .touchID else {
            print("skipped: no Touch ID on this machine")
            return
        }
        let key = [UInt8](repeating: 0x5a, count: 32).withUnsafeBytes { BiometricKey(copying: $0) }
        do {
            try store.store(key, network: "regtest")
        } catch {
            // `swift test` is not a signed app: the data-protection keychain
            // refuses it (errSecMissingEntitlement).
            #expect(error.code == "desktop.os_error")
            print("skipped: keychain refused the item (\(error.detail))")
            return
        }
        defer { try? store.delete(network: "regtest") }
        let read = try await store.retrieve(network: "regtest", reason: "dashwallet-desktop test")
        #expect(read.withUnsafeBytes { Array($0) } == [UInt8](repeating: 0x5a, count: 32))
        try store.delete(network: "regtest")
        await #expect(throws: PlatformServiceError.self) {
            _ = try await store.retrieve(network: "regtest", reason: "x")
        }
    }

    @Test func BiometricKeyCopiesAndZeroesItsBytes() {
        var source: [UInt8] = [1, 2, 3]
        let key = source.withUnsafeBytes { BiometricKey(copying: $0) }
        source = [0, 0, 0]
        #expect(key.count == 3)
        #expect(key.withUnsafeBytes { Array($0) } == [1, 2, 3])
    }
}
#endif
