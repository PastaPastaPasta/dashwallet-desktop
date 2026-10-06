// QR modules from Core Image's generator. Demo mode uses it in place of the
// engine's `qr_matrix` (the real app gets its matrices from Rust dw-uri).
#if os(macOS)
import CoreImage
import Foundation

public enum MacQRCodeEncoder {
    /// Row-major modules (`true` = dark) without the quiet zone, or `nil`
    /// when Core Image cannot encode `text`. Error correction level M, as
    /// dash-qt uses.
    public static func modules(for text: String) -> (size: Int, modules: [Bool])? {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let image = filter.outputImage else { return nil }
        let extent = image.extent.integral
        let width = Int(extent.width)
        let height = Int(extent.height)
        guard width > 0, width == height else { return nil }
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        let context = CIContext(options: [.useSoftwareRenderer: true])
        context.render(
            image, toBitmap: &pixels, rowBytes: width * 4, bounds: extent, format: .RGBA8,
            colorSpace: CGColorSpaceCreateDeviceRGB())
        // Core Image adds a one-module light border on each side.
        let border = 1
        let size = width - 2 * border
        guard size > 0 else { return nil }
        var modules = [Bool](repeating: false, count: size * size)
        for y in 0..<size {
            for x in 0..<size {
                // Bitmap rows run top to bottom.
                let offset = ((y + border) * width + (x + border)) * 4
                modules[y * size + x] = pixels[offset] < 128
            }
        }
        return (size, modules)
    }
}
#endif
