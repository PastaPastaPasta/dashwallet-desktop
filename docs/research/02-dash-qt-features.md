# 02 — dash-qt (Dash Core GUI) user-facing feature inventory

> Research note for the greenfield desktop wallet. Goal: full user-facing parity with dash-qt, plus wallet
> backup and restore that works in both directions.
>
> **Source baseline**
> - Dash Core `upstream/develop` @ `3789ec0c719e` (2026-10-04, version string 24.1.0-pre). This is effectively
>   identical to `v24.0.0-rc.2` (2026-09-30): only 8 files in `src/qt` differ, by about 130 lines.
> - Local checkout `/Users/pasta/workspace/dash`: branch `feat/v24-relative-unlock-limit` @ `0a6ff10ee00f`. Its
>   GUI is older than develop (it lacks the shared-masternode UI), so develop was read through `git archive`. The
>   tree was not modified.
> - Latest stable release is **v23.1.8**. Where it matters, this note says what v23.1.8 lacks: Register
>   Masternode wizard, masternode maintenance dialogs, shared masternodes, proposal vote dialog, Migrate Wallet
>   menu, descriptor-by-default.
> - File references are `src/qt/<file>:<approx line>` unless stated otherwise.
>
> **Method:** seven parallel read-through passes over every `src/qt/*.cpp` and `forms/*.ui` file, plus the
> relevant parts of `src/wallet`, `src/coinjoin`, `src/governance`, `src/evo` and `src/rpc`. Line numbers are
> approximate.

---

## 0. Headline findings (read first)

### Not in dash-qt
These are things a reader might assume exist:
- **No RBF / bump fee.**
- **No BIP70.** Only a "BIP70 is no longer supported" warning remains.
- **No InstantSend checkbox.** InstantSend is automatic.
- **No "pay only required fee".**
- **No "restore from recovery phrase" screen.** GUI restore only takes a `.dat` file. Mnemonic restore is CLI
  (`-mnemonic`) or console (`upgradetohd`).
- **No keypool setting.**
- **No Window tab in Options.** Tray options are on Main.
- **No wallet repair** `-salvagewallet` / `-zapwallettxes` / `-upgradewallet`. Repair is only Rescan, Rescan
  (full) and Rebuild Index.

### Dash-specific features with no Bitcoin Core equivalent
- CoinJoin: overview mixing panel, a separate **"CoinJoin" send tab**, mixing-only unlock, automatic backups.
- **Resend transaction**, and **Unlock dust UTXO** with dust-attack protection.
- **Discreet mode**.
- Masternodes tab with a full ProTx registration and maintenance suite, plus **v24 shared (multi-party)
  masternodes**.
- Governance tab: list, vote, create and resume proposals, and a status-bar "governance clock".
- Themes: Light, Dark, Traditional. Font family, scale and weight controls.
- A Debug window "Network" sub-tab covering Credit Pool, InstantSend, Masternodes, ChainLocks and Quorums.
- Mnemonic verification after wallet creation, and **Show Recovery Phrase**.

### Wallet and backup compatibility rules
- **Default wallet type.** New wallets are descriptor (SQLite) wallets by default in v24. In v23.1.8 the GUI
  checkbox is unchecked and the RPC default is `false`.
- **Both wallet types are BIP39 + BIP44 HD.**
  - External chain `m/44'/5'/0'/0/i`, internal `m/44'/5'/0'/1/i`. Coin type is `1'` on testnet, devnet and
    regtest.
  - Descriptor wallets also watch `m/9'/5'/4'/0'/0/i` for funds mixed by Android/dashj.
- **CoinJoin on Core mixes on the ordinary external chain.**
  - A restore must use a lookahead of about 1000 keys per chain (`-keypool` = 1000). The BIP44 gap limit of 20
    will miss funds.
- **BIP39 quirks in Core** (`src/wallet/bip39.cpp:134-149`):
  - The checksum mask uses `2 ^ cs_len` (XOR, not a power). Core therefore accepts some phrases that fail the
    BIP39 checksum.
  - The seed salt `"mnemonic"+passphrase` is cut off at 256 bytes, with **no NFKD** normalisation.
  - English wordlist only. 12, 15, 18, 21 or 24 words.

### Full node vs SPV
- **Needs a full node:**
  - the RPC console;
  - mempool stats;
  - PoSe, last-paid and next-payment data for masternodes;
  - governance vote tallies, fundable set and funded status;
  - pruning, reindex and dbcache;
  - the Credit Pool stats.
- **Can work SPV:**
  - wallet and send/receive;
  - CoinJoin (dashj already does this);
  - InstantSend and ChainLock verification;
  - the masternode list via `mnlistdiff` / `qrinfo`;
  - casting governance votes;
  - proposal creation.
- See §20.

---

## 1. Application startup, lifecycle and command line (`bitcoin.cpp`, `main.cpp`, `initexecutor.cpp`)

| Feature | Behaviour to match |
|---|---|
| Startup sequence | 1. Parse args. Any bare token not starting `-` is an error unless it starts `dash:` (case-insensitive). Options may not follow a `dash:` URI.<br>2. QSettings identity: org `Dash`, domain `dash.org`, app `Dash-Qt`; renamed per network to `Dash-Qt-testnet`, `Dash-Qt-regtest` or `Dash-Qt-<devnetname>`. **All GUI settings are separate for each network.**<br>3. Translations.<br>4. `-printcrashinfo`.<br>5. `-help` / `-version` dialog. Modal on Windows, stdout elsewhere.<br>6. Intro / datadir chooser.<br>7. Read config and select network.<br>8. Single-instance IPC hand-off.<br>9. PaymentServer (wallet builds only).<br>10. Load fonts. Failure is fatal: "Error: Failed to load application fonts."<br>11. Splash.<br>12. Node.<br>13. OptionsModel.<br>14. Validate font and CSS args.<br>15. Main window.<br>16. `appInitMain` on a worker thread.<br>17. When init finishes: create ClientModel and WalletController, show the window, show the first-run Appearance Setup dialog, replay queued URIs. |
| settings.json read error | Critical box with **Reset \| Abort**. Default is Reset; Abort exits. A write failure says "Check that settings file is writable, or try running with -nosettings." |
| Network selection | `-testnet`, `-regtest`, `-devnet=<name>`, `-chain=`. There is no GUI network picker; only `dash.conf` or CLI. See §16 for per-network styling. |
| GUI command-line flags (`bitcoin.cpp:487-505`) | See the flags table below. |
| Core flags that change the shell | `-disablewallet`: no wallet UI, the RPC console becomes the central widget, no PaymentServer. `-prune`. `-disablegovernance`, which pruning forces from the Intro dialog. `-nosettings`. `-datadir`. |
| Exceptions | "Runaway exception": a fatal box, then exit. "Internal error": a non-fatal box. |
| Shutdown window | Shows "Dash Core is shutting down…" / "Do not shut down the computer until this window disappears." It **cannot be closed**. Quit routes from Cmd+Q, Dock, tray Exit and the `stop` RPC (a 200 ms poll timer). |
| Restart | The Repair → Rebuild Index button restarts the app detached with the same args plus `-reindex`. No confirmation is shown, and it runs only once per click storm. |
| Windows session end | Blocks logoff until the node shuts down cleanly (ShutdownBlockReason). |
| macOS App Nap | Disabled while syncing, during IBD/reindex, or while any wallet is mixing. |
| Autostart ("Start Dash Core on system login") | **Windows:** `Startup\Dash Core[ (chain)].lnk` with args `-min -chain=<chain>`.<br>**Linux:** `~/.config/autostart/dashcore[-chain].desktop`.<br>**macOS:** not supported; the option is hidden. |

GUI command-line flags (`bitcoin.cpp:487-505`):

| Flag | Default | Effect |
|---|---|---|
| `-choosedatadir` | 0 | Force the Intro dialog. |
| `-min` | 0 | Start minimized or in the tray; also suppresses the splash. |
| `-splash` | 1 | Show the splash screen. |
| `-resetguisettings` | 0 | Reset the GUI settings and force the Intro dialog. |
| `-lang=<xx_YY>` | system locale | UI language. |
| `-windowtitle=<s>` | "" | Appended to the window title. |
| `-font-family` | SystemDefault | Montserrat, SystemDefault, or any installed family. |
| `-font-scale` | 0 | Range −100…100. |
| `-font-weight-normal`, `-font-weight-bold` | — | Range 0–8. |
| `-custom-css-dir` | — | Must contain `general.css`, `dark.css`, `light.css` and `traditional.css`. |
| `-uiplatform` | — | Debug only. |
| `-debug-ui` | 0 | Debug only. Hot-reloads CSS and forces hidden widgets to show. |

Invalid values for the font and CSS flags are fatal startup errors.

### 1.1 Intro / data-directory chooser (`intro.cpp`, `forms/intro.ui`)

**When it is shown:**
- No `-datadir` on the command line, and any of:
  - the stored `strDataDir` doesn't exist;
  - `-choosedatadir` is set;
  - the default directory changed (`strDataDirDefault`);
  - `fReset` is set;
  - `-resetguisettings` is set.

**Layout:**
- Title "Welcome". Text: "Welcome to Dash Core." / "As this is the first time the program is launched, you can choose where Dash Core will store its data."
- Radio buttons: "Use the default data directory" and "Use a custom data directory:", with a "…" folder picker.
- Free-space label, computed asynchronously:
  - "%n GB of space available".
  - Red, with "(of %n GB needed)", when there isn't enough.
  - Yellow when less than 10 GB would be left.
- Status messages: "A new data directory will be created." / "Directory already exists. Add /name …" / "Path already exists, and is not a directory." / "Cannot create data directory here."

**Prune row:**
- "Limit block chain storage to [N] GB", with the hint "(sufficient to restore backups %n day(s) old)". Days = GB·1e9 / (2.25 MB × 576 blocks).
- Minimum 1 GB. Default 2 GB, or the `-prune` value.
- Auto-checked when free space is below chain + 10 GB. Assumed sizes: mainnet 57 + 1 GB, testnet 10 + 1 GB.
- Pruning from the Intro forces `disablegovernance=1` and `txindex=0`.

**Completion:**
- OK creates `<dir>` and `<dir>/wallets`, then saves `strDataDir`, `strDataDirDefault` and `fReset=false`.
- Cancel exits.
- For our SPV app this becomes a wallet/data directory chooser. Prune and size do not apply.

### 1.2 Splash screen (`splashscreen.cpp`)

**Look:**
- Frameless 440×360 rounded card.
- Dash logo, "Dash Core", the version, and a network badge (devnet only).
- 4 px progress bar with status text.

**Progress mapping:**
- Init messages map to fixed phases: Loading P2P addresses 0–2 %, banlist 2–3 %, block index 3–75 %, verifying/replaying 75–88 %, pruning 88–90 %, network threads 90–94 %.
- Wallet phases restart the bar: "Verifying wallet(s)…" 0–20 %, "Loading wallet…" 20–100 %, "Rescanning…" 0–100 %.
- The bar never moves backwards.

**Exit:** pressing **Q**, or closing the splash, starts an emergency shutdown during startup.

### 1.3 Single instance and IPC (`paymentserver.cpp`)

- IPC socket name is `"DashQt" + qHash(datadir-net path)`.
- A second launch **with** `dash:` URIs sends them to the running instance through QLocalSocket and exits 0.
- A second launch **without** URIs carries on and fails on the datadir lock.
- macOS: `QFileOpenEvent` handling, and `CFBundleURLSchemes = dash` in Info.plist.
- URIs received before the UI is ready are queued and replayed by `uiReady()`.

---

## 2. Main window (`bitcoingui.cpp`, `walletframe.cpp`, `walletview.cpp`)

**Window:**
- Title: `Dash Core[ - <windowtitle>][ - <wallet display name>][ - <network add text>]`.
  - The default wallet adds no text.
  - Testnet and regtest add **no** title text; only the icon tint changes. Devnet adds `[devnet: <name>]`.
- Geometry is saved in QSettings `MainWindowGeometry`.
- Minimum width is about 980 px and grows with the number of visible tabs.
- Ctrl+W closes the window. On Windows/Linux this exits unless minimize-on-close is set.

**Wallet-disabled mode (`-disablewallet`):** the window is just the Tools window. No toolbar, wallet menus, unit
selector, HD/lock icons or tray wallet items.

**Drag and drop:** dropped URLs are sent to the URI handler.

### 2.1 Menus — exact labels (`createMenuBar` ~636)

Accelerators (`&`) are shown as in the source. "Ctrl" means Cmd on macOS.

**File** (wallet builds):

| Item | Status tip / behaviour | Enabled when |
|---|---|---|
| Create Wallet… | "Create a new wallet" → CreateWalletActivity (§12.2) | WalletController exists |
| Open Wallet ▸ | Submenu listing `listWalletDir()`. Unnamed wallet shows as "[default wallet]". Wallets already loaded are disabled. Empty list shows "No wallets available". | controller exists |
| Close Wallet… | Confirm: "Are you sure you wish to close the wallet <i>%1</i>?" plus a pruning resync warning. **Also removes the wallet from load-on-startup.** | ≥1 wallet |
| Close All Wallets… | "Are you sure you wish to close all wallets?" | ≥1 wallet |
| Migrate Wallet | Legacy → descriptor migration (§12.6). *Not in v23.1.8.* | current wallet is legacy |
| &Backup Wallet… | Save dialog "Backup Wallet", filter `Wallet Data (*.dat)`. Result: "Backup Successful" / "Backup Failed". | ≥1 wallet |
| Restore Wallet… | Open dialog "Load Wallet Backup" (`*.dat`) → input box "Restore Wallet" / "Wallet Name" → restore | controller exists |
| Open &URI… | Open URI dialog (§15) | ≥1 wallet |
| Sign &message… / &Verify message… | Sign/Verify dialog (§8) | ≥1 wallet |
| &Load PSBT from file… / Load PSBT from &clipboard… | §5.8 | wallet build |
| Open &debug log file | Opens `<datadir>/<net>/debug.log` in the OS default app | always |
| Open &wallet configuration file | Opens `dash.conf`. **Does nothing if the file doesn't exist.** | always |
| Show Automatic &Backups | Opens the backups folder (`-walletbackupsdir`, default `<datadir>/backups`) | wallet build |
| E&xit | Ctrl+Q | always |

**Settings:**

| Item | Behaviour |
|---|---|
| &Encrypt Wallet… | Enabled only when the wallet is Unencrypted |
| &Change Passphrase… | Enabled when the wallet is encrypted |
| &Show Recovery Phrase… | §12.5. Disabled for NoKeys wallets. |
| &Unlock Wallet… | Visible when Locked or UnlockedForMixingOnly. Always a full unlock. |
| &Lock Wallet | Visible when Unlocked or UnlockedForMixingOnly |
| &Discreet mode | **Ctrl+Shift+D**, checkable. QSettings `mask_values`. See §3.4. |
| &Options… | Preferences role (Cmd+, on macOS). Opens on the Main tab. |

**Window:**
- &Minimize (Ctrl+M).
- Zoom and "Main Window" (macOS only).
- &Sending addresses / &Receiving addresses. Each opens the address book as its own window.
- &Information (Ctrl+Shift+I), &Console (Ctrl+Shift+C), &Network Traffic (Ctrl+Shift+G), &Peers (Ctrl+Shift+P),
  &Repair (Ctrl+Shift+R). These open the Tools window on that tab.

**Help:**
- &Command-line options. A non-modal dialog showing the `dash-qt -h` text as a two-column table.
- CoinJoin &information. Only visible when CoinJoin is enabled. A rich-text explainer of the denominations
  0.001–10, the "1000 change addresses / ~100 mixing events / backups required" warning, and a docs link.
- &About Dash Core. Version plus licence text.
- About &Qt.

### 2.2 Tab bar and keyboard shortcuts

Text-only, mutually exclusive buttons, in this order:

| # | Label | Status tip | Visibility |
|---|---|---|---|
| 1 | &Overview | "Show general overview of wallet" | always |
| 2 | &Send | "Send coins to a Dash address" | always |
| 3 | &Receive | "Request payments (generates QR codes and dash: URIs)" | always |
| 4 | &Transactions | "Browse transaction history" | always |
| 5 | &CoinJoin | "Send CoinJoin funds to a Dash address" | only when CoinJoin is enabled |
| 6 | &Masternodes | "Browse masternodes" | only when `fShowMasternodesTab` (default **off**) |
| 7 | &Governance | "View Governance Proposals" | only when governance is enabled at node level **and** `fShowGovernanceTab` (default **off**) |

- Visible tabs get **Alt+1…N** on Windows/Linux and **Cmd+1…N** on macOS, renumbered whenever tabs are shown or
  hidden.
- Hiding the active tab jumps to Overview.
- Masternodes and Governance stay usable with no wallet loaded; they show node-level pages.

**Wallet selector:** a combo box in the toolbar, hidden while ≤1 wallet is loaded. Switching wallets changes the
current WalletView and the window title.

**No wallet loaded:** the panel says "No wallet has been loaded. Go to File > Open Wallet to load a wallet. - OR -"
with a **[Create a new wallet]** button.

### 2.3 Status bar (left to right: progress text and bar; icons on the right)

**Unit selector**
- Click opens a menu: DASH / mDASH / μDASH / duffs (testnet: tDASH / mtDASH / μtDASH / tduffs).
- Stored in QSettings `DisplayDashUnit`.

**HD icon:** green, shown only for HD wallets. Tooltip "HD key generation is <b>enabled</b>".

**Lock icon:**

| Wallet state | Icon |
|---|---|
| Unencrypted | red open lock |
| Unlocked | red open lock |
| UnlockedForMixingOnly | orange open lock, tooltip "…unlocked for mixing only" |
| Locked | green closed lock |
| NoKeys | hidden |

**Proxy icon:** shown when the IPv4 and IPv6 proxies are set. Tooltip "Proxy is <b>enabled</b>: ip:port". Click
opens Options → Network. Known quirk: the tooltip is not refreshed when the proxy changes.

**Connections icon:**
- Five levels: 0, 1–2, 3–5, 6–7, ≥8 peers.
- Red when the network is disabled. Blinks while there are 0 peers.
- Tooltip "%n active connection(s) to Dash network" or "Network activity disabled".
- Click menu: "Show Peers tab", "Disable network activity" / "Enable network activity".

