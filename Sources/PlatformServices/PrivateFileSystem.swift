// Owner-only directories and files for the wallet's data (DESIGN-opus §1.7).
// The wallet storage refuses a database below a group- or world-writable
// directory, and a plain `FileManager.createDirectory` takes the umask's mode:
// 0775 under the umask 002 of Ubuntu and Fedora desktops (user-private groups).
// Files the app writes (settings, CSV and PSBT exports) would likewise be 0664.
// Windows keeps its per-user ACLs under the profile; nothing changes there.
//
// A mode is never changed by path: a chmod changes whatever is at the path
// when it runs, and someone who can write a directory on the way can swap the
// checked entry for a symlink in between (review D1-r2). Paths are walked one
// component at a time from `/`, each directory opened with
// `openat(O_DIRECTORY | O_NOFOLLOW)` relative to its parent's descriptor, and
// a mode is read with `fstat` and changed with `fchmod` on that descriptor.
// Symlinks on the way are resolved here by the same rules. A mode changes only
// when the item is the current user's, no directory on the way lets another
// user replace its entries (another user owns it, or it is group- or
// other-writable without the sticky bit, or sticky and the entry is neither
// ours, root's nor the owner of `/`'s), and no symlink was followed into the
// owned directory.
// The same rules as the engine's dw-fs crate.
//
// The system calls come through Foundation, which re-exports the C library
// (Glibc, Darwin); scripts/lint-imports.sh allows only Foundation here.
import Foundation

public enum PrivateFileSystem {
    /// Creates `url` and its missing parents, each restricted to the current
    /// user (0700) whatever the umask. Existing directories keep their mode:
    /// a `--datadir` or chosen directory and its parents are the user's, and
    /// the storage check names the one that is too open.
    public static func createDirectory(_ url: URL) throws {
        #if os(Windows)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        #else
            let walk = try Walk(url.standardizedFileURL.path, owned: nil)
            walk.notes.forEach(logToStandardError)
        #endif
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
        #if os(Windows)
            try createDirectory(url)
            return nil
        #else
            let url = url.standardizedFileURL
            let walk = try Walk(url.deletingLastPathComponent().path, owned: url.lastPathComponent)
            walk.notes.forEach(log)
            guard let change = walk.ownedChange else { return nil }
            log("restricted \(url.path) to the current user (mode was \(octal(change.old)), now \(octal(change.new)))")
            return change.old
        #endif
    }

    /// Writes `data` to the new file `url`, readable and writable by the
    /// current user only (0600) from its creation, whatever the umask: wallet
    /// exports and settings hold addresses, labels and transaction history.
    /// With `replacing`, the data goes to a private temp file in the same
    /// directory that then replaces `url` (as `Data.write(options: .atomic)`
    /// does); an existing file is replaced, never re-moded. Without it, an
    /// existing `url` is an error.
    public static func writeFile(_ data: Data, to url: URL, replacing: Bool) throws {
        #if os(Windows)
            try data.write(to: url, options: replacing ? .atomic : .withoutOverwriting)
        #else
            let path = url.path
            guard replacing else {
                try writeNew(data, path: path)
                return
            }
            let temp = url.deletingLastPathComponent()
                .appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp").path
            try writeNew(data, path: temp)
            guard rename(temp, path) == 0 else {
                let error = PrivateFileError(path: path, operation: "rename", code: errno)
                unlink(temp)
                throw error
            }
        #endif
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

    /// Runs after each directory is opened, before its mode is looked at:
    /// the tests swap the path for a symlink there. Task-local, so a test's
    /// hook never sees another test's walk.
    @TaskLocal static var opened: (@Sendable (String) -> Void)?

    #if !os(Windows)
        private static func writeNew(_ data: Data, path: String) throws {
            // O_EXCL: never an existing file, nor a symlink's target.
            let fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
            guard fd >= 0 else { throw PrivateFileError(path: path, operation: "create", code: errno) }
            var written = false
            defer {
                close(fd)
                if !written { unlink(path) }
            }
            // 0600 even under a umask that masks owner bits.
            guard fchmod(fd, mode_t(0o600)) == 0 else {
                throw PrivateFileError(path: path, operation: "chmod", code: errno)
            }
            try data.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
                var offset = 0
                while offset < bytes.count {
                    let count = write(fd, bytes.baseAddress! + offset, bytes.count - offset)
                    if count < 0 {
                        if errno == EINTR { continue }
                        throw PrivateFileError(path: path, operation: "write", code: errno)
                    }
                    offset += count
                }
            }
            guard fsync(fd) == 0 else { throw PrivateFileError(path: path, operation: "fsync", code: errno) }
            written = true
        }

