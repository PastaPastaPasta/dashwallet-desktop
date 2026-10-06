// Swift-side preferences (DESIGN-opus §1.7 "UI prefs" / "Global prefs").
import DashKit
import Foundation
import Observation

/// A JSON value, so the settings file can hold sections whose types live in
/// other modules (e.g. WalletFeatures' `UIPreferences`) without this module
/// knowing them.
public enum JSONValue: Sendable, Hashable, Codable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

/// `SettingsProviding` backed by two JSON files:
/// - `settings.json`: display settings, "require authentication for every
///   payment", and named sections other modules store (`section(_:as:)`);
/// - `global.json`: the last network a session was opened on.
///
/// Writes are atomic. A file that cannot be decoded is moved to `<name>.bak`
/// and defaults are used; `recoveredFromCorruption` then reports it so the UI
/// can offer dash-qt's Reset/Abort choice (QT-007). A missing file means
/// defaults, silently. Both files live in the data root; the network a
/// session runs on comes from the lifecycle queue (`SessionObserving`).
@MainActor
@Observable
public final class SettingsStore: SettingsProviding, SessionObserving {
    // Missing keys take their defaults, so files from older versions load.
    private struct SettingsFile: Codable {
        var version = 1
        var display = DisplaySettings()
        var requireAuthenticationForEveryPayment = true
        var sections: [String: JSONValue] = [:]

        init() {}

