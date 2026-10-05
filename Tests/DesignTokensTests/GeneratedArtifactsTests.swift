import Foundation
import Testing
@testable import DesignTokens

/// Repository root, found from this file's real location (symlinks resolved so the tests also work from a
/// scratch package that links the sources in).
private let repoRoot: URL = URL(fileURLWithPath: #filePath)
    .resolvingSymlinksInPath()
    .deletingLastPathComponent()  // DesignTokensTests
    .deletingLastPathComponent()  // Tests
    .deletingLastPathComponent()

private let iconsDir = repoRoot.appendingPathComponent("Resources/Icons")

private func loadTokensJSON() throws -> [String: Any] {
    let url = repoRoot.appendingPathComponent("Resources/Tokens/tokens.json")
    let data = try Data(contentsOf: url)
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
}

@Suite("Typography, spacing and radii")
struct TypographyTests {
    @Test("Type scale matches DashUIKit")
    func scale() {
        #expect(DashTextStyle.allStyles.count == 16)
        #expect(DashTextStyle.largeTitle == DashTextStyle(name: "largeTitle", size: 34, weight: .bold, lineHeight: 41))
        #expect(DashTextStyle.body == DashTextStyle(name: "body", size: 17, weight: .regular, lineHeight: 22))
        #expect(DashTextStyle.calloutMedium.weight == .semibold)
        #expect(DashTextStyle.caption2 == DashTextStyle(name: "caption2", size: 11, weight: .regular, lineHeight: 13))
        #expect(DashFontWeight.semibold.rawValue == 600)
        #expect(Set(DashTextStyle.allStyles.map(\.name)).count == 16)
        for style in DashTextStyle.allStyles {
            #expect(style.lineHeight > style.size, "\(style.name)")
            #expect(style.tracking == 0, "\(style.name)")
        }
        let sizes = DashTextStyle.allStyles.map(\.size)
        #expect(sizes == sizes.sorted(by: >), "scale is ordered largest first")
    }

    @Test("Typography.swift agrees with the scale the generator parsed from DashUIKit")
    func matchesTokensJSON() throws {
        let json = try loadTokensJSON()
        let parsed = try #require(json["typography"] as? [[String: Any]])
        #expect(parsed.count == DashTextStyle.allStyles.count)
        for (entry, style) in zip(parsed, DashTextStyle.allStyles) {
            #expect(entry["name"] as? String == style.name)
            #expect((entry["size"] as? Double) == style.size, "\(style.name) size")
            #expect((entry["lineHeight"] as? Double) == style.lineHeight, "\(style.name) line height")
            #expect(entry["weight"] as? String == "\(style.weight)", "\(style.name) weight")
        }
    }

    @Test("Spacing, radii and button metrics")
    func metrics() {
        #expect(DashRadius.standard == 12)
        #expect(DashRadius.card == 20)
        #expect(DashSpacing.stack == 15)
        #expect(DashSpacing.screenHorizontal == 20)
        #expect(DashButtonMetrics.large.verticalPadding == 14)
        #expect(DashButtonMetrics.large.cornerRadius == 16)
        #expect(DashButtonMetrics.extraSmall.horizontalPadding == 8)
        let steps = [DashSpacing.xxxs, DashSpacing.xxs, DashSpacing.xs, DashSpacing.s, DashSpacing.sm, DashSpacing.m,
                     DashSpacing.l, DashSpacing.xl, DashSpacing.xxl, DashSpacing.xxxl]
        #expect(steps == steps.sorted() && Set(steps).count == steps.count)
    }
}

@Suite("Generated artifacts")
struct GeneratedArtifactsTests {
    @Test("tokens.json colours match the generated Swift tokens")
    func colorsMatchJSON() throws {
        let json = try loadTokensJSON()
        let colors = try #require(json["colors"] as? [String: [[String: Any]]])
        let namespaces: [(String, [String: DashColor])] = [
            ("DashColor", DashColor.allTokens), ("DashColor.App", DashColor.App.allTokens),
        ]
        for (ns, tokens) in namespaces {
            let entries = try #require(colors[ns])
            #expect(entries.count == tokens.count, "\(ns) count")
            for entry in entries {
                let name = try #require(entry["token"] as? String)
                let color = try #require(tokens[name], "\(ns).\(name) missing from Swift")
                for appearance in DashAppearance.allCases {
                    let slot = try #require(entry["\(appearance)"] as? [String: Any])
                    let srgb = try #require(slot["srgb"] as? [Double])
                    let c = color.resolved(for: appearance)
                    // JSONSerialization does not always parse decimals to the nearest double; allow 1e-12.
                    let swift = [c.red, c.green, c.blue, c.alpha]
                    #expect(srgb.count == 4 && zip(srgb, swift).allSatisfy { abs($0 - $1) < 1e-12 },
                            "\(ns).\(name) \(appearance): json \(srgb) swift \(swift)")
                }
            }
        }
    }

    @Test("Purple colour sets are recorded as excluded")
    func exclusionsRecorded() throws {
        let json = try loadTokensJSON()
        let excluded = try #require(json["excluded"] as? [[String: Any]])
        #expect(excluded.contains { $0["asset"] as? String == "Purple" })
    }

    @Test("Icon set: at most 120 icons, every file present, no unlisted files")
    func iconFiles() throws {
        #expect(DashIconToken.allCases.count <= 120)
        #expect(DashIconToken.allCases.count == 119)
        let present = Set(try FileManager.default.contentsOfDirectory(atPath: iconsDir.path))
        var expected: Set<String> = ["NOTICE"]
        for icon in DashIconToken.allCases {
            for file in icon.fileNames {
                #expect(present.contains(file), "\(icon) is missing \(file)")
                expected.insert(file)
            }
        }
        #expect(present.subtracting(expected).isEmpty, "unlisted files: \(present.subtracting(expected).sorted())")
    }

    @Test("Icons.swift agrees with tokens.json and every icon passed the purple check")
    func iconsMatchJSON() throws {
        let json = try loadTokensJSON()
        let icons = try #require(json["icons"] as? [[String: Any]])
        #expect(icons.map { $0["file"] as? String } == DashIconToken.allCases.map(\.rawValue))
        for entry in icons {
            let icon = try #require(DashIconToken(rawValue: entry["file"] as? String ?? ""))
            #expect(Set(entry["files"] as? [String] ?? []) == Set(icon.fileNames), "\(icon)")
            #expect(entry["purpleCheck"] as? String == "passed", "\(icon)")
            #expect(entry["group"] as? String == icon.metadata.group.rawValue, "\(icon)")
        }
    }

    @Test("Icon lookups")
    func iconLookups() {
        #expect(DashIconToken.tabHome.metadata.group == .tab)
        #expect(DashIconToken.tabHome.metadata.isTemplate)
        #expect(DashIconToken.tabHome.fileName(for: .light) == "tab-home@2x.png")
        #expect(DashIconToken.tabHome.fileName(for: .dark, scale: 3) == "tab-home@3x.png")
        #expect(DashIconToken.send.metadata.hasDarkVariant)
        #expect(DashIconToken.send.fileName(for: .dark) == "action-send-dark@2x.png")
        #expect(DashIconToken.send.fileName(for: .light, scale: 1) == "action-send@2x.png")
        #expect(DashIconToken.send.fileName(for: .light, scale: 4) == "action-send@3x.png")
        #expect(DashIconToken.wallets.metadata.format == .svg)
        #expect(DashIconToken.wallets.fileNames == ["settings-wallets.svg"])
        let groups = Set(DashIconToken.allCases.map(\.metadata.group))
        for required: DashIconGroup in [.tab, .action, .transaction, .coinjoin, .masternode, .governance, .settings] {
            #expect(groups.contains(required), "\(required)")
        }
    }

    @Test("Icon NOTICE carries the MIT licence text")
    func notice() throws {
        let text = try String(contentsOf: iconsDir.appendingPathComponent("NOTICE"), encoding: .utf8)
        #expect(text.contains("Permission is hereby granted, free of charge"))
        #expect(text.contains("Dash Core Group"))
    }
}
