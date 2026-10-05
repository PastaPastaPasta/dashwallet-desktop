import Foundation
import PlatformServicesDesktop
import Testing

struct DesktopDataDirectoryTests {
    let home = URL(fileURLWithPath: "/home/alice", isDirectory: true)

    @Test func linuxUsesXDGDataHome() {
        let root = DesktopDataDirectory.root(for: .linux, environment: ["XDG_DATA_HOME": "/data/xdg"], home: home)
        #expect(root.path == "/data/xdg/dashwallet")
    }

    @Test func linuxFallsBackForMissingEmptyOrRelativeXDGDataHome() {
        for environment in [[:], ["XDG_DATA_HOME": ""], ["XDG_DATA_HOME": "relative/dir"]] {
            let root = DesktopDataDirectory.root(for: .linux, environment: environment, home: home)
            #expect(root.path == "/home/alice/.local/share/dashwallet")
        }
    }

    @Test func linuxFlatpakDataHome() {
        let flatpak = "/home/alice/.var/app/org.dashfoundation.DashWallet/data"
        let root = DesktopDataDirectory.root(for: .linux, environment: ["XDG_DATA_HOME": flatpak], home: home)
        #expect(root.path == flatpak + "/dashwallet")
    }

    @Test func windowsUsesAppData() {
        let root = DesktopDataDirectory.root(for: .windows, environment: ["APPDATA": "/c/Users/alice/AppData/Roaming"], home: home)
        #expect(root.path == "/c/Users/alice/AppData/Roaming/Dash/DashWallet")
    }

    @Test func windowsFallsBackToRoamingUnderHome() {
        let root = DesktopDataDirectory.root(for: .windows, environment: [:], home: home)
        #expect(root.path == "/home/alice/AppData/Roaming/Dash/DashWallet")
    }

    @Test func macOSUsesApplicationSupport() {
        let root = DesktopDataDirectory.root(for: .macOS, environment: ["XDG_DATA_HOME": "/ignored"], home: home)
        #expect(root.path == "/home/alice/Library/Application Support/org.dashfoundation.DashWallet")
    }

    @Test func prepareCreatesTheDirectory() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("dwd-datadir-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: base) }
        let root = try DesktopDataDirectory.prepare(base.appendingPathComponent("a/b"))
        var isDirectory: ObjCBool = false
        #expect(FileManager.default.fileExists(atPath: root.path, isDirectory: &isDirectory))
        #expect(isDirectory.boolValue)
    }
}
