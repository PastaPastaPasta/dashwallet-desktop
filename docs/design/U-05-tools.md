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
| Remove unconfirmed transactions (asks first; rescans) | `dropUnconfirmed(walletId)` | IOS-034 |
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
| Export… (CSV "Label","Address") | `save_file_as` (D2, D13) | QT-095 |
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
| Lock / Unlock a coin; Lock all, Unlock all; "(n locked)" | `lockOutpoints`, `unlockOutpoints` | QT-070 |
| Copy address, label, amount, `txid:vout` | clipboard | QT-070 |
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
| D4 | Information reads `nodeInfo` and `warnings` when the tab shows and again on every `sync` event (the event bridge invalidates `["tools"]`). A value only a full node has (mempool) shows "—" with "Requires full-node data source" (DESIGN-opus §1.14); an unknown one before sync shows "—". The banner shows the most severe warning (`warnings()` is ordered) with copy per `WarningCodeName`. | QT-143's rows where an SPV wallet knows them; the Swift `InformationViewModel` rules. |
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
| D16 | PSBT from the clipboard is a paste field (paste, then Load), not a clipboard read: the webview's clipboard read prompts or fails on WebKitGTK and needs a permission the app does not grant. | dash-qt's "Paste from clipboard" in the place the desktop allows. |
| D17 | Sign asks for a `spend` grant capped at the analysis' `total` (what leaves the wallet, fee included) and signs; with `total` unknown (an input's previous transaction is missing) nothing is asked or signed ("The amount this transaction sends could not be determined."). The credential follows the payment policy as Pay's confirm sheet does (U-03 D3). After signing the PSBT is analysed again; dash-qt's "Signed transaction successfully. Transaction is ready to broadcast." or "Signed %1 inputs, but more signatures are still required.". Broadcast: "Transaction broadcast successfully! Transaction ID: %1", or "Transaction broadcast failed: %1"; `psbt.broadcast_unknown` says the transaction may have been sent and does not offer a second broadcast. | Swift review H1: the grant covers the outflow including the fee; the engine refuses more (`psbt.grant_exceeded`). |
| D18 | Create Unsigned (QT-076/077): Pay's confirm sheet offers it as a secondary action for every wallet, and instead of Send for a watch-only wallet. It calls `draftCreateUnsigned` on the confirmed draft (no grant, nothing reserved), copies the base64 to the clipboard, and opens Tools ▸ PSBT with it loaded: "The PSBT has been copied to the clipboard. You can also save it." dash-qt's "Enable PSBT controls" option is not added. | U-04 D5 left PSBT controls to U-05. A watch-only wallet's only way to pay is a PSBT; one more button in the sheet costs less than an option that hides it. |
| D19 | Coin Selection reuses U-03's coin control, not a copy of it: the coin rows and the summary move from `pay/CoinControl.tsx` into `components/coins.tsx` (`CoinTable`, `CoinSummary`), which Pay's Advanced panel and this tab both render; the selection is the same `txid:vout` keys of `pay/model.ts`. The tab adds dash-qt's technical columns, the tree, locks and copy. "Pay with these coins" opens Pay ▸ To an address with Advanced on "Selected coins" and that selection (`ui.payCoins`), as dash-qt's Coin Selection feeds the send dialog. | The brief: coin control exists; one table, one selection model. |
| D20 | The fixture (U-07) models what the tabs meet: node facts and warnings, a console with a handful of commands (`getblockcount`, `getbalance`, `help`, `walletpassphrase`, `sendtoaddress` needing a spend grant, `getmempoolinfo` not available, an RPC error), peers, a rescan whose progress advances per read, the address book's rules (duplicates, own address, receive not deletable), sign and verify with deterministic signatures, PSBT analysis by a fixed table of fixture PSBTs, coin locks, and files (`World.files`, a cancel). | Playwright drives every state without an engine; the real behaviour is T1's. |
| D21 | `docs/parity.md` is left as it is (U-01 D17, U-03 D20, U-04 D20); the tests below map each row to its TypeScript test by parity ID. | One re-baseline pass for the Tauri column. |

## Follow-ups to file

| Follow-up | Rows waiting on it |
|---|---|
| U-04 adopts `choose_file_path` (D2) | U-04 D19's save-as and open dialogs, Backups ▸ Show folder stays an opener question |
| Tool view preferences into `settings.json` (D8) | console font size, Coin Selection mode and sort |
| DP6-03 | Information's DashPay card |
| DP1-09, DP1-02 facade call | Repair's Finish transfers and Unrecorded asset locks (D12) |
| M6 | network traffic (QT-146), external signer (QT-080) |
