# MG-00: Masternodes sidebar item — UX spec, mocks and decisions

ROADMAP MG-00 (release 1.1, DEC-01, DASHPAY §5a). The spec itself is **UX-SPEC §4.25**; it replaces §4.22–4.24,
which now point to it. This file holds the mock index, the decisions (D-rows) the spec cites, the
follow-ups for MG-01…MG-13, and the questions left for pasta. Design only: no renderer code changes.

The look follows the DashPay screens (U-02…U-05, DP2-07), and the flows follow mobile where mobile has them:
iOS `MasternodesScreen` for my and tracked masternodes, keys, credit withdrawal and the voting privacy note.
Neither mobile app has governance proposals, registration or shared masternodes, so those screens are new. They
are built from the same parts: split view, cards, sheets, wizards and inline passphrase. Facts come from the
parked branches (`m3/r2-governance`, `m3/r3-protx`) and from Dash Core v24.0.0-rc.3 (`0364ed626e`). They are
the protocol limits, fee, vote rules, PoSe and shared-masternode rules the copy relies on.

| Part | Where |
|---|---|
| Spec | `docs/design/UX-SPEC.md` §4.25.1–4.25.8 |
| Mock sources (one file per board, frames in HTML comments) | `docs/design/ux/mg-00/src/{masternodes,governance,registration,maintenance,shared}.html` |
| Mock-only styles (renderer tokens only) | `docs/design/ux/mg-00/mg-00.css` |
| Built boards (self-contained: fonts, tokens, `app.css`, icons inlined; `?theme=dark`, `#frame`) | `docs/design/ux/mg-00/*.html`, built by `python3 -I docs/design/ux/mg-00/build.py` |
| Screenshots, 1280 × 820, light and dark | `docs/design/screenshots/mg-00/<frame>-{light,dark}.png`, rendered by `node docs/design/ux/mg-00/render.mjs [board…]` (Playwright from `apps/desktop`) |

