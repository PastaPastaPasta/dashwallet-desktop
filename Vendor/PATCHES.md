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

### P2 — CSS font weights in GtkBackend (UX-SPEC §2.2, Inter on Linux)

Rationale: the app bundles Inter (Regular, Medium, SemiBold, Bold) as the Linux/Windows UI face.
Upstream 0.10.0 maps SwiftCrossUI's weights one step heavier on GTK (regular→500, medium→600,
semibold→700) to imitate AppKit with the default GTK face, so with Inter every regular text drew
in Medium and every medium text in SemiBold.

Change (`Sources/GtkBackend/GtkBackend.swift`, `cssProperties(for:isControl:)`, comment
`dashwallet-desktop patch P2`): light→300, regular→400, medium→500, semibold→600 (the CSS /
OpenType numbers); ultraLight, thin, bold, heavy and black are unchanged. Other backends are not
touched.

### P8 — coalesced observation updates (CPU pinned at 100 % after a model change)

Rationale: after a send, the GTK main thread ran at 100 % indefinitely and every AT-SPI query
timed out (docs/screenshots/ux/linux/RESULTS.md, 2026-10-08). There was no update loop. A
single assignment (`TransactionsViewModel.selection` in `reveal(txid:)`) fired about 950
`onChange` callbacks, and they then ran one by one, each re-laying out a large subtree in
7–12 s. Swift's `withObservationTracking` merges a nested tracking scope's accesses into the
enclosing scope. `ViewGraphNode` lays out its children inside its own `observe(with:_:)` call,
so every ancestor of a view that reads a property also observes it. Upstream runs each
notification on its own main-thread hop. A child's update renews only the child's own
observation, so the parent, grandparent and so on each run a full update of their whole subtree
after it. Upstream 0.10.0 and `main` (still commit `0f3ec39` on 2026-10-08) have the same
code, and no upstream issue reports it. The change is in the backend-independent core, so it
applies to every backend.

Change (`Sources/SwiftCrossUI/State/ModelObserver.swift`, `ViewGraph/ViewGraphNode.swift`,
`Environment/EnvironmentValues.swift`, `_App.swift`, comments `dashwallet-desktop patch P8`):
- The main-thread hop no longer calls `viewModelDidChange` itself. It queues the call in
  `ModelObserverUpdateQueue`, which schedules one flush per batch. The flush runs after the
  hops that the same changes have already scheduled.
- The flush runs the queued updates shallowest first and skips any observer whose observation
  was renewed in the meantime. An ancestor's update lays out its subtree again, which renews the
  descendants' observations, so their own updates are dropped. One batch of changes therefore
  costs one update of each shallowest affected subtree.
- Depth: `ModelObserver.observationDepth`, which every observer declares (there is no default):
  `_App` -1 (it refreshes every window), windows 0. A `ViewGraphNode` takes its depth from the
  internal environment value `viewGraphDepth` that its parent passes on (window root view = 1).
  A sheet's content root, created in `SheetModifier.commit` from the modifier's parent
  environment, is placed one level below the modifier explicitly.
- `.onChange` and `.onAppear` run their actions after the update, through
  `runInMainThread`. `OnChangeModifier` compares the value in `commit` (upstream: in
  `computeLayout`, with a "Should this go in computeLayout or commit?" TODO), and
  `OnAppearModifier` schedules its action when it creates its widget (upstream: called it
  there). See the rule below for why.
- A deferred action never outlives its view
  (`Views/Modifiers/Lifecycle/LifecycleHookModifier.swift`). Both modifiers are
  `LifecycleHookModifier`s: their node's children own a lifetime flag that ends synchronously when
  the node is released. That is the same moment `OnDisappearModifierChildren` schedules the
  `.onDisappear` action. A queued action checks the flag and does nothing once it has ended. It
  holds only the flag, never the children, so it cannot keep the node alive. Without this, a view
  removed before the main loop reached its queued action (a GTK event handled first, or any update
  in between) had its cleanup run and then its appear or change action. That action could restart
  a resource or a task that nothing would stop (review DW-D4 r2).

  The lifecycle guarantees under P8, then:
  - `.onAppear` and `.onChange` actions run after the update that queued them, outside any
    observation or layout scope, and only while their view is in the graph.
  - A view removed before its queued `.onAppear` ran gets no appear action, but its
    `.onDisappear` action still runs. Cleanup must therefore cope with a resource that was
    never started (upstream always ran appear first, synchronously).
  - `.onDisappear` runs from a main-actor `Task` (unchanged), so its timing relative to other
    queued main-loop work is not fixed. Only the "no action after removal" rule above is.
  - `.task` starts its task during the update (below), before the view's deferred
    `.onAppear` runs. A task body can therefore run before the appear action
    (`taskMayRunBeforeAppear`). Code that needs the appear action's setup should do that setup
    in the task, or not depend on the order.
  - None of this matches SwiftUI's ordering exactly. It is what this patch guarantees.
