// macOS data root (DESIGN-opus §1.7):
// ~/Library/Application Support/org.dashfoundation.DashWallet/
#if os(macOS)
import Foundation
import PlatformServices

public struct MacDataLocation: DataLocating {
    public static let bundleDirectoryName = "org.dashfoundation.DashWallet"

    public init() {}

    /// Not created here: `create: true` would make a missing Application
    /// Support with the umask's mode. The caller creates missing components
    /// owner-only (`PrivateFileSystem`).
    public func defaultDataRoot() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: false)
        return support.appendingPathComponent(Self.bundleDirectoryName, isDirectory: true)
    }
}
#endif
