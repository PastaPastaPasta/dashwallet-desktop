// QR code drawn from a module matrix (the Rust encoder produces the matrix;
// this view only paints it). One path per code, so GTK draws a single widget
// instead of one per module.
import DesignTokens
import SwiftCrossUI

/// The dark modules of a QR matrix as one path, scaled to the bounds and
/// surrounded by a quiet zone.
public struct QRModulesShape: Shape {
    let size: Int
    let modules: [Bool]
    let quietZone: Int

    /// `modules` is row-major (`modules[y * size + x]`, `true` = dark).
    public init(size: Int, modules: [Bool], quietZone: Int = 4) {
        self.size = size
        self.modules = modules
        self.quietZone = quietZone
    }

    public nonisolated func path(in bounds: Path.Rect) -> Path {
        var path = Path()
        guard size > 0, modules.count == size * size else { return path }
        let total = Double(size + 2 * quietZone)
        let side = min(bounds.width, bounds.height)
        let module = side / total
        let originX = bounds.x + (bounds.width - side) / 2 + Double(quietZone) * module
        let originY = bounds.y + (bounds.height - side) / 2 + Double(quietZone) * module
        for y in 0..<size {
            var x = 0
            while x < size {
                guard modules[y * size + x] else {
                    x += 1
                    continue
                }
                // Merge a horizontal run of dark modules into one rectangle.
                let start = x
                while x < size, modules[y * size + x] { x += 1 }
                path = path.addRectangle(
                    Path.Rect(
                        x: originX + Double(start) * module, y: originY + Double(y) * module,
                        width: Double(x - start) * module, height: module))
            }
        }
        return path
    }
}

/// A square QR code on a white background (dark-on-light in both appearances,
/// as scanners expect).
public struct QRCodeView: View {
    let size: Int
    let modules: [Bool]
    let side: Int

    public init(size: Int, modules: [Bool], side: Int = 220) {
        self.size = size
        self.modules = modules
        self.side = side
    }

    public var body: some View {
        ZStack {
            Rectangle().fill(Color.white)
            QRModulesShape(size: size, modules: modules).fill(Color.black)
        }
        .frame(width: Double(side), height: Double(side))
    }
}
