# Plan to 1.0 by Wednesday 2026-12-02

Written 2026-11-18 on pasta's direction: finish 1.0 of the plan within two weeks, with parallel lanes and no
work parked for capacity. This page is the dated plan. It replaces the dates in `docs/design/ROADMAP.md` §7.1
(the scope there and in `docs/research/gap-2026-10-31/RELEASE-PLAN.md` stands, less the cuts below). The product
owner updates the State column as items land and reports against it daily.

**Cut on 2026-11-19** (the project manager's decision): the cut order in "Risks" is applied now, so that the
1.0 core is in the release candidate on 11-30. What was cut and what stays is in "Cuts applied 2026-11-19".

## Where we are

- Released: `v0.1.0` (2026-11-03) and `v0.2.0` (published 2026-11-20 00:27 UTC, a day after its date). 0.2 is
  `main` as it was at the release pull request (#72, 11-19) plus four changes made on the `release/0.2` branch
  (#92, #100, #104 and the notes, #107), all merged back into `main`. Everything else that merged to `main` after #72 ships in 0.3.
- State on 2026-11-21, 03:15 UTC: 15 issues are open on the "1.0 RC must-fix" milestone and 20 are closed.
  One more was filed at 03:51 UTC (#202, below).
  - Merged on 11-20: #84, #85 and #87 (#123), #108 (#115), #125 (#124), six of #81's `wallet.dat`
    follow-ups (#119), the interim lock for #105 (#132; the SQLite bump follows with the Platform re-pin due
    by 11-25), the balance on Home while the wallet syncs (#140; Activity already showed the payment), the
    localization foundation (#146), the upgrade-path check from v0.2.0, which closes part of #101 (#152),
    #141 (#158), #122 (#147), #113 (#154), the H-02 fuzz targets with their nightly job (#149), the three
    DashPay suites (#129), capture protection on Import keys (#164), the engine part of
    withdrawing an identity's credits to the user's own Platform address (#169, lane C), #78 (#160),
    #88 (#134) and the quote and check of a username registration paid from the Shielded balance (#180,
    lane C). #134 merged with a load that cannot read all of its evidence restoring no claim; what it
    leaves is in #170.
  - `main` went red on 11-20 at 21:40 UTC, from #160's changes, first seen in the run at #184, in jobs
    that pull requests skip. #160 changed two
    doc comments on exported vault methods without regenerating the Swift bindings, and its new
    power-loss test replaces `app.sqlite` while a handle on it is open, which Windows refuses. #189
    regenerated the comment text but missed two checksum lines, because the bindings check prints only
    the first 40 lines of its diff; #193 set them, and #191 skipped the test on Windows. Product merges
    were held until `main`'s Swift and Windows jobs were green again, at 01:14 UTC on 11-21. The run of
    `main` at #193 finished green in every job.
  - Merged on 11-21 by 04:10 UTC: the two checksum lines (#193, at 00:12 UTC), item 6 of #173 (#185), the fix for case 18 of #170 (#195, the release
    owner's masternode lane), #148 (#187), the nightly-failure issue (#181), a refused proposal reading
    as a refusal (#197, two items of #188), **the pin of the rust-dashcore and platform forks for #45,
    #46, #47 and #120 (#159), which is therefore in 0.3**, item 1 of #173 (#198), the renderer part
    of #133 (#137), and the Swift bindings check on pull requests (#199, item 7 of #186;
    it added about nine minutes to "Rust tests (Linux)" on its first run). From #159 on, the wallet's data is one-way for this release: the first start writes
    two storage migrations, after a copy to `backups/auto`. #159 merged with three known limits, ruled
    by the release owner and listed in #177 (items 9, 10 and 11); the fixes for 9 and 10 are being
    written in the rust-dashcore fork and re-pin in their own pull request, aimed at 11-22 18:00 UTC.
    The first run of `main` that holds the last four of these merges had not finished at the time of
    writing.
  - `main` was red again on 11-21 from 03:17 to 04:12 UTC, in the macOS job of the run at #181: one
    `dwcli` test pins a status line that depends on timing when SPV runs. The status read met a
    writer on the wallet manager and gave its other correct answer (#202). The abandonment checks of
    the test's one-shot run passed before that assertion; its session half did not run. The next macOS
    job to run, at #137, passed. The fix is to that test only (#203, in a pull request). Merges
    are not held for it: #199 merged at 04:07 UTC.
  - Merged at 04:46 UTC: #190 (#192). The holder is found and fixed (the session hub's sweep-news hook
    kept the app database), the Windows skip is removed, and the test passes on Windows in a run of
    the merge head. What it leaves is in #200, for 1.0.x.
  - In a pull request:
    - #68, #69 and the engine part of #74 (#121). The review of 11-21 found eight more cases; the
      release owner ruled seven into a third fix round, which is written and in its gates. One of them
      (Repair refusing a stuck funding whose locks hold coins of both kinds, with no way out for the
      user) gets a fallback to the previous behaviour. It merges after a re-check of that round.
    - #86 (#194), in a fix round after its review (three findings and the review bot's ten).
    - Case 14 of #170 (#183): its fix round is pushed. It merges after 0.3 is cut, see the table below.
    - The auto-updater, PK-03 (#196, the release owner's lane). It is off until the first signed release.
  - Written, no pull request yet: the `dwcli` part of #73 (the rest of #73 is rust-dashcore changes,
    which follow the fork lane's fixes for #177).
  - Not started: the rest of #101, what is left of #81 apart from the watch-only rewind and the retry of
    a stored range that fails at load, which are in #121, and the rest of the cases the funds fixes leave, filed
    on 11-20 as #170 (after #134), #172 (after #121), #173 (after #160) and #177 (after #159). Their plan is
    in "What the funds fixes leave for the release candidate". Also filed: what is open on the
    lock screen after #137 (#174, for 1.0.x), the traffic of an idle wallet's shielded sync (#178, for
    the release candidate, in lane C), a stop test that fails now and then (#182), what the nightly
    reporter leaves (#186), what the #148 fix leaves (#188, for 1.0.x), and the short holders of the app
    database after a close (#200, for 1.0.x).
- Remaining, checked against the code on 2026-11-18: about 200 agent-days without the partner integrations
  (about 225 with them). In 14 days that needs 14–16 lanes running the whole time, so review and CI
  turnaround, not writing code, sets the pace.
- Rules that make this possible: every pull request is small and merges the moment its review and required
  checks are green; unrelated pull requests never wait for each other; a red `main` gets a fix within the hour;
  a flaky end-to-end run is retried, tracked (issue #29) and never blocks a lane. A red nightly is to keep
  one issue open, labelled `nightly-failure`, until the next green one (#181).

## Release dates

| Version | Date | Contents |
|---|---|---|
| 0.2 | Thu 11-19; published Fri 11-20 | `main` at #72 plus the `release/0.2` fixes; instant incoming payments and the fast launch move to 0.3 |
| 0.3 | Mon 11-23; published Sun 11-22 | instant incoming payments, the fast launch, masternode screens (MG-10), Platform ↔ Shielded, CoinJoin fixes, legacy `wallet.dat`, empty states, unlock once |
| 0.4 | Thu 11-26 | the rest of the core: shared masternodes (MG-10b), the lane C items that have merged by then, CoinJoin V24, PSBT under Advanced, the screens whose copy is in the message catalog by then (English) |
| 0.5 | — | not released: its contents (commerce, BIP70) are cut to 1.0.x |
| 0.6 | Sun 11-29 | hardening: fuzzing, kill -9 matrix, performance, trust gate, auto-update; the lane C items kept and merged after 0.4; the last screens' copy in the catalog; the translations |
| 0.9 (RC) | Mon 11-30 | security review fixes in; QA pass on three OSes |
| 1.0 | Wed 12-02 | RC fixes; release checklist; signed locally by pasta (#780) |

## Lanes

Each lane is one agent working its items in order, one pull request per item. "PO" is the product-owner
thread; "MG" is the masternode and governance thread, which is also the release owner.

| Lane | Owner | Items, in order | Days | State |
|---|---|---|---|---|
| A Payments | PO | **P1** instant incoming payments: engine, then Home and Activity while syncing | 11-18 → 11-21 | engine merged (#42, 11-19); the balance on Home while syncing merged (#140, 11-20; Activity already showed the payment); announcing a payment that arrives during sync is in a pull request (#158, for #141) |
| B Launch | PO | **P5** window in two seconds: the shell. Cut on 11-20: last-known balance at once | 11-18 → 11-21 | the blank-launch fix merged (#33, 11-19): the window no longer waits about 30 s on a desktop whose portal does not answer. The two-second target is not shown yet: #33 measured the splash at 0.9–1.4 s on a warm launch at lower load, and within 2 s in one of five launches on a heavily loaded machine; it is measured on each OS in the QA pass (H-08). Last-known balance at once was looked at on 11-20 and not built: the whole wait is under the start-up splash, before the app knows whether it is locked, so it would show a balance before the lock is known, against #133. Moved to 1.0.x on 11-20 (the project manager's decision) |
| C Shielded and Platform | MG | finish item 0 (BL-617) → Platform ↔ Shielded → Max drains → top-up from Shielded or Platform → funding-source picker with shielded-funded identity → pay a contact from any balance → withdrawal to own Platform address → protocol advisory banner → vault scopes and viewing-key cache → idle shielded-sync traffic (#178, RC) | 11-18 → 11-27 | item 0 part A3b (#26), Max on Send from Shielded (#63) and the Platform → Shielded quote (#110, 11-20) merged; the engine part of withdrawal to own Platform address merged (#169, 11-20); #49 in review |
| D Masternode screens | MG | MG-10 app bindings → My masternodes → registration wizard → ProUpRegTx → MG-10b shared masternodes → shown by default when the wallet holds keys → voting-key import → contested-vote polish | 11-19 → 11-25 | engine done; the first app bindings (#51) and the shared abandon spends (#94) merged; the api wrappers and fixture world in review (#55) |
| E Masternode checks | MG | MG-08 `dwcli` owner commands and regtest suites → MG-12 signing-path review and BL-582..585 | 11-19 → 11-26 | the owner gov commands and nightly regtest suites merged (#35); of MG-12, voting keys refusing plain message signing merged (#44) |
| F CoinJoin | PO | BL-614 → BL-615 → V24 promotion and demotion → mixed-send warnings. Cut: sleep prevention, start on login | 11-19 → 11-25 | BL-614 (#38) and BL-615 (#39) merged (11-19); their follow-up fixes (#68, #69 and the engine part of #74) are in #121: its review of 11-21 found eight more cases, seven of them are in a third fix round, which is written and in its gates, it merges when its checks are green and that round's check is closed, and what it leaves is listed in #172; V24 promotion and demotion is on a branch, no pull request yet; the mixed-send warnings are not started (they follow #137) |
| G Core compatibility | PO | legacy Berkeley DB `wallet.dat`. Cut: MP-07 sweep and BIP38, MP-06 phrase repair, MP-05 BIP70, spend unconfirmed change, per-transaction notifications | 11-19 → 11-23 | `wallet.dat` merged (#34, 11-20); six fixes from its last review merged (#119, 11-20); what is left is listed in #81: the watch-only rewind and the retry of a stored range that fails at load are in #121, the rest is not started |
| H Marketplace | PO | Cut: X4 (browse, buy, extra names, instant-name offer), identity-verify link | — | cut to 1.0.x |
| I Invitations | PO | Cut: the storage patch, X3 create, share and history, shielded-funded and small invites, the Home claim card, scanning an invitation's QR code. Claiming an invitation from a link or pasted text shipped in 0.1 and stays | — | cut to 1.0.x |
| J DashConnect | PO | Cut: App Connect v1 and v2 | — | cut to 1.0.x |
| K Everyday polish | PO | #25 empty states → P7 unlock once → P15 PSBT under Advanced. Cut: MP-04 tax categories, lock-screen receive and scan, log export, the support path and the backup reminder, transaction metadata sync and restore on Platform | 11-18 → 11-24 | all three merged on 11-19 (#25, #37, #57); what is left of unlock once was #78, fixed and merged (#160, 11-20); what that leaves is listed in #173 |
| P Localization | PO | MP-14, translated in this repository by agents (no Transifex): the message catalog, locale choice, plurals and the right-to-left check, with one screen converted as the pattern (11-19 → 11-21) → every screen's copy moved into the catalog, one small pull request per screen area, each after that area's open pull requests have merged (11-21 → 11-26) → copy freeze 11-27 → the translations, each checked by a second pass (11-27 → 11-29) | 11-19 → 11-29 | the foundation merged (#146, 11-20): the catalog, the language choice, plurals and two fixture-only pseudo-locales, with the shell, the lock screen and Settings ▸ General converted; what must be fixed before the first translation and the first right-to-left language ship is listed in #155; the per-area conversions start 11-21 |
| L Commerce | PO | Cut: MP-09 Explore, MP-13 CrowdNode withdraw, MP-10, MP-11, MP-12 | — | cut to 1.0.x |
| M Hardening | PO | H-02 fuzzing → H-03 kill -9 matrix → H-06 performance → H-09 trust gate on by default → **H-01 security review (11-27 → 11-29)**. Cut: H-10 node pinning, Tor/SOCKS5, MP-03, MP-15 | 11-19 → 11-29 | H-02 merged (#149, 11-20): ten fuzz targets, a seed replay in the test suite and a nightly job, with its gaps listed in #166; a PSBT defect its fuzzing found is fixed (#130, 11-20). H-09 merged (#334, 11-23): contact payments and masternode withdrawals refuse on unverified Platform data. H-06 merged (#343, 11-23; probe fixes #355, 11-23): the contacts query passes (p50 34 ms at 1,000 contacts); `history_page(200)` (p95 120 ms on 40k records, 6× the 20 ms budget), wallet open and engine memory miss, filed for 1.0.x as #296–#298; the contacts read's per-row rebuild is a 1.0.x follow-up (#299). A withdrawal-test flake (#344) is fixed (#349, 11-23). H-03 under way, not merged: the kill -9 matrix's harness and rows for registration, contact requests, profile edits, a reserved contact payment, sync apply, top-up, a `.dwbackup` restore and a phrase import; its findings are filed for 1.0.x (#358 avatar files, #381 bell rows). H-01 not started (from 11-27) |
| N Release and packaging | MG | cut 0.2 → `parity.md` redone for the Tauri app → PK-03 auto-update → W-02 Windows services → Flatpak → SBOM and checklist → retire Swift (U-08) → each release cut. Cut: Windows Hello in W-02 (passphrase only on Windows), and what is left of the tray companion on Windows and Linux | 11-19 → 12-02 | 0.2.0 published 11-20; 0.3.0 published 11-22, the first release that can update itself; `parity.md` redone (#32); SBOM and checklist merged (#54); Flatpak merged (#56) |
| O Test suites and QA | PO | DP2-06 (never-cut), DP1-07, DP3-05 suites → visual baselines refresh, issue #29 → iOS/Android restore test → H-04 real-node flows → H-08 full QA pass on three OSes → H-05 accessibility → H-07 mainnet canary, all done by 11-30 for the release candidate; 12-01 is for its fixes | 11-19 → 11-30 | the three suites merged (#129, 11-20), with the harness gaps listed in #168; their nightly on testnet is not installed yet, because where its failure messages go is not decided; the upgrade-path checks from v0.1.0 (#102) and from v0.2.0 (#152) merged on 11-20, and the #152 review items closed in #280 and #281 (11-22); the v0.3.0 fixture for #101 is captured from the tagged release; the by-hand H-08 script for macOS and Windows is written (`docs/qa/H-08-macos-windows.md`, #278, for pasta around 11-28); keyboard-only DashPay journeys merged (#289), and the defects they found (#286, #287, #288, #291) are the H-05 fixes; visual baselines refresh not started, and it now has to cover the whole set (about 150 images differ on `main`) |

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
| DashConnect, App Connect v1 and v2 | lane J | the whole lane |
| Explore, CrowdNode withdraw, DashSpend, Buy & Sell, ZenLedger (MP-09…MP-13) | lane L | the whole lane; the partner credentials are no longer needed for 1.0 |
| Username marketplace (X4) and the identity-verify link | lane H | the whole lane |
| Creating invitations (X3) and its storage patch, the Home claim card, scanning an invitation's QR code | lane I | the whole lane; claiming an invitation from a link or pasted text stays |
| MP-04 tax categories, MP-06 phrase repair, MP-07 sweep and BIP38, MP-05 BIP70 | lanes G, K | the 0.4 and 0.5 parity items |
| Spend unconfirmed change, per-transaction notifications, lock-screen receive and scan, log export, the support path, the backup reminder, transaction metadata sync and restore on Platform | lanes G, K | small parity items after the cut ones |
| CoinJoin sleep prevention and start on login | lane F | an extra cut, beyond ROADMAP §6's order |
| Tor/SOCKS5 | lane M | an extra cut, beyond ROADMAP §6's order |
| Last-known balance at launch (the second half of P5) | lane B | cut on 2026-11-20, the project manager's decision: it would show a balance before the app knows whether it is locked (#133); the blank-launch fix merged (#33, 11-19) |

ROADMAP §6's cuts 3, 4 and 7 do not apply, because those shipped: X1a; DP6-01 and DP6-02 (the Identities page
and withdrawing an identity's credits to Core; withdrawing to one's own Platform address is a lane C item, not
this one); and MP-01 fiat. Cut 8 (MP-14, English
only) is not applied, on pasta's decision of 2026-11-19: localization is in 1.0, as lane P.

**The 1.0 core, due in the release candidate on 11-30:**

- everything released in 0.1 and on `main` today (the DashPay identity, username, contacts and payments flows,
  send and receive, history, CoinJoin, backup and restore, security, the tools and settings);
- instant incoming payments (lane A) and the two-second launch (lane B: the window in two seconds; showing the
  last-known balance at once is cut to 1.0.x);
- the wallet-holder masternode and governance screens (lanes D and E), with shared masternodes. The repository's
  `CLAUDE.md` and DASHPAY still call this milestone "release 1.1"; ROADMAP §7 (DEC-1026, 2026-11-01) moved it
  into 1.0, owner-side only as before;
- Platform ↔ Shielded transfers and the item 0 fixes (lane C; the release owner decides which of its later items stay);
- the CoinJoin fixes BL-614 and BL-615, V24 promotion and demotion, mixed-send warnings (lane F);
- opening a legacy Berkeley DB `wallet.dat` (lane G);
- empty states, unlock once, PSBT under Advanced (lane K);
- localization (lane P): the app's own screens in the languages below; text that comes from the engine or the
  network as a message, not a code, stays English in 1.0;
- hardening: fuzzing, the kill -9 matrix, performance, the trust gate, the security review (lane M);
- the DashPay suites, the QA pass on three OSes, accessibility, the canary (lane O);
- releases and packaging: auto-update, Flatpak, SBOM and checklist (lane N).

`docs/parity.md` still names 0.4 and 0.5 for the cut rows; its Release column is brought in line in lane N.

### Localization without slipping the release candidate

- **Languages in the release candidate:** Spanish, Portuguese, German, French, Italian, Russian, Turkish,
  Simplified Chinese, Japanese, Korean, and Arabic (which is also the right-to-left check). English stays the
  source and the fallback for any missing string. More languages are a 1.0.x item.
- **Who translates:** agents, in this repository; no Transifex and no outside access. Each language gets a second,
  independent pass that checks meaning, length and the wallet's terms (a glossary is kept with the catalog), and
  the release notes say the translations are machine-made and that corrections are welcome as pull requests.
- **Not slipping 11-30:** moving a screen's copy into the catalog changes no behavior, and English stays the
  default until a language is complete, so a half-converted app is always releasable. Each conversion pull
  request covers one screen area and waits for that area's open pull requests, so it does not collide with the
  other lanes. Copy freezes on 11-27; a string changed after that ships in English until its translation lands.
- **If it slips:** the release candidate ships the languages that are complete and checked; the rest follow in
  1.0.x. Localization never holds the release candidate.

### H-08: the upgrade path from an earlier release

Users of 0.1.0 (and of each later release) upgrade in place, so a data root an older release wrote must open with
the same wallets, balances, history, labels, settings and keys.

**Linux, automated.** `testdata/upgrade/<tag>/` holds a data root written by that release's own Linux binary
(`apps/desktop/scripts/upgrade-fixture.sh <tag>`; how, in `testdata/upgrade/README.md`) and the release's answers to
its read commands (46 for v0.1.0, 61 for v0.2.0). `dw-app`'s `upgrade_path_tests` opens a copy twice with the current
build and requires the same answers and the same message signatures, apart from the differences the test lists as
known. Each committed `expected.json` is pinned by its SHA-256 in the test, so an answer cannot be edited to hide a
difference. It runs in `cargo test --workspace`: "Rust tests (Linux)" on every pull request that changes code, and the
macOS job on `main`. A new release is covered by committing its directory:

```sh
(cd apps/desktop && DASHD=<dir>/dashd scripts/upgrade-fixture.sh v0.3.0 <sha256 of the release's .deb>)
# commit testdata/upgrade/v0.3.0 and pin the SHA-256 of its expected.json in SNAPSHOT_SHA256
(cd rust && cargo test -p dw-app --lib upgrade_path_tests)                # test upgrade_path_tests::… ok
```

**macOS and Windows, by hand in the QA pass** (not run yet): [`docs/qa/H-08-macos-windows.md`](qa/H-08-macos-windows.md).
The automated check does not cover each OS's installer upgrading in place, its default data root, its secret store,
uninstalling or the unsigned first launch, so that script does: on a Mac with Apple silicon, an Intel Mac, Windows 11
and Windows 10, it installs the release candidate fresh, then upgrades in place from 0.1.0, 0.2.0 and 0.3.0 twice
each, once with the vault behind a passphrase and once with its key in the OS secret store (one vault per network,
so these are two runs from an empty data root), downgrades, restores, and uninstalls with the data kept and removed. From a clean data root the network is chosen on Welcome, and 0.1.0 applies it only after a restart (#101
item 12). No release has quick unlock, so there is no Touch ID or Windows Hello step. Its sections 7 to 9 add, on
both OSes, the DashPay journeys the Linux suites automate, VoiceOver and Narrator (for H-05), and the DashPay flows
that only the browser fixture tests cover, run in the real app on testnet.

## What the funds fixes leave for the release candidate

Decided 2026-11-20 with the release owner. The fixes for #88 (#134), for #68, #69 and #74 (#121), for #78
(#160) and for #45, #46, #47 and #120 (#159) each leave cases open. A case is RC must-fix when it can hide a
payment or coins, release a spent coin, or let a payment through without the credential the user chose; the
rest are for 1.0.x. One exemption, added on 11-20: a case is not RC must-fix when it only understates what
the wallet holds, never offers a coin that cannot be spent, and corrects itself at the next block with
nothing for the user to do. Such a case is for 1.0.x and gets a line in the release notes. The cases are
listed in #170, #172, #173 and #177.

**#170, the balance after a restart.** Eleven cases are RC must-fix, all in the table. Two were added on
11-20 after #134's last review: case 16, the part of case 14 its fix does not reach, and case 17. Case 15
(a failed write of the claim repair) left the list the same day: two more readings of the pinned store
found no path to it, so it is for 1.0.x. Case 18 is not counted here: a ProTx whose sent mark cannot be
written reads as sent although nothing went out, new with #134 and narrow; the release owner's masternode
lane has written the fix, in its own pull request, for 0.3 if that is reviewed and green by 11-22 18:00 UTC. Sizes are
working days with tests and review, from reading the code; nothing was built except case 14.

| Case of #170 | For the release candidate | Days | State |
|---|---|---|---|
| 14: a locked payment that a ChainLocked spend beat cannot be abandoned (worse than before #134) | promised. The fix is 85 lines outside tests, over the 50 set for 0.3, so it merges after 0.3 is cut and 0.3 carries a notes line | written | in a pull request (#183), fix round pushed on 11-21; to be reviewed on 11-23 |
| 16: the same payment when the winner pays only other addresses, so nothing the wallet keeps proves it lost | needs rust-dashcore, so it goes with the fork pin batch; if it cannot make the release candidate's pins, the release owner decides on 11-25, in writing, with its notes line | not sized; sized by 11-25 | not started |
| 17: a shared masternode registration's marker names the other participants' inputs, so a co-participant can get a coin the wallet has not found yet hidden until the registration is abandoned | closed by #159 at load: its start-up repair drops a claim whose input script is not the coin owner's signature, at the first start after the coin is stored (the fork lane's answer on 11-20, with a test). If #159 is not in the release candidate, the marker has to name only the outpoints this wallet signed. Checking the other participants' signatures when the registration is built is for 1.0.x | with #159; the marker change is not sized | closed at load by #159, merged 11-21 |
| 11, the finality order: a spent coin can be offered | promised | under 1 | starts 11-21 |
| 5: abandon, and a winner the store has no row for | parked for 1.0.x; the release candidate's notes carry the disclosure | 1.5 to 2.5 | parked for 1.0.x: #250 closed unmerged; the root fix is #237; the RC release-notes disclosure is [#237 comment](https://github.com/PastaPastaPasta/dashwallet-desktop/issues/237#issuecomment-7128932297) |
| 1: a crash during a send | promised | 2.5 to 3.5 | not started |
| 7: unreadable evidence at load | promised. Its pull request first closes the two ways #134 still counts a load as complete when it is not (a skipped malformed row, and an abandoned mark that was never written) | 1.5 to 2.5 | not started |
| 3 and 4 together: phantom change of a beaten payment, and its restored child (also when the winner is in the history and the child still claims its other coin) | attempted | 4.5 to 6.5 | not started |
| 2: a coin stored unspent that the wallet does not hold | attempted, after 3 and 4 | 2 to 3 | not started |
| 6: a reorg that disconnects a plain block | expected in 1.0.x; needs the platform fork and a pin bump | 3 to 4.5 | not started |

- Each case gets its own pull request, with a review and a test that fails without the fix. They are worked
  in the order of the table: two lanes from 11-21, and a third when the fork lane frees (it stays on #177's fixes after #159 merged; the lanes #121
  and #160 free go to #172 and #173, below).
- **Promised.** If a promised case is not merged by 11-28 18:00 UTC, the release owner decides then, in
  writing, whether the release candidate moves or the case becomes a line in its notes. Nothing slips by
  default.
- **Attempted.** Whatever is not in review by 11-27 18:00 UTC goes into the release candidate's notes (the
  lines are written), and the work continues for 1.0.x.
- **Case 6** may still make the release candidate if the fork pin batch has room. It is not traded for a
  promised case. The same holds for the part of case 14 that needs a key-wallet change (a peer relays the
  beaten payment again after the restart); otherwise that part is a 1.0.x line in #170. Case 16 goes with
  the same batch but is RC must-fix, so it is decided in writing, not left for 1.0.x by default.
- **The root fix** (rebuilding key-wallet's spent-input state at load, across rust-dashcore, platform and
  this repository) takes 12 to 17 days at the shortest. It leaves the 1.0 plan and is a 1.1 item. No RC
  must-fix case needs it, and it would close cases 5 and 14 only partly.

**#172, CoinJoin and restored wallets, and #173, the signing window.** #172 holds six RC must-fix cases
and #173 two. Sizes are working days with tests and review, from a read of the code on 11-20; nothing was
built. Every fix is in this repository, with no pin bump.

| Case | For the release candidate | Days | State |
|---|---|---|---|
| #172: a wallet whose birth is below dash-spv's header anchor never sees its older coins | promised | 2.5 to 4 | not started; follows #121 |
| #173 item 6: lowering the spending limit leaves issued window grants usable | promised | 0.5 | merged 11-21 (#185) |
| #172: attaching keys with an earlier birth height rescans nothing | promised | 0.5 to 1 | not started |
| #172: CoinJoin Recovery stores its start and its address range in two writes | promised | 0.5 to 1 | not started |
| #172: older-build wallets not replayed on upgrade: a restored seed whose only CoinJoin use the narrow scan missed, and a seed created here but mixed elsewhere (two cases, one fix) | promised (moved from attempted on 11-20, when the replay was timed and chosen), with the first-start message below | 0.5 to 1.25, plus the first-start message (not sized) | not started; follows the header-anchor case |
| #172: an older-build wallet loaded, then a quit before widening | promised, on top of the row above | 0.5 to 1 | not started |
| #173 item 1: an older manifest and spending limit put back together | attempted; a known limit remains even with the fix (both files rolled back together) | 2 to 3 | merged 11-21 (#198); what it leaves is items 13 and 14 of #173, for 1.0.x |

- The same dates apply as for #170: a promised case not merged by 11-28 18:00 UTC is decided in writing by
  the release owner; an attempted case not in review by 11-27 18:00 UTC goes into the notes.
- Promised work in these two tables is twelve cases: #170's five (6 to 9.5 days left, with case 14 written) and seven here (5 to 8.75 days, plus the first-start message, which is not sized). #177 adds two more (2.5 days), below. The release
  owner checks it against what has merged on 11-25 18:00 UTC; if the promised cases are not on track
  then, the release candidate moves or scope is cut in writing that day, not on 11-28.
- **The header-anchor case is the most harmful of the eight, and it is ordinary use, not a fault.** A
  restore without a date scans from the first block, and dash-spv sets its header anchor once. Anyone who
  restores an older wallet after another wallet has synced sees none of its payments below that anchor
  (the checkpoint at or below the first wallet's birth), on `main` and in 0.2 today. Until the fix: Tools ▸
  Repair ▸ "Reset chain data and resync". Its size depends on whether SPV can be restarted safely while
  mixing or a Platform pass runs. It cannot start before #121 merges, so #121 has to merge by about 11-23
  for this promise to hold; the 11-24 report below covers it. 0.3's release notes carry the workaround,
  and a detection-only change is sized in the first hour after #121 merges: when a wallet is
  restored or added, or its birth is lowered, below the stored header start, the app shows a notice with a
  button to that Repair action, and resets nothing itself. It ships in 0.3 only if it is about 50 lines or
  fewer outside tests; otherwise the notes line stands alone.
- **The older-build cases.** The two not-replayed cases have one workable fix: every wallet upgraded from
  0.1 or 0.2 that has a CoinJoin account or a stored address lookahead is replayed once from its birth,
  with sends held until it catches up. The product owner chose it on 11-20, after timing it on mainnet
  with 0.2.0: on an install that already holds the chain's filters, a rescan from the earliest block the
  app scans (height 200,000, dash-spv's first checkpoint) took about 17 seconds, on a heavily loaded
  machine, and downloaded nothing again. That was an empty wallet. A wallet with history matches more
  blocks and downloads those not already stored, and an install whose stored filters start above the
  wallet's birth downloads the missing range at first-sync speed (8 min 45 s for the whole range with
  nothing on disk). Neither was measured. The promise includes the message: the first start after the
  upgrade says why sends are held and roughly for how long. The message is not in the size above: it
  needs the app to tell an upgrade replay from a rescan the user started, and its copy has to be in the
  catalog before the copy freeze on 11-27. The loaded-then-quit case is fixed on top of the replay. In
  0.2.0, a rescan started from Tools ▸ Repair does not hold sends, and nothing outside that tab shows it
  is running; #121 holds sends while its replays run. If #121 is not in 0.3, 0.3's notes tell the user to
  wait for a rescan to finish before sending.
- Lanes: the header-anchor case goes to the #121 lane when #121 merges (the same functions), and the
  older-build cases follow it there. #173's two are with the #160 lane now that #160 has merged, and the attach-keys
  and Recovery cases follow them there. The two #170 lanes are not used for these, so four lanes work on
  the RC must-fix cases once #121 and #160 have merged. If neither lane has freed by 11-24, the product
  owner reports it that day and the release owner decides what they displace. In one lane the
  header-anchor and older-build cases take 3.5 to 6.25 days from #121's merge, so with #121 merging on
  11-23 the top of that range misses 11-28. The older-build cases do not depend on the header-anchor fix
  (different functions). If #121 has not merged by 11-23 18:00 UTC, they start in a lane of their own
  that day (the release owner's decision, 11-20); otherwise the 11-24 report says whether they need one.

**#177, after the fork pin, and #178.** #177 lists what #159 leaves. Three of its items are RC
must-fix, ruled by the release owner on 11-20:

| Item of #177 | For the release candidate | Days | State |
|---|---|---|---|
| 2: an identity-funding or DashPay payment status written before the upgrade keeps a finality that was never verified | promised | 1.5 | not started; follows the fork lane's fixes for items 9 and 10 |
| 6: transactions dropped as dead are remembered only until a restart, so a peer can have one credited again | promised | 1 | not started |
| 8: the start-up repair does not follow a dropped transaction to what spends its outputs, or to another wallet it pays | attempted, after items 2 and 6 | 1 | not started |

- The same dates apply as above: 11-28 18:00 UTC for the promised items, 11-27 18:00 UTC for the attempted
  one, and the release owner's check on 11-25 18:00 UTC.
- #159 merged on 11-21 and is in 0.3. The fork lane takes these three, in that order, after its fixes
  for items 9 and 10 (a dismissed transaction's child, and a lock that arrives late), which re-pin first
  in their own pull request.
- Item 1 (a payment to a newly used address of an already-synced wallet can stay unseen until it is
  mined) falls under the exemption above: 1.0.x, with a notes line. Item 4 (a spend the wallet cannot
  judge still debits the coin) is 1.0.x as long as no funds account can hold a coin a peer could claim
  that way; the fork lane's reading so far is that none can, and one more answer is due.

#178 is not a funds fault: a wallet that is merely open downloads about 0.5 GB an hour from Platform for
shielded-note sync. It is an RC item in lane C, sized and scheduled by the release owner.

## What only pasta can unblock

Without these the release still ships on 12-02, in the reduced form the release plan already allows, except for
signing: without it the final tag waits (first row).
No longer on this list: partner credentials and Explore database access (lane L is cut), and Transifex access
(the translations are made in this repository).

| Needed | For | By | If it does not arrive |
|---|---|---|---|
| Signing 1.0 on pasta's own machines with Dash Core's identities: the Apple Developer ID with notarization, the Windows Authenticode certificate on pasta's YubiKey, pasta's PGP key over `SHA256SUMS`, and a stable updater key that stays local | signed installers, `SHA256SUMS.asc`, the signed update feed | 11-27 | the final tag fails closed and doesn't publish (#780); shipping 1.0 unsigned needs a written ruling. Release candidates are unsigned either way |
| Up to 0.5 DASH on mainnet and an iOS user for an hour | mainnet canary (H-07) | 11-28 | canary runs on testnet only |
| A funded testnet wallet created on iOS and one on Android | same-seed restore test | 11-24 | checked by code comparison only |
| A real-Mac session and a real Windows PC | Touch ID, VoiceOver, Narrator, first notarized launch | 11-28 | passphrase unlock only on macOS; screen-reader checks on Linux only |
| Go-ahead for the upstream platform pull request (E0-04.P3/P4), or agreement to drop it | Mode A dispatch | 11-21 | dropped as superseded by item 0's hook |

Signing follows Dash Core's detached-signatures model (#780; the runbook is `docs/release/SIGNING.md`, which lands
with it). CI builds the unsigned files and `SHA256SUMS` and holds no signing secrets. pasta signs on pasta's own
machines, and CI attaches the detached signatures, checks them and publishes. Release candidates (`-rc.N`) stay
unsigned.

## Risks

- **The date is tight.** The earlier plan put 1.0 in late January. If a lane slips, the cut order of ROADMAP §6
  applies, then lanes J, L and the 0.4 parity items move to a 1.0.x release. The never-cut list stands.
  This was applied on 2026-11-19 (above). The next cuts, if the core slips again: PSBT under Advanced, mixed-send
  warnings, CoinJoin V24, then the later items of lane C.
- **Review and CI throughput.** A full CI run takes about 90 minutes on macOS. With 12 lanes active the runners
  queue; engine-only and renderer-only pull requests should not wait on each other's jobs.
- **The RC must-fix list grew on 11-20.** It holds 24 cases: 22 left by the four funds fixes (#170
  eleven, #172 six, #173 two, #177 three), #178 in lane C, and #190 (fixed in #192, merged 11-21). Of #170's, case 14 is written, eight add up
  to about 15.5 to 23.5 working days by its table (12.5 to 19 without case 6, which is expected in
  1.0.x), case 16 is not sized and case 17 is closed at load by #159; #172's and #173's eight add up to 7 to 11.75, with the
  first-start message not sized; #177's three add up to 3.5; #178 is not sized. Fourteen of the 22 are
  promised, and #190 is fixed and merged. Case 16 has no term yet. The tables above say which cases are promised, which are
  attempted and when each is decided.
- **Security review on new code.** H-01 runs 11-27 → 11-29 over code written the same fortnight. Its
  findings land before the release candidate or the affected feature ships disabled.
