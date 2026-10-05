// macOS data root (DESIGN-opus §1.7):
// ~/Library/Application Support/org.dashfoundation.DashWallet/
#if os(macOS)
import Foundation
import PlatformServices

public struct MacDataLocation: DataLocating {
    public static let bundleDirectoryName = "org.dashfoundation.DashWallet"

    public init() {}

    public func defaultDataRoot() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        return support.appendingPathComponent(Self.bundleDirectoryName, isDirectory: true)
    }
}
#endif
