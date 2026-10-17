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
| Latch not set | proceed (nothing is unverified), after the destination check below |
| Latch set, every entity of the subject verified | proceed |
| `Enforce`, an entity unverified | re-fetch each unverified entity in its own trust scope (E0-10b §4): a fetch that used only SPV-verified quorum keys and equals what the library holds is recorded; the subject is read again from the library; repeat every 3 s until nothing is unverified (proceed) or the wait (60 s) ends: `platform.context_unavailable`. The gate does not wait for masternode sync separately: the provider serves SPV-verified keys for any quorum SPV already holds, so a round may verify before sync ends, and one that cannot simply fails its round. A top-up or registration funding is held before the gate while Platform writes wait (below) |
| `Enforce` or `Probe`, a re-fetch refused as a mismatch | `platform.trust_mismatch` at once, nothing moved; the provider has already sent `Notice{PlatformTrustMismatch}` |
| `Probe` (degraded), an entity unverified | one round of re-fetches, at most 10 s: verified ones are recorded; a mismatch refuses (above); anything else proceeds on trusted-provider data (rule 5 suspended) |
| `Off` (trusted-only), an entity unverified | proceed: SPV is not consulted, so nothing can be compared |
| A payee whose paying account was built from another request than their current one, or (after the reservation) whose reserved address is not in it (D12, D21) | `platform.context_unavailable` in every setting, latch or not (D25): at the wait nothing is taken, after the reservation the address is dropped; a DashPay sync is started so the library's sweep queues the rebuild a retry needs |
| A paying account with no stamp at all (before its first rebuild after the upgrade to the store that keeps the stamp, or after a crash between its re-key and its rebuild, D28) | refused as above, in every setting, latch or not (D28, which replaces D26) |
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
   pays an address taken from unverified data (one DIP-15 index skipped). Each of the re-check's reads also reads
   the reserved address with the request it checks: an address no longer in the payee's paying account refuses
   the same way (D21). Under `Probe` and `Off` the re-check fetches nothing but still reads the payee once for these
   destination checks (D25). The address is kept only once the re-check passed, marked checked (D14). The lease and its charge end with the refusal, as for any refusal after
   the reservation (DP3-01 D4).

In every rule 5 setting, with or without the latch (D25), the payee is also not payable while its paying account
was built from another request than their current one (D12), or while it carries no stamp (D28):
`platform.context_unavailable` at once, and a DashPay sync is started so the library's sweep queues the
rebuild. At the wait a queued `RegisterExternal` exempts it (the reservation's drain
is to rebuild the account); at the re-check, after the drain, nothing does.

