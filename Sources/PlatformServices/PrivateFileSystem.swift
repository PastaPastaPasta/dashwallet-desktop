// Owner-only directories and files for the wallet's data (DESIGN-opus §1.7).
// The wallet storage refuses a database below a group- or world-writable
// directory, and a plain `FileManager.createDirectory` takes the umask's mode:
// 0775 under the umask 002 of Ubuntu and Fedora desktops (user-private groups).
// Windows keeps its per-user ACLs under the profile; nothing changes there.
import Foundation

public enum PrivateFileSystem {
    /// Creates `url` and its missing parents, each restricted to the current
    /// user (0700) whatever the umask. Existing directories keep their mode:
    /// a `--datadir` or chosen directory and its parents are the user's, and
    /// the storage check names the one that is too open.
    public static func createDirectory(_ url: URL) throws {
        let fm = FileManager.default
        var missing: [URL] = []
        var current = url.standardizedFileURL
        while !fm.fileExists(atPath: current.path) {
            missing.append(current)
            let parent = current.deletingLastPathComponent()
            if parent.path == current.path { break }
            current = parent
        }
        for dir in missing.reversed() {
            do {
                try fm.createDirectory(
                    at: dir, withIntermediateDirectories: false, attributes: Self.permissions(0o700))
            } catch {
                // Created meanwhile by another thread or process: not ours.
                if isDirectory(dir) { continue }
                throw error
            }
            try setMode(dir, 0o700)
        }
        guard isDirectory(url) else {
            throw CocoaError(.fileWriteFileExists, userInfo: [NSFilePathErrorKey: url.path])
        }
    }

    /// `createDirectory` for a directory the app owns (the default data
    /// root): one that already exists with group or other permissions, left
    /// by an older build or a permissive umask, loses them and `log` says so.
    /// A symlink is left alone (the user put it there; the storage check
    /// follows it). Returns the previous mode when it changed one.
    @discardableResult
    public static func createOwnedDirectory(
        _ url: URL, log: (String) -> Void = PrivateFileSystem.logToStandardError
    ) throws -> Int? {
        try createDirectory(url)
        guard let change = try removeGroupAndOther(url) else { return nil }
        log("restricted \(url.path) to the current user (mode was \(octal(change.old)), now \(octal(change.new)))")
        return change.old
    }

    /// Takes group and other permissions off the existing file `url` (0664
    /// becomes 0600): the settings files the app writes next to the wallet
    /// data.
    public static func restrictFile(_ url: URL) throws {
        _ = try removeGroupAndOther(url)
    }

    /// Writes `message` to stderr, as the app's other diagnostics.
    public static func logToStandardError(_ message: String) {
        FileHandle.standardError.write(Data("dash-wallet: \(message)\n".utf8))
    }

    /// The POSIX permission bits of `url`; `nil` on Windows.
    public static func mode(of url: URL) throws -> Int? {
        #if os(Windows)
            return nil
        #else
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            return (attributes[.posixPermissions] as? NSNumber)?.intValue
        #endif
    }

    /// (old mode, new mode) when it changed `url`; `nil` for a symlink, an
    /// owner-only item, or on Windows.
    private static func removeGroupAndOther(_ url: URL) throws -> (old: Int, new: Int)? {
        #if os(Windows)
            return nil
        #else
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            guard attributes[.type] as? FileAttributeType != .typeSymbolicLink,
                let old = (attributes[.posixPermissions] as? NSNumber)?.intValue, old & 0o077 != 0
            else { return nil }
            try setMode(url, old & ~0o077)
            return (old: old, new: old & ~0o077)
        #endif
    }

    private static func setMode(_ url: URL, _ mode: Int) throws {
        if let attributes = permissions(mode) {
            try FileManager.default.setAttributes(attributes, ofItemAtPath: url.path)
        }
    }

    private static func permissions(_ mode: Int) -> [FileAttributeKey: Any]? {
        #if os(Windows)
            return nil
        #else
            return [.posixPermissions: NSNumber(value: mode)]
        #endif
    }

    private static func isDirectory(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    private static func octal(_ mode: Int) -> String {
        let digits = String(mode, radix: 8)
        return String(repeating: "0", count: max(0, 4 - digits.count)) + digits
    }
}
