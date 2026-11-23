#!/usr/bin/env swift
// gen-tokens.swift — generates the DesignTokens sources, tokens.json and the exported icon set.
//
// Usage:
//   swift scripts/gen-tokens.swift <dashwallet-ios-repo> <DashUIKit-repo> <dashwallet-desktop-repo>
//
// Inputs:
//   <DashUIKit>/Sources/DashUIKit/Resources/Media.xcassets       colour sets -> DashColor.<token>
//   <dashwallet-ios>/Shared/Resources/SharedAssets.xcassets     colour sets -> DashColor.App.<token>
//   <DashUIKit>/Sources/DashUIKit/Foundation/DashTextStyle.swift type scale -> tokens.json "typography"
//   <out>/scripts/icon-manifest.json                             curated icon list
//   <dashwallet-ios>/DashWallet/Resources/AppAssets.xcassets     icon source ("ios")
//   <DashUIKit>/Sources/DashUIKit/Resources/Media.xcassets       icon source ("dashuikit")
//
// Outputs (all paths relative to <out>):
//   Sources/DesignTokens/Generated/Colors.swift
//   Sources/DesignTokens/Generated/Icons.swift
//   Resources/Tokens/tokens.json
//   Resources/Icons/*  (+ Resources/Icons/NOTICE)
//
// Colour rules:
//   - Components may be hex ("0x8D"), floats ("0.553") or 8-bit integers ("112"); alpha is always a float
//     (or hex). These are the forms Xcode writes and actool accepts.
//   - display-p3, gray and linear colour spaces are converted to gamma-encoded sRGB.
//   - light = the "light" luminosity entry, else the appearance-less entry; dark = the "dark" entry, else light.
//   - When one catalog holds two colour sets with the same name, the first one in sorted depth-first
//     order wins. That matches the asset actool ships (checked with Xcode 26.6 on SharedAssets "Black").
//   - Purple-ish colours (HSV hue 260...320 with saturation > 0.25) and colour sets named purple, violet,
//     indigo, lavender, magenta, lilac, plum or mauve are excluded and listed in tokens.json "excluded".
//
// The script exits non-zero on any input it does not understand instead of guessing.

import Foundation
#if canImport(ImageIO)
import CoreGraphics
import ImageIO
#endif

// MARK: - Diagnostics

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("gen-tokens: error: \(message)\n".utf8))
    exit(1)
}

func note(_ message: String) {
    FileHandle.standardError.write(Data("gen-tokens: \(message)\n".utf8))
}

// MARK: - Arguments

let arguments = CommandLine.arguments
guard arguments.count == 4 else {
    fail("usage: swift scripts/gen-tokens.swift <dashwallet-ios-repo> <DashUIKit-repo> <output-repo-root>")
}
let iosRepo = URL(fileURLWithPath: arguments[1]).standardizedFileURL
let uiKitRepo = URL(fileURLWithPath: arguments[2]).standardizedFileURL
let outRoot = URL(fileURLWithPath: arguments[3]).standardizedFileURL

let fileManager = FileManager.default

func requireDirectory(_ url: URL) {
    var isDirectory: ObjCBool = false
    guard fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory), isDirectory.boolValue else {
        fail("missing directory \(url.path)")
    }
}

let uiKitMedia = uiKitRepo.appendingPathComponent("Sources/DashUIKit/Resources/Media.xcassets")
let iosShared = iosRepo.appendingPathComponent("Shared/Resources/SharedAssets.xcassets")
let iosAppAssets = iosRepo.appendingPathComponent("DashWallet/Resources/AppAssets.xcassets")
let uiKitTextStyle = uiKitRepo.appendingPathComponent("Sources/DashUIKit/Foundation/DashTextStyle.swift")
let iconManifestURL = outRoot.appendingPathComponent("scripts/icon-manifest.json")
let generatedDir = outRoot.appendingPathComponent("Sources/DesignTokens/Generated")
let tokensDir = outRoot.appendingPathComponent("Resources/Tokens")
let iconsDir = outRoot.appendingPathComponent("Resources/Icons")

for dir in [uiKitMedia, iosShared, iosAppAssets] { requireDirectory(dir) }

// MARK: - Minimal ordered JSON writer (deterministic output, shortest round-trip doubles)

