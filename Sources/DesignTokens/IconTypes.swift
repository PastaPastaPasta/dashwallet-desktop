import Foundation

/// File format of an exported icon.
public enum DashIconFormat: Sendable, Hashable {
    /// Raster PNGs at the listed scale factors (`name@2x.png`, `name@3x.png`; scale 1 has no suffix).
    case png(scales: [Int])
    /// A single SVG file (`name.svg`).
    case svg
    /// A single PDF file (`name.pdf`).
    case pdf
}

/// Describes the exported files of a `DashIconToken`.
public struct DashIconMetadata: Sendable, Hashable {
    public let group: DashIconGroup
    public let format: DashIconFormat
    /// Whether a `-dark` variant exists next to every light file.
    public let hasDarkVariant: Bool
    /// The source imageset is marked "template": a single-colour mask meant to be tinted.
    public let isTemplate: Bool
    /// `<catalog>:<imageset path>` the icon was copied from (`ios` = dashwallet-ios AppAssets.xcassets,
    /// `dashuikit` = DashUIKit Media.xcassets).
    public let source: String

    public init(group: DashIconGroup, format: DashIconFormat, hasDarkVariant: Bool, isTemplate: Bool, source: String) {
        self.group = group
        self.format = format
        self.hasDarkVariant = hasDarkVariant
        self.isTemplate = isTemplate
        self.source = source
    }
}

extension DashIconToken {
    /// File names in `Resources/Icons` for this icon, light files first.
    public var fileNames: [String] {
        let meta = metadata
        let variants = meta.hasDarkVariant ? [rawValue, rawValue + "-dark"] : [rawValue]
        switch meta.format {
        case .svg:
            return variants.map { $0 + ".svg" }
        case .pdf:
            return variants.map { $0 + ".pdf" }
        case .png(let scales):
            return variants.flatMap { base in scales.map { base + ($0 == 1 ? "" : "@\($0)x") + ".png" } }
        }
    }

    /// The best file for the requested appearance and display scale: the variant for that appearance when
    /// one exists, and for PNGs the smallest exported scale that is at least `scale` (else the largest).
    public func fileName(for appearance: DashAppearance, scale: Int = 2) -> String {
        let meta = metadata
        let base = appearance == .dark && meta.hasDarkVariant ? rawValue + "-dark" : rawValue
        switch meta.format {
        case .svg:
            return base + ".svg"
        case .pdf:
            return base + ".pdf"
        case .png(let scales):
            let sorted = scales.sorted()
            let chosen = sorted.first(where: { $0 >= scale }) ?? sorted.last ?? 1
            return base + (chosen == 1 ? "" : "@\(chosen)x") + ".png"
        }
    }
}
