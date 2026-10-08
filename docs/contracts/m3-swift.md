# M3 Swift contract (WalletRuntime seams, view models)

Status: **contract**, 2026-10-07. Code: `Sources/WalletRuntime/Contracts/M3{CoinJoin,MasternodeKeys,Tools}.swift`
(protocols and value types only), the DashKit mapping of the M3 events and errors (`Sources/DashKit/Models.swift`,
`DashKitError+M3.swift`) and `ServiceErrorCode.m3EngineCodes`. Engine side: [`m3-engine.md`](m3-engine.md), whose
§1 is the **item → owner table** for every M3 checklist item. Everything in [`m1-swift.md`](m1-swift.md) §1
(layering, `ServiceError`, unknown = `nil`, secrets as `SecretBuffer`, observation, no singletons) and
[`m2-swift.md`](m2-swift.md) still applies. The visual source of truth is `docs/design/UX-SPEC.md` once it exists
(dashwallet-iOS look, desktop-adapted; Dash blue palette, no purple/violet).

**Scope change (2026-10-07).** Governance and masternode management are left to Dash Core (repo CLAUDE.md "Product
scope"). Their contracts, adapters, view models, demo services and tests are not on `main`; they are kept on the
branches `m3/r2-governance` and `m3/r3-protx`. Of the masternode work only the iOS Masternode Keys tool (IOS-083)
stays. The sections below that described the rest are marked parked.

## 1. Who builds what

| Owner | Builds |
|---|---|
| **R1** | `DashKit/EngineClient+M3CoinJoin.swift` (wrappers of m3-engine.md §2.1 and §2.6) and `WalletRuntime/M3/CoinJoinAdapters.swift`: `CoinJoinControlling`, `MixedCoinsMoving`, `NetworkStatisticsProviding`. |
| **R2** | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| **R3** | `DashKit/EngineClient+M3MasternodeKeys.swift` and `WalletRuntime/M3/MasternodeKeyAdapters.swift`: `MasternodeKeychainProviding`. The other masternode protocols: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| **V1** | The view models of §3 (`Sources/WalletFeatures/{CoinJoin,Masternodes}/**` plus additions to Shell, Options, Lock, Send, Tools), `M3Services` (the bundle of the protocols above, `M3Services.swift` + `M3Services+Live.swift`, as M2 did), fakes for tests, and the demo behaviour in `Sources/WalletDemo` (it answers like the engine: stubs stay `not_implemented`, no fake success). Tests are named after checklist ids (`QT041_…`, `IOS083_…`). |
| **U** | MacUI and CrossUI screens over §3. |

R owners do not change `EngineProtocol` / `FakeEngine` for M3: the M3 wrappers are extensions on `EngineClient` and
the adapters take an `EngineClient`. Until an adapter lands, the view model shows the feature as unavailable
(`not_implemented` → "Not available yet"), never an empty success state.

The CoinJoin adapter turns its engine event into the protocol's stream: `coinJoinChanged(network, wallet)` →
`CoinJoinControlling.statusChanges()`, a re-query signal (`EventBus` may coalesce it).

Live wiring: `WalletRuntimeServices.m3` (`M3RuntimeServices`) builds the three adapters on the app's
`EngineClient`; `M3Services.live(runtime:)` bundles them, and both app composition roots build one per run. A
runtime over another engine (tests' `FakeEngine`) has no `m3`, and `M3Services.live(runtime:)` then answers
`not_implemented` (`UnavailableM3Service`).

## 2. Protocols

### 2.1 CoinJoin (`M3CoinJoin.swift`) — adapter R1

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `CoinJoinControlling` | `limits()`, `settings()`, `setSettings(_:)`, `status(wallet:)`, `statusChanges()`, `start(wallet:)`, `stop(wallet:)`, `salt/setSalt/generateSalt`. | `coinjoin_limits`, `coinjoin_settings`, `set_coinjoin_settings`, `coinjoin_status`, `start_mixing`, `stop_mixing`, `coinjoin_salt`, `set_coinjoin_salt`, `generate_coinjoin_salt`; event `CoinJoin` | QT-041…050, QT-112 |
| `MixedCoinsMoving` | `recoveryScan(wallet:)`, `plan(wallet:destination:)`, `move(wallet:destination:grant:)` (`.spend(max: ≥ plan.total)`). | `coinjoin_recovery_scan`, `mixed_coins_sweep_plan`, `move_mixed_coins` | IOS-057 |

The CoinJoin send page stays `TransactionSending` with `CoinSourceChoice.fullyMixed` (M1, already in
`SendViewModel`); its coin control lists `utxos(fullyMixedOnly: true)`.

### 2.2 Governance — adapter R2

**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

### 2.3 Masternode keychain (`M3MasternodeKeys.swift`) — adapter R3

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `MasternodeKeychainProviding` | `keys(wallet:role:range:)` (≤ 100; roles `owner`, `voting`, `operator`, `platformNode`), `reveal(wallet:role:index:grant:)` (`.revealSecret`). | `masternode_keys`, `Vault.reveal_masternode_key` | IOS-083 |

The masternode list, registration, maintenance, shared and tracked masternodes and evonode protocols:
**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

### 2.4 Tools (`M3Tools.swift`) — adapter R1

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `NetworkStatisticsProviding` | `statistics() -> NetworkStatistics` (credit pool and InstantSend `nil` on SPV, MN/EvoNode counts, best ChainLock, quorums). | `network_stats` | QT-144 |

The Governance sub-tab of Tools → Information: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

### 2.5 Errors and parameters

`ServiceErrorCode.m3EngineCodes` lists every m3-engine.md §4 code; `SettingsAndCodesTests` compares it with the
table. `ServiceError.parameters` carries `min_duffs`; DashKit fills it (`DashKitError+M3.swift`). A sweep's
stopping error is `MixedCoinsSweepResult.failureCode`.

## 3. M3 view models (V1) — public API sketch

All are `@MainActor @Observable public final class`, built from `AppEnvironment` + `M3Services` (or the specific
protocols in tests). Strings come from dash-qt (research 02 §9) and iOS through `L10n+CoinJoin/Masternodes`.

### CoinJoin
- **CoinJoinPanelViewModel** (QT-041…044, 047…050, QT-112): `status`, `buttonTitle` ("Start CoinJoin" / "Stop
  CoinJoin" / "(Disabled)"), `statusText` ("Enabled"/"Disabled" + ", keys left: N" only when `keysLeft != nil`),
  `amountAndRoundsText` (`~` and red when `insufficientInputs`), `progress` + tooltip lines, advanced fields
  (`showAdvanced` from Options), `sessionStatusText` (QT-050), `toggle()`: first use shows the "Most Common" filter
  hint once (settings flag); below the minimum shows "CoinJoin requires at least %2 to use."; a locked vault asks
  "Unlock wallet for mixing only" (`Vault.unlock(.mixingOnly)`), and a cancel shows "Wallet is locked and user
  declined to unlock. Disabling CoinJoin.". Re-reads on `statusChanges()` for its wallet only (QT-049).
- **CoinJoinOptions** in `OptionsViewModel.coinJoin` (QT-046, 047, QT-135): engine settings (`setSettings` on OK,
  revert on Cancel, ranges from `limits()`, goal ≤ hard cap), plus host settings `showAdvancedCoinJoinUI`
  (`fShowAdvancedCJUI`), `lowKeysWarning` (`fLowKeysWarning`, hidden for HD-only wallets but kept for import
  parity), `showCoinJoinNotifications` (M2).
- **CoinJoin send page** (QT-051): `SendViewModel` in its CoinJoin mode (exists) gains the mixed balance header and
  "Send mixed funds"; coin control lists fully mixed coins only (QT-071).
- **MixedCoinsViewModel** (IOS-057): recovery scan with progress, plan preview, move with grant, partial-result
  copy, "Later" remembered per balance; entries in Overview, Options/Security and Tools.
- **Shell** (QT-012, 016, 018, 022, 029, 153): CoinJoin section and tray/Dock entry follow `features.coinJoin` and
  the CoinJoin enable option; Settings ▸ "Unlock Wallet for mixing only"; the orange mixing-only lock icon (exists);
  Help ▸ CoinJoin information enabled.

### Masternodes
- **MasternodeKeychainViewModel** (IOS-083): key pages per role (owner, voting, operator, platform node) with
  reveal behind `.revealSecret`, copy of public data and revealed secrets.
- The list, detail, registration, maintenance, shared and tracked masternode view models: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

### Governance
**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

### Tools
- **NetworkInformationViewModel** (QT-144): the Network sub-tab from `NetworkStatisticsProviding`; credit pool and
  InstantSend rows "—" with "Requires full-node data source"; mempool rows stay M2's.

## 4. Decisions carried from the engine contract

- QT-048: no backup gate and no keypool warning for our HD wallets (m3-engine.md §8); the view models show only
  the `CoinJoinUnavailable` states.
- QT-124, QT-132 and the evonode views: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**
