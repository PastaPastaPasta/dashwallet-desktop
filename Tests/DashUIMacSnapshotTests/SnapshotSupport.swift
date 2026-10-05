// Renders SwiftUI views with ImageRenderer and compares them with reference PNGs in __Snapshots__.
#if os(macOS)
import AppKit
import DashUIMac
import SwiftUI
import Testing

/// Pixel buffer of a rendered or decoded image, RGBA8 premultiplied, sRGB.
struct Bitmap: Equatable {
    let width: Int
    let height: Int
    let bytes: [UInt8]

    init?(cgImage: CGImage) {
        width = cgImage.width
        height = cgImage.height
        var buffer = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = buffer.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(
                data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8,
                bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ) else { return false }
            context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return nil }
        bytes = buffer
    }

    init?(pngAt url: URL) {
        guard let data = try? Data(contentsOf: url),
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
        self.init(cgImage: image)
    }

    /// Share of pixels whose largest channel difference exceeds `tolerance` (0...255).
    func differingFraction(from other: Bitmap, tolerance: UInt8 = 12) -> Double {
        precondition(width == other.width && height == other.height)
        var differing = 0
        for pixel in 0..<(width * height) {
            for channel in 0..<4 {
                let a = bytes[pixel * 4 + channel]
                let b = other.bytes[pixel * 4 + channel]
                if (a > b ? a - b : b - a) > tolerance {
                    differing += 1
                    break
                }
            }
        }
        return Double(differing) / Double(width * height)
    }

    /// Number of distinct colours, ignoring the lowest 4 bits of each channel.
    var coarseColorCount: Int {
        var seen = Set<UInt32>()
        for pixel in 0..<(width * height) {
            let base = pixel * 4
            let key = (UInt32(bytes[base] >> 4) << 12) | (UInt32(bytes[base + 1] >> 4) << 8)
                | (UInt32(bytes[base + 2] >> 4) << 4) | UInt32(bytes[base + 3] >> 4)
            seen.insert(key)
        }
        return seen.count
    }
}

enum Snapshot {
    /// Directory of the reference PNGs, next to this file.
    static let directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("__Snapshots__")

    /// Set `DWD_RECORD_SNAPSHOTS=1` to rewrite every reference image instead of comparing.
    static var isRecording: Bool { ProcessInfo.processInfo.environment["DWD_RECORD_SNAPSHOTS"] == "1" }

    /// Renders `view` at `width` points (height fits the content) on the gallery background.
    @MainActor
    static func render<V: View>(_ view: V, width: CGFloat, scheme: ColorScheme) -> CGImage? {
        let content = view
            .frame(width: width, alignment: .leading)
            .padding(16)
            .background(Color.dash.primaryBackground)
            .environment(\.colorScheme, scheme)
        let renderer = ImageRenderer(content: content)
        renderer.scale = 1
        renderer.isOpaque = true
        return renderer.cgImage
    }

    static func write(_ image: CGImage, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let rep = NSBitmapImageRep(cgImage: image)
        guard let data = rep.representation(using: .png, properties: [:]) else {
            throw SnapshotError.encodingFailed
        }
        try data.write(to: url)
    }

    /// Compares `image` with `__Snapshots__/<name>.png`, or records it when recording or when the
    /// reference is missing (the latter also fails the test, so a missing file is noticed).
    static func assertMatches(_ image: CGImage, named name: String, maxDifferingFraction: Double = 0.005,
                              sourceLocation: SourceLocation = #_sourceLocation) throws {
        let url = directory.appendingPathComponent(name + ".png")
        let exists = FileManager.default.fileExists(atPath: url.path)
        if isRecording || !exists {
            try write(image, to: url)
            if !exists && !isRecording {
                Issue.record("No reference for \(name); recorded \(url.lastPathComponent). Re-run to compare.",
                             sourceLocation: sourceLocation)
            }
            return
        }
        guard let actual = Bitmap(cgImage: image), let expected = Bitmap(pngAt: url) else {
            Issue.record("Could not decode \(name)", sourceLocation: sourceLocation)
            return
        }
        guard actual.width == expected.width, actual.height == expected.height else {
            try write(image, to: failureURL(name))
            Issue.record("""
                \(name): size \(actual.width)x\(actual.height) != reference \(expected.width)x\(expected.height); \
                actual written to \(failureURL(name).path)
                """, sourceLocation: sourceLocation)
            return
        }
        let fraction = actual.differingFraction(from: expected)
        if fraction > maxDifferingFraction {
            try write(image, to: failureURL(name))
            Issue.record("""
                \(name): \(String(format: "%.2f", fraction * 100))% of pixels differ; actual written to \
                \(failureURL(name).path)
                """, sourceLocation: sourceLocation)
        }
    }

    private static func failureURL(_ name: String) -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("DashUIMacSnapshots/\(name).actual.png")
    }
}

enum SnapshotError: Error {
    case encodingFailed
}
#endif