**Governance clock** ("moon" icon)
- Shown only if governance is enabled, there are peers, `fShowGovernanceTab` is on and `show_governance_clock`
  is on (default off).
- Animated before governance sync. After sync it shows the phase of the voting cycle; blue means "awaiting
  superblock".
- Tooltip lines: "~%1 (%2 blocks) left for voting" / "…left for superblock" / "Superblock imminent" / "Voting
  period ended", then "~%1% of budget committed (%2 / %3)".
- Click opens the Governance tab.

**Sync icon:** an orange spinner while syncing; green "synced" when done. Clicking it, or the progress bar,
toggles the sync overlay (§2.5).

**Progress text:**
- "Synchronizing with network…"
- "Syncing Headers (%1%)…"
- "Indexing blocks on disk…"
- "Processing blocks on disk…"
- "Connecting to peers…"

**Progress bar:**
- Shows "%1 behind".
- Tooltip while catching up: "Catching up…<br>Processed %n block(s)…<br>Last received block was generated %1 ago.<br>Transactions after this will not yet be visible."
- Tooltip when done: "Up to date".

The status bar does **not** show masternode-list or governance sync stages separately; only the spinner state
reflects them.

### 2.4 Tray icon, tray menu, Dock, notifications

**Tray icon:**
- Network-tinted app icon (macOS uses a template image).
- Tooltip "Dash Core client" plus the network text.
- Hidden when `fHideTrayIcon` is set.
- Left-click toggles show/hide (not on macOS).

**Tray / Dock menu:**
- S&how / &Hide (not on macOS)
- &Send, &CoinJoin, &Receive
- Sign &message…, &Verify message…
- &Options…
- &Information, &Debug console, &Network Monitor, &Peers list, Wallet &Repair
- Open &debug log file, Open &wallet configuration file, Show Automatic &Backups
- E&xit (not on macOS)

The whole menu is disabled while any modal dialog is open.

**Minimize and close** (Windows/Linux only):
- `fMinimizeToTray`: minimizing hides the window to the tray.
- `fMinimizeOnClose`: the close button minimizes instead of quitting.

**Notification backends**, in order of preference: macOS notification center, Linux D-Bus
`org.freedesktop.Notifications`, tray balloon, none. Timeout 10 s.

**Transaction notifications**
- Title: "Incoming transaction" or "Sent transaction".
- Body:
  ```
  Date: …
  Amount: …          (signed, with unit)
  Wallet: …          (only if more than 1 wallet is loaded)
  Type: …
  Label: …           (or Address: …)
  ```
- Suppressed during initial block download.
- CoinJoin internal types are suppressed unless `fShowCoinJoinPopups` is on (default **on**).
- Batching: notifications are grouped over 100 ms. With 100 or more queued, one summary is shown instead:
  "Received and sent multiple transactions" / "Sent multiple transactions" / "Received multiple transactions",
  with Sent and Received totals.

### 2.5 Modal sync overlay (`modaloverlay.cpp`)

**Behaviour:**
- Slides over the main window while the chain tip is more than 25 minutes old.
- Hidden permanently once synced, for the rest of the session.
- Reopened by clicking the progress bar.

**Text:**
- "Recent transactions may not yet be visible, and therefore your wallet's balance might be incorrect…"
- "Attempting to spend Dash that are affected by not-yet-displayed transactions will not be accepted by the network."

**Fields:**
- Number of blocks left. Shows "Unknown. Syncing Headers (%1, %2%)…" while headers are behind.
- Last block time.
- Progress %.
- Progress increase per hour.
- Estimated time left until synced.

**Buttons:** "Hide". After the user hides it, it does not reopen automatically.

---

## 3. Overview page (`overviewpage.cpp`, `forms/overviewpage.ui`, `transactionoverviewwidget.cpp`)

The page has three columns: Balances | CoinJoin (hidden when CoinJoin is disabled) | Recent transactions. An
alert banner runs across the top and shows `getStatusBarWarnings()`.

### 3.1 Balances grid

| Row | Spendable (tooltip) | Watch-only column |
|---|---|---|
| Available: | `balance` — "Your current spendable balance" | `watch_only_balance` |
| Pending: | `unconfirmed_balance` — "Total of transactions that have yet to be confirmed, and do not yet count toward the spendable balance" | unconfirmed watch-only |
| Immature: | `immature_balance` — "Mined balance that has not yet matured". The row is shown only when non-zero. | immature watch-only |
| **Total:** | sum of the three rows above | sum of the watch-only rows |

**Watch-only column.** Shown only when the wallet has watch-only scripts and has private keys. It appears with
"Spendable:" / "Watch-only:" headers.

**Pure watch-only (legacy wallet with private keys disabled).** The Spendable column shows the watch-only values.

**Descriptor wallets.** Only the Spendable column and the anonymized balance are filled in.

**Formatting:**
- Amounts are **truncated, not rounded**, to the QSettings `digits` value: default **2**, choices 2–8, restart
  needed.
- Thousands are always separated with a thin space (U+2009).
- The unit is appended.
- Amounts use the "font for money" Appearance option and can be selected with the mouse.

### 3.2 Out-of-sync labels

- An "(out of sync)" label appears next to the Balances, CoinJoin and Recent transactions headers.
- Tooltip: "The displayed information may be out of date. Your wallet automatically synchronizes with the Dash
  network after a connection is established, but this process has not completed yet."

### 3.3 Recent transactions list

**Size:** 5 rows when CoinJoin is disabled, 6 when enabled, 8 in advanced-UI mixing. Row height is 54 px.

**Filtering.** Uses `COMMON_TYPES` and **hides conflicted transactions and these internal types**:
- CoinJoin Mixing
- CoinJoin Collateral Payment
- CoinJoin Make Collateral Inputs
- CoinJoin Create Denominations
- Dust Receive
- Received via CoinJoin

**Row contents:**
- Line 1: date and time (`<locale short date> hh:mm`), an InstantSend lock icon if locked, and the signed
  amount with its unit (coloured).
- Line 2: an eye icon if watch-only, then the label or address.

**Click** switches to the Transactions tab and selects that transaction.

### 3.4 Discreet mode (Settings → Discreet mode, Ctrl+Shift+D)

- Each digit of every balance is replaced with `#`, e.g. `#.## DASH`.
- The CoinJoin "Amount and Rounds" text becomes `#### DASH / 0 Rounds`.
- **The recent-transactions list is hidden entirely.**
- Status tip: "Discreet mode activated for the Overview tab. To unmask the values, uncheck Settings->Discreet mode."
- **Only the Overview tab is masked.**

### 3.5 CoinJoin panel

Covered in §9.2.

---

## 4. Transactions page (`transactionview.cpp`, `transactiontablemodel.cpp`, `transactionrecord.cpp`, `transactiondesc.cpp`, `transactionfilterproxy.cpp`)

### 4.1 Transaction types

The enum order is fixed: it defines the filter bit positions.

| # | Type | Display string | Produced when |
|---|---|---|---|
| 0 | Other | "" | mixed debit/credit that cannot be split (net amount) |
| 1 | Generated | "Mined" | coinbase output to us |
| 2 | SendToAddress | "Sent to" | all inputs are ours; one record per non-change output |
| 3 | SendToOther | "Sent to" | output with no address (uses `mapValue["to"]`) |
| 4 | RecvWithAddress | "Received with" | our output to a known address |
| 5 | RecvFromOther | "Received from" | our output not matched by address |
| 6 | SendToSelf | "Payment to yourself" | all inputs and outputs are ours |
| 7 | RecvWithCoinJoin | "Received via CoinJoin" | still in the enum, **never assigned** |
| 8 | CoinJoinMixing | "CoinJoin Mixing" | `is_denominate`: all outputs are denominations, net 0 |
| 9 | CoinJoinCollateralPayment | "CoinJoin Collateral Payment" | single in/out at collateral amounts |
| 10 | CoinJoinMakeCollaterals | "CoinJoin Make Collateral Inputs" | self-send creating collateral outputs |
| 11 | CoinJoinCreateDenominations | "CoinJoin Create Denominations" | self-send with denominated outputs |
| 12 | CoinJoinSend | "CoinJoin Send" | tx with `mapValue["DS"]=="1"` |
| 13 | PlatformTransfer | "Platform Transfer" | asset-unlock credit from Platform |
| 14 | DustReceive | "Dust Receive" | dust protection on, foreign inputs, credit ≤ threshold |
| 15 | DataTransaction | "Data Transaction" | OP_RETURN output |
| 16 | MasternodeRegistration | "Masternode Registration" | ProRegTx (one record per tx, net amount) |
| 17 | MasternodeUpdate | "Masternode Update" | ProUpServ, ProUpReg, ProUpRev, ProUpShare, ProUpSharedRegistrar |
| 18 | AssetLock | "Asset Lock" | AssetLockTx where we own at least one input |

**Fee attribution:** in an outgoing transaction the fee is added to the debit of the **first** non-change output.

### 4.2 Status

**Rules:**
- Depth < 0 → Conflicted.
- Depth 0 → Unconfirmed, or Abandoned.
- Depth < 6 and not ChainLocked → Confirming.
- Otherwise → Confirmed. **A ChainLock confirms the transaction immediately.**
- Coinbase → Immature, or "Generated but not accepted".

**Status text:**
- "Confirming (%1 of %2 recommended confirmations)"
- "Confirmed (%1 confirmations)"
- Suffixes: ", verified via InstantSend" and ", locked via ChainLocks".

**Icons:**
- `transaction_0`
- `transaction_1..5`
- `synced`
- `transaction_abandoned` (red)
- `transaction_locked` (InstantSend)

### 4.3 Table

**Columns:** Status icon | Watch-only (eye; hidden unless the wallet is watch-only) | Date | Type | Address / Label
| Amount (<unit>).

**Default sort:** Date, newest first. Multi-row selection is allowed.

**Amount column:**
- Shown in `[brackets]` when the transaction doesn't count toward the balance.
- Colours: green for receive, mined and Platform transfer; red for sends, data and asset lock; orange for
  self-send, CoinJoin internal, dust and masternode transactions.
- Rows that don't count toward the balance are greyed.

**Special-type tooltips:**
- MN Registration: "Registers a masternode. The amount is this wallet's net balance change…"

**Selection total:** a "Selected amount:" label sums the selected rows. It shows red when negative.

### 4.4 Filters

**Watch-only:** All / Yes / No. Shown only for watch-only wallets.

**Date:**
- Options: All, Today, This week, This month, Last month, This year, Range…
- Persisted as `transactionDate`.
- Range selection uses date pickers persisted as `transactionDateFrom` / `transactionDateTo`. **The end date is
  exclusive.**

**Type:**
- Options:
  - All
  - Most Common
  - Received with
  - Sent to
  - CoinJoin Send
  - CoinJoin Make Collateral Inputs
  - CoinJoin Create Denominations
  - CoinJoin Mixing
  - CoinJoin Collateral Payment
  - To yourself
  - Mined
  - Masternode
  - Platform Transfer
  - Asset Lock
  - Data Transaction
  - Dust Receive
  - Other
- Persisted as `transactionTypeFilter`, a bitmask.
- The five CoinJoin entries are hidden when CoinJoin is disabled.

**Search:**
- Placeholder: "Enter address, transaction id, or label to search".
- Case-insensitive match against the address, label or txid.
- Debounced by 200 ms.

**Min amount:**
- Uses the display unit and compares against the absolute value.
- "," is accepted as ".".

The table **does** show conflicted transactions, unlike the overview.

### 4.5 Context menu, in order

1. &Copy address
2. Copy &label
3. Copy &amount — signed, with no separators.
4. Copy transaction &ID
5. Copy &raw transaction — hex.
6. Copy full transaction &details — `"<date> <status>. <type> (<label>) <address> <amount>"`.
7. &Show transaction details
8. A&bandon transaction
   - Enabled only when the transaction is not already abandoned, has depth 0, is not in the mempool and has no
     InstantSend lock.
9. Rese&nd transaction
   - Enabled for a single selection with depth 0 that is not abandoned, not coinbase and not InstantSend-locked.
10. &Unlock dust UTXO — only on DustReceive rows.
11. &Edit address label — opens Edit address, or New sending address if the address isn't in the book yet.
12. Show address &QR code
13. "Show in <host>" — one entry per third-party URL in `strThirdPartyTxUrls` (`|`-separated; `%s` is replaced
    with the txid).

**Other actions:**
- Double-click opens the details dialog.
- Ctrl+C copies the full details text.

### 4.6 Transaction details dialog

Title: "Details for <txid>". The fields are listed in order.

**Status:**
- "conflicted with a transaction with %1 confirmations"
- "0/unconfirmed, in memory pool" or "0/unconfirmed, not in memory pool"
- ", abandoned"
- "%1/unconfirmed"
- "%1 confirmations"
- ", locked via ChainLocks"
- ", verified via InstantSend"

**Date.**

**Type:** shown only for MN Registration, MN Update and Asset Lock.

**Source / From / To:**
- "Source: Generated"
- "Source: Platform Transfer"
- "From: …"
- "To: … (own address | watch-only, label: …)"

**Amounts:**
- Credit, with "(matures in %n more block(s))" for immature coinbase.
- Debit, per output.
- Total debit and Total credit.
- Transaction fee.
- Net amount.

**Message and Comment.**

**Identifiers and size:**
- Transaction ID.
- Output index.
- Transaction total size.
- Payload: the OP_RETURN hex, for Data transactions only.

**Notes:**
- Messages from the URI order form.
- The coinbase maturity note.
- When any debug log category is enabled, a Debug section with inputs and the raw transaction.

### 4.7 CSV export

**Export** button at the bottom of the tab:
- Dialog title: "Export Transaction History".
- File filter: `*.csv`.
- Exports **what the current filter shows**.

**Columns, exact header strings, in order:**
1. `Confirmed` (true/false)
2. `Watch-only` (only when the wallet is watch-only)
3. `Date` (ISO `yyyy-MM-ddTHH:mm:ss`)
4. `Type`
5. `Label`
6. `Address`
7. `Amount (<unit>)` (signed, no separators)
8. `ID`

**Encoding:** every field, headers included, is wrapped in double quotes, and embedded quotes are doubled.
Fields are separated by `,` and lines end with `\n`.

---

## 5. Send page (`sendcoinsdialog.cpp`, `sendcoinsentry.cpp`, `coincontroldialog.cpp`, `walletmodel.cpp`, `psbtoperationsdialog.cpp`)

There are **two send pages**:
- **Send** — any funds.
- **CoinJoin** — a second instance of `SendCoinsDialog(true)`:
  - Send button text: "S&end mixed funds".
  - Balance shown: the anonymized (mixed) balance.
  - Coin selection is restricted to `ONLY_FULLY_MIXED`.
  - No custom change address. There is no change output; the excess goes to the fee.
  - The transaction is tagged `DS=1`.

Payment URIs always open the regular **Send** page.

### 5.1 Recipient entry fields

| Field | Behaviour |
|---|---|
| Pay &To | Placeholder "Enter a Dash address (e.g. %1)". Pasting a full `dash:` URI fills every field. Validation strips whitespace and zero-width characters and allows base58 only. |
| Address-book button (Alt+A) | Opens the sending address book in selection mode. |
| Paste button (Alt+P) | Pastes from the clipboard. |
| &Label | Auto-filled from the address book. |
| A&mount | Plain line edit in the current display unit (there is no unit combo box). Typing "," converts to ".". Reformatted when focus leaves. Range 0…21M DASH. |
| S&ubtract fee from amount | Default comes from the option `SubFeeFromAmount`. With multiple recipients the fee is split equally. |
| Use available balance | Fills in the available balance minus the other entries, and **ticks subtract-fee**. |
| Message | Read-only. Shown only when the entry came from a URI with `message=`. Stored locally as the order form; never sent over the network. |
| Remove entry | Removing the last entry leaves a blank entry. |

**Per-entry validation** (the offending field turns red):
- invalid address
- amount ≤ 0
- dust — against a dust relay fee of 3000 duff/kB, about 546 duffs for P2PKH

**Dialog buttons:**
- Add &Recipient
- Clear &All — also clears coin control and the custom change address, unless "keep" is set.
- S&end

**Balance label:**
- "Balance:"
- "Watch-only balance:" for watch-only wallets.
- "External balance:" for external-signer wallets. Upstream bug: this shows 0.

### 5.2 Units (`bitcoinunits.cpp`)

| Unit | Testnet name | Factor | Decimals |
|---|---|---|---|
| DASH | tDASH | 1e8 | 8 |
| mDASH | mtDASH | 1e5 | 5 |
| μDASH (U+03BC) | μtDASH | 100 | 2 |
| duffs | tduffs | 1 | 0 |

- Settings index order: DASH=0, mDASH=1, uDASH=2, duffs=3.
- The thousands separator is a thin space. The decimal mark is always "." (never localized).
- Parsing allows at most 18 digits and no sign.

### 5.3 Fee section

**Layout:**
- The fee section can be collapsed: "Transaction Fee: <summary> [Choose…]" and "Hide".
- Collapsed state is stored in QSettings `fFeeSectionMinimized` (default true).

**Fee modes:**
- **Recommended** (`nFeeRadio`=0):
  - Shows the smart-fee rate `<rate> <unit>/kB` and "Estimated to begin confirmation within %n block(s)."
  - "Confirmation time target" options:

    | Option | Blocks |
    |---|---|
    | 5 minutes | 2 |
    | 10 minutes | 4 |
    | 15 minutes | 6 (default `-txconfirmtarget`) |
    | 30 minutes | 12 |
    | 60 minutes | 24 |
    | 2 hours | 48 |
    | 6 hours | 144 |
    | 21 hours | 504 |
    | 42 hours | 1008 |

  - The target is stored in blocks in `nConfTarget`. An old `nSmartFeeSliderPosition` value is migrated as
    25 − value.
- **Custom** (`nFeeRadio`=1):
  - "[amount] per kilobyte", stored in `nTransactionFee` (duff/kB).
  - Clamped to at least the required fee, max(mintxfee, minrelay) = 1000 duff/kB.
  - Warning: "A too low fee might result in a never confirming transaction (read the tooltip)".

