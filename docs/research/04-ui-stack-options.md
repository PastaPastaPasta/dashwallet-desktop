# 04 — UI stack options for a cross-platform desktop Dash wallet

*Research date: 2026-10-05. Facts come from web sources fetched that day, GitHub API metadata, and hands-on builds on this Mac (Xcode 26.6 / Swift 6.3.3, arm64). Sources are listed at the end.*

---

## 0. Decision summary

**Recommendation, ranked:**

| Rank | Option | Verdict |
|---|---|---|
| **1** | **B — Shared Swift core (`@Observable` ViewModels + services) → native SwiftUI on macOS, SwiftCrossUI on Linux (GtkBackend) and Windows (WinUIBackend)** | **Go, with conditions.** It is the closest to "built like the iOS app" and is proven to work on macOS (§2). Linux is credible. **Windows is the open risk.** Gate Windows on a time-boxed spike (§7), because WinUIBackend currently crashes under UI Automation clients and exposes nothing to screen readers. |
| 2 | A — SwiftCrossUI on all three, including AppKitBackend on macOS | The fallback if we can't fund two view trees. Same Windows/Linux risk as B, and a worse macOS app than SwiftUI for little saving. |
| 3 | C1 — Rust UI on the Rust core (egui as dash-evo-tool does, or Slint) | The lowest-risk way to ship on all three OSes **today**. Dash already ships it (dash-evo-tool: weekly dmg/flatpak/Windows builds, egui 0.36, AccessKit, egui_kittest). It fails the "built like iOS" goal and does not feel native. **This is the Windows escape hatch if the B gate fails.** |
| 4 | C2 — Compose Multiplatform desktop on kotlin-sdk | Needs kotlin-sdk ported from an Android library to KMP/JVM, plus desktop JNI builds. JVM bundle. No accessibility on Linux. |
| 5 | C3 — Tauri (web UI), Flutter, Qt/QML | Each is viable in general. None is closer to the iOS stack than C1, and each adds a third language or runtime. |
| ✗ | Tokamak, Adwaita-for-Swift, SwiftGodot, raw swift-winrt UI | Not suitable (§4.5). |

**Two blockers apply to every Swift option (A and B), whichever UI we pick:**

1. **SwiftDashSDK as it stands does not build off Apple platforms.** Of its 257 Swift files, 154 `import SwiftData` (141 `@Model` types). SwiftData is Apple-only. The Rust core ships as an `.xcframework` binary target, which is also Apple-only. `PlatformWalletManager` is a Combine `ObservableObject`, and secrets go through Keychain, `Security`, `LocalAuthentication` and `CryptoKit`. The good news is that this is contained: 148 of the 151 files under `Persistence/` hold the SwiftData use, while `KeyWallet/` (21 files), `Address/`, `DPP/` and most of `FFI/` and `PlatformWallet/` don't touch it. **Prerequisite work:** split SwiftDashSDK into a portable `SwiftDashSDKCore` with a persistence protocol, plus an Apple-only `SwiftDashSDKSwiftData` adapter. Ship the Rust FFI as an SE-0482 artifact bundle for Linux and Windows. `rs-platform-wallet-storage` already gives cross-platform SQLite persistence and an OS-keyring/encrypted-vault secret store. It is the natural desktop backend, but no FFI crate exposes it yet.
2. **Use Observation `@Observable`, not `ObservableObject`/Combine, in the shared layer.** Combine does not exist on Linux or Windows. The iOS app today has about 99 `ObservableObject` files and 120 `import Combine` files, so its ViewModels can't be lifted into the desktop app unchanged. They need porting to `@Observable` (§3).

---

## 1. Hard facts about our own stack (verified 2026-10-05)

| Item | Finding | How verified |
|---|---|---|
| SwiftDashSDK manifest | `swift-tools-version: 6.0`, platforms `.iOS(.v18), .macOS(.v15)`. Rust comes in as `.binaryTarget(name: "DashSDKFFI", path: "DashSDKFFI.xcframework")`, and it links `SystemConfiguration`. | sparse clone of `dashpay/platform@bc32136` (2026-10-04), `packages/swift-sdk/Package.swift` |
| SwiftData footprint | 154/257 files import SwiftData, with 141 `@Model` and 14 `ModelContext` uses. 148 of these are in `Persistence/`; the rest are 2 in `FFI/`, 2 in `PlatformWallet/`, 1 in `Core/` and 1 in `Services/`. | grep on the sparse clone |
| Other Apple-only APIs | `Security`/Keychain (4 files), `CryptoKit` (4), `LocalAuthentication` (1), `CoreData` (2, legacy store bridge), and `Combine` `@Published` in `PlatformWalletManager*` and `DataManager`. | grep |
| Rust FFI crates | `rs-unified-sdk-ffi` builds `staticlib` + `cdylib`. `platform-wallet-ffi` builds `staticlib` + `cdylib` + `rlib`. Neither depends on `platform-wallet-storage`. | Cargo.toml |
| `platform-wallet-storage` | "SQLite persistence and keyring_core secret backends (encrypted-file + OS keyring)". WAL, backup/restore, migrations. It promises that no private keys go into the DB. dash-evo-tool already uses it. | crate README + Cargo.toml |
| kotlin-sdk | **Android library only**: AGP `android.library`, Room, artifact `dash-sdk-android`, with JNI `.so` built only for Android ABIs by cargo-ndk. There is no KMP/JVM-desktop target. | `packages/kotlin-sdk/sdk/build.gradle.kts` |
| dashwallet-ios | Mixed UIKit/SwiftUI: about 240 files import UIKit, about 241 import SwiftUI, about 120 import Combine, about 99 use `ObservableObject`, 1 uses `@Observable`. About 241 files reference SwiftDashSDK. | GitHub code search, `dashpay/dashwallet-ios@develop` |
| dash-evo-tool | egui/eframe **0.36.2** (wgpu), `egui_kittest` 0.36.2, Rust 1.98. Consumes `dash-sdk`, `platform-wallet` and `platform-wallet-storage` from platform as Rust crates. Weekly releases (latest `v1.0.0-weekly.20260929`) ship macOS dmg (arm64 + x86_64), Linux flatpak and zip (x86_64 + aarch64), and a Windows zip. | GitHub API |

