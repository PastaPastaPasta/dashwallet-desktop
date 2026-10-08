// swift-tools-version:6.2
// Single root package for dashwallet-desktop (DESIGN-opus §1.6, §3.2).
//
// The Rust core arrives as an SE-0482 static-library artifact bundle built by
// scripts/build-core.sh into Artifacts/ (gitignored). Run that script before
// `swift build`.
//
// DWD_HEADLESS=1 drops every SwiftCrossUI-based target. The swift-cross-ui
// dependency stays declared, so headless resolution keeps its dependencies'
// pins in Package.resolved; SwiftPM only builds the targets that remain. Use
// it for headless CI and Linux containers without GTK.
//
// SwiftCrossUI is the vendored, patched 0.10.0 in Vendor/swift-cross-ui
// (Vendor/PATCHES.md), a local package: no fetch of the fork itself.
import Foundation
import PackageDescription

let headless = ProcessInfo.processInfo.environment["DWD_HEADLESS"] == "1"

let crossUI: [Target.Dependency] = [
    .product(name: "SwiftCrossUI", package: "swift-cross-ui"),
    .product(name: "DefaultBackend", package: "swift-cross-ui"),
]

// The native backend of each OS, for code that reaches the native widgets
// through SwiftCrossUI's `inspect` hooks (list-row names until fork patch
// P4, the quit hook, the GTK application name). Windows has none yet.
let nativeBackend: [Target.Dependency] = [
    .product(name: "GtkBackend", package: "swift-cross-ui", condition: .when(platforms: [.linux])),
    .product(name: "Gtk", package: "swift-cross-ui", condition: .when(platforms: [.linux])),
    .product(name: "AppKitBackend", package: "swift-cross-ui", condition: .when(platforms: [.macOS])),
]

var products: [Product] = [
    .library(name: "DashKit", targets: ["DashKit"]),
    .library(name: "WalletRuntime", targets: ["WalletRuntime"]),
    .library(name: "AppServices", targets: ["AppServices"]),
    .library(name: "WalletFeatures", targets: ["WalletFeatures"]),
    .library(name: "MacUI", targets: ["MacUI"]),
    // The macOS app's delegate builds the Dock menu with it (AppKit stays out of MacUI).
    .library(name: "PlatformServicesMac", targets: ["PlatformServicesMac"]),
]

let dependencies: [Package.Dependency] = [
    .package(path: "Vendor/swift-cross-ui"),
    // Already a dependency of SwiftCrossUI (same pin in Package.resolved); DashUICross decodes and
    // tints the exported PNG icons with it before handing them to `Image`.
    .package(url: "https://github.com/stackotter/swift-image-formats", .upToNextMinor(from: "0.5.0")),
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
    // Demo mode (`--demo`) for both apps: the WalletRuntime service protocols
    // over in-memory sample wallets, with the engine's rules and its pure
    // functions (units, URIs, QR, message verification, mnemonics).
    .target(name: "WalletDemo", dependencies: ["WalletRuntime", "WalletFeatures", "PlatformServices"]),
    // DashUIKit is vendored into Sources/DashUIMac/DashUIKit (see Sources/DashUIMac/VENDORED.md).
    // Sources/DashUIMac/Resources/Icons is a symlink to the repository's exported icon set
    // (Resources/Icons); `.process` copies its files flat into the resource bundle.
    .target(
        name: "DashUIMac",
        dependencies: ["DesignTokens"],
        exclude: ["VENDORED.md"],
        resources: [.process("Resources/Icons")]
    ),
    // Every MacUI source is wrapped in `#if os(macOS)`; elsewhere the module is empty.
    .target(
        name: "MacUI",
        dependencies: [
            .target(name: "DashUIMac", condition: .when(platforms: [.macOS])),
            "DesignTokens",
            "WalletDemo",
            "WalletFeatures",
            "WalletRuntime",
            "PlatformServices",
            .target(name: "PlatformServicesMac", condition: .when(platforms: [.macOS])),
        ]
    ),
    .testTarget(name: "DashKitTests", dependencies: ["DashKit"]),
    // The demo services against the engine's rules (grants, sends, addresses).
    .testTarget(name: "WalletDemoTests", dependencies: ["WalletDemo", "WalletFeatures", "WalletRuntime"]),
    .testTarget(name: "WalletRuntimeTests", dependencies: ["WalletRuntime", "DashKit", "PlatformServices"]),
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
    // macOS-only: renders the MacUI screens over the demo services; with
    // DWD_WRITE_SCREENSHOTS=1 it writes them to docs/screenshots/m1.
    .testTarget(
        name: "MacUITests",
        dependencies: [
            .target(name: "MacUI", condition: .when(platforms: [.macOS])),
            .target(name: "PlatformServicesMac", condition: .when(platforms: [.macOS])),
            "WalletDemo",
            "WalletFeatures",
            "WalletRuntime",
        ]
    ),
    .testTarget(name: "RepoChecksTests"),
    // DashKit: the umask 002 test opens the engine on a prepared data root.
    .testTarget(
        name: "PlatformServicesDesktopTests", dependencies: ["PlatformServicesDesktop", "PlatformServices", "DashKit"]),
    // macOS-only: the sources compile to nothing elsewhere. The keychain test
    // skips itself without Touch ID or outside a signed app.
    .testTarget(
        name: "PlatformServicesMacTests",
        dependencies: [
            "PlatformServices", .target(name: "PlatformServicesMac", condition: .when(platforms: [.macOS])),
        ]
    ),
]

if !headless {
    products.append(.executable(name: "dash-wallet", targets: ["DashWalletCross"]))
    targets += [
        // Sources/DashUICross/Resources/Icons is a symlink to the exported icon set (as for
        // DashUIMac); Resources/Fonts holds Inter (OFL), the Linux/Windows UI face (UX-SPEC §2.2).
        .target(
            name: "DashUICross",
            dependencies: ["DesignTokens", .product(name: "ImageFormats", package: "swift-image-formats")]
                + crossUI + nativeBackend,
            resources: [.process("Resources/Icons"), .copy("Resources/Fonts")]
        ),
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
        // Colour roles, text rules, icon tinting and bundled resources of DashUICross.
        .testTarget(
            name: "DashUICrossTests",
            dependencies: [
                "DashUICross", "DesignTokens", .product(name: "ImageFormats", package: "swift-image-formats"),
            ]
        ),
        // The vendored SwiftCrossUI's patches that need no backend (Vendor/PATCHES.md).
        .testTarget(
            name: "SwiftCrossUIPatchTests",
            dependencies: [.product(name: "SwiftCrossUI", package: "swift-cross-ui")]
        ),
        // Composition root: live runtime over the engine, or the --demo services.
        .executableTarget(
            name: "DashWalletCross",
            dependencies: [
                "CrossUI", "DashUICross", "WalletDemo", "WalletFeatures", "WalletRuntime", "PlatformServices",
                "PlatformServicesDesktop", "DesignTokens",
            ] + crossUI + nativeBackend
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
