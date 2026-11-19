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
  kinds and the two this document adds: `Register` (F9, F10) and `ToContact` (P4, P8). For both, the only move
  allowed beside a held record is its exact retry.
- **The input-0 fee reserve** (C-routes C1-4, C1-5): a Platform route whose transition takes its fee from input 0
  leaves a fee reserve unclaimed there. F9 applies it to registration.

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
| F5 | One `Registration` lease still covers the whole flow. For a `platform_addresses` row it also carries the Platform-address signer. For a `shielded` row it also carries the shielded spend key. It never carries both, and neither row gets the Core funding signer. The grant is a `PlatformOp` with `max_duffs` 0. For Platform, `max_credits` = `funding_credits` + `fee_reserve_credits` + the name and profile credits. For Shielded, `max_credits` = the denomination + the name and profile credits. | The scopes exist (`rust/crates/dw-vault/src/signer.rs:113`, `:159`) and the permit already hands them out (`P/lease/call.rs:841-850`, `:855`). The proof-of-possession signatures come from the same identity signer as today (`P/registration_flow.rs:3618`). A lease holds only the keys its row can use. The grant consents to the planned debit; the metered fee has no ceiling (C-routes C1-5). |
| F6 | `funds_committed` for a new-source row is true from the moment a signature for its identity call may have been released, until evidence proves that call can never execute. `discard_registration` refuses while it is true. | m4 §2.2's discard rule, and the invitation's `claim_discard_refused` (`P/registration_flow.rs:1180-1203`): a discard must not leave an identity on Platform that no step finishes. |
| F7 | An unknown outcome is a wait, not a failure. New `RegistrationWait::PaymentUnknown`: the row keeps its phase, the next proved read settles it, and no user action is needed or offered except "Check again". | `platform.broadcast_unknown` as a failure reads "Registration stopped → Open Repair" (DP1-09 R8), and Repair's "Finish transfers" handles asset locks only. These sources have no lock to finish; reads alone decide. |

**Platform addresses (C4a)**

