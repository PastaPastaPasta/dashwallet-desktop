// M2 service contracts: app shell — command line, startup phases, shutdown,
// tray/window behaviour settings and transaction notifications presentation
// (m2-swift.md §2.9; owner S1).
import Foundation

/// Parsed command line (QT-006). dash-qt flags kept where they mean
/// something to an SPV wallet; node-only flags are rejected as unknown.
public struct LaunchOptions: Sendable, Hashable {
    /// `--min`: start minimized (to the tray where there is one); no splash.
    public var startMinimized: Bool
    /// `--splash=0` hides the splash screen.
    public var showSplash: Bool
    /// `--resetguisettings`: back up and reset `settings.json`, show the
    /// data-directory chooser.
    public var resetGUISettings: Bool
    /// `--choosedatadir`: show the data-directory chooser.
    public var chooseDataDirectory: Bool
    /// `--datadir=<path>`.
    public var dataDirectory: URL?
    /// `--testnet`, `--regtest`, `--devnet=<name>`, `--chain=<name>`.
    public var network: DashNetwork?
    /// `--lang=<xx_YY>`.
    public var language: String?
    /// `--windowtitle=<s>`, appended to the window title.
    public var windowTitleSuffix: String?
    /// `dash:` (and other registered scheme) URIs; options may not follow one.
    public var uris: [String]
    /// `-help`: show the command-line options dialog (QT-153) and quit.
    public var showHelp: Bool
    /// `-version`: show the version and quit.
    public var showVersion: Bool

    public init(
        startMinimized: Bool = false, showSplash: Bool = true, resetGUISettings: Bool = false,
        chooseDataDirectory: Bool = false, dataDirectory: URL? = nil, network: DashNetwork? = nil,
        language: String? = nil, windowTitleSuffix: String? = nil, uris: [String] = [], showHelp: Bool = false,
        showVersion: Bool = false
    ) {
        self.startMinimized = startMinimized
        self.showSplash = showSplash
        self.resetGUISettings = resetGUISettings
        self.chooseDataDirectory = chooseDataDirectory
        self.dataDirectory = dataDirectory
        self.network = network
        self.language = language
        self.windowTitleSuffix = windowTitleSuffix
        self.uris = uris
        self.showHelp = showHelp
        self.showVersion = showVersion
    }
}

/// Command-line parsing and the `--help` text (QT-006, QT-153 "Command-line
/// options"). Errors: `launch.unknown_option`, `launch.invalid_value`,
/// `launch.option_after_uri`.
public protocol LaunchArgumentsParsing: Sendable {
    func parse(_ arguments: [String]) throws(ServiceError) -> LaunchOptions
    /// One line per option, for the help dialog (copy is localized by V1).
    var optionNames: [String] { get }
}

/// Startup phases for the splash (QT-005). The bar never moves backwards.
public enum StartupPhase: Sendable, Hashable {
    case loadingSettings
    case openingNetwork(DashNetwork)
    case loadingWallets
    case startingSync
    case ready
    /// Startup stopped; the code selects the error text (QT-010).
    case failed(ServiceErrorCode)
}

/// Startup progress (owner S1, fed by the lifecycle queue). Pressing Q or
/// closing the splash calls `requestEmergencyQuit()`.
@MainActor
public protocol StartupProgressing: AnyObject {
    var phase: StartupPhase { get }
    /// 0...1 within the whole startup; monotonic.
    var progress: Double { get }
    func changes() -> AsyncStream<StartupPhase>
    func requestEmergencyQuit()
}

/// Graceful quit (QT-008): stop SPV, close the session, flush settings. The
/// shutdown window cannot be closed while `isShuttingDown`.
@MainActor
public protocol ShutdownCoordinating: AnyObject {
    var isShuttingDown: Bool { get }
    /// Idempotent; returns when the engine has shut down.
    func shutdown() async
}

/// Window and tray behaviour kept in `global.json` (QT-028…030, QT-136).
public struct ShellSettings: Sendable, Hashable, Codable {
    /// dash-qt `fHideTrayIcon` inverted; ignored where no tray exists.
    public var showTrayIcon: Bool
    /// `fMinimizeToTray` (Windows/Linux).
    public var minimizeToTray: Bool
    /// `fMinimizeOnClose` (Windows/Linux).
    public var minimizeOnClose: Bool
    /// QT-033 `fShowCoinJoinPopups` (default on).
    public var showCoinJoinNotifications: Bool
    /// IOS-105: transaction notifications on (the OS may still deny them).
    public var notificationsEnabled: Bool

    public init(
        showTrayIcon: Bool = true, minimizeToTray: Bool = false, minimizeOnClose: Bool = false,
        showCoinJoinNotifications: Bool = true, notificationsEnabled: Bool = true
    ) {
        self.showTrayIcon = showTrayIcon
        self.minimizeToTray = minimizeToTray
        self.minimizeOnClose = minimizeOnClose
        self.showCoinJoinNotifications = showCoinJoinNotifications
        self.notificationsEnabled = notificationsEnabled
    }
}

@MainActor
public protocol ShellSettingsProviding: AnyObject {
    var shell: ShellSettings { get }
    func update(_ shell: ShellSettings) throws(ServiceError)
    func changes() -> AsyncStream<ShellSettings>
}
