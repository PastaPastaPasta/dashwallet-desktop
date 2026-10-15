# dashwallet-desktop — UX specification (visual and interaction source of truth)

Status: **authoritative for visuals and interaction**, 2026-10-06 (M3). Product scope, architecture and
feature rules stay with `DESIGN.md` → `DESIGN-opus.md`. Where a screen in this file disagrees with a
dash-qt *string, limit or formula* in `docs/research/02-dash-qt-features.md`, research 02 wins for the
string or number, and this file wins for layout, styling and interaction. **Exception:** the masternode and
governance screens (§4.25, MG-00) are mobile-inspired by DEC-01 and DASHPAY §5a; there Dash Core's limits and
formulas still win, but only the two dash-qt strings named in MG-00 D29 are kept verbatim.

Who uses this file:
- the **restyle agents** (MacUI + DashUIMac, CrossUI + DashUICross) — sections 2–5;
- the **design reviewer** — sections 6 and 7;
- the **feature agents** for CoinJoin (sections 3, 4.21 and 5) and for masternodes and governance (MG-09,
  MG-10, MG-10b: sections 3, 4.25 and 5).

Reference images: `docs/design/ux/home-light.png`, `docs/design/ux/home-dark.png` (a rendered mock of §4.5,
drawn with the real tokens and the exported icons; sample data). They show the target. The current app
screenshots are in `docs/screenshots/m1`, `m2` (and `…/linux`).

Sources read for this spec (all claims below come from these files):
- dashwallet-iOS: `DashWallet/Sources/UI/Home/Views/HomeView.swift`, `Home Balance View/HomeBalanceView.swift`,
  `Shortcuts/ShortcutsBarView.swift`, `ShortcutItemView.swift`, `Models/ShortcutAction.swift`,
  `Cells/SyncingHeaderView.swift`, `SwiftUI Components/{Color,Font}+DWStyle.swift`, `MenuItem.swift`,
  `Style/Style.swift`, `Menu/Security/SecurityMenuScreen.swift`, `Payments/Pay/Confirm/ConfirmPaymentViewController.swift`,
  `Payments/Receive/RequestAmount/RequestAmountScreen.swift`, `Menu/Tools/MasternodesScreen.swift`,
  `LockScreen/*`, `Setup/SecureWallet/*`, `Tx/Details/*`, `DashWalletScreenshotsUITests` (store screenshots:
  Home, Send confirmation, Receive, Advanced Security, Seed phrase backup), `fastlane/metadata/en-US`.
- DashUIKit `e8d9243` (`dashwallet-desktop-deps/dashuikit-src/…/docs/*.md` and sources) and our port
  `Sources/DashUIMac/DashUIKit` (`VENDORED.md`).
- `Sources/DesignTokens` (+ generator `scripts/gen-tokens.swift`, manifest `scripts/icon-manifest.json`,
  output `Resources/Tokens/tokens.json`, `Resources/Icons`).
- Current app: `Sources/MacUI/**`, `Sources/DashUIMac/Desktop/**`, `Sources/CrossUI/**`, `Sources/DashUICross/**`,
  every PNG in `docs/screenshots/m1`, `m2` and their `linux/` folders.
- SwiftCrossUI 0.10.0 sources at the pinned revision `0f3ec39` (capabilities in §3.0), ADR 0002.

---

## 1. Design principles

1. **It is the Dash iOS wallet, on a desk.** Same palette (white cards on a light-grey canvas, Dash blue
   `#008DE4` as the only saturated accent), same type scale, same components (balance hero, shortcut card,
   day-grouped transaction cards, menu cards with 30 pt icons, bottom-sheet chrome, toasts). Someone who
   uses the phone app recognises every screen.
2. **Desktop ergonomics, not a blown-up phone.** Sidebar navigation instead of a tab bar; resizable windows
   with a centred content column (max widths in §2.4); sheets and secondary windows instead of push
   navigation; hover, focus rings, keyboard shortcuts, right-click context menus, tooltips (`.help`) and
   text selection everywhere a phone would use long-press.
3. **dash-qt power stays, styled.** Every dash-qt feature, string and limit (research 02) is kept. Dense,
   technical views (transaction table mode, coin control, peers, console, PSBT, raw transaction, keys)
   keep their table form (masternodes and governance are the exception: §4.25, mobile-inspired), but use the same tokens, fonts and row metrics. They are
   *technical views* (§5.6) — the only places monospaced type is allowed.
4. **One accent, colour means status.** Dash blue for primary actions, selection, links and focus. Green,
   red, orange and yellow only for status (badges, illustrations, validation, warnings). Amounts are never
   coloured by direction — the direction is the icon. No purple/violet anywhere (the generator already drops
   `Purple`; do not add one back, not even via a system accent).
