# G-03 U3 screenshot review (SwiftCrossUI against UX-SPEC's Cross column)

An independent reviewer (Opus) made this review on 2026-10-11 from the gate screenshots alone,
without running the app. It reads only UX-SPEC's Cross column; §3.0's accepted limits of
SwiftCrossUI 0.10 do not count as defects.

**Shots:**
- `agentbox/` (Linux, GtkBackend);
- `macos/` (AppKitBackend);
- `windows/` (WinUIBackend; only the lock screen renders).

**Method:**
- Colours were sampled from the PNGs with ImageMagick. The macOS PNGs carry a "Display" ICC
  profile and were converted to sRGB first.
- Linux font family, size and weight come from `agentbox/styles-*.json`.

**Checked against the window crop:** the black band under the Linux status bar is the harness's
crop, which takes the window's size including area GTK has not drawn. It is not an app defect.

## Checklist (one check per screen per appearance)

**Common to every screen (C1–C9):**
- C1 §2.1 surface roles: canvas F7F7F7/141519, card FFFFFF/1E1F24, cardRaised, hero RoleAA #0063B7.
- C2 §2.1a AA text variants: link #006FC3/#1594EC, warning text #B6542C, a `danger` text variant.
- C3 §2.2 face and scale: Inter on Linux/Windows, system font on macOS; style sizes and weights.
- C4 §2.4 radii of the visible components.
- C5 §2.5 Cross elevation: 1 px border on light cards, none in dark, a border on floating cards.
- C6 §4.1/C3 status bar: text items; order unit → HD → lock → peers → sync; 28 px high; accent
  progress bar.
- C7 C1 sidebar: PNG icon and text; the selected row is an accent pill, r8, with white text.
- C8 C2 header row: lock, discreet and wallet controls, each with icon and text.
- C9 §2.2/§7: no clipped or truncated text.

**Lock (11 checks; C5, C7 and C8 do not apply):**
- L1 hero-blue overlay, white wordmark, capsule.
- L2 title2 title in white, prompt in white at 90 %.
- L3 white passphrase field, 360 wide.
- L4 "For mixing only" checkbox.
- L5 Unlock button, Quick Receive and Scan to Send.

**Overview (18 checks):**
- O1 hero band with the capsule inside it.
- O2 largeTitle amount with the glyph and thin-space grouping.
- O3 "(out of sync)" in `caution`.
- O4 breakdown strip: white 8 %, r12, equal cells, an Immature cell when non-zero, one-line
  subtitles.
- O5 shortcut card: r20, at most 520 wide, straddling the hero edge, 4 items with icons.
- O6 HistoryHeader.
- O7 day-group card, r10, with the §5.7 date.
- O8 rows: 30 pt icon, human title, time, orange "Locked", AmountText `[sign][number][glyph]` in
  `.compact`.
- O9 node-warning banner under the hero.

**Transactions (15 checks):**
- T1 TopIntro.
- T2 List | Table control, List by default.
- T3 Export.
- T4 filter fields in `fieldFill`.
- T5 chips All · Sent · Received · Mixing · Rewards, with the active chip shown.
- T6 rows as O8.

**Detail (16 checks):**
- D1 C23 sheet.
- D2 50 pt icon and title3.
- D3 title1 amount at full precision with the glyph.
- D4 status chip and a date in the "Oct 6, 2026 at 1:39 AM" form.
- D5 value rows: proportional addresses, amounts with the glyph, monospaced txid.
- D6 copy buttons.
- D7 nested `cardRaised` card.

## 1. Per platform, screen and appearance

### Linux (GtkBackend)

The basics are right:
- Inter is used everywhere, and the txid is monospaced.
- Every sampled role colour is exact: canvas, card, cardRaised FAFAFA/1D2023, hero #0063B7, strip
  #146FBD (white at 8 %), sidebar pill #006FC3, links #006FC3/#1594EC, "Locked" #B6542C/#FA9169,
  warning-banner fill.
- All §2.2 type sizes and weights match.

**Lock, light and dark: FAIL, 6/11 each.**
- §2.4: the passphrase field corner is about 4 px; the spec says r16.
- §4.1/C3: the status bar order is HD, Locked, tDASH, 0 peers, Sync details, so the unit is not
  first. The bar is 42 px high; the spec says 28.
- §4.4 L3: the field is #E6E6E6 in light and #1A73BE in dark, the GTK default entry, not white.
- §4.4 L4: there is no "For mixing only" checkbox.
- §4.4 L5: there is no Quick Receive or Scan to Send.
- Cannot judge: Unlock is disabled in the shot, so its enabled style is not visible.

