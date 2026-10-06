# M1 Swift contract (WalletRuntime seams and view models)

Status: **contract**, 2026-10-05. Code: `Sources/WalletRuntime/Contracts/*.swift` (protocols and value types
only). Engine side: [`m1-engine.md`](m1-engine.md). Design: DESIGN-opus §1.6 (targets and imports),
§1.11 (view models), §1.12 (runtime adapters).

## 1. Layering

```
WalletFeatures (view models) ──depends on──▶ WalletRuntime/Contracts (protocols + value types)
                                                     ▲ implemented by
WalletRuntime adapters (WalletHost, LifecycleQueue, SPVCoordinator, …) ──▶ DashKit (EngineClient) ──▶ dw-ffi
```

- WalletFeatures may import Foundation, Observation, WalletRuntime, AppServices, PlatformServices and
  DesignTokens (lint-enforced), **not** DashKit. So the value types the view models see are declared in
  WalletRuntime, Foundation-only. They have the same names as their DashKit counterparts (`Amount`,
  `WalletID`, `DashNetwork`, `WalletBalances`); inside WalletRuntime the module's own types shadow
  DashKit's, so adapter code spells the DashKit ones `DashKit.Amount` etc.
- Every service throws one error type, `ServiceError { code: ServiceErrorCode, detail, recipientIndex,
  retryAfterSeconds, parameters }`. `code.rawValue` is the engine's stable code (m1-engine.md §4). View models
  pick UI copy by code and fill numbers from `parameters`; `detail` is for logs only.
- Unknown values are `nil` (balances, fees, heights, sync status before the first snapshot). View models
  render "unknown", never zero.
- Secrets are `any SecretBuffer` (zeroing class). The adapter's conforming type wraps DashKit
  `SecretBytes`. View models create buffers from text-field input through `VaultProviding.makeSecret(utf8:)`
  and drop the `String` at once.
- Observation: state holders (`WalletStateProviding`, `SyncStatusProviding`, `AuthenticationGating`,
  `SettingsProviding`) are `@MainActor` and expose current values plus an `AsyncStream` of changes; the
  concrete adapters are `@Observable`. Query/command services are `Sendable` with `async throws` methods.
  View models subscribe in a `Task { for await … }` and re-query (DESIGN-opus §1.11).
- Composition: one `AppEnvironment` struct built in each app's `@main` holds the service instances. No
  singletons.

## 2. Protocols

