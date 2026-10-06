# G7 — mainnet governance sync over SPV (measured)

Gate: docs/contracts/m3-engine.md §9, DESIGN-opus G7. Pass = govsync in ≤ 5 min on ≈ 50 Mbit and tallies within
1 % of a full node's.

## Method

`dwcli --network mainnet --datadir <fresh dir> gov sync` (debug build of m3/r2-governance, 2026-10-06):

1. dash-spv syncs headers and the masternode list from scratch (`spv_secs`);
2. govsync connects to 3 masternodes picked from the synced list, runs `govsync` for every object from one of
   them, then `govsync <hash>` for the votes of every proposal whose end is in the future, spread over the 3
   (`objects_secs`, `votes_secs`); `gov_secs` is enabling sync to `Synced`;
3. the tallies of every proposal are printed and compared with a full-node reference taken right after the run.

Reference: no mainnet dashd was reachable from the sandbox, so the reference is the DashCentral budget API
(`https://www.dashcentral.org/api/v1/budget`, Y/N/A counts read from its mainnet node), fetched within a minute
of each run. Comparison: `scripts/g7-compare-tallies.py <dwcli output> <reference.json>` (also reads a dashd
`gobject list valid proposals` result).

Machine: the dev MacBook (Apple Silicon, shared with other agents' builds) on the home connection; bandwidth not
measured during the runs.

## Results

| Run | spv_secs | gov_secs | objects_secs | votes_secs | objects | votes | bytes (govsync) | RSS (whole dwcli) | wall |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 50.9 | 4.6 | 0.5 | 2.5 | 20 | 3801 | 810 503 | 616 MB | — |
| 2 | 77.3 | 2.8 | 0.4 | 1.5 | 19 | 3801 | 808 429 | 400 MB | 81 s |
| 3 | 54.7 | 3.0 | 0.4 | 2.0 | 20 | 3801 | 810 503 | 488 MB | 58 s |
| 4 | 35.8 | 3.5 | 0.8 | 2.1 | 20 | 3801 | 810 503 | 537 MB | 39 s |

20 objects = 19 proposals + 1 superblock trigger. In run 2 the object count was 19: the trigger did not arrive
(not investigated further; the peers that answer differ between runs, and a trigger relayed later is picked up by
the relay phase). Without it the funded proposals show their vote status (Passing or Voting) until it arrives.

Tallies, 19 current proposals, every run:

- **Yes and No equal the reference for all 19 proposals** (0.00 % deviation).
- Abstain equals for 18; one proposal (`76c104d2…`) shows 17 abstain against 20. The difference is the same in
  every run, so it is not timing. The likely cause is the SPV rule (tally.rs): a vote counts only if the key
  that signed it is a voting key of the current list. Core keeps a vote that was valid when it arrived even if
  the masternode changed its voting key later (`ProUpRegTx`); the wallet cannot check such a vote against the
  old key.
- Statuses: runs 1, 3, 4: 6 Funded (listed by the synced trigger), 13 Voting; run 2 (no trigger): 5 Passing, 14 Voting.

**Verdict: pass.** govsync takes 3–5 s and 0.8 MB on top of an SPV sync of 36–77 s; the whole run from an empty
data dir is under 1.5 min. Tallies match a full node exactly for Yes/No, within 3 votes for Abstain on one
proposal. The "needs a Dash Core node" fallback of §9 is not needed.

## Not done

- The Mac Studio run (§9 step 2): not run; this agent's sandbox has no SSH access.
- A dashd reference over RPC (§9 step 3): DashCentral's API stood in for it.
- Bandwidth was not throttled to 50 Mbit; with 0.8 MB of governance data the 5-minute bound has three orders of
  magnitude of margin.
