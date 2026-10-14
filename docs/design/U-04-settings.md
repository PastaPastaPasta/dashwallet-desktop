# U-04: Settings and Options, Security, Wallets, About, backups, imports and exports (Tauri)

ROADMAP U-04 for the Tauri renderer. Settings is one route (`settings`, ⌘/Ctrl+,) with five tabs:
**General · Security · Wallets · Backups · About**. It merges dash-qt's Options dialog (QT-135…141), its
Settings and File menus' wallet actions (QT-015/016, QT-101…117) and the iOS Settings, Security and
Wallets screens (IOS-104…111) into the mobile information architecture (DASHPAY §4, R11; UX-SPEC §4.12,
§4.13, §4.17, §4.18). As in U-02 and U-03, every engine call goes through dw-app's generated commands
(E0-13). U-04 adds two commands of the app's own, `ui_settings` and `set_ui_settings` (D2), and no engine
call. A row the engine or the shell cannot serve yet shows as **not yet**: it names the task that brings
it (ROADMAP §0) and is never stubbed.

| Part | Where |
|---|---|
| View models (no React) | `apps/desktop/src/settings/{prefs,options,security,wallets,files,autoLock}.ts` |
| Their tests (by parity ID) | `apps/desktop/src/settings/*.test.ts` |
| Screens | `src/screens/settings/{SettingsPage,General,Security,Wallets,Backups,About}.tsx` and their sheets |
| UI settings file | dw-app `api/app.rs` (`ui_settings`, `set_ui_settings`), `<data root>/settings.json` |
| Fixture handlers | `src/fixture/settings.ts` (wallet lifecycle, backups, imports, exports, vault credential changes, UI settings) |
| Playwright | `e2e/settings.spec.ts` (flows, axe, light and dark, Chromium and WebKit), `e2e/visual.spec.ts` (baselines) |
| Real app on dashd regtest (T1) | `e2e-tauri/settings.ts`, `scripts/e2e-tauri-settings.sh` (`pnpm e2e:tauri:settings`) |

## Screen inventory

Each row says what it binds to. "not yet (X)" rows show the row with the reason and the owning task X.
The QT and IOS rows keep their status in `docs/parity.md` (D20).

### General (QT-135…141, QT-075, IOS-104/105/106, QT-020/036/039)

| Row | Binds to | Parity |
|---|---|---|
| Appearance: System / Light / Dark | `set_ui_settings.theme` (D2, D3) | QT-140 |
| Unit to show amounts in: DASH / mDASH / μDASH / duffs | `set_ui_settings.unit`; every amount on screen and typed in Pay (D4) | QT-139, QT-152 |
| Decimal digits (2–8) | `set_ui_settings.decimalDigits`; Home's hero and breakdown (UX-SPEC §5.2) | QT-020, QT-036 |
| Subtract fee from amount by default | `set_ui_settings.subtractFeeByDefault`; Pay ▸ To an address starts with it (D5) | QT-137 |
| Enable dust attack protection, threshold 1–1,000,000 duffs | `dustProtection`, `setDustProtection` | QT-075 |
| Network: the window's network and data directory | `launchInfo.network`, `nodeInfo.dataDir` | QT-002 |
| Switch network | **not yet (U-04b, D6)** | IOS-106 |
| User Interface language | **not yet (MP-14)**: English only | QT-139, IOS-118 |
| Local currency | **not yet (MP-01)** | IOS-104 |
| Notifications | **not yet (DP2-09 for DashPay, W-02 for Windows toasts)** | IOS-105, QT-033 |
| Start on system login, tray behaviour | **not yet (W-02 / PK)**, the tray is U-01's | QT-136 |
| SOCKS5 proxy, Tor | **not yet**: the engine has no proxy setting (D7) | QT-138 |
| Not applicable to this wallet (SPV) | an info note listing dash-qt's node-only options | QT-136, QT-138 |
| Reset Options | resets the UI settings file to its defaults after a confirmation (D8) | QT-141 |

### Security (UX-SPEC §4.13, IOS-108/015/016/011/109, QT-111/113)

