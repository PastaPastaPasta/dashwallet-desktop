# H-09: time until Platform proofs get SPV-verified quorum keys

Status: research for the H-09 trust gate (MG). Measured 2026-11-22, 15:55–17:27Z, on agentbox, read-only on
**testnet**. Builds on `docs/research/dashpay/trust-spike.md` (E0-10a), which measured an older pin.

## Question

Under H-09 a DashPay contact payment refuses until the contact's data has been re-fetched with Platform proofs whose
quorum keys SPV holds as `Verified`. How long after (a) a fresh install and (b) a restart of a synced data dir does the
wallet start serving `verified = true` keys for the quorums live Platform proofs cite?

## Answer

| Start | Samples | First `verified = true` (median) | Worst | Verified share of lookups, minute 5 to end of run (≤ 62 min) |
|---|---|---|---|---|
| Fresh install (empty data dir) | 5 | 60 s | 60 s | 100 % |
| Restart of a synced data dir | 5 | 8 s | 9 s | 100 % |

The pin no longer has the E0-10a problem. Fresh install → first SPV-verified key was 4.0 h mainnet / 7.6 h testnet on
the older pin, and restarts stalled. Now a fresh install verifies as soon as masternode sync finishes (24–41 s), and a
restart verifies from its stored list in under 10 s, before masternode sync reports done. After that, every lookup in
every run was `verified = true`: 100 % of lookups from minute 1 to the end of each run, with no refusals and no
mismatches.

Platform moved to a newly mined quorum every 3–7 minutes (11–12 quorums an hour). In every run each new quorum
was already `Verified` the first time a proof cited it. The only `verified = false` keys came before masternode sync
on a fresh install (the first 30–60 s) and in the first 3–9 s of a restart, when the trusted service supplied the key
as the degraded mode allows.

For the H-09 gate this means a contact payment would wait at most about a minute after a fresh install and about ten
seconds after a restart, plus the re-fetch itself. Resolution is the 30 s query cadence for fresh installs: the first
query after masternode sync was always verified.

## Method

- Build: `dwcli` (`rust/bin/dwcli`) at `9c3fc3a81`, `cargo build --release -p dwcli` with
  `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0`. rust-dashcore is the
  PastaPastaPasta fork at `570d686cbb` (`rust/Cargo.lock`); platform at `373342cb38`. Engine code unchanged.
- Trust mode: the session default, `Degraded` (`trust.rs` `DEFAULT_MODE`). It serves an SPV `Verified` key when SPV
  holds one (after checking it against the trusted service) and the trusted key otherwise, so `verified = true` in
  the debug log is exactly "SPV held this quorum as `Verified`", the condition the H-09 rule needs.
- Each run: a fresh data dir under `/work/scratch/h09-ttv/`, a vault encrypted with a scratch passphrase, and a
  throwaway wallet from a newly generated mnemonic (no funds). Then
  `dwcli --network testnet --verbose-events dashpay session --spv`, with stdin fed by a scratch script that sends
  `{"args":["identity","find","dash"]}` every 30 s (the DPNS name `dash` resolves on testnet; each request verifies
  several proofs, so 4–20 lookups per request). `DWCLI_LOG=warn,dw_engine::platform::trust=debug,dw_engine=info,dash_spv=info`.
- Measured per run, from process start: masternode sync done (the first `Sync` event with both the `Headers` and
  `Masternodes` phases done, the condition `SyncTracker::masternodes_synced` uses), the first
  `quorum key served … verified=true`, and the share of `quorum key served/refused` lines with `verified=true` in
  the five minutes before the 10, 30 and 60 minute marks.
- Runs stopped by closing stdin, so dwcli shut SPV down cleanly and the restarts reopened clean storage.
- Load: agentbox's 32 vCPUs were at load 35–80 throughout; up to five clients ran in parallel.

Runs:

| Run | Start type | Started (Z) | Ran | Data dir |
|---|---|---|---|---|
| f1, f2 | fresh install | 15:55:11 | 61 min | own |
| p3, p4, p5 | fresh install | 15:56:35 | 10 min | own; stopped to be restarted |
| r3, r4, r5 | restart, 0 min offline | 16:06:54 | 62 min | p3, p4, p5 after their 10 min |
| r1 | restart, 0 min offline | 16:56:17 | 31 min | f1 after its 61 min |
| r2 | restart, 20 min offline | 17:16:22 | 11 min | f2 after its 61 min |

## Results per run

"MN sync" is masternode sync done, "first verified" the first `verified = true` key, both seconds after process start.
Shares are verified/all lookups in the window.

### Fresh installs

