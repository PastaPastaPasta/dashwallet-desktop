# E0-10a trust spike: can dash-spv anchor Platform proofs?

Status: research (DASHPAY §2.2 gate). Measured 2026-10-09 → 2026-10-10 on agentbox, read-only on testnet and mainnet.
Probe code: `tools/trust-spike/` (README there). Raw data: `/work/scratch/e0-10a/run1/` (agentbox, not tracked).
`trust-spike-data/` beside this file holds `summary.json` (`tools/trust-spike/analysis/summary.py` over run1) and
the quorum base-block heights.

Revisions:

| What | Revision |
|---|---|
| platform (the pin) | `bc41f1bc23` (v5.0-dev; `rust/Cargo.toml`) |
| rust-dashcore at the pin | `40268cc0` (platform's lock; also v5.1-dev's today) |
| rust-dashcore `dev` | `c19973ab` (contains #1072 `8820cf91`, #1075 `896b0534`, #1094 `c5a9b944`) |
| probe binaries | `961927f` for the 25 h and morning runs; post-day runs used binaries built from `0dc6b0a`'s Rust source (identical to the head's) |

---

## 0. Verdict

**R7 at the pin: FAIL.** dash-spv at `40268cc0` holds every quorum that real proofs cite, with the trusted service's
key: 0 misses and 0 key mismatches over a day, on both networks. But it leaves most of them `Skipped(UnknownBlock(..))`,
and a quorum that starts out `Skipped` stays that way until it rotates out. After the first masternode sync, 4 of the
24 active Platform quorums were `Verified` (on testnet, two pairs 288 blocks apart, not the newest). After that, each
quorum whose commitment was mined while the client ran became `Verified`, except the first one after the start. The
first `Verified` key came when Platform first cited one of those quorums: after 4.0 h on mainnet and 7.6 h on testnet.
Over the day, 57 % (mainnet) and 62 % (testnet) of live lookups were `Verified` on first ask. Every restart starts again
at 4 of 24, and two of the pin's three mainnet restarts stalled for 20 min.

**`dev` closes most of the status gap.** At `c19973ab`, 100 % of live lookups were `Verified` on first ask, on both
networks, with 0 misses and 0 mismatches. The first verified key came 45 s after a fresh install and 10–16 s after a
restart.

One gap remains at `dev`. The first Platform quorum mined after the start stayed `Skipped(MissedList)` for 11 h on
mainnet and 7 h on testnet before it became `Verified`. Platform did not cite it in that window, so the 100 % owes
something to timing: a provider requiring `Verified` would have refused every proof citing it for those hours (§4.2).

The gain is attributed to #1094 by mechanism (`UnknownBlock` ↔ #1094's height resolution), not isolated by a run.
#1072 and #1075 are not what closes it: the pin already found every live tuple (§4).

**Enforcement still needs three more things** (§6):

- a fix for that first quorum after the start;
- a lookup the desktop can hold to `Verified`. Every consumer drops the status, so `trust.rs` cannot require it through
  platform-wallet today;
