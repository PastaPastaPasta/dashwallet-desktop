# Plan to 1.0 by Wednesday 2026-12-02

Written 2026-11-18 on pasta's direction: finish 1.0 of the plan within two weeks, with parallel lanes and no
work parked for capacity. This page is the dated plan. It replaces the dates in `docs/design/ROADMAP.md` §7.1
(the scope there and in `docs/research/gap-2026-10-31/RELEASE-PLAN.md` stands, less the cuts below). The product
owner updates the State column as items land and reports against it daily.

**Cut on 2026-11-19** (the project manager's decision): the cut order in "Risks" is applied now, so that the
1.0 core is in the release candidate on 11-30. What was cut and what stays is in "Cuts applied 2026-11-19".

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
| 0.2 | Thu 11-19 | what is on `main` when it is green (it waits on #40); instant incoming payments and the fast launch move to 0.3 |
| 0.3 | Mon 11-23 | instant incoming payments, the fast launch, masternode screens (MG-10), Platform ↔ Shielded, CoinJoin fixes, legacy `wallet.dat`, empty states, unlock once |
| 0.4 | Thu 11-26 | the rest of the core: shared masternodes (MG-10b), what lane C keeps, CoinJoin V24, PSBT under Advanced |
| 0.5 | — | not released: its contents (commerce, BIP70) are cut to 1.0.x |
| 0.6 | Sun 11-29 | hardening: fuzzing, kill -9 matrix, performance, trust gate, auto-update |
| 0.9 (RC) | Mon 11-30 | security review fixes in; QA pass on three OSes |
| 1.0 | Wed 12-02 | RC fixes; release checklist; signed if the certificates exist |

## Lanes

Each lane is one agent working its items in order, one pull request per item. "PO" is the product-owner
thread; "MG" is the masternode and governance thread, which is also the release owner.

| Lane | Owner | Items, in order | Days | State |
|---|---|---|---|---|
| A Payments | PO | **P1** instant incoming payments: engine, then Home and Activity while syncing | 11-18 → 11-21 | engine in review (#42, two blockers being fixed); the screen follows |
| B Launch | PO | **P5** window in two seconds: shell, then last-known balance at once | 11-18 → 11-21 | shell in review (#33, fixes in progress) |
| C Shielded and Platform | MG | finish item 0 (BL-617) → Platform ↔ Shielded → Max drains → top-up from Shielded or Platform → funding-source picker with shielded-funded identity → pay a contact from any balance → withdrawal to own Platform address → protocol advisory banner → vault scopes and viewing-key cache | 11-18 → 11-27 | item 0 part A3b merged (#26) |
| D Masternode screens | MG | MG-10 app bindings → My masternodes → registration wizard → ProUpRegTx → MG-10b shared masternodes → shown by default when the wallet holds keys → voting-key import → contested-vote polish | 11-19 → 11-25 | engine done |
| E Masternode checks | MG | MG-08 `dwcli` owner commands and regtest suites → MG-12 signing-path review and BL-582..585 | 11-19 → 11-26 | not started |
| F CoinJoin | PO | BL-614 → BL-615 → V24 promotion and demotion → mixed-send warnings. Cut: sleep prevention, start on login | 11-19 → 11-25 | BL-614 (#38) and BL-615 (#39) in review, fixes in progress |
| G Core compatibility | PO | legacy Berkeley DB `wallet.dat`. Cut: MP-07 sweep and BIP38, MP-06 phrase repair, MP-05 BIP70, spend unconfirmed change, per-transaction notifications | 11-19 → 11-23 | `wallet.dat` in review (#34, fixes in progress) |
| H Marketplace | PO | Cut: X4 (browse, buy, extra names, instant-name offer), identity-verify link | — | cut to 1.0.x |
| I Invitations | PO | Cut: the storage patch, X3 create, share and history, shielded-funded and small invites, the Home claim card. Claiming an invitation shipped in 0.1 and stays | — | cut to 1.0.x |
| J DashConnect | PO | Cut: App Connect v1 and v2 | — | cut to 1.0.x |
| K Everyday polish | PO | #25 empty states → P7 unlock once → P15 PSBT under Advanced. Cut: MP-04 tax categories, lock-screen receive, log export and backup reminder, transaction notes on Platform, MP-14 localization (English only) | 11-18 → 11-24 | #25 and unlock once (#37) in review, fixes in progress; P15 lane running |
| L Commerce | PO | Cut: MP-09 Explore, MP-13 CrowdNode withdraw, MP-10, MP-11, MP-12 | — | cut to 1.0.x |
| M Hardening | PO | H-02 fuzzing → H-03 kill -9 matrix → H-06 performance → H-09 trust gate on by default → **H-01 security review (11-27 → 11-29)**. Cut: H-10 node pinning, Tor/SOCKS5, MP-03, MP-15 | 11-19 → 11-29 | H-02 lane running |
| N Release and packaging | MG | cut 0.2 → `parity.md` redone for the Tauri app → PK-03 auto-update → W-02 Windows services → Flatpak → SBOM and checklist → retire Swift (U-08) → each release cut | 11-19 → 12-02 | not started |
| O Test suites and QA | PO | DP2-06 (never-cut), DP1-07, DP3-05 suites → visual baselines refresh, issue #29 → iOS/Android restore test → H-04 real-node flows → H-08 full QA pass on three OSes → H-05 accessibility → H-07 mainnet canary | 11-19 → 12-01 | suites lane running |

## Cuts applied 2026-11-19

The schedule slipped a day in its first two days (review and CI turnaround, and a day without agent capacity),
so the cut order below under "Risks" is applied in full now rather than lane by lane later. Everything cut moves
to a 1.0.x release unless a row says otherwise; nothing on the never-cut list is touched.

| Cut | From | Note |
|---|---|---|
| H-10 healthy-node pinning | lane M | ROADMAP §6, cut 1 |
| MP-15 and MP-03 | lane M | ROADMAP §6, cut 2 |
| Windows Hello (passphrase only on Windows) | lane N, W-02 | ROADMAP §6, cut 5; the rest of W-02 is the release owner's to keep or cut |
| The tray companion on Windows and Linux | U-01 | ROADMAP §6, cut 6; only what is not on `main` yet |
| MP-14 localization | lane K | ROADMAP §6, cut 8: 1.0 is English only |
| DashConnect, App Connect v1 and v2 | lane J | the whole lane |
| Explore, CrowdNode withdraw, DashSpend, Buy & Sell, ZenLedger (MP-09…MP-13) | lane L | the whole lane; the partner credentials are no longer needed for 1.0 |
| Username marketplace (X4) and the identity-verify link | lane H | the whole lane |
| Creating invitations (X3) and its storage patch | lane I | the whole lane; claiming an invitation stays |
| MP-04 tax categories, MP-06 phrase repair, MP-07 sweep and BIP38, MP-05 BIP70 | lanes G, K | the 0.4 and 0.5 parity items |
| Spend unconfirmed change, per-transaction notifications, lock-screen receive, log export, backup reminder, transaction notes on Platform | lanes G, K | small parity items after the cut ones |
| CoinJoin sleep prevention and start on login | lane F | |
| Tor/SOCKS5 | lane M | |

ROADMAP §6's cuts 3, 4 and 7 (X1a, the Identities page, fiat) do not apply: those shipped.

**The 1.0 core, due in the release candidate on 11-30:**

- everything released in 0.1 and on `main` today (the DashPay identity, username, contacts and payments flows,
  send and receive, history, CoinJoin, backup and restore, security, the tools and settings);
- instant incoming payments (lane A) and the two-second launch (lane B);
- the wallet-holder masternode and governance screens (lanes D and E), with shared masternodes;
- Platform ↔ Shielded transfers and the item 0 fixes (lane C; the release owner decides which of its later items stay);
- the CoinJoin fixes BL-614 and BL-615, V24 promotion and demotion, mixed-send warnings (lane F);
- opening a legacy Berkeley DB `wallet.dat` (lane G);
- empty states, unlock once, PSBT under Advanced (lane K);
- hardening: fuzzing, the kill -9 matrix, performance, the trust gate, the security review (lane M);
- the DashPay suites, the QA pass on three OSes, accessibility, the canary (lane O);
- releases and packaging: auto-update, Flatpak, SBOM and checklist (lane N).

`docs/parity.md` still names 0.4 and 0.5 for the cut rows; its Release column is brought in line in lane N.

## What only pasta can unblock

Without these the release still ships on 12-02, in the reduced form the release plan already allows.

| Needed | For | By | If it does not arrive |
|---|---|---|---|
| Apple Developer ID and notarization, Windows signing certificate | signed installers, auto-update | 11-27 | 1.0 ships unsigned and says so; auto-update ships off |
| A release key, and the choice of PGP or Sigstore | signed checksums | 11-27 | build attestations only, as today |
| ~~Partner credentials (CTX, PiggyCards, Topper, Uphold, Coinbase, SwapKit, ZenLedger)~~ | DashSpend, Buy & Sell, ZenLedger | — | no longer needed for 1.0: lane L is cut |
| Up to 0.5 DASH on mainnet and an iOS user for an hour | mainnet canary (H-07) | 11-30 | canary runs on testnet only |
| A funded testnet wallet created on iOS and one on Android | same-seed restore test | 11-24 | checked by code comparison only |
| A real-Mac session and a real Windows PC | Touch ID, Windows Hello, VoiceOver, Narrator, first notarized launch | 11-30 | passphrase unlock only; screen-reader checks on Linux only |
| ~~Transifex access; Explore database access~~ | localization pull; merchant data | — | no longer needed for 1.0: localization and Explore are cut |
| Go-ahead for the upstream platform pull request (E0-04.P3/P4), or agreement to drop it | Mode A dispatch | 11-21 | dropped as superseded by item 0's hook |

## Risks

- **The date is tight.** The earlier plan put 1.0 in late January. If a lane slips, the cut order of ROADMAP §6
  applies, then lanes J, L and the 0.4 parity items move to a 1.0.x release. The never-cut list stands.
  This was applied on 2026-11-19 (above). The next cuts, if the core slips again: PSBT under Advanced, mixed-send
  warnings, CoinJoin V24, then the later items of lane C.
- **Review and CI throughput.** A full CI run takes about 90 minutes on macOS. With 15 lanes the runners
  queue; engine-only and renderer-only pull requests should not wait on each other's jobs.
- **Security review on new code.** H-01 runs 11-27 → 11-29 over code written the same fortnight. Its
  findings land before the release candidate or the affected feature ships disabled.
