# C-funding: registration from Platform or Shielded (D-C4), and paying a contact from any balance (D-C5)

Design notes for PLAN-1.0 lane C, items 4 and 5. They settle what the engine slices C4a, C4b, C5a, C5b and the renderer
slices C4c, C5c build. D-C4 is an addendum to [DP1-02](DP1-02-ownership-model.md) and
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
| F3 | `platform_addresses` stores `credits` (the accepted quote's debit) at Draft. The plan step adds `inputs` (address, nonce, credits each) and `identity` (the id those inputs give). `shielded` stores `denomination` and `fallback` at Draft, and `identity` once it is known (F11). The row's `identity` column is set only when the identity is proved to exist. | The invitation row does the same: the expected id lives in `funding`, and the column is "the only identity the row adopts" (`P/registration_rows.rs:104-113`). The plan travels with the row in a `.dwbackup`, so an imported row still knows what it may have signed. |
| F4 | `RegistrationRequest.funding` gains `PlatformAddresses{credits}` and `Shielded{denomination}`. The host passes back the number its quote showed. A start whose number no longer matches the engine's own plan is refused with `registration.quote_changed` before anything is written. | `CoreBalance` keeps "the locked amount from the accepted quote" for the same reason (`P/registration_rows.rs:98-101`): the user confirmed one number, and a protocol-version change between quote and start must not swap it. |
| F5 | One `Registration` lease still covers the whole flow. For a `platform_addresses` row it also carries the Platform-address signer. For a `shielded` row it also carries the shielded spend key. It never carries both, and neither row gets the Core funding signer. The grant is a `PlatformOp` with `max_duffs` 0 and `max_credits` = the debit (F3) plus the name and profile credits. | The scopes exist (`rust/crates/dw-vault/src/signer.rs:113`, `:159`) and the permit already hands them out (`P/lease/call.rs:841-850`, `:855`). The proof-of-possession signatures come from the same identity signer as today (`P/registration_flow.rs:3618`). A lease holds only the keys its row can use. |
| F6 | `funds_committed` for a new-source row is true from the moment a signature for its identity call may have been released, until evidence proves that call can never execute. `discard_registration` refuses while it is true. | m4 §2.2's discard rule, and the invitation's `claim_discard_refused` (`P/registration_flow.rs:1180-1203`): a discard must not leave an identity on Platform that no step finishes. |
| F7 | An unknown outcome is a wait, not a failure. New `RegistrationWait::PaymentUnknown`: the row keeps its phase, the next proved read settles it, and no user action is needed or offered except "Check again". | `platform.broadcast_unknown` as a failure reads "Registration stopped → Open Repair" (DP1-09 R8), and Repair's "Finish transfers" handles asset locks only. These sources have no lock to finish; reads alone decide. |

**Platform addresses (C4a)**

| # | Decision | Why |
|---|---|---|
| F8 | The registration signs at nonces the engine fixed and recorded first. The library's `register_from_addresses` fetches the nonces itself at submit (`W/identity/network/register_from_addresses.rs:101-110`), so C4a does not use it as it stands. It uses the SDK's explicit form (`rs-sdk/src/platform/transition/put_identity.rs:115-123`), then the library's two public follow-ups: `reconcile_address_infos` (`W/platform_addresses/wallet.rs:292`) and `add_identity` (`W/identity/state/manager/lifecycle.rs:36`). If those are not reachable from the engine's handles, C4a needs a small fork unit **UP-2** (`register_from_addresses` taking inputs with nonces) that rides RP-1. | The identity id is a hash of the input addresses and their nonces (`rs-dpp/src/state_transition/traits/state_transition_identity_id_from_inputs.rs:26-44`). With fetched nonces the id is unknown before signing, and two attempts could sign at different nonces and create two identities from two debits. With fixed nonces Drive executes at most one transition per input nonce (`P/platform_moves.rs:54-60`). |
| F9 | Order of the funding step, under the Platform-address wallet lock: one proved read under rule 5 (`P/platform_moves.rs:8-13`, `:2456`); refuse with `credits.move_pending` if any move is pending or holding; plan the inputs to give exactly `credits`, with no change output; write the plan into the row (F3), durably; write a move record of the new kind `Register` naming the draft, `pending`, and read it back; only then sign and send. | This is the order every Platform-address move follows (`P/platform_moves.rs:54-57`, `:2441-2500`). Recording the debit as a move gives the registration the existing hold, the nonce settlement, and the rule that a move's own spend is never an `outgoing` activity row (`:21-24`). No change output means nothing comes back to an address, so no `incoming` row needs attributing. |
| F10 | Settlement. **Done:** a proved read shows every input at or past its signed nonce, and a proved fetch shows the planned identity exists. The row adopts that id and moves to `IdentityRegistered`. **Can never execute:** not every input was signed, Platform's structure or signature check refused it, or an input's nonce is taken and the planned identity is proved absent. The move is `failed`, the plan is cleared from the row, and the row may plan again. **Anything else:** `PaymentUnknown` (F7), and the row keeps its plan. A plan whose move is `failed` by height may still execute, so the row re-signs only that plan. | Rule 1. Re-signing the same inputs, nonces, amounts and keys gives the same identity id, so a retry is idempotent. A row imported without its move record follows the same three cases: it can re-sign its plan safely whether or not another install already signed it. |

**Shielded (C4b)**

| # | Decision | Why |
|---|---|---|
| F11 | The identity call is the library's pool-funded create through the dispatch hook, with the keyset entry point UP-1 adds (never the seed path, `PW:1690`). The engine records the hooked `IdentityCreate` artifact (`rust/crates/dw-appdb/src/dispatch.rs:539-545`) and writes the created id into the row when the call returns it. The id is `double_sha256` of the sorted nullifiers (`OPS:2188-2189`), so it exists only once notes are selected. If the hook's registration does not carry it, UP-1 adds it. | The engine holds a key set, never the seed (lane-C §0 finding 1). The id cannot be planned ahead as F9 does, so the artifact is the record of what may have been sent. |
| F12 | Evidence, in this order. The journal's disposition of the artifact (item 0: executed, definitely unsent, unresolved). Then the identity: by the row's stored id, else by the seed's identity at the row's index (`P/registration_flow.rs:1921-1936`). **Executed and an identity of this seed exists at the row's index:** adopt it. **Definitely unsent:** the row returns to `KeysPrepared` and may select notes again. **Unresolved:** `PaymentUnknown`; no second create runs while a spend of the wallet is unrecorded or unresolved. | A second create selects other notes, so it would have another id. Platform refuses a create whose key hashes are already registered (`rs-drive-abci/.../identity_create_from_shielded_pool/state/v1/mod.rs:66-72`), so a duplicate can never make a second identity; F13 covers where its funds go. The existing gate `shielded.unrecorded_spend` (`P/shielded_pool.rs:563`, `:1153`) already blocks the next spend. |
| F13 | The required fallback address is always the wallet's own Platform receive address (`P/platform_addresses.rs:429`), fixed at Draft and stored in the row. It is never typed, pasted or external. If Platform refuses the create for duplicate keys, the denomination less a penalty lands there (`state/v1/mod.rs:72`, `:137`). The engine then adopts the identity that already holds the keys (it is this seed's), and the Platform activity shows the rise as "Returned from a registration", not as a stranger's `incoming`. | iOS passes a required Platform fallback (`ios.md:673`). On desktop the only safe meaning is "back to me". The rise is attributed by the armed-and-absorbed rule that "Add from Dash wallet" uses (`P/platform_moves.rs:24-35`); C1b needs the same rule for Shielded → Platform, and both use one implementation. |
| F14 | The denomination comes from the protocol version in force, never from a constant: `shielded_identity_create_denominations`. At PV12 the set is 0.1, 0.3, 0.5, 1 DASH (`PV/drive_abci_versions/drive_abci_validation_versions/v8.rs:434`, used by `PV/v12.rs`). From PV13 it is 0.03, 0.1, 0.25, 0.5, 1 DASH (`v9.rs:431`, `v10.rs:506`). The engine picks the smallest member that, less the predicted create fee, covers what the name and profile need at that version. An empty set is `platform.feature_off{feature: "shielded_registration"}`. | The library refuses a non-member before proving (`W/shielded/note_selection.rs:230-242`) and refuses a denomination that does not exceed the fee (`:252-259`). Core's amounts are the yardstick: 0.03 DASH for a plain name (`P/registration_flow.rs:79`) and the version's contested amount (`:92-98`). This also follows iOS #1267 (0.03 DASH for a standard name) without hard-coding it. |
| F15 | The change goes back to the pool, to the account's default shielded address (`OPS:2179-2180`, `:2256`). It is not spendable until a sync pass finds it. The quote says so, and the engine counts it as the wallet's the whole time. | The library sets the change address itself; the engine has no choice to make. The copy prevents "my shielded balance dropped by more than the fee". |
| F16 | Readiness is an engine quote, checked again at the start. Items: the pool is active at this version (PV12); the feature is on; a denomination exists (F14); the spendable balance covers it; no unrecorded or unresolved spend; the shielded tree is synced; the notes have rested (F17). The pool's 250-note floor (`minimum_pool_notes_for_outgoing`, `v10.rs:470`) cannot be read ahead, so it is the refusal `shielded.pool_too_small{notes, required}` at the call (`P/shielded_pool.rs:853-854`), which every node raises before execution: nothing moves. | iOS's checklist (`ios.md:37`, `:673`). Every item but the rest is already a refusal of a shielded spend on main; the quote only reports them before Confirm. |
| F17 | Note rest: the source is not ready until 3 hours after the wallet's latest incoming shielded activity (`ShieldedActivity.created_at`, `P/shielded.rs:383-384`). A restored wallet's rows have no time; they count as rested once the shielded sync is complete. The engine enforces it at the start, with no override. **MG to confirm** (M2). | iOS's rule. It is a privacy rule, not a consensus rule: spending a note minutes after shielding it links the two. Using the latest incoming activity is stricter than checking only the selected notes, and needs no library change. |
| F18 | Shielded is never the default (DASHPAY F26, `docs/design/DASHPAY.md:1129`). The picker opens on Dash wallet every time, does not remember the last source, and the Join page's main button stays the Core start. The engine has no default funding. | The decision is already made; this row says where it is enforced. |

