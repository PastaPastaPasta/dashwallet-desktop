@testable import DashKit
import Foundation
import Testing

@Suite struct SecretBytesTests {
    @Test func holdsTheBytesItWasGiven() {
        let secret = SecretBytes(utf8: "correct horse")
        #expect(secret.count == 13)
        #expect(!secret.isEmpty)
        #expect(secret.withUnsafeBytes { Array($0) } == Array("correct horse".utf8))
        #expect(secret.utf8String() == "correct horse")
        #expect(SecretBytes([]).isEmpty)
    }

    @Test func descriptionNeverShowsTheContent() {
        let secret = SecretBytes(utf8: "hunter2")
        #expect(secret.description == "SecretBytes(7 bytes)")
        #expect(!"\(secret)".contains("hunter2"))
    }

    @Test func consumingWipesTheSourceData() {
        var data = Data("abandon about".utf8)
        let secret = SecretBytes(consuming: &data)
        #expect(secret.utf8String() == "abandon about")
        #expect(data.count == 13)
        #expect(data.allSatisfy { $0 == 0 })
    }

    @Test func temporaryDataIsACopy() async {
        let secret = SecretBytes(utf8: "pw")
        let seen = await secret.withTemporaryData { data in Array(data) }
        #expect(seen == Array("pw".utf8))
        #expect(secret.utf8String() == "pw")
    }

    @Test func secureZeroOverwritesEveryByte() {
        let buffer = UnsafeMutableRawBufferPointer.allocate(byteCount: 64, alignment: 1)
        defer { buffer.deallocate() }
        buffer.initializeMemory(as: UInt8.self, repeating: 0xA5)
        secureZero(buffer)
        #expect(buffer.allSatisfy { $0 == 0 })
        // An empty buffer is a no-op.
        secureZero(UnsafeMutableRawBufferPointer(start: nil, count: 0))
    }
}
