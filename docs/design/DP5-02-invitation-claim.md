# DP5-02: Invitation claim

ROADMAP DP5-02 (engine, T2): status, the inviter preview, an identity and a name paid by the invitation, and a contact
request to the inviter. [DASHPAY §2.9](DASHPAY.md) (the link stash, DP5-01) and §3.4 (registration rows, DP1-02) are
the base: the claim is a new **entry state of DP1-02's registration machine**, funded by the invitation's asset lock
instead of a lock the wallet builds, and it reads the link from DP5-01's stash by id. DP5-03 builds the UI on it.
Acceptance: an identity and a name with zero Core balance; the inviter gets the request; an already-claimed
invitation is detected (the mainnet claim is H-07).

Platform is read **at the pin**, PastaPastaPasta/platform `82fb33aff8`, which carries **#4997** (`e52344c905`,
"invitation claim status for the invitee, idempotent claim"). Paths below are under its `packages/`; `inv.rs` is
`rs-platform-wallet/src/wallet/identity/network/invitation.rs`, `crypto.rs` is
`rs-platform-wallet/src/wallet/identity/crypto/invitation.rs`.

| Part | Where |
|---|---|
| Status and preview (`invitation_status`) | `rust/crates/dw-engine/src/platform/invitations.rs`, `invitation_check.rs` (pure checks, tests `invitation_check_tests.rs`) |
| Non-deleting read of a stashed invitation | `rust/crates/dw-vault/src/vault/links.rs` (`read_stashed_invitation`), tests `links_tests.rs` |
| The claim step, the link wipe, the request to the inviter | `registration_flow.rs` (`claim`, `forget_link`, `request_inviter`), `registration.rs` (`plan`), `registration_rows.rs` (`Funding::Invitation`) |
| The request under the registration lease | `requests.rs` (`send` takes the flows its lease may be of) |
| Kill matrix, statuses, zero duffs, mutants | `registration_tests/invitation.rs` |
| Signing equality with Platform's own construction | `rust/crates/dw-engine/tests/dp5_02_claim_signing.rs` |
| Test-only `invite create` | `dw-engine/src/test_support.rs` (`create_invitation`), dwcli feature `invitation-create-test`, build check `rust/bin/dwcli/tests/invite_create_gate.rs` |
| Contract | [m4 §2.2, §2.9, §7 item 62](../contracts/m4-dashpay-engine.md), Contract-Version 35 |

## 1. Status

`NetworkSession.invitation_status(link_id)` works on the session, before any wallet exists (F18), and never needs the
vault's key for a plaintext (pre-vault) entry. It reads the link with a new **non-deleting** vault read (DP5-01 D16) and
mirrors the inspection #4997 runs (`inv.rs:478` `invitation_claim_status` → `:712` `inspect_invitation_funding` →
`:967` `inspect_funding_tx`), from the library's public pieces: that call is bound to a wallet and returns no outpoint.

