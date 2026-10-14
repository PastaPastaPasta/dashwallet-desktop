# U-01: shell and information architecture (Tauri)

The window around the screens: the routes of [DASHPAY §4](DASHPAY.md) (R11), the toolbar, the
status bar, the native menus and their shortcuts, the tray, one instance per data root, the saved
window geometry, and deep links (`dash:`, `pay:`, `dashwallet:`, `dashpay:`). Later tasks fill
the routes: U-02..U-06 and the DashPay tasks replace the placeholders.

| Part | Where |
|---|---|
| Link routing, the link queue | `rust/crates/dw-app/src/shell/links.rs` |
| Shell events, the shared state, link intake | `rust/crates/dw-app/src/shell/mod.rs` |
| Menus, shortcuts, tray | `shell/menu.rs`, `shell/tray.rs` |
| Single instance, window geometry | `shell/native.rs` |
| Commands `take_links`, `route_link`, `shell_sync` | `src/api/shell.rs`, `capabilities/shell.json` |
| `--selftest-link-trace`, `selftest_link_routed` | `src/launch.rs`, `src/api/selftest.rs` |
| Scheme registration | `tauri.conf.json` `plugins.deep-link.desktop.schemes` |
| Routes, toolbar, sidebar, status bar | `apps/desktop/src/app/{ui,shellBridge}.ts`, `src/components/shell.tsx`, `src/screens/{Routes,ShellDialogs}.tsx` |
| Tests | Rust unit tests in each module; `e2e/routes.spec.ts`; `e2e-tauri/smoke.ts`; `ci/github/deep-links.sh` |

## Flow of a link

1. The OS hands the link over.
   - Linux and Windows: a launch argument. When the app already runs, the second launch's
     single-instance plugin forwards its whole command line to the first and exits.
   - macOS: `RunEvent::Opened`.
2. The shell keeps the arguments in one of the four schemes. It queues each one (`Inbox`: at most
   16 links, each at most 4096 bytes, zeroed on drop), brings the window forward and sends
   `shell://event {kind: "linksPending"}`.
3. The renderer calls `takeLinks(network)` at start-up and after each `linksPending`. dw-app
   routes each link for the window's network into a `LinkRoute`.