- a quorum list anchored to the chain. `Verified` is necessary but not sufficient: until dash-spv checks each
  MnListDiff against the coinbase merkle roots (dashpay/rust-dashcore#1117, open), a verified quorum is not anchored
  to the chain.

Until all three land, 1.0 ships DASHPAY §2.2's degraded mode.

| Gate metric (DASHPAY §2.2) | Pin `40268cc0` | `dev` `c19973ab` |
|---|---|---|
| Holds the Platform type at the cited heights | yes: 332/332 mainnet, 368/368 testnet live tuples found, keys equal | same |
| Status of those entries, first ask (live) | `Verified` 57 % mainnet, 62 % testnet; the rest `Skipped(UnknownBlock)` | `Verified` 100 %; one active quorum was not `Verified` for 7–11 h, and no proof cited it then |
| Fresh install → first SPV-verified key | 4.0 h mainnet, 7.6 h testnet | 45 s on both (= masternode sync) |
| Restart → live tuples `Verified` | testnet: 0 % (2 morning restarts and the post-day one); mainnet: 2 of 3 restarts stalled for 20 min. The post-day restart served 5/5 live tuples `Verified` (they cited the newest quorum), but only 6 of the day's 339 tuples | 100 % on first ask; first verified key after 10–16 s |
| Miss rate over one day | 0 % | 0 % |

---

## 1. Method

**Part 1, at platform's pin.** `trust-spike-pin capture` fetches, every 120 s, the DPNS and DashPay contracts (both
networks) and the two fixture identities (testnet), each from one evonode picked at random from the SDK's seed list.
The SDK verifies each proof through a `ContextProvider` that forwards to the trusted quorum service
(`TrustedHttpContextProvider`, as `dw-engine` uses) and records every
`get_quorum_public_key(type, hash, core_chain_locked_height)` call. Those calls are exactly the (type, hash, height)
tuples real proofs cite. A fresh trusted provider per query keeps its cache from answering, so every tuple also gets
the service's key.

**Part 2, dash-spv probes.** `trust-spike-{pin,dev} spv` runs a dash-spv client configured like the desktop's
(`dw-engine` `Session::spv_config`: masternode sync on, default peers; filters off, see §8), tails the tuple log and
asks `DashSpvClient::get_quorum_at_height(ccl, type, hash)` for each tuple, the call platform-wallet's
`SpvRuntime::get_quorum_public_key` makes (`rs-platform-wallet/src/spv/runtime.rs:340` at the pin), with the same byte
reversal. It logs whether the quorum was found, its `LLMQEntryVerificationStatus`, and whether its key equals the
trusted service's. A tuple is asked within 15 s of its capture and re-asked for 30 min until it resolves `Verified`.
The same probe source builds at the pin and at `dev`.

Runs (all started 2026-10-09 ≈09:05Z unless noted):

| Run | What |
|---|---|
| `cap-{testnet,mainnet}` | Part 1, 780 rounds × 120 s (26 h) |
| `{pin,dev}-{testnet,mainnet}` | fresh install, then 25 h live |
| `restart-{pin,dev}-{testnet,mainnet}` | fresh install, then two restarts on the same storage, 20 min each (09:09–10:12Z) |
| post-day | 2026-10-10 10:08–10:28Z: each 25 h store reopened (a restart after a day), plus a fresh install, at the pin and at `dev`, on both networks. All 8 clients ran in parallel for 20 min, and each asked every tuple of the day. |

Definitions used below (`tools/trust-spike/analysis/analyze.py`):

- a tuple is **live** when the trusted service keyed it and the probe first asked for it after that run's masternode
  sync had completed;
- a **hit** is a lookup that returned the quorum with the trusted service's key. A different key fails the proof, so
  it counts as a miss (none occurred);
- a **verified hit** is a hit whose entry is `Verified`.

The probe polls every 15 s, so every time to sync or to the first key is rounded up to that tick.

Read-only throughout. DAPI queries are reads. SPV peers are public mainnet/testnet nodes. The local oracles were used
only for pure `getblockheader` calls (testnet quorum heights), never as SPV peers, because dash-spv sends `getqrinfo`
and `getmnlistdiff`, which are not pure reads there. Mainnet quorum heights come from the public Insight API, since the
mainnet oracle has not reached the tip. Nothing touched `/work/chains` or `/work/bench`.

---

## 2. What real proofs cite

| | mainnet | testnet |
|---|---|---|
| span | 25.97 h, 780 rounds | 25.97 h, 780 rounds |
| queries (each from one random evonode) | 1 560 to 258 distinct evonodes | 3 120 to 30 distinct evonodes |
| proof verified (SDK, trusted keys) | 1 461 (93.7 %) | 3 120 (100 %) |
| quorum types cited | 4 (LLMQ_100_67) only | 6 (LLMQ_25_67) only |
| distinct (type, hash, height) tuples | 346 | 384 |
| distinct quorums | 7 (+1 from stale nodes, below) | 30 |
| cited chain-locked heights | 2552329–2552916 | 1569078–1569737 |
| cited height − quorum's base-block height | 19–589 blocks | 19–585 blocks |
| cited height behind the SPV header tip (live, both revisions) | 0–3, median 0 | 0–4, median 0 |
| DAPI round trip incl. proof verification | p50 488 ms, p95 759 ms | p50 267 ms, p95 345 ms |

