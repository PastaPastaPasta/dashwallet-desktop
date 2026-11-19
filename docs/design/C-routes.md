# C-routes: Platform ↔ Shielded, Core Max drains, top-up from Platform or Shielded, credits to Platform

Design notes D-C1, D-C2, D-C3 and D-C6 for PLAN-1.0 lane C (`docs/PLAN-1.0.md`; scoping in the lane-C brief). They
settle the questions each engine slice needs answered before it starts: C1a–C1c, C2a–C2c, C3a–C3c, C6a–C6b. D-C4
(registration from Platform or Shielded) and D-C5 (pay a contact from any balance) are separate notes.

Cites are at dashwallet-desktop `main` 31c35e77 unless marked. Abbreviations:

- `E/` = `rust/crates/dw-engine/src/`, `P/` = `E/platform/`, `UI/` = `apps/desktop/src/`, m4 =
  `docs/contracts/m4-dashpay-engine.md` (Contract-Version 65 at this base).
- Library: the pinned fork PastaPastaPasta/platform 72d0a3e7 (`rust/Cargo.toml:93-112`). `PW` =
  `packages/rs-platform-wallet/src/wallet/platform_wallet.rs`, `OPS` = `…/wallet/shielded/operations.rs`, `DPP` =
  `packages/rs-dpp/src/`, `VER` = `packages/rs-drive/src/verify/state_transition/verify_state_transition_was_executed_with_proof/v0/mod.rs`,
  `PVF` = `packages/rs-platform-version/src/version/feature_initial_protocol_versions.rs`.
- key-wallet: rust-dashcore 8e7ad2c1 (the workspace pin), `KW` = `key-wallet/src/wallet/managed_wallet_info/asset_lock_builder.rs`.

## 0. Rules every route below follows

These are the safety patterns on `main`. Every decision row below keeps all of them; none is relaxed.

