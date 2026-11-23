// dashwallet-desktop patch P1 (Vendor/PATCHES.md): accessibility modifiers.
import AppKit
@_spi(Backends) import SwiftCrossUI

extension AppKitBackend: BackendFeatures.Accessibility {
    /// `accessibilityLabel` is the name VoiceOver reads and `accessibilityHelp`
    /// the hint. `nil` restores AppKit's default (for a button, its title).
    public func setAccessibility(of widget: NSView, label: String?, hint: String?) {
        if let button = widget as? NSCustomButton {
            button.explicitAccessibilityLabel = label
        } else if widget.accessibilityLabel() != label {
            widget.setAccessibilityLabel(label)
        }
        if widget.accessibilityHelp() != hint {
            widget.setAccessibilityHelp(hint)
        }
    }
}
