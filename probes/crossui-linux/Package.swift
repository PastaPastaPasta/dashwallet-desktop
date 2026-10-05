// swift-tools-version:6.2
// Standalone G2 probe: SwiftCrossUI 0.10.0 + GtkBackend on Linux, sharing a
// Foundation+Observation-only view model with a headless test target.
import PackageDescription

let package = Package(
    name: "CrossUILinuxProbe",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/moreSwift/swift-cross-ui", exact: "0.10.0"),
    ],
    targets: [
        // Shared view-model layer: Foundation + Observation only (no UI imports).
        .target(name: "WalletProbeCore"),
        .executableTarget(
            name: "CrossUILinuxProbe",
            dependencies: [
                "WalletProbeCore",
                .product(name: "SwiftCrossUI", package: "swift-cross-ui"),
                .product(name: "GtkBackend", package: "swift-cross-ui"),
            ]
        ),
        .testTarget(name: "WalletProbeCoreTests", dependencies: ["WalletProbeCore"]),
    ]
)