Platform's proofs cite one of the 24 active quorums of its type, not necessarily the newest one. The cited quorum stays
fixed for a stretch: about 50–110 blocks (2–5 h) on mainnet and 5–28 blocks on testnet. Within a round, every proof from
an up-to-date evonode cites the same quorum. Testnet cited 30 distinct quorums, and it came back to one it had cited 490
blocks earlier. Mainnet moved through 7 in the day. The ages fit the active window: a quorum's commitment is mined
within ≈ 20 blocks of its base block, and it stays active until 24 newer ones are mined, about 576 blocks later. The
heights are base-block heights; every one is a multiple of the DKG interval, 24.

**Stale evonodes.** Two mainnet evonodes (`158.247.208.247`, `149.28.223.171`) served proofs at chain-locked height
2532096 the whole day, about 20 300–20 800 blocks behind, citing a quorum whose base block is 2531760. Neither the
trusted service nor dash-spv (either revision) knows that quorum, so the SDK rejected all 12 such proofs (0.8 % of
mainnet queries) with "quorum not found". In 9 of those 11 rounds, another node's proof verified. In one, the other
query failed on the GroveDB envelope (below); in the other, both queries hit the stale nodes.

**Other mainnet failures** (not trust-related): invalid peer certificate 32, deadline 25, "unsupported GroveDB proof
envelope version 0" 21 (nodes serving a proof format the pin's SDK rejects), TLS fatal alert 5, other 4.

The capture ran about 35 min past the post-day runs. That is why §4.4 counts 339 and 376 tuples, against 346 and 384
here.

---

## 3. dash-spv at the pin (`40268cc0`)

### 3.1 Live, 25 h from a fresh install

| | mainnet | testnet |
|---|---|---|
| masternode sync done after | 45 s | 30 s |
| live tuples (trusted-keyed, first asked after sync) | 332 | 368 |
| found with the trusted key, first ask | 332 (100 %) | 368 (100 %) |
| `Verified` on first ask | 188 (56.6 %) | 229 (62.2 %) |
| first `Verified` key after start | 14 287 s (4.0 h) | 27 489 s (7.6 h) |
| key mismatches | 0 | 0 |
| entries not `Verified` | all `Skipped(UnknownBlock(<quorum hash>))` | same |

### 3.2 Why some are verified and some are not

