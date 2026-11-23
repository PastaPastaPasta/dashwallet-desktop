// swift-tools-version:5.10
// Vendored SwiftCrossUI 0.10.0 (stackotter/swift-cross-ui, tag 0.10.0,
// 0f3ec3958b79cdc39a1a3e604516b71ab33a9043), cut down to the targets
// dashwallet-desktop builds, with the patches listed in ../PATCHES.md.
//
// Kept: SwiftCrossUI (+ its macro plugin and metadata support), DefaultBackend,
// AppKitBackend (macOS development builds), GtkBackend + Gtk + CGtk +
// GtkCHelpers (Linux), WinUIBackend + WinUIInterop (Windows, design gate G3).
// Left out: the Android, UIKit, Gtk3, Curses/Qt/LVGL and Dummy backends,
// the porting kit, the GTK code generator, examples, benchmarks, tests and the
// DocC catalog, and with them the AndroidKit, swift-java, XMLCoder,
// swift-docc-plugin, swift-benchmark and swift-collections dependencies.
// The upstream compile-time options (SCUI_DEFAULT_BACKEND, SCUI_LIBRARY_TYPE,
// hot reloading, benchmark visualisation) are dropped: the products are
// SwiftPM's automatic library type and DefaultBackend picks the OS backend.

import CompilerPluginSupport
import PackageDescription

#if os(macOS)
    let defaultBackendDependencies: [Target.Dependency] = [
        .target(name: "AppKitBackend", condition: .when(platforms: [.macOS]))
    ]
#else
    let defaultBackendDependencies: [Target.Dependency] = [
        .target(name: "WinUIBackend", condition: .when(platforms: [.windows])),
        .target(name: "GtkBackend", condition: .when(platforms: [.linux])),
    ]
#endif

let package = Package(
    name: "swift-cross-ui",
    platforms: [.macOS(.v10_15)],
    products: [
        .library(name: "SwiftCrossUI", targets: ["SwiftCrossUI"]),
        .library(name: "AppKitBackend", targets: ["AppKitBackend"]),
        .library(name: "GtkBackend", targets: ["GtkBackend"]),
        .library(name: "WinUIBackend", targets: ["WinUIBackend"]),
        .library(name: "DefaultBackend", targets: ["DefaultBackend"]),
        .library(name: "Gtk", targets: ["Gtk"]),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-syntax.git", "601.0.0"..<"604.0.0"),
        .package(url: "https://github.com/stackotter/swift-macro-toolkit", .upToNextMinor(from: "0.9.0")),
        .package(url: "https://github.com/stackotter/swift-image-formats", .upToNextMinor(from: "0.5.0")),
        .package(url: "https://github.com/moreSwift/swift-winui", .upToNextMinor(from: "0.2.2")),
        .package(url: "https://github.com/swhitty/swift-mutex", .upToNextMinor(from: "0.0.6")),
        .package(url: "https://github.com/moreSwift/swift-observation-polyfill", .upToNextMinor(from: "0.1.1")),
        .package(url: "https://github.com/apple/swift-log.git", from: "1.6.4"),
    ],
    targets: [
        .target(
            name: "SwiftCrossUI",
            dependencies: [
                "SwiftCrossUIMacrosPlugin",
                "SwiftCrossUIMetadataSupport",
                .product(name: "ImageFormats", package: "swift-image-formats"),
                .product(name: "Logging", package: "swift-log"),
                .product(name: "Mutex", package: "swift-mutex"),
                .product(name: "ObservationPolyfillCore", package: "swift-observation-polyfill"),
                .product(name: "ObservationPolyfill", package: "swift-observation-polyfill"),
            ],
            swiftSettings: [.enableUpcomingFeature("StrictConcurrency")]
        ),
        .target(name: "SwiftCrossUIMetadataSupport"),
        .target(name: "DefaultBackend", dependencies: defaultBackendDependencies),
        .target(name: "AppKitBackend", dependencies: ["SwiftCrossUI"]),
        .target(name: "GtkBackend", dependencies: ["SwiftCrossUI", "Gtk", "CGtk"]),
        .systemLibrary(
            name: "CGtk",
            pkgConfig: "gtk4",
            providers: [
                .brew(["gtk4"]),
                .apt(["libgtk-4-dev clang"]),
            ]
        ),
        .target(name: "Gtk", dependencies: ["CGtk", "GtkCHelpers"], exclude: ["LICENSE.md"]),
        // Gtk helpers implemented in C (upstream: hard or impossible in Swift).
        .target(name: "GtkCHelpers", dependencies: ["CGtk"]),
        .target(
            name: "WinUIBackend",
            dependencies: [
                "SwiftCrossUI",
                "WinUIInterop",
                .product(name: "WinUI", package: "swift-winui"),
                .product(name: "UWP", package: "swift-winui"),
                .product(name: "CWinRT", package: "swift-winui"),
                .product(name: "WinAppSDK", package: "swift-winui"),
                .product(name: "WindowsFoundation", package: "swift-winui"),
                .product(name: "Mutex", package: "swift-mutex"),
            ]
        ),
        .target(name: "WinUIInterop", dependencies: []),
        .macro(
            name: "SwiftCrossUIMacrosPlugin",
            dependencies: [
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftSyntaxMacros", package: "swift-syntax"),
                .product(name: "SwiftCompilerPlugin", package: "swift-syntax"),
                .product(name: "MacroToolkit", package: "swift-macro-toolkit"),
            ]
        ),
    ]
)
