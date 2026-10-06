// Screen-capture protection while a recovery phrase is visible (IOS-006,
// DESIGN-opus §1.13): `NSWindow.sharingType = .none` on every app window.
#if os(macOS)
import AppKit
import Foundation
import PlatformServices

/// Excludes the app's windows from screenshots and screen recordings while
/// secret content is shown. macOS does not report captures to the app, so
/// `events()` finishes without yielding.
@MainActor
public final class MacScreenCaptureGuard: ScreenCaptureGuard {
    private var visible = false
    private var observer: NSObjectProtocol?

    public init() {}

    @discardableResult
    public func setSecretContentVisible(_ visible: Bool) -> Bool {
        self.visible = visible
        apply()
        if visible {
            // Windows opened while the phrase is shown (sheets, panels) get
            // the same treatment.
            if observer == nil {
                observer = NotificationCenter.default.addObserver(
                    forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main
                ) { [weak self] _ in
                    MainActor.assumeIsolated { self?.apply() }
                }
            }
        } else if let observer {
            NotificationCenter.default.removeObserver(observer)
            self.observer = nil
        }
        return visible
    }

    public func events() -> AsyncStream<ScreenCaptureEvent> {
        AsyncStream { $0.finish() }
    }

    private func apply() {
        for window in NSApplication.shared.windows {
            window.sharingType = visible ? .none : .readOnly
        }
    }
}
#endif
