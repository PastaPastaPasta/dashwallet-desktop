# M3 engine contract (`dw-ffi`)

Status: **contract**, 2026-10-07. Milestone M3 "CoinJoin and the iOS Masternode Keys tool" (DESIGN-opus §5.1).
Code: `rust/crates/dw-ffi/src/api/{coinjoin,network_stats,masternode_keys}.rs`, the engine modules
`rust/crates/dw-engine/src/{coinjoin,masternode_keys}.rs`, the crates `dw-p2p` and `dw-coinjoin`, and the event of
§3. Generated Swift:
`Sources/DashWalletCore/Generated/DashWalletCore.swift`. The Swift side is [`m3-swift.md`](m3-swift.md).
Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) and [`m2-engine.md`](m2-engine.md) still applies.

**Scope change (2026-10-07).** Governance (R2) and the masternode list, ProTx flows, shared masternodes, tracked
masternodes and evonode tools (R3) were left to Dash Core by this change (repo CLAUDE.md "Product scope" then; DEC-01 of 2026-10-08 brings the owner-side flows back as milestone MG, DASHPAY §5a; operator-signed ProUpServTx and ProUpRevTx stay out). Their engine, FFI and
Swift code is not on `main`; it is kept on the branches `m3/r2-governance` and `m3/r3-protx`. The sections below
that described them are marked parked. From R3 only the iOS Masternode Keys tool (IOS-083) stays: `masternode_keys`
and `Vault.reveal_masternode_key`, implemented in `dw-engine/src/masternode_keys.rs`.

Every M3 call on `main` holds behaviour (**works**). Calls whose finished form needs Platform or the shielded
pool return `NotImplemented { call }` with a suffix: "move mixed coins" to the shielded pool uses `.shielded` and
lands with M4. Implementers do not change a signature without updating this file, m3-swift.md and the generated
bindings in the same change. `rust/crates/dw-ffi/src/api/m3_tests.rs` tests the calls' argument checks and
compares the §4 codes with this file.

## 0. Owners

| Owner | Scope (paths) |
|---|---|
| **R1 coinjoin** | crates `dw-p2p` and `dw-coinjoin`; `dw-engine/src/coinjoin.rs` (+ the CoinJoin parts of `send/` for `CoinSource::FullyMixedOnly` and of `keys.rs` for the recovery lookahead); FFI `coinjoin.rs`, `network_stats.rs`; event `CoinJoin`; console `coinjoin`, `coinjoinsalt`, `getcoinjoininfo`; regtest/testnet mixing (G6) |
| **R2 governance** | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| **R3 masternode keys** | `dw-engine/src/masternode_keys.rs`, FFI `masternode_keys.rs`, the vault's `with_revealed_seed`. The masternode list, ProTx, shared and tracked masternodes and evonode tools: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| **V1 view models** | `Sources/WalletFeatures/**` (m3-swift.md §3), fakes, `ServiceErrorCode.m3EngineCodes` users |
| **U UI** (later phase) | MacUI and CrossUI screens over V1's view models (UX-SPEC.md once it exists) |

There is no S owner in M3: each R owner also writes the DashKit wrappers (`Sources/DashKit/EngineClient+M3<Domain>.swift`)
and the WalletRuntime adapter (`Sources/WalletRuntime/M3/<Domain>Adapters.swift`) for its domain's protocols in
m3-swift.md §2, one file per domain.

## 1. Item → owner

Primary owner first, "+" = supporting owner. Every item also needs U (MacUI + CrossUI) unless it has no screen.
Items marked (rest) are partial rows whose M1/M2 part is done.

