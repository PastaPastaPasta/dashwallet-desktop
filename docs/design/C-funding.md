# C-funding: registration from Platform or Shielded (D-C4), and paying a contact from any balance (D-C5)

Design notes for PLAN-1.0 lane C, items 4 and 5. They settle what the engine slices C4a, C4b, C5a, C5b and the renderer
slices C4c, C5c build (split as the "Slices" section lists). D-C4 is an addendum to
[DP1-02](DP1-02-ownership-model.md) and
[DP1-09](DP1-09-registration-wizard.md). D-C5 is an addendum to [DP3-01](DP3-01-notes.md) and
[DP3-06](DP3-06-pay-to-contact.md). Nothing here changes a decision of those documents for the Dash wallet (Core)
source.

Base: `origin/main` 31c35e77. The library is the pinned fork (`rust/Cargo.toml:93-112`).

Path abbreviations:

- `E/` = `rust/crates/dw-engine/src/`, `P/` = `E/platform/`, `UI/` = `apps/desktop/src/`.
- `W/` = the library's `packages/rs-platform-wallet/src/wallet/`, `PW` = `W/platform_wallet.rs`,
  `OPS` = `W/shielded/operations.rs`, `PV/` = `packages/rs-platform-version/src/version/`.
- m4 = `docs/contracts/m4-dashpay-engine.md`. Each engine slice takes the next free Contract-Version when it merges.

Two rules run through both notes:

1. **A signed plan is the only plan.** Once a signature may have left the process for a payment or a registration, the
   engine only ever re-signs those exact terms, until evidence proves they can never execute. It never builds a second,
   different transition for the same intent while the first is unknown.
2. **Only evidence releases a hold** (DP1-02 I7). A timeout, a failure code or "not found" never does.

Rules shared with [C-routes](C-routes.md) (D-C1, D-C2, D-C3, D-C6), which owns them; this document points there and
does not restate them:

- **Lock order** (C-routes §0): the identity's write lock, then the shielded wallet lock (`P/shielded_pool.rs:401`),
  then the Platform move lock (`P/platform_moves.rs:1562`). Every call here that takes two of them takes them in that
  order: C4b and C5a take the shielded lock, then the move lock where they write a Platform record (F13); C4a and C5b
  take the move lock alone. "Accept and pay" releases the identity's write lock when the accept returns, before the
  payment takes any other.
- **What may run next to a held record, by move kind** (the table of that name in C-routes §0). It covers main's
  kinds and the two this document adds: `Register` (F9, F10) and `ToContact` (P4, P8). Beside a held `Register` or a
  held `ToContact`, a "Move to Dash wallet" at the record's inputs and nonces may run (F10, P8). The exact-terms set
  stays main's: `Transfer` alone.
- **The input-0 fee reserve** (C-routes C1-4, C1-5): a Platform route whose transition takes its fee from input 0
  leaves a fee reserve unclaimed there. F9 applies it to registration. In both documents `debit_credits` is the
  grant's cap (the amount plus the reserve), and the copy shows the amount plus `fee_credits`, as "about".
- **Fresh absence.** A proof that something is absent decides only when its height is at or above the height of the
  proof it is weighed against (the nonce proof in F10, the execution evidence in F12 and F13). An older proof of
  absence decides nothing.

---

## D-C4: funding-source picker, registration from Platform or Shielded

### What exists

- `RegistrationFunding` is `CoreBalance | Invitation | ExistingIdentity | FaucetAssetLock` (`P/registration.rs:53-75`).
- The stored form is `Funding`, a versioned, tagged JSON value (`P/registration_rows.rs:17`, `:97-124`, decode
  `:168-198`).
- The driver picks the funding step by kind at `KeysPrepared` (`P/registration_flow.rs:1864-1870`). An invitation goes
  from `KeysPrepared` to `IdentityRegistered` in one identity-creating call, with no asset lock of the wallet.
- DP1-02's ledger reads every row that is not `CoreBalance` as `Unfunded`: "no user funds; never funds"
  (DP1-02 §3.3 row 2).
- The Join page starts a Core registration only (`UI/screens/dashpay/JoinDashPay.tsx:184`).

### Decisions

| # | Decision | Why |
|---|---|---|
| F1 | Both new sources take the invitation claim's shape: `Draft → KeysPrepared → (one identity-creating call) → IdentityRegistered`, then the name and profile steps unchanged. They never enter `FundingSent` or `ProofWaiting`, and they build no asset lock. | The driver already branches on the funding at `KeysPrepared` (`P/registration_flow.rs:1864-1870`). DP1-02's asset-lock ledger takes no lock for a row that is not `CoreBalance` (§3.3 row 2), so its invariants I3–I6 are untouched. |
| F2 | Two new stored kinds, written at funding version 1: `platform_addresses` and `shielded` (fields in F3 and F9). `FUNDING_VERSION` stays 1. | An older build decodes an unknown kind as `Unsupported` (`P/registration_rows.rs:195`; the test at `:307` pins exactly `platform_addresses`). Such a row fails with `registration.unsupported` and is never funded, discarded or advanced (`P/registration_flow.rs:1130-1157`). C4a moves that test line to another unknown kind. |
| F3 | `platform_addresses` stores `funding_credits` (what the identity receives) and `fee_reserve_credits` (F9) at Draft, both from the accepted quote. The plan step adds `inputs` (address, nonce and claimed credits each, in address order) and `identity` (the id those inputs give). `shielded` stores `denomination` and `fallback` at Draft, and `identity` once it is known (F11). The row's `identity` column is set only when the identity is proved to exist. | The invitation row does the same: the expected id lives in `funding`, and the column is "the only identity the row adopts" (`P/registration_rows.rs:104-113`). The plan travels with the row in a `.dwbackup`, so an imported row still knows what it may have signed. |
| F4 | `RegistrationRequest.funding` gains `PlatformAddresses{funding_credits, fee_reserve_credits}` and `Shielded{denomination}`. The host passes back the numbers its quote showed. A start whose numbers no longer match the engine's own plan is refused with `registration.quote_changed` before anything is written. | `CoreBalance` keeps "the locked amount from the accepted quote" for the same reason (`P/registration_rows.rs:98-101`): the user confirmed one cost, and a protocol-version change between quote and start must not swap it. |
| F5 | One `Registration` lease still covers the whole flow. For a `platform_addresses` row it also carries the Platform-address signer. For a `shielded` row it also carries the shielded spend key. It never carries both, and neither row gets the Core funding signer. The grant is a `PlatformOp` with `max_duffs` 0. For Platform, `max_credits` = `debit_credits` (`funding_credits` + `fee_reserve_credits`) + the name and profile credits. For Shielded, `max_credits` = the denomination + the name and profile credits. | The scopes exist (`rust/crates/dw-vault/src/signer.rs:113`, `:159`) and the permit already hands them out (`P/lease/call.rs:841-850`, `:855`). The proof-of-possession signatures come from the same identity signer as today (`P/registration_flow.rs:3618`). A lease holds only the keys its row can use. The grant consents to the planned debit; the metered fee has no ceiling (C-routes C1-5). |
| F6 | `funds_committed` for a new-source row is true from the moment a signature for its identity call may have been released, until evidence proves that call can never execute. `discard_registration` refuses while it is true. | m4 §2.2's discard rule, and the invitation's `claim_discard_refused` (`P/registration_flow.rs:1180-1203`): a discard must not leave an identity on Platform that no step finishes. |
| F7 | An unknown outcome is a wait, not a failure. New `RegistrationWait::PaymentUnknown`: the row keeps its phase, the next proved read settles it, and no user action is needed or offered except "Check again". One exception: a Platform plan whose record is `failed` by height is `RegistrationWait::PaymentHeld`, which offers "Move to Dash wallet" (F10, F22). | `platform.broadcast_unknown` as a failure reads "Registration stopped → Open Repair" (DP1-09 R8), and Repair's "Finish transfers" handles asset locks only. These sources have no lock to finish; reads alone decide. |

**Platform addresses (C4a)**

