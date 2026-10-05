import Foundation

/// Secret material held in Swift only transiently (DESIGN-opus §1.5 rule 5).
/// The buffer is overwritten with zeros on deinit. It has no `description`
/// that reveals content.
public final class SecretBytes: @unchecked Sendable, CustomStringConvertible {
    // Mutated only in deinit, after the last reference is gone.
    private var storage: [UInt8]

    public init(_ bytes: [UInt8]) {
        storage = bytes
    }

    /// Copies the UTF-8 bytes of `string`. The caller's `String` is not wiped;
    /// convert as early as possible and let the string go out of scope.
    public convenience init(utf8 string: String) {
        self.init(Array(string.utf8))
    }

    public var count: Int { storage.count }

    /// Borrow the bytes without copying them out.
    public func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try storage.withUnsafeBytes(body)
    }

    /// Decodes the bytes as UTF-8 for transient display. The returned `String`
    /// is not zeroed; keep its lifetime as short as possible.
    public func utf8String() -> String? {
        String(bytes: storage, encoding: .utf8)
    }

    public var description: String { "SecretBytes(\(storage.count) bytes)" }

    deinit {
        // Best-effort wipe: Swift offers no guaranteed non-elidable memset, so
        // this overwrites the buffer in place before it is released.
        storage.withUnsafeMutableBytes { raw in
            for i in raw.indices {
                raw[i] = 0
            }
        }
    }
}
