# MG-11: Username voting (Tauri)

ROADMAP MG-11 on the Tauri renderer: the UI for MG-07's contested-name voting (IOS-079). Masternodes whose voting
key the wallet holds vote on who gets a contested DPNS username: one of the contenders, Abstain, or Lock (nobody
gets it). The engine side is MG-07's `Governance::contested_names` and `vote_name` (`docs/contracts/mg-engine.md`
§2.6); MG-11 binds them in dw-app and builds the tab, the detail pane, the vote sheet and its result. iOS is the
UX and behaviour reference, Android the vote-count wording (UX-SPEC §4.25.1, §4.25.4; MG-00's follow-up for
MG-07/MG-11). Tier T0: fixture and vectors only, no live network.

| Part | Where |
|---|---|
| Commands `governance_contested_names`, `governance_vote_name`; their DTOs (`NameRejectionName`, D15); the contest codes' `data` | `rust/crates/dw-app/src/api/governance.rs`, `src/dto/governance.rs`, `src/error/governance.rs` |
| Vectors (MG-07's fixtures through dw-app) | `rust/crates/dw-app/src/governance_vectors_tests.rs` → `apps/desktop/src/fixture/vectors/governance.json` |
| View model (pure TypeScript) and its tests | `apps/desktop/src/governance/names.ts`, `names.test.ts` |
| Query | `apps/desktop/src/governance/queries.ts` `useContestedNames` |
| Screens | `apps/desktop/src/screens/masternodes/NamesTab.tsx`, `NameVoteSheet.tsx`; the tab bar in `MasternodesPage.tsx` |
| Fixture: state, handlers, scenarios | `apps/desktop/src/fixture/names.ts`, `names.test.ts` |
| Playwright (flows, axe, light and dark, Chromium and WebKit) | `e2e/names.spec.ts`; baselines in `e2e/visual.spec.ts` (`names-*`) |

## Screen inventory

| Part | What it shows | Spec |
|---|---|---|
| Tab bar | **Governance** · **Username voting** under the "Masternodes" title (D1, D2) | §4.25.1, MG-00 D3 |
| Intro | one line: what the vote decides and Drive's rule | — |
| Toolbar | search (DPNS-normalized), Sort: Ending soonest · Name · Most votes · Most lock votes (D4) | iOS |
| Contest rows | label; "2 contenders · ends Jan 15 · You: 2 of 3 voted"; the weighted vote count; "Ends in 3 hours" (warning) under a day, "Voting ended" (neutral) after the end (D7) | MG-09 D8 rows |
| Detail pane | label, chip, "Ends Jan 15 at 9:00 AM · in about 100 days 21 hours"; the weighted tally (contenders, Lock, Abstain) with who leads (D5, D6); the partial-list note (D8); Your masternodes with their present choice and votes left (D9); the vote-limit note; Vote, or why not (D10) | §4.25.4 |
| Vote sheet | the end time; Your vote (contenders, Abstain, Lock as radios); Vote with (one row per voter: custody, weight, its vote now); privacy and last-vote warnings; the limit note; passphrase; "N masternodes · weight w · no fee"; Vote for 8ZtW…R97P (D10–D13) | §4.25.4 |
| Result | "1 of 2 votes counted · 1 not confirmed yet", "Abstain on a11ce · weight 1 of 2", one line per masternode with Counted / Unconfirmed / Not counted and a sentence; Check again while one is unconfirmed (D14, D15) | §4.25.4 |
| States | loading; Couldn't read (with the code's sentence and Try again); no contest; no match; browsing only (no voter); watch-only (D16) | MG-09 D24 |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Username voting is a tab beside Governance on the Masternodes page (`Tabs`, the WAI-ARIA tablist), not a section of the Governance tab. MG-10 adds My masternodes to the same bar. This reverses MG-09 D1's "no tab bar until MG-10": there are now two tabs. | Name votes need no governance sync (MG-07 D2), so they can't sit under Governance's Disabled state, and they read Platform, not the synced store. iOS lists Masternodes and Voting as siblings under Governance; MG-00 D3 plans tabs on this page. |
| D2 | The tab is page state (`useState`), not `ui.masternodesTab`; the page opens on Governance. The header's More and Create proposal show only on the Governance tab. | MG-10 owns `ui.masternodesTab` (MG-09 F2); keeping it page-local avoids a shared `ui.ts` edit while MG-04 and MG-10 are in flight. Governance's actions do nothing for names. |
| D3 | One query, `["contestedNames", walletId]`, outside the `["governance"]` prefix; it re-reads every 60 s, after a vote, on Try again and on Check again. | The `governance` event comes up to once a second during a sync and would send Platform reads for nothing; a contest changes on Platform's blocks, not Core's. |
| D4 | Search compares as DPNS does (lower case, `o` → `0`, `i`/`l` → `1`), so "Alice" finds `a11ce`; a search with no match says so with that rule. The sorts are iOS's; Ending soonest (default) is the engine's order. | Contested labels are stored normalized (MG-07 D15); a user types the name, not the label. |
| D5 | Contenders show as identity ids, shortened (`8ZtW…R97P`), the full id on hover and a copy button. | The facade carries no contender names (MG-07 §3); a DPNS reverse lookup per contender is a follow-up (F1). |
| D6 | The tally is weighted (an evonode's vote is 4); a missing count reads 0. "Leading" follows Drive's rule: the top contender takes the name unless Lock has strictly more votes (`check_for_ended_vote_polls`, pin `82fb33aff8`). A tie between top contenders names no one ("Tied at 9 votes between 2 contenders"). | Drive breaks a contender tie by document order, which differs between versions (MG-07 §4), so the app doesn't promise a winner. |
| D7 | A contest is closed from its `endsAt` on the app's clock (MG-07 D8): a neutral "Voting ended" chip, a notice ("Platform gives the name out by these votes in its next block, and then it leaves this list") and Vote off. A contest ending within a day has a warning "Ends in …" chip. A resolved contest is not listed: `contested_names` returns open contests only, so it leaves at the next read. | Platform resolves at the first block after the end; until then the engine still lists it, and a vote would be refused as `contest_closed`. |
| D8 | When `contendersComplete` is false the row says "2+ contenders" and the pane and sheet say only the first 100 contenders are listed and votable here. | MG-07 D9: the engine reads one page of contenders; `not_a_contender` refuses the rest. |
| D9 | Votes left use Android's wording ("3 votes left", "One vote left", "No votes left") and a note under Your masternodes and in the sheet says the count is this wallet's: "votes sent from another wallet aren't included". | `votes_used` is a local lower bound (MG-07 D6); saying so is the honest reading. |
| D10 | Vote is off, with the reason under it, for a watch-only wallet, a closed contest, a wallet with no voter, and when every voter used its votes on the name. The sheet presets no choice; the choices are the contenders (by Platform's order), then Abstain, then Lock, each with what it means and its votes; the button names the choice ("Vote for 8ZtW…R97P", "Vote Lock"). | A preset equal to a voter's present choice would only be refused as `already_voted` (MG-07 D18). iOS's order: approve, abstain, lock. |
| D11 | One row per voter (§4.25.4's rows): its name and custody ("key in this wallet · voting key 1" or "key you added"), weight, votes left and its vote now. A voter is off when it used its votes on the name (the engine refuses it, MG-07 D6) or when its present choice is the one picked (Platform answers `already_voted`). Every voter with a vote left starts checked (MG-00 D12), with the privacy warning at two or more; a checked voter casting its last vote gets a warning that its choice can't change after it. Only checked, eligible voters are sent (`pickedVoters`). | iOS starts with a single node selected; the desktop keeps MG-00 D12's "all eligible, warn on linkage" for both vote sheets so they behave alike. |
| D12 | A PoSe-banned masternode stays a voter, labelled "PoSe banned, still votes on names". | Drive looks voters up in its full masternode list (`transform_into_action/v0`), so a banned masternode's name vote counts, unlike its proposal vote. |
| D13 | The sheet keeps the read it was opened from while the list re-reads under it (MG-09 D31). A whole-call refusal stays in the sheet as the passphrase field's error, in a sentence: the three contest codes name the label or the identity from their new `data` (`contestLabel{label}`, `contender{identity}`, MG-07 D13); the rest are MG-09's governance sentences or the send/Platform ones. The list re-reads after a refusal. | A closed contest or a contender gone between opening and sending must say which; the label in `data` keeps the message out of the UI. |
| D14 | The result counts a vote only when `rejected` is null. `unconfirmed` is "Unconfirmed" with "Sent, but Platform didn't confirm it. It may still count: check again in a minute."; it turns Counted only after **Check again** re-reads and finds that masternode's present choice is the one cast. The first result never uses the re-read made right after the vote. Chips: Counted (success), Unconfirmed and Not counted (neutral, MG-09 D26). | MG-07 D7: an unproved vote may or may not have landed; showing it as success would be a false claim. Only an explicit re-read is evidence. |
| D15 | dw-app passes `rejected` as `NameRejectionName`, the closed union of the 17 reasons of MG-07 §3 (`every_name_vote_reason_crosses_as_itself` holds it to MG-07's fixture, which has each once); a reason this build doesn't know crosses as `refused`. The renderer's sentence map is typed by that union, so tsc refuses a reason without its own sentence; none shows the raw value. An unknown masternode is named by its hash. | MG-09 D13's rule. The union also keeps the reasons out of the fixture's sentinels: the production bundle holds them because the generated bindings do (the 20-character `masternode_not_found` and `platform_unavailable` failed check-bundle while they were bare strings). |
| D16 | States: loading ("Reading the votes on usernames from Platform…"); an error with its sentence and Try again (`list_unavailable`: "The masternode list hasn't loaded yet…"; `platform.unavailable`/`timeout`: "Couldn't reach Dash Platform…"); no open contest; browsing only when the wallet has no voter; watch-only. | MG-09 D24's states for the second tab. |
| D17 | dw-app adds no codes: the three contest codes gain `data` (two new `ErrorData` kinds). No m-contract §4 table changes, and the Swift host binds no governance call, so Swift T0 has nothing to run against and `engineCodes` are unchanged. | Brief: run Swift only if a §4 table or codes Swift must know change; neither does. |
| D18 | The engine's `open_contests` collects owned labels before streaming the polls, and a compile-time test asserts both name calls are `Send`. No signature change, so no Contract-Version bump. | A closure over `&(String, u64)` held across the await failed the Tauri command's `Send` bound ("FnOnce is not general enough"); MG-07 never bound the calls, so nothing caught it. |
| D19 | The fixture holds MG-07's record as a literal (`fixture/names.ts`); `fixture/names.test.ts` holds it equal to the vectors. Its voters reuse the governance fixture's `a1…`, `b2…`, `c3…` hashes. `vote_name` refuses in the facade's order and counts as Drive does (weight moved, `already_voted`, the limit); a scenario changes values, never the shape. | A fixture module may not import the vectors (`fixture.test.ts`: test data stays out of fixture builds). |
| D20 | View-model mutants: the limit ignored in `nameVoterRows`, `unconfirmed` counted as success in `nameVoteResult`, and an ineligible checked voter sent by `pickedVoters`. Each is killed by a named test (report). The brief's example "a masternode without a voting key offered" has no state in this record: MG-07 D3's voters hold a key by construction, so the nearest is a voter that can't vote with the choice. | Mutate where a wrong answer reaches Platform or the user. |
| D21 | Visual baselines: six new (`names-list`, `names-vote`, `names-vote-result`, light and dark). MG-09's sixteen governance baselines were re-shot because the tab bar moves the page down by 46 px; the diff was checked by eye to be that shift and nothing else. | The tab bar is in every Masternodes screen. |
| D22 | On a contest row, the warning chip's tint is drawn over the card colour (`.mg-row .chip-warning`), as on contact rows. | The selected row's accent tint under a plain warning chip failed axe's colour contrast in light (both engines). |
| D23 | IOS-079 stays `partial`: list, search and sort, detail, approve / abstain / lock, several masternodes in one vote and node selection are done; iOS's vote history (a list of past votes) is not, because the facade keeps only each masternode's present choice and this wallet's count. iOS's Voting toggle is Settings ▸ General ▸ Show Masternodes (MG-09 D1). Voting on several contests at once is not offered. Not driven live (T0). | Parity counts what a user can do. |

## Follow-ups

| # | Follow-up |
|---|---|
| F1 | Contender names: a DPNS lookup of each contender's identity (or the contest's documents) so rows read "alice (8ZtW…R97P)" (D5). |
| F2 | Vote history (IOS-079): an engine record of this wallet's name votes with their times, then a History section in the pane (D23). |
| F3 | MG-10: `ui.masternodesTab` replaces the page-local tab state, and My masternodes joins the bar (D1, D2); voter rows take the masternode's label (MG-09 F3). |
| F4 | A live run (T2) of the UI against dashmate when a contest exists there, as MG-07 D17 did for the engine. |