**Fallback:**
- When no estimate is available, the wallet uses `-fallbackfee` (1000 duff/kB) and shows "Note: Not enough data
  for fee estimation, using the fallback fee instead."

**Caps:**
- `-maxtxfee` defaults to 0.1 DASH.
- The GUI also has its own "absurdly high fee" check.

### 5.4 Confirmation flow

1. Validate every entry.
2. If the wallet is Locked or mixing-only, ask for a **full unlock**. The wallet is relocked, or returned to
   mixing-only, right after the transaction is prepared.
3. Prepare: build and sign. If coins were hand-picked, **only those coins** are used.
4. If an address appears more than once: ask "Confirm duplicate recipients" (Yes/Cancel). This is a question, not
   an error.
5. Show "Confirm send coins".

**"Confirm send coins" contents:**
- "Do you want to create this transaction?" followed by "Please, review your transaction." A PSBT variant of this
  text exists.
- One line per recipient: `<amt> to '<label>' (<addr>)`, plus "from wallet '%2'" when multiwallet. At most 10
  lines, then "(%1 of %2 entries displayed)".
- "using **any available funds**" or "using **CoinJoin funds only**".
- Transaction fee. On the CoinJoin page, adds "(CoinJoin transactions have higher fees usually due to no change
  output being allowed)".
- Transaction size in kB, and the fee rate.
- CoinJoin page only: "This transaction will consume %n input(s)". At ≥10 inputs, a privacy warning with a docs
  link.
- Total Amount, plus "(=" the other units ")".

**Dialog buttons:**
- **Send** is disabled for 3 seconds ("Send (3)…").
- The default button is **Cancel**.
- When PSBT controls are on, or the wallet cannot sign, a "Create Unsigned" button is added.

**After commit:**
- The recipient is added to the address book with purpose `send`. **This overwrites the existing label, even on
  your own receive addresses.**
- The form clears.
- The app jumps to Transactions with the new transaction selected.

### 5.5 Errors (exact strings)

| Error | Message |
|---|---|
| InvalidAddress | "The recipient address is not valid. Please recheck." |
| InvalidAmount | "The amount to pay must be larger than 0." |
| AmountExceedsBalance | "The amount exceeds your balance." |
| AmountWithFeeExceedsBalance | "The total exceeds your balance when the %1 transaction fee is included." |
| TransactionCreationFailed | "Transaction creation failed!" |
| AbsurdFee | "A fee higher than %1 is considered an absurdly high fee." |

The wallet can also raise these messages:
- "Insufficient funds."
- "Transaction amount too small"
- "Transaction too large"
- "The transaction amount is too small to pay the fee"
- "Unable to locate enough mixed funds for this transaction. CoinJoin uses exact denominated amounts…"
- "The preselected coins total amount does not cover the transaction target…"
- "Fee exceeds maximum configured by user…"
- "Fee estimation failed. Fallbackfee is disabled…"

### 5.6 Coin control

Enable it in Options → Wallet: "Enable coin &control features" (`fCoinControlFeatures`, default off).

**Panel on the Send page:**
- "Coin Control Features" header and an [Inputs…] button.
- Shows "automatically selected" when no coins are picked. Otherwise shows Quantity, Bytes, Amount, Fee, After
  Fee and Change.
- "Insufficient funds!" appears in red when needed.
- Right-click any summary value to copy it: Copy quantity, amount, fee, after fee, bytes or change.
- "Custom change address" checkbox plus an address field (§5.7).

**Coin Selection dialog:**
- Buttons:
  - "(un)select all"
  - "(un)lock all" — writes to the wallet database, so locks persist across restarts.
  - Hide/Show CoinJoin coins
  - Tree mode / **List mode** (list is the default)
  - "(%1 locked)" count
- Columns: ☐ | Amount | Received with label | Received with address | Mixing Rounds | Date | Confirmations.
  - Locked coins are disabled and shown with a red lock.
  - In tree mode, coins are grouped by address and change is labelled "(change)".
- Persistent settings: `nCoinControlMode` (true = list), `nCoinControlSortColumn`, `nCoinControlSortOrder`.
  - Default sort is Amount, descending.
- Right-click on a coin:
  - &Copy address
  - Copy &label
  - Copy &amount
  - Copy transaction &ID and output index (`txid:vout`)
  - L&ock unspent / &Unlock unspent
- Keys: Space toggles the checkbox; Esc closes.

**CoinJoin filtering:**
- On the regular page, denominated and collateral coins are **hidden by default**. The button toggles between
  "Show all coins" and "Hide CoinJoin coins".
- On the CoinJoin page, list mode is forced and only fully mixed coins are shown. The button toggles between
  "Show all CoinJoin coins" (other coins disabled) and "Show spendable coins only".

**Size and fee estimate:**
- bytes = 148·inputs + 34·(outputs+1) + 10, minus 34 if there is no change.
- Values are prefixed "≈". Tooltip: "Can vary +/- %1 duff(s) per input."
- Dust change is added to the fee.
- On the CoinJoin page, all change becomes fee.

**Spent selections:** if a selected coin has been spent, it is unselected with the message "Some coins were
unselected because they were spent."

### 5.7 Custom change address and dust protection

**Custom change address:**
- Invalid address: "Warning: Invalid Dash address".
- Address not in this wallet: "Warning: Unknown change address", then a Yes/Cancel confirm "The address you
  selected for change is not part of this wallet…".
- "Keep custom change address" (Options, `fKeepChangeAddress`) remembers the address in `sCustomChangeAddress`.

**Dust attack protection** (Options → Wallet):
- `dustprotectionthreshold`, default 10,000 duffs when enabled, range 1–1,000,000.
- **Automatically locks** small incoming foreign UTXOs. The locks persist.
- The "Unlock dust UTXO" context action releases them (§4.5).

### 5.8 PSBT and external signer

**PSBT controls** (Options → Wallet, "Enable &PSBT controls", `enable_psbt_controls`, default off)
- Adds the "Create Unsigned" path to the confirm dialog.
- Wallets with private keys disabled always get the "Cr&eate Unsigned" button.

**Create Unsigned:**
- The base64 PSBT is **copied to the clipboard automatically**.
- A dialog offers Save or Discard. Save writes a **binary** `.psbt` named `<label-or-addr>-<amount>…psbt`.

**Load PSBT** (File menu):
- From file: binary or base64, under 100 MiB.
- From clipboard: base64 only.
- Opens the PSBT Operations dialog.

**PSBT Operations dialog:**
- Description lines:
  - " * Sends %1 to %2 (own address)" — this amount is always in DASH.
  - Fee.
  - Total — in the display unit.
  - "Transaction has %1 unsigned inputs."
- Status messages:
  - "Transaction is missing some information about inputs."
  - "Transaction still needs signature(s)." Possible suffixes: "(But no wallet is loaded.)", "(…cannot sign…)",
    "(…does not have the right keys.)".
  - "Transaction is fully signed and ready for broadcast."
- Buttons:
  - **Sign Tx** — asks to unlock first.
  - **Broadcast Tx** — max fee rate 0.1 DASH/kB.
  - **Copy to Clipboard** — base64.
  - **Save…** — binary.
  - **Close**.

**External signer (HWI):**
- Configure the script path in Options → Wallet "External signer script path" (`signer`, restart needed).
- The Send button reads "Sign on device".
- Wallets are created with the "External signer" checkbox.
- If signing is incomplete, the PSBT is shown instead.
- Receive dialog: a "&Verify" button shows the address on the device.

### 5.9 Spending behaviour visible to users

- **Spend unconfirmed change** (`spendzeroconfchange`, default true, restart needed).
- **InstantSend:**
  - Automatic. There is no checkbox.
  - InstantSend-locked funds count as trusted and can be spent immediately.
  - The URI `IS=` parameter is ignored.
- **Input/output order:** BIP69 ordering when no change position is fixed.
- **nSequence:** `SEQUENCE_FINAL-1`, for anti-fee-sniping. There is no RBF.
- **Platform (DIP-18 bech32m) addresses:** not sendable from the GUI. A URI containing one gives "This is a Dash
  Platform address, not a Dash Core address".
- **P2SH recipients:** accepted. There is no UI for creating multisig addresses.

---

## 6. Receive page (`receivecoinsdialog.cpp`, `receiverequestdialog.cpp`, `recentrequeststablemodel.cpp`, `qrimagewidget.cpp`)

**Form:**
- Header text: "Use this form to request payments. All fields are **optional**."
- Fields:
  - &Label — placeholder "Enter a label to associate with the new receiving address".
  - &Amount — a display-unit field. 0 or empty means no amount.
  - &Message — "…Note: The message will not be sent with the payment over the Dash network."
- Buttons:
  - "&Create new receiving address" — enabled only when the wallet can hand out addresses.
  - "Clear".
- There is **no** "reuse address" option and no address-type choice.

**Create:**
- Generates a new address via `getNewDestination(label)` and stores it with purpose `receive`.
- If the wallet is locked or the keypool is empty, it asks to unlock and retries.
- Errors: "Could not unlock wallet." / "Could not generate new address".
- On success it opens the Request payment dialog and clears the form.

**Requested payments history:**
- Table columns: Date | Label "(no label)" | Message "(no message)" | Requested (<unit>) "(no amount requested)".
- Stored in the wallet's receive-request records (destdata). Remove writes an empty record.
- Buttons: Show and Remove. Double-click also opens the request.
- Context menu: Copy &URI, &Copy address, Copy &label, Copy &message, Copy &amount.

**Request payment dialog:**
- Title: "Request payment to <label|addr>".
- QR code of the URI, with the address drawn under it.
- Fields, each hidden when empty: URI, Address, Amount, Label, Message, Wallet.
- Buttons:
  - Copy &URI
  - Copy &Address
  - &Verify (external signer only)
  - &Save Image… (PNG)

**QR generation:**
- libqrencode with ECC level **L**, 8-bit mode.
- `MAX_URI_LENGTH` = **255**. A longer URI gives "Resulting URI too long, try to reduce the text for label / message."
- Rendered at 300 px plus a 20 px caption.
- Right-click: Save Image… / Copy Image.
- The image can also be dragged out.

**Generic QR dialog:** "Show address QR code" in the address book and transaction list encodes `dash:<addr>`
only.

---

## 7. Address book (`addressbookpage.cpp`, `addresstablemodel.cpp`, `editaddressdialog.cpp`)

**Modes:**
- **Manage:** separate windows, "Sending addresses - <wallet>" and "Receiving addresses - <wallet>".
- **Select:** used from the Send entry (sending list), Sign message (receiving list) and Verify message (sending
  list). Double-click accepts. The button reads "C&hoose".

**Header text:**
- Sending: "These are your Dash addresses for sending payments. Always check the amount and the receiving address
  before sending coins."
- Receiving: "…Use the 'Create new receiving address' button in the receive tab to create new addresses."

**Search:** "Enter address or label to search". **Wildcard** match on address or label, case-insensitive.

**Table:** columns Label ("(no label)") | Address. Sorted by label, case-insensitive.

**Buttons:**
- &New — sending only.
- &Copy
- &Show QR code
- &Delete — sending only. **Receiving addresses cannot be deleted.**
- &Export — CSV with columns `Label`, `Address`, untranslated headers.
- C&lose

**Context menu:** &Copy Address, Copy &Label, &Edit, Show address &QR code, &Delete (sending only).

**Purpose mapping:**
- `send` → Sending.
- `receive` → Receiving.
- `unknown` or empty → Receiving if the address is ours, otherwise Sending.
- Any other purpose (e.g. `refund`) → hidden.
- Change addresses never appear.

**Edit dialog:**
- Titles: "New sending address" / "Edit sending address" / "Edit receiving address" (the address field is
  read-only for receiving).
- Errors:
  - "The entered address \"%1\" is not a valid Dash address."
  - "Address \"%1\" already exists as a receiving address with label \"%2\" and so cannot be added as a sending address."
  - "The entered address \"%1\" is already in the address book with label \"%2\"."
  - "Could not unlock wallet."
  - "New key generation failed."

---

## 8. Sign / Verify message (`signverifymessagedialog.cpp`, `src/util/message.cpp`)

**Window:** "Signatures - Sign / Verify a Message", with two pages: Sign Message and Verify Message.

**Sign page:**
- Fields:
  - Address, with buttons for the address book (receiving list, Alt+A) and paste (Alt+P).
  - Message.
  - Signature: read-only, monospace, with a copy button.
- Buttons: "Sign &Message" and "Clear &All".
- Requires a **full unlock**.
- Errors:
  - "The entered address is invalid…"
  - "The entered address does not refer to a key…" (only P2PKH addresses can sign)
  - "Wallet unlock was cancelled."
  - "Private key for the entered address is not available."
  - "Message signing failed."
- Success: "Message signed." in green.

**Signature format:**
- Compact recoverable ECDSA, 65 bytes, **base64**.
- Signs `Hash(ser("DarkCoin Signed Message:\n") ‖ ser(message))` — length-prefixed strings, double-SHA256.
- **The magic string is `"DarkCoin Signed Message:\n"`.**

**Verify page:**
- Fields: address (sending address book button), message, signature.
- No unlock needed.
- Results:
  - "Message verified."
  - "The signature could not be decoded…"
  - "The signature did not match the message digest…"
  - "Message verification failed."
- The status line clears whenever a field gets focus.

---
## 9. CoinJoin (formerly PrivateSend)

`gCoinJoinName = "CoinJoin"`. Every user-facing string builds the name in through `%1`.

### 9.1 Constants a compatible client must reproduce

- **Denominations:**

  | DASH | Duffs |
  |---|---|
  | 10.0001 | 10·COIN + 10000 |
  | 1.00001 | COIN + 1000 |
  | 0.100001 | COIN/10 + 100 |
  | 0.0100001 | COIN/100 + 10 |
  | 0.00100001 | COIN/1000 + 1 |

  The denomination bitmask is `1<<i`, where i=0 is the largest denomination.
- **Collateral:** 0.0001 DASH to 0.0004 DASH. The minimum balance needed to start mixing is
  smallest denomination + max collateral = **0.00140001 DASH**.
- **Participants:** 3–20 per session on mainnet, 2–20 on testnet. Each entry holds at most 9 inputs.
- **Timeouts:** 30 s in the queue, 15 s for signing.
- **"Fully mixed" rule** (`wallet/coinjoin.cpp:405`):
  - A coin is fully mixed when rounds ≥ N **and** either rounds ≥ N+3 or `SHA256(outpoint‖wallet CoinJoin salt)`
    is odd.
  - So about half of coins stop at N rounds, a quarter at N+1, and so on.
  - The per-wallet salt is stored under the `cj_salt` key and controlled by the `coinjoinsalt` RPC.
  - **The CoinJoin balance and which coins the CoinJoin send page may spend both depend on this rule.**
- **Post-V24 denomination promotion/demotion:** `PROMOTION_RATIO=10`, `GAP_DIVISOR=5`. These fields only appear
  at protocol version `COINJOIN_REBALANCE_VERSION` or later.

### 9.2 Overview CoinJoin panel

Fields:

| Field | Shown | Content |
|---|---|---|
| **Status** | always | "Enabled" or "Disabled". In advanced mode it adds ", keys left: N" (shown red when N < 100; legacy wallets only). |
| **Completion** | advanced mode only | Progress bar (formula below). |
| **CoinJoin Balance** | always | The `anonymized_balance`. |
| **Amount and Rounds** | always | e.g. `1000 DASH / 4 Rounds`. If there aren't enough compatible inputs it shows `~X / N Rounds` in red, with the tooltip "Not enough compatible inputs to mix…". |
| **Submitted Denom** | advanced mode only | e.g. `1.00001; 0.100001;` or "n/a". |

**Button:** "Start CoinJoin" / "Stop CoinJoin", or "(Disabled)".

**Progress formula** (`overviewpage.cpp:462-512`):
```
max = min(anonymizable + anonymized, targetAmount)
denomPart = min(1, denominated/max)·100        weight 1
normPart  = min(1, normalizedAnon/max)·100     weight rounds
fullPart  = min(1, anonymized/max)·100         weight 2
progress  = Σ ceil(part·weight/(3+rounds)·100)/100, capped at 100
```
- `normalizedAnon` = Σ over denominated coins of `value·min(rounds,N)/N`.
- Tooltip: "Overall progress / Denominated / Partially mixed / Mixed / Denominated inputs have X of N rounds on average".

**Start/stop flow:**
1. On first use, an info box suggests choosing the "Most Common" transaction filter.
2. If the balance is below 0.00140001, it shows "CoinJoin requires at least %2 to use."
3. If the wallet is locked, it opens **"Unlock wallet for mixing only"**. If the user cancels: "Wallet is locked
   and user declined to unlock. Disabling CoinJoin."
4. **Stop** calls `resetPool()` and then `stopMixing()`.

**Disabled states.** The panel is disabled when the node is a masternode, or when automatic backups are 0 or
have failed. Tooltips:

| Cause | Tooltip |
|---|---|
| backups disabled | "Automatic backups are disabled, no mixing available!" |
| backup failed (−1) | "ERROR! Failed to create automatic backup…" |
| keypool locked (−2) | "WARNING! Failed to replenish keypool, please unlock your wallet to do so." |

Upstream quirk: this gate also applies to descriptor wallets, which do not create automatic backups.

**Low keys warning (legacy wallets):**
- Fires when fewer than 100 keys are left since the last automatic backup. It shows a message box and creates an
  automatic backup.
- The core stops mixing below 50 keys.
- Can be turned off with the `fLowKeysWarning` option.

**Other behaviour:**
- Mixing state is **per wallet**: each wallet view has its own Start/Stop. The CoinJoin options are global.
- The panel refreshes on a 1 s timer.
- **Not shown in the GUI:** the per-session status strings (§9.4), queue size, and the masternode being mixed
  with. These are only available through RPC.

### 9.3 Settings

Options → CoinJoin tab, plus the enable checkbox on the Wallet tab.

