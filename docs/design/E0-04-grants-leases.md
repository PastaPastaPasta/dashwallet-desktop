# E0-04: grants and leases — design

**Status:** design (D) for review under DEC-57. It is not normative until that review closes. Once approved, it
replaces the **DRAFT** E0-04 text in [`DASHPAY.md`](DASHPAY.md) §2.6 ("Commit points", "At lock time" and the
open-issues list) and the draft clauses of the E0-04 row in [`ROADMAP.md`](ROADMAP.md).

**Inputs:**
- DASHPAY §2.6 and §3.3–3.4;
- the design-input checks `docs/design/checks/e0_04_{dispatch_model,split_model,mutations}.py`;
- E0-03's merged vault code (`rust/crates/dw-vault`, `rust/crates/dw-engine/src/platform/`);
- reviews DW-E0-03 r3 (Opus) and r4 (GPT), and the fix4 findings listed in DASHPAY §2.6;
- DEC-18, DEC-57 and DECISIONS-PENDING B5.

**Pins read:**
- platform `bc41f1bc23`. `PW` = `packages/rs-platform-wallet/src`, `PWS` = `packages/rs-platform-wallet-storage/src`,
  `SDK` = `packages/rs-sdk/src`.
- rust-dashcore `40268cc0` (`dash-spv`, `key-wallet`).

**Spec check:** `python3 -I docs/design/checks/e0_04_design_model.py` (exit 0 = pass; about 15 s).

## 0. Decisions at a glance

1. **The dispatch record moves out of platform-wallet and into a journal the host owns.** It is a small SQLite file,
   `<network>/dispatch.sqlite`, opened with `synchronous=FULL`, and only the engine's dispatch fence writes it. The
   asset-lock row, its changeset merge and its SQLite upsert stay exactly as they are at the pin. Every persistence
   problem the review found (M-B a–d) was a problem of carrying the record through that merge, so the record no
   longer goes there.
2. **One step under one mutex decides every hand-off.** The fence's `admit` holds the lease table's mutex J and in
   that step reads the artifact's journal entry, checks the origin lease, charges the budget, and either
   compare-and-sets `Unsent → Committing` (with a permit) or `Unsent → Revoked`. Nothing about the decision is read
   earlier. That closes M-A. `admit` takes no library lock and never waits on anything but J and its own journal write,
   so the lock-order deadlock M-A describes cannot happen (§5.8).
3. **The commit is that compare-and-set.** A First hand-off of a registered artifact starts its transport only after
   its `Dispatching` write has returned durably. A failed or ambiguous write makes the entry `Ambiguous`: it is never
   cleaned up, and it is resent only after a later write succeeds.
4. **Registration before tracking.** platform-wallet registers an asset lock with the fence (durable `Unsent` plus its
   origin lease) before it tracks the `Built` row, and makes the row durable before it asks to hand it off. When the
   journal is created it is seeded with a `PreFence` entry, counted as possibly sent, for every asset-lock row already
   in `wallet.sqlite`. This closes M-B (d). A row with no entry at all has unknown provenance and is neither sent nor
   cleaned up, so the decision never rests on what a call site says about the row.
5. **Row-less bytes are remembered too.** The fence keeps, per process, the ids of row-less artifacts it admitted as a
   First. A later hand-off of the same bytes (a `TxDraft` repeat, a re-signed state transition with identical bytes)
   is a Resend, never a definite refusal of something that may already be on the wire.
6. **Revocation happens at the call, not in the drain.** `lock_vault` revokes every lease and snapshots the permits in
   one synchronous step under J, before its first await. Its drain then only waits. Each permit carries a deadline
   (its grant + H), and the host enforces that deadline whatever the library does. So `lock_vault` returns within
   `max(H, vault-gate wait)` of its call, for any number of leases. A dropped future, or a second call, changes
   nothing. A lease whose creation began before the call and finished after it is refused (`lock_gen`). This closes
   M-C.
7. **Exactly one party cleans up, and nothing can drop the cleanup.** Only the caller whose compare-and-set made
   `Revoked` cleans up. The cleanup runs as a library-spawned task, so dropping the caller cannot lose it, and a build
   dropped before `admit` abandons its row through a synchronous drop guard. The release is owner-guarded by the
   build's reservation token.
8. **A catch-up at load.** dw has none today. E0-04 adds one: after the journal is loaded and before user flows, every
   tracked `Built` or `Broadcast` row is resumed, and the journal decides. Possibly-sent rows get their inputs reserved
   again. Never-dispatched rows are cleaned up.
9. **Leases** are engine objects with a 128-bit random id. A lease is bound to one wallet and one flow, and holds
   scoped signers and per-purpose budgets. Its tokens are dropped once the signers are issued. On a locked vault it
   also holds one `KeyHold`, the only strong reference to the grant's own key. It drops the hold when the engine's
   InstantSend wait ends or the flow falls back to the ChainLock wait.
10. **Grants:**
    - `PlatformOp{max_duffs, max_credits}`.
    - A new `IdentityScan` purpose, the only one that releases the scan master key.
    - `authorize_set`: several grants from one credential check, for "Accept and pay".
    - `platform_signer` takes the funding cap from the token.
    - `Vault::epoch()`, so the engine sees every epoch change.
11. **One upstream PR** (B5) puts the fence at two choke points, each wrapping every call site of its kind:
    - platform-wallet's `TransactionBroadcaster` dispatch;
    - rs-sdk's `BroadcastStateTransition::broadcast`.

    It also adds the registration call and the library's obligations (§5.6). Until that PR is in the pin, flows
    driven by the library keep DASHPAY's interim wording ("a send already signed may still go out").

| Open issue (DASHPAY §2.6) | Resolution | Checked by |
|---|---|---|
| M-A read, fence and write not atomic; lock order | §5.4 single J step with CAS; §5.8 admit never blocks; cleanup by the CAS winner only (§5.5) | model Part 1 (`split-literal` caught, `split-cas` safe, M-A replay), Part 3 |
| M-B (a) `store` not durable | journal written with `synchronous=FULL` before the transport (§6.2); row durable before admit (L3) | `transport-first`, `no-flush` mutations |
| M-B (b) merge not monotone, resurrected rows | the record is not in the changeset; the journal never moves `Dispatching → Unsent`; a resurrected row meets its entry (§6.3) | Part 1 `flow+crash` |
| M-B (c) failed write is not a clean `Unsent` | `Ambiguous`: no cleanup, Deferred, retried before any transport (§6.4) | `clean-on-write-fail` mutation |
| M-B (d) rows from before the record | the journal is seeded with `PreFence` for them ⇒ Resend; no entry ⇒ kept, not sent (§5.4, §6.5) | `legacy` and `unknown` scenarios; `legacy-unsent`, `no-entry-resend` mutations |
| M-C H bound and drain end | deadline from the grant covers write and transport; host-side expiry; revoke at the freeze; `lock_gen` on lease creation (§8) | Part 2 (all five wrong timing rules fail) |
| m-1 lease ids across processes | 128-bit random ids, matched with the wallet (§4.1) | `lease-reuse` mutation |
| m-2 Part 2 cannot fail | Part 2 is now a tick simulation with per-lease states and the freeze as an event | Part 2 mutations |
| m-3 undocumented atomicity | stated (§10.1); the split variants show what breaks | `split-*` variants |
| nits | §A.2 | — |

## 1. What the pin does

These facts were checked at the pins. Everything below depends on them.

| # | Fact | Where |
|---|---|---|
| F1 | Every Core-transaction hand-off in platform-wallet goes through `TransactionBroadcaster::broadcast`. The sites: the asset-lock build `build.rs:1142`; resume, Built arm `recovery.rs:1176` and Broadcast arm `:1491`; `CoreWallet::dispatch_unexpired` `core/broadcast.rs:158` (finalized sends, ProUpServTx); the plain `broadcast_transaction` `broadcast.rs:288`; `reservations.rs:154`; the contact payment `payments.rs:1446`; the load replay `manager/load.rs:620`. None holds the wallet-manager guard at the call. | PW |
| F2 | platform-wallet builds only `SpvBroadcaster` (`broadcaster.rs:236`). dw also builds one of its own for PSBT broadcasts (`dw-engine/src/send/psbt.rs:464`), which bypasses anything installed in the manager. It calls `SpvRuntime::broadcast_transaction_and_wait` (`spv/runtime.rs:323-338`), which holds `client.read()` for the whole wait: 60 s acceptance + 5 s grace. That deadline starts only after lock waits with no bound (`client/transactions.rs:89`), so it is not a hard bound. `Rejected` comes back for "client not started", zero peers and `NotConnected`. | PW, dw |
| F3 | dash-spv's `broadcast_transaction` takes the network mutex, checks the peer count and enqueues the tx locally (`client/transactions.rs:32-50`). The mempool task then sends it and records it in `broadcasts` (`sync/mempool/manager.rs:363/465`), which it rebroadcasts every 600 s (`:48`, `:677-741`). That set is filled only by local `broadcast_transaction` and is not persisted. | rust-dashcore |
| F4 | The asset lock is tracked `Built` (`build.rs:1098`) and stored (`:1110`) before the broadcast at `:1142`. `queue_asset_lock_changeset` logs a `store` error and carries on (`asset_lock/manager.rs:220-236`). | PW |
| F5 | dw opens the persister with `SqlitePersisterConfig::new` (`dw-engine/src/session.rs:288`): `FlushMode::Immediate` (`store` commits inline), WAL, `synchronous=NORMAL` (`PWS/sqlite/config.rs:13, 148`). So a returned `store` survives a process crash but not a power loss. | dw, PWS |
| F6 | `resume_asset_lock` runs only when it is called: inside library flows, after an InstantSend timeout (`orchestration.rs:701`, 300 s) and on the ChainLock recovery paths (`registration.rs:197/454`, `orchestration.rs:566`); by the library's deferred-resume task, which waits up to 10 minutes for the transport (`recovery.rs:224, 592`) and can run beside a live build; and by a host's launch catch-up. iOS has one (`catchUpStuckAssetLocks`). **dw has none**: no dw code calls `resume_asset_lock`. Resume claims (`tracking.rs:82`) keep `untrack_asset_lock` from removing the row (`:172`). | PW, dw |
| F7 | Reservation tokens and the funding-account list are in memory only. At load the inputs of a `Built` row are **not** reserved again. The release paths are input-keyed and owner-guarded when they have the token (`reservations.rs:223-263`). Some paths have no token and release unconditionally (the generic send `reservations.rs:154-167`, `payments.rs:1446ff`). | PW |
| F8 | dw's SQLite backend returns an empty `unconfirmed_outgoing_txs`, so the load replay never re-dispatches anything in dw (`PWS/sqlite/persister.rs:1861`). | PWS |
| F9 | In rs-sdk, `put_to_platform_and_wait_for_response` is build and sign, then `StateTransition::broadcast` (`SDK/platform/transition/broadcast.rs:125`), then `wait_for_response` (`:172`). `broadcast_with_retries` (`:280`) re-sends the same bytes, possibly to the same node. The SDK's default is 3 retries, so at most 4 sends (`SDK/sdk.rs:89-95`). The wait has its own 30 s × 3 setting (`rs-dapi-client/src/transport/grpc.rs:438-446`). | SDK |
| F10 | `submit_with_cl_height_retry` (`orchestration.rs:353`) signs again with a higher `user_fee_increase`, so each retry is a new artifact. Re-running a registration from an existing lock (`orchestration.rs:701` → `registration.rs:227`) signs again with the same `user_fee_increase`, and since ECDSA signing is deterministic the bytes are identical. | PW |
| F11 | Shielded redrives store signed state transitions and send them again from the coordinator (`shielded/operations.rs:3187/3257`). Shielded is 1.1 (DEC-20). | PW |
| F12 | The key-wallet `Signer` used by `build_asset_lock_with_signer` signs digests. The blanket `TransactionSigner` impl (`transaction_builder.rs:1073`) gives the signer no view of the transaction, so the vault cannot check a cap before it signs a library-built transaction. | rust-dashcore |
| F13 | `SdkBuilder` cannot take a custom request executor (`SDK/sdk.rs:309, 778`), so the host cannot intercept a state-transition broadcast without an rs-sdk change. | SDK |
| F14 | The library holds `generation.payment_guard()`, a lifecycle read lock, across a broadcast (`manager/load.rs:605`, `signed_payment_registry.rs:423`). The DashPay registration path falls back to the ChainLock proof internally (`registration.rs:171-200`), where its caller cannot see it. | PW |
| F15 | `change_passphrase`, `encrypt`, `recover` and `destroy` reach the vault through the generic `vault_op` (`dw-engine/src/keys.rs:119-136`; `dw-ffi/src/api/vault.rs:519-531`). That emits `VaultLockState` only when the lock state changes (`keys.rs:202-210`), and a passphrase change ends the epoch without changing it. | dw |

