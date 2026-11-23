// Transaction notifications on macOS (QT-031, IOS-105, IOS-116) through
// UserNotifications.
#if os(macOS)
import Foundation
import PlatformServices
import UserNotifications

/// `SystemNotifying` over `UNUserNotificationCenter`.
///
/// UserNotifications only works for a process inside an app bundle (it
/// traps otherwise), so outside one — `swift test`, a bare executable —
/// `authorization()` is `.unavailable` and `post` fails with
/// `desktop.unsupported`. Clicks come back through `activations()` with
/// the notification's deep link. Create one per app and keep it alive: it
/// is the center's delegate.
public final class MacNotifier: NSObject, SystemNotifying, UNUserNotificationCenterDelegate, @unchecked Sendable {
    private static let deepLinkKey = "deepLink"

    // `lock` guards `continuations`.
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<String?>.Continuation] = [:]
    private let center: UNUserNotificationCenter?

    /// `inBundle` defaults to whether this process runs from an `.app`.
    public init(inBundle: Bool = Bundle.main.bundleURL.pathExtension == "app") {
        center = inBundle ? UNUserNotificationCenter.current() : nil
        super.init()
        center?.delegate = self
    }

    public func authorization() async -> NotificationAuthorization {
        guard let center else { return .unavailable }
        return Self.map(await center.notificationSettings().authorizationStatus)
    }

    public func requestAuthorization() async -> NotificationAuthorization {
        guard let center else { return .unavailable }
        _ = try? await center.requestAuthorization(options: [.alert, .sound])
        return await authorization()
    }

    public func post(_ notification: SystemNotification) async throws(PlatformServiceError) {
        guard let center else { throw .unsupported("notifications outside an app bundle") }
        switch await authorization() {
        case .authorized: break
        case .denied: throw PlatformServiceError(code: "platform.denied", detail: "notifications are off for the app")
        case .notDetermined, .unavailable:
            throw PlatformServiceError(code: "platform.denied", detail: "notifications were not allowed yet")
        }
        let content = UNMutableNotificationContent()
        content.title = notification.title
        content.body = notification.body
        if let link = notification.deepLink { content.userInfo = [Self.deepLinkKey: link] }
        let request = UNNotificationRequest(identifier: notification.id, content: content, trigger: nil)
        do {
            try await center.add(request)
        } catch {
            throw PlatformServiceError(code: "desktop.os_error", detail: error.localizedDescription)
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

    // MARK: UNUserNotificationCenterDelegate

    public func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let link = response.notification.request.content.userInfo[Self.deepLinkKey] as? String
        let targets = lock.withLock { Array(continuations.values) }
        for continuation in targets { continuation.yield(link) }
    }

    /// Shows notifications while the app is frontmost too, as dash-qt does.
    public func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list]
    }

    private static func map(_ status: UNAuthorizationStatus) -> NotificationAuthorization {
        switch status {
        case .authorized, .provisional, .ephemeral: .authorized
        case .denied: .denied
        case .notDetermined: .notDetermined
        @unknown default: .denied
        }
    }
}
#endif