5. **Honest data.** A value we do not have is "—" with a tooltip that says why ("Requires full-node data
   source", "Not available until sync completes"), never `0`, never a guessed fiat. A fiat line appears only
   when a rate source exists.
6. **Calm hierarchy.** One large number per screen (the hero amount or the sheet amount); everything else is
   subhead/footnote. One primary (filledBlue) button per surface.
7. **Same vocabulary in both toolkits.** MacUI (SwiftUI) and CrossUI (SwiftCrossUI) use the same component
   names, anatomy and tokens (DESIGN-opus §1.10). Where SwiftCrossUI cannot draw something, §3 names the
   fallback; the fallback is the spec for Cross, not a bug to work around per screen.
8. **Accessible by construction.** Every icon-only control has a label (macOS) — and on Cross, where
   SwiftCrossUI 0.10 has no accessibility modifiers (ADR 0002 A1), there are **no icon-only controls**: icon +
   text always.

---

## 2. Tokens

Tokens live in `Sources/DesignTokens`. Colours and icons are **generated** by `scripts/gen-tokens.swift`
from the dashwallet-ios `Shared/Resources/SharedAssets.xcassets` (→ `DashColor.App.*`) and DashUIKit
`Media.xcassets` (→ `DashColor.*`), with the curated icon list `scripts/icon-manifest.json`; output
`Sources/DesignTokens/Generated/{Colors,Icons}.swift`, `Resources/Tokens/tokens.json`, `Resources/Icons/*`.
Type scale, spacing, radii and button metrics are hand-written files transcribed from DashUIKit
(`Typography.swift`, `Spacing.swift`, `Radii.swift`, `ComponentMetrics.swift`) and tested against
`tokens.json`. **Never hand-copy a hex value** into UI code; add a token or an alias.

### 2.1 Semantic colour roles (use these; they map to generated tokens)

The two catalogs disagree on surfaces and DashUIKit has some dark-mode values that are wrong for a desktop
(white switch track, light grey "tertiary background" in dark). The roles below pick one value per job.
Add them as a hand-written `DashColor.Role` namespace in `Sources/DesignTokens/Roles.swift` whose members
are **aliases of generated tokens** (no hex literals), plus a test that each alias resolves.

| Role (`DashColor.Role.*`) | Alias of | Light | Dark | Use |
|---|---|---|---|---|
| `canvas` | `App.secondaryBackgroundColor` | #F7F7F7 | #141519 | window/page background (iOS `dw_secondaryBackground`) |
| `card` | `secondaryBackground` | #FFFFFF | #1E1F24 | cards, menu cards, sheets, popovers' content |
| `cardRaised` | `App.tertiaryBackgroundColor` | #FAFAFA | #1D2023 | nested card inside a card, table header |
| `sidebar` | system sidebar material (macOS) / `App.secondaryBackgroundColor` (Cross) | — | — | sidebar column |
| `hero` | `App.dashNavigationBarBlueColor` | #008DE3 | #008DE4 | balance hero band, lock screen |
| `heroCard` | `white` × `DashOpacity.heroCard` (0.12, see note) | #FFFFFF @12% | #FFFFFF @12% | breakdown strip inside the hero |
| `accent` | `blue` | #008DE4 | #008DE4 | primary buttons, selection, links, focus ring, switches ON |
| `accentTint` | `blueAlpha10` | #008DE4 @10% | #008DE4 @10% | selected row tint, info badges, detail chips |
| `textPrimary` | `primaryText` | #0A0A0D | #FFFFFF @90% | titles, values, amounts |
| `textSecondary` | `secondaryText` | #525C66 | #FFFFFF @80% | subtitles, captions, fiat lines, field labels |
| `textTertiary` | `tertiaryText` | #75808A | #FFFFFF @60% | day-header weekday, hints, disabled labels |
| `textOnHero` | `whiteText` | #FFFFFF | #FFFFFF | all text on `hero` |
| `textLink` | `blueText` | #008DE4 | #008DE4 | plain links, "Filter", inline actions |
| `separator` | `App.separatorLineColor` | #D5D5D5 | #4A4A4A | hairlines between rows (0.5 pt mac / 1 px Cross) |
| `fieldFill` | `textFieldCryptoAddressBackground` | #B0B6BC @10% | #B0B6BC @10% | text-field and search fill |
| `fieldStroke` | `buttonStrokeGrayStroke` | #75808A @25% | #75808A @25% | focused/hover field border |
| `success` | `green` | #3DB58A | #3DB58A | success illustration, "Funded", "Locked (ChainLock)" status |
| `successTint` | `greenAlpha10` | #3DB58A @10% | — same — | success badge background |
| `danger` | `red` | #EB3842 | #EB3842 | errors, destructive buttons, PoSe-banned |
| `dangerTint` | `redAlpha10` | #EB3842 @10% | — same — | error banner/badge background |
| `warning` | `orange` | #FA9169 | #FA9169 | network capsule, warnings, mixing-only lock, "Locked" maturity |
| `warningTint` | `orangeAlpha10` | #FA9169 @10% | — same — | warning banner background |
| `caution` | `yellow` | #FFBF42 | #FFBF42 | stalled sync, stale rate |
| `overlay` | `App.modalDimmingColor` | #04040F @40% | #04040F @40% | scrim behind modal overlays |
| `toastFill` | `toastBackground` | #0A0B0D @90% | #B0B6BC @10% on blur | toast |
| `switchOn` | `blue` (override; DashUIKit `switchTrackFillOn` is white in dark) | #008DE4 | #008DE4 | toggles |
| `switchOff` | `switchTrackFillOff` (light) / `whiteAlpha20` (dark) | #B0B6BC | #FFFFFF @20% | toggles |

Notes:
- `heroCard`: iOS draws `Color.dash.white.opacity(0.12)` and dividers at `0.18`. Add generated-adjacent
  aliases `whiteAlpha12`/`whiteAlpha18` **only** if the catalog gains them; until then the role is
  `white` with an explicit opacity constant (`DashOpacity.heroCard = 0.12`, `heroDivider = 0.18`,
  `heroSecondaryText = 0.7`, `heroHint = 0.5`) — opacities are tokens too, not inline literals.
- **Do not use** for desktop surfaces: `primaryBackground` (DashUIKit; dark = pure #000000 — the current
  dark Overview is black because of it), `tertiaryBackground` (dark = #EBEDEE, a catalog bug),
  `buttonStrokeGrayContent` / `buttonPlain*ContentDisabled` in dark (they resolve to near-black on dark).
  `DashButton` styles that use those get a desktop override in `Roles.swift` (`strokeGrayContent` →
  `primaryText`, `plainContentDisabled` → `textTertiary`).
- Shadow: `Color.dash.shadow` (light `#B8C2CC` @10%, dark clear) — see §2.6.
- The full generated palette (every named colour, both catalogs, light + dark) is Appendix A.

### 2.1a WCAG AA variants (DEC-36)

The iOS palette fails WCAG 2.1 AA (4.5:1 for text under 18.7 px bold / 24 px) on several pairs the
roles above prescribe: white on Dash blue (3.5:1), blue text on canvas or on `accentTint` (3.2–3.3:1),
`textTertiary` on white (4.0:1), the hero strip's text (2.2–3.0:1), the lock prompt (2.8:1), the
network capsule (2.5:1) and orange "Locked" text (2.3:1). Accessibility is a product requirement, so
**both stacks draw the variants below** wherever text sits on, or is drawn in, the role; gate G-UI
judges U4 on them. They live in `Roles.swift` as `DashColor.RoleAA` and `DashOpacityAA` (hex literals,
because the iOS catalogs have no such colours). Each keeps its role's hue in OKLCH and changes lightness
only as far as AA needs; non-text uses (icons, focus rings, switch tracks: 3:1) may keep the iOS value.

| Role | Light | Dark | Replaces | Worst text pair |
|---|---|---|---|---|
| `RoleAA.accent` | #006FC3 | #006FC3 | `accent` as a fill under white text | white 5.2:1 |
| `RoleAA.textLink` | #006FC3 | #1594EC | `textLink`, and `accent` / `buttonPlainBlueContent` used as text | on `accentTint` 4.6:1 (light), 4.5:1 (dark) |
| `RoleAA.hero` | #0063B7 | #0063B7 | `hero` | strip subtitle 4.5:1 |
| `RoleAA.textTertiary` | #68737D | #FFFFFF @60% | `textTertiary` | on canvas 4.5:1 |
| `RoleAA.warning` | #B6542C | #FA9169 | `warning` as text | on canvas 4.6:1 |
| `RoleAA.networkCapsule` | #B6542C | #B6542C | the capsule's `warning` × 0.9 | white 4.9:1 |
| `RoleAA.danger` | #C8202B | #FF6961 | `danger` as text (error states, danger chips) | on canvas 5.3:1, on `dangerTint` 4.9:1 (light); on canvas 6.5:1, on `dangerTint` 5.3:1 (dark) |
| `DashOpacityAA.heroCard` | 0.08 | 0.08 | `DashOpacity.heroCard` (0.12) | — |
| `DashOpacityAA.heroSecondaryText` | 0.9 | 0.9 | `heroSecondaryText` (0.7) and the lock prompt's `whiteAlpha80` | — |
| `DashOpacityAA.networkCapsule` | 1 | 1 | `DashOpacity.networkCapsule` (0.9) | — |

`danger` (#EB3842) is 4.06:1 on white and 4.2:1 on the dark card, so red text uses `RoleAA.danger`. The
gate's four screens draw none; the splash's error state does (found by U-07's axe run of the fixture
error states).

### 2.2 Typography

Scale = DashUIKit `DashTextStyle` (already `DesignTokens.DashTextStyle`):

| Style | Size / weight / line | Desktop use |
|---|---|---|
| `largeTitle` | 34 bold / 41 | hero amount only |
| `title1` | 28 bold / 34 | sheet amount (confirm, success), onboarding titles |
| `title2` | 22 bold / 28 | page title (TopIntro) |
| `title3` | 20 bold / 25 | card titles in empty states, wizard step titles |
| `headline` | 17 bold / 22 | section headers inside pages ("Balances", "Requested payments") |
| `body` | 17 regular / 22 | long-form text in onboarding/info dialogs only |
| `callout` / `calloutMedium` | 16 / 21 | button labels (large), primary form fields |
| `subhead` / `subheadMedium` | 15 / 20 | menu-row titles (`subheadMedium`), field values, "History" header |
| `footnote` / `footnoteMedium` | 13 / 18 | transaction rows (title `footnoteMedium`, time `footnote`), table cells, fiat lines |
| `caption1` / `caption1Medium` | 12 / 16 | status badges, detail chips, status bar, help text |
| `caption2` | 11 / 13 | shortcut captions (`caption2` semibold), network capsule (11 bold, tracking 1.2) |

Fonts:
- macOS: SF Pro via `.system(size:weight:)` (already). Apply `.dashFont(_:)` (line height) everywhere
  except inside tables.
- Linux/Windows: **bundle Inter** (OFL) in `DashUICross` resources and select it in `DashTextStyle.font`
  (DESIGN-opus §1.10 decided this; it is not done yet — Linux screenshots show the GTK default face).
  SwiftCrossUI has no line-height modifier; vertical rhythm comes from explicit spacing (§2.4).
- Digits: `monospacedDigit()` (tabular figures, proportional face) for any number that sits in a column
  (tables, the breakdown strip, coin control). **Never** `design: .monospaced` except in technical views
  (§5.6). Amount text uses `medium` weight in rows, `bold` in the hero.
- Text never truncates without a tooltip; addresses/hashes truncate in the middle (§5.5), titles at the tail.

### 2.3 Spacing (existing `DashSpacing`, plus layout constants to add)

Existing steps: 2 · 4 · 6 · 8 · 10 · 12 · 16 · 20 · 24 · 40, `stack` 15, `screenHorizontal` 20,
`menuCardInner` 6, transaction row 10 h / 12 v / 16 icon gap.

Add `Sources/DesignTokens/Layout.swift` (`DashLayout`):

| Constant | Value | Rule |
|---|---|---|
| `pagePaddingH` | 24 | horizontal page inset inside the detail column |
| `pagePaddingTop` | 20 | top inset under the toolbar |
| `contentMaxWidth` | 760 | Home, Receive, Settings-style pages: centred column |
| `formMaxWidth` | 640 | Send, Sign/Verify, forms, wizards |
| `tableMaxWidth` | ∞ | technical tables fill the column |
| `sectionGap` | 24 | between cards/sections |
| `cardPadding` | 16 | inside cards (menu cards use `menuCardInner` 6 + row padding 10) |
| `rowMinHeight` | 56 | menu rows (iOS `SecurityMenuScreen` `.frame(minHeight: 56)`) |
| `txRowMinHeight` | 62 | transaction rows (30 icon + 12+12 padding + text) |
| `sidebarWidth` | min 200 / ideal 220 / max 280 | sidebar column |
| `heroHeight` | 236 (with breakdown) / 180 (hidden balance) | Home hero band incl. the half of the shortcut card that overlaps |
| `sheetWidth` | 480 / 640 / 760 | small (confirm, alerts with fields) / medium (details) / wizard |
| `windowMin` | 920 × 600 | main window (already) |
| `statusBarHeight` | 28 | |

### 2.4 Radii (existing `DashRadius`)

| Token | Value | Use |
|---|---|---|
| `card` | 20 (continuous) | menu cards, shortcut card, cards on pages, toasts |
| `standard` | 12 | hero breakdown strip, toasts' inner pill, input wells, QR well |
| `textField` | 16 | AddressField / large inputs (iOS `AddressFieldView`) |
| `searchField` | 14 | SearchBar |
| `transactionIcon` | 12 | non-circular tx icon tiles (merchant logos) |
| day-group card | 10 | `TransactionGroupCard` (iOS `RoundedShape(…radii: 10)`) — add `DashRadius.group = 10` |
| `switcher` 7 / `small` 6 | badges and chips (detail chip radius 7) |
| sheets | 16 on macOS sheets is system-drawn; Cross dialogs use `card` 20 | |
| `DashButtonMetrics` | 16 / 14 / 11 / 9 | large / medium / small / extraSmall buttons |

SwiftCrossUI draws plain circular corners (no continuous style). Accept the difference.

### 2.5 Elevation

| Level | macOS | Cross (no shadow API in 0.10) |
|---|---|---|
| 0 canvas | none | none |
| 1 card | `Color.dash.shadow`, radius 10, y 5 (DashUIKit `MenuViewModifier`) | light: 1 px border `gray300Alpha30`; dark: none (cards already lighter than canvas) |
| 1b menu card (settings menus) | radius 20, y 5 (iOS `SecurityMenuScreen`) | as level 1 |
| 2 floating (shortcut card over hero, toasts, popovers) | radius 10, y 5 + `separator` 0.5 pt border in dark | border `gray300Alpha30` (light) / `whiteAlpha10` (dark) |
| 3 modal | system sheet / window shadow | toolkit dialog |

Dark mode has no shadows (DashUIKit makes `shadow` clear); separation comes from `card` vs `canvas` contrast.

### 2.6 Motion

| Name | Value | Source |
|---|---|---|
| `standard` | 0.35 s ease-in-out | iOS `Style.swift kAnimationDuration` |
| `balanceToggle` | 0.3 s ease-in-out | `HomeBalanceView` |
| `overlay` | 0.2 s opacity | current MainWindowView lock/overlay |
| `press` | scale 0.93, spring(0.4, 0.5) | `ShortcutCellButton`; nav buttons 0.88 + opacity 0.7 |
| `syncingPulse` | opacity 0.3↔0.7, 0.8 s autoreverse | "Syncing Balance" caption |
| hover | 0.12 s background fade | desktop only |

Respect "Reduce motion" (macOS `accessibilityReduceMotion`): drop scale/pulse, keep fades. Cross: no
animation API worth using — state changes are instant.

### 2.7 Iconography

- **Exported set** (`Resources/Icons`, `DashIconToken`, 157 icons in 14 groups, light/dark PNG @2x/@3x):
  tx icons are coloured circles (`tx-received` green ↓, `tx-sent` blue ↑, `tx-internal-transfer` light-blue
  ⇅, `tx-mixing` blue shuffle, `tx-mining`, `tx-error`), `action-*` are 30 pt blue circles (menu/shortcut
  style), `settings-*` are blue filled glyphs (menu rows), `glyph-dash-currency` is the Dash "D" currency
  glyph (template; tint with the text colour), `brand-dash-logo(-testnet)` the wordmark.
- **macOS**: use exported tokens through `DashIconImage`. SF Symbols only for (a) sidebar items, (b) toolbar
  buttons, (c) status-bar items, (d) inline glyphs with no exported equivalent (`doc.on.doc`,
  `arrow.triangle.2.circlepath`, `moonphase.*`). SF Symbols render as templates in `accent`/`textSecondary`.
- **Cross**: PNG only (SwiftCrossUI `Image(URL)` supports png/jpg/webp, **not SVG**). `DashUICross` gets a
  resource bundle with the exported set (same as `DashUIMac`) and a `DashIcon(_ token:)` view that picks the
  `-dark` file by appearance. Icon always next to text (A1).
- **Add to `icon-manifest.json`** (regenerate; do not hand-copy):
  - `shortcut-bar-receive`, `-send`, `-scan-qr`, `-send-address`, `-backup`, `shortcut_getTestDash`,
    `shortcut_switchNetwork`, `shortcut_syncNow` (iOS `AppAssets/Shortcuts`) → group `shortcut`;
  - `dash_logo_template`, `logo` (white-on-blue wordmark for the hero and lock screen);
  - DashUIKit `Menu/dash-logo-square` (QR centre badge, see §4.8);
  - rasterise `settings-wallets.svg` to PNG (it is the only SVG in the set; Cross cannot load it).
- Sizes: tx/menu icons 30 pt; shortcut icons 46 pt frame (asset ~40); sidebar/toolbar symbols 16–18 pt;
  status bar 14 pt; illustrations 90 pt.

Sidebar symbol map (macOS): Overview `house`, Send `arrow.up.right`, Receive `arrow.down.left`, Transactions
`list.bullet.rectangle`, CoinJoin `shuffle`, Masternodes `server.rack` (already in
`MainWindowView.Sidebar.icon`; keep). There is no Governance item: it is a tab of Masternodes (§4.25.1).

### 2.8 Token changes to make (summary for WS-11/12)

1. `Roles.swift`: `DashColor.Role` aliases (§2.1) + `DashOpacity` constants; test every alias.
2. `Layout.swift`: `DashLayout` (§2.3); `DashRadius.group = 10`; `DashMotion` (§2.6); `DashElevation`
   descriptors (radius, y, colour role) that both toolkits read.
3. Manifest additions (§2.7) and regeneration; SVG → PNG.
4. Inter font files + licence in `DashUICross` resources; `DashTextStyle.font` picks Inter on Linux/Windows.
5. `AmountStyle.compact` in `WalletFeatures/Common/AmountFormatter.swift` (§5.2) with golden tests.
6. App accent: `Apps/macOS` asset catalog `AccentColor` = #008DE4 (generated from `DashBlueColor` by the
   token script, not typed) **and** `.tint(Color.dash.blue)` on every scene root, so sidebar selection,
   focus rings, default buttons, toggles and pickers are Dash blue regardless of the user's system accent
   (the current screenshots show the system accent `#3A87DB`-ish on "Create a new wallet", "Send",
   sidebar selection).

---

## 3. Component inventory

### 3.0 What SwiftCrossUI 0.10.0 can draw (pinned `0f3ec39`)

Available: `VStack/HStack/ZStack`, `Text` (font, colour, line limit, selection), `Image` from PNG/JPG/WebP
(`resizable`), shapes (`RoundedRectangle`, `Capsule`, `Circle`) with `fill`, `background`, `overlay`,
`cornerRadius`, gradients, `ScrollView`, `List` (selection), `Table`, `NavigationSplitView`, `Button`,
`Toggle` (switch/checkbox/button styles), `TextField`, `SecureField`, `TextEditor`, `Picker` (menu,
segmented, radio, inline), `Slider`, `ProgressView` (spinner + bar), `Menu`, `.sheet`, `.alert`,
`.onHover`, `.onTapGesture`, `.help`, `.focusable`, `GeometryReader`, `Color.opacity`.
**Not available:** shadows, view opacity/scale effects, `contextMenu`, keyboard shortcuts on buttons
(menu accelerators only), accessibility modifiers, line height, continuous corners, SVG, charts.
Fallback rules used below: shadow → hairline border; context menu → an overflow `Menu` button ("More")
in the row/selection toolbar; press/hover scale → background tint change on hover; charts → stacked
horizontal progress bars; icon-only button → icon + short text.

### 3.1 Inventory (iOS component → desktop component)

Names are the same in `DashUIMac` and `DashUICross`. "Port" = vendored DashUIKit component used as is;
"New" = desktop component in `DashUIMac/Desktop` / `DashUICross`.

| # | Desktop component | iOS / DashUIKit source | Anatomy | States | Sizes | MacUI | CrossUI |
|---|---|---|---|---|---|---|---|
| C1 | `AppSidebar` | tab bar (`MainTabbarController`) | sections → rows: 16 pt symbol + `subhead` title; optional count badge | normal, hover, selected (accent pill, white text), disabled-hidden | width 200–280 | `List(selection:)` `.listStyle(.sidebar)` + `.tint(.dash.blue)`; `Label(title, systemImage:)` | `List` with custom row: PNG icon + text; selected = `accent` pill r8 + white text; hover = `accentTint` |
| C2 | `MainToolbar` | iOS nav bar | leading: none; trailing: wallet picker (≥2 wallets), discreet toggle, lock | — | system | `ToolbarItemGroup`; **no network/demo badges** (§4.1) | header row above the page (icon+text buttons) |
| C3 | `WalletStatusBar` | dash-qt status bar | left: sync text + 120 pt progress bar; right: unit menu, HD, lock, proxy, peers, gov clock, sync | syncing / synced / stalled / offline | 28 | `DashUIMac/Desktop/StatusBar` restyled: `caption1`, 14 pt symbols, tooltips verbatim from research 02 §2.3 | text items only (A1), same order; network/demo text **removed** (shown elsewhere) |
| C4 | `BalanceHero` | `HomeBalanceView` | network capsule · syncing caption · amount (`largeTitle`, glyph ×0.7) · fiat (`subhead`) · "Known balance"/hint · breakdown strip | shown, hidden (discreet), syncing, unknown ("—" + "Balance unavailable"), partial | band 236 / 180 | New (replaces `BalanceHeader`) | same; pulse → static caption |
| C5 | `BalanceBreakdownStrip` | `HomeBalanceView.breakdownCard` | cells: icon? + title `footnoteMedium` + sub `caption2` @70% + amount right (`footnote` medium) separated by `heroDivider` | per-cell syncing spinner, unavailable "—" | max 600 wide; cells equal | New | same (no translucency issue: `Color.opacity` exists) |
| C6 | `ShortcutCard` / `ShortcutItem` | `ShortcutsBarView`, `ShortcutItemView` | one card r20, padding 4, 4 equal items; item = 46 pt icon + `caption2` semibold 2 lines | normal, hover (tint), pressed (0.93), disabled (40 %) | card ≤ 520 wide, straddles the hero edge | New (replace 4 separate cards in `HomeShortcuts.swift`) | items as `Button` with image+text, hover tint |
| C7 | `HistoryHeader` | `SyncingHeaderView` | "History" `subhead` secondary · spacer · "Syncing 47.0%" (button → sync overlay) · "Filter" plain blue small button + filter icon | synced / syncing | full content width | New | same |
| C8 | `TransactionGroupCard` | `HomeView` section | header (day `footnoteMedium` left, weekday `footnote` tertiary right, height 38) + rows; card r10, `card` fill, shadow L1 | — | content width | New | border instead of shadow |
| C9 | `TransactionRow` | DashUIKit `TransactionView` (port) | 30 pt icon (+14 pt badge), title `footnoteMedium`, subtitle time `footnote` secondary + detail chip (`caption1Medium`, `blueText` on `blueAlpha10`, r7), trailing: status text (orange `caption1Medium`) + `AmountText` + fiat `footnote` | normal, hover (`accentTint` bg), selected, pending (chip "Pending"), conflicted/abandoned (title secondary + chip `danger`) | min height 62 | **use the vendored `TransactionView`** (exists, unused) | port in `DashUICross/TransactionView.swift`: replace +/- tiles by tx icons |
| C10 | `AmountText` | DashUIKit `DashAmount`/`DashBalanceView` | [sign][number][2 pt][glyph or " unit"] | normal, masked (`#`), unknown ("—") | `size`, `weight`, `glyphFactor` | New: takes a **formatted string** + `AmountUnitDisplay` (glyph/name) (§5) | same; glyph = PNG |
| C11 | `MenuCard` + `MenuRow` | DashUIKit `MenuViewModifier` + iOS app `MenuItem` | card r20 inner 6; row: 30 pt icon, title `subheadMedium`, help `footnote` tertiary, trailing accessory (chevron, toggle, value text, button, amount, badge); rows separated by spacing 2 (no lines) | normal, hover, disabled (secondary text, dimmed icon), destructive (title `danger`) | row min 56 | port `MenuItem` + `MenuViewModifier`; extend accessories: `.chevron`, `.picker`, `.stepper`, `.badge` | `DashUICross/Containers.swift` `MenuItem` gains icon + accessory enum |
| C12 | `DetailList` / `DetailRow` / `CopyRow` | DashUIKit `List1View`, iOS `MasternodeCopyRow`, `RequestAmountScreen.addressRow` | label `footnote` secondary over/left of value `subhead` primary (selectable, wraps); `CopyRow` adds a tintedGray `copy` icon button (medium) | normal, copied (toast) | two layouts: stacked (narrow) / label-left 160 pt (wide) | New | stacked only |
| C13 | `TopIntro` | DashUIKit `TopIntroView` | title `title2` + up to two description lines `subhead` secondary | — | max width `contentMaxWidth`, trailing padding 60 | port | same |
| C14 | `DashButton` | DashUIKit `DashButton` (port) | 11 styles × 4 sizes (`DashButtonMetrics`), leading/trailing icon, loading | enabled, hover (+4 % overlay), pressed, disabled, loading | L/M/S/XS | port; **replace every `.borderedProminent`/`.bordered`/plain `Button` in screen code** (except toolbar/menus/tables) | exists; add hover tint |
| C15 | `AmountEntry` | `EnterAmountView` (dual-swap) + `NumericKeyboardView` | large centred amount (`title1`) with glyph, secondary line (fiat or the other unit), swap button ⇅, Max button (tintedBlue S), unit picker | empty ("0"), editing, invalid (red secondary line with dash-qt error), max, disabled | width ≤ 520 | New, keyboard input (no on-screen keypad on desktop) | text field + label row (no swap animation) |
| C16 | `AddressField` | DashUIKit `AddressFieldView` (port) | caption, multi-line field r16 on `fieldFill`, trailing actions: address book, paste, (scan QR file); error text | empty, focused (border), filled-blurred (read-out), error (red tint + text), disabled | width form | port (exists) — use it in Send, Sign/Verify, Wizards | `DashTextField` + icon+text buttons |
| C17 | `QRCard` | `RequestAmountScreen.qrCode` | QR 200 pt in 10 pt well on white, optional centre badge (§4.8), address `CopyRow` below, actions row | loading (spinner same square), no address, URI too long (error text) | 220 square | `DashUIMac/Desktop/QRView` restyle | `QRCodeView` |
| C18 | `SearchBar` | DashUIKit `SearchBar` (port) | magnifier, field r14 `fieldFill`, clear | empty, focused, filled | 32 h | port | `DashTextField` with leading icon |
| C19 | `SegmentedControl` | iOS `SegmentedControl` / `segmentControl*` tokens | capsule group `segmentControlBackgroundGroup`, selected pill `segmentControlBackground` | — | 32 h | custom (not system `.segmented`, which draws grey macOS segments) | `Picker(.segmented)` (GTK look accepted) |
| C20 | `Badge` / `StatusChip` / `NetworkCapsule` | iOS network badge, `TransactionView` detail chip | text `caption1Medium`, r7 tinted bg; capsule = `caption2` bold tracking 1.2, white on `orange` @90% | tones: info(blue), success, warning, danger, neutral(gray300Alpha20) | 18–20 h | `Desktop/Badge.swift` restyle + `NetworkCapsule` | `DashBadge` |
| C21 | `Toast` | DashUIKit `Toast` (port) | blurred dark pill, icon, message, optional ✕, optional action | warning, info, error, success, copied, loading, noInternet | auto-dismiss 3 s (copied 1.5 s); bottom-centre of the window, 24 pt above the status bar | port + `ToastHost` modifier (queue) | `Toast` exists (inline banner); add bottom overlay host |
| C22 | `SystemMessage` (banner) | DashUIKit `SystemMessageView` | icon, title, subtitle, ≤2 buttons, ✕ | info, warning, error | content width | port; use for backup reminder, stalled sync, launch error, disabled CoinJoin | inline card |
| C23 | `SheetScaffold` | DashUIKit `BottomSheet` (port) | header 64: back · centred title `subheadMedium` · close ✕ (44); content; footer action bar (Cancel tintedGray L + primary filledBlue L, equal widths) | dismissible / locked (signing, broadcasting) | `sheetWidth` | port (exists as macOS sheet chrome) — use for **every** sheet | `.sheet` + same header row (text buttons "Back"/"Close") |
| C24 | `ResultView` | `SuccessIllustration`, `ErrorIllustration`, `SuccessTxDetailViewController` | 90 pt illustration, title `title2`, amount `AmountText` title1, detail rows, buttons | success, error, unknown outcome (warning illustration) | sheet medium | port illustrations | PNG illustrations (export `illustration-*`), same layout |
| C25 | `EmptyState` | iOS "There are no transactions to display" | 60 pt icon (tinted `textTertiary`), title `headline`, message `subhead` secondary, optional action | — | centred in card | New | same |
| C26 | `LoadingState` | `LoadingSpinner`, "Loading transactions" row | spinner + `footnote` secondary text in a row | inline / full-page | — | port `LoadingSpinner` | `ProgressView` |
| C27 | `ProgressBar` / `ProgressRing` | sync progress, CoinJoin completion | bar 4 pt r2 `accent` on `progressBackgroundColor`; ring 6 pt stroke | determinate / indeterminate | — | New | bar = `ProgressView(value:)`; ring → bar fallback |
| C28 | `DataTable` (technical) | dash-qt tables | header `footnoteMedium` secondary on `cardRaised`, rows 28 `footnote`, zebra off, separators `separator`, sort indicator, column resize, selection `accentTint` | sorting, selected, empty (EmptyState inside) | full width | `Desktop/DataTable.swift` restyle (monospacedDigit, not monospaced) | `Table` |
| C29 | `WizardHeader` | dash-qt "Step %1 of %2 · %3" | step text `footnote` secondary, title `title3`, segmented progress (n dots/bar) | — | wizard sheet | New | text + bar |
| C30 | `PhraseGrid` / `PhraseChip` | `DWSeedPhraseView` / `DWSeedWordView` | preview: one white card, words in 3 (12 words) or 4 (24) columns, index `caption1` tertiary + word `calloutMedium` `blueText`; verify: blue filled chips (white text) to click in order, used chips grey (`disabledButtonColor`), wrong click flashes `red` | preview, verify, error | card ≤ 640 | New (replace the current per-word white tiles) | same (no flash animation: red border for 1 s) |
| C31 | `PassphraseField` | dash-qt passphrase + iOS PIN field | secure field, reveal button, strength/caps-lock hint | — | form | `Desktop/PassphraseField.swift` restyle | `DashSecureField` |
| C32 | `KeyValueGrid` (technical facts) | dash-qt Information tab | two-column grid label `footnote` secondary / value `footnote` primary selectable | — | — | New | `KeyValueRow` exists |
| C33 | context menus | iOS long-press | dash-qt order (research 02 §4.5 etc.) | — | — | `.contextMenu` | overflow `Menu` "More" button |

### 3.2 Interaction rules shared by all components

- **Hover**: rows and items get `accentTint` (selected rows keep it stronger: `blueAlpha20`); buttons
  darken 4 %. Cursor stays the arrow (no pointing hand except on links).
- **Focus**: keyboard focus ring = `accent` 2 pt, offset 2, radius = component radius (macOS default
  focus ring with `.tint` gives this). Full keyboard access: Tab order follows reading order; Return =
  default action; Escape = cancel/close sheet; ⌘C copies the selected row's primary value; arrow keys
  move list selection; Space toggles.
- **Copy feedback**: every copy shows `Toast(.copied, "Copied")` (iOS string) — never silent.
- **Destructive actions**: `filledRed` button only inside a confirmation; the trigger is `plainRed`.
- **Disabled**: explain why with a tooltip (`.help`) — e.g. "Coin Control Features…" disabled → tooltip
  "Enable coin control in Options ▸ Wallet".

---

## 4. Screens

Conventions: sketches are at ~1:10; `[Btn]` = DashButton (style noted), `(…)` = text field,
`‹card›` = `MenuCard`/card; every screen exists in light and dark and must be screenshotted in both.
Copy rule: dash-qt strings verbatim where dash-qt has the feature (research 02); iOS strings where the
element comes from iOS; new strings only where neither has one (mark them in L10n with a comment `new`).
§4.25 (masternodes and governance) follows its own rule, stated there (MG-00 D15, D29).

### 4.1 Shell: main window, sidebar, toolbar, status bar, title, menus

```
┌● ● ●  Dash Wallet - Main - [testnet]                       [wallet ▾] [👁] [🔒]┐
├────────────┬───────────────────────────────────────────────────────────────────┤
│ ⌂ Overview │                                                                   │
│ ↗ Send     │                     page (canvas, centred column)                 │
│ ↙ Receive  │                                                                   │
│ ☰ Transact.│                                                                   │
│ ⤨ CoinJoin*│                                                                   │
│ ▤ Mastern.*│                                                                   │
│            │                                                                   │
├────────────┴───────────────────────────────────────────────────────────────────┤
│ Up to date ▬▬▬▬            tDASH ▾   HD   🔒   ⇄ proxy   ▮▮▮ 8   vote 1d ✓     │
└────────────────────────────────────────────────────────────────────────────────┘
```
- **Window title**: dash-qt's (`MainViewModel.windowTitle`, QT-003/QT-011) — kept; it is the *only* place
  "[testnet]"/"[demo]" text appears in the chrome. The toolbar must **not** repeat it as badges (today:
  title + "Demo" + "Testnet" toolbar badges + a third "Testnet" badge on the balance card).
- **Network marker**: the orange `NetworkCapsule` in the hero (Overview) and in the lock screen; elsewhere
  the title tag and the status-bar unit (`tDASH`) carry it.
- **Demo mode**: one neutral `Badge("Demo")` at the left of the status bar with the tooltip
  `MacStrings.App.demoHelp`; not in the toolbar, not a sentence in the status bar (Cross today shows
  "Demo mode: sample data, nothing is sent (passphrase: demo)").
- **Sidebar**: §C1. Shortcut tooltips "(⌘1)" renumbered by visible items (QT-012, exists). Masternodes is one item
  with the tabs My masternodes and Governance, shown under the rules of §4.25.1 (MG-00 D1–D3; there is no
  separate Governance item). Cross only:
  a second section "More" with Address Book, Sign / Verify, Tools, PSBT, Wallets, Options, Security,
  About as normal rows (same style, PNG `settings-*` icons), replacing today's bold blue link buttons
  (`CrossUI/WalletRootView.swift` `toolButton`).
- **Toolbar** (macOS): wallet picker (QT-014), discreet toggle (`eye`/`eye.slash`), lock button. Nothing
  else. The lock button and the menu's "Lock wallet" stay enabled in the "key held" state: they run Lock, which
  revokes the flow's lease although the vault is already locked.
- **Status bar**: §C3. Left: sync text (dash-qt strings) + thin `accent` progress bar while syncing
  (click → sync overlay). Right, in dash-qt order: unit selector, HD (green `checkmark.shield` /
  hidden), lock (`lock.fill` green = locked, `lock.open.fill` red = unlocked/unencrypted, orange =
  mixing only; **key held**: the vault's own icon with a badge while a flow holds its own key, that is an orange
  `lock.fill` with a badge on a locked vault and the mixing-only icon with a badge on a mixing-only one. Tooltip
  "Registration of @name holds a key for 4:12 — Lock to cancel" before the funds are committed, "… — Lock to stop"
  after; E0-04 design §16.10 C7), proxy, connections (`antenna.radiowaves.left.and.right` + count; red when disabled),
  governance item (§4.25.1), sync (`checkmark.circle.fill` green / `arrow.triangle.2.circlepath` orange
  spinning). Tooltips verbatim (research 02 §2.3). Cross: the same items as short text ("tDASH", "HD",
  "Locked", "8 peers", "Synced"); in the key-held state "Locked · key held" or "Mixing only · key held".
- **App menus**: dash-qt menus exactly (QT-015…018, `WalletCommands.swift`, `CrossUI/ShellMenus.swift`);
  M3 adds Help ▸ "CoinJoin information". Masternode keys and governance numbers open from the Masternodes
  page's More menus (§4.25.1, MG-00 D7, D20), not from the menu bar.

### 4.2 Splash, data directory, startup error, shutdown

```
            [Dash wordmark, blue, 32 h]   (no separate "Dash Wallet" title; the wordmark carries it)
        ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬ (accent bar, 240 w)
              Loading wallet…  (subhead secondary)
              Press Q to quit  (footnote tertiary)
```
- Replace the SF Symbol `d.circle.fill` (`MacUI/Shell/ShellViews.swift:69`, `:269`) by `brand-dash-logo` (testnet:
  `brand-dash-logo-testnet`); progress bar in `accent` (today grey).
- Data directory chooser (QT intro): `TopIntro` + `MenuCard` with the two radio rows (`RadioButtonRow`)
  "Use the default data directory" / "Use a custom data directory:" + path field + "…" button; strings
  from research 02 §1.1.
- Shutdown: same layout as splash, text "Shutting down…" (dash-qt "Dash Core is shutting down…" adapted
  in L10n), no progress bar (indeterminate spinner).
- Settings unreadable (m2 `settings-unreadable`): `SystemMessage(.error)` card + buttons (dash-qt strings).

### 4.3 Onboarding (create / restore / verify / passphrase)

Canvas background, centred column 560 wide, top-aligned at 15 % of the window height.
```
Welcome:            [Dash wordmark]                               
                    Welcome to Dash Wallet (title1)
                    A wallet for Dash on your desktop. (subhead secondary)
                    [ Create a new wallet ]   filledBlue L fillsWidth
                    [ Restore wallet      ]   tintedGray L fillsWidth
                    Advanced options ▸  (plainBlue S; reveals network + phrase length in a MenuCard)
Phrase:   ‹ Back    Your recovery phrase (title2) + warning text (subhead secondary)
                    ‹PhraseGrid card: 1 galaxy  2 rocket  3 velvet …›
                    ☐ I have written down my recovery phrase   (checkbox row, iOS)
                    [ Continue ] filledBlue, disabled until checked
Verify:             Tap the words in the correct order (iOS DWVerifySeedPhrase)
                    ‹slots card: 1 ___  2 ___ …›   ‹chips: [rocket][maple]…›
Passphrase:         Encrypt wallet: PassphraseField ×2 + strength + dash-qt warning text
Done:               SuccessIllustration · "Your wallet is ready" · [Open wallet]
Restore:            AddressField-style multi-line phrase box with per-word validation chips,
                    birth date/height row, [Restore] filledBlue
```
- Network selection and phrase length move into "Advanced options" (default mainnet / 12 words) — the
  welcome screen of iOS has no network picker; dash-qt's is in Options. Keep the controls, demote them.
- Back button = `NavigationBarElement.back` style (chevron + "Back", plainBlue), top-left of the column.
- Phrase words: `PhraseGrid` (C30), not 12 separate white tiles. Words are selectable/copy-protected per
  the existing screen-capture guard.
- Errors (wrong word order, invalid phrase): red text under the grid + chips flash; never an alert.

### 4.4 Lock screen, unlock sheet, mixing-only unlock

Full-window overlay on `hero` blue (iOS lock screen: white logo, white text, actions on the dark/blue
background; we use the hero blue instead of iOS's photo background `image_bg`).
```
                         [Dash wordmark, white]
                         TESTNET (capsule, non-mainnet only)
                    Unlock wallet (title2 white)
         This operation needs your wallet passphrase… (subhead white 80%)
                 (•••••••••••••••)  white field, r16, 360 w
                 ☐ For mixing only   (white checkbox + label)
          [ Unlock ] filledWhiteBlue L        [Touch ID] tintedWhite (when enabled)
     ── Quick Receive ──  [↙ Quick Receive] tintedWhite M   [⌗ Scan to Send] tintedWhite M
```
- dash-qt passphrase strings (research 02 §12.3); iOS button names "Quick Receive", "Scan to Send".
- Wrong passphrase: field shakes (macOS; Cross: red border) + white error text; lockout countdown text.
- `UnlockSheet` (operation-triggered unlock while the window is open) = `SheetScaffold` small on `card`
  with the same fields; title from the operation ("Unlock wallet for mixing only" for CoinJoin).

### 4.5 Overview (Home) — reference: `docs/design/ux/home-{light,dark}.png`

```
┌──────────────────────── hero (blue band, full width of detail column) ───────────────────────┐
│                                   TESTNET                                                   │
│                              Syncing balance  (only while syncing)                          │
│                             30.39698287 Ð      (largeTitle white, glyph ×0.7)               │
│                                US$ 912.31      (subhead white, only with a rate source)     │
│          ┌ Available  Spendable now      30.19698287 Ð │ Pending  Awaiting conf.   0.20 Ð ┐ │
│          └──────────────────────── breakdown strip (white 12 %) ──────────────────────────┘ │
│                    ┌────────── shortcut card (white, r20, straddles the edge) ──────────┐    │
└────────────────────│   (↓)Receive     (↑)Send     [⌗]Scan QR     (⟳)Back up              │────┘
                     └─────────────────────────────────────────────────────────────────────┘
          [backup reminder SystemMessage, if any]     [CoinJoin card, when enabled §4.21]
          History                                         Syncing 47.0%   Filter ⚲
          ┌ Today                                                         Tuesday ┐
          │ (↓) Received            01:40 [Pending]                       +0.20 Ð │
          │ (↓) Coffee refund       01:10 [InstantSend]                   +1.25 Ð │
          └───────────────────────────────────────────────────────────────────────┘
          ┌ Yesterday                                                      Monday ┐ …
          (Loading more transactions…)
```
- **Hero** (C4): one total (dash-qt Total), floored to the Decimal digits setting (QT-036). Breakdown cells
  = dash-qt rows Available, Pending, Immature (only when non-zero) (QT-034), each with a one-line
  explanation (`caption2`, new strings: "Spendable now", "Awaiting confirmation", "Mining/masternode
  rewards maturing") — click opens a small explainer popover (iOS `BalanceInfoSheet` pattern). M4 adds
  Platform/Shielded cells like iOS.
- **Out of sync** (QT-037): the "Syncing balance" caption becomes "(out of sync)" in `caution` with the
  dash-qt tooltip; the capsule row keeps its place so the layout never jumps.
- **Discreet** (QT-039): amount shown masked (`###.##`, from the VM) with an `eye.slash` circle button
  (58 pt, white on black 20 %) to its left; breakdown amounts masked; History replaced by the
  `EmptyState` "Recent transactions are hidden in discreet mode" (existing string) — rows are not
  rendered. Click on the hero amount toggles (iOS tap-to-hide); one-time hint "Click to hide balance"
  (`caption1` white 50 %, iOS "Tap to hide balance" adapted). Right-click hero: "Hide balance",
  "Display unit ▸" (QT-020), "Copy balance".
- **Shortcut card** (C6): the VM's 4 slots (`ShortcutBarViewModel`); icons from `shortcut-bar-*`;
  right-click a slot → "Replace with ▸ …" (iOS long-press customise).
- **History**: `HistoryHeader` + `TransactionGroupCard`s (C8) from the VM's recent list grouped by day
  ("Today", "Yesterday", "October 4, 2026" + weekday). Overview shows the last N (dash-qt recent count) and a
  "See all transactions" plainBlue button under the last card → Transactions page. Click a row →
  transaction detail sheet (iOS) — *and* select it on the Transactions page (QT-038) when opened from
  there.
- Empty wallet: `EmptyState` (`tx-all` icon, "There are no transactions to display" (iOS), action
  [Receive Dash] filledBlue M).
- Loading: `LoadingState` "Loading transactions" (iOS) in place of the cards.
- Node warnings (QT-040, e.g. prerelease/alert text): `SystemMessage(.warning)` at the top of the page,
  under the hero.
- Errors / stalled: `SystemMessage` above History ("No connection to the Dash network" + [Change peers]
  after 45 s stall, IOS-023 strings).
- Width: content column 760 centred; the hero band spans the column's full width; at window widths
  < 980 the breakdown strip stacks its cells vertically (iOS layout).

### 4.6 Sync overlay and sync details

- Overlay = centred `card` r20 on `overlay` scrim (today: grey `#888` scrim). Title `title2`, dash-qt
  text (research 02 §2.5), then a `DetailList` (label-left) with Status, Number of blocks left, Last
  block time, Progress (`ProgressBar` + %), Progress increase per hour, Estimated time left; [Hide]
  tintedGray M. iOS per-phase rows (headers / filter headers / filters / masternode lists, IOS-023) as a
  second card "Sync phases" with a `ProgressBar` per phase and peers count.
- Sync details (`Main/SyncDetails.swift`, peers table) is a technical view (DataTable).

### 4.7 Send, coin control in Send, confirm, result

```
TopIntro  Send  /  "Send Dash to a Dash address" (new)
‹card Recipient 1                                                    [✕ remove]›
   Pay To      (AddressField  yAb3Cd4Ef5Gh…  [📖 Address book][⎘ Paste])
   Label       (field)
   Amount      AmountEntry  0.25 Ð  [⇅ unit]  [Max]      ☐ Subtract fee from amount
[+ Add Recipient] tintedBlue M
‹card Transaction Fee  ( Recommended | Custom )  segmented
   Confirmation time target  [15 minutes (6 blocks) ▾]   or custom fee field›
‹card Coin control (only when enabled): Inputs… [Choose…] · Custom change address›
────────────────────────────────────────── sticky footer bar (card, top hairline)
 Balance: 30.19 Ð                [Clear All] tintedGray M   [Send] filledBlue L
```
- Column `formMaxWidth` 640, centred. Footer bar spans the column; "Coin Control Features…" lives in the
  Coin control card, not the footer.
- Address field: proportional font (today `design: .monospaced`, `Send/SendView.swift:238`); validation
  inline (red tint + dash-qt error string).
- **Confirm sheet** (C23, width 480) — iOS `ConfirmPaymentSheet` layout carrying dash-qt content
  (research 02 §5.4):
  ```
           Confirm send coins            (header title)       ✕
                0.25000374 Ð            (title1, total, full precision)
              Do you want to create this transaction?  (subhead secondary)
  ‹menu card›  Pay to       Alice · yAb3Cd4Ef5Gh…Uv3Wx4   (one row per recipient, ≤10, then "(%1 of %2 entries displayed)")
               Using        any available funds | CoinJoin funds only
               Network fee  0.00000374 Ð
               Size         0.374 kB · 0.00001000 Ð/kB
               Total        0.25000374 Ð   (= 250.00374 mDASH …)
  [ Cancel ] tintedGray L (default, Return)      [ Send (3) ] filledBlue L (enabled after 3 s)
  ```
  PSBT variant adds [Create Unsigned] tintedBlue L. Amounts at full precision (iOS uses 8 digits here for
  the same reason: a confirm that names a rounded fee is wrong).
- **Result**: iOS shows the new transaction's detail screen (`SuccessTxDetailViewController`, in
  `Tx/Details/TxDetailViewController.swift`) with a [Close] button. Desktop: `ResultView` success = green
  illustration + the transaction's state title + amount + the detail rows of §4.9, buttons [Close]
  filledBlue (iOS) and [Show in Transactions] tintedBlue. Failure: error illustration + dash-qt error
  string. Unknown broadcast outcome: warning illustration + the existing `MacStrings.Send.outcomeUnknown*`
  copy.
- dash-qt jumps to Transactions with the new transaction selected after commit (research 02 §5.4): do it
  when the result sheet closes (either button).

### 4.8 Receive and Request payment

```
TopIntro  Receive
┌ ‹card, 360 w›                    ┐   ┌ ‹card› Request payment                           ┐
│        [QR 200, Dash badge]       │   │ Use this form to request payments. All fields   │
│  Address                          │   │ are optional.                                   │
│  yQkAZdnn922YucpQ9DZedU1uFY4… [⎘] │   │ Label    (Enter a label to associate …)         │
│ [⎘ Copy URI] [↻ New address]      │   │ Amount   AmountEntry (compact)                  │
└───────────────────────────────────┘   │ Message  (…)                                    │
                                        │ [Clear] tintedGray M  [Create new receiving     │
                                        │                         address] filledBlue M   │
                                        └─────────────────────────────────────────────────┘
Requested payments history  (headline)
‹DataTable: Date | Label | Message | Requested (Ð)›  + [Show] [Remove] row actions
```
- Two columns ≥ 980 pt wide, stacked below. Address in `CopyRow` (proportional, wraps on two lines, no
  truncation in the receive card — it is the verification surface).
- Button labels must never truncate (today "Copy Addr…", "New Addre…"): the card is wide enough, or
  the buttons go icon+text on a second row.
- QR centre badge (`dash-logo-square`, 18 % of the side) **only if** the QR is generated with ECC level
  ≥ Q; dash-qt uses level L with a 255-char URI cap (research 02 §6), where a badge would break scanning.
  Until the engine QR call takes an ECC level, no badge.
- "Request payment to …" dialog = `SheetScaffold` medium with `QRCard` + `DetailList` (URI, Address,
  Amount, Label, Message, Wallet — each hidden when empty) + [Copy URI] [Copy Address] [Save Image…].
- Watching for a payment (iOS live watcher) is M5; not in scope now.

### 4.9 Transactions (list, table, filters, detail)

```
TopIntro Transactions                                        [List | Table] (segmented) [Export…]
Filter bar: ( Search address, transaction id or label ) [Date: All ▾] [Type: All ▾] (Min amount)
            chips: All · Sent · Received · Mixing · Rewards     (iOS filter categories, dash-qt types)
List mode:  HistoryHeader-less TransactionGroupCards (as Overview)
Table mode: DataTable — status icon | Date | Type | Address/Label | Amount (tDASH)   (dash-qt §4.3)
Footer:     "25 transactions"  (footnote secondary)
```
- **List mode is the default** (iOS). Row title rule (§5.4): address-book label → metadata → type title
  ("Received", "Sent", "Sent to yourself", "Mixing", "Masternode reward", "Mined"…). Never the raw
  address as the title; the address is in the detail sheet and table mode.
- Row status: chip in the subtitle ("Pending", "InstantSend", "Unconfirmed", "Abandoned", "Conflicted",
  "Immature"…); ChainLocked/confirmed rows have no chip (quiet when fine). Locked coinbase →
  orange "Locked" trailing status (iOS).
- Table mode keeps dash-qt columns, sorting, context menu (research 02 §4.5) and status icons
  (`clock` unconfirmed, `bolt.fill` InstantSend, `lock.fill` ChainLocked in `success`).
- **Detail sheet** (C23 medium 640, iOS `TxDetailViewController` + dash-qt §4.6 content):
  ```
              (tx-detail icon 50)  Received          (title3)
                       +0.20 Ð      (title1)   fiat (subhead secondary)
              [Pending] status chip           Oct 6, 2026 at 01:39
  ‹card› From        yNBLvUC…eusVmKavE        ⎘
         To          ye3zmge…ZgfJkQ4 (own address) ⎘
         Network fee 0.00000226 Ð
         Status      0/unconfirmed, in memory pool
         Transaction ID 57143a02…a943469a4 ⎘   (monospaced: technical value)
         Size        192 bytes
  ‹card› Label  (field)  [Save]
  ‹card› Actions: [Abandon transaction] plainRed  [Resend] tintedBlue  [Open in Insight ↗] plainBlue
  ▸ Inputs (n) / ▸ Outputs (n) / ▸ Raw transaction   (disclosure groups, technical)
  ```
  Disabled actions hidden when they cannot apply (not shown greyed as today), except where dash-qt shows
  them disabled — then with a tooltip.

### 4.10 Address book (window on macOS, page on Cross)

- `TopIntro` with the dash-qt explanatory text for Sending/Receiving; `SegmentedControl` Sending |
  Receiving; `SearchBar`; list in a `MenuCard`: rows = 30 pt initial avatar (blue circle, white initial,
  iOS contact avatar) + label `subheadMedium` + address `footnote` secondary middle-truncated; hover →
  [⎘] [✎] buttons; right-click = dash-qt context menu (Copy Address, Copy Label, Edit, Delete, Show QR…).
- Toolbar row: [+ New] tintedBlue M, [Export…] plainBlue; Copy/Edit/Delete move to row hover/context
  (today a disabled button bar).
- Empty: `EmptyState` "No addresses yet".
- Edit dialog: `SheetScaffold` small with Label + AddressField.

### 4.11 Sign / Verify message

- `SegmentedControl` Sign Message | Verify Message (today a system `TabView`); dash-qt warning text in a
  `SystemMessage(.warning)`; `AddressField` with Address book / Paste actions; message `TextEditor` in a
  card (r16, `fieldFill`); signature as a technical value (monospaced) in a `CopyRow`; buttons [Sign
  Message] filledBlue M, [Clear All] tintedGray M; result line in `success`/`danger` with dash-qt strings.

### 4.12 Settings (macOS Settings scene) and Options

- One **Settings window** with the macOS toolbar-tab style (icons + labels): General (= dash-qt Main),
  Wallet, CoinJoin (M3), Network, Display, Appearance, Notifications, Security. Each tab = scrollable
  page of `MenuCard`s with `MenuRow`s (icon 30, title, help text, accessory). Bottom bar (dash-qt):
  [Reset Options] plainRed, [Cancel] tintedGray, [OK] filledBlue.
  Today: Options uses `Form { }.formStyle(.grouped)` (system grey, no icons) and Settings is a separate
  two-row form floating under a large empty band (`Settings/SettingsView.swift`).
- Merge today's "Settings" (network + menu bar) into General; the iOS "More" menu items that are not in
  dash-qt (Local currency, Notifications) live in General/Notifications.
- Row types: toggle (`switchOn` blue), picker (value text + chevron → menu), stepper (value + −/+),
  numeric field (right-aligned), info (value text secondary), action (chevron).
- "Not applicable to this wallet (SPV)" list (m2 linux options): `SystemMessage(.info)` at the bottom of
  the affected tab, listing the options, instead of a free-floating card.
- Cross: Options is a page with `SegmentedControl` tabs (today: blue link-text tabs).

### 4.13 Security

`TopIntro` "Security" + menu cards, iOS `SecurityMenuScreen` order adapted:
```
‹card› (🔑) Wallet encryption           Encrypted (value)
       (🔄) Change passphrase…           ›
       (📄) Show recovery phrase…         ›
‹card› (☝) Touch ID                      toggle | "Not available on this Mac" (help text)
       (⏱) Auto lock                     5 minutes ▾   (= Lock, armed only while the vault holds its key; a
                                                          flow's progress screen is not activity; E0-04 §4.8, C5)
       (✓) Require authentication for every payment   toggle
       (👁) Autohide balance              toggle
‹card› (?) Forgot passphrase?            ›
       (🗑) Wipe wallet                   ›   (title in danger)
```
Icons: `settings-security`, `settings-pin`, `settings-recovery-phrase`, `settings-biometrics`,
`settings-advanced-security`, `settings-spending-confirmation`, `settings-autohide-balance`,
`settings-reset-wallet`. Today: system grouped form with raw buttons inside rows.

### 4.14 Coin control (technical view)

- `SheetScaffold` large (900 × 640, resizable). Summary strip on `cardRaised`: Quantity, Bytes, Amount,
  Fee, After Fee, Change in a `KeyValueGrid` (tabular digits, proportional font; today monospaced
  amounts in the list column).
- Toolbar: [(un)select all] [(un)lock all] tintedGray S, "(n locked)", `SegmentedControl` Tree | List
  (today radio buttons), [Show all coins].
- `DataTable` columns per dash-qt (Amount, Label, Address, Date, Confirmations, Mixing Rounds (M3));
  checkbox column; lock glyph; addresses middle-truncated.
- Footer [OK] filledBlue.

### 4.15 PSBT operations

- `SheetScaffold` medium; empty state "Load a PSBT" with [Load from file…] [Paste from clipboard];
  loaded: `SystemMessage` with analysis status, `DetailList` (inputs/outputs/fee), technical
  monospaced base64 box (collapsed by default), actions [Sign] filledBlue, [Broadcast], [Copy], [Save…].
  Errors (m2 `psbt-load-error`): `SystemMessage(.error)` with dash-qt text.

### 4.16 Tools window (Information, Console, Network Traffic, Peers, Repair)

Technical views, restyled not redesigned:
- `SegmentedControl` tabs centred in the toolbar (exists).
- Information: sections as cards (`headline` titles) with `KeyValueGrid`; unknown values "—" + "Requires
  full-node data source" help text (exists — keep). Hashes/paths selectable; hashes monospaced.
- Console: monospaced output on `card`, input field at the bottom, dash-qt welcome text; colours: commands
  `blueText`, errors `danger`.
- Peers / banned: `DataTable`; detail pane on the right as `DetailList`.
- Repair: menu-card rows with explanations and [Rescan…] etc. buttons.
- Network Traffic: chart when U2 lands; until then the honest empty state (exists).

### 4.17 Wallets

- List in a `MenuCard`: row = `settings-wallet` icon, name `subheadMedium`, id `footnote` tertiary
  middle-truncated (proportional; today monospaced), status chip ("Open" success / "Closed" neutral),
  trailing "Open at startup" toggle; selection `accentTint` (today grey fill).
- Actions in a header row: [Add Wallet ▾] filledBlue M, [Open] [Close] tintedGray M, [Wallet ▾] menu
  (backup, encrypt…), right: [Show Automatic Backups] plainBlue, [Close All] plainRed.

### 4.18 About, command-line options

- About: `brand-dash-logo`, version `subhead`, dash-qt licence text in a scrollable card, links
  plainBlue. Command-line options: monospaced technical text in a card.

### 4.19 Menu bar companion (macOS `MenuBarExtra`, IOS-117)

```
 Dash Wallet                       TESTNET
 30.39698287 Ð        (title3 bold, AmountText; masked in discreet)
 Up to date           (footnote secondary)
 Last transaction  (↓) Received  +0.20 Ð   (TransactionRow compact)
 ─────
 [QR 140]  yc1HqG…7TA53s ⎘
 (Request amount)  [Copy Request]
 ─────
 Send…  Transactions…  Sign message…  Options…  Information   (menu rows with SF Symbols)
 Open Dash Wallet   Quit Dash Wallet
```
Today: monospaced balance (`MenuBar/MenuBarContentView.swift:36`), raw address as the last-transaction
title, green amount, two separate Testnet/Demo badges.

### 4.20 Alerts and small sheets

- System `alert` for yes/no questions with dash-qt text (Close Wallet, duplicate recipients, abandon).
- Anything with a field or more than two sentences → `SheetScaffold` small (Open URI, Backup wallet,
  Existing data, Encrypt/Change passphrase, Edit address).
- Toasts for copy/success notifications; `SystemMessage` for persistent conditions.

### 4.21 CoinJoin — Overview card, CoinJoin page, mixing status (M3, QT-041…051)

**Overview CoinJoin card** (dash-qt Overview panel, QT-041; shown when CoinJoin is enabled), placed
between the shortcut card and History:
```
‹card r20›  (coinjoin-mixing 30)  CoinJoin            Status: Enabled        [Start CoinJoin]
            ▬▬▬▬▬▬▬▬▬▬▬▬░░░░░ 63 %   (Completion; advanced mode only)
            CoinJoin Balance  1.23456789 Ð      Amount and Rounds  1000 DASH / 4 Rounds
            Submitted Denom  1.00001; 0.100001;  (advanced only)
```
- Button: [Start CoinJoin] filledBlue M / [Stop CoinJoin] tintedGray M / "(Disabled)" disabled with the
  dash-qt tooltip for the cause (backups disabled / backup failed / keypool).
- "~X / N Rounds" in `danger` with the dash-qt tooltip "Not enough compatible inputs to mix…".
- Completion tooltip = dash-qt multi-line text (Overall progress / Denominated / Partially mixed / Mixed /
  average rounds). Formula from research 02 §9.2 (engine/VM; the view only draws the number).
- Status line when keys are low: "Status: Enabled, keys left: N" with N in `danger` below 100.

**CoinJoin page** (sidebar "CoinJoin", QT-012 tooltip "Send CoinJoin funds to a Dash address"):
```
TopIntro  CoinJoin / "Send mixed funds. Mixing breaks the link between your coins and their history." (new)
‹Mixing status card›  ProgressRing 72 (completion)  |  Mixing — 3 sessions active (status string §9.4)
                      Mixed 1.2345 Ð of 10 Ð target · 4 rounds          [Stop CoinJoin] tintedGray
                      ▸ Session details (advanced): queue, submitted denominations, last pool message
‹Send form›  identical to §4.7 with source fixed to "CoinJoin funds only"; confirm sheet adds
             "(CoinJoin transactions have higher fees usually due to no change output being allowed)"
             and "This transaction will consume %n input(s)" (+ privacy warning ≥ 10 inputs, docs link)
```
- Session status strings (§9.4) are shown verbatim as the status line (dash-qt only exposes them via RPC;
  showing them is a desktop improvement). They come from the engine — no invented progress text.
- Start with a locked wallet → mixing-only unlock sheet (§4.4). Decline → dash-qt string "Wallet is
  locked and user declined to unlock. Disabling CoinJoin." as a toast(.warning).
- Below-minimum balance: `SystemMessage(.info)` "CoinJoin requires at least %2 to use."
- First use: `SystemMessage(.info)` suggesting the "Most Common" transaction filter (dash-qt), with
  [OK] and [Show information] (opens the CoinJoin information sheet).
- Mixing transactions in history: iOS groups them as one "Mixing Transactions" row per day with
  `tx-mixing` icon and the `arrow.triangle.2.circlepath` amount accessory, count top text
  "%d transaction(s)"; click → grouped detail sheet. Table mode keeps the five dash-qt CoinJoin types.
- Popups for mixing transactions: suppressed unless "Show popups for mixing transactions" (QT option).
- **CoinJoin settings** (Settings ▸ CoinJoin tab, research 02 §9.3): MenuCard rows — Enable CoinJoin
  features (toggle; also on Wallet tab per dash-qt), Enable advanced interface, Show popups for mixing
  transactions, Warn if the wallet is running out of keys, Enable multi-session + Parallel sessions
  stepper 1–10, Mixing rounds stepper 2–16, Target balance AmountEntry (2–21,000,000, step 10), Inputs
  per denomination Target/Maximum steppers (10–100000; target ≤ maximum). Live-apply, like dash-qt.
- **CoinJoin information** (Help menu): `SheetScaffold` medium, `TopIntro` + Core's information text.

### 4.22 Masternodes — replaced by §4.25

Replaced by **§4.25 Masternodes (MG-00)** (DASHPAY §5a, DEC-01). The dash-qt table layout described here
before is gone: My masternodes is §4.25.2, the registration wizard §4.25.5, the update sheets §4.25.6,
shared masternodes §4.25.8. Operator-signed Update Service and Revoke are out (§4.25.7).

### 4.23 Governance — replaced by §4.25

Replaced by **§4.25.3** (Governance tab: clock, budget, proposals, create, Mine) and **§4.25.4** (vote sheet).

### 4.24 Governance clock — replaced by §4.25

Replaced by the clock card in **§4.25.3** and the status-bar item in **§4.25.1** (MG-00 D19).

### 4.25 Masternodes (MG-00; release 1.1; built by MG-09, MG-10, MG-10b)

Owner-side masternode and governance flows for wallet holders (DEC-01). Scope is DASHPAY §5a's left column;
everything in its right column (operator and server) is out and appears only as an explanation (§4.25.7).
Mocks: `docs/design/ux/mg-00/*.html` (open `…?theme=dark` for dark; `#<frame>` for one frame), screenshots
`docs/design/screenshots/mg-00/<frame>-{light,dark}.png`. Decisions are the D-rows in
`docs/design/MG-00-masternodes-ux.md`; "D7" below means that file's row D7.

Rules that apply to every screen here:
- **Mobile-inspired, not dash-qt.** Layout and wording follow the DashPay screens (U-02…U-05, DP2-07's
  split view) and iOS `MasternodesScreen`. No data tables, no column pickers. dash-qt strings are kept
  verbatim only for the two warnings that carry risk: the proposal fee and the operator-key PoSe ban (D29).
  Every other string here is new (mark `new` in L10n) or from iOS.
- **Honest data.** Status comes from the SPV deterministic masternode list, which carries only the ProTx
  hash, service, operator key, voting key, type and valid/banned. Owner, payout address, payees, collateral,
  operator's share and registration height come from the masternode's ProRegTx (operator and voting keys
  are always read live from the list). The current payout address and payees are known only after the
  wallet has applied **every** later ProUpRegTx (and ProUpShareTx for a shared masternode) for that ProTx hash
  up to the tip; whether the operator's share is paid yet comes the same way: it is paid once the latest
  ProUpServTx sets an operator payout address, and Core clears that address on every revocation (ProUpRevTx)
  and every operator-key change (ProUpRegTx or ProUpSharedRegTx), so the wallet applies all of these in block
  order (D42, D46). The wallet only reads ProUpServTx and ProUpRevTx; it never sends them. MG-03 watches each known ProTx hash through compact filters from the registration height,
  including after a restore (D42). Until that check has finished, these values show "Checking…", payout custody in "In this wallet" reads "Payout address:
  checking", and actions that depend on them (a fixed withdrawal destination) are disabled with "Checking
  the current payout address…". Without the ProRegTx they show "—" or are left out, and actions that need
  them aren't offered. Values only a full node knows
  (PoSe score, last-paid height, next payment, consecutive payments, ban and revive heights) show "—" with
  the hint "needs a full node". Nothing is estimated except values labelled as estimates: the governance
cutoff (D19) and vote tallies (D45) (D9). Payouts are what this wallet's history shows.
- **No dead buttons.** An action the wallet can't do says why in its tooltip or in a row note, or is not
  shown. Operator actions are one explanation sheet, never disabled lookalikes of dash-qt's buttons (D24).
- **Signing.** Every signing flow ends with the inline Passphrase field on its last step, as Pay's
  ConfirmSheet does (D14). The flow takes a single-use grant (E0-04, MG-05) under a named lease (D35); a
  locked wallet browses freely and asks only there.
- **Testnet and mainnet look the same.** Network differences are validation messages, not modes.

#### 4.25.1 Sidebar item, visibility, page frame and shared states

**Sidebar.** One item **Masternodes** (icon: two stacked server boxes, `sidebar-icon` 20 × 20 stroke), last:
after CoinJoin, and after Explore when Explore is shown (DASHPAY R11). Shown when any of these is true (D1):
1. the wallet holds a masternode role: collateral, owner key, voting key, payout script or share refund
   (the same set as Core's `protx list wallet`);
2. it holds an unspent output of exactly 1,000 or 4,000 Dash (a collateral-shaped coin);
3. it tracks a masternode, or has a registration, shared session or proposal in progress;
4. Settings ▸ General ▸ **Show Masternodes** is on (new toggle, default off; this is DASHPAY §5a's
   "advanced mode"; there is no general advanced mode or Advanced tab, D2).

When the item disappears (for example the last collateral is spent), the route stays reachable from the
setting. Badge (red count, as Contacts): items that need the user's action only — a PoSe-banned masternode
whose owner key or collateral is in this wallet, a registration or shared session waiting on this user, a proposal whose fee is confirmed but
not yet submitted because submission failed. Unvoted proposals never badge (D8).

**Page frame.** `TopIntro` "Masternodes", trailing actions; then `tabs` **My masternodes | Governance**
(`ui.masternodesTab`, remembered, D3). Header actions per tab:

| Tab | Primary (filledBlue S) | More menu (tintedGray S) |
|---|---|---|
| My masternodes | Set up a masternode | Track a masternode… · Masternode keys… · Join a shared masternode… |
| Governance | Create proposal | Show ended proposals · Governance numbers… (D20) |

Below the tabs, a small muted line on the right: "Masternode list at block 1,214,880" (the SPV list height).

**Status bar.** The dash-qt moon-phase clock (QT-026) is replaced (D19). Until the estimated cutoff for
the next payout, while the wallet holds at least one eligible voting key, the status bar shows a short text item "Nov 4 votes:
about 1 d 6 h left" (mock `gov-list`); click → Governance. Otherwise nothing. Tooltip: "Votes cast before
about block 2,373,386 (the estimated cutoff) count for the Nov 4 payout at block 2,375,048. Later votes still
count, but may only change later payouts." (D19)

**States shared by every screen** (each screen section lists only its additions):

| State | What the user sees |
|---|---|
| Loading (masternode list not yet synced) | `loading-state` "Loading the masternode list…" in place of the list; own masternodes already known from the wallet show with the `Unknown` chip and "list syncing" |
| List failed | `mg-notice-warning` "Couldn't load the masternode list. Statuses may be out of date." [Try again]; rows keep their last status with "as of block N" |
| Locked | Everything browses. The lock is asked for only on a signing step, by the inline Passphrase field. Mock `mn-locked` |
| Watch-only | Info notice "**Watch-only wallet.** It shows the masternodes whose collateral or payout address it watches, and their payouts. Voting, registering and changing keys need the wallet that holds the keys." Signing actions are disabled with the reason as tooltip; Set up uses dash-qt's "This wallet is watch-only. Masternode registration requires a wallet that can sign transactions." Roles read "(watched)". Mock `mn-watchonly` |
| No peers / offline | Lists keep their last data with "as of block N"; signing steps say "Not connected. The transaction will be sent when the wallet reconnects." only where the engine queues; otherwise the commit button is disabled with tooltip "Not connected to the network" |
| Network mismatch | Never a page state: a field, paste or file from another network is refused where it is entered, with "This is a Mainnet address. This wallet is on Testnet." (address), "9999 is the Mainnet port. This wallet is on Testnet, where masternodes use 19999." (port), "This key is for Mainnet…" (key), "This invitation is for Mainnet…" (shared messages), "No masternode with this ProTx hash on Testnet." (track) |
| Network not upgraded | Features that need a deployment the network hasn't activated (v24 shared masternodes, `evo_shares`) show their choice disabled with "Needs the v24 network upgrade, not active on Mainnet yet." The engine reports activation; the UI never assumes it (D33) |

#### 4.25.2 My masternodes

Mock `mn-list`. A split view like Contacts (DP2-07): list on the left (`contacts-list`), the selected
masternode on the right (`contacts-pane`). A summary strip on top (`mg-summary`): **Masternodes** "3 yours" / "2 active · 1 tracked", **Payouts, last 30
days** "7.83 to payout addresses in this wallet", **Evonode credits** "0.42 ready to withdraw" (only with an
evonode).

**List groups** (in this order, empty groups hidden; D4):

| Group | Holds |
|---|---|
| Needs attention | PoSe-banned masternodes whose owner key or collateral is in this wallet, with "noticed Oct 12" (the wallet's own first observation, D9); a banned one where only the voting key is here stays under My masternodes with its chip |
| In progress | registrations not yet in the list (`Registering`, `Needs signature`) and shared sessions (§4.25.8) |
| My masternodes | masternodes with a role in this wallet, including shared ones ("you hold 1 of 3 shares") |
| Tracked | masternodes added with Track (IOS-082), whatever keys were attached; meta "finding its collateral" until the outpoint is resolved (D43) |
| Retired (N) | collapsed disclosure: deregistered masternodes that still have history in this wallet |

**Row:** icon (`masternode-network-monitor`), name (the user's label, default "Masternode 1"/"Evonode 1"),
sub "Masternode · 203.0.113.7:19999" / "Evonode · …" / "Shared masternode · you hold 1 of 3 shares",
trailing status chip and a meta line ("Paid Oct 13", "0 of 1 confirmations", "0.42 credits to claim").

**Status chips** (D5): `Active` (success: dot + primary text, D30), `PoSe banned` (danger), `Retired`
(neutral), `Unknown` (neutral: not in the list, or list syncing), `Registering` (info: sent, waiting for the
list), `Needs signature` (warning: external collateral, waiting for the proof). No "Next payment".

**Detail pane** (`mg-pane`):
- header: icon, name (rename by clicking it), chips: status + type (`Masternode` / `Evonode` / `Shared · 3 shares`);
- facts (`mg-facts`, each only when known, see Honest data): Service; Registered ("Mar 3, 2026 · block
  1,123,456"); Operator's share; Last payout (amount · date, from history; "— (the payout address is not in
  this wallet)"); Payouts, 30 days with **See in Activity** (opens Activity filtered to this masternode's
  payouts, D10); PoSe score "— (needs a full node)"; ProTx hash (copy); payees "Payouts split 3 ways" when a
  v24 masternode has several (D28); evonode: Platform node ID, Platform status (as Platform
  reports it, IOS-081), "Credits to claim 0.42" with [Withdraw…]; after Release collateral: a
  warning note "Collateral released: spending it ends this masternode." with [Lock it again] (D38);
- **In this wallet** (`mg-roles`): Collateral, Owner key, Voting key, Payout address, Operator key (only
  for self-operators, from the keychain), each ✓ held or – elsewhere;
- actions: [Vote on proposals] (opens Governance filtered to "Needs my vote"; disabled with "A banned
  masternode can't vote" or "Its voting key is not in this wallet"), [Change keys or payout…] (§4.25.6;
  shown with the owner key), [More]: Rename… · Copy ProTx hash · Copy collateral outpoint (when known) ·
  Add keys… (tracked) · Service update or revoke… (§4.25.7) · Release collateral… (collateral in this
  wallet) · Stop tracking (tracked only). Items the wallet can't do carry a `mg-menu-note` ("Needs the owner
  key", "Done by the operator on the node").

**PoSe banned** (mock `mn-banned`): danger notice in the pane "**This masternode is banned and earns
nothing.** This wallet first saw it banned on Oct 12: the server stopped doing its work. Your operator fixes
the server, then sends a service update from the node. The wallet can't do that: it needs the operator key."
Office node's owner key and collateral are in this wallet, so it badges and the notice adds "To move to
another operator, use Change keys or payout."; Vote on proposals is disabled. The More menu shows the
operator item with its note (§4.25.7) and Release collateral. A banned masternode never recovers on its own
(Core), so the notice stays until the list shows it valid. The date is the wallet's own first sighting: the
SPV list carries no ban height (D9).

**Empty** (mock `mn-empty`): `mg-empty` "No masternodes yet" / "Masternodes and evonodes registered with this
wallet's keys appear here after the wallet syncs. You can also track any masternode on the network by its IP
address or ProTx hash." and three points: **Own one** "Lock 1,000 Dash (4,000 for an evonode) as collateral.
It stays in your wallet." · **Someone runs it** "A hosting provider, or you, runs the server. The wallet never
needs the server's key." · **Earn and vote** "Payouts arrive in this wallet, and the node votes on how the
network spends its budget." Then [Set up a masternode] [Track a masternode]. Reached from the setting, or
when the wallet holds a collateral-shaped coin (D1); then the intro adds "This wallet holds a 1,000 Dash
output that can be a masternode collateral." 

**Track a masternode** (sheet medium, mock `mn-track`; IOS-082): search "IP address or ProTx hash" [Find]
(iOS also searches by private key; here keys are added after the masternode is found, D13), "Track any
masternode or evonode on the network. It doesn't have to belong to this wallet." → result card (type,
service, status, ProTx hash, voting address: only what the list carries) → "Label (optional)" → **Keys (optional)** "Add a key to act for this
masternode from here. Keys are kept encrypted in this wallet and used only to sign what you approve.":
Voting key (checked live: "Matches this masternode's voting key" / "This key doesn't belong to this
masternode"), Owner key ("Lets you withdraw evonode credits to its payout address. Checked the first time
you use it." — the list doesn't carry the owner key, D13). Footer note "Tracking sends nothing to the
network." [Track]. Above the footer: "After you track it, the wallet looks up its registration to find its
collateral and current payout address. Until it finds its collateral, the masternode can't vote here."
The lookup (D43): the list entry's `confirmedHash` names the block at the registration height plus the
network's minimum masternode confirmations. The wallet looks that hash up in its verified header chain,
subtracts the network's `nMasternodeMinimumConfirmations` (15 on mainnet; 1 on testnet, devnet and
regtest) from its height, downloads the block at the result from a
full-block peer, checks the block against the verified header at that height (hash and merkle root), and
takes the transaction whose txid equals the ProTx hash: the ProRegTx. The collateral outpoint follows
Core's rule: when the payload's collateral hash is null (the collateral is created in the ProRegTx itself,
as for every shared registration), it is (ProRegTx txid, payload index), and the wallet checks that output:
the masternode type's collateral amount (1,000 Dash, 4,000 for an evonode) and the right script (a single
address, or the shared-collateral template for a shared masternode); otherwise it is the payload outpoint as
given (Core checked that external collateral when it accepted the registration). That derived outpoint is
the one votes are mapped by (MG-02). Then the wallet reconciles later updates from that height (D42). A
failed check counts as a failed lookup. A masternode not yet confirmed waits for it. While the
lookup runs (usually seconds) the row shows "finding its collateral" (mock `mn-list`, Club evonode); if it
fails: "Couldn't find this masternode's registration. It can't vote from this wallet yet." [Try again]. Keys
can be added later from the masternode's More menu. Errors:
"No masternode with this address or ProTx hash on Testnet."; "Already tracked as Friend's evonode.".

**Masternode keys** (sheet large, mock `mn-keys`; IOS-083, QT-124 as the reveal gate only): segments Owner ·
Voting · Operator · Platform node; "Keys this wallet derives for masternodes it registers. 2 of 3 used.";
rows "Keypair 1 · yNq3…8Rz1 · Used by Home node · 203.0.113.7:19999" / "Not yet used" with [Show]; the shown
keypair's Address or public key (copy) and "Private key · Hidden" [Reveal…] "Needs your passphrase. Anyone
with this key controls the masternode's registration." (operator: "…runs the masternode's server"). Reveal
uses the RevealSecret gate and shows the secret in a monospaced box until the user hides it or closes the
sheet. This is the only place an operator secret is ever shown (§5a).

**Withdraw credits** (sheet small, mock `mn-withdraw`; IOS-081 owner part; engine MG-13, UI MG-10): "Hosted
evonode · Claimable 0.42000000"; Amount + [Max] ("Max leaves enough for the Platform fee."); To: fixed to the
payout address when signing with the owner key, any address when the payout key is in the wallet (iOS rule,
D23); the fixed destination appears only once the current payout address is reconciled (D42): help "The
evonode's current payout address, in this wallet · checked up to block 1,214,880"; until then "Checking the
current payout address…" and Withdraw is disabled; "Signed with: Owner key"; Platform fee "Taken from the credits"; info notice "With the owner key,
credits can only go to the registered payout address. Dash Platform pays them out on the Dash chain; they
show in Activity when they arrive."; Passphrase; [Withdraw]. States: loading ("Checking Platform for
credits…", Withdraw disabled); Platform unreachable ("Couldn't reach Dash Platform. Try again later." [Try
again]); too little ("0.00002 credits is less than the Platform fee."); locked (passphrase on the sheet);
watch-only (not offered).

**Withdrawal with an unconfirmed result** (mock `mn-withdraw-pending`, D44): when the request was sent
but its result didn't arrive (the engine's unconfirmed outcome: accepted for broadcast, or its
acknowledgement lost), the sheet and the evonode's pane show a persistent "Withdrawal sent; checking its
result" with Amount, To, "Sent Oct 15 at 3:41 PM · last checked 3:44 PM", "The request was sent, but
Platform hasn't confirmed the result yet. The wallet is checking whether Platform applied it.", and the
warning "**Don't send it again from
another device.** If this one went through, sending again would start a second withdrawal. Withdraw stays
off while the wallet checks the result." (shown in Sent; the Couldn't confirm row replaces it) Withdraw is
disabled (tooltip "Waiting for the result of the
withdrawal already sent"); [Close] keeps checking ("Keeps checking after you close this"), across restarts.
This needs an extended Platform-wallet API (MG-13 dependency, D44): before broadcasting, the engine
hands back and the wallet stores the signed transition, its ID and the identity nonce it used; an
ambiguous outcome returns them; and the engine can look a transition up by ID and rebroadcast the identical
bytes. The outcomes:
- **Done:** proof that **this** transition was applied (its result returned for its ID, or an equivalent
  record uniquely attributable to it; the upstream change names which) → "Withdrawal done" (credits arrive
  as Dash in Activity).
- **Rejected:** a rejection **on the merits** (not a nonce error) of the first broadcast, or of a
  rebroadcast while the transition's nonce is still unused → "Platform rejected the withdrawal: <reason>.
  Nothing was withdrawn." and only then [Try again].
- **Still checking:** anything else. The wallet looks the transition up and rebroadcasts the **same**
  signed bytes now and then (same nonce, so it can't apply twice); nothing new is offered to send. Neither
  the balance (rewards can credit it meanwhile) nor the nonce counts as proof, and a **nonce rejection of a
  rebroadcast** proves nothing either way: Platform nonces work in a window ("already present" means that
  nonce was used, by this withdrawal or by another request; "too far in past" only that the window moved
  on). Either one, with no proof, moves to Couldn't confirm.
- **Couldn't confirm** (D47): once the identical transition can no longer be accepted and no proof is
  found. Two branches, by the rebroadcast's nonce error:
  - "already present": "Couldn't confirm this withdrawal. Its nonce was used, by this withdrawal or by
    another request from this evonode, so it can't be sent again."
  - "too far in past": "Couldn't confirm this withdrawal. The original request is too old to retry, and
    the wallet still can't confirm whether it went through." (Platform's window moved on; this says nothing
    about whether that nonce was used.)

  Both continue "Check whether 0.4199 Dash arrived at yPay8vK2…Wm4c (Activity, or a block explorer;
  withdrawals can take a few hours)." with [It arrived] and [It didn't arrive…]; the second asks "Allow a
  new withdrawal? If the first one did go through, a new one withdraws again from this evonode's credits.
  Only continue if 0.4199 Dash didn't arrive." [Cancel] [Allow a new withdrawal]. Withdraw stays off until
  one is chosen.
- **Marked as received / Marked as not received** (D47): the user's choice, recorded as such and never as
  "Withdrawal done". The row reads "Marked as received" (or "Marked as not received"); the detail view adds
  a quiet "You marked this; the network hasn't confirmed it." Withdraw is enabled again. The wallet keeps
  the transition ID and signed bytes and keeps looking the transition up (no rebroadcast: it can't be
  accepted any more).
- **Late proof** (D47): proof found after "Marked as received" upgrades the record to "Withdrawal done"
  without a notice. Proof found after "Marked as not received" corrects the record to "Withdrawal done" and
  shows a notice naming the proven withdrawal and its stored destination: "The withdrawal of 0.4199 Dash
  to yPay8vK2…Wm4c, sent Oct 15, did go through. Its record now says Withdrawal done. If you withdrew again
  since, that withdrawal may also have gone through: check its own status and destination." When this
  wallet sent the later withdrawal, the notice links it ([Show the later withdrawal]); only when that later
  transition has proof too does the notice say both went through, each with its own amount and
  destination ("Both withdrawals went through: 0.4199 Dash to yPay8vK2…Wm4c on Oct 15, and 0.4199 Dash to
  yQx7…3hLp on Oct 16."). Proof found while still in
  Couldn't confirm settles it as Done directly. There is no proof that a transition was **not** applied,
  so nothing downgrades a marked record.

The state machine: Sent → Done (proof) · Rejected (merits) · Couldn't confirm (no longer acceptable, no
proof); Couldn't confirm → Marked as received · Marked as not received (user) · Done (late proof); Marked as
received → Done (late proof, silent); Marked as not received → Done (late proof, with the notice).

While Sent or Couldn't confirm is open, the summary's "Evonode credits" note reads "withdrawal being
checked", and the evonode's pane shows the same state with Withdraw credits disabled (mock `mn-withdraw-pending`, behind the
sheet).