## 2. Terms

- **Artifact:** signed bytes that can leave the process: a Core transaction (id: txid) or a state transition (id:
  the hash of its serialized bytes).
- **Registered artifact:** an artifact that platform-wallet persists before its hand-off and may re-dispatch from
  another task or a later process. At the pin that means asset locks. Shielded redrives join them in 1.1 (§7.5).
  Every other artifact is **row-less**: it is handed off only inside the call that signed it.
- **Hand-off:** the call after which the bytes may leave the process. For a Core transaction that is dash-spv's local
  `broadcast_transaction` (an enqueue, F3), or one DAPI request; for a state transition, one
  `StateTransition::broadcast` including its retries.
- **First / Resend:** a First hand-off makes a possible dispatch that did not exist before. A Resend repeats one
  already recorded and commits nothing new.
- **Commit:** the point after which an artifact counts as possibly sent: the J step that grants its First permit.
  For a registered artifact that step is the compare-and-set `Unsent → Committing`, and its `Dispatching` write
  (spawned in the same step) must succeed before any transport call. For a row-less artifact it is the grant, which
  also records its id in the process's row-less set.
- **Origin:** the lease a First is made under: an `OriginTag` (lease id) or `Unleased(kind)` for the engine paths that
  have no lease (§5.2).
- **Permit:** the RAII proof that a First was admitted. It carries a deadline: the grant instant plus H. The lock
  drain waits for permits only, and stops waiting for each one at its deadline.
- **H:** the hand-off deadline, 10 s (Q6).
- **J:** the lease table's `std::sync::Mutex`, held only for in-memory updates, never across an await or any I/O.

## 3. Grants (dw-vault)

### 3.1 `PlatformOp{max_duffs, max_credits}`

- `GrantPurpose::PlatformOp` gets `max_duffs: u64` (Core funding: asset locks for registration and top-up) and
  `max_credits: u64` (state-transition spending, §4.2). Either can be 0, which means the purpose is not granted.
- `requires_credential` is unchanged: `PlatformOp` follows the `Spend` column of the m1-engine §2.2 credential
  table.
- The quick-unlock spend limit does not apply. `QuickUnlock` keeps issuing `Spend` and `SignMessage` only.

### 3.2 `IdentityScan`

- A new purpose, wallet-scoped, uncapped and not `requires_credential`.
- `Vault::scan_key` accepts only an `IdentityScan` token and refuses `PlatformOp` with `GrantPurposeMismatch`. So a
  capped flow token cannot release the master key, as m1-engine §2.2 demands.
- The unattended bring-up asks for `IdentityScan` with `Credential::None`. That works in the prompt-free states only,
  as today.

### 3.3 `authorize_set`

```rust
pub fn authorize_set(&self, purposes: &[GrantPurpose], wallet: Option<&WalletId>, credential: Credential<'_>)
    -> Result<Vec<AuthGrant>, VaultError>;
```

- One credential check (one Argon2id run, one throttle count) issues one grant per purpose, all with the same
  wallet binding and TTL.
- If the vault holds no full-scope key, each grant carries its own copy of the unwrapped key, as single grants do.
- Every purpose must be allowed for the credential, or nothing is issued.
- "Accept and pay" (DASHPAY §2.3) asks for `[PlatformOp{0, accept_cost}, Spend{amount + fee}]` and gets one prompt.

### 3.4 Caps the vault enforces

- `platform_signer(wallet, token, PlatformFunding{max_duffs})` is refused unless `max_duffs ≤ token.max_duffs`, as
  m1-engine §2.2 requires. The engine passes the lease's remaining funding budget.
- Requesting `PlatformIdentity` from a token with `max_credits = 0` is refused.
- Requesting `PlatformFunding` from a token with `max_duffs = 0` is refused.
- The vault still cannot see the transaction it signs for (F12), so the debit check stays in the engine (§4.2).

### 3.5 `KeyHold`: an own key the lease can drop

Today a signer that carries a grant's own key holds an `Arc<Key32>`. Every clone of the signer keeps the key alive,
including clones held inside a library call, so "the lease drops its key" would not erase anything.

- E0-04 changes own-key signers to hold a `Weak<Key32>`.
- `Vault::hold_key(tokens: &mut [GrantToken]) -> Option<KeyHold>` builds the one strong `Arc` from the first own key
  among the tokens and erases every token's copy. Every own key of a grant set is the same data key, so one hold
  serves the whole set, including "Accept and pay"'s two grants.
- `Vault::platform_signer_held(wallet, &KeyHold, token, scope)` and `signer_held(…)` issue signers that reference it.
- When the `KeyHold` is dropped, the key is erased. A signer operation in progress keeps it only until that operation
  ends (≈1 ms), because it upgrades the `Weak` inside the gate. Every later call fails `Locked`.
- Vault-key signers (vault unlocked with scope Full) are unchanged and still depend on the vault's epoch.
- `Vault::epoch() -> u64` (new, public, not secret) lets the engine detect every epoch change, whichever call made it
  (§8.6).

### 3.6 Contract changes (m1-engine §2.2)

- The purpose list adds `PlatformOp{max_duffs, max_credits}` and `IdentityScan`.
- The credential table puts `IdentityScan` in the `Spend` column.
- `authorize_set` gets a row.
- `Vault.lock()` becomes async and returns a `LockReport` (§8.4).
- The `PlatformFunding` note changes from "advisory today" to "capped by the token; the debit is checked at the fence
  (§4.2)".

## 4. Leases (dw-engine `platform/lease.rs`)

### 4.1 What a lease holds

```rust
pub struct Lease {                       // Arc<Lease>; owned by its flow
    id: LeaseId,                         // 128-bit, OS RNG (m-1); never persisted as "live"
    wallet: WalletId,
    flow: FlowKind,                      // Registration, TopUp, ProfileEdit, ContactRequest, Accept, AcceptAndPay, …
    table: Arc<LeaseTable>,
}
struct LeaseEntry {                      // inside LeaseTable, under J
    state: LeaseState,
    purposes: Vec<PurposeBudget>,        // §4.2
    grants: Vec<GrantMeta>,              // purpose and caps; the tokens are dropped once the signers exist
    key: Option<KeyHold>,                // own-key leases only, one per lease (§3.5, §4.4)
    key_until: Option<Instant>,
    signers: LeaseSigners,               // scoped VaultSigners issued from the tokens
    created_gen: u64,                    // lock_gen read before the grants were redeemed (§8.3)
    flow_task: Option<AbortHandle>,      // so close can stop the flow (§8.5)
}
```

- A lease is created from one or more grants with `NetworkSession::begin_lease(wallet, flow, grant_ids)`, in three
  steps:
  1. read `lock_gen`;
  2. redeem every grant (so the 120 s grant lifetime cannot run out mid-flow, DASHPAY §2.3), build the one `KeyHold`
     and issue every signer the lease's purposes need, then drop the tokens;
  3. insert into the table, in a J step that refuses if `lock_gen` has changed since step 1.

  If step 3 refuses, the hold and the signers are dropped and the call fails `lease.locked`; the flow asks again.
- Every token must be for the lease's wallet.
- A lease is ended by `Lease::end()` (or by dropping the last `Arc`). The table keeps its entry until its last permit
  has dropped.
- `LeaseId` values are random, so an origin recorded by an earlier process never matches a lease of this one. The
  fence also compares the wallet.

### 4.2 Purposes and budgets

| Purpose | From grant | Signers it yields | Charged at First admit |
|---|---|---|---|
| `Funding{max_duffs}` | `PlatformOp.max_duffs` | `PlatformFunding{remaining}` | wallet debit of the Core transaction |
| `Credits{max_credits}` | `PlatformOp.max_credits` | `PlatformIdentity` | credit cost of the state transition |
| `Spend{max_duffs}` | `Spend` | the `Spend` signer, for `TxDraft` (DP3-01) | wallet debit of the Core transaction |
| `Crypto` | any `PlatformOp` | `DashPayCrypto` | never hands off |

- **Wallet debit:** the sum of the wallet-owned input values minus the outputs that pay the wallet, which is m1's
  `total_debit = external_sent + fee`. The fence never computes it itself, because that would take the wallet-manager
  guard inside `admit` (§5.8):
  - for an asset lock, the library passes the debit to `register`, from the inputs it just selected; `register`
    charges the `Funding` budget and fails if it does not fit;
  - for a row-less Core send under a lease, which is always engine-built (`TxDraft`), the engine charges the `Spend`
    budget at `prepare`, before signing. The fence charges nothing more.
- **Credit cost:** the explicit credits the transition moves out of the identity (transfer, withdrawal, top-up from
  credits) plus `fee_bound(st)`. `fee_bound` comes from DP1-06's cost table; until that lands, it is a conservative
  constant per transition type, documented next to the table. See Q7.
- **When the charge happens:**
  - Funding: at `register`, under J;
  - Spend: at `TxDraft::prepare`;
  - Credits: at First admit, under J, together with the permit.

  If a charge does not fit, the operation is refused before anything is committed.
- **Refunds:**
  - a `register` whose artifact ends `Revoked` (refused or abandoned) refunds its Funding charge, because it was
    never sent;
  - a row-less First that finishes `NotSent` refunds its charge;
  - a committed registered artifact keeps its charge.
- **Pre-sign check.** Where the engine builds the transaction itself (`TxDraft`), the debit is checked against the
  remaining budget before signing, as m1 does today. The test "a lease cannot sign a BIP44 spend above its cap"
  applies to that path. The library builds asset locks and signs them in the same call (F12). For them:
  - the engine refuses to start a build whose `amount + fee_bound` exceeds the funding budget;
  - `register` refuses a built transaction whose debit exceeds it. The build aborts and releases its inputs, so an
    over-cap asset lock is signed but never tracked and never leaves the process (Q8).

### 4.3 Lifecycle

```
            begin_lease
                │
                ▼
   ┌──────── Active ──────────────────────────────┐
   │   (signs; admits Firsts)                     │
   │      │ funding handed off, own key            │ epoch change other than lock
   │      ▼                                       ▼ (unlock, scope change)
   │  AwaitingProof{key_until}                NeedsGrant
   │      │ key_until passes / park(ProofWaiting) │ rebind(grant) ──► Active
   │      ▼                                       │
   │  Parked{ProofWaiting}  ── (no key; admits Firsts of artifacts already signed)
   │                                              │
   └─► lock_vault / close / change_passphrase / remove_wallet ──► Revoked{cause}   (terminal)
       end() ──► Ended (terminal)
```

| State | Signs | Admits a First | Holds an own key |
|---|---|---|---|
| `Active` | yes | yes | if it was issued on a locked or mixing-only vault |
| `AwaitingProof` | yes, until `key_until` | yes | yes, until `key_until` |
| `Parked{ProofWaiting}` | no (`LeaseError::Parked`) | yes | no |
| `NeedsGrant` | no; `rebind` takes a fresh grant and keeps the id, budgets and permits | yes | no |
| `Revoked{Lock \| Close \| PassphraseChange \| WalletRemoved}` | no | no (refused) | no; dropped in the revoking J step |
| `Ended` | no | no | no |

