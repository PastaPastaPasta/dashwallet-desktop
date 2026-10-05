// macOS implementations of the PlatformServices protocols (WS-08/WS-03).
// Placeholder; compiles to an empty module on other platforms.
#if os(macOS)
import Foundation
import PlatformServices

public enum PlatformServicesMacModule {
    public static let name = "PlatformServicesMac"
}
#endif
