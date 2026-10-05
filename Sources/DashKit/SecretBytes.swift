import Foundation

/// Secret material held in Swift only transiently (DESIGN-opus §1.5 rule 5).
///
/// The bytes live in one heap allocation this object owns; nothing is copied
/// into a Swift `Array` or `String` unless the caller asks (`utf8String()`).
/// `deinit` overwrites the allocation with `secureZero`, which the optimizer
/// cannot remove, before freeing it. `description` never shows the content.
public final class SecretBytes: @unchecked Sendable, CustomStringConvertible {
    // Written only in init and deinit; reads go through `withUnsafeBytes`.
    private let storage: UnsafeMutableRawBufferPointer

    /// Allocates `count` bytes and lets `fill` write them.
    private init(count: Int, fill: (UnsafeMutableRawBufferPointer) -> Void) {
        // Allocate at least one byte so `baseAddress` is never nil.
        storage = .allocate(byteCount: max(count, 1), alignment: 1)
        storage.initializeMemory(as: UInt8.self, repeating: 0)
        self.count = count
        fill(UnsafeMutableRawBufferPointer(rebasing: storage[0..<count]))
    }

    /// Copies `bytes` into a new zeroing buffer. The source is not wiped.
    public convenience init(copying bytes: UnsafeRawBufferPointer) {
        self.init(count: bytes.count) { $0.copyMemory(from: bytes) }
    }

    /// Copies `bytes`. The caller's array is not wiped; prefer
    /// `init(consuming:)` for `Data` received from the engine.
    public convenience init(_ bytes: [UInt8]) {
        self.init(count: bytes.count) { dst in bytes.withUnsafeBytes { dst.copyMemory(from: $0) } }
    }

    /// Copies `data` and then overwrites `data` with zeros, so the secret
    /// survives only in this object. Used for bytes returned by the engine.
    public convenience init(consuming data: inout Data) {
        // Read `data` in place (no second reference), so the wipe below
        // overwrites the buffer the bytes came in, not a copy-on-write clone.
        self.init(count: data.count) { dst in data.withUnsafeBytes { dst.copyMemory(from: $0) } }
        data.withUnsafeMutableBytes { secureZero($0) }
    }

    /// Copies the UTF-8 bytes of `string`. The caller's `String` is not wiped;
    /// convert as early as possible and let the string go out of scope.
    public convenience init(utf8 string: String) {
        let utf8 = string.utf8
        self.init(count: utf8.count) { dst in
            for (i, byte) in utf8.enumerated() {
                dst[i] = byte
            }
        }
    }

    /// Number of secret bytes.
    public let count: Int

    public var isEmpty: Bool { count == 0 }

    /// Borrows the bytes without copying them out. `body` must not let the
    /// pointer escape.
    public func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try body(UnsafeRawBufferPointer(rebasing: storage[0..<count]))
    }

    /// Runs `body` with a `Data` copy of the bytes (what the generated
    /// bindings take) and zeroes that copy when `body` returns or throws.
    public func withTemporaryData<R>(_ body: (Data) async throws -> R) async rethrows -> R {
        var data = withUnsafeBytes { Data($0) }
        defer { data.withUnsafeMutableBytes { secureZero($0) } }
        return try await body(data)
    }

    /// Decodes the bytes as UTF-8 for transient display. The returned `String`
    /// is not zeroed; keep its lifetime as short as possible.
    public func utf8String() -> String? {
        withUnsafeBytes { String(bytes: $0, encoding: .utf8) }
    }

    public var description: String { "SecretBytes(\(count) bytes)" }

    deinit {
        secureZero(storage)
        storage.deallocate()
    }
}

/// Overwrites `buffer` with zeros in a way the optimizer cannot elide.
///
/// On Apple platforms this is `memset_s`, which C11 Annex K guarantees is not
/// removed. Elsewhere the loop runs in a function built without optimization
/// and never inlined, so the stores cannot be proven dead and dropped.
@inline(never)
func secureZero(_ buffer: UnsafeMutableRawBufferPointer) {
    guard let base = buffer.baseAddress, buffer.count > 0 else { return }
    #if canImport(Darwin)
    _ = memset_s(base, buffer.count, 0, buffer.count)
    #else
    unoptimizedZero(base, buffer.count)
    #endif
}

#if !canImport(Darwin)
@inline(never)
@_optimize(none)
private func unoptimizedZero(_ base: UnsafeMutableRawPointer, _ count: Int) {
    let bytes = base.assumingMemoryBound(to: UInt8.self)
    for i in 0..<count {
        bytes[i] = 0
    }
}
#endif
