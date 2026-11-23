// Windows/Linux implementations of the M2 OS-service protocols over the
// dw-desktop calls (docs/contracts/m2-engine.md §2.10, m2-swift.md §2.7).
//
// Compiled on every OS so the macOS build type-checks them and the tests
// run; on macOS the dw-desktop calls answer `desktop.unsupported` except
// QR decoding. Windows paths are written but not built or run (no Windows
// runner): UNVERIFIED there.
import DashKit
import Foundation
import PlatformServices

extension PlatformServiceError {
    /// The DashKit error of a dw-desktop call, with its code.
    init(_ error: DashKitError) {
        self.init(code: error.code, detail: error.detail)
    }
}

/// Runs a dw-desktop call and maps its error.
func desktopCall<T>(_ body: () throws(DashKitError) -> T) throws(PlatformServiceError) -> T {
    do {
        return try body()
    } catch {
        throw PlatformServiceError(error)
    }
}

/// The reverse-DNS app id the Linux `.desktop` files and the Windows
/// registry values use.
public enum DesktopAppIdentity {
    public static let appID = "org.dashfoundation.DashWallet"
    public static let displayName = "Dash Wallet"
    /// The schemes of DESIGN-opus §1.13.
    public static let uriSchemes = ["dash", "pay", "dashwallet", "dashpay", "dashid", "dash-key", "dash-st"]

    /// `true` inside Flatpak, where the manifest registers schemes and the
    /// Background portal (not implemented) owns autostart.
    public static var isFlatpak: Bool {
        ProcessInfo.processInfo.environment["FLATPAK_ID"] != nil
            || FileManager.default.fileExists(atPath: "/.flatpak-info")
    }

    /// The running executable.
    public static var executable: URL {
        URL(fileURLWithPath: CommandLine.arguments.first ?? "", isDirectory: false).standardizedFileURL
    }
}

// MARK: Single instance (QT-001)

/// `SingleInstanceCoordinating` over dw-desktop: Linux a lock file and
/// socket in `$XDG_RUNTIME_DIR`, Windows a named mutex and pipe. The guard
/// lives as long as this object.
public final class DesktopSingleInstance: SingleInstanceCoordinating, @unchecked Sendable {
    // `lock` guards `guardObject`, `continuations` and `backlog`.
    private let lock = NSLock()
    private var guardObject: DesktopInstanceGuard?
    private var continuations: [UUID: AsyncStream<[String]>.Continuation] = [:]
    private var backlog: [[String]] = []

    public init() {}

    deinit {
        guardObject?.release()
    }

    /// `.forwarded` when another instance took `arguments` (the caller
    /// exits 0). When the other instance stops between the two steps, this
    /// one tries once more to become primary.
    public func claim(key: String, arguments: [String]) throws(PlatformServiceError) -> SingleInstanceRole {
        for _ in 0..<2 {
            let acquired = try desktopCall { () throws(DashKitError) in
                try DesktopFunctions.acquireSingleInstance(key: key) { [weak self] forwarded in
                    self?.deliver(forwarded)
                }
            }
            if let acquired {
                lock.withLock { guardObject = acquired }
                return .primary
            }
            let delivered = try desktopCall { () throws(DashKitError) in
                try DesktopFunctions.forwardToPrimary(key: key, arguments: arguments)
            }
            if delivered { return .forwarded }
        }
        throw PlatformServiceError(code: "desktop.os_error", detail: "another instance holds \(key) but did not answer")
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

    /// Frees the key (at quit).
    public func release() {
        let held = lock.withLock {
            defer { guardObject = nil }
            return guardObject
        }
        held?.release()
    }

    private func deliver(_ arguments: [String]) {
        let targets = lock.withLock {
            if continuations.isEmpty { backlog.append(arguments) }
            return Array(continuations.values)
        }
        for continuation in targets { continuation.yield(arguments) }
    }
}

// MARK: URI schemes (QT-150, IOS-048)

/// Per-user registration for the Linux tarball and an unpackaged Windows
/// build; Flatpak and the MSI register at install time.
public struct DesktopURISchemeRegistration: URISchemeRegistering {
    private let executable: URL

    public init(executable: URL = DesktopAppIdentity.executable) {
        self.executable = executable
    }

    public var registeredAtInstall: Bool { DesktopAppIdentity.isFlatpak }

    public func register(schemes: [String]) throws(PlatformServiceError) {
        let executable = executable
        try desktopCall { () throws(DashKitError) in
            try DesktopFunctions.registerURISchemes(
                appID: DesktopAppIdentity.appID, executable: executable, schemes: schemes)
        }
    }
}

// MARK: Autostart (QT-009)

/// Linux XDG autostart entry / Windows `Run` value, one per network
/// (`<app id>-<network>`), started with dash-qt's `--min` plus the
/// arguments the caller passes (the network). Not offered inside Flatpak.
public struct DesktopLaunchAtLogin: LaunchAtLoginManaging {
    private let appID: String
    private let executable: URL

    /// - Parameter network: names the entry, so each network has its own.
    public init(network: String, executable: URL = DesktopAppIdentity.executable) {
        appID = "\(DesktopAppIdentity.appID)-\(network)"
        self.executable = executable
    }