**Picker (C4c)**

| # | Decision | Why |
|---|---|---|
| F19 | The Join page keeps "Create a username" as the one main action (Core). A secondary link, "Pay another way", opens the "Pay with" step: Dash wallet (selected), Platform balance, Shielded balance. Each row shows its balance and, when it cannot pay, the one reason. **MG to confirm** (M1). | DP1-09 D1's two-step wizard stays the common path. iOS hides Platform funding behind Advanced mode (`ios.md` PADDR-14); desktop has no such mode, so the link is the equivalent. |
| F20 | The confirm step shows the source, what leaves it, the fee, and what the identity starts with. For Shielded it also shows the denomination and the change line (F15). | DP1-09's confirm shows the lock and fee for Core; the same facts, per source. |
| F21 | Resume follows DP1-09's rule "the engine's rows are the truth". `PaymentUnknown` maps to the progress screen with "Check again". A contested name is allowed from both sources, at the larger amount F14 or F9 gives. | DP1-09 R1–R8 and D4. |

### Kill and resume

The DP1-02 bar: a kill at any transition loses nothing, and a relaunch never pays twice.

| Kill point | Platform addresses | Shielded |
|---|---|---|
| Before the plan or artifact is written | Nothing happened. The row is `KeysPrepared`; a resume plans afresh. | Same. |
| After the record, before a signature | The move record is `pending` and unsigned, so it can never execute and is cleared (`P/platform_moves.rs:61-63`). The row plans again. | The artifact is registered and nothing was sent: item 0's load sort abandons it (A4b, STOP-8). The row returns to `KeysPrepared`. |
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
| Shielded: pool too small | "The shielded pool is too small for this right now. Nothing was paid. Try again later, or pay with your Dash wallet." |
| Confirm, Platform | "N Dash leaves your Platform balance. Your identity starts with about M credits." |
| Confirm, Shielded | "N Dash leaves your Shielded balance. The rest of the notes used returns to your Shielded balance after the next sync." |
| `registration.quote_changed` | "The cost changed. Review it and confirm again. Nothing was paid." |
| `PaymentUnknown` | "We could not confirm the payment yet. Dash Wallet keeps checking; do not start again. Nothing more is needed from you." Button: "Check again". |
| Refund to the Platform balance (F13) | "This wallet already had an identity, so the payment came back to your Platform balance, less a network fee of N." |