**Overview, light and dark: FAIL, 8/18 each.**
- §2.1a: "Unlocked" in the status bar is `danger` #EB3842 at 12 pt with no AA variant (4.4:1 on
  F7F7F7, about 3.9:1 on 141519).
- §4.1/C3: the status bar order and height are wrong, as on the lock screen.
- C2: there is no header row. Lock is a text-only "Lock Wallet" button in the sidebar, and the
  discreet toggle is a "Hide balance" pill in the hero.
- §2.2/§7 clipping:
  - "Sign / Verify Messa…" is cut off in the sidebar;
  - "Awaiting confirmati-on" breaks across 3 lines.
- §4.5 O1: the REGTEST capsule sits on the canvas above the hero band.
- §4.5/C5 O4:
  - the cells are unequal, about 397 px against 203 px;
  - the subtitle wraps;
  - there is no Immature cell, although every visible row is immature coinbase.
- C6 O5: the 4th shortcut, "Spend", is a blank grey disc with no glyph.
- §5.7 O7: the day header reads "10 February 2026", while Transactions in the same run reads
  "February 10, 2026".
- §5.1 O8: amounts are drawn as "[+0.00000008]"; the brackets are not in the AmountText anatomy.
- §4.5 O9: the pre-release warning spans the whole window top; the spec puts it under the hero.
- Cannot judge: the hero's 8 decimals depend on the Decimal-digits setting.

**Transactions, light and dark: FAIL, 7/15 each.**
- C2, C6 and C8 fail as on Overview.
- §2.4: the search and min-amount fields have corners of about 4 px; the spec says r14.
- C9: the "Type" label is clipped to "Ty…", and the sidebar is truncated.
- §2.1 T4: the fields are the GTK default #E6E6E6/#35363A, not `fieldFill`. There is no leading
  search icon (C18).
- §4.9 T5: the chips are All/Sent/Received only. There is no Mixing chip, and no chip looks
  selected.
- T6: the amounts are bracketed.

**Detail, light and dark: FAIL, 7/16 each.**
- C2, C4, C6, C8 and C9 fail as on Transactions.
- C23/§4.9 D1: the detail is an inline card in the page with a "Close" button, not a `.sheet`.
- D4: there is no status chip. The header date "10 Feb 2026 at 2:16 AM" does not match the row's
  "Feb 10, 2026 at 2:16 AM".
- §5.1 D5: Credit and Net amount read "+0.00000008 tDASH", with the unit name instead of the glyph.
- D6: there is no copy button beside To. The txid row sits under the status bar.

**Linux total: 56/120.**

### macOS (AppKitBackend)

Layout and type match Linux, with the system font. "Sign / Verify Message" and "Type" do not
truncate. The day header is consistently "February 10, 2026".

**Lock: FAIL, 6/11 in light and 5/11 in dark.**
- C1: the hero measures #0079C3 in sRGB.
- §2.4: the field corner is about 4 px.
- C6: the status bar order is wrong, the bar is 33 px high, and the progress bar uses the system
  accent #007AFE.
- L4 and L5 fail as on Linux.
- Dark L3: the field is #1E1E1E.

**Overview, light and dark: FAIL, 8/18 each.**
- C1: every colour is shifted: canvas F9F9F9 in light, 1A1C21 / card 28292F in dark, hero 0079C3.
- C2: links are #0084CD (about 3.8:1 on canvas), "Locked" is #C46938 (about 3.9:1 on white), and
  red is #F15152.
- Linux renders the same tokens exactly, so this is probably AppKitBackend converting colours in a
  non-sRGB space. That is not provable from a PNG.
