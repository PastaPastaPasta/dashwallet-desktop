# H-03: the kill -9 equivalence matrix

ROADMAP H-03: "Kill -9 equivalence (DESIGN G5) across registration, request, payment and profile". Acceptance:
"Equal state ≤ 60 s after restart". DESIGN-opus §6 G5: kill -9 at any point of a sync, send or registration, then
restart; the state must equal the reference (balances, history, identities, tokens, DashPay overlay) once the
re-sync ran, within 60 s. Written for the H-01 security review (11-27).

Base: `m3/r2-governance` at `306f1883`.

## How to read it

Rows are flows. Columns are kill points:

| | Kill point |
|---|---|
| **K1** | Before signing. |
| **K2** | After signing, before the durable record. |
| **K3** | After the durable record, before the broadcast. |
| **K4** | After the broadcast, before its confirmation (outcome, Platform's answer, the next phase) is stored. |
| **K5** | During a restart's replay (the SPV rescan the engine owes, or the DashPay pass that re-reads Platform). |

Each cell gives the state expected after the restart, then its coverage:

- **Covered**: an existing test kills there, restarts and checks that state. *In-process* means the kill is a
  task abort plus a session close and reopen on the same data directory (the registration fixture's
  `Fixture::kill` and `reopen_with`, which also puts back a `VACUUM INTO` copy of `dispatch.sqlite` taken at the
  kill).
- **New**: added by this change. *Real kill*: a child process (the test binary itself) is stopped at a named point
  with SIGKILL, and the parent opens the same data directory and checks the state within 60 s.
- **Gap**: no test. The reason is given.
- **Defect**: a test shows the state differs after the restart. It is checked in as `#[ignore = "H-03: …"]`;
  the write-ups are under "Defects".
- **n/a**: the flow has no such point.

"Not equal offline" in a DashPay cell means the state comes back only from Platform. The bring-up reads it again
at every start (`bringup.rs` step 1, `start_wallet_subsystems`: identity, contact requests and contact accounts,
within 20 s, or 3 s for a wallet created here). Offline tests cannot run that re-sync, so those cells rely on the
bring-up's own tests plus what the library stored before the kill.

## The matrix

### 1. Identity registration (Core-funded asset lock → IdentityCreate → name → profile)

| | Expected after restart | Coverage |
|---|---|---|
| K1 | Crash: the funding step's marker stands with its durable-record flag and reads `NotSent`. The row closes `Cancelled(registration.funding_cancelled)`, nothing is committed, no lock is built and a new start is allowed (C5). Before a later step (identity, name, profile): the step runs again once. | **Covered (in-process).** `platform::registration_tests::a_crash_before_the_funding_signature_closes_its_row` (dispatch.sqlite snapshot); `…::a_kill_at_any_transition_resumes_to_one_registration` (`RegisterIdentity`, `RegisterName`, `CreateProfile` × `Before`); `platform::lease::call_tests::the_kill_matrix_of_a_funding_call_step` (killed before the marker; killed after the marker, before the first signature). |
| K2 | The funding signature was released but the lock was not recorded (the tap's write or P2b's signed-lock record never landed). The step reads `MaybeSent` and is never funded again. The call's own lock goes to its row once it is seen (MN-1), or the row waits `FundingUnknown` and "Finish transfers" recovers the lock. Exactly one lock. | **Covered (in-process).** `call_tests::the_kill_matrix_of_a_funding_call_step` (`kills_released`, `kills_signed`); `lease::mode_b_tests::r6_01_a_released_call_with_no_recorded_lock_reads_maybe_sent`, `…::a_kill_mid_call_that_signed_nothing_reads_not_sent_and_refuses_the_step`; `registration_tests::an_untapped_index_zero_lock_is_its_calls_own`, `…::an_untapped_index_one_lock_is_its_calls_own`, `…::a_lock_record_that_fails_is_retried_then_left_to_its_row`, `…::finish_gives_a_running_calls_lock_to_its_row`; `registration_tests::rules::review_r6_own_lock_proof_race_must_not_wedge`. |
| K3 | The lock is tracked and its outpoint is on the row before the broadcast (Restore rule 3), phase `FundingSent`. The flow resumes to one lock, one identity, one name and one profile. | **Covered (in-process).** `registration_tests::a_kill_at_any_transition_resumes_to_one_registration` (`Fund` × `Tracked`); `journal_tests::a_tracked_registration_lock_reaches_its_flow_before_the_broadcast`. |
| K4 | Broadcast or Platform call done, the next phase not stored. The flow resumes to exactly one of each: no second lock, identity, name or profile (MN-3), and one credit top-up for a top-up. | **Covered (in-process).** `registration_tests::a_kill_at_any_transition_resumes_to_one_registration` (`Fund` × `After`, `WaitProof` × both, `RegisterIdentity`, `RegisterName`, `CreateProfile` × `After`); `registration_tests::step2::step2_a_parked_flow_whose_first_attempt_landed_signs_nothing_more`, `…::step2_a_landed_attempt_behind_a_locked_resign_records_the_identity`; `topup_tests::a_crash_after_the_lock_is_tracked_is_resumed`, `…::a_crash_before_the_apply_is_finished_by_finish_transfers`, `…::a_crash_after_platform_took_the_lock_adds_the_credits_once`. |
| K5 | A replay holds new asset-lock funding until it reaches its target, then the flow goes on. Journal recovery runs the resume pass again. | **Covered (in-process).** `registration_tests::a_funding_refused_during_a_replay_waits_for_sync`; `topup_tests::a_replay_holds_asset_lock_funding_until_it_reaches_its_target`; `lease::resume_tests::journal_recovery_reruns_the_pass`, `…::a_pass_sends_no_lock_before_its_durable_record`. |

No registration cell has a real-process kill. The fixture's scripted library keeps Platform's side (locks,
identities, names) in memory, so a child process could not hand it on. The in-process kill aborts the flow's task
and closes the session, so no destructor of the flow writes anything after the kill point. K1 also puts back the
dispatch journal as the crash left it.

### 2. Contact request: send

| | Expected after restart | Coverage |
|---|---|---|
| K1 | Nothing durable: the lease and its call permit are memory only (`lease/call.rs:540-549`, a non-step permit). No request is sent or stored. | **Gap (low risk).** No restart test. Nothing is written before the signature (`requests.rs:624-692`). |
| K2 | No engine record precedes the library's call. The library signs and broadcasts in one call (`RequestNet::send`). After the restart `dispatch_status` reads `None`. Platform holds the request or does not; the bring-up re-reads sent requests, so the list matches Platform within its budget. | **Gap (needs Platform).** In-process read-back only: `requests_tests::a_failed_send_that_landed_is_pending`. Not equal offline until the bring-up runs. Risk R1 below. |
| K3 | n/a: there is no record before the broadcast. | n/a |
| K4 | The library stored the sent request (`add_sent_contact_request`, `FlushMode::Immediate`). After the kill it reads back as a pending outgoing request, offline. | **New (real kill).** `platform::h03_kill9_dashpay_tests::dashpay_records_survive_a_kill` (Grace). |
| K5 | A replayed restore or changeset gives the same read model and journals each event once. | **Covered.** `contacts_tests::a_replayed_restore_reads_the_same`; `journal_tests::replayed_changesets_journal_each_event_once`. |

### 3. Contact request: accept

| | Expected after restart | Coverage |
|---|---|---|
| K1 | As send K1. | **Gap (low risk).** As send K1. |
| K2 | As send K2. After the bring-up the contact is established if our reverse request landed, and still incoming if not. | **Gap (needs Platform).** In-process read-back only: `requests_tests::a_failed_accept_that_landed_is_established`. |
| K3 | n/a. | n/a |
| K4 | Our reverse request is stored and the contact reads established after the kill, offline. | **New (real kill).** `dashpay_records_survive_a_kill` (Erin established, Frank still incoming). |
| K5 | As send K5. | **Covered.** As send K5; `journal_tests::row_contacts_established_journals_who_accepted`. |

### 4. DashPay payment: send (pay to contact)

| | Expected after restart | Coverage |
|---|---|---|
| K1 | A `pending` payout row with no source record was never signed. The restart settles it not sent, unlocks the contact and returns its address to the pair. | **Covered (in-process).** `send::contact_pay_tests::a_restart_clears_a_payout_with_no_source_record_and_returns_its_address`. |
| K2 | Signed and reserved in memory, the contact row not yet written. Nothing is left: coins free, contact unlocked. | **Covered (in-process + real kill on the shared path).** `send::contact_tests::a_failed_record_refuses_the_hand_off`; coin side: `mempool_sync_tests::h03_kill9_core_tests::h03_core_send_killed_after_signing_leaves_nothing`. |
| K3 | The contact row (written before the hand-off, D23) holds the contact locked until evidence settles it. Nothing left the wallet. | **Covered (in-process).** `contact_tests::a_restart_after_the_hand_off_finds_the_contact_locked`, `…::a_row_on_disk_holds_the_contact_at_the_hand_off`, `…::a_held_record_holds_the_hand_off`, `…::a_conflict_proves_a_lock_unsent_after_a_restart`; `contact_pay_tests::a_restart_keeps_a_recorded_payout_as_the_lock_for_every_source`, `…::a_restart_keeps_a_payout_whose_source_record_cannot_be_read`. |
| K4 | The contact stays locked until the fence's evidence settles it (`a_load_settles_a_lock_on_the_evidence`). Its coins must stay spent. | **Covered for the contact lock; Defect D1 for its coins.** Contact: as K3. Coins: the payment goes through the Core send's hand-off and sent mark, so D1 applies when the wallet had not stored the transaction. Not tested separately. |
| K5 | A replay refuses spends and PSBT signing until it reaches its target. | **Covered.** `send::flow_tests::a_replay_refuses_spends_until_it_reaches_its_target`, `…::a_restart_mid_rescan_resumes_the_replay_and_holds_spends`. |

### 5. DashPay payment: receive

The wallet signs nothing, so K1–K3 are n/a. The kill points are a payment seen in the mempool and stored, then its
block or attribution not stored.

| | Expected after restart | Coverage |
|---|---|---|
| K1–K3 | n/a | n/a |
| K4 | Seen and stored unconfirmed: after the restart it shows once, still unconfirmed; the relay seeing it again adds nothing; its block confirms the same row. Attribution to the contact reads the same. | **Covered (in-process).** `mempool_sync_tests::a_restart_mid_sync_keeps_the_payment_once`; `payments_tests::a_replayed_restore_attributes_the_same`; `journal_tests::row_payments_journal_received_payments`. |
| K5 | A replay counts a received coin once, including CoinJoin coins past index 99 and coins past a spent bridge. | **Covered (in-process).** `restore_tests::a_reopened_session_restores_a_coinjoin_coin_past_99_and_a_replay_counts_it_once`, `…::a_coin_no_gap_reaches_stays_counted_once_through_a_replay`, `…::a_quit_mid_import_replays_from_the_birth_at_the_next_open`, `…::a_quit_mid_load_replays_from_the_checkpoint_the_wallet_was_closed_at`. |

Not covered: a received payment to a contact's DIP-15 address across a real restart before the contact's account is
rebuilt. That needs the contact-crypto drain, which needs the vault unlocked and Platform.

### 6. Profile update

| | Expected after restart | Coverage |
|---|---|---|
| K1 | Nothing durable (non-step permit). | **Gap (low risk).** As send K1. |
| K2 | The put went out, or not. The cache is not updated before Platform answers. After the restart the same edit is a no-op if it landed (D7) or a conflict if Platform moved (D23), never a second document. | **Covered (in-process), without a restart.** `profile_tests::an_edit_that_changes_nothing_sends_nothing`, `…::an_edit_of_a_changed_profile_is_a_conflict`, `…::a_failed_write_is_read_back`. |
| K3 | n/a. | n/a |
| K4 | The library stored the new profile (`set_dashpay_profile`), and it reads back the same after the kill, offline. | **New (real kill).** `dashpay_records_survive_a_kill` (profile). |
| K5 | n/a: no replay rewrites it; the bring-up reads Platform. | n/a |

### 7. Plain Core send

| | Expected after restart | Coverage |
|---|---|---|
| K1 | Nothing durable: every coin free, nothing sent. | **Covered (via K2).** A kill after the signature leaves nothing (below), so a kill before it leaves nothing too; `send::flow_tests::prepare_signs_reserves_and_abandon_releases` (in-process). |
| K2 | The reservation was memory only. Every coin is free and offered again, the history and balance are as before, and there is no sent mark. | **New (real kill).** `mempool_sync_tests::h03_kill9_core_tests::h03_core_send_killed_after_signing_leaves_nothing`. |
| K3 | The sent mark (#88) was written, the hand-off not entered. Nothing left the wallet. The load drops the mark of a payment the wallet does not hold, and the state is as before the payment. | **New (real kill).** `…::h03_core_send_killed_before_the_hand_off_leaves_nothing`. |
| K4 | Handed to the network. Its coins must stay spent (not offered to another payment) and the payment must show. | **New (real kill)** when the wallet stored the transaction first: `…::h03_core_send_killed_after_the_wallet_stored_it_keeps_its_coins_spent`. **Defect D1** when it had not: `…::h03_core_send_killed_after_the_hand_off_keeps_its_coins_spent` (ignored). |
| K5 | A replay refuses spends until its target. Coins spent unmined are not credited again by a rescan after a restart. A cancelled replay stays held across a restart. | **Covered (in-process).** `flow_tests::a_restart_mid_rescan_resumes_the_replay_and_holds_spends`, `…::a_cancelled_replay_stays_held_across_a_restart`, `…::a_quit_between_the_record_and_the_rewind_costs_only_a_replay`; `own_spend_balance_tests::a_rescan_after_a_restart_does_not_credit_again_a_coin_spent_unmined`, `…::a_coin_spent_unmined_stays_spent_two_restarts_later`. |

## Counts

There are 35 cells (7 flows × 5 kill points).

| Status | Cells | Which |
|---|---|---|
| Covered by existing tests | 16 | Registration K1–K5; send K5; accept K5; contact payment K1, K2, K3, K5; receive K4, K5; profile K2; Core send K1, K5 |
| New in this change | 5 | Send K4; accept K4; profile K4 (one real-kill test covers all three); Core send K2, K3 |
| Defect | 2 | Core send K4 and contact payment K4 (both D1) |
| Gap | 5 | Send K1, K2; accept K1, K2; profile K1 |
| n/a | 7 | Send K3; accept K3; receive K1–K3; profile K3, K5 |

**New tests: 5.** `h03_kill9_core_tests` adds four Core send tests, one of them `#[ignore]`d for D1, plus their child
process `core_send_child`. `h03_kill9_dashpay_tests` adds `dashpay_records_survive_a_kill` and its child,
`dashpay_record_child`. Both children are `#[ignore]`d: each runs only when its parent starts it.

Every new test is a real kill: the child is stopped with SIGKILL, the parent checks the exit signal, and nothing of
the child's runs after the kill point.

Five gaps remain, all of the same kind: a kill before a Platform write's signature, or between its signature and
the library's record of it. The first leaves nothing durable. The second needs Platform's answer, which only the
bring-up's re-read gives.

## Defects

### D1: a Core payment handed off before the wallet stored it is forgotten after a kill -9

- **Flow:** plain Core send. The same hand-off and sent mark serve a DashPay contact payment, so it is affected too.
- **Kill point:** K4. The payment was handed to the network (`TxDraft::dispatch`, `send/mod.rs:1713`), but the wallet had not yet
  stored the transaction. That happens only when dash-spv's own relay delivers it to the wallet manager and the
  persister writes it.
- **Test:** `mempool_sync_tests::h03_kill9_core_tests::h03_core_send_killed_after_the_hand_off_keeps_its_coins_spent`,
  `#[ignore = "H-03: a payment handed off but not yet stored is forgotten after a kill -9"]`. Run it with
  `cargo test -p dw-engine --lib -- h03_core_send_killed_after_the_hand_off --include-ignored`.
- **Expected:** after the restart, the coins the payment signed are not offered to another payment and the payment
  shows (as unconfirmed, or with an unknown outcome), as when the wallet had stored it. Its twin,
  `h03_core_send_killed_after_the_wallet_stored_it_keeps_its_coins_spent`, passes.
- **Observed:** all three input coins are free again within 60 s. The history holds only the funding transaction,
  the balance is whole, and the `sent:` mark is gone. Two runs gave the same result.
- **Suspected cause:** the `sent:` mark (#88), written before the hand-off (`send/mod.rs:1541-1546`, `mark_sent`
  at `send/mod.rs:2223-2245`), stores only the coins, not the signed transaction. At load,
  `restore_unmined_spends` (`history_ops.rs:419-445`) restores claims only for payments the history holds
  (`history.rs:368-395`, `unmined_own_spends`). `settled_sends` (`history.rs:399-410`, `txs.get(t).is_none_or(…)`)
  then treats a payment missing from the history as settled and drops its mark. "Gone from the store"
  (`tx_actions.rs:88-93`) cannot tell "never left" from "left, but its record never landed". The mark is written
  before the hand-off precisely because the payment may leave, yet it is read as if a missing record proves it did not.
- **Funds:** no coins are lost; the payment is valid on the network. But until a peer relays it again or a block
  includes it, the wallet overstates the balance by the payment, so the 60 s bound does not hold. It also offers
  the spent coins to the next payment, which peers reject as a double spend. A user who thinks the first payment
  failed may pay again from other coins: **a double payment**. For a contact payment the contact stays locked (D23),
  but its coins are freed the same way.
- **Not changed:** engine and funds code are left for the PO to route.

## Risks (not shown by a test)

- **R1: a re-send between the restart and the bring-up's re-read.** `Own::eligibility` (`requests.rs:122-153`)
  asks only the wallet's own state, then the contact's key set on Platform. It never asks whether our request is
  already on Platform. After a K2 kill whose request landed, a send before the bring-up has re-read the sent
  requests signs a second transition. Platform should refuse it (the DashPay contract's unique index on owner,
  recipient and account reference); `read_back` then finds the first request and records it. So state converges,
  but the refused transition may cost a processing fee in credits. Not shown: the offline fixture has no bring-up and
  its fake Platform enforces no unique index.
- **R2: the 60 s bound depends on Platform and peers.** DashPay cells K2 come back within the bring-up's budget
  (20 s; 3 s for a created wallet) only when DAPI answers. D1 comes back only when a peer relays the transaction again
  or a block includes it.

## Panics vs crashes in registration (#212)

#212 asks what a registration job leaves behind when it **panics** part way. Release builds unwind
(`rust/Cargo.toml:235`, `panic = "unwind"`). The `dw-registrations` actor catches a job's panic
(`registration_flow.rs:723-731`). A panic in the run's own task (`registration_flow.rs:3103`) is caught by tokio.
For storage a panic is a crash: SQLite rolls back the write in flight, and every kill point above covers it. These
notes list what lives in memory, which a crash clears and a caught panic, in a process that goes on, may not. No
code was changed for them.

What a caught panic releases as a crash does (dropped on unwind):

- the flow's claim: `Claim::drop` clears the holder, `in_call`, the task, the lease and the offer
  (`registration_flow.rs:1041-1054`);
- the call permit: `CallPermit::drop` ends the call (`lease/call.rs:1042-1048`);
- the coin fence and its temporary UTXO locks (`coins.rs:432-451`; if the wallet manager is busy, the release is
  spawned, so it lands a little later);
- the identity write lock (a tokio mutex) and the library's async state locks.

By cell:

| | What a caught panic leaves that a crash would not |
|---|---|
| K1 | **The step's resolution differs.** A crash leaves the marker with its durable-record flag, and the row closes `funding_cancelled`. A panic runs `CallPermit::drop` → `end(false)` in the live process: the unsigned step resolves `NotSent`, its marker goes, and the next run funds the same step again. Both are safe (one lock at most); they are different outcomes. Covered by `a_flow_aborted_before_the_funding_signature_retries_its_step` (the in-process abort is the panic's path). **The lease stays live:** the facade's lease from `begin_flow` is owned by the session (`flow_leases`, `flows.rs:270-286`; its `LeaseOwner` ends it on drop, `lease/table.rs:844-853`), not by the run. A panic drops only the run's clone, so the lease, its remaining budgets and, when it holds one, its own vault key stay live until `end_flow` or the idle reaper. A crash ends it. A failed run leaves it the same way, so this is not specific to a panic. |
| K2 | Same as a crash for the step: the permit's end, after a released signature, reads `MaybeSent`. If the panic hits the tap's actor job (`note_lock_tracked`), `act_blocking` returns `None` and the tap reads it as "no row took the lock" (`registration_flow.rs:1285-1288`, `.unwrap_or(Ok(None))`). The lock is then left to the funding call's end and "Finish transfers", the path `skip_tap` covers, just as a crash between the library's store and the tap's write. The lease, as K1. |
| K3 | `FlowState.is_until` (the InstantSend window's end, an in-memory `Instant`) and `FlowState.backed_up` survive the panic. `Claim::drop` does not clear them (`registration_flow.rs:580-611`). The next run in the same process reads the old window's end, where a crash would start without one, and skips the automatic backup of `FundingSent` that a restarted process would schedule again. Both are benign. The lease, as K1. |
| K4 | As K3. Also: a panic **while a `std::sync::Mutex` is held** leaves whatever that critical section had half-written. The registration map (`Flows::with`, `registration_flow.rs:676-679`) and the lease table's J (`lease/table.rs:415-417`) both ignore poisoning (`PoisonError::into_inner`), so later calls go on from that state; a crash would clear it. I found no panicking call inside those sections, so this is a hazard class, not a found defect. |
| K5 | Nothing more than K4. A replay's hold is stored, not in memory. |

Separately: after a panicking job, `Flows::act` reports `platform.internal` with "the registration actor stopped"
(`registration_flow.rs:748-751`), although the actor keeps running.

## Gates

Run on strixbox under `nice -n 10`, with load5 between 87 and 120.

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean.
- The H-03 tests, all included (`cargo test -p dw-engine --lib -- h03_ --include-ignored`), twice: 6 passed each
  time. D1's test failed both times, as written up.
- `cargo test -p dw-engine --locked --lib` (`LimitNOFILE=65536`): 2433 passed, 8 failed, 19 ignored. All eight
  failed waiting on something under load ("never: SPV runs", "timed out waiting for the names pass", "the dispatch
  journal is unavailable" while the journal opened, a 13 s close). Run again with `--exact`, all eight pass;
  `own_spend_disk_tests::a_payment_locked_apart_from_its_record_is_not_abandoned_after_a_restart` needed a second
  try, then passed twice more. None of the eight is in a file this change edits.
