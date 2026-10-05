// Loads the exported icon set (Resources/Icons, copied into this module's resource bundle) as NSImages.
#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI

/// Reads `DashIconToken` files from a directory and caches the decoded images.
///
/// The default library reads the icons bundled with DashUIMac. Views read the library from the
/// environment (`\.dashIconLibrary`), so a host can point it at another directory.
public final class DashIconLibrary: @unchecked Sendable {
    /// Directory that holds the files named by `DashIconToken.fileNames`, or nil when the icon set
    /// is not available; every lookup then returns nil.
    public let directory: URL?
    // NSCache is thread-safe; it is the only mutable state.
    private let cache = NSCache<NSString, NSImage>()

    public init(directory: URL?) {
        self.directory = directory
    }

    /// The icons shipped in DashUIMac's resource bundle. SwiftPM's `.process` rule flattens
    /// Resources/Icons into the bundle's resource directory.
    public static let bundled = DashIconLibrary(directory: Bundle.module.resourceURL)

    /// The icon for an appearance, or nil when its files are missing.
    ///
    /// PNG icons carry @2x and @3x files; both become representations of one image whose point
    /// size is the @2x pixel size divided by two.
    public func image(for token: DashIconToken, appearance: DashAppearance) -> NSImage? {
        guard let directory else { return nil }
        let variant = appearance == .dark && token.metadata.hasDarkVariant ? "-dark" : ""
        let key = (token.rawValue + variant) as NSString
        if let cached = cache.object(forKey: key) { return cached }

        let image: NSImage?
        switch token.metadata.format {
        case .svg, .pdf:
            image = NSImage(contentsOf: directory.appendingPathComponent(token.fileName(for: appearance)))
        case .png(let scales):
            image = Self.loadPNG(token: token, variant: variant, scales: scales, directory: directory)
        }
        if let image { cache.setObject(image, forKey: key) }
        return image
    }

    private static func loadPNG(token: DashIconToken, variant: String, scales: [Int], directory: URL) -> NSImage? {
        var pointSize: NSSize?
        var reps: [NSImageRep] = []
        for scale in scales.sorted() {
            let name = token.rawValue + variant + (scale == 1 ? "" : "@\(scale)x") + ".png"
            guard let data = try? Data(contentsOf: directory.appendingPathComponent(name)),
                  let rep = NSBitmapImageRep(data: data) else { continue }
            let size = NSSize(width: CGFloat(rep.pixelsWide) / CGFloat(scale),
                              height: CGFloat(rep.pixelsHigh) / CGFloat(scale))
            rep.size = size
            pointSize = pointSize ?? size
            reps.append(rep)
        }
        guard let pointSize, !reps.isEmpty else { return nil }
        let image = NSImage(size: pointSize)
        image.addRepresentations(reps)
        image.isTemplate = token.metadata.isTemplate
        return image
    }
}

private struct DashIconLibraryKey: EnvironmentKey {
    static let defaultValue = DashIconLibrary.bundled
}

public extension EnvironmentValues {
    /// Where DashUIMac views load `DashIconToken` images from.
    var dashIconLibrary: DashIconLibrary {
        get { self[DashIconLibraryKey.self] }
        set { self[DashIconLibraryKey.self] = newValue }
    }
}
#endif
