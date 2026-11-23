// Reads and restores the frame of the window a SwiftUI view lives in, for
// remembered window geometry (dash-qt saves each window's geometry).
#if os(macOS)
import AppKit
import SwiftUI

/// A window frame in screen coordinates (origin bottom-left, AppKit).
public struct WindowFrame: Sendable, Hashable {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }
}

/// Place in the background of a window's root view. On the first attach it
/// moves the window to `initialFrame` when that frame is still on a screen;
/// afterwards it reports the frame after every move and resize.
public struct WindowFrameAccessor: NSViewRepresentable {
    public let initialFrame: WindowFrame?
    public let onChange: (WindowFrame) -> Void

    public init(initialFrame: WindowFrame?, onChange: @escaping (WindowFrame) -> Void) {
        self.initialFrame = initialFrame
        self.onChange = onChange
    }

    public func makeNSView(context: Context) -> FrameTrackingView {
        let view = FrameTrackingView()
        view.initialFrame = initialFrame
        view.onChange = onChange
        return view
    }

    public func updateNSView(_ view: FrameTrackingView, context: Context) {
        view.onChange = onChange
    }

    public final class FrameTrackingView: NSView {
        var initialFrame: WindowFrame?
        var onChange: ((WindowFrame) -> Void)?
        private var observers: [NSObjectProtocol] = []
        private var restored = false

        public override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            observers.forEach(NotificationCenter.default.removeObserver)
            observers = []
            guard let window else { return }
            if !restored, let frame = initialFrame {
                restored = true
                let rect = NSRect(x: frame.x, y: frame.y, width: frame.width, height: frame.height)
                if NSScreen.screens.contains(where: { $0.visibleFrame.intersects(rect) }), rect.width > 100, rect.height > 100 {
                    window.setFrame(rect, display: true)
                }
            }
            for name in [NSWindow.didMoveNotification, NSWindow.didEndLiveResizeNotification] {
                observers.append(NotificationCenter.default.addObserver(
                    forName: name, object: window, queue: .main
                ) { [weak self] _ in
                    MainActor.assumeIsolated { self?.report() }
                })
            }
        }

        private func report() {
            guard let frame = window?.frame else { return }
            onChange?(WindowFrame(x: frame.minX, y: frame.minY, width: frame.width, height: frame.height))
        }

        isolated deinit {
            observers.forEach(NotificationCenter.default.removeObserver)
        }
    }
}
#endif
