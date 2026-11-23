// Where the wallet keeps its data (DESIGN-opus §1.7 "Data root per OS").
import Foundation

/// The per-OS default data root: the directory holding one sub-directory
/// per network plus `settings.json` and `global.json`. `--datadir` and the
/// first-run chooser (QT-004) override it in the app.
public protocol DataLocating: Sendable {
    /// The default data root. Throws when the OS gives no user directory.
    func defaultDataRoot() throws -> URL
}

/// A fixed data root (`--datadir`, tests).
public struct FixedDataLocation: DataLocating {
    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    public func defaultDataRoot() throws -> URL {
        root
    }
}