| Item | Area | Owner | Engine / service surface |
|---|---|---|---|
| QT-012 (rest) | CoinJoin tab and shortcut | V1 | `ShellModel` sections from Options (Wallet ▸ CoinJoin). The Masternodes and Governance tabs: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-016 (rest) | Settings ▸ "Unlock Wallet for mixing only" | V1 | `Vault.unlock(…, MixingOnly)` (M1) |
| QT-018 (rest) | Help ▸ CoinJoin information enabled | V1 | — (Swift text) |
| QT-022 (rest) | orange mixing-only lock icon | V1 | `VaultLockState::UnlockedMixingOnly` (M1) |
| QT-026 | governance clock | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-029 (rest) | tray / Dock "CoinJoin" entry | V1 | — |
| QT-041 | Overview CoinJoin panel | R1 + V1 | `coinjoin_status`, `start_mixing`, `stop_mixing`, event `CoinJoin` |
| QT-042 | progress formula + tooltip | R1 | `CoinJoinStatus.progress` (`dw-coinjoin` `progress`) |
| QT-043 | fully-mixed rule with the salt | R1 | `CoinJoinBalances.fully_mixed`, `UtxoFilter.fully_mixed_only` (M1), `coinjoin_salt`, `set_coinjoin_salt`, `generate_coinjoin_salt` |
| QT-044 | start (min balance, first-use hint, unlock for mixing), stop resets | R1 + V1 | `start_mixing` (`coinjoin.insufficient_funds`, `coinjoin.vault_locked`), `stop_mixing` |
| QT-045 | protocol, MN selection, BLS-verified `dsq` | R1 | `dw-p2p`, `dw-coinjoin` |
| QT-046 | CoinJoin options, live | R1 + V1 | `coinjoin_settings`, `set_coinjoin_settings`, `coinjoin_limits` |
| QT-047 | advanced UI, low-keys warning, popups toggles | V1 | Swift settings (QSettings equivalents); `keys_left` is `None` for HD wallets |
| QT-048 | mixing disabled conditions | R1 + V1 | `CoinJoinStatus.unavailable`; see §8 (backup gate not copied, DESIGN-opus §7.5) |
| QT-049 | per-wallet state, global options | R1 | per-wallet calls, per-network settings |
| QT-050 | session status text | R1 + V1 | `CoinJoinStatus.{status, sessions, queue_size}` |
| QT-051 | CoinJoin send page | R1 + V1 | `TxDraft` + `CoinSource::FullyMixedOnly` (M1) with the §5 rules |
| QT-061 (rest) | full unlock for a send, mixing-only kept | V1 + R1 | passphrase grant on a mixing-only vault (M2 rule: the lock state does not change) |
| QT-071 (rest) | CoinJoin page's coin control: fully mixed only | V1 + R1 | `utxos(UtxoFilter { fully_mixed_only: true })` (M1) |
| QT-112 | unlock for mixing only | R1 + V1 | `Vault.unlock(MixingOnly)` (M1); mixing stops with `stop_reason = VaultLocked` when the vault locks |
| QT-118 | Masternodes tab, filters, browsable without wallet | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-119 | list columns | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-120 | owned detection | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-121 | context menu | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-122 | details dialog | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-123 | Register wizard | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-124 | operator secret gate | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-125 | Update Service / Registrar / Revoke | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-126 | shared MN creation | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-127 | shared MN maintenance | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-128 | Governance tab, sources, title filter | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-129 | proposal columns, statuses | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-130 | context menu (raw JSON, URL, vote) | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-131 | vote dialog | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-132 | Create Proposal wizard | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-133 | Resume Proposals | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-134 | governance info panel | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-135 (rest) | Options ▸ CoinJoin tab | V1 | as QT-046 |
| QT-139 (rest) | Display ▸ Show governance clock | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-144 | Information: mempool; Network sub-tab | R1 + V1 | `network_stats` (R1), `NodeInfo` mempool `None` (M2). The Governance sub-tab: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-145 (rest) | console: CoinJoin commands | R1 | `console_execute` (M2). Governance and masternode commands: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| QT-153 (rest) | CoinJoin information explainer | V1 | — |
| IOS-030 (rest) | CoinJoin withdrawals group | V1 + R1 | `TxType` CoinJoin values (M2) |
| IOS-057 | recovery scan + move mixed coins | R1 + V1 | `coinjoin_recovery_scan`, `mixed_coins_sweep_plan`, `move_mixed_coins` (shielded: M4) |
| IOS-058 | mixing (decide) | R1 | **IN** (DESIGN-opus §7.1): everything under QT-041…051 |
| IOS-080 | MN list + detail, claimable balance, epoch blocks | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| IOS-081 | evonode status, credit withdrawal, unban with pending state | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| IOS-082 | tracked MNs + attached keys, tracked withdraw / unban | — | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** |
| IOS-083 | masternode keychain | R3 + V1 | `masternode_keys`, `Vault.reveal_masternode_key` (**works**) |