---

## 2. Hands-on results on this Mac

Environment: `swift-driver 1.148.6, Apple Swift 6.3.3 (swiftlang-6.3.3.1.3)`, `Xcode 26.6 (17F113)`, target `arm64-apple-macosx26.0`. Rust 1.97.1. `swiftly` is not installed and not needed on macOS.

| Check | Result |
|---|---|
| `brew list gtk4` | **Not installed** ("No such keg"). The Homebrew formula is gtk4 4.24.1 with 15 deps. I didn't install it, since GtkBackend belongs on Linux CI. `qt 6.11.2` and `pkgconf 3.0.7` are installed. |
| **Probe 1** `scratch/scui-hello`: SwiftCrossUI **0.10.0** + `AppKitBackend`, `@Observable` VM in `@State` | **Builds.** `swift build`: "Build complete! (68.61s)" cold, debug. The binary is 14 MB debug. The dependency graph is 23 packages, including swift-syntax 603.0.2, swift-winui 0.2.2, swift-java, AndroidKit, SwiftKotlin, image codecs and swift-observation-polyfill. The only warnings are the GTK pkg-config hints. |
| **Probe 2** `scratch/shared-vm-probe`: **the Option B shape** | **Builds and tests pass.** One `WalletCore` target (only `Foundation` + `Observation`, `@MainActor @Observable WalletViewModel`) calls a **Rust `staticlib` linked through an SE-0482 `staticLibrary` artifact bundle**, with a C header and modulemap. Two front ends use the *same* ViewModel: `MacSwiftUIApp` (SwiftUI) and `CrossApp` (SwiftCrossUI `DefaultBackend` → AppKit). `swift build` completed in 55 s. **`swift test` (Swift Testing) passed 2/2 headless.** One test checks the Rust FFI is linked. The other checks that `receive()` mutates state through Rust and fires `withObservationTracking`'s `onChange`. `nm` shows the `_dash_probe_*` symbols in both executables. The SwiftUI binary is 120 KB; the SwiftCrossUI one is 14 MB. |
| Running the GUIs | Not launched. Per instructions, no GUI windows were opened; building was enough. |
| Sandbox notes (dev-env, not product) | Inside this Claude sandbox, SwiftPM fails with `sandbox-exec: sandbox_apply: Operation not permitted` because its manifest sandbox nests inside Seatbelt. `--disable-sandbox` gets past that, but git still can't write `.git/config` or `.git/hooks` in checkouts. Resolve and build only worked **outside** the sandbox. Network to github.com worked. |

**Why the shared ViewModel works on both UIs.** SwiftCrossUI's `ModelObserver` calls `ObservationPolyfillCore.withObservationTracking`. On macOS 14+ that function **delegates to the real `Observation.withObservationTracking`**:
`if #available(macOS 14, …) { return Observation.withObservationTracking(apply, onChange: onChange()) }`.
On Linux and Windows the polyfill *is* the stdlib Observation. SwiftCrossUI's `State` has an explicit `init where Value: Observation.Observable & AnyObject`. So a plain stdlib `@Observable` class is first-class in SwiftUI and SwiftCrossUI on every OS.

To reproduce: `cd scratch/shared-vm-probe && swift build && swift test` (run outside the Claude sandbox). The Rust lib is rebuilt with `cd rust/dash_probe && cargo build --release --target aarch64-apple-darwin`. I deleted its `target/` afterwards; the built `.a` stays inside the artifact bundle.

---

## 3. Toolchain state off Apple platforms (2026)