Implementer for every adapter: **C** (Swift runtime), on top of DashKit. "Engine calls" lists what the
adapter calls; owners of those calls are in m1-engine.md.

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `WalletHosting` | Owns `EngineClient` and the active session; `start(network:options:)`, `stop()`, `activeNetwork`, `dataDirectory(for:)`. Only the lifecycle queue calls start/stop. | `Engine(...)`, `open_network`, `close_network`, `shutdown`, `network_dir` | QT-002, IOS-106 |
| `LifecycleQueueing` | Serial queue for start/stop/switch network/import/remove wallet; `transition` + `transitions()` for the overlay. | `start_spv`/`stop_spv`, `import_wallet`, `remove_wallet` | IOS-018, QT-008, QT-101 |
| `WalletStateProviding` | `wallets`, `selectedWalletID`, `balances` (all optional until loaded), `changes()`, `select`, `rename`. | `wallet_infos`, `balances`, `rename_wallet`; events `Balances`, `WalletCreated/Removed` | QT-014, QT-034, IOS-019/021, IOS-110 |
| `SyncStatusProviding` | `status: SyncStatus?` (damped `progress`, `isDone` gate, `isStalled` after 45 s), `changes()`, `peers()`, `rotatePeers()`, `rescan(from:)`. | `sync_snapshot`, `peers`, `rotate_peers`, `rescan`; event `Sync`, notice `SyncStalled` | QT-024/025/027, QT-117, QT-147, IOS-023 |
| `VaultProviding` | `status`, `create(passphrase:)`, `encrypt`, `changePassphrase`, `revealMnemonic`, `generateMnemonic`, `checkMnemonic`, `makeSecret(utf8:)`. | `Vault.*`, `generate_mnemonic`, `check_mnemonic` | QT-102/103, QT-111/113, IOS-002…007, IOS-010 |
| `AuthenticationGating` | The one auth primitive: `lockState`, `lockStateChanges()`, `requirement(for:)`, `authorize(_:credential:)`, `revoke(_:)`, `unlock`, `lock`. | `Vault.authorize`, `revoke_grant`, `unlock`, `lock`; event `LockState` | QT-022, QT-061, IOS-012…017 |
| `TransactionSending` / `TransactionDrafting` | `makeDraft(wallet:)`, `maxSpendable`; draft setters, `estimate`, `prepare(grant:)` (never broadcasts), `broadcast`, `abandon`. `PreparedTransaction.id` names the engine `PreparedTx` the draft holds. | `new_tx_draft`, `TxDraft.*`, `max_spendable` | QT-051…067, IOS-041…052 |
| `HistoryProviding` | `page(wallet:query:)`, `detail`, `setLabel`, `changes(wallet:)` (txids per `HistoryChanged`). | `history_page`, `tx_detail`, `set_tx_label` | QT-086…094, IOS-027…031 |
| `ReceiveProviding` | `currentAddress`, `nextAddress`, `addresses`, `createRequest`, `requests`, `deleteRequest`. | `receive.rs` | QT-081…085, IOS-053…055 |
| `CoinControlProviding` | `utxos`, `lock`, `unlock`, `lockedOutpoints`. | `coins.rs` | QT-068…075 |
| `AddressBookProviding` | `entries(wallet:purpose:search:)`, `save(…replace:)`, `delete`. | `labels.rs` | QT-095…098 |
| `MessageSigning` | `sign(wallet:address:message:grant:)`, `verify(address:message:signature:)`. | `sign_message`, `verify_message` | QT-099/100 |
| `URIHandling` | `parsePaymentURI`, `buildPaymentURI`, `classifyAddress`, `qrMatrix(for:)` — for the active network. | `uri.rs` | QT-054, QT-084/085, QT-149, IOS-042/048 |
| `AmountFormatting` | `format(_:unit:style:)`, `parse(_:unit:)`, `unitName` — for the active network. | `units.rs` | QT-020, QT-036, QT-039, QT-152 |
| `SettingsProviding` | `display: DisplaySettings { unit, decimalDigits, hideBalances }`, `lastNetwork`, `update`, `changes()`; backed by `settings.json` / `global.json`. | — (Swift only) | QT-020, QT-039, QT-135, IOS-020 |

Review findings for C (details in m1-engine.md §5): **M2** stale cached session in `EngineClient.open`,
**M4** `EventBus` drops lifecycle events under load, **L2** release the engine off the main thread,
**L3** `SecretBytes` wipe. All four are fixed in DashKit (generation counter in `EngineClient`, lifecycle
events queued apart from signals, `shutdown()` on the client's actor plus a debug assertion, `memset_s` /
unoptimised wipe of one owned allocation).

### 2.1 Adapters (implemented)

