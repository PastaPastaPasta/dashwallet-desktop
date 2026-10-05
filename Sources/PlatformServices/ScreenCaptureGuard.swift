// Screen-capture protection while a recovery phrase is visible (IOS-006).
import Foundation

/// A capture the OS reported while secret content was on screen.
public enum ScreenCaptureEvent: Sendable, Hashable {
    case screenshotTaken
    case recordingStarted
    case recordingStopped
}

/// Implemented per OS in PlatformServicesMac / PlatformServicesDesktop.
@MainActor
public protocol ScreenCaptureGuard: AnyObject {
    /// Marks secret content as shown (`true`) or hidden (`false`). While
    /// shown, the implementation excludes the app's windows from capture
    /// where the OS allows it. Returns whether that exclusion is in effect.
    @discardableResult
    func setSecretContentVisible(_ visible: Bool) -> Bool
    /// Captures the OS reports; streams nothing on OSes that report none.
    func events() -> AsyncStream<ScreenCaptureEvent>
}
