# M3 Swift contract (WalletRuntime seams, view models)

Status: **contract**, 2026-10-06. Code: `Sources/WalletRuntime/Contracts/M3{CoinJoin,Governance,Masternodes,Tools}.swift`
(protocols and value types only), the DashKit mapping of the M3 events and errors (`Sources/DashKit/Models.swift`,
`DashKitError+M3.swift`) and `ServiceErrorCode.m3EngineCodes`. Engine side: [`m3-engine.md`](m3-engine.md), whose
§1 is the **item → owner table** for every M3 checklist item. Everything in [`m1-swift.md`](m1-swift.md) §1
(layering, `ServiceError`, unknown = `nil`, secrets as `SecretBuffer`, observation, no singletons) and
[`m2-swift.md`](m2-swift.md) still applies. The visual source of truth is `docs/design/UX-SPEC.md` once it exists
(dashwallet-iOS look, desktop-adapted; Dash blue palette, no purple/violet).

## 1. Who builds what

| Owner | Builds |
|---|---|
| **R1** | `DashKit/EngineClient+M3CoinJoin.swift` (wrappers of m3-engine.md §2.1 and §2.6) and `WalletRuntime/M3/CoinJoinAdapters.swift`: `CoinJoinControlling`, `MixedCoinsMoving`, `NetworkStatisticsProviding`. |
| **R2** | `DashKit/EngineClient+M3Governance.swift` and `WalletRuntime/M3/GovernanceAdapters.swift`: `GovernanceProviding`, `GovernanceVoting`, `ProposalCreating`. |
| **R3** | `DashKit/EngineClient+M3Masternodes.swift` and `WalletRuntime/M3/MasternodeAdapters.swift`: `MasternodeListProviding`, `MasternodeRegistering`, `MasternodeMaintaining`, `SharedMasternodeCoordinating`, `MasternodeKeychainProviding`, `TrackedMasternodeManaging`, `EvonodeServicing`. |
| **V1** | The view models of §3 (`Sources/WalletFeatures/{CoinJoin,Masternodes,Governance}/**` plus additions to Shell, Options, Lock, Send, Tools), `M3Services` (the bundle of the protocols above, `M3Services.swift` + `M3Services+Live.swift`, as M2 did), fakes for tests, and the demo behaviour in `Sources/WalletDemo` (it answers like the engine: stubs stay `not_implemented`, no fake success). Tests are named after checklist ids (`QT041_…`, `IOS083_…`). |
| **U** | MacUI and CrossUI screens over §3. |

R owners do not change `EngineProtocol` / `FakeEngine` for M3: the M3 wrappers are extensions on `EngineClient` and
the adapters take an `EngineClient`. Until an adapter lands, the view model shows the feature as unavailable
(`not_implemented` → "Not available yet"), never an empty success state.

Each adapter turns its engine event into the protocol's stream: `coinJoinChanged(network, wallet)` →
`CoinJoinControlling.statusChanges()`, `governanceChanged` → `GovernanceProviding.changes()`, `masternodesChanged`
→ `MasternodeListProviding.changes()`. These are re-query signals (`EventBus` may coalesce them; `.resynchronize`
counts as all three).

## 2. Protocols

### 2.1 CoinJoin (`M3CoinJoin.swift`) — adapter R1

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `CoinJoinControlling` | `limits()`, `settings()`, `setSettings(_:)`, `status(wallet:)`, `statusChanges()`, `start(wallet:)`, `stop(wallet:)`, `salt/setSalt/generateSalt`. | `coinjoin_limits`, `coinjoin_settings`, `set_coinjoin_settings`, `coinjoin_status`, `start_mixing`, `stop_mixing`, `coinjoin_salt`, `set_coinjoin_salt`, `generate_coinjoin_salt`; event `CoinJoin` | QT-041…050, QT-112 |
| `MixedCoinsMoving` | `recoveryScan(wallet:)`, `plan(wallet:destination:)`, `move(wallet:destination:grant:)` (`.spend(max: ≥ plan.total)`). | `coinjoin_recovery_scan`, `mixed_coins_sweep_plan`, `move_mixed_coins` | IOS-057 |

The CoinJoin send page stays `TransactionSending` with `CoinSourceChoice.fullyMixed` (M1, already in
`SendViewModel`); its coin control lists `utxos(fullyMixedOnly: true)`.

