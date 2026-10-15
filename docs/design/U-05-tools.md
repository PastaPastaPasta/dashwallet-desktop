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
| Verify: address, message, signature, Verify, Clear All (an edit clears a result, D38) | `verifyMessage` (its outcome and dash-qt text) | QT-100 |

### PSBT (QT-076…079)

| Row | Binds to | Parity |
|---|---|---|
| Load from file… (binary or base64, under 100 MiB) | `open_file(psbt)` (D2) → `analyzePsbt` | QT-078 |
| Paste a PSBT (base64) | a text field → `analyzePsbt` (D16) | QT-078 |
| Summary lines (" * Sends …", own address, fee, total, unsigned inputs) and the status line | `analyzePsbt(walletId, psbt)` | QT-079 |
| Sign (a spend grant covering what leaves the wallet) | `authorize(spend total)`, `signPsbt` (D17) | QT-079 |
| Broadcast (once per payment this session, D40) | `broadcastPsbt`; `analyzePsbt`'s `unsignedTxid` | QT-079 |
| Copy, Save… (binary `.psbt`, owner-only) | clipboard, `save_file_as(psbt)` (D2) | QT-077, QT-079 |
| Pay ▸ confirm: Create Unsigned (checks it against the sheet, copies the PSBT, opens it here) | `draftCreateUnsigned`, `analyzePsbt` (D18, D37) | QT-076, QT-077 |
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
| D1 | Tools stays one route with U-01's eight tabs; each tab is its own component, mounted only while it is the current tab, keyed by the window's wallet (U-03 D24), so a switch drops the console's lines, a loaded PSBT and a selection. The console (lines, history, font size) and Coin Selection (view and selection) survive a tab change within the wallet: the page holds one model of each per window wallet (D8). A switch also cancels a grant request still waiting (D27). | U-01 built the tabs, the menus and their shortcuts; one page per tool keeps the information architecture of R11. A loaded PSBT or a coin selection of one wallet must not carry over to the next. |
| D2 | **One native file chooser for the app, owned here** (U-04 D19 waits on it). dw-app's `api/files.rs` shows the OS dialog through `tauri-plugin-dialog`'s Rust API only: the plugin is installed but no capability grants its commands, so the renderer cannot open a dialog or name a path by itself. Three commands: `open_file(purpose)` (dialog, then reads the chosen file, capped per purpose: a PSBT under 100 MiB, dash-qt's limit; returns its name and bytes in base64, or `null` when cancelled); `save_file_as(purpose, suggestedName, data)` (dialog with the OS's own overwrite question, then writes the bytes owner-only, 0600 on Unix; returns the path or `null`); `choose_file_path(purpose, save, suggestedName)` (the dialog alone, returning the path, for files the engine writes or reads itself: U-04's backups, exports and imports adopt it when they rebase). Purposes name the dialog's title and filters: `psbt`, `csv`, `walletBackup`, `walletFile`, `coreExport`. The renderer's `app/files.ts` is the one place screens call. | Two tasks need it; one helper with the purposes of both. Keeping the dialog in Rust means a path only ever comes from the user's own choice in an OS dialog, never from renderer code: a compromised page cannot read or write files silently. The overwrite prompt is the OS's, as dash-qt's `getSaveFileName`. |
| D3 | Every tab's errors show copy chosen by code (dash-qt's strings where it has one), never the engine's `message`; an unknown code shows "Something went wrong (`code`)." through U-02's `failureText` (U-02 D16). The console's RPC errors are dw-console's own reply text, as dash-qt prints them. | One rule for every screen; the engine's messages are diagnostics. |
| D4 | Information reads `nodeInfo` and `warnings` when the tab shows and again whenever the sync status is read anew (every `sync` event refreshes it); Peers reads `peers()` the same way. A value only a full node has (mempool) shows "—" with "Requires full-node data source" (DESIGN-opus §1.14); an unknown one before sync shows "—". The banner shows the most severe warning (`warnings()` is ordered) with copy per `WarningCodeName`. | QT-143's rows where an SPV wallet knows them; the Swift `InformationViewModel` rules. |
| D5 | The console never touches a shell: a line goes to `consoleExecute`, which dw-console parses (dash-qt's grammar: nested calls, `[key]`, quoting) and answers from the engine. Commands only a full node answers say "Not available in SPV mode." The input field is cleared after each submit. Output text is never interpreted as HTML. | The brief, and dash-qt's console semantics without an RPC server. |
| D6 | Echo and history keep only the redacted line: on success `ConsoleOutput.filtered`; on a failure the line goes through `consoleRedact` first, and if that fails too (the line does not parse) nothing is echoed or kept and the console says "Error: Invalid command line". History keeps 50 lines; ↑/↓ browse it; Tab completes a command name. | dash-qt's `RPCConsole::historyAdd` with its sensitive-command filter: `walletpassphrase "secret"` must never be echoed, kept or shown again. |
| D7 | `console.authorization_required` names a purpose and a wallet (`ErrorData.authorize`). The console asks for the passphrase with U-02's `PassphrasePrompt` and lockout policy (`LockModel.attempt`) when the vault is encrypted, then `authorize`s that purpose and runs the same line again with the grant. An unencrypted vault authorizes with the `unencrypted` credential without a prompt. A grant the line did not spend (it failed first) is revoked. The line waits in the model only until it runs again or the prompt is cancelled. | Swift `ConsoleViewModel` (`QT145_authorizationRequiredAsksAndRunsTheLineAgain`, `…unencryptedVaultAuthorizesWithoutAPrompt`); U-02 D12's one policy for every passphrase. |
| D8 | View preferences of the tools (console font size, Coin Selection's mode, sort and CoinJoin toggle) last while the window shows the wallet: they live in the page's models, not in a file. | U-04's settings file is not merged; adding fields to it from here would collide with its D24 ordering. Follow-up: move them into `settings.json` once U-04 lands. |
| D9 | Peers lists what `peers()` reports and offers Change peers. Disconnect, ban, unban and the banned list are not built: the engine has no call for them, and CLAUDE.md's product scope leaves node administration and peer tools beyond the console to the node. The tab says so in one line. QT-147 stays partial. | A button that does nothing must not be shown (ROADMAP §0); the scope rule says it is not ours to build. |
| D10 | Rescan: three starts (the wallet's birth, genesis, a typed height: a whole number from 0, else refused before any call). While one runs, the tab polls `rescanProgress` every second and on every `sync` event, shows "Rescanning… current / target" with a bar, and offers Cancel. `rescan_in_progress` says dash-qt's "Wallet is currently rescanning. Abort existing rescan or wait.". | QT-117's three entry points; the engine reports progress by polling (no event carries it). |
| D11 | Reset chain data asks first (it deletes headers and filters, keeps wallets, vault and app data), then stops SPV, resets, and starts SPV again; a failure of the stop (the engine can stop SPV and then report a Platform drain) or of the reset still starts SPV. | The engine refuses the reset while SPV runs (`spv_running`); Swift `QT148_resetChainDataStopsSPVFirstAndStartsItAgain`. |
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
| D25 | Load flakes on agentbox (load 30–60) are rerun alone before anything is changed: the WebKit PSBT flow (QT078_QT079) timed out once in a full run and passed 3 of 3 alone; WebKit QT117 timed out once at load 249 and passed 3 of 3 alone. T1 timed out once at load 136 waiting for the funding (SPV sync held at 75 % before any Tools step) and passed alone with the same binary. The first gate on c4862e6f failed 7 WebKit tests outside Tools (onboarding axe, Pay's vault wait, Activity) at load 136–183 with 30 s timeouts; at load ~55 they passed 12 of 12 on this branch and on main c5860490 side by side. The axe check runs one test per tab, so eleven scans no longer share one 30 s budget. The fix-round gate on 916be922 (load 170) lost the fill right after the PSBT tab remounted once, in WebKit D40; it passed 8 of 8 alone at load 150, so D40 fills until Load is enabled. | The brief's load rule. |
| D26 | After Sign Tx the engine's analysis says `complete` but still counts the input as unsigned: `sign_psbt` adds signatures without finalizing, and dw-psbt counts inputs without a final scriptSig (dash-qt's `PSBTInputSigned`). dash-qt's `fillPSBT` finalizes, so its dialog then shows no "unsigned inputs" line. The renderer leaves that line out when the status is `complete`. | Found by T1 on the real engine; the engine change is a follow-up, not U-05's. |
| D27 | One grant request at a time, claimed before `obtain`'s first await. The sheet cannot be closed while the passphrase is checked; a grant that arrives for a request no longer waiting (cancelled, or the tab or wallet switched, which cancels it) is revoked and never handed to the next request. A cancel before the prompt shows ends the request, revoking a grant the vault's own credential brought meanwhile. | Opus review r1 H1, M4, M5: a cancelled 10 DASH PSBT grant could have answered the next request. |
| D28 | Console Run and Sign Message are busy from their first moment (before the redact or the address check), so a second Enter or click cannot run a line or sign twice. | Opus review r1 H2, M6: `sendtoaddress` twice on an unencrypted vault. |
| D29 | dw-app reduces a suggested file name to a bare name (no directory part, no control characters, 200 characters) before the dialog sees it. | Opus review r1 L16: the page must not choose where the dialog opens. |
| D30 | PSBT: an unknown broadcast is a neutral note; every PSBT broadcast this session is remembered in one log that outlives the tab and the model (`SESSION_BROADCASTS`, by the payment since D40), so loading it again, from any tab or wallet, offers no second broadcast. When the signed PSBT cannot be read back, the signatures are kept and the old analysis dropped: Copy and Save work, Sign and Broadcast do not act on stale lines. | Opus review r1 L12, L13; r2 L3 (the map was per model). |
| D31 | One file dialog at a time: while one is open, dw-app refuses another (`file_dialog`, "a file dialog is already open") instead of stacking a second over it. A row menu (Address Book, Coin Selection) closes on Escape and gives focus back to its button. | Opus review r1 L16, L20. |
| D32 | T1's Repair step does not require the progress card: 103 regtest blocks rescan before the screen's first one-second poll. T1 waits for the rescan to start (the buttons disable or the card shows), then for it to end without a problem, notes whether the card showed, and checks that Pay's balance afterwards is the funding less the payment and its fee. That check shows the rescan left the wallet's coins as they were; it does not show the rescan found anything the wallet lacked. The progress card and Cancel are covered on the fixture (QT117, QT148). | Found by T1 after review r1 asked for a running rescan; r2 L4 (the end could be read before the start). |
| D33 | A tool action takes `grants.epoch` when it begins and passes it to `obtain`/`withGrant` as `since`. A cancel after that (a tab or wallet switch, D27) ends a grant request the action makes later: Console's line after `consoleRedact`/`consoleExecute`, Sign Message's after `classifyAddress`, Repair's Resolve after `feePolicy`. No prompt for the old wallet's command shows on the new tab, and an unencrypted vault issues no grant for it. A cancel while the vault's own credential is refused (`vault.locked` …) does not fall through to the prompt. Reset chain data shows its first failure; a restart that fails after it does not replace it. | Opus review r2 M1, M2, L5. |
| D34 | Pay's confirm sheet shows neither the prompt nor Send until the wallet list says whether the wallet can sign; when the list cannot be read or does not hold the wallet, a note says it cannot send now and Create Unsigned still works. The one file dialog at a time rule (D31) frees the dialog when the request that opened it goes away. | Opus review r1 (watch-only while loading); r2 L6, L7. |
| D35 | Follow-merge of U-04 (main dcc2a62a): Tools amounts follow U-04 D4's display unit. The shared coin table and summary (Pay's coin control and Coin Selection) take U-04's `Display`; PSBT lines and Coin Selection's copied amount use the display unit, as dash-qt's `BitcoinUnits` calls do. U-04 and U-05 both bound `nodeInfo`, so it is bound once and the fixture answers it once (Tools' handler, with U-04's data directory and user agent, so About and General read what they did). | The merge; U-04 D4. |
| D36 | Follow-merge of main 43992da6 (DP3-01) merged without conflicts, so no full re-gate: dw-app (the only crate both sides touch) passed fmt, clippy and its tests with and without `selftest`, including `bindings_are_current`; tsc and vitest (609) passed; the Playwright specs the shared fixture handlers and lock report feed (tools, lockreport, wallet, contacts) passed 250 of 250 on Chromium and WebKit. The full gate passed on 12226231, just before this merge. Tools ▸ Information's two baselines were regenerated: only the user agent and data directory changed (D35). | The manager's rule for clean follow-merges. |
| D37 | Create Unsigned checks the PSBT against the confirm sheet before it copies or opens it: `analyzePsbt` on the result, then one output paying the recipient the sheet's amount, the change the sheet showed (none, or one output of that amount, to the chosen change address or else the wallet's own), the sheet's fee, and as many inputs as the estimate (the payment is matched first, so change sent back to the recipient's own address is still checked). Anything else, or a PSBT that cannot be analysed, copies nothing, opens nothing, and says "The payment changed. Review it again.", as Send's `keepsPromise` refuses a changed plan. Inputs are compared by count: the analysis lists no outpoints, and with the outputs and fee fixed the inputs' total is fixed too; chosen coins are exact in the engine (U-03 D7). | Sol review r1 finding 1: the engine plans again in `create_unsigned` (a coin spent or a fee estimate moved while the sheet was open). Opus check of the fix round L3 (change to the recipient's address). |
| D38 | Sign/Verify results belong to the inputs they were made from. Editing the address (typed or chosen) or message clears the signature and its line; editing Verify's address, message or signature clears its outcome. Each sign and verify, and each edit, bumps a counter: a verify reply carrying an older number is dropped, and inputs edited while the passphrase sheet is open are not signed (the grant is revoked), and a signature for inputs edited during the engine call is not shown; "The address or message changed while signing. Sign the message again." says why. | Sol review r1 finding 2; Opus check of the fix round L1 (no signature for an abandoned message). |
| D39 | The address book's wildcard filter is a greedy matcher that backs up only to the last star (runs of stars collapse), not a regular expression: each character is case-folded alone, as the `i` flag did; its work stays within pattern × text, so a query of many stars against a long label cannot stall the renderer. | Sol review r1 finding 4: `.*` per star backtracked for seconds. Opus check of the fix round L2 (case folding). |
| D40 | PSBT broadcasts are keyed by the payment, not the base64: `analyzePsbt` returns `unsignedTxid`, the engine's txid of the unsigned transaction (dw-psbt `unsigned_txid`), which signatures and other PSBT fields (proprietary or unknown keys, xpubs, input data) do not change. Dash's txid covers the scriptSigs, so the finalized txid would differ per signature set; one unsigned transaction is one payment whatever signs it, and a second broadcast of it can only mislead. `SESSION_BROADCASTS` is a log (a `Model`) of pending, sent (with its txid) and unknown broadcasts: a broadcast is pending before its request goes out, a refusal removes it, and every PSBT tab subscribes, so a tab opened mid-flight, or the payment loaded again in any encoding, offers no broadcast and says "This transaction is being broadcast already." or "This transaction was broadcast already. Transaction ID: %1". The log is renderer memory: a webview reload starts a new one (the app offers no reload; settings and secrets stay out of webview storage, U-02, U-04 D2). | Sol review r1 finding 3; supersedes D30's map keyed by base64. |
| D41 | Windows today: `save_file_as` sets no ACL (the 0600 mode is `cfg(unix)`), so the saved PSBT or CSV inherits the chosen folder's DACL: in the user's profile folders that is the user, SYSTEM and Administrators; in a shared folder, whoever that folder admits. Replacing a file swaps in the new file's inherited DACL, so a narrower ACL the user had set on the old one is lost. Not owner-only, so a follow-up: create the file with a protected DACL of the owner alone (`windows-sys` `Win32_Security`, as dw-desktop already links) and a Windows CI check. dash-qt writes these files with the default permissions too. A PSBT carries no private key; it names addresses, amounts and key paths. | Sol review r1 note on the file chooser. |

## Tests by parity ID

Unit tests lead with the parity row they cover (`src/tools/*.test.ts`); Playwright tests in
`e2e/tools.spec.ts` lead with theirs and run on Chromium and WebKit; T1 is `e2e-tauri/tools.ts`.

| Row | View-model tests (vitest) | Playwright (fixture) | T1 (dashd regtest) |
|---|---|---|---|
| QT-143, QT-144, QT-040 | `information.test.ts` QT143_*, QT144_*, QT040_* | QT143_QT040 | Information's height = dashd's |
| QT-145 | `console.test.ts` QT145_* (13, with D6 redaction, D7 grants) | QT145, QT145_D6, QT145_D7 | `getblockcount` = dashd's height; `getblock` not available |
| QT-147, IOS-023 | `information.test.ts` QT147_*, IOS023_* | QT147_IOS023 | dashd's address is the one peer listed (the engine reports no user agent; follow-up) |
| QT-117, QT-148, IOS-034, IOS-113, E0-04 C9 | `repair.test.ts` QT117_*, QT148_*, IOS034_*, IOS113_* (12) | QT117, QT148, IOS034, IOS113_C9 | rescan from genesis runs to its end |
| QT-095…098 | `addressBook.test.ts` QT095_*…QT098_*, D39_* (12) | QT095_QT098, QT095_QT096, QT097 | New sending entry; Pay's picker fills it |
| QT-099, QT-100 | `signVerify.test.ts` QT099_*, QT100_*, D38_* (13) | QT099_QT100_QT097, D38 | dashd's `verifymessage` accepts the app's signature |
| QT-076…079 | `psbt.test.ts` QT077_*, QT078_*, QT079_*, D40_* (19); `pay/model.test.ts` D37 (2); dw-engine `psbt_analysis_names_the_unsigned_transaction` | QT078_QT079, D40, QT078_QT077_D2, QT076_QT077_D18, QT076_D37 (2), QT076_D18 | Create Unsigned → Sign Tx → Broadcast Tx; dashd's mempool holds it |
| QT-068…074 | `coins.test.ts` QT069_*…QT074_* (13) | QT069_QT071_QT072, QT070, QT068_D19 | lock, unlock, select, Pay with these coins |
| D2 file chooser | `app/files.test.ts` (4); dw-app `api/files.rs` (3) | QT078_QT077_D2, QT095_QT096 (CSV) | not driven (D24) |
| axe, light and dark | — | every tab, the editor, the repair question, the grant sheet | — |
| Visual | — | `e2e/visual.spec.ts` tools-* (8 tabs × 2 themes) | screenshots in `$DW_E2E_OUT` |

## T1 evidence

`scripts/e2e-tauri-tools.sh` on agentbox (2026-10-15, display :57, dashd 23.1.0 regtest), 11 steps,
PASS at 9c27eb71 (after the follow-merge of main dcc2a62a, U-04):

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
9. Sign Tx (T1 checks that the spend grant asked for the passphrase) → no unsigned-inputs line →
   Broadcast Tx: the engine answers `psbt.broadcast_unknown`, which shows as a note (D30), Broadcast
   is not offered again, and dashd's mempool holds the transaction paying 1.25 DASH to the book's
   address; one block mines it.
10. Repair: a rescan from the first block starts (its progress card showed: "Rescanning…"), ends
    without a problem (D32), and Pay then says Available: 8.74999774, 10 DASH less 1.25 DASH and
    the 226-duff fee dashd's `getmempoolentry` reported.

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
| Engine: `analyzePsbt` lists the inputs' outpoints | Create Unsigned compares the inputs themselves with the plan, not their count (D37) |
| dw-app: owner-only DACL for `save_file_as` on Windows, with a Windows CI check (D41) | saved PSBTs and CSVs are owner-only on Windows as on Unix |