### Contract rows (m4)

| Slice | Section | Rows |
|---|---|---|
| C4a | §2.2, §3, §4 | `RegistrationFunding::PlatformAddresses{credits}`; `RegistrationQuote` gains `funding_credits`, `fee_credits`, `available_credits` (0 for other sources; the duff fields are 0 for this one); `RegistrationWait::PaymentUnknown`; `registration.quote_changed`; the start's refusals `credits.move_pending` and `platform.insufficient_credits`; the rule "a planned row re-signs only its plan" (F10). §2.12: the move kind `Register` and its activity row. |
| C4b | §2.2, §2.13, §3, §4 | `RegistrationFunding::Shielded{denomination}`; `RegistrationQuote.readiness` (each item of F16 with its state, and `ready_at` for the rest); `registration.shielded_not_ready{reason, ready_at}`; `platform.feature_off{feature: "shielded_registration"}`; the fallback rule (F13) and the refund's activity row; the evidence order (F12). |
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
- Settlement needs both the nonces and the identity. Mutants: settle on the nonces alone; settle on the library's
  answer without a proved read.
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
| P2 | Order of `contact_pay`, the same as `prepare_contact`: live payee check; the pair's lock; rule 5 on the payee; the source's own gates and quote; the grant's cap; the lease; reserve the address and re-check rule 5 on it; claim the pair; write the payout record durably; sign and send; settle. Every refusal before the reservation takes no address, and every refusal before the lease keeps the grant. | `E/send/contact.rs:117-209` and `:222-268`. The trust gate and its re-check apply to the payee, whatever pays (`:133-137`, `:249-256`). |
| P3 | The source's gates are the ones on main, none relaxed. Shielded: the hook check, the wallet's shielded lock, `confirm_unrecorded`, the quote under the pass hold, the withdrawal range (`P/shielded_pool.rs:781-796`, `:924-963`, `:1150-1154`). Platform: the wallet lock, the proved read under rule 5, `credits.move_pending`, the withdrawal limits (`P/platform_moves.rs:2441-2500`). | A contact payment is an unshield or a withdrawal with a different destination. It must pass everything those pass. |
| P4 | The address comes only from the reservation made inside the same call. No facade call takes a Core address for a contact payout, and `move_to_core` keeps refusing an address that is not the wallet's own. The Platform move gets a new internal target for a contact payout beside the own-address target (`P/platform_moves.rs:2020-2025`), built only by `contact_pay`. | C5b needs an external Core script; lane-C calls it "a new `to` kind". Keeping it internal means the own-address rule of "Move to Dash wallet" is not loosened for any host input. |
| P5 | The pair's reserved address is shared with the Core path: a draft's unused address is the payout's, and the reverse. The payout claims it before it signs. It goes back to the pair only when the transition can never execute: refused before a signature, or refused by Platform's structure, signature or pool-floor check. Otherwise it is consumed, also when the payment is later proved not sent. | DP3-01 D7 and D21. An address that may have reached the network is never paid twice (DIP-15 rotation). Giving it back on a certain refusal avoids a gap in the contact's receiving chain. |
| P6 | One lock per pair, across all three sources. A payout writes a row in a new table `dp_contact_payout` (wallet, identity, contact, source, reference, address, credits, expected duffs, note, state, since), synced as DP3-01 D27 requires, before anything is signed. While its state is `pending` it is the pair's lock in memory, so the existing check (`P/payment_locks.rs:202-221`) refuses every other payment to that contact, from any source. A Core lock likewise refuses a payout. | DP3-01 D23: the row, not memory, is the record of every payment that may have gone out. The existing row is keyed by a Core txid and a payout has none before it executes, so it needs its own key: the shielded activity id, or the move id. |
| P7 | What clears the lock. **Sent:** Platform executed the transition. Shielded: the spend is `Done`, or item 0's disposition is executed. Platform: the move is `done` as itself. **Not sent:** only proof that it can never execute. **Anything else: the lock stays.** The Core arrival of the payout is not waited for. | The lock exists to stop a second payment while the first is unknown (DP3-01 D8, D9). Once Platform has executed the withdrawal the outcome is known: the payout is queued by consensus and is only late. Waiting for the asset unlock would hold the contact for a long time with nothing left to learn. |
| P8 | A Platform payout that is `failed` by height still holds its nonces and may still execute. While it holds, the only move allowed beside it is its exact retry: the same inputs, nonces, amount and address. Any other move is `credits.move_pending`. The lock stays until its nonces are taken. | Rule 1. This is the rule a held send to a Platform address already has (`P/platform_moves.rs:66-74`). Without it a different move at the same nonces could execute instead, and no proof would say which one did (`:87-91`). |
| P9 | A shielded payout whose result is unconfirmed is `pending`, as every shielded spend is (`P/shielded_pool.rs:996-1045`). The contact stays locked until the scan or the journal decides. The user's acknowledgement of an unresolved spend in Tools ▸ Repair (A4c) does not clear the contact's lock. | An acknowledgement is not evidence (rule 2). The exit is real: the nullifiers are seen spent, or the anchor expires with them unspent. |
| P10 | History names the contact from the payout row. `ShieldedActivity` and the Platform activity row gain `contact` (the `Counterparty` record), joined by the row's reference. Without a row (a restore from the seed), the join falls back to the destination address through the existing payee scripts, with their shared-xpub exclusion (`P/attribution.rs:12-14`, `:243-253`). | The wallet does not fund the asset unlock, so the Core rule cannot attribute it. The payout row is the wallet's own durable word; the address fallback is the same "by address first" rule Core uses. |
| P11 | `contact_activity` and the frequent strip include payouts. `ActivityItem` and `PaymentLock` gain `source` (`core`, `shielded`, `platform`) and `reference`. `txid` keeps its type and equals `reference`; a host reads `source` before treating it as a Core txid. `ActivityItem` also gains `core_txid`, set once the wallet has seen the payout on Core. `contact.payment_locked{txid}` carries the same reference. | Additive: no host breaks, and dw-ffi binds no DashPay record (DP1-09 D5). A payout the user just made must show at once, before any Core transaction exists. |
| P12 | The Core arrival is display only. If the wallet reports a transaction that pays the reserved address, the engine matches it by address and amount, fills `core_txid`, and shows no second row for it. If the wallet does not report such transactions, the row stays "Sent" with no Core link. | Whether key-wallet reports an unfunded transaction to a contact's external address is not established at this base; C5a's brief checks it first. Nothing in the money rules depends on the answer. |
| P13 | The amount typed is what leaves the balance for the contact. The confirm shows "They receive about N Dash" from the quote's `expected_duffs`, and the fee. No Max and no "subtract fee" for these sources. **MG to confirm** (M3). | The existing quotes take credits and return the expected payout (`P/shielded_pool.rs:781-796`; `P/platform_moves.rs:171-176`). Solving backwards for an exact received amount would need a new planner for a small gain. |
| P14 | The smallest and largest payment are Platform's withdrawal limits at the version in force (`P/withdrawal.rs:446-453`). Below the minimum the refusal names it and points to the Dash wallet. | A payout is a withdrawal. The limit is consensus, not ours to change. |
| P15 | "Accept and pay" stays one lease and one prompt. `begin_flow(AcceptAndPay)` takes the source; the lease carries the accept's scope plus the one key class the source needs (the shielded spend key, or the Platform-address signer), never the Core spend signer and never both. One `PlatformOp` grant covers the accept's credits plus the payout's total. A plain payout uses a new one-payment flow `ContactPayout` from a `PlatformOp` grant. | DP3-01 D14 and DP3-06 D10. Both the accept and a shielded or Platform spend are charged in credits (`P/shielded_pool.rs:953`), so one credit cap is exact. Partial success is unchanged (DP3-01 D15): "You are now contacts" and the payment's own outcome. |
| P16 | The "From" step defaults to Dash wallet every time and does not remember the last source. A source with nothing to pay with is listed with its reason, not hidden. **MG to confirm** (M4). | The same rule as F18, for the same reason: the private balance is never spent by habit. |
| P17 | `note` is stored in the payout row and shown in history. It is not written to the chain. **MG to confirm** (M5). | An unshield to Core has no memo, and the wallet does not author the asset unlock, so there is nowhere on chain to put it. |

