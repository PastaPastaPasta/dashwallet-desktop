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
  retryAfterSeconds }`. `code.rawValue` is the engine's stable code (m1-engine.md §4). View models pick UI
  copy by code; `detail` is for logs only.
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
| `AuthenticationGating` | The one auth primitive: `lockState`, `lockStateChanges()`, `requirement(for:)`, `authorize(_:credential:)`, `unlock`, `lock`. | `Vault.authorize`, `unlock`, `lock`; event `LockState` | QT-022, QT-061, IOS-012…017 |
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
**L3** `SecretBytes` wipe.

## 3. M1 view models (WalletFeatures) — public API sketch

All are `@MainActor @Observable public final class`, constructed with `AppEnvironment` (or the specific
protocols for tests). Outputs are plain values; flows are enums with exhaustive transitions.

### OnboardingViewModel (QT-102…105, IOS-002…007, IOS-010)

```swift
enum OnboardingStep { case welcome, choosePassphrase, showPhrase, verifyPhrase, restorePhrase, restoreOptions, working, done(WalletID), failed(ServiceError) }
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
func finish() async                        // vault.create (if none) → lifecycle.importWallet → done
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
enum SendPhase { case editing, authorizing, preparing, confirm(PreparedTxSummary), broadcasting, done(txid: String), failed(ServiceError) }
struct RecipientEntry: Identifiable { var address: String; var amountText: String; var subtractFee: Bool; var label: String; var error: ServiceErrorCode? }
var entries: [RecipientEntry]
var phase: SendPhase
var source: CoinSourceChoice               // .fullyMixed on the CoinJoin page (QT-051)
var fee: FeeChoice
var estimate: TxEstimate?
func addRecipient(); func removeRecipient(_ id: RecipientEntry.ID)
func paste(_ text: String)                 // URIHandling.parsePaymentURI fills an entry (QT-054)
func useMax(for id: RecipientEntry.ID) async
func review() async                        // validate → AuthenticationGating.authorize(.spend) → prepare → .confirm
func confirm() async                       // broadcast ONLY here (iOS rule 4)
func cancel() async                        // abandon the prepared tx
```

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
func exportCSV() async throws(ServiceError) -> String  // dash-qt columns (QT-093), pages through history
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
