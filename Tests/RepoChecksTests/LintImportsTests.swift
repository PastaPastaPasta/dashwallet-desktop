import Foundation
import Testing

/// Runs scripts/lint-imports.sh against the repository and against a
/// synthetic tree with a known violation.
@Suite struct LintImportsTests {
    static let repoRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // RepoChecksTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // repo root

    static var script: URL { repoRoot.appendingPathComponent("scripts/lint-imports.sh") }

    struct Result {
        let status: Int32
        let stderr: String
    }

    static func run(root: URL) throws -> Result {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/bash")
        p.arguments = [script.path, root.path]
        let err = Pipe()
        p.standardError = err
        p.standardOutput = Pipe()
        try p.run()
        let data = err.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return Result(status: p.terminationStatus, stderr: String(decoding: data, as: UTF8.self))
    }

    static func write(_ text: String, to url: URL) throws {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try text.write(to: url, atomically: true, encoding: .utf8)
    }

    @Test func repositoryPassesLint() throws {
        let r = try Self.run(root: Self.repoRoot)
        #expect(r.status == 0, "\(r.stderr)")
    }

    @Test func detectsForbiddenImportsAndUnknownTargets() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("lint-imports-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        // View models must not import SwiftUI; attribute/kind forms are caught too.
        try Self.write("import Foundation\n@preconcurrency import SwiftUI\n",
                       to: tmp.appendingPathComponent("Sources/WalletFeatures/A.swift"))
        try Self.write("import struct Combine.AnyPublisher\n",
                       to: tmp.appendingPathComponent("Sources/DashKit/B.swift"))
        // Attributes with arguments (`@_spi(Name)`) are recognised as well.
        try Self.write("import Foundation\n@_spi(Internal) import AppKit\n",
                       to: tmp.appendingPathComponent("Sources/WalletRuntime/F.swift"))
        try Self.write("import Foundation\n", to: tmp.appendingPathComponent("Sources/Mystery/C.swift"))
        // Allowed: a test target importing its target and that target's deps.
        try Self.write("import Testing\n@testable import DashKit\nimport DashWalletCore\n",
                       to: tmp.appendingPathComponent("Tests/DashKitTests/D.swift"))
        // Comments are ignored.
        try Self.write("import Foundation // import SwiftUI\n",
                       to: tmp.appendingPathComponent("Sources/PlatformServices/E.swift"))

        let r = try Self.run(root: tmp)
        #expect(r.status == 1)
        #expect(r.stderr.contains("imports 'SwiftUI', not allowed in WalletFeatures"))
        #expect(r.stderr.contains("imports 'Combine', not allowed in DashKit"))
        #expect(r.stderr.contains("imports 'AppKit', not allowed in WalletRuntime"))
        #expect(r.stderr.contains("no allowed-imports entry for Sources/Mystery"))
        #expect(!r.stderr.contains("DashKitTests"))
        #expect(!r.stderr.contains("PlatformServices/E.swift"))
    }
}
