# MG-03: the `Masternodes` facade

ROADMAP MG-03 (release 1.1, size M, engine, tiers T1 and T2, stack-independent): the `Masternodes` facade and its
contract. It covers:
- my masternodes: those whose collateral, owner, voting or payout key is in the wallet;
- tracking any masternode and attaching keys (IOS-082, the library's tracked masternodes);
- status from the SPV list;
- payouts attributed in history;
- evonode status and claimable credits (IOS-080).

The contract is [`docs/contracts/mn-engine.md`](../contracts/mn-engine.md).

Acceptance:
- a regtest masternode funded from the wallet shows its status and payouts;
- an evonode on T2 (testnet) shows claimable credits.

Inputs:
- MG-01's crates and its §7 hand-over ([`MG-01-salvage.md`](MG-01-salvage.md));
- MG-00's UX rows ([`MG-00-masternodes-ux.md`](MG-00-masternodes-ux.md) D1, D4, D5, D9, D10, D13, D22, D28, D42, D43,
  D46, and the MG-03 follow-up);
- UX-SPEC §4.25.2;
- the pinned platform-wallet (`ebe37f8a`) and rust-dashcore (`d3d50520a`);
- Dash Core v24 (`~/workspace/dash`, `src/evo/{deterministicmns,dmnstate,dmn_types}.*`, `src/rpc/masternode.cpp`).

D-numbers in this file are MG-03's (D1 and up). MG-00, MG-01 and MG-02 rows are cited as "MG-00 D42" and so on.

| Part | Where |
|---|---|
| The facade, records, errors, list follower | `rust/crates/dw-engine/src/masternodes/` |
| The coalesced event | `EngineEvent::Masternodes` (`dw-engine/src/events.rs`), `AppEvent::Masternodes` (`dw-app/src/bridge.rs`) |
| Contract test | `rust/crates/dw-engine/tests/mn_masternodes_contract.rs` |
| dwcli `mn` commands | `rust/bin/dwcli/src/masternodes.rs` |
| Regtest suite | `regtest/functional/dwd_masternodes.py` |
| T2 (testnet) check | `dwcli --network testnet mn --spv …` runs (§11, D22) |

## 1. What each source can prove

The wallet has four sources. Each field the facade returns comes from exactly one of them, and the records say which
(MG-00's MG-03 follow-up: "report where each masternode field comes from").

| Source | Fields | Trust |
|---|---|---|
| **SPV list** (dash-spv's simplified masternode list at its tip, through platform-wallet's `SpvRuntime::masternode_list_summaries`) | ProTx hash, service address, operator public key, voting key id, type (regular or evonode), platform node id and HTTP port, valid (Core: not PoSe-banned) | Verified by dash-spv against the chain (DIP-4 merkle roots in coinbase payloads). It carries **no owner key, payout script or collateral** (MG-01 §5, MG-02 §1). |
| **Provider transactions the wallet holds** (`provider_masternode_txs_blocking`: every special transaction one of the wallet's accounts matched, with block height and in-block position) | Registration height, collateral outpoint, owner key id, payout script, operator's share, revocation; the latest ProUpRegTx's payout and keys | Each transaction was in a block of the verified chain (or the mempool, height 0). The set holds only what **matched this wallet** (§3), so a later update that doesn't name a wallet key or script can be missing (D5). |
| **Wallet history** (the engine's dash-qt records) | Payouts: coinbase outputs to a payout script | The wallet's own transactions. |
| **Platform** (dash-sdk, proved reads through the trusted context provider) | An evonode's claimable credits: the balance of its owner identity (id = ProTx hash) | A proof-checked `IdentityBalance` fetch. |

The tracked-masternode registry's `refresh` in platform-wallet also learns a ProRegTx through DAPI's
`getTransaction`. That answer has no proof and no header check, so MG-03 neither calls `refresh` nor reads its
`registration` snapshot (D6).

## 2. Status

The status mirrors Core's names where Core has them (`masternodelist status` prints `ENABLED` or `POSE_BANNED`,
`rpc/masternode.cpp:648`; a list entry is valid iff `!IsBanned()`, `deterministicmns.h:274`). Three states come from
the wallet's side, since Core has no name for them (D2):

| `MasternodeStatus` | When | UX chip (MG-00 D5) |
|---|---|---|
| `enabled` | In the SPV list, valid. | Active |
| `pose_banned` | In the SPV list, not valid. `banned_seen_at` is the wallet's first sighting (MG-00 D9), kept until the list shows it valid again. | PoSe banned |
| `removed` | Not in a synced list, and the wallet saw it listed before or holds its ProRegTx confirmed at or below the list height. Core removes a masternode when its collateral is spent; that is the only way out of the list. | Retired |
| `unconfirmed` | The wallet holds its ProRegTx but it isn't in a block yet. | Registering |
| `unknown` | The SPV list isn't synced (no list, or the headers or masternode phase is still running), or a tracked masternode was never seen listed. | Unknown |

"Synced" is the tracker's `masternodes_synced()` (headers and masternode phases done; DASHPAY §2.2). The list height
is the masternode phase's reported height (`MasternodeListState.height`). platform-wallet's summaries drop
`MasternodeList::known_height` (D3).

## 3. My masternodes and their roles

A wallet's masternodes are aggregated from its provider transactions with platform-wallet's `aggregate_masternodes`
(Core's `block.vtx` order, latest-wins keys and payout, per-type numbering), fed the transactions that
`provider_masternode_txs_blocking` returns with their heights and positions. The engine then reads each held ProRegTx
again for what the library's record leaves out (D1):
- the collateral outpoint per Core (`dw_protx::payloads::resolve_collateral`: a null payload hash names an output of
  the ProRegTx, MG-01 §7);
- that internal output's amount, which must be 1,000 or 4,000 Dash for the type, or the collateral stays unresolved
  (MG-00 D43);
- the operator's share (`operatorReward`).

The wallet's provider transactions are matched by key-wallet's router: a ProRegTx by owner, voting, operator or
platform key and by payout script, and a ProUpRegTx by voting or operator key and payout script
(`transaction_router/mod.rs:111+`). So a masternode registered with a wallet key, or paying a wallet address, shows
up after a sync or rescan.

Roles (`MasternodeRoles`), each answering "is this in **this** wallet":

| Role | Test |
|---|---|
| `collateral` | The resolved collateral outpoint is an unspent output of the wallet. |
| `owner` | The owner key id is the hash160 of one of the wallet's derived owner keys (indexes 0–19, as MG-02's voting keys), or an owner key is attached to it (tracked). |
| `voting` | The **list's** voting key id (always live, MG-00 D42) is one of the wallet's derived voting keys (0–19), or a voting key is attached. |
| `payout` | The payout script is one of the wallet's own scripts. |
| `operator` | The operator public key is one of the wallet's derived operator keys (platform-wallet's derive-and-compare, a self-operator). Shown, never used to sign (MG-01 §4). |

A masternode is "mine" for a wallet when any role holds. A collateral held by the wallet but registered with no wallet
key and no wallet payout is not found, because key-wallet doesn't match a ProRegTx by an external collateral outpoint
it only references (D4). The regtest suite checks the owner-key path and the payout path separately (§8).

**Collateral-shaped coins** (MG-00 D1): unspent outputs of exactly 1,000 or 4,000 Dash that are not a known
masternode's collateral are listed in `MasternodeOverview.collateral_coins`. The sidebar uses them to show the item, and
the empty page uses them for its intro.

## 4. Tracking and attached keys

Tracking uses platform-wallet's registry (`tracked_masternodes_service`), which the engine's `WalletStore` already
persists in `wallet.sqlite` (`tracked_masternodes`, V006). A tracked masternode belongs to the network, not to a
wallet: every wallet's overview lists it under `tracked` (UX §4.25.2's Tracked group).