- Not changed: tracking scopes still merge. The window's root node still observes nearly every
  property read in the window, so nearly any model change costs one whole-window layout (P8
  only stops the chain of them). A deeper fix would track a node's own `body` and layout
  separately from its children's. That is a larger change to upstream's update model, and it
  is not done here.
- **Rule: views must not write observed state while they lay out** (in `body`, in
  `computeLayout`, or from backend code that a layout calls, such as a signal handler fired by a
  widget update). `withObservationTracking` starts observing a node's layout only when that
  layout returns. A write made during it is missed by every ancestor that read the old value
  earlier in the same pass. Their pending updates have already run, because they are the
  shallowest, so they stay stale until some unrelated change. Upstream has the same race, but
  its order is random (the review measured 3 stale runs in 12); P8 makes it certain. This is
  why `.onChange` and `.onAppear` now run after the update. Writes made while an update commits
  are safe: every observation of that update has started by then. `.onDisappear` was already
  deferred (a main-actor `Task` from a `deinit`). `.task`, built on `.onChange`, still starts
  its task during the update (`OnChangeModifier.runsAfterUpdate`): the start writes only its
  `@State`, which observation does not track, and a deferred start could come after the
  view's `.onDisappear` and leave a task nobody cancels (`taskCancelledAfterImmediateRemoval`).
  The task itself runs later. A known
  path that can still write during layout: `Picker` updates its `GtkDropDown` from
  `computeLayout` (an upstream TODO), and replacing its options can fire the previous update's
  selection handler.
- Tests: `Tests/SwiftCrossUIPatchTests` (root package, also in the headless graph), over a fake
  backend built on the vendored `BackendFeatures.BaseStubs` (`FakeBackend.swift`):
  - `ModelObserverUpdateQueueTests`: the queue on its own.
  - `ViewGraphUpdateTests`: real view graphs. The updates wait for one flush, each node's depth
    is its parent's plus one, and a batch costs exactly one update of the shallowest affected
    subtree (over 5 batches). These fail if the hop in `observe` or the depth propagation is
    reverted. Also covered: ancestors that do not read the property, a descendant dropped in the
    same batch, a change before the flush or during commit, two windows, `.onChange` and
    `.onAppear` writes read by an ancestor, `.task` starting during the update, and the rule
    above as a known issue.
  - `LifecycleTests`: queued `.onAppear` and `.onChange` actions (initial and later ones) are
    dropped when their view goes first, and a late appear cannot start a task. These fail
    without the lifetime check. Also: `.task` is cancelled when its view goes before the main
    loop runs, and the task-before-appear order.
  - `WindowSizeTests` (P10).

  The target needs only the framework core (no GTK, no backend), so `DWD_HEADLESS=1` keeps it.
  `scripts/linux-docker-test.sh` runs it on Linux without GTK. `container-demo.sh` also runs it,
  before the GUI sessions, built as the `DashWalletDesktopPackageTests` product, because in the
  full graph a plain `swift test` also builds swift-winui's Windows-only C target on Linux.
  `swift test` runs it on macOS.

### P9 — GTK CSS reloaded only when it changes

Rationale: the other half of the same slowness. GtkBackend gives every widget its own
`GtkCssProvider`, registered for the whole display. Every `load_from_data` therefore
invalidates style matching for every widget on the display. Upstream reloaded providers when
nothing had changed:
- `css.clear()` followed by `css.set(...)` mutates `Widget.css` twice. Its `didSet` equality
  guard sees the empty intermediate block, so every update of a `Text`, text field, button,
  toggle, date picker, text editor or sheet loaded CSS twice.
- View-label buttons (`GtkCustomButton.loadCSS`) and pickers (`updatePicker`) loaded their CSS
  on every update.
- `size(of:whenDisplayedIn:)` restyled one shared measurement label for every line-limited
  text, so the label's CSS flipped whenever two consecutive texts differed in font or colour.
  In the post-send profile this path alone took 21 % of the main thread and also made the
  following `gtk_widget_create_pango_context` calls expensive (28 %).

Change (comments `dashwallet-desktop patch P9`):
- `Gtk/Utility/CSS/CSSProvider.swift`: `loadCss(from:)` skips data equal to what the
  provider already holds. The guard tracks the provider's content, not one writer's, so
  widgets whose provider has several writers keep upstream's result (for example a button
  whose `.cornerRadius` writes the same provider as `GtkCustomButton.loadCSS`).