### 2.2 Governance (`M3Governance.swift`) — adapter R2

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `GovernanceProviding` | `parameters()`, `syncState()`, `setSyncEnabled(_:)`, `changes()`, `proposals(_:)`, `detail(hash:)`, `info()`, `clock()`. | `governance_params`, `governance_sync_state`, `set_governance_sync_enabled`, `proposals`, `proposal_detail`, `governance_info`, `governance_clock`; event `Governance` | QT-026, QT-128…130, QT-134, QT-144 |
| `GovernanceVoting` | `votingMasternodes(proposal:wallet:)`, `cast(_:on:with:grant:)` (`.governance`). | `voting_masternodes`, `cast_votes` | QT-131 |
| `ProposalCreating` | `superblockDates(count:)`, `validate(_:)`, `json(_:)`, `payloadHex(_:)`, `create(wallet:draft:grant:)` (`.spend(max: ≥ 1 DASH + fee)`), `pending(wallet:)`, `submit(wallet:hash:)`. | `superblock_dates`, `validate_proposal`, `proposal_json`, `proposal_payload_hex`, `create_proposal`, `pending_proposals`, `submit_proposal` | QT-132, QT-133 |

### 2.3 Masternodes (`M3Masternodes.swift`) — adapter R3

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `MasternodeListProviding` | `defaults()`, `state()`, `changes()`, `list(_ query:)`, `detail(proTxHash:)`. | `masternode_network_defaults`, `masternode_list_state`, `masternodes`, `masternode_detail`; event `Masternodes` | QT-118…122, IOS-080 |
| `MasternodeRegistering` | `collateralCandidates`, `feeSourceCandidates`, `prepare(_:grant:) -> PreparedRegistrationReference`, `operatorSecret(_:)`, `confirmOperatorSecret(_:last4:)`, `submit(_:collateralSignature:)`, `abandon(_:)`. The adapter keeps the engine `PreparedRegistration` behind the reference's `id`. | `collateral_candidates`, `fee_source_candidates`, `prepare_registration`, `PreparedRegistration.*` | QT-123, QT-124 |
| `MasternodeMaintaining` | `prepareUpdateService`, `prepareUpdateRegistrar`, `prepareRevoke`, `prepareShareRewardUpdate`, `prepareDissolveNow` → `PreparedProviderTransaction`; `broadcast`, `abandon`; `createStandbyDissolution`, `broadcastStandbyDissolution`. Typed operator secrets are `SecretBuffer`s. | `prepare_update_service`, `prepare_update_registrar`, `prepare_revoke`, `prepare_share_reward_update`, `prepare_dissolve_now`, `PreparedProviderTx.*`, `create_standby_dissolution`, `broadcast_standby_dissolution` | QT-125, QT-127, IOS-081 (unban) |
| `SharedMasternodeCoordinating` | `create`, `importMessage` (paste routing), `sessions`, `message`, `contribute`, `approve`, `sign`, `broadcast`, `abandon`, `startKeyRotation`, `startDissolveTogether`. | `create_shared_session`, `import_shared_message`, `shared_session*`, `start_shared_key_rotation`, `start_dissolve_together` | QT-126, QT-127 |
| `MasternodeKeychainProviding` | `keys(wallet:role:range:)` (≤ 100), `reveal(wallet:role:index:grant:)` (`.revealSecret`). | `masternode_keys`, `Vault.reveal_masternode_key(wallet, None, …)` | IOS-083 |
| `TrackedMasternodeManaging` | `locate`, `tracked`, `track`, `untrack`, `setLabel`, `attach(_:role:proTxHash:grant:)` (`.masternodeOperation`), `detach`, `reveal(role:proTxHash:grant:)`. | `locate_masternodes`, `tracked_masternodes`, `track_masternode`, `untrack_masternode`, `set_tracked_masternode_label`, `attach_masternode_key`, `detach_masternode_key`, `Vault.reveal_masternode_key(None, hash, …)` | IOS-082 |
| `EvonodeServicing` | `status(proTxHash:)`, `withdraw(proTxHash:credits:destination:grant:)`. May answer `not_implemented` until M4 (Platform). | `evonode_status`, `withdraw_evonode_credits` | IOS-080, IOS-081 |

### 2.4 Tools (`M3Tools.swift`) — adapter R1

| Protocol | Role | Engine calls | Serves |
|---|---|---|---|
| `NetworkStatisticsProviding` | `statistics() -> NetworkStatistics` (credit pool and InstantSend `nil` on SPV, MN/EvoNode counts, best ChainLock, quorums). | `network_stats` | QT-144 |

The Governance sub-tab of Tools → Information is `GovernanceProviding.info()`.

### 2.5 Errors and parameters