- `find(query)` searches the SPV list by ProTx hash (64 hex) or service address (`ip:port`, or `ip` alone for any
  port; every match is returned), using platform-wallet's `find_in_summaries`. No private key is accepted: MG-00 D13
  attaches keys only after the masternode is found.
- `track(pro_tx_hash, label)` needs the masternode in the list at the time (D7). It refuses one already tracked.
- `untrack` deletes the attached keys **first**, then the registry row (MG-00 D22). If the deletion fails (for
  example, the vault is locked), nothing is untracked.
- `attach_key(pro_tx_hash, role, key)` takes a WIF or 64-hex secp256k1 secret for `owner`, `voting` or `payout`. These
  are the vault's roles; an operator or platform secret is refused by type (MG-01 D7). The check is the one the list
  allows (MG-00 D13):
  - `voting`: the key's hash160 must equal the list's voting key id, else `masternodes.key_mismatch`;
  - `owner`: the list doesn't carry the owner key, so it is stored unchecked unless the wallet holds the ProRegTx (then
    it is checked against its owner key id);
  - `payout`: checked against the payout script when the wallet holds one, otherwise stored unchecked.

  The secret is stored with `Vault::store_masternode_key`, which needs the full-scope data key (an unlocked or
  unencrypted vault). The returned `KeyCheck` says `matched` or `unchecked`. Attaching a key to an untracked masternode
  is refused (`masternodes.not_tracked`).
- `detach_key(pro_tx_hash, role)` removes one role.
- Labels: a tracked masternode's label lives in the registry. A wallet masternode's label lives in app.sqlite
  (`settings_kv`, `masternodes.label:<hash>`). The default name is the library's per-type numbering ("Masternode 1",
  "Evonode 1"), reported as `type_index`.

The four `Vault` masternode-key methods stay internal and are reached through this facade, which adds the checks above
(D8).

## 5. Payouts in history

