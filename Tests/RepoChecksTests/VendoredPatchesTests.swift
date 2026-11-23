import Foundation
import Testing

/// The vendored SwiftCrossUI keeps its licence, and its patches stay listed:
/// every `dashwallet-desktop patch Pn` marker in Vendor/swift-cross-ui has a
/// `### Pn` section in Vendor/PATCHES.md, and every section marks a file.
@Suite struct VendoredPatchesTests {
    static let vendor = LintImportsTests.repoRoot.appendingPathComponent("Vendor")

    /// Patch ids ("P1", ...) of the markers in each vendored file.
    static func markers() throws -> [String: Set<String>] {
        let root = vendor.appendingPathComponent("swift-cross-ui")
        let regex = try Regex("dashwallet-desktop patch (P[0-9]+)")
        var found: [String: Set<String>] = [:]
        let files = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil)
        while let url = files?.nextObject() as? URL {
            guard ["swift", "c", "h"].contains(url.pathExtension),
                let text = try? String(contentsOf: url, encoding: .utf8)
            else { continue }
            for match in text.matches(of: regex) {
                if let id = match.output[1].substring { found[url.path, default: []].insert(String(id)) }
            }
        }
        return found
    }

    @Test func licenceAndPackageAreThere() {
        let root = Self.vendor.appendingPathComponent("swift-cross-ui")
        for name in ["LICENSE", "Package.swift", "Sources/Gtk/LICENSE.md"] {
            #expect(FileManager.default.fileExists(atPath: root.appendingPathComponent(name).path), "\(name)")
        }
    }

    @Test func everyPatchMarkerIsDocumentedAndEveryPatchIsMarked() throws {
        let doc = try String(contentsOf: Self.vendor.appendingPathComponent("PATCHES.md"), encoding: .utf8)
        let documented = Set(
            try doc.matches(of: Regex("(?m)^### (P[0-9]+) ")).compactMap { $0.output[1].substring.map(String.init) })
        let marked = try Self.markers().values.reduce(into: Set<String>()) { $0.formUnion($1) }
        #expect(!documented.isEmpty)
        #expect(marked == documented, "marked \(marked.sorted()), documented \(documented.sorted())")
    }
}