indirect enum JSONValue {
    case object([(String, JSONValue)])
    case array([JSONValue])
    case string(String)
    case number(Double)
    case integer(Int)
    case bool(Bool)
    case null

    func render(indent: Int = 0) -> String {
        let pad = String(repeating: "  ", count: indent)
        let inner = String(repeating: "  ", count: indent + 1)
        switch self {
        case .object(let pairs):
            if pairs.isEmpty { return "{}" }
            let body = pairs.map { "\(inner)\(JSONValue.quote($0.0)): \($0.1.render(indent: indent + 1))" }
            return "{\n" + body.joined(separator: ",\n") + "\n\(pad)}"
        case .array(let items):
            if items.isEmpty { return "[]" }
            let body = items.map { "\(inner)\($0.render(indent: indent + 1))" }
            return "[\n" + body.joined(separator: ",\n") + "\n\(pad)]"
        case .string(let s): return JSONValue.quote(s)
        case .number(let d): return swiftDouble(d)
        case .integer(let i): return String(i)
        case .bool(let b): return b ? "true" : "false"
        case .null: return "null"
        }
    }

    static func quote(_ s: String) -> String {
        var out = "\""
        for scalar in s.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\t": out += "\\t"
            case "\r": out += "\\r"
            default:
                if scalar.value < 0x20 {
                    out += String(format: "\\u%04x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        return out + "\""
    }
}

/// Shortest round-trip decimal form, always with a fractional part ("1.0", "0.5529411764705883").
func swiftDouble(_ d: Double) -> String {
    let s = "\(d)"
    return s.contains(".") || s.contains("e") ? s : s + ".0"
}

// MARK: - Colour model

struct SRGB: Equatable {
    var r: Double
    var g: Double
    var b: Double
    var a: Double

    var hex: String {
        func byte(_ v: Double) -> Int { Int((min(max(v, 0), 1) * 255).rounded()) }
        return String(format: "#%02X%02X%02X", byte(r), byte(g), byte(b))
    }

    /// HSV with hue in degrees 0..<360 and saturation/value in 0...1.
    var hsv: (h: Double, s: Double, v: Double) {
        let maxC = max(r, g, b), minC = min(r, g, b), delta = maxC - minC
        let s = maxC == 0 ? 0 : delta / maxC
        var h = 0.0
        if delta > 0 {
            if maxC == r {
                h = 60 * ((g - b) / delta).truncatingRemainder(dividingBy: 6)
            } else if maxC == g {
                h = 60 * ((b - r) / delta + 2)
            } else {
                h = 60 * ((r - g) / delta + 4)
            }
        }
        if h < 0 { h += 360 }
        return (h, s, maxC)
    }

    var isPurpleish: Bool {
        let (h, s, _) = hsv
        return s > 0.25 && h >= 260 && h <= 320
    }
}

/// sRGB transfer curve, mirrored for negative input so out-of-gamut values stay out of 0...1 (they are
/// clamped and reported later by `parseColor`).
func srgbEncode(_ linear: Double) -> Double {
    let x = abs(linear)
    let encoded = x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1 / 2.4) - 0.055
    return linear < 0 ? -encoded : encoded
}

func srgbDecode(_ encoded: Double) -> Double {
    encoded <= 0.04045 ? encoded / 12.92 : pow((encoded + 0.055) / 1.055, 2.4)
}

/// Parses one colour component string. Hex ("0x8D") and integer ("112") forms are 8-bit values; a decimal
/// point marks a float in 0...1. For alpha the integers "0" and "1" mean transparent and opaque.
func parseComponent(_ raw: Any, isAlpha: Bool, context: String) -> Double {
    // Xcode writes components as strings. A bare JSON number is rejected: after parsing, 1 and 1.0 are the
    // same value, so it cannot be classified as an 8-bit integer or a float.
    guard let string = raw as? String else {
        fail("\(context): component \(raw) is not a string; write it as Xcode does (\"0x8D\", \"0.553\", \"141\")")
    }
    let s = string.trimmingCharacters(in: .whitespaces)
    if s.lowercased().hasPrefix("0x") {
        guard let v = Int(s.dropFirst(2), radix: 16), (0...255).contains(v) else {
            fail("\(context): bad hex component \(s)")
        }
        return Double(v) / 255
    }
    if s.contains(".") {
        guard let d = Double(s) else { fail("\(context): bad float component \(s)") }
        return d
    }
    guard let v = Int(s) else { fail("\(context): unrecognised component \(s)") }
    if isAlpha {
        guard v == 0 || v == 1 else { fail("\(context): integer alpha \(s) is ambiguous") }
        return Double(v)
    }
    guard (0...255).contains(v) else { fail("\(context): integer component \(s) out of 0...255") }
    return Double(v) / 255
}

/// Converts a colour-set "color" dictionary to gamma-encoded sRGB.
func parseColor(_ color: [String: Any], context: String) -> (SRGB, String) {
    if color["reference"] != nil || color["platform"] != nil {
        fail("\(context): system colour references are not supported")
    }
    guard let space = color["color-space"] as? String else { fail("\(context): missing color-space") }
    guard let comps = color["components"] as? [String: Any] else { fail("\(context): missing components") }
    func comp(_ key: String) -> Double {
        guard let raw = comps[key] else { fail("\(context): missing component \(key)") }
        return parseComponent(raw, isAlpha: key == "alpha", context: context)
    }
    let alpha = comps["alpha"] == nil ? 1.0 : comp("alpha")
    var out: SRGB
    switch space {
    case "srgb", "extended-srgb":
        out = SRGB(r: comp("red"), g: comp("green"), b: comp("blue"), a: alpha)
    case "extended-linear-srgb":
        out = SRGB(r: srgbEncode(comp("red")), g: srgbEncode(comp("green")), b: srgbEncode(comp("blue")), a: alpha)
    case "display-p3", "extended-display-p3":
        // Display P3 shares the sRGB transfer curve; convert in linear light with the D65 P3 -> sRGB matrix.
        let r = srgbDecode(comp("red")), g = srgbDecode(comp("green")), b = srgbDecode(comp("blue"))
        let lr = 1.2249401 * r - 0.2249404 * g
        let lg = -0.0420569 * r + 1.0420571 * g
        let lb = -0.0196376 * r - 0.0786361 * g + 1.0982735 * b
        out = SRGB(r: srgbEncode(lr), g: srgbEncode(lg), b: srgbEncode(lb), a: alpha)
    case "gray-gamma-22", "extended-gray":
        let white = comp("white")
        let w = srgbEncode(white < 0 ? -pow(-white, 2.2) : pow(white, 2.2))
        out = SRGB(r: w, g: w, b: w, a: alpha)
    default:
        fail("\(context): unsupported color-space \(space)")
    }
    let clamped = SRGB(r: min(max(out.r, 0), 1), g: min(max(out.g, 0), 1), b: min(max(out.b, 0), 1),
                       a: min(max(out.a, 0), 1))
    if clamped != out { note("warning: \(context): colour clamped into sRGB gamut") }
    return (clamped, space)
}

// MARK: - Asset catalog traversal

func readJSON(_ url: URL) -> [String: Any] {
    guard let data = try? Data(contentsOf: url) else { fail("cannot read \(url.path)") }
    guard let object = try? JSONSerialization.jsonObject(with: data), let dict = object as? [String: Any] else {
        fail("invalid JSON in \(url.path)")
    }
    return dict
}

func sortedChildren(_ dir: URL) -> [URL] {
    guard let names = try? fileManager.contentsOfDirectory(atPath: dir.path) else { fail("cannot list \(dir.path)") }
    return names.filter { !$0.hasPrefix(".") }.sorted().map { dir.appendingPathComponent($0) }
}

struct AssetRef {
    let name: String          // asset name as looked up at runtime (namespace-prefixed when applicable)
    let relativePath: String  // path of the .colorset / .imageset inside the catalog
    let url: URL
}

/// Walks a catalog depth-first in sorted order and returns every asset with the given extension.
func collectAssets(catalog: URL, ext: String) -> [AssetRef] {
    var out: [AssetRef] = []
    func walk(_ dir: URL, relative: String, namespace: String) {
        for child in sortedChildren(dir) {
            var isDirectory: ObjCBool = false
            guard fileManager.fileExists(atPath: child.path, isDirectory: &isDirectory), isDirectory.boolValue else {
                continue
            }
            let name = child.lastPathComponent
            let rel = relative.isEmpty ? name : relative + "/" + name
            if name.hasSuffix("." + ext) {
                let base = String(name.dropLast(ext.count + 1))
                out.append(AssetRef(name: namespace + base, relativePath: rel, url: child))
            } else if !(child.pathExtension.hasSuffix("set") && child.pathExtension.count > 3) {
                // Plain group folder (asset directories of other types, e.g. ".imageset", are skipped).
                walk(child, relative: rel, namespace: childNamespace(child, parent: namespace))
            }
        }
    }
    func childNamespace(_ dir: URL, parent: String) -> String {
        let contents = dir.appendingPathComponent("Contents.json")
        guard fileManager.fileExists(atPath: contents.path) else { return parent }
        let props = readJSON(contents)["properties"] as? [String: Any]
        if (props?["provides-namespace"] as? Bool) == true {
            return parent + dir.lastPathComponent + "/"
        }
        return parent
    }
    walk(catalog, relative: "", namespace: "")
    return out
}

// MARK: - Colour sets

struct ColorToken {
    let token: String
    let asset: String
    let source: String       // "<catalog-key>:<relative path>"
    let light: SRGB
    let dark: SRGB
    let hasDarkAppearance: Bool
    let colorSpace: String
    let group: String        // folder path inside the catalog, used for MARK comments
}

struct Exclusion {
    let asset: String
    let source: String
    let reason: String
}

struct Shadowed {
    let asset: String
    let source: String
    let winner: String
    let sameValue: Bool
}

let swiftKeywords: Set<String> = [
    "associatedtype", "class", "deinit", "enum", "extension", "fileprivate", "func", "import", "init", "inout",
    "internal", "let", "open", "operator", "private", "protocol", "public", "rethrows", "static", "struct",
    "subscript", "typealias", "var", "break", "case", "continue", "default", "defer", "do", "else", "fallthrough",
    "for", "guard", "if", "in", "repeat", "return", "switch", "where", "while", "as", "catch", "false", "is", "nil",
    "super", "self", "Self", "throw", "throws", "true", "try", "Type", "Any", "some",
]

/// lowerCamelCase identifier from an asset name: words split on non-alphanumerics, a leading run of capitals
/// lowered ("UIColor" -> "uiColor"), later words capitalised.
func camelCase(_ name: String) -> String {
    let words = name.split(whereSeparator: { !($0.isLetter || $0.isNumber) }).map(String.init)
    guard !words.isEmpty else { fail("asset name \(name) has no identifier characters") }
    var result = ""
    for (i, word) in words.enumerated() {
        if i == 0 {
            let chars = Array(word)
            var upperRun = 0
            while upperRun < chars.count, chars[upperRun].isUppercase { upperRun += 1 }
            if upperRun > 1 && upperRun < chars.count && chars[upperRun].isLowercase { upperRun -= 1 }
            let head = String(chars[0..<max(upperRun, 1)]).lowercased()
            result += head + String(chars[max(upperRun, 1)...])
        } else {
            result += word.prefix(1).uppercased() + word.dropFirst()
        }
    }
    if let first = result.first, first.isNumber { result = "_" + result }
    return result
}

func swiftIdentifier(_ token: String) -> String {
    swiftKeywords.contains(token) ? "`\(token)`" : token
}

let purpleNamePattern = try! NSRegularExpression(
    pattern: "purple|violet|indigo|lavender|magenta|lilac|plum|mauve", options: [.caseInsensitive])

func parseColorSet(_ ref: AssetRef, catalogKey: String) -> (light: SRGB, dark: SRGB, hasDark: Bool, space: String) {
    let context = "\(catalogKey):\(ref.relativePath)"
    let json = readJSON(ref.url.appendingPathComponent("Contents.json"))
    guard let entries = json["colors"] as? [[String: Any]] else { fail("\(context): no colors array") }
    var anyEntry: [String: Any]?, lightEntry: [String: Any]?, darkEntry: [String: Any]?
    // Prefer entries without a display-gamut qualifier; fall back to P3-gamut entries only when nothing else.
    let ungamuted = entries.filter { ($0["display-gamut"] as? String).map { $0.lowercased() == "srgb" } ?? true }
    for entry in (ungamuted.isEmpty ? entries : ungamuted) {
        if let idiom = entry["idiom"] as? String, idiom != "universal" { continue }
        guard let color = entry["color"] as? [String: Any] else { continue }  // unset slot in Xcode
        let appearances = entry["appearances"] as? [[String: Any]] ?? []
        if appearances.contains(where: { ($0["appearance"] as? String) == "contrast" }) { continue }
        let luminosity = appearances.first(where: { ($0["appearance"] as? String) == "luminosity" })?["value"] as? String
        switch luminosity {
        case nil: anyEntry = color
        case "light": lightEntry = color
        case "dark": darkEntry = color
        default: fail("\(context): unknown luminosity \(luminosity ?? "")")
        }
    }
    guard let lightSource = lightEntry ?? anyEntry else { fail("\(context): no universal light/any colour") }
    let (light, space) = parseColor(lightSource, context: context)
    if let darkSource = darkEntry {
        let (dark, darkSpace) = parseColor(darkSource, context: context + " (dark)")
        return (light, dark, true, darkSpace == space ? space : space + "+" + darkSpace)
    }
    return (light, light, false, space)
}

func loadColors(catalog: URL, catalogKey: String) -> (tokens: [ColorToken], excluded: [Exclusion], shadowed: [Shadowed]) {
    var tokens: [ColorToken] = []
    var excluded: [Exclusion] = []
    var shadowed: [Shadowed] = []
    var firstByAsset: [String: (source: String, light: SRGB, dark: SRGB)] = [:]
    var tokenNames: [String: String] = [:]
    for ref in collectAssets(catalog: catalog, ext: "colorset") {
        let source = "\(catalogKey):\(ref.relativePath)"
        let parsed = parseColorSet(ref, catalogKey: catalogKey)
        if let winner = firstByAsset[ref.name] {
            shadowed.append(Shadowed(asset: ref.name, source: source, winner: winner.source,
                                     sameValue: winner.light == parsed.light && winner.dark == parsed.dark))
            continue
        }
        firstByAsset[ref.name] = (source, parsed.light, parsed.dark)
        let range = NSRange(ref.name.startIndex..., in: ref.name)
        if purpleNamePattern.firstMatch(in: ref.name, range: range) != nil {
            excluded.append(Exclusion(asset: ref.name, source: source, reason: "name denotes purple"))
            continue
        }
        if parsed.light.isPurpleish || parsed.dark.isPurpleish {
            excluded.append(Exclusion(asset: ref.name, source: source,
                                      reason: "purple hue (light \(parsed.light.hex), dark \(parsed.dark.hex))"))
            continue
        }
        let token = camelCase(ref.name)
        if let clash = tokenNames[token] {
            fail("\(source): token \(token) collides with \(clash)")
        }
        tokenNames[token] = source
        let group = (ref.relativePath as NSString).deletingLastPathComponent
        tokens.append(ColorToken(token: token, asset: ref.name, source: source, light: parsed.light,
                                 dark: parsed.dark, hasDarkAppearance: parsed.hasDark, colorSpace: parsed.space,
                                 group: group))
    }
    for e in excluded { note("excluded \(e.source): \(e.reason)") }
    for s in shadowed where !s.sameValue {
        note("duplicate asset \(s.asset): \(s.source) is shadowed by \(s.winner) (values differ)")
    }
    return (tokens, excluded, shadowed)
}

let uiKitColors = loadColors(catalog: uiKitMedia, catalogKey: "dashuikit")
let iosColors = loadColors(catalog: iosShared, catalogKey: "ios-shared")
if uiKitColors.tokens.isEmpty || iosColors.tokens.isEmpty { fail("no colour sets found") }

// MARK: - Typography (parsed from DashUIKit DashTextStyle.swift)

struct TypeStyle {
    let name: String
    let size: Double
    let weight: String
    let lineHeight: Double
}

func loadTypography() -> [TypeStyle] {
    guard let source = try? String(contentsOf: uiKitTextStyle, encoding: .utf8) else {
        fail("cannot read \(uiKitTextStyle.path)")
    }
    let regex = try! NSRegularExpression(pattern:
        #"static let (\w+) = DashTextStyle\(size: ([0-9.]+), weight: \.(\w+), lineHeight: ([0-9.]+)\)"#)
    let matches = regex.matches(in: source, range: NSRange(source.startIndex..., in: source))
    let styles = matches.map { m -> TypeStyle in
        func group(_ i: Int) -> String { String(source[Range(m.range(at: i), in: source)!]) }
        return TypeStyle(name: group(1), size: Double(group(2))!, weight: group(3), lineHeight: Double(group(4))!)
    }
    if styles.isEmpty { fail("no DashTextStyle tokens found in \(uiKitTextStyle.path)") }
    return styles
}

let typography = loadTypography()

// MARK: - Icons

struct IconEntry {
    let id: String
    let file: String
    let group: String
    let source: String
    let path: String
}

struct ExportedIcon {
    let entry: IconEntry
    let format: String        // "svg" | "pdf" | "png"
    let scales: [Int]         // raster scales exported; empty for vectors
    let hasDark: Bool
    let isTemplate: Bool
    let files: [String]
    let purpleCheck: String   // "passed" | "skipped (...)"
}

func loadManifest() -> [IconEntry] {
    let json = readJSON(iconManifestURL)
    guard let icons = json["icons"] as? [[String: Any]] else { fail("icon manifest has no icons array") }
    var ids = Set<String>(), files = Set<String>()
    let entries = icons.map { dict -> IconEntry in
        guard let id = dict["id"] as? String, let file = dict["file"] as? String, let group = dict["group"] as? String,
              let source = dict["source"] as? String, let path = dict["path"] as? String else {
            fail("icon manifest entry is missing a field: \(dict)")
        }
        guard ids.insert(id).inserted else { fail("icon manifest: duplicate id \(id)") }
        guard files.insert(file).inserted else { fail("icon manifest: duplicate file \(file)") }
        guard file.range(of: "^[a-z0-9]+(-[a-z0-9]+)*$", options: .regularExpression) != nil else {
            fail("icon manifest: file name \(file) must be kebab-case")
        }
        return IconEntry(id: id, file: file, group: group, source: source, path: path)
    }
    if entries.count > 140 { fail("icon manifest has \(entries.count) icons; the limit is 140") }
    return entries
}

/// Fraction-based purple check: the icon fails when more than 2 % of its visible pixels are purple-ish.
func purpleCheckRaster(_ url: URL) -> String {
    #if canImport(ImageIO)
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { fail("cannot decode \(url.path)") }
    var image: CGImage?
    if url.pathExtension.lowercased() == "pdf" {
        guard let doc = CGPDFDocument(url as CFURL), let page = doc.page(at: 1) else { fail("cannot open \(url.path)") }
        let box = page.getBoxRect(.mediaBox)
        let scale = 4.0
        let w = Int(box.width * scale), h = Int(box.height * scale)
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { fail("context") }
        ctx.scaleBy(x: scale, y: scale)
        ctx.drawPDFPage(page)
        image = ctx.makeImage()
    } else {
        image = CGImageSourceCreateImageAtIndex(src, 0, nil)
    }
    guard let cg = image else { fail("cannot decode \(url.path)") }
    let w = cg.width, h = cg.height
    var pixels = [UInt8](repeating: 0, count: w * h * 4)
    let ok = pixels.withUnsafeMutableBytes { buf -> Bool in
        guard let ctx = CGContext(data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
        return true
    }
    if !ok { fail("cannot rasterise \(url.path)") }
    var visible = 0, purple = 0
    for i in stride(from: 0, to: pixels.count, by: 4) {
        let a = Double(pixels[i + 3]) / 255
        guard a > 0.5 else { continue }
        visible += 1
        let c = SRGB(r: Double(pixels[i]) / 255 / a, g: Double(pixels[i + 1]) / 255 / a,
                     b: Double(pixels[i + 2]) / 255 / a, a: 1)
        if c.isPurpleish && c.hsv.v > 0.15 { purple += 1 }
    }
    if visible > 0 && Double(purple) / Double(visible) > 0.02 {
        return "failed (\(purple) of \(visible) visible pixels purple-ish)"
    }
    return "passed"
    #else
    return "skipped (no ImageIO on this platform)"
    #endif
}

/// CSS colour keywords whose hue is purple-ish (HSV hue 260...320, saturation > 0.25).
let purpleColorNames = [
    "blueviolet", "darkmagenta", "darkorchid", "darkviolet", "fuchsia", "magenta", "mediumorchid",
    "mediumpurple", "orchid", "plum", "purple", "rebeccapurple", "violet",
]

/// Finds purple-ish colours written in an SVG: hex (3, 4, 6 or 8 digits), rgb()/rgba(), hsl()/hsla() and
/// colour keywords. Gradients and CSS variables are covered only through the literal colours they contain.
func purpleCheckSVG(_ url: URL) -> String {
    guard let text = try? String(contentsOf: url, encoding: .utf8) else { fail("cannot read \(url.path)") }
    let range = NSRange(text.startIndex..., in: text)
    func capture(_ m: NSTextCheckingResult, _ i: Int) -> String { String(text[Range(m.range(at: i), in: text)!]) }

    let hexRegex = try! NSRegularExpression(pattern: "#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\\b")
    for m in hexRegex.matches(in: text, range: range) {
        var hex = capture(m, 1)
        if hex.count <= 4 { hex = hex.map { "\($0)\($0)" }.joined() }
        let v = Int(hex.prefix(6), radix: 16)!
        let c = SRGB(r: Double((v >> 16) & 0xFF) / 255, g: Double((v >> 8) & 0xFF) / 255, b: Double(v & 0xFF) / 255, a: 1)
        if c.isPurpleish { return "failed (colour #\(hex))" }
    }

    func channel(_ s: String) -> Double {
        let t = s.trimmingCharacters(in: .whitespaces)
        return t.hasSuffix("%") ? (Double(t.dropLast()) ?? 0) / 100 : (Double(t) ?? 0) / 255
    }
    let rgbRegex = try! NSRegularExpression(
        pattern: "rgba?\\(\\s*([0-9.]+%?)[\\s,]+([0-9.]+%?)[\\s,]+([0-9.]+%?)", options: [.caseInsensitive])
    for m in rgbRegex.matches(in: text, range: range) {
        let c = SRGB(r: channel(capture(m, 1)), g: channel(capture(m, 2)), b: channel(capture(m, 3)), a: 1)
        if c.isPurpleish { return "failed (colour \(capture(m, 0)))" }
    }

    let hslRegex = try! NSRegularExpression(
        pattern: "hsla?\\(\\s*([0-9.]+)(?:deg)?[\\s,]+([0-9.]+)%[\\s,]+([0-9.]+)%", options: [.caseInsensitive])
    for m in hslRegex.matches(in: text, range: range) {
        let h = (Double(capture(m, 1)) ?? 0).truncatingRemainder(dividingBy: 360)
        let sl = (Double(capture(m, 2)) ?? 0) / 100, l = (Double(capture(m, 3)) ?? 0) / 100
        // HSL -> HSV saturation for the same hue.
        let v = l + sl * min(l, 1 - l)
        let sv = v == 0 ? 0 : 2 * (1 - l / v)
        if sv > 0.25 && h >= 260 && h <= 320 { return "failed (colour \(capture(m, 0)))" }
    }

    let nameRegex = try! NSRegularExpression(
        pattern: "(?:fill|stroke|stop-color|color)\\s*[=:]\\s*[\"']?\\s*([a-zA-Z]+)", options: [.caseInsensitive])
    for m in nameRegex.matches(in: text, range: range) where purpleColorNames.contains(capture(m, 1).lowercased()) {
        return "failed (colour \(capture(m, 1)))"
    }
    return "passed"
}

func exportIcons(_ entries: [IconEntry]) -> [ExportedIcon] {
    let catalogs = ["ios": iosAppAssets, "dashuikit": uiKitMedia]
    try? fileManager.createDirectory(at: iconsDir, withIntermediateDirectories: true)
    var exported: [ExportedIcon] = []
    for entry in entries {
        guard let catalog = catalogs[entry.source] else { fail("icon \(entry.id): unknown source \(entry.source)") }
        let setURL = catalog.appendingPathComponent(entry.path)
        let context = "icon \(entry.id) (\(entry.source):\(entry.path))"
        guard entry.path.hasSuffix(".imageset") else { fail("\(context): path must be an .imageset") }
        let json = readJSON(setURL.appendingPathComponent("Contents.json"))
        guard let images = json["images"] as? [[String: Any]] else { fail("\(context): no images array") }
        let props = json["properties"] as? [String: Any]
        let isTemplate = (props?["template-rendering-intent"] as? String) == "template"

        struct Slot { let file: String; let scale: String?; let dark: Bool }
        var slots: [Slot] = []
        for image in images {
            guard let file = image["filename"] as? String else { continue }
            if let idiom = image["idiom"] as? String, idiom != "universal" { continue }
            let appearances = image["appearances"] as? [[String: Any]] ?? []
            let luminosity = appearances.first(where: { ($0["appearance"] as? String) == "luminosity" })?["value"] as? String
            slots.append(Slot(file: file, scale: image["scale"] as? String, dark: luminosity == "dark"))
        }
        func isVector(_ slot: Slot) -> Bool { ["pdf", "svg"].contains((slot.file as NSString).pathExtension.lowercased()) }
        if slots.contains(where: isVector), !slots.contains(where: { isVector($0) && !$0.dark }) {
            fail("\(context): vector imageset has only a dark-appearance file")
        }
        let vector = slots.first { isVector($0) && !$0.dark }
        var copies: [(from: String, to: String)] = []
        var hasDark = false
        var format: String
        var scales: [Int] = []
        if let vector {
            format = (vector.file as NSString).pathExtension.lowercased()
            copies.append((vector.file, "\(entry.file).\(format)"))
            if let dark = slots.first(where: { $0.dark && ($0.file as NSString).pathExtension.lowercased() == format }),
               dark.file != vector.file {
                copies.append((dark.file, "\(entry.file)-dark.\(format)"))
                hasDark = true
            }
        } else {
            format = "png"
            var wanted: [String] = ["2x", "3x"].filter { s in slots.contains { $0.scale == s && !$0.dark } }
            if wanted.isEmpty { wanted = slots.contains { $0.scale == "1x" && !$0.dark } ? ["1x"] : [] }
            if wanted.isEmpty { fail("\(context): no PNG at 1x/2x/3x") }
            for s in wanted {
                guard let light = slots.first(where: { $0.scale == s && !$0.dark }) else { continue }
                guard (light.file as NSString).pathExtension.lowercased() == "png" else {
                    fail("\(context): unsupported raster \(light.file)")
                }
                let suffix = s == "1x" ? "" : "@\(s)"
                copies.append((light.file, "\(entry.file)\(suffix).png"))
                if let dark = slots.first(where: { $0.scale == s && $0.dark }) {
                    copies.append((dark.file, "\(entry.file)-dark\(suffix).png"))
                    hasDark = true
                }
                scales.append(Int(s.dropLast())!)
            }
            let darkScales = Set(wanted.filter { s in slots.contains { $0.scale == s && $0.dark } })
            if !darkScales.isEmpty && darkScales != Set(wanted) {
                fail("\(context): dark variant exists at \(darkScales.sorted()) but not at every exported scale \(wanted)")
            }
        }
        var checks: [String] = []
        for (from, to) in copies {
            let src = setURL.appendingPathComponent(from)
            let dst = iconsDir.appendingPathComponent(to)
            guard fileManager.fileExists(atPath: src.path) else { fail("\(context): missing file \(from)") }
            try? fileManager.removeItem(at: dst)
            do { try fileManager.copyItem(at: src, to: dst) } catch { fail("\(context): copy failed: \(error)") }
            checks.append(format == "svg" ? purpleCheckSVG(dst) : purpleCheckRaster(dst))
        }
        if let bad = checks.first(where: { $0.hasPrefix("failed") }) {
            fail("\(context): purple check \(bad)")
        }
        exported.append(ExportedIcon(entry: entry, format: format, scales: scales,
                                     hasDark: hasDark, isTemplate: isTemplate,
                                     files: copies.map(\.to), purpleCheck: checks.first ?? "passed"))
    }
    // Remove files from earlier runs that are no longer produced.
    let keep = Set(exported.flatMap(\.files) + ["NOTICE"])
    for name in (try? fileManager.contentsOfDirectory(atPath: iconsDir.path)) ?? [] where !keep.contains(name) {
        try? fileManager.removeItem(at: iconsDir.appendingPathComponent(name))
        note("removed stale icon file \(name)")
    }
    return exported
}

let icons = exportIcons(loadManifest())

// MARK: - Source provenance

func gitHead(_ repo: URL) -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = ["git", "-C", repo.path, "rev-parse", "HEAD"]
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = FileHandle.nullDevice
    do { try process.run() } catch { return "unknown" }
    process.waitUntilExit()
    let out = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    let sha = out.trimmingCharacters(in: .whitespacesAndNewlines)
    return process.terminationStatus == 0 && !sha.isEmpty ? sha : "unknown"
}

let iosCommit = gitHead(iosRepo)
let uiKitCommit = gitHead(uiKitRepo)

// MARK: - Colors.swift

func rgbaLiteral(_ c: SRGB) -> String {
    "RGBA(red: \(swiftDouble(c.r)), green: \(swiftDouble(c.g)), blue: \(swiftDouble(c.b)), alpha: \(swiftDouble(c.a)))"
}

func describe(_ c: SRGB) -> String {
    c.a == 1 ? c.hex : "\(c.hex) @ \(Int((c.a * 100).rounded()))%"
}

func colorDeclarations(_ tokens: [ColorToken], indent: String) -> String {
    var out = ""
    var lastGroup: String?
    for t in tokens {
        if t.group != lastGroup {
            out += "\n\(indent)// MARK: \(t.group.isEmpty ? "(root)" : t.group)\n\n"
            lastGroup = t.group
        }
        let values = t.hasDarkAppearance ? "light \(describe(t.light)), dark \(describe(t.dark))" : describe(t.light)
        out += "\(indent)/// Asset `\(t.asset)`: \(values).\n"
        if t.hasDarkAppearance {
            out += "\(indent)public static let \(swiftIdentifier(t.token)) = DashColor(\n"
            out += "\(indent)    light: \(rgbaLiteral(t.light)),\n"
            out += "\(indent)    dark: \(rgbaLiteral(t.dark)))\n"
        } else {
            out += "\(indent)public static let \(swiftIdentifier(t.token)) = DashColor(\(rgbaLiteral(t.light)))\n"
        }
    }
    out += "\n\(indent)/// Every token in this namespace keyed by its Swift name.\n"
    out += "\(indent)public static let allTokens: [String: DashColor] = [\n"
    for t in tokens { out += "\(indent)    \(JSONValue.quote(t.token)): \(swiftIdentifier(t.token)),\n" }
    out += "\(indent)]\n"
    out += "\n\(indent)/// Asset-catalog name for every token in this namespace, keyed by its Swift name.\n"
    out += "\(indent)public static let assetNames: [String: String] = [\n"
    for t in tokens { out += "\(indent)    \(JSONValue.quote(t.token)): \(JSONValue.quote(t.asset)),\n" }
    out += "\(indent)]\n"
    return out
}

let header = """
// Generated by scripts/gen-tokens.swift. Do not edit by hand; rerun the script instead:
//   swift scripts/gen-tokens.swift <dashwallet-ios> <DashUIKit> <dashwallet-desktop>


"""

var colorsSwift = header + """
// Colour tokens as light/dark sRGB pairs.
//   DashColor.<token>      DashUIKit Sources/DashUIKit/Resources/Media.xcassets (the `Color.dash.*` design system)
//   DashColor.App.<token>  dashwallet-ios Shared/Resources/SharedAssets.xcassets (the app palette, `Color+DWStyle`)
// Excluded and shadowed colour sets are listed in Resources/Tokens/tokens.json.

import Foundation

extension DashColor {
"""
colorsSwift += colorDeclarations(uiKitColors.tokens, indent: "    ")
colorsSwift += """

    /// The dashwallet-ios app palette (`SharedAssets.xcassets`).
    public enum App {

"""
colorsSwift += colorDeclarations(iosColors.tokens, indent: "        ")
colorsSwift += "    }\n}\n"

// MARK: - Icons.swift

let groupOrder = icons.reduce(into: [String]()) { if !$0.contains($1.entry.group) { $0.append($1.entry.group) } }
var iconsSwift = header + """
// Icons exported to Resources/Icons by the generator. Provenance and licence: Resources/Icons/NOTICE.

import Foundation

/// Groups of the exported icon set.
public enum DashIconGroup: String, CaseIterable, Sendable {

"""
for g in groupOrder { iconsSwift += "    case \(swiftIdentifier(g)) = \(JSONValue.quote(g))\n" }
iconsSwift += """
}

/// One icon of the exported set. The raw value is the base file name in Resources/Icons.
public enum DashIconToken: String, CaseIterable, Sendable {

"""
for g in groupOrder {
    iconsSwift += "    // MARK: \(g)\n"
    for icon in icons where icon.entry.group == g {
        iconsSwift += "    case \(swiftIdentifier(icon.entry.id)) = \(JSONValue.quote(icon.entry.file))\n"
    }
    iconsSwift += "\n"
}
iconsSwift += """
    /// Format, scales, variants and provenance of this icon's exported files.
    public var metadata: DashIconMetadata {
        switch self {

"""
for icon in icons {
    let format: String
    switch icon.format {
    case "svg": format = ".svg"
    case "pdf": format = ".pdf"
    default: format = ".png(scales: [\(icon.scales.map(String.init).joined(separator: ", "))])"
    }
    iconsSwift += """
            case .\(icon.entry.id):
                DashIconMetadata(group: .\(icon.entry.group), format: \(format), hasDarkVariant: \(icon.hasDark), \
    isTemplate: \(icon.isTemplate), source: \(JSONValue.quote("\(icon.entry.source):\(icon.entry.path)")))

    """
}
iconsSwift += "        }\n    }\n}\n"

// MARK: - tokens.json

func colorJSON(_ c: SRGB) -> JSONValue {
    .object([("hex", .string(c.hex)), ("alpha", .number(c.a)),
             ("srgb", .array([.number(c.r), .number(c.g), .number(c.b), .number(c.a)]))])
}

func namespaceJSON(_ tokens: [ColorToken]) -> JSONValue {
    .array(tokens.map { t in
        .object([("token", .string(t.token)), ("asset", .string(t.asset)), ("source", .string(t.source)),
                 ("colorSpace", .string(t.colorSpace)), ("hasDarkAppearance", .bool(t.hasDarkAppearance)),
                 ("light", colorJSON(t.light)), ("dark", colorJSON(t.dark))])
    })
}

let tokensJSON: JSONValue = .object([
    ("generatedBy", .string("scripts/gen-tokens.swift")),
    ("sources", .object([
        ("dashuikit", .object([("repository", .string("dashpay/DashUIKit")), ("commit", .string(uiKitCommit)),
                               ("catalog", .string("Sources/DashUIKit/Resources/Media.xcassets"))])),
        ("ios-shared", .object([("repository", .string("dashpay/dashwallet-ios")), ("commit", .string(iosCommit)),
                                ("catalog", .string("Shared/Resources/SharedAssets.xcassets"))])),
        ("ios", .object([("repository", .string("dashpay/dashwallet-ios")), ("commit", .string(iosCommit)),
                         ("catalog", .string("DashWallet/Resources/AppAssets.xcassets"))])),
    ])),
    ("colors", .object([
        ("DashColor", namespaceJSON(uiKitColors.tokens)),
        ("DashColor.App", namespaceJSON(iosColors.tokens)),
    ])),
    ("excluded", .array((uiKitColors.excluded + iosColors.excluded).map {
        .object([("asset", .string($0.asset)), ("source", .string($0.source)), ("reason", .string($0.reason))])
    })),
    ("shadowed", .array((uiKitColors.shadowed + iosColors.shadowed).map {
        .object([("asset", .string($0.asset)), ("source", .string($0.source)), ("winner", .string($0.winner)),
                 ("sameValue", .bool($0.sameValue))])
    })),
    ("typography", .array(typography.map {
        .object([("name", .string($0.name)), ("size", .number($0.size)), ("weight", .string($0.weight)),
                 ("lineHeight", .number($0.lineHeight)), ("tracking", .number(0))])
    })),
    ("icons", .array(icons.map {
        .object([("id", .string($0.entry.id)), ("file", .string($0.entry.file)), ("group", .string($0.entry.group)),
                 ("source", .string("\($0.entry.source):\($0.entry.path)")), ("format", .string($0.format)),
                 ("scales", .array($0.scales.map { .integer($0) })), ("hasDarkVariant", .bool($0.hasDark)),
                 ("isTemplate", .bool($0.isTemplate)), ("files", .array($0.files.map { .string($0) })),
                 ("purpleCheck", .string($0.purpleCheck))])
    })),
])

// MARK: - NOTICE

func licenseText(_ url: URL) -> String {
    guard let text = try? String(contentsOf: url, encoding: .utf8) else { fail("cannot read \(url.path)") }
    return text.trimmingCharacters(in: .whitespacesAndNewlines)
}

let notice = """
Icons in this directory are copied by scripts/gen-tokens.swift from two repositories owned by
Dash Core Group (github.com/dashpay). Both are MIT-licensed.

dashwallet-ios (commit \(iosCommit))
  Source: DashWallet/Resources/AppAssets.xcassets
  Licence: the repository LICENSE file, reproduced below. It is the MIT licence text under the
  heading "breadwallet license"; the Dash source files carry "Copyright (c) Dash Core Group,
  Licensed under the MIT License".

DashUIKit (commit \(uiKitCommit))
  Source: Sources/DashUIKit/Resources/Media.xcassets
  Licence: MIT. The repository has no LICENSE file; its README states "MIT — see the per-file
  headers", and the headers read "Copyright (c) 2026 Dash Core Group. Licensed under the MIT
  License (https://opensource.org/licenses/MIT)".

Per-icon sources are listed in Resources/Tokens/tokens.json ("icons").

---- dashwallet-ios LICENSE ----

\(licenseText(iosRepo.appendingPathComponent("LICENSE")))

---- MIT License (DashUIKit) ----

Copyright (c) 2026 Dash Core Group

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

"""

// MARK: - Write outputs

func write(_ text: String, to url: URL) {
    do {
        try fileManager.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url, options: .atomic)
    } catch {
        fail("cannot write \(url.path): \(error)")
    }
}

write(colorsSwift, to: generatedDir.appendingPathComponent("Colors.swift"))
write(iconsSwift, to: generatedDir.appendingPathComponent("Icons.swift"))
write(tokensJSON.render() + "\n", to: tokensDir.appendingPathComponent("tokens.json"))
write(notice, to: iconsDir.appendingPathComponent("NOTICE"))

// The macOS app's accent colour is Dash blue, taken from dashwallet-ios `DashBlueColor`, so sidebar
// selection, focus rings, default buttons and toggles never follow the user's system accent.
// Written only when the output repository has the macOS app's asset catalog.
let accentCatalog = outRoot.appendingPathComponent("Apps/macOS/DashWallet/Assets.xcassets")
if fileManager.fileExists(atPath: accentCatalog.path) {
    guard let blue = iosColors.tokens.first(where: { $0.asset == "DashBlueColor" }) else {
        fail("ios-shared: DashBlueColor not found (needed for the macOS AccentColor)")
    }
    func component(_ v: Double) -> String { String(format: "%.3f", v) }
    let accent = """
    {
      "colors" : [
        {
          "color" : {
            "color-space" : "srgb",
            "components" : {
              "alpha" : "\(component(blue.light.a))",
              "blue" : "\(component(blue.light.b))",
              "green" : "\(component(blue.light.g))",
              "red" : "\(component(blue.light.r))"
            }
          },
          "idiom" : "universal"
        }
      ],
      "info" : {
        "author" : "xcode",
        "version" : 1
      }
    }

    """
    write(accent, to: accentCatalog.appendingPathComponent("AccentColor.colorset/Contents.json"))
}

note("""
wrote \(uiKitColors.tokens.count) DashColor + \(iosColors.tokens.count) DashColor.App tokens, \
\(typography.count) text styles, \(icons.count) icons \
(\(uiKitColors.excluded.count + iosColors.excluded.count) colour sets excluded, \
\(uiKitColors.shadowed.count + iosColors.shadowed.count) shadowed)
""")
