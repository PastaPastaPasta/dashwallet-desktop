// M2 OS-service protocols (docs/contracts/m2-swift.md §2.10, owner S1).
// macOS implements them in PlatformServicesMac (AppKit, UserNotifications,
// LocalAuthentication, Security); Windows and Linux in
// PlatformServicesDesktop over the dw-desktop calls of dw-ffi (desktop.rs).
// Foundation only: these types cannot see WalletRuntime.
import Foundation

/// An OS-service failure. `code` uses the engine's `desktop.*` codes
/// (m2-engine.md §4) plus `platform.denied` (the user or OS refused) and
/// `platform.cancelled` (the user dismissed a prompt); WalletRuntime maps it
/// to `ServiceError` with the same code.
public struct PlatformServiceError: Error, Sendable, Equatable {
    public let code: String
    public let detail: String

    public init(code: String, detail: String = "") {
        self.code = code
        self.detail = detail
    }

    public static func unsupported(_ feature: String) -> Self {
        Self(code: "desktop.unsupported", detail: feature)
    }
}

// MARK: Single instance and URIs (QT-001, QT-019, QT-150, IOS-048)

/// What `claim` found.
public enum SingleInstanceRole: Sendable, Hashable {
    /// This process is the primary; forwarded arguments arrive on
    /// `forwardedArguments()`.
    case primary
    /// Another instance took the arguments; this process exits 0.
    case forwarded
}

/// macOS: LaunchServices already keeps one instance and delivers URLs
/// (`application(_:open:)`), so `claim` returns `.primary` and URLs come
/// through `forwardedArguments()`. Windows/Linux: dw-desktop local socket.
public protocol SingleInstanceCoordinating: AnyObject, Sendable {
    /// `key` = `DashWallet-<network>`; `arguments` are forwarded when another
    /// instance holds the key.
    func claim(key: String, arguments: [String]) throws(PlatformServiceError) -> SingleInstanceRole
    func forwardedArguments() -> AsyncStream<[String]>
}

/// Per-user scheme registration where no installer did it (Linux tarball).
public protocol URISchemeRegistering: Sendable {
    var registeredAtInstall: Bool { get }
    func register(schemes: [String]) throws(PlatformServiceError)
}

// MARK: Autostart (QT-009)

public protocol LaunchAtLoginManaging: Sendable {
    /// `false` on macOS (dash-qt hides the option there).
    var isSupported: Bool { get }
    func isEnabled() throws(PlatformServiceError) -> Bool
    /// Starts with `--min` and the active network.
    func setEnabled(_ enabled: Bool, arguments: [String]) throws(PlatformServiceError)
}

// MARK: Tray (QT-028…030, IOS-117)

public struct TrayMenuEntry: Sendable, Hashable, Identifiable {
    public let id: String
    public let title: String
    public let enabled: Bool
    public let isSeparator: Bool

    public init(id: String, title: String, enabled: Bool = true, isSeparator: Bool = false) {
        self.id = id
        self.title = title
        self.enabled = enabled
        self.isSeparator = isSeparator
    }

    public static func separator(_ id: String) -> Self {
        Self(id: id, title: "", enabled: false, isSeparator: true)
    }
}

public enum TrayEvent: Sendable, Hashable {
    /// Left click: show or hide the main window (Windows/Linux).
    case activated
    case menuItem(id: String)
}

/// Windows/Linux tray icon. macOS uses SwiftUI `MenuBarExtra` and Dock menu
/// instead, so `isAvailable` is false there.
@MainActor
public protocol TrayControlling: AnyObject {
    /// `false` without a tray host (GNOME without AppIndicator).
    var isAvailable: Bool { get }
    func show(tooltip: String, iconPNG: Data, menu: [TrayMenuEntry]) throws(PlatformServiceError)
    func update(tooltip: String?, menu: [TrayMenuEntry]?) throws(PlatformServiceError)
    func setVisible(_ visible: Bool)
    func events() -> AsyncStream<TrayEvent>
}

// MARK: Notifications (QT-031…033, IOS-105, IOS-116)

public enum NotificationAuthorization: Sendable, Hashable {
    case notDetermined, denied, authorized
    /// The OS has no notification service reachable (no D-Bus daemon).
    case unavailable
}

public struct SystemNotification: Sendable, Hashable {
    public let id: String
    public let title: String
    public let body: String
    /// Route opened when the user clicks it (e.g. a transaction).
    public let deepLink: String?

    public init(id: String, title: String, body: String, deepLink: String? = nil) {
        self.id = id
        self.title = title
        self.body = body
        self.deepLink = deepLink
    }
}

