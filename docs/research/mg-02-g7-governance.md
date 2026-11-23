# MG-02 G7: governance sync on mainnet against Core's counts

2026-10-16. The wallet's governance sync (`dwcli governance`, dw-engine's governance facade over dw-p2p) against
mainnet, compared with a mainnet Core node's vote counts. Design: `docs/design/MG-02-governance.md` §8.

## Method

- **Reference.** DashCentral's budget API (`https://www.dashcentral.org/api/v1/budget`), which reads a mainnet Core
  node (D14). The program's own reference node is still in initial block download, and the brief forbids touching
  its chain directory.
- **Read-only, concurrency 1.** One `dwcli` process at a time. It ran dash-spv for headers and the masternode list,
  then one govsync from 3 masternodes picked at random from the list. The reference was fetched once, right after
  `dwcli` exited.
- **Pass.** Every proposal's yes and no estimates equal the reference, and the reference lies inside the wallet's
  range.

## Runs

| Run | Data dir | Start (UTC) | Synced | Objects | Votes | Governance bytes | Objects phase | Votes phase | Peers |
|---|---|---|---|---|---|---|---|---|---|
| cold | empty | 15:05:47 | 15:10:45 | 26 (1 unconfirmed) | 4,032 | 863,632 | 0.41 s | 4.40 s | 3 |
| measured | warm (headers and list kept) | 15:49:04 | 15:49:38 | 27 | 4,039 | 865,163 | 0.25 s | 2.77 s | 188.40.184.72, 45.76.205.140, 207.148.68.109 (port 9999) |

The reference was fetched at 15:49:40 (HTTP 200, 21,416 bytes), 2 s after the measured run synced. The cold run's
five minutes are dash-spv's header and list sync; governance itself took under 5 s in both runs.

## Result

**PASS.**
- All 26 current proposals appear on both sides.
- The yes and no estimates equal the reference for all 26.
- The reference's yes, no and abstain all lie inside the wallet's ranges for all 26.
- 4 votes are ranged.

The "Status" column is the wallet's (`ProposalStatus`). The ranges are the wallet's `WeightRange`, shown only where
min differs from max.

| Proposal | Hash | Yes | No | Abstain | Core yes | Core no | Core abstain | Status |
|---|---|---|---|---|---|---|---|---|
| DCG-Operations-Aug-Nov-26 | `91808ffd` | 748 | 44 | 0 | 748 | 44 | 0 | funded |
| mnowatch-hosting-2026 | `594f3220` | 664 | 8 | 4 | 664 | 8 | 4 | funded |
| Dash-Italia-Management-2026 | `ce46b7dc` | 660 | 36 | 0 | 660 | 36 | 0 | funded |
| dash-growth-marketing-budget | `64be8974` | 660 | 248 | 0 | 660 | 248 | 0 | funded |
| vulnerability-disclosure-compensation | `10b85795` | 519 | 121 | 20 | 519 | 121 | 20 | funded |
| dash-growth-q4-2026 | `d784b13a` | 709 | 438 | 5 | 709 | 438 | 5 | funded |
| Dash-Pay-name-registration-rules | `a5465cdb` | 330 | 8 | 38 | 330 | 8 | 38 | voting |
| Dash-APAC-November | `ce5c6a70` | 278 | 8 | 0 | 278 | 8 | 0 | voting |
| c2pool-daemonless-finalize | `fa758340` | 317 | 121 | 5 | 317 | 121 | 5 | voting |
| DASH-MULTILINGUAL-EXPANSION_Q4 | `a4cf6159` | 285 | 94 | 31 | 285 | 94 | 31 | voting |
| Dash-communities-without-censorship | `15f8fb4a` | 398 | 225 | 14 | 398 | 225 | 14 | voting |
| dashcentral-operations4 | `42e96906` | 175 | 10 | 0 | 175 | 10 | 0 | voting |
| OrchardPay | `84106141` | 182 | 22 | 9 | 182 | 22 | 9 | voting |
| BTCBACKPORTSVIJAY_04 | `a3b0b14b` | 268 | 130 | 0 | 268 | 130 | 0 | voting |
| BTCBackportsVijay_03 | `8576a3db` | 370 | 260 | 18 | 370 | 260 | 18 | voting |
| pshenmic-dev-dash-desktop-november-2026 | `b55fdc68` | 120 | 28 | 0 | 120 | 28 | 0 | voting |
| APAC-BizDev-HK-China-SG-Nov-Dec-26 | `74d0ba8f` | 51 | 8 | 0 | 51 | 8 | 0 | voting |
| dash-intents | `1f0c5006` | 54 | 12 | 0 | 54 | 12 | 0 | voting |
| Business-Development-Events-November-26 | `72acbacc` | 35 | 0 | 0 | 35 | 0 | 0 | voting |
| kwvg-core-oct26 | `e7d9d690` | 48 | 16 | 0 | 48 | 16 | 0 | voting |
| Dash-Mass-Organic-Content-Attraction | `54faf096` | 16 | 0 | 12 | 16 | 0 | 12 | voting |
| pshenmic-dev-dfo-november-2026 | `165b2cda` | 19 | 28 | 0 | 19 | 28 | 0 | voting |
| DashDaoGrowthEngine | `0f44c301` | 139 | 156 | 25 | 139 | 156 | 25 | voting |
| encointerUBI-mean26 | `76c104d2` | 39 | 234 | 17 [17–29] | 39 | 234 | 20 | voting |
| GEG-PPP-30M-Film-On-Chain | `e1cd4d7f` | 27 | 340 | 24 | 27 | 340 | 24 | voting |
| 2026-05-MakeDashASafeStoreOfValue | `5966468c` | 31 | 389 | 39 | 31 | 389 | 39 | voting |

## The ranged votes

`encointerUBI-mean26` has 4 abstain votes whose voting keys belong to **revoked** masternodes (D34):
- Core's ProUpRevTx resets a masternode's state to the legacy version (`ResetOperatorFields`).
- The legacy SML entry serializes no `nType`.
- So an SPV client cannot tell a revoked evonode from a revoked regular masternode. Core still weighs a revoked
  evonode's vote at 4.

The wallet therefore gives these votes a weight between 1 and 4, estimated at 1. Core's abstain count of 20 is the
wallet's 17 plus one revoked evonode counted at 4 rather than 1, which is inside [17, 29].

Mainnet's list has 354 revoked entries, and these are the only votes among them. An earlier run, made before D34,
counted these votes at exactly 1. Its abstain count (17 against Core's 20) is how the gap was found in the data.
The testnet vector (`testdata/governance_votes_testnet.json`) has 7 such votes out of 11, and dashd weighs each at
4.

## Reproduce

```sh
dwcli --datadir <empty dir> --network mainnet --no-platform --passphrase-file <file> \
    governance --timeout-secs 3000 --min-proposals 1      # DWCLI_LOG=dw_governance=debug names the peers
curl -s https://www.dashcentral.org/api/v1/budget          # once, right after
```