| Topic | State | Implication |
|---|---|---|
| Swift releases | 6.3 (2026-03-24) shipped the first official Android SDK. 6.3.3 (2026-06-29/30) has toolchains for Linux and Windows x86_64 + arm64. 6.4.0 tagged 2026-09-15. Xcode 26.6 ships 6.3.3. | Pin one toolchain version across Xcode, Linux and Windows CI. That's 6.3.3 today; move to 6.4 once Xcode ships it. |
| Windows Workgroup | Announced 2026-01-26. Meets biweekly; the September 2026 agenda was posted. Active threads on COM interop. | Windows is a staffed, official platform. UI is still outside its scope. |
| Install tooling | `swiftly` 1.2.0 (2026-09-22) covers **Linux and macOS only**. Windows uses `winget install Swift.Toolchain` plus VS 2022 (Win11 SDK 22621, VC x64 + ARM64 tools) and Developer Mode. The Browser Company also publishes daily Windows toolchain builds (`thebrowsercompany/swift-build`, latest 20261004.3). | Windows CI needs a hand-maintained toolchain image. |
| Foundation | From Swift 6 on, Linux and Windows use **swift-foundation** (FoundationEssentials/Internationalization) under corelibs-foundation. You get URLSession async, AttributedString and Predicate. On Windows, zlib, curl and libxml have to be built and shipped. | Data, JSON, URL, Date and Locale work the same everywhere. Don't use NSKeyedArchiver-style or Objective-C-only APIs. |
| Observation | Part of the open-source toolchain on every platform. SE-0506 Advanced Observation Tracking is in progress. The `Observations` async sequence (6.2) works off-Apple, **but on Apple it needs OS 26**. | Use `@Observable` and `withObservationTracking`. Gate `Observations` behind `#available` or avoid it, since the macOS app will likely target 14 or 15. |
| Combine | Apple-only. OpenCombine exists and its CI covers Windows, but it doesn't integrate with SwiftUI and adds a dependency. | Shared code is Observation + async/await only. |
| Concurrency | Available on Linux and Windows (`libswift_Concurrency`). SwiftCrossUI is `@MainActor` and hops to the main thread through `backend.runInMainThread`. | `@MainActor` ViewModels with Rust callbacks marshalled into `Task { @MainActor in … }`. This pattern works the same on all UIs. |
| Static stdlib on Windows | **Not finished.** The driver part (`swiftrtT.obj`) landed in 6.4. Static Foundation and dispatch are still open (swiftlang/swift#83446). | Ship Swift runtime DLLs next to the `.exe` and package them in the MSI/MSIX. |
| Linking Rust static libs | **SE-0482 "Binary Static Library Dependencies" was implemented in Swift 6.2.** Artifact bundles with `"type": "staticLibrary"` give `.a` on Linux and Apple, `.lib` on Windows, plus a required modulemap. One known Windows SwiftPM bug: a zipped bundle can't end in `.artifactbundle` (#10077). For Windows MSVC, the Rust staticlib needs explicit system libs: `ws2_32, bcrypt, ntdll, userenv, kernel32, advapi32, dbghelp` (take the list from `--print native-static-libs` on the real crate). Rust uses `/MD` (msvcrt), which matches Swift's default. Avoid `/WHOLEARCHIVE` (LNK2005 with Rust 1.79+). | This replaces the XCFramework on Linux and Windows. The platform repo's FFI build has to produce per-triple bundles: `x86_64/aarch64-unknown-linux-gnu`, `x86_64/aarch64-unknown-windows-msvc`. |

---

## 4. Option-by-option evaluation

### 4.1 Option A — Swift everywhere with a SwiftUI-like cross-platform framework

#### SwiftCrossUI (`moreSwift/swift-cross-ui`, formerly `stackotter/swift-cross-ui`)
- **Status:** v0.10.0 on 2026-09-30. Releases are roughly monthly through 2026: 0.2.1 Mar 4 → 0.3 Mar 20 → 0.4 Apr 10 → 0.5 Apr 29 → 0.6 May 14 → 0.7 Jun 3 → 0.8 Jul 2 → 0.9 Aug 19 → 0.10 Sep 30. 1,758★, 92 forks, 218 open issues, 23 contributors. **Bus factor is about one:** stackotter has 685 commits; the next contributor has 73. It is pre-1.0, so expect breaking changes between minors. It has an Open Collective and an LLM policy.
- **Backends:** AppKit (README: "supports all features"), UIKit, WinUI ("most features"), Gtk 4 ("most features", runs on Linux/macOS/Windows), Gtk3 (legacy, "quite buggy on macOS"), Android. **Qt, Curses and LVGL backends are commented out in Package.swift.** DefaultBackend picks per OS; `SCUI_DEFAULT_BACKEND` overrides it.
- **Views available (0.10):** Button, Toggle/Checkbox/Switch, TextField, SecureField, TextEditor, Picker, DatePicker, ColorPicker, Slider, ProgressView, List, Table, ScrollView, NavigationStack/SplitView/Link, Menu, sheets, alerts, gradients, shapes, GeometryReader, WebView, ContentUnavailableView, FocusState (AppKit only), AppStorage, hot reload. That covers a wallet's screens: balance, tx list/table, send form, receive (QR as Image), settings, sheets.
- **Gaps that matter for a wallet:**
  - **Accessibility:** the core has *no* `accessibilityLabel`/`accessibilityHint` modifiers (one mention in all of `Sources/SwiftCrossUI`). Native widgets inherit the OS's accessibility on AppKit and GTK.
  - **Issue #787 (open, filed 2026-09-25):** on WinUIBackend, *UI Automation clients crash the app* (access violation in `Microsoft.UI.Xaml.dll`) whenever the window contains List, NavigationSplitView, ScrollView or TextField. Windows that don't crash expose only one UIA element, so content is invisible to Narrator. The likely root cause is the upstream WinUI bug microsoft-ui-xaml#11028, which hits controls created in code, and SwiftCrossUI creates every control in code. **This blocks a11y on Windows and blocks UIA-based UI tests.**
  - The Windows bindings (`moreSwift/swift-winui` 0.2.2, 0★) are a fork of The Browser Company's archived bindings. They target **Windows App SDK 1.5-preview1** and need the matching Windows App Runtime installed.
  - `DummyBackend` exists (CI runs layout tests on it) but **is not exported as a product**, so apps can't use it for headless view tests.
- **Packaging:** Swift Bundler (moreSwift) produces .app, .msi (WiX, auto-installed) and .rpm, with Linux and Windows hot reload. The last *tagged* release is v2.0.7 (about 2024). v3.0.0 is "being prepared", and the Windows/MSI work since June 2026 is on `main` only. No AppImage or Flatpak support found. Flathub's `org.freedesktop.Sdk.Extension.swift6` is maintained (pushed 2026-09-17) and works for a manual Flatpak manifest.
- **Real apps:** only Video Village's Linux plugin manager is documented, and the source is the author's own site. **No shipped Windows app found.**

#### Other Swift UI frameworks
| Project | Status | Verdict |
|---|---|---|
| Adwaita for Swift (AparokshaUI) | GitHub repo archived 2024-10-17; moved to git.aparoksha.dev. Commits in Feb 2026, no tagged release since 2024. GNOME/libadwaita only. | Linux-only and sleepy. SwiftCrossUI's GtkBackend covers Linux better. |
| swift-adwaita (makoni) | 1.0.0 around 2026-03-31. Imperative GTK4/libadwaita wrapper. | Linux-only and not SwiftUI-shaped. |
| The Browser Company swift-winrt / swift-winui | swift-winrt is active (pushed 2026-10-02; last release v0.1.396, 2026-03-11). **swift-winui is archived** (2025-10). Arc for Windows (Swift + WinUI 3) is in maintenance after the Atlassian acquisition (closed Oct 2025). TBC never built a declarative UI layer. Dia for Windows is "this fall" and its stack is unconfirmed. | Raw WinRT is a toolkit, not a UI framework. SwiftCrossUI's WinUIBackend sits on top of it. |
| Tokamak | **Archived**; last release 0.11.1 (2022-11). | Dead. |
| SwiftGodot | Active (v0.79.0, 2026-08-01) but it's a game engine. | Wrong tool for a wallet: no native widgets, a11y or forms. |
| WinAppUI (SwiftUI-like over WinUI) | 0★, last push 2023-12. | Dead. |
| Skip (skip.tools) | Fully open source since 2026-01, but iOS → Android only. | Not desktop. |

**Industry signal on Windows-native UI.** Raycast 2.0 (beta May 2026) uses Swift for its macOS host. Its engineering blog says it **rejected WinUI 3** ("far from great… fairly young and not widely tested") and chose a C# host with a shared React/WebView UI. This doesn't make WinUI unusable, but it is the most recent public data point from a Swift-native shop.

### 4.2 Option B — SwiftUI on macOS, shared Swift core, SwiftCrossUI on Linux and Windows

**Feasibility of sharing ViewModels:** shown working (§2). Rules for the shared `WalletCore` package:

1. **Imports allowed:** `Foundation` (or `FoundationEssentials`), `Observation`, `SwiftDashSDKCore`, and the Rust C module. **Never** SwiftUI, Combine, AppKit, UIKit, SwiftData or SwiftCrossUI.
2. **State:** `@MainActor @Observable final class`. Don't use `ObservableObject`/`@Published`. Combine's doesn't exist off-Apple, and SwiftCrossUI ships its *own* `ObservableObject`/`Published` that clash by name with Combine's. View-local state stays in the views (`@State`), and both frameworks have a `Bindable`.
3. **No UI value types in ViewModels:** no `Color`, `Image`, `LocalizedStringKey` or `Font`. Expose `String`, `Decimal`/`Int64` duffs, domain enums and `Data` (QR payloads). Each UI layer maps these to its own types.
4. **Navigation as data:** route enums in the ViewModel. SwiftUI and SwiftCrossUI both have `NavigationStack(path:)` and `NavigationPath`.
5. **Async and FFI callbacks:** Rust → C callback → `Task { @MainActor in vm.apply(event) }`. Long work runs in non-isolated services or actors.
6. **Platform services behind protocols** that each app injects: `SecretStore` (Keychain/LocalAuthentication on macOS; Windows Credential Manager or DPAPI; libsecret/Secret Service), `WalletPersistence` (SwiftData on Apple if desired, `platform-wallet-storage` SQLite elsewhere, or that everywhere), `Clipboard`, `URLOpener`, `Notifications`.
7. **Deployment floor:** macOS 14 (for `@Observable` in SwiftUI). Avoid `Observations` (macOS 26) in shared code.

**Code reuse estimate:** ViewModels, services, formatting, validation and SDK glue are shared (100% on desktop, and partly shareable with iOS once the iOS VMs move to `@Observable`). View code is written twice: SwiftUI (macOS, possibly shared with iOS SwiftUI screens) and SwiftCrossUI (Linux and Windows, one tree). The APIs are deliberately close (`VStack`, `Text`, `Button`, `List`, `NavigationStack`, `.sheet`, `.alert`), so porting a view is mostly mechanical. Source-level sharing between the two isn't practical: different modules, missing modifiers, and different style systems.

**Why B over A:** the macOS app is the flagship and the one users compare with the iOS app. SwiftUI on macOS gets full accessibility, XCUITest, Mac idioms (menus, toolbar, Settings scene, `.searchable`, focus), and future SwiftUI improvements for free. The extra cost over A is one more view tree, and it removes SwiftCrossUI as a risk on the most important desktop platform.

### 4.3 Option C — Alternatives on the Rust core or other runtimes

| Option | Current status (2026) | iOS-stack fidelity | Native feel | a11y | Testability | Packaging | Rust core link |
|---|---|---|---|---|---|---|---|
| **egui/eframe** (dash-evo-tool) | 0.36.2 (Sep 2026); 0.34 (Mar 2026) made AccessKit always-on; 0.35 (Jun 2026) added an inspection protocol and egui_mcp. 30.8k★. | Low: immediate-mode Rust, no MVVM. | Low: custom-drawn, looks "egui". | AccessKit always-on (UIA, NSAccessibility, AT-SPI). | **Strong:** `egui_kittest` headless plus snapshots, already used by DET. | DET already ships dmg/flatpak/zip on all three. | **Native:** depends on `platform-wallet` crates directly, no FFI. |
| **Slint** | 1.18.1 (2026-09-21). 24k★. **Since 1.16 Fluent is the default style everywhere and native-looking styles are being deprecated** (2026-03-31). | Low–medium: declarative `.slint` DSL plus Rust. | Medium: polished but uniform Fluent look. | AccessKit since 1.1. | Testing backend and software renderer run headless. | cargo-packager/bundle; royalty-free licence for desktop apps. | Native Rust. |
| **Iced** | 0.14 (2025-12-07): headless testing, time-travel debug. Last push 2026-10-04. | Low: Elm architecture. | Low–medium. | **None in mainline** (AccessKit WIP; System76's COSMIC fork has some). | Headless test crate. | Manual. | Native Rust. |
| **Dioxus** | 0.7.10 (2026-07-30); 0.8 alpha. Webview (wry) or Blitz native renderer. | Low: React-like. | Web-ish. | Depends on webview/Blitz. | Web tooling. | `dx bundle`. | Native Rust. |
| **Tauri** | 2.12.0 (2026-09-26), "biggest 2.x update"; dropped Windows 7. 111k★. | Low: web UI + Rust. | Web (system WebView). | Good (browser a11y). | WebDriver (tauri-driver) on Win/Linux; not macOS WKWebView. | **Best in class:** msi/nsis/dmg/AppImage/deb/rpm, updater, signing. | Native Rust. |
| **Compose Multiplatform desktop** | 1.12.1 (2026-09-22), 1.13 alpha. Desktop is "production ready". JVM only, no Kotlin/Native desktop. | Medium: declarative + ViewModel, mirrors the Android app, not iOS. | Medium (Skia). | macOS full; Windows via Java Access Bridge (opt-in module); **Linux not supported.** | `runComposeUiTest` headless on JVM. | jpackage dmg/msi/deb (JDK 17+), bundles a JRE (+60–80 MB), notarization in the Gradle plugin. | kotlin-sdk is **Android-only**, so it would need a KMP refactor plus desktop JNI builds (`rs-unified-sdk-jni` is a cdylib; needs mac/win/linux builds). |
| **Flutter** | Stable 3.44.x. Multi-window is still main-channel/experimental (Canonical-led). | Low: Dart. | Medium (custom-drawn, Material/Cupertino look). | Semantics tree mapped to each platform. | Widget tests headless; integration_test on desktop. | flutter build per OS; msix package. | `dart:ffi` or flutter_rust_bridge on the C ABI. |
| **Qt 6 / QML** | 6.11.2 (2026-08-18), 6.8 LTS to 2029. Qt Bridges: Rust bridge public beta 2026-07-01; **Swift bridge not yet** (later phase). cxx-qt 0.10.0 (2026-08-24). Qt 6.11.2 is installed here. | Low. | High on Windows/Linux; OK on macOS. | Mature on all three. | Squish/QtTest. | macdeployqt/windeployqt/linuxdeploy; LGPL obligations. | cxx-qt or qtbridge (Rust); C ABI from C++. |

### 4.4 Cross-cutting matrix for the Swift options

| Concern | macOS (SwiftUI) | Linux (SwiftCrossUI GtkBackend) | Windows (SwiftCrossUI WinUIBackend) |
|---|---|---|---|
| Native feel | ★★★★★ | ★★★★ (real GTK4 widgets; no libadwaita styling) | ★★★ (real WinUI 3 controls, limited styling) |
| Maturity | ★★★★★ | ★★★ | ★★ (preview WinAppSDK bindings, fork with 0★, no shipped apps) |
| a11y | ★★★★★ | ★★★ (GTK4 AT-SPI on native widgets; no a11y modifiers) | ★ (**#787: UIA crash + empty tree**) |
| Unit tests (ViewModels) | `swift test` | `swift test` in a Linux container (headless) | `swift test` on a windows-latest runner |
| UI automation | XCUITest (needs an Xcode app target, not bare SwiftPM) | AT-SPI (dogtail/pyatspi) under Xvfb/Weston. Unproven with SwiftCrossUI. | UIA (WinAppDriver/FlaUI): **blocked by #787** |
| Packaging | Xcode archive → codesign (hardened runtime) → `notarytool` → dmg | Flatpak (GNOME runtime + `swift6` SDK extension), or Swift Bundler rpm. AppImage would be manual. | Swift Bundler MSI (WiX, unreleased v3), or the MSIX/winapp CLI template (iankoex). Must ship Swift runtime DLLs + Windows App Runtime 1.5 (bootstrapper). |
| Rust link | XCFramework (as iOS today) or an SE-0482 bundle | SE-0482 `.a` bundle (`*-unknown-linux-gnu`) | SE-0482 `.lib` bundle (`*-unknown-windows-msvc`) + system libs |

### 4.5 Rejected

- **Tokamak:** archived since 2022.
- **Adwaita-for-Swift:** Linux-only, no releases since 2024.
- **SwiftGodot:** a game engine.
- **WinAppUI:** dead.
- **Hand-written swift-winrt UI:** an imperative XAML-in-code toolkit, i.e. writing a Windows app by hand, and it hits the same WinUI automation-peer bug.
- **Qt via Swift:** no Swift bridge yet.

---

## 5. Scoring (weights reflect the brief: "built like iOS", native feel, shippable)

| Criterion (weight) | B: SwiftUI + SCUI | A: SCUI everywhere | C1: egui/Slint | C2: Compose | C3: Tauri |
|---|---|---|---|---|---|
| Fidelity to the iOS stack (×3) | 5 | 4 | 1 | 2 | 1 |
| Native feel (×2) | 5 / 4 / 3 | 3 / 4 / 3 | 2 | 3 | 3 |
| Maturity / risk (×3) | 3 (Win 2) | 2 | **5** (already shipping for Dash) | 4 | 5 |
| a11y (×2) | 5 / 3 / **1** | 3 / 3 / **1** | 4 | 3 (Linux 0) | 4 |
| Testability (×1) | 4 | 3 | 5 | 4 | 3 |
| Packaging (×1) | 3 | 3 | 4 | 4 | 5 |
| Rust link effort (×1) | 3 (SE-0482 + SDK split) | 3 | 5 (native crates) | 2 (KMP + JNI port) | 5 |
| **Weighted total (max 65)** | **48** | 38 | 44 | 40 | 45 |

Where a cell has per-OS values (macOS / Linux / Windows), they are averaged before weighting. B wins on fidelity and macOS quality. Its Windows a11y score is the single biggest drag, which is why the Windows gate below matters.

Tauri edges out egui/Slint on raw score (45 vs 44), but §0 still ranks C1 above C3. Dash already ships C1 (dash-evo-tool) on the same Rust crates, with working CI and packaging. Tauri would add a web front-end stack, making TypeScript a third language, with no fidelity gain.

---

## 6. Recommended architecture (Option B)

```
dashwallet-desktop/
  Packages/
    WalletCore/            # @Observable ViewModels, services, formatters — Foundation + Observation only
    PlatformServices/      # protocols: SecretStore, WalletPersistence, Clipboard, …
    PlatformServicesApple/ # Keychain/LocalAuthentication (+ optional SwiftData adapter)
    PlatformServicesWin/   # Credential Manager / DPAPI
    PlatformServicesLinux/ # libsecret (Secret Service)
  Apps/
    macOS/                 # Xcode app target, SwiftUI views, XCUITest
    CrossPlatform/         # SwiftCrossUI views (GtkBackend on Linux, WinUIBackend on Windows)
  (depends on) SwiftDashSDKCore  ← split out of platform/packages/swift-sdk, no SwiftData
               DashSDKFFI.artifactbundle  ← per-triple static libs (SE-0482)
```

Upstream work this implies in `dashpay/platform`:
1. Split SwiftDashSDK into a portable core and a SwiftData persistence adapter. Port `PlatformWalletManager` from Combine to `@Observable`.
2. Produce an SE-0482 artifact bundle for the unified FFI covering linux-gnu and windows-msvc (x86_64 + aarch64), alongside the XCFramework.
3. Optionally expose `platform-wallet-storage` (SQLite + keyring) through FFI, so desktop persistence and secrets come from shared Rust code that dash-evo-tool already exercises, rather than three Swift adapters.

---

## 7. Gates and spikes before committing

| # | Spike (time-box) | Pass criteria | If it fails |
|---|---|---|---|
| G1 | **Linux:** build `shared-vm-probe` with GtkBackend in an Ubuntu 24.04 container (Swift 6.3.3 + libgtk-4-dev) plus the linux-gnu Rust bundle. Run `swift test` headless. Launch under Xvfb and dump the AT-SPI tree. (2 days) | Builds, tests pass, labelled widgets appear in AT-SPI. | Linux falls back to C1 (egui/Slint) for Linux only. |
| G2 | **Windows:** same probe on windows-latest with WinUIBackend and the msvc `.lib` bundle. Run a Narrator/FlaUI tree walk. Package an MSI (Swift Bundler main) or MSIX. (3–4 days) | No UIA crash. Text, Button and TextField are exposed. Installer runs on a clean VM with runtime DLLs + Windows App Runtime. | Option 1: ship Windows on SwiftCrossUI **GtkBackend** (functional, non-native, weak a11y). Option 2 (preferred for a wallet): Windows via **C1 (egui like DET, or Slint)** until #787 / microsoft-ui-xaml#11028 is fixed. The ViewModel layer is still not shared in that case, so decide early. |
| G3 | **SDK split:** prototype `SwiftDashSDKCore` without SwiftData and compile it on Linux. (3 days) | KeyWallet + PlatformWallet + FFI compile on Linux with a stub persistence. | Desktop persistence moves entirely into Rust (`platform-wallet-storage` via FFI). |
| G4 | **SwiftCrossUI governance:** pin 0.10.x, fork into `dashpay/`, budget upstream PRs for a11y modifiers (`accessibilityLabel` etc.) and for exporting DummyBackend. | Maintainer accepts the direction. | Keep the fork; the bus-factor risk is accepted knowingly. |

---

## 8. Risks and open questions

- **SwiftCrossUI bus factor and churn:** about one core maintainer, pre-1.0, monthly breaking minors. Mitigations: pin, fork, keep views thin, and hold all logic in `WalletCore` so the UI can be swapped.
- **WinUI 3 itself:** Windows App SDK 1.5-preview1 bindings, an archived upstream, and Raycast's public rejection of WinUI 3. Arc, the main proof point, is in maintenance.
- **Windows Swift runtime distribution:** static stdlib isn't done, so runtime DLLs ship with the app. Installer size and DLL-conflict testing are needed. SwiftCrossUI 0.7 cut about 200 MB from Windows builds by statically linking swift-winui, which shows how heavy this layer is.
- **UI automation on the Swift path:** XCUITest needs an Xcode app target (not a bare SwiftPM executable). Cross-platform UI tests are unproven (AT-SPI, UIA blocked).
- **Duplicated view work in B:** two view trees. This is acceptable only if the shared ViewModels keep the view code thin.
- **dash-evo-tool overlap:** DET is already a cross-platform Dash desktop app on the same Rust crates. Product needs to settle whether the new wallet replaces it, complements it, or reuses its Windows/Linux packaging and CI.
- **Unverified here:** GTK4 accessibility on Windows and macOS (assume none or weak), Swift Bundler AppImage support, and arm64 Windows for swift-winui (the README lists arm64 runtime installers; older TBC docs said x64 only).

---

## Sources

Swift toolchain and language
- [Swift 6.3 Released (swift.org)](https://www.swift.org/blog/swift-6.3-released/)
- [Swift 6.2 Released (swift.org)](https://www.swift.org/blog/swift-6.2-released/)
- [Announcing the Windows Workgroup (swift.org)](https://www.swift.org/blog/announcing-windows-workgroup/) · [Forums announcement](https://forums.swift.org/t/announcing-the-windows-workgroup/84374) · [Biweekly agenda](https://forums.swift.org/t/biweekly-windows-workgroup-agenda-notes/89334)
- [Install Swift — Windows](https://www.swift.org/install/windows/) · [WinGet](https://www.swift.org/install/windows/winget/)
- [swiftlang/swiftly](https://github.com/swiftlang/swiftly) (1.2.0 release notes via GitHub API)
- [swift-foundation now available (Swift Forums)](https://forums.swift.org/t/swift-foundation-now-available/73530) · [swift-corelibs-foundation](https://github.com/swiftlang/swift-corelibs-foundation)
- [Michael Tsai — Swift 6.2: Observations](https://mjtsai.com/blog/2025/10/31/swift-6-2-observations/)
- [Perception vs Observation on Linux/Windows](https://github.com/pointfreeco/swift-perception/discussions/71) · [OpenCombine](https://github.com/MaxDesiatov/OpenCombine)
- [Towards -static-stdlib support on Windows](https://forums.swift.org/t/towards-static-stdlib-support-on-windows/77728) · [swiftlang/swift#83446](https://github.com/swiftlang/swift/issues/83446) · [swift-driver 6.4.0 release](https://github.com/swiftlang/swift-driver/releases/tag/swift-6.4.0-RELEASE)
- [SE-0482 Binary Static Library Dependencies](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0482-swiftpm-static-library-binary-target-non-apple-platforms.md) · [SwiftPM #10077](https://github.com/swiftlang/swift-package-manager/issues/10077)
- [Rust: native libraries required for staticlibs](https://users.rust-lang.org/t/native-libraries-required-to-link-to-rust-static-libraries/113470) · [rust-lang/rust#129020 (LNK2005)](https://github.com/rust-lang/rust/issues/129020)

Swift UI frameworks
- [moreSwift/swift-cross-ui](https://github.com/moreSwift/swift-cross-ui) · [Releases](https://github.com/moreSwift/swift-cross-ui/releases) · [Issue #787 WinUI UIA crash](https://github.com/moreSwift/swift-cross-ui/issues/787) · [Issue #451](https://github.com/moreSwift/swift-cross-ui/issues/451) · [Issue #753](https://github.com/moreSwift/swift-cross-ui/issues/753) · [Swift Package Index](https://swiftpackageindex.com/moreSwift/swift-cross-ui)
- [moreSwift/swift-winui](https://github.com/moreSwift/swift-winui) · [thebrowsercompany/swift-winui (archived)](https://github.com/thebrowsercompany/swift-winui) · [thebrowsercompany/swift-winrt](https://github.com/thebrowsercompany/swift-winrt) · [thebrowsercompany/swift-build](https://github.com/thebrowsercompany/swift-build) · [Swift, meet WinRT](https://speakinginswift.substack.com/p/swift-meet-winrt?open=false) · [iankoex/swift-WinUI3App](https://github.com/iankoex/swift-WinUI3App)
- [Swift Bundler](http://swiftbundler.dev/) · [moreSwift/swift-bundler commits](https://github.com/moreSwift/swift-bundler/commits/main) · [stackotter open-source](https://stackotter.dev/open-source)
- [adwaita-swift (Gitea)](https://git.aparoksha.dev/aparoksha/adwaita-swift) · [AparokshaUI GitHub (archived)](https://github.com/AparokshaUI) · [swift-adwaita](https://swiftpackageindex.com/makoni/swift-adwaita)
- [Skip open-sourced (InfoQ)](https://www.infoq.com/news/2026/01/swift-skip-open-sourced/)
- [Flathub swift6 SDK extension](https://github.com/flathub/org.freedesktop.Sdk.Extension.swift6)
- Tokamak, SwiftGodot, WinAppUI: GitHub API metadata (`TokamakUI/Tokamak` archived; `migueldeicaza/SwiftGodot` v0.79.0)

Industry data points
- [Arc (web browser) — Wikipedia](https://en.wikipedia.org/wiki/Arc_(web_browser)) · [Dia (web browser) — Wikipedia](https://en.wikipedia.org/wiki/Dia_(web_browser)) · [Dia for Windows](https://www.diabrowser.com/windows) · [Fatbobman #86 (Arc/Dia/TCA/SwiftUI)](https://fatbobman.com/en/weekly/issue-086/)
- [Raycast — A Technical Deep Dive Into the New Raycast](https://www.raycast.com/blog/a-technical-deep-dive-into-the-new-raycast)

Alternatives
- [egui releases](https://github.com/emilk/egui/releases) · [egui 0.34.0](https://newreleases.io/project/github/emilk/egui/release/0.34.0) · [docs.rs egui](https://docs.rs/crate/egui/latest)
- [Slint blog](https://slint.dev/blog/) · [Slint 1.15](https://slint.dev/blog/slint-1.15-released) · [Deprecating native-looking styles](https://slint.dev/blog/default-native-style-change) · [Slint releases](https://github.com/slint-ui/slint/releases)
- [Iced 0.14 (Phoronix)](https://www.phoronix.com/news/Iced-0.14-Rust-GUI-LIbrary) · [SE Radio 713](https://se-radio.net/2026/03/se-radio-713-hector-ramon-jimenez-on-building-a-gui-library-in-rust/) · [iced releases](https://github.com/iced-rs/iced/releases)
- [Tauri 2.12](https://v2.tauri.app/blog/tauri-2.12/) · [Tauri releases](https://tauri.app/release/core/)
- [Dioxus 0.7](https://dioxuslabs.com/blog/release-070/) · [Dioxus 0.8 roadmap](https://github.com/DioxusLabs/dioxus/discussions/5024)
- [Compose Multiplatform 1.10.0 (JetBrains)](https://blog.jetbrains.com/kotlin/2026/01/compose-multiplatform-1-10-0/) · [CMP releases](https://github.com/JetBrains/compose-multiplatform/releases) · [Desktop accessibility](https://kotlinlang.org/docs/multiplatform/compose-desktop-accessibility.html) · [Kotlin/Native desktop CMP-1923](https://youtrack.jetbrains.com/projects/CMP/issues/CMP-1923/Kotlin-Native-Support-for-Desktop)
- [What's new in Flutter 3.41](https://blog.flutter.dev/whats-new-in-flutter-3-41-302ec140e632) · [Flutter 3.38](https://flutter.dev/blog/whats-new-in-flutter-3-38) · [Flutter multi-window (Aug 2026)](https://startdebugging.net/2026/08/how-to-enable-multi-window-support-in-a-flutter-desktop-app/)
- [Qt 6.11.2 Released](https://www.qt.io/blog/qt-6.11.2-released) · [Qt Bridges Rust beta](https://www.qt.io/blog/qt-bridges-public-beta-for-rust) · [Qt Bridges (Phoronix)](https://www.phoronix.com/news/Qt-Bridges-New-Languages) · [KDAB/cxx-qt](https://github.com/KDAB/cxx-qt)

Our repos (GitHub API / sparse clone, 2026-10-05)
- `dashpay/platform@bc32136` — `packages/swift-sdk`, `packages/kotlin-sdk`, `packages/rs-unified-sdk-ffi`, `packages/rs-unified-sdk-jni`, `packages/rs-platform-wallet-ffi`, `packages/rs-platform-wallet-storage`
- `dashpay/dashwallet-ios@develop` (code search counts)
- `dashpay/dash-evo-tool@v1.0-dev` (Cargo.toml, releases)

Hands-on artifacts
- `/Users/pasta/workspace/dashwallet-desktop/scratch/scui-hello/` — SwiftCrossUI 0.10.0 + AppKitBackend hello world (`build.log`, `Package.resolved`; `.build` removed)
- `/Users/pasta/workspace/dashwallet-desktop/scratch/shared-vm-probe/` — Option-B probe: shared `@Observable` VM + Rust SE-0482 bundle + SwiftUI and SwiftCrossUI front ends + Swift Testing (`build.log`, `test.log`)
- `/Users/pasta/workspace/dashwallet-desktop/scratch/platform-sparse/` — sparse clone used for the SwiftDashSDK portability audit