| # | Rule | Where it is on main |
|---|---|---|
| R1 | **Record before signing.** A Platform-address debit is written durably (`pending`, able to land, inputs and the nonces it signs at) and read back before any key signs; only a definitive refusal clears it. | `P/platform_moves.rs:54-81` (module doc), `MoveRecord` `:163`, `run` `:2441`; m4 §2.12 `move_to_core` (m4:591) |
| R2 | **Settle on proof, never on the call's answer.** Address debits settle by the inputs' proved nonces; identity debits by DP6-02 D24's evidence; shielded spends by the hook's artifact and the scan. A proved result whose transition kind does not bind execution (`VER:3176-3240`, `binds = false`) is a snapshot, not evidence. | `P/platform_moves.rs:907` (`settle`); DP6-02 D24; m4 §2.13 "Unconfirmed spends" (m4:663) |
| R3 | **Unknown outcome moves nothing more.** An unknown outcome holds what it may still spend (`credits.move_pending`, the shielded notes, the identity's unconfirmed nonce) and no host offers a retry. | m4 §2.12 `platform_send` (m4:596), §4.1 `dispatch_status`; DP6-02 D23 |
| R4 | **Rule 5 (the money gate).** An identity write passes `money_gate(Subject::Identity)`; an address read is repeated under `trust::scoped` until its proofs used only SPV-verified quorum keys. Both before the grant is redeemed, so a refusal keeps the grant. | `P/withdrawal.rs:750`, `P/topup.rs:435`, `P/trust.rs:545`; m4 item 55 |
| R5 | **Leases and grants.** One lease per call, begun from the quote's `GrantRequest`; a lease of another flow is `platform.grant_invalid`; a refusal before the redeem keeps the grant. | `P/flows.rs:21-85`, `P/requests.rs:219` (`write_lease`), `:487` (`write_permit`); `P/shielded_pool.rs:812` (`authority`) |
| R6 | **The dispatch hook.** Every shielded note spend is stored unsent, its artifact (kind, `st_hash`, nullifiers, `identity_id` for top-ups) written to `dispatch.sqlite` before its first broadcast; without the hook nothing is sent. | m4 §2.13 "The dispatch hook" (m4:686), item 87 (m4:4246); `rust/crates/dw-appdb/src/dispatch.rs:370-385`, `:539-564` |
| R7 | **Attribution.** A debit or credit the wallet made is never an `outgoing`/`incoming` row: a pending move's inputs are not compared; an armed fund absorbs its address's rise up to its credits; a record closed without landing gives what it absorbed back as `incoming`. | `P/platform_moves.rs:14-50` (module doc), `hold_fund` `:495`, `Arming` `:545`; `P/platform_fund.rs:1-45` |

**Lock order.** Where a call takes more than one of the engine's per-wallet locks it takes them in this order, and no
existing path takes them the other way round: the identity's write lock (`identity_writes::identity_locks`), then
the shielded wallet lock (`P/shielded_pool.rs:401`), then the Platform move lock (`P/platform_moves.rs:1562`). The
fund lock (`P/platform_fund.rs:375`) stays before the move lock, as now (`hold_fund`, `P/platform_moves.rs:495`). A
test per slice (`lock_order_*`) runs the new call against the existing holders of the later locks and must not
deadlock.

**Build the transition at the recorded nonces.** The library calls lane C first named for three routes hide the
nonces they sign at (`PW:519` `top_up_from_addresses` and `PW:600` `transfer_credits_to_addresses_with_external_signer`
fetch them inside rs-sdk, `identity/network/top_up_from_addresses.rs:54-90`). R1 and R2 need them before signing.
So, as DP6-02 D4 did for withdrawals and as moves do with `ExplicitWithNonces` (`P/platform_moves.rs:1493`), the
engine builds those transitions itself with dpp's constructors at the nonces it recorded and broadcasts them with
the SDK:
- `IdentityTopUpFromAddressesTransition::try_from_inputs_with_signer(identity, inputs_with_nonce, …)`
  (`DPP/state_transition/state_transitions/identity/identity_topup_from_addresses_transition/methods/mod.rs:33`);
- `IdentityCreditTransferToAddressesTransition::try_from_identity(identity, recipients, …, nonce, …)`
  (`…/identity_credit_transfer_to_addresses_transition/methods/mod.rs:29`).

The library then does not reconcile those addresses itself. Its next sync pass does, and every engine decision
reads Platform anyway. The shield from Platform is the exception (D-C1 row 3).

**Contract versions.** Every engine slice below changes m4 once. #49 takes 66 and A5 takes 67, so the next free
version is 68. Each slice takes the next free version when it merges (rebase and re-bless
`E/tests/m4_dashpay_contract.rs`), and adds the next free m4 §7 item.

## D-C1: Platform ↔ Shielded (ROUTE-11, ROUTE-14; slices C1a, C1b, C1c)

Neither direction exists. Transfer answers `"unsupported"` (`UI/transfer/model.ts:31-35`, copy `:59`). The library
offers the shield as `shielded_shield_from_account` (`PW:1872`) with its Max planner `shielded_shield_preflight`
(`PW:1837`). The unshield to a Platform address is `shielded_unshield_to` (`PW:1496`). That call takes the seed and
is refused once the dispatch hook is set (`OPS:1580-1596`, `refuse_store_recording_without_hook` `OPS:3167`), so
C1b waits for UP-1's keyset entry point and RP-1.

### Decisions

| # | Decision | Why |
|---|---|---|
| C1-1 | **(a) Own addresses only, both ways, for 1.0.** Platform→Shielded pays the wallet's own shielded account 0 (the library's default note address, `PW:1872`). Shielded→Platform pays an address in the wallet's own payment-account pool (default: `platform_receive_address`, m4 §2.12); any other `to` is `shielded.invalid_address{detail: "not this wallet's Platform address"}`. External recipients are not offered. **MG to confirm.** | iOS allows external (ROUTE-11/14, `docs/research/gap-2026-10-31/ios.md:710`, `:713`). But each pool already pays strangers on its own: `platform_send` with operation-bound grants (m4:596) and `shielded_send` (m4:824). A cross-pool payment to a stranger would be a third "pays someone else" path. It would need `platform_send`'s operation, retry and `may_land` rules over two settlement models. Own-only keeps every outcome inside the user's balances (R3 worst case: credits sit in the wrong one of their own pools). |
| C1-2 | **Facade placement.** Platform→Shielded is `platform_shield_quote(amount)` / `platform_shield(amount, grant)` in `platform_addresses.rs` (`CreditsError`), because its money, hold, record and settlement are `platform_moves`'. Shielded→Platform is `unshield_to_platform_quote(credits, to)` / `unshield_to_platform(credits, to, grant, confirm_unrecorded)` in `shielded.rs` (`ShieldedError`), because it is a hooked note spend like `unshield_to_core` (`P/shielded.rs:569`). `CreditsError` gains one transparent arm, `Shielded(ShieldedError)`, which carries the pool's gate (`shielded.not_active`) and key state (`shielded.not_synced{account}`) with their codes unchanged. | Each call reports the codes of the side whose state decides it. The transparent arm adds no code (`P/errors.rs` `ShieldedError::Platform` is the same pattern the other way round). |
| C1-3 | **Platform→Shielded uses the library's `shield_to` with the engine's inputs.** Under the move lock (R1), the engine takes one proved read with rule 5, plans the inputs, and records a `MoveRecord` with the new `MoveKind::ToShielded`: the inputs, each at proved nonce + 1, the claim per input, `credits`, and the protocol version. It then calls `operations::shield_to` (`OPS:569`, public) with exactly those inputs, the bound viewing keys, the account's own address (`recipient: None`) and a `VaultPlatformAddressSigner` that refuses any address outside the plan. Not `shielded_shield_from_account` (`PW:1872`): that plans on the library's cached balances (`PW:1770-1818`), and rule 5 needs a proved read. Not a dpp-built transition either. | `shield_to` writes the live `Shield` row whose id the scan's OVK recovery matches (`OPS:677-693`). A dpp-built shield would leave only the scan-derived row, which reads as `Received` (`P/shielded.rs:325-327`), so attribution would get worse. The cost: `shield_to` fetches the nonces itself (`fetch_inputs_with_nonce`, `OPS:624-636`). Under the move lock with no move held, those are the recorded ones unless another device used an input in between. That is m4's accepted one-device assumption (m4:602-606): a nonce another device took reads as this move done. A node behind gives a used nonce, which Drive refuses, and the record settles `failed` by height. Platform enforces the balances, so nothing is spent twice. Follow-up: an explicit-nonce `shield_to` in UP-1 would close the gap (row C1-10). |
| C1-4 | **The plan and Max.** The engine mirrors the library's shield planner (`plan_shield_inputs`, `PW:177`, private). Candidates are funded addresses, sorted as `PlatformAddress` orders them. The fee reserve `shield_fee_reserve_credits(version)` (`OPS:120`, public; twice the flat shielded fee) stays on input 0, because `shield_to` claims the flat fee on input 0 and deducts the metered fee there (`reserve_shield_fee_on_input_0` `OPS:611`, `DeductFromInput(0)` `:639`). Later inputs below `min_input_amount` are left out, and at most `max_address_inputs` are taken. `All` takes `usable − reserve`. The quote is `PlatformShieldQuote{credits (the note's value), pool_fee_credits (the flat fee), fee_reserve_credits, debit_credits = credits + pool fee + reserve, inputs, grant (max_credits = debit_credits)}`. | Input 0 must be the BTreeMap-first address for the library's fee placement, so the engine cannot use the move planner's largest-first fee input (`P/platform_moves.rs:1215`). Mirroring keeps "quote and Max equal the preflight" true, which a test holds. |
| C1-5 | **The fee is no ceiling.** Platform deducts the metered fee from input 0's remainder at execution with no cap. The quote shows it as "about", and the grant consents to the planned debit only. | m4 §2.12 round-3 ruling (m4:626) for the same deduction rule; the reserve is the library's own band. |
| C1-6 | **Gates, in order, before the grant is redeemed.** The shielded gate (PV ≥ 12, `shielded.not_active`; mainnet is at 13, ROADMAP §7.3); address funds (PV ≥ 11, `PVF:3`) are implied. The account's viewing keys must be bound (`shielded.not_synced{account}`). Then the move lock's read with rule 5 (`platform.context_unavailable`), the hold (`credits.move_pending{id}` while any move, send or `ToShielded` may still execute), the plan's refusals, and the grant (`platform.grant_exceeded`; a lease not `PlatformAddressMove` is `platform.grant_invalid`). | R4, R5. The lease is the existing `PlatformAddressMove` (`P/flows.rs:59`): the same vault scope (`PlatformAddress`, `rust/crates/dw-vault/src/signer.rs:113`), the same consent (moving the user's own Platform credits), and no new `FlowKind`. |
| C1-7 | **(c) Holds.** `platform_shield` takes the shielded wallet lock, then the move lock (lock order §0). A `pending` `ToShielded` refuses every new move, send, shield and C3a top-up with `credits.move_pending{id}`, and a new shield is refused while any of those is held. It has no exact retry, because `shield_to` cannot re-sign at stored nonces. It leaves `pending` when its nonces are taken (`done`) or when a proof above the first read after its call still shows them unused (`failed` by height). A record `failed` by height may still execute, so it keeps holding as m4:591 rules: the only later move allowed is a `ToCore` move that signs exactly its inputs at its nonces, and a new shield is still refused. Of the two, at most one executes. `unshield_to_platform` takes the shielded wallet lock only. It spends no address nonce, so it neither joins nor waits on `move_pending`. | The shielded lock makes Close Wallet and the network close see a shielded call in flight (`P/shielded_pool.rs:398-400`, m4:723-746) while the proof is built. The address side keeps the one-hold-per-wallet rule moves already have. |
| C1-8 | **(b) Attribution, Platform→Shielded.** `PlatformActivityKind` gains `ToShielded`, one row per record, `credits` = the debit and `may_land` as for moves. While the record is `pending` its inputs are not compared. Once `done`, the first fall of each input up to what it took (input 0: also the flat fee and the metered fee) is the record's, so no `outgoing` row appears. A `ToShielded` group settles as moves' groups (`P/platform_moves.rs:83-91`). On the shielded side the library's live row is `Shield`, out of `PlatformAddress` (no change). | R7: the move's rule (`P/platform_moves.rs:20-24`) with one more kind. |
| C1-9 | **(b) Attribution, Shielded→Platform.** `PlatformActivityKind` gains `FromShielded`, with a `FromShieldedRecord{activity, st_hash, to, credits, state, armed}` kept in app.sqlite under the move lock. The hook's accept writes it, armed, right after the dispatch artifact and before the hook returns `Ok` (R6): that is the signature release for this purpose. While it is `pending` and armed, a rise of `to` up to `credits` is absorbed, so no `incoming` row appears. It is `done` when the spend's artifact reads `executed` (A5's disposition: the library's proved result, which binds for `Unshield`, `VER:3212`, or the scan's nullifiers). It is `failed` when the artifact reads `not_executed`, and what it absorbed then returns as `incoming`. A spend the hook refused writes nothing. | R7 with `platform_fund`'s armed and absorbed rule (`P/platform_moves.rs:24-50`). Arming at the hook ties the claim to the one moment a transition can leave. A crash between the artifact and the record leaves a plain `incoming` row: a wrong label, no money. |
| C1-10 | **Upstream (UP-1, RP-1).** C1b uses UP-1's `shielded_unshield_to_with_keyset` (F-hook-f3's pattern: the FVK is checked before any build or store write). C1b's `to` is parsed with `PlatformAddress::from_bech32m_string` and checked against the network HRP, as `PW:1496` does. **Optional in UP-1:** `shield_to_with_nonces`, `shield_to` taking `inputs_with_nonce`, which would let C1a sign at the recorded nonces and drop C1-3's one-device residual. C1a does not wait for it. | UP-1 is fork-only (DEC-1302). C1a is ready to build without it. |
| C1-11 | **Floors and minimums.** The unshield is refused by Platform below the pool's outgoing floor (`minimum_pool_notes_for_outgoing`, 250 notes), mapped as `unshield_to_core` maps it: `shielded.pool_too_small{notes, required}` (m4:826). Its minimum is the unshield's structure minimum at the version, else `shielded.below_minimum{min}`. The shield's minimum is the structure check's (amount > 0 and Σ claims ≥ amount + flat fee) plus the first input clearing the reserve; below that is `credits.send_out_of_range{min}`. | Existing codes; no new floor check before Platform's own (m4:826 "the engine does not check the floor first"). |

### User-facing copy (new states and refusals)

| State | Copy |
|---|---|
| Transfer, Platform → Shielded, form | "Moves credits from your Platform addresses into Shielded. Fee about {pool fee + reserve estimate}." |
| Confirm | "Move {amount} to Shielded? It comes out of your Platform addresses. Dash Platform takes a fee of about {fee} from {address}." |
| Done | "Moved to Shielded. It shows in your shielded balance after the next shielded sync." |
| Pending (unknown outcome) | "This move may still go through. We'll check again; don't move these credits again until it shows as done." (no retry button) |
| Held (`credits.move_pending`) | existing MOVE_COPY text for a pending move (`UI/platform/move.ts`) |
| Transfer, Shielded → Platform, Done | "Moved to your Platform address {addr}." |
| Pending | existing shielded pending copy (`SHIELDED_COPY`, `UI/shielded/flow.ts`): "may still go through", no retry |
| Platform activity row `toShielded` / `fromShielded` | "To Shielded" / "From Shielded" (never "Sent" / "Received") |
| `TRANSFER_COPY.unsupported` | removed once both routes land (C1c) |

### Contract rows

- **C1a** (m4 §2.12, §2.13 cross-reference, §3, §4, §5, §7 next item; Contract-Version next):
  - `platform_shield_quote(amount: WithdrawAmount)` and `platform_shield(amount, grant)`, both async and returning
    `CreditsError` (rows C1-2…C1-8).
  - `PlatformShieldQuote` and `PlatformShieldOutcome{id, credits, state, may_land}`.
  - `PlatformActivityKind::ToShielded`; the `MoveKind::ToShielded` store encoding.
  - `CreditsError::Shielded(ShieldedError)`, a transparent arm with no new code.
  - Downgrade text beside m4:607-612: a 0.2 build skips a `to_shielded` record and so drops its hold. It pays only
    the user's own pool, and Drive takes each nonce once, so nothing is paid twice.
- **C1b** (m4 §2.12 activity, §2.13, §3, §5, §7 next item; Contract-Version next):
  - `unshield_to_platform_quote(credits, to: Option<String>)` and
    `unshield_to_platform(credits, to, grant, confirm_unrecorded)`, both `ShieldedError`, with gates as
    `unshield_to_core`'s (journal, `confirm_unrecorded`, hook) plus C1-1's `to` rule.
  - `PlatformActivityKind::FromShielded` and its record.
  - The artifact kind `unshield` already exists (`dispatch.rs:543`). A5's disposition handling covers it, and A4b's
    load sort reads it.
- **C1c:** none (renderer).

### Tests that prove the money rules

C1a (`P/platform_moves_tests.rs` and a new `platform_shield_tests.rs`):

| Test | It shows | Mutant it kills |
|---|---|---|
| `shield_quote_and_max_equal_the_preflight` | The quote and Max equal `shielded_shield_preflight` over the same balances, at several address sets, including one with its first address below the reserve. | planner sorting largest-first |
| `shield_record_is_durable_before_the_signer_signs` | The record is `pending` with its inputs at proved + 1, read back, before the first signature. A failed write is `storage` with nothing signed. | record after signing |
| `shield_refused_before_signing_keeps_the_grant` | A refusal before signing (the gate, the hold, the plan, rule 5) leaves the grant unredeemed. | — |
| `shield_unknown_outcome_holds_the_next_move` | An unknown outcome leaves the record `pending`. The next `move_to_core`, `platform_send`, `platform_shield` and `top_up_from_platform` are `credits.move_pending`. | settle on the call's `Ok` |
| `shield_settles_only_on_proved_nonces` | `done` needs a proof with every input at or past its nonce. A lower proof decides nothing. | settle without the proved nonce (brief's mutant) |
| `shield_signer_refuses_an_address_outside_the_plan` | A build that asks for a signature from an address outside the plan is refused unsigned. | signer admits any address |
| `shield_attribution_no_stray_outgoing` | After `done`, the inputs' falls are the record's: no `outgoing` row. The group rule holds. | — |
| `lock_order_shield_vs_unshield_vs_move` | Concurrent C1a, C1b and `move_to_core` finish. | — |

C1b (`P/shielded_tests.rs`, `P/shielded_dispatch_tests.rs`):

| Test | It shows | Mutant it kills |
|---|---|---|
| `unshield_to_core`'s set, rerun for the new call | gates, pending, unresolved, `pool_too_small` | — |
| `unshield_to_platform_refused_without_hook` | Without the hook: `platform.unavailable`, nothing quoted or signed. | the seed path (brief's mutant: call `shielded_unshield_to` with the seed) |
| `unshield_to_platform_refuses_a_foreign_to` | `to` outside the payment pool is refused before the grant. | — |
| `from_shielded_absorbs_only_while_armed` | Before the hook's accept, a rise is `incoming`. After it, the rise is absorbed up to `credits`. A `not_executed` artifact gives it back. | arm at quote time |
| `from_shielded_done_only_on_executed_artifact` | — | done on the call's `Ok` |

C1c: vitest over `ROUTES` and `MAX_DERIVE` for both routes; Playwright (Chromium, WebKit) over the fixture world;
DEC-1484 images in light and dark.

### Out of scope

- External recipients either way (C1-1, pending MG).
- ShieldFromIdentity (`PW:2078`, PV14), not a lane-C item.
- A pool-floor precheck.
- Moving a held `ToShielded`'s nonces into a new shield (no exact retry, C1-7).
- Shielded→Platform Max beyond `shieldedSpendMax` (C1c derives it in the renderer, as C2b does).

## D-C2: Core Max drains (ROUTE-05; slices C2a, C2b, C2c)

Today's Transfer Max from the Dash wallet is a two-quote renderer probe (`UI/transfer/model.ts:276-380`;
`coreFundedMax` `:349`, `MAX_DERIVE` `:374-378`). It locks `balance − fee` with coin selection, so change can stay
in Core. The only engine drain is CoinJoin account 0's (`P/shield_drain.rs`, `fund_drain_with`
`P/registration_flow.rs:3362-3384`).

### Decisions

| # | Decision | Why |
|---|---|---|
| C2-1 | **A Core Max drains BIP44 account 0 only.** It drains no BIP32 account, no DashPay-receiving account and no CoinJoin account. The quote reports `left_duffs`: spendable coins in the wallet's other accounts that stay behind. | key-wallet's asset-lock funding names only `Bip44{account_index}` and `CoinJoin{account_index}` (`KW:65-76`). A drain must name exactly one account family (`KW:271-287`), and CoinJoin is drain-only and sole-source (`KW:282-285`) because pooling links mixed coins. Mixed coins have their own move (`shield_mixed_coins`, m4:821). |
| C2-2 | **Held coins refuse; no fallback to the probe Max.** If any coin of BIP44 account 0 is held (a user's "Lock unspent", a send's or sweep's reservation), the drain is refused before signing with `shielded.coins_locked` / `credits.coins_locked`. The UI says to unlock it or type an amount. **MG to confirm** (the alternative is a silent fallback to today's probe Max). | The library's drain consumes every final coin of the account and cannot be told to skip one (`P/shield_drain.rs:5-15`; `KW:399`, `SelectionStrategy::All`). Signing a lock that spends a coin the user locked breaks the lock. A silent fallback would make "Max" mean two different things, and the probe Max can hit the same refusal anyway (m4:516: the builder "would select" a held coin). |
| C2-3 | **Unconfirmed coins.** The drain takes the account's final coins (confirmed or InstantSend-locked: the builder's `require_final_inputs`, `KW:397`). A coin that is not final yet is left out and reported as `arriving_duffs`. A coin that turns final between the quote and the build makes the build differ from the plan, and the drain signer refuses it unsigned (`coins_changed`, row C2-5). | No refusal is needed for a coin the builder leaves out. The plan the user confirmed is exactly what is signed. Unlike the CoinJoin drain, plain BIP44 coins have no foreign unconfirmed ancestors (the reason `P/shield_drain.rs:12-14` refuses). |
| C2-4 | **The 50-input cap refuses before signing.** More than `MAX_LOCK_INPUTS` (50) final coins is `*.too_many_inputs{inputs, max}`. Copy: "Max needs at most 50 coins; this wallet has {n}. Enter an amount instead." **MG to confirm** the copy and that no "Max of the 50 largest" is offered. | An InstantSend proof carries the whole transaction and stops fitting a transition at about 65 inputs (m4:784-787). The cap and its check before signing already exist for shields and funds (m4:818, item 83, item 85). A 50-largest Max would leave coins behind under a "Max" label. |
| C2-5 | **The drain signer.** C2a funds a `DrainAll{minimum_lock_duffs}` lock over `Bip44{0}` (the `fund_drain_with` pattern with the BIP44 account). It runs under the existing `Shield` (`shield/<k>/funding`) or `PlatformAddressFund` (`paddr/<k>/funding`) lease and step, with a `DrainPlan`-bounded counted funding signer (`P/signers.rs:410`, `drain_signer` `P/shield_drain.rs:190`). It signs only the planned coins, one signature each, with their sum within the grant. It admits the build before its first signature only when the inputs are exactly the planned coins at their planned values and the one output is the lock's credit burn within the planned fee. Anything else is `coins_changed`, unsigned, with nothing tracked or sent. `CreditsError` gains `credits.coins_changed` (Platform side); `shielded.coins_changed` exists. | The vault cannot see a debit (`P/shield_drain.rs:20-22`). Only the call's signer can hold a whole-balance spend to what the user confirmed. Same rule as MIX-SHIELD (m4 items 82–83). |
| C2-6 | **Fee and amounts.** The fee comes out of the lock: `lock = Σ inputs − fee`. A shield's credits are `lock × 1000 − pool_fee`; a fund's are `lock × 1000 − fee estimate`. The grant is `PlatformOp{max_duffs = Σ inputs}`. A lock below the floor (the shield's `drain_floor`, `P/shield_drain.rs:66`; the fund's fee estimate + 1 duff) is `*.below_minimum{min}` before signing. | The drain's debit is the coins' sum. The fee is inside it, so the grant caps everything that leaves. |
| C2-7 | **Restart safety is the existing steps'.** The drain's lock is an ordinary shield or fund lock: `pending_shields`/`resume_shield`, `pending_platform_funds`/`resume_platform_fund`, and Repair's `resolve_stuck_funding`. For a BIP44 drain step, Repair spends exactly the lock's recorded inputs back to the wallet, as it does for a CoinJoin drain (m4:821). | A kill after the lock is signed resumes the same outpoint (DEC-1045 / DEC-1051), never a new lock. |
| C2-8 | **Max to an external shielded address (C2b)** needs no engine change. The Shielded sheet's send gets Max through `shieldedSpendMax` (`UI/transfer/model.ts:361`). Pay still refuses shielded addresses (`UI/screens/pay/model.ts:137-138`); Pay is not in this item. | The send's fee is the note-selection fee for the notes the amount needs (`P/shielded_pool.rs:726-749`). The probe at the whole balance names `needed` with every note, and the second quote confirms the derived amount, so a non-monotone step can only make the second quote refuse (the line shows it), never overspend. |

### User-facing copy

| State | Copy |
|---|---|
| Confirm, Core Max → Shielded | "Move everything in your Dash wallet, {total}, to Shielded? The network fee (about {fee}) comes out of it." Add "{left} received from contacts or other accounts stays in your Dash wallet." when `left_duffs > 0`, and "{arriving} still arriving stays in your Dash wallet." when `arriving_duffs > 0`. |
| Confirm, Core Max → Platform | the same with "to your Platform address" |
| Held coin | "A coin you locked is in your Dash wallet. Unlock it under Coins, or enter an amount." |
| Coins changed | "Your Dash wallet changed since this was worked out. Review the amount again." |
| Over 50 coins | C2-4's copy |

### Contract rows

- **C2a** (m4 §2.12, §2.13, §3, §4, §7 next item; Contract-Version next):
  - `shield_quote(amount: FundAmount)` and `shield_from_core(amount, grant)`, where `FundAmount = Duffs{duffs} | All`
    replaces `duffs: u64`. Likewise `platform_fund_quote(amount)` and `platform_fund(amount, grant)`.
  - `All`'s quote adds `inputs`, `left_duffs` and `arriving_duffs`.
  - New code `credits.coins_changed`. The `All` refusals are `*.coins_locked`, `*.too_many_inputs`, `*.below_minimum`
    and `*.coins_changed`.
  - dw-app gains the amount enum, and the generated TS is regenerated.
- **C2b, C2c:** none.

### Tests that prove the money rules

| Test | It shows | Mutant it kills |
|---|---|---|
| `core_max_signs_exactly_the_planned_coins` | The build's inputs equal the plan. A coin added between the quote and the build is `coins_changed`, unsigned. | signer admits an extra input (brief's mutant) |
| `core_max_refuses_a_held_coin_before_signing` | Nothing is signed, tracked or sent, and the grant is unredeemed. | drop the held check |
| `core_max_refuses_past_50_inputs_before_signing` | `too_many_inputs` with `inputs ≥ 51`, the Funding charge refunded. | — |
| `core_max_leaves_other_accounts` | A BIP32 / DashPay-receiving / CoinJoin coin is never an input. `left_duffs` is reported. | fund from the wrong account |
| `kill_after_drain_lock_signed_resumes_same_outpoint` | Restart, `pending_shields` / `pending_platform_funds` lists it, and resume applies the same outpoint. | — |
| `drain_grant_caps_the_debit` | A grant below Σ inputs is `platform.grant_exceeded` before the redeem. | — |

C2b and C2c: vitest (MAX_DERIVE uses the engine's `All` for `coreToShielded` and `coreToPlatform`, and the confirm
copy); Playwright; DEC-1484 images (C2c).

### Out of scope

- Draining BIP32, DashPay-receiving or several accounts at once.
- Combining coins for BIP44.
- Max in Pay to a shielded address (Pay does not send shielded).
- An engine Max for Shielded→anything (renderer-derived).

## D-C3: top-up from Platform or Shielded (ROUTE-12, ROUTE-17; slices C3a, C3b, C3c)

Today's top-up is Core-only: DP1-06 / DP1-10 (`P/topup.rs`, facade `P/credits.rs:226-291`,
`UI/screens/dashpay/TopUpSheet.tsx`). iOS tops up from Core, Platform or Shielded. For Shielded it recommends two
steps through an own Platform address (`ios.md:630`, `:675`, `TransferPrivacyTip.swift:123`).

### Decisions

| # | Decision | Why |
|---|---|---|
| C3-1 | **Sources:** Dash wallet (the default, unchanged), Platform, Shielded. Only the wallet's own identities can be topped up, not watched ones (`identity.not_found` / `invalid_argument`, as `top_up` and `withdraw` refuse). | The library would top up any identity (`PW:1535` doc). The facade's top-ups are the wallet's own (m4:516). |
| C3-2 | **Platform source: an address move to the identity.** `top_up_from_platform_quote(identity, amount: WithdrawAmount)` / `top_up_from_platform(identity, amount, grant)` run `platform_moves`' `run` (`P/platform_moves.rs:2441`) with a new `Target::Identity(id)` and `MoveKind::ToIdentity{identity}`. That gives one proved read with rule 5, the hold, the plan, the record before signing (R1), and nonces at proved + 1. The transition is built by dpp at those nonces (§0) and signed by the payment-address keys under a `PlatformAddressMove` lease, then broadcast by the SDK. | §0: the library's `top_up_from_addresses` (`PW:519`) hides the nonces. A move with another target keeps every rule of moves, with no second hold model. |
| C3-3 | **Platform source: gates.** First the money gate with `Subject::Identity` (R4; DP3-04 D6, as `top_up` at `P/topup.rs:435`). Then the identity's write lock, then the move lock (lock order §0), the address read with rule 5, and PV ≥ 11 (`platform.feature_off{feature: "platform_addresses"}`). The plan's range is Platform's for IdentityTopUpFromAddresses at the version (`min_input_amount` per input, the top-up base cost; dpp's `calculate_min_required_fee` for the fee estimate). `All` uses the move planner's `All` (`P/platform_moves.rs:1215`), with the fee estimate kept on the fee input. | R4: the identity id and keys are Platform data. The identity's write lock is held throughout, so a balance read before and after measures this top-up alone (as `P/topup.rs:46-48`). |
| C3-4 | **Platform source: settlement and evidence.** The record settles exactly as a move does: `done` once a proof shows every input at or past its signed nonce, `failed` by height as moves, and groups by moves' rule. The identity's proved balance in the broadcast's answer is shown as `balance` and is not evidence: `IdentityTopUpFromAddresses` does not bind execution (`VER:3199`; the proof is the identity's balance and the input addresses only, `VER:1911-1946`). An unknown outcome returns `state: pending` and holds the next move (`credits.move_pending`, R3). There is no `broadcast_unknown` error, because the record names it. | R2. The nonces are the transition's own inputs, so "every input reached its nonce" is the move's proof that this group executed. |
| C3-5 | **Platform source: attribution.** `PlatformActivityKind::ToIdentity{identity}`, a debit row with moves' no-`outgoing` rule (R7). On the identity side the credits block re-reads the balance on `Platform{Credits}`, as after a Core top-up. | R7. |
| C3-6 | **Shielded source: two steps through an own Platform address, not the direct type-22 transition.** **MG to confirm.** Step 1 is C1b (`unshield_to_platform` to the receive address). Step 2 is C3a (`top_up_from_platform` of the step-1 amount), offered only once step 1's `FromShielded` record is `done` and the address's proved balance shows it. The sheet runs both steps. If the app closes between them, the credits stay on the user's Platform address (visible on Home and in Transfer) and the sheet offers "Top up from Platform". There is no persisted intent and no automatic second step. C3b (direct) is deferred, and UP-1 can drop `shielded_identity_top_up_from_pool_with_keyset` (C4b still needs `…_create_from_pool_with_keyset`). | (1) The direct transition needs PV14 (`PVF:6`), and mainnet is at PV13 (ROADMAP §7.3), so direct would read "not available" there. The two-step route needs PV12 and PV11, which mainnet has. (2) It is iOS's recommended route. (3) It needs no code beyond C1b and C3a, and each step settles by an existing proof model. (4) Neither step moves money on the other's unknown outcome: step 2 starts only after step 1 is proved, and a step-2 unknown is a held move. Privacy is no worse than direct: both publish the amount credited to the identity. |
| C3-7 | **Shielded source: if MG picks direct (C3b).** `top_up_from_shielded_quote(identity, credits)` / `top_up_from_shielded(identity, credits, grant, confirm_unrecorded)` in `shielded.rs`. It goes through UP-1's keyset entry point under a `ShieldedSpend` lease and the hook, with a hooked `IdentityTopUp` artifact whose `identity_id` is set (`dispatch.rs:539-564`). Gates: the identity's write lock, then the shielded lock; the money gate (`Subject::Identity`); PV ≥ 14, else `platform.feature_off{feature: "shielded_top_up"}`; the gates of `shielded_send` (journal, `confirm_unrecorded`, hook). **Evidence:** only the artifact's disposition (A5: the scan's nullifiers spent). The library's proved post-top-up balance (`OPS:1350-1361`) is shown, never evidence, because a spent nullifier cannot tell this top-up from a competing spend of the same notes (`VER:3226-3231`). | R6, R2. It is the existing hooked-spend model with one more kind, as lane-C §0.2 found the journal ready. |
| C3-8 | **The sheet (C3c).** The "Pay with" choice is Dash wallet / Platform / Shielded, each with its balance, quote, Max and outcome. Platform is disabled below PV11 or with nothing movable. Shielded is disabled until the shielded account is ready, and shows the floor refusal copy when the pool is too small. The default stays Dash wallet. | DASHPAY F26 spirit (shielded is never the default funding); the DP1-10 sheet's existing flow. |

### User-facing copy

| State | Copy |
|---|---|
| Source picker | "Pay with: Dash wallet · Platform · Shielded" |
| Platform, confirm | "Top up {identity} with {amount} from your Platform addresses? Dash Platform takes a fee of about {fee}." |
| Platform, pending | "This top-up may still go through. Your Platform balance stays held until it does; don't top up from Platform again until it shows as done." |
| Shielded, step 1 of 2 | "Step 1 of 2: moving {amount} from Shielded to your Platform address." |
| Shielded, step 2 of 2 | "Step 2 of 2: topping up {identity} from your Platform address." |
| Shielded, between steps (after a restart or a pending step 1) | "{amount} is on your Platform address, waiting to top up {identity}. Top up from Platform to finish." |
| Direct only (if C3b): below PV14 | "Topping up straight from Shielded isn't available on this network yet." |

### Contract rows

- **C3a** (m4 §2.8, §2.12 activity, §3, §5, §7 next item; Contract-Version next):
  - `top_up_from_platform_quote(identity, amount)` and `top_up_from_platform(identity, amount, grant)`, both
    `CreditsError`.
  - `TopUpFromPlatformQuote{credits, fee_credits, debit_credits, inputs, grant}` and
    `TopUpFromPlatformOutcome{id, credits, state, may_land, balance}`.
  - `PlatformActivityKind::ToIdentity` (row field `identity`) and the `MoveKind::to_identity` store encoding, with the
    downgrade text as C1a's.
- **C3b** (only if MG picks direct): m4 §2.13 `top_up_from_shielded_quote` and `top_up_from_shielded`; the
  `feature_off` feature name `shielded_top_up`.
- **C3c:** none.

### Tests that prove the money rules

| Test | It shows | Mutant it kills |
|---|---|---|
| `top_up_from_platform_settles_on_proved_nonces` | — | settle on the answer's identity balance |
| `top_up_from_platform_unknown_holds_the_next_move` | An unknown outcome leaves the record `pending`; the next move, send, shield or top-up from Platform is `credits.move_pending`. | — |
| `top_up_from_platform_refusal_keeps_the_grant` | The money gate, the identity check, PV11, the plan and rule 5 refuse with the grant unredeemed. | money gate after the redeem |
| `top_up_from_platform_record_before_signing` | as C1a's | record after signing |
| `top_up_from_platform_attribution` | no `outgoing` row; `ToIdentity` names the identity | — |
| `lock_order_top_up_from_platform_vs_withdraw_vs_move` | the identity lock is taken before the move lock | — |
| renderer `two_step_waits_for_step_one_done` (vitest) | Step 2 is offered only after step 1's `FromShielded` is `done`. A pending step 1 shows "may still go through" and no step 2. | offer step 2 on step 1's `Ok` |
| C3b, if built | `shielded_send`'s set, plus the PV14 refusal before anything is quoted or signed, and the artifact carrying `identity_id` | the seed path; settle on the library's proved balance |

### Out of scope

- Topping up identities the wallet only watches, or someone else's.
- A persisted two-step intent or automatic step 2.
- ShieldFromIdentity.
- Registration funding (D-C4).

## D-C6: credit withdrawal to own Platform address (ROUTE-22; slices C6a, C6b) — DP6-02 addendum

DP6-02 withdraws credits to Core only (`P/withdrawal.rs`, facade `P/credits.rs:301-341`,
`UI/screens/dashpay/WithdrawSheet.tsx`). iOS also withdraws to an own Platform address and to nothing else (ROUTE-22
and ROUTE-23, `ios.md:721-722`).

### Decisions

| # | Decision | Why |
|---|---|---|
| C6-1 | **Destination: the wallet's own Platform receive address only.** The quote names `platform_receive_address` (m4 §2.12). The call takes `to` and refuses anything outside the payment account's derived pool with `invalid_argument`. | iOS's scope (`ios.md:721-722`); nothing pays a stranger. |
| C6-2 | **The transition: dpp's `IdentityCreditTransferToAddressesTransition::try_from_identity`** with one recipient (`to → credits`), `user_fee_increase` 0, the signer, the key and the nonce passed explicitly (§0). It is broadcast by the SDK's `broadcast_and_wait`. It does not go through `PW:600`. | DP6-02 D4: the evidence and the next nonce need the nonce, which the library hides. |
| C6-3 | **Key and lease.** Signed by a CRITICAL TRANSFER key (`identity_credit_transfer_to_addresses_transition/v0/identity_signed.rs`: purpose TRANSFER, level CRITICAL), chosen as a withdrawal chooses it (`requests::flow_keys`, `P/requests.rs:439`; none is `identity.keys_missing{transfer}`). It runs under the existing `Withdraw` lease (`P/flows.rs:24`) and grant `PlatformOp{max_credits = credits + fee_credits}`. | The same keys, budget and consent as a withdrawal. A new `FlowKind` would add contract surface for no new authority. |
| C6-4 | **Gates, in order, before the grant is redeemed:** the identity is the wallet's; the money gate (`Subject::Identity`, R4); the identity's write lock; PV ≥ 11 (`platform.feature_off{feature: "platform_addresses"}`); the quote again on a fresh proved balance; the session's unconfirmed nonce (row C6-6); the grant's cap. | As DP6-02 §2 "Gates", plus the address-funds version. |
| C6-5 | **The Max reserve.** `fee_credits = max(min_fee, metered_bound)`. `min_fee` is dpp's `calculate_min_required_fee` for the transition with one recipient at the version (`…/state_transition_estimated_fee_validation.rs:13`; built on `state_transition_min_fees.credit_transfer_to_addresses`, 500,000 in `STATE_TRANSITION_MIN_FEES_VERSION1`). `metered_bound` is DP6-02 §1's method: the identity's balance and nonce update priced as one `identity_update` minimum, plus one address-balance insert at the largest size priced by Drive's estimator, with the non-exact processing doubled. `All` = balance − `fee_credits`. The amount must be at least the version's `min_output_amount` (`…/v0/state_transition_validation.rs:36-42`), else `credits.withdrawal_out_of_range{min, max}`; `max` is the balance less the reserve. | Rules 1 and 2 of DP6-02 §1 apply unchanged to this transition (estimated fee before, metered fee after). The output's minimum is the transition's own structure rule. Unlike DP6-02's, the metered bound may exceed `min_fee`, because an address insert costs storage; the `max` covers it. |
| C6-6 | **The nonce: shared with withdrawals.** `WithdrawalNonces` (`P/withdrawal.rs:259`) becomes the session's per-identity record of identity-nonce writes this flow signed. `Signed` gains a variant, `Transfer{to, credits, st_hash, before}`, beside the withdrawal's `{document, credits}`. A withdrawal and a transfer of one identity take the unconfirmed nonce while Drive would take it (`next`, `:286-300`), so of an unconfirmed withdrawal or transfer and the next one, at most one executes. The floor and `platform.unavailable` rules are D24's and D26's, unchanged. | One identity has one nonce sequence. Separate states would let a transfer sign N+1 beside a pending withdrawal at N, and both could execute (the D24 cache bug, for two flows). |
| C6-7 | **Evidence: what counts as executed.** Executed when the nonce was signed once and the identity's proved balance is at least `credits` below the one read under the lock (D24 §3's drop arm). Executed also when the recipient's proved balance has risen by at least `credits` over the `FromIdentity` record's baseline, at a proof height above the read under the lock, with the identity's proved nonce at or past the signed one: the Platform-side record absorbed it (row C6-8). Anything else is unknown. The library's proved answer (`VER:1815-1845`: the identity's balance and revision, and the recipients' balances) feeds both arms. It is never evidence alone (`VER:3198`, `binds = false`). | No document exists to read back (DP6-02's document arm). The two arms are the two balances the transition changes. The nonce condition stops a deposit to the address from another source counting as this transfer while the transfer could still be pending. |
| C6-8 | **Unknown outcome and the next write.** An unknown outcome is `platform.broadcast_unknown{artifact}`, where `artifact` is Platform's hash of the transition (hex SHA-256, as shielded `transition`, m4:675). No retry is offered (D23's copy, adapted below). Before the next withdrawal or transfer of the identity signs: with the unconfirmed one's nonce still unused, it signs at that nonce (at most one executes). With the nonce used, the unconfirmed transfer is final either way: one that C6-7 shows executed refuses the new one unsigned as `credits.withdrawal_landed{artifact}` (the existing code: the sheet is the withdraw sheet), and one not shown executed lets the new one take a new nonce, because the old one can no longer execute. | R3. D24's guarantee for the new kind: "if you withdraw again, at most one of the two goes through", and a retry is refused once the first is seen to have landed. |
| C6-9 | **Attribution.** `PlatformActivityKind::FromIdentity{identity}`, a `FromIdentityRecord{st_hash, identity, to, credits, nonce, state, armed}` in app.sqlite. It is written armed under the move lock before the signature (lock order: identity, then move). It absorbs a rise of `to` up to `credits` while pending (R7), so no `incoming` row appears. It is `done` on C6-7's evidence. It is `failed` when the identity's proved nonce has passed the signed one and a read at a height above that proof shows nothing absorbed; anything it absorbed then goes back as `incoming`. On the identity side, `withdrawals` lists documents only, so it does not list a transfer. The withdraw sheet's result and Platform activity show it. | R7 with `platform_fund`'s pattern. A transfer whose nonce was taken by something else can never execute, so `failed` is final. |
| C6-10 | **PV gate.** Below PV11 the destination "Platform" is not offered and the engine refuses with `platform.feature_off{feature: "platform_addresses"}`. | `PVF:3`. |

### User-facing copy

| State | Copy |
|---|---|
| Withdraw sheet, destination | "To: Dash wallet · Platform" |
| Platform, confirm | "Move {amount} of credits to your Platform address {addr}? Dash Platform keeps up to {fee} for the fee." |
| Done | "Moved to your Platform address. It's in your Platform balance now." |
| Unknown (`broadcast_unknown`) | "It may have been sent. Check your Platform balance before trying again. If you move credits again, at most one of the two goes through." (transition id shown; no retry button) |
| Landed (`credits.withdrawal_landed`) | DP6-02 D23's copy: "Your last withdrawal went through after all, so this one does not. Withdraw again only if you want a second one." |
| Platform activity row | "From {identity name}" |

### Contract rows

- **C6a** (m4 §2.8, §2.12 activity, §3, §5, §7 next item; Contract-Version next):
  - `withdraw_to_platform_quote(identity, amount)` → `WithdrawToPlatformQuote{credits, fee_credits, to, grant}`.
  - `withdraw_to_platform(identity, to, amount, grant)` → `WithdrawToPlatformOutcome{credits, to, remaining_credits, transition}`.
  - Errors: those of `withdraw`, plus `platform.feature_off{platform_addresses}`.
  - `PlatformActivityKind::FromIdentity`.
  - The `Withdraw` lease's semantics row says it also pays a transfer to an own Platform address.
- **C6b:** none.

### Tests that prove the money rules

| Test | It shows | Mutant it kills |
|---|---|---|
| `max_leaves_the_reserve` | At each known version, Max passes dpp's estimated-fee check and the metered bound covers Drive's estimate for one recipient. | reserve = `min_fee` only |
| `transfer_and_withdrawal_share_the_unconfirmed_nonce` | After an unknown withdrawal at N, a transfer signs at N while Drive takes it, and the reverse. | separate nonce states |
| `a_retry_after_an_unknown_transfer_cannot_execute_twice` | D24's test for the new kind. | the SDK cache's nonce |
| `unknown_transfer_is_broadcast_unknown_without_retry` | The outcome is `platform.broadcast_unknown{artifact = st hash}`, and the session's entry is kept. | `Ok` on the call's answer |
| `landed_transfer_refuses_the_next_unsigned` | `credits.withdrawal_landed`, with the grant unredeemed. | no read-back before signing |
| `a_deposit_while_pending_is_not_this_transfer` | A rise of `to` with the identity's nonce still below the signed one is not evidence. | drop the nonce condition |
| `from_identity_absorbs_only_while_armed` | no `incoming` row for the transfer's rise | arm after signing |
| `transfer_to_a_foreign_address_is_refused` | `invalid_argument` before the grant | — |
| T2 (dashmate devnet), recorded as DP6-02 D12 | one transfer to an own Platform address: proved nonce before and after, the balance drop arm, the address rise | — |

C6b: vitest (`dashpay/withdraw.ts` destination), Playwright, DEC-1484 images in light and dark.

### Out of scope

- Transfers to someone else's Platform address, and identity → Shielded (ROUTE-23 is dead on iOS).
- Persisting the unconfirmed entry across restarts (BL-335 covers withdrawals; the transfer variant joins it there).
- Listing transfers under My Profile's recent withdrawals.

## MG to confirm

| Row | Recommendation |
|---|---|
| C1-1 | Platform ↔ Shielded pays own addresses only in 1.0; no external recipients. |
| C2-2 | A held coin refuses Core Max (no silent fallback to the probe Max). |
| C2-4 | Over 50 coins refuses Core Max with "enter an amount" (no "50 largest" Max). |
| C3-6 | Top-up from Shielded is the two-step route (C1b, then C3a); C3b (direct, PV14) is deferred, and UP-1 may drop the top-up keyset entry point. |

## Open risks

- **C1-3 residual.** Until UP-1 offers an explicit-nonce `shield_to`, a `ToShielded` record's nonces are the
  engine's read, not what the library signed. Another device spending the same address in that window can mark it
  `done` early. Nothing is spent twice and the money stays in the user's pools; this is the one-device assumption
  m4 already makes for moves.
- **Fee deduction with no cap** (C1-5, C3-3): Platform takes the metered fee from the fee input's remainder at
  execution, as for every address transition today (m4 round 3).
- **C6-7's recipient arm** relies on a fresh own address. An outside deposit of at least the amount, after the
  identity's nonce passed, reads as the transfer. That is a wrong label, and nothing is moved twice.
- **m4 serialization.** Five slices each take a Contract-Version at merge (§0).