- C7: the selected sidebar row is a grey pill (#DCDCDC in light, #464646 in dark) with white text,
  close to unreadable in light.
- C6, C8, O1, O4, O5, O8 and O9 fail as on Linux.

**Transactions, light and dark: FAIL, 6/15 each.**
- C1, C2, C6, C7 and C8 fail as on Overview.
- C4/T4: the fields are AppKit bordered rectangles, with no fieldFill, no r14 and no search icon.
- T5 and T6 fail as on Linux.

**Detail, light and dark: FAIL, 6/16 each.**
- C1, C2, C4, C6, C7 and C8 fail as above.
- D1, D4, D5 and D6 fail as on Linux.
- The header date has the correct form.

**macOS total: 51/120.**

### Windows (WinUIBackend)

**Lock, light and dark: FAIL, 4/9 each.**
- C6 and C9 are not counted: the window extends below the taskbar or past the screen edge, so the
  status bar is not visible.
- Passes: hero #0063B7, capsule #B6542C, wordmark, title, prompt.
- §2.2: the face looks like Segoe UI, not Inter. It is about 7 % narrower, with Segoe glyph shapes.
  There is no font dump to confirm this.
- §2.4: the field corner is about 4 px.
- L3: the field is translucent.
- L4 and L5 are missing.

**Overview, Transactions and Detail, light and dark: not rendered.** All six processes exit with
0xC00000FD (stack overflow) before they paint. Their PNGs are desktop captures.

**Windows total: 8/18,** on the lock screen only.

## 2. Totals

| Platform | Checks passed | Note |
|---|---|---|
| Linux | 56/120 | |
| macOS | 51/120 | |
| Windows | 8/18 | lock only; 6 of 8 shots crashed |

## 3. U3(b): fidelity gap against the main spec

| # | Item | Status |
|---|---|---|
| 1 | No shadows; hairline borders instead | permitted (§2.5, §3.0) |
| 2 | Circular, not continuous, corners | permitted (§2.4) |
| 3 | The sidebar is a custom List on canvas, not system sidebar material | permitted (§2.1 `sidebar` Cross, C1) |
| 4 | The status bar uses text items, not symbols | permitted (C3, §4.1); its order and 28 px height are **not** |
| 5 | Inter on Linux, not SF | required (§2.2); Windows apparently not using Inter is **not permitted** |
| 6 | No line height, so a different vertical rhythm | permitted (§2.2, §3.0) |
| 7 | A hover tint instead of press or scale effects; no motion | permitted (§2.6, §3.0) |
| 8 | No accessibility labels, so controls are icon and text | permitted (§1.8, ADR A1) |
| 9 | Context menus become a "More" menu | permitted (§3.0); not visible in the shots |
| 10 | Toolkit-native drop-downs and unit picker | permitted (§3.0 Picker) |
| 11 | No toolbar or header row; lock is in the sidebar; the discreet toggle is a persistent "Hide balance" pill | **not permitted** (C2 Cross, §4.5) |
| 12 | The transaction detail is inline, not a sheet | **not permitted** (C23 Cross `.sheet`) |
| 13 | The text fields are toolkit defaults (r≈4, wrong fill), and search has no icon | **not permitted** (§2.4, C16, C18) |
| 14 | The lock screen has no mixing-only checkbox, no Quick Receive or Scan to Send, and the field is not white | **not permitted** (§4.4) |
| 15 | Amounts are bracketed ("[+0.00000008]") | **not permitted** (§5.1) |
| 16 | The breakdown strip has unequal cells, a wrapped subtitle and no Immature cell | **not permitted** (C5, §4.5) |
| 17 | The capsule is outside or straddling the hero | **not permitted** (§4.5, C4) |
| 18 | The node-warning banner is at the window top | **not permitted** (§4.5) |
| 19 | The disabled shortcut has no glyph | **not permitted** (C6) |
| 20 | The detail has no status chip, and the filter chips lack Mixing and an active state | **not permitted** (§4.9) |
| 21 | Detail amounts use "tDASH", not the glyph | **not permitted** (§5.1) |
| 22 | Linux date formats differ between screens | **not permitted** (§5.7) |
| 23 | "Unlocked" is red text without an AA variant | **not permitted** (§2.1a) |
| 24 | macOS colours are shifted; links and "Locked" fall below 4.5:1 | **not permitted** (§2.1, §2.1a) |
| 25 | The macOS sidebar selection is grey with white text | **not permitted** (C1) |
| 26 | The macOS progress bar uses the system accent | **not permitted** (§2.8.6, C27) |
| 27 | Linux sidebar labels truncate | **not permitted** (§2.2) |
| 28 | The detail's label-left grid is closer to the main spec than Cross C12 ("stacked only") | a Cross deviation, not a gap |
| 29 | Windows: 6 of 8 shots crash | functional, not styling |

## 4. Rendering bugs

- **Capsule outside the hero band:** Linux overview at (646–697, 60–77); macOS at (597–669, 93–110).
- **Hyphenated breakdown subtitle:** Linux overview at (783, 200–240).
- **Sidebar "Sign / Verify Messa…":** every Linux main-window shot, at (53, 407).
- **"Ty…" label:** Linux transactions and detail, at (372, 193).
- **Blank "Spend" disc:** Linux at (864, 318); macOS at (828, 346).
- **macOS light sidebar selection:** white on #DCDCDC.
- **macOS colour shift:** on every screen.
- **Linux dark overview:** the hover tint on Send is captured in dark only.
- **Linux dates:** "10 February 2026" against "February 10, 2026", and "10 Feb 2026" against "Feb 10,
  2026" in the detail.
- **"40053 transactions":** ungrouped, in `styles-*-transactions.json` off-screen; §5.7 wants
  "40,053".
- **Windows:**
  - The window is placed partly off-screen.
  - The later PNGs are desktop captures.