| Row | Binds to | Parity |
|---|---|---|
| Wallet encryption: Encrypted / Not encrypted | `lockStatus.encrypted` | QT-111 |
| Encrypt wallet… (unencrypted vault) | `authorize(changeCredential, null, unencrypted)`, `encryptVault` (D9) | QT-111 |
| Change passphrase… | `changePassphrase(old, new)` under the lockout policy (D10) | QT-111, IOS-108 |
| Show recovery phrase… | U-02's sheet (`dialog: "recoveryPhrase"`) | QT-113, IOS-108 |
| Touch ID / Windows Hello quick unlock, spending limit | **not yet (U-08 after B4; W-02)**: "Not available on this computer" (D11) | IOS-011, IOS-016 |
| Auto lock: Never, 1 minute, 5 minutes, 1 hour, 24 hours | renderer inactivity timer → `lock` (D12) | IOS-015 |
| Require authentication for every payment | `paymentAuthentication`, `setPaymentAuthentication(on, changeCredential grant)` (D13) | IOS-016 |
| Autohide balance | `set_ui_settings.autohideBalance`: discreet mode at launch (D14) | IOS-020, IOS-108 |
| Forgot passphrase? | U-02's `ForgotPassphraseSheet` (D15) | IOS-014 |
| Wipe wallet… | typed sentence, `authorize(wipe)` + `removeWallet` per wallet, `destroyVault` (D16) | IOS-109 |

### Wallets (UX-SPEC §4.17, IOS-110/111, QT-101/114)

| Row | Binds to | Parity |
|---|---|---|
| List: name, id, Open / Closed, watch-only, Open at startup | `walletLoadStates`, `listWallets`, `setLoadOnStartup` (D17) | QT-101, IOS-110 |
| Add wallet… | U-02's onboarding (`ui.onboarding`) | QT-102, IOS-110 |
| Add watch-only wallet… (xpub) | `importWatchOnly` | QT-114 |
| Open / Close | `loadWallet` / `unloadWallet` with the startup list (D17) | QT-101 |
| Rename… | `renameWallet` (1–64 characters) | IOS-110 |
| Show recovery phrase… | U-02's sheet for that wallet | IOS-110 |
| Extended public key… (QR, Copy) | `accountXpub(walletId, 0)`, `qrMatrix` | IOS-111 |
| Remove… | `authorize(wipe, walletId)`, `removeWallet` (D18) | IOS-110 |

### Backups (QT-106…110, QT-116)

| Row | Binds to | Parity |
|---|---|---|
| Back up a wallet… | `backupWallet(walletId, <backup directory>/<name>-<time>.dwbackup, passphrase \| null)` (D19) | QT-110 |
| Automatic backups to keep (0–10), their folder, the list | `backupPolicy`, `setBackupPolicy`, `automaticBackups(null)`, `backupDirectory` | QT-116 |
| Show folder | **not yet**: the app has no opener (U-03 D15); the path has Copy (D19) | QT-116 |
| Import a wallet file… (`.dwbackup`, `dumpwallet`, `wallet.dat`) | `inspectWalletFile` → `restoreBackup` / `importDumpWallet` / `importWalletDat` (D19, D21) | QT-106, QT-107, QT-110 |
| Import keys… (HD seed hex, xprv, `listdescriptors true`) | `importKeyMaterial` | QT-108 |
| Export for Dash Core… (`dumpwallet` file, `importdescriptors` JSON) | `coreMnemonicCompatibility`, `authorize(revealSecret)`, `exportForCore` (D19) | QT-109 |

### About (QT-018, QT-153, IOS-107)

| Row | Binds to | Parity |
|---|---|---|
| Version, engine version, network, data directory, backup folder | `launchInfo`, `nodeInfo.dataDir`, `backupDirectory` | QT-153, IOS-107 |
| Command-line options | **not yet**: dw-app prints them with `--help`; no command returns them (D22) | QT-018 |
| Support, GitHub, log export | **not yet (U-05 Tools, IOS-112)**: no opener (U-03 D15) | IOS-107, IOS-112 |

Help ▸ About keeps U-01's small About sheet.

## Slice plan

