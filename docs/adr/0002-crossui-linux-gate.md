# ADR 0002 — Gate G2: SwiftCrossUI (GtkBackend) on Linux

- Status: **accepted** (gate result recorded 2026-10-05, M0, WS-12)
- Gate definition: DESIGN-opus.md §6 G2. Probe: List, TextField, Button, Toggle, sidebar navigation.
  Pass criterion: "builds; labelled widgets visible in AT-SPI". Fallback if GTK4 fundamentally fails:
  a Linux-only egui UI over `dw-engine`.
- Evidence: `probes/crossui-linux/RESULTS.md` (numbers, full AT-SPI dumps, screenshots) and
  `probes/crossui-linux/evidence/`.
- Harness: `ci/linux/Dockerfile.swift-gtk` (image `dwd-linux-swift-gtk`) +
  `probes/crossui-linux/scripts/run-in-container.sh`.

## Verdict: PASS (conditional)

Linux stays on SwiftCrossUI 0.10.0 + GtkBackend. The egui fallback is **not** triggered.

Why PASS:
- The probe (shared `@MainActor @Observable` view model with Foundation + Observation only, plus a
  SwiftCrossUI wallet window) builds with Swift 6.3.3 on Ubuntu 24.04 / GTK 4.14.5. Its 13 headless
  Swift Testing tests pass on **aarch64** and on **x86_64** (Rosetta).
- Under Xvfb + D-Bus + the AT-SPI bus, all 16 hard checks pass on both architectures (7 soft checks record gaps) and stayed
  stable over 4 runs. The window is a named `frame`. The button is a `push button` named "Send". The
  text field is an editable `text` with Text/EditableText. The switch and the checkbox are `check box`
  nodes. The sidebar list and the 50-row transaction list are `list`/`list item` nodes, with their
  text on child `label`s.
- The app can be **driven** entirely through AT-SPI: `Selection` switches pages and selects rows,
  `EditableText` types the address, `Text` reads it back, and `Action` presses Send and flips the
  switch. Each action reaches the view model, and the reply shows up in the tree. So the dogtail/AT-SPI
  smoke tests planned for `apps.yml` are feasible as designed.
- All widgets are native GTK 4 widgets (`GtkButton`, `GtkEntry`, `GtkListBox`, `GtkSwitch`,
  `GtkCheckButton`, `GtkLabel`, `GtkPaned`, `GtkScrolledWindow`), so GTK provides roles, states and
  actions for free.

Why conditional (must be fixed in our fork before the M5 accessibility pass, and P5 before any
CrossUI screen with a list ships):
- SwiftCrossUI has no accessibility modifiers. Any control whose visible label is a separate
  `Text` therefore has an empty accessible name. That covers both `Toggle` styles that render a label,
  `TextField` with a caption, and list rows.
- One crash class: an unscrolled `List` makes the window grow past X11's 32767-px limit
  (X `BadAlloc`, app exits). This happened at 34 or more single-line rows in the probe.

## Accessibility gaps found

| Gap | Observed | Impact |
|---|---|---|
| A1 No `accessibilityLabel`/`Hint`/`Hidden` modifiers in SwiftCrossUI 0.10.0 | No API exists; only `.help()` (a tooltip) | Icon-only controls cannot get a name. This breaks DESIGN §5.3 DoD "accessibility label on icon-only controls" on Cross platforms. |
| A2 `Toggle` label not associated | `Toggle("Hide balance").toggleStyle(.switch)` → `check box ""` with a sibling `label "Hide balance"`; the same for `.checkbox` | Orca announces "check box, not checked" with no name. Tests can't find toggles by name. |
| A3 `TextField` has no name | `text ""` + attribute `placeholder-text:Dash address`; the "Pay to" caption is unrelated | Orca may read the placeholder (unverified). There is no stable name for tests or assistive tech. |
| A4 List rows have no own name | `list item ""` → `panel` → `label "…"` | AT-SPI clients that read the row's name get nothing. Orca usually flattens descendant text for focused rows (unverified). Tests must look at descendants. |
| A5 `GtkCheckButton` exposes no AT-SPI action (GTK 4.14.5) | `check box` from `.checkbox` has `actions=[]`; `GtkSwitch` has `toggle` | It cannot be toggled by an AT client through `Action`. Keyboard Space should still work (unverified). This is a GTK behaviour, not SwiftCrossUI. |
| A6 Deep anonymous containers | Every layout `GtkFixed` → `panel ""`; controls sit 10–20 levels deep | Noise for screen-reader object navigation; slower tree walks. Not blocking. |
| A7 Application name = executable name | AT-SPI app name `CrossUILinuxProbe` (and `"."` under Rosetta) | Orca's app announcement shows the binary name. Set `g_set_application_name("Dash Wallet")`. |
| A8 GTK 4.14 `GtkText` `getText(0,-1)` returns `""` | `characterCount` is right; an explicit end offset works | A harness quirk only (handled in `atspi_smoke.py`). Check whether Orca reads entry contents. |

Layout issues found on the way (no a11y impact): sidebar and page content are centred vertically
rather than top-aligned, and there is no libadwaita styling.

## Recommended fork patches (`dashpay/swift-cross-ui`, branch `dwd-0.10`)

Ordered by priority. Each one is small and uses GTK 4.0+ API that exists today, so none needs a GTK
change.

