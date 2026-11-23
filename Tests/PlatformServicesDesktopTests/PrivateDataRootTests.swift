import DashKit
import Foundation
@testable import PlatformServices
import PlatformServicesDesktop
import Testing

/// Data roots are owner-only whatever the umask: the wallet storage refuses a
/// database below a group-writable directory (0775 under umask 002).
@Suite struct PrivateDataRootTests {
    /// A fresh owner-only scratch directory, removed when the test ends.
    final class Scratch {
        let url: URL
        init() throws {
            url = FileManager.default.temporaryDirectory
                .appendingPathComponent("dwd-private-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(
                at: url, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        }
        deinit { try? FileManager.default.removeItem(at: url) }
    }

    static func mode(_ url: URL) throws -> Int {
        try #require(try PrivateFileSystem.mode(of: url))
    }

    static func chmod(_ url: URL, _ mode: Int) throws {
        try FileManager.default.setAttributes([.posixPermissions: NSNumber(value: mode)], ofItemAtPath: url.path)
    }

    #if !os(Windows)
        @Test func createsMissingParentsOwnerOnlyAndKeepsExistingOnes() throws {
            let scratch = try Scratch()
            let existing = scratch.url.appendingPathComponent("existing")
            try FileManager.default.createDirectory(at: existing, withIntermediateDirectories: false)
            try Self.chmod(existing, 0o755)

            let root = existing.appendingPathComponent("share/dashwallet")
            try DesktopDataDirectory.prepare(root, defaultRoot: scratch.url.appendingPathComponent("elsewhere"))
            #expect(try Self.mode(existing) == 0o755)
            #expect(try Self.mode(existing.appendingPathComponent("share")) == 0o700)
            #expect(try Self.mode(root) == 0o700)
        }

        @Test func onlyTheDefaultRootIsRestrictedWhenItExists() throws {
            let scratch = try Scratch()
            let defaultRoot = scratch.url.appendingPathComponent("dashwallet")
            let chosen = scratch.url.appendingPathComponent("chosen")
            for dir in [defaultRoot, chosen] {
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: false)
                try Self.chmod(dir, 0o775)
            }
            var logged: [String] = []

            try DesktopDataDirectory.prepare(chosen, defaultRoot: defaultRoot) { logged.append($0) }
            #expect(try Self.mode(chosen) == 0o775)
            #expect(logged.isEmpty)

            try DesktopDataDirectory.prepare(defaultRoot, defaultRoot: defaultRoot) { logged.append($0) }
            #expect(try Self.mode(defaultRoot) == 0o700)
            #expect(logged == ["restricted \(defaultRoot.path) to the current user (mode was 0775, now 0700)"])

            try DesktopDataDirectory.prepare(defaultRoot, defaultRoot: defaultRoot) { logged.append($0) }
            #expect(logged.count == 1, "an owner-only root is left alone")
        }

        @Test func onlyGroupAndOtherBitsAreRemovedAndSymlinksAreLeftAlone() throws {
            let scratch = try Scratch()
            let readOnly = scratch.url.appendingPathComponent("read-only")
            try FileManager.default.createDirectory(at: readOnly, withIntermediateDirectories: false)
            try Self.chmod(readOnly, 0o555)
            #expect(try PrivateFileSystem.createOwnedDirectory(readOnly) { _ in } == 0o555)
            #expect(try Self.mode(readOnly) == 0o500)
            try Self.chmod(readOnly, 0o700)

            let target = scratch.url.appendingPathComponent("target")
            try FileManager.default.createDirectory(at: target, withIntermediateDirectories: false)
            try Self.chmod(target, 0o775)
            let link = scratch.url.appendingPathComponent("link")
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: target)
            #expect(try PrivateFileSystem.createOwnedDirectory(link) { _ in } == nil)
            #expect(try Self.mode(target) == 0o775)

