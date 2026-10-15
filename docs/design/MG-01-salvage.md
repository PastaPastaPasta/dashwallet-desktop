# MG-01: Masternode and governance engine salvage

ROADMAP MG-01 (release 1.1, size L, engine, tier T0, stack-independent). The row: partly revert `aab64fc`
(dw-p2p governance wire commands, `PeerPicker::full_nodes`, events, error mapping, crate base files), then apply
`dw-governance`, `dw-protx` and the vault's masternode keys from the parked branches `m3/r2-governance` and
`m3/r3-protx`, resolving the conflict with `374c883`. It leaves out the operator-signed builders
(`prepare_update_service`, `prepare_revoke`), their CLI and console commands, and the M3 FFI and Swift adapters.

Acceptance: the crates' unit and oracle tests are green (object hashes and collateral scripts equal dashd's); no
operator-key signing path remains; the overlap with platform-wallet's masternode code is listed, with the library
preferred.

MG-01 builds no facade. `Governance` is MG-02, `Masternodes` MG-03, ProTx flows MG-04, authorization MG-05. What
lands here are the crates those facades call, with the MG-00 rows (UX-SPEC §4.25, `MG-00-masternodes-ux.md`
D1–D47) checked against them. The last section lists what MG-02…MG-05 must change.

| Part | Where |
|---|---|
| Governance wire names, full-node peers | `rust/crates/dw-p2p/src/{commands,peers,lib}.rs` |
| Governance objects, votes, proposals, clock, budget, store, govsync | `rust/crates/dw-governance/` |
| ProRegTx / ProUpRegTx payloads, service rules, operator public keys, v24 shared codecs, collateral outpoint | `rust/crates/dw-protx/` |
| Keys attached to tracked masternodes | `rust/crates/dw-vault/src/vault/masternode.rs` |
| Oracle vectors and their generators | `testdata/governance_*.json`, `testdata/protx_*.json`, `testdata/oracle/dashd/gen_*` |
| The no-operator-signing guard | `rust/crates/dw-protx/tests/no_operator_signing.rs` |

## 1. History of the parked branches

Both branches fork from `7e51024a0e` (809 commits behind the base `7668c4a6`). Main then removed the M3
governance, masternode-list and ProTx code (`aab64fc`, 2026-10-07) and ported only the keychain part of r3 into
main (`374c883`: `masternode_keys`, `Vault::with_revealed_seed`).

| Branch | Commits (oldest first) | Kind |
|---|---|---|
| `m3/r2-governance` | `0c1e550f` dw-governance crate · `f22982b2` engine module · `ae6bada6` oracle test · `3a979509` dwcli gov · `83513840` console · `5c936faa` DashKit/Swift adapter · `e7d16f8c` regtest suite · `a5b0681e` fmt · `a873e82b` G7 docs · `51b41600` Swift adapter tests | crate, facade, CLI, Swift |
| `m3/r3-protx` | `5bec8ec2` dw-protx crate · `2a50a3bd` vault keys · `604b217b` engine module · `01a7c117` FFI · `3fe22176` dwcli/console · `50ac39ab` regtest suite · `ea93be65` engine fix · `1f78d1b1` DashKit/Swift adapter · `2214fdfb` parity · `11ab3b21` docs | crate, vault, facade, FFI, CLI, Swift |

Only the crate and vault commits (`0c1e550f`, `ae6bada6`, `5bec8ec2`, `2a50a3bd`) and the `aab64fc` hunks the row
names are salvaged. Everything else is a facade, a binding or a suite that a later row rebuilds; it stays on the
branches for reference.

## 2. What comes from where

### 2.1 Partial revert of `aab64fc`