    public var isSupported: Bool {
        #if os(Linux) || os(Windows)
            !DesktopAppIdentity.isFlatpak
        #else
            false
        #endif
    }

    public func isEnabled() throws(PlatformServiceError) -> Bool {
        let appID = appID
        return try desktopCall { () throws(DashKitError) in try DesktopFunctions.autostartEnabled(appID: appID) }
    }

    public func setEnabled(_ enabled: Bool, arguments: [String]) throws(PlatformServiceError) {
        let entry = DesktopAutostartEntry(
            appID: appID, displayName: DesktopAppIdentity.displayName, executable: executable,
            arguments: ["--min"] + arguments.filter { $0 != "--min" && $0 != "-min" })
        try desktopCall { () throws(DashKitError) in try DesktopFunctions.setAutostart(entry, enabled: enabled) }
    }
}

// MARK: Tray (QT-028…030, IOS-117)

/// dw-desktop has no tray backend yet (the StatusNotifierItem needs a
/// D-Bus stack, the Windows icon a message loop), so there is no tray: the
/// options that need one stay hidden and the window is the only surface,
/// as on a GNOME session without the AppIndicator extension.
@MainActor
public final class DesktopTray: TrayControlling {
    public init() {}

    public var isAvailable: Bool { false }

    public func show(tooltip: String, iconPNG: Data, menu: [TrayMenuEntry]) throws(PlatformServiceError) {
        throw .unsupported("tray icon (no StatusNotifierItem / Shell_NotifyIcon backend yet)")
    }

    public func update(tooltip: String?, menu: [TrayMenuEntry]?) throws(PlatformServiceError) {
        throw .unsupported("tray icon (no StatusNotifierItem / Shell_NotifyIcon backend yet)")
    }

    public func setVisible(_ visible: Bool) {}

    public func events() -> AsyncStream<TrayEvent> {
        AsyncStream { $0.finish() }
    }
}

// MARK: Notifications (QT-031, IOS-105, IOS-116)

/// `SystemNotifying` over dw-desktop's notifier: Linux `notify-send`,
/// Windows toasts. Without one (no `notify-send`, macOS)
/// `authorization()` is `.unavailable` and `post` fails with
/// `desktop.unsupported`. There is no permission prompt on these OSes, so
/// an available notifier is `.authorized`.
public final class DesktopNotifications: SystemNotifying, @unchecked Sendable {
    private let client: DesktopNotifierClient?
    private let unavailableReason: String
    // `lock` guards `continuations`.
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<String?>.Continuation] = [:]

    public init(appID: String = DesktopAppIdentity.appID) {
        var reason = ""
        var made: DesktopNotifierClient?
        let relay = ActivationRelay()
        do {
            made = try DesktopNotifierClient(appID: appID) { _, link in relay.send(link) }
        } catch {
            reason = error.detail
        }
        client = made
        unavailableReason = reason
        relay.target = self
    }

    public func authorization() async -> NotificationAuthorization {
        client == nil ? .unavailable : .authorized
    }

    public func requestAuthorization() async -> NotificationAuthorization {
        await authorization()
    }

    public func post(_ notification: SystemNotification) async throws(PlatformServiceError) {
        guard let client else { throw .unsupported("notifications (\(unavailableReason))") }
        try desktopCall { () throws(DashKitError) in
            try client.notify(
                id: notification.id, title: notification.title, body: notification.body,
                deepLink: notification.deepLink)
        }
    }

    public func activations() -> AsyncStream<String?> {
        let (stream, continuation) = AsyncStream<String?>.makeStream()
        let id = UUID()
        lock.withLock { continuations[id] = continuation }
        continuation.onTermination = { [weak self] _ in
            self?.lock.withLock { _ = self?.continuations.removeValue(forKey: id) }
        }
        return stream
    }

    fileprivate func activated(_ link: String?) {
        let targets = lock.withLock { Array(continuations.values) }
        for continuation in targets { continuation.yield(link) }
    }
}

/// Forwards clicks from the dw-desktop thread to the notifier, which does
/// not exist yet when the Rust observer is created.
private final class ActivationRelay: @unchecked Sendable {
    // `lock` guards `target`.
    private let lock = NSLock()
    private weak var _target: DesktopNotifications?

    var target: DesktopNotifications? {
        get { lock.withLock { _target } }
        set { lock.withLock { _target = newValue } }
    }

    func send(_ link: String?) {
        target?.activated(link)
    }
}

// MARK: Biometric key store (IOS-011)

/// Windows Hello lands in M6 and Linux has no biometric store, so quick
/// unlock is hidden: `kind` is `.none` and every call is unsupported.
public struct DesktopBiometricKeyStore: BiometricKeyStoring {
    public init() {}

    public var kind: BiometricKind { .none }

    public func store(_ key: BiometricKey, network: String) throws(PlatformServiceError) {
        throw .unsupported("quick unlock (Windows Hello is M6; Linux has none)")
    }

    public func retrieve(network: String, reason: String) async throws(PlatformServiceError) -> BiometricKey {
        throw .unsupported("quick unlock (Windows Hello is M6; Linux has none)")
    }

