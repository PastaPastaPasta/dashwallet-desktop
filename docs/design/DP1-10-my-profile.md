# DP1-10: My Profile, username request status and top-up (Tauri)

ROADMAP DP1-10 on the Tauri renderer: [DASHPAY](DASHPAY.md) F5 ("iOS model; contest watch …; the temporary name is the
main name until the contest resolves"), F7 ("chip → sheet"), F8 ("top-up sheet with both [iOS presets and fee reserve;
Android's action estimate and low-credit warnings]; inline low-credit warnings"), and §4's `MyProfileViewModel` ("the
profile sheet": `identity_detail`, `refresh_balance`, `profile`), `UsernameRequestStatusViewModel` ("tallies, deadline,
temporary name": `contest_status`, `register_name`) and `CreditsViewModel` ("top-up presets 0.05 / 0.1 / custom ≥ 0.01,
'≈ N contact requests'": `cost_table`, `top_up`). It also takes DP1-08's F2 (the contest outcome rows) and D12 (the
contested chip opens My Profile), and DP1-09's F1 (the instant username beside a contested one). Size M, tier T0;
acceptance is the state inventory below in light and dark, with axe, on Chromium and WebKit.

The engine is merged: DP1-04's `contest_status` (m4 §2.3), DP1-06's credits calls (§2.8), DP4-01's `profile`, DP1-05's
`identities` (balance, `credit_level`, names). dw-app exposes them (E0-13). This task adds **one engine read**
(D3): the wallet's top-up funding steps whose outcome is unknown, so Repair can list and resolve one after a relaunch.
Copy follows the mobile wallets (Android `strings-dashpay.xml` at v11.9.1, iOS as DP1-08 cites it; the low-credit copy
is m4 item 44's, Android's), adapted (D-rows). Out of scope: Edit Profile and avatars (DP4-03), the Identities page and
choosing the main name (DP6-01), withdrawals (DP6-02).

| Part | Where |
|---|---|
| Engine read `stuck_top_up_fundings` + its contract (m4 item 54, Contract-Version 28), dw-app command, dwcli `identity top-up-stuck` | `dw-engine/src/platform/{credits,topup}.rs`, `lease/mode_b.rs`; `dw-app/src/api/identity.rs`, `dto`; `dwcli/src/dashpay.rs` |
| View models (pure TypeScript): the page, the credits block, the request status, the outcome rows, their copy | `apps/desktop/src/dashpay/profile.ts` |
| The top-up flow (amount, quote, confirm, outcome; one per wallet, kept across navigation) and its copy | `apps/desktop/src/dashpay/topup.ts` |
| Their tests, by parity ID (IOS-068, IOS-069; AND-071, AND-078) | `src/dashpay/{profile,topup}.test.ts` |
| Queries (keys under `["dashpay", …]`) | `src/dashpay/profileQueries.ts` |
| My Profile page, the top-up sheet, the Settings card | `src/screens/dashpay/{MyProfile,TopUpSheet}.tsx` |
| Shared files (additive): `Backend` and its Tauri binding, the route, the chip, the Home card's low-credit state, Repair's stuck list, Settings ▸ General, the fixture's handlers, scenarios and styles | `api/{backend,tauri}.ts`, `app/App.tsx`, `screens/Routes.tsx`, `components/shell.tsx`, `dashpay/status.ts`, `tools/repair.ts`, `screens/tools/Repair.tsx`, `screens/settings/General.tsx`, `fixture/{handlers,dashpay,scenarios,tools,topup}.ts`, `styles/app.css` |
| Playwright (flows, axe; light and dark; Chromium and WebKit) | `e2e/profile.spec.ts`; baselines in `e2e/visual.spec.ts` |

## Bindings

| Call | Used for |
|---|---|
| `dashpay_status`, `identities` (the shell's queries) | which identity the page shows; names, balance, `creditLevel`, `balanceStale`, profile |
| `identity_refresh_balance(identity)` | once per page visit (D9); its answer arrives as `Platform{Identities}` |
| `credits_cost_table` (DP2-09's query) | "≈ N contact requests · M profile updates"; the custom amount's floor |
| `contest_status(identity, label)` | the request card while the status is `contestPending` (D11) |
| `dashpay_events(identity, null, 50)` | the outcome rows: the newest contest outcomes (D12) |
| `credits_pending_top_ups` | pending top-up rows: Finish, "Credits arrived" (D15) |
| `credits_stuck_top_up_fundings` (new, D3) | stuck rows on the page and in Tools ▸ Repair |
| `credits_top_up_quote(identity, duffs)` | the confirm step; refusals before Confirm |
| `credits_top_up(identity, duffs, grant)` | Confirm & pay, under the quote's grant through U-05's `GrantModel` |
| `credits_resume_top_up(txid, grant)`, `credits_resolve_top_up(txid)` | Finish, "Credits arrived" |
| `resolve_stuck_funding(step, grant)` (Repair, U-05 D12) | a stuck top-up's "send all coins to yourself" |
| `register_name` through DP1-09's credits path | "Create instant username" (D13) |

Re-reads: every `Platform` signal re-reads `["dashpay", …]` (events.ts), which holds every key here; a
`dispatchResolved` signal re-reads the stuck top-ups, and so does a resolution in Tools ▸ Repair (D16); a balance change
re-reads the top-up quote, which sits under DP1-09's `quote` key (D18).

## State inventory

**Page** (`page-profile`, route `profile`; `data-state` on `profile-body`). axe runs on every row in both themes on
Chromium and WebKit; the visual baselines (light and dark, Chromium) cover P3, P4, P5, C2–C4, C6, C6-unknown, C7,
R1, R1 with the instant offer, R3–R5 and the sheet's T1, T2, T4 (D26).

| # | Input | Page | Scenario |
|---|---|---|---|
| P1 | status unread | busy line "Loading your profile…" | — |
| P2 | status read failed | DP1-08's S2 card (Try again) | `dashpay-registered` + `fail=dashpay_status:none` |
| P3 | `noIdentity`, `registering`, `startupIncomplete`, watch-only | "You have no DashPay profile yet" + DP1-08's Home card for the state, with its action (D2) | `dashpay-no-identity`, `dashpay-registering` |
| P4 | `ready`, named identity | header (initials avatar, display name, @name, public message), usernames, identity id with copy, credits, outcome rows | `dashpay-registered` |
| P5 | `ready`, no name (`namesState` none / updating / unknown) | header with "No username" + Choose a username (DP1-09 credits path), "Updating…", "Username not loaded" | `dashpay-no-username`, `dashpay-names-updating`, `dashpay-names-unknown` |
| P6 | `contestPending` | P4 + the request card (R1–R6) above the credits | `dashpay-contest-pending`, `dashpay-contest-no-temporary` |

**Credits block** (`profile-credits`, `data-level`):

| # | Input | Shows | Top up | Scenario |
|---|---|---|---|---|
| C1 | balance known, `ok` | "Credit balance" in Dash (D6) + "≈ N contact requests · M profile updates" | enabled | `dashpay-registered` |
| C2 | `low` | C1 + warning "Your credit balance is low" / "Top-up your credits to continue making changes to your profile and adding contacts" | enabled, primary | `dashpay-credits-low` |
| C3 | `empty` | C1 + "Your credit balance is fully depleted" / "You can continue to use DashPay for payments but you cannot update your profile or add more contacts until you top up your credit balance" | enabled, primary | `dashpay-low-credits` |
| C4 | balance `null` | "Credit balance unknown" + "Dash Wallet reads it from the network." | enabled | `dashpay-credits-unknown` |
| C5 | `balanceStale` | C1 + "Updating…" beside the figure | enabled | `dashpay-credits-stale` |
| C6 | a pending top-up of the identity | row "Top-up of N Dash pending" + its line (D15) + Finish (or "Credits arrived" / Finish when `consumptionUnknown`) | disabled: "Finish the pending top-up first." | `dashpay-top-up-pending`, `dashpay-top-up-unknown` |
| C7 | a stuck top-up funding of the identity | row "The payment may have been sent" (`maybeSent`) or "The payment is being sent" (`willBeSent`) + recovery copy + Open Repair (D16) | disabled: the row's line | `dashpay-top-up-stuck` |
| C8 | discreet mode | the figure and the estimate masked (D7) | as above | `dashpay-registered` + discreet |

**Request card** (`profile-request`, `data-state`; IOS-068/135, AND-071): the own contest as `contest_status` reads it.

| # | Input | Card | Scenario |
|---|---|---|---|
| R1 | `open`, `endsAt` | "Voting – @label" / "Masternode owners are deciding whether you get this username." / "Results on <date>." + the tally (D11) | `dashpay-contest-pending` |
| R2 | `open`, no end | R1 with "We will notify you when voting ends." | `dashpay-contest-pending-no-end` |
| R3 | `won` (read before the status moved) | "@label is yours" / "The network approved your username." | `dashpay-contest-won` |
| R4 | `lost{winner}` | "@label went to another user" (Android `request_username_taken_results`); winner `null`: "Voting on @label ended with no winner" | `dashpay-contest-lost` |
| R5 | `locked` | "@label was blocked" / "The network voted to block this username." | `dashpay-contest-locked` |
| R6 | `contest_status` failed | R1/R2 from the status alone + "Could not read the votes." + Try again | `dashpay-contest-pending` + `fail=contest_status:none` |

Every open card says the temporary name: "Until then you are @temp." when the identity has one, else "Create instant
username" (D13). Links: "What is username voting?" (DP1-08's sheet).

**Outcome rows** (`profile-outcomes`, D12): the newest three contest outcomes from the bell's journal: "@x — yours"
(`contestWon`), "@x — went to another user" (`contestLost`), "@x — blocked" (`contestLocked`), each with its date.

**Top-up sheet** (`topup-sheet`, `data-step`; IOS-131, AND-078). Title "Top up credits".

| # | Step | Shows | Actions |
|---|---|---|---|
| T1 | `amount` | presets 0.05 and 0.1 Dash and "Other amount" (a field, ≥ 0.01 Dash, D5); under each, "≈ N contact requests · M profile updates" | Continue (`topup-next`), Cancel |
| T2 | `confirm` | "You have chosen to top up your identity. The following amount will be deducted from your balance." (Android `topup_confirm_message`); Amount, Top-up fee, Total; "Adds about N credits: ≈ … contact requests"; a refusal of the quote instead of the rows (D17) | Back, Confirm & pay (`topup-pay`) |
| T3 | `running` | "Adding credits…" + "The network confirms the payment first. This can take a minute. You can close this: My Profile shows the top-up." | (closable, D14) |
| T4 | `done`, `applied` | "Credits added" / "+N credits. Your balance is X." | Done |
| T5 | `done`, `proofWaiting` | "Payment sent" / "The network has not confirmed the payment yet. Your credits are added once it does; My Profile shows the top-up until then." | Done |
| T6 | `done`, `notApplied` | "The credits were not added yet" / "Your payment is safe. Finish the top-up from My Profile." | Done |
| T7 | `done`, `consumptionUnknown` | "Check your credit balance" / "The network says this payment was already used. If your credits went up, choose 'Credits arrived' in My Profile; otherwise Finish tries again." | Done |
| T8 | `refused` | the refusal's copy (D17); `platform.broadcast_unknown`: "The payment may have been sent" + the recovery copy + Open Repair (`dashpay-top-up-lost`) | Back (when the amount can change), Open Repair, Close |

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | My Profile is the `profile` route's page (`page-profile`), not a modal sheet; the top-up is a sheet over it. The chip (every identity state, the contested chip included), the Home card, Settings and the native menu route there. | U-01 laid the route out (menus and links reach it); the page holds state that outlives a sheet (a contest that runs for weeks, a top-up whose proof is pending), as DP1-09 D1 reasons for the wizard. DASHPAY's "sheet" is the mobile shape. |
| D2 | Without a main identity the page shows "You have no DashPay profile yet" and DP1-08's Home card for the state (`homeCard`), with the card's own action (Join, Continue, Unlock, Repair). | One state machine (DP1-08 D1); the user who opens My Profile from the menu before joining gets the way in. |
| D3 | **Engine read** `DashPay::stuck_top_up_fundings()` → `Vec<StuckTopUpFunding{step, identity_index, identity, state}>`: the wallet's `topup/<i>-<k>/funding` steps whose funding the gate holds as possibly sent (`MaybeSent`, `WillBeSent`), the very steps that refuse the identity's next top-up (`next_step`); a step leaves the list once it settles, a step Repair resolved once its self-spend is ChainLocked (`dispatchResolved`, D22); a top-up marker this build cannot read makes the read `platform.unavailable`. `platform.unavailable` until the dispatch journal is read. m4 item 54, Contract-Version 28; dw-app `credits_stuck_top_up_fundings`; dwcli `identity top-up-stuck`. | The brief's scope 3. A top-up's unknown funding is told only by `top_up`'s refusal (`platform.broadcast_unknown{artifact}`); after a relaunch nothing listed it, so Repair could not offer its resolution and the page could not say why Top up is refused. Registrations list theirs through `registrations` (DP1-09 D31); top-ups keep no rows (m4 item 44), so a read of the journal's steps is the minimal one. |
| D4 | The header uses the contacts' initials avatar (`Avatar`), the profile's display name (else the username), @name, the public message; "Edit profile" is a disabled button with "Editing your profile comes with a later version." | DP4-03 owns Edit Profile and avatars; the mobile header's place for it is kept. |
| D5 | Top-up presets 0.05 and 0.1 Dash; a custom amount of at least 0.01 Dash, or the cost table's `topUpMinDuffs` if that is higher, typed in the display unit (`parseAmount`). | DASHPAY §4 `CreditsViewModel`; the engine's floor wins if it rises. |
| D6 | Credits are shown as their Dash value (`credits ÷ creditsPerDuff` duffs, the display formatter), labelled "Credit balance", with Android's action estimate as "≈ N contact requests · M profile updates" (`floor(balance ÷ contactRequest)`, `floor(balance ÷ profileUpdate)` from the cost table). | Android and iOS show the balance in Dash; raw credits (10¹¹) read as noise. The estimate is Android's `buy_credits_estimated_items`. |
| D7 | Discreet mode masks the credit figure, the estimates and the top-up's "your balance is X"; amounts the user types or chooses stay. | Credits are the user's money; DP1-08 D20's rule. |
| D8 | The low and empty warnings are the engine's `creditLevel` with m4 item 44's copy (Android's, with Android's swapped titles corrected). Home shows a card for them too (`lowCredits`, below): "Buy credits" opens My Profile with the top-up sheet. | F8 "inline low-credit warnings"; the Home card is DP1-08's banner slot, quiet otherwise. |
| D9 | The page asks `refresh_balance` once per visit per identity; the summary's `balanceStale` shows "Updating…" meanwhile. A failed refresh shows nothing new (the engine retries in the background). | DP1-06: the engine already refreshes stale balances; a visit is when the user looks. |
| D10 | `identity_detail` is not used: it is still a stub (m4 §2.1), and the summary carries everything the page shows (revision and keys belong to the Identities page, DP6-01). | No engine change for fields the page does not show. |
| D11 | The tally: "Votes for you: N" (the `isSelf` contender), "Other requests: k, N votes" (the others, summed), "Votes to block: N", "Abstained: N"; an unknown count (`null`) reads "—". Read once per visit and on each Platform signal; no poll. | IOS-135 shows the tallies; the identities of other contenders are not anyone's username yet, so the card does not list them. |
| D12 | The outcome rows read the first page (50) of the main identity's events and keep the newest three `contestWon/Lost/Locked`. They do not mark anything read (the bell does). | DP1-08 F2 / D14: outcomes live in the journal, not the status. A contest older than 50 events is gone from the profile but still in the bell. |
| D13 | "Create instant username" opens DP1-09's credits path (`register_name`) in an instant mode that refuses a contested label: "An instant username needs a number 2–9 or 20 characters or more." The engine makes a plain name registered during the identity's own contest its temporary name. | DP1-09 F1; Android "Create instant username" on its voting screen. A contested label would start a second contest. |
| D14 | One top-up flow per wallet (`topUpFor`), kept while the app runs: the sheet can be closed while the call runs and reopened to its result; the page shows "Topping up…" meanwhile, then "Your top-up has finished." with Show (`profile-top-up-unseen`) until the result is seen. A result seen is dropped on Close; an unseen one is dropped when a pending row's call or Repair's resolution overtakes it. One call at a time: Confirm is held while the grant is asked for and the call runs. | No second asset lock from a double click (DP1-09 D8); a top-up waits up to the InstantSend window. |
| D15 | Pending rows: unproved "Waiting for the network to confirm the payment."; proved "Ready to add."; `consumptionUnknown` "The network says this payment was already used. Check your balance." Finish is `resume_top_up` under a `platformOp{maxDuffs: 1, maxCredits: 0}` grant (the apply's funding key, nothing spent; m4 §2.8); "Credits arrived" (`resolve_top_up`, no grant) only on a proved row Platform said was used: `consumptionUnknown`, or a call this session answered `ConsumptionUnknown` before the library marked the lock final (`answeredUnknown`). | m4 §2.8: `resume_top_up` needs `max_duffs ≥ 1`; `resolve_top_up` refuses an unproved lock. |
| D16 | A stuck funding (D3) is resolved only in Tools ▸ Repair, which lists it beside the registrations' ("A top-up of @name's credits"), and resolves it with `resolve_stuck_funding(step)`. The page and the sheet route there ("Open Repair"); no retry is offered. | E0-04: "a top-up's `broadcast_unknown` offers no retry" (H11); DP1-09's routing. |
| D17 | Refusal copy by code: `credits.funding_insufficient` "Not enough Dash: N needed[, N available]" (available hidden in discreet mode), `credits.below_minimum` "The smallest top-up is N.", `credits.top_up_pending` "Finish the pending top-up first.", `credits.coins_locked` "The coins to pay with are held by another payment.", `platform.broadcast_unknown` (D16), `platform.will_be_sent` "The payment is still being sent. Try again in a minute.", `platform.context_unavailable` "Waiting for the network to sync and verify this identity. Try again shortly.", `platform.trust_mismatch` "This identity's data could not be verified against the network. Nothing was paid.", `platform.unavailable` "Dash Platform did not answer. Try again.", `platform.signer_unavailable` "Unlock the wallet fully and try again.", `platform.grant_exceeded` "The cost went above what you approved.", else the code. | DP3-04's gate refuses top-ups with `context_unavailable` and `trust_mismatch` (its item 52): surfaced, not duplicated. |
| D18 | The quote's key is `["dashpay","quote",w,"topUp",identity,duffs]`, under DP1-09's `quote` key, so a balance change re-reads it (events.ts already does). | Its fee and shortfall move with the wallet's coins. |
| D19 | The top-up pays from the wallet's ordinary coins, as a registration does; the sheet offers no CoinJoin source and says nothing of mixing. | DP1-08 D9: the asset-lock funding spends the send account's coins only. |
| D20 | Settings ▸ General gains a "DashPay" card: "My Profile" (with the identity's name) when the wallet has a main identity, else "Join DashPay". | The brief's Settings entry; one additive card, no Settings structure change. |
| D21 | The contested chip opens My Profile (was the Join page, DP1-08 D12); the voting card's action stays the voting info sheet and gains nothing. | DP1-08 D12's handover; the Join page's voting card is unchanged. |
| D22 | The stuck top-ups' key (`["dashpay","stuckTopUps",w]`) is also re-read by `dispatchResolved` (a Repair self-spend ChainLocked), by Repair's confirmed resolution, and whenever a top-up or a pending row's call ends (`onTopUpSettled`, wired in `bridgeEvents`, so whatever screen is shown): those re-read `["dashpay"]`. | A settled funding is a wallet-side dispatch, not a Platform signal, and a funding left unknown sends no signal at all; a re-read tied to the page missed a call that ended after the user left it (review m2). |
| D23 | Test titles cite IOS-068 (username request status) and IOS-069 (the profile sheet and its top-up), the `docs/parity.md` rows for these screens. | The brief named no IDs; these are the rows that describe them. |
| D24 | The fixture's `topUpMinDuffs` stays 50 000; the renderer's floor is the larger of it and 0.01 Dash (D5), so the fixture exercises the 0.01 Dash floor. | The engine's own floor is 0.01 Dash (DP1-06); the cost table's value only ever raises it. |
| D25 | Load flakes recorded at load 120–140: the visual `profile-credits-low light` ("two consecutive stable screenshots") and `txDetail light` (1 759 px) each passed alone on rerun; so did dw-engine's `trust_tests::after_masternode_sync_the_degraded_mode_falls_back_at_once` (its 2 s wall-clock bound, 3/3 alone). | The brief's rule: rerun alone, record, don't blame the code. |
| D26 | Visual baselines cover the rows that differ in layout (the inventory's list); the rest differ in copy only and are asserted by `profile.spec.ts` with axe in both themes. Credits baselines wait on the estimate, the last line drawn. | 38 baselines already; copy-only rows add review noise, not coverage. The wait removes the D25 credits flake's cause (review m3). |
| D27 | The fixture settles a resolved stuck top-up at once, as it does DP1-09's registrations; the engine keeps it listed until the self-spend is ChainLocked, and D22's `dispatchResolved` re-read removes it then. Repair shows the outcome "You can top up again once it is confirmed." and keeps the Resolve button for a step still listed (a second Resolve is refused with `spend_unsettled`). | One fixture rule for every stuck funding; a resolved-and-waiting row in Repair belongs with the registrations' (follow-up). |
| D28 | The fixture refuses in the engine's order: the quote's checks (below the minimum, a pending lock, the wallet's coins), then for `top_up` a stuck funding before the grant. `dashpay-top-up-lost` leaves the next funding unknown. | m4 §2.8; the page holds Top up for a stuck funding, so only `top_up` meets the refusal (review nit). |
| D29 | A known balance before the cost table is read shows its credits ("N credits") and no estimate, never "Credit balance unknown". A failed stuck top-up read in Repair says so (`repair-stuck-top-ups-unreadable`) unless the registrations' read failed too. | Review m3, m6. |

## Home card addition

| # | Status (input) | Card `data-state` | Title / message | Action | Scenario |
|---|---|---|---|---|---|
| S19 | `ready`, named, `creditLevel` `low` | `lowCredits` | "Your credit balance is low" / "Top-up your credits to continue making changes to your profile and adding contacts" | Buy credits → My Profile, top-up sheet open | `dashpay-credits-low` |
| S20 | `ready`, named, `creditLevel` `empty` | `emptyCredits` | "Your credit balance is fully depleted" / "You can continue to use DashPay for payments but you cannot update your profile or add more contacts until you top up your credit balance" | as S19 | `dashpay-low-credits` |

## Slice plan

1. This document.
2. The engine read, its contract, dw-app command, generated types, dwcli command, tests.
3. `Backend` bindings; the view models and the top-up flow with their tests.
4. The fixture: credits handlers, contest status, stuck fundings, Repair; scenarios.
5. My Profile, the top-up sheet, the chip, the Home card states, Repair's list, the Settings card.
6. Playwright flows + axe; visual baselines; `docs/parity.md` rows.

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | Edit Profile, avatars | DP4-03 |
| F2 | Choosing the main name among several; other identities | DP6-01 |
| F3 | Withdraw credits | DP6-02 |
| F4 | Fiat beside the Dash amounts | the price feed task |
