# DP6-02: withdraw credits, with a fee reserve on Max

ROADMAP DP6-02 (size M, engine + UI, tier T2, engine review Y / UI N). Acceptance: "Max leaves enough for the network
to accept the withdrawal." Parity: DASHPAY F8 ("withdraw in DP6"; IOS-070, IOS-132), listed as F3 in DP1-10's
deferred table. iOS is the reference for the flow (amount with Max, destination, confirm, progress).

The engine part implements m4 §2.8's `withdraw_quote` and `withdraw` stubs (declared by DP1-06) and adds one read,
`withdrawals`, for the status after broadcast (§3). Every number below is Platform's, read at the engine's pin,
PastaPastaPasta/platform `82fb33aff8` (5.0.0-beta.2, the same revision `rust/Cargo.toml` pins), and taken at run time
from the `PlatformVersion` of the network's protocol version, as the SDK last proved it (the cost table's source,
`credit_costs.rs`). Paths below are under `packages/` of that checkout.

| Part | Where |
|---|---|
| Quote, Max, minimum, Core output, the transition, the status read | `dw-engine/src/platform/withdrawal.rs` (new, not facade) with `withdrawal_tests.rs`; `credits.rs` (facade bodies, `withdrawals`, records) |
| Signing key selection (TRANSFER), the lease | `requests.rs` (`transfer_keys`, `signing_keys` gain `Withdraw`; `write_lease`, `write_permit`, `flow_signer` shared) |
| Contract | `docs/contracts/m4-dashpay-engine.md` §2.8, §3, §4, item 62; Contract-Version 35 |
| App commands, DTOs | `dw-app/src/api/identity.rs`, `dto` |
| dwcli | `identity withdraw --quote-only` (the quote), `identity withdraw`, `identity withdrawals` (D13) |
| UI | `apps/desktop/src/dashpay/withdraw.ts` (view model), `screens/dashpay/WithdrawSheet.tsx`, My Profile's credits block, fixture handlers and scenarios, `e2e/withdraw.spec.ts`, visual baselines of the sheet |

## 1. The fee reserve on Max

Platform checks a withdrawal three times, and Max must pass all three.

1. **Before execution, the estimated fee** (`rs-dpp/.../identity_credit_withdrawal_transition/state_transition_estimated_fee_validation.rs`):
   the identity's balance must be at least `amount + calculate_min_required_fee`, which is
   `fee_version.state_transition_min_fees.credit_withdrawal`: **400,000,000 credits** (0.004 Dash) in every
   `STATE_TRANSITION_MIN_FEES_VERSION1` (`rs-platform-version/src/version/fee/state_transition_min_fees/v1.rs:6`), which
   every fee version up to FEE_VERSION3 (protocol 14 and 15) uses.
2. **After execution, the metered fee** (`rs-drive-abci/.../validate_fees_of_event/v0/mod.rs`, `ExecutionEvent::Paid`):
   the balance after the principal (`balance − amount`) must cover the fee Drive metered: the withdrawal document's
   storage and processing plus `perform_network_threshold_signing` (100,000,000 credits,
   `fee/processing/v1.rs:12`), scaled by `user_fee_increase`. Otherwise the transition is refused
   `IdentityInsufficientBalanceError`.
3. **The amount's range** (Drive's basic structure, `rs-drive-abci/.../identity_credit_withdrawal/structure/`):
   - `structure/v1` (protocol ≤ 13): `min_withdrawal_amount ≤ amount ≤ max_withdrawal_amount`, i.e. 1,000,000 ≤
     amount ≤ 50,000,000,000,000 credits (`system_limits/v2.rs`, `v3.rs`: 1,000 duffs to 500 Dash; 190,000 before
     protocol 12, `v1.rs`).
   - `structure/v2` (protocol 14, 15): additionally `amount ≥ min_withdrawal_amount + core_fee_in_credits(fee rate)`
     (`rs-dpp/src/withdrawal/mod.rs` `min_withdrawal_amount_with_core_fee`), because from 14 the Core fee is carved
     out of the amount: 1,000,000 + 190 × 1 × 1,000 = **1,190,000 credits** at our fee rate.