- `Sources/GtkBackend/Features/{TextViews,TextFields,StringLabelButtons,ToggleButtons,
  DatePickers,TextEditors,Sheets}.swift`: each update writes its CSS block in one
  `set(properties:clear: true)`, so neither `Widget.css`'s guard nor the provider sees the empty
  intermediate block.
- `TextViews.swift`, `size(of:whenDisplayedIn:)`: for a label (`Text`), which is styled for
  the same environment just before it is measured, the line-limit height uses the label's own
  Pango context. Text editors, styled only at commit, still use the shared measurement label.

Effect, measured with P8 (debug build, demo send flow): the post-send busy period went from
never ending to about 15 s, and a whole-window update of the Transactions page from about 7 s
to about 3 s. Upstream `main` has the same code.

### P10 — window sizes: measured menu bar, no re-request of a refused size

Rationale: the live-mode version of the same symptom. After a wallet was created on the live
engine, the Overview's minimum content height was 787 px and the window's content area was
785 px. GtkBackend sets a window's size as content plus a hard-coded 25 px menu bar, but the
app's menu bar (File / Settings / Window / Help) is 27 px with this theme. So every
`setSize(ofWindow:)` left the content 2 px short, and GTK's allocation (785) did not match the
preempted size (787). The resize callback fired, `WindowReference` clamped back up to the
content minimum and requested 787 again, and so on: a resize ping-pong with a whole-window
layout in each round, at 100 % CPU, with every AT-SPI query timing out ("the new wallet's
Overview is shown"). Any window that cannot take the requested size behaves the same way, for
example one a window manager has maximized or tiled on a small screen. Upstream 0.10.0 and `main`
have the same code (the hard-coded height has a "Don't hardcode this" TODO).

Change (comments `dashwallet-desktop patch P10`):
- GtkBackend (`Features/CoreWindowing.swift`, `menubarHeight(ofWindow:)`): measure the menu
  bar, the window's direct child with the CSS name `menubar`, at its minimum height, as
  GtkApplicationWindow allocates it. The old 25 px stays as the fallback for when GTK has not
  created the menu bar yet. That was observed at the window's first sizing.
- GtkBackend (`setSizeLimits`): the window's minimum is the content's minimum plus the menu
  bar. Upstream applied the content's minimum to the whole window, so a user could shrink the
  content one menu bar below its minimum.
- GtkCHelpers (`gtk_custom_root_widget.c`): the root widget reports the first allocation after
  a preempted size (`setSize(ofWindow:to:)`) even when it matches. Upstream returned early on a
  match, so an honoured request was never answered.
- SwiftCrossUI (`Scenes/WindowReference.swift`): remember the size last requested with
  `setSize(ofWindow:to:)`. The next resize event answers it:
  - The same size means the request was honoured. The request is forgotten, and there is no new
    update, because the update that made the request already laid the window out at that size.
  - A different size means it was refused. The refusal (requested, kept) is noted. A later
    update that would request the same size again while the window still has the kept size
    lays out at that size instead. Any other window size clears the refusal, so unrelated
    resizes behave as upstream's do.

  The first version of this patch had no report of an honoured request. The request then stayed
  until the next resize, and a later user resize was taken for a refusal. That left a window
  shrunk to its GTK minimum with the content clipped (review DW-D4 r1).

  On GTK a re-request matters little once the minimum includes the menu bar: a window manager
  keeps the window at least that tall. Without one (Xvfb), a window forced below it stays there.
  `setSize(ofWindow:to:)` sets the window's default size, and GTK does not resize a mapped
  window when that value is unchanged. The re-request then gets no allocation and stays
  pending until the next one, which answers it as a refusal.

  AppKitBackend does not report an honoured request (`windowWillResize` only sees user
  resizes). The request stays until the user resizes, which then counts as a refusal. That is
  harmless there, because AppKit keeps the content at least at its minimum itself.
- Tests: `Tests/SwiftCrossUIPatchTests/WindowSizeTests.swift` drives a real `WindowReference`
  over the fake backend with the app's numbers (a 1100x760 default window, a content minimum
  of 787). The checks: the window grows to the minimum and the report costs no update, a later
  shrink below the minimum is re-requested (not taken for a refusal), and a request a window
  manager refuses is not repeated.
  The demo harness's "the main thread goes idle" checks run after the live onboarding and the
  send flows (`ci/linux/crossui/atspi_demo.py`).
