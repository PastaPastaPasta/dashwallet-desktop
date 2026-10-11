# ADR 0003 — The desktop UI stack (gate G-UI)

- Status: **draft**. G-02 fills the Tauri column, G-03 the SwiftCrossUI column; the manager decides in
  G-04 (DEC-12, DEC-90) under DASHPAY §2.1's rule.
- Gate definition: [`DASHPAY.md` §2.1, "Gate G-UI"](../design/DASHPAY.md) (criteria U1–U8), with the
  G-01 rulings of DEC-36: U4 is judged on UX-SPEC §2.1a's WCAG AA colours; U5 separates the UI's own
  cost from the engine's.
- Tauri inputs: the G-01 spike ([report](../research/g-01-tauri-spike.md)), measured by G-02 on branch
  `dw/g-02-tauri-measure`. Raw data: [`docs/research/g-02-data/`](../research/g-02-data/).

## Results

<!-- G-02 fills the Tauri column, G-03 replaces only the X_… tokens in the SwiftCrossUI column (agreed
2026-10-11). Each side's detail and raw data stay in its own section below. -->

Verdicts apply DASHPAY §2.1's thresholds and DEC-36's rulings. For U5, DEC-36 separates the UI's own
cost from the engine's, so a U5 cell can read "**pass (UI)** / **fail (engine)**". Machines: agentbox
(32 vCPU, no GPU, shared, load 80–150 during these runs), and the GitHub-hosted `ubuntu-latest`,
`windows-latest` and `macos-latest` runners. Each side's method section lists the runner specs.

| # | Criterion (pass) | Platform | Tauri (G-02) | SwiftCrossUI (G-03) |
|---|---|---|---|---|
| U1 | Real engine: balance and first 200 rows identical to `dwcli` | Linux | 20,601,436,392,896 duffs in both; 200/200 rows identical (txid, record index, amount); 40k-tx regtest wallet. **pass** | X_U1_LINUX |
| U2 | Build, install, launch, open the wallet, screenshot; no crash in 10 min | Windows | T_U2_WINDOWS | X_U2_WINDOWS |
| U3a | UX-SPEC §2 checklist per screen (the candidate's column) | Linux | 212/212 computed-style checks in WebKitGTK (the app), Chromium and WebKit (Playwright); 160/160 on the real 40k wallet. **pass** | X_U3A_LINUX |
| U3a | | Windows | T_U3A_WINDOWS | X_U3A_WINDOWS |
| U3a | | macOS | T_U3A_MACOS | X_U3A_MACOS |
| U3b | Fidelity gap against the main spec (recorded) | all | **0** items short. Deliberate choices: the sheet radius is 20 (the Cross dialog value), and the modal reuses the floating elevation | X_U3B |
| U3c | Container boxes within 2 px across engines | all | T_U3C | n/a (Tauri only) |
| U4 | 0 unnamed interactive elements; the walk completes | Linux (AT-SPI) | 0 unnamed (17, 19, 3 and 2 interactive); the whole flow is driven through AT-SPI Actions. axe: 0 serious/critical **including `color-contrast`** on the §2.1a palette (4 screens × light/dark × Chromium/WebKit). **pass** | X_U4_LINUX |
| U4 | UIA walk completes and names every control | Windows (UIA) | T_U4_WINDOWS | X_U4_WINDOWS |
| U4 | Every action reachable by keyboard | all | Tab order: toolbar → sidebar → page; Enter opens a row, Escape closes and returns focus; lock and unlock by keyboard. **pass** | X_U4_KEYBOARD |
| U5 | `history_page(200)` p95 ≤ 20 ms | Linux | UI/IPC share p95 **2.8 ms** on CI Ubuntu and **10.8 ms** on agentbox. The full call's p95 is 90 / 472 ms, of which the engine is 88 / 454 ms (E-HIST). **pass (UI)** / **fail (engine)** | X_U5_HISTORY |
| U5 | Cold start to first paint ≤ 1.5 s (fixture Overview with balance and history) | Linux | **790–805 ms** on CI Ubuntu (load 2, CPU 440–460 ms); **816–2,496 ms** on agentbox at load 87–91 (3 of 5 runs over 1.5 s; CPU 850–1,100 ms). On the 40k wallet: 3.1–3.2 s (CI) and 5.2–12.0 s (agentbox); the engine's own wallet open (`dwcli list`) takes 5.2–9.2 s on agentbox. **pass (UI)** on the quiet runner and on CPU time / **fail (engine)** for wallet open | X_U5_COLD |
| U5 | PSS of all processes, idle, ≤ 400 MB | Linux | Fixture (UI only): **200 MB** on agentbox, **264 MB** on CI. 40k wallet: 414 / 450 MB in total, of which the engine is about 193 / 154 MB (UI share 221 / 296 MB). **pass (UI)**; the total exceeds 400 MB on a 40k wallet (engine share → H-06) | X_U5_PSS |
| U6 | Allowlist, CSP, navigation, XSS, unlisted command | all | T_U6 | n/a (Tauri only) |
| U7 | Each package installs (or unpacks) and launches | Linux | `.deb` (23.6 MB) installs with apt; the installed app passes its self-test; it removes cleanly. The AppImage (104.9 MB) runs and passes its self-test. **pass** | X_U7_LINUX |
| U7 | | Windows | T_U7_WINDOWS | X_U7_WINDOWS |
| U7 | | macOS | T_U7_MACOS | X_U7_MACOS |
| U8 | Build, launch, every screen in light and dark; no crash | macOS | T_U8_MACOS | X_U8_MACOS |

- **Tauri:** T_SUMMARY
- **SwiftCrossUI:** X_SUMMARY

## Tauri: method and raw data (G-02)

TAURI_METHOD

## SwiftCrossUI: method and raw data (G-03)

*(G-03)*

## Decision (G-04)

*(the manager)*
