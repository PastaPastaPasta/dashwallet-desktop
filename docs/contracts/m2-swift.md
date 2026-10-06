# M2 Swift contract (WalletRuntime and PlatformServices seams, view models)

Status: **contract**, 2026-10-06. Code: `Sources/WalletRuntime/Contracts/M2*.swift` (protocols and value types
only), `Sources/PlatformServices/DesktopServices.swift` (OS-service protocols), and the DashKit mapping of the M2
events and errors (`Sources/DashKit/Models.swift`, `DashKitError+M2.swift`). Engine side:
[`m2-engine.md`](m2-engine.md), whose §1 is the **item → owner table** for every M2 checklist item. Everything
in [`m1-swift.md`](m1-swift.md) §1 (layering, `ServiceError`, unknown = `nil`, secrets as `SecretBuffer`,
observation, no singletons) still applies.

## 1. Who builds what

| Owner | Builds |
|---|---|
| **S1** | DashKit wrappers (`EngineProtocol` grows by the M2 calls, one domain per PR; `FakeEngine` and `WalletDemo.DemoEngine` follow the engine's rules for each call they implement), the WalletRuntime adapters for every protocol in §2, the PlatformServices implementations (macOS in `PlatformServicesMac`, Windows/Linux in `PlatformServicesDesktop` over `dw-desktop`), the notification presenter, auto-lock, startup/shutdown coordination and the composition (`AppEnvironment+M2.swift`, append-only per DESIGN-opus §5.3 rule 3). |
| **V1** | The view models in §3 (`Sources/WalletFeatures/**`), against the protocols with fakes until S1's adapters land. Tests are named after checklist ids (`QT091_…`, `IOS034_…`). |
| **U** | MacUI and CrossUI screens over the §3 view models. |

Stubs: until an engine call lands, the adapter surfaces `not_implemented` and the view model shows the feature
as unavailable. It never shows an empty success state.

## 2. Protocols

### 2.1 Wallet lifecycle (`M2WalletLifecycle.swift`)

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `WalletLifecycleManaging` | `existingNetworks()`, `loadStates()`, `loadStateChanges()`, `load`, `unload`, `setLoadOnStartup`, `importWatchOnly(xpub:options:)`, `accountXpub(wallet:account:)`. Load and unload run behind the lifecycle queue without an overlay transition. | `existing_networks`, `wallet_load_states`, `load_wallet`, `unload_wallet`, `set_load_on_startup`, `import_watch_only`, `account_xpub`; event `WalletLoadChanged` | QT-101, QT-114, IOS-009, IOS-110, IOS-111 |

Value types: `WalletLoadState`, `WatchOnlyImportOptions`, `AccountXpub`, `NetworkDataInfo`. `WalletStateProviding`
(M1) lists loaded wallets only; its adapter reloads on `walletLoadChanged`.

### 2.2 Transactions (`M2Transactions.swift`)

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `TransactionActing` | `extras`, `abandon`, `resend`, `dropUnconfirmed(wallet:)`, `exportCSV(wallet:filter:sort:options:) -> Data` (exact dash-qt bytes; `HistoryCSVOptions { unit, typeNames, timeZone }`). | `tx_detail_extras`, `abandon_transaction`, `resend_transaction`, `drop_unconfirmed`, `export_history_csv` | QT-075, QT-090…093, IOS-031/032, IOS-034 |
| `TransactionNotifying` | `batches() -> AsyncStream<TransactionNoticeBatch>`: one batch per engine `NewTransactions`, rows read with `tx_notices`. | `tx_notices`; event `NewTransactions` | QT-031…033, IOS-116 |
| `FeeAndCoinSelectionProviding` | `feePolicy()`, `summary(wallet:outpoints:payAmounts:fee:allChangeToFee:)`. | `fee_policy`, `coin_selection_summary` | QT-057/058, QT-072/074 |

`exportCSV` replaces M1's view-model CSV writer (`TransactionsViewModel.exportCSV`), so the bytes come from one
place with golden vectors. The view model keeps its signature and delegates once the call lands.
`TransactionActionRefusal` (`tx_action.refused`, `parameters["refusal"]`) selects the copy for a refused abandon
or resend.

### 2.3 Compatibility, backups, PSBT (`M2Compat.swift`)

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `WalletFileImporting` | `inspect(_:) -> WalletFileKind`, `importDumpWallet`, `importWalletDat(_:passphrase:options:)`, `importKeyMaterial(_:options:)` → `WalletImportReport`. Runs on the lifecycle queue (transition `.addingWallet`). | `inspect_wallet_file`, `import_dump_wallet`, `import_wallet_dat`, `import_key_material` | QT-106…108 |
| `CoreExporting` | `export(wallet:format:to:grant:) -> CoreExportReport` (`.revealSecret` grant), `mnemonicCompatibility(wallet:)`. | `export_for_core`, `core_mnemonic_compatibility` | QT-109 |
| `BackupProviding` | `backup(wallet:to:passphrase:)`, `restore(from:passphrase:)`, `automaticBackups(wallet:)`, `policy()`, `setKeep(_:)`. | `backup_wallet`, `restore_backup`, `automatic_backups`, `backup_policy`, `set_backup_policy` | QT-110, QT-116 |
| `PSBTHandling` | `createUnsigned(from:)`, `load(_ data:)`, `base64`, `bytes`, `analyze(_:wallet:)`, `sign(_:wallet:grant:)` (`.spend(max: ≥ externalSent)`), `broadcast`, `release`. A `PSBTReference { id, unsignedTxid }` names the engine object the adapter holds, as `PreparedTransaction` does. | `TxDraft.create_unsigned`, `parse_psbt`, `Psbt.*`, `analyze_psbt`, `sign_psbt`, `broadcast_psbt` | QT-076…079 |

`KeyMaterial` payloads are `SecretBuffer`s. File locations are `URL`s; the adapter passes `path`.

### 2.4 Tools window (`M2Tools.swift`)

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `NodeInformationProviding` | `information() -> NodeInformation`, `warnings() -> [NodeWarning]`. | `node_info`, `warnings` | QT-040, QT-143, IOS-107 |
| `PeerModerating` | `disconnect`, `ban(address:for:)`, `unban`, `bannedPeers`. The peer list stays `SyncStatusProviding.peers()`. | `disconnect_peer`, `ban_peer`, `unban_peer`, `banned_peers` | QT-147 |
| `RepairProviding` | `rescanProgress`, `cancelRescan`, `resetChainData` (stop SPV → reset → start, on the lifecycle queue), `setBirthHeight`. Starting a rescan stays `SyncStatusProviding.rescan(from:)`. | `rescan_progress`, `cancel_rescan`, `reset_chain_data`, `set_birth_height` | QT-117, QT-148, IOS-113 |
| `ConsoleExecuting` | `commands()`, `redact(_:)`, `execute(_:wallet:grant:) -> ConsoleResult` (`.output(text:isJSON:)` or `.authorizationRequired(purpose, wallet:)`; the adapter turns the engine's `console.authorization_required` into that case). Lines are `SecretBuffer`s. | `console_commands`, `console_redact`, `console_execute` | QT-145 |
| `LogExporting` | `exportLogs(to:) -> URL`; adds the Swift log file to the engine's zip. | `Engine.export_logs` | IOS-112 |

### 2.5 Security (`M2Security.swift`, `Vault.swift`)

| Protocol | Role | Engine / OS calls | Serves |
|---|---|---|---|
| `QuickUnlockManaging` | `provider`, `policy()`, `enroll(grant:)`, `remove()`, `setSpendLimit(_:grant:)`, `credential(reason:) -> Credential` (biometric prompt → `.quickUnlock(wrapKey:)`). | `enroll_quick_unlock`, `remove_quick_unlock`, `quick_unlock_policy`, `set_quick_unlock_spend_limit`; `BiometricKeyStoring` | IOS-011, IOS-016 |
| `AutoLockControlling` (`@MainActor`) | `interval`, `setInterval(_:)`, `noteActivity()`. Locks on timeout, system sleep and screen lock (`IdleMonitoring`). Default `.never` (dash-qt). | `Vault.lock` (M1) | IOS-015 |
| `VaultRecovering` | `recover(wallet:mnemonic:bip39Passphrase:newPassphrase:)`, `destroy(credential:)`. | `recover_with_mnemonic`, `destroy` | IOS-009, IOS-014, IOS-109 |

`Credential` gains `.quickUnlock(wrapKey:)`. `AuthenticationGate.requirement(for:)` returns
`.quickUnlockOrPassphrase` for `.spend`/`.signMessage` once the vault is enrolled, and `.passphrase` for reveal,
wipe and credential changes. A `vault.quick_unlock_limit_exceeded` or `vault.passphrase_stale` answer makes the
gate ask for the passphrase instead. Rust enforces both rules; the gate's requirement is a UI hint, not the
control (DESIGN-opus §1.8).

### 2.6 Shell (`M2Shell.swift`)

| Protocol | Role | Serves |
|---|---|---|
| `LaunchArgumentsParsing` | `parse(_:) -> LaunchOptions` (`--min`, `--splash`, `--resetguisettings`, `--choosedatadir`, `--datadir`, `--testnet`/`--regtest`/`--devnet`/`--chain`, `--lang`, `--windowtitle`, trailing URIs) and `optionNames` for the help dialog. Errors `launch.unknown_option`, `launch.invalid_value`, `launch.option_after_uri`. | QT-006, QT-153 |
| `StartupProgressing` (`@MainActor`) | `phase`, monotonic `progress`, `changes()`, `requestEmergencyQuit()` (Q on the splash). | QT-005 |
| `ShutdownCoordinating` (`@MainActor`) | `isShuttingDown`, `shutdown()`: SPV stop → session close → settings flush. The shutdown window cannot be closed while it runs. | QT-008 |
| `ShellSettingsProviding` (`@MainActor`) | `ShellSettings { showTrayIcon, minimizeToTray, minimizeOnClose, showCoinJoinNotifications, notificationsEnabled }` in `global.json`. | QT-028…030, QT-033, QT-136, IOS-105 |

QT-007 (corrupt settings): `SettingsStore.recoveredFromCorruption` (M1) is non-empty after it moved a bad file
to `.bak`. The app shows dash-qt's Reset / Abort question before the main window. Abort quits without writing.

### 2.7 OS services (`PlatformServices/DesktopServices.swift`)

Foundation-only protocols. Errors are `PlatformServiceError { code, detail }` with the `desktop.*` codes,
`platform.denied` and `platform.cancelled`. WalletRuntime maps them to `ServiceError` with the same code.

| Protocol | macOS | Windows / Linux | Serves |
|---|---|---|---|
| `SingleInstanceCoordinating` | LaunchServices (always `.primary`; `application(_:open:)` feeds `forwardedArguments()`) | `acquire_single_instance` / `forward_to_primary` | QT-001, QT-019 |
| `URISchemeRegistering` | Info.plist (`registeredAtInstall`) | `register_uri_schemes` (tarball) / installer | QT-150, IOS-048 |
| `LaunchAtLoginManaging` | `isSupported = false` (dash-qt hides it) | `autostart_enabled` / `set_autostart` | QT-009 |
| `TrayControlling` | not used (`MenuBarExtra` + Dock menu) | `TrayIcon` | QT-028…030, IOS-117 |
| `SystemNotifying` | UserNotifications | `DesktopNotifier` | QT-031, IOS-105, IOS-116 |
| `BiometricKeyStoring` | keychain item, `.biometryCurrentSet` | Windows Hello (M6), Linux `none` | IOS-011 |
| `IdleMonitoring` | `NSWorkspace` sleep / screen-lock notifications + idle time | session D-Bus / `WM_POWERBROADCAST` | IOS-015 |
| `ClipboardProviding` | `NSPasteboard` | SwiftCrossUI / GTK clipboard | QT-090, IOS-043 |
| `QRImageDecoding` | Vision or `decode_qr_codes` | `decode_qr_codes` | IOS-043 |
| `DataDirectoryInspecting` | FileManager | FileManager | QT-004 |
| `FileRevealing` | `NSWorkspace.activateFileViewerSelecting` | `xdg-open` / `explorer` | QT-116, QT-143 |
| `ScreenCaptureGuard` (M1) | `sharingType = .none` | `set_window_capture_excluded` (Windows), banner (Linux) | IOS-006 |

`BiometricKey` is the zeroing buffer the key store returns. WalletRuntime wraps it as a `SecretBuffer`.

### 2.8 Errors

`ServiceErrorCode.m2EngineCodes` lists every m2-engine.md §4 code; `SettingsAndCodesTests` compares it with the
table. Swift-side codes added: `launch.unknown_option`, `launch.invalid_value`, `launch.option_after_uri`,
`platform.denied`, `platform.cancelled`. `ServiceError.parameters` gains `limit_duffs`, `size_bytes`,
`duffs_per_kb`, `version` and `refusal`.

## 3. M2 view models (V1) — public API sketch

All are `@MainActor @Observable public final class`, built from `AppEnvironment` (or the specific protocols in
tests). The screens are U's work.

### ShellModel (QT-011, 012, 015…018, 021, 022, 153)
```swift
var windowTitle: String                    // "Dash Wallet - <wallet> - [testnet]" + --windowtitle suffix
var sections: [SidebarItem]                // Overview, Send, Receive, Transactions (+ M3 tabs by option)
var menus: [MenuModel]                     // File/Settings/Window/Help items with enablement and shortcuts
var hdIconVisible: Bool; var lockIcon: LockIcon?   // nil when unencrypted (dash-qt)
var windowGeometry: WindowGeometry?        // saved per window in settings.json
func perform(_ command: ShellCommand) async   // routes menu, tray and Dock items
```
`ShellCommand` covers every dash-qt menu entry (research 02 §2.1). Entries whose feature is not ready are present
and disabled, with the reason in their help text.

### StartupViewModel, DataDirectoryChooserViewModel, ShutdownViewModel (QT-004, 005, 007, 008)
The chooser offers the default or a custom directory, shows `DataDirectoryStatus` with dash-qt's texts, and
creates the directory on OK. Splash: `phase`, `progress`, `quit()`. The corrupt-settings prompt: `reset()` /
`abort()`. Shutdown: `isVisible` while `ShutdownCoordinating.isShuttingDown`.

### OptionsViewModel (QT-135…141, IOS-104/105/107, QT-033)
```swift
var main: MainOptions            // startOnLogin (LaunchAtLoginManaging), showTrayIcon, minimizeToTray, minimizeOnClose
var wallet: WalletOptions        // subtractFeeByDefault, coinControl, psbtControls, keepCustomChangeAddress,
                                 // dustProtection: Amount? (engine), automaticBackups: Int (engine)
var network: NetworkOptionsView  // proxy fields shown, disabled "requires engine update" until U1; numeric-IP validation
var display: DisplayOptions      // language, unit, digits (M1), showMasternodesTab, showGovernanceTab,
                                 // showGovernanceClock, thirdPartyTxURLs ("|"-separated, %s), localCurrency (list only)
var appearance: AppTheme         // light/dark/system (DESIGN.md R2)
var notifications: NotificationOptions  // enabled (+ OS authorization), showCoinJoinNotifications
var restartRequired: Bool
func apply() async throws(ServiceError)   // OK; Cancel = discard()
func discard()
func resetOptions() async throws(ServiceError) -> [URL]   // backs up settings.json/global.json, resets, quits
```
SPV-inapplicable node options are not shown. A footnote lists them (DESIGN-opus §1.14).

### CoinControlViewModel (QT-068…074)
```swift
var mode: ListOrTree; var sort: CoinSort                // persisted
var coins: [Utxo]; var selected: Set<OutPoint>; var showCoinJoinCoins: Bool
var summary: CoinSelectionSummary?                       // FeeAndCoinSelectionProviding.summary, "≈" values
var customChange: String?; var customChangeWarning: CustomChangeWarning?   // invalid / unknown address confirm
var unselectedNotice: Bool                               // "Some coins were unselected because they were spent."
func toggle(_ outpoint: OutPoint); func selectAll(); func lockAll() async; func lock(_:) async; func unlock(_:) async
func copy(_ field: CoinCopyField, of outpoint: OutPoint) -> String
func source() -> CoinSourceChoice                         // .outpoints(selected) or .any
```

### PSBTViewModel (QT-076…079)
`createUnsigned(draft:)` (copies base64 to the clipboard and offers Save), `load(file:)`, `loadFromClipboard()`,
then `analysis`, `statusLine`, `sign(passphrase:)` (`.spend(max: externalSent)`), `broadcast()`, `copy()`,
`save(to:)`.

### Tools: InformationViewModel, ConsoleViewModel, PeersViewModel (M1 + ban), RepairViewModel (QT-143…148)
- Information: `NodeInformation` refreshed on sync changes; `nil` fields show "—", and the full-node rows say so.
- Console: `history` (50 entries, redacted), `walletSelection` (shown with ≥ 2 wallets), `run(line:)`. On
  `.authorizationRequired` the view model asks for the credential, then runs the line again. It also handles
  font size, `clear()` and the anti-scam banner.
- Peers: the M1 list plus `disconnect`, `ban(for:)`, `unban`, `banned`.
- Repair: `rescan(.walletBirth | .genesis)`, `progress`, `cancel()`, `resetChainData()` (with confirmation,
  which dash-qt omits), `dropUnconfirmed()`.

### WalletManagementViewModel (QT-101, 106…110, 114, 116, IOS-009, IOS-110, IOS-111)
```swift
var wallets: [WalletLoadState]
func open(_ id: WalletID) async; func close(_ id: WalletID) async; func closeAll() async
func rename(_ id: WalletID, to: String) async; func remove(_ id: WalletID) async     // .wipe grant
func importFile(_ url: URL) async                       // inspect → dumpwallet / wallet.dat (passphrase) / .dwbackup
func importKeyMaterial(_ kind: KeyMaterialKind, text: String) async
func addWatchOnly(xpub: String, name: String?) async
func backup(_ id: WalletID, to url: URL, passphrase: String?) async
func exportForCore(_ id: WalletID, format: CoreExportFormat, to url: URL) async      // .revealSecret grant
var automaticBackups: [WalletBackup]; func showBackupsFolder()
func xpub(_ id: WalletID) async -> AccountXpub?          // with QR via URIHandling.qrMatrix
var existingData: [NetworkDataInfo]                      // IOS-009 first-run Keep / Delete All
```

### TransactionsViewModel extras (QT-090…093, IOS-027…032, IOS-034)
`extras(for:)`, `abandon`, `resend`, `unlockDust`, `copyRawTransaction`, `copyFullDetails` (dash-qt line format),
`thirdPartyLinks(for:)`, `dayGroups` (IOS-027), iOS category filter chips (IOS-028), CoinJoin rows grouped per day
(IOS-030), `exportCSV` through `TransactionActing`.

### SecurityViewModel (IOS-011, 014…016, 108, 109)
Quick-unlock toggle and limit picker (`QuickUnlockPolicy.spendLimitOptions`), auto-lock picker, "require
authentication for every payment" (M1 setting), `forgotPassphrase` flow (phrase → wallet check → new passphrase,
listing `walletsWithoutSecrets`), and `wipe(confirmation:)`: the typed phrase confirmation, then `remove_wallet`
for each wallet, then `destroy`.

### Smaller view models
- **BackupReminderViewModel** (IOS-005): due 24 h after the first incoming funds while the phrase was never
  revealed or exported. State lives in a `settings.json` section.
- **ShortcutBarViewModel** (IOS-025): 4 slots with state-dependent defaults. Actions whose feature is M4/M5 are
  disabled.
- **AboutViewModel** (IOS-107, QT-153): version, network, data directory, GitHub/support links, log export.
- **FaucetShortcut** (IOS-121): opens the web faucet on testnet. The in-app PoW faucet is AppServices work for
  M5 and is shown as unavailable.
- **NotificationPresenter** (S1, QT-031…033): consumes `TransactionNotifying`. It drops `catchUp` batches, hides
  CoinJoin-internal rows unless enabled, and shows one summary for ≥ 100 rows. Titles "Incoming transaction" /
  "Sent transaction"; the body has Date, Amount, Wallet (multiwallet only), Type, and Label or Address.
- **MenuBarCompanionViewModel** (IOS-117): the M1 companion plus a request amount and pay-from-clipboard.

## 4. S1 implementation status (2026-10-06, branch `m2/s1-desktop-services`)

- **Composition.** `WalletRuntime/M2/DesktopRuntime.swift`: `DesktopRuntimeServices(runtime:platform:…)` builds
  every S1 service over one `WalletRuntimeServices` and a `DesktopOSServices`. `DesktopOSServices` is the OS services
  the app's `@main` picks: `PlatformServicesMac` or `PlatformServicesDesktop`.
- **Done.** `QuickUnlockService`, `VaultRecoveryService`, `AutoLockController` (setting in `settings.json`
  section `autoLock`), `ShellSettingsStore` (`global.json` section `shell`), `TransactionNotificationFeed` +
  `NotificationPresenter` (dash-qt's rules and English texts, localizable through `TransactionNotificationText`),
  `StartupProgress`, `ShutdownCoordinator`, `IncomingURIRouter`, `LaunchArgumentsParser`, `LogExportService` and
  `QRImageImport`.
- **Settings.** `SettingsStore.resetToDefaults()` backs up both files to `.bak` and resets them. It serves QT-007
  Reset, `-resetguisettings` and Options "Reset". `LaunchOptions` gained `showHelp` and `showVersion`
  (`-help`, `-version`).
- **R1/R2 adapters (added at the M2 integration merge).** `DashKit/EngineClient+M2.swift` wraps every R1/R2
  engine call; `WalletRuntime/M2/EngineToolAdapters.swift` implements `WalletLifecycleManaging`,
  `TransactionActing`, `FeeAndCoinSelectionProviding`, `WalletFileImporting`, `CoreExporting`, `BackupProviding`,
  `PSBTHandling`, `NodeInformationProviding`, `PeerModerating`, `RepairProviding`, `ConsoleExecuting` and a
  `DustProtectionService` (V1's `DustProtectionControlling`). `DesktopRuntimeServices` builds them, and
  `WalletFeatures.M2Services.live(desktop:launchOptions:clipboard:)` composes the live `M2Services`
  (`SettingsStore` serves `OptionsResetting` and `PaymentAuthenticationSetting`).
  - Open/close wallet, Dash Core imports, watch-only import and `.dwbackup` restore run on the lifecycle queue
    (`LifecycleQueue.runWalletOperation`) and reload the wallet list; imports and restore show `.addingWallet`.
  - `RepairService.resetChainData` stops SPV, resets and starts SPV again on the queue, even when the reset fails.
  - `exportCSV` writes dates at the time zone's current UTC offset: the engine takes one offset per file, so
    dates on the other side of a DST change are an hour off dash-qt's local time.
  - Peer moderation answers `not_implemented` until upstream U2. `tx_notices` is implemented (R1), so the
  notification feed now shows rows.
  - The app composition roots (MacUI, DashWalletCross) do not build `DesktopRuntimeServices`/`M2Services` yet;
    that is U's wiring with the screens.
- **macOS (`PlatformServicesMac`).**
  - `MacSingleInstance`: the app delegate feeds `application(_:open:)` to `deliver(urls:)`.
  - `MacNotifier` (UserNotifications): `.unavailable` outside an `.app` bundle.
  - `MacBiometricKeyStore` (keychain `.biometryCurrentSet`, LAContext prompt first): needs a signed app with
    keychain-access-groups.
  - `MacQRImageDecoder` (Vision), `MacStatusItemTray`, `MacIdleMonitor`, `MacClipboard`, `MacFileRevealer`, and
    `MacLaunchAtLogin` (`isSupported == false` by default, as dash-qt).
- **Windows/Linux (`PlatformServicesDesktop`).** Wrappers over dw-desktop.
  - `DesktopTray.isAvailable == false`: there is no tray backend.
  - `DesktopIdleMonitor` has no OS source yet, so auto-lock uses only its inactivity timer there.
  - `DesktopBiometricKeyStore.kind == .none`.
  - `CommandLineClipboard` (wl-clipboard / xclip) is Linux only.

## 5. U implementation status, macOS (2026-10-06, branch `m2/mac-ui`)

- **Composition.** `MacAppComposition` builds `DemoEnvironment.makeWithM2` for `--demo` (with the Mac clipboard)
  and, live, `DesktopRuntimeServices` over `PlatformServicesMac` plus `M2Services.live`; the launch runs through
  `StartupProgress.run` so the splash follows the engine. `MacFeatureModels` owns every §3 view model once per run.
  `MainViewModel(env:m2:)` gives the Transactions page the M2 actions.
- **Launch options.** MacUI's `LaunchOptions` keeps its own switches (`--demo`, `--datadir <path>`, …) and passes
  dash-qt's `-min`, `-splash`, `-windowtitle=`, `-choosedatadir`, `-resetguisettings`, `-lang=`, `-help`,
  `-version`, chain options and URIs to `LaunchArgumentsParser` (`runtime`, `argumentError`).
- **Data directory (QT-004).** The chooser opens on `-choosedatadir`, or on the first run when nothing is stored and
  the default directory does not exist; the choice is kept in `UserDefaults` (`DataDirectory`), like dash-qt's
  `strDataDir`.
- **Menus.** File, Settings, Window and Help come from `ShellModel.menus`; About, Options… (Cmd-,) and Exit (Cmd-Q)
  sit in the app menu, Minimize in the system Window menu, as Qt places them on macOS. The Dock menu is dash-qt's
  tray menu.
- **Known limits.** Send has no custom change address yet, so the coin-control panel shows that option disabled
  with the reason. The Network Traffic tab says it is not available. In demo mode PSBT parsing, Dash Core
  imports/exports, backups, watch-only import and log export answer `not_implemented` and the screens show it.
