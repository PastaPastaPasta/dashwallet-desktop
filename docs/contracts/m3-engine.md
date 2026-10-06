# M3 engine contract (`dw-ffi`)

Status: **contract**, 2026-10-06. Milestone M3 "Masternodes, governance, CoinJoin" (DESIGN-opus §5.1). Code:
`rust/crates/dw-ffi/src/api/{coinjoin,network_stats,governance,masternode,protx,masternode_keys}.rs`, the
failure types in `rust/crates/dw-engine/src/{coinjoin,governance,masternodes}.rs`, the new crates `dw-p2p`,
`dw-coinjoin`, `dw-governance`, `dw-protx`, and the three events of §3. Generated Swift:
`Sources/DashWalletCore/Generated/DashWalletCore.swift`. The Swift side is [`m3-swift.md`](m3-swift.md).
Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) and [`m2-engine.md`](m2-engine.md) still applies.

Every M3 call exists in the FFI now. Each stub first does what the finished call will do before any work: it
parses its wallet ids, hashes and outpoints (`invalid_argument`) and checks the session (`network_not_open`).
Then it returns its domain's `NotImplemented { call }`, where `call` is `"<Object>.<method>"`. No call fakes
success. Calls that hold behaviour already (**works**):

- `coinjoin_limits()`, `governance_params(network)`, `masternode_network_defaults(network)`: Dash Core's
  constants, from `dw-coinjoin::{denoms,settings}`, `dw-governance::params`, `dw-protx::params`.
- `set_coinjoin_settings` checks every range first (`invalid_argument` naming the field).
- `set_coinjoin_salt` checks the salt is 64 lowercase hex; `import_shared_message` refuses text over 2 MiB
  (`masternode.shared_envelope_too_large`); `masternode_keys` refuses `count > 100`;
  `Vault.reveal_masternode_key` needs exactly one of `wallet_id` and `pro_tx_hash`.
- Stubs that take a typed operator secret or a key to attach overwrite those bytes before they return.

Calls whose finished form needs Platform (evonode credits) return `NotImplemented` with a `.platform` suffix
(`NetworkSession.evonode_status.platform`, `NetworkSession.withdraw_evonode_credits.platform`), and "move mixed
coins" to the shielded pool uses `.shielded`. Those may stay `NotImplemented` through M3 and land with M4's
Platform work; the other stubs are M3 work. Implementers replace the stub body. They do not change a signature
without updating this file, m3-swift.md and the generated bindings in the same change.
`rust/crates/dw-ffi/src/api/m3_tests.rs` tests the stubs and compares the §4 codes with this file.

## 0. Owners

| Owner | Scope (paths) |
|---|---|
| **R1 coinjoin** | crates `dw-p2p` and `dw-coinjoin`; `dw-engine/src/coinjoin.rs` (+ the CoinJoin parts of `send/` for `CoinSource::FullyMixedOnly` and of `keys.rs` for the recovery lookahead); FFI `coinjoin.rs`, `network_stats.rs`; event `CoinJoin`; console `coinjoin`, `coinjoinsalt`, `getcoinjoininfo`; regtest/testnet mixing (G6) |
| **R2 governance** | crate `dw-governance`; `dw-engine/src/governance.rs`; FFI `governance.rs`; event `Governance`; console `gobject`, `getgovernanceinfo`, `getsuperblockbudget`; the G7 measurement (§9) |
| **R3 protx + masternodes** | crate `dw-protx`; `dw-engine/src/masternodes.rs`; FFI `masternode.rs`, `protx.rs`, `masternode_keys.rs`; event `Masternodes`; console `masternodelist`, `protx`, `bls` |
| **V1 view models** | `Sources/WalletFeatures/**` (m3-swift.md §3), fakes, `ServiceErrorCode.m3EngineCodes` users |
| **U UI** (later phase) | MacUI and CrossUI screens over V1's view models (UX-SPEC.md once it exists) |

There is no S owner in M3: each R owner also writes the DashKit wrappers (`Sources/DashKit/EngineClient+M3<Domain>.swift`)
and the WalletRuntime adapter (`Sources/WalletRuntime/M3/<Domain>Adapters.swift`) for its domain's protocols in
m3-swift.md §2, one file per domain, so the three never edit the same Swift file. Until an adapter lands, V1 works
against fakes.

Shared files and how to touch them without collisions:

- `dw-engine/src/error.rs`, `lib.rs`, `events.rs` and `dw-ffi/src/api/{mod,error,wallet,engine}.rs` already carry
  every M3 variant and module; owners do not need to edit them. A new failure variant goes into the owner's own
  failure enum (`CoinJoinFailure`, `GovernanceFailure`, `MasternodeFailure`) plus its FFI mapping and §4 row.