        init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
            display = try c.decodeIfPresent(DisplaySettings.self, forKey: .display) ?? DisplaySettings()
            requireAuthenticationForEveryPayment =
                try c.decodeIfPresent(Bool.self, forKey: .requireAuthenticationForEveryPayment) ?? true
            sections = try c.decodeIfPresent([String: JSONValue].self, forKey: .sections) ?? [:]
        }
    }

    private struct GlobalFile: Codable {
        var version = 1
        var lastNetwork: DashNetwork?
        /// Sections other modules store for every network (M2: `shell`).
        var sections: [String: JSONValue] = [:]

        init() {}

        init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
            lastNetwork = try c.decodeIfPresent(DashNetwork.self, forKey: .lastNetwork)
            sections = try c.decodeIfPresent([String: JSONValue].self, forKey: .sections) ?? [:]
        }
    }

    public static let settingsFileName = "settings.json"
    public static let globalFileName = "global.json"
    public static let decimalDigitsRange = 2...8

    public var display: DisplaySettings { settings.display }
    public var lastNetwork: DashNetwork? { global.lastNetwork }
    /// iOS "Require authentication for every payment" (default on).
    public var requireAuthenticationForEveryPayment: Bool { settings.requireAuthenticationForEveryPayment }
    /// Files that were unreadable at load and moved to `.bak`.
    public private(set) var recoveredFromCorruption: [URL] = []
    /// The last failure to write a file (e.g. while recording `lastNetwork`,
    /// which has no caller to throw to).
    public private(set) var lastError: ServiceError?

    private var settings: SettingsFile
    private var global: GlobalFile
    @ObservationIgnored public let settingsURL: URL
    @ObservationIgnored public let globalURL: URL
    @ObservationIgnored private let broadcaster: StateBroadcaster<DisplaySettings>

    /// Loads (or defaults) both files in `directory`.
    public convenience init(directory: URL) {
        self.init(
            settingsURL: directory.appendingPathComponent(Self.settingsFileName),
            globalURL: directory.appendingPathComponent(Self.globalFileName))
    }

    public init(settingsURL: URL, globalURL: URL) {
        self.settingsURL = settingsURL
        self.globalURL = globalURL
        var recovered: [URL] = []
        var loaded = Self.load(SettingsFile.self, from: settingsURL, recovered: &recovered) ?? SettingsFile()
        if !Self.decimalDigitsRange.contains(loaded.display.decimalDigits) {
            loaded.display.decimalDigits = DisplaySettings().decimalDigits
        }
        settings = loaded
        global = Self.load(GlobalFile.self, from: globalURL, recovered: &recovered) ?? GlobalFile()
        broadcaster = StateBroadcaster(loaded.display)
        recoveredFromCorruption = recovered
    }

    public func changes() -> AsyncStream<DisplaySettings> {
        broadcaster.stream()
    }

    /// Stores `display`. `decimalDigits` outside 2...8 is `invalid_argument`.
    public func update(_ display: DisplaySettings) throws(ServiceError) {
        guard Self.decimalDigitsRange.contains(display.decimalDigits) else {
            throw ServiceError(code: .invalidArgument, detail: "decimal digits \(display.decimalDigits) not in 2...8")
        }
        guard display != settings.display else { return }
        var next = settings
        next.display = display
        try write(next)
        broadcaster.send(display)
    }

    public func setRequireAuthenticationForEveryPayment(_ value: Bool) throws(ServiceError) {
        guard value != settings.requireAuthenticationForEveryPayment else { return }
        var next = settings
        next.requireAuthenticationForEveryPayment = value
        try write(next)
    }

    /// A section another module stored, decoded as `type`; `nil` when absent
    /// or no longer decodable as `type`.
    public func section<T: Decodable>(_ key: String, as type: T.Type) -> T? {
        guard let value = settings.sections[key], let data = try? JSONEncoder().encode(value) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    public func setSection<T: Encodable>(_ key: String, _ value: T) throws(ServiceError) {
        let json: JSONValue
        do {
            json = try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(value))
        } catch {
            throw ServiceError(code: .invalidArgument, detail: "section \(key) is not encodable: \(error)")
        }
        var next = settings
        next.sections[key] = json
        try write(next)
    }

    /// A section of `global.json`, decoded as `type`; `nil` when absent or
    /// no longer decodable as `type`.
    public func globalSection<T: Decodable>(_ key: String, as type: T.Type) -> T? {
        guard let value = global.sections[key], let data = try? JSONEncoder().encode(value) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    public func setGlobalSection<T: Encodable>(_ key: String, _ value: T) throws(ServiceError) {
        let json: JSONValue
        do {
            json = try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(value))
        } catch {
            throw ServiceError(code: .invalidArgument, detail: "section \(key) is not encodable: \(error)")
        }
        var next = global
        next.sections[key] = json
        do {
            try Self.save(next, to: globalURL)
        } catch {
            lastError = error
            throw error
        }
        global = next
        lastError = nil
    }

    /// Resets both files to defaults (`-resetguisettings`, Options "Reset
    /// options", QT-007 "Reset"). Each existing file is first copied to
    /// `<name>.bak` (replacing an older copy; a file already moved there as
    /// corrupt stays). Returns the copies made. Clears
    /// `recoveredFromCorruption`. The last network is kept, as dash-qt keeps
    /// the data directory.
    @discardableResult
    public func resetToDefaults() throws(ServiceError) -> [URL] {
        let fm = FileManager.default
        var backups: [URL] = []
        for url in [settingsURL, globalURL] where fm.fileExists(atPath: url.path) {
            let backup = url.appendingPathExtension("bak")
            do {
                try? fm.removeItem(at: backup)
                try fm.copyItem(at: url, to: backup)
            } catch {
                throw ServiceError(code: .settingsWriteFailed, detail: "backing up \(url.lastPathComponent): \(error)")
            }
            backups.append(backup)
        }
        var freshGlobal = GlobalFile()
        freshGlobal.lastNetwork = global.lastNetwork
        do {
            try Self.save(SettingsFile(), to: settingsURL)
            try Self.save(freshGlobal, to: globalURL)
        } catch {
            lastError = error
            throw error
        }
        settings = SettingsFile()
        global = freshGlobal
        recoveredFromCorruption = []
        lastError = nil
        broadcaster.send(settings.display)
        return backups
    }

    // MARK: SessionObserving

    public func sessionDidStart(_ network: DashKit.DashNetwork) async {
        let opened = DashNetwork(network)
        guard global.lastNetwork != opened else { return }
        var next = global
        next.lastNetwork = opened
        do {
            try Self.save(next, to: globalURL)
            global = next
            lastError = nil
        } catch {
            lastError = error
        }
    }

    public func sessionWillStop(_ network: DashKit.DashNetwork) async {}

    public func walletsDidChange(_ network: DashKit.DashNetwork) async {}

    // MARK: Files

    private func write(_ next: SettingsFile) throws(ServiceError) {
        do {
            try Self.save(next, to: settingsURL)
        } catch {
            lastError = error
            throw error
        }
        settings = next
        lastError = nil
    }

    private static func save<T: Encodable>(_ value: T, to url: URL) throws(ServiceError) {
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            let data = try encoder.encode(value)
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: url, options: .atomic)
        } catch {
            throw ServiceError(code: .settingsWriteFailed, detail: "\(url.lastPathComponent): \(error)")
        }
    }

    /// The decoded file; `nil` when missing. An undecodable file is moved
    /// to `.bak` (replacing an older one) and reported in `recovered`.
    private static func load<T: Decodable>(_ type: T.Type, from url: URL, recovered: inout [URL]) -> T? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        if let value = try? JSONDecoder().decode(T.self, from: data) { return value }
        let backup = url.appendingPathExtension("bak")
        let fm = FileManager.default
        try? fm.removeItem(at: backup)
        try? fm.moveItem(at: url, to: backup)
        recovered.append(url)
        return nil
    }
}