- **P1. Accessibility modifiers (fixes A1, enables A2–A4).** Add `.accessibilityLabel(_:)`,
  `.accessibilityHint(_:)` and `.accessibilityHidden(_:)`, carried as **environment values** that
  each control's `update…` reads, in the same way as `isEnabled`. Using the environment means the label
  lands on the real control widget, not on a wrapper container. Add a new
  `BackendFeatures.Accessibility` with `setAccessibility(of:label:hint:hidden:)`.
  GtkBackend: `gtk_accessible_update_property(GTK_ACCESSIBLE(w), GTK_ACCESSIBLE_PROPERTY_LABEL, …)`,
  `…_DESCRIPTION` for hints, `gtk_accessible_update_state(…_STATE_HIDDEN, …)`.
  AppKitBackend: `setAccessibilityLabel`/`Help`. WinUIBackend: `AutomationProperties.SetName`, which is
  useful once G3/#787 is resolved. Upstream this; the maintainer has an open interest in a11y.
- **P2. Toggle label association (A2).** In `Toggle.body`, pass `label` to `ToggleSwitch`/`Checkbox` as
  their accessibility label (P1 plumbing). Alternative for GTK `.checkbox`: use the native
  `gtk_check_button_set_label`, which also gives a clickable label.
- **P3. TextField name (A3).** Default the accessible label to the placeholder when no explicit
  `accessibilityLabel` is set. Our `DashUICross` text-field component will always pass its caption
  ("Pay to") explicitly.
- **P4. List row names (A4).** In `GtkBackend.setItems(ofSelectableListView:)`, set each
  `GtkListBoxRow`'s `LABEL` from the row's accessibility label. When the row's content is a single
  `Text`, use that text.
- **P5. List layout / window growth (crash).** (a) Measure list rows at the list's proposed width.
  Today a 55-char `Text` row adds about 965 px to the minimum height while it renders at 29 px. (b) Make
  `List` scroll by itself, as SwiftUI's does, by wrapping the `GtkListBox` in a `GtkScrolledWindow`.
  (c) Defensive: clamp window auto-resize to the monitor work area, and never request more than
  32767 px. **Until P5 lands, WS-12 rule:** every CrossUI `List` sits inside a `ScrollView`, and
  single-line row text uses `.lineLimit(1)`. Add this to the WS-12 review checklist.
- **P6. Container noise (A6), investigate.** Check whether creating the layout `GtkFixed` with
  `accessible-role = GTK_ACCESSIBLE_ROLE_PRESENTATION` (a construct-only property) drops it from
  the AT-SPI tree in GTK 4.14+. Low priority.
- **P7. App name (A7).** In GtkBackend, call `g_set_application_name` from the app metadata's
  display name.
- Not a fork patch (A5): report the `GtkCheckButton` missing-action behaviour upstream to GTK if it
  persists in GTK ≥ 4.16. Our smoke tests toggle checkboxes by keyboard, or use the `.switch` style
  where the design allows.
- Already planned (DESIGN §5.2): export `DummyBackend` for `CrossUISmokeTests`. This probe does not
  cover it.

## Consequences

- WS-12 builds Linux screens on SwiftCrossUI GtkBackend as designed. The shared view-model layer is
  confirmed to run unchanged under SwiftCrossUI on Linux: `@Bindable` over a `@MainActor @Observable`
  class works for TextField/Toggle/List selection bindings.
- `apps.yml` Linux job: reuse `ci/linux/Dockerfile.swift-gtk` (or the same apt packages on
  `ubuntu-24.04` runners) and the `container-a11y-smoke.sh` flow: `dbus-run-session`, Xvfb, then
  `at-spi-bus-launcher --launch-immediately`. Find the app by PID. Use `getText(0, characterCount)`.
  Run x86_64 natively. Under Rosetta the first cold build failed once with a SwiftPM
  "unexpected JSON message" error, and the retry passed.
- Packaging: the binary links the Swift runtime dynamically (83 shared libraries in `ldd`). The
  tarball needs `--static-swift-stdlib`, as §3.4 plans. A release build is 6.3 MB stripped (aarch64).
- Cost: a cold debug build of a SwiftCrossUI app pulls 23 packages (swift-syntax, swift-java,
  AndroidKit, swift-winui, …) and takes ~4 min on Apple-silicon Docker and ~7.5 min under Rosetta. A
  cold release build takes ~9 min. Cache `.build` in CI.

## Not verified

- Orca behaviour: no screen-reader session was run. Whether Orca reads placeholder text, descendant
  row text or entry contents is inferred, not observed.
- Wayland: only X11/Xvfb was tested. The GTK AT-SPI path is the same, but the window-growth crash
  may show differently there (no 32767 X limit).
- Keyboard-only navigation and focus order: not exercised.
- Table, sheet, menu and Image (QR) from the full G2 probe list in DESIGN §6. This probe covered
  List, TextField, Button, Toggle and NavigationSplitView. Table/sheet/menu/Image belong in WS-12's
  first screens, under the same harness.
- Behaviour on GTK newer than 4.14.5 (e.g. Flatpak GNOME runtime 47/48): re-run the harness in the
  Flatpak SDK when packaging starts.
- Real-GPU RSS and release-build memory: the 260–380 MB measured is a debug build on llvmpipe.
