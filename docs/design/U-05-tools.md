# U-05: Tools (Information, Console, Peers, Repair), PSBT, Sign/Verify, address book, Coin Selection (Tauri)

ROADMAP U-05 for the Tauri renderer. Tools is one route (`tools`) with the eight tabs U-01 put on it
(WAI-ARIA tabs, `ui.toolsTab`, the Tools menu and its shortcuts): **Information · Console · Peers ·
Repair · Address Book · Sign/Verify · PSBT · Coin Selection**. They are dash-qt's technical views
(UX-SPEC §4.10, §4.11, §4.14–§4.16), restyled, not redesigned; DASHPAY R11 puts them under Tools. As
in U-02…U-04 every engine call goes through dw-app's generated commands (E0-13), and a row the engine
cannot serve yet shows as **not yet**: it names the task that brings it (ROADMAP §0) and is never
stubbed. U-05 adds three app commands, the shared native file chooser (D2), and no engine call.

| Part | Where |
|---|---|
| View models (no React) | `apps/desktop/src/tools/{information,console,peers,repair,addressBook,signVerify,psbt,coins}.ts` |
| Their tests (by parity ID) | `apps/desktop/src/tools/*.test.ts` |
| Screens | `src/screens/tools/{ToolsPage,Information,Console,Peers,Repair,AddressBook,SignVerify,Psbt,CoinSelection}.tsx`, the shared coin table `src/components/coins.tsx` |
| File chooser (D2) | dw-app `api/files.rs` (`open_file`, `save_file_as`, `choose_file_path`); the renderer's `src/app/files.ts` |
| Fixture handlers | `src/fixture/tools.ts` (node facts, console, peers, rescan, address book, messages, PSBT, coin locks, files) |
| Playwright | `e2e/tools.spec.ts` (flows, axe, light and dark, Chromium and WebKit), `e2e/visual.spec.ts` (baselines) |
| Real app on dashd regtest (T1) | `e2e-tauri/tools.ts`, `scripts/e2e-tauri-tools.sh` (`pnpm e2e:tauri:tools`) |

## Screen inventory

Each row says what it binds to. "not yet (X)" rows show the row with the reason and the owning task
X. The QT and IOS rows keep their status in `docs/parity.md` (D21).

### Information (QT-143, QT-144, QT-040, IOS-023)

| Row | Binds to | Parity |
|---|---|---|
| General: client version, engine version, user agent, data directory, startup time | `launchInfo`, `nodeInfo` | QT-143 |
| Network: name, connections (in / out), local addresses | `nodeInfo.network`, `connectionsIn/Out`, `localAddresses` | QT-143 |
| Block chain: current block height, last block time and hash, best ChainLock | `nodeInfo.tip*`, `bestChainlock` | QT-143 |
| Masternodes, EvoNodes: total (enabled) | `nodeInfo.masternodes/evonodes`; "—" until the list phase finished | QT-144 |
| Memory pool: transactions, usage | `nodeInfo.mempool*`: "—" and "Requires full-node data source" on SPV (D4) | QT-144 |
| The warning banner, most severe first | `warnings()` (D4) | QT-040 |
| DashPay card (loops, last pass, quorum source, Sync now) | **not yet (DP6-03)** | — |
| Network traffic graph | **not yet**: the SPV client exposes no byte counters (QT-146, M6) | QT-146 |

### Console (QT-145)

| Row | Binds to | Parity |
|---|---|---|
| Welcome text and dash-qt's anti-scam warning; Clear keeps them | renderer copy | QT-145 |
| A line runs through dw-console's grammar on the engine; JSON pretty-printed, errors in red | `consoleExecute(walletId, line, grantId)` (D5) | QT-145 |
| Echo and history (50, ↑/↓) hold only the redacted line | `ConsoleOutput.filtered`, `consoleRedact` (D6) | QT-145 |
| Tab completion, `help` | `consoleCommands` | QT-145 |
| Wallet selector (two wallets or more), "Executing command using …" | `listWallets` | QT-145 |
| A command that needs a grant asks for the passphrase and runs again | `console.authorization_required` → `authorize` → `consoleExecute(…, grantId)` (D7) | QT-145 |
| Font size − / + | renderer, this session (D8) | QT-145 |

