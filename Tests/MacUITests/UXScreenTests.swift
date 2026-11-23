// The UX-SPEC states that the M1/M2 screen tests do not show: Overview in
// discreet mode, while catching up and at the minimum window size; the
// Verify Message tab. With DWD_WRITE_SCREENSHOTS=1 the PNGs go to
// docs/screenshots/ux/mac/.
#if os(macOS)
import AppKit
import DashUIMac
import Foundation
import SwiftUI
import Testing
import WalletDemo
import WalletFeatures
import WalletRuntime

@testable import MacUI

@MainActor
@Suite(.serialized)
struct UXScreenTests {
    @Test(arguments: [ColorScheme.light, .dark])
    func QT039_overviewDiscreet(_ scheme: ColorScheme) async throws {
        let model = try await M2ScreenTests.model(.funded, scheme)
        let main = try #require(model.main)
        let home = try #require(main.home)
        main.settings.setDiscreet(true)
        try await ScreenTests.settle { home.discreet && home.recent.isEmpty }
        #expect(home.formattedTotal?.contains("#") == true)
        try await ScreenTests.capture(
            ScreenTests.chrome(model, OverviewView.make(model: model, main: main, home: home, perform: { _ in })),
            ScreenTests.mainSize, scheme, "overview-discreet")
    }

    /// The offline demo is behind the tip: the hero says "(out of sync)".
    @Test(arguments: [ColorScheme.light, .dark])
    func QT037_overviewOutOfSync(_ scheme: ColorScheme) async throws {
        let model = try await M2ScreenTests.model(.offline, scheme)
        let main = try #require(model.main)
        let home = try #require(main.home)
        main.hideSyncOverlay()
        #expect(home.outOfSync)
        try await ScreenTests.capture(
            ScreenTests.chrome(model, OverviewView.make(model: model, main: main, home: home, perform: { _ in })),
            ScreenTests.mainSize, scheme, "overview-out-of-sync")
    }

    /// The minimum window: the breakdown strip stacks, nothing is clipped.
    @Test(arguments: [ColorScheme.light])
    func overviewMinimumWindow(_ scheme: ColorScheme) async throws {
        let model = try await M2ScreenTests.model(.funded, scheme)
        let main = try #require(model.main)
        let home = try #require(main.home)
        try await ScreenTests.capture(
            ScreenTests.chrome(model, OverviewView.make(model: model, main: main, home: home, perform: { _ in })),
            CGSize(width: 920, height: 600), scheme, "overview-920x600")
    }

    @Test(arguments: [ColorScheme.light, .dark])
    func QT100_verifyMessageTab(_ scheme: ColorScheme) async throws {
        let model = try await ScreenTests.model(.funded, scheme)
        model.signVerifyTab = .verify
        try await ScreenTests.capture(
            SignVerifyWindow(model: model), CGSize(width: 640, height: 560), scheme, "sign-verify-verify")
    }
}
#endif
