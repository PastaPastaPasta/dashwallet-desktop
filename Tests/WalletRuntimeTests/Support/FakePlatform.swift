// In-memory OS services for the M2 runtime tests. Each behaves like the
// real one in the cases the tests use: the key store refuses a missing
// item with `vault.quick_unlock_unavailable`, the notifier fails when it is
// unavailable, and so on.
import Foundation
import PlatformServices
import WalletRuntime

final class FakeKeyStore: BiometricKeyStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String: [UInt8]] = [:]
    let kind: BiometricKind
    var storeError: PlatformServiceError?
    var retrieveError: PlatformServiceError?
    private(set) var prompts: [String] = []

    init(kind: BiometricKind = .touchID) {
        self.kind = kind
    }

    func item(_ network: String) -> [UInt8]? {
        lock.withLock { items[network] }
    }

    func store(_ key: BiometricKey, network: String) throws(PlatformServiceError) {
        if let storeError { throw storeError }
        let bytes = key.withUnsafeBytes { Array($0) }
        lock.withLock { items[network] = bytes }
    }

    func retrieve(network: String, reason: String) async throws(PlatformServiceError) -> BiometricKey {
        lock.withLock { prompts.append(reason) }
        if let retrieveError { throw retrieveError }
        guard let bytes = lock.withLock({ items[network] }) else {
            throw PlatformServiceError(code: "vault.quick_unlock_unavailable", detail: "no item")
        }
        return bytes.withUnsafeBytes { BiometricKey(copying: $0) }
    }

    func delete(network: String) throws(PlatformServiceError) {
        _ = lock.withLock { items.removeValue(forKey: network) }
    }
}

final class FakeNotifier: SystemNotifying, @unchecked Sendable {
    private let lock = NSLock()
    private var _posted: [SystemNotification] = []
    var available = true

    var posted: [SystemNotification] {
        lock.withLock { _posted }
    }

    func authorization() async -> NotificationAuthorization {
        available ? .authorized : .unavailable
    }

    func requestAuthorization() async -> NotificationAuthorization {
        await authorization()
    }

    func post(_ notification: SystemNotification) async throws(PlatformServiceError) {
        guard available else { throw .unsupported("notifications") }
        lock.withLock { _posted.append(notification) }
    }

    func activations() -> AsyncStream<String?> {
        AsyncStream { $0.finish() }
    }
}

/// Idle events the test sends by hand.
final class FakeIdle: IdleMonitoring, @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: AsyncStream<IdleEvent>.Continuation?

    func events() -> AsyncStream<IdleEvent> {
        let (stream, continuation) = AsyncStream<IdleEvent>.makeStream()
        lock.withLock { self.continuation = continuation }
        return stream
    }

    func send(_ event: IdleEvent) {
        _ = lock.withLock { continuation }?.yield(event)
    }
}

/// A single-instance coordinator whose forwarded launches the test sends.
final class FakeSingleInstance: SingleInstanceCoordinating, @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: AsyncStream<[String]>.Continuation?
    var role: SingleInstanceRole = .primary
    private(set) var claimedKeys: [String] = []

    func claim(key: String, arguments: [String]) throws(PlatformServiceError) -> SingleInstanceRole {
        lock.withLock { claimedKeys.append(key) }
        return role
    }

    func forwardedArguments() -> AsyncStream<[String]> {
        let (stream, continuation) = AsyncStream<[String]>.makeStream()
        lock.withLock { self.continuation = continuation }
        return stream
    }

    func forward(_ arguments: [String]) {
        _ = lock.withLock { continuation }?.yield(arguments)
    }
}

struct FakeQRDecoder: QRImageDecoding {
    var codes: [String] = []

    func decode(imageData: Data) throws(PlatformServiceError) -> [String] {
        guard !imageData.isEmpty else { throw PlatformServiceError(code: "desktop.image_unreadable") }
        guard !codes.isEmpty else { throw PlatformServiceError(code: "desktop.no_qr_code") }
        return codes
    }
}

struct FakeClipboard: ClipboardProviding {
    var image: Data?

    func string() -> String? { nil }
    func setString(_ string: String) {}
    func imageData() -> Data? { image }
}

struct UnusedPlatformService: URISchemeRegistering, LaunchAtLoginManaging, FileRevealing {
    var registeredAtInstall: Bool { true }
    var isSupported: Bool { false }

    func register(schemes: [String]) throws(PlatformServiceError) { throw .unsupported("test") }
    func isEnabled() throws(PlatformServiceError) -> Bool { throw .unsupported("test") }
    func setEnabled(_ enabled: Bool, arguments: [String]) throws(PlatformServiceError) { throw .unsupported("test") }
    func reveal(_ url: URL) throws(PlatformServiceError) { throw .unsupported("test") }
}

extension DesktopOSServices {
    static func fake(
        keys: FakeKeyStore = FakeKeyStore(), notifier: FakeNotifier = FakeNotifier(), idle: FakeIdle? = nil,
        instance: FakeSingleInstance = FakeSingleInstance()
    ) -> DesktopOSServices {
        DesktopOSServices(
            singleInstance: instance, uriSchemes: UnusedPlatformService(), launchAtLogin: UnusedPlatformService(),
            notifications: notifier, biometricKeys: keys, idle: idle, clipboard: FakeClipboard(),
            qrDecoder: FakeQRDecoder(), dataDirectories: FileSystemDataDirectoryInspector(),
            fileRevealer: UnusedPlatformService())
    }
}