| # | Decision | Why |
|---|---|---|
| F8 | **One create route.** The registration signs at nonces the engine fixed and recorded first, through the SDK's `PutIdentity::put_with_address_funding(&sdk, inputs_with_nonce, None, identity_signer, address_signer, settings)` (`rs-sdk/src/platform/transition/put_identity.rs:115-123`, body `:399-470`), reached through the `sdk` handle the flow already holds (`P/registration_flow.rs:3155-3159`). It never uses the library's `register_from_addresses`, which fetches the nonces itself at submit (`W/identity/network/register_from_addresses.rs:101-110`), nor rs-sdk's `fetch_inputs_with_nonce`. After a proved result the engine calls the library's two follow-ups: `reconcile_address_infos` through `wallet.platform()` (`W/platform_addresses/wallet.rs:292`), with no wallet-state guard held (its lock order is provider, then wallet manager, `:262-270`); then `add_identity` through `wallet.state_mut()` and `wallet.persister()` (`W/identity/state/manager/lifecycle.rs:36`), as `P/identity_reads.rs:289-293` does. `IdentityAlreadyExists` from `add_identity` (`lifecycle.rs:45-47`) is success: discovery added it first. Neither follow-up decides anything; a failure of either is logged and the next sync heals it. No fork unit is needed. | The identity id is a hash of the input addresses and their nonces (`rs-dpp/src/state_transition/traits/state_transition_identity_id_from_inputs.rs:26-44`). With fetched nonces the id is unknown before signing, and two attempts could sign at different nonces and create two identities from two debits. With fixed nonces Drive executes at most one transition per input nonce (`P/platform_moves.rs:54-60`). The SDK form returns the proved identity and the address infos in one call. |
| F9 | Order of the funding step, under the Platform move lock: one proved read under rule 5 (`P/platform_moves.rs:8-13`, `:2456`); refuse with `credits.move_pending` if any move is pending or holding; plan; write the plan into the row (F3), durably; write a move record of the new kind `Register` naming the draft, `pending` and able to land, and read it back; only then sign and send. **The plan** follows C-routes C1-4: the candidates are the funded addresses in `PlatformAddress` order; the claims sum to exactly `funding_credits`, with no change output; and input 0, the first in that order, keeps `fee_reserve_credits` unclaimed on top of its claim. The reserve is the registration cost and the transition's fee at the version in force, with the planner's band. A plan whose input 0 cannot keep the reserve is refused before anything is written or signed (`platform.insufficient_credits`). The quote's debit is `funding_credits` plus the fee, shown as "about". Registration has no Max. | The SDK's create fixes the fee on input 0 (`put_identity.rs:414-415`). Drive gives the identity exactly the sum of the claims and takes the registration cost and the metered fee from input 0's remaining balance, or refuses with a state error. That refusal is not `never_valid` (`P/platform_moves.rs:1403-1413`), so a plan that drained input 0 would hold every Platform move until funds happened to arrive there. The move planner's largest-first fee source (`:1215`) cannot be used for the same reason. The order is the one every Platform-address move follows (`:54-57`, `:2441-2500`); recording the debit as a move gives the existing hold, the nonce settlement, and the rule that a move's own spend is never an `outgoing` activity row (`:21-24`). |
| F10 | Settlement. **Done:** the SDK's proved result for this transition, which binds execution and checks the identity id against the inputs (`put_identity.rs:447-463`); or, after a restart or an unknown answer, a proved read that shows every input at or past its signed nonce together with a proved fetch of the planned identity. The row adopts that id and moves to `IdentityRegistered`. **Can never execute:** Platform's structure or signature check refused it; or an input's nonce is taken and the planned identity is proved absent; or, in the process that made the call and never after a restart, the call returned having released no signature. The move is `failed` and unable to land, the plan is cleared from the row, and the row may plan again. **Anything else:** `PaymentUnknown` (F7), and the row keeps its plan. A plan whose move is `failed` by height may still execute, so the row re-signs only that plan, and the only move allowed beside it is that exact retry (the C-routes §0 table, kind `Register`). | Rule 1. Re-signing the same inputs, nonces, claims and keys gives the same identity id, so a retry is idempotent. A `pending` record read back after a restart is always treated as able to land (`P/platform_moves.rs:76-82`, `:2540-2545`): the engine cannot know whether a signature left. A row imported without its move record follows the same cases: it can re-sign its plan safely whether or not another install already signed it. |

**Shielded (C4b)**