| Setting | Range | Default | Store |
|---|---|---|---|
| Enable CoinJoin features | bool | **true** (the CLI help wrongly says 0) | settings.json `enablecoinjoin`. Applied immediately; reverted if the dialog is cancelled. |
| Enable advanced interface | bool | false | QSettings `fShowAdvancedCJUI` |
| Show popups for mixing transactions | bool | true | QSettings `fShowCoinJoinPopups` |
| Warn if the wallet is running out of keys | bool | true | QSettings `fLowKeysWarning` |
| Enable multi-session | bool | false | `coinjoinmultisession` |
| Parallel sessions | 1–10 | 4 | `coinjoinsessions` |
| Mixing rounds | 2–16 | 4 | `coinjoinrounds` |
| Target balance (DASH) | 2–21,000,000, step 10 | 1000 | `coinjoinamount` |
| Inputs per denomination: Target | 10–100000 | 50 | `coinjoindenomsgoal`. Kept ≤ Maximum. |
| Inputs per denomination: Maximum | 10–100000 | 300 | `coinjoindenomshardcap` |
| Autostart | — | false | `-coinjoinautostart`. CLI only, no GUI control. |

All of these apply live, with no restart. Legacy `PrivateSend*` QSettings keys are **deleted**, not migrated.

### 9.4 Session status strings

These come from `coinjoin status`, not the GUI.

**General status:**
- "CoinJoin is idle."
- "Submitted to masternode, waiting in queue ."
- "Found enough users, signing…"
- "Can't mix while sync in progress."
- "No Masternodes detected."
- "Not enough funds to mix."
- "Found unconfirmed denominated outputs, will wait till they confirm to continue."
- "No compatible Masternode found."
- "Can't mix: no compatible inputs found!"
- "Masternode queue is full."
- "Last queue was created too recently."

**Pool messages:** about 20 more, e.g. "Entries are full.", "Collateral not valid.", "Transaction created
successfully." (`coinjoin.cpp:505`).

### 9.5 CoinJoin elsewhere in the GUI

- **CoinJoin send tab:** see §5.
- **Coin control:** "Mixing Rounds" column (§5.6).
- **Transaction types and filters:** five CoinJoin types (§4.1).
- **Help:** the "CoinJoin information" dialog.
- **Status bar:** an orange lock when the wallet is unlocked for mixing only.
- **Notifications:** popups for mixing transactions are suppressed unless that option is on.
- **macOS:** App Nap is disabled while mixing.

### 9.6 RPCs and network requirements

**RPCs:**
- `coinjoin start|stop|reset|status`
- `coinjoinsalt generate|get|set`
- `getcoinjoininfo`: returns `enabled, multisession, max_sessions, max_rounds, max_amount, denoms_goal, denoms_hardcap, queue_size, running, pending_inputs, sessions[], keys_left, warnings`.
- `setcoinjoinrounds`, `setcoinjoinamount`
- `walletpassphrase … mixingonly=true`

**Network requirements:**
- **P2P messages:** `dsa`, `dsi`, `dsf`, `dss`, `dsc`, `dssu`, `dsq` (BLS-signed by the operator), `dstx` and
  `senddsq`.
- **Deterministic masternode list:** needed to verify `dsq` operator signatures and to connect to masternodes.
- **Chain synced.**
- **Own-collateral check:** Core validates its own collateral against the local mempool. An SPV client has to
  approximate this check.
- Neither governance data nor a full UTXO set is needed.
- **SPV-capable:** dashj / Dash Android already mixes over SPV and is the reference implementation.

---

## 10. Masternodes tab

Files: `masternodelist.cpp`, `masternodemodel.cpp`, `masternodewizard.cpp`, `masternodedialogs.cpp`,
`masternodeoperationrunner.cpp`, `sharedmn*.cpp`, `mnsharesession.cpp`.

v23.1.8 only had the read-only list (§10.1–10.2). Everything from §10.3 on is new in v24.

**Visibility:**
- Controlled by Options → Display **"Show Masternodes Tab"** (QSettings `fShowMasternodesTab`, default **off**).
  The checkbox tooltip still describes old sub-tabs that no longer exist.
- Hidden when running with `-disablewallet`.
- The list can be browsed with no wallet loaded.

**Persisted filter state:** `mnListHideBanned`, `mnListTypeFilter`, `mnListFilterText`, `mnListOwnedOnly`.

### 10.1 List

**Toolbar:**
- Type combo: All / Regular / Evo / Shared.
- Filter text box: "Filter by any property (e.g. address or protx hash)".
  - Literal, case-insensitive substring match.
  - Searches service, type, PoSe, heights, payout addresses, operator reward text, collateral/owner/voting
    addresses, proTx hash, and share addresses.
  - Does **not** search the operator pubkey or platform node ID.
- **Owned** checkbox.
- **Hide banned** checkbox.
- **Register Masternode…** button.
- **Shared Masternode…** button.
- "Node Count:" shows the filtered count only.

**Columns:**

| Column | Content |
|---|---|
| Status icon | Active: tooltip "Active for X". Banned: "Banned for X". |
| Service | — |
| Type | Regular / Evo / "Shared (you hold %1 of %2)". Hidden when the type filter is Regular or Evo. |
| PoSe Score | — |
| Registered | — |
| Last Paid | — |
| Next Payment | Projected height, or "UNKNOWN". |
| Operator Reward | "NONE", "x.xx% to <addr>", or "…but not claimed". |
| ProTx Hash | Hidden column. |

**Owned** means any of these:
- the collateral is in the wallet;
- the wallet holds the owner key, the voting key, the operator payout script, or a payout script;
- for shared masternodes, the wallet holds a share owner key or a share refund script.

**Refresh:** coalesced on masternode-list change notifications, at most every 3 s (30 s while syncing).
"Next payment" comes from the projected payee list. The collateral address comes from a UTXO lookup.

**Context menu:**
- Copy ProTx Hash
- Copy Collateral Outpoint (`txid-n`)
- Update Service…
- Update Registrar… (not shown for shared masternodes)
- Shared masternodes only: Change Reward Address…, Rotate Keys…, Dissolve…, Create Standby Dissolution…
- Revoke…
- Filter by ▸ Collateral / Payout / Owner / Voting Address
- There is **no "Copy IP"** entry.

**Details dialog** (double-click): "Details for Masternode <hash>", showing:
- ProTx hash, operator public key, owner/payout/voting/collateral addresses, collateral hash and index
- type, registered height, last paid, consecutive payments, operator reward
- for shared masternodes: shares table, early period, early-exit penalty, standby status
- network addresses, Platform HTTPS/P2P addresses, Platform node ID
- PoSe penalty, ban height and revived height

### 10.2 What needs a full node

| Data | Source | SPV / light alternative |
|---|---|---|
| PoSe score, ban/revive heights, last paid, next payment | DMN state and projected payees | Full node or trusted indexer only |
| Owner/payout addresses, operator reward, shares | Not in the SML | Replay ProRegTx/ProUp* transactions |
| Service, type, valid/banned, voting key, operator key, platform node ID | — | Available from an SML via `mnlistdiff`/`qrinfo` |

### 10.3 Register Masternode wizard

Title: "Register Masternode" or "Register EvoNode".

**Requirements:**
- A wallet with private keys. External-signer wallets are not supported.
- The node must be ready.

**Steps:** Type → Collateral → Service → Keys → Payout → Platform (Evo only) → Fee → Review → Save operator key →
Prove collateral ownership (external collateral only) → Complete. The header shows "Step %1 of %2 · %3".

| Step | Inputs and rules |
|---|---|
| Type | Masternode (1 000 DASH) or EvoNode (4 000 DASH). |
| Collateral | Three choices:<br>(a) **Send from this wallet to a new address** (default; the address is generated fresh).<br>(b) **Existing wallet UTXO**: exact amount, ≥1 confirmation, P2PKH, not locked, not already used as collateral.<br>(c) **External collateral** (e.g. a hardware wallet): enter txid and output index. |
| Service | Comma- or space-separated `IP:port`. Default ports: 9999 main, 19999 test, 19799 devnet, 19899 regtest. **Optional** — without it the node stays inactive until an Update Service. Before v24, an Evo registration needs at least one address. |
| Keys | **Owner address** (P2PKH, fresh).<br>**Voting address** (blank means use the owner address).<br>**Operator BLS key**: generate a new one, or paste an existing 96-hex pubkey. **Basic scheme only.**<br>Owner and voting addresses must differ from the collateral address. |
| Payout | Payout address (P2PKH or P2SH; must differ from owner, voting and collateral).<br>Operator reward 0.00–100.00 %, stored ×100. Shows a warning when above 0. |
| Platform (Evo) | Platform node ID, 40 hex characters.<br>v24: Platform P2P and HTTPS address lists.<br>Pre-v24: two port fields. Default ports are main 26656/443, test 22000/22001, devnet 22100/22101, regtest 22200/22201. |
| Fee source | Pick a wallet address with spendable funds. There is no "automatic" choice. When funding the collateral from the wallet, the address must hold collateral + fee. |
| Review | Summary cards, then a send-confirm countdown. |
| Save operator key | Shows the secret and a `masternodeblsprivkey=<hex>` line, each with a copy button. The user must **type the last 4 characters** before continuing. **The secret is never stored.** |
| Prove ownership (external) | Message to sign = `payout|operatorReward|ownerAddr|votingAddr|payloadHash`. The user pastes back the base64 signmessage signature. |
| Complete | ProTx hash and next steps. |

**Error explanations** are added for these codes: `bad-protx-dup-key`, `bad-protx-dup-addr`, `bad-protx-version`,
`too-early`, and others.

**RPC equivalents:** `protx register_fund[_evo]`, `register[_evo]`, `register_prepare[_evo]` / `register_submit`.

### 10.4 Maintenance dialogs

**Update Service** (ProUpServTx):
- Fields: service address(es), the operator **secret** typed in each time (it must match the registered pubkey),
  Evo platform fields, operator payout address (only if the reward is above 0), and fee source (Automatic
  recommended).
- Also **revives a PoSe-banned** masternode.

**Update Registrar** (ProUpRegTx):
- Requires the owner key in the wallet.
- Fields: operator pubkey (legacy scheme if the ProTx version is LegacyBLS), voting address, payout address. With
  multiple payouts the payout field is disabled with the note "use the RPC".
- Only changed fields are sent.
- Warning: "Changing the operator key immediately PoSe-bans the masternode…"

**Revoke** (ProUpRevTx):
- Reason: Not specified / Termination of service / Compromised keys / Change of keys.
- Requires the operator secret.

### 10.5 Shared masternodes (v24, regular masternodes only)

**Rules:**
- 2–8 shares, each ≥ 100 DASH, summing to exactly 1000.
- The collateral is created inside the registration transaction.
- The early-exit period is ≤ 420480 blocks, with a penalty smaller than the smallest share.

**How participants exchange data:**
- **Clipboard or `.json` files only**, sent over any channel the participants choose. **There is no network
  transport.**
- Envelopes are JSON: `type "dash-shared-mn-session"`, `version 1`, `network`, `sessionId`, `revision`, `stage`,
  `fundingTx`, `shares[]`, `terms{}`, `contributions[]`, `protx`, `consentHash`, `sigs[]`, …, `fingerprint`.
- The fingerprint is 4 bytes of SHA256 shown as `XXXX-XXXX`. The session code is the first 6 hex characters of the
  sessionId.
- A message from a different network is a hard error.
- Messages over 2 MiB are refused.

**Coordinator flow:** three rounds — Invitation → Details, Locked Terms → Approvals, Signing Request → Signed
Contributions — then broadcast.

**Participant flow:** paste the message, then reserve coins, approve, and sign.

**Safety checks:**
- Refuse to sign foreign or short-changed inputs.
- Coin reservations are persistent (`lockCoin` written to the wallet database).
- Detect that a reserved coin was spent.
- Close protection: offers to save the session or release the coins.

**Maintenance actions:**
- Change Reward Address (ProUpShareTx).
- Rotate Operator/Voting Key: all owners approve, then the preparer sends.
- Dissolve:
  - **Now** (unilateral; pays the penalty during the early period, with a checkbox to accept it).
  - **Together** (unanimous; returns all principals).
  - **Standby Dissolution**: two raw transaction hex blobs saved to `.txt` for later `sendrawtransaction`. Their
    existence is recorded in QSettings `sharedmn/standby/<hash>`.
- Paste routing: when a paste is detected, sigs envelopes or standby hex are routed to the matching dialog.

**RPC equivalents:** `protx shared_register_prepare`, `shared_sign`, `shared_combine`, `shared_dissolve[_prepare]`,
`shared_update_share`, `shared_update_registrar_prepare`.

---

## 11. Governance tab

Files: `proposallist.cpp`, `proposalmodel.cpp`, `proposalvotedialog.cpp`, `proposalcreate.cpp`,
`proposalresume.cpp`, `proposalinfo.cpp`.

**Enablement:**
- Options → Display **"Show Governance Tab"** (QSettings `fShowGovernanceTab`, default off).
- Plus "Show governance clock" (QSettings `show_governance_clock`).
- Both are disabled when governance is disabled at the node, which happens with `-disablegovernance` or with
  pruning set from the Intro dialog.

### 11.1 Proposal list

**Toolbar:**
- **Source** combo: "Active Proposals", or "My Proposals" (proposals stored in the wallet; this hides the Votes
  column).
- **Filter by Title**: case-insensitive, title only. There is no status filter.
- ⓘ button: opens Tools → Information → Governance. Tooltip: "%n masternode(s) available for voting", the
  deadline, and the budget committed.
- Voting deadline label: "Voting deadline: ~%1 left (%2 blocks, block %3)" / "…passed…" / "waiting for sync…".
- Bottom row: **Votes…**, **Create Proposal**, **Resume Proposal**, "Proposal Count:".

**Columns:**

| Column | Content |
|---|---|
| Status icon | — |
| Title | — |
| Amount | — |
| Start | — |
| End | — |
| Votes | `"%1Y, %2N, %3A (%4%5)"`, where the last part is the margin vs the passing threshold. |
| My Votes | `"%1Y, %2N, %3A / %4 unvoted"` (weighted), or "No voting keys". |
| Hash | — |

**Status** is evaluated in this order:
1. **Funded** — appears in an active superblock trigger.
2. **Lapsed** — past the end date.
3. **Confirming** — fewer than 6 collateral confirmations.
4. **Pending** — not yet broadcast.
5. **Passing / Failing** — inside the maturity window.
6. **Voting** — needs more votes.
7. **Passing / Unfunded** — budget saturated.

Each status has its own tooltip.

**Threshold:** `max(nGovernanceMinQuorum, weightedValidMNs/10)`. Evo masternodes have weight 4; min quorum is 10
on mainnet and 1 elsewhere.

**Context menu:**
- Copy Raw JSON.
- Open Proposal URL… — http/https only, with an "External Link Warning" confirmation that defaults to No.
- Vote Yes / Vote No / Vote Abstain.

**Double-click:** opens a details HTML view with title, URL, destination address, payment amount, payments
requested, start, end, object hash, parent hash, collateral date and collateral hash.

### 11.2 Voting

**Who can vote:** masternodes whose **voting key** is in the wallet. Owner keys are not used.

**Dialog** ("Proposal Votes"):
- Outcome combo: Yes / No / Abstain.
- Checkable table: Masternode, Voting Address, Weight, Current Vote, Vote Time, ProTx Hash.
- Select All / Clear Selection.
- Summary of selected weight.
- **Vote %1** button.

**Process:**
- Requires a full unlock.
- Signs `CGovernanceVote(collateral outpoint, hash, FUNDING, outcome)` and relays it.
- Results box: "Voted successfully %n time(s)" / "Failed to vote %n time(s)" plus per-masternode errors.

**Limits:**
- One vote change per masternode per hour, enforced by the network ("Masternode voting too often").
- The GUI only casts **funding** votes. RPC also supports `valid`, `delete` and `endorsed`.

### 11.3 Create Proposal wizard

**Fields:**
- Proposal name: 1–40 characters, `[-_a-z0-9]`, case-insensitive.
- Description URL: no spaces, ≥ 4 characters.
- Payment date: the next 12 superblocks with estimated dates.
- Payments: 1–12.
- Payment address: P2PKH or P2SH.
- Payment amount: > 0.
- Total amount (derived).
- View JSON / View Payload buttons.

**Upstream bug:** the chosen payment date **has no effect**. `start_epoch = now − cycle/2`; `end_epoch = now +
((n−1)·cycle + cycle/2)` (all in seconds). The payload is ≤ 512 bytes.

**JSON key order:**
`{"name","payment_address","payment_amount","url","start_epoch","end_epoch","type":1}`.

**Create:**
- Requires a full unlock.
- Confirmation: "Creating a proposal pays 1 DASH to the network. This fee is non-refundable regardless of outcome."
- Creates the collateral transaction: an **`OP_RETURN <govobj hash>` output worth 1 DASH**, plus change. This is
  `gobject prepare`. The governance object is stored in the wallet (`g_object` record).
- Then opens Resume automatically.

**Resume Proposals:**
- Lists unbroadcast, unexpired wallet proposals showing title, URL, payments, collateral hash and collateral
  status (Unknown / Pending / Ready).
- **Broadcast** is enabled at ≥ 1 confirmation. It runs `gobject submit` and reports "Proposal has been
  broadcasted to the network with hash %1".
- The proposal shows as Confirming until it reaches 6 confirmations.

### 11.4 Governance info panel

Located in Tools → Information → Governance.

**General:**
- Voting cycles.
- Last superblock, Next superblock.
- Voting cutoff block (next superblock − maturity window).

**Participation:**
- Masternodes voting / EvoNodes voting, as "N (M eligible)".
- Passing threshold.

**Node:**
- Masternodes controlled, Votes controlled.

**Proposals:**
- Proposal count, passing, failing, unfunded "(short X)".
- Budget allocated: "funded / available (x%)".

**Chart:** a budget donut chart.

**Mainnet parameters:**
- Superblock cycle 16616 blocks, maturity window 1662.
- Testnet/devnet: cycle 24, window 8.
- Regtest: cycle 20, window 10.

### 11.5 SPV feasibility

| Feature | Needs |
|---|---|
| Proposals, all votes, tallies, funded status, fundable set | **Full node** in practice. Governance objects and votes come from P2P `govsync` with full-node peers. This is a large download, and peers only answer once they are themselves synced. DAPI/Platform do not serve them. |
| Casting votes, creating proposals, broadcasting proposals | Possible from SPV: sign and relay `govobjvote` / `govobj`, and build the collateral transaction. |

