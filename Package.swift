// swift-tools-version:6.2
// Single root package for dashwallet-desktop (DESIGN-opus §1.6, §3.2).
//
// The Rust core arrives as an SE-0482 static-library artifact bundle built by
// scripts/build-core.sh into Artifacts/ (gitignored). Run that script before
// `swift build`.
//
// DWD_HEADLESS=1 drops every SwiftCrossUI-based target. The swift-cross-ui
// dependency stays declared, so headless resolution keeps its pin in
// Package.resolved; SwiftPM only builds the targets that remain. Use it for
// headless CI and Linux containers without GTK.
import Foundation
import PackageDescription

let headless = ProcessInfo.processInfo.environment["DWD_HEADLESS"] == "1"

let crossUI: [Target.Dependency] = [
    .product(name: "SwiftCrossUI", package: "swift-cross-ui"),
    .product(name: "DefaultBackend", package: "swift-cross-ui"),
]

var products: [Product] = [
    .library(name: "DashKit", targets: ["DashKit"]),
    .library(name: "WalletRuntime", targets: ["WalletRuntime"]),
    .library(name: "AppServices", targets: ["AppServices"]),
    .library(name: "WalletFeatures", targets: ["WalletFeatures"]),
    .library(name: "MacUI", targets: ["MacUI"]),
]

// TODO(fork): switch to the dashpay/swift-cross-ui fork pinned by revision.
let dependencies: [Package.Dependency] = [
    .package(url: "https://github.com/stackotter/swift-cross-ui", exact: "0.10.0"),
]

var targets: [Target] = [
    // Rust core: C header + module map + libdashwallet_core.a
    .binaryTarget(name: "DashWalletCoreFFI", path: "Artifacts/DashWalletCore.artifactbundle"),
    // UniFFI-generated Swift (committed). Swift 5 mode: generated code is not
    // strict-concurrency clean.
    .target(
        name: "DashWalletCore",
        dependencies: ["DashWalletCoreFFI"],
        swiftSettings: [.swiftLanguageMode(.v5)]
    ),
    .target(name: "DashKit", dependencies: ["DashWalletCore"]),
    .target(name: "PlatformServices"),
    .target(name: "PlatformServicesMac", dependencies: ["PlatformServices"]),
    .target(name: "PlatformServicesDesktop", dependencies: ["PlatformServices", "DashKit"]),
    .target(name: "WalletRuntime", dependencies: ["DashKit", "PlatformServices"]),
    .target(name: "AppServices", dependencies: ["DashKit", "PlatformServices"]),
    .target(name: "DesignTokens"),
    .target(
        name: "WalletFeatures",
        dependencies: ["WalletRuntime", "AppServices", "PlatformServices", "DesignTokens"]
    ),
    // DashUIKit is vendored into Sources/DashUIMac/DashUIKit (see Sources/DashUIMac/VENDORED.md).
    // Sources/DashUIMac/Resources/Icons is a symlink to the repository's exported icon set
    // (Resources/Icons); `.process` copies its files flat into the resource bundle.
    .target(
        name: "DashUIMac",
        dependencies: ["DesignTokens"],
        exclude: ["VENDORED.md"],
        resources: [.process("Resources/Icons")]
    ),
    .target(
        name: "MacUI",
        dependencies: [
            .target(name: "DashUIMac", condition: .when(platforms: [.macOS])),
            "WalletFeatures",
            .target(name: "PlatformServicesMac", condition: .when(platforms: [.macOS])),
        ]
    ),
    .testTarget(name: "DashKitTests", dependencies: ["DashKit"]),
    .testTarget(name: "WalletRuntimeTests", dependencies: ["WalletRuntime", "DashKit"]),
    .testTarget(name: "DesignTokensTests", dependencies: ["DesignTokens"]),
    // View-model flows against in-memory fakes of the WalletRuntime contracts.
    // testdata/amount_format.json is read by path, not bundled.
    .testTarget(name: "WalletFeaturesTests", dependencies: ["WalletFeatures", "WalletRuntime"]),
    // macOS-only: the sources compile to nothing elsewhere. Reference PNGs are read from
    // __Snapshots__ by path, not bundled.
    .testTarget(
        name: "DashUIMacSnapshotTests",
        dependencies: ["DashUIMac", "DesignTokens"],
        exclude: ["__Snapshots__"]
    ),
    .testTarget(name: "RepoChecksTests"),
    .testTarget(name: "PlatformServicesDesktopTests", dependencies: ["PlatformServicesDesktop"]),
]

if !headless {
    products.append(.executable(name: "dash-wallet", targets: ["DashWalletCross"]))
    targets += [
        .target(name: "DashUICross", dependencies: ["DesignTokens"] + crossUI),
        .target(
            name: "CrossUI",
            dependencies: [
                "DashUICross",
                "DesignTokens",
                "WalletFeatures",
                // Value types the view models expose (Amount, TxRecord, DashNetwork, ...).
                "WalletRuntime",
                .target(name: "PlatformServicesDesktop", condition: .when(platforms: [.linux, .windows])),
            ] + crossUI
        ),
        // Composition root: live runtime over the engine, or --demo fakes.
        .executableTarget(
            name: "DashWalletCross",
            dependencies: [
                "CrossUI", "DashUICross", "WalletFeatures", "WalletRuntime", "PlatformServices",
                "PlatformServicesDesktop", "DashKit", "DashWalletCore", "DesignTokens",
            ] + crossUI
        ),
    ]
}

let package = Package(
    name: "DashWalletDesktop",
    defaultLocalization: "en",
    platforms: [.macOS(.v14)],
    products: products,
    dependencies: dependencies,
    targets: targets
)
