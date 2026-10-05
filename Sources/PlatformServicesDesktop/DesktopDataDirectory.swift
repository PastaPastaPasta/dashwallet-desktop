// Data root of the desktop app per OS (DESIGN-opus §1.7). Each network keeps
// its own directory below the root; the wallet host names those.
//
// This file is compiled on every OS (not only Linux/Windows) so the macOS
// development build of the SwiftCrossUI app and the tests can use it.
import Foundation

public enum DesktopDataDirectory {
    public enum HostOS: Sendable, Hashable {
        case linux
        case windows
        case macOS

        /// The OS this binary was built for.
        public static var current: HostOS {
            #if os(Windows)
                .windows
            #elseif os(macOS)
                .macOS
            #else
                .linux
            #endif
        }
    }

    /// Directory name under `$XDG_DATA_HOME` on Linux.
    public static let linuxDirectoryName = "dashwallet"
    /// Bundle identifier used on macOS (`~/Library/Application Support/<id>`).
    public static let macOSDirectoryName = "org.dashfoundation.DashWallet"

    /// The data root for `os`:
    /// - Linux: `$XDG_DATA_HOME/dashwallet`, falling back to `~/.local/share/dashwallet`
    ///   when the variable is unset, empty or relative (XDG Base Directory spec). Inside
    ///   Flatpak `XDG_DATA_HOME` already points at `~/.var/app/<app-id>/data`.
    /// - Windows: `%APPDATA%\Dash\DashWallet`, falling back to `<home>\AppData\Roaming\Dash\DashWallet`.
    /// - macOS: `~/Library/Application Support/org.dashfoundation.DashWallet`.
    ///
    /// `--datadir` overrides the result; that is the caller's job.
    public static func root(
        for os: HostOS = .current,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> URL {
        switch os {
        case .linux:
            let base: URL
            if let xdg = environment["XDG_DATA_HOME"], xdg.hasPrefix("/") {
                base = URL(fileURLWithPath: xdg, isDirectory: true)
            } else {
                base = home.appendingPathComponent(".local/share", isDirectory: true)
            }
            return base.appendingPathComponent(linuxDirectoryName, isDirectory: true)
        case .windows:
            let base: URL
            if let appData = environment["APPDATA"], !appData.isEmpty {
                base = URL(fileURLWithPath: appData, isDirectory: true)
            } else {
                base = home.appendingPathComponent("AppData", isDirectory: true)
                    .appendingPathComponent("Roaming", isDirectory: true)
            }
            return base.appendingPathComponent("Dash", isDirectory: true)
                .appendingPathComponent("DashWallet", isDirectory: true)
        case .macOS:
            return home.appendingPathComponent("Library", isDirectory: true)
                .appendingPathComponent("Application Support", isDirectory: true)
                .appendingPathComponent(macOSDirectoryName, isDirectory: true)
        }
    }

    /// Creates `root` (and its parents) if missing and returns it.
    @discardableResult
    public static func prepare(_ root: URL) throws -> URL {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }
}
