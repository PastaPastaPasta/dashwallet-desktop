// SwiftUI screens, scenes and menus for the macOS app (WS-11). The app's
// `@main` builds a `MacAppModel` and returns `DashWalletScenes(model:)`.
// Empty module on other platforms.
#if os(macOS)
import SwiftUI

public enum MacUIModule {
    public static let name = "MacUI"
}
#endif