1. This document.
2. The UI settings file: dw-app `ui_settings` / `set_ui_settings`, the theme at launch, bindings, fixture, `settings/prefs.ts`.
3. The Settings route with its tabs; General (with Reset Options) and About; `NOT_YET.settings` removed.
4. Security: encrypt, change passphrase, payment authentication, auto lock, autohide, Forgot, Wipe.
5. Wallets: list, open/close, startup, rename, remove, xpub, watch-only, add.
6. Backups: back up, automatic backups, import a file, import keys, export for Dash Core.
7. The display unit and decimal digits everywhere; subtract fee by default in Pay.
8. Playwright flows, axe and visual baselines; T1 on regtest; evidence and the test map.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Settings is one route with five tabs (General, Security, Wallets, Backups, About), tabs as U-01's Tools page has them (WAI-ARIA tabs). The tab lives in the shell store (`ui.settingsTab`), so a menu or test can open one. U-02 D23's Wallet card became the Wallets tab: its two buttons keep their test ids `settings-add-wallet` and `settings-recovery-phrase` (T1 presses them). `NOT_YET.settings` is removed. | UX-SPEC §4.12 draws a settings window with toolbar tabs; the Tauri shell has one window, and its routes are pages (U-01 D10). Each tab is a page of `MenuCard`s (C11). |
| D2 | UI settings live in `<data root>/settings.json`, owned by dw-app: `ui_settings` reads it (a missing or unreadable file reads as the defaults), `set_ui_settings` validates and writes it atomically (a temporary file renamed over it). Fields: `theme` (`null` = System), `unit`, `decimalDigits` (2–8), `autohideBalance`, `autoLockSecs` (`null` = Never), `subtractFeeByDefault`. One file for the data root, not per network. Not in `localStorage`. | The Swift host kept them in `settings.json` (m1-swift `SettingsProviding`); the window's state is already `<data root>/window-state.json`. The webview's storage is shared by every data root and survives none of the data root's moves, and T1's fresh data roots would read an earlier run's settings. Display settings are a person's, not a network's. |
| D3 | The theme: `--theme` (tests, the self-test) wins; otherwise the file's. dw-app sets the native window's theme from it at launch and again when `set_ui_settings` changes it, so the title bar follows the page. | The window chrome is native; the page alone would leave a light title bar over a dark page. |
| D4 | The display unit applies to every amount the renderer formats (`format/amount.ts` takes a unit, `useDisplay()` gives the screens the unit and its name), to Pay's amount field (typed in the display unit, parsed by dw-units' `parseAmount(text, unit)`) and to Max. Coin control's fee rate stays DASH/kB, as dash-qt's. The decimal digits apply to Home's hero and breakdown (UX-SPEC §5.2), everything else keeps its unit's full or compact decimals. | QT-139/152: dash-qt's unit is everywhere at once; a unit that changed some amounts would mislabel the rest. |
| D5 | "Subtract fee from amount by default" sets the box's initial state of each new Pay form; Max still turns it on (U-03 D8). dash-qt's other Wallet-tab options are not built: coin control is U-03's collapsed Advanced disclosure (U-03 D7), PSBT controls come with U-05, "Keep custom change address" and "Spend unconfirmed change" have no engine setting. | QT-137's options where they mean something on this wallet; no setting that changes nothing. |
| D6 | Switching networks is **not yet** (follow-up U-04b). General shows the window's network and data directory, and how to open another: launch Dash Wallet with `--network`. | E0-13 D9: the launch flags (DAPI, SPV peers, the quorum service, CA) are the launch network's trust settings; `openNetwork` on another network would carry them over. A switch needs dw-app to scope those flags to their network and remember the last network for the next launch, and the renderer to drop every query and store keyed to the old one. That is a task of its own, not a Settings row. |
| D7 | Rows with no engine or shell support are listed with "Not available yet" and their owning task: language (MP-14), local currency (MP-01), notifications (DP2-09, W-02), start on login and the tray options (W-02, PK), the proxy (no engine setting; follow-up), quick unlock (U-08 after B4, W-02). An info note lists dash-qt's node-only options as not applicable to an SPV wallet (UX-SPEC §4.12). | ROADMAP §0 and the brief: a row that does nothing must say so, and say who brings it. |
| D8 | Reset Options asks first (dash-qt's "Confirm options reset", without "Client will be shut down"), then writes the defaults. The file holds nothing else, so no backup copy is kept and no restart is needed: the settings apply at once. Dust protection is the engine's, not the file's, and is not reset. | dash-qt quits because its options apply at start-up; ours apply live. |
| D9 | Encrypt wallet (QT-111) on an unencrypted vault: dash-qt's warning ("If you encrypt your wallet and lose your passphrase, you will LOSE ALL OF YOUR DASH!"), the new passphrase twice (U-02's `NewPassphraseForm`, QT-111 rules), then `authorize(changeCredential, null, unencrypted)` and `encryptVault`. The vault ends locked (the engine revokes every lease), so the lock screen follows; the sheet says so when it is unlocked again. | The engine's rule: encrypting needs a `changeCredential` grant and leaves the vault locked. |
| D10 | Change passphrase: one form with the current passphrase and the new one twice, as dash-qt's dialog. The new one is checked first (equal, not empty, at most 1024 characters); then `changePassphrase(old, new)` runs as one `LockModel.attempt` (U-02 D12): a wrong current passphrase counts a failure, shows the attempts left and the wait, and stops at eight. Every field is cleared after each submit. | dw-vault counts a failure on every passphrase check. |
| D11 | Quick unlock (Touch ID, Windows Hello) and its spending limit: one row, "Not available on this computer", with the reason. dw-app's `quickUnlockPolicy` and `enrollQuickUnlock` exist, but no build has an OS biometric store yet: macOS stays off until B4 (ROADMAP U-08) and Windows Hello is W-02. | A toggle that cannot be turned on is worse than a plain note. |
| D12 | Auto lock (IOS-015): Never (default), 1 minute, 5 minutes, 1 hour, 24 hours; iOS's "Immediately" is left out (a desktop window has no background state to lock on). The timer runs in the renderer (`settings/autoLock.ts`), is armed only while an encrypted vault holds its key (`unlocked` or `unlockedMixingOnly`), restarts on keyboard, pointer and wheel input in the window, and on expiry calls `lock` (the Lock button's call) and says "Locked automatically.". A screen showing a flow's progress is not activity. | E0-04 §4.8: auto lock is `lock_vault`, armed only while the key is held; Q23. The engine's own relock timer (`unlockFor`) counts from the unlock, not from the last input. |
| D13 | Require authentication for every payment: the toggle reads `paymentAuthentication`; changing it either way asks for the passphrase (`LockModel.attempt` over `authorize(changeCredential)`) and calls `setPaymentAuthentication(on, grant)`. On an unencrypted vault it is off and disabled, with "Encrypt the wallet first." | E0-13 D11: the engine stores and enforces it per vault and takes a `changeCredential` grant for a change; `vault.not_encrypted` otherwise. |
| D14 | Autohide balance (IOS-020): discreet mode is on when the app starts. The toolbar's Discreet toggle still shows and hides at will; it does not change the setting. | iOS: "Autohide balance" hides the balance on every launch. |
| D15 | Forgot passphrase? opens U-02's sheet (`ui.forgotPassphrase`) from Security as from the lock screen, shown only when the vault is encrypted and holds a wallet's secrets (U-02 D11). | One flow, one sheet. |
| D16 | Wipe wallet (IOS-109): a sheet that says what is deleted (iOS's "Delete All Wallets?" text), asks for iOS's sentence typed exactly, then the passphrase (encrypted vault). For every registered wallet (closed ones are opened first, as the engine removes only loaded wallets) it issues a `wipe` grant and calls `removeWallet`, then `destroyVault`. The passphrase stays in the model only until the wipe ends. A failed removal stops before the vault is destroyed and says which wallet. Afterwards the network has no wallet, so onboarding opens (U-02 D1). | Swift `SecurityViewModel.wipe` and its tests; `destroyVault` refuses a vault that still holds secrets (`vault.not_empty`). |
| D17 | Wallets list: every registered wallet from `walletLoadStates` in creation order, with `listWallets`' facts (watch-only, recovery phrase) for the open ones. Open adds the wallet to the startup list and Close removes it, as dash-qt's File menu does (QT-101); the toggle sets it alone. Closing the last open wallet is disabled ("Open another wallet first"): with no wallet loaded the shell would start onboarding (U-02 D1), and dash-qt's "No wallet loaded" panel is not part of the mobile layout. If `walletLoadStates` is not available, the list falls back to the open wallets without Open/Close (Swift `IOS110_listFallsBack…`). | QT-101, IOS-110; the empty-network rule stays U-02's. |
| D18 | Remove wallet: iOS's confirmation ("Remove the wallet … from this computer? Its keys are deleted…"), the passphrase (`LockModel.attempt` over `authorize(wipe, walletId)`), then `removeWallet`. The window's wallet falls back to the first; removing the last wallet opens onboarding. | IOS-110; the engine needs a `wipe` grant. |
| D19 | Files without a file chooser. The app has no native file dialog (no dialog plugin; adding one is a new dependency and a permission, follow-up BL "native file chooser" for U-04 and U-05's PSBT). Until then: backups and exports are written into the network's backup folder (`backupDirectory`) under a generated name (`<wallet name>-YYYY-MM-DD-HH-MM-SS.dwbackup`, `…-dumpwallet.txt`, `…-descriptors.json`; names reduced to letters, digits, `-` and `_`), and the result shows the full path with Copy. Imports take a path typed or pasted into a field. The engine never writes over an existing file (`backup.destination_unwritable`, `compat.destination_unwritable`); the seconds in the name keep two backups in one minute apart. | dash-qt asks where to save; a fixed folder is the honest subset that works on all three OSes today. The engine already takes any path from the renderer, so a chooser adds convenience, not trust. |
| D20 | `docs/parity.md` is left as it is (U-01 D17, U-03 D20): it measures the Swift apps; the Tauri re-baseline moves the rows. The tests below map each row to its TypeScript test by parity ID. | One re-baseline pass, not a row per task. |
| D21 | Import a wallet file reads what the file is (`inspectWalletFile`, from its content) and goes on by kind: a `.dwbackup` asks for its passphrase and restores (`restoreBackup`); a `dumpwallet` file imports (`importDumpWallet`), with the labels imported and the loose keys left out reported; a SQLite `wallet.dat` asks for its passphrase when encrypted (`importWalletDat`); a Berkeley DB `wallet.dat` says to use `dumpwallet` instead; a PSBT says to open it in Tools ▸ PSBT (U-05); anything else is unknown. A file of another network is refused before any import. Errors show copy by code (U-02 D16). | Swift `WalletManagementViewModel.importFile`; dash-qt's import dialogs. |
| D22 | Command-line options (QT-018) are not shown: dw-app prints them for `--help`, and no command returns them. A copy in the renderer would drift. Follow-up: a `launch_usage` command. | No second source of truth. |
| D23 | Every amount field is typed in the display unit, not only Pay's: Receive's request amount and Activity's minimum amount too (dash-qt's `BitcoinAmountField` follows the unit everywhere). The refusal names the unit ("Enter an amount in mDASH, for example 0.25."). The status bar's unit name follows it as well. A form keeps its text when the unit changes; the text is parsed again in the new unit (Settings is another route, so an open form is gone by then). Coin control's custom fee rate stays DASH/kB (D4). | QT-139/152: one unit everywhere, typed and shown. |
| D24 | Display settings are written one at a time, in the order they were made, and each builds on the value the previous save asked for, not on the store (which changes only when dw-app answers). A save that fails leaves the store at the last value dw-app wrote. | T1 lost a unit change: three quick changes each sent the whole file from a store that had not caught up, so the third wrote the first one back. `QT020_aChangeMadeBeforeTheLastSaveAnsweredIsKept`. |
| D25 | Onboarding opens as soon as the last wallet is removed, so the wipe's final `destroyVault` runs behind it. If it fails (only an I/O error can fail it there, since the passphrase was already checked), its message is hidden. The empty vault stays, and onboarding creates the next wallet in it under the old passphrase. App does not hold onboarding back while a wipe runs. Follow-up (U-04): keep onboarding closed while a wipe is working, so a failed destroy shows on the sheet. | A rare error path; an App-level wipe gate is more than this slice needs. T1 waits for `lock_status` to read noVault rather than for onboarding. |
| D26 | Remove is disabled for the last open wallet while another wallet is closed ("Open another wallet first to remove this one"). Removing it would empty the wallet list, so onboarding would open with no Cancel, and the closed wallet (off the startup list since Close, D17) could not be reached until a new wallet was made. With no closed wallet, the last wallet can be removed and onboarding opens as U-02 D1 says. | D17's rule for Close, which Remove would otherwise get around (found by review). `QT101_theLastOpenWalletCannotBeRemovedWhileAnotherIsClosed`. |
| D27 | The wipe deletes what the engine holds (wallets, vault), not the files in the backup folder: automatic and manual `.dwbackup` files and Dash Core exports stay. The sheet says so under iOS's text, naming the folder (`backupDirectory`), and that an export holds private keys in plain text. A stopped wipe says which wallets before the failing one were already removed. Follow-up (U-04): an opt-in "also delete the backup folder" step, once the engine has a command for it. | dash-qt and iOS keep no backup folder of their own, so their texts do not cover it. A renderer-side delete would need a file-system permission. |

## Follow-ups to file

None of these is in ROADMAP or the backlog yet; the "not yet" rows and D-rows that name them wait
on a manager to file them:

| Follow-up | Rows waiting on it |
|---|---|
| U-04b: switch networks in the app (D6) | General ▸ Switch network; IOS-106 tests not ported |
| Native file chooser, a dialog plugin (D19) | Backups ▸ Show folder; save-as for backups and exports; U-05's PSBT files |
| An engine proxy setting | General ▸ SOCKS5 proxy or Tor (D7) |
| A `launch_usage` command (D22) | About ▸ Command-line options |
| Keep onboarding closed while a wipe works (D25) | a failed final `destroyVault` shows on the sheet |
| An engine command to delete the backup folder (D27) | an opt-in step of the wipe |

## Deviations from the slice plan

- Slices 3–6 landed as two commits: the view models of Security, Wallets and Backups with their
  tests (`f5b8d393`), then the screens of all five tabs (`3cd4ce8a`). The screens share one
  `SettingsPage`, the `parts.tsx` rows and one CSS section, so one commit per tab would have
  carried half-built shared parts.
- The fixture models the engine's rules that the screens meet (grants, closed wallets, file kinds
  by content, the network of a file, Berkeley DB, an existing destination), not its file formats:
  a "file" is an entry of `World.settings.files` (`src/fixture/settings.ts`). The real formats are
  T1's (a `.dwbackup` and a dumpwallet export written to disk and read back).