The stamp names the request an account was built from, not the address taken from it. The wallet manager's lock is
released between the reservation and the re-check, and a sync pass can meanwhile replace the payee's request (a
rotation), tear the paying account down and the drain rebuild it from the new request: the stamp then matches the
new request, which may verify, while the reserved address came from the old account. So the re-check's every read
also checks, under the same wallet-manager read lock as the stamp and the request, that the reserved address is in
the address pool of the paying account the library holds now (D21). A rebuild from a new xpub holds none of the old
addresses; one from the same xpub holds those within the window it pre-generates (the gap limit), and such an
address is still one the verified request designates. An older index beyond that window refuses, fail-closed.

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
| `platform.context_unavailable` | the wait ended with an entity unverified (masternode sync unfinished included), a top-up's funding was held on `writes_wait`, the payee changed to unverified content after the reservation, or the payee's paying account is not the one their current request designates (D25) | "Couldn't verify {payee} with the Dash network yet. Nothing was sent. Try again in a minute." |
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
| D7 | The degraded mode probes: one bounded round of re-fetches for the unverified entities, refusing only on a mismatch. | "A mismatch with SPV still refuses" has no other hook for a contact payment: the payment fetches nothing from Platform. The probe also records what verifies. It is not protection against a compromised fallback service (the release-decision note below §8's table). |
| D8 | The trusted-only toggle proceeds without a probe. | SPV is not consulted, so no mismatch can show; the toggle is a developer's choice. |
| D9 | The gate records verified re-fetches itself, under the wallet manager's read lock, only for content the library still holds unchanged. | E0-10b §5's record step, scoped to the subject: waiting for the 10-minute background task would not fit the wait. |
| D10 | No facade call to verify ahead of the passphrase. | The gate in `prepare` is the choke point and cannot be bypassed. A host-side "verify before Confirm" read is a follow-up for H-09 or the UI (the contact row's `unverified` already tells the host when the step will show). |
| D11 | `kind::PAYMENT` stays without a consumer. | The gate checks the entities that decide where money goes; a record that a payment's txid verified says nothing about that. |
| D12 | In every rule 5 setting, with or without the latch (amended by D25, Sol r2), a payee whose paying account is stale (`external_account_reference` is not the current incoming request's; an absent one counts too, D28) is not payable: the gate and the re-check treat it as gone. At the gate a queued `RegisterExternal` exempts it; at the re-check (after the reservation's drain) nothing does. | The address derives from the account's stored xpub, not from the request the gate verifies; the library stamps an account with its request only when built from it and every re-key clears the stamp, so the stamp equal to the verified request's is what binds the address to it. A rebuild deferred before a restart reloads with its queue but may drain without stamping (D25), and a drain can leave its op queued and still derive from the old account (r2 #1), hence the strict re-check. `RegisterReceiving` never rebuilds the paying account. The first draft applied it only under `Enforce` with the latch set, since before the latch nothing came through the fallback (r2 #3); Sol r2 found that pays the account a crash leaves unrebuilt (D25). Stricter than the library's own trigger (`has_external && !broken && mismatch`): a broken channel is refused earlier, a missing account is refused at the gate under `Enforce` unless its rebuild is queued, and fails the reservation if the drain does not rebuild it. |
| D13 | The re-check verifies in one bounded round instead of refusing on anything unverified. | T2: the contact's DPNS name lands in the background after the first contacts read, often between the wait and the reservation, and a no-wait re-check refused every first payment of a session. One round (10 s) verifies it. The coin fence is held across it, so the wallet's estimates, coin locks and other prepares wait up to 10 s plus one fetch's blocking lookups (under `Enforcing` a lookup on a quorum SPV lacks can block for the rule-3 wait, r2 #6); the 60 s wait stays before the fence. Usually nothing is pending and the re-check reads memory only. |
| D14 | A kept reserved address records whether rule 5's re-check passed for it. Under `Enforce`, an unchecked one (kept while rule 5 was off, or given back by a failed hand-off) is dropped and a new one reserved by the next `prepare`. Rule 5 is read once for the reservation and its re-check (r2 #5); the gate's own reading may differ, and either order of a mode change between them is safe (a stricter re-check, or an address kept unchecked). A transaction already prepared is broadcast as prepared: the broadcast does not re-gate, so one prepared under `Probe`/`Off` and broadcast after the mode changed pays its address (r2 #4; the mode changes at run time only through the developer toggle and tests). | An address taken while the toggle or the degraded mode let unverified data through must not be paid once rule 5 applies (review r1 #3). A given-back address is marked unchecked because the hand-off does not carry how it was kept; under `Enforce` this skips one DIP-15 index after a failed hand-off (follow-up: carry the bit on `PreparedTx.contact`, r2 #7). |
| D15 | Correct the bound instead of moving each fetch into its own task. | Review r1 #7: a spawned fetch timed out on its join handle would leave blocked fetches running detached. Fails closed either way; the bound is documented as "plus one fetch's blocking lookups". |
| D16 | Review r1 #2 (a `prepare` dropped between reservation and re-check keeps the address) is not reachable: `TxDraft::prepare` runs in its own task (`on_runtime`), which a host dropping its future does not cancel. Keeping the address only after the re-check closes it anyway. | Checked with a test that aborted the caller: the inner task ran on (and held the coin fence). |
| D17 | `payee_held` keeps its own copy of the contacts read model's precedence. | Sharing one helper means restructuring `owner_contacts` (a shared file DP3-07 and MG-02 also touch); `the_gate_and_the_contacts_row_agree` and `an_unverified_profile_holds_the_payment` pin the two together, and the payee-without-profile/label mutations are caught. |
| D18 | T2 is read-mostly: `t2_testnet_the_money_gate_verifies_a_contact` (`#[ignore]`, public testnet, fixture wallets A and B) syncs A to the tip, latches the fallback, runs the degraded and the enforcing gate on B and on A's identity, then an enforcing `prepare` to B. Nothing is broadcast, so no A→B payment was needed. | The acceptance rests on the gate and `prepare`, not on the broadcast DP3-01/DP3-06 already cover; evidence below. |
| D19 | No Swift `engineCodes` or Swift T0 change. | No m4 §4 table changed and no error code was added (D2); m4 item 55 is behaviour only (Contract-Version 29, surface unchanged). |
| D20 | Gate load flake, outside this task: `lease::call_tests::a_call_steps_marker_is_durable_before_its_permit_returns` and `…::a_call_step_dropped_during_its_marker_write_resolves_after_it` failed in the full dw-engine run (1022 s at load 120–200) and passed 20 of 20 rerun alone from the same binary. | This branch does not touch `platform/lease`; both wait on wall-clock windows. |
| D21 | The re-check binds the reserved address to the paying account the library holds now: under the read lock of each of its reads, an address not in that account's address pool makes the payee not payable (`platform.context_unavailable`, the address dropped). Checked in the reads, not in `record`, so what verified is still recorded. It applies in every rule 5 setting, with or without the latch (D25): a kept address whose account a rotation replaced then refuses once (`platform.context_unavailable`) and is dropped, so a retry reserves from the current account. Under `Probe` and `Off` the re-check fetches nothing (D7, D8) but reads the payee once for this. The binding inherits the soundness of the library's stamp (possible security note: see manager). | Sol r1 #1 (Opus r3 #7): the stamp alone let a verified new request pass an address from the account it replaced. Pool membership is the library's own record of what the current account derives: a rotation removes the immutable account and its pool together and the rebuild installs both (the registration exits early while a pool exists). |
| D22 | The re-check's last read is a point in time, as the gate is: a rotation after it does not change the signed transaction, which the broadcast does not re-gate (§4). | The output pays the address the verified request designated at the last re-check; the payee's later changes do not move it, and re-gating at broadcast would refuse a payment whose destination was checked. |
| D23 | Sol r1 rules Opus r3 #6 (narrow the queued-`RegisterExternal` exemption to one whose ciphertext matches), r2 #7 (carry the checked bit on `PreparedTx.contact`) and r1 #8 (gate before the cheap refusals) hardening follow-ups, not blockers. | Each fails closed today (Sol r1, "Requested rulings"). |
| D24 | DP1-10 also claimed m4 item 54 / Contract-Version 28 and merged first (main 5b90e25b); at that follow-merge this task's item became 55 and its version 29. | Manager's ruling on Sol r1. |
| D25 | The stale-account check (D12) and the address binding (D21) are destination checks, not trust checks. They refuse with `platform.context_unavailable` in every rule 5 setting (`Enforce`, `Probe`, `Off`), with or without the latch: at the gate, before anything is taken (the queued-`RegisterExternal` exemption kept), and in the re-check, strictly, with the address dropped. On that refusal the engine starts a DashPay sync (`kick_dashpay_sync`, which does nothing while Platform is not running); the library's sweep queues the rebuild for an account whose marker does not match its request, and the retry's reservation drains it. | Sol r2 (Major): a crash between persisting a rotated request and the sweep leaves the marker cleared and the old account in place, with no latch, and the latch-gated check paid an address from an account the payee's current request no longer designates. Which account the money goes to is not a trust question, so a stamp naming another request and an address outside the pool refuse in every mode; what an absent stamp costs is D26, replaced by D28. The sync is the library's own rebuild path; no unit test observes it, since the fixtures run without Platform. A refusal can take two retries to converge: an op queued before a restart, drained while the old account is still in place, registers as already existing and does not stamp, and the next sweep rebuilds it. |
| D26 | Replaced by D28 (DP3-04b), once the store kept the stamp. As built in DP3-04: an absent stamp is staleness under `Enforce` only. `Probe` and `Off` refuse a stamp naming another request and an address outside the current pool (D21), and proceed on an absent stamp. | The library's SQLite store has no column for `external_account_reference` (`rs-platform-wallet-storage` `schema/contacts.rs`: it "restores `None`, which conservatively forces the next sweep to re-verify"), so every paying account loads unstamped after every launch until the first DashPay pass tears it down and a drain rebuilds and stamps it, and a rotation whose rebuild was not drained looks the same. Failing closed on the absence would refuse contact payments after each launch until that pass, and every one while Platform is unreachable or with `--no-platform` (Opus r5 #1), so the ruling's condition ("if it costs nothing on the normal path") does not hold for `Probe` and `Off`, and they keep D7's reading: refuse on a mismatch, proceed on absence. `Enforce` (the ruling's case, tests and H-09) pays that cost: a contact payment before the launch's first completed DashPay pass is refused (the bring-up runs one before its drain, and after it the queued rebuild passes the gate), and so is every contact payment while Platform is not running, verified payees included. Persisting the stamp in the library is a prerequisite for H-09 making `Enforcing` the default, unless H-09 accepts the offline refusal. A stamp whose store write keeps failing makes every retry refuse, kick, rebuild and refuse again: closed, not converging. For the release decision: under the degraded default a rotation whose rebuild had not been drained and stamped (a crash before the sweep, a quit between the sweep's in-memory teardown and the drain, or a drain that raced the rotation) can pay the payee's previous account until the next completed DashPay pass; offline, for the whole session (Opus r6 #1). |
| D28 | An absent stamp is a stale paying account in every rule 5 setting, latch or not (DP3-04b): at the gate, with the queued-`RegisterExternal` exemption kept, and at the strict re-check, `platform.context_unavailable` and a DashPay sync started. This replaces D26's `Enforce`-only reading; the gate no longer reads rule 5 for the destination. | With D29's pin the store keeps `external_account_reference` (V019, written by the library's `note_external_account_registered`), so a paying account reloads with the stamp it was last built with and a launch no longer rebuilds every account. An account is then unstamped only (a) before its first rebuild after the upgrade: rows from an older build load `NULL`, and the next completed DashPay pass rebuilds and stamps them; (b) after a crash between the re-key, which persists the new request with no stamp, and the rebuild, when the account is still the old one; (c) a contact accepted in this session before its drain, which the queued `RegisterExternal` exempts at the gate as before. Persistence alone does not close D26's degraded window: (b) is exactly what D26 let `Probe` and `Off` pay. It closes because refusing the absence no longer costs anything on the normal path, which was the ruling's condition. The cost left is (a), once per contact: offline or with `--no-platform`, a payment to a contact stamped before the upgrade is refused until a DashPay pass completes, in every setting. After that a relaunch pays it offline too. For H-09: BL-189 (the stamp persisted) is met, so making `Enforcing` the default no longer brings an offline refusal after every launch, and the degraded default no longer pays a crash's old account. What remains for H-09 is the degraded default's trust model (D7) and (a). A stamp whose store write keeps failing still refuses and kicks on every retry (D26): closed, not converging. |
| D29 | Every PastaPastaPasta/platform rev in `rust/Cargo.toml` moves together to `dw/dp3-04b-pool-rotation`@f2f15b55ca: the pin before, ebe37f8a67, plus the `rs-platform-wallet-storage` fix (V019). (1) A DashPay external account whose xpub changes has its pool rows retired, in the flush's one transaction, into `core_address_pool_retired`, keyed by the old xpub with their `used` flags. A pool row is placed by the xpub it derives from: the current xpub goes live; an earlier one (retired, or registered earlier in the same flush) goes to history; any other is a typed-key conflict as before. Rotating back restores that generation. `load()` never puts a retired row into a pool or the used set, so an old address stays known (`retired_pool_addresses`) and is never payable. (2) The contact's stamp is persisted. BL-187 is unchanged: the rust-dashcore rev stays 8fe0a381, exactly platform's (its `Cargo.toml` and `Cargo.lock` are untouched on the branch), and the `[patch]` to rust-dashcore-dashpay d3d50520a stays. | Sol r3's High finding: a new-xpub rotation's `RegisterExternal` rolled back on every retry as a `TypedPoolKeyConflict` on index 0, so the rotated contact could never be paid, and each relaunch reloaded the old, unstamped account. The new dw-engine test runs the real SQLite persister through the rotation and a relaunch; on the old pin its store fails with that error. V019 is also claimed by other open platform branches; the number is to be agreed before an upstream PR (ask). |
| D30 | A refusal on the paying account starts no DashPay sync while a pass is in flight (`rebuild_soon` checks `dashpay_sync().is_syncing()`). | Sol r3 note (Opus r5 #5): `sync_now` skips a pass already running, so each retry in a burst only spawned a task. It is one condition; a refusal while no pass runs still starts one. |
| D27 | Opus r5 and r6 on the Sol r2 round (r6 applied in full: the D26 wording, D12's clauses, the absent stamp in the drain-leaves test): #1 (the stamp is not persisted) became D26; #2 (doc drift), #3 (the strict re-check in every setting), #4 and #6 (the destination-only re-check under `Probe`/`Off`) are applied. #5 (no debounce on the sync a `NotPayable` refusal starts) is a follow-up, as is persisting the stamp in `rs-platform-wallet-storage`, which removes D26's cost. DP3-04b did both (D29, D30). | A pass already in flight absorbs further kicks (`sync_now`), the refusal comes before the grant is redeemed, and after D26 only `Enforce` or a real mismatch refuses, so a retry loop costs passes, not money. The stamp's column is an upstream schema change. |

**Release decision: the degraded default (D7).** The degraded mode is acceptable only under its trust model: the
trusted quorum service and proof-bearing Platform responses stay trusted while SPV cannot verify, and a disagreement
SPV finds is an extra refusal. Its probe proceeds on absence, timeout or an unverified result, so whoever controls the
trusted responses can withhold or delay the comparison, or supply a false quorum key while SPV holds none, and so
redirect a contact payment or a top-up. The probe does not protect against a compromised fallback service. Keeping
`Degraded` as the default (DEC-168) is a product and release decision, raised with pasta; this task does not change
it. A second exposure under the degraded default (D26): a payee's rotation whose rebuild had not been drained and
stamped (a crash before the sweep, a quit between the sweep's in-memory teardown and the drain, or a drain that raced
the rotation) can pay the payee's previous account until the next completed DashPay pass, and offline for the whole
session; `Enforcing` refuses it. DP3-04b withdraws this second exposure (D28): with the stamp kept by the store, every
setting refuses an unstamped account, and the one-time cost is a contact payment refused offline until the first
DashPay pass after the upgrade.

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
| D12 stale paying account | `contact_tests::a_stale_paying_account_is_not_paid` (no latch, then latch with `RegisterReceiving` queued), `…::a_stale_account_the_drain_leaves_is_not_paid` (strict re-check) |
| D13 a name landing before the re-check | `contact_tests::a_name_landing_before_the_recheck_is_verified_there` |
| a reused address claimed during the re-check | `contact_tests::an_address_claimed_before_it_is_kept_is_not_kept_again` |
| D21 an address from a replaced account | `contact_tests::an_address_from_a_replaced_account_is_not_paid` (rotation and rebuild at the reservation, the new request verifiable; no fetch beyond the gate's, so the binding refused) |
| D21 control: the same xpub rebuilt | `contact_tests::an_address_the_rebuilt_account_still_holds_is_paid` |
| D21 without the latch: the account torn down | `contact_tests::an_address_from_a_torn_down_account_is_not_paid` (under `Enforce`, `Probe` and `Off`, D25) |
| D25 an account a crash left unrebuilt | `contact_tests::an_account_a_crash_left_unrebuilt_is_not_paid` (rotated request persisted, marker cleared, no latch, `Enforce`; refused, then paid once rebuilt and stamped) |
| D28 an unstamped account per setting | `contact_tests::an_unstamped_account_is_not_paid_in_any_setting` (the upgrade's state and the crash's: refused, nothing reserved, under `Enforce`, `Probe` and `Off`) |
| D29 a rotation through the real store, relaunched | `contact_tests::a_rotation_the_store_kept_is_what_a_relaunch_pays` (SQLite persister: the accept, then a new-xpub rotation stored, a relaunch; the new account and a matching stamp reload, the new pool holds the new address and not the old; a kept old address is refused and dropped in every setting, or replaced under `Enforce` when unchecked (D14); then the new account is paid) and the storage tests of `rs-platform-wallet-storage` (`tests/sqlite_dashpay_external_rotation.rs`) |
| D12/D25 the strict re-check in every setting | `contact_tests::a_stale_account_the_drain_leaves_is_not_paid` (stamp `Some(77)` and no stamp refused everywhere, D28) and the no-latch part of `…::a_stale_paying_account_is_not_paid` run under `Enforce`, `Probe` and `Off` |
| a fresh address displaced before its keep | `contact_tests::a_fresh_address_displaced_before_it_is_kept_is_not_paid` (the first payment's give-back lands between the second's reservation and keep) |
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
- **Sol r1 round** (2026-10-17, head 7402d6b1, after the follow-merge of main 5b90e25b): GATE EXIT 0. fmt, clippy
  (workspace and dw-app `selftest`) clean; tests passed 2053, failed 2, both wall-clock tests in `e0_05_bringup`
  (`start_spv_returns_at_once_and_spv_starts_after_the_bring_up`, `close_reports_a_key_read_it_could_not_wait_for`)
  that the gate reran alone and passed on the first try; tsc clean; vitest 63 files / 895 tests. Load-flake reruns
  for BL-181, in `platform/lease` (untouched here): on c2c5625c
  `lease::tests::a_dropped_admit_or_register_keeps_the_fence_consistent` failed in the full run ("expected First, got
  Deferred") and 2 of 20 alone at load ~230; it passed in the 7402d6b1 run. Mutations: 22 of 22 caught on 6ca994b2
  (`mutate-r1.sh`: the 19 above with the corrected claimed-slot text, plus displaced-fresh-ignored,
  account-binding-ignored, account-binding-always-refuses), and account-binding-absent-pays caught on c2c5625c by the
  no-latch teardown test.
- **Sol r2 round** (2026-10-17, head 5652f117, after the follow-merge of main e27e9e95): GATE EXIT 0. fmt, clippy
  (workspace and dw-app `selftest`) clean; tests passed 2060, failed 1, the `e0_05_bringup` wall-clock test
  `close_reports_a_key_read_it_could_not_wait_for`, which the gate reran alone and passed on the first try; tsc clean;
  vitest 64 files / 900 tests. No BL-181 lease flake this run; in an earlier targeted run (load ~300)
  `lease::session_tests::r1_f3_a_hanging_hand_off_ends_at_the_deadline_as_maybe_sent` and
  `trust_tests::after_masternode_sync_the_degraded_mode_falls_back_at_once` failed and passed alone (neither file is
  touched here). The gate ran without the shared sccache wrapper (`RUSTC_WRAPPER=`): the shared server had stalled
  (2 compiles in 30 s). Mutations: `mutate-r2.sh` 4 of 4 caught on 661b33b5, then `mutate-r2b.sh` 6 of 6 on 0a143899
  (stamp ignored, absent stamp paid under `Enforce`, absent stamp refused everywhere, the `Probe`/`Off` re-check
  skipped, the gate's destination check skipped, an absent account binding). T2 on 5652f117 from a fresh launch
  (the stamp reloads absent, D26): passed (row below).
- **T2** (D18, public testnet, `t2_testnet_the_money_gate_verifies_a_contact`):

  | Head | B unverified before | Degraded probe | Enforcing gate | Enforcing `prepare` |
  |---|---|---|---|---|
  | b988c88b | no | 0.3 ms | pass, attempt 1 | 66 ms, pass |
  | b988c88b | yes | 134 ms, verified B | pass, attempt 1 | 132 ms, pass |
  | 5bbfb646 | yes | 155 ms, verified B | pass, attempt 1 | 171 ms, pass |
  | 5652f117 | no | 0.3 ms | pass, attempt 1 | 69 ms, pass |

  Each `prepare` paid B's contact address yYyXvcRUyj12iqbpvWsAtXgenAM9YKzax9 (txid cbe51e9a…, never
  broadcast). An earlier head (dc87f1ca) refused at `prepare` because B's name landed between the gate and
  the reservation (D13). Another run right after masternode sync waited 119 s and moved nothing (§5, D15).