| Protocol | Adapter (`Sources/WalletRuntime`) | Notes |
|---|---|---|
| `WalletHosting` | `Host/WalletHost` (actor) | Owns the engine; `ActiveNetwork` is readable from any thread. |
| `LifecycleQueueing` | `Host/LifecycleQueue` (actor) | Start: host → observers → SPV; stop: reverse; `shutdown()` releases the engine. |
| `WalletStateProviding` | `State/WalletStateModel` → `WalletState` | `wallets` `nil` until `wallet_infos` succeeds (`lastError` otherwise); `Balances` events re-read one wallet. |
| `SyncStatusProviding` | `State/SPVCoordinator` + `SyncProgressDamper` | 10 % max step, monotonic per SPV run, `isDone` after 3.25 s of `caught_up`, stall at 45 s or `SyncStalled`. |
| `AuthenticationGating` | `State/AuthenticationGate` | Lock state from `Vault.status` + `LockState` events; `authorize` under a 60 s watchdog (`auth.timed_out`, late grant revoked). `requirement(for:)`: none for no-vault/no-keys/unencrypted (no passphrase slot); passphrase when locked or mixing-only; when unlocked, passphrase for reveal/credential change/wipe (dw-vault refuses those grants without it whenever the vault has a passphrase slot) and, while "require authentication for every payment" is on (default), for spend/sign. |
| `VaultProviding` | `Services/VaultService` | Grant purpose checked before the engine; every returned status is forwarded to the gate. `SecretBuffer` = DashKit `SecretBytes` (zeroed on deinit); foreign buffers are copied into one. |
| `TransactionSending` / `TransactionDrafting` | `Services/TransactionSender`, `TransactionDraft` (actor) | Holds engine `PreparedTx` handles by `PreparedTransaction.id`. Setters abandon unsent prepared txs (M-7). After `send.no_peers`, `send.broadcast_rejected` or `send.prepared_tx_spent` the engine has released the inputs and the handle is dropped (`abandon` no-op, `broadcast` → `send.prepared_tx_unknown`). Argument/session errors (`invalid_argument`, `network_not_open`, `wallet_not_found`, `not_implemented`) keep the tx's previous state. Any other failure may have reached a peer and marks the tx "outcome unknown": `abandon` then throws `send.broadcast_outcome_unknown` and keeps the inputs reserved; `broadcast` may be called again. |
| `HistoryProviding` | `Services/HistoryService` | `changes(wallet:)` merges pending txid lists; `.resynchronize` yields `[]`. |
| `ReceiveProviding`, `CoinControlProviding`, `AddressBookProviding` | `Services/QueryServices` | Thin; engine stubs surface as `not_implemented`. |
| `MessageSigning`, `URIHandling`, `AmountFormatting` | `Services/ToolServices` (`MessageService`, `URIService`, `EngineAmountFormatter`) | Pure engine functions on the open network, else the last one, else the composition's fallback network. |
| `SettingsProviding` | `State/SettingsStore` | `settings.json` (display, payment-auth setting, named sections) and `global.json` (`lastNetwork`) in the data root; atomic writes; unreadable file → `.bak` + defaults + `recoveredFromCorruption`. |

`ServiceError` carries `parameters: [String: Int64]` (M-5: `fee`, `available`, `max_duffs`, `failed_attempts`,
`retry_after_secs`, `height`, `index`). `ServiceErrorCode.engineCodes` lists every §4 code; a test compares it
with m1-engine.md (M-9). Swift→FFI integers convert with `exactly:` / range checks in DashKit (M-8); the
`AmountStyle` digit counts clamp to 0...8 because `format` cannot throw.

### 2.2 Composition

```swift
// macOS @main (Linux/Windows: FixedDataLocation or the XDG / %APPDATA% root)
let root = try MacDataLocation().defaultDataRoot()
let runtime = try WalletRuntimeServices.live(dataRoot: root, networkOptions: { _ in NetworkOptions() })
let env = AppEnvironment(runtime: runtime, screenCapture: nil)   // WalletFeatures
Task { try await runtime.launch(defaultNetwork: .mainnet) }      // opens settings.lastNetwork ?? default
// applicationShouldTerminate / window close: try await runtime.shutdown()
```

`WalletRuntimeServices.init(engine:settings:…)` builds the same graph on any `EngineProtocol` (tests use
`FakeEngine`). Session observers run in the order settings → auth → wallet state → sync on start, reversed
on stop.

## 3. M1 view models (WalletFeatures) — public API sketch

All are `@MainActor @Observable public final class`, constructed with `AppEnvironment` (or the specific
protocols for tests). Outputs are plain values; flows are enums with exhaustive transitions.

### OnboardingViewModel (QT-102…105, IOS-002…007, IOS-010)

```swift
enum OnboardingStep { case welcome, choosePassphrase, unlockVault, showPhrase, verifyPhrase, restorePhrase, restoreOptions, working, done(WalletID), failed(OnboardingFailure) }
var step: OnboardingStep
var wordCount: Int                         // 12 or 24
var phraseWords: [String]                  // display only, from the SecretBuffer, cleared on leaving showPhrase
var verifyChallenge: [Int]                 // word positions to confirm (IOS-004 chips)
var restoreCheck: MnemonicCheck?           // live validation while typing
var options: WalletImportOptions           // birth height, Core compatibility, lookahead
func startCreate()                         // generateMnemonic → showPhrase (nothing stored yet)
func setPassphrase(_ text: String?, confirmation: String?) // nil = unencrypted vault
func confirmWrittenDown()                  // → verifyPhrase
func verify(word: String, at index: Int) -> Bool
func startRestore()
func updateRestoreText(_ text: String)     // makeSecret + checkMnemonic
func finish() async                        // vault.create (if none) → lifecycle.importWallet → done; locked vault → .unlockVault
func unlockVault(passphrase: String) async // AuthenticationGating.unlock(.full) → finish (adding a wallet to a locked vault)
```

