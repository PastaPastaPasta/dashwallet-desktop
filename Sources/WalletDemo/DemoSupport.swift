// Building blocks for the demo services: change fan-out, a lock-guarded
// value, zeroing secret buffers, deterministic pseudo-random data and valid
// base58check addresses for the sample wallets.
import Foundation
import WalletRuntime

/// Fans one value out to every open `AsyncStream`. Callable from any thread.
final class DemoBroadcaster<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<Value>.Continuation] = [:]

    /// A stream that first yields `initial` (when given), then every `send`.
    func stream(initial: Value? = nil) -> AsyncStream<Value> {
        let id = UUID()
        let (stream, continuation) = AsyncStream<Value>.makeStream(bufferingPolicy: .bufferingNewest(32))
        if let initial { continuation.yield(initial) }
        continuation.onTermination = { [weak self] _ in
            guard let self else { return }
            _ = self.lock.withLock { self.continuations.removeValue(forKey: id) }
        }
        lock.withLock { continuations[id] = continuation }
        return stream
    }

    func send(_ value: Value) {
        let targets = lock.withLock { Array(continuations.values) }
        for continuation in targets { continuation.yield(value) }
    }
}

/// A value guarded by a lock, for state that synchronous nonisolated
/// protocol requirements read (the network the formatter and URI handler use).
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

/// A secret buffer that zeroes its bytes on deinit (the live runtime wraps
/// DashKit `SecretBytes`).
final class DemoSecret: SecretBuffer, @unchecked Sendable {
    private var bytes: [UInt8]

    init(utf8 text: String) {
        bytes = Array(text.utf8)
    }

    deinit {
        bytes.withUnsafeMutableBufferPointer { buffer in
            for index in buffer.indices { buffer[index] = 0 }
        }
    }

    var count: Int { bytes.count }

    func withUnsafeBytes<R>(_ body: (UnsafeRawBufferPointer) throws -> R) rethrows -> R {
        try bytes.withUnsafeBytes(body)
    }

    /// The bytes as text; the demo compares passphrases and phrases this way.
    static func text(_ secret: any SecretBuffer) -> String {
        secret.withUnsafeBytes { String(decoding: $0, as: UTF8.self) }
    }
}

/// SplitMix64 seeded from text: deterministic demo txids, addresses and keys,
/// so screenshots and UI tests see the same rows on every run.
struct DemoRandom {
    private var state: UInt64

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

    /// `count` random bytes as lowercase hex (txids, wallet ids).
    mutating func hex(bytes count: Int) -> String {
        bytes(count).map { String(format: "%02x", $0) }.joined()
    }

    /// A pay-to-pubkey-hash address of `network` with a random hash. It is a
    /// valid base58check address (the engine accepts it), but nobody holds
    /// its key.
    mutating func address(on network: DashNetwork) -> String {
        DemoAddress.p2pkh(hash: bytes(20), network: network)
    }
}

/// Dash base58check addresses (dash-qt `CBitcoinAddress`).
enum DemoAddress {
    static let alphabet = Array("123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".utf8)

    /// P2PKH version byte: 76 (`X…`) on mainnet, 140 (`y…`) on the test networks.
    static func p2pkh(hash: [UInt8], network: DashNetwork) -> String {
        let version: UInt8 = network == .mainnet ? 76 : 140
        let payload = [version] + hash
        let checksum = SHA256.hash(SHA256.hash(payload)).prefix(4)
        return base58(payload + checksum)
    }

    static func base58(_ bytes: [UInt8]) -> String {
        var digits: [UInt8] = []
        for byte in bytes {
            var carry = Int(byte)
            for index in digits.indices {
                carry += Int(digits[index]) << 8
                digits[index] = UInt8(carry % 58)
                carry /= 58
            }
            while carry > 0 {
                digits.append(UInt8(carry % 58))
                carry /= 58
            }
        }
        let zeros = bytes.prefix { $0 == 0 }.count
        let encoded = [UInt8](repeating: alphabet[0], count: zeros) + digits.reversed().map { alphabet[Int($0)] }
        return String(decoding: encoded, as: UTF8.self)
    }
}

/// FIPS 180-4 SHA-256, for address checksums only (Foundation has no hash on Linux).
enum SHA256 {
    private static let k: [UInt32] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ]

    static func hash(_ data: [UInt8]) -> [UInt8] {
        var h: [UInt32] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
        ]
        var message = data
        let bitLength = UInt64(data.count) * 8
        message.append(0x80)
        while message.count % 64 != 56 { message.append(0) }
        for shift in stride(from: 56, through: 0, by: -8) {
            message.append(UInt8(truncatingIfNeeded: bitLength >> UInt64(shift)))
        }
        func rotr(_ x: UInt32, _ n: UInt32) -> UInt32 { (x >> n) | (x << (32 - n)) }
        var w = [UInt32](repeating: 0, count: 64)
        for chunk in stride(from: 0, to: message.count, by: 64) {
            for i in 0..<16 {
                let j = chunk + 4 * i
                w[i] = UInt32(message[j]) << 24 | UInt32(message[j + 1]) << 16 | UInt32(message[j + 2]) << 8
                    | UInt32(message[j + 3])
            }
            for i in 16..<64 {
                let s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3)
                let s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10)
                w[i] = w[i - 16] &+ s0 &+ w[i - 7] &+ s1
            }
            var a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7]
            for i in 0..<64 {
                let t1 = hh &+ (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) &+ ((e & f) ^ (~e & g)) &+ k[i] &+ w[i]
                let t2 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) &+ ((a & b) ^ (a & c) ^ (b & c))
                hh = g
                g = f
                f = e
                e = d &+ t1
                d = c
                c = b
                b = a
                a = t1 &+ t2
            }
            h[0] &+= a
            h[1] &+= b
            h[2] &+= c
            h[3] &+= d
            h[4] &+= e
            h[5] &+= f
            h[6] &+= g
            h[7] &+= hh
        }
        return h.flatMap { word in (0..<4).map { UInt8(truncatingIfNeeded: word >> UInt32(24 - 8 * $0)) } }
    }
}

extension ServiceError {
    /// An error with the engine's code; `detail` marks it as the demo's.
    static func demo(
        _ code: ServiceErrorCode, _ detail: String = "demo", recipient: Int? = nil, retryAfter: UInt64? = nil,
        parameters: [String: Int64] = [:]
    ) -> ServiceError {
        var parameters = parameters
        if let recipient { parameters["index"] = Int64(recipient) }
        return ServiceError(
            code: code, detail: detail, recipientIndex: recipient, retryAfterSeconds: retryAfter, parameters: parameters)
    }
}