public protocol SystemNotifying: AnyObject, Sendable {
    func authorization() async -> NotificationAuthorization
    /// Asks once; returns the resulting state.
    func requestAuthorization() async -> NotificationAuthorization
    func post(_ notification: SystemNotification) async throws(PlatformServiceError)
    /// Deep links of clicked notifications.
    func activations() -> AsyncStream<String?>
}

// MARK: Quick unlock key store (IOS-011)

/// A secret released by the OS biometric store, in one owned allocation
/// that is zeroed on deinit (as DashKit `SecretBytes`). WalletRuntime treats
/// it as a `SecretBuffer`. Immutable after init.
public final class BiometricKey: Sendable {
    private nonisolated(unsafe) let storage: UnsafeMutableRawBufferPointer

    /// Copies `bytes`; the caller zeroes its own copy.
    public init(copying bytes: UnsafeRawBufferPointer) {
        storage = .allocate(byteCount: max(bytes.count, 1), alignment: 1)
        if let source = bytes.baseAddress, bytes.count > 0 {
            storage.baseAddress!.copyMemory(from: source, byteCount: bytes.count)
        }
        count = bytes.count
    }

    deinit {
        Self.zero(storage)
        storage.deallocate()
    }

    public let count: Int

    public func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try body(UnsafeRawBufferPointer(rebasing: storage[0..<count]))
    }

    @inline(never)
    private static func zero(_ buffer: UnsafeMutableRawBufferPointer) {
        guard let base = buffer.baseAddress, buffer.count > 0 else { return }
        #if canImport(Darwin)
        _ = memset_s(base, buffer.count, 0, buffer.count)
        #else
        unoptimizedZero(base, buffer.count)
        #endif
    }
}

#if !canImport(Darwin)
@inline(never)
@_optimize(none)
private func unoptimizedZero(_ base: UnsafeMutableRawPointer, _ count: Int) {
    let bytes = base.assumingMemoryBound(to: UInt8.self)
    for i in 0..<count {
        bytes[i] = 0
    }
}
#endif

public enum BiometricKind: Sendable, Hashable {
    case touchID, windowsHello, none
}

/// Holds the vault's slot B wrap key behind biometrics: macOS keychain item
/// with `.biometryCurrentSet`; Windows Hello (M6). Keyed per network.
public protocol BiometricKeyStoring: Sendable {
    var kind: BiometricKind { get }
    func store(_ key: BiometricKey, network: String) throws(PlatformServiceError)
    /// Shows the biometric prompt with `reason`; `platform.cancelled` when
    /// dismissed.
    func retrieve(network: String, reason: String) async throws(PlatformServiceError) -> BiometricKey
    func delete(network: String) throws(PlatformServiceError)
}

// MARK: Activity, clipboard, QR images, files

public enum IdleEvent: Sendable, Hashable {
    case systemWillSleep
    case screenLocked
    case userIdle(seconds: Int)
}

/// Auto-lock triggers (IOS-015).
public protocol IdleMonitoring: Sendable {
    func events() -> AsyncStream<IdleEvent>
}

public protocol ClipboardProviding: Sendable {
    func string() -> String?
    func setString(_ string: String)
    /// PNG/TIFF bytes of an image on the clipboard (QR from clipboard, IOS-043).
    func imageData() -> Data?
}

/// QR codes in an image file or clipboard image (IOS-043; camera is M6).
public protocol QRImageDecoding: Sendable {
    /// Every code found, in reading order; `desktop.no_qr_code` when none.
    func decode(imageData: Data) throws(PlatformServiceError) -> [String]
}

/// QT-004 first-run chooser checks.
public enum DataDirectoryState: Sendable, Hashable {
    /// "A new data directory will be created."
    case willCreate
    /// "Directory already exists. Add /name if you intend to create a new directory here."
    case exists
    /// "Path already exists, and is not a directory."
    case notADirectory
    /// "Cannot create data directory here."
    case cannotCreate
}

public struct DataDirectoryStatus: Sendable, Hashable {
    public let state: DataDirectoryState
    /// Free bytes on the volume; `nil` when unknown.
    public let availableBytes: Int64?

    public init(state: DataDirectoryState, availableBytes: Int64?) {
        self.state = state
        self.availableBytes = availableBytes
    }
}

public protocol DataDirectoryInspecting: Sendable {
    func inspect(_ url: URL) async -> DataDirectoryStatus
    /// Creates the directory (and its parents) with user-only permissions.
    func create(_ url: URL) throws(PlatformServiceError)
}

/// "Open data folder", "Show automatic backups", "Open debug log".
public protocol FileRevealing: Sendable {
    func reveal(_ url: URL) throws(PlatformServiceError)
}