**The reserve.** `fee_credits = max(min_fee, metered_bound)`, where `min_fee` is dpp's own
`calculate_min_required_fee` for the transition at the network's version (rule 1, read from the version, never a
constant here), and `metered_bound` is an upper bound of rule 2 built like the cost table's document prices
(DP1-06): Drive's estimator of creating the withdrawals contract's `withdrawal` document at the largest sizes, with
the non-exact processing doubled, plus the version's `perform_network_threshold_signing`, plus the identity's balance
and nonce updates priced as one `identity_update` minimum. At every known protocol version the bound is below
`min_fee` (a test holds it), so the reserve is Platform's 400,000,000; the `max` keeps the reserve right if a later
fee version prices the metered part higher. `user_fee_increase` is 0, so nothing scales it. What must fit is Drive's
*estimate* (rule 2 runs `apply_drive_operations` with `apply = false`, `validate_fees_of_event/v0`), not the charge;
the bound's `WITHDRAWAL_IDENTITY_OPS` (20,000,000) for the balance, nonce and system-credit operations is an
unmeasured allowance, and `min_fee` dominates the `max`, so it cannot under-reserve (D25). T2 shows the estimate fits:
Max passed at protocol ≥ 14, and the real charge (balance before − after − amount) was 211,977,820 credits for a
0.2 Dash withdrawal and 194,396,680 for Max, which left 205,603,411 of its 400,000,000 reserve.

**Max** (`WithdrawAmount::All`) takes `balance − fee_credits`, rounded down to whole duffs (a multiple of
`CREDITS_PER_DUFF`, 1,000): Platform turns credits into duffs by truncating division (`rs-dpp/src/identity/
credits_converter.rs`), so the sub-duff remainder would leave the identity and buy nothing. The balance is
Platform's, proved, read afresh by the quote and again by the withdrawal under the identity's write lock. A Max that
falls below the minimum is `platform.insufficient_credits{needed: minimum + fee_credits, available}`: there is
nothing to withdraw.

**The Core output.** What arrives at `to` is Platform's own conversion of the withdrawal document it will store,
`Document::try_into_asset_unlock_base_transaction_info` (`rs-dpp/src/withdrawal/document_try_into_asset_unlock_base_transaction_info/`),
called by the engine on a document built as Drive builds it (`rs-drive/.../identity_credit_withdrawal/v0/transformer.rs`),
stamped as `transform_into_action` 1 stamps it (`rs-drive-abci/.../state_transitions/mod.rs:86`):
- version 0 (protocol ≤ 13): `amount ÷ 1,000` duffs; the Core fee (190 bytes × rate) comes from the credit pool on
  top;
- version 1 (protocol 14, 15): `amount ÷ 1,000 − 190 × rate` duffs (the fee is taken from the amount).

`expected_duffs` is that output's value. **Dust.** Core refuses outputs below its dust threshold (546 duffs for
P2PKH, 540 for P2SH at 3,000 duffs/kB, `rs-dpp/src/withdrawal/core_dust_threshold.rs`), and from protocol 14
Platform fails an expired withdrawal below it (`system_limits/v4.rs`: `core_dust_relay_fee_per_kb`). The minimum
keeps every output above it: at the minimum the output is 1,000 duffs. A test holds this at every version with a
dust rate.

**The fee rate.** `core_fee_per_byte = 1`, dpp's `MIN_CORE_FEE_PER_BYTE` and the SDK's own default
(`rs-sdk/src/platform/transition/withdraw_from_identity.rs`: `core_fee_per_byte.unwrap_or(1)`). Platform requires a
non-zero Fibonacci number (`structure/v1`), capped at 6,765 from protocol 14 (`system_limits/v4.rs`). The asset
unlock transaction is 190 bytes (`ASSET_UNLOCK_TX_SIZE`); 190 duffs is Core's minimum relay fee for it, and a
higher rate only costs the user more (from 14 it comes out of their amount).

