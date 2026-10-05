// Command-line switches of the macOS app and its data location.
#if os(macOS)
import Foundation
import WalletFeatures

/// What the app was asked to run.
public struct LaunchOptions: Sendable, Equatable {
    /// `--demo` / `--fixture` (funded) or `--demo-scenario <name>`; `nil` runs the real wallet.
    public var demoScenario: DemoScenario?
    /// `--appearance light|dark` overrides the theme setting (screenshots).
    public var appearance: AppTheme?
    /// `--no-menu-bar-extra` hides the menu bar companion (UI tests).
    public var menuBarExtra: Bool

    public init(demoScenario: DemoScenario? = nil, appearance: AppTheme? = nil, menuBarExtra: Bool = true) {
        self.demoScenario = demoScenario
        self.appearance = appearance
        self.menuBarExtra = menuBarExtra
    }

    public var isDemo: Bool { demoScenario != nil }

    /// Reads the switches above; unknown arguments (Xcode's `-NS…` pairs,
    /// file paths) are ignored. `DWD_DEMO=1` in the environment also selects demo mode.
    public static func parse(_ arguments: [String], environment: [String: String] = [:]) -> LaunchOptions {
        var options = LaunchOptions()
        if environment["DWD_DEMO"] == "1" { options.demoScenario = .funded }
        var index = 1
        while index < arguments.count {
            let argument = arguments[index]
            let value = index + 1 < arguments.count ? arguments[index + 1] : nil
            switch argument {
            case "--demo", "--fixture":
                options.demoScenario = options.demoScenario ?? .funded
            case "--demo-scenario":
                if let value, let scenario = DemoScenario(rawValue: value) {
                    options.demoScenario = scenario
                    index += 1
                }
            case "--appearance":
                if let value, let theme = AppTheme(rawValue: value) {
                    options.appearance = theme
                    index += 1
                }
            case "--no-menu-bar-extra":
                options.menuBarExtra = false
            default:
                break
            }
            index += 1
        }
        return options
    }
}

public enum AppPaths {
    public static let bundleIdentifier = "org.dashfoundation.DashWallet"

    /// `~/Library/Application Support/org.dashfoundation.DashWallet/`, the
    /// root of every network's data directory.
    public static func dataDirectory(fileManager: FileManager = .default) -> URL {
        let base = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? fileManager.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support")
        return base.appendingPathComponent(bundleIdentifier, isDirectory: true)
    }
}
#endif
