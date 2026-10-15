# BL-153: the `DashPay.status` body

Backlog row BL-153. `DashPay.status` (`rust/crates/dw-engine/src/platform/startup.rs`) was still E0-08's skeleton: it
answered `platform.not_implemented`, and no ROADMAP row owned its body. As a result the real app showed no identity
chip, the sidebar showed Contacts for wallets without an identity, and DP2-07 needed its D19 fallback (F11 there).
This task gives the call its body, signals its changes, wires the shell to it and removes the fallback.

The contract is m4-dashpay-engine.md §2.1 (`status()`: sync, the banner state F1 shared by the Home card, the chip and
the Contacts empty state) and §5 (`NoIdentity{reason}`), DASHPAY §3.6 and §4 (`DashPayStatusModel`), U-01 D1 and D11
(chip and sidebar), E0-13's binding `dashpay_status`, and the generated `DashPayStatus.ts`. No signature changes.

| Part | Where |
|---|---|
| The facade call | `dw-engine/src/platform/startup.rs` (`DashPay::status`) |
| The rules (pure `derive_status`) and the gatherer (`dashpay_status_of`) | `dw-engine/src/platform/banner.rs` |
| Startup changes signalled (`Platform{Startup}`) | `dw-engine/src/platform/runtime.rs` (`StartupWrite`), `events.rs`, `dw-app/src/bridge.rs` |
| One identity read for the list and the main-name rows (`identity_read`) | `dw-engine/src/platform/recovery.rs` |
| The contest watch signals a learned end and a dropped watch (`Platform{Names}`) | `dw-engine/src/platform/contest_watch.rs` |
| Engine tests | `dw-engine/src/platform/banner_tests.rs` (the rules, a real offline session, the signal), `contest_watch_tests.rs` (the `Names` signal) |
| Contract | `docs/contracts/m4-dashpay-engine.md` §2.1, the §6 `EngineEvent::Platform` row, §7 item 49 (version 23); `m1-engine.md` E0-05 item 4 |
| Shell (chip, sidebar, bell) | `apps/desktop/src/api/queries.ts` (`useDashPayShell`), `src/components/shell.tsx`, `src/app/App.tsx` |
| Contacts (D19 removed) | `apps/desktop/src/contacts/{queries,model}.ts` |
| Event bridge | `apps/desktop/src/api/events.ts` |
| Fixture scenario `dashpay-contest-pending`, Playwright | `src/fixture/scenarios.ts`, `e2e/routes.spec.ts`, `e2e/contacts.spec.ts` |
| Real app (GL) | `e2e-tauri/contacts.ts` checks the chip names the fixture wallet's main username |

## What `status` reports, and where each part comes from

`status` is sync and must be cheap and offline-safe, so it reads only memory and app.sqlite: no Platform or DAPI read.
Every input is local:

