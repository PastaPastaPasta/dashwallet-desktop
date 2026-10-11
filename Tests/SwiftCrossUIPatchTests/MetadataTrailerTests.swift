import Foundation
import Testing

@testable import SwiftCrossUI

extension PatchTests {
    /// Patch P12 (Vendor/PATCHES.md): the Swift Bundler metadata trailer is read from the end of
    /// the executable without reading the rest of it.
    @Suite("SwiftCrossUI patch P12: metadata trailer")
    struct MetadataTrailerTests {
        static let json = Data(#"{"appIdentifier":"org.dash.wallet.desktop.CrossUI","appVersion":"0.1.0"}"#.utf8)

        /// Writes `bytes` to a new temporary file and returns its URL.
        static func file(_ bytes: Data) throws -> URL {
            let url = FileManager.default.temporaryDirectory
                .appendingPathComponent("p12-\(UUID().uuidString)")
            try bytes.write(to: url)
            return url
        }

        static func trailer(_ json: Data, length: UInt64? = nil) -> Data {
            var data = json
            withUnsafeBytes(of: (length ?? UInt64(json.count)).bigEndian) { data.append(contentsOf: $0) }
            data.append(contentsOf: "SBUNMETA".utf8)
            return data
        }

        @Test("the JSON after an executable's bytes")
        func readsTrailer() throws {
            let url = try Self.file(Data(repeating: 0x7f, count: 1 << 20) + Self.trailer(Self.json))
            defer { try? FileManager.default.removeItem(at: url) }
            #expect(try swiftBundlerMetadataJSON(inExecutableAt: url) == Self.json)
        }

        @Test("nil without the magic bytes, or for a file shorter than a trailer")
        func noTrailer() throws {
            for bytes in [Data(repeating: 1, count: 4096), Data("SBUNMETA".utf8), Data()] {
                let url = try Self.file(bytes)
                defer { try? FileManager.default.removeItem(at: url) }
                #expect(try swiftBundlerMetadataJSON(inExecutableAt: url) == nil)
            }
        }

        @Test("a length past the start of the file throws")
        func badLength() throws {
            let url = try Self.file(Self.trailer(Self.json, length: 1 << 40))
            defer { try? FileManager.default.removeItem(at: url) }
            #expect(throws: (any Error).self) { try swiftBundlerMetadataJSON(inExecutableAt: url) }
        }

        @Test("a missing file throws")
        func missingFile() {
            let url = URL(fileURLWithPath: "/nonexistent/p12-\(UUID().uuidString)")
            #expect(throws: (any Error).self) { try swiftBundlerMetadataJSON(inExecutableAt: url) }
        }
    }
}