## 2. Calls

Status of every call: **works** unless marked otherwise. "In-memory read" calls are sync and must stay O(1)
(m1-engine.md rule 3); the rest are async.

### 2.1 CoinJoin (`coinjoin.rs`) — owner R1

| Call | Kind | Semantics | Errors (besides common) | Serves |
|---|---|---|---|---|
| `coinjoin_limits()` | sync, free | **works**. Denominations (largest first), `min_mixing_balance` 140001, collateral 10000…40000, option ranges, dash-qt defaults. | — | QT-044, QT-046 |
| `coinjoin_settings()` | sync | **works**. `CoinJoinSettings { enabled, multi_session, max_sessions, rounds, target_amount_dash, denoms_goal, denoms_hard_cap }`; dash-qt defaults until changed. Per network, stored in app.sqlite. | — | QT-046, QT-049 |
| `set_coinjoin_settings(s)` | async | **works**. Validates, applies live, stores. `enabled = false` stops every wallet (`stop_reason = Disabled`). Rounds/target changes take effect at the next session. | `invalid_argument` | QT-046 |
| `coinjoin_status(wallet)` | sync | **works**. `CoinJoinStatus` (below). In-memory read of the status the engine refreshes once a second (the first read of a wallet computes it); re-query on `CoinJoin`. | — | QT-041…043, 048, 050 |
| `start_mixing(wallet)` | async | **works**. Idempotent. Needs `enabled`, a wallet with keys, balance ≥ 0.00140001 DASH and a vault that is unencrypted, unlocked or unlocked for mixing only. Runs while SPV syncs (status `SyncInProgress`, as Core). | `coinjoin.disabled`, `coinjoin.watch_only`, `coinjoin.insufficient_funds{min_duffs}`, `coinjoin.vault_locked` | QT-044, QT-112 |
| `stop_mixing(wallet)` | async | **works**. `resetPool` then stop: open sessions are left, reserved coins released. Idempotent. | — | QT-044 |
| `coinjoin_salt(wallet)` / `set_coinjoin_salt(wallet, hex)` / `generate_coinjoin_salt(wallet)` | async | Core `coinjoinsalt get/set/generate`. The salt is created at first use, imported from `cj_salt` with a Dash Core wallet (R1 adds this to `import_wallet_dat`), stored in app.sqlite. `set` is refused while mixing (`invalid_argument`). **works** (the wallet.dat import keeps `cj_salt`, or the older `ps_salt`). | `invalid_argument` | QT-043, QT-145 |
| `coinjoin_recovery_scan(wallet)` | async | **works**. IOS-057: raises the DIP9 CoinJoin account's lookahead and the BIP44 chains' to 1000, rescans from the birth height (a normal rescan: `rescan_progress`, `cancel_rescan`), returns `CoinJoinRecoveryReport { coinjoin_addresses_scanned, bip44_addresses_scanned, coinjoin_balance, new_transactions }`. | `coinjoin.spv_not_running`; `invalid_argument` while another rescan runs | IOS-057 |
| `mixed_coins_sweep_plan(wallet, dest)` | async | **works**. Chunks of ≤ 500 inputs of the CoinJoin account's coins above 1000 duffs, with fees. Reads coins only. `Shielded`: M4. | `coinjoin.nothing_to_move` | IOS-057 |
| `move_mixed_coins(wallet, dest, grant)` | async | **works**. Broadcasts the plan's chunks to a fresh BIP44 receive address of the same wallet; `Spend` grant ≥ total. Stops at the first failing chunk and returns `MixedCoinsSweepResult { txids, moved, remaining, failure_code }`. `Shielded`: M4 (`NotImplemented{…move_mixed_coins.shielded}`). | `coinjoin.nothing_to_move`, `coinjoin.vault_locked`, `coinjoin.grant_invalid`, `coinjoin.no_peers`, `coinjoin.broadcast_rejected{reason}` | IOS-057 |

