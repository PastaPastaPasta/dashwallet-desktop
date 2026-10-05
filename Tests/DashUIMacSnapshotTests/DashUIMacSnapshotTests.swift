#if os(macOS)
import AppKit
import DesignTokens
import SwiftUI
import Testing
@testable import DashUIMac

/// Renders every gallery sample in light and dark appearance and compares it with the reference
/// PNG in __Snapshots__ (`DWD_RECORD_SNAPSHOTS=1` re-records them).
@Suite("DashUIMac snapshots")
@MainActor
struct DashUIMacSnapshotTests {
    static let sampleIDs = DashUIMacGallery.samples.map(\.id)

    @Test("Gallery sample matches its reference image", arguments: sampleIDs, [ColorScheme.light, .dark])
    func sampleMatchesReference(id: String, scheme: ColorScheme) throws {
        let sample = try #require(DashUIMacGallery.samples.first { $0.id == id })
        let image = try #require(Snapshot.render(sample.view, width: sample.width, scheme: scheme))
        let bitmap = try #require(Bitmap(cgImage: image))
        // A blank render (a view ImageRenderer cannot draw) has only the background colour.
        #expect(bitmap.coarseColorCount > 2, "\(id) rendered blank")
        try Snapshot.assertMatches(image, named: "\(id)-\(scheme == .light ? "light" : "dark")")
    }

    @Test("Light and dark renders differ", arguments: sampleIDs)
    func appearancesDiffer(id: String) throws {
        let sample = try #require(DashUIMacGallery.samples.first { $0.id == id })
        let light = try #require(Snapshot.render(sample.view, width: sample.width, scheme: .light).flatMap(Bitmap.init))
        let dark = try #require(Snapshot.render(sample.view, width: sample.width, scheme: .dark).flatMap(Bitmap.init))
        #expect(light.width == dark.width && light.height == dark.height)
        if light.width == dark.width && light.height == dark.height {
            #expect(light.differingFraction(from: dark) > 0.05)
        }
    }

    @Test("The gallery view renders both columns")
    func galleryRenders() throws {
        let renderer = ImageRenderer(content: DashUIMacGallery().frame(width: 1200, height: 900))
        renderer.scale = 1
        let image = try #require(renderer.cgImage)
        #expect(image.width == 1200)
    }
}

@Suite("DashUIMac tokens and icons")
struct DashUIMacTokenTests {
    @Test("Colour tokens resolve per appearance")
    func colorResolvesPerAppearance() throws {
        let color = NSColor(dash: DashColor.primaryText)
        let light = try #require(NSAppearance(named: .aqua))
        let dark = try #require(NSAppearance(named: .darkAqua))
        var lightRGB: NSColor?
        var darkRGB: NSColor?
        light.performAsCurrentDrawingAppearance { lightRGB = color.usingColorSpace(.sRGB) }
        dark.performAsCurrentDrawingAppearance { darkRGB = color.usingColorSpace(.sRGB) }
        let l = try #require(lightRGB)
        let d = try #require(darkRGB)
        #expect(abs(l.redComponent - DashColor.primaryText.light.red) < 0.002)
        #expect(abs(d.redComponent - DashColor.primaryText.dark.red) < 0.002)
        #expect(abs(d.alphaComponent - DashColor.primaryText.dark.alpha) < 0.002)
    }