| Input | Source | Kind |
|---|---|---|
| The wallet exists, the session is open | `require_wallet`, the op gate | memory |
| Watch-only (`read_only`) | the vault's seed index (`has_wallet_secret`), as `dashpay_startup` reads it | memory |
| The bring-up outcome (`StartupStatus`) and the identity it named | `PlatformRuntime.startup` (`dashpay_startup`) | memory |
| The wallet's identities, the main one, its shown names, whether the read was live | the library's `PlatformWalletInfo` (or the recovery snapshot while a pass holds the lock) plus `dp_main_identity` (cached) and the `dp_prefs` main-name rows: `identity_read`, the read `identities()` makes | memory + app.sqlite |
| Its temporary name (a contested registration's second name) | the same read's `dp_prefs` rows (`MainNamePrefs.temporary`) | app.sqlite |
| Registrations in progress | `dp_registration` rows: `phase`, `identity`, `error`, `retryable` (the row-level `failure_of`; no ledger, no library call) | app.sqlite |
| Contests | `dp_contest_watch` rows: `identity`, `label`, `ends_at`, `last_state` | app.sqlite |
| Platform proved no identity (7-day TTL), created here | `settings_kv` local-scope `dashpay.no_identity`, `dashpay.created_here` (the bring-up markers) | app.sqlite |
| Core and masternode sync done | the sync tracker's `caught_up()` (headers, filters, masternodes, blocks) | memory |

No field needs the network. Fields that a live read could refine are listed in F1 below.

### The rules (`derive_status`, in order)

1. **An identity is known** (the identity list's main entry; while the list is a snapshot showing none, the identity
   the bring-up named, else the newest finished registration's, D4):
   1. a registration in progress that creates *that* identity (not funded by an existing identity), before its name
      step ended (`Draft` … `NameRequested`, no `name_error`, not failed or failed retryably) → `Registering{draft}`
      (D5, D15);
   2. on a live read, the main identity shows no name but its temporary one, and it has an open contest (a
      `dp_contest_watch` row not yet won, or a `Contested` registration row) → `ContestPending{identity, label,
      ends_at}`; `ends_at` is `None` while the watch has not read it (D6);
   3. else `Ready{main}`.
2. **No identity is known:**
   1. watch-only → `NoIdentity{WatchOnly}`;
   2. a registration in progress that creates its identity (any phase short of `Done`, not failed or failed
      retryably, not `ExistingIdentity`-funded, no `name_error`; the newest one) → `Registering{draft}` (D15);
   3. on a live read, the absence is established: the bring-up said `NoIdentity`, or Platform proved it within 7
      days (the marker), or the wallet was created here (D3) → `NoIdentity{reason}` with `WaitingForSync` while SPV
      is not caught up, else `None`;
   4. else `StartupIncomplete{startup}` (D2, D16).

A failed status read is a `PlatformError` (`wallet_not_found`, a closed session, an app.sqlite read error): never a
guessed state.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | `status` reads memory and app.sqlite only (rows, settings, the identity snapshot, the sync tracker), in one call on the caller's thread like `identities()`. No network read, no library lock wait (`try_read` + snapshot, as DEC-138). | §2.1 says sync; the chip and sidebar read it on every DashPay event, offline included. |
| D2 | `StartupIncomplete` is reported only while no identity is known. An identity on file is `Ready` (or `Registering`/`ContestPending`) whatever the bring-up said; `sync_status` still carries the outcome. The contract's §2.1 row says so now. | The banner's job is the identity. A `--sync`-less launch (bring-up `NotRun`) with an identity on file must still show the chip and Contacts (DP2-07 F10's main case). |
| D3 | No identity known, but the wallet was created here (`dashpay.created_here`), or Platform proved the absence within its 7-day TTL (`dashpay.no_identity`, the bring-up's own rule): `NoIdentity`, not `StartupIncomplete`. Same marker reads as the bring-up (`bring_up_markers`, shared). | A new wallet's seed has no identity until this app registers one (which `dp_registration` shows). Without this a new wallet would hide Join until a bring-up runs. |
| D4 | A snapshot read (a pass holds the library lock) that lists no identity: the identity the bring-up named (`DashPayStartup.identity`), else the newest `Done` registration row with an identity and no failure, stands in. A live read is never overridden: an empty live list has no identity, whatever an older bring-up said. | The snapshot is empty before a pass's first snapshot; the bring-up's id and a finished flow are the same library's earlier answers. A live list is the library itself (review: the fallback must not outlive a removal). |
| D5 | A registration counts as in progress when its row is short of `Done` and has no failure, or a retryable one (DP1-02 `failure_of`). A non-retryable failure (including a row this build cannot read, DEC-171) does not: the user can join again; `finish_asset_locks` owns stranded locks. With an identity known, only a row naming that identity and short of its name step's end (`Draft`…`NameRequested`) is `Registering`: a second identity's registration does not hide the first one's Contacts, and an identity whose name is registered (or contested) is no longer registering. The newest such row's id is `draft`. | U-01 D11: Join DashPay while registering, Contacts once an identity exists. |
| D6 | `ContestPending` needs a live identity read (`live`, i.e. `names_updating == false`) whose shown names are empty or only the temporary name, and an open contest of the main identity: a `dp_contest_watch` row not `won:` (`Seen::Won` waits for the library to list the name), or a `Contested` registration row (the watch row may come one pass later). Label: the registration's `Contested` label if any, else the watch row ending first (unknown ends last). `ends_at` is the row's `ends_at`, `None` while 0. A snapshot read is `Ready`. | F5: the temporary name is the main name until the contest resolves; an identity that already owns another name is `Ready` (DP1-04 contests of a second name). The snapshot shows no names (DEC-138), so it cannot tell. |
| D7 | `WaitingForSync` = the sync tracker is not `caught_up()` (headers, filter headers, filters, masternodes and blocks; false while SPV is stopped). | §5: "Core or masternode sync has not finished, so Join is disabled". A stopped SPV cannot register either. |
| D8 | The startup map signals `Platform{Startup}` (new `PlatformChange::Startup`, bound as `{domain: "startup"}`) whenever a wallet's outcome or named identity changes, from one place: `write_startup()` returns a guard that compares on drop. m1 item 4 deferred this signal to E0-06; nothing sent it, so a bring-up ending `NoIdentity` re-read nothing. | The status depends on the outcome; hosts re-query on signals and never poll (§3.5). One hook covers the 12 write sites. |
| D9 | The renderer re-reads the DashPay status on `sync` and `spvStateChanged` too, and only the cached statuses that are `noIdentity` (`bridgeEvents` filters `["dashpay", "status"]` by its data), not every DashPay query. | `WaitingForSync` ends with sync, which sends no Platform signal, and only `noIdentity` carries it. Sync events come up to 4 Hz: an identity's wallet re-reads nothing. |
| D10 | DP2-07 D19 removed (its F11): Contacts reads the shell's status query; `null`/`orStub` and the `platform.not_implemented` mapping are gone, with their unit and e2e cases. With D2, `startupIncomplete` means no identity is known, so Contacts shows "Starting DashPay…" for it and no longer reads `identities` (D2 there updated). | The fallback's reason is gone; D2's `identities` read for `startupIncomplete` would never find an identity now. |
| D11 | The shell's main identity is `ready.main` or `contestPending.identity` (was `ready` only): a contested identity gets its bell count and Contacts badge. | U-01 D11: a contested username keeps Contacts; its requests still arrive. |
| D12 | The chip of an identity with no username says "No username" on a live read, "Updating…" only while the names are updating (`names_updating`) or the identities read has not answered. | With the real status a nameless identity (a name step that failed, `name_error`) is `Ready`; "Updating…" for ever would be wrong. |
| D13 | Sidebar gating is unchanged: Join DashPay for `noIdentity` and `registering`, Contacts otherwise (also while the status is unknown or `startupIncomplete`, where the page says "Starting DashPay…"). | U-01 D11. Unknown → Contacts avoids a Join flash for the common, registered case; `startupIncomplete` may still find an identity, so Join would invite a second registration. |
| D14 | dw-app's IPC smoke test used `dashpay_status` as its example of an engine refusal crossing the bridge; it now uses `dashpay_sync_now` (still a stub) and asserts `dashpay_status` answers `noIdentity` with `waitingForSync` for its new, offline wallet. dwcli's tests did the same: the command table lists `dashpay status` as implemented, `dashpay_cli` checks its line and runs the refusal through `test_stub` (`invite pending`), and the shutdown tests (`dashpay_r5`, `dashpay_r6`) keep `dashpay status` as their command and expect its success line (exit 0 when the shutdown is healthy). | The refusal path stays covered; the shutdown tests only need a command that prints one line. |
| D15 | A registration row funded by an existing identity (`Funding::ExistingIdentity`, a name added to an identity that already exists) or whose name step failed (`name_error`) is never `Registering`, with or without an identity known (on a snapshot read without one, such a row means an identity exists, so the status waits: D16). | Review: the identity existed before the flow, or its flow ended without a name; either way it is the user's identity now (the chip says "No username", D12). |
| D16 | A snapshot read proves no absence: without a live read, neither the markers nor the bring-up's `NoIdentity` give `NoIdentity`; the status is `StartupIncomplete{startup}` (the bring-up's outcome as is, `NoIdentity` included) until the pass ends and `Platform{Identities}` re-reads it. Markers are read only on a live, empty read. | Review: the pass holding the lock may be the one finding the identity; `NoIdentity` would offer Join (a second registration) for that moment. "Starting DashPay…" is the safe transient. |
| D17 | `status` takes the identity list, the live flag and the main-name rows from one read (`identity_read`, which `identity_summaries` now wraps), not a second `dp_prefs` read. | Review: one consistent read; one SQLite read fewer. |
| D18 | The contest watch sends `Platform{Names}` when a pass learns a contest's end (`ends_at` changes on an open or unplaced verdict) and when it drops a watch unplaced for a day. | `ContestPending.ends_at` and the pending label come from these rows; nothing else signalled their change. |

## Follow-ups

| # | What | Owner |
|---|---|---|
| F1 | A live check of a `StartupIncomplete` wallet (a restore before its bring-up) could ask Platform once; `status` stays local by design, `discover_identities` and the bring-up cover it. | none (by design) |
| F2 | DP1-08's banner states, Home card and Join intro render every variant, `WaitingForSync` disabled Join included. | DP1-08 |
| F3 | `StartupIncomplete{NotRun}` on a `--sync`-less launch of a restored wallet with no identity on file: say sync is off (DP2-07 F10). | E0-05 / launch-flag owner |
| F4 | The `dashpay.no_identity` marker expires after 7 days with no signal: a restored wallet without an identity, whose bring-up has not run since, turns from `NoIdentity` into `StartupIncomplete` silently until the next DashPay event re-reads it. A timer signal at the expiry would close it. | E0-05 owner (backlog) |

## Tests

| What | Test | Guards |
|---|---|---|
| Each rule and its precedence | `banner_tests::rules_*` (pure `derive_status`) | D2–D7, D15, D16 |
| Snapshot reads: the bring-up's identity, the newest finished flow's, no absence | `banner_tests::rules_a_snapshot_read_falls_back_to_identities_on_file` | D4, D16 |
| The contest watch's `Names` signal on a learned end and a dropped unplaced watch, none on an unchanged end | `contest_watch_tests::a_contest_still_voting_is_adopted_and_learns_its_end`, `an_unplaced_watch_ends_after_a_day_without_news` | D18 |
| Real session: a new wallet is `NoIdentity{WaitingForSync}` offline; a restore is `StartupIncomplete{NotRun}` until the proven-absence marker (within its TTL); a registration row makes it `Registering`; an identity on file is `Ready`; a contest row with only the temporary name is `ContestPending`; a watch-only wallet; an unknown wallet | `banner_tests::a_session_reads_the_status_from_what_is_on_file` | D1, D3, D5, D6 |
| `Platform{Startup}` on a changed outcome or identity, none on an unchanged write, one on a removal | `banner_tests::startup_changes_are_signalled_once` | D8 |
| Contract: `DashPay.status` in `IMPLEMENTED`, `wallet_not_found` for an unknown wallet | `tests/m4_dashpay_contract.rs` | — |
| dw-app: `dashpay_status` over IPC | `ipc_tests` | D14 |
| dwcli: `dashpay status` one-shot, in a session, with an unknown wallet | `dwcli` `dashpay::tests`, `tests/dashpay_cli.rs`, `dashpay_r5.rs`, `dashpay_r6.rs` | D14 |
| Renderer: chip per state, invalidations, sync re-reading only `noIdentity` statuses | `src/components/shell.test.ts`, `src/api/events.test.ts` | D8, D9, D12 |
| Contacts: owner per status (no `null` case) | `contacts/model.test.ts` | D10 |
| Playwright: chip and sidebar per scenario (contest pending added); the D19 stub case removed from Contacts | `e2e/routes.spec.ts`, `e2e/contacts.spec.ts` | D10–D13 |

Mutation check (`/work/scratch/dw-bl153/mutants.py`, 35 hand-written mutants of `banner.rs`, `runtime.rs`,
`startup.rs` and `contest_watch.rs`, each run against `banner_tests` and `contest_watch_tests`): 34 killed. The one
survivor is equivalent: dropping `read.live` from the gatherer's marker read changes nothing observable, because
`derive_status` ignores the markers on a snapshot anyway; the guard only saves a SQLite read.