`ServiceErrorCode.m3EngineCodes` lists every m3-engine.md §4 code; `SettingsAndCodesTests` compares it with the
table. `ServiceError.parameters` carries `min_duffs`, `needed`, `available`, `confirmations`, `retry_after_secs`,
`size_bytes`, and the enum positions `field` (`ProposalField.allCases`), `role` (`MasternodeKeyRole.allCases`) and
`refusal` (`CollateralRefusal.allCases`); DashKit fills them (`DashKitError+M3.swift`). Per-masternode vote results
carry the code in `VoteResult.errorCode`; a sweep's stopping error is `MixedCoinsSweepResult.failureCode`.

## 3. M3 view models (V1) — public API sketch

All are `@MainActor @Observable public final class`, built from `AppEnvironment` + `M3Services` (or the specific
protocols in tests). Strings come from dash-qt (research 02 §9–11) through `L10n+CoinJoin/Masternodes/Governance`.

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
- **MasternodeListViewModel** (QT-118…121, IOS-080): `query` (type, text, owned, hide banned — persisted as
  `mnListTypeFilter`, `mnListFilterText`, `mnListOwnedOnly`, `mnListHideBanned`), `rows`, `nodeCount`, `state`,
  `columns` (Type hidden for Regular/Evo filters), context-menu actions (copy proTxHash, copy collateral `txid-n`,
  filter by collateral/payout/owner/voting address, Update Service/Registrar (not for shared)/Revoke, shared
  actions), full-node columns "—" with the honest tooltip.
- **MasternodeDetailViewModel** (QT-122, IOS-080): every §10.1 field, shares table, evonode Platform status (shown
  as unavailable while `not_implemented`).
- **RegisterMasternodeWizardViewModel** (QT-123, 124): pages Type → Collateral → Service → Keys → Payout → Platform
  (Evo) → Fee → Review → Save operator key → Prove ownership (external) → Complete, "Step %1 of %2 · %3", default
  ports from `defaults()`, validation messages from `masternode.*` codes, the last-4 gate, dash-qt's error
  explanations for `bad-protx-*` reasons.
- **Maintenance view models** (QT-125, IOS-081): Update Service (also "Unban" with pending state), Update Registrar
  (PoSe-ban warning), Revoke (reason picker); each prepare → review → broadcast.
- **SharedMasternodeViewModel** (QT-126, 127): coordinator and participant flows, fingerprint/session code display,
  copy/save/paste of envelopes, close protection (save or release coins), dissolve now/together/standby.
- **MasternodeKeychainViewModel** (IOS-083) and **TrackedMasternodesViewModel** (IOS-082): key pages per role with
  reveal behind `.revealSecret`, track by IP/hash/key, attach keys, capabilities.

### Governance
- **ProposalListViewModel** (QT-128…130): source (Active / My Proposals), title filter, rows with dash-qt status
  text and tooltips, Votes column `"%1Y, %2N, %3A (%4%5)"`, My Votes `"%1Y, %2N, %3A / %4 unvoted"` or "No voting
  keys", deadline label ("Voting deadline: ~%1 left (%2 blocks, block %3)" / passed / "waiting for sync…"), Open URL
  (http/https only, External Link Warning defaulting to No), Copy Raw JSON, sync state ("from peers, may lag").
  Turns `setSyncEnabled(true)` on while the tab or clock is enabled.
- **ProposalVoteViewModel** (QT-131): outcome, checkable masternodes, Select All / Clear, weight summary, "Vote %1",
  results "Voted successfully %n time(s)" / "Failed to vote %n time(s)" with per-masternode errors.
- **CreateProposalWizardViewModel** (QT-132): fields and validation, 12 payment dates, total, View JSON/Payload, the
  non-refundable 1 DASH confirmation, then opens Resume.
- **ResumeProposalsViewModel** (QT-133): pending list, collateral status, Broadcast at ≥ 1 confirmation, "Proposal
  has been broadcasted to the network with hash %1".
- **GovernanceInfoViewModel** (QT-134, Tools ▸ Information ▸ Governance) and **GovernanceClockViewModel** (QT-026,
  status bar; opens Governance on click; shown with Display ▸ Show governance clock).

### Tools
- **NetworkInformationViewModel** (QT-144): the Network sub-tab from `NetworkStatisticsProviding`; credit pool and
  InstantSend rows "—" with "Requires full-node data source"; mempool rows stay M2's.

## 4. Decisions carried from the engine contract

- QT-048: no backup gate and no keypool warning for our HD wallets (m3-engine.md §8); the view models show only
  the `CoinJoinUnavailable` states.
- QT-124: the engine refuses `submit` before the last-4 gate; the view model enforces the same page order.
- QT-132: the chosen payment date is honoured; the wizard says so in its help text.
- Platform-dependent evonode views stay visible and say "Not available yet" while the engine answers
  `not_implemented` (`.platform` calls).
