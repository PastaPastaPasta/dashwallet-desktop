# BL-162: app writes to Platform "fail once SPV has synced" (DP2-08 F6)

Backlog row BL-162. DP2-08's testnet GL (D32, F6 there) saw two of three contact-request sends from the real app end
after about 2 min in "Could not reach Dash Platform. Try again." with nothing on Platform, while a dwcli send went
through in 6 s. D32 read the failures as "after the status bar said Synced" and suspected the trust layer: a write's
proof waiting on SPV for a Platform quorum SPV could not find (`Quorum not found: type 6_25/67`).

**Result: the trust layer is cleared and the failure does not reproduce. The cause is outside the desktop.** The
premise was wrong: both failed sends were made while SPV was still syncing, and "Synced" was only the state when the
failure surfaced 2 min later. Profile writes from the real app succeed before and after masternode sync, and with the
app starved of CPU. After masternode sync the pinned dash-spv holds and verifies testnet's Platform quorums, so every
key is served SPV-verified. No desktop, dash-spv or platform-wallet defect was found, so there is no behavioural fix.
The 123 s both failures took is the SDK's DAPI budget, most simply a transition Platform accepted and never committed.
What changed is diagnosability: the app logs when asked (`DW_LOG`), the provider logs where each key came from, there
is a GL probe for timed writes, and three tests pin the cleared behaviour.

| Part | Where |
|---|---|
| `DW_LOG` (EnvFilter, stderr, off by default), and the usage line | `rust/crates/dw-app/src/lib.rs` (`run`), `launch.rs` (`USAGE`), `Cargo.toml` |
| "quorum key served `verified=`" / "quorum key refused" debug lines | `rust/crates/dw-engine/src/platform/trust.rs` (`quorum_public_key`) |
| Tests | `trust_tests.rs`: `after_masternode_sync_the_degraded_mode_falls_back_at_once`, `a_hung_spv_lookup_is_bounded_and_falls_back`, `a_hung_spv_lookup_ends_with_the_enforcing_wait` |
| GL probe: one profile edit from the real app at a chosen sync phase, or reads only | `apps/desktop/e2e-tauri/profile-write.ts`, `scripts/e2e-tauri-profile-write.sh` |

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
two later sends went through. The renderer shows "Could not reach Dash Platform. Try again." only for
`platform.unavailable` or `platform.timeout` (`contacts/actions.ts`): DAPI transport outcomes (`DapiClientError`,
`StaleNode`, `TimeoutReached`, an exhausted retry), never a trust refusal, which maps to `platform.context_unavailable`
or `platform.trust_mismatch` (`errors.rs`).

Both failures took exactly 123 s (21:40:23 → 21:42:26, 21:45:43 → 21:47:46): a fixed budget, not a flaky network. The
engine passes no settings, so the SDK's apply (platform fork at ebe37f8a):