---

## 12. Wallet lifecycle (create / open / close / restore / migrate / encrypt / passphrase / backup / recovery phrase)

### 12.1 Multiwallet

**Wallet directory:**
- Each wallet is a directory `walletdir/<name>/wallet.dat`.
- The unnamed default wallet is `walletdir/wallet.dat`.
- walletdir is `-walletdir`, else `<datadir>/<net>/wallets` if it exists, else `<datadir>/<net>`.

**Load at startup:**
- Stored in the settings.json `"wallet"` array.
- Create, Open and Restore add the wallet to it; Close removes it.
- There is no separate GUI toggle.
- Core never auto-creates a default wallet.

### 12.2 Create Wallet dialog (`createwalletdialog.cpp/.ui`)

**Fields:**
- **Wallet Name.** Placeholder "Wallet". The OK button reads "Create" and needs a name.
- **Encrypt Wallet.** **Checked by default.**
- **Advanced options** (hidden by default):

  | Option | Default |
  |---|---|
  | Disable Private Keys | off |
  | Make Blank Wallet | off |
  | **Descriptor Wallet** | **on** in v24 (off in v23.1.8) |
  | External signer | off; enabled only if a signer is detected |

- There is **no** avoid-reuse option and **no mnemonic entry**.

**How the checkboxes interact:**

| When | Effect |
|---|---|
| Encrypt is on | Disable-keys and External signer are disabled. |
| Disable-keys is on | Encrypt goes off and Blank is forced on. |
| External signer is on | Descriptor, Disable-keys and Blank are forced on; Encrypt goes off; all are locked. |
| Exactly one signer is found | External signer is checked and the name comes from the signer. |

**Create flow:**
1. If Encrypt is checked, show the passphrase dialog.
2. Show progress "Creating Wallet <b>%1</b>…".
3. If the new wallet is HD, has private keys and can hand out addresses, run **mnemonic verification** (§12.5).
   Cancelling this does **not** undo the wallet; it only warns "Please make sure you have saved your mnemonic
   phrase safely."

**Backend notes:**
- A wallet created encrypted is created blank and then given a **random** mnemonic. `-mnemonic` is ignored on
  this path.
- `-mnemonic` / `-mnemonicpassphrase` / `-mnemonicbits` / `-hdseed` only apply when the wallet is created
  unencrypted (the first-run path).

### 12.3 Passphrase dialog (`askpassphrasedialog.cpp`)

**Modes:**

| Mode | Title | Text |
|---|---|---|
| Encrypt | "Encrypt wallet" | "…use a passphrase of **ten or more random characters**, or **eight or more words**." |
| Unlock | "Unlock wallet" | "This operation needs your wallet passphrase to unlock the wallet." |
| UnlockMixing | "Unlock wallet for mixing only" | same as Unlock |
| ChangePass | "Change passphrase" | — |

There is no Decrypt mode; Core cannot decrypt a wallet.

**Input rules:**
- Up to 1024 characters. **No minimum length.**
- Caps-lock warning: "Warning: The Caps Lock key is on!"
- A "Show passphrase" checkbox.

**Encrypt:**
- Confirmation: "Warning: If you encrypt your wallet and lose your passphrase, you will **LOSE ALL OF YOUR
  DASH**!" Defaults to Cancel.
- Success: "Your wallet is now encrypted." For HD wallets it adds that **old unencrypted backups still contain the
  same seed** — Dash keeps the seed and mnemonic when encrypting, unlike Bitcoin.
- No restart is needed.

**Unlock:**
- Wrong passphrase: "The passphrase entered for the wallet decryption was incorrect."
- A NUL-byte variant of the message exists for passphrases set before v23.

**Unlock behaviour:**
- **GUI unlock has no timeout.** The wallet stays unlocked until locked. RPC `walletpassphrase` uses a timeout.
- Going from mixing-only to full unlock re-prompts. Sending relocks back to mixing-only.

### 12.4 Wallet states (`WalletModel::EncryptionStatus`)

The states are `NoKeys`, `Unencrypted`, `Locked`, `UnlockedForMixingOnly` and `Unlocked`. They drive the menu
items and the lock icon (§2.3).

### 12.5 Mnemonic verification and Show Recovery Phrase (`mnemonicverificationdialog.cpp`)

**Step 1 — the words:**
- Shown masked as `NN. •••••••` in 3 columns (4 columns for 24+ words), with Show/Hide.
- Warning: "WARNING: If you lose your mnemonic seed phrase, you will lose access to your wallet forever. Write it
  down in a safe place and never share it with anyone."
- The "I have written down my mnemonic" checkbox is enabled only after the user has pressed Show once.

**Step 2 — verify:**
- Asks for **3 random distinct positions**, sorted.
- Input is trimmed and lowercased, then compared exactly.
- ✓/✗ is shown live; Continue is enabled only when all three match.
- Back returns to step 1.

**Show Recovery Phrase** (Settings menu):
- The same dialog in view-only mode, behind a full unlock.
- Errors: "No Recovery Phrase" (private keys disabled) or "…was not created with HD … mode…".
- **The mnemonic passphrase is never displayed in the GUI.**

### 12.6 Open, Close, Restore, Migrate, Backup, Rescan

**Open:** progress "Opening Wallet <b>%1</b>…". Failures show "Open wallet failed"; warnings show "Open wallet
warning".

**Restore** (`RestoreWallet`):
- Copies the selected `.dat` (BDB or SQLite; the format is auto-detected) to `walletdir/<name>/wallet.dat`.
- Errors: "Backup file does not exist" / "Database already exists."
- Success: "Wallet restored successfully".

**Migrate** (experimental):
- Requires the passphrase if the wallet is encrypted.
- Creates a backup `<name>-<unixtime>.legacy.bak` in the wallet directory.
- Converts to descriptor wallets, plus separate watch-only and solvables wallets if needed. The mnemonic is kept.
- Results: "Migration Successful" / "Migration failed".

**Backup Wallet:**
- A flushed copy for BDB, or an `sqlite3_backup` for SQLite.
- The filter suggests `*.dat`.

**Automatic backups** (legacy BDB only; a **no-op for SQLite**):
- Folder: `<datadir>/<net>/backups` or `-walletbackupsdir`.
- Name: `<walletname>.YYYY-MM-DD-HH-MM`.
- Keeps the newest `-createwalletbackups` (default 10, max 10).
- Created on every load, after creation, and when CoinJoin runs low on keys.

**Rescan** (Tools → Repair):
- "Rescan Chain" (from wallet birthday) and "Rescan Chain (full)" (from genesis).
- Runs in-process with a progress dialog that has **Cancel** (`abortrescan`).

---

## 13. Options dialog (`optionsdialog.cpp`, `optionsmodel.cpp`, `appearancewidget.cpp`)

**Tabs:** **Main, Wallet, CoinJoin, Network, Display, Appearance**. They are buttons over a stack, with Alt/Cmd
+1…N shortcuts.

**Tab visibility:**
- CoinJoin is visible only when CoinJoin is enabled.
- Wallet and CoinJoin are removed under `-disablewallet`.
- On macOS the tray options and start-on-login are hidden.

### 13.1 Main

| Control | Store / key | Default | Range | Restart |
|---|---|---|---|---|
| Start Dash Core on system login | OS autostart entry | off | — | no |
| Show tray icon | QSettings `fHideTrayIcon` (inverted) | shown | — | no (live) |
| Minimize to the tray instead of the taskbar | `fMinimizeToTray` | off | — | no |
| Minimize on close | `fMinimizeOnClose` | off | — | no |
| Prune block storage to [N] GB | settings.json `prune` (MiB) + `prune-prev` | off / 2 GB | ≥1 GB | yes |
| Size of database cache [MiB] | `dbcache` | 300 | 4–16384 | yes |
| Number of script verification threads | `par` | 0 (auto; <0 leaves cores free) | −cores…15 | yes |
| Enable RPC server | `server` | off | — | yes |

### 13.2 Wallet

| Control | Store / key | Default | Restart |
|---|---|---|---|
| Subtract fee from amount by default | QSettings `SubFeeFromAmount` | off | no |
| Enable coin control features | `fCoinControlFeatures` | off | no |
| Enable PSBT controls | `enable_psbt_controls` | off | no |
| Keep custom change address | `fKeepChangeAddress` | off | no |
| Spend unconfirmed change | settings.json `spendzeroconfchange` | on | yes |
| Enable CoinJoin features | `enablecoinjoin` | on | no; applied immediately |
| Enable dust attack protection + Dust threshold [duffs] | `dustprotectionthreshold` (0 = off) + `-prev` | off / 10000 | no; locks dust immediately |
| External signer script path | `signer` | "" | yes |

### 13.3 CoinJoin

See §9.3.

### 13.4 Network

| Control | Store / key | Default | Restart |
|---|---|---|---|
| Map port using UPnP | `upnp` | off | no (live) |
| Map port using NAT-PMP | `natpmp` | off | no (live) |
| Allow incoming connections | `listen` | on | yes |
| Connect through SOCKS5 proxy (default proxy): Proxy IP / Port | `proxy` = "ip:port" + `proxy-prev` | off / 127.0.0.1:9050 | yes |
| Used for reaching peers via: IPv4 / IPv6 / Tor | **Read-only indicators** | — | — |
| Use separate SOCKS5 proxy to reach peers via Tor onion services | `onion` + `onion-prev` | off / 127.0.0.1:9050 | yes |

**Proxy validation:**
- The proxy IP must be **numeric** IPv4 or IPv6 (hostnames are rejected). Port must be 1–65535.
- On failure: "The supplied proxy address is invalid." and OK is disabled.

There are no I2P or CJDNS controls.

### 13.5 Display

| Control | Store / key | Default | Restart |
|---|---|---|---|
| User Interface language | settings.json `lang` (migrated from QSettings `language`) | "(default)" = system locale | yes |
| Show Masternodes Tab | `fShowMasternodesTab` | off | no |
| Show Governance Tab | `fShowGovernanceTab` | off | no |
| Show governance clock | `show_governance_clock` | off | no |
| Unit to show amounts in | `DisplayDashUnit` | DASH | no |
| Decimal digits | QSettings `digits` (string) | "2", range 2–8 | **yes** |
| Third-party transaction URLs | `strThirdPartyTxUrls`, e.g. `https://example.com/tx/%s`; `|`-separated | "" | yes |

**Language list:** entries look like "Deutsch - Deutschland (de)". A translation help link points to Transifex.

### 13.6 Appearance (first-run "Appearance Setup" uses the same widget)

| Control | Store | Default | Range | Behaviour |
|---|---|---|---|---|
| Theme | QSettings `theme` | **Light** | Dark / Light / Traditional | live preview; reverted on Cancel |
| Font Family | settings.json `font-family` | **SystemDefault** | Montserrat (embedded) / SystemDefault | live; resets the weight sliders |
| Font Scale (Smaller ⟷ Bigger) | `font-scale` | 0 | GUI −30…+30 in steps of 10 (1 % of a 12 pt base per unit); CLI −100…100 | live |
| Font Weight Normal | `font-weight-normal` | font-dependent (Light; ExtraLight on macOS) | 0–8 = Thin…Black, limited to weights the font supports | live; Normal ≤ Bold |
| Font Weight Bold | `font-weight-bold` | font-dependent (Medium) | same | live; Bold ≥ Normal |
| Font in the Overview tab | QSettings `FontForMoney` | `application_font` | Default monospace "<family>" / Embedded "Roboto Mono" / Use existing font / Custom… | live |

**First-run dialog** ("Appearance Setup"):
- Shown while `fAppearanceSetupDone` is false.
- Text: "Please choose your preferred settings for the appearance of Dash Core".
- A single **Save** button.

### 13.7 Bottom area, Reset, OK/Cancel

**Overridden by command line:**
- "Options set in this dialog are overridden by the command line:" followed by a list of `-name=value`.
- Only the CoinJoin-enable, dust and font controls are actually disabled when overridden; the others stay
  editable but are ignored.

**Restart warnings:**
- "Client restart required to activate changes." is persistent while `fRestartRequired` is set.
- "This change would require a client restart." appears transiently for 10 s.

**Reset Options:**
1. Confirmation: "Client restart required… Current settings will be backed up at <datadir>… Client will be shut
   down. Do you want to proceed?" Defaults to Cancel.
2. Backs up `settings.json.bak` and `guisettings.ini.bak`, clears the GUI keys, sets `fReset=true` so the Intro
   shows next launch, and removes autostart.
3. **Quits** the app; it does not restart.

**OK:**
- Submits the changed values, then calls `markDirty` on each wallet so balances re-render.

**Cancel:**
- Reverts the live previews: theme, fonts and the CoinJoin toggle.

**Not in the dialog:** "Open Configuration File" (it lives in the File menu) and any keypool setting.

---

## 14. Tools window ("Debug window") (`rpcconsole.cpp`, `forms/debugwindow.ui`)

**Window:**
- Title "Tools window".
- Geometry is saved in `RPCConsoleWindowGeometry`.
- Esc and Ctrl+W close it.

**Tabs:**

| # | Tab | Shortcut |
|---|---|---|
| 1 | &Information | Ctrl+Shift+I |
| 2 | &Console | Ctrl+Shift+C |
| 3 | &Network Traffic | Ctrl+Shift+G |
| 4 | &Peers | Ctrl+Shift+P |
| 5 | &Repair | Ctrl+Shift+R |

### 14.1 Information

It has three sub-tabs. The last one used is remembered in `RPCConsoleInfoView`.

**General**

| Section | Fields |
|---|---|
| General | Client version, User Agent, Datadir, Blocksdir, Startup time |
| Network | Name (main / test / devnet / regtest), Number of connections "N (In: a / Out: b)", Local Addresses |
| Block chain | Current block height, Last block time, Last block hash |
| Memory Pool | Current number of transactions, Memory usage "x / y MB" |

**Network**

| Section | Fields |
|---|---|
| Credit Pool | Last block change, Total locked, Pending unlocks, Withdrawal limit |
| InstantSend | Verified locks, Unverified locks, Locks awaiting transaction, Unprotected transactions |
| Masternodes | "Total: X (Enabled: Y)" |
| EvoNodes | "Total: X (Enabled: Y)" |
| ChainLocks | Height, time and hash of the best ChainLock |
| Quorums | One row per LLMQ type: "N active (x.x% health)", with a tooltip showing rotation and age |

**Governance:** see §11.4. This sub-tab exists only when governance is enabled.

### 14.2 Console

**Wallet selector:**
- Shown only when 2 or more wallets are loaded.
- The first wallet is selected automatically.
- Switching prints "Executing command using \"<name>\" wallet" or "Executing command without any wallet".

**Controls:**
- Font size: Ctrl+- and Ctrl++ (4–40 pt), saved in `consoleFontSize`.
- Clear: Ctrl+L.
- The welcome text includes a red **anti-scam warning**.

**History:**
- Up/Down arrows browse it. 50 entries, kept in memory only.
- **Sensitive commands are redacted** in the echo and in history: `importprivkey`, `importmulti`, `sethdseed`,
  `signmessagewithprivkey`, `signrawtransactionwithkey`, `upgradetohd`, `walletpassphrase`,
  `walletpassphrasechange`, `encryptwallet`. Their arguments are replaced with `(…)`.

**Tab completion:** every RPC name, `help <cmd>`, and `help-console`.

**Command syntax** (shown by `help-console`):
- Arguments are separated by whitespace or commas.
- `"…"` and `'…'` quoting, with escapes.
- **Nested calls:** `getblock(getblockhash(0) 1)`.
- **Result indexing:** `[key]` and `[0]`, e.g. `getblock(getblockhash(0),1)[tx][0]`.

**Execution and errors:**
- Commands run on a worker thread. "Executing…" blocks further input; `stop` is always allowed.
- Parse errors open a box: "Error: Invalid command line".
- RPC errors are shown as "message (code N)".

**`walletpassphrase` timeout:** handled by the Qt RPC timer.

### 14.3 Network Traffic

- Range slider: 5m, 10m, 15m, **30m** (default), 1h, 2h, 3h, 6h, 12h, 24h.
- **Reset** button.
- Green area for received, red for sent, in kB/s, with Total Received / Sent.
- History is collected for every range from app start, so switching ranges loses nothing.

### 14.4 Peers

**Peer table:**
- Columns: Peer, Age, Address, Direction, Type, Network, Ping, Sent, Received, User Agent.
- Refreshes every 250 ms while visible. Multi-select.
- Header state is saved in `PeersTabPeerHeaderState`.
- Context menu: &Copy address, &Disconnect, Ban for 1 &hour / 1 d&ay / 1 &week / 1 &year.

**Banned peers:**
- Shown only when the list is non-empty.
- Columns: IP/Netmask, Banned Until.
- Context menu: &Copy IP/Netmask, &Unban.

**Detail panel**, shown for a single selected peer:
- Node Type: Regular / Masternode / Verified Masternode.
- PoSe Score, for masternodes.
- Transport (v1 / v2 BIP324) and Session ID.
- Network, Permissions, Direction/Type, Version, User Agent, Services.
- Transaction Relay, High Bandwidth.
- Starting Block, Synced Headers, Synced Blocks.
- Connection Time, Last Block, Last Transaction, Last Send, Last Receive.
- Sent, Received.
- Ping, Ping Wait, Min Ping, Time Offset.
- Mapped AS.
- Address Relay, Addresses Processed, Addresses Rate-Limited.

### 14.5 Repair

**Wallet path:** follows the console's wallet selector.

**Buttons:**

| Button | Action |
|---|---|
| **Rescan Chain** | Rescan from the wallet birthday. |
| **Rescan Chain (full)** | Rescan from genesis. |
| **Rebuild Index** | Restarts the app with `-reindex`. **No confirmation dialog.** |

Messages: "Rescan unavailable / Wallet is currently rescanning…" and "Rescan failed. Potentially corrupted data
files."

---

## 15. URI handling (`guiutil.cpp:281-389`, `paymentserver.cpp`, `openuridialog.cpp`)

### 15.1 Parsing