| Status | Detected by |
|---|---|
| `Invalid{Malformed}` | `dw_uri::deeplink::normalize_invitation` (DP5-01's parser), then `crypto.rs:405` `parse_invitation_uri` fails; or the fetched transaction is not the funding transaction in either byte order (`inv.rs:974`), is not an asset lock or has no credit output the voucher key controls (`crypto.rs:546` `voucher_credit_output`), or the link's islock does not decode or does not lock it (`inv.rs:991-1010`). |
| `Invalid{WrongNetwork}` | `crypto.rs:524` `wif_network_matches` is false for the session's network (`inv.rs:847` `ensure_invitation_network`). |
| `Invalid{AssetLockNotFound}` | `rs-sdk/src/core/transaction.rs:99` `Sdk::get_transaction` misses on both byte orders, five attempts 3 s apart, as `inv.rs:769` `fetch_funding_tx_with_retry` does. A transport error is not a miss: it is the call's `platform.*` error. |
| `Claimed` | `Identity::fetch` finds an identity at the prospective id, `ChainAssetLockProof::new(0, outpoint).create_identifier()` (`inv.rs:864`, the same derivation as DP1-02's `identity_for`). One-way, as #4997 says (`inv.rs:421-440`): `false` does not prove the lock unspent, since the inviter's reclaim top-up consumes it without an identity, and Platform has no query for spent asset locks. The claim meets that case (§3). |
| `Expired` | The stash no longer holds the id: its 30-day TTL ran out (DP5-01 D8), or it was dismissed, or this wallet's claim finished and wiped it (§5). The link itself carries **no expiry** at the pin (`crypto.rs:139` `ParsedInvitation`: key, network, txid, islock, inviter), so nothing else expires. |
| `Valid{inviter, funding_duffs, contested_allowed, expires_at}` | Everything else. `funding_duffs` is the voucher's credit output (`inv.rs:956`), what the claim spends; `contested_allowed` is `funding_duffs ≥ CONTESTED_LOCK_DUFFS` (0.25 DASH, DP1-02's floor for a contested name); `expires_at` is the stash entry's. A ChainLock-only link whose transaction is not chain-locked yet is `Valid` (the claim waits for the lock), as #4997 reports it. |

An id of another kind of link is `invitation.invalid`; a malformed id `invalid_argument`; a sealed entry while the
vault is locked `platform.signer_unavailable` (DP5-01 D10).

## 2. The inviter preview

From the link's `du` (username) only: DPNS `domain` lookup of the label (the SDK proves it), then the identity's
DashPay profile (`displayName`, `avatarUrl`). `Valid.inviter` is a `Counterparty` with the identity, the name DPNS
holds, and the profile's fields.

- No `du`, or DPNS has no such name: `inviter: None`. The link's own `display-name` and `avatar-url` are never shown:
  they are the link's word, unproved, and a stranger's link could claim any name.
- The profile read fails or there is none: the inviter with the username alone.
- The DPNS read fails: the whole call fails with its `platform.*` code (the renderer says it can't check the invitation
  now), never a `Valid` that would silently drop the inviter, and with it the request.

## 3. The claim

`start_registration(RegistrationRequest{funding: Invitation{link_id}, label, temporary_label, initial_profile})`. The
row keeps DP1-02's phases, writes and claims; only `Draft → KeysPrepared → IdentityRegistered` differs.

**Start** (`plan`), with no Core-balance check:
1. The wallet already has an identity: `invitation.already_has_identity` (an invitation makes a first identity, as on
   iOS and Android).
2. The status above: `Claimed` → `invitation.claimed`, `Expired` → `invitation.expired`, `Invalid` →
   `invitation.invalid`.
3. A contested label with `contested_allowed` false: `name.unavailable_for_invite`.
4. The credits the plan needs (identity create, the names, the profile, the request when there is an inviter)
   over `funding_duffs × 1000`: `registration.funding_insufficient{needed, available}`, in duffs.
5. The quote: `lock_duffs = funding_duffs`, fee 0, total 0, `available_duffs` 0, and a grant of **`max_duffs` 0** with
   `max_credits` the plan's credits. A `PlatformOp` grant without duffs gives the lease no funding signer at all
   (`lease/session.rs:134`), so nothing in the flow can sign a Core input. A grant with duffs, or a lease issued with
   a funding signer, is refused, `platform.grant_invalid` (D8).
6. The row's `funding`: `{"v":1,"kind":"invitation","link_id":…,"inviter":"<base58>"}` (`inviter` absent without
   one). Never the link.

**Draft → KeysPrepared:** the claim hold, then DP1-02's `assign_index` (the ledger's next free index). The hold is the
funding hold without "SPV caught up": no Core funds move, and the index only needs DP1-05's discovery answered
(`startup_hold`) and DP3-04's trust gate (`writes_wait`, Platform writes wait for masternode sync).

