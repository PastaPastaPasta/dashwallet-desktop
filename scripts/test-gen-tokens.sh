#!/bin/sh
# Runs scripts/gen-tokens.swift against a small synthetic fixture (colour-space conversion, luminosity rules,
# namespaced folders, duplicate names, purple exclusion, icon export) and checks the output. The fixture
# borrows two PNGs from the real DashUIKit checkout, so the DashUIKit path is required.
# Usage: scripts/test-gen-tokens.sh <DashUIKit-repo>
set -eu

[ $# -eq 1 ] || { echo "usage: $0 <DashUIKit-repo>" >&2; exit 2; }
repo=$(cd "$(dirname "$0")/.." && pwd)
uikit_src=$1
png_dir="$uikit_src/Sources/DashUIKit/Resources/Media.xcassets/Icons & Illustrations/Menu/menu-send.imageset"
[ -f "$png_dir/menu-send@2x.png" ] || { echo "missing $png_dir/menu-send@2x.png" >&2; exit 2; }

fx=$(mktemp -d "${TMPDIR:-/tmp}/gen-tokens-fixture.XXXXXX")
trap 'rm -rf "$fx"' EXIT INT TERM
failures=0
check() {  # check <description> <file> <fixed string expected in file>
    if grep -qF -- "$3" "$2"; then echo "ok   $1"; else echo "FAIL $1: '$3' not in $(basename "$2")"; failures=$((failures + 1)); fi
}

colorset() {  # colorset <dir> <colors JSON array body>
    mkdir -p "$1"
    printf '{ "colors" : [ %s ], "info" : { "author" : "xcode", "version" : 1 } }\n' "$2" > "$1/Contents.json"
}
srgb() {  # srgb <r> <g> <b> <a>
    printf '"color" : { "color-space" : "srgb", "components" : { "red" : "%s", "green" : "%s", "blue" : "%s", "alpha" : "%s" } }' "$1" "$2" "$3" "$4"
}
dark='"appearances" : [ { "appearance" : "luminosity", "value" : "dark" } ]'
light='"appearances" : [ { "appearance" : "luminosity", "value" : "light" } ]'

# DashUIKit side
ui="$fx/DashUIKit"
media="$ui/Sources/DashUIKit/Resources/Media.xcassets"
colorset "$media/Colors/Blue.colorset" "{ $(srgb 0x00 0x8D 0xE4 1.000), \"idiom\" : \"universal\" }"
colorset "$media/Colors/P3.colorset" '{ "color" : { "color-space" : "display-p3", "components" : { "red" : "0.800", "green" : "0.400", "blue" : "0.200", "alpha" : "1.000" } }, "idiom" : "universal" }'
colorset "$media/Colors/P3Red.colorset" '{ "color" : { "color-space" : "display-p3", "components" : { "red" : "1.000", "green" : "0.000", "blue" : "0.000", "alpha" : "1.000" } }, "idiom" : "universal" }'
colorset "$media/Colors/GrayGamma.colorset" '{ "color" : { "color-space" : "gray-gamma-22", "components" : { "white" : "0.600", "alpha" : "0.500" } }, "idiom" : "universal" }'
colorset "$media/Colors/Lum.colorset" "{ $(srgb 0x11 0x11 0x11 1.000), \"idiom\" : \"universal\" }, { $light, $(srgb 0x22 0x22 0x22 1.000), \"idiom\" : \"universal\" }, { $dark, $(srgb 0x33 0x33 0x33 1.000), \"idiom\" : \"universal\" }"
colorset "$media/Colors/Grape.colorset" "{ $(srgb 0x80 0x00 0xFF 1.000), \"idiom\" : \"universal\" }"
colorset "$media/Colors/Violet.colorset" "{ $(srgb 0x00 0x8D 0xE4 1.000), \"idiom\" : \"universal\" }"
mkdir -p "$media/Brand"
echo '{ "info" : { "author" : "xcode", "version" : 1 }, "properties" : { "provides-namespace" : true } }' > "$media/Brand/Contents.json"
colorset "$media/Brand/Accent.colorset" "{ $(srgb 0.000 1.000 0.000 1.000), \"idiom\" : \"universal\" }"
mkdir -p "$media/Icons/menu-send.imageset"
cp "$png_dir/Contents.json" "$png_dir"/*.png "$media/Icons/menu-send.imageset/"
mkdir -p "$ui/Sources/DashUIKit/Foundation"
echo '    static let body = DashTextStyle(size: 17, weight: .regular, lineHeight: 22)' > "$ui/Sources/DashUIKit/Foundation/DashTextStyle.swift"

# dashwallet-ios side
ios="$fx/ios"
shared="$ios/Shared/Resources/SharedAssets.xcassets"
colorset "$shared/A/Dup.colorset" "{ $(srgb 0x11 0x11 0x11 1.000), \"idiom\" : \"universal\" }"
colorset "$shared/B/Dup.colorset" "{ $(srgb 0x22 0x22 0x22 1.000), \"idiom\" : \"universal\" }"
colorset "$shared/IntComponents.colorset" "{ $(srgb 112 112 114 0.070), \"idiom\" : \"universal\" }, { $dark, \"idiom\" : \"universal\" }"
app="$ios/DashWallet/Resources/AppAssets.xcassets"
mkdir -p "$app/Vec/logo.imageset"
echo '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#008DE4"/></svg>' > "$app/Vec/logo.imageset/logo.svg"
echo '{ "images" : [ { "filename" : "logo.svg", "idiom" : "universal" } ], "info" : { "author" : "xcode", "version" : 1 }, "properties" : { "template-rendering-intent" : "template" } }' > "$app/Vec/logo.imageset/Contents.json"
echo "MIT fixture licence" > "$ios/LICENSE"

# Output repo
out="$fx/out"
mkdir -p "$out/scripts" "$out/Resources/Icons"
echo stale > "$out/Resources/Icons/stale.png"
cat > "$out/scripts/icon-manifest.json" <<'EOF'
{ "icons": [
  { "id": "send", "file": "action-send", "group": "action", "source": "dashuikit", "path": "Icons/menu-send.imageset" },
  { "id": "logo", "file": "brand-logo", "group": "brand", "source": "ios", "path": "Vec/logo.imageset" }
] }
EOF

swift "$repo/scripts/gen-tokens.swift" "$ios" "$ui" "$out" 2> "$fx/log.txt" || { cat "$fx/log.txt"; exit 1; }
colors="$out/Sources/DesignTokens/Generated/Colors.swift"
icons="$out/Sources/DesignTokens/Generated/Icons.swift"
json="$out/Resources/Tokens/tokens.json"

check "hex sRGB" "$colors" 'Asset `Blue`: #008DE4.'
check "display-p3 converted to sRGB" "$colors" 'Asset `P3`: #DB5E1F.'
check "gray-gamma-22 converted to sRGB" "$colors" 'Asset `GrayGamma`: #9A9A9A @ 50%.'
check "luminosity light beats any" "$colors" 'Asset `Lum`: light #222222, dark #333333.'
check "namespaced folder" "$colors" 'public static let brandAccent = DashColor('
check "float components" "$colors" 'Asset `Brand/Accent`: #00FF00.'
check "first duplicate wins" "$colors" 'Asset `Dup`: #111111.'
check "8-bit integer components, empty dark slot" "$colors" 'public static let intComponents = DashColor(RGBA(red: 0.4392156862745098, green: 0.4392156862745098, blue: 0.4470588235294118, alpha: 0.07))'
check "purple hue excluded" "$json" '"reason": "purple hue (light #8000FF, dark #8000FF)"'
check "purple name excluded" "$json" '"reason": "name denotes purple"'
check "shadowed duplicate recorded" "$json" '"source": "ios-shared:B/Dup.colorset"'
check "typography parsed" "$json" '"name": "body"'
check "png icon metadata" "$icons" 'DashIconMetadata(group: .action, format: .png(scales: [2, 3]), hasDarkVariant: true, isTemplate: false'
check "svg icon metadata" "$icons" 'DashIconMetadata(group: .brand, format: .svg, hasDarkVariant: false, isTemplate: true'
for f in action-send@2x.png action-send@3x.png action-send-dark@2x.png action-send-dark@3x.png brand-logo.svg NOTICE; do
    if [ -f "$out/Resources/Icons/$f" ]; then echo "ok   exported $f"; else echo "FAIL missing $f"; failures=$((failures + 1)); fi
done
if [ -e "$out/Resources/Icons/stale.png" ]; then echo "FAIL stale icon kept"; failures=$((failures + 1)); else echo "ok   stale icon removed"; fi
if grep -q "Grape\|Violet" "$colors"; then echo "FAIL purple token emitted"; failures=$((failures + 1)); else echo "ok   no purple token emitted"; fi


# Out-of-gamut colours are clamped with a warning (P3 pure red lies outside sRGB).
check "out-of-gamut clamp reported" "$fx/log.txt" 'dashuikit:Colors/P3Red.colorset: colour clamped into sRGB gamut'
check "out-of-gamut clamped value" "$colors" 'Asset `P3Red`: #FF0000.'

# Negative cases: each edit must make the generator exit non-zero with the given message, then is undone.
run=0
expect_failure() {  # expect_failure <description> <expected stderr text>
    run=$((run + 1))
    if swift "$repo/scripts/gen-tokens.swift" "$ios" "$ui" "$out" 2> "$fx/neg$run.txt"; then
        echo "FAIL $1: generator succeeded"; failures=$((failures + 1))
    else
        check "$1" "$fx/neg$run.txt" "$2"
    fi
}
svg="$app/Vec/logo.imageset/logo.svg"
cp "$svg" "$fx/logo.svg.orig"
for colour in '#9B30FF' '#9B30FFCC' 'rgb(155, 48, 255)' 'hsl(270, 100%, 59%)' 'purple'; do
    sed -e "s/#008DE4/$colour/" "$fx/logo.svg.orig" > "$svg"
    expect_failure "purple SVG colour $colour rejected" "purple check failed (colour"
done
cp "$fx/logo.svg.orig" "$svg"

# A mostly purple PNG (raster check needs ImageIO, so macOS only).
send="$media/Icons/menu-send.imageset"
if [ "$(uname)" = Darwin ]; then
    cat > "$fx/purple-png.swift" <<'EOF'
import CoreGraphics
import Foundation
import ImageIO
let ctx = CGContext(data: nil, width: 16, height: 16, bitsPerComponent: 8, bytesPerRow: 0,
                    space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
ctx.setFillColor(CGColor(srgbRed: 0.6, green: 0.2, blue: 1.0, alpha: 1))
ctx.fill(CGRect(x: 0, y: 0, width: 16, height: 16))
let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL, "public.png" as CFString, 1, nil)!
CGImageDestinationAddImage(dest, ctx.makeImage()!, nil)
precondition(CGImageDestinationFinalize(dest))
EOF
    cp "$send/menu-send@2x.png" "$fx/send2x.png"
    swift "$fx/purple-png.swift" "$send/menu-send@2x.png"
    expect_failure "purple PNG rejected" 'visible pixels purple-ish'
    cp "$fx/send2x.png" "$send/menu-send@2x.png"
fi

# Dark PNG at some scales only.
cp "$send/Contents.json" "$fx/send-contents.orig"
cat > "$send/Contents.json" <<'EOF'
{ "images" : [
  { "filename" : "menu-send@2x.png", "idiom" : "universal", "scale" : "2x" },
  { "appearances" : [ { "appearance" : "luminosity", "value" : "dark" } ], "filename" : "menu-send-dark@2x.png", "idiom" : "universal", "scale" : "2x" },
  { "filename" : "menu-send@3x.png", "idiom" : "universal", "scale" : "3x" }
], "info" : { "author" : "xcode", "version" : 1 } }
EOF
expect_failure "partial dark variants rejected" 'dark variant exists at ["2x"] but not at every exported scale'
cp "$fx/send-contents.orig" "$send/Contents.json"

# Vector imageset whose only file is the dark one.
cp "$app/Vec/logo.imageset/Contents.json" "$fx/logo-contents.orig"
echo '{ "images" : [ { "appearances" : [ { "appearance" : "luminosity", "value" : "dark" } ], "filename" : "logo.svg", "idiom" : "universal" } ], "info" : { "author" : "xcode", "version" : 1 } }' > "$app/Vec/logo.imageset/Contents.json"
expect_failure "dark-only vector rejected" 'vector imageset has only a dark-appearance file'
cp "$fx/logo-contents.orig" "$app/Vec/logo.imageset/Contents.json"

# Bare JSON number component.
colorset "$media/Colors/Numeric.colorset" '{ "color" : { "color-space" : "srgb", "components" : { "red" : 1, "green" : "0.0", "blue" : "0.0", "alpha" : "1.000" } }, "idiom" : "universal" }'
expect_failure "numeric component rejected" 'is not a string'
rm -rf "$media/Colors/Numeric.colorset"

# A colour set without any usable colour.
colorset "$media/Colors/Empty.colorset" "{ $dark, \"idiom\" : \"universal\" }"
expect_failure "empty colour set rejected" 'no universal light/any colour'
rm -rf "$media/Colors/Empty.colorset"

# With every edit undone the fixture generates again.
if swift "$repo/scripts/gen-tokens.swift" "$ios" "$ui" "$out" 2> "$fx/final.txt"; then
    echo "ok   fixture regenerates after negative cases"
else
    echo "FAIL fixture no longer generates"; cat "$fx/final.txt"; failures=$((failures + 1))
fi

echo "$failures failure(s)"
[ "$failures" -eq 0 ]