#### 4.25.3 Governance

Mock `gov-list` (mainnet sample). Single column on top, split view below.

**Clock card** (replaces §4.24): title "About 1 day 6 hours left to vote for the Nov 4 payout"; sub "Next
payout to proposals: Nov 4, around 3 PM · block 2,375,048. Later votes may only count for later payouts."; a cycle bar (`mg-cycle`)
from the last superblock to the next with the **estimated cutoff** mark (start of the maturity window, 1,662
blocks on mainnet: Core begins building the superblock trigger there, it doesn't reject later votes, D19) and
the current position; labels "Oct 6 · last payout", "estimated cutoff, Nov 1", "Nov 4". Times come from block
heights at 2.5 minutes a block, always "about". After the cutoff: "Votes now may be too late for the Nov 4
payout; they still count for later ones. Payout in about 2 days." While awaiting the superblock: "Payout due
now (block 2,375,048)".

**Budget card:** "Budget for Nov 4", muted "from peers, an estimate" (D45; every governance count is
taken from the votes peers have sent, and SPV peers don't prove they sent them all); "4,760 of 6,018 goes to 9 passing
proposals"; "2 more pass the vote but don't fit (690)"; a stacked bar (`mg-budget-bar`: passing, the part
that doesn't fit). Core packs proposals by net votes and skips one that doesn't fit, so "Over budget" can
sit above a smaller proposal that is paid.

**Toolbar:** search "Search proposals"; segmented **All 37 · Needs my vote 12 · Mine 1** ("Needs my vote"
appears only with an eligible voting key); Sort ▾ Closest to passing (default) · Most asked · Newest ·
Name.

**Proposal row:** name, "1,500 Ð × 3 payments · You: 1 of 3 seen voting" (or "once", "You: Yes (last
seen)", "You: no vote seen"; D45), a vote bar (`mg-votebar`: yes / no /
abstain, with the funding-line mark), status chip and the margin
("about +414 over", "about 692 short": margins are estimates like every vote count, D45; when the range
from the votes seen crosses the line it is shown, "about 6 short; −18 to +6", generally not symmetric
because each unverifiable vote counts 0, 1 or 4).

**Status chips** (D17; engine `ProposalStatus`):

| Engine | Chip | Style | Tooltip |
|---|---|---|---|
| Passing | Passing | success | "From the votes peers sent: enough net yes votes and fits the budget." |
| Unfunded | Over budget | warning | "From the votes peers sent: enough votes, but proposals with more votes use up the budget." |
| Voting | Needs votes | info | "From the votes peers sent: not enough net yes votes yet." |
| Failing | Not funded | danger | "From the votes peers sent: more no than yes votes." |
| Funded | Funded | success | "Paid in the last superblock." |
| Confirming | Confirming | neutral | "Submitted after the first confirmation; fully valid after six." |
| Pending | Ready to submit / Fee paid | warning / neutral | Mine only (below) |
| Lapsed | Ended | neutral | "All payments made or the end date passed." Hidden unless "Show ended proposals" |
| Passing, Unfunded, Voting or Failing whose range from the votes seen crosses the funding line or changes its budget order | **Too close to call** | neutral | "The vote count is an estimate, and this one is within its margin." (D45) |

**Detail pane:** name, chip + "paid Nov 4 if it stays above the line", "Asks for 1,500 Ð × 3 payments,
Nov 4 to Jan 3", "Paid to"
(address, copy), Link (plainBlue; opening shows dash-qt's External Link Warning, default No), weighted votes
"about 771 net yes · needs 357" with legend "Yes about 812 · No about 41 · Abstain about 9 · Funding line"
(weighted: an evonode counts 4) and the range "From the votes peers sent: 759 to 783 net yes. Unseen votes
can change it." (D45: the range covers the votes seen whose masternode type the engine can't verify, a
weight of 0, 1 or 4 each; it is conditional on the votes seen, so the pane never says the result can't
change), "Your masternodes" (each with its last vote seen and time, "Yes · last vote seen 40 min ago", or "No
vote seen"; a masternode is listed only once its outpoint is known, D43, one whose collateral isn't found
yet shows "—"), [Vote…] filledBlue (opens §4.25.4), More: Copy proposal
hash · Copy raw JSON. Threshold = max(min quorum, weighted valid masternodes / 10) computed by the engine
(Core's `fundingthreshold` omits the floor).

**Governance numbers** (sheet medium, mock `gov-numbers`; from Governance ▸ More; QT-134, D20): **This cycle**
— Last superblock "Block 2,358,432 · Oct 6", Estimated cutoff "Block 2,373,386 · about Nov 1: votes after it may be too late for the Nov 4 payout", Next superblock
"Block 2,375,048 · about Nov 4", Cycle "16,616 blocks; the network starts fixing a payout 1,662 blocks before its superblock";
**Votes and budget** — Voting weight "3,570: 2,850 masternodes and 180 evonodes, which count 4 each", Funding
line "357 net yes votes: a tenth of the weight, and at least 10", Budget "6,018 per superblock" (these three
are exact, from the chain and the masternode list, and carry no "about"), **Passing, estimated** "about
4,760 to about 9 of 37 open · about 2 more over budget" (the only vote-derived row; its label scopes the
estimate), Data "Block 2,372,666 · 8 peers; some votes may be missing" (D45). Before sync, the vote and
budget rows show "after sync". [Done].

**Create proposal** (wizard sheet large, 3 steps; mocks `gov-create`, `gov-create-review`):
1. **Details:** Name (1–40 characters `a-z 0-9 - _`, help "Lower case letters, digits, - and _"), Link
   (help "A page describing the proposal, such as a Dash Central or forum post").
2. **Payments:** First payment (the next 12 superblocks with date and block; help "Payments happen at
   superblocks, about every 29 days."), Number of payments (1–12; "Last payment Feb 1, 2027."), Amount per
   payment, Total, Pay to (**A new address in this wallet** / **Another address…**). When the nearest
   superblock's estimated cutoff is within two days the wizard preselects the next one and says so: "The
   estimated cutoff for the Nov 4 payout is in about 1 day 6 hours, so the first payment is Dec 4. New proposals rarely gather enough
   votes in a day." Info: "Up to 6,018 Dash is available at each superblock. From the votes peers have sent, passing proposals
   already ask for about 4,760, so yours fits only if it passes with more votes than others that don't
   fit." (shown once proposals are synced; the budget is exact, the passing total an estimate, D45; this
   is shown before the non-refundable 1 Dash fee).
3. **Review and pay the fee:** detail list (Name, Link, Payments "1,200 × 3, Dec 4 to Feb 1 · total
   3,600", Paid to, Fee "1.00000000 + network fee 0.00000227"), disclosure "Proposal data (JSON)", warning
   with dash-qt's verbatim bold sentences "Creating a proposal pays 1 DASH to the network. This fee is
   non-refundable regardless of outcome." then "After it confirms, the wallet submits the proposal for you.
   You can close this window; Mine shows the progress." Passphrase. [Pay 1 Dash and create].

The chosen first payment is honoured (dash-qt's date bug is not copied). The proposal and its fee
transaction are saved in the wallet before the fee is sent, so a crash can't burn the fee (D18).

**Mine** (mock `gov-mine`): the user's proposals with their own chips — `Fee paid 0 of 1` ("Fee paid Oct 15
at 2:02 PM · waiting for its first confirmation, then the wallet submits it"; D16), `Confirming 4 of 6`
("Submitted after the first confirmation · fully valid after 6"), then the public statuses. If submission fails: `Ready to submit` (warning) with [Submit] and the error ("No peer accepted
the proposal. Try again in a few minutes."). Note "Proposals you create are kept with this wallet, so you can finish them after a restart; they travel
in wallet backups."

**States** (adds to §4.25.1):
- **Syncing** (mock `gov-syncing`; engine phases Waiting/SyncingObjects/SyncingVotes): the clock card shows
  (it needs only headers); the list area is `loading-state` "Loading proposals from the network… 64%"; no
  counts or tallies show until Synced: "1,204 of about 1,880 objects and votes. Counts appear when the sync
  is complete, never partly."; the budget card shows only "6,018 available" and "What passing proposals take
  shows after sync"; Create proposal stays enabled (D21).
- **No voting key:** info notice "**Browsing only.** This wallet holds no masternode voting keys, so it can
  read proposals but not vote. You can still create a proposal." [Add a voting key] (opens Track ▸ Add its
  keys). "Needs my vote" hidden.
- **Failed:** "Couldn't load proposals: no peer is serving governance data." [Try again].
- **Empty cycle:** "No proposals for Nov 4 yet." [Create proposal]. **Needs my vote empty:** "You have voted
  on every open proposal." **Mine empty:** "You haven't created a proposal." **No match:** "No proposals
  match “fund”."
- **Disabled** (governance sync off for the network, engine phase Disabled): the tab shows "Governance data is
  off for this network." without the clock.

#### 4.25.4 Vote sheet

Mock `gov-vote`, result `gov-vote-result`. Sheet medium "Vote on dash-incubator-nov", one proposal per
sheet (D11).
- Summary line: amount, payments, status and net votes.
- **Outcome:** segmented Yes · No · Abstain (radiogroup), no default selection when no last vote has been
  seen; preselects the last vote seen otherwise.
- **Vote with:** one row per masternode the engine lists for this wallet (owned and tracked), with a checkbox,
  name, service, key custody, weight (1, or 4 for an evonode) and last vote seen (column "Last vote seen";
  "None" when no vote has been seen, D45). Custody (D13): "key in this
  wallet" (keychain), "key you added" (tracked, vault), "Its voting key is not in this wallet" + [Add key]
  (disabled row). Disabled rows also for: voted less than an hour ago ("Voted Yes 40 min ago · can change
  after 4:12 PM", the network accepts one change per masternode per hour), PoSe banned ("a banned
  masternode can't vote"), collateral not yet found ("Key you added · finding its collateral first": tracked,
  outpoint unresolved, D43; never pre-checked; last vote seen "—"). Eligible rows start checked.
- **Privacy:** warning notice, shown when two or more rows are checked: "Voting with several masternodes
  together shows that one person controls them. Choose fewer to keep them unlinked." (iOS idea, D12).
- Note: "A new vote replaces that masternode's earlier one. The network accepts one change per masternode
  per hour."
- Footer note "2 masternodes · weight 8 · no fee", Passphrase, [Vote Yes] (label follows the outcome).
  Votes are free messages, not transactions; no extra confirmation step (D14).
- **Result** (same sheet): "2 of 2 votes sent" (success) or "1 of 2 votes sent" with per-masternode lines
  and the outcome with its weight ("Yes · weight 4 of 8"): "Hosted evonode · weight 4 · Sent", "Friend's
  evonode · Its voting key changed, so the key you added no longer matches. Update it in the masternode's
  details. · Not sent"; note "The counts update as peers relay the votes."; [Done]. Engine `governance.*`
  codes map to sentences; never raw codes.
- **States:** locked (passphrase on the sheet); watch-only (Vote disabled, tooltip "Voting needs the
  wallet that holds the voting keys"); syncing (Vote disabled until Synced: "Votes can be sent once
  proposals finish loading").

#### 4.25.5 Registration wizard

Sheet large with a step list (`mg-wizard`, 188 px left column) over the current tab; mocks `reg-type` →
`reg-done`. Footer [Back] tintedGray, [Continue] filledBlue (disabled until the step is valid; the reason is
the first field error). Esc / close on a step after 1 asks "Discard this registration?" (nothing was sent;
reserved coins are released). Steps (D25, the order of §5a with the type first):

1. **Type** (`reg-type`): "What are you setting up?" — **Masternode** ("1,000 Dash collateral. Runs
   Dash Core.") / **Evonode** ("4,000 Dash collateral. Also runs Dash Platform and earns Platform credits.") /
   **Shared masternode** ("2 to 8 people each put in a share of the collateral and split the payouts.") (opens §4.25.8; note "Masternode or evonode: you
   choose in the shared setup."; disabled without v24, §4.25.1). "Who runs the server?":
   **A hosting provider, or someone else** ("They give you an operator key (a public key), the server's
   address and their fee. You keep the collateral and the owner key.") / **I run it myself** ("The wallet fills
   in the operator key from its masternode keys. You copy the operator secret to your server later, from
   Masternode keys.").
2. **Collateral** (`reg-collateral`): "Where is the 1,000 Dash collateral?" / "The collateral stays yours.
   While it is unspent the masternode stays registered; spending it ends the masternode." — **Send it from
   this wallet** (FundNew; "Moves 1,000 Dash to a new address in this wallet, in the same transaction as the
   registration." + "Available: 1,204.37 Dash"; not enough → disabled with "You need 1,000 Dash and the fee;
   you have 812.40."), **Use a 1,000 Dash output already in this wallet** (ExistingUtxo; "Pick an unspent
   output of exactly 1,000 Dash."; rows outpoint · address · confirmations; one already used is disabled "In
   use"), **It is in another wallet** (External; "For example dash-qt or a hardware wallet. Enter the output;
   before sending you sign a message with that wallet to prove it is yours."; the output must be exactly the
   collateral at a regular P2PKH address — "This output can't be a masternode collateral: it isn't 1,000 Dash
   at a regular address.").
3. **Operator** (`reg-operator`; hosted): Operator public key (BLS, 48 bytes; ok "A valid BLS public key";
   help "The key the server signs with. Your operator keeps the matching secret on the server; never paste
   a secret here."; a pasted secret → "This looks like a secret key. Ask your operator for the public key."),
   Server address (IP and port; required, D26; help "Ask your operator for it. A masternode registered
   without an address earns nothing until the operator adds one."), Operator's share of payouts (0–100 %,
   two decimals; help "The operator gets this share of every payout once they set their payout address from
   the server; until then it stays with you. Leave it at 0 unless you agreed on a share."). Evonode adds
   Platform node ID, Platform P2P port and Platform HTTPS port (as the operator gives them).
   **Self-run** (`reg-operator-evo`): the operator key and (evonode) Platform node ID are filled from this
   wallet's keychain (next unused operator keypair, keypair 3 in the mock) with "Enter one instead"; a note
   "After registering, put the operator secret on the server: Masternode keys ▸ Operator ▸ Reveal." The
   wallet never generates a secret for the user to save here (§5a; QT-124's save gate is gone, D27).
4. **Keys and payout** (`reg-keys`): Owner key — "This wallet, owner keypair 3" / "Controls the
   registration: changing keys and payout. Always kept in this wallet." (no choice, D25). Voting — **Vote
   from this wallet** ("Uses voting keypair 3.") / **Let someone else vote** ("Enter their voting address.
   You can change it later."). Payouts go to — **A new address in this wallet** ("Payouts appear in Activity
   as "Masternode reward".") / **Another address** ("For example a cold wallet. This wallet then can't show
   the payouts."). Validation (Core's rules): the payout address may not be the owner or voting key's
   address; the collateral key may not be the owner or voting key. The wallet adds its own: a new owner keypair
   per masternode, and a voting keypair separate from it when this wallet votes.
   One payout address only (D28).
5. **Review** (`reg-review`): detail list (Type "Masternode, run by your operator", Collateral with "Locked
   against spending while the masternode is registered", Server, Operator key "…, from your operator",
   Operator's share "5% of each payout", Voting, Payouts to, Network fee "…, from this wallet", Then "Send
   your operator the ProTx hash. The masternode earns once the server runs and the network accepts it."),
   Passphrase, footer note "Only the fee leaves this wallet." (FundNew: "1,000 Dash move to a new address in
   this wallet; only the fee leaves it."), [Register]. External collateral: [Continue] to step 6 instead,
   without the passphrase.
6. **Prove the collateral** (external only, `reg-prove`): "Sign with the wallet that holds the collateral" /
   "The collateral b07e…c1d2:0 is in another wallet. In that wallet, sign this message with the collateral's
   address (in dash-qt: File ▸ Sign message), then paste the signature here." Address (copy), Message (copy,
   monospaced: `payout|operator share in basis points|owner|voting|payload hash`, Core's format), Signature
   with a live check ("The signature matches the collateral's address"), "The message holds this
   registration's details. If you go back and change them, sign again." (D25), Passphrase, [Register]. The
   prepared registration survives closing the app (`Needs signature` in My masternodes, mock `mn-locked`).
   The coins chosen for its fee are reserved while it waits, because the signed message covers them; if they
   are spent anyway (another wallet on the same seed), the step says "The coins for the fee were spent. Sign
   the new message." and shows the new message.
7. **Sent** (`reg-done`): "Registration sent", chip `Registering` "waiting for 1 confirmation", ProTx hash,
   next steps (hosted): "Send your operator the ProTx hash. They start the server with it." · "The masternode
   shows Active once the network sees the server." · "Payouts then arrive in this wallet and show in
   Activity as "Masternode reward"." Self-run replaces the first with "Reveal the operator secret in
   Masternode keys and put it on your server." [Copy ProTx hash] [Done].

**States:** locked (passphrase on Review/Prove); watch-only (Set up disabled, §4.25.1); network mismatch
(per field, §4.25.1); not enough funds (step 2); a collateral-shaped coin spent while the wizard is open
("This output was spent. Choose another."); engine refusal on Register maps `protx.*` codes to sentences
("The operator key is already used by another masternode.", "This server address is already registered.").

**Hosted vs self vs shared vs external** are not separate wizards: "who runs it" decides step 3, the
collateral choice decides step 6, and Shared hands over to §4.25.8.

#### 4.25.6 Change keys, payout, and ending a masternode

**Change keys or payout** (sheet medium, mock `upd-registrar`; ProUpRegTx, QT-125's Update Registrar):
"Home node · signed with its owner key, which is in this wallet. Change only what you need; the rest
stays." Fields: Operator public key [Keep current], Voting (Keep / this wallet / another address), Payouts
go to (Keep / new address / another). When the operator key changes: dash-qt verbatim warning "Changing the
operator key immediately PoSe-bans the masternode. The new operator must send an Update Service transaction
to revive it." rendered as "**Changing the operator key immediately PoSe-bans the masternode.** It earns
nothing until the new operator sends a service update from their server." (D29: the bold sentence is
verbatim). Note: "A new voting key drops this masternode's current votes; vote again afterwards. Changing
only the voting key or the payout address doesn't ban the masternode." Network fee, Passphrase; the button
names the change ("Change operator", "Change voting key", "Change payout", "Save changes" for several).
Without the owner key the menu item is disabled with "Needs the owner key". Shared masternodes use
§4.25.8 instead.

**Release collateral** (sheet small, mock `upd-release`): "Home node's 1,000 Dash collateral is locked so you
don't spend it by accident. Releasing it makes it spendable again." Danger notice "**Spending the collateral
ends the masternode at once.** It leaves the list, stops earning and can't be brought back; a new masternode
needs a new registration." Checkbox "I understand that Home node ends when I spend this collateral". "Nothing
is sent now. Until you spend the coins you can lock them again from this masternode's details." [Release]
plainRed. This is the owner's way to end a masternode (D38); there is no revoke button. Afterwards the
pane shows "Collateral released: spending it ends this masternode." with [Lock it again].

#### 4.25.7 Operator-only actions (out of scope, explained)

Mock `mn-operator`. More ▸ **Service update or revoke…** (menu note "Done by the operator on the node") opens
an explanation sheet "Service update and revocation", not a form (D24): "Your operator signs these with the
operator key, which stays on the server. Dash Wallet doesn't hold that key, so it can't send them."
(self-run: "This wallet derives your operator key, but Dash Wallet doesn't use it to sign service updates or
revocations. Send those from your node; the secret is in Masternode keys.") Three rows:
**Bring a banned masternode back** "The operator fixes the server, then sends a service update (`protx
update_service`) from the node." · **Stop the operator** "The operator can revoke its key (`protx revoke`).
You can also replace the operator yourself: change the operator key in Change keys or payout. The masternode
then waits for the new operator's service update." · **End the masternode** "Spend the collateral. The
masternode leaves the list at once. Use Release collateral in this masternode's More menu." Then "For Office
node, send your operator this ProTx hash:" (copy) [Copy ProTx hash] [Done]. Self-operators get the same
sheet: their operator secret is in Masternode keys, and they send these from their node.

The two owner rows depend on what this wallet holds (the mock is Office node, which holds both):

| Row | Owner key / collateral in this wallet | Held elsewhere |
|---|---|---|
| Stop the operator, 2nd sentence | "You can also replace the operator yourself: change the operator key in Change keys or payout. …" | "Whoever holds the owner key can replace the operator with a new operator key." |
| End the masternode, 2nd sentence | "Use Release collateral in this masternode's More menu." | "Spend the collateral from the wallet that holds it." |
| Tracked, no keys | — | Both "held elsewhere" sentences; the hash line reads "This masternode's ProTx hash:" |

Out of scope here, per DASHPAY §5a's right column: ProUpServTx (Update Service, Unban, IOS-081/082 unban),
ProUpRevTx (Revoke), generating or holding an operator BLS secret the wallet doesn't derive, Platform node
operation, node administration. QT-124 survives only as the Masternode keys reveal gate.

#### 4.25.8 Shared masternodes (v24, MG-04b, MG-10b)

Mocks in `shared.html`. Needs v24 on the wallet's network (§4.25.1). The type is chosen in the setup's first
step; Evonode there is disabled with "Shared evonodes need a later network upgrade, not active on this
network yet." until the engine reports `evo_shares` active (D33). Amounts below are for a masternode
(1,000 Dash); an evonode's shares add up to 4,000.

**Start or continue** (sheet large, `sh-start`): intro "Two to eight people fund one masternode together and
split its payouts by share. One person coordinates; everyone else pastes what they receive and copies back a
reply. Nothing reaches the network until the coordinator sends the registration." Two cards: **Start one**
([Start a shared masternode]) and **Join or continue** ([Paste] [Open file…]); facts: Shares ("2 to 8. Each is
at least 100 Dash, and together they make exactly 1,000 Dash."), Your coins ("Stay in this wallet, reserved
for the session (your share plus your part of the network fee), until the registration is sent."), Leaving ("Anyone can dissolve the masternode alone at
any time and everyone gets their share back. In the early period, whoever leaves alone pays a penalty to the
others."); "Sessions in progress" list.

**Coordinator setup** (wizard sheet large "Start a shared masternode", steps **Shares · Masternode · Exit
terms · Your coins · Invite**):
1. **Shares** (`sh-setup`): "Who takes part, and with how much?"; type cards **Masternode** ("Shares add up to
   1,000 Dash.") / **Evonode** ("Shares add up to 4,000 Dash. Also runs Dash Platform."); participant rows
   (name field, share field in Dash, [Remove]; the coordinator's row "Alice (you) · Coordinator"); [Add a
   participant] (up to 8); live sum "1,000 of 1,000 Dash · 3 shares" (error "200 Dash still unassigned" /
   "100 Dash too much"); help "2 to 8 people. Each share is at least 100 Dash and earns its part of
   every payout, after the operator's share once the operator sets a payout address. Names are labels: they go into the invitation so everyone sees the same list, and nothing
   else uses them." Errors: "Each share needs at least 100 Dash."
2. **Masternode:** the registration wizard's step 3 fields (operator public key or "I run it myself", server
   address, operator's share; evonode Platform fields) plus **Votes** — "From this wallet" (default) /
   "Another address" (D34). Same validation and copy as §4.25.5.
3. **Exit terms** (`sh-terms`): "How can people leave?" / "Anyone can always dissolve the masternode alone,
   and every share goes back to its owner. An early period protects the group: whoever leaves alone during
   it pays a penalty to the others."; Early period (None / 3 months / 6 months / 1 year / 2 years (the
   longest); "Counted from the registration."; 2 years = 420,480 blocks, Core's maximum); Early-exit penalty
   in Dash ("Less than the smallest share (300 Dash). Split among the others by share."; 0 and disabled with
   no early period); summary rows Leaving alone "Costs 20 Dash until about Apr 2027, then nothing", Leaving
   together "Always free when everyone agrees", Network fee to leave "Paid from the share of whoever
   leaves". With an early period and a 0 penalty, Core's warning becomes "With an early period and no
   penalty, anyone can leave early for just the network fee."
4. **Your coins:** the coordinator's own share plus the fee maximum is reserved from this wallet ("400.0001
   Dash of your 1,204.37 will be reserved: your 400 Dash share and up to 0.0001 Dash for the network fee"), refunds and payouts to new addresses in this wallet (or another payout address).
5. **Invite:** [Copy invitation] [Save to file…], the session code (6 hex) and the check code (`XXXX-XXXX`),
   "Send it to each participant. Ask each to read the check code back to you." [Done] opens the session
   sheet.

**Fee and rewards** (D41, D46). The collateral output must be exactly 1,000 Dash, so each participant
funds their share **plus** their part of the network fee. The invitation fixes a fee maximum per
participant (from the coordinator's estimate with headroom: 0.0001 Dash in the mock); each wallet reserves
share + maximum, and the Join button names the total. The locked terms state the real fee, split by share,
each part at or under the maximum; the unused rest comes back as change. If the coordinator's estimate
before locking the terms exceeds a maximum, the coordinator sends a new invitation with a higher maximum
and every participant joins again (their wallet reserves the difference); nobody's contribution rises
without them accepting. Once the terms are approved the fee is fixed; if it no longer relays at Send, the
coordinator goes back to Lock terms and everyone approves again. Core pays the operator's share
only once the operator has set a payout address with a service update (ProUpServTx; a registration has
none, and an operator-key change clears it), and then first, splitting only the rest by shares. So payout
copy states both: the current share and the conditional one. Invitation: "30% of each payout; about 28.5%
once the operator's payout address is set"; locked terms (mock `sh-session` "Payout split"): "Now 40%,
30%, 30% of each payout. Once the operator's payout address is set, its 5% comes first: about 38%, 28.5%,
28.5%"; detail pane: the current state from MG-03's reconciliation (D46).

**Session sheet** (`sh-session`): title "Shared masternode · Session 3F9A2C"; stages (`mg-stages`) **1 Invite ·
2 Collect details · 3 Lock terms · 4 Approvals · 5 Signatures · 6 Send** (D31: user-facing names, not R3's or
dash-qt's); heading for the current stage ("2 of 3 approved the locked terms"), check code; share bar;
participants (name, role, share, stage chip `Approved` / `Waiting` / `Signed`); the terms (server, operator
key, operator's share 5%, Payout split, Network fee "0.00006, split by share: Alice 0.000024, Bob 0.000018,
Carol 0.000018 (each within the 0.0001 maximum from the invitation)", early period); note "Send each
reply back here. If a check code differs from the one the sender sees, the message changed on the way: ask
for it again."; footer [Copy …] [Save to file…] | [Close] [Paste a reply]. The coordinator's last stage is
[Send registration] with Passphrase.

**Participant** (`sh-join`): Paste or open → "Join a shared masternode": "Alice invites you to share a
masternode", "Check code 7D3B-E519 · session 3F9A2C"; info notice "Before joining, ask Alice to read you the
check code, by phone or in person. If it differs, the invitation changed on the way: don't join."; share
bar; Your share ("300: 30% of each payout; about 28.5% once the operator's payout address is set · Alice 400 ·
Carol 300"), Masternode ("203.0.113.30:19999 · run by Alice · operator's share 5%, paid only after the
operator sets a payout address"), Votes ("From Alice's wallet"), Leaving ("Alone: 20 Dash penalty during a
6-month early period, then free. Together: free."), Network fee ("Split by share, at most 0.0001 Dash from you;
the exact amount is in the locked terms"), Your coins ("300.0001 Dash of your 412.80 are reserved until the
registration is sent or you leave the session: your 300 Dash share and up to 0.0001 Dash for the network
fee. What the fee doesn't use comes back as change. Refunds and payouts go to new addresses in this
wallet."); footer note "Nothing is sent to the
network now", [Decline] [Join: reserve 300.0001 Dash], which reserves the coins and shows the reply to copy
back (not enough coins: disabled with "You need 300.0001 Dash; you have 212.80."). A re-invitation with a
higher fee maximum shows "The network fee went up: Alice asks for up to 0.0003 Dash from you instead of
0.0001." and the button "Join again: reserve 300.0003 Dash". Later pastes (locked
terms, signing request) open the same session at its stage; approving and signing each ask for the
passphrase. A participant never builds the transaction.

**Close** (`sh-close`): alert "Keep this session for later?" "Keeping it leaves your 400.0001 Dash reserved so you
can carry on where you left off. Discarding releases them; if the others are still waiting for you, they
have to start over." [Discard and release coins] plainRed · [Cancel] · [Keep and close] (D32).

**After registration** (`sh-detail`): the masternode lists under My masternodes ("you hold 1 of 3 shares"),
pane with the share bar and legend, Your payouts ("40% of each payout to yRw4…n8Ka; about 38% once the operator's payout address is set"),
Operator's share ("5%, not paid yet: you haven't set an operator payout address. Set it with a service
update from your node · checked up to block 1,214,880": this pane is the operator Alice's; another
participant sees "Alice hasn't set an operator payout address"; once set: "5%, paid first" and Your payouts reads "about 38% of each payout (after the operator's
5%) to yRw4…n8Ka"), Your refund, Early
period ("Ends at block 1,319,400, about Apr 13, 2027"), Standby dissolution ("Saved on this computer on Oct
15" + [Save a copy…]); actions [Change my payout address] (ProUpShareTx, this share only, fee from this
wallet), [Change keys together] (ProUpSharedRegTx: a new session where every owner approves; operator-key
warning as §4.25.6), [Dissolve…].

**Standby dissolution:** right after the registration confirms the wallet prepares and stores a
zero-penalty dissolution of this user's share (valid once the early period ends, Core's advice) and offers
[Save a copy…] to a file kept elsewhere (D32).

**Dissolve** (sheet medium, `sh-dissolve`), segments **Alone, now · Together · Standby**:
- Alone: "Dissolving ends the masternode and returns every share to its owner's refund address. You can do
  it alone at any time." You get back (share − penalty − fee, the fee comes out of your share), the others
  (their shares plus the penalty split by share), Network fee "0.001 Dash, taken from your share"; in the
  early period a warning "**Early exit: you pay the 20 Dash penalty.** Leaving is free after block 1,319,400
  (about Apr 13, 2027), or at any time if everyone agrees: use Together." and a checkbox "I accept paying the
  20 Dash early-exit penalty" gating [Dissolve now] (plainRed). Passphrase.
- Together: a session like registration (everyone signs), no penalty.
- Standby: shows the saved standby, "valid from block 1,319,400", [Send it] when valid.

**States:** v24 inactive (Shared disabled); locked (passphrase per signing stage); watch-only (view only; no
Start/Join); network mismatch ("This invitation is for Mainnet. This wallet is on Testnet."); bad paste ("This
isn't a shared-masternode message." / "This reply belongs to session 7B21E0, not 3F9A2C."); too large (over
2 MiB: "This file is too large to be a shared-masternode message."); a participant's coins spent meanwhile
("Bob's coins were spent. Ask Bob for a new reply."); coordinator vanished (participant can Discard at any
time; coins come back at once since nothing was sent).

---

## 5. Amount, address and value formatting

### 5.1 The `AmountText` component

`AmountText(text: String, unit: AmountUnitDisplay, size: Double, weight: DashFontWeight,
glyphFactor: Double = 1.0)` where `AmountUnitDisplay = .glyph | .name(String) | .none`.
- Layout: `[sign][number]` then 2 pt then the Dash glyph (`glyph-dash-currency`, height
  `size × glyphFactor`, tinted with the text colour, baseline-aligned) **or** a space + unit name.
- `.glyph` only for the DASH unit (mainnet `DASH` and testnet `tDASH` — the network is signalled by the
  capsule/title). mDASH / μDASH / duffs use `.name` (there is no glyph for them). Accessibility label
  always spells the unit name (`"0.25 tDASH"`), so QT-003 testnet unit names stay in every spoken,
  copied and exported value; tooltips on amounts show the dash-qt `.withUnit` string.
- Colour: `textPrimary` (white on the hero). Never green/red by direction.
- The view never formats numbers: the VM passes a string from `AmountFormatting` (dash-qt rules:
  thin-space U+2009 grouping, "." decimal mark, QT-152). No locale decimal comma.

### 5.2 Precision styles (which formatter style where)

| Surface | Style | Example (0.20000000 / 30.39698287) |
|---|---|---|
| Overview hero + breakdown | `.floored(digits:)` from the Decimal digits setting (QT-036, default 2) | `0.20` / `30.39` |
| Transaction rows, menu bar, toasts, notifications, Overview recent rows | **new `.compact`**: exact value, trailing zeros removed down to a minimum of 2 decimals, **never rounded** | `0.20` / `30.39698287` / `-0.50000226` |
| Confirm sheets, tx detail, PSBT, coin control, CSV, clipboard | `.gui` / `.withUnit` full unit decimals | `0.20000000` / `30.39698287` |
| Amount entry (editing) | the user's text; on blur reformat with `.compact` | |
| Technical tables | `.plain(separators: .standard)` full decimals, tabular digits, right-aligned | |

`.compact` is added to `AmountStyle` in `WalletFeatures` with golden vectors next to
`testdata/amount_format.json` (Swift + Rust `dw-units` must agree). Rationale: iOS shows ≤ 5 decimals with
rounding (`DashAmount`), which can misstate a balance; dash-qt shows 8 always, which is noisy. Trimming
zeros keeps every significant duff and reads like iOS.

### 5.3 Sign, masking, unknowns, fiat

- Rows: `+`/`-` always (iOS `amountSign: .always`); zero has no sign; internal moves / payment to
  yourself: no sign + `arrow.triangle.2.circlepath` accessory in `textSecondary` (iOS). Balances: no sign.
- Discreet mode: the VM's masked string (`#`), unit kept; masked values are not copyable.
- Unknown: "—" in the amount's place + tooltip with the reason; "Balance unavailable" under the hero
  (iOS). Never `0` for unknown, never a placeholder number.
- Fiat: secondary line under the amount (`subhead` hero, `footnote` rows, `textSecondary`), formatted by
  the locale currency formatter. Shown **only** when a rate source exists (M5 `RatesService`); stale
  (> 30 min) shows the iOS stale-rate toast; failure hides the line. No "≈", no "$0.00".

### 5.4 Transaction titles and subtitles (list mode)

Title priority: address-book/label → service metadata (M5) → contact (M4) → type title. Type titles (new
L10n table `TxTitle`, en from iOS where it exists): Received, Sent, Sent to yourself, Internal transfer,
Mixing, CoinJoin Withdrawals, Masternode reward, Mined, Provider transaction (ProTx), Asset lock.
Subtitle: local time (`h:mm a` / `HH:mm` per locale; date lives in the day header) + one status chip.
dash-qt's "Received with"/"Sent to" + address phrasing is kept verbatim in **table mode** and the detail
sheet.

### 5.5 Addresses, hashes, keys

- Addresses: proportional face (SF / Inter), `footnote` in rows, `subhead` in copy rows. In single-line
  rows: middle truncation by layout (`truncationMode(.middle)`); for a fixed string (Cross, toasts,
  notifications) `prefix(12) + "…" + suffix(12)` when longer than 24 characters (iOS confirm sheet rule).
  Full address where the user verifies it (receive card, confirm "Pay to" tooltip + detail, request
  dialog), wrapping, selectable.
- Hashes (txid, ProTx, block), keys, WIFs, BLS keys, scripts, base64 PSBT, JSON payloads: monospaced in
  the platform mono face (SF Mono on macOS via `design: .monospaced`; the toolkit `monospace` family on
  GTK/WinUI — no extra font is bundled), `footnote`, selectable, with a copy button. Long values wrap or
  middle-truncate with the full value in the tooltip.
- Labels never show "(no label)" in list rows (that string is for dash-qt tables only).

### 5.6 Technical views (the monospaced allowlist)

Monospaced type is allowed **only** for the value kinds in §5.5 (hashes, keys, scripts, payloads, console
output) and in: Tools ▸ Console, raw transaction views, PSBT payload box, command-line options text,
operator-key/shared-MN envelopes. Amounts and addresses are never monospaced (tables use
`monospacedDigit()`). The design review greps for `design: .monospaced` and checks each hit against
this list.

### 5.7 Dates and numbers

- Day headers: "Today", "Yesterday", else `October 4, 2026` (localized `.long` date) + weekday on the right.
- Detail: `Oct 6, 2026 at 1:39 AM` (`.abbreviated` + `.shortened`, locale). Cross today uses
  `en_US_POSIX` "yyyy-MM-dd HH:mm" (`CrossUI/Support.swift` `Format`) → switch to the locale formatter.
- Counts with grouping (`Node Count: 3,512`), percentages one decimal in progress ("47.0%").

---

## 6. Defects in the current app (prioritised)

P0 = breaks the iOS look on a primary screen; P1 = visible inconsistency; P2 = polish.

| # | P | Where | Defect | Fix (section) |
|---|---|---|---|---|
| 1 | P0 | `MacUI/Home/OverviewView.swift`, `DashUIMac/Desktop/BalanceHeader.swift` | Flat card grid (balance card + separate "Balances" grid card + recent card) instead of the iOS blue balance hero with breakdown; hero is a white card with `title1` text | §4.5, C4/C5 |
| 2 | P0 | `OverviewView.swift` (`RecentTransactionRow`), `MenuBar/MenuBarContentView.swift`, `Transactions/TransactionsView.swift` | Amounts in `design: .monospaced` with the "tDASH" suffix, coloured green for incoming; no Dash glyph; 49 `monospaced` uses across 21 UI files | §5 |
| 3 | P0 | `OverviewView.swift`, `TransactionsView.swift` list mode, CrossUI `OverviewScreen.swift` | Raw addresses as row titles ("yemuxfBQzXyP…q83rg2uhUBz5w", "Received with" + address); no day-group cards; absolute dates on every row | §4.5, §4.9, §5.4 |
| 4 | P0 | `OverviewView.swift` | Vendored `TransactionView` (iOS row) exists but is unused; a home-made row with SF Symbol arrows in tinted squares is used instead (Cross: "+"/"-" text tiles, `DashUICross/TransactionView.swift`) | C9 |
| 5 | P0 | app-wide (`Apps/macOS`, scene roots) | System accent colour instead of Dash blue: sidebar selection, "Create a new wallet", "Send", focus rings, toggles render `#3A87DB`-ish (user accent) | §2.8 item 6 |
| 6 | P0 | `MainWindowView.swift` `MainToolbar` + `OverviewView.balanceCard` overlay | Network shown three times (title "[testnet]", toolbar "Testnet" badge, balance-card "Testnet" badge) plus "[demo]" + "Demo" badge | §4.1 |
| 7 | P0 | dark mode, `MainWindowView.walletView` (`Color.dash.primaryBackground`) | Dark canvas is pure #000000 (DashUIKit `primaryBackground` dark) instead of iOS #141519 | §2.1 |
| 8 | P0 | CrossUI all pages (`Support.swift` `Page`, `OverviewScreen.swift`) | No hero, GTK default font (Inter not bundled), status bar is a sentence ("Demo mode: sample data, nothing is sent (passphrase: demo) Testnet #1234567 …"), sidebar "Tools" are bold blue link buttons | §4.1, §2.2, C1, C3 |
| 9 | P1 | `Home/HomeShortcuts.swift` | Four separate white cards with SF Symbols instead of one shortcut card with the iOS 46 pt `shortcut-bar-*` icons; uneven heights ("Scan QR", "1 tDash" tiles are taller) | C6 |
| 10 | P1 | `Options/OptionsView.swift`, `Security/SecurityOptionsTab.swift`, `Settings/SettingsView.swift`, `Transactions/TransactionDetailView.swift`, `Tools/ToolsView.swift` | System `Form { }.formStyle(.grouped)` (grey macOS cards, no icons, raw buttons inside rows) instead of Dash menu cards with 30 pt icons; Settings window shows two rows floating under a large empty band | §4.12, §4.13, C11 |
| 11 | P1 | `Send/SendView.swift` + confirm | Pay-to field monospaced (`:238`); "Coin Control Features…" in the footer; confirm sheet is the dash-qt paragraph text with no amount header or rows; "Send (3)" disabled-grey button | §4.7 |
| 12 | P1 | `Receive/ReceiveView.swift` | Address monospaced (`:55`); action buttons truncate ("Copy Addr…", "New Addre…"); amount field styled differently from Send's; table header plain | §4.8 |
| 13 | P1 | `Lock/LockScreenView.swift` | Plain grey page with a blue padlock; "Unlock" disabled grey, "Quick Receive" bordered; not the iOS blue lock screen with logo and white actions | §4.4 |
| 14 | P1 | `Onboarding/OnboardingView.swift:105`, `Shell/ShellViews.swift:69,269`, `App/DashWalletScenes.swift:95` | SF Symbol `d.circle.fill` used as the Dash logo instead of the wordmark/`dash-logo-square`; network picker + phrase length on the welcome card; system grey segmented controls; phrase as 12 separate tiles; splash progress bar grey | §4.2, §4.3, C30 |
| 15 | P1 | `Transactions/TransactionDetailView.swift` | Form sheet with "Close" button, raw full addresses right-aligned, disabled action buttons always visible, no amount header | §4.9 |
| 16 | P1 | `Main/SyncDetails.swift` overlay | Scrim is opaque mid-grey; labels bold secondary; no progress bar | §4.6 |
| 17 | P1 | `Wallets/WalletsView.swift` | Grey selected row, monospaced wallet id, unordered action bar of seven equal buttons | §4.17 |
| 18 | P1 | `AddressBook/AddressBookView.swift` | Dense table with monospaced addresses and a disabled Copy/Edit/Delete button bar | §4.10 |
| 19 | P1 | `SignVerify/SignVerifyView.swift` | Address placeholder monospaced; system `TabView`; message editor is a bare rectangle | §4.11 |
| 20 | P1 | `CoinControl/CoinControlView.swift` | Amount column monospaced (use tabular digits); radio buttons for Tree/List; summary not grouped | §4.14 |
| 21 | P1 | Cross `TransactionsScreen.swift`, `Support.swift` `Format.date` | Dates `yyyy-MM-dd HH:mm` en_US_POSIX regardless of locale | §5.7 |
| 22 | P1 | `Resources/Icons/settings-wallets.svg` | Only SVG in the exported set; SwiftCrossUI cannot load it | §2.7 |
| 23 | P1 | `DashUICross` | No icon resources at all; Cross screens are text-only | §2.7 |
| 24 | P2 | `MenuBar/MenuBarContentView.swift` | Monospaced balance and address, raw address as last-tx title, green amount, Testnet + Demo badges | §4.19 |
| 25 | P2 | `Tools/ToolsView.swift` Information | Sections without cards, values not selectable everywhere, block hash not monospaced while datadir is | §4.16 |
| 26 | P2 | Cross `OptionsScreen.swift` | Tabs rendered as blue link text; OK/Cancel in the middle of the page; "Not applicable (SPV)" as a loose card | §4.12 |
| 27 | P2 | `DashUIMac/Desktop/Badge.swift`, `DashUICross/Containers.swift` `DashBadge` | Badge radius 6 / capsule mix; network badge orange-on-orange-10 % (iOS: white on orange capsule) | C20 |
| 28 | P2 | various | "—" placeholders sometimes without the reason tooltip; copy actions without toast feedback | §3.2, §5.3 |

---

## 7. Acceptance checklist (restyle agents and the design reviewer)

Global
- [ ] `Roles.swift`, `Layout.swift` (`DashLayout`, `DashMotion`, `DashElevation`, `DashOpacity`) exist, are
      aliases only (no hex literals; `grep -rn "#[0-9A-Fa-f]\{6\}" Sources/{MacUI,CrossUI,DashUIMac/Desktop,DashUICross}`
      returns nothing) and are tested.
- [ ] Dash blue is the accent on macOS regardless of the system accent (screenshot with the system accent set
      to graphite/red shows blue selection, buttons, toggles, focus rings).
- [ ] Canvas `#F7F7F7` / `#141519`, cards `#FFFFFF` / `#1E1F24` in every screenshot (no pure black page).
- [ ] No `design: .monospaced` outside the §5.6 allowlist; amounts use `AmountText`; tables use
      `monospacedDigit()`.
- [ ] No `.borderedProminent` / `.bordered` / default-styled `Button` in page content (toolbar, menus,
      tables and system alerts excepted); one filledBlue button per surface.
- [ ] No `Form { }.formStyle(.grouped)` in Settings/Security/Options/Tx detail; menu cards with 30 pt icons.
- [ ] Network appears once in the chrome (title tag) + the hero/lock capsule; no toolbar network/demo badges.
- [ ] Every row title is human text (label, type title); no raw address as a title anywhere (grep the
      screenshots for `^y[1-9A-HJ-NP-Za-km-z]{25,}` in titles).
- [ ] Every copy action shows the "Copied" toast; every "—" has a reason tooltip.
- [ ] Every icon-only control has an accessibility label (macOS); Cross has no icon-only controls.
- [ ] Inter bundled and used on Linux/Windows; `DashUICross` ships the PNG icon set; no SVG references.
- [ ] Light and dark screenshots for every screen in §4 (macOS + Linux), committed under
      `docs/screenshots/m3/` (+ `linux/`), each compared side by side with this spec.

Per screen (reviewer ticks each against §4)
- [ ] Overview matches `docs/design/ux/home-light.png` / `home-dark.png`: blue hero band, white largeTitle
      amount with glyph, capsule only off-mainnet, breakdown strip, shortcut card straddling the hero,
      History header, day-group cards with iOS rows; discreet/unknown/syncing/out-of-sync/empty states
      screenshotted.
- [ ] Send form ≤ 640 wide, AddressField, AmountEntry, sticky footer; confirm sheet with the amount header,
      dash-qt rows, Cancel default + 3 s Send countdown; success/failure/unknown result views.
- [ ] Receive: QR card, address CopyRow (no truncated buttons), request form card, history table.
- [ ] Transactions: list mode default (iOS rows, day groups), table mode technical, filters, detail sheet.
- [ ] Lock screen on hero blue with wordmark and white actions; unlock sheet; mixing-only variant.
- [ ] Onboarding: wordmark, primary/secondary buttons, Advanced options disclosure, PhraseGrid, verify chips.
- [ ] Settings: one window, toolbar tabs, menu cards, Reset/Cancel/OK bar; Security menu matches §4.13.
- [ ] Address book, Sign/Verify, Wallets, Coin control, PSBT, Tools, About, Splash/Shutdown, Menu bar
      companion restyled per §4.10–§4.19.
- [ ] M3: Overview CoinJoin card, CoinJoin page (status card + CoinJoin send), CoinJoin settings tab.
- [ ] MG (§4.25): every frame of `docs/design/ux/mg-00` in light and dark, with the states listed per screen
      (loading, empty, error, locked, watch-only, network mismatch, full-node-only "—").
- [ ] dash-qt strings verbatim where research 02 has them (spot-check 10 per screen against research 02).
- [ ] Window resized to 920 × 600 and to 1920 × 1200: no clipped text, no truncated buttons, content column
      centred, hero breakdown stacks below 980 pt.
- [ ] Keyboard: every screen operable without a mouse (Tab order, Return/Escape, ⌘1…N, ⌘C on rows).

---

## Appendix A — every generated colour token (light / dark)

Generated from `Resources/Tokens/tokens.json` (gen-tokens output). `@N%` = alpha. Excluded by the
generator: `Purple` (both catalogs). Use the roles in §2.1 in screen code; this table is for component
authors and reviewers.

### A.1 DashUIKit `Media.xcassets` → `DashColor.<token>`

| Token | Asset | Group | Light | Dark |
|---|---|---|---|---|
| `blueBackground` | BlueBackground | Colors/Background | #008DE4 | #008DE4 |
| `primaryBackground` | PrimaryBackground | Colors/Background | #F5F5F7 | #000000 |
| `secondaryBackground` | SecondaryBackground | Colors/Background | #FFFFFF | #1E1F24 |
| `tertiaryBackground` | TertiaryBackground | Colors/Background | #EBEDEE | #EBEDEE |
| `badgeBackground1` | BadgeBackground1 | Colors/Badge | #3DB58A @10% | #3DB58A @10% |
| `badgeBackground2` | BadgeBackground2 | Colors/Badge | #008DE4 @10% | #008DE4 @10% |
| `badgeBackgroundCont1` | BadgeBackgroundCont1 | Colors/Badge | #3DB58A | #3DB58A |
| `badgeBackgroundCont2` | BadgeBackgroundCont2 | Colors/Badge | #008DE4 | #008DE4 |
| `bottomNavBackground` | BottomNavBackground | Colors/Bottom Nav | #FFFFFF | #141519 |
| `buttonFilledBlueBackground` | ButtonFilledBlueBackground | Colors/Buttons/Styles/FilledBlue | #008DE4 | #008DE4 |
| `buttonFilledBlueBackgroundDisabled` | ButtonFilledBlueBackgroundDisabled | Colors/Buttons/Styles/FilledBlue | #0A0B0D @5% | #FFFFFF @10% |
| `buttonFilledBlueContent` | ButtonFilledBlueContent | Colors/Buttons/Styles/FilledBlue | #FFFFFF | #FFFFFF |
| `buttonFilledBlueContentDisabled` | ButtonFilledBlueContentDisabled | Colors/Buttons/Styles/FilledBlue | #0A0B0D @40% | #FFFFFF @40% |
| `buttonFilledOrangeBackground` | ButtonFilledOrangeBackground | Colors/Buttons/Styles/FilledOrange | #F99168 | #F99168 |
| `buttonFilledOrangeBackgroundDisabled` | ButtonFilledOrangeBackgroundDisabled | Colors/Buttons/Styles/FilledOrange | #0A0B0D @5% | #FFFFFF @10% |
| `buttonFilledOrangeContent` | ButtonFilledOrangeContent | Colors/Buttons/Styles/FilledOrange | #FFFFFF | #FFFFFF |
| `buttonFilledOrangeContentDisabled` | ButtonFilledOrangeContentDisabled | Colors/Buttons/Styles/FilledOrange | #0A0B0D @40% | #FFFFFF @40% |
| `buttonFilledRedBackground` | ButtonFilledRedBackground | Colors/Buttons/Styles/FilledRed | #EB3842 | #EB3842 |
| `buttonFilledRedBackgroundDisabled` | ButtonFilledRedBackgroundDisabled | Colors/Buttons/Styles/FilledRed | #0A0B0D @5% | #FFFFFF @10% |
| `buttonFilledRedContent` | ButtonFilledRedContent | Colors/Buttons/Styles/FilledRed | #FFFFFF | #FFFFFF |
| `buttonFilledRedContentDisabled` | ButtonFilledRedContentDisabled | Colors/Buttons/Styles/FilledRed | #0A0B0D @40% | #FFFFFF @40% |
| `buttonFilledWhiteBackground` | ButtonFilledWhiteBackground | Colors/Buttons/Styles/FilledWhite | #FFFFFF | #FFFFFF |
| `buttonFilledWhiteBackgroundDisabled` | ButtonFilledWhiteBackgroundDisabled | Colors/Buttons/Styles/FilledWhite | #FFFFFF @5% | #FFFFFF @10% |
| `buttonFilledWhiteContent` | ButtonFilledWhiteContent | Colors/Buttons/Styles/FilledWhite | #008DE4 | #008DE4 |
| `buttonFilledWhiteContentDisabled` | ButtonFilledWhiteContentDisabled | Colors/Buttons/Styles/FilledWhite | #FFFFFF @50% | #FFFFFF @50% |
| `buttonPlainBlackContent` | ButtonPlainBlackContent | Colors/Buttons/Styles/PlainBlack | #0A0B0D | #FFFFFF |
| `buttonPlainBlackContentDisabled` | ButtonPlainBlackContentDisabled | Colors/Buttons/Styles/PlainBlack | #0A0B0D @40% | #FFFFFF @40% |
| `buttonPlainBlueContent` | ButtonPlainBlueContent | Colors/Buttons/Styles/PlainBlue | #008DE4 | #008DE4 |
| `buttonPlainBlueContentDisabled` | ButtonPlainBlueContentDisabled | Colors/Buttons/Styles/PlainBlue | #0A0B0D @40% | #0A0B0D @40% |
| `buttonPlainRedContent` | ButtonPlainRedContent | Colors/Buttons/Styles/PlainRed | #EB3842 | #EB3842 |
| `buttonPlainRedContentDisabled` | ButtonPlainRedContentDisabled | Colors/Buttons/Styles/PlainRed | #0A0B0D @40% | #0A0B0D @40% |
| `buttonPlainWhiteContent` | ButtonPlainWhiteContent | Colors/Buttons/Styles/PlainWhite | #FFFFFF | #FFFFFF |
| `buttonPlainWhiteContentDisabled` | ButtonPlainWhiteContentDisabled | Colors/Buttons/Styles/PlainWhite | #FFFFFF @50% | #FFFFFF @50% |
| `buttonStrokeGrayBackgroundDisabled` | ButtonStrokeGrayBackgroundDisabled | Colors/Buttons/Styles/StrokeGray | #0A0B0D @15% | #0A0B0D @15% |
| `buttonStrokeGrayContent` | ButtonStrokeGrayContent | Colors/Buttons/Styles/StrokeGray | #0A0A0D | #0A0A0D |
| `buttonStrokeGrayContentDisabled` | ButtonStrokeGrayContentDisabled | Colors/Buttons/Styles/StrokeGray | #0A0B0D @40% | #0A0B0D @40% |
| `buttonStrokeGrayStroke` | ButtonStrokeGrayStroke | Colors/Buttons/Styles/StrokeGray | #75808A @25% | #75808A @25% |
| `buttonTintedBlueBackground` | ButtonTintedBlueBackground | Colors/Buttons/Styles/TintedBlue | #008DE4 @5% | #008DE4 @5% |
| `buttonTintedBlueBackgroundDisabled` | ButtonTintedBlueBackgroundDisabled | Colors/Buttons/Styles/TintedBlue | #0A0B0D @5% | #0A0B0D @5% |
| `buttonTintedBlueContent` | ButtonTintedBlueContent | Colors/Buttons/Styles/TintedBlue | #008DE4 | #008DE4 |
| `buttonTintedBlueContentDisabled` | ButtonTintedBlueContentDisabled | Colors/Buttons/Styles/TintedBlue | #0A0B0D @40% | #0A0B0D @40% |
| `buttonTintedGrayBackground` | ButtonTintedGrayBackground | Colors/Buttons/Styles/TintedGray | #B0B6BC @10% | #FFFFFF @10% |
| `buttonTintedGrayBackgroundDisabled` | ButtonTintedGrayBackgroundDisabled | Colors/Buttons/Styles/TintedGray | #0A0B0D @5% | #FFFFFF @5% |
| `buttonTintedGrayContent` | ButtonTintedGrayContent | Colors/Buttons/Styles/TintedGray | #0A0B0D | #FFFFFF |
| `buttonTintedGrayContentDisabled` | ButtonTintedGrayContentDisabled | Colors/Buttons/Styles/TintedGray | #0A0B0D @40% | #FFFFFF @10% |
| `buttonTintedWhiteBackground` | ButtonTintedWhiteBackground | Colors/Buttons/Styles/TintedWhite | #FFFFFF @10% | #FFFFFF @10% |
| `buttonTintedWhiteBackgroundDisabled` | ButtonTintedWhiteBackgroundDisabled | Colors/Buttons/Styles/TintedWhite | #FFFFFF @5% | #FFFFFF @5% |
| `buttonTintedWhiteContent` | ButtonTintedWhiteContent | Colors/Buttons/Styles/TintedWhite | #FFFFFF | #FFFFFF |
| `buttonTintedWhiteContentDisabled` | ButtonTintedWhiteContentDisabled | Colors/Buttons/Styles/TintedWhite | #FFFFFF @50% | #FFFFFF @50% |
| `grabberFill` | GrabberFill | Colors/Grabber | #B0B6BC | #FFFFFF @30% |
| `listGiftCardNumberBackground` | ListGiftCardNumberBackground | Colors/List | #F5F6F7 | #FFFFFF @10% |
| `navBackButton` | NavBackButton | Colors/Nav | #141519 | #F5F6F7 |
| `backgroundOverlay` | BackgroundOverlay | Colors/Overlay | #0A0B0D @50% | #0A0B0D @50% |
| `searchBackground` | SearchBackground | Colors/Search | #75808A @10% | #B0B6BC @10% |
| `searchClearIcon` | SearchClearIcon | Colors/Search | #0A0B0D @30% | #FFFFFF @20% |
| `searchIcon` | SearchIcon | Colors/Search | #0A0B0D @50% | #FFFFFF @50% |
| `searchPlaceholder` | SearchPlaceholder | Colors/Search | #0A0B0D @30% | #FFFFFF @50% |
| `searchTextEntered` | SearchTextEntered | Colors/Search | #0A0B0D | #FFFFFF |
| `segmentControlBackground` | SegmentControlBackground | Colors/Segment Control | #FFFFFF | #FFFFFF @20% |
| `segmentControlBackgroundGroup` | SegmentControlBackgroundGroup | Colors/Segment Control | #B0B6BC @20% | #FFFFFF @10% |
| `segmentControlContNotSelected` | SegmentControlContNotSelected | Colors/Segment Control | #0A0B0D @40% | #FFFFFF @40% |
| `segmentControlContSelected` | SegmentControlContSelected | Colors/Segment Control | #0A0B0D | #FFFFFF |
| `segmentControlDivider` | SegmentControlDivider | Colors/Segment Control | #0A0B0D @10% | #FFFFFF @10% |
| `selectBackgroundSelected` | SelectBackgroundSelected | Colors/Select | #008DE4 @5% | #FFFFFF |
| `selectStrokeDefault` | SelectStrokeDefault | Colors/Select | #B0B6BC @30% | #FFFFFF |
| `selectStrokeSelected` | SelectStrokeSelected | Colors/Select | #008DE4 | #FFFFFF |
| `shortcutBarBackground` | ShortcutBarBackground | Colors/Shortcut Bar | #FFFFFF | #141519 |
| `statusBarElements` | StatusBarElements | Colors/Status Bar | #141519 | #FFFFFF |
| `stepperBorder` | StepperBorder | Colors/Stepper | #0A0B0D | #FFFFFF @15% |
| `stepperBorderDisabled` | StepperBorderDisabled | Colors/Stepper | #0A0B0D @5% | #FFFFFF @10% |
| `stepperElement` | StepperElement | Colors/Stepper | #141519 | #FFFFFF |
| `stepperElementDisabled` | StepperElementDisabled | Colors/Stepper | #B0B6BC | #FFFFFF @30% |
| `switchThumbFill` | SwitchThumbFill | Colors/Switch | #FFFFFF | #FFFFFF |
| `switchTrackFillOff` | SwitchTrackFillOff | Colors/Switch | #B0B6BC | #FFFFFF |
| `switchTrackFillOffDisabled` | SwitchTrackFillOffDisabled | Colors/Switch | #0A0B0D @20% | #FFFFFF |
| `switchTrackFillOn` | SwitchTrackFillOn | Colors/Switch | #008DE4 | #FFFFFF |
| `blueText` | BlueText | Colors/Text | #008DE4 | #008DE4 |
| `errorText` | ErrorText | Colors/Text | #EB3842 | #EB3842 |
| `primaryText` | PrimaryText | Colors/Text | #0A0A0D | #FFFFFF @90% |
| `secondaryText` | SecondaryText | Colors/Text | #525C66 | #FFFFFF @80% |
| `successText` | SuccessText | Colors/Text | #3DB58A | #3DB58A |
| `tertiaryText` | TertiaryText | Colors/Text | #75808A | #FFFFFF @60% |
| `whiteText` | WhiteText | Colors/Text | #FFFFFF | #FFFFFF |
| `textFieldCryptoAddressBackground` | TextFieldCryptoAddressBackground | Colors/TextField/CryptoAddress | #B0B6BC @10% | #B0B6BC @10% |
| `textFieldCryptoAddressIcon` | TextFieldCryptoAddressIcon | Colors/TextField/CryptoAddress | #141519 | #FFFFFF |
| `toastBackground` | ToastBackground | Colors/Toast | #0A0B0D @90% | #B0B6BC @10% |
| `toastText` | ToastText | Colors/Toast | #FFFFFF | #FFFFFF |
| `blue` | Blue | Colors/Tokens/Blue | #008DE4 | #008DE4 |
| `blueAlpha10` | BlueAlpha10 | Colors/Tokens/Blue | #008DE4 @10% | #008DE4 @10% |
| `blueAlpha20` | BlueAlpha20 | Colors/Tokens/Blue | #008DE4 @20% | #008DE4 @20% |
| `blueAlpha30` | BlueAlpha30 | Colors/Tokens/Blue | #008DE4 @30% | #008DE4 @30% |
| `blueAlpha40` | BlueAlpha40 | Colors/Tokens/Blue | #008DE4 @40% | #008DE4 @40% |
| `blueAlpha5` | BlueAlpha5 | Colors/Tokens/Blue | #008DE4 @5% | #008DE4 @5% |
| `blueAlpha50` | BlueAlpha50 | Colors/Tokens/Blue | #008DE4 @50% | #008DE4 @50% |
| `blueAlpha90` | BlueAlpha90 | Colors/Tokens/Blue | #008DE4 @90% | #008DE4 @90% |
| `black1000Alpha10` | Black1000Alpha10 | Colors/Tokens/Gray/Black | #0A0B0D @10% | #0A0B0D @10% |
| `black1000Alpha15` | Black1000Alpha15 | Colors/Tokens/Gray/Black | #0A0B0D @15% | #0A0B0D @15% |
| `black1000Alpha20` | Black1000Alpha20 | Colors/Tokens/Gray/Black | #0A0B0D @20% | #0A0B0D @20% |
| `black1000Alpha30` | Black1000Alpha30 | Colors/Tokens/Gray/Black | #0A0B0D @30% | #0A0B0D @30% |
| `black1000Alpha40` | Black1000Alpha40 | Colors/Tokens/Gray/Black | #0A0B0D @40% | #0A0B0D @40% |
| `black1000Alpha5` | Black1000Alpha5 | Colors/Tokens/Gray/Black | #0A0A0D @5% | #0A0A0D @5% |
| `black1000Alpha50` | Black1000Alpha50 | Colors/Tokens/Gray/Black | #0A0B0D @50% | #0A0B0D @50% |
| `black1000Alpha60` | Black1000Alpha60 | Colors/Tokens/Gray/Black | #0A0B0D @60% | #0A0B0D @60% |
| `black1000Alpha70` | Black1000Alpha70 | Colors/Tokens/Gray/Black | #0A0B0D @70% | #0A0B0D @70% |
| `black1000Alpha8` | Black1000Alpha8 | Colors/Tokens/Gray/Black | #0A0B0D @8% | #0A0B0D @8% |
| `black1000Alpha80` | Black1000Alpha80 | Colors/Tokens/Gray/Black | #0A0B0D @80% | #0A0B0D @80% |
| `black1000Alpha90` | Black1000Alpha90 | Colors/Tokens/Gray/Black | #0A0B0D @90% | #0A0B0D @90% |
| `black` | Black | Colors/Tokens/Gray | #0A0A0D | #0A0A0D |
| `black800` | Black800 | Colors/Tokens/Gray | #1F1F24 | #1F1F24 |
| `black900` | Black900 | Colors/Tokens/Gray | #14141A | #14141A |
| `gray100` | Gray100 | Colors/Tokens/Gray | #EAEDED | #EAEDED |
| `gray200` | Gray200 | Colors/Tokens/Gray | #CFCFD6 | #CFCFD6 |
| `gray300Alpha10` | Gray300Alpha10 | Colors/Tokens/Gray/Gray300 | #B0B5BD @10% | #B0B5BD @10% |
| `gray300Alpha20` | Gray300Alpha20 | Colors/Tokens/Gray/Gray300 | #B0B5BD @20% | #B0B5BD @20% |
| `gray300Alpha30` | Gray300Alpha30 | Colors/Tokens/Gray/Gray300 | #B0B5BD @30% | #B0B5BD @30% |
| `gray300Alpha40` | Gray300Alpha40 | Colors/Tokens/Gray/Gray300 | #B0B5BD @40% | #B0B5BD @40% |
| `gray300Alpha5` | Gray300Alpha5 | Colors/Tokens/Gray/Gray300 | #B0B5BD @5% | #B0B5BD @5% |
| `gray300Alpha50` | Gray300Alpha50 | Colors/Tokens/Gray/Gray300 | #B0B5BD @50% | #B0B5BD @50% |
| `gray300Alpha60` | Gray300Alpha60 | Colors/Tokens/Gray/Gray300 | #B0B5BD @60% | #B0B5BD @60% |
| `gray300Alpha70` | Gray300Alpha70 | Colors/Tokens/Gray/Gray300 | #B0B5BD @70% | #B0B5BD @70% |
| `gray300Alpha80` | Gray300Alpha80 | Colors/Tokens/Gray/Gray300 | #B0B5BD @80% | #B0B5BD @80% |
| `gray300Alpha90` | Gray300Alpha90 | Colors/Tokens/Gray/Gray300 | #B0B5BD @90% | #B0B5BD @90% |
| `gray300` | Gray300 | Colors/Tokens/Gray | #B0B5BD | #B0B5BD |
| `gray400Alpha10` | Gray400Alpha10 | Colors/Tokens/Gray/Gray400 | #757F8A @10% | #757F8A @10% |
| `gray400Alpha13` | Gray400Alpha13 | Colors/Tokens/Gray/Gray400 | #757F8A @13% | #757F8A @13% |
| `gray400Alpha25` | Gray400Alpha25 | Colors/Tokens/Gray/Gray400 | #757F8A @25% | #757F8A @25% |
| `gray400` | Gray400 | Colors/Tokens/Gray | #757F8A | #757F8A |
| `gray50` | Gray50 | Colors/Tokens/Gray | #F5F5F7 | #F5F5F7 |
| `gray500` | Gray500 | Colors/Tokens/Gray | #525C66 | #525C66 |
| `green` | Green | Colors/Tokens/Green | #3DB58A | #3DB58A |
| `greenAlpha10` | GreenAlpha10 | Colors/Tokens/Green | #3DB58A @10% | #3DB58A @10% |
| `lightBlue` | LightBlue | Colors/Tokens/Light blue | #78C4F5 | #78C4F5 |
| `lightBlueAlpha10` | LightBlueAlpha10 | Colors/Tokens/Light blue | #78C4F5 @10% | #78C4F5 @10% |
| `orange` | Orange | Colors/Tokens/Orange | #FA9169 | #FA9169 |
| `orangeAlpha10` | OrangeAlpha10 | Colors/Tokens/Orange | #FA9169 @10% | #FA9169 @10% |
| `red` | Red | Colors/Tokens/Red | #EB3842 | #EB3842 |
| `redAlpha10` | RedAlpha10 | Colors/Tokens/Red | #EB3842 @10% | #EB3842 @10% |
| `redAlpha5` | RedAlpha5 | Colors/Tokens/Red | #EB3842 @5% | #EB3842 @5% |
| `white` | White | Colors/Tokens/White | #FFFFFF | #FFFFFF |
| `whiteAlpha10` | WhiteAlpha10 | Colors/Tokens/White | #FFFFFF @10% | #FFFFFF @10% |
| `whiteAlpha15` | WhiteAlpha15 | Colors/Tokens/White | #FFFFFF @15% | #FFFFFF @15% |
| `whiteAlpha20` | WhiteAlpha20 | Colors/Tokens/White | #FFFFFF @20% | #FFFFFF @20% |
| `whiteAlpha30` | WhiteAlpha30 | Colors/Tokens/White | #FFFFFF @30% | #FFFFFF @30% |
| `whiteAlpha40` | WhiteAlpha40 | Colors/Tokens/White | #FFFFFF @40% | #FFFFFF @40% |
| `whiteAlpha5` | WhiteAlpha5 | Colors/Tokens/White | #FFFFFF @5% | #FFFFFF @5% |
| `whiteAlpha50` | WhiteAlpha50 | Colors/Tokens/White | #FFFFFF @50% | #FFFFFF @50% |
| `whiteAlpha60` | WhiteAlpha60 | Colors/Tokens/White | #FFFFFF @60% | #FFFFFF @60% |
| `whiteAlpha70` | WhiteAlpha70 | Colors/Tokens/White | #FFFFFF @70% | #FFFFFF @70% |
| `whiteAlpha80` | WhiteAlpha80 | Colors/Tokens/White | #FFFFFF @80% | #FFFFFF @80% |
| `whiteAlpha90` | WhiteAlpha90 | Colors/Tokens/White | #FFFFFF @90% | #FFFFFF @90% |
| `yellow` | Yellow | Colors/Tokens/Yellow | #FFBF42 | #FFBF42 |
| `yellowAlpha10` | YellowAlpha10 | Colors/Tokens/Yellow | #FFBF42 @10% | #FFBF42 @10% |
| `topper` | Topper | Colors/Tokens/custom | #BCF291 | #BCF291 |
| `uphold` | Uphold | Colors/Tokens/custom | #4ACC69 | #4ACC69 |
| `toolbarRoundBorder` | ToolbarRoundBorder | Colors/Toolbar | #B0B6BC @30% | #FFFFFF @15% |
| `toolbarRoundContent` | ToolbarRoundContent | Colors/Toolbar | #0A0B0D | #FFFFFF |
| `topIntroButtonBackground` | TopIntroButtonBackground | Colors/Top Intro | #B0B6BC @20% | #B0B6BC @10% |
| `topIntroButtonContent` | TopIntroButtonContent | Colors/Top Intro | #141519 | #FFFFFF |

### A.2 dashwallet-ios `SharedAssets.xcassets` → `DashColor.App.<token>`

| Token | Asset | Group | Light | Dark |
|---|---|---|---|---|
| `backgroundColor` | BackgroundColor | Colors | #FFFFFF | #1E1F24 |
| `black` | Black | Colors/Black | #000000 | #000000 |
| `black1000Alpha30` | Black1000Alpha30 | Colors/Black | #0A0B0D @30% | #FFFFFF @30% |
| `black1000Alpha40` | Black1000Alpha40 | Colors/Black | #0A0B0D @40% | #0A0B0D @40% |
| `black1000Alpha5` | Black1000Alpha5 | Colors/Black | #0A0B0D @5% | #FFFFFF @5% |
| `black1000Alpha50` | Black1000Alpha50 | Colors/Black | #0A0B0D @50% | #FFFFFF @50% |
| `black1000Alpha8` | Black1000Alpha8 | Colors/Black | #0A0B0D @8% | #FFFFFF @8% |
| `blackAlpha10` | BlackAlpha10 | Colors/Black | #000000 @10% | #000000 @10% |
| `blackAlpha20` | BlackAlpha20 | Colors/Black | #000000 @20% | #000000 @20% |
| `blackAlpha30` | BlackAlpha30 | Colors/Black | #000000 @30% | #000000 @30% |
| `blackAlpha40` | BlackAlpha40 | Colors/Black | #000000 @40% | #000000 @40% |
| `blackAlpha5` | BlackAlpha5 | Colors/Black | #000000 @5% | #000000 @5% |
| `blackAlpha50` | BlackAlpha50 | Colors/Black | #000000 @50% | #000000 @50% |
| `blackAlpha60` | BlackAlpha60 | Colors/Black | #000000 @60% | #000000 @60% |
| `blackAlpha70` | BlackAlpha70 | Colors/Black | #000000 @70% | #000000 @70% |
| `blackAlpha80` | BlackAlpha80 | Colors/Black | #000000 @80% | #000000 @80% |
| `blackAlpha90` | BlackAlpha90 | Colors/Black | #000000 @90% | #000000 @90% |
| `blue` | Blue | Colors/Blue | #008DE4 | #008DE4 |
| `blueAlpha10` | BlueAlpha10 | Colors/Blue | #008DE4 @10% | #008DE4 @10% |
| `blueAlpha20` | BlueAlpha20 | Colors/Blue | #008DE4 @20% | #008DE4 @20% |
| `blueAlpha30` | BlueAlpha30 | Colors/Blue | #008DE4 @30% | #008DE4 @30% |
| `blueAlpha40` | BlueAlpha40 | Colors/Blue | #008DE4 @40% | #008DE4 @40% |
| `blueAlpha5` | BlueAlpha5 | Colors/Blue | #008DE4 @5% | #008DE4 @5% |
| `blueAlpha50` | BlueAlpha50 | Colors/Blue | #008DE4 @50% | #008DE4 @50% |
| `blueAlpha90` | BlueAlpha90 | Colors/Blue | #008DE4 @90% | #008DE4 @90% |
| `blueGradientStartColor` | BlueGradientStartColor | Colors | #00BBE4 | #00BBE4 |
| `buttonRedColor` | ButtonRedColor | Colors | #EA3943 | #EA3943 |
| `chevronColor` | ChevronColor | Colors | #D9D9D9 | #D9D9D9 |
| `topper` | Topper | Colors/Custom | #BCF292 | #BCF292 |
| `uphold` | Uphold | Colors/Custom | #49CC68 | #49CC68 |
| `darkBlueColor` | DarkBlueColor | Colors | #011F5F | #011F5F |
| `dashBlueColor` | DashBlueColor | Colors | #008DE4 | #008DE4 |
| `dashNavigationBarBlueColor` | DashNavigationBarBlueColor | Colors | #008DE3 | #008DE4 |
| `declineButtonColor` | DeclineButtonColor | Colors | #D6D6D6 | #D6D6D6 |
| `disabledButtonColor` | DisabledButtonColor | Colors | #DBDBDB | #585858 |
| `disabledButtonTextColor` | DisabledButtonTextColor | Colors | #FFFFFF | #9B9B9B |
| `black1000Alpha10` | Black1000Alpha10 | Colors/Gray/Black | #0A0B0D @10% | #0A0B0D @10% |
| `black1000Alpha15` | Black1000Alpha15 | Colors/Gray/Black | #0A0B0D @15% | #0A0B0D @15% |
| `black1000Alpha20` | Black1000Alpha20 | Colors/Gray/Black | #0A0B0D @20% | #0A0B0D @20% |
| `black1000Alpha60` | Black1000Alpha60 | Colors/Gray/Black | #0A0B0D @60% | #0A0B0D @60% |
| `black1000Alpha70` | Black1000Alpha70 | Colors/Gray/Black | #0A0B0D @70% | #0A0B0D @70% |
| `black1000Alpha80` | Black1000Alpha80 | Colors/Gray/Black | #0A0B0D @80% | #0A0B0D @80% |
| `black1000Alpha90` | Black1000Alpha90 | Colors/Gray/Black | #0A0B0D @90% | #0A0B0D @90% |
| `black800` | Black800 | Colors/Gray | #1E1F24 | #1E1F24 |
| `black900` | Black900 | Colors/Gray | #141519 | #141519 |
| `gray100` | Gray100 | Colors/Gray | #EBEDEE | #EBEDEE |
| `gray200` | Gray200 | Colors/Gray | #CED2D5 | #CED2D5 |
| `gray300Alpha10` | Gray300Alpha10 | Colors/Gray/Gray300 | #B0B6BC @10% | #B0B6BC @10% |
| `gray300Alpha20` | Gray300Alpha20 | Colors/Gray/Gray300 | #B0B6BC @20% | #FFFFFF @10% |
| `gray300Alpha30` | Gray300Alpha30 | Colors/Gray/Gray300 | #B0B6BC @30% | #B0B6BC @30% |
| `gray300Alpha40` | Gray300Alpha40 | Colors/Gray/Gray300 | #B0B6BC @40% | #B0B6BC @40% |
| `gray300Alpha5` | Gray300Alpha5 | Colors/Gray/Gray300 | #B0B6BC @5% | #B0B6BC @5% |
| `gray300Alpha50` | Gray300Alpha50 | Colors/Gray/Gray300 | #B0B6BC @50% | #B0B6BC @50% |
| `gray300Alpha60` | Gray300Alpha60 | Colors/Gray/Gray300 | #B0B6BC @60% | #B0B6BC @60% |
| `gray300Alpha70` | Gray300Alpha70 | Colors/Gray/Gray300 | #B0B6BC @70% | #B0B6BC @70% |
| `gray300Alpha80` | Gray300Alpha80 | Colors/Gray/Gray300 | #B0B6BC @80% | #B0B6BC @80% |
| `gray300Alpha90` | Gray300Alpha90 | Colors/Gray/Gray300 | #B0B6BC @90% | #B0B6BC @90% |
| `gray300` | Gray300 | Colors/Gray | #B0B6BC | #B0B6BC |
| `gray400Alpha10` | Gray400Alpha10 | Colors/Gray/Gray400 | #75808A @10% | #75808A @10% |
| `gray400Alpha13` | Gray400Alpha13 | Colors/Gray/Gray400 | #75808A @13% | #75808A @13% |
| `gray400Alpha25` | Gray400Alpha25 | Colors/Gray/Gray400 | #75808A @25% | #75808A @25% |
| `gray400` | Gray400 | Colors/Gray | #75808A | #75808A |
| `gray50` | Gray50 | Colors/Gray | #F5F6F7 | #F5F6F7 |
| `gray500` | Gray500 | Colors/Gray | #525C66 | #525C66 |
| `grayButtonColor` | GrayButtonColor | Colors | #191C1F @4% | #202326 |
| `green` | Green | Colors/Green | #3DB58A | #3DB58A |
| `greenAlpha10` | GreenAlpha10 | Colors/Green | #3DB58A @10% | #3DB58A @10% |
| `iconTintColor` | IconTintColor | Colors | #000000 | #FFFFFF |
| `lightBlue` | LightBlue | Colors/LightBlue | #78C4F5 | #78C4F5 |
| `lightBlueAlpha10` | LightBlueAlpha10 | Colors/LightBlue | #78C4F5 @10% | #78C4F5 @10% |
| `lightBlueButtonColor` | LightBlueButtonColor | Colors | #008DE4 @8% | #008DE4 @8% |
| `modalDimmingColor` | ModalDimmingColor | Colors | #04040F @40% | #04040F @40% |
| `numberKeyboardHighlightedTextColor` | NumberKeyboardHighlightedTextColor | Colors | #FEFFFF | #FEFFFF |
| `numberKeyboardTextColor` | NumberKeyboardTextColor | Colors | #4A4A4A | #DBDBDB |
| `orange` | Orange | Colors/Orange | #FA9169 | #FA9169 |
| `orangeAlpha10` | OrangeAlpha10 | Colors/Orange | #FA9169 @10% | #FA9169 @10% |
| `pinBackgroundColor` | PinBackgroundColor | Colors | #D7D7D7 | #D7D7D7 |
| `pinInputDotColor` | PinInputDotColor | Colors | #FFFFFF | #FFFFFF |
| `progressBackgroundColor` | ProgressBackgroundColor | Colors | #DBDBDB | #DBDBDB |
| `quaternaryFillColor` | QuaternaryFillColor | Colors | #707072 @7% | #747476 @18% |
| `red` | Red | Colors/Red | #EB3842 | #EB3842 |
| `redAlpha10` | RedAlpha10 | Colors/Red | #EB3842 @10% | #EB3842 @10% |
| `redAlpha5` | RedAlpha5 | Colors/Red | #EB3842 @5% | #EB3842 @5% |
| `redColor` | RedColor | Colors | #D0021B | #D0021B |
| `searchBackground` | SearchBackground | Colors | #75808A @10% | #75808A @10% |
| `secondaryBackgroundColor` | SecondaryBackgroundColor | Colors | #F7F7F7 | #141519 |
| `segmentSliderColor` | SegmentSliderColor | Colors | #DDDDDD | #DDDDDD |
| `separatorLineColor` | SeparatorLineColor | Colors | #D5D5D5 | #4A4A4A |
| `shadowColor` | ShadowColor | Colors | #000000 | #000000 |
| `shortcutSpecialBackgroundColor` | ShortcutSpecialBackgroundColor | Colors | #D7D7D7 | #1D1D1D |
| `greenColor` | GreenColor | Colors/System | #3EB489 | #3EB489 |
| `orangeColor` | OrangeColor | Colors/System | #FA9269 | #FA9269 |
| `systemRedColor` | SystemRedColor | Colors/System | #DA2C43 | #DA2C43 |
| `systemYellowColor` | SystemYellowColor | Colors/System | #FFC043 | #FFC043 |
| `tabbarBorderColor` | TabbarBorderColor | Colors | #EAEAEA | #494949 |
| `tabbarInactiveButtonColor` | TabbarInactiveButtonColor | Colors | #C6C6C6 | #C6C6C6 |
| `tertiaryBackgroundColor` | TertiaryBackgroundColor | Colors | #FAFAFA | #1D2023 |
| `darkTitleColor` | DarkTitleColor | Colors/TextColors | #000000 | #F0F0F0 |
| `label` | Label | Colors/TextColors | #0A0B0D | #FFFFFF |
| `lightTitleColor` | LightTitleColor | Colors/TextColors | #FFFFFF | #E5E5E5 |
| `quaternaryTextColor` | QuaternaryTextColor | Colors/TextColors | #9B9B9B | #9B9B9B |
| `secondaryTextColor` | SecondaryTextColor | Colors/TextColors | #525C66 | #A4ABB3 |
| `tertiaryTextColor` | TertiaryTextColor | Colors/TextColors | #75808A | #FFFFFF @60% |
| `tintColor` | TintColor | Colors | #FFFFFF | #E6E6E6 |
| `white` | White | Colors/White | #FFFFFF | #FFFFFF |
| `whiteAlpha10` | WhiteAlpha10 | Colors/White | #FFFFFF @10% | #FFFFFF @10% |
| `whiteAlpha15` | WhiteAlpha15 | Colors/White | #FFFFFF @15% | #FFFFFF @15% |
| `whiteAlpha20` | WhiteAlpha20 | Colors/White | #FFFFFF @20% | #FFFFFF @20% |
| `whiteAlpha30` | WhiteAlpha30 | Colors/White | #FFFFFF @30% | #FFFFFF @30% |
| `whiteAlpha40` | WhiteAlpha40 | Colors/White | #FFFFFF @40% | #FFFFFF @40% |
| `whiteAlpha5` | WhiteAlpha5 | Colors/White | #FFFFFF @5% | #FFFFFF @5% |
| `whiteAlpha50` | WhiteAlpha50 | Colors/White | #FFFFFF @50% | #FFFFFF @50% |
| `whiteAlpha60` | WhiteAlpha60 | Colors/White | #FFFFFF @60% | #FFFFFF @60% |
| `whiteAlpha70` | WhiteAlpha70 | Colors/White | #FFFFFF @70% | #FFFFFF @70% |
| `whiteAlpha80` | WhiteAlpha80 | Colors/White | #FFFFFF @80% | #FFFFFF @80% |
| `whiteAlpha90` | WhiteAlpha90 | Colors/White | #FFFFFF @90% | #FFFFFF @90% |
| `yellow` | Yellow | Colors/Yellow | #FFC043 | #FFC043 |
| `yellowAlpha10` | YellowAlpha10 | Colors/Yellow | #FFC043 @10% | #FFC043 @10% |

## Appendix B — implementation notes for the restyle

- macOS offscreen visual QA only (repo `CLAUDE.md`): `ScreenTests.capture` / `ImageRenderer`,
  `DWD_WRITE_SCREENSHOTS=1`; Linux via `scripts/crossui-linux-demo.sh` in Docker.
- Re-record `DashUIMacSnapshotTests` only for intended changes; add gallery entries
  (`DashUIMacGallery.swift`, `DashUICross/Gallery.swift`) for every new component in §3 (C4–C8, C10, C12,
  C15, C17, C19–C30) in light and dark.
- Reference mock generator: the PNGs in `docs/design/ux/` were drawn from `tokens.json` and
  `Resources/Icons` with a throwaway PIL script; they are illustrations, not snapshot baselines.
- Order of work that keeps the app shippable: tokens (§2.8) → AmountText + TransactionRow + MenuCard →
  Overview → Send/Receive/Transactions → Settings/Security → remaining screens → M3 screens.