A payout is a wallet history record of a coinbase transaction (`TxType::Generated`) whose address is the masternode's
payout address. Only the owner's payout is attributed: the operator's payout address is set by a ProUpServTx, which
the wallet sees only when it pays a wallet script (D9). `payouts(wallet, pro_tx_hash, since)` returns these records,
newest first, as `MasternodePayout` (txid, record index, amount, height, time, matured). Each overview row carries
the total of the last 30 days (D10's "Payouts, 30 days") and the newest payout.

Matching by script is MG-00 D10's rule. The engine doesn't recompute Core's payee for the block: an SPV wallet can't
(no full list per height). Two masternodes with one payout address share their payouts, and both rows list them (D9).

## 6. Evonode credits

`evonode_credits(pro_tx_hash)` fetches the balance of the evonode's owner identity, whose id is the ProTx hash in
display order (platform-wallet `tracked.rs` `learn`). It uses dash-sdk's proved `IdentityBalance` fetch. The answer is
`EvonodeCredits { pro_tx_hash, credits, duffs, fetched_at }`. One duff is 1,000 credits (dpp's
`CREDITS_PER_DUFF`).

Refusals, in order:
- `invalid_argument` for a malformed hash;
- `masternodes.list_unavailable` while the list isn't synced;
- `masternodes.not_found` when the hash isn't in the list;
- `masternodes.not_evonode` for a regular masternode;
- `masternodes.platform_unavailable` when the session runs without Platform or DAPI fails;
- `masternodes.no_identity` when Platform has no identity for it (an evonode registered before Platform).

Platform node status (IOS-081's "Platform status") and the withdrawal are MG-13's.

## 7. What waits for an upstream API

Two MG-00 rows need data that dash-spv and platform-wallet don't expose at the pin. MG-03 reports the gap and doesn't
work around it:

| Row | Needs | At the pin | MG-03 |
|---|---|---|---|
| MG-00 D43 (backfill a tracked masternode's collateral: the block at `confirmedHash`'s height − `nMasternodeMinimumConfirmations`, checked against the verified header) | a header lookup by hash and a block download from dash-spv | `DashSpvClient::storage()` is public in dash-spv, but platform-wallet's `SpvRuntime` keeps the client private and exposes neither. The list summaries also drop `confirmedHash`. | `resolve_collateral` returns `masternodes.unsupported{feature: "collateral_lookup"}`. A tracked masternode's `collateral` is `CollateralState::Unresolved` (D11). |
| MG-00 D42 (watch every known ProTx hash through compact filters from the registration height, so updates that don't name a wallet key are applied) | a filter watch on raw elements | the SPV filter matcher takes the wallet's scripts and outpoints only | Payout and payees come from the wallet's own provider transactions. `payout_checked_height` is always `None` ("checking", MG-00 D42), so the UI never offers a fixed withdrawal destination yet (D5). |

| MG-00 D1 and D28 for a v24 shared masternode (Core `src/evo/providertx.h`: version 3 replaces `scriptPayout` with `payouts`, and a shared registration has `shares` with per-share refund and reward scripts; ProUpShareTx and ProUpSharedRegTx update them) | rust-dashcore types for the version-3 payload and transaction types 10–12 | the pinned rust-dashcore (`d3d50520a`, PastaPastaPasta/rust-dashcore-dashpay, which platform `ebe37f8a` pins) reads every ProRegTx/ProUpRegTx with the version-2 layout, and refuses Core's v24 version-3 bytes outright | The masternode is `shared_unsupported`: its payouts are left out with `payouts_incomplete` set, and it stays in Mine when a provider transaction of it is in the wallet (D25). The share model is MG-03b. |

All three are follow-ups (§10). Nothing in MG-03 depends on them for its acceptance.

## 8. The event and the follower

`EngineEvent::Masternodes { network }` means "re-read what you show". It fires at most once a second, keeping the
trailing edge (MG-02's coalescer). It is sent:
- when the list's entry for a masternode of interest changes (status, service, voting key or type);
- when a masternode of interest appears in or leaves the list;
- when the tracked set, a label or an attached key changes;
- when the wallets' set of masternodes changes.

A masternode is of interest when any loaded wallet has it or it is tracked.

The follower runs for the session's life. On each new masternode-list height (the tracker's reading, polled every 5
s) it reads the summaries once, refreshes the interest set, and writes the first sighting of each newly banned
masternode of interest (`masternodes.banned_seen:<hash>`). It deletes that record once the masternode is valid again
(MG-00 D9: "the notice stays until the list shows it valid"). It also writes `masternodes.listed_seen:<hash>`, so
`removed` can be told from "never seen" (§2). The event is engine-side for dw-ffi (the frozen Swift shells don't carry
it, MG-02 D7), and dw-app maps it to `AppEvent::Masternodes`.

## 9. dwcli `mn`

`dwcli mn <cmd>` drives the facade only. Each command prints one JSON line like `dashpay`:
`{"ok":true,"result":…}` or `{"ok":false,"error":{"code","message"}}` with exit status 1. Every failure is a line,
not only the facade's (D26): the vault's unlock (m1 `vault`, e.g. a wrong `--passphrase-file`), a setup step before
the command (`setup`: the passphrase file, the data root, opening the network), a refused command line
(`invalid_argument`, naming no value; exit status 2) and a panic (`internal`). No line or stderr message repeats an
argument (D23).

| Command | Facade call |
|---|---|
| `mn list-state` | `list_state` |
| `mn list [--wallet W]` | `list` |
| `mn status <hash> [--wallet W]` | `masternode` |
| `mn find <query>` | `find` |
| `mn track <hash> [--label L]` | `track` |
| `mn untrack <hash>` | `untrack` |
| `mn label <hash> [--label L]` | `set_label` |
| `mn attach <hash> --role R` (key on stdin, never argv) | `attach_key` |
| `mn detach <hash> --role R` | `detach_key` |
| `mn payouts <hash> [--wallet W] [--since T]` | `payouts` |
| `mn credits <hash>` | `evonode_credits` |

`--spv` starts SPV and waits for the masternode list (`--sync-timeout`, default 120 s) before the read. Without
`--spv`, a list read answers from what the session has, which is `list_unavailable` on a cold start.
`--sync-height H` (implies `--spv`) also waits until every loaded wallet has scanned height `H`, so a test reads
after the block it just mined (D18).

## 10. Slices and follow-ups

| Slice | Content |
|---|---|
| 1 | This doc, the contract, the facade skeleton with every call (stubs where not yet implemented), errors, records, `EngineEvent::Masternodes`, the contract test; `list` and `masternode` for wallet masternodes with status and roles; `dwcli mn list/status`. |
| 2 | Tracking: `find`, `track`, `untrack`, `set_label`, `attach_key`, `detach_key`, the follower (first-banned, listed-seen, the event); `dwcli mn find/track/untrack/label/attach/detach`. |
| 3 | Payouts and evonode credits; `dwcli mn payouts/credits`; the T2 check. |
| 4 | The regtest suite (§11). |

As built, slices 1–3 landed as one engine commit and one dwcli commit, with every call implemented (no stubs left;
`resolve_collateral` is the D11 refusal). Slice 4 added `list_state` (D17), `--sync-height` (D18), and the history
fix that the restart in the suite exposed (D15).

Follow-ups for later rows:
- **MG-03b, shared masternodes (D25):** the share table, per-share payouts and payout attribution, and
  ProUpShareTx / ProUpSharedRegTx in the aggregation. Depends on rust-dashcore exposing the v24 version-3 payload
  (the types D25 names) at the pin platform-wallet uses. That bump is also a wallet-wide sync blocker wherever
  version 3 is active (D25), so it should come before MG-03b's own work.
- **D43/D42 upstream:** a platform-wallet `SpvRuntime` accessor for header-by-hash and a block fetch (dash-spv has
  both internally), and a raw-element filter watch. MG-03b, or MG-13 before the fixed withdrawal destination.
- **MG-10:** the Activity filter by masternode. It uses `payouts()` txids, or a `HistoryFilter` extension (D10).
- **MG-04:** `Registering`/`Needs signature` for the wallet's own pending registrations, which MG-04 persists.
- **E0-13 (MG binding):** bind the facade for the UI.
- **MG-12:** review the attached-key flow (key deletion on untrack).

## 11. Tests and evidence

- T0:
  - the contract test (generated surface, call rows, codes, and the refusal order on a session without a list);
  - unit tests of aggregation (collateral resolution, internal-collateral amount check, operator share,
    latest-wins, a version-3 ProRegTx/ProUpRegTx flagged and Core v24 version-3 bytes refused by the pin), a shared
    row kept in Mine and out of the payout totals (and a version-3 list entry), status mapping, the follower's banned
    and listed bookkeeping, key checks, an attached key's id across an app.sqlite reopen with a rotated voting key
    reading `key_mismatch`, a voting key holding only when `matched` (D29), a remembered version-3 entry, a shared
    masternode with no payout reference, coinbase income no row explains flagging the total once the list had a
    version-3 entry, a version-2 row's payouts unattributed once any version-3 entry was seen, now or remembered
    (D30), every wallet-set event bumping the follower's generation (D28), and the parser of `find`;
  - an attached key whose vault write fails at the read-back or the directory fsync (dw-vault `fault-inject`) records
    no id, and after a restart reads the id of the key the vault holds (D31); a power loss after replacing a key, with
    each barrier skipped in turn (dw-appdb `durability-sim`), never leaves the old id holding the role (D34); the
    version-3 markers survive power loss (D35); an uncompressed WIF is refused (D36);
    a failing address of a comma-separated `--dapi` is redacted in the SDK's own log line (D37);
  - every facade call given a secret-shaped 64 hex (as hash, wallet id and key) quotes it in no error (D23); a label
    for an unknown hash is refused (D27); an import and a removal each send the event at the same list height (D28);
  - `dwcli/tests/mn_cli.rs`: every `mn` subcommand with the secret-shaped value prints one JSON line and neither
    stdout nor stderr holds it; a wrong passphrase file, a missing one, a refused command line and a bare `mn` are
    JSON lines too, and a value `mn` elsewhere doesn't make a line (D26); every global option given the secret
    (`--peer`, `--ca-cert`, `--datadir`, `--passphrase-file`, `--dapi`, `--quorum-url`, `--network`,
    `--initial-protocol-version`, `--shutdown-timeout`, and the `--sync-*` options; `--dapi` and `--quorum-url` also as
    URLs with Platform on) prints one line, fixed text for a setup failure, and neither stdout nor the stderr log
    (`DWCLI_LOG=info`) holds the value; and with `DWCLI_FAULT_INJECT=probe-spawn-fail` the teardown fails before the line, which is then
    `shutdown`, exit 1 (D32).
- T1, `regtest/functional/dwd_masternodes.py`, on Core's functional framework (dashd v24.0.0-rc.2): a controller, which
  is dwcli's SPV peer, and the framework's 2 static masternodes. DKG, InstantSend and ChainLocks stay off, so the
  masternodes the test registers run no node and are never PoSe-punished. The wallet's DIP3 keys are derived in Python
  from the mnemonic, an oracle independent of the engine. Every `dwcli mn` call is a fresh process, so each read also
  covers reopening the wallet (D15). Before dwcli syncs, the test mines whole DIP24 cycles until Core answers
  `quorum rotationinfo` (D16).
  1. a dwcli wallet receives 1,000 Dash, listed as a collateral-shaped coin;
  2. the controller registers A with that output as external collateral, signed by dwcli's `sign`
     (`protx register_prepare`, then `register_submit`), with the wallet's owner and voting keys 0 and a wallet payout.
     B uses Core's collateral and the wallet's owner key 1 (the owner-key match, §3). C uses Core's keys and a wallet
     payout (the payout match). `dwcli mn list` shows all three `enabled` with the expected roles, collateral and
     registration facts; the coin is no longer offered;
  3. tracking: `find` a static masternode by service address, `track`, `already_tracked`, `not_found`, `not_tracked`,
     labels, C's voting and owner keys attached (`matched`) and a wrong key refused (`key_mismatch`), `untrack`
     deleting the keys; on the synced list, every `mn` subcommand given a secret-shaped 64 hex quotes it nowhere
     (D23); a ProUpRegTx (`protx update_registrar`) rotates D's voting key and the next read, a fresh process, reports
     the attached key `key_mismatch` with `voting: false` (D24), and a read without SPV `unchecked`, still
     `voting: false` (D29);
  4. after blocks are mined, `mn payouts` of A and C equal Core's coinbase payments to their payout addresses, newest
     first, and mature after 100 blocks;
  5. `protx revoke` of A makes it `pose_banned` (Core bans a revoked masternode); the wallet spending the collateral
     makes it `removed`.
- T2 (testnet, reads only, concurrency 1), run with dwcli on a fresh data dir (D22): `mn --spv list-state`, then
  `mn --spv find <ip>` with an address from the testnet DNS seed until an evonode is listed there, then
  `mn --spv credits <hash>`. Recorded in the MG-03 report (2026-10-16: list height 1,573,854; evonode
  `9cb04f27…145c` at 68.67.122.1:19999, 1,143,994,435,125,307 credits).

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Aggregate with the library's `aggregate_masternodes` over `provider_masternode_txs_blocking`'s transactions, then re-read each held ProRegTx for the collateral (resolved and amount-checked) and the operator's share (`masternodes/aggregate.rs`). The record's own `collateral` and `status` are not used. | The library's `MasternodeRecord.collateral` copies the payload outpoint, which is null for internal collateral (MG-01 §5 "dw-protx" row), and it drops the operator's share; its `status` needs a list the engine reads itself (§2). Reusing the aggregation keeps the library's ordering, latest-wins rules and numbering instead of a second copy of them. |
| D2 | Status names: `enabled` and `pose_banned` mirror Core (`ENABLED`, `POSE_BANNED`). `removed`, `unconfirmed` and `unknown` are the wallet's own. | Core names only list members. A removed masternode is gone from Core's list with no state, and an unconfirmed registration is not in it yet. The UX maps them to Retired, Registering and Unknown (MG-00 D5). |
| D3 | The list height is the tracker's masternode-phase height, not `MasternodeList::known_height`. | platform-wallet's summaries drop `known_height`, and the SPV client isn't reachable from the engine. The phase height is the height dash-spv reports the list synced to. The tracker already keeps it. |
| D4 | A collateral-only masternode (external collateral in this wallet, no wallet key or payout) isn't found. | key-wallet's router matches a ProRegTx by keys, payout and spent or created outputs, not by a referenced outpoint. Matching every ProRegTx's outpoint against the UTXO set needs every ProRegTx, which BIP158 filters don't deliver for an outpoint the wallet doesn't spend. The UX case (a holder registering with keys elsewhere) is rare. MG-04's registrations always use a wallet owner key (MG-00 D25). |
| D5 | Payout and payees come from the wallet's own provider transactions. `payout_checked_height` is `None` until MG-00 D42's ProTx-hash watch exists. | A filter watch on raw ProTx hashes is not in the pinned dash-spv. Reporting "checking" is MG-00 D42's honest state, and no withdrawal destination is offered from it (MG-13 is out of scope here). |
| D6 | Do not call the registry's `refresh` or use its `registration` snapshot. | It fetches the ProRegTx through DAPI `getTransaction`, unproved and not header-checked. MG-00 D43 rejected that path for collateral. The owner-identity balance it also reads comes from a proved fetch, which `evonode_credits` does directly. |
| D7 | `track` needs the masternode in the current list. | It is how `find` located it, and it means the voting key check has a list entry to compare against. A ProTx hash that isn't listed (removed, or not registered yet) has nothing to show but `unknown`. |
| D8 | The vault's four masternode-key methods stay unbound. Their dw-app `EXCLUDED` reasons now say they are reached through `Masternodes` (`attach_key`, `detach_key`, `untrack`, `masternode`). | MG-01 §7 asked MG-03 to bind them. Binding them as IPC would let a host store a voting key without the list check (MG-00 D13), or delete keys without untracking. The facade calls them after its checks. |
| D9 | Attribute only the owner's payouts, by payout script. A shared payout address attributes the same records to each masternode that uses it. | The operator payout script is set by a ProUpServTx the wallet sees only when it pays a wallet script. An SPV wallet can't recompute Core's payee per block. MG-00 D10 defines attribution as "this wallet's history". |
| D10 | Payouts are returned by reference (txid, record index) from a facade call, rather than adding a masternode field to `TxRecord`. | `TxRecord` is shared by every history consumer (dw-app DTO, dw-ffi, TS types, Swift T0). The UI's filter by masternode (MG-10) can take the txids. A `HistoryFilter` field can come with MG-10 if it needs one. |
| D11 | `resolve_collateral` exists in the contract and refuses `masternodes.unsupported{feature}`. | The brief: a missing upstream API is a typed refusal, not an invented workaround. Voting for a tracked masternode (MG-02/MG-05) needs the outpoint. The call fixes where it will come from. |
| D12 | Labels of wallet masternodes and the follower's first-banned and listed-seen times live in app.sqlite's `settings_kv` (global scope, `masternodes.` keys), not a new table. They are not in `.dwbackup` exports. | The set is small and per network (app.sqlite is per network). Siblings in flight add migrations, and refinery refuses divergent version histories, so a migration here risks a merge-time clash for no gain. A first-banned date after a restore is a new first sighting, as MG-00 D9 defines it. |
| D13 | The facade is `NetworkSession::masternodes()`, one handle per session, with the wallet passed per call. | Tracked masternodes belong to the network (platform-wallet's registry is per network), and one overview mixes them with each wallet's own. It matches MG-02's `governance()`. |
| D14 | The contract is its own file, `mn-engine.md`, with its own generated surface and test, instead of a section of MG-02's `mg-engine.md`. | The brief, and MG-02 is in flight with that file. The two tests are near copies; one shared harness is a follow-up once both have merged. |
| D15 | `wallet_masternodes` adds the provider transactions of the session's history store to the library's before aggregating (`aggregate::add_history`). | platform-wallet's `provider_masternode_txs_blocking` reads only the records it holds in memory, and its storage doesn't replay persisted records at open (`INTENTIONAL(tx-record-rehydration-gap)` in platform-wallet-storage). After a restart the wallet's own masternodes vanished until a rescan. The engine's history store already loads the persisted records at open (`load_history`). Upstream: rehydrate them, or take them as an argument. |
| D16 | The regtest suite mines whole DIP24 cycles (`llmq_test_dip0024`, 24 blocks) until Core answers `quorum rotationinfo` at the tip, instead of running DKG. | dash-spv's list sync starts with QRInfo, and Core answers it only with a stored quorum snapshot for each cycle it covers. Core stores one per cycle when it connects the cycle's block, without DKG, once there are enabled masternodes. With DKG on, the test's node-less masternodes would be PoSe-punished. |
| D17 | `list_state()` reports whether the SPV list is synced and its height, without a wallet. | dwcli and the UI need to wait for the list without picking a wallet, and the refusal `list_unavailable` alone can't say when to retry. |
| D18 | `dwcli mn --sync-height H` waits for every loaded wallet's scan height as well as the list. | A synced list doesn't mean the wallet has scanned the block the test just mined, so a read could miss a transaction it just made. Scan heights are already in the session's wallet state. |
| D19 | An attached payout key is `matched` when it pays the wallet's P2PKH payout script, otherwise `unchecked`; it is never refused. A non-P2PKH payout script isn't a reference. | The wallet's payout script is latest-wins over the provider transactions it holds, and a ProUpRegTx that names no wallet key or script isn't held (D5), so the real current payout key could be refused. Owner and voting checks stay strict: the owner key id is the ProRegTx's or a held ProUpRegTx's, and the voting key id is the live list's. |
| D20 | Tracking changes run one at a time per session (a `tokio::sync::Mutex` in the session's masternodes runtime, held across each `track`, `untrack`, `set_label`, `attach_key` and `detach_key`). | Each call takes the operation guard more than once (the registry, then the vault), so an `attach_key` racing an `untrack` could store a key after the keys were deleted, leaving a secret no call can reach (MG-00 D22). |
| D21 | A row is `mine` only when the wallet holds a role through its own provider transactions; a provider transaction that gives it none makes no row. | key-wallet's router also matches a ProTx by an input the wallet spent (its fee), so the library's set can hold masternodes of others. "Mine" is any role (§3), and an attached key is the user's claim, not the wallet's. |
| D22 | The T2 check is a recorded dwcli run, not an ignored engine test. | dwcli drives only the facade, as a test would, and a run on a fresh data dir includes the SPV sync a test would need anyway. An ignored test that needs testnet and minutes of sync wouldn't run in CI either. |
| D23 | No error message or code payload carries a hash, key or wallet id the caller passed: `not_found`, `already_tracked`, `not_tracked`, `not_evonode` and `no_identity` are bare codes with fixed text, a read's `wallet_not_found` names "the requested wallet", and the SDK's text of a failed credits read goes to the log only. dwcli's lines and stderr follow. | A private key pasted where a ProTx hash goes is valid 64 hex, so the malformed-input guard doesn't stop it, and an echo puts it in logs and terminals (Sol r1 Major 1). The caller has the hash it passed; the code is enough for the UI's copy. |
| D24 | An attached key's hash160 (of its public key; the voting key id for `voting`) is recorded in app.sqlite (`masternodes.key:<hash>:<role>`): the old id is cleared before the vault write, the new one recorded after it succeeds (none after a failed write, D31), and deleted with the key (`detach_key`, `untrack`). Every read compares it again: `voting` with the synced list, `owner` with a loaded wallet's ProRegTx, `payout` with its P2PKH payout script (D19), as `AttachedKey { role, key_id, check }`; `KeyCheck` gains `key_mismatch`. A key holds its role only with a `key_id` and no mismatch (a voting key only when `matched`, D29). | MG-00 D13 asks for a live comparison. A role name alone kept reporting `voting: true` after a ProUpRegTx rotated the key, or after a masternode attached `unchecked` was listed with another key (Sol r1 Major 2). The id isn't secret, so app.sqlite holds it; the vault stays the authority for whether a key is attached. Clearing first and recording last means an interruption leaves at most a key without an id, which holds no role, or an id without a key, which no read uses; never an id naming a key the vault doesn't hold (validator r1b: recording first made a re-attach that died before the vault write read the old key as `matched`). If only the last write fails, `attach_key` fails with `storage` although the key is stored; it holds no role until attached again. Keeping the id inside the vault record would tie them, but the vault is a separate store with its own format (MG-01). |
| D25 | A masternode is `shared_unsupported` when a version-3 ProRegTx/ProUpRegTx is among the wallet's provider transactions or its list entry is version 3 (extended network info). Its record's keys, payout, service and operator share aren't used (only type, mode and collateral read right), it is left out of `payouts()` (`unsupported{feature: "shared_masternodes"}`), its row's payouts and the overview total (`payouts_incomplete`), and it stays in Mine when a provider transaction of it is in the wallet. The share model is split out as **MG-03b** (share table, per-share payouts, ProUpShareTx/ProUpSharedRegTx), which depends on rust-dashcore exposing the v24 version-3 payload. Upstream dashpay/rust-dashcore `dev` (d35a8d6c0) has it: `provider_registration::{MasternodePayoutShare, CollateralShare, SharedCollateral}` with `ProviderRegistrationPayload::{payouts, shared_collateral}` (`SharedCollateral { shares, join_sigs, early_period_blocks, early_penalty }`, #1152, d775b1bf8), `ProviderUpdateRegistrarPayload::payouts` (#1138, 0d215f3f4), and `provider_update_share::ProviderUpdateSharePayload`, `provider_update_shared_registrar::ProviderUpdateSharedRegistrarPayload` with ProDisTx (#1156, 2102cacf2). The node-program branches carry the same types in their `dash` crate (5b16efb7f; `node/integration` 9b75db6b3, EV1-1 `node/ev1-1-netinfo` 61785a452, EV2b `node/ev2b-dml-apply` ebab193e8). The pinned fork (`d3d50520a`) has none. | Core v24 `src/evo/providertx.h` serialises `payouts` and `shares` where version 2 has `scriptPayout` (Sol r1 Major 3). Misread fields would name the wrong payout and the wrong owner, so the row fails visibly instead. The pin refuses Core's version-3 bytes (a unit test feeds them; a ProUpRegTx of version 3 almost always fails too, the payload length isn't checked), so on real chain data the flag comes from the list's version-3 entry (D29); the history flag covers a payload the pin happens to decode. A raw scan of share outputs can't run on transactions the pin can't decode; the scan that can is of the wallet's coinbase outputs, D29. A version-3 list entry also flags a version-2 registration, since a version-3 ProUpRegTx the wallet doesn't hold may have shared its payout. Wider than MG-03: dash-spv at the pin drops a network message it can't decode (`dash-spv/src/network/peer.rs`, decode error → resync), so a block holding a version-3 provider transaction likely can't be fetched; wherever v24's version 3 is active, the pin bump is a wallet-wide blocker, not only MG-03b's. An external collateral of a version-3 masternode isn't seen: the list summary carries no collateral outpoint (as D4). |
| D26 | Every `dwcli mn` failure is one `{ok:false,error:{code,message}}` line: the facade's, the vault's unlock (m1 `vault`, generic), a setup step (`setup`), a refused command line or a bare `dwcli mn` (`invalid_argument`, when clap's partial parse names the `mn` subcommand), a panic (`internal`, payload withheld). A line is printed once. A setup step's message is fixed text naming the option, never its value (D32). | A client of the one-line protocol couldn't parse a wrong `--passphrase-file`, which only reached stderr (Sol r1 Major 4). The same shape as DashPay's (E0-09), with mn's own once-flag. |
| D27 | `set_label` refuses a hash that isn't tracked, a loaded wallet's, or in the synced list: `not_found`, or `list_unavailable` while the list that might hold it isn't synced. | Labels of any 64 hex grew app.sqlite without bound, and every overview and follower pass loads them all (Sol r1 Low 5). |
| D28 | A change of the wallet set (`WalletCreated`, `WalletRemoved`, `WalletLoadChanged`) bumps the follower's generation and sends the event, through a wrapper of the session's sink (every wallet-set emitter uses `session.sink`, the wrapped one; the hub and the coalescer keep the raw sink and emit none of these). | Which masternodes are "mine" changes with the wallets while the list height stays put, and the follower re-read only on a new height or a local change (Sol r1 Low 6). One wrapper hears every create, import, restore, load, unload and removal path, including later ones, without a call at each. |
| D29 | (Validator r1b.) An attached `voting` key holds only when `matched`; `unchecked` (the list not synced, or the masternode not listed) holds nothing, while `owner` and `payout` hold unchecked as before. A version-3 list entry is remembered (`masternodes.shared_seen:<hash>`, written by the follower, never cleared), so a row stays `shared_unsupported` before the list syncs. A shared masternode's payout is no key reference. A version-3 list entry alone doesn't make a row `mine`: the wallet's own record still has to name a role, its payout counting even though it is hidden. Once the list had a version-3 entry, a coinbase output to the wallet in the 30-day window that pays no row sets `payouts_incomplete`: it may be a share the wallet can't attribute (widened by D30). | Without the list, an unchecked voting key read as held, the same false positive as Sol r1 Major 2 for minutes after each start (the list takes that long to sync on mainnet), while `attach_key` refuses a voting key then; the owner key never changes and the payout check is advisory (D19). Before a sync, a masternode moved to version 3 by a transaction the wallet doesn't hold showed its stale payout and counted it in the totals. A fee the wallet paid isn't a role (D21). The share recipient whose masternode the wallet can't see still gets coinbase outputs, which decode; flagging them is the visible failure Major 3 asks for. |
| D30 | (Sol r2 Major 3.) Once a version-3 masternode was in the synced list, now or before (`masternodes.any_shared_seen`, written by the follower from the whole list and never cleared, beside the per-masternode `shared_seen`), or a version-3 provider transaction is known, every coinbase output to the wallet is `unattributed` (`MasternodePayout::unattributed`): no row's `payouts_30d` and not the overview's `payouts_30d` count it, the row whose payout script it pays gets `payouts_incomplete: true`, and the overview's new `unattributed_30d` counts it with any other coinbase income no row counts. `payouts()` still lists it, flagged. The overview's `payouts_incomplete` is any row's, or unattributed income in the window once version 3 was seen. | Core v24 pays a version-3 masternode's reward to its share scripts, any of which may be a script the wallet watches, a version-2 row's payout script included, so that row's total would hold someone's share. The exact rule (only scripts some remembered version-3 entry's shares name) needs those scripts, and the pin decodes no version-3 payload: the list summary carries none, and dash-spv at the pin drops a block holding provider types 10–12 (BL-187, the desktop pin owner's). So the conservative rule Sol set for that case applies: any version-3 entry seen plus coinbase income to a watched script means incomplete. The marker is remembered, so a removed version-3 masternode's recent shares still count. Wherever v24's version 3 is live this leaves every version-2 total at 0 with the income under `unattributed_30d`; MG-03b, after the pin bump, narrows it to the scripts shares actually name. The marker is network-wide and never cleared, so one version-3 entry keeps every wallet's version-2 totals at 0 until then; and a version-3 masternode removed before this install first synced (a fresh restore) isn't remembered, so its shares in the window still count as version-2 payouts. The first sighting sends the `Masternodes` event, though no followed masternode changed (validator r2). |
| D31 | (Sol r2 Major 2.) After a failed `store_masternode_key`, no id is recorded or restored: the role reads attached, `key_id: None`, `unchecked`, holding nothing. The (hash, role) is marked unsettled for the session; a later session's read records the id of the key the vault then holds (`Vault::masternode_key_id`, hash160 of its compressed public key, full scope; it syncs the vault directory first, so the key it names is durable even after a failed directory fsync), under the tracking lock, and only then. A locked vault records nothing, and a key the vault can't read is logged and skipped rather than failing the read. Keys still unsettled in this session aren't reconciled. dw-vault gains a debug-only `fault-inject` feature (dev-dependencies only) that fails one write at the directory fsync or at the read-back. | A vault write that fails after its rename may have stored the new key: the directory fsync failing leaves the new file on disk and the old records in memory, the read-back failing leaves the new one in both. Restoring the old id then named a key the vault no longer holds as `matched`. This session's vault can't answer which key is on disk, since its memory may differ from the file; a new session reads the file. The engine test injects both failures, reads `None` twice, restarts, and reads the new key's id. |
| D32 | (Sol r2 Major 1, Low 4.) Every `dwcli mn` setup failure prints fixed text naming the option at fault, never its value: `invalid --peer value`, `--ca-cert file not readable or not a PEM certificate`, `--datadir could not be opened`, `--datadir is in use by another process`, `--passphrase-file not readable`, `invalid or missing --dapi value`, `unknown --initial-protocol-version`, and otherwise `the network could not be opened (check --dapi, --quorum-url and --peer)`; the `--spv` waits name `--sync-timeout` and `--sync-height`. The engine's message, which may quote the value, is classified and dropped: not printed, and only its error code is logged. With `DWCLI_LOG`, an `mn` command's log lines are written with every global option's value (as given and as `{:?}` quotes it, 4 characters or longer) replaced by `<redacted>`, since a library may log one (the SDK's quorum fetch logs the `--quorum-url` URL at ERROR, validator r2). clap refusals (`--network`, `--shutdown-timeout`, numbers) were already value-free (E0-09). An `mn` command's SPV stop and engine shutdown run before its line, each within the `--shutdown-timeout` deadline; if they fail the line is `shutdown` (exit 1) instead of `ok: true`, and an abandoned engine prints its line from `abandon`. | dwcli copied `NetworkSession::open`'s `InvalidArgument` (quoting `--peer`) and `InvalidConfig` (quoting the `--ca-cert` path) into the JSON line and stderr. Other dwcli commands keep the engine's detail. Printing `ok: true` and then exiting 1 after a failed teardown told a client two things. The engine's own messages stay as they are (other callers read them). The abandon message on stderr names the deadline's length, a duration and no secret. DashPay's setup line still carries the engine's detail (E0-09's, outside MG-03; a follow-up). |
| D33 | (Sol r3 Major 1.) An `mn` command's `--verbose-events` prints each event's kind only (its variant name), never its fields: they may carry a devnet name or a notice quoting `--quorum-url`. The log writer (D32) redacts every non-empty option value, with no length floor, in each form a library prints it: as given, `{:?}`-quoted, and for a URL as parsed, plus its user name, password, host, `host:port` and path apart; matched in any ASCII case (`url` lowercases a host, other printers may not) and replaced in one pass over the line, so overlapping values go together and a marker is never matched again (validator r3). The sweep runs every global option with `--verbose-events` and `DWCLI_LOG=info`, including a `--quorum-url` with user-info on mainnet (whose trusted-context provider refuses an HTTP URL in a notice) and a mixed-case one on regtest. | `StderrSink` printed `{event:?}` outside the tracing writer, so `SessionOpened { network }` showed a devnet name and a `Notice` the provider's error quoting the URL (Sol r3). Kind only is the ruling's second form and needs no redaction to be right; a short value (a devnet named `a`, `--datadir .`) now redacts every `a` or `.` in a log line, which the log's purpose (debugging) tolerates. Non-`mn` commands print events in full as before. |
| D34 | (Sol r3 Major 2.) Both barriers: (a) `attach_key` clears the old id with `AppDb::set_setting_durable` (one new dw-appdb helper on the existing `durably`: `synchronous=FULL` for the transaction, `fullfsync` on Apple platforms) before the vault write starts, so no power loss brings the old id back beside the new key; (b) once a session, at the first read with the vault unlocked, every recorded id is compared with the vault's key (`masternode_key_id`), and one naming another key is dropped: the key reads `unchecked`, holds nothing, and the next session records its vault key's id (D31). dw-appdb gains a debug-only `durability-sim` feature (engine dev-dependency only): an armed database saves its files at each durable write and `power_loss` puts them back, and `skip_barriers` makes a durable write an ordinary one. The engine test attaches B over A and loses power after the vault write: with neither barrier A's id is back beside B, holding the role (`matched` once the list names A); with (a) alone B's id is recorded; with (b) alone the id is dropped. A drop is written before the key is marked unsettled and the session verified, so a failed write leaves both to the next read; the session is verified only by a comparison that ran, not by a read with nothing to compare. A read takes the tracking lock only if no change holds it: changes take it before the session gate and reads after, so a waiting read could hang a close queued on the gate behind both; a read that finds it held leaves out the ids it hasn't compared (they hold nothing on that read) and sends `Masternodes` so the host reads again, as does a reconcile that wrote an id; an id the read couldn't compare because the vault locked meanwhile or couldn't read the key is left out too, and so is an unsettled key's id from notes loaded before its drop was written (validator r3, tested). | app.sqlite runs WAL with `synchronous=NORMAL`, so the delete of A's id wasn't synced while the vault fsyncs B's rename; recovery could restore A's id next to B, and the D31 reconcile only looked at missing ids (Sol r3). (a) closes the window; (b) also catches what (a) can't: an id restored by a backup copy, a vault restored apart, or a failed directory fsync whose rename was later lost. The comparison costs one decryption per attached key per session. |
| D35 | (Sol r3 Major 3.) The version-3 markers (`masternodes.any_shared_seen` and `masternodes.shared_seen:<hash>`) are written with `set_setting_durable` before `record_sightings` returns, so no payout read relies on a marker a power loss can take back. A test with the barrier skipped shows both lost, and with it both kept. | A lost marker after the version-3 masternode left the list silently counted its shares as version-2 payouts (Sol r3). The per-masternode marker has the same stickiness promise (D29), so it gets the same barrier; both are written once per masternode or once ever. |
| D36 | (Sol r3 Low 4.) `attach_key` refuses an uncompressed WIF with fixed text (`invalid_argument`, "an uncompressed WIF is not accepted; use its compressed WIF or 64 hex"); a compressed WIF and 64 hex are accepted, and every id is of the compressed public key. | The vault keeps the bare 32-byte secret, and D31/D34 read its id back compressed, so an uncompressed WIF's recorded id would turn into another on reconcile and lose a legitimate role (Sol r3). Masternode keys from Core's `protx` RPCs and the mobile wallets are compressed. No signature, record or code changes, so `Contract-Version` stays 3 (D33–D35 change none either). |
| D37 | (Sol r4 Major 1.) An `mn` command's log also redacts each item of a list option, split the way its reader splits it: the engine joins the `--dapi` values with commas and the SDK splits that list again (rs-dapi-client `AddressList::from_str`, `address_list.rs:608-617`), logging a failing address alone (`dapi_client.rs`, WARN "ban address {address} due to error"), so every comma item of a `--dapi` value, as given and trimmed, is a value with all of D33's forms (each item's URL, host, `host:port` and path). The sweep of the other options found no other list: a `--peer` is parsed whole as a socket address (its printed form, `[::1]` for `[0::1]`, its IP, and the IPv4 dashcore prints for a mapped or compatible IPv6 one are forms now; so is a bracketed URL host without its brackets) and `--quorum-url` is one URL (`Url::parse`); the devnet name and the paths are single values. A dwcli test drives a session's own SDK request through dw-engine's debug-only `test-support` hook (`test_support::fetch_identity_balance`, enabled by dwcli's dev-dependency only, so dwcli still depends on no Platform crate): a two-address `--dapi` with path tokens (`127.0.0.1` and `localhost`, hosts every CI runner answers on; the pinned SDK panics building the TLS config of an IPv6 DAPI host, `InvalidDnsNameError` in rs-dapi-client `tonic_channel.rs:70`, which the engine reports as `internal`), the session options `run` builds (`session_options`) and the SDK `build_sdk` builds from them, an identity-balance fetch (the credits read) against closed ports, and every log line of the process at `info` through the redactor (the test runs in a process of its own with the log global, as dwcli's is: tracing's callsite cache is per process). It asserts both addresses were tried (two "ban address" lines) and that no token and neither host appears; with the split removed it fails on `ban address http://<redacted>/pathTOKENa` (mutation-checked). The binary sweep has a `--dapi` list case. | `option_values` kept the whole argument, whose one URL parse put the second address inside the first's path, so neither item's own path or the second host was a form; the redactor hid the first host and left `/pathTOKEN` (Sol r4). The hook skips only the list check: an `mn credits` through the binary needs a synced masternode list (a dashd regtest), while the session, its SDK, the SDK's line and the redactor are the binary's. In the shared test process a scoped subscriber caught nothing once other tests had reached the SDK's callsites. Known residual, no option value: a hostname's resolved IP is in hyper's connect error text (validator r4). |
| D38 | (Sol r4 Low 2.) The tests that use dw-appdb's power-loss simulator or dw-vault's fault injection are `#[cfg(all(test, debug_assertions))]`, imports included (`a_durable_setting_survives_power_loss`, `version_3_markers_survive_power_loss`, and tracking's `ambiguous_writes` module, whose fault-injection tests had the same gap since D31). The simulator stays out of release builds; `cargo test --release -p dw-appdb` and `cargo test --release -p dw-engine masternodes` compile and pass. | `durability_sim` and `fault` need `debug_assertions`, which a release test build lacks, so their tests didn't compile there (Sol r4). Exposing them to release test builds would put power loss and fault injection one feature flag from a release binary; the gate runs the debug build, where they run. |
| D39 | (Sol r5 Major 1.) An `mn` command's log line is cut at its first link (`dw_uri::redact::redact_links`) from the event as written, and only then are the option values replaced (D33, D37); the value pass leaves the bytes of a marker already in the line (`<link>` or its own `<redacted>`) as they are. A dwcli test parses `--network devnet:p`, logs an SDK-style WARN event whose `error` carries `dashpay://invite?pk=SECRET` through the `mn` writer, and asserts the line has the event and `<link>` and no `SECRET`; with the order swapped back it fails (mutation-checked), and a unit test with values `link` and `e` keeps both markers whole (mutation-checked with the skip removed). The `mn` sink now only writes to stderr. | The merge with DP5-01 ran the value pass first, so a short value could break a link's scheme or its marker (`p` turns `dashpay://invite?pk=SECRET` into `dash<redacted>ay://invite?<redacted>k=SECRET`), which the link redactor then no longer recognises, leaving the secret in the line (Sol r5). A link cut first takes everything after it, values included; a marker left whole stays recognisable and costs nothing, since a marker's text is the same whoever wrote it. |
| D40 | (Sol r5 Low 2.) dw-engine's `test_support` module is `#[doc(hidden)]`, and its comments (`lib.rs`, the module, the feature in `Cargo.toml`) state its gate exactly: the `test-support` feature, which only dwcli's dev-dependency enables, and `debug_assertions`, so any debug build of dw-engine with the feature has it and a release or dist build never does. No further gate. | `cargo build -p dw-engine --features test-support` exports it in a debug non-test build, so "tests only" overstated the gate (Sol r5). The API is one read-only identity-balance fetch through a session's own SDK; release and dist profiles have no `debug_assertions`, and dwcli's release build doesn't enable dev-dependencies. |