### Outcomes

| Outcome of the call | Lock | Address | What the user sees |
|---|---|---|---|
| Refused before a signature | none written, or its row removed | stays reserved for the pair | The refusal. "Nothing was paid." |
| Refused by Platform's structure, signature or pool-floor check | cleared (not sent) | back to the pair | "Payment not sent." |
| Executed | cleared (sent) | consumed | "Payment sent. It arrives in @bob's wallet once the Dash network processes it." |
| Unknown | held | consumed | "Payment status unknown" and the locked banner. |
| Later proved executed | cleared (sent) | consumed | The banner goes; history shows it sent. |
| Later proved never executed | cleared (not sent) | consumed | "The payment was not sent. You can pay this contact again." |

A restart at any point reads the payout rows back. A `pending` row is the lock again, and the source's own records
(the shielded journal, the move record) settle it as they would a live one.

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
| C5a | §2.5, §2.13, §3, §4, §5 | `contact_pay_quote`, `contact_pay` with `source: shielded`; `ContactPayQuote` and `ContactPayOutcome`; `PaymentLock.source`, `.reference`; `ActivityItem.source`, `.reference`, `.core_txid`; `ShieldedActivity.contact`; `FlowKind::ContactPayout`; `begin_flow(AcceptAndPay)`'s source; the lock rules P6, P7 and P9; the address rule P5. Schema: `dp_contact_payout`. |
| C5b | §2.5, §2.12, §3 | `source: platform`; the contact-payout move kind and its activity row with `contact`; the exact-retry rule P8; the statement that `move_to_core`'s own-address rule is unchanged (P4). |
| C5c | none | Renderer only. |