        /// Directories are walked with `O_PATH` on Linux, which needs only
        /// search permission (`/home` at 0711); elsewhere they must be readable.
        #if os(Linux)
            private static let directoryFlags = O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC | 0o10000000  // O_PATH
        #else
            private static let directoryFlags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
        #endif

        /// The walk from `/` to `chosen`, then to `owned` in it. Every
        /// directory opened stays open until the walk ends; each is the
        /// actual child of the one before it, so `..` pops one.
        private final class Walk {
            struct Dir {
                let fd: Int32
                var mode: Int
                let owner: uid_t
                let path: String
                /// No untrusted user could have replaced an entry from `/` to here.
                let safe: Bool
            }

            private var dirs: [Dir] = []
            private let euid = geteuid()
            /// The owner of `/`: root, or the uid root maps to in a user
            /// namespace (Flatpak), as the storage's ancestor check trusts.
            private let rootOwner: uid_t
            private var links = 0
            /// A symlink was followed that an untrusted user could have put there.
            private var unsafeLink = false
            /// The owned directory is a symlink: its target is left as it is.
            private var ownedLink = false
            /// What was left as it was, and why.
            private(set) var notes: [String] = []
            /// The owned directory's previous and new mode, when it changed.
            private(set) var ownedChange: (old: Int, new: Int)?

            init(_ chosen: String, owned: String?) throws {
                guard chosen.hasPrefix("/") else { throw PrivateFileError(path: chosen, operation: "open", code: EINVAL) }
                let root = open("/", PrivateFileSystem.directoryFlags)
                guard root >= 0 else { throw PrivateFileError(path: "/", operation: "open", code: errno) }
                let rootStat: (mode: Int, owner: uid_t)
                do { rootStat = try Self.describe(root, path: "/") } catch {
                    close(root)
                    throw error
                }
                rootOwner = rootStat.owner
                dirs = [Dir(fd: root, mode: rootStat.mode, owner: rootStat.owner, path: "/", safe: true)]
                // A stack: the next step is last. "/" is a step of its own.
                var steps: [(name: String, owned: Bool)] = []
                if let owned { steps.append((owned, true)) }
                steps += Self.steps(chosen).reversed().map { ($0, false) }
                while let step = steps.popLast() {
                    switch step.name {
                    case "/": closeAll(keepingFirst: 1)
                    case ".": continue
                    case "..": closeAll(keepingFirst: max(1, dirs.count - 1))
                    default:
                        // A symlink's target is the user's choice, wherever it is.
                        if let target = try enter(step.name, owned: step.owned) {
                            steps += Self.steps(target).reversed().map { ($0, false) }
                        }
                    }
                }
            }

            deinit { closeAll(keepingFirst: 0) }

            private func closeAll(keepingFirst count: Int) {
                while dirs.count > count { close(dirs.removeLast().fd) }
            }

            private static func steps(_ path: String) -> [String] {
                (path.hasPrefix("/") ? ["/"] : []) + path.split(separator: "/").map(String.init)
            }