- **Firsts while parked.** `Parked` and `NeedsGrant` still admit Firsts, because nothing in them needs a key. A
  parked flow has signed nothing new, and an artifact it signed before parking is the same as one an `Active` lease
  holds. Only revocation stops dispatch.
- **Signers stop with their tokens.** A vault epoch change ends every token (E0-03), so every lease's signers fail
  `Locked` after any epoch change.
  - The revoking calls of §8.6 go through dedicated session methods, which freeze before they call the vault.
  - Every other vault call runs through `vault_op`, which compares `Vault::epoch()` before and after. A changed epoch
    moves every lease to `NeedsGrant`, drops every `KeyHold` and re-creates the background lease, whatever the lock
    state did (F15).

### 4.4 Key hold on a locked vault

An own-key lease, one issued on a `Locked` or `UnlockedMixingOnly` vault, holds a `KeyHold` (§3.5).

- **Which flows may hold one.** An own-key registration or top-up must use DASHPAY §2.6's split flow:
  1. `build_asset_lock_transaction` (fenced);
  2. the engine's own proof wait;
  3. `AssetLockFunding::FromExistingAssetLock`.

  The monolithic library path falls back to the ChainLock proof where the engine cannot see it (F14), so it can run
  only under a vault-key lease.
- **Without a proof wait:** `key_until = created + 120 s`, the grant TTL.
- **The InstantSend window:**
  - The split flow calls `lease.begin_proof_wait()` when it starts the engine's InstantSend wait. That sets
    `key_until = now + 300 s` and moves the lease to `AwaitingProof`.
  - That is the window of DASHPAY §2.6, started from an in-memory instant. Nothing persisted moves it (DASHPAY §3.4).
  - The engine's own wait uses the same 300 s, so the key cannot drop before an InstantSend lock that the wait would
    still accept.
  - If a flow never calls it, a funding permit's `finish` sets `key_until = max(key_until, finish + 300 s)` as a floor.
- **Expiry:** a timer task drops the `KeyHold` at `key_until` and moves the lease to `Parked{ProofWaiting}`.
- **ChainLock fallback:** when the engine's wait gives up on InstantSend (`FinalityTimeout`), the flow calls
  `lease.park(ProofWaiting)`, which drops the key at once.
- **Resuming:** a parked registration resumes with a new grant and a new lease ("Finish registering @alice").
- **Unlocked vault (vault-key leases):** no `KeyHold`. The lease lives until the flow ends or a revoking call ends it.

### 4.5 The background `DashPayCrypto` lease

- The session owns at most one per wallet with a seed.
- **It exists** while the vault is `Unlocked` with scope Full, `Unencrypted` or `NoKeys`. It is created on the
  `VaultLockState` event, or at bring-up if the vault is already in one of those states.
- **It is dropped** in the freeze step of `lock_vault` and on any move to `Locked` or `UnlockedMixingOnly`.
- **It is re-created** after an epoch change that keeps the vault prompt-free: a passphrase change, or a scope change
  back to Full. The engine notices those through `Vault::epoch()` in `vault_op` and in the dedicated methods (§4.3),
  not through `VaultLockState`, which a passphrase change does not emit.
- **Its contents:** one `Vault::dashpay_crypto_signer`, no tokens, no budgets.
- **No hand-off, ever.** It has no purpose that hands off, so the fence refuses any `admit` with its origin.
- **What it serves:** the sweeps (contact accounts, decrypted contact xpubs). A crypto product meant for a
  submission (the encrypted xpub of a contact request) is made in the flow whose lease covers that submission
  (DASHPAY §2.6).

### 4.6 What the UI sees

```rust
pub struct LeaseView {
    pub id: String,                  // first 8 hex digits; for logs and the UI only
    pub wallet_id: String,
    pub flow: FlowKind,
    pub state: LeaseStateView,       // Active | AwaitingProof{key_until} | Parked{reason}
                                     //   | NeedsGrant | Revoked{cause} | Ended
    pub own_key: bool,
    pub key_expires_in_secs: Option<u64>,
    pub budgets: Vec<BudgetView>,    // {purpose, cap, spent}
    pub in_flight: u32,              // permits held now
}
```

- `NetworkSession::leases() -> Vec<LeaseView>`.
- Events: `EngineEvent::LeaseChanged{network, lease}`, and
  `EngineEvent::LockProgress{network, phase: Draining{in_flight, deadline_in_ms} | Done(LockReport)}`.
- UI copy, following DASHPAY §2.6:
  - an own-key lease in `Active` or `AwaitingProof`: "Registration in progress — Lock to cancel";
  - while a drain has permits in flight: "Locking — finishing a send already handed off";
  - outcomes: "sent before the lock" or "may have been sent".

### 4.7 API sketch

```rust
impl NetworkSession {
    pub async fn begin_lease(self: &Arc<Self>, wallet: WalletId, flow: FlowKind, grants: &[String])
        -> Result<Arc<Lease>, EngineError>;        // waits while a drain is in progress (§8.3)
    pub async fn lock_vault(self: &Arc<Self>) -> Result<LockReport, EngineError>;   // §8
    pub fn leases(&self) -> Vec<LeaseView>;
}
impl Lease {
    pub fn identity_signer(&self, identity_indices: &[u32]) -> Result<VaultIdentitySigner, LeaseError>;
    pub fn funding_signer(&self) -> Result<VaultSigner, LeaseError>;   // PlatformFunding{remaining}
    pub fn contact_crypto(&self) -> Result<VaultContactCrypto, LeaseError>;
    pub fn spend(&self) -> Result<LeaseSpend<'_>, LeaseError>;        // for TxDraft::prepare_with_lease
    pub fn scope<F: Future>(&self, f: F) -> impl Future<Output = F::Output>;  // DispatchScope::lease(self.id)
    pub fn begin_proof_wait(&self);                                   // §4.4
    pub fn park(&self, reason: ParkReason);
    pub fn rebind(&self, grant_id: &str) -> Result<(), LeaseError>;
    pub fn end(self: Arc<Self>);
}
```

## 5. The dispatch fence

### 5.1 Where the library calls it

**Core transactions.** When the host installs a fence, platform-wallet wraps its broadcaster in a `FencedBroadcaster`.
Every site of F1 goes through it, so one wrapper covers every Core hand-off. The PR also splits
`SpvRuntime::broadcast_transaction_and_wait` into three steps:
1. subscribe to dash-spv's events (no permit);
2. `DashSpvClient::broadcast_transaction`, the local enqueue of F3, under the permit;
3. wait for the acceptance event (no permit).

`client.read()` is held only for step 2, which also stops a pending wait from blocking `stop()` (an E0-05 concern).
`DapiBroadcaster` holds the permit around its one request. dw's PSBT path builds its own `SpvBroadcaster` (F2),
outside the manager, so it bypasses the wrapper. That is acceptable because those bytes are `Unleased(External)`, and
P4 routes it through the fenced broadcaster anyway, so every Core hand-off in dw has one path.

**State transitions.** `Sdk::with_dispatch_fence` (new), called by `StateTransition::broadcast`
(`SDK/platform/transition/broadcast.rs:125`). Every `put_to_platform*` path and every `broadcast_and_wait` reaches
it, so all the platform-wallet call sites the inventory found (identity, documents, DPNS, contactInfo, contact
requests, transfers, withdrawals, tokens, platform addresses, masternode withdrawal) are covered by one call.
`broadcast_with_retries` runs inside one permit:
- each attempt's timeout is clamped to the time left before the permit's deadline;
- no retry starts after the deadline.

**Registration.** At the asset-lock build (`build.rs`, before `track_asset_lock` at `:1098`), platform-wallet calls
`fence.register`. It also calls `fence.abandon` on every exit that would untrack a row that was never handed off
(§5.5).

### 5.2 Origin: `DispatchScope`

- platform-wallet defines a task-local `DispatchScope` (the PR) with the values `Lease(OriginTag)` and
  `Unleased(UnleasedKind)`.
- The engine wraps every library call that can hand off, either in `lease.scope(…)` or in
  `DispatchScope::unleased(kind, …)`.
- The library reads the scope at `register` and `admit`. Where it spawns its own task to finish a hand-off it began
  in a scope, it captures the scope and re-installs it in that task (L11). The engine does the same for its own
  spawns: `tx_actions.rs:414` hands off inside `tokio::spawn`, so the scope is installed inside that future (H7).
- A hand-off with no scope gets no origin. The journal then decides for a registered artifact. A row-less artifact is
  refused: it **fails closed** (Q12), which is safe because a refusal never sends.

| `UnleasedKind` | Paths (dw-engine) | Drained by `lock_vault` |
|---|---|---|
| `Send` | `TxDraft::broadcast` for M1 sends (`send/mod.rs:1072-1079`); see Q2 | no |
| `Mixing` | CoinJoin denomination and collateral transactions (`coinjoin.rs:1539, 2255`) | no; mixing stops on lock anyway |
| `External` | PSBT broadcast of bytes signed elsewhere (`send/psbt.rs:469`) | no |
| `Rebroadcast` | `tx_actions` resend of a transaction already in the wallet's records (`tx_actions.rs:415`) | no |

- **Registered artifacts:** an unleased origin is refused at `register`. Every registered artifact belongs to a lease.
- **Unleased Firsts:** admitted without a permit, as today.

### 5.3 Types

```rust
pub struct DispatchRequest<'a> {
    pub wallet: WalletId,
    pub artifact: ArtifactRef<'a>,  // CoreTx{tx, tracked_row: bool} | StateTransition{st, hash}
    pub origin: Option<DispatchOrigin>,  // from DispatchScope; None when unscoped
    pub site: DispatchSite,         // diagnostic only; never decides the kind
}
pub enum Verdict {
    First(DispatchPermit),          // hand off now, under the permit, until permit.deadline()
    FirstUnleased,                  // Unleased origin: hand off now, no permit
    Resend,                         // a recorded possible dispatch: hand off now, no permit
    Refused { cleanup: bool },      // provably never sent and never will be; `cleanup` only for the CAS winner
    Deferred,                       // not now; outcome unknown (MaybeSent): keep the row and the reservation
                                    // (also a tracked row with no entry: unknown provenance, with a Notice)
}
#[async_trait] pub trait DispatchFence: Send + Sync {
    /// Durable Unsent{origin}, after charging `debit_duffs` to the origin lease's Funding budget.
    async fn register(&self, wallet: WalletId, txid: Txid, debit_duffs: u64) -> Result<(), FenceError>;
    async fn admit(&self, req: DispatchRequest<'_>) -> Verdict;
    /// One J step, no I/O; callable from Drop.
    fn abandon(&self, wallet: WalletId, txid: Txid) -> Abandon;   // Revoked{cleanup} | Committed
}
impl DispatchPermit {
    pub fn deadline(&self) -> Instant;
    pub fn finish(self, outcome: Outcome);   // Drop = finish(MaybeSent)
}
```

`tracked_row` is a fact about the artifact: it has a persisted row. It does not say First or Resend. Only the journal
decides that for such an artifact. A tracked row the journal does not know is `Deferred`, never resent, because of
`tracked_row` alone (Q16).

### 5.4 `admit`: the one decision

```
admit(req):
  if the journal is not loaded yet: → Deferred                    // never waits (§5.8)
  lock J
    e = journal_mem.get(req.wallet, artifact_id)
    match e:
      Revoked                → Refused{cleanup: false}
      Dispatching | PreFence → Resend
      Committing(other)      → Deferred                            // the other caller resolves it; no waiting
      Ambiguous              → e := Committing(retry), spawn the Dispatching write; unlock; await it
                                 ok: e := Dispatching → Resend    fail: e := Ambiguous → Deferred
      Unsent{origin: L}      → if live(L) and L.wallet == req.wallet:
                                   e := Committing(me); p := new permit(L, now + H);
                                   spawn the Dispatching write (it owns e from now on); unlock; await it
                                     ok, now < p.deadline: e := Dispatching → First(p)
                                     ok, deadline passed:  e := Dispatching → Deferred
                                     fail (either way):    e := Ambiguous   → Deferred
                               else: e := Revoked; refund L's Funding charge → Refused{cleanup: true}
      none, tracked_row      → Deferred + Notice{DispatchRecordMissing}
                               // unknown provenance: never sent, never cleaned up (Q16)
      none, row-less         → if id ∈ rowless_admitted: → Resend          // the same bytes again (§5.5)
                               else match req.origin:
                                 Lease(L) live, admissible, Credits fit → p := new permit;
                                                                  insert id → First(p)
                                 Lease(L) otherwise             → Refused{cleanup: false}
                                 Unleased(_)                    → insert id → FirstUnleased
                                 None                           → Refused{cleanup: false}   // fail closed
  unlock J
```

