# MG masternodes engine contract (`dw-engine` facade)

Contract-Version: 1

Status: **draft**, 2026-10-16 (ROADMAP MG-03). Milestone 1.1, masternodes and governance (UX-SPEC §4.25, MG-00).
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
| Keys | Attached keys are secp256k1 secrets for `owner`, `voting` or `payout`, kept by the vault (MG-01 D7). No call takes, holds, returns or signs with an operator or platform-node key (MG-01's guard). No call signs anything (ProTx management is MG-04's). |

## 2. Calls

Kind is `sync` (memory only) or `async` (on the engine runtime). Every call is implemented.

### 2.1 Reads

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `NetworkSession.masternodes()` | sync | The handle (§0). Infallible: returns `Masternodes`, not a `Result`. **Implemented.** | — |
| `list_state()` | sync | `MasternodeListState`: whether the SPV list is synced (the headers and masternode phases done) and the masternode phase's height (D3). Memory only. **Implemented.** | `MasternodesError` |
| `list(wallet: WalletId)` | async | `MasternodeOverview`: the list state, the wallet's own masternodes (in the library's registration order) then the tracked ones not among them (in tracking order), each a `MasternodeRow` with status (§5), roles, collateral, payout address, 30-day payouts and the newest payout; the collateral-shaped coins (MG-00 D1); the wallet's 30-day payout total, each history record counted once. `wallet_not_found` for an unknown wallet. **Implemented.** | `MasternodesError` |
| `masternode(wallet: WalletId, pro_tx_hash: String)` | async | `MasternodeDetail`: the row, the list's facts, the registration facts from the wallet's provider transactions, `payout_checked_height` (always `None` until MG-00 D42's watch exists, D5), `tracked_at`. Any masternode the wallet owns or tracks, or that the synced list holds. `invalid_argument` for a malformed hash (first), then `masternodes.not_found{query}`, or `masternodes.list_unavailable` while the list that might hold it isn't synced. **Implemented.** | `MasternodesError` |
| `payouts(wallet: WalletId, pro_tx_hash: String, since: Option<u64>)` | async | Every `MasternodePayout` of the masternode, newest first (by height, then time): the wallet's coinbase history records to its payout address (design §5, D9), without orphaned ones; with `since`, only those at or after it. Empty when the payout address isn't known. Refuses like `masternode`. **Implemented.** | `MasternodesError` |
| `find(query: String)` | async | The synced list's entries matching a ProTx hash (64 hex) or a service address (`ip:port`, or `ip` for any port), in list order, as `ListedMasternode` with whether each is tracked. `invalid_argument` for anything else (first), `masternodes.list_unavailable` while the list isn't synced. No private key is accepted (MG-00 D13). **Implemented.** | `MasternodesError` |
| `evonode_credits(pro_tx_hash: String)` | async | `EvonodeCredits`: the balance of the evonode's owner identity (id = the ProTx hash's display bytes), read with a Platform proof (dash-sdk `IdentityBalance`). Refusals in order: `invalid_argument`, `masternodes.list_unavailable`, `masternodes.not_found{query}`, `masternodes.not_evonode{pro_tx_hash}`, `masternodes.platform_unavailable{detail}` (no Platform in this session, or DAPI failed), `masternodes.no_identity{pro_tx_hash}`. The fetch runs outside the session's operation guard. **Implemented.** | `MasternodesError` |
| `resolve_collateral(pro_tx_hash: String)` | async | The header-checked collateral outpoint of any listed masternode (MG-00 D43). `invalid_argument` for a malformed hash, else always `masternodes.unsupported{feature: "collateral_lookup"}`: dash-spv exposes no header-by-hash or block fetch at the pin (D11). **Implemented** (as the refusal). | `MasternodesError` |

### 2.2 Tracking and attached keys

Every call here sends §6's event after it changes something.

| Call | Kind | Semantics | Errors |
|---|---|---|---|
| `track(pro_tx_hash: String, label: Option<String>)` | async | Adds a masternode of the synced list to platform-wallet's registry (wallet.sqlite; it belongs to the network, D13). Returns its `ListedMasternode`. Refusals in order: `invalid_argument` (hash, or a label over `MAX_MASTERNODE_LABEL` characters after trimming), `masternodes.list_unavailable`, `masternodes.not_found{query}` (not in the list, D7), `masternodes.already_tracked{pro_tx_hash}`. **Implemented.** | `MasternodesError` |
| `untrack(pro_tx_hash: String)` | async | Deletes the attached keys first, then the registry row (MG-00 D22). Returns `UntrackOutcome { keys_deleted }`. `masternodes.not_tracked{pro_tx_hash}`; a vault that can't delete (locked: `vault`) leaves it tracked. **Implemented.** | `MasternodesError` |
| `set_label(pro_tx_hash: String, label: Option<String>)` | async | Trims the label; blank or `None` clears it (back to the default name, `type_index`). A tracked masternode's label is the registry's; any other's is kept in app.sqlite (D12). `invalid_argument` for a malformed hash or a label over `MAX_MASTERNODE_LABEL` characters. **Implemented.** | `MasternodesError` |
| `attach_key(pro_tx_hash: String, role: AttachedKeyRole, key: Zeroizing<String>)` | async | Stores a WIF (mainnet's for mainnet, a test network's otherwise) or 64-hex secp256k1 secret for `role` of a tracked masternode, replacing an earlier one. Checked first (MG-00 D13): `voting` against the synced list's voting key id (`masternodes.list_unavailable` while it isn't synced; `unchecked` when the list no longer holds it); `owner` against the owner key id, and `payout` against a P2PKH payout script, of a ProRegTx/ProUpRegTx a loaded wallet holds, otherwise `unchecked`. Returns `KeyCheck`. Refusals in order: `invalid_argument` (hash, key), `masternodes.not_tracked{pro_tx_hash}`, `masternodes.list_unavailable`, `masternodes.key_mismatch{role}`, then the vault's (`vault`, for one without the full-scope data key). **Implemented.** | `MasternodesError` |
| `detach_key(pro_tx_hash: String, role: AttachedKeyRole)` | async | Deletes the key attached for `role`. Returns whether there was one. `invalid_argument`, `masternodes.not_tracked{pro_tx_hash}`, then the vault's. **Implemented.** | `MasternodesError` |