**Pooling.** `Pooling::Never`. It is the only value Platform accepts: `structure/v1` refuses any other with
`NotImplementedCreditWithdrawalTransitionPoolingError`, and Drive stores `Never` whatever was sent
(`transformer.rs`). It is also the SDK's choice.

## 2. Destination and signing

- **Destination.** The default is the wallet's current receive address (`current_receive_address`, as Receive shows
  it), so the payout lands in this wallet's history. The user may paste another Dash address; the UI checks it with
  `classify_address` for the wallet's network, and the engine parses it again: an address of another network, or
  anything but P2PKH or P2SH (Platform's `InvalidCreditWithdrawalTransitionOutputScriptError`, `structure/v1`), is
  `invalid_argument`. The transition always carries the output script, so Drive never derives one from a key
  (`transformer.rs`, the `None` branch).
- **Key.** Platform signs a withdrawal with a `TRANSFER` or `OWNER` key at `CRITICAL`
  (`identity_credit_withdrawal_transition/v1/identity_signed.rs`), and refuses an output script signed with an
  `OWNER` key (`signature_purpose_matches_requirements/v0`). A DashPay identity has no OWNER key; the wallet
  registers key 3 as `TRANSFER`, `CRITICAL`, ECDSA (`keys_policy.rs`). The engine signs with the first enabled
  `TRANSFER` `CRITICAL` ECDSA key the vault holds, passed to dpp explicitly; none is `identity.keys_missing`.
- **Construction.** dpp's `IdentityCreditWithdrawalTransition::try_from_identity` with exactly the SDK's parameters
  (`withdraw_from_identity.rs`): the identity, `Some(script)`, the amount, `Pooling::Never`, rate 1,
  `user_fee_increase` 0, the signer, `Some(key)`, `TransferPreferred`, the nonce, the SDK's version. The nonce is
  the engine's, never the SDK's cache's (D24): an unconfirmed withdrawal's while Platform would take it, else one
  past Platform's proved nonce. The engine builds and broadcasts it itself rather than through
  platform-wallet's `withdraw_credits_with_external_signer`, which hides the nonce and output script the status read
  needs (§3). A test holds the bytes the engine signs, through the vault's identity signer under a `Withdraw` lease,
  equal to dpp's `try_from_identity` with the SDK's choices called with a second signer of the same vault
  (`lease.identity_signer(&[0])`) and no key given (`None`, so dpp picks the TRANSFER key itself); ECDSA is
  deterministic, so the bytes match, and the signature is verified on its own with key 3's public key, which the test
  checks is TRANSFER, CRITICAL and ECDSA.
- **Grant.** `withdraw_quote`'s grant is `PlatformOp{max_duffs: 0, max_credits: credits + fee_credits}`: everything
  that may leave the identity. The withdrawal runs under a `Withdraw` lease, redeemed while the vault holds the
  TRANSFER key, and one call permit charged that amount from the lease's Credits budget (requests.rs's
  `write_lease`/`write_permit`).
- **Gates, in order, before the grant is redeemed:** the identity is the wallet's (not watched: `invalid_argument`);
  the money gate (DP3-04 rule 5, `Subject::Identity`, D6 of DP3-04: the identity and its keys, as for a top-up); the
  identity's write lock; the quote again on a fresh proved balance; the grant's cap.

## 3. Status after broadcast

The SDK broadcasts and waits for Platform's proved result (`broadcast_and_wait_for_affected_state`). That result is
the identity's balance, proved, after the wait: for a withdrawal Drive proves the balance only
(`verify_state_transition_was_executed_with_proof/v0`), and rs-sdk documents it as "NOT evidence that this specific
transition" executed (`broadcast.rs:73`). So the engine counts the withdrawal as **executed** only when the proved
balance is at least the amount below the one it read under the identity's lock, or when the withdrawal's document
reads back with the amount (D24); otherwise the outcome is unknown. Executed means the credits left the identity and
Platform queued a `withdrawal` document of the withdrawals contract. The Core payout comes later: Platform pools
queued documents (`withdrawal_transactions_per_block_limit` 4), a quorum signs the asset unlock transaction, Core
mines it, and Platform marks the document `COMPLETE`.