The boards wrap each frame in the real shell markup (`shell.tsx`'s sidebar, toolbar and status bar) and reuse
`app.css` classes: sheets, tabs, segmented controls, chips, buttons, detail lists, Contacts' split view and
loading states. New parts are prefixed `mg-` and use only generated tokens. MG-09/10 can lift them into `app.css`
as they are, or map them to components.

## Mock index

| Frame | Shows | Spec |
|---|---|---|
| `mn-list` | My masternodes: summary, groups (Needs attention, My masternodes, Tracked), detail pane with roles | §4.25.2 |
| `mn-empty` | Empty page, reached from the setting | §4.25.2 |
| `mn-banned` | A PoSe-banned masternode, Retired disclosure, the More menu with its notes | §4.25.2, §4.25.7 |
| `mn-operator` | The operator-only explanation sheet | §4.25.7 |
| `mn-track` | Track a masternode and attach keys | §4.25.2 |
| `mn-keys` | Masternode keys (IOS-083), reveal behind the passphrase | §4.25.2 |
| `mn-withdraw` | Withdraw evonode credits (MG-13) | §4.25.2 |
| `gov-list` | Governance: clock card, budget, filters, proposal list, detail pane (mainnet) | §4.25.3 |
| `gov-syncing` | Proposals loading; a wallet without voting keys | §4.25.3 |
| `gov-vote` | Vote sheet: four custody/eligibility cases, privacy warning, passphrase | §4.25.4 |
| `gov-vote-result` | Partial result with a per-masternode reason | §4.25.4 |
| `gov-create` | Create a proposal, step 2 (payments, closing-soon rule, budget fit) | §4.25.3 |
| `gov-create-review` | Step 3: review, the verbatim fee warning, passphrase | §4.25.3 |
| `gov-mine` | Mine: fee paid (auto-submit pending), confirming | §4.25.3 |
| `reg-type` | Wizard step 1: type, who runs the server | §4.25.5 |
| `reg-collateral` | Step 2: from this wallet / existing output / another wallet | §4.25.5 |
| `reg-operator` | Step 3, hosted, with a network-mismatch field error | §4.25.5 |
| `reg-operator-evo` | Step 3, self-run evonode, keys from the keychain | §4.25.5 |
| `reg-keys` | Step 4: owner, voting, payout | §4.25.5 |
| `reg-review` | Step 5: review and passphrase | §4.25.5 |
| `reg-prove` | Step 6 (external collateral): sign in the other wallet | §4.25.5 |
| `reg-done` | Sent, next steps | §4.25.5 |
| `upd-registrar` | Change keys or payout (ProUpRegTx), with the PoSe-ban warning | §4.25.6 |
| `upd-release` | Release collateral (ends the masternode when spent) | §4.25.6 |
| `mn-watchonly` | A watch-only wallet | §4.25.1 |
| `mn-locked` | A locked wallet with an external-collateral registration waiting for its signature | §4.25.1, §4.25.5 |
| `sh-start` | Shared masternode: start, join or continue | §4.25.8 |
| `sh-session` | A running session (coordinator, approvals stage) | §4.25.8 |
| `sh-close` | "Keep this session for later?" | §4.25.8 |
| `sh-detail` | A shared masternode in My masternodes | §4.25.8 |
| `sh-dissolve` | Dissolve alone in the early period | §4.25.8 |

The sample data is consistent per board. The masternode, registration, maintenance and shared boards use
testnet (`tDASH`, port 19999, `y…` addresses). The governance board uses mainnet (`DASH`, the 16,616-block
cycle, `X…` addresses). Block heights, dates and amounts are illustrative.

## §5a in/out review

Every screen was checked against DASHPAY §5a. The left column (owner side) maps to screens as follows:

| §5a (in) | Screen |
|---|---|
| Governance list and detail, voting one or many masternodes, creating a proposal (fee, confirmations, submit, resume), clock, superblock dates, budget | §4.25.3, §4.25.4 |
| My masternodes (collateral, owner, voting or payout key in the wallet), tracking and attaching keys, SPV status, payouts in history, evonode status and credits, credit withdrawal to the payout address | §4.25.2 |
| Registration wizard: regular and evonode; FundNew, ExistingUtxo, External with a signed message; operator public key, service address and Platform details as given; self-operator fills the operator key from the keychain | §4.25.5 |
| ProUpRegTx (voting, payout or operator key) | §4.25.6 |
| v24 shared masternodes: creation, reward-address change, key rotation, dissolution, standby | §4.25.8 |
| Optional contested-name voting (MG-07) | not specified here; MG-11 reuses §4.25.4's rows (follow-ups) |

The right column (out) appears only as explanation:
- ProUpServTx (service update and Unban) and ProUpRevTx (revoke) are one explanation sheet. There are no forms and no disabled lookalike buttons (D24).
- No operator BLS secret is generated or held outside the keychain reveal (D27).
- Platform node operation and node administration don't appear at all.

One flow sits on the line and stays in: **Release collateral** (D38). It sends nothing. It unlocks the
collateral coin locally, and spending that coin is an ordinary payment that ends the masternode by Core's rules.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | Show the **Masternodes** sidebar item when any of these holds: the wallet has a masternode role (collateral, owner key, voting key, payout script, share refund: Core's `protx list wallet` set, from `Masternodes`); it holds an unspent output of exactly 1,000 or 4,000 Dash; it tracks a masternode or has a registration, shared session or proposal in progress; or the setting of D2 is on. | DASHPAY §5a: "appears when the wallet holds masternode keys or a collateral, or in advanced mode". A collateral-shaped coin counts as "a collateral" so a holder finds the wizard without a setting. In-progress items must stay reachable. |
| D2 | Add one toggle, **Settings ▸ Advanced ▸ Show Masternodes** (default off), instead of a general "advanced mode". | No advanced mode exists in the app (U-04). dash-qt has separate per-tab toggles (`fShowMasternodesTab`, `fShowGovernanceTab`), and one item needs one toggle. A general mode would hide unrelated things for one feature. |
| D3 | Use one sidebar item with two tabs, **My masternodes \| Governance** (route `masternodes`, tab remembered per window as `ui.masternodesTab`), placed after CoinJoin. No separate Governance item. | §5a names exactly this. Most governance users are masternode owners, and one item keeps the sidebar to the mobile size. |
| D4 | Lay out My masternodes as Contacts' split view (list + pane) with the groups Needs attention · In progress · My masternodes · Tracked · Retired (collapsed); empty groups are hidden. | DP2-07's pattern; iOS sections own / Tracked / Retired. "Needs attention" puts the one actionable case (a ban) first. |
| D5 | Status chips: `Active`, `PoSe banned`, `Retired`, `Unknown`, `Registering`, `Needs signature`. No "next payment" chip or column. | The SPV list knows valid and banned; the last two are the wallet's own in-progress states. iOS uses the same first four. |
| D6 | Each tab has one primary header action, **Set up a masternode** or **Create proposal**. Everything else goes in the tab's More menu: Track a masternode…, Masternode keys…, Join a shared masternode…; Show ended proposals, Governance numbers…. | It's the mobile primary-action pattern, and it keeps rarely used tools out of the header. Joining a shared masternode starts from a paste, not the wizard. |
| D7 | Masternode keys (IOS-083) opens from Masternodes ▸ More as a sheet, not from Settings or a Window menu. MG-10 builds it in the renderer. | The Tauri renderer has no IOS-083 screen yet (it exists only on the Swift side), and keys belong next to the masternodes that use them. |
| D8 | The sidebar badge counts only items that need the user: a banned masternode the user owns, a registration or shared session waiting on this user, a proposal whose submission failed. Unvoted proposals never badge. | A badge for every open proposal would be permanent noise, about 30 a month on mainnet. |
| D9 | Show only what the SPV list and wallet history know. Full-node values (PoSe score, last-paid height, next payment, ban and revive heights) are "— (needs a full node)". Ban dates read "noticed Oct 12": the wallet's first sighting. Nothing is estimated. | Honest data (UX-SPEC rule, R3 "Requires full-node data source"). A wrong next-payment estimate is worse than none. |
| D10 | Payouts come from this wallet's history. "Payouts in 30 days" links to Activity filtered to the masternode's payouts. | §5a "payouts attributed in history". A payout to an outside address is invisible, and the copy says so (reg-keys "This wallet then can't show the payouts"). |
| D11 | The vote sheet covers one proposal; it opens from the proposal's detail pane or row. There is no voting on several proposals at once. | It matches mobile's one-decision sheets and keeps the outcome unambiguous. dash-qt's vote window is also per proposal. |
| D12 | Preselect every eligible masternode and show the privacy warning when two or more are checked: "Voting with several masternodes at once shows the network that one person controls them. Choose fewer to keep them unlinked." | iOS warns that nodes voting together are publicly linked. Most owners vote with everything; the warning lets the rest opt out. |
| D13 | Voting-key custody has three cases: "key in this wallet" (keychain), "key you added" (tracked, encrypted in the vault, IOS-082), and "Its voting key is not in this wallet" + Add key. Tracking searches by IP or ProTx hash; keys are attached after the masternode is found and are checked live against it. | Finding by private key (iOS) mixes search with secret entry. Checking each key against the found masternode catches wrong keys before they're stored. |
| D14 | Every signing flow ends with the inline Passphrase field on its final step, as Pay's ConfirmSheet does. dash-qt's countdown confirm and separate unlock dialogs go. Votes need no further confirmation; the button names the outcome ("Vote Yes"). | It's one pattern across the app (U-03), and the grant is single-use per action (E0-04). Votes cost nothing and can be changed after an hour. |
| D15 | Copy uses plain words: "server", "operator", "operator's share", "payouts", "collateral", "evonode". It avoids dash-qt's "service", "operator reward", "payee", "ProRegTx" except where the user must match a CLI or Core term (`protx update_service`, "ProTx hash", "PoSe banned"). | It's the mobile tone (DASHPAY), and these screens are for holders, not operators. |
| D16 | Proposal fee then submit is one action. The wallet submits the proposal itself at the fee transaction's first confirmation, retrying while the app runs. Mine replaces dash-qt's Resume window and shows `Fee paid 0 of 1`, then `Confirming 4 of 6`, then the public status; [Submit] appears only after a failure. | Submitting needs no signature, so a manual step adds only a way to lose the 1 DASH fee. Core accepts a submit at 1 confirmation, and peers relay it at 6. |
| D17 | Proposal chips map from the engine status: Passing → Passing, Unfunded → **Over budget**, Voting → **Needs votes**, Failing → **Not funded**, Funded → Funded, Confirming → Confirming, Pending → Ready to submit / Fee paid (Mine only), Lapsed → **Ended** (hidden by default). | "Unfunded" and "Voting" mean nothing to a holder. The new words say what to do or what happens. |
| D18 | Save the proposal and its fee transaction in the wallet before the fee is broadcast, and include them in wallet backups. | Core: losing the prepare parameters burns the fee. A crash between fee and submit must be recoverable. |
| D19 | The governance clock becomes a card on the Governance tab: the full cycle with the voting cutoff marked, next payout date and block. The dash-qt moon-phase status-bar icon goes. The status bar shows the short text "Voting closes in 1 d 6 h" only while voting is open and the wallet holds an eligible voting key. | QT-026 adapted: the deadline matters only to someone who can vote, and a cycle bar explains it better than a moon phase. Cross already uses text items. |
| D20 | The governance numbers of dash-qt's info panel (QT-134) go in a **Governance numbers…** sheet from Governance ▸ More. It shows cycle, last and next superblock, cutoff, participation weights, threshold, budget and sync state. There's no Tools ▸ Information sub-tab. | Those numbers belong with governance. Tools stays dash-qt's technical views (U-05). The clock and budget cards already carry the everyday part. |
| D21 | While proposals sync, the clock and budget total show (headers alone suffice), the list is a progress state with counts of objects and votes, no tally shows partly, and Create proposal stays enabled. | govsync phases. A partial tally read as final would mislead voters. Creating a proposal needs only superblock heights. |
| D22 | Tracked masternodes vote, and withdraw credits, when their keys are attached. **Stop tracking** confirms and then deletes the attached keys from the vault. | IOS-082. Keys added for a masternode leave with it, so no orphan secrets stay behind. |
| D23 | Credit withdrawal follows iOS: with the owner key the destination is fixed to the payout address; with the payout key in the wallet, any address. The sheet names the key used. | IOS-081 owner part; §5a "to the payout address"; the library's `masternode_withdraw`. |
| D24 | Operator-signed actions (service update, Unban, revoke) are one explanation sheet from More ▸ "Service update or revoke…" (menu note "Done by the operator on the node"). It lists the owner's alternatives: replace the operator through Change keys, or end the masternode through Release collateral. There are no disabled Update Service, Revoke or Unban buttons. | §5a right column. "No dead buttons" (D-row style of U-0x). An owner whose node is banned needs to know who acts, not a greyed button. |
| D25 | The wizard runs **Type and who runs it → Collateral → Operator → Keys and payout → Review**, plus **Prove the collateral** for External. The owner key always comes from this wallet. FundNew creates the collateral to a new wallet address in the registration transaction. The fee is paid from this wallet automatically, with no fee-source picker. Going back after the proof message is built means signing again. | §5a order, with the type first because it sets the collateral amount and the evonode fields. An owner key held elsewhere would make the masternode unmanageable from here. Core's external message includes the payload hash, so any change invalidates the signature. |
| D26 | The server address is required. | Core allows an empty address, but the masternode then starts PoSe banned until the operator sends a service update; a holder would see a "banned" masternode on day one. Operators who want that flow use the CLI. |
| D27 | No operator-secret save gate (dash-qt's QT-124 "type the last 4 characters"). Hosted: the operator public key is pasted (a pasted secret is refused). Self-run: the next unused keychain operator keypair fills the key, and the secret is revealed later in Masternode keys. | §5a: the wallet generates no operator secret outside the keychain reveal flow. QT-124 survives as the reveal gate only. |
| D28 | Registration and Change keys set one payout address. A masternode already registered with v24 multiple payees shows the split read-only ("Payouts split 3 ways") and changes it only through Change keys as one address. | v24 allows 1–8 payees, but §5a lists only "the payout key", and multi-payee editing is a separate design. It is a follow-up, not a dead control. |
| D29 | Keep dash-qt strings verbatim only for the two warnings with financial risk: "Creating a proposal pays 1 DASH to the network. This fee is non-refundable regardless of outcome." and "Changing the operator key immediately PoSe-bans the masternode." (followed by our plain explanation). Everything else is new or iOS copy. | UX-SPEC copy rule (research 02 wins for strings where dash-qt has the feature) balanced against §5a's "mobile-inspired". These two sentences are the ones a user must not misread. |
| D30 | The success chip (`Active`, `Approved`, `Passing`) is a green dot with primary-colour text on a success tint, not green text. | Green text on the success tint fails WCAG AA in both themes. |
| D31 | Shared sessions show the user-facing stages **Invite · Collect details · Lock terms · Approvals · Signatures · Send**. | R3's protocol rounds and dash-qt's stage names disagree. These describe what the user does at each stage, and participants see the same list. |
| D32 | A shared session persists until it is sent or discarded, with its coins reserved. Closing asks "Keep this session for later?" (Keep and close / Discard and release coins / Cancel). After registration the wallet prepares and stores a zero-penalty standby dissolution and offers Save a copy…. | Core advice: every participant should keep a standby dissolution. Losing a session silently would strand the others. Reserved coins must not be spent into a conflicting transaction. |
| D33 | Shared masternodes appear only when the engine reports v24 active on the wallet's network. They are masternode-only until `evo_shares` is active. When a deployment is inactive, the choice is disabled with "Needs the v24 network upgrade, not active on Mainnet yet." | Core v24.0.0-rc.3 ships v24 and `evo_shares` as NEVER_ACTIVE on mainnet; testnet activates them. The UI never assumes activation. |
| D34 | A shared masternode has one voting key, by default from the coordinator's wallet; the coordinator can set another address. Changing it, or the operator key, is **Change keys together**: a session every owner approves. | Core: one voting address per masternode; plain `update_registrar` is invalid for shared masternodes, and ProUpSharedRegTx needs every owner. |
| D35 | Lease names for the status bar's key-held tooltip: "Masternode registration", "Key change", "Voting", "Proposal", "Credit withdrawal", "Shared masternode", "Dissolution". | E0-04 §16.10 shows the flow by name ("Registration of @name holds a key for 4:12"). MG flows had no names. |
| D36 | Network mismatch is a field-level refusal with a sentence naming both networks, never a page mode. | Wrong-network input is the realistic case (a mainnet port, address, key or invitation). The wallet itself is on one network. |
| D37 | A watch-only wallet is read-only. It lists the masternodes whose collateral or payout it watches, and each signing action is disabled with its reason. A locked wallet browses everything and asks for the passphrase only at the final step. | It's the same treatment as the rest of the app (U-03). dash-qt's watch-only registration tooltip is kept. |
| D38 | **Release collateral** unlocks the collateral coin locally, with a danger notice and a checkbox, and sends nothing; it can be locked again until spent. It is the owner's way to end a masternode. | Core: spending the collateral deregisters it at once. The wallet auto-locks collateral, so ending needs an explicit, reversible unlock rather than a "revoke" the owner can't sign. |
| D39 | One core server address field. IPv6 and Tor are accepted where the network takes version-3 payloads, and additional addresses (Core allows up to four per purpose) are left to the CLI. | It covers the hosted case. The extra entries are operator concerns, and the engine picks the payload version (MG-04). |
| D40 | Mocks are static HTML built from the renderer's own tokens, `app.css`, fonts and icons (`build.py`), screenshotted with Playwright. They don't use the fixture app. | There's no MG code in the renderer yet. Built this way the mocks stay pixel-true to the current styles, and MG-09/10 can diff their screenshots against them. |

## Follow-ups for MG-01…MG-13

| Task | Follow-up from this spec |
|---|---|
| MG-01 | Keep the R2 `ProposalStatus` set and govsync phases (Disabled/Waiting/SyncingObjects/SyncingVotes/Synced/Failed) that §4.25.3 maps (D17, D21). |
| MG-02 | Expose for the clock card: last and next superblock heights with time estimates, the cutoff height, and cycle position. For the budget: total, allocated to passing, the part that doesn't fit, and per-proposal margin. The threshold must include the min-quorum floor. Per proposal: my masternodes' votes with time. Sync progress as counts (objects + votes, approximate total). The reason behind phase Disabled. |
| MG-03 | Role set = `protx list wallet` including share refunds (D1); detect collateral-shaped coins; persist the first time a masternode is seen banned (D9); tag payouts with the masternode for the Activity filter (D10); report the list height; persist tracked keys in the vault and delete them on Stop tracking (D22); report the payout split for v24 multiple payees (D28). |
| MG-04 | Require the service address (D26); take the operator's share as a percent with 2 decimals and convert it to basis points. Persist a prepared External registration (payload + message) across restarts (`Needs signature`). Refuse a pasted BLS secret where a public key is expected. Map `protx.*` refusals to codes the UI can phrase (duplicate operator key, duplicate address, key reuse across roles). Add **release / relock collateral** (D38). Pick the payload version, with v3 where required (D39). |
| MG-04b | Persist sessions with reserved coins (D32), session code and check code, and a network tag in every message (D36). Name the stages per D31. Auto-prepare and store a standby dissolution after confirmation. Give a dissolve preview (your return, the others' bonus split, fee from the actor's share). Report activation of v24 / `evo_shares` (D33). Decide and expose who pays the registration fee, then show it in the locked terms. |
| MG-05 | Use the lease names of D35. Use one grant per vote batch (N votes, one passphrase); a voting lease never signs a transaction. |
| MG-06 | Save before broadcasting the fee and include the drafts in backups (D18). Auto-submit at 1 confirmation with retry (D16). Keep every non-OP_RETURN output of the fee transaction P2PKH (Core rule: a P2SH change output invalidates the fee). Provide the next 12 superblocks with dates and a "voting closes within two days" flag for the payments step. |
| MG-07 / MG-11 | Contested-name voting gets a short spec of its own when MG-11 starts. Reuse §4.25.4's masternode rows (custody, weight, one-hour rule, privacy warning). Choices are the contenders plus Abstain and Lock instead of Yes/No/Abstain. |
| MG-08 | None from the UX. The CLI keeps Core terms. |
| MG-09 | Build §4.25.3–4.25.4: the status-bar item (D19), Governance numbers… (D20), Mine (D16) and the badge rule (D8). Visual baselines start from these mocks; lift `mg-*` styles into `app.css`. |
| MG-10 | Build §4.25.1, §4.25.2, §4.25.5–4.25.7: the Settings toggle (D2), sidebar visibility (D1), Masternode keys (D7), Track (D13), Change keys, Release collateral, the operator sheet (D24), and the Activity filter by masternode (D10). |
| MG-10b | Build §4.25.8, including the coordinator setup fields only outlined in the spec (participants, settings, exit terms, contribution) and the participant summary. |
| MG-12 | Review the paste-based shared messages (check codes, session binding, network tag, size cap), standby dissolution files (signed transactions on disk), the external collateral signature check, voting privacy (D12) and key deletion on Stop tracking. |
| MG-13 | Build the withdraw sheet with the key-dependent destination rule (D23). |
| parity.md (each task when it delivers) | QT-026 → adapted (D19); QT-134 → Governance numbers sheet (D20); QT-124 → reveal gate only (D27); QT-125 Update Service/Revoke → explanation sheet (D24). |

## Open questions for pasta

1. **Auto-show on a 1,000 or 4,000 Dash coin (D1).** Holding exactly that amount shows the Masternodes item
   without asking. The alternative is to require the setting, at the cost of discoverability for would-be
   owners.
2. **Server address required (D26).** This rules out "register first, the operator adds the address later",
   which some hosting providers might ask for. Core allows it, but the masternode starts banned.
3. **One payout address (D28).** v24 allows up to 8 payees per masternode. Is a multi-payee editor wanted in
   1.1, or later?
4. **Governance clock in the status bar (D19).** We keep a short text item only while voting is open and the
   wallet can vote. Dropping it entirely is the simpler choice.