Whichever of C5a and C5b merges first carries the shared rows (the two calls, the table, the lock and activity fields).

### Tests that prove the money rules

C5a and C5b, each for its source:

- An unknown outcome holds the contact: a second payment from each of the three sources is `contact.payment_locked`.
- The payout row is on disk before a signature is released. Mutant: write it after the call; the kill-before-return
  test must fail.
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
  established and the address reserved; the lease cannot sign a Core spend.

C5b only:

- `move_to_core` still refuses an address that is not the wallet's own. Mutant: route the contact target through it.
- Beside a held payout only its exact retry runs; any other move is `credits.move_pending`. Mutant: allow a move with
  other terms at the same nonces.
- Nonce settlement as "Move to Dash wallet": mutant, settle without the proved nonce.

C5a only:

- The spend goes through the dispatch hook, and the journal's disposition for the kind settles the lock.
- `shielded.unrecorded_spend` refuses the payout unless confirmed, before an address is taken.

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

## MG to confirm

| # | Row | Recommended |
|---|---|---|
| M1 | F19 | The other sources sit behind a "Pay another way" link on the Join page; the main button stays the Core start. |
| M2 | F17 | The 3-hour rest blocks a Shielded-funded registration, with no override, measured from the latest incoming shielded activity. |
| M3 | P13 | The typed amount is what leaves the balance; the confirm shows what the contact receives. No Max for these sources. |
| M4 | P16 | "From" opens on Dash wallet every time and does not remember the last choice. |
| M5 | P17 | The note stays in this wallet's history only. |

