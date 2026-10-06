import DashWalletCore
import Foundation

// The dw-desktop calls (docs/contracts/m2-engine.md §2.10): free functions
// that need no engine or session. Windows and Linux use them for their OS
// services; on macOS they answer `desktop.unsupported` except
// `decodeQRCodes`, which works everywhere.

/// The OS biometric provider the Rust side offers for vault slot B.
public enum DesktopQuickUnlockProvider: Sendable, Hashable {
    case touchID, windowsHello, unavailable
}

/// An autostart entry (QT-009).
public struct DesktopAutostartEntry: Sendable, Hashable {
    public var appID: String
    public var displayName: String
    public var executable: URL
    public var arguments: [String]

    public init(appID: String, displayName: String, executable: URL, arguments: [String]) {
        self.appID = appID
        self.displayName = displayName
        self.executable = executable
        self.arguments = arguments
    }
}

public enum DesktopFunctions {
    /// Every QR code in a PNG/JPEG/BMP image, in reading order (IOS-043).
    /// `desktop.no_qr_code` / `desktop.image_unreadable` otherwise.
    public static func decodeQRCodes(_ image: Data) throws(DashKitError) -> [String] {
        try mapped { try DashWalletCore.decodeQrCodes(image: image) }
    }

    public static var quickUnlockProvider: DesktopQuickUnlockProvider {
        switch DashWalletCore.desktopQuickUnlockProvider() {
        case .touchId: .touchID
        case .windowsHello: .windowsHello
        case .unavailable: .unavailable
        }
    }

    public static func registerURISchemes(appID: String, executable: URL, schemes: [String]) throws(DashKitError) {
        try mapped {
            try DashWalletCore.registerUriSchemes(appId: appID, execPath: executable.path, schemes: schemes)
        }
    }

    public static func autostartEnabled(appID: String) throws(DashKitError) -> Bool {
        try mapped { try DashWalletCore.autostartEnabled(appId: appID) }
    }

    public static func setAutostart(_ entry: DesktopAutostartEntry, enabled: Bool) throws(DashKitError) {
        let ffi = DashWalletCore.AutostartEntry(
            appId: entry.appID, displayName: entry.displayName, execPath: entry.executable.path,
            args: entry.arguments)
        try mapped { try DashWalletCore.setAutostart(entry: ffi, enabled: enabled) }
    }

    /// Windows capture exclusion of window `hwnd`; whether it took effect.
    public static func setWindowCaptureExcluded(hwnd: UInt64, excluded: Bool) throws(DashKitError) -> Bool {
        try mapped { try DashWalletCore.setWindowCaptureExcluded(hwnd: hwnd, excluded: excluded) }
    }

    /// Becomes the primary for `key`; `nil` when another process is.
    /// `onForwarded` runs on a dw-desktop thread for each later launch.
    public static func acquireSingleInstance(
        key: String, onForwarded: @escaping @Sendable ([String]) -> Void
    ) throws(DashKitError) -> DesktopInstanceGuard? {
        let observer = InstanceObserverAdapter(onForwarded)
        return try mapped { try DashWalletCore.acquireSingleInstance(key: key, observer: observer) }
            .map(DesktopInstanceGuard.init)
    }

    /// Sends `arguments` to the primary for `key`; `false` when none runs.
    public static func forwardToPrimary(key: String, arguments: [String]) throws(DashKitError) -> Bool {
        try mapped { try DashWalletCore.forwardToPrimary(key: key, args: arguments) }
    }
}

/// Held by the primary instance; `release()` (or deinit) frees the key.
public final class DesktopInstanceGuard: Sendable {
    private let guardObject: DashWalletCore.InstanceGuard

    init(_ guardObject: DashWalletCore.InstanceGuard) {
        self.guardObject = guardObject
    }

    public func release() {
        guardObject.release()
    }
}

private final class InstanceObserverAdapter: DashWalletCore.InstanceObserver, @unchecked Sendable {
    private let body: @Sendable ([String]) -> Void

    init(_ body: @escaping @Sendable ([String]) -> Void) {
        self.body = body
    }

    func onForwarded(args: [String]) {
        body(args)
    }
}

/// Desktop notifications through dw-desktop (Linux `notify-send`, Windows
/// toast). `desktop.unsupported` when the session cannot show them.
public final class DesktopNotifierClient: Sendable {
    private let notifier: DashWalletCore.DesktopNotifier

    /// `onActivated(id, deepLink)` runs on a dw-desktop thread.
    public init(appID: String, onActivated: @escaping @Sendable (String, String?) -> Void) throws(DashKitError) {
        let observer = NotificationObserverAdapter(onActivated)
        notifier = try mapped { try DashWalletCore.DesktopNotifier(appId: appID, observer: observer) }
    }

    public func notify(id: String, title: String, body: String, deepLink: String?) throws(DashKitError) {
        let note = DashWalletCore.DesktopNotification(id: id, title: title, body: body, deepLink: deepLink)
        try mapped { try notifier.notify(notification: note) }
    }
}

private final class NotificationObserverAdapter: DashWalletCore.NotificationObserver, @unchecked Sendable {
    private let body: @Sendable (String, String?) -> Void

    init(_ body: @escaping @Sendable (String, String?) -> Void) {
        self.body = body
    }

    func onActivated(id: String, deepLink: String?) {
        body(id, deepLink)
    }
}