- `dw-ffi/Cargo.toml` and `dw-engine/Cargo.toml` already depend on the new crates. Each new crate's own
  `Cargo.toml` belongs to its owner.
- `dw-console`: each owner adds its commands in its own module (`src/m3_coinjoin.rs`, `src/m3_governance.rs`,
  `src/m3_masternode.rs`) and one dispatch arm in `exec.rs`; the command table in `lib.rs` already lists them.
- `dw-p2p` is R1's. R2 uses its public API (§6). A change R2 needs is made by R1 or agreed with R1 first.

## 1. Item → owner

Primary owner first, "+" = supporting owner. Every item also needs U (MacUI + CrossUI) unless it has no screen.
Items marked (rest) are partial rows whose M1/M2 part is done.

| Item | Area | Owner | Engine / service surface |
|---|---|---|---|
| QT-012 (rest) | CoinJoin / Masternodes / Governance tabs and shortcuts | V1 | `ShellModel` sections from Options (Display ▸ Show Masternodes/Governance Tab, Wallet ▸ CoinJoin) |
| QT-016 (rest) | Settings ▸ "Unlock Wallet for mixing only" | V1 | `Vault.unlock(…, MixingOnly)` (M1) |
| QT-018 (rest) | Help ▸ CoinJoin information enabled | V1 | — (Swift text) |
| QT-022 (rest) | orange mixing-only lock icon | V1 | `VaultLockState::UnlockedMixingOnly` (M1) |
| QT-026 | governance clock | R2 + V1 | `governance_clock`, event `Governance` |
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
| QT-118 | Masternodes tab, filters, browsable without wallet | R3 + V1 | `masternode_list_state`, `masternodes(MasternodeQuery)`, event `Masternodes` |
| QT-119 | list columns | R3 | `MasternodeRow` (full-node columns `None`, §7) |
| QT-120 | owned detection | R3 | `MasternodeRow.owned_roles` |
| QT-121 | context menu | V1 | `MasternodeRow` fields; filters set `MasternodeQuery.text` |
| QT-122 | details dialog | R3 + V1 | `masternode_detail` |
| QT-123 | Register wizard | R3 + V1 | `collateral_candidates`, `fee_source_candidates`, `prepare_registration`, `PreparedRegistration.*`, `masternode_network_defaults` |
| QT-124 | operator secret gate | R3 + V1 | `PreparedRegistration.operator_secret`, `confirm_operator_secret`; `submit` refuses before the gate |
| QT-125 | Update Service / Registrar / Revoke | R3 + V1 | `prepare_update_service`, `prepare_update_registrar`, `prepare_revoke`, `PreparedProviderTx.*` |
| QT-126 | shared MN creation | R3 + V1 | `create_shared_session`, `import_shared_message`, `shared_session_*` |
| QT-127 | shared MN maintenance | R3 + V1 | `prepare_share_reward_update`, `start_shared_key_rotation`, `prepare_dissolve_now`, `start_dissolve_together`, `create_standby_dissolution`, `broadcast_standby_dissolution`, paste routing via `import_shared_message` |
| QT-128 | Governance tab, sources, title filter | R2 + V1 | `set_governance_sync_enabled`, `governance_sync_state`, `proposals(ProposalQuery)` |
| QT-129 | proposal columns, statuses | R2 | `ProposalRow` |
| QT-130 | context menu (raw JSON, URL, vote) | V1 + R2 | `proposal_detail.raw_json`, `ProposalRow.url` |
| QT-131 | vote dialog | R2 + V1 | `voting_masternodes`, `cast_votes` |
| QT-132 | Create Proposal wizard | R2 + V1 | `superblock_dates`, `validate_proposal`, `proposal_json`, `proposal_payload_hex`, `create_proposal`, `governance_params` |
| QT-133 | Resume Proposals | R2 + V1 | `pending_proposals`, `submit_proposal` |
| QT-134 | governance info panel | R2 + V1 | `governance_info` |
| QT-135 (rest) | Options ▸ CoinJoin tab | V1 | as QT-046 |
| QT-139 (rest) | Display ▸ Show governance clock | V1 | turns `set_governance_sync_enabled` on with the clock |
| QT-144 | Information: mempool; Network sub-tab; Governance sub-tab | R1 + R2 + V1 | `network_stats` (R1), `NodeInfo` mempool `None` (M2), `governance_info` (R2) |
| QT-145 (rest) | console: CoinJoin, governance, masternode commands | R1, R2, R3 (each its commands) | `console_execute` (M2) |
| QT-153 (rest) | CoinJoin information explainer | V1 | — |
| IOS-030 (rest) | CoinJoin withdrawals group | V1 + R1 | `TxType` CoinJoin values (M2) |
| IOS-057 | recovery scan + move mixed coins | R1 + V1 | `coinjoin_recovery_scan`, `mixed_coins_sweep_plan`, `move_mixed_coins` (shielded: M4) |
| IOS-058 | mixing (decide) | R1 | **IN** (DESIGN-opus §7.1): everything under QT-041…051 |
| IOS-080 | MN list + detail, claimable balance, epoch blocks | R3 + V1 | `masternodes(owned_only)`, `masternode_detail`, `evonode_status` (Platform part may be M4) |
| IOS-081 | evonode status, credit withdrawal, unban with pending state | R3 + V1 | `evonode_status`, `withdraw_evonode_credits` (may be M4), `prepare_update_service` (unban, M3) |
| IOS-082 | tracked MNs + attached keys, tracked withdraw / unban | R3 + V1 | `locate_masternodes`, `track_masternode`, `untrack_masternode`, `tracked_masternodes`, `set_tracked_masternode_label`, `attach_masternode_key`, `detach_masternode_key` |
| IOS-083 | masternode keychain | R3 + V1 | `masternode_keys`, `Vault.reveal_masternode_key` |