            /// Opens the directory `name` in the current one, creating it 0700
            /// when missing, and makes it current. For a symlink, returns its
            /// target instead.
            private func enter(_ name: String, owned: Bool) throws -> String? {
                let parent = dirs[dirs.count - 1]
                let path = parent.path == "/" ? "/\(name)" : "\(parent.path)/\(name)"
                var created = false
                // Another process creating or removing it meanwhile gets a retry.
                for _ in 0..<3 {
                    let fd = openat(parent.fd, name, PrivateFileSystem.directoryFlags)
                    if fd >= 0 {
                        let info: (mode: Int, owner: uid_t)
                        do {
                            info = try Self.describe(fd, path: path)
                            if owned { try checkOwner(info.owner, path: path) }
                        } catch {
                            close(fd)
                            throw error
                        }
                        var dir = Dir(
                            fd: fd, mode: info.mode, owner: info.owner, path: path,
                            safe: parent.safe && !replaceable(parent, entryOwner: info.owner))
                        dirs.append(dir)
                        PrivateFileSystem.opened?(path)
                        let wanted: Int
                        switch (created, owned) {
                        case (true, _): wanted = 0o700  // mkdirat gave 0700 less the umask.
                        case (false, true): wanted = info.mode & ~0o077
                        case (false, false): wanted = info.mode
                        }
                        if try setMode(dir, wanted) {
                            if owned && !created { ownedChange = (info.mode, wanted) }
                            // Closed to others now: its entries are stable.
                            dir.mode = wanted
                            dirs[dirs.count - 1] = dir
                        }
                        return nil
                    }
                    let error = errno
                    switch error {
                    case ENOENT where !created:
                        if mkdirat(parent.fd, name, mode_t(0o700)) == 0 {
                            created = true
                        } else if errno != EEXIST {
                            throw PrivateFileError(path: path, operation: "mkdir", code: errno)
                        }
                    // O_NOFOLLOW refuses a symlink with ELOOP (EMLINK on the
                    // BSDs); O_DIRECTORY refuses anything else with ENOTDIR.
                    case ELOOP, EMLINK, ENOTDIR:
                        var link = stat()
                        guard fstatat(parent.fd, name, &link, AT_SYMLINK_NOFOLLOW) == 0 else {
                            throw PrivateFileError(path: path, operation: "stat", code: errno)
                        }
                        guard Int(link.st_mode) & 0o170000 == 0o120000 else {
                            throw PrivateFileError(path: path, operation: "open", code: ENOTDIR)
                        }
                        if owned { try checkOwner(link.st_uid, path: path) }
                        links += 1
                        guard links <= 40 else { throw PrivateFileError(path: path, operation: "open", code: ELOOP) }
                        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX) + 1)
                        let length = readlinkat(parent.fd, name, &buffer, buffer.count)
                        guard length >= 0 else { throw PrivateFileError(path: path, operation: "readlink", code: errno) }
                        guard length < buffer.count else {
                            throw PrivateFileError(path: path, operation: "readlink", code: ENAMETOOLONG)
                        }
                        let bytes = buffer[0..<length].map { UInt8(bitPattern: $0) }
                        guard let target = String(bytes: bytes, encoding: .utf8) else {
                            throw PrivateFileError(path: path, operation: "readlink", code: EILSEQ)
                        }
                        if !parent.safe || replaceable(parent, entryOwner: link.st_uid) { unsafeLink = true }
                        if owned {
                            ownedLink = true
                            notes.append("\(path) is a symlink: its target keeps its mode")
                        }
                        return target
                    default:
                        throw PrivateFileError(path: path, operation: "open", code: error)
                    }
                }
                throw PrivateFileError(path: path, operation: "open", code: EAGAIN)
            }

            /// Gives `dir` the mode `wanted` when it differs and the rules
            /// allow it; notes what it leaves. Whether it changed it.
            private func setMode(_ dir: Dir, _ wanted: Int) throws -> Bool {
                // Below an owned symlink: noted once, when it was followed.
                guard wanted != dir.mode, !ownedLink else { return false }
                guard dir.safe && !unsafeLink else {
                    notes.append(
                        "left \(dir.path) at mode \(octal(dir.mode)): it may not be the app's (another user can "
                            + "replace an entry on the way)")
                    return false
                }
                guard dir.owner == euid else {
                    throw PrivateFileError(path: dir.path, operation: "chmod", code: EPERM)
                }
                // The walk's O_PATH descriptor cannot change a mode: reopen
                // the very same directory through it.
                let writable = openat(dir.fd, ".", O_RDONLY | O_DIRECTORY | O_CLOEXEC)
                guard writable >= 0 else { throw PrivateFileError(path: dir.path, operation: "open", code: errno) }
                defer { close(writable) }
                guard fchmod(writable, mode_t(wanted)) == 0 else {
                    throw PrivateFileError(path: dir.path, operation: "chmod", code: errno)
                }
                return true
            }

            private func trusted(_ uid: uid_t) -> Bool { uid == euid || uid == 0 || uid == rootOwner }

            /// Whether an untrusted user could replace the entry of
            /// `entryOwner` in `dir`.
            private func replaceable(_ dir: Dir, entryOwner: uid_t) -> Bool {
                !trusted(dir.owner) || (dir.mode & 0o022 != 0 && (dir.mode & 0o1000 == 0 || !trusted(entryOwner)))
            }

            /// The app's own directory, or a symlink in its place, is never an
            /// untrusted user's: one could only have been planted while a
            /// directory above was open to others.
            private func checkOwner(_ owner: uid_t, path: String) throws {
                guard trusted(owner) else { throw PrivateFileError(path: path, operation: "open", code: EPERM) }
            }

            private static func describe(_ fd: Int32, path: String) throws -> (mode: Int, owner: uid_t) {
                var info = stat()
                guard fstat(fd, &info) == 0 else { throw PrivateFileError(path: path, operation: "stat", code: errno) }
                return (Int(info.st_mode) & 0o7777, info.st_uid)
            }
        }

        private static func octal(_ mode: Int) -> String {
            let digits = String(mode, radix: 8)
            return String(repeating: "0", count: max(0, 4 - digits.count)) + digits
        }
    #endif
}

#if !os(Windows)
    /// A failed system call on a path.
    public struct PrivateFileError: Error, LocalizedError, CustomStringConvertible {
        public let path: String
        public let operation: String
        public let code: Int32

        public var description: String { "\(path): \(operation) failed: \(String(cString: strerror(code)))" }
        public var errorDescription: String? { description }
    }
#endif
