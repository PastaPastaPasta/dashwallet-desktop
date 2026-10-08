# dashwallet-desktop — UX specification (visual and interaction source of truth)

Status: **authoritative for visuals and interaction**, 2026-10-06 (M3). Product scope, architecture and
feature rules stay with `DESIGN.md` → `DESIGN-opus.md`. Where a screen in this file disagrees with a
dash-qt *string, limit or formula* in `docs/research/02-dash-qt-features.md`, research 02 wins for the
string or number, and this file wins for layout, styling and interaction.

Who uses this file:
- the **restyle agents** (MacUI + DashUIMac, CrossUI + DashUICross) — sections 2–5;
- the **design reviewer** — sections 6 and 7;
- the **M3 feature agents** (CoinJoin, Masternodes, Governance) — sections 3, 4.21–4.24 and 5.

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
   technical views (transaction table mode, coin control, peers, console, PSBT, raw transaction, keys,
   masternode table) keep their table form, but use the same tokens, fonts and row metrics. They are
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
`list.bullet.rectangle`, CoinJoin `shuffle`, Masternodes `server.rack`, Governance `checkmark.seal`
(already in `MainWindowView.Sidebar.icon`; keep).

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
│ ✓ Governa.*│                                                                   │
├────────────┴───────────────────────────────────────────────────────────────────┤
│ Up to date ▬▬▬▬            tDASH ▾   HD   🔒   ⇄ proxy   ▮▮▮ 8   ◐ gov   ✓     │
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
- **Sidebar**: §C1. Shortcut tooltips "(⌘1)" renumbered by visible items (QT-012, exists). Cross only:
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
  governance clock (§4.24), sync (`checkmark.circle.fill` green / `arrow.triangle.2.circlepath` orange
  spinning). Tooltips verbatim (research 02 §2.3). Cross: the same items as short text ("tDASH", "HD",
  "Locked", "8 peers", "Synced"); in the key-held state "Locked · key held" or "Mixing only · key held".
- **App menus**: dash-qt menus exactly (QT-015…018, `WalletCommands.swift`, `CrossUI/ShellMenus.swift`);
  M3 adds Help ▸ "CoinJoin information", Window ▸ "Masternode Keys" and "Governance information".

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

### 4.22 Masternodes (M3, QT-118…127)

Visibility: Display option "Show Masternodes Tab" (default off), plus shown automatically when the wallet
owns masternodes (iOS "Nodes" shortcut behaviour).
```
TopIntro  Masternodes                                   [Register Masternode…] filledBlue M
                                                        [Shared Masternode…]   tintedBlue M
‹Your masternodes card›  (only when Owned ≥ 1, iOS MasternodesScreen sections)
   (masternode-keys) 203.0.113.7:9999   Evo · [Active] chip        Next payment ~ block 2 345 678  ›
   (masternode-keys) 198.51.100.4:9999  Regular · [PoSe banned]    —                             ›
   Tracked ▸ 2      Retired ▸ 1
Filter bar: (Filter by any property (e.g. address or protx hash)) [All|Regular|Evo|Shared] ☐ Owned ☐ Hide banned
‹DataTable› ● | Service | Type | PoSe Score | Registered | Last Paid | Next Payment | Operator Reward
Footer: Node Count: 3 512
```
- Full-node-only columns (PoSe score, last paid, next payment, operator reward, payout) show "—" with
  tooltip "Requires full-node data source" unless a NodeLink/dashd source is configured
  (DESIGN-opus §1.14). Never estimated.
- Status icon: `circle.fill` `success` active ("Active for X"), `danger` banned ("Banned for X").
- Context menu: dash-qt order (Copy ProTx Hash, Copy Collateral Outpoint, Update Service…, Update
  Registrar…, shared-only items, Revoke…, Filter by ▸). Cross: row "More" menu.
- **Detail sheet** (medium 640; iOS sections): header (status chip, type, service, ProTx hash copy row),
  Registration, Keys (copy rows: owner/voting address, key hashes, operator BLS pubkey, platform node ID,
  payout), Key ownership (Owner/Voting/Operator/Platform node: "In this wallet" / "Not in this wallet"),
  Collateral, Revocation, Shared shares table (shared MNs), Claimable balance (evonode; with
  [Refresh]); actions row: [Update Service…] [Update Registrar…] tintedBlue, [Revoke…] plainRed; evonode:
  [Request status], [Withdraw…], [Unban…] (IOS-081).
- **Register wizard** (sheet 760 × 620, `WizardHeader` "Step %1 of %2 · %3"): Type (two large radio cards
  "Masternode · 1,000 DASH" / "EvoNode · 4,000 DASH") → Collateral (three `RadioButtonRow`s) → Service →
  Keys → Payout (+ reward slider/field, warning > 0) → Platform (Evo) → Fee source → Review (`DetailList`
  + countdown confirm) → Save operator key (technical box with the secret and the
  `masternodeblsprivkey=` line, copy buttons, "Type the last 4 characters" field gating Continue; red
  `SystemMessage` "The secret is never stored.") → Prove ownership (external) → Complete
  (`ResultView` success + ProTx hash copy row + next steps). Footer: [Back] tintedGray L, [Continue]
  filledBlue L. Validation inline per field with dash-qt rules/strings; engine error codes mapped to
  research 02 explanations.