**Scheme and address:**
- The scheme must be exactly `dash` (case-insensitive via QUrl).
- `dash://` is rejected: "'dash://' is not a valid URI. Use 'dash:' instead."
- The address is the URL path. One trailing `/` is trimmed.

**Query parameters:**
- `label` → label.
- `message` → message.
- `amount` → amount, **always in DASH units**. An invalid value makes the whole parse fail.
- `IS` → ignored.
- `req-*`:
  - The `req-` prefix is stripped.
  - If the remainder is a known key, it is used.
  - If not, **the whole URI is rejected**.
- Any other unknown key is ignored.
- A repeated key: the last value wins.

### 15.2 Generating

Format: `dash:<addr>[?amount=<DASH, 8dp, no separators>][&label=<pct-enc>][&message=<pct-enc>]`.

- Parameters appear in that fixed order.
- The first parameter starts with `?`, the rest with `&`.

### 15.3 Errors

| Situation | Message |
|---|---|
| Unparsable URI | "URI cannot be parsed! This can be caused by an invalid Dash address or malformed URI parameters." |
| `r=` parameter, or a payment-request file | "Cannot process payment request as BIP70 is no longer supported…" |

### 15.4 Sources of URIs

- argv
- IPC from a second instance
- macOS FileOpen events
- drag-and-drop
- the **Open URI** dialog: field placeholder `dash:`, a paste button, and OK is enabled only when the URI parses
- a pasted `dash:` URI in the Send "Pay To" field

**Where URIs go:** always the regular **Send** page of the current wallet. They are queued while a send is in
progress.

**Without a wallet:** with `-disablewallet` there is no URI handler.

---

## 16. Theming, fonts, network styles, translations

### Themes

Three themes (CSS in `res/css/`):
- **Light** (default): `general.css` + `light.css`.
- **Dark**: `general.css` + `dark.css`.
- **Traditional**: `traditional.css` only, a near-native Qt look.

**How they apply:**
- Any theme whose name starts with "Dark" switches the dark colour and icon palettes.
- Icons are re-tinted for each theme.
- An invalid theme falls back to Light.

### Fonts

- Montserrat, all weights embedded.
- Roboto Mono, embedded, used for amounts and the console on macOS.
- SystemDefault.
- Weight arguments 0–8 map to Thin, ExtraLight, Light, Normal, Medium, DemiBold, Bold, ExtraBold and Black.
- Tooltips longer than 80 characters wrap.

### Network styles

| Network | Icon hue shift | Title text | macOS icon |
|---|---|---|---|
| main | 0 | "" | `dash_macos_mainnet` |
| test | +190 | "" | `dash_macos_testnet` |
| devnet | +35 | `[devnet: <name>]` | `dash_macos_devnet` |
| regtest | +160 | "" | `dash_macos_regtest` |

Testnet and regtest have empty title text, which looks like a regression. On those networks the units also
change, e.g. tDASH.

### Translations

**21 locales:** ar, bg, de, en, es, fi, fr, it, ja, ko, nl, pl, pt, ro, ru, sk, th, tr, vi, zh_CN, zh_TW.
- There are 1652 messages, and every locale is complete in this snapshot.
- Source file: `dash_en.xlf`, managed on Transifex.

**Language resolution:**
- Order: system locale → QSettings `language` → `-lang` (settings.json `lang` also feeds `-lang`).
- Qt's own `qt_<lang>` file is loaded as well as the Dash `:/translations/<lang>` file.

---

## 17. Settings storage and migration (needed if we import a user's dash-qt preferences)

### Stores

**QSettings** (separate per network; the app name is `Dash-Qt[-testnet|-regtest|-<devnet>]`):
- Windows: registry.
- macOS: plist `org.dash.Dash-Qt*`.
- Linux: `~/.config/Dash/Dash-Qt*.conf`.

**settings.json:** `<datadir>/<net>/settings.json`.

### Keys that stay in QSettings

| Area | Keys |
|---|---|
| Window and tray | `MainWindowGeometry`, `fHideTrayIcon`, `fMinimizeToTray`, `fMinimizeOnClose` |
| Display | `DisplayDashUnit`, `digits`, `strThirdPartyTxUrls`, `theme`, `FontForMoney`, `mask_values` |
| Wallet and send | `fCoinControlFeatures`, `enable_psbt_controls`, `fKeepChangeAddress`, `sCustomChangeAddress`, `SubFeeFromAmount` |
| Fees | `fFeeSectionMinimized`, `nFeeRadio`, `nConfTarget`, `nTransactionFee` |
| Coin control | `nCoinControlMode`, `nCoinControlSortColumn`, `nCoinControlSortOrder` |
| Tabs | `fShowMasternodesTab`, `fShowGovernanceTab`, `show_governance_clock` |
| CoinJoin | `fShowAdvancedCJUI`, `fShowCoinJoinPopups`, `fLowKeysWarning`, `hasMixed` |
| Data directory | `strDataDir`, `strDataDirDefault` |
| App state | `fReset`, `fRestartRequired`, `nSettingsVersion`, `fAppearanceSetupDone` |
| Transaction filters | `transactionDate`, `transactionDateFrom`, `transactionDateTo`, `transactionTypeFilter` |
| Masternode list | `mnListHideBanned`, `mnListTypeFilter`, `mnListFilterText`, `mnListOwnedOnly`, `sharedmn/standby/<hash>` |
| Tools window | `RPCConsoleWindowGeometry`, `RPCConsoleInfoView`, `consoleFontSize`, `PeersTabPeerHeaderState`, `PeersTabBanlistHeaderState`, splitter sizes |

### Keys in settings.json

- **Wallets:** `wallet` (the load-on-startup list).
- **Node:** `dbcache`, `par`, `spendzeroconfchange`, `signer`, `upnp`, `natpmp`, `listen`, `server`, `prune`.
- **Network:** `proxy`, `onion`.
- **Language:** `lang`.
- **Fonts:** `font-family`, `font-scale`, `font-weight-normal`, `font-weight-bold`.
- **Dust:** `dustprotectionthreshold`.
- **CoinJoin:** `enablecoinjoin`, `coinjoinmultisession`, `coinjoinsessions`, `coinjoinrounds`,
  `coinjoinamount`, `coinjoindenomsgoal`, `coinjoindenomshardcap`.
- **Remembered values:** `*-prev` keys.
- Some numeric values are stored **as strings**, for Bitcoin 22.x compatibility.

### Migration

`checkAndMigrate`:
- Moves the old QSettings keys (`nDatabaseCache`, `fUseProxy`/`addrProxy`, `nCoinJoin*`, …) into settings.json.
- **Deletes** the legacy `PrivateSend*` keys without migrating them.
## 18. Wallet file formats, HD and mnemonic standard, backup compatibility

This section matters for two migrations: restoring a dash-qt wallet in our app, and moving our wallet back into dash-qt.

### 18.1 Database formats

**Format detection:**

| Format | How to recognize it |
|---|---|
| **Legacy BDB 4.8.30** `wallet.dat` | File is at least 4096 bytes. Magic `62 31 05 00` (LE) at offset 12. |
| **Descriptor SQLite** `wallet.dat` | Header `"SQLite format 3\0"`. `PRAGMA application_id` at offset 68 equals the network magic: mainnet `bf 0c 6b bd`, testnet `ce e2 ca ff`. `user_version` is 0. |

- If a file matches both formats, it is rejected as ambiguous.
- SQLite schema: a single table `main(key BLOB PRIMARY KEY, value BLOB)`.

**Defaults:**
- v24 default: descriptor (SQLite). This applies to the GUI checkbox and to `createwallet descriptors=true`.
- v23.1.8 default: legacy BDB.
- BDB wallets can still be created and read.

**Migration:**
- `migratewallet` is still marked EXPERIMENTAL.
- There is **no read-only BDB parser** in Core. Reading BDB needs libdb 4.8.

**Record keys** (`walletdb.cpp:38-75`):

| Category | Keys |
|---|---|
| Encryption | `mkey`, `key`, `ckey` |
| Legacy HD chain | `hdchain`, `chdchain`, `hdpubkey` |
| Key pool and scripts | `keymeta`, `pool`, `cscript`, `watchs`, `watchmeta` |
| Address book | `name`, `purpose`, `destdata` |
| Transactions and chain state | `tx`, `bestblock(_nomerkle)`, `orderposnext`, `lockedutxo` |
| Wallet metadata | `flags`, `minversion`, `version`, `settings` |
| Descriptors | `walletdescriptor`, `walletdescriptorcache`, `walletdescriptorlhcache`, `walletdescriptorkey`, `walletdescriptorckey`, `activeexternalspk`, `activeinternalspk` |
| **Dash-specific** | `ps_salt`, `cj_salt`, `cj_pending_obs`, `g_object` (governance proposals), `platform_data` |

**Wallet flags:**

| Flag | Value |
|---|---|
| AVOID_REUSE | `1<<0` |
| KEY_ORIGIN_METADATA | `1<<1` |
| LAST_HARDENED_XPUB_CACHED | `1<<2` |
| DISABLE_PRIVATE_KEYS | `1<<32` |
| BLANK_WALLET | `1<<33` |
| DESCRIPTORS | `1<<34` |
| EXTERNAL_SIGNER | `1<<35` |

An unknown flag at bit 32 or above means the wallet will not open.

**`dash-wallet` tool:**
- Commands: `info | create | salvage | wipetxes | dump | createfromdump`.
- Dump format:
  - First line: `BITCOIN_CORE_WALLET_DUMP,1`.
  - Then `format,<bdb|sqlite>`.
  - Then one hex key/value pair per line.
  - Last line: `checksum,<sha256>`.
- This is a portable way to get every record out of either format.

### 18.2 Mnemonic and derivation (both wallet types)

**BIP39** (`src/wallet/bip39.cpp`):
- **English wordlist only.**
- Word count: 12, 15, 18, 21 or 24. The CLI `-mnemonicbits` takes 128–256 in steps of 32; the default is 128, giving 12 words.

**Checksum quirk:**
- The mask is `(2 ^ cs_len) << (8 - cs_len)`, which is XOR, not a power.
- Only some checksum bits are checked:

  | Words | Checksum bits checked |
  |---|---|
  | 12 | 2 of 4 |
  | 15 | 3 of 5 |
  | 18 | 1 of 6 |
  | 21 | 2 of 7 |
  | 24 | 2 of 8 |

- Phrases that Core generates are always valid. Phrases that users import may not be.

**Seed derivation:**
- `PBKDF2-HMAC-SHA512(mnemonic bytes, ("mnemonic"+passphrase)[0:256], 2048, 64)`.
- **No NFKD normalization.** The salt is truncated to 256 bytes.
- NUL bytes in the passphrase are allowed since v23.
- The mnemonic passphrase may be up to 256 characters.

**BIP32 and BIP44:**
- Master key: BIP32 "Bitcoin seed" HMAC.
- Coin type: **5'** on mainnet, **1'** on testnet, devnet and regtest.
- Only account 0 is used.

**Derivation paths:**

| Purpose | Path |
|---|---|
| Receive (external) | `m/44'/5'/0'/0/i` |
| Change (internal) | `m/44'/5'/0'/1/i` |
| Descriptor wallets only, since v23: DIP9 "mobile CoinJoin" (watched, never used for new addresses) | `m/9'/5'/4'/0'/0/i` |

**Legacy HD** (`CHDChain`):
- Serialized as `{nVersion=1, id=Hash(seed), fCrypted, vchSeed, vchMnemonic, vchMnemonicPassphrase, map<account,{extCounter,intCounter}>}`.
- Encrypted legacy chains are AES-256-CBC under the wallet master key, with IV = first 16 bytes of the chain id.
- Derived private keys are **not stored**. Only `hdpubkey` records are stored, and the keys are re-derived.

**Descriptor wallets:**
- Three descriptors:
  - `pkh(xprv/44h/<c>h/0h/0/*)` — active, external.
  - `pkh(xprv/44h/<c>h/0h/1/*)` — active, internal.
  - `pkh(xprv/9h/<c>h/4h/0h/0/*)` — inactive. `listdescriptors` marks it `"coinjoin": true`.
- The mnemonic and passphrase are stored next to the master key in `walletdescriptorkey` / `walletdescriptorckey`. The value is `((privkey, hash), (mnemonic, passphrase))`.
- For encrypted wallets the IV is `Hash(pubkey)[0:16]`.
- `listdescriptors true` returns `desc, mnemonic, mnemonicpassphrase, timestamp, active, internal, coinjoin, range, next, next_index`.

**Other key sources:**
- `-hdseed=<hex>` (legacy only) and `sethdseed` (legacy only, WIF → raw seed) create HD wallets **without a mnemonic**.
- `upgradetohd "mnemonic" "mnemonicpassphrase" "walletpassphrase" rescan`:
  - Turns a non-HD or **blank** wallet of either type into an HD wallet.
  - An empty mnemonic generates a new one.
  - This is the **only way to restore a mnemonic into an existing dash-qt wallet**. There is no GUI screen for it.
- `dumphdinfo` (legacy only) returns `{hdseed, mnemonic, mnemonicpassphrase}`.

### 18.3 CoinJoin key usage (affects restore scanning)

**Which chain each kind of output uses:**

| Output | Chain |
|---|---|
| Core mixing outputs and denomination creation | **ordinary external chain** `m/44'/c'/0'/0/i` |
| Collateral change | internal chain |
| dashj / Android mixing | `m/9'/c'/4'/0'/0/i` |

**Keypool:**
- `-keypool` defaults to **1000**. Legacy wallets keep 1000 keys ahead on each chain. Descriptor wallets keep `range_end = next_index + 1000`.
- CoinJoin reserves keys for sessions that never confirm, so **gaps far larger than 20 are normal**.

**Restore implication:** scan with a lookahead of at least 1000 on both BIP44 chains and on the DIP9 chain, and extend the lookahead whenever a key is found in use.

### 18.4 Encryption scheme

**Master key:**
- `CMasterKey{crypted, salt[8], method 0, iterations, other}`.
- Iterations are calibrated to about 100 ms, with a minimum of 25000.
- Key and IV come from an EVP_BytesToKey-style SHA-512 loop: `sha512(pass‖salt)`, then `iterations−1` more rounds. The first 32 bytes are the key and the next 16 are the IV.
- AES-256-CBC protects the 32-byte master key.

**Key records:**
- `ckey` and descriptor `ckey` records are AES-256-CBC under the master key, with IV = `Hash(pubkey)[0:16]`.

**Behavior on encrypt and passphrase change:**
- Encrypting keeps the same seed and mnemonic. This differs from Bitcoin Core, which generates new keys on encryption.
- Changing the passphrase re-encrypts only the master key.

### 18.5 dumpwallet / importwallet (legacy only)

Descriptor wallets cannot use dumpwallet, importwallet or the other legacy import RPCs. They fail with "Only legacy wallets are supported by this command".

**dumpwallet output format:**

```
# Wallet dump created by Dash Core <version>
# * Created on <ISO8601>
# * Best block at time of backup was <height> (<hash>),
#   mined on <ISO8601>

# mnemonic: <words>
# mnemonic passphrase: <passphrase>

# HD seed: <hex>

# extended private masterkey: <xprv>
# extended public masterkey: <xpub>

# external chain counter: N
# internal chain counter: M

<WIF> <ISO8601> label=<%xx-encoded> # addr=<P2PKH> hdkeypath=m/44'/5'/0'/0/i
<WIF> <ISO8601> reserve=1 # addr=…
<WIF> <ISO8601> change=1 # addr=…
<hexscript> <ISO8601|0> script=1 # addr=<P2SH>

# End of dump
```

Format notes:
- Keys are sorted by birth time.
- Labels are %xx-encoded for bytes ≤ 32, bytes ≥ 128, and `%`.
- Watch-only scripts are not dumped.
- There are no `hdseed=1` or `inactivehdseed=1` lines (those are Bitcoin-only).
- dumpwallet refuses to overwrite an existing file.

**importwallet behavior:**
- Imports each WIF as a **loose key** and each script.
- **Ignores the HD/mnemonic comment lines.**
- Rescans from the earliest key time.

**Other legacy-only RPCs:** `importprivkey`, `importaddress`, `importpubkey`, `importmulti`, `importelectrumwallet`, `dumpprivkey`.

**Descriptor wallets** use `importdescriptors` instead. It takes no mnemonic.

**Removed:** KeePass integration is gone.

### 18.6 What our app must implement for compatibility

**1. Restore from a dash-qt mnemonic (with optional passphrase)**
- Normalize input to lowercase, single-spaced English words.
- Validate with strict BIP39. If that fails, fall back to Core's weak checksum check and show a warning.
- Use Core's exact seed function: no NFKD, salt truncated to 256 bytes, NUL bytes allowed.
- Scan `44'/c'/0'/0`, `44'/c'/0'/1` and `9'/c'/4'/0'/0` (P2PKH, plus P2PK to be safe), with a lookahead of at least 1000.

**2. Restore from other secrets dash-qt can produce**
- The raw `hdseed` hex (this may be a 32-byte `sethdseed` seed rather than a BIP39 seed).
- An xprv from the dump file, or from `listdescriptors true`.
- Warn that pre-HD wallets, and wallets upgraded with `upgradetohd`, may contain random keys outside the HD chain. Those need a key dump or the wallet file itself.

**3. Import a dash-qt dumpwallet file**
- Parse the comment header and rebuild the HD wallet from it. This is better than Core's own importwallet, which drops the mnemonic.
- Import the WIF keys with their labels and the reserve/change markers. Use `hdkeypath` to recognize HD keys.
- Import P2SH scripts.
- Use the earliest key time as the wallet birthday.

**4. Import `wallet.dat` directly**
- SQLite: read the `main` table directly.
- BDB: needs a custom read-only parser for BDB 4.8 btree pages.
- Records to decode: `mkey`, `(c)hdchain`, `(c)key`, `walletdescriptor*`, `name`, `purpose`, `keymeta`/`pool`/`hdpubkey`, `cscript`, `watchs`, `lockedutxo` and `flags`.
- Seed the lookahead from `mapAccounts[0]` counters or the descriptor `next_index`.

