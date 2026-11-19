# Plan to 1.0 by Wednesday 2026-12-02

Written 2026-11-18 on pasta's direction: finish 1.0 of the plan within two weeks, with parallel lanes and no
work parked for capacity. This page is the dated plan. It replaces the dates in `docs/design/ROADMAP.md` §7.1
(the scope there and in `docs/research/gap-2026-10-31/RELEASE-PLAN.md` stands). The product owner updates the
State column as items land and reports against it daily.

## Where we are

- Released: `v0.1.0` (2026-11-03) only. `main` is about 560 commits past it; 0.2 was never cut.
- Remaining, checked against the code on 2026-11-18: about 200 agent-days without the partner integrations
  (about 225 with them). In 14 days that needs 14–16 lanes running the whole time, so review and CI
  turnaround, not writing code, sets the pace.
- Rules that make this possible: every pull request is small and merges the moment its review and required
  checks are green; unrelated pull requests never wait for each other; a red `main` gets a fix within the hour;
  a flaky end-to-end run is retried, tracked (issue #29) and never blocks a lane.

## Release dates

| Version | Date | Contents |
|---|---|---|
| 0.2 | Thu 11-19 | what is on `main` now, plus instant incoming payments and the fast launch if they have landed |
| 0.3 | Mon 11-23 | masternode screens (MG-10, MG-10b), Platform ↔ Shielded, CoinJoin fixes, legacy `wallet.dat` |
| 0.4 | Thu 11-26 | marketplace, invitation creation, DashConnect, remaining mobile parity |
| 0.5 | Sat 11-28 | Explore, CrowdNode withdraw, BIP70; partner integrations if credentials exist, disabled otherwise |
| 0.6 | Sun 11-29 | hardening: fuzzing, kill -9 matrix, performance, trust gate, auto-update |
| 0.9 (RC) | Mon 11-30 | security review fixes in; QA pass on three OSes |
| 1.0 | Wed 12-02 | RC fixes; release checklist; signed if the certificates exist |

## Lanes

Each lane is one agent working its items in order, one pull request per item. "PO" is the product-owner
thread; "MG" is the masternode and governance thread, which is also the release owner.

| Lane | Owner | Items, in order | Days | State |
|---|---|---|---|---|
| A Payments | PO | **P1** instant incoming payments: engine, then Home and Activity while syncing | 11-18 → 11-20 | engine lane running |
| B Launch | PO | **P5** window in two seconds: shell, then last-known balance at once | 11-18 → 11-20 | shell lane running |
| C Shielded and Platform | MG | finish item 0 (BL-617) → Platform ↔ Shielded → Max drains → top-up from Shielded or Platform → funding-source picker with shielded-funded identity → pay a contact from any balance → withdrawal to own Platform address → protocol advisory banner → vault scopes and viewing-key cache | 11-18 → 11-27 | item 0 part A3b merged (#26) |
| D Masternode screens | MG | MG-10 app bindings → My masternodes → registration wizard → ProUpRegTx → MG-10b shared masternodes → shown by default when the wallet holds keys → voting-key import → contested-vote polish | 11-19 → 11-25 | engine done |
| E Masternode checks | MG | MG-08 `dwcli` owner commands and regtest suites → MG-12 signing-path review and BL-582..585 | 11-19 → 11-26 | not started |
| F CoinJoin | PO | BL-614 → BL-615 → V24 promotion and demotion → sleep prevention, start on login, mixed-send warnings | 11-19 → 11-25 | not started |
| G Core compatibility | PO | legacy Berkeley DB `wallet.dat` → MP-07 sweep and BIP38 → MP-06 phrase repair → MP-05 BIP70 → spend unconfirmed change, per-transaction notifications | 11-19 → 11-26 | not started |
| H Marketplace | PO | X4: browse, buy, extra names, instant-name offer → identity-verify link | 11-20 → 11-26 | not started |
| I Invitations | PO | storage patch → X3 create, share, history → shielded-funded and small invites → Home claim card | 11-20 → 11-26 | not started |
| J DashConnect | PO | App Connect v1 → v2 | 11-20 → 11-27 | not started |
| K Everyday polish | PO | #25 empty states → P7 unlock once → P15 PSBT under Advanced → MP-04 tax categories → lock-screen receive → log export and backup reminder → transaction notes on Platform → MP-14 localization | 11-18 → 11-27 | #25 in review |
| L Commerce | PO | MP-09 Explore → MP-13 CrowdNode withdraw → MP-10, MP-11, MP-12 (need partner credentials) | 11-22 → 11-28 | not started |
| M Hardening | PO | H-02 fuzzing → H-03 kill -9 matrix → H-06 performance → H-09 trust gate on by default → H-10 node pinning → Tor/SOCKS5 → MP-03, MP-15 → **H-01 security review (11-27 → 11-29)** | 11-21 → 11-29 | not started |
| N Release and packaging | MG | cut 0.2 → `parity.md` redone for the Tauri app → PK-03 auto-update → W-02 Windows services → Flatpak → SBOM and checklist → retire Swift (U-08) → each release cut | 11-19 → 12-02 | not started |
| O Test suites and QA | PO | DP2-06 (never-cut), DP1-07, DP3-05 suites → visual baselines refresh, issue #29 → iOS/Android restore test → H-04 real-node flows → H-08 full QA pass on three OSes → H-05 accessibility → H-07 mainnet canary | 11-20 → 12-01 | not started |

## What only pasta can unblock

Without these the release still ships on 12-02, in the reduced form the release plan already allows.

| Needed | For | By | If it does not arrive |
|---|---|---|---|
| Apple Developer ID and notarization, Windows signing certificate | signed installers, auto-update | 11-27 | 1.0 ships unsigned and says so; auto-update ships off |
| A release key, and the choice of PGP or Sigstore | signed checksums | 11-27 | build attestations only, as today |
| Partner credentials (CTX, PiggyCards, Topper, Uphold, Coinbase, SwapKit, ZenLedger) | DashSpend, Buy & Sell, ZenLedger | 11-22 | those screens ship disabled |
| Up to 0.5 DASH on mainnet and an iOS user for an hour | mainnet canary (H-07) | 11-30 | canary runs on testnet only |
| A funded testnet wallet created on iOS and one on Android | same-seed restore test | 11-24 | checked by code comparison only |
| A real-Mac session and a real Windows PC | Touch ID, Windows Hello, VoiceOver, Narrator, first notarized launch | 11-30 | passphrase unlock only; screen-reader checks on Linux only |
| Transifex access; Explore database access | localization pull; merchant data | 11-23 | English only; Explore ships without live data |
| Go-ahead for the upstream platform pull request (E0-04.P3/P4), or agreement to drop it | Mode A dispatch | 11-21 | dropped as superseded by item 0's hook |

## Risks

- **The date is tight.** The earlier plan put 1.0 in late January. If a lane slips, the cut order of ROADMAP §6
  applies, then lanes J, L and the 0.4 parity items move to a 1.0.x release. The never-cut list stands.
- **Review and CI throughput.** A full CI run takes about 90 minutes on macOS. With 15 lanes the runners
  queue; engine-only and renderer-only pull requests should not wait on each other's jobs.
- **Security review on new code.** H-01 runs 11-27 → 11-29 over code written the same fortnight. Its
  findings land before the release candidate or the affected feature ships disabled.