| Hunk | Salvaged | Notes |
|---|---|---|
| `dw-p2p/src/commands.rs`: `govsync`, `govobj`, `govobjvote`, `ssc` | yes | Wire names for dw-governance's transport now, dw-p2p's sessions in MG-02. |
| `dw-p2p/src/peers.rs`: `PeerPicker::full_nodes` and the configured extra peers | yes | As `PeerPicker::with_full_nodes(extra)` instead of a second `new` argument, so CoinJoin's `PeerPicker::default()` is unchanged. |
| `dw-p2p/src/{lib,codec,testing}.rs` doc lines | yes | Governance named as a consumer again. |
| Workspace members and `[workspace.dependencies]` of `dw-governance`, `dw-protx` | yes | Crate base files. |
| `dw-governance/{Cargo.toml,src/lib.rs,src/params.rs}`, `dw-protx/{Cargo.toml,src/lib.rs,src/params.rs}` | yes | Superseded by the branch heads (2.2, 2.3). |
| `dw-engine/src/events.rs`: `EngineEvent::{Governance, Masternodes}` | **no** (D3) | |
| `dw-engine/src/error.rs` + `dw-ffi` error mapping: `EngineError::Governance`, the M3 `MasternodeFailure` codes | **no** (D3) | |
| `dw-engine` → `dw-governance`/`dw-protx` dependencies | **no** (D3) | Added by the facade that first calls them. |
| `dw-vault`: `GrantPurpose::{MasternodeOp, Governance}` | **no** | MG-05 restores them in the new shape (`MasternodeOp{max_duffs}`). |
| `dw-engine/src/{governance,masternodes}.rs`, `dw-ffi/src/api/{governance,masternode,protx,m3_tests}.rs` | **no** | Facades (MG-02/03/04) and the M3 FFI (out by the row). |
| `docs/contracts/m3-engine.md`, `m1-engine.md` | **no** | The new contract is MG-02's `mg-engine.md`. |

### 2.2 `dw-governance` (from `m3/r2-governance`)

| File | Salvaged | Notes |
|---|---|---|
| `params.rs` | yes | Checked against v24.0.0-rc.3 `chainparams.cpp` (start, cycle, window, min quorum on all four networks). |
| `object.rs` | yes | `CGovernanceObject` wire form and hash. |
| `vote.rs` | yes | `CGovernanceVote` wire form, hash, signing digest per network, key recovery. |
| `proposal.rs` | yes | Create-proposal validation, dash-qt key order, synced proposal and trigger parsing. |
| `clock.rs` | yes | Superblock heights, maturity window, ETA, budget (`GetPaymentsLimit`). |
| `store.rs` | yes | Objects and the current vote per **collateral outpoint** (Core `mapCurrentMNVotes`). |
| `tally.rs` | **partly** (D5) | Kept: `Tally`, the threshold, `fundable`, the eight dash-qt statuses and their sort. Left out: `MasternodeIndex`, `counted_votes`, `tally`, which map votes to masternodes by voting key (MG-00 D45 *Why*). |
| `net.rs` | yes (D4) | Its own P2P transport, verbatim. MG-02 moves govsync onto `dw_p2p::Session` and deletes it. |
| `sync.rs` | yes (D4) | govsync phases (Disabled/Waiting/SyncingObjects/SyncingVotes/Synced/Failed), which MG-00's MG-01 follow-up asks to keep. |
| `tests/golden.rs`, `testdata/governance_objects.json`, `testdata/oracle/dashd/gen_governance_objects.py` | yes | The branch's oracle test. |
| engine `governance/*.rs`, dwcli `gov.rs`, console `m3_governance.rs`, Swift, `regtest/functional/dwd_governance.py`, G7 docs and script | no | MG-02 (facade, console), MG-08 (dwcli, nightly suites), E0-13 (bindings). |

### 2.3 `dw-protx` (from `m3/r3-protx`)

