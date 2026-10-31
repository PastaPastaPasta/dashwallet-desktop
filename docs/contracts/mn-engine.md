# MG masternodes engine contract (`dw-engine` facade)

Contract-Version: 17

Status: **draft**, 2026-10-17 (ROADMAP MG-03; version 2 after the Sol r1 review: D23–D28; version 3 after Sol r2: D30–D32; version 4 adds MG-04's ProTx owner flows, §2.3, citing
[`MG-04-protx-owner.md`](../design/MG-04-protx-owner.md)'s D-rows as "MG-04 D1" and so on; version 5, after MG-04's r2 review, adds `operation` to `masternodes.protx_refused`; version 6 adds MG-04b slice 1a's shared-masternode sessions, §2.4, citing [`MG-04b-shared-masternodes.md`](../design/MG-04b-shared-masternodes.md)'s D-rows as "MG-04b D1" and so on; version 7, after the slice's validator review, binds `lock_shared_terms` to a revision and `approve_shared` to a consent hash, and adds `SharedSession.consent_hash`, MG-04b D56; version 8 adds `sign_shared_funding`'s consent-hash refusal, MG-04b D65; version 9 adds slice 1b's sending: `can_send` true, `send_shared_registration`, `abandon_shared_session`, `update_share` and `ProTxOperationKind::UpdateShare`, `SharedSession.debit`, `MasternodeRow.shares`, MG-04b D68–D75; version 10, after its review r1, puts sending behind one switch that is off until slice 1c, `can_send` false and the four sending calls refusing `unsupported`, DEC-678, MG-04b D71); version 11, after its review r2, says where `resend_operation` of an `update_share` row refuses (MG-04b D71); version 12 turns sending on (slice 1c: D39 closed by MG-04b D75, M9 by D78); version 13 adds `ShareTermsView.service_read`, Core's reading of a service written in another form (MG-04b D78); version 14 adds MG-04b slice 2a's dissolve now: `preview_dissolve`, `dissolve`, `ProTxOperationKind::Dissolve`, `DissolveRequest`, `DissolvePreview` and `ShareReturn`, a `dissolve` row's resend and state, MG-04b D79–D83; version 15 adds MG-04b slice 2b's standby: `create_standby`, `standby`, `export_standby`, `send_standby`, `Standby`, `StandbyDescription`, `StandbyFile`, `StandbySource`, `StandbyVariant`, `SharedImport::Standby` (and `SharedImport::Session`'s session boxed, its JSON unchanged), `SharedNext::CreateStandby`, a `dissolve` row of another share's standby, MG-04b D84–D91; version 16 adds MG-04b slice 2c's dissolve together: `start_dissolve_together`, `approve_maintenance`, `send_maintenance`, `masternodes.shared_actor_fee{fee}`, `SharedWarning::SkippedSignatures` and `NoApprovals`, the import of a `dissolve` sigs envelope and of a targeted bare array, a dissolve-together session's stages, and a `dissolve` row it sends, MG-04b D92–D98; version 17 adds MG-04b slice 2d's key change: `start_key_change`, `KeyChangeRequest`, `KeyChangeView` and `SharedSession.key_change`, `SharedWarning::OperatorReset` and `UsableByFunder`, `DeadReason::Abandoned`, the import of a `registrar` sigs envelope and of a bare array targeting a key change, a key-change session's stages, its abandon before `out_at`, `shared_message{reason}`'s `not_key_change`, `inputs_hash` and `stale`, and `update_pending{operation}` naming a key-change session; and it breaks `send_maintenance`, which returned a `ProTxOperation` and now returns `MaintenanceSent` (`operation` for a dissolution, `session` for a key change, each boxed, which the JSON doesn't show), MG-04b D99–D105. Milestone 1.1, masternodes and governance (UX-SPEC §4.25, MG-00).
Code: `rust/crates/dw-engine/src/masternodes/` (§0). Design: [`MG-03-masternodes.md`](../design/MG-03-masternodes.md);
its D-rows are cited as "D1" and so on. Everything in [`m1-engine.md`](m1-engine.md) §1 (conventions) applies, with
the changes in §1 below.

The facade is plain Rust in dw-engine. `dwcli mn` drives it directly; E0-13 binds it for the UI stack, with binding
derives in the binding crate only. Governance has its own file ([`mg-engine.md`](mg-engine.md), MG-02); MG-04
(ProTx management) and MG-13 (credit withdrawal) extend this one. **A change to a signature, a record or a code
updates this file and raises `Contract-Version` in the same change.**
`rust/crates/dw-engine/tests/mn_masternodes_contract.rs` enforces it the way `mg_governance_contract.rs` enforces
mg-engine: the generated §3 listing with its SHA-256 (`DW_BLESS=1` to approve a raised version), one §2 row per call
with matching Kind and Errors, and the §4 codes per variant.

## 0. Object model and files

```
Engine ──open_network──▶ NetworkSession ──masternodes()──▶ Masternodes   (one shared state per session)
```

`NetworkSession::masternodes()` is sync and infallible and returns a cheap handle (`Clone`). The state it reads lives
in the session: the SPV masternode list, platform-wallet's tracked-masternode registry, the vault's attached keys,
app.sqlite's notes, and the follower that sends §6's event for the session's life. Any number of handles may exist.
After `close_network` every call returns `network_not_open`.

| File | Owner | Contents |
|---|---|---|
| `masternodes/mod.rs` | MG-03 | `Masternodes`, `NetworkSession::masternodes`, the re-exports |
| `masternodes/records.rs` | MG-03 | the records of §3 |
| `masternodes/reads.rs` | MG-03 | §2.1 |
| `masternodes/tracking.rs` | MG-03 | §2.2 |
| `masternodes/errors.rs` | MG-03 | `MasternodesError` |
| `masternodes/aggregate.rs` | MG-03 | a wallet's masternodes from its provider transactions (not facade) |
| `masternodes/follow.rs` | MG-03 | the list reader, the app.sqlite notes and the follower (not facade) |
| `masternodes/protx.rs` | MG-04 | §2.3 |
| `masternodes/protx_records.rs` | MG-04 | the records of §2.3 |
| `masternodes/protx/` | MG-04 | keys, the transaction builder and the operations' rows and hand-off (not facade) |
| `masternodes/shared.rs` | MG-04b | §2.4 |
| `masternodes/shared_records.rs` | MG-04b | the records of §2.4 |
| `masternodes/shared/` | MG-04b | the session store and views, the funding builder, the flows and §3.0's import table (not facade) |

## 1. Conventions (changes to m1-engine.md §1)

| Topic | Rule |
|---|---|
| Hashes | ProTx hashes and txids are 64-character lower-case hex in Core's display order (byte-reversed), as `protx list` prints them. Input is accepted in either case. An outpoint is `"<txid>-<n>"` (Core's `ToStringShort`). |
| Amounts | Duffs, `u64`; Platform credits `u64` (`EvonodeCredits.credits`). |
| Times | Unix seconds, `u64`. |
| Sources | Each field comes from one source (design §1): the SPV list (`ListFacts`, status), provider transactions a wallet holds (`RegistrationFacts`, roles), the wallet's history (payouts), Platform (credits). |
| The list | Only a synced list counts (the headers and masternode phases done; design §2). Before that, list-only facts are absent and status is `unknown`, except a held unconfirmed ProRegTx (`unconfirmed`). |
| Records | `serde` `Serialize` and `Deserialize`, plus `Debug`, `Clone` and `PartialEq`. Unit enums are snake-case strings; enums with data are tagged with `kind`. |
| Errors | `MasternodesError` (§4); `code()` is a stable string, `engine()` returns a wrapped `EngineError`. |
| Keys | Attached keys are secp256k1 secrets for `owner`, `voting` or `payout`, kept by the vault (MG-01 D7). No call takes, holds, returns or signs with an operator or platform-node secret (MG-01's guard, `dw-protx/tests/no_operator_signing.rs`): the operator key is a BLS **public** key, given or the keychain's (MG-04 D14). Only §2.3's calls sign, with the vault's `MasternodeOwner` signer under a lease from a `MasternodeOp` grant (MG-05): the inputs, an `ExistingUtxo` collateral's message, and a ProUpRegTx's owner signature, which the vault computes over the payload it is handed and signs with a keychain or attached owner key (MG-04 D11, D12). §2.4's calls sign with keychain owner keys 0–19 only, through the vault's shared owner signer (MG-04b D11), under a `SharedMasternode` lease from a `MasternodeOp` grant: a share's consent signature and a participant's funding inputs; the attached owner key stays ProUpRegTx-only, and nothing signs as operator. |

## 2. Calls

Kind is `sync` (memory only) or `async` (on the engine runtime). Every call is implemented.

### 2.1 Reads

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.masternodes()` | sync | The handle (§0). Infallible: returns `Masternodes`, not a `Result`. **Implemented.** | — |
| `list_state()` | sync | `MasternodeListState`: whether the SPV list is synced (the headers and masternode phases done) and the masternode phase's height (D3). Memory only. **Implemented.** | `MasternodesError` |
| `list(wallet: WalletId)` | async | `MasternodeOverview`: the list state, the wallet's own masternodes (in the library's registration order) then the tracked ones not among them (in tracking order), each a `MasternodeRow` with status (§5), roles, collateral, payout address, 30-day payouts and the newest payout; the collateral-shaped coins (MG-00 D1); the wallet's 30-day payout total, each history record counted once. `wallet_not_found` for an unknown wallet. **Implemented.** | `MasternodesError` |
| `masternode(wallet: WalletId, pro_tx_hash: String)` | async | `MasternodeDetail`: the row, the list's facts, the registration facts from the wallet's provider transactions, `payout_checked_height` (always `None` until MG-00 D42's watch exists, D5), `tracked_at`. Any masternode the wallet owns or tracks, or that the synced list holds. `invalid_argument` for a malformed hash (first), then `masternodes.not_found`, or `masternodes.list_unavailable` while the list that might hold it isn't synced. A `wallet_not_found` names no wallet id (D23). **Implemented.** | `MasternodesError` |
| `payouts(wallet: WalletId, pro_tx_hash: String, since: Option<u64>)` | async | Every `MasternodePayout` of the masternode, newest first (by height, then time): the wallet's coinbase history records to its payout address (design §5, D9), without orphaned ones; with `since`, only those at or after it. Empty when the payout address isn't known. Each is `unattributed` once version 3 was seen (§5, D30). Refuses like `masternode`, then `masternodes.unsupported{feature: "shared_masternodes"}` for a `shared_unsupported` one (D25). **Implemented.** | `MasternodesError` |
| `find(query: String)` | async | The synced list's entries matching a ProTx hash (64 hex) or a service address (`ip:port`, or `ip` for any port), in list order, as `ListedMasternode` with whether each is tracked. `invalid_argument` for anything else (first), `masternodes.list_unavailable` while the list isn't synced. No private key is accepted (MG-00 D13). **Implemented.** | `MasternodesError` |
| `evonode_credits(pro_tx_hash: String)` | async | `EvonodeCredits`: the balance of the evonode's owner identity (id = the ProTx hash's display bytes), read with a Platform proof (dash-sdk `IdentityBalance`). Refusals in order: `invalid_argument`, `masternodes.list_unavailable`, `masternodes.not_found`, `masternodes.not_evonode`, `masternodes.platform_unavailable{detail}` (no Platform in this session, or DAPI failed; the SDK's text goes to the log only, as it may name the identity), `masternodes.no_identity`. The fetch runs outside the session's operation guard. **Implemented.** | `MasternodesError` |
| `resolve_collateral(pro_tx_hash: String)` | async | The header-checked collateral outpoint of any listed masternode (MG-00 D43). `invalid_argument` for a malformed hash, else always `masternodes.unsupported{feature: "collateral_lookup"}`: dash-spv exposes no header-by-hash or block fetch at the pin (D11). **Implemented** (as the refusal). | `MasternodesError` |

### 2.2 Tracking and attached keys

Every call here sends §6's event after it changes something.

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `track(pro_tx_hash: String, label: Option<String>)` | async | Adds a masternode of the synced list to platform-wallet's registry (wallet.sqlite; it belongs to the network, D13). Returns its `ListedMasternode`. Refusals in order: `invalid_argument` (hash, or a label over `MAX_MASTERNODE_LABEL` characters after trimming), `masternodes.list_unavailable`, `masternodes.not_found` (not in the list, D7), `masternodes.already_tracked`. **Implemented.** | `MasternodesError` |
| `untrack(pro_tx_hash: String)` | async | Deletes the attached keys first, then the registry row (MG-00 D22). Returns `UntrackOutcome { keys_deleted }`; the keys' recorded ids go too (D24). `invalid_argument`, `masternodes.not_tracked`; a vault that can't delete (locked: `vault`) leaves it tracked. The event is sent once keys were deleted, even if the registry row then couldn't be removed. **Implemented.** | `MasternodesError` |
| `set_label(pro_tx_hash: String, label: Option<String>)` | async | Trims the label; blank or `None` clears it (back to the default name, `type_index`). A tracked masternode's label is the registry's; a loaded wallet's masternode's or a listed one's is kept in app.sqlite (D12). Refusals in order: `invalid_argument` for a malformed hash or a label over `MAX_MASTERNODE_LABEL` characters, then for any other hash `masternodes.not_found`, or `masternodes.list_unavailable` while the list that might hold it isn't synced (D27). **Implemented.** | `MasternodesError` |
| `attach_key(pro_tx_hash: String, role: AttachedKeyRole, key: Zeroizing<String>)` | async | Stores a compressed WIF (mainnet's for mainnet, a test network's otherwise; an uncompressed one is refused, D36) or 64-hex secp256k1 secret for `role` of a tracked masternode, replacing an earlier one. Checked first (MG-00 D13): `voting` against the synced list's voting key id (`masternodes.list_unavailable` while it isn't synced; `unchecked` when the list no longer holds it); `owner` against the owner key id of a ProRegTx/ProUpRegTx a loaded wallet holds, otherwise `unchecked`; `payout` is `matched` when it pays that wallet's P2PKH payout script, otherwise `unchecked`, never refused (that script can be stale, D19). The hash160 of its public key is recorded in app.sqlite: the old id is cleared, durably (D34), before the vault write and the new one recorded after it succeeds, and every read compares it again (§5, D24). After a failed vault write no id is recorded: the key may be stored all the same, so it reads `key_id: None`, `unchecked`, holding nothing, until a later session's read records the id of the key the vault holds (D31). Tracking changes (`track`, `untrack`, `set_label`, `attach_key`, `detach_key`) run one at a time per session, so a key can't be stored for a masternode an `untrack` is removing. Returns `KeyCheck`. Refusals in order: `invalid_argument` (hash, key), `masternodes.not_tracked`, `masternodes.list_unavailable`, `masternodes.key_mismatch{role}`, then the vault's (`vault`, for one without the full-scope data key). **Implemented.** | `MasternodesError` |
| `detach_key(pro_tx_hash: String, role: AttachedKeyRole)` | async | Deletes the key attached for `role`, then its recorded id. Returns whether there was one. `invalid_argument`, `masternodes.not_tracked`, then the vault's. **Implemented.** | `MasternodesError` |

### 2.3 ProTx owner flows (MG-04)

A `RegistrationRequest` is checked first on its own (`invalid_argument`; `masternodes.unsupported{feature:
"protx_v3", field: "service"}` for a service only a version-3 payload carries, MG-04 D1; `masternodes.protx_refused`
for a port Core refuses), then against the synced list (`masternodes.list_unavailable` before it is synced), the
wallet's masternodes and its saved operations: keys in use (`masternodes.protx_refused{reason:
"bad-protx-dup-key"}` for a given one, MG-04 D5, D13; `masternodes.keys_exhausted{role}` when every keychain index
0–19 is in use), a voting address that is the owner key's (`invalid_argument`, MG-04 D52), a listed service
(`bad-protx-dup-netinfo-entry`), the collateral (`masternodes.collateral_unavailable`, MG-04 D23), and an operator key,
platform node id or service that one of the wallet's operations names while it may go out or is out and not in the
list (`masternodes.protx_refused{reason: "protx-dup", operation}`, Core's mempool reason, naming that operation,
MG-04 D51; an update claims only its operator key). A `conflicted` operation holds no key and no collateral
(MG-04 D54). An operation id is 32 lower-case hex (`invalid_argument` otherwise), then `masternodes.operation_not_found`,
then `masternodes.in_flight` while another call on it runs, then `masternodes.operation_state{state}` when the call
doesn't apply to its state, then (a resent update) `masternodes.update_superseded{operation}` for a later update, or `masternodes.update_pending{operation}` for an earlier one handed off and not in a block (MG-04 D43, D49). An `UpdateRegistrarRequest` is checked first on its own (`invalid_argument`, the ProTx
hash among it), then against the synced list (`masternodes.list_unavailable`, `masternodes.not_found` when not
listed), then the masternode (`masternodes.unsupported{feature: "shared_masternodes"}` for a version-3 one), then its
owner key (`masternodes.no_owner_key`, MG-04 D12), then any update while an operation of a loaded wallet on the
masternode may not be in the list yet (`masternodes.update_pending{operation}`, MG-04 D43, D49), then a keychain key to pick
(`masternodes.keys_exhausted`), a new voting address that is the owner key's (`invalid_argument`, MG-04 D52), a new
operator key a pending operation of the wallet names (`protx-dup` with its `operation`, MG-04 D51), then Core's rules
(`masternodes.protx_refused`: `bad-protx-dup-key`, `bad-protx-payee-reuse`, `bad-protx-collateral-reuse`, …). A
shared masternode (one a registration session of this wallet holds) is refused right after the list's
`masternodes.not_found`, before its owner facts: `masternodes.protx_refused{reason: "bad-protx-shared-mn"}`, as
consensus refuses any ProUpRegTx on one (rc.3 `specialtxman.cpp:1444-1448`; MG-04b D30). A grant is the lease's: `MasternodeOp` for the wallet, its cap covering the fee
(`send.grant_exceeded`, nothing signed); its refusals come through `Platform`/`Engine` unchanged (MG-05 D12). Each
signing call saves its row durably, with its inputs locked (MG-04 D25), before the hand-off; then hands the bytes to
`TxDraft`'s fenced path, and returns the operation whatever the hand-off's outcome: `sent` (it may be out), or
`not_sent` with `last_error` (definitely not). Every call that changes a row sends §6's event.

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `registration_keys(wallet: WalletId, kind: MasternodeType)` | async | `ProTxKeys`: the keychain owner, voting and operator keys (and an evonode's platform node key) a registration of `kind` would use by default, the lowest index 0–19 no masternode of the list, of the wallet or of a saved operation uses (MG-04 D13). Public keys and addresses only. `wallet_not_found`, `masternodes.list_unavailable`, `masternodes.keys_exhausted{role}`. **Implemented.** | `MasternodesError` |
| `preview_registration(wallet: WalletId, request: RegistrationRequest)` | async | `RegistrationPreview`: the resolved terms, the fee and `total_debit` (the fee: a `FundNew` collateral stays in the wallet), and the collateral amount. Locks, reserves, issues and signs nothing; a `NewAddress` payout and a `FundNew` collateral address are issued at registration, so the preview leaves them empty. Refusals as above, then the funding's `send.*` (`send.amount_exceeds_balance`, …). **Implemented.** | `MasternodesError` |
| `register(wallet: WalletId, request: RegistrationRequest, grant: String)` | async | A version-2 ProRegTx with a `FundNew` collateral (an output of the transaction to a fresh address) or an `ExistingUtxo` one (an unspent wallet coin of the type's amount at a P2PKH address, locked, MG-04 D9; its key signs Core's `MakeSignString`; a `FundNew` output is locked in the write that saves the row, MG-04 D53), regular or evonode, funded from the wallet's standard coins, BIP69-sorted (MG-04 D2, D3). Returns the `ProTxOperation`. `invalid_argument` for an `External` collateral. **Implemented.** | `MasternodesError` |
| `prepare_external_registration(wallet: WalletId, request: RegistrationRequest)` | async | An `External` collateral (another wallet's coin, MG-04 D4): plans and saves the unsigned transaction with its fee inputs locked (MG-04 D8) and returns the operation as `needs_signature` with its `SignRequest`: the collateral address and Core's message, byte-identical to `protx register_prepare`'s `signMessage`. Takes no grant and signs nothing (MG-04 D19). `invalid_argument` for another collateral, or for a coin or address of this wallet. **Implemented.** | `MasternodesError` |
| `submit_external_registration(wallet: WalletId, operation: String, signature: String, grant: String)` | async | Checks `signature` (base64, Core's `signmessage`) by the collateral address over the saved message (`masternodes.signature_mismatch`), refuses an operator key, platform node id or service another pending operation of the wallet names (`masternodes.protx_refused{reason: "protx-dup", operation}`, MG-04 D51), signs the inputs under the grant, saves and hands off as `register` does. Only from `needs_signature`. **Implemented.** | `MasternodesError` |
| `refresh_external_registration(wallet: WalletId, operation: String)` | async | Plans a `needs_signature` or `inputs_spent` row again: new fee inputs, a new message, the same terms. The old inputs are unlocked and the new ones locked; a plan that fails keeps the old ones locked. **Implemented.** | `MasternodesError` |
| `preview_update_registrar(wallet: WalletId, request: UpdateRegistrarRequest)` | async | `UpdateRegistrarPreview`: the masternode's terms as they read after the update (a `NewAddress` payout empty, MG-04 D41), the payload version, the fee and `total_debit` (the fee), the owner key that would sign (`OwnerKeySource`), which of the operator key, voting key and payout change (`payout_changes` is `None` when the wallet holds no payout of it, MG-04 D10), and `pose_bans` (a new operator key bans the masternode until the operator's service update). Issues, locks and signs nothing. Refusals as above, then the funding's `send.*`. **Implemented.** | `MasternodesError` |
| `update_registrar(wallet: WalletId, request: UpdateRegistrarRequest, grant: String)` | async | A version-2 ProUpRegTx under a `key_change` lease: the operator and voting keys kept from the synced list or set (an operator public key given or the keychain's), the payout named (MG-04 D10), funded and sorted as `register`'s, the payload signed by the masternode's owner key (a keychain key 0–19, else an attached `owner` key matched to its ProRegTx, MG-04 D11, D12), saved and handed off as `register` does. Returns the `ProTxOperation` (`kind: update_registrar`, `pro_tx_hash` the masternode's). **Implemented.** | `MasternodesError` |
| `operations(wallet: WalletId)` | async | Every `ProTxOperation` of the wallet, newest first, its state derived now (§5). Settled rows' input locks are released on the way (MG-04 D25); the follower also unlocks a `conflicted` row's collateral once no other operation names it (MG-04 D54). **Implemented.** | `MasternodesError` |
| `resend_operation(wallet: WalletId, operation: String, grant: String)` | async | Hands a `signed`, `not_sent` or `sent` row's saved bytes to the fence again (MG-04 D7): under its own lease, charged the row's fee, its inputs reserved again (`send.outpoint_unavailable` when another owner holds one). The same txid, so a resend of bytes that may be out is the fence's resend. An `update_share` row resends under a `SharedMasternode` lease (MG-04b D73), a `dissolve` row under a `Dissolution` lease charged its penalty and fee (MG-04b D80; 0 for a row of another share's standby, D87); while shared sending is off (§2.4) either refuses `masternodes.unsupported{feature: "shared_masternodes"}`, and after the state check either refuses `masternodes.update_superseded{operation}` while an `update_share` or `dissolve` of its masternode saved after it is `signed`, `not_sent`, `sent` or `confirmed`, or `masternodes.update_pending{operation}` while one saved before it is pending as `update_share`'s check reads it (MG-04b D23, D80). That refusal comes after the row is read, since the row's kind decides it: after `wallet_not_found`, the id's `invalid_argument`, `masternodes.in_flight` and `masternodes.operation_not_found`, and before the state check. An `update_registrar` row is refused while another update of its masternode saved no earlier is `signed`, `not_sent`, `sent` or `confirmed` (`masternodes.update_superseded{operation}`, MG-04 D43), or an earlier one is `sent` and not in a block (`masternodes.update_pending{operation}`, MG-04 D49). **Implemented.** | `MasternodesError` |
| `discard_operation(wallet: WalletId, operation: String)` | async | Deletes a `needs_signature`, `inputs_spent`, `signed` or `not_sent` row, unlocking its inputs and the collateral it locked (MG-04 D6). A row whose bytes may be out (`sent_at` set) is never deleted: `masternodes.operation_state`. **Implemented.** | `MasternodesError` |
| `release_collateral(wallet: WalletId, pro_tx_hash: String)` | async | Unlocks the collateral coin of one of the wallet's masternodes and remembers the release, so the follower leaves it unlocked (MG-04 D9). Sends nothing. `invalid_argument`, `masternodes.not_found` (not the wallet's), `masternodes.collateral_unavailable` (no resolved collateral). **Implemented.** | `MasternodesError` |
| `lock_collateral(wallet: WalletId, pro_tx_hash: String)` | async | Locks it again and forgets the release. Refuses as `release_collateral`. **Implemented.** | `MasternodesError` |

### 2.4 Shared masternodes (MG-04b, slices 1a, 1b, 2a, 2b, 2c and 2d)

v24's multi-party registration as dash-qt's session envelope runs it (MG-04b §2–§4): this wallet takes part as
coordinator or participant, and the coordinator signs last and sends the registration (slice 1b). Messages pass as
text (`SharedMessage.text`, Core's file name with it). A session id is 64 lower-case hex. The calls that produce bytes the network accepts
(`sign_shared_funding`, `send_shared_registration`, `abandon_shared_session`, `update_share`, `dissolve`,
`create_standby`, `export_standby`, `send_standby`, `start_dissolve_together`, `approve_maintenance`,
`send_maintenance`, and `resend_operation` of an `update_share` or `dissolve` row)
are behind one switch, on since MG-04b slice 1c (DEC-678,
MG-04b D71): turned off, `can_send` is false and each of them refuses `masternodes.unsupported{feature: "shared_masternodes"}` right after
the wallet check; `resend_operation` refuses it once it has read the row (§2.3). On, they hold the
wallet's coin fence from their coin read through the durable write and the hand-off. Fees as MG-04b D9: the coordinator pays the whole registration fee,
and each participant contributes exactly its share.

Refusals, in order (MG-04b §7): arguments (`invalid_argument`), `wallet_not_found` (naming no id),
`masternodes.unsupported{feature: "shared_masternodes"}` (sending off for the calls above; v24 not `active` for `start_shared`, `join_shared`,
`lock_shared_terms`, `approve_shared`, `sign_shared_funding`, `send_shared_registration`, `preview_update_share`,
`update_share`, `preview_dissolve`, `dissolve`, `create_standby`, `export_standby`, `send_standby`,
`start_dissolve_together`, `approve_maintenance`, `send_maintenance`, `start_key_change` and the import of a standby or a sigs message; `field:
"service"` for a coordinator service that isn't IPv4, MG-04b D5; `approve_shared` can only judge the service from the
session's terms, so it refuses it after the session checks below, and never for a session it already approved), the list (`masternodes.list_unavailable`,
`masternodes.shared_too_early`, `masternodes.shared_registration_unknown`), the session
(`masternodes.shared_session_not_found`, `masternodes.in_flight`, `masternodes.shared_stage{stage}`; then, for lock and
approve, `masternodes.shared_message{reason: "conflict"}` when the session isn't the copy the caller reviewed,
MG-04b D56; then, for lock, approve, sign and send, `shared_stage{stage: "dead"}` when a reserved input is no longer this
session's locked, unspent coin or another row holds it, MG-04b D17, D50, D69; then, for send, `shared_stage{stage:
"unsigned"}` while a participant's input is unsigned, MG-04b D71), the message
(`masternodes.shared_message{reason, share}`), Core's rules (`masternodes.protx_refused{reason}`, Core's reasons
verbatim), the consent checks (`masternodes.shared_funding_mismatch{reason, needed}`; at lock the fee floor's
`fee_low` before the coordinator's own-side checks, MG-04b §3.1 step 3), then the lease (`Platform`)
and the funding's `send.*`. No refusal repeats pasted text (MG-00 D36). Every call that changes a session sends §6's
event. A session's coins are locked from the save that records its contribution until the session is discarded or
settles (`confirmed`, `dead`, or `conflicted` once the conflict is 6 blocks deep or ChainLocked, MG-04b D70). Its keys
count as in use for §2.3's registrations too. A coin a session or an MG-04 operation holds stays locked whoever
unlocks it: another row's release skips it, and m1's `unlock_outpoints` refuses it (`invalid_argument`, MG-04b D69).

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `shared_capability(wallet: WalletId)` | async | `SharedCapability`: `v24` `active` once a version-3 list entry was seen (`masternodes.any_shared_seen`, MG-03 D30), else `unknown`; `evonode_shares` always `inactive` (MG-04b D4); `can_send` (§2.4's lead). **Implemented.** | `MasternodesError` |
| `shared_sessions(wallet: WalletId)` | async | Every `SharedSession` of the wallet, newest first, its stage derived now (§5). **Implemented.** | `MasternodesError` |
| `shared_session(wallet: WalletId, session: String)` | async | One `SharedSession`. **Implemented.** | `MasternodesError` |
| `start_shared(wallet: WalletId, request: StartSharedRequest)` | async | Coordinator: checks the request (2 to 8 shares, distinct trimmed labels, an IPv4 service, Core's share rules), picks the keychain owner, voting and operator keys (§2.3's rule), selects and locks confirmed coins for its share and the fee, issues its refund address, saves the draft and returns it with its Invitation. **Implemented.** | `MasternodesError` |
| `set_shared_fee(wallet: WalletId, session: String, fee: u64)` | async | Coordinator, `draft`, while no other party's contribution is in (else `shared_stage{stage: "replied"}`, MG-04b D33): re-selects its coins for share + the new fee (the old locks released in the same write), `revision` + 1. **Implemented.** | `MasternodesError` |
| `import_shared_message(wallet: WalletId, text: String, target: Option<String>)` | async | Applies a pasted or opened message by MG-04b §3.0's table: a new participant session from an Invitation (`invited`, no coin touched), or the next copy of a held one; a re-pasted copy changes nothing (`changed: false`). Signs nothing. A standby (one hex line of a type-10 transaction) is checked as MG-04b D89 lists, against the registration in the wallet's history and the synced list (`shared_message{reason: "not_standby"}` first for a payload that doesn't decode, which names the masternode; `shared_registration_unknown`, `shared_too_early`, then `not_standby`, Core's rules as `protx_refused{reason}` with `bad-prodis-sig` for its signature, `shared_message{reason: "outputs"}`, then `invalid_argument` for a fee below the relay minimum), stored (`source: imported`, D84) and described (`SharedImport::Standby{description, stored}`; `stored: false` when the wallet holds that variant already). A `dissolve` sigs envelope, or a bare signature array for the dissolve-together session named by `target`, is checked as MG-04b D92 lists against the registration in the wallet's history and the synced list (`shared_registration_unknown`, `shared_too_early`, or `shared_stage` with the stored session's stage when it is past `signing` and `combined`, a late reply; then `shared_message{reason}`: `not_dissolution`, `other_masternode`, `signed`, `outputs`, `other_session` for another request or masternode than the target's; then `invalid_argument` below the relay minimum of the combined size, D96); its signatures are taken one by one, each verified by its share's owner key, and the rest skipped with `SharedWarning::SkippedSignatures` (§2.5's rules; `NoApprovals` for a `signatures` value that isn't an array); the session is created (`participant`) or updated, `changed` saying whether the stored session changed (a signature taken, or this wallet's shares of it). A bare array's `received_code` is its own JSON's fingerprint. A `registrar` sigs envelope, or a bare signature array for the key-change session named by `target`, is checked as MG-04b D104 lists: `shared_message{reason: "other_session"}` first for another request (another sign hash: keys or fee inputs) or masternode than the target's; then against the registration in the wallet's history and the synced list (`shared_registration_unknown`, `shared_too_early`, or a late reply's stage as a dissolution's; then `shared_message{reason}`: `not_key_change` for a transaction that isn't a version-3 type-12 ProUpSharedRegTx, `other_masternode`, Core's trivial rules as `protx_refused{reason}`, a signature count other than the share count as `protx_refused{reason: "bad-proupsharedreg-sig-count"}`, and `inputs_hash` when the payload's `inputsHash` isn't its inputs'); its signatures are taken as a dissolution's, verified over `SerializeHash` of the payload without them; the session is created (`participant`, its `base_keys` the synced list's keys at that import, D25) or updated. A bare array without a target is `shared_message{reason: "parse"}`, and a target of another kind `invalid_argument`. **Implemented.** | `MasternodesError` |
| `join_shared(wallet: WalletId, session: String, request: JoinSharedRequest)` | async | Participant, `invited`: fills the named share (or the single open one), picks its owner key, issues its refund address, locks coins for the share, records its contribution, returns the Details. **Implemented.** | `MasternodesError` |
| `shared_message(wallet: WalletId, session: String)` | async | `SharedMessage`: the message to copy now, its code and Core's file name (`shared-mn-session-<CODE>.json`). **Implemented.** | `MasternodesError` |
| `lock_shared_terms(wallet: WalletId, session: String, revision: u32, grant: String)` | async | Coordinator, `draft`, at `revision`: the `SharedSession.revision` the coordinator reviewed, from the session record (`shared_session`, or `mn shared show`); another revision now (a reply absorbed since) is `shared_message{reason: "conflict"}` (MG-04b D56). Then every row and contribution in (else `shared_stage{stage: "incomplete"}`): freezes the registration, runs MG-04b §3.1 step 3's checks and the fee floor (`shared_funding_mismatch{reason: "fee_low", needed}`), checks its reservation, signs its shares' consent (charged 0), returns Locked Terms. **Implemented.** | `MasternodesError` |
| `unlock_shared_terms(wallet: WalletId, session: String)` | async | Coordinator: back to `draft` before `combined`, the approvals dropped, `revision` + 1. **Implemented.** | `MasternodesError` |
| `approve_shared(wallet: WalletId, session: String, consent_hash: String, grant: String)` | async | Participant, `signing`, of `consent_hash`: the `SharedSession.consent_hash` the participant reviewed, from the session record (64 hex, compared ignoring case; another shape is `invalid_argument`). It binds what is signed: a re-lock that changes only envelope-side text keeps the hash; another one now (a re-locked copy imported since, MG-04b M-1 case 2) is `shared_message{reason: "conflict"}` and signs nothing (MG-04b D56). Then MG-04b §4's checks on the frozen registration against its recorded side, then its shares' consent signatures (charged 0), returns the Approval; approving again returns the same. **Implemented.** | `MasternodesError` |
| `sign_shared_funding(wallet: WalletId, session: String, grant: String)` | async | Participant, `combined`: §4's checks and every approval verified, and a copy whose consent hash isn't the one this wallet approved refused `masternodes.shared_message{reason: "conflict"}` (MG-04b D65), then its inputs signed (charged `SharedSession.debit`, refunded if signing fails), `out_at` written durably with the signed copy before anything returns, and the Signed Contribution returned. Nothing is handed to the network: the coordinator sends. **Implemented.** | `MasternodesError` |
| `send_shared_registration(wallet: WalletId, session: String, grant: String)` | async | Coordinator, `combined`: §4's checks, every participant's input signed (else `shared_stage{stage: "unsigned"}`, MG-04b D24, D71), then its own inputs signed last (charged `SharedSession.debit`: its share and the whole fee), `out_at` written durably with the signed registration, and the registration handed off as MG-04's rows are (`TxDraft`'s fence). A failed hand-off keeps `out_at` and records its code in `last_error`. At `sent`, a resend of the stored bytes, charged `debit` (refunded when the fence runs it as a resend of bytes that may be out), refused `shared_stage{stage: "abandoned"}` once the session was abandoned. Returns the Final Record. **Implemented.** | `MasternodesError` |
| `abandon_shared_session(wallet: WalletId, session: String, grant: String)` | async | A session with `out_at` set, not `confirmed` or `conflicted` (else `shared_stage`): spends its largest unspent reserved coin back to a new change address of the wallet at the relay-minimum fee (charged), `abandon_txid` written durably first, so the registration can no longer be mined. The row and its locks stay until the follower settles it. A coordinator whose registration the wallet recorded abandons that transaction first (MG-04 D49). With no reserved coin unspent and free of other pending spends, `shared_stage` naming the caller's stage while out (`sent` for the coordinator, `funding_signed` for a participant); a coin worth no more than the fee and dust, `send.amount_with_fee_exceeds_balance`. Returns the session. Reads the stored row, so a session whose envelope no longer parses can be abandoned. A key-change session (MG-04b D102): only its funder's (`shared_stage{stage: "participant"}` otherwise), and not `confirmed`, `conflicted` or `dead` (`shared_stage`); before `out_at` nothing is spent and nothing charged: its reservation and locks are released in one durable write, the row kept, reading `dead{reason: abandoned}` (no other wallet can finish the request: its fee inputs are signed by the funder alone, after the signatures are inserted, R3); with `out_at` set, as a registration's above. **Implemented.** | `MasternodesError` |
| `discard_shared_session(wallet: WalletId, session: String)` | async | Deletes a session nothing of which may be out and unlocks its coins; one with `out_at` set is `shared_stage` (MG-04b D17). Both read the stored row, so a session whose envelope no longer parses can be discarded (MG-04b D57). A dissolve-together session is `shared_too_early` while its masternode is out of the synced list and the wallet's filter scan is below the list's height (the dissolution may be its own, MG-04b D94); one marked out is deleted when no operation holds its txid (never handed off), else `shared_stage` (MG-04b D95). A key-change session with `out_at` set is `shared_stage` with its stage (MG-04b D102). **Implemented.** | `MasternodesError` |
| `preview_update_share(wallet: WalletId, request: UpdateShareRequest)` | async | `UpdateSharePreview`: the ProUpShareTx this wallet would build for one of its shares of a confirmed registration of its sessions, Core's §5.1 rules run on it, its fee. `share_not_mine`, `shared_too_early` (tip ≤ the registration's height, or the masternode not in the synced list, MG-04b m-16), `shared_registration_unknown`. Issues, locks and signs nothing. **Implemented.** | `MasternodesError` |
| `update_share(wallet: WalletId, request: UpdateShareRequest, grant: String)` | async | The ProUpShareTx `preview_update_share` shows, under a `SharedMasternode` lease charged its fee: a `NewAddress` reward issued, the payload signed by the share's keychain owner key, its fee inputs by the wallet's, saved as a `ProTxOperation` of kind `update_share` with its input locks and handed off as §2.3's rows are. Refuses as `preview_update_share`, except that `masternodes.update_pending{operation}`, while another `update_share` or a `dissolve` of the masternode is `signed`, `not_sent` or `sent`, or `confirmed` in a block not yet buried (ChainLocked or 6 confirmations), comes after `list_unavailable` and before `shared_too_early`, `shared_registration_unknown` and `share_not_mine` (MG-04b D23, D73, D80); `share_not_mine` also when the keychain key's signature isn't the share's owner key. **Implemented.** | `MasternodesError` |
| `preview_dissolve(wallet: WalletId, request: DissolveRequest)` | async | `DissolvePreview`: the unilateral ProDisTx this wallet would send now for one of its shares of a confirmed registration of its sessions, Core's `BuildDissolution` output with the share as actor and `DissolveShared`'s penalty at the list's height + 1 (MG-04b D79): `early`, the penalty, the fee, `valid_from_height` (the first penalty-free height, D82), every share's return in share order (the actor's `amount` 0 when its output is omitted), `warnings` (`actor_output_dust`, an actor output below dust, which won't relay) and `total_debit` (the penalty and the fee). `invalid_argument` with Core's texts, before the wallet, for a fee of 0 or above 1,000,000 duffs. Then, in order: `shared_registration_unknown` (no confirmed session of the wallet registered it), `shared_too_early` (the synced list isn't a block past the registration, or doesn't hold it), `share_not_mine` (the share isn't the wallet's), `invalid_argument` for a penalty and fee above the share (Core's text) and for a fee below the relay minimum for the exact size, and Core's rules (`protx_refused`). Signs nothing. **Implemented.** | `MasternodesError` |
| `dissolve(wallet: WalletId, request: DissolveRequest, grant: String)` | async | The ProDisTx `preview_dissolve` shows, under a `Dissolution` lease charged the penalty and the fee: signed by the share's keychain owner key over `MakeSignHash(tx, 1)` (the vault reads the share table from the registration in the wallet's history, MG-04b D29), Core's rules run on the signed copy, saved as a `ProTxOperation` of kind `dissolve` with no fee inputs and no locks (MG-04b D80) and handed off as §2.3's rows are. Refuses as `preview_dissolve`, with `update_pending` as `update_share`'s; then `invalid_argument` for an actor output below dust, which the preview only warns of (MG-04b D83); and `share_not_mine` when the keychain key isn't the actor share's. **Implemented.** | `MasternodesError` |
| `create_standby(wallet: WalletId, pro_tx_hash: String, fee: u64, grant: String)` | async | Signs Core's standby pair paying `fee` (`createStandby`: the full-principal variant at penalty 0 and the immediate one at `nEarlyPenalty`; one copy at penalty 0, MG-04b m-20) for each share of the masternode whose owner key is a keychain owner key of the wallet, the registration read from the wallet's history (MG-04b D86), under one `Dissolution` lease charged each share's penalty and fee; each variant passes the import's check, then is stored, synced, before the call returns the masternode's `Vec<Standby>`. Signing again stores nothing new (MG-04b D84). `invalid_argument` for the fee as `dissolve`'s, before the wallet; then `shared_registration_unknown`, `shared_too_early` (m-16), `share_not_mine` (no share of the wallet), `invalid_argument` for Core's builder refusals and a fee below the relay minimum, then the lease. **Implemented.** | `MasternodesError` |
| `standby(wallet: WalletId, pro_tx_hash: String)` | async | The wallet's stored standbys of the masternode by share and fee, as `Standby` (which variants are held, `full_valid_from` from the registration's current block, the source): never their bytes (MG-04b m-22). **Implemented.** | `MasternodesError` |
| `export_standby(wallet: WalletId, pro_tx_hash: String, share: u16, fee: u64, grant: String)` | async | `StandbyFile`: Core's file name (`standby-dissolution-<first 8 of the hash>-share<k>.txt`) and text (`standbyFileText`; MG-04b D90's dates and unit) of the standby of `share` paying `fee`, under a `MasternodeOp` grant charged 0 (MG-04b D21). `invalid_argument` when no such standby is stored; then the registration and list as `create_standby`'s, then the lease. **Implemented.** | `MasternodesError` |
| `send_standby(wallet: WalletId, pro_tx_hash: String, share: u16, fee: u64, variant: StandbyVariant, grant: String)` | async | Hands a stored variant to the network as a `ProTxOperation` of kind `dissolve`, as `dissolve` does (MG-04b D87): `update_pending` as `dissolve`'s; `invalid_argument` when no such standby is stored; the registration and list as `create_standby`'s; `shared_stage{stage: "full_valid"}` for the immediate variant once the full one is valid and stored (m-20, D88); `shared_too_early` for the full one before it is valid (D85); `invalid_argument` for a variant not stored; the stored bytes checked again as an import, `invalid_argument` for a fee below the relay minimum; `invalid_argument` for a dust actor output (D83); then a `Dissolution` lease charged the variant's penalty and the fee, or 0 when the actor share isn't the wallet's (a `foreign` row, whose resend is charged 0 too, and which settles by the list as an unseen dissolution does, MG-04b D87). Validity is judged at the tip: the list's height, or the headers' when the list lags them (D85). **Implemented.** | `MasternodesError` |
| `start_dissolve_together(wallet: WalletId, request: DissolveRequest, grant: String)` | async | Prepares the masternode's unanimous ProDisTx as `protx shared_dissolve_prepare` does (`BuildDissolution` with penalty 0 and `request.fee` from the actor's share, MG-04b D92) with `request.share_index` the actor, a share of this wallet (`share_not_mine`), and signs every share of this wallet over `MakeSignHash(tx, n)` under one `Dissolution` lease charged the fee (§6.3; the vault reads the share table from the registration in the history, MG-04b D29). `update_pending` as `dissolve`'s (MG-04b D23); the relay minimum of the combined size (`invalid_argument`, D96); a dust actor output refused (`invalid_argument`, D83); Core's rules on the combined shape. Returns `SharedOutcome`: the session (kind `dissolve_together`, this wallet its coordinator, id m-17's, D93) and its `Request`. The same request again signs what is missing and returns the session, its role kept (a request imported first stays this wallet's as participant, its message the `Reply`). **Implemented.** | `MasternodesError` |
| `approve_maintenance(wallet: WalletId, session: String, grant: String, accept_actor_fee: bool)` | async | A dissolve-together session at `signing` (else `shared_stage`, `dead` for a delisted masternode), then `update_pending` as `dissolve`'s: the request re-checked (D92) and the relay minimum, then every share of this wallet not yet signed is signed under one `Dissolution` lease charged the fee when the actor share is among them, else 0 (§6.3). Signing the actor share here (the preparer signed its own at start, so only a request another wallet prepared) is `masternodes.shared_actor_fee{fee}` unless `accept_actor_fee`, or unless its fee is at most the relay minimum (MG-04b m-15, D96). Nothing left to sign returns the session uncharged. Returns `SharedOutcome` with the `Reply`. A key-change session at `signing` (else `shared_stage`; `stale`, the list's keys no longer its base, is `shared_message{reason: "stale"}`, MG-04b D25), then `update_pending` as below, the request re-checked (D104), `share_not_mine` when no share is this wallet's, then this wallet's unsigned shares signed under one `SharedMasternode` lease charged 0 (§6.3, MG-04b D100); `accept_actor_fee` isn't read. `invalid_argument` for a session of another kind. **Implemented.** | `MasternodesError` |
| `start_key_change(wallet: WalletId, request: KeyChangeRequest, grant: String)` | async | Prepares a shared masternode's ProUpSharedRegTx as `protx shared_update_registrar_prepare` does (`PrepareSharedRegistrarUpdate`, MG-04b §3.3): each key kept (copied from the synced list, the operator key in its basic serialization) or set by §2.3's pickers (the operator key basic; the voting key a P2PKH address, as dash-qt requires, and never a share's owner key, `masternodes.shared_voting`, MG-04 D52); the fee inputs picked from confirmed coins under the coin fence, change to a new address; Core's rules on the request with empty signatures (`protx_refused{reason}`: `bad-protx-dup-key`, `bad-proupsharedreg-payee-reuse` and the rest); the synced list's keys recorded as the session's base (D25); every share of this wallet signed over `SerializeHash` of the payload under one `SharedMasternode` lease charged 0 (§6.3, MG-04b D100); then the session written with its fee inputs reserved and locked in one durable write. In order: `invalid_argument` for a malformed hash, `wallet_not_found`, `unsupported` (v24), `list_unavailable`, `update_pending` (below), `shared_registration_unknown`, `shared_too_early`, `share_not_mine`, the pickers' refusals (`protx_refused{reason: "protx-dup", operation}` for an operator key a pending operation or key change of the wallet names, MG-04 D51), `invalid_argument` for a request that changes no key, the funding's `send.*`, Core's rules (a kept operator key the list holds as the null key a revocation leaves is `bad-protx-key-null`), then the lease. A request equal to one this wallet abandoned before its send (the same keys on the coins it released) replaces that session (MG-04b D102). Returns `SharedOutcome`: the session (kind `key_change`, this wallet its funder as `coordinator`, id m-17's over kind `registrar`, MG-04b D99) and its `Request`. Not behind the sending switch: it signs no input. A key-change session's requested keys count as in use for §2.3's and §2.4's key pickers until it is discarded, and its operator key as a pending claim while it may go out and isn't yet listed (MG-04b D99). **Implemented.** | `MasternodesError` |
| `send_maintenance(wallet: WalletId, session: String, grant: String)` | async | A dissolve-together session at `combined` (else `shared_stage`, `dead` for a delisted masternode), then `update_pending` as `dissolve`'s: the signatures inserted in share order (`InsertUnanimousSignatures`), Core's rules with them, the relay minimum and D83; under a `Dissolution` lease charged 0, `signed_tx`, `txid` and `out_at` written durably, then handed off as a `ProTxOperation` of kind `dissolve` (penalty 0; `foreign` when the actor share isn't this wallet's; its resend charged 0, MG-04b D95). Any wallet holding every signature may send. Returns `MaintenanceSent::Operation{operation}`. A key-change session (MG-04b D99): its funder's only (`shared_stage{stage: "participant"}` otherwise, as Core's `CombineShared` requires the wallet that funded it), at `combined` (else `shared_stage`; `stale` is `shared_message{reason: "stale"}`), then `update_pending` (below); the signatures inserted in share order, the fee inputs, still this session's locked unspent coins, signed again over the result (SIGHASH_ALL, `Finish`), Core's rules with the signatures, under one `SharedMasternode` lease charged the fee (§6.3, D100); `signed_tx`, `txid` and `out_at` written durably, then handed off as `send_shared_registration`'s registration (`TxDraft`'s fence). The session is the record: its locks stay until it settles and `abandon_shared_session` is its exit (D99, D102). At `sent`, a resend of the stored bytes, charged the fee (refunded when the fence runs it as a resend of bytes that may be out); a request out whose base the list left for other keys than its own reads `stale` and isn't resent (`shared_message{reason: "stale"}`; its exit is the abandon, MG-04b D25). Returns `MaintenanceSent::Session{session}`. **Implemented.** | `MasternodesError` |

## 3. Surface (generated)

Generated by `mn_masternodes_contract.rs` from the facade's source files; `DW_BLESS=1` rewrites it after
`Contract-Version` is raised.

<!-- BEGIN GENERATED: masternodes-surface -->
<!-- surface-sha256: cb5bf9c3a148008a3fb98a33f5fe4687f254935fc4fbd95872e43aa59cfc29c0 version: 17 -->

```rust
// src/masternodes/errors.rs
#[derive(Debug, thiserror::Error)]
pub enum MasternodesError {
    ListUnavailable,
    NotFound,
    AlreadyTracked,
    NotTracked,
    KeyMismatch { role: AttachedKeyRole },
    NotEvonode,
    NoIdentity,
    PlatformUnavailable { detail: String },
    Unsupported { feature: String, field: Option<String> },
    NotImplemented { call: String },
    ProtxRefused { reason: String, operation: Option<String> },
    CollateralUnavailable,
    SignatureMismatch,
    OperationNotFound,
    OperationState { state: String },
    InFlight,
    KeysExhausted { role: String },
    NoOwnerKey,
    UpdatePending { operation: String },
    UpdateSuperseded { operation: String },
    SharedSessionNotFound,
    SharedStage { stage: String },
    SharedMessage { reason: String, share: Option<u16> },
    SharedFundingMismatch { reason: String, needed: Option<u64> },
    ShareNotMine,
    SharedTooEarly,
    SharedRegistrationUnknown,
    SharedActorFee { fee: u64 },
    Platform(PlatformError),
    Engine(EngineError),
}
impl MasternodesError {
    pub fn code(&self) -> &'static str;
    pub fn engine(&self) -> Option<&EngineError>;
}
impl From<dw_vault::VaultError> for MasternodesError;
impl From<tokio::task::JoinError> for MasternodesError;
// src/masternodes/mod.rs
pub use errors::MasternodesError;
pub use protx_records::{ CollateralChoice, CollateralKind, EvonodeTerms, KeepOr, KeyPick, OperatorChoice, OwnerKeySource, PayoutChoice, PlatformFacts, PlatformNodeChoice, ProTxKeys, ProTxOperation, ProTxOperationKind, ProTxOperationState, ProTxTerms, RegistrationPreview, RegistrationRequest, SignRequest, UpdateRegistrarPreview, UpdateRegistrarRequest, VotingChoice, };
pub use records::{ AttachedKey, AttachedKeyRole, CollateralCoin, CollateralState, EvonodeCredits, HeldShares, KeyCheck, ListFacts, ListedMasternode, MasternodeDetail, MasternodeListState, MasternodeOverview, MasternodePayout, MasternodeRoles, MasternodeRow, MasternodeStatus, MasternodeType, RegistrationFacts, UntrackOutcome, };
pub use shared_records::{ Activation, DeadReason, DissolvePreview, DissolveRequest, JoinSharedRequest, KeyBase, KeyChangeRequest, KeyChangeView, MaintenanceSent, RewardChoice, ShareReturn, ShareRow, ShareSlot, ShareTerms, ShareTermsView, SharedCapability, SharedImport, SharedKind, SharedMessage, SharedNext, SharedOutcome, SharedRole, SharedSession, SharedStage, SharedWarning, Standby, StandbyDescription, StandbyFile, StandbySource, StandbyVariant, StartSharedRequest, UpdateSharePreview, UpdateShareRequest, };
pub use tracking::MAX_MASTERNODE_LABEL;
#[derive(Clone)]
pub struct Masternodes {
    ..
}
impl NetworkSession {
    pub fn masternodes(self: &Arc<Self>) -> Masternodes;
}
// src/masternodes/protx.rs
impl Masternodes {
    pub async fn registration_keys(&self, wallet: WalletId, kind: MasternodeType) -> Result<ProTxKeys, MasternodesError>;
    pub async fn preview_registration(&self, wallet: WalletId, request: RegistrationRequest) -> Result<RegistrationPreview, MasternodesError>;
    pub async fn register(&self, wallet: WalletId, request: RegistrationRequest, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn prepare_external_registration(&self, wallet: WalletId, request: RegistrationRequest) -> Result<ProTxOperation, MasternodesError>;
    pub async fn submit_external_registration(&self, wallet: WalletId, operation: String, signature: String, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn refresh_external_registration(&self, wallet: WalletId, operation: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn preview_update_registrar(&self, wallet: WalletId, request: UpdateRegistrarRequest) -> Result<UpdateRegistrarPreview, MasternodesError>;
    pub async fn update_registrar(&self, wallet: WalletId, request: UpdateRegistrarRequest, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn operations(&self, wallet: WalletId) -> Result<Vec<ProTxOperation>, MasternodesError>;
    pub async fn resend_operation(&self, wallet: WalletId, operation: String, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn discard_operation(&self, wallet: WalletId, operation: String) -> Result<(), MasternodesError>;
    pub async fn release_collateral(&self, wallet: WalletId, pro_tx_hash: String) -> Result<(), MasternodesError>;
    pub async fn lock_collateral(&self, wallet: WalletId, pro_tx_hash: String) -> Result<(), MasternodesError>;
}
// src/masternodes/protx_records.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollateralChoice {
    FundNew,
    ExistingUtxo { outpoint: String },
    External { outpoint: String, address: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollateralKind {
    FundNew,
    ExistingUtxo,
    External,
}
impl CollateralChoice {
    pub fn kind(&self) -> CollateralKind;
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperatorChoice {
    Given { public_key: String },
    Keychain { index: u32 },
    NextUnused,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VotingChoice {
    Keychain { index: u32 },
    Address { address: String },
    NextUnused,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayoutChoice {
    NewAddress,
    Address { address: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlatformNodeChoice {
    Given { node_id: String },
    Keychain { index: u32 },
    NextUnused,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvonodeTerms {
    pub node: PlatformNodeChoice,
    pub p2p_port: Option<u16>,
    pub http_port: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRequest {
    pub kind: MasternodeType,
    pub collateral: CollateralChoice,
    pub service: String,
    pub operator: OperatorChoice,
    pub operator_reward: u16,
    pub owner_index: Option<u32>,
    pub voting: VotingChoice,
    pub payout: PayoutChoice,
    pub evonode: Option<EvonodeTerms>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformFacts {
    pub node_id: String,
    pub p2p_port: u16,
    pub http_port: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProTxTerms {
    pub masternode_type: MasternodeType,
    pub version: u16,
    pub service: String,
    pub owner_address: String,
    pub owner_index: Option<u32>,
    pub voting_address: String,
    pub voting_index: Option<u32>,
    pub operator_public_key: String,
    pub operator_index: Option<u32>,
    pub operator_reward: u16,
    pub payout_address: String,
    pub collateral: Option<CollateralKind>,
    pub collateral_outpoint: Option<String>,
    pub collateral_address: Option<String>,
    pub platform: Option<PlatformFacts>,
    pub platform_node_index: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationPreview {
    pub terms: ProTxTerms,
    pub fee: u64,
    pub total_debit: u64,
    pub collateral_amount: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum KeepOr<T> {
    Keep,
    Set(T),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRegistrarRequest {
    pub pro_tx_hash: String,
    pub operator: KeepOr<OperatorChoice>,
    pub voting: KeepOr<VotingChoice>,
    pub payout: PayoutChoice,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerKeySource {
    Keychain { index: u32 },
    Attached,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRegistrarPreview {
    pub terms: ProTxTerms,
    pub version: u16,
    pub fee: u64,
    pub total_debit: u64,
    pub owner_key: OwnerKeySource,
    pub operator_changes: bool,
    pub voting_changes: bool,
    pub payout_changes: Option<bool>,
    pub pose_bans: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProTxKeys {
    pub owner: KeyPick,
    pub voting: KeyPick,
    pub operator: KeyPick,
    pub platform_node: Option<KeyPick>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPick {
    pub index: u32,
    pub address: Option<String>,
    pub public_key: Option<String>,
    pub node_id: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProTxOperationKind {
    Registration,
    UpdateRegistrar,
    UpdateShare,
    Dissolve,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProTxOperationState {
    NeedsSignature,
    InputsSpent,
    Signed,
    NotSent,
    Sent,
    Confirmed { height: u32 },
    Conflicted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignRequest {
    pub address: String,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProTxOperation {
    pub id: String,
    pub kind: ProTxOperationKind,
    pub pro_tx_hash: Option<String>,
    pub txid: Option<String>,
    pub state: ProTxOperationState,
    pub sign_request: Option<SignRequest>,
    pub fee: u64,
    pub created_at: u64,
    pub signed_at: Option<u64>,
    pub sent_at: Option<u64>,
    pub last_error: Option<String>,
    pub terms: ProTxTerms,
}
// src/masternodes/reads.rs
impl Masternodes {
    pub fn list_state(&self) -> Result<MasternodeListState, MasternodesError>;
    pub async fn list(&self, wallet: WalletId) -> Result<MasternodeOverview, MasternodesError>;
    pub async fn masternode(&self, wallet: WalletId, pro_tx_hash: String) -> Result<MasternodeDetail, MasternodesError>;
    pub async fn payouts(&self, wallet: WalletId, pro_tx_hash: String, since: Option<u64>) -> Result<Vec<MasternodePayout>, MasternodesError>;
    pub async fn find(&self, query: String) -> Result<Vec<ListedMasternode>, MasternodesError>;
    pub async fn evonode_credits(&self, pro_tx_hash: String) -> Result<EvonodeCredits, MasternodesError>;
    pub async fn resolve_collateral(&self, pro_tx_hash: String) -> Result<String, MasternodesError>;
}
// src/masternodes/records.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeType {
    Regular,
    Evonode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeStatus {
    Enabled,
    PoseBanned,
    Removed,
    Unconfirmed,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachedKeyRole {
    Owner,
    Voting,
    Payout,
}
impl AttachedKeyRole {
    pub fn vault_name(self) -> &'static str;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCheck {
    Matched,
    KeyMismatch,
    Unchecked,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedKey {
    pub role: AttachedKeyRole,
    pub key_id: Option<String>,
    pub check: KeyCheck,
}
impl AttachedKey {
    pub fn holds(&self) -> bool;
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollateralState {
    Known { outpoint: String },
    Unresolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeRoles {
    pub collateral: bool,
    pub owner: bool,
    pub voting: bool,
    pub payout: bool,
    pub operator: bool,
    pub attached: Option<Vec<AttachedKey>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFacts {
    pub kind: MasternodeType,
    pub valid: bool,
    pub service: Option<String>,
    pub operator_public_key: String,
    pub voting_address: String,
    pub platform_node_id: Option<String>,
    pub platform_http_port: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationFacts {
    pub registered_height: Option<u32>,
    pub collateral: CollateralState,
    pub owner_address: Option<String>,
    pub voting_address: Option<String>,
    pub payout_address: Option<String>,
    pub payout_script: Option<String>,
    pub operator_reward: Option<u16>,
    pub revoked: bool,
    pub revocation_reason: Option<u16>,
    pub transactions: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodePayout {
    pub txid: String,
    pub record_index: u32,
    pub amount: u64,
    pub address: String,
    pub height: Option<u32>,
    pub timestamp: Option<u64>,
    pub matured: bool,
    pub unattributed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldShares {
    pub held: u8,
    pub of: u8,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeRow {
    pub pro_tx_hash: String,
    pub kind: Option<MasternodeType>,
    pub type_index: u32,
    pub label: Option<String>,
    pub status: MasternodeStatus,
    pub banned_seen_at: Option<u64>,
    pub service: Option<String>,
    pub mine: bool,
    pub tracked: bool,
    pub shared_unsupported: bool,
    pub shares: Option<HeldShares>,
    pub roles: MasternodeRoles,
    pub collateral: CollateralState,
    pub payout_address: Option<String>,
    pub payouts_30d: u64,
    pub payouts_incomplete: bool,
    pub last_payout: Option<MasternodePayout>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeListState {
    pub synced: bool,
    pub height: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollateralCoin {
    pub outpoint: String,
    pub address: String,
    pub amount: u64,
    pub kind: MasternodeType,
    pub confirmations: u32,
    pub user_locked: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeOverview {
    pub list: MasternodeListState,
    pub masternodes: Vec<MasternodeRow>,
    pub collateral_coins: Vec<CollateralCoin>,
    pub payouts_30d: u64,
    pub unattributed_30d: u64,
    pub payouts_incomplete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasternodeDetail {
    pub row: MasternodeRow,
    pub list: Option<ListFacts>,
    pub registration: Option<RegistrationFacts>,
    pub payout_checked_height: Option<u32>,
    pub tracked_at: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedMasternode {
    pub pro_tx_hash: String,
    pub facts: ListFacts,
    pub tracked: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntrackOutcome {
    pub keys_deleted: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvonodeCredits {
    pub pro_tx_hash: String,
    pub credits: u64,
    pub duffs: u64,
    pub fetched_at: u64,
}
// src/masternodes/shared.rs
impl Masternodes {
    pub async fn shared_capability(&self, wallet: WalletId) -> Result<SharedCapability, MasternodesError>;
    pub async fn shared_sessions(&self, wallet: WalletId) -> Result<Vec<SharedSession>, MasternodesError>;
    pub async fn shared_session(&self, wallet: WalletId, session: String) -> Result<SharedSession, MasternodesError>;
    pub async fn start_shared(&self, wallet: WalletId, request: StartSharedRequest) -> Result<SharedOutcome, MasternodesError>;
    pub async fn set_shared_fee(&self, wallet: WalletId, session: String, fee: u64) -> Result<SharedOutcome, MasternodesError>;
    pub async fn import_shared_message(&self, wallet: WalletId, text: String, target: Option<String>) -> Result<SharedImport, MasternodesError>;
    pub async fn join_shared(&self, wallet: WalletId, session: String, request: JoinSharedRequest) -> Result<SharedOutcome, MasternodesError>;
    pub async fn shared_message(&self, wallet: WalletId, session: String) -> Result<SharedMessage, MasternodesError>;
    pub async fn lock_shared_terms(&self, wallet: WalletId, session: String, revision: u32, grant: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn unlock_shared_terms(&self, wallet: WalletId, session: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn approve_shared(&self, wallet: WalletId, session: String, consent_hash: String, grant: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn sign_shared_funding(&self, wallet: WalletId, session: String, grant: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn send_shared_registration(&self, wallet: WalletId, session: String, grant: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn abandon_shared_session(&self, wallet: WalletId, session: String, grant: String) -> Result<SharedSession, MasternodesError>;
    pub async fn discard_shared_session(&self, wallet: WalletId, session: String) -> Result<(), MasternodesError>;
    pub async fn preview_update_share(&self, wallet: WalletId, request: UpdateShareRequest) -> Result<UpdateSharePreview, MasternodesError>;
    pub async fn update_share(&self, wallet: WalletId, request: UpdateShareRequest, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn preview_dissolve(&self, wallet: WalletId, request: DissolveRequest) -> Result<DissolvePreview, MasternodesError>;
    pub async fn dissolve(&self, wallet: WalletId, request: DissolveRequest, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn create_standby(&self, wallet: WalletId, pro_tx_hash: String, fee: u64, grant: String) -> Result<Vec<Standby>, MasternodesError>;
    pub async fn standby(&self, wallet: WalletId, pro_tx_hash: String) -> Result<Vec<Standby>, MasternodesError>;
    pub async fn export_standby(&self, wallet: WalletId, pro_tx_hash: String, share: u16, fee: u64, grant: String) -> Result<StandbyFile, MasternodesError>;
    pub async fn send_standby(&self, wallet: WalletId, pro_tx_hash: String, share: u16, fee: u64, variant: StandbyVariant, grant: String) -> Result<ProTxOperation, MasternodesError>;
    pub async fn start_dissolve_together(&self, wallet: WalletId, request: DissolveRequest, grant: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn start_key_change(&self, wallet: WalletId, request: KeyChangeRequest, grant: String) -> Result<SharedOutcome, MasternodesError>;
    pub async fn approve_maintenance(&self, wallet: WalletId, session: String, grant: String, accept_actor_fee: bool) -> Result<SharedOutcome, MasternodesError>;
    pub async fn send_maintenance(&self, wallet: WalletId, session: String, grant: String) -> Result<MaintenanceSent, MasternodesError>;
}
// src/masternodes/shared_records.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Active,
    Inactive,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedCapability {
    pub v24: Activation,
    pub evonode_shares: Activation,
    pub can_send: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareTerms {
    pub service: String,
    pub operator: OperatorChoice,
    pub operator_reward: u16,
    pub voting: VotingChoice,
    pub early_period_blocks: u32,
    pub early_penalty: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSlot {
    pub label: String,
    pub amount: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSharedRequest {
    pub shares: Vec<ShareSlot>,
    pub terms: ShareTerms,
    pub fee: u64,
    pub reward: RewardChoice,
    pub node_run_by: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RewardChoice {
    Refund,
    NewAddress,
    Address { address: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinSharedRequest {
    pub share_index: Option<u16>,
    pub reward: RewardChoice,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedRole {
    Coordinator,
    Participant,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedKind {
    Registration,
    DissolveTogether,
    KeyChange,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedStage {
    Invited,
    Draft,
    Signing,
    Combined,
    FundingSigned,
    Sent,
    Confirmed { height: u32 },
    Conflicted,
    Stale,
    Dead { reason: DeadReason },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadReason {
    InputsSpent,
    Delisted,
    Abandoned,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedNext {
    Join,
    Copy,
    Wait,
    Lock,
    Approve,
    Sign,
    Send,
    CreateStandby,
    Abandon,
    Nothing,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRow {
    pub index: u16,
    pub label: String,
    pub amount: u64,
    pub owner_address: Option<String>,
    pub refund_address: Option<String>,
    pub reward_address: Option<String>,
    pub mine: bool,
    pub contributed: bool,
    pub approved: bool,
    pub funding_signed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareTermsView {
    pub service: String,
    pub service_read: Option<String>,
    pub operator_public_key: String,
    pub voting_address: String,
    pub operator_reward: i32,
    pub early_period_blocks: u32,
    pub early_penalty: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBase {
    pub operator_public_key: String,
    pub voting_address: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChangeView {
    pub requested: KeyBase,
    pub current: Option<KeyBase>,
    pub funder: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChangeRequest {
    pub pro_tx_hash: String,
    pub operator: KeepOr<OperatorChoice>,
    pub voting: KeepOr<VotingChoice>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaintenanceSent {
    Operation { operation: Box<ProTxOperation> },
    Session { session: Box<SharedSession> },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSession {
    pub id: String,
    pub code: String,
    pub message_code: String,
    pub kind: SharedKind,
    pub role: SharedRole,
    pub stage: SharedStage,
    pub revision: u32,
    pub consent_hash: Option<String>,
    pub rows: Vec<ShareRow>,
    pub terms: ShareTermsView,
    pub node_run_by: String,
    pub fee: Option<u64>,
    pub debit: Option<u64>,
    pub pro_tx_hash: Option<String>,
    pub txid: Option<String>,
    pub reserved: Vec<String>,
    pub next: SharedNext,
    pub warnings: Vec<SharedWarning>,
    pub approved_terms: Option<ShareTermsView>,
    pub base_keys: Option<KeyBase>,
    pub key_change: Option<KeyChangeView>,
    pub actor_fee: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMessage {
    pub text: String,
    pub code: String,
    pub name: String,
    pub file_name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedOutcome {
    pub session: SharedSession,
    pub message: SharedMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedImport {
    Session { session: Box<SharedSession>, changed: bool, received_code: String, warnings: Vec<SharedWarning> },
    Standby { description: StandbyDescription, stored: bool },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedWarning {
    ZeroPenalty,
    EditedMessage,
    UnvaluedInputs { shares: Vec<u16>, count: u32 },
    EmptyService,
    ActorOutputDust,
    PenaltyPaidAfterPeriod,
    SkippedSignatures { shares: Vec<u16>, count: u32 },
    NoApprovals,
    OperatorReset,
    UsableByFunder,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateShareRequest {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub reward: RewardChoice,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSharePreview {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub reward_address: Option<String>,
    pub reward_is_refund: bool,
    pub fee: u64,
    pub total_debit: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DissolveRequest {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub fee: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareReturn {
    pub share_index: u16,
    pub amount: u64,
    pub refund_address: Option<String>,
    pub mine: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DissolvePreview {
    pub early: bool,
    pub penalty: u64,
    pub fee: u64,
    pub valid_from_height: u32,
    pub returns: Vec<ShareReturn>,
    pub warnings: Vec<SharedWarning>,
    pub total_debit: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandbyVariant {
    Full,
    Immediate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandbySource {
    Created,
    Imported,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standby {
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub fee: u64,
    pub full_valid_from: u32,
    pub full: bool,
    pub immediate: bool,
    pub source: StandbySource,
    pub created_at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbyDescription {
    pub standby: Standby,
    pub variant: StandbyVariant,
    pub txid: String,
    pub penalty: u64,
    pub returns: Vec<ShareReturn>,
    pub returns_principal: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbyFile {
    pub file_name: String,
    pub text: String,
}
// src/masternodes/tracking.rs
pub const MAX_MASTERNODE_LABEL: usize;
impl Masternodes {
    pub async fn track(&self, pro_tx_hash: String, label: Option<String>) -> Result<ListedMasternode, MasternodesError>;
    pub async fn untrack(&self, pro_tx_hash: String) -> Result<UntrackOutcome, MasternodesError>;
    pub async fn set_label(&self, pro_tx_hash: String, label: Option<String>) -> Result<(), MasternodesError>;
    pub async fn attach_key(&self, pro_tx_hash: String, role: AttachedKeyRole, key: Zeroizing<String>) -> Result<KeyCheck, MasternodesError>;
    pub async fn detach_key(&self, pro_tx_hash: String, role: AttachedKeyRole) -> Result<bool, MasternodesError>;
}
```

<!-- END GENERATED: masternodes-surface -->

## 4. Error codes

| Error enum | Codes |
|---|---|
| `MasternodesError` | `masternodes.list_unavailable`, `masternodes.not_found`, `masternodes.already_tracked`, `masternodes.not_tracked`, `masternodes.key_mismatch`, `masternodes.not_evonode`, `masternodes.no_identity`, `masternodes.platform_unavailable`, `masternodes.unsupported`, `masternodes.not_implemented`, `masternodes.protx_refused`, `masternodes.collateral_unavailable`, `masternodes.signature_mismatch`, `masternodes.operation_not_found`, `masternodes.operation_state`, `masternodes.in_flight`, `masternodes.keys_exhausted`, `masternodes.no_owner_key`, `masternodes.update_pending`, `masternodes.update_superseded`, `masternodes.shared_session_not_found`, `masternodes.shared_stage`, `masternodes.shared_message`, `masternodes.shared_funding_mismatch`, `masternodes.share_not_mine`, `masternodes.shared_too_early`, `masternodes.shared_registration_unknown`, `masternodes.shared_actor_fee` |

- `masternodes.list_unavailable`: the call needs the synced SPV list and there is none yet (or it is still syncing).
No code carries a hash, a key or a wallet id the caller passed, and no message repeats one: a pasted private key has
a hash's shape (D23).

- `masternodes.not_found`: not in the synced list, and not the wallet's or tracked.
- `masternodes.already_tracked` / `masternodes.not_tracked`.
- `masternodes.key_mismatch{role}`: the key is not the masternode's key for `role`.
- `masternodes.not_evonode`: a regular masternode has no claimable credits.
- `masternodes.no_identity`: Platform has no identity for the evonode (registered before Platform).
- `masternodes.platform_unavailable{detail}`: the session runs without Platform, or the proved read failed.
- `masternodes.unsupported{feature, field}`: needs what the pin lacks: `collateral_lookup` (an upstream API, D11),
  `shared_masternodes` (the v24 version-3 payload, D25; MG-03b), or `protx_v3` (a ProTx only version 3 carries,
  MG-04 D1; BL-291) with `field` naming what needs it (`service`: a host name, IPv6 or several entries). `field` is
  `None` for the others.
- `masternodes.not_implemented{call}`: reserved for stubs of later MG tasks; no MG-03 or MG-04 call returns it.
- `masternodes.protx_refused{reason, operation}`: one of Core's rules refuses the ProTx; `reason` is Core's reject
  reason verbatim (`bad-protx-dup-key`, `bad-protx-netinfo-port`, …, MG-04 D5). `protx-dup` is Core's mempool refusal
  of an operator key, platform node id or service another pending ProTx holds; the wallet refuses it against its own
  pending operations before signing (MG-04 D51), and `operation` names the one holding it (discard it if it was
  never sent, else abandon its transaction). `operation` is `None` for every other reason.
- `masternodes.collateral_unavailable`: the collateral isn't an unspent wallet coin of the type's amount at a P2PKH
  address, or is a collateral of the wallet's masternodes or saved operations already (MG-04 D23), or is a coin a
  payment or an asset lock holds, or one that a proposal fee spends that the wallet hasn't seen and that hasn't failed (MG-04 D47).
- `masternodes.signature_mismatch`: the external signature isn't the collateral address's over the saved message.
- `masternodes.operation_not_found`: no such operation of the wallet.
- `masternodes.operation_state{state}`: the call doesn't apply to the operation's state (§5's snake-case name).
- `masternodes.in_flight`: another call on the operation is running; for a shared session, also while the follower's pass reads it (MG-04b D60); retry.
- `masternodes.keys_exhausted{role}`: every keychain key index 0–19 of `role` is in use (MG-04 D13).
- `masternodes.no_owner_key`: a ProUpRegTx needs the masternode's owner key, and no keychain owner key 0–19 nor an
  attached `owner` key matched to its ProRegTx has it, or no loaded wallet holds the ProRegTx that names it (MG-04
  D12).
- `masternodes.update_pending{operation}`: a ProUpRegTx while `operation` (the id of a loaded wallet's ProTx on the
  same masternode) is signed, not sent or sent, or confirmed above the list's height: the list may not show its keys
  yet, and two updates out at once may be mined in either order. Also a resend of an update while an earlier one is
  sent and not in a block. Wait for the list, or discard `operation` while it is not sent; for a resend, discarding
  the row being resent (while it is not sent) also ends the refusal. One that never lists (dropped): abandon its
  transaction (`abandon_transaction`), unlock one of its inputs and spend it; once that spend is in a block
  `operation` reads `conflicted` and blocks nothing (MG-04 D43, D49). Also an `update_share` or a `dissolve` while an
  `update_share` or a `dissolve` of the same masternode is signed, not sent or sent (MG-04b D73, D80). For those and
  a key change's start, approval and send, `operation` may instead name a key-change session of the masternode sent
  from this wallet, `sent` or `stale` (Core may still mine it), or `confirmed` in a block not yet buried (MG-04b D105).
- `masternodes.update_superseded{operation}`: `resend_operation` of a ProUpRegTx that `operation`, a later update of
  the same masternode (maybe another loaded wallet's) that is or may go out, would undo. Discard this row while it is
  not sent; a sent one ends as `update_pending` says (MG-04 D43, D49). `update_pending`'s `operation` may also be
  another loaded wallet's.
- `masternodes.shared_session_not_found`: no shared-masternode session of the wallet has this id.
- `masternodes.shared_stage{stage}`: the call doesn't apply at the session's stage (§5's snake-case name, the derived
  `dead` among them), or to its role (`coordinator`, `participant`: the caller's role, which the call doesn't take);
  `replied` for a fee change after another party's reply (MG-04b D33), `incomplete` for a lock before every row and
  contribution is in, `unsigned` for a send before every participant's input is signed (MG-04b D71), `abandoned` for a resend of an
  abandoned session (MG-04b D72).
- `masternodes.shared_message{reason, share}`: a pasted or opened message is refused: `not_shared`, `too_large`,
  `parse`, `network`, `other_session`, `terms_changed`, `older`, `conflict` (also a lock or approval of a copy other
  than the session's current one, MG-04b D56), `bad_signature` (with `share`), `not_participant`, `labels`;
  for a dissolve-together request `not_dissolution`, `other_masternode`, `signed` and `outputs` (MG-04b D92); for a
  key change `not_key_change`, `other_masternode` and `inputs_hash` (MG-04b D104), and `stale` for an approval or send
  once the synced list's keys aren't the session's base (MG-04b D25). A change
  to this wallet's own row is `shared_funding_mismatch{reason: "row_changed"}`. Never the text (MG-00 D36).
- `masternodes.shared_funding_mismatch{reason, needed}`: a consent or funding check of MG-04b §4 fails:
  `foreign_input`, `overdraw`, `row_changed`, `contribution_changed`, `time_lock`, `template`, `join_signature`,
  `not_own_inputs`, `fee_low` (with `needed`, the minimum fee in duffs).
- `masternodes.share_not_mine`: the share's owner key isn't one this wallet holds.
- `masternodes.shared_too_early`: a maintenance call while the list's tip is at or below the registration's height;
  `discard_shared_session` of a dissolve-together session while its masternode is out of the synced list and the
  wallet's filter scan is below the list's height (MG-04b D94).
- `masternodes.shared_registration_unknown`: the registration isn't a confirmed one of this wallet's sessions
  (MG-04b D29).
- `masternodes.shared_actor_fee{fee}`: `approve_maintenance` of a dissolve-together request another wallet prepared
  that takes `fee` duffs, above the relay minimum, from this wallet's actor share, without `accept_actor_fee`
  (MG-04b m-15, D96).
- `masternodes.unsupported{feature: "shared_masternodes"}` also covers v24 not `active` (§2.4);
  `masternodes.protx_refused` also carries Core's shared reasons and
  `bad-protx-shared-mn` (§2.3, MG-04b D30).
- `Platform(PlatformError)`: a lease's refusal (MG-05 D12), its code unchanged.
- `Engine(EngineError)`: `network_not_open`, `wallet_not_found`, `invalid_argument`, `vault` (a locked vault among
  them), `storage`,
  `internal` and the rest of m1 §4, unchanged; `TxDraft`'s `send.*` failures among them (`send.grant_exceeded`,
  `send.outpoint_unavailable`, `send.amount_exceeds_balance`, …). These are not new §4 codes for the m1–m4 tables, so the Swift
  `engineCodes` are unchanged; MG-04b's `masternodes.*` codes are this file's, not m1's, and the Swift `engineCodes`
  stay unchanged with them.

## 5. Status

`MasternodeStatus` (design §2, D2), first match wins:

| Status | When |
|---|---|
| `enabled` | In the synced list, valid (Core `ENABLED`). |
| `pose_banned` | In the synced list, not valid (Core `POSE_BANNED`). `banned_seen_at` is the first sighting (MG-00 D9). |
| `unconfirmed` | A loaded wallet's ProRegTx for it isn't in a block yet. |
| `unknown` | The list isn't synced. |
| `removed` | Not in the synced list, and it was seen listed before (app.sqlite note or the registry's `ever_listed`), or the wallet's ProRegTx is at or below the list height. |
| `unknown` | Otherwise (a tracked masternode never seen listed, or a registration above the list height). |

Roles (`MasternodeRoles`, design §3): `collateral` (the resolved collateral is an unspent wallet output), `owner`
(derived owner key 0–19, or an attached owner key that holds), `voting` (the list's voting key id, else the wallet's
latest, is a derived voting key 0–19, or an attached voting key holds), `payout` (the payout script is the wallet's),
`operator` (the operator key is one the wallet derives, shown only), `attached` (`None` while the vault is locked;
any other vault error fails the read).

Each `AttachedKey` (D24) carries its role, `key_id` (the hash160 recorded at attach, 40 hex; `None` for a key attached
before ids were kept) and `check`, compared on every read with what the wallet knows then: `voting` with the synced
list's voting key id (`matched` / `key_mismatch`; `unchecked` while it isn't listed or the list isn't synced),
`owner` with the owner key id of a ProRegTx a loaded wallet holds (`matched` / `key_mismatch`, else `unchecked`),
`payout` with that wallet's P2PKH payout script (`matched`, else `unchecked`, D19; none for a `shared_unsupported`
one). A key holds its role when it has a `key_id` and isn't `key_mismatch`, and a voting key only when `matched`
(D29): a voting key a ProUpRegTx rotated away, one attached `unchecked` before the masternode was listed with another
key, or any voting key while the list isn't synced reports `voting: false` (MG-00 D13). A key with no `key_id` (its
vault write failed in this session, D31) holds nothing; a later session's read records the id of the key the vault
holds, with the full-scope data key. Once a session, every recorded id is compared with the vault's key, and one
naming another key is dropped the same way (D34); until then, a read made while a tracking change runs counts those
keys as holding nothing and sends `Masternodes`, so the host reads again.

A row is `mine` when the wallet holds one of these roles through its own provider transactions (an attached key
doesn't count); a provider transaction that gives it none (it paid the fee of someone's ProRegTx) makes no row.

`shared_unsupported` (D25): a version-3 ProRegTx or ProUpRegTx among the wallet's provider transactions, or a version-3
(extended network info) list entry, now or remembered from an earlier sighting (D29). Core v24 replaced `scriptPayout` with `payouts` and `shares` in that version; the
pinned library decodes it (since BL-187), but the engine doesn't read those fields until MG-03b. Such a row reports no `owner`, `voting`, `payout` or `operator` role from that record, no payout
address or registration keys, no payouts (`payouts()` refuses, `payouts_30d` 0), and the overview's
`payouts_incomplete` is set: its total leaves those out. It stays `mine` when a version-3 provider transaction of it is
in the wallet's history, or when the wallet's own version-2 record names a role (its payout counting, though hidden);
a list entry alone doesn't make it `mine` (D29). The share table, per-share payouts and ProUpShareTx /
ProUpSharedRegTx are MG-03b. A registration of one of the wallet's confirmed shared sessions is also
`shared_unsupported`, and it reads as shared: `shares` is `HeldShares{held, of}`, this wallet's shares of it ("you
hold k of n"); it is `mine` and in the overview (MG-04b D74). Every other row's `shares` is `None`.

Shared payouts (D30): once any version-3 entry was in the synced list, now or remembered
(`masternodes.any_shared_seen`, any masternode's), or a version-3 provider transaction is known, a share may pay any
script the wallet watches, a version-2 row's payout script included, and the engine doesn't read share scripts yet
(MG-03b). So every coinbase output to the wallet is then `unattributed`: no row's `payouts_30d` and not the
overview's count it, the row it pays has `payouts_incomplete`, and the overview's `unattributed_30d` counts it with
any other coinbase income no row counts. `payouts()` still lists it. A row's `payouts_incomplete` is also set when it
is `shared_unsupported`. The overview's `payouts_incomplete` is any row's, or unattributed income in the window once
version 3 was seen.

ProTx operations (§2.3, MG-04 §5): `ProTxOperationState`, derived on every read, first match wins:

| State | When |
|---|---|
| `needs_signature` | An `External` row prepared and not signed, its fee inputs unspent. |
| `inputs_spent` | The same, a fee input since spent: refresh it. |
| `confirmed{height}` | Its transaction is in a block of the wallet's history; or a `sent` `dissolve` that pays this wallet nothing (its actor output omitted) once its masternode is out of the synced list, the wallet's filter-scan checkpoint has reached a height the list can't be above, and the registration is in a block at or below it, at that height (MG-04b D81). |
| `conflicted` | Another transaction in a block spends one of its inputs. |
| `sent` | Signed, `sent_at` set: handed to the network or may have been (`last_error` keeps an unknown outcome's code). |
| `not_sent` | Signed, its attempts definitely not sent (`last_error`: a `send.*` code). |
| `signed` | Signed and saved, no hand-off started (a crash between the save and the hand-off). |

Shared-masternode sessions (§2.4, MG-04b §3): `SharedStage`, derived on every read, first match wins:

| Stage | When |
|---|---|
| `confirmed{height}` | The registration is in a block of the wallet's history: by the complete envelope's txid, or as the transaction spending the frozen template's inputs to its outputs (a participant holds only its own signed copy). |
| `conflicted` | Bytes may be out (`out_at` set) and another transaction in a block spends one of this wallet's inputs. |
| `sent` / `funding_signed` | `out_at` set: the coordinator's hand-off (1b), or a participant's Signed Contribution returned. |
| `dead{reason: inputs_spent}` | Nothing out, and one of this wallet's reserved inputs is no longer unspent (MG-04b D17): never re-selected; discard it. |
| `invited` | A participant's imported Invitation, not joined. |
| `draft` | Rows and contributions being gathered. |
| `signing` | The terms are locked; approvals being gathered. |
| `combined` | Every share approved; the funding inputs being signed. |
| `funding_signed` | Some funding inputs signed. |

`SharedSession.revision` is the envelope's revision and `consent_hash` the frozen registration's `consentHash` as
the envelope shows it (`None` in a draft): `lock_shared_terms` and `approve_shared` take them back from the record
the caller reviewed (MG-04b D56).

`SharedSession.next` says what this wallet does next (`join`, `copy`, `wait`, `lock`, `approve`, `sign`, `send`, `abandon` (a key change this wallet funds once it is `stale`, MG-04b D25),
`create_standby` for a confirmed registration in the synced list while a share of the wallet has no standby (MG-04b D91), `nothing`), and `warnings` lists `zero_penalty`, `empty_service` and `unvalued_inputs` (another party's inputs this
wallet can't value).

A dissolve-together session (`kind: dissolve_together`, MG-04b D94) uses the same `SharedStage`, first match wins:
`confirmed{height}` when its combined transaction is in a block of the history; once sent from here, its `dissolve`
operation's `confirmed{height}`, `conflicted`, else `sent`; the collateral spent in a block of the history by the
request signed, `confirmed{height}`, or by another transaction, `dead{reason: delisted}`; `dead{reason: delisted}`
when the synced list above the registration's height omits the masternode and the wallet's filter scan has reached
the list's height (below it the dissolution may be this session's own, not yet in the history, MG-04b D81); `combined` with every share's signature;
else `signing`. A wallet the dissolution pays nothing can't see it, and reads `dead{delisted}` (MG-04b D94). Its `next` is
`approve` while a share of this wallet is unsigned, `copy` while another's is, `send` once combined, `wait` once
sent, else `nothing`; `fee` is the request's, `debit` and `actor_fee` the fee when the actor share is this wallet's
(`debit` 0 otherwise), and `warnings` `actor_output_dust` (D83). An import's own `warnings` add `skipped_signatures{shares,
count}` and `no_approvals` (MG-04b §2.5, D92, D97).
`discard_shared_session` deletes one not sent, or marked out but never handed off, and refuses one while the scan is
behind a list without its masternode (`shared_too_early`); `abandon_shared_session` refuses it (`invalid_argument`,
D95).

A key-change session (`kind: key_change`, MG-04b D103) uses the same `SharedStage`, first match wins. The funder's:
`dead{reason: abandoned}` once abandoned before `out_at`; `confirmed{height}` when its sent transaction is in a block of
the history; with `out_at` set, `conflicted` when another transaction in a block spends one of its fee inputs;
before `out_at`, `dead{reason: inputs_spent}` when a fee input is no longer unspent. Then every wallet's:
`dead{reason: delisted}` as a dissolution's (the funder's request with `out_at` set too: it can't be mined once the
masternode is gone); for an approver, `confirmed{height}` at the list's height once the synced list's keys are the
requested ones (other than the base; an approver can't see the transaction); `stale` once the synced list's keys are
neither the base nor (for an approver, or the funder's request with `out_at` set) the requested ones (MG-04b D25: a
request out stays valid in Core, so its resend would undo the later change); `sent` with `out_at` set; `combined` with
every share's signature; else `signing`. Its `key_change` holds the `requested` keys, the list's `current` ones and whether
this wallet is the `funder`; `base_keys` its base; `fee` and `debit` the funder's fee (`None` for an approver); `next`
`approve` while a share of this wallet is unsigned, `copy` while another's is or, for an approver, once combined, `send`
for the funder once combined, `wait` once sent, `abandon` for the funder once `stale`, else `nothing`; and `warnings` `operator_reset` when the operator key
changes (Core resets the operator fields and PoSe-bans the masternode until a ProUpServTx signed by the new key, the
key written in the basic scheme, one operator change per masternode in the mempool, MG-04b §3.3) and
`usable_by_funder` (a complete request stays usable by the funder's keys until one of its fee inputs is spent: an
abandon once it is out spends one; before, an abandon only releases them, MG-04b D25, D102).

## 6. Events

`EngineEvent::Masternodes { network }`, at most once a second with the trailing edge kept. The follower polls every
5 s; on a new list height, or after a §2.2 call, it re-reads the masternodes of interest (every loaded wallet's own
and the tracked ones), writes first sightings of bans and of listing to app.sqlite, and sends the event when one of
their list entries changed. A §2.2 call sends it too, and so does a change of the wallet set (`WalletCreated`,
`WalletRemoved`, `WalletLoadChanged`: a create, import, restore, load, unload or removal), at the same list height
(D28). §2.3's calls send it after they change an operation or a lock, and §2.4's after they change a session, its locks or a standby. The host re-reads what it shows. Each pass
the follower also releases settled operations' and shared sessions' input locks and locks the collateral of each masternode of the
wallet's it holds unspent, unless released (MG-04 D9, D25). dw-app forwards it as
`AppEvent::Masternodes`; dw-ffi does not forward it until E0-13 binds the facade.

## 7. Persistence

| What | Where |
|---|---|
| Tracked masternodes, their labels | platform-wallet's registry in wallet.sqlite (`tracked_masternodes`, already migrated) |
| Attached keys | the vault (`Vault::store_masternode_key`, MG-01 D7) |
| Labels of untracked masternodes, `banned_seen`, `listed_seen` | app.sqlite `settings_kv`, global scope, keys `masternodes.label:<hash>`, `masternodes.banned_seen:<hash>`, `masternodes.listed_seen:<hash>` (no migration, D12) |
| Attached keys' ids (hash160 of the public key, not secret) | app.sqlite `settings_kv`, global scope, `masternodes.key:<hash>:<role>` (D24) |
| A version-3 list entry seen (never cleared) | app.sqlite `settings_kv`, global scope, `masternodes.shared_seen:<hash>` (D29) |
| Any version-3 entry seen in the synced list (never cleared) | app.sqlite `settings_kv`, global scope, `masternodes.any_shared_seen` (D30) |
| ProTx operations | app.sqlite `masternode_ops` (migration `V2026102101`, MG-04 §3): the request, terms, unsigned or signed bytes, the fee inputs, the attempt (`sent_at` durable before each hand-off, `last_error`); not in `.dwbackup` (MG-04 D24) |
| ProTx input and collateral locks | app.sqlite's persistent coin locks (`lock_manual`), from the row's save until settled or discarded (MG-04 D8, D9, D25) |
| A released collateral | app.sqlite `settings_kv`, wallet scope, `masternodes.collateral_released:<txid>-<n>` (MG-04 D9) |
| Shared-masternode sessions | app.sqlite `shared_mn_sessions` (migration `V2026102201`, MG-04b §9.1): the latest envelope (public data only), this wallet's shares and recorded contribution, its reserved outpoints, the last approved terms, the signed copy and `out_at` (durable before a Signed Contribution is returned); one row per wallet and id; not in `.dwbackup` (MG-04b D19) |
| Shared-session locks | app.sqlite's persistent coin locks, from the save that records the contribution until discarded or settled (MG-04b D17) |
| Standbys | app.sqlite `shared_mn_standby` (migration `V2026103001`, MG-04b §9.1): one row per wallet, masternode, actor share and fee, the full and immediate variants' signed bytes in plaintext, `full_valid_from`, the source; rows are added or completed, never changed (MG-04b D84); deleted with the wallet; not in `.dwbackup` (MG-04b D19) |

The version-3 markers are written durably (`synchronous=FULL`, D35), as is the clearing of an attached key's id
before the vault write (D34).