| # | Decision | Why |
|---|---|---|
| F8 | **One create route.** The registration signs at nonces the engine fixed and recorded first, through the SDK's `PutIdentity::put_with_address_funding(&sdk, inputs_with_nonce, None, identity_signer, address_signer, settings)` (`rs-sdk/src/platform/transition/put_identity.rs:115-123`, body `:399-470`), reached through the `sdk` handle the flow already holds (`P/registration_flow.rs:3155-3159`). It never uses the library's `register_from_addresses`, which fetches the nonces itself at submit (`W/identity/network/register_from_addresses.rs:101-110`), nor rs-sdk's `fetch_inputs_with_nonce`. After a proved result the engine calls the library's two follow-ups: `reconcile_address_infos` through `wallet.platform()` (`W/platform_addresses/wallet.rs:292`), with no wallet-state guard held (its lock order is provider, then wallet manager, `:262-270`); then `add_identity` through `wallet.state_mut()` and `wallet.persister()` (`W/identity/state/manager/lifecycle.rs:36`), as `P/identity_reads.rs:289-293` does. `IdentityAlreadyExists` from `add_identity` (`lifecycle.rs:45-47`) is success: discovery added it first. Neither follow-up decides anything; a failure of either is logged and the next sync heals it. No fork unit is needed. | The identity id is a hash of the input addresses and their nonces (`rs-dpp/src/state_transition/traits/state_transition_identity_id_from_inputs.rs:26-44`). With fetched nonces the id is unknown before signing, and two attempts could sign at different nonces and create two identities from two debits. With fixed nonces Drive executes at most one transition per input nonce (`P/platform_moves.rs:54-60`). The SDK form returns the proved identity and the address infos in one call. |
| F9 | Order of the funding step, under the Platform move lock: one proved read under rule 5 (`P/platform_moves.rs:8-13`, `:2456`); refuse with `credits.move_pending` if any move is pending or holding; plan; write the plan into the row (F3), durably; write a move record of the new kind `Register` naming the draft, `pending` and able to land, and read it back; only then sign and send. **The plan** follows C-routes C1-4: the candidates are the funded addresses in `PlatformAddress` order; input 0 is the first address in that order whose balance is above the reserve, as C1-4's planner takes it, and it keeps `fee_reserve_credits` unclaimed on top of its claim; the claims sum to exactly `funding_credits`, with no change output. **The reserve** is 3 × `calculate_min_required_fee` for the plan's inputs and the keys being created (`rs-dpp/.../identity_create_from_addresses_transition/state_transition_estimated_fee_validation.rs:15-40`), the band C-routes C3-3 uses for the top-up. There is no separate registration cost. Input 0's claim is at least `min_input_amount` and the claims reach `min_identity_funding_amount`; a plan that cannot meet those, or has no address above the reserve, is refused before anything is written or signed (`platform.insufficient_credits`). The quote gives `fee_credits` (one times the minimum fee, shown as "about") and `debit_credits` = `funding_credits` + `fee_reserve_credits`, which is the grant's cap and not what the copy shows. Registration has no Max. | The SDK's create fixes the fee on input 0 (`put_identity.rs:414-415`). Drive gives the identity exactly the sum of the claims and takes the metered fee from input 0's remaining balance, or refuses with a state error. That refusal is not `never_valid` (`P/platform_moves.rs:1403-1413`): nothing is paid, no nonce is used, and the signed create stays valid. So a plan whose input 0 fell short would hold; the band makes that unlikely and F10 gives the way out. The library's planner picks input 0 the same way (`PW:186-196`), so a first address that holds only dust does not refuse every registration. The move planner's largest-first fee source (`:1215`) cannot be used for the same reason. The order is the one every Platform-address move follows (`:54-57`, `:2441-2500`); recording the debit as a move gives the existing hold, the nonce settlement, and the rule that a move's own spend is never an `outgoing` activity row (`:21-24`). |
| F10 | Settlement. **Done:** the SDK's proved result for this transition, which binds execution and checks the identity id against the inputs (`put_identity.rs:447-463`); or, after a restart or an unknown answer, a proved read that shows every input at or past its signed nonce together with a proved fetch that finds the planned identity. The row adopts that id and moves to `IdentityRegistered`. **Never signed or never in a block:** in the process that made the call and never after a restart, the call returned having released no signature; or Platform's structure or signature check refused the transition before any block and a proved read shows every input's nonce unused. The move is `failed` and unable to land, the plan is cleared from the row, and the row may plan again. **Nonces taken, no identity:** two proofs decide, each read by id at a height at or above the height of the nonce proof (an older proof of absence decides nothing: the fetch may have reached a node behind). They are the planned identity, and the withdrawal document of each signed exit (below). The exit's document present: the exit executed, the create can never execute, the plan is cleared, and the row goes back to Confirm (F22). The identity absent and no exit document present, or no exit signed: the create executed as a paid failure, which took the nonces and a penalty and made no identity. That includes a key structure or proof-of-possession error reported after the nonces were taken. The row then follows F12's order: it adopts an identity of this seed found at the row's index after a discovery pass; otherwise it fails with `registration.refunded`, not retryable, its plan is cleared, and nothing retries or plans again automatically. **Anything else:** `PaymentUnknown` (F7), and the row keeps its plan. A plan whose move is `failed` by height may still execute, so the row re-signs only that plan and never plans a second registration while it holds. **The way out:** beside a held `Register`, a "Move to Dash wallet" that signs exactly the record's inputs at its nonces may run (the C-routes §0 table, kind `Register`; as C3-9 for the top-up). Whichever of the two executes takes the nonce. The planned identity present says the registration is done and the exit can never execute. The exit's withdrawal document present says the exit executed; its id is computed from input 0's nonce and address and the exit record's stored Core script (`rs-drive/src/state_transition_action/address_funds/address_credit_withdrawal/v0/transformer.rs:66-82`). Taken nonces alone never say the exit ran. The two records settle by this evidence, not by main's rule for groups with other terms (`P/platform_moves.rs:83-91`): the member the evidence names is `done` and the other is `failed`, never `replaced`; after a paid failure both are `failed`. `Register` is not in the exact-terms set, which stays `Transfer` alone; P8 gives `ToContact` the same way out. | Rule 1. Re-signing the same inputs, nonces, claims and keys gives the same identity id, so a retry is idempotent. A `pending` record read back after a restart is always treated as able to land (`P/platform_moves.rs:76-82`, `:2540-2545`): the engine cannot know whether a signature left. If the metered fee is more than input 0 kept, Drive refuses the create unpaid and it stays valid, so without the way out the wallet's Platform balance would stay held until a deposit reached that one address. The move is not a second registration, so rule 1 holds. Drive turns a create that fails inside a block (unique key hashes already present, key bounds or limits, key structure, proof of possession) into an action that bumps every input nonce and takes a penalty of 10,000,000 or 50,000,000 credits (`rs-drive-abci/.../identity_create_from_addresses/mod.rs:39-58`, `.../state/v1/mod.rs:60-106`; `drive_abci_validation_versions/v10.rs:456-458`). Both the fee and the penalty come from the inputs (`rs-drive/.../bump_address_input_nonces_action/v0/transformer.rs:14-21`), so the refund copy names the penalty. Each cause is deterministic for the row's keys, so a new plan would pay the penalty again every time; that is why "taken and absent" never plans again. Main's group rule would mark the newer record `done` on the nonces alone, and history would show "Moved to Dash wallet" for a registration's debit. A row imported without its move record follows the same cases: it can re-sign its plan safely whether or not another install already signed it. |

**Shielded (C4b)**