## Open risks

1. **C4a's library call.** If the engine cannot reach the SDK's explicit-nonce create and the library's two follow-ups,
   C4a waits for a fork unit UP-2 and RP-1 (F8). It is not on lane-C's schedule today.
2. **The created id from the hook.** C4b needs the pool-funded create's identity id in the artifact or the call's
   return on every path (F11). If UP-1 does not carry it, the fallback is the lookup by the row's index (F12), which
   waits for a discovery pass.
3. **Restore from the seed.** A contact address paid from Platform or Shielded is found used again only if the wallet
   reports transactions to a contact's external address (P12). If it does not, the next payment after such a restore
   may reuse that address. The money still reaches the contact; the rotation is weaker.
4. **Downgrade.** An older build must not pay a contact that a `dp_contact_payout` row locks. C5a's brief checks that
   an older build cannot open the newer app database; if it can, the lock is also mirrored into `dp_payment_lock`.
5. **Two devices on one wallet.** The Platform rules assume one device per wallet, as the moves do today
   (`P/platform_moves.rs:91`). A second device can take a nonce; F10 and P8 then read the proof, never a guess, but a
   payout replaced that way is settled as not sent only once the proof shows it.
6. **The rest rule after a restore** (F17) treats restored notes as rested. A wallet restored minutes after shielding
   can register from Shielded at once.
7. **Size.** C4b is the largest slice in the lane and depends on UP-1, RP-1, A5 and A4b. If the date slips, items 4 and
   5 are the first cut (lane-C schedule).
