// Foundation-only OS services every OS shares: the first-run data
// directory checks (QT-004).
import Foundation

/// `DataDirectoryInspecting` over FileManager, with dash-qt's states
/// (intro.cpp `FreespaceChecker`): a missing path that can be created, an
/// existing directory, a path that is a file, or a path whose nearest
/// existing parent is not a writable directory. Free space is read from
/// the nearest existing ancestor's volume.
public struct FileSystemDataDirectoryInspector: DataDirectoryInspecting {
    public init() {}

    public func inspect(_ url: URL) async -> DataDirectoryStatus {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        let path = url.standardizedFileURL.path
        let state: DataDirectoryState
        if fm.fileExists(atPath: path, isDirectory: &isDirectory) {
            state = isDirectory.boolValue ? .exists : .notADirectory
        } else if let parent = Self.nearestExisting(url), Self.isWritableDirectory(parent) {
            state = .willCreate
        } else {
            state = .cannotCreate
        }
        return DataDirectoryStatus(state: state, availableBytes: Self.availableBytes(near: url))
    }

    public func create(_ url: URL) throws(PlatformServiceError) {
        do {
            try FileManager.default.createDirectory(
                at: url, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        } catch {
            throw PlatformServiceError(code: "desktop.os_error", detail: "\(url.path): \(error.localizedDescription)")
        }
    }

    /// `url` itself or its closest ancestor that exists.
    static func nearestExisting(_ url: URL) -> URL? {
        var current = url.standardizedFileURL
        let fm = FileManager.default
        while !fm.fileExists(atPath: current.path) {
            let parent = current.deletingLastPathComponent()
            if parent.path == current.path { return nil }
            current = parent
        }
        return current
    }

    private static func isWritableDirectory(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
            && FileManager.default.isWritableFile(atPath: url.path)
    }

    private static func availableBytes(near url: URL) -> Int64? {
        guard let existing = nearestExisting(url) else { return nil }
        #if os(macOS)
            if let values = try? existing.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]),
                let bytes = values.volumeAvailableCapacityForImportantUsage
            {
                return bytes
            }
        #endif
        guard let attributes = try? FileManager.default.attributesOfFileSystem(forPath: existing.path),
            let free = attributes[.systemFreeSize] as? NSNumber
        else { return nil }
        return free.int64Value
    }
}
