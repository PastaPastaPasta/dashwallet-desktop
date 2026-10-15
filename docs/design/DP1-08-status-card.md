# DP1-08: DashPay status card, identity chip, Join intro, FAQ and voting info (Tauri)

ROADMAP DP1-08 on the Tauri renderer: [DASHPAY](DASHPAY.md) F1 ("one banner state machine for the Home card, the
chip and the Contacts empty state; disabled until sync completes; real costs from the quote; CoinJoin privacy tip"),
§4's `DashPayStatusModel` (Home card, chip) and `JoinDashPayViewModel` (intro, readiness, FAQ, voting info), and
BL-153's follow-up F2 ("DP1-08 renders every banner state, `WaitingForSync` disabled Join included").
Acceptance: every banner state is rendered in light and dark.

The engine side is merged: BL-153's `DashPay.status` (m4 §2.1, rules in item 49), DP1-02's `registrations` and
`registration_quote` (§2.2), DP1-05's `identities`. This task adds **no engine change and no dw-app command**: it binds
`registration_quote` (generated already, E0-13) into the renderer's `Backend`, and reads the rest as the shell already
does. Copy follows the mobile wallets (iOS `JoinDashPayView.swift`, `JoinDashPayScreen.swift`, `VotingInfoScreen.swift`,
`ContactsScreen.swift` `DashPayFAQSheet` at `develop`; Android `strings-dashpay.xml`), adapted where the desktop differs
(D-rows). The registration wizard is DP1-09: every "create" or "continue" action ends at a ROADMAP §0 "not yet"
placeholder.

| Part | Where |
|---|---|
| View model (pure TypeScript): the card per state, the chip, the Join page's readiness, the copy | `apps/desktop/src/dashpay/status.ts` |
| Its tests, by parity ID (IOS-065, IOS-129, IOS-134; AND-024, 069–071) | `apps/desktop/src/dashpay/status.test.ts` |
| Queries (`registrations`, the quote; keys under `["dashpay", …]`, so `Platform` signals re-read them) | `apps/desktop/src/dashpay/queries.ts` |
| Home card, Join page, voting info and FAQ sheets | `src/screens/dashpay/{DashPayCard,JoinDashPay,AboutDashPay}.tsx` |
| Shared files (additive): Home slot, route, chip, `Backend.registrationQuote`, the fixture's quote and scenarios | `screens/Overview.tsx`, `app/App.tsx`, `screens/Routes.tsx`, `components/shell.tsx`, `api/{backend,tauri}.ts`, `fixture/{handlers,dashpay,scenarios}.ts` |
| Playwright (flows, axe; light and dark; Chromium and WebKit) | `e2e/dashpay.spec.ts`; baselines in `e2e/visual.spec.ts` |
| Real app on testnet (GL), reads only | `e2e-tauri/dashpay-status.ts`, `scripts/e2e-tauri-dashpay-status.sh` |

## Bindings

| Input | Call / signal | Used for |
|---|---|---|
| The banner state | `dashpay_status` (`["dashpay","status",w]`; re-read on `Platform{Startup,Registration,Identities,Names}`, on `sync` while `noIdentity` (BL-153 D9) and on the wallet's lifecycle events (D19)) | every surface |
| The main identity's names | `identities` (the shell's query) | `ready`: name, "No username", "Updating…" |
| The registration a `registering` status names | `registrations` (`["dashpay","registrations",w]`), the row whose `draft` the status names | step, label, wait, failure |
| The cost of a username | `registration_quote` for a fixed non-contested label (`["dashpay","quote",w]`, D6) | Join page: "about N Dash", the balance check |
| The wallet's spendable balance | `balance` (`confirmed`) | Join page: "You need at least N Dash" |
| The vault | `lock_status` | a registration waiting for an unlock: Unlock (mixing only) |
| Sync | `sync_status` | `waitingForSync`: the percent beside "Available after sync finishes" |

## State inventory

Each row is one rendered state of the Home card (`dashpay-card`, `data-state=<id>`) and its chip, with the copy. All
are baselined in light and dark (`e2e/visual.spec.ts` "dashpay-card-<id>"), on the fixture scenario named.

| # | Status (input) | Card `data-state` | Card title / message | Action | Chip | Scenario |
|---|---|---|---|---|---|---|
| S1 | read pending | none (no card) | — | — | hidden | — |
| S2 | read failed, a refetch after a read included (D24) | `error` | "Could not read DashPay's state" / "Your wallet works as usual." | Try again (refetch) | hidden | `dashpay-status-error` |
| S3 | `startupIncomplete{starting}` (and `ready`/`noIdentity` on a snapshot, BL-153 D16) | `looking` | "Looking for your DashPay identity…" (spinner) | — | hidden | `dashpay-starting` |
| S4 | `startupIncomplete{notRun}` | `notStarted` | "DashPay has not started" / "It starts when the wallet connects to the Dash network." | — | hidden | `dashpay-not-started` |
| S5 | `startupIncomplete{identityUnsettled}` | `unlockToLook` | "Unlock to look for your DashPay identity" / "DashPay looks for your identity once the wallet is fully unlocked." | Unlock (mixing only: the unlock sheet) | hidden | `dashpay-identity-unsettled` |
| S6 | `startupIncomplete{discoveryFailed, identityScanIncomplete, partialNoIdentity, partialAccountsPending}`; `seedBindingUnverified` | `lookFailed` | "Could not finish looking for your DashPay identity" / "Dash Wallet looks again the next time it starts."; for `seedBindingUnverified` (a restart does not fix it) "Could not confirm your DashPay identity" / "Its keys could not be checked against this wallet's recovery phrase." | — | hidden | `dashpay-look-failed` |
| S7 | `noIdentity{null}` | `join` | "Join DashPay" / "Request your username" (iOS) | Join DashPay → Join page | "Join DashPay" | `dashpay-no-identity` |
| S8 | `noIdentity{waitingForSync}` | `joinWaiting` | "Join DashPay" (secondary) / "Create a username, find your friends and add them to your contacts" + note "Available after sync finishes · N %" (iOS) | disabled | "Join DashPay" | `dashpay-waiting-for-sync` |
| S9 | `noIdentity{watchOnly}` | `watchOnly` | "DashPay needs this wallet's keys" / "A watch-only wallet cannot join DashPay." | — | hidden (D3) | `dashpay-watch-only` |
| S10 | `registering`, row unknown (not read yet, read failed, or no row by that draft) | `registering` | "Creating your username" / "Your username is being created on the Dash network. It may take some time." (Android) | — | "Registering…" | `dashpay-registering-unread` |
| S11 | `registering`, row running (`waiting` null, `instantSend`, `chainLock`, `network`, `sync`, `startup`) | `creating` | "Creating – @label" / step "(1/3) Processing payment", "(2/3) Creating identity", "(3/3) Registering username" (Android steps, D4) + progress bar; a wait adds its line ("Waiting for the network to confirm the payment", …) | — | "@label · Registering" | `dashpay-registering` |
| S12 | `registering`, `waiting: unlock` | `creatingUnlock` | step + "Unlock the wallet to continue" | Unlock (mixing only) | as S11 | `dashpay-registering-unlock` |
| S13 | `registering`, `waiting: authorize`, or a retryable `failure` | `creatingPaused` | step + "Registration paused" / the failure's reason (D5) | Continue → Join page | as S11 | `dashpay-registering-paused` |
| S14 | `registering`, `waiting: fundingUnknown` or `needsFinish` | `creatingRepair` | step + "Funds locked — finishing" (`needsFinish`) or "The payment may have been sent" (`fundingUnknown`) / "Open Tools ▸ Repair to finish the transfer." | Open Repair (Tools ▸ Repair) | as S11 | `tools-stuck-funding` |
| S15 | `contestPending{label, endsAt}` | `voting` | "Voting – @label" / "Masternode owners are deciding whether you get this username." + "Results on <date>" or "We will notify you when voting ends." (iOS) + "Until then you are @temp." when a temporary name shows | What is username voting? (sheet) | "@label · Pending" | `dashpay-contest-pending`, `dashpay-contest-pending-no-end` |
| S16 | `ready`, main identity with a name | none (D2) | — | — | avatar + "@name" | `dashpay-registered` |
| S17 | `ready`, main identity without a name, Platform said it owns none (live read, `namesCheck` null: D23) | `noUsername` | "Finish username registration" / "Your identity is ready. Use its existing credits to register a username." (iOS) | Choose a username → Join page | "No username" | `dashpay-no-username` |
| S18 | `ready`, names updating, not read yet, or not fetched yet (`namesCheck: pending`, D23) | none | — | — | "Updating…" | `dashpay-names-updating`, `dashpay-names-pending` |
| S18b | `ready`, no name and whether it owns one unknown (`namesCheck: failed`: the question failed, or a name not loaded yet; D23) | none | — | — | "Username not loaded" | `dashpay-names-unknown` |

Join page (`page-joinDashPay`): the intro (iOS `JoinDashPayScreen`: "Create username", the pitch, three features), then
a readiness block by status, then the links. Wizard and invitation entries end at placeholders (D7, D8).

| # | Status | Readiness block | Primary action | Scenario |
|---|---|---|---|---|
| J1 | `noIdentity{null}` | cost "A username costs about N Dash. Usernames the network votes on cost more; the exact cost is shown before you pay." (quote, D6); "You need at least N Dash to create a username." when the wallet's whole balance is below it (D18) | Create username → wizard placeholder | `dashpay-no-identity` |
| J2 | `noIdentity{waitingForSync}` | as J1 + "Available after sync finishes" | disabled | `dashpay-waiting-for-sync` |
| J3 | `noIdentity{watchOnly}` | "A watch-only wallet cannot join DashPay. Restore the wallet from its recovery phrase to join." | none | `dashpay-watch-only` |
| J4 | `startupIncomplete{…}` | S3–S6's card | no Create username (BL-153 D13: Join could start a second registration); the card's own button | `dashpay-starting` |
| J5 | `registering`, `contestPending`, `ready` | the Home card for that state (S10–S17) | the card's | as S10–S17 |
| J6 | read failed | S2 | Try again | `dashpay-no-identity` + `fail=dashpay_status:none` |
| J7 | quote failed or unread | the cost sentence without the figure: "Usernames the network votes on cost more than others. The exact cost is shown before you pay." | as J1 | `dashpay-no-identity` + `fail=registration_quote:none` |

Links on every Join page state: "What is username voting?" (voting info sheet) and "About DashPay" (FAQ sheet).

**Voting info sheet** (`voting-info`, IOS-134 / AND-071): "What is username voting?", "The Dash network has to vote to
approve some usernames before they are created", then Voting is only required in some cases (2–9 or 20+ characters);
Some usernames can be blocked; A second, instant username (F5's temporary name); You will be notified (the bell, DP2-05);
Keep your recovery phrase safe (D10); Masternode owners vote (voting from this wallet comes with MG-07, F21).

**FAQ sheet** (`dashpay-faq`, IOS-129): What is DashPay? · What does it cost? · How private is DashPay? · When will
contact requests become private? · How does this compare to Bitcoin? · … to Ethereum accounts? · … to Unstoppable
Domains? · What if I lose my recovery phrase? (D10, D11).

**Light and dark**: every card state, the chip states, the Join page (J1 with and without a shortfall, J2, J3, J5 voting), the voting info and the
FAQ sheet are baselined in both themes (Chromium, the pinned image); axe runs on each in both themes on Chromium and
WebKit (`e2e/dashpay.spec.ts`). Colours are the existing tokens only (`card-l1`, `accent` and `danger`
tints); no new token.

## Slice plan

1. This document.
2. The view model and its tests; `Backend.registrationQuote` and the fixture's handler; the queries.
3. The Home card and the chip (shell) on every state; fixture scenarios.
4. The Join page, the voting info and FAQ sheets; the wizard placeholder.
5. Playwright flows + axe, visual baselines (light, dark); the overview baselines re-blessed (D2).
6. The real-app GL script on testnet wallet A (reads only); `docs/parity.md` rows.

## D-rows

| # | Decision | Why |
|---|---|---|
| D1 | One pure function per surface over the same inputs: `homeCard(status, main, registration, lock)` → a card model or `null`, `chipState(...)` (BL-153's, extended), `joinReadiness(...)`. Copy lives in one table (`status.ts` `COPY`). | F1: one banner state machine for the Home card, the chip and the Contacts empty state; the copy is tested by state without React. |
| D2 | No Home card while the status is unread (S1), for a `ready` identity with a name (S16), or while its names update (S18). | The steady state is quiet (UX-SPEC §4.5 "quiet when fine"); the chip already names the user. A card while unread would flash on every launch. |
| D3 | The chip is hidden for `noIdentity{watchOnly}` (was "Join DashPay", BL-153). | A watch-only wallet cannot join; a chip inviting it to would lead to a page that refuses. The Home card says why (S9). |
| D4 | A registration's step comes from its row's phase: `draft`, `keysPrepared`, `fundingSent` → 1/3 "Processing payment"; `proofWaiting` → 2/3 "Creating identity"; `identityRegistered`, `nameRequested` → 3/3 "Registering username". Android's three steps; a contested name is the status's `contestPending`, not a step. | AND-024 / iOS `DWDPRegistrationStatus`; `registrations` gives the phase, the status only the draft. |
| D5 | A paused registration (S13) shows the failure's code as copy from a table (`registration.funding_insufficient` "Not enough Dash to pay for the username" with needed/available, `registration.islock_timeout`, `platform.insufficient_credits`, `platform.unavailable`, …), the code itself for an unknown one. Continue goes to the Join page, whose wizard (DP1-09) resumes it. | DP1-09 owns `resume_registration` and its grant; DP1-08 shows the state and the way in. |
| D6 | The Join page quotes a fixed, valid non-contested label (`dashpaydesktopquote2`, 20 characters, a digit 2–9) with Core funding. A non-contested name's cost does not depend on the label (`name_cost`: the fee bound, no vote fund), and the quote is local (no DAPI). A contested name's cost depends on its contenders (a network read), so the page says "cost more; the exact cost is shown before you pay" rather than a figure. A failed quote drops the figure (J7). | F1 "real costs from the quote", without asking for a label before the wizard. Android hard-codes 0.03/0.25 Dash; the engine's floors may change. |
| D7 | "Create username", "Continue" and "Choose a username" open the wizard's placeholder in the Join page ("Choosing and registering a username is not available yet.", `placeholder-registration`, with Back). | ROADMAP §0: the wizard is DP1-09. |
| D8 | The invitation note (U-01) stays on the Join page; no "Have an invitation?" entry. | DP5-01/DP5-03 own invitations. |
| D9 | No CoinJoin tip (Android `mix_dash_message`, "mix your balance before you create your username"). Revised after review: the desktop pays for a username from the send account's coins only (`CoreBalance` funding, `spendable_of`), so following the tip would move the funds out of reach of the registration. | Android funds the username from mixed coins; this engine does not. The tip returns if the funding learns to spend mixed coins. |
| D10 | "Passphrase" in the mobile copy becomes "recovery phrase". | On the desktop the passphrase locks the vault; the username right belongs to the seed (the recovery phrase). |
| D11 | The FAQ drops iOS's dated and Shielded promises ("September–October 2026", "Shielded DashPay") and its "Why do I need to enable it?" (that is F9's Contacts banner). "What's coming next?" is not carried. | DASHPAY §5: shielded is X2 (1.1); dates go stale; the FAQ is the Join page's. |
| D12 | The contested chip keeps BL-153's "@label · Pending" and still opens the Join page, which now shows the voting card (S15) and the voting info. DP1-10 moves it to My Profile with the profile sheet. | My Profile is a placeholder until DP1-10; the Join page now explains the state. |
| D13 | The Home card sits between the shortcut card and History (UX-SPEC §4.5's `SystemMessage` slot), full column width, `card-l1`. It is not dismissible yet (F1). | iOS lets the Home call to action be dismissed; that needs a persisted UI setting (`UiSettings` is engine-owned). |
| D14 | Contest outcomes (won, lost, blocked) are not a banner state: the status has no such variant (a lost contest is `ready`, S17 or S16), and the outcome is in the notification journal (DP2-05). | Not derivable from `status` (BL-153 item 49); the bell owns events. iOS's "Rejected – / Blocked –" rows are F2. |
| D15 | The `funded` scenario (no identity) now shows the Join card on Home: its overview baselines are re-blessed. | It is S7, the state every new wallet starts in. |
| D16 | The `recovery-phrase` (a sheet over the funded Home) and `forgot` baselines are re-blessed too. The lock screen covers a rendered shell, whose Home now holds the Join card; `forgot` moves 189 pixels of the sheet's text rasterisation (the same in every run), not its layout. | Both follow from D15; `txDetail` and the other baselines are unchanged. |
| D17 | The card's "Available after sync finishes · N%" uses the status bar's `syncPercent` and its one-decimal format, so the two agree. A sync a few blocks from the end reads "100.0%" in both; the fixture shows a mid-sync point (65%). | One percentage in the window; the rounding is the status bar's concern (a follow-up), not a banner state. |
| D18 | The Join page's shortfall compares the quote with the wallet's whole balance (`Balances.total`), and says nothing while the balance is still syncing in (J2). Revised after review: the engine pays from the send account's trusted, unlocked, unreserved coins (`spendable_of`), a figure the renderer cannot read; `Balances.confirmed` both counts coins it cannot spend (CoinJoin, locked, reserved) and leaves out trusted change it can. The total never claims a shortfall that is not there; the wizard (DP1-09) shows the plan's exact figure. | No engine change for an early hint. Follow-up: carry the plan's available amount in `RegistrationQuote`. |
| D19 | A registration's row is read again on every `vaultLockState` event. A flow that parks for an unlock or an authorization changes its `waiting` without writing its row, so no `Platform{registration}` signal says so; the lock events are when it parks. The username quote is read again on `balances` (its fee grows with the wallet's coins). | Derivable without an engine change. |
| D20 | In discreet mode the paused card's funding line leaves out the available amount ("… N needed."). | The Home balance is masked in discreet mode; the card must not show it either. |
| D21 | **Superseded by D23** (kept for the record). S17 waited until the main identity has read nameless for 10 s (`NAMELESS_HOLD_MS`), counted once per identity per session, so a revisit of Home does not restart it; until then the card and the chip read it as S18 (none, "Updating…"); the hold lives in the always-mounted shell hook (`useHeldNames`), so its per-identity marks are cleared when a name arrives. Found on testnet (GL, wallet A): after the bring-up the engine reported the identity as current with no names for 0.6 s before its names pass, so Home flashed "Finish username registration" on a wallet that has one. | The read model cannot tell "no username" from "names not fetched yet" in that window (`recovery.rs summaries`: `names_updating: !current`). Follow-up for the engine: report `names_updating` until the first names pass; the hold can then go. A truly nameless identity also reads "You have joined DashPay." on the Join page for those 10 s. |
| D22 | The gate's Playwright run (82f5e57c, load 140–160) timed out 8 onboarding axe walks (6 chromium, 2 webkit; each at a different step: the axe pass, a click, a text read). Rerun alone: onboarding.spec 46/46. Recorded as a load flake. | The rest of that run passed (764); no DP1-08 change touches onboarding beyond the Home rendered under the lock screen (D16). |
| D23 | **Amends D21 (Sol r1 finding 1).** The engine says whether an identity's names are known: `IdentitySummary.namesCheck` (`pending`, `failed`, null; m4 contract item 51, version 25). It is null only for an identity with a name or one Platform said this session owns no DPNS name: the engine asks Platform per identity that shows no name (one read-only query, 10 s) when the names pass a restore or a discovery owes ends, when each bring-up ends, and every minute with the balance pass while unanswered. `pending` until it answers (and while a names pass is owed); `failed` when the question failed or Platform said the identity owns a name not loaded yet. S17 and "No username" need it null; `pending` reads as S18 ("Updating…"); `failed` as S18b, neutral ("Username not loaded"), never "No username" nor S17. The 10-second hold (`NAMELESS_HOLD_MS`, `useHeldNames`) is gone. `my_user_link` is `platform.unavailable` while it is set. | No clock on the UI side is evidence that names were fetched (the first names pass has a 20 s budget, a restart can cut it off), and no pass outcome is either: the library's pass skips an identity whose names it could not fetch and still succeeds (Opus delta M1), and an identity stored without a pass (on file, an unlock, a rerun) was never fetched (M2). Only Platform's per-identity answer is. Answers last for the session: Close/Open asks again. With Platform off (`--no-platform`), or SPV stopped before the bring-up, nothing is asked and the chip stays "Updating…"; offline, the question fails and the chip says "Username not loaded" until Platform answers. A nameless identity's S17 waits for its answer, a second or two after the bring-up. |
| D24 | **Sol r1 finding 2.** A status read that fails is an error even with an older answer cached (TanStack keeps `data` on a failed refetch): Home and the Join page show S2 (no Join, no Create username), the chip and the sidebar's Join go. A failed identities read hides the main identity (the chip says "Updating…", no S17). | The cached answer may be the very state that changed (a restore turning `noIdentity` into `ready`); an affirmative action must not rest on it. |
| D25 | **Sol r1 finding 3.** The card's polite live region (`role="status"`) holds its title (the username), its step and its lines. | A registration's username and stage are the news when it arrives or advances. Same layout: the region takes the body's column and gap. |

## Deferred

| # | What | Owner |
|---|---|---|
| F1 | Dismiss the Home call to action (iOS `onDismiss`), persisted per wallet | a settings task (needs a `UiSettings` field) |
| F2 | iOS's outcome rows ("Rejected – @x", "Blocked – @x", "Your username has been successfully created") from the journal | DP1-10 (username request status) |
| F3 | Wizard, resume, invitation and "Have an invitation?" | DP1-09, DP5-03 |
| F4 | The Contacts empty state reuses `homeCard` copy (it keeps DP2-07's own copy today, which agrees) | DP2-09 or later, to avoid a shared-file clash now |
