// Small building blocks for the demo services: change fan-out, zeroing
// secret buffers and deterministic pseudo-random data.
#if os(macOS)
import Foundation
import WalletRuntime

/// Fans one value out to every open `AsyncStream`. Callable from any thread.
final class DemoBroadcaster<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<Value>.Continuation] = [:]

    /// A stream that first yields `initial` (when given), then every `send`.
    func stream(initial: Value? = nil) -> AsyncStream<Value> {
        let id = UUID()
        let (stream, continuation) = AsyncStream<Value>.makeStream(bufferingPolicy: .bufferingNewest(8))
        if let initial { continuation.yield(initial) }
        continuation.onTermination = { [weak self] _ in
            guard let self else { return }
            self.lock.withLock { self.continuations[id] = nil }
        }
        lock.withLock { continuations[id] = continuation }
        return stream
    }

    func send(_ value: Value) {
        let targets = lock.withLock { Array(continuations.values) }
        for continuation in targets { continuation.yield(value) }
    }
}

/// A value guarded by a lock, for state that sync nonisolated protocol
/// requirements read (the active network of the formatter and URI handler).
final class DemoLocked<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value

    init(_ value: Value) {
        self.value = value
    }

    var current: Value {
        get { lock.withLock { value } }
        set { lock.withLock { value = newValue } }
    }
}

/// A secret buffer that zeroes its bytes on deinit.
final class DemoSecret: SecretBuffer, @unchecked Sendable {
    private var bytes: [UInt8]

    init(utf8 text: String) {
        bytes = Array(text.utf8)
    }

    deinit {
        for index in bytes.indices { bytes[index] = 0 }
    }

    var count: Int { bytes.count }

    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try bytes.withUnsafeBytes(body)
    }

    /// The bytes as text; demo code only.
    var text: String { String(decoding: bytes, as: UTF8.self) }
}

/// SplitMix64: deterministic demo txids, addresses and signatures.
struct DemoRandom {
    private var state: UInt64

    init(seed: UInt64) {
        state = seed
    }

    /// FNV-1a of `text`, a stable seed (Swift's `Hasher` is per-process).
    init(text: String) {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in text.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        state = hash
    }

    mutating func next() -> UInt64 {
        state &+= 0x9e37_79b9_7f4a_7c15
        var z = state
        z = (z ^ (z >> 30)) &* 0xbf58_476d_1ce4_e5b9
        z = (z ^ (z >> 27)) &* 0x94d0_49bb_1331_11eb
        return z ^ (z >> 31)
    }

    mutating func bytes(_ count: Int) -> [UInt8] {
        (0..<count).map { _ in UInt8(truncatingIfNeeded: next()) }
    }

    mutating func hex(bytes count: Int) -> String {
        bytes(count).map { String(format: "%02x", $0) }.joined()
    }

    static let base58 = Array("123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz")

    /// A 34-character string that looks like a testnet P2PKH address.
    mutating func testnetAddress() -> String {
        "y" + String((0..<33).map { _ in Self.base58[Int(next() % UInt64(Self.base58.count))] })
    }
}

extension ServiceError {
    static func demo(_ code: ServiceErrorCode, _ detail: String = "demo", recipient: Int? = nil) -> ServiceError {
        ServiceError(code: code, detail: detail, recipientIndex: recipient)
    }
}
#endif