### Peers (QT-147, IOS-023)

| Row | Binds to | Parity |
|---|---|---|
| Table: address, direction, user agent, version, height, ping, connected for, sent, received; "—" where dash-spv reports nothing; a row opens its details | `peers()` | QT-147 |
| Change peers (drops every connection and dials again) | `rotatePeers` | IOS-023 |
| Disconnect, ban, unban, banned list | **not built**: node administration stays with the node (CLAUDE.md "Product scope"); no facade call (D9) | QT-147 |

### Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9)

| Row | Binds to | Parity |
|---|---|---|
| Rescan from the wallet's birth, from genesis, from a height; progress; Cancel | `rescan`, `rescanProgress`, `cancelRescan` (D10) | QT-117, IOS-113 |
| Wallet birth height | `walletInfo.birthHeight`, `setBirthHeight` | IOS-113 |
| Remove unconfirmed transactions of this wallet (asks first; rescans) | `dropUnconfirmed(walletId)` | IOS-034 |
| Reset chain data and resync (asks first) | `stopSpv`, `resetChainData`, `startSpv` (D11) | QT-148 |
| Resolve stuck funding: send all coins to yourself, per registration waiting on unknown funding | `registrations`, `authorize(spend ≤ maxTxFee)`, `resolveStuckFunding` (D12) | E0-04 C9 |
| Finish transfers (stranded asset locks), Unrecorded asset locks: Send it / Cancel it | **not yet (DP1-09; DP1-02's facade call, E0-04 §16.11)** (D12) | — |

### Address Book (QT-095…098)

| Row | Binds to | Parity |
|---|---|---|
| Sending / Receiving, dash-qt's explanation for each, sorted by label | `addressBook(walletId, purpose, null)` | QT-095, QT-096 |
| Search with `*` and `?` over label and address | renderer (D13) | QT-095 |
| New (sending): address and label, checked for the network | `classifyAddress`, `saveAddressBookEntry(…, replace: false)` | QT-095, QT-098 |
| Edit label (both); sending entries also Delete | `saveAddressBookEntry(…, replace: true)`, `deleteAddressBookEntry` | QT-095, QT-096 |
| Copy address, Copy label, Show QR | `buildPaymentUri`, `qrMatrix` | QT-095, QT-096 |
| Export… (CSV "Label","Address"; the OS dialog asks before it replaces a file, D2) | `save_file_as` (D2, D13) | QT-095 |
| Picker: Pay's address field and Sign's address field choose from it | the same list in selection mode (D14) | QT-097 |

### Sign/Verify (QT-099, QT-100)

| Row | Binds to | Parity |
|---|---|---|
| Sign: address (Choose… from receiving addresses), message, Sign, the signature with Copy, Clear All | `classifyAddress`, `authorize(signMessage)`, `signMessage` (D15) | QT-099 |
| Verify: address, message, signature, Verify, Clear All | `verifyMessage` (its outcome and dash-qt text) | QT-100 |

### PSBT (QT-076…079)

| Row | Binds to | Parity |
|---|---|---|
| Load from file… (binary or base64, under 100 MiB) | `open_file(psbt)` (D2) → `analyzePsbt` | QT-078 |
| Paste a PSBT (base64) | a text field → `analyzePsbt` (D16) | QT-078 |
| Summary lines (" * Sends …", own address, fee, total, unsigned inputs) and the status line | `analyzePsbt(walletId, psbt)` | QT-079 |
| Sign (a spend grant covering what leaves the wallet) | `authorize(spend total)`, `signPsbt` (D17) | QT-079 |
| Broadcast | `broadcastPsbt` | QT-079 |
| Copy, Save… (binary `.psbt`, owner-only) | clipboard, `save_file_as(psbt)` (D2) | QT-077, QT-079 |
| Pay ▸ confirm: Create Unsigned (copies the PSBT, opens it here) | `draftCreateUnsigned` (D18) | QT-076, QT-077 |
| External signer (HWI) | **not yet (M6)** | QT-080 |

### Coin Selection (QT-069…075)

| Row | Binds to | Parity |
|---|---|---|
| The coin table U-03's Pay uses (`components/coins.tsx`), with Date, Label, Mixing rounds | `utxos` (D19) | QT-069 |
| List / Tree (by address, with totals); sort by any column | renderer, this session (D8) | QT-069 |
| Show all coins / Hide CoinJoin coins (hidden by default) | `Utxo.coinjoinDenominated` | QT-071 |
| A row's Actions menu (dash-qt's context menu): Copy address, label, amount, `txid:vout`; Lock / Unlock unspent | clipboard, `lockOutpoints`, `unlockOutpoints` (D22) | QT-070 |
| Toggle lock state (dash-qt's button: every shown coin flips); "(n locked)" | `lockOutpoints`, `unlockOutpoints` | QT-070 |
| Select all; summary (Quantity, Amount, Fee, After fee, Bytes, Change; "≈") | `coinSelectionSummary` (U-03's panel) | QT-072 |
| Pay with the selected coins | Pay ▸ To an address with coin control on "Selected coins" (D19) | QT-068 |
| Spent selected coins are unselected with dash-qt's notice | the refreshed `utxos` | QT-074 |

## Slice plan

1. This document.
2. The shared file chooser: dw-app `open_file`, `save_file_as`, `choose_file_path`, bindings, the
   fixture's handlers, `src/app/files.ts`.
3. View models and their tests: Information, Console, Peers, Repair.
4. View models and their tests: Address Book, Sign/Verify, PSBT, Coin Selection.
5. The fixture's handlers and scenarios for every tab.
6. Screens: Information, Console, Peers, Repair.
7. Screens: Address Book (and its picker in Pay and Sign), Sign/Verify, PSBT (and Create Unsigned in
   Pay's confirm sheet), Coin Selection (the shared coin table).
8. Playwright flows, axe and visual baselines; T1 on regtest; evidence and the test map.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Tools stays one route with U-01's eight tabs; each tab is its own component, mounted only while it is the current tab, keyed by the window's wallet (U-03 D24), so a switch drops the console's lines, a loaded PSBT and a selection. The console's history survives a tab change within the wallet (it lives in the page, D8). | U-01 built the tabs, the menus and their shortcuts; one page per tool keeps the information architecture of R11. A loaded PSBT or a coin selection of one wallet must not carry over to the next. |
| D2 | **One native file chooser for the app, owned here** (U-04 D19 waits on it). dw-app's `api/files.rs` shows the OS dialog through `tauri-plugin-dialog`'s Rust API only: the plugin is installed but no capability grants its commands, so the renderer cannot open a dialog or name a path by itself. Three commands: `open_file(purpose)` (dialog, then reads the chosen file, capped per purpose: a PSBT under 100 MiB, dash-qt's limit; returns its name and bytes in base64, or `null` when cancelled); `save_file_as(purpose, suggestedName, data)` (dialog with the OS's own overwrite question, then writes the bytes owner-only, 0600 on Unix; returns the path or `null`); `choose_file_path(purpose, save, suggestedName)` (the dialog alone, returning the path, for files the engine writes or reads itself: U-04's backups, exports and imports adopt it when they rebase). Purposes name the dialog's title and filters: `psbt`, `csv`, `walletBackup`, `walletFile`, `coreExport`. The renderer's `app/files.ts` is the one place screens call. | Two tasks need it; one helper with the purposes of both. Keeping the dialog in Rust means a path only ever comes from the user's own choice in an OS dialog, never from renderer code: a compromised page cannot read or write files silently. The overwrite prompt is the OS's, as dash-qt's `getSaveFileName`. |
| D3 | Every tab's errors show copy chosen by code (dash-qt's strings where it has one), never the engine's `message` for codes the tab knows; an unknown code shows "Something went wrong (`code`)." (U-02 D16). | One rule for every screen; the engine's messages are diagnostics. |
| D4 | Information reads `nodeInfo` and `warnings` when the tab shows and again whenever the sync status is read anew (every `sync` event refreshes it); Peers reads `peers()` the same way. A value only a full node has (mempool) shows "—" with "Requires full-node data source" (DESIGN-opus §1.14); an unknown one before sync shows "—". The banner shows the most severe warning (`warnings()` is ordered) with copy per `WarningCodeName`. | QT-143's rows where an SPV wallet knows them; the Swift `InformationViewModel` rules. |
| D5 | The console never touches a shell: a line goes to `consoleExecute`, which dw-console parses (dash-qt's grammar: nested calls, `[key]`, quoting) and answers from the engine. Commands only a full node answers say "Not available in SPV mode." The input field is cleared after each submit. Output text is never interpreted as HTML. | The brief, and dash-qt's console semantics without an RPC server. |
| D6 | Echo and history keep only the redacted line: on success `ConsoleOutput.filtered`; on a failure the line goes through `consoleRedact` first, and if that fails too (the line does not parse) nothing is echoed or kept and the console says "Error: Invalid command line". History keeps 50 lines; ↑/↓ browse it; Tab completes a command name. | dash-qt's `RPCConsole::historyAdd` with its sensitive-command filter: `walletpassphrase "secret"` must never be echoed, kept or shown again. |
| D7 | `console.authorization_required` names a purpose and a wallet (`ErrorData.authorize`). The console asks for the passphrase with U-02's `PassphrasePrompt` and lockout policy (`LockModel.attempt`) when the vault is encrypted, then `authorize`s that purpose and runs the same line again with the grant. An unencrypted vault authorizes with the `unencrypted` credential without a prompt. A grant the line did not spend (it failed first) is revoked. The line waits in the model only until it runs again or the prompt is cancelled. | Swift `ConsoleViewModel` (`QT145_authorizationRequiredAsksAndRunsTheLineAgain`, `…unencryptedVaultAuthorizesWithoutAPrompt`); U-02 D12's one policy for every passphrase. |
| D8 | View preferences of the tools (console font size, Coin Selection's mode and sort) last for the session: they live in the page, not in a file. | U-04's settings file is not merged; adding fields to it from here would collide with its D24 ordering. Follow-up: move them into `settings.json` once U-04 lands. |
| D9 | Peers lists what `peers()` reports and offers Change peers. Disconnect, ban, unban and the banned list are not built: the engine has no call for them, and CLAUDE.md's product scope leaves node administration and peer tools beyond the console to the node. The tab says so in one line. QT-147 stays partial. | A button that does nothing must not be shown (ROADMAP §0); the scope rule says it is not ours to build. |
| D10 | Rescan: three starts (the wallet's birth, genesis, a typed height: a whole number from 0, else refused before any call). While one runs, the tab polls `rescanProgress` every second and on every `sync` event, shows "Rescanning… current / target" with a bar, and offers Cancel. `rescan_in_progress` says dash-qt's "Wallet is currently rescanning. Abort existing rescan or wait.". | QT-117's three entry points; the engine reports progress by polling (no event carries it). |
| D11 | Reset chain data asks first (it deletes headers and filters, keeps wallets, vault and app data), then stops SPV, resets, and starts SPV again; a failure after the stop still starts SPV. | The engine refuses the reset while SPV runs (`spv_running`); Swift `QT148_resetChainDataStopsSPVFirstAndStartsItAgain`. |
| D12 | Repair lists the registrations of the wallet whose funding is unknown (`registrations()` with `waiting: fundingUnknown`; step `registration/<draft>/funding`) with E0-04 C9's "Resolve stuck funding: send all coins to yourself". It asks first, then the passphrase (spend grant capped at `feePolicy.maxTxFee`: the self-spend's outflow is its fee, and the engine refuses a fee above that cap), then `resolveStuckFunding`. A refusal shows its own text (`StuckFundingRefusalName`) and, with `retryLater`, says to try again later. When `registrations` fails (no Platform) the section says it cannot read them. "Finish transfers" needs a `PlatformOp` grant whose size only the registration flow knows, and "Unrecorded asset locks" has no facade call yet: both rows say not yet, with DP1-09 and DP1-02. | The brief maps Repair to E0-04.P2b's call. The cap is the tightest the renderer can name without guessing a fee. |
| D13 | Address book search is dash-qt's wildcard filter (`*`, `?`, case-insensitive) on the label or the address, in the renderer over the purpose's entries; the engine's `search` is not used (it is a substring match). Export writes dash-qt's CSV (`"Label","Address"`, quotes doubled) through `save_file_as(csv)`. | dash-qt's `QSortFilterProxyModel` with `setFilterWildcard`; the Swift `WildcardMatcher` tests. |
| D14 | Editing an entry changes its label only; a sending entry's address is shown read-only (delete and add it again to change it). Receiving entries cannot be deleted (dash-qt). The picker (QT-097) is the same list in a sheet: Pay ▸ To an address offers the sending book, Sign offers the receiving book; Choose or a double click fills the field. | dash-qt's edit dialog changes the address of a sending entry by delete-and-add internally; the engine's `replace` keeps the address. |
| D15 | Sign checks the address first (`classifyAddress`): not an address of this network is "The entered address is invalid.", a P2SH address is "The entered address does not refer to a key."; neither asks for a grant. Then a `signMessage` grant: with the vault unlocked (fully), the `unencrypted` credential; locked or unlocked for mixing only, the passphrase under U-02's policy. A grant not redeemed because signing failed is revoked (Swift L2). Engine refusals map to dash-qt's texts (`address_not_mine`: "Private key for the entered address is not available."). The message and signature are not secret (the passphrase is). Verify shows the engine's `VerifyMessageResult.text`, green for `verified`, red otherwise. | QT-099/100 and the Swift `SignVerifyViewModel` tests. |
| D16 | PSBT from the clipboard is a paste field (paste the base64 into it, then Load), not a clipboard read: the webview's clipboard read prompts or fails on WebKitGTK and needs a permission the app does not grant. Copy to Clipboard writes through the webview's clipboard; a refused write says "Could not copy". | dash-qt's "Paste from clipboard" in the place the desktop allows. |
| D17 | Sign asks for a `spend` grant capped at the analysis' `total` (what leaves the wallet, fee included) and signs; with `total` unknown (an input's previous transaction is missing) nothing is asked or signed ("The amount this transaction sends could not be determined."). The credential follows the payment policy as Pay's confirm sheet does (U-03 D3). After signing the PSBT is analysed again; dash-qt's "Signed transaction successfully. Transaction is ready to broadcast." or "Signed %1 inputs, but more signatures are still required.". Broadcast: "Transaction broadcast successfully! Transaction ID: %1", or "Transaction broadcast failed: %1"; `psbt.broadcast_unknown` is not a failure: it says the transaction was handed to the network and its outcome is unknown, without "Transaction broadcast failed:", and does not offer a second broadcast. On regtest (T1) the engine answers so while dashd's mempool already holds the transaction, as U-03's Pay result does. | Swift review H1: the grant covers the outflow including the fee; the engine refuses more (`psbt.grant_exceeded`). |
| D18 | Create Unsigned (QT-076/077): Pay's confirm sheet offers it as a secondary action for every wallet, and instead of Send for a watch-only wallet. It calls `draftCreateUnsigned` on the confirmed draft (no grant, nothing reserved), copies the base64 to the clipboard, and opens Tools ▸ PSBT with it loaded: "The PSBT has been copied to the clipboard. You can also save it." (only when the copy succeeded; a refused copy leaves Copy and Save… to the user). For a watch-only wallet the sheet asks no passphrase and Send is not shown; the engine refuses only the prepare step of a watch-only wallet (`SendFailure::WatchOnly`), so its estimate and review work as for any wallet. dash-qt's "Enable PSBT controls" option is not added. | U-04 D5 left PSBT controls to U-05. A watch-only wallet's only way to pay is a PSBT; one more button in the sheet costs less than an option that hides it. |
| D19 | Coin Selection reuses U-03's coin control, not a copy of it: the coin rows and the summary move from `pay/CoinControl.tsx` into `components/coins.tsx` (`CoinTable`, `CoinSummary`), which Pay's Advanced panel and this tab both render; the selection is the same `txid:vout` keys of `pay/model.ts`. The tab adds dash-qt's technical columns, the tree, locks and copy. "Pay with these coins" opens Pay ▸ To an address with Advanced on "Selected coins" and that selection (`ui.payCoins`), as dash-qt's Coin Selection feeds the send dialog. | The brief: coin control exists; one table, one selection model. |
| D20 | The fixture (U-07) models what the tabs meet: node facts and warnings (scenario `tools-warnings`), a console with dw-console's command table in miniature (`getblockcount`, `getbalance`, `getconnectioncount`, `getnetworkinfo`, `help`, `help-console`, `listwallets`, `uptime`, `validateaddress`, `verifymessage`, `signmessage` needing a `signMessage` grant; `dumpwallet`, `getblock`, `getbestblockhash`, `stop` not available; RPC errors for a wrong arity), peers, a rescan whose progress advances per read, the address book's rules (duplicates, own address, receive not deletable), sign and verify with deterministic signatures, PSBT analysis by a fixed table of fixture PSBTs, coin locks, files (`World.tools.files`, a cancel), and a watch-only wallet (scenario `watch-only`). The fixture's watch-only refusal sits where the engine's does, in `draft_prepare`, not in the estimate. Fixture diagnostics are worded unlike the renderer's copy, so `check-bundle`'s sentinels never find a fixture string in the production bundle. | Playwright drives every state without an engine; the real behaviour is T1's. |
| D21 | `docs/parity.md` is left as it is (U-01 D17, U-03 D20, U-04 D20); the tests below map each row to its TypeScript test by parity ID. | One re-baseline pass for the Tauri column. |
| D22 | Coin Selection and Peers use a wide page (the content column drops its maximum width), Coin Selection's Label and Address titles are short, dates are short, and a coin's copies and its lock are one Actions menu per row, as dash-qt's context menu. Toggle lock state is dash-qt's one button, not Lock all / Unlock all. | dash-qt's columns did not fit the reading column at 1280 px: rows wrapped and the table scrolled sideways. |
| D23 | The Console's input submits on Enter itself (`requestSubmit`): WebKit, the app's webview on Linux and macOS, takes Enter in an input with a datalist for its suggestions and never submits the form. | Found by the WebKit Playwright project; Chromium submits either way. |
| D24 | T1 (`e2e-tauri/tools.ts`) does not drive the OS file dialogs (WebDriver cannot reach a GTK dialog); Playwright drives open, save and cancel on the fixture (QT078_QT077_D2, QT068_D19). Tools is opened in T1 with its menu shortcut through the X server (xdotool), as U-04's T1 opens Settings. | The dialog is the OS's; what the app does with the answer is the same code either way. |
| D25 | Load flakes on agentbox (load 30–60) are rerun alone before anything is changed: the WebKit PSBT flow (QT078_QT079) timed out once in a full run and passed 3 of 3 alone. | The brief's load rule. |
| D26 | After Sign Tx the engine's analysis says `complete` but still counts the input as unsigned: `sign_psbt` adds signatures without finalizing, and dw-psbt counts inputs without a final scriptSig (dash-qt's `PSBTInputSigned`). dash-qt's `fillPSBT` finalizes, so its dialog then shows no "unsigned inputs" line. The renderer leaves that line out when the status is `complete`. | Found by T1 on the real engine; the engine change is a follow-up, not U-05's. |

## Tests by parity ID

Unit tests lead with the parity row they cover (`src/tools/*.test.ts`); Playwright tests in
`e2e/tools.spec.ts` lead with theirs and run on Chromium and WebKit; T1 is `e2e-tauri/tools.ts`.

| Row | View-model tests (vitest) | Playwright (fixture) | T1 (dashd regtest) |
|---|---|---|---|
| QT-143, QT-144, QT-040 | `information.test.ts` QT143_*, QT144_*, QT040_* | QT143_QT040 | Information's height = dashd's |
| QT-145 | `console.test.ts` QT145_* (13, with D6 redaction, D7 grants) | QT145, QT145_D6, QT145_D7 | `getblockcount` = dashd's height; `getblock` not available |
| QT-147, IOS-023 | `information.test.ts` QT147_*, IOS023_* | QT147_IOS023 | dashd listed with its user agent |
| QT-117, QT-148, IOS-034, IOS-113, E0-04 C9 | `repair.test.ts` QT117_*, QT148_*, IOS034_*, IOS113_* (12) | QT117, QT148, IOS034, IOS113_C9 | rescan from genesis runs to its end |
| QT-095…098 | `addressBook.test.ts` QT095_*…QT098_* (10) | QT095_QT098, QT095_QT096, QT097 | New sending entry; Pay's picker fills it |
| QT-099, QT-100 | `signVerify.test.ts` QT099_*, QT100_* (7) | QT099_QT100_QT097 | dashd's `verifymessage` accepts the app's signature |
| QT-076…079 | `psbt.test.ts` QT077_*, QT078_*, QT079_* (16) | QT078_QT079, QT078_QT077_D2, QT076_QT077_D18, QT076_D18 | Create Unsigned → Sign Tx → Broadcast Tx; dashd's mempool holds it |
| QT-068…074 | `coins.test.ts` QT069_*…QT074_* (13) | QT069_QT071_QT072, QT070, QT068_D19 | lock, unlock, select, Pay with these coins |
| D2 file chooser | `app/files.test.ts` (4); dw-app `api/files.rs` (3) | QT078_QT077_D2, QT095_QT096 (CSV) | not driven (D24) |
| axe, light and dark | — | every tab, the editor, the repair question, the grant sheet | — |
| Visual | — | `e2e/visual.spec.ts` tools-* (8 tabs × 2 themes) | screenshots in `$DW_E2E_OUT` |

## T1 evidence

`scripts/e2e-tauri-tools.sh` on agentbox (2026-10-15, display :57, dashd 23.1.0 regtest), 11 steps,
PASS:

1. dashd mines 101 blocks; dwcli makes an encrypted vault and a wallet; dashd pays it 10 DASH, one
   block. The app syncs SPV from dashd alone and is unlocked; Pay counts the 10 DASH.
2. Information: current block height 102, as dashd's.
3. Console: `getblockcount` answers 102; `getblock 1` says "Not available in SPV mode."
4. Peers: dashd, outbound, its only peer (the other columns "—": see the follow-ups).
5. Sign/Verify: the wallet signs with its address; dashd's `verifymessage` returns true; Verify says
   "Message verified.".
6. Address Book: a dashd address saved as a sending entry.
7. Coin Selection: the funding coin `txid:0` locked through its Actions menu, unlocked, selected.
8. Pay with these coins → "Selected coins" → the book's address through Pay's picker, 1.25 DASH →
   Review → Create Unsigned → Tools ▸ PSBT: " * Sends 1.25000000 tDASH to …", the change line, fee,
   total, one unsigned input.
9. Sign Tx (the spend grant asks the passphrase) → no unsigned-inputs line → Broadcast Tx: the
   engine answers `psbt.broadcast_unknown`, Broadcast is not offered again, and dashd's mempool holds
   the transaction paying 1.25 DASH to the book's address; one block mines it.
10. Repair: a rescan from the first block runs to its end.

Screenshots of every step, the app's and dashd's logs and `result.json` go to `$DW_E2E_OUT`.

## Follow-ups to file

| Follow-up | Rows waiting on it |
|---|---|
| U-04 adopts `choose_file_path` (D2) | U-04 D19's save-as and open dialogs, Backups ▸ Show folder stays an opener question |
| Tool view preferences into `settings.json` (D8) | console font size, Coin Selection mode and sort |
| DP6-03 | Information's DashPay card |
| DP1-09, DP1-02 facade call | Repair's Finish transfers and Unrecorded asset locks (D12) |
| M6 | network traffic (QT-146), external signer (QT-080) |
| Engine: peer details from dash-spv (user agent, version, starting height, ping, bytes) | Peers shows "—" in those columns: `peers()` reports address and connection time only (QT-147 stays partial) |
| Engine: `sign_psbt` finalizes complete inputs, as dash-qt's `fillPSBT` (D26) | the renderer's `complete` guard on the unsigned-inputs line can go |