            // A symlink in an intermediate component of a chosen directory is
            // followed; what it leads to keeps its mode.
            try PrivateFileSystem.createDirectory(link.appendingPathComponent("below"))
            #expect(try Self.mode(target) == 0o775)
            #expect(try Self.mode(target.appendingPathComponent("below")) == 0o700)
        }

        /// Review D1-r2: the root is swapped for a symlink after it is opened,
        /// before its mode is looked at. The directory that was opened is the
        /// one restricted; the symlink's target is not touched.
        @Test func aRootSwappedAfterItIsOpenedIsTheOneChanged() throws {
            let scratch = try Scratch()
            let root = scratch.url.appendingPathComponent("dashwallet")
            let moved = scratch.url.appendingPathComponent("moved")
            let outside = scratch.url.appendingPathComponent("outside")
            for dir in [root, outside] {
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: false)
                try Self.chmod(dir, 0o775)
            }
            // The walked path: /private/var/... for /var/... on macOS.
            let rootSuffix = "/\(scratch.url.lastPathComponent)/dashwallet"
            try PrivateFileSystem.$opened.withValue({ path in
                // The walk opens the root once.
                guard path.hasSuffix(rootSuffix) else { return }
                try? FileManager.default.moveItem(at: root, to: moved)
                try? FileManager.default.createSymbolicLink(at: root, withDestinationURL: outside)
            }) {
                try PrivateFileSystem.createOwnedDirectory(root) { _ in }
            }
            #expect(try Self.mode(outside) == 0o775)
            #expect(try Self.mode(moved) == 0o700)
        }

        /// A group member could have swapped anything below a group-writable
        /// directory: nothing there is changed, and the log says so.
        @Test func nothingChangesBelowADirectoryOthersCanWrite() throws {
            let scratch = try Scratch()
            let shared = scratch.url.appendingPathComponent("shared")
            let root = shared.appendingPathComponent("dashwallet")
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            try Self.chmod(shared, 0o775)
            try Self.chmod(root, 0o775)
            var logged: [String] = []
            #expect(try PrivateFileSystem.createOwnedDirectory(root) { logged.append($0) } == nil)
            #expect(try Self.mode(root) == 0o775)
            #expect(logged.count == 1 && logged[0].contains("/shared/dashwallet at mode 0775"), "\(logged)")
        }

        @Test func filesAreWrittenOwnerOnlyAndReplacedOrRefused() throws {
            let scratch = try Scratch()
            let file = scratch.url.appendingPathComponent("transactions.csv")
            try PrivateFileSystem.writeFile(Data("a".utf8), to: file, replacing: false)
            #expect(try Self.mode(file) == 0o600)
            #expect(throws: (any Error).self) {
                try PrivateFileSystem.writeFile(Data("b".utf8), to: file, replacing: false)
            }
            #expect(try String(contentsOf: file, encoding: .utf8) == "a")

            // Replacing an existing file the user chose: the new one is 0600,
            // and no temp file is left.
            try Self.chmod(file, 0o644)
            try PrivateFileSystem.writeFile(Data("c".utf8), to: file, replacing: true)
            #expect(try String(contentsOf: file, encoding: .utf8) == "c")
            #expect(try Self.mode(file) == 0o600)
            #expect(try FileManager.default.contentsOfDirectory(atPath: scratch.url.path) == ["transactions.csv"])

            // A symlink at a name that must be new is refused, not followed.
            let target = scratch.url.appendingPathComponent("target")
            try Data("theirs".utf8).write(to: target)
            let link = scratch.url.appendingPathComponent("link.psbt")
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: target)
            #expect(throws: (any Error).self) {
                try PrivateFileSystem.writeFile(Data("x".utf8), to: link, replacing: false)
            }
            #expect(try String(contentsOf: target, encoding: .utf8) == "theirs")
        }

        @Test func aFileInTheWayIsAnError() throws {
            let scratch = try Scratch()
            let file = scratch.url.appendingPathComponent("file")
            try Data("x".utf8).write(to: file)
            #expect(throws: (any Error).self) { try PrivateFileSystem.createDirectory(file) }
            #expect(throws: (any Error).self) {
                try PrivateFileSystem.createDirectory(file.appendingPathComponent("below"))
            }
        }
    #endif

    #if os(Linux)
        /// A parent the user may search but not read, as `/home` at 0711 on
        /// some distributions, is walked through (O_PATH).
        @Test func aSearchOnlyParentIsNoObstacle() throws {
            let scratch = try Scratch()
            let home = scratch.url.appendingPathComponent("home")
            let root = home.appendingPathComponent("dashwallet")
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            try Self.chmod(root, 0o775)
            try Self.chmod(home, 0o311)
            defer { try? Self.chmod(home, 0o700) }
            try PrivateFileSystem.createOwnedDirectory(root) { _ in }
            #expect(try Self.mode(root) == 0o700)
        }

        /// Set to a scratch directory in the child process of the umask test.
        static let childVariable = "DWD_UMASK_CHILD_DIR"

        /// Runs `umaskChild` in a copy of this test process started under
        /// umask 002 (the umask is process-wide, and the other suites run in
        /// parallel in this process). The child writes a result file, so a
        /// filter that matches nothing cannot pass.
        @Test func aFreshDataRootOpensUnderUmask002() throws {
            guard ProcessInfo.processInfo.environment[Self.childVariable] == nil else { return }
            let scratch = try Scratch()
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/sh")
            process.arguments = [
                "-c", #"umask 002 && exec "$0" --testing-library swift-testing --filter 'PrivateDataRootTests/umaskChild'"#,
                CommandLine.arguments[0],
            ]
            var environment = ProcessInfo.processInfo.environment
            environment[Self.childVariable] = scratch.url.path
            process.environment = environment
            let output = Pipe()
            process.standardOutput = output
            process.standardError = output
            try process.run()
            // A hung child must not hang the suite: reading ends when it dies.
            DispatchQueue.global().asyncAfter(deadline: .now() + 180) {
                if process.isRunning { process.terminate() }
            }
            let log = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            process.waitUntilExit()
            #expect(process.terminationStatus == 0, "\(log)")
            let result = try? String(contentsOf: scratch.url.appendingPathComponent("result"), encoding: .utf8)
            #expect(result == "ok", "\(log)")
        }

        @Test(.enabled(if: ProcessInfo.processInfo.environment[PrivateDataRootTests.childVariable] != nil))
        func umaskChild() async throws {
            let scratch = URL(
                fileURLWithPath: try #require(ProcessInfo.processInfo.environment[Self.childVariable]),
                isDirectory: true)
            let status = try String(contentsOfFile: "/proc/self/status", encoding: .utf8)
            try #require(status.contains("Umask:\t0002"), "\(status)")

            // A fresh install: `$XDG_DATA_HOME` and its parent are missing too.
            let fresh = DesktopDataLocation(
                os: .linux, environment: ["XDG_DATA_HOME": scratch.appendingPathComponent("local/share").path],
                home: scratch)
            let freshRoot = try fresh.defaultDataRoot()
            // An install whose root an older build created group-writable.
            let olderHome = scratch.appendingPathComponent("older/share")
            try FileManager.default.createDirectory(
                at: olderHome, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try FileManager.default.createDirectory(
                at: olderHome.appendingPathComponent(DesktopDataDirectory.linuxDirectoryName),
                withIntermediateDirectories: false)
            let older = DesktopDataLocation(os: .linux, environment: ["XDG_DATA_HOME": olderHome.path], home: scratch)
            try #require(try Self.mode(olderHome.appendingPathComponent(DesktopDataDirectory.linuxDirectoryName)) == 0o775)
            let olderRoot = try older.defaultDataRoot()

            for directory in ["local", "local/share", "local/share/dashwallet", "older/share/dashwallet"] {
                #expect(try Self.mode(scratch.appendingPathComponent(directory)) == 0o700, "\(directory)")
            }
            for root in [freshRoot, olderRoot] {
                let engine = try EngineClient(dataRoot: root, workerThreads: 2)
                try await engine.open(
                    .regtest,
                    options: SessionOptions(dapiAddresses: ["http://127.0.0.1:1"], quorumURL: "http://127.0.0.1:1"))
                try await engine.shutdown()
                #expect(try Self.mode(root.appendingPathComponent("regtest")) == 0o700)
                #expect(try Self.mode(root.appendingPathComponent("regtest/app.sqlite")) == 0o600)
            }
            // Exports (CSV, PSBT) and settings, both ways of writing.
            let exported = scratch.appendingPathComponent("transactions.csv")
            try PrivateFileSystem.writeFile(Data("x".utf8), to: exported, replacing: true)
            let psbt = scratch.appendingPathComponent("tx.psbt")
            try PrivateFileSystem.writeFile(Data("x".utf8), to: psbt, replacing: false)
            #expect(try Self.mode(exported) == 0o600)
            #expect(try Self.mode(psbt) == 0o600)
            try Data("ok".utf8).write(to: scratch.appendingPathComponent("result"))
        }
    #endif
}