**KeysPrepared (Invitation) → IdentityRegistered:** the claim step, under the `registration/<draft>/identity` step
permit, as DP1-02's identity step:
0. The claim hold again (D25): after a restart the bring-up's discovery comes before the checks below.
1. An identity at the row's index: adopted (a restored row, or a claim whose record was lost).
2. The link from the stash, not deleted: gone → `invitation.expired`, not retryable; sealed while locked → parked for
   Unlock; unparsable → `invitation.invalid`.
3. `inv.rs:532` `claim_invitation(parsed, index, keys, signer)`: keys 0–5 of the index by DP1-01's policy
   (`keys_policy::registration_specs`, `VaultIdentitySigner::derive_public_keys`), the permit's counting identity
   signer for their witnesses. The library refetches the transaction, rebuilds the proof (InstantSend, ChainLock on a
   stale islock, `inv.rs:813`), and signs the transition's outer signature with the **voucher key** in process
   (`put_to_platform_and_wait_for_response_with_private_key`). The wallet's `AssetLockFunding` is not touched; no
   Core transaction is built.
4. Its outcome:
   - the identity → `IdentityRegistered`;
   - `AssetLockNotChainLocked` (a stale islock and a funding block not chain-locked yet) → waits for the ChainLock;
   - a ChainLock link whose funding block is not chain-locked yet → waits for the ChainLock before the call (D21);
   - the outpoint already consumed (`error.rs:1255` `is_asset_lock_already_consumed`) after the library's own
     adoption (`inv.rs:877`: the identity at the prospective id carrying exactly our keys is ours): the engine looks
     the id up itself, up to three times while nothing is found (D24). Our keys there, or a failed lookup → waits and
     claims again, for the library to adopt it; nothing, or other keys → `invitation.claimed`, not retryable;
   - a network mismatch → `invitation.invalid`; the network, a timeout → waits; anything else → DP1-02's `failed`.

| Crash at | On restart |
|---|---|
| `Draft`, before the index | The hold and `assign_index` again. |
| `KeysPrepared`, before the call | The call again. |
| In the call, before Platform took it | The call again; the transaction is the same, Tenderdash's duplicate cache is the SDK's to resolve. |
| After Platform created the identity, before `IdentityRegistered` | Step 1 adopts it once the library has recorded it; otherwise the call again meets "already consumed" and the library adopts the identity carrying our keys (`inv.rs:917`). Never a second identity: one outpoint makes one. |
| `IdentityRegistered` and later | DP1-02's name and profile steps, unchanged; then §4. |

- **Another device** claiming the same link first: our call meets "consumed", the identity there carries other keys,
  so `invitation.claimed`. The same seed on two devices derives the same keys at the same index, so whichever lands
  first is adopted by both.
- **An asset lock already used** (the inviter reclaimed it with a top-up, or any other spend): "consumed" with no
  identity at the prospective id, also `invitation.claimed` (§4 has no separate code; the status's `Claimed` is
  one-way, this is the late detection #4997 describes).
- **No duffs from the wallet, ever:** the grant has no duffs (step 5), the invitation step never calls `fund` or
  `funding_permit`, and the library's claim builds no transaction.
  `a_claim_registers_publishes_and_asks_the_inviter_with_no_duffs` (a funded wallet) and
  `a_wallet_without_coins_or_sync_claims` (an empty one) run the whole flow and assert no funding call, no tracked lock
  and a lease with no funding signer.

## 4. The request to the inviter

After the profile step (or in its place without a profile), an invitation row with an inviter takes `ProfileCreated`
and sends DP2-02's contact request from the new identity to the inviter before `Done`, under the registration's own
lease (its `PlatformOp` grant carries the DashPay crypto signer and the request's `CONTACT_REQUEST_CREDITS`, counted in
the plan). `requests::send` takes the flows a lease may be of; the registration passes `[Registration]`, the facade
keeps `[ContactRequest]`, so no other caller gains a registration lease.

Idempotent across restarts: the send reads the pair's eligibility first; `PendingOutgoing` (our request is on
Platform) and `AlreadyContact` count as sent, and a send whose outcome is unknown is read back (DP2-02). A refusal
positively known to repeat (the inviter has no DashPay keys, not enough credits) is logged and the row goes `Done`:
the identity and the name are what was paid for, and the user can add the inviter from Contacts. The network waits;
a missing authority parks, as every step after the identity.