| # | Decision | Why |
|---|---|---|
| F11 | The identity call is the library's pool-funded create through the dispatch hook, with the keyset entry point UP-1 adds (never the seed path, `PW:1690`). The engine records the hooked `IdentityCreate` artifact (`rust/crates/dw-appdb/src/dispatch.rs:539-545`) and writes the created id into the row when the call returns it. The id is `double_sha256` of the sorted nullifiers (`OPS:2188-2189`), so it exists only once notes are selected. If the hook's registration does not carry it, UP-1 adds it. | The engine holds a key set, never the seed (lane-C §0 finding 1). The id cannot be planned ahead as F9 does, so the artifact is the record of what may have been sent. |
| F12 | Evidence, in this order. The journal's disposition of the artifact (item 0: executed, definitely unsent, unresolved). Then the identity: by the row's stored id, else by the seed's identity at the row's index after a discovery pass (`P/registration_flow.rs:1921-1936`). **Executed, and the identity exists at the stored id:** adopt it. Every "no identity" below means a proof of absence at a height at or above the height of the execution evidence; an older one decides nothing and the row stays `PaymentUnknown`. **Executed, no identity at the stored id, an identity of this seed at the row's index:** the duplicate-keys refund; adopt that identity (F13). **Executed, and no identity of this seed at the stored id or at the index after a discovery pass:** the create took Platform's fallback for another reason (key bounds or key limits). The row fails with `registration.refunded`, not retryable; its plan is cleared; nothing retries automatically (F13 for where the funds are). **Definitely unsent** (A5 reads it so, with the stored transition abandoned and never redriven): the row returns to `KeysPrepared` and may select notes again. A refusal for the pool's note floor is not that: it stays unresolved until A5 says so (F16). **Unresolved:** `PaymentUnknown`; no second create runs while a spend of the wallet is unrecorded or unresolved. **Two or more notes:** the bundle has no padding, so its nullifiers read spent may be another install's spend of the same notes, and they never claim a refund. The identity present adopts it; absent at or above the spent read, the notes are gone whoever spent them: the engine proves it `consumed`, the library's hold on them is acknowledged once they read spent, and the row adopts an identity of this seed at the row's index, else fails `registration.payment_consumed`, not retryable. | A second create selects other notes, so it would have another id. Platform's fallback fires for duplicate key hashes and also for key contract-bounds and key-limit failures (`rs-drive-abci/.../identity_create_from_shielded_pool/state/v1/mod.rs:66-72`, `:84-101`, `:103-150`); each is deterministic, so a blind retry would lose a penalty every time. A duplicate can never make a second identity. The existing gate `shielded.unrecorded_spend` (`P/shielded_pool.rs:563`, `:1153`) already blocks the next spend. |
| F13 | The required fallback address is always the wallet's own Platform receive address (`P/platform_addresses.rs:429`), fixed at Draft and stored in the row. It is never typed, pasted or external. It is published beside the new identity, so it is never an address handed out before or after: the lowest unused address (which Receive may be showing) and the next are both marked used and the next is the fallback, recorded durably with its index before the row names it. Every receive address handed out raises a durable record of the highest index handed out before it is returned, and every hand-out and every fallback first marks used each index below that record and each recorded fallback. So Receive never offers a fallback, and no fallback is an address handed out before it, across restarts and after its row is discarded. A start that is refused takes no fallback. A dedicated fallback derivation is a 1.0.x follow-up (#894). When Platform takes the fallback, the denomination less a penalty lands there (`state/v1/mod.rs:72`, `:137`). The rise is attributed with C-routes C1-9's record and kind: a `FromShieldedRecord` whose source is the registration's draft, shown as `PlatformActivityKind::FromShielded` with the label "Returned from a registration". It is written and armed only once the refund is proved: the disposition reads executed and the identity at the stored id is proved absent at a height at or above the execution height (F12). The record absorbs only a rise in a read whose stored baseline is below the execution height, up to the denomination less the penalty; if the baseline is already at or above it, the record settles `done` and absorbs nothing. It is never armed while the create is pending, and never in the success case. It is written under the shielded wallet lock, then the move lock (lock order above). Two armed records on one address (a C1b unshield defaults to the same address) absorb in the order they were armed, each up to its own credits, and no credit is counted for both. | iOS passes a required Platform fallback (`ios.md:673`). On desktop the only safe meaning is "back to me". Arming while the create is pending would absorb unrelated deposits to the wallet's default address and, on success, keep them, hiding an `incoming` row. Arming after the proof costs at most a wrong label: a read that runs between the execution and the arming has the rise in its baseline and shows it as a plain `incoming` row. The baseline rule then stops the record from absorbing the next unrelated deposit in its place. No money depends on the label. C-routes C1-9 carries the same sentence for this source. |
| F14 | The denomination comes from the protocol version in force, never from a constant: `shielded_identity_create_denominations`. At PV12 the set is 0.1, 0.3, 0.5, 1 DASH (`PV/drive_abci_versions/drive_abci_validation_versions/v8.rs:434`, used by `PV/v12.rs`). From PV13 it is 0.03, 0.1, 0.25, 0.5, 1 DASH (`v9.rs:431`, `v10.rs:506`). The engine picks the smallest member that, less the predicted create fee, covers what the name and profile need at that version. An empty set is `platform.feature_off{feature: "shielded_registration"}`. | The library refuses a non-member before proving (`W/shielded/note_selection.rs:230-242`) and refuses a denomination that does not exceed the fee (`:252-259`). Core's amounts are the yardstick: 0.03 DASH for a plain name (`P/registration_flow.rs:79`) and the version's contested amount (`:92-98`). This also follows iOS #1267 (0.03 DASH for a standard name) without hard-coding it. |
| F15 | The change goes back to the pool, to the account's default shielded address (`OPS:2179-2180`, `:2256`). It is not spendable until a sync pass finds it. The quote says so, and the engine counts it as the wallet's the whole time. | The library sets the change address itself; the engine has no choice to make. The copy prevents "my shielded balance dropped by more than the fee". |
| F16 | Readiness is an engine quote, checked again at the start. Items: the pool is active at this version (PV12); the feature is on; a denomination exists (F14); the spendable balance covers it; no unrecorded or unresolved spend; the shielded tree is synced; the notes have rested (F17). The pool's 250-note floor (`minimum_pool_notes_for_outgoing`, `v10.rs:470`) cannot be read ahead, so it shows up as the refusal `shielded.pool_too_small{notes, required}` at the call (`P/shielded_pool.rs:853-854`). That refusal is not final: the signed create stays valid while its anchor is, and the pool only grows. The row waits in `PaymentUnknown` until A5 reads the artifact definitely unsent with the stored transition abandoned (F12). | iOS's checklist (`ios.md:37`, `:673`). Every item but the rest is already a refusal of a shielded spend on main; the quote only reports them before Confirm. The floor is a state check on a count that only rises (`rs-drive-abci/.../shielded_common/mod.rs:320-349`), and the engine keeps the signed bytes for redrive (C-routes R6). |
| F17 | Note rest: the source is not ready until 3 hours after the newest time of the wallet's unspent own notes and incoming notes (ruled M2; clock ruled M2a, notes M2b). The library stamps no time on any row a scan derives, which covers every payment from another wallet and every shield from another device (platform-wallet 72d0a3e7 `activity.rs:455-473`, run every pass, `coordinator.rs:1809-1816`). Nor does it always file such a note as incoming: a shield from another device of a wallet that has spent is a `SelfTransfer`, and a payment in the same batch as an own operation gets no row of its own (`activity.rs` at 08d1eb70, `:528-541`, `:575-603`, `:666-689`). So the engine keeps its own clock in the wallet's local scope of `app.sqlite`, per note commitment, recorded at each completed sync pass and each quote for every unspent own note, whatever its row, and every incoming row's note. Change from the user's own spends rests too. A note's time is when the engine first saw it, or a row's non-zero `created_at` when that is earlier; a recorded time never moves, and once the first scan is marked no note is recorded as restored. The notes first seen in the wallet's first completed shielded scan after it was created, restored or imported into this install are restored and count as rested once the sync is complete. A note first seen before the first completed scan is recorded (a failed mark write, an exit between the sync and the recording, a first scan that ends `Err`, or a quote before the mark) also counts as restored. A clock that cannot be read, or a first scan not yet complete, is not rested, with no `ready_at`. The engine enforces it at the start, with no override. | iOS's rule. It is a privacy rule, not a consensus rule: spending a note minutes after shielding it links the two. Using the latest incoming activity is stricter than checking only the selected notes, and needs no library change. The clock's scope is cleared with the wallet, so a re-import starts with a new first scan: deleting and re-importing a wallet counts its notes as restored again, which M2a allows. |
| F18 | Shielded is never the default (DASHPAY F26, `docs/design/DASHPAY.md:1129`). The picker opens on Dash wallet every time, does not remember the last source, and the Join page's main button stays the Core start. The engine has no default funding. | The decision is already made; this row says where it is enforced. |