| # | Decision | Why |
|---|---|---|
| F11 | The identity call is the library's pool-funded create through the dispatch hook, with the keyset entry point UP-1 adds (never the seed path, `PW:1690`). The engine records the hooked `IdentityCreate` artifact (`rust/crates/dw-appdb/src/dispatch.rs:539-545`) and writes the created id into the row when the call returns it. The id is `double_sha256` of the sorted nullifiers (`OPS:2188-2189`), so it exists only once notes are selected. If the hook's registration does not carry it, UP-1 adds it. | The engine holds a key set, never the seed (lane-C §0 finding 1). The id cannot be planned ahead as F9 does, so the artifact is the record of what may have been sent. |
| F12 | Evidence, in this order. The journal's disposition of the artifact (item 0: executed, definitely unsent, unresolved). Then the identity: by the row's stored id, else by the seed's identity at the row's index after a discovery pass (`P/registration_flow.rs:1921-1936`). **Executed, and the identity exists at the stored id:** adopt it. **Executed, no identity at the stored id, an identity of this seed at the row's index:** the duplicate-keys refund; adopt that identity (F13). **Executed, and no identity of this seed at the stored id or at the index after a discovery pass:** the create took Platform's fallback for another reason (key bounds or key limits). The row fails with `registration.refunded`, not retryable; its plan is cleared; nothing retries automatically (F13 for where the funds are). **Definitely unsent** (A5 reads it so, with the stored transition abandoned and never redriven): the row returns to `KeysPrepared` and may select notes again. A refusal for the pool's note floor is not that: it stays unresolved until A5 says so (F16). **Unresolved:** `PaymentUnknown`; no second create runs while a spend of the wallet is unrecorded or unresolved. | A second create selects other notes, so it would have another id. Platform's fallback fires for duplicate key hashes and also for key contract-bounds and key-limit failures (`rs-drive-abci/.../identity_create_from_shielded_pool/state/v1/mod.rs:66-72`, `:84-101`, `:103-150`); each is deterministic, so a blind retry would lose a penalty every time. A duplicate can never make a second identity. The existing gate `shielded.unrecorded_spend` (`P/shielded_pool.rs:563`, `:1153`) already blocks the next spend. |
| F13 | The required fallback address is always the wallet's own Platform receive address (`P/platform_addresses.rs:429`), fixed at Draft and stored in the row. It is never typed, pasted or external. When Platform takes the fallback, the denomination less a penalty lands there (`state/v1/mod.rs:72`, `:137`). The rise is attributed with C-routes C1-9's record and kind: a `FromShieldedRecord` whose source is the registration's draft, shown as `PlatformActivityKind::FromShielded` with the label "Returned from a registration". It is written and armed only once the refund is proved: the disposition reads executed and the identity at the stored id is proved absent. It absorbs a rise of the fallback address up to the denomination less the penalty, and is `done` when it has. It is never armed while the create is pending, and never in the success case. It is written under the shielded wallet lock, then the move lock (lock order above). Two armed records on one address (a C1b unshield defaults to the same address) absorb in the order they were armed, each up to its own credits, and no credit is counted for both. | iOS passes a required Platform fallback (`ios.md:673`). On desktop the only safe meaning is "back to me". Arming while the create is pending would absorb unrelated deposits to the wallet's default address and, on success, keep them, hiding an `incoming` row. Arming after the proof costs at most a wrong label: a read that runs between the execution and the arming shows the rise as a plain `incoming` row. No money depends on it. |
| F14 | The denomination comes from the protocol version in force, never from a constant: `shielded_identity_create_denominations`. At PV12 the set is 0.1, 0.3, 0.5, 1 DASH (`PV/drive_abci_versions/drive_abci_validation_versions/v8.rs:434`, used by `PV/v12.rs`). From PV13 it is 0.03, 0.1, 0.25, 0.5, 1 DASH (`v9.rs:431`, `v10.rs:506`). The engine picks the smallest member that, less the predicted create fee, covers what the name and profile need at that version. An empty set is `platform.feature_off{feature: "shielded_registration"}`. | The library refuses a non-member before proving (`W/shielded/note_selection.rs:230-242`) and refuses a denomination that does not exceed the fee (`:252-259`). Core's amounts are the yardstick: 0.03 DASH for a plain name (`P/registration_flow.rs:79`) and the version's contested amount (`:92-98`). This also follows iOS #1267 (0.03 DASH for a standard name) without hard-coding it. |
| F15 | The change goes back to the pool, to the account's default shielded address (`OPS:2179-2180`, `:2256`). It is not spendable until a sync pass finds it. The quote says so, and the engine counts it as the wallet's the whole time. | The library sets the change address itself; the engine has no choice to make. The copy prevents "my shielded balance dropped by more than the fee". |
| F16 | Readiness is an engine quote, checked again at the start. Items: the pool is active at this version (PV12); the feature is on; a denomination exists (F14); the spendable balance covers it; no unrecorded or unresolved spend; the shielded tree is synced; the notes have rested (F17). The pool's 250-note floor (`minimum_pool_notes_for_outgoing`, `v10.rs:470`) cannot be read ahead, so it shows up as the refusal `shielded.pool_too_small{notes, required}` at the call (`P/shielded_pool.rs:853-854`). That refusal is not final: the signed create stays valid while its anchor is, and the pool only grows. The row waits in `PaymentUnknown` until A5 reads the artifact definitely unsent with the stored transition abandoned (F12). | iOS's checklist (`ios.md:37`, `:673`). Every item but the rest is already a refusal of a shielded spend on main; the quote only reports them before Confirm. The floor is a state check on a count that only rises (`rs-drive-abci/.../shielded_common/mod.rs:320-349`), and the engine keeps the signed bytes for redrive (C-routes R6). |
| F17 | Note rest: the source is not ready until 3 hours after the wallet's latest incoming shielded activity (`ShieldedActivity.created_at`, `P/shielded.rs:383-384`). A restored wallet's rows have no time; they count as rested once the shielded sync is complete. The engine enforces it at the start, with no override. Ruled (M2). | iOS's rule. It is a privacy rule, not a consensus rule: spending a note minutes after shielding it links the two. Using the latest incoming activity is stricter than checking only the selected notes, and needs no library change. |
| F18 | Shielded is never the default (DASHPAY F26, `docs/design/DASHPAY.md:1129`). The picker opens on Dash wallet every time, does not remember the last source, and the Join page's main button stays the Core start. The engine has no default funding. | The decision is already made; this row says where it is enforced. |