`CoinJoinStatus`:

- `state` `Idle | Mixing | Stopping`; `stop_reason` `UserRequested | VaultLocked | WalletUnloaded |
  SessionClosed | Disabled`.
- `unavailable` `Disabled | WatchOnly | InsufficientFunds{min_duffs}` ("(Disabled)" button). A locked vault is not
  "unavailable": the host offers "Unlock wallet for mixing only" and, if declined, shows "Wallet is locked and user
  declined to unlock. Disabling CoinJoin.".
- `balances { anonymizable, denominated, normalized_anonymized, fully_mixed }`: Core's names
  (`GetAnonymizableBalance`, …); `fully_mixed` is the panel's "CoinJoin Balance" and the CoinJoin page's
  spendable. The M1 `WalletBalances.coinjoin` becomes this value (§5).
- `progress { overall_percent, denominated_percent, partially_mixed_percent, mixed_percent, average_rounds }` with
  dash-qt's formula (research 02 §9.2): `max = min(anonymizable + fully_mixed, target)`; parts are
  `min(1, x/max)·100` for denominated (weight 1), normalized (weight rounds) and fully mixed (weight 2);
  `overall = Σ ceil(part·weight/(3+rounds)·100)/100`, capped at 100. Golden vectors
  `testdata/coinjoin_progress.json` (R1 adds them, checked against dash-qt's code path).
- `amount_and_rounds { amount, rounds, insufficient_inputs }`, `submitted_denominations`, `sessions[] {
  pro_tx_hash, service, denomination, state (PoolState), entries, last_message (PoolMessage) }`, `status`
  (Core's `strAutoDenomResult` as `CoinJoinStatusCode`, with `Masternode{message}` for "Masternode: …"),
  `queue_size`, `keys_left` (`None`: HD wallets have no keypool).

### 2.2 Governance — owner R2

**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** `governance_params`, govsync, proposals, votes, proposal creation, `governance_info` and
`governance_clock` are not on `main`.

### 2.3 Masternode list — owner R3

**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** `masternode_network_defaults`, `masternode_list_state`, `masternodes(query)` and
`masternode_detail` are not on `main`. The Information window's masternode and EvoNode counts stay
(`NodeInfo.masternodes/evonodes`, M2; `NetworkStats.masternodes/evonodes`, §2.6).

### 2.4 Provider transactions — owner R3

**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** Registration, Update Service/Registrar, Revoke and the v24 shared-masternode calls are not on
`main`.

### 2.5 Masternode keychain (`masternode_keys.rs`) — owner R3

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `masternode_keys(wallet, role, start, count)` | async | **works**. Derived provider keys of `role` (`Owner|Voting|Operator|PlatformNode`) for indexes `start..start+count` (`count ≤ 100`): path, address (secp256k1 roles), public key, legacy BLS form (operator), platform node id (platform node). Public data, no grant. Paths (DIP3/DIP9, key-wallet `AccountType::Provider*Keys`): `m/9'/coin'/3'/1'/i` voting, `2'` owner, `3'` operator (BLS), `4'` platform node (ed25519, hardened child `i'`); `coin` 5 on mainnet, 1 elsewhere. Platform node keys are read from the 20 keys platform-wallet pre-derives at import (SLIP-10 has no public derivation); indexes past the pool are left out. Payout keys are BIP44 addresses and are not listed. | `invalid_argument`, `masternode.watch_only` (no provider account) | IOS-083 |
| `Vault.reveal_masternode_key(wallet, role, index, grant)` | async | **works**. One private key of a wallet's derived key. `RevealSecret` grant for that wallet, consumed; the vault decrypts the seed for this one derivation (`Vault::with_revealed_seed`). `RevealedMasternodeKey { private_key_hex, wif?, tenderdash_key? }` as bytes: WIF for secp256k1 roles, the Tenderdash form (base64 of seed ‖ public key) for platform node keys. | `masternode.watch_only`, `vault_locked`, `grant_invalid` | IOS-083 |

Tracked masternodes with attached keys (IOS-082) and the evonode tools (IOS-080, IOS-081): **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

### 2.6 Network sub-tab (`network_stats.rs`) — owner R1

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `network_stats()` | sync | **works** (quorums: platform-wallet has no quorum-list accessor at the pin, so the list stays empty; hosts show it as unavailable). `NetworkStats { credit_pool: None, instantsend: None, masternodes, evonodes, best_chainlock, quorums[] { llmq_name, llmq_type, active, health_percent, rotated } }`. Credit pool and InstantSend counters need a full node (§7); quorums come from the synced list's quorum entries (empty before). In-memory read; re-query on `Sync`. | — (`SyncError` domain) | QT-144 |

## 3. Events (additions to `EngineEvent`)

| Event | When | Host reaction | Emitter |
|---|---|---|---|
| `CoinJoin { network, wallet_id }` | Mixing state, sessions, status code, balances or progress of the wallet changed. ≤ 1 Hz per wallet (dash-qt's 1 s panel timer), trailing edge kept. | re-query `coinjoin_status` | R1 |

A re-query signal (DashKit: not lifecycle; `EventBus` may coalesce it). `HistoryChanged`/`Balances` still fire for
the transactions mixing creates. The `Governance` and `Masternodes` events: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

## 4. Error codes

Common codes unchanged (m1-engine.md §4). Domain codes (a dw-ffi test compares these rows with the enums; a Swift
test compares them with `ServiceErrorCode.m3EngineCodes`):

| Domain enum | Codes |
|---|---|
| `CoinJoinError` | `coinjoin.disabled`, `coinjoin.watch_only`, `coinjoin.insufficient_funds`, `coinjoin.vault_locked`, `coinjoin.grant_invalid`, `coinjoin.nothing_to_move`, `coinjoin.spv_not_running`, `coinjoin.no_peers`, `coinjoin.broadcast_rejected` |
| `MasternodeError` | `masternode.watch_only`, `masternode.vault_locked`, `masternode.grant_invalid` |

`GovernanceError` and the other `masternode.*` codes: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**

`network_stats` uses `SyncError` and adds no code. Parameters the UI shows (review M-5 rule), forwarded by DashKit in
`ServiceError.parameters`: `min_duffs`.

## 5. Changes to M1/M2 behaviour

- **`CoinSource::FullyMixedOnly` (R1).** Stops returning `NotImplemented`. Candidates: coins of the CoinJoin account
  that pass the QT-043 rule, confirmed or IS-locked. The draft has **no change output**: the difference between the
  inputs and the recipients becomes fee (dash-qt CoinJoin page); `TxEstimate.fee` shows it, and `send.absurd_fee`
  still caps it. The transaction is stored with the DS=1 mark so history shows `CoinJoinSend`. A `ChangePolicy`
  other than `Auto` is refused with `invalid_argument` for this source.
- **`WalletBalances.coinjoin` (R1).** Becomes `CoinJoinBalances.fully_mixed` (the M1 doc comment says the rule was
  not applied yet).
- **`UtxoFilter.fully_mixed_only` and `Utxo.coinjoin_rounds` (R1).** Use the real rounds (input-chain walk) and the
  salt rule.
- **Import (R1).** `import_wallet_dat` stores the `cj_salt` record as the wallet's CoinJoin salt.
- **Vault (R1).** A vault that locks while a wallet mixes stops it (`stop_reason = VaultLocked`). Mixing signs with
  the mixing-only scope (`AccountType::CoinJoin` keys and collateral inputs only); a send from a mixing-only vault
  still needs a passphrase grant and leaves the lock state as it was (M2). Denomination and collateral
  transactions made from BIP44 coins are signed with `Vault::mixing_funding_signer` (scope
  `CoinJoinFunding`: CoinJoin-account and BIP44 paths, no grant), as Core lets a mixing-only unlock create them;
  the engine uses it only for transactions whose outputs all pay the wallet.
- **Restore lookahead (R1).** Wallets imported with `core_compat` scan the DIP9 CoinJoin account and the BIP44
  chains with gap 1000 (DESIGN.md R2), as `coinjoin_recovery_scan` does for existing wallets.
- **`NodeInfo.masternodes/evonodes`** keep their M2 meaning.
- **Console (R1).** The commands listed under "CoinJoin" in `dw-console`'s table become available (Core result
  shapes, the redaction list unchanged). "Governance", "Masternode" and "Evo" stay unavailable: **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**
- **Vault grants.** The `MasternodeOp` and `Governance` grant purposes of m1-engine.md are removed with the parked
  domains; the keychain reveal uses `RevealSecret`.

## 6. Crates

| Crate | Owner | Now | To build |
|---|---|---|---|
| `dw-p2p` | R1 | `commands` (wire names verified against Core `protocol.cpp`) | `codec`, `session` (handshake incl. `senddsq`, ping, rate limit), `peers` (from the SPV list), `proxy` (after U1) |
| `dw-coinjoin` | R1 | `denoms`, `settings` (ranges, defaults, validation), `status` (`PoolState`, `PoolMessage` wire order, status codes) | `rounds`, `progress`, `planner`, `messages`, `queue`, `session`, `client`, `recovery` |
| `dw-governance`, `dw-protx` | R2, R3 | **Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).** The keychain paths live in `dw-engine/src/masternode_keys.rs`. | — |

`dw-p2p` public API the engine's CoinJoin uses: `Session::connect(addr, network, SessionConfig) -> Result<Session>`,
`Session::send(command, payload)`, `Session::subscribe(&[command]) -> Receiver<(command, payload)>`,
`Session::close()`, and a `PeerPicker` over a masternode-list snapshot (`masternodes()`, a "recently used" ring).
The snapshot comes from the engine (dash-spv's `masternode_list_engine`, exposed through platform-wallet's SPV
runtime as M2's `count_masternodes` already reads it).

## 7. Full-node-only features (DESIGN-opus §1.14)

| Feature | M3 answer |
|---|---|
| Mempool count/usage, credit pool, InstantSend counters | `None` (`NodeInfo`, `NetworkStats`). |
| CoinJoin own-collateral check | Approximated: our collateral is valid when confirmed, the right amount and unspent in our view (Core checks its mempool). |

## 8. Decisions and open points

- **IOS-058: mixing is IN** (DESIGN-opus §7.1). If G6 fails, mixing ships as "Experimental" (off by default);
  balance and "move mixed coins" stay.
- **QT-048: the automatic-backup gate is not copied** (DESIGN-opus §7.5, research 02 §21 quirk 10). Our wallets are
  HD with no keypool to exhaust, so `keys_left` is `None` and no "low keys" warning fires; the only "disabled"
  states are `CoinJoinUnavailable::{Disabled, WatchOnly, InsufficientFunds}`. The parity row records this as a
  deliberate deviation.
- **Governance and masternode management were Dash Core's** (repo CLAUDE.md "Product scope" at the time): **Parked (branches `m3/r2-governance`, `m3/r3-protx`).** Superseded by DEC-01: the owner-side flows are milestone MG (DASHPAY §5a); the operator side stays with the node and its CLI.
- **CoinJoin output chain.** DIP9 CoinJoin account (DESIGN.md R2, overriding Fable's BIP44 default).
- **G6** gate: DESIGN-opus §6. G7 (governance sync) is parked with governance.

## 9. G7 — mainnet governance sync measurement (R2)

**Parked — Dash Core scope (branches `m3/r2-governance`, `m3/r3-protx`).**