**Picker (C4c)**

| # | Decision | Why |
|---|---|---|
| F19 | The Join page keeps "Create a username" as the one main action (Core). A secondary link, "Pay another way", opens the "Pay with" step: Dash wallet (selected), Platform balance, Shielded balance. Each row shows its balance and, when it cannot pay, the one reason. Ruled (M1). | DP1-09 D1's two-step wizard stays the common path. iOS hides Platform funding behind Advanced mode (`ios.md` PADDR-14); desktop has no such mode, so the link is the equivalent. |
| F20 | The confirm step shows the source, what leaves it, the fee, and what the identity starts with. For Shielded it also shows the denomination and the change line (F15). | DP1-09's confirm shows the lock and fee for Core; the same facts, per source. |
| F21 | Resume follows DP1-09's rule "the engine's rows are the truth". `PaymentUnknown` maps to the progress screen with "Check again". A contested name is allowed from both sources, at the larger amount the quote gives (F14, F9). | DP1-09 R1–R8 and D4. |
| F22 | A Platform registration whose record is `failed` by height shows its own state (`RegistrationWait::PaymentHeld`), with the copy below and the action "Move to Dash wallet", which starts that move at the record's inputs and nonces (F10). After the exit is proved executed the row returns to the Confirm step with a new quote and plans only when the user confirms again. Resume never plans by itself. | Without the state the progress screen says to wait while the only way out is on another screen. As the top-up's held state in C-routes ("Platform, held after it was not taken"). The exit frees the rest of the Platform balance; spending it at once on a new plan needs the user's word. |

### Kill and resume

The DP1-02 bar: a kill at any transition loses nothing, and a relaunch never pays twice.

| Kill point | Platform addresses | Shielded |
|---|---|---|
| Before the plan or artifact is written | Nothing happened. The row is `KeysPrepared`; a resume plans afresh. | Same. |
| After the record, when the process cannot say whether a signature left | The same as the next row. The record was stored `pending` and able to land before signing (`P/platform_moves.rs:2540-2545`), and a restart reads every such record that way (`:75-81`). The row keeps its plan and re-signs only that plan; F10 settles it. It never plans again here. | The artifact is registered. Item 0's load sort decides: an artifact with no send is abandoned (A4b, STOP-8) and the row returns to `KeysPrepared` only once A5 reads it definitely unsent. Until then, `PaymentUnknown`. |
| After a signature, outcome unknown | `PaymentUnknown`. The next proved read decides (F10). The row re-signs only its stored plan. | `PaymentUnknown`. The journal's disposition and the nullifier scan decide (F12). No second create runs meanwhile. |
| After Platform executed, before the row was written | The read shows the nonces taken and the planned identity present: the row adopts it. | The disposition is executed and the seed's identity is at the row's index: the row adopts it. |
| After `IdentityRegistered` | DP1-02 unchanged. | DP1-02 unchanged. |

A restore from the seed alone has no row. DashPay's bring-up discovers the identity at its index as it does today, and
the user finishes with "Choose a username" (DP1-09 D6).

### Copy