4. The renderer opens the route, or shows a toast saying why it did not.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The toolbar chip is the DashPay identity chip: "Join DashPay", "Registering…" or "@name · Pending", and an avatar with @name, which opens My Profile. Network and sync stay in the title and the status bar. | DASHPAY §4. UX-SPEC §4.1 keeps network badges out of the toolbar. |
| D2 | One instance per data root, through `tauri-plugin-single-instance`. On Linux the D-Bus name is the bundle id, plus a hash of the data root when `--datadir` is given. macOS and Windows key the instance by bundle id: one instance per user. Without a session bus, every launch runs on its own. | dash-qt keys its IPC server by data directory. The plugin's macOS and Windows keys can't be changed. A per-root name lets tests and two wallets run side by side. |
| D3 | No `tauri-plugin-deep-link`. The bundler registers the schemes from `plugins.deep-link.desktop.schemes`: the Linux `.desktop` `MimeType`, the macOS `CFBundleURLTypes`, and the Windows NSIS `Software\Classes\<scheme>`. Links come from argv and from `RunEvent::Opened`. Nothing is registered at run time. | The plugin's Linux and Windows argv handling takes a single argument. Run-time registration writes into the user's `~/.local` and registry. The packages already register the schemes. |
| D4 | Routing happens in Rust. `dash:` and `pay:` go through dash-qt's URI handler for the window's network: `pay:` is read as `dash:`, and iOS `user=` is kept as an unverified payee name. `dashwallet://pay=` is rebuilt as a `dash:` URI and goes through the same handler. `dashpay://user` routes to Add contact. Invitations reach the renderer as a bare `invitation`, since DP5-01 owns intake and stash. `dashwallet://request=address`, DashConnect, integrations and BIP70 are `unsupported`. | One parser for every way in (links, Open URI). Payment rules stay Core-exact. The bearer secret never crosses IPC and is never logged. |
| D5 | Links route even while the vault is locked; the page is there after unlock. | iOS defers links until unlock. The desktop lock screen covers the shell, so routing first gives the same result. |
| D6 | Native menus via `tauri::menu`. One table (`menu::ITEMS`) gives each item's id, label, accelerator and action. The menu bar and the tray menu are built from it. A test checks that ids and accelerators are unique and that every route and Tools tab has a way in. Commands reach the renderer as `shell://event {kind: "command"}`. In a browser, the renderer binds the same keys. | dash-qt's menus (QT-015..018), DASHPAY §4 "Tools, the menus". One source of truth for shortcuts. |
| D7 | Shortcuts: ⌘/Ctrl+1..6 for the sidebar sections, ⌘O Open URI, ⌘L Lock, ⌘⇧D Discreet, ⌘, Settings, ⌘⇧I/C/P/R for Information, Console, Peers and Repair (dash-qt's), ⌘Q Exit (Quit on macOS). None of them shadow the editing keys. | Follows dash-qt and macOS conventions. |
| D8 | Tray: Show or Hide, Pay, Receive, Activity, Lock, Quit. A left click toggles the window on Windows and macOS. On Linux, the tray is left out when no AppIndicator library loads (libappindicator-sys would abort). Closing the window still quits; minimize-to-tray and close-to-tray are U-04 Options (QT-030). | QT-028/029. A missing tray library must not crash the app. |
| D9 | Window geometry is saved via `tauri-plugin-window-state`: size, position, maximised and full screen. With `--datadir` it goes to `<datadir>/window-state.json`. It is skipped with `--window-size` and in the self-test, which need a known size. | QT-011. Keeps the gate deterministic. |
| D10 | Routes: home, pay (To a contact / To an address), receive, contacts, activity, coinJoin, explore, notifications, profile, settings, tools (8 tabs), joinDashPay. The `Route` and `ToolsTab` types are generated from Rust. Overview became Home and Transactions became Activity, with testids `page-home` and `page-activity`; the self-test's screen names are unchanged. | DASHPAY §4 names. One source of truth for menus and the renderer. |
| D11 | The sidebar shows Home, Pay, Receive, Contacts (with a requests badge; Join DashPay when there is no identity or one is still registering), Activity and CoinJoin. Explore is hidden until its task. Masternodes come in 1.1. | DASHPAY §4. dash-qt shows CoinJoin by default. |
| D12 | Placeholders carry `data-testid="placeholder-<route>"`. Pay ▸ To an address already shows the prefilled address, amount, label and message fields, with a disabled Continue until U-03. Add contact shows the link's username and identity, with a disabled Send. | Links have to route end to end now. The real screens replace these pages. |
| D13 | File ▸ Open URI… opens a sheet that calls `route_link`, the same router as clicked links. Drag and drop (QT-019) is deferred. | Reuses D4. Drop needs the webview drop handler; that comes with U-03. |
| D14 | The bell opens Notifications and shows the main identity's `dashpay_unread_count`. The Contacts badge counts the `requests` section. `platform` and `dashPayNews` events invalidate every DashPay shell query. | DASHPAY §4 `DashPayStatusModel`. |
| D15 | The wallet picker is a native `<select>`, shown only with 2+ wallets (QT-014). It switches the wallet the window shows (`ui.walletId`). | dash-qt shows its combo box only with 2+ wallets. |
| D16 | The CI link checks build a debug bundle with the `selftest` feature, install it per OS, open links through the OS (`xdg-open`, `open`, `start`) and read the link trace: `received <source> <scheme>` from Rust and `routed <route>` from the renderer. The trace holds no link contents. | Shows that registration, forwarding and routing work on each OS without a screen reader or WebDriver. |
| D17 | `docs/parity.md` is left as it is. | It measures the Swift apps on the M2 baseline and has no Tauri column; the Tauri re-baseline adds U-01's rows (QT-011/012/014–019/028–030, IOS-048/117) in one pass. |
| D18 | `selftest_link_routed` without `--selftest-link-trace` refuses with `app.not_selftest`. | The code for a self-test command in a normal launch. |