## 3. Surface (generated)

Generated by `mn_masternodes_contract.rs` from the facade's source files; `DW_BLESS=1` rewrites it after
`Contract-Version` is raised.

<!-- BEGIN GENERATED: masternodes-surface -->
<!-- surface-sha256: db16db31a92ee1c44611ad163bbc8fcd5c3cc4a3dae7f339bde296468eef1e91 version: 1 -->

```rust
// src/masternodes/errors.rs
#[derive(Debug, thiserror::Error)]
pub enum MasternodesError {
    ListUnavailable,
    NotFound { query: String },
    AlreadyTracked { pro_tx_hash: String },
    NotTracked { pro_tx_hash: String },
    KeyMismatch { role: AttachedKeyRole },
    NotEvonode { pro_tx_hash: String },
    NoIdentity { pro_tx_hash: String },
    PlatformUnavailable { detail: String },
    Unsupported { feature: String },
    NotImplemented { call: String },
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
pub use records::{ AttachedKeyRole, CollateralCoin, CollateralState, EvonodeCredits, KeyCheck, ListFacts, ListedMasternode, MasternodeDetail, MasternodeListState, MasternodeOverview, MasternodePayout, MasternodeRoles, MasternodeRow, MasternodeStatus, MasternodeType, RegistrationFacts, UntrackOutcome, };
pub use tracking::MAX_MASTERNODE_LABEL;
#[derive(Clone)]
pub struct Masternodes {
    ..
}
impl NetworkSession {
    pub fn masternodes(self: &Arc<Self>) -> Masternodes;
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
    Unchecked,
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
    pub attached: Option<Vec<AttachedKeyRole>>,
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
    pub roles: MasternodeRoles,
    pub collateral: CollateralState,
    pub payout_address: Option<String>,
    pub payouts_30d: u64,
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
| `MasternodesError` | `masternodes.list_unavailable`, `masternodes.not_found`, `masternodes.already_tracked`, `masternodes.not_tracked`, `masternodes.key_mismatch`, `masternodes.not_evonode`, `masternodes.no_identity`, `masternodes.platform_unavailable`, `masternodes.unsupported`, `masternodes.not_implemented` |

- `masternodes.list_unavailable`: the call needs the synced SPV list and there is none yet (or it is still syncing).
- `masternodes.not_found{query}`: not in the synced list, and not the wallet's or tracked.
- `masternodes.already_tracked{pro_tx_hash}` / `masternodes.not_tracked{pro_tx_hash}`.
- `masternodes.key_mismatch{role}`: the key is not the masternode's key for `role`.
- `masternodes.not_evonode{pro_tx_hash}`: a regular masternode has no claimable credits.
- `masternodes.no_identity{pro_tx_hash}`: Platform has no identity for the evonode (registered before Platform).
- `masternodes.platform_unavailable{detail}`: the session runs without Platform, or the proved read failed.
- `masternodes.unsupported{feature}`: needs an upstream API the pin lacks (`collateral_lookup`, D11).
- `masternodes.not_implemented{call}`: reserved for stubs of later MG tasks; no MG-03 call returns it.
- `Engine(EngineError)`: `network_not_open`, `wallet_not_found`, `invalid_argument`, `vault` (a locked vault among
  them), `storage`,
  `internal` and the rest of m1 §4, unchanged. These are not new §4 codes for the m1–m4 tables, so the Swift
  `engineCodes` are unchanged.

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
(derived owner key 0–19, or attached), `voting` (the list's voting key id, else the wallet's latest, is a derived
voting key 0–19, or attached), `payout` (the payout script is the wallet's), `operator` (the operator key is one the
wallet derives, shown only), `attached` (`None` while the vault is locked).

## 6. Events

`EngineEvent::Masternodes { network }`, at most once a second with the trailing edge kept. The follower polls every
5 s; on a new list height, or after a §2.2 call, it re-reads the masternodes of interest (every loaded wallet's own
and the tracked ones), writes first sightings of bans and of listing to app.sqlite, and sends the event when one of
their list entries changed. A §2.2 call sends it too. The host re-reads what it shows. dw-app forwards it as
`AppEvent::Masternodes`; dw-ffi does not forward it until E0-13 binds the facade.

## 7. Persistence

| What | Where |
|---|---|
| Tracked masternodes, their labels | platform-wallet's registry in wallet.sqlite (`tracked_masternodes`, already migrated) |
| Attached keys | the vault (`Vault::store_masternode_key`, MG-01 D7) |
| Labels of untracked masternodes, `banned_seen`, `listed_seen` | app.sqlite `settings_kv`, global scope, keys `masternodes.label:<hash>`, `masternodes.banned_seen:<hash>`, `masternodes.listed_seen:<hash>` (no migration, D12) |