| Where | Text |
|---|---|
| Join page link | "Pay another way" |
| Picker title | "Pay with" |
| Rows | "Dash wallet", "Platform balance", "Shielded balance" |
| Row cannot pay: balance | "Not enough here. You need at least N Dash." |
| Row cannot pay: a move is pending | "Another Platform transfer is still being confirmed. Try again when it is done." |
| Shielded: not offered on this network | "Not available on this network yet." |
| Shielded: notes resting | "Your shielded funds arrived less than 3 hours ago. For your privacy, you can use them from HH:MM." |
| Shielded: an earlier spend is unknown | "An earlier shielded payment is still being checked. Try again when it is settled." |
| Shielded: still syncing | "Your shielded balance is still syncing." |
| Shielded: pool too small | "The shielded pool is too small for this right now. Dash Wallet is checking that nothing was sent; do not start again until it says so." Then, once it is definitely unsent: "Nothing was paid. Try again later, or pay with your Dash wallet." |
| Confirm, Platform | "About N Dash leaves your Platform balance: M for your identity and a network fee of about F. Your identity starts with M." (M is `funding_credits`, exact; F is `fee_credits`; N is M plus F. `debit_credits` is the grant's cap and is not shown.) |
| Confirm, Shielded | "N Dash leaves your Shielded balance. The rest of the notes used returns to your Shielded balance after the next sync." |
| `registration.quote_changed` | "The cost changed. Review it and confirm again. Nothing was paid." |
| `PaymentUnknown` | "We could not confirm the payment yet. Dash Wallet keeps checking; do not start again. Nothing more is needed from you." Button: "Check again". |
| `PaymentHeld` (F22) | "This payment hasn't gone through yet and still could. To free your Platform balance, move these credits to your Dash wallet; only one of the two can happen." Action: "Move to Dash wallet". |
| After the exit executed (F22) | "The credits moved to your Dash wallet and no identity was paid for. Review the cost and confirm to try again." |
| `registration.refunded`, Platform source (F10) | "The network could not create the identity with this wallet's keys and kept N from your Platform balance for the attempt: a network fee and a penalty for the failed request. The rest of your Platform balance was not used." (N is the inputs' proved fall in balance at the nonce proof against the balances the plan signed against: the processing fee plus Drive's 10,000,000- or 50,000,000-credit penalty. If any input's balance cannot be compared, for example because a deposit arrived, the copy drops the amount: "…and kept a network fee and a penalty from your Platform balance for the attempt.") |
| Refund, an identity already existed (F13) | "This wallet already had an identity, so the payment came back to your Platform balance, less a network fee of N." |
| `registration.refunded` (F12) | "The network could not create the identity with this wallet's keys. The payment came back to your Platform balance, less a network fee of N. Nothing else was paid." |
| `registration.payment_consumed` (F12) | "Dash Wallet couldn't register the username. The Shielded funds chosen to pay for it have been spent, but no username was registered. Either this payment spent them, and Dash Platform kept a penalty and sent the rest to your Platform balance, or another device using this recovery phrase spent them. Check your Platform balance before you start a new registration." |

### Contract rows (m4)

| Slice | Section | Rows |
|---|---|---|
| C4a-1, C4a-2 | §2.2, §2.12, §3, §4 | `RegistrationFunding::PlatformAddresses{funding_credits, fee_reserve_credits}`; `RegistrationQuote` gains `funding_credits`, `fee_reserve_credits`, `fee_credits` (the estimate the copy shows), `debit_credits` (funding plus reserve: the grant's cap, never shown as what leaves), `available_credits` (0 for other sources; the duff fields are 0 for this one); `RegistrationWait::PaymentUnknown`; `RegistrationWait::PaymentHeld{move_id}` (F22); `registration.refunded` for a paid failure (F10); `registration.quote_changed`; the start's refusals `credits.move_pending` and `platform.insufficient_credits`; the input-0 reserve (F9); the rule "a planned row re-signs only its plan" (F10). §2.12: the move kind `Register`, its activity row, and its line in the held-record rule (a "Move to Dash wallet" at its inputs and nonces may run beside it; the pair settles by F10's evidence). |
| C4b-1, C4b-2 | §2.2, §2.12, §2.13, §3, §4 | `RegistrationFunding::Shielded{denomination}`; `RegistrationQuote.readiness` (each item of F16 with its state, and `ready_at` for the rest); `registration.shielded_not_ready{reason, ready_at}`; `registration.refunded`; `platform.feature_off{feature: "shielded_registration"}`; the fallback rule and the refund record's arming (F13); the evidence order (F12); the pool-floor rule (F16). |
| C4c | none | Renderer only: the picker and the `PaymentHeld` state (F22). |

A new error code may touch `rust/crates/dw-ffi/src/api/error.rs`; if its mapping is exhaustive, the slice runs
`scripts/build-core.sh` and commits the regenerated Swift (lane-C §0 finding 7).

### Tests that prove the money rules

C4a-1 (quote, plan, the `Register` kind):

- `fee_reserve_on_input_0`: after the plan, input 0 keeps `fee_reserve_credits` above its claim and the claims sum to
  `funding_credits`; with no address above the reserve the quote refuses before signing. Mutant: the move planner's
  largest-first fee source.
- `fee_reserve_is_three_times_min_fee`: the reserve is 3 × `calculate_min_required_fee` for the plan's inputs and keys.
- Input 0 is the first address above the reserve: a first address holding dust is skipped, not a refusal.
- Input 0's claim below `min_input_amount`, or claims below `min_identity_funding_amount`, refuse in the quote.
- `debit_credits` is funding plus reserve and equals the grant's cap; `fee_credits` is what the copy shows.
- The grant covers `debit_credits` plus the name and profile credits, and no less.
- Settlement needs the SDK's proved result, or both the nonces and the identity from proved reads. Mutants: settle on
  the nonces alone; settle on an answer that carries no proof.
- `absence_older_than_the_nonce_proof_decides_nothing`: nonces taken at height h and the identity absent at h−1 → no
  re-plan, and the row stays `PaymentUnknown`. Mutant: drop the height check.
- `register_fee_shortfall_holds_and_to_core_exits`: the create is refused for input 0's shortfall and the record holds;
  the wait is `PaymentHeld`; a "Move to Dash wallet" at its inputs and nonces runs and executes; fresh proofs show
  the exit's withdrawal document present and the identity absent; the exit is `done`, the `Register` record `failed`,
  and the row waits at Confirm: no plan is made until the user confirms again. A move at other inputs or nonces is
  still `credits.move_pending`. Mutants: put `Register` in the exact-terms set; plan on resume.
- The opposite branch: the exit is signed, then the create executes. The row adopts the identity, the `Register`
  record is `done`, the exit's record is `failed`, and no "Moved to Dash wallet" row appears. Mutant: main's group
  rule (the newest is `done`).
- `register_paid_failure_does_not_replan`: nonces taken, no identity, no exit document (with an exit signed, and with
  none) → no new plan, no second penalty, the row fails `registration.refunded` and is not retryable. Mutant: plan
  again on "nonce taken and absent". With an exit signed, the `Register` and the exit's records both end `failed`, and
  no "Moved to Dash wallet" row appears. Mutant: main's group rule (the newest is `done`).
- `register_duplicate_keys_adopts_index_identity`: the same, with an identity of this seed at the row's index → the
  row adopts it.
- A structure refusal with the nonces proved unused may plan again; the same error after the nonces were taken is the
  paid failure.
- A pending registration refuses the next Platform move with `credits.move_pending`, and a pending move refuses the
  registration.
- No `outgoing` activity row appears for the registration's spend.
- An older build's decode of both new kinds is `Unsupported`; a newer row is never funded or discarded there.
- `lock_order_register`: the call runs against a holder of the move lock and does not deadlock.

C4a-2 (flow, resume):

- A kill at each row of the table above loses nothing, and the relaunch pays once (mock SDK, as DP1-02 §11.2).
- The transition is signed at the recorded nonces. Mutant: fetch nonces at submit; the test that moves a nonce between
  plan and call must fail.
- A row with a plan re-signs the same terms and reaches the same identity id. Mutant: re-plan while the first is
  unknown.
- A kill after the record with no signature known keeps the plan: after a deposit changes the balances, the relaunch
  signs the same inputs and nonces. Mutant: clear an unsigned `pending` record at load.
- `IdentityAlreadyExists` from `add_identity` is success, and a failed `reconcile_address_infos` changes no decision.
- A refusal before signing keeps the grant and leaves no move record.
- Discard is refused while `funds_committed`.

C4b-1 (readiness, denomination):

- Every readiness item refuses at the start as well as in the quote. Mutant: check the rest only in the quote.
- The denomination follows the version: PV12, PV13 and PV14 give the members of F14; an empty set is `feature_off`.
- The fallback is the wallet's own receive address. Mutant: accept a caller's address; the test must fail to compile or
  fail.

C4b-2 (flow, evidence, refund):

- The kill matrix, including both item-0 windows (before and after the artifact write).
- The id written to the row equals `double_sha256` of the sorted nullifiers of the built bundle.
- Mutant: the seed entry point. A test that the hook saw the spend must fail.
- No second create while a spend is unresolved or unrecorded.
- A duplicate-keys refund adopts the existing identity and shows one attributed activity row.
- Executed with no identity at the stored id or the index fails the row with `registration.refunded` and starts no
  second create. Mutant: treat it as unsent.
- Executed, with the identity absent only in a proof older than the execution evidence: the row stays
  `PaymentUnknown`, does not fail `registration.refunded`, and no refund record is armed. Mutant: drop the height check.
- The refund record is armed only after the proof. A deposit to the fallback address while the create is pending, and
  one after a successful create, are both plain `incoming` rows. Mutant: arm at the hook.
- An address read between the execution and the arming, then an unrelated deposit: that deposit is an `incoming` row
  and the record settles `done` having absorbed nothing. Mutant: absorb whatever the baseline.
- Two armed records on one address absorb in arming order and never count one credit twice.
- A pool-floor refusal keeps the row in `PaymentUnknown` until A5 reads definitely unsent. Mutant: return to
  `KeysPrepared` on the refusal.
- `lock_order_shielded_register`: shielded lock, then move lock, against the existing holders.

C4c: vitest for the picker model (default, no memory of the last source, each disabled reason) and for the
`PaymentHeld` state (its copy and action; Confirm after the exit, with no plan made); Playwright on Chromium
and WebKit; DEC-1484 images, light and dark.

### Out of scope

- Top-up from Platform or Shielded (D-C3).
- A user-chosen denomination, and more than one identity per wallet from the picker.
- A registration funded partly from two sources.
- Repair UI for these sources beyond "Check again": the acknowledgement of an unresolved shielded spend is A4c's.
- Advanced mode.
- Reading the pool's note count ahead of the call.

---

## D-C5: pay a contact from any balance

### What exists

- A contact payment is a Core `TxDraft` (`E/send/contact.rs:1-10`). `prepare_contact` runs, in order: the live payee
  check, the payment-lock check, rule 5's gate on the payee, the coin fence, the lease, the charge, and only then the
  address reservation (`:117-209`).
- The reserved address is kept per pair for the session and reused until a broadcast claims it (`:222-268`;
  `P/payment_locks.rs:38-43`, `:227-263`).
- The lock row is written durably before the first hand-off (`P/payment_locks.rs:269-311`, DP3-01 D23 and D27). It is
  keyed by the Core txid and settled from Core evidence (`:467-501`).
- The library's reservation is one pool "shared by Core sends and externally funded contact payouts"
  (`W/identity/network/payments.rs:1032-1033`). It marks the address used and flushes before it returns
  (`:1096-1105`, `:1177-1184`).
- Shielded → Core already accepts any Core address (`P/shielded.rs:569-583`, `P/shielded_pool.rs:1141-1158`).
- Platform → Core pays only the wallet's own receiving addresses (`P/platform_moves.rs:2699-2722`).
- History names a contact by the output address, in a transaction the wallet funded (`P/attribution.rs:5-11`,
  `:206-216`). A payout from Platform arrives in an asset-unlock transaction the wallet did not fund.

### Decisions

| # | Decision | Why |
|---|---|---|
| P1 | Two new engine calls, not a `TxDraft`: `contact_pay_quote(identity, contact, credits, source)` and `contact_pay(identity, contact, credits, source, note, grant, confirm_unrecorded)`, with `source` = `shielded` or `platform`. The Core source stays the draft, unchanged. | A draft is a Core transaction with coins, change and a fee rate. These sources have none of those; they are one Platform state transition each. |
| P2 | Order of `contact_pay`, the same as `prepare_contact`: live payee check; the pair's lock; rule 5 on the payee; the source's own gates and quote; the grant's cap; the lease; reserve the address and re-check rule 5 on it; claim the pair; assign the payout's reference; write the payout row durably; write the source record; sign and send; settle. Every refusal before the reservation takes no address, and every refusal before the lease keeps the grant. | `E/send/contact.rs:117-209` and `:222-268`. The trust gate and its re-check apply to the payee, whatever pays (`:133-137`, `:249-256`). The reference comes first so the row can name its source record before that record exists (P6). |
| P3 | The source's gates are the ones on main, none relaxed. Shielded: the hook check, the wallet's shielded lock, `confirm_unrecorded`, the quote under the pass hold, the withdrawal range (`P/shielded_pool.rs:781-796`, `:924-963`, `:1150-1154`). Platform: the wallet lock, the proved read under rule 5, `credits.move_pending`, the withdrawal limits (`P/platform_moves.rs:2441-2500`). | A contact payment is an unshield or a withdrawal with a different destination. It must pass everything those pass. |
| P4 | The address comes only from the reservation made inside the same call. No facade call takes a Core address for a contact payout, and `move_to_core` keeps refusing an address that is not the wallet's own. The Platform move gets a new internal target and move kind, `ToContact`, beside the own-address target (`P/platform_moves.rs:2020-2025`), built only by `contact_pay`. | C5b needs an external Core script; lane-C calls it "a new `to` kind". Keeping it internal means the own-address rule of "Move to Dash wallet" is not loosened for any host input. |
| P5 | The pair's reserved address is shared with the Core path: a draft's unused address is the payout's, and the reverse. The payout claims it before it signs. It goes back to the pair in two cases only: the call was refused before a signature, or Platform's structure or signature check refused the transition. Otherwise it is consumed, also when the payment is later proved not sent, and also when a "Move to Dash wallet" executed in a held payout's place (P8): a contact address is never reused after a signed payout. A refusal for the shielded pool's note floor is not a give-back: the lock stays and the address stays consumed until A5 reads the payout definitely unsent with the stored transition abandoned and never redriven; then the lock clears and the address is still consumed. | DP3-01 D7 and D21. An address that may have reached the network is never paid twice (DIP-15 rotation). The floor is a state check on a note count that only rises (`rs-drive-abci/.../shielded_common/mod.rs:320-349`), so the signed unshield stays valid while its anchor is, and the engine keeps its bytes for redrive (C-routes R6). Handing the address back would let a Core payment and a later redrive both pay it. |
| P6 | One lock per pair, across all three sources. The payout's reference is assigned first: for Platform it is the move's id, chosen before the move record is written; for Shielded it is an id the engine makes, and the hook's accept writes the spend's `st_hash` into the row, durably, before it writes the dispatch artifact; if that write fails the accept refuses the spend and nothing is sent. The row goes into a new table `dp_contact_payout` (wallet, identity, contact, source, reference, `st_hash`, address, credits, expected duffs, note, state, since), synced as DP3-01 D27 requires, before the source record and before anything is signed. While its state is `pending` it is the pair's lock in memory, so the existing check (`P/payment_locks.rs:202-221`) refuses every other payment to that contact, from any source. A Core lock likewise refuses a payout. **On restart**, a `pending` row whose source record is absent was never sent: it is cleared as not sent and its address goes back to the pair. Absent means, for Platform, no move record with its id; for Shielded, no `st_hash`, or an `st_hash` with no artifact in the journal. A row whose `st_hash` has an artifact is settled by A4b and A5, never cleared by this rule. | DP3-01 D23: the row, not memory, is the record of every payment that may have gone out. The existing row is keyed by a Core txid and a payout has none before it executes. Both sources write their own record before signing (C-routes R1, R6), so a row with no source record cannot have released a signature. Writing `st_hash` first closes the window in which the journal holds an artifact a pass may re-send while the row does not name it; without this rule a kill between the two writes would lock the contact for good. |
| P7 | What clears the lock. **Sent:** Platform executed the transition. Shielded: the spend is `Done`, or item 0's disposition is executed. Platform: the move is `done` as itself; where a "Move to Dash wallet" was also signed at its nonces, P8's evidence says which of the two executed. **Not sent:** only proof that it can never execute: a structure or signature refusal, A5's "definitely unsent" with the stored transition abandoned, a Platform move unable to land (P8's exit having executed in its place included), or P6's restart case. **Anything else: the lock stays**, a pool-floor refusal included (P5). The Core arrival of the payout is not waited for. | The lock exists to stop a second payment while the first is unknown (DP3-01 D8, D9). Once Platform has executed the withdrawal the outcome is known: the payout is queued by consensus and is only late. Waiting for the asset unlock would hold the contact for a long time with nothing left to learn. |
| P8 | A Platform payout that is `failed` by height still holds its nonces and may still execute. The contact's lock stays while it holds. Beside it, two moves may run: its own retry on the same terms (the same inputs, nonces, amount and address), and a "Move to Dash wallet" that signs exactly the record's inputs at its nonces, as beside a held `Register` (F10; the C-routes §0 table, kind `ToContact`). Any other move is `credits.move_pending`, and no second payment to the contact starts. Whichever executes takes the input nonce. Which one did is decided by each withdrawal's own document, fetched by id at a height at or above the nonce proof. The id is computed from the first input's nonce and address and the output script (`rs-drive/src/state_transition_action/address_funds/address_credit_withdrawal/v0/transformer.rs:66-82`): the contact's script for the payout, and each signed exit record's stored Core script for the move. Until a proof at that height is read the lock stays. **The payout's document present:** the row is sent. **The nonce taken and the payout's document absent:** not sent, whichever write took the nonce. The two records settle by this evidence, not by main's rule for groups with other terms: the one whose document is present is `done` and the other is `failed`, never `replaced`. Neither document present (a second device took the nonce): both `failed`. **Not sent** means the payout can never execute: its row settles not sent, the lock clears, and the contact's address is not returned to the pair. It stays consumed, and the next payment reserves a new one. C5b leaves main's exact-terms set as it is, `Transfer` alone (`P/platform_moves.rs:2555-2559`). C5b also keeps a band of 3 × the fee estimate on the payout's fee input, where main's withdrawal planner keeps one. | Rule 1 holds: the move pays the wallet itself, so it is not a second payment to the contact, and at most one of the two executes because both sign the same nonces (`P/platform_moves.rs:54-60`). Without the way out, a payout whose fee input fell short would hold every Platform move and the contact until a deposit reached that one address. Main's rule for groups with other terms attributes nothing because it reads only nonces (`:87-91`); here the two withdrawals pay different scripts, so the proved withdrawal document says which executed. The document's owner is the withdrawals contract's own, so no user can remove it. An address withdrawal has no paid failure at the pin, so with one device a taken nonce is always one of the two; the absence rule also ends the hold if a second device took it (open risk 4). The address stays consumed because the signed payout's bytes named it and may have reached a node (P5). |
| P9 | A shielded payout whose result is unconfirmed is `pending`, as every shielded spend is (`P/shielded_pool.rs:996-1045`). The contact stays locked until the scan or the journal decides. The user's acknowledgement of an unresolved spend in Tools ▸ Repair (A4c) does not clear the contact's lock. | An acknowledgement is not evidence (rule 2). The exit is real: the nullifiers are seen spent, or the anchor expires with them unspent. |
| P10 | History names the contact from the payout row. `ShieldedActivity` and the Platform activity row gain `contact` (the `Counterparty` record), joined by the row's reference. Without a row (a restore from the seed), the join falls back to the destination address through the existing payee scripts, with their shared-xpub exclusion (`P/attribution.rs:12-14`, `:243-253`). | The wallet does not fund the asset unlock, so the Core rule cannot attribute it. The payout row is the wallet's own durable word; the address fallback is the same "by address first" rule Core uses. |
| P11 | `contact_activity` and the frequent strip include payouts. `ActivityItem` and `PaymentLock` gain `source` (`core`, `shielded`, `platform`) and `reference`. `txid` keeps its type and equals `reference`; a host reads `source` before treating it as a Core txid. `ActivityItem` also gains `core_txid`, set once the wallet has seen the payout on Core. `contact.payment_locked{txid}` carries the same reference. | Additive: no host breaks, and dw-ffi binds no DashPay record (DP1-09 D5). A payout the user just made must show at once, before any Core transaction exists. |
| P12 | The Core arrival is display only. If the wallet reports a transaction that pays the reserved address, the engine matches it by address and amount, fills `core_txid`, and shows no second row for it. If the wallet does not report such transactions, the row stays "Sent" with no Core link. | Whether key-wallet reports an unfunded transaction to a contact's external address is not established at this base; C5-0's brief checks it first. Nothing in the money rules depends on the answer. |
| P13 | The amount typed is what leaves the balance for the contact. The confirm shows "They receive about N Dash" from the quote's `expected_duffs`, and the fee. No Max and no "subtract fee" for these sources. Ruled (M3). | The existing quotes take credits and return the expected payout (`P/shielded_pool.rs:781-796`; `P/platform_moves.rs:171-176`). Solving backwards for an exact received amount would need a new planner for a small gain. |
| P14 | The smallest and largest payment are Platform's withdrawal limits at the version in force (`P/withdrawal.rs:446-453`). Below the minimum the refusal names it and points to the Dash wallet. | A payout is a withdrawal. The limit is consensus, not ours to change. |
| P15 | "Accept and pay" stays one lease and one prompt. `begin_flow(AcceptAndPay)` takes the source; the lease carries the accept's scope plus the one key class the source needs (the shielded spend key, or the Platform-address signer), never the Core spend signer and never both. One `PlatformOp` grant covers the accept's credits plus the payout's total. A plain payout uses a new one-payment flow, `FlowKind::ContactPayFromBalance`, from a `PlatformOp` grant. It is not `ContactPayment` (`P/flows.rs:37`), which is a Core draft under a `Spend` grant; neither lease is accepted for the other's call. | DP3-01 D14 and DP3-06 D10. Both the accept and a shielded or Platform spend are charged in credits (`P/shielded_pool.rs:953`), so one credit cap is exact. Partial success is unchanged (DP3-01 D15): "You are now contacts" and the payment's own outcome. |
| P16 | The "From" step defaults to Dash wallet every time and does not remember the last source. A source with nothing to pay with is listed with its reason, not hidden. Ruled (M4). | The same rule as F18, for the same reason: the private balance is never spent by habit. |
| P17 | `note` is stored in the payout row and shown in history. It is not written to the chain. Ruled (M5). | An unshield to Core has no memo, and the wallet does not author the asset unlock, so there is nowhere on chain to put it. |

### Outcomes

| Outcome of the call | Lock | Address | What the user sees |
|---|---|---|---|
| Refused before a signature | none written, or its row removed | stays reserved for the pair | The refusal. "Nothing was paid." |
| Refused by Platform's structure or signature check | cleared (not sent) | back to the pair | "Payment not sent." |
| Refused for the shielded pool's note floor | held until A5 reads definitely unsent, then cleared (not sent) | consumed | "The shielded pool is too small for this right now. Dash Wallet is checking that nothing was sent. Do not pay again until this is settled." |
| Executed | cleared (sent) | consumed | "Payment sent. It arrives in @bob's wallet once the Dash network processes it." |
| Unknown | held | consumed | "Payment status unknown" and the locked banner. |
| Later proved executed | cleared (sent) | consumed | The banner goes; history shows it sent. |
| Later proved never executed | cleared (not sent) | consumed | "The payment was not sent. You can pay this contact again." |
| A "Move to Dash wallet" executed in a held Platform payout's place (P8) | cleared (not sent) | consumed | "The payment was not sent. The amount moved to your Dash wallet instead. You can pay this contact again." |

A restart at any point reads the payout rows back. A `pending` row with its source record is the lock again, and that
record (the shielded journal's artifact, the move record) settles it as it would a live one. A `pending` row with no
source record (no move record; no `st_hash`, or an `st_hash` with no artifact) is cleared as not sent, and its address
goes back to the pair (P6).

### Copy

| Where | Text |
|---|---|
| Step title | "From" |
| Rows | "Dash wallet", "Shielded balance", "Platform balance", each with its balance |
| Confirm | "Pay @bob N Dash from your Shielded balance. They receive about M Dash. Network fee F." (and the same for Platform) |
| Sent | "Payment sent. It arrives in @bob's wallet once the Dash network processes it. This can take a while." |
| History row status before Core arrival | "Sent · arriving" |
| Unknown | "We could not confirm this payment. It may still go through. Do not pay again until this is settled." |
| Locked banner (extends DP3-06 D6) | "A payment to this contact from your Shielded balance may have been sent." (or "Platform balance"). Button: "Check again". |
| Below the minimum | "The smallest payment from here is N Dash. Pay a smaller amount from your Dash wallet." |
| A Platform move is pending | "Another Platform transfer is still being confirmed. Try again when it is done." |
| A shielded spend is unknown | "An earlier shielded payment is still being checked. Try again when it is settled." |
| Not available at this version | "Not available on this network yet." |
| Accept went through, payment did not | "You and Bob are now contacts." then the payment's refusal (DP3-06 D12) |

### Contract rows (m4)

| Slice | Section | Rows |
|---|---|---|
| C5-0 | §2.5, §3, §4, §5 | `contact_pay_quote`, `contact_pay` (every source refused `platform.feature_off` until its slice lands); `ContactPayQuote` and `ContactPayOutcome`; `PaymentLock.source`, `.reference`; `ActivityItem.source`, `.reference`, `.core_txid`; `FlowKind::ContactPayFromBalance`; `begin_flow(AcceptAndPay)`'s source; the lock rules P6 and P7; the address rule P5. Schema: `dp_contact_payout`. |
| C5a | §2.5, §2.13, §3 | `source: shielded`; `ShieldedActivity.contact`; P9; the pool-floor rule (P5). |
| C5b | §2.5, §2.12, §3 | `source: platform`; the move kind `ToContact` and its activity row with `contact`; the held-payout rule P8 (its own retry, or a "Move to Dash wallet" at its inputs and nonces; the exact-terms set unchanged); the statement that `move_to_core`'s own-address rule is unchanged (P4). |
| C5c | none | Renderer only. |

C5-0 carries the shared rows and merges first; C5a and C5b each add one source.

### Tests that prove the money rules

C5-0 (shared rows; each source's slice runs the same list for its source):

- An unknown outcome holds the contact: a second payment from each of the three sources is `contact.payment_locked`.
- The payout row is on disk before a signature is released. Mutant: write it after the call; the kill-before-return
  test must fail.
- A restart with a `pending` row and its source record locks the contact again, and the source's evidence then
  settles it.
- The lock clears on execution and on proof of never executing, and on nothing else. Mutant: clear on a timeout.
- An address that may have reached the network is never paid again; one refused before a signature is reused.
- Rule 5 refuses before the grant is redeemed and before an address is taken, and the re-check after the reservation
  drops an address that no longer verifies.
- History, contact activity and the frequent strip name the contact; no second row appears if the payout is later seen
  on Core.
- "Accept and pay": one grant, one lease; an accept that succeeds with a payment that fails leaves the contact
  established and the address reserved; the lease cannot sign a Core spend, and a `ContactPayment` lease is refused by
  `contact_pay`.
- `lock_order_contact_pay`: the call runs against the existing holders of the later locks and does not deadlock.

C5a (Shielded):

- The spend goes through the dispatch hook, and the journal's disposition for the kind settles the lock. Mutant: clear
  the lock on the Repair acknowledgement.
- `shielded.unrecorded_spend` refuses the payout unless confirmed, before an address is taken.
- The withdrawal minimum and maximum refuse before the grant is redeemed.
- The accept writes `st_hash` into the payout row before the artifact, and refuses the spend when that write fails
  (nothing is sent). Mutant: write the artifact first.
- A kill after each of the two writes. After the `st_hash` write alone, the relaunch clears the row as not sent,
  unlocks the contact and returns the address to the pair. After the artifact, the lock stays and A4b and A5 settle
  it. Mutant: clear a row whose `st_hash` has an artifact.
- A pool-floor refusal holds the lock and keeps the address consumed; a Core payment to the contact is refused
  meanwhile; A5's definitely-unsent clears the lock and the address is not reused. Mutant: give the address back on
  the refusal.

C5b (Platform):

- `move_to_core` still refuses an address that is not the wallet's own. Mutant: route the contact target through it.
- `to_contact_fee_shortfall_holds_and_to_core_exits`: the payout is refused for its fee input's shortfall and the
  record holds, with the contact locked; a "Move to Dash wallet" at its inputs and nonces runs and executes; a proof
  at or above the nonce proof shows the withdrawal paid the wallet's own address; the payout row settles not sent, the
  lock clears, the address is not returned to the pair and the next payment reserves a new one. Mutants: return the
  address; settle on the nonces alone.
- Beside a held payout, a move at other inputs or nonces and a second payment to the contact are refused; its own
  retry on the same terms runs.
- The opposite branch: with both signed at one nonce and the payout's withdrawal document proved, the row is sent,
  the `ToContact` record is `done`, the move's record is `failed` and no "Moved to Dash wallet" row appears.
  Mutant: main's group rule.
- The nonce taken with neither document present at a fresh height → not sent, and the lock clears. A proof older
  than the nonce proof decides nothing and the lock stays. Mutant: drop the height check.
- The payout's fee input keeps 3 × the fee estimate. Mutant: main's planner, which keeps one.
- A kill between the payout row and the move record: the relaunch clears the row as not sent, unlocks the contact and
  returns the address to the pair. Mutant: keep such a row as the lock.
- Nonce settlement as "Move to Dash wallet". Mutants: settle without the proved nonce; clear the lock on the library's
  `Ok` without proof.
- The withdrawal minimum and maximum refuse before the grant is redeemed.

C5c: vitest for the "From" model (default, reasons, the lock banner per source, the confirm lines); Playwright on
Chromium and WebKit; DEC-1484 images, light and dark.

### Out of scope

- Max and "subtract fee" from Shielded or Platform.
- Paying a contact's Platform or shielded address directly: DashPay payments go to the contact's DIP-15 Core address.
- Paying from identity credits.
- Coin control, PSBT and "Create unsigned" for these sources.
- An exact-received amount (P13).
- Rebuilding payout history after a restore from the seed alone, beyond the address fallback of P10.

---

## Slices

Each engine slice is at most about 500 non-test lines. Sizes are estimates.

| Slice | Content | Non-test lines | Depends on |
|---|---|---|---|
| C4a-1 | Platform registration: the quote, the input-0 plan (F9), the stored kind (F2–F4), the move kind `Register` with its settlement, the fresh-absence rule and the "Move to Dash wallet" exit (F10), dw-app DTOs | ~400 | D-C4; C-routes §0 table |
| C4a-2 | Platform registration: the flow step, the lease scope (F5), the SDK create and its follow-ups (F8), resume, discard, `PaymentUnknown` (F6, F7) | ~350 | C4a-1 |
| C4b-1 | Shielded registration: the readiness quote, the denomination per version, the stored kind (F14, F16, F17) | ~300 | D-C4 |
| C4b-2 | Shielded registration: the flow step through the hook, the evidence order, the fallback and the refund record (F11–F13, F15) | ~450 | C4b-1, UP-1, RP-1, A5, A4b, and C-routes' C1b-2, which adds C1-9's record; C4b-2 adds only the registration source to it |
| C4c | The picker (F19–F22) and the held state; per source | ~350 | C4a-2 (Platform), C4b-2 (Shielded) |
| C5-0 | Shared rows: the two calls, `dp_contact_payout`, the lock's source and reference, the activity fields and joins (P10–P12), `ContactPayFromBalance`, the `AcceptAndPay` source (P15) | ~450 | D-C5 |
| C5a | Shielded source: the payout through `unshield`, the `st_hash` write before the artifact (P6), P9, the pool-floor rule | ~300 | C5-0, A5 |
| C5b | Platform source: the `ToContact` target and kind, the held-payout rule with its "Move to Dash wallet" exit and evidence, the 3 × fee band (P8), nonce settlement | ~350 | C5-0; C-routes §0 table |
| C5c | The "From" step; per source | ~250 | C5a, C5b |

## MG rulings

M1–M5 were ruled as recommended (MG, 2026-11-19 05:1xZ). MG also ruled one create route for C4a: the SDK's
`put_with_address_funding` at recorded nonces (F8).

| # | Row | Ruling |
|---|---|---|
| M1 | F19 | The other sources sit behind a "Pay another way" link on the Join page; the main button stays the Core start. |
| M2 | F17 | The 3-hour rest blocks a Shielded-funded registration, with no override, measured from the latest incoming shielded activity. |
| M3 | P13 | The typed amount is what leaves the balance; the confirm shows what the contact receives. No Max for these sources. |
| M4 | P16 | "From" opens on Dash wallet every time and does not remember the last choice. |
| M5 | P17 | The note stays in this wallet's history only. |

## Open risks

1. **The created id from the hook.** C4b needs the pool-funded create's identity id in the artifact or the call's
   return on every path (F11). If UP-1 does not carry it, the fallback is the lookup by the row's index (F12), which
   waits for a discovery pass.
2. **Restore from the seed.** A contact address paid from Platform or Shielded is found used again only if the wallet
   reports transactions to a contact's external address (P12). If it does not, the next payment after such a restore
   may reuse that address. The money still reaches the contact; the rotation is weaker.
3. **Downgrade.** An older build must not pay a contact that a `dp_contact_payout` row locks. C5-0's brief checks that
   an older build cannot open the newer app database; if it can, the lock is also mirrored into `dp_payment_lock`.
4. **Two devices on one wallet.** The Platform rules assume one device per wallet, as the moves do today
   (`P/platform_moves.rs:91`). A second device can take a nonce; F10 and P8 then read the proof, never a guess. A
   payout replaced that way settles not sent once its document is proved absent (P8). A registration replaced that way
   reads as a paid failure and fails without planning again (F10); the user starts a new one.
5. **The rest rule after a restore** (F17) treats the notes of the first completed scan after a create, restore or
   import as rested. A wallet restored (or upgraded to the build with the clock) minutes after shielding or being paid
   can register from Shielded at once; any note first seen after that scan rests 3 hours. A note first seen before the
   first completed scan is recorded (a failed mark write, an exit between the sync and the recording, a first scan
   that ends `Err`, or a quote before the mark) counts as restored too. Deleting and re-importing the wallet counts
   its notes as restored again; M2a allows it.
6. **A pool-floor refusal waits on A5.** Until A5's disposition can read a floor-refused spend definitely unsent and
   abandon its stored transition, such a registration or payout stays `PaymentUnknown` or locked (F12, F16, P5).
7. **The metered fee has no ceiling.** The reserve is 3 × the minimum fee (F9, P8). Past that band Drive refuses the
   transition unpaid and it stays valid. A held `Register` or `ToContact` then has the "Move to Dash wallet" exit (F10,
   P8), which the user must take: until then the wallet's other Platform moves are `credits.move_pending`, and for a
   payout the contact stays locked. The exit moves the funds to the Dash wallet, not to the contact, and uses up one of
   the contact's addresses. C4c offers it for a registration (F22). C5c must offer it from the locked banner; the
   copy for that is not written here.
8. **Size.** C4b-2 is the largest slice and depends on UP-1, RP-1, A5, A4b and C1b-2. If the date slips, items 4 and
   5 are the first cut (lane-C schedule).