## 2. Calls

Status of every call: **stub** unless marked **works**. "In-memory read" calls are sync and must stay O(1)
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

### 2.2 Governance (`governance.rs`) — owner R2

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `governance_params(network)` | sync, free | **works**. Superblock start/cycle/maturity window, min quorum, 1 DASH fee, 6 fee confirmations, name ≤ 40, payload ≤ 512 B, ≤ 12 payments, EvoNode weight 4, 1 h vote interval, 150 s spacing. Regtest values are Core's defaults (`-budgetparams` may differ on a node). | — | QT-132, QT-134 |
| `governance_sync_state()` | sync | `GovernanceSyncState { phase: Disabled|Waiting|SyncingObjects|SyncingVotes|Synced|Failed, objects, votes, peers, bytes_received, last_synced_at }`. | — | QT-128, QT-134, G7 |
| `set_governance_sync_enabled(on)` | async | Starts/stops govsync (persisted). The host turns it on while the Governance tab or the clock is enabled, so a wallet that never shows governance never downloads it. Sync starts once headers and the masternode list synced (phase `Waiting` before). Objects first (`govsync` with a zero hash), then votes per current-cycle proposal; votes are checked against the list's voting key ids; invalid ones are dropped. | — | QT-128 |
| `proposals(query)` | async | `ProposalRow`s sorted by dash-qt's status order (Funded, Passing, Unfunded, Voting, Confirming, Pending, Failing, Lapsed; then deficit). `Active` needs sync on; `Mine{wallet}` works without sync and includes pending ones. Title filter: case-insensitive substring. | `governance.sync_disabled` | QT-128, QT-129 |
| `proposal_detail(hash)` | async | `ProposalDetail { row, parent_hash, collateral_txid, created_at, payments, raw_json }`. | `governance.proposal_not_found` | QT-130 |
| `voting_masternodes(hash, wallet?)` | async | Masternodes whose **voting** key a wallet holds (derived DIP3 voting keys of all wallets, or one wallet's) or a tracked masternode has attached; with current vote, vote time and `next_vote_at` (1 h rule). | `governance.proposal_not_found`, `governance.not_synced` (masternode list not synced) | QT-131 |
| `cast_votes(hash, outcome, mns, grant)` | async | Signs `CGovernanceVote(collateral outpoint, hash, FUNDING, outcome, time)` with each voting key and relays it to ≥ 1 peer. `Governance` grant. One `VoteResult { pro_tx_hash, error_code, detail }` per masternode ("Voted successfully %n time(s)" / "Failed to vote %n time(s)"); a masternode inside the 1 h window gets `governance.vote_too_often` without being sent. | `governance.no_voting_keys`, `governance.vault_locked`, `governance.grant_invalid`, `governance.no_peers` (whole call) | QT-131 |
| `superblock_dates(count)` | sync | The next `count` (≤ 12) superblocks `{ height, estimated_time }` from the tip. | `governance.not_synced` (no tip) | QT-132 |
| `validate_proposal(draft)` | sync | Failing `ProposalField`s in field order (name 1–40 `[-_a-z0-9]`, URL no spaces ≥ 4, P2PKH/P2SH of this network, amount > 0, count 1–12, first payment one of the next 12 superblocks, payload ≤ 512 B). | — | QT-132 |
| `proposal_json(draft)` / `proposal_payload_hex(draft)` | sync | "View JSON" (key order `name, payment_address, payment_amount, url, start_epoch, end_epoch, type`) / "View Payload". The chosen first superblock is honoured (dash-qt bug not copied): `start_epoch` = its estimated time − cycle/2, `end_epoch` = estimated time of the last payment + cycle/2. | `governance.invalid_proposal{field}` | QT-132 |
| `create_proposal(wallet, draft, grant)` | async | `gobject prepare`: validates, builds and broadcasts the 1 DASH `OP_RETURN <hash>` collateral transaction (+ change), stores the proposal as pending in app.sqlite. `Spend` grant ≥ 1 DASH + fee. Returns the `PendingProposal`. | `governance.invalid_proposal{field}`, `governance.insufficient_funds{needed, available}`, `governance.watch_only`, `governance.vault_locked`, `governance.grant_invalid`, `governance.no_peers`, `governance.broadcast_rejected{reason}` | QT-132 |
| `pending_proposals(wallet)` | async | Unsubmitted, unexpired proposals with `collateral_status Unknown|Pending|Ready` and confirmations. Re-query on `Governance` and `HistoryChanged`. | — | QT-133 |
| `submit_proposal(wallet, hash)` | async | `gobject submit` at ≥ 1 confirmation; returns the hash. The proposal then shows as `Confirming` until 6 confirmations. | `governance.collateral_unconfirmed{confirmations}`, `governance.proposal_expired`, `governance.proposal_not_found`, `governance.no_peers`, `governance.broadcast_rejected{reason}` | QT-133 |
| `governance_info()` | async | `GovernanceInfo` (QT-134 fields: cycle, last/next superblock + ETA, voting cutoff, MN/EvoNode participation "N (M eligible)", threshold `max(min_quorum, weighted_valid/10)`, controlled MNs and votes, counts passing/failing/unfunded + short, budget allocated/available). Fields needing synced objects are `None` until synced; the budget comes from the height. | — | QT-134, QT-144 |
| `governance_clock()` | sync | `GovernanceClock { cycle_progress, next_superblock, blocks_to_superblock, superblock_eta, voting_cutoff, voting_open, budget_committed }`. Needs the tip; `budget_committed` `None` until synced. | `governance.not_synced` (no tip) | QT-026 |

Status rule (dash-qt order, research 02 §11.1): Funded (in an active trigger) → Lapsed (past end) → Confirming
(< 6 collateral confirmations) → Pending (not broadcast) → inside the maturity window Passing/Failing → Voting
(needs votes) → Passing/Unfunded (budget saturated). Margin = `(yes − no) − threshold` with weighted votes.

### 2.3 Masternode list (`masternode.rs`) — owner R3

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `masternode_network_defaults(network)` | sync, free | **works**. Default Core/Platform ports (9999/26656/443 main, 19999/22000/22001 test, 19799/22100/22101 devnet, 19899/22200/22201 regtest), collaterals 1000/4000 DASH, shares 2–8 ≥ 100 DASH, early period ≤ 420480, envelope ≤ 2 MiB, reward ≤ 100.00 %. | — | QT-123, QT-126 |
| `masternode_list_state()` | sync | `{ available, height, total, enabled, evo_total, evo_enabled, syncing }` from the SPV list. | — | QT-118, QT-144 |
| `masternodes(query)` | async | Rows matching `MasternodeQuery { type_filter, text, owned_only, hide_banned }` (dash-qt's literal case-insensitive search fields, research 02 §10.1). Without a wallet the list still works. Before the list synced: wallet and tracked masternodes only, status `Unknown`. | — | QT-118…121, IOS-080 |
| `masternode_detail(pro_tx_hash)` | async | `MasternodeDetail` (QT-122 fields; shares, early period, penalty, standby flag, network/Platform address lists, revocation reason, PoSe heights `None` on SPV). | `masternode.not_found` | QT-122, IOS-080 |

Row sources (research 02 §10.2): service, type, valid/banned, voting key, operator key and platform node id from the
SML (`dash-spv` masternode list engine, platform-wallet `MasternodeListSummary`); owner/payout/collateral, shares,
registration height and operator reward from provider transactions the wallets hold (platform-wallet
`aggregate_masternodes`) or that tracked masternodes recorded. Owned detection (QT-120): collateral in a wallet;
owner, voting, payout or operator-payout key in a wallet (derived DIP3 provider keys and BIP44 addresses); operator
key or platform node key derived by a wallet (platform-wallet `operator_key_index`, `platform_key_index`); share
owner or refund script of a shared masternode; tracked (`OwnedRole::Tracked`). Refresh: the `Masternodes` event.

### 2.4 Provider transactions (`protx.rs`) — owner R3

Every flow: `prepare_*` validates, builds, signs and reserves inputs (nothing sent) → the host reviews `summary()` →
`broadcast()`/`submit()` sends, or `abandon()` releases. `MasternodeOp` grants for every signing call; a `FundNew`
registration also needs the grant's spend allowance to cover the collateral.

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `collateral_candidates(wallet, type)` | async | Wallet UTXOs of exactly the collateral amount, each with `refusal: WrongAmount|Unconfirmed|NotP2pkh|Locked|AlreadyCollateral|NotFound` or `None`. | — | QT-123 |
| `fee_source_candidates(wallet)` | async | Addresses with spendable coins (no "automatic" in the Register wizard, dash-qt). | — | QT-123 |
| `prepare_registration(request, grant)` | async | `RegistrationRequest { wallet_id, node_type, collateral: FundNew|ExistingUtxo|External, service_addresses, owner_address?, voting_address?, operator_key: Generate|Existing{hex}, payout_address, operator_reward_x100, platform?, fee_source }`. Rules: service optional (`IP:port`, network default ports; pre-v24 EvoNode needs one); owner and voting P2PKH and ≠ collateral address; payout P2PKH/P2SH ≠ owner/voting/collateral; operator key basic-scheme BLS; Platform node id 40 hex, required for EvoNodes only; fee source must hold the fee (+ collateral with `FundNew`). External: the registration is prepared but not signed by the collateral. | `masternode.invalid_service`, `invalid_key{role}`, `invalid_payout`, `duplicate_address`, `collateral_unavailable{refusal}`, `insufficient_funds{needed, available}`, `watch_only`, `vault_locked`, `grant_invalid` | QT-123 |
| `PreparedRegistration.summary()` | sync | `RegistrationSummary` (review page incl. `pro_tx_hash`, `fee`, `total_spent`, `operator_secret_required`, `collateral_sign_message` for external). | — | QT-123 |
| `PreparedRegistration.operator_secret()` | sync | The generated secret and the `masternodeblsprivkey=` line as bytes (shown once, never stored; gone after `submit`/`abandon`). | `invalid_argument` (key not generated) | QT-124 |
| `PreparedRegistration.confirm_operator_secret(last4)` | sync | Opens the gate when `last4` equals the secret's last 4 characters (case-insensitive); returns whether it did. | — | QT-124 |
| `PreparedRegistration.submit(signature?)` | async | Broadcasts; `signature` = base64 `signmessage` of `collateral_sign_message` for external collateral. Returns the proTxHash. Refused before the gate opened (dash-qt's page order: secret before broadcast). Core reject reasons (`bad-protx-dup-key`, `bad-protx-dup-addr`, `bad-protx-version`, `too-early`, …) arrive in `broadcast_rejected.reason`; the host adds dash-qt's explanations. | `masternode.operator_secret_unconfirmed`, `collateral_signature_invalid`, `no_peers`, `broadcast_rejected{reason}` | QT-123, QT-124 |
| `PreparedRegistration.abandon()` | async | Releases inputs, forgets the secret. | — | QT-123 |
| `prepare_update_service(request, grant)` | async | ProUpServTx; revives a PoSe-banned node (IOS-081 unban). `operator_secret` typed each time (dash-qt) or `None` = a wallet-derived or tracked-attached operator key. Platform fields for EvoNodes; operator payout only with reward > 0. Entries with extended network info that a version-2 payload would overwrite are refused. | `masternode.operator_secret_mismatch`, `key_not_in_wallet{role}`, `invalid_service`, `unsupported_entry`, `insufficient_funds` | QT-125, IOS-081, IOS-082 |
| `prepare_update_registrar(request, grant)` | async | ProUpRegTx with the owner key; only changed fields. `summary().bans_masternode` when the operator key changes ("Changing the operator key immediately PoSe-bans the masternode…"). Refused for shared masternodes (`unsupported_entry`). | `masternode.key_not_in_wallet{owner}`, `invalid_key`, `invalid_payout` | QT-125 |
| `prepare_revoke(request, grant)` | async | ProUpRevTx, reason `NotSpecified|TerminationOfService|CompromisedKeys|ChangeOfKeys`, operator key as above. | `masternode.operator_secret_mismatch`, `key_not_in_wallet{operator}` | QT-125 |
| `PreparedProviderTx.summary()/broadcast()/abandon()` | sync/async | `ProviderTxSummary { kind, pro_tx_hash, txid, fee, penalty, bans_masternode }`; `broadcast` returns the txid. | `masternode.no_peers`, `broadcast_rejected{reason}` | QT-125, QT-127 |
| `create_shared_session(wallet, terms)` | async | Coordinator starts a v24 shared registration: 2–8 shares ≥ 100 DASH summing to 1000, early period ≤ 420480, penalty < smallest share. Session persisted in app.sqlite. | `invalid_argument` (terms), as registration | QT-126 |
| `import_shared_message(wallet, text)` | async | Paste/file routing (QT-127): ≤ 2 MiB (**works**), this network only, checks fingerprint/revision/stage; an envelope (`type "dash-shared-mn-session"`, `version 1`) goes into its session (created on an invitation); standby-dissolution text is returned for broadcast. | `masternode.shared_envelope_too_large{size_bytes}`, `shared_envelope_invalid`, `shared_network_mismatch`, `shared_session_not_found` | QT-126, QT-127 |
| `shared_sessions(wallet)` | async | Open sessions (resumable after restart), with `reserved_coin_spent` when a reserved coin was spent elsewhere. | — | QT-126 |
| `shared_session_message(id)` | async | The outgoing envelope for the current stage (`json`, `fingerprint` `XXXX-XXXX`, file name). | `shared_session_not_found` | QT-126 |
| `shared_session_contribute(id, contribution)` | async | Participant: reserves coins (persistent locks) and fills a share's owner/payout/refund addresses. | `shared_session_not_found`, `collateral_unavailable` | QT-126 |
| `shared_session_approve(id, grant)` / `shared_session_sign(id, grant)` | async | Consent signature with the share owner key / signs the wallet's inputs after checking no foreign or short-changed input is asked. | `shared_inputs_refused`, `shared_coin_spent{outpoint}` | QT-126 |
| `shared_session_broadcast(id)` | async | Coordinator combines and broadcasts; returns the txid (proTxHash of a registration). | `no_peers`, `broadcast_rejected` | QT-126 |
| `shared_session_abandon(id)` | async | Leaves and releases reserved coins (close protection). | — | QT-126 |
| `prepare_share_reward_update(hash, share, payout, fee_wallet, grant)` | async | ProUpShareTx (type 11). | `key_not_in_wallet{owner}`, `invalid_payout` | QT-127 |
| `start_shared_key_rotation(wallet, hash, operator_key?, voting?)` | async | ProUpSharedRegTx (type 12) through a session every owner approves. | — | QT-127 |
| `prepare_dissolve_now(hash, accept_penalty, fee_wallet, grant)` | async | ProDissolveTx (type 10), unilateral; `accept_penalty` must be true inside the early period. | `invalid_argument` | QT-127 |
| `start_dissolve_together(wallet, hash)` | async | Unanimous dissolution session (all principals returned). | — | QT-127 |
| `create_standby_dissolution(wallet, hash, grant)` / `broadcast_standby_dissolution(txs)` | async | The two raw transactions to save as `.txt` (the engine records that one exists, `MasternodeDetail.has_standby_dissolution`) / broadcast them later in order. | `no_peers`, `broadcast_rejected` | QT-127 |

ProDissolveTx, ProUpShareTx, ProUpSharedRegTx and the share fields of ProRegTx are not in rust-dashcore at the
pin (verified: `special_transaction/` has the four classic ProTx payloads only). R3 writes their codecs in
`dw-protx::shared::payloads` from Core `src/evo/providertx.h` (no upstream patch), with payload-equality tests
against dashd's `protx shared_*` RPCs on regtest.

### 2.5 Keychain, tracked masternodes, evonode tools (`masternode_keys.rs`) — owner R3

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `masternode_keys(wallet, role, start, count)` | async | Derived provider keys (`count ≤ 100`, **works**): path, address (secp256k1 roles), public key, legacy BLS form, platform node id, where used (`MasternodeKeyUsage { pro_tx_hash, service, revoked }`). Public data, no grant. Paths (DIP3/DIP9, key-wallet `AccountType::Provider*Keys`): `m/9'/coin'/3'/1'/i` voting, `2'` owner, `3'` operator (BLS), `4'` platform (ed25519); payout keys are BIP44 addresses. | `invalid_argument` | IOS-083 |
| `Vault.reveal_masternode_key(wallet?, pro_tx_hash?, role, index, grant)` | async | One private key: a wallet's derived key (`wallet_id`, `index`) or a tracked masternode's attached key (`pro_tx_hash`). `RevealSecret` grant. `RevealedMasternodeKey { private_key_hex, wif?, tenderdash_key? }` as bytes. Exactly one of the two ids (**works**). | `masternode.key_not_in_wallet{role}`, `vault_locked`, `grant_invalid` | IOS-083, IOS-082 |
| `locate_masternodes(query)` | async | List search by IP, `IP:port`, proTxHash, owner/voting/payout address, operator key. | `masternode.list_unavailable` | IOS-082 |
| `tracked_masternodes()` / `track_masternode(hash, label?)` / `untrack_masternode(hash)` / `set_tracked_masternode_label(hash, label?)` | async | platform-wallet `TrackedMasternodes` (persisted in wallet.sqlite). Untrack deletes attached keys from the vault. `TrackedMasternode { row, label, attached_roles, capabilities }`. | `masternode.already_tracked`, `not_found` | IOS-082 |
| `attach_masternode_key(hash, role, key, grant)` / `detach_masternode_key(hash, role)` | async | WIF/hex (secp256k1), hex (BLS), hex/base64 (ed25519); must match the registered key for the role. Stored in the vault under `(network, proTxHash, role)`; `MasternodeOp` grant. The key bytes are wiped when the call returns. | `masternode.invalid_key{role}`, `vault_locked`, `grant_invalid` | IOS-082 |
| `evonode_status(hash)` | async | `EvonodePlatformStatus { claimable_credits, epoch_proposed_blocks, epoch_index }` (Platform queries). **May be M4**: `NotImplemented{…evonode_status.platform}`. | `masternode.platform_unavailable` | IOS-080, IOS-081 |
| `withdraw_evonode_credits(hash, amount, dest, grant)` | async | platform-wallet `masternode_withdraw` with the owner or payout key (wallet-derived or attached); `PayoutAddress` or any address (owner key only). Returns the state-transition id. **May be M4** (`.platform`). | `masternode.key_not_in_wallet`, `platform_unavailable`, `vault_locked`, `grant_invalid` | IOS-081, IOS-082 |

### 2.6 Network sub-tab (`network_stats.rs`) — owner R1

| Call | Kind | Semantics | Errors | Serves |
|---|---|---|---|---|
| `network_stats()` | sync | **works** (quorums: platform-wallet has no quorum-list accessor at the pin, so the list stays empty; hosts show it as unavailable). `NetworkStats { credit_pool: None, instantsend: None, masternodes, evonodes, best_chainlock, quorums[] { llmq_name, llmq_type, active, health_percent, rotated } }`. Credit pool and InstantSend counters need a full node (§7); quorums come from the synced list's quorum entries (empty before). In-memory read; re-query on `Sync`/`Masternodes`. | — (`SyncError` domain) | QT-144 |

## 3. Events (additions to `EngineEvent`)

| Event | When | Host reaction | Emitter |
|---|---|---|---|
| `CoinJoin { network, wallet_id }` | Mixing state, sessions, status code, balances or progress of the wallet changed. ≤ 1 Hz per wallet (dash-qt's 1 s panel timer), trailing edge kept. | re-query `coinjoin_status` | R1 |
| `Governance { network }` | Sync phase/counters changed, objects or votes arrived, a pending proposal's collateral changed. ≤ 1 Hz, trailing edge kept. | re-query what is shown | R2 |
| `Masternodes { network }` | List changed, owned detection or tracked masternodes changed. ≤ every 3 s, every 30 s while SPV is not caught up (dash-qt). | re-query list/state | R3 |

All three are re-query signals (DashKit: not lifecycle; `EventBus` may coalesce them). `HistoryChanged`/`Balances`
still fire for the transactions mixing, proposals and ProTx create.

## 4. Error codes

Common codes unchanged (m1-engine.md §4). Domain codes (a dw-ffi test compares these rows with the enums; a Swift
test compares them with `ServiceErrorCode.m3EngineCodes`):

| Domain enum | Codes |
|---|---|
| `CoinJoinError` | `coinjoin.disabled`, `coinjoin.watch_only`, `coinjoin.insufficient_funds`, `coinjoin.vault_locked`, `coinjoin.grant_invalid`, `coinjoin.nothing_to_move`, `coinjoin.spv_not_running`, `coinjoin.no_peers`, `coinjoin.broadcast_rejected` |
| `GovernanceError` | `governance.sync_disabled`, `governance.not_synced`, `governance.proposal_not_found`, `governance.invalid_proposal`, `governance.no_voting_keys`, `governance.vote_too_often`, `governance.insufficient_funds`, `governance.collateral_unconfirmed`, `governance.proposal_expired`, `governance.watch_only`, `governance.vault_locked`, `governance.grant_invalid`, `governance.no_peers`, `governance.broadcast_rejected` |
| `MasternodeError` | `masternode.list_unavailable`, `masternode.not_found`, `masternode.key_not_in_wallet`, `masternode.invalid_service`, `masternode.invalid_key`, `masternode.invalid_payout`, `masternode.duplicate_address`, `masternode.collateral_unavailable`, `masternode.insufficient_funds`, `masternode.operator_secret_mismatch`, `masternode.operator_secret_unconfirmed`, `masternode.collateral_signature_invalid`, `masternode.unsupported_entry`, `masternode.watch_only`, `masternode.vault_locked`, `masternode.grant_invalid`, `masternode.no_peers`, `masternode.broadcast_rejected`, `masternode.shared_envelope_invalid`, `masternode.shared_envelope_too_large`, `masternode.shared_network_mismatch`, `masternode.shared_session_not_found`, `masternode.shared_inputs_refused`, `masternode.shared_coin_spent`, `masternode.already_tracked`, `masternode.platform_unavailable` |

`network_stats` uses `SyncError` and adds no code. Parameters the UI shows (review M-5 rule), forwarded by DashKit in
`ServiceError.parameters`: `min_duffs`, `needed`, `available`, `confirmations`, `retry_after_secs`, `size_bytes`,
`field` (`ProposalField` index), `role` (`MasternodeKeyRole` index), `refusal` (`CollateralRefusal` index).
`vote_too_often` in a `VoteResult` carries no parameters; the host reads `VotingMasternode.next_vote_at`.

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
- **`NodeInfo.masternodes/evonodes`** keep their M2 meaning; `masternode_list_state` adds heights and Evo splits.
- **Console (R1/R2/R3).** The commands listed under "CoinJoin", "Governance", "Masternode" and "Evo" in
  `dw-console`'s table become available as each domain lands (Core result shapes, the redaction list unchanged).

## 6. Crates

| Crate | Owner | Now | To build |
|---|---|---|---|
| `dw-p2p` | R1 | `commands` (wire names verified against Core `protocol.cpp`) | `codec`, `session` (handshake incl. `senddsq`, ping, rate limit), `peers` (from the SPV list), `proxy` (after U1) |
| `dw-coinjoin` | R1 | `denoms`, `settings` (ranges, defaults, validation), `status` (`PoolState`, `PoolMessage` wire order, status codes) | `rounds`, `progress`, `planner`, `messages`, `queue`, `session`, `client`, `recovery` |
| `dw-governance` | R2 | `params` (per network) | `object`, `vote`, `sync`, `tally`, `clock`, `proposal` |
| `dw-protx` | R3 | `params` (ports, collaterals, share limits) | `register`, `update_service`, `update_registrar`, `revoke`, `bls`, `keychain`, `shared::{payloads, envelope, session, dissolve}` |

`dw-p2p` public API R2 relies on (R1 delivers it first, with an in-process mock peer for tests):
`Session::connect(addr, network, SessionConfig) -> Result<Session>`, `Session::send(command, payload)`,
`Session::subscribe(&[command]) -> Receiver<(command, payload)>`, `Session::close()`, and a `PeerPicker` over a
masternode-list snapshot (`masternodes()`, `full_nodes()`, a "recently used" ring). The masternode list snapshot
comes from the engine (dash-spv's `masternode_list_engine`, exposed through platform-wallet's SPV runtime as M2's
`count_masternodes` already reads it).

## 7. Full-node-only features (DESIGN-opus §1.14)

| Feature | M3 answer |
|---|---|
| PoSe score, ban/revive heights, last paid, next payment, consecutive payments | `None`; the host shows "—" with "Requires full-node data source". |
| Owner/payout/collateral of masternodes the wallets never saw | `None` (no ProRegTx replay of foreign masternodes from SPV). |
| Mempool count/usage, credit pool, InstantSend counters | `None` (`NodeInfo`, `NetworkStats`). |
| Governance objects, votes, tallies, funded state | SPV-native over govsync (R2); every value comes from synced objects or is `None`. If G7 fails, tallies are shown only with the dashd data source (post-M3) and voting/creation still work. |
| CoinJoin own-collateral check | Approximated: our collateral is valid when confirmed, the right amount and unspent in our view (Core checks its mempool). |

## 8. Decisions and open points

- **IOS-058: mixing is IN** (DESIGN-opus §7.1). If G6 fails, mixing ships as "Experimental" (off by default);
  balance and "move mixed coins" stay.
- **QT-048: the automatic-backup gate is not copied** (DESIGN-opus §7.5, research 02 §21 quirk 10). Our wallets are
  HD with no keypool to exhaust, so `keys_left` is `None` and no "low keys" warning fires; the only "disabled"
  states are `CoinJoinUnavailable::{Disabled, WatchOnly, InsufficientFunds}`. The parity row records this as a
  deliberate deviation.
- **QT-124 order.** Like dash-qt (`RegisterMasternodeWizard::startRegistration`), the secret gate comes before any
  broadcast; the engine enforces it.
- **QT-132 payment date.** The chosen superblock is honoured (dash-qt ignores it).
- **CoinJoin output chain.** DIP9 CoinJoin account (DESIGN.md R2, overriding Fable's BIP44 default).
- **Evonode Platform calls** may move to M4 with an honest `.platform` `NotImplemented`; the Core-side unban is M3.
- **G6/G7** gates: §9 and DESIGN-opus §6.

## 9. G7 — mainnet governance sync measurement (R2)

1. `dwcli gov-sync --network mainnet --peers 3` (R2 adds the subcommand) runs govsync from a fresh data dir
   against 3 peers picked from the synced list, logging per phase: objects, votes, bytes received, wall time,
   peak RSS.
2. Run it three times on the dev Mac over the home connection (≈ 50 Mbit) and once on the Mac Studio; record the
   results in `docs/research/g7-govsync.md` (R2 creates it).
3. Compare tallies of every current-cycle proposal with a reference node's `gobject list valid proposals`
   (mainnet dashd on the build server, read over RPC by a script, never shipped).
4. Pass: ≤ 5 min on 50 Mbit and tallies within 1 % of the reference (DESIGN-opus G7). Fail: tallies only with the
   dashd data source; the list shows "needs a Dash Core node" for tallies; voting and creation stay SPV.
