#!/bin/sh
# Builds a scratch SwiftPM package that links in Sources/DesignTokens and Tests/DesignTokensTests, runs
# `swift test` on it, then deletes it. Lets the DesignTokens target be tested on its own, without the root
# Package.swift. Usage: scripts/test-design-tokens.sh [extra swift test args]
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/design-tokens-check.XXXXXX")
trap 'rm -rf "$scratch"' EXIT INT TERM

mkdir -p "$scratch/Sources" "$scratch/Tests"
ln -s "$repo/Sources/DesignTokens" "$scratch/Sources/DesignTokens"
ln -s "$repo/Tests/DesignTokensTests" "$scratch/Tests/DesignTokensTests"
cat > "$scratch/Package.swift" <<'EOF'
// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "DesignTokensCheck",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "DesignTokens"),
        .testTarget(name: "DesignTokensTests", dependencies: ["DesignTokens"]),
    ]
)
EOF

cd "$scratch"
swift test "$@"
