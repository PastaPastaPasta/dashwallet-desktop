import Foundation

/// A gamma-encoded sRGB colour with straight (non-premultiplied) alpha. Every component is in 0...1.
///
/// Toolkit-neutral: SwiftUI, SwiftCrossUI, GTK and WinUI adapters convert from this type.
public struct RGBA: Sendable, Hashable, CustomStringConvertible {
    public let red: Double
    public let green: Double
    public let blue: Double
    public let alpha: Double

    public init(red: Double, green: Double, blue: Double, alpha: Double = 1) {
        self.red = red
        self.green = green
        self.blue = blue
        self.alpha = alpha
    }

    /// Builds a colour from a 24-bit `0xRRGGBB` value.
    public init(hex: UInt32, alpha: Double = 1) {
        self.init(red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  alpha: alpha)
    }

    /// `#RRGGBB`, each channel rounded to the nearest 8-bit value; alpha is not included.
    public var hex: String {
        String(format: "#%02X%02X%02X", Self.byte(red), Self.byte(green), Self.byte(blue))
    }

    /// `#RRGGBBAA`, each channel rounded to the nearest 8-bit value.
    public var hexWithAlpha: String {
        hex + String(format: "%02X", Self.byte(alpha))
    }

    /// Hue in degrees (0..<360), saturation and value in 0...1.
    public var hsv: (hue: Double, saturation: Double, value: Double) {
        let maxC = max(red, green, blue)
        let minC = min(red, green, blue)
        let delta = maxC - minC
        let saturation = maxC == 0 ? 0 : delta / maxC
        var hue = 0.0
        if delta > 0 {
            if maxC == red {
                hue = 60 * ((green - blue) / delta).truncatingRemainder(dividingBy: 6)
            } else if maxC == green {
                hue = 60 * ((blue - red) / delta + 2)
            } else {
                hue = 60 * ((red - green) / delta + 4)
            }
        }
        if hue < 0 { hue += 360 }
        return (hue, saturation, maxC)
    }

    public var description: String {
        alpha == 1 ? hex : hexWithAlpha
    }

    private static func byte(_ value: Double) -> Int {
        Int((min(max(value, 0), 1) * 255).rounded())
    }
}

/// Light or dark appearance.
public enum DashAppearance: Sendable, Hashable, CaseIterable {
    case light
    case dark
}

/// A colour token: one sRGB value per appearance. Colour sets without a dark variant use the
/// same value for both.
public struct DashColor: Sendable, Hashable {
    public let light: RGBA
    public let dark: RGBA

    public init(light: RGBA, dark: RGBA) {
        self.light = light
        self.dark = dark
    }

    /// A token that looks the same in light and dark appearance.
    public init(_ any: RGBA) {
        self.init(light: any, dark: any)
    }

    /// The value for the given appearance.
    public func resolved(for appearance: DashAppearance) -> RGBA {
        switch appearance {
        case .light: light
        case .dark: dark
        }
    }
}