- **`live(L)`:** `L` is in this process's table, not `Revoked` or `Ended`, and is a lease that hands off (not the
  background lease).
- **Origins at load:** an origin read from the journal at load is never live (§4.1).
- **One step:** every read of `e` and every change to it happens under J in one critical section. The durable write
  happens outside J, but `Committing` excludes every other decision on that artifact while it runs, so nothing can
  decide on an earlier read (M-A).
- **The write is spawned inside the J step**, on the blocking pool, and that task owns the resolution of
  `Committing`. A dropped `admit` future therefore never leaves an entry in `Committing` without a writer; the
  orphaned write still resolves it to `Dispatching` or `Ambiguous` (the model's `W` steps).
- **No waiting on another caller.** A caller that finds `Committing(other)` gets `Deferred` at once and keeps
  everything. The committer resolves the entry within H. A resume that was deferred simply ends, and the next resume
  finds `Dispatching`.

### 5.5 `register`, `abandon`, cleanup, the row-less set and `finish`

- **`register(wallet, txid, debit)`:**
  - Its origin comes from the scope, and it must be `Lease(L)` with `L` live, or it fails.
  - It charges `debit` to `L`'s Funding budget, under J, and fails if that does not fit.
  - It writes `Unsent{origin: L.id, process: P}` to the journal durably, then puts it in memory.
  - Registering the same txid again with the same origin is a no-op. With another origin it fails. That is safe
    because the credit output's key index is consumed at signing (`asset_lock_builder.rs:578-620`), so a rebuild of
    the same flow gets a new txid.
  - If `register` fails, the library aborts the build before it tracks anything: it releases the reservation with
    the build's token, nothing is left behind, and it reports a definite not-sent.
- **`abandon(wallet, txid)`:**
  - It is synchronous: one J step with no I/O, since `Revoked` is never persisted.
  - The build calls it on every exit that would untrack a row never handed off: a failed store or flush, a transport
    that is not ready (L8). A **drop guard** calls it too, held from `track` until `admit` has decided, so a build
    future dropped in that window (a host-cancelled call) still abandons (L14).
  - `Unsent` becomes `Revoked`, the Funding charge is refunded, and the answer is `Revoked{cleanup: true}`.
  - `Revoked` is answered `Revoked{cleanup: false}`.
  - `Committing`, `Dispatching` or `Ambiguous` is answered `Committed`: the build keeps its row and reservation and
    reports MaybeSent.
- **Cleanup:**
  - It is done only by the caller that got `cleanup: true`, so it happens at most once per artifact per process, and
    on a reload the catch-up (H6) refuses the row again.
  - The library runs it as a **spawned task** (`shared_handle()`), not inline in the caller's future (L6). So a
    caller dropped right after the verdict, or a drop guard (which cannot await), still gets its row cleaned up.
  - The task untracks the row (overriding resume claims, since `Revoked` proves the bytes never left and never can).
  - It releases the funding reservation owner-guarded by the build's token. The PR keeps the token and the funding
    accounts with the in-memory row, so a resume that wins the CAS can release them too.
  - It queues the row's removal.
  - After a reload there is no token and nothing of this row reserved (F7, L12), so the cleanup only untracks.
  - Other refused callers drop their claims and report Cancelled.
- **The row-less set.**
  - The fence keeps, in memory and per process, the ids of row-less artifacts it answered `First` or `FirstUnleased`.
  - A later `admit` of the same id is a `Resend`, whatever its origin. That covers:
    - a `TxDraft` repeat (`send/mod.rs:1052-1080`);
    - a user resend (`tx_actions.rs:415`);
    - a registration re-run from an existing lock that signs identical bytes (F10).

    Under a revoked lease those bytes would otherwise get a "definite" refusal while dash-spv may still be
    rebroadcasting them (F3).
  - The set is not persisted, and does not need to be. Row-less bytes are handed off only in the process that signed
    them (dash-spv's broadcast set is not persisted either, F3, and dw has no load replay, F8). A flow that resumes in
    a later process signs again, and its own persisted state (the `dp_registration` phase, say) already records the
    earlier MaybeSent.
- **`finish(outcome)`:**
  - It records `Sent`, `MaybeSent` (also the drop default) or `NotSent` on the permit.
  - It feeds the lock report and, for a funding permit, the floor of the IS window (§4.4).
  - It refunds the budget on a row-less `NotSent`.
  - For a registered artifact, a definite transport rejection after `Dispatching` does **not** untrack or release
    (L13). The row stays and is resent later (Q13). The library runs its readiness check (client started, peers > 0)
    **before** `admit`, and a not-ready transport is handled through `abandon`, so this case reduces to a peer loss
    in the moment between the check and the enqueue.

### 5.6 What the library must do (the PR's obligations)

| # | Obligation |
|---|---|
| L1 | Call `admit` at every hand-off of F1 and F9. Never decide First or Resend itself. |
| L2 | Hold no wallet-manager guard, `payment_guard`, `build_persist_serial` or other library lock while it awaits `admit` or `register`. The fence takes none of them, so this is not needed to keep the drain from deadlocking. It is needed so that the journal write, an fsync, never stalls every wallet reader. Model Part 3 also shows why the fence itself must never take the wallet guard. |
| L3 | Make the row durable before `admit` for a tracked row: propagate the `store` result (no log-and-continue as in `queue_asset_lock_changeset`) and `flush` when `!store_commits_inline()`. |
| L4 | Call `register` before tracking an asset lock, with the debit of the inputs it selected, and abort if it fails. |
| L5 | `First(p)`: call the transport only if `now < p.deadline()`, under `timeout_at(p.deadline())`; then `finish`. A timeout is MaybeSent. |
| L6 | `Refused{cleanup: true}` and `Abandon::Revoked{cleanup: true}`: spawn the cleanup of §5.5 as a library task (overriding resume claims), then report the definite not-sent error (`DispatchRefused`). `cleanup: false`: drop the claim and report the same error. Never map `Refused` onto a release path without a token (F7). |
| L7 | `Deferred` and `Abandon::Committed`: keep the row, the reservation and the in-broadcast fence; report the unknown outcome (`TransactionBroadcastUnconfirmed`). |
| L8 | Run the transport readiness check before `admit`; if it fails, `abandon` (for a tracked row) and report the definite rejection. |
| L9 | Never insert an outgoing transaction into the wallet's records, or into dash-spv's broadcast set, before its admitted hand-off. A test checks this. |
| L10 | Split the SPV broadcast as §5.1; in rs-sdk, clamp each attempt to the permit's deadline. |
| L11 | Capture `DispatchScope` across the library's own spawns of hand-off work. |
| L12 | Provide a load-time re-reservation: reserve the inputs of a tracked row the host names (owner: the row), released when the row reaches a terminal status. The host calls it for every possibly-sent row in its catch-up (H6). |
| L13 | After `admit` returned `First` for a tracked row (its `Dispatching` record is durable), a transport rejection neither untracks the row nor releases its inputs. The pin's `Rejected` arm (`build.rs:1144-1233`) applies only before `admit`, through `abandon`. |
| L14 | Hold a drop guard from `track` until `admit` has decided, whose `Drop` calls `abandon` and spawns its cleanup. |

With no fence installed, the library behaves exactly as at the pin, so other hosts (iOS) are unaffected.

### 5.7 What the host must do

| # | Obligation |
|---|---|
| H1 | J is never held across an await, I/O or any library lock. The fence takes no library lock at all, so J is a leaf. |
| H2 | The durable writes happen outside J, on the journal's own connection, with `synchronous=FULL` (§6). |
| H3 | Every permit has a deadline of grant + H, and the drain treats a permit past its deadline as ended, whether or not the library dropped it. |
| H4 | Revocation (the freeze) and the permit snapshot happen in one J step, before `lock_vault`'s first await. |
| H5 | Lease ids are 128-bit random. The journal stores the id, the wallet and the process nonce, and every check compares the wallet too. |
| H6 | **Catch-up at load.** After the journal is loaded and before any user flow, the engine resumes every tracked `Built` or `Broadcast` row of every loaded wallet, unscoped, so the journal decides. A row at `Dispatching` or `PreFence` first gets its inputs reserved again (L12). The same runs when a closed wallet is opened. |
| H7 | Every engine call site that can hand off runs inside a `DispatchScope`, including hand-offs inside a task the engine spawns (`tx_actions.rs:414`). |
| H8 | `begin_lease` reads `lock_gen` before it redeems any grant, and its insert refuses if `lock_gen` changed (§4.1, §8.3). |
| H9 | Every vault call that can end the epoch either is a revoking call with its own session method (§8.6), or runs through `vault_op`, which compares `Vault::epoch()` before and after (§4.3). |

### 5.8 Lock order and why nothing can deadlock

- The draft's design used a FIFO `RwLock` per lease. A task that held the wallet-manager guard G and then asked for a
  shared permit queued behind the drain's exclusive request, while the permit holder waited for G. That is a
  deadlock (model Part 3 finds it).
- In this design nothing in the fence waits on anything another task holds, except J:
  - admission is a non-blocking decision under J. `admit` does not wait for another caller's commit (it answers
    `Deferred`), and it does not wait for the journal load (it answers `Deferred`);
  - the debit is supplied by `register`'s caller, so `admit` never takes the wallet guard;
  - the only thing `admit` awaits is its own spawned journal write, which holds no lock;
  - the drain takes no lock at all, and only waits for permits to end or expire;
  - J is a leaf (H1).
- A permit holder that waits for G (dash-spv's mempool task takes it, F3), or for `payment_guard` (F14), delays only
  itself, and its deadline cuts it. So no wait-for cycle can include the drain or `admit`.
- Model Part 3 confirms that the design has no deadlock. It also shows that an `admit` which took the wallet guard
  would deadlock a caller that holds G behind a queued writer (tokio's `RwLock` is fair). That is why the debit moved
  to `register`.

## 6. The dispatch journal (persistence medium)

### 6.1 Why the host owns it

The draft kept the record in the asset-lock row. Each fix then needed a matching change in three places
(`AssetLockChangeSet::merge`, the SQLite upsert guard in `PWS/sqlite/schema/asset_locks.rs:140-150`, and Swift's
`persistAssetLocks`), plus a flush contract and a tombstone (M-B a–d). The row's wire format is positional bincode
that rejects trailing bytes, so even one new field needs a migration and a versioned blob.

A journal that only the fence writes avoids all of that:
- its writes are single statements the host controls;
- it is monotone by construction;
- it is durable on the host's own terms;
- it never travels through a changeset.

The library keeps its row exactly as at the pin. That row now serves funds tracking only, not the cancel promise.

### 6.2 Medium

- **File:** `<network dir>/dispatch.sqlite`, mode 0600, beside `app.sqlite`. It is a separate file because:
  - it needs a stricter durability setting than `app.sqlite`;
  - it must not take part in `.dwbackup` row export, which takes every table with a `wallet_id` column
    (`dw-appdb/src/rows.rs:69-89`);
  - it must outlive wallet removal (§6.5).
- **Code:** a module of dw-appdb (`dispatch.rs`) with its own embedded migrations. One connection behind a mutex,
  used only from the blocking pool.
- **Pragmas:** `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=2000`, and on Apple platforms `fullfsync=ON`
  and `checkpoint_fullfsync=ON` (macOS `fsync` does not flush the disk cache).
- **Schema:**

```sql
CREATE TABLE meta (k TEXT PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID;   -- schema version, created_at
CREATE TABLE dispatch (
  wallet       BLOB NOT NULL CHECK (length(wallet) = 32),
  txid         BLOB NOT NULL CHECK (length(txid) = 32),
  origin_lease BLOB NOT NULL CHECK (length(origin_lease) = 16),
  process      BLOB NOT NULL CHECK (length(process) = 16),   -- per-session nonce, diagnostics
  state        INTEGER NOT NULL CHECK (state IN (0, 1, 2)),  -- 0 Unsent, 1 Dispatching, 2 PreFence
  registered_at INTEGER NOT NULL,
  dispatched_at INTEGER,
  PRIMARY KEY (wallet, txid)
) WITHOUT ROWID;
```

- **Statements:**
  - `register`: `INSERT … ON CONFLICT DO NOTHING`, then a read-back. A row with another origin is an error.
  - `Dispatching`: `UPDATE dispatch SET state = 1, dispatched_at = ? WHERE wallet = ? AND txid = ? AND state = 0`,
    or a no-op if the row is already 1.
  - No statement sets `state` to 0, and none deletes a row.

### 6.3 States

| In memory | On disk | Meaning |
|---|---|---|
| none | none | row-less artifact; or a row of unknown provenance (kept, not sent, `Notice`) |
| `PreFence` | 2 | the row existed when the journal was created; possibly sent, so Resend only |
| `Unsent{origin}` | 0 | registered, never possibly sent |
| `Committing(owner)` | 0 or 1 | a `Dispatching` write is in progress |
| `Dispatching` | 1 | possibly sent; only Resends from now on |
| `Ambiguous` | 0 or 1 | a write failed; it may still have reached disk |
| `Revoked` | 0 | refused or abandoned in this process; never possibly sent |

- **Monotone:** on disk, 0 → 1 only, and 2 is written only by the seeding. In memory, `Unsent` goes to `Committing`
  or `Revoked`, `Committing` goes to `Dispatching` or `Ambiguous`, and `Ambiguous` goes to `Committing` (retry). None
  of them goes back.
- **`Revoked` is not persisted** and does not need to be. At load every origin is dead, so an on-disk 0 is refused on
  its first `admit`, which is exactly what `Revoked` would do. A row brought back by the library's merge or upsert
  (M-B b) meets the same entry and is refused again. The journal decides, not the row.

### 6.4 Write failures

- **`register` fails:** the build aborts (L4). An entry that reached disk anyway, with no row, is harmless.
- **`Dispatching` fails** (including an fsync error, after which the frames may still be durable): the entry becomes
  `Ambiguous` and `admit` answers `Deferred`.
  - The transport is not called, and the flow reports MaybeSent.
  - In this process the artifact is resent only after a later write succeeds (`Ambiguous` → `Committing(retry)` →
    `Dispatching` → Resend). That retry needs no lease: the commit already happened, under its permit, before any
    lock.
  - At the next load the file decides. 1 means Resend. 0 means the transport was never called, because I1 guarantees
    that, so the row is refused and cleaned up.
- **The journal cannot be opened:** the fence refuses every registered-artifact `register` and every leased First,
  and raises a `Notice{DispatchJournalUnavailable}`. Tracked rows get `Deferred`, so nothing is sent or cleaned up
  without the journal. Unleased paths still work.

### 6.5 Load, loss, rollback, GC

- **Seeding.** When the session opens and `dispatch.sqlite` has no `meta.seeded` row (the first session with the
  fence, or a journal that was deleted), the fence reads every asset-lock row of every wallet from `wallet.sqlite`.
  It uses the read-only connection dw already uses for history (`dw-engine/src/history_ops.rs:86`), so wallets the
  user closed are included. It then inserts a `PreFence` entry for each row whose status is not `consumed`, and sets
  `meta.seeded`, in one transaction. No fence-era row can exist yet: the fence admits nothing before the seeding is
  done.
- **Load:** at session open, after the seeding and before `start_wallet_subsystems` or any `admit`, the fence reads
  every row into memory. Until then `admit` answers `Deferred` and `register` fails (in practice no flow can run that
  early). The catch-up (H6) runs right after.
- **Wallet removal:** journal rows are kept. A wallet removed and imported again finds its old entries, and the
  library's rows for it come back only from `wallet.sqlite`, which has the matching entries.
- **The journal is deleted or reset:** the next session seeds it again, so every current row becomes `PreFence` and
  is resent. That is safe for funds (nothing possibly sent is cleaned up). It can send an asset lock the user had
  cancelled, but only after someone deletes a file by hand.
- **`app.sqlite`** is irrelevant here; the journal does not live there.
- **The journal is rolled back** to an older copy by hand, with `wallet.sqlite` newer: a row registered after the copy
  has no entry, so it is kept and not sent, with a `Notice` (safe). A row dispatched after the copy reads `Unsent`
  and is refused, so its row is untracked even though it may be on the wire. DP1-05's asset-lock reconstruction
  recovers that lock from the chain (`RecoveredFromChain`). dw never restores this file, so this is the only
  residual, and it needs manual file surgery.
- **`wallet.sqlite` is restored or rolled back:** every row meets its entry. A row from before the seeding that the
  seeding never saw has no entry, so it is kept and not sent; platform-wallet still tracks its proofs, which need no
  hand-off. Safe both ways.
- **GC:** none in 1.0 (Q10). A row is about 120 bytes, one per asset lock ever built.

## 7. Commit points and crash consistency

### 7.1 Per artifact class

| Class | Commit | Possibly sent from | Resent by |
|---|---|---|---|
| Registered (asset lock) | start of the `Dispatching` write, under a First permit | that write | resume, deferred resume, load catch-up, dash-spv's rebroadcast (within the process) |
| Row-less Core tx (contact payment, M1 send) | grant of the First permit (or `FirstUnleased`) | the enqueue into dash-spv | dash-spv's rebroadcast set only; there is no load replay in dw (F8) |
| State transition | grant of the First permit | the first broadcast attempt | rs-sdk's retries inside the same permit only; a re-signed retry (F10) is a new artifact and a new First |

### 7.2 An asset lock, step by step

| Step | Durable afterwards | Crash right after: what the catch-up at load does (H6) |
|---|---|---|
| 1. sign (lease's funding signer) | nothing | nothing to do; inputs are free |
| 2. `register` (entry 0) | entry 0 | orphan entry; no row; nothing to do |
| 3. track `Built`, `store` inline (L3) | entry 0, row | row with entry 0 → refused → untracked (nothing of it reserved after load) |
| 4. readiness check; on failure `abandon` → cleanup | as 3, or removal queued | as 3 (a removal that did not land is refused again) |
| 5. `admit`: `Committing`, permit, charge (under J) | as 3 | as 3 |
| 6. `Dispatching` write in progress | entry 0 or 1 | 0 → refused (transport never called, I1); 1 → Resend |
| 7. write done | entry 1, row | inputs reserved again (L12), then Resend (MaybeSent) |
| 8. transport call (enqueue) under `timeout_at(deadline)` | as 7 | as 7; the bytes may already be out |
| 9. transport returns or times out; `finish` | as 7 | as 7 |
| 10. status `Built → Broadcast` (`store`) | entry 1, row `Broadcast` | as 7, through the Broadcast arm (a Resend) |
| 11. proof wait (no permit), IS window (§4.4) | — | platform-wallet's tracking; DP1-02 resumes its flow |

Power loss is the one case the table does not cover. `wallet.sqlite` runs `synchronous=NORMAL` (F5), so the row of
step 3 can be lost while entry 1 of step 7 survives. The lock is then on the chain but has no `Built` row, and DP1-05's
reconstruction finds it as `RecoveredFromChain` (Q11).

### 7.3 Row-less Core transactions

- `admit` decides from the origin (§5.4). There is no journal entry and nothing to persist.
- A First permit covers the enqueue, and its grant records the id in the row-less set.
- After a lock the lease is revoked. A first hand-off of new bytes under it is refused, and since nothing was
  enqueued, nothing can be rebroadcast. A repeat of bytes already admitted is a Resend (§5.5).

### 7.4 State transitions

- The permit covers `StateTransition::broadcast` and its retries, all clamped to the deadline. An identical
  re-signed transition (F10) is a Resend through the row-less set.
- `wait_for_response` runs without the permit.
- A timeout is MaybeSent, and the flow keeps waiting for the proof to learn the outcome.

### 7.5 Re-dispatch paths

| Path | Reaches the fence as | Decided by |
|---|---|---|
| `resume_asset_lock`, Built arm (`recovery.rs:1176`) and Broadcast arm (`:1491`), all callers (F6) and dw's catch-up at load (H6) | CoreTx, `tracked_row`, usually unscoped | the journal (`PreFence` for rows older than the journal) |
| deferred-resume task (`recovery.rs:592`) | same; spawned by the library, unscoped | the journal |
| load replay (`load.rs:620`) | CoreTx, row-less, unscoped | never runs in dw (F8); if it did, the fail-closed rule would refuse it, which is safe (nothing sent) |
| dash-spv's 600 s rebroadcast (F3) | not at all | an entry exists only after a fenced hand-off, so membership is the record |
| shielded redrives (F11, 1.1) | state transition, persisted before dispatch | must be registered artifacts, keyed by ST hash, before the shielded work ships (X-phase) |

## 8. Lock, close and other revocations

### 8.1 `lock_vault`

```rust
pub async fn lock_vault(self: &Arc<Self>) -> Result<LockReport, EngineError> {
    let _op = self.try_enter()?;                     // as today; a closing session handles leases itself
    self.cancel_relock();
    let drain = self.leases.freeze(Cause::Lock);     // (1) one J step, synchronous
    let vault = self.vault.clone();
    let gate = self.rt.spawn_blocking(move || vault.lock()); // (2) the vault gate (E0-03)
    let report = drain.wait().await;                 // (3) the drain
    let status = gate.await?;
    Ok(LockReport { status, ..report })
}
```

1. **Freeze.** In one J step:
   - every lease that is not `Ended` becomes `Revoked{Lock}`, and every `KeyHold` is dropped;
   - the background lease is dropped;
   - the in-flight permits are snapshotted into `S`, and `deadline_max = max(p.deadline for p in S)`;
   - `lock_gen` is incremented.
2. **Vault gate.** `vault.lock()` ends the epoch and waits for gated vault operations already running. It is started
   before the first await and runs to completion even if the future is dropped.
3. **Drain.** The future waits until every `p ∈ S` has dropped or is past its deadline. It wakes on the permits'
   notify or a timer, so it is not polling. It emits `LockProgress::Draining` while it waits.

`relock_after`'s timer runs the same three steps. The FFI's `Vault.lock()` becomes async and calls `lock_vault`.

### 8.2 The bound

Let `t0` be the moment of the freeze, which happens before `lock_vault`'s first await, so it is its call time.

- Every permit in `S` was granted before `t0` (J orders them), so its deadline is less than `t0 + H`.
- No permit is granted after `t0` under a lease that existed at `t0`, because all of them are revoked at `t0`.
- No new lease is created while the drain has unexpired permits (§8.3), so no permit outside `S` can appear for the
  drain to wait on.
- The drain ends at the first instant at which every `p ∈ S` has dropped or passed its deadline. That is at most
  `max_{p∈S} deadline < t0 + H`. The host enforces this itself (H3), so a library that ignores its deadline cannot
  stretch it.
- The vault gate ends at `t0 + T_gate`.

So **`lock_vault` returns by `t0 + max(H, T_gate) + ε`**, where ε is the timer's wakeup latency (tokio's 1 ms
granularity plus scheduling). This holds for any number of leases and permits.

- `T_gate` is the E0-03 gate wait: about 1 ms per in-flight signature, or one vault-file write and its read-back.
  H does not bound it.
- The tests assert the bound with paused tokio time, where ε = 0, and with real time at a tolerance of 250 ms.
- In ROADMAP and DASHPAY, "within H of the freeze" and "H plus one poll" both become "within `max(H, T_gate)` of the
  call, plus timer slack".

### 8.3 How the drain ends, and what happens around it

- **Every permit ended or expired:** the normal end. Permits that expired are reported MaybeSent.
- **The future is dropped mid-drain:** nothing changes. Revocation already happened at the freeze; the permits keep
  their deadlines; the vault gate finishes on the blocking pool. There is no admission state to reopen (M-C).
- **A second `lock_vault` during a drain:**
  - its freeze revokes nothing new (no lease can have been created, see the next item);
  - its snapshot is the subset of `S` still in flight, so it returns by the same `t0 + H`;
  - the second call therefore joins the first.
- **`begin_lease` across a freeze.** `begin_lease` reads `lock_gen` before it redeems anything, and its insert
  refuses if `lock_gen` changed (H8). So a lease whose redemption began before a lock's call and whose insert would
  come after it never exists. Its tokens were redeemed under the old epoch and are dropped unused, and the flow
  asks again. That closes the window in which such a lease could sign before the vault gate ends the epoch.
- **`begin_lease` during a drain** also waits until no permit granted before the latest freeze is still in flight
  and unexpired. That condition is computed from the permit table, not from a flag the lock future owns, so a dropped
  future cannot wedge it. The wait is at most H.
- **Unlocking during a drain** works: the vault was locked in step 2. A grant during the drain also works; only the
  lease made from it waits. So "lock_vault returned" is a clean boundary for the UI (nit 4).

### 8.4 Outcomes

```rust
pub struct LockReport {
    pub status: VaultStatus,
    pub revoked: Vec<LeaseSummary>,             // {id, flow, wallet}
    pub in_flight: Vec<(LeaseSummary, Outcome)>, // each permit of S: Sent | MaybeSent | NotSent
}
```

A flow whose lease was revoked reports one of four outcomes:
- **Cancelled:** its First was refused, or it had nothing in flight. It is definite, and its registered artifact (if
  any) is `Revoked` and cleaned up.
- **Sent:** its permit finished `Sent` before the drain ended ("sent before the lock").
- **MaybeSent:** its permit expired, or its outcome is unknown ("may have been sent").
- **Committed, not yet sent:** a registered artifact stuck in `Ambiguous` or `Deferred`. It is reported as MaybeSent,
  and a later Resend may send it.

### 8.5 Close

`NetworkSession::close` runs these steps in order, before its `gate.close()`:
1. the freeze with `Cause::Close`;
2. the drain (at most H);
3. abort every flow task registered with a lease (`flow_task`);
4. then the existing steps (gate, pump, manager shutdown).

Flows hold the session's `OpGuard` while they run, so the abort must come before `gate.close()`, which waits for
guards. A flow aborted after the drain is outside every permit, or inside an expired one. The library's drop handling
(sticky claims, `InBroadcastPin::drop`) then settles it as pending, and a committed asset lock is resent at the next
session. E0-05 owns the cancelation of the bring-up task. This design adds only the lease steps.

### 8.6 Other revocations and epoch changes

| Event | Leases | Drain | Background lease |
|---|---|---|---|
| `lock_vault`, relock timer, FFI `Vault.lock()` | all `Revoked{Lock}` | yes | dropped |
| close | all `Revoked{Close}` | yes, then abort flows | dropped |
| `change_passphrase` (vault stays unlocked; epoch ends) | all `Revoked{PassphraseChange}` (Q4) | yes | re-created |
| `remove_wallet` | that wallet's, `Revoked{WalletRemoved}` | yes (that wallet's permits) | that wallet's dropped |
| unlock or scope change (epoch ends) | `NeedsGrant`, `KeyHold` dropped (Q5) | no | created or dropped by the new state |
| encrypt, recover, destroy | as `lock_vault` | yes | dropped |
| any other call that ends the epoch (caught by `vault_op`'s epoch check, H9) | `NeedsGrant` | no | re-created if the vault is prompt-free |

The revoking rows (lock, close, passphrase change, wallet removal, encrypt, recover, destroy) each get a session
method that freezes first: `lock_vault`, `close`, and new `change_passphrase`, `encrypt_vault`, `recover_vault`,
`destroy_vault` and `remove_wallet`. dw-ffi's `Vault` calls those instead of the generic `vault_op` (F15).

## 9. Why it is correct

### 9.1 Invariants

- **I1. Commit before hand-off.** The transport of a registered artifact is called only after its `Dispatching`
  write has returned success, in this process or an earlier one (`admit` returns `First` or `Resend` only from
  `Dispatching`).
- **I2. Commits only under a live lease.** `Unsent → Committing` and every row-less `First` happen in a J step that
  finds the origin lease live. Every lease that exists when `lock_vault` is called is revoked in that call's freeze, a
  J step. So no commit is ordered after a lock's call under a lease that lock revoked.
- **I3. Exclusive outcomes.** `Revoked` and `Dispatching` (or `Committing`, or `Ambiguous`) are reached only from
  `Unsent`, each by a compare-and-set under J. On disk, 0 → 1 only. At load, `Unsent` with a dead origin is refused,
  as `Revoked` would be. `PreFence` is never `Unsent`: it counts as `Dispatching` and is never cleaned up. A tracked
  row with no entry is neither sent nor cleaned up.
- **I4. A refusal is definite.** `Refused` or `Revoked{…}` ⇒ the entry was `Unsent` (I3) ⇒ no `Dispatching` write
  ever succeeded ⇒ no transport call ever happened (I1) and none can (I3). So the cleanup, which happens only on
  `cleanup: true`, never touches bytes that may be on the wire. For a row-less artifact a refusal is given only to an
  id that was never admitted in this process (the row-less set), and row-less bytes are handed off only in the
  process that signed them (§5.5).
- **I5. The drain is complete.** `lock_vault` returns only after every permit granted before its freeze has dropped or
  expired (H3, H4).
- **I6. One owner-guarded cleanup that cannot be dropped.** Only the CAS winner cleans up, at most once per process.
  The cleanup is a spawned task, so dropping the caller cannot lose it. A build dropped before `admit` abandons
  through its drop guard. The release takes the build's token, so it never frees another build's hold.
- **I7. Origins die with the process.** Lease ids are random and in memory only.
- **I8. The row is durable before its hand-off** (L3). That is for funds tracking, not for the cancel promise.
- **I9. No lease outlives the freeze it raced.** Every lease in the table at a freeze is revoked by it, and a lease
  whose creation began before a freeze is never inserted after it (H8).

### 9.2 The ordering theorem

> After `lock_vault` returns, no artifact signed under a lease it revoked makes a hand-off whose commit was not
> ordered before the `lock_vault` call. Every hand-off whose commit was ordered before the call has ended (Sent or
> NotSent) or is reported MaybeSent.

- **Proof.** By I2, every commit under a revoked lease precedes the freeze in J's order, and the freeze happens at the
  call. A First's commit is the grant of its permit. If that permit's hand-off had not ended by the return, I5 means
  it was past its deadline, so it is reported MaybeSent. A registered artifact's later Resends (resume, load) all
  follow its `Dispatching` write (I1), which belongs to that same commit.
- **Corollary (Lock to cancel).** A flow whose First is refused reports Cancelled. By I4 nothing of it ever leaves,
  and its row and reservation are released once, by a cleanup nothing can drop (I6).

### 9.3 Liveness

- **A genuine possible dispatch is never lost.** An entry at 1 or 2 is resent by every resume, and by the catch-up
  at every load (H6), with its inputs reserved again (L12). Cleanup, which needs `Unsent`, can never reach it (I3).
- **A never-dispatched row is cleaned up** at once by the CAS winner in this process, or by the catch-up at the next
  load (H6), since its lease died with the process.
- **No deadlock** (§5.8).

## 10. The model check

### 10.1 What it models, and its limits

`docs/design/checks/e0_04_design_model.py`:
- **Part 1** explores every interleaving of these actors:
  - O, the original flow, which may be dropped between track and admit;
  - R, a resume of the same row; after a crash, R is the catch-up at load (H6);
  - K, a spawned cleanup task;
  - W, an orphaned journal write;
  - L, `lock_vault`;
  - B, another build that may take freed inputs after a reload;
  - one crash with every partial outcome, and the reload.

  Its seven scenarios are:
  - `flow`;
  - `flow+crash`;
  - `restart`, an unsent row at load;
  - `ambiguous`;
  - `legacy`, a `PreFence` row;
  - `rowless`, a row-less First, then a repeat of the same bytes, then the lock;
  - `unknown`, a row with no entry.
- **Part 2** is a tick simulation of the bound. It also checks the race between lease creation and the freeze.
- **Part 3** explores the lock order: the draft's permit lock, the design's non-blocking admit, and an admit that
  would take the wallet guard.

Atomic steps:
- each J critical section is one step: `admit`'s decision, `abandon`, the freeze, a permit drop;
- each durable write is one step with three outcomes: ok, failed, and failed but durable;
- each transport call has its outcomes: sent, rejected, deadline passed, then bytes leaving late or never.

Limits. What the model does not cover:
- more than one registered artifact, lease, resume or crash;
- the library's internals (claims, the in-broadcast pin), which are abstracted to "row tracked" and "inputs reserved";
- budgets and caps, which are plain arithmetic under J and are covered by unit tests (§12);
- epoch changes other than the lock, and the background lease (§4.3, §8.6; unit tests);
- the vault gate's own ordering, which E0-03's lock-race tests cover.

Some checks are confirmations rather than discoveries, and they are labelled as such. In Part 2, "no First after
the call" and "a second call joins" follow from revocation at the freeze, so the design passes them by construction.
Their teeth are the rejected rules, which fail them. Part 1's "deadlock" check ignores only the step that drops
`lock_vault`'s future, so a holder that could never end would show; no mutation produces one, and Part 3 covers lock
order.

It is a model of this design's decisions, not a test of the code.

### 10.2 Property → check

| Property | Check (violation text) |
|---|---|
| I2, theorem | "a commit after lock_vault was called"; "a first actual send after lock_vault returned" |
| I1 | "a hand-off without a durable Dispatching record" |
| I8 | "a hand-off before its row was durable" |
| I4 | "a possibly-sent row cleaned up"; "reported not sent, but sent or still sendable" (also for a row-less repeat) |
| I6 | "the cleanup ran twice in one process"; "another build's reservation released"; "a never-dispatched row left reserved" (a dropped build or caller) |
| I3 | "an artifact both Revoked and Dispatching" |
| I5 | "lock_vault returned while a hand-off it waits for ran". It is judged from the hand-offs themselves (a First begun before the freeze, still running, not past its deadline), not from the permit set. |
| I7 | "a commit under an origin from an earlier process" |
| I9 | Part 2: a lease begun before the call and inserted after it |
| H6, L12 | "inputs of a possibly-sent row left selectable after load"; "a genuine possible dispatch was not resent" |
| liveness | "a never-dispatched row left reserved"; "a genuine possible dispatch was not resent" (ambiguous and pre-fence rows); "deadlock" |
| unknown provenance | "a hand-off without a durable Dispatching record"; "a row of unknown provenance cleaned up" |
| bound | Part 2: the drain ends within H for 1–3 leases; the rejected timing rules fail |
| lock order | Part 3: no reachable state without a step, for the design's admit |

### 10.3 Replays

- **The r4 M1 counterexample.** Under the r3 rule (kind from the call path, the pin's claim exclusion), the model
  admits it and flags "a first actual send after lock_vault returned". The design admits no such trace. Its
  counterpart ends with nothing sent, the row gone, the reservation released once and O reporting Cancelled.
- **The M-A interleaving.** O dispatches, the lock comes, and only then does the resume act. Under the design the
  resume's admit reads `Dispatching` and resends, and the inputs stay reserved.
- **Barrier 1.** Nothing handed off; Cancelled; row gone.
- **Barrier 2.** `lock_vault` cannot return before the hand-off; Sent.
- **Barrier 2 with a stall.** Returns at the deadline; MaybeSent; late bytes are allowed.

### 10.4 Mutations (each must be caught)

| Mutation | Stands for | Caught as |
|---|---|---|
| `r3` | kind from the call path; claim exclusion | hand-off without a durable record (restart) |
| `split-literal` | read, fence and write as separate steps (M-A) | possibly-sent row cleaned up |
| `no-override` | a refusal does not override another party's claim | never-dispatched row left reserved |
| `no-cleanup` | a refusal leaves the row | never-dispatched row left reserved |
| `strict-resend` | a Resend needs a live lease | ambiguous dispatch not resent |
| `check-then-act` | lease check and permit in separate steps | commit after lock_vault was called |
| `transport-first` | the transport before the record is durable | hand-off without a durable record |
| `clean-on-write-fail` | a failed write leaves a clean `Unsent` (M-B c) | reported not sent, but still sendable |
| `legacy-unsent` | a `PreFence` row treated as never sent (M-B d) | possible dispatch not resent; row cleaned up |
| `no-entry-resend` | a row with no entry resent (trusting the call site's `tracked_row`) | hand-off without a durable record |
| `abort-direct` | an aborted build releases without the fence | possibly-sent row cleaned up |
| `no-flush` | hand-off before the row is durable | hand-off before its row was durable |
| `revoke-in-drain` | revocation at the drain's end; a drop reopens (M-C) | commit after lock_vault was called |
| `no-drain-wait` | return without waiting for the snapshot | lock_vault returned while a hand-off ran |
| `lease-reuse` | per-process lease ids (m-1) | commit under an earlier process's origin |
| `unguarded-release` | release without the build's token | another build's reservation released |
| `rowless-no-memory` | a row-less repeat under a revoked lease is refused | reported not sent, but sent |
| `no-rereserve` | the catch-up leaves a possibly-sent row's inputs free | inputs left selectable after load |
| `no-drop-guard` | a build dropped before admit keeps its row | never-dispatched row left reserved |
| `inline-cleanup` | the refused caller cleans up inline | never-dispatched row left reserved (caller dropped) |
| `every-refuser-cleans` | every refused caller cleans up | the cleanup ran twice in one process |
| `permit-ends-at-first` | the permit is dropped when the transport starts | lock_vault returned while a hand-off ran |

- `split-cas`, a split read and act whose act is a compare-and-set, passes. It is what the single J step guarantees.
- Part 2 fails each of `sequential`, `transport-deadline`, `host-waits`, `drop-reopens` and `no-gen-check`.
- Part 3 finds the draft's deadlock, and the one an `admit` taking the wallet guard would cause.

The three draft inputs stay unchanged as history. The design check replaces `e0_04_dispatch_model.py` as the spec
check DASHPAY §2.6 names.

## 11. The upstream PR (B5) and the interim

- **Content:** one PR against platform `v5.1-dev`, with two commits:
  - (1) rs-sdk: `DispatchFence` trait and types in a small module that platform-wallet re-exports;
    `Sdk::with_dispatch_fence`; the call and the per-attempt clamping in `StateTransition::broadcast`;
  - (2) platform-wallet: `FencedBroadcaster`, the SPV split, `register` with the debit, the synchronous `abandon`
    and its drop guard, the spawned cleanup with the token kept on the in-memory row, the load-time re-reservation,
    `DispatchScope`, and the obligations L1–L14 with their tests.

  With no fence installed, behaviour is unchanged.
- **Carriage:** the desktop carries it as a cherry-pick onto its pin branch (DEC-18), next to E0-11's carried branch.
  The PR is opened only with pasta's go-ahead (B5), which this design asks for at its approval.
- **Interim, until the pin carries it:**
  - P1 and P2 ship (§13).
  - Engine-owned hand-offs (`TxDraft`) can already hold a permit, taken around the whole library call and cut at H,
    since the library's broadcast-and-wait is not split yet. That gives a correct but coarse MaybeSent for a send
    still waiting for acceptance.
  - Flows driven by the library keep DASHPAY's interim wording: "Locking stops new signatures; a send already signed
    may still go out." E0-04 is not done for them until P4.

## 12. Test plan

All tests follow the vault race tests' style. They use a recording fake transport, and real-time stress runs use the
abortable rendezvous from E0-03 `a79b3a9`.

**dw-vault (P1)**
- `PlatformOp` caps:
  - `platform_signer` refuses `PlatformFunding{max_duffs + 1}`;
  - a token with `max_credits = 0` gets no `PlatformIdentity`;
  - a token with `max_duffs = 0` gets no `PlatformFunding`.
- `scan_key` refuses a `PlatformOp` token and accepts `IdentityScan`, in every vault state of the credential table.
- `authorize_set`:
  - one passphrase check (counted by the throttle as one);
  - one grant per purpose;
  - all or nothing when a purpose is not allowed for the credential.
- `KeyHold`: dropping it erases the key while a signer clone is held. The clone then fails `Locked`, and an operation
  in flight finishes. Lock-race style: 16 signers, 300 drops.

**Lease table (P2)**, with paused tokio time unless noted:
- The state machine of §4.3, transition by transition, including `NeedsGrant` and `rebind`.
- Budgets:
  - an engine-built BIP44 spend over the remaining `Spend` budget is refused before signing ("a lease cannot sign a
    BIP44 spend above its cap");
  - a fenced Core First over the `Funding` budget is refused, with Revoked and cleanup;
  - a row-less `NotSent` is refunded.
- `DashPayCrypto` signs no transaction and no state transition: every hand-off purpose is refused, and the scope
  refuses signing (E0-03 tests stay).
- Background lease:
  - created in `Unlocked` and `Unencrypted`, absent in `UnlockedMixingOnly` and `Locked`;
  - dropped in the freeze;
  - re-created after `change_passphrase`.
- Key hold:
  - 120 s without funding;
  - 300 s after the funding `finish`;
  - `park` drops it at once;
  - the signer then fails `Locked`, and the lease is `Parked{ProofWaiting}`.
- Bound:
  - one, two and eight leases with stalled permits: `lock_vault` returns at most `max(H, T_gate)` after the call;
  - a First at 0.9 H is refused;
  - a second `lock_vault` joins;
  - a dropped `lock_vault` future leaves every lease revoked and `begin_lease` unblocked at the deadline;
  - `begin_lease` during a drain waits, then succeeds;
  - `begin_lease` paused between its redemption and its insert while `lock_vault` runs: the insert is refused
    (`lease.locked`), and the redeemed signers fail `Locked`.
- Epoch wiring: `change_passphrase`, `encrypt`, `recover` and `destroy` through dw-ffi revoke and drain. A vault call
  made through `vault_op` that ends the epoch moves leases to `NeedsGrant` and re-creates the background lease (H9).
- Key hold:
  - `authorize_set` on a locked vault yields one `KeyHold`, and no token copy of the key survives (the erased
    buffers are checked under the test allocator);
  - `begin_proof_wait` starts the 300 s window, and `park` on `FinalityTimeout` drops the key.
- Real time: the same bounds, within 250 ms.

**Journal (P2)**
- The CAS statements. No `1 → 0` statement exists (schema test). Register is idempotent; the same txid with another
  origin is refused.
- Fault injection through a `JournalBackend` trait with a failing implementation: a `register` failure aborts the
  build.
- A `Dispatching` failure, in both forms (lost, and durable despite the error), gives `Ambiguous` and Deferred, then
  a retry gives Resend; across a reopen, 0 means refused and 1 means Resend.
- An unopenable journal refuses leased Firsts and raises the `Notice`.
- `synchronous=FULL` (and `fullfsync` on Apple) are read back after opening.

**Fence conformance (P2 against a fake library, P4 against platform-wallet)**. Each case runs for a Core transaction
and for a state transition.
- **Barrier 1:** a flow holding a released signature pauses before `admit`; `lock_vault` runs and returns; the flow
  resumes. Nothing is recorded, it reports Cancelled, and the `Built` row is gone with its inputs released once.
- **Barrier 2:** the flow pauses after `admit` returned `First`, before the transport. `lock_vault` on another thread
  has not returned after 200 ms; the flow hands off once, and only then does `lock_vault` return. It reports Sent.
- **Barrier 2 with a stalled transport:** `lock_vault` returns within `max(H, T_gate)`; the flow reports MaybeSent,
  never Cancelled.
- **Barrier 2 with a stalled `Dispatching` write:** the same bound; the flow reports Deferred and MaybeSent, and no
  transport call starts after the deadline (L5).
- **A row-less repeat:** a `TxDraft` under a lease times out (MaybeSent), `lock_vault` returns, and the repeat is
  admitted as a Resend. It is never reported Cancelled, and the inputs stay reserved.
- **A dropped build:** the build future is dropped after `track` and before `admit` decides. The drop guard
  abandons, the spawned cleanup untracks the row and releases its inputs once, and nothing is recorded. The same
  holds when the CAS winner's future is dropped right after its verdict.
- **Mutation build:** a fence that checks the lease outside J (check-then-act) fails barrier 2.

**Dispatch-record cases (P4, asset locks through platform-wallet)**
1. **A resume of an unsent row.** The flow tracks `Built` (entry 0) and pauses before `admit`. A resume claims the
   row and pauses before its `admit`. `lock_vault` returns. Either one is refused first (both orders); the winner
   cleans up once, and the other reports Cancelled or drops its claim. Nothing is recorded.
2. **An unsent row at load.** A row with entry 0 is refused by recovery and untracked; nothing is reserved after
   load (F7), and nothing is recorded.
3. **A genuine ambiguous dispatch.** Entry 1 and a stalled transport give MaybeSent; `lock_vault` returns; a resume
   then sends a Resend, which the fake records.
4. **M-A order.** The resume admits after O's commit and after the lock: Resend; the inputs stay reserved.
5. **A pre-fence row** (`PreFence`, from the seeding): Resend at load, never cleaned up. The seeding covers closed
   wallets and skips `consumed` rows.
6. **A deleted journal:** the next session seeds it again; every `Built` row is resent; nothing is cleaned up.
7. **A failed `Dispatching` write:** no hand-off now; the row and reservation are kept.
8. **A row with no entry** (an older `wallet.sqlite` copied in by hand): not sent, not cleaned up, `Notice`.
9. **The catch-up at load (H6):** after a kill at each step of §7.2, the next session's catch-up does what the
   table's last column says. Entry 1 rows get their inputs reserved again before any other build can select them,
   and a build started right after the catch-up cannot spend them.

**Library obligations (P3, in the platform PR)**
- At every F1 and F9 site, `admit` and `register` run with no wallet-manager guard or `payment_guard` held (a
  test-only guard-depth probe).
- `register` comes before `track`, and the row is durable before `admit` (a fake persister in Manual mode).
- `Refused{cleanup: true}` overrides a resume claim, and the release is owner-guarded (a newer build's reservation
  survives).
- `Deferred` keeps the row and the in-broadcast fence.
- The readiness check comes before `admit`.
- **Nothing enters `unconfirmed_outgoing_txs` or dash-spv's broadcast set before its admitted hand-off (L9, nit 3).**
- rs-sdk: one `admit` per `broadcast`, however many retries; the per-attempt timeout is clamped; no attempt starts
  after the deadline.

**Stress (P4)**
- 16 flows (asset locks and state transitions mixed), 4 resume tasks, and 300 `lock_vault`, unlock and `begin_lease`
  cycles.
- Random pauses at every step, and random write and transport faults.
- The fence and the fake transport log every J step and every transport call in J order. A checker replays the log
  against I1–I6:
  - each recorded hand-off's commit precedes the freeze of every lock that revoked its lease;
  - no refused artifact is ever recorded;
  - each lock returns within `max(H, T_gate)` + 250 ms;
  - each reservation is released at most once.
- Mutation runs: the stress must fail with check-then-act, revoke-in-drain and transport-first builds.

**Kill matrix (P4, T1)**
- A subprocess harness with fail points, env-selected, at every step of the §7.2 table, run against regtest dashd.
- Kill -9, restart, and check the outcome column. This is the base DP1-02's kill matrix grows from.

**FFI and integration (P4)**
- `Vault.lock()` is async in dw-ffi and both UI shells, and `LockReport` maps to the UI copy.
- Regtest: a registration with lock during the IS wait gives MaybeSent or Sent, then a keyless park. Lock before the
  funding hand-off gives Cancelled with no transaction in the mempool.
- Devnet (T2): DAPI blackholed during an identity create gives MaybeSent at H; the proof arrives later or not.

## 13. Phase plan

| Phase | Content | Needs | Size | Done when |
|---|---|---|---|---|
| P0 | this design, design review (DEC-57) | — | — | review closed; open questions decided |
| P1 | dw-vault: §3 (caps, `IdentityScan`, `authorize_set`, cap from token, `KeyHold`); m1-engine §2.2 updated | P0 | S | P1 tests green; second-agent review |
| P2 | dw-engine: lease table (with `lock_gen`), background lease, `LeaseView` and events, async `lock_vault` and close steps, the revoking session methods and `vault_op`'s epoch check (H9), FFI async lock, `dispatch.sqlite` journal with its seeding, `EngineFence` (row-less set included) against a fake library harness | P1 | M | lease, journal and fence-conformance tests green, including the stress checker on the fake library; model check passes |
| P3 | the platform PR (§11) and its tests; the cherry-pick on the dw pin branch | P0, B5 | M–L | PR open upstream; cherry-pick builds with dw; the PR's L-tests green |
| P4 | wiring: install the fence on the manager and the SDK; scopes on every engine call site (§5.2, H7), PSBT through the fenced broadcaster; the catch-up at load (H6) in the bring-up; the dispatch-record cases, stress and kill matrix; DASHPAY §2.6 and ROADMAP updated | P2, P3, E0-05 (bring-up order, flow-task cancelation) | M | the E0-04 ROADMAP acceptance list (as amended by §15) green |
| P5 | (if Q2 = yes) `TxDraft` leases for M1 sends; m1 contract and both shells updated | P4 | S | send flow tests updated; `send.cancelled_by_lock` |

P1 and P2 need no upstream change and can start as soon as the review closes. DP1-02 can build against P2's API and
the fake library while P3 waits on B5.

## 14. Open questions

Each question has a recommendation. Q1 is pasta's (B5). The rest are the manager's (DEC-22 practice) or the review's.

| # | Question | Recommendation |
|---|---|---|
| Q1 | B5: may we open the one platform PR of §11, an rs-sdk hook plus platform-wallet wiring, against `v5.1-dev`, and carry its cherry-pick? | **Yes.** Without it, library flows never get Lock to cancel. The PR is opt-in (no fence, no change), and smaller than the draft's, since it no longer touches the changeset merge, the SQLite upsert or the row format. |
| Q2 | Should M1 `TxDraft` sends be leased too, so Lock cancels a prepared but not yet broadcast payment? | **Yes, as P5.** It costs one per-send lease from the `Spend` token and a new `send.cancelled_by_lock`. It makes "Lock always wins" uniform. Until then M1 sends are `Unleased(Send)`, as today. |
| Q3 | CoinJoin, PSBT and rebroadcast as `Unleased` (not drained)? | **Yes.** Mixing signs with an unattended signer and stops on lock. PSBT bytes were signed elsewhere. A rebroadcast commits nothing new. |
| Q4 | Does `change_passphrase` revoke flow leases (freeze and drain) although the vault stays unlocked? | **Yes.** The user may be reacting to a leaked passphrase; E0-03 already ends every token, so the leases cannot sign anyway; stopping their dispatch too keeps one rule: a security action revokes. |
| Q5 | Do unlock and scope change (epoch ends) revoke leases, or move them to `NeedsGrant`? | **`NeedsGrant`.** Revoking would make "unlock" cancel a registration started on a locked vault, which no user expects. Dispatch of artifacts already signed continues, and `rebind` resumes signing (without a prompt when the vault is now `Unlocked` and the host setting allows `Credential::None`). |
| Q6 | H = 10 s? | **Yes.** A Core First is a local enqueue plus one fsync; a state-transition First is clamped by the deadline. The UI then waits at most 10 s on Lock, only when something is in flight. |
| Q7 | `max_credits` bounds an estimate (explicit credits plus `fee_bound`), since Platform charges the actual fee at execution. Accept? | **Accept.** Platform cannot charge more than the identity holds, and each re-signed retry (F10) is charged again. DP1-06 replaces the constants with its cost table. |
| Q8 | Ask key-wallet for a pre-sign hook (`Signer::approve_transaction(tx, inputs)`), so the cap blocks the signature of a library-built asset lock and not only its hand-off? | **Defer.** The hand-off refusal guarantees nothing over the cap leaves the process, and the bytes are an asset lock that credits the user's own identity. File it as a rust-dashcore follow-up. |
| Q9 | During a drain, does `begin_lease` wait or fail? | **Wait** (at most H). The UI is showing "Locking…" then anyway, and waiting keeps "lock_vault returned" a clean boundary. |
| Q10 | Journal GC? | **None in 1.0.** About 120 bytes per asset lock ever built. Revisit if a profile ever shows it. |
| Q11 | Power loss can drop a `Built` row (`wallet.sqlite` at `synchronous=NORMAL`) after its lock was sent. Raise wallet.sqlite to FULL? | **No.** FULL makes every changeset fsync during sync. Rely on DP1-05's reconstruction (`RecoveredFromChain`), and document the case. |
| Q12 | Fail closed for unscoped row-less hand-offs? | **Yes,** with a debug assertion and a `Notice` in release builds. A missed scope breaks a flow visibly, in its tests, instead of silently escaping Lock. |
| Q13 | A registered artifact whose transport returned a definite rejection after `Dispatching` stays committed, and is resent later, even after a lock (L13). Accept? | **Accept.** The alternative, the pin's own `Rejected` arm (untrack and release when no claim exists), is also fund-safe: the entry at 1 is then orphaned. But a kill before that removal lands would resend, at the next load, bytes the user was told were not sent. The readiness check before `admit` (L8) narrows the case to a peer loss in the moment between the check and the enqueue. The UI says "will be sent when the network is back". |
| Q14 | Separate `dispatch.sqlite`, or a table in `app.sqlite`? | **Separate file** (§6.2): stricter durability, kept out of `.dwbackup` export, and kept past wallet removal. |
| Q15 | Shielded redrives (1.1) persist signed state transitions. | **Rule:** they become registered artifacts, keyed by ST hash, before shielded ships. The X-phase task adds `register` on their persist path. |
| Q16 | A tracked row with no journal entry: Resend it (trust the library's `tracked_row` and assume pre-fence) or keep it unsent? | **Keep it unsent**, with a `Notice`. The seeding gives every real pre-fence row an entry, so a row without one is either a bug (a missed `register`) or a file restored by hand. Resending would reopen the r4 shape: a possible dispatch inferred from the call site. Keeping the row loses nothing, since platform-wallet tracks its proofs without a hand-off. |
| Q17 | The catch-up at load (H6) and the re-reservation (L12) are new behaviour. dw had no catch-up at all, and the pin never reserves a loaded row's inputs again (F7). Make them part of E0-04, the re-reservation in the platform PR? | **Yes.** Without the catch-up, a row whose bytes never left (killed between the write and the enqueue) is never resent, and an `Unsent` row is never cleaned up. Without the re-reservation, another build can spend the inputs of a lock that may be on the wire. Both gaps exist at the pin today; E0-04 is where they first matter. |
| Q18 | The row-less set is per process, not persisted. Enough? | **Yes** (§5.5). Row-less bytes are handed off only in the process that signed them, and a flow resumed later signs again and carries its own record of the earlier outcome. Persisting the set would add a journal write to every send for no case that needs it. |
| Q19 | Should a leased flow's state transitions use the split flow everywhere, or only where an own key is held? | **Only for own-key registration and top-up** (§4.4), where the ChainLock fallback must be visible. Vault-key leases may use the library's one-call paths. |

## 15. Changes to other documents after the review

- **DASHPAY §2.6:**
  - replace the DRAFT bullet (from "Commit points" to the end of "Open issues") with a summary of §0, §5, §7 and §8
    and a pointer here;
  - correct the "re-dispatch" row of the commit-point table: dw has no launch catch-up until H6 (F6);
  - the spec check becomes `e0_04_design_model.py`;
  - §3.3 gets `IdentityScan` and `authorize_set`;
  - §3.4's store table gets `dispatch.sqlite`.
- **ROADMAP E0-04:**
  - drop "(draft)";
  - "within H" becomes "within `max(H, T_gate)` of the call";
  - replace the acceptance's dispatch-record list with §12's dispatch-record cases 1–7 and "two leases" with the
    bound tests;
  - add the phases of §13.
- **m1-engine §2.1 and §2.2:** as §3.6, plus async `Vault.lock()` and `LockReport`.
- **DECISIONS-PENDING B5:** name §11's PR content.

## Appendix A. Closure of DASHPAY §2.6's open-issues list

### A.1 Majors and minors

- **M-A.**
  - The single J step (§5.4) re-reads the entry and compare-and-sets it. The model's `split-literal` mutation
    reproduces both traces of `e0_04_split_model.py` as violations, and `split-cas` passes.
  - "Never both" is I3.
  - The lock-order concern is closed differently from the review's suggestion (permit first, then guard): there is
    no permit lock to queue behind, because admission is a non-blocking decision and the drain only waits. The
    review's rule survives as L2, so a journal fsync never stalls the wallet's readers, and the fence takes no
    library lock at all (H1), which model Part 3 shows is what keeps `admit` deadlock-free.
  - The test the review asked for (resume classifies `Unsent`, flow dispatches, lock, resume refused, inputs stay
    reserved) is dispatch-record case 4, and in the model it is the M-A replay.
- **M-B.**
  - (a) The journal write is synchronous `FULL` and returns before the transport (I1); the row is durable before
    admit (L3).
  - (b) The record left the changeset (§6.1); the journal is monotone (§6.2–6.3); a resurrected row is refused again
    by its entry.
  - (c) A failed write is `Ambiguous`, never `Unsent` (§6.4).
  - (d) The journal is seeded with `PreFence` for every row that predates it, and `PreFence` means Resend (§6.5).
    No entry means unknown provenance: kept and not sent. So the decision never rests on the call site.
- **M-C.**
  - H starts at the permit's grant and covers the write and the transport (§5.4).
  - The host enforces it (H3).
  - Revocation is in the synchronous freeze (H4).
  - A dropped future and a second call are §8.3.
  - "Reopen admission only for leases created after the freeze" becomes `lock_gen` (H8): a lease whose creation
    straddles a freeze is never inserted.
  - Model Part 2 covers all of this, including the creation race.
- **m-1.** 128-bit random ids, wallet compared too (H5); the `lease-reuse` mutation.
- **m-2.** Part 2 is a per-lease tick simulation with the freeze as an event; four wrong rules fail it.
- **m-3.** The atomicity is stated in §10.1 and made checkable by the `split-*` variants.

### A.2 Nits

1. **The H wording** in ROADMAP and in DASHPAY's tests bullet is aligned to "within `max(H, T_gate)` of the call,
   plus timer slack" (§8.2, §15).
2. **"Finds the lease revoked"** becomes "is refused (its lease was revoked at the lock's call)". With revocation at
   the freeze, there is no separate admission flag left to refuse on.
3. **A test that nothing enters the host's unconfirmed-outgoing set before its permitted hand-off:** L9 and its
   platform-wallet test (§12). In dw that set is always empty (F8); the test still guards other backends.
4. **An unlock plus a grant during the drain:** both succeed; only `begin_lease` waits, at most H (§8.3, Q9).
5. **The r4 n1 stress barrier:** fixed in E0-03 (`a79b3a9`, the abortable rendezvous). E0-04's stress tests reuse it.