**5. Export to dash-qt** (in order of preference)
- (a) **Mnemonic + passphrase.** The user creates a **Blank** wallet in dash-qt, then runs `upgradetohd "<words>" "<pass>" "<walletpass>"` in the console. This works for both wallet types and makes Show Recovery Phrase work. Only export phrases that also pass Core's check.
- (b) A **dumpwallet-format** text file. This imports only into a *legacy* wallet, and only as loose keys.
- (c) An **`importdescriptors` JSON** containing the three descriptors with checksums. This does not include the mnemonic.
- (d) Stretch goal: write a **SQLite descriptor `wallet.dat`** that dash-qt opens via Restore Wallet. This is the only true one-file import.

**6. Parity decision: displaying the mnemonic passphrase**
dash-qt never shows the mnemonic passphrase. Decide whether our app shows it.

---

## 19. RPC console surface (what power users reach through the console)

The console runs any RPC. These are the user-relevant groups in develop:

- **Wallet:**
  - Status and info: `getwalletinfo`, `getbalance(s)`, `getunconfirmedbalance`, `listwallets`, `listwalletdir`.
  - Addresses and labels: `getnewaddress`, `getrawchangeaddress`, `getaddressinfo`, `getaddressesbylabel`, `listlabels`, `setlabel`, `listaddressbalances`, `listaddressgroupings`.
  - Transaction history: `gettransaction`, `listtransactions`, `listsinceblock`, `listreceivedbyaddress/label`, `getreceivedbyaddress/label`.
  - Coins and UTXO locks: `listunspent`, `lockunspent`, `listlockunspent`.
  - Sending: `send`, `sendall`, `sendmany`, `sendtoaddress`, `settxfee`.
  - Abandoning: `abandontransaction`.
  - Key pool: `keypoolrefill`, `newkeypool`.
  - Wallet lifecycle: `createwallet`, `loadwallet`, `unloadwallet`, `restorewallet`, `migratewallet`, `backupwallet`, `upgradewallet`, `setwalletflag`.
  - Encryption and locking: `encryptwallet`, `walletpassphrase` (with `mixingonly`), `walletpassphrasechange`, `walletlock`.
  - Import and export: `dumpwallet`, `importwallet`, `dumpprivkey`, `dumphdinfo`, `importprivkey`, `importaddress`, `importpubkey`, `importmulti`, `importdescriptors`, `listdescriptors`, `importelectrumwallet`, `importprunedfunds`, `removeprunedfunds`.
  - HD seed: `upgradetohd`, `sethdseed`.
  - Multisig and message signing: `addmultisigaddress`, `signmessage`.
  - PSBT and raw transactions: `walletcreatefundedpsbt`, `walletprocesspsbt`, `signrawtransactionwithwallet`, `simulaterawtransaction`.
  - Hardware signer: `walletdisplayaddress`.
  - Rescanning: `rescanblockchain`, `abortrescan`.
  - Maintenance: `wipewallettxes`.
  - CoinJoin settings: `setcoinjoinrounds`, `setcoinjoinamount`.
- **CoinJoin:** `coinjoin start|stop|reset|status`, `coinjoinsalt generate|get|set`, `getcoinjoininfo`.
- **Governance:**
  - `gobject` subcommands: `check`, `prepare`, `list-prepared`, `submit`, `deserialize`, `count`, `get`, `getcurrentvotes`, `list`, `diff`, `vote-alias`, `vote-many`.
  - Also: `voteraw`, `getgovernanceinfo`, `getsuperblockbudget`.
- **Masternodes and evo:**
  - `masternode` subcommands: `count`, `status`, `outputs`, `payments`, `winners`, `connect`.
  - Also: `masternodelist`.
  - `protx` registration: `register[_evo|_fund|_prepare]`, `register_submit`.
  - `protx` updates: `update_service[_evo]`, `update_registrar`, `revoke`.
  - `protx` queries: `list`, `info`, `diff`, `listdiff`.
  - `protx shared_*`.
  - BLS keys: `bls generate|fromsecret`.
  - Quorums: `quorum *`.
  - Lock verification: `verifychainlock`, `verifyislock`, `getbestchainlock`.
- **Utility:** `validateaddress`, `verifymessage`, `signmessagewithprivkey`, `createmultisig`, `deriveaddresses`, `getdescriptorinfo`, `estimatesmartfee`.
- **Raw transactions:** standard Bitcoin set, plus `getislocks`, `gettxchainlocks`, `getassetunlockstatuses`.
- **Node:** standard Bitcoin set, plus `mnsync`, `spork`, `getspecialtxes`, `getcreditpoolinfo`, and the address-index RPCs when `-addressindex` is enabled.

An SPV app cannot offer a full Core RPC console. Possible substitutes:
- A local wallet-command console.
- A "connect to my node" RPC client.

---

## 20. Full node vs SPV/DAPI: per-feature matrix

| Feature area | dash-qt today | SPV / DAPI feasibility for our app |
|---|---|---|
| Balances, history, send/receive, labels, address book, QR/URI, sign/verify, PSBT, coin control, UTXO lock, dust protection | wallet + chain | **Yes.** Use BIP157/158 filters or bloom filters plus headers. Fee estimation needs a fallback or an external estimate, because there is no mempool or block stats. |
| InstantSend lock status, ChainLock status | node | **Yes.** Verify `islock`/`clsig` against quorum keys obtained via `qrinfo`/`mnlistdiff`. dash-spv and dashj already do this. |
| CoinJoin mixing | node + wallet | **Yes.** dashj is the reference. Needs the MN list, `dsq` relay, direct masternode connections, BLS verification, an approximation of collateral validity, and an exact port of the "fully mixed" salt rule. |
| Masternode list: service, type, valid/banned, voting/operator keys, platform ID | DMN list | **Yes**, via `mnlistdiff`/`qrinfo` (SML). |
| Masternode PoSe score, ban heights, last paid, next payment, owner/payout addresses, shares | DMN full state | **No** for PoSe and payments; those need a full node or a trusted indexer. Owner/payout addresses and shares can be recovered by replaying ProRegTx/ProUp* transactions. |
| ProTx register / update / revoke, shared MN | wallet + node validation | Building and signing: **yes**. Losing pre-broadcast node validation (dup key, netinfo, v24 version) means more failures appear only after broadcast. |
| Governance list, tallies, funded/fundable status, budget %, governance clock | governance object sync | **Full node** in practice: a P2P `govsync` of all objects and votes from full peers. DAPI has no governance data. |
| Voting, proposal creation and broadcast | wallet + node | **Yes.** Sign and relay `govobjvote`/`govobj`; the collateral is an ordinary transaction. |
| Debug → Information: version, connections, header height | node | Yes |
| Debug → mempool stats, credit pool, InstantSend counters, quorum health | node | No, or partially (quorum info is available from `qrinfo`). |
| Peers table, ban/disconnect, traffic graph | node | Yes, for our own P2P stack. |
| RPC console | node | No; substitute a local command console or a remote-node RPC client. |
| Rescan / Rebuild Index | node | Rescan maps to filter re-sync from the wallet birthday. Reindex does not apply. |
| Intro prune/size, dbcache, par, listen, UPnP/NAT-PMP, RPC server | node | Not applicable; proxy/Tor settings still apply. |

---

## 21. Upstream quirks and bugs (decide deliberately whether to copy them)

1. **No title text on testnet or regtest.** The window and splash show nothing; only the icon tint differs (`networkstyle.cpp:28-30`).
2. **DASH unit description prints the literal "%1Dash"** (`bitcoinunits.cpp:50`).
3. **External-signer wallets show "External balance" as 0.**
4. **Wrong text in the PSBT-only confirmation.** It shows both "create" and "draft" questions, with a missing space.
5. **Custom-fee tooltip describes an "at least" mode that no longer exists.**
6. **Sending overwrites address-book labels**, including your own receiving labels, or clears them when the label is empty.
7. **Sending to the same address twice only asks for confirmation.** Bitcoin Core blocks this.
8. **The "Payment date" in the proposal wizard has no effect.**
9. **`-enablecoinjoin` help says default 0, but the actual default is 1.**
10. **The automatic-backup gate disables CoinJoin for descriptor wallets** even though those wallets never create automatic backups.
11. **Two different governance funding thresholds:** the list margin uses a min-quorum floor; `Proposal::status` does not.
12. **The BIP39 checksum check is weak**, and seed derivation skips NFKD (§18.2).
13. **Prune spin box minimum is always 1**, due to an operator-precedence bug.
14. **Reset Options clears QSettings a second time on exit**, which also wipes the restored `strDataDir` (inherited from Bitcoin Core).
15. **The proxy status-bar tooltip goes stale** when the proxy changes.
16. **Masternodes-tab tooltip still mentions sub-tabs that no longer exist.**
17. **Untranslated strings:** "Unsigned Transaction", "PSBT Operations", "CoinControl" and the address-book CSV headers.

---

## 22. Parity checklist

Each line is an acceptance item. **Scope** marks what the feature depends on:
- **[W]** wallet only (no network)
- **[S]** works with SPV
- **[F]** full node or indexer data needed
- **[U]** UI or platform behaviour only

IDs are stable; group prefixes are only for readability.

