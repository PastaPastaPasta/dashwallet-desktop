# Vendored packages and their patches

## `swift-cross-ui` (SwiftCrossUI 0.10.0)

- Upstream: <https://github.com/stackotter/swift-cross-ui>, tag `0.10.0`, commit
  `0f3ec3958b79cdc39a1a3e604516b71ab33a9043`. MIT licence, kept in `swift-cross-ui/LICENSE` (and
  `Sources/Gtk/LICENSE.md` for the generated GTK bindings).
- Why vendored: the app needs accessibility patches (ADR 0002) that upstream 0.10.0 does not have.
  A local package keeps every build (macOS, the Linux Docker scripts) off a network fetch of a
  fork, and nothing has to be pushed anywhere. `Package.swift` depends on it with
  `.package(path: "Vendor/swift-cross-ui")`; its own remote dependencies (swift-syntax,
  swift-log, swift-mutex, swift-image-formats, swift-observation-polyfill, swift-macro-toolkit,
  swift-winui) still resolve through `Package.resolved` as before.
- What is kept: the source of the targets the app builds, unchanged except for the patches
  below: `SwiftCrossUI`, `SwiftCrossUIMacrosPlugin`, `SwiftCrossUIMetadataSupport`,
  `DefaultBackend`, `AppKitBackend` (macOS development builds), `GtkBackend`, `Gtk`, `CGtk`,
  `GtkCHelpers` (Linux), `WinUIBackend`, `WinUIInterop` (Windows, design gate G3).
- What is left out: the Android, UIKit, Gtk3, Curses, Qt, LVGL and Dummy backends, the porting
  kit, the GTK code generator, examples, benchmarks, tests, the DocC catalog and the `.gyb`
  templates (their generated `.swift` files are kept). With them go the AndroidKit, swift-java,
  XMLCoder, swift-docc-plugin, swift-benchmark and swift-collections dependencies.
  `Package.swift` is rewritten for that set; the upstream compile-time options
  (`SCUI_DEFAULT_BACKEND`, `SCUI_LIBRARY_TYPE`, hot reloading, benchmark visualisation) are
  dropped.
- Updating: copy the same targets from the new tag, re-apply the patches below (each patched
  file has a `dashwallet-desktop patch Pn` comment), and run the macOS build plus both Linux
  Docker scripts (`scripts/linux-docker-test.sh`, `scripts/crossui-linux-demo.sh`).

### P1 — accessibility modifiers (ADR 0002 gap A1; fixes A2–A3 and drop-down names)

Rationale: upstream 0.10.0 has no way to give a control an accessible name. Toggles, text fields
and drop-downs whose visible label is a separate `Text` had no name or, for GTK drop-downs, the
selected option as their name ("Never", "tDASH"), so Orca could not announce them and the AT-SPI
checks could not find them. The app's earlier workaround reached the native widget through
`inspect` hooks; it worked for entries and switches but never reached the `GtkDropDown` of a
`Picker`.

Change:
- `View.accessibilityLabel(_:)` and `View.accessibilityHint(_:)`
  (`Sources/SwiftCrossUI/Views/Modifiers/AccessibilityModifiers.swift`), carried as the
  environment values `accessibilityLabel` / `accessibilityHint` (`EnvironmentValues.swift`), the
  same way as `isEnabled`, so the name lands on the control widget rather than a container.
- `BackendFeatures.Accessibility` (`Backend/BackendFeatures/Accessibility.swift`), an optional
  backend feature with `setAccessibility(of:label:hint:)`, applied by
  `BackendHelpers.applyAccessibilityProperties` from `ViewGraphNode.commit()` for every focusable
  view (the controls) and from `_BuiltinPickerImplementation.commit` for the picker widget
  itself. A backend without the feature logs a warning once when a label is set.
- GtkBackend (`Features/Accessibility.swift`): `GTK_ACCESSIBLE_PROPERTY_LABEL` (AT-SPI name) and
  `GTK_ACCESSIBLE_PROPERTY_DESCRIPTION`. The value set last is kept as object data, so an
  unchanged value is not set again on every commit, and `nil` resets only what the patch set.
  GTK 4.14 computes a name from the labelled-by relation before the label property, and
  GtkDropDown's template points that relation at its selected item (`button_item`); setting a
  label therefore also resets the widget's labelled-by relation. That is why the label alone
  (and the app's old workaround) left drop-downs named after their selection. Removing the label
  later does not restore the template's relation: the drop-down is then unnamed.
- AppKitBackend (`Features/Accessibility.swift`): `setAccessibilityLabel` /
  `setAccessibilityHelp`; `NSCustomButton.accessibilityLabel()` returns the explicit label first.
- WinUIBackend: not implemented (no Windows build yet); it logs the warning.

Not in this patch: `accessibilityHidden` (no control needs it yet), list-row names (P4, still
done by `DashUICross.accessibleRowNames` through `inspect`), and P2/P3 as separate defaults —
`DashToggle`, `DashTextField`, `DashSecureField` and `DashPicker` pass their caption explicitly.