**Picker (C4c)**

| # | Decision | Why |
|---|---|---|
| F19 | The Join page keeps "Create a username" as the one main action (Core). A secondary link, "Pay another way", opens the "Pay with" step: Dash wallet (selected), Platform balance, Shielded balance. Each row shows its balance and, when it cannot pay, the one reason. Ruled (M1). | DP1-09 D1's two-step wizard stays the common path. iOS hides Platform funding behind Advanced mode (`ios.md` PADDR-14); desktop has no such mode, so the link is the equivalent. |
| F20 | The confirm step shows the source, what leaves it, the fee, and what the identity starts with. For Shielded it also shows the denomination and the change line (F15). | DP1-09's confirm shows the lock and fee for Core; the same facts, per source. |
| F21 | Resume follows DP1-09's rule "the engine's rows are the truth". `PaymentUnknown` maps to the progress screen with "Check again". A contested name is allowed from both sources, at the larger amount the quote gives (F14, F9). | DP1-09 R1–R8 and D4. |

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
| Confirm, Platform | "About N Dash leaves your Platform balance: M for your identity and a network fee of about F. Your identity starts with M." (M is `funding_credits`, exact; N is M plus the fee.) |
| Confirm, Shielded | "N Dash leaves your Shielded balance. The rest of the notes used returns to your Shielded balance after the next sync." |
| `registration.quote_changed` | "The cost changed. Review it and confirm again. Nothing was paid." |
| `PaymentUnknown` | "We could not confirm the payment yet. Dash Wallet keeps checking; do not start again. Nothing more is needed from you." Button: "Check again". |
| Refund, an identity already existed (F13) | "This wallet already had an identity, so the payment came back to your Platform balance, less a network fee of N." |
| `registration.refunded` (F12) | "The network could not create the identity with this wallet's keys. The payment came back to your Platform balance, less a network fee of N. Nothing else was paid." |

### Contract rows (m4)

| Slice | Section | Rows |
|---|---|---|
| C4a-1, C4a-2 | §2.2, §2.12, §3, §4 | `RegistrationFunding::PlatformAddresses{funding_credits, fee_reserve_credits}`; `RegistrationQuote` gains `funding_credits`, `fee_reserve_credits`, `debit_credits` (their sum, "about"), `available_credits` (0 for other sources; the duff fields are 0 for this one); `RegistrationWait::PaymentUnknown`; `registration.quote_changed`; the start's refusals `credits.move_pending` and `platform.insufficient_credits`; the input-0 reserve (F9); the rule "a planned row re-signs only its plan" (F10). §2.12: the move kind `Register`, its activity row, and its line in the held-record rule. |
| C4b-1, C4b-2 | §2.2, §2.12, §2.13, §3, §4 | `RegistrationFunding::Shielded{denomination}`; `RegistrationQuote.readiness` (each item of F16 with its state, and `ready_at` for the rest); `registration.shielded_not_ready{reason, ready_at}`; `registration.refunded`; `platform.feature_off{feature: "shielded_registration"}`; the fallback rule and the refund record's arming (F13); the evidence order (F12); the pool-floor rule (F16). |
| C4c | none | Renderer only. |

