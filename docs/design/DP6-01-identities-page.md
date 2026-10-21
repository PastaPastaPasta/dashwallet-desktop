# DP6-01: the Identities page (advanced)

ROADMAP DP6-01 (size M, engine + UI, tiers T0 and T2): "Identities page (advanced): list, set main, keys, refresh,
find". Parity row IOS-071, "Identities screen (list, set main, keys, refresh, find, get username)"; iOS's
`IdentitiesScreen.swift` (`IdentityDetailScreen`, `IdentityPublicKeysScreen`) is the UX reference. DASHPAY F7 puts the
page in Settings ▸ Advanced; §4's `IdentitiesViewModel` reads `identities` and `identity_detail`. DP1-10's My Profile
stays the profile sheet (DP1-10 F2 handed "choosing the main name among several; other identities" to this task).

Dependencies, merged: DP1-05 (`identities`, `set_main_identity`, `discover_identities`, the main identity's rules in
`recovery.rs`), DP1-06 (`refresh_balance`, the balance's block time), DP1-03 (`set_main_name`), DP2-03 (`resolve_user`,
the DPNS domain read), BL-153 (`DashPay.status`), U-04/U-05 (Settings tabs, `GrantModel`).

## Facade API

| Need | Call | State |
|---|---|---|
| List: id, index, names, main name, balance, which is main | `identities()` (sync) | **exists** (DP1-05, DP1-06, DP1-08) |
| Keys: id, purpose, security level, type, read-only, disabled, contract bounds; revision | `identity_detail(identity)` (async) | **stub → implemented here** (records exist since E0-08) |
| Set main | `set_main_identity(identity)` (async) | **exists** (DP1-05) |
| An identity's main name among its names | `set_main_name(identity, label?)` | **exists** (DP1-03) |
| Refresh one | `refresh_identity(identity) -> IdentityDetail` (async) | **new** |
| Refresh all | `refresh_identities() -> Vec<IdentityRefresh>` (async) | **new** |
| Find by id or DPNS name, read-only | `find_identity(query) -> Option<FoundIdentity>` (async) | **new** |
| Find the seed's identities past the last index (iOS "Find identities") | `discover_identities(grant)` (async) | **exists** (DP1-05) |
| Balance only | `refresh_balance(identity)` | exists (DP1-06), not used by the page |

New records (`identity.rs`):

```rust
pub struct IdentityRefresh { pub identity: String, pub failure_code: Option<String> }   // SyncPassReport's form
pub struct FoundIdentity {
    pub identity: String,            // Base58
    pub found_by: FoundBy,           // Id | Name{label}   (kind-tagged)
    pub names: Vec<String>,          // DPNS labels whose records.identity is it, as spelled, at most 10
    pub balance: Option<u64>,        // credits, from the fetched identity
    pub revision: Option<u64>,
    pub public_keys: Vec<IdentityKeyInfo>,
    pub own: bool,                   // one of this wallet's identities
    pub unverified: bool,            // m4 §5: verified only through the trusted fallback
}
```

Semantics:

- **`identity_detail(identity)`**: the identity's `identities()` summary plus the library's in-memory identity (its
  revision and public keys, by key id). Reads memory only, after the library's read lock (the call is async, so it waits
  for a sync pass's write rather than answer from the snapshot). Not one of the wallet's: `identity.not_found`; a
  malformed id: `invalid_argument`.
- **`refresh_identity(identity)`**: one of the wallet's identities, else `identity.not_found`. Under the identity's write
  lock (`identity_writes.rs`, so it never races a key upgrade or a name write on the revision), fetches the identity from
  Platform (proved through the trust provider, so `platform.trust_mismatch` and `context_unavailable` reach the caller),
  and folds Platform's **keys and revision** into the library's copy, keeping the library's balance and its block time;
  then refreshes the balance the DP1-06 way (`Balances::refresh`, proved, with its block time, an older answer never
  replacing a newer one). Platform has no such identity: `identity.not_found`. Returns `identity_detail`. The library's
  store signals `Platform{Identities}`.
- **`refresh_identities()`**: `refresh_identity` for each of the wallet's identities in index order, one at a time; each
  outcome with the failed one's code. Fails as a whole only for `network_not_open`, `wallet_not_found` and the like.