- Ending a wipe closes its sheet: the shell has nothing left to stand on, onboarding opens and a
  toast says "All wallet data was deleted." (found by `e2e/settings.spec.ts`: the sheet's own Done
  sat under onboarding and stayed open behind it).

## T1 evidence

2026-10-14, branch `dw/u-04-settings`: app and renderer built at `2c599647` (the D24 save queue),
with the harness's D25 wait for the destroyed vault. Run on agentbox (Xvfb, WebKitGTK through
tauri-driver) against `dwd-regtest-dashd:24.0.0-rc.2` (compose project `dw-e2e-settings`).
`scripts/e2e-tauri-settings.sh` → PASS, 9 steps:

1. unlock: Wallet 1 holds 1.5 DASH through SPV (funding txid `1c2bfee4e98cf15d…`, 2 blocks mined)
2. General: Dark, mDASH with 3 digits (Home 1500.000 mtDASH), Pay starts with Subtract fee;
   `ui_settings` agrees
3. Security: a wrong current passphrase is counted, then the passphrase changes (the old one is
   refused afterwards); payment authentication off and on; autohide on
4. Wallets: Everyday renamed; the account tpub makes the watch-only Watcher; Close leaves it
   unloaded and off the startup list
5. Backups: `Everyday-<date>.dwbackup` written; the dumpwallet export is a `# Wallet dump` file
6. relaunch: `settings.json` kept mDASH, 3 digits, Dark and autohide (Home `#.### mtDASH`);
   Watcher stayed closed; the new passphrase unlocks