A new error code may touch `rust/crates/dw-ffi/src/api/error.rs`; if its mapping is exhaustive, the slice runs
`scripts/build-core.sh` and commits the regenerated Swift (lane-C §0 finding 7).

### Tests that prove the money rules

C4a:

- A kill at each row of the table above loses nothing, and the relaunch pays once (mock SDK, as DP1-02 §11.2).
- The transition is signed at the recorded nonces. Mutant: fetch nonces at submit; the test that moves a nonce between
  plan and call must fail.
- A row with a plan re-signs the same terms and reaches the same identity id. Mutant: re-plan while the first is
  unknown.
- Settlement needs the SDK's proved result, or both the nonces and the identity from proved reads. Mutants: settle on
  the nonces alone; settle on an answer that carries no proof.
- `fee_reserve_on_input_0`: after the plan, input 0 keeps `fee_reserve_credits` above its claim and the claims sum to
  `funding_credits`; a plan whose input 0 lacks the reserve is refused before signing. Mutant: the move planner's
  largest-first fee source.
- The grant covers `funding_credits` plus the reserve plus the name and profile credits, and no less.
- A kill after the record with no signature known keeps the plan: after a deposit changes the balances, the relaunch
  signs the same inputs and nonces. Mutant: clear an unsigned `pending` record at load.
- `IdentityAlreadyExists` from `add_identity` is success, and a failed `reconcile_address_infos` changes no decision.
- `lock_order_register`: the call runs against a holder of the move lock and does not deadlock.
- A refusal before signing keeps the grant and leaves no move record.
- A pending registration refuses the next Platform move with `credits.move_pending`, and a pending move refuses the
  registration.
- No `outgoing` activity row appears for the registration's debit.
- An older build's decode of both new kinds is `Unsupported`; a newer row is never funded or discarded there.
- Discard is refused while `funds_committed`.

C4b:

- The kill matrix, including both item-0 windows (before and after the artifact write).
- The id written to the row equals `double_sha256` of the sorted nullifiers of the built bundle.
- Mutant: the seed entry point. A test that the hook saw the spend must fail.
- No second create while a spend is unresolved or unrecorded.
- Every readiness item refuses at the start as well as in the quote. Mutant: check the rest only in the quote.
- The denomination follows the version: PV12, PV13 and PV14 give the members of F14; an empty set is `feature_off`.
- The fallback is the wallet's own receive address. Mutant: accept a caller's address; the test must fail to compile or
  fail.
- A duplicate-keys refund adopts the existing identity and shows one attributed activity row.
- Executed with no identity at the stored id or the index fails the row with `registration.refunded` and starts no
  second create. Mutant: treat it as unsent.
- The refund record is armed only after the proof. A deposit to the fallback address while the create is pending, and
  one after a successful create, are both plain `incoming` rows. Mutant: arm at the hook.
- Two armed records on one address absorb in arming order and never count one credit twice.
- A pool-floor refusal keeps the row in `PaymentUnknown` until A5 reads definitely unsent. Mutant: return to
  `KeysPrepared` on the refusal.
- `lock_order_shielded_register`: shielded lock, then move lock, against the existing holders.

