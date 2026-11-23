// The Windows/Linux OS services over dw-desktop. On macOS the dw-desktop
// calls answer `desktop.unsupported` (the Mac implementations serve the app
// there), except QR decoding, which runs everywhere; these tests check both.
// Decoding real images is tested in dw-desktop (Rust, PNG/JPEG/BMP).
import Foundation
import PlatformServices
import PlatformServicesDesktop
import Testing

struct DesktopOSServicesTests {
    @Test func IOS043_decoderErrorsKeepTheirCodes() {
        do {
            _ = try DesktopQRImageDecoder().decode(imageData: Data("nope".utf8))
            Issue.record("decoded garbage")
        } catch {
            #expect(error.code == "desktop.image_unreadable")
        }
    }

    @Test func QT004_dataDirectoryStates() async throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("dwd-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: base) }
        let inspector = FileSystemDataDirectoryInspector()
        let fresh = await inspector.inspect(base.appendingPathComponent("a/b"))
        #expect(fresh.state == .willCreate)
        #expect((fresh.availableBytes ?? 0) > 0)
        try inspector.create(base.appendingPathComponent("a/b"))
        #expect(await inspector.inspect(base.appendingPathComponent("a/b")).state == .exists)
        let file = base.appendingPathComponent("file")
        try Data().write(to: file)
        #expect(await inspector.inspect(file).state == .notADirectory)
        #expect(await inspector.inspect(file.appendingPathComponent("child")).state == .cannotCreate)
    }

    #if os(macOS)
        @Test func QT001_QT009_QT150_unsupportedOnMacOS() async throws {
            do {
                _ = try DesktopSingleInstance().claim(key: "DashWallet-regtest", arguments: [])
                Issue.record("claimed on macOS")
            } catch {
                #expect(error.code == "desktop.unsupported")
            }
            #expect(!DesktopLaunchAtLogin(network: "regtest").isSupported)
            do {
                _ = try DesktopLaunchAtLogin(network: "regtest").isEnabled()
                Issue.record("autostart on macOS")
            } catch {
                #expect(error.code == "desktop.unsupported")
            }
            do {
                try DesktopURISchemeRegistration().register(schemes: ["dash"])
                Issue.record("registered on macOS")
            } catch {
                #expect(error.code == "desktop.unsupported")
            }
            let notifications = DesktopNotifications()
            #expect(await notifications.authorization() == .unavailable)
            await #expect(throws: PlatformServiceError.self) {
                try await notifications.post(SystemNotification(id: "x", title: "t", body: "b"))
            }
            #expect(DesktopBiometricKeyStore().kind == .none)
            #expect(await !MainActor.run { DesktopTray().isAvailable })
        }
    #endif
}
