# DP1-09: DashPay username registration wizard, progress and resume (Tauri)

ROADMAP DP1-09 on the Tauri renderer: [DASHPAY](DASHPAY.md) F2 ("wizard → progress that can be closed; persisted
state machine; 'Funds locked — finishing' rows; Tools ▸ Repair 'Finish transfers'"), F4 ("inline rule checklist,
live availability, contested chip and explainer"), and §4's `RegistrationViewModel` ("name → contested confirmation →
funding → review → progress → done; resume"; IA: "The registration wizard can be closed while it runs; Home and the
chip then show its progress"). Acceptance: register a username from the UI; close the app mid-flow and resume.

The engine is merged: DP1-02's state machine and calls (m4 §2.2), DP1-03's names (§2.3), DP1-06's credits, DP1-04's
contest watch, BL-153's `status`. dw-app already exposes every call (E0-13). This task adds **no engine change and no
dw-app command**: it binds the registration and name calls into the renderer's `Backend`, builds the wizard on the
Join DashPay page (replacing DP1-08's placeholder, D7 there), and gives Tools ▸ Repair its "Finish transfers". Copy
follows the mobile wallets (Android `strings-dashpay.xml` at v11.9.1, iOS as DP1-08 cites it), adapted (D-rows).

| Part | Where |
|---|---|
| View model (pure TypeScript): the name field, the confirm step, the progress and result screens, their copy | `apps/desktop/src/dashpay/registration.ts` |
| The wizard's flow state (step, label, quote, draft; one per wallet, kept across navigation) | `apps/desktop/src/dashpay/wizard.ts` |
| Their tests, by parity ID (IOS-066, IOS-067, IOS-134, IOS-139; AND-024) | `src/dashpay/registration.test.ts`, `src/dashpay/wizard.test.ts` |
| Queries (`check_username`, `name_availability` debounced 400 ms, `registrations`, quote per label) | `src/dashpay/queries.ts` (additive) |
| The wizard's screens | `src/screens/dashpay/Registration.tsx`; `JoinDashPay.tsx` mounts it |
| Repair's Finish transfers | `src/tools/repair.ts`, `src/screens/tools/Repair.tsx` (additive) |
| Shared files (additive): `Backend` and its Tauri binding, the events table (D14), the fixture's handlers, a registration run and scenarios | `api/{backend,tauri,events}.ts`, `fixture/{handlers,dashpay,scenarios,index}.ts`, `fixture/registration.ts` |
| Playwright (flows, resume across a relaunch, axe; light and dark; Chromium and WebKit) | `e2e/registration.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on testnet (GL): wallet E created in the app, funded from A, registers, closed mid-flow, relaunched | `e2e-tauri/registration.ts`, `scripts/e2e-tauri-registration.sh` |

## The engine's states (inventory)

`RegistrationStatus` (m4 §2.2, §3) gives each row's `phase`, `waiting`, `failure`, `fundsCommitted`, `holdsKey`,
`running` (D26), `nameFailure` and `contestEndsAt`. The engine drives a row only while a run holds it: a run starts with
`start_registration` or `resume_registration`, keeps its lease through the network, proof and sync waits, and parks
(ends) only at a step that needs authority it lacks. **Nothing resumes a row at session open or unlock** (P2b's
catch-up is not built; m4 §2.2 "On E0-04.P2b"). A row no run drives reports a wait derived from the row and the world:
`Unlock` while the vault lacks its full key, else `Authorize` ("Confirm to finish"; `registration_flow.rs`
`authority_wait`), `Sync`/`Startup` while the funding hold lasts, `ChainLock` for a lock without its proof,
`FundingUnknown`/`NeedsFinish` for the repair cases. The wizard's screens map them:

| # | Row (input) | Screen | Primary action | Secondary |
|---|---|---|---|---|
| R1 | `running` (D26), any wait but `authorize`, `unlock` and the repair ones; or idle and held for `sync`/`startup` (once the hold lifts it reads `authorize`/`unlock`: R2/R3) | Progress: step 1–3 (DP1-08 D4) + its wait line | "Let me know when it's done" (Home) | — |
| R2 | `waiting: authorize` (a revoked lease, a passphrase change), or `!running` and `chainLock`/`network` (after a relaunch: `chainLock` until a run records the proof; D21, D26) | Progress + "Paused. Confirm to finish creating @x." | Continue (grant, then `resume_registration`) | — |
| R3 | `waiting: unlock` | Progress + "Unlock the wallet to finish creating @x." | Unlock (mixing only: the unlock sheet; locked: the lock screen covers it) | — |
| R4 | `waiting: fundingUnknown` | Progress + "The payment may have been sent" + the recovery copy (D9) | Open Repair | — |
| R5 | `waiting: needsFinish` | Progress + "Funds locked — finishing" + the recovery copy | Finish transfers (D10) | Open Repair |
| R6 | `failure`, retryable | Paused + the failure's reason (DP1-08 D5 `failureText`) + the recovery copy when `fundsCommitted` | Continue (grant, `resume_registration`) | Choose another username (only while `!fundsCommitted`, D7) |
| R7 | `failure`, not retryable, `!fundsCommitted` (before funding, `funding_cancelled`, `funded_elsewhere`) | "Registration failed" + the reason | Choose another username (discards the row, D7) | — |
| R8 | `failure`, not retryable, `fundsCommitted` (`broadcast_unknown` and kin) | "Registration stopped" + the recovery copy | Open Repair | Choose another username (D27) |
| R9 | `done`, `nameFailure` null, not contested | Done: "Hello @x, your account is ready" (Android `processing_done_*`) | Done (Home) | — |
| R10 | `done` with `nameFailure` (the identity exists, the name step refused for good) | "Your identity is ready, but @x could not be registered" + the code's reason | Choose another username (credits path, D6) | — |
| R11 | `contested` / status `contestPending` | "Voting for your username has begun" + "Results on <date>" or "We will notify you…" | Done | What is username voting? |
| R12 | the row is gone (discarded elsewhere) or unreadable | the Join page as the status says | — | — |

## Screens

All on the Join DashPay page (route `joinDashPay`, `page-joinDashPay`). The two choosing steps carry a header line
"Step n of 2 · title" (`registration-step`, after UX-SPEC C29's wizard header, without its segmented bar for two
steps); the progress and result screens have none. Width `formMaxWidth` (640).

1. **Choose** (`registration-name`): title "Choose your Dash username" (Android `choose_your_username`); the field
   (`registration-name-field`, `autocapitalize=off`, `spellcheck=false`, max 23 characters typed); the rule checklist
   from `check_username` (each rule a row with a pass / fail mark: "Between 3 and 23 characters", "Letters, numbers
   and hyphens only", "Must start and end with a letter or number", "No two hyphens in a row"); under it the
   availability line from `name_availability` (400 ms debounce, latest wins: "Checking…", "Username available",
   "Username taken", "This username has already been requested, but you can request it too…", "This username is
   blocked", "Could not check availability. You can still continue."). A valid contested label adds the note
   (Android `dash_network_must_vote_to_approve`): "The Dash network must vote to approve this username before it is
   created. Usernames with a number 2–9, or 20 characters or more, are approved at once." and the "What is username
   voting?" link (DP1-08's sheet). Continue follows D3.
2. **Confirm** (`registration-confirm`): "You have chosen @x as your username. Please note that the username can NOT be
   changed once it is registered." (Android `new_account_confirm_message`); the cost from `registration_quote` for
   that label: "Upgrade fee" with the lock + fee in Dash (D2), and the line "Part of it becomes credits that pay for
   contacts and profile changes."; for a contested label, "The network votes on this username: about two weeks. If
   it is not approved, the payment is not returned; your identity keeps its credits." (D4); a shortfall line when the
   quote's total exceeds the wallet's balance (D5); the check "I accept" (Android `new_account_confirm_accept`);
   Back and "Confirm & pay". Confirm asks for the grant the quote names (`platformOp{maxDuffs, maxCredits}`) through
   U-05's `GrantModel` (the passphrase sheet when needed), then `start_registration`.
3. **Progress** (`registration-progress`, `data-phase`, `data-waiting`): "Creating your username" + "Your username @x is
   being created on the Dash network. It may take some time." (Android `creating_your_username_message`), the three
   steps with the current one marked (`(1/3) Processing payment`, `(2/3) Creating identity`, `(3/3) Registering
   username`), the wait line, and "You can close this page or the app: the registration goes on, and Home shows its
   progress." with "Let me know when it's done" (Home). R2–R8 add their lines and actions.
4. **Result**: R9 (`registration-done`), R10 (`registration-name-failed`), R11 (`registration-voting`), R7/R8
   (`registration-failed`).

The **credits path** (D6): a `ready` identity without a username (S17 "Choose a username") opens step 1 then a
confirm that quotes `grant_request(registerName{label})` ("Paid from your identity's credits: N credits") and calls
`register_name`; its result is R9 or R11.

## Resume rules

- **The engine's rows are the truth.** The Join page opens on the progress screen whenever the status is
  `registering{draft}`, for that draft's row, whatever the wizard's own state says; on a relaunch the wizard's state is
  empty and the row alone decides (R1–R8). Home's card and the chip (DP1-08) read the same row through the same
  `homeCard`/`chipState`; the wizard reuses `stepOf`, `failureText` and the step copy (D11).
- **No second asset lock.** The wizard never calls `start_registration` while the status is `registering`, while a
  registrations read lists an open row, or twice for one Confirm (the button is held while the call runs, and the
  model refuses a second start; D8). A row that waits is moved on with `resume_registration(draft, grant)` only; the
  engine's ledger refuses a second funding anyway (`registration.in_progress`), which the wizard maps to the progress
  screen of the draft it names.
- **After a relaunch** no run drives the row. The engine reports what it lacks first (`idle_wait`): `chainLock` while
  its lock has no recorded proof, then `Authorize` (or `Unlock`). Either way, with no run driving it (`running` false), the row waits
  for Continue (D21, D26), with the passphrase when "Require authentication for every payment" is on or the vault is not
  fully unlocked (D12). The engine then runs the keyless waits and the signing steps under one grant.
- **Before Confirm** nothing is spent or written: closing the app on step 1 or 2 drops the typed name, and a relaunch
  starts at the Join page (D13).
- **Done** shows once, in the session that watched the row end: after a relaunch the Join page says "You have joined
  DashPay as @x." (DP1-08 J5) or shows the voting card.

## Bindings

| Call | Used for |
|---|---|
| `check_username(label)` (sync, offline) | the checklist; no wallet needed |
| `name_availability(wallet, label)` | the availability line, debounced 400 ms, latest wins; `["dashpay","availability",w,label]` |
| `registration_quote(wallet, request)` | the confirm step's cost and grant, per label (`["dashpay","quote",w,label]`); DP1-08's fixed-label quote stays for the Join page |
| `start_registration(wallet, request, grant)` | Confirm & pay |
| `start_registration_past_unknown_funding(wallet, request, grant, past)` | after the wedge copy (m4 §2.2 "In progress"), D9 |
| `registrations(wallet)` | the progress screen's row (DP1-08's query key) |
| `resume_registration(wallet, draft, grant)` | Continue (R2, R6) |
| `discard_registration(wallet, draft)` | Choose another username on an unfunded failed row (R6, R7) |
| `finish_asset_locks(wallet, grant)` | R5 and Tools ▸ Repair "Finish transfers" (D10) |
| `grant_request(wallet, identity, registerName{label})`, `register_name(wallet, identity, label, grant)` | the credits path (D6) |
| `authorize` (through `GrantModel`) | every grant |

Re-reads: every `Platform` signal re-reads `["dashpay", …]` (DP1-08); a `leaseChanged` of a `registration` lease also
re-reads the registrations (D14).

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | The wizard lives on the Join DashPay page, not in a sheet. | DASHPAY IA: it "can be closed while it runs"; the chip and the card already lead to the Join page (DP1-08 D12), so the progress is one route for Home, the chip and the wizard. A sheet would close with navigation. |
| D2 | The cost is shown in Dash only. | The renderer has no exchange-rate source (no fiat anywhere in the app yet); the brief's "fiat if available". It comes with the price feed when one lands. |
| D3 | Continue is allowed when availability says `available`, `contestOpen` (another identity requested it: "This username has already been requested, but you can request it too and let the network vote to decide if you can have it", Android) or `unknown` (Platform did not answer: "Could not check availability…"); never on `invalid`, `taken` or `locked`. The confirm step's quote checks the label again. | Android lets the user go on after a network error ("you can try again at no extra cost") and offers to join an open contest; a taken or blocked name cannot be had (D16). |
| D4 | A contested label is allowed (the engine prices it at 0.25 Dash), with the voting explanation on steps 1 and 2. The instant (temporary) username is not offered in the wizard. | F5's temporary name is `register_name` on the identity while its contest is open; DP1-10 (username request status) owns that screen, as Android offers "Create instant username" from its voting screen. |
| D5 | `RegistrationQuote` gains `available_duffs` (m4 item 52, Contract-Version 26): the engine already computed it for the start's check (`plan.available_duffs`, `spendable_of`) and dropped it. The confirm step and the Join page say "You need at least N Dash" only when it is below the quote's total; `registration.funding_insufficient` at the start shows the start's own `needed`/`available`. This closes DP1-08 D18's follow-up: J1's shortfall no longer reads `Balances.total`. | The brief: expose it if cheap. It is one field through the engine, dw-app and the generated type; the m4 §3 listing is re-blessed. dw-ffi binds no DashPay record, so no Swift surface changes (the Swift T0 suite is run anyway, as the brief asks for a contract change). |
| D6 | "Choose a username" (S17) runs the credits path: step 1, a confirm quoting `grant_request(registerName{label})`, `register_name`. | An identity without a name pays from its credits (m4 §2.3); no asset lock. |
| D7 | Choose another username discards the row first (`discard_registration`) and is offered only while `!fundsCommitted`. A funded row is never discarded from the wizard. | m4 §2.2: discard refuses a committed row; the funds must not be stranded. |
| D8 | One start per Confirm: the model holds `starting` and refuses a second call, and the button is disabled while the grant is asked for and the call runs. | No second asset lock from a double click. |
| D9 | The recovery copy (R4–R8): "Your funds are safe. The payment is locked for this registration and Dash Wallet finishes it once the network confirms it. If it does not finish, Tools ▸ Repair can finish the transfer." for a committed row; `registration.in_progress` past a wedged funding shows m4 §2.2's wedge copy verbatim and calls `start_registration_past_unknown_funding` only after the user confirms it. | m4 §2.2 names the copy; "a failed asset lock must never lose funds: show what the engine says about recovery". |
| D10 | Tools ▸ Repair gains "Finish transfers" (`finish_asset_locks`) under a `platformOp{maxDuffs: 1, maxCredits}` grant sized as DP1-02's quote bound, with its report ("N resumed, N completed, N still pending"); the "Asset locks" not-yet row keeps only "sending or cancelling unrecorded asset locks". | m4 §2.2: Finish transfers needs `max_duffs ≥ 1` for top-ups and spends none; R5 and DP1-08 S14 point to it. |
| D11 | The wizard reuses DP1-08's `stepOf`, `failureText`, `COPY.creating` and the card's wait lines (exported); it does not fork them. | One state machine for Home, the chip and the wizard. |
| D12 | A relaunch does not resume a row by itself: the progress screen offers Continue. | Resuming signs (the identity step with the lock's key, the name with the identity's); with "Require authentication for every payment" on (the default) that needs the passphrase, which an unasked sheet must not pop up for. |
| D13 | Steps 1–2 are not persisted. | Nothing is spent or written before Confirm; the mobile wallets restart the choice too. |
| D14 | A `leaseChanged` event for a `registration` lease re-reads the registrations (events.ts, additive). This fixes DP1-08's follow-up: a passphrase change revokes the flow's lease (`passphraseChange`) and parks the row without a `vaultLockState`. | The lease event is what the engine sends; no engine change. |
| D15 | While the progress screen shows a running row it re-reads `registrations` every 5 s, on top of the signals. | A run's `waiting` changes (InstantSend to ChainLock, a park) write no row, so no `Platform{Registration}` says so (engine research, `registration_flow.rs` `Step::Wait`/`Park`). The read is local (app.sqlite and memory). An engine signal on those changes is a follow-up (F6). |
| D16 | The availability check runs before Confirm, and the wizard does not offer Confirm on a `taken` or `locked` label; `start_registration` itself does not check (a taken name ends `done` with `nameFailure: name.taken`, R10). | The identity and its credits are then kept and the name is chosen again from credits (D6), so nothing is lost; but the user should not pay for an identity whose name they cannot have when the check could have said so. |
| D17 | The GL run is on public testnet, not T2. `scripts/e2e-tauri-registration.sh` creates wallet E in the app, funds it from fixture wallet A's Core coins with dwcli, registers a non-contested name (a digit 2–9), closes the app once the row has its lock's txid in `fundingSent`/`proofWaiting`, and resumes after the relaunch. A ledger (`DP1-09.json`, written exclusively, then per step) and a kept data root let a stopped run go on without a second wallet, payment or lock. | agentbox runs no dashmate devnet (no containers, nothing listening), as for DP2-07/08/09. Testnet writes are allowed for this acceptance only; E's phrase is kept 0600 in the fixture before any coin moves (BL-163). |
| D18 | The fixture models a run as `runs` (drafts with a run this session) and the registration lease: a row is advanced only while its run is there and its lease is live, or (a scenario's run) no registration lease exists; `relaunch()` keeps the world through `sessionStorage`, drops runs, grants and leases, and locks an encrypted vault. No proof, credit or coin is modelled: `availableDuffs` is the confirmed balance less CoinJoin coins, and a run debits nothing. Reads answer copies of the world's rows. | The flows to prove are the UI's (step, park, Continue, no second start); the engine's own accounting has its tests. A read answering the world's object, changed in place by `advance`, looked unchanged to the query cache (found in the e2e run). |
| D19 | A relaunch on step 1 or 2 lands on the Join card (D13); a relaunch after Confirm lands on Home with the card paused (`creatingPaused`), whose action opens the row's screen. | Home is the app's start; the card already leads to the Join page (DP1-08 D12). |
| D20 | The two steps' header is the line "Step n of 2 · title", without C29's segmented bar. | A bar of two segments says no more than the line; the progress screen keeps DP1-08's three-step bar (D11). |
| D21 | A row waits for Continue (`awaitsContinue`, status.ts) when it says `authorize`, or a keyless wait while no run drives it (first read as `!holdsKey`; corrected by D26). Home's card says `creatingPaused` and the page R2 for it; only other rows are polled (D15). The fixture reports an idle proofless row as `chainLock`, a parked run as `authorize`. | Review r1 H1: the engine's `idle_wait` reports an idle `fundingSent`/`proofWaiting` row with no recorded proof as `ChainLock` whether or not a run waits, and nothing moves the lock's status outside a run, so the old R1 mapping left a relaunched row on "Let me know when it's done" for ever. A run this app starts or resumes always holds a lease, so `!holdsKey` marks an idle row; taking a running row for idle costs one passphrase (resume hands a running flow the grant), the reverse strands the screen. An engine field saying whether a run drives the row would replace the inference (F7). |
| D22 | When the status is `noIdentity`, the Join page shows the newest row that failed for good with its funds committed (R8) instead of the intro. The fixture's status and start refusal follow the engine: `registering` names a row short of done and not failed for good (banner.rs `in_progress`); a start is refused by such a row or by a failed row with funds committed. | Review r1 M1: such a row is no longer `registering` but still blocks a start, so the funds-are-safe copy and Repair were only reachable after a refused Confirm. |
| D23 | While the draft this session started has no row in the cache yet, the page shows a busy progress screen, never the Join intro with Create. | Review r1 M2: between the start's answer and the registrations read, Create would have been offered again (the engine's guard still refuses a second lock). |
| D24 | The wedge keeps every blocker the engine named and starts past all of them; a refusal of that start shows inside the sheet. Step 2 takes the trimmed label. On the credits path step 2 shows `grant_request`'s credits bound and, for a contested label, the "I accept" check. "Choose another username" on a done row needs its identity. | Review r1 M3, M4, L1, L4. |
| D25 | Load flakes recorded (r2: the webkit axe tests of the Join page and the Tools sheets timed out at 30 s at load 219 and pass alone, 52/52; one `dashpay-join-waiting dark` screenshot timed out and passes alone): once each, the Home card `creatingPaused light` (chromium) and `error dark` (webkit) card-state tests timed out on their first read under load (box load 50–190); both pass alone and in the next full runs. In one full vitest run (after the r1 fixes) `files.test.ts` (not touched here) and the every-scenario JSON test failed on time; both pass alone, and the latter has its own 30 s timeout now. | The common brief: rerun alone before blaming the code. |
| D26 | `RegistrationStatus` gains `running` (m4 item 53, Contract-Version 27): a driver runs the flow. `awaitsContinue` is `authorize`, or `!running` with `chainLock` or `network`; `instantSend` (never an idle wait), `sync` and `startup` are never paused (an idle row held there shows progress; when the hold lifts, `idle_wait` gives `authorize`/`unlock` and D15's poll sees the pause). The fixture reports `running` from its runs and `holdsKey` only for a grant made while the vault had no full-scope key. The GL fails when the screen shows anything but progress for the running row before the close, or stays paused a minute after Continue. | Review r2 H1: D21 read `!holdsKey` as idle, but `holds_key` means the lease holds a key of its *own*, which a run on an unlocked vault never has and a run past the InstantSend window has dropped: every Core registration on an unlocked wallet showed paused while it ran, D15's poll stopped, and Continue looped (resume hands a running flow the grant and the row still read paused). F7 done instead of a lease-table inference. |
| D27 | The stopped screen (R8) also offers "Choose another username": step 1, nothing discarded. A start the engine refuses shows the stopped row again (`registration.in_progress`), with a line saying its payment must be finished first (Repair). Step 2 says the cost could not be worked out when the credits bound fails, as for the quote. A draft of this session's that a fresh read no longer lists (discarded elsewhere) returns the page to the Join intro; a failed read shows the busy screen with a line, not busy. | Review r2 M1 (`funds_committed` is sticky, so a Settled or untracked-lock row reads committed though the engine's start gate takes a new start past it: the page had no way to Create), L1, L2. The fixture keeps `fundsCommitted` as its start-gate proxy, so there it refuses. |

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | The instant (temporary) username beside a contested one | DP1-10 |
| F2 | An initial profile (display name, avatar) entered in the wizard | DP4-03 (Edit Profile) |
| F3 | Invitation-funded registration and "Have an invitation?" | DP5-03 |
| F4 | Fiat beside the Dash cost | the price feed task |
| F5 | (done here: D5) | — |
| F6 | A `Platform{Registration}` signal when a run's `waiting` changes without a row write, so D15's poll can go | engine follow-up |
| F7 | (done here: D26, m4 item 53) | — |
| F8 | Continue's grant on a funded row could be `maxDuffs: 1` (the lock is paid), and a grant a running flow did not take could be revoked; today it expires | engine/contract follow-up (review r1 L2) |
| F9 | Finish transfers' credits bound is one non-contested registration's; several stranded rows or a contested one may park for another grant | follow-up (review r1 L3) |
| F10 | `start_registration` signals the row before `start_run` claims it, so a read in between sees `running: false` and `authorize` (paused, poll stopped) until the run's first write signals; send the signal after the claim | engine follow-up (review r3 nit; predates D26) |

## Slice plan

1. This document.
2. `Backend` bindings; the view model and the flow model with their tests.
3. The fixture: handlers for the calls, a registration run the tests advance, relaunch, scenarios.
4. The screens on the Join page; the placeholder and its tests go; Repair's Finish transfers.
5. Playwright flows (start, progress, relaunch and resume, failures, credits path) + axe; visual baselines.
6. The GL script on testnet (wallet E) and its run; `docs/parity.md` rows.