    public func delete(network: String) throws(PlatformServiceError) {
        throw .unsupported("quick unlock (Windows Hello is M6; Linux has none)")
    }
}

// MARK: QR images (IOS-043)

/// dw-desktop's decoder (PNG, JPEG, BMP); works on every OS.
public struct DesktopQRImageDecoder: QRImageDecoding {
    public init() {}

    public func decode(imageData: Data) throws(PlatformServiceError) -> [String] {
        try desktopCall { () throws(DashKitError) in try DesktopFunctions.decodeQRCodes(imageData) }
    }
}

// MARK: Clipboard (QT-090, IOS-043)

#if os(Linux)
    /// The clipboard through the session's command-line tools: `wl-paste` /
    /// `wl-copy` on Wayland, else `xclip`. `nil` when none is installed;
    /// `setString` then does nothing and `isAvailable` says so. Images are
    /// read as PNG.
    public struct CommandLineClipboard: ClipboardProviding {
        private enum Tool {
            case wayland(paste: URL, copy: URL)
            case xclip(URL)
        }

        private let tool: Tool?

        public init(environment: [String: String] = ProcessInfo.processInfo.environment) {
            func find(_ name: String) -> URL? {
                (environment["PATH"] ?? "/usr/bin:/bin").split(separator: ":")
                    .map { URL(fileURLWithPath: String($0)).appendingPathComponent(name) }
                    .first { FileManager.default.isExecutableFile(atPath: $0.path) }
            }
            if environment["WAYLAND_DISPLAY"] != nil, let paste = find("wl-paste"), let copy = find("wl-copy") {
                tool = .wayland(paste: paste, copy: copy)
            } else if environment["DISPLAY"] != nil, let xclip = find("xclip") {
                tool = .xclip(xclip)
            } else {
                tool = nil
            }
        }

        public var isAvailable: Bool { tool != nil }

        public func string() -> String? {
            switch tool {
            case .wayland(let paste, _): run(paste, ["--no-newline", "--type", "text/plain"]).flatMap { String(data: $0, encoding: .utf8) }
            case .xclip(let xclip): run(xclip, ["-selection", "clipboard", "-o"]).flatMap { String(data: $0, encoding: .utf8) }
            case nil: nil
            }
        }

        public func setString(_ string: String) {
            switch tool {
            case .wayland(_, let copy): _ = run(copy, [], input: Data(string.utf8))
            case .xclip(let xclip): _ = run(xclip, ["-selection", "clipboard", "-i"], input: Data(string.utf8))
            case nil: break
            }
        }

        public func imageData() -> Data? {
            switch tool {
            case .wayland(let paste, _): run(paste, ["--type", "image/png"])
            case .xclip(let xclip): run(xclip, ["-selection", "clipboard", "-t", "image/png", "-o"])
            case nil: nil
            }
        }

        private func run(_ tool: URL, _ arguments: [String], input: Data? = nil) -> Data? {
            let process = Process()
            process.executableURL = tool
            process.arguments = arguments
            let output = Pipe()
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            let inputPipe = input.map { _ in Pipe() }
            process.standardInput = inputPipe ?? FileHandle.nullDevice
            do { try process.run() } catch { return nil }
            if let input, let inputPipe {
                inputPipe.fileHandleForWriting.write(input)
                try? inputPipe.fileHandleForWriting.close()
            }
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return process.terminationStatus == 0 && !data.isEmpty ? data : nil
        }
    }
#endif

// MARK: Idle events (IOS-015)

/// No OS idle source yet on Windows/Linux (session D-Bus `PrepareForSleep`
/// / `Lock`, `WM_POWERBROADCAST`): the stream ends at once, and auto-lock
/// relies on its in-app inactivity timer only.
public struct DesktopIdleMonitor: IdleMonitoring {
    public init() {}

    public func events() -> AsyncStream<IdleEvent> {
        AsyncStream { $0.finish() }
    }
}

// MARK: Files (QT-116, QT-143)

/// Opens the folder that holds `url` (Linux `xdg-open`, Windows
/// `explorer /select,`).
public struct DesktopFileRevealer: FileRevealing {
    public init() {}

    public func reveal(_ url: URL) throws(PlatformServiceError) {
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw PlatformServiceError(code: "desktop.os_error", detail: "\(url.path) does not exist")
        }
        let process = Process()
        #if os(Windows)
            process.executableURL = URL(fileURLWithPath: "C:\\Windows\\explorer.exe")
            process.arguments = ["/select,\(url.path)"]
        #elseif os(Linux)
            process.executableURL = URL(fileURLWithPath: "/usr/bin/xdg-open")
            var isDirectory: ObjCBool = false
            FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
            process.arguments = [isDirectory.boolValue ? url.path : url.deletingLastPathComponent().path]
        #else
            throw .unsupported("revealing files (PlatformServicesMac on macOS)")
        #endif
        #if os(Windows) || os(Linux)
            do {
                try process.run()
            } catch {
                throw PlatformServiceError(code: "desktop.os_error", detail: "\(error)")
            }
        #endif
    }
}
