# BL-162: app writes to Platform "fail once SPV has synced" (DP2-08 F6)

Backlog row BL-162. DP2-08's testnet GL (D32, F6 there) saw two of three contact-request sends from the real app end
after about 2 min in "Could not reach Dash Platform. Try again." with nothing on Platform, while a dwcli send went
through in 6 s. D32 read the failures as "after the status bar said Synced" and suspected the trust layer: a write's
proof waiting on SPV for a Platform quorum SPV could not find (`Quorum not found: type 6_25/67`).

**Result: the cause of DP2-08's two failures is unresolved.** They left no engine log, and nothing found since names
it. What the evidence does show:

- **Writes after sync did not fail (3/3).** Four profile edits from dwcli and the real app on fresh roots succeeded:
  three after masternode sync (one of them at this branch), one in the headers phase with the app starved of CPU.
- **The failed sends were pressed before masternode sync**, about 7 s after launch ("Syncing 72.8 %" and "73.2 %").
  "Synced" was only the state 2 min later, when they failed, so D32's "after Synced" premise was wrong.
- **The 123 s both took matches the SDK's DAPI budget**, by either of two paths (below). It doesn't say which call ran
  out, or whether Platform received a transition.
- **The trust provider did not wait them out.** The default degraded mode spends at most the 5 s SPV lookup bound per
  key before it falls back, never the 120 s wait (enforcing-only). A trust refusal shows a different message. After
  masternode sync the pinned dash-spv verifies testnet's Platform quorums, so every key is served SPV-verified.

No desktop, dash-spv or platform-wallet defect was found, so there is no behavioural fix. What changed is
diagnosability: the app logs when asked (`DW_LOG`), the provider logs where each key came from, there is a GL probe for
timed writes confined to a scratch root, and three tests pin the trust behaviour that was checked.

| Part | Where |
|---|---|
| `DW_LOG` (EnvFilter, stderr, off by default), and the usage line | `rust/crates/dw-app/src/lib.rs` (`run`), `launch.rs` (`USAGE`), `Cargo.toml` |
| "quorum key served `verified=`" / "quorum key refused" debug lines | `rust/crates/dw-engine/src/platform/trust.rs` (`quorum_public_key`) |
| Tests | `trust_tests.rs`: `after_masternode_sync_the_degraded_mode_falls_back_at_once`, `a_hung_spv_lookup_is_bounded_and_falls_back`, `a_hung_spv_lookup_ends_with_the_enforcing_wait` |
| GL probe: one profile edit from the real app at a chosen sync phase, or reads only | `apps/desktop/e2e-tauri/profile-write.ts`, `scripts/e2e-tauri-profile-write.sh` |
| The probe's scratch boundary and leak sweep (vitest: `scratch.test.ts`); `driver.ts` re-exports its `leaks` | `apps/desktop/e2e-tauri/scratch.ts` |

## Evidence

### What D32 saw, re-read

The failed runs left no engine log: the app had no subscriber. Their screenshots and ledgers
(`/work/scratch/dw-dp2-08/gl-out-2`, `gl-out-3`; `<fixture>/DP2-08.failed-{1,2}.json`) give this timeline:

| Run | App launched | Send pressed | Status bar at Send | Ended | Status bar then | Result |
|---|---|---|---|---|---|---|
| gl-2 | 21:40:16 | 21:40:23 | "Synchronizing… 72.8 %", Syncing | 21:42:26 | Up to date, Synced | `platform.unavailable`/`timeout`, nothing on Platform |
| gl-3 | ~21:45:36 | 21:45:43 | "73.2 %", Syncing | 21:47:46 | Synced | same |
| dwcli probe B → D | | 21:49:23 | (one-shot `--spv`) | +6 s | | sent |
| gl-4 (log subscriber) | 21:58:02 | 21:58:08 | Syncing | +~6 s | | sent |

A fresh data root starts SPV at the 1,508,000 checkpoint. Its headers, filter headers and filters phases each start
near 96 % of a 1,573,187 tip, and masternodes start at 0, so 72.8 % is the first seconds of the headers phase. Every
send, failed or not, went out ~7 s after launch, before masternode sync. That is also why each card said "Unverified"
and why every proof logged `Quorum not found`: SPV had no masternode list yet, and the degraded mode served the trusted
key (design E0-10b).

The other facts at the time: `sar` puts agentbox's load at 110 (run queue 134) at 21:40, and 51–60 by 21:50 when the
two later sends went through.