| File | Salvaged | Notes |
|---|---|---|
| `params.rs` | yes | Collateral amounts, share limits, default ports (checked against v24 `chainparams.cpp`). |
| `service.rs` | yes | Version-2 service rules (`MnNetInfo`, `CheckProviderNetworkFields`). |
| `payloads.rs` | **partly** | Kept: ProRegTx placeholder and finalizer (FundNew, existing collateral with the collateral key's sign string), ProUpRegTx (owner-signed), `resolve_collateral`. **Left out:** `update_service_placeholder`, `finalize_update_service`, `service_payload_fields`, `revoke_placeholder`, `finalize_revoke` (operator BLS signatures). |
| `bls.rs` | **partly** (D6) | Kept: operator **public** key parsing (`parse_public_hex`, `legacy_of`). **Left out:** `generate` (operator secret generation, §5a and MG-00 D27), `parse_secret_hex`, `secret_hex`, `public_keys`, `matches` (secret handling; the library has `bls_public_keys` and `verify_masternode_key`), `sign_hash` (operator signing). |
| `keychain.rs` | **no** | Main's `dw-engine/src/masternode_keys.rs` (`374c883`) already derives every role through platform-wallet's `derive_provider_key_at_index` and writes the Tenderdash form; the paths and node id are the library's. |
| `shared.rs` | yes | ProDisTx (10), ProUpShareTx (11), ProUpSharedRegTx (12) and the share table; still absent from dashcore at the pin. Every signature here is an owner's. |
| engine `masternodes/{list,keys,protx}.rs` | no | MG-03 (list, tracked keys), MG-04 (registration, ProUpRegTx). `prepare_update_service`, `prepare_revoke`, the operator-secret gate (`operator_secret`, `confirm_operator_secret`) and the self-operator secret generation are left out for good. |
| dwcli `mn.rs`, console `m3_masternode.rs`, FFI `masternode/protx.rs`, Swift, `regtest/harness/tests/test_protx.py` | no | Out by the row (CLI/console of the operator builders, M3 FFI and Swift); the owner commands come back with MG-08. |

### 2.4 Vault masternode keys (from `2a50a3bd`, conflict with `374c883`)

`374c883` ported `Vault::with_revealed_seed` into `dw-vault/src/vault/masternode.rs`; `2a50a3bd` adds, in the same
file, `grant_wallet`, the attached keys of tracked masternodes and two `VaultSigner` methods. Main's copy is newer:
the read is gated (`Vault::gated`, epoch rules from `d36acdbe`…`589e709c`), so the resolution keeps main's
`with_revealed_seed` as it is and adds the attached keys on main's primitives:

| r3 item | Resolution |
|---|---|
| `with_revealed_seed` | main's version (gated), unchanged. |
| `mnkey/<proTxHash>/<role>` records, `store_masternode_key`, `masternode_key`, `masternode_key_roles`, `delete_masternode_keys` | ported (D7). Roles are `owner`, `voting`, `payout` only: an operator or platform-node secret is refused (§5a: the wallet holds no operator secret it does not derive). Writing and deleting need the full-scope data key, as `store_wallet_secret` does; reading needs a redeemed `RevealSecret` grant and runs inside the gate. |
| `grant_wallet` | left out: only the r3 attach flow, which took a grant bound to any wallet, used it. |
| `VaultSigner::with_seed` | left out: it existed to sign with operator BLS and platform-node keys. |
| `VaultSigner::secp_secret` | left out (D8): MG-04/MG-05 decide how a payload finalizer gets the owner or collateral key. |

## 3. Oracle evidence

The correctness bar: governance object hashes, vote hashes and signatures, and collateral scripts equal dashd's.

| What | Vector | Source |
|---|---|---|
| Object hash, `OP_RETURN` collateral script | `testdata/governance_objects.json` (4 cases, the branch's) | regtest `gobject prepare` (recorded by r2) |
| Vote hash, vote signature bytes, acceptance | `testdata/governance_votes.json` | regtest v24.0.0-rc.3: `signmessage` with the voting key, `voteraw` with the Rust signature, `gobject getcurrentvotes` |
| Testnet signing digest (signature hash, not a signed message) | `testdata/governance_votes_testnet.json` | the shared testnet v24 oracle: raw votes of current proposals over one per-object `govsync`, `gobject getcurrentvotes` for their hashes, `protx listdiff` for each voter's voting key |
| Collateral outpoint (null-hash internal and external collateral, MG-00 D43) and the internal collateral output | `testdata/protx_collateral.json` | regtest: `protx register_fund` and `protx register`, `protx info` |
| ProRegTx sign string of an external collateral | same file | regtest `protx register_prepare` (`signMessage`) |
| Superblock budget and governance parameters | `testdata/governance_budget.json` | `getsuperblockbudget` and `getgovernanceinfo` on mainnet and testnet oracles and on regtest |

The shared oracles get pure reads only, one at a time (`getgovernanceinfo`, `getsuperblockbudget`,
`gobject getcurrentvotes`, `protx listdiff`, one `govsync` per object hash, which Core rate-limits per peer and
object). Every generator records the dashd version and the commands; regenerating is described in
`testdata/oracle/README.md`.

## 4. No operator-key signing path

`rust/crates/dw-protx/tests/no_operator_signing.rs` scans every `.rs` file of the workspace for the operator-signed
builders and their primitives (ProUpServTx/ProUpRevTx payload construction, the r3 and platform-wallet builder
names, BLS secret keys and BLS signing calls). The only allowed hit is `dw-coinjoin`'s test helper that plays a
masternode signing `dsq`/`dstx` (D9). Rerun:

```sh
cd rust && cargo test -p dw-protx --test no_operator_signing
```

## 5. Overlap with platform-wallet (`ebe37f8a67`)

The library is preferred wherever it covers the same thing.

| Need | platform-wallet | Salvaged code | Use |
|---|---|---|---|
| Typed SML entries, find by IP or ProTx hash | `masternode::list::{MasternodeListSummary, find_in_summaries}` | (r3 engine `list.rs`, not salvaged) | library (MG-03) |
| Locate by key, check an attached key against the masternode | `locator::{parse_locator_input, locate_in_summaries, verify_masternode_key}` | (r3 engine `keys.rs`, not salvaged) | library (MG-03, D13) |
| Wallet's masternodes from its provider transactions, in block order | `record::aggregate_masternodes`, `wallet_masternodes_blocking` | (r3 engine `list.rs`) | library (MG-03, D42) |
| Collateral outpoint of a masternode | `MasternodeRecord.collateral` copies the **payload** outpoint | `dw_protx::payloads::resolve_collateral` | **dw-protx**: the library's field is `(0…0, n)` for internal collateral, which D43 and Core resolve to `(ProRegTx txid, n)` |
| Tracked-masternode registry and its persistence | `tracked::TrackedMasternodes` | — | library (MG-03) |
| Secrets attached to tracked masternodes | none (the library leaves them to the host) | `Vault` `mnkey/` records | **dw-vault** |
| Provider keys per role and index, WIF, Tenderdash form | `derive_provider_key_at_index` (+ key-wallet) | `dw-protx::keychain` (dropped) | library, via main's `masternode_keys.rs` |
| Operator public key from a secret, platform node id | `locator::{bls_public_keys, ed25519_node_id}` | `bls::public_keys`, `keychain::platform_node_id_hex` (dropped) | library |
| Operator public key **input** (96 hex, basic form), legacy form | none | `bls::{parse_public_hex, legacy_of}` | dw-protx |
| ProRegTx / ProUpRegTx payload build and sign | none | `payloads.rs` | dw-protx (MG-04) |
| v24 shared payloads | none (nor dashcore) | `shared.rs` | dw-protx (MG-04b) |
| Version-2 service and Platform port rules | none public (`update_service::service_payload_fields` is `pub(crate)`) | `service.rs` | dw-protx |
| ProUpServTx (revive) | `update_service::{prepare,execute}_masternode_update_service` | (dropped) | **neither**: operator-signed, never called (guard, §4) |
| Evonode credit withdrawal | `wallet::masternode_withdrawal` (`masternode_withdraw`) | — | library (MG-13, after the D44 upstream change) |
| Governance (objects, votes, sync, tally, clock, budget) | none | `dw-governance` | dw-governance |

## 6. Slice plan

Each commit builds and passes `cargo test` for the crates it touches.

1. `docs(design)`: this file.
2. `feat(dw-p2p)`: governance wire commands and full-node peers.
3. `feat(dw-governance)`: the crate (2.2) with its unit and golden tests.
4. `feat(dw-protx)`: the crate (2.3) without operator signing.
5. `feat(dw-vault)`: attached masternode keys (2.4).
6. `test`: v24 regtest and oracle vectors (§3) with their generators.
7. `test(dw-protx)`: the no-operator-signing guard (§4).

## 7. What MG-02…MG-05 must change

| Row | Change |
|---|---|
| MG-02 | Move govsync onto `dw_p2p::Session` and delete `dw-governance/src/net.rs` (the row says "govsync over dw-p2p"); re-run G7 since the transport changes. Write a tally keyed by the D43-derived collateral outpoint with the D45 range (unverifiable type → weight 0, 1 or 4) instead of r2's voting-key `MasternodeIndex`. Name the cutoff the estimated cutoff (`Clock::voting_cutoff`, D19). Core v24 reads a non-zero `govsync` with an **empty** filter as an object fetch (`object_fetch_filter`) and rate-limits each object's vote sync per peer: `sync.rs` already sends a non-empty filter, keep it so. Define `GovernanceFailure`, its codes and the `Governance` event in `mg-engine.md`. |
| MG-03 | Resolve collateral with `dw_protx::payloads::resolve_collateral`, not `MasternodeRecord.collateral`, and add D43's checks of the internal collateral output (amount 1,000/4,000 Dash, single-address or shared template script). Persist attached keys with `Vault::store_masternode_key` (owner, voting, payout only) and delete them on Stop tracking (D22). Define the `Masternodes` event and the masternode failure codes. |
| MG-04 | Add the version-3 payload where the network needs it (D39; `service.rs` is version 2, IPv4 only). Refuse a pasted BLS secret where a public key is expected (`bls::parse_public_hex` refuses non-points; detect 64-hex input with the library's `parse_locator_input`). Self-operator keys come from the keychain only (D27): no operator secret is generated. Decide how the finalizer gets the owner or collateral secret (r3's `VaultSigner::secp_secret` is on the branch). |
| MG-05 | Restore `GrantPurpose::{Governance, MasternodeOp{max_duffs}}` and the scopes; give `MasternodeVoting` a read of vault-held voting keys (`Vault::masternode_key` takes `RevealSecret` today) and decide whether attaching a key needs a grant beyond the full-scope data key. |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Salvage only the crate and vault commits and the named `aab64fc` hunks; facades, bindings, CLI, console, Swift and regtest suites stay on the branches. | The row and the brief: MG-01 builds no facade; MG-02/03/04/08 and E0-13 own those parts, against a newer engine and the MG-00 rows. |
| D2 | Port the code onto main's dependencies (dashcore 0.45 at `d3d50520a`, `dash_pkc` BLS, the `gated` vault) rather than reverting files byte for byte. | The branches fork 809 commits back; their dashcore and vault APIs moved. |
| D3 | Do not restore `EngineEvent::{Governance, Masternodes}`, `EngineError::Governance` or the M3 masternode failure codes, although the row lists "events, error mapping". | Both are IPC: `dw-app` maps every event and error to generated TypeScript (`AppEvent`, `MasternodeCode`), so restoring them means a contract and bindings change (and the Swift T0 suite) for variants nothing emits. Their shape belongs to MG-02's `mg-engine.md` and E0-13's MG binding. |
| D4 | Keep `net.rs` and `sync.rs` as they are, with their tests. | `sync.rs` holds the govsync phases MG-00 asks MG-01 to keep, and both were measured on mainnet (G7) together. Moving the transport now could not be validated without MG-02's 4-masternode suite. |
| D5 | Drop the voting-key tally (`MasternodeIndex`, `counted_votes`, `tally`). | MG-00 D45 *Why* and MG-02's follow-up: votes are keyed by collateral outpoint; the voting-key mapping skews weights for shared keys and evonodes. `store.rs` already keys current votes by outpoint. |
| D6 | dw-protx handles no BLS secret at all. | §5a and D27: no operator secret is generated or held outside the keychain reveal, and the wallet never signs with an operator key. Public-key parsing is all MG-04 needs; the library covers secret → public. |
| D7 | Attached keys: roles `owner`, `voting`, `payout`; write and delete under the full-scope data key; read under `RevealSecret`. | No new grant purpose (`GrantPurposeSpec` is IPC, and MG-05 owns the purposes). The full-scope write matches `store_wallet_secret`; the read keeps "no secret leaves without a grant". An operator role would let the wallet hold an operator secret it does not derive (§5a). |
| D8 | Leave `VaultSigner::secp_secret` out. | It hands a raw key out of the signer; with no caller in MG-01 its scope rules would be guessed ahead of MG-05. |
| D9 | The guard allows `dw-coinjoin::bls::testing::OperatorKey`. | It plays a masternode signing queue messages in CoinJoin tests; it signs no wallet transaction. |
