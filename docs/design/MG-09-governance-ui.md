# MG-09: Governance tab (Tauri)

ROADMAP MG-09 on the Tauri renderer: the Governance tab of UX-SPEC §4.25.3–4.25.4 (MG-00), with the §4.25.1 frame
it needs (MG-00's hand-over row: whichever of MG-09/MG-10 lands first builds the shell). It shows the governance
clock and budget, the proposal list with filters and a detail pane, the vote sheet over the wallet's masternodes,
the Create proposal wizard and Mine with MG-06's states, and Governance numbers… (QT-134). Acceptance: on regtest,
vote on a proposal and create one from the UI.

The engine side is merged: MG-02's `Governance` facade (`docs/contracts/mg-engine.md`), MG-05's voting grants
and MG-06's own proposals, bound in dw-app as the `governance_*` commands (BL-232, E0-13). The page uses that
binding as it is and adds no command (D2). The copy is MG-00's; deviations are D-rows here.

| Part | Where |
|---|---|
| View model (pure TypeScript, no React) | `apps/desktop/src/governance/model.ts` |
| Its tests | `apps/desktop/src/governance/model.test.ts` |
| Queries (TanStack, keys under `["governance", …]`) | `apps/desktop/src/governance/queries.ts` |
| Screens | `apps/desktop/src/screens/masternodes/*.tsx` |
| Styles lifted from the mocks (`mg-*`) | `apps/desktop/src/styles/app.css` |
| Fixture: governance state, handlers, scenarios | `apps/desktop/src/fixture/governance.ts`, `src/fixture/scenarios.ts`, `src/fixture/governance.test.ts` |
| Playwright (flows, axe, light and dark, Chromium and WebKit) | `e2e/governance.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on regtest (GL) | `e2e-tauri/regtest-governance.ts`, `scripts/e2e-regtest-governance.sh` |

## Screen inventory

| Part | What it shows | Spec |
|---|---|---|
| Sidebar item | **Masternodes**, last; badge = own proposals that need the user (D5) | §4.25.1, MG-00 D1, D8 |
| Settings ▸ General | **Show Masternodes** toggle (default off) | MG-00 D2 |
| Page frame | `TopIntro` "Masternodes", More and Create proposal; the list height line | §4.25.1 |
| Status bar | "Nov 4 votes: about 1 d 6 h left" while the cutoff is ahead and the wallet holds an eligible voting key (D7) | §4.25.1, MG-00 D19 |
| Clock card | time left to the estimated cutoff, next payout and block, the cycle bar with the cutoff mark | §4.25.3 |
| Budget card | "Budget for <date>", "from peers, an estimate", passing / doesn't fit bar and lines | §4.25.3, MG-00 D45 |
| Toolbar | search, segmented All · Needs my vote · Mine, Sort | §4.25.3 |
| Proposal rows | name, amount × payments, "You: …", vote bar with the funding line, chip, margin | §4.25.3 |
| Detail pane | chip and note, Asks for, Paid to (copy), Link (D9), weighted votes and range, Your masternodes, Vote…, More | §4.25.3 |
| Vote sheet | outcome, Vote with (one row per voting masternode), privacy warning, note, passphrase, Vote <outcome>; then the result | §4.25.4 |
| Create proposal | 3-step wizard: Details, Payments, Review and pay the fee | §4.25.3 |
| Mine | own proposals with MG-06's states, [Submit] per MG-06 D24, the backups note | §4.25.3, MG-06 D24 |
| Governance numbers… | This cycle, Votes and budget | §4.25.3, MG-00 D20 |
| States | syncing with progress, no voting key, failed, disabled (sync off, D6), empty, no match, watch-only, locked | §4.25.1, §4.25.3 |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | MG-09 builds §4.25.1's frame: the route `masternodes`, the sidebar item last, Settings ▸ General ▸ **Show Masternodes** (`ui.showMasternodes`, default off) and the shared states. Until MG-10 adds **My masternodes**, the page has no tab bar: its title stays "Masternodes" and its body is the Governance tab. MG-10 adds the tab bar and `ui.masternodesTab`. | A tab bar with one tab is a dead control (§4.25 "no dead buttons"); keeping the frame's title and route means MG-10 only adds the tab. |
| D2 | No new dw-app command. Every read and action is a `governance_*` command bound by BL-232 (`api/generated/commands.ts`). | The brief: one path into the governance engine; the binding already covers §2.1–§2.5 but `relay_vote` (console only, MG-02 D93). |
| D3 | Sidebar visibility (MG-00 D1) uses what dw-app exposes: the setting (4), an own proposal that is not `expired` or `fee_failed` (3, "a proposal in progress"), and a collateral-shaped coin, an unspent output of exactly 1,000 or 4,000 Dash (2). Masternode roles and tracked masternodes (1, the rest of 3) need MG-03's `Masternodes` facade, which dw-app does not bind yet; MG-10 binds it and adds them. Until then a wallet that holds only a masternode's voting key shows the item through the setting. | The coins are the wallet's own, from the existing `utxos` query (`keys.coinsAll`, re-read on `balances` and `historyChanged`), so a spent collateral hides the item again; no new engine call. MG-10 owns the `Masternodes` binding (ROADMAP MG-10 depends on MG-03); binding it here would be MG-10's work in MG-09's PR. A voting-key holder can find the toggle; nothing is lost but the automatic item. |
| D4 | The vote sheet lists `voting_masternodes(wallet, proposal)`: the tip list's masternodes whose voting key the wallet's keychain derives (indexes 0–19). A row is "Masternode `1a2b3c4d…`" (the ProTx hash's first 8 hex), "Evonode" or "Masternode", "key in this wallet · voting key <n+1>", weight, last vote seen. The mock's "key you added", "Its voting key is not in this wallet" + Add key and "finding its collateral first" rows, names and service addresses need MG-03/MG-10 data (tracked masternodes, labels, the list's service) that the binding does not carry; they come with MG-10. | The brief: "lists only the wallet's masternodes whose voting key the vault holds"; `VotingMasternode` carries the ProTx hash, type, weight, validity, key index and the votes seen, nothing else. |
| D5 | Badge (MG-00 D8, MG-09's part): the count of own proposals that need the user, the ones whose [Submit] follows a failure: `ready_to_submit{no_peer_accepted}`, and `fee_unsent` with a `last_error`. Unvoted proposals never badge. | MG-00 D8 "a proposal whose submission failed"; MG-06 D24 says which states show [Submit], and these two are the ones a failure put there. |
| D6 | Governance sync is off by default (mg-engine §2.1). The Disabled state ("Governance data is off for this network.") gets a **Turn on** button (`governance_set_sync_enabled(true)`), and turning **Show Masternodes** on asks the same once ("Also load governance data from the network?"). | MG-00 shows the Disabled state but no way out of it, and no other screen sets the engine's setting. |
| D7 | Status-bar item: shown while `clock.voting_open` and the wallet holds an eligible voting key, known from the vote rows of the current proposals (D8): any row `valid`. The item reads the same `proposals` and `voting_masternodes` queries as the list (shared cache), only while the Masternodes item is shown (D3); with no proposals read yet, it is not shown. It opens Masternodes. | The engine reports voting keys per proposal only (`voting_masternodes`). Showing the deadline to someone who can't vote is the noise D19 avoids. |
| D8 | "You: …" in rows, "Needs my vote" and the detail's Your masternodes read `voting_masternodes` per listed proposal (one query per hash, under `["governance", …]`, re-read on each `governance` event), only once synced (the rows exist only then). A proposal needs my vote when it is open and at least one valid row has no vote seen. A shared key (`key_holders > 1`) reads "Last vote seen with this shared key: Yes (2 masternodes use this key)", never as each masternode's vote (mg-engine §2.4). | The engine has no per-wallet vote summary; a proposal list is tens of rows (mainnet about 40), and the call reads memory and the keychain's derived ids. |
| D9 | The proposal Link is shown as text with Copy, not opened. | The renderer has no URL opener (no shell or opener plugin), and adding one is a shell permission change outside this task. dash-qt's External Link Warning defaults to No, so copying is the safe equivalent. Follow-up F1. |
| D10 | Status chips (MG-00 D17) and Too close to call (D45): `close_call` replaces Passing, Over budget, Needs votes and Not funded with the neutral chip and shows the range. The margin is `net_yes_estimate − passing_threshold` ("about +414 over", "about 692 short"); its range is net (yes.min − no.max … yes.max − no.min) minus the threshold. | MG-00 §4.25.3; the tally's `WeightRange`s are the engine's bounds (mg-engine §1). |
| D11 | The one-hour rule (§4.25.4): a row whose own vote seen is younger than 3,600 s is disabled with "Voted Yes 40 min ago · can change after 4:12 PM", using the vote's own time. A `valid: false` row is disabled "PoSe banned: a banned masternode can't vote". | Core refuses an update within `GOVERNANCE_UPDATE_MIN`; the wallet checks against the vote's time (MG-02 D37). |
| D12 | Vote: one `Governance` grant from the inline passphrase (MG-05), then `governance_vote` with the checked ProTx hashes; the result lists each `VoteCast` in order: "Sent" with its peers, or the `rejected` reason as a sentence (D13). A whole-call error stays on the sheet with its sentence; the passphrase is asked again. | MG-00 D14 and MG-05 D6. |
| D13 | Error sentences: every `governance.*` code, each `vote_rejected` reason and each per-masternode `rejected` value maps to a sentence in `model.ts` (never a raw code); unknown values fall back to "The network refused the vote." | §4.25.4 "Engine governance.* codes map to sentences". |
| D14 | Create proposal: Details (name, link), Payments (first payment from `upcoming_superblocks(12)` with `cutoff_soon` preselecting the next one, count 1–12, amount, total, Pay to: a new receive address of this wallet or another address), Review (`preview_proposal`: the data JSON, fee and network fee). [Pay 1 Dash and create] asks a `Spend` grant capped at `fee + network_fee` from the preview, then `create_proposal`. `invalid_proposal{fields}` sends the user back to the first failing field's step with each field's message. | §4.25.3; MG-06 D10 caps the grant at 1 DASH plus the network fee. |
| D15 | Mine follows MG-06 D24: [Submit] shows for `fee_unsent`, `fee_paid{0}` and `ready_to_submit{no_peer_accepted}`, with `last_error`'s sentence. The mock's "Fee paid 0 of 1" row has no [Submit]; D24 (later, and written for MG-09) adds it, because a fee seen but not mined may need its saved bytes sent again. | MG-06 D24 is the engine's rule for when `submit_proposal` acts. |
| D16 | Mine states: `fee_unsent` "Fee not sent" (warning); `fee_paid{n}` "Fee paid" n of 1; `ready_to_submit` "Ready to submit" (warning) with its reason (`not_synced`: "Waiting for governance data to load", `sync_disabled`: "Governance data is off. Turn it on to submit." with Turn on); `confirming{n}` "Confirming" n of 6; `public{status}` the public chip (D10; no status yet: "Submitted"); `fee_failed` "Fee failed" (danger) "The fee transaction was rejected or replaced, so this proposal can't be submitted."; `expired` "Ended". | MG-00 §4.25.3 names the first ones; the rest follow MG-06's states. |
| D17 | A slow [Submit] shows "Sending…" on the button and keeps the row; `governance.submit_refused{in_flight}` reads "The fee is being sent. Try again in a moment."; `send.outpoint_unavailable` names the coin. | MG-06 D24. |
| D18 | Locked and watch-only (§4.25.1): a locked wallet browses and gives the passphrase on the sheet; a watch-only wallet shows the info notice, Vote and Create proposal are disabled with "Voting needs the wallet that holds the voting keys" and "Creating a proposal needs a wallet that can sign transactions". | §4.25.1, §4.25.4. |
| D19 | No "Add a voting key" button in the no-voting-key notice. | It opens Track ▸ Add its keys, which is MG-10's sheet. |
| D20 | dw-app gains no command but two shell fields: `Route::Masternodes` (the route, `shell/mod.rs`) and `UiSettings::show_masternodes` (default false, `settings.rs`, serde-defaulted so older files read). Bindings regenerated (`Route.ts`, `UiSettings.ts`). | The route and the settings file are dw-app's (U-01, U-04); the renderer can't add either alone. |
| D21 | The vote and create sheets ask for the passphrase as Pay's confirm sheet does (`credentialPrompt`: the lock and "Require authentication for every payment"; a vault that asks anyway shows the field; a wrong passphrase holds the button for the vault's wait), shared in `screens/masternodes/credential.tsx`. | MG-05 counts a `Governance` grant with the paying purposes (the vault's credential rule), so the same policy applies; one prompt pattern across sheets. |
| D22 | Mine is the list's third filter, as the mock shows; with sync off it shows under the Disabled notice, since own proposals work without governance data (mg-engine §2.5). | MG-00 `gov-mine`; MG-06 D24's Submit of `fee_unsent` needs no sync. |
| D23 | The pane's "Asks for" span is the proposal's own start and end dates (`start_epoch`, `end_epoch`), not its payout superblocks. | `ProposalRow` carries the epochs; the superblocks a proposal is paid at depend on votes each cycle. |
| D24 | The page's search, filter, sort, selection and Show ended proposals are page state: they reset when the route changes. | No link or menu opens a proposal yet; keeping them in `ui.ts` would add shared-file state for nothing (siblings edit ui.ts). |
| D25 | More (page): Show/Hide ended proposals and Governance numbers…; More (pane): Copy link, Copy proposal hash. A small menu button (`MoreMenu`, arrow keys, Escape) in `screens/masternodes/common.tsx`, styled from mg-00's `.mg-menu`. | The renderer had no menu component; MG-00 shows these Mores. |
| D26 | A notice's button (Turn on, Try again) is filled blue, not the mock's tinted blue; the vote result's "Not sent" chip is neutral, not warning. | Both mock colours fail axe's colour contrast in the light theme (tinted blue on the accent tint; warning text on the sheet's raised card). The sentence beside the chip says why it wasn't sent. |
| D27 | Resume (§4.25.3) is Mine's: once the fee is paid the engine owns the proposal (MG-06), so Mine lists it and its Submit or Turn on resumes it after a restart. The wizard keeps nothing before payment: closing it drops the unsent form. Amounts in the wizard's sentences, the Pay button and Governance numbers are compact (2 decimals); the Total and the review's fee are exact. | A paid proposal already survives a restart in the engine; a second, UI-side draft store would be a second truth for the same proposal. An unpaid form costs nothing to retype. Eight decimals in prose ("6018.00000000 tDASH") read as noise; where the user checks the money, it is exact. |

## Follow-ups

| # | Follow-up |
|---|---|
| F1 | A URL opener with dash-qt's External Link Warning (default No) for proposal links (D9). |
| F3 | BL-style: when MG-10 binds MG-03, vote rows and "Your masternodes" swap "Masternode 1a2b3c4d" for the masternode's label and service address (D4). |
| F2 | MG-10: the tab bar and My masternodes, the `Masternodes` binding, role-based sidebar visibility (D3), vote rows for tracked masternodes, names and service addresses (D4), Add a voting key (D19), the masternode half of the badge. |
