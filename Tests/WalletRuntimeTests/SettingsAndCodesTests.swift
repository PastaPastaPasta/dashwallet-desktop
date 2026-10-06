import Foundation
import Testing
@testable import WalletRuntime

@MainActor
@Suite struct SettingsStoreTests {
    @Test func defaultsWithoutFiles() {
        let dir = TempDir()
        let store = SettingsStore(directory: dir.url)
        #expect(store.display == DisplaySettings())
        #expect(store.lastNetwork == nil)
        #expect(store.requireAuthenticationForEveryPayment)
        #expect(store.recoveredFromCorruption.isEmpty)
    }

    @Test func updatesPersistAndNotify() async throws {
        let dir = TempDir()
        let store = SettingsStore(directory: dir.url)
        let changes = store.changes()
        var iterator = changes.makeAsyncIterator()
        #expect(await iterator.next() == DisplaySettings())

        let next = DisplaySettings(unit: .milliDash, decimalDigits: 4, hideBalances: true)
        try store.update(next)
        #expect(await iterator.next() == next)
        #expect(SettingsStore(directory: dir.url).display == next)
    }

    @Test func decimalDigitsOutsideTwoToEightAreRejected() {
        let store = SettingsStore(directory: TempDir().url)
        #expect(throws: ServiceError.self) { try store.update(DisplaySettings(decimalDigits: 9)) }
        #expect(throws: ServiceError.self) { try store.update(DisplaySettings(decimalDigits: 1)) }
        #expect(store.display.decimalDigits == 8)
    }

    @Test func corruptFilesMoveToBackupAndDefaultsApply() throws {
        let dir = TempDir()
        let url = dir.url.appendingPathComponent(SettingsStore.settingsFileName)
        try Data("{not json".utf8).write(to: url)
        let store = SettingsStore(directory: dir.url)
        #expect(store.display == DisplaySettings())
        #expect(store.recoveredFromCorruption == [url])
        #expect(FileManager.default.fileExists(atPath: url.appendingPathExtension("bak").path))
        #expect(!FileManager.default.fileExists(atPath: url.path))
    }

    @Test func olderFilesWithMissingKeysStillLoad() throws {
        let dir = TempDir()
        let url = dir.url.appendingPathComponent(SettingsStore.settingsFileName)
        try Data(#"{"display":{"unit":{"duffs":{}},"decimalDigits":2,"hideBalances":false}}"#.utf8).write(to: url)
        let store = SettingsStore(directory: dir.url)
        #expect(store.display.unit == .duffs)
        #expect(store.display.decimalDigits == 2)
        #expect(store.recoveredFromCorruption.isEmpty)
    }

    @Test func sectionsRoundTripOtherModulesTypes() throws {
        struct Prefs: Codable, Equatable {
            var theme: String
            var from: Date?
            var count: Int
        }
        let dir = TempDir()
        let store = SettingsStore(directory: dir.url)
        let value = Prefs(theme: "dark", from: Date(timeIntervalSince1970: 1_700_000_000), count: 3)
        try store.setSection("ui", value)
        #expect(store.section("ui", as: Prefs.self) == value)
        #expect(SettingsStore(directory: dir.url).section("ui", as: Prefs.self) == value)
        #expect(store.section("missing", as: Prefs.self) == nil)
    }

    @Test func unwritableLocationReportsSettingsWriteFailed() throws {
        let dir = TempDir()
        let blocker = dir.url.appendingPathComponent("file")
        try Data().write(to: blocker)
        // The settings "directory" is a regular file, so writes must fail.
        let store = SettingsStore(directory: blocker)
        do {
            try store.update(DisplaySettings(unit: .duffs))
            Issue.record("write into a file path must fail")
        } catch {
            #expect(error.code == .settingsWriteFailed)
        }
        #expect(store.display == DisplaySettings())
    }
}

@Suite struct ServiceErrorCodeTests {
    /// Every code in the m1-engine.md §4 table (common codes, domain enums
    /// and the legacy `EngineError` row).
    static func contractCodes() throws -> Set<String> {
        let doc = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("docs/contracts/m1-engine.md")
        let text = try String(contentsOf: doc, encoding: .utf8)
        let start = try #require(text.range(of: "## 4. Error codes"))
        let end = try #require(text.range(of: "## 5.", range: start.upperBound..<text.endIndex))
        let section = text[start.upperBound..<end.lowerBound]
        var codes = Set<String>()
        let regex = /`([a-z_]+(?:\.[a-z0-9_]+)?)`/
        for line in section.split(separator: "\n") {
            let isCommon = line.hasPrefix("Common to every domain")
            let isTableRow = line.hasPrefix("| `") && !line.hasPrefix("| Domain")
            guard isCommon || isTableRow else { continue }
            // The first cell of a table row is the enum name, not a code.
            let body = isTableRow ? line.split(separator: "|", omittingEmptySubsequences: true).dropFirst().joined() : String(line)
            for match in body.matches(of: regex) {
                codes.insert(String(match.1))
            }
        }
        return codes
    }

    @Test func engineCodesMatchTheContractTable() throws {
        // DashKit reports the legacy EngineError codes under their wallet.* names.
        let legacy: Set<String> = ["invalid_mnemonic", "wallet_already_exists"]
        let contract = try Self.contractCodes().subtracting(legacy)
        let swift = Set(ServiceErrorCode.engineCodes.map(\.rawValue))
        #expect(contract.subtracting(swift).isEmpty, "missing in Swift: \(contract.subtracting(swift).sorted())")
        #expect(swift.subtracting(contract).isEmpty, "not in the contract: \(swift.subtracting(contract).sorted())")
        #expect(swift.count == ServiceErrorCode.engineCodes.count, "duplicates in engineCodes")
    }

    /// The domain codes of the m2-engine.md §4 table.
    static func m2ContractCodes() throws -> Set<String> {
        let doc = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("docs/contracts/m2-engine.md")
        let text = try String(contentsOf: doc, encoding: .utf8)
        let start = try #require(text.range(of: "## 4. Error codes"))
        let end = try #require(text.range(of: "## 5.", range: start.upperBound..<text.endIndex))
        var codes = Set<String>()
        let regex = /`([a-z_]+\.[a-z0-9_]+)`/
        for line in text[start.upperBound..<end.lowerBound].split(separator: "\n")
        where line.hasPrefix("| `") {
            // The first cell names the enum; the second lists its codes.
            let body = line.split(separator: "|", omittingEmptySubsequences: true).dropFirst().joined()
            for match in body.matches(of: regex) {
                codes.insert(String(match.1))
            }
        }
        return codes
    }

    @Test func m2EngineCodesMatchTheContractTable() throws {
        let contract = try Self.m2ContractCodes()
        let swift = Set(ServiceErrorCode.m2EngineCodes.map(\.rawValue))
        #expect(!contract.isEmpty)
        #expect(contract.subtracting(swift).isEmpty, "missing in Swift: \(contract.subtracting(swift).sorted())")
        #expect(swift.subtracting(contract).isEmpty, "not in the contract: \(swift.subtracting(contract).sorted())")
        #expect(swift.count == ServiceErrorCode.m2EngineCodes.count, "duplicates in m2EngineCodes")
        #expect(Set(ServiceErrorCode.engineCodes).isDisjoint(with: ServiceErrorCode.m2EngineCodes))
    }
}
