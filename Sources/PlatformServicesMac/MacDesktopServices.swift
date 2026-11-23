// macOS implementations of the M2 OS-service protocols
// (docs/contracts/m2-swift.md §2.7): single instance and URIs through
// LaunchServices, login item, status item, idle events, clipboard, QR
// images (Vision) and file reveal. Notifications and the biometric key
// store are in their own files.
#if os(macOS)
import AppKit
import Foundation
import PlatformServices
import ServiceManagement
import Vision

// MARK: Single instance and URIs (QT-001, QT-150, IOS-048)

/// LaunchServices keeps one instance per app bundle and hands later opens
/// to `NSApplicationDelegate.application(_:open:)`. `claim` therefore always
/// answers `.primary`; the app delegate passes opened URLs to `deliver`,
/// and they come out of `forwardedArguments()` like a forwarded launch on
/// the other OSes.
public final class MacSingleInstance: SingleInstanceCoordinating, @unchecked Sendable {
    // `lock` guards `continuations` and `backlog`.
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<[String]>.Continuation] = [:]
    /// URLs delivered before anyone listened (a cold launch by URL).
    private var backlog: [[String]] = []

    public init() {}

    public func claim(key: String, arguments: [String]) throws(PlatformServiceError) -> SingleInstanceRole {
        .primary
    }

    public func forwardedArguments() -> AsyncStream<[String]> {
        let (stream, continuation) = AsyncStream<[String]>.makeStream()
        let id = UUID()
        let pending = lock.withLock {
            continuations[id] = continuation
            defer { backlog.removeAll() }
            return backlog
        }
        for arguments in pending { continuation.yield(arguments) }
        continuation.onTermination = { [weak self] _ in
            self?.lock.withLock { _ = self?.continuations.removeValue(forKey: id) }
        }
        return stream
    }

    /// Call from `application(_:open:)` and for files dropped on the Dock icon.
    public func deliver(urls: [URL]) {
        let arguments = urls.map { $0.isFileURL ? $0.path : $0.absoluteString }
        guard !arguments.isEmpty else { return }
        let targets = lock.withLock {
            if continuations.isEmpty { backlog.append(arguments) }
            return Array(continuations.values)
        }
        for continuation in targets { continuation.yield(arguments) }
    }
}

/// The bundle's `CFBundleURLTypes` registers the schemes when the app is
/// installed; there is nothing to do at run time.
public struct MacURISchemeRegistration: URISchemeRegistering {
    public init() {}

    public var registeredAtInstall: Bool { true }

    public func register(schemes: [String]) throws(PlatformServiceError) {
        throw .unsupported("URI schemes at run time (Info.plist registers them)")
    }
}

// MARK: Login item (QT-009)

/// dash-qt hides "Start on system login" on macOS, so the app builds this
/// with `offered: false` and the option stays hidden. With `offered: true`
/// it uses `SMAppService.mainApp` (macOS 13+), which needs a signed app
/// bundle; login items start without arguments, so `--min` cannot be
/// passed (the app would read its own setting instead).
public struct MacLaunchAtLogin: LaunchAtLoginManaging {
    private let offered: Bool

    public init(offered: Bool = false) {
        self.offered = offered
    }

    public var isSupported: Bool { offered }

    public func isEnabled() throws(PlatformServiceError) -> Bool {
        guard offered else { throw .unsupported("start on login (hidden on macOS, as dash-qt)") }
        return SMAppService.mainApp.status == .enabled
    }

    public func setEnabled(_ enabled: Bool, arguments: [String]) throws(PlatformServiceError) {
        guard offered else { throw .unsupported("start on login (hidden on macOS, as dash-qt)") }
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            throw PlatformServiceError(code: "desktop.os_error", detail: error.localizedDescription)
        }
    }
}

// MARK: Status item (QT-028…030, IOS-117)

/// `TrayControlling` over an `NSStatusItem` with a menu. The macOS app uses
/// SwiftUI `MenuBarExtra` and the Dock menu (m2-swift.md §2.7); this is for
/// an AppKit-hosted window or tests. Clicking the item opens the menu, so
/// `.activated` is not sent.
@MainActor
public final class MacStatusItemTray: NSObject, TrayControlling {
    private var item: NSStatusItem?
    private var continuations: [UUID: AsyncStream<TrayEvent>.Continuation] = [:]

    public override init() {}

    public var isAvailable: Bool { true }

    public func show(tooltip: String, iconPNG: Data, menu: [TrayMenuEntry]) throws(PlatformServiceError) {
        guard let image = NSImage(data: iconPNG) else {
            throw PlatformServiceError(code: "desktop.image_unreadable", detail: "status item icon")
        }
        image.isTemplate = true
        let item = item ?? NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = image
        item.button?.toolTip = tooltip
        item.menu = makeMenu(menu)
        item.isVisible = true
        self.item = item
    }

    public func update(tooltip: String?, menu: [TrayMenuEntry]?) throws(PlatformServiceError) {
        guard let item else { throw PlatformServiceError(code: "desktop.os_error", detail: "status item not shown") }
        if let tooltip { item.button?.toolTip = tooltip }
        if let menu { item.menu = makeMenu(menu) }
    }

    public func setVisible(_ visible: Bool) {
        item?.isVisible = visible
    }

