# UX restyle of the SwiftCrossUI app — Linux results (M3, branch `m3/ux-cross`)

Command: `DWD_CROSSUI_SUITE=ux scripts/crossui-linux-demo.sh docs/screenshots/ux/linux`
(GtkBackend, GTK 4.14, Xvfb 1280×860, aarch64 `swift:6.3.3-noble`, image `dwd-linux-crossui`).
The `ux` suite first replays the M1 flows and the M2 menus / options / tools / pages flows as a
regression check (screenshots stay in the container; checks in `regression-checks.json`), then
captures every restyled screen in light and dark (`--appearance light|dark`, new option) into this
folder (`atspi-checks.json`, one `atspi-*.txt` tree and one PNG per screen).

## Runs

| Run | Commit | Outcome |
|---|---|---|
| 1 | `48d1df2` | Did not build: `CGtk.GObject` vs `Gtk.GObject` ambiguity in `ToolkitTheme.swift` (Linux-only code). Fixed in `4ff2983`; a compile-only Docker build (no app run) then passed. |
| 2 | `4ff2983` | Built. Every session that shows the Overview (or the gallery) trapped at start: `Fatal error: Double value cannot be converted to Int because it is either infinite or NaN` in SwiftCrossUI `Button.computeLayout` — the shortcut item's label had `maxWidth: .infinity`, and `Button` converts its label size to `Int`. All other sessions passed. Also showed that GtkBackend ignores `preferredColorScheme`, so `--appearance dark` and the Theme setting had no effect on GTK. Fixed in `a3aa1e8`. |
| 3 | `a3aa1e8` | **The screenshots in this folder.** No crash. 93 UX checks, 5 hard failures; 140 regression hard checks, 3 failures (below). |

After run 3 (no fourth run allowed), commit `0025a9f` changed only measured constants and one view
split, verified by the macOS build only: sidebar row height 38 → 48 and sidebar min width 200 →
240 (run 3 shows the fourth sidebar row, Transactions, scrolled out of its list and "Sign / Verify …",
"PSBT Operati…" truncated), transaction list rows 62 → 76 per row (the day cards clip their last
row in run 3), the sidebar wordmark always `brand-dash-logo` (the testnet variant rendered tiny),
white icons on the selected sidebar pill (blue `settings-*` tiles vanished on it), and the
transaction details moved into their own observing view (see failure 1). **These are not verified
on Linux.**

## Failures in run 3 (honest list)

1. **Transaction details do not appear after selecting a row in list mode** (`ux-*-05-transaction-details`,
   regression `m2-pages` "details show Abandon / Resend", "details list 'Net amount'"). The row is
   selected (highlighted in `ux-light-05-transaction-details.png`), and the details do appear at the
   next re-render (dark run: after pressing "Table", `ux-dark-06-transactions-table.png` shows them),
   so the selection works but the page is not re-rendered when `model.detail` arrives. In M2 the same
   check passed with the single table list. Not root-caused; the post-run change (details in their own
   view that reads `model.detail`) is a guess and unverified.
2. **`send: the sent payment is listed on the Transactions page`** (regression). Passed in run 2 with the
   same Transactions code, failed in run 3; probably the same stale-render problem (after the send the
   app reveals the new transaction). Unverified.
3. `press 'Table'`, `press 'Address Book'` and the Wallets marker in the light main session: the
   light session stalled after the failed details wait (the same presses passed in the dark session,
   and `ux-dark-07-address-book.png` exists). There is no `ux-light-06`/`ux-light-07` screenshot.

Everything else passed: Overview (hero, breakdown, shortcut card, day cards), Send, Receive,
Transactions list and table, Sign / Verify, Tools (Information, Console, Peers), PSBT, Wallets,
Options, Security, Settings, About, onboarding (welcome, phrase), lock screen, sync overlay and the
component gallery, in both appearances; the M1 onboarding / tools / overlay flows and the M2 menus,
options + coin selection and tools flows.

## What was looked at (Read tool, every PNG of run 3)

- **Overview** (`ux-*-01-overview`): blue hero band (`App.dashNavigationBarBlueColor`), TESTNET capsule,
  total in large bold Inter with the Dash glyph, breakdown strip (Available / Spendable now, Pending /
  Awaiting confirmation) on white 12 %, "Hide balance" button, white shortcut card straddling the
  band (iOS icons), "History" + Filter, day cards with iOS rows (green ↓ / blue ↑ icons, type titles
  "Received", label "Alice", time, Pending / InstantSend chips, `+0.20Ð` compact amounts, no green
  amounts, no raw addresses). Matches `docs/design/ux/home-light.png` closely. Dark: canvas `#141519`,
  cards `#1E1F24`, no pure black.
- **Shell**: sidebar with PNG icons and the Dash-blue selected pill, "MORE" section with the tool
  pages as rows (no blue link buttons); status bar is text items only: Demo badge, "Up to date",
  HD, Unencrypted (red), unit picker, "8 peers", "Synced" — no network, no demo sentence.
- Inter is the UI face; monospaced text only in the console, the transaction ID and signatures.
- Native GTK widgets take the Dash blue accent (switches, progress bars, list selection).
- Remaining visual issues seen: in run 3 the sidebar list cut its 4th row and truncated two More rows,
  and day cards clipped their last row (constants changed after the run, unverified); the selected
  More row hid its icon; the hero shows the demo's 8 decimals because the demo's Decimal digits
  setting is 8 (floored per QT-036, not a layout bug); day headers read "6 October 2026" instead of
  "Today" because swift-corelibs-foundation does not do relative date formatting.

## Not covered by this run

- M3 screens (CoinJoin card and page, masternodes, governance) are not built on Cross; the CoinJoin,
  Masternodes and Governance sections still show the "not available yet" empty state.
- Windows (WinUIBackend): not built.
- macOS: the Cross app was built (`swift build --product dash-wallet`, AppKitBackend) but not run —
  the repository rules forbid opening its window on the developer's Mac, and there is no offscreen
  backend in the vendored SwiftCrossUI.