The request is sent whenever the link names an inviter, as the mobile wallets do; DP5-03 says so before the claim
(F2 for an opt-out).

## 5. Log and memory hygiene

- The link stays in the stash (sealed once a vault exists, DP5-01 D5) until the claim is durable: once a run sees
  `IdentityRegistered` written it calls `forget_invitation(link_id)` (the zeroing delete of DP5-01 D18). A failed
  delete is logged and tried again at the next step and the next run; the TTL bounds it.
- The text read from the stash is `Zeroizing`; the parsed link is the library's `ParsedInvitation`, which wipes its key
  on drop (`crypto.rs:159`) and redacts itself in `Debug`; the voucher's `PrivateKey` is wiped by the library's guard
  (`inv.rs:101`).
- No log, error or event carries the link, the voucher key or the funding txid. Errors name the code only; the
  engine's tracing carries the draft id and phase. DP5-01's redaction (`dw_uri::redact`) still cuts any link text.
- The row's `funding` holds the link id and the inviter's identity, both non-secret.

## 6. Errors and the contract

No new §4 code: `invitation.invalid`, `invitation.claimed`, `invitation.expired`, `invitation.already_has_identity`,
`name.unavailable_for_invite`, `registration.funding_insufficient`, `platform.*` already exist. So no Swift
`engineCodes` change; the contract text changes, so the Swift T0 suite runs.

m4 delta, Contract-Version 34 → 35, item 62:
- §2.9 `invitation_status` implemented (no longer a stub), with §1's detection and §2's preview rules.
- §2.2 registration: `Invitation` funding is implemented; its quote (§3 step 5) and refusals (§3 steps 1–4); an
  `Invitation` row runs (no longer `invitation.invalid` when read); its phases; the request (§4).
- `RegistrationFunding`, `InvitationStatus` and every signature are unchanged, so no ts-rs type changes.

## 7. Tests and evidence

- Unit: every status (`invitation_check_tests.rs`: each malformed reason, wrong network, a miss on both byte orders
  and the retry count, a transport error, claimed, valid with and without an inviter, the ChainLock-only link, the
  stash states); every persisted state and the kill matrix of §3 (`registration_tests/invitation.rs`); restarts; the
  link wiped after `IdentityRegistered` and not before; the request once across restarts.
- Zero duffs: §3, with mutants killed (the already-claimed check skipped; Core funding on a failed claim; the grant's
  `max_duffs` not 0; the wipe before the identity; the link never wiped; D7, D20 and D21 dropped; a `claimed` refusal
  retried; a landed claim not adopted in its step). Record: `/work/scratch/dw-dp5-02/mut/MUTANTS.md`.
