// The exported icon set (Resources/Icons, `DashIconToken`) as SwiftCrossUI
// images (UX-SPEC §2.7). SwiftCrossUI 0.10 draws PNG, not SVG, and cannot
// tint an image, so this file decodes the PNG once, recolours it when asked
// (template icons such as the Dash currency glyph) and caches the result.
import DesignTokens
import Foundation
import ImageFormats
import SwiftCrossUI

/// How an icon's pixels are coloured.
public enum IconTint: Sendable, Hashable {
    /// The exported colours (light or `-dark` file by appearance).
    case original
    /// Every pixel takes the colour; the alpha channel keeps the shape. Used for
    /// template glyphs and for icons on the blue hero or a selected row.
    case color(DashColor)
}

/// An exported icon at a fixed size. Icons are never the only content of a
/// control on Cross (ADR 0002 A1): callers put text next to them.
public struct DashIcon: View {
    let token: DashIconToken
    let width: Double
    let height: Double
    let tint: IconTint
    let flipped: Bool

    @Environment(\.colorScheme) var colorScheme

    /// `size` is the icon's height; the width follows the file's aspect ratio
    /// unless `width` is given.
    public init(_ token: DashIconToken, size: Double, width: Double? = nil, tint: IconTint = .original, flipped: Bool = false) {
        self.token = token
        self.height = size
        self.tint = tint
        self.flipped = flipped
        if let width {
            self.width = width
        } else if let aspect = IconCache.aspectRatio(of: token) {
            self.width = (size * aspect).rounded()
        } else {
            self.width = size
        }
    }

    public var body: some View {
        let appearance: DashAppearance = colorScheme == .dark ? .dark : .light
        if let image = IconCache.image(token, appearance: appearance, tint: tint, flipped: flipped) {
            Image(image).resizable().frame(width: width, height: height)
        } else {
            // A missing resource is a packaging bug; keep the layout stable.
            Spacer().frame(width: width, height: height)
        }
    }
}

/// Decoded (and tinted) icons, keyed by file and tint. The same array
/// instance is returned for a key, so SwiftCrossUI's image-change check is
/// cheap.
@MainActor
enum IconCache {
    private struct Key: Hashable {
        let file: String
        let tint: RGBAKey?
        let flipped: Bool
    }

    private struct RGBAKey: Hashable {
        let red: UInt8, green: UInt8, blue: UInt8, alpha: UInt8
    }

    private static var images: [Key: ImageFormats.Image<ImageFormats.RGBA>] = [:]
    private static var missing: Set<String> = []

    static func aspectRatio(of token: DashIconToken) -> Double? {
        guard let image = image(token, appearance: .light, tint: .original, flipped: false), image.height > 0 else {
            return nil
        }
        return Double(image.width) / Double(image.height)
    }

    static func image(
        _ token: DashIconToken, appearance: DashAppearance, tint: IconTint, flipped: Bool
    ) -> ImageFormats.Image<ImageFormats.RGBA>? {
        let file = token.fileName(for: appearance, scale: 2)
        // A tinted icon uses the light file: only the alpha channel matters.
        let tintColor: DesignTokens.RGBA?
        switch tint {
        case .original: tintColor = nil
        case .color(let color): tintColor = color.resolved(for: appearance)
        }
        let sourceFile = tintColor == nil ? file : token.fileName(for: .light, scale: 2)
        let key = Key(file: sourceFile, tint: tintColor.map(byteKey), flipped: flipped)
        if let cached = images[key] { return cached }
        guard let decoded = decode(sourceFile) else { return nil }
        var result = decoded
        if let tintColor { result = tinted(result, with: tintColor) }
        if flipped { result = verticallyFlipped(result) }
        images[key] = result
        return result
    }

    private static func decode(_ file: String) -> ImageFormats.Image<ImageFormats.RGBA>? {
        let plain = Key(file: file, tint: nil, flipped: false)
        if let cached = images[plain] { return cached }
        guard !missing.contains(file) else { return nil }
        let name = (file as NSString).deletingPathExtension
        let ext = (file as NSString).pathExtension
        guard
            let url = Bundle.module.url(forResource: name, withExtension: ext),
            let data = try? Data(contentsOf: url),
            let image = try? ImageFormats.Image<ImageFormats.RGBA>.load(from: Array(data))
        else {
            missing.insert(file)
            return nil
        }
        images[plain] = image
        return image
    }

    private static func byteKey(_ color: DesignTokens.RGBA) -> RGBAKey {
        RGBAKey(red: byte(color.red), green: byte(color.green), blue: byte(color.blue), alpha: byte(color.alpha))
    }

    private static func byte(_ value: Double) -> UInt8 {
        UInt8((min(max(value, 0), 1) * 255).rounded())
    }

    /// Replaces the colour of every pixel, multiplying alpha by the tint's alpha.
    static func tinted(
        _ image: ImageFormats.Image<ImageFormats.RGBA>, with color: DesignTokens.RGBA
    ) -> ImageFormats.Image<ImageFormats.RGBA> {
        let key = byteKey(color)
        var bytes = image.bytes
        var index = 0
        while index + 3 < bytes.count {
            bytes[index] = key.red
            bytes[index + 1] = key.green
            bytes[index + 2] = key.blue
            bytes[index + 3] = UInt8((Int(bytes[index + 3]) * Int(key.alpha) + 127) / 255)
            index += 4
        }
        return ImageFormats.Image(width: image.width, height: image.height, bytes: bytes)
    }

    static func verticallyFlipped(_ image: ImageFormats.Image<ImageFormats.RGBA>) -> ImageFormats.Image<ImageFormats.RGBA> {
        let rowBytes = image.width * 4
        var bytes = [UInt8]()
        bytes.reserveCapacity(image.bytes.count)
        for row in stride(from: image.height - 1, through: 0, by: -1) {
            bytes.append(contentsOf: image.bytes[(row * rowBytes)..<((row + 1) * rowBytes)])
        }
        return ImageFormats.Image(width: image.width, height: image.height, bytes: bytes)
    }
}