### LockViewModel (QT-111, IOS-012/013)

```swift
var lockState: VaultLockState?
var retryAfter: Duration?                  // throttling countdown
var error: ServiceErrorCode?
func unlock(passphrase: String, mixingOnly: Bool) async
func lock() async
```

### HomeViewModel (QT-034, QT-036…039, IOS-019…023)

```swift
var balances: WalletBalances?              // nil = unknown
var formattedTotal: String?                // AmountFormatting, respects discreet mode
var recent: [TxRecord]                     // first page, 5–10 rows
var sync: SyncStatus?
var networkBadge: DashNetwork?             // non-mainnet badge (IOS-022)
var discreet: Bool
func toggleDiscreet()
func rotatePeers() async                   // shown when sync.isStalled
```

### SendViewModel (QT-052…063, IOS-041…052; DESIGN-opus §1.11)

```swift
enum SendPhase { case editing, confirmDuplicates, authorizing, preparing, confirm(PreparedTxSummary), broadcasting, done(txid: String), failed(SendFailure), broadcastUnknown(txid: String, failure: SendFailure) }
struct RecipientEntry: Identifiable { var address: String; var amountText: String; var subtractFee: Bool; var label: String; var error: ServiceErrorCode? }
var entries: [RecipientEntry]
var phase: SendPhase
var source: CoinSourceChoice               // .fullyMixed on the CoinJoin page (QT-051)
var fee: FeeChoice
var estimate: TxEstimate?
func addRecipient(); func removeRecipient(_ id: RecipientEntry.ID)
func paste(_ text: String)                 // URIHandling.parsePaymentURI fills an entry (QT-054)
func useMax(for id: RecipientEntry.ID) async  // maxSpendable − other entries, and subtractFee = true (M-4)
func review() async                        // validate → [confirmDuplicates] → authorize(.spend(max: Σ amounts)) → prepare → .confirm
func acknowledgeDuplicates() async         // "Yes": merge entries paying the same address, then review
func authorize(passphrase: String) async   // only in .authorizing; moves to .preparing before the vault answers
func confirm() async                       // broadcast ONLY here (iOS rule 4)
var canBroadcastAgain: Bool                // .broadcastUnknown and the engine still holds the signed tx
func broadcastAgain() async                // the same signed tx (same txid) again, only after an unknown outcome
func cancel() async                        // abandon the prepared tx, revoke an unredeemed grant; ignored once broadcast
func dismiss() async                       // leave done / failed (abandons, keeps the form) / broadcastUnknown (never abandons)
```

Send rules (m1-engine.md §2.7.1, review M-5/M-7/M-8, H-4; final review H3, M1, M3, M6, M7, Lows):
- The engine caps `external_sent` (value paid to scripts the wallet does not own), not the fee. The view model
  asks for `authorize(.spend(max:))` with `max = Σ recipient amounts`, no slack: subtract-fee shares and recipients
  the wallet owns only lower `external_sent`, and the fee is bounded separately by `send.absurd_fee`. The view
  model never sets a change address; a host that sets one the wallet does not own must add the change amount.
- Duplicates (QT-060): the engine refuses an address twice (`send.duplicate_address{index}`). `review()` asks
  first (`.confirmDuplicates`); "Yes" (`acknowledgeDuplicates()`) merges the entries with the same cleaned address
  into the first of them — amounts summed and re-formatted, subtract-fee if any had it, the first non-empty label,
  distinct messages joined by newlines — then reviews the merged form; "No" is `cancel()`. A merged amount above
  21 M DASH is an amount error on the merged entry.
- `authorize(passphrase:)` moves to `.preparing` before awaiting the vault, so a second submission while the
  passphrase is checked is ignored (one grant, one prepare).
- Any edit of entries, fee or source in confirmDuplicates / authorizing / preparing / confirm / failed returns to
  `.editing`, abandons the prepared tx and revokes an unredeemed grant; a prepare that finishes later is abandoned.
