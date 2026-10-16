# DP3-04: the money-move trust gate (DASHPAY §2.2 rule 5)

Status: design for ROADMAP row DP3-04. Builds on E0-10b's provider and positive provenance
(`docs/design/E0-10b-trust-provider.md`, DEC-125, DEC-183), DP3-01's contact payments (`DP3-01-notes.md`), DP1-02's
registration and DP1-06's top-up. H-09 (the trust release gate) makes `Enforcing` the default later; this task
makes the gate correct in both modes now.

## 1. What the rule asks

Rule 5: money never moves on data verified only through the trusted-quorum fallback. Registration, top-up and a
payment to a contact whose channel data or DPNS name was verified only through the fallback wait, behind a short
"Verifying with the network…" step, until the data re-verifies against SPV. In the degraded mode (DEC-168, the
default until #1117 is in the graph) rule 5 is suspended, and a mismatch between SPV and the trusted service still
refuses (rule 4).

"Verified" is E0-10b's positive provenance: once the fallback latch is set, an entity is unverified unless
`dp_trust_verified` holds a record whose digest equals the digest of what the library holds now (DEC-183). Before
the latch nothing is unverified, so the gate passes.

## 2. Every money-moving path

| Path | Where Core money moves | Platform-sourced data that decides where it goes | Gate |
|---|---|---|---|
| Contact payment (`TxDraft::prepare` of a `ContactRecipient`, DP3-01) | the signed payment to the contact's DIP-15 address | the contact requests the relation rests on (the incoming one carries the xpub the address derives from; both bind the key indices), the contact's DPNS name the read models show, its cached profile (the name and picture the user picks the payee by) | `prepare_contact`: the wait before the coin fence and the grant; a re-check after the reservation (§4) |
| "Accept and pay" (`accept_request` then `prepare`, DP3-01 D14) | the payment after the accept | as above; the outgoing request is the one the accept just wrote | the same `prepare_contact` gate (the accept itself moves no Core money) |
| Top-up (`top_up`, DP1-06) | the asset lock's funding | the target identity (an own identity: its id and keys as the library holds them) | `top_up`: the funding hold (§3) and the wait before the identity's write lock and the grant |
| Registration (`start_registration`, `resume_registration`, `start_registration_past_unknown_funding`, DP1-02) | the asset lock's funding | none: the new identity does not exist yet | the funding hold (§3): no funding while Platform writes wait for masternode sync |
| `resume_top_up`, "Finish transfers" (`finish_asset_locks`), the identity-create step of a funded registration | no Core money: they apply a lock already funded to the wallet's own identity | — | not gated (D4) |
| Address sends, PSBT, CoinJoin, masternode and governance flows | the payment | none (the address is the user's or a URI's) | not gated. A `dash:` URI's `user=` is display only: the payment goes to the URI's address (D5) |
| Invitations, withdrawals | — | — | stubs today (DP5, DP6-02); their tasks call the same gate (D6) |

## 3. Decision table

`Rule5` is what the provider's mode and the developer toggle make of rule 5 (`TrustProvider::rule_5`):
`Enforce` (`Enforcing`, toggle off), `Probe` (`Degraded`, toggle off), `Off` (the trusted-only toggle, any mode).

| Case | Outcome |
|---|---|
| Latch not set | proceed (nothing is unverified) |
| Latch set, every entity of the subject verified | proceed |
| `Enforce`, an entity unverified | re-fetch each unverified entity in its own trust scope (E0-10b §4): a fetch that used only SPV-verified quorum keys and equals what the library holds is recorded; the subject is read again from the library; repeat every 3 s until nothing is unverified (proceed) or the wait (60 s) ends: `platform.context_unavailable`. The gate does not wait for masternode sync separately: the provider serves SPV-verified keys for any quorum SPV already holds, so a round may verify before sync ends, and one that cannot simply fails its round. A top-up or registration funding is held before the gate while Platform writes wait (below) |
| `Enforce` or `Probe`, a re-fetch refused as a mismatch | `platform.trust_mismatch` at once, nothing moved; the provider has already sent `Notice{PlatformTrustMismatch}` |
| `Probe` (degraded), an entity unverified | one round of re-fetches, at most 10 s: verified ones are recorded; a mismatch refuses (above); anything else proceeds on trusted-provider data (rule 5 suspended) |
| `Off` (trusted-only), an entity unverified | proceed: SPV is not consulted, so nothing can be compared |
| The subject is gone from the library (a contact no longer a contact, an identity not held) | the existing checks refuse first (`invalid_argument`, `identity.not_found`); if it goes during the wait, `Enforce` refuses `platform.context_unavailable` and `Probe`/`Off` leave it to those checks |

Registration and top-up fundings hold while `TrustProvider::writes_wait()` (Enforcing, toggle off, masternode sync
not done): `PwLib::funding_hold` answers `RegistrationWait::Sync` then, as it does while SPV is not caught up. SPV
caught up and masternode sync are not the same reading (the trust provider's flag is sticky and needs the
masternode phase to have reported), so the hold reads both.

## 4. Where the gate sits

A contact payment, in `prepare_contact` (DP3-01 D4's order, with the gate added):

1. the watch-only check and the live payee check (relation, channel), as before;
2. **the payment lock** (`check_payment_lock`), moved ahead of the wait so that a locked contact refuses at once;
3. **the gate's wait** (§3). It runs after the passphrase (the host authorized the `Spend` grant before `prepare`)
   and before anything is taken: no coin fence (so other sends of the wallet are not held), no grant redeemed (a
   refusal keeps it for a retry within its life), no address reserved. 60 s is half the default grant life
   (`DEFAULT_GRANT_TTL_SECS`, 120 s), so a grant authorized just before `prepare` outlives the wait;
4. the coin fence, the grant or lease checks, contact crypto, the plan (`resolve`, which checks the payment lock
   again), the lease, the Spend charge, the reservation, as before;
5. **a re-check after the reservation** (`payout_address`), under the coin fence (D13): what became unverified
   since the wait (a sync pass replaced the payee's data, or the contact's DPNS name landed and is now shown) gets
   one round of re-fetches bounded like the probe (10 s); anything still unverified, or a mismatch, refuses
   (`platform.context_unavailable`, `platform.trust_mismatch`) and the address is not kept, so no later `prepare`
   pays an address taken from unverified data (one DIP-15 index skipped). The address is kept only once the
   re-check passed, marked checked (D14). The lease and its charge end with the refusal, as for any refusal after
   the reservation (DP3-01 D4).

Under `Enforce`, with the latch set, the payee is also not payable while its paying account was not built from the
request the gate verifies (D12): `platform.context_unavailable` at once. At the wait a queued `RegisterExternal`
exempts it (the reservation's drain is to rebuild the account); at the re-check, after the drain, nothing does.

A reused reserved address that another payment's broadcast claimed between the reservation and the keep is not kept
again, and a fresh one that another payment's give-back displaced is not returned: the payment is
`send.prepared_tx_spent`, as the claim would have refused it.

The broadcast does not run the gate again: the signed transaction pays the address the re-check passed, which
the payee's later changes do not move. The contact's identity key the reservation's drain may fetch to build the
paying account is not a gate entity: the verified request binds it through its encryption (only the holder of the
right key's secret could have encrypted the xpub), so a forged key cannot redirect money; at worst the build fails
or, rarely, yields an xpub nobody controls, which the request's own verification makes a fault, not an attack.

The wait ends early, and the rest of `prepare` then refuses as it would have: on a vault Lock (`send.vault_locked`
from the grant check, the lease's revocation for a lease), and when the session closes. A payment lock written
during the wait is found by `resolve` (`contact.payment_locked{txid}`).

A top-up, in `top_up`: the funding hold first (as before, now with `writes_wait`), the journal check, then the wait
on the identity, then the identity's write lock and everything after it as before. The wait is outside the write
lock so it does not hold the identity's other writes. The destination is the identity id, which a later change of
its keys does not move, so no re-check follows (D3).

A registration has nothing to wait for besides masternode sync; its run already waits on `RegistrationWait::Sync`
and shows "Waiting for the network to sync".

## 5. Waits and timeouts

| What | Value | Why |
|---|---|---|
| Gate wait (`Enforce`) | 60 s | inside the grant's 120 s life; the host shows "Verifying with the network…" while `prepare` runs |
| Retry interval | 3 s | a re-fetch is a few DAPI calls; the background task (`reverify.rs`) may record first, and the next round sees it |
| Probe (`Probe`) | 10 s for the whole round | rule 5 is suspended: the probe only looks for a mismatch, and a slow Platform must not hold a Core payment long |

Each re-fetch runs under the remaining time. A quorum lookup inside it may block its task (the provider's rule-3
wait under `Enforcing`, E0-10b §3, up to 2 minutes; the trusted service's HTTPS call under `Degraded`), which a
timeout around the fetch cannot cut short; so the gate's wait is bounded by 60 s, the probe and the re-check by
10 s, each plus one fetch's blocking lookups (D15). T2 saw a 60 s wait end at 119 s for that reason.

## 6. Refusal codes and wording

No new code. m4 §4 already names the code for "money that would move on data verified only through the fallback":

| Code | When | Wording (for the host, DP3-06 F5) |
|---|---|---|
| `platform.context_unavailable` | the wait ended with an entity unverified (masternode sync unfinished included), a top-up's funding was held on `writes_wait`, or the payee changed to unverified content after the reservation | "Couldn't verify {payee} with the Dash network yet. Nothing was sent. Try again in a minute." |
| `platform.trust_mismatch` | a re-fetch was refused because SPV and the trusted service disagree on a quorum key | "The Dash network and the quorum service disagree about {payee}'s details, so nothing was sent." |

A contact payment reports them as `EngineError::Contact(ContactError::Platform(..))`, like its other Platform
refusals (DP3-01 D16); a top-up as `CreditsError::Platform(..)`.

## 7. The per-contact lock (DP3-01)

The gate never takes, writes or clears a payment lock. It runs after the lock check and before the claim: a locked
or in-flight contact refuses before the wait (`contact.payment_locked`), one locked during the wait refuses in
`resolve`, and the first broadcast's claim (DP3-01 D20) and pre-hand-off row (DP3-01 D23) are unchanged. A gate refusal is before
any hand-off, so it never locks a contact. The dropped reservation of §4 step 5 is the reservation DP3-01 D7 keeps; it is
dropped only when the payee's current content is unverified.

## 8. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The payee's entities are exactly what makes the contacts read model show the contact `unverified` (DP2-01): its profile if cached, each request the relation rests on, the shown DPNS name. A test pins the two together. | Rule 5 names the channel data and DPNS resolution; the profile's name and picture are how the user picks the payee. One definition: what the list shows unverified is what cannot be paid. |
| D2 | No new error code: `platform.context_unavailable` and `platform.trust_mismatch` (m4 §4). | m4 §4 already assigns the code; no §4 table changes, so no Swift `engineCodes` change. |
| D3 | A top-up's subject is the identity and its keys only; its profile and names are not. No re-check after the wait. | The credits go to the identity id. The keys are in the identity's digest, so a key change made through the fallback still makes it unverified and gated. |
| D4 | Applying a lock already funded (resume, "Finish transfers", the identity-create step) is not gated. | No Core money moves; the lock goes to the wallet's own identity, which the keys derive. E0-10b §6 leaves writes-wait for the other write flows to H-09. |
| D5 | Address sends are not gated, a URI's `user=` included. | The destination is not Platform-sourced. |
| D6 | Invitation funding and withdrawals call the same gate when they land. | They are stubs today. |
| D7 | The degraded mode probes: one bounded round of re-fetches for the unverified entities, refusing only on a mismatch. | "A mismatch with SPV still refuses" has no other hook for a contact payment: the payment fetches nothing from Platform. The probe also records what verifies. |
| D8 | The trusted-only toggle proceeds without a probe. | SPV is not consulted, so no mismatch can show; the toggle is a developer's choice. |
| D9 | The gate records verified re-fetches itself, under the wallet manager's read lock, only for content the library still holds unchanged. | E0-10b §5's record step, scoped to the subject: waiting for the 10-minute background task would not fit the wait. |
| D10 | No facade call to verify ahead of the passphrase. | The gate in `prepare` is the choke point and cannot be bypassed. A host-side "verify before Confirm" read is a follow-up for H-09 or the UI (the contact row's `unverified` already tells the host when the step will show). |
| D11 | `kind::PAYMENT` stays without a consumer. | The gate checks the entities that decide where money goes; a record that a payment's txid verified says nothing about that. |
| D12 | Under `Enforce` with the latch set, a payee whose paying account is stale (`external_account_reference` is not the current incoming request's) is not payable: the gate and the re-check treat it as gone. At the gate a queued `RegisterExternal` exempts it; at the re-check (after the reservation's drain) nothing does. | The address derives from the account's stored xpub, not from the request the gate verifies; the library stamps an account with its request only when built from it and every re-key clears the stamp, so the stamp equal to the verified request's is what binds the address to it. A rebuild deferred before a restart is lost with its in-memory queue (review r1 #1), and a drain can leave its op queued and still derive from the old account (r2 #1), hence the strict re-check. `RegisterReceiving` never rebuilds the paying account. Before the latch nothing came through the fallback, so DP3-01's rule stands (r2 #3); the degraded mode keeps it too (rule 5 suspended). Stricter than the library's own trigger (`has_external && !broken && mismatch`): a broken channel is refused earlier, a missing account fails the reservation. |
| D13 | The re-check verifies in one bounded round instead of refusing on anything unverified. | T2: the contact's DPNS name lands in the background after the first contacts read, often between the wait and the reservation, and a no-wait re-check refused every first payment of a session. One round (10 s) verifies it. The coin fence is held across it, so the wallet's estimates, coin locks and other prepares wait up to 10 s plus one fetch's blocking lookups (under `Enforcing` a lookup on a quorum SPV lacks can block for the rule-3 wait, r2 #6); the 60 s wait stays before the fence. Usually nothing is pending and the re-check reads memory only. |
| D14 | A kept reserved address records whether rule 5's re-check passed for it. Under `Enforce`, an unchecked one (kept while rule 5 was off, or given back by a failed hand-off) is dropped and a new one reserved by the next `prepare`. Rule 5 is read once for the reservation and its re-check (r2 #5); the gate's own reading may differ, and either order of a mode change between them is safe (a stricter re-check, or an address kept unchecked). A transaction already prepared is broadcast as prepared: the broadcast does not re-gate, so one prepared under `Probe`/`Off` and broadcast after the mode changed pays its address (r2 #4; the mode changes at run time only through the developer toggle and tests). | An address taken while the toggle or the degraded mode let unverified data through must not be paid once rule 5 applies (review r1 #3). A given-back address is marked unchecked because the hand-off does not carry how it was kept; under `Enforce` this skips one DIP-15 index after a failed hand-off (follow-up: carry the bit on `PreparedTx.contact`, r2 #7). |
| D15 | Correct the bound instead of moving each fetch into its own task. | Review r1 #7: a spawned fetch timed out on its join handle would leave blocked fetches running detached. Fails closed either way; the bound is documented as "plus one fetch's blocking lookups". |
| D16 | Review r1 #2 (a `prepare` dropped between reservation and re-check keeps the address) is not reachable: `TxDraft::prepare` runs in its own task (`on_runtime`), which a host dropping its future does not cancel. Keeping the address only after the re-check closes it anyway. | Checked with a test that aborted the caller: the inner task ran on (and held the coin fence). |
| D17 | `payee_held` keeps its own copy of the contacts read model's precedence. | Sharing one helper means restructuring `owner_contacts` (a shared file DP3-07 and MG-02 also touch); `the_gate_and_the_contacts_row_agree` and `an_unverified_profile_holds_the_payment` pin the two together, and the payee-without-profile/label mutations are caught. |
| D18 | T2 is read-mostly: `t2_testnet_the_money_gate_verifies_a_contact` (`#[ignore]`, public testnet, fixture wallets A and B) syncs A to the tip, latches the fallback, runs the degraded and the enforcing gate on B and on A's identity, then an enforcing `prepare` to B. Nothing is broadcast, so no A→B payment was needed. | The acceptance rests on the gate and `prepare`, not on the broadcast DP3-01/DP3-06 already cover; evidence below. |
| D19 | No Swift `engineCodes` or Swift T0 change. | No m4 §4 table changed and no code was added (D2); m4 item 54 is behaviour only (Contract-Version 28, surface unchanged). |
| D20 | Gate load flake, outside this task: `lease::call_tests::a_call_steps_marker_is_durable_before_its_permit_returns` and `…::a_call_step_dropped_during_its_marker_write_resolves_after_it` failed in the full dw-engine run (1022 s at load 120–200) and passed 20 of 20 rerun alone from the same binary. | This branch does not touch `platform/lease`; both wait on wall-clock windows. |

## 9. Tests (T0)

| Row / property | Test |
|---|---|
| latch not set: proceed | `money_gate_tests::without_the_latch_everything_passes` |
| verified: proceed | `…::a_verified_payee_passes_without_a_fetch` |
| `Enforce`, unverified: re-fetch, record, proceed | `…::enforcing_refetches_records_and_passes` |
| `Enforce`, waits until a round verifies | `…::enforcing_waits_until_spv_verifies` |
| `Probe` bounded by its 10 s | `…::the_degraded_probe_is_bounded` |
| race: content replaced during the wait | `…::content_replaced_during_the_wait_is_verified_anew` |
| session closing ends the wait | `…::a_closing_session_ends_the_wait` |
| subject gone during the wait | `…::a_subject_gone_refuses_only_when_enforcing` |
| an identity verifies by its keys | `…::an_identity_verifies_by_its_keys` |
| `Enforce`, times out | `…::enforcing_refuses_when_nothing_verifies_in_time` |
| `Enforce`/`Probe`, mismatch | `…::a_mismatch_refuses_in_every_mode` |
| `Probe` proceeds, records | `…::degraded_probes_once_and_proceeds` |
| `Off` proceeds, fetches nothing | `…::trusted_only_proceeds_without_a_fetch` |
| differing content never records | `…::a_differing_refetch_records_nothing` |
| race: a verification recorded elsewhere during the wait | `…::a_record_written_during_the_wait_releases_it` |
| race: vault Lock during the wait | `…::a_vault_lock_ends_the_wait` and `contact_tests::a_lock_during_the_gate_wait_moves_nothing` |
| D1: the gate's view equals the contacts row | `contact_tests::the_gate_and_the_contacts_row_agree` |
| no money moves on an unverified payee | `contact_tests::an_unverified_payee_moves_nothing` (no grant redeemed, no address, no broadcast) |
| a mismatched payee moves nothing, either mode | `contact_tests::a_mismatched_payee_moves_nothing_in_either_mode` |
| `Probe` and `Off` pay an unverified payee | `contact_tests::the_degraded_mode_and_the_toggle_pay_an_unverified_payee` |
| the shown DPNS name is a payee entity | `contact_tests::a_shown_username_must_verify_too` |
| accept and pay waits for the new request | `contact_tests::accept_and_pay_waits_for_the_new_request` |
| a payment lock during the wait | `contact_tests::a_payment_lock_during_the_gate_wait_refuses` |
| replaced after the wait: refused, address dropped | `contact_tests::a_payee_replaced_before_the_reservation_is_refused` |
| replaced after the reservation: refused, not kept | `contact_tests::a_payee_replaced_after_the_reservation_is_refused` |
| the re-check verifies in one round, refuses what does not verify | `money_gate_tests::the_recheck_verifies_what_appeared_since_the_gate`, `…::the_recheck_refuses_what_does_not_verify_in_its_round` |
| D12 stale paying account | `contact_tests::a_stale_paying_account_is_not_paid` (latch, `RegisterReceiving` queued), `…::a_stale_account_the_drain_leaves_is_not_paid` (strict re-check) |
| D13 a name landing before the re-check | `contact_tests::a_name_landing_before_the_recheck_is_verified_there` |
| a reused address claimed during the re-check | `contact_tests::an_address_claimed_before_it_is_kept_is_not_kept_again` |
| D14 unchecked kept address | `contact_tests::an_address_kept_without_rule_5_is_not_paid_under_it` |
| D1 cached profile | `contact_tests::an_unverified_profile_holds_the_payment` |
| top-up of an unverified identity moves nothing | `topup_tests::an_unverified_identity_is_not_topped_up` |
| degraded top-up proceeds | `topup_tests::the_degraded_mode_tops_up_an_unverified_identity` |
| funding hold reads `writes_wait` | `topup_tests::the_funding_hold_waits_for_platform_writes` |
| mutation | each gate call removed or weakened in turn makes at least one of the above fail (`mutate.sh`, recorded in the evidence) |

## 10. Evidence (2026-10-16)

- **Gate** (head 1aa8beda, then the UI-only follow-merge 975a4fa8): fmt, clippy (workspace and dw-app
  `selftest`) clean; tests passed 2048, failed 2 (D20, each 20/20 alone); tsc clean; vitest 58 files /
  823 tests on 1aa8beda and 60 / 855 on 975a4fa8. The merge changes no file under `rust/`. No Playwright
  spec is affected: this task changes no UI.
- **Mutations** (`mutate.sh` on 1aa8beda): 19 of 19 caught. Each removes or weakens one gate call or
  check (prepare gate, reservation re-check, tainted or unchecked kept address, stale paying account and its
  latch, the claimed slot, the re-check round, payee profile and label, top-up gate, `writes_wait` hold,
  probe, mismatch, `Enforce`, scoped tally, lock). One first draft of the claimed-slot mutation could not
  fail by construction and was replaced.
- **T2** (D18, public testnet, `t2_testnet_the_money_gate_verifies_a_contact`):

  | Head | B unverified before | Degraded probe | Enforcing gate | Enforcing `prepare` |
  |---|---|---|---|---|
  | b988c88b | no | 0.3 ms | pass, attempt 1 | 66 ms, pass |
  | b988c88b | yes | 134 ms, verified B | pass, attempt 1 | 132 ms, pass |
  | 5bbfb646 | yes | 155 ms, verified B | pass, attempt 1 | 171 ms, pass |

  Each `prepare` paid B's contact address yYyXvcRUyj12iqbpvWsAtXgenAM9YKzax9 (txid cbe51e9a…, never
  broadcast). An earlier head (dc87f1ca) refused at `prepare` because B's name landed between the gate and
  the reservation (D13). Another run right after masternode sync waited 119 s and moved nothing (§5, D15).
