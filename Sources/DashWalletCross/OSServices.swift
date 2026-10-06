// The OS services this app run uses (docs/contracts/m2-swift.md §2.7):
// PlatformServicesDesktop's dw-desktop wrappers, and the clipboard each OS
// has: `wl-copy`/`xclip` on Linux, NSPasteboard in macOS development builds,
// none on Windows yet.
import CrossUI
import Foundation
import PlatformServices
import PlatformServicesDesktop
import WalletRuntime

#if os(macOS)
    import AppKit
#endif

enum AppOSServices {
    /// The desktop services for `network` (it names the autostart entry).
    static func make(network: DashNetwork) -> DesktopOSServices {
        DesktopOSServices(
            singleInstance: DesktopSingleInstance(), uriSchemes: DesktopURISchemeRegistration(),
            launchAtLogin: DesktopLaunchAtLogin(network: AppLaunchOptions.name(of: network)),
            notifications: DesktopNotifications(), biometricKeys: DesktopBiometricKeyStore(), idle: DesktopIdleMonitor(),
            clipboard: clipboard.service, qrDecoder: DesktopQRImageDecoder(),
            dataDirectories: FileSystemDataDirectoryInspector(), fileRevealer: DesktopFileRevealer())
    }

    /// The clipboard and whether it can read and write.
    static var clipboard: (service: any ClipboardProviding, available: Bool) {
        #if os(Linux)
            let clipboard = CommandLineClipboard()
            return (clipboard, clipboard.isAvailable)
        #elseif os(macOS)
            return (PasteboardClipboard(), true)
        #else
            return (NoClipboard(), false)
        #endif
    }

    /// No tray backend exists in dw-desktop yet (`DesktopTray.isAvailable`).
    @MainActor
    static func capabilities() -> CrossPlatformCapabilities {
        CrossPlatformCapabilities(clipboard: clipboard.available, tray: DesktopTray().isAvailable)
    }
}

#if os(macOS)
    /// The general pasteboard (macOS development builds of this app).
    struct PasteboardClipboard: ClipboardProviding {
        func string() -> String? {
            MainActor.assumeIsolated { NSPasteboard.general.string(forType: .string) }
        }

        func setString(_ string: String) {
            MainActor.assumeIsolated {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(string, forType: .string)
            }
        }

        func imageData() -> Data? {
            MainActor.assumeIsolated {
                NSPasteboard.general.data(forType: .png) ?? NSPasteboard.general.data(forType: .tiff)
            }
        }
    }
#endif

/// No clipboard (Windows until the toolkit's is wired): reads nothing, and
/// the screens show text instead of copying it.
struct NoClipboard: ClipboardProviding {
    func string() -> String? { nil }
    func setString(_ string: String) {}
    func imageData() -> Data? { nil }
}