    @Test("Every exported icon loads from the bundle in both appearances")
    func everyIconLoads() {
        let library = DashIconLibrary.bundled
        #expect(library.directory != nil)
        for token in DashIconToken.allCases {
            for appearance in DashAppearance.allCases {
                let image = library.image(for: token, appearance: appearance)
                #expect(image != nil, "\(token.rawValue) \(appearance)")
                if let image { #expect(image.size.width > 0 && image.size.height > 0, "\(token.rawValue)") }
            }
        }
    }

    @Test("PNG icons use the @2x pixel size as points times two and keep both scales")
    func pngIconGeometry() throws {
        let image = try #require(DashIconLibrary.bundled.image(for: .toastInfo, appearance: .light))
        #expect(image.size == NSSize(width: 18, height: 18))
        #expect(image.representations.count == 2)
    }

    @Test("A library without a directory returns no images")
    func missingDirectory() {
        #expect(DashIconLibrary(directory: nil).image(for: .send, appearance: .light) == nil)
    }

    @Test("Dark variants are picked only where they exist")
    func darkVariant() throws {
        let library = DashIconLibrary.bundled
        let lightSend = try #require(library.image(for: .send, appearance: .light))
        let darkSend = try #require(library.image(for: .send, appearance: .dark))
        #expect(lightSend !== darkSend)
        let lightToast = try #require(library.image(for: .toastInfo, appearance: .light))
        let darkToast = try #require(library.image(for: .toastInfo, appearance: .dark))
        #expect(lightToast === darkToast)
    }
}

@Suite("DashUIMac component logic")
@MainActor
struct DashUIMacComponentTests {
    private struct Row: Identifiable {
        let id: Int
        let name: String
        let value: Int
    }

    private let rows = [Row(id: 1, name: "b", value: 3), Row(id: 2, name: "a10", value: 1),
                        Row(id: 3, name: "a9", value: 2)]

    private func table(_ order: DataTableSortOrder?) -> DataTable<Row> {
        DataTable(
            rows: rows,
            columns: [
                DataTableColumn("Name", id: "name", value: \.name),
                DataTableColumn("Value", id: "value", sortBy: { $0.value < $1.value }) { Text("\($0.value)") },
                DataTableColumn("Fixed", id: "fixed", sortable: false, value: \.name),
            ],
            selection: .constant([]),
            sortOrder: .constant(order)
        )
    }

    @Test("DataTable keeps row order without a sort order")
    func unsorted() {
        #expect(table(nil).displayedRows.map(\.id) == [1, 2, 3])
    }

    @Test("DataTable sorts text columns with localized standard compare")
    func textSort() {
        #expect(table(DataTableSortOrder(columnID: "name")).displayedRows.map(\.name) == ["a9", "a10", "b"])
        #expect(table(DataTableSortOrder(columnID: "name", ascending: false)).displayedRows.map(\.name)
            == ["b", "a10", "a9"])
    }

    @Test("DataTable sorts by a custom ordering")
    func customSort() {
        #expect(table(DataTableSortOrder(columnID: "value")).displayedRows.map(\.id) == [2, 3, 1])
    }

    @Test("DataTable ignores unsortable or unknown columns")
    func unsortable() {
        #expect(table(DataTableSortOrder(columnID: "fixed")).displayedRows.map(\.id) == [1, 2, 3])
        #expect(table(DataTableSortOrder(columnID: "missing")).displayedRows.map(\.id) == [1, 2, 3])
    }

    @Test("QRView accepts only complete square matrices")
    func qrValidity() {
        #expect(QRView(size: 2, modules: [true, false, false, true], accessibilityLabel: "").isValid)
        #expect(!QRView(size: 2, modules: [true], accessibilityLabel: "").isValid)
        #expect(!QRView(size: 0, modules: [], accessibilityLabel: "").isValid)
    }

    @Test("QRView draws dark modules where the matrix is true")
    func qrPixels() throws {
        // 2×2 matrix, no quiet zone, 20 pt: the top-left and bottom-right 10 pt quadrants are dark.
        let view = QRView(size: 2, modules: [true, false, false, true], quietZone: 0, accessibilityLabel: "")
            .frame(width: 20, height: 20)
        let renderer = ImageRenderer(content: view)
        renderer.scale = 1
        let bitmap = try #require(renderer.cgImage.flatMap(Bitmap.init))
        func luminance(_ x: Int, _ y: Int) -> Int { Int(bitmap.bytes[(y * bitmap.width + x) * 4]) }
        #expect(luminance(5, 5) < 40)
        #expect(luminance(15, 15) < 40)
        #expect(luminance(15, 5) > 215)
        #expect(luminance(5, 15) > 215)
    }

    @Test("Strength meter fills one to four segments")
    func strengthSegments() {
        #expect(PassphraseStrength.allCases.map(\.filledSegments) == [1, 1, 2, 3, 4])
        #expect(PassphraseStrength.weak < .strong)
    }
}
#endif