What the engine observes:
- **Proved means checked against the quorum keys the context provider serves** (DP3-04): how far those keys are
  trusted is the trust mode's, not this flow's. On T2's devnet they come from the trusted service alone (D15).
- **Platform's document.** Its id is deterministic (`Document::generate_document_id_v0(withdrawals contract,
  identity, "withdrawal", nonce ‖ output script)`, `transformer.rs`), so the outcome names it. The new read
  `withdrawals(identity)` lists the identity's documents, proved, newest first (index `identityRecent`: `$ownerId`,
  `$updatedAt`), at most 20, read oldest first in pages and the newest kept (D16): id, credits, the Core output as Platform computes it, the destination address, status,
  and the times. Status is the contract's: `Queued` (0), `Pooled` (1), `Broadcasted` (2), `Complete` (3),
  `Expired` (4: Core did not mine the signed transaction in time; Platform re-signs it), `Failed` (5, protocol 14+:
  terminal, the payout was below dust; the credits are not returned, `withdrawals-contract/src/lib.rs`). A value
  this build does not know reads `Unknown`.
- **The Core payment.** With the default destination, SPV sees the asset unlock transaction pay the wallet's
  receive address, and it shows in History like any incoming payment.

What the UI shows: the sheet's result ("Withdrawal queued: N Dash to yX…; Dash Platform sends it to the Dash chain,
usually within minutes, or hours later when Platform's daily withdrawal limit is reached": Platform pools queued
documents only up to its daily limit, `rs-dpp/src/withdrawal/daily_withdrawal_limit`, applied in
`pool_withdrawals_into_transactions_queue/v1`, so a large withdrawal can stay Queued for hours), and My Profile lists the identity's recent withdrawals under the credits with
their state: "Pending" (Queued, Pooled, Broadcasted, Expired), "Sent to Core" (Complete), "Failed" (Failed),
re-read on every `Platform` signal and every 60 s while one is pending.

**Refused and stuck**, as codes:

| Case | Code |
|---|---|
| Platform refused the transition at execution: balance short (rule 1 or 2) | `platform.insufficient_credits{needed, available}` (the mapping of Platform's balance error, unchanged), when the document does not read back (D21). A CheckTx refusal is read back as an unknown outcome (D21). |
| the amount outside Platform's range (rule 3), explicit amounts only | `credits.withdrawal_out_of_range{min, max}` (credits) — **new** |
| no TRANSFER key the vault holds | `identity.keys_missing{purpose: Transfer}` |
| any other consensus refusal of the executed transition | the existing `From<dash_sdk::Error>` mapping (`internal{detail}` for one with no code) |
| broadcast with no answer, and the document is not on Platform when read back | `platform.broadcast_unknown{artifact}`, `artifact` the withdrawal document's id (Base58), the id `withdrawals` lists it under once it executes. The UI never offers a retry: "It may have been sent. Check your withdrawals in My Profile before trying again.", shows the document id, and keeps the withdrawal "not on Dash Platform yet" on My Profile until `withdrawals` lists it (D23). |
| the broadcast failed, was refused, or answered a balance that did not drop by the amount, and the read-back finds the document with the amount | `Ok` with no remaining balance, as a write that landed (D24) |
| a proved balance that did not drop by the amount, and no such document | `platform.broadcast_unknown{artifact}`, as an unanswered broadcast (D24) |
| stuck on Core (Queued/Pooled/Broadcasted for long, Expired) | not an error: `withdrawals` shows it pending; Platform keeps re-signing it. |
| Failed (dust, protocol 14+) | `withdrawals` row `Failed`; the UI says the credits were not returned. Our minimum makes this unreachable for our own withdrawals. |

## 4. Errors and the contract delta (Contract-Version 34 → 35, item 62)

- `withdraw_quote(identity, amount)`: implemented. Refuses an identity the wallet lacks (`identity.not_found`), a
  watched one (`invalid_argument`), an explicit amount outside `[min, max]` (`credits.withdrawal_out_of_range`), an
  amount whose total exceeds the balance (`platform.insufficient_credits`), and Max with nothing to take (the same).
- `withdraw(identity, to, amount, grant)`: implemented. The quote's refusals under the write lock, then `to`
  (`invalid_argument`), the money gate (`platform.context_unavailable`, `platform.trust_mismatch`), the key
  (`identity.keys_missing`), the grant (`platform.grant_exceeded`, `needs_grant`, `grant_invalid`). The outcome:
  `WithdrawOutcome{credits, expected_duffs, remaining_credits, withdrawal}` — **`withdrawal` is new**: the document
  id (base58). `expected_duffs` is the output Platform computes for the real script.
- `withdrawals(identity)` **new**, async, `CreditsError`: `Vec<Withdrawal{id, credits, duffs, to, status,
  created_at, updated_at}>`.
- `WithdrawalStatus` new: `Queued | Pooled | Broadcasted | Complete | Expired | Failed | Unknown`.
- `CreditsError::WithdrawalOutOfRange{min, max}` → `credits.withdrawal_out_of_range` **new §4 code**. The Swift
  code tables (`ServiceErrorCode.engineCodes`, `m2EngineCodes`, `m3EngineCodes`) mirror the m1–m3 tables only; no
  Swift table reads m4's §4, so none gains it (D14). The Swift T0 suite runs anyway (`scripts/linux-docker-test.sh`).
- `IdentityError::KeysMissing` gains no variant: `KeyPurpose::Transfer` exists.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The reserve is `max(dpp's min required fee, a metered bound)`, at the network's protocol version; the metered bound is the cost table's method on the withdrawal document plus threshold signing plus one identity update minimum. | Rule 1 is Platform's own number; rule 2 is metered, so only a bound can cover it. A test shows the bound stays under the minimum at every known version, so the reserve equals Platform's figure, and still follows a fee version that changes either part. |
| D2 | Max rounds down to whole duffs. | Truncating division would burn up to 999 credits for nothing. |
| D3 | `core_fee_per_byte = 1`, `Pooling::Never`, `user_fee_increase = 0`. | The SDK's parameters; Never is the only pooling Platform accepts; rate 1 is Core's minimum relay rate for the 190-byte unlock and, from protocol 14, the user pays it out of the amount. |
| D4 | The engine builds and broadcasts the transition itself (dpp `try_from_identity` + the SDK's broadcast), not platform-wallet's withdraw. | The status read needs the nonce and the output script to name the document; the library's call returns neither. Same SDK calls underneath. |
| D5 | One new error code, `credits.withdrawal_out_of_range{min, max}`, in credits. A bad destination is `invalid_argument` (the UI validates it first with `classify_address`, as Send does). | `credits.below_minimum{min}` is in duffs for top-ups (and the binding names its field `duffs`); a second unit on one code would mislead. A typed destination code would add surface for a case the UI already prevents. |
| D6 | The withdrawal is gated by DP3-04's money gate with `Subject::Identity`, like a top-up. | DP3-04 D6; the Platform data that decides where the credits go is the identity and its keys. |
| D7 | An unanswered broadcast is read back by the document id; if absent, `platform.broadcast_unknown{document id}` and no retry is offered. | The document id gives the user a way to see it land. A later withdrawal of the session cannot execute beside it (D24), but nothing retries on the user's behalf. |
| D8 | Status comes from Platform's withdrawal documents (`withdrawals`), not from a local table. | Platform's record is authoritative and proved, survives reinstalls, and shows withdrawals made by another device of the same identity. |
| D9 | Withdraw sits in My Profile's credits block beside Top up (DASHPAY §4 `CreditsViewModel`: top-up and withdraw together; iOS keeps both in the identity's credits section). The sheet is a separate flow (`withdraw.ts`), not a mode of the top-up flow. | §4 IA; the two flows share nothing but the balance. |
| D10 | The sheet's amount is in Dash (the display unit) with a Max toggle; credits stay internal, converted at `creditsPerDuff`. | As the top-up sheet (DP1-10 D6). |
| D11 | Recent withdrawals show on My Profile (at most 5 there), re-read on `Platform` signals and every 60 s while any is pending. | "Pending/complete" must be visible after the sheet closes; Platform makes no event for a document status change. |
| D12 | T2 records what it sees. Expected: Platform's acceptance and the balance moving, with Core completion possibly missing on a 3-masternode devnet. Seen: both asset unlocks were quorum-signed, mined and chain-locked, both documents went `complete`, and the wallet's SPV received both payouts (`/work/scratch/dw-dp6-02/t2/EVIDENCE.md`). | The brief's devnet facts. |
| D13 | dwcli keeps the stub's command shape: the brief's `identity withdraw-quote` is `identity withdraw --quote-only`, as `identity top-up --quote-only`; `identity withdrawals` is new. | The m4 stubs already had `identity withdraw` with `--quote-only`; a second command for the quote would duplicate its arguments. |
| D14 | No Swift `engineCodes` change: Swift's code tables (`engineCodes`, `m2EngineCodes`, `m3EngineCodes`) mirror the m1–m3 contracts and no Swift test reads m4's §4. The Swift T0 suite ran anyway: 568 tests in 73 suites passed (1 known issue). | dw-common's rule is for a table Swift mirrors; checked rather than assumed. |
| D15 | T2's identity was registered by an uncommitted harness (`/work/scratch/dw-dp6-02/t2/harness/withdraw_live.rs`, DP6-01's pattern: keys 0–3 from the wallet's own seed, a chain asset lock from the seed node), and T2 ran with the developer toggle "trusted quorums only" set in app.sqlite (dwcli has no command for it). Everything after registration went through dwcli. | On this devnet dash-spv never gets the masternode list (no dip0024 quorum), so the engine holds a registration's funding and DP3-04's enforcing gate holds every write; the toggle is the product's own way to trust the quorum service on a devnet. |
| D16 | `withdrawals` reads the identity's documents oldest first, 100 a page, and keeps the newest 20, not one descending query. | At the pin a descending `$updatedAt` query's proof fails verification on the devnet ("Proof op family does not match the query direction: inverted op in a left-to-right walk", T2 step 4). Only the owner creates its withdrawals, so the pages end. A Platform follow-up, not this flow's to fix. |
| D17 | The rules, the payout and the documents' payouts are read at the SDK's protocol version after a proved read (the balance, the documents), never before. | The SDK seeds regtest (and mainnet, testnet) at protocol 13 and ratchets up only once a proof verifies. T2 step 7 showed a fresh process quoting protocol 13's minimum on a protocol 14+ network. A test (`the_rules_are_the_proved_versions_not_the_seed`) holds the order and kills mutant R2. |
| D18 | Visual baselines: three new shots of the withdraw sheet (form, confirm, done) in light and dark. The existing My Profile shots that show the credits block change only by the Withdraw button beside Top up; those are re-baselined, nothing else. | The brief limits baselines to the new sheet; the button is the one change to the page. |
| D19 | The fixture's withdrawal handlers and state live in `fixture/topup.ts` (the credits module, `CreditsWorld`), and the UI's queries in a new `dashpay/withdrawQueries.ts`. | Shares the identity's balance with the top-up handlers; keeps `fixture.test.ts`'s module list and `profileQueries.ts` untouched. |
| D20 | My Profile disables Withdraw while the balance is unknown or zero, and while a withdrawal runs ("Withdrawing…"); the recent withdrawals sit under the top-up rows, "Pending" for Queued, Pooled, Broadcasted and Expired. | §3: Platform keeps re-signing an expired payout; only Complete and Failed end. |
| D21 | A broadcast's failure is a refusal (nothing happened, the grant's quote kept) only when the wait's answer carries Platform's consensus error. A broadcast error without a cause (DAPI answers its own wait timeout, or a dropped Tenderdash subscription, that way), any transport error and a CheckTx consensus error are unknown outcomes: read back by the document id, else `platform.broadcast_unknown`. `settled()` holds the rule; `only_a_consensus_refusal_says_nothing_happened` tests it. | Review r1 M1: the earlier rule took every broadcast error as a refusal, so a timed-out wait offered the user a second withdrawal while the first could still execute. CheckTx stays unknown (review m1 declined): the SDK resends to other nodes, and a later send's refusal (the nonce used, the credits gone) can follow a first send Platform took. The cost is a CheckTx refusal reading "It may have been sent". **Trust (review r1 m3):** "refused" rests on one DAPI node's unproved answer. An honest rs-dapi fills the cause only from a committed transaction's result with a non-zero code (`wait_for_state_transition_result.rs`; every other error, its own timeout included, goes out without one), so for it `cause: Some` means "in a block, failed, final"; a lying node can invent a cause while the transition sits in a mempool. Two things make that lie harmless: a refusal is read back by the document id like an unknown outcome, so one that executed is reported done (`a_refusal_with_the_document_on_platform_is_done`); and the refused withdrawal's nonce stays the session's unconfirmed one, so a withdrawal the user makes next signs at it while Platform would take it, and at most one of the two executes (D24). |
| D22 | `withdrawals` keeps one row per document id, its newest; the paging window is a pure `newest()` over a page reader, tested at 0, 1, 20, 99, 100, 101, 200 and 250 documents. Max is held to the quote's grant by `write_lease` (credits arriving after the quote refuse it as `platform.grant_exceeded`, unredeemed; tested). | Review r1 m3, m5: a document updated between two page reads comes twice; the window had no test. |
| D23 | After `platform.broadcast_unknown`, the flow keeps the document id for the identity while the app runs: the refusal shows it, My Profile lists "Withdrawal not on Dash Platform yet" with the id, read every minute until `withdrawals` lists that id, and the form says what is guaranteed: "A new withdrawal now takes its place: only one of the two can go through." Withdraw stays enabled. Confirm is off while the quote is refetching or its refetch failed. **Guaranteed (D24):** in the session, a withdrawal made after an unconfirmed one signs at its nonce while Platform's nonce rule would still take it, so at most one of the two executes; once Platform could no longer take it (its nonce used or 24 behind), it can no longer execute, and the next withdrawal takes a new nonce. After a restart the id and the session's nonce are gone; the next withdrawal signs one past Platform's proved nonce, which is the pending one's own nonce unless another identity-nonce write (a key update) executed after it (D24's residual). | Review r1 m2, m4: `withdrawals` lists executed documents only, so a mempool transition was invisible and invited a retry. A lock-out could hold forever for a transition that never got in; a warning cannot. Review r1 m1 (manager ruling): the warning became a guarantee. The id is not persisted across restarts (follow-up). Accepted limits (review r2): after a real CheckTx refusal (D21) nothing ever lists the id, so the row, the warning and the minute's read last until the app restarts; and a landed document more than 20 updates back falls out of the read window and shows as unconfirmed again. The form reads the list itself only while one is unconfirmed. |
| D24 | **The nonce and what counts as executed (review r1 m1, m2; manager ruling).** The withdrawal's nonce never comes from the SDK's cache. The engine reads Platform's nonce, proved (`IdentityNonceFetcher`, rs-sdk `platform/fetch.rs:319`: the stored value with Drive's missing-revision bits), and keeps per session and identity (`NetworkSession::withdrawal_nonces`) the nonce of the last withdrawal signed and not seen executed, and a floor past every one seen executed. The next withdrawal signs at the unconfirmed nonce while `validate_identity_nonce_update(proved, k)` (rs-dpp `identity/identity_nonce.rs:99-160`) holds: k unused and within 24 of Platform's newest, above it or a missing revision below it; else at `max(proved & IDENTITY_NONCE_VALUE_FILTER + 1, floor)`. A withdrawal counts as executed only on a proved balance at least the amount below the one read under the identity's lock, or the document `document_id(owner, k, script)` read back with the amount (the amount is not in the id); then the floor moves past k. Any other answer leaves k unconfirmed, a refusal included (D21). After signing, the SDK's identity-nonce cache is stepped once (`get_identity_nonce(owner, true)`), so its other users (key updates) take a nonce past k. Tests: `a_retry_after_an_unknown_outcome_cannot_execute_twice` (fails on ae89f4de: both execute, backport in `/work/scratch/dw-dp6-02/mut/backport-ae89f4de.diff`), `a_node_behind_a_seen_withdrawal_does_not_bring_its_nonce_back`, `a_success_without_the_balance_drop_is_unconfirmed`, `a_document_of_another_amount_is_not_this_withdrawal`, `a_refusal_with_the_document_on_platform_is_done`; mutants A6MUT 6–11 (the cache's nonce, no reuse, any `Ok`, any amount, no floor, no read-back on a refusal) each killed. | **Why not the cache:** rs-sdk's `get_or_fetch_nonce` (`internal_cache/mod.rs:265-360`) merges `max(cached, platform)` and bumps (`:323-343`), and `refresh_identity_nonce` (`sdk.rs:628`, which the broadcast step calls on an error, `broadcast.rs:166`) only marks the entry stale, so after an unknown outcome at N+1 the session's next withdrawal signed N+2, and Drive's rule lets both execute. **Why reuse and not only a proved read:** proved+1 alone is the pending nonce only while nothing else used the identity nonce; a key update executing after it leaves the pending nonce a valid missing revision, and proved+1 would sign a third nonce beside it (the test's `other_write`). **Why the floor:** the SDK accepts a proof up to `metadata_height_tolerance` blocks old (`sdk.rs:248`, `verify_metadata_height` `:746`), so a node one block behind a withdrawal just seen executed answers its nonce as unused; signing at it would be refused and the read-back would find the earlier document, a false "done" for a withdrawal that never ran. **Why the balance drop:** for a withdrawal Drive proves the identity's balance only (rs-drive `verify_state_transition_was_executed_with_proof/v0` `:1381-1403`, not execution-proved, `:3176`), and rs-sdk calls the result "NOT evidence that this specific transition" executed (`broadcast.rs:73`); one node can answer the unchanged balance. Credits arriving meanwhile only send a real success to the read-back. **Residual:** the state is per session; after a restart (the D23 marker is gone too) the next withdrawal signs at proved+1, which equals the pending nonce except when another identity-nonce write executed after it. A node behind the chain can also make a withdrawal refuse with a nonce error; that is safe. |
| D25 | The review's remaining nits, recorded and left (manager ruling): **n1** the metered bound's identity-operations allowance (`WITHDRAWAL_IDENTITY_OPS`, 20,000,000) is unmeasured, and what must fit is Drive's estimate, not the charge; §1 says so. **n4** `newest()` pages with `StartAfter(last id)` on `$updatedAt` ascending: a page's last document updated between two page reads moves to the end, and the next page can skip the documents between. Harmless: it needs more than 100 withdrawals and a status change at that instant, it only hides rows (never invents or moves money), and the next read (every 60 s while one is pending, and on each `Platform` signal) starts over. Every poll re-reads every page, O(n/100) proved queries a minute while one is pending; a descending query replaces it once its proof verifies (D16, follow-up). **n6** after a genuine CheckTx refusal Tenderdash caches the refused transaction's hash, so after a restart an identical retry (same nonce, amount and address) is refused as cached and reads `broadcast_unknown` again; harmless, and if D23 ever persists the id its copy should say to change the amount or address. **n5** fixed: a document whose amount does not read lists as "an unknown amount" (`Withdrawal.credits` is `Option`, `null` to the host), not 0. **n2**, **n3** fixed in §2 and §3. | The review rated them nits; none moves money. |