The newest list after the first sync had 4 of its 24 Platform quorums `Verified` and 20 `Skipped(UnknownBlock)` on both
networks. On testnet the 4 have base blocks 1568736 and 1569024, and 1568760 and 1569048. Those are two pairs 288 blocks
apart (presumably QRInfo's lists at h − c, h − 2c, …) and not the newest; most mainnet hashes in that list have no
resolved height. No quorum that started `Skipped` ever became `Verified`; each left the list when it rotated out. Each
quorum mined while the client ran entered as `Verified`, except the first one after the start (base 2552328 on mainnet,
1569072 on testnet). That one stayed `Skipped(UnknownBlock)` for the rest of the run; its work block (base − 8) predates
the start. So the count rose by one per DKG cycle, from 4 to 23 (mainnet) and 24 (testnet) after 25 h.

The first verified key therefore arrives whenever Platform first cites one of the verified quorums. On mainnet that was
after 4.0 h, a quorum mined after the start (base 2552400). On testnet it was after 7.6 h, the initial quorum with base
1568760.

### 3.3 Restarts

| Run | mainnet | testnet |
|---|---|---|
| morning restarts 1 and 2 (same storage, 20 min each) | stalled at stored tip 2552339 for the full 20 min, no lookups served ("Received 8000 headers … but no segment matched", the #1102 symptom) | synced in 18 s; 0 % `Verified` |
| post-day restart (25 h store) | synced in 48 s; 5/5 live tuples `Verified` (Platform was citing the newest quorum, base 44 below the tip), but of the day's 339 tuples only those 6 | synced in 41 s; 0 of 376 `Verified` |
| post-day fresh install | synced in 122 s; 6 of 339 `Verified` | synced in 107 s; 0 of 376 `Verified` |

The pin keeps statuses in memory only, so every start begins again at 4/24. In the post-day runs, the pin reported
the rotated-out quorums as `Unknown` rather than `Skipped`. The mainnet stall did not recur in the post-day restart:
it is intermittent.

### 3.4 Memory

The pin keeps one masternode list per block and never drops one: 56 lists after sync, 617 after 25 h (mainnet). RSS
peaked during the first sync, within 15 min: 396 MiB on mainnet and 281 MiB on testnet. After that it grew with the
list count:

| Network | hours 1–6 | hours 18–26 |
|---|---|---|
| mainnet | 72–135 MiB | 108–185 MiB |
| testnet | 51–86 MiB | 66–114 MiB |

`dev` stayed far lower (§4.5).

### 3.5 Lookup latency

Median 0.004 ms, p99 0.025 ms. The worst lookups took 1.9 s (mainnet) and 1.3 s (testnet); this spike did not
investigate them (probably lock contention with masternode processing).

---

## 4. dash-spv at `dev` (`c19973ab`)

### 4.1 Live, 25 h from a fresh install

| | mainnet | testnet |
|---|---|---|
| masternode sync done after | 45 s | 45 s |
| first `Verified` key after start | 45 s | 45 s |
| live tuples | 332 | 368 |
| found with the trusted key, first ask | 332 (100 %) | 368 (100 %) |
| `Verified` on first ask | 332 (100 %) | 368 (100 %) |
| key mismatches | 0 | 0 |
| Platform quorums `Verified` in the newest list | 24/24 at sync and at the end; 23/24 from 0.5 h to 11.3 h (§4.2) | 24/24 at sync and at the end; 23/24 from 0.3 h to 7.4 h |
| lookup latency | median 0.018 ms, p99 1.0 ms, max 7.5 ms | median 0.014 ms, p99 0.6 ms, max 88 ms |

### 4.2 The first quorum after the start

At `dev`, the same quorum the pin never verified (§3.2) entered the newest list as `Skipped(MissedList(base − 8))`: base
2552328 on mainnet, 1569072 on testnet. Its work block predates the start, and the engine had not received that list. It
stayed so for 0.5–11.3 h (mainnet) and 0.3–7.4 h (testnet) and then became `Verified`. Platform first cited it at 17.9 h
and 15.8 h, by which time it was `Verified`. A proof citing it in that window would have come back non-`Verified`, and
rule 3's 2-minute wait would not have covered that. `dev` recovers after hours (by mechanism, #1094); the pin never
does.

### 4.3 Restarts

The morning restarts used the same storage, 20 min apart. All three (one fresh install, then two restarts) on each
network were 100 % `Verified` on first ask. The restarts served their first verified key after 12–16 s (mainnet) and
10–12 s (testnet), replayed from storage (#1073); masternode sync reported done at 25–31 s. No restart stalled.

### 4.4 Day-old tuples (post-day)

Each post-day client asked all 339 mainnet / 376 testnet tuples of the day. Ask heights were up to 571 (mainnet) and
643 (testnet) blocks below the tip, and the cited quorums' base blocks up to 1 052 blocks below it.

| Run | active quorum (in the run's newest list) | rotated-out quorum |
|---|---|---|
| fresh install, mainnet | 255/255 `Verified` | 84/84 found, `Skipped(NotMarkedForVerification)` |
| fresh install, testnet | 160/160 `Verified` | 216/216 found, `Skipped(NotMarkedForVerification)` |
| restart of the 25 h store, mainnet | 255/255 `Verified` | 84/84 `Verified` |
| restart of the 25 h store, testnet | 160/160 `Verified` | 202 `Verified`, 14 `Skipped(MissedList(1569112))` (one quorum) |

So a fresh `dev` install verifies exactly the quorums active at its tip. It also holds the older ones with the right key
(so does the pin, which reports them `Unknown`), but cannot verify them, because their work-block lists predate its
first sync. A long-running client keeps the statuses it earned while the quorum was active. That matters only for a
proof whose chain-locked height trails the tip by most of an active window, which no up-to-date evonode served (§2).
With 8 clients syncing in parallel, fresh installs (pin and `dev`) took 107–154 s to finish masternode sync, against
30–45 s alone.

### 4.5 Memory

RSS peaked during the first sync: 102 MiB on mainnet and 76 MiB on testnet. For the rest of the day it stayed at 25–44
MiB, apart from one excursion to 59–64 MiB on testnet (7.5–7.9 h). The pin's figures are in §3.4.

---

## 5. R7 at the pin, and whether `dev` closes the gap

DASHPAY §2.2 makes the provider require the strongest status dash-spv offers, unless the spike shows that the weaker
one is still anchored to the chain.

**At the pin, R7 fails.** The quorum is found and its key is right, but its status is
`Skipped(UnknownBlock(<quorum hash>))`: dash-spv skipped verifying the commitment because it did not know the quorum's
block. This spike did not show that the status is anchored to the chain, so §2.2's rule applies: require `Verified`. A
provider that requires `Verified` would refuse 38–43 % of live proofs over a day, and every one until Platform cites one
of the few verified quorums: 4.0 h and 7.6 h after the fresh installs, and only by chance at once after the post-day
mainnet restart. That is worse than the 2-minute bounded wait of rule 3 by orders of magnitude. A provider that accepts
`Skipped` adds nothing this spike could show beyond the trusted service's own key, which it already compares. So 1.0
ships the degraded mode: trusted keys, compared with SPV's (which matched in every case), rule 5 suspended.

**At `dev`, most of the status gap closes.** Every live proof's quorum was `Verified` 45 s after a fresh install and
10–16 s after a restart, on both networks, with no misses.

One exception: the first quorum after the start stayed non-`Verified` for 7–11 h. No proof happened to cite it then
(§4.2).

#1094 is the likely change, by mechanism. #1072 and #1075 bound memory and serve older heights, but a fresh install
cannot verify what they serve (§4.4), and no live proof needed it.

**`dev` alone does not make R7 pass.** Three items remain (§6 items 3, 4 and 5):

- the first quorum after the start;
- the desktop cannot read the status through platform-wallet;
- `Verified` is not yet chain-anchored (#1117).

R7 passes when the desktop's graph carries all of the following:

- `dev`'s engine (platform #5307);
- a fix for that quorum;
- a status-enforcing lookup;
- the #1117 fix.

---

## 6. What is needed (E0-10b, E0-10c)

Ordered by what blocks R7.

1. **The status gap: #1094 or equivalent.** `c5a9b944` "resolve masternode engine heights from the header storage"
   (merged 2026-10-07) is, by mechanism, the change that makes the active Platform quorums `Verified` at sync (§4); no
   run isolated it. #1072/#1075 are not needed for that: no live proof cited a quorum outside the lists a freshly
   synced client already holds (§2, §4).
2. **Restart.** #1073 (`72854066`, persist and replay the masternode messages) makes a restart serve verified keys
   in 10–16 s instead of a full QRInfo round. The pin's mainnet restarts stalled on headers ("no segment matched",
   §3.3), the symptom #1102 (`7c7b167d`) fixes; dev did not stall in any restart.
3. **The first quorum after the start.** At both revisions, the first Platform quorum whose work block (base − 8)
   predates the start is not verified when its commitment arrives. It stays `Skipped(UnknownBlock)` at the pin for its
   whole life, and `Skipped(MissedList(base − 8))` at `dev` for 7–11 h (§4.2). Under enforcement, every proof citing it
   in that window would be refused, far beyond rule 3's 2-minute wait. dash-spv should fetch the missing list (or redo
   the verification) as soon as the commitment arrives. This is E0-10c work even after the pin moves.
4. **A lookup that enforces or reports the status.** At both revisions `get_quorum_at_height` returns any entry that is
   not `Invalid` (`Unknown` and `Skipped` included), and every consumer drops the status:
   - platform-wallet's `SpvRuntime::get_quorum_public_key` returns only the 48-byte key, at the pin and at #5307's head
     (`d8b911d9`, `runtime.rs:321`);
   - dash-spv-ffi's `ffi_dash_spv_get_quorum_public_key` does the same at `dev` (`platform_integration.rs:42`).

   `SpvRuntime` keeps the client private, so `trust.rs` (E0-10b) cannot require `Verified` through the API the desktop
   links. Either dash-spv refuses non-`Verified` entries in that call (or adds a verified-only variant), or
   platform-wallet exposes the entry. The second is a platform patch, which DEC-18 allows only as a cherry-pick of a
   public upstream PR, so it needs a manager decision or an upstream request.
5. **Chain-anchored lists: #1117.** `Verified` is necessary but not sufficient: until dash-spv checks each MnListDiff
   against the coinbase merkle roots (dashpay/rust-dashcore#1117, open), a verified quorum is not anchored to the
   chain. R7 enforcement waits for that fix.
6. **Memory over long sessions: #1072/#1075.** The pin keeps a masternode list per block and never drops one (§3.4);
   `dev` prunes to its retention window and rebuilds a pruned height from storage. Neither is needed for live proofs.

**Route to the desktop.** platform #5307 (open, base v5.1-dev, head `d8b911d9`) pins rust-dashcore `e6873e95`, which
branches from `dev` at `78b660cb` and contains #1072, #1073, #1075, #1094 and #1102. So DASHPAY §3.9's v5.1 move brings
the fix, items 3 and 4 apart. A backport onto `40268cc0` (E0-10c's other route) would carry #1094's breaking changes
(the engine moves from `dashcore` into `dash-spv` as a private type; `DashSpvClient::masternode_list_engine` becomes
`pub(crate)`, and platform-wallet's `runtime.rs:507,540,584,608,621` call it), i.e. most of #5307's platform-side work.
The pin move is the cheaper route; E0-10c then reduces to items 3 and 4 and the item-5 question.

**For E0-10b.** Build `trust.rs` for the degraded mode now: trusted keys, compared with SPV's whenever SPV has the
quorum (at the pin it has it for every live proof, with a matching key, but `Skipped`). Rule 3's bounded wait (2 min) is
generous: at `dev` a fresh install served its first verified key after ≈45 s and a restart after 10–16 s, and no live
proof cited a quorum SPV did not hold once synced. The exception is the first quorum after the start (item 3). Rule 3's
premise, "every node's proof cites the same quorum", holds for up-to-date evonodes but not for stale ones (§2). A quorum
unknown at a chain-locked height far below SPV's tip means a stale node, so retrying another DAPI node is right there,
and the fallback still is not. Flip to enforcement when the graph carries items 1–5.

---

## 7. What this means for SB0 (shared base)

SB0's MN-list pipeline (SB0 §5, `dash-mnlist`) serves this lookup for mobile, desktop and rdashd `-spv`. What the spike
measured that pins down its requirements:

- **Which type.** Platform proofs cite only the network's Platform type: LLMQ_100_67 (type 4) on mainnet, LLMQ_25_67
  (type 6) on testnet (devnet: `platform_type()`'s override; regtest `LlmqtypeTestnetPlatform`; not measured here).
  Both have a DKG interval of 24 and 24 signing-active quorums, so 576-block active windows. No proof cited another
  type.
- **Which heights.** The cited height is Platform's core chain-locked height. For every up-to-date evonode it was 0–4
  blocks (median 0) behind the SPV header tip. The cited quorum was any of the 24 active ones, its base block 19–589
  blocks below the cited height. So SPV must hold and verify, for the Platform type, every quorum active at its tip:
  those whose base block is in the last ≈ 600 blocks (one 576-block window plus the ≈ 20 blocks a commitment takes to be
  mined). It also needs each quorum's work-block list (base − 8), which `Verified` needs. The `dev` engine's floor of 4
  active windows (2304 blocks; retention 2312) covers that with margin; about 96 Platform quorums are mined per 2304
  blocks.
- **Verify what a fresh client holds, not only the active set.** A fresh `dev` client verifies only the quorums active
  at its tip. It holds older ones, but leaves them `Skipped(NotMarkedForVerification)` (§4.4). For live proofs that is
  enough. If SB0 wants the retention window to serve proofs (a lagging node, a proof fetched before a restart and
  verified after), its QRInfo request must also cover the work-block lists of the rotated-out quorums it keeps.
  Otherwise the window should be documented as "held, not verified".
- **Stale evonodes cite stale quorums.** Two mainnet evonodes, frozen ≈ 20 000 blocks behind, cited a quorum with base
  block 2531760, outside any retention window. Neither revision nor the trusted service held it. The lookup must fail
  closed there (it does), and the caller should try another node rather than the fallback (§2).
- **Keyed by (type, hash), not by list.** A proof names the quorum. What the lookup needs is the commitment, its
  status, and that it was mined at or below the cited height and is not older than the floor. SB0 §5.3's
  `QuorumSet: (llmq_type, quorum_hash) → { commitment, status }` fits this directly, without the per-list walk-back
  dash-spv does today. The sparse `lists` must still include each such quorum's work-block list, so that it can become
  `Verified`.
- **Only `Verified` serves a proof.** SB0 §5.2 step 6 runs quorum selection over the authenticated, chain-anchored (#1117) set, then
  requires the selected commitment to be `Verified`. A Platform proof names its quorum, so that commitment must be
  `Verified`. `platform_integration.rs` keeps its shape (SB0 §5.5), but must refuse or report non-`Verified`, because
  every consumer measured here drops the status (§6 item 4).
- **Verify the first quorum after the start.** SB0's QRInfo/MnListDiff scheduling must fetch the work-block list of
  every active Platform quorum, including one whose work block predates the sync and whose commitment arrives after it
  (§4.2, §6 item 3).
- **Persist statuses.** A restart should serve verified keys without a QRInfo round. `dev`'s message replay gets
  10–16 s; persisting the quorum set with statuses in the SPV store (SB0 §9.5) would make it immediate.
- **Latency is small but not bounded.** At `dev`, in-memory lookups took a median 0.02 ms and a p99 of about 1 ms over
  the day. The worst cases were 88 ms (live), 323 ms (a restart's p99) and 2.1–2.9 s with 8 clients syncing in parallel;
  this spike did not trace their cause. The synchronous `ContextProvider` can read a `snapshot()` directly, but SB0's
  snapshot should not wait on a writer that holds the engine during sync.

---

## 8. Limits

- Filters were off. The desktop also syncs compact filters, which compete for peers and bandwidth at first launch,
  so its time to the first verified key will be longer than measured here.
- One vantage point (agentbox), default peer discovery, one day. A quorum rotation pattern that only shows over weeks
  (or a long Platform halt) would not appear.
- The trusted service's key is the reference. A mismatch would have meant one side was wrong; none occurred, so this
  does not decide which.
- The probe asks up to 15 s after the proof was fetched, a little later than the SDK does, which could hide a miss at
  the exact moment a new quorum is first cited.
- Devnet/dashmate types were not measured.
- One fresh install per network and revision for the 25 h runs. Whether a proof cites the first quorum after the start
  while it is unverified (§4.2) is down to timing; a second day could show it. The gap itself reproduced: the morning
  fresh `dev` install on testnet also had 1569072 `Skipped(MissedList(1569064))` at its 0.25 h tick.
- Mainnet quorum heights come from the public Insight API, because the mainnet oracle has not reached the tip.
- `verified_rate` reports dash-spv's own status field.