- Broadcast errors `send.broadcast_rejected`, `send.prepared_tx_spent`, `send.no_peers`, `send.prepared_tx_unknown`,
  `network_not_open`, `wallet_not_found`, `invalid_argument`, `not_implemented` are definite (`.failed`). After the
  first four the engine has released the inputs and spent the `PreparedTx`: the view model drops it, there is no
  retry, and `dismiss()` returns to `.editing` with the form kept so Review runs a new grant and prepare (M1).
  After the session/argument errors the tx is still pending and `dismiss()` abandons it.
- Any other broadcast error, including the engine's `send.broadcast_unknown`, is `.broadcastUnknown`: the user is
  routed to the transaction, the inputs stay reserved and the view model keeps the draft and prepared tx so
  `broadcastAgain()` can send the same signed tx (M3). A failed second attempt keeps `.broadcastUnknown` (the first
  attempt may still have reached a peer) with that attempt's failure; if the engine released the tx on it
  (`send.no_peers` etc.) `canBroadcastAgain` turns false. `cancel()` is ignored; `dismiss()` clears the form and
  drops the handles without abandoning (the engine refuses: `send.prepared_tx_spent`), so the inputs stay
  reserved in this session.
- Address book (QT-063, M7): the engine's broadcast owns the labels from the send form. After a successful send
  the view model only adds recipients without a label that are not in the address book yet (purpose send,
  `replace: false`, best effort); it never relabels.
- `ServiceError` needs the numeric context of a code (review M-5): `parameters: [String: Int64]` with `index`,
  `fee`, `available`, `max_duffs` as the engine reports them. `send.amount_with_fee_exceeds_balance` shows
  `parameters["fee"]` (the estimate is nil when `estimate()` itself failed this way; M6).
  `send.amount_too_small_after_fee{index}` marks that entry's amount with Dash Core's "The transaction amount is
  too small to pay the fee".
- Integer conversions to the engine use `UInt64(exactly:)` / `UInt8(exactly:)` and map a failure to
  `send.invalid_amount{index}` (amounts) or `invalid_argument` (counts, digits); never a trapping `UInt64(x)`
  (review M-8).

### ReceiveViewModel (QT-081…085, IOS-053…055)

```swift
var address: AddressInfo?
var qr: QRMatrix?                          // from URIHandling.qrMatrix(for: uri)
var requestAmountText: String; var label: String; var message: String
var uri: String?                           // buildPaymentURI
var requests: [ReceiveRequest]
func newAddress() async
func createRequest() async
func copyURI() -> String?
```

### TransactionsViewModel (QT-086…094, IOS-027…031)

```swift
var filter: HistoryFilter                  // persisted via SettingsProviding (QT-089)
var rows: [TxRecord]
var hasMore: Bool
var selection: Set<TxRecord.ID>; var selectedTotal: Amount?
var detail: TransactionDetail?
func reload() async; func loadMore() async
func select(_ id: TxRecord.ID) async       // loads detail
func setLabel(_ label: String?, txid: String) async
func exportCSV() async throws(ServiceError) -> String  // dash-qt columns (QT-093), pages through history; restarts on history.stale_cursor (3 attempts)
```

### AddressBookViewModel (QT-095…098)

```swift
var purpose: AddressPurpose; var search: String
var entries: [AddressBookEntry]
var selectionMode: Bool                    // picker for Send / Sign (QT-097)
func save(address: String, label: String, replace: Bool) async
func delete(address: String) async
func exportCSV() -> String                 // Label, Address
```

### SignVerifyViewModel (QT-099/100)

```swift
var address: String; var message: String; var signature: String
var result: SignVerifyResult?              // .signed / .verified / .failed(ServiceErrorCode) → dash-qt texts
func sign() async                          // authorize(.signMessage) → MessageSigning.sign
func verify()
```

### SettingsViewModel (QT-020, QT-135…141 subset, IOS-104/106)

```swift
var network: DashNetwork; var availableNetworks: [DashNetwork]
var display: DisplaySettings               // unit, decimal digits, discreet mode
var vault: VaultStatus?
func switchNetwork(to: DashNetwork) async  // LifecycleQueueing.switchNetwork
func update(_ display: DisplaySettings)
func encryptWallet(passphrase: String) async
func changePassphrase(old: String, new: String) async
func revealPhrase() async -> RevealedMnemonic?   // authorize(.revealSecret) first
```