7. Wallets: Watcher opened again, then removed with the passphrase
8. Wipe: the typed sentence and the passphrase; onboarding opens; `list_wallets` is empty and
   `lock_status` reads noVault
9. no phrase or passphrase in the data root, the logs or the other output files

Earlier runs failed twice. Step 2 caught the D24 race (a lost unit change). Step 8 read
`lock_status` the moment onboarding opened, before the final `destroyVault` had answered (D25).

## Ported view-model tests

The Swift `WalletFeatures` and `WalletRuntime` tests of these screens, ported by parity ID
(`apps/desktop/src/settings/*.test.ts`, `src/format/display.test.ts`, vitest; the settings file in
dw-app's `settings.rs`). The screens' flows by the same IDs are `e2e/settings.spec.ts`.

| Swift test | TypeScript (or Rust) test |
|---|---|
| `OptionsViewModelTests.QT135_okAppliesChangedValuesAndCancelReverts` | not ported: each setting saves when it changes (D2), there is no OK or Cancel; e2e `QT135_theAppearanceFollowsTheSettingWithoutALaunchTheme` |
| `OptionsViewModelTests.QT136_mainTabIsHiddenOnMacOSAndAutostartUsesMinAndNetwork` | not ported: start on login is not yet (W-02, PK; D7), e2e `rows without an engine say who brings them` |
| `OptionsViewModelTests.QT137_walletDefaultsAreDashQts` | `options.test.ts` same name; e2e `QT137_subtractFeeByDefaultStartsEachPayForm` |
| `OptionsViewModelTests.QT075_dustProtectionRangeAndEngineCall` | `QT075_dustThresholdIsBetweenOneAndAMillionDuffs`, `QT075_protectionIsSentToTheEngine` |
| `OptionsViewModelTests.QT075_dustProtectionNotImplementedIsShownUnavailable` | not ported: the engine implements dust protection; a failed read shows its error (`settings-dust-error`) |
| `OptionsViewModelTests.QT116_automaticBackupsKeepIsSentToTheEngine` | `files.test.ts` `QT116_automaticBackupsAndFolder`; e2e `QT116_automaticBackupsListAndKeep` |
| `OptionsViewModelTests.QT138_proxyAcceptsNumericAddressesOnly` | not ported: no proxy setting in the engine (D7) |
| `OptionsViewModelTests.QT139_displayTabOptionsAndLanguageRestart` | `QT139_unitsAreDashQts`; `display.test.ts` `QT139_QT152_everyAmountIsInTheDisplayUnit`; language is not yet (MP-14, D7) |
| `OptionsViewModelTests.IOS104_localCurrencyListIsSearchable` | not ported: local currency is not yet (MP-01, D7) |
| `OptionsViewModelTests.IOS105_enablingNotificationsAsksTheOSOnce` | not ported: notifications are not yet (DP2-09, W-02; D7) |
| `OptionsViewModelTests.QT033_coinJoinPopupsOptionIsKeptInShellSettings` | not ported: CoinJoin's options come with its screen |
| `OptionsViewModelTests.QT141_resetOptionsConfirmsBacksUpAndQuits` | same name (no backup copy, no quit: D8); e2e `QT141_resetOptionsAsksFirst` |
| `SettingsViewModelTests.IOS106_switchNetworkGoesThroughTheLifecycleQueue`, `IOS018_followsLifecycleTransitions`, `switchFailureIsShown` | not ported: switching networks is U-04b (D6) |
| `SettingsViewModelTests.QT020_QT036_QT039_displaySettingsPersistAndClamp` | same name; `display.test.ts` `QT020_QT036_heroAndBreakdownFloorToTheDecimalDigits` |
| `SettingsViewModelTests.themeAndLanguage` | same name |
| `SettingsViewModelTests.QT111_encryptWallet` | `security.test.ts` same name, `QT111_alreadyEncryptedIsRefused`; e2e `QT111_encryptWalletLocksIt` |
| `SettingsViewModelTests.QT111_changePassphrase` | same name, `QT111_aWrongCurrentPassphraseCountsAsAnAttempt`; e2e `QT111_changePassphrase` |
| `SettingsViewModelTests.QT113_revealNeedsAuthorizationFirst` | U-02's `recovery.test.ts` (`IOS108_…`): Settings reuses its sheet |
| `WalletManagementViewModelTests.QT101_closeKeepsDataAndLeavesLoadOnStartup` | `wallets.test.ts` same name, `QT101_theLastOpenWalletCannotBeClosed`, `QT101_theLastOpenWalletCannotBeRemovedWhileAnotherIsClosed` (D26), `QT101_theStartupListIsSetAlone` |
| `WalletManagementViewModelTests.IOS110_listFallsBackToLoadedWalletsWhileLoadStatesAreNotImplemented` | same name |
| `WalletManagementViewModelTests.IOS110_removeNeedsAWipeGrant` | same name, `IOS110_anUnencryptedVaultRemovesWithoutAPassphrase`, `IOS110_renameKeepsTheEnginesRule` |
| `WalletManagementViewModelTests.QT107_dumpwalletImportReportsKeysNotImported` | `files.test.ts` same name, `QT107_aFileOfAnotherNetworkIsRefusedBeforeAnyImport` |
| `WalletManagementViewModelTests.QT106_encryptedWalletDatAsksForItsPassphrase` | same name |
| `WalletManagementViewModelTests.QT106_berkeleyDBSaysWhatToDoInstead` | same name |
| `WalletManagementViewModelTests.QT078_psbtFileIsHandedToThePSBTDialog` | same name (the dialog is U-05's: the import refuses the file and says so) |
| `WalletManagementViewModelTests.QT108_hdSeedHexGoesIntoAZeroingBuffer` | same name, `QT108_theEnginesRefusalIsShownByCode` |
| `WalletManagementViewModelTests.QT114_watchOnlyWalletFromAnXpub` | `wallets.test.ts` same name |
| `WalletManagementViewModelTests.QT110_backupOfAnUnencryptedVaultNeedsAPassphrase` | `files.test.ts` same name, `QT110_backupOfAnEncryptedVaultUsesItsPassphrase` |
| `WalletManagementViewModelTests.QT110_restoringABackupAsksForItsPassphrase` | same name |
| `WalletManagementViewModelTests.QT116_automaticBackupsAndFolder` | same name |
| `WalletManagementViewModelTests.QT109_exportForCoreNeedsARevealGrantAndReportsWarnings` | same name, `QT109_aLockedVaultStillOffersTheExport` |
| `WalletManagementViewModelTests.IOS111_xpubWithQRCode` | `wallets.test.ts` same name |
| `WalletManagementViewModelTests.IOS009_existingDataPromptAndDeleteAllWithTypedSentence` | `security.test.ts` same name |
| `SecurityViewModelTests.IOS011_quickUnlockEnrollsWithAChangeCredentialGrantAndDefaultLimit`, `IOS011_quickUnlockIsHiddenWithoutBiometricsOrEncryption`, `IOS016_spendLimitOptionsAreTheIOSOnes` | not ported: quick unlock and its spending limit are not yet (U-08 after B4, W-02; D7); e2e `quick unlock is not yet, and says who brings it` |
| `SecurityViewModelTests.IOS015_autoLockDefaultsToNeverAndIsSet` | same name |
| `SecurityViewModelTests.IOS016_requireAuthenticationAndAutohideBalance` | same name; e2e `IOS016_paymentAuthenticationAsksForThePassphrase`, `IOS020_autohideBalanceIsSavedForTheNextStart` |
| `SecurityViewModelTests.IOS108_revealPhraseOfAChosenWalletRecordsTheBackup` | U-02's `recovery.test.ts` `IOS108_revealPhraseOfAChosenWalletNeedsThePassphrase` (each wallet row opens it for its wallet) |
| `SecurityViewModelTests.IOS014_forgotPassphraseChecksThePhraseThenReplacesTheVault`, `IOS014_aPhraseOfAnotherWalletIsAMismatch` | U-02's `forgot.test.ts` same names (Settings reuses the flow) |
| `SecurityViewModelTests.IOS109_wipeNeedsTheSentenceThenRemovesEveryWalletAndDestroysTheVault` | `IOS009_existingDataPromptAndDeleteAllWithTypedSentence`, `IOS109_wipeRemovesClosedWalletsToo`, `IOS109_anUnencryptedVaultNeedsNoPassphrase`; e2e `IOS009_deleteAllWithTypedSentence` |
| `SecurityViewModelTests.IOS109_aFailedRemovalStopsBeforeTheVaultIsDestroyed` | same name, `IOS109_aStoppedWipeSaysWhatItAlreadyRemoved` |
| `QuickUnlockServiceTests.IOS011_*`, `IOS016_spendLimitOptionsOnly`, `IOS014_IOS109_recoveryAndDestroyDropTheStoredKey` | not ported: no stored key without quick unlock (D7) |
| `AutoLockTests.IOS015_defaultIsNeverAndTheIntervalPersists` | `IOS015_autoLockDefaultsToNeverAndIsSet`; dw-app `settings.rs` `a_written_file_reads_back` |
| `AutoLockTests.IOS015_inactivityLocksOnceUntilActivity` | `autoLock.test.ts` `IOS015_locksAfterTheWaitWithoutInput`, `IOS015_inputRestartsTheWait`, `IOS015_disarmingStopsIt`, `IOS015_armedOnlyWhileAnEncryptedVaultHoldsItsKey`; e2e `IOS015_autoLockLocksAfterTheChosenIdleTime` |
| `AutoLockTests.IOS015_sleepScreenLockAndIdleEvents` | not ported: the OS's sleep and screen-lock events are W-02's; this window's own input is the idle signal (D12) |
| `SettingsStoreTests.defaultsWithoutFiles` | dw-app `settings.rs` `a_missing_file_reads_as_the_defaults` |
| `SettingsStoreTests.updatesPersistAndNotify` | `a_written_file_reads_back`; T1 step 6 (the file after a relaunch) |
| `SettingsStoreTests.decimalDigitsOutsideTwoToEightAreRejected` | `out_of_range_values_are_refused`; `options.test.ts` `QT020_QT036_QT039_…` |
| `SettingsStoreTests.corruptFilesMoveToBackupAndDefaultsApply`, `olderFilesWithMissingKeysStillLoad` | `an_unreadable_or_hand_edited_file_falls_back` (no backup copy: the file holds display settings only, D8) |
| `SettingsStoreTests.unwritableLocationReportsSettingsWriteFailed` | not ported as such: dw-app's write error reaches General as "The settings could not be saved." (`useSavePrefs`); refusals in `QT020_QT036_QT039_…` |

QT and IOS parity rows keep their status (D20): the rows above that are not yet stay open with
their owning task.
