// Draws a QR code from a module matrix computed elsewhere (the Rust core's `qr_matrix`).
#if os(macOS)
import DesignTokens
import SwiftUI

/// A QR code drawn from its modules: `modules[y * size + x]`, `true` = dark.
///
/// The view adds a light quiet zone around the code, keeps it square, and scales it to the
/// space it is given. Dark modules stay dark on light in both appearances so scanners read it.
/// A matrix whose module count is not `size * size` is shown as "QR code unavailable".
public struct QRView: View {
    public let size: Int
    public let modules: [Bool]
    /// Width of the light border in modules (the QR spec asks for 4).
    public let quietZone: Int
    public let accessibilityLabel: String

    public init(size: Int, modules: [Bool], quietZone: Int = 4, accessibilityLabel: String) {
        self.size = size
        self.modules = modules
        self.quietZone = max(quietZone, 0)
        self.accessibilityLabel = accessibilityLabel
    }

    /// Whether the matrix is a complete square.
    public var isValid: Bool { size > 0 && modules.count == size * size }

    public var body: some View {
        Group {
            if isValid {
                Canvas(rendersAsynchronously: false) { context, canvasSize in
                    draw(in: &context, canvasSize: canvasSize)
                }
                .accessibilityElement()
                .accessibilityLabel(Text(accessibilityLabel))
                .accessibilityAddTraits(.isImage)
            } else {
                unavailable
            }
        }
        .aspectRatio(1, contentMode: .fit)
    }

    private func draw(in context: inout GraphicsContext, canvasSize: CGSize) {
        let side = min(canvasSize.width, canvasSize.height)
        let count = size + 2 * quietZone
        let module = side / CGFloat(count)
        let origin = CGPoint(x: (canvasSize.width - side) / 2, y: (canvasSize.height - side) / 2)

        context.fill(Path(CGRect(origin: origin, size: CGSize(width: side, height: side))), with: .color(Self.light))

        var path = Path()
        for y in 0..<size {
            for x in 0..<size where modules[y * size + x] {
                path.addRect(CGRect(
                    x: origin.x + CGFloat(x + quietZone) * module,
                    y: origin.y + CGFloat(y + quietZone) * module,
                    width: module,
                    height: module))
            }
        }
        // Antialiasing off so neighbouring modules do not leave hairline seams.
        context.fill(path, with: .color(Self.dark), style: FillStyle(antialiased: false))
    }

    private var unavailable: some View {
        RoundedRectangle(cornerRadius: DashRadius.standard, style: .continuous)
            .fill(Color.dash.gray300Alpha10)
            .overlay(
                Text(NSLocalizedString("QR code unavailable", bundle: .module, comment: "QRView"))
                    .font(DashTextStyle.footnote.font)
                    .foregroundStyle(Color.dash.secondaryText)
                    .multilineTextAlignment(.center)
                    .padding(DashSpacing.s)
            )
            .accessibilityElement(children: .combine)
    }

    private static let dark = Color(dash: DashColor.black.resolved(for: .light))
    private static let light = Color(dash: DashColor.white.resolved(for: .light))
}
#endif