C4c: vitest for the picker model (default, no memory of the last source, each disabled reason); Playwright on Chromium
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
| P5 | The pair's reserved address is shared with the Core path: a draft's unused address is the payout's, and the reverse. The payout claims it before it signs. It goes back to the pair in two cases only: the call was refused before a signature, or Platform's structure or signature check refused the transition. Otherwise it is consumed, also when the payment is later proved not sent. A refusal for the shielded pool's note floor is not a give-back: the lock stays and the address stays consumed until A5 reads the payout definitely unsent with the stored transition abandoned and never redriven; then the lock clears and the address is still consumed. | DP3-01 D7 and D21. An address that may have reached the network is never paid twice (DIP-15 rotation). The floor is a state check on a note count that only rises (`rs-drive-abci/.../shielded_common/mod.rs:320-349`), so the signed unshield stays valid while its anchor is, and the engine keeps its bytes for redrive (C-routes R6). Handing the address back would let a Core payment and a later redrive both pay it. |
| P6 | One lock per pair, across all three sources. The payout's reference is assigned first: for Platform it is the move's id, chosen before the move record is written; for Shielded it is an id the engine makes, and the hook's accept writes the artifact's `st_hash` into the row before it returns. The row goes into a new table `dp_contact_payout` (wallet, identity, contact, source, reference, `st_hash`, address, credits, expected duffs, note, state, since), synced as DP3-01 D27 requires, before the source record and before anything is signed. While its state is `pending` it is the pair's lock in memory, so the existing check (`P/payment_locks.rs:202-221`) refuses every other payment to that contact, from any source. A Core lock likewise refuses a payout. **On restart**, a `pending` row whose source record is absent (no move record with its id; no `st_hash`) was never signed: it is cleared as not sent and its address goes back to the pair. | DP3-01 D23: the row, not memory, is the record of every payment that may have gone out. The existing row is keyed by a Core txid and a payout has none before it executes. Both sources write their own record before signing (C-routes R1, R6), so a row with no source record cannot have released a signature; without this rule a kill between the two writes would lock the contact for good. |
| P7 | What clears the lock. **Sent:** Platform executed the transition. Shielded: the spend is `Done`, or item 0's disposition is executed. Platform: the move is `done` as itself. **Not sent:** only proof that it can never execute: a structure or signature refusal, A5's "definitely unsent" with the stored transition abandoned, a Platform move unable to land, or P6's restart case. **Anything else: the lock stays**, a pool-floor refusal included (P5). The Core arrival of the payout is not waited for. | The lock exists to stop a second payment while the first is unknown (DP3-01 D8, D9). Once Platform has executed the withdrawal the outcome is known: the payout is queued by consensus and is only late. Waiting for the asset unlock would hold the contact for a long time with nothing left to learn. |
| P8 | A Platform payout that is `failed` by height still holds its nonces and may still execute. While it holds, the only move allowed beside it is its exact retry: the same inputs, nonces, amount and address. Any other move is `credits.move_pending`. The lock stays until its nonces are taken. C5b adds `ToContact` to main's exact-terms set, which today keys only on `Transfer` (`P/platform_moves.rs:2555-2559`); the C-routes §0 table lists the result beside the other kinds. | Rule 1. This is the rule a held send to a Platform address already has (`P/platform_moves.rs:66-74`). Without it a different move at the same nonces could execute instead, and no proof would say which one did (`:87-91`). |
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