| Run | Load (1/5/15 min) | MN sync | First verified | 0–10 min | 5–10 min | 25–30 min | 55–60 min |
|---|---|---|---|---|---|---|---|
| f1 | 50 / 81 / 78 | 35 s | 60 s | 109/139 (78 %) | 50/50 (100 %) | 50/50 (100 %) | 50/50 (100 %) |
| f2 | 50 / 81 / 78 | 41 s | 60 s | 90/124 (73 %) | 50/50 (100 %) | 50/50 (100 %) | 50/50 (100 %) |
| p3 | 66 / 81 / 79 | 24 s | 30 s | 93/124 (75 %) | 50/50 (100 %) | – | – |
| p4 | 66 / 81 / 79 | 26 s | 30 s | 93/124 (75 %) | 50/50 (100 %) | – | – |
| p5 | 66 / 81 / 79 | 33 s | 60 s | 109/139 (78 %) | 50/50 (100 %) | – | – |

The 0–10 min share is below 100 % only because of the lookups before masternode sync. f1 and f2: 202/202 lookups
verified in 10–30 min and 303/303 in 30–60 min.

### Restarts

| Run | Load (1/5/15 min) | Offline before | MN sync | First verified | 0–10 min | 5–10 min | 25–30 min | 55–60 min |
|---|---|---|---|---|---|---|---|---|
| r3 | 60 / 61 / 69 | 0 min | 11 s | 8 s | 117/117 (100 %) | 50/50 (100 %) | 50/50 (100 %) | 50/50 (100 %) |
| r4 | 60 / 61 / 69 | 0 min | 12 s | 8 s | 117/117 (100 %) | 50/50 (100 %) | 50/50 (100 %) | 50/50 (100 %) |
| r5 | 60 / 61 / 69 | 0 min | 15 s | 9 s | 117/122 (96 %) | 50/50 (100 %) | 50/50 (100 %) | 50/50 (100 %) |
| r1 | 50 / 78 / 81 | 0 min | 16 s | 9 s | 117/122 (96 %) | 50/50 (100 %) | 50/50 (100 %) | – |
| r2 | 35 / 36 / 47 | 20 min | 13 s | 7 s | 117/117 (100 %) | 50/50 (100 %) | – | – |

r3–r5: 202/202 verified in 10–30 min, 303/303 in 30–60 min; r1: 202/202 in 10–30 min. r1 was stopped at 31 min
and r2 at 11 min, so they have no 60 (r1) or 30 and 60 (r2) minute figure. In r3, r4 and r2 the first lookup came at
7–8 s and was already verified; in r5 and r1 the first came at 3 s, before SPV had loaded its list, and the trusted
key was served until 9 s. r2's first lookup cited `00000100d22a5066…`, a quorum mined while it was offline, and SPV
held it `Verified` at 7 s, before masternode sync reported done (13 s).

## Quorums cited

All proofs cited LLMQ type 6 (testnet's Platform quorum type). Fresh installs saw, in order:

| Quorum hash | First cited (f1) | First `Verified` |
|---|---|---|
| `0000003386e4f9894eba832f7aece3379c52013d17330e95030db11b139e0aa6` | 6 s, once per fresh run, before sync | never asked again |
| `00000095ab79c0c344da0ecaf7ed37a1ce12e1d6f1cb97aec1348983284f3fe9` | 3 s | 60 s (first ask after sync) |
| `000000ad3311d4ed1ff6e97443159386c7cc46a74695347dd8dd80a05a948e46` | 137 s | on first citation |
| `0000012825a0daeddcabe3a7ed397accd1a20573de5cb0a6f1688ac06de62761` | 540 s | on first citation |
| `0000008d3ebd1416…`, `000000ba0e181882…`, `0000008527f33b8f…`, `00000085118d02a4…`, `00000053bfc36ad8…`, `00000174adee3237…`, `0000004bc8a4724a…`, `00000035ee1a8ecd…`, `000000915ac79309…` | 900–3506 s | each on first citation |

The restarts cited the same sequence from `00000128…` on, plus `0000009e6dc5ce9e…`, `00000081c3c12da7…`,
`00000100d22a5066…`, `000000e506a3b422…` and `000000718a82cc1c…` after f1 and f2 had stopped; each was `Verified` on
first citation too. `00000033…` was asked once per fresh install, at
4–6 s, before masternode sync, and no proof cited it again, so whether SPV later verified it was not observed.

## Limits

- Testnet only, one afternoon. E0-10a's failure on the older pin was about which quorums SPV left
  `Skipped(UnknownBlock)`, which can depend on where the chain is in its DKG cycle; this sample covered about one
  hour of rotations (12 quorums) per run.
- The longest offline gap before a restart was 20 minutes (r2). A restart after days offline (a stale tip several cycles
  behind) was not measured. A restart after days offline is still to be measured (1.0.x).
- 30 s query cadence: first-verified times for fresh installs are rounded up to the next query after masternode
  sync.
- Parallel clients on one host, at load 35–80: sync times are likely upper bounds for an idle desktop.

Raw logs were in `/work/scratch/h09-ttv/` (agentbox, deleted after the write-up).