- Signing: `dp5_02_claim_signing.rs` builds the IdentityCreate transition with
  `IdentityCreateTransition::try_from_identity_with_signer_and_private_key` (`rs-dpp/.../identity_create_transition/methods/mod.rs:27`,
  what the SDK's `identity_create_with_asset_lock_and_private_key` calls) twice for one voucher key and proof: once with
  the engine's vault signer and DP1-01 keys, once with keys derived independently from the seed and a plain key
  signer. The serialized bytes are equal.
- Test-only `invite create`: dwcli's `invitation-create-test` feature (debug builds only; `compile_error!` otherwise)
  funds an `IdentityInvitation` asset lock from the wallet and encodes the link with `crypto.rs:318`
  `encode_invitation_uri`; the voucher key is derived from the wallet's phrase file at the lock's path (the vault never
  exports a key). `invite_create_gate.rs` proves a release build has no such command and refuses the feature.
- T2 on the dashmate devnet: `/work/scratch/dw-dp5-02/t2/EVIDENCE.md` (outside the repository; the task report has
  its summary).

## Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The claim is an entry state of DP1-02's machine (funding `Invitation`), not a parallel flow: its phases, claims, writes, restart and discard rules hold; only `Draft → KeysPrepared → IdentityRegistered` differs. | Brief; one crash story. |
| D2 | Status mirrors #4997's inspection from public pieces on `NetworkSession`, not `invitation_claim_status`. | F18: a link may be checked before any wallet; the call is wallet-bound and returns no outpoint. The checks are the same (§1 citations). |
| D3 | `Claimed` is the prospective-id check, one-way; a lock spent another way is found at the claim and reported `invitation.claimed`. | Platform has no spent-lock query at the pin (`inv.rs:434-436`). A separate "used" code adds nothing the user can act on. |
| D4 | `Expired` means the stash no longer holds the id. | The link has no expiry at the pin; the stash's TTL is the only one. |
| D5 | `contested_allowed` is `funding_duffs ≥ CONTESTED_LOCK_DUFFS`. | DP1-02's floor for a contested registration; the voucher's value is the tier signal (`inv.rs:458-462`). |
| D6 | The preview trusts DPNS and the profile only, never the link's display name or avatar; a DPNS read failure fails the call. | The link is unproved; a silent `inviter: None` would also drop the request. |
| D7 | Start refuses a wallet that has any identity (`invitation.already_has_identity`). | The mobile wallets' rule; one invitation makes a first identity. |
| D8 | The grant has `max_duffs` 0; the quote's totals are 0; a start with a duff-carrying grant, or a lease issued with a funding signer, is refused (`platform.grant_invalid`, review m5, r2 minor 1). | A lease without duffs has no funding signer: the zero-duff guarantee is structural, not only tested. |
| D9 | The claim hold drops "SPV caught up". | Nothing on Core is spent or chosen; the index needs DP1-05's answer and DP3-04's gate only. On the devnet SPV never catches up (no dip0024 quorum). |
| D10 | Keys are DP1-01's six at the row's index, signed by the permit's counting identity signer; the outer signature is the library's raw-key path with the voucher key. | Same identity shape as a Core-funded registration; the vault holds no voucher key. |
| D11 | An identity at the row's index is adopted before the stash is read. | A restored row on another machine has no link; the seed's identity at that index is the wallet's. |
| D12 | A link gone before the identity is `invitation.expired`, not retryable; a sealed link while locked parks. | Nothing else can make the identity; a lock is the user's to lift. |
| D13 | The link is wiped once `IdentityRegistered` is written, retried at later steps on failure. | §2.9 "the claim finished"; before that a crash needs it. |
| D14 | The inviter is resolved at start and stored in `funding` (non-secret). | The link is wiped before the request step; a restart must still know whom to ask. |
| D15 | The request runs between `ProfileCreated` and `Done`, under the registration's lease (`requests::send` takes the lease's allowed flows); a refusal for good is logged and the row is `Done`. | One lease and one grant for the whole claim; no new column or phase, so no row-format change. |
| D16 | The request is always sent when the link names an inviter. | iOS and Android behaviour; the acceptance. Opt-out is F2. |
| D17 | dwcli: the existing `dashpay invite status/claim` are the commands (status carries the preview); `invite create` exists only with the test feature. | The closest fit to the command table (brief's "or the closest fit"). |
| D18 | Test-only create derives the voucher key from the phrase file, builds the lock with the lease's uncounted funding signer (no Mode B permit). | The vault exports no key; P2b's funding permits are registration-step bound. Test-only code, never in a release build. |
| D19 | dwcli's shared "stub command" for its error-shape tests (`test_stub.rs`) becomes a command the engine refuses (`invite status l`, `invalid_argument`). | `invitation_status` was the last call needing no wallet that was still a stub, and those tests run without one. Stub routing stays covered by dwcli's call table (`dashpay sync` → `DashPay.sync_now`) and the m4 contract test. |
| D20 | An idle invitation row's `waiting` reads the claim hold, not the funding hold. | D9 for the status: an idle claim never shows "waiting for sync". |
| D21 | A ChainLock link (no islock) whose funding transaction is not chain-locked yet waits for the ChainLock before the library is called. | The library refuses that link as invalid data, which would fail the row; the engine's own fetch already knows the lock's state. |
| D22 | Load flakes, rerun alone and passed: dwcli `dashpay_r3` `a_one_shot_command_abandons_a_stalled_engine` and `the_passphrase_is_wiped_before_a_blocked_write` (`init-vault`'s 10 s shutdown deadline missed at load 228); dw-engine `platform::lease::tests::a_failed_step_write_hands_off_no_copy_until_it_is_repaired` (gate run 1, passed alone on try 1); final gate: dw-engine `platform::lease::call_tests::a_call_steps_marker_is_durable_before_its_permit_returns` (BL-181; passed alone on try 1, the shared rerun script's `^lease::` pattern misses the `platform::` prefix), WebKit `contact-pane.spec.ts:43`, `dashpay.spec.ts:139`, `names.spec.ts:210`, `registration.spec.ts:320` ×2 (19 tests alone: all pass), Swift `SPVCoordinatorTests.doneOnlyAfterThePeakDelay` (alone with its target: 100 tests pass). | Untouched by DP5-02; wall-clock bounds under agentbox load (BL-100). |
| D23 | MG-01's operator-signing scan (`dw-protx/tests/no_operator_signing.rs`) allows one exact line of the signing test: the `&dpp::native_bls::NativeBlsModule` argument. | Every dpp IdentityCreate builder takes a BLS module; the claim's transition has no BLS key and an ECDSA outer signature, so the module signs nothing. An exact-text entry, not the whole file, so nothing else in the test can name a BLS type. For review. |
| D24 | A "consumed" refusal is read with the engine's own lookup of the identity at the lock's id (`consumed_lookup`, `consumed_verdict`), up to three lookups 2 s apart while none is found: our keys there or a failed lookup retry the claim; nothing or other keys is `invitation.claimed`. | Review M1: the library reports a failed adoption lookup as the same refusal, which would fail our own landed claim for good. Accepted residual (r2 minor 3): three proved absences from a node a block behind still end `invitation.claimed`; DP1-05's discovery finds the identity later (F4's stored id would let a sweep revive the row). |
| D25 | The claim step waits for the claim hold before D11's check. | Review m2: after a restart at `KeysPrepared` the bring-up's discovery must answer before a gone link (dismissed, or past the TTL) fails a claim that landed unrecorded. |

## Follow-ups

- F1: The mainnet claim (H-07).
- F2: An opt-out of the request to the inviter (D16), and a note on the `Done` row when the request was refused for
  good (D15).
- F3: Use #4997's wallet-bound `invitation_claim_status` instead of the mirror (D2) if Platform adds an unbound form.
- F4 (review m1): D11 adopts any identity at the row's index. The same seed registering at the same index on another
  device at the same time would make the claim adopt that identity. Store the prospective id in the row's `funding`
  and adopt only it, as `create_identity` compares against `identity_for(outpoint)`.
- F5 (review m3, m4): a discard of a claim row after its identity step released signatures, and the stash's 30-day
  TTL on a row parked that long, can leave an unnamed identity. Refuse that discard, and keep the stash entries of
  open claim rows.
- F6 (review m6): CI builds neither `--features invitation-create-test` nor the ignored release check; the DP5-02
  gate does. Add both to CI.
- F7 (review n6, n8, for DP5-03): DPNS proves only that the link's name maps to an identity, not that this user made
  the invitation, so the UI must not show the inviter as verified. `invitation_status` reads `Claimed` for this
  wallet's own claim in flight, so the UI reads the row as well.