- **`find_identity(query)`**: `query` trimmed. 32 bytes in Base58: by id. Otherwise a DPNS label (`alice` or
  `alice.dash`, any case, `resolve_user`'s form): by name, via the DPNS domain (DP2-03's read), then by its
  `records.identity`. Neither form: `invalid_argument`. No domain, or no identity on Platform: `None`. The names
  are a `records.identity` query (the SDK's `get_dpns_usernames_by_identity`, limit 10); their failure leaves `names`
  empty, never fails the find. **Read-only**: nothing is added to the library (unlike the library's
  `load_identity_by_dpns_name`, which watches the identity), nothing is recorded as verified. Needs no key.

Errors: no new code. `identity.not_found`, `invalid_argument`, and the shared `platform.*` mapping (E0-05)
cover every refusal. Nothing changes in m1–m4 §4, so neither the Swift `engineCodes` nor the Swift T0 suite is touched.

**Contract delta** (`docs/contracts/m4-dashpay-engine.md`): §2.1 rows for `identity_detail` (implemented) and the three
new calls; §0's `identity.rs` row and `NOT_FACADE` list (`identity_reads.rs`, `identity_tests.rs`); §7 item 61
"The Identities page (DP6-01, version 34)"; `Contract-Version: 34`; §3 re-blessed. The contract test's `IMPLEMENTED`
gains the four calls.

## Engine layout

| File | What |
|---|---|
| `identity.rs` (facade) | the four calls' bodies (one line each) and the new records |
| `identity_reads.rs` (new, not facade) | `IdentityNet` (the seam: identity by id, DPNS domain, names by identity; the SDK or a test's script), the dpp → record key mapping, `detail_of`, `refresh_of`, `find_of` |
| `identity_tests.rs` (new, not facade) | session tests over the scripted net |
| dw-app `api/identity.rs`, `identity/keys.rs` | `identity_refresh`, `identity_refresh_all`, `identity_find`; `IdentityRefresh`, `FoundIdentity`, `FoundBy` DTOs |
| dwcli `dashpay.rs` | `identity refresh [--all \| --identity I]`, `identity find <query>`; the session grammar's types |

## dwcli

`identity list`, `identity set-main <id>`, `identity show [--identity I]` (the keys; `identity keys` is not added, D4),
`identity refresh [--identity I | --all]`, `identity find <id-or-name>`, `identity discover` (exists). Facade only.

## UI

Settings gains an **Advanced** tab (`settings-tabs-advanced`) whose page is **Identities** (`identities-page`), F7's
place. View model `src/dashpay/identities.ts` (pure TypeScript), queries in `src/dashpay/identityQueries.ts`, screen
`src/screens/settings/Identities.tsx`.

**List** (`identities-list`, one `identity-row` per identity, `data-main` on the main one):

| # | Input | Shows | Scenario |
|---|---|---|---|
| L1 | identities unread | "Loading identities…" | — |
| L2 | read failed | "Could not read this wallet's identities." + Try again | `dashpay-registered` + `fail=identities:none` |
| L3 | none (`noIdentity`, watch-only, …) | "This wallet has no identities." + Join DashPay (status allows) + Find my identities | `dashpay-no-identity` |
| L4 | one or more | per row: initials avatar, @main name ("No username" / "Updating…" / "Username not loaded"), "Main" badge, "Index N" (the derivation index), short id with copy, credit balance (Dash, masked in discreet mode; "Unknown"; "Updating…" when stale), "Unverified" mark; actions Set as main (not on the main row), Keys, Refresh | `dashpay-identities` (two identities, the second main by choice) |
| L5 | names updating (snapshot) | L4 with "Updating…" | `dashpay-names-updating` |

Header actions: **Refresh all** (`refresh_identities`; then "Refreshed N identities" or "N could not be refreshed"),
**Find my identities** (`discover_identities` under an `identityScan` grant through `GrantModel`; "Found N new
identities" / "No new identities found"), **Look up** (the find panel).

**Set as main**: one call at a time; the row's badge moves on the next read (`Platform{Identities}` re-reads the
shell's `identities` query, which the chip and My Profile share). A failure shows its copy on the row.

**Detail** (`identity-detail`, opened by Keys): identity id with copy, index, revision ("Unknown" while `null`), its
usernames with "Main name" and "Set as main name" (`set_main_name`), and the keys table (`identity-keys`): ID, Purpose,
Security level, Type, Status ("Enabled", "Disabled", "Read-only"), contract bound ("DashPay contact requests" for the
DashPay contract's `contactRequest`, else the short contract id). States: loading, failed + Try again, loaded, no keys.

**Find** (`identity-find`): a field "Identity ID or username" and Find. States: idle, invalid ("Enter an identity ID or a
username"), searching, not found ("No identity with this ID or username"), failed (copy by code + Try again), found
(id with copy, how it was found, usernames or "No username", credit balance, revision, the keys table, "This is one of
your identities" when `own`, "Unverified").

Copy by code: `platform.unavailable` "Dash Platform did not answer. Try again.", `platform.context_unavailable`
"Waiting for the network to sync and verify this identity. Try again shortly.", `platform.trust_mismatch` "This
identity's data could not be verified against the network.", `identity.not_found` "This identity is not on Dash
Platform.", `platform.timeout` "Dash Platform took too long to answer. Try again.", else the code.

Fixture: scenario `dashpay-identities` (two identities; the second main; keys for each; a directory identity to find by
name and id), plus failure switches through the existing `fail=` mechanism. Playwright `e2e/identities.spec.ts`
(IOS-071) with axe in light and dark on Chromium and WebKit; visual baselines for L4, the detail and a found result,
light and dark (Chromium). View-model mutants: the main badge on the wrong row; the disabled status read as enabled.

## T2 (dashmate devnet)

Run on 2026-10-21 against the shared dashmate `local` devnet with the branch's `dwcli`, one wallet from a seed under
`/work/scratch/dw-dp6-01/t2/`; each command's JSON is kept in `/work/scratch/dw-dp6-01/t2/ev/` (`README` lists them).
The two identities were registered on Platform by the seed's own keys (D11), `dp601-alpha7` and `dp601-beta8`.

| Step | Evidence | Result |
|---|---|---|
| Find my identities | `02-discover` | `2`: the scan finds both from the seed |
| List | `03-list` | index 0 and 1, index 0 main (no choice yet), names as registered |
| Set main, then list in a new process | `04`–`06` | index 1 main; `dashpay status` names it |
| Keys | `07-show-A`, `08-show-main` | revision 0, keys 0–2 (MASTER, HIGH, CRITICAL auth); no `--identity` shows the main one |
| Refresh one, refresh all | `09`, `10`, `14` | the balance current again (`balance_stale` false); both refreshed, no failure |
| Find by name, by id, none | `11`–`13` | found by `name` and by `id`, both `own`; an unknown name answers `null` |
| A key added on Platform, then refresh | `15`–`17` | before: revision 0, 3 keys (the cached copy); after `refresh`: revision 1 with key 3 (MEDIUM auth); a new process shows it |

Every identity stays `unverified`: the devnet's quorum keys come from MG-07's sidecar, not from a masternode list
(D11). The page itself is proven on the fixture (T0); the devnet run proves the engine and `dwcli` calls it uses.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The page is Settings ▸ Advanced ▸ Identities: one new Settings tab "Advanced" holding the page, no new route or sidebar item. | DASHPAY F7 ("Identities page in Settings ▸ Advanced"); iOS keeps it behind advanced mode; one tab is the smallest shared edit. |
| D2 | `refresh_identity` folds keys and revision only; the balance is refreshed through DP1-06's `Balances::refresh`, not replaced by the fetched identity's. | The library's `refresh_identity` replaces the whole identity, balance included, without a block time: an older answer could replace a newer balance, which DP1-06 forbids (m4 item 44). Its error mapping also turns proof failures into an untyped string; a direct proved fetch keeps `trust_mismatch`. |
| D3 | `refresh_identity` holds the identity's write lock while it fetches and folds. A Refresh started while a top-up, registration or profile write holds that lock waits for it, showing "Updating…", and the page's one-action rule holds the other actions meanwhile. | A key upgrade or name write builds on the cached revision (`requests.rs` `enable_keys`); a refresh between its read and its write would move the revision under it. The wait is bounded by that write's own timeouts. |
| D4 | No `dwcli identity keys`: `identity show` already prints the keys (`identity_detail`), now implemented. | "The closest fit to the existing `identity …` commands"; one command per facade call. |
| D5 | "Find" is the brief's: a read-only lookup by id or DPNS name (`find_identity`), which also answers iOS's "get username" (whose name is this identity). iOS's "Find identities" (scan the seed) is `discover_identities`, already DP1-05's, offered as **Find my identities**. "Get a username" as registration of an extra name is IOS-126 (the DP1-09 credits path), linked from a row without a name. | The brief defines find; m4 §2.1 already calls `discover_identities` DP6-01's "find". Both are on the page, named apart. |
| D6 | A found identity is never added to the wallet or watched, and nothing is recorded as verified (`dp_trust_verified`). | Read-only by the brief; the library's `load_identity_by_dpns_name` adds a watched identity, which would then show in sync passes. |
| D7 | `FoundIdentity.names` are the domains whose `records.identity` is the identity, as spelled, at most 10, no contest or main-name rules. A failed names read leaves the list empty. | DPNS indexes `records.identity`, not `$ownerId` (SDK note); a lookup needs the names, not the wallet's main-name model. |
| D8 | **Restore and the main identity.** The choice lives in `dp_main_identity` (app.sqlite): it survives a restart and a `.dwbackup` restore brings it back (wallet rows). A restore from the recovery phrase alone has no row, so the main identity is the lowest index once identities are rediscovered (DP1-05 `recovery.rs`). A row naming an identity the wallet no longer has is ignored (lowest index) and stays until the next choice. | DP1-05's rules, unchanged; the brief asks what a restore does to a user-chosen main. |
| D9 | `refresh_identities` refreshes one identity at a time. | Each fetch is a few proved queries; one at a time keeps DAPI load and the write locks bounded, and a wallet has few identities. |
| D10 | `identity_detail` reads memory after the library's read lock, not the snapshot. | The call is async (m4 §1), so it may wait; keys from a stale snapshot would show an old key set after a refresh. |
| D11 | **T2 seeding.** On the dashmate `local` devnet dash-spv never finishes its masternode list (no `llmq_test_dip0024` quorum answers QRInfo; MG-07 saw the same), so `funding_hold()` keeps a `dwcli identity register` draft at `waiting: sync` with nothing funded. The two identities are therefore registered straight on Platform by an uncommitted `#[ignore]` harness (kept as `/work/scratch/dw-dp6-01/t2/identity_live.patch`) with the keys the T2 wallet's own seed derives at `m/9'/1'/5'/0'/0'/i'/k'`, key 0 the MASTER auth key the discovery scan looks up. Everything after that (discover, list, set-main, show, refresh, find) runs through `dwcli` on the branch build. | The closest proof the devnet allows: the identities are the wallet's own (the engine finds and signs for them), only their creation bypasses the engine's funding path, which DP1-09 owns and this task does not change. |
| D12 | A key's contract bounds map to `contract_bound` (the contract id) plus `contract_bound_document_type` for `SingleContractDocumentType`; a `SingleContract` bound has no document type. The UI names DashPay's `contactRequest` binding "DashPay contact requests" and shows any other contract by its short id. | `IdentityKeyInfo` already has both fields (DP2-01); one field per bound part, no new record. |
| D13 | Adding the Advanced tab changes the tab strip on every Settings screen, so the twelve `settings-*` visual baselines are regenerated with it. The pixel diff of each old and new baseline is confined to the tab strip. New baselines: `identities-list`, `identities-detail`, `identities-found` (light and dark). | The brief allows baselines for the new page only; a new tab cannot leave the strip as it was, and nothing else on those screens moved. |
| D14 | `SettingsPage` takes the window's `network` (App already holds it), for the credit balance's unit. | The one shared-file edit the page needs in `App.tsx`; no new query. |
| D15 | **Set as main and Set as main name signal themselves.** `set_main_identity` sends `Platform{Identities}` and `Platform{Startup}`; a stored main-name pick sends `Platform{Identities}`. The fixture's handlers send the same. | Neither is a library changeset, so the changeset tap never saw them and the page, the chip and My Profile kept the old main until an unrelated signal (Opus review B1). Engine tests assert both signals (killed when removed). |
| D16 | A refresh's fold may leave memory ahead of disk when a key store fails part-way (`replace_key` and `set_status` mutate, then persist). Kept: memory only ever moves toward Platform's answer, the call fails `storage`, and the next refresh after a restart folds again from the older stored copy. | The library offers no persist-first form; reordering would need its internals (Opus review N4). |
| D17 | Error copy (review S1, S2, N6): `invalid_argument` has no page-wide copy (the look-up shows its own; Set as main name says the identity no longer owns that name); `platform.proof_invalid` reads "Dash Platform's answer could not be verified. Try again."; `identity.not_found` keeps one text for the rare case an identity left the wallet mid-action. A key is "DashPay contact requests" only when bound to DashPay's system contract (`dashpay_contract::ID`, the same on every network) and its `contactRequest` type (N2). | Each failure the engine can return says what happened; the look-up's input check stays its own. |

## Slice plan

1. This document.
2. Engine: `identity_reads.rs`, the facade bodies, tests, m4 contract (v34), dw-app commands and generated types, dwcli.
3. UI: Backend binding, view model + tests (mutants), fixture scenario and handlers, the Advanced tab and page,
   Playwright + axe, visual baselines, `docs/parity.md` IOS-071.
4. T2 on the dashmate devnet.
