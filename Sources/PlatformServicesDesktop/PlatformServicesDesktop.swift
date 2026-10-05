// Windows/Linux implementations of the PlatformServices protocols over the
// dw-desktop UniFFI objects (WS-08). Placeholder; empty module on macOS.
#if os(Linux) || os(Windows)
import Foundation
import PlatformServices

public enum PlatformServicesDesktopModule {
    public static let name = "PlatformServicesDesktop"
}
#endif