    public func events() -> AsyncStream<TrayEvent> {
        let (stream, continuation) = AsyncStream<TrayEvent>.makeStream()
        let id = UUID()
        continuations[id] = continuation
        continuation.onTermination = { [weak self] _ in
            Task { @MainActor in self?.continuations[id] = nil }
        }
        return stream
    }

    private func makeMenu(_ entries: [TrayMenuEntry]) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false
        for entry in entries {
            if entry.isSeparator {
                menu.addItem(.separator())
                continue
            }
            let item = NSMenuItem(title: entry.title, action: #selector(selected(_:)), keyEquivalent: "")
            item.target = self
            item.isEnabled = entry.enabled
            item.representedObject = entry.id
            menu.addItem(item)
        }
        return menu
    }

    @objc private func selected(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        for continuation in continuations.values { continuation.yield(.menuItem(id: id)) }
    }
}

// MARK: Idle events (IOS-015)

/// Sleep (`NSWorkspace.willSleepNotification`), screen lock
/// (`com.apple.screenIsLocked`, and displays going to sleep) and the time
/// since the last input in the login session, polled every `pollInterval`
/// and reported once it reaches a minute.
public final class MacIdleMonitor: IdleMonitoring {
    private let pollInterval: Duration

    public init(pollInterval: Duration = .seconds(30)) {
        self.pollInterval = pollInterval
    }

    public func events() -> AsyncStream<IdleEvent> {
        let (stream, continuation) = AsyncStream<IdleEvent>.makeStream(bufferingPolicy: .bufferingNewest(8))
        let workspace = NSWorkspace.shared.notificationCenter
        let distributed = DistributedNotificationCenter.default()
        let tokens: [(NotificationCenter, NSObjectProtocol)] = [
            (workspace, workspace.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: nil) { _ in
                continuation.yield(.systemWillSleep)
            }),
            (workspace, workspace.addObserver(forName: NSWorkspace.screensDidSleepNotification, object: nil, queue: nil) { _ in
                continuation.yield(.screenLocked)
            }),
            (distributed, distributed.addObserver(
                forName: Notification.Name("com.apple.screenIsLocked"), object: nil, queue: nil
            ) { _ in
                continuation.yield(.screenLocked)
            }),
        ]
        let poll = pollInterval
        let poller = Task {
            while !Task.isCancelled {
                try? await Task.sleep(for: poll)
                // `~0` is kCGAnyInputEventType.
                guard let anyInput = CGEventType(rawValue: ~0) else { continue }
                let idle = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: anyInput)
                if idle >= 60 { continuation.yield(.userIdle(seconds: Int(idle))) }
            }
        }
        let observers = UncheckedObservers(tokens)
        continuation.onTermination = { _ in
            poller.cancel()
            observers.remove()
        }
        return stream
    }
}

/// Notification observer tokens removed when the idle stream ends.
private final class UncheckedObservers: @unchecked Sendable {
    private let tokens: [(NotificationCenter, NSObjectProtocol)]

    init(_ tokens: [(NotificationCenter, NSObjectProtocol)]) {
        self.tokens = tokens
    }

    func remove() {
        for (center, token) in tokens { center.removeObserver(token) }
    }
}

// MARK: Clipboard (QT-090, IOS-043)

/// The general pasteboard: plain text, and PNG or TIFF image data.
public struct MacClipboard: ClipboardProviding {
    public init() {}

    public func string() -> String? {
        NSPasteboard.general.string(forType: .string)
    }

    public func setString(_ string: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(string, forType: .string)
    }

    public func imageData() -> Data? {
        let pasteboard = NSPasteboard.general
        return pasteboard.data(forType: .png) ?? pasteboard.data(forType: .tiff)
    }
}

// MARK: QR images (IOS-043)

/// QR codes in any image format ImageIO reads (PNG, JPEG, TIFF, HEIC, …)
/// with Vision, in reading order (top to bottom, then left to right).
public struct MacQRImageDecoder: QRImageDecoding {
    public static let maximumBytes = 32 * 1024 * 1024

    public init() {}

    public func decode(imageData: Data) throws(PlatformServiceError) -> [String] {
        guard imageData.count <= Self.maximumBytes else {
            throw PlatformServiceError(code: "desktop.image_unreadable", detail: "\(imageData.count) bytes")
        }
        guard NSImage(data: imageData) != nil else {
            throw PlatformServiceError(code: "desktop.image_unreadable", detail: "not an image ImageIO reads")
        }
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.qr]
        do {
            try VNImageRequestHandler(data: imageData, options: [:]).perform([request])
        } catch {
            throw PlatformServiceError(code: "desktop.image_unreadable", detail: error.localizedDescription)
        }
        // Vision's origin is bottom-left: higher maxY is nearer the top.
        let codes = (request.results ?? [])
            .compactMap { observation -> (CGRect, String)? in
                observation.payloadStringValue.map { (observation.boundingBox, $0) }
            }
            .sorted { a, b in
                abs(a.0.maxY - b.0.maxY) > 0.02 ? a.0.maxY > b.0.maxY : a.0.minX < b.0.minX
            }
            .map(\.1)
        guard !codes.isEmpty else { throw PlatformServiceError(code: "desktop.no_qr_code") }
        return codes
    }
}

// MARK: Files (QT-116, QT-143)

/// Selects the item in a Finder window.
public struct MacFileRevealer: FileRevealing {
    public init() {}

    public func reveal(_ url: URL) throws(PlatformServiceError) {
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw PlatformServiceError(code: "desktop.os_error", detail: "\(url.path) does not exist")
        }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
}
#endif