"Could not reach Dash Platform. Try again." is the renderer's text for `platform.unavailable` and `platform.timeout`
(`contacts/actions.ts`). That is a family of causes, not a transport trace. In `errors.rs`:

| Code | From |
|---|---|
| `platform.unavailable` | SDK `DapiClientError`, `StaleNode`; an exhausted retry (`NoAvailableAddressesToRetry`) whose last error maps to nothing more specific; platform-wallet `IdentityDiscoveryIncomplete` with an unmapped cause; `ContactSyncUnreachable` |
| `platform.timeout` | SDK `TimeoutReached`; platform-wallet `SeedBindingUnanswered`, `FinalityTimeout` |

A trust refusal is outside the family (`platform.context_unavailable`, `platform.trust_mismatch`). Which member the two
failures were is unknown: no log recorded it.

Both failures took exactly 123 s (21:40:23 → 21:42:26, 21:45:43 → 21:47:46): consistent with a fixed timeout budget; the cause is unknown. The
engine passes no settings, so the SDK's apply (platform fork at ebe37f8a):

| DAPI call | Per attempt | Attempts | Worst case |
|---|---|---|---|
| reads, nonce fetch, broadcast (`rs-sdk` `DEFAULT_REQUEST_SETTINGS.retries = 3`, rs-dapi-client's 10 s) | 10 s | 4 | 40 s |
| `waitForStateTransitionResult` (its own `RequestSettings` in `rs-dapi-client` `transport/grpc.rs`) | 30 s | 4 | 120 s |

Two readings fit 123 s, among others:

- (a) the broadcast was accepted (check_tx passed), the wait got no result in 4 × 30 s, and the engine's read-back
  (`requests.rs` `read_back`) found nothing at once;
- (b) the broadcast failed after 40 s, the SDK refreshed the identity nonce for 40 s more (`broadcast.rs`, on a failed
  broadcast), and the read-back took the last 40 s.

Neither is confirmed. There is no transport trace of the failures, so the 123 s shows the SDK's budget was spent, not
which call spent it or whether Platform accepted anything.

### The trust provider before and after masternode sync

The default mode is `Degraded` (DEC-168). No session opens in `Enforcing`, so nothing waits for SPV on account of
masternode sync. `writes_wait` and the 120 s `UNKNOWN_QUORUM_WAIT` are `Enforcing`-only. In `Degraded` each proof's key
comes from one bounded SPV lookup and the trusted service. A lookup SPV doesn't answer costs up to its 5 s bound before
the fallback, never the 120 s wait:

- before masternode sync, SPV misses (`Absent`), so the trusted key is served unverified behind the durable latch;
- after it, SPV answers `Verified`, so SPV's key is served once the trusted key agrees (or the service is unreachable).

Measured on the real app with `DW_LOG=…,dw_engine=debug` (read-only run `ro1`, fresh testnet root, 4 min past sync):

| Phase | Keys served | SPV-verified | `Quorum not found` |
|---|---|---|---|
| before masternode sync (first 25 s) | 27 | 0 | 27 |
| after masternode sync | 75 | 75 | 0 |

dwcli's session on a fresh root (`r1`) behaves the same: misses before masternode sync, none after. dash-spv at the
pin (d3d50520a, #1150) does track LLMQ_25_67 quorums: the quorums it lacks are absent only until its first QRInfo.

### Live writes (profile edits on fixture wallet A; budget 6)

| # | Path | Phase | Result |
|---|---|---|---|
| 1 | dwcli `dashpay session --spv`, fresh root | ~3 min after masternode sync | ok, 3.8 s |
| 2 | real app, IPC `dashpay_update_profile`, fresh root | 61 s after Synced and masternodes done | ok, 1.9 s, read back |
| 3 | real app pinned to 2 CPUs with 4 busy loops on them, fresh root | headers phase, the phase of D32's failures | ok, 2.6 s, read back |
| 4 | real app at e040ac13, fresh root, `DW_LOG` with `dash_sdk::platform::transition=trace` | 61.5 s after Synced and masternodes done | ok, 1.7 s, read back; broadcast succeeded (trace); keys served 28/32 unverified before masternode sync, 29/29 SPV-verified after, none refused; no ANSI codes in the log |

The record with times is `/work/scratch/dw-bl162/writes.md`. Logs: `/work/scratch/dw-bl162/{t2/r1,w2,w3,w4,ro1,…}`.
The f1 check of the reworked probe (reads only, no write) is `ro2`.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | `DW_LOG` stays: an env var in EnvFilter syntax, stderr only, off by default, the twin of dwcli's `DWCLI_LOG`, listed in `--help`. Colour only on a terminal; a logger already installed is reported (`DW_LOG ignored: …`), never a panic. No file sink. | Small and safe: nothing logs unless a developer asks. The app writes no log file, but a desktop session may keep stderr (journald, `~/.xsession-errors`), and the log names identities, addresses and contacts, so it is a developer switch, not a user setting. No crate on the path logs phrases, seeds, keys or passphrases at any level (Opus review); the GL scans its logs for them (none in w2–w4, ro1). Without it D32 had no evidence. |
| D2 | The reproduction writes are profile edits sent through the app's own IPC from the renderer context (tauri-driver, `authorize` + `dashpay_update_profile`). | The renderer has no profile editor at 8276bf62. The command, its capability and the engine path are the ones a screen would use. Contact requests are excluded: the fixture has no unrelated pair left. |
| D3 | Every probe uses a fresh data root, as DP2-08's GL did. | The fresh root is what made SPV miss the Platform quorum for the first minute: the condition D32 blamed. |
| D4 | CPU starvation: the app's unit pinned to two CPUs, with four busy loops on them (memory: busy hogs, not `taskset` alone). The app was checked to run with `Cpus_allowed_list: 30-31`. | It imitates D32's load of 110 on 32 CPUs and a small laptop, without touching the code. |
| D5 | The cause of DP2-08's two failures is recorded as unresolved. Ruled out: the trust provider waiting them out (the codes, the modes, measurement), and "writes fail after sync" (four writes after it or in the same phase succeeded). Not established: which member of the unavailable/timeout family they were, or whether Platform received the transitions. Unconfirmed candidate: a testnet DAPI/Platform episode during 21:40–21:48 UTC, at box load 110. No upstream patch is proposed. | Nothing recorded the failures, and none of the probes reproduced them. The two sit in one 8-minute window that writes outside it never hit. The public explorer (testnet.platform-explorer.pshenmic.dev) was down (503), so a Platform stall couldn't be checked. Stating a cause would outrun the evidence. |
| D6 | No behavioural change to the trust provider. The three new tests pin what was checked (degraded mode after masternode sync never waits, with a fake SPV that answers at once; a hung SPV lookup costs at most its bound in both modes; in the enforcing wait it ends at the wait's deadline) and hold for the base's behaviour; the two hung-lookup tests need D7's setter to build. Dropping the wait's `limit.min(left)` fails the third. | The brief's "tests that fail before the fix" has no fix to fail against. A test that failed would mean inventing a defect. |
| D7 | The SPV lookup bound becomes a field (`spv_lookup_timeout`) with a test-only setter, next to `with_waits`. Production keeps 5 s. | The hung-lookup test would otherwise take 5 s per mode. |
| D8 | The "quorum key served/refused" lines are `debug`, and their fields are formatted only when enabled. | One line per proof is too many at `info`. D32's question, verified or trusted, needs exactly this line. |
| D9 | The probe has a `never` mode: the same run, reads only, with the same ending as a write (result.json, the leak scan of OUT including `tauri-driver.log`, PASS or FAIL). f1: it used to return early and skip that ending (Sol r1 P2). | The before/after-sync measurement needs no write budget, and a reads-only PASS must mean the same checks passed. |
| D10 | "Could not reach Dash Platform. Try again." stays the message for these failures. | It is the family's message, and the engine reads Platform back before it reports a contact request failed (`requests.rs` `read_back`): a request found there is reported sent. It is not proof the write is absent: the broadcast may have been accepted, and when the read-back fails too the original error is returned. A retry is then safe for these writes (a second request to the same contact breaks the contract's unique index on owner, recipient and account reference; a profile edit is idempotent), so a message that admits the outcome is unknown belongs to E0-04's `platform.broadcast_unknown` (P3), not here. |
| D11 | The leak scan is read-only outside what the run created. `DW_E2E_SCAN_EXTRA` is scanned and a leak there fails the run, but nothing there is deleted. A leaked file is deleted only if it is a regular file inside the run directory or OUT. `leaks()` (moved to `scratch.ts`, re-exported by `driver.ts`) skips symlinks, so no walk leaves the directory it was given; that holds for every probe that uses `driver.ts`'s `leaks` (`settings.ts` keeps its own, F5). The leak report lists what was deleted apart from what was left in place, on interrupt too. | Sol r1 P1: an extra scan pointed at the fixture would have deleted `<W>.mnemonic`. Deleting is a cleanup of the run's own output, not a reach into someone else's directory. |
| D12 | One scratch root holds every runtime file: `DW_PROFILE_ROOT` (default `$TMPDIR/dw-profile-write`), canonicalized and refused if it overlaps the checkout (is it, lies inside it, or holds it), before anything is created. Each run makes `run-XXXXXX` (0700) under it with the data root, the passphrase file, the app's `XDG_DATA_HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` (the single-instance lock and socket) and `TMPDIR`, and OUT (`<run>/out`). A caller's `DW_E2E_OUT` is canonicalized (symlinks resolved) and must be a new directory under the root; `result.json` is created exclusively. `DW_E2E_SCAN_EXTRA` may not hold the root (the run's own passphrase file would read as a leak). At the end, everything in the run but OUT is deleted, after the verdict is formed; a failure to delete is reported, not thrown over the verdict. `HOME` is not redirected: nothing in a run was seen writing there (ro2: nothing new in `~/.local/share`, `~/.cache`, `~/.config`). | Sol r1 P3: OUT defaulted into the checkout and the data root went to `tmpdir()` apart from it. Pointing the app's XDG dirs into the run also keeps WebKit's caches and storage out of `$HOME`, and the scan covers them. |
| D13 | `scripts/e2e-tauri-profile-write.sh` does not build by default: it runs the binaries in `<target>/debug` and refuses if they are missing. `DW_E2E_SKIP_BUILD=0` opts into `scripts/e2e-tauri.sh`'s build (which writes `dist/`, and `rust/target` unless `CARGO_TARGET_DIR` is set). | Sol r1 P3: the delegated build wrote into the checkout by default. A build is an explicit developer step, not a side effect of a probe. |
| D14 | vitest also runs `e2e-tauri/**/*.test.ts`. `scratch.ts` imports Node built-ins only, so its tests need no driver environment. | The boundary rules need tests that fail if they break, and the desktop gate's vitest step runs them. |

## Follow-ups

| # | What | Owner |
|---|---|---|
| F1 | Correct DP2-08's D32 and F6: the failed sends went out during sync ("Syncing 72.8 %"), not after Synced. F6 is closed by BL-162 (trust layer cleared, not reproduced). | done on main d0fef6b4 (DP2-08 merge) |
| F2 | If a write fails like this again: rerun with `DW_LOG=info,dw_engine=debug,dash_sdk=debug,dash_sdk::platform::transition=trace,rs_dapi_client=debug` (each failed DAPI attempt and ban is logged; broadcast success is only at `trace`, which tells reading (a) from (b)), or `scripts/e2e-tauri-profile-write.sh` with `DW_PROFILE_WRITE_AT=early`. dw_engine prints a quorum hash in display order (`00000000c9…`), dash-spv's `Quorum not found` in byte order (`…00000000`): reverse one to match. | whoever sees it |
| F3 | A log file a user can send. A Windows release build has no console, so `DW_LOG` writes nowhere there. | packaging / support |
| F4 | A long write shows nothing for 2 min before failing. A progress or "still trying" state on the card, or a `PutSettings::wait_timeout` shorter than the 4 × 30 s wait, would help. | DashPay UX |
| F5 | `bell-accept.ts` and `add-contact.ts` still delete leaked files found under their `DW_E2E_SCAN_EXTRA` (the pattern of Sol r1 P1; their walks no longer follow symlinks, D11): port `removeOwned` from `scratch.ts`. `settings.ts` has its own `leaks()`, which follows symlinks: switch it to the shared one. | DashPay and settings GL owners |

## Tests

- `cargo test -p dw-engine --lib trust_tests`: the three new tests, alongside the existing per-rule tests.
- `pnpm test` (vitest) runs `e2e-tauri/scratch.test.ts`: lookalike paths, a root that is, lies inside or holds the
  checkout refused (before it is created), an output path through a symlink or `..` refused, an existing output refused, a directory symlink not
  followed, and a leak outside the owned directories reported and kept.
- GL: `DW_TESTNET_FIXTURE=<dir> DW_PROFILE_ROOT=<scratch> DW_PROFILE_WRITE_AT=synced scripts/e2e-tauri-profile-write.sh`
  (one profile edit of fixture wallet A after masternode sync, read back; `never` for reads only; `early` for the
  headers phase). Build the app and dwcli first, or set `DW_E2E_SKIP_BUILD=0`.