1. **QT-001** [Shell] [U] Single-instance app; a second launch with `dash:` URIs hands them to the running instance and exits; a second launch without URIs does not start a second copy.
2. **QT-002** [Shell] [U] Separate settings and data per network (mainnet/testnet/devnet/regtest), chosen by flag or config (`-testnet`, `-regtest`, `-devnet=<name>`, `-chain=`).
3. **QT-003** [Shell] [U] Network-specific branding: icon tint per network; devnet label `[devnet: <name>]`; testnet units tDASH/mtDASH/μtDASH/tduffs.
4. **QT-004** [Shell] [U] First-run data/wallet directory chooser: default or custom path, free-space check, create the directory; `-choosedatadir` forces it.
5. **QT-005** [Shell] [U] Splash/loading screen with phase-based progress and an emergency quit key (Q) during startup.
6. **QT-006** [Shell] [U] Start minimized (`-min`) and hide the splash; `-resetguisettings`; `-lang`; `-windowtitle`; font and theme CLI overrides (`-font-family`, `-font-scale`, `-font-weight-*`).
7. **QT-007** [Shell] [U] Corrupt settings-file prompt with Reset / Abort.
8. **QT-008** [Shell] [U] Shutdown window that cannot be closed ("…is shutting down… Do not shut down the computer…"); graceful quit from menu, tray, Dock, Cmd+Q and OS session end.
9. **QT-009** [Shell] [U] Start on system login (Windows Startup shortcut / Linux XDG autostart, launched with `-min`); hidden on macOS.
10. **QT-010** [Shell] [U] Fatal and runaway error dialogs, and a non-fatal "Internal error" dialog.
11. **QT-011** [Window] [U] Window title: `Dash Core - <wallet name> - <network>`; window geometry saved and restored.
12. **QT-012** [Window] [U] Tab bar: Overview, Send, Receive, Transactions, CoinJoin*, Masternodes*, Governance* (* optional), with Alt/Cmd+1…N shortcuts renumbered by visible tabs.
13. **QT-013** [Window] [U] "No wallet loaded" panel with a "Create a new wallet" button.
14. **QT-014** [Window] [W] Wallet selector combo box (shown only with 2+ wallets); switching wallets updates title and pages.
15. **QT-015** [Window] [U] File menu items: Create, Open ▸, Close, Close All, Migrate, Backup, Restore, Open URI, Sign message, Verify message, Load PSBT (file / clipboard), Open debug log, Open config file, Show Automatic Backups, Exit (Ctrl+Q).
16. **QT-016** [Window] [U] Settings menu: Encrypt Wallet, Change Passphrase, Show Recovery Phrase, Unlock Wallet, Lock Wallet, Discreet mode (Ctrl+Shift+D), Options.
17. **QT-017** [Window] [U] Window menu: Minimize (Ctrl+M), Sending addresses, Receiving addresses, and tools tabs Information/Console/Traffic/Peers/Repair (Ctrl+Shift+I/C/G/P/R).
18. **QT-018** [Window] [U] Help menu: Command-line options dialog, CoinJoin information dialog (only when CoinJoin is on), About Dash Core (version and licence), About Qt (or an equivalent toolkit credit).
19. **QT-019** [Window] [U] Drag and drop a `dash:` URI onto the window opens Send pre-filled.
20. **QT-020** [Status] [U] Status bar unit selector (DASH/mDASH/μDASH/duffs) that changes the display unit everywhere.
21. **QT-021** [Status] [W] HD status icon (green, tooltip "HD key generation is enabled").
22. **QT-022** [Status] [W] Wallet lock icon with 4 states: unencrypted, unlocked, unlocked for mixing only (orange), locked; tooltips match.
23. **QT-023** [Status] [S] Proxy icon (shown when a proxy is set, tooltip `ip:port`); clicking opens Network options.
24. **QT-024** [Status] [S] Connections icon with 5 levels, plus states for 0 peers (blinking) and network disabled; menu to Show Peers and Disable/Enable network activity.
25. **QT-025** [Status] [S] Sync spinner, then "synced" icon; progress text "Synchronizing with network… / Syncing Headers (x%)… / Connecting to peers…"; progress bar "<time> behind" with catch-up tooltip.
26. **QT-026** [Status] [F] Governance clock (moon phase) showing voting-period progress, superblock ETA and % of budget committed; opt-in; clicking opens Governance.
27. **QT-027** [Status] [S] Sync overlay: blocks left, last block time, progress %, progress per hour, ETA, Hide button; appears automatically while the tip is more than 25 minutes old.
28. **QT-028** [Tray] [U] Tray icon with tooltip; left-click shows or hides the window; option to hide the tray icon.
29. **QT-029** [Tray] [U] Tray / Dock menu: Show/Hide, Send, CoinJoin, Receive, Sign, Verify, Options, Information, Debug console, Network Monitor, Peers list, Wallet Repair, debug log, config file, automatic backups, Exit; disabled while a modal dialog is open.
30. **QT-030** [Tray] [U] Minimize to tray; minimize on close (Windows and Linux).
31. **QT-031** [Notify] [W] Desktop notification for each incoming or sent transaction: Date, Amount, Wallet (multiwallet only), Type, Label or Address.
32. **QT-032** [Notify] [W] Notifications batched; summary "Received/Sent multiple transactions" when 100 or more are pending; none during initial sync.
33. **QT-033** [Notify] [W] Option to suppress notifications for CoinJoin mixing transactions (`fShowCoinJoinPopups`, default on).
34. **QT-034** [Overview] [W] Balances: Available, Pending, Immature (shown only when non-zero), Total.
35. **QT-035** [Overview] [W] Watch-only balance column (Available/Pending/Immature/Total), shown when the wallet has watch-only scripts.
36. **QT-036** [Overview] [W] Amounts truncated to the "Decimal digits" setting (default 2, range 2–8), with thin-space thousands separators and the unit.
37. **QT-037** [Overview] [S] "(out of sync)" labels with explanatory tooltip until sync completes.
38. **QT-038** [Overview] [W] Recent transactions list (5, 6 or 8 rows depending on CoinJoin mode) that hides CoinJoin-internal, dust and conflicted transactions; shows date, InstantSend lock icon, signed coloured amount, label or address; click jumps to Transactions with that transaction selected.
39. **QT-039** [Overview] [W] Discreet mode: every digit shown as `#`, recent transactions hidden; applies to the Overview only; setting persisted.
40. **QT-040** [Overview] [U] Alert/warning banner showing node warnings (e.g. prerelease build).
41. **QT-041** [CoinJoin] [S] CoinJoin panel on Overview: Status (Enabled/Disabled, keys left in advanced mode), CoinJoin Balance, Amount and Rounds (red "~X" when inputs are insufficient), Completion bar and Submitted Denom (advanced mode), Start/Stop button.
42. **QT-042** [CoinJoin] [W] Mixing progress % computed exactly with dash-qt's formula (denominated weight 1, normalized weight = rounds, fully mixed weight 2) and the matching tooltip breakdown.
43. **QT-043** [CoinJoin] [W] "Fully mixed" rule including the per-wallet CoinJoin salt coin-flip (rounds ≥ N, then ≥ N+3 or odd hash), so CoinJoin balance matches Core.
44. **QT-044** [CoinJoin] [S] Start mixing: minimum balance 0.00140001 DASH, first-use hint, unlock-for-mixing-only prompt; Stop resets the pool.
45. **QT-045** [CoinJoin] [S] Full CoinJoin client protocol (dsa/dsi/dsf/dss/dsc/dssu/dsq/dstx/senddsq), masternode selection, BLS verification of dsq, collateral handling, multi-session, denomination goal and hard cap, post-V24 promotion/demotion.
46. **QT-046** [CoinJoin] [W] CoinJoin settings: enable (default on), rounds 2–16 (4), target 2–21M (1000), multi-session (off), sessions 1–10 (4), denominations target 10–100000 (50) and max (300; target ≤ max); applied live.
47. **QT-047** [CoinJoin] [W] Advanced CoinJoin UI toggle, low-keys warning toggle, popups toggle.
48. **QT-048** [CoinJoin] [W] Mixing disabled when backups are disabled or failed, or the keypool is exhausted (legacy wallets); keys-left warning below 100 keys; stop below 50.
49. **QT-049** [CoinJoin] [W] Per-wallet mixing state in multiwallet; CoinJoin options are global.
50. **QT-050** [CoinJoin] [W] Mixing-session status text (equivalent of `coinjoin status`) available to the user (beyond dash-qt, recommended).
51. **QT-051** [Send] [W] Separate "CoinJoin" send page that spends only fully mixed funds, shows the mixed balance, has no change output (excess goes to fee), tags the transaction DS=1, and has the button "Send mixed funds".
52. **QT-052** [Send] [W] Multiple recipients: Add Recipient, remove entry, Clear All.
53. **QT-053** [Send] [W] Recipient fields: Pay To (with address-book picker Alt+A and paste Alt+P), Label (auto-filled from the book), Amount, "Subtract fee from amount", "Use available balance".
54. **QT-054** [Send] [W] Paste a `dash:` URI into Pay To to fill address, amount, label and message; the URI message is shown read-only and stored locally.
55. **QT-055** [Send] [W] Per-entry validation (invalid address, amount ≤ 0, dust about 546 duffs) that highlights the bad field.
56. **QT-056** [Send] [W] Amount field in the current unit; "," accepted as "."; never localized; 0–21M DASH; reformatted on blur.
57. **QT-057** [Send] [S] Fee: Recommended (smart fee with confirmation target choices 2/4/6/12/24/48/144/504/1008 blocks shown as 5 min…42 h) or Custom per kB (minimum 1000 duff/kB); fallback-fee warning; collapsible fee section; settings persisted.
58. **QT-058** [Send] [W] Fee caps: maximum transaction fee 0.1 DASH and an "absurdly high fee" check.
59. **QT-059** [Send] [W] Confirmation dialog: recipients (max 10 listed), funds source (CoinJoin only / any), fee, size, fee rate, input count and ≥10-input privacy warning (CoinJoin page), total with alternate units; Send disabled for a 3 s countdown; default button Cancel.
60. **QT-060** [Send] [W] Duplicate-recipient confirmation (Yes/Cancel), not a hard error.
61. **QT-061** [Send] [W] Full unlock requested before signing; mixing-only state restored afterwards.
62. **QT-062** [Send] [W] Exact error messages for invalid address, invalid amount, amount exceeds balance, total-with-fee exceeds balance, creation failed, absurd fee, insufficient mixed funds.
63. **QT-063** [Send] [W] After sending: recipient added to the address book (purpose send), form cleared, jump to Transactions with the new transaction selected.
64. **QT-064** [Send] [S] InstantSend automatic (no toggle); InstantSend-locked inputs are spendable immediately; ChainLocked transactions count as confirmed.
65. **QT-065** [Send] [W] "Spend unconfirmed change" option (default on).
66. **QT-066** [Send] [W] No RBF/bumpfee (Dash has none); nSequence = final−1; BIP69 input/output ordering.
67. **QT-067** [Send] [W] Send to P2SH addresses; reject Platform (DIP-18) addresses with "This is a Dash Platform address, not a Dash Core address".
68. **QT-068** [CoinCtl] [W] Coin control enable option; Send-page panel: Inputs…, automatic vs selected, Quantity/Bytes/Amount/Fee/After Fee/Change, "Insufficient funds!", copy actions on each value.
69. **QT-069** [CoinCtl] [W] Coin Selection dialog: list mode (default) and tree mode, columns Amount/Label/Address/Mixing Rounds/Date/Confirmations, sorting persisted, (un)select all, (un)lock all, locked count.
70. **QT-070** [CoinCtl] [W] UTXO context menu: copy address/label/amount/`txid:vout`, Lock / Unlock unspent; locks persist across restarts.
71. **QT-071** [CoinCtl] [W] CoinJoin coins hidden by default on the regular page (Show all / Hide CoinJoin coins); the CoinJoin page shows only fully mixed coins (Show all CoinJoin coins / Show spendable coins only).
72. **QT-072** [CoinCtl] [W] Size and fee estimate (148 bytes/input, 34 bytes/output, +10), "≈" prefix, dust change added to fee, CoinJoin overpay.
73. **QT-073** [CoinCtl] [W] Custom change address with validation and "Unknown change address" confirmation; "Keep custom change address" option.
74. **QT-074** [CoinCtl] [W] Spent selected coins auto-unselected with a notice.
75. **QT-075** [Dust] [W] Dust attack protection option (threshold default 10000 duffs, range 1–1,000,000) that automatically locks small foreign incoming UTXOs; "Dust Receive" transaction type; "Unlock dust UTXO" action.
76. **QT-076** [PSBT] [W] "Enable PSBT controls" option, which adds "Create Unsigned" to the send confirmation.
77. **QT-077** [PSBT] [W] Watch-only / no-private-key wallets: "Create Unsigned" PSBT copied to the clipboard and offered for saving as binary `.psbt` with a suggested file name.
78. **QT-078** [PSBT] [W] Load PSBT from file (binary or base64, under 100 MiB) or from clipboard (base64).
79. **QT-079** [PSBT] [S] PSBT Operations dialog: summary (sends, own address, fee, total, unsigned input count), status analysis text, Sign Tx, Broadcast Tx, Copy to Clipboard, Save…
80. **QT-080** [PSBT] [W] External signer (HWI) support: script path option, "Sign on device" send button, external-signer wallets, show address on device (Verify).
81. **QT-081** [Receive] [W] Receive form: Label, Amount, Message (all optional), Create new receiving address, Clear; unlock and retry when the keypool is locked or empty.
82. **QT-082** [Receive] [W] Request payment dialog: QR code with the address drawn under it, URI, Address, Amount, Label, Message, Wallet; Copy URI, Copy Address, Save Image (PNG).
83. **QT-083** [Receive] [W] Requested payments history stored in the wallet: columns Date/Label/Message/Requested; Show/Remove; context copy URI/address/label/message/amount.
84. **QT-084** [Receive] [U] QR: ECC level L, URI limit 255 characters with error "Resulting URI too long…", right-click Save/Copy image, drag out.
85. **QT-085** [Receive] [W] URI generation `dash:<addr>?amount=<DASH 8dp>&label=&message=` in this exact order with percent-encoding.
86. **QT-086** [Tx] [W] All 19 transaction types with their exact display strings, including CoinJoin, Platform Transfer, Asset Lock, Masternode Registration/Update, Data, Dust Receive.
87. **QT-087** [Tx] [S] Status model: Unconfirmed / Confirming (x of 6) / Confirmed / Conflicted / Abandoned / Immature / Not accepted; ChainLock confirms immediately; ", verified via InstantSend" / ", locked via ChainLocks" suffixes; status icons.
88. **QT-088** [Tx] [W] Table: Status, Watch-only, Date, Type, Address/Label, Amount (unit); amount in brackets when not counted toward the balance; colour scheme; multi-select with a "Selected amount" sum.
89. **QT-089** [Tx] [W] Filters: watch-only, date (All/Today/This week/This month/Last month/This year/Range with an exclusive end date), type (17 entries, CoinJoin entries hidden when CoinJoin is off), search (address/txid/label), minimum amount; date and type filters persisted.
90. **QT-090** [Tx] [W] Context menu: copy address/label/amount/txid/raw transaction/full details, Show details, Abandon, Resend, Unlock dust UTXO, Edit address label, Show address QR, third-party explorer links.
91. **QT-091** [Tx] [W] Abandon transaction (unconfirmed, not in mempool, not InstantSend-locked) and Resend transaction (depth 0, not abandoned, not InstantSend-locked).
92. **QT-092** [Tx] [W] Transaction details view with every field in §4.6 (status strings, from/to, credit/debit/fee/net, message, comment, txid, output index, size, OP_RETURN payload, maturity note).
93. **QT-093** [Tx] [W] CSV export of the filtered view with the exact columns Confirmed, (Watch-only), Date (ISO), Type, Label, Address, Amount (unit), ID; every field quoted.
94. **QT-094** [Tx] [U] Third-party transaction URL setting (`|`-separated, `%s` = txid), shown as "Show in <host>" menu entries.
95. **QT-095** [Addr] [W] Sending address book: New, Edit, Delete, Copy, Show QR, Export CSV (Label, Address), wildcard search.
96. **QT-096** [Addr] [W] Receiving address book: edit label only; no delete; Copy, QR, Export.
97. **QT-097** [Addr] [W] Address book selection mode (send picker, sign/verify pickers) with Choose and double-click.
98. **QT-098** [Addr] [W] Purpose handling (send/receive/unknown/other) and duplicate-address error messages matching §7.
99. **QT-099** [Sign] [W] Sign message: P2PKH address, message, base64 compact signature using magic "DarkCoin Signed Message:\n"; requires full unlock; exact error strings.
100. **QT-100** [Sign] [U] Verify message: address, message, signature; results Verified / bad base64 / digest mismatch / verification failed; no wallet needed.
101. **QT-101** [WalletLC] [W] Multiwallet: open any wallet in the wallet directory, close, close all; load-on-startup list maintained by open/close.
102. **QT-102** [WalletLC] [W] Create Wallet dialog: name, Encrypt (default on), Disable Private Keys, Make Blank Wallet, Descriptor Wallet (default on), External signer, with dash-qt's interlocks.
103. **QT-103** [WalletLC] [W] After create: mnemonic display (masked, show/hide) followed by verification of 3 random words; cancelling keeps the wallet but warns.
104. **QT-104** [WalletLC] [W] Restore from mnemonic (12/15/18/21/24 English words) with optional passphrase, compatible with Core's seed derivation (no NFKD, salt cut at 256 bytes) and accepting phrases that only pass Core's weak checksum (with a warning). Not in dash-qt GUI but required for our app.
105. **QT-105** [WalletLC] [S] Restore scan of m/44'/c'/0'/0, m/44'/c'/0'/1 and m/9'/c'/4'/0'/0 with a lookahead of at least 1000, extended on use.
106. **QT-106** [WalletLC] [W] Restore from a dash-qt backup file: SQLite descriptor and legacy BDB `wallet.dat` (detect format; decrypt with passphrase).
107. **QT-107** [WalletLC] [W] Import a dash-qt `dumpwallet` file: rebuild HD from the mnemonic/seed/xprv header; import WIF keys with labels and reserve/change markers; import P2SH scripts; birthday from the earliest key.
108. **QT-108** [WalletLC] [W] Import from raw `hdseed` hex, xprv, or `listdescriptors true` output.
109. **QT-109** [WalletLC] [W] Export for dash-qt: mnemonic+passphrase (with `upgradetohd` instructions), dumpwallet-format file, importdescriptors JSON; stretch goal: SQLite descriptor `wallet.dat`.
110. **QT-110** [WalletLC] [W] Backup Wallet to a file chosen by the user (`.dat`, either format).
111. **QT-111** [WalletLC] [W] Encrypt wallet (warning text, confirmation, keeps the same seed), change passphrase, unlock, lock; passphrases up to 1024 characters; caps-lock warning; show-passphrase toggle.
112. **QT-112** [WalletLC] [W] Unlock for mixing only: a distinct wallet state that allows CoinJoin but not sending, re-prompts for full unlock when sending, and returns to mixing-only afterwards.
113. **QT-113** [WalletLC] [W] Show Recovery Phrase (behind full unlock), with "No Recovery Phrase" and non-HD cases; decide whether to show the mnemonic passphrase (dash-qt does not).
114. **QT-114** [WalletLC] [W] Watch-only / no-private-key wallets, and blank wallets; HD upgrade of a blank or non-HD wallet (`upgradetohd` equivalent).
115. **QT-115** [WalletLC] [W] Migrate a legacy wallet to descriptor (when importing legacy files), preserving the mnemonic and creating `.legacy.bak`.
116. **QT-116** [WalletLC] [W] Automatic wallet backups (legacy-style, rotating 10, `<name>.YYYY-MM-DD-HH-MM`) and a "Show Automatic Backups" action; or document why our format makes them unnecessary.
117. **QT-117** [WalletLC] [S] Rescan from wallet birthday, and full rescan, with progress and cancel.
118. **QT-118** [MN] [S] Masternodes tab (opt-in setting): list browsable without a wallet; type filter All/Regular/Evo/Shared; text filter; Owned; Hide banned; filters persisted; node count.
119. **QT-119** [MN] [F] List columns: status (active/banned for duration), Service, Type, PoSe Score, Registered, Last Paid, Next Payment, Operator Reward.
120. **QT-120** [MN] [S] Owned detection (collateral, owner, voting, payout, operator payout, shared owner/refund keys).
121. **QT-121** [MN] [U] Context menu: Copy ProTx Hash, Copy Collateral Outpoint, Filter by Collateral/Payout/Owner/Voting address, plus the actions below.
122. **QT-122** [MN] [F] Details dialog with every field in §10.1, including shares, early period, Platform addresses and PoSe heights.
123. **QT-123** [MN] [S] Register Masternode/EvoNode wizard: type, collateral (fund new / existing UTXO / external with message signing), service addresses, owner/voting addresses, operator BLS key (generate or paste, basic scheme), payout and operator reward, Platform fields for Evo (v24 address lists or pre-v24 ports), fee source, review, send countdown.
124. **QT-124** [MN] [W] Operator secret save gate: show the secret and `masternodeblsprivkey=` line, require typing the last 4 characters; never persist the secret.
125. **QT-125** [MN] [S] Update Service (revives PoSe-banned nodes), Update Registrar (owner key), Revoke (with reason), each requiring the operator secret or owner key as in dash-qt.
126. **QT-126** [MN] [S] Shared masternode creation (2–8 shares, ≥100 DASH each, early period and penalty) with the clipboard/file JSON envelope protocol, fingerprints, session codes and safety refusals; envelopes interoperable with dash-qt.
127. **QT-127** [MN] [S] Shared masternode maintenance: change reward address, rotate keys (multi-party), dissolve now / together, standby dissolution files, paste routing.
128. **QT-128** [Gov] [F] Governance tab (opt-in setting; disabled when the node has governance disabled): Active / My Proposals; title filter; info tooltip; voting deadline label; Proposal Count.
129. **QT-129** [Gov] [F] Proposal columns: status, Title, Amount, Start, End, Votes (Y/N/A plus margin), My Votes (weighted), Hash; status logic Funded/Lapsed/Confirming/Pending/Passing/Failing/Voting/Unfunded with tooltips.
130. **QT-130** [Gov] [U] Proposal context menu: Copy Raw JSON, Open URL (http/https only, with external-link warning), Vote Yes/No/Abstain; double-click shows details.
131. **QT-131** [Gov] [S] Vote dialog: outcome, choose which masternodes (by voting key) to vote with, weight summary, current votes; signs and relays funding votes; results summary; respects the 1-hour update limit.
132. **QT-132** [Gov] [S] Create Proposal wizard: name (≤40, `[-_a-z0-9]`), URL, payment date (12 superblocks), payments 1–12, address, amount, total; View JSON/Payload; 1 DASH OP_RETURN collateral transaction with a non-refundable warning; proposal stored in the wallet.
133. **QT-133** [Gov] [S] Resume Proposals: list pending wallet proposals, poll confirmations, Broadcast at ≥1 confirmation (`gobject submit`).
134. **QT-134** [Gov] [F] Governance info panel: cycles, last/next superblock, voting cutoff, MN/EvoNode participation, passing threshold, MNs/votes controlled, proposal counts, budget allocated, donut chart.
135. **QT-135** [Options] [U] Options dialog with tabs Main, Wallet, CoinJoin, Network, Display, Appearance; OK applies, Cancel reverts live previews; "overridden by command line" notice; restart-required notices.
136. **QT-136** [Options] [U] Main tab: start on login, show tray icon, minimize to tray, minimize on close (plus node-only prune/dbcache/par/RPC server, not applicable to SPV).
137. **QT-137** [Options] [W] Wallet tab: subtract fee by default, coin control, PSBT controls, keep change address, spend unconfirmed change, enable CoinJoin, dust protection and threshold, external signer path.
138. **QT-138** [Options] [S] Network tab: SOCKS5 proxy IP and port (numeric IP only, validated), separate Tor onion proxy, read-only "reachable via IPv4/IPv6/Tor" indicators; UPnP/NAT-PMP and allow-incoming where applicable.
139. **QT-139** [Options] [U] Display tab: language (21 locales plus system default), Show Masternodes Tab, Show Governance Tab, Show governance clock, unit, decimal digits 2–8, third-party transaction URLs.
140. **QT-140** [Options] [U] Appearance: themes Light (default) / Dark / Traditional with live preview; font family Montserrat / SystemDefault; font scale; normal and bold weight sliders limited to weights the font supports; Overview amount font choice; first-run "Appearance Setup" dialog.
141. **QT-141** [Options] [U] Reset Options with confirmation, backup of the old settings, reset to defaults, and quit.
142. **QT-142** [Options] [U] Import existing dash-qt preferences (QSettings + settings.json keys in §17) on first run (optional, recommended).
143. **QT-143** [Tools] [S] Information tab: client version, user agent, data directory, startup time, network name, connections (in/out), local addresses, block height/time/hash.
144. **QT-144** [Tools] [F] Information: mempool count and usage; Network sub-tab: Credit Pool, InstantSend counters, MN/EvoNode counts, ChainLock, quorum health; Governance sub-tab.
145. **QT-145** [Tools] [F] RPC console with wallet selector, history, sensitive-command redaction, tab completion, nested-call and `[index]` syntax, `help-console`, font size, clear, anti-scam warning; for SPV, a local command console or remote RPC client.
146. **QT-146** [Tools] [S] Network traffic graph: ranges 5m–24h (default 30m), Reset, received/sent kB/s and totals.
147. **QT-147** [Tools] [S] Peers table (Peer, Age, Address, Direction, Type, Network, Ping, Sent, Received, User Agent) with detail panel; Disconnect; Ban 1h/1d/1w/1y; banned list with Unban; copy address.
148. **QT-148** [Tools] [S] Repair tab: Rescan / Rescan (full); Rebuild Index equivalent ("reset chain data / resync").
149. **QT-149** [URI] [U] `dash:` URI parsing identical to Core (label/message/amount in DASH, IS ignored, `req-*` unknown rejects the URI, other unknown keys ignored, trailing `/` trimmed); reject `dash://`; BIP70 `r=` gives the "no longer supported" warning.
150. **QT-150** [URI] [U] Register the `dash:` URI scheme with the OS; Open URI dialog; URIs always open the regular Send page.
151. **QT-151** [I18n] [U] Translations for the same 21 locales (ar, bg, de, en, es, fi, fr, it, ja, ko, nl, pl, pt, ro, ru, sk, th, tr, vi, zh_CN, zh_TW); language selection with system default.
152. **QT-152** [Units] [U] Units DASH/mDASH/μDASH/duffs (8/5/2/0 decimals), thin-space grouping, "." decimal, 18-digit parse limit, display unit persisted.
153. **QT-153** [Help] [U] CoinJoin information explainer and command-line/help dialog equivalent; About dialog with version and licence.
154. **QT-154** [Fmt] [W] Wallet encryption compatible with Core when writing Core-format files (master key with SHA-512 key derivation, AES-256-CBC, Hash(pubkey) IVs), if we export `wallet.dat`.

---

*Generated 2026-10-05 from Dash Core develop @ 3789ec0c719e (≈ v24.0.0-rc.2). Re-verify against the v24.0.0 final tag before using as a release gate.*