| DAPI call | Per attempt | Attempts | Worst case |
|---|---|---|---|
| reads, nonce fetch, broadcast (`rs-sdk` `DEFAULT_REQUEST_SETTINGS.retries = 3`, rs-dapi-client's 10 s) | 10 s | 4 | 40 s |
| `waitForStateTransitionResult` (its own `RequestSettings` in `rs-dapi-client` `transport/grpc.rs`) | 30 s | 4 | 120 s |

Two readings fit 123 s:

- (a) the broadcast was accepted (check_tx passed), the wait got no result in 4 × 30 s, and the engine's read-back
  (`requests.rs` `read_back`) found nothing at once;
- (b) the broadcast failed after 40 s, the SDK refreshed the identity nonce for 40 s more (`broadcast.rs`, on a failed
  broadcast), and the read-back took the last 40 s.

(a) is the simpler: reads answered moments before, and it is "reads through, writes time out" (D5), a transition
Platform accepted and never committed. Without a log neither can be proven.

### The trust provider before and after masternode sync

The default mode is `Degraded` (DEC-168). No session opens in `Enforcing`, so nothing waits for SPV on account of
masternode sync. `writes_wait` and the 120 s `UNKNOWN_QUORUM_WAIT` are `Enforcing`-only. In `Degraded` each proof's key
comes from one bounded SPV lookup (5 s) and the trusted service:

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
| 4 | real app at this branch's head, fresh root | after masternode sync | see the report (GL) |

The record with times is `/work/scratch/dw-bl162/writes.md`. Logs: `/work/scratch/dw-bl162/{t2/r1,w2,w3,ro1,…}`.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | `DW_LOG` stays: an env var in EnvFilter syntax, stderr only, off by default, the twin of dwcli's `DWCLI_LOG`, listed in `--help`. Colour only on a terminal; a logger already installed is reported (`DW_LOG ignored: …`), never a panic. No file sink. | Small and safe: nothing logs unless a developer asks. The app writes no log file, but a desktop session may keep stderr (journald, `~/.xsession-errors`), and the log names identities, addresses and contacts, so it is a developer switch, not a user setting. No crate on the path logs phrases, seeds, keys or passphrases at any level (Opus review); the GL scans its logs for them (none in w2–w4, ro1). Without it D32 had no evidence. |
| D2 | The reproduction writes are profile edits sent through the app's own IPC from the renderer context (tauri-driver, `authorize` + `dashpay_update_profile`). | The renderer has no profile editor at 8276bf62. The command, its capability and the engine path are the ones a screen would use. Contact requests are excluded: the fixture has no unrelated pair left. |
| D3 | Every probe uses a fresh data root, as DP2-08's GL did. | The fresh root is what made SPV miss the Platform quorum for the first minute: the condition D32 blamed. |
| D4 | CPU starvation: the app's unit pinned to two CPUs, with four busy loops on them (memory: busy hogs, not `taskset` alone). The app was checked to run with `Cpus_allowed_list: 30-31`. | It imitates D32's load of 110 on 32 CPUs and a small laptop, without touching the code. |
| D5 | The cause is recorded as outside the desktop and unproven: a testnet DAPI/Platform episode during 21:40–21:48 UTC that let reads through and made writes time out, at high box load. No upstream patch is proposed. | The trust layer is ruled out by code (the codes, the modes) and by measurement. Nothing in dash-spv or platform-wallet misbehaved in any run. The two failures sit in one 8-minute window that writes outside it never hit. The public explorer (testnet.platform-explorer.pshenmic.dev) was down (503), so a stall couldn't be checked. |
| D6 | No behavioural change to the trust provider. The three new tests pin what was cleared (degraded mode after masternode sync falls back at once; a hung SPV lookup is bounded in both modes; in the enforcing wait it ends at the wait's deadline) and hold for the base's behaviour; the two hung-lookup tests need D7's setter to build. Dropping the wait's `limit.min(left)` fails the third. | The brief's "tests that fail before the fix" has no fix to fail against. A test that failed would mean inventing a defect. |
| D7 | The SPV lookup bound becomes a field (`spv_lookup_timeout`) with a test-only setter, next to `with_waits`. Production keeps 5 s. | The hung-lookup test would otherwise take 5 s per mode. |
| D8 | The "quorum key served/refused" lines are `debug`, and their fields are formatted only when enabled. | One line per proof is too many at `info`. D32's question, verified or trusted, needs exactly this line. |
| D9 | The probe has a `never` mode: the same run, reads only. | The before/after-sync measurement needs no write budget. |
| D10 | "Could not reach Dash Platform. Try again." stays the message for these failures. | The last DAPI outcome was a transport failure, and the engine reads Platform back before it reports one (`requests.rs` `read_back`): a request found there is reported sent. It is not proof the write is absent: under reading (a) the broadcast was accepted, and when the read-back fails too the original error is returned. A retry is then safe for these writes (a second request to the same contact breaks the contract's unique index on owner, recipient and account reference; a profile edit is idempotent), so a message that admits the outcome is unknown belongs to E0-04's `platform.broadcast_unknown` (P3), not here. |

## Follow-ups

| # | What | Owner |
|---|---|---|
| F1 | Correct DP2-08's D32 and F6: the failed sends went out during sync ("Syncing 72.8 %"), not after Synced. F6 is closed by BL-162 (trust layer cleared, not reproduced). | DP2-08 doc owner |
| F2 | If a write fails like this again: rerun with `DW_LOG=info,dw_engine=debug,dash_sdk=debug,dash_sdk::platform::transition=trace,rs_dapi_client=debug` (each failed DAPI attempt and ban is logged; broadcast success is only at `trace`, which tells reading (a) from (b)), or `scripts/e2e-tauri-profile-write.sh` with `DW_PROFILE_WRITE_AT=early`. dw_engine prints a quorum hash in display order (`00000000c9…`), dash-spv's `Quorum not found` in byte order (`…00000000`): reverse one to match. | whoever sees it |
| F3 | A log file a user can send. A Windows release build has no console, so `DW_LOG` writes nowhere there. | packaging / support |
| F4 | A long write shows nothing for 2 min before failing. A progress or "still trying" state on the card, or a `PutSettings::wait_timeout` shorter than the 4 × 30 s wait, would help. | DashPay UX |

## Tests

- `cargo test -p dw-engine --lib trust_tests`: the three new tests, alongside the existing per-rule tests.
- GL: `DW_TESTNET_FIXTURE=<dir> DW_PROFILE_WRITE_AT=synced scripts/e2e-tauri-profile-write.sh` (one profile edit of
  fixture wallet A after masternode sync, read back; `never` for reads only; `early` for the headers phase).