A restart at any point reads the payout rows back. A `pending` row with its source record is the lock again, and that
record (the shielded journal's artifact, the move record) settles it as it would a live one. A `pending` row with no
source record is cleared as not sent, and its address goes back to the pair (P6).

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
| C5b | §2.5, §2.12, §3 | `source: platform`; the move kind `ToContact` and its activity row with `contact`; the exact-retry rule P8 and `ToContact` in the exact-terms set; the statement that `move_to_core`'s own-address rule is unchanged (P4). |
| C5c | none | Renderer only. |

C5-0 carries the shared rows and merges first; C5a and C5b each add one source.

### Tests that prove the money rules

C5a and C5b, each for its source:

- An unknown outcome holds the contact: a second payment from each of the three sources is `contact.payment_locked`.
- The payout row is on disk before a signature is released. Mutant: write it after the call; the kill-before-return
  test must fail.
- A kill between the payout row and the source record: the relaunch clears the row as not sent, unlocks the contact
  and returns the address to the pair. Mutant: keep such a row as the lock.
- A restart with a `pending` row locks the contact again, and the source's evidence then settles it.
- The lock clears on execution and on proof of never executing, and on nothing else. Mutants: clear on a timeout; clear
  on the library's `Ok` without proof (Platform); clear on the Repair acknowledgement (Shielded).
- An address that may have reached the network is never paid again; one refused before a signature is reused.
- Rule 5 refuses before the grant is redeemed and before an address is taken, and the re-check after the reservation
  drops an address that no longer verifies.
- History, contact activity and the frequent strip name the contact; no second row appears if the payout is later seen
  on Core.
- The withdrawal minimum and maximum refuse before the grant is redeemed.
- "Accept and pay": one grant, one lease; an accept that succeeds with a payment that fails leaves the contact
  established and the address reserved; the lease cannot sign a Core spend, and a `ContactPayment` lease is refused by
  `contact_pay`.
- `lock_order_contact_pay`: the call runs against the existing holders of the later locks and does not deadlock.

C5b only:

- `move_to_core` still refuses an address that is not the wallet's own. Mutant: route the contact target through it.
- Beside a held payout only its exact retry runs; any other move is `credits.move_pending`. Mutant: leave `ToContact`
  out of the exact-terms set.
- Nonce settlement as "Move to Dash wallet": mutant, settle without the proved nonce.

C5a only:

- The spend goes through the dispatch hook, and the journal's disposition for the kind settles the lock.
- `shielded.unrecorded_spend` refuses the payout unless confirmed, before an address is taken.
- A pool-floor refusal holds the lock and keeps the address consumed; a Core payment to the contact is refused
  meanwhile; A5's definitely-unsent clears the lock and the address is not reused. Mutant: give the address back on
  the refusal.
- The hook's accept writes the `st_hash` into the payout row before it returns.

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
| C4a-1 | Platform registration: the quote, the input-0 plan (F9), the stored kind (F2–F4), the move kind `Register` with its settlement and held-record rule (F10), dw-app DTOs | ~400 | D-C4; C-routes §0 table |
| C4a-2 | Platform registration: the flow step, the lease scope (F5), the SDK create and its follow-ups (F8), resume, discard, `PaymentUnknown` (F6, F7) | ~350 | C4a-1 |
| C4b-1 | Shielded registration: the readiness quote, the denomination per version, the stored kind (F14, F16, F17) | ~300 | D-C4 |
| C4b-2 | Shielded registration: the flow step through the hook, the evidence order, the fallback and the refund record (F11–F13, F15) | ~450 | C4b-1, UP-1, RP-1, A5, A4b; C1-9's record if C1b merged first, else C4b-2 adds it |
| C4c | The picker (F19–F21); per source | ~350 | C4a-2 (Platform), C4b-2 (Shielded) |
| C5-0 | Shared rows: the two calls, `dp_contact_payout`, the lock's source and reference, the activity fields and joins (P10–P12), `ContactPayFromBalance`, the `AcceptAndPay` source (P15) | ~450 | D-C5 |
| C5a | Shielded source: the payout through `unshield`, the hook's `st_hash` write, P9, the pool-floor rule | ~300 | C5-0, A5 |
| C5b | Platform source: the `ToContact` target and kind, the exact-terms set, nonce settlement | ~350 | C5-0; C-routes §0 table |
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
   (`P/platform_moves.rs:91`). A second device can take a nonce; F10 and P8 then read the proof, never a guess, but a
   payout replaced that way is settled as not sent only once the proof shows it.
5. **The rest rule after a restore** (F17) treats restored notes as rested. A wallet restored minutes after shielding
   can register from Shielded at once.
6. **A pool-floor refusal waits on A5.** Until A5's disposition can read a floor-refused spend definitely unsent and
   abandon its stored transition, such a registration or payout stays `PaymentUnknown` or locked (F12, F16, P5).
7. **The fee reserve's size** (F9) is an estimate with a band; the metered fee has no ceiling. If input 0's remainder
   falls short at execution Drive refuses, and the record holds until the same plan can land.
8. **Size.** C4b-2 is the largest slice and depends on UP-1, RP-1, A5 and A4b. If the date slips, items 4 and 5 are
   the first cut (lane-C schedule).