- **Maintenance sheets** (small/medium): Update Service, Update Registrar (warning `SystemMessage` "Changing
  the operator key immediately PoSe-bans the masternode…"), Revoke (reason picker); operator secret
  field = `PassphraseField` (never stored).
- **Shared masternode session** (window, 760 × 640): header with session code `ABC123` and fingerprint
  `XXXX-XXXX` (monospaced, in neutral badges), stage stepper (Invitation → Details → Locked Terms →
  Approvals → Signing → Broadcast), participants `DataTable`, primary area for the current step, actions
  [Copy message] [Save .json…] [Paste message]; close protection alert ("Save session" / "Release
  coins" / Cancel) per research 02 §10.5.
- **Masternode Keys** (Window ▸ Masternode Keys, IOS-083): `SegmentedControl` Owner | Voting | Operator
  (BLS) | Evonode operator (ed25519); list card rows = index, address/pubkey middle-truncated, chips
  "Used at ip:port" (info) / "Revoked" (danger); detail sheet with copy rows (address, public key, legacy
  key, Platform node ID, Tenderdash node key) and private key/WIF behind [Reveal] (auth gate), shown in a
  technical monospaced box with screen-capture guard.
- **Tracked masternodes** (IOS-082): "Tracked" row → list; [+ Track masternode] sheet with SearchBar
  "IP, proTxHash or key" and result card; attach-keys sheet with one `PassphraseField` per key type.
- Empty (no MNs, list syncing): `EmptyState` "No masternodes yet" + iOS explanation strings; list
  loading: `LoadingState` "Loading masternode list…".

### 4.23 Governance (M3, QT-128…134)

Visibility: Display option "Show Governance Tab" (default off).
```
TopIntro  Governance                     [Create Proposal] filledBlue M  [Resume Proposal] tintedBlue M
‹info strip SystemMessage(.info)› Voting deadline: ~3 days left (1 234 blocks, block 2 345 678)  ⓘ
Filter bar: [Active Proposals | My Proposals]   (Filter by Title)            [List | Table]
‹proposal cards list›
  (status chip Passing)  dash-core-group-oct     1,000 Ð × 3 payments · Nov 1 – Jan 30
                         ▮▮▮▮▮▮▮▮▮░░ 512Y · 48N · 12A  (+152)       My votes: 2Y / 1 unvoted   ›
Footer: Proposal Count: 37        [Votes…] (selected proposal)
```
- Status chip colours: Funded `success`, Passing `success` tint, Failing `danger`, Voting `info`,
  Confirming / Pending `warning`, Unfunded `warning`, Lapsed neutral; tooltips from research 02 §11.1.
- Votes bar: proportional Yes/No/Abstain segments (`green` / `red` / `gray300`) + dash-qt text
  `"%1Y, %2N, %3A (%4%5)"`; margin in `success`/`danger`.
- Before governance sync: `LoadingState` "Synchronizing governance objects… N %" (no list, no counts).
  Votes and tallies are SPV-native via govsync once synced; never shown partially as final.
- Table mode: dash-qt columns (Status, Title, Amount, Start, End, Votes, My Votes, Hash).
- **Proposal detail** (sheet medium): title, URL (plainBlue; opening shows the dash-qt "External Link
  Warning" confirmation defaulting to No), `DetailList` (destination, payment amount, payments
  requested, start, end, object hash, parent hash, collateral date/hash — hashes monospaced), votes bar,
  actions [Vote Yes] [Vote No] [Vote Abstain] (tintedBlue/tintedGray), [Copy Raw JSON].
- **Vote sheet** ("Proposal Votes", medium 760): outcome `SegmentedControl` Yes | No | Abstain;
  `DataTable` with checkboxes (Masternode, Voting Address, Weight, Current Vote, Vote Time, ProTx Hash);
  [Select All] [Clear Selection]; "Selected weight: N"; [Vote %1] filledBlue L; result view with
  "Voted successfully %n time(s)" / "Failed to vote %n time(s)" + per-MN errors list. Needs full unlock.
- **Create proposal wizard** (sheet 640, 3 steps): Details (name 1–40 `[-_a-z0-9]`, URL) → Payments
  (payment date picker listing the next 12 superblocks with dates, payments stepper 1–12, address
  field, amount `AmountEntry`, derived total) → Review ([View JSON] [View Payload] technical boxes) →
  confirmation alert "Creating a proposal pays 1 DASH to the network. This fee is non-refundable
  regardless of outcome." → opens Resume. (Fable: the chosen payment date is honoured; dash-qt's bug is
  not copied.)
- **Resume proposals** (sheet medium): list rows with title, URL, payments, collateral hash (copy),
  collateral status chip Unknown/Pending/Ready, [Broadcast] filledBlue S enabled at ≥ 1 confirmation;
  success toast "Proposal has been broadcasted to the network with hash %1".
- **Governance information** (Tools ▸ Information ▸ Governance section, QT-134): cards General,
  Participation, Node, Proposals, Budget; budget donut on macOS (Swift Charts `SectorMark`, colours
  `accent` / `gray300`), Cross fallback = stacked horizontal bar with legend.

### 4.24 Governance clock (status bar, QT-026)

- macOS: SF Symbol `moonphase.*` chosen from the cycle phase (8 phases), `accent` when "awaiting
  superblock"; animated (`arrow.triangle.2.circlepath` pulse) before governance sync. Tooltip = dash-qt
  lines (research 02 §2.3). Click → Governance page.
- Cross: text item "Gov 63 %" / "Gov syncing" with the same tooltip.
- Shown only when governance tab + "Show governance clock" are on and there are peers (dash-qt rule).

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
- [ ] M3: Overview CoinJoin card, CoinJoin page (status card + CoinJoin send), CoinJoin settings tab,
      Masternodes page (owned card + table + detail + register wizard + maintenance sheets + shared session +
      keys + tracked), Governance page (proposal cards + table + detail + vote sheet + create wizard +
      resume), governance clock — each with empty, loading, error and full-node-only ("—" + reason) states.
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
